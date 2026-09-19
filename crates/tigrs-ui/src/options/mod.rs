// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! View options, display settings toggles, and interactive option menu for tigrs.
//!
//! Provides 100% parity with Tig's display options and 18-item toggle menu (`o`),
//! supporting direct key shortcuts, command line toggling (`:toggle <name>`),
//! and interactive menu navigation with hotkey selection.

// These option enums are shared with the config-file layer, which parses,
// validates, and serializes them via `config_enum!`; see `tigrs_core::config_enums`.
pub use tigrs_core::MainSubjectRuleConfig;
pub use tigrs_core::config_enums::{
    AuthorFormat, CommitOrder, DateFormat, DiffHintsMode, DiffIndicator, DiffLayout,
    DiffPresentation, GraphDisplay, IgnoreSpace, LineGraphics, MainAuthorColor, MainSpotlight,
    MemoryProfile, UiThemeId, WordDiffPairing,
};

pub mod menu;
pub mod registry;

pub use menu::*;
pub use registry::*;

/// Complete configuration options controlling display behavior across views.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewOptions {
    /// Display line numbers in views.
    pub line_number: bool,
    /// Commit date display formatting mode.
    pub date_format: DateFormat,
    /// Commit author display formatting mode.
    pub author_format: AuthorFormat,
    /// Display committer instead of or alongside author.
    pub committer: bool,
    /// Box-drawing graphics style (Unicode or ASCII).
    pub line_graphics: LineGraphics,
    /// Commit DAG graph display mode.
    pub commit_title_graph: GraphDisplay,
    /// Display file names in diff / tree / status.
    pub file_name: bool,
    /// Display file sizes in tree view.
    pub file_size: bool,
    /// Ignore whitespace differences in diffs.
    pub ignore_space: IgnoreSpace,
    /// Emphasise the changed words within a modified line, rather than
    /// colouring the whole line uniformly.
    pub word_diff: bool,
    /// Ordering of commits in log / main view.
    pub commit_order: CommitOrder,
    /// Display branch heads and tag ref labels `[main]`, `[v1.0]`.
    pub commit_title_refs: bool,
    /// Display local uncommitted changes in main view.
    pub show_changes: bool,
    /// Include untracked files among the local changes shown in main view.
    pub show_untracked: bool,
    /// Display commit SHA IDs in main view.
    pub commit_id: bool,
    /// Filter diff view to single file.
    pub file_filter: bool,
    /// Filter revisions in main view.
    pub rev_filter: bool,
    /// Highlight commit title overflow past column threshold (e.g. 50).
    pub commit_title_overflow: Option<usize>,
    /// Show untracked directories as single entries in status view.
    pub status_show_untracked_dirs: bool,
    /// Vertical split mode for dual views.
    pub vertical_split: bool,
    /// Number of diff context lines.
    pub diff_context: usize,
    /// Tab expansion visual width.
    pub tab_size: usize,
    /// Whether to pass line number (+LINE) to editor command.
    pub editor_line_number: bool,
    /// Diff presentation style: banner, fancy, or classic.
    pub diff_presentation: DiffPresentation,
    /// Diff layout mode: unified or side-by-side.
    pub diff_layout: DiffLayout,
    /// Diff indicator sign mode: auto, yes, or no.
    pub diff_indicator: DiffIndicator,
    /// Word diff line pairing mode: similarity or positional.
    pub word_diff_pairing: WordDiffPairing,
    /// Minimum terminal width required for side-by-side view.
    pub side_by_side_min_width: u16,
    /// External diff formatter command string.
    pub diff_formatter: String,
    /// Maximum line count for full-file diff context expansion.
    pub diff_context_full_max_lines: usize,
    /// Whether mouse tracking is enabled.
    pub mouse: bool,
    /// Code syntax highlighting enabled in diff, blob, and blame views.
    pub syntax_highlighting: bool,
    /// Syntax highlighting color theme name.
    pub syntax_theme: String,
    /// UI view color theme across Adaptive, Dark, Light, and High-Contrast families.
    pub ui_theme: UiThemeId,
    /// Custom color customizations mapping area or prefix names to color specs.
    pub colors: indexmap::IndexMap<String, tigrs_core::ColorSpec>,
    /// Prevent any repository state mutations (`true` by default unless `--update-mode` or `:set read-only = false`).
    pub read_only: bool,
    /// Memory-for-speed caching and prefetch profile (`Greedy` by default).
    pub memory_profile: MemoryProfile,
    /// Soft-wrap long lines in diff view instead of truncating horizontally (`wrap-lines`).
    pub wrap_lines: bool,
    /// Detect and highlight moved code blocks across diff hunks and files (`color-moved`).
    pub color_moved: bool,
    /// Pin an adaptive sticky file and function header when a file's banner scrolls out of view (`diff-sticky-header`).
    pub diff_sticky_header: bool,
    /// Contextual action hint chips in diff file banners (`diff-hints`).
    pub diff_hints: DiffHintsMode,
    /// Automatically collapse files marked `linguist-generated` in `.gitattributes` (`diff-collapse-generated`).
    pub diff_collapse_generated: bool,
    /// Author column coloring mode in main view (`hash`, `me`, `off`).
    pub main_author_color: MainAuthorColor,
    /// Interactive spotlight mode in main view (`off`, `author`, `ancestry`).
    pub main_spotlight: MainSpotlight,
    /// Whether non-matching rows are dimmed (`Attrs::DIM`) while a spotlight is active.
    pub main_spotlight_dim_others: bool,
    /// Dim commits not reachable from `HEAD` when browsing `--all` (`main-dim-unreachable`).
    pub main_dim_unreachable: bool,
    /// Color commit SHAs by push/upstream status (`↑` ahead / unmerged).
    pub main_push_status: bool,
    /// Apply a 6-bucket age heatmap gradient to the Date column (`main-date-heat`).
    pub main_date_heat: bool,
    /// Apply semantic pattern rules (`fixup!`, `Revert`, `WIP`, conventional prefixes, `#123`) to commit subjects.
    pub main_subject_rules: bool,
    /// Highlight the shortest unique SHA prefix when the `id` column is visible (`main-unique-prefix`).
    pub main_unique_prefix: bool,
    /// Dim merge commit subject boilerplate (`Merge branch ...`, `Merge pull request ...`) (`main-dim-merges`).
    pub main_dim_merges: bool,
    /// User-defined `[colors.authors]` mapping author name or email to color spec string.
    pub author_colors: indexmap::IndexMap<String, String>,
    /// User-defined `[[main.subject-rules]]` entries from `config.toml`.
    pub custom_subject_rules: Vec<MainSubjectRuleConfig>,
    /// Whether ANSI color output is disabled (`NO_COLOR` / `ColorLevel::Monochrome`).
    pub no_color: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            line_number: false,
            date_format: DateFormat::Relative,
            author_format: AuthorFormat::Full,
            committer: false,
            line_graphics: LineGraphics::Utf8,
            commit_title_graph: GraphDisplay::Auto,
            file_name: true,
            file_size: true,
            ignore_space: IgnoreSpace::No,
            word_diff: true,
            commit_order: CommitOrder::Default,
            commit_title_refs: true,
            show_changes: true,
            show_untracked: true,
            commit_id: false,
            file_filter: false,
            rev_filter: false,
            commit_title_overflow: None,
            status_show_untracked_dirs: true,
            vertical_split: false,
            diff_context: 3,
            tab_size: 8,
            editor_line_number: true,
            diff_presentation: DiffPresentation::Banner,
            diff_layout: DiffLayout::Unified,
            diff_indicator: DiffIndicator::Auto,
            word_diff_pairing: WordDiffPairing::Similarity,
            side_by_side_min_width: 80,
            diff_formatter: String::new(),
            diff_context_full_max_lines: 50_000,
            mouse: false,
            syntax_highlighting: true,
            syntax_theme: crate::highlight::DEFAULT_SYNTAX_THEME.to_string(),
            ui_theme: UiThemeId::Default,
            colors: indexmap::IndexMap::new(),
            read_only: false,
            memory_profile: MemoryProfile::Greedy,
            wrap_lines: false,
            color_moved: true,
            diff_sticky_header: true,
            diff_hints: DiffHintsMode::Auto,
            diff_collapse_generated: true,
            main_author_color: MainAuthorColor::Hash,
            main_spotlight: MainSpotlight::Author,
            main_spotlight_dim_others: true,
            main_dim_unreachable: true,
            main_push_status: true,
            main_date_heat: true,
            main_subject_rules: true,
            main_unique_prefix: true,
            main_dim_merges: true,
            author_colors: indexmap::IndexMap::new(),
            custom_subject_rules: Vec::new(),
            no_color: false,
        }
    }
}

/// Sentinel value indicating full-file context expansion.
pub const DIFF_CONTEXT_FULL: usize = usize::MAX;

impl ViewOptions {
    /// Sentinel value indicating full-file context expansion.
    pub const DIFF_CONTEXT_FULL: usize = DIFF_CONTEXT_FULL;

    /// Resolves the semantic [`crate::ui_theme::UiPalette`] for `self.ui_theme`
    /// with any explicit `[colors]` user overrides layered on top.
    #[must_use]
    pub fn ui_palette(&self) -> crate::ui_theme::UiPalette {
        let mut palette = crate::ui_theme::UiPalette::for_theme(self.ui_theme);
        if !self.colors.is_empty() {
            palette.apply_custom_colors(&self.colors);
        }
        palette
    }

    /// Returns true if full file diff context expansion is enabled.
    #[must_use]
    pub fn is_full_context(&self) -> bool {
        self.diff_context == Self::DIFF_CONTEXT_FULL
    }

    /// Applies settings from deserialized TOML `Config`.
    pub fn apply_config(&mut self, config: &tigrs_core::Config) {
        self.tab_size = if (1..=32).contains(&config.general.tab_size) {
            config.general.tab_size
        } else {
            8
        };
        self.editor_line_number = config.general.editor_line_number;
        self.line_graphics = config.general.line_graphics;
        self.vertical_split = matches!(
            config.general.vertical_split.to_lowercase().as_str(),
            "vertical" | "true" | "yes" | "on" | "1"
        );
        self.line_number = config.view.line_number;
        self.date_format = DateFormat::from_config_str(&config.view.date);
        self.author_format = AuthorFormat::from_config_str(&config.view.author);
        self.committer = config.view.committer;
        self.commit_title_graph = GraphDisplay::from_config_str(&config.view.commit_title_graph);
        self.file_name = !matches!(
            config.view.file_name.trim().to_ascii_lowercase().as_str(),
            "no" | "false" | "off" | "0"
        );
        self.file_size = !matches!(
            config.view.file_size.trim().to_ascii_lowercase().as_str(),
            "no" | "false" | "off" | "0"
        );
        self.commit_order = config.view.commit_order;
        self.commit_title_refs = config.view.commit_title_refs;
        self.ignore_space = config.view.ignore_space;
        self.show_changes = config.view.show_changes;
        self.show_untracked = config.view.show_untracked;
        self.commit_id = config.view.id;
        self.file_filter = config.view.file_filter;
        self.rev_filter = config.view.rev_filter;
        self.commit_title_overflow = if config.view.commit_title_overflow > 0 {
            Some(config.view.commit_title_overflow)
        } else {
            None
        };
        self.status_show_untracked_dirs = config.view.status_show_untracked_dirs;

        self.diff_presentation = config.view.diff_presentation;
        self.diff_layout = config.view.diff_layout;
        self.diff_indicator = config.view.diff_indicator;
        self.word_diff_pairing = config.view.word_diff_pairing;
        self.word_diff = config.view.word_diff;
        self.side_by_side_min_width = config.view.side_by_side_min_width;
        self.diff_formatter.clone_from(&config.view.diff_formatter);
        self.diff_context_full_max_lines = config.view.diff_context_full_max_lines;
        self.mouse = config.general.mouse;
        self.read_only = config.general.read_only;
        self.syntax_highlighting = config.view.syntax_highlighting;
        self.ui_theme = config.view.ui_theme;
        if !config.view.syntax_theme.trim().is_empty() {
            self.syntax_theme =
                crate::highlight::canonical_theme_name(&config.view.syntax_theme).to_string();
        }
        if self.ui_theme != UiThemeId::Default
            && config
                .view
                .syntax_theme
                .trim()
                .eq_ignore_ascii_case(crate::highlight::DEFAULT_SYNTAX_THEME)
        {
            self.syntax_theme =
                crate::highlight::canonical_theme_name(self.ui_theme.paired_syntax_theme())
                    .to_string();
        }
        if config.view.diff_context.eq_ignore_ascii_case("full") {
            self.diff_context = Self::DIFF_CONTEXT_FULL;
        } else if let Ok(n) = config.view.diff_context.parse::<usize>() {
            self.diff_context = n;
        }
        self.colors.clone_from(&config.colors);
        self.memory_profile = config.performance.memory_profile;
        self.wrap_lines = config.view.wrap_lines;
        self.color_moved = config.view.color_moved;
        self.diff_sticky_header = config.view.diff_sticky_header;
        self.diff_hints = config.view.diff_hints;
        self.diff_collapse_generated = config.view.diff_collapse_generated;
        self.main_author_color = config.view.main_author_color;
        self.main_spotlight = config.view.main_spotlight;
        self.main_spotlight_dim_others = config.view.main_spotlight_dim_others;
        self.main_dim_unreachable = config.view.main_dim_unreachable;
        self.main_push_status = config.view.main_push_status;
        self.main_date_heat = config.view.main_date_heat;
        self.main_subject_rules = config.view.main_subject_rules;
        self.main_unique_prefix = config.view.main_unique_prefix;
        self.main_dim_merges = config.view.main_dim_merges;
        self.author_colors.clone_from(&config.author_colors);
        self.custom_subject_rules
            .clone_from(&config.main_subject_rules);
    }

    /// Synchronizes all runtime `ViewOptions` values back into a serializable [`tigrs_core::Config`].
    pub fn write_into_config(&self, config: &mut tigrs_core::Config) {
        config.general.tab_size = self.tab_size;
        config.general.editor_line_number = self.editor_line_number;
        config.general.line_graphics = self.line_graphics;
        config.general.vertical_split = if self.vertical_split {
            "vertical".to_string()
        } else {
            "auto".to_string()
        };
        config.general.mouse = self.mouse;
        config.general.read_only = self.read_only;

        config.view.line_number = self.line_number;
        config.view.date = match self.date_format {
            DateFormat::Relative => "default".to_string(),
            other => other.as_str().to_string(),
        };
        config.view.author = self.author_format.as_str().to_string();
        config.view.committer = self.committer;
        config.view.commit_title_graph = match self.commit_title_graph {
            GraphDisplay::Auto => "v2".to_string(),
            other => other.as_str().to_string(),
        };
        config.view.file_name = if self.file_name {
            "always".to_string()
        } else {
            "no".to_string()
        };
        config.view.file_size = if self.file_size {
            "default".to_string()
        } else {
            "no".to_string()
        };
        config.view.commit_order = self.commit_order;
        config.view.commit_title_refs = self.commit_title_refs;
        config.view.ignore_space = self.ignore_space;
        config.view.show_changes = self.show_changes;
        config.view.show_untracked = self.show_untracked;
        config.view.id = self.commit_id;
        config.view.file_filter = self.file_filter;
        config.view.rev_filter = self.rev_filter;
        config.view.commit_title_overflow = self.commit_title_overflow.unwrap_or(0);
        config.view.status_show_untracked_dirs = self.status_show_untracked_dirs;

        config.view.diff_presentation = self.diff_presentation;
        config.view.diff_layout = self.diff_layout;
        config.view.diff_indicator = self.diff_indicator;
        config.view.word_diff_pairing = self.word_diff_pairing;
        config.view.word_diff = self.word_diff;
        config.view.side_by_side_min_width = self.side_by_side_min_width;
        config.view.diff_formatter.clone_from(&self.diff_formatter);
        config.view.diff_context_full_max_lines = self.diff_context_full_max_lines;
        config.view.syntax_highlighting = self.syntax_highlighting;
        config.view.syntax_theme.clone_from(&self.syntax_theme);
        config.view.ui_theme = self.ui_theme;
        config.view.diff_context = if self.diff_context == Self::DIFF_CONTEXT_FULL {
            "full".to_string()
        } else {
            self.diff_context.to_string()
        };
        config.view.wrap_lines = self.wrap_lines;
        config.view.color_moved = self.color_moved;
        config.view.diff_sticky_header = self.diff_sticky_header;
        config.view.diff_hints = self.diff_hints;
        config.view.diff_collapse_generated = self.diff_collapse_generated;
        config.view.main_author_color = self.main_author_color;
        config.view.main_spotlight = self.main_spotlight;
        config.view.main_spotlight_dim_others = self.main_spotlight_dim_others;
        config.view.main_dim_unreachable = self.main_dim_unreachable;
        config.view.main_push_status = self.main_push_status;
        config.view.main_date_heat = self.main_date_heat;
        config.view.main_subject_rules = self.main_subject_rules;
        config.view.main_unique_prefix = self.main_unique_prefix;
        config.view.main_dim_merges = self.main_dim_merges;
        config.author_colors.clone_from(&self.author_colors);
        config
            .main_subject_rules
            .clone_from(&self.custom_subject_rules);
        config.performance.memory_profile = self.memory_profile;
    }

    /// Adapts options according to negotiated terminal capabilities.
    #[must_use]
    pub fn with_capabilities(mut self, caps: &crate::term_cap::TerminalCapabilities) -> Self {
        if !caps.supports_unicode_box {
            self.line_graphics = LineGraphics::Ascii;
        }
        if caps.color_profile == crate::term_cap::ColorProfile::Monochrome {
            self.no_color = true;
        }
        self
    }

    /// Creates a new `ViewOptions` with default values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggles line number display on/off.
    pub fn toggle_line_number(&mut self) -> String {
        self.line_number = !self.line_number;
        format!(
            ":set line-number = {}",
            if self.line_number { "yes" } else { "no" }
        )
    }

    /// Cycles commit date display format.
    pub fn toggle_date(&mut self) -> String {
        self.date_format = self.date_format.next();
        format!(":set date = {}", self.date_format.as_str())
    }

    /// Cycles commit date display format backward.
    pub fn prev_date(&mut self) -> String {
        self.date_format = self.date_format.prev();
        format!(":set date = {}", self.date_format.as_str())
    }

    /// Cycles commit author display format.
    pub fn toggle_author(&mut self) -> String {
        self.author_format = self.author_format.next();
        format!(":set author = {}", self.author_format.as_str())
    }

    /// Cycles commit author display format backward.
    pub fn prev_author(&mut self) -> String {
        self.author_format = self.author_format.prev();
        format!(":set author = {}", self.author_format.as_str())
    }

    /// Toggles committer display on/off.
    pub fn toggle_committer(&mut self) -> String {
        self.committer = !self.committer;
        format!(
            ":set committer = {}",
            if self.committer { "yes" } else { "no" }
        )
    }

    /// Toggles line graphics between UTF-8 and ASCII.
    pub fn toggle_line_graphics(&mut self) -> String {
        self.line_graphics = self.line_graphics.next();
        format!(":set line-graphics = {}", self.line_graphics.as_str())
    }

    /// Cycles line graphics backward.
    pub fn prev_line_graphics(&mut self) -> String {
        self.line_graphics = self.line_graphics.prev();
        format!(":set line-graphics = {}", self.line_graphics.as_str())
    }

    /// Cycles revision graph display mode.
    pub fn toggle_commit_title_graph(&mut self) -> String {
        self.commit_title_graph = self.commit_title_graph.next();
        format!(
            ":set commit-title-graph = {}",
            self.commit_title_graph.as_str()
        )
    }

    /// Cycles revision graph display mode backward.
    pub fn prev_commit_title_graph(&mut self) -> String {
        self.commit_title_graph = self.commit_title_graph.prev();
        format!(
            ":set commit-title-graph = {}",
            self.commit_title_graph.as_str()
        )
    }

    /// Toggles file name display on/off.
    pub fn toggle_file_name(&mut self) -> String {
        self.file_name = !self.file_name;
        format!(
            ":set file-name = {}",
            if self.file_name { "yes" } else { "no" }
        )
    }

    /// Toggles file size display in tree view on/off.
    pub fn toggle_file_size(&mut self) -> String {
        self.file_size = !self.file_size;
        format!(
            ":set file-size = {}",
            if self.file_size { "yes" } else { "no" }
        )
    }

    /// Cycles whitespace ignore mode in diffs.
    pub fn toggle_ignore_space(&mut self) -> String {
        self.ignore_space = self.ignore_space.next();
        format!(":set ignore-space = {}", self.ignore_space.as_str())
    }

    /// Cycles whitespace ignore mode in diffs backward.
    pub fn prev_ignore_space(&mut self) -> String {
        self.ignore_space = self.ignore_space.prev();
        format!(":set ignore-space = {}", self.ignore_space.as_str())
    }

    /// Toggles word-level emphasis within modified diff lines.
    pub fn toggle_word_diff(&mut self) -> String {
        self.word_diff = !self.word_diff;
        format!(
            ":set word-diff = {}",
            if self.word_diff { "yes" } else { "no" }
        )
    }

    /// Cycles commit ordering in log/main view.
    pub fn toggle_commit_order(&mut self) -> String {
        self.commit_order = self.commit_order.next();
        format!(":set commit-order = {}", self.commit_order.as_str())
    }

    /// Cycles commit ordering in log/main view backward.
    pub fn prev_commit_order(&mut self) -> String {
        self.commit_order = self.commit_order.prev();
        format!(":set commit-order = {}", self.commit_order.as_str())
    }

    /// Toggles ref label display `[main]`, `[v1.0]`.
    pub fn toggle_commit_title_refs(&mut self) -> String {
        self.commit_title_refs = !self.commit_title_refs;
        format!(
            ":set commit-title-refs = {}",
            if self.commit_title_refs { "yes" } else { "no" }
        )
    }

    /// Toggles uncommitted local changes display.
    pub fn toggle_show_changes(&mut self) -> String {
        self.show_changes = !self.show_changes;
        format!(
            ":set show-changes = {}",
            if self.show_changes { "yes" } else { "no" }
        )
    }

    /// Toggles untracked files within the main view's local changes display.
    pub fn toggle_show_untracked(&mut self) -> String {
        self.show_untracked = !self.show_untracked;
        format!(
            ":set show-untracked = {}",
            if self.show_untracked { "yes" } else { "no" }
        )
    }

    /// Toggles commit SHA ID display in main view.
    pub fn toggle_id(&mut self) -> String {
        self.commit_id = !self.commit_id;
        format!(":set id = {}", if self.commit_id { "yes" } else { "no" })
    }

    /// Toggles file filtering mode.
    pub fn toggle_file_filter(&mut self) -> String {
        self.file_filter = !self.file_filter;
        format!(
            ":set file-filter = {}",
            if self.file_filter { "yes" } else { "no" }
        )
    }

    /// Toggles revision filtering mode.
    pub fn toggle_rev_filter(&mut self) -> String {
        self.rev_filter = !self.rev_filter;
        format!(
            ":set rev-filter = {}",
            if self.rev_filter { "yes" } else { "no" }
        )
    }

    /// Toggles commit title overflow threshold between 50 and off.
    pub fn toggle_commit_title_overflow(&mut self) -> String {
        self.commit_title_overflow = match self.commit_title_overflow {
            None => Some(50),
            Some(_) => None,
        };
        format!(
            ":set commit-title-overflow = {}",
            match self.commit_title_overflow {
                Some(limit) => limit.to_string(),
                None => "no".to_string(),
            }
        )
    }

    /// Toggles untracked directory display in status view.
    pub fn toggle_status_show_untracked_dirs(&mut self) -> String {
        self.status_show_untracked_dirs = !self.status_show_untracked_dirs;
        format!(
            ":set status-show-untracked-dirs = {}",
            if self.status_show_untracked_dirs {
                "yes"
            } else {
                "no"
            }
        )
    }

    /// Toggles vertical vs horizontal dual view split.
    pub fn toggle_vertical_split(&mut self) -> String {
        self.vertical_split = !self.vertical_split;
        format!(
            ":set vertical-split = {}",
            if self.vertical_split { "yes" } else { "no" }
        )
    }

    /// Modifies diff context lines by `delta` (bounded at >= 0, up to full).
    pub fn toggle_diff_context(&mut self, delta: i8) -> String {
        if delta > 0 {
            if self.diff_context == Self::DIFF_CONTEXT_FULL {
                // already at full
            } else if self.diff_context >= 20 {
                self.diff_context = Self::DIFF_CONTEXT_FULL;
            } else {
                let current = self.diff_context as i64;
                self.diff_context = (current + i64::from(delta)).max(0) as usize;
            }
        } else if delta < 0 {
            if self.diff_context == Self::DIFF_CONTEXT_FULL {
                self.diff_context = 20;
            } else {
                let current = self.diff_context as i64;
                self.diff_context = (current + i64::from(delta)).max(0) as usize;
            }
        }
        let ctx_str = if self.diff_context == Self::DIFF_CONTEXT_FULL {
            "full".to_string()
        } else {
            self.diff_context.to_string()
        };
        format!(":set diff-context = {ctx_str}")
    }

    /// Toggles diff presentation between classic and fancy.
    pub fn toggle_diff_presentation(&mut self) -> String {
        self.diff_presentation = self.diff_presentation.next();
        format!(
            ":set diff-presentation = {}",
            self.diff_presentation.as_str()
        )
    }

    /// Cycles diff presentation backward.
    pub fn prev_diff_presentation(&mut self) -> String {
        self.diff_presentation = self.diff_presentation.prev();
        format!(
            ":set diff-presentation = {}",
            self.diff_presentation.as_str()
        )
    }

    /// Toggles diff layout between unified and side-by-side.
    pub fn toggle_diff_layout(&mut self) -> String {
        self.diff_layout = self.diff_layout.next();
        format!(":set diff-layout = {}", self.diff_layout.as_str())
    }

    /// Cycles diff layout backward.
    pub fn prev_diff_layout(&mut self) -> String {
        self.diff_layout = self.diff_layout.prev();
        format!(":set diff-layout = {}", self.diff_layout.as_str())
    }

    /// Cycles diff indicator setting (auto -> yes -> no).
    pub fn toggle_diff_indicator(&mut self) -> String {
        self.diff_indicator = self.diff_indicator.next();
        format!(":set diff-indicator = {}", self.diff_indicator.as_str())
    }

    /// Cycles diff indicator setting backward.
    pub fn prev_diff_indicator(&mut self) -> String {
        self.diff_indicator = self.diff_indicator.prev();
        format!(":set diff-indicator = {}", self.diff_indicator.as_str())
    }

    /// Toggles word diff line pairing between similarity and positional.
    pub fn toggle_word_diff_pairing(&mut self) -> String {
        self.word_diff_pairing = self.word_diff_pairing.next();
        format!(
            ":set word-diff-pairing = {}",
            self.word_diff_pairing.as_str()
        )
    }

    /// Cycles word diff line pairing backward.
    pub fn prev_word_diff_pairing(&mut self) -> String {
        self.word_diff_pairing = self.word_diff_pairing.prev();
        format!(
            ":set word-diff-pairing = {}",
            self.word_diff_pairing.as_str()
        )
    }

    /// Toggles mouse tracking support on/off.
    pub fn toggle_mouse(&mut self) -> String {
        self.mouse = !self.mouse;
        format!(":set mouse = {}", if self.mouse { "yes" } else { "no" })
    }

    /// Toggles code syntax highlighting support on/off.
    pub fn toggle_syntax_highlighting(&mut self) -> String {
        self.syntax_highlighting = !self.syntax_highlighting;
        format!(
            ":set syntax-highlighting = {}",
            if self.syntax_highlighting {
                "yes"
            } else {
                "no"
            }
        )
    }

    /// Cycles to the next available syntax highlighting theme.
    pub fn cycle_syntax_theme(&mut self) -> String {
        let next = crate::highlight::next_theme_name(&self.syntax_theme);
        self.syntax_theme = next.to_string();
        let themes = crate::highlight::available_theme_names();
        let pos = themes.iter().position(|&t| t == next).map_or(1, |i| i + 1);
        format!(
            ":set syntax-theme = {} ({}/{})",
            self.syntax_theme,
            pos,
            themes.len()
        )
    }

    /// Cycles to the previous available syntax highlighting theme.
    pub fn prev_syntax_theme(&mut self) -> String {
        let themes = crate::highlight::available_theme_names();
        let cur_pos = themes
            .iter()
            .position(|&t| t.eq_ignore_ascii_case(&self.syntax_theme))
            .unwrap_or(0);
        let prev_pos = if cur_pos == 0 {
            themes.len().saturating_sub(1)
        } else {
            cur_pos - 1
        };
        if let Some(&prev_name) = themes.get(prev_pos) {
            self.syntax_theme = prev_name.to_string();
        }
        format!(
            ":set syntax-theme = {} ({}/{})",
            self.syntax_theme,
            prev_pos + 1,
            themes.len()
        )
    }

    /// Cycles to the next built-in UI color theme across Adaptive, Dark, Light, and High-Contrast families,
    /// synchronizing `syntax_theme` to the theme's paired default.
    pub fn cycle_ui_theme(&mut self) -> String {
        self.ui_theme = self.ui_theme.next();
        self.syntax_theme =
            crate::highlight::canonical_theme_name(self.ui_theme.paired_syntax_theme()).to_string();
        format!(
            ":set ui-theme = {} ({} {}/{}, syntax: {})",
            self.ui_theme.as_str(),
            self.ui_theme.family_label(),
            self.ui_theme.position(),
            UiThemeId::ALL.len(),
            self.syntax_theme,
        )
    }

    /// Cycles to the previous built-in UI color theme across Adaptive, Dark, Light, and High-Contrast families,
    /// synchronizing `syntax_theme` to the theme's paired default.
    pub fn prev_ui_theme(&mut self) -> String {
        self.ui_theme = self.ui_theme.prev();
        self.syntax_theme =
            crate::highlight::canonical_theme_name(self.ui_theme.paired_syntax_theme()).to_string();
        format!(
            ":set ui-theme = {} ({} {}/{}, syntax: {})",
            self.ui_theme.as_str(),
            self.ui_theme.family_label(),
            self.ui_theme.position(),
            UiThemeId::ALL.len(),
            self.syntax_theme,
        )
    }

    /// Toggles read-only protection mode on/off.
    pub fn toggle_read_only(&mut self) -> String {
        self.read_only = !self.read_only;
        if self.read_only {
            ":set read-only = true (Read-Only Mode enabled: repository modifications blocked)"
                .to_string()
        } else {
            ":set read-only = false (Update Mode enabled: repository modifications allowed)"
                .to_string()
        }
    }

    /// Toggles soft line wrapping in diff view on/off.
    pub fn toggle_wrap_lines(&mut self) -> String {
        self.wrap_lines = !self.wrap_lines;
        format!(
            ":set wrap-lines = {}",
            if self.wrap_lines { "yes" } else { "no" }
        )
    }

    /// Toggles moved code block detection and highlighting on/off.
    pub fn toggle_color_moved(&mut self) -> String {
        self.color_moved = !self.color_moved;
        format!(
            ":set color-moved = {}",
            if self.color_moved { "yes" } else { "no" }
        )
    }

    /// Toggles adaptive sticky file and function header in diff view on/off.
    pub fn toggle_diff_sticky_header(&mut self) -> String {
        self.diff_sticky_header = !self.diff_sticky_header;
        format!(
            ":set diff-sticky-header = {}",
            if self.diff_sticky_header { "yes" } else { "no" }
        )
    }

    /// Cycles contextual action hint chips display mode in diff file banners.
    pub fn toggle_diff_hints(&mut self) -> String {
        self.diff_hints = self.diff_hints.next();
        format!(":set diff-hints = {}", self.diff_hints.as_str())
    }

    /// Cycles contextual action hint chips display mode backward.
    pub fn prev_diff_hints(&mut self) -> String {
        self.diff_hints = self.diff_hints.prev();
        format!(":set diff-hints = {}", self.diff_hints.as_str())
    }

    /// Toggles automatic collapsing of files marked `linguist-generated` in `.gitattributes`.
    pub fn toggle_diff_collapse_generated(&mut self) -> String {
        self.diff_collapse_generated = !self.diff_collapse_generated;
        format!(
            ":set diff-collapse-generated = {}",
            if self.diff_collapse_generated {
                "yes"
            } else {
                "no"
            }
        )
    }

    /// Cycles main view author column coloring mode (`hash` -> `me` -> `off`).
    pub fn toggle_main_author_color(&mut self) -> String {
        self.main_author_color = self.main_author_color.next();
        format!(
            ":set main-author-color = {}",
            self.main_author_color.as_str()
        )
    }

    /// Cycles main view author column coloring mode backward.
    pub fn prev_main_author_color(&mut self) -> String {
        self.main_author_color = self.main_author_color.prev();
        format!(
            ":set main-author-color = {}",
            self.main_author_color.as_str()
        )
    }

    /// Cycles main view spotlight mode (`off` -> `author` -> `ancestry`).
    pub fn toggle_main_spotlight(&mut self) -> String {
        self.main_spotlight = self.main_spotlight.next();
        format!(":set main-spotlight = {}", self.main_spotlight.as_str())
    }

    /// Cycles main view spotlight mode backward.
    pub fn prev_main_spotlight(&mut self) -> String {
        self.main_spotlight = self.main_spotlight.prev();
        format!(":set main-spotlight = {}", self.main_spotlight.as_str())
    }

    /// Toggles dimming of non-matching rows when a spotlight is active.
    pub fn toggle_main_spotlight_dim_others(&mut self) -> String {
        self.main_spotlight_dim_others = !self.main_spotlight_dim_others;
        format!(
            ":set main-spotlight-dim-others = {}",
            if self.main_spotlight_dim_others {
                "yes"
            } else {
                "no"
            }
        )
    }

    /// Toggles dimming of commits not reachable from `HEAD`.
    pub fn toggle_main_dim_unreachable(&mut self) -> String {
        self.main_dim_unreachable = !self.main_dim_unreachable;
        format!(
            ":set main-dim-unreachable = {}",
            if self.main_dim_unreachable {
                "yes"
            } else {
                "no"
            }
        )
    }

    /// Toggles push/upstream status SHA coloring (`↑` ahead / unmerged).
    pub fn toggle_main_push_status(&mut self) -> String {
        self.main_push_status = !self.main_push_status;
        format!(
            ":set main-push-status = {}",
            if self.main_push_status { "yes" } else { "no" }
        )
    }

    /// Toggles the 6-bucket age heatmap gradient on the Date column.
    pub fn toggle_main_date_heat(&mut self) -> String {
        self.main_date_heat = !self.main_date_heat;
        format!(
            ":set main-date-heat = {}",
            if self.main_date_heat { "yes" } else { "no" }
        )
    }

    /// Toggles semantic pattern rules on commit subjects (`fixup!`, `Revert`, `WIP`, `#123`).
    pub fn toggle_main_subject_rules(&mut self) -> String {
        self.main_subject_rules = !self.main_subject_rules;
        format!(
            ":set main-subject-rules = {}",
            if self.main_subject_rules { "yes" } else { "no" }
        )
    }

    /// Toggles bolding of the shortest unique SHA prefix when the `id` column is shown.
    pub fn toggle_main_unique_prefix(&mut self) -> String {
        self.main_unique_prefix = !self.main_unique_prefix;
        format!(
            ":set main-unique-prefix = {}",
            if self.main_unique_prefix { "yes" } else { "no" }
        )
    }

    /// Toggles dimming of merge commit subject boilerplate.
    pub fn toggle_main_dim_merges(&mut self) -> String {
        self.main_dim_merges = !self.main_dim_merges;
        format!(
            ":set main-dim-merges = {}",
            if self.main_dim_merges { "yes" } else { "no" }
        )
    }
}

/// Converts timestamp in seconds to formatted date string (`YYYY-MM-DD` or `YYYY-MM-DD HH:MM`).
///
/// Delegates to [`tigrs_git::format_timestamp`], which utilizes `gix::date::Time`.
#[must_use]
pub fn format_timestamp(secs: i64, iso: bool) -> String {
    tigrs_git::format_timestamp(secs, iso)
}

/// Formats an author's name into an abbreviated form (e.g. `"L. Torvalds"`).
#[must_use]
pub fn abbreviate_author(name: &str) -> String {
    let trimmed = name.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() <= 1 {
        return trimmed.to_string();
    }
    let Some(&last) = parts.last() else {
        return trimmed.to_string();
    };
    let initials: Vec<String> = parts[..parts.len() - 1]
        .iter()
        .filter_map(|p| p.chars().next().map(|c| format!("{c}.")))
        .collect();

    format!("{} {last}", initials.join(" "))
}

/// Extracts email username before `@` if present.
#[must_use]
pub fn author_email_prefix(name_or_email: &str) -> String {
    let trimmed = name_or_email.trim();
    if let Some(pos) = trimmed.find('@') {
        trimmed[..pos].to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_toggle_menu_items_present() {
        assert_eq!(TOGGLE_MENU_ITEMS.len(), 44);
        let hotkeys: Vec<char> = TOGGLE_MENU_ITEMS.iter().map(|it| it.hotkey).collect();
        for ch in [
            '.', 'D', 'S', 'H', 'A', 'T', '~', 'g', '#', '*', 'W', 'w', 'l', 'F', 'C', 'u', 'X',
            '%', '^', '$', 'd', '|', 'y', 's', 'c', 'm', 't', 'L', 'M', 'K', 'B', 'G', 'a', '&',
            'U', '+', '=', '!', '_', '?', '@', ';', 'x', 'z',
        ] {
            assert!(hotkeys.contains(&ch), "missing menu hotkey {ch:?}");
        }

        // Every OptionId in OPTIONS_REGISTRY must appear in TOGGLE_MENU_ITEMS.
        for desc in OPTIONS_REGISTRY {
            assert!(
                TOGGLE_MENU_ITEMS
                    .iter()
                    .any(|it| it.action == MenuAction::Toggle(desc.id)),
                "OptionId {:?} missing from TOGGLE_MENU_ITEMS",
                desc.id
            );
        }

        // Hotkeys must be unique for the menu's single-key lookup to be unambiguous.
        let mut sorted = hotkeys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), hotkeys.len(), "duplicate menu hotkey");
    }

    #[test]
    fn test_option_menu_navigation_and_hotkeys() {
        let mut menu = OptionMenuState::new();
        assert_eq!(menu.selected, 0);
        assert_eq!(menu.current_item().hotkey, 't');

        menu.next();
        assert_eq!(menu.selected, 1);
        assert_eq!(menu.current_item().hotkey, 'H');

        menu.prev();
        assert_eq!(menu.selected, 0);

        // Wrap around backward
        menu.prev();
        assert_eq!(menu.selected, TOGGLE_MENU_ITEMS.len() - 1);
        assert_eq!(menu.current_item().hotkey, '*');

        // Wrap around forward
        menu.next();
        assert_eq!(menu.selected, 0);

        // Hotkey lookup
        assert_eq!(OptionMenuState::find_hotkey('W'), Some(13));
        assert!(OptionMenuState::find_hotkey('y').is_some());
        assert!(OptionMenuState::find_hotkey('s').is_some());
        assert!(OptionMenuState::find_hotkey('c').is_some());
        assert!(OptionMenuState::find_hotkey('m').is_some());
        assert!(OptionMenuState::find_hotkey('S').is_some());
        assert!(OptionMenuState::find_hotkey('H').is_some());
        assert!(OptionMenuState::find_hotkey('t').is_some());
        assert!(OptionMenuState::find_hotkey('L').is_some());
        assert!(OptionMenuState::find_hotkey('M').is_some());
        assert_eq!(OptionMenuState::find_hotkey('Z'), None);

        // Render line
        let line = menu.render_line(50);
        assert!(line.contains("Toggle option UI color theme [t]"));
        assert!(line.ends_with(&format!("(1 of {})", TOGGLE_MENU_ITEMS.len())));
    }

    #[test]
    fn test_toggle_all_options() {
        let mut opts = ViewOptions::new();

        // 1. line-number
        assert!(!opts.line_number);
        assert_eq!(opts.toggle_line_number(), ":set line-number = yes");
        assert!(opts.line_number);
        assert_eq!(opts.toggle_line_number(), ":set line-number = no");
        assert!(!opts.line_number);

        // 2. date
        assert_eq!(opts.date_format, DateFormat::Relative);
        assert_eq!(opts.toggle_date(), ":set date = short");
        assert_eq!(opts.date_format, DateFormat::Short);
        assert_eq!(opts.toggle_date(), ":set date = iso");
        assert_eq!(opts.toggle_date(), ":set date = no");
        assert_eq!(opts.toggle_date(), ":set date = relative");

        // 3. author
        assert_eq!(opts.author_format, AuthorFormat::Full);
        assert_eq!(opts.toggle_author(), ":set author = abbreviated");
        assert_eq!(opts.toggle_author(), ":set author = email");
        assert_eq!(opts.toggle_author(), ":set author = no");
        assert_eq!(opts.toggle_author(), ":set author = full");

        // 4. committer
        assert_eq!(opts.toggle_committer(), ":set committer = yes");
        assert!(opts.committer);

        // 5. line-graphics
        assert_eq!(opts.line_graphics, LineGraphics::Utf8);
        assert_eq!(opts.toggle_line_graphics(), ":set line-graphics = ascii");
        assert_eq!(opts.toggle_line_graphics(), ":set line-graphics = utf-8");

        // 6. commit-title-graph
        assert_eq!(opts.commit_title_graph, GraphDisplay::Auto);
        assert_eq!(
            opts.toggle_commit_title_graph(),
            ":set commit-title-graph = v2"
        );
        assert_eq!(
            opts.toggle_commit_title_graph(),
            ":set commit-title-graph = no"
        );
        assert_eq!(
            opts.toggle_commit_title_graph(),
            ":set commit-title-graph = auto"
        );

        // 7. file-name
        assert_eq!(opts.toggle_file_name(), ":set file-name = no");

        // 8. file-size
        assert_eq!(opts.toggle_file_size(), ":set file-size = no");

        // 9. ignore-space
        assert_eq!(opts.ignore_space, IgnoreSpace::No);
        assert_eq!(opts.toggle_ignore_space(), ":set ignore-space = all");
        assert_eq!(opts.toggle_ignore_space(), ":set ignore-space = some");
        assert_eq!(opts.toggle_ignore_space(), ":set ignore-space = at-eol");
        assert_eq!(opts.toggle_ignore_space(), ":set ignore-space = no");

        // 10. commit-order
        assert_eq!(opts.commit_order, CommitOrder::Default);
        assert_eq!(opts.toggle_commit_order(), ":set commit-order = topo");
        assert_eq!(opts.toggle_commit_order(), ":set commit-order = date");
        assert_eq!(
            opts.toggle_commit_order(),
            ":set commit-order = author-date"
        );
        assert_eq!(opts.toggle_commit_order(), ":set commit-order = reverse");
        assert_eq!(opts.toggle_commit_order(), ":set commit-order = default");

        // 11. commit-title-refs
        assert_eq!(
            opts.toggle_commit_title_refs(),
            ":set commit-title-refs = no"
        );

        // 12. show-changes
        assert_eq!(opts.toggle_show_changes(), ":set show-changes = no");

        // 13. id
        assert_eq!(opts.toggle_id(), ":set id = yes");

        // 14. file-filter
        assert_eq!(opts.toggle_file_filter(), ":set file-filter = yes");

        // 15. rev-filter
        assert_eq!(opts.toggle_rev_filter(), ":set rev-filter = yes");

        // 16. commit-title-overflow
        assert_eq!(opts.commit_title_overflow, None);
        assert_eq!(
            opts.toggle_commit_title_overflow(),
            ":set commit-title-overflow = 50"
        );
        assert_eq!(opts.commit_title_overflow, Some(50));
        assert_eq!(
            opts.toggle_commit_title_overflow(),
            ":set commit-title-overflow = no"
        );

        // 17. status-show-untracked-dirs
        assert_eq!(
            opts.toggle_status_show_untracked_dirs(),
            ":set status-show-untracked-dirs = no"
        );

        // 18. vertical-split
        assert_eq!(opts.toggle_vertical_split(), ":set vertical-split = yes");

        // 19. diff-context
        assert_eq!(opts.diff_context, 3);
        assert_eq!(opts.toggle_diff_context(1), ":set diff-context = 4");
        assert_eq!(opts.toggle_diff_context(-2), ":set diff-context = 2");
        opts.diff_context = 20;
        assert_eq!(opts.toggle_diff_context(1), ":set diff-context = full");
        assert!(opts.is_full_context());
        assert_eq!(opts.toggle_diff_context(-1), ":set diff-context = 20");
        assert!(!opts.is_full_context());

        // 20. diff-presentation
        assert_eq!(opts.diff_presentation, DiffPresentation::Banner);
        assert_eq!(
            opts.toggle_diff_presentation(),
            ":set diff-presentation = fancy"
        );
        assert_eq!(opts.diff_presentation, DiffPresentation::Fancy);
        assert_eq!(
            opts.toggle_diff_presentation(),
            ":set diff-presentation = classic"
        );
        assert_eq!(opts.diff_presentation, DiffPresentation::Classic);
        assert_eq!(
            opts.toggle_diff_presentation(),
            ":set diff-presentation = banner"
        );
        assert_eq!(opts.diff_presentation, DiffPresentation::Banner);

        // 21. diff-layout
        assert_eq!(opts.diff_layout, DiffLayout::Unified);
        assert_eq!(opts.toggle_diff_layout(), ":set diff-layout = side-by-side");
        assert_eq!(opts.diff_layout, DiffLayout::SideBySide);
        assert_eq!(opts.toggle_diff_layout(), ":set diff-layout = unified");

        // 22. diff-indicator
        assert_eq!(opts.diff_indicator, DiffIndicator::Auto);
        assert_eq!(opts.toggle_diff_indicator(), ":set diff-indicator = yes");
        assert_eq!(opts.diff_indicator, DiffIndicator::Yes);
        assert_eq!(opts.toggle_diff_indicator(), ":set diff-indicator = no");
        assert_eq!(opts.diff_indicator, DiffIndicator::No);
        assert_eq!(opts.toggle_diff_indicator(), ":set diff-indicator = auto");

        // 23. word-diff-pairing
        assert_eq!(opts.word_diff_pairing, WordDiffPairing::Similarity);
        assert_eq!(
            opts.toggle_word_diff_pairing(),
            ":set word-diff-pairing = positional"
        );
        assert_eq!(opts.word_diff_pairing, WordDiffPairing::Positional);
        assert_eq!(
            opts.toggle_word_diff_pairing(),
            ":set word-diff-pairing = similarity"
        );

        // 24. mouse
        assert!(!opts.mouse);
        assert_eq!(opts.toggle_mouse(), ":set mouse = yes");
        assert!(opts.mouse);
        assert_eq!(opts.toggle_mouse(), ":set mouse = no");
        assert!(!opts.mouse);
    }

    #[test]
    fn test_toggle_by_name() {
        let mut opts = ViewOptions::new();
        assert_eq!(
            opts.toggle_by_name("line-number").unwrap(),
            ":set line-number = yes"
        );
        assert_eq!(
            opts.toggle_by_name("ignore-space").unwrap(),
            ":set ignore-space = all"
        );
        assert_eq!(
            opts.toggle_by_name("vertical-split").unwrap(),
            ":set vertical-split = yes"
        );
        assert_eq!(
            opts.toggle_by_name("diff-presentation").unwrap(),
            ":set diff-presentation = fancy"
        );
        assert_eq!(
            opts.toggle_by_name("diff-layout").unwrap(),
            ":set diff-layout = side-by-side"
        );
        assert_eq!(
            opts.toggle_by_name("diff-context").unwrap(),
            ":set diff-context = 4"
        );
        assert_eq!(
            opts.toggle_by_name("diff-indicator").unwrap(),
            ":set diff-indicator = yes"
        );
        assert_eq!(
            opts.toggle_by_name("word-diff-pairing").unwrap(),
            ":set word-diff-pairing = positional"
        );
        assert_eq!(opts.toggle_by_name("mouse").unwrap(), ":set mouse = yes");
        assert!(opts.toggle_by_name("invalid-opt").is_err());
    }

    #[test]
    fn test_set_by_name_vertical_split_and_common_options() {
        let mut opts = ViewOptions::default();
        assert!(!opts.vertical_split);

        // vertical-split yes/true/vertical
        let (msg, eff) = opts.set_by_name("vertical-split", "yes").unwrap();
        assert_eq!(msg, ":set vertical-split = yes");
        assert_eq!(eff, OptionEffect::None);
        assert!(opts.vertical_split);

        let (msg, eff) = opts.set_by_name("vertical-split", "horizontal").unwrap();
        assert_eq!(msg, ":set vertical-split = no");
        assert_eq!(eff, OptionEffect::None);
        assert!(!opts.vertical_split);

        // line-number
        let (msg, _) = opts.set_by_name("line-number", "yes").unwrap();
        assert_eq!(msg, ":set line-number = yes");
        assert!(opts.line_number);

        // line-graphics
        let (msg, _) = opts.set_by_name("line-graphics", "ascii").unwrap();
        assert_eq!(msg, ":set line-graphics = ascii");
        assert_eq!(opts.line_graphics, LineGraphics::Ascii);

        // commit-order
        let (msg, _) = opts.set_by_name("commit-order", "topo").unwrap();
        assert_eq!(msg, ":set commit-order = topo");
        assert_eq!(opts.commit_order, CommitOrder::Topo);

        // ignore-space
        let (msg, eff) = opts.set_by_name("ignore-space", "all").unwrap();
        assert_eq!(msg, ":set ignore-space = all");
        assert_eq!(eff, OptionEffect::RefreshDiff);
        assert_eq!(opts.ignore_space, IgnoreSpace::All);

        // tab-size
        let (msg, _) = opts.set_by_name("tab-size", "4").unwrap();
        assert_eq!(msg, ":set tab-size = 4");
        assert_eq!(opts.tab_size, 4);
    }

    #[test]
    fn test_date_formatting() {
        // Unix epoch timestamp: 1726214745 (2024-09-13 08:05:45)
        let short = format_timestamp(1_726_214_745, false);
        assert_eq!(short, "2024-09-13");
        let iso = format_timestamp(1_726_214_745, true);
        assert_eq!(iso, "2024-09-13 08:05");
    }

    #[test]
    fn test_author_helpers() {
        assert_eq!(abbreviate_author("Linus Torvalds"), "L. Torvalds");
        assert_eq!(abbreviate_author("John Michael Smith"), "J. M. Smith");
        assert_eq!(abbreviate_author("Alice"), "Alice");

        assert_eq!(author_email_prefix("torvalds@linux.org"), "torvalds");
        assert_eq!(author_email_prefix("Linus Torvalds"), "Linus Torvalds");
    }

    #[test]
    fn test_options_panel_bidirectional_cycling_and_toml_persistence() {
        let mut opts = ViewOptions::default();
        let default_baseline = ViewOptions::default();

        // 1. Bidirectional enum cycling (next + prev) restores initial state for every OptionId
        for desc in OPTIONS_REGISTRY {
            let initial_val = opts.option_value_string(desc.id);
            let _ = opts.toggle_by_id(desc.id);
            assert_ne!(
                opts.option_value_string(desc.id),
                initial_val,
                "toggle should change {}",
                desc.canonical_name
            );
            let _ = opts.prev_by_id(desc.id);
            assert_eq!(
                opts.option_value_string(desc.id),
                initial_val,
                "prev should restore {}",
                desc.canonical_name
            );
        }

        // 2. Modify several options, write into Config, save to a temporary local .toml file
        opts.toggle_line_number();
        opts.toggle_diff_layout();
        opts.toggle_date();
        opts.toggle_mouse();
        assert_eq!(opts.count_modified_from(&default_baseline), 4);

        let dir = tempfile::tempdir().unwrap();
        let custom_toml_path = dir.path().join("custom_user_config.toml");

        // Pre-populate existing [security] and [keybindings] to verify surgical preservation
        std::fs::write(
            &custom_toml_path,
            "[security]\ntrust_external_hooks = true\n\n[keybindings.generic]\n\"Q\" = \"quit\"\n",
        )
        .unwrap();

        let mut cfg = tigrs_core::Config::load_from_file(&custom_toml_path).unwrap();
        opts.write_into_config(&mut cfg);
        let saved_count = cfg
            .save_to_path(&tigrs_core::Config::default(), &custom_toml_path, true)
            .unwrap();
        assert!(saved_count >= 4);

        // 3. Reload the saved TOML file and verify both modified options AND preserved sections
        let reloaded_cfg = tigrs_core::Config::load_from_file(&custom_toml_path).unwrap();
        assert!(reloaded_cfg.security.trust_external_hooks);
        assert_eq!(
            reloaded_cfg
                .keybindings
                .get("generic.Q")
                .map(String::as_str),
            Some("quit")
        );

        let mut reloaded_opts = ViewOptions::default();
        reloaded_opts.apply_config(&reloaded_cfg);
        assert!(reloaded_opts.line_number);
        assert_eq!(reloaded_opts.diff_layout, DiffLayout::SideBySide);
        assert_eq!(reloaded_opts.date_format, DateFormat::Short);
        assert!(reloaded_opts.mouse);

        // 4. Verify OptionMenuState category tabs and panel rendering
        let mut menu = OptionMenuState::new();
        menu.set_category_tab(2); // Diff & Code
        assert_eq!(menu.category, Some(OptionCategory::Diff));
        assert_eq!(menu.visible_indices().len(), 13);

        let lines = menu.render_panel_lines(
            &reloaded_opts,
            &default_baseline,
            &default_baseline,
            Some(&custom_toml_path),
            120,
            12,
        );
        assert!(lines.len() >= 6);
        assert!(lines[0].contains("Options & Config"));
        assert!(lines[0].contains(" 2:Diff & Code (13) "));
        let joined = lines.join("\n");
        assert!(joined.contains("Diff Layout"));
        assert!(joined.contains("Ignore Whitespace"));
        assert!(!joined.contains("(ignore-space)"));
        assert!(joined.contains(":set diff-layout = side-by-side"));
        assert!(joined.contains("TOML: [view].diff_layout"));
    }

    #[test]
    fn test_ui_theme_cycling_set_and_toml_persistence() {
        let mut opts = ViewOptions::default();
        assert_eq!(opts.ui_theme, UiThemeId::Default);

        // 1. Cycle through all 14 built-in themes and verify paired syntax theme + palette
        for &expected in &UiThemeId::ALL[1..] {
            let msg = opts.cycle_ui_theme();
            assert_eq!(opts.ui_theme, expected);
            assert!(msg.contains(expected.as_str()));
            assert!(msg.contains(expected.family_label()));
            let palette = opts.ui_palette();
            assert!(palette.canvas_bg.is_some());
            assert!(palette.text_fg.is_some());
            assert!(palette.title_bar_active.bg.is_some());
            assert!(palette.status_bar.bg.is_some());
        }
        // One more cycle wraps back to Default
        opts.cycle_ui_theme();
        assert_eq!(opts.ui_theme, UiThemeId::Default);

        // 2. `:set ui-theme ?` lists available themes
        let (list_msg, eff) = opts.set_by_name("ui-theme", "?").unwrap();
        assert_eq!(eff, OptionEffect::None);
        assert!(list_msg.contains("catppuccin-mocha"));
        assert!(list_msg.contains("catppuccin-latte"));
        assert!(list_msg.contains("high-contrast-dark"));
        assert!(list_msg.contains("high-contrast-light"));

        // 3. `:set ui-theme = catppuccin-latte` sets Light theme and pairs syntax theme
        let (set_msg, eff) = opts.set_by_name("ui-theme", "catppuccin-latte").unwrap();
        assert_eq!(eff, OptionEffect::RefreshSyntax);
        assert_eq!(opts.ui_theme, UiThemeId::CatppuccinLatte);
        assert!(opts.ui_theme.is_light());
        assert_eq!(opts.syntax_theme, "Solarized (light)");
        assert!(set_msg.contains("catppuccin-latte (Light)"));

        // 4. Persist to config.toml and reload
        let dir = tempfile::tempdir().unwrap();
        let toml_path = dir.path().join("theme_config.toml");
        let mut cfg = tigrs_core::Config::default();
        opts.write_into_config(&mut cfg);
        cfg.save_to_path(&tigrs_core::Config::default(), &toml_path, true)
            .unwrap();
        let raw_toml = std::fs::read_to_string(&toml_path).unwrap();
        assert!(raw_toml.contains("ui_theme = \"catppuccin-latte\""));

        let reloaded_cfg = tigrs_core::Config::load_from_file(&toml_path).unwrap();
        assert_eq!(reloaded_cfg.view.ui_theme, UiThemeId::CatppuccinLatte);
        let mut reloaded_opts = ViewOptions::default();
        reloaded_opts.apply_config(&reloaded_cfg);
        assert_eq!(reloaded_opts.ui_theme, UiThemeId::CatppuccinLatte);
        assert_eq!(reloaded_opts.syntax_theme, "Solarized (light)");
    }
}
