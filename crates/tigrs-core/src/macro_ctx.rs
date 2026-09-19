// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Macro variable context and command template expansion for tigrs.
//!
//! Implements strict shell-quoted expansion of Tig's canonical macro tokens
//! (`%(commit)`, `%(branch)`, `%(file)`, `%(prompt)`, etc.) to prevent command injection
//! vulnerabilities when interpolating repository-controlled strings into shell pipelines.

use crate::error::{Result, TigError};
use crate::quote::{MacroLookup, QuoteState, append_quoted_in_context, find_matching_close_paren};
use std::borrow::Cow;

/// Contextual variables available for macro expansion during command execution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MacroContext {
    /// SHA hash of the selected commit (e.g. in Main or Diff view).
    pub commit: Option<String>,
    /// SHA hash or ref name of the current HEAD.
    pub head: Option<String>,
    /// Object ID of the selected or viewed blob.
    pub blob: Option<String>,
    /// Current local branch name.
    pub branch: Option<String>,
    /// Remote name or upstream tracking branch.
    pub remote: Option<String>,
    /// Tag associated with the current commit.
    pub tag: Option<String>,
    /// Selected refname in refs view or branch list.
    pub refname: Option<String>,
    /// Stash identifier (e.g. `stash@{0}`).
    pub stash: Option<String>,
    /// Current relative directory path in tree view.
    pub directory: Option<String>,
    /// Selected file path in diff, status, tree, blob, or blame view.
    pub file: Option<String>,
    /// Old file path before rename/copy.
    pub file_old: Option<String>,
    /// Selected line number (1-based) in diff, blob, or blame view.
    pub lineno: Option<usize>,
    /// Old line number (1-based) in diff view.
    pub lineno_old: Option<usize>,
    /// Selected line text.
    pub text: Option<String>,
    /// Path to the repository `.git` directory (`%(repo:git-dir)`).
    pub repo_git_dir: Option<String>,
    /// Path to the working tree root (`%(repo:worktree)`).
    pub repo_worktree: Option<String>,
    /// Current prefix path relative to repository root (`%(repo:prefix)`).
    pub repo_prefix: Option<String>,
    /// Relative path to repository root (`%(repo:cdup)`).
    pub repo_cdup: Option<String>,
    /// Repository HEAD reference (`%(repo:head)`).
    pub repo_head: Option<String>,
    /// Repository default remote (`%(repo:remote)`).
    pub repo_remote: Option<String>,
}

impl MacroLookup for MacroContext {
    fn lookup(&self, key: &str) -> Option<Cow<'_, str>> {
        self.get_var_cow(key)
    }
}

impl MacroContext {
    /// Creates a new empty [`MacroContext`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Retrieves a variable by its canonical macro key name (without `%(...)`).
    pub fn get_var(&self, key: &str) -> Option<String> {
        self.get_var_cow(key).map(Cow::into_owned)
    }

    /// Retrieves a variable by its canonical macro key name as a [`Cow<str>`], avoiding clones.
    pub fn get_var_cow(&self, key: &str) -> Option<Cow<'_, str>> {
        fn safe_rel_path(s: &str) -> Cow<'_, str> {
            let cleaned = crate::ansi::strip_control_chars(s);
            if cleaned.starts_with('-') || cleaned.starts_with('+') {
                Cow::Owned(format!("./{cleaned}"))
            } else {
                cleaned
            }
        }
        fn safe_ref_token(s: &str) -> Cow<'_, str> {
            let cleaned = crate::ansi::strip_control_chars(s);
            if cleaned.starts_with('-') || cleaned.starts_with('+') {
                Cow::Owned(cleaned.trim_start_matches(['-', '+']).to_string())
            } else {
                cleaned
            }
        }
        match key {
            "commit" => self.commit.as_deref().map(Cow::Borrowed),
            "head" => self.head.as_deref().map(safe_ref_token),
            "blob" => self.blob.as_deref().map(Cow::Borrowed),
            "branch" => self.branch.as_deref().map(safe_ref_token),
            "remote" => self.remote.as_deref().map(safe_ref_token),
            "tag" => self.tag.as_deref().map(safe_ref_token),
            "refname" => self.refname.as_deref().map(safe_ref_token),
            "stash" => self.stash.as_deref().map(safe_ref_token),
            "directory" => Some(
                self.directory
                    .as_deref()
                    .map_or(Cow::Borrowed("."), safe_rel_path),
            ),
            "file" => self.file.as_deref().map(safe_rel_path),
            "file_old" => self.file_old.as_deref().map(safe_rel_path),
            "lineno" => self.lineno.map(|n| {
                let mut buf = itoa::Buffer::new();
                Cow::Owned(buf.format(n).to_owned())
            }),
            "lineno_old" => self.lineno_old.map(|n| {
                let mut buf = itoa::Buffer::new();
                Cow::Owned(buf.format(n).to_owned())
            }),
            "text" => self.text.as_deref().map(Cow::Borrowed),
            "repo:git-dir" => self.repo_git_dir.as_deref().map(Cow::Borrowed),
            "repo:worktree" => self.repo_worktree.as_deref().map(Cow::Borrowed),
            "repo:prefix" => self.repo_prefix.as_deref().map(safe_rel_path),
            "repo:cdup" => self.repo_cdup.as_deref().map(Cow::Borrowed),
            "repo:head" => self.repo_head.as_deref().map(safe_ref_token),
            "repo:remote" => self.repo_remote.as_deref().map(safe_ref_token),
            _ => None,
        }
    }

    /// Internal helper that yields the inner key slice of each well-formed `%(key)` token in `template`.
    pub fn for_each_macro_token<'a, F>(template: &'a str, mut f: F)
    where
        F: FnMut(&'a str),
    {
        let mut rest = template;
        while let Some(start_idx) = rest.find("%(") {
            let after_start = &rest[start_idx + 2..];
            if let Some(end_idx) = find_matching_close_paren(after_start) {
                f(&after_start[..end_idx]);
                rest = &after_start[end_idx + 1..];
            } else {
                break;
            }
        }
    }

    /// Checks if a command template contains any `%(prompt)` or `%(prompt ...)` tokens.
    pub fn has_prompt_token(template: &str) -> bool {
        let mut found = false;
        Self::for_each_macro_token(template, |key| {
            if key == "prompt" || key.starts_with("prompt ") {
                found = true;
            }
        });
        found
    }

    /// Extracts all prompt messages required by a command template.
    pub fn extract_prompt_labels(template: &str) -> Vec<String> {
        let mut labels = Vec::new();
        Self::for_each_macro_token(template, |key| {
            if key == "prompt" {
                labels.push(String::new());
            } else if let Some(msg) = key.strip_prefix("prompt ") {
                labels.push(msg.trim().to_string());
            }
        });
        labels
    }

    /// Expands a command template string, substituting all `%(...)` tokens.
    ///
    /// Every substituted value is safely quoted using [`crate::quote::shell_quote`] to completely
    /// neutralize command injection risks.
    ///
    /// If an interactive prompt token `%(prompt)` or `%(prompt <msg>)` is encountered,
    /// `prompt_resolver` is invoked with the optional prompt message. If `prompt_resolver`
    /// returns `None`, expansion aborts immediately with a cancellation error.
    ///
    /// # Errors
    ///
    /// Returns [`TigError::Command`] if an unknown macro token is encountered, or if the
    /// user cancels an interactive prompt.
    pub fn expand<F>(&self, template: &str, mut prompt_resolver: F) -> Result<String>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let mut result = String::with_capacity(template.len() + 64);
        let mut rest = template;
        let mut quote_state = QuoteState::Unquoted;

        while let Some(start_idx) = rest.find("%(") {
            let prefix = &rest[..start_idx];
            quote_state.advance(prefix);
            result.push_str(prefix);
            let after_start = &rest[start_idx + 2..];

            if let Some(end_idx) = find_matching_close_paren(after_start) {
                let key = &after_start[..end_idx];

                if key == "prompt" {
                    let prompt_val = prompt_resolver("").ok_or_else(|| {
                        TigError::Command("Command cancelled by user".to_string())
                    })?;
                    append_quoted_in_context(&mut result, &prompt_val, quote_state);
                } else if let Some(prompt_msg) = key.strip_prefix("prompt ") {
                    let prompt_val = prompt_resolver(prompt_msg.trim()).ok_or_else(|| {
                        TigError::Command("Command cancelled by user".to_string())
                    })?;
                    append_quoted_in_context(&mut result, &prompt_val, quote_state);
                } else if let Some(val) = self.get_var_cow(key) {
                    append_quoted_in_context(&mut result, &val, quote_state);
                } else {
                    return Err(TigError::Command(format!(
                        "Unknown or unavailable macro '%({key})' in command: '{template}'"
                    )));
                }

                rest = &after_start[end_idx + 1..];
            } else {
                // Unclosed '%(', treat remainder as literal
                result.push_str(&rest[start_idx..]);
                rest = "";
                break;
            }
        }

        result.push_str(rest);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_macro_expansion_basic() {
        let mut ctx = MacroContext::new();
        ctx.commit = Some("a1b2c3d4e5f6".to_string());
        ctx.file = Some("src/main.rs".to_string());

        let cmd = ctx
            .expand("git log -p %(commit) -- %(file)", |_| None)
            .unwrap();
        assert_eq!(cmd, "git log -p 'a1b2c3d4e5f6' -- 'src/main.rs'");
    }

    #[test]
    fn test_macro_expansion_injection_defense() {
        let mut ctx = MacroContext::new();
        // Malicious branch name crafted to execute arbitrary shell commands
        ctx.branch = Some("feature/foo; rm -rf ~; #".to_string());
        ctx.commit = Some("$(reboot)".to_string());

        let cmd = ctx
            .expand("git checkout %(branch) && git show %(commit)", |_| None)
            .unwrap();

        // Must be safely single-quoted
        assert_eq!(
            cmd,
            "git checkout 'feature/foo; rm -rf ~; #' && git show '$(reboot)'"
        );
    }

    #[test]
    fn test_macro_prompt_tokens() {
        assert!(MacroContext::has_prompt_token(
            "git checkout -b %(prompt Branch name: )"
        ));
        assert!(MacroContext::has_prompt_token("echo %(prompt)"));
        assert!(!MacroContext::has_prompt_token("git log %(commit)"));

        let labels = MacroContext::extract_prompt_labels(
            "git commit -m %(prompt Commit message: ) --author=%(prompt Author: )",
        );
        assert_eq!(labels, vec!["Commit message:", "Author:"]);
    }

    #[test]
    fn test_macro_prompt_resolution() {
        let ctx = MacroContext::new();
        let cmd = ctx
            .expand(
                "git tag -a %(prompt Tag name: ) -m %(prompt Message: )",
                |msg| match msg {
                    "Tag name:" => Some("v1.0.0".to_string()),
                    "Message:" => Some("Release 1.0.0".to_string()),
                    _ => None,
                },
            )
            .unwrap();

        assert_eq!(cmd, "git tag -a 'v1.0.0' -m 'Release 1.0.0'");
    }

    #[test]
    fn test_macro_prompt_cancellation() {
        let ctx = MacroContext::new();
        let res = ctx.expand("git tag %(prompt Tag name: )", |_| None);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("cancelled"));
    }

    #[test]
    fn test_unknown_macro_error() {
        let ctx = MacroContext::new();
        let res = ctx.expand("git show %(unknown_var)", |_| None);
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("Unknown or unavailable macro")
        );
    }

    #[test]
    fn test_macro_context_direct_interpolation() {
        use crate::quote::interpolate_command;
        let mut ctx = MacroContext::new();
        ctx.commit = Some("abc1234".to_string());
        ctx.file = Some("src/lib.rs".to_string());
        ctx.lineno = Some(42);

        let cmd = "git show %(commit):%(file) --line %(lineno)";
        let interpolated = interpolate_command(cmd, &ctx).expect("direct MacroContext lookup");
        assert_eq!(interpolated, "git show 'abc1234':'src/lib.rs' --line '42'");
    }

    #[test]
    fn test_macro_context_all_vars() {
        let mut ctx = MacroContext::new();
        assert_eq!(ctx.get_var("directory"), Some(".".to_string()));
        ctx.directory = Some("crates/tigrs".to_string());
        ctx.commit = Some("c1".to_string());
        ctx.head = Some("h1".to_string());
        ctx.blob = Some("b1".to_string());
        ctx.branch = Some("main".to_string());
        ctx.remote = Some("origin".to_string());
        ctx.tag = Some("v1.0".to_string());
        ctx.refname = Some("refs/heads/main".to_string());
        ctx.stash = Some("stash@{0}".to_string());
        ctx.file = Some("foo.rs".to_string());
        ctx.file_old = Some("bar.rs".to_string());
        ctx.lineno = Some(10);
        ctx.lineno_old = Some(5);
        ctx.text = Some("hello world".to_string());
        ctx.repo_git_dir = Some(".git".to_string());
        ctx.repo_worktree = Some("/path/to/repo".to_string());
        ctx.repo_prefix = Some("crates/".to_string());
        ctx.repo_cdup = Some("../".to_string());
        ctx.repo_head = Some("main".to_string());
        ctx.repo_remote = Some("upstream".to_string());

        assert_eq!(ctx.get_var("nonexistent"), None);
        assert_eq!(ctx.get_var("commit"), Some("c1".to_string()));
        assert_eq!(ctx.get_var("lineno_old"), Some("5".to_string()));
        assert_eq!(
            ctx.get_var("repo:worktree"),
            Some("/path/to/repo".to_string())
        );
        assert_eq!(ctx.get_var("lineno"), Some("10".to_string()));
        assert_eq!(ctx.get_var("directory"), Some("crates/tigrs".to_string()));
    }

    #[test]
    fn test_macro_prompt_empty_label_and_unclosed_tokens() {
        assert!(!MacroContext::has_prompt_token("git status %(commit"));
        assert_eq!(
            MacroContext::extract_prompt_labels("git commit -m %(prompt) -m %(unclosed"),
            vec![""]
        );

        let ctx = MacroContext::new();
        // %(prompt) without label
        let resolved = ctx
            .expand("echo %(prompt)", |msg| {
                assert_eq!(msg, "");
                Some("hello".to_string())
            })
            .unwrap();
        assert_eq!(resolved, "echo 'hello'");

        // %(prompt) without label cancelled
        let cancelled = ctx.expand("echo %(prompt)", |_| None);
        assert!(cancelled.is_err());

        // Unclosed '%(' is treated as literal
        let _literal = ctx
            .expand("echo %(commit) %(unclosed", |_| None)
            .unwrap_err();
        // Should error because %(commit) is unavailable, but if we set commit:
        let mut ctx2 = MacroContext::new();
        ctx2.commit = Some("abc".to_string());
        let literal2 = ctx2
            .expand("echo %(commit) %(unclosed-token", |_| None)
            .unwrap();
        assert_eq!(literal2, "echo 'abc' %(unclosed-token");
    }
}
