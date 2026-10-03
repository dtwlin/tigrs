// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! View navigation, cursor motion, and split view synchronization.

use super::layout::ViewKind;
use super::{
    AppState, BlameWorkerResponse, DIFF_DEBOUNCE_INTERVAL, DiffRequestTarget, Flow,
    SPECULATIVE_PREFETCH_INTERVAL,
};
use crate::view::{BlameView, ChangesKind, DiffView, MainRow, MainView, View};
use std::time::Instant;

/// Opens the `BlameView` for `path` at `commit_id`, seeding the view immediately
/// with blob lines when async workers are enabled and computing per-line blame in the background.
pub fn open_blame(
    app: &mut AppState,
    commit_id: tigrs_git::ObjectId,
    path: &str,
    cursor: usize,
    visible_height: usize,
) {
    let engine = match app.engine {
        Some(ref e) => e.clone(),
        None => return,
    };

    if let Some(cached) = engine.get_cached_blame(commit_id, path) {
        let mut bv = BlameView::from_result((*cached).clone());
        bv.refresh_highlighting(&app.options);
        bv.set_cursor(cursor, visible_height);
        app.views.blame_view = Some(bv);
        app.push_view(ViewKind::Blame);
        return;
    }

    if app.blame_task.has_sender() {
        match engine.read_blob_at_commit_path(commit_id, path) {
            Ok(blob) => {
                let mut bv = BlameView::from_blob(commit_id, blob);
                bv.refresh_highlighting(&app.options);
                bv.set_cursor(cursor, visible_height);
                app.views.blame_view = Some(bv);
                app.push_view(ViewKind::Blame);

                if let (req_id, token, Some(tx)) = app.blame_task.begin_request() {
                    let eng = engine.clone();
                    let p = path.to_string();
                    tigrs_core::global_compute_pool().spawn(move || {
                        if token.is_cancelled() {
                            return;
                        }
                        let res = eng.blame_file_cancellable(commit_id, &p, &token);
                        if token.is_cancelled() {
                            return;
                        }
                        let _ = tigrs_core::send_or_yield(
                            &tx,
                            BlameWorkerResponse {
                                commit_id: Some(commit_id),
                                path: p,
                                request_id: req_id,
                                nav: crate::app::BlameNavKind::Initial,
                                result: res,
                            },
                            &token,
                        );
                    });
                }
                return;
            }
            Err(err) => {
                app.status_message = Some(format!("Failed to read file: {err}"));
                return;
            }
        }
    }

    match engine.blame_file(commit_id, path) {
        Ok(res) => {
            let mut bv = BlameView::from_result(res);
            bv.refresh_highlighting(&app.options);
            bv.set_cursor(cursor, visible_height);
            app.views.blame_view = Some(bv);
            app.push_view(ViewKind::Blame);
        }
        Err(err) => {
            app.status_message = Some(format!("Blame failed: {err}"));
        }
    }
}

/// Builds the diff for an uncommitted-changes row from the cached status scan.
///
/// Returns `None` when there is no engine or no scan yet, or when the section
/// turns out to be empty.
pub fn compute_changes_diff(
    app: &AppState,
    kind: ChangesKind,
) -> Option<std::sync::Arc<tigrs_git::CommitDiff>> {
    let engine = app.engine.as_ref()?;
    let generation = engine.generation();
    if let Some((cached_diff, _)) = app.changes_diff_cache.get(&(kind, generation)) {
        return Some(std::sync::Arc::clone(cached_diff));
    }
    let report = app.changes_report.as_ref()?;

    let (section, items) = match kind {
        ChangesKind::Untracked => (
            tigrs_git::StatusSection::Untracked,
            report.untracked.as_slice(),
        ),
        ChangesKind::Unstaged => (
            tigrs_git::StatusSection::Unstaged,
            report.unstaged.as_slice(),
        ),
        ChangesKind::Staged => (tigrs_git::StatusSection::Staged, report.staged.as_slice()),
    };

    engine
        .compute_status_section_diff(section, items)
        .ok()
        .map(std::sync::Arc::new)
}

/// Returns a cached `DiffView` for `kind` at the current repository generation,
/// or computes and caches it on miss.
pub fn get_or_create_changes_diff_view(app: &mut AppState, kind: ChangesKind) -> Option<DiffView> {
    let generation = app.engine.as_ref()?.generation();
    if let Some((_, cached_view)) = app.changes_diff_cache.get(&(kind, generation)) {
        return Some(cached_view.clone());
    }
    let diff_data = compute_changes_diff(app, kind)?;
    let view = app.create_diff_view(std::sync::Arc::clone(&diff_data));
    app.changes_diff_cache
        .insert((kind, generation), (diff_data, view.clone()));
    Some(view)
}

/// Returns the main view's current selection as owned, `Copy` values.
///
/// Exactly one element is `Copy` when a row is selected. Returning plain values
/// (rather than a borrowed `MainRow`) lets callers mutate `app` afterwards.
pub fn main_selection(app: &AppState) -> (Option<ChangesKind>, Option<tigrs_git::ObjectId>) {
    match app
        .views
        .main_view
        .as_ref()
        .and_then(MainView::selected_row)
    {
        Some(MainRow::Changes(change)) => (Some(change.kind), None),
        Some(MainRow::Commit(commit)) => (None, Some(commit.id)),
        None => (None, None),
    }
}

/// Rebuilds the diff view to match the main view's current selection.
///
/// No-ops when the diff already shows that selection, so this is cheap to call
/// on every cursor movement.
pub fn refresh_diff_for_main_selection(app: &mut AppState) {
    let (changes_kind, commit_id) = main_selection(app);

    if let Some(kind) = changes_kind {
        request_changes_diff(app, kind);
        return;
    }

    if let Some(commit_id) = commit_id {
        request_commit_diff(app, commit_id);
    }
}

/// Points the diff view at `kind` (`Staged`, `Unstaged`, or `Untracked`),
/// using the generation-scoped cache or background worker pool when available.
pub fn request_changes_diff(app: &mut AppState, kind: ChangesKind) {
    let Some(engine) = &app.engine else {
        return;
    };
    let generation = engine.generation();

    if app.views.diff_view.as_ref().map(DiffView::title) == Some(kind.title()) {
        app.pending_diff_target = None;
        app.diff_task.cancel_in_flight();
        return;
    }

    if let Some((_, cached_view)) = app.changes_diff_cache.get(&(kind, generation)) {
        app.pending_diff_target = None;
        app.diff_task.cancel_in_flight();
        app.views.diff_view = Some(cached_view.clone());
        return;
    }

    if app.diff_task.has_sender() {
        app.diff_task.active_request_id = app.diff_task.active_request_id.wrapping_add(1);
        let req_id = app.diff_task.active_request_id;
        let deadline = Instant::now() + DIFF_DEBOUNCE_INTERVAL;
        app.pending_diff_target = Some((DiffRequestTarget::Changes(kind), deadline, req_id));
        return;
    }

    app.pending_diff_target = None;
    if let Some(view) = get_or_create_changes_diff_view(app, kind) {
        app.views.diff_view = Some(view);
    }
}

/// Points the diff view at `commit_id`, preferring the cheapest path available.
///
/// Order of preference: no-op when already displayed, `DiffDocumentCache` + `GitLruCache`
/// zero-copy hit (`< 20 µs`), debounced background computation, and finally a synchronous
/// compute when no worker channel is wired up (tests, piped mode).
pub fn request_commit_diff(app: &mut AppState, commit_id: tigrs_git::ObjectId) {
    let Some(engine) = &app.engine else {
        return;
    };

    if app.views.diff_view.as_ref().map(DiffView::commit_id) == Some(commit_id) {
        app.pending_diff_target = None;
        app.diff_task.cancel_in_flight();
        return;
    }

    // Fast path: instant zero-copy lookup from LRU cache (`Arc<CommitDiff>` + `DiffDocumentCache`)
    if let Some(cached) = engine.get_cached_diff(&commit_id) {
        app.pending_diff_target = None;
        app.diff_task.cancel_in_flight();
        app.views.diff_view = Some(app.create_diff_view(cached));
        return;
    }

    // Debounced async path when diff_task channel is active
    if app.diff_task.has_sender() {
        app.diff_task.active_request_id = app.diff_task.active_request_id.wrapping_add(1);
        let req_id = app.diff_task.active_request_id;
        let deadline = Instant::now() + DIFF_DEBOUNCE_INTERVAL;
        app.pending_diff_target = Some((DiffRequestTarget::Commit(commit_id), deadline, req_id));
        return;
    }

    app.pending_diff_target = None;
    if let Ok(diff_data) = engine.compute_commit_diff_cached(commit_id) {
        app.views.diff_view = Some(app.create_diff_view(diff_data));
    }
}

/// Opens the diff view for the main view's current selection and pushes it onto
/// the view stack.
pub fn open_diff_for_main_selection(app: &mut AppState) {
    open_diff_for_main_selection_with_mode(app, false);
}

/// Opens diff for current main selection, optionally maximizing the diff view
/// to full-window (matching upstream Tig's `REQ_VIEW_DIFF` / `'d'` key) or
/// maintaining/opening a dual split view (matching `REQ_ENTER` / `<Enter>`).
pub fn open_diff_for_main_selection_with_mode(app: &mut AppState, maximize: bool) {
    app.pending_speculative_prefetch = None;
    app.pending_diff_target = None;
    app.diff_task.cancel_in_flight();
    let (changes_kind, commit_id) = main_selection(app);

    if let Some(kind) = changes_kind {
        if app.views.diff_view.as_ref().map(DiffView::title) == Some(kind.title()) {
            app.push_view(ViewKind::Diff);
            app.views.maximized = maximize;
            return;
        }
        if let Some(view) = get_or_create_changes_diff_view(app, kind) {
            app.views.diff_view = Some(view);
            app.push_view(ViewKind::Diff);
            app.views.maximized = maximize;
        }
        return;
    }

    if let (Some(commit_id), Some(engine)) = (commit_id, &app.engine) {
        if app.views.diff_view.as_ref().map(DiffView::commit_id) == Some(commit_id) {
            app.push_view(ViewKind::Diff);
            app.views.maximized = maximize;
            return;
        }
        let diff = engine
            .get_cached_diff(&commit_id)
            .or_else(|| engine.compute_commit_diff_cached(commit_id).ok());
        if let Some(diff_data) = diff {
            app.views.diff_view = Some(app.create_diff_view(diff_data));
            app.push_view(ViewKind::Diff);
            app.views.maximized = maximize;
        }
    }
}

/// Synchronizes the split `DiffView` pane with the current `MainView` selection if open.
pub fn sync_split_diff_if_open(app: &mut AppState) {
    if !app.views.view_stack.contains(&ViewKind::Diff) || app.views.diff_view.is_none() {
        return;
    }
    refresh_diff_for_main_selection(app);
}

/// Returns a cached `DiffView` for `item` at the current repository generation and render key,
/// or computes and caches it on miss.
pub fn get_or_create_status_item_diff_view(
    app: &mut AppState,
    item: tigrs_git::StatusItem,
) -> Option<DiffView> {
    let engine = app.engine.as_ref()?;
    let generation = engine.generation();
    let render_key = app.current_diff_render_key();
    let cache_key = (item.clone(), generation, render_key);
    if let Some(cached_view) = app.status_item_diff_cache.get(&cache_key) {
        return Some(cached_view.clone());
    }
    let diff_data = engine.compute_status_item_diff(&item).ok()?;
    let view = app.create_status_diff_view(diff_data, item);
    app.status_item_diff_cache.insert(cache_key, view.clone());
    Some(view)
}

/// Synchronizes the split `DiffView` pane with the current `StatusView` item selection if open,
/// consulting `status_item_diff_cache` first and offloading cold misses to `diff_task`.
pub fn sync_split_status_diff_if_open(app: &mut AppState) {
    if !app.views.view_stack.contains(&ViewKind::Diff) || app.views.diff_view.is_none() {
        return;
    }
    let Some(item) = app
        .views
        .status_view
        .as_ref()
        .and_then(|s| s.selected_item().cloned())
    else {
        return;
    };
    let Some(engine) = &app.engine else {
        return;
    };
    if app.views.diff_view.as_ref().and_then(DiffView::status_item) == Some(&item) {
        app.pending_diff_target = None;
        app.diff_task.cancel_in_flight();
        return;
    }
    let generation = engine.generation();
    let render_key = app.current_diff_render_key();
    if let Some(cached_view) =
        app.status_item_diff_cache
            .get(&(item.clone(), generation, render_key))
    {
        app.pending_diff_target = None;
        app.diff_task.cancel_in_flight();
        app.views.diff_view = Some(cached_view.clone());
        return;
    }
    if app.diff_task.has_sender() {
        app.diff_task.active_request_id = app.diff_task.active_request_id.wrapping_add(1);
        let req_id = app.diff_task.active_request_id;
        let deadline = Instant::now() + DIFF_DEBOUNCE_INTERVAL;
        app.pending_diff_target = Some((DiffRequestTarget::StatusItem(item), deadline, req_id));
        return;
    }
    app.pending_diff_target = None;
    if let Some(view) = get_or_create_status_item_diff_view(app, item) {
        app.views.diff_view = Some(view);
    }
}

/// Resolves the commit that `kind`'s cursor currently points at.
///
/// Only covers the views whose `Enter` action opens a commit diff, which are
/// exactly the views that can act as a parent of the diff view.
pub fn selected_commit_for_view(app: &AppState, kind: ViewKind) -> Option<tigrs_git::ObjectId> {
    match kind {
        ViewKind::Refs | ViewKind::Stash | ViewKind::Reflog | ViewKind::Log | ViewKind::Blame => {
            app.view_ref(kind).and_then(View::selected_commit_id)
        }
        _ => None,
    }
}

/// Synchronizes the split `DiffView` pane with `kind`'s selected commit if open.
pub fn sync_split_commit_diff_if_open(app: &mut AppState, kind: ViewKind) {
    if !app.views.view_stack.contains(&ViewKind::Diff) || app.views.diff_view.is_none() {
        return;
    }
    if let Some(commit_id) = selected_commit_for_view(app, kind) {
        request_commit_diff(app, commit_id);
    }
}

/// Navigation motions supported across all views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavMotion {
    /// Move the cursor down by `n` rows.
    Down(usize),
    /// Move the cursor up by `n` rows.
    Up(usize),
    /// Advance the viewport and cursor down by one full page.
    PageDown,
    /// Move the viewport and cursor up by one full page.
    PageUp,
    /// Advance the viewport and cursor down by half a page.
    HalfPageDown,
    /// Move the viewport and cursor up by half a page.
    HalfPageUp,
    /// Jump to the first row (`0`).
    FirstLine,
    /// Jump to the final row.
    LastLine,
    /// Scroll the viewport down by `n` lines without shifting focus.
    ScrollDown(usize),
    /// Scroll the viewport up by `n` lines without shifting focus.
    ScrollUp(usize),
    /// Place the cursor directly on the 0-based row index.
    SetCursor(usize),
}

impl AppState {
    /// Clears the active view instance of `kind` and cancels its background tasks.
    pub fn clear_view(&mut self, kind: ViewKind) {
        self.cancel_for_view(kind);
        self.views.clear_view(kind);
    }
}

/// Applies `motion` to `view`.
pub fn dispatch_motion(view: &mut dyn View, motion: NavMotion, visible_height: usize) {
    match motion {
        NavMotion::Down(n) => view.move_down_by(n, visible_height),
        NavMotion::Up(n) => view.move_up_by(n, visible_height),
        NavMotion::PageDown => view.page_down(visible_height),
        NavMotion::PageUp => view.page_up(visible_height),
        NavMotion::HalfPageDown => view.move_down_by(visible_height / 2, visible_height),
        NavMotion::HalfPageUp => view.move_up_by(visible_height / 2, visible_height),
        NavMotion::FirstLine => view.scroll_top(),
        NavMotion::LastLine => view.scroll_bottom(visible_height),
        NavMotion::ScrollDown(n) => view.scroll_line_down(n, visible_height),
        NavMotion::ScrollUp(n) => view.scroll_line_up(n, visible_height),
        NavMotion::SetCursor(row) => view.set_cursor(row, visible_height),
    }
}

/// Dispatches a `NavMotion` to the view corresponding to `kind`.
pub fn dispatch_nav_motion_for_view(
    app: &mut AppState,
    kind: ViewKind,
    motion: NavMotion,
    visible_height: usize,
) {
    if let Some(view) = app.view_mut(kind) {
        dispatch_motion(view, motion, visible_height);
    }
}

/// Dispatches a `NavMotion` to the current active view.
pub fn dispatch_nav_motion(app: &mut AppState, motion: NavMotion, visible_height: usize) {
    if let Some(kind) = app.active_view() {
        dispatch_nav_motion_for_view(app, kind, motion, visible_height);
    }
}

/// Returns the cursor row of the view identified by `kind`, if it exists.
pub fn view_cursor_index(app: &AppState, kind: ViewKind) -> Option<usize> {
    app.view_ref(kind).map(View::cursor)
}

/// Applies `motion` to the parent pane when a split secondary view is focused,
/// synchronizing the child pane afterward.
pub fn move_in_parent_view(app: &mut AppState, motion: NavMotion, visible_height: usize) -> Flow {
    let Some(parent) = app.parent_view() else {
        dispatch_nav_motion(app, motion, visible_height);
        sync_split_views_after_motion(app);
        return Flow::Continue;
    };

    let parent_height = app.visible_height_for(parent, visible_height);
    let before = view_cursor_index(app, parent);
    dispatch_nav_motion_for_view(app, parent, motion, parent_height);

    if view_cursor_index(app, parent) != before {
        sync_split_views_after_scroll(app, parent);
    }

    Flow::Continue
}

/// Scrolls `kind`'s viewport down by `n` lines without moving input focus.
pub fn scroll_view_down(app: &mut AppState, kind: ViewKind, n: usize, visible_height: usize) {
    dispatch_nav_motion_for_view(app, kind, NavMotion::ScrollDown(n), visible_height);
}

/// Scrolls `kind`'s viewport up by `n` lines without moving input focus.
pub fn scroll_view_up(app: &mut AppState, kind: ViewKind, n: usize, visible_height: usize) {
    dispatch_nav_motion_for_view(app, kind, NavMotion::ScrollUp(n), visible_height);
}

/// Scrolls `kind`'s viewport left by `n` columns.
pub fn scroll_view_left(app: &mut AppState, kind: ViewKind, n: usize) {
    if kind == ViewKind::Diff
        && let Some(ref mut diff) = app.views.diff_view
    {
        diff.scroll_left(n);
    }
}

/// Scrolls `kind`'s viewport right by `n` columns.
pub fn scroll_view_right(app: &mut AppState, kind: ViewKind, n: usize) {
    if kind == ViewKind::Diff
        && let Some(ref mut diff) = app.views.diff_view
    {
        diff.scroll_right(n);
    }
}

/// Resets `kind`'s horizontal scroll offset to column 0.
pub fn scroll_view_first_col(app: &mut AppState, kind: ViewKind) {
    if kind == ViewKind::Diff
        && let Some(ref mut diff) = app.views.diff_view
    {
        diff.scroll_first_col();
    }
}

/// Refreshes the split diff or blob pane after `kind`'s selection may have moved,
/// and triggers speculative diff prefetching on idle when single-pane.
pub fn sync_split_views_after_scroll(app: &mut AppState, kind: ViewKind) {
    match kind {
        ViewKind::Main => {
            sync_split_diff_if_open(app);
            schedule_speculative_diff_prefetch_if_idle(app);
        }
        ViewKind::Status => sync_split_status_diff_if_open(app),
        ViewKind::Refs | ViewKind::Stash | ViewKind::Reflog | ViewKind::Blame | ViewKind::Log => {
            sync_split_commit_diff_if_open(app, kind);
        }
        ViewKind::Tree => sync_split_tree_blob_if_open(app),
        ViewKind::Grep => sync_split_grep_blob_if_open(app),
        _ => {}
    }
}

/// Synchronizes the split `BlobView` pane with `TreeView`'s selected file entry if open.
pub fn sync_split_tree_blob_if_open(app: &mut AppState) {
    if !app.views.view_stack.contains(&ViewKind::Blob) || app.views.blob_view.is_none() {
        return;
    }
    let Some((entry_kind, entry_oid, entry_path, commit_id)) = app
        .views
        .tree_view
        .as_ref()
        .and_then(|t| match t.selected_row()? {
            crate::view::TreeRow::Entry(e) if !e.is_dir() => {
                Some((e.kind, e.oid, e.path.clone(), t.commit_oid()))
            }
            _ => None,
        })
    else {
        return;
    };
    if app.views.blob_view.as_ref().is_some_and(|bv| {
        bv.commit_oid() == commit_id && bv.blob_oid() == entry_oid && bv.path() == entry_path
    }) {
        return;
    }
    let Some(engine) = &app.engine else {
        return;
    };
    let blob_res = if entry_kind == tigrs_git::TreeEntryKind::Commit {
        Ok(tigrs_git::BlobContent {
            oid: entry_oid,
            path: entry_path,
            size: 0,
            is_binary: false,
            lines: tigrs_core::LineBuffer::from(vec![format!("Subproject commit {entry_oid}")]),
        })
    } else {
        engine.read_blob(entry_oid, &entry_path)
    };
    if let Ok(blob) = blob_res {
        app.views.blob_view = Some(crate::view::BlobView::new_with_options(
            commit_id,
            blob,
            &app.options,
        ));
    }
}

/// Synchronizes the split `BlobView` pane with `GrepView`'s selected match if open.
pub fn sync_split_grep_blob_if_open(app: &mut AppState) {
    if !app.views.view_stack.contains(&ViewKind::Blob) || app.views.blob_view.is_none() {
        return;
    }
    let Some((path, line_num)) = app
        .views
        .grep_view
        .as_ref()
        .and_then(|g| g.selected_match().map(|m| (m.path.clone(), m.line_num)))
    else {
        return;
    };
    let Some(engine) = &app.engine else {
        return;
    };
    let Ok(head_id) = engine.head_commit_id() else {
        return;
    };
    let visible = app.visible_height_for(ViewKind::Blob, 24);
    if let Some(ref mut bv) = app.views.blob_view
        && bv.commit_oid() == head_id
        && bv.path() == path
    {
        bv.set_cursor(line_num.saturating_sub(1), visible);
        return;
    }
    if let Ok(blob) = engine.read_blob_at_commit_path(head_id, &path) {
        let mut bv = crate::view::BlobView::new_with_options(head_id, blob, &app.options);
        bv.set_cursor(line_num.saturating_sub(1), visible);
        app.views.blob_view = Some(bv);
    }
}

/// Schedules a speculative background commit diff calculation when user idles on a commit.
pub fn schedule_speculative_diff_prefetch_if_idle(app: &mut AppState) {
    app.cancels.cancel_prefetch();
    let (changes_kind, commit_id) = main_selection(app);
    if changes_kind.is_some() {
        app.pending_speculative_prefetch = None;
        return;
    }
    if let (Some(commit_id), Some(engine)) = (commit_id, &app.engine) {
        let radius = app.options.memory_profile.prefetch_window_radius();
        if radius == 0 {
            app.pending_speculative_prefetch = None;
            return;
        }
        // Check the selected commit first, then ±1..=radius neighbors in main_view.
        let mut candidate_oid = None;
        if engine.get_cached_diff(&commit_id).is_none() {
            candidate_oid = Some(commit_id);
        } else if let Some(ref main) = app.views.main_view {
            let commits = main.commits();
            if let Some(center) = main.commit_index_for_id(&commit_id) {
                for dist in 1..=radius {
                    if let Some(next_c) = commits.get(center + dist)
                        && engine.get_cached_diff(&next_c.id).is_none()
                    {
                        candidate_oid = Some(next_c.id);
                        break;
                    }
                    if let Some(prev_idx) = center.checked_sub(dist)
                        && let Some(prev_c) = commits.get(prev_idx)
                        && engine.get_cached_diff(&prev_c.id).is_none()
                    {
                        candidate_oid = Some(prev_c.id);
                        break;
                    }
                }
            }
        }
        if let Some(target_oid) = candidate_oid {
            let deadline = Instant::now() + SPECULATIVE_PREFETCH_INTERVAL;
            app.pending_speculative_prefetch = Some((target_oid, deadline));
        } else {
            app.pending_speculative_prefetch = None;
        }
    } else {
        app.pending_speculative_prefetch = None;
    }
}

/// Synchronizes split views after a cursor motion in the currently active view.
pub fn sync_split_views_after_motion(app: &mut AppState) {
    if let Some(kind) = app.active_view() {
        sync_split_views_after_scroll(app, kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::MainView;

    #[test]
    fn test_view_ref_and_mut_dispatch() {
        let mut app = AppState::default();
        assert!(app.view_ref(ViewKind::Main).is_none());
        assert!(!app.has_view(ViewKind::Main));

        let mut main = MainView::new("main".to_string());
        main.set_cursor(0, 24);
        app.views.main_view = Some(main);

        assert!(app.view_ref(ViewKind::Main).is_some());
        assert!(app.has_view(ViewKind::Main));
        assert_eq!(view_cursor_index(&app, ViewKind::Main), Some(0));
        assert_eq!(
            app.view_ref(ViewKind::Main).map(View::scroll_offset),
            Some(0)
        );
        assert_eq!(app.view_ref(ViewKind::Main).map(View::line_count), Some(0));

        dispatch_nav_motion_for_view(&mut app, ViewKind::Main, NavMotion::Down(1), 24);
        assert_eq!(view_cursor_index(&app, ViewKind::Main), Some(0));

        app.clear_view(ViewKind::Main);
        assert!(app.views.main_view.is_none());
        assert!(!app.has_view(ViewKind::Main));
    }
}
