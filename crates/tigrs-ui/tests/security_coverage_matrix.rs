// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Security coverage matrix: regression tests for every hostile-repository
//! finding that was not already pinned by `security_rce_prevention.rs`.
//!
//! `tigrs` renders repositories that an attacker may fully control: every byte
//! of `.git/config`, `.git/info/attributes`, `.gitattributes`, `.gitmodules`,
//! ref names, commit metadata, tree entry names, and worktree symlinks is
//! untrusted input. The audit rounds that produced the hardening in
//! `tigrs-git` are enumerated below together with the test that keeps each
//! fix from regressing.
//!
//! | ID | Finding | Hardening | Test |
//! |----|---------|-----------|------|
//! | T1 | Repository-local `.tigrc` command bindings (the upstream C `tig` class of bug) | `tigrs` only ever reads `$XDG_CONFIG_HOME/tigrs/config.toml`; no repository-relative or `$HOME/.tigrc` source exists | [`test_repository_local_tigrc_is_never_loaded`] |
//! | T2 | `git blame` running `diff.<drv>.textconv` / `filter.<drv>.smudge` | `blame.rs` passes `--no-textconv` on top of `safe_git_command` | [`test_blame_never_runs_repository_textconv_or_filter_drivers`] |
//! | T3 | `:grep` flag injection and textconv execution | `run_git_grep` passes `--no-textconv`, `-e <pattern>`, and a trailing `--` | [`test_grep_is_not_flag_injectable_and_sanitizes_output`] |
//! | T4 | Command-executing Git configuration keys | `ConfigSanitizationScanner::into_plan` emits `-c key=<neutral>` overrides | [`test_command_executing_config_keys_are_neutralized_exhaustively`] |
//! | T5 | Terminal escape / OSC-52 / BiDi injection through repository metadata | `tigrs_core::ansi::strip_control_chars` on every engine-produced string | [`test_terminal_escape_injection_is_stripped_across_engine_surfaces`] |
//! | T6 | External diff formatter inheriting a hostile Git environment | `run_external_formatter` applies `apply_untrusted_repo_env` | [`test_external_formatter_child_runs_with_hardened_git_env`] |
//! | T7 | `:!cmd` / `:+cmd` / editor handover inheriting a hostile Git environment | all shell spawn sites apply `apply_untrusted_repo_env` | [`test_shell_handover_sites_apply_untrusted_repo_env`] |
//! | T8 | `[include]` depth bomb and self-referential include cycle | depth cap of 16 plus a visited-set in `collect_config_with_includes` | [`test_config_include_bomb_and_cycle_terminate_and_are_scanned`] |
//! | T9 | Worktree symlink loops and `.git` / out-of-tree escapes | `MAX_SYMLINK_DEPTH` = 40 in `resolve_worktree_components` | [`test_worktree_symlink_loops_and_escapes_are_rejected`] |
//! | T10 | Tier-1 `gix` executing a *global* driver bound by a hostile worktree `.gitattributes` | `discover_repository` rejects every `gix::config::Source` for driver sections | [`test_gix_tier1_rejects_global_drivers_bound_by_hostile_gitattributes`] |
//! | T11 | Unbounded regex compilation from `diff.<drv>.xfuncname` | `MAX_PATTERN_BYTES` / `MAX_ALTERNATIVES` caps in `userdiff::compile` | [`test_attacker_controlled_xfuncname_cannot_stall_the_diff_pipeline`] |
//! | T12 | UTF-8 BOM and CRLF smuggling in attribute driver bindings | BOM strip + CRLF trim in `extract_attribute_drivers_from_line` for `$GIT_DIR/info/attributes`; `GIT_ATTR_SOURCE=<empty tree>` disables worktree `.gitattributes` outright | [`test_bom_and_crlf_attribute_bindings_are_still_discovered`] |
//! | T13 | `core.attributesFile` / `diff.external` pointing at attacker scripts | covered by the T4 static override table | [`test_command_executing_config_keys_are_neutralized_exhaustively`] |
//! | T14 | Whole-surface sweep of a repository arming every primitive at once | the sum of all of the above | [`test_fully_armed_repository_sweep_files_backend`] and [`test_fully_armed_repository_sweep_reftable_backend`] |
//! | T15 | `LineBuffer::from_raw_bytes` UTF-8 fast-path bypass for C1 8-bit controls (`0xC2`) and Trojan Source BiDi overrides (`0xE2`) when no ASCII C0 controls are present | `LineBuffer::from_raw_bytes`, `LineBuffer::from_lines`, `truncate_visible_width`, and `verify_no_control_chars` reject `0xC2`/`0xE2` dangerous Unicode ranges | [`test_blob_line_buffer_fast_path_strips_c1_controls_and_bidi_overrides`] |
//! | T16 | Repository trust check logical disjunction (`work_dir \|\| git_dir`) allowing untrusted `gitdir` / `commondir` or untrusted `work_dir` to bypass `security.trusted_repos` | `AppState::is_current_repo_trusted` requires logical conjunction (`&&`) across `work_dir`, `eng.work_dir()`, `eng.info().git_dir`, and all `discover_git_metadata_dirs(work_dir)` | [`test_repo_trust_requires_conjunction_of_work_dir_and_all_git_metadata_dirs`] |
//! | T17 | `Change::Rewrite` (`FileChangeStatus::Renamed` and `FileChangeStatus::Copied`) missing `strip_control_chars` on `source_location` | `diff.rs` sanitizes `source_location` via `strip_control_chars(&source_location.to_str_lossy()).into_owned()` | [`test_diff_rename_and_copy_source_paths_strip_control_chars`] |
//! | T18 | `find_func_context` (`diff.rs`) and `parse_hunk_header` (`status.rs`) missing `strip_control_chars` on `DiffHunk::func_context`, plus `O(N^2)` backward rescan across hunks | `find_func_context` and `parse_hunk_header` strip control chars, and `HunkCollector` memoizes `last_func_scan` for `O(N)` scanning with cancellation checks | [`test_diff_hunk_func_context_strips_control_chars_and_runs_in_linear_time`] |
//! | T19 | Unbounded `:grep` output buffering (`run_git_grep` / `run_cli_grep`), `status.rs` header path sanitization, and `log_view.rs` diffstat spoofing (`CWE-451`) | `run_git_grep` and `run_cli_grep` bound stdout at 16 MiB and 20,000 matches; `parse_unified_diff` strips control chars from all header paths; `LogView` renders diffstat from structured fields | [`test_grep_command_bounds_matches_and_strips_control_chars`] |
//! | T20 | `S0` `Config::default_config_path` relative/missing `HOME` fallback to `.` loading worktree `.config/tigrs/config.toml` (`CWE-114`), and `S0` editor/macro argument injection when control/BiDi chars precede `-` or `+` (`\x1b-c!sh`, `CWE-88`) | `default_config_path` and `default_history_path` require `Path::is_absolute` and return `None` when unset/relative; `build_editor_command_line`, `validate_target_readable`, and `MacroContext::get_var_cow` strip control chars before checking leading `-`/`+` | [`test_s0_config_home_fallback_and_editor_control_char_argument_injection`] |

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tigrs_core::cancel::CancellationToken;
use tigrs_core::config::Config;
use tigrs_git::GitEngine;
use tigrs_git::status::{StatusItem, StatusSection};
use tigrs_ui::TerminalCapabilities;
use tigrs_ui::app::commands::execute_parsed_command;
use tigrs_ui::app::layout::ViewKind;
use tigrs_ui::app::{AppState, Flow};
use tigrs_ui::prompt::ParsedCommand;
use tigrs_ui::view::MainView;

/// Set by a parent test on a re-executed copy of this test binary.
///
/// Process environment cannot be mutated in-process: the workspace is edition
/// 2024 (`std::env::set_var` is `unsafe`) and every crate is
/// `#![forbid(unsafe_code)]`. Tests that must control `HOME`,
/// `XDG_CONFIG_HOME`, or `GIT_CONFIG_GLOBAL` therefore re-exec this binary with
/// a single `#[test]` selected and this variable set; the selected test is a
/// no-op in the parent process.
const CHILD_ENV: &str = "TIGRS_SECURITY_COVERAGE_CHILD";

/// Payload substring that must never appear in rendered output, and whose
/// side-effect marker files must never be created.
const PWNED: &str = "TIGRS_PWNED";

fn in_child_process() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

/// Re-executes this test binary running exactly one test with extra environment.
fn run_child_test(name: &str, cwd: &Path, envs: &[(&str, &Path)], remove: &[&str]) {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = Command::new(exe);
    cmd.args([name, "--exact", "--nocapture", "--test-threads=1"])
        .current_dir(cwd)
        .env(CHILD_ENV, "1");
    for (key, value) in envs {
        cmd.env(key, value);
    }
    for key in remove {
        cmd.env_remove(key);
    }
    let out = cmd.output().expect("re-exec test binary");
    assert!(
        out.status.success(),
        "child test `{name}` failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Alice Developer")
        .env("GIT_AUTHOR_EMAIL", "alice@example.com")
        .env("GIT_COMMITTER_NAME", "Alice Developer")
        .env("GIT_COMMITTER_EMAIL", "alice@example.com")
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// Initializes a repository pinned to an explicit ref backend.
///
/// Git >= 2.55 creates `reftable` repositories by default, and
/// `tigrs_git::status::scan_status` only takes the in-process (Tier-1) `gix`
/// path when the repository is **not** reftable-backed. Tests that must
/// exercise Tier-1 therefore have to ask for `files` explicitly, or they
/// silently re-test the already-hardened Tier-2 CLI path and assert nothing.
fn init_repo(dir: &Path, ref_format: &str) -> bool {
    fs::create_dir_all(dir).expect("create repo dir");
    let format_flag = format!("--ref-format={ref_format}");
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(dir)
        .args(["init", "-q", &format_flag, "--initial-branch=main"])
        .status()
        .expect("spawn git init");
    if !status.success() {
        if ref_format == "files" {
            git(dir, &["init", "-q", "--initial-branch=main"]);
        } else {
            return false;
        }
    }
    git(dir, &["config", "user.name", "Alice Developer"]);
    git(dir, &["config", "user.email", "alice@example.com"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    true
}

/// The `-c key=value` overrides `safe_git_command` injects for `repo`.
fn hardened_cli_overrides(repo: &Path) -> Vec<String> {
    tigrs_git::safe_git_command(repo)
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

/// Asserts that `-c <expected>` is present in a hardened argument vector.
fn assert_override(args: &[String], expected: &str) {
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "-c" && pair[1] == expected),
        "missing hardening override `-c {expected}`; got {args:?}"
    );
}

/// Reads a config value back through the hardened command builder.
fn hardened_config_get(repo: &Path, key: &str) -> String {
    let out = tigrs_git::safe_git_command(repo)
        .args(["config", "--get", key])
        .output()
        .expect("git config --get");
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// Returns every regular file directly inside `dir`, as sorted file names.
fn marker_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read marker dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Keeps the module-level traceability matrix honest.
///
/// A coverage matrix that drifts out of date is worse than no matrix at all: it
/// asserts coverage that no longer exists. This test parses this file's own
/// source and requires the finding table and the test functions to agree in
/// both directions.
#[test]
fn test_traceability_matrix_matches_the_tests_in_this_file() {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("security_coverage_matrix.rs"),
    )
    .expect("read own source");

    let mut declared: Vec<String> = Vec::new();
    let mut referenced: Vec<String> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("fn test_")
            && let Some(name) = rest.split('(').next()
        {
            declared.push(format!("test_{name}"));
        }
        if trimmed.starts_with("//! |") {
            let mut rest = trimmed;
            while let Some(start) = rest.find("[`test_") {
                rest = &rest[start + 2..];
                let Some(end) = rest.find('`') else { break };
                referenced.push(rest[..end].to_string());
                rest = &rest[end..];
            }
        }
    }
    declared.sort();
    declared.dedup();
    referenced.sort();
    referenced.dedup();

    assert!(
        !declared.is_empty() && !referenced.is_empty(),
        "the matrix parser found nothing; it has drifted from the file layout"
    );
    for name in &referenced {
        assert!(
            declared.contains(name)
                || *name == "test_traceability_matrix_matches_the_tests_in_this_file",
            "the matrix cites `{name}`, which does not exist"
        );
    }
    for name in &declared {
        assert!(
            referenced.contains(name)
                || name == "test_traceability_matrix_matches_the_tests_in_this_file",
            "`{name}` is not listed in the coverage matrix; add a row for the finding it pins"
        );
    }
}

// ---------------------------------------------------------------------------
// T1: repository-local `.tigrc` is never a configuration source
// ---------------------------------------------------------------------------

/// Child half of [`test_repository_local_tigrc_is_never_loaded`].
#[test]
fn child_config_load_default_only_reads_xdg_config_toml() {
    if !in_child_process() {
        return;
    }
    let config = Config::load_default();
    assert!(
        config
            .keybindings
            .iter()
            .any(|(key, action)| key == "generic.R" && action == "view-refs"),
        "the single legitimate source (`$HOME/.config/tigrs/config.toml`) must load: {:?}",
        config.keybindings
    );
    for (key, action) in &config.keybindings {
        assert!(
            !key.contains(PWNED) && !action.contains(PWNED),
            "a repository-relative or `$HOME/.tigrc` binding leaked into the keymap: {key} = {action}"
        );
    }
}

#[test]
fn test_repository_local_tigrc_is_never_loaded() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    fs::create_dir_all(home.join(".config").join("tigrs")).unwrap();
    init_repo(&repo, "files");

    // Upstream-`tig`-style run-command bindings, planted everywhere a naive
    // port might look for them.
    let hostile = format!("bind generic X !sh -c \"touch {PWNED}\"\nset main-view = id\n");
    for relative in [".tigrc", ".tigrs", ".git/tigrc"] {
        fs::write(repo.join(relative), &hostile).unwrap();
    }
    fs::write(home.join(".tigrc"), &hostile).unwrap();
    // A hostile TOML file in the repository, in case a future refactor were to
    // start honouring repository-relative TOML.
    fs::write(
        repo.join("config.toml"),
        format!("[keybindings]\n\"generic.X\" = \"!sh -c touch-{PWNED}\"\n"),
    )
    .unwrap();
    // The one legitimate source.
    fs::write(
        home.join(".config").join("tigrs").join("config.toml"),
        "[keybindings]\n\"generic.R\" = \"view-refs\"\n",
    )
    .unwrap();

    run_child_test(
        "child_config_load_default_only_reads_xdg_config_toml",
        &repo,
        &[("HOME", home.as_path())],
        &["XDG_CONFIG_HOME"],
    );

    assert!(
        !repo.join(PWNED).exists(),
        "no repository-local run-command may execute"
    );
}

// ---------------------------------------------------------------------------
// T2: `git blame` never runs repository drivers
// ---------------------------------------------------------------------------

#[test]
fn test_blame_never_runs_repository_textconv_or_filter_drivers() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let markers = temp.path().join("markers");
    fs::create_dir_all(&markers).unwrap();
    init_repo(&repo, "files");

    fs::write(
        repo.join("story.txt"),
        "line one\nline two\nline three\nline four\n",
    )
    .unwrap();
    fs::write(
        repo.join(".gitattributes"),
        "* diff=evilconv filter=evilfilter\n",
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "seed"]);

    for (key, marker) in [
        ("diff.evilconv.textconv", "blame-textconv"),
        ("diff.evilconv.command", "blame-diffcmd"),
        ("filter.evilfilter.clean", "blame-clean"),
        ("filter.evilfilter.smudge", "blame-smudge"),
        ("filter.evilfilter.process", "blame-process"),
    ] {
        let payload = format!("touch {}; cat", markers.join(marker).display());
        git(&repo, &["config", "--local", key, &payload]);
    }
    git(
        &repo,
        &["config", "--local", "filter.evilfilter.required", "true"],
    );

    let engine = GitEngine::open(Some(&repo)).expect("open engine");
    let head = engine.head_commit_id().expect("head");
    let blame = engine.blame_file(head, "story.txt").expect("blame");

    assert_eq!(blame.lines.len(), 4, "blame must still annotate every line");
    assert!(
        marker_names(&markers).is_empty(),
        "blame executed repository-controlled drivers: {:?}",
        marker_names(&markers)
    );
    // `--no-textconv` must be on the Tier-2 command line regardless of which
    // tier actually served this call.
    let blame_source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tigrs-git/src/blame.rs")
            .canonicalize()
            .expect("canonicalize blame.rs"),
    )
    .expect("read blame.rs");
    assert!(
        blame_source.contains("\"blame\", \"--no-textconv\""),
        "the CLI blame fallback must keep passing --no-textconv"
    );
}

// ---------------------------------------------------------------------------
// T3: `:grep` is neither flag-injectable nor an escape-sequence carrier
// ---------------------------------------------------------------------------

#[test]
fn test_grep_is_not_flag_injectable_and_sanitizes_output() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let markers = temp.path().join("markers");
    fs::create_dir_all(&markers).unwrap();
    init_repo(&repo, "files");

    // A tracked file whose *content* carries a title-setting OSC sequence, an
    // OSC-52 clipboard write, and a BiDi override.
    let hostile_line = "needle \u{1b}]0;TitleHijack\u{7}\u{1b}]52;c;cGF5bG9hZA==\u{7}\u{1b}[31m\u{202e}dangerous\n";
    fs::write(repo.join("notes.txt"), hostile_line).unwrap();
    fs::write(repo.join(".gitattributes"), "* diff=evilconv\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "seed"]);
    let payload = format!("touch {}; cat", markers.join("grep-textconv").display());
    git(
        &repo,
        &["config", "--local", "diff.evilconv.textconv", &payload],
    );

    let engine = GitEngine::open(Some(&repo)).expect("open engine");
    let config = Config::default();
    let mut app = AppState::with_engine_and_config(Some(engine), Some(&config));
    app.views.main_view = Some(MainView::new("main".to_string()));
    app.views.view_stack.push(ViewKind::Main);

    // 1. A pattern that is a real `git grep` flag capable of running a command
    //    must be treated as a literal pattern, not as an option.
    let injected = format!(
        "--open-files-in-pager=touch {}",
        markers.join("grep-pager").display()
    );
    let flow = execute_parsed_command(
        &mut app,
        ParsedCommand::parse(&format!(":grep {injected}")),
        24,
    );
    assert_eq!(flow, Flow::Continue);
    assert!(
        marker_names(&markers).is_empty(),
        "`:grep` pattern was parsed as a flag: {:?}",
        marker_names(&markers)
    );

    // 2. A pattern that is a pathspec-looking argument must not escape the `--`.
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":grep -- --cached"), 24);
    assert_eq!(flow, Flow::Continue);

    // 3. A real match must render with every control sequence stripped.
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":grep needle"), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Grep));
    let grep_view = app.views.grep_view.as_ref().expect("grep view");
    assert!(
        grep_view.line_count() >= 1,
        "expected a grep hit for `needle`"
    );
    for index in 0..grep_view.line_count() {
        let row = grep_view.row_text(index).unwrap_or_default();
        assert!(!row.contains('\u{1b}'), "grep row leaked ESC: {row:?}");
        assert!(
            !row.contains('\u{202e}'),
            "grep row leaked BiDi override: {row:?}"
        );
        assert!(!row.contains('\u{7}'), "grep row leaked BEL: {row:?}");
    }
    assert!(
        marker_names(&markers).is_empty(),
        "`:grep` ran a repository textconv driver: {:?}",
        marker_names(&markers)
    );
}

// ---------------------------------------------------------------------------
// T4 / T13: exhaustive command-executing configuration key neutralization
// ---------------------------------------------------------------------------

/// Every repository-controlled Git configuration key that can execute a command,
/// paired with the value `tigrs` must force it to.
const STATIC_NEUTRALIZATIONS: &[(&str, &str)] = &[
    ("core.fsmonitor", "false"),
    ("core.hooksPath", "/dev/null"),
    ("core.attributesFile", "/dev/null"),
    ("core.alternateRefsCommand", "/bin/false"),
    ("diff.external", ""),
    ("core.pager", "cat"),
    ("interactive.diffFilter", ""),
    ("core.sshCommand", "/bin/false"),
    ("core.gitProxy", "/bin/false"),
    ("core.askPass", "/bin/false"),
    ("credential.helper", ""),
    ("protocol.ext.allow", "never"),
    ("gpg.program", "/bin/false"),
    ("gpg.openpgp.program", "/bin/false"),
    ("gpg.x509.program", "/bin/false"),
    ("gpg.ssh.program", "/bin/false"),
    ("gpg.ssh.defaultKeyCommand", "/bin/false"),
    ("uploadpack.packObjectsHook", "/bin/false"),
    ("sendemail.smtpServer", "/bin/false"),
    ("sendemail.validate", "false"),
    ("http.proxy", ""),
    ("https.proxy", ""),
];

#[test]
fn test_command_executing_config_keys_are_neutralized_exhaustively() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let markers = temp.path().join("markers");
    fs::create_dir_all(&markers).unwrap();
    init_repo(&repo, "files");

    let payload = format!("touch {}/hit", markers.display());

    // Arm every static key.
    for (key, _) in STATIC_NEUTRALIZATIONS {
        git(&repo, &["config", "--local", key, &payload]);
    }
    // Arm every dynamically-discovered family.
    for key in [
        "filter.evilfilter.clean",
        "filter.evilfilter.smudge",
        "filter.evilfilter.process",
        "diff.evildiff.command",
        "diff.evildiff.textconv",
        "merge.evilmerge.driver",
        "alias.evilalias",
        "pager.log",
        "credential.https://example.com.helper",
        "remote.evilremote.uploadpack",
        "remote.evilremote.receivepack",
        "remote.evilremote.proxy",
        "remote.evilremote.vcs",
        "difftool.eviltool.cmd",
        "difftool.eviltool.path",
        "mergetool.eviltool.cmd",
        "mergetool.eviltool.path",
        "guitool.eviltool.cmd",
        "man.eviltool.cmd",
    ] {
        git(&repo, &["config", "--local", key, &payload]);
    }
    git(
        &repo,
        &["config", "--local", "filter.evilfilter.required", "true"],
    );
    git(
        &repo,
        &["config", "--local", "diff.evildiff.cachetextconv", "true"],
    );

    let args = hardened_cli_overrides(&repo);

    for (key, value) in STATIC_NEUTRALIZATIONS {
        assert_override(&args, &format!("{key}={value}"));
    }
    for expected in [
        "filter.evilfilter.clean=",
        "filter.evilfilter.smudge=",
        "filter.evilfilter.process=",
        "filter.evilfilter.required=false",
        "diff.evildiff.command=",
        "diff.evildiff.textconv=",
        "diff.evildiff.cachetextconv=false",
        "merge.evilmerge.driver=",
        "alias.evilalias=",
        "pager.log=false",
        "credential.https://example.com.helper=",
        "remote.evilremote.uploadpack=/bin/false",
        "remote.evilremote.receivepack=/bin/false",
        "remote.evilremote.proxy=",
        "remote.evilremote.vcs=",
        "difftool.eviltool.cmd=",
        "difftool.eviltool.path=/bin/false",
        "mergetool.eviltool.cmd=",
        "mergetool.eviltool.path=/bin/false",
        "guitool.eviltool.cmd=",
        "man.eviltool.cmd=",
    ] {
        assert_override(&args, expected);
    }

    // Behavioural confirmation: Git itself must report the neutralized values.
    for (key, value) in STATIC_NEUTRALIZATIONS {
        assert_eq!(
            hardened_config_get(&repo, key),
            *value,
            "`{key}` must resolve to the neutralized value under safe_git_command"
        );
    }
    assert_eq!(hardened_config_get(&repo, "alias.evilalias"), "");
    assert_eq!(hardened_config_get(&repo, "filter.evilfilter.clean"), "");
    assert_eq!(hardened_config_get(&repo, "diff.evildiff.textconv"), "");

    // The same plan must be applied by the shell-handover helper.
    let mut shell = Command::new("sh");
    tigrs_git::apply_untrusted_repo_env(&mut shell, &repo);
    let env_keys: Vec<String> = shell
        .get_envs()
        .filter_map(|(key, value)| value.map(|_| key.to_string_lossy().into_owned()))
        .collect();
    for required in [
        "GIT_CONFIG_COUNT",
        "GIT_OPTIONAL_LOCKS",
        "GIT_TERMINAL_PROMPT",
        "GIT_PAGER",
        "GIT_ATTR_NOSYSTEM",
        "GIT_ATTR_SOURCE",
    ] {
        assert!(
            env_keys.iter().any(|key| key == required),
            "apply_untrusted_repo_env must export {required}; got {env_keys:?}"
        );
    }

    assert!(
        marker_names(&markers).is_empty(),
        "scanning the hostile configuration executed something: {:?}",
        marker_names(&markers)
    );
}

// ---------------------------------------------------------------------------
// T5: terminal escape / OSC-52 / BiDi stripping across every engine surface
// ---------------------------------------------------------------------------

fn assert_terminal_safe(label: &str, text: &str) {
    for (name, ch) in [
        ("ESC", '\u{1b}'),
        ("BEL", '\u{7}'),
        ("CSI-8bit", '\u{9b}'),
        ("OSC-8bit", '\u{9d}'),
        ("RLO", '\u{202e}'),
        ("LRO", '\u{202d}'),
        ("RLI", '\u{2067}'),
        ("PDI", '\u{2069}'),
    ] {
        assert!(
            !text.contains(ch),
            "{label} leaked {name} into rendered output: {text:?}"
        );
    }
}

#[test]
fn test_terminal_escape_injection_is_stripped_across_engine_surfaces() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo, "files");

    // Git refuses ASCII control characters in refnames, so the escape sequences
    // are smuggled through commit subjects, author identities, and file names,
    // which have no such restriction.
    let hostile_subject =
        "Fix \u{1b}]0;TitleHijack\u{7}\u{1b}]52;c;cGF5bG9hZA==\u{7}\u{1b}[2J\u{202e}gnitset";
    let hostile_author = "Mallory \u{1b}[31mRed\u{1b}[0m\u{202e}yrollaM";

    fs::write(repo.join("alpha.txt"), "alpha\n").unwrap();
    git(&repo, &["add", "-A"]);
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(&repo)
        .args(["-c"])
        .arg(format!("user.name={hostile_author}"))
        .args([
            "-c",
            "user.email=mallory@example.com",
            "commit",
            "-q",
            "-m",
            hostile_subject,
        ])
        .status()
        .expect("spawn git commit");
    assert!(status.success(), "hostile commit must be creatable");

    // A second commit so the diff and reflog surfaces have content.
    fs::write(repo.join("alpha.txt"), "alpha\nbeta\n").unwrap();
    fs::write(
        repo.join("gamma.txt"),
        "content \u{1b}]52;c;cGF5bG9hZA==\u{7} tail\n",
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", hostile_subject]);

    // An unstaged change so the status surface has content.
    fs::write(repo.join("alpha.txt"), "alpha\nbeta\ngamma\u{1b}[31m\n").unwrap();
    git(&repo, &["stash", "-q", "-u"]);
    fs::write(repo.join("alpha.txt"), "alpha\nbeta\ndelta\u{1b}[31m\n").unwrap();

    let engine = GitEngine::open(Some(&repo)).expect("open engine");
    let cancel = CancellationToken::none();

    let chunks = engine
        .stream_commits(None, Some(32), CancellationToken::none())
        .expect("stream commits");
    let mut seen_commits = 0usize;
    for chunk in chunks {
        for commit in chunk.expect("commit chunk") {
            seen_commits += 1;
            assert_terminal_safe("commit summary", &commit.summary);
            assert_terminal_safe("commit author", &commit.author_name);
        }
    }
    assert!(seen_commits >= 2, "both commits must be streamed");

    let head = engine.head_commit_id().expect("head");
    let diff = engine.compute_commit_diff(head).expect("commit diff");
    assert!(!diff.files.is_empty(), "the diff must still render");

    for entry in engine.list_refs().expect("list refs") {
        assert_terminal_safe("ref entry", &format!("{entry:?}"));
    }
    for entry in engine.list_stashes().expect("list stashes") {
        assert_terminal_safe("stash entry", &format!("{entry:?}"));
    }
    for entry in engine.read_reflog("HEAD").expect("reflog") {
        assert_terminal_safe("reflog entry", &format!("{entry:?}"));
    }

    let report = engine.load_status(&cancel).expect("status");
    for item in report
        .staged
        .iter()
        .chain(report.unstaged.iter())
        .chain(report.untracked.iter())
    {
        assert_terminal_safe("status item", &item.path);
    }

    let listing = engine.read_tree(head, "").expect("read tree");
    assert_terminal_safe("tree listing", &format!("{listing:?}"));

    let branch = engine.current_branch().expect("current branch");
    assert_terminal_safe("current branch", &branch);
}

// ---------------------------------------------------------------------------
// T6: the external diff formatter child runs with a hardened Git environment
// ---------------------------------------------------------------------------

#[test]
fn test_external_formatter_child_runs_with_hardened_git_env() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo, "files");
    fs::write(repo.join("alpha.txt"), "alpha\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "seed"]);
    fs::write(repo.join("alpha.txt"), "alpha\nbeta\n").unwrap();
    git(&repo, &["commit", "-q", "-a", "-m", "second"]);

    let engine = GitEngine::open(Some(&repo)).expect("open engine");
    let head = engine.head_commit_id().expect("head");
    let diff = engine.compute_commit_diff(head).expect("diff");

    let caps = TerminalCapabilities::default();
    // The formatter is a *user*-configured command, but it inherits the
    // hardened Git environment so that anything it shells out to cannot be
    // steered by a hostile repository.
    let document = tigrs_ui::diff::formatter::run_external_formatter(
        &diff,
        "printf 'PAGER=%s\\n' \"$(git config --get core.pager)\"; \
         printf 'PROMPT=%s\\n' \"${GIT_TERMINAL_PROMPT:-unset}\"; \
         printf 'LOCKS=%s\\n' \"${GIT_OPTIONAL_LOCKS:-unset}\"; \
         printf 'FSMONITOR=%s\\n' \"$(git config --get core.fsmonitor)\"; cat >/dev/null",
        80,
        &caps,
    )
    .expect("run external formatter");

    let rendered: String = (0..document.len())
        .filter_map(|index| document.row_search_text(index))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered.contains("PAGER=cat"),
        "formatter child must see the neutralized core.pager: {rendered}"
    );
    assert!(
        rendered.contains("PROMPT=0"),
        "formatter child must see GIT_TERMINAL_PROMPT=0: {rendered}"
    );
    assert!(
        rendered.contains("LOCKS=0"),
        "formatter child must see GIT_OPTIONAL_LOCKS=0: {rendered}"
    );
    assert!(
        rendered.contains("FSMONITOR=false"),
        "formatter child must see the neutralized core.fsmonitor: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// T7: every shell-handover site hardens the child environment
// ---------------------------------------------------------------------------

#[test]
fn test_shell_handover_sites_apply_untrusted_repo_env() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let markers = temp.path().join("markers");
    fs::create_dir_all(&markers).unwrap();
    init_repo(&repo, "files");
    fs::write(repo.join("alpha.txt"), "alpha\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "seed"]);

    // Arm a repository hook and a repository alias.
    let hooks = repo.join(".git").join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    fs::write(
        &hook,
        format!("#!/bin/sh\ntouch {}\n", markers.join("hook").display()),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let alias_payload = format!("!touch {}", markers.join("alias").display());
    git(
        &repo,
        &["config", "--local", "alias.evilalias", &alias_payload],
    );
    git(
        &repo,
        &[
            "config",
            "--local",
            "core.pager",
            &format!("touch {}; cat", markers.join("pager").display()),
        ],
    );

    // Exactly the construction used by `:!cmd`, `:+cmd`, and the editor
    // handover in `tigrs_ui::app`.
    fs::write(repo.join("alpha.txt"), "alpha\nbeta\n").unwrap();
    let mut shell = Command::new("sh");
    shell
        .arg("-c")
        .arg("git evilalias >/dev/null 2>&1; git log -1 >/dev/null 2>&1; git commit -q -a -m handover >/dev/null 2>&1; true")
        .current_dir(&repo);
    tigrs_git::apply_untrusted_repo_env(&mut shell, &repo);
    let status = shell.status().expect("run handover shell");
    assert!(status.success(), "handover shell must complete");

    assert!(
        marker_names(&markers).is_empty(),
        "the handover shell executed repository-controlled code: {:?}",
        marker_names(&markers)
    );

    // Guard against a future refactor silently dropping the hardening at any of
    // the shell-spawning call sites.
    let ui_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for relative in ["app/mod.rs", "app/commands.rs", "diff/formatter.rs"] {
        let path = ui_src.join(relative);
        let source = fs::read_to_string(&path).expect("read ui source");
        assert!(
            source.contains("apply_untrusted_repo_env"),
            "{relative} spawns child processes and must harden their environment"
        );
    }
}

// ---------------------------------------------------------------------------
// T8: `[include]` depth bombs and cycles
// ---------------------------------------------------------------------------

/// Length of the `[include]` chain used by the include-bomb scenario, far past
/// both Git's own `MAX_INCLUDE_DEPTH` (10) and the scanner's cap (16).
const CHAIN: usize = 512;

#[test]
fn test_config_include_bomb_and_cycle_terminate_and_are_scanned() {
    let temp = TempDir::new().unwrap();
    let markers = temp.path().join("markers");
    fs::create_dir_all(&markers).unwrap();
    let payload = format!("touch {}", markers.join("include").display());

    // ---- Scenario A: a chain within Git's own `MAX_INCLUDE_DEPTH` (10). -----
    // Git really does evaluate these files, so the neutralization has to be
    // observable end to end and not merely "too deep to matter".
    let bounded = temp.path().join("bounded");
    init_repo(&bounded, "files");
    let bounded_git = bounded.join(".git");
    for level in 0..8 {
        let next = bounded_git.join(format!("chain{}", level + 1));
        fs::write(
            bounded_git.join(format!("chain{level}")),
            format!("[include]\n\tpath = {}\n", next.display()),
        )
        .unwrap();
    }
    fs::write(
        bounded_git.join("chain5"),
        format!(
            "[core]\n\tfsmonitor = {payload}\n\thooksPath = {}\n\
             [filter \"deepfilter\"]\n\tclean = {payload}\n\
             [alias]\n\tdeepalias = !{payload}\n\
             [include]\n\tpath = {}\n",
            bounded.join("evil-hooks").display(),
            bounded_git.join("chain6").display()
        ),
    )
    .unwrap();
    fs::write(bounded_git.join("chain8"), "[core]\n\tbare = false\n").unwrap();
    let bounded_config = bounded_git.join("config");
    let content = format!(
        "{}[include]\n\tpath = {}\n",
        fs::read_to_string(&bounded_config).unwrap(),
        bounded_git.join("chain0").display()
    );
    fs::write(&bounded_config, content).unwrap();

    let args = hardened_cli_overrides(&bounded);
    assert_override(&args, "core.fsmonitor=false");
    assert_override(&args, "core.hooksPath=/dev/null");
    assert_override(&args, "filter.deepfilter.clean=");
    assert_override(&args, "alias.deepalias=");
    assert_eq!(
        hardened_config_get(&bounded, "core.fsmonitor"),
        "false",
        "an included `core.fsmonitor` must still be neutralized"
    );
    assert_eq!(
        hardened_config_get(&bounded, "core.hooksPath"),
        "/dev/null",
        "an included `core.hooksPath` must still be neutralized"
    );
    assert_eq!(hardened_config_get(&bounded, "alias.deepalias"), "");
    assert_eq!(hardened_config_get(&bounded, "filter.deepfilter.clean"), "");

    // ---- Scenario B: an unbounded bomb plus two include cycles. ------------
    // Git itself refuses these (it dies at depth 10), so the requirement is
    // that `tigrs`' own scanner terminates promptly and fails closed rather
    // than recursing forever or blowing the stack.
    let bomb = temp.path().join("bomb");
    init_repo(&bomb, "files");
    let bomb_git = bomb.join(".git");
    for level in 0..CHAIN {
        let next = bomb_git.join(format!("chain{}", level + 1));
        fs::write(
            bomb_git.join(format!("chain{level}")),
            format!("[include]\n\tpath = {}\n", next.display()),
        )
        .unwrap();
    }
    // The tail of the chain loops back into a two-node cycle.
    fs::write(
        bomb_git.join(format!("chain{CHAIN}")),
        format!(
            "[include]\n\tpath = {}\n",
            bomb_git.join("cycle-a").display()
        ),
    )
    .unwrap();
    fs::write(
        bomb_git.join("cycle-a"),
        format!(
            "[include]\n\tpath = {}\n",
            bomb_git.join("cycle-b").display()
        ),
    )
    .unwrap();
    fs::write(
        bomb_git.join("cycle-b"),
        format!(
            "[include]\n\tpath = {}\n[filter \"cyclefilter\"]\n\tclean = {payload}\n",
            bomb_git.join("cycle-a").display()
        ),
    )
    .unwrap();
    // A self-referential include, and a wildcard include that fans out.
    fs::write(
        bomb_git.join("self.inc"),
        format!(
            "[include]\n\tpath = {}\n",
            bomb_git.join("self.inc").display()
        ),
    )
    .unwrap();
    let bomb_config = bomb_git.join("config");
    let content = format!(
        "{}[include]\n\tpath = {}\n\tpath = {}\n\tpath = {}\n",
        fs::read_to_string(&bomb_config).unwrap(),
        bomb_git.join("chain0").display(),
        bomb_git.join("self.inc").display(),
        bomb_git.join("cycle-a").display()
    );
    fs::write(&bomb_config, content).unwrap();

    let started = Instant::now();
    let bomb_args = hardened_cli_overrides(&bomb);
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(30),
        "include-bomb scanning must terminate promptly, took {elapsed:?}"
    );
    assert_override(&bomb_args, "core.fsmonitor=false");
    assert_override(&bomb_args, "core.hooksPath=/dev/null");
    // The cycle is reachable inside the scanner's depth cap, so its driver name
    // is still discovered and neutralized.
    assert_override(&bomb_args, "filter.cyclefilter.clean=");

    // Git itself fails closed on the cycle rather than honouring anything.
    let out = tigrs_git::safe_git_command(&bomb)
        .args(["config", "--get", "core.fsmonitor"])
        .output()
        .expect("git config --get");
    let value = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    assert!(
        value.is_empty() || value == "false",
        "git must fail closed or report the neutralized value, got {value:?}"
    );

    assert!(
        marker_names(&markers).is_empty(),
        "scanning the include bomb executed something: {:?}",
        marker_names(&markers)
    );
}

// ---------------------------------------------------------------------------
// T9: worktree symlink loops and escapes
// ---------------------------------------------------------------------------

#[test]
fn test_worktree_symlink_loops_and_escapes_are_rejected() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "top secret\n").unwrap();
    init_repo(&repo, "files");
    fs::write(repo.join("real.txt"), "real\n").unwrap();

    // A two-node symlink loop, a self-loop, and a long chain.
    symlink("loop-b", repo.join("loop-a")).unwrap();
    symlink("loop-a", repo.join("loop-b")).unwrap();
    symlink("self-loop", repo.join("self-loop")).unwrap();
    for level in 0..64 {
        symlink(
            format!("chain{}", level + 1),
            repo.join(format!("chain{level}")),
        )
        .unwrap();
    }
    symlink("chain0", repo.join("chain64")).unwrap();

    // Escapes.
    symlink(outside.join("secret.txt"), repo.join("escape-abs")).unwrap();
    symlink("../outside/secret.txt", repo.join("escape-rel")).unwrap();
    symlink(".git/config", repo.join("escape-gitdir")).unwrap();
    fs::create_dir_all(repo.join("nested")).unwrap();
    symlink("../.git", repo.join("nested").join("dotgit")).unwrap();

    let started = Instant::now();
    for hostile in [
        "loop-a",
        "loop-b",
        "self-loop",
        "chain0",
        "chain64",
        "escape-abs",
        "escape-rel",
        "escape-gitdir",
        "nested/dotgit/config",
    ] {
        let result = tigrs_git::verify_worktree_path_safety(&repo, hostile);
        assert!(
            result.is_err(),
            "`{hostile}` must be rejected, got {:?}",
            result.map(|path| path.display().to_string())
        );
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "symlink resolution must terminate promptly, took {elapsed:?}"
    );

    // Traversal and absolute paths are rejected before any filesystem access.
    for hostile in [
        "../outside/secret.txt",
        "/etc/passwd",
        "a/../../b",
        ".git/config",
        ".GIT/config",
    ] {
        assert!(
            tigrs_git::verify_relative_path(hostile).is_err(),
            "`{hostile}` must be rejected by verify_relative_path"
        );
    }

    // A legitimate path still resolves.
    let resolved = tigrs_git::verify_worktree_path_safety(&repo, "real.txt").expect("real.txt");
    assert!(resolved.ends_with("real.txt"));
}

// ---------------------------------------------------------------------------
// T10: Tier-1 `gix` must ignore *global* drivers bound by hostile attributes
// ---------------------------------------------------------------------------

/// Child half of [`test_gix_tier1_rejects_global_drivers_bound_by_hostile_gitattributes`].
#[test]
fn child_gix_tier1_ignores_global_drivers() {
    if !in_child_process() {
        return;
    }
    let repo = PathBuf::from(std::env::var_os("TIGRS_CHILD_REPO").expect("TIGRS_CHILD_REPO"));
    let markers =
        PathBuf::from(std::env::var_os("TIGRS_CHILD_MARKERS").expect("TIGRS_CHILD_MARKERS"));

    let engine = GitEngine::open(Some(&repo)).expect("open engine");
    let cancel = CancellationToken::none();
    let report = engine.load_status(&cancel).expect("status");
    assert_eq!(
        report.unstaged.len(),
        1,
        "the Tier-1 gix status path must still report the modified file"
    );

    let item = StatusItem::new('M', StatusSection::Unstaged, "tracked.txt", None);
    let diff = engine.compute_status_item_diff(&item).expect("item diff");
    assert!(!diff.files.is_empty(), "the item diff must still render");

    let head = engine.head_commit_id().expect("head");
    let _ = engine.compute_commit_diff(head).expect("commit diff");
    let _ = engine.blame_file(head, "tracked.txt").expect("blame");

    let found = marker_names(&markers);
    assert!(
        found.is_empty(),
        "in-process gix executed a globally-defined driver bound by repository attributes: {found:?}"
    );
}

#[test]
fn test_gix_tier1_rejects_global_drivers_bound_by_hostile_gitattributes() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    let markers = temp.path().join("markers");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&markers).unwrap();
    init_repo(&repo, "files");

    // The drivers live in the *user's* global configuration, where
    // `gix::config::section::is_trusted` would ordinarily accept them...
    let global = home.join("gitconfig");
    fs::write(
        &global,
        format!(
            "[filter \"globalfilter\"]\n\tclean = touch {clean}; cat\n\tsmudge = touch {smudge}; cat\n\tprocess = touch {process}\n\trequired = true\n\
             [diff \"globalconv\"]\n\ttextconv = touch {textconv}; cat\n\tcommand = touch {command}; cat\n\
             [merge \"globalmerge\"]\n\tdriver = touch {merge}\n",
            clean = markers.join("global-clean").display(),
            smudge = markers.join("global-smudge").display(),
            process = markers.join("global-process").display(),
            textconv = markers.join("global-textconv").display(),
            command = markers.join("global-command").display(),
            merge = markers.join("global-merge").display(),
        ),
    )
    .unwrap();

    // ...and the *repository* only supplies the binding.
    fs::write(repo.join("tracked.txt"), "v1\n").unwrap();
    fs::write(
        repo.join(".gitattributes"),
        "* filter=globalfilter diff=globalconv merge=globalmerge\n",
    )
    .unwrap();
    let info = repo.join(".git").join("info");
    fs::create_dir_all(&info).unwrap();
    fs::write(
        info.join("attributes"),
        "* filter=globalfilter diff=globalconv\n",
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "seed"]);
    fs::write(repo.join("tracked.txt"), "v2-modified\n").unwrap();

    run_child_test(
        "child_gix_tier1_ignores_global_drivers",
        &repo,
        &[
            ("HOME", home.as_path()),
            ("GIT_CONFIG_GLOBAL", global.as_path()),
            ("TIGRS_CHILD_REPO", repo.as_path()),
            ("TIGRS_CHILD_MARKERS", markers.as_path()),
        ],
        &["XDG_CONFIG_HOME"],
    );

    assert!(
        marker_names(&markers).is_empty(),
        "a globally-defined driver fired: {:?}",
        marker_names(&markers)
    );
}

// ---------------------------------------------------------------------------
// T11: attacker-controlled `xfuncname` cannot stall the diff pipeline
// ---------------------------------------------------------------------------

#[test]
fn test_attacker_controlled_xfuncname_cannot_stall_the_diff_pipeline() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo, "files");

    // 64 KiB of catastrophically-backtracking alternatives, one per line: each
    // line costs one regex compilation unless the caps hold.
    let alternative = "^(a+)+(b+)+(c+)+$";
    let mut pattern = String::new();
    while pattern.len() < 64 * 1024 {
        pattern.push_str(alternative);
        pattern.push('\n');
    }

    fs::write(repo.join(".gitattributes"), "* diff=bomb\n").unwrap();
    fs::write(repo.join("alpha.txt"), "fn one() {}\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "seed"]);
    git(
        &repo,
        &["config", "--local", "diff.bomb.xfuncname", &pattern],
    );

    let mut body = String::new();
    for index in 0..2000 {
        body.push_str("fn generated_");
        body.push_str(&index.to_string());
        body.push_str("() { let _ = 0; }\n");
    }
    fs::write(repo.join("alpha.txt"), &body).unwrap();
    git(&repo, &["commit", "-q", "-a", "-m", "grow"]);

    let engine = GitEngine::open(Some(&repo)).expect("open engine");
    let head = engine.head_commit_id().expect("head");

    let started = Instant::now();
    let diff = engine.compute_commit_diff(head).expect("commit diff");
    let elapsed = started.elapsed();

    assert!(!diff.files.is_empty(), "the diff must still render");
    assert!(
        elapsed < Duration::from_secs(30),
        "an oversized xfuncname must be ignored rather than compiled, took {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// T12: BOM and CRLF smuggling in attribute driver bindings
// ---------------------------------------------------------------------------

#[test]
fn test_bom_and_crlf_attribute_bindings_are_still_discovered() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo, "files");

    // `$GIT_DIR/info/attributes` is repository-controlled and is parsed by
    // `attr.c`, which tolerates a UTF-8 BOM, CRLF line endings, C-quoted
    // patterns, and vertical-tab-prefixed lines. The scanner must discover the
    // driver names in all of those shapes so it can neutralize them even when
    // the driver bodies live in configuration it cannot see.
    let info = repo.join(".git").join("info");
    fs::create_dir_all(&info).unwrap();
    let mut attributes = Vec::new();
    attributes.extend_from_slice("\u{feff}".as_bytes());
    attributes.extend_from_slice(b"* filter=bomdrv diff=bomconv\r\n");
    attributes.extend_from_slice(b"\"*.md\"filter=quoteddrv\r\n");
    attributes.extend_from_slice(b"\x0b# not-a-comment merge=vtabdrv\r\n");
    attributes.extend_from_slice(b"*.bin filter=nuldrv\x00 diff=hidden\n");
    attributes.extend_from_slice(b"*.tsv\tfilter=tabdrv\tdiff=tabconv\r\n");
    fs::write(info.join("attributes"), &attributes).unwrap();

    // A tracked worktree `.gitattributes` using the same shapes. This one is
    // defended differently: `safe_git_command` exports `GIT_ATTR_SOURCE=<empty
    // tree>`, which makes Git read path attributes from an empty tree instead
    // of the worktree, so the bindings never resolve at all.
    let mut worktree_attributes = Vec::new();
    worktree_attributes.extend_from_slice("\u{feff}".as_bytes());
    worktree_attributes.extend_from_slice(b"*.rs filter=wtdrv diff=wtconv merge=wtmerge\r\n");
    fs::write(repo.join(".gitattributes"), &worktree_attributes).unwrap();

    let args = hardened_cli_overrides(&repo);
    for driver in ["bomdrv", "quoteddrv", "tabdrv"] {
        assert_override(&args, &format!("filter.{driver}.clean="));
        assert_override(&args, &format!("filter.{driver}.smudge="));
        assert_override(&args, &format!("filter.{driver}.process="));
        assert_override(&args, &format!("filter.{driver}.required=false"));
    }
    for driver in ["bomconv", "tabconv"] {
        assert_override(&args, &format!("diff.{driver}.command="));
        assert_override(&args, &format!("diff.{driver}.textconv="));
        assert_override(&args, &format!("diff.{driver}.cachetextconv=false"));
    }
    assert_override(&args, "merge.vtabdrv.driver=");
    assert_override(&args, "filter.nuldrv.clean=");

    // `core.attributesFile` and the system attributes file must both be off.
    assert_override(&args, "core.attributesFile=/dev/null");
    let mut probe = Command::new("sh");
    tigrs_git::apply_untrusted_repo_env(&mut probe, &repo);
    let attr_env: Vec<(String, String)> = probe
        .get_envs()
        .filter_map(|(key, value)| {
            value.map(|value| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
        })
        .filter(|(key, _)| key.starts_with("GIT_ATTR"))
        .collect();
    assert!(
        attr_env
            .iter()
            .any(|(key, value)| key == "GIT_ATTR_NOSYSTEM" && value == "1"),
        "GIT_ATTR_NOSYSTEM=1 must be exported; got {attr_env:?}"
    );
    assert!(
        attr_env.iter().any(|(key, value)| key == "GIT_ATTR_SOURCE"
            && (value == "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
                || value == "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321")),
        "GIT_ATTR_SOURCE must pin attributes to the empty tree; got {attr_env:?}"
    );

    // Behavioural confirmation: the worktree binding does not resolve, while
    // the `$GIT_DIR/info/attributes` binding still does (and is therefore
    // neutralized by the `-c` overrides asserted above).
    fs::write(repo.join("lib.rs"), "fn main() {}\n").unwrap();
    let out = tigrs_git::safe_git_command(&repo)
        .args(["check-attr", "filter", "diff", "merge", "--", "lib.rs"])
        .output()
        .expect("git check-attr");
    let rendered = String::from_utf8_lossy(&out.stdout);
    assert!(
        !rendered.contains("wtdrv")
            && !rendered.contains("wtconv")
            && !rendered.contains("wtmerge"),
        "GIT_ATTR_SOURCE must suppress worktree .gitattributes bindings: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// T14: whole-surface sweep of a fully armed repository
// ---------------------------------------------------------------------------

/// Builds a repository that arms every repository-controlled execution
/// primitive `tigrs` has ever been found to touch.
///
/// The repository content and history are created *first*: a hostile
/// repository arrives fully armed, but this fixture has to use the real `git`
/// CLI to build it, and `git add` refuses to run once a required clean filter
/// is configured. Arming last reproduces the same on-disk end state.
fn build_fully_armed_repo(repo: &Path, markers: &Path, ref_format: &str) -> bool {
    use std::os::unix::fs::{PermissionsExt, symlink};

    if !init_repo(repo, ref_format) {
        return false;
    }
    let git_dir = repo.join(".git");
    let payload = |name: &str| format!("touch {}; cat", markers.join(name).display());

    // ---- Phase 1: tracked content, hostile metadata, and history. ----------
    fs::write(
        repo.join("payload.txt"),
        "before \u{1b}]52;c;cGF5bG9hZA==\u{7}\u{1b}[2J\u{202e} after\n",
    )
    .unwrap();
    fs::write(repo.join("tracked.txt"), "v1\n").unwrap();
    fs::write(
        repo.join(".gitattributes"),
        "* filter=armed diff=armed merge=armed\n",
    )
    .unwrap();
    fs::write(
        repo.join(".gitmodules"),
        "[submodule \"evil\"]\n\tpath = evil\n\turl = ext::sh -c touch% /tmp/nope\n",
    )
    .unwrap();
    symlink("../../../etc/passwd", repo.join("escape.link")).unwrap();
    symlink(".git/config", repo.join("gitdir.link")).unwrap();
    symlink("loop-b", repo.join("loop-a")).unwrap();
    symlink("loop-a", repo.join("loop-b")).unwrap();

    git(repo, &["add", "-A"]);
    let hostile_subject = "Seed \u{1b}]0;Hijack\u{7}\u{1b}[31m\u{202e}deeS";
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(repo)
        .arg("-c")
        .arg("user.name=Mallory \u{1b}[31mRed\u{1b}[0m")
        .args([
            "-c",
            "user.email=mallory@example.com",
            "commit",
            "-q",
            "-m",
            hostile_subject,
        ])
        .status()
        .expect("spawn git commit");
    assert!(status.success());

    fs::write(repo.join("tracked.txt"), "v2\n").unwrap();
    git(repo, &["commit", "-q", "-a", "-m", hostile_subject]);
    git(repo, &["tag", "-a", "v0.0.1", "-m", hostile_subject]);
    fs::write(repo.join("tracked.txt"), "v3-stashed\n").unwrap();
    git(repo, &["stash", "-q", "-u"]);
    fs::write(repo.join("tracked.txt"), "v4-dirty\n").unwrap();
    fs::write(repo.join("untracked.txt"), "brand new\n").unwrap();

    // ---- Phase 2: arm every execution primitive. ---------------------------
    // 1. Every static command-executing key.
    for (key, _) in STATIC_NEUTRALIZATIONS {
        let marker = key.replace('.', "-");
        git(repo, &["config", "--local", key, &payload(&marker)]);
    }

    // 2. Every dynamic driver family.
    for (key, marker) in [
        ("filter.armed.clean", "filter-clean"),
        ("filter.armed.smudge", "filter-smudge"),
        ("filter.armed.process", "filter-process"),
        ("diff.armed.textconv", "diff-textconv"),
        ("diff.armed.command", "diff-command"),
        ("merge.armed.driver", "merge-driver"),
        ("alias.armedalias", "alias"),
        ("pager.log", "pager-log"),
        ("pager.diff", "pager-diff"),
        ("pager.blame", "pager-blame"),
        ("pager.show", "pager-show"),
        ("pager.status", "pager-status"),
        ("credential.https://example.com.helper", "credential-url"),
        ("remote.origin.uploadpack", "remote-uploadpack"),
        ("remote.origin.receivepack", "remote-receivepack"),
        ("remote.origin.proxy", "remote-proxy"),
        ("remote.origin.vcs", "remote-vcs"),
        ("difftool.armedtool.cmd", "difftool"),
        ("mergetool.armedtool.cmd", "mergetool"),
        ("guitool.armedtool.cmd", "guitool"),
        ("man.armedtool.cmd", "mantool"),
    ] {
        git(repo, &["config", "--local", key, &payload(marker)]);
    }
    git(
        repo,
        &["config", "--local", "filter.armed.required", "true"],
    );
    git(
        repo,
        &["config", "--local", "diff.armed.cachetextconv", "true"],
    );
    git(
        repo,
        &["config", "--local", "diff.armed.xfuncname", "^(x+)+$"],
    );

    // 3. `$GIT_DIR/info/attributes` with a BOM and CRLF line endings; the
    //    tracked worktree `.gitattributes` was committed in phase 1.
    let info = git_dir.join("info");
    fs::create_dir_all(&info).unwrap();
    let mut info_attributes = Vec::new();
    info_attributes.extend_from_slice("\u{feff}".as_bytes());
    info_attributes.extend_from_slice(b"* filter=armed diff=armed merge=armed\r\n");
    fs::write(info.join("attributes"), &info_attributes).unwrap();

    // 4. Every hook Git might fire during a read-only browse.
    let hooks = git_dir.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    for name in [
        "pre-commit",
        "post-checkout",
        "post-index-change",
        "reference-transaction",
        "fsmonitor-watchman",
        "pre-auto-gc",
        "post-rewrite",
    ] {
        let hook = hooks.join(name);
        fs::write(
            &hook,
            format!("#!/bin/sh\ntouch {}\n", markers.join(name).display()),
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // 5. An include chain that hides more of the same. The chain is bounded
    //    (Git dies at `MAX_INCLUDE_DEPTH` = 10, which would make every later
    //    surface fail closed and hide real findings); cycles are covered
    //    separately by `test_config_include_bomb_and_cycle_terminate_and_are_scanned`.
    fs::write(
        git_dir.join("armed2.inc"),
        format!(
            "[filter \"includedfilter\"]\n\tclean = {}\n[alias]\n\tincludedalias = !{}\n",
            payload("included-filter-clean"),
            payload("included-alias")
        ),
    )
    .unwrap();
    fs::write(
        git_dir.join("armed.inc"),
        format!(
            "[core]\n\tfsmonitor = {}\n\tsshCommand = {}\n[include]\n\tpath = {}\n",
            payload("included-fsmonitor"),
            payload("included-sshcommand"),
            git_dir.join("armed2.inc").display()
        ),
    )
    .unwrap();
    let config_path = git_dir.join("config");
    let config = format!(
        "{}[include]\n\tpath = {}\n",
        fs::read_to_string(&config_path).unwrap(),
        git_dir.join("armed.inc").display()
    );
    fs::write(&config_path, config).unwrap();
    true
}

fn sweep_every_engine_surface(repo: &Path, markers: &Path, label: &str) {
    let engine = GitEngine::open(Some(repo)).expect("open engine");
    let cancel = CancellationToken::none();

    let chunks = engine
        .stream_commits(None, Some(64), CancellationToken::none())
        .expect("stream commits");
    let mut commits = Vec::new();
    for chunk in chunks {
        commits.extend(chunk.expect("commit chunk"));
    }
    assert!(commits.len() >= 2, "{label}: commits must still stream");
    for commit in &commits {
        assert_terminal_safe(label, &commit.summary);
        assert_terminal_safe(label, &commit.author_name);
    }

    let head = engine.head_commit_id().expect("head");
    let _ = engine.compute_commit_diff(head).expect("commit diff");
    let _ = engine.list_refs().expect("list refs");
    let _ = engine.list_stashes().expect("list stashes");
    let _ = engine.current_branch().expect("current branch");
    let _ = engine.read_reflog("HEAD").expect("reflog");
    let _ = engine.read_tree(head, "").expect("read tree");
    let _ = engine.blame_file(head, "tracked.txt").expect("blame");

    let report = engine.load_status(&cancel).expect("status");
    for item in report
        .staged
        .iter()
        .chain(report.unstaged.iter())
        .chain(report.untracked.iter())
    {
        assert_terminal_safe(label, &item.path);
        let _ = engine.compute_status_item_diff(item);
    }
    for (section, items) in [
        (StatusSection::Staged, &report.staged),
        (StatusSection::Unstaged, &report.unstaged),
        (StatusSection::Untracked, &report.untracked),
    ] {
        let _ = engine.compute_status_section_diff(section, items);
    }

    let found = marker_names(markers);
    assert!(
        found.is_empty(),
        "{label}: a hostile repository primitive executed: {found:?}"
    );
}

#[test]
fn test_fully_armed_repository_sweep_files_backend() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let markers = temp.path().join("markers");
    fs::create_dir_all(&markers).unwrap();
    assert!(build_fully_armed_repo(&repo, &markers, "files"));

    let started = Instant::now();
    sweep_every_engine_surface(&repo, &markers, "files backend");
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "the sweep must not hang"
    );
}

#[test]
fn test_fully_armed_repository_sweep_reftable_backend() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let markers = temp.path().join("markers");
    fs::create_dir_all(&markers).unwrap();
    if !build_fully_armed_repo(&repo, &markers, "reftable") {
        eprintln!("git does not support --ref-format=reftable on this system, skipping test");
        return;
    }

    let started = Instant::now();
    sweep_every_engine_surface(&repo, &markers, "reftable backend");
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "the sweep must not hang"
    );
}

// =============================================================================
// T15 — LineBuffer::from_raw_bytes UTF-8 fast-path C1 control & BiDi bypass
// =============================================================================

#[test]
fn test_blob_line_buffer_fast_path_strips_c1_controls_and_bidi_overrides() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo, "files");

    // Pure UTF-8 blob with NO ASCII C0 controls (< 0x20), designed to trigger the
    // LineBuffer::from_raw_bytes UTF-8 check while carrying:
    // - U+202E (Right-to-Left Override, 0xE2 0x80 0xAE) and U+2066..U+2069 (BiDi isolates)
    // - U+009B (8-bit CSI, 0xC2 0x9B) and U+009D (8-bit OSC, 0xC2 0x9D)
    let hostile_blob = concat!(
        "fn is_admin(user: &str) -> bool {\n",
        "    // \u{202e} } return true; // \u{202c}\n",
        "    let banner = \"\u{009b}2J\u{009d}52;c;dGVzdA==\u{009c}\u{2066}spoofed\u{2069}\";\n",
        "    user == \"admin\"\n",
        "}\n",
    );
    fs::write(repo.join("trojan_blob.rs"), hostile_blob).unwrap();
    git(&repo, &["add", "trojan_blob.rs"]);
    git(&repo, &["commit", "-m", "Add trojan_blob.rs"]);

    let engine = GitEngine::open(Some(&repo)).expect("open engine");
    let head = engine.head_commit_id().expect("head");
    let blob = engine
        .read_blob_at_commit_path(head, "trojan_blob.rs")
        .expect("read_blob_at_commit_path");
    assert!(!blob.is_binary);
    assert_eq!(blob.lines.len(), 5);

    for (idx, line) in blob.lines.iter().enumerate() {
        assert_terminal_safe(&format!("blob line {idx}"), line);
    }
    assert_eq!(&blob.lines[1], "    //  } return true; // ");
    assert_eq!(
        &blob.lines[2],
        "    let banner = \"2J52;c;dGVzdA==spoofed\";"
    );

    let raw_lines = engine
        .read_blob_raw_lines(blob.oid)
        .expect("read_blob_raw_lines");
    for (idx, line) in raw_lines.iter().enumerate() {
        assert_terminal_safe(&format!("raw blob line {idx}"), line);
    }
}

// =============================================================================
// T16 — Repository trust requires conjunction of work_dir and all git_dirs
// =============================================================================

#[test]
fn test_repo_trust_requires_conjunction_of_work_dir_and_all_git_metadata_dirs() {
    let temp = TempDir::new().unwrap();
    let trusted_work = temp.path().join("trusted_work");
    let untrusted_gitdir = temp.path().join("untrusted_gitdir");
    let untrusted_commondir = temp.path().join("untrusted_commondir");
    fs::create_dir_all(&trusted_work).unwrap();
    fs::create_dir_all(&untrusted_commondir).unwrap();

    // Initialize a real repo in untrusted_gitdir, then turn trusted_work/.git
    // into a gitfile pointing at untrusted_gitdir.
    init_repo(&untrusted_gitdir, "files");
    git(
        &untrusted_gitdir,
        &["config", "--local", "core.editor", "touch /tmp/pwned"],
    );
    let real_git_dir = untrusted_gitdir.join(".git");
    fs::write(
        trusted_work.join(".git"),
        format!("gitdir: {}\n", real_git_dir.display()),
    )
    .unwrap();
    fs::write(trusted_work.join("file.txt"), "hello\n").unwrap();

    let engine = GitEngine::open(Some(&trusted_work)).expect("open split gitdir engine");
    assert!(
        !engine.is_read_only(),
        "Every opened repository defaults to Update Mode (read_only = false)"
    );

    // Every repository is untrusted by default (no trusted_repos allowlist exists).
    let cfg = Config::default();
    let app = AppState::with_engine_and_config(Some(engine.clone()), Some(&cfg));
    assert!(
        !app.is_current_repo_trusted(&trusted_work),
        "repo must NEVER be trusted for .git/config execution (no allowlist)"
    );
    assert!(
        !app.is_current_repo_trusted(&untrusted_gitdir),
        "gitdir must NEVER be trusted for .git/config execution"
    );
    assert!(
        !app.is_current_repo_trusted(&untrusted_commondir),
        "commondir must NEVER be trusted for .git/config execution"
    );
    assert!(
        !app.is_read_only(),
        "AppState launches in Update Mode (read_only = false) by default"
    );
}

// =============================================================================
// T17 — Diff rename and copy source_location control-character sanitization
// =============================================================================

#[test]
fn test_diff_rename_and_copy_source_paths_strip_control_chars() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo, "files");

    // Create a file whose filename embeds OSC-52, 8-bit C1 CSI, and BiDi override
    // sequences with enough lines for tree diff rewrite (rename) tracking to match.
    let hostile_source_name =
        "source_\u{1b}]52;c;cGF5bG9hZA==\u{7}\u{009b}31m\u{202e}evil\u{202c}.txt";
    let content = "unchanged line for similarity tracking in rename detection\n".repeat(32);
    fs::write(repo.join(hostile_source_name), &content).unwrap();
    git(&repo, &["add", "--", hostile_source_name]);
    git(&repo, &["commit", "-m", "Add file with hostile name"]);

    git(
        &repo,
        &["mv", "--", hostile_source_name, "clean_destination.txt"],
    );
    git(
        &repo,
        &["commit", "-m", "Rename hostile file to clean name"],
    );

    let engine = GitEngine::open(Some(&repo)).expect("open engine");
    let head = engine.head_commit_id().expect("head");
    let diff = engine.compute_commit_diff(head).expect("commit diff");

    assert!(!diff.files.is_empty(), "rename commit diff must have files");
    let mut saw_sanitized_source = false;
    for file in &diff.files {
        assert_terminal_safe("diff file path", &file.path);
        match &file.status {
            tigrs_git::diff::FileChangeStatus::Renamed { source_path, .. }
            | tigrs_git::diff::FileChangeStatus::Copied { source_path, .. } => {
                assert_terminal_safe("diff rewrite source_path", source_path);
                assert_eq!(source_path, "source_31mevil.txt");
                saw_sanitized_source = true;
            }
            _ => {}
        }
    }
    assert!(
        saw_sanitized_source,
        "expected FileChangeStatus::Renamed with sanitized source_path, got {:?}",
        diff.files.iter().map(|f| &f.status).collect::<Vec<_>>()
    );
}

// =============================================================================
// T18 — DiffHunk::func_context control-char sanitization and O(N) scan bound
// =============================================================================

#[test]
fn test_diff_hunk_func_context_strips_control_chars_and_runs_in_linear_time() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo, "files");

    // Function header line starting with an identifier (`fn_entry`) followed by
    // OSC-52, 8-bit C1 CSI, and Trojan Source BiDi override sequences, followed
    // by 200 separated hunks that all scan backwards toward the top of the file.
    let mut initial_file =
        String::from("fn_entry \u{1b}]52;c;cGF5bG9hZA==\u{7}\u{009b}31m\u{202e}spoof\u{202c}()\n");
    let mut modified_file = initial_file.clone();
    for block in 0..200 {
        for pad in 0..8 {
            let line = format!("    context_{block}_{pad}\n");
            initial_file.push_str(&line);
            modified_file.push_str(&line);
        }
        initial_file.push_str("    old_val\n");
        modified_file.push_str("    new_val\n");
    }
    fs::write(repo.join("mod.rs"), &initial_file).unwrap();
    git(&repo, &["add", "mod.rs"]);
    git(
        &repo,
        &["commit", "-m", "Initial file with hostile func_context"],
    );

    fs::write(repo.join("mod.rs"), &modified_file).unwrap();
    git(&repo, &["commit", "-a", "-m", "Modify 200 separate hunks"]);

    let engine = GitEngine::open(Some(&repo)).expect("open engine");
    let head = engine.head_commit_id().expect("head");
    let started = Instant::now();
    let diff = engine.compute_commit_diff(head).expect("commit diff");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "memoized func_context scan across 200 hunks must complete quickly, took {elapsed:?}"
    );
    assert_eq!(diff.files.len(), 1);
    assert!(diff.files[0].hunks.len() >= 100);
    for (idx, hunk) in diff.files[0].hunks.iter().enumerate() {
        let ctx = hunk
            .func_context
            .as_deref()
            .expect("each hunk must resolve the function context");
        assert_terminal_safe(&format!("hunk {idx} func_context"), ctx);
        assert_eq!(ctx, "fn_entry 31mspoof()");
    }
}

// =============================================================================
// T19 — Bounded :grep execution and status/log view defense-in-depth sanitization
// =============================================================================

#[test]
fn test_grep_command_bounds_matches_and_strips_control_chars() {
    use std::fmt::Write as _;
    use tigrs_ui::app::{AppState, Flow, execute_parsed_command};
    use tigrs_ui::prompt::ParsedCommand;

    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo, "files");

    // Create 21,000 lines containing `needle` plus OSC-52, 8-bit C1 CSI, and
    // Trojan Source BiDi sequences to exercise both `MAX_GREP_MATCHES` (20,000)
    // and control-character stripping.
    let mut bulk = String::with_capacity(21_000 * 48);
    for idx in 0..21_000 {
        let _ = writeln!(
            bulk,
            "needle_{idx} \u{1b}]52;c;cGF5bG9hZA==\u{7}\u{009b}31m\u{202e}spoof\u{202c}"
        );
    }
    fs::write(repo.join("bulk.txt"), &bulk).unwrap();
    git(&repo, &["add", "bulk.txt"]);
    git(&repo, &["commit", "-m", "Add bulk grep target"]);

    let engine = GitEngine::open(Some(&repo)).expect("open engine");
    let mut app = AppState::with_engine_and_config(Some(engine), None);

    let flow = execute_parsed_command(&mut app, ParsedCommand::Grep("needle_".to_string()), 24);
    assert!(matches!(flow, Flow::Continue));
    let grep_view = app.views.grep_view.as_ref().expect("grep view populated");
    assert_eq!(
        grep_view.line_count(),
        20_000,
        "grep matches must be capped at MAX_GREP_MATCHES (20,000)"
    );

    let mut out = Vec::new();
    grep_view
        .render(&mut out, 120, 24)
        .expect("render grep view");
    let rendered = String::from_utf8(out).expect("utf8 row");
    assert!(
        !rendered.contains("\u{1b}]52")
            && !rendered.contains('\u{009b}')
            && !rendered.contains('\u{202e}'),
        "grep row leaked control or BiDi characters: {rendered:?}"
    );
}

// =============================================================================
// T20 — S0 Config HOME fallback RCE (CWE-114) and S0 Editor/Macro argument injection (CWE-88)
// =============================================================================

#[test]
fn child_config_rejects_relative_or_missing_home_and_xdg() {
    if !in_child_process() {
        return;
    }
    assert!(
        Config::default_config_path().is_none(),
        "default_config_path must return None when HOME/XDG_CONFIG_HOME/TIGRS_CONFIG are unset or relative, got {:?}",
        Config::default_config_path()
    );
    let loaded = Config::load_default();
    assert!(
        loaded.view.diff_formatter.is_empty(),
        "worktree .config/tigrs/config.toml must never be loaded: {:?}",
        loaded.view.diff_formatter
    );
    assert!(
        !loaded.general.read_only && !loaded.security.trust_external_hooks,
        "worktree .config/tigrs/config.toml must never enable hooks or load untrusted config"
    );
}

#[test]
fn test_s0_config_home_fallback_and_editor_control_char_argument_injection() {
    use tigrs_core::macro_ctx::MacroContext;
    use tigrs_ui::editor::{EditTarget, build_editor_command_line};

    // 1. Verify S0 CWE-114: relative or unset HOME / XDG_CONFIG_HOME / TIGRS_CONFIG
    // never falls back to `.` (loading `.config/tigrs/config.toml` from the repo worktree).
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo, "files");
    let hostile_cfg_dir = repo.join(".config").join("tigrs");
    fs::create_dir_all(&hostile_cfg_dir).unwrap();
    fs::write(
        hostile_cfg_dir.join("config.toml"),
        "[security]\ntrusted_repos = [\".\"]\n[view]\ndiff_formatter = \"touch /tmp/pwned\"\n",
    )
    .unwrap();

    // Case A: HOME, XDG_CONFIG_HOME, TIGRS_CONFIG completely unset
    run_child_test(
        "child_config_rejects_relative_or_missing_home_and_xdg",
        &repo,
        &[],
        &["HOME", "XDG_CONFIG_HOME", "TIGRS_CONFIG"],
    );
    // Case B: HOME, XDG_CONFIG_HOME, TIGRS_CONFIG set to relative paths
    run_child_test(
        "child_config_rejects_relative_or_missing_home_and_xdg",
        &repo,
        &[
            ("HOME", Path::new(".")),
            ("XDG_CONFIG_HOME", Path::new(".config")),
            ("TIGRS_CONFIG", Path::new(".config/tigrs/config.toml")),
        ],
        &[],
    );

    // 2. Verify S0 CWE-88: leading control/ANSI/BiDi characters (`\x1b-c!sh`, `\x08+!sh`,
    // `\u{202e}--upload-pack=evil`) cannot bypass the `./` prefix in `build_editor_command_line`
    // or `MacroContext` before `shell_quote` strips control characters.
    for (hostile_path, expected_cmd) in [
        ("\u{1b}[0m-c!sh", "vim './-c!sh'"),
        ("\u{0008}+!sh", "vim './+!sh'"),
        ("\u{009b}-c:!sh", "vim './-c:!sh'"),
        ("\u{202e}-c!sh", "vim './-c!sh'"),
    ] {
        let target = EditTarget {
            path: hostile_path.to_string(),
            line: None,
            temp: false,
        };
        let cmd = build_editor_command_line("vim", &target, false);
        assert_eq!(
            cmd, expected_cmd,
            "hostile path {hostile_path:?} must be prefixed with ./ after stripping control chars"
        );
    }

    let ctx = MacroContext {
        file: Some("\u{1b}[0m-c!sh".to_string()),
        file_old: Some("\u{0008}+!sh".to_string()),
        branch: Some("\u{202e}--upload-pack=evil".to_string()),
        ..Default::default()
    };
    assert_eq!(ctx.get_var("file").as_deref(), Some("./-c!sh"));
    assert_eq!(ctx.get_var("file_old").as_deref(), Some("./+!sh"));
    assert_eq!(ctx.get_var("branch").as_deref(), Some("upload-pack=evil"));

    // 3. Verify S0 CWE-78: `$(...)` inside double quotes (`echo "$(echo %(branch))"`)
    // must NOT wrap the single-quoted macro payload in inner double quotes (`"'$(...)'"`).
    let marker_sub = temp.path().join("PWNED_T20_SUBSHELL");
    let sub_ctx = MacroContext {
        branch: Some(format!("$(touch {})", marker_sub.display())),
        ..Default::default()
    };
    let expanded = sub_ctx
        .expand("echo \"$(printf '%s' %(branch))\"", |_| None)
        .expect("macro expansion inside $() within double quotes must succeed");
    assert!(
        !expanded.contains("\"'$("),
        "expanded command must not wrap single-quoted payload in inner double quotes: {expanded}"
    );
    tigrs_core::quote::verify_shell_safety(&expanded)
        .expect("expanded $() subshell command must pass verify_shell_safety");
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(&expanded)
        .output()
        .expect("sh -c must execute safely");
    assert!(out.status.success());
    assert!(
        !marker_sub.exists(),
        "S0 RCE: $() command substitution inside double quotes executed payload!"
    );

    // Also verify trailing backslash before macro (`echo \%(branch)`) neutralizes `\` into `\\`
    // so an attacker-controlled branch `$(touch ...)'` cannot unquote its opening single quote.
    let bs_ctx = MacroContext {
        branch: Some(format!("$(touch {})'", marker_sub.display())),
        ..Default::default()
    };
    let bs_expanded = bs_ctx
        .expand("echo \\%(branch)", |_| None)
        .expect("macro expansion after trailing backslash must succeed");
    tigrs_core::quote::verify_shell_safety(&bs_expanded)
        .expect("trailing-backslash macro expansion must pass verify_shell_safety");
    let bs_out = std::process::Command::new("sh")
        .arg("-c")
        .arg(&bs_expanded)
        .output()
        .expect("sh -c must execute safely");
    assert!(bs_out.status.success());
    assert!(
        !marker_sub.exists(),
        "S0 RCE: trailing backslash before macro escaped opening single quote!"
    );

    // 4. Verify S0 CWE-88: Git pathspec magic prefixes (`:(top)...`, `:/...`, `:!...`, `:^...`)
    // are rejected by `verify_relative_path` and `safe_git_command` enforces `GIT_LITERAL_PATHSPECS=1`.
    for magic in [
        ":(top)sym/passwd",
        ":(icase)sym/passwd",
        ":/sym/passwd",
        ":!sym",
        ":^sym",
    ] {
        assert!(
            tigrs_git::path_security::verify_relative_path(magic).is_err(),
            "pathspec magic {magic:?} must be rejected by verify_relative_path"
        );
    }
    let git_cmd = tigrs_git::path_security::safe_git_command(&repo);
    let envs: std::collections::HashMap<_, _> = git_cmd.get_envs().collect();
    assert_eq!(
        envs.get(std::ffi::OsStr::new("GIT_LITERAL_PATHSPECS"))
            .and_then(|v| *v),
        Some(std::ffi::OsStr::new("1")),
        "safe_git_command must export GIT_LITERAL_PATHSPECS=1"
    );
    assert!(
        envs.get(std::ffi::OsStr::new("GIT_WORK_TREE"))
            .and_then(|v| *v)
            .is_some(),
        "safe_git_command must pin GIT_WORK_TREE for worktree repositories"
    );

    // 5. Verify S0 CWE-22: `.git/config` (and `[include]`d config) with `core.worktree`
    // pointing outside the repository is rejected by `discover_repository`.
    let outside_wt = temp.path().join("outside_wt");
    std::fs::create_dir_all(&outside_wt).unwrap();
    let inc_cfg = repo.join(".git").join("escape.inc");
    std::fs::write(
        &inc_cfg,
        format!("[core]\n\tworktree = {}\n", outside_wt.display()),
    )
    .unwrap();
    let cfg_path = repo.join(".git").join("config");
    let orig_cfg = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    std::fs::write(
        &cfg_path,
        format!("{orig_cfg}\n[include]\n\tpath = escape.inc\n"),
    )
    .unwrap();
    assert!(
        tigrs_git::discovery::discover_repository(Some(&repo)).is_err(),
        "discover_repository must reject core.worktree escaping work_dir (even via [include])"
    );
}
