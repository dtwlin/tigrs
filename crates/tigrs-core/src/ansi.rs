// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! ANSI sanitization and control sequence filtering policies.
//!
//! Provides defense against terminal injection attacks (arbitrary escape sequences,
//! terminal title spoofing, screen clearing, cursor repositioning) while preserving
//! legitimate text and optional SGR styling (for colored piped logs and diffs).

use std::borrow::Cow;

#[inline]
pub(crate) fn is_dangerous_unicode_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{0080}'..='\u{009F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// Strips all ANSI escape sequences and non-printable control characters from `input`.
///
/// This policy is mandatory for untrusted repository-supplied metadata such as
/// commit subjects, author names, email addresses, branch names, and tag labels.
///
/// Returns [`Cow::Borrowed`] when no control characters or escape sequences are present,
/// avoiding heap allocation entirely for clean inputs.
///
/// Kept characters:
/// - Printable ASCII (`0x20..=0x7E`)
/// - Standard whitespace: `\t` and `\n` (newlines)
/// - Valid multibyte UTF-8 characters (excluding C1 controls and `BiDi` overrides)
///
/// Stripped characters:
/// - All ANSI escape sequences (CSI `\x1b[...]`, OSC `\x1b]...`, 2-byte escapes)
/// - ASCII control codes (`0x00..=0x1F` except `\t` and `\n`)
/// - Delete (`0x7F`)
/// - UTF-8 C1 control codes (`U+0080..=U+009F`, e.g. 8-bit CSI/OSC)
/// - Unicode `BiDi` override characters (`U+202A..=U+202E`, `U+2066..=U+2069`)
/// - Carriage return `\r` is stripped to prevent line-overwriting attacks
pub fn strip_control_chars(input: &str) -> Cow<'_, str> {
    let bytes = input.as_bytes();
    let len = bytes.len();

    // Fast-path: check if any sanitization is required
    let mut needs_strip = false;
    for &b in bytes {
        if b < 0x20 {
            if b != b'\t' && b != b'\n' {
                needs_strip = true;
                break;
            }
        } else if b == 0x7f || b == 0x1b || b == 0xc2 || b == 0xe2 {
            needs_strip = true;
            break;
        }
    }

    if !needs_strip {
        return Cow::Borrowed(input);
    }

    let mut out = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        let b = bytes[i];
        if b == 0x1b {
            // Escape sequence start: skip it entirely
            i = skip_escape_sequence(bytes, i);
        } else if b == b'\t' || b == b'\n' {
            out.push(b as char);
            i += 1;
        } else if b < 0x20 || b == 0x7f {
            // Control character or DEL: skip
            i += 1;
        } else if b < 0x80 {
            // Printable ASCII
            out.push(b as char);
            i += 1;
        } else {
            // Multibyte UTF-8 sequence
            if let Some(ch) = input.get(i..).and_then(|s| s.chars().next()) {
                if !is_dangerous_unicode_char(ch) {
                    out.push(ch);
                }
                i += ch.len_utf8();
            } else {
                // Invalid UTF-8 boundary fallback
                i += 1;
            }
        }
    }

    if out.len() == len {
        Cow::Borrowed(input)
    } else {
        Cow::Owned(out)
    }
}

/// Filters ANSI escape sequences to allow ONLY SGR (Select Graphic Rendition) color codes.
///
/// Used when consuming colored piped input (e.g. `git log --color | tigrs`) or pre-formatted
/// diffs where color formatting should be preserved for the TUI, but harmful sequences
/// (cursor movement, clear screen, OSC hyperlinks/titles, mode switching) must be neutralized.
///
/// Returns [`Cow::Borrowed`] when no escape sequences or non-printable characters are found.
///
/// Allowed:
/// - SGR sequences: `\x1b[<params>m` (e.g., `\x1b[0m`, `\x1b[31;1m`, `\x1b[38;2;r;g;bm`)
/// - Printable text, tabs, and newlines
///
/// Stripped:
/// - Non-SGR CSI commands (`\x1b[2J`, `\x1b[H`, `\x1b[?25l`, etc.)
/// - OSC sequences (`\x1b]...`)
/// - Dangerous control codes (`\r`, `\x07`, `\x08`, C1 controls, `BiDi` overrides)
pub fn filter_sgr_only(input: &str) -> Cow<'_, str> {
    let bytes = input.as_bytes();
    let len = bytes.len();

    let mut needs_filter = false;
    for &b in bytes {
        if b < 0x20 {
            if b != b'\t' && b != b'\n' {
                needs_filter = true;
                break;
            }
        } else if b == 0x7f || b == 0x1b || b == 0xc2 || b == 0xe2 {
            needs_filter = true;
            break;
        }
    }

    if !needs_filter {
        return Cow::Borrowed(input);
    }

    let mut out = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        let b = bytes[i];
        if b == 0x1b {
            let start = i;
            let next_i = skip_escape_sequence(bytes, i);
            if is_sgr_sequence(&bytes[start..next_i]) {
                // Safe SGR sequence, preserve verbatim
                if let Ok(s) = std::str::from_utf8(&bytes[start..next_i]) {
                    out.push_str(s);
                }
            }
            i = next_i;
        } else if b == b'\t' || b == b'\n' {
            out.push(b as char);
            i += 1;
        } else if b < 0x20 || b == 0x7f {
            // Control character: skip
            i += 1;
        } else if b < 0x80 {
            out.push(b as char);
            i += 1;
        } else if let Some(ch) = input.get(i..).and_then(|s| s.chars().next()) {
            if !is_dangerous_unicode_char(ch) {
                out.push(ch);
            }
            i += ch.len_utf8();
        } else {
            i += 1;
        }
    }

    if out.len() == len {
        Cow::Borrowed(input)
    } else {
        Cow::Owned(out)
    }
}

/// Sanitizes an owned [`String`] in place, reusing its allocation when possible.
///
/// [`strip_control_chars`] returns [`Cow::Borrowed`] for the overwhelmingly common
/// case of clean input. Calling `.into_owned()` on that borrow allocates a second
/// heap buffer and drops the original, which is pure waste when the caller already
/// owns the string. This helper returns the original allocation untouched in that
/// case, and only takes the sanitized copy when characters were actually stripped.
///
/// # Examples
///
/// ```
/// use tigrs_core::ansi::sanitize_string;
///
/// assert_eq!(sanitize_string("src/main.rs".to_string()), "src/main.rs");
/// assert_eq!(sanitize_string("evil\x1b[2Jpath".to_string()), "evilpath");
/// ```
#[must_use]
pub fn sanitize_string(input: String) -> String {
    match strip_control_chars(&input) {
        Cow::Borrowed(_) => input,
        Cow::Owned(clean) => clean,
    }
}

/// Returns the terminal display width of `input`, ignoring ANSI escape sequences and control characters.
pub fn visible_width(input: &str) -> usize {
    let clean = strip_control_chars(input);
    unicode_width::UnicodeWidthStr::width(&*clean)
}

/// Truncates a string slice to fit within `max_width` display columns without allocation.
///
/// Accounts for wide characters (e.g. CJK, emojis) using terminal display width.
/// Returns a subslice `&str` of `s` that fits within `max_width` columns.
/// If `s` already fits within `max_width` columns, returns `s` unchanged.
///
/// Guaranteed not to panic or split UTF-8 character boundaries.
#[inline]
#[must_use]
pub fn truncate_display_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut total_w = 0;
    for (idx, ch) in s.char_indices() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if total_w + cw > max_width {
            return &s[..idx];
        }
        total_w += cw;
    }
    s
}

/// Truncates or pads a string to exactly `target_width` display columns.
///
/// Accounts for wide characters (CJK, emojis) using Unicode terminal display width,
/// preventing `{:<w$}` scalar-count padding from overflowing terminal columns.
#[must_use]
pub fn pad_display_width(s: &str, target_width: usize) -> Cow<'_, str> {
    if target_width == 0 {
        return Cow::Borrowed("");
    }
    let truncated = truncate_display_width(s, target_width);
    let cur_w = unicode_width::UnicodeWidthStr::width(truncated);
    if cur_w == target_width && truncated.len() == s.len() {
        return Cow::Borrowed(s);
    }
    let pad = target_width.saturating_sub(cur_w);
    let mut out = String::with_capacity(truncated.len() + pad);
    out.push_str(truncated);
    for _ in 0..pad {
        out.push(' ');
    }
    Cow::Owned(out)
}

/// Truncates a string to fit within `max_width` display columns, appending "..." if truncated.
///
/// Returns [`Cow::Borrowed`] if `s` fits within `max_width` or if `max_width <= 3` (no room for ellipsis).
/// Otherwise returns [`Cow::Owned`] with "..." appended within the column limit.
#[must_use]
pub fn truncate_display_width_ellipsis(s: &str, max_width: usize) -> Cow<'_, str> {
    let truncated = truncate_display_width(s, max_width);
    if truncated.len() == s.len() {
        return Cow::Borrowed(s);
    }
    if max_width <= 3 {
        return Cow::Borrowed(truncated);
    }
    let prefix = truncate_display_width(s, max_width.saturating_sub(3));
    let mut out = String::with_capacity(prefix.len() + 3);
    out.push_str(prefix);
    out.push_str("...");
    Cow::Owned(out)
}

/// Truncates a string to at most `max_width` visible terminal columns, preserving
/// ANSI SGR escape sequences and appending a reset code (`\x1b[0m`) if truncated while styled.
pub fn truncate_visible_width(input: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len.min(max_width * 2));
    let mut cur_width = 0;
    let mut i = 0;
    let mut in_style = false;

    while i < len {
        let b = bytes[i];
        if b == 0x1b {
            let start = i;
            let next_i = skip_escape_sequence(bytes, i);
            let seq = &bytes[start..next_i];
            if is_sgr_sequence(seq)
                && let Ok(s) = std::str::from_utf8(seq)
            {
                out.push_str(s);
                in_style = s != "\x1b[0m" && s != "\x1b[m";
            }
            i = next_i;
        } else if b == b'\t' || b == b'\n' {
            let ch_w = 1;
            if cur_width + ch_w > max_width {
                break;
            }
            out.push(b as char);
            cur_width += ch_w;
            i += 1;
        } else if b < 0x20 || b == 0x7f {
            i += 1;
        } else if let Some(ch) = input.get(i..).and_then(|s| s.chars().next()) {
            if !is_dangerous_unicode_char(ch) {
                let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if cur_width + ch_w > max_width {
                    break;
                }
                out.push(ch);
                cur_width += ch_w;
            }
            i += ch.len_utf8();
        } else {
            i += 1;
        }
    }

    if in_style {
        out.push_str("\x1b[0m");
    }

    out
}

/// Returns true if the slice is an ANSI SGR sequence (`\x1b[...m`).
fn is_sgr_sequence(seq: &[u8]) -> bool {
    if seq.len() >= 3 && seq[0] == 0x1b && seq[1] == b'[' && seq[seq.len() - 1] == b'm' {
        // Verify all intermediate characters are standard SGR parameter chars (digits, semicolons, colons)
        seq[2..seq.len() - 1]
            .iter()
            .all(|&b| b.is_ascii_digit() || b == b';' || b == b':')
    } else {
        false
    }
}

/// Finds the end of an escape sequence starting at `bytes[start]`.
/// Returns the index of the first byte immediately following the sequence.
fn skip_escape_sequence(bytes: &[u8], start: usize) -> usize {
    let len = bytes.len();
    if start >= len || bytes[start] != 0x1b {
        return start + 1;
    }

    let mut i = start + 1;
    if i >= len {
        return len;
    }

    match bytes[i] {
        b'[' => {
            // CSI: \x1b [ <params> <intermediates> <final>
            // Parameters: 0x30..=0x3F ('0'-'9', ';', '?', etc.)
            // Intermediates: 0x20..=0x2F
            // Final: 0x40..=0x7E ('A'-'Z', 'a'-'z', etc.)
            i += 1;
            while i < len && (bytes[i] >= 0x20 && bytes[i] <= 0x3f) {
                i += 1;
            }
            if i < len && (bytes[i] >= 0x40 && bytes[i] <= 0x7e) {
                i += 1;
            }
            i
        }
        b']' => {
            // OSC: \x1b ] ... (terminated by BEL 0x07 or ST \x1b \)
            i += 1;
            while i < len {
                if bytes[i] == 0x07 {
                    i += 1;
                    break;
                }
                if bytes[i] == 0x1b && i + 1 < len && bytes[i + 1] == b'\\' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            i
        }
        b'P' | b'_' | b'^' | b'X' => {
            // DCS, APC, PM, SOS: terminated by ST (\x1b \)
            i += 1;
            while i < len {
                if bytes[i] == 0x1b && i + 1 < len && bytes[i + 1] == b'\\' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            i
        }
        0x20..=0x2f => {
            // Intermediate bytes followed by single ASCII final byte (0x30..=0x7e)
            while i < len && (0x20..=0x2f).contains(&bytes[i]) {
                i += 1;
            }
            if i < len && (0x30..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
            i
        }
        0x30..=0x7e => {
            // Standard 2-character ASCII escape sequence (e.g. \x1b=, \x1b>, \x1bM)
            i + 1
        }
        _ => {
            // Standalone/malformed ESC before control char or UTF-8 lead byte:
            // consume ONLY the ESC byte (0x1b) itself so UTF-8 char boundaries remain intact.
            i
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_sequence_before_multibyte_utf8_no_panic() {
        let input = "\x1b🦀\x1bé\x1b\u{FFFD}Hello";
        assert_eq!(strip_control_chars(input), "🦀é\u{FFFD}Hello");
        assert_eq!(filter_sgr_only(input), "🦀é\u{FFFD}Hello");
        assert_eq!(truncate_visible_width(input, 4), "🦀é\u{FFFD}");
    }

    #[test]
    fn test_strip_plain_text() {
        assert_eq!(strip_control_chars("Hello, World!"), "Hello, World!");
        assert_eq!(
            strip_control_chars("Tab\tSeparated\nLines"),
            "Tab\tSeparated\nLines"
        );
        assert_eq!(
            strip_control_chars("Unicode: 🦀 日本語 — done"),
            "Unicode: 🦀 日本語 — done"
        );
    }

    #[test]
    fn test_strip_control_codes() {
        // Null byte, BEL, backspace, carriage return
        assert_eq!(
            strip_control_chars("Evil\0Payload\x07Beep\x08Back\rOver"),
            "EvilPayloadBeepBackOver"
        );
        assert_eq!(strip_control_chars("Del\x7fChar"), "DelChar");
    }

    #[test]
    fn test_strip_csi_sequences() {
        // Clear screen \x1b[2J, cursor home \x1b[H
        assert_eq!(
            strip_control_chars("Before\x1b[2J\x1b[HAfter"),
            "BeforeAfter"
        );
        // Color sequence \x1b[31;1m
        assert_eq!(strip_control_chars("\x1b[31;1mRed Text\x1b[0m"), "Red Text");
        // Cursor movement \x1b[10;20H
        assert_eq!(strip_control_chars("Line\x1b[10;20HJump"), "LineJump");
    }

    #[test]
    fn test_strip_osc_sequences() {
        // OSC terminal title injection: \x1b]0;hacked\x07
        assert_eq!(
            strip_control_chars("Normal\x1b]0;Title Injection\x07Text"),
            "NormalText"
        );
        // OSC terminated by ST: \x1b]8;;http://evil.com\x1b\\Click\x1b]8;;\x1b\\
        assert_eq!(
            strip_control_chars("Visit \x1b]8;;https://example.com\x1b\\Link\x1b]8;;\x1b\\ here"),
            "Visit Link here"
        );
    }

    #[test]
    fn test_filter_sgr_preserves_colors() {
        let input = "\x1b[31mRed\x1b[0m and \x1b[1;32mBold Green\x1b[0m";
        assert_eq!(filter_sgr_only(input), input);

        // 24-bit truecolor
        let truecolor = "\x1b[38;2;255;100;50mCustom\x1b[0m";
        assert_eq!(filter_sgr_only(truecolor), truecolor);
    }

    #[test]
    fn test_filter_sgr_removes_dangerous_csi_and_osc() {
        let hostile = "\x1b[31mRed\x1b[2J\x1b[H\x1b[0m and \x1b]0;Evil\x07\x1b[32mGreen\x1b[0m";
        let filtered = filter_sgr_only(hostile);
        assert_eq!(filtered, "\x1b[31mRed\x1b[0m and \x1b[32mGreen\x1b[0m");
    }

    #[test]
    fn test_filter_sgr_strips_control_codes() {
        let input = "\x1b[33mWarning:\0\x07\r Overwrite\x1b[0m";
        assert_eq!(filter_sgr_only(input), "\x1b[33mWarning: Overwrite\x1b[0m");
    }

    #[test]
    fn test_visible_width() {
        assert_eq!(visible_width("hello"), 5);
        assert_eq!(visible_width("\x1b[31mhello\x1b[0m"), 5);
        assert_eq!(visible_width("hello\x1b[2Jworld"), 10);
        assert_eq!(visible_width("🦀"), 2);
    }

    #[test]
    fn test_truncate_visible_width() {
        let input = "\x1b[31mHello World\x1b[0m";
        assert_eq!(truncate_visible_width(input, 5), "\x1b[31mHello\x1b[0m");
        assert_eq!(truncate_visible_width("Hello World", 5), "Hello");
        assert_eq!(truncate_visible_width("🦀🦀🦀", 4), "🦀🦀");
    }

    #[test]
    fn test_strip_control_chars_cow_borrowed_vs_owned() {
        use std::borrow::Cow;
        let clean = "feat(core): implement zero-allocation paths\t\nvalid unicode: 🚀";
        let res = strip_control_chars(clean);
        assert!(matches!(res, Cow::Borrowed(_)));
        assert_eq!(res, clean);

        let dirty = "feat(core): evil\x07bell\x1b[2Jclear";
        let res_dirty = strip_control_chars(dirty);
        assert!(matches!(res_dirty, Cow::Owned(_)));
        assert_eq!(res_dirty, "feat(core): evilbellclear");
    }

    #[test]
    fn test_filter_sgr_cow_borrowed_vs_owned() {
        use std::borrow::Cow;
        let clean = "clean text with nothing to filter\t\n";
        let res = filter_sgr_only(clean);
        assert!(matches!(res, Cow::Borrowed(_)));
        assert_eq!(res, clean);

        let dirty = "\x1b[31mred\x1b[2Jclear\x1b[0m";
        let res_dirty = filter_sgr_only(dirty);
        assert!(matches!(res_dirty, Cow::Owned(_)));
        assert_eq!(res_dirty, "\x1b[31mredclear\x1b[0m");
    }

    #[test]
    fn test_filter_sgr_unicode_and_del() {
        let text = "\x1b[32mCommit: 🚀 \x7f\x00feat\x1b[0m";
        let res = filter_sgr_only(text);
        assert_eq!(res, "\x1b[32mCommit: 🚀 feat\x1b[0m");
    }

    #[test]
    fn test_truncate_visible_width_style_reset() {
        let text = "\x1b[31mHello World";
        let trunc = truncate_visible_width(text, 5);
        assert_eq!(trunc, "\x1b[31mHello\x1b[0m");

        // Trailing single escape character is stripped by skip_escape_sequence
        let trailing_esc = "foo\x1b";
        assert_eq!(truncate_visible_width(trailing_esc, 10), "foo");
    }

    #[test]
    fn test_skip_escape_sequence_edge_cases() {
        // Trailing escape at end of byte buffer
        let bytes = b"abc\x1b";
        assert_eq!(skip_escape_sequence(bytes, 3), 4);

        // Not an escape byte at start index
        assert_eq!(skip_escape_sequence(bytes, 1), 2);
        assert_eq!(skip_escape_sequence(bytes, 10), 11);

        // SGR sequences
        assert!(is_sgr_sequence(b"\x1b[m")); // Standard SGR reset
        assert!(!is_sgr_sequence(b"\x1b[invalidm"));
        assert!(!is_sgr_sequence(b"\x1b]0;title\x07"));
    }

    #[test]
    fn test_truncate_display_width_ascii() {
        assert_eq!(truncate_display_width("hello world", 5), "hello");
        assert_eq!(truncate_display_width("hello world", 11), "hello world");
        assert_eq!(truncate_display_width("hello world", 20), "hello world");
        assert_eq!(truncate_display_width("hello world", 0), "");
        assert_eq!(truncate_display_width("", 5), "");
    }

    #[test]
    fn test_truncate_display_width_unicode_and_cjk() {
        // CJK characters have display width 2
        assert_eq!(truncate_display_width("日本語", 4), "日本");
        assert_eq!(truncate_display_width("日本語", 5), "日本");
        assert_eq!(truncate_display_width("日本語", 6), "日本語");
        assert_eq!(truncate_display_width("🦀 crab", 4), "🦀 c");
        assert_eq!(truncate_display_width("🦀 crab", 3), "🦀 ");
        assert_eq!(truncate_display_width("🦀 crab", 2), "🦀");
        assert_eq!(truncate_display_width("🦀 crab", 1), "");
    }

    #[test]
    fn test_truncate_display_width_ellipsis() {
        assert_eq!(
            truncate_display_width_ellipsis("hello world", 8),
            "hello..."
        );
        assert_eq!(
            truncate_display_width_ellipsis("hello world", 15),
            "hello world"
        );
        assert_eq!(truncate_display_width_ellipsis("hello", 3), "hel");
        assert_eq!(truncate_display_width_ellipsis("hello", 2), "he");
        assert_eq!(
            truncate_display_width_ellipsis("path/to/very/long/file.rs", 12),
            "path/to/v..."
        );
    }

    #[test]
    fn test_strip_c1_controls_and_bidi_overrides() {
        // U+009B is 8-bit CSI, U+202E is Right-to-Left Override (Trojan Source)
        let input = "safe\u{009B}2J\u{202E}evil\u{2066}text\u{2069}";
        assert_eq!(strip_control_chars(input), "safe2Jeviltext");
        assert_eq!(filter_sgr_only(input), "safe2Jeviltext");
        assert_eq!(truncate_visible_width(input, 80), "safe2Jeviltext");
    }

    #[test]
    fn test_pad_display_width() {
        // "日本" has 2 chars, 4 display columns -> padding to width 6 adds 2 spaces (total 4 chars, 6 columns)
        let padded = pad_display_width("日本", 6);
        assert_eq!(padded, "日本  ");
        assert_eq!(unicode_width::UnicodeWidthStr::width(&*padded), 6);
    }
}
