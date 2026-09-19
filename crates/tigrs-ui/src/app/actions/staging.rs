// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Staging, hunk management, and reverting action handlers.

use crate::app::layout::ViewKind;
use crate::app::{AppState, Flow};
use std::sync::Arc;
use tigrs_core::cancel::CancellationToken;
use tigrs_git::{CommitDiff, StatusItem, StatusSection};

fn refresh_status_report_and_views(app: &mut AppState, visible_height: usize) {
    if let Some(ref engine) = app.engine {
        let (_src, token) = CancellationToken::new();
        if let Ok(report) = engine.load_status(&token) {
            app.changes_report = Some(report.clone());
            let _ = app.apply_changes_rows();
            let is_status_active = app.active_view() == Some(ViewKind::Status);
            if let Some(ref mut status) = app.views.status_view {
                status.refresh(report);
                if is_status_active {
                    let cur = status.cursor_index();
                    status.set_cursor(cur, visible_height);
                }
            }
        }
    }
}

fn apply_updated_status_diff(
    app: &mut AppState,
    diff_data: CommitDiff,
    status_item: StatusItem,
    visible_height: usize,
) {
    let is_empty_diff = diff_data.files.is_empty()
        || diff_data
            .files
            .iter()
            .all(|f| f.hunks.is_empty() && !f.is_binary);
    if is_empty_diff {
        app.pop_active_view();
        refresh_status_report_and_views(app, visible_height);
        crate::app::dispatch::sync_split_status_diff_if_open(app);
        return;
    }

    if let Some(mut diff_view) = app.views.diff_view.take() {
        let options = app.options.clone();
        let caps = app.caps.clone();
        let diff_arc = Arc::new(diff_data);
        let item_for_provider = status_item.clone();
        app.with_status_blob_provider(Some(&item_for_provider), |provider| {
            diff_view.replace_status_diff_preserving_state(
                diff_arc,
                status_item,
                &options,
                &caps,
                provider,
                visible_height,
            );
        });
        app.views.diff_view = Some(diff_view);
    } else {
        let new_view = app.create_status_diff_view(diff_data, status_item);
        app.views.diff_view = Some(new_view);
    }
    refresh_status_report_and_views(app, visible_height);
}

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
                        refresh_status_report_and_views(app, visible_height);
                        crate::app::dispatch::sync_split_status_diff_if_open(app);
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
                Some((status_item, hunk.clone()))
            });
            if let Some((mut status_item, hunk)) = action {
                let outcome = app.engine.as_ref().map(|engine| {
                    let res = match status_item.section {
                        StatusSection::Staged => {
                            engine.unstage_hunk_bytes(status_item.raw_path_bytes(), &hunk)
                        }
                        StatusSection::Untracked => {
                            let r = engine
                                .stage_file_os(status_item.os_path(), status_item.os_old_path());
                            if r.is_ok() {
                                status_item.section = StatusSection::Unstaged;
                                status_item.status_code = 'M';
                            }
                            r
                        }
                        StatusSection::Unstaged | StatusSection::Unmerged => {
                            engine.stage_hunk_bytes(status_item.raw_path_bytes(), &hunk)
                        }
                    };
                    res.map(|()| engine.compute_status_item_diff(&status_item))
                });
                match outcome {
                    Some(Ok(Ok(diff_data))) => {
                        apply_updated_status_diff(app, diff_data, status_item, visible_height);
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
        Some((status_item, hunk.clone(), line_indices))
    });
    if let Some((mut status_item, hunk, line_indices)) = action {
        let outcome = app.engine.as_ref().map(|engine| {
            let is_staged = status_item.section == StatusSection::Staged;
            let raw_path = status_item.raw_path_bytes();
            let res = if is_staged {
                engine.unstage_lines_bytes(raw_path, &hunk, &line_indices)
            } else {
                let r = engine.stage_lines_bytes(raw_path, &hunk, &line_indices);
                if r.is_ok() && status_item.section == StatusSection::Untracked {
                    status_item.section = StatusSection::Unstaged;
                    status_item.status_code = 'M';
                }
                r
            };
            res.map(|()| engine.compute_status_item_diff(&status_item))
        });
        match outcome {
            Some(Ok(Ok(diff_data))) => {
                apply_updated_status_diff(app, diff_data, status_item, visible_height);
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
                    refresh_status_report_and_views(app, visible_height);
                    crate::app::dispatch::sync_split_status_diff_if_open(app);
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
            let selected_hunk = diff.selected_hunk().map(|(_, h)| h.clone());
            let Some(ref engine) = app.engine else {
                return Flow::Continue;
            };
            let res = match status_item.section {
                StatusSection::Unstaged | StatusSection::Unmerged => {
                    if let Some(ref hunk) = selected_hunk {
                        engine.discard_hunk_bytes(status_item.raw_path_bytes(), hunk)
                    } else {
                        app.status_message = Some("No diff hunk selected".to_string());
                        return Flow::Continue;
                    }
                }
                StatusSection::Untracked => {
                    if selected_hunk.is_some() {
                        engine.discard_untracked_file_os(status_item.os_path())
                    } else {
                        app.status_message = Some("No diff hunk selected".to_string());
                        return Flow::Continue;
                    }
                }
                StatusSection::Staged => {
                    app.status_message =
                        Some("Cannot revert staged changes (press 'u' to unstage)".to_string());
                    return Flow::Continue;
                }
            };
            Some(res.map(|()| status_item))
        };

        match action_res {
            Some(Ok(status_item)) => {
                if status_item.section == StatusSection::Untracked {
                    app.pop_active_view();
                    refresh_status_report_and_views(app, visible_height);
                    crate::app::dispatch::sync_split_status_diff_if_open(app);
                } else if let Some(engine) = app.engine.clone() {
                    if let Ok(new_diff_data) = engine.compute_status_item_diff(&status_item) {
                        apply_updated_status_diff(app, new_diff_data, status_item, visible_height);
                    } else {
                        refresh_status_report_and_views(app, visible_height);
                        crate::app::dispatch::sync_split_status_diff_if_open(app);
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
