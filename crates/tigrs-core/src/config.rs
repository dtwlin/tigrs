// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Structured TOML configuration loader and runtime settings for tigrs.
//!
//! Handles:
//! - Default location: `$XDG_CONFIG_HOME/tigrs/config.toml` or `~/.config/tigrs/config.toml`.
//! - Explicit `-c/--config` path override.
//! - Flat and nested `[keybindings]` maps (`generic.q = "quit"` or `[keybindings.generic] q = "quit"`).
//! - Color declarations (`[colors] "cursor" = "white blue bold"` or `[colors."cursor"] fg = "white"`).
//! - Deserialization into typed view options and security policies.

use crate::error::{Result, TigError};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use crate::config_enums::{
    CommitOrder, DiffHintsMode, DiffIndicator, DiffLayout, DiffPresentation, IgnoreSpace,
    LineGraphics, MainAuthorColor, MainSpotlight, MemoryProfile, UiThemeId, WordDiffPairing,
};

/// Custom regex subject-highlighting rule (`[[main.subject-rules]]`).
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MainSubjectRuleConfig {
    /// Regular expression pattern matched against commit subjects.
    pub pattern: String,
    /// Foreground color name or `#rrggbb` hex string.
    pub fg: String,
    /// Render matching subjects in bold.
    pub bold: bool,
    /// Render matching subjects dimmed.
    pub dim: bool,
    /// Render matching subjects in italic.
    pub italic: bool,
    /// Render matching subjects underlined.
    pub underline: bool,
}

/// Primary configuration structure deserialized from `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// General UI, scrolling, and history settings.
    pub general: GeneralConfig,
    /// View layout and commit order settings.
    pub view: ViewConfig,
    /// Performance and memory-for-speed caching settings.
    pub performance: PerformanceConfig,
    /// Security rules for untrusted repositories.
    pub security: SecuritySettings,
    /// Keybindings mapping `scope.key` to an action string.
    pub keybindings: IndexMap<String, String>,
    /// Color customizations mapping area names to color definitions.
    pub colors: IndexMap<String, ColorSpec>,
    /// Per-author color overrides (`[colors.authors]` mapping author name/email/`"*"` to color string).
    pub author_colors: IndexMap<String, String>,
    /// Custom main-view subject highlighting rules (`[[main.subject-rules]]`).
    pub main_subject_rules: Vec<MainSubjectRuleConfig>,
    /// File path from which this configuration was loaded (e.g. via `--config`), if any.
    #[serde(skip)]
    pub loaded_path: Option<PathBuf>,
    /// Explicit CLI read-only override (`--read-only` = `Some(true)`, `--update-mode` = `Some(false)`).
    #[serde(skip)]
    pub cli_read_only_override: Option<bool>,
}

/// Performance tuning and memory-for-speed cache profile (`[performance]` table).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PerformanceConfig {
    /// Memory profile controlling cache budgets, revwalk chunking, and prefetch window (`"greedy"` by default).
    pub memory_profile: MemoryProfile,
}

/// Case-sensitivity setting supporting boolean (`true`/`false`) or string (`"auto"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum IgnoreCaseOption {
    /// Explicit boolean flag.
    Bool(bool),
    /// String mode such as `"auto"` or `"smart"`.
    String(String),
}

impl Default for IgnoreCaseOption {
    fn default() -> Self {
        Self::Bool(false)
    }
}

/// General editor and terminal configuration.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Spaces per tab indentation.
    pub tab_size: usize,
    /// Search case sensitivity: `true`, `false`, or `"auto"` (smart case).
    pub ignore_case: IgnoreCaseOption,
    /// Mouse click and scroll support.
    pub mouse: bool,
    /// Line wrapping in diff, blob, and blame views.
    pub wrap_lines: bool,
    /// Wrap-around search when reaching buffer boundaries.
    pub wrap_search: bool,
    /// Line graphics drawing mode.
    pub line_graphics: LineGraphics,
    /// Horizontal scroll step (columns or percentage).
    #[serde(
        deserialize_with = "deserialize_string_or_scalar",
        default = "default_horizontal_scroll"
    )]
    pub horizontal_scroll: String,
    /// Split view orientation: "auto", "horizontal", or "vertical" (or boolean `true`/`false`).
    #[serde(
        deserialize_with = "deserialize_string_or_scalar",
        default = "default_vertical_split"
    )]
    pub vertical_split: String,
    /// Percentage or line height allocated to split child views.
    #[serde(
        deserialize_with = "deserialize_string_or_scalar",
        default = "default_split_view_height"
    )]
    pub split_view_height: String,
    /// Percentage or column width allocated to vertical split child views.
    #[serde(
        deserialize_with = "deserialize_string_or_scalar",
        default = "default_split_view_width"
    )]
    pub split_view_width: String,
    /// Focus child view automatically upon opening.
    pub focus_child: bool,
    /// Pass line numbers to `$EDITOR`.
    pub editor_line_number: bool,
    /// Maximum history entries retained across sessions.
    pub history_size: usize,
    /// Auto-refresh mode: "auto", "manual", or "periodic".
    #[serde(
        deserialize_with = "deserialize_string_or_scalar",
        default = "default_refresh_mode"
    )]
    pub refresh_mode: String,
    /// Interval in seconds for periodic refresh.
    pub refresh_interval: u64,
    /// Prevent any repository state mutations (`false` by default; enable with `--read-only`).
    #[serde(default = "default_read_only")]
    pub read_only: bool,
}

const fn default_read_only() -> bool {
    false
}

fn default_horizontal_scroll() -> String {
    "50%".to_string()
}

fn default_vertical_split() -> String {
    "auto".to_string()
}

fn default_split_view_height() -> String {
    "67%".to_string()
}

fn default_split_view_width() -> String {
    "50%".to_string()
}

fn default_refresh_mode() -> String {
    "auto".to_string()
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            tab_size: 8,
            ignore_case: IgnoreCaseOption::Bool(false),
            mouse: false,
            wrap_lines: false,
            wrap_search: true,
            line_graphics: LineGraphics::default(),
            horizontal_scroll: default_horizontal_scroll(),
            vertical_split: default_vertical_split(),
            split_view_height: default_split_view_height(),
            split_view_width: default_split_view_width(),
            focus_child: true,
            editor_line_number: true,
            history_size: 500,
            refresh_mode: default_refresh_mode(),
            refresh_interval: 10,
            read_only: false,
        }
    }
}

fn default_diff_context() -> String {
    "3".to_string()
}

fn deserialize_string_or_scalar<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;
    struct StringOrScalarVisitor;
    impl de::Visitor<'_> for StringOrScalarVisitor {
        type Value = String;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string, integer, float, or boolean")
        }
        fn visit_bool<E>(self, v: bool) -> std::result::Result<Self::Value, E> {
            Ok(v.to_string())
        }
        fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E> {
            Ok(v.to_string())
        }
        fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E> {
            Ok(v.to_string())
        }
        fn visit_f64<E>(self, v: f64) -> std::result::Result<Self::Value, E> {
            Ok(v.to_string())
        }
        fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(v.to_string())
        }
    }
    deserializer.deserialize_any(StringOrScalarVisitor)
}

fn deserialize_diff_context<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_string_or_scalar(deserializer)
}

/// View-specific display preferences.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewConfig {
    /// Display source file line numbers on hunk lines and header row numbers (`line_number` / `number` / `nu`).
    #[serde(alias = "number", alias = "nu", alias = "lineno")]
    pub line_number: bool,
    /// Commit order in the main log view.
    pub commit_order: CommitOrder,
    /// Diff indicator sign mode. Accepts a TOML boolean as well as a string.
    pub diff_indicator: DiffIndicator,
    /// Ignore whitespace differences in diff view.
    pub ignore_space: IgnoreSpace,
    /// Automatically show uncommitted working tree changes in main view.
    pub show_changes: bool,
    /// Show untracked files in status view.
    pub show_untracked: bool,
    /// Expand untracked files inside untracked directories in the status view.
    pub status_show_untracked_files: bool,
    /// Show untracked directory entries in the status view.
    pub status_show_untracked_dirs: bool,
    /// Diff presentation style.
    pub diff_presentation: DiffPresentation,
    /// Diff layout mode.
    pub diff_layout: DiffLayout,
    /// Number of diff context lines ("3", "0".."N", or "full").
    #[serde(
        deserialize_with = "deserialize_diff_context",
        default = "default_diff_context"
    )]
    pub diff_context: String,
    /// Word diff intra-line highlighting enabled.
    pub word_diff: bool,
    /// Word diff line pairing algorithm.
    pub word_diff_pairing: WordDiffPairing,
    /// Minimum terminal width required for side-by-side view.
    pub side_by_side_min_width: u16,
    /// External diff formatter command string (optional).
    pub diff_formatter: String,
    /// Maximum line count for full-file diff context expansion.
    pub diff_context_full_max_lines: usize,
    /// Code syntax highlighting enabled in diff, blob, and blame views.
    pub syntax_highlighting: bool,
    /// Syntax highlighting color theme name (e.g. "Dracula", "Nord", "Monokai Extended").
    pub syntax_theme: String,
    /// UI view color theme across Adaptive, Dark, Light, and High-Contrast families.
    pub ui_theme: crate::config_enums::UiThemeId,
    /// Date column format ("default", "relative", "relative-compact", "short", "custom", "no").
    #[serde(
        deserialize_with = "deserialize_string_or_scalar",
        default = "default_date_format"
    )]
    pub date: String,
    /// Author column format ("full", "abbreviated", "email", "email-user", "no").
    #[serde(
        deserialize_with = "deserialize_string_or_scalar",
        default = "default_author_format"
    )]
    pub author: String,
    /// Show committer column in main view.
    pub committer: bool,
    /// Commit graph rendering mode ("v2", "v1", "no").
    #[serde(
        deserialize_with = "deserialize_string_or_scalar",
        default = "default_commit_title_graph"
    )]
    pub commit_title_graph: String,
    /// File name display mode ("always", "auto", "no").
    #[serde(
        deserialize_with = "deserialize_string_or_scalar",
        default = "default_file_name_mode"
    )]
    pub file_name: String,
    /// File size display mode ("default", "units", "no").
    #[serde(
        deserialize_with = "deserialize_string_or_scalar",
        default = "default_file_size_mode"
    )]
    pub file_size: String,
    /// Show branch/tag reference decorations on commit titles.
    pub commit_title_refs: bool,
    /// Show abbreviated commit ID column in main/blame views.
    pub id: bool,
    /// Enable active path filtering in main/diff views.
    pub file_filter: bool,
    /// Enable active revision filtering in main view.
    pub rev_filter: bool,
    /// Column width threshold for commit subject overflow warning (0 = disabled).
    pub commit_title_overflow: usize,
    /// Soft-wrap long lines in diff view instead of horizontal truncation (`wrap_lines` / `wrap`).
    #[serde(alias = "wrap", alias = "line_wrap")]
    pub wrap_lines: bool,
    /// Detect and highlight relocated code blocks across diff hunks and files (`color_moved`).
    #[serde(alias = "moved")]
    pub color_moved: bool,
    /// Pin an adaptive sticky file and function header when a file's banner scrolls out of view (`diff_sticky_header`).
    pub diff_sticky_header: bool,
    /// Contextual action hint chips in diff file banners (`auto`, `always`, `never`).
    pub diff_hints: DiffHintsMode,
    /// Automatically collapse files marked `linguist-generated` in `.gitattributes` (`diff_collapse_generated`).
    pub diff_collapse_generated: bool,
    /// Author identity coloring mode in main/log views (`hash`, `me`, `off`).
    pub main_author_color: MainAuthorColor,
    /// Interactive spotlight mode in main view (`off`, `author`, `ancestry`).
    pub main_spotlight: MainSpotlight,
    /// Dim non-matching rows when `main_spotlight` is active.
    pub main_spotlight_dim_others: bool,
    /// Dim commits not reachable from `HEAD` (`main_dim_unreachable`).
    pub main_dim_unreachable: bool,
    /// Color commit SHAs by upstream push/merge status (`main_push_status`).
    pub main_push_status: bool,
    /// Color the date column using a 6-bucket age heatmap (`main_date_heat`).
    pub main_date_heat: bool,
    /// Highlight semantic commit subject patterns (`fixup!`, `Revert`, `WIP`, conventional commits, `#123`).
    pub main_subject_rules: bool,
    /// Highlight the shortest unique SHA prefix in bold (`main_unique_prefix`).
    pub main_unique_prefix: bool,
    /// Dim merge commit subjects in main view (`main_dim_merges`).
    pub main_dim_merges: bool,
}

fn default_date_format() -> String {
    "default".to_string()
}

fn default_author_format() -> String {
    "full".to_string()
}

fn default_commit_title_graph() -> String {
    "v2".to_string()
}

fn default_file_name_mode() -> String {
    "always".to_string()
}

fn default_file_size_mode() -> String {
    "default".to_string()
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            line_number: false,
            commit_order: CommitOrder::default(),
            diff_indicator: DiffIndicator::default(),
            ignore_space: IgnoreSpace::default(),
            show_changes: true,
            show_untracked: true,
            status_show_untracked_files: true,
            status_show_untracked_dirs: true,
            diff_presentation: DiffPresentation::default(),
            diff_layout: DiffLayout::default(),
            diff_context: "3".to_string(),
            word_diff: true,
            word_diff_pairing: WordDiffPairing::default(),
            side_by_side_min_width: 80,
            diff_formatter: String::new(),
            diff_context_full_max_lines: 50_000,
            syntax_highlighting: true,
            syntax_theme: "Dracula".to_string(),
            ui_theme: crate::config_enums::UiThemeId::default(),
            date: default_date_format(),
            author: default_author_format(),
            committer: false,
            commit_title_graph: default_commit_title_graph(),
            file_name: default_file_name_mode(),
            file_size: default_file_size_mode(),
            commit_title_refs: true,
            id: false,
            file_filter: false,
            rev_filter: false,
            commit_title_overflow: 0,
            wrap_lines: false,
            color_moved: true,
            diff_sticky_header: true,
            diff_hints: DiffHintsMode::default(),
            diff_collapse_generated: true,
            main_author_color: MainAuthorColor::default(),
            main_spotlight: MainSpotlight::default(),
            main_spotlight_dim_others: false,
            main_dim_unreachable: false,
            main_push_status: true,
            main_date_heat: true,
            main_subject_rules: true,
            main_unique_prefix: true,
            main_dim_merges: false,
        }
    }
}

/// Security policies for untrusted repositories.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecuritySettings {
    /// Allow untrusted repositories to invoke hooks (disabled by default).
    pub trust_external_hooks: bool,
    /// Allow untrusted repositories to spawn fsmonitor binaries (disabled by default).
    pub fsmonitor_allowed: bool,
}

/// Expands a leading `~` to `$HOME`.
#[must_use]
pub fn expand_tilde(path_str: &str) -> PathBuf {
    if (path_str == "~" || path_str.starts_with("~/"))
        && let Ok(home) = std::env::var("HOME")
    {
        if path_str == "~" {
            return PathBuf::from(home);
        }
        return PathBuf::from(home).join(&path_str[2..]);
    }
    PathBuf::from(path_str)
}

/// Color specification supporting either string format or structured table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ColorSpec {
    /// Shorthand string: `"fg bg [attr1, attr2]"`.
    String(String),
    /// Structured table with explicit fields.
    Table {
        /// Foreground color specifier.
        fg: String,
        /// Background color specifier.
        bg: String,
        /// Text style attributes (such as `"bold"`, `"underline"`, or `"reverse"`).
        #[serde(default)]
        attributes: Vec<String>,
    },
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MainSectionRaw {
    #[serde(alias = "subject-rules")]
    subject_rules: Vec<toml::Value>,
}

#[derive(Debug, Deserialize)]
struct ConfigRaw {
    #[serde(default)]
    general: GeneralConfig,
    #[serde(default)]
    view: ViewConfig,
    #[serde(default)]
    performance: PerformanceConfig,
    #[serde(default)]
    security: SecuritySettings,
    #[serde(default)]
    keybindings: IndexMap<String, toml::Value>,
    #[serde(default)]
    colors: IndexMap<String, toml::Value>,
    #[serde(default)]
    main: MainSectionRaw,
}

impl Config {
    /// Returns the resolved canonical config file path, if available.
    #[must_use]
    pub fn default_config_path() -> Option<PathBuf> {
        if let Ok(env_path) = std::env::var("TIGRS_CONFIG") {
            let trimmed = env_path.trim();
            if !trimmed.is_empty() {
                let path = PathBuf::from(trimmed);
                if path.is_absolute() {
                    return Some(path);
                }
            }
        }

        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            let trimmed = xdg.trim();
            if !trimmed.is_empty() {
                let base = PathBuf::from(trimmed);
                if base.is_absolute() {
                    return Some(base.join("tigrs/config.toml"));
                }
            }
        }

        if let Ok(home) = std::env::var("HOME") {
            let trimmed = home.trim();
            if !trimmed.is_empty() {
                let home_path = PathBuf::from(trimmed);
                if home_path.is_absolute() {
                    return Some(home_path.join(".config/tigrs/config.toml"));
                }
            }
        }

        None
    }

    /// Loads configuration from default paths (`config.toml`), returning default settings if absent.
    #[must_use]
    pub fn load_default() -> Self {
        if let Some(path) = Self::default_config_path()
            && path.exists()
        {
            match Self::load_from_file(&path) {
                Ok(config) => return config,
                Err(err) => {
                    eprintln!("tigrs warning: {err}; falling back to default configuration");
                }
            }
        }
        Self::default()
    }

    /// Maximum allowed configuration file size (4 MiB) to prevent OOM or special-file hangs.
    pub const MAX_CONFIG_FILE_BYTES: u64 = 4 * 1024 * 1024;

    /// Reads a regular file up to [`Self::MAX_CONFIG_FILE_BYTES`] into a UTF-8 string.
    fn read_bounded_config_string(path: &Path) -> Result<String> {
        let file = std::fs::File::open(path).map_err(|e| {
            TigError::Config(format!(
                "Failed to read config file '{}': {e}",
                path.display()
            ))
        })?;
        let meta = file.metadata().map_err(|e| {
            TigError::Config(format!(
                "Failed to stat config file '{}': {e}",
                path.display()
            ))
        })?;
        if !meta.is_file() {
            return Err(TigError::Config(format!(
                "Failed to read config file '{}': not a regular file",
                path.display()
            )));
        }
        if meta.len() > Self::MAX_CONFIG_FILE_BYTES {
            return Err(TigError::Config(format!(
                "Failed to read config file '{}': file size ({} bytes) exceeds 4 MiB limit",
                path.display(),
                meta.len()
            )));
        }
        let mut content = String::new();
        let mut bounded = std::io::Read::take(file, Self::MAX_CONFIG_FILE_BYTES + 1);
        std::io::Read::read_to_string(&mut bounded, &mut content).map_err(|e| {
            TigError::Config(format!(
                "Failed to read config file '{}': {e}",
                path.display()
            ))
        })?;
        if content.len() as u64 > Self::MAX_CONFIG_FILE_BYTES {
            return Err(TigError::Config(format!(
                "Failed to read config file '{}': file exceeds 4 MiB limit",
                path.display()
            )));
        }
        Ok(content)
    }

    /// Loads and parses configuration from a specific TOML file path.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = Self::read_bounded_config_string(path)?;
        let mut cfg = Self::parse_toml(&content)?;
        cfg.loaded_path = Some(path.to_path_buf());
        Ok(cfg)
    }

    /// Parses TOML string content, normalizing both flat and nested keybindings.
    pub fn parse_toml(content: &str) -> Result<Self> {
        let raw: ConfigRaw = toml::from_str(content)
            .map_err(|e| TigError::Config(format!("Failed to deserialize configuration: {e}")))?;

        let mut keybindings = IndexMap::new();
        for (k, v) in raw.keybindings {
            if let Some(table) = v.as_table() {
                // [keybindings.scope]
                for (inner_key, inner_val) in table {
                    if let Some(s) = inner_val.as_str() {
                        keybindings.insert(format!("{k}.{inner_key}"), s.to_string());
                    }
                }
            } else if let Some(s) = v.as_str() {
                keybindings.insert(k, s.to_string());
            }
        }

        let mut colors = IndexMap::new();
        let mut author_colors = IndexMap::new();
        for (area, val) in raw.colors {
            if area.eq_ignore_ascii_case("authors")
                && let Some(table) = val.as_table()
            {
                for (author_key, color_val) in table {
                    if let Some(s) = color_val.as_str() {
                        author_colors.insert(author_key.clone(), s.to_string());
                    }
                }
                continue;
            }
            if let Some(s) = val.as_str() {
                colors.insert(area, ColorSpec::String(s.to_string()));
            } else if let Some(table) = val.as_table() {
                let fg = table
                    .get("fg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default")
                    .to_string();
                let bg = table
                    .get("bg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default")
                    .to_string();
                let attributes = table
                    .get("attributes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(ToString::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                colors.insert(area, ColorSpec::Table { fg, bg, attributes });
            }
        }

        let mut main_subject_rules = Vec::new();
        for entry in raw.main.subject_rules {
            if let Some(t) = entry.as_table() {
                let pattern = t
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if pattern.is_empty() {
                    continue;
                }
                let style_table = t.get("style").and_then(|v| v.as_table());
                let fg = style_table
                    .and_then(|st| st.get("fg"))
                    .or_else(|| t.get("fg"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("yellow")
                    .to_string();
                let bold = style_table
                    .and_then(|st| st.get("bold"))
                    .or_else(|| t.get("bold"))
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                let dim = style_table
                    .and_then(|st| st.get("dim"))
                    .or_else(|| t.get("dim"))
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                let italic = style_table
                    .and_then(|st| st.get("italic"))
                    .or_else(|| t.get("italic"))
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                let underline = style_table
                    .and_then(|st| st.get("underline"))
                    .or_else(|| t.get("underline"))
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                main_subject_rules.push(MainSubjectRuleConfig {
                    pattern,
                    fg,
                    bold,
                    dim,
                    italic,
                    underline,
                });
            }
        }

        Ok(Config {
            general: raw.general,
            view: raw.view,
            performance: raw.performance,
            security: raw.security,
            keybindings,
            colors,
            author_colors,
            main_subject_rules,
            loaded_path: None,
            cli_read_only_override: None,
        })
    }

    /// Merges the current `[general]`, `[view]`, and `[performance]` configuration into `existing_toml` (if any),
    /// preserving comments, key ordering, `[security]`, `[keybindings]`, `[colors]`, and any custom user tables.
    ///
    /// When `minimal` is `true`, only keys whose values differ from `default_baseline` are
    /// persisted (and keys reverted to `default_baseline` are removed from `[general]` / `[view]` / `[performance]`).
    /// When `minimal` is `false`, all `[general]`, `[view]`, and `[performance]` keys are written.
    pub fn render_merged_toml(
        &self,
        default_baseline: &Self,
        existing_toml: Option<&str>,
        minimal: bool,
    ) -> Result<(String, usize)> {
        let had_existing = existing_toml.is_some_and(|s| !s.trim().is_empty());
        let mut doc: toml_edit::DocumentMut = if let Some(raw_str) = existing_toml {
            if raw_str.trim().is_empty() {
                toml_edit::DocumentMut::new()
            } else {
                raw_str.parse::<toml_edit::DocumentMut>().map_err(|e| {
                    TigError::Config(format!("Failed to parse existing TOML for merging: {e}"))
                })?
            }
        } else {
            toml_edit::DocumentMut::new()
        };

        let current_general = toml::Value::try_from(&self.general)
            .map_err(|e| TigError::Config(format!("Failed to serialize [general]: {e}")))?;
        let default_general = toml::Value::try_from(&default_baseline.general)
            .map_err(|e| TigError::Config(format!("Failed to serialize default [general]: {e}")))?;

        let current_view = toml::Value::try_from(&self.view)
            .map_err(|e| TigError::Config(format!("Failed to serialize [view]: {e}")))?;
        let default_view = toml::Value::try_from(&default_baseline.view)
            .map_err(|e| TigError::Config(format!("Failed to serialize default [view]: {e}")))?;

        let current_perf = toml::Value::try_from(&self.performance)
            .map_err(|e| TigError::Config(format!("Failed to serialize [performance]: {e}")))?;
        let default_perf = toml::Value::try_from(&default_baseline.performance).map_err(|e| {
            TigError::Config(format!("Failed to serialize default [performance]: {e}"))
        })?;

        let mut persisted_count = 0usize;

        for (section_name, current_val, default_val) in [
            ("general", current_general, default_general),
            ("view", current_view, default_view),
            ("performance", current_perf, default_perf),
        ] {
            let Some(current_map) = current_val.as_table() else {
                continue;
            };
            let default_map = default_val.as_table();

            if section_name == "view"
                && let Some(tbl) = doc
                    .get_mut(section_name)
                    .and_then(toml_edit::Item::as_table_mut)
            {
                for alias in ["number", "nu", "lineno"] {
                    tbl.remove(alias);
                }
            }

            for (k, v) in current_map {
                let is_default = default_map.and_then(|d| d.get(k)) == Some(v);
                if minimal && is_default {
                    if let Some(tbl) = doc
                        .get_mut(section_name)
                        .and_then(toml_edit::Item::as_table_mut)
                    {
                        tbl.remove(k);
                    }
                    continue;
                }

                let Some(edit_val) = toml_value_to_edit_value(v) else {
                    continue;
                };
                persisted_count += 1;

                if !doc.contains_key(section_name) {
                    doc.insert(
                        section_name,
                        toml_edit::Item::Table(toml_edit::Table::new()),
                    );
                }
                if let Some(tbl) = doc
                    .get_mut(section_name)
                    .and_then(toml_edit::Item::as_table_mut)
                {
                    if let Some(existing_item) = tbl.get_mut(k)
                        && let Some(existing_val) = existing_item.as_value_mut()
                    {
                        let old_decor = existing_val.decor().clone();
                        *existing_val = edit_val;
                        *existing_val.decor_mut() = old_decor;
                    } else {
                        tbl.insert(k, toml_edit::Item::Value(edit_val));
                    }
                }
            }
        }

        let serialized = doc.to_string();
        if had_existing {
            Ok((serialized, persisted_count))
        } else {
            let header = "# tigrs configuration — managed by tigrs Options Panel (press `o`)\n\n";
            let final_output = if serialized.trim().is_empty() {
                format!("{header}# All options are currently set to built-in defaults.\n")
            } else {
                format!("{header}{serialized}")
            };
            Ok((final_output, persisted_count))
        }
    }

    /// Atomically writes the merged TOML configuration to `path`, creating parent directories
    /// if necessary. Follows symbolic links so dotfile-managed symlinks are preserved,
    /// preserves existing file permissions (or defaults to `0600`), and preserves
    /// comments, `[security]`, `[keybindings]`, and `[colors]` sections when `path` already exists.
    /// Returns the number of persisted `[general]` + `[view]` + `[performance]` keys.
    pub fn save_to_path(
        &self,
        default_baseline: &Self,
        path: &Path,
        minimal: bool,
    ) -> Result<usize> {
        let target_path = resolve_symlink_target(path);
        let existing_meta = std::fs::metadata(&target_path).ok();
        let existing_content = if existing_meta.is_some() {
            Some(Self::read_bounded_config_string(&target_path)?)
        } else {
            None
        };

        let (rendered, count) =
            self.render_merged_toml(default_baseline, existing_content.as_deref(), minimal)?;

        let parent = target_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                TigError::Config(format!(
                    "Failed to create config directory '{}': {e}",
                    parent.display()
                ))
            })?;
        }

        let file_name = target_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.toml");
        let prefix = format!(".{file_name}.tmp.");
        let mut builder = tempfile::Builder::new();
        builder.prefix(&prefix);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = existing_meta.map_or_else(
                || std::fs::Permissions::from_mode(0o600),
                |m| m.permissions(),
            );
            builder.permissions(perms);
        }

        let mut tmp = builder.tempfile_in(parent).map_err(|e| {
            TigError::Config(format!(
                "Failed to create temporary config in '{}': {e}",
                parent.display()
            ))
        })?;
        std::io::Write::write_all(&mut tmp, rendered.as_bytes()).map_err(|e| {
            TigError::Config(format!(
                "Failed to write temporary config in '{}': {e}",
                parent.display()
            ))
        })?;
        tmp.as_file().sync_all().map_err(|e| {
            TigError::Config(format!(
                "Failed to sync temporary config in '{}': {e}",
                parent.display()
            ))
        })?;
        tmp.persist(&target_path).map_err(|e| {
            TigError::Config(format!(
                "Failed to atomically replace config '{}': {}",
                target_path.display(),
                e.error
            ))
        })?;
        if let Ok(dir_file) = std::fs::File::open(parent) {
            let _ = dir_file.sync_all();
        }

        Ok(count)
    }
}

/// Converts a scalar `toml::Value` into a `toml_edit::Value`.
fn toml_value_to_edit_value(v: &toml::Value) -> Option<toml_edit::Value> {
    match v {
        toml::Value::String(s) => Some(toml_edit::Value::from(s.as_str())),
        toml::Value::Integer(i) => Some(toml_edit::Value::from(*i)),
        toml::Value::Float(f) => Some(toml_edit::Value::from(*f)),
        toml::Value::Boolean(b) => Some(toml_edit::Value::from(*b)),
        _ => None,
    }
}

/// Resolves up to 40 levels of symbolic links on `path` so atomic file replacement
/// writes next to and replaces the symlink's target rather than replacing the symlink itself.
#[must_use]
pub fn resolve_symlink_target(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::read_link(&current) {
            Ok(target) => {
                if target.is_absolute() {
                    current = target;
                } else if let Some(parent) = current.parent() {
                    current = parent.join(target);
                } else {
                    current = target;
                }
            }
            Err(_) => break,
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_template_config() {
        let template = include_str!("../../../config.toml.example");
        let config = Config::parse_toml(template).expect("failed to parse template config");

        assert_eq!(config.general.tab_size, 8);
        assert!(!config.general.mouse);
        assert!(config.general.wrap_search);
        assert_eq!(config.general.split_view_height, "67%");
        assert_eq!(config.view.commit_order, CommitOrder::Default);
        assert!(config.view.show_changes);

        // Verify keybindings are parsed
        assert_eq!(
            config.keybindings.get("generic.q").map(String::as_str),
            Some("quit")
        );
        assert_eq!(
            config.keybindings.get("main.Enter").map(String::as_str),
            Some("view-diff")
        );
        assert_eq!(
            config.keybindings.get("diff.@").map(String::as_str),
            Some("chunk-toggle")
        );

        // Verify colors are parsed
        assert!(config.colors.contains_key("cursor"));
        assert!(config.colors.contains_key("diff-stat"));
    }

    #[test]
    fn test_parse_nested_keybindings_and_colors() {
        let toml_str = r#"
[general]
tab_size = 4
mouse = true

[keybindings.generic]
"q" = "quit"
"j" = "move-down"

[keybindings.main]
"Enter" = "view-diff"

[colors."cursor"]
fg = "white"
bg = "blue"
attributes = ["bold"]

[colors."diff-add"]
fg = "green"
bg = "default"
"#;

        let config = Config::parse_toml(toml_str).expect("failed to parse nested config");
        assert_eq!(config.general.tab_size, 4);
        assert!(config.general.mouse);

        assert_eq!(
            config.keybindings.get("generic.q").map(String::as_str),
            Some("quit")
        );
        assert_eq!(
            config.keybindings.get("generic.j").map(String::as_str),
            Some("move-down")
        );
        assert_eq!(
            config.keybindings.get("main.Enter").map(String::as_str),
            Some("view-diff")
        );

        match config.colors.get("cursor") {
            Some(ColorSpec::Table { fg, bg, attributes }) => {
                assert_eq!(fg, "white");
                assert_eq!(bg, "blue");
                assert_eq!(attributes, &["bold"]);
            }
            _ => panic!("cursor color not parsed as table"),
        }
    }

    #[test]
    fn test_default_config_fallback() {
        let config = Config::parse_toml("").expect("parse empty toml");
        assert_eq!(config.general.tab_size, 8);
        assert_eq!(config.general.vertical_split, "auto");
        assert_eq!(config.general.split_view_height, "67%");
        assert_eq!(config.view.commit_order, CommitOrder::Default);
        assert!(config.view.show_changes);
        assert!(!config.security.trust_external_hooks);
        assert!(!config.security.fsmonitor_allowed);
        assert!(config.keybindings.is_empty());
        assert!(config.colors.is_empty());
    }

    #[test]
    fn test_invalid_toml_error() {
        let res = Config::parse_toml("not a valid [toml = syntax");
        assert!(res.is_err());
    }

    #[test]
    fn test_color_string_format() {
        let toml_str = r#"
[colors]
"diff-add" = "green"
"diff-del" = "red"
"#;
        let config = Config::parse_toml(toml_str).expect("parse simple colors");
        match config.colors.get("diff-add") {
            Some(ColorSpec::String(val)) => assert_eq!(val, "green"),
            other => panic!("expected string color, got {other:?}"),
        }
        match config.colors.get("diff-del") {
            Some(ColorSpec::String(val)) => assert_eq!(val, "red"),
            other => panic!("expected string color, got {other:?}"),
        }
    }

    #[test]
    fn test_security_settings_override() {
        let toml_str = r"
[security]
trust_external_hooks = true
fsmonitor_allowed = true
";
        let config = Config::parse_toml(toml_str).expect("parse security");
        assert!(config.security.trust_external_hooks);
        assert!(config.security.fsmonitor_allowed);
    }

    #[test]
    fn test_config_load_nonexistent_file() {
        let path = std::path::Path::new("/nonexistent/path/to/config.toml");
        let res = Config::load_from_file(path);
        assert!(res.is_err());
    }

    #[test]
    fn test_config_type_mismatch_error_reporting() {
        // String instead of integer for tab_size
        let toml_str = r#"
[general]
tab_size = "eight"
"#;
        let res = Config::parse_toml(toml_str);
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("Configuration error:"));

        // Non-boolean for trust_external_hooks
        let toml_str2 = r#"
[security]
trust_external_hooks = "yes"
"#;
        let res2 = Config::parse_toml(toml_str2);
        assert!(res2.is_err());

        // String instead of table for general section
        let toml_str3 = r#"
general = "flat string"
"#;
        let res3 = Config::parse_toml(toml_str3);
        assert!(res3.is_err());
    }

    #[test]
    fn test_config_empty_sections_handled_safely() {
        let toml_str = r"
[general]
[view]
[colors]
[keybindings]
[security]
";
        let config = Config::parse_toml(toml_str).expect("empty sections must parse cleanly");
        assert_eq!(config.general.tab_size, 8);
        assert_eq!(config.view.commit_order, CommitOrder::Default);
        assert!(config.colors.is_empty());
        assert!(config.keybindings.is_empty());
    }

    #[test]
    fn test_config_value_default_and_security_defaults() {
        assert_eq!(IgnoreCaseOption::default(), IgnoreCaseOption::Bool(false));

        let toml_str = r"
[security]
trust_external_hooks = true
fsmonitor_allowed = false
";
        let config = Config::parse_toml(toml_str).expect("security must parse");
        assert!(config.security.trust_external_hooks);
        assert!(!config.security.fsmonitor_allowed);
    }

    #[test]
    fn test_config_load_and_default_path() {
        let _ = Config::default_config_path();
        let _ = Config::load_default();

        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nonexistent_config.toml");
        assert!(Config::load_from_file(&missing).is_err());
        assert_eq!(Config::default().general.tab_size, 8);

        let temp_file = dir.path().join("tigrs_cfg.toml");
        std::fs::write(&temp_file, "[general]\ntab_size = 4\n").unwrap();
        let from_file = Config::load_from_file(&temp_file).unwrap();
        assert_eq!(from_file.general.tab_size, 4);
    }

    #[test]
    fn test_config_ignore_space_enum_and_indexmap_ordering() {
        let toml_some = "[view]\nignore_space = \"some\"\n";
        let cfg = Config::parse_toml(toml_some).expect("parse some");
        assert_eq!(cfg.view.ignore_space, IgnoreSpace::Some);

        let toml_eol = "[view]\nignore_space = \"at-eol\"\n";
        let cfg = Config::parse_toml(toml_eol).expect("parse at-eol");
        assert_eq!(cfg.view.ignore_space, IgnoreSpace::AtEol);

        let toml_bool_true = "[view]\nignore_space = true\n";
        let cfg = Config::parse_toml(toml_bool_true).expect("parse true");
        assert_eq!(cfg.view.ignore_space, IgnoreSpace::All);

        let toml_order = r#"
[keybindings]
"main.z" = "quit"
"main.a" = "view-diff"
"main.m" = "view-main"
"#;
        let cfg = Config::parse_toml(toml_order).expect("parse order");
        let keys: Vec<&str> = cfg.keybindings.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["main.z", "main.a", "main.m"]);
    }

    #[test]
    fn test_toml_flexible_scalar_setting_types() {
        let toml_input = r#"
[general]
vertical_split = true
horizontal_scroll = 4
split_view_height = 12
split_view_width = 60
refresh_mode = "periodic"
refresh_interval = 5

[view]
diff_context = 5
ignore_space = true
commit_order = "topo"

[keybindings.main]
G = "move-last-line"
"#;
        let cfg = Config::parse_toml(toml_input).expect("parse flexible scalar TOML into Config");
        assert_eq!(cfg.general.vertical_split, "true");
        assert_eq!(cfg.general.horizontal_scroll, "4");
        assert_eq!(cfg.general.split_view_height, "12");
        assert_eq!(cfg.general.split_view_width, "60");
        assert_eq!(cfg.general.refresh_mode, "periodic");
        assert_eq!(cfg.general.refresh_interval, 5);
        assert_eq!(cfg.view.diff_context, "5");
        assert_eq!(cfg.view.ignore_space, IgnoreSpace::All);
        assert_eq!(cfg.view.commit_order, CommitOrder::Topo);
        assert_eq!(
            cfg.keybindings.get("main.G").map(String::as_str),
            Some("move-last-line")
        );
    }

    #[test]
    fn test_read_only_default_and_security_no_allowlist() {
        let cfg = Config::default();
        assert!(!cfg.general.read_only);
        let sec_cfg = crate::SecurityConfig::new();
        assert!(!sec_cfg.is_repo_trusted(std::path::Path::new("/tmp/repo-alpha")));
    }
}
