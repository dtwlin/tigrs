// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Word-level (intra-line) diffing for paired removed/added hunk lines.
//!
//! Unified diffs mark a modified line as a whole-line delete plus a whole-line
//! add, which forces the reader to scan both lines character by character to
//! find what actually changed. This module runs a second, token-level diff pass
//! over such a pair and reports the character ranges that differ, so the view
//! layer can emphasise only those ranges.
//!
//! This is a **presentation-only** concern. Nothing here feeds
//! [`crate::patch`], so it can never perturb byte-exact `git`-compatible patch
//! output.

use gix::diff::blob::{Algorithm, Diff, InternedInput};

/// Longest line (in characters) eligible for word-level diffing.
///
/// Minified bundles and generated single-line data files blow up the token
/// count without producing a readable result, so they are skipped outright.
pub const MAX_WORD_DIFF_LINE_CHARS: usize = 1024;

/// Percentage of a line that may change before the pair is considered
/// unrelated and emphasis is suppressed.
///
/// When two lines share almost nothing, marking the differences highlights
/// nearly the whole line, which is noisier than leaving it plain.
const MAX_CHANGED_PERCENT: usize = 70;

/// Half-open range of character offsets within a single line of text.
///
/// Offsets are in `char`s, not bytes, because the renderer truncates and pads
/// by terminal column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharSpan {
    /// First character in the span.
    pub start: usize,
    /// One past the last character in the span.
    pub end: usize,
}

impl CharSpan {
    /// Number of characters covered by this span.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Returns true when the span covers no characters.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// Character ranges that differ between a removed line and its paired added
/// line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WordDiff {
    /// Ranges to emphasise on the removed (`-`) line.
    pub old: Vec<CharSpan>,
    /// Ranges to emphasise on the added (`+`) line.
    pub new: Vec<CharSpan>,
}

/// Character class used to group adjacent characters into one token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Identifier-like run (alphanumeric or `_`).
    Word,
    /// Run of whitespace.
    Space,
    /// Anything else; each character stands alone.
    Punct,
}

fn classify(ch: char) -> Class {
    if ch.is_alphanumeric() || ch == '_' {
        Class::Word
    } else if ch.is_whitespace() {
        Class::Space
    } else {
        Class::Punct
    }
}

/// A token together with its character offsets in the source line.
#[derive(Debug, Clone, Copy)]
struct Token<'a> {
    /// Character offset of the first character.
    start: usize,
    /// Character offset one past the last character.
    end: usize,
    /// The token text, used as the interning key.
    text: &'a str,
}

/// Splits a line into word, whitespace, and single-punctuation tokens.
///
/// Grouping identifiers into one token (rather than diffing per character)
/// keeps the emphasis aligned with what a reader perceives as "the thing that
/// changed": renaming `foo_bar` to `foo_baz` marks one word, not one letter.
/// Punctuation is deliberately per-character so that a change from `(a, b)` to
/// `(a; b)` marks just the separator.
fn tokenize(line: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut chars = line.char_indices().peekable();
    let mut char_idx = 0usize;

    while let Some((byte_start, ch)) = chars.next() {
        let start_char = char_idx;
        char_idx += 1;
        let mut byte_end = byte_start + ch.len_utf8();
        let class = classify(ch);

        if class != Class::Punct {
            while let Some(&(next_byte, next_ch)) = chars.peek() {
                if classify(next_ch) == class {
                    chars.next();
                    char_idx += 1;
                    byte_end = next_byte + next_ch.len_utf8();
                } else {
                    break;
                }
            }
        }

        tokens.push(Token {
            start: start_char,
            end: char_idx,
            text: &line[byte_start..byte_end],
        });
    }

    tokens
}

/// Collapses a contiguous run of tokens into a single character span,
/// merging with the previous span when they abut.
fn push_span(spans: &mut Vec<CharSpan>, tokens: &[Token<'_>], range: std::ops::Range<usize>) {
    if range.is_empty() {
        return;
    }
    let Some(first) = tokens.get(range.start) else {
        return;
    };
    let Some(last) = tokens.get(range.end - 1) else {
        return;
    };

    let span = CharSpan {
        start: first.start,
        end: last.end,
    };

    if let Some(prev) = spans.last_mut()
        && prev.end == span.start
    {
        prev.end = span.end;
        return;
    }
    spans.push(span);
}

/// Total number of characters covered by a set of spans.
fn covered(spans: &[CharSpan]) -> usize {
    spans.iter().map(CharSpan::len).sum()
}

/// Computes the differing character ranges between a removed line and its
/// paired added line.
///
/// Returns `None` when emphasis would not help the reader:
///
/// - either line is empty,
/// - either line exceeds [`MAX_WORD_DIFF_LINE_CHARS`],
/// - the lines are identical,
/// - or the lines are so dissimilar that more than
///   `MAX_CHANGED_PERCENT` of *both* sides changed.
///
/// The histogram algorithm is used rather than Myers because it produces more
/// intuitive groupings when a token repeats many times on one line (commas,
/// brackets, repeated keywords). Patch-parity concerns do not apply: this
/// output is never serialised into a patch.
#[must_use]
pub fn word_diff(old: &str, new: &str) -> Option<WordDiff> {
    if old.is_empty() || new.is_empty() || old == new {
        return None;
    }

    let old_chars = old.chars().count();
    let new_chars = new.chars().count();
    if old_chars > MAX_WORD_DIFF_LINE_CHARS || new_chars > MAX_WORD_DIFF_LINE_CHARS {
        return None;
    }

    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);

    let mut input: InternedInput<&str> = InternedInput::default();
    input.update_before(old_tokens.iter().map(|t| t.text));
    input.update_after(new_tokens.iter().map(|t| t.text));

    let diff = Diff::compute(Algorithm::Histogram, &input);

    let mut result = WordDiff::default();
    for hunk in diff.hunks() {
        push_span(
            &mut result.old,
            &old_tokens,
            hunk.before.start as usize..hunk.before.end as usize,
        );
        push_span(
            &mut result.new,
            &new_tokens,
            hunk.after.start as usize..hunk.after.end as usize,
        );
    }

    if result.old.is_empty() && result.new.is_empty() {
        return None;
    }

    // Suppress emphasis on pairs that share almost nothing. Both sides must be
    // heavily rewritten before we give up: replacing a short line with a long
    // one still benefits from marking the common prefix.
    let old_changed = covered(&result.old) * 100;
    let new_changed = covered(&result.new) * 100;
    if old_changed > old_chars * MAX_CHANGED_PERCENT
        && new_changed > new_chars * MAX_CHANGED_PERCENT
    {
        return None;
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(line: &str, spans: &[CharSpan]) -> Vec<String> {
        let chars: Vec<char> = line.chars().collect();
        spans
            .iter()
            .map(|s| chars[s.start..s.end].iter().collect())
            .collect()
    }

    #[test]
    fn test_single_word_change_is_isolated() {
        let old = "let value = compute_total(a, b);";
        let new = "let value = compute_sum(a, b);";
        let wd = word_diff(old, new).expect("lines are similar");

        assert_eq!(spans(old, &wd.old), vec!["compute_total"]);
        assert_eq!(spans(new, &wd.new), vec!["compute_sum"]);
    }

    #[test]
    fn test_punctuation_change_marks_only_separator() {
        let old = "foo(a, b)";
        let new = "foo(a; b)";
        let wd = word_diff(old, new).expect("lines are similar");

        assert_eq!(spans(old, &wd.old), vec![","]);
        assert_eq!(spans(new, &wd.new), vec![";"]);
    }

    #[test]
    fn test_pure_insertion_marks_nothing_on_old_side() {
        let old = "fn run() {";
        let new = "pub fn run() {";
        let wd = word_diff(old, new).expect("lines are similar");

        assert!(wd.old.is_empty(), "nothing was removed: {:?}", wd.old);
        assert_eq!(spans(new, &wd.new), vec!["pub "]);
    }

    #[test]
    fn test_identical_lines_return_none() {
        assert!(word_diff("same", "same").is_none());
    }

    #[test]
    fn test_empty_line_returns_none() {
        assert!(word_diff("", "x").is_none());
        assert!(word_diff("x", "").is_none());
    }

    #[test]
    fn test_unrelated_lines_return_none() {
        // Nothing in common: emphasising everything is noise.
        assert!(word_diff("aaaa bbbb cccc", "zzzz yyyy xxxx").is_none());
    }

    #[test]
    fn test_overlong_line_returns_none() {
        let long = "x".repeat(MAX_WORD_DIFF_LINE_CHARS + 1);
        let other = format!("{long}y");
        assert!(word_diff(&long, &other).is_none());
    }

    #[test]
    fn test_multibyte_offsets_are_char_based() {
        let old = "let s = \"héllo wörld\";";
        let new = "let s = \"héllo there\";";
        let wd = word_diff(old, new).expect("lines are similar");

        // Spans must index characters, not bytes, or the umlauts shift them.
        assert_eq!(spans(old, &wd.old), vec!["wörld"]);
        assert_eq!(spans(new, &wd.new), vec!["there"]);
    }

    #[test]
    fn test_adjacent_changes_merge_into_one_span() {
        let old = "a = 1;";
        let new = "a = 22;";
        let wd = word_diff(old, new).expect("lines are similar");

        assert_eq!(wd.old.len(), 1, "spans should merge: {:?}", wd.old);
        assert_eq!(spans(old, &wd.old), vec!["1"]);
        assert_eq!(spans(new, &wd.new), vec!["22"]);
    }

    #[test]
    fn test_whitespace_only_change_is_marked() {
        let old = "if (x)";
        let new = "if  (x)";
        let wd = word_diff(old, new).expect("lines are similar");
        assert!(!wd.new.is_empty());
    }

    #[test]
    fn test_spans_are_sorted_and_non_overlapping() {
        let old = "alpha beta gamma delta";
        let new = "alpha BETA gamma DELTA";
        let wd = word_diff(old, new).expect("lines are similar");

        for spans in [&wd.old, &wd.new] {
            for pair in spans.windows(2) {
                assert!(
                    pair[0].end <= pair[1].start,
                    "overlapping or unsorted spans: {spans:?}"
                );
            }
        }
    }
}
