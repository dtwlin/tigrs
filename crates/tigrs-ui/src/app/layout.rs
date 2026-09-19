// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Screen geometry, split layouts, and view classification.

/// Classification of interactive views in the application stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum ViewKind {
    /// Main commit history and revision graph view.
    Main = 0,
    /// Working-tree status and interactive staging view.
    Status = 1,
    /// Repository directory tree browser view.
    Tree = 2,
    /// File blob content viewer.
    Blob = 3,
    /// Per-line commit blame annotation view.
    Blame = 4,
    /// Commit and working-tree diff patch viewer.
    Diff = 5,
    /// Keybinding and command help overlay.
    Help = 6,
    /// Local/remote branches, tags, and stashes reference list view.
    Refs = 7,
    /// Git stash stack browser view.
    Stash = 8,
    /// Repository-wide regex search (`grep`) results view.
    Grep = 9,
    /// Reference transaction log (`reflog`) view.
    Reflog = 10,
    /// Detailed commit log with diffstats view.
    Log = 11,
    /// Standard input / external command output pager view.
    Pager = 12,
}

impl ViewKind {
    /// All 13 canonical view kinds in default fallback priority order.
    pub const ALL: [Self; 13] = [
        Self::Diff,
        Self::Blame,
        Self::Blob,
        Self::Tree,
        Self::Status,
        Self::Main,
        Self::Help,
        Self::Refs,
        Self::Stash,
        Self::Grep,
        Self::Reflog,
        Self::Log,
        Self::Pager,
    ];

    /// Returns the canonical Tig action name that opens this view.
    #[must_use]
    pub const fn action_name(self) -> &'static str {
        match self {
            Self::Main => "view-main",
            Self::Status => "view-status",
            Self::Tree => "view-tree",
            Self::Blob => "view-blob",
            Self::Blame => "view-blame",
            Self::Diff => "view-diff",
            Self::Help => "view-help",
            Self::Refs => "view-refs",
            Self::Stash => "view-stash",
            Self::Grep => "view-grep",
            Self::Reflog => "view-reflog",
            Self::Log => "view-log",
            Self::Pager => "view-pager",
        }
    }

    /// Resolves a canonical Tig action name (e.g. `"view-main"`) to its view kind.
    ///
    /// Matching is case-sensitive; callers are expected to lowercase first.
    #[must_use]
    pub fn from_action_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.action_name() == name)
    }
}

/// Minimum terminal width required to render a side-by-side (vertical) split.
pub const VSPLIT_MIN_WIDTH: u16 = 30;
/// Minimum terminal height required to render a side-by-side (vertical) split.
pub const VSPLIT_MIN_HEIGHT: u16 = 4;
/// Minimum terminal width required to render a stacked (horizontal) split.
pub const HSPLIT_MIN_WIDTH: u16 = 20;
/// Minimum terminal height required to render a stacked (horizontal) split.
pub const HSPLIT_MIN_HEIGHT: u16 = 6;

/// Rows every view reserves for chrome: one title bar plus one status bar.
pub const VIEW_CHROME_ROWS: usize = 2;

/// Number of content lines scrolled per mouse wheel notch.
pub const MOUSE_SCROLL_LINES: usize = 3;

/// Screen rectangle occupied by one rendered view pane.
///
/// Coordinates are zero-based and expressed in terminal cells, matching the
/// coordinate space of [`crossterm::event::MouseEvent`], so a pane can be
/// hit-tested directly against a pointer position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneLayout {
    /// View rendered into this pane.
    pub kind: ViewKind,
    /// Zero-based column of the pane's left edge.
    pub x: u16,
    /// Zero-based row of the pane's top edge.
    pub y: u16,
    /// Pane width in columns.
    pub width: u16,
    /// Pane height in rows, including the title and status bars.
    pub height: u16,
}

impl PaneLayout {
    /// Returns the number of content rows the pane can display, excluding chrome.
    #[must_use]
    pub fn visible_height(&self) -> usize {
        (self.height as usize).saturating_sub(VIEW_CHROME_ROWS)
    }

    /// Reports whether the zero-based terminal cell `(column, row)` falls inside the pane.
    #[must_use]
    pub fn contains(&self, column: u16, row: u16) -> bool {
        column >= self.x
            && column < self.x.saturating_add(self.width)
            && row >= self.y
            && row < self.y.saturating_add(self.height)
    }
}

/// Geometry of the panes currently displayed on screen.
///
/// This is the single source of truth shared by rendering and input routing:
/// `render_active_direct` draws exactly the rectangles described here, and
/// mouse events are dispatched by hit-testing against the same rectangles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewLayout {
    /// A single view occupying the whole terminal.
    Single(PaneLayout),
    /// Two views displayed simultaneously, separated either side-by-side
    /// (`vertical`) or stacked top/bottom.
    Split {
        /// Pane holding the base view (left or top).
        primary: PaneLayout,
        /// Pane holding the child view (right or bottom).
        secondary: PaneLayout,
        /// Whether the panes are arranged side-by-side rather than stacked.
        vertical: bool,
    },
}

impl ViewLayout {
    /// Returns the pane containing the zero-based terminal cell `(column, row)`.
    ///
    /// Returns `None` when the position falls on the vertical split separator
    /// column, which belongs to no pane.
    #[must_use]
    pub fn pane_at(&self, column: u16, row: u16) -> Option<PaneLayout> {
        match self {
            Self::Single(pane) => pane.contains(column, row).then_some(*pane),
            Self::Split {
                primary, secondary, ..
            } => {
                if primary.contains(column, row) {
                    Some(*primary)
                } else if secondary.contains(column, row) {
                    Some(*secondary)
                } else {
                    None
                }
            }
        }
    }

    /// Returns the pane rendering `kind`, if that view is currently on screen.
    #[must_use]
    pub fn pane_for(&self, kind: ViewKind) -> Option<PaneLayout> {
        match self {
            Self::Single(pane) => (pane.kind == kind).then_some(*pane),
            Self::Split {
                primary, secondary, ..
            } => {
                if primary.kind == kind {
                    Some(*primary)
                } else if secondary.kind == kind {
                    Some(*secondary)
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_kind_action_name_bijective_mapping() {
        for kind in ViewKind::ALL {
            let name = kind.action_name();
            assert_eq!(ViewKind::from_action_name(name), Some(kind));
        }
        assert_eq!(ViewKind::from_action_name("view-nonexistent"), None);
    }

    #[test]
    fn test_pane_layout_and_view_layout_hit_testing() {
        let left = PaneLayout {
            kind: ViewKind::Main,
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        };
        let right = PaneLayout {
            kind: ViewKind::Diff,
            x: 41, // column 40 is separator
            y: 0,
            width: 39,
            height: 24,
        };
        assert_eq!(left.visible_height(), 22);
        assert!(left.contains(0, 0));
        assert!(left.contains(39, 23));
        assert!(!left.contains(40, 0));

        let split = ViewLayout::Split {
            primary: left,
            secondary: right,
            vertical: true,
        };

        assert_eq!(split.pane_at(10, 5), Some(left));
        assert_eq!(split.pane_at(50, 5), Some(right));
        // Separator column 40 belongs to neither pane
        assert_eq!(split.pane_at(40, 5), None);

        assert_eq!(split.pane_for(ViewKind::Main), Some(left));
        assert_eq!(split.pane_for(ViewKind::Diff), Some(right));
        assert_eq!(split.pane_for(ViewKind::Status), None);
    }
}
