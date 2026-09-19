// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Line run alignment algorithms for word-diff intra-line highlighting
//! and side-by-side row pairing.
//!
//! Provides `AlignPolicy::Similarity` and `AlignPolicy::Positional`.

use crate::options::WordDiffPairing;

/// Maximum line run length for O(N*M) similarity alignment to adhere to the <= 1ms budget.
pub const MAX_ALIGN_RUN_LEN: usize = 64;

/// Strategy used to pair removed lines with added lines in a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignPolicy {
    /// Similarity-based pairing using token overlap, matching asymmetric additions and removals.
    Similarity,
    /// 1:1 index-based positional pairing (diff-highlight legacy style).
    Positional,
}

impl From<WordDiffPairing> for AlignPolicy {
    fn from(p: WordDiffPairing) -> Self {
        match p {
            WordDiffPairing::Similarity => Self::Similarity,
            WordDiffPairing::Positional => Self::Positional,
        }
    }
}

/// Tokenizes a line into lightweight word slices for similarity comparison.
fn tokenize(s: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        if c.is_alphanumeric() || c == '_' {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(st) = start.take() {
            tokens.push(&s[st..i]);
        }
    }
    if let Some(st) = start {
        tokens.push(&s[st..]);
    }
    tokens
}

/// Computes normalized token overlap similarity between two lines in `0.0..=1.0`.
#[allow(clippy::cast_precision_loss)]
fn line_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let tok_a = tokenize(a);
    let tok_b = tokenize(b);
    if tok_a.is_empty() && tok_b.is_empty() {
        return 1.0;
    }
    if tok_a.is_empty() || tok_b.is_empty() {
        return 0.0;
    }

    let mut matches = 0usize;
    let mut b_used = vec![false; tok_b.len()];

    for ta in &tok_a {
        for (j, tb) in tok_b.iter().enumerate() {
            if !b_used[j] && ta == tb {
                b_used[j] = true;
                matches += 1;
                break;
            }
        }
    }

    let union = tok_a.len() + tok_b.len() - matches;
    if union == 0 {
        0.0
    } else {
        (matches as f64) / (union as f64)
    }
}

/// Minimum similarity threshold to consider two lines a modification pair rather than separate delete/add.
const MIN_SIMILARITY_THRESHOLD: f64 = 0.35;

/// Aligns a contiguous run of removed lines against a contiguous run of added lines.
///
/// Returns a sequence of paired rows `(Option<old_idx>, Option<new_idx>)` in document order.
#[must_use]
pub fn align_runs(
    old_lines: &[&str],
    new_lines: &[&str],
    policy: AlignPolicy,
) -> Vec<(Option<usize>, Option<usize>)> {
    if old_lines.is_empty() {
        return (0..new_lines.len()).map(|j| (None, Some(j))).collect();
    }
    if new_lines.is_empty() {
        return (0..old_lines.len()).map(|i| (Some(i), None)).collect();
    }

    // Fall back to positional if run is excessive or if positional policy is requested
    if policy == AlignPolicy::Positional
        || old_lines.len() > MAX_ALIGN_RUN_LEN
        || new_lines.len() > MAX_ALIGN_RUN_LEN
    {
        return align_positional(old_lines.len(), new_lines.len());
    }

    align_similarity(old_lines, new_lines)
}

/// Positional alignment: zips min(`len_a`, `len_b`), then appends remainders.
fn align_positional(len_a: usize, len_b: usize) -> Vec<(Option<usize>, Option<usize>)> {
    let min_len = len_a.min(len_b);
    let mut out = Vec::with_capacity(len_a.max(len_b));

    for i in 0..min_len {
        out.push((Some(i), Some(i)));
    }
    for i in min_len..len_a {
        out.push((Some(i), None));
    }
    for j in min_len..len_b {
        out.push((None, Some(j)));
    }
    out
}

/// Similarity alignment using Needleman-Wunsch global sequence alignment.
fn align_similarity(old_lines: &[&str], new_lines: &[&str]) -> Vec<(Option<usize>, Option<usize>)> {
    let n = old_lines.len();
    let m = new_lines.len();

    // Cost matrix: dp[i][j] stores optimal alignment score
    let gap_penalty = -0.25;
    let mut dp = vec![vec![0.0f64; m + 1]; n + 1];

    for (i, row) in dp.iter_mut().enumerate().skip(1) {
        #[allow(clippy::cast_precision_loss)]
        let score = (i as f64) * gap_penalty;
        row[0] = score;
    }
    for (j, cell) in dp[0].iter_mut().enumerate().skip(1) {
        #[allow(clippy::cast_precision_loss)]
        let score = (j as f64) * gap_penalty;
        *cell = score;
    }

    for i in 1..=n {
        for j in 1..=m {
            let sim = line_similarity(old_lines[i - 1], new_lines[j - 1]);
            let match_score = if sim >= MIN_SIMILARITY_THRESHOLD {
                sim
            } else {
                -0.5
            };

            let score_diag = dp[i - 1][j - 1] + match_score;
            let score_up = dp[i - 1][j] + gap_penalty;
            let score_left = dp[i][j - 1] + gap_penalty;

            dp[i][j] = score_diag.max(score_up).max(score_left);
        }
    }

    // Traceback to reconstruct the alignment
    let mut i = n;
    let mut j = m;
    let mut rev_out = Vec::with_capacity(n + m);

    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let sim = line_similarity(old_lines[i - 1], new_lines[j - 1]);
            let match_score = if sim >= MIN_SIMILARITY_THRESHOLD {
                sim
            } else {
                -0.5
            };
            let diag = dp[i - 1][j - 1] + match_score;

            if (dp[i][j] - diag).abs() < 1e-6 && sim >= MIN_SIMILARITY_THRESHOLD {
                rev_out.push((Some(i - 1), Some(j - 1)));
                i -= 1;
                j -= 1;
                continue;
            }
        }

        if i > 0 && (j == 0 || (dp[i][j] - (dp[i - 1][j] + gap_penalty)).abs() < 1e-6) {
            rev_out.push((Some(i - 1), None));
            i -= 1;
        } else if j > 0 {
            rev_out.push((None, Some(j - 1)));
            j -= 1;
        }
    }

    rev_out.reverse();
    rev_out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_align_empty_runs() {
        assert_eq!(
            align_runs(&[], &["a"], AlignPolicy::Similarity),
            vec![(None, Some(0))]
        );
        assert_eq!(
            align_runs(&["a"], &[], AlignPolicy::Similarity),
            vec![(Some(0), None)]
        );
        assert_eq!(align_runs(&[], &[], AlignPolicy::Similarity), vec![]);
    }

    #[test]
    fn test_align_positional() {
        let old = ["one", "two", "three"];
        let new = ["one modified", "two modified"];
        let res = align_runs(&old, &new, AlignPolicy::Positional);
        assert_eq!(
            res,
            vec![(Some(0), Some(0)), (Some(1), Some(1)), (Some(2), None),]
        );
    }

    #[test]
    fn test_align_similarity_diff_highlight_failure_case() {
        // Classic git diff-highlight failure case documented in its README:
        // Removed: ["one", "two", "three", "four"]
        // Added:   ["two 2", "three 3", "four 4", "five 5"]
        let old = ["one", "two", "three", "four"];
        let new = ["two 2", "three 3", "four 4", "five 5"];

        let res = align_runs(&old, &new, AlignPolicy::Similarity);

        // Under similarity, "one" is removed standalone,
        // while "two" pairs with "two 2", "three" with "three 3", "four" with "four 4",
        // and "five 5" is added standalone!
        assert_eq!(
            res,
            vec![
                (Some(0), None),    // "one" deleted
                (Some(1), Some(0)), // "two" -> "two 2"
                (Some(2), Some(1)), // "three" -> "three 3"
                (Some(3), Some(2)), // "four" -> "four 4"
                (None, Some(3)),    // "five 5" added
            ]
        );
    }
}
