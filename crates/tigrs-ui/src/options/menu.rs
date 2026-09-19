// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive Options & Config Panel (`o` / `:options`) state, categories, and menu entries.

use super::{OptionId, ViewOptions};
use unicode_width::UnicodeWidthStr;

/// Category tab grouping options in the interactive Options & Config Panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OptionCategory {
    /// UI theme, syntax theme, split orientation, box graphics, mouse, and read-only lock (`1:Theme & UI`).
    Appearance,
    /// Diff layout, header style, sticky header, word-diff, moved code, whitespace, and line numbers (`2:Diff & Code`).
    Diff,
    /// Revision graph, commit order, ref badges, spotlight, push status, and commit columns (`3:History & Graph`).
    History,
    /// Uncommitted changes, untracked files, path/revision filters, and file columns (`4:Files & Filters`).
    Files,
}

impl OptionCategory {
    /// Returns the short display label for this category tab.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Theme & UI",
            Self::Diff => "Diff & Code",
            Self::History => "History & Graph",
            Self::Files => "Files & Filters",
        }
    }
}

/// Activation effect of an entry in the interactive option toggle menu.
///
/// Menu entries are resolved to a strongly-typed action at compile time rather
/// than to an option name string that has to be re-parsed on every activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuAction {
    /// Cycle the given option through its next value.
    Toggle(OptionId),
    /// Cycle the given option backward through its previous value.
    Prev(OptionId),
    /// Adjust the number of diff context lines by the given delta.
    DiffContext(i8),
    /// Reset a single option to its built-in default value.
    Reset(OptionId),
    /// Reset all options to their built-in default values.
    ResetAll,
}

/// Descriptor for one item in the interactive option toggle menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToggleMenuItem {
    /// Single-character hotkey associated with this option.
    pub hotkey: char,
    /// Descriptive human-readable label shown in the menu.
    pub label: &'static str,
    /// Action performed when this entry is activated.
    pub action: MenuAction,
}

/// Compile-time helper that builds a [`ToggleMenuItem`] directly from an [`OptionId`]'s
/// [`OptionDescriptor`] in [`OPTIONS_REGISTRY`], eliminating duplicated hotkey/label tables.
const fn menu_toggle(id: OptionId) -> ToggleMenuItem {
    let desc = id.descriptor();
    let Some(hotkey) = desc.menu_hotkey else {
        panic!("OptionId has no menu_hotkey in OPTIONS_REGISTRY");
    };
    let Some(label) = desc.menu_label else {
        panic!("OptionId has no menu_label in OPTIONS_REGISTRY");
    };
    ToggleMenuItem {
        hotkey,
        label,
        action: MenuAction::Toggle(id),
    }
}

/// All 44 interactive options in the Options & Config Panel, ordered logically by category:
/// 1. `Appearance` (`Theme & UI`, 7 items)
/// 2. `Diff` (`Diff & Code`, 13 items)
/// 3. `History` (`History & Graph`, 17 items)
/// 4. `Files` (`Files & Filters`, 7 items)
pub static TOGGLE_MENU_ITEMS: &[ToggleMenuItem] = &[
    // 1. Theme & UI (7)
    menu_toggle(OptionId::UiTheme),
    menu_toggle(OptionId::SyntaxTheme),
    menu_toggle(OptionId::SyntaxHighlighting),
    menu_toggle(OptionId::VerticalSplit),
    menu_toggle(OptionId::LineGraphics),
    menu_toggle(OptionId::Mouse),
    menu_toggle(OptionId::ReadOnly),
    // 2. Diff & Code (13)
    menu_toggle(OptionId::DiffLayout),
    menu_toggle(OptionId::DiffPresentation),
    menu_toggle(OptionId::DiffStickyHeader),
    menu_toggle(OptionId::WordDiff),
    menu_toggle(OptionId::ColorMoved),
    ToggleMenuItem {
        hotkey: 'c',
        label: "diff context",
        action: MenuAction::DiffContext(1),
    },
    menu_toggle(OptionId::IgnoreSpace),
    menu_toggle(OptionId::WrapLines),
    menu_toggle(OptionId::LineNumber),
    menu_toggle(OptionId::DiffCollapseGenerated),
    menu_toggle(OptionId::DiffHints),
    menu_toggle(OptionId::WordDiffPairing),
    menu_toggle(OptionId::DiffIndicator),
    // 3. History & Graph (17)
    menu_toggle(OptionId::CommitTitleGraph),
    menu_toggle(OptionId::CommitOrder),
    menu_toggle(OptionId::CommitTitleRefs),
    menu_toggle(OptionId::Date),
    menu_toggle(OptionId::Author),
    menu_toggle(OptionId::Id),
    menu_toggle(OptionId::MainAuthorColor),
    menu_toggle(OptionId::MainSpotlight),
    menu_toggle(OptionId::MainSpotlightDimOthers),
    menu_toggle(OptionId::MainPushStatus),
    menu_toggle(OptionId::MainDateHeat),
    menu_toggle(OptionId::MainUniquePrefix),
    menu_toggle(OptionId::MainSubjectRules),
    menu_toggle(OptionId::MainDimMerges),
    menu_toggle(OptionId::MainDimUnreachable),
    menu_toggle(OptionId::CommitTitleOverflow),
    menu_toggle(OptionId::Committer),
    // 4. Files & Filters (7)
    menu_toggle(OptionId::ShowChanges),
    menu_toggle(OptionId::ShowUntracked),
    menu_toggle(OptionId::StatusShowUntrackedDirs),
    menu_toggle(OptionId::FileFilter),
    menu_toggle(OptionId::RevFilter),
    menu_toggle(OptionId::FileName),
    menu_toggle(OptionId::FileSize),
];

/// State of an active interactive Options & Config Panel (`o` / `:options`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionMenuState {
    /// Index of currently highlighted item in `TOGGLE_MENU_ITEMS`.
    pub selected: usize,
    /// Optional category filter tab (`None` = show all options).
    pub category: Option<OptionCategory>,
    /// Optional inline path editor buffer when the user triggers Save-As (`P`).
    pub save_as_input: Option<String>,
    /// Whether saving persists only non-default / modified keys (`true`) or all keys (`false`).
    pub minimal_save: bool,
    /// Live search query entered via `/` inside the menu (`""` when unfiltered).
    pub filter_query: String,
    /// Whether keyboard input is currently typing into `filter_query` (`/`).
    pub filter_active: bool,
}

impl Default for OptionMenuState {
    fn default() -> Self {
        Self::new()
    }
}

impl OptionMenuState {
    /// Creates a new option menu initialized in the `All` view at the first item.
    #[must_use]
    pub fn new() -> Self {
        Self {
            selected: 0,
            category: None,
            save_as_input: None,
            minimal_save: true,
            filter_query: String::new(),
            filter_active: false,
        }
    }

    /// Creates a context-aware option menu pre-selected on the category tab matching `view`.
    #[must_use]
    pub fn for_view(view: Option<crate::app::layout::ViewKind>) -> Self {
        use crate::app::layout::ViewKind;
        let category = match view {
            Some(ViewKind::Diff | ViewKind::Blob | ViewKind::Blame) => Some(OptionCategory::Diff),
            Some(
                ViewKind::Main
                | ViewKind::Log
                | ViewKind::Refs
                | ViewKind::Reflog
                | ViewKind::Stash,
            ) => Some(OptionCategory::History),
            Some(ViewKind::Status | ViewKind::Tree | ViewKind::Grep) => Some(OptionCategory::Files),
            Some(ViewKind::Help | ViewKind::Pager) => Some(OptionCategory::Appearance),
            None => None,
        };
        let mut state = Self {
            selected: 0,
            category,
            save_as_input: None,
            minimal_save: true,
            filter_query: String::new(),
            filter_active: false,
        };
        state.sync_selection_to_visible();
        state
    }

    /// Returns the category of a `ToggleMenuItem` at `idx`.
    #[must_use]
    pub fn item_category(idx: usize) -> OptionCategory {
        match TOGGLE_MENU_ITEMS.get(idx).map(|it| it.action) {
            Some(MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id)) => {
                id.descriptor().category
            }
            Some(MenuAction::DiffContext(_)) => OptionCategory::Diff,
            _ => OptionCategory::Files,
        }
    }

    /// Returns the optional in-view keyboard shortcut badge (e.g. `"[v]"`, `"[w]"`) for a menu action.
    #[must_use]
    pub const fn item_view_shortcut(action: MenuAction) -> Option<&'static str> {
        match action {
            MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id) => match id {
                OptionId::VerticalSplit => Some("[|]"),
                OptionId::LineGraphics => Some("[~]"),
                OptionId::DiffLayout => Some("[v]"),
                OptionId::DiffPresentation | OptionId::CommitTitleRefs => Some("[F]"),
                OptionId::WordDiff => Some("[w]"),
                OptionId::ColorMoved => Some("[M]"),
                OptionId::IgnoreSpace => Some("[W]"),
                OptionId::WrapLines => Some("[zW]"),
                OptionId::LineNumber => Some("[.]"),
                OptionId::CommitTitleGraph => Some("[G]"),
                OptionId::CommitOrder => Some("[I]"),
                OptionId::MainAuthorColor => Some("[a]"),
                OptionId::MainSpotlight => Some("[*]"),
                OptionId::MainPushStatus => Some("[U]"),
                OptionId::Date => Some("[D]"),
                OptionId::Author => Some("[A]"),
                OptionId::Id => Some("[X]"),
                OptionId::ShowChanges => Some("[C]"),
                OptionId::FileFilter => Some("[%]"),
                OptionId::RevFilter => Some("[^]"),
                OptionId::FileName => Some("[#]"),
                _ => None,
            },
            MenuAction::DiffContext(_) => Some("[/]"),
            MenuAction::ResetAll => None,
        }
    }

    fn item_matches_query(idx: usize, query_lower: &str) -> bool {
        let Some(item) = TOGGLE_MENU_ITEMS.get(idx) else {
            return false;
        };
        let title = Self::item_human_title(item.action).to_ascii_lowercase();
        if title.contains(query_lower) || item.label.to_ascii_lowercase().contains(query_lower) {
            return true;
        }
        match item.action {
            MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id) => {
                let d = id.descriptor();
                d.canonical_name.to_ascii_lowercase().contains(query_lower)
                    || d.description.to_ascii_lowercase().contains(query_lower)
                    || d.aliases
                        .iter()
                        .any(|a| a.to_ascii_lowercase().contains(query_lower))
            }
            MenuAction::DiffContext(_) => {
                "diff-context".contains(query_lower)
                    || "surrounding unchanged context lines".contains(query_lower)
            }
            MenuAction::ResetAll => "reset-all".contains(query_lower),
        }
    }

    /// Returns the indices in `TOGGLE_MENU_ITEMS` visible under the active `/` search query or category filter.
    #[must_use]
    pub fn visible_indices(&self) -> Vec<usize> {
        let q = self.filter_query.trim();
        if !q.is_empty() {
            let q_lower = q.to_ascii_lowercase();
            return (0..TOGGLE_MENU_ITEMS.len())
                .filter(|&i| Self::item_matches_query(i, &q_lower))
                .collect();
        }
        (0..TOGGLE_MENU_ITEMS.len())
            .filter(|&i| match self.category {
                None => true,
                Some(cat) => Self::item_category(i) == cat,
            })
            .collect()
    }

    /// Ensures `self.selected` points to a currently visible item if any exist.
    pub fn sync_selection_to_visible(&mut self) {
        let visible = self.visible_indices();
        if !visible.contains(&self.selected)
            && let Some(&first) = visible.first()
        {
            self.selected = first;
        }
    }

    /// Switches to a category tab (`1` = Theme & UI, `2` = Diff & Code, `3` = History & Graph, `4` = Files & Filters, `5` = All).
    pub fn set_category_tab(&mut self, tab: u8) {
        self.filter_active = false;
        self.filter_query.clear();
        self.category = match tab {
            1 => Some(OptionCategory::Appearance),
            2 => Some(OptionCategory::Diff),
            3 => Some(OptionCategory::History),
            4 => Some(OptionCategory::Files),
            _ => None,
        };
        self.sync_selection_to_visible();
    }

    /// Cycles forward to the next category tab (`Tab`).
    pub fn next_category_tab(&mut self) {
        let next_tab = match self.category {
            Some(OptionCategory::Appearance) => 2,
            Some(OptionCategory::Diff) => 3,
            Some(OptionCategory::History) => 4,
            Some(OptionCategory::Files) => 5,
            None => 1,
        };
        self.set_category_tab(next_tab);
    }

    /// Cycles backward to the previous category tab (`Shift-Tab` / `BackTab`).
    pub fn prev_category_tab(&mut self) {
        let prev_tab = match self.category {
            Some(OptionCategory::Appearance) => 5,
            Some(OptionCategory::Diff) => 1,
            Some(OptionCategory::History) => 2,
            Some(OptionCategory::Files) => 3,
            None => 4,
        };
        self.set_category_tab(prev_tab);
    }

    /// Jumps to the first visible option in the current tab or filter (`g` / `Home`).
    pub fn first(&mut self) {
        if let Some(&first) = self.visible_indices().first() {
            self.selected = first;
        }
    }

    /// Jumps to the last visible option in the current tab or filter (`G` / `End`).
    pub fn last(&mut self) {
        if let Some(&last) = self.visible_indices().last() {
            self.selected = last;
        }
    }

    /// Moves selection up by `step` visible items, clamping at the top (`PgUp` / `Ctrl-U`).
    pub fn page_up(&mut self, step: usize) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0);
        self.selected = visible[pos.saturating_sub(step.max(1))];
    }

    /// Moves selection down by `step` visible items, clamping at the bottom (`PgDn` / `Ctrl-D`).
    pub fn page_down(&mut self, step: usize) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0);
        let next_pos = (pos + step.max(1)).min(visible.len() - 1);
        self.selected = visible[next_pos];
    }

    /// Moves selection to the previous visible menu item, wrapping around.
    pub fn prev(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
            let prev_pos = if pos == 0 { visible.len() - 1 } else { pos - 1 };
            self.selected = visible[prev_pos];
        } else {
            self.selected = visible[0];
        }
    }

    /// Moves selection to the next visible menu item, wrapping around.
    pub fn next(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
            let next_pos = (pos + 1) % visible.len();
            self.selected = visible[next_pos];
        } else {
            self.selected = visible[0];
        }
    }

    /// Returns the reverse (`Prev`) action for the currently highlighted item.
    #[must_use]
    pub fn current_prev_action(&self) -> MenuAction {
        match self.current_item().action {
            MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id) => {
                MenuAction::Prev(id)
            }
            MenuAction::DiffContext(_) => MenuAction::DiffContext(-1),
            MenuAction::ResetAll => MenuAction::ResetAll,
        }
    }

    /// Returns the `Reset` action for the currently highlighted item.
    #[must_use]
    pub fn current_reset_action(&self) -> Option<MenuAction> {
        match self.current_item().action {
            MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id) => {
                Some(MenuAction::Reset(id))
            }
            MenuAction::DiffContext(_) | MenuAction::ResetAll => None,
        }
    }

    /// Finds a menu item matching `hotkey` (case-sensitive).
    #[must_use]
    pub fn find_hotkey(hotkey: char) -> Option<usize> {
        TOGGLE_MENU_ITEMS.iter().position(|it| it.hotkey == hotkey)
    }

    /// Returns the currently selected menu item.
    #[must_use]
    pub fn current_item(&self) -> &'static ToggleMenuItem {
        &TOGGLE_MENU_ITEMS[self.selected.min(TOGGLE_MENU_ITEMS.len().saturating_sub(1))]
    }

    /// Formats the compact 1-line fallback status line for tiny terminals (`height < 8 || width < 30`):
    /// `"Toggle option <label> [<hotkey>]                     (X of Y)"`.
    #[must_use]
    pub fn render_line(&self, width: usize) -> String {
        if let Some(ref input) = self.save_as_input {
            let prompt_str = format!("Save config TOML to path: {input}");
            let w = UnicodeWidthStr::width(prompt_str.as_str());
            if w >= width {
                return prompt_str;
            }
            return format!("{prompt_str}{}", " ".repeat(width - w));
        }

        let item = self.current_item();
        let left = format!("Toggle option {} [{}]", item.label, item.hotkey);
        let right = format!("({} of {})", self.selected + 1, TOGGLE_MENU_ITEMS.len());

        let left_w = UnicodeWidthStr::width(left.as_str());
        let right_w = UnicodeWidthStr::width(right.as_str());

        if left_w + right_w >= width {
            return left;
        }

        let padding = width.saturating_sub(left_w + right_w);
        format!("{left}{}{right}", " ".repeat(padding))
    }

    /// Returns a clear, Title-Case human label for a menu action.
    #[must_use]
    pub const fn item_human_title(action: MenuAction) -> &'static str {
        match action {
            MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id) => match id {
                OptionId::LineNumber => "Line Numbers",
                OptionId::Date => "Commit Date",
                OptionId::Author => "Commit Author",
                OptionId::Committer => "Committer Column",
                OptionId::LineGraphics => "Box & Graph Chars",
                OptionId::CommitTitleGraph => "Revision Graph",
                OptionId::FileName => "File Name Column",
                OptionId::FileSize => "File Size Column",
                OptionId::IgnoreSpace => "Ignore Whitespace",
                OptionId::WordDiff => "Inline Word Diff",
                OptionId::CommitOrder => "Commit Sort Order",
                OptionId::CommitTitleRefs => "Branch & Tag Badges",
                OptionId::ShowChanges => "Uncommitted Changes",
                OptionId::ShowUntracked => "Untracked Files",
                OptionId::Id => "Commit SHA Column",
                OptionId::FileFilter => "Pathspec Filter",
                OptionId::RevFilter => "Revision Filter",
                OptionId::CommitTitleOverflow => "Title Overflow (>50c)",
                OptionId::StatusShowUntrackedDirs => "Untracked Dirs",
                OptionId::VerticalSplit => "Split Orientation",
                OptionId::DiffPresentation => "Diff Header Style",
                OptionId::DiffLayout => "Diff Layout",
                OptionId::DiffIndicator => "Diff +/- Signs",
                OptionId::WordDiffPairing => "Word Diff Pairing",
                OptionId::Mouse => "Mouse Support",
                OptionId::SyntaxHighlighting => "Syntax Highlighting",
                OptionId::SyntaxTheme => "Code Syntax Theme",
                OptionId::ReadOnly => "Read-Only Lock",
                OptionId::UiTheme => "UI Color Theme",
                OptionId::WrapLines => "Soft Wrap Lines",
                OptionId::ColorMoved => "Moved Code Detection",
                OptionId::DiffStickyHeader => "Sticky File Header",
                OptionId::DiffHints => "Banner Key Hints",
                OptionId::DiffCollapseGenerated => "Collapse Generated Files",
                OptionId::MainAuthorColor => "Author Coloring",
                OptionId::MainSpotlight => "Commit Spotlight",
                OptionId::MainSpotlightDimOthers => "Spotlight Dim Others",
                OptionId::MainDimUnreachable => "Dim Unreachable Commits",
                OptionId::MainPushStatus => "Push Status Indicator",
                OptionId::MainDateHeat => "Date Age Heatmap",
                OptionId::MainSubjectRules => "Commit Subject Rules",
                OptionId::MainUniquePrefix => "Unique SHA Prefix",
                OptionId::MainDimMerges => "Dim Merge Subjects",
            },
            MenuAction::DiffContext(_) => "Context Lines",
            MenuAction::ResetAll => "Reset All Options",
        }
    }

    /// Returns the count of menu items belonging to `cat` (`None` = all items).
    #[must_use]
    pub fn category_count(cat: Option<OptionCategory>) -> usize {
        match cat {
            None => TOGGLE_MENU_ITEMS.len(),
            Some(c) => (0..TOGGLE_MENU_ITEMS.len())
                .filter(|&i| Self::item_category(i) == c)
                .count(),
        }
    }

    /// Renders the multi-row interactive **Options & Config Panel** drawer lines,
    /// each formatted to `width` terminal columns with **2 chrome rows** (1 top tab/status bar
    /// and 1 bottom combined inspector + key-hint bar) and clean 3-part option rows.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn render_panel_lines(
        &self,
        current: &ViewOptions,
        saved_baseline: &ViewOptions,
        default_baseline: &ViewOptions,
        target_path: Option<&std::path::Path>,
        width: usize,
        max_rows: usize,
    ) -> Vec<String> {
        if width < 30 || max_rows < 4 {
            return vec![self.render_line(width)];
        }

        let pad_or_trunc = |s: &str, w: usize| -> String {
            let truncated = tigrs_core::ansi::truncate_display_width(s, w);
            let cur_w = UnicodeWidthStr::width(truncated);
            if cur_w < w {
                format!("{truncated}{}", " ".repeat(w - cur_w))
            } else {
                truncated.to_string()
            }
        };

        let visible = self.visible_indices();
        let chrome_rows = 2usize;
        let list_rows = max_rows
            .saturating_sub(chrome_rows)
            .max(1)
            .min(visible.len().max(1));
        let total_rows = list_rows + chrome_rows;
        let mut out = Vec::with_capacity(total_rows);

        let palette = current.ui_palette();
        let header_sgr = crate::ui_theme::UiPalette::sgr_for_style(palette.drawer_header);
        let row_selected_sgr =
            crate::ui_theme::UiPalette::sgr_for_style(palette.drawer_row_selected);
        let row_normal_sgr = crate::ui_theme::UiPalette::sgr_for_style(palette.drawer_row_normal);
        let footer_sgr = crate::ui_theme::UiPalette::sgr_for_style(palette.drawer_footer);

        // 1. Top Header Bar: Category Tabs + Save Status / Filter Badge
        let tabs = [
            (Some(OptionCategory::Appearance), "1:Theme & UI"),
            (Some(OptionCategory::Diff), "2:Diff & Code"),
            (Some(OptionCategory::History), "3:History & Graph"),
            (Some(OptionCategory::Files), "4:Files & Filters"),
            (None, "5:All"),
        ];
        let tab_active_sgr = if palette.id == tigrs_core::config_enums::UiThemeId::Default {
            "\x1b[0;1;30;46m".to_string()
        } else {
            format!(
                "\x1b[0m{}",
                crate::ui_theme::UiPalette::sgr_for_style(palette.drawer_row_selected)
            )
        };
        let reset_header = format!("\x1b[0m{header_sgr}");
        let is_filtered = !self.filter_query.trim().is_empty() || self.filter_active;
        let mut tab_plain = String::from(" Options & Config");
        let mut tab_styled = String::from(" Options & Config");
        for (cat, label) in tabs {
            let count = Self::category_count(cat);
            if !is_filtered && self.category == cat {
                let _ = std::fmt::Write::write_fmt(
                    &mut tab_plain,
                    format_args!("  {label} ({count}) "),
                );
                let _ = std::fmt::Write::write_fmt(
                    &mut tab_styled,
                    format_args!(" {tab_active_sgr} {label} ({count}) {reset_header}"),
                );
            } else {
                let _ = std::fmt::Write::write_fmt(&mut tab_plain, format_args!(" {label}"));
                let _ = std::fmt::Write::write_fmt(&mut tab_styled, format_args!(" {label}"));
            }
        }
        if is_filtered {
            let cursor = if self.filter_active { "█" } else { "" };
            let _ = std::fmt::Write::write_fmt(
                &mut tab_plain,
                format_args!(
                    "  /Filter: {}{cursor} ({}) ",
                    self.filter_query,
                    visible.len()
                ),
            );
            let _ = std::fmt::Write::write_fmt(
                &mut tab_styled,
                format_args!(
                    " {tab_active_sgr} /Filter: {}{cursor} ({}) {reset_header}",
                    self.filter_query,
                    visible.len()
                ),
            );
        } else {
            tab_plain.push(' ');
            tab_styled.push(' ');
        }

        let unsaved_count = current.count_modified_from(saved_baseline);
        let default_count = current.count_modified_from(default_baseline);
        let mode_tag = if self.minimal_save { "Minimal" } else { "Full" };
        let right_badge = if unsaved_count > 0 {
            format!(" ● {unsaved_count} unsaved (p:Save, M:{mode_tag}) ")
        } else {
            format!(" ✓ Saved ({default_count} custom, M:{mode_tag}) ")
        };

        let header_left_w = UnicodeWidthStr::width(tab_plain.as_str());
        let header_right_w = UnicodeWidthStr::width(right_badge.as_str());
        let header_line = if header_left_w + header_right_w <= width {
            format!(
                "{header_sgr}{tab_styled}{}{right_badge}\x1b[0m",
                "─".repeat(width - header_left_w - header_right_w)
            )
        } else {
            let short_badge = if unsaved_count > 0 {
                format!(" ● {unsaved_count} unsaved ")
            } else {
                format!(" ✓ Saved ({default_count}) ")
            };
            let short_w = UnicodeWidthStr::width(short_badge.as_str());
            if header_left_w + short_w <= width {
                format!(
                    "{header_sgr}{tab_styled}{}{short_badge}\x1b[0m",
                    "─".repeat(width - header_left_w - short_w)
                )
            } else {
                format!("{header_sgr}{}\x1b[0m", pad_or_trunc(&tab_plain, width))
            }
        };
        out.push(header_line);

        // 2. Clean 3-Part Option Rows (Title + Value Pill + Description + Right-Aligned View Shortcut)
        let setting_col_w: usize = 26;
        let value_col_w: usize = 28;

        if visible.is_empty() {
            let empty_msg = format!(
                "   No options matching '{}' — press Backspace or Esc to clear filter",
                self.filter_query
            );
            out.push(format!(
                "{row_normal_sgr}\x1b[2m{}\x1b[0m",
                pad_or_trunc(&empty_msg, width)
            ));
        } else {
            let sel_pos = visible
                .iter()
                .position(|&idx| idx == self.selected)
                .unwrap_or(0);
            let scroll_top = if sel_pos >= list_rows {
                (sel_pos + 1 - list_rows).min(visible.len().saturating_sub(list_rows))
            } else {
                0
            };

            let total_vis = visible.len().max(1);
            let thumb_size = ((list_rows * list_rows) / total_vis).clamp(1, list_rows);
            let max_scroll = total_vis.saturating_sub(list_rows);
            let thumb_start = (scroll_top * list_rows.saturating_sub(thumb_size))
                .checked_div(max_scroll)
                .unwrap_or(0);

            for row_i in 0..list_rows {
                let Some(&item_idx) = visible.get(scroll_top + row_i) else {
                    out.push(format!("{row_normal_sgr}{}\x1b[0m", " ".repeat(width)));
                    continue;
                };
                let item = &TOGGLE_MENU_ITEMS[item_idx];
                let is_selected = item_idx == self.selected;
                let is_unsaved = current.option_differs_from(saved_baseline, item.action);
                let is_non_default = current.option_differs_from(default_baseline, item.action);

                let human_title = Self::item_human_title(item.action);
                let desc_str = match item.action {
                    MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id) => {
                        id.descriptor().description
                    }
                    MenuAction::DiffContext(_) => {
                        "Surrounding unchanged context lines per hunk (0..20..full)"
                    }
                    MenuAction::ResetAll => "Reset all options to built-in defaults",
                };
                let shortcut_badge = Self::item_view_shortcut(item.action).unwrap_or("");

                let scroll_ch = if total_vis > list_rows {
                    if row_i >= thumb_start && row_i < thumb_start + thumb_size {
                        '┃'
                    } else {
                        '│'
                    }
                } else {
                    ' '
                };

                let usable_w = width.saturating_sub(1);
                let title_padded = pad_or_trunc(human_title, setting_col_w);
                // Prefix columns: " ▸ ● " (5) + setting_col_w (26) + " " (1) + value_col_w (28) + " " (1) = 61 cols
                let prefix_cols = 5 + setting_col_w + 1 + value_col_w + 1;
                let desc_total_avail = usable_w.saturating_sub(prefix_cols);
                let badge_w = if shortcut_badge.is_empty() || desc_total_avail < 12 {
                    0
                } else {
                    UnicodeWidthStr::width(shortcut_badge) + 1
                }
                .min(desc_total_avail);
                let desc_avail = desc_total_avail.saturating_sub(badge_w);
                let desc_padded = pad_or_trunc(desc_str, desc_avail);
                let badge_suffix = if badge_w > 0 {
                    format!(" {shortcut_badge}")
                } else {
                    String::new()
                };

                if is_selected {
                    let mod_ch = if is_unsaved {
                        '●'
                    } else if is_non_default {
                        '•'
                    } else {
                        ' '
                    };
                    let widget_styled = current.format_option_widget_styled(
                        item.action,
                        &row_selected_sgr,
                        value_col_w,
                    );
                    let tail_plain = format!(" {desc_padded}{badge_suffix}");
                    let tail_padded = pad_or_trunc(&tail_plain, usable_w.saturating_sub(60));
                    out.push(format!(
                        "{row_selected_sgr} ▸ {mod_ch} {title_padded} {widget_styled}{tail_padded}{scroll_ch}\x1b[0m"
                    ));
                } else {
                    let reset_base = format!("\x1b[0m{row_normal_sgr}");
                    let key_sgr = format!("{row_normal_sgr}\x1b[1;36m");
                    let dim_sgr = format!("{row_normal_sgr}\x1b[2m");
                    let warn_sgr = format!("{row_normal_sgr}\x1b[1;33m");
                    let cust_sgr = format!("{row_normal_sgr}\x1b[36m");

                    let lead_marker = if is_unsaved {
                        format!("{warn_sgr}●{reset_base}")
                    } else if is_non_default {
                        format!("{cust_sgr}•{reset_base}")
                    } else {
                        " ".to_string()
                    };

                    let widget_styled = current.format_option_widget_styled(
                        item.action,
                        &row_normal_sgr,
                        value_col_w,
                    );
                    let badge_styled = if badge_w > 0 {
                        format!(" {key_sgr}{shortcut_badge}{reset_base}")
                    } else {
                        String::new()
                    };

                    let row_str = format!(
                        "{row_normal_sgr}   {lead_marker} {title_padded} {widget_styled} {dim_sgr}{desc_padded}{reset_base}{badge_styled}{dim_sgr}{scroll_ch}\x1b[0m"
                    );
                    out.push(row_str);
                }
            }
        }

        // 3. Unified Bottom Bar: Selected Option Command/TOML Inspector + Concise Key Hints
        if let Some(ref input) = self.save_as_input {
            let prompt_line =
                format!(" Save TOML Config As: {input}█   (Enter: Save to file, Esc: Cancel)");
            out.push(format!(
                "\x1b[1;30;43m{}\x1b[0m",
                pad_or_trunc(&prompt_line, width)
            ));
        } else if self.filter_active {
            let filter_line = format!(
                " / Filter options: {}█   ({} match(es) — Enter/↑↓: Select, Esc: Clear)",
                self.filter_query,
                visible.len()
            );
            out.push(format!(
                "{footer_sgr}{}\x1b[0m",
                pad_or_trunc(&filter_line, width)
            ));
        } else {
            let path_display = target_path.map_or_else(
                || "~/.config/tigrs/config.toml".to_string(),
                |p| p.display().to_string(),
            );
            let sel_item = self.current_item();
            let (set_cmd, toml_path) = match sel_item.action {
                MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id) => {
                    let d = id.descriptor();
                    let val = current.option_value_string(id);
                    (
                        format!(":set {} = {}", d.canonical_name, val),
                        format!("[{}].{}", d.toml_section, d.toml_key),
                    )
                }
                MenuAction::DiffContext(_) => {
                    let val = if current.diff_context == ViewOptions::DIFF_CONTEXT_FULL {
                        "full".to_string()
                    } else {
                        current.diff_context.to_string()
                    };
                    (
                        format!(":set diff-context = {val}"),
                        "[view].diff_context".to_string(),
                    )
                }
                MenuAction::ResetAll => (":reset-all".to_string(), String::new()),
            };

            let left_inspector = format!(" ▸ {set_cmd}  (TOML: {toml_path} → {path_display})");
            let right_hints =
                " │ ↑↓:Move  Space/←→:Change  Tab/1-5:Tab  /:Search  p:Save  r:Reset  q:Close ";
            let left_w = UnicodeWidthStr::width(left_inspector.as_str());
            let right_w = UnicodeWidthStr::width(right_hints);
            let footer_plain = if left_w + right_w <= width {
                format!(
                    "{left_inspector}{}{right_hints}",
                    " ".repeat(width - left_w - right_w)
                )
            } else {
                let compact_left = format!(" ▸ {set_cmd} (TOML: {toml_path})");
                let compact_w = UnicodeWidthStr::width(compact_left.as_str());
                if compact_w + right_w <= width {
                    format!(
                        "{compact_left}{}{right_hints}",
                        " ".repeat(width - compact_w - right_w)
                    )
                } else {
                    pad_or_trunc(&format!("{compact_left}{right_hints}"), width)
                }
            };
            out.push(format!("{footer_sgr}{footer_plain}\x1b[0m"));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::layout::ViewKind;
    use crate::options::ViewOptions;

    #[test]
    fn test_option_menu_state_navigation_and_category_filtering() {
        let mut menu = OptionMenuState::new();
        assert_eq!(menu.category, None);
        let all_len = menu.visible_indices().len();
        assert_eq!(
            all_len, 44,
            "All 44 options must be present in TOGGLE_MENU_ITEMS"
        );

        menu.next();
        assert_eq!(menu.selected, 1);
        menu.prev();
        assert_eq!(menu.selected, 0);

        // Jump to last ('G') and first ('g')
        menu.last();
        assert_eq!(menu.selected, 43);
        menu.first();
        assert_eq!(menu.selected, 0);

        // Cycle category tabs forward: None -> 1:Appearance -> 2:Diff -> 3:History -> 4:Files -> 5:All
        menu.next_category_tab();
        assert_eq!(menu.category, Some(OptionCategory::Appearance));
        assert_eq!(menu.visible_indices().len(), 7);

        // Direct category selection (2 = Diff & Code)
        menu.set_category_tab(2);
        assert_eq!(menu.category, Some(OptionCategory::Diff));
        let diff_indices = menu.visible_indices();
        assert_eq!(diff_indices.len(), 13);
        assert!(
            diff_indices
                .iter()
                .any(|&i| TOGGLE_MENU_ITEMS[i].action
                    == MenuAction::Toggle(OptionId::DiffStickyHeader)),
            "Diff category must include diff-sticky-header"
        );
        assert!(
            diff_indices
                .iter()
                .any(|&i| TOGGLE_MENU_ITEMS[i].action == MenuAction::Toggle(OptionId::DiffHints)),
            "Diff category must include diff-hints"
        );
        assert!(
            diff_indices.iter().any(|&i| TOGGLE_MENU_ITEMS[i].action
                == MenuAction::Toggle(OptionId::DiffCollapseGenerated)),
            "Diff category must include diff-collapse-generated"
        );

        // Context-aware opening per active view
        let diff_menu = OptionMenuState::for_view(Some(ViewKind::Diff));
        assert_eq!(diff_menu.category, Some(OptionCategory::Diff));
        let main_menu = OptionMenuState::for_view(Some(ViewKind::Main));
        assert_eq!(main_menu.category, Some(OptionCategory::History));
        let status_menu = OptionMenuState::for_view(Some(ViewKind::Status));
        assert_eq!(status_menu.category, Some(OptionCategory::Files));
        let help_menu = OptionMenuState::for_view(Some(ViewKind::Help));
        assert_eq!(help_menu.category, Some(OptionCategory::Appearance));

        // Inline '/' search filtering across all 44 options
        menu.filter_query = "wrap".to_string();
        menu.sync_selection_to_visible();
        let filtered = menu.visible_indices();
        assert!(
            filtered
                .iter()
                .any(|&i| TOGGLE_MENU_ITEMS[i].action == MenuAction::Toggle(OptionId::WrapLines)),
            "Search filter 'wrap' must match wrap-lines"
        );
    }

    #[test]
    fn test_render_panel_lines_shows_pills_inspector_bar_and_unsaved_state() {
        let mut menu = OptionMenuState::new();
        menu.set_category_tab(2); // 2:Diff & Code

        let baseline = ViewOptions::default();
        let mut current = baseline.clone();
        current.diff_sticky_header = false; // 1 unsaved change vs baseline

        let lines = menu.render_panel_lines(&current, &baseline, &baseline, None, 120, 14);
        let raw = lines.join("\n");
        let mut joined = String::with_capacity(raw.len());
        let mut in_esc = false;
        for ch in raw.chars() {
            if ch == '\x1b' {
                in_esc = true;
            } else if in_esc {
                if ch.is_ascii_alphabetic() {
                    in_esc = false;
                }
            } else {
                joined.push(ch);
            }
        }

        // Verify boolean pills, category header, unsaved count badge, and unified bottom inspector bar
        assert!(
            joined.contains("Options & Config") && joined.contains(" 2:Diff & Code (13) "),
            "Header must show title and active category tab without [] brackets, got:\n{joined}"
        );
        assert!(
            joined.contains(" ● ON  ") && joined.contains(" ○ OFF "),
            "Panel rows must render compact rectangle boolean pills without [] brackets, got:\n{joined}"
        );
        assert!(
            joined.contains("◀  unified  · side-by-side ▶"),
            "Panel rows must render active choice without [] brackets, got:\n{joined}"
        );
        // Verify full-rectangle background highlight SGR sequences are emitted for active tab, active enum, and ON/OFF pills
        assert!(
            raw.contains("\x1b[0;1;30;46m 2:Diff & Code (13) ")
                && raw.contains("\x1b[0;1;30;46m unified ")
                && raw.contains("\x1b[0;1;30;42m ● ON  ")
                && raw.contains("\x1b[0;2;37;40m ○ OFF "),
            "Panel must emit full-rectangle background SGR highlights for selected tab and values"
        );
        assert!(
            joined.contains("TOML: [view].") && joined.contains("● 1 unsaved"),
            "Panel must show TOML key path and unsaved badge, got:\n{joined}"
        );
    }
}
