// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Navigation, drill-down (`Enter`), parent navigation, and hunk/file jumps.

use crate::app::dispatch::{open_diff_for_main_selection, refresh_diff_for_main_selection};
use crate::app::layout::ViewKind;
use crate::app::{AppState, Flow};
use crate::view::{BlameView, BlobView, LogView, TreeRow, TreeView};

/// Handles the `Enter` action: drilling down into commits, diffs, directories, or blobs.
pub fn handle_enter(app: &mut AppState, visible_height: usize) -> Flow {
    match app.active_view() {
        Some(ViewKind::Main) => open_diff_for_main_selection(app),
        Some(ViewKind::Status) => {
            let item = app
                .views
                .status_view
                .as_ref()
                .and_then(|s| s.selected_item().cloned());
            let section = app
                .views
                .status_view
                .as_ref()
                .and_then(crate::view::status_view::StatusView::selected_section);
            if let Some(engine) = &app.engine {
                if let Some(item) = item {
                    match engine.compute_status_item_diff(&item) {
                        Ok(diff_data) => {
                            app.views.diff_view =
                                Some(app.create_status_diff_view(diff_data, item));
                            app.push_view(ViewKind::Diff);
                            app.views.maximized = false;
                        }
                        Err(e) => {
                            app.status_message = Some(format!("Failed to load diff: {e}"));
                        }
                    }
                } else if let Some(sec) = section {
                    let kind_opt = match sec {
                        tigrs_git::StatusSection::Staged => Some(crate::view::ChangesKind::Staged),
                        tigrs_git::StatusSection::Unstaged => {
                            Some(crate::view::ChangesKind::Unstaged)
                        }
                        tigrs_git::StatusSection::Untracked => {
                            Some(crate::view::ChangesKind::Untracked)
                        }
                        tigrs_git::StatusSection::Unmerged => None,
                    };
                    let generation = engine.generation();
                    if let Some(kind) = kind_opt
                        && let Some((_, cached_view)) =
                            app.changes_diff_cache.get(&(kind, generation))
                    {
                        app.views.diff_view = Some(cached_view.clone());
                        app.push_view(ViewKind::Diff);
                        app.views.maximized = false;
                        return Flow::Continue;
                    }
                    let items: Vec<tigrs_git::StatusItem> = app
                        .views
                        .status_view
                        .as_ref()
                        .map(|s| {
                            let r = s.report();
                            match sec {
                                tigrs_git::StatusSection::Staged => r.staged.clone(),
                                tigrs_git::StatusSection::Unstaged => r.unstaged.clone(),
                                tigrs_git::StatusSection::Untracked => r.untracked.clone(),
                                tigrs_git::StatusSection::Unmerged => r.unmerged.clone(),
                            }
                        })
                        .unwrap_or_default();
                    let (_src, token) = tigrs_core::cancel::CancellationToken::new();
                    match engine.compute_status_section_diff_cancellable(sec, &items, &token) {
                        Ok(diff_data) => {
                            let diff_arc = std::sync::Arc::new(diff_data);
                            let view = app.create_diff_view(std::sync::Arc::clone(&diff_arc));
                            if let Some(kind) = kind_opt {
                                app.changes_diff_cache
                                    .insert((kind, generation), (diff_arc, view.clone()));
                            }
                            app.views.diff_view = Some(view);
                            app.push_view(ViewKind::Diff);
                            app.views.maximized = false;
                        }
                        Err(e) => {
                            app.status_message = Some(format!("Failed to load section diff: {e}"));
                        }
                    }
                }
            }
        }
        Some(ViewKind::Tree) => {
            let selected = app
                .views
                .tree_view
                .as_ref()
                .and_then(|t| t.selected_row().cloned());
            let commit_oid = app.views.tree_view.as_ref().map(TreeView::commit_oid);
            if let (Some(row), Some(commit_id)) = (selected, commit_oid) {
                match row {
                    TreeRow::ParentDir => {
                        let parent = app
                            .views
                            .tree_view
                            .as_ref()
                            .and_then(|t| t.parent_path())
                            .unwrap_or("")
                            .to_string();
                        if let Some(engine) = &app.engine
                            && let Ok(listing) = engine.read_tree(commit_id, &parent)
                            && let Some(ref mut tree) = app.views.tree_view
                        {
                            tree.set_listing(listing);
                        }
                    }
                    TreeRow::Entry(entry) => {
                        if entry.is_dir() {
                            let subpath = entry.path.clone();
                            if let Some(engine) = &app.engine
                                && let Ok(listing) = engine.read_tree(commit_id, &subpath)
                                && let Some(ref mut tree) = app.views.tree_view
                            {
                                tree.set_listing(listing);
                            }
                        } else {
                            let oid = entry.oid;
                            let file_path = entry.path.clone();
                            let is_commit_entry = entry.kind == tigrs_git::TreeEntryKind::Commit;
                            if let Some(engine) = &app.engine {
                                let blob_res = if is_commit_entry {
                                    Ok(tigrs_git::BlobContent {
                                        oid,
                                        path: file_path,
                                        size: 0,
                                        is_binary: false,
                                        lines: tigrs_core::LineBuffer::from(vec![format!(
                                            "Subproject commit {oid}"
                                        )]),
                                    })
                                } else {
                                    engine.read_blob(oid, &file_path)
                                };
                                if let Ok(blob) = blob_res {
                                    app.views.blob_view = Some(BlobView::new_with_options(
                                        commit_id,
                                        blob,
                                        &app.options,
                                    ));
                                    app.push_view(ViewKind::Blob);
                                    app.views.maximized = false;
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(ViewKind::Blame) => {
            let commit_id = app
                .views
                .blame_view
                .as_ref()
                .and_then(BlameView::current_commit_id);
            if let (Some(commit_id), Some(engine)) = (commit_id, &app.engine)
                && let Ok(diff_data) = engine.compute_commit_diff_cached(commit_id)
            {
                app.views.diff_view = Some(app.create_diff_view(diff_data));
                app.push_view(ViewKind::Diff);
                app.views.maximized = false;
            }
        }
        Some(ViewKind::Refs) => {
            let commit_id = app
                .views
                .refs_view
                .as_ref()
                .and_then(|r| r.selected_ref().map(|re| re.commit_id));
            if let (Some(commit_id), Some(engine)) = (commit_id, &app.engine) {
                match engine.compute_commit_diff_cached(commit_id) {
                    Ok(diff_data) => {
                        app.views.diff_view = Some(app.create_diff_view(diff_data));
                        app.push_view(ViewKind::Diff);
                        app.views.maximized = false;
                    }
                    Err(err) => {
                        app.status_message = Some(format!("Failed to compute diff: {err}"));
                    }
                }
            }
        }
        Some(ViewKind::Stash) => {
            let commit_id = app
                .views
                .stash_view
                .as_ref()
                .and_then(|s| s.selected_stash().map(|se| se.commit_id));
            if let (Some(commit_id), Some(engine)) = (commit_id, &app.engine) {
                match engine.compute_commit_diff_cached(commit_id) {
                    Ok(diff_data) => {
                        app.views.diff_view = Some(app.create_diff_view(diff_data));
                        app.push_view(ViewKind::Diff);
                        app.views.maximized = false;
                    }
                    Err(err) => {
                        app.status_message = Some(format!("Failed to compute stash diff: {err}"));
                    }
                }
            }
        }
        Some(ViewKind::Reflog) => {
            let commit_id = app
                .views
                .reflog_view
                .as_ref()
                .and_then(|r| r.selected_entry().map(|re| re.new_id));
            if let (Some(commit_id), Some(engine)) = (commit_id, &app.engine) {
                match engine.compute_commit_diff_cached(commit_id) {
                    Ok(diff_data) => {
                        app.views.diff_view = Some(app.create_diff_view(diff_data));
                        app.push_view(ViewKind::Diff);
                        app.views.maximized = false;
                    }
                    Err(err) => {
                        app.status_message = Some(format!("Failed to compute reflog diff: {err}"));
                    }
                }
            }
        }
        Some(ViewKind::Log) => {
            let commit_id = app
                .views
                .log_view
                .as_ref()
                .and_then(LogView::selected_commit_id);
            if let (Some(commit_id), Some(engine)) = (commit_id, &app.engine) {
                match engine.compute_commit_diff_cached(commit_id) {
                    Ok(diff_data) => {
                        app.views.diff_view = Some(app.create_diff_view(diff_data));
                        app.push_view(ViewKind::Diff);
                        app.views.maximized = false;
                    }
                    Err(err) => {
                        app.status_message = Some(format!("Failed to compute log diff: {err}"));
                    }
                }
            }
        }
        Some(ViewKind::Grep) => {
            let m = app
                .views
                .grep_view
                .as_ref()
                .and_then(|g| g.selected_match().cloned());
            if let (Some(m), Some(engine)) = (m, &app.engine)
                && let Ok(head_id) = engine.head_commit_id()
            {
                match engine.read_blob_at_commit_path(head_id, &m.path) {
                    Ok(blob) => {
                        let mut bv = BlobView::new(head_id, blob);
                        bv.set_cursor(m.line_num.saturating_sub(1), visible_height);
                        app.views.blob_view = Some(bv);
                        app.push_view(ViewKind::Blob);
                        app.views.maximized = false;
                    }
                    Err(err) => {
                        app.status_message = Some(format!("Cannot open '{}': {err}", m.path));
                    }
                }
            }
        }
        Some(ViewKind::Diff) => {
            if let Some(mut diff_view) = app.views.diff_view.take() {
                let options = app.options.clone();
                let msg = app.with_blob_provider(|provider| {
                    diff_view.handle_enter(&options, provider, visible_height)
                });
                if let Some(msg) = msg {
                    app.status_message = Some(msg);
                }
                app.views.diff_view = Some(diff_view);
            }
        }
        _ => {}
    }
    Flow::Continue
}

/// Handles the `Parent` action: navigates to parent directory, parent blame, or older log commit.
pub fn handle_parent(app: &mut AppState, visible_height: usize) -> Flow {
    match app.active_view() {
        Some(ViewKind::Diff) => {
            if let Some(ref mut main) = app.views.main_view {
                main.move_down(1, visible_height);
            }
            refresh_diff_for_main_selection(app);
        }
        Some(ViewKind::Tree) => {
            let sub_info = app.views.tree_view.as_ref().and_then(|t| {
                (!t.current_path().is_empty())
                    .then(|| (t.parent_path().unwrap_or("").to_string(), t.commit_oid()))
            });
            if let Some((parent, commit_oid)) = sub_info
                && let Some(engine) = &app.engine
                && let Ok(listing) = engine.read_tree(commit_oid, &parent)
                && let Some(ref mut tree) = app.views.tree_view
            {
                tree.set_listing(listing);
            }
        }
        Some(ViewKind::Blame) => {
            let blame_info = app.views.blame_view.as_ref().and_then(|b| {
                b.current_parent_commit_id()
                    .map(|parent| (parent, b.path().to_string(), b.cursor()))
            });
            if let Some((parent_id, path, cursor)) = blame_info {
                if let Some(engine) = &app.engine
                    && let Some(cached) = engine.get_cached_blame(parent_id, &path)
                {
                    if let Some(ref mut blame) = app.views.blame_view {
                        blame.navigate_to(parent_id, path, cursor);
                        blame.apply_blame_result((*cached).clone());
                        blame.set_cursor(cursor, visible_height);
                    }
                    return Flow::Continue;
                }
                if app.blame_task.has_sender() {
                    if let Some(engine) = &app.engine {
                        let eng = engine.clone();
                        if let (req_id, token, Some(worker_tx)) = app.blame_task.begin_request() {
                            let p = path.clone();
                            if let Some(ref mut blame) = app.views.blame_view {
                                blame.set_status_message("Loading blame...".to_string());
                            }
                            tigrs_core::global_compute_pool().spawn(move || {
                                if token.is_cancelled() {
                                    return;
                                }
                                let res = eng.blame_file_cancellable(parent_id, &p, &token);
                                if token.is_cancelled() {
                                    return;
                                }
                                let _ = tigrs_core::send_or_yield(
                                    &worker_tx,
                                    crate::app::BlameWorkerResponse {
                                        commit_id: Some(parent_id),
                                        path: p.clone(),
                                        request_id: req_id,
                                        nav: crate::app::BlameNavKind::Parent {
                                            commit_id: parent_id,
                                            path: p,
                                            cursor,
                                            visible_height,
                                        },
                                        result: res,
                                    },
                                    &token,
                                );
                            });
                        }
                    }
                } else if let Some(engine) = &app.engine {
                    match engine.blame_file(parent_id, &path) {
                        Ok(res) => {
                            if let Some(ref mut blame) = app.views.blame_view {
                                blame.navigate_to(parent_id, path, cursor);
                                blame.apply_blame_result(res);
                                blame.set_cursor(cursor, visible_height);
                            }
                        }
                        Err(err) => {
                            if let Some(ref mut blame) = app.views.blame_view {
                                blame.set_status_message(format!("Cannot blame parent: {err}"));
                            }
                        }
                    }
                }
            } else if let Some(ref mut blame) = app.views.blame_view {
                blame.set_status_message("The selected commit has no parents".to_string());
            }
        }
        _ => {}
    }
    Flow::Continue
}

/// Handles the `Back` action: navigates backwards in blame history or tree directory hierarchy.
pub fn handle_back(app: &mut AppState, visible_height: usize) -> Flow {
    match app.active_view() {
        Some(ViewKind::Blame) => {
            let prev_entry = app
                .views
                .blame_view
                .as_ref()
                .and_then(|b| b.peek_history().cloned());
            if let Some(prev) = prev_entry {
                if let Some(engine) = &app.engine
                    && let Some(cached) = engine.get_cached_blame(prev.commit_oid, &prev.path)
                {
                    if let Some(ref mut blame) = app.views.blame_view {
                        let _ = blame.pop_history();
                        blame.navigate_back_to(prev, (*cached).clone(), visible_height);
                    }
                    return Flow::Continue;
                }
                if app.blame_task.has_sender() {
                    if let Some(engine) = &app.engine {
                        let eng = engine.clone();
                        if let (req_id, token, Some(worker_tx)) = app.blame_task.begin_request() {
                            let commit_oid = prev.commit_oid;
                            let path = prev.path.clone();
                            if let Some(ref mut blame) = app.views.blame_view {
                                blame.set_status_message("Loading blame...".to_string());
                            }
                            tigrs_core::global_compute_pool().spawn(move || {
                                if token.is_cancelled() {
                                    return;
                                }
                                let res = eng.blame_file_cancellable(commit_oid, &path, &token);
                                if token.is_cancelled() {
                                    return;
                                }
                                let _ = tigrs_core::send_or_yield(
                                    &worker_tx,
                                    crate::app::BlameWorkerResponse {
                                        commit_id: Some(commit_oid),
                                        path,
                                        request_id: req_id,
                                        nav: crate::app::BlameNavKind::Back {
                                            prev_entry: prev,
                                            visible_height,
                                        },
                                        result: res,
                                    },
                                    &token,
                                );
                            });
                        }
                    }
                } else if let Some(engine) = &app.engine
                    && let Ok(res) = engine.blame_file(prev.commit_oid, &prev.path)
                    && let Some(ref mut blame) = app.views.blame_view
                {
                    let _ = blame.pop_history();
                    blame.navigate_back_to(prev, res, visible_height);
                }
            } else if let Some(ref mut blame) = app.views.blame_view {
                blame.set_status_message("Already at start of blame history".to_string());
            }
        }
        Some(ViewKind::Tree) => {
            let sub_info = app.views.tree_view.as_ref().and_then(|t| {
                (!t.current_path().is_empty())
                    .then(|| (t.parent_path().unwrap_or("").to_string(), t.commit_oid()))
            });
            if let Some((parent, commit_oid)) = sub_info {
                if let Some(engine) = &app.engine
                    && let Ok(listing) = engine.read_tree(commit_oid, &parent)
                    && let Some(ref mut tree) = app.views.tree_view
                {
                    tree.set_listing(listing);
                }
            } else if app.views.view_stack.len() > 1 {
                app.pop_active_view();
            }
        }
        _ => {
            if app.views.view_stack.len() > 1 {
                app.pop_active_view();
            }
        }
    }
    Flow::Continue
}

/// Handles the `NextHunk` action in `DiffView`.
pub fn handle_next_hunk(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(ref mut diff) = app.views.diff_view {
        diff.next_hunk(visible_height);
    }
    Flow::Continue
}

/// Handles the `PrevHunk` action in `DiffView`.
pub fn handle_prev_hunk(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(ref mut diff) = app.views.diff_view {
        diff.prev_hunk(visible_height);
    }
    Flow::Continue
}

/// Handles the `NextFile` action in `DiffView`.
pub fn handle_next_file(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(ref mut diff) = app.views.diff_view {
        diff.next_file(visible_height);
    }
    Flow::Continue
}

/// Handles the `PrevFile` action in `DiffView`.
pub fn handle_prev_file(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(ref mut diff) = app.views.diff_view {
        diff.prev_file(visible_height);
    }
    Flow::Continue
}

/// Handles the `ToggleFileFold` (`za`) action in `DiffView`.
pub fn handle_toggle_file_fold(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(mut diff_view) = app.views.diff_view.take() {
        let options = app.options.clone();
        let msg = app.with_blob_provider(|provider| {
            diff_view.toggle_fold_current_file(&options, provider, visible_height)
        });
        if let Some(msg) = msg {
            app.status_message = Some(msg);
        }
        app.views.diff_view = Some(diff_view);
    }
    Flow::Continue
}

/// Handles the `FoldAllFiles` (`zM`) action in `DiffView`.
pub fn handle_fold_all_files(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(mut diff_view) = app.views.diff_view.take() {
        let options = app.options.clone();
        let msg = app.with_blob_provider(|provider| {
            diff_view.fold_all_files(&options, provider, visible_height)
        });
        app.status_message = Some(msg);
        app.views.diff_view = Some(diff_view);
    }
    Flow::Continue
}

/// Handles the `UnfoldAllFiles` (`zR`) action in `DiffView`.
pub fn handle_unfold_all_files(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(mut diff_view) = app.views.diff_view.take() {
        let options = app.options.clone();
        let msg = app.with_blob_provider(|provider| {
            diff_view.unfold_all_files(&options, provider, visible_height)
        });
        app.status_message = Some(msg);
        app.views.diff_view = Some(diff_view);
    }
    Flow::Continue
}

/// Handles the `ExpandHunkContext` (`+` / `=`) action in `DiffView`.
pub fn handle_expand_hunk_context(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(mut diff_view) = app.views.diff_view.take() {
        let options = app.options.clone();
        let msg = app.with_blob_provider(|provider| {
            diff_view.expand_current_hunk_context(10, &options, provider, visible_height)
        });
        if let Some(msg) = msg {
            app.status_message = Some(msg);
        }
        app.views.diff_view = Some(diff_view);
    }
    Flow::Continue
}

/// Handles the `ShrinkHunkContext` (`_`) action in `DiffView`.
pub fn handle_shrink_hunk_context(app: &mut AppState, visible_height: usize) -> Flow {
    if let Some(mut diff_view) = app.views.diff_view.take() {
        let options = app.options.clone();
        let msg = app.with_blob_provider(|provider| {
            diff_view.shrink_current_hunk_context(10, &options, provider, visible_height)
        });
        if let Some(msg) = msg {
            app.status_message = Some(msg);
        }
        app.views.diff_view = Some(diff_view);
    }
    Flow::Continue
}

/// Handles the `ToggleDiffFileDetails` (`i`) action in `DiffView`.
pub fn handle_toggle_diff_file_details(app: &mut AppState) -> Flow {
    if let Some(ref mut diff_view) = app.views.diff_view {
        diff_view.toggle_file_details();
    }
    Flow::Continue
}

/// Handles the `YankDiffText` (`y`) action in `DiffView`, copying the canonical
/// `diff --git` or `@@` header when on a file banner or hunk separator.
pub fn handle_yank_diff_text(app: &mut AppState) -> Flow {
    if let Some(ref diff_view) = app.views.diff_view
        && let Some(yanked) = diff_view.yank_text_at_cursor()
    {
        let first_line = yanked.lines().next().unwrap_or(&yanked);
        app.status_message = Some(format!("Copied: {first_line}"));
    }
    Flow::Continue
}
