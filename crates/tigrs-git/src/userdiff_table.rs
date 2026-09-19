// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Clean-room builtin language function-header patterns for `.gitattributes` `diff=<driver>`.
//!
//! When a repository associates a path with a builtin driver name (for example
//! `*.rs diff=rust` or `*.py diff=python`), [`crate::userdiff::FuncMatcher`] uses
//! the patterns below to extract the enclosing declaration or section header for
//! `@@ ... @@ <context>` hunk headers.
//!
//! Each entry in `funcname` is evaluated in order:
//! - A leading `!` marks a negative rejection rule (for example, rejecting C/C++
//!   `goto` labels or control-flow keywords before accepting top-level signatures).
//! - Positive rules use capture group 1 (if present) or the full match (`$0`) as
//!   the hunk header context string.

/// A builtin language function-header driver selected by `.gitattributes` `diff=<name>`.
pub struct BuiltinDriver {
    /// Driver identifier matching the `diff=<name>` gitattribute value.
    pub name: &'static str,
    /// Whether the regular expression patterns should be matched case-insensitively.
    pub case_insensitive: bool,
    /// Ordered list of regular expression rules; entries starting with `!` reject a line.
    pub funcname: &'static [&'static str],
}

/// Clean-room builtin language drivers, sorted alphabetically by `name`.
pub static BUILTIN_DRIVERS: &[BuiltinDriver] = &[
    BuiltinDriver {
        name: "ada",
        case_insensitive: true,
        funcname: &[
            r"!^\s*with\s+",
            r"!^.*\b(is\s+new|renames|is\s+separate)\b",
            r"^\s*((procedure|function|package|protected|task)\s+.*)$",
        ],
    },
    BuiltinDriver {
        name: "bash",
        case_insensitive: false,
        funcname: &[
            r"^\s*((function\s+[A-Za-z_][A-Za-z0-9_]*(\s*\(\s*\))?|[A-Za-z_][A-Za-z0-9_]*\s*\(\s*\)).*)$",
        ],
    },
    BuiltinDriver {
        name: "bibtex",
        case_insensitive: false,
        funcname: &[r"^(@[A-Za-z]+\s*\{?\s*[^,\s\}]+).*$"],
    },
    BuiltinDriver {
        name: "cpp",
        case_insensitive: false,
        funcname: &[
            r"!^\s*[A-Za-z_][A-Za-z0-9_]*\s*:\s*($|//|/\*)",
            r"^((::\s*)?[A-Za-z_~].*)$",
        ],
    },
    BuiltinDriver {
        name: "csharp",
        case_insensitive: false,
        funcname: &[
            r"!^\s*(if|else|for|foreach|while|do|switch|case|default|return|throw|catch|using|lock|fixed|new)\b",
            r"^\s*(((public|private|protected|internal|static|abstract|sealed|virtual|override|async|unsafe|partial|new)\s+)*(class|struct|interface|enum|record|namespace)\s+.*)$",
            r"^\s*([A-Za-z_][A-Za-z0-9_<>,.\[\]\?\s]*\s+[A-Za-z_][A-Za-z0-9_.]*\s*\([^;]*)$",
        ],
    },
    BuiltinDriver {
        name: "css",
        case_insensitive: true,
        funcname: &[r"![:;]\s*$", r"^([@#.:_A-Za-z0-9\-\[\]].*)$"],
    },
    BuiltinDriver {
        name: "dts",
        case_insensitive: false,
        funcname: &[
            r"!;",
            r"!=",
            r"^\s*((\/\s*\{|&?[A-Za-z_][A-Za-z0-9_,@\-]*).*)$",
        ],
    },
    BuiltinDriver {
        name: "elixir",
        case_insensitive: false,
        funcname: &[r"^\s*((def(p|macro|macrop|module|protocol|impl)?|test)\s+.*)$"],
    },
    BuiltinDriver {
        name: "fortran",
        case_insensitive: true,
        funcname: &[
            r"!^([Cc*!]|\s*!)",
            r"!^\s*module\s+procedure\b",
            r"^\s*((end\s+)?(program|module|subroutine|function|block\s+data)\s+[A-Za-z_].*)$",
        ],
    },
    BuiltinDriver {
        name: "fountain",
        case_insensitive: true,
        funcname: &[r"^((\.[^.]|(int|ext|est|int\.?/ext|i/e)[\.\s]).*)$"],
    },
    BuiltinDriver {
        name: "golang",
        case_insensitive: false,
        funcname: &[
            r"^\s*(func\s+.*)$",
            r"^\s*(type\s+[A-Za-z_][A-Za-z0-9_]*\s+(struct|interface)\b.*)$",
        ],
    },
    BuiltinDriver {
        name: "html",
        case_insensitive: false,
        funcname: &[r"^\s*(<[Hh][1-6](\s[^>]*)?>.*)$"],
    },
    BuiltinDriver {
        name: "ini",
        case_insensitive: false,
        funcname: &[r"^\s*(\[[^\]]+\])"],
    },
    BuiltinDriver {
        name: "java",
        case_insensitive: false,
        funcname: &[
            r"!^\s*(if|else|for|while|do|switch|case|default|return|throw|catch|new|instanceof)\b",
            r"^\s*(((public|protected|private|static|final|abstract|synchronized|native|strictfp|sealed|non-sealed)\s+)*(class|interface|enum|record)\s+.*)$",
            r"^\s*([A-Za-z_<>\[\].,\?\s]+\s+[A-Za-z_][A-Za-z0-9_]*\s*\([^;]*)$",
        ],
    },
    BuiltinDriver {
        name: "kotlin",
        case_insensitive: false,
        funcname: &[
            r"^\s*(((public|private|protected|internal|open|abstract|final|sealed|data|inline|suspend|tailrec|operator|infix|external|override)\s+)*(fun|class|interface|object)\s+.*)$",
        ],
    },
    BuiltinDriver {
        name: "markdown",
        case_insensitive: false,
        funcname: &[r"^ {0,3}(#{1,6}\s+.*)$"],
    },
    BuiltinDriver {
        name: "matlab",
        case_insensitive: false,
        funcname: &[r"^\s*((function|classdef)\s+.*|(%%|#)\s+.*)$"],
    },
    BuiltinDriver {
        name: "objc",
        case_insensitive: false,
        funcname: &[
            r"!^\s*(if|else|for|while|do|switch|return)\b",
            r"^(\s*[-+]\s*\([^)]+\)\s*[A-Za-z_].*)$",
            r"^(@(interface|implementation|protocol)\s+.*)$",
            r"^\s*([A-Za-z_][A-Za-z0-9_\s\*]+\s+[A-Za-z_][A-Za-z0-9_]*\s*\([^;]*)$",
        ],
    },
    BuiltinDriver {
        name: "pascal",
        case_insensitive: true,
        funcname: &[
            r"^\s*(((class\s+)?(procedure|function)|constructor|destructor|interface|implementation|initialization|finalization)\b.*)$",
            r"^.*=\s*(class|record)\b.*$",
        ],
    },
    BuiltinDriver {
        name: "perl",
        case_insensitive: false,
        funcname: &[
            r"^(package\s+.*)$",
            r"^\s*(sub\s+[A-Za-z0-9_':]+.*)$",
            r"^(BEGIN|END|INIT|CHECK|UNITCHECK|AUTOLOAD|DESTROY)\b.*$",
            r"^(=head[0-9]\s+.*)$",
        ],
    },
    BuiltinDriver {
        name: "php",
        case_insensitive: false,
        funcname: &[
            r"^\s*(((public|protected|private|static|abstract|final)\s+)*function\s+.*)$",
            r"^\s*(((final|abstract|readonly)\s+)*(class|interface|trait|enum)\s+.*)$",
        ],
    },
    BuiltinDriver {
        name: "python",
        case_insensitive: false,
        funcname: &[r"^\s*((class|(async\s+)?def)\s+.*)$"],
    },
    BuiltinDriver {
        name: "r",
        case_insensitive: false,
        funcname: &[r"^\s*([A-Za-z.][A-Za-z0-9_.]*\s*(<-|=)\s*function\b.*)$"],
    },
    BuiltinDriver {
        name: "ruby",
        case_insensitive: false,
        funcname: &[r"^\s*((class|module|def)\s+.*)$"],
    },
    BuiltinDriver {
        name: "rust",
        case_insensitive: false,
        funcname: &[
            r#"^\s*((pub(\([^)]*\))?\s+)?((async|const|unsafe|extern(\s+"[^"]+")?)\s+)*(struct|enum|union|mod|trait|fn|impl|macro_rules!)\b[^;]*)$"#,
        ],
    },
    BuiltinDriver {
        name: "scheme",
        case_insensitive: false,
        funcname: &[r"^\s*(\((define|def[A-Za-z0-9_-]*|library|module|struct|class)\b.*)$"],
    },
    BuiltinDriver {
        name: "swift",
        case_insensitive: false,
        funcname: &[
            r"^\s*((@[A-Za-z_][A-Za-z0-9_]*(\([^)]*\))?\s+)*([a-z]+\s+)*(func|init|deinit|subscript|class|struct|enum|protocol|extension|actor)\b.*)$",
        ],
    },
    BuiltinDriver {
        name: "tex",
        case_insensitive: false,
        funcname: &[r"^(\\(part|chapter|(sub)*section)\*?\{.*)$"],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::userdiff::FuncMatcher;
    use std::collections::HashSet;

    #[test]
    fn test_all_builtin_drivers_are_valid_and_compile() {
        assert_eq!(
            BUILTIN_DRIVERS.len(),
            28,
            "Expected 28 builtin language drivers"
        );

        let mut seen = HashSet::new();
        for driver in BUILTIN_DRIVERS {
            assert!(!driver.name.is_empty(), "Driver name must not be empty");
            assert!(
                seen.insert(driver.name),
                "Duplicate driver name found: {}",
                driver.name
            );
            assert!(
                !driver.funcname.is_empty(),
                "Driver {} has no funcname patterns",
                driver.name
            );

            // Verify that each driver compiles cleanly via FuncMatcher
            let matcher = FuncMatcher::builtin(driver.name);
            assert!(
                matcher.is_some(),
                "Failed to compile regex patterns for driver '{}'",
                driver.name
            );
        }
    }

    #[test]
    fn test_core_language_drivers_present() {
        let expected = [
            "ada", "bash", "bibtex", "cpp", "csharp", "css", "dts", "elixir", "fortran",
            "fountain", "golang", "html", "ini", "java", "kotlin", "markdown", "matlab", "objc",
            "pascal", "perl", "php", "python", "r", "ruby", "rust", "scheme", "swift", "tex",
        ];
        assert_eq!(expected.len(), 28);
        for lang in &expected {
            assert!(
                BUILTIN_DRIVERS.iter().any(|d| d.name == *lang),
                "Missing expected driver: {lang}"
            );
        }
    }

    #[test]
    fn test_additional_language_driver_matching() {
        // Golang driver matches func and type
        let go_matcher = FuncMatcher::builtin("golang").expect("golang driver");
        assert!(
            go_matcher
                .find(b"func (s *Server) ServeHTTP(w ResponseWriter, r *Request) {")
                .is_some()
        );
        assert!(go_matcher.find(b"type Config struct {").is_some());
        assert_eq!(go_matcher.find(b"    x := 1"), None);

        // Markdown driver matches ATX headings (# ...)
        let md_matcher = FuncMatcher::builtin("markdown").expect("markdown driver");
        assert_eq!(
            md_matcher.find(b"# Overview").as_deref(),
            Some("# Overview")
        );
        assert_eq!(
            md_matcher.find(b"## Section 2").as_deref(),
            Some("## Section 2")
        );
        assert_eq!(md_matcher.find(b"plain paragraph line"), None);

        // HTML driver matches heading tags
        let html_matcher = FuncMatcher::builtin("html").expect("html driver");
        assert!(
            html_matcher
                .find(b"<h1 class=\"title\">Header</h1>")
                .is_some()
        );
        assert!(html_matcher.find(b"<div class=\"container\">").is_none());

        // Bash driver matches function definitions
        let bash_matcher = FuncMatcher::builtin("bash").expect("bash driver");
        assert!(bash_matcher.find(b"function build_all() {").is_some());
        assert!(bash_matcher.find(b"cleanup() {").is_some());
        assert_eq!(bash_matcher.find(b"echo hello"), None);
    }
}
