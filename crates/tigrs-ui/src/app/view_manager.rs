// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! View navigation stack, split-window pairing, and polymorphic `View` lookup (`ViewManager`).

use super::layout::ViewKind;
use crate::view::{
    BlameView, BlobView, DiffView, GrepView, HelpView, LogView, MainView, PagerView, ReflogView,
    RefsView, StashView, StatusView, TreeView, View,
};

pub(crate) const STACKABLE_VIEWS: [ViewKind; 6] = [
    ViewKind::Status,
    ViewKind::Tree,
    ViewKind::Blob,
    ViewKind::Blame,
    ViewKind::Diff,
    ViewKind::Log,
];

pub(crate) const FALLBACK_VIEWS: [ViewKind; 6] = [
    ViewKind::Help,
    ViewKind::Refs,
    ViewKind::Stash,
    ViewKind::Grep,
    ViewKind::Reflog,
    ViewKind::Pager,
];

/// Unsizes a concrete view reference to `&dyn View`.
#[inline]
fn as_view<V: View>(view: &V) -> &dyn View {
    view
}

/// Unsizes a concrete view reference to `&mut dyn View`.
#[inline]
fn as_view_mut<V: View>(view: &mut V) -> &mut dyn View {
    view
}

/// Owned manager for all 13 canonical views, view navigation stack, and split-window layout flags.
#[derive(Default)]
pub struct ViewManager {
    /// Commit graph and revision history (`main`) view instance.
    pub main_view: Option<MainView>,
    /// Commit and working-tree patch (`diff`) view instance.
    pub diff_view: Option<DiffView>,
    /// Working-tree file staging (`status`) view instance.
    pub status_view: Option<StatusView>,
    /// Repository directory browser (`tree`) view instance.
    pub tree_view: Option<TreeView>,
    /// File content viewer (`blob`) view instance.
    pub blob_view: Option<BlobView>,
    /// Line authorship annotation (`blame`) view instance.
    pub blame_view: Option<BlameView>,
    /// Keybinding and command reference (`help`) view instance.
    pub help_view: Option<HelpView>,
    /// Branches, tags, and stashes (`refs`) view instance.
    pub refs_view: Option<RefsView>,
    /// Git stash stack (`stash`) view instance.
    pub stash_view: Option<StashView>,
    /// Repository regex search (`grep`) view instance.
    pub grep_view: Option<GrepView>,
    /// Reference transaction history (`reflog`) view instance.
    pub reflog_view: Option<ReflogView>,
    /// Detailed commit log with diffstats (`log`) view instance.
    pub log_view: Option<LogView>,
    /// Raw stdin / command output (`pager`) view instance.
    pub pager_view: Option<PagerView>,
    /// Ordered navigation stack of open views (top is focused).
    pub view_stack: Vec<ViewKind>,
    /// Primary pane view anchored when a secondary split pane is open.
    pub split_base: Option<ViewKind>,
    /// Whether the active pane is temporarily maximized (`O`) over a split layout.
    pub maximized: bool,
}

impl ViewManager {
    /// Returns whether the view of `kind` is currently instantiated.
    #[must_use]
    pub fn has_view(&self, kind: ViewKind) -> bool {
        self.view_ref(kind).is_some()
    }

    /// Returns a borrowed reference to the view corresponding to `kind`, if active.
    #[must_use]
    pub fn view_ref(&self, kind: ViewKind) -> Option<&dyn View> {
        match kind {
            ViewKind::Main => self.main_view.as_ref().map(as_view),
            ViewKind::Diff => self.diff_view.as_ref().map(as_view),
            ViewKind::Status => self.status_view.as_ref().map(as_view),
            ViewKind::Tree => self.tree_view.as_ref().map(as_view),
            ViewKind::Blob => self.blob_view.as_ref().map(as_view),
            ViewKind::Blame => self.blame_view.as_ref().map(as_view),
            ViewKind::Help => self.help_view.as_ref().map(as_view),
            ViewKind::Refs => self.refs_view.as_ref().map(as_view),
            ViewKind::Stash => self.stash_view.as_ref().map(as_view),
            ViewKind::Grep => self.grep_view.as_ref().map(as_view),
            ViewKind::Reflog => self.reflog_view.as_ref().map(as_view),
            ViewKind::Log => self.log_view.as_ref().map(as_view),
            ViewKind::Pager => self.pager_view.as_ref().map(as_view),
        }
    }

    /// Returns a mutable reference to the view corresponding to `kind`, if active.
    #[must_use]
    pub fn view_mut(&mut self, kind: ViewKind) -> Option<&mut dyn View> {
        match kind {
            ViewKind::Main => self.main_view.as_mut().map(as_view_mut),
            ViewKind::Diff => self.diff_view.as_mut().map(as_view_mut),
            ViewKind::Status => self.status_view.as_mut().map(as_view_mut),
            ViewKind::Tree => self.tree_view.as_mut().map(as_view_mut),
            ViewKind::Blob => self.blob_view.as_mut().map(as_view_mut),
            ViewKind::Blame => self.blame_view.as_mut().map(as_view_mut),
            ViewKind::Help => self.help_view.as_mut().map(as_view_mut),
            ViewKind::Refs => self.refs_view.as_mut().map(as_view_mut),
            ViewKind::Stash => self.stash_view.as_mut().map(as_view_mut),
            ViewKind::Grep => self.grep_view.as_mut().map(as_view_mut),
            ViewKind::Reflog => self.reflog_view.as_mut().map(as_view_mut),
            ViewKind::Log => self.log_view.as_mut().map(as_view_mut),
            ViewKind::Pager => self.pager_view.as_mut().map(as_view_mut),
        }
    }

    /// Clears the active view instance of `kind`.
    pub fn clear_view(&mut self, kind: ViewKind) {
        match kind {
            ViewKind::Main => self.main_view = None,
            ViewKind::Diff => self.diff_view = None,
            ViewKind::Status => self.status_view = None,
            ViewKind::Tree => self.tree_view = None,
            ViewKind::Blob => self.blob_view = None,
            ViewKind::Blame => self.blame_view = None,
            ViewKind::Help => self.help_view = None,
            ViewKind::Refs => self.refs_view = None,
            ViewKind::Stash => self.stash_view = None,
            ViewKind::Grep => self.grep_view = None,
            ViewKind::Reflog => self.reflog_view = None,
            ViewKind::Log => self.log_view = None,
            ViewKind::Pager => self.pager_view = None,
        }
    }

    /// Returns the currently active view at the top of the stack.
    #[must_use]
    pub fn active_view(&self) -> Option<ViewKind> {
        self.view_stack
            .iter()
            .rev()
            .copied()
            .find(|&kind| self.has_view(kind))
            .or_else(|| ViewKind::ALL.into_iter().find(|&kind| self.has_view(kind)))
    }

    /// Returns the *parent* of the active view: the view it was opened from.
    #[must_use]
    pub fn parent_view(&self) -> Option<ViewKind> {
        let active = self.active_view()?;
        let active_pos = self.view_stack.iter().rposition(|&kind| kind == active)?;
        self.view_stack[..active_pos]
            .iter()
            .rev()
            .copied()
            .find(|&kind| kind != active && self.has_view(kind))
    }

    /// Returns the (primary, secondary) views for dual split-window display, if split view is possible.
    #[must_use]
    pub fn split_views(&self) -> Option<(ViewKind, ViewKind)> {
        if self.view_stack.len() < 2 {
            return None;
        }
        let active = self.active_view()?;
        let base = self.split_base.filter(|b| self.view_stack.contains(b));
        let primary = base.unwrap_or(self.view_stack[0]);
        let secondary = self
            .view_stack
            .iter()
            .rev()
            .find(|&&v| v != primary)
            .copied()?;

        if !matches!(secondary, ViewKind::Diff | ViewKind::Pager | ViewKind::Blob) {
            return None;
        }

        if active != primary && active != secondary {
            return None;
        }

        Some((primary, secondary))
    }

    /// Ensures the view stack accurately reflects any directly initialized views.
    pub fn ensure_view_stack(&mut self) {
        let mut present = [false; 13];
        for kind in ViewKind::ALL {
            present[kind as usize] = self.has_view(kind);
        }
        self.view_stack.retain(|&kind| present[kind as usize]);

        if self.has_view(ViewKind::Main) && !self.view_stack.contains(&ViewKind::Main) {
            self.view_stack.insert(0, ViewKind::Main);
        }
        for kind in STACKABLE_VIEWS {
            if self.has_view(kind) && !self.view_stack.contains(&kind) {
                self.view_stack.push(kind);
            }
        }
        if self.view_stack.is_empty() {
            for kind in FALLBACK_VIEWS {
                if self.has_view(kind) {
                    self.view_stack.push(kind);
                    break;
                }
            }
        }
    }

    /// Pushes a view kind onto the stack.
    pub fn push_view(&mut self, kind: ViewKind) {
        self.ensure_view_stack();
        if matches!(kind, ViewKind::Diff | ViewKind::Pager | ViewKind::Blob)
            && let Some(current) = self
                .view_stack
                .iter()
                .rev()
                .find(|&&v| {
                    v != kind && !matches!(v, ViewKind::Diff | ViewKind::Pager | ViewKind::Blob)
                })
                .copied()
        {
            self.split_base = Some(current);
        } else if self.split_base.is_none() && !self.view_stack.is_empty() {
            let first = self.view_stack[0];
            self.split_base = Some(first);
        }
        self.view_stack.retain(|&v| v != kind);
        self.view_stack.push(kind);
    }

    /// Pops the active view from the stack and clears its field.
    pub fn pop_active_view(&mut self) -> Option<ViewKind> {
        self.ensure_view_stack();
        let popped = self.view_stack.pop();
        if let Some(p) = popped {
            self.clear_view(p);
        }
        if self.view_stack.len() < 2 || self.split_views().is_none() {
            self.split_base = None;
        }
        let has_split_secondary = self
            .view_stack
            .iter()
            .any(|&v| matches!(v, ViewKind::Diff | ViewKind::Pager | ViewKind::Blob));
        if self.view_stack.len() < 2 || !has_split_secondary {
            self.maximized = false;
        }
        popped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use crate::view::{MainView, PagerView};

    #[test]
    fn test_view_stack_push_pop_parent_and_split_base_invariants() {
        let mut app = AppState::default();
        app.views.main_view = Some(MainView::new("main".to_string()));
        app.push_view(ViewKind::Main);
        assert_eq!(app.active_view(), Some(ViewKind::Main));
        assert_eq!(app.parent_view(), None);

        // Pushing Pager (or Diff) over Main records Main as split_base and resolves Main as parent_view
        app.views.pager_view = Some(PagerView::new("Pager".to_string(), Vec::new()));
        app.push_view(ViewKind::Pager);
        assert_eq!(app.active_view(), Some(ViewKind::Pager));
        assert_eq!(app.parent_view(), Some(ViewKind::Main));
        assert_eq!(app.views.split_base, Some(ViewKind::Main));

        // Re-pushing an existing view deduplicates it and moves it to the top of the stack
        app.push_view(ViewKind::Main);
        assert_eq!(app.views.view_stack, vec![ViewKind::Pager, ViewKind::Main]);

        // Popping active view restores previous view and clears split_base when < 2 views remain
        assert_eq!(app.pop_active_view(), Some(ViewKind::Main));
        assert_eq!(app.active_view(), Some(ViewKind::Pager));
        assert_eq!(app.views.split_base, None);
    }
}
