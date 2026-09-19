// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Prompt line command execution, search dispatch, and macro handling.

use super::actions::execute_action;
use super::dispatch::{NavMotion, dispatch_nav_motion, sync_split_views_after_motion};
use super::layout::ViewKind;
use super::{AppState, Flow};
use crate::keymap::{RunCommand, RunFlags};
use crate::prompt::{ParsedCommand, PromptKind, PromptState};
use crate::search::{ActiveSearch, SearchDirection, SearchPattern, SearchResult};
use crate::view::GrepMatch;
use std::sync::Arc;
use tigrs_core::macro_ctx::MacroContext;

const MAX_GREP_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_GREP_MATCHES: usize = 20_000;

/// Runs a sandboxed `git grep` in `work_dir` and returns sanitized [`GrepMatch`] entries.
pub fn run_git_grep(
    pattern: &str,
    work_dir: Option<&std::path::Path>,
) -> tigrs_core::error::Result<Vec<GrepMatch>> {
    use std::io::Read as _;

    let dir = work_dir.unwrap_or_else(|| std::path::Path::new("."));
    let mut cmd = tigrs_git::safe_git_command(dir);
    cmd.args([
        "grep",
        "--no-textconv",
        "-n",
        "-I",
        "-z",
        "-e",
        pattern,
        "--",
    ]);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| tigrs_core::error::TigError::Git(format!("Failed to run git grep: {e}")))?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let (mut raw_stdout, raw_stderr) = std::thread::scope(|s| {
        let stderr_handle = s.spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut err) = stderr_pipe {
                let mut bounded = (&mut err).take(4096);
                let _ = bounded.read_to_end(&mut buf);
                let _ = std::io::copy(&mut err, &mut std::io::sink());
            }
            buf
        });

        let mut out_buf = Vec::new();
        if let Some(mut out) = stdout_pipe {
            let mut bounded = (&mut out).take((MAX_GREP_OUTPUT_BYTES + 1) as u64);
            let _ = bounded.read_to_end(&mut out_buf);
        }
        if out_buf.len() > MAX_GREP_OUTPUT_BYTES {
            let _ = child.kill();
        }
        let err_buf = stderr_handle.join().unwrap_or_default();
        (out_buf, err_buf)
    });

    let truncated_output = raw_stdout.len() > MAX_GREP_OUTPUT_BYTES;
    if truncated_output {
        raw_stdout.truncate(MAX_GREP_OUTPUT_BYTES);
    }

    let status = child.wait().map_err(|e| {
        tigrs_core::error::TigError::Git(format!("Failed to wait on git grep: {e}"))
    })?;

    if !truncated_output && !status.success() && status.code() != Some(1) {
        let stderr =
            tigrs_core::strip_control_chars(&String::from_utf8_lossy(&raw_stderr)).into_owned();
        return Err(tigrs_core::error::TigError::Git(format!(
            "git grep failed: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&raw_stdout);
    Ok(parse_git_grep_output(&stdout))
}

/// Parses `git grep -n -z` (or colon-delimited `git grep -n`) output into sanitized [`GrepMatch`] entries.
#[must_use]
pub fn parse_git_grep_output(stdout: &str) -> Vec<GrepMatch> {
    let mut matches = Vec::new();
    if stdout.contains('\0') {
        let mut rest = stdout;
        while !rest.is_empty() && matches.len() < MAX_GREP_MATCHES {
            let Some((path, after_path)) = rest.split_once('\0') else {
                break;
            };
            let Some((line_num_str, after_num)) = after_path.split_once('\0') else {
                break;
            };
            let (content_raw, next_rest) = match after_num.split_once('\n') {
                Some((c, r)) => (c, r),
                None => (after_num, ""),
            };
            rest = next_rest;
            let content = content_raw.strip_suffix('\r').unwrap_or(content_raw);
            if let Ok(line_num) = line_num_str.parse::<usize>()
                && !path.is_empty()
            {
                matches.push(GrepMatch {
                    path: tigrs_core::strip_control_chars(path).replace(['\n', '\r'], " "),
                    line_num,
                    content: tigrs_core::strip_control_chars(content).replace(['\n', '\r'], " "),
                });
            }
        }
        return matches;
    }

    for line in stdout.lines() {
        if matches.len() >= MAX_GREP_MATCHES {
            break;
        }
        let mut parts = line.splitn(3, ':');
        let Some(p) = parts.next() else { continue };
        let Some(ln) = parts.next().and_then(|s| s.parse::<usize>().ok()) else {
            continue;
        };
        let c = parts.next().unwrap_or_default();
        if !p.is_empty() {
            matches.push(GrepMatch {
                path: tigrs_core::strip_control_chars(p).replace(['\n', '\r'], " "),
                line_num: ln,
                content: tigrs_core::strip_control_chars(c).replace(['\n', '\r'], " "),
            });
        }
    }
    matches
}

/// Executes a parsed prompt command line on the application state.
pub fn execute_parsed_command(
    app: &mut AppState,
    cmd: ParsedCommand,
    visible_height: usize,
) -> Flow {
    match cmd {
        ParsedCommand::Empty => Flow::Continue,

        ParsedCommand::LineNumber(n) => {
            let target_line = n.saturating_sub(1);
            dispatch_nav_motion(app, NavMotion::SetCursor(target_line), visible_height);
            Flow::Continue
        }

        ParsedCommand::Goto(target) => {
            if let Some(ref engine) = app.engine
                && let Ok(oid) = engine.resolve_revision(&target)
                && let Some(ref mut main) = app.views.main_view
                && let Some(row) = main.row_for_commit_id(&oid)
            {
                main.set_cursor(row, visible_height);
                return Flow::Continue;
            }
            // Fallback: zero-allocation nibble prefix match on commit ID in MainView
            if let Some(ref mut main) = app.views.main_view
                && let Some(row) = main.find_row_by_hex_prefix(&target)
            {
                main.set_cursor(row, visible_height);
                return Flow::Continue;
            }
            app.status_message = Some(format!("Cannot resolve revision: '{target}'"));
            Flow::Continue
        }

        ParsedCommand::Grep(pattern) => {
            let work_dir = app.engine.as_ref().map(tigrs_git::GitEngine::work_dir);
            match run_git_grep(&pattern, work_dir) {
                Ok(matches) => {
                    let count = matches.len();
                    let gv = crate::view::GrepView::new(pattern.clone(), matches);
                    app.views.grep_view = Some(gv);
                    app.push_view(ViewKind::Grep);
                    app.status_message =
                        Some(format!("Found {count} grep matches for '{pattern}'"));
                }
                Err(err) => {
                    app.status_message = Some(format!("Grep failed: {err}"));
                }
            }
            Flow::Continue
        }

        ParsedCommand::Shell {
            command,
            foreground,
        } => {
            if app.check_read_only_blocked("shell command") {
                return Flow::Continue;
            }
            let flags = if foreground {
                RunFlags::empty()
            } else {
                RunFlags::ECHO
            };
            let cmd_obj = std::sync::Arc::new(RunCommand { flags, command });
            super::handle_run_command_action(app, &cmd_obj)
        }

        ParsedCommand::Builtin(action) => execute_action(app, &action, visible_height),

        ParsedCommand::Toggle(name) => {
            app.toggle_option(&name);
            Flow::Continue
        }

        ParsedCommand::Set { variable, value } => {
            match app.options.set_by_name(&variable, &value) {
                Ok((msg, effect)) => {
                    app.sync_read_only_state();
                    app.invalidate_screen();
                    app.status_message = Some(msg);
                    app.apply_option_effect(effect);
                }
                Err(err) => {
                    app.status_message = Some(err);
                }
            }
            Flow::Continue
        }

        ParsedCommand::SaveConfig { path, minimal } => {
            app.save_config_to_toml(path.as_deref(), minimal);
            Flow::Continue
        }

        ParsedCommand::SourceConfig(path) => {
            app.source_config_from_toml(&path);
            Flow::Continue
        }

        ParsedCommand::Search { query, backward } => {
            let dir = if backward {
                SearchDirection::Backward
            } else {
                SearchDirection::Forward
            };
            execute_search(app, &query, dir, visible_height);
            Flow::Continue
        }

        ParsedCommand::Echo(msg) | ParsedCommand::Unknown(msg) => {
            app.status_message = Some(msg);
            Flow::Continue
        }
    }
}

/// Executes an external shell or internal command.
pub fn execute_run_command(app: &mut AppState, command_line: &str, cmd: &RunCommand) -> Flow {
    if cmd.flags.contains(RunFlags::INTERNAL) {
        let parsed = ParsedCommand::parse(command_line);
        return execute_parsed_command(app, parsed, 24);
    }

    if app.check_read_only_blocked("external command") {
        return Flow::Continue;
    }

    if let Err(err) = tigrs_core::verify_shell_safety(command_line) {
        app.status_message = Some(format!("Invalid command: {err}"));
        return Flow::Continue;
    }

    if cmd.flags.contains(RunFlags::SILENT) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut child_cmd = std::process::Command::new(&shell);
        child_cmd
            .arg("-c")
            .arg(command_line)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(work_dir) = app.engine.as_ref().map(|e| e.work_dir().to_path_buf()) {
            child_cmd.current_dir(&work_dir);
            if !app.is_current_repo_trusted(&work_dir) {
                tigrs_git::apply_untrusted_repo_env(&mut child_cmd, &work_dir);
            }
        }
        match child_cmd.spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                app.status_message = Some("Command started in background".to_string());
            }
            Err(err) => {
                app.status_message = Some(format!("Failed to start command: {err}"));
            }
        }
    } else if cmd.flags.contains(RunFlags::ECHO) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut child_cmd = std::process::Command::new(&shell);
        child_cmd
            .arg("-c")
            .arg(command_line)
            .stdin(std::process::Stdio::null());
        if let Some(work_dir) = app.engine.as_ref().map(|e| e.work_dir().to_path_buf()) {
            child_cmd.current_dir(&work_dir);
            if !app.is_current_repo_trusted(&work_dir) {
                tigrs_git::apply_untrusted_repo_env(&mut child_cmd, &work_dir);
            }
        }
        let output = child_cmd.output();
        match output {
            Ok(out) => {
                if out.status.success() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let msg = stdout.lines().next().unwrap_or("Done");
                    app.status_message = Some(tigrs_core::strip_control_chars(msg).into_owned());
                } else {
                    let err = String::from_utf8_lossy(&out.stderr);
                    let msg = err.lines().next().unwrap_or("Command failed");
                    app.status_message = Some(tigrs_core::strip_control_chars(msg).into_owned());
                }
            }
            Err(e) => {
                app.status_message = Some(format!("Failed to execute command: {e}"));
            }
        }
    } else {
        // Foreground execution requiring TTY handover
        app.pending_handover = Some((command_line.to_string(), cmd.clone()));
    }

    if cmd.flags.contains(RunFlags::EXIT) {
        Flow::Quit
    } else {
        Flow::Continue
    }
}

/// Searches the active view for a query string in the given direction and moves the cursor to the first match.
pub fn execute_search(
    app: &mut AppState,
    query: &str,
    direction: SearchDirection,
    visible_height: usize,
) {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return;
    }

    let pattern = SearchPattern::new(trimmed, None);
    app.active_search = Some(ActiveSearch {
        pattern: pattern.clone(),
        direction,
    });

    execute_search_with_pattern(app, &pattern, direction, visible_height);
}

const MAX_SEARCH_SCAN_ITEMS: usize = 500_000;

/// Executes a search with a precompiled [`SearchPattern`] in the given direction.
pub fn execute_search_with_pattern(
    app: &mut AppState,
    pattern: &SearchPattern,
    direction: SearchDirection,
    visible_height: usize,
) {
    let (_search_source, search_cancel) = tigrs_core::cancel::CancellationToken::new();
    let result = if let Some(kind) = app.active_view() {
        let search_res = if let Some(view) = app.view_ref(kind) {
            let total = view.line_count().min(MAX_SEARCH_SCAN_ITEMS);
            let start = view.cursor().min(total.saturating_sub(1));
            crate::search::search_items(
                total,
                start,
                direction,
                pattern,
                Some(&search_cancel),
                |idx, pat| view.matches_search(idx, pat),
            )
        } else {
            SearchResult::NotFound
        };
        if let SearchResult::Found { index, .. } = search_res
            && let Some(view) = app.view_mut(kind)
        {
            view.set_cursor(index, visible_height);
        }
        search_res
    } else {
        SearchResult::NotFound
    };

    match result {
        SearchResult::Found { wrapped: true, .. } => {
            app.status_message = Some(if direction.is_forward() {
                "Search wrapped to top".to_string()
            } else {
                "Search wrapped to bottom".to_string()
            });
        }
        SearchResult::Found { wrapped: false, .. } => {
            app.status_message = None;
        }
        SearchResult::NotFound => {
            app.status_message = Some(format!(
                "Pattern not found: '{}'",
                tigrs_core::strip_control_chars(pattern.raw())
            ));
        }
        SearchResult::Cancelled => {
            app.status_message = Some("Search cancelled".to_string());
        }
    }

    if matches!(result, SearchResult::Found { .. }) {
        sync_split_views_after_motion(app);
    }
}

/// Handles user input submission for interactive macro prompt (`%(prompt)`).
pub(crate) fn handle_interactive_macro_submit(
    app: &mut AppState,
    template: &str,
    mut answers: Vec<String>,
    user_input: &str,
    run_cmd: Option<&RunCommand>,
) -> Flow {
    answers.push(user_input.to_string());
    let labels = MacroContext::extract_prompt_labels(template);

    if answers.len() < labels.len() {
        let next_label = tigrs_core::strip_control_chars(&labels[answers.len()]);
        app.prompt = Some(PromptState::new(PromptKind::InteractiveMacro {
            label: if next_label.is_empty() {
                ": ".to_string()
            } else if next_label.ends_with(':') {
                format!("{next_label} ")
            } else {
                format!("{next_label}: ")
            },
            template: template.to_string(),
            answers,
            run_command: run_cmd.map(|c| Arc::new(c.clone())),
        }));
        Flow::Continue
    } else {
        let ctx = app.macro_context();
        let mut answers_iter = answers.into_iter();
        match ctx.expand(template, |_| answers_iter.next()) {
            Ok(expanded) => {
                let safe_expanded = tigrs_core::strip_control_chars(&expanded);
                let default_cmd = RunCommand {
                    flags: RunFlags::default(),
                    command: expanded.clone(),
                };
                let cmd = run_cmd.unwrap_or(&default_cmd);
                if cmd.flags.contains(RunFlags::CONFIRM) {
                    app.prompt = Some(PromptState::new(PromptKind::Confirm {
                        message: format!("Run: {safe_expanded}? [y/N]"),
                        expanded_command: expanded,
                        run_command: Arc::new(cmd.clone()),
                    }));
                    Flow::Continue
                } else {
                    execute_run_command(app, &expanded, cmd)
                }
            }
            Err(e) => {
                app.status_message =
                    Some(tigrs_core::strip_control_chars(&e.to_string()).into_owned());
                Flow::Continue
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_parsed_command_core_variants() {
        let mut app = AppState::default();
        app.views.view_stack.push(ViewKind::Main);

        // Empty command continues cleanly
        assert_eq!(
            execute_parsed_command(&mut app, ParsedCommand::Empty, 20),
            Flow::Continue
        );

        // Echo command updates status message
        assert_eq!(
            execute_parsed_command(
                &mut app,
                ParsedCommand::Echo("Hello Linux SQE".to_string()),
                20
            ),
            Flow::Continue
        );
        assert_eq!(app.status_message.as_deref(), Some("Hello Linux SQE"));

        // Goto unknown revision sets error message
        assert_eq!(
            execute_parsed_command(&mut app, ParsedCommand::Goto("deadbeef999".to_string()), 20),
            Flow::Continue
        );
        assert!(
            app.status_message
                .as_deref()
                .unwrap()
                .contains("Cannot resolve revision")
        );

        // Toggle option via ParsedCommand::parse
        let orig_ln = app.options.line_number;
        execute_parsed_command(&mut app, ParsedCommand::parse("toggle line-number"), 20);
        assert_eq!(app.options.line_number, !orig_ln);

        // Set option via ParsedCommand::parse
        execute_parsed_command(&mut app, ParsedCommand::parse("set tab-size = 4"), 20);
        assert_eq!(app.options.tab_size, 4);

        // Quit command returns Flow::Quit
        assert_eq!(
            execute_parsed_command(&mut app, ParsedCommand::parse("quit"), 20),
            Flow::Quit
        );
    }
}
