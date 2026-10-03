// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Staging, hunk management, and reverting action handlers.

use crate::app::layout::ViewKind;
use crate::app::{AppState, Flow};
use crate::view::StatusView;
use tigrs_core::cancel::CancellationToken;
use tigrs_git::StatusSection;

/// Handles the `StatusUpdate` action: stages/unstages the selected file in `StatusView`
/// or the selected hunk in `DiffView`.
pub fn handle_status_update(app: &mut AppState, visible_height: usize) -> Flow {
    match app.active_view() {
        Some(ViewKind::Status) => {
            let item = app
                .views
                .status_view
                .as_ref()
                .and_then(|s| s.selected_item().cloned());
            if let (Some(item), Some(engine)) = (item, &app.engine) {
                let res = match item.section {
                    StatusSection::Staged => {
                        engine.unstage_file_os(item.os_path(), item.os_old_path())
                    }
                    StatusSection::Unstaged | StatusSection::Untracked => {
                        engine.stage_file_os(item.os_path(), item.os_old_path())
                    }
                    StatusSection::Unmerged => Ok(()),
                };
                match res {
                    Ok(()) => {
                        let old_cursor = app
                            .views
                            .status_view
                            .as_ref()
                            .map_or(0, StatusView::cursor_index);
                        let (_src, token) = CancellationToken::new();
                        if let Ok(report) = engine.load_status(&token) {
                            app.changes_report = Some(report.clone());
                            let _ = app.apply_changes_rows();
                            let mut new_status = StatusView::new(report);
                            new_status.set_cursor(old_cursor, visible_height);
                            app.views.status_view = Some(new_status);
                        }
                    }
                    Err(e) => {
                        app.status_message = Some(format!("Staging failed: {e}"));
                    }
                }
            }
        }
        Some(ViewKind::Diff) => {
            let action = app.views.diff_view.as_ref().and_then(|diff| {
                let status_item = diff.status_item()?.clone();
                let (_file, hunk) = diff.selected_hunk()?;
                Some((status_item, hunk.clone(), diff.cursor_index()))
            });
            if let Some((status_item, hunk, old_cursor)) = action {
                let outcome = app.engine.as_ref().map(|engine| {
                    let res = match status_item.section {
                        StatusSection::Staged => {
                            engine.unstage_hunk_bytes(status_item.raw_path_bytes(), &hunk)
                        }
                        StatusSection::Untracked => {
                            engine.stage_file_os(status_item.os_path(), status_item.os_old_path())
                        }
                        StatusSection::Unstaged | StatusSection::Unmerged => {
                            engine.stage_hunk_bytes(status_item.raw_path_bytes(), &hunk)
                        }
                    };
                    res.map(|()| engine.compute_status_item_diff(&status_item))
                });
                match outcome {
                    Some(Ok(Ok(diff_data))) => {
                        let mut new_view = app.create_status_diff_view(diff_data, status_item);
                        new_view.set_cursor(old_cursor, visible_height);
                        app.views.diff_view = Some(new_view);
                        if let Some(ref engine) = app.engine {
                            let (_src, token) = CancellationToken::new();
                            if let Ok(report) = engine.load_status(&token) {
                                app.changes_report = Some(report.clone());
                                let _ = app.apply_changes_rows();
                                if let Some(ref mut status) = app.views.status_view {
                                    status.refresh(report);
                                }
                            }
                        }
                    }
                    Some(Ok(Err(e))) => {
                        app.status_message = Some(format!("Failed to refresh diff: {e}"));
                    }
                    Some(Err(e)) => {
                        app.status_message = Some(format!("Stage hunk failed: {e}"));
                    }
                    None => {}
                }
            }
        }
        _ => {}
    }
    Flow::Continue
}

/// Handles the `StageUpdateLine` action: stages or unstages individual selected diff lines.
pub fn handle_stage_update_line(app: &mut AppState, visible_height: usize) -> Flow {
    let action = app.views.diff_view.as_ref().and_then(|diff| {
        let status_item = diff.status_item()?.clone();
        let (_file, hunk, line_indices) = diff.selected_lines()?;
        Some((status_item, hunk.clone(), line_indices, diff.cursor_index()))
    });
    if let Some((status_item, hunk, line_indices, old_cursor)) = action {
        let outcome = app.engine.as_ref().map(|engine| {
            let is_staged = status_item.section == StatusSection::Staged;
            let raw_path = status_item.raw_path_bytes();
            let res = if is_staged {
                engine.unstage_lines_bytes(raw_path, &hunk, &line_indices)
            } else {
                engine.stage_lines_bytes(raw_path, &hunk, &line_indices)
            };
            res.map(|()| engine.compute_status_item_diff(&status_item))
        });
        match outcome {
            Some(Ok(Ok(diff_data))) => {
                let mut new_view = app.create_status_diff_view(diff_data, status_item);
                new_view.set_cursor(old_cursor, visible_height);
                app.views.diff_view = Some(new_view);
                if let Some(ref engine) = app.engine {
                    let (_src, token) = CancellationToken::new();
                    if let Ok(report) = engine.load_status(&token) {
                        app.changes_report = Some(report.clone());
                        let _ = app.apply_changes_rows();
                        if let Some(ref mut status) = app.views.status_view {
                            status.refresh(report);
                        }
                    }
                }
            }
            Some(Ok(Err(e))) => {
                app.status_message = Some(format!("Failed to refresh diff: {e}"));
            }
            Some(Err(e)) => {
                app.status_message = Some(format!("Stage line failed: {e}"));
            }
            None => {}
        }
    }
    Flow::Continue
}

/// Handles the `StatusRevert` action: discards unstaged or untracked changes for the selected file or hunk.
pub fn handle_status_revert(app: &mut AppState, visible_height: usize) -> Flow {
    if app.active_view() == Some(ViewKind::Status) {
        let item = app
            .views
            .status_view
            .as_ref()
            .and_then(|s| s.selected_item().cloned());
        if let (Some(item), Some(engine)) = (item, &app.engine) {
            let res = match item.section {
                StatusSection::Unstaged => {
                    engine.discard_file_changes_os(item.os_path(), item.os_old_path())
                }
                StatusSection::Untracked => engine.discard_untracked_file_os(item.os_path()),
                _ => Ok(()),
            };
            match res {
                Ok(()) => {
                    let old_cursor = app
                        .views
                        .status_view
                        .as_ref()
                        .map_or(0, StatusView::cursor_index);
                    let (_src, token) = CancellationToken::new();
                    if let Ok(report) = engine.load_status(&token) {
                        app.changes_report = Some(report.clone());
                        let _ = app.apply_changes_rows();
                        let mut new_status = StatusView::new(report);
                        new_status.set_cursor(old_cursor, visible_height);
                        app.views.status_view = Some(new_status);
                    }
                }
                Err(e) => {
                    app.status_message = Some(format!("Revert failed: {e}"));
                }
            }
        }
    } else if app.active_view() == Some(ViewKind::Diff) {
        let action_res = {
            let Some(ref diff) = app.views.diff_view else {
                return Flow::Continue;
            };
            let Some(status_item) = diff.status_item().cloned() else {
                app.status_message =
                    Some("Cannot revert in commit diff (open from Status view)".to_string());
                return Flow::Continue;
            };
            let old_cursor = diff.cursor_index();
            let selected_hunk = diff.selected_hunk().map(|(_, h)| h.clone());
            let Some(ref engine) = app.engine else {
                return Flow::Continue;
            };
            let res = match status_item.section {
                StatusSection::Unstaged | StatusSection::Unmerged => {
                    if let Some(ref hunk) = selected_hunk {
                        engine.discard_hunk_bytes(status_item.raw_path_bytes(), hunk)
                    } else {
                        engine.discard_file_changes_os(
                            status_item.os_path(),
                            status_item.os_old_path(),
                        )
                    }
                }
                StatusSection::Untracked => engine.discard_untracked_file_os(status_item.os_path()),
                StatusSection::Staged => {
                    app.status_message =
                        Some("Cannot revert staged changes (press 'u' to unstage)".to_string());
                    return Flow::Continue;
                }
            };
            Some(res.map(|()| (status_item, old_cursor)))
        };

        match action_res {
            Some(Ok((status_item, old_cursor))) => {
                if let Some(engine) = app.engine.clone() {
                    if let Ok(new_diff_data) = engine.compute_status_item_diff(&status_item) {
                        if new_diff_data.files.is_empty() {
                            app.pop_active_view();
                        } else {
                            let mut new_diff =
                                app.create_status_diff_view(new_diff_data, status_item);
                            new_diff.set_cursor(old_cursor, visible_height);
                            app.views.diff_view = Some(new_diff);
                        }
                    }
                    let (_src, token) = CancellationToken::new();
                    if let Ok(report) = engine.load_status(&token) {
                        app.changes_report = Some(report.clone());
                        let _ = app.apply_changes_rows();
                        if let Some(ref mut status) = app.views.status_view {
                            status.refresh(report);
                        }
                    }
                }
            }
            Some(Err(e)) => {
                app.status_message = Some(format!("Revert failed: {e}"));
            }
            None => {}
        }
    }
    Flow::Continue
}
