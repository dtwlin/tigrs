// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Action execution and keybinding command dispatch.

pub mod navigation;
pub mod options;
pub mod prompt;
pub mod staging;
pub mod view_switch;

pub use navigation::*;
pub use options::*;
pub use prompt::*;
pub use staging::*;
pub use view_switch::*;

use super::dispatch::{
    NavMotion, dispatch_nav_motion, move_in_parent_view, scroll_view_down, scroll_view_first_col,
    scroll_view_left, scroll_view_right, scroll_view_up, sync_split_views_after_motion,
    sync_split_views_after_scroll,
};
use super::{AppState, Flow};
use crate::keymap::Action;

/// Dispatches a resolved keymap action to the active view or application state.
pub fn execute_action(app: &mut AppState, action: &Action, visible_height: usize) -> Flow {
    // 1. Delegate option toggles and settings menu actions
    if let Some(flow) = options::handle_options_action(app, action) {
        return flow;
    }

    // Flush any coalesced diff/syntax refresh before running non-option actions (e.g. navigation/staging).
    app.flush_deferred_view_refresh();

    // 2. Delegate view switching and lifecycle actions
    match action {
        Action::Quit => Flow::Quit,

        Action::ViewClose => view_switch::handle_view_close(app),
        Action::OpenView(kind) => view_switch::handle_open_view(app, *kind, visible_height),
        Action::ViewStage => view_switch::handle_view_stage(app),
        Action::ViewNext => view_switch::handle_view_next(app),
        Action::Maximize => view_switch::handle_maximize(app),

        // Navigation & drill-down
        Action::Enter => navigation::handle_enter(app, visible_height),
        Action::Parent => navigation::handle_parent(app, visible_height),
        Action::Back => navigation::handle_back(app, visible_height),
        Action::NextHunk => navigation::handle_next_hunk(app, visible_height),
        Action::PrevHunk => navigation::handle_prev_hunk(app, visible_height),
        Action::NextFile => navigation::handle_next_file(app, visible_height),
        Action::PrevFile => navigation::handle_prev_file(app, visible_height),
        Action::ToggleFileFold => navigation::handle_toggle_file_fold(app, visible_height),
        Action::FoldAllFiles => navigation::handle_fold_all_files(app, visible_height),
        Action::UnfoldAllFiles => navigation::handle_unfold_all_files(app, visible_height),
        Action::ExpandHunkContext => navigation::handle_expand_hunk_context(app, visible_height),
        Action::ShrinkHunkContext => navigation::handle_shrink_hunk_context(app, visible_height),
        Action::ToggleDiffFileDetails => navigation::handle_toggle_diff_file_details(app),
        Action::YankDiffText => navigation::handle_yank_diff_text(app),

        // Stepping parent view selection in dual split view
        Action::Next => move_in_parent_view(app, NavMotion::Down(1), visible_height),
        Action::Previous => move_in_parent_view(app, NavMotion::Up(1), visible_height),

        // Motion & Scrolling
        Action::MoveDown => {
            dispatch_nav_motion(app, NavMotion::Down(1), visible_height);
            sync_split_views_after_motion(app);
            Flow::Continue
        }
        Action::MoveUp => {
            dispatch_nav_motion(app, NavMotion::Up(1), visible_height);
            sync_split_views_after_motion(app);
            Flow::Continue
        }
        Action::MovePageDown => {
            dispatch_nav_motion(app, NavMotion::PageDown, visible_height);
            sync_split_views_after_motion(app);
            Flow::Continue
        }
        Action::MovePageUp => {
            dispatch_nav_motion(app, NavMotion::PageUp, visible_height);
            sync_split_views_after_motion(app);
            Flow::Continue
        }
        Action::MoveHalfPageDown => {
            dispatch_nav_motion(app, NavMotion::HalfPageDown, visible_height);
            sync_split_views_after_motion(app);
            Flow::Continue
        }
        Action::MoveHalfPageUp => {
            dispatch_nav_motion(app, NavMotion::HalfPageUp, visible_height);
            sync_split_views_after_motion(app);
            Flow::Continue
        }
        Action::MoveFirstLine | Action::GotoHead => {
            dispatch_nav_motion(app, NavMotion::FirstLine, visible_height);
            sync_split_views_after_motion(app);
            Flow::Continue
        }
        Action::MoveLastLine => {
            dispatch_nav_motion(app, NavMotion::LastLine, visible_height);
            sync_split_views_after_motion(app);
            Flow::Continue
        }
        Action::ScrollLineDown => {
            if let Some(kind) = app.active_view() {
                scroll_view_down(app, kind, 1, visible_height);
                sync_split_views_after_scroll(app, kind);
            }
            Flow::Continue
        }
        Action::ScrollLineUp => {
            if let Some(kind) = app.active_view() {
                scroll_view_up(app, kind, 1, visible_height);
                sync_split_views_after_scroll(app, kind);
            }
            Flow::Continue
        }
        Action::ScrollPageDown => {
            if let Some(kind) = app.active_view() {
                let step = visible_height.saturating_sub(2).max(1);
                scroll_view_down(app, kind, step, visible_height);
                sync_split_views_after_scroll(app, kind);
            }
            Flow::Continue
        }
        Action::ScrollPageUp => {
            if let Some(kind) = app.active_view() {
                let step = visible_height.saturating_sub(2).max(1);
                scroll_view_up(app, kind, step, visible_height);
                sync_split_views_after_scroll(app, kind);
            }
            Flow::Continue
        }
        Action::ScrollLeft => {
            if let Some(kind) = app.active_view() {
                scroll_view_left(app, kind, 4);
            }
            Flow::Continue
        }
        Action::ScrollRight => {
            if let Some(kind) = app.active_view() {
                scroll_view_right(app, kind, 4);
            }
            Flow::Continue
        }
        Action::ScrollFirstCol => {
            if let Some(kind) = app.active_view() {
                scroll_view_first_col(app, kind);
            }
            Flow::Continue
        }

        // Staging & Reverting
        Action::StatusUpdate => {
            if app.check_read_only_blocked("status-update") {
                return Flow::Continue;
            }
            staging::handle_status_update(app, visible_height)
        }
        Action::StageUpdateLine => {
            if app.check_read_only_blocked("stage-update-line") {
                return Flow::Continue;
            }
            staging::handle_stage_update_line(app, visible_height)
        }
        Action::StageUpdatePart | Action::StageSplitChunk | Action::StatusMerge => {
            if app.check_read_only_blocked("stage-update") {
                return Flow::Continue;
            }
            Flow::Continue
        }
        Action::StatusRevert => {
            if app.check_read_only_blocked("status-revert") {
                return Flow::Continue;
            }
            staging::handle_status_revert(app, visible_height)
        }

        // Prompts, Search, External execution, Editor
        Action::Prompt => prompt::handle_prompt(app),
        Action::Search => prompt::handle_search(app),
        Action::SearchBack => prompt::handle_search_back(app),
        Action::Run(cmd) => {
            if !cmd.flags.contains(crate::keymap::RunFlags::INTERNAL)
                && app.check_read_only_blocked("external command")
            {
                return Flow::Continue;
            }
            prompt::handle_run_command_action(app, cmd)
        }
        Action::FindNext => prompt::handle_find_next(app, visible_height),
        Action::FindPrev => prompt::handle_find_prev(app, visible_height),
        Action::Edit => {
            if app.check_read_only_blocked("edit") {
                return Flow::Continue;
            }
            prompt::handle_edit(app)
        }

        // Utility & System Actions
        Action::ScreenRedraw => {
            app.invalidate_screen();
            Flow::Continue
        }
        Action::StopLoading => {
            app.cancel_all();
            app.status_message = Some("Stopped background loading".to_string());
            Flow::Continue
        }
        Action::ShowVersion => {
            app.status_message = Some(format!("tigrs version {}", env!("CARGO_PKG_VERSION")));
            Flow::Continue
        }
        Action::Suspend => {
            app.pending_suspend = true;
            Flow::Continue
        }
        Action::Refresh => {
            if let Some(ref mut eng) = app.engine {
                let _ = eng.purge_caches();
            }
            if let Some(eng) = app.engine.clone() {
                match app.active_view() {
                    Some(crate::app::layout::ViewKind::Status) => {
                        let old_cursor = app
                            .views
                            .status_view
                            .as_ref()
                            .map_or(0, crate::view::StatusView::cursor_index);
                        let (_src, token) = tigrs_core::cancel::CancellationToken::new();
                        if let Ok(report) = eng.load_status(&token) {
                            app.changes_report = Some(report.clone());
                            let _ = app.apply_changes_rows();
                            let mut new_status = crate::view::StatusView::new(report);
                            new_status.set_cursor(old_cursor, visible_height);
                            app.views.status_view = Some(new_status);
                        }
                    }
                    Some(crate::app::layout::ViewKind::Refs) => {
                        if let Ok(refs) = eng.list_refs() {
                            app.views.refs_view = Some(crate::view::RefsView::new(refs));
                        }
                    }
                    Some(crate::app::layout::ViewKind::Stash) => {
                        if let Ok(stashes) = eng.list_stashes() {
                            app.views.stash_view = Some(crate::view::StashView::new(stashes));
                        }
                    }
                    Some(crate::app::layout::ViewKind::Diff) => {
                        app.refresh_diff_view();
                    }
                    _ => {
                        let (_src, token) = tigrs_core::cancel::CancellationToken::new();
                        if let Ok(report) = eng.load_status(&token) {
                            app.changes_report = Some(report);
                            let _ = app.apply_changes_rows();
                        }
                        app.refresh_diff_view();
                    }
                }
            }
            app.invalidate_screen();
            app.status_message = Some("Refreshed".to_string());
            Flow::Continue
        }

        _ => Flow::Continue,
    }
}
