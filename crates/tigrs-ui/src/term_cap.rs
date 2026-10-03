// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Terminal capability negotiation and color profile downgrading.
//!
//! Inspects terminal environment (`COLORTERM`, `TERM`, `NO_COLOR`, `LANG`)
//! to determine supported color depth (`TrueColor` 24-bit, 256-color ANSI, 16-color ANSI, Monochrome),
//! line graphics capability (UTF-8 box drawing vs ASCII), and Kitty keyboard protocol support.
//! Provides ANSI escape sequence translation and downsampling across color profiles.

use std::env;

/// Color rendering capability profile of a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ColorProfile {
    /// No color support (or `NO_COLOR` active). Bold, dim, reverse, and underline only.
    Monochrome = 0,
    /// 16 standard ANSI colors (30-37, 90-97 fg; 40-47, 100-107 bg).
    Ansi16 = 1,
    /// 256-color palette (xterm-256color palette, 38;5;N / 48;5;N).
    Ansi256 = 2,
    /// 24-bit `TrueColor` RGB (38;2;R;G;B / 48;2;R;G;B).
    #[default]
    TrueColor = 3,
}

impl ColorProfile {
    /// Returns a human-readable identifier for this profile.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Monochrome => "monochrome",
            Self::Ansi16 => "16-color",
            Self::Ansi256 => "256-color",
            Self::TrueColor => "truecolor",
        }
    }
}

/// Negotiated capabilities of the active terminal session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCapabilities {
    /// Supported color depth.
    pub color_profile: ColorProfile,
    /// Terminal supports Kitty enhanced keyboard protocol.
    pub supports_kitty_keyboard: bool,
    /// Terminal and locale support UTF-8 box-drawing characters (`│`, `├`, etc.).
    pub supports_unicode_box: bool,
    /// Terminal supports DEC mode 2026 synchronized output (`\x1b[?2026h` / `\x1b[?2026l`).
    pub supports_synchronized_output: bool,
}

impl Default for TerminalCapabilities {
    fn default() -> Self {
        Self {
            color_profile: ColorProfile::TrueColor,
            supports_kitty_keyboard: false,
            supports_unicode_box: true,
            supports_synchronized_output: false,
        }
    }
}

impl TerminalCapabilities {
    /// Probes the host environment to negotiate terminal capabilities.
    ///
    /// Memoized after the first probe so per-frame renderers do not invoke
    /// `std::env::var` repeatedly.
    #[must_use]
    pub fn detect() -> Self {
        static DETECTED: std::sync::OnceLock<TerminalCapabilities> = std::sync::OnceLock::new();
        DETECTED
            .get_or_init(|| Self::from_env(|var| env::var(var).ok()))
            .clone()
    }

    /// Negotiates terminal capabilities using a custom environment lookup closure.
    pub fn from_env<F>(lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        // 1. Check NO_COLOR (https://no-color.org/): if set and non-empty, force monochrome
        if let Some(no_color) = lookup("NO_COLOR")
            && !no_color.is_empty()
        {
            return Self {
                color_profile: ColorProfile::Monochrome,
                supports_kitty_keyboard: detect_kitty_keyboard(&lookup),
                supports_unicode_box: detect_unicode_support(&lookup),
                supports_synchronized_output: detect_synchronized_output(&lookup),
            };
        }

        // 2. Check explicit override: TIGRS_COLOR
        if let Some(override_val) = lookup("TIGRS_COLOR") {
            let lower = override_val.trim().to_lowercase();
            let profile = match lower.as_str() {
                "none" | "mono" | "monochrome" | "0" => Some(ColorProfile::Monochrome),
                "16" | "ansi" => Some(ColorProfile::Ansi16),
                "256" | "ansi256" => Some(ColorProfile::Ansi256),
                "truecolor" | "24bit" | "rgb" => Some(ColorProfile::TrueColor),
                _ => None,
            };
            if let Some(p) = profile {
                return Self {
                    color_profile: p,
                    supports_kitty_keyboard: detect_kitty_keyboard(&lookup),
                    supports_unicode_box: detect_unicode_support(&lookup),
                    supports_synchronized_output: detect_synchronized_output(&lookup),
                };
            }
        }

        // 3. Negotiate color profile via COLORTERM and TERM
        let color_profile = detect_color_profile(&lookup);
        let supports_kitty_keyboard = detect_kitty_keyboard(&lookup);
        let supports_unicode_box = detect_unicode_support(&lookup);
        let supports_synchronized_output = detect_synchronized_output(&lookup);

        Self {
            color_profile,
            supports_kitty_keyboard,
            supports_unicode_box,
            supports_synchronized_output,
        }
    }
}

fn detect_color_profile<F>(lookup: &F) -> ColorProfile
where
    F: Fn(&str) -> Option<String>,
{
    // Check COLORTERM first: "truecolor" or "24bit" guarantees 24-bit TrueColor
    if let Some(colorterm) = lookup("COLORTERM") {
        let val = colorterm.trim().to_lowercase();
        if val == "truecolor" || val == "24bit" {
            return ColorProfile::TrueColor;
        }
    }

    let term = lookup("TERM").unwrap_or_default().trim().to_lowercase();

    // Check dumb terminals
    if term == "dumb" {
        return ColorProfile::Monochrome;
    }

    // Modern terminals that are known to support truecolor even without COLORTERM set
    if term.contains("kitty")
        || term.contains("alacritty")
        || term.contains("wezterm")
        || term.contains("foot")
        || term.contains("ghostty")
        || term.contains("rio")
        || term.contains("iterm")
    {
        return ColorProfile::TrueColor;
    }

    // Standard 16-color ANSI terminals (vt100, linux console, xterm generic, bare screen)
    if (term == "linux" || term.starts_with("vt") || term == "xterm" || term == "screen")
        && !term.contains("256")
    {
        return ColorProfile::Ansi16;
    }

    // 256-color terminals
    if term.contains("256color")
        || term.contains("256")
        || term.contains("tmux")
        || term.contains("screen")
    {
        return ColorProfile::Ansi256;
    }

    // Default to Ansi256 on unspecified POSIX terminals
    ColorProfile::Ansi256
}

fn detect_kitty_keyboard<F>(lookup: &F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    if lookup("KITTY_WINDOW_ID").is_some() || lookup("GHOSTTY_RESOURCES_DIR").is_some() {
        return true;
    }
    if let Some(term) = lookup("TERM")
        && (term.contains("kitty") || term.contains("ghostty") || term.contains("wezterm"))
    {
        return true;
    }
    false
}

fn detect_unicode_support<F>(lookup: &F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(term) = lookup("TERM")
        && (term == "linux" || term.starts_with("vt1"))
    {
        return false;
    }
    for var in &["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Some(val) = lookup(var)
            && !val.is_empty()
        {
            let lower = val.to_lowercase();
            return lower.contains("utf-8") || lower.contains("utf8");
        }
    }
    true
}

fn detect_synchronized_output<F>(lookup: &F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    // Explicit override takes precedence: TIGRS_SYNC_OUTPUT
    if let Some(val) = lookup("TIGRS_SYNC_OUTPUT") {
        let lower = val.trim().to_lowercase();
        if lower == "0" || lower == "false" || lower == "off" || lower == "no" {
            return false;
        }
        if lower == "1" || lower == "true" || lower == "on" || lower == "yes" {
            return true;
        }
    }

    if lookup("KITTY_WINDOW_ID").is_some()
        || lookup("GHOSTTY_RESOURCES_DIR").is_some()
        || lookup("ITERM_SESSION_ID").is_some()
        || lookup("WT_SESSION").is_some()
    {
        return true;
    }

    if let Some(prog) = lookup("TERM_PROGRAM") {
        let p = prog.to_lowercase();
        if p.contains("iterm")
            || p.contains("wezterm")
            || p.contains("ghostty")
            || p.contains("vscode")
            || p.contains("alacritty")
            || p.contains("foot")
        {
            return true;
        }
    }

    if let Some(term) = lookup("TERM") {
        let t = term.to_lowercase();
        if t.contains("kitty")
            || t.contains("ghostty")
            || t.contains("wezterm")
            || t.contains("alacritty")
            || t.contains("foot")
            || t.contains("rio")
            || t.contains("tmux")
        {
            return true;
        }
    }

    if let Some(colorterm) = lookup("COLORTERM") {
        let val = colorterm.trim().to_lowercase();
        if val == "truecolor" || val == "24bit" {
            return true;
        }
    }

    false
}

// =========================================================================
// Color downsampling algorithms: TrueColor -> 256 -> 16 -> Monochrome
// =========================================================================

/// Resolves RGB components to the nearest 256-color palette index (0..255).
#[must_use]
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    // Check if this color is close to grayscale
    let diff_red_green = (i16::from(r) - i16::from(g)).abs();
    let diff_green_blue = (i16::from(g) - i16::from(b)).abs();
    let diff_red_blue = (i16::from(r) - i16::from(b)).abs();

    if diff_red_green < 8 && diff_green_blue < 8 && diff_red_blue < 8 {
        // Grayscale ramp from 232 (gray 8) to 255 (gray 238)
        let avg = ((u16::from(r) + u16::from(g) + u16::from(b)) / 3) as u8;
        if avg < 4 {
            return 16; // Pure black in color cube
        }
        if avg > 248 {
            return 231; // Pure white in color cube
        }
        let gray_idx = (f32::from(avg.saturating_sub(8)) / 10.0).round() as u8;
        return 232 + gray_idx.min(23);
    }

    // 6x6x6 color cube: steps at 0, 95, 135, 175, 215, 255
    let to_cube_step = |val: u8| -> u8 {
        if val < 48 {
            0
        } else if val < 115 {
            1
        } else if val < 155 {
            2
        } else if val < 195 {
            3
        } else if val < 235 {
            4
        } else {
            5
        }
    };

    let ir = to_cube_step(r);
    let ig = to_cube_step(g);
    let ib = to_cube_step(b);

    16 + 36 * ir + 6 * ig + ib
}

/// Converts an ANSI 256-color palette index (0..255) to a standard 16-color ANSI code.
///
/// Returns the standard SGR foreground code (30-37 or 90-97) or background code (40-47 or 100-107).
#[must_use]
pub fn ansi256_to_ansi16(idx: u8, is_bg: bool) -> u8 {
    let base_16 = if idx < 16 {
        idx
    } else if (232..=255).contains(&idx) {
        // Grayscale ramp
        let level = idx - 232;
        if level < 4 {
            0 // Black
        } else if level < 12 {
            8 // Bright black / dark gray
        } else if level < 20 {
            7 // White / light gray
        } else {
            15 // Bright white
        }
    } else {
        // 6x6x6 cube: decode r, g, b in 0..5
        let cube = idx - 16;
        let r = cube / 36;
        let g = (cube % 36) / 6;
        let b = cube % 6;

        let high_r = r >= 3;
        let high_g = g >= 3;
        let high_b = b >= 3;

        let bright = r >= 4 || g >= 4 || b >= 4;

        let code = match (high_r, high_g, high_b) {
            (false, false, false) => 0, // Black
            (true, false, false) => 1,  // Red
            (false, true, false) => 2,  // Green
            (true, true, false) => 3,   // Yellow
            (false, false, true) => 4,  // Blue
            (true, false, true) => 5,   // Magenta
            (false, true, true) => 6,   // Cyan
            (true, true, true) => 7,    // White
        };

        if bright { code + 8 } else { code }
    };

    if is_bg {
        if base_16 < 8 {
            40 + base_16
        } else {
            100 + (base_16 - 8)
        }
    } else if base_16 < 8 {
        30 + base_16
    } else {
        90 + (base_16 - 8)
    }
}

/// Converts RGB components directly to a standard 16-color ANSI code.
#[must_use]
pub fn rgb_to_ansi16(r: u8, g: u8, b: u8, is_bg: bool) -> u8 {
    let idx256 = rgb_to_ansi256(r, g, b);
    ansi256_to_ansi16(idx256, is_bg)
}

/// Downgrades ANSI escape sequences in `text` to match `target_profile`.
///
/// - If `target_profile == TrueColor`: text is unmodified.
/// - If `target_profile == Ansi256`: 24-bit RGB codes (`38;2;r;g;b` / `48;2;r;g;b`) are converted to `38;5;idx` / `48;5;idx`.
/// - If `target_profile == Ansi16`: 24-bit RGB and 256-color codes are mapped to standard 16-color codes (`30..37`, `90..97`).
/// - If `target_profile == Monochrome`: all color sequences are stripped, preserving bold (1), dim (2), underline (4), reverse (7), and reset (0).
#[must_use]
pub fn downgrade_ansi(text: &str, target_profile: ColorProfile) -> String {
    if target_profile == ColorProfile::TrueColor {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();

    while let Some((_, ch)) = chars.next() {
        if ch == '\x1b'
            && let Some(&(_, '[')) = chars.peek()
        {
            chars.next(); // consume '['

            // Collect CSI sequence until a terminating character in 0x40..=0x7E
            let mut params_str = String::new();
            let mut terminator = None;

            for (_, c) in chars.by_ref() {
                if (0x40..=0x7E).contains(&(c as u32)) {
                    terminator = Some(c);
                    break;
                }
                params_str.push(c);
            }

            if terminator == Some('m') {
                // This is an SGR sequence
                let transformed = transform_sgr(&params_str, target_profile);
                if !transformed.is_empty() {
                    out.push_str("\x1b[");
                    out.push_str(&transformed);
                    out.push('m');
                }
            } else if let Some(term) = terminator {
                // Non-SGR CSI sequence (e.g. cursor moves, clear screen): preserve
                out.push_str("\x1b[");
                out.push_str(&params_str);
                out.push(term);
            } else {
                out.push('\x1b');
                out.push('[');
                out.push_str(&params_str);
            }
            continue;
        }
        out.push(ch);
    }

    out
}

/// Parses and transforms semicolon-separated SGR parameter numbers for the target color profile.
fn transform_sgr(params_str: &str, target: ColorProfile) -> String {
    if params_str.is_empty() {
        return "0".to_string();
    }

    let tokens: Vec<u32> = params_str
        .split(';')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect();

    if tokens.is_empty() {
        return "0".to_string();
    }

    let mut out_tokens = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let code = tokens[i];

        // 38 (fg color) or 48 (bg color)
        if (code == 38 || code == 48) && i + 1 < tokens.len() {
            let is_bg = code == 48;
            let mode = tokens[i + 1];

            if mode == 2 && i + 4 < tokens.len() {
                // 38;2;R;G;B
                let r = tokens[i + 2].min(255) as u8;
                let g = tokens[i + 3].min(255) as u8;
                let b = tokens[i + 4].min(255) as u8;

                match target {
                    ColorProfile::TrueColor => {
                        out_tokens.extend_from_slice(&tokens[i..=i + 4]);
                    }
                    ColorProfile::Ansi256 => {
                        let idx = rgb_to_ansi256(r, g, b);
                        out_tokens.push(if is_bg { 48 } else { 38 });
                        out_tokens.push(5);
                        out_tokens.push(u32::from(idx));
                    }
                    ColorProfile::Ansi16 => {
                        let c16 = rgb_to_ansi16(r, g, b, is_bg);
                        out_tokens.push(u32::from(c16));
                    }
                    ColorProfile::Monochrome => {
                        // Stripped
                    }
                }
                i += 5;
                continue;
            } else if mode == 5 && i + 2 < tokens.len() {
                // 38;5;idx
                let idx = tokens[i + 2].min(255) as u8;
                match target {
                    ColorProfile::TrueColor | ColorProfile::Ansi256 => {
                        out_tokens.extend_from_slice(&tokens[i..=i + 2]);
                    }
                    ColorProfile::Ansi16 => {
                        let c16 = ansi256_to_ansi16(idx, is_bg);
                        out_tokens.push(u32::from(c16));
                    }
                    ColorProfile::Monochrome => {
                        // Stripped
                    }
                }
                i += 3;
                continue;
            }
        }

        // Standard 16-color codes: 30..=37, 39, 40..=47, 49, 90..=97, 100..=107
        if is_color_code(code) {
            match target {
                ColorProfile::TrueColor | ColorProfile::Ansi256 | ColorProfile::Ansi16 => {
                    out_tokens.push(code);
                }
                ColorProfile::Monochrome => {
                    // Stripped
                }
            }
            i += 1;
            continue;
        }

        // Text style attributes (bold, dim, underline, reverse, reset)
        out_tokens.push(code);
        i += 1;
    }

    out_tokens
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(";")
}

fn is_color_code(code: u32) -> bool {
    matches!(
        code,
        30..=37 | 39 | 40..=47 | 49 | 90..=97 | 100..=107
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_no_color() {
        let caps = TerminalCapabilities::from_env(|var| {
            if var == "NO_COLOR" {
                Some("1".to_string())
            } else {
                None
            }
        });
        assert_eq!(caps.color_profile, ColorProfile::Monochrome);
    }

    #[test]
    fn test_detect_colorterm_truecolor() {
        let caps = TerminalCapabilities::from_env(|var| {
            if var == "COLORTERM" {
                Some("truecolor".to_string())
            } else {
                None
            }
        });
        assert_eq!(caps.color_profile, ColorProfile::TrueColor);
    }

    #[test]
    fn test_detect_term_256color() {
        let caps = TerminalCapabilities::from_env(|var| {
            if var == "TERM" {
                Some("xterm-256color".to_string())
            } else {
                None
            }
        });
        assert_eq!(caps.color_profile, ColorProfile::Ansi256);
    }

    #[test]
    fn test_detect_term_linux_ansi16() {
        let caps = TerminalCapabilities::from_env(|var| {
            if var == "TERM" {
                Some("linux".to_string())
            } else {
                None
            }
        });
        assert_eq!(caps.color_profile, ColorProfile::Ansi16);
    }

    #[test]
    fn test_rgb_to_ansi256() {
        // Pure black
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
        // Pure white
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
        // Pure red
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
        // Pure green
        assert_eq!(rgb_to_ansi256(0, 255, 0), 46);
        // Pure blue
        assert_eq!(rgb_to_ansi256(0, 0, 255), 21);
    }

    #[test]
    fn test_downgrade_truecolor_to_256() {
        let input = "\x1b[38;2;255;0;0mHello\x1b[0m";
        let downgraded = downgrade_ansi(input, ColorProfile::Ansi256);
        assert_eq!(downgraded, "\x1b[38;5;196mHello\x1b[0m");
    }

    #[test]
    fn test_downgrade_truecolor_to_16() {
        let input = "\x1b[38;2;255;0;0mHello\x1b[0m";
        let downgraded = downgrade_ansi(input, ColorProfile::Ansi16);
        // 255;0;0 maps to bright red (91)
        assert_eq!(downgraded, "\x1b[91mHello\x1b[0m");
    }

    #[test]
    fn test_downgrade_to_monochrome() {
        let input = "\x1b[1;31mBold Red\x1b[0m";
        let downgraded = downgrade_ansi(input, ColorProfile::Monochrome);
        // Color 31 is stripped, 1 (bold) and 0 (reset) are retained
        assert_eq!(downgraded, "\x1b[1mBold Red\x1b[0m");
    }

    #[test]
    fn test_downgrade_preserves_plain_text() {
        let input = "Plain text with no escapes";
        let downgraded = downgrade_ansi(input, ColorProfile::Monochrome);
        assert_eq!(downgraded, input);
    }

    #[test]
    fn test_detect_synchronized_output() {
        // Default / empty env: false
        let caps = TerminalCapabilities::from_env(|_| None);
        assert!(!caps.supports_synchronized_output);

        // Explicit override: 1
        let caps = TerminalCapabilities::from_env(|var| {
            if var == "TIGRS_SYNC_OUTPUT" {
                Some("1".to_string())
            } else {
                None
            }
        });
        assert!(caps.supports_synchronized_output);

        // Explicit override: 0 suppresses detection even in Kitty
        let caps = TerminalCapabilities::from_env(|var| match var {
            "TIGRS_SYNC_OUTPUT" => Some("0".to_string()),
            "KITTY_WINDOW_ID" => Some("1".to_string()),
            _ => None,
        });
        assert!(!caps.supports_synchronized_output);

        // Kitty window id
        let caps = TerminalCapabilities::from_env(|var| {
            if var == "KITTY_WINDOW_ID" {
                Some("1".to_string())
            } else {
                None
            }
        });
        assert!(caps.supports_synchronized_output);

        // Ghostty resources dir
        let caps = TerminalCapabilities::from_env(|var| {
            if var == "GHOSTTY_RESOURCES_DIR" {
                Some("/usr/share/ghostty".to_string())
            } else {
                None
            }
        });
        assert!(caps.supports_synchronized_output);

        // TERM=alacritty
        let caps = TerminalCapabilities::from_env(|var| {
            if var == "TERM" {
                Some("alacritty".to_string())
            } else {
                None
            }
        });
        assert!(caps.supports_synchronized_output);

        // COLORTERM=truecolor
        let caps = TerminalCapabilities::from_env(|var| {
            if var == "COLORTERM" {
                Some("truecolor".to_string())
            } else {
                None
            }
        });
        assert!(caps.supports_synchronized_output);
    }

    #[test]
    fn test_color_profile_as_str_and_overrides() {
        assert_eq!(ColorProfile::Monochrome.as_str(), "monochrome");
        assert_eq!(ColorProfile::Ansi16.as_str(), "16-color");
        assert_eq!(ColorProfile::Ansi256.as_str(), "256-color");
        assert_eq!(ColorProfile::TrueColor.as_str(), "truecolor");

        // TIGRS_COLOR overrides
        for val in ["none", "mono", "monochrome", "0"] {
            let caps = TerminalCapabilities::from_env(|var| {
                if var == "TIGRS_COLOR" {
                    Some(val.to_string())
                } else {
                    None
                }
            });
            assert_eq!(caps.color_profile, ColorProfile::Monochrome);
        }

        for val in ["16", "ansi"] {
            let caps = TerminalCapabilities::from_env(|var| {
                if var == "TIGRS_COLOR" {
                    Some(val.to_string())
                } else {
                    None
                }
            });
            assert_eq!(caps.color_profile, ColorProfile::Ansi16);
        }

        for val in ["256", "ansi256"] {
            let caps = TerminalCapabilities::from_env(|var| {
                if var == "TIGRS_COLOR" {
                    Some(val.to_string())
                } else {
                    None
                }
            });
            assert_eq!(caps.color_profile, ColorProfile::Ansi256);
        }

        for val in ["truecolor", "24bit", "rgb"] {
            let caps = TerminalCapabilities::from_env(|var| {
                if var == "TIGRS_COLOR" {
                    Some(val.to_string())
                } else {
                    None
                }
            });
            assert_eq!(caps.color_profile, ColorProfile::TrueColor);
        }

        // TERM_PROGRAM detection
        for prog in ["iTerm.app", "WezTerm", "ghostty", "vscode", "foot"] {
            let caps = TerminalCapabilities::from_env(|var| {
                if var == "TERM_PROGRAM" {
                    Some(prog.to_string())
                } else {
                    None
                }
            });
            assert!(caps.supports_synchronized_output);
        }
    }

    #[test]
    fn test_color_conversions_and_downgrades() {
        // Grayscale RGB to 256
        let gray256 = rgb_to_ansi256(128, 128, 128);
        assert!((232..=255).contains(&gray256));

        // Truecolor passthrough
        let input_rgb = "\x1b[38;2;10;20;30mRGB\x1b[0m";
        assert_eq!(
            downgrade_ansi(input_rgb, ColorProfile::TrueColor),
            input_rgb
        );

        // 256 color passthrough on Ansi256
        let input_256 = "\x1b[38;5;123m256\x1b[0m";
        assert_eq!(downgrade_ansi(input_256, ColorProfile::Ansi256), input_256);

        // 256 color to Ansi16
        let fg_sample = "\x1b[38;5;196mRed\x1b[0m";
        assert_eq!(
            downgrade_ansi(fg_sample, ColorProfile::Ansi16),
            "\x1b[91mRed\x1b[0m"
        );

        // Background 256 color to Ansi16
        let bg_sample = "\x1b[48;5;196mRedBg\x1b[0m";
        assert_eq!(
            downgrade_ansi(bg_sample, ColorProfile::Ansi16),
            "\x1b[101mRedBg\x1b[0m"
        );

        // Background 256 grayscale to Ansi16
        let input_gray_bg = "\x1b[48;5;234mDarkBg\x1b[0m";
        let down_gray_bg = downgrade_ansi(input_gray_bg, ColorProfile::Ansi16);
        assert!(down_gray_bg.contains("\x1b[40m") || down_gray_bg.contains("\x1b[100m"));

        // Non-SGR CSI sequence preservation
        let csi_cursor = "\x1b[2J\x1b[HHello\x1b[0m";
        assert_eq!(
            downgrade_ansi(csi_cursor, ColorProfile::Monochrome),
            "\x1b[2J\x1b[HHello\x1b[0m"
        );

        // Monochrome stripping of standard, 256, and RGB colors
        let colored_text =
            "\x1b[1;31;42mBoldRedGreen\x1b[38;2;10;20;30mRGB\x1b[38;5;123m256\x1b[0m";
        let mono = downgrade_ansi(colored_text, ColorProfile::Monochrome);
        assert!(!mono.contains("31"));
        assert!(!mono.contains("42"));
        assert!(!mono.contains("38;2"));
        assert!(!mono.contains("38;5"));
        assert!(mono.contains("\x1b[1m")); // Bold preserved

        // Grayscale levels
        assert_eq!(ansi256_to_ansi16(242, false), 90); // level 10 (< 12) -> base 8 -> fg 90
        assert_eq!(ansi256_to_ansi16(247, false), 37); // level 15 (< 20) -> base 7 -> fg 37
        assert_eq!(ansi256_to_ansi16(254, false), 97); // level 22 (>= 20) -> base 15 -> fg 97

        // RGB 6-cube primaries and secondaries
        assert_eq!(ansi256_to_ansi16(16, false), 30); // Black (0, 0, 0) -> fg 30
        assert_eq!(ansi256_to_ansi16(46, false), 92); // Green bright (0, 5, 0) -> fg 92
        assert_eq!(ansi256_to_ansi16(226, false), 93); // Yellow bright (5, 5, 0) -> fg 93
        assert_eq!(ansi256_to_ansi16(21, false), 94); // Blue bright (0, 0, 5) -> fg 94
        assert_eq!(ansi256_to_ansi16(201, false), 95); // Magenta bright (5, 0, 5) -> fg 95
        assert_eq!(ansi256_to_ansi16(51, false), 96); // Cyan bright (0, 5, 5) -> fg 96
        assert_eq!(ansi256_to_ansi16(231, false), 97); // White bright (5, 5, 5) -> fg 97

        // Background conversions
        assert_eq!(ansi256_to_ansi16(16, true), 40); // Black bg -> 40
        assert_eq!(ansi256_to_ansi16(242, true), 100); // Bright black bg -> 100

        // NO_COLOR and dumb terminal detection
        let no_color_caps = TerminalCapabilities::from_env(|var| {
            if var == "NO_COLOR" {
                Some("1".to_string())
            } else {
                None
            }
        });
        assert_eq!(no_color_caps.color_profile, ColorProfile::Monochrome);

        let dumb_caps = TerminalCapabilities::from_env(|var| {
            if var == "TERM" {
                Some("dumb".to_string())
            } else {
                None
            }
        });
        assert_eq!(dumb_caps.color_profile, ColorProfile::Monochrome);

        // rgb_to_ansi16
        assert_eq!(rgb_to_ansi16(0, 0, 0, false), 30);
        assert_eq!(rgb_to_ansi16(0, 0, 0, true), 40);

        // downgrade_ansi with non-SGR CSI sequence and unterminated sequence
        let csi_seq = "before\x1b[2Jmiddle\x1b[10;20Hafter";
        assert_eq!(downgrade_ansi(csi_seq, ColorProfile::Ansi16), csi_seq);
        let unterminated = "broken\x1b[123";
        assert_eq!(
            downgrade_ansi(unterminated, ColorProfile::Ansi16),
            unterminated
        );
        let non_csi_escape = "test\x1b(Bmore";
        assert_eq!(
            downgrade_ansi(non_csi_escape, ColorProfile::Ansi16),
            non_csi_escape
        );

        // TerminalCapabilities::detect() default invocation
        let detected = TerminalCapabilities::detect();
        assert!(format!("{detected:?}").contains("TerminalCapabilities"));

        // Test sync output overrides
        for val in &["0", "false", "off", "no"] {
            let caps = TerminalCapabilities::from_env(|k| {
                if k == "TIGRS_SYNC_OUTPUT" {
                    Some((*val).to_string())
                } else {
                    None
                }
            });
            assert!(!caps.supports_synchronized_output);
        }
        for val in &["1", "true", "on", "yes"] {
            let caps = TerminalCapabilities::from_env(|k| {
                if k == "TIGRS_SYNC_OUTPUT" {
                    Some((*val).to_string())
                } else {
                    None
                }
            });
            assert!(caps.supports_synchronized_output);
        }

        // Test environment branches for kitty keyboard, unicode, sync, color
        let env_tests = [
            ("WT_SESSION", "1", true, false, true),
            ("ITERM_SESSION_ID", "1", true, false, true),
            ("KITTY_WINDOW_ID", "1", true, true, true),
            ("GHOSTTY_RESOURCES_DIR", "/path", true, true, true),
        ];
        for (var, val, exp_sync, exp_kitty, exp_unicode) in env_tests {
            let caps = TerminalCapabilities::from_env(|k| {
                if k == var {
                    Some(val.to_string())
                } else {
                    None
                }
            });
            assert_eq!(caps.supports_synchronized_output, exp_sync);
            assert_eq!(caps.supports_kitty_keyboard, exp_kitty);
            assert_eq!(caps.supports_unicode_box, exp_unicode);
        }

        // Test TERM_PROGRAM
        for prog in &[
            "iterm2",
            "wezterm",
            "ghostty",
            "vscode",
            "alacritty",
            "foot",
        ] {
            let caps = TerminalCapabilities::from_env(|k| {
                if k == "TERM_PROGRAM" {
                    Some((*prog).to_string())
                } else {
                    None
                }
            });
            assert!(caps.supports_synchronized_output);
        }

        // Test COLORTERM=24bit
        let caps_24bit = TerminalCapabilities::from_env(|k| {
            if k == "COLORTERM" {
                Some("24bit".to_string())
            } else {
                None
            }
        });
        assert_eq!(caps_24bit.color_profile, ColorProfile::TrueColor);
        assert!(caps_24bit.supports_synchronized_output);

        // Test TERM variations
        let caps_kitty = TerminalCapabilities::from_env(|k| {
            if k == "TERM" {
                Some("xterm-kitty".to_string())
            } else {
                None
            }
        });
        assert_eq!(caps_kitty.color_profile, ColorProfile::TrueColor);
        assert!(caps_kitty.supports_kitty_keyboard);
        assert!(caps_kitty.supports_synchronized_output);

        let caps_screen = TerminalCapabilities::from_env(|k| {
            if k == "TERM" {
                Some("screen".to_string())
            } else {
                None
            }
        });
        assert_eq!(caps_screen.color_profile, ColorProfile::Ansi16);

        let caps_screen_256 = TerminalCapabilities::from_env(|k| {
            if k == "TERM" {
                Some("screen-256color".to_string())
            } else {
                None
            }
        });
        assert_eq!(caps_screen_256.color_profile, ColorProfile::Ansi256);

        let caps_sync_off = TerminalCapabilities::from_env(|k| match k {
            "TERM" => Some("xterm-kitty".to_string()),
            "TIGRS_SYNC_OUTPUT" => Some("0".to_string()),
            _ => None,
        });
        assert!(!caps_sync_off.supports_synchronized_output);

        let caps_sync_on = TerminalCapabilities::from_env(|k| match k {
            "TERM" => Some("xterm".to_string()),
            "TIGRS_SYNC_OUTPUT" => Some("1".to_string()),
            _ => None,
        });
        assert!(caps_sync_on.supports_synchronized_output);

        let caps_vt = TerminalCapabilities::from_env(|k| {
            if k == "TERM" {
                Some("vt100".to_string())
            } else {
                None
            }
        });
        assert_eq!(caps_vt.color_profile, ColorProfile::Ansi16);
        assert!(!caps_vt.supports_unicode_box);

        let caps_linux = TerminalCapabilities::from_env(|k| {
            if k == "TERM" {
                Some("linux".to_string())
            } else {
                None
            }
        });
        assert_eq!(caps_linux.color_profile, ColorProfile::Ansi16);
        assert!(!caps_linux.supports_unicode_box);

        let caps_utf8 = TerminalCapabilities::from_env(|k| {
            if k == "LC_ALL" {
                Some("en_US.UTF-8".to_string())
            } else {
                None
            }
        });
        assert!(caps_utf8.supports_unicode_box);
    }
}
