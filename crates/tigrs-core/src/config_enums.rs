// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! String-valued display option enums shared by the config file and the UI.
//!
//! Every one of these settings is reachable from two directions — a
//! `config.toml` key and the interactive `:set` / toggle menu — and each
//! direction historically re-implemented its own
//! `match value.to_lowercase().as_str()` block. Those blocks drifted: some
//! accepted aliases the others rejected, and the config loader silently fell
//! back to the default on a typo.
//!
//! `config_enum!` derives all of it from a single alias table per enum, so
//! parsing, rendering, cycling, and (de)serialization can no longer disagree.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

/// Error returned when a configuration value does not name a known variant.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[error("invalid {type_name} value '{value}' (expected one of: {})", expected.join(", "))]
pub struct ParseConfigEnumError {
    /// Name of the option type that failed to parse (e.g. `"DiffLayout"`).
    pub type_name: &'static str,
    /// The offending value, trimmed.
    pub value: String,
    /// Canonical spellings accepted for this type.
    pub expected: &'static [&'static str],
}

/// Declares a string-valued configuration enum from a single alias table.
///
/// Each variant is written as `Variant = "canonical" , "alias", ...;`. From
/// that the macro generates:
///
/// - [`Self::as_str`] and [`std::fmt::Display`], returning the canonical spelling;
/// - [`Self::ALL`] and [`Self::next`], cycling variants in declaration order;
/// - [`std::str::FromStr`], accepting the canonical spelling and every alias
///   ASCII-case-insensitively and rejecting anything else;
/// - [`serde::Serialize`] / [`serde::Deserialize`] built on the two above, so a
///   `config.toml` file and `:set` accept the exact same syntax, and an
///   unrecognized value is reported instead of silently becoming the default.
///
/// A TOML boolean deserializes as if the strings `"true"` / `"false"` had been
/// written, which lets enums that list those as aliases (such as
/// `DiffIndicator`) accept `diff_indicator = true`.
macro_rules! config_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$vmeta:meta])*
                $variant:ident = $canonical:literal $(, $alias:literal)* ;
            )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
        pub enum $name {
            $(
                $(#[$vmeta])*
                $variant,
            )+
        }

        impl $name {
            /// Every variant, in toggle-cycle order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Canonical spellings, in the same order as [`Self::ALL`].
            pub const EXPECTED: &'static [&'static str] = &[$($canonical),+];

            /// Returns the canonical option value representation.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $canonical,)+
                }
            }

            /// Returns the next variant in the toggle cycle, wrapping around.
            #[must_use]
            pub fn next(self) -> Self {
                let idx = Self::ALL
                    .iter()
                    .position(|candidate| *candidate == self)
                    .unwrap_or(0);
                Self::ALL[(idx + 1) % Self::ALL.len()]
            }

            /// Returns the previous variant in the toggle cycle, wrapping around.
            #[must_use]
            pub fn prev(self) -> Self {
                let idx = Self::ALL
                    .iter()
                    .position(|candidate| *candidate == self)
                    .unwrap_or(0);
                Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
            }

            /// Parses a value from a configuration string, falling back to `Self::default()` on unrecognized input.
            #[must_use]
            pub fn from_config_str(s: &str) -> Self {
                s.parse().unwrap_or_default()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = ParseConfigEnumError;

            fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
                let value = s.trim();
                $(
                    if value.eq_ignore_ascii_case($canonical)
                        $(|| value.eq_ignore_ascii_case($alias))*
                    {
                        return Ok(Self::$variant);
                    }
                )+
                Err(ParseConfigEnumError {
                    type_name: stringify!($name),
                    value: value.to_string(),
                    expected: Self::EXPECTED,
                })
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                struct Visitor;

                impl de::Visitor<'_> for Visitor {
                    type Value = $name;

                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        write!(f, "one of: {}", $name::EXPECTED.join(", "))
                    }

                    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                        v.parse().map_err(de::Error::custom)
                    }

                    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
                        let as_text = if v { "true" } else { "false" };
                        as_text.parse().map_err(de::Error::custom)
                    }
                }

                deserializer.deserialize_any(Visitor)
            }
        }
    };
}

config_enum! {
    /// Line graphics rendering style (Unicode box-drawing vs ASCII).
    pub enum LineGraphics {
        /// UTF-8 Unicode box-drawing characters (`│`, `├`, `─`, `┌`).
        #[default]
        Utf8 = "utf-8", "utf8", "unicode", "default";
        /// ASCII fallback characters (`|`, `\`, `/`, `+`).
        Ascii = "ascii";
    }
}

config_enum! {
    /// Commit sorting order in revision logs.
    pub enum CommitOrder {
        /// Default Git commit ordering (reverse chronological).
        #[default]
        Default = "default", "auto";
        /// Topological sorting (`--topo-order`).
        Topo = "topo", "topo-order";
        /// Date sorting (`--date-order`).
        Date = "date", "date-order";
        /// Author date sorting (`--author-date-order`).
        AuthorDate = "author-date", "author_date";
        /// Reverse commit order (`--reverse`).
        Reverse = "reverse";
    }
}

config_enum! {
    /// Diff presentation mode (single-line banner, fancy diff-so-fancy style, or classic unified).
    pub enum DiffPresentation {
        /// Single-line Gerrit-inspired file banner and human-readable hunk separators (default).
        #[default]
        Banner = "banner", "default", "compact", "gerrit";
        /// Fancy presentation with rule headers, functional context, and empty line markers.
        Fancy = "fancy";
        /// Classic unified diff presentation (`diff --git`, `index`, `---`, `+++`, `@@`).
        Classic = "classic", "git", "raw";
    }
}

config_enum! {
    /// Contextual action hint chips display mode in diff file banners (`[view] diff_hints`).
    pub enum DiffHintsMode {
        /// Automatically show hint chips when terminal width >= 100 columns.
        #[default]
        Auto = "auto", "default";
        /// Always show hint chips in file banners.
        Always = "always", "yes", "true", "on", "1";
        /// Never show hint chips in file banners.
        Never = "never", "no", "false", "off", "0";
    }
}

config_enum! {
    /// Diff layout mode (unified single pane vs side-by-side dual pane).
    pub enum DiffLayout {
        /// Unified diff view.
        #[default]
        Unified = "unified", "single", "default";
        /// Two-pane side-by-side diff view.
        SideBySide = "side-by-side", "side_by_side", "side-to-side", "side_to_side";
    }
}

config_enum! {
    /// Diff indicator (`+`/`-`) column presentation mode.
    pub enum DiffIndicator {
        /// Automatic: signs shown in classic, stripped in fancy (unless monochrome).
        #[default]
        Auto = "auto", "default";
        /// Always display the signs column.
        Yes = "yes", "true", "on", "1";
        /// Always strip the signs column (unless monochrome).
        No = "no", "false", "off", "0";
    }
}

config_enum! {
    /// Word diff line pairing algorithm.
    pub enum WordDiffPairing {
        /// Token similarity-based pairing.
        #[default]
        Similarity = "similarity", "default";
        /// Positional 1:1 pairing (equal-length runs only).
        Positional = "positional";
    }
}

config_enum! {
    /// Whitespace difference handling in diffs.
    pub enum IgnoreSpace {
        /// Respect all whitespace changes.
        #[default]
        No = "no", "false", "off", "0", "none", "default";
        /// Ignore all whitespace (`-w` / `--ignore-all-space`).
        All = "all", "yes", "true", "on", "1";
        /// Ignore whitespace amount changes (`-b` / `--ignore-space-change`).
        Some = "some";
        /// Ignore whitespace at end of lines (`--ignore-space-at-eol`).
        AtEol = "at-eol", "at_eol";
    }
}

config_enum! {
    /// Commit date display formatting mode.
    pub enum DateFormat {
        /// Human-friendly relative date (e.g. `"2d ago"`, `"3h ago"`).
        #[default]
        Relative = "relative", "default", "yes", "true", "on", "1";
        /// Short ISO date (`"YYYY-MM-DD"`).
        Short = "short";
        /// Full ISO timestamp (`"YYYY-MM-DD HH:MM"`).
        Iso = "iso", "custom", "local";
        /// Disable date column.
        Off = "no", "false", "off", "0";
    }
}

config_enum! {
    /// Commit author display formatting mode.
    pub enum AuthorFormat {
        /// Full author name (e.g. `"Linus Torvalds"`).
        #[default]
        Full = "full", "default", "yes", "true", "on", "1";
        /// Abbreviated author name (e.g. `"L. Torvalds"`).
        Abbreviated = "abbreviated";
        /// Author email username before `@`.
        Email = "email", "email-user";
        /// Disable author column.
        Off = "no", "false", "off", "0";
    }
}

config_enum! {
    /// Commit graph display mode.
    pub enum GraphDisplay {
        /// Automatically show graph when log order allows.
        #[default]
        Auto = "auto", "default", "yes", "true", "on", "1", "v1";
        /// Extended v2 graph algorithm.
        V2 = "v2";
        /// Disable graph drawing.
        No = "no", "false", "off", "0";
    }
}

config_enum! {
    /// Built-in UI view color theme across Adaptive, Dark, Light, and High-Contrast families.
    pub enum UiThemeId {
        /// Terminal-native adaptive ANSI palette preserving default foreground/background.
        #[default]
        Default = "default", "ansi", "adaptive", "terminal", "auto";
        /// Catppuccin Mocha soothing dark pastel palette.
        CatppuccinMocha = "catppuccin-mocha", "catppuccin_mocha", "catppuccin", "mocha", "dark";
        /// Dracula vibrant dark purple/cyan palette.
        Dracula = "dracula";
        /// Tokyo Night modern indigo dark palette.
        TokyoNight = "tokyo-night", "tokyo_night", "tokyonight";
        /// Gruvbox Dark warm retro earthy palette.
        GruvboxDark = "gruvbox-dark", "gruvbox_dark", "gruvbox";
        /// Nord arctic bluish dark palette.
        Nord = "nord";
        /// Solarized Dark precision CIELAB dark palette.
        SolarizedDark = "solarized-dark", "solarized_dark", "solarized";
        /// GitHub Primer Dark high-contrast dark palette.
        GithubDark = "github-dark", "github_dark", "githubdark";
        /// Catppuccin Latte warm daylight pastel light palette.
        CatppuccinLatte = "catppuccin-latte", "catppuccin_latte", "latte";
        /// GitHub Primer Light daylight palette.
        GithubLight = "github-light", "github_light", "githublight", "light";
        /// Solarized Light warm ivory daylight palette.
        SolarizedLight = "solarized-light", "solarized_light", "solarizedlight";
        /// Gruvbox Light warm paper daylight palette.
        GruvboxLight = "gruvbox-light", "gruvbox_light", "gruvboxlight";
        /// WCAG AAA High-Contrast Dark pitch-black palette.
        HighContrastDark = "high-contrast-dark", "high_contrast_dark", "hc-dark", "hc_dark", "high-contrast", "high_contrast", "hc";
        /// WCAG AAA High-Contrast Light pure-white palette.
        HighContrastLight = "high-contrast-light", "high_contrast_light", "hc-light", "hc_light";
    }
}

impl UiThemeId {
    /// Returns the 1-based position of this theme in [`Self::ALL`] (`1..=14`).
    #[must_use]
    pub const fn position(self) -> usize {
        match self {
            Self::Default => 1,
            Self::CatppuccinMocha => 2,
            Self::Dracula => 3,
            Self::TokyoNight => 4,
            Self::GruvboxDark => 5,
            Self::Nord => 6,
            Self::SolarizedDark => 7,
            Self::GithubDark => 8,
            Self::CatppuccinLatte => 9,
            Self::GithubLight => 10,
            Self::SolarizedLight => 11,
            Self::GruvboxLight => 12,
            Self::HighContrastDark => 13,
            Self::HighContrastLight => 14,
        }
    }

    /// Returns the human-readable family label (`"Adaptive"`, `"Dark"`, `"Light"`, or `"High-Contrast"`).
    #[must_use]
    pub const fn family_label(self) -> &'static str {
        match self {
            Self::Default => "Adaptive",
            Self::CatppuccinMocha
            | Self::Dracula
            | Self::TokyoNight
            | Self::GruvboxDark
            | Self::Nord
            | Self::SolarizedDark
            | Self::GithubDark => "Dark",
            Self::CatppuccinLatte
            | Self::GithubLight
            | Self::SolarizedLight
            | Self::GruvboxLight => "Light",
            Self::HighContrastDark | Self::HighContrastLight => "High-Contrast",
        }
    }

    /// Returns `true` when this theme is designed for a light/daylight background.
    #[must_use]
    pub const fn is_light(self) -> bool {
        matches!(
            self,
            Self::CatppuccinLatte
                | Self::GithubLight
                | Self::SolarizedLight
                | Self::GruvboxLight
                | Self::HighContrastLight
        )
    }

    /// Returns the recommended canonical `syntect` syntax highlighting theme paired with this UI theme.
    #[must_use]
    pub const fn paired_syntax_theme(self) -> &'static str {
        match self {
            Self::Default | Self::Dracula => "Dracula",
            Self::CatppuccinMocha => "base16-mocha.dark",
            Self::TokyoNight | Self::Nord => "Nord",
            Self::GruvboxDark => "gruvbox-dark",
            Self::SolarizedDark => "Solarized (dark)",
            Self::GithubDark => "TwoDark",
            Self::CatppuccinLatte | Self::SolarizedLight => "Solarized (light)",
            Self::GithubLight | Self::HighContrastLight => "GitHub",
            Self::GruvboxLight => "gruvbox-light",
            Self::HighContrastDark => "Visual Studio Dark+",
        }
    }
}

config_enum! {
    /// Memory-for-speed caching and speculative prefetch profile (`[performance] memory_profile`).
    pub enum MemoryProfile {
        /// Conservative memory footprint (~410 MB on a 1.5M-commit repo).
        Lean = "lean", "low", "minimal";
        /// Mid-tier caches and prefetch depth (~700 MB on a 1.5M-commit repo).
        Balanced = "balanced", "medium";
        /// Aggressive caching, larger delta-base/object caches, and wider prefetch (~1.5 GB on a 1.5M-commit repo).
        #[default]
        Greedy = "greedy", "high", "max", "default";
    }
}

config_enum! {
    /// Author coloring mode in main and log views (`[view] main_author_color`).
    pub enum MainAuthorColor {
        /// Deterministic 10-hue palette hash per author identity (lazygit-style, default).
        #[default]
        Hash = "hash", "auto", "default", "yes", "true", "on", "1";
        /// Highlight only the current repository user's commits; mute other authors.
        Me = "me", "self", "mine";
        /// Uniform theme `author_fg` color for all authors (classic tig style).
        Off = "off", "no", "false", "0", "none";
    }
}

config_enum! {
    /// Interactive spotlight mode in main view (`[view] main_spotlight`).
    pub enum MainSpotlight {
        /// Spotlight disabled.
        Off = "off", "no", "false", "0", "none";
        /// Highlight all commits authored by the currently selected commit's author (default).
        #[default]
        Author = "author", "same-author", "same_author", "who", "yes", "true", "on", "1", "default";
        /// Highlight all commits that are ancestors of the currently selected commit.
        Ancestry = "ancestry", "branch", "lineage", "parents";
    }
}

impl MemoryProfile {
    /// Foreground `gix` decoded-object cache limit in bytes (`PACK_CACHE_LIMIT_BYTES`).
    #[must_use]
    pub const fn object_cache_bytes(self) -> usize {
        match self {
            Self::Lean => 64 * 1024 * 1024,
            Self::Balanced => 256 * 1024 * 1024,
            Self::Greedy => 1024 * 1024 * 1024,
        }
    }

    /// Foreground `gix` pack delta-base cache limit in bytes (`MemoryCappedHashmap`).
    #[must_use]
    pub const fn delta_pack_cache_bytes(self) -> usize {
        match self {
            Self::Lean => 32 * 1024 * 1024,
            Self::Balanced => 256 * 1024 * 1024,
            Self::Greedy => 1024 * 1024 * 1024,
        }
    }

    /// Per-worker pooled `gix` decoded-object cache limit in bytes.
    #[must_use]
    pub const fn worker_object_cache_bytes(self) -> usize {
        match self {
            Self::Lean => 16 * 1024 * 1024,
            Self::Balanced => 64 * 1024 * 1024,
            Self::Greedy => 128 * 1024 * 1024,
        }
    }

    /// Per-worker pooled `gix` pack delta-base cache limit in bytes.
    #[must_use]
    pub const fn worker_delta_pack_cache_bytes(self) -> usize {
        match self {
            Self::Lean => 16 * 1024 * 1024,
            Self::Balanced => 64 * 1024 * 1024,
            Self::Greedy => 128 * 1024 * 1024,
        }
    }

    /// Entry capacity for `GitLruCache` commit diffs.
    #[must_use]
    pub const fn diff_cache_capacity(self) -> usize {
        match self {
            Self::Lean => 64,
            Self::Balanced => 512,
            Self::Greedy => 2048,
        }
    }

    /// Byte budget for `GitLruCache` commit diffs.
    #[must_use]
    pub const fn diff_cache_byte_budget(self) -> usize {
        match self {
            Self::Lean => 32 * 1024 * 1024,
            Self::Balanced => 128 * 1024 * 1024,
            Self::Greedy => 512 * 1024 * 1024,
        }
    }

    /// Entry capacity for `GitLruCache` tree listings.
    #[must_use]
    pub const fn tree_cache_capacity(self) -> usize {
        match self {
            Self::Lean => 64,
            Self::Balanced => 512,
            Self::Greedy => 2048,
        }
    }

    /// Byte budget for `GitLruCache` tree listings.
    #[must_use]
    pub const fn tree_cache_byte_budget(self) -> usize {
        match self {
            Self::Lean => 16 * 1024 * 1024,
            Self::Balanced => 64 * 1024 * 1024,
            Self::Greedy => 128 * 1024 * 1024,
        }
    }

    /// Entry capacity for `GitLruCache` decoded blobs and raw lines.
    #[must_use]
    pub const fn blob_cache_capacity(self) -> usize {
        match self {
            Self::Lean => 32,
            Self::Balanced => 256,
            Self::Greedy => 512,
        }
    }

    /// Byte budget for `GitLruCache` decoded blobs and raw lines.
    #[must_use]
    pub const fn blob_cache_byte_budget(self) -> usize {
        match self {
            Self::Lean => 64 * 1024 * 1024,
            Self::Balanced => 128 * 1024 * 1024,
            Self::Greedy => 256 * 1024 * 1024,
        }
    }

    /// Entry capacity for `GitLruCache` blame results (`0` disables caching in `lean`).
    #[must_use]
    pub const fn blame_cache_capacity(self) -> usize {
        match self {
            Self::Lean => 0,
            Self::Balanced => 32,
            Self::Greedy => 128,
        }
    }

    /// Byte budget for `GitLruCache` blame results.
    #[must_use]
    pub const fn blame_cache_byte_budget(self) -> usize {
        match self {
            Self::Lean => 0,
            Self::Balanced => 64 * 1024 * 1024,
            Self::Greedy => 256 * 1024 * 1024,
        }
    }

    /// Entry capacity for `DiffDocumentCache`.
    #[must_use]
    pub const fn diff_document_cache_capacity(self) -> usize {
        match self {
            Self::Lean => 32,
            Self::Balanced => 256,
            Self::Greedy => 1024,
        }
    }

    /// Byte budget for `DiffDocumentCache`.
    #[must_use]
    pub const fn diff_document_cache_byte_budget(self) -> usize {
        match self {
            Self::Lean => 64 * 1024 * 1024,
            Self::Balanced => 128 * 1024 * 1024,
            Self::Greedy => 512 * 1024 * 1024,
        }
    }

    /// Maximum cumulative syntax-highlighted lines across a single `CommitDiff`.
    ///
    /// Set to 2,500 lines (~50 full terminal screens) across all memory profiles so
    /// multi-file commits are fully highlighted end-to-end while still bounding huge
    /// uncommitted working-tree diffs (100+ files / 30,000+ lines).
    #[must_use]
    pub const fn max_diff_highlight_total_lines(self) -> usize {
        match self {
            Self::Lean | Self::Balanced | Self::Greedy => 2_500,
        }
    }

    /// Batch size for streaming commit history revwalk chunks.
    #[must_use]
    pub const fn revwalk_chunk_size(self) -> usize {
        match self {
            Self::Lean => 250,
            Self::Balanced | Self::Greedy => 512,
        }
    }

    /// Maximum extra revwalk batches absorbed per UI tick when no input is pending.
    #[must_use]
    pub const fn revwalk_absorb_batches(self) -> usize {
        match self {
            Self::Lean => 2,
            Self::Balanced => 16,
            Self::Greedy => 32,
        }
    }

    /// Speculative diff prefetch radius (`±N` commits around the cursor).
    #[must_use]
    pub const fn prefetch_window_radius(self) -> usize {
        match self {
            Self::Lean => 1,
            Self::Balanced => 8,
            Self::Greedy => 16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_spelling_round_trips_through_parsing() {
        for variant in DiffLayout::ALL {
            assert_eq!(variant.as_str().parse::<DiffLayout>().unwrap(), *variant);
        }
        for variant in IgnoreSpace::ALL {
            assert_eq!(variant.as_str().parse::<IgnoreSpace>().unwrap(), *variant);
        }
        for variant in CommitOrder::ALL {
            assert_eq!(variant.as_str().parse::<CommitOrder>().unwrap(), *variant);
        }
        for variant in DiffIndicator::ALL {
            assert_eq!(variant.as_str().parse::<DiffIndicator>().unwrap(), *variant);
        }
    }

    #[test]
    fn test_aliases_and_case_insensitivity() {
        assert_eq!(
            "SIDE_TO_SIDE".parse::<DiffLayout>().unwrap(),
            DiffLayout::SideBySide
        );
        assert_eq!("Single".parse::<DiffLayout>().unwrap(), DiffLayout::Unified);
        assert_eq!(
            "  auto  ".parse::<CommitOrder>().unwrap(),
            CommitOrder::Default
        );
        assert_eq!("UTF8".parse::<LineGraphics>().unwrap(), LineGraphics::Utf8);
        assert_eq!("On".parse::<DiffIndicator>().unwrap(), DiffIndicator::Yes);
    }

    /// A typo must be reported, not silently swallowed into the default.
    #[test]
    fn test_unknown_value_is_rejected_with_a_helpful_message() {
        let err = "sidebyside".parse::<DiffLayout>().unwrap_err();
        assert_eq!(err.value, "sidebyside");
        let text = err.to_string();
        assert!(text.contains("DiffLayout"), "{text}");
        assert!(text.contains("side-by-side"), "{text}");
    }

    #[test]
    fn test_next_cycles_in_declaration_order_and_wraps() {
        assert_eq!(CommitOrder::Default.next(), CommitOrder::Topo);
        assert_eq!(CommitOrder::Topo.next(), CommitOrder::Date);
        assert_eq!(CommitOrder::Date.next(), CommitOrder::AuthorDate);
        assert_eq!(CommitOrder::AuthorDate.next(), CommitOrder::Reverse);
        assert_eq!(CommitOrder::Reverse.next(), CommitOrder::Default);

        assert_eq!(LineGraphics::Utf8.next(), LineGraphics::Ascii);
        assert_eq!(LineGraphics::Ascii.next(), LineGraphics::Utf8);
    }

    #[test]
    fn test_serde_uses_canonical_strings_and_accepts_bools() {
        #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
        struct Wrapper {
            layout: DiffLayout,
            indicator: DiffIndicator,
        }

        let toml_text = "layout = \"side_to_side\"\nindicator = true\n";
        let parsed: Wrapper = toml::from_str(toml_text).unwrap();
        assert_eq!(parsed.layout, DiffLayout::SideBySide);
        assert_eq!(parsed.indicator, DiffIndicator::Yes);

        let emitted = toml::to_string(&parsed).unwrap();
        assert!(emitted.contains("layout = \"side-by-side\""), "{emitted}");
        assert!(emitted.contains("indicator = \"yes\""), "{emitted}");
    }

    #[test]
    fn test_serde_rejects_unknown_value() {
        let err = toml::from_str::<DiffPresentation>("\"plaid\"").unwrap_err();
        assert!(err.to_string().contains("plaid"), "{err}");
    }
}
