// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Editor integration for opening files from views in an external editor.
//!
//! Replicates upstream Tig's `e` / `edit` action (`REQ_EDIT`), with parity across
//! editor command resolution precedence, POSIX shell path escaping, hunk line-number
//! computation, and per-view target resolution.

use crate::ViewKind;
use crate::app::AppState;
use crate::view::TreeRow;
use tigrs_git::{DiffHunk, DiffLineKind, FileChangeStatus};

/// A file target to be opened in an external editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditTarget {
    /// Repository-root-relative file path.
    pub path: String,
    /// 1-based line number; `None` indicates no line number (upstream `lineno == 0`).
    pub line: Option<u32>,
    /// Whether this target is a temporary file that must be cleaned up after exit.
    pub temp: bool,
}

/// Reasons why an editor invocation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditRefusal {
    /// Upstream: "Nothing to edit"
    NothingToEdit,
    /// Upstream: "File has been deleted."
    Deleted,
    /// Upstream: "Edit only supported for files"
    NotAFile,
    /// Upstream: "Failed to open file: {0}"
    Unreadable(String),
}

impl EditRefusal {
    /// User-visible status message matching upstream Tig's messages.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NothingToEdit => "Nothing to edit".to_string(),
            Self::Deleted => "File has been deleted.".to_string(),
            Self::NotAFile => "Edit only supported for files".to_string(),
            Self::Unreadable(path) => format!("Failed to open file: {path}"),
        }
    }
}

/// Pending editor invocation to be executed during TTY handover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorInvocation {
    /// Fully formatted shell command line (e.g. `vim +42 'src/main.rs'`).
    pub command_line: String,
    /// Resolved target for reporting and potential cleanup.
    pub target: EditTarget,
    /// Whether a line number argument was passed. If true and the editor fails,
    /// line numbers are disabled for the remainder of the session.
    pub used_line_number: bool,
}

/// Computes the 1-based line number in the *new* side of the file corresponding to
/// `hunk.lines[idx]`.
///
/// Mirrors upstream `diff_get_lineno(view, line, /*old=*/false)` (src/diff.c:625-654):
/// Starts at `hunk.new_start` and increments for every line strictly preceding `idx`
/// that is not a deletion. Cursor on a deletion line therefore yields the new-side
/// line that the deletion precedes.
#[must_use]
pub fn hunk_new_lineno(hunk: &DiffHunk, idx: usize) -> u32 {
    let clamped_idx = idx.min(hunk.lines.len());
    let offset = hunk.lines[..clamped_idx]
        .iter()
        .filter(|l| l.kind != DiffLineKind::Remove)
        .count() as u32;
    hunk.new_start + offset
}

/// Resolves the editor command string following upstream Tig's exact precedence:
///
/// `$TIG_EDITOR` -> `$GIT_EDITOR` -> `core.editor` -> `$VISUAL` -> `$EDITOR` -> `"vi"`.
///
/// Per Decision 4, empty or whitespace-only values are treated as unset.
/// Uses an injected environment lookup closure for safe, deterministic testing.
#[must_use]
pub fn resolve_editor(env: &dyn Fn(&str) -> Option<String>, core_editor: Option<&str>) -> String {
    let clean = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    clean(env("TIG_EDITOR"))
        .or_else(|| clean(env("GIT_EDITOR")))
        .or_else(|| clean(core_editor.map(str::to_owned)))
        .or_else(|| clean(env("VISUAL")))
        .or_else(|| clean(env("EDITOR")))
        .unwrap_or_else(|| "vi".to_string())
}

/// Builds the complete shell command string for executing the editor.
///
/// Mirrors upstream `src/display.c:171-181`:
/// - The editor command itself is deliberately NOT escaped (allowing commands like `code -w`).
/// - The file path is always escaped using POSIX single-quoting via `tigrs_core::quote::shell_quote`.
/// - `+LINE` precedes the file path when line numbers are enabled and present.
#[must_use]
pub fn build_editor_command_line(editor: &str, target: &EditTarget, line_numbers: bool) -> String {
    // Strip control characters FIRST so a leading ESC/control/BiDi sequence (e.g. `\x1b-c!sh`
    // or `\x08+!sh`) cannot mask a leading `-` or `+` before `shell_quote` strips it (CWE-88).
    let cleaned_path = tigrs_core::ansi::strip_control_chars(&target.path);
    let safe_path = if cleaned_path.starts_with('-') || cleaned_path.starts_with('+') {
        format!("./{cleaned_path}")
    } else {
        cleaned_path.into_owned()
    };
    let quoted_path = tigrs_core::quote::shell_quote(&safe_path);
    match target.line.filter(|_| line_numbers) {
        Some(n) => format!("{editor} +{n} {quoted_path}"),
        None => format!("{editor} {quoted_path}"),
    }
}

/// Validates that the target path exists, is free of path traversal, does not access .git metadata, and resides within the working tree.
fn validate_target_readable(
    app: &AppState,
    mut target: EditTarget,
) -> Result<EditTarget, EditRefusal> {
    if tigrs_core::ansi::strip_control_chars(&target.path) != target.path {
        return Err(EditRefusal::Unreadable(target.path));
    }
    let Ok(clean_rel) = tigrs_git::verify_relative_path(&target.path) else {
        return Err(EditRefusal::Unreadable(target.path));
    };
    target.path = clean_rel;
    if let Some(ref engine) = app.engine {
        if engine.info().is_bare {
            return Err(EditRefusal::NothingToEdit);
        }
        let work_dir = engine.work_dir();
        let Ok(safe_path) = tigrs_git::verify_worktree_path_safety(work_dir, &target.path) else {
            return Err(EditRefusal::Unreadable(target.path));
        };
        if !safe_path.exists() {
            return Err(EditRefusal::Unreadable(target.path));
        }
    }
    Ok(target)
}

/// Resolves the edit target for the currently active view in `app`.
///
/// Returns `Ok(EditTarget)` if the active view has an editable file under cursor,
/// or `Err(EditRefusal)` with the specific refusal reason.
pub fn resolve_edit_target(app: &AppState) -> Result<EditTarget, EditRefusal> {
    match app.active_view() {
        Some(ViewKind::Status) => {
            let item = app
                .views
                .status_view
                .as_ref()
                .and_then(|s| s.selected_item())
                .ok_or(EditRefusal::NothingToEdit)?;
            if item.status_code == 'D' {
                return Err(EditRefusal::Deleted);
            }
            let target = EditTarget {
                path: item.path.clone(),
                line: None,
                temp: false,
            };
            validate_target_readable(app, target)
        }

        Some(ViewKind::Diff) => {
            let diff = app
                .views
                .diff_view
                .as_ref()
                .ok_or(EditRefusal::NothingToEdit)?;

            let (path, line) = if let Some((file, hunk, line_idx)) = diff.selected_line() {
                if file.status == FileChangeStatus::Deleted || file.path == "/dev/null" {
                    return Err(EditRefusal::Deleted);
                }
                (file.path.clone(), Some(hunk_new_lineno(hunk, line_idx)))
            } else if let Some((file, hunk)) = diff.selected_hunk() {
                if file.status == FileChangeStatus::Deleted || file.path == "/dev/null" {
                    return Err(EditRefusal::Deleted);
                }
                (file.path.clone(), Some(hunk.new_start))
            } else if let Some(file) = diff.current_file_diff() {
                if file.status == FileChangeStatus::Deleted || file.path == "/dev/null" {
                    return Err(EditRefusal::Deleted);
                }
                (file.path.clone(), None)
            } else if let Some(path) = diff.current_file() {
                if path == "/dev/null" {
                    return Err(EditRefusal::Deleted);
                }
                (path.to_string(), None)
            } else {
                return Err(EditRefusal::NothingToEdit);
            };

            let target = EditTarget {
                path,
                line,
                temp: false,
            };
            validate_target_readable(app, target)
        }

        Some(ViewKind::Blob) => {
            let blob = app
                .views
                .blob_view
                .as_ref()
                .ok_or(EditRefusal::NothingToEdit)?;
            let line = (blob.cursor() + 1) as u32;
            let target = EditTarget {
                path: blob.path().to_string(),
                line: Some(line),
                temp: false,
            };
            validate_target_readable(app, target)
        }

        Some(ViewKind::Tree) => {
            let tree = app
                .views
                .tree_view
                .as_ref()
                .ok_or(EditRefusal::NothingToEdit)?;
            let row = tree.selected_row().ok_or(EditRefusal::NothingToEdit)?;
            match row {
                TreeRow::ParentDir => Err(EditRefusal::NotAFile),
                TreeRow::Entry(entry) => {
                    if entry.is_dir() {
                        Err(EditRefusal::NotAFile)
                    } else {
                        let target = EditTarget {
                            path: entry.path.clone(),
                            line: None,
                            temp: false,
                        };
                        validate_target_readable(app, target)
                    }
                }
            }
        }

        Some(ViewKind::Grep) => {
            let grep = app
                .views
                .grep_view
                .as_ref()
                .ok_or(EditRefusal::NothingToEdit)?;
            let m = grep.selected_match().ok_or(EditRefusal::NothingToEdit)?;
            let target = EditTarget {
                path: m.path.clone(),
                line: Some(m.line_num as u32),
                temp: false,
            };
            validate_target_readable(app, target)
        }

        Some(ViewKind::Blame) => {
            let blame = app
                .views
                .blame_view
                .as_ref()
                .ok_or(EditRefusal::NothingToEdit)?;
            let line = blame
                .selected_line()
                .map(|l| l.line_number as u32)
                .or_else(|| Some((blame.cursor() + 1) as u32));
            let target = EditTarget {
                path: blame.path().to_string(),
                line,
                temp: false,
            };
            validate_target_readable(app, target)
        }

        Some(ViewKind::Log) => {
            let log = app
                .views
                .log_view
                .as_ref()
                .ok_or(EditRefusal::NothingToEdit)?;
            if let Some((path, _commit_id)) = log.selected_file() {
                let target = EditTarget {
                    path: path.to_string(),
                    line: None,
                    temp: false,
                };
                validate_target_readable(app, target)
            } else {
                Err(EditRefusal::NothingToEdit)
            }
        }

        _ => Err(EditRefusal::NothingToEdit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tigrs_git::{
        DiffHunk, DiffLineKind, DiffSummaryStats, FileChangeStatus, FileDiff, HunkLine, ObjectId,
        StatusItem, StatusSection,
    };

    #[test]
    fn test_hunk_new_lineno_various_positions() {
        let hunk = DiffHunk {
            old_start: 10,
            old_len: 5,
            new_start: 20,
            new_len: 6,
            func_context: None,
            lines: vec![
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "context 1".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Remove,
                    content: "old line".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Add,
                    content: "new line 1".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Add,
                    content: "new line 2".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "context 2".to_string(),
                    no_newline_at_eof: false,
                },
            ],
        };

        // idx = 0: first line (context 1) -> new_start (20)
        assert_eq!(hunk_new_lineno(&hunk, 0), 20);

        // idx = 1: deletion line -> new_start + 1 context line = 21
        assert_eq!(hunk_new_lineno(&hunk, 1), 21);

        // idx = 2: first addition -> 1 context line before it, deletion ignored = 21
        // (the deletion did not advance new_start, so the first addition sits at 21)
        assert_eq!(hunk_new_lineno(&hunk, 2), 21);

        // idx = 3: second addition -> 1 context + 1 addition before it = 22
        assert_eq!(hunk_new_lineno(&hunk, 3), 22);

        // idx = 4: context 2 -> 1 context + 2 additions before it = 23
        assert_eq!(hunk_new_lineno(&hunk, 4), 23);

        // idx >= lines.len(): beyond end
        assert_eq!(hunk_new_lineno(&hunk, 10), 24);
    }

    #[test]
    fn test_resolve_editor_precedence() {
        let make_env = |vars: &[(&'static str, &'static str)]| {
            let map: std::collections::HashMap<String, String> = vars
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            move |key: &str| map.get(key).cloned()
        };

        // 1. TIG_EDITOR overrides everything
        let env = make_env(&[
            ("TIG_EDITOR", "tig-edit"),
            ("GIT_EDITOR", "git-edit"),
            ("VISUAL", "vis-edit"),
            ("EDITOR", "ed-edit"),
        ]);
        assert_eq!(resolve_editor(&env, Some("core-edit")), "tig-edit");

        // 2. GIT_EDITOR overrides core.editor and below
        let env = make_env(&[
            ("GIT_EDITOR", "git-edit"),
            ("VISUAL", "vis-edit"),
            ("EDITOR", "ed-edit"),
        ]);
        assert_eq!(resolve_editor(&env, Some("core-edit")), "git-edit");

        // 3. core.editor overrides VISUAL and below
        let env = make_env(&[("VISUAL", "vis-edit"), ("EDITOR", "ed-edit")]);
        assert_eq!(resolve_editor(&env, Some("core-edit")), "core-edit");

        // 4. VISUAL overrides EDITOR
        let env = make_env(&[("VISUAL", "vis-edit"), ("EDITOR", "ed-edit")]);
        assert_eq!(resolve_editor(&env, None), "vis-edit");

        // 5. EDITOR overrides default
        let env = make_env(&[("EDITOR", "ed-edit")]);
        assert_eq!(resolve_editor(&env, None), "ed-edit");

        // 6. Default fallback is "vi"
        let env = make_env(&[]);
        assert_eq!(resolve_editor(&env, None), "vi");

        // 7. Empty or whitespace env variables are treated as unset
        let env = make_env(&[
            ("TIG_EDITOR", "   "),
            ("GIT_EDITOR", ""),
            ("VISUAL", "  \t "),
            ("EDITOR", "subl"),
        ]);
        assert_eq!(resolve_editor(&env, Some("")), "subl");
    }

    #[test]
    fn test_build_editor_command_line() {
        let target_with_line = EditTarget {
            path: "src/app.rs".to_string(),
            line: Some(42),
            temp: false,
        };

        // With line numbers enabled
        assert_eq!(
            build_editor_command_line("vim", &target_with_line, true),
            "vim +42 'src/app.rs'"
        );

        // With line numbers disabled
        assert_eq!(
            build_editor_command_line("vim", &target_with_line, false),
            "vim 'src/app.rs'"
        );

        // Target without line number
        let target_no_line = EditTarget {
            path: "README.md".to_string(),
            line: None,
            temp: false,
        };
        assert_eq!(
            build_editor_command_line("nano", &target_no_line, true),
            "nano 'README.md'"
        );

        // Multi-word editor string passed unescaped
        assert_eq!(
            build_editor_command_line("code -w -g", &target_with_line, true),
            "code -w -g +42 'src/app.rs'"
        );

        // Path with spaces and single quotes
        let target_tricky = EditTarget {
            path: "dir/it's a tricky file.rs".to_string(),
            line: Some(10),
            temp: false,
        };
        assert_eq!(
            build_editor_command_line("vim", &target_tricky, true),
            "vim +10 'dir/it'\\''s a tricky file.rs'"
        );
    }

    #[test]
    fn test_resolve_edit_target_status_view() {
        let mut app = AppState::default();
        // Empty status view -> NothingToEdit
        app.push_view(ViewKind::Status);
        assert_eq!(resolve_edit_target(&app), Err(EditRefusal::NothingToEdit));

        // Status view with a modified file
        let report = tigrs_git::StatusReport {
            staged: vec![StatusItem::new(
                'M',
                StatusSection::Staged,
                "src/main.rs",
                None,
            )],
            unstaged: vec![],
            untracked: vec![],
            unmerged: vec![],
            branch: "main".to_string(),
            head_commit: None,
        };
        app.views.status_view = Some(crate::view::StatusView::new(report));
        let target = resolve_edit_target(&app).expect("target for status M");
        assert_eq!(target.path, "src/main.rs");
        assert_eq!(target.line, None);

        // Status view with deleted file -> Deleted refusal
        let del_report = tigrs_git::StatusReport {
            staged: vec![StatusItem::new(
                'D',
                StatusSection::Staged,
                "deleted.txt",
                None,
            )],
            unstaged: vec![],
            untracked: vec![],
            unmerged: vec![],
            branch: "main".to_string(),
            head_commit: None,
        };
        app.views.status_view = Some(crate::view::StatusView::new(del_report));
        assert_eq!(resolve_edit_target(&app), Err(EditRefusal::Deleted));
    }

    #[test]
    fn test_resolve_edit_target_diff_view_deleted_file() {
        let mut app = AppState::default();
        let commit_diff = tigrs_git::CommitDiff {
            commit_id: ObjectId::null(gix::hash::Kind::Sha1),
            parent_ids: vec![],
            author_name: Arc::from(""),
            author_email: Arc::from(""),
            author_date: String::new(),
            committer_name: Arc::from(""),
            committer_email: Arc::from(""),
            committer_date: String::new(),
            title: Arc::from("Test diff"),
            body: None,
            files: vec![FileDiff {
                path: "gone.rs".to_string(),
                status: FileChangeStatus::Deleted,
                old_id: None,
                new_id: None,
                old_mode: None,
                new_mode: None,
                is_binary: false,
                additions: 0,
                deletions: 10,
                hunks: vec![],
            }],
            stats: DiffSummaryStats::default(),
        };
        let mut diff_view = crate::view::DiffView::new(commit_diff);
        diff_view.next_file(24);
        app.views.diff_view = Some(diff_view);
        app.push_view(ViewKind::Diff);

        assert_eq!(resolve_edit_target(&app), Err(EditRefusal::Deleted));
    }

    #[test]
    fn test_resolve_edit_target_grep_view() {
        let mut app = AppState::default();
        let matches = vec![crate::view::GrepMatch {
            path: "crates/tigrs-ui/src/lib.rs".to_string(),
            line_num: 42,
            content: "pub mod editor;".to_string(),
        }];
        app.views.grep_view = Some(crate::view::GrepView::new("editor".to_string(), matches));
        app.push_view(ViewKind::Grep);

        let target = resolve_edit_target(&app).expect("grep target");
        assert_eq!(target.path, "crates/tigrs-ui/src/lib.rs");
        assert_eq!(target.line, Some(42));
    }

    #[test]
    fn test_resolve_edit_target_tree_view() {
        let mut app = AppState::default();
        let listing = tigrs_git::TreeListing {
            commit_oid: ObjectId::null(gix::hash::Kind::Sha1),
            path: String::new(),
            parent_path: None,
            entries: vec![
                tigrs_git::TreeEntry {
                    name: "src".to_string(),
                    path: "src".to_string(),
                    kind: tigrs_git::TreeEntryKind::Tree,
                    mode: 0o040_000,
                    oid: ObjectId::null(gix::hash::Kind::Sha1),
                    size: None,
                },
                tigrs_git::TreeEntry {
                    name: "Cargo.toml".to_string(),
                    path: "Cargo.toml".to_string(),
                    kind: tigrs_git::TreeEntryKind::Blob,
                    mode: 0o100_644,
                    oid: ObjectId::null(gix::hash::Kind::Sha1),
                    size: Some(500),
                },
            ],
        };

        let tree_view = crate::view::TreeView::new(listing);
        // Cursor starts at 0 (directory: src)
        app.views.tree_view = Some(tree_view);
        app.push_view(ViewKind::Tree);

        assert_eq!(resolve_edit_target(&app), Err(EditRefusal::NotAFile));

        // Move cursor to file (Cargo.toml)
        if let Some(ref mut t) = app.views.tree_view {
            t.move_down(24);
        }
        let target = resolve_edit_target(&app).expect("file in tree");
        assert_eq!(target.path, "Cargo.toml");
        assert_eq!(target.line, None);
    }

    #[test]
    fn test_resolve_edit_target_blob_view() {
        let mut app = AppState::default();
        let blob = tigrs_git::BlobContent {
            oid: ObjectId::null(gix::hash::Kind::Sha1),
            path: "src/lib.rs".to_string(),
            size: 100,
            is_binary: false,
            lines: vec![
                "line 1".to_string(),
                "line 2".to_string(),
                "line 3".to_string(),
                "line 4".to_string(),
                "line 5".to_string(),
            ]
            .into(),
        };

        let mut blob_view = crate::view::BlobView::new(ObjectId::null(gix::hash::Kind::Sha1), blob);
        blob_view.set_cursor(2, 24); // 0-indexed cursor at 2 -> line 3
        app.views.blob_view = Some(blob_view);
        app.push_view(ViewKind::Blob);

        let target = resolve_edit_target(&app).expect("blob target");
        assert_eq!(target.path, "src/lib.rs");
        assert_eq!(target.line, Some(3));
    }

    #[test]
    fn test_resolve_edit_target_blame_view() {
        let mut app = AppState::default();
        let oid = ObjectId::null(gix::hash::Kind::Sha1);
        let res = tigrs_git::BlameResult {
            commit_id: oid,
            path: "src/main.rs".to_string(),
            is_binary: false,
            lines: vec![
                tigrs_git::BlameLine {
                    line_number: 1,
                    commit_id: oid,
                    short_commit_id: Arc::from("11111111"),
                    author: Arc::from("Tester"),
                    author_date: Arc::from("2026-09-14"),
                    summary: Arc::from("Commit 1"),
                    content: "fn main() {".to_string(),
                    is_hunk_start: true,
                    parent_commit_id: None,
                    source_path: None,
                    source_line_number: 1,
                },
                tigrs_git::BlameLine {
                    line_number: 2,
                    commit_id: oid,
                    short_commit_id: Arc::from("11111111"),
                    author: Arc::from("Tester"),
                    author_date: Arc::from("2026-09-14"),
                    summary: Arc::from("Commit 1"),
                    content: "    println!(\"hello\");".to_string(),
                    is_hunk_start: false,
                    parent_commit_id: None,
                    source_path: None,
                    source_line_number: 2,
                },
            ],
        };

        let mut blame_view = crate::view::BlameView::from_result(res);
        blame_view.set_cursor(1, 24); // cursor at index 1 -> line_number 2
        app.views.blame_view = Some(blame_view);
        app.push_view(ViewKind::Blame);

        let target = resolve_edit_target(&app).expect("blame target");
        assert_eq!(target.path, "src/main.rs");
        assert_eq!(target.line, Some(2));
    }

    #[test]
    fn test_execute_action_edit_sets_pending_editor() {
        let mut app = AppState::default();
        let matches = vec![crate::view::GrepMatch {
            path: "crates/tigrs-ui/src/app.rs".to_string(),
            line_num: 120,
            content: "pub fn run_app".to_string(),
        }];
        app.views.grep_view = Some(crate::view::GrepView::new("run_app".to_string(), matches));
        app.push_view(ViewKind::Grep);

        assert!(!app.options.read_only);

        // Blocked when Read-Only mode is enabled
        app.options.read_only = true;
        let flow_ro = crate::app::execute_action(&mut app, &crate::keymap::Action::Edit, 24);
        assert_eq!(flow_ro, crate::app::Flow::Continue);
        assert!(app.pending_editor.is_none());
        assert_eq!(
            app.status_message.as_deref(),
            Some(crate::app::READ_ONLY_WARNING_MSG)
        );

        // Unlock Update Mode
        app.options.read_only = false;
        let flow = crate::app::execute_action(&mut app, &crate::keymap::Action::Edit, 24);
        assert_eq!(flow, crate::app::Flow::Continue);

        let inv = app.pending_editor.expect("pending editor should be set");
        assert_eq!(inv.target.path, "crates/tigrs-ui/src/app.rs");
        assert_eq!(inv.target.line, Some(120));
        assert!(inv.used_line_number);
        assert!(
            inv.command_line
                .contains("+120 'crates/tigrs-ui/src/app.rs'")
        );
    }
}
