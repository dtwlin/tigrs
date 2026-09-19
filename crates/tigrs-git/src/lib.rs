// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Git engine, repository discovery, revwalk, and object retrieval for tigrs.

#![forbid(unsafe_code)]

pub mod blame;
pub mod cache;
pub mod diff;
pub mod discovery;
pub mod engine;
pub mod graph_table;
pub mod invalidation;
pub mod patch;
pub mod path_security;
pub mod reftable;
pub mod revwalk;
pub mod stage;
pub mod status;
pub mod tree;
pub mod types;
pub mod userdiff;
mod userdiff_table;
pub mod watcher;
pub mod worddiff;

pub use blame::{
    BlameLine, BlameResult, compute_blame, compute_blame_gix, compute_blame_via_cli,
    format_epoch_date,
};
pub use cache::GitLruCache;
pub use diff::{
    CommitDiff, DiffHunk, DiffLineKind, DiffSummaryStats, FileChangeStatus, FileDiff, HunkLine,
    compute_commit_diff,
};
pub use engine::GitEngine;
pub use gix::ObjectId;
pub use graph_table::GraphOidTable;
pub use invalidation::{CacheInvalidator, PackFileStamp, RepoGeneration};
pub use patch::{IndexAbbrev, format_patch};
pub use path_security::{
    apply_untrusted_repo_env, discover_git_metadata_dirs, safe_git_command, verify_relative_path,
    verify_worktree_path_safety,
};
pub use revwalk::{
    RevwalkSpec, commit_modifies_pathspecs, path_matches, stream_commit_chunks,
    stream_commit_chunks_with_spec,
};
pub use stage::{
    apply_patch, discard_file_changes, discard_untracked_file, stage_file, stage_hunk, stage_line,
    synthesize_hunk_patch, synthesize_line_patch, unstage_file, unstage_hunk, unstage_line,
};
pub use status::{StatusItem, StatusReport, StatusSection, compute_status_item_diff, scan_status};
pub use tree::{
    BlobContent, TreeEntry, TreeEntryKind, TreeListing, decode_blob_lines, format_human_size,
    is_binary_data, read_blob, read_blob_at_commit_path, read_tree_at_path,
};
pub use types::{
    CommitSummary, ParentIds, RefEntry, RefKind, ReflogEntry, RepoInfo, StashEntry,
    format_epoch_date_buf, format_relative_date, format_timestamp,
};
pub use userdiff::{is_path_marked_linguist_generated, mark_path_linguist_generated};
pub use watcher::{HEARTBEAT_INTERVAL, RepoChangeEvent, RepoWatcher};
pub use worddiff::{CharSpan, MAX_WORD_DIFF_LINE_CHARS, WordDiff, word_diff};
