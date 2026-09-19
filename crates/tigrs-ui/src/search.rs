// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Cancellable progressive search engine and active match tracking for tigrs.
//!
//! Provides regex and smart-case pattern matching, forward and backward directional
//! searching with wrap-around detection, cooperative cancellation via [`CancellationToken`],
//! and in-line visual match highlighting.

use tigrs_core::cancel::CancellationToken;

/// Number of items searched between cancellation flag checks to guarantee responsive UI.
const CANCELLATION_CHECK_INTERVAL: usize = 256;

/// The direction of a search query traversal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchDirection {
    /// Search forward from cursor toward end, wrapping to beginning.
    Forward,
    /// Search backward from cursor toward beginning, wrapping to end.
    Backward,
}

impl SearchDirection {
    /// Returns the inverted direction.
    #[must_use]
    pub fn reverse(self) -> Self {
        match self {
            Self::Forward => Self::Backward,
            Self::Backward => Self::Forward,
        }
    }

    /// Returns `true` if searching forward.
    #[must_use]
    pub fn is_forward(self) -> bool {
        matches!(self, Self::Forward)
    }
}

/// A compiled search pattern supporting regex with graceful literal fallback.
#[derive(Clone, Debug)]
pub struct SearchPattern {
    /// Raw unparsed query string entered by the user.
    raw: String,
    /// Whether matching is case-sensitive.
    case_sensitive: bool,
    /// Precompiled regex if the pattern is a valid regular expression.
    regex: Option<regex::Regex>,
}

impl PartialEq for SearchPattern {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw && self.case_sensitive == other.case_sensitive
    }
}

impl Eq for SearchPattern {}

impl SearchPattern {
    /// Creates a new search pattern.
    ///
    /// If `case_sensitive` is `None`, smart-case is applied: the query is
    /// case-sensitive if it contains any uppercase characters, and case-insensitive
    /// otherwise. If regex compilation fails, literal string search is used as a fallback.
    #[must_use]
    pub fn new(query: &str, case_sensitive: Option<bool>) -> Self {
        let is_case_sensitive =
            case_sensitive.unwrap_or_else(|| query.chars().any(char::is_uppercase));

        let regex = regex::RegexBuilder::new(query)
            .case_insensitive(!is_case_sensitive)
            .build()
            .ok();

        Self {
            raw: query.to_string(),
            case_sensitive: is_case_sensitive,
            regex,
        }
    }

    /// Returns the raw search query string.
    #[must_use]
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns `true` if the search is case-sensitive.
    #[must_use]
    pub fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// Returns `true` if `text` matches this search pattern.
    #[must_use]
    pub fn is_match(&self, text: &str) -> bool {
        if let Some(ref re) = self.regex {
            re.is_match(text)
        } else if self.case_sensitive {
            text.contains(&self.raw)
        } else {
            text.to_ascii_lowercase()
                .contains(&self.raw.to_ascii_lowercase())
        }
    }

    /// Returns `true` if `id`'s 40-character hex representation matches this search pattern
    /// without allocating a heap `String`.
    #[must_use]
    pub fn is_match_oid(&self, id: &tigrs_git::ObjectId) -> bool {
        let mut buf = [0u8; 40];
        let mut slice: &mut [u8] = &mut buf;
        if id.write_hex_to(&mut slice).is_ok()
            && let Ok(s) = std::str::from_utf8(&buf)
        {
            return self.is_match(s);
        }
        false
    }

    /// Finds all non-overlapping matching byte ranges `(start, end)` in `text`.
    #[must_use]
    pub fn find_matches(&self, text: &str) -> Vec<(usize, usize)> {
        if self.raw.is_empty() {
            return Vec::new();
        }

        if let Some(ref re) = self.regex {
            re.find_iter(text)
                .filter(|m| !m.is_empty())
                .map(|m| (m.start(), m.end()))
                .collect()
        } else {
            let mut matches = Vec::new();
            if self.case_sensitive {
                let mut start = 0;
                while let Some(pos) = text[start..].find(&self.raw) {
                    let match_start = start + pos;
                    let match_end = match_start + self.raw.len();
                    matches.push((match_start, match_end));
                    start = match_end;
                }
            } else {
                let needle = self.raw.to_ascii_lowercase();
                let haystack = text.to_ascii_lowercase();
                let mut start = 0;
                while let Some(pos) = haystack[start..].find(&needle) {
                    let match_start = start + pos;
                    let match_end = match_start + needle.len();
                    matches.push((match_start, match_end));
                    start = match_end;
                }
            }
            matches
        }
    }
}

/// The result of executing a search across a sequence of items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchResult {
    /// A matching item was found at the given index.
    Found {
        /// Index of the matching item.
        index: usize,
        /// Whether the search had to wrap around the beginning or end of the collection.
        wrapped: bool,
    },
    /// No item in the collection matched the search pattern.
    NotFound,
    /// Search was aborted early via [`CancellationToken`].
    Cancelled,
}

/// Active search configuration stored in application state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveSearch {
    /// The active pattern being searched.
    pub pattern: SearchPattern,
    /// The current search direction.
    pub direction: SearchDirection,
}

impl ActiveSearch {
    /// Creates a new active search.
    #[must_use]
    pub fn new(query: &str, direction: SearchDirection) -> Self {
        Self {
            pattern: SearchPattern::new(query, None),
            direction,
        }
    }
}

/// Progressively searches an indexed collection of items, checking for cancellation periodically.
///
/// Iterates across items in the specified direction starting from `start_cursor`. If no match
/// is found before reaching the edge, wraps around to check the remaining items.
pub fn search_items<F>(
    total_count: usize,
    start_cursor: usize,
    direction: SearchDirection,
    pattern: &SearchPattern,
    token: Option<&CancellationToken>,
    mut item_matches: F,
) -> SearchResult
where
    F: FnMut(usize, &SearchPattern) -> bool,
{
    if total_count == 0 || pattern.raw().is_empty() {
        return SearchResult::NotFound;
    }

    let start = start_cursor.min(total_count - 1);
    let mut checked_count = 0;

    match direction {
        SearchDirection::Forward => {
            // First phase: (start + 1 .. total_count)
            for idx in (start + 1)..total_count {
                checked_count += 1;
                if checked_count % CANCELLATION_CHECK_INTERVAL == 0
                    && let Some(tok) = token
                    && tok.is_cancelled()
                {
                    return SearchResult::Cancelled;
                }
                if item_matches(idx, pattern) {
                    return SearchResult::Found {
                        index: idx,
                        wrapped: false,
                    };
                }
            }

            // Second phase: wrap around from 0 up to start
            for idx in 0..=start {
                checked_count += 1;
                if checked_count % CANCELLATION_CHECK_INTERVAL == 0
                    && let Some(tok) = token
                    && tok.is_cancelled()
                {
                    return SearchResult::Cancelled;
                }
                if item_matches(idx, pattern) {
                    return SearchResult::Found {
                        index: idx,
                        wrapped: true,
                    };
                }
            }
        }
        SearchDirection::Backward => {
            // First phase: (0 .. start).rev()
            if start > 0 {
                for idx in (0..start).rev() {
                    checked_count += 1;
                    if checked_count % CANCELLATION_CHECK_INTERVAL == 0
                        && let Some(tok) = token
                        && tok.is_cancelled()
                    {
                        return SearchResult::Cancelled;
                    }
                    if item_matches(idx, pattern) {
                        return SearchResult::Found {
                            index: idx,
                            wrapped: false,
                        };
                    }
                }
            }

            // Second phase: wrap around from (start .. total_count).rev()
            for idx in (start..total_count).rev() {
                checked_count += 1;
                if checked_count % CANCELLATION_CHECK_INTERVAL == 0
                    && let Some(tok) = token
                    && tok.is_cancelled()
                {
                    return SearchResult::Cancelled;
                }
                if item_matches(idx, pattern) {
                    return SearchResult::Found {
                        index: idx,
                        wrapped: true,
                    };
                }
            }
        }
    }

    SearchResult::NotFound
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_pattern_smart_case() {
        let pat_lower = SearchPattern::new("hello", None);
        assert!(!pat_lower.is_case_sensitive());
        assert!(pat_lower.is_match("Hello World"));
        assert!(pat_lower.is_match("HELLO WORLD"));
        assert!(pat_lower.is_match("hello world"));

        let pat_upper = SearchPattern::new("Hello", None);
        assert!(pat_upper.is_case_sensitive());
        assert!(pat_upper.is_match("Hello World"));
        assert!(!pat_upper.is_match("hello world"));
    }

    #[test]
    fn test_search_pattern_regex_and_fallback() {
        let pat_regex = SearchPattern::new(r"feat\(\w+\):", None);
        assert!(pat_regex.is_match("feat(ui): add search"));
        assert!(!pat_regex.is_match("fix: nothing"));

        // Invalid regex syntax fallback to literal
        let pat_invalid = SearchPattern::new("[unclosed", None);
        assert!(pat_invalid.regex.is_none());
        assert!(pat_invalid.is_match("test [unclosed tag"));
        assert!(!pat_invalid.is_match("test [other tag"));
    }

    #[test]
    fn test_search_items_forward_and_wrap() {
        let items = ["apple", "banana", "cherry", "date", "banana 2"];
        let pat = SearchPattern::new("banana", None);

        // Start at 0, finds banana at 1 without wrapping
        let res = search_items(
            items.len(),
            0,
            SearchDirection::Forward,
            &pat,
            None,
            |idx, p| p.is_match(items[idx]),
        );
        assert_eq!(
            res,
            SearchResult::Found {
                index: 1,
                wrapped: false
            }
        );

        // Start at 1, finds banana 2 at 4 without wrapping
        let res = search_items(
            items.len(),
            1,
            SearchDirection::Forward,
            &pat,
            None,
            |idx, p| p.is_match(items[idx]),
        );
        assert_eq!(
            res,
            SearchResult::Found {
                index: 4,
                wrapped: false
            }
        );

        // Start at 4, wraps around and finds banana at 1
        let res = search_items(
            items.len(),
            4,
            SearchDirection::Forward,
            &pat,
            None,
            |idx, p| p.is_match(items[idx]),
        );
        assert_eq!(
            res,
            SearchResult::Found {
                index: 1,
                wrapped: true
            }
        );

        // Not found
        let pat_missing = SearchPattern::new("orange", None);
        let res = search_items(
            items.len(),
            0,
            SearchDirection::Forward,
            &pat_missing,
            None,
            |idx, p| p.is_match(items[idx]),
        );
        assert_eq!(res, SearchResult::NotFound);
    }

    #[test]
    fn test_search_items_backward_and_wrap() {
        let items = ["banana 1", "apple", "cherry", "banana 2"];
        let pat = SearchPattern::new("banana", None);

        // Start at 3, finds banana 1 at 0 without wrapping
        let res = search_items(
            items.len(),
            3,
            SearchDirection::Backward,
            &pat,
            None,
            |idx, p| p.is_match(items[idx]),
        );
        assert_eq!(
            res,
            SearchResult::Found {
                index: 0,
                wrapped: false
            }
        );

        // Start at 0, wraps around and finds banana 2 at 3
        let res = search_items(
            items.len(),
            0,
            SearchDirection::Backward,
            &pat,
            None,
            |idx, p| p.is_match(items[idx]),
        );
        assert_eq!(
            res,
            SearchResult::Found {
                index: 3,
                wrapped: true
            }
        );
    }

    #[test]
    fn test_search_cancellation() {
        let items: Vec<String> = (0..1000).map(|i| format!("item {i}")).collect();
        let pat = SearchPattern::new("item 999", None);

        let (source, token) = CancellationToken::new();
        source.cancel();

        let res = search_items(
            items.len(),
            0,
            SearchDirection::Forward,
            &pat,
            Some(&token),
            |idx, p| p.is_match(&items[idx]),
        );
        assert_eq!(res, SearchResult::Cancelled);

        // Backward search cancellation
        let res_back = search_items(
            items.len(),
            items.len() - 1,
            SearchDirection::Backward,
            &pat,
            Some(&token),
            |idx, p| p.is_match(&items[idx]),
        );
        assert_eq!(res_back, SearchResult::Cancelled);
    }

    #[test]
    fn test_search_direction_and_active_search() {
        assert_eq!(
            SearchDirection::Forward.reverse(),
            SearchDirection::Backward
        );
        assert_eq!(
            SearchDirection::Backward.reverse(),
            SearchDirection::Forward
        );
        assert!(SearchDirection::Forward.is_forward());
        assert!(!SearchDirection::Backward.is_forward());

        let active = ActiveSearch::new("needle", SearchDirection::Backward);
        assert_eq!(active.direction, SearchDirection::Backward);
        assert_eq!(active.pattern.raw, "needle");

        // Zero items search
        let res = search_items(
            0,
            0,
            SearchDirection::Forward,
            &active.pattern,
            None,
            |_idx, _p| true,
        );
        assert_eq!(res, SearchResult::NotFound);
    }

    #[test]
    fn test_search_pattern_equality_and_literal_matching() {
        let p1 = SearchPattern::new("test", None);
        let p2 = SearchPattern::new("test", None);
        let p3 = SearchPattern::new("Test", None);
        assert_eq!(p1, p2);
        assert_ne!(p1, p3);

        // Literal pattern (invalid regex)
        let lit_case_insensitive = SearchPattern::new("[bracket", None);
        assert!(lit_case_insensitive.regex.is_none());
        assert_eq!(
            lit_case_insensitive
                .find_matches("start [bracket middle [BRACKET end")
                .len(),
            2
        );

        let lit_case_sensitive = SearchPattern::new("[Bracket", None);
        assert_eq!(
            lit_case_sensitive
                .find_matches("start [bracket middle [Bracket end")
                .len(),
            1
        );
    }
}
