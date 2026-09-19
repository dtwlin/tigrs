// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Command prompt, search, external execution, and editor invocation action handlers.

use crate::app::commands::{execute_run_command, execute_search_with_pattern};
use crate::app::{AppState, Flow};
use crate::keymap::{RunCommand, RunFlags};
use crate::prompt::{PromptKind, PromptState};
use std::sync::Arc;
use tigrs_core::macro_ctx::MacroContext;

/// Handles the `Prompt` action: opens the `:` ex command prompt.
pub fn handle_prompt(app: &mut AppState) -> Flow {
    app.prompt = Some(PromptState::new(PromptKind::Command));
    Flow::Continue
}

/// Handles the `Search` action: opens the `/` forward search prompt.
pub fn handle_search(app: &mut AppState) -> Flow {
    app.prompt = Some(PromptState::new(PromptKind::SearchForward));
    Flow::Continue
}

/// Handles the `SearchBack` action: opens the `?` backward search prompt.
pub fn handle_search_back(app: &mut AppState) -> Flow {
    app.prompt = Some(PromptState::new(PromptKind::SearchBackward));
    Flow::Continue
}

/// Handles the `Run` action: executes a user-defined external command or macro.
pub fn handle_run_command_action(app: &mut AppState, cmd: &Arc<RunCommand>) -> Flow {
    let ctx = app.macro_context();
    if MacroContext::has_prompt_token(&cmd.command) {
        let labels = MacroContext::extract_prompt_labels(&cmd.command);
        let first_label = labels.first().cloned().unwrap_or_default();
        app.prompt = Some(PromptState::new(PromptKind::InteractiveMacro {
            label: if first_label.is_empty() {
                ": ".to_string()
            } else if first_label.ends_with(':') {
                format!("{first_label} ")
            } else {
                format!("{first_label}: ")
            },
            template: cmd.command.clone(),
            answers: Vec::new(),
            run_command: Some(Arc::clone(cmd)),
        }));
        Flow::Continue
    } else {
        match ctx.expand(&cmd.command, |_| None) {
            Ok(expanded) => {
                if cmd.flags.contains(RunFlags::CONFIRM) {
                    app.prompt = Some(PromptState::new(PromptKind::Confirm {
                        message: format!("Run: {expanded}? [y/N]"),
                        expanded_command: expanded,
                        run_command: Arc::clone(cmd),
                    }));
                    Flow::Continue
                } else {
                    execute_run_command(app, &expanded, cmd)
                }
            }
            Err(err) => {
                app.status_message = Some(err.to_string());
                Flow::Continue
            }
        }
    }
}

/// Handles the `FindNext` action: repeats search in current direction.
pub fn handle_find_next(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(search) = app.active_search.clone() {
        execute_search_with_pattern(app, &search.pattern, search.direction, visible_height);
    } else {
        app.status_message = Some("No previous search query".to_string());
    }
    Flow::Continue
}

/// Handles the `FindPrev` action: repeats search in reverse direction.
pub fn handle_find_prev(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(search) = app.active_search.clone() {
        execute_search_with_pattern(
            app,
            &search.pattern,
            search.direction.reverse(),
            visible_height,
        );
    } else {
        app.status_message = Some("No previous search query".to_string());
    }
    Flow::Continue
}

/// Handles the `Edit` action: opens the file and line at cursor in `$EDITOR`.
pub fn handle_edit(app: &mut AppState) -> Flow {
    if let Some(ref engine) = app.engine {
        if engine.info().is_bare {
            app.status_message = Some("Nothing to edit".to_string());
            return Flow::Continue;
        }
        if app.core_editor.is_none() {
            let is_trusted = app.is_current_repo_trusted(engine.work_dir());
            app.core_editor = engine.core_editor(is_trusted);
        }
    }
    match crate::editor::resolve_edit_target(app) {
        Ok(target) => {
            let editor = crate::editor::resolve_editor(
                &|k| std::env::var(k).ok(),
                app.core_editor.as_deref(),
            );
            let line_numbers_enabled =
                app.options.editor_line_number && !app.editor_line_number_disabled;
            let used_line_number = target.line.is_some() && line_numbers_enabled;
            let command_line =
                crate::editor::build_editor_command_line(&editor, &target, line_numbers_enabled);
            app.pending_editor = Some(crate::editor::EditorInvocation {
                command_line,
                target,
                used_line_number,
            });
        }
        Err(refusal) => {
            app.status_message = Some(refusal.message());
        }
    }
    Flow::Continue
}
