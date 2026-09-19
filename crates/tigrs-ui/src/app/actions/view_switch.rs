// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! View switching, opening, maximizing, and hierarchy navigation.

use crate::app::dispatch::{
    compute_changes_diff, main_selection, open_blame, open_diff_for_main_selection_with_mode,
};
use crate::app::layout::ViewKind;
use crate::app::{AppState, Flow};
use crate::prompt::{PromptKind, PromptState};
use crate::view::{
    BlobView, DiffView, HelpView, LogView, MainView, ReflogView, RefsView, StashView, StatusView,
    TreeRow, TreeView, View,
};
use tigrs_core::cancel::CancellationToken;
use tigrs_git::ObjectId;

/// Opens or switches focus to `kind`, delegating to the view-specific handler.
///
/// This is the single entry point for the `view-*` family of actions; the
/// exhaustive match guarantees every [`ViewKind`] stays wired up.
pub fn handle_open_view(app: &mut AppState, kind: ViewKind, visible_height: usize) -> Flow {
    match kind {
        ViewKind::Main => handle_view_main(app),
        ViewKind::Status => handle_view_status(app),
        ViewKind::Tree => handle_view_tree(app),
        ViewKind::Blob => handle_view_blob(app, visible_height),
        ViewKind::Blame => handle_view_blame(app, visible_height),
        ViewKind::Diff => handle_view_diff(app),
        ViewKind::Help => handle_view_help(app),
        ViewKind::Refs => handle_view_refs(app),
        ViewKind::Stash => handle_view_stash(app),
        ViewKind::Grep => handle_view_grep(app),
        ViewKind::Reflog => handle_view_reflog(app),
        ViewKind::Log => handle_view_log(app, visible_height),
        ViewKind::Pager => handle_view_pager(app),
    }
}

/// Handles the `ViewClose` action: navigates up the directory hierarchy or pops the active view.
pub fn handle_view_close(app: &mut AppState) -> Flow {
    if app.active_view() == Some(ViewKind::Diff)
        && let Some(ref mut diff_view) = app.views.diff_view
        && diff_view.close_file_details()
    {
        return Flow::Continue;
    }
    if app.active_view() == Some(ViewKind::Tree) {
        let tree_info = app.views.tree_view.as_ref().and_then(|tree| {
            if tree.current_path().is_empty() {
                None
            } else {
                Some((
                    tree.parent_path().unwrap_or("").to_string(),
                    tree.commit_oid(),
                ))
            }
        });
        if let Some((parent, commit_oid)) = tree_info {
            if let Some(engine) = &app.engine
                && let Ok(listing) = engine.read_tree(commit_oid, &parent)
                && let Some(ref mut tree) = app.views.tree_view
            {
                tree.set_listing(listing);
            }
            return Flow::Continue;
        }
    }

    if app.views.view_stack.len() > 1 {
        app.pop_active_view();
        Flow::Continue
    } else {
        Flow::Quit
    }
}

/// Handles the `ViewMain` action: opens or brings focus to the commit log (`MainView`).
pub fn handle_view_main(app: &mut AppState) -> Flow {
    if app.views.main_view.is_some() {
        app.push_view(ViewKind::Main);
        app.views.maximized = true;
    } else if let Some(engine) = &app.engine {
        let branch = engine
            .current_branch()
            .unwrap_or_else(|_| "HEAD".to_string());
        let mut main = MainView::new(branch);
        if let Some(total) = engine.fast_commit_count_estimate() {
            main.set_total_commits(total);
        }
        let (_cancel_src, token) = CancellationToken::new();
        if let Ok(iter) = engine.stream_commits(None, Some(100), token) {
            for commits in iter.flatten() {
                main.append_commits(commits);
            }
        }
        main.set_finished();
        app.views.main_view = Some(main);
        app.push_view(ViewKind::Main);
        app.views.maximized = true;
    }
    Flow::Continue
}

/// Handles the `ViewLog` action: opens the rich revision log view with diffstats.
pub fn handle_view_log(app: &mut AppState, visible_height: usize) -> Flow {
    if app.active_view() == Some(ViewKind::Log) {
        if !app.views.maximized {
            app.views.maximized = true;
        }
    } else {
        let target_commit_id = app
            .active_view()
            .and_then(|kind| app.view_ref(kind))
            .and_then(View::selected_commit_id)
            .or_else(|| app.engine.as_ref().and_then(|e| e.head_commit_id().ok()));

        if app.views.log_view.is_none()
            && let Some(engine) = app.engine.clone()
        {
            const MAX_LOG_VIEW_WINDOW: usize = 4096;
            const LOG_VIEW_LOOKBEHIND: usize = 512;
            let branch = engine
                .current_branch()
                .unwrap_or_else(|_| "HEAD".to_string());
            let mut log = LogView::new(branch);
            let commit_ids: Vec<ObjectId> = if let Some(ref main) = app.views.main_view {
                let commits = main.commits();
                let full_target_idx = target_commit_id
                    .and_then(|tid| main.commit_index_for_id(&tid))
                    .unwrap_or(0);
                let win_start = full_target_idx.saturating_sub(LOG_VIEW_LOOKBEHIND);
                let win_end = (win_start + MAX_LOG_VIEW_WINDOW).min(commits.len());
                commits[win_start..win_end].iter().map(|c| c.id).collect()
            } else {
                let (_src, token) = CancellationToken::new();
                let mut ids = Vec::new();
                if let Ok(iter) = engine.stream_commits(None, Some(100), token) {
                    'outer: for batch in iter.flatten() {
                        for c in batch {
                            ids.push(c.id);
                            if ids.len() >= MAX_LOG_VIEW_WINDOW {
                                break 'outer;
                            }
                        }
                    }
                }
                ids
            };

            let refs_by_commit: std::collections::HashMap<tigrs_git::ObjectId, Vec<String>> =
                if let Some(ref main) = app.views.main_view
                    && !main.ref_names_by_commit().is_empty()
                {
                    main.ref_names_by_commit()
                } else {
                    let refs_list = engine.list_refs().ok().unwrap_or_default();
                    let mut map: std::collections::HashMap<tigrs_git::ObjectId, Vec<String>> =
                        std::collections::HashMap::new();
                    for r in refs_list {
                        map.entry(r.commit_id).or_default().push(r.name);
                    }
                    map
                };

            let target_idx = target_commit_id
                .and_then(|tid| commit_ids.iter().position(|id| *id == tid))
                .unwrap_or(0);

            let (sync_start, sync_end) = if app.log_task.has_sender() && !commit_ids.is_empty() {
                (
                    target_idx.saturating_sub(2),
                    (target_idx + 2).min(commit_ids.len() - 1),
                )
            } else {
                (0, commit_ids.len().saturating_sub(1))
            };

            for (idx, &id) in commit_ids.iter().enumerate() {
                let ref_names = refs_by_commit.get(&id).map(Vec::as_slice);
                if !app.log_task.has_sender() || (idx >= sync_start && idx <= sync_end) {
                    if let Ok(diff) = engine.compute_commit_diff_cached(id) {
                        log.append_commit_diff(&diff, ref_names);
                    } else {
                        log.append_commit_placeholder(id, ref_names);
                    }
                } else {
                    log.append_commit_placeholder(id, ref_names);
                }
            }
            app.views.log_view = Some(log);

            if app.log_task.has_sender() && !commit_ids.is_empty() {
                let after = (sync_end + 1)..commit_ids.len();
                let before = (0..sync_start).rev();
                let rem: Vec<tigrs_git::ObjectId> =
                    after.chain(before).map(|i| commit_ids[i]).collect();

                if !rem.is_empty()
                    && let (req_id, token, Some(worker_tx)) = app.log_task.begin_request()
                {
                    let eng = engine;
                    tigrs_core::global_compute_pool().spawn(move || {
                        for chunk in rem.chunks(16) {
                            if token.is_cancelled() {
                                return;
                            }
                            let mut batch = Vec::with_capacity(chunk.len());
                            for &id in chunk {
                                if token.is_cancelled() {
                                    return;
                                }
                                let ref_names = refs_by_commit.get(&id).cloned();
                                if let Ok(diff) =
                                    eng.compute_commit_diff_cached_cancellable(id, &token)
                                {
                                    batch.push((diff, ref_names));
                                }
                            }
                            if !batch.is_empty()
                                && !tigrs_core::send_or_yield(
                                    &worker_tx,
                                    crate::app::LogWorkerResponse {
                                        request_id: req_id,
                                        diffs: batch,
                                    },
                                    &token,
                                )
                            {
                                return;
                            }
                        }
                    });
                }
            }
        }

        if let (Some(target_id), Some(ref mut log)) =
            (target_commit_id, app.views.log_view.as_mut())
        {
            log.jump_to_commit(&target_id, visible_height);
        }

        if app.views.log_view.is_some() {
            app.push_view(ViewKind::Log);
            app.views.maximized = true;
        }
    }
    Flow::Continue
}

/// Handles the `ViewDiff` action: opens or brings focus to the diff view for the selection.
pub fn handle_view_diff(app: &mut AppState) -> Flow {
    match app.active_view() {
        Some(ViewKind::Main) => open_diff_for_main_selection_with_mode(app, true),
        Some(ViewKind::Diff) => {
            if app.views.view_stack.len() >= 2 && !app.views.maximized {
                app.views.maximized = true;
            }
        }
        Some(ViewKind::Status) => {
            let item = app
                .views
                .status_view
                .as_ref()
                .and_then(|s| s.selected_item().cloned());
            if let Some(item) = item
                && let Some(view) =
                    crate::app::dispatch::get_or_create_status_item_diff_view(app, item)
            {
                app.views.diff_view = Some(view);
                app.push_view(ViewKind::Diff);
                app.views.maximized = true;
            }
        }
        Some(
            kind @ (ViewKind::Blame
            | ViewKind::Refs
            | ViewKind::Stash
            | ViewKind::Reflog
            | ViewKind::Log),
        ) => {
            let commit_id = app.view_ref(kind).and_then(View::selected_commit_id);
            if let (Some(commit_id), Some(engine)) = (commit_id, &app.engine) {
                match engine.compute_commit_diff_cached(commit_id) {
                    Ok(diff_data) => {
                        app.views.diff_view = Some(app.create_diff_view(diff_data));
                        app.push_view(ViewKind::Diff);
                        app.views.maximized = true;
                    }
                    Err(err) => {
                        if kind != ViewKind::Blame {
                            app.status_message = Some(format!("Failed to compute diff: {err}"));
                        }
                    }
                }
            }
        }
        _ => {
            if app.views.diff_view.is_some() {
                app.push_view(ViewKind::Diff);
                app.views.maximized = true;
            }
        }
    }
    Flow::Continue
}

/// Handles the `ViewStatus` action: toggles or opens the working tree status view.
pub fn handle_view_status(app: &mut AppState) -> Flow {
    if app.active_view() == Some(ViewKind::Status) {
        if app.views.view_stack.len() > 1 {
            app.pop_active_view();
        } else {
            return Flow::Quit;
        }
    } else if app.views.status_view.is_some() {
        app.push_view(ViewKind::Status);
    } else if let Some(engine) = &app.engine {
        let (_src, token) = CancellationToken::new();
        if let Ok(report) = engine.load_status(&token) {
            app.views.status_view = Some(StatusView::new(report));
            app.push_view(ViewKind::Status);
        }
    }
    Flow::Continue
}

/// Handles the `ViewStage` action: opens the staging view for the active status item or diff.
pub fn handle_view_stage(app: &mut AppState) -> Flow {
    match app.active_view() {
        Some(ViewKind::Status) => {
            let item = app
                .views
                .status_view
                .as_ref()
                .and_then(|s| s.selected_item().cloned());
            if let Some(item) = item
                && let Some(view) =
                    crate::app::dispatch::get_or_create_status_item_diff_view(app, item)
            {
                app.views.diff_view = Some(view);
                app.push_view(ViewKind::Diff);
                app.views.maximized = true;
            }
        }
        Some(ViewKind::Main) => {
            let (changes_kind, _) = main_selection(app);
            if let Some(kind) = changes_kind {
                if let Some(diff_data) = compute_changes_diff(app, kind) {
                    app.views.diff_view = Some(app.create_diff_view(diff_data));
                    app.push_view(ViewKind::Diff);
                    app.views.maximized = true;
                }
            } else if app
                .views
                .diff_view
                .as_ref()
                .and_then(DiffView::status_item)
                .is_some()
            {
                app.push_view(ViewKind::Diff);
                app.views.maximized = true;
            } else {
                app.status_message = Some(
                    "No stage content, press 's' to open the status view and choose file"
                        .to_string(),
                );
            }
        }
        Some(ViewKind::Diff) => {
            if app
                .views
                .diff_view
                .as_ref()
                .and_then(DiffView::status_item)
                .is_some()
            {
                app.views.maximized = true;
            } else {
                app.status_message = Some(
                    "No stage content, press 's' to open the status view and choose file"
                        .to_string(),
                );
            }
        }
        _ => {
            if app
                .views
                .diff_view
                .as_ref()
                .and_then(DiffView::status_item)
                .is_some()
            {
                app.push_view(ViewKind::Diff);
                app.views.maximized = true;
            } else {
                app.status_message = Some(
                    "No stage content, press 's' to open the status view and choose file"
                        .to_string(),
                );
            }
        }
    }
    Flow::Continue
}

/// Handles the `ViewTree` action: opens directory tree view for the selected commit or HEAD.
pub fn handle_view_tree(app: &mut AppState) -> Flow {
    match app.active_view() {
        Some(ViewKind::Diff) => {
            let is_status = app
                .views
                .diff_view
                .as_ref()
                .and_then(DiffView::status_item)
                .is_some();
            let commit_id = app.views.diff_view.as_ref().map(DiffView::commit_id);
            if let (Some(commit_id), Some(engine)) = (commit_id, &app.engine) {
                let resolved_commit = if is_status
                    || commit_id.is_null()
                    || commit_id == tigrs_git::ObjectId::empty_tree(commit_id.kind())
                {
                    engine.head_commit_id().ok().unwrap_or(commit_id)
                } else {
                    commit_id
                };
                if let Ok(listing) = engine.read_tree(resolved_commit, "") {
                    app.views.tree_view = Some(TreeView::new(listing));
                    app.push_view(ViewKind::Tree);
                }
            }
        }
        Some(ViewKind::Status) => {
            if let Some(engine) = &app.engine
                && let Ok(head_id) = engine.head_commit_id()
                && let Ok(listing) = engine.read_tree(head_id, "")
            {
                app.views.tree_view = Some(TreeView::new(listing));
                app.push_view(ViewKind::Tree);
            }
        }
        Some(ViewKind::Main) => {
            if let Some(engine) = &app.engine {
                let commit_id = app
                    .views
                    .main_view
                    .as_ref()
                    .and_then(|m| m.selected_commit().map(|c| c.id))
                    .or_else(|| engine.head_commit_id().ok());
                if let Some(id) = commit_id
                    && let Ok(listing) = engine.read_tree(id, "")
                {
                    app.views.tree_view = Some(TreeView::new(listing));
                    app.push_view(ViewKind::Tree);
                }
            }
        }
        Some(
            kind @ (ViewKind::Blame
            | ViewKind::Blob
            | ViewKind::Refs
            | ViewKind::Stash
            | ViewKind::Reflog
            | ViewKind::Log),
        ) => {
            let commit_id = app.view_ref(kind).and_then(View::selected_commit_id);
            if let (Some(commit_id), Some(engine)) = (commit_id, &app.engine)
                && let Ok(listing) = engine.read_tree(commit_id, "")
            {
                app.views.tree_view = Some(TreeView::new(listing));
                app.push_view(ViewKind::Tree);
            }
        }
        _ => {
            if app.views.tree_view.is_some() {
                app.push_view(ViewKind::Tree);
            } else if let Some(engine) = &app.engine
                && let Ok(head_id) = engine.head_commit_id()
                && let Ok(listing) = engine.read_tree(head_id, "")
            {
                app.views.tree_view = Some(TreeView::new(listing));
                app.push_view(ViewKind::Tree);
            }
        }
    }
    Flow::Continue
}

/// Handles the `ViewBlob` action: inspects the selected file blob in `BlobView`.
pub fn handle_view_blob(app: &mut AppState, visible_height: usize) -> Flow {
    match app.active_view() {
        Some(ViewKind::Tree) => {
            let entry = app
                .views
                .tree_view
                .as_ref()
                .and_then(|t| match t.selected_row() {
                    Some(TreeRow::Entry(e)) if !e.is_dir() => Some(e.clone()),
                    _ => None,
                });
            let commit_id = app.views.tree_view.as_ref().map(TreeView::commit_oid);
            if let (Some(entry), Some(commit_id), Some(engine)) = (entry, commit_id, &app.engine) {
                let blob_res = if entry.kind == tigrs_git::TreeEntryKind::Commit {
                    Ok(tigrs_git::BlobContent {
                        oid: entry.oid,
                        path: entry.path.clone(),
                        size: 0,
                        is_binary: false,
                        lines: tigrs_core::LineBuffer::from(vec![format!(
                            "Subproject commit {}",
                            entry.oid
                        )]),
                    })
                } else {
                    engine.read_blob(entry.oid, &entry.path)
                };
                if let Ok(blob) = blob_res {
                    app.views.blob_view =
                        Some(BlobView::new_with_options(commit_id, blob, &app.options));
                    app.push_view(ViewKind::Blob);
                }
            }
        }
        Some(ViewKind::Blame) => {
            let info = app
                .views
                .blame_view
                .as_ref()
                .map(|b| (b.commit_oid(), b.path().to_string(), b.cursor()));
            if let (Some((commit_oid, path, cursor)), Some(engine)) = (info, &app.engine)
                && let Ok(blob) = engine.read_blob_at_commit_path(commit_oid, &path)
            {
                let mut bv = BlobView::new_with_options(commit_oid, blob, &app.options);
                bv.set_cursor(cursor, visible_height);
                app.views.blob_view = Some(bv);
                app.push_view(ViewKind::Blob);
            }
        }
        Some(ViewKind::Diff) => {
            let is_status = app
                .views
                .diff_view
                .as_ref()
                .and_then(DiffView::status_item)
                .is_some();
            let info = app
                .views
                .diff_view
                .as_ref()
                .and_then(|d| d.current_file().map(|f| (d.commit_id(), f.to_string())));
            if let (Some((commit_id, path)), Some(engine)) = (info, &app.engine) {
                let resolved_commit = if is_status
                    || commit_id.is_null()
                    || commit_id == tigrs_git::ObjectId::empty_tree(commit_id.kind())
                {
                    engine.head_commit_id().ok().unwrap_or(commit_id)
                } else {
                    commit_id
                };
                if let Ok(blob) = engine.read_blob_at_commit_path(resolved_commit, &path) {
                    app.views.blob_view = Some(BlobView::new_with_options(
                        resolved_commit,
                        blob,
                        &app.options,
                    ));
                    app.push_view(ViewKind::Blob);
                }
            }
        }
        Some(ViewKind::Status) => {
            let item = app
                .views
                .status_view
                .as_ref()
                .and_then(|s| s.selected_item().cloned());
            if let (Some(item), Some(engine)) = (item, &app.engine)
                && let Ok(head_id) = engine.head_commit_id()
                && let Ok(blob) = engine.read_blob_at_commit_path(head_id, &item.path)
            {
                app.views.blob_view = Some(BlobView::new_with_options(head_id, blob, &app.options));
                app.push_view(ViewKind::Blob);
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
                && let Ok(blob) = engine.read_blob_at_commit_path(head_id, &m.path)
            {
                let mut bv = BlobView::new_with_options(head_id, blob, &app.options);
                bv.set_cursor(m.line_num.saturating_sub(1), visible_height);
                app.views.blob_view = Some(bv);
                app.push_view(ViewKind::Blob);
            }
        }
        Some(ViewKind::Log) => {
            let info = app
                .views
                .log_view
                .as_ref()
                .and_then(|l| l.selected_file().map(|(p, c)| (c, p.to_string())));
            if let (Some((commit_id, path)), Some(engine)) = (info, &app.engine)
                && let Ok(blob) = engine.read_blob_at_commit_path(commit_id, &path)
            {
                app.views.blob_view =
                    Some(BlobView::new_with_options(commit_id, blob, &app.options));
                app.push_view(ViewKind::Blob);
            }
        }
        _ => {
            if app.views.blob_view.is_some() {
                app.push_view(ViewKind::Blob);
            } else {
                app.status_message =
                    Some("No file chosen, press 't' to open tree view".to_string());
            }
        }
    }
    Flow::Continue
}

/// Handles the `ViewBlame` action: opens line annotations in `BlameView`.
pub fn handle_view_blame(app: &mut AppState, visible_height: usize) -> Flow {
    match app.active_view() {
        Some(ViewKind::Diff) => {
            let is_status = app
                .views
                .diff_view
                .as_ref()
                .and_then(DiffView::status_item)
                .is_some();
            let info = app
                .views
                .diff_view
                .as_ref()
                .and_then(|d| d.current_file().map(|f| (d.commit_id(), f.to_string())));
            if let Some((commit_id, path)) = info {
                let resolved_commit = if is_status
                    || commit_id.is_null()
                    || commit_id == tigrs_git::ObjectId::empty_tree(commit_id.kind())
                {
                    app.engine
                        .as_ref()
                        .and_then(|e| e.head_commit_id().ok())
                        .unwrap_or(commit_id)
                } else {
                    commit_id
                };
                open_blame(app, resolved_commit, &path, 0, visible_height);
            }
        }
        Some(ViewKind::Blob) => {
            let info = app
                .views
                .blob_view
                .as_ref()
                .map(|b| (b.commit_oid(), b.path().to_string(), b.cursor()));
            if let Some((commit_oid, path, cursor)) = info {
                open_blame(app, commit_oid, &path, cursor, visible_height);
            }
        }
        Some(ViewKind::Tree) => {
            let entry = app
                .views
                .tree_view
                .as_ref()
                .and_then(|t| match t.selected_row() {
                    Some(TreeRow::Entry(e)) if !e.is_dir() => Some(e.clone()),
                    _ => None,
                });
            let commit_id = app.views.tree_view.as_ref().map(TreeView::commit_oid);
            if let (Some(entry), Some(commit_id)) = (entry, commit_id) {
                open_blame(app, commit_id, &entry.path, 0, visible_height);
            }
        }
        Some(ViewKind::Status) => {
            let item = app
                .views
                .status_view
                .as_ref()
                .and_then(|s| s.selected_item().cloned());
            if let (Some(item), Some(engine)) = (item, &app.engine)
                && let Ok(head_id) = engine.head_commit_id()
            {
                open_blame(app, head_id, &item.path, 0, visible_height);
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
                open_blame(
                    app,
                    head_id,
                    &m.path,
                    m.line_num.saturating_sub(1),
                    visible_height,
                );
            }
        }
        Some(ViewKind::Log) => {
            let info = app
                .views
                .log_view
                .as_ref()
                .and_then(|l| l.selected_file().map(|(p, c)| (c, p.to_string())));
            if let Some((commit_id, path)) = info {
                open_blame(app, commit_id, &path, 0, visible_height);
            }
        }
        _ => {
            if app.views.blame_view.is_some() {
                app.push_view(ViewKind::Blame);
            } else {
                app.status_message =
                    Some("No file chosen, press 't' to open tree view".to_string());
            }
        }
    }
    Flow::Continue
}

/// Handles the `ViewHelp` action: displays keybinding quick-reference guide.
pub fn handle_view_help(app: &mut AppState) -> Flow {
    if app.active_view() == Some(ViewKind::Help) {
        if app.views.view_stack.len() > 1 {
            app.pop_active_view();
        }
    } else {
        if app.views.help_view.is_none() {
            app.views.help_view = Some(HelpView::new());
        }
        app.push_view(ViewKind::Help);
    }
    Flow::Continue
}

/// Handles the `ViewRefs` action: displays repository branches, tags, and remotes.
pub fn handle_view_refs(app: &mut AppState) -> Flow {
    if app.active_view() == Some(ViewKind::Refs) {
        if app.views.view_stack.len() > 1 {
            app.pop_active_view();
        }
    } else if let Some(ref engine) = app.engine {
        match engine.list_refs() {
            Ok(refs) => {
                app.views.refs_view = Some(RefsView::new(refs));
                app.push_view(ViewKind::Refs);
            }
            Err(err) => {
                app.status_message = Some(format!("Failed to list refs: {err}"));
            }
        }
    } else if app.views.refs_view.is_some() {
        app.push_view(ViewKind::Refs);
    }
    Flow::Continue
}

/// Handles the `ViewStash` action: displays git stash stack.
pub fn handle_view_stash(app: &mut AppState) -> Flow {
    if app.active_view() == Some(ViewKind::Stash) {
        if app.views.view_stack.len() > 1 {
            app.pop_active_view();
        }
    } else if let Some(ref engine) = app.engine {
        match engine.list_stashes() {
            Ok(stashes) => {
                app.views.stash_view = Some(StashView::new(stashes));
                app.push_view(ViewKind::Stash);
            }
            Err(err) => {
                app.status_message = Some(format!("Failed to list stashes: {err}"));
            }
        }
    } else if app.views.stash_view.is_some() {
        app.push_view(ViewKind::Stash);
    }
    Flow::Continue
}

/// Handles the `ViewReflog` action: displays HEAD/branch reference transitions.
pub fn handle_view_reflog(app: &mut AppState) -> Flow {
    if app.active_view() == Some(ViewKind::Reflog) {
        if app.views.view_stack.len() > 1 {
            app.pop_active_view();
        }
    } else if let Some(ref engine) = app.engine {
        let branch = engine
            .current_branch()
            .unwrap_or_else(|_| "HEAD".to_string());
        match engine.read_reflog(&branch) {
            Ok(entries) => {
                app.views.reflog_view = Some(ReflogView::new(branch, entries));
                app.push_view(ViewKind::Reflog);
            }
            Err(err) => {
                app.status_message = Some(format!("Failed to read reflog: {err}"));
            }
        }
    } else if app.views.reflog_view.is_some() {
        app.push_view(ViewKind::Reflog);
    }
    Flow::Continue
}

/// Handles the `ViewGrep` action: opens prompt for grep query or switches to results.
pub fn handle_view_grep(app: &mut AppState) -> Flow {
    if app.active_view() == Some(ViewKind::Grep) || app.views.grep_view.is_none() {
        app.prompt = Some(PromptState::with_initial_text(PromptKind::Command, "grep "));
    } else {
        app.push_view(ViewKind::Grep);
    }
    Flow::Continue
}

/// Handles the `ViewPager` action: displays arbitrary piped text in `PagerView`.
pub fn handle_view_pager(app: &mut AppState) -> Flow {
    if app.active_view() == Some(ViewKind::Pager) {
        if app.views.view_stack.len() > 1 {
            app.views.view_stack.retain(|&v| v != ViewKind::Pager);
        }
    } else if app.views.pager_view.is_some() {
        app.push_view(ViewKind::Pager);
    } else {
        app.status_message = Some("No pager content".to_string());
    }
    Flow::Continue
}

/// Handles the `ViewNext` action: cycles active pane focus across the split panes or view stack.
pub fn handle_view_next(app: &mut AppState) -> Flow {
    if !app.views.maximized
        && let Some((primary, secondary)) = app.views.split_views()
    {
        let next = if app.active_view() == Some(primary) {
            secondary
        } else {
            primary
        };
        app.views.view_stack.retain(|&v| v != next);
        app.views.view_stack.push(next);
        app.invalidate_screen();
        return Flow::Continue;
    }
    if app.views.view_stack.len() > 1 {
        let first = app.views.view_stack.remove(0);
        app.views.view_stack.push(first);
        app.invalidate_screen();
    }
    Flow::Continue
}

/// Handles the `Maximize` action: toggles between split-view and full-screen layout.
pub fn handle_maximize(app: &mut AppState) -> Flow {
    if app.views.view_stack.len() >= 2 {
        app.views.maximized = !app.views.maximized;
        app.invalidate_screen();
        app.status_message = Some(if app.views.maximized {
            "View maximized".to_string()
        } else {
            "View split restored".to_string()
        });
    } else {
        app.status_message = Some("No split view to maximize".to_string());
    }
    Flow::Continue
}
