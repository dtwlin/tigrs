// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Theme and styling definitions for diff presentation.
//!
//! Provides `DiffStyle` and `DiffTheme` modeling colors, attributes, and
//! presentation modes across different terminal capabilities.

use crate::headless::{CellAttrs, Color};
use crate::options::{DiffIndicator, DiffPresentation};
use crate::term_cap::{ColorProfile, TerminalCapabilities};

/// Text attribute flags for diff styling.
///
/// Diff styles and rendered terminal cells describe the exact same SGR
/// attribute set, so this is an alias rather than a parallel type.
pub type Attrs = CellAttrs;

/// A combined foreground, background, and attribute style specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffStyle {
    /// Foreground color.
    pub fg: Option<Color>,
    /// Background color.
    pub bg: Option<Color>,
    /// Font / text attributes.
    pub attrs: Attrs,
}

impl DiffStyle {
    /// Creates a style with only a foreground color.
    #[must_use]
    pub const fn fg(color: Color) -> Self {
        Self {
            fg: Some(color),
            bg: None,
            attrs: Attrs::empty(),
        }
    }

    /// Creates a style with a foreground color and attributes.
    #[must_use]
    pub const fn fg_attrs(color: Color, attrs: Attrs) -> Self {
        Self {
            fg: Some(color),
            bg: None,
            attrs,
        }
    }

    /// Creates a style with only text attributes, preserving the terminal's
    /// default foreground and background colors (`\x1b[39m` / `\x1b[49m`).
    #[must_use]
    pub const fn attrs_only(attrs: Attrs) -> Self {
        Self {
            fg: None,
            bg: None,
            attrs,
        }
    }

    /// Creates a style with reverse video.
    #[must_use]
    pub const fn reverse(fg: Option<Color>, bg: Option<Color>) -> Self {
        Self {
            fg,
            bg,
            attrs: Attrs::REVERSE,
        }
    }
}

/// A custom dynamic line prefix color rule matching upstream Tig `color "<prefix>" fg bg [attr]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineColorRule {
    /// Prefix to match (case-insensitively).
    pub prefix: String,
    /// Style to apply when line matches.
    pub style: DiffStyle,
}

/// Theme describing the color and formatting rules for diff rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffTheme {
    /// Commit metadata and generic diff headers.
    pub header: DiffStyle,
    /// Commit ID header (`commit <hash>`).
    pub commit: DiffStyle,
    /// Merge commit parents header (`Merge: <parents>`).
    pub merge: DiffStyle,
    /// Author name and email headers.
    pub author: DiffStyle,
    /// Committer name and email headers.
    pub committer: DiffStyle,
    /// Author / committer date headers.
    pub date: DiffStyle,
    /// First line of commit message (subject).
    pub message_title: DiffStyle,
    /// Body lines of commit message.
    pub message_body: DiffStyle,
    /// Git commit message trailer (e.g. `Signed-off-by:`, `Reviewed-by:`).
    pub trailer: DiffStyle,
    /// Single file line in the diffstat summary table.
    pub stat_file: DiffStyle,
    /// Aggregated diffstat summary line.
    pub stat_summary: DiffStyle,
    /// Text exceeding commit title overflow column limit.
    pub overflow: DiffStyle,
    /// Old file path / removal header.
    pub file_old: DiffStyle,
    /// New file path / addition header.
    pub file_new: DiffStyle,
    /// Hunk range header (`@@ ... @@`).
    pub hunk_header: DiffStyle,
    /// Unchanged context lines.
    pub context: DiffStyle,
    /// Added diff lines.
    pub add: DiffStyle,
    /// Removed diff lines.
    pub del: DiffStyle,
    /// Intra-line added word emphasis.
    pub add_emphasis: DiffStyle,
    /// Intra-line removed word emphasis.
    pub del_emphasis: DiffStyle,
    /// Rule lines, separators, and delimiters.
    pub delimiter: DiffStyle,
    /// Line number gutter.
    pub gutter: DiffStyle,
    /// Empty line block indicator for added/removed whitespace-only lines.
    pub empty_block: DiffStyle,
    /// Whether leading `+`/`-`/` ` sign characters should be stripped during paint.
    pub strip_signs: bool,
    /// Custom dynamic line prefix rules (evaluated in order).
    pub custom_rules: Vec<LineColorRule>,
    /// Negotiated terminal color profile for downsampling syntax colors.
    pub profile: ColorProfile,
}

impl DiffTheme {
    /// Classic upstream Tig diff theme (color-per-line, leading signs preserved).
    #[must_use]
    pub fn classic() -> Self {
        Self {
            header: DiffStyle::fg(Color::Yellow),
            commit: DiffStyle::fg(Color::Green),
            merge: DiffStyle::fg(Color::Blue),
            author: DiffStyle::fg(Color::Cyan),
            committer: DiffStyle::fg(Color::Magenta),
            date: DiffStyle::fg(Color::Yellow),
            message_title: DiffStyle::fg_attrs(Color::White, Attrs::BOLD),
            message_body: DiffStyle::default(),
            trailer: DiffStyle::fg(Color::Yellow),
            stat_file: DiffStyle::default(),
            stat_summary: DiffStyle::fg_attrs(Color::White, Attrs::BOLD),
            overflow: DiffStyle::fg(Color::Red),
            file_old: DiffStyle::fg(Color::Red),
            file_new: DiffStyle::fg(Color::Green),
            hunk_header: DiffStyle::fg(Color::Cyan),
            context: DiffStyle::default(),
            add: DiffStyle::fg(Color::Green),
            del: DiffStyle::fg(Color::Red),
            add_emphasis: DiffStyle {
                fg: None,
                bg: Some(Color::Ansi256(22)),
                attrs: Attrs::BOLD,
            },
            del_emphasis: DiffStyle {
                fg: None,
                bg: Some(Color::Ansi256(52)),
                attrs: Attrs::BOLD,
            },
            delimiter: DiffStyle::attrs_only(Attrs::DIM),
            gutter: DiffStyle::attrs_only(Attrs::DIM),
            empty_block: DiffStyle::reverse(Some(Color::Green), None),
            strip_signs: false,
            custom_rules: Vec::new(),
            profile: ColorProfile::TrueColor,
        }
    }

    /// Fancy diff-so-fancy style theme (rule banners, human hunk headers, stripped signs).
    #[must_use]
    pub fn fancy() -> Self {
        Self {
            header: DiffStyle::fg_attrs(Color::Yellow, Attrs::BOLD),
            commit: DiffStyle::fg_attrs(Color::Green, Attrs::BOLD),
            merge: DiffStyle::fg_attrs(Color::Blue, Attrs::BOLD),
            author: DiffStyle::fg_attrs(Color::Cyan, Attrs::BOLD),
            committer: DiffStyle::fg_attrs(Color::Magenta, Attrs::BOLD),
            date: DiffStyle::fg(Color::Yellow),
            message_title: DiffStyle::fg_attrs(Color::BrightWhite, Attrs::BOLD),
            message_body: DiffStyle::default(),
            trailer: DiffStyle::fg_attrs(Color::Yellow, Attrs::BOLD),
            stat_file: DiffStyle::default(),
            stat_summary: DiffStyle::fg_attrs(Color::White, Attrs::BOLD),
            overflow: DiffStyle::fg_attrs(Color::BrightRed, Attrs::BOLD),
            file_old: DiffStyle::fg_attrs(Color::Red, Attrs::BOLD),
            file_new: DiffStyle::fg_attrs(Color::Green, Attrs::BOLD),
            hunk_header: DiffStyle::fg_attrs(Color::Cyan, Attrs::BOLD),
            context: DiffStyle::default(),
            add: DiffStyle::fg(Color::Green),
            del: DiffStyle::fg(Color::Red),
            add_emphasis: DiffStyle::fg_attrs(Color::BrightGreen, Attrs::BOLD),
            del_emphasis: DiffStyle::fg_attrs(Color::BrightRed, Attrs::BOLD),
            delimiter: DiffStyle::attrs_only(Attrs::DIM),
            gutter: DiffStyle::attrs_only(Attrs::DIM),
            empty_block: DiffStyle::reverse(Some(Color::Green), None),
            strip_signs: true,
            custom_rules: Vec::new(),
            profile: ColorProfile::TrueColor,
        }
    }

    /// Gerrit-style single-line banner theme (compact file banner, human hunk separators,
    /// leading `+`/`-` signs preserved by default in Unified mode).
    #[must_use]
    pub fn banner() -> Self {
        let mut theme = Self::classic();
        theme.header = DiffStyle::fg_attrs(Color::Yellow, Attrs::BOLD);
        theme.hunk_header = DiffStyle::fg_attrs(Color::Cyan, Attrs::BOLD);
        theme
    }

    /// Applies color overrides and custom line rules from a Tig/Tigrs configuration color map.
    pub fn apply_custom_colors(
        &mut self,
        colors: &indexmap::IndexMap<String, tigrs_core::ColorSpec>,
    ) {
        for (area, spec) in colors {
            let style = style_from_spec(spec);
            let key = area.trim();
            match key.to_ascii_lowercase().as_str() {
                "diff-header" | "diff_header" => self.header = style,
                "diff-add" | "diff_add" => self.add = style,
                "diff-del" | "diff_del" => self.del = style,
                "diff-stat" | "diff_stat" => {
                    self.stat_file = style;
                    self.stat_summary = style;
                }
                "commit" => self.commit = style,
                "author" => self.author = style,
                "committer" => self.committer = style,
                "date" => self.date = style,
                "trailer" => self.trailer = style,
                "delimiter" => self.delimiter = style,
                "line-number" | "line_number" | "gutter" => self.gutter = style,
                "overflow" => self.overflow = style,
                "hunk-header" | "hunk_header" => self.hunk_header = style,
                "file-old" | "file_old" => self.file_old = style,
                "file-new" | "file_new" => self.file_new = style,
                _ => {
                    let unquoted = key.trim_matches('"');
                    match unquoted.to_ascii_lowercase().as_str() {
                        "author: " => self.author = style,
                        "commit: " | "committer " | "tagger: " => self.committer = style,
                        "merge: " | "parent " | "tree " => self.merge = style,
                        "date: " | "authordate: " | "commitdate: " | "taggerdate: " => {
                            self.date = style;
                        }
                        "commit " => self.commit = style,
                        _ => {
                            self.custom_rules.push(LineColorRule {
                                prefix: unquoted.to_string(),
                                style,
                            });
                        }
                    }
                }
            }
        }
    }

    /// Applies semantic colors from a [`crate::ui_theme::UiPalette`] onto this diff theme.
    pub fn apply_ui_palette(&mut self, palette: &crate::ui_theme::UiPalette) {
        if palette.id == tigrs_core::UiThemeId::Default {
            return;
        }
        let body_style = DiffStyle {
            fg: palette.text_fg,
            bg: palette.canvas_bg,
            attrs: Attrs::empty(),
        };
        self.header = palette.diff_header;
        self.commit = DiffStyle {
            fg: Some(palette.status_staged_fg),
            bg: palette.canvas_bg,
            attrs: if self.strip_signs {
                Attrs::BOLD
            } else {
                Attrs::empty()
            },
        };
        self.merge = DiffStyle {
            fg: Some(palette.tree_dir_fg),
            bg: palette.canvas_bg,
            attrs: if self.strip_signs {
                Attrs::BOLD
            } else {
                Attrs::empty()
            },
        };
        self.author = DiffStyle {
            fg: Some(palette.author_fg),
            bg: palette.canvas_bg,
            attrs: if self.strip_signs {
                Attrs::BOLD
            } else {
                Attrs::empty()
            },
        };
        self.committer = DiffStyle {
            fg: Some(palette.committer_fg),
            bg: palette.canvas_bg,
            attrs: if self.strip_signs {
                Attrs::BOLD
            } else {
                Attrs::empty()
            },
        };
        self.date = DiffStyle {
            fg: Some(palette.date_fg),
            bg: palette.canvas_bg,
            attrs: Attrs::empty(),
        };
        self.message_title = palette.diff_message_title;
        self.message_body = body_style;
        self.trailer = palette.diff_header;
        self.stat_file = body_style;
        self.stat_summary = palette.diff_stat_summary;
        self.overflow = DiffStyle {
            fg: Some(palette.title_overflow_fg),
            bg: palette.canvas_bg,
            attrs: Attrs::BOLD,
        };
        self.file_old = DiffStyle {
            fg: Some(palette.status_unstaged_fg),
            bg: palette.canvas_bg,
            attrs: Attrs::BOLD,
        };
        self.file_new = DiffStyle {
            fg: Some(palette.status_staged_fg),
            bg: palette.canvas_bg,
            attrs: Attrs::BOLD,
        };
        self.hunk_header = palette.diff_hunk_header;
        self.context = body_style;
        self.add = palette.diff_add;
        self.del = palette.diff_del;
        self.add_emphasis = palette.diff_add_emphasis;
        self.del_emphasis = palette.diff_del_emphasis;
        self.delimiter = palette.muted_style;
        self.gutter = palette.muted_style;
    }

    /// Resolves base theme according to `presentation`.
    #[must_use]
    pub fn for_presentation(presentation: DiffPresentation) -> Self {
        match presentation {
            DiffPresentation::Banner => Self::banner(),
            DiffPresentation::Classic => Self::classic(),
            DiffPresentation::Fancy => Self::fancy(),
        }
    }

    /// Builds a complete [`DiffTheme`] from [`crate::options::ViewOptions`] and [`TerminalCapabilities`],
    /// layering the active `ui_theme` palette and any custom `[colors]` overrides.
    #[must_use]
    pub fn from_options(
        options: &crate::options::ViewOptions,
        caps: &TerminalCapabilities,
    ) -> Self {
        let mut theme = Self::for_presentation(options.diff_presentation)
            .for_capabilities(caps, options.diff_indicator);
        let palette = options.ui_palette();
        theme.apply_ui_palette(&palette);
        if !options.colors.is_empty() {
            theme.apply_custom_colors(&options.colors);
        }
        theme
    }

    /// Adapts the theme to negotiated terminal capabilities and user indicator choice.
    ///
    /// Non-negotiable architectural invariant:
    /// When `ColorProfile::Monochrome` is detected (e.g. `NO_COLOR`, `TERM=dumb`),
    /// `strip_signs` is unconditionally force-disabled because sign characters are
    /// the only remaining disambiguation signal.
    #[must_use]
    pub fn for_capabilities(
        mut self,
        caps: &TerminalCapabilities,
        indicator: DiffIndicator,
    ) -> Self {
        self.profile = caps.color_profile;
        if caps.color_profile == ColorProfile::Monochrome {
            // Hard forced-enable override: sign stripping is forbidden in monochrome!
            self.strip_signs = false;
        } else {
            match indicator {
                DiffIndicator::Auto => {
                    // Respect theme default (false in classic, true in fancy)
                }
                DiffIndicator::Yes => {
                    self.strip_signs = false;
                }
                DiffIndicator::No => {
                    self.strip_signs = true;
                }
            }
        }
        self
    }
}

/// Maps a `config.toml` attribute name to its [`Attrs`] bit.
///
/// Unknown names are ignored so a typo in a color rule degrades to "no extra
/// attribute" rather than failing the whole theme.
fn attr_from_config_name(name: &str) -> Option<Attrs> {
    match name.to_ascii_lowercase().as_str() {
        "bold" => Some(Attrs::BOLD),
        "dim" => Some(Attrs::DIM),
        "italic" => Some(Attrs::ITALIC),
        "underline" => Some(Attrs::UNDERLINE),
        "reverse" | "standout" => Some(Attrs::REVERSE),
        "strike" | "strikethrough" => Some(Attrs::STRIKE),
        _ => None,
    }
}

/// Parses a [`tigrs_core::ColorSpec`] into a [`DiffStyle`].
#[must_use]
pub fn style_from_spec(spec: &tigrs_core::ColorSpec) -> DiffStyle {
    match spec {
        tigrs_core::ColorSpec::String(s) => {
            let fg = Color::parse(s);
            DiffStyle {
                fg,
                bg: None,
                attrs: Attrs::default(),
            }
        }
        tigrs_core::ColorSpec::Table { fg, bg, attributes } => {
            let fg_col = Color::parse(fg);
            let bg_col = Color::parse(bg);
            let attrs = attributes
                .iter()
                .filter_map(|attr| attr_from_config_name(attr))
                .fold(Attrs::empty(), std::ops::BitOr::bitor);
            DiffStyle {
                fg: fg_col,
                bg: bg_col,
                attrs,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_style_constructors_and_attr_parsing() {
        let fg_only = DiffStyle::fg(Color::Green);
        assert_eq!(fg_only.fg, Some(Color::Green));
        assert_eq!(fg_only.bg, None);
        assert!(fg_only.attrs.is_empty());

        let fg_bold = DiffStyle::fg_attrs(Color::Red, Attrs::BOLD | Attrs::UNDERLINE);
        assert!(fg_bold.attrs.contains(Attrs::BOLD));
        assert!(fg_bold.attrs.contains(Attrs::UNDERLINE));

        assert_eq!(attr_from_config_name("BOLD"), Some(Attrs::BOLD));
        assert_eq!(attr_from_config_name("standout"), Some(Attrs::REVERSE));
        assert_eq!(attr_from_config_name("strikethrough"), Some(Attrs::STRIKE));
        assert_eq!(attr_from_config_name("unknown_attr"), None);
    }

    #[test]
    fn test_diff_theme_from_presentation_and_color_profiles() {
        let caps_tc = TerminalCapabilities {
            color_profile: ColorProfile::TrueColor,
            supports_kitty_keyboard: true,
            supports_unicode_box: true,
            supports_synchronized_output: true,
        };
        let caps_mono = TerminalCapabilities {
            color_profile: ColorProfile::Monochrome,
            supports_kitty_keyboard: false,
            supports_unicode_box: false,
            supports_synchronized_output: false,
        };

        // Fancy presentation with TrueColor and DiffIndicator::Auto strips +/- signs by default
        let mut fancy_tc = DiffTheme::for_presentation(DiffPresentation::Fancy)
            .for_capabilities(&caps_tc, DiffIndicator::Auto);
        assert!(
            fancy_tc.strip_signs,
            "Fancy TrueColor with Indicator::Auto must strip +/- signs"
        );
        assert!(fancy_tc.add_emphasis.attrs.contains(Attrs::BOLD));
        assert!(fancy_tc.empty_block.attrs.contains(Attrs::REVERSE));

        // Default gutter and delimiter use terminal default foreground (fg == None) + Attrs::DIM
        assert_eq!(fancy_tc.gutter, DiffStyle::attrs_only(Attrs::DIM));
        assert_eq!(fancy_tc.delimiter, DiffStyle::attrs_only(Attrs::DIM));

        // Test apply_custom_colors override (including line-number)
        let mut custom_colors = indexmap::IndexMap::new();
        custom_colors.insert(
            "diff-add".to_string(),
            tigrs_core::ColorSpec::String("cyan".to_string()),
        );
        custom_colors.insert(
            "line-number".to_string(),
            tigrs_core::ColorSpec::String("yellow".to_string()),
        );
        fancy_tc.apply_custom_colors(&custom_colors);
        assert_eq!(fancy_tc.add.fg, Some(Color::Cyan));
        assert_eq!(fancy_tc.gutter.fg, Some(Color::Yellow));

        // Monochrome terminal MUST NEVER strip +/- signs even if Indicator::No is requested
        let mono_theme = DiffTheme::for_presentation(DiffPresentation::Fancy)
            .for_capabilities(&caps_mono, DiffIndicator::No);
        assert!(
            !mono_theme.strip_signs,
            "Monochrome terminal must preserve +/- signs for accessibility"
        );

        // Classic presentation with Indicator::Yes preserves signs and uses adaptive DIM gutter
        let classic = DiffTheme::for_presentation(DiffPresentation::Classic)
            .for_capabilities(&caps_tc, DiffIndicator::Yes);
        assert!(!classic.strip_signs);
        assert_eq!(classic.gutter, DiffStyle::attrs_only(Attrs::DIM));
    }
}
