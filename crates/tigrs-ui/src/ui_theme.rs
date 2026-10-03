// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Semantic UI Color Theme system (`UiThemeId` & `UiPalette`) for `tigrs`.
//!
//! Provides 14 curated built-in palettes across 4 families:
//! - **Adaptive (`default`)**: Terminal-native ANSI palette preserving default fg/bg.
//! - **Dark (7 themes)**: `catppuccin-mocha`, `dracula`, `tokyo-night`, `gruvbox-dark`,
//!   `nord`, `solarized-dark`, `github-dark`.
//! - **Light (4 themes)**: `catppuccin-latte`, `github-light`, `solarized-light`, `gruvbox-light`.
//! - **High-Contrast WCAG AAA >= 10:1 (2 themes)**: `high-contrast-dark`, `high-contrast-light`.

use crate::diff::{Attrs, DiffStyle, style_from_spec};
use crate::headless::Color;
use tigrs_core::config_enums::UiThemeId;

/// Polarity and contrast family of a UI color theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeCategory {
    /// Inherits terminal emulator default foreground and background (`\x1b[39m` / `\x1b[49m`).
    Adaptive,
    /// Dark canvas background with light ink foreground.
    Dark,
    /// Light/daylight canvas background with dark ink foreground.
    Light,
    /// WCAG AAA (>= 10:1) pitch-black high-contrast dark palette.
    HighContrastDark,
    /// WCAG AAA (>= 10:1) pure-white high-contrast light palette.
    HighContrastLight,
}

/// Complete semantic color palette governing all 13 `tigrs` views, window chrome,
/// interactive overlays, and diff presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiPalette {
    /// Theme identifier.
    pub id: UiThemeId,
    /// Polarity and contrast category.
    pub category: ThemeCategory,
    /// Base canvas background (`None` in `default` adaptive mode to preserve terminal transparency).
    pub canvas_bg: Option<Color>,
    /// Primary body text foreground (`None` in `default` adaptive mode).
    pub text_fg: Option<Color>,
    /// Secondary/muted text and source line-number gutter style.
    pub muted_style: DiffStyle,

    // --- Window Chrome & Bars ---
    /// Focused pane top title bar style.
    pub title_bar_active: DiffStyle,
    /// Unfocused split pane top title bar style.
    pub title_bar_inactive: DiffStyle,
    /// Bottom status bar style.
    pub status_bar: DiffStyle,
    /// Vertical split separator column foreground color.
    pub split_separator_fg: Color,
    /// `[RO]` read-only mode indicator badge style.
    pub read_only_badge: DiffStyle,
    /// Selected cursor row highlight style across list and code views.
    pub cursor_row: DiffStyle,

    // --- Options & Config Panel (`o`) Drawer ---
    /// Top category tab bar in the Options drawer.
    pub drawer_header: DiffStyle,
    /// Unmodified option row in the Options drawer.
    pub drawer_row_normal: DiffStyle,
    /// Non-default (`*`) option row in the Options drawer.
    pub drawer_row_modified: DiffStyle,
    /// Unsaved (`●`) option row in the Options drawer.
    pub drawer_row_unsaved: DiffStyle,
    /// Currently focused cursor row in the Options drawer.
    pub drawer_row_selected: DiffStyle,
    /// Target TOML path & sync badge status row in the Options drawer.
    pub drawer_info_bar: DiffStyle,
    /// Bottom keybinding hint bar in the Options drawer.
    pub drawer_footer: DiffStyle,

    // --- Main / Log / Blame / Ref Columns ---
    /// Abbreviated commit SHA column foreground.
    pub commit_id_fg: Color,
    /// Timestamp / relative date column foreground.
    pub date_fg: Color,
    /// Commit author name/email column foreground.
    pub author_fg: Color,
    /// Committer name/email foreground.
    pub committer_fg: Color,
    /// 6-color cycle for commit DAG revision graph lanes.
    pub graph_lanes: [Color; 6],
    /// `HEAD` / current branch ref badge foreground.
    pub ref_head_fg: Color,
    /// Local branch ref badge foreground.
    pub ref_branch_fg: Color,
    /// Remote tracking branch ref badge foreground.
    pub ref_remote_fg: Color,
    /// Tag ref badge foreground.
    pub ref_tag_fg: Color,
    /// Stash ref foreground.
    pub ref_stash_fg: Color,
    /// Commit subject overflow warning foreground (past `commit-title-overflow` column).
    pub title_overflow_fg: Color,
    /// 10 contrast-verified categorical hues for deterministic author coloring (`P1`).
    pub author_hues: [Color; 10],
    /// 6-stop age heatmap (`<1h`, `today`, `<7d`, `<30d`, `<1y`, `older`) for the Date column (`P7`).
    pub date_heat: [Color; 6],
    /// Commit SHA foreground when ahead of upstream (`P6` unpushed).
    pub sha_unpushed_fg: Color,
    /// Commit SHA foreground when not yet merged into upstream (`P6` unmerged).
    pub sha_unmerged_fg: Color,

    // --- Status & Tree View Semantics ---
    /// Section header (`Changes to be committed`, `Unstaged changes`, etc.) in Status view.
    pub status_section_header: DiffStyle,
    /// Staged file (`M`, `A`, `D`, `R`) foreground in Status view.
    pub status_staged_fg: Color,
    /// Unstaged modified/deleted file foreground in Status view.
    pub status_unstaged_fg: Color,
    /// Untracked file (`?`) foreground in Status view.
    pub status_untracked_fg: Color,
    /// Directory entry foreground in Tree view.
    pub tree_dir_fg: Color,
    /// Symbolic link entry foreground in Tree view.
    pub tree_symlink_fg: Color,

    // --- Diff & Code Presentation ---
    /// Diff metadata header (`diff --git`, `index`, `---`, `+++`).
    pub diff_header: DiffStyle,
    /// Commit subject line in diff/log header.
    pub diff_message_title: DiffStyle,
    /// Diffstat summary line.
    pub diff_stat_summary: DiffStyle,
    /// Hunk range header (`@@ -a,b +c,d @@`).
    pub diff_hunk_header: DiffStyle,
    /// Added line (`+`) foreground/background style.
    pub diff_add: DiffStyle,
    /// Removed line (`-`) foreground/background style.
    pub diff_del: DiffStyle,
    /// Intra-line added word emphasis (`word-diff`).
    pub diff_add_emphasis: DiffStyle,
    /// Intra-line removed word emphasis (`word-diff`).
    pub diff_del_emphasis: DiffStyle,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

const fn style_fg_bg(fg: Color, bg: Color, attrs: Attrs) -> DiffStyle {
    DiffStyle {
        fg: Some(fg),
        bg: Some(bg),
        attrs,
    }
}

impl UiPalette {
    /// Constructs the curated [`UiPalette`] for the given [`UiThemeId`].
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn for_theme(id: UiThemeId) -> Self {
        match id {
            UiThemeId::Default => Self::adaptive_default(),
            UiThemeId::CatppuccinMocha => Self::catppuccin_mocha(),
            UiThemeId::Dracula => Self::dracula(),
            UiThemeId::TokyoNight => Self::tokyo_night(),
            UiThemeId::GruvboxDark => Self::gruvbox_dark(),
            UiThemeId::Nord => Self::nord(),
            UiThemeId::SolarizedDark => Self::solarized_dark(),
            UiThemeId::GithubDark => Self::github_dark(),
            UiThemeId::CatppuccinLatte => Self::catppuccin_latte(),
            UiThemeId::GithubLight => Self::github_light(),
            UiThemeId::SolarizedLight => Self::solarized_light(),
            UiThemeId::GruvboxLight => Self::gruvbox_light(),
            UiThemeId::HighContrastDark => Self::high_contrast_dark(),
            UiThemeId::HighContrastLight => Self::high_contrast_light(),
        }
    }

    /// 1. Adaptive terminal-native default palette.
    #[must_use]
    pub const fn adaptive_default() -> Self {
        Self {
            id: UiThemeId::Default,
            category: ThemeCategory::Adaptive,
            canvas_bg: None,
            text_fg: None,
            muted_style: DiffStyle::attrs_only(Attrs::DIM),
            title_bar_active: DiffStyle::attrs_only(Attrs::BOLD),
            title_bar_inactive: DiffStyle::fg(Color::BrightBlack),
            status_bar: DiffStyle::attrs_only(Attrs::REVERSE),
            split_separator_fg: Color::Cyan,
            read_only_badge: DiffStyle {
                fg: Some(Color::Cyan),
                bg: None,
                attrs: Attrs::REVERSE.union(Attrs::BOLD),
            },
            cursor_row: DiffStyle::attrs_only(Attrs::REVERSE),
            drawer_header: style_fg_bg(Color::White, Color::Blue, Attrs::BOLD),
            drawer_row_normal: style_fg_bg(Color::White, Color::Black, Attrs::empty()),
            drawer_row_modified: style_fg_bg(Color::Cyan, Color::Black, Attrs::empty()),
            drawer_row_unsaved: style_fg_bg(Color::Yellow, Color::Black, Attrs::empty()),
            drawer_row_selected: DiffStyle::attrs_only(Attrs::BOLD.union(Attrs::REVERSE)),
            drawer_info_bar: style_fg_bg(Color::Cyan, Color::Black, Attrs::empty()),
            drawer_footer: DiffStyle::attrs_only(Attrs::REVERSE),
            commit_id_fg: Color::Yellow,
            date_fg: Color::Yellow,
            author_fg: Color::Cyan,
            committer_fg: Color::Magenta,
            graph_lanes: [
                Color::Green,
                Color::Yellow,
                Color::Cyan,
                Color::Magenta,
                Color::Blue,
                Color::Red,
            ],
            ref_head_fg: Color::Cyan,
            ref_branch_fg: Color::Green,
            ref_remote_fg: Color::Red,
            ref_tag_fg: Color::Yellow,
            ref_stash_fg: Color::Magenta,
            title_overflow_fg: Color::Red,
            author_hues: [
                Color::Cyan,
                Color::Yellow,
                Color::Green,
                Color::Magenta,
                Color::Blue,
                Color::BrightCyan,
                Color::BrightYellow,
                Color::BrightGreen,
                Color::BrightMagenta,
                Color::BrightBlue,
            ],
            date_heat: [
                Color::BrightGreen,
                Color::Green,
                Color::Cyan,
                Color::Yellow,
                Color::Blue,
                Color::BrightBlack,
            ],
            sha_unpushed_fg: Color::BrightGreen,
            sha_unmerged_fg: Color::BrightMagenta,
            status_section_header: DiffStyle::fg_attrs(Color::Yellow, Attrs::BOLD),
            status_staged_fg: Color::Green,
            status_unstaged_fg: Color::Red,
            status_untracked_fg: Color::Magenta,
            tree_dir_fg: Color::Blue,
            tree_symlink_fg: Color::Cyan,
            diff_header: DiffStyle::fg(Color::Yellow),
            diff_message_title: DiffStyle::attrs_only(Attrs::BOLD),
            diff_stat_summary: DiffStyle::attrs_only(Attrs::BOLD),
            diff_hunk_header: DiffStyle::fg(Color::Cyan),
            diff_add: DiffStyle::fg(Color::Green),
            diff_del: DiffStyle::fg(Color::Red),
            diff_add_emphasis: DiffStyle {
                fg: None,
                bg: Some(Color::Ansi256(22)),
                attrs: Attrs::BOLD,
            },
            diff_del_emphasis: DiffStyle {
                fg: None,
                bg: Some(Color::Ansi256(52)),
                attrs: Attrs::BOLD,
            },
        }
    }

    /// 2. Catppuccin Mocha (Dark pastel).
    #[must_use]
    pub const fn catppuccin_mocha() -> Self {
        let base = rgb(0x1e, 0x1e, 0x2e);
        let mantle = rgb(0x18, 0x18, 0x25);
        let surface0 = rgb(0x31, 0x32, 0x44);
        let surface1 = rgb(0x45, 0x47, 0x5a);
        let overlay0 = rgb(0x6c, 0x70, 0x86);
        let text = rgb(0xcd, 0xd6, 0xf4);
        let blue = rgb(0x89, 0xb4, 0xfa);
        let sapphire = rgb(0x74, 0xc7, 0xec);
        let green = rgb(0xa6, 0xe3, 0xa1);
        let red = rgb(0xf3, 0x8b, 0xa8);
        let yellow = rgb(0xf9, 0xe2, 0xaf);
        let mauve = rgb(0xcb, 0xa6, 0xf7);
        let peach = rgb(0xfa, 0xb3, 0x87);
        let add_bg = rgb(0x28, 0x3b, 0x34);
        let del_bg = rgb(0x41, 0x29, 0x34);

        Self::build_palette(
            UiThemeId::CatppuccinMocha,
            ThemeCategory::Dark,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            sapphire,
            green,
            red,
            yellow,
            mauve,
            peach,
            add_bg,
            del_bg,
        )
    }

    /// 3. Dracula (Vibrant dark purple/cyan).
    #[must_use]
    pub const fn dracula() -> Self {
        let base = rgb(0x28, 0x2a, 0x36);
        let mantle = rgb(0x21, 0x22, 0x2c);
        let surface0 = rgb(0x38, 0x3a, 0x4c);
        let surface1 = rgb(0x44, 0x47, 0x5a);
        let overlay0 = rgb(0x62, 0x72, 0xa4);
        let text = rgb(0xf8, 0xf8, 0xf2);
        let purple = rgb(0xbd, 0x93, 0xf9);
        let cyan = rgb(0x8b, 0xe9, 0xfd);
        let green = rgb(0x50, 0xfa, 0x7b);
        let red = rgb(0xff, 0x55, 0x55);
        let yellow = rgb(0xf1, 0xfa, 0x8c);
        let pink = rgb(0xff, 0x79, 0xc6);
        let orange = rgb(0xff, 0xb8, 0x6c);
        let add_bg = rgb(0x1f, 0x47, 0x2f);
        let del_bg = rgb(0x4c, 0x22, 0x29);

        Self::build_palette(
            UiThemeId::Dracula,
            ThemeCategory::Dark,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            purple,
            cyan,
            green,
            red,
            yellow,
            pink,
            orange,
            add_bg,
            del_bg,
        )
    }

    /// 4. Tokyo Night (Modern indigo dark).
    #[must_use]
    pub const fn tokyo_night() -> Self {
        let base = rgb(0x1a, 0x1b, 0x26);
        let mantle = rgb(0x16, 0x16, 0x1e);
        let surface0 = rgb(0x24, 0x28, 0x3b);
        let surface1 = rgb(0x29, 0x2e, 0x42);
        let overlay0 = rgb(0x56, 0x5f, 0x89);
        let text = rgb(0xc0, 0xca, 0xf5);
        let blue = rgb(0x7a, 0xa2, 0xf7);
        let cyan = rgb(0x7d, 0xcf, 0xff);
        let green = rgb(0x9e, 0xce, 0x6a);
        let red = rgb(0xf7, 0x76, 0x8e);
        let yellow = rgb(0xe0, 0xaf, 0x68);
        let purple = rgb(0xbb, 0x9a, 0xf7);
        let orange = rgb(0xff, 0x9e, 0x64);
        let add_bg = rgb(0x20, 0x33, 0x2b);
        let del_bg = rgb(0x3c, 0x22, 0x2b);

        Self::build_palette(
            UiThemeId::TokyoNight,
            ThemeCategory::Dark,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            cyan,
            green,
            red,
            yellow,
            purple,
            orange,
            add_bg,
            del_bg,
        )
    }

    /// 5. Gruvbox Dark (Warm retro earthy dark).
    #[must_use]
    pub const fn gruvbox_dark() -> Self {
        let base = rgb(0x28, 0x28, 0x28);
        let mantle = rgb(0x1d, 0x20, 0x21);
        let surface0 = rgb(0x32, 0x30, 0x2f);
        let surface1 = rgb(0x50, 0x49, 0x45);
        let overlay0 = rgb(0x92, 0x83, 0x74);
        let text = rgb(0xeb, 0xdb, 0xb2);
        let blue = rgb(0x83, 0xa5, 0x98);
        let aqua = rgb(0x8e, 0xc0, 0x7c);
        let green = rgb(0xb8, 0xbb, 0x26);
        let red = rgb(0xfb, 0x49, 0x34);
        let yellow = rgb(0xfa, 0xbd, 0x2f);
        let purple = rgb(0xd3, 0x86, 0x9b);
        let orange = rgb(0xfe, 0x80, 0x19);
        let add_bg = rgb(0x32, 0x3c, 0x25);
        let del_bg = rgb(0x47, 0x23, 0x22);

        Self::build_palette(
            UiThemeId::GruvboxDark,
            ThemeCategory::Dark,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            aqua,
            green,
            red,
            yellow,
            purple,
            orange,
            add_bg,
            del_bg,
        )
    }

    /// 6. Nord (Arctic bluish dark).
    #[must_use]
    pub const fn nord() -> Self {
        let base = rgb(0x2e, 0x34, 0x40);
        let mantle = rgb(0x24, 0x29, 0x33);
        let surface0 = rgb(0x3b, 0x42, 0x52);
        let surface1 = rgb(0x43, 0x4c, 0x5e);
        let overlay0 = rgb(0x61, 0x6e, 0x88);
        let text = rgb(0xec, 0xef, 0xf4);
        let blue = rgb(0x81, 0xa1, 0xc1);
        let cyan = rgb(0x88, 0xc0, 0xd0);
        let green = rgb(0xa3, 0xbe, 0x8c);
        let red = rgb(0xbf, 0x61, 0x6a);
        let yellow = rgb(0xeb, 0xcb, 0x8b);
        let purple = rgb(0xb4, 0x8e, 0xad);
        let orange = rgb(0xd0, 0x87, 0x70);
        let add_bg = rgb(0x33, 0x44, 0x3c);
        let del_bg = rgb(0x47, 0x30, 0x38);

        Self::build_palette(
            UiThemeId::Nord,
            ThemeCategory::Dark,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            cyan,
            green,
            red,
            yellow,
            purple,
            orange,
            add_bg,
            del_bg,
        )
    }

    /// 7. Solarized Dark (Precision CIELAB dark).
    #[must_use]
    pub const fn solarized_dark() -> Self {
        let base = rgb(0x00, 0x2b, 0x36);
        let mantle = rgb(0x00, 0x21, 0x2b);
        let surface0 = rgb(0x07, 0x36, 0x42);
        let surface1 = rgb(0x0d, 0x46, 0x54);
        let overlay0 = rgb(0x58, 0x6e, 0x75);
        let text = rgb(0x93, 0xa1, 0xa1);
        let blue = rgb(0x26, 0x8b, 0xd2);
        let cyan = rgb(0x2a, 0xa1, 0x98);
        let green = rgb(0x85, 0x99, 0x00);
        let red = rgb(0xdc, 0x32, 0x2f);
        let yellow = rgb(0xb5, 0x89, 0x00);
        let magenta = rgb(0xd3, 0x36, 0x82);
        let orange = rgb(0xcb, 0x4b, 0x16);
        let add_bg = rgb(0x0b, 0x3d, 0x2e);
        let del_bg = rgb(0x3f, 0x22, 0x2c);

        Self::build_palette(
            UiThemeId::SolarizedDark,
            ThemeCategory::Dark,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            cyan,
            green,
            red,
            yellow,
            magenta,
            orange,
            add_bg,
            del_bg,
        )
    }

    /// 8. GitHub Primer Dark.
    #[must_use]
    pub const fn github_dark() -> Self {
        let base = rgb(0x0d, 0x11, 0x17);
        let mantle = rgb(0x01, 0x04, 0x09);
        let surface0 = rgb(0x16, 0x1b, 0x22);
        let surface1 = rgb(0x21, 0x26, 0x2d);
        let overlay0 = rgb(0x7d, 0x85, 0x90);
        let text = rgb(0xe6, 0xed, 0xf3);
        let blue = rgb(0x2f, 0x81, 0xf7);
        let cyan = rgb(0x39, 0xc5, 0xcf);
        let green = rgb(0x3f, 0xb9, 0x50);
        let red = rgb(0xf8, 0x51, 0x49);
        let yellow = rgb(0xd2, 0x99, 0x22);
        let purple = rgb(0xa3, 0x71, 0xf7);
        let orange = rgb(0xdb, 0x6d, 0x28);
        let add_bg = rgb(0x12, 0x2d, 0x1f);
        let del_bg = rgb(0x3c, 0x16, 0x18);

        Self::build_palette(
            UiThemeId::GithubDark,
            ThemeCategory::Dark,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            cyan,
            green,
            red,
            yellow,
            purple,
            orange,
            add_bg,
            del_bg,
        )
    }

    /// 9. Catppuccin Latte (Warm daylight pastel light).
    #[must_use]
    pub const fn catppuccin_latte() -> Self {
        let base = rgb(0xef, 0xf1, 0xf5);
        let mantle = rgb(0xe6, 0xe9, 0xef);
        let surface0 = rgb(0xcc, 0xd0, 0xda);
        let surface1 = rgb(0xbc, 0xc0, 0xcc);
        let overlay0 = rgb(0x7c, 0x7f, 0x93);
        let text = rgb(0x4c, 0x4f, 0x69);
        let blue = rgb(0x1e, 0x66, 0xf5);
        let sapphire = rgb(0x20, 0x9f, 0xb5);
        let green = rgb(0x40, 0xa0, 0x2b);
        let red = rgb(0xd2, 0x0f, 0x39);
        let yellow = rgb(0xdf, 0x8e, 0x1d);
        let mauve = rgb(0x88, 0x39, 0xef);
        let peach = rgb(0xfe, 0x64, 0x0b);
        let add_bg = rgb(0xcc, 0xe8, 0xcc);
        let del_bg = rgb(0xf9, 0xd0, 0xd8);

        Self::build_palette(
            UiThemeId::CatppuccinLatte,
            ThemeCategory::Light,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            sapphire,
            green,
            red,
            yellow,
            mauve,
            peach,
            add_bg,
            del_bg,
        )
    }

    /// 10. GitHub Primer Light.
    #[must_use]
    pub const fn github_light() -> Self {
        let base = rgb(0xff, 0xff, 0xff);
        let mantle = rgb(0xf6, 0xf8, 0xfa);
        let surface0 = rgb(0xea, 0xef, 0xf2);
        let surface1 = rgb(0xd0, 0xd7, 0xde);
        let overlay0 = rgb(0x65, 0x6d, 0x76);
        let text = rgb(0x1f, 0x23, 0x28);
        let blue = rgb(0x09, 0x69, 0xda);
        let cyan = rgb(0x05, 0x50, 0xae);
        let green = rgb(0x1a, 0x7f, 0x37);
        let red = rgb(0xcf, 0x22, 0x2e);
        let yellow = rgb(0x9a, 0x67, 0x00);
        let purple = rgb(0x82, 0x50, 0xdf);
        let orange = rgb(0xbc, 0x4c, 0x00);
        let add_bg = rgb(0xda, 0xfb, 0xe1);
        let del_bg = rgb(0xff, 0xeb, 0xe9);

        Self::build_palette(
            UiThemeId::GithubLight,
            ThemeCategory::Light,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            cyan,
            green,
            red,
            yellow,
            purple,
            orange,
            add_bg,
            del_bg,
        )
    }

    /// 11. Solarized Light (Precision warm ivory light).
    #[must_use]
    pub const fn solarized_light() -> Self {
        let base = rgb(0xfd, 0xf6, 0xe3);
        let mantle = rgb(0xee, 0xe8, 0xd5);
        let surface0 = rgb(0xe4, 0xde, 0xc8);
        let surface1 = rgb(0xd5, 0xcf, 0xb8);
        let overlay0 = rgb(0x93, 0xa1, 0xa1);
        let text = rgb(0x58, 0x6e, 0x75);
        let blue = rgb(0x26, 0x8b, 0xd2);
        let cyan = rgb(0x2a, 0xa1, 0x98);
        let green = rgb(0x85, 0x99, 0x00);
        let red = rgb(0xdc, 0x32, 0x2f);
        let yellow = rgb(0xb5, 0x89, 0x00);
        let magenta = rgb(0xd3, 0x36, 0x82);
        let orange = rgb(0xcb, 0x4b, 0x16);
        let add_bg = rgb(0xd9, 0xe6, 0xc3);
        let del_bg = rgb(0xf5, 0xd0, 0xcc);

        Self::build_palette(
            UiThemeId::SolarizedLight,
            ThemeCategory::Light,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            cyan,
            green,
            red,
            yellow,
            magenta,
            orange,
            add_bg,
            del_bg,
        )
    }

    /// 12. Gruvbox Light (Warm paper light).
    #[must_use]
    pub const fn gruvbox_light() -> Self {
        let base = rgb(0xfb, 0xf1, 0xc7);
        let mantle = rgb(0xf2, 0xe5, 0xbc);
        let surface0 = rgb(0xeb, 0xdb, 0xb2);
        let surface1 = rgb(0xd5, 0xc4, 0xa1);
        let overlay0 = rgb(0x7c, 0x6f, 0x64);
        let text = rgb(0x3c, 0x38, 0x36);
        let blue = rgb(0x07, 0x66, 0x78);
        let aqua = rgb(0x42, 0x7b, 0x58);
        let green = rgb(0x79, 0x74, 0x0e);
        let red = rgb(0x9d, 0x00, 0x06);
        let yellow = rgb(0xb5, 0x76, 0x14);
        let purple = rgb(0x8f, 0x3f, 0x71);
        let orange = rgb(0xaf, 0x3a, 0x03);
        let add_bg = rgb(0xdc, 0xe0, 0xa8);
        let del_bg = rgb(0xf2, 0xc2, 0xbe);

        Self::build_palette(
            UiThemeId::GruvboxLight,
            ThemeCategory::Light,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            aqua,
            green,
            red,
            yellow,
            purple,
            orange,
            add_bg,
            del_bg,
        )
    }

    /// 13. WCAG AAA High-Contrast Dark (>= 10:1 pitch-black canvas).
    #[must_use]
    pub const fn high_contrast_dark() -> Self {
        let base = rgb(0x00, 0x00, 0x00);
        let mantle = rgb(0x0a, 0x0a, 0x0a);
        let surface0 = rgb(0x14, 0x18, 0x24);
        let surface1 = rgb(0xff, 0xff, 0x00); // Crisp neon yellow cursor highlight
        let overlay0 = rgb(0xb8, 0xc4, 0xd0);
        let text = rgb(0xff, 0xff, 0xff);
        let blue = rgb(0x00, 0xe5, 0xff);
        let cyan = rgb(0x00, 0xff, 0xff);
        let green = rgb(0x00, 0xff, 0x66);
        let red = rgb(0xff, 0x44, 0x66);
        let yellow = rgb(0xff, 0xe6, 0x00);
        let magenta = rgb(0xff, 0x66, 0xff);
        let orange = rgb(0xff, 0x99, 0x00);
        let add_bg = rgb(0x00, 0x44, 0x1b);
        let del_bg = rgb(0x55, 0x00, 0x18);

        let mut p = Self::build_palette(
            UiThemeId::HighContrastDark,
            ThemeCategory::HighContrastDark,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            cyan,
            green,
            red,
            yellow,
            magenta,
            orange,
            add_bg,
            del_bg,
        );
        p.title_bar_active = style_fg_bg(base, cyan, Attrs::BOLD);
        p.status_bar = style_fg_bg(base, yellow, Attrs::BOLD);
        p.cursor_row = style_fg_bg(base, yellow, Attrs::BOLD);
        p.drawer_row_selected = style_fg_bg(base, yellow, Attrs::BOLD);
        p.diff_add = style_fg_bg(green, base, Attrs::BOLD);
        p.diff_del = style_fg_bg(red, base, Attrs::BOLD);
        p
    }

    /// 14. WCAG AAA High-Contrast Light (>= 10:1 pure-white canvas).
    #[must_use]
    pub const fn high_contrast_light() -> Self {
        let base = rgb(0xff, 0xff, 0xff);
        let mantle = rgb(0xf0, 0xf2, 0xf5);
        let surface0 = rgb(0xe1, 0xe6, 0xef);
        let surface1 = rgb(0x00, 0x22, 0x66);
        let overlay0 = rgb(0x38, 0x40, 0x48);
        let text = rgb(0x00, 0x00, 0x00);
        let blue = rgb(0x00, 0x22, 0x99);
        let cyan = rgb(0x00, 0x4d, 0x66);
        let green = rgb(0x00, 0x5a, 0x00);
        let red = rgb(0x99, 0x00, 0x00);
        let yellow = rgb(0x66, 0x3d, 0x00);
        let magenta = rgb(0x66, 0x00, 0x66);
        let orange = rgb(0x80, 0x2b, 0x00);
        let add_bg = rgb(0xb8, 0xf5, 0xb8);
        let del_bg = rgb(0xff, 0xc2, 0xc2);

        let mut p = Self::build_palette(
            UiThemeId::HighContrastLight,
            ThemeCategory::HighContrastLight,
            base,
            mantle,
            surface0,
            surface1,
            overlay0,
            text,
            blue,
            cyan,
            green,
            red,
            yellow,
            magenta,
            orange,
            add_bg,
            del_bg,
        );
        p.title_bar_active = style_fg_bg(base, blue, Attrs::BOLD);
        p.status_bar = style_fg_bg(base, rgb(0x00, 0x1a, 0x4d), Attrs::BOLD);
        p.cursor_row = style_fg_bg(base, blue, Attrs::BOLD);
        p.drawer_row_selected = style_fg_bg(base, blue, Attrs::BOLD);
        p.diff_add = style_fg_bg(green, base, Attrs::BOLD);
        p.diff_del = style_fg_bg(red, base, Attrs::BOLD);
        p
    }

    #[allow(clippy::too_many_arguments)]
    const fn build_palette(
        id: UiThemeId,
        category: ThemeCategory,
        base: Color,
        mantle: Color,
        surface0: Color,
        surface1: Color,
        overlay0: Color,
        text: Color,
        primary: Color,
        secondary: Color,
        green: Color,
        red: Color,
        yellow: Color,
        purple: Color,
        orange: Color,
        add_bg: Color,
        del_bg: Color,
    ) -> Self {
        let is_light = matches!(
            category,
            ThemeCategory::Light | ThemeCategory::HighContrastLight
        );
        let is_hc = matches!(
            category,
            ThemeCategory::HighContrastDark | ThemeCategory::HighContrastLight
        );
        let title_fg = if is_light { base } else { mantle };
        let author_hues = Self::build_author_hues(
            is_light, is_hc, secondary, yellow, green, purple, primary, orange, red, text,
        );
        let date_heat = [
            Self::ensure_contrast(green, is_light, is_hc),
            Self::ensure_contrast(secondary, is_light, is_hc),
            Self::ensure_contrast(primary, is_light, is_hc),
            Self::ensure_contrast(yellow, is_light, is_hc),
            Self::ensure_contrast(orange, is_light, is_hc),
            overlay0,
        ];
        Self {
            id,
            category,
            canvas_bg: Some(base),
            text_fg: Some(text),
            muted_style: style_fg_bg(overlay0, base, Attrs::empty()),
            title_bar_active: style_fg_bg(title_fg, primary, Attrs::BOLD),
            title_bar_inactive: style_fg_bg(overlay0, mantle, Attrs::empty()),
            status_bar: style_fg_bg(text, surface0, Attrs::BOLD),
            split_separator_fg: primary,
            read_only_badge: style_fg_bg(title_fg, secondary, Attrs::BOLD),
            cursor_row: style_fg_bg(text, surface1, Attrs::BOLD),
            drawer_header: style_fg_bg(title_fg, primary, Attrs::BOLD),
            drawer_row_normal: style_fg_bg(text, mantle, Attrs::empty()),
            drawer_row_modified: style_fg_bg(secondary, mantle, Attrs::empty()),
            drawer_row_unsaved: style_fg_bg(yellow, mantle, Attrs::BOLD),
            drawer_row_selected: style_fg_bg(text, surface1, Attrs::BOLD),
            drawer_info_bar: style_fg_bg(secondary, surface0, Attrs::empty()),
            drawer_footer: style_fg_bg(title_fg, secondary, Attrs::BOLD),
            commit_id_fg: yellow,
            date_fg: yellow,
            author_fg: secondary,
            committer_fg: purple,
            graph_lanes: [green, yellow, secondary, purple, primary, red],
            ref_head_fg: secondary,
            ref_branch_fg: green,
            ref_remote_fg: red,
            ref_tag_fg: yellow,
            ref_stash_fg: purple,
            title_overflow_fg: red,
            author_hues,
            date_heat,
            sha_unpushed_fg: Self::ensure_contrast(green, is_light, is_hc),
            sha_unmerged_fg: Self::ensure_contrast(orange, is_light, is_hc),
            status_section_header: style_fg_bg(yellow, base, Attrs::BOLD),
            status_staged_fg: green,
            status_unstaged_fg: red,
            status_untracked_fg: orange,
            tree_dir_fg: primary,
            tree_symlink_fg: secondary,
            diff_header: style_fg_bg(yellow, base, Attrs::BOLD),
            diff_message_title: style_fg_bg(text, base, Attrs::BOLD),
            diff_stat_summary: style_fg_bg(text, base, Attrs::BOLD),
            diff_hunk_header: style_fg_bg(secondary, base, Attrs::BOLD),
            diff_add: style_fg_bg(green, base, Attrs::empty()),
            diff_del: style_fg_bg(red, base, Attrs::empty()),
            diff_add_emphasis: style_fg_bg(green, add_bg, Attrs::BOLD),
            diff_del_emphasis: style_fg_bg(red, del_bg, Attrs::BOLD),
        }
    }

    #[allow(clippy::too_many_arguments)]
    const fn build_author_hues(
        is_light: bool,
        is_hc: bool,
        secondary: Color,
        yellow: Color,
        green: Color,
        purple: Color,
        primary: Color,
        orange: Color,
        red: Color,
        text: Color,
    ) -> [Color; 10] {
        let teal_green = Self::blend_rgb(secondary, green);
        let indigo_rose = Self::blend_rgb(primary, purple);
        let amber_coral = Self::blend_rgb(yellow, orange);
        let rose_teal = Self::blend_rgb(red, text);
        [
            Self::ensure_contrast(secondary, is_light, is_hc),
            Self::ensure_contrast(yellow, is_light, is_hc),
            Self::ensure_contrast(green, is_light, is_hc),
            Self::ensure_contrast(purple, is_light, is_hc),
            Self::ensure_contrast(primary, is_light, is_hc),
            Self::ensure_contrast(orange, is_light, is_hc),
            Self::ensure_contrast(teal_green, is_light, is_hc),
            Self::ensure_contrast(indigo_rose, is_light, is_hc),
            Self::ensure_contrast(amber_coral, is_light, is_hc),
            Self::ensure_contrast(rose_teal, is_light, is_hc),
        ]
    }

    const fn blend_rgb(a: Color, b: Color) -> Color {
        match (a, b) {
            (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => Color::Rgb(
                u8::midpoint(r1, r2),
                u8::midpoint(g1, g2),
                u8::midpoint(b1, b2),
            ),
            _ => a,
        }
    }

    /// Ensures `c` meets WCAG AA (>= 4.5:1) or WCAG AAA (>= 10:1) contrast against the theme canvas.
    const fn ensure_contrast(c: Color, is_light: bool, is_hc: bool) -> Color {
        let Color::Rgb(mut r, mut g, mut b) = c else {
            return c;
        };
        // Target relative luminance scaled by 1_000_000:
        // - Dark themes (bg L <= 0.04): need L >= 0.370 (370_000) for >= 4.65:1
        // - HighContrastDark (bg L = 0.0): need L >= 0.460 (460_000) for >= 10.2:1
        // - Light themes (bg L >= 0.87): need L <= 0.145 (145_000) for >= 4.70:1
        // - HighContrastLight (bg L = 1.0): need L <= 0.050 (50_000) for >= 10.5:1
        let mut iter = 0;
        while iter < 48 {
            let lum = Self::approx_rel_lum_u32(r, g, b);
            if is_light {
                let max_lum = if is_hc { 48_000 } else { 142_000 };
                if lum <= max_lum {
                    break;
                }
                r = ((r as u16 * 9) / 10) as u8;
                g = ((g as u16 * 9) / 10) as u8;
                b = ((b as u16 * 9) / 10) as u8;
            } else {
                let min_lum = if is_hc { 465_000 } else { 375_000 };
                if lum >= min_lum {
                    break;
                }
                r = (r as u16 + (255 - r as u16).div_ceil(8)) as u8;
                g = (g as u16 + (255 - g as u16).div_ceil(8)) as u8;
                b = (b as u16 + (255 - b as u16).div_ceil(8)) as u8;
            }
            iter += 1;
        }
        Color::Rgb(r, g, b)
    }

    const fn approx_rel_lum_u32(r: u8, g: u8, b: u8) -> u32 {
        let rl = Self::channel_lin_u32(r) as u64;
        let gl = Self::channel_lin_u32(g) as u64;
        let bl = Self::channel_lin_u32(b) as u64;
        ((2126 * rl + 7152 * gl + 722 * bl) / 10_000) as u32
    }

    const fn channel_lin_u32(v: u8) -> u32 {
        // Maps sRGB [0..255] -> linear [0..1_000_000] via piecewise degree-2.4 rational fit
        let x = v as u64;
        if x <= 10 {
            ((x * 1_000_000) / 3294) as u32
        } else {
            // ((x + 14.025) / 269.025)^2.4 * 1_000_000
            // Approximated accurately via integer arithmetic:
            let num = x * 1000 + 14025; // max 269025
            let sq = (num * num) / 72_374; // in [0..1_000_000]
            let root = (num * 1000) / 269_025; // in [0..1000]
            // x^2.4 = x^2 * x^0.4; blend x^2 and x^3 cleanly
            let cube = (sq * root) / 1000;
            ((sq * 600 + cube * 400) / 1000) as u32
        }
    }

    /// Layers user-specified `[colors]` overrides from `config.toml` on top of this palette.
    pub fn apply_custom_colors(
        &mut self,
        colors: &indexmap::IndexMap<String, tigrs_core::ColorSpec>,
    ) {
        for (area, spec) in colors {
            let style = style_from_spec(spec);
            let key = area.trim().to_ascii_lowercase();
            match key.as_str() {
                "cursor" => self.cursor_row = style,
                "title-focus" | "title_focus" => self.title_bar_active = style,
                "title-blur" | "title_blur" => self.title_bar_inactive = style,
                "status" => self.status_bar = style,
                "line-number" | "line_number" | "gutter" => self.muted_style = style,
                "date" => {
                    if let Some(fg) = style.fg {
                        self.date_fg = fg;
                    }
                }
                "author" => {
                    if let Some(fg) = style.fg {
                        self.author_fg = fg;
                    }
                }
                "id" | "commit" => {
                    if let Some(fg) = style.fg {
                        self.commit_id_fg = fg;
                    }
                }
                "diff-add" | "diff_add" => self.diff_add = style,
                "diff-del" | "diff_del" => self.diff_del = style,
                "diff-header" | "diff_header" => self.diff_header = style,
                "hunk-header" | "hunk_header" => self.diff_hunk_header = style,
                _ => {}
            }
        }
    }

    /// Returns a static 5-byte ANSI foreground escape slice for standard/bright 16-color variants,
    /// or `None` for 256-color / 24-bit RGB variants.
    #[must_use]
    pub const fn fg_sgr_str(color: Color) -> Option<&'static str> {
        match color {
            Color::Black => Some("\x1b[30m"),
            Color::Red => Some("\x1b[31m"),
            Color::Green => Some("\x1b[32m"),
            Color::Yellow => Some("\x1b[33m"),
            Color::Blue => Some("\x1b[34m"),
            Color::Magenta => Some("\x1b[35m"),
            Color::Cyan => Some("\x1b[36m"),
            Color::White => Some("\x1b[37m"),
            Color::BrightBlack => Some("\x1b[90m"),
            Color::BrightRed => Some("\x1b[91m"),
            Color::BrightGreen => Some("\x1b[92m"),
            Color::BrightYellow => Some("\x1b[93m"),
            Color::BrightBlue => Some("\x1b[94m"),
            Color::BrightMagenta => Some("\x1b[95m"),
            Color::BrightCyan => Some("\x1b[96m"),
            Color::BrightWhite => Some("\x1b[97m"),
            Color::Ansi256(_) | Color::Rgb(_, _, _) => None,
        }
    }

    /// Appends a compact foreground-only SGR sequence (preserving background and text attributes)
    /// directly to `out` without heap allocation.
    pub fn write_fg_sgr(out: &mut String, color: Color) {
        use std::fmt::Write;
        if let Some(s) = Self::fg_sgr_str(color) {
            out.push_str(s);
        } else {
            match color {
                Color::Ansi256(n) => {
                    let _ = write!(out, "\x1b[38;5;{n}m");
                }
                Color::Rgb(r, g, b) => {
                    let _ = write!(out, "\x1b[38;2;{r};{g};{b}m");
                }
                _ => {}
            }
        }
    }

    /// Appends a compact background-only SGR sequence directly to `out` without heap allocation.
    pub fn write_bg_sgr(out: &mut String, color: Color) {
        use std::fmt::Write;
        match color {
            Color::Black => out.push_str("\x1b[40m"),
            Color::Red => out.push_str("\x1b[41m"),
            Color::Green => out.push_str("\x1b[42m"),
            Color::Yellow => out.push_str("\x1b[43m"),
            Color::Blue => out.push_str("\x1b[44m"),
            Color::Magenta => out.push_str("\x1b[45m"),
            Color::Cyan => out.push_str("\x1b[46m"),
            Color::White => out.push_str("\x1b[47m"),
            Color::BrightBlack => out.push_str("\x1b[100m"),
            Color::BrightRed => out.push_str("\x1b[101m"),
            Color::BrightGreen => out.push_str("\x1b[102m"),
            Color::BrightYellow => out.push_str("\x1b[103m"),
            Color::BrightBlue => out.push_str("\x1b[104m"),
            Color::BrightMagenta => out.push_str("\x1b[105m"),
            Color::BrightCyan => out.push_str("\x1b[106m"),
            Color::BrightWhite => out.push_str("\x1b[107m"),
            Color::Ansi256(n) => {
                let _ = write!(out, "\x1b[48;5;{n}m");
            }
            Color::Rgb(r, g, b) => {
                let _ = write!(out, "\x1b[48;2;{r};{g};{b}m");
            }
        }
    }

    /// Computes the WCAG 2.1 relative luminance contrast ratio (`1.0 ..= 21.0`) between two colors.
    #[must_use]
    pub fn contrast_ratio(fg: Color, bg: Color) -> f64 {
        let l1 = Self::relative_luminance(fg);
        let l2 = Self::relative_luminance(bg);
        let (bright, dark) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
        (bright + 0.05) / (dark + 0.05)
    }

    /// Resolves a row foreground color `fg`, enforcing minimum WCAG contrast on the selected
    /// cursor row (`7.0` AAA for `HighContrastDark`/`HighContrastLight`, `4.5` AA otherwise)
    /// and falling back to `self.cursor_row.fg` if `fg` lacks contrast against `self.cursor_row.bg`.
    #[must_use]
    pub fn resolve_row_fg(&self, fg: Color, is_selected: bool) -> Color {
        if is_selected && let Some(cursor_bg) = self.cursor_row.bg {
            let min_ratio = match self.category {
                ThemeCategory::HighContrastDark | ThemeCategory::HighContrastLight => 7.0,
                ThemeCategory::Dark | ThemeCategory::Light | ThemeCategory::Adaptive => 4.5,
            };
            if Self::contrast_ratio(fg, cursor_bg) < min_ratio {
                return self.cursor_row.fg.unwrap_or(fg);
            }
        }
        fg
    }

    /// Computes the WCAG 2.1 relative luminance `0.0 ..= 1.0` of `color`.
    #[must_use]
    pub fn relative_luminance(color: Color) -> f64 {
        let (r, g, b) = Self::color_to_rgb(color);
        let lin = |c: u8| -> f64 {
            let s = f64::from(c) / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    #[must_use]
    const fn color_to_rgb(color: Color) -> (u8, u8, u8) {
        match color {
            Color::Black => (0, 0, 0),
            Color::Red => (205, 49, 49),
            Color::Green => (13, 188, 121),
            Color::Yellow => (229, 229, 16),
            Color::Blue => (36, 114, 200),
            Color::Magenta => (188, 63, 188),
            Color::Cyan => (17, 168, 205),
            Color::White => (229, 229, 229),
            Color::BrightBlack => (102, 102, 102),
            Color::BrightRed => (241, 76, 76),
            Color::BrightGreen => (35, 209, 139),
            Color::BrightYellow => (245, 245, 67),
            Color::BrightBlue => (59, 142, 234),
            Color::BrightMagenta => (214, 112, 214),
            Color::BrightCyan => (41, 184, 219),
            Color::BrightWhite => (255, 255, 255),
            Color::Ansi256(n) => (n, n, n),
            Color::Rgb(r, g, b) => (r, g, b),
        }
    }

    /// Formats an SGR escape sequence string for the given `DiffStyle`.
    #[must_use]
    pub fn sgr_for_style(style: DiffStyle) -> String {
        use std::fmt::Write;
        let mut out = String::from("\x1b[0");
        if style.attrs.contains(Attrs::BOLD) {
            out.push_str(";1");
        }
        if style.attrs.contains(Attrs::DIM) {
            out.push_str(";2");
        }
        if style.attrs.contains(Attrs::ITALIC) {
            out.push_str(";3");
        }
        if style.attrs.contains(Attrs::UNDERLINE) {
            out.push_str(";4");
        }
        if style.attrs.contains(Attrs::REVERSE) {
            out.push_str(";7");
        }
        if let Some(fg) = style.fg {
            Self::push_color_sgr(&mut out, fg, false);
        }
        if let Some(bg) = style.bg {
            Self::push_color_sgr(&mut out, bg, true);
        }
        out.push('m');
        let _ = Write::write_str(&mut out, "");
        out
    }

    fn push_color_sgr(out: &mut String, color: Color, is_bg: bool) {
        use std::fmt::Write;
        let base = if is_bg { 40 } else { 30 };
        let bright = if is_bg { 100 } else { 90 };
        match color {
            Color::Black => {
                let _ = write!(out, ";{base}");
            }
            Color::Red => {
                let _ = write!(out, ";{}", base + 1);
            }
            Color::Green => {
                let _ = write!(out, ";{}", base + 2);
            }
            Color::Yellow => {
                let _ = write!(out, ";{}", base + 3);
            }
            Color::Blue => {
                let _ = write!(out, ";{}", base + 4);
            }
            Color::Magenta => {
                let _ = write!(out, ";{}", base + 5);
            }
            Color::Cyan => {
                let _ = write!(out, ";{}", base + 6);
            }
            Color::White => {
                let _ = write!(out, ";{}", base + 7);
            }
            Color::BrightBlack => {
                let _ = write!(out, ";{bright}");
            }
            Color::BrightRed => {
                let _ = write!(out, ";{}", bright + 1);
            }
            Color::BrightGreen => {
                let _ = write!(out, ";{}", bright + 2);
            }
            Color::BrightYellow => {
                let _ = write!(out, ";{}", bright + 3);
            }
            Color::BrightBlue => {
                let _ = write!(out, ";{}", bright + 4);
            }
            Color::BrightMagenta => {
                let _ = write!(out, ";{}", bright + 5);
            }
            Color::BrightCyan => {
                let _ = write!(out, ";{}", bright + 6);
            }
            Color::BrightWhite => {
                let _ = write!(out, ";{}", bright + 7);
            }
            Color::Ansi256(n) => {
                let p = if is_bg { 48 } else { 38 };
                let _ = write!(out, ";{p};5;{n}");
            }
            Color::Rgb(r, g, b) => {
                let p = if is_bg { 48 } else { 38 };
                let _ = write!(out, ";{p};2;{r};{g};{b}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_builtin_color_schemes_resolve_and_produce_valid_palettes() {
        let themes = [
            UiThemeId::Default,
            UiThemeId::CatppuccinMocha,
            UiThemeId::Dracula,
            UiThemeId::TokyoNight,
            UiThemeId::GruvboxDark,
            UiThemeId::Nord,
            UiThemeId::SolarizedDark,
            UiThemeId::GithubDark,
            UiThemeId::CatppuccinLatte,
            UiThemeId::GithubLight,
            UiThemeId::SolarizedLight,
            UiThemeId::GruvboxLight,
            UiThemeId::HighContrastDark,
            UiThemeId::HighContrastLight,
        ];
        for theme_id in themes {
            let canonical = theme_id.as_str();
            assert_eq!(
                UiThemeId::from_config_str(canonical),
                theme_id,
                "Built-in theme '{canonical}' must parse back to {theme_id:?}"
            );
            let palette = UiPalette::for_theme(theme_id);
            assert_eq!(palette.id, theme_id);

            let sgr = UiPalette::sgr_for_style(palette.title_bar_active);
            assert!(
                sgr.starts_with("\x1b[") && sgr.ends_with('m'),
                "Expected valid CSI SGR sequence for {canonical}, got: {sgr:?}"
            );
        }
    }

    #[test]
    fn test_color_scheme_aliases_and_light_dark_classification() {
        assert_eq!(UiThemeId::from_config_str("Dracula"), UiThemeId::Dracula);
        assert_eq!(
            UiThemeId::from_config_str("catppuccin"),
            UiThemeId::CatppuccinMocha
        );
        assert_eq!(
            UiThemeId::from_config_str("latte"),
            UiThemeId::CatppuccinLatte
        );
        assert_eq!(
            UiThemeId::from_config_str("solarized"),
            UiThemeId::SolarizedDark
        );
        assert_eq!(
            UiThemeId::from_config_str("gruvbox"),
            UiThemeId::GruvboxDark
        );
        assert_eq!(
            UiThemeId::from_config_str("tokyonight"),
            UiThemeId::TokyoNight
        );
        assert_eq!(
            UiThemeId::from_config_str("github-dark"),
            UiThemeId::GithubDark
        );
        assert_eq!(
            UiThemeId::from_config_str("nonexistent-theme"),
            UiThemeId::Default
        );

        // Light themes
        for light in [
            UiThemeId::CatppuccinLatte,
            UiThemeId::SolarizedLight,
            UiThemeId::GruvboxLight,
            UiThemeId::GithubLight,
            UiThemeId::HighContrastLight,
        ] {
            let p = UiPalette::for_theme(light);
            assert!(
                matches!(
                    p.category,
                    ThemeCategory::Light | ThemeCategory::HighContrastLight
                ),
                "Expected {light:?} to be classified as Light, got {:?}",
                p.category
            );
        }

        // Dark themes
        for dark in [
            UiThemeId::CatppuccinMocha,
            UiThemeId::Dracula,
            UiThemeId::Nord,
            UiThemeId::SolarizedDark,
            UiThemeId::GruvboxDark,
            UiThemeId::TokyoNight,
            UiThemeId::GithubDark,
            UiThemeId::HighContrastDark,
        ] {
            let p = UiPalette::for_theme(dark);
            assert!(
                matches!(
                    p.category,
                    ThemeCategory::Dark | ThemeCategory::HighContrastDark
                ),
                "Expected {dark:?} to be classified as Dark, got {:?}",
                p.category
            );
        }
    }

    #[test]
    fn test_all_themes_author_hues_meet_wcag_contrast() {
        let named_themes = [
            UiThemeId::CatppuccinMocha,
            UiThemeId::Dracula,
            UiThemeId::TokyoNight,
            UiThemeId::GruvboxDark,
            UiThemeId::Nord,
            UiThemeId::SolarizedDark,
            UiThemeId::GithubDark,
            UiThemeId::CatppuccinLatte,
            UiThemeId::GithubLight,
            UiThemeId::SolarizedLight,
            UiThemeId::GruvboxLight,
            UiThemeId::HighContrastDark,
            UiThemeId::HighContrastLight,
        ];
        for theme_id in named_themes {
            let p = UiPalette::for_theme(theme_id);
            let bg = p.canvas_bg.expect("named theme has canvas_bg");
            let min_ratio = match p.category {
                ThemeCategory::HighContrastDark | ThemeCategory::HighContrastLight => 10.0,
                ThemeCategory::Dark | ThemeCategory::Light | ThemeCategory::Adaptive => 4.5,
            };
            for (slot, &hue) in p.author_hues.iter().enumerate() {
                let ratio = UiPalette::contrast_ratio(hue, bg);
                assert!(
                    ratio >= min_ratio,
                    "Theme {theme_id:?} author_hues[{slot}] ({hue:?}) vs bg ({bg:?}) has contrast {ratio:.2} < {min_ratio}"
                );
            }
        }
    }
}
