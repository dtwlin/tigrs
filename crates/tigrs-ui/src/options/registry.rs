// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Declarative option registry (`OptionId`, `OptionDescriptor`, `OPTIONS_REGISTRY`) and `:set` / `:toggle` dispatch.

use super::{
    AuthorFormat, CommitOrder, DateFormat, DiffIndicator, DiffLayout, DiffPresentation,
    GraphDisplay, IgnoreSpace, LineGraphics, MainAuthorColor, MainSpotlight, MemoryProfile,
    MenuAction, OptionCategory, TOGGLE_MENU_ITEMS, UiThemeId, ViewOptions, WordDiffPairing,
};

/// Side effect required by the application state after an option is modified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionEffect {
    /// No view rebuild required; change takes effect immediately on the next render.
    None,
    /// Rebuild uncommitted changes rows in the main view (`show-changes`, `show-untracked`).
    RefreshChanges,
    /// Rebuild structured diff document (`diff-layout`, `diff-presentation`, `diff-context`, etc.).
    RefreshDiff,
    /// Refresh syntax highlighting across code views (`DiffView`, `BlobView`, `BlameView`).
    RefreshSyntax,
}

/// Strongly-typed identifier for toggleable options in `ViewOptions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptionId {
    /// Display line numbers (`line-number`).
    LineNumber,
    /// Commit date display format (`date`).
    Date,
    /// Commit author display format (`author`).
    Author,
    /// Display committer instead of or alongside author (`committer`).
    Committer,
    /// Box-drawing graphics style (`line-graphics`).
    LineGraphics,
    /// Commit DAG graph display mode (`commit-title-graph`).
    CommitTitleGraph,
    /// Display file names in diff/tree/status (`file-name`).
    FileName,
    /// Display file sizes in tree view (`file-size`).
    FileSize,
    /// Ignore whitespace differences in diffs (`ignore-space`).
    IgnoreSpace,
    /// Word-level emphasis within modified diff lines (`word-diff`).
    WordDiff,
    /// Commit sorting order in log/main view (`commit-order`).
    CommitOrder,
    /// Display branch heads and tag ref labels (`commit-title-refs`).
    CommitTitleRefs,
    /// Display local uncommitted changes in main view (`show-changes`).
    ShowChanges,
    /// Include untracked files among local changes in main view (`show-untracked`).
    ShowUntracked,
    /// Display commit SHA IDs in main view (`id`).
    Id,
    /// Filter diff view to single file (`file-filter`).
    FileFilter,
    /// Filter revisions in main view (`rev-filter`).
    RevFilter,
    /// Highlight commit title overflow past column threshold (`commit-title-overflow`).
    CommitTitleOverflow,
    /// Show untracked directories as single entries in status view (`status-show-untracked-dirs`).
    StatusShowUntrackedDirs,
    /// Vertical split mode for dual views (`vertical-split`).
    VerticalSplit,
    /// Diff presentation style: banner, fancy, or classic (`diff-presentation`).
    DiffPresentation,
    /// Diff layout mode: unified or side-by-side (`diff-layout`).
    DiffLayout,
    /// Diff indicator (+/-) column presentation mode (`diff-indicator`).
    DiffIndicator,
    /// Word diff line pairing algorithm (`word-diff-pairing`).
    WordDiffPairing,
    /// Mouse tracking support (`mouse`).
    Mouse,
    /// Code syntax highlighting in diff, blob, and blame views (`syntax-highlighting`).
    SyntaxHighlighting,
    /// Active syntax highlighting color theme (`syntax-theme`).
    SyntaxTheme,
    /// Read-only repository protection mode (`read-only`).
    ReadOnly,
    /// Active UI color theme across all views and chrome (`ui-theme`).
    UiTheme,
    /// Soft line wrapping for long code lines (`wrap-lines`).
    WrapLines,
    /// Highlight moved/relocated code blocks in diffs (`color-moved`).
    ColorMoved,
    /// Pin adaptive sticky file and function header in diff view (`diff-sticky-header`).
    DiffStickyHeader,
    /// Contextual action hint chips in diff file banners (`diff-hints`).
    DiffHints,
    /// Automatically collapse files marked `linguist-generated` in `.gitattributes` (`diff-collapse-generated`).
    DiffCollapseGenerated,
    /// Author column coloring mode in main view (`main-author-color`).
    MainAuthorColor,
    /// Interactive same-author / ancestry spotlight in main view (`main-spotlight`).
    MainSpotlight,
    /// Dim non-matching rows when a spotlight is active (`main-spotlight-dim-others`).
    MainSpotlightDimOthers,
    /// Dim commits not reachable from `HEAD` (`main-dim-unreachable`).
    MainDimUnreachable,
    /// Color commit SHAs by push/upstream status (`main-push-status`).
    MainPushStatus,
    /// Apply 6-bucket age heatmap gradient to Date column (`main-date-heat`).
    MainDateHeat,
    /// Apply semantic subject highlighting rules (`main-subject-rules`).
    MainSubjectRules,
    /// Highlight shortest unique SHA prefix (`main-unique-prefix`).
    MainUniquePrefix,
    /// Dim merge commit subject boilerplate (`main-dim-merges`).
    MainDimMerges,
}

impl OptionId {
    /// Returns the canonical action name for toggling this option.
    #[must_use]
    pub const fn action_name(self) -> &'static str {
        match self {
            Self::LineNumber => "toggle-lineno",
            Self::Date => "toggle-date",
            Self::Author => "toggle-author",
            Self::Committer => "toggle-committer",
            Self::LineGraphics => "toggle-graphic",
            Self::CommitTitleGraph => "toggle-rev-graph",
            Self::FileName => "toggle-filename",
            Self::FileSize => "toggle-file-size",
            Self::IgnoreSpace => "toggle-ignore-space",
            Self::WordDiff => "toggle-word-diff",
            Self::CommitOrder => "toggle-commit-order",
            Self::CommitTitleRefs => "toggle-refs",
            Self::ShowChanges => "toggle-show-changes",
            Self::ShowUntracked => "toggle-show-untracked",
            Self::Id => "toggle-id",
            Self::FileFilter => "toggle-file-filter",
            Self::RevFilter => "toggle-rev-filter",
            Self::CommitTitleOverflow => "toggle-title-overflow",
            Self::StatusShowUntrackedDirs => "toggle-untracked-dirs",
            Self::VerticalSplit => "toggle-vertical-split",
            Self::DiffPresentation => "toggle-diff-presentation",
            Self::DiffLayout => "toggle-diff-layout",
            Self::DiffIndicator => "toggle-diff-indicator",
            Self::WordDiffPairing => "toggle-word-diff-pairing",
            Self::Mouse => "toggle-mouse",
            Self::SyntaxHighlighting => "toggle-syntax-highlighting",
            Self::SyntaxTheme => "cycle-syntax-theme",
            Self::ReadOnly => "toggle-read-only",
            Self::UiTheme => "cycle-ui-theme",
            Self::WrapLines => "toggle-wrap-lines",
            Self::ColorMoved => "toggle-color-moved",
            Self::DiffStickyHeader => "toggle-diff-sticky-header",
            Self::DiffHints => "toggle-diff-hints",
            Self::DiffCollapseGenerated => "toggle-diff-collapse-generated",
            Self::MainAuthorColor => "toggle-main-author-color",
            Self::MainSpotlight => "toggle-main-spotlight",
            Self::MainSpotlightDimOthers => "toggle-main-spotlight-dim-others",
            Self::MainDimUnreachable => "toggle-main-dim-unreachable",
            Self::MainPushStatus => "toggle-main-push-status",
            Self::MainDateHeat => "toggle-main-date-heat",
            Self::MainSubjectRules => "toggle-main-subject-rules",
            Self::MainUniquePrefix => "toggle-main-unique-prefix",
            Self::MainDimMerges => "toggle-main-dim-merges",
        }
    }

    /// Returns the static `OptionDescriptor` for this option ID via exhaustive match.
    #[must_use]
    pub const fn descriptor(self) -> &'static OptionDescriptor {
        match self {
            Self::LineNumber => &OPTIONS_REGISTRY[0],
            Self::Date => &OPTIONS_REGISTRY[1],
            Self::Author => &OPTIONS_REGISTRY[2],
            Self::Committer => &OPTIONS_REGISTRY[3],
            Self::LineGraphics => &OPTIONS_REGISTRY[4],
            Self::CommitTitleGraph => &OPTIONS_REGISTRY[5],
            Self::FileName => &OPTIONS_REGISTRY[6],
            Self::FileSize => &OPTIONS_REGISTRY[7],
            Self::IgnoreSpace => &OPTIONS_REGISTRY[8],
            Self::WordDiff => &OPTIONS_REGISTRY[9],
            Self::CommitOrder => &OPTIONS_REGISTRY[10],
            Self::CommitTitleRefs => &OPTIONS_REGISTRY[11],
            Self::ShowChanges => &OPTIONS_REGISTRY[12],
            Self::ShowUntracked => &OPTIONS_REGISTRY[13],
            Self::Id => &OPTIONS_REGISTRY[14],
            Self::FileFilter => &OPTIONS_REGISTRY[15],
            Self::RevFilter => &OPTIONS_REGISTRY[16],
            Self::CommitTitleOverflow => &OPTIONS_REGISTRY[17],
            Self::StatusShowUntrackedDirs => &OPTIONS_REGISTRY[18],
            Self::VerticalSplit => &OPTIONS_REGISTRY[19],
            Self::DiffPresentation => &OPTIONS_REGISTRY[20],
            Self::DiffLayout => &OPTIONS_REGISTRY[21],
            Self::DiffIndicator => &OPTIONS_REGISTRY[22],
            Self::WordDiffPairing => &OPTIONS_REGISTRY[23],
            Self::Mouse => &OPTIONS_REGISTRY[24],
            Self::SyntaxHighlighting => &OPTIONS_REGISTRY[25],
            Self::SyntaxTheme => &OPTIONS_REGISTRY[26],
            Self::ReadOnly => &OPTIONS_REGISTRY[27],
            Self::UiTheme => &OPTIONS_REGISTRY[28],
            Self::WrapLines => &OPTIONS_REGISTRY[29],
            Self::ColorMoved => &OPTIONS_REGISTRY[30],
            Self::DiffStickyHeader => &OPTIONS_REGISTRY[31],
            Self::DiffHints => &OPTIONS_REGISTRY[32],
            Self::DiffCollapseGenerated => &OPTIONS_REGISTRY[33],
            Self::MainAuthorColor => &OPTIONS_REGISTRY[34],
            Self::MainSpotlight => &OPTIONS_REGISTRY[35],
            Self::MainSpotlightDimOthers => &OPTIONS_REGISTRY[36],
            Self::MainDimUnreachable => &OPTIONS_REGISTRY[37],
            Self::MainPushStatus => &OPTIONS_REGISTRY[38],
            Self::MainDateHeat => &OPTIONS_REGISTRY[39],
            Self::MainSubjectRules => &OPTIONS_REGISTRY[40],
            Self::MainUniquePrefix => &OPTIONS_REGISTRY[41],
            Self::MainDimMerges => &OPTIONS_REGISTRY[42],
        }
    }
}

/// Function signature for setting an option by variable name and value string.
pub type OptionSetterFn =
    fn(&mut ViewOptions, &str, &str) -> Result<(String, OptionEffect), String>;

/// Declarative descriptor for a single toggleable/settable option in `OPTIONS_REGISTRY`.
pub struct OptionDescriptor {
    /// Unique identifier for the option.
    pub id: OptionId,
    /// Primary kebab-case option name (e.g. `"line-number"`).
    pub canonical_name: &'static str,
    /// Accepted alias names in `config.toml` and `:set` / `:toggle` commands.
    pub aliases: &'static [&'static str],
    /// Single-character hotkey in the interactive options menu (`O`).
    pub menu_hotkey: Option<char>,
    /// Human-readable label shown in the interactive options menu.
    pub menu_label: Option<&'static str>,
    /// Logical category tab in the Options & Config Panel.
    pub category: OptionCategory,
    /// Target section in `config.toml` (`"general"` or `"view"`).
    pub toml_section: &'static str,
    /// Target `snake_case` key name in `config.toml`.
    pub toml_key: &'static str,
    /// Concise human-readable description shown in the Options & Config Panel.
    pub description: &'static str,
    /// View refresh side effect triggered when this option changes.
    pub effect: OptionEffect,
    /// Whether toggling this option requires invalidating the damage-tracking screen cache.
    pub invalidates_screen: bool,
    /// Mutator function that cycles or toggles the option forward and returns a status message.
    pub toggle: fn(&mut ViewOptions) -> String,
    /// Mutator function that cycles the option backward (`prev`) and returns a status message.
    pub prev: fn(&mut ViewOptions) -> String,
    /// Mutator function that parses and assigns an explicit string value to the option.
    pub set: OptionSetterFn,
}

/// Registry table mapping all toggleable/settable options to their names, aliases, side effects, and mutators.
pub const OPTIONS_REGISTRY: &[OptionDescriptor] = &[
    OptionDescriptor {
        id: OptionId::LineNumber,
        canonical_name: "line-number",
        aliases: &["lineno", "line_number", "number", "nu"],
        menu_hotkey: Some('.'),
        menu_label: Some("line numbers"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "line_number",
        description: "Show source line numbers in diff, blob, and main views",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_line_number,
        prev: ViewOptions::toggle_line_number,
        set: ViewOptions::set_line_number,
    },
    OptionDescriptor {
        id: OptionId::Date,
        canonical_name: "date",
        aliases: &[],
        menu_hotkey: Some('D'),
        menu_label: Some("dates"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "date",
        description: "Commit timestamp format (relative, short, iso, or hidden)",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_date,
        prev: ViewOptions::prev_date,
        set: ViewOptions::set_date,
    },
    OptionDescriptor {
        id: OptionId::Author,
        canonical_name: "author",
        aliases: &[],
        menu_hotkey: Some('A'),
        menu_label: Some("author"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "author",
        description: "Commit author column style (full, abbreviated, email, hidden)",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_author,
        prev: ViewOptions::prev_author,
        set: ViewOptions::set_author,
    },
    OptionDescriptor {
        id: OptionId::Committer,
        canonical_name: "committer",
        aliases: &[],
        menu_hotkey: Some('T'),
        menu_label: Some("committer"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "committer",
        description: "Display committer alongside or instead of author",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_committer,
        prev: ViewOptions::toggle_committer,
        set: ViewOptions::set_committer,
    },
    OptionDescriptor {
        id: OptionId::LineGraphics,
        canonical_name: "line-graphics",
        aliases: &["graphics", "graphic"],
        menu_hotkey: Some('~'),
        menu_label: Some("graphics"),
        category: OptionCategory::Appearance,
        toml_section: "general",
        toml_key: "line_graphics",
        description: "Box-drawing character style (utf-8 or ascii)",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_line_graphics,
        prev: ViewOptions::prev_line_graphics,
        set: ViewOptions::set_line_graphics,
    },
    OptionDescriptor {
        id: OptionId::CommitTitleGraph,
        canonical_name: "commit-title-graph",
        aliases: &["rev-graph"],
        menu_hotkey: Some('g'),
        menu_label: Some("revision graph"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "commit_title_graph",
        description: "Commit DAG revision graph rendering mode (auto, v2, no)",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_commit_title_graph,
        prev: ViewOptions::prev_commit_title_graph,
        set: ViewOptions::set_commit_title_graph,
    },
    OptionDescriptor {
        id: OptionId::FileName,
        canonical_name: "file-name",
        aliases: &["filename"],
        menu_hotkey: Some('#'),
        menu_label: Some("file names"),
        category: OptionCategory::Files,
        toml_section: "view",
        toml_key: "file_name",
        description: "Show file name column in diff, tree, and status views",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_file_name,
        prev: ViewOptions::toggle_file_name,
        set: ViewOptions::set_file_name,
    },
    OptionDescriptor {
        id: OptionId::FileSize,
        canonical_name: "file-size",
        aliases: &[],
        menu_hotkey: Some('*'),
        menu_label: Some("file sizes"),
        category: OptionCategory::Files,
        toml_section: "view",
        toml_key: "file_size",
        description: "Show file size column in tree view",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_file_size,
        prev: ViewOptions::toggle_file_size,
        set: ViewOptions::set_file_size,
    },
    OptionDescriptor {
        id: OptionId::IgnoreSpace,
        canonical_name: "ignore-space",
        aliases: &[],
        menu_hotkey: Some('W'),
        menu_label: Some("space changes"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "ignore_space",
        description: "Ignore whitespace changes in diffs (no, all, some, at-eol)",
        effect: OptionEffect::RefreshDiff,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_ignore_space,
        prev: ViewOptions::prev_ignore_space,
        set: ViewOptions::set_ignore_space,
    },
    OptionDescriptor {
        id: OptionId::WordDiff,
        canonical_name: "word-diff",
        aliases: &["word_diff"],
        menu_hotkey: Some('w'),
        menu_label: Some("word diff"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "word_diff",
        description: "Highlight changed tokens within modified diff lines",
        effect: OptionEffect::RefreshDiff,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_word_diff,
        prev: ViewOptions::toggle_word_diff,
        set: ViewOptions::set_word_diff,
    },
    OptionDescriptor {
        id: OptionId::CommitOrder,
        canonical_name: "commit-order",
        aliases: &[],
        menu_hotkey: Some('l'),
        menu_label: Some("commit order"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "commit_order",
        description: "Commit sorting order in main log view",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_commit_order,
        prev: ViewOptions::prev_commit_order,
        set: ViewOptions::set_commit_order,
    },
    OptionDescriptor {
        id: OptionId::CommitTitleRefs,
        canonical_name: "commit-title-refs",
        aliases: &["refs"],
        menu_hotkey: Some('F'),
        menu_label: Some("reference display"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "commit_title_refs",
        description: "Show branch and tag ref badges on commit titles",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_commit_title_refs,
        prev: ViewOptions::toggle_commit_title_refs,
        set: ViewOptions::set_commit_title_refs,
    },
    OptionDescriptor {
        id: OptionId::ShowChanges,
        canonical_name: "show-changes",
        aliases: &["show_changes"],
        menu_hotkey: Some('C'),
        menu_label: Some("local change display"),
        category: OptionCategory::Files,
        toml_section: "view",
        toml_key: "show_changes",
        description: "Show staged/unstaged working tree changes in main view",
        effect: OptionEffect::RefreshChanges,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_show_changes,
        prev: ViewOptions::toggle_show_changes,
        set: ViewOptions::set_show_changes,
    },
    OptionDescriptor {
        id: OptionId::ShowUntracked,
        canonical_name: "show-untracked",
        aliases: &["show_untracked"],
        menu_hotkey: Some('u'),
        menu_label: Some("untracked change display"),
        category: OptionCategory::Files,
        toml_section: "view",
        toml_key: "show_untracked",
        description: "Show untracked working tree files in main/status views",
        effect: OptionEffect::RefreshChanges,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_show_untracked,
        prev: ViewOptions::toggle_show_untracked,
        set: ViewOptions::set_show_untracked,
    },
    OptionDescriptor {
        id: OptionId::Id,
        canonical_name: "id",
        aliases: &[],
        menu_hotkey: Some('X'),
        menu_label: Some("commit ID display"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "id",
        description: "Show abbreviated commit SHA column in main view",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_id,
        prev: ViewOptions::toggle_id,
        set: ViewOptions::set_id,
    },
    OptionDescriptor {
        id: OptionId::FileFilter,
        canonical_name: "file-filter",
        aliases: &[],
        menu_hotkey: Some('%'),
        menu_label: Some("file filtering"),
        category: OptionCategory::Files,
        toml_section: "view",
        toml_key: "file_filter",
        description: "Filter commits and diffs by active path filter",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_file_filter,
        prev: ViewOptions::toggle_file_filter,
        set: ViewOptions::set_file_filter,
    },
    OptionDescriptor {
        id: OptionId::RevFilter,
        canonical_name: "rev-filter",
        aliases: &[],
        menu_hotkey: Some('^'),
        menu_label: Some("revision filtering"),
        category: OptionCategory::Files,
        toml_section: "view",
        toml_key: "rev_filter",
        description: "Filter main view commits by active revision arguments",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_rev_filter,
        prev: ViewOptions::toggle_rev_filter,
        set: ViewOptions::set_rev_filter,
    },
    OptionDescriptor {
        id: OptionId::CommitTitleOverflow,
        canonical_name: "commit-title-overflow",
        aliases: &["title-overflow"],
        menu_hotkey: Some('$'),
        menu_label: Some("commit title overflow display"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "commit_title_overflow",
        description: "Highlight commit subjects exceeding 50 columns",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_commit_title_overflow,
        prev: ViewOptions::toggle_commit_title_overflow,
        set: ViewOptions::set_commit_title_overflow,
    },
    OptionDescriptor {
        id: OptionId::StatusShowUntrackedDirs,
        canonical_name: "status-show-untracked-dirs",
        aliases: &["untracked-dirs"],
        menu_hotkey: Some('d'),
        menu_label: Some("untracked directory info"),
        category: OptionCategory::Files,
        toml_section: "view",
        toml_key: "status_show_untracked_dirs",
        description: "Show untracked directories as collapsed entries in status view",
        effect: OptionEffect::RefreshChanges,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_status_show_untracked_dirs,
        prev: ViewOptions::toggle_status_show_untracked_dirs,
        set: ViewOptions::set_status_show_untracked_dirs,
    },
    OptionDescriptor {
        id: OptionId::VerticalSplit,
        canonical_name: "vertical-split",
        aliases: &[],
        menu_hotkey: Some('|'),
        menu_label: Some("view split"),
        category: OptionCategory::Appearance,
        toml_section: "general",
        toml_key: "vertical_split",
        description: "Use side-by-side vertical split for dual panes",
        effect: OptionEffect::None,
        invalidates_screen: true,
        toggle: ViewOptions::toggle_vertical_split,
        prev: ViewOptions::toggle_vertical_split,
        set: ViewOptions::set_vertical_split,
    },
    OptionDescriptor {
        id: OptionId::DiffPresentation,
        canonical_name: "diff-presentation",
        aliases: &["presentation"],
        menu_hotkey: Some('y'),
        menu_label: Some("presentation style"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "diff_presentation",
        description: "Diff header and hunk decoration style (banner, fancy, or classic)",
        effect: OptionEffect::RefreshDiff,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_diff_presentation,
        prev: ViewOptions::prev_diff_presentation,
        set: ViewOptions::set_diff_presentation,
    },
    OptionDescriptor {
        id: OptionId::DiffLayout,
        canonical_name: "diff-layout",
        aliases: &[
            "layout",
            "diff-mode",
            "side-by-side",
            "side-to-side",
            "single",
            "unified",
        ],
        menu_hotkey: Some('s'),
        menu_label: Some("diff layout"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "diff_layout",
        description: "Compare old and new files in unified or side-by-side panes",
        effect: OptionEffect::RefreshDiff,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_diff_layout,
        prev: ViewOptions::prev_diff_layout,
        set: ViewOptions::set_diff_layout,
    },
    OptionDescriptor {
        id: OptionId::DiffIndicator,
        canonical_name: "diff-indicator",
        aliases: &["indicator"],
        menu_hotkey: Some('+'),
        menu_label: Some("diff +/- indicators"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "diff_indicator",
        description: "Leading +/- indicator column in diff view (auto, yes, no)",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_diff_indicator,
        prev: ViewOptions::prev_diff_indicator,
        set: ViewOptions::set_diff_indicator,
    },
    OptionDescriptor {
        id: OptionId::WordDiffPairing,
        canonical_name: "word-diff-pairing",
        aliases: &["pairing"],
        menu_hotkey: Some('='),
        menu_label: Some("word diff pairing"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "word_diff_pairing",
        description: "Word diff line pairing algorithm (similarity or positional)",
        effect: OptionEffect::RefreshDiff,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_word_diff_pairing,
        prev: ViewOptions::prev_word_diff_pairing,
        set: ViewOptions::set_word_diff_pairing,
    },
    OptionDescriptor {
        id: OptionId::Mouse,
        canonical_name: "mouse",
        aliases: &[],
        menu_hotkey: Some('m'),
        menu_label: Some("mouse support"),
        category: OptionCategory::Appearance,
        toml_section: "general",
        toml_key: "mouse",
        description: "Enable mouse wheel scrolling and row click selection",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_mouse,
        prev: ViewOptions::toggle_mouse,
        set: ViewOptions::set_mouse,
    },
    OptionDescriptor {
        id: OptionId::SyntaxHighlighting,
        canonical_name: "syntax-highlighting",
        aliases: &["syntax-highlight", "syntax"],
        menu_hotkey: Some('S'),
        menu_label: Some("syntax highlighting"),
        category: OptionCategory::Appearance,
        toml_section: "view",
        toml_key: "syntax_highlighting",
        description: "Enable Tree-sitter / syntect syntax highlighting in code views",
        effect: OptionEffect::RefreshSyntax,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_syntax_highlighting,
        prev: ViewOptions::toggle_syntax_highlighting,
        set: ViewOptions::set_syntax_highlighting,
    },
    OptionDescriptor {
        id: OptionId::SyntaxTheme,
        canonical_name: "syntax-theme",
        aliases: &["theme"],
        menu_hotkey: Some('H'),
        menu_label: Some("syntax theme"),
        category: OptionCategory::Appearance,
        toml_section: "view",
        toml_key: "syntax_theme",
        description: "Color theme for syntax highlighting (Dracula, Nord, etc.)",
        effect: OptionEffect::RefreshSyntax,
        invalidates_screen: false,
        toggle: ViewOptions::cycle_syntax_theme,
        prev: ViewOptions::prev_syntax_theme,
        set: ViewOptions::set_syntax_theme,
    },
    OptionDescriptor {
        id: OptionId::ReadOnly,
        canonical_name: "read-only",
        aliases: &["readonly", "read_only", "ro"],
        menu_hotkey: Some('!'),
        menu_label: Some("read-only mode"),
        category: OptionCategory::Appearance,
        toml_section: "general",
        toml_key: "read_only",
        description: "Prevent repository state mutations (staging, commits, stash)",
        effect: OptionEffect::None,
        invalidates_screen: true,
        toggle: ViewOptions::toggle_read_only,
        prev: ViewOptions::toggle_read_only,
        set: ViewOptions::set_read_only,
    },
    OptionDescriptor {
        id: OptionId::UiTheme,
        canonical_name: "ui-theme",
        aliases: &["color-theme", "ui_theme", "color_theme", "uitheme"],
        menu_hotkey: Some('t'),
        menu_label: Some("UI color theme"),
        category: OptionCategory::Appearance,
        toml_section: "view",
        toml_key: "ui_theme",
        description: "Full UI color theme (Adaptive, Dark, Light, High-Contrast)",
        effect: OptionEffect::RefreshSyntax,
        invalidates_screen: true,
        toggle: ViewOptions::cycle_ui_theme,
        prev: ViewOptions::prev_ui_theme,
        set: ViewOptions::set_ui_theme,
    },
    OptionDescriptor {
        id: OptionId::WrapLines,
        canonical_name: "wrap-lines",
        aliases: &["wrap_lines", "wrap", "line-wrap"],
        menu_hotkey: Some('L'),
        menu_label: Some("wrap long lines"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "wrap_lines",
        description: "Soft-wrap long code lines in diff view instead of horizontal clipping",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_wrap_lines,
        prev: ViewOptions::toggle_wrap_lines,
        set: ViewOptions::set_wrap_lines,
    },
    OptionDescriptor {
        id: OptionId::ColorMoved,
        canonical_name: "color-moved",
        aliases: &["color_moved", "moved-blocks"],
        menu_hotkey: Some('M'),
        menu_label: Some("color moved blocks"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "color_moved",
        description: "Highlight relocated code blocks with distinct moved-from / moved-to tints",
        effect: OptionEffect::RefreshDiff,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_color_moved,
        prev: ViewOptions::toggle_color_moved,
        set: ViewOptions::set_color_moved,
    },
    OptionDescriptor {
        id: OptionId::DiffStickyHeader,
        canonical_name: "diff-sticky-header",
        aliases: &["diff_sticky_header", "sticky-header", "sticky_header"],
        menu_hotkey: Some('K'),
        menu_label: Some("sticky file header"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "diff_sticky_header",
        description: "Pin an adaptive sticky file and function header when scrolling",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_diff_sticky_header,
        prev: ViewOptions::toggle_diff_sticky_header,
        set: ViewOptions::set_diff_sticky_header,
    },
    OptionDescriptor {
        id: OptionId::DiffHints,
        canonical_name: "diff-hints",
        aliases: &["diff_hints", "banner-hints"],
        menu_hotkey: Some('B'),
        menu_label: Some("diff banner hints"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "diff_hints",
        description: "Contextual keyboard action hint chips in diff file banners (auto, always, never)",
        effect: OptionEffect::RefreshDiff,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_diff_hints,
        prev: ViewOptions::prev_diff_hints,
        set: ViewOptions::set_diff_hints,
    },
    OptionDescriptor {
        id: OptionId::DiffCollapseGenerated,
        canonical_name: "diff-collapse-generated",
        aliases: &["diff_collapse_generated", "collapse-generated"],
        menu_hotkey: Some('G'),
        menu_label: Some("collapse generated files"),
        category: OptionCategory::Diff,
        toml_section: "view",
        toml_key: "diff_collapse_generated",
        description: "Automatically collapse files marked linguist-generated in .gitattributes",
        effect: OptionEffect::RefreshDiff,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_diff_collapse_generated,
        prev: ViewOptions::toggle_diff_collapse_generated,
        set: ViewOptions::set_diff_collapse_generated,
    },
    OptionDescriptor {
        id: OptionId::MainAuthorColor,
        canonical_name: "main-author-color",
        aliases: &["main_author_color", "author-color", "author_color"],
        menu_hotkey: Some('a'),
        menu_label: Some("author color"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "main_author_color",
        description: "Author column color mode in main view (hash, me, off)",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_main_author_color,
        prev: ViewOptions::prev_main_author_color,
        set: ViewOptions::set_main_author_color,
    },
    OptionDescriptor {
        id: OptionId::MainSpotlight,
        canonical_name: "main-spotlight",
        aliases: &["main_spotlight", "spotlight"],
        menu_hotkey: Some('&'),
        menu_label: Some("author/ancestry spotlight"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "main_spotlight",
        description: "Interactive spotlight in main view (off, author, ancestry)",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_main_spotlight,
        prev: ViewOptions::prev_main_spotlight,
        set: ViewOptions::set_main_spotlight,
    },
    OptionDescriptor {
        id: OptionId::MainSpotlightDimOthers,
        canonical_name: "main-spotlight-dim-others",
        aliases: &["main_spotlight_dim_others", "spotlight-dim-others"],
        menu_hotkey: Some('_'),
        menu_label: Some("spotlight dim others"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "main_spotlight_dim_others",
        description: "Dim non-matching rows when author/ancestry spotlight is active",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_main_spotlight_dim_others,
        prev: ViewOptions::toggle_main_spotlight_dim_others,
        set: ViewOptions::set_main_spotlight_dim_others,
    },
    OptionDescriptor {
        id: OptionId::MainDimUnreachable,
        canonical_name: "main-dim-unreachable",
        aliases: &["main_dim_unreachable", "dim-unreachable"],
        menu_hotkey: Some('?'),
        menu_label: Some("dim unreachable"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "main_dim_unreachable",
        description: "Dim commits not reachable from HEAD when browsing --all",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_main_dim_unreachable,
        prev: ViewOptions::toggle_main_dim_unreachable,
        set: ViewOptions::set_main_dim_unreachable,
    },
    OptionDescriptor {
        id: OptionId::MainPushStatus,
        canonical_name: "main-push-status",
        aliases: &["main_push_status", "push-status"],
        menu_hotkey: Some('U'),
        menu_label: Some("push status indicator"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "main_push_status",
        description: "Highlight ahead/unpushed (↑) and unmerged commit SHAs in main view",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_main_push_status,
        prev: ViewOptions::toggle_main_push_status,
        set: ViewOptions::set_main_push_status,
    },
    OptionDescriptor {
        id: OptionId::MainDateHeat,
        canonical_name: "main-date-heat",
        aliases: &["main_date_heat", "date-heat"],
        menu_hotkey: Some('@'),
        menu_label: Some("date heatmap"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "main_date_heat",
        description: "Apply 6-bucket age heatmap gradient to Date column in main view",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_main_date_heat,
        prev: ViewOptions::toggle_main_date_heat,
        set: ViewOptions::set_main_date_heat,
    },
    OptionDescriptor {
        id: OptionId::MainSubjectRules,
        canonical_name: "main-subject-rules",
        aliases: &["main_subject_rules", "subject-rules"],
        menu_hotkey: Some(';'),
        menu_label: Some("subject rules"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "main_subject_rules",
        description: "Highlight fixup!, Revert, WIP, conventional prefixes, and #123 in subjects",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_main_subject_rules,
        prev: ViewOptions::toggle_main_subject_rules,
        set: ViewOptions::set_main_subject_rules,
    },
    OptionDescriptor {
        id: OptionId::MainUniquePrefix,
        canonical_name: "main-unique-prefix",
        aliases: &["main_unique_prefix", "unique-prefix"],
        menu_hotkey: Some('x'),
        menu_label: Some("unique SHA prefix"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "main_unique_prefix",
        description: "Bold shortest unique SHA prefix when id column is shown",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_main_unique_prefix,
        prev: ViewOptions::toggle_main_unique_prefix,
        set: ViewOptions::set_main_unique_prefix,
    },
    OptionDescriptor {
        id: OptionId::MainDimMerges,
        canonical_name: "main-dim-merges",
        aliases: &["main_dim_merges", "dim-merges"],
        menu_hotkey: Some('z'),
        menu_label: Some("dim merge subjects"),
        category: OptionCategory::History,
        toml_section: "view",
        toml_key: "main_dim_merges",
        description: "Dim Merge branch / Merge pull request boilerplate in commit subjects",
        effect: OptionEffect::None,
        invalidates_screen: false,
        toggle: ViewOptions::toggle_main_dim_merges,
        prev: ViewOptions::toggle_main_dim_merges,
        set: ViewOptions::set_main_dim_merges,
    },
];

/// Looks up an [`OptionDescriptor`] in [`OPTIONS_REGISTRY`] by canonical name or alias (case-insensitive).
#[must_use]
pub fn find_descriptor(name: &str) -> Option<&'static OptionDescriptor> {
    OPTIONS_REGISTRY.iter().find(|d| {
        d.canonical_name.eq_ignore_ascii_case(name)
            || d.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    })
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "1" | "on" => Some(true),
        "false" | "no" | "0" | "off" => Some(false),
        _ => None,
    }
}

impl ViewOptions {
    /// Toggles an option by its `OptionId`, returning the status message and required view refresh side effect.
    pub fn toggle_by_id(&mut self, id: OptionId) -> (String, OptionEffect) {
        let desc = id.descriptor();
        let msg = (desc.toggle)(self);
        (msg, desc.effect)
    }

    /// Cycles an option backward by its `OptionId`, returning the status message and required view refresh side effect.
    pub fn prev_by_id(&mut self, id: OptionId) -> (String, OptionEffect) {
        let desc = id.descriptor();
        let msg = (desc.prev)(self);
        (msg, desc.effect)
    }

    /// Resets a single option to its value in `ViewOptions::default()`.
    pub fn reset_by_id(&mut self, id: OptionId) -> (String, OptionEffect) {
        let def = Self::default();
        let val = def.option_value_string(id);
        let desc = id.descriptor();
        let _ = (desc.set)(self, desc.canonical_name, &val);
        (
            format!(
                ":reset {} = {}",
                desc.canonical_name,
                self.option_value_string(id)
            ),
            desc.effect,
        )
    }

    /// Applies an option menu entry activation, returning the status message and
    /// required view refresh side effect.
    pub fn apply_menu_action(&mut self, action: MenuAction) -> (String, OptionEffect) {
        match action {
            MenuAction::Toggle(id) => self.toggle_by_id(id),
            MenuAction::Prev(id) => self.prev_by_id(id),
            MenuAction::DiffContext(delta) => {
                let prev = self.diff_context;
                let msg = self.toggle_diff_context(delta);
                let effect = if self.diff_context == prev {
                    OptionEffect::None
                } else {
                    OptionEffect::RefreshDiff
                };
                (msg, effect)
            }
            MenuAction::Reset(id) => self.reset_by_id(id),
            MenuAction::ResetAll => {
                let preserved_colors = self.colors.clone();
                *self = Self::default();
                self.colors = preserved_colors;
                (
                    "Reset all view options to built-in defaults".to_string(),
                    OptionEffect::RefreshSyntax,
                )
            }
        }
    }

    /// Returns the canonical string value of `id` in `self`.
    #[must_use]
    pub fn option_value_string(&self, id: OptionId) -> String {
        match id {
            OptionId::LineNumber => if self.line_number { "yes" } else { "no" }.to_string(),
            OptionId::Date => self.date_format.as_str().to_string(),
            OptionId::Author => self.author_format.as_str().to_string(),
            OptionId::Committer => if self.committer { "yes" } else { "no" }.to_string(),
            OptionId::LineGraphics => self.line_graphics.as_str().to_string(),
            OptionId::CommitTitleGraph => self.commit_title_graph.as_str().to_string(),
            OptionId::FileName => if self.file_name { "yes" } else { "no" }.to_string(),
            OptionId::FileSize => if self.file_size { "yes" } else { "no" }.to_string(),
            OptionId::IgnoreSpace => self.ignore_space.as_str().to_string(),
            OptionId::WordDiff => if self.word_diff { "yes" } else { "no" }.to_string(),
            OptionId::CommitOrder => self.commit_order.as_str().to_string(),
            OptionId::CommitTitleRefs => {
                if self.commit_title_refs { "yes" } else { "no" }.to_string()
            }
            OptionId::ShowChanges => if self.show_changes { "yes" } else { "no" }.to_string(),
            OptionId::ShowUntracked => if self.show_untracked { "yes" } else { "no" }.to_string(),
            OptionId::Id => if self.commit_id { "yes" } else { "no" }.to_string(),
            OptionId::FileFilter => if self.file_filter { "yes" } else { "no" }.to_string(),
            OptionId::RevFilter => if self.rev_filter { "yes" } else { "no" }.to_string(),
            OptionId::CommitTitleOverflow => self
                .commit_title_overflow
                .map_or_else(|| "no".to_string(), |v| v.to_string()),
            OptionId::StatusShowUntrackedDirs => if self.status_show_untracked_dirs {
                "yes"
            } else {
                "no"
            }
            .to_string(),
            OptionId::VerticalSplit => if self.vertical_split { "yes" } else { "no" }.to_string(),
            OptionId::DiffPresentation => self.diff_presentation.as_str().to_string(),
            OptionId::DiffLayout => self.diff_layout.as_str().to_string(),
            OptionId::DiffIndicator => self.diff_indicator.as_str().to_string(),
            OptionId::WordDiffPairing => self.word_diff_pairing.as_str().to_string(),
            OptionId::Mouse => if self.mouse { "yes" } else { "no" }.to_string(),
            OptionId::SyntaxHighlighting => if self.syntax_highlighting {
                "yes"
            } else {
                "no"
            }
            .to_string(),
            OptionId::SyntaxTheme => self.syntax_theme.clone(),
            OptionId::ReadOnly => if self.read_only { "yes" } else { "no" }.to_string(),
            OptionId::UiTheme => self.ui_theme.as_str().to_string(),
            OptionId::WrapLines => if self.wrap_lines { "yes" } else { "no" }.to_string(),
            OptionId::ColorMoved => if self.color_moved { "yes" } else { "no" }.to_string(),
            OptionId::DiffStickyHeader => {
                if self.diff_sticky_header { "yes" } else { "no" }.to_string()
            }
            OptionId::DiffHints => self.diff_hints.as_str().to_string(),
            OptionId::DiffCollapseGenerated => if self.diff_collapse_generated {
                "yes"
            } else {
                "no"
            }
            .to_string(),
            OptionId::MainAuthorColor => self.main_author_color.as_str().to_string(),
            OptionId::MainSpotlight => self.main_spotlight.as_str().to_string(),
            OptionId::MainSpotlightDimOthers => if self.main_spotlight_dim_others {
                "yes"
            } else {
                "no"
            }
            .to_string(),
            OptionId::MainDimUnreachable => if self.main_dim_unreachable {
                "yes"
            } else {
                "no"
            }
            .to_string(),
            OptionId::MainPushStatus => {
                if self.main_push_status { "yes" } else { "no" }.to_string()
            }
            OptionId::MainDateHeat => if self.main_date_heat { "yes" } else { "no" }.to_string(),
            OptionId::MainSubjectRules => {
                if self.main_subject_rules { "yes" } else { "no" }.to_string()
            }
            OptionId::MainUniquePrefix => {
                if self.main_unique_prefix { "yes" } else { "no" }.to_string()
            }
            OptionId::MainDimMerges => if self.main_dim_merges { "yes" } else { "no" }.to_string(),
        }
    }

    fn author_short_str(&self) -> &'static str {
        match self.author_format {
            AuthorFormat::Abbreviated => "abbrev",
            other => other.as_str(),
        }
    }

    /// Formats the interactive widget pill with full-rectangle background selection highlights
    /// (` ● ON  ` on solid green, ` ○ OFF ` on recessed surface, and ` active ` choice on solid
    /// cyan/accent background), padded to `slot_width` display columns.
    #[must_use]
    pub fn format_option_widget_styled(
        &self,
        action: MenuAction,
        base_sgr: &str,
        slot_width: usize,
    ) -> String {
        let palette = self.ui_palette();
        let on_sgr = if palette.id == UiThemeId::Default {
            "\x1b[0;1;30;42m".to_string()
        } else {
            let fg = palette
                .title_bar_active
                .fg
                .unwrap_or(crate::headless::Color::Black);
            format!(
                "\x1b[0m{}",
                crate::ui_theme::UiPalette::sgr_for_style(crate::diff::DiffStyle {
                    fg: Some(fg),
                    bg: Some(palette.ref_branch_fg),
                    attrs: crate::diff::Attrs::BOLD,
                })
            )
        };
        let off_sgr = if palette.id == UiThemeId::Default {
            "\x1b[0;2;37;40m".to_string()
        } else {
            format!(
                "\x1b[0m{}",
                crate::ui_theme::UiPalette::sgr_for_style(crate::diff::DiffStyle {
                    fg: palette.muted_style.fg,
                    bg: palette.status_bar.bg,
                    attrs: crate::diff::Attrs::DIM,
                })
            )
        };
        let active_sgr = if palette.id == UiThemeId::Default {
            "\x1b[0;1;30;46m".to_string()
        } else {
            format!(
                "\x1b[0m{}",
                crate::ui_theme::UiPalette::sgr_for_style(palette.drawer_footer)
            )
        };
        let dim_sgr = format!("{base_sgr}\x1b[2m");
        let reset_to_base = format!("\x1b[0m{base_sgr}");

        let format_bool_styled = |v: bool| -> (String, usize) {
            if v {
                (format!("{on_sgr} ● ON  {reset_to_base}"), 7)
            } else {
                (format!("{off_sgr} ○ OFF {reset_to_base}"), 7)
            }
        };

        let format_choices_styled = |choices: &[&str], active: &str| -> (String, usize) {
            let plain = {
                let mut p = String::from("◀ ");
                for (i, &c) in choices.iter().enumerate() {
                    if i > 0 {
                        p.push_str(" · ");
                    }
                    if c.eq_ignore_ascii_case(active) {
                        p.push(' ');
                        p.push_str(c);
                        p.push(' ');
                    } else {
                        p.push_str(c);
                    }
                }
                p.push_str(" ▶");
                p
            };
            let plain_w = unicode_width::UnicodeWidthStr::width(plain.as_str());
            if plain_w > slot_width {
                let idx = choices
                    .iter()
                    .position(|&c| c.eq_ignore_ascii_case(active))
                    .unwrap_or(0);
                let idx_str = format!("({}/{})", idx + 1, choices.len());
                let vis_w = 2 + 1 + active.len() + 1 + 1 + idx_str.len() + 2;
                let s = format!(
                    "{dim_sgr}◀ {reset_to_base}{active_sgr} {active} {reset_to_base} {dim_sgr}{idx_str} ▶{reset_to_base}"
                );
                (s, vis_w)
            } else {
                use std::fmt::Write as _;
                let mut s = format!("{dim_sgr}◀ {reset_to_base}");
                for (i, &c) in choices.iter().enumerate() {
                    if i > 0 {
                        let _ = write!(s, "{dim_sgr} · {reset_to_base}");
                    }
                    if c.eq_ignore_ascii_case(active) {
                        let _ = write!(s, "{active_sgr} {c} {reset_to_base}");
                    } else {
                        let _ = write!(s, "{dim_sgr}{c}{reset_to_base}");
                    }
                }
                let _ = write!(s, "{dim_sgr} ▶{reset_to_base}");
                (s, plain_w)
            }
        };

        let (styled, vis_w) = match action {
            MenuAction::DiffContext(_) => {
                let val = if self.diff_context == Self::DIFF_CONTEXT_FULL {
                    "full".to_string()
                } else {
                    format!("{} lines", self.diff_context)
                };
                let plain = format!("◀  {val}  (0..full) ▶");
                let w = unicode_width::UnicodeWidthStr::width(plain.as_str());
                let s = format!(
                    "{dim_sgr}◀ {reset_to_base}{active_sgr} {val} {reset_to_base} {dim_sgr}(0..full) ▶{reset_to_base}"
                );
                (s, w)
            }
            MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id) => match id {
                OptionId::LineNumber => format_bool_styled(self.line_number),
                OptionId::Committer => format_bool_styled(self.committer),
                OptionId::FileName => format_bool_styled(self.file_name),
                OptionId::FileSize => format_bool_styled(self.file_size),
                OptionId::WordDiff => format_bool_styled(self.word_diff),
                OptionId::CommitTitleRefs => format_bool_styled(self.commit_title_refs),
                OptionId::ShowChanges => format_bool_styled(self.show_changes),
                OptionId::ShowUntracked => format_bool_styled(self.show_untracked),
                OptionId::Id => format_bool_styled(self.commit_id),
                OptionId::FileFilter => format_bool_styled(self.file_filter),
                OptionId::RevFilter => format_bool_styled(self.rev_filter),
                OptionId::StatusShowUntrackedDirs => {
                    format_bool_styled(self.status_show_untracked_dirs)
                }
                OptionId::VerticalSplit => format_bool_styled(self.vertical_split),
                OptionId::Mouse => format_bool_styled(self.mouse),
                OptionId::SyntaxHighlighting => format_bool_styled(self.syntax_highlighting),
                OptionId::ReadOnly => format_bool_styled(self.read_only),
                OptionId::WrapLines => format_bool_styled(self.wrap_lines),
                OptionId::ColorMoved => format_bool_styled(self.color_moved),
                OptionId::DiffStickyHeader => format_bool_styled(self.diff_sticky_header),
                OptionId::DiffCollapseGenerated => format_bool_styled(self.diff_collapse_generated),
                OptionId::MainSpotlightDimOthers => {
                    format_bool_styled(self.main_spotlight_dim_others)
                }
                OptionId::MainDimUnreachable => format_bool_styled(self.main_dim_unreachable),
                OptionId::MainPushStatus => format_bool_styled(self.main_push_status),
                OptionId::MainDateHeat => format_bool_styled(self.main_date_heat),
                OptionId::MainSubjectRules => format_bool_styled(self.main_subject_rules),
                OptionId::MainUniquePrefix => format_bool_styled(self.main_unique_prefix),
                OptionId::MainDimMerges => format_bool_styled(self.main_dim_merges),
                OptionId::MainAuthorColor => {
                    format_choices_styled(&["hash", "me", "off"], self.main_author_color.as_str())
                }
                OptionId::MainSpotlight => format_choices_styled(
                    &["off", "author", "ancestry"],
                    self.main_spotlight.as_str(),
                ),
                OptionId::CommitTitleOverflow => match self.commit_title_overflow {
                    Some(limit) => {
                        let p = format!(" ● {limit} col ");
                        let w = unicode_width::UnicodeWidthStr::width(p.as_str());
                        (format!("{on_sgr} ● {limit} col {reset_to_base}"), w)
                    }
                    None => (format!("{off_sgr} ○ OFF {reset_to_base}"), 7),
                },
                OptionId::Date => format_choices_styled(
                    &["relative", "short", "iso", "no"],
                    self.date_format.as_str(),
                ),
                OptionId::Author => format_choices_styled(
                    &["full", "abbrev", "email", "no"],
                    self.author_short_str(),
                ),
                OptionId::LineGraphics => {
                    format_choices_styled(&["utf-8", "ascii"], self.line_graphics.as_str())
                }
                OptionId::CommitTitleGraph => {
                    format_choices_styled(&["auto", "v2", "no"], self.commit_title_graph.as_str())
                }
                OptionId::IgnoreSpace => format_choices_styled(
                    &["no", "all", "some", "at-eol"],
                    self.ignore_space.as_str(),
                ),
                OptionId::CommitOrder => format_choices_styled(
                    &["default", "topo", "date", "author-date", "reverse"],
                    self.commit_order.as_str(),
                ),
                OptionId::DiffPresentation => format_choices_styled(
                    &["banner", "fancy", "classic"],
                    self.diff_presentation.as_str(),
                ),
                OptionId::DiffHints => {
                    format_choices_styled(&["auto", "always", "never"], self.diff_hints.as_str())
                }
                OptionId::DiffLayout => {
                    format_choices_styled(&["unified", "side-by-side"], self.diff_layout.as_str())
                }
                OptionId::DiffIndicator => {
                    format_choices_styled(&["auto", "yes", "no"], self.diff_indicator.as_str())
                }
                OptionId::WordDiffPairing => format_choices_styled(
                    &["similarity", "positional"],
                    self.word_diff_pairing.as_str(),
                ),
                OptionId::SyntaxTheme => {
                    let themes = crate::highlight::available_theme_names();
                    let pos = themes
                        .iter()
                        .position(|&t| t.eq_ignore_ascii_case(&self.syntax_theme))
                        .map_or(1, |i| i + 1);
                    let suffix = format!("({}/{})", pos, themes.len());
                    let max_name = slot_width.saturating_sub(7 + suffix.len()).max(6);
                    let short_name =
                        tigrs_core::ansi::truncate_display_width(&self.syntax_theme, max_name);
                    let plain = format!("◀  {short_name}  {suffix} ▶");
                    let w = unicode_width::UnicodeWidthStr::width(plain.as_str());
                    let s = format!(
                        "{dim_sgr}◀ {reset_to_base}{active_sgr} {short_name} {reset_to_base} {dim_sgr}{suffix} ▶{reset_to_base}"
                    );
                    (s, w)
                }
                OptionId::UiTheme => {
                    let suffix = format!(
                        "{} {}/{}",
                        self.ui_theme.family_label(),
                        self.ui_theme.position(),
                        UiThemeId::ALL.len()
                    );
                    let name = self.ui_theme.as_str();
                    let plain = format!("◀  {name}  {suffix} ▶");
                    let w = unicode_width::UnicodeWidthStr::width(plain.as_str());
                    let s = format!(
                        "{dim_sgr}◀ {reset_to_base}{active_sgr} {name} {reset_to_base} {dim_sgr}{suffix} ▶{reset_to_base}"
                    );
                    (s, w)
                }
            },
            MenuAction::ResetAll => (String::new(), 0),
        };

        if vis_w < slot_width {
            format!("{styled}{}", " ".repeat(slot_width - vis_w))
        } else {
            styled
        }
    }

    /// Returns `true` if the given menu action's underlying option differs between `self` and `other`.
    #[must_use]
    pub fn option_differs_from(&self, other: &Self, action: MenuAction) -> bool {
        match action {
            MenuAction::DiffContext(_) => self.diff_context != other.diff_context,
            MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id) => {
                self.option_value_string(id) != other.option_value_string(id)
            }
            MenuAction::ResetAll => self != other,
        }
    }

    /// Counts how many menu items in `TOGGLE_MENU_ITEMS` differ between `self` and `other`.
    #[must_use]
    pub fn count_modified_from(&self, other: &Self) -> usize {
        TOGGLE_MENU_ITEMS
            .iter()
            .filter(|it| self.option_differs_from(other, it.action))
            .count()
    }

    /// Toggles an option by its canonical Tig name string, returning both the status
    /// message and the required view refresh side effect.
    pub fn toggle_by_name_with_effect(
        &mut self,
        name: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(desc) = find_descriptor(name) {
            let msg = (desc.toggle)(self);
            return Ok((msg, desc.effect));
        }
        match name.to_ascii_lowercase().as_str() {
            "diff-context" | "context" => {
                Ok((self.toggle_diff_context(1), OptionEffect::RefreshDiff))
            }
            "update-mode" | "update_mode" | "rw" => {
                Ok((self.toggle_read_only(), OptionEffect::None))
            }
            "memory-profile" | "memory_profile" => {
                self.memory_profile = self.memory_profile.next();
                Ok((
                    format!(":set memory-profile = {}", self.memory_profile.as_str()),
                    OptionEffect::RefreshDiff,
                ))
            }
            _ => Err(format!("Unknown option: '{name}'")),
        }
    }

    /// Toggles an option by its canonical Tig name string.
    pub fn toggle_by_name(&mut self, name: &str) -> Result<String, String> {
        self.toggle_by_name_with_effect(name).map(|(msg, _)| msg)
    }

    /// Sets an option by name to `value` via `:set <variable> = <value>`,
    /// returning the status message and required view refresh side effect.
    pub fn set_by_name(
        &mut self,
        variable: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if value.trim().eq_ignore_ascii_case("toggle") {
            return self.toggle_by_name_with_effect(variable);
        }
        if let Some(desc) = find_descriptor(variable) {
            return (desc.set)(self, variable, value);
        }
        let key_lower = variable.to_lowercase();
        let val_text = value.to_lowercase();
        match key_lower.as_str() {
            "update-mode" | "update_mode" | "rw" => {
                if let Some(b) = parse_bool(value) {
                    let ro = !b;
                    let ro_str = if ro { "true" } else { "false" };
                    self.set_read_only("read-only", ro_str)
                } else {
                    Err(format!(
                        "Invalid value for '{key_lower}': '{value}' (expected true/false/yes/no)"
                    ))
                }
            }
            "memory-profile" | "memory_profile" => match val_text.trim() {
                "lean" | "low" => {
                    self.memory_profile = MemoryProfile::Lean;
                    Ok((
                        ":set memory-profile = lean".to_string(),
                        OptionEffect::RefreshDiff,
                    ))
                }
                "balanced" | "medium" | "normal" => {
                    self.memory_profile = MemoryProfile::Balanced;
                    Ok((
                        ":set memory-profile = balanced".to_string(),
                        OptionEffect::RefreshDiff,
                    ))
                }
                "greedy" | "high" | "fast" | "default" => {
                    self.memory_profile = MemoryProfile::Greedy;
                    Ok((
                        ":set memory-profile = greedy".to_string(),
                        OptionEffect::RefreshDiff,
                    ))
                }
                _ => Err(format!(
                    "Invalid value for 'memory-profile': '{value}' (expected lean, balanced, or greedy)"
                )),
            },
            "side-by-side-min-width" | "side-to-side-min-width" => {
                if let Ok(min_w) = value.trim().parse::<u16>() {
                    self.side_by_side_min_width = min_w;
                    Ok((
                        format!(":set {key_lower} = {min_w}"),
                        OptionEffect::RefreshDiff,
                    ))
                } else {
                    Err(format!(
                        "Invalid value for '{key_lower}': '{value}' (expected positive integer)"
                    ))
                }
            }
            "diff-context" | "context" => {
                if val_text == "full" {
                    self.diff_context = Self::DIFF_CONTEXT_FULL;
                    Ok((
                        ":set diff-context = full".to_string(),
                        OptionEffect::RefreshDiff,
                    ))
                } else if let Ok(n) = value.trim().parse::<usize>() {
                    self.diff_context = n;
                    Ok((
                        format!(":set diff-context = {n}"),
                        OptionEffect::RefreshDiff,
                    ))
                } else {
                    Err(format!(
                        "Invalid value for 'diff-context': '{value}' (expected integer or 'full')"
                    ))
                }
            }
            "tab-size" => {
                if let Ok(n) = value.trim().parse::<usize>() {
                    if (1..=32).contains(&n) {
                        self.tab_size = n;
                        Ok((format!(":set tab-size = {n}"), OptionEffect::RefreshDiff))
                    } else {
                        Err(format!(
                            "Invalid value for 'tab-size': '{value}' (must be 1-32)"
                        ))
                    }
                } else {
                    Err(format!(
                        "Invalid value for 'tab-size': '{value}' (expected integer)"
                    ))
                }
            }
            other => Err(format!(
                "Option '{other}' cannot be set via :set; use :toggle {other}"
            )),
        }
    }

    fn set_line_number(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.line_number = b;
            Ok((
                format!(":set line-number = {}", if b { "yes" } else { "no" }),
                OptionId::LineNumber.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'line-number': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_date(&mut self, _var: &str, value: &str) -> Result<(String, OptionEffect), String> {
        match value.to_lowercase().as_str() {
            "relative" | "default" => {
                self.date_format = DateFormat::Relative;
                Ok((
                    ":set date = relative".to_string(),
                    OptionId::Date.descriptor().effect,
                ))
            }
            "local" | "iso" => {
                self.date_format = DateFormat::Iso;
                Ok((
                    format!(":set date = {}", self.date_format.as_str()),
                    OptionId::Date.descriptor().effect,
                ))
            }
            "short" => {
                self.date_format = DateFormat::Short;
                Ok((
                    ":set date = short".to_string(),
                    OptionId::Date.descriptor().effect,
                ))
            }
            "no" | "false" | "off" | "0" => {
                self.date_format = DateFormat::Off;
                Ok((
                    ":set date = no".to_string(),
                    OptionId::Date.descriptor().effect,
                ))
            }
            _ => Err(format!(
                "Invalid value for 'date': '{value}' (expected relative/local/iso/short/no)"
            )),
        }
    }

    fn set_author(&mut self, _var: &str, value: &str) -> Result<(String, OptionEffect), String> {
        match value.to_lowercase().as_str() {
            "full" | "default" => {
                self.author_format = AuthorFormat::Full;
                Ok((
                    ":set author = full".to_string(),
                    OptionId::Author.descriptor().effect,
                ))
            }
            "abbreviated" | "abbr" => {
                self.author_format = AuthorFormat::Abbreviated;
                Ok((
                    ":set author = abbreviated".to_string(),
                    OptionId::Author.descriptor().effect,
                ))
            }
            "email" | "email-user" => {
                self.author_format = AuthorFormat::Email;
                Ok((
                    ":set author = email".to_string(),
                    OptionId::Author.descriptor().effect,
                ))
            }
            "no" | "false" | "off" | "0" => {
                self.author_format = AuthorFormat::Off;
                Ok((
                    ":set author = no".to_string(),
                    OptionId::Author.descriptor().effect,
                ))
            }
            _ => Err(format!(
                "Invalid value for 'author': '{value}' (expected full/abbreviated/email/email-user/no)"
            )),
        }
    }

    fn set_committer(&mut self, _var: &str, value: &str) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.committer = b;
            Ok((
                format!(":set committer = {}", if b { "yes" } else { "no" }),
                OptionId::Committer.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'committer': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_line_graphics(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        match value.to_lowercase().as_str() {
            "ascii" => {
                self.line_graphics = LineGraphics::Ascii;
                Ok((
                    ":set line-graphics = ascii".to_string(),
                    OptionId::LineGraphics.descriptor().effect,
                ))
            }
            "utf-8" | "utf8" | "default" => {
                self.line_graphics = LineGraphics::Utf8;
                Ok((
                    ":set line-graphics = default".to_string(),
                    OptionId::LineGraphics.descriptor().effect,
                ))
            }
            _ => Err(format!(
                "Invalid value for 'line-graphics': '{value}' (expected ascii or default/utf-8)"
            )),
        }
    }

    fn set_commit_title_graph(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        match value.to_lowercase().as_str() {
            "auto" | "default" | "yes" | "true" | "1" | "on" => {
                self.commit_title_graph = GraphDisplay::Auto;
                Ok((
                    ":set commit-title-graph = auto".to_string(),
                    OptionId::CommitTitleGraph.descriptor().effect,
                ))
            }
            "v2" => {
                self.commit_title_graph = GraphDisplay::V2;
                Ok((
                    ":set commit-title-graph = v2".to_string(),
                    OptionId::CommitTitleGraph.descriptor().effect,
                ))
            }
            "no" | "false" | "off" | "0" => {
                self.commit_title_graph = GraphDisplay::No;
                Ok((
                    ":set commit-title-graph = no".to_string(),
                    OptionId::CommitTitleGraph.descriptor().effect,
                ))
            }
            _ => Err(format!(
                "Invalid value for 'commit-title-graph': '{value}' (expected auto/v2/no)"
            )),
        }
    }

    fn set_file_name(&mut self, _var: &str, value: &str) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.file_name = b;
            Ok((
                format!(":set file-name = {}", if b { "yes" } else { "no" }),
                OptionId::FileName.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'file-name': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_file_size(&mut self, _var: &str, value: &str) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.file_size = b;
            Ok((
                format!(":set file-size = {}", if b { "yes" } else { "no" }),
                OptionId::FileSize.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'file-size': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_ignore_space(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Ok(mode) = value.parse::<IgnoreSpace>() {
            self.ignore_space = mode;
            Ok((
                format!(":set ignore-space = {}", mode.as_str()),
                OptionId::IgnoreSpace.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'ignore-space': '{value}' (expected yes/no/all/some/at-eol)"
            ))
        }
    }

    fn set_word_diff(&mut self, _var: &str, value: &str) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.word_diff = b;
            Ok((
                format!(":set word-diff = {}", if b { "yes" } else { "no" }),
                OptionId::WordDiff.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'word-diff': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_commit_order(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Ok(order) = value.parse::<CommitOrder>() {
            self.commit_order = order;
            Ok((
                format!(":set commit-order = {}", order.as_str()),
                OptionId::CommitOrder.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'commit-order': '{value}' (expected topo/date/author-date/reverse/default)"
            ))
        }
    }

    fn set_commit_title_refs(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.commit_title_refs = b;
            Ok((
                format!(":set commit-title-refs = {}", if b { "yes" } else { "no" }),
                OptionId::CommitTitleRefs.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'commit-title-refs': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_show_changes(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.show_changes = b;
            Ok((
                format!(":set show-changes = {}", if b { "yes" } else { "no" }),
                OptionId::ShowChanges.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'show-changes': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_show_untracked(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.show_untracked = b;
            Ok((
                format!(":set show-untracked = {}", if b { "yes" } else { "no" }),
                OptionId::ShowUntracked.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'show-untracked': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_id(&mut self, _var: &str, value: &str) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.commit_id = b;
            Ok((
                format!(":set id = {}", if b { "yes" } else { "no" }),
                OptionId::Id.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'id': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_file_filter(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.file_filter = b;
            Ok((
                format!(":set file-filter = {}", if b { "yes" } else { "no" }),
                OptionId::FileFilter.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'file-filter': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_rev_filter(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.rev_filter = b;
            Ok((
                format!(":set rev-filter = {}", if b { "yes" } else { "no" }),
                OptionId::RevFilter.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'rev-filter': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_commit_title_overflow(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Ok(n) = value.trim().parse::<usize>() {
            self.commit_title_overflow = if n > 0 { Some(n) } else { None };
            Ok((
                format!(
                    ":set commit-title-overflow = {}",
                    if n > 0 {
                        n.to_string()
                    } else {
                        "no".to_string()
                    }
                ),
                OptionId::CommitTitleOverflow.descriptor().effect,
            ))
        } else if let Some(b) = parse_bool(value) {
            self.commit_title_overflow = if b { Some(50) } else { None };
            Ok((
                format!(
                    ":set commit-title-overflow = {}",
                    if b { "50" } else { "no" }
                ),
                OptionId::CommitTitleOverflow.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'commit-title-overflow': '{value}' (expected column number or yes/no)"
            ))
        }
    }

    fn set_status_show_untracked_dirs(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.status_show_untracked_dirs = b;
            Ok((
                format!(
                    ":set status-show-untracked-dirs = {}",
                    if b { "yes" } else { "no" }
                ),
                OptionId::StatusShowUntrackedDirs.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'status-show-untracked-dirs': '{value}' (expected yes/no)"
            ))
        }
    }

    fn set_vertical_split(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        let is_vertical = match value.to_lowercase().as_str() {
            "vertical" | "yes" | "true" | "1" | "on" => Some(true),
            "horizontal" | "no" | "false" | "0" | "off" => Some(false),
            _ => None,
        };
        if let Some(b) = is_vertical {
            self.vertical_split = b;
            Ok((
                format!(":set vertical-split = {}", if b { "yes" } else { "no" }),
                OptionId::VerticalSplit.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'vertical-split': '{value}' (expected vertical/horizontal or yes/no)"
            ))
        }
    }

    fn set_diff_presentation(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        match value.to_lowercase().as_str() {
            "banner" | "default" | "compact" | "gerrit" => {
                self.diff_presentation = DiffPresentation::Banner;
                Ok((
                    ":set diff-presentation = banner".to_string(),
                    OptionId::DiffPresentation.descriptor().effect,
                ))
            }
            "fancy" => {
                self.diff_presentation = DiffPresentation::Fancy;
                Ok((
                    ":set diff-presentation = fancy".to_string(),
                    OptionId::DiffPresentation.descriptor().effect,
                ))
            }
            "classic" | "git" | "raw" => {
                self.diff_presentation = DiffPresentation::Classic;
                Ok((
                    ":set diff-presentation = classic".to_string(),
                    OptionId::DiffPresentation.descriptor().effect,
                ))
            }
            _ => Err(format!(
                "Invalid value for 'diff-presentation': '{value}' (expected banner, fancy, or classic)"
            )),
        }
    }

    fn set_diff_layout(
        &mut self,
        var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        let key_lower = var.to_lowercase();
        let val_text = value.to_lowercase();
        match key_lower.as_str() {
            "side-by-side" | "side-to-side" => {
                let b = parse_bool(value).unwrap_or(true);
                self.diff_layout = if b {
                    DiffLayout::SideBySide
                } else {
                    DiffLayout::Unified
                };
                Ok((
                    format!(":set {key_lower} = {}", if b { "yes" } else { "no" }),
                    OptionId::DiffLayout.descriptor().effect,
                ))
            }
            "single" | "unified" => {
                let b = parse_bool(value).unwrap_or(true);
                self.diff_layout = if b {
                    DiffLayout::Unified
                } else {
                    DiffLayout::SideBySide
                };
                Ok((
                    format!(":set {key_lower} = {}", if b { "yes" } else { "no" }),
                    OptionId::DiffLayout.descriptor().effect,
                ))
            }
            _ => match val_text.as_str() {
                "side-by-side" | "side_by_side" | "side-to-side" | "side_to_side" => {
                    self.diff_layout = DiffLayout::SideBySide;
                    let disp = if val_text.contains("to") {
                        "side-to-side"
                    } else {
                        "side-by-side"
                    };
                    Ok((
                        format!(":set {key_lower} = {disp}"),
                        OptionId::DiffLayout.descriptor().effect,
                    ))
                }
                "unified" | "single" | "default" => {
                    self.diff_layout = DiffLayout::Unified;
                    let disp = if val_text == "single" {
                        "single"
                    } else {
                        "unified"
                    };
                    Ok((
                        format!(":set {key_lower} = {disp}"),
                        OptionId::DiffLayout.descriptor().effect,
                    ))
                }
                _ => Err(format!(
                    "Invalid value for '{key_lower}': '{value}' (expected side-by-side/side-to-side or unified/single)"
                )),
            },
        }
    }

    fn set_diff_indicator(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        match value.to_lowercase().as_str() {
            "auto" | "default" => {
                self.diff_indicator = DiffIndicator::Auto;
                Ok((
                    ":set diff-indicator = auto".to_string(),
                    OptionId::DiffIndicator.descriptor().effect,
                ))
            }
            "yes" | "true" | "1" | "on" => {
                self.diff_indicator = DiffIndicator::Yes;
                Ok((
                    ":set diff-indicator = yes".to_string(),
                    OptionId::DiffIndicator.descriptor().effect,
                ))
            }
            "no" | "false" | "0" | "off" => {
                self.diff_indicator = DiffIndicator::No;
                Ok((
                    ":set diff-indicator = no".to_string(),
                    OptionId::DiffIndicator.descriptor().effect,
                ))
            }
            _ => Err(format!(
                "Invalid value for 'diff-indicator': '{value}' (expected auto/yes/no)"
            )),
        }
    }

    fn set_word_diff_pairing(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        match value.to_lowercase().as_str() {
            "similarity" | "default" => {
                self.word_diff_pairing = WordDiffPairing::Similarity;
                Ok((
                    ":set word-diff-pairing = similarity".to_string(),
                    OptionId::WordDiffPairing.descriptor().effect,
                ))
            }
            "positional" => {
                self.word_diff_pairing = WordDiffPairing::Positional;
                Ok((
                    ":set word-diff-pairing = positional".to_string(),
                    OptionId::WordDiffPairing.descriptor().effect,
                ))
            }
            _ => Err(format!(
                "Invalid value for 'word-diff-pairing': '{value}' (expected similarity or positional)"
            )),
        }
    }

    fn set_mouse(&mut self, _var: &str, value: &str) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.mouse = b;
            Ok((
                format!(":set mouse = {}", if b { "yes" } else { "no" }),
                OptionId::Mouse.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'mouse': '{value}' (expected true/false/yes/no)"
            ))
        }
    }

    fn set_syntax_highlighting(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.syntax_highlighting = b;
            Ok((
                format!(
                    ":set syntax-highlighting = {}",
                    if b { "yes" } else { "no" }
                ),
                OptionId::SyntaxHighlighting.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'syntax-highlighting': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_syntax_theme(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        let val_text = value.to_lowercase();
        if value == "?" || val_text == "list" {
            let themes = crate::highlight::available_theme_names().join(", ");
            Ok((
                format!("Available syntax themes: {themes}"),
                OptionEffect::None,
            ))
        } else if let Some((canonical, _)) = crate::highlight::resolve_theme(value) {
            self.syntax_theme = canonical.to_string();
            Ok((
                format!(":set syntax-theme = {canonical}"),
                OptionId::SyntaxTheme.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Unknown syntax theme: '{value}' (use ':set syntax-theme ?' to list available themes)"
            ))
        }
    }

    fn set_read_only(&mut self, _var: &str, value: &str) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.read_only = b;
            let status = if b {
                ":set read-only = yes (Read-Only Mode ENABLED)"
            } else {
                ":set read-only = no (Update Mode ENABLED — repository modifications allowed)"
            };
            Ok((status.to_string(), OptionId::ReadOnly.descriptor().effect))
        } else {
            Err(format!(
                "Invalid value for 'read-only': '{value}' (expected true/false/yes/no/on/off)"
            ))
        }
    }

    fn set_ui_theme(&mut self, _var: &str, value: &str) -> Result<(String, OptionEffect), String> {
        let val_text = value.trim().to_lowercase();
        if val_text == "?" || val_text == "list" {
            let names: Vec<String> = UiThemeId::ALL
                .iter()
                .map(|t| format!("{} ({})", t.as_str(), t.family_label()))
                .collect();
            return Ok((
                format!("Available UI color themes: {}", names.join(", ")),
                OptionEffect::None,
            ));
        }
        if let Ok(theme) = val_text.parse::<UiThemeId>() {
            self.ui_theme = theme;
            self.syntax_theme =
                crate::highlight::canonical_theme_name(theme.paired_syntax_theme()).to_string();
            Ok((
                format!(
                    ":set ui-theme = {} ({})",
                    theme.as_str(),
                    theme.family_label()
                ),
                OptionId::UiTheme.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Unknown UI color theme: '{value}' (use ':set ui-theme ?' to list available themes)"
            ))
        }
    }

    fn set_wrap_lines(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.wrap_lines = b;
            Ok((
                format!(":set wrap-lines = {}", if b { "yes" } else { "no" }),
                OptionId::WrapLines.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'wrap-lines': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_color_moved(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.color_moved = b;
            Ok((
                format!(":set color-moved = {}", if b { "yes" } else { "no" }),
                OptionId::ColorMoved.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'color-moved': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_diff_sticky_header(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.diff_sticky_header = b;
            Ok((
                format!(":set diff-sticky-header = {}", if b { "yes" } else { "no" }),
                OptionId::DiffStickyHeader.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'diff-sticky-header': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_diff_hints(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        match value.to_lowercase().as_str() {
            "auto" | "default" => {
                self.diff_hints = tigrs_core::DiffHintsMode::Auto;
                Ok((
                    ":set diff-hints = auto".to_string(),
                    OptionId::DiffHints.descriptor().effect,
                ))
            }
            "always" | "yes" | "true" | "1" | "on" => {
                self.diff_hints = tigrs_core::DiffHintsMode::Always;
                Ok((
                    ":set diff-hints = always".to_string(),
                    OptionId::DiffHints.descriptor().effect,
                ))
            }
            "never" | "no" | "false" | "0" | "off" => {
                self.diff_hints = tigrs_core::DiffHintsMode::Never;
                Ok((
                    ":set diff-hints = never".to_string(),
                    OptionId::DiffHints.descriptor().effect,
                ))
            }
            _ => Err(format!(
                "Invalid value for 'diff-hints': '{value}' (expected auto, always, or never)"
            )),
        }
    }

    fn set_diff_collapse_generated(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.diff_collapse_generated = b;
            Ok((
                format!(
                    ":set diff-collapse-generated = {}",
                    if b { "yes" } else { "no" }
                ),
                OptionId::DiffCollapseGenerated.descriptor().effect,
            ))
        } else {
            Err(format!(
                "Invalid value for 'diff-collapse-generated': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_main_author_color(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "hash" | "default" | "yes" | "true" | "on" | "1" => {
                self.main_author_color = MainAuthorColor::Hash;
                Ok((
                    ":set main-author-color = hash".to_string(),
                    OptionEffect::None,
                ))
            }
            "me" | "self" => {
                self.main_author_color = MainAuthorColor::Me;
                Ok((
                    ":set main-author-color = me".to_string(),
                    OptionEffect::None,
                ))
            }
            "off" | "no" | "false" | "0" | "none" => {
                self.main_author_color = MainAuthorColor::Off;
                Ok((
                    ":set main-author-color = off".to_string(),
                    OptionEffect::None,
                ))
            }
            _ => Err(format!(
                "Invalid value for 'main-author-color': '{value}' (expected hash, me, or off)"
            )),
        }
    }

    fn set_main_spotlight(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "no" | "false" | "0" | "none" | "default" => {
                self.main_spotlight = MainSpotlight::Off;
                Ok((":set main-spotlight = off".to_string(), OptionEffect::None))
            }
            "author" | "same-author" | "same_author" | "yes" | "true" | "on" | "1" => {
                self.main_spotlight = MainSpotlight::Author;
                Ok((
                    ":set main-spotlight = author".to_string(),
                    OptionEffect::None,
                ))
            }
            "ancestry" | "lineage" | "dag" => {
                self.main_spotlight = MainSpotlight::Ancestry;
                Ok((
                    ":set main-spotlight = ancestry".to_string(),
                    OptionEffect::None,
                ))
            }
            _ => Err(format!(
                "Invalid value for 'main-spotlight': '{value}' (expected off, author, or ancestry)"
            )),
        }
    }

    fn set_main_spotlight_dim_others(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.main_spotlight_dim_others = b;
            Ok((
                format!(
                    ":set main-spotlight-dim-others = {}",
                    if b { "yes" } else { "no" }
                ),
                OptionEffect::None,
            ))
        } else {
            Err(format!(
                "Invalid value for 'main-spotlight-dim-others': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_main_dim_unreachable(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.main_dim_unreachable = b;
            Ok((
                format!(
                    ":set main-dim-unreachable = {}",
                    if b { "yes" } else { "no" }
                ),
                OptionEffect::None,
            ))
        } else {
            Err(format!(
                "Invalid value for 'main-dim-unreachable': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_main_push_status(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.main_push_status = b;
            Ok((
                format!(":set main-push-status = {}", if b { "yes" } else { "no" }),
                OptionEffect::None,
            ))
        } else {
            Err(format!(
                "Invalid value for 'main-push-status': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_main_date_heat(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.main_date_heat = b;
            Ok((
                format!(":set main-date-heat = {}", if b { "yes" } else { "no" }),
                OptionEffect::None,
            ))
        } else {
            Err(format!(
                "Invalid value for 'main-date-heat': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_main_subject_rules(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.main_subject_rules = b;
            Ok((
                format!(":set main-subject-rules = {}", if b { "yes" } else { "no" }),
                OptionEffect::None,
            ))
        } else {
            Err(format!(
                "Invalid value for 'main-subject-rules': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_main_unique_prefix(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.main_unique_prefix = b;
            Ok((
                format!(":set main-unique-prefix = {}", if b { "yes" } else { "no" }),
                OptionEffect::None,
            ))
        } else {
            Err(format!(
                "Invalid value for 'main-unique-prefix': '{value}' (expected yes/no/on/off)"
            ))
        }
    }

    fn set_main_dim_merges(
        &mut self,
        _var: &str,
        value: &str,
    ) -> Result<(String, OptionEffect), String> {
        if let Some(b) = parse_bool(value) {
            self.main_dim_merges = b;
            Ok((
                format!(":set main-dim-merges = {}", if b { "yes" } else { "no" }),
                OptionEffect::None,
            ))
        } else {
            Err(format!(
                "Invalid value for 'main-dim-merges': '{value}' (expected yes/no/on/off)"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_options_registry_integrity_and_human_titles() {
        let mut seen_ids = HashSet::new();
        let mut seen_names = HashSet::new();

        for (idx, desc) in OPTIONS_REGISTRY.iter().enumerate() {
            assert_eq!(
                desc.id as usize, idx,
                "OPTIONS_REGISTRY index {idx} must match OptionId discriminant {:?}",
                desc.id
            );
            assert!(
                seen_ids.insert(desc.id),
                "Duplicate OptionId in OPTIONS_REGISTRY: {:?}",
                desc.id
            );
            assert!(
                seen_names.insert(desc.canonical_name),
                "Duplicate canonical_name in OPTIONS_REGISTRY: {}",
                desc.canonical_name
            );
            assert!(!desc.toml_section.is_empty() && !desc.toml_key.is_empty());
            assert!(
                !desc.description.is_empty(),
                "Description for {:?} must not be empty",
                desc.id
            );
        }
    }

    #[test]
    fn test_diff_header_options_set_and_widget_formatting() {
        let mut opts = ViewOptions::default();

        // 1. diff-presentation = banner / fancy / classic
        assert_eq!(
            opts.option_value_string(OptionId::DiffPresentation),
            "banner"
        );
        (OptionId::DiffPresentation.descriptor().set)(&mut opts, "diff-presentation", "fancy")
            .unwrap();
        assert_eq!(opts.diff_presentation, DiffPresentation::Fancy);
        (OptionId::DiffPresentation.descriptor().set)(&mut opts, "diff-presentation", "banner")
            .unwrap();
        assert_eq!(opts.diff_presentation, DiffPresentation::Banner);

        // 2. diff-sticky-header
        assert!(opts.diff_sticky_header);
        (OptionId::DiffStickyHeader.descriptor().set)(&mut opts, "diff-sticky-header", "no")
            .unwrap();
        assert!(!opts.diff_sticky_header);

        // 3. diff-hints = auto / always / never
        (OptionId::DiffHints.descriptor().set)(&mut opts, "diff-hints", "always").unwrap();
        assert_eq!(opts.diff_hints, tigrs_core::DiffHintsMode::Always);
        (OptionId::DiffHints.descriptor().set)(&mut opts, "diff-hints", "never").unwrap();
        assert_eq!(opts.diff_hints, tigrs_core::DiffHintsMode::Never);
        assert!(
            (OptionId::DiffHints.descriptor().set)(&mut opts, "diff-hints", "invalid").is_err()
        );

        // 4. diff-collapse-generated
        (OptionId::DiffCollapseGenerated.descriptor().set)(
            &mut opts,
            "diff-collapse-generated",
            "off",
        )
        .unwrap();
        assert!(!opts.diff_collapse_generated);
        assert!(
            (OptionId::DiffCollapseGenerated.descriptor().set)(
                &mut opts,
                "diff-collapse-generated",
                "bad"
            )
            .is_err()
        );
    }
}
