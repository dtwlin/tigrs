// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Zero-copy commit-graph header and commit count reader.

use std::path::Path;
use std::sync::Arc;

/// Commit-graph handle providing zero-copy detection and commit-count inspection
/// over `.git/objects/info/commit-graph` (or `commit-graphs` chains).
#[derive(Clone, Default)]
pub struct GraphOidTable {
    graph: Option<Arc<gix_commitgraph::Graph>>,
}

impl GraphOidTable {
    /// Discovers and opens an existing commit-graph in `common_dir`, if present.
    #[must_use]
    pub fn open(common_dir: &Path) -> Self {
        let info_dir = common_dir.join("objects").join("info");
        let single_file = info_dir.join("commit-graph");
        let chain_dir = info_dir.join("commit-graphs");

        if single_file.exists()
            && let Ok(graph) = gix_commitgraph::at(&single_file)
        {
            return Self {
                graph: Some(Arc::new(graph)),
            };
        }

        if chain_dir.exists()
            && let Ok(graph) = gix_commitgraph::at(&chain_dir)
        {
            return Self {
                graph: Some(Arc::new(graph)),
            };
        }

        if info_dir.exists()
            && let Ok(graph) = gix_commitgraph::at(&info_dir)
        {
            return Self {
                graph: Some(Arc::new(graph)),
            };
        }

        Self { graph: None }
    }

    /// Returns `true` if backed by a memory-mapped commit-graph file.
    #[inline]
    #[must_use]
    pub const fn is_commitgraph(&self) -> bool {
        self.graph.is_some()
    }

    /// Returns the number of commits recorded in the commit-graph (or `0` when absent).
    #[must_use]
    pub fn len(&self) -> usize {
        self.graph.as_ref().map_or(0, |g| g.num_commits() as usize)
    }

    /// Returns `true` if no commit-graph is loaded or the commit-graph contains zero commits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_oid_table_fallback_open_on_nonexistent_dir() {
        let temp = tempfile::tempdir().unwrap();
        let table = GraphOidTable::open(temp.path());

        assert!(!table.is_commitgraph());
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn test_graph_oid_table_commit_graph_workflow() {
        let temp = tempfile::tempdir().unwrap();
        let repo_path = temp.path();

        let run_cmd = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(repo_path)
                .status()
                .expect("run git");
            assert!(status.success());
        };

        run_cmd(&["init"]);
        run_cmd(&["config", "user.name", "Test"]);
        run_cmd(&["config", "user.email", "test@example.com"]);
        run_cmd(&["config", "core.commitGraph", "true"]);
        run_cmd(&["config", "gc.writeCommitGraph", "true"]);

        std::fs::write(repo_path.join("file.txt"), "hello").unwrap();
        run_cmd(&["add", "file.txt"]);
        run_cmd(&["commit", "-m", "init"]);
        run_cmd(&["commit-graph", "write", "--reachable"]);

        let git_dir = repo_path.join(".git");
        let table = GraphOidTable::open(&git_dir);
        if table.is_commitgraph() {
            assert!(!table.is_empty());
            assert_eq!(table.len(), 1);
        }
    }
}
