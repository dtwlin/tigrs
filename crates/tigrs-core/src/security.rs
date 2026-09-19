// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Global security configuration and trust boundaries.
//!
//! Enforces that every repository is untrusted by default (no path allowlist):
//! repository-local configurations (`core.fsmonitor`, `diff.*.textconv`, `core.pager`,
//! `core.editor`, `filter`/`diff`/`merge` drivers, and repository-local git hooks)
//! are unconditionally neutralized unless explicitly unlocked via `--update-mode` or
//! `:set read-only = false` for user-initiated mutations.

use std::path::Path;

/// Global security policy governing repository trust and external execution boundaries.
///
/// Every repository is treated as untrusted for repository-local config execution (`is_repo_trusted`
/// always returns `false`); no path allowlist (`trusted_repos`) is used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecurityConfig;

impl SecurityConfig {
    /// Creates a new `SecurityConfig`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Returns `false`: every repository is untrusted by default without any path allowlist.
    #[must_use]
    pub const fn is_repo_trusted(&self, _repo_path: &Path) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_repo_untrusted_without_allowlist() {
        let config = SecurityConfig::new();
        for candidate in ["/tmp/untrusted-repo", "/home/user/work/my-repo", "."] {
            let path = Path::new(candidate);
            assert!(!config.is_repo_trusted(path));
        }
    }
}
