// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Zero-allocation contiguous line buffer backed by `Arc<str>` and a compact `Arc<[u32]>` offset table.
//!
//! Replaces `Vec<String>` for large text blobs and files, reducing 100,000+ small heap allocations
//! to just 2 allocations (`Arc<str>` + `Arc<[u32]>`) while providing O(1) line indexing and slicing.

use crate::ansi::strip_control_chars;
use std::ops::Index;
use std::sync::Arc;

/// Contiguous text buffer with an O(1) line-offset lookup table.
///
/// # Offset Limit (4 GiB)
///
/// Line byte offsets are stored as `u32` (`Arc<[u32]>`) to halve index memory footprint
/// compared to `usize`. Inputs exceeding `u32::MAX` (~4 GiB) saturate offsets at `u32::MAX`
/// and clamp to valid UTF-8 boundaries when sliced.
///
/// # Line Endings (Read-Only)
///
/// Input with `\r\n` (CRLF) is normalized to `\n` in the internal contiguous buffer.
/// This structure is designed for read-only display and searching, not byte-for-byte
/// round-trip file serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineBuffer {
    text: Arc<str>,
    /// Byte offsets of the start of each line within `text`, plus a trailing sentinel `text.len()`.
    /// Empty if the buffer contains zero lines.
    line_starts: Arc<[u32]>,
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self::empty()
    }
}

impl LineBuffer {
    /// Creates an empty `LineBuffer` with zero lines and no heap allocations.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            text: Arc::from(""),
            line_starts: Arc::from([]),
        }
    }

    /// Builds a `LineBuffer` from raw bytes, checking for binary content (NUL in first 8000 bytes)
    /// and stripping terminal control characters while indexing line offsets in a single pass.
    ///
    /// Returns `(is_binary, LineBuffer)`.
    #[must_use]
    pub fn from_raw_bytes(data: &[u8]) -> (bool, Self) {
        let check_len = data.len().min(8000);
        if memchr::memchr(0, &data[..check_len]).is_some() {
            return (true, Self::empty());
        }
        if data.is_empty() {
            return (false, Self::empty());
        }

        // Fast path: valid UTF-8 with no C0 control characters (\r, \x1b, etc.), DELETE (0x7f),
        // UTF-8 C1 control lead byte (0xc2), or Unicode BiDi override lead byte (0xe2).
        if let Ok(utf8_str) = std::str::from_utf8(data) {
            let clean = !data.iter().any(|&b| {
                (b < 0x20 && b != b'\t' && b != b'\n') || b == 0x7f || b == 0xc2 || b == 0xe2
            });
            if clean {
                let mut starts = Vec::new();
                starts.push(0);
                for pos in memchr::memchr_iter(b'\n', data) {
                    let next = pos + 1;
                    if next < data.len() {
                        starts.push(u32::try_from(next).unwrap_or(u32::MAX));
                    }
                }
                let text: Arc<str> = if data.ends_with(b"\n") {
                    starts.push(u32::try_from(data.len()).unwrap_or(u32::MAX));
                    Arc::from(utf8_str)
                } else {
                    let mut owned = String::with_capacity(data.len() + 1);
                    owned.push_str(utf8_str);
                    owned.push('\n');
                    starts.push(u32::try_from(owned.len()).unwrap_or(u32::MAX));
                    Arc::from(owned)
                };
                return (
                    false,
                    Self {
                        text,
                        line_starts: Arc::from(starts),
                    },
                );
            }
        }

        let raw_text = String::from_utf8_lossy(data);
        let mut sanitized = String::with_capacity(raw_text.len());
        let mut starts = Vec::new();

        for line in raw_text.lines() {
            let start_pos = u32::try_from(sanitized.len()).unwrap_or(u32::MAX);
            starts.push(start_pos);
            let clean = strip_control_chars(line);
            sanitized.push_str(&clean);
            sanitized.push('\n');
        }

        if !starts.is_empty() {
            let end_pos = u32::try_from(sanitized.len()).unwrap_or(u32::MAX);
            starts.push(end_pos);
        }

        (
            false,
            Self {
                text: Arc::from(sanitized),
                line_starts: Arc::from(starts),
            },
        )
    }

    /// Creates a `LineBuffer` from a slice or iterator of string-like lines,
    /// sanitizing each line against terminal control sequences and `BiDi` overrides.
    #[must_use]
    pub fn from_lines<I, S>(lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let iter = lines.into_iter();
        let (lower, _) = iter.size_hint();
        let mut buf = String::with_capacity(lower * 32);
        let mut starts = Vec::with_capacity(lower.saturating_add(1));

        for line in iter {
            let s = strip_control_chars(line.as_ref());
            let start_pos = u32::try_from(buf.len()).unwrap_or(u32::MAX);
            starts.push(start_pos);
            buf.push_str(&s);
            buf.push('\n');
        }

        if !starts.is_empty() {
            let end_pos = u32::try_from(buf.len()).unwrap_or(u32::MAX);
            starts.push(end_pos);
        }

        Self {
            text: Arc::from(buf),
            line_starts: Arc::from(starts),
        }
    }

    /// Returns the number of lines in the buffer.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.line_starts.len().saturating_sub(1)
    }

    /// Returns `true` if the buffer contains zero lines.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the line at `index` (0-indexed) with trailing newline stripped, or `None` if out of bounds.
    #[inline]
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&str> {
        let mut start = (*self.line_starts.get(index)? as usize).min(self.text.len());
        let mut end = (*self.line_starts.get(index + 1)? as usize).min(self.text.len());
        while start > 0 && !self.text.is_char_boundary(start) {
            start -= 1;
        }
        while end > start && !self.text.is_char_boundary(end) {
            end -= 1;
        }
        let slice = self.text.get(start..end)?;
        let s = slice.strip_suffix('\n').unwrap_or(slice);
        Some(s.strip_suffix('\r').unwrap_or(s))
    }

    /// Returns an iterator over all lines in the buffer.
    #[inline]
    pub fn iter(&self) -> LineBufferIter<'_> {
        LineBufferIter {
            buffer: self,
            front: 0,
            back: self.len(),
        }
    }

    /// Materializes all lines into a `Vec<String>` (useful for interoperability with APIs requiring owned strings).
    #[must_use]
    pub fn to_vec(&self) -> Vec<String> {
        self.iter().map(str::to_string).collect()
    }

    /// Returns the underlying contiguous text slice.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl Index<usize> for LineBuffer {
    type Output = str;

    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("LineBuffer index out of bounds")
    }
}

impl<'a> IntoIterator for &'a LineBuffer {
    type Item = &'a str;
    type IntoIter = LineBufferIter<'a>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl From<Vec<String>> for LineBuffer {
    fn from(lines: Vec<String>) -> Self {
        Self::from_lines(&lines)
    }
}

impl From<&[String]> for LineBuffer {
    fn from(lines: &[String]) -> Self {
        Self::from_lines(lines)
    }
}

impl From<&[&str]> for LineBuffer {
    fn from(lines: &[&str]) -> Self {
        Self::from_lines(lines)
    }
}

impl<const N: usize> From<[&str; N]> for LineBuffer {
    fn from(lines: [&str; N]) -> Self {
        Self::from_lines(lines)
    }
}

impl<const N: usize> From<[String; N]> for LineBuffer {
    fn from(lines: [String; N]) -> Self {
        Self::from_lines(lines)
    }
}

impl PartialEq<Vec<&str>> for LineBuffer {
    fn eq(&self, other: &Vec<&str>) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == *b)
    }
}

impl PartialEq<Vec<String>> for LineBuffer {
    fn eq(&self, other: &Vec<String>) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == b.as_str())
    }
}

impl PartialEq<&[&str]> for LineBuffer {
    fn eq(&self, other: &&[&str]) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == *b)
    }
}

/// Iterator over the lines of a [`LineBuffer`].
#[derive(Debug, Clone)]
pub struct LineBufferIter<'a> {
    buffer: &'a LineBuffer,
    front: usize,
    back: usize,
}

impl<'a> Iterator for LineBufferIter<'a> {
    type Item = &'a str;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.front < self.back {
            let item = self.buffer.get(self.front);
            self.front += 1;
            item
        } else {
            None
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.back.saturating_sub(self.front);
        (len, Some(len))
    }

    #[inline]
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.front = self.front.saturating_add(n);
        self.next()
    }
}

impl ExactSizeIterator for LineBufferIter<'_> {}

impl DoubleEndedIterator for LineBufferIter<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front < self.back {
            self.back -= 1;
            self.buffer.get(self.back)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_buffer_basic() {
        let (is_bin, buf) = LineBuffer::from_raw_bytes(b"first line\nsecond line\r\nthird line");
        assert!(!is_bin);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.get(0), Some("first line"));
        assert_eq!(buf.get(1), Some("second line"));
        assert_eq!(buf.get(2), Some("third line"));
        assert_eq!(buf.get(3), None);
        assert_eq!(&buf[1], "second line");

        let collected: Vec<&str> = buf.iter().collect();
        assert_eq!(collected, vec!["first line", "second line", "third line"]);
    }

    #[test]
    fn test_line_buffer_binary_detection() {
        let (is_bin, buf) = LineBuffer::from_raw_bytes(b"hello\x00world");
        assert!(is_bin);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_line_buffer_from_vec() {
        let buf = LineBuffer::from(vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(buf.len(), 2);
        assert_eq!(&buf[0], "alpha");
        assert_eq!(&buf[1], "beta");
    }

    #[test]
    fn test_line_buffer_fast_path_vs_sanitized_equivalence() {
        let (bin1, clean_nl) = LineBuffer::from_raw_bytes(b"line 1\nline 2\nline 3\n");
        let (bin2, clean_no_nl) = LineBuffer::from_raw_bytes(b"line 1\nline 2\nline 3");
        let (bin3, crlf) = LineBuffer::from_raw_bytes(b"line 1\r\nline 2\r\nline 3\r\n");
        assert!(!bin1 && !bin2 && !bin3);
        assert_eq!(clean_nl, clean_no_nl);
        assert_eq!(clean_nl, crlf);
        assert_eq!(clean_nl.len(), 3);
        assert_eq!(&clean_nl[0], "line 1");
        assert_eq!(&clean_nl[1], "line 2");
        assert_eq!(&clean_nl[2], "line 3");
    }

    #[test]
    fn test_line_buffer_strips_c1_controls_and_bidi_overrides_without_c0_chars() {
        // Pure UTF-8 input with NO ASCII C0 controls (< 0x20), containing:
        // - U+009B (8-bit CSI, 0xC2 0x9B) and U+009D (8-bit OSC, 0xC2 0x9D)
        // - U+202E (Right-to-Left Override, 0xE2 0x80 0xAE) and U+2066..U+2069 (BiDi isolates)
        let payload = "fn check() { // \u{202e}evil\u{202c}\nlet x = \"\u{009b}2J\u{009d}52;c;dGVzdA==\u{009c}\u{2066}hidden\u{2069}\";\n";
        let (is_bin, buf) = LineBuffer::from_raw_bytes(payload.as_bytes());
        assert!(!is_bin);
        assert_eq!(buf.len(), 2);
        assert_eq!(&buf[0], "fn check() { // evil");
        assert_eq!(&buf[1], "let x = \"2J52;c;dGVzdA==hidden\";");

        let from_lines = LineBuffer::from(vec![payload.lines().next().unwrap().to_string()]);
        assert_eq!(&from_lines[0], "fn check() { // evil");
    }

    #[test]
    fn test_line_buffer_traits_conversions_and_double_ended_iter() {
        let default_buf = LineBuffer::default();
        assert!(default_buf.is_empty());
        assert_eq!(default_buf.len(), 0);
        assert_eq!(default_buf.as_str(), "");
        assert_eq!(default_buf.to_vec(), Vec::<String>::new());

        let (is_bin_empty, empty_raw) = LineBuffer::from_raw_bytes(b"");
        assert!(!is_bin_empty);
        assert!(empty_raw.is_empty());

        let arr_str = LineBuffer::from(["alpha", "beta", "gamma"]);
        let slice_str: &[&str] = &["alpha", "beta", "gamma"];
        let from_slice_str = LineBuffer::from(slice_str);
        let owned_strings = ["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let from_arr_string = LineBuffer::from(owned_strings.clone());
        let from_slice_string = LineBuffer::from(owned_strings.as_slice());

        assert_eq!(arr_str, from_slice_str);
        assert_eq!(arr_str, from_arr_string);
        assert_eq!(arr_str, from_slice_string);
        assert_eq!(arr_str, vec!["alpha", "beta", "gamma"]);
        assert_eq!(arr_str, owned_strings.to_vec());
        assert_eq!(arr_str, slice_str);
        assert_eq!(arr_str.to_vec(), owned_strings.to_vec());
        assert_eq!(arr_str.as_str(), "alpha\nbeta\ngamma\n");

        let mut via_ref = Vec::new();
        for line in &arr_str {
            via_ref.push(line);
        }
        assert_eq!(via_ref, vec!["alpha", "beta", "gamma"]);

        let mut iter = arr_str.iter();
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.size_hint(), (3, Some(3)));
        assert_eq!(iter.next_back(), Some("gamma"));
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.nth(1), Some("beta"));
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }
}
