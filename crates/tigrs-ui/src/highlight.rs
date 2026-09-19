// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Lazy syntax highlighting and theme engine using `syntect` + `two-face`.
//!
//! Maintains 0 MB memory cost at rest.
//! Syntax sets (200+ languages) and themes (25+ dark/light palettes) are
//! initialized strictly on first use via `LazyLock`.

use crate::Color;
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

static CORE_SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
static LAZY_EXTRA_THEMES: LazyLock<two_face::theme::EmbeddedLazyThemeSet> =
    LazyLock::new(two_face::theme::extra);
static LAZY_THEME_INDEX: LazyLock<HashMap<String, two_face::theme::EmbeddedThemeName>> =
    LazyLock::new(|| {
        let mut index = HashMap::new();
        for &theme_name in two_face::theme::EmbeddedLazyThemeSet::theme_names() {
            index.insert(normalize_theme_key(theme_name.as_name()), theme_name);
        }
        for (alias, target) in THEME_ALIASES {
            if let Some(&embedded) = index.get(&normalize_theme_key(target)) {
                index.entry((*alias).to_string()).or_insert(embedded);
            }
        }
        index
    });
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(|| {
    let mut ts: ThemeSet = two_face::theme::extra().into();
    let defaults = ThemeSet::load_defaults();
    for (name, theme) in defaults.themes {
        ts.themes.entry(name).or_insert(theme);
    }
    ts
});
static THEME_INDEX: LazyLock<HashMap<String, &'static str>> = LazyLock::new(|| {
    let ts = &*THEME_SET;
    let mut index: HashMap<String, &'static str> = ts
        .themes
        .keys()
        .map(|k| (normalize_theme_key(k), k.as_str()))
        .collect();
    // Aliases never shadow a real theme of the same normalized name.
    for (alias, target) in THEME_ALIASES {
        if let Some((canonical, _)) = ts.themes.get_key_value(*target) {
            index.entry((*alias).to_string()).or_insert(canonical);
        }
    }
    index
});

/// Popular user-friendly aliases mapped to canonical `syntect` theme names.
const THEME_ALIASES: &[(&str, &str)] = &[
    ("monokai", "Monokai Extended"),
    ("gruvbox", "gruvbox-dark"),
    ("solarized", "Solarized (dark)"),
    ("onehalf", "OneHalfDark"),
    ("onehalfdark", "OneHalfDark"),
    ("vscode", "Visual Studio Dark+"),
    ("vscodedark", "Visual Studio Dark+"),
    ("githubdark", "TwoDark"),
    ("catppuccin", "base16-mocha.dark"),
    ("catppuccinmocha", "base16-mocha.dark"),
];

/// Maximum line count to run full syntax highlighting on a single file.
/// Larger files fall back to plain text for instant rendering and bounded memory.
pub const MAX_HIGHLIGHT_LINES: usize = 10_000;

/// Maximum cumulative diff lines across an entire `CommitDiff` to highlight eagerly with `syntect`.
/// Prevents multi-file diffs (e.g. 60+ modified files totaling 30,000+ lines) from stalling
/// on single-threaded regex highlighting while keeping typical multi-file commits fully highlighted.
pub const MAX_DIFF_HIGHLIGHT_TOTAL_LINES: usize = 2_500;

/// Maximum cumulative diff lines across an entire `CommitDiff` to run intra-line word diff on.
pub const MAX_DIFF_WORD_DIFF_TOTAL_LINES: usize = 1_000;

/// Default syntax theme name.
pub const DEFAULT_SYNTAX_THEME: &str = "Dracula";

/// A styled character range produced by the syntax highlighter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxSpan {
    /// Start character index (0-based, relative to line text).
    pub start: u32,
    /// End character index (exclusive, relative to line text).
    pub end: u32,
    /// Syntax token foreground color.
    pub fg: Color,
    /// Optional bold modifier from syntax scope.
    pub bold: bool,
    /// Optional italic modifier from syntax scope.
    pub italic: bool,
}

/// Returns the lazily initialized `SyntaxSet` containing 200+ language grammars.
pub fn syntax_set() -> &'static SyntaxSet {
    &SYNTAX_SET
}

/// Returns the lazily initialized `ThemeSet` combining `two-face` extra themes and `syntect` defaults.
pub fn theme_set() -> &'static ThemeSet {
    &THEME_SET
}

fn normalize_theme_key(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Returns the lazily built index mapping normalized theme names and aliases to
/// canonical theme names.
///
/// Built once on first lookup so that fuzzy resolution is O(1) instead of
/// re-normalizing every one of the ~30 catalog entries on each call.
fn theme_index() -> &'static HashMap<String, &'static str> {
    &THEME_INDEX
}

/// Resolves a theme name (exact, case-insensitive, or popular alias) to its canonical name and `Theme`.
#[must_use]
pub fn resolve_theme(name: &str) -> Option<(&'static str, &'static Theme)> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(&embedded) = LAZY_THEME_INDEX.get(&normalize_theme_key(trimmed)) {
        return Some((embedded.as_name(), LAZY_EXTRA_THEMES.get(embedded)));
    }
    let ts = theme_set();
    if let Some((k, v)) = ts.themes.get_key_value(trimmed) {
        return Some((k.as_str(), v));
    }
    let canonical = *theme_index().get(&normalize_theme_key(trimmed))?;
    let (k, v) = ts.themes.get_key_value(canonical)?;
    Some((k.as_str(), v))
}

/// Returns all available theme names sorted alphabetically.
#[must_use]
pub fn available_theme_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = theme_set().themes.keys().map(String::as_str).collect();
    names.sort_unstable_by_key(|a| a.to_ascii_lowercase());
    names
}

/// Returns the canonical theme name for `name`, or `DEFAULT_SYNTAX_THEME` if unrecognized.
#[must_use]
pub fn canonical_theme_name(name: &str) -> &'static str {
    let trimmed = name.trim();
    if trimmed.eq_ignore_ascii_case(DEFAULT_SYNTAX_THEME) {
        return DEFAULT_SYNTAX_THEME;
    }
    if !trimmed.is_empty()
        && let Some(&embedded) = LAZY_THEME_INDEX.get(&normalize_theme_key(trimmed))
    {
        return embedded.as_name();
    }
    resolve_theme(name).map_or(DEFAULT_SYNTAX_THEME, |(k, _)| k)
}

/// Returns the next theme name in the catalog after `current`.
#[must_use]
pub fn next_theme_name(current: &str) -> &'static str {
    let names = available_theme_names();
    if names.is_empty() {
        return DEFAULT_SYNTAX_THEME;
    }
    let canonical = canonical_theme_name(current);
    let idx = names.iter().position(|&n| n == canonical).unwrap_or(0);
    names[(idx + 1) % names.len()]
}

/// Gets the default theme ("Dracula" or fallback).
pub fn default_theme() -> &'static Theme {
    LAZY_EXTRA_THEMES.get(two_face::theme::EmbeddedThemeName::Dracula)
}

/// Gets the theme matching `name` or falls back to `default_theme()`.
pub fn get_theme(name: &str) -> &'static Theme {
    if name.trim().eq_ignore_ascii_case(DEFAULT_SYNTAX_THEME) {
        return default_theme();
    }
    resolve_theme(name).map_or_else(default_theme, |(_, t)| t)
}

fn find_syntax_and_set(
    path: &str,
) -> Option<(
    &'static syntect::parsing::SyntaxReference,
    &'static SyntaxSet,
)> {
    let ext_opt = Path::new(path).extension().and_then(|ext| ext.to_str());
    let name_opt = Path::new(path).file_name().and_then(|name| name.to_str());
    let core = &*CORE_SYNTAX_SET;
    let core_ext = match ext_opt {
        Some(ext) if ext.eq_ignore_ascii_case("h") => Some("c"),
        other => other,
    };
    if let Some(syntax) = core_ext
        .and_then(|ext| core.find_syntax_by_extension(ext))
        .or_else(|| name_opt.and_then(|name| core.find_syntax_by_token(name)))
        && !std::ptr::eq(syntax, core.find_syntax_plain_text())
    {
        return Some((syntax, core));
    }
    let extra = syntax_set();
    let syntax = ext_opt
        .and_then(|ext| extra.find_syntax_by_extension(ext))
        .or_else(|| name_opt.and_then(|name| extra.find_syntax_by_token(name)))?;
    if std::ptr::eq(syntax, extra.find_syntax_plain_text()) {
        return None;
    }
    Some((syntax, extra))
}

fn find_core_diff_syntax(
    path: &str,
) -> Option<(
    &'static syntect::parsing::SyntaxReference,
    &'static SyntaxSet,
)> {
    let ext_opt = Path::new(path).extension().and_then(|ext| ext.to_str());
    let name_opt = Path::new(path).file_name().and_then(|name| name.to_str());
    if let Some(ext) = ext_opt {
        let lower = ext.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "md" | "markdown" | "adoc" | "asciidoc" | "rst" | "org" | "txt" | "text"
        ) {
            return None;
        }
    }
    let mapped_ext = ext_opt.map(|ext| match ext.to_ascii_lowercase().as_str() {
        "h" => "c",
        "toml" | "ini" | "cfg" | "conf" => "yaml",
        "ts" | "tsx" | "mts" | "cts" | "jsx" | "mjs" | "cjs" => "js",
        "kt" | "kts" => "scala",
        "swift" | "zig" => "rs",
        _ => ext,
    });
    let core = &*CORE_SYNTAX_SET;
    let syntax = mapped_ext
        .and_then(|ext| core.find_syntax_by_extension(ext))
        .or_else(|| name_opt.and_then(|name| core.find_syntax_by_token(name)))?;
    if std::ptr::eq(syntax, core.find_syntax_plain_text()) {
        return None;
    }
    Some((syntax, core))
}

/// Context for incremental, streaming syntax highlighting across sequential lines.
pub struct IncrementalHighlighter {
    highlighter: HighlightLines<'static>,
    syntax_set: &'static SyntaxSet,
    line_buf: String,
}

impl std::fmt::Debug for IncrementalHighlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncrementalHighlighter").finish()
    }
}

impl IncrementalHighlighter {
    /// Creates a new incremental highlighter for the specified file path using the default theme.
    #[must_use]
    pub fn new(path: &str) -> Self {
        Self::with_theme(path, DEFAULT_SYNTAX_THEME)
    }

    /// Creates a new incremental highlighter for the specified file path and theme name.
    #[must_use]
    pub fn with_theme(path: &str, theme_name: &str) -> Self {
        let theme = get_theme(theme_name);
        let (syntax, ss) = find_syntax_and_set(path).unwrap_or_else(|| {
            let core = &*CORE_SYNTAX_SET;
            (core.find_syntax_plain_text(), core)
        });

        let highlighter = HighlightLines::new(syntax, theme);
        Self {
            highlighter,
            syntax_set: ss,
            line_buf: String::new(),
        }
    }

    /// Creates a new incremental highlighter for diff views if `path` matches a supported language grammar.
    ///
    /// Uses `CORE_SYNTAX_SET` with lightweight extension aliases so diff rendering avoids
    /// deserializing the 200+ grammar `two-face` catalog or compiling multi-embedded prose grammars.
    #[must_use]
    pub fn with_theme_if_supported(path: &str, theme_name: &str) -> Option<Self> {
        let (syntax, ss) = find_core_diff_syntax(path)?;
        let theme = get_theme(theme_name);
        Some(Self {
            highlighter: HighlightLines::new(syntax, theme),
            syntax_set: ss,
            line_buf: String::new(),
        })
    }

    /// Highlights the next line in the sequence, returning structured `SyntaxSpan` tokens.
    pub fn highlight_line_spans(&mut self, line: &str) -> Vec<SyntaxSpan> {
        let line_str: &str = if line.ends_with('\n') {
            line
        } else {
            self.line_buf.clear();
            self.line_buf.push_str(line);
            self.line_buf.push('\n');
            &self.line_buf
        };

        let Ok(ranges) = self.highlighter.highlight_line(line_str, self.syntax_set) else {
            return Vec::new();
        };

        let mut spans = Vec::with_capacity(ranges.len());
        let mut char_offset: u32 = 0;
        for (style, piece) in ranges {
            let piece_trimmed = piece.trim_end_matches(&['\r', '\n'][..]);
            let char_len = piece_trimmed.chars().count() as u32;
            if char_len > 0 {
                let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                let bold = style.font_style.contains(FontStyle::BOLD);
                let italic = style.font_style.contains(FontStyle::ITALIC);
                spans.push(SyntaxSpan {
                    start: char_offset,
                    end: char_offset + char_len,
                    fg,
                    bold,
                    italic,
                });
                char_offset += char_len;
            }
        }
        spans
    }

    /// Highlights the next line in the sequence, preserving parser state and returning ANSI escaped string.
    pub fn highlight_next(&mut self, line: &str) -> String {
        let line_str: &str = if line.ends_with('\n') {
            line
        } else {
            self.line_buf.clear();
            self.line_buf.push_str(line);
            self.line_buf.push('\n');
            &self.line_buf
        };

        match self.highlighter.highlight_line(line_str, self.syntax_set) {
            Ok(ranges) => {
                let mut escaped = as_24_bit_terminal_escaped(&ranges[..], false);
                if escaped.ends_with('\n') {
                    escaped.pop();
                    if escaped.ends_with('\r') {
                        escaped.pop();
                    }
                }
                escaped.push_str("\x1b[0m");
                escaped
            }
            Err(_) => line.to_string(),
        }
    }
}

/// Highlights lines of source code for the given file path using the default theme.
///
/// Returns ANSI `TrueColor` escaped strings per line.
/// If `lines.len() > MAX_HIGHLIGHT_LINES` or if syntax cannot be determined,
/// lines are returned unhighlighted.
pub fn highlight_code(path: &str, lines: &[String]) -> Vec<String> {
    highlight_code_with_theme(path, lines, DEFAULT_SYNTAX_THEME)
}

/// Highlights lines of source code for the given file path and theme name.
pub fn highlight_code_with_theme(path: &str, lines: &[String], theme_name: &str) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }

    if lines.len() > MAX_HIGHLIGHT_LINES {
        return lines.to_vec();
    }

    let mut inc = IncrementalHighlighter::with_theme(path, theme_name);
    let mut out_lines = Vec::with_capacity(lines.len());
    for line in lines {
        out_lines.push(inc.highlight_next(line));
    }
    out_lines
}

/// Highlights lines of source code and downgrades ANSI colors to match the target color profile.
pub fn highlight_code_with_profile(
    path: &str,
    lines: &[String],
    profile: crate::term_cap::ColorProfile,
) -> Vec<String> {
    if profile == crate::term_cap::ColorProfile::Monochrome {
        return lines.to_vec();
    }
    let highlighted = highlight_code(path, lines);
    if profile == crate::term_cap::ColorProfile::TrueColor {
        highlighted
    } else {
        highlighted
            .into_iter()
            .map(|l| crate::term_cap::downgrade_ansi(&l, profile))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_rust_code() {
        let code = vec![
            "fn main() {".to_string(),
            "    let x: u32 = 42;".to_string(),
            "}".to_string(),
        ];
        let highlighted = highlight_code("test.rs", &code);
        assert_eq!(highlighted.len(), 3);
        // Ensure ANSI escape sequences are present
        assert!(highlighted[0].contains("\x1b[38;2;"));
    }

    #[test]
    fn test_highlight_plain_text_fallback() {
        let code = vec!["just plain text".to_string()];
        let highlighted = highlight_code("test.unknown_extension_xyz", &code);
        assert_eq!(highlighted.len(), 1);
    }

    #[test]
    fn test_highlight_code_with_profile() {
        use crate::term_cap::ColorProfile;

        let code = vec!["pub fn test() -> bool { true }".to_string()];

        // Monochrome returns plain lines without ANSI escapes
        let mono = highlight_code_with_profile("test.rs", &code, ColorProfile::Monochrome);
        assert_eq!(mono, code);
        assert!(!mono[0].contains("\x1b["));

        // TrueColor contains 24-bit escape codes
        let truecolor = highlight_code_with_profile("test.rs", &code, ColorProfile::TrueColor);
        assert!(truecolor[0].contains("\x1b[38;2;"));

        // Ansi256 downgrades from TrueColor
        let ansi256 = highlight_code_with_profile("test.rs", &code, ColorProfile::Ansi256);
        assert!(!ansi256[0].contains("\x1b[38;2;"));
    }

    #[test]
    fn test_highlight_empty_and_special_languages() {
        // Empty lines
        let empty: Vec<String> = vec![];
        assert!(highlight_code("test.rs", &empty).is_empty());

        let blank_line = vec![String::new()];
        let res = highlight_code("test.rs", &blank_line);
        assert_eq!(res.len(), 1);

        // Python and C
        let py_code = vec!["def hello(): pass".to_string()];
        let py_hl = highlight_code("script.py", &py_code);
        assert_eq!(py_hl.len(), 1);

        let c_code = vec!["int main(void) { return 0; }".to_string()];
        let c_hl = highlight_code("main.c", &c_code);
        assert_eq!(c_hl.len(), 1);

        // Modern languages supported via two-face (TOML, TypeScript, Dockerfile)
        let toml_code = vec!["[package]\nname = \"tigrs\"".to_string()];
        let toml_hl = highlight_code("Cargo.toml", &toml_code);
        assert!(toml_hl[0].contains("\x1b[38;2;"));

        let ts_code = vec!["const greeting: string = 'hello';".to_string()];
        let ts_hl = highlight_code("app.ts", &ts_code);
        assert!(ts_hl[0].contains("\x1b[38;2;"));
    }

    #[test]
    fn test_theme_resolution_and_cycling() {
        let themes = available_theme_names();
        assert!(
            themes.len() >= 25,
            "Expected at least 25 themes from two-face + syntect, got {}",
            themes.len()
        );

        // Fuzzy, case-insensitive, and alias matching
        assert_eq!(resolve_theme("dracula").map(|(k, _)| k), Some("Dracula"));
        assert_eq!(resolve_theme("nord").map(|(k, _)| k), Some("Nord"));
        assert_eq!(
            resolve_theme("monokai").map(|(k, _)| k),
            Some("Monokai Extended")
        );
        assert_eq!(
            resolve_theme("SOLARIZED DARK").map(|(k, _)| k),
            Some("Solarized (dark)")
        );
        assert_eq!(resolve_theme("nonexistent_theme_xyz"), None);

        // Theme cycling wraps around cleanly
        let first = themes[0];
        let second = next_theme_name(first);
        assert_eq!(second, themes[1]);
        let last = themes[themes.len() - 1];
        assert_eq!(next_theme_name(last), first);
    }

    #[test]
    fn test_highlight_line_spans_tokenizer() {
        let mut inc = IncrementalHighlighter::with_theme("src/main.rs", "Dracula");
        assert!(format!("{inc:?}").contains("IncrementalHighlighter"));
        let spans = inc.highlight_line_spans("pub fn compute(x: usize) -> bool { true }");
        assert!(
            !spans.is_empty(),
            "SyntaxLineHighlighter must produce non-empty SyntaxSpan tokens"
        );
        assert_eq!(spans[0].start, 0);
        assert!(spans.last().unwrap().end > 0);

        // Empty line returns empty spans
        let mut plain_inc = IncrementalHighlighter::with_theme("notes.unknownext", "Dracula");
        assert!(plain_inc.highlight_line_spans("").is_empty());
        assert_eq!(plain_inc.highlight_line_spans("plain text line").len(), 1);
    }

    #[test]
    fn test_all_theme_aliases_and_fallbacks() {
        assert_eq!(resolve_theme(""), None);
        assert_eq!(resolve_theme("   "), None);
        assert_eq!(
            resolve_theme("gruvbox").map(|(k, _)| k),
            Some("gruvbox-dark")
        );
        assert_eq!(
            resolve_theme("solarized").map(|(k, _)| k),
            Some("Solarized (dark)")
        );
        assert_eq!(
            resolve_theme("onehalf").map(|(k, _)| k),
            Some("OneHalfDark")
        );
        assert_eq!(
            resolve_theme("onehalfdark").map(|(k, _)| k),
            Some("OneHalfDark")
        );
        assert_eq!(
            resolve_theme("vscode").map(|(k, _)| k),
            Some("Visual Studio Dark+")
        );
        assert_eq!(
            resolve_theme("vscodedark").map(|(k, _)| k),
            Some("Visual Studio Dark+")
        );
        assert_eq!(resolve_theme("githubdark").map(|(k, _)| k), Some("TwoDark"));
        assert_eq!(
            resolve_theme("catppuccin").map(|(k, _)| k),
            Some("base16-mocha.dark")
        );
        assert_eq!(
            resolve_theme("catppuccinmocha").map(|(k, _)| k),
            Some("base16-mocha.dark")
        );

        // Fallback functions
        assert_eq!(
            canonical_theme_name("nonexistent_theme_xyz"),
            DEFAULT_SYNTAX_THEME
        );
        let def = default_theme();
        let fallback = get_theme("nonexistent_theme_xyz");
        assert_eq!(def.name, fallback.name);
    }

    #[test]
    fn test_highlight_code_with_profiles_and_large_files() {
        use crate::term_cap::ColorProfile;
        let lines = vec!["fn main() { let x = 42; }".to_string()];
        let ansi256 = highlight_code_with_profile("main.rs", &lines, ColorProfile::Ansi256);
        assert!(ansi256[0].contains("\x1b[38;5;"));

        let ansi16 = highlight_code_with_profile("main.rs", &lines, ColorProfile::Ansi16);
        assert!(ansi16[0].contains("\x1b["));

        let mono = highlight_code_with_profile("main.rs", &lines, ColorProfile::Monochrome);
        assert_eq!(mono[0], lines[0]);

        // Files exceeding MAX_HIGHLIGHT_LINES return unhighlighted clone immediately
        let huge: Vec<String> = vec!["let a = 1;".to_string(); MAX_HIGHLIGHT_LINES + 1];
        let huge_out = highlight_code_with_profile("main.rs", &huge, ColorProfile::TrueColor);
        assert_eq!(huge_out.len(), MAX_HIGHLIGHT_LINES + 1);
        assert_eq!(huge_out[0], "let a = 1;");
    }

    #[test]
    fn test_cpp_macro_function_and_lambda_highlighting() {
        let mut hl =
            IncrementalHighlighter::with_theme_if_supported("src/main.cc", DEFAULT_SYNTAX_THEME)
                .expect("C++ syntax highlighter should be supported");
        let lines = [
            "DEFINE_TEST_GROUP(crypto_suite, nullptr, nullptr);",
            "",
            "DEFINE_TEST_CASE(crypto_suite, verify_signature_flow) {",
            "  VerifyHelper([&]() {",
            "    const std::span<const uint8_t> digest(kTestVector);",
            "    EXPECT_OK(RunVerify(digest), \"verification failed\");",
            "  });",
            "}",
            "",
            "DEFINE_TEST_CASE(crypto_suite, reject_corrupted_digest) {",
            "  uint8_t buf[32] = {0};",
            "  buf[0] ^= 0xff;",
            "  EXPECT_EQ(RunVerify(buf), -EINVAL);",
            "}",
        ];

        for line in lines {
            let spans = hl.highlight_line_spans(line);
            if !line.is_empty() {
                assert!(
                    !spans.is_empty(),
                    "Expected non-empty syntax spans for C++ line: {line}"
                );
            }
        }
    }
}
