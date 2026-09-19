// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! tigrs CLI entry point.

#![forbid(unsafe_code)]

#[cfg(feature = "system-alloc")]
#[global_allocator]
static GLOBAL: std::alloc::System = std::alloc::System;

mod args;

use crate::args::CliArgs;
use clap::Parser;
use crossbeam_channel::{SendTimeoutError, bounded};
use gix::ObjectId;
use rustix::termios::isatty;
use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tigrs_core::ansi::filter_sgr_only;
use tigrs_core::cancel::CancellationToken;
use tigrs_core::error::TigError;
use tigrs_git::{CommitSummary, GitEngine};
#[cfg(test)]
use tigrs_ui::parse_git_grep_output;
use tigrs_ui::{
    BlameView, BlobView, DiffView, GrepMatch, GrepView, LogView, MainView, ReflogView, RefsView,
    StashView, StatusView, TreeView, run_app, run_blame_app, run_blob_app, run_diff_app,
    run_git_grep, run_grep_app, run_log_app, run_reflog_app, run_refs_app, run_stash_app,
    run_status_app, run_tree_app,
};

fn run_cli_grep(
    pattern: &str,
    work_dir: Option<&std::path::Path>,
) -> Result<Vec<GrepMatch>, String> {
    run_git_grep(pattern, work_dir).map_err(|e| e.to_string())
}

/// Number of commit batches that may be in flight between the revwalk worker
/// and the render loop before the worker throttles itself.
const CHANNEL_DEPTH: usize = 64;

/// How long a blocked send waits before re-checking the cancellation flag.
const SEND_POLL_INTERVAL: Duration = Duration::from_millis(50);

fn main() -> ! {
    // Install panic hook immediately so terminal raw mode and alternate screen
    // are cleanly restored on any unexpected panic or abort.
    tigrs_ui::install_panic_hook();

    let args = CliArgs::parse();

    // Version display subcommand: `tigrs version`
    if args.subcommand == "version" {
        println!("tigrs {}", tigrs_core::APP_VERSION);
        std::process::exit(0);
    }

    // Shell completions generation: `tigrs completions [shell]`
    if args.subcommand == "completions" {
        std::process::exit(run_completions(&args));
    }

    // UNIX man page generation: `tigrs man`
    if args.subcommand == "man" {
        std::process::exit(run_man());
    }

    if args.debug_frame_stats {
        tigrs_ui::set_debug_frame_stats(true);
    }

    // Dual-Stream TTY: a non-terminal stdin with default arguments means
    // we were handed piped data to page (e.g. `git diff | tigrs`) rather than
    // an explicit repository view to browse, matching upstream Tig.
    let is_piped =
        !isatty(std::io::stdin()) && args.subcommand == "log" && args.rev_args.is_empty();
    let code = if is_piped {
        run_piped_mode()
    } else {
        run_repository_mode(&args)
    };

    // The terminal has already been restored by `TtyController`'s Drop impl at
    // this point. If the run loop exited cleanly due to a POSIX termination
    // signal (`SIGINT`/`SIGTERM`/`SIGHUP`), exit with `128 + signo` so shell
    // loops and test harnesses see the signal termination.
    if code == 0
        && let Some(sig) = tigrs_ui::take_last_exit_signal()
    {
        std::process::exit(128 + sig);
    }

    // Exiting directly skips tearing down the loaded history, which
    // can be hundreds of megabytes of individually-freed allocations on large
    // repositories; that teardown is pure latency the user would sit through
    // after they already pressed 'q'.
    std::process::exit(code);
}

/// Browses a Git repository interactively.
fn run_repository_mode(args: &CliArgs) -> i32 {
    let mut config = if let Some(ref path) = args.config {
        match tigrs_core::Config::load_from_file(path) {
            Ok(cfg) => cfg,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        }
    } else {
        tigrs_core::Config::load_default()
    };

    let engine = match GitEngine::open(args.directory.as_deref()) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("tigrs: {err}");
            return 1;
        }
    };

    // Launch in Update Mode by default (`config.general.read_only = false`),
    // allowing `--read-only` to enable Read-Only Mode or `--update-mode` to override `config.toml`.
    if args.read_only {
        config.general.read_only = true;
        config.cli_read_only_override = Some(true);
    } else if args.update_mode {
        config.general.read_only = false;
        config.cli_read_only_override = Some(false);
    }
    engine.set_read_only(config.general.read_only);

    // Direct status view mode: `tigrs status`
    if args.subcommand == "status" {
        let (cancel_src, token) = CancellationToken::new();
        let report = match engine.load_status(&token) {
            Ok(report) => report,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };

        let view = StatusView::new(report);
        let result = run_status_app(view, &cancel_src, Some(engine), Some(&config));
        return match result {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tigrs: {err}");
                1
            }
        };
    }

    // Direct diff view mode: `tigrs show [rev]`
    if args.subcommand == "show" {
        let rev_target = args.rev_args.first().map_or("HEAD", String::as_str);
        let commit_id = match engine.resolve_revision(rev_target) {
            Ok(id) => id,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };

        let diff = match engine.compute_commit_diff(commit_id) {
            Ok(diff) => diff,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };

        let (cancel_src, _token) = CancellationToken::new();
        let view = DiffView::new_deferred(diff);
        let result = run_diff_app(view, &cancel_src, Some(engine), Some(&config));
        return match result {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tigrs: {err}");
                1
            }
        };
    }

    // Direct tree view mode: `tigrs tree [rev] [path]`
    if args.subcommand == "tree" {
        let (rev_target, path_target) = if args.rev_args.len() >= 2 {
            (args.rev_args[0].as_str(), args.rev_args[1].as_str())
        } else if let Some(candidate) = args.rev_args.first() {
            if engine.resolve_revision(candidate).is_ok() {
                (candidate.as_str(), "")
            } else {
                ("HEAD", candidate.as_str())
            }
        } else {
            ("HEAD", "")
        };
        let commit_id = match engine.resolve_revision(rev_target) {
            Ok(id) => id,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };
        let listing = match engine.read_tree(commit_id, path_target) {
            Ok(listing) => listing,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };

        let (cancel_src, _token) = CancellationToken::new();
        let view = TreeView::new(listing);
        return match run_tree_app(view, &cancel_src, Some(engine), Some(&config)) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tigrs: {err}");
                1
            }
        };
    }

    // Direct blob view mode: `tigrs blob [rev] <path>`
    if args.subcommand == "blob" {
        let (rev_target, path_target) = if args.rev_args.len() >= 2 {
            (args.rev_args[0].as_str(), args.rev_args[1].as_str())
        } else if let Some(path) = args.rev_args.first() {
            ("HEAD", path.as_str())
        } else {
            eprintln!("tigrs: missing file path for blob view");
            return 1;
        };
        let commit_id = match engine.resolve_revision(rev_target) {
            Ok(id) => id,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };
        let blob = match engine.read_blob_at_commit_path(commit_id, path_target) {
            Ok(blob) => blob,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };

        let (cancel_src, _token) = CancellationToken::new();
        let mut opts = tigrs_ui::ViewOptions::default();
        opts.apply_config(&config);
        let view = BlobView::new_with_options(commit_id, blob, &opts);
        return match run_blob_app(view, &cancel_src, Some(engine), Some(&config)) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tigrs: {err}");
                1
            }
        };
    }

    // Direct blame view mode: `tigrs blame [rev] <path>`
    if args.subcommand == "blame" {
        let (rev_target, path_target) = if args.rev_args.len() >= 2 {
            (args.rev_args[0].as_str(), args.rev_args[1].as_str())
        } else if let Some(path) = args.rev_args.first() {
            ("HEAD", path.as_str())
        } else {
            eprintln!("tigrs: missing file path for blame view");
            return 1;
        };

        let commit_id = match engine.resolve_revision(rev_target) {
            Ok(id) => id,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };

        let blob = match engine.read_blob_at_commit_path(commit_id, path_target) {
            Ok(blob) => blob,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };

        let (cancel_src, _token) = CancellationToken::new();
        let view = BlameView::from_blob(commit_id, blob);
        return match run_blame_app(view, &cancel_src, Some(engine), Some(&config)) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tigrs: {err}");
                1
            }
        };
    }

    // Direct refs view mode: `tigrs refs`
    if args.subcommand == "refs" {
        let refs = match engine.list_refs() {
            Ok(refs) => refs,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };
        let (cancel_src, _token) = CancellationToken::new();
        let view = RefsView::new(refs);
        return match run_refs_app(view, &cancel_src, Some(engine), Some(&config)) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tigrs: {err}");
                1
            }
        };
    }

    // Direct stash view mode: `tigrs stash`
    if args.subcommand == "stash" {
        let stashes = match engine.list_stashes() {
            Ok(stashes) => stashes,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };
        let (cancel_src, _token) = CancellationToken::new();
        let view = StashView::new(stashes);
        return match run_stash_app(view, &cancel_src, Some(engine), Some(&config)) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tigrs: {err}");
                1
            }
        };
    }

    // Direct reflog view mode: `tigrs reflog [ref]`
    if args.subcommand == "reflog" {
        let target_ref = args.rev_args.first().map_or("HEAD", String::as_str);
        let entries = match engine.read_reflog(target_ref) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };
        let (cancel_src, _token) = CancellationToken::new();
        let view = ReflogView::new(target_ref.to_string(), entries);
        return match run_reflog_app(view, &cancel_src, Some(engine), Some(&config)) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tigrs: {err}");
                1
            }
        };
    }

    // Direct grep view mode: `tigrs grep [pattern]`
    if args.subcommand == "grep" {
        let pattern = args.rev_args.first().cloned().unwrap_or_default();
        let matches = match run_cli_grep(&pattern, engine.info().work_dir.as_deref()) {
            Ok(m) => m,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };
        let (cancel_src, _token) = CancellationToken::new();
        let view = GrepView::new(pattern, matches);
        return match run_grep_app(view, &cancel_src, Some(engine), Some(&config)) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tigrs: {err}");
                1
            }
        };
    }

    // Direct log view mode: `tigrs log [rev_args...]`
    if args.subcommand == "log" && CliArgs::is_explicit_subcommand_from(std::env::args_os()) {
        let mut raw_args = Vec::new();
        raw_args.extend(args.rev_args.iter().cloned());

        let spec = match engine.parse_rev_args(&raw_args) {
            Ok(spec) => spec,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };
        let spec = match engine.resolve_spec_tips(spec) {
            Ok(spec) => spec,
            Err(err) => {
                eprintln!("tigrs: {err}");
                return 1;
            }
        };

        let branch_name = if raw_args.is_empty() {
            engine
                .current_branch()
                .unwrap_or_else(|_| "HEAD".to_string())
        } else {
            raw_args.join(" ")
        };

        let (cancel_src, token) = CancellationToken::new();
        let mut log_view = LogView::new(branch_name);

        let refs_list = engine.list_refs().ok().unwrap_or_default();
        let mut refs_by_commit: std::collections::HashMap<ObjectId, Vec<String>> =
            std::collections::HashMap::new();
        for r in refs_list {
            refs_by_commit.entry(r.commit_id).or_default().push(r.name);
        }
        let mut loaded = 0usize;
        if let Ok(stream) = engine.stream_commits_spec(spec, Some(20), token) {
            for batch in stream {
                let Ok(commits) = batch else { break };
                for c in commits {
                    if loaded >= 20 {
                        break;
                    }
                    let ref_names = refs_by_commit.get(&c.id).map(Vec::as_slice);
                    if let Ok(diff) = engine.compute_commit_diff_cached(c.id) {
                        log_view.append_commit_diff(&diff, ref_names);
                        loaded += 1;
                    }
                }
                if loaded >= 20 {
                    break;
                }
            }
        }

        return match run_log_app(log_view, &cancel_src, Some(engine), Some(&config)) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tigrs: {err}");
                1
            }
        };
    }

    let mut raw_args = Vec::new();
    if args.subcommand != "log" && args.subcommand != "main" {
        raw_args.push(args.subcommand.clone());
    }
    raw_args.extend(args.rev_args.iter().cloned());

    let spec = match engine.parse_rev_args(&raw_args) {
        Ok(spec) => spec,
        Err(err) => {
            eprintln!("tigrs: {err}");
            return 1;
        }
    };

    // Resolve the implicit HEAD tip before the TUI takes over the terminal, so a
    // repository whose HEAD cannot be read reports why instead of rendering an
    // empty log. A repository with no commits yet resolves to no tips and is
    // rendered as an empty history, matching `git log` on a fresh repository.
    let spec = match engine.resolve_spec_tips(spec) {
        Ok(spec) => spec,
        Err(err) => {
            eprintln!("tigrs: {err}");
            return 1;
        }
    };

    let branch_name = if raw_args.is_empty() {
        engine
            .current_branch()
            .unwrap_or_else(|_| "HEAD".to_string())
    } else {
        raw_args.join(" ")
    };

    let (cancel_src, token) = CancellationToken::new();
    let (tx, rx) = bounded::<Vec<CommitSummary>>(CHANNEL_DEPTH);
    let view = MainView::new(branch_name);

    // Spawn the commit count estimator on a background thread so neither commit-graph
    // mapping nor pack index sampling delays spawning the primary revwalk worker or TTFF.
    {
        let total_handle = view.total_commits_handle();
        let counting_engine = engine.clone();
        let counting_spec = spec.clone();
        thread::spawn(move || {
            if counting_spec.is_full_history()
                && let Some(est) = counting_engine.fast_commit_count_estimate()
            {
                total_handle.store(est, std::sync::atomic::Ordering::Relaxed);
                if est >= tigrs_ui::view::LARGE_HISTORY_THRESHOLD {
                    return;
                }
            }
            if let Ok(total) = counting_engine.count_commits_for_spec(&counting_spec) {
                total_handle.store(total, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }

    // The walker cannot draw on the terminal, so any failure is recorded here and
    // reported after the TUI has been torn down.
    let walk_error = Arc::new(Mutex::new(None::<String>));
    let chunk_size = config.performance.memory_profile.revwalk_chunk_size();

    let engine_worker = engine.clone();
    let worker_error = Arc::clone(&walk_error);
    let worker = thread::spawn(move || {
        let record = |err: TigError| {
            if let Ok(mut slot) = worker_error.lock() {
                *slot = Some(err.to_string());
            }
        };

        let stream = match engine_worker.stream_commits_spec(spec, Some(chunk_size), token.clone())
        {
            Ok(stream) => stream,
            Err(err) => {
                record(err);
                return;
            }
        };

        for batch in stream {
            let batch = match batch {
                Ok(batch) => batch,
                // Cancellation is the expected way this walk ends early.
                Err(TigError::Cancelled) => return,
                Err(err) => {
                    record(err);
                    return;
                }
            };

            // A bounded channel means a fast walker can outrun the renderer.
            // Re-check cancellation while parked so quitting stays responsive
            // instead of blocking until the renderer drains a full buffer.
            let mut pending = batch;
            loop {
                if token.is_cancelled() {
                    return;
                }
                match tx.send_timeout(pending, SEND_POLL_INTERVAL) {
                    Ok(()) => break,
                    Err(SendTimeoutError::Timeout(unsent)) => pending = unsent,
                    Err(SendTimeoutError::Disconnected(_)) => return,
                }
            }
        }
    });

    let result = run_app(view, &rx, &cancel_src, Some(engine), Some(&config));

    // Order matters: cancel first, then hang up the channel so any parked send
    // fails immediately rather than waiting out its poll interval.
    cancel_src.cancel();
    drop(rx);
    let _ = worker.join();

    if let Some(err) = walk_error.lock().ok().and_then(|mut slot| slot.take()) {
        eprintln!("tigrs: {err}");
    }

    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("tigrs: {err}");
            1
        }
    }
}

/// Maximum number of lines buffered in piped stdin mode to prevent OOM.
const MAX_PIPED_LINES: usize = 500_000;
/// Maximum byte length of a single line read in piped stdin mode (64 KiB).
const MAX_PIPED_LINE_BYTES: u64 = 64 * 1024;

/// Consumes piped input from `stdin` and displays it interactively in the TUI.
fn run_piped_mode() -> i32 {
    use std::io::Read as _;
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut line = String::new();
    let mut rows = Vec::new();

    let dummy_id = ObjectId::empty_tree(gix::hash::Kind::Sha1);
    let origin: Arc<str> = Arc::from("stdin");
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    )
    .unwrap_or(0);

    while rows.len() < MAX_PIPED_LINES
        && (&mut reader)
            .take(MAX_PIPED_LINE_BYTES)
            .read_line(&mut line)
            .unwrap_or(0)
            > 0
    {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        rows.push(CommitSummary {
            id: dummy_id,
            parents: tigrs_git::ParentIds::new(),
            author_name: Arc::clone(&origin),
            author_time_secs: now,
            summary: Box::from(filter_sgr_only(trimmed)),
        });
        line.clear();
    }

    let mut view = MainView::new("stdin".to_string());
    view.append_commits(rows);
    view.set_finished();

    let (cancel_src, _token) = CancellationToken::new();
    // No producer: the sender is dropped immediately so the view reports a
    // completed stream rather than waiting on a channel that will never fill.
    let (_, rx) = bounded::<Vec<CommitSummary>>(1);

    let config = tigrs_core::Config::load_default();
    if let Err(err) = run_app(view, &rx, &cancel_src, None, Some(&config)) {
        eprintln!("tigrs: {err}");
        return 1;
    }

    0
}

fn generate_completions<W: std::io::Write>(shell: clap_complete::Shell, writer: &mut W) {
    use clap::CommandFactory;
    let mut cmd = CliArgs::command();
    clap_complete::generate(shell, &mut cmd, "tigrs", writer);
}

/// Generates shell completion scripts for supported shells (bash, zsh, fish, elvish, powershell).
fn run_completions(args: &CliArgs) -> i32 {
    use clap_complete::Shell;
    use std::io::Write;
    use std::str::FromStr;

    let shell_name = args
        .shell
        .as_deref()
        .or_else(|| args.rev_args.first().map(String::as_str))
        .unwrap_or("bash");

    let Ok(shell) = Shell::from_str(shell_name) else {
        eprintln!(
            "tigrs: unsupported shell '{shell_name}'. Supported shells: bash, zsh, fish, elvish, powershell"
        );
        return 1;
    };

    let mut buf = Vec::with_capacity(8192);
    generate_completions(shell, &mut buf);
    if let Err(err) = std::io::stdout().write_all(&buf) {
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            return 0;
        }
        eprintln!("tigrs: failed to write completions: {err}");
        return 1;
    }
    0
}

fn generate_man_page<W: std::io::Write>(writer: &mut W) -> Result<(), std::io::Error> {
    use clap::CommandFactory;
    let cmd = CliArgs::command();
    let man = clap_mangen::Man::new(cmd);
    man.render(writer)
}

/// Generates the standard UNIX roff/troff man page for tigrs.
fn run_man() -> i32 {
    if let Err(err) = generate_man_page(&mut std::io::stdout()) {
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            return 0;
        }
        eprintln!("tigrs: failed to render man page: {err}");
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap_complete::Shell;

    #[test]
    fn test_parse_git_grep_output() {
        let sample = "src/main.rs:10:fn main() {\nsrc/lib.rs:25:pub fn run() {\nempty:\ninvalid:line\npath/file.rs:5:foo:bar:baz\n";
        let matches = parse_git_grep_output(sample);
        assert_eq!(matches.len(), 3);

        assert_eq!(matches[0].path, "src/main.rs");
        assert_eq!(matches[0].line_num, 10);
        assert_eq!(matches[0].content, "fn main() {");

        assert_eq!(matches[1].path, "src/lib.rs");
        assert_eq!(matches[1].line_num, 25);
        assert_eq!(matches[1].content, "pub fn run() {");

        assert_eq!(matches[2].path, "path/file.rs");
        assert_eq!(matches[2].line_num, 5);
        assert_eq!(matches[2].content, "foo:bar:baz");
    }

    #[test]
    fn test_parse_git_grep_output_empty() {
        let matches = parse_git_grep_output("");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_generate_completions_all_supported_shells() {
        let shells = [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::Elvish,
            Shell::PowerShell,
        ];

        for shell in shells {
            let mut buf = Vec::new();
            generate_completions(shell, &mut buf);
            assert!(
                !buf.is_empty(),
                "Completions for {shell:?} should not be empty"
            );
            let output = String::from_utf8_lossy(&buf);
            assert!(
                output.contains("tigrs"),
                "Completions for {shell:?} must reference tigrs"
            );
        }
    }

    #[test]
    fn test_parse_git_grep_output_edge_cases() {
        let sample = "\
:12:empty path should be skipped
malformed_line_no_colons
file.rs:not_a_number:invalid line number
file.rs:100:let x: Option<usize> = Some(1); // multiple:colons:in:code
path/to/file.rs:42:
only/path/and/line.rs:99
\n   \n
";
        let matches = parse_git_grep_output(sample);
        assert_eq!(matches.len(), 3);

        assert_eq!(matches[0].path, "file.rs");
        assert_eq!(matches[0].line_num, 100);
        assert_eq!(
            matches[0].content,
            "let x: Option<usize> = Some(1); // multiple:colons:in:code"
        );

        assert_eq!(matches[1].path, "path/to/file.rs");
        assert_eq!(matches[1].line_num, 42);
        assert_eq!(matches[1].content, "");

        assert_eq!(matches[2].path, "only/path/and/line.rs");
        assert_eq!(matches[2].line_num, 99);
        assert_eq!(matches[2].content, "");
    }

    #[test]
    fn test_run_completions_cli() {
        let valid_args = CliArgs::try_parse_from(["tigrs", "completions", "-s", "zsh"]).unwrap();
        assert_eq!(run_completions(&valid_args), 0);

        let positional_args = CliArgs::try_parse_from(["tigrs", "completions", "fish"]).unwrap();
        assert_eq!(run_completions(&positional_args), 0);

        let invalid_args =
            CliArgs::try_parse_from(["tigrs", "completions", "-s", "invalid_shell"]).unwrap();
        assert_eq!(run_completions(&invalid_args), 1);
    }

    #[test]
    fn test_generate_man_page() {
        let mut buf = Vec::new();
        generate_man_page(&mut buf).expect("render man page");
        assert!(!buf.is_empty());
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains(".TH") || output.contains("tigrs"));
        assert!(output.contains("SYNOPSIS") || output.contains("DESCRIPTION"));
    }

    #[test]
    fn test_run_man_execution() {
        assert_eq!(run_man(), 0);
    }

    #[test]
    fn test_cli_directory_and_config_handling() {
        let temp_dir = tempfile::tempdir().expect("tempdir");

        // Non-existent directory should return error on GitEngine::open
        let non_existent = temp_dir.path().join("not_a_repo");
        let err = GitEngine::open(Some(&non_existent));
        assert!(err.is_err());

        // Non-existent config file should fail
        let non_existent_cfg = temp_dir.path().join("no_config.toml");
        let cfg_err = tigrs_core::Config::load_from_file(&non_existent_cfg);
        assert!(cfg_err.is_err());

        // Valid config file should succeed
        let valid_cfg = temp_dir.path().join("valid_config.toml");
        std::fs::write(
            &valid_cfg,
            "[general]\ntab_size = 8\n\n[view]\nshow_changes = false\n",
        )
        .expect("write config");
        let loaded = tigrs_core::Config::load_from_file(&valid_cfg).expect("load valid config");
        assert_eq!(loaded.general.tab_size, 8);
        assert!(!loaded.view.show_changes);
    }

    #[test]
    fn test_run_cli_grep_in_temp_repo() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let repo_path = temp_dir.path();

        // Initialize git repo using command line
        let init_res = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init"])
            .current_dir(repo_path)
            .output();

        if let Ok(output) = init_res
            && output.status.success()
        {
            let test_file = repo_path.join("search_target.txt");
            std::fs::write(
                &test_file,
                "first line\nneedle: found on line 2\nthird line\n",
            )
            .expect("write target");

            // git add
            let _ = std::process::Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(["add", "search_target.txt"])
                .current_dir(repo_path)
                .output();

            let matches = run_cli_grep("needle", Some(repo_path)).expect("run_cli_grep");
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].path, "search_target.txt");
            assert_eq!(matches[0].line_num, 2);
            assert!(matches[0].content.contains("needle: found on line 2"));

            // Grep with no matches should return empty vec, not error
            let no_matches =
                run_cli_grep("nonexistent_needle_token", Some(repo_path)).expect("grep no match");
            assert!(no_matches.is_empty());
        }
    }

    #[test]
    fn test_run_repository_mode_error_flows() {
        let temp_dir = tempfile::tempdir().expect("tempdir");

        // 1. Non-existent directory
        let bad_dir_args = CliArgs::try_parse_from([
            "tigrs",
            "-C",
            "/nonexistent/directory/that/does/not/exist",
            "status",
        ])
        .unwrap();
        assert_eq!(run_repository_mode(&bad_dir_args), 1);

        // 2. Non-existent config
        let bad_cfg_args =
            CliArgs::try_parse_from(["tigrs", "-c", "/nonexistent/config/file.toml", "status"])
                .unwrap();
        assert_eq!(run_repository_mode(&bad_cfg_args), 1);

        // 3. Valid repo with invalid revision
        let repo_path = temp_dir.path();
        let _ = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init"])
            .current_dir(repo_path)
            .output();

        let bad_show_args = CliArgs::try_parse_from([
            "tigrs",
            "-C",
            repo_path.to_str().unwrap(),
            "show",
            "nonexistent_rev_12345",
        ])
        .unwrap();
        assert_eq!(run_repository_mode(&bad_show_args), 1);

        let bad_tree_args = CliArgs::try_parse_from([
            "tigrs",
            "-C",
            repo_path.to_str().unwrap(),
            "tree",
            "nonexistent_rev_12345",
        ])
        .unwrap();
        assert_eq!(run_repository_mode(&bad_tree_args), 1);

        let bad_blob_args = CliArgs::try_parse_from([
            "tigrs",
            "-C",
            repo_path.to_str().unwrap(),
            "blob",
            "nonexistent_rev_12345",
        ])
        .unwrap();
        assert_eq!(run_repository_mode(&bad_blob_args), 1);

        let bad_blame_args = CliArgs::try_parse_from([
            "tigrs",
            "-C",
            repo_path.to_str().unwrap(),
            "blame",
            "nonexistent_file.rs",
        ])
        .unwrap();
        assert_eq!(run_repository_mode(&bad_blame_args), 1);

        // blob missing path
        let blob_missing_path =
            CliArgs::try_parse_from(["tigrs", "-C", repo_path.to_str().unwrap(), "blob", "HEAD"])
                .unwrap();
        assert_eq!(run_repository_mode(&blob_missing_path), 1);

        // blame missing path
        let blame_missing_path =
            CliArgs::try_parse_from(["tigrs", "-C", repo_path.to_str().unwrap(), "blame"]).unwrap();
        assert_eq!(run_repository_mode(&blame_missing_path), 1);

        // blame with explicit rev and nonexistent file
        let blame_bad_file_with_rev = CliArgs::try_parse_from([
            "tigrs",
            "-C",
            repo_path.to_str().unwrap(),
            "blame",
            "HEAD",
            "nonexistent_file_path.rs",
        ])
        .unwrap();
        assert_eq!(run_repository_mode(&blame_bad_file_with_rev), 1);

        // tree nonexistent path
        let tree_bad_path = CliArgs::try_parse_from([
            "tigrs",
            "-C",
            repo_path.to_str().unwrap(),
            "tree",
            "HEAD",
            "nonexistent_subdir",
        ])
        .unwrap();
        assert_eq!(run_repository_mode(&tree_bad_path), 1);

        // log with invalid revision
        let log_bad_rev = CliArgs::try_parse_from([
            "tigrs",
            "-C",
            repo_path.to_str().unwrap(),
            "log",
            "nonexistent_branch_xyz",
        ])
        .unwrap();
        assert_eq!(run_repository_mode(&log_bad_rev), 1);
    }

    #[test]
    fn test_run_completions_and_man() {
        let bad_shell = CliArgs::try_parse_from(["tigrs", "completions", "unknown_shell"]).unwrap();
        assert_eq!(run_completions(&bad_shell), 1);

        let good_shell = CliArgs::try_parse_from(["tigrs", "completions", "bash"]).unwrap();
        assert_eq!(run_completions(&good_shell), 0);

        let shell_flag =
            CliArgs::try_parse_from(["tigrs", "completions", "--shell", "zsh"]).unwrap();
        assert_eq!(run_completions(&shell_flag), 0);

        assert_eq!(run_man(), 0);
    }

    #[test]
    fn test_parse_git_grep_output_crlf_and_empty() {
        assert!(parse_git_grep_output("").is_empty());
        assert!(parse_git_grep_output("   \n\n\t\n").is_empty());

        let crlf_sample = "src/lib.rs:10:line with crlf\r\nsrc/lib.rs:20:second line\r\n";
        let matches = parse_git_grep_output(crlf_sample);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].path, "src/lib.rs");
        assert_eq!(matches[0].line_num, 10);
        assert_eq!(matches[1].line_num, 20);
    }

    #[test]
    fn test_parse_git_grep_output_nul_delimited_with_newlines_and_escapes() {
        let nul_sample = "dir/file\nwith\nnewlines.rs\x0042\x00let x = \"\x1b[31minjected\";\nother.rs\x007\x00clean line\n";
        let matches = parse_git_grep_output(nul_sample);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].path, "dir/file with newlines.rs");
        assert_eq!(matches[0].line_num, 42);
        assert_eq!(matches[0].content, "let x = \"injected\";");
        assert_eq!(matches[1].path, "other.rs");
        assert_eq!(matches[1].line_num, 7);
        assert_eq!(matches[1].content, "clean line");
    }
}
