// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Option toggles and settings menu action handlers.

use crate::app::{AppState, Flow};
use crate::keymap::Action;
use crate::options::MenuAction;
use crate::prompt::PromptState;

/// Handles option toggles and settings menu actions.
/// Returns `Some(Flow)` if `action` was recognized as an options action, or `None` otherwise.
pub fn handle_options_action(app: &mut AppState, action: &Action) -> Option<Flow> {
    match action {
        Action::Options => {
            app.prompt = Some(PromptState::new_option_menu_for_view(app.active_view()));
            Some(Flow::Continue)
        }

        Action::ToggleOption(id) => {
            app.apply_menu_action(MenuAction::Toggle(*id));
            Some(Flow::Continue)
        }

        Action::ToggleDiffContext(delta) => {
            app.apply_menu_action(MenuAction::DiffContext(*delta));
            Some(Flow::Continue)
        }

        _ => None,
    }
}
