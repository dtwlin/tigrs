// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Git's userdiff drivers: the per-language patterns that produce the function
//! context shown after `@@ ... @@` in a hunk header.
//!
//! Git resolves the `diff` attribute for a path (`*.c diff=cpp` and friends in
//! `.gitattributes`) and uses that driver's `xfuncname` pattern instead of its
//! generic fallback. Without this, every hunk header in a repository that sets
//! the attribute is wrong — on the Linux kernel that was 86% of all observed
//! divergences from `git`.
//!
//! The matching rules reproduced here are xdiff's `ff_regexp` and `def_ff`.

use crate::userdiff_table::BUILTIN_DRIVERS;
use regex::bytes::{Regex, RegexBuilder};
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

const MAX_GENERATED_PATH_ENTRIES: usize = 4096;

static GENERATED_PATHS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Records whether `rela_path` was marked `linguist-generated` by `.gitattributes`.
pub fn mark_path_linguist_generated(rela_path: &str, generated: bool) {
    if let Ok(mut guard) = GENERATED_PATHS.lock() {
        if generated {
            if guard.len() >= MAX_GENERATED_PATH_ENTRIES && !guard.contains(rela_path) {
                guard.clear();
            }
            guard.insert(rela_path.to_string());
        } else {
            guard.remove(rela_path);
        }
    }
}

/// Returns `true` if `rela_path` was marked `linguist-generated` by `.gitattributes`.
#[must_use]
pub fn is_path_marked_linguist_generated(rela_path: &str) -> bool {
    GENERATED_PATHS
        .lock()
        .is_ok_and(|guard| guard.contains(rela_path))
}

/// Maximum bytes of function context Git copies into a hunk header.
///
/// This is the size of xdiff's stack buffer in `xdl_emit_hunk_hdr`.
pub const FUNC_CONTEXT_MAX: usize = 80;

/// One alternative of a driver's funcname pattern.
///
/// Git stores the alternatives newline-separated in a single string and tries
/// them in order; an alternative prefixed with `!` rejects the line outright
/// rather than accepting it.
struct Alternative {
    negate: bool,
    regex: Regex,
}

/// Selects the line that becomes a hunk's function context.
pub struct FuncMatcher {
    kind: Kind,
}

/// How a matcher decides, kept private so the compiled regexes are an
/// implementation detail.
enum Kind {
    /// xdiff's `def_ff`: any line starting with an identifier character.
    Default,
    /// A driver's `xfuncname` alternatives, in Git's order.
    Patterns(Vec<Alternative>),
}

impl FuncMatcher {
    /// The generic matcher Git uses when no `diff` driver applies.
    pub fn default_matcher() -> Self {
        Self {
            kind: Kind::Default,
        }
    }

    /// Looks up a builtin driver by the value of the `diff` attribute.
    ///
    /// Returns `None` for names Git does not know, which is also what Git does:
    /// an unknown driver falls back to the default matcher.
    pub fn builtin(name: &str) -> Option<Self> {
        let driver = BUILTIN_DRIVERS.iter().find(|d| d.name == name)?;
        Self::from_patterns(driver.funcname, driver.case_insensitive)
    }

    /// Compiles an explicit list of alternatives, e.g. from `diff.<name>.xfuncname`.
    ///
    /// Returns `None` if any alternative fails to compile. Git's patterns are
    /// POSIX extended regular expressions; the few constructs Rust's engine
    /// spells differently are translated by `translate_pattern`. Failing the
    /// whole driver rather than silently dropping one alternative keeps the
    /// fallback behaviour predictable: a bad pattern means the default matcher,
    /// not a subtly wrong one.
    pub fn from_patterns(patterns: &[&str], case_insensitive: bool) -> Option<Self> {
        let mut out = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let (negate, body) = match pattern.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, *pattern),
            };
            let regex = RegexBuilder::new(&translate_pattern(body))
                .case_insensitive(case_insensitive)
                .build()
                .ok()?;
            out.push(Alternative { negate, regex });
        }
        (!out.is_empty()).then_some(FuncMatcher {
            kind: Kind::Patterns(out),
        })
    }

    /// Returns the function context for `line`, or `None` if it is not a
    /// function line.
    ///
    /// `line` must already have its newline removed; a trailing `\r` is
    /// stripped here because xdiff excludes it from the match.
    pub fn find(&self, line: &[u8]) -> Option<String> {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let matched = match &self.kind {
            Kind::Default => {
                // xdiff's `def_ff` takes the whole line when it starts with an
                // identifier character.
                let first = *line.first()?;
                if !(first.is_ascii_alphabetic() || first == b'_' || first == b'$') {
                    return None;
                }
                line
            }
            Kind::Patterns(alternatives) => {
                let captures = alternatives.iter().find_map(|alternative| {
                    alternative
                        .regex
                        .captures(line)
                        .map(|caps| (alternative.negate, caps))
                });
                let (negate, caps) = captures?;
                if negate {
                    return None;
                }
                // Git prefers the first capture group when the pattern has one,
                // so a driver can match on context while reporting only part of
                // the line. Otherwise the whole match is used — note that this
                // is the *match*, not the line.
                let m = caps.get(1).or_else(|| caps.get(0))?;
                &line[m.start()..m.end()]
            }
        };

        // Truncate first, strip trailing whitespace second. The reverse order
        // leaves a stray space on signatures that were cut mid-argument.
        let mut capped = &matched[..matched.len().min(FUNC_CONTEXT_MAX)];
        if let Err(err) = std::str::from_utf8(capped)
            && err.error_len().is_none()
        {
            capped = &capped[..err.valid_up_to()];
        }
        let mut end = capped.len();
        while end > 0 && capped[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        Some(String::from_utf8_lossy(&capped[..end]).to_string())
    }
}

/// Rewrites the POSIX/GNU constructs Git's patterns use into the equivalents
/// Rust's regex engine understands.
///
/// Two differences matter in practice:
///
/// * `\<` and `\>` are GNU word boundaries, which Rust spells `\b`.
/// * In a POSIX bracket expression a `]` in first position is a literal
///   (`[][:alnum:]]` is "a right bracket or an alphanumeric"), while Rust
///   reads it as an empty class and refuses to compile. Git's `csharp`, `java`
///   and `objc` drivers all rely on this, so the bracket is escaped instead.
///
/// Everything else — alternation, repetition, POSIX classes like
/// `[[:alpha:]]` — is already common to both engines.
fn translate_pattern(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('\\');
                match chars.next() {
                    // GNU word boundaries, outside classes only: inside a
                    // bracket expression POSIX treats a backslash literally.
                    Some('<' | '>') if !in_class => {
                        out.pop();
                        out.push_str(r"\b");
                    }
                    Some(next) => out.push(next),
                    None => {}
                }
            }
            '[' if !in_class => {
                in_class = true;
                out.push('[');
                if chars.peek() == Some(&'^') {
                    chars.next();
                    out.push('^');
                }
                // A `]` here is a literal member of the class, not its end.
                if chars.peek() == Some(&']') {
                    chars.next();
                    out.push_str(r"\]");
                }
            }
            '[' if in_class => {
                if chars.peek() == Some(&':') {
                    // A POSIX class like `[:alnum:]` — copy through to its close.
                    out.push('[');
                    for inner in chars.by_ref() {
                        out.push(inner);
                        if inner == ']' {
                            break;
                        }
                    }
                } else {
                    // Rust reads a nested `[` as a class union; POSIX reads a
                    // literal bracket.
                    out.push_str(r"\[");
                }
            }
            // Rust gives doubled `&` and `~` set-operator meanings inside a
            // class that POSIX does not have.
            '&' | '~' if in_class => {
                out.push('\\');
                out.push(c);
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            other => out.push(other),
        }
    }
    out
}

/// Resolves each path's `diff` attribute to the matcher Git would use.
///
/// One instance serves a whole commit diff: the attribute stack and the
/// compiled regexes are both expensive enough that rebuilding them per file
/// would dominate the diff itself.
pub struct DriverResolver<'repo> {
    repo: &'repo gix::Repository,
    /// `None` when the repository has no usable attribute source, in which case
    /// every path gets the default matcher — which is what Git does too.
    stack: Option<gix::AttributeStack<'repo>>,
    outcome: gix::attrs::search::Outcome,
    /// Compiled matchers by driver name. `None` records a name that has no
    /// usable pattern, so it is not looked up again.
    by_name: std::collections::HashMap<String, Option<std::rc::Rc<FuncMatcher>>>,
    default: std::rc::Rc<FuncMatcher>,
}

impl<'repo> DriverResolver<'repo> {
    /// Builds a resolver for `repo`.
    ///
    /// Attributes come from the worktree when there is one, falling back to the
    /// index, mirroring what `git diff-tree` reads by default.
    pub fn new(repo: &'repo gix::Repository) -> Self {
        use gix::worktree::stack::state::attributes::Source;

        let stack = if repo.workdir().is_some() {
            let empty_index = gix::index::State::new(repo.object_hash());
            repo.attributes_only(&empty_index, Source::WorktreeThenIdMapping)
                .ok()
        } else {
            repo.index_or_empty()
                .ok()
                .and_then(|index| repo.attributes_only(&index, Source::IdMapping).ok())
        };

        let mut outcome = gix::attrs::search::Outcome::default();
        outcome.initialize_with_selection(
            &gix::attrs::search::MetadataCollection::default(),
            ["diff", "linguist-generated"],
        );

        Self {
            repo,
            stack,
            outcome,
            by_name: std::collections::HashMap::new(),
            default: std::rc::Rc::new(FuncMatcher::default_matcher()),
        }
    }

    /// Returns the matcher for `rela_path` and records its `linguist-generated` `.gitattributes` state.
    pub fn matcher_for(&mut self, rela_path: &str) -> std::rc::Rc<FuncMatcher> {
        let is_gen = self.is_linguist_generated(rela_path);
        mark_path_linguist_generated(rela_path, is_gen);
        let Some(name) = self.driver_name(rela_path) else {
            return self.default.clone();
        };
        if let Some(cached) = self.by_name.get(&name) {
            return cached.clone().unwrap_or_else(|| self.default.clone());
        }
        let matcher = self.compile(&name).map(std::rc::Rc::new);
        self.by_name.insert(name, matcher.clone());
        matcher.unwrap_or_else(|| self.default.clone())
    }

    /// Returns `true` when `rela_path` has the `linguist-generated` attribute set in `.gitattributes`.
    pub fn is_linguist_generated(&mut self, rela_path: &str) -> bool {
        let Self { stack, outcome, .. } = self;
        let Some(stack) = stack.as_mut() else {
            return false;
        };
        let Ok(platform) = stack.at_entry(rela_path, Some(gix::index::entry::Mode::FILE)) else {
            return false;
        };
        outcome.reset();
        platform.matching_attributes(outcome);
        for attr in outcome.iter_selected() {
            if attr.assignment.name.as_str() == "linguist-generated" {
                use gix::attrs::StateRef;
                use gix::bstr::ByteSlice;
                return match attr.assignment.state {
                    StateRef::Set => true,
                    StateRef::Value(v) => {
                        let s = v.as_bstr().to_str_lossy();
                        s.eq_ignore_ascii_case("true") || s == "1"
                    }
                    StateRef::Unset | StateRef::Unspecified => false,
                };
            }
        }
        false
    }

    /// Reads the value of the `diff` attribute for `rela_path`, if it has one.
    ///
    /// A bare `diff` or `-diff` yields no name: the first means "text, default
    /// driver" and the second is handled upstream by `gix`, which reports such
    /// paths as binary.
    fn driver_name(&mut self, rela_path: &str) -> Option<String> {
        // Destructured so the attribute stack and the search outcome are
        // borrowed as disjoint fields rather than through `self`.
        let Self { stack, outcome, .. } = self;
        let stack = stack.as_mut()?;
        let platform = stack
            .at_entry(rela_path, Some(gix::index::entry::Mode::FILE))
            .ok()?;
        outcome.reset();
        platform.matching_attributes(outcome);
        let attr = outcome
            .iter_selected()
            .find(|a| a.assignment.name.as_str() == "diff")?;
        attr.assignment.state.as_bstr().map(ToString::to_string)
    }

    /// Compiles the driver called `name`.
    ///
    /// A user-configured `diff.<name>.xfuncname` wins over a builtin of the
    /// same name, matching Git's precedence.
    ///
    /// Both `name` (chosen by a `.gitattributes` entry) and the pattern (read
    /// from the repository's own configuration) are attacker-controlled in a
    /// hostile repository, and every newline in the value costs one additional
    /// regex compilation. The two bounds below keep that work proportionate;
    /// an oversized value is simply ignored, which falls back to the builtin or
    /// default matcher. Git itself compiles `xfuncname` as a single regex, so
    /// no realistic configuration comes close to these limits.
    fn compile(&self, name: &str) -> Option<FuncMatcher> {
        /// Largest repository-supplied pattern value that is compiled at all.
        const MAX_PATTERN_BYTES: usize = 8 * 1024;
        /// Largest number of newline-separated alternatives that is compiled.
        const MAX_ALTERNATIVES: usize = 32;

        let configured = self
            .repo
            .config_snapshot()
            .string_by("diff", Some(name.into()), "xfuncname")
            .map(|value| value.to_string())
            .filter(|value| value.len() <= MAX_PATTERN_BYTES);
        if let Some(pattern) = configured {
            let alternatives: Vec<&str> = pattern.split('\n').collect();
            if alternatives.len() <= MAX_ALTERNATIVES
                && let Some(matcher) = FuncMatcher::from_patterns(&alternatives, false)
            {
                return Some(matcher);
            }
        }
        FuncMatcher::builtin(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matcher_accepts_identifier_starts() {
        let m = FuncMatcher::default_matcher();
        assert_eq!(m.find(b"int main(void)").as_deref(), Some("int main(void)"));
        assert_eq!(m.find(b"_start:").as_deref(), Some("_start:"));
        assert_eq!(m.find(b"$var = 1").as_deref(), Some("$var = 1"));
        assert_eq!(m.find(b"\tindented()"), None);
        assert_eq!(m.find(b"}"), None);
        assert_eq!(m.find(b""), None);
    }

    #[test]
    fn trailing_cr_is_excluded_from_the_context() {
        let m = FuncMatcher::default_matcher();
        assert_eq!(
            m.find(b"int main(void)\r").as_deref(),
            Some("int main(void)")
        );
    }

    #[test]
    fn truncation_happens_before_whitespace_is_stripped() {
        let m = FuncMatcher::default_matcher();
        // 78 identifier bytes, then a space, then more: the cut at 80 exposes a
        // trailing space that must then be removed.
        let line = format!("{}{}", "a".repeat(79), " tail");
        let got = m.find(line.as_bytes()).unwrap();
        assert_eq!(got.len(), 79, "expected the exposed space to be stripped");
        assert!(!got.ends_with(' '));
    }

    #[test]
    fn negative_alternatives_reject_the_line() {
        // Mirrors the shape of Git's drivers: reject labels, accept the rest.
        let m = FuncMatcher::from_patterns(&["!^[ \t]*case", "^[A-Za-z].*"], false).unwrap();
        assert_eq!(m.find(b"\tcase 1:"), None);
        assert_eq!(m.find(b"int main(void)").as_deref(), Some("int main(void)"));
    }

    #[test]
    fn first_capture_group_wins_over_the_whole_match() {
        let m = FuncMatcher::from_patterns(&["^prefix: (.*)$"], false).unwrap();
        assert_eq!(m.find(b"prefix: body").as_deref(), Some("body"));
    }

    #[test]
    fn case_insensitive_drivers_match_either_case() {
        let m = FuncMatcher::from_patterns(&["^(function.*)"], true).unwrap();
        assert_eq!(m.find(b"FUNCTION Foo").as_deref(), Some("FUNCTION Foo"));
    }

    #[test]
    fn gnu_word_boundaries_are_translated() {
        assert_eq!(translate_pattern(r"\<word\>"), r"\bword\b");
        assert_eq!(translate_pattern(r"\[literal\]"), r"\[literal\]");
        let m = FuncMatcher::from_patterns(&[r"^.*\<fn\>.*$"], false).unwrap();
        assert_eq!(m.find(b"pub fn foo()").as_deref(), Some("pub fn foo()"));
    }

    #[test]
    fn posix_leading_bracket_in_a_class_is_a_literal() {
        // Straight from git's java driver; Rust's engine rejects this as-is.
        let translated = translate_pattern(r"[][[:alnum:]@_.]");
        assert_eq!(translated, r"[\]\[[:alnum:]@_.]");
        let re = Regex::new(&translated).expect("translated pattern compiles");
        assert!(re.is_match(b"]"), "a literal ] is a member of the class");
        assert!(re.is_match(b"["), "so is a literal [");
        assert!(re.is_match(b"x"), "and alphanumerics");
    }

    #[test]
    fn every_builtin_driver_compiles() {
        for driver in crate::userdiff_table::BUILTIN_DRIVERS {
            assert!(
                FuncMatcher::builtin(driver.name).is_some(),
                "driver {} failed to compile",
                driver.name
            );
        }
    }

    #[test]
    fn rust_driver_matches_impl_blocks_not_inner_functions() {
        let m = FuncMatcher::builtin("rust").expect("rust is a builtin driver");
        assert_eq!(m.find(b"impl Thread {").as_deref(), Some("impl Thread {"));
        assert_eq!(m.find(b"pub fn foo() {").as_deref(), Some("pub fn foo() {"));
        assert_eq!(m.find(b"    let x = 1;"), None);
    }

    #[test]
    fn cpp_driver_rejects_labels() {
        let m = FuncMatcher::builtin("cpp").expect("cpp is a builtin driver");
        assert_eq!(
            m.find(b"static void foo(int x)").as_deref(),
            Some("static void foo(int x)")
        );
        assert_eq!(m.find(b"out:"), None, "labels are not function headers");
    }

    #[test]
    fn python_driver_matches_def_and_class() {
        let m = FuncMatcher::builtin("python").expect("python is a builtin driver");
        assert_eq!(m.find(b"def main():").as_deref(), Some("def main():"));
        assert_eq!(m.find(b"class Foo:").as_deref(), Some("class Foo:"));
        assert_eq!(m.find(b"x = 1"), None);
    }

    #[test]
    fn unknown_driver_names_have_no_matcher() {
        assert!(FuncMatcher::builtin("definitely-not-a-driver").is_none());
    }

    #[test]
    fn oversized_repository_xfuncname_is_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        gix::init(root).expect("init repo");
        std::fs::write(root.join(".gitattributes"), "* diff=hostile\n").expect("write attributes");
        std::fs::write(root.join("file.txt"), "int main(void)\n").expect("write file");

        // 33 alternatives (one over the cap), none of which match an ordinary C
        // function header. If the cap were missing, `find` below would return
        // `None` because the custom driver would win over the default matcher.
        let mut config = String::from("[diff \"hostile\"]\n\txfuncname = \"");
        for _ in 0..33 {
            config.push_str("^ZZZNOPE\\n");
        }
        config.push_str("\"\n");
        let config_path = root.join(".git").join("config");
        let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
        std::fs::write(&config_path, format!("{existing}{config}")).expect("write config");

        let repo = gix::open(root).expect("open repo");
        let mut resolver = DriverResolver::new(&repo);
        let matcher = resolver.matcher_for("file.txt");
        assert_eq!(
            matcher.find(b"int main(void)").as_deref(),
            Some("int main(void)"),
            "an oversized xfuncname must fall back to the default matcher"
        );
    }
}
