// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Streaming topological commit revwalk engine.
//!
//! Provides memory-bounded, cancellable iteration over commit history with
//! control-code sanitization, author interning and metadata extraction.

use crate::types::{CommitSummary, ParentIds};
use gix::ObjectId;
use gix::bstr::ByteSlice;
use std::collections::HashMap;
use std::sync::Arc;
use tigrs_core::ansi::strip_control_chars;
use tigrs_core::cancel::CancellationToken;
use tigrs_core::error::{Result, TigError};

/// Specification for commit history traversal and filtering.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RevwalkSpec {
    /// Commits to include in traversal (starting tips).
    ///
    /// An empty list traverses nothing. The implicit "walk from `HEAD`" default
    /// is applied by [`crate::GitEngine::resolve_spec_tips`], which can read
    /// `HEAD` from every supported ref backend.
    pub included: Vec<ObjectId>,
    /// Commits (and their ancestors) to exclude from traversal (`^ref`, `A..B`).
    pub excluded: Vec<ObjectId>,
    /// Pathspecs to filter commits by (only include commits that modified these paths).
    pub pathspecs: Vec<String>,
    /// Human-readable title for the view header (e.g. "main..feature", "HEAD -- src/").
    pub display_title: String,
}

impl RevwalkSpec {
    /// Returns whether this walks one tip's complete history, unfiltered.
    ///
    /// Only then does a repository-wide commit count (such as the
    /// commit-graph's) describe the same set of commits the walk will yield.
    /// Any exclusion or pathspec makes that number meaningless as a total.
    #[must_use]
    pub fn is_full_history(&self) -> bool {
        self.included.len() == 1 && self.excluded.is_empty() && self.pathspecs.is_empty()
    }
}

/// Matches a repo-relative path against a git pathspec pattern.
pub fn path_matches(path: &str, pattern: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "." || pattern == "./" {
        return true;
    }
    let norm_pat = pattern
        .strip_prefix("./")
        .unwrap_or(pattern)
        .trim_end_matches('/');
    let norm_path = path.strip_prefix("./").unwrap_or(path);

    if norm_path == norm_pat {
        return true;
    }
    if norm_path.starts_with(norm_pat)
        && let Some(rest) = norm_path.strip_prefix(norm_pat)
        && rest.starts_with('/')
    {
        return true;
    }
    false
}

/// Looks up a single filename entry inside a Git tree object without heap allocation.
#[inline]
fn find_entry_in_tree(
    repo: &gix::Repository,
    tree_id: ObjectId,
    name: &[u8],
) -> Option<(bool, ObjectId)> {
    let obj = repo.find_object(tree_id).ok()?;
    let tree_iter = gix::objs::TreeRefIter::from_bytes(&obj.data, tree_id.kind());
    for entry_res in tree_iter {
        let entry = entry_res.ok()?;
        if entry.filename == name {
            return Some((entry.mode.is_tree(), entry.oid.to_owned()));
        }
    }
    None
}

/// Checks if a normalized pathspec differs between `old_tree_id` (if any) and `new_tree_id`
/// in $O(\text{depth})$ time by walking down Merkle tree hashes and short-circuiting whenever
/// a parent directory's subtree `ObjectId` is identical (`Phase C`).
fn pathspec_changed_merkle(
    repo: &gix::Repository,
    old_tree_id: Option<ObjectId>,
    new_tree_id: ObjectId,
    pathspec: &str,
) -> bool {
    let norm = pathspec
        .trim()
        .strip_prefix("./")
        .unwrap_or(pathspec.trim())
        .trim_end_matches('/');
    if norm.is_empty() || norm == "." {
        return old_tree_id != Some(new_tree_id);
    }
    if old_tree_id == Some(new_tree_id) {
        return false;
    }

    let mut old_curr = old_tree_id;
    let mut new_curr = Some(new_tree_id);
    let mut comps = norm.split('/').peekable();

    while let Some(comp) = comps.next() {
        if comp.is_empty() {
            continue;
        }
        // Merkle DAG short-circuit: if both trees have the exact same subtree ObjectId
        // (or neither has the directory), nothing underneath could possibly differ.
        if old_curr == new_curr {
            return false;
        }
        let is_last = comps.peek().is_none();
        let comp_bytes = comp.as_bytes();

        let old_entry = old_curr.and_then(|tid| find_entry_in_tree(repo, tid, comp_bytes));
        let new_entry = new_curr.and_then(|tid| find_entry_in_tree(repo, tid, comp_bytes));

        if is_last {
            return old_entry != new_entry;
        }

        old_curr = old_entry
            .filter(|(is_tree, _)| *is_tree)
            .map(|(_, oid)| oid);
        new_curr = new_entry
            .filter(|(is_tree, _)| *is_tree)
            .map(|(_, oid)| oid);
    }
    false
}

fn commit_treesame_modifies_pathspecs(
    repo: &gix::Repository,
    new_tree_oid: ObjectId,
    parents: &[ObjectId],
    pathspecs: &[String],
) -> bool {
    if parents.is_empty() {
        return pathspecs
            .iter()
            .any(|ps| pathspec_changed_merkle(repo, None, new_tree_oid, ps));
    }

    let mut parent_trees =
        smallvec::SmallVec::<[Option<ObjectId>; 2]>::with_capacity(parents.len());
    for parent_id in parents {
        let old_tree_oid = repo.find_object(*parent_id).ok().and_then(|p| {
            gix::objs::CommitRef::from_bytes(&p.data, parent_id.kind())
                .ok()
                .map(|c| c.tree())
        });
        parent_trees.push(old_tree_oid);
    }

    // Git TREESAME rule: a commit modifies pathspec `ps` iff `ps` differs from EVERY parent's tree.
    pathspecs.iter().any(|ps| {
        parent_trees
            .iter()
            .all(|&old_tree_oid| pathspec_changed_merkle(repo, old_tree_oid, new_tree_oid, ps))
    })
}

/// Checks if an already loaded commit modified any file matching the given pathspecs.
pub fn commit_object_modifies_pathspecs(
    repo: &gix::Repository,
    commit: &gix::Commit<'_>,
    parents: &[ObjectId],
    pathspecs: &[String],
) -> bool {
    if pathspecs.is_empty() {
        return true;
    }

    let Ok(new_tree) = commit.tree_id() else {
        return false;
    };
    commit_treesame_modifies_pathspecs(repo, new_tree.detach(), parents, pathspecs)
}

/// Checks if a commit modified any file matching the given pathspecs.
pub fn commit_modifies_pathspecs(
    repo: &gix::Repository,
    commit_id: ObjectId,
    parents: &[ObjectId],
    pathspecs: &[String],
) -> bool {
    if pathspecs.is_empty() {
        return true;
    }

    let Ok(commit_obj) = repo.find_object(commit_id) else {
        return false;
    };
    let Some(commit) = commit_obj
        .peel_to_kind(gix::object::Kind::Commit)
        .ok()
        .and_then(|o| o.try_into_commit().ok())
    else {
        return false;
    };
    commit_object_modifies_pathspecs(repo, &commit, parents, pathspecs)
}

/// Streams commits starting from `start_id` in topological/date order.
pub fn stream_commit_chunks(
    repo: &gix::Repository,
    start_id: ObjectId,
    chunk_size: usize,
    cancel: CancellationToken,
) -> Result<impl Iterator<Item = Result<Vec<CommitSummary>>> + '_> {
    let spec = RevwalkSpec {
        included: vec![start_id],
        excluded: Vec::new(),
        pathspecs: Vec::new(),
        display_title: start_id.to_hex().to_string(),
    };
    stream_commit_chunks_with_spec(repo, spec, chunk_size, cancel)
}

/// Streams commits adhering to a complete [`RevwalkSpec`] (ranges, exclusions, pathspecs).
///
/// The walk starts from exactly the tips named in `spec.included`; a spec with
/// no tips yields no commits. Resolving the implicit `HEAD` default is the
/// caller's responsibility (see [`crate::GitEngine::resolve_spec_tips`]) because
/// `gix` cannot read `HEAD` from the reftable backend.
pub fn stream_commit_chunks_with_spec(
    repo: &gix::Repository,
    spec: RevwalkSpec,
    chunk_size: usize,
    cancel: CancellationToken,
) -> Result<impl Iterator<Item = Result<Vec<CommitSummary>>> + '_> {
    let walk_platform = repo.rev_walk(spec.included);
    let walk_platform = if spec.excluded.is_empty() {
        walk_platform
    } else {
        walk_platform.with_hidden(spec.excluded)
    };

    let walk = walk_platform
        .all()
        .map_err(|err| TigError::Git(format!("Revwalk initialization failed: {err}")))?;

    Ok(CommitChunkIterator {
        repo,
        walk,
        chunk_size,
        first_batch_yielded: false,
        cancel,
        finished: false,
        authors: AuthorInterner::default(),
        pathspecs: spec.pathspecs,
    })
}

/// Deduplicates author names across a revwalk.
///
/// Repositories contain vastly more commits than distinct authors, so sharing
/// one allocation per author instead of one per commit removes the single
/// largest contributor to retained memory during a full-history walk.
#[derive(Default)]
struct AuthorInterner {
    seen: HashMap<Box<[u8]>, Arc<str>>,
}

impl AuthorInterner {
    fn intern(&mut self, raw: &[u8]) -> Arc<str> {
        if let Some(existing) = self.seen.get(raw) {
            return Arc::clone(existing);
        }
        let sanitized: Arc<str> = Arc::from(strip_control_chars(&raw.as_bstr().to_str_lossy()));
        self.seen.insert(Box::from(raw), Arc::clone(&sanitized));
        sanitized
    }
}

struct CommitChunkIterator<'a, I> {
    repo: &'a gix::Repository,
    walk: I,
    chunk_size: usize,
    first_batch_yielded: bool,
    cancel: CancellationToken,
    finished: bool,
    authors: AuthorInterner,
    pathspecs: Vec<String>,
}

/// Parses an ASCII integer timestamp from raw commit header bytes.
#[inline]
fn parse_ascii_i64(bytes: &[u8]) -> i64 {
    let mut i = 0;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    let mut neg = false;
    if i < bytes.len() && bytes[i] == b'-' {
        neg = true;
        i += 1;
    }
    let mut val: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        val = val
            .saturating_mul(10)
            .saturating_add(i64::from(bytes[i] - b'0'));
        i += 1;
    }
    if neg { -val } else { val }
}

/// Fast SIMD-assisted extractor for raw Git commit object bytes (`Phase C`).
///
/// Extracts `(author_name_bytes, author_timestamp_secs, sanitized_summary)` in a single pass
/// without full `gix::CommitRef` header/GPG/paragraph parsing.
#[inline]
fn extract_commit_metadata_fast(raw: &[u8], time_hint: Option<i64>) -> (&[u8], i64, Box<str>) {
    let (headers, body) = if let Some(pos) = memchr::memmem::find(raw, b"\n\n") {
        (&raw[..pos], &raw[pos + 2..])
    } else {
        (raw, &[][..])
    };

    let body_trimmed = body.strip_prefix(b"\n").unwrap_or(body);
    let line_end = memchr::memchr(b'\n', body_trimmed).unwrap_or(body_trimmed.len());
    let mut summary_bytes = &body_trimmed[..line_end];
    if let Some(stripped) = summary_bytes.strip_suffix(b"\r") {
        summary_bytes = stripped;
    }

    // Fast path: if summary is pure printable ASCII (0x20..=0x7E), construct Box<str> directly
    // without intermediate Cow/String allocations or control-character stripping passes.
    let summary: Box<str> = if summary_bytes.iter().all(|&b| (0x20..=0x7e).contains(&b)) {
        Box::from(std::str::from_utf8(summary_bytes).unwrap_or(""))
    } else {
        Box::from(&*strip_control_chars(
            &summary_bytes.as_bstr().to_str_lossy(),
        ))
    };

    let author_line = if headers.starts_with(b"author ") {
        let end = memchr::memchr(b'\n', headers).unwrap_or(headers.len());
        Some(&headers[7..end])
    } else if let Some(pos) = memchr::memmem::find(headers, b"\nauthor ") {
        let start = pos + 8;
        let rest = &headers[start..];
        let end = memchr::memchr(b'\n', rest).unwrap_or(rest.len());
        Some(&rest[..end])
    } else {
        None
    };

    if let Some(line) = author_line
        && let Some(gt_pos) = memchr::memrchr(b'>', line)
        && let Some(lt_pos) = memchr::memrchr(b'<', &line[..gt_pos])
    {
        let mut name_bytes = &line[..lt_pos];
        while name_bytes.last() == Some(&b' ') {
            name_bytes = &name_bytes[..name_bytes.len() - 1];
        }
        let timestamp = if let Some(t) = time_hint {
            t
        } else {
            parse_ascii_i64(&line[gt_pos + 1..])
        };
        return (name_bytes, timestamp, summary);
    }

    (b"<unknown>", time_hint.unwrap_or(0), summary)
}

type ParsedCommitRaw = (
    ObjectId,
    ParentIds,
    smallvec::SmallVec<[u8; 64]>,
    i64,
    Box<str>,
);

/// Inflates and parses a single commit object using a thread-local repository handle.
fn parse_commit_raw(
    repo: &gix::Repository,
    commit_id: ObjectId,
    parents: ParentIds,
    pathspecs: &[String],
) -> std::result::Result<Option<ParsedCommitRaw>, String> {
    let obj = repo
        .find_object(commit_id)
        .map_err(|err| format!("Failed to load commit {commit_id}: {err}"))?;

    if !pathspecs.is_empty() {
        let commit = gix::objs::CommitRef::from_bytes(&obj.data, commit_id.kind())
            .map_err(|err| format!("Corrupt commit {commit_id}: {err}"))?;
        if !commit_treesame_modifies_pathspecs(repo, commit.tree(), &parents, pathspecs) {
            return Ok(None);
        }
    }

    let (raw_author, author_time_secs, summary) = extract_commit_metadata_fast(&obj.data, None);
    Ok(Some((
        commit_id,
        parents,
        smallvec::SmallVec::from_slice(raw_author),
        author_time_secs,
        summary,
    )))
}

impl<'a, I> Iterator for CommitChunkIterator<'a, I>
where
    I: Iterator<
        Item = std::result::Result<gix::revision::walk::Info<'a>, gix::revision::walk::iter::Error>,
    >,
{
    type Item = Result<Vec<CommitSummary>>;

    fn next(&mut self) -> Option<Self::Item> {
        use rayon::prelude::*;

        if self.finished {
            return None;
        }

        if self.cancel.is_cancelled() {
            self.finished = true;
            return Some(Err(TigError::Cancelled));
        }

        let target_chunk_size = if self.first_batch_yielded {
            self.chunk_size.max(1)
        } else {
            self.first_batch_yielded = true;
            self.chunk_size.clamp(1, 64)
        };
        let mut chunk = Vec::with_capacity(target_chunk_size);
        let has_pathspecs = !self.pathspecs.is_empty();

        // Fast-path for unfiltered revwalks: sequential iteration over the warm pack delta cache
        // combined with SIMD commit metadata extraction maximizes delta-base hit rates and keeps TTFF < 25ms.
        if !has_pathspecs {
            while chunk.len() < target_chunk_size {
                if self.cancel.is_cancelled() {
                    self.finished = true;
                    if !chunk.is_empty() {
                        return Some(Ok(chunk));
                    }
                    return Some(Err(TigError::Cancelled));
                }

                match self.walk.next() {
                    Some(Ok(info)) => {
                        let commit_id = info.id;
                        let parents: ParentIds = info.parent_ids().map(gix::Id::detach).collect();

                        match self.repo.find_object(commit_id) {
                            Ok(obj) => {
                                let (raw_author, author_time_secs, summary) =
                                    extract_commit_metadata_fast(&obj.data, None);
                                let author_name = self.authors.intern(raw_author);
                                chunk.push(CommitSummary {
                                    id: commit_id,
                                    parents,
                                    author_name,
                                    author_time_secs,
                                    summary,
                                });
                            }
                            Err(err) => {
                                self.finished = true;
                                return Some(Err(TigError::Git(format!(
                                    "Failed to load commit {commit_id}: {err}"
                                ))));
                            }
                        }
                    }
                    Some(Err(err)) => {
                        self.finished = true;
                        return Some(Err(TigError::Git(format!("Revwalk error: {err}"))));
                    }
                    None => {
                        self.finished = true;
                        break;
                    }
                }
            }

            return if chunk.is_empty() {
                None
            } else {
                Some(Ok(chunk))
            };
        }

        // Parallel Rayon pathspec-filtered revwalk (`Phase C`)
        let batch_target = 512;
        while chunk.len() < self.chunk_size {
            if self.cancel.is_cancelled() {
                self.finished = true;
                if !chunk.is_empty() {
                    return Some(Ok(chunk));
                }
                return Some(Err(TigError::Cancelled));
            }

            let mut candidates: Vec<(ObjectId, ParentIds)> = Vec::with_capacity(batch_target);
            for _ in 0..batch_target {
                match self.walk.next() {
                    Some(Ok(info)) => {
                        let commit_id = info.id;
                        let parents: ParentIds = info.parent_ids().map(gix::Id::detach).collect();
                        candidates.push((commit_id, parents));
                    }
                    Some(Err(err)) => {
                        self.finished = true;
                        return Some(Err(TigError::Git(format!("Revwalk error: {err}"))));
                    }
                    None => {
                        self.finished = true;
                        break;
                    }
                }
            }

            if candidates.is_empty() {
                break;
            }

            if candidates.len() >= 16 {
                let sync_repo = self.repo.clone().into_sync();
                let pathspecs = &self.pathspecs;
                let cancel = &self.cancel;

                let results: Vec<std::result::Result<Option<ParsedCommitRaw>, String>> =
                    tigrs_core::pool::global_compute_pool().install(|| {
                        candidates
                            .into_par_iter()
                            .map_init(
                                || {
                                    let mut tl_repo = sync_repo.to_thread_local();
                                    tl_repo.object_cache_size_if_unset(32 * 1024 * 1024);
                                    tl_repo.objects.set_pack_cache(|| {
                                        Box::new(
                                            gix::odb::pack::cache::lru::MemoryCappedHashmap::new(
                                                32 * 1024 * 1024,
                                            ),
                                        )
                                    });
                                    tl_repo
                                },
                                |tl_repo, (cid, parents)| {
                                    if cancel.is_cancelled() {
                                        return Ok(None);
                                    }
                                    parse_commit_raw(tl_repo, cid, parents, pathspecs)
                                },
                            )
                            .collect()
                    });

                if self.cancel.is_cancelled() {
                    self.finished = true;
                    return Some(Err(TigError::Cancelled));
                }

                for res in results {
                    match res {
                        Ok(Some((commit_id, parents, raw_author, author_time_secs, summary))) => {
                            let author_name = self.authors.intern(&raw_author);
                            chunk.push(CommitSummary {
                                id: commit_id,
                                parents,
                                author_name,
                                author_time_secs,
                                summary,
                            });
                        }
                        Ok(None) => {}
                        Err(err_msg) => {
                            self.finished = true;
                            return Some(Err(TigError::Git(err_msg)));
                        }
                    }
                }
            } else {
                for (cid, parents) in candidates {
                    match parse_commit_raw(self.repo, cid, parents, &self.pathspecs) {
                        Ok(Some((commit_id, parents, raw_author, author_time_secs, summary))) => {
                            let author_name = self.authors.intern(&raw_author);
                            chunk.push(CommitSummary {
                                id: commit_id,
                                parents,
                                author_name,
                                author_time_secs,
                                summary,
                            });
                        }
                        Ok(None) => {}
                        Err(err_msg) => {
                            self.finished = true;
                            return Some(Err(TigError::Git(err_msg)));
                        }
                    }
                }
            }

            if !chunk.is_empty() {
                return Some(Ok(chunk));
            }
        }

        if chunk.is_empty() {
            None
        } else {
            Some(Ok(chunk))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_full_history_rejects_every_kind_of_filter() {
        let tip = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let other = ObjectId::from_hex(b"2222222222222222222222222222222222222222").unwrap();

        let full = RevwalkSpec {
            included: vec![tip],
            ..Default::default()
        };
        assert!(full.is_full_history());

        // No tips at all walks nothing, so a repository-wide total is wrong.
        assert!(!RevwalkSpec::default().is_full_history());

        assert!(
            !RevwalkSpec {
                included: vec![tip, other],
                ..Default::default()
            }
            .is_full_history()
        );

        assert!(
            !RevwalkSpec {
                included: vec![tip],
                excluded: vec![other],
                ..Default::default()
            }
            .is_full_history()
        );

        assert!(
            !RevwalkSpec {
                included: vec![tip],
                pathspecs: vec!["src/".to_string()],
                ..Default::default()
            }
            .is_full_history()
        );
    }

    #[test]
    fn test_author_interner_shares_allocations() {
        let mut interner = AuthorInterner::default();
        let a = interner.intern(b"Linus Torvalds");
        let b = interner.intern(b"Linus Torvalds");
        let c = interner.intern(b"Greg Kroah-Hartman");

        assert_eq!(&*a, "Linus Torvalds");
        assert_eq!(&*c, "Greg Kroah-Hartman");
        // The same author must resolve to the very same allocation.
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(interner.seen.len(), 2);
    }

    #[test]
    fn test_author_interner_sanitizes() {
        let mut interner = AuthorInterner::default();
        let name = interner.intern(b"Evil\x1b[2JName");
        assert_eq!(&*name, "EvilName");
    }

    #[test]
    fn test_revwalk_spec_default() {
        let spec = RevwalkSpec::default();
        assert!(spec.included.is_empty());
        assert!(spec.excluded.is_empty());
        assert!(spec.pathspecs.is_empty());
        assert!(spec.display_title.is_empty());
    }

    #[test]
    fn test_path_matches() {
        assert!(path_matches("src/main.rs", "src/"));
        assert!(path_matches("src/main.rs", "src"));
        assert!(path_matches("src/sub/mod.rs", "src"));
        assert!(!path_matches("src_other/main.rs", "src"));
        assert!(path_matches("README.md", "README.md"));
        assert!(!path_matches("Cargo.toml", "README.md"));
        assert!(path_matches("any/file.rs", "."));
        assert!(path_matches("any/file.rs", "./"));
        assert!(path_matches("any/file.rs", ""));
        assert!(path_matches("./src/main.rs", "./src"));
        assert!(!path_matches("src/main.rs", "other"));
    }

    fn create_test_repo_with_commits() -> (tempfile::TempDir, gix::Repository, Vec<ObjectId>) {
        use std::fs;
        use std::process::Command;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .expect("git");
            assert!(status.success());
        };

        run(&["init"]);
        run(&["config", "user.name", "Tester"]);
        run(&["config", "user.email", "tester@example.com"]);

        fs::create_dir_all(path.join("src")).expect("mkdir");
        fs::create_dir_all(path.join("docs")).expect("mkdir");

        // Commit 1: src/main.rs
        fs::write(path.join("src/main.rs"), "fn main() {}\n").expect("write");
        run(&["add", "."]);
        run(&["commit", "-m", "Commit 1: Add main"]);

        // Commit 2: docs/index.md
        fs::write(path.join("docs/index.md"), "# Documentation\n").expect("write");
        run(&["add", "."]);
        run(&["commit", "-m", "Commit 2: Add docs"]);

        // Commit 3: update src/main.rs
        fs::write(
            path.join("src/main.rs"),
            "fn main() { println!(\"Hello\"); }\n",
        )
        .expect("write");
        run(&["add", "."]);
        run(&["commit", "-m", "Commit 3: Update main"]);

        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["log", "--format=%H"])
            .current_dir(path)
            .output()
            .expect("git log");
        let log_str = String::from_utf8(output.stdout).unwrap();
        let oids: Vec<ObjectId> = log_str
            .lines()
            .map(|l| ObjectId::from_hex(l.trim().as_bytes()).unwrap())
            .collect();

        let repo = gix::open(path).expect("open repo");
        (dir, repo, oids)
    }

    #[test]
    fn test_revwalk_with_pathspec_filtering() {
        let (_dir, repo, oids) = create_test_repo_with_commits();
        assert_eq!(oids.len(), 3);

        let (_source, cancel) = CancellationToken::new();

        // Pathspec: ["src/"] -> should only include Commit 3 and Commit 1
        let spec_src = RevwalkSpec {
            included: vec![oids[0]], // HEAD (commit 3)
            excluded: Vec::new(),
            pathspecs: vec!["src/".to_string()],
            display_title: "HEAD -- src/".to_string(),
        };

        let iter = stream_commit_chunks_with_spec(&repo, spec_src, 50, cancel.clone()).unwrap();
        let chunks: Vec<Vec<CommitSummary>> = iter.map(|r| r.unwrap()).collect();
        let all_commits: Vec<CommitSummary> = chunks.into_iter().flatten().collect();

        assert_eq!(all_commits.len(), 2);
        assert_eq!(all_commits[0].id, oids[0]); // Commit 3
        assert_eq!(all_commits[1].id, oids[2]); // Commit 1

        // Pathspec: ["docs/"] -> should only include Commit 2
        let spec_docs = RevwalkSpec {
            included: vec![oids[0]],
            excluded: Vec::new(),
            pathspecs: vec!["docs/".to_string()],
            display_title: "HEAD -- docs/".to_string(),
        };

        let iter_docs = stream_commit_chunks_with_spec(&repo, spec_docs, 50, cancel).unwrap();
        let chunks_docs: Vec<Vec<CommitSummary>> = iter_docs.map(|r| r.unwrap()).collect();
        let all_docs_commits: Vec<CommitSummary> = chunks_docs.into_iter().flatten().collect();

        assert_eq!(all_docs_commits.len(), 1);
        assert_eq!(all_docs_commits[0].id, oids[1]); // Commit 2
    }

    #[test]
    fn test_revwalk_cancellation_mid_stream() {
        let (_dir, repo, oids) = create_test_repo_with_commits();

        let (source, cancel) = CancellationToken::new();
        // Cancel immediately
        source.cancel();

        let spec = RevwalkSpec {
            included: vec![oids[0]],
            excluded: Vec::new(),
            pathspecs: Vec::new(),
            display_title: "HEAD".to_string(),
        };

        let mut iter = stream_commit_chunks_with_spec(&repo, spec, 1, cancel).unwrap();
        // Because token is cancelled, next() should return TigError::Cancelled, then None
        assert!(matches!(iter.next(), Some(Err(TigError::Cancelled))));
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_commit_modifies_pathspecs() {
        let (_dir, repo, oids) = create_test_repo_with_commits();
        // oids: [Commit 3, Commit 2, Commit 1]
        // Commit 2 has parent Commit 1
        let commit2_id = oids[1];
        let commit1_id = oids[2];

        // Commit 2 touched docs/index.md
        assert!(commit_modifies_pathspecs(
            &repo,
            commit2_id,
            &[commit1_id],
            &["docs".to_string()]
        ));
        assert!(!commit_modifies_pathspecs(
            &repo,
            commit2_id,
            &[commit1_id],
            &["src".to_string()]
        ));
        // Empty pathspec matches any commit
        assert!(commit_modifies_pathspecs(
            &repo,
            commit2_id,
            &[commit1_id],
            &[]
        ));
    }

    /// A spec without tips walks nothing. The implicit `HEAD` default belongs to
    /// [`crate::GitEngine::resolve_spec_tips`], which supports every ref backend.
    #[test]
    fn test_revwalk_without_tips_yields_no_commits() {
        let (_dir, repo, _oids) = create_test_repo_with_commits();
        let (_source, cancel) = CancellationToken::new();

        let iter = stream_commit_chunks_with_spec(&repo, RevwalkSpec::default(), 50, cancel)
            .expect("an empty spec must not fail");
        let commits: Vec<CommitSummary> = iter.flat_map(|r| r.unwrap()).collect();

        assert!(commits.is_empty());
    }

    #[test]
    fn test_revwalk_adaptive_first_chunk_clamped_to_64() {
        use std::fmt::Write as _;
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();

        let init = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "-b", "main", "--ref-format=files"])
            .current_dir(path)
            .status()
            .expect("git init");
        assert!(init.success());

        let mut fi_stream = String::with_capacity(32 * 1024);
        for i in 1..=180_u32 {
            let msg = format!("Commit number {i}");
            let _ = write!(
                fi_stream,
                "commit refs/heads/main\nmark :{i}\nauthor Alice Developer <alice@example.com> {} +0000\ncommitter Alice Developer <alice@example.com> {} +0000\ndata {}\n{msg}\n",
                1_700_000_000 + i,
                1_700_000_000 + i,
                msg.len(),
            );
            if i > 1 {
                let _ = writeln!(fi_stream, "from :{}", i - 1);
            }
            let _ = writeln!(fi_stream, "M 100644 inline file.txt\ndata 4\n{i:03}\n");
        }

        let mut child = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["fast-import", "--quiet"])
            .current_dir(path)
            .stdin(Stdio::piped())
            .spawn()
            .expect("git fast-import");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(fi_stream.as_bytes())
            .expect("write fast-import stream");
        assert!(child.wait().expect("wait fast-import").success());

        let repo = gix::open(path).expect("open gix repo");
        let head_id = repo.head_id().expect("head_id").detach();

        // 1. When chunk_size = 100 (> 64), the first batch is clamped to 64 for fast TTFF,
        //    the second batch uses the full chunk_size (100), and the final batch has the remaining 16.
        let (_src1, cancel1) = CancellationToken::new();
        let spec1 = RevwalkSpec {
            included: vec![head_id],
            excluded: Vec::new(),
            pathspecs: Vec::new(),
            display_title: "HEAD".to_string(),
        };
        let chunk_lengths: Vec<usize> = stream_commit_chunks_with_spec(&repo, spec1, 100, cancel1)
            .expect("stream with chunk_size 100")
            .map(|res| res.expect("chunk ok").len())
            .collect();
        assert_eq!(chunk_lengths, vec![64, 100, 16]);

        // 2. When chunk_size = 10 (<= 64), the first batch respects chunk_size (10).
        let (_src2, cancel2) = CancellationToken::new();
        let spec2 = RevwalkSpec {
            included: vec![head_id],
            excluded: Vec::new(),
            pathspecs: Vec::new(),
            display_title: "HEAD".to_string(),
        };
        let small_lengths: Vec<usize> = stream_commit_chunks_with_spec(&repo, spec2, 10, cancel2)
            .expect("stream with chunk_size 10")
            .map(|res| res.expect("chunk ok").len())
            .collect();
        assert_eq!(small_lengths.len(), 18);
        assert!(small_lengths.iter().all(|&len| len == 10));
    }

    #[test]
    fn test_commit_modifies_pathspecs_merge_and_root_commits() {
        use std::fs;
        use std::process::Command;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?} failed");
        };

        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "alice@example.com"]);
        run(&["config", "user.name", "Alice Developer"]);

        fs::write(path.join("a.txt"), "v1\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-m", "root commit"]);

        run(&["checkout", "-b", "side"]);
        fs::write(path.join("side.txt"), "side\n").unwrap();
        run(&["add", "side.txt"]);
        run(&["commit", "-m", "side commit"]);

        run(&["checkout", "main"]);
        fs::write(path.join("main_only.txt"), "main\n").unwrap();
        run(&["add", "main_only.txt"]);
        run(&["commit", "-m", "main commit"]);

        // Normal merge commit (TREESAME to side for side.txt and TREESAME to main for main_only.txt)
        run(&["merge", "--no-ff", "side", "-m", "merge side into main"]);

        let repo = gix::open(path).unwrap();
        let merge_id = repo.head_id().unwrap().detach();
        let (_src, cancel) = CancellationToken::new();

        // Walking with pathspec "side.txt" should yield only `side commit` (not the clean merge commit, which is TREESAME to parent 2 for side.txt)
        let spec = RevwalkSpec {
            included: vec![merge_id],
            excluded: Vec::new(),
            pathspecs: vec!["side.txt".to_string()],
            display_title: "HEAD -- side.txt".to_string(),
        };
        let commits: Vec<CommitSummary> = stream_commit_chunks_with_spec(&repo, spec, 50, cancel)
            .unwrap()
            .flat_map(|r| r.unwrap())
            .collect();
        assert_eq!(commits.len(), 1);
        assert_eq!(&*commits[0].summary, "side commit");
    }
}
