// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Primary `GitEngine` abstraction orchestrating in-process gix operations and Tier-2 fallbacks.

use crate::discovery::discover_repository;
use crate::reftable::{HeadState, resolve_head_state_via_cli};
use crate::types::{CommitSummary, RepoInfo};
use gix::ObjectId;
use parking_lot::RwLock;
use std::path::Path;
use std::sync::Arc;
use tigrs_core::cancel::CancellationToken;
use tigrs_core::error::{Result, TigError};

/// Default baseline size in bytes for the bounded in-memory object cache (64 MB).
pub const PACK_CACHE_LIMIT_BYTES: usize = 64 * 1024 * 1024;

/// Maximum number of warm worker `gix::Repository` instances retained in the shared recycle pool.
const MAX_POOLED_WORKER_REPOS: usize = 24;

/// Reported when `HEAD` exists but no commit has been made yet.
const UNBORN_HEAD_MESSAGE: &str = "HEAD does not point to any commit (unborn branch)";

/// Wrapper around `Option<gix::Repository>` that implements `Deref` so the handle
/// can be returned to the warm worker pool in `Drop` without `unsafe`.
struct PooledRepo(Option<gix::Repository>);

impl std::ops::Deref for PooledRepo {
    type Target = gix::Repository;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("repository present until Drop")
    }
}

impl std::ops::DerefMut for PooledRepo {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("repository present until Drop")
    }
}

fn configure_repo_caches(repo: &mut gix::Repository, object_bytes: usize, pack_bytes: usize) {
    repo.object_cache_size(object_bytes);
    repo.objects.set_pack_cache(move || {
        Box::new(gix::odb::pack::cache::lru::MemoryCappedHashmap::new(
            pack_bytes,
        ))
    });
}

/// The central Git engine managing repository access, cache bounds, and ref routing.
pub struct GitEngine {
    repo: PooledRepo,
    info: RepoInfo,
    /// Resolved once at open time for reftable repositories, where every ref
    /// lookup costs a process spawn. `None` for the in-process `gix` path.
    cached_head: Option<HeadState>,
    invalidator: crate::invalidation::CacheInvalidator,
    graph_table: Arc<RwLock<Option<crate::graph_table::GraphOidTable>>>,
    cache: Arc<crate::cache::GitLruCache>,
    /// Shared pool of warmed worker `gix::Repository` handles recycled across background tasks (`T0-1`).
    worker_repo_pool: Arc<parking_lot::Mutex<Vec<gix::Repository>>>,
    /// Active memory-for-speed profile (`Greedy` by default).
    memory_profile: Arc<parking_lot::Mutex<tigrs_core::MemoryProfile>>,
    /// Whether this `GitEngine` instance is a cloned worker handle that should be recycled on `Drop`.
    is_pooled_clone: bool,
    /// Atomic read-only enforcement gate (`false` by default; enable via `--read-only` or `:set read-only = true`).
    read_only: Arc<std::sync::atomic::AtomicBool>,
}

impl Clone for GitEngine {
    fn clone(&self) -> Self {
        let profile = *self.memory_profile.lock();
        let repo = if let Some(warm_repo) = self.worker_repo_pool.lock().pop() {
            warm_repo
        } else {
            let mut fresh = (*self.repo).clone();
            configure_repo_caches(
                &mut fresh,
                profile.worker_object_cache_bytes(),
                profile.worker_delta_pack_cache_bytes(),
            );
            fresh
        };
        Self {
            repo: PooledRepo(Some(repo)),
            info: self.info.clone(),
            cached_head: self.cached_head.clone(),
            invalidator: self.invalidator.clone(),
            graph_table: Arc::clone(&self.graph_table),
            cache: Arc::clone(&self.cache),
            worker_repo_pool: Arc::clone(&self.worker_repo_pool),
            memory_profile: Arc::clone(&self.memory_profile),
            is_pooled_clone: true,
            read_only: Arc::clone(&self.read_only),
        }
    }
}

impl Drop for GitEngine {
    fn drop(&mut self) {
        if self.is_pooled_clone
            && let Some(repo) = self.repo.0.take()
        {
            let mut pool = self.worker_repo_pool.lock();
            if pool.len() < MAX_POOLED_WORKER_REPOS {
                pool.push(repo);
            }
        }
    }
}

impl GitEngine {
    /// Opens a repository at or above the given path using the default [`tigrs_core::MemoryProfile`] (`Greedy`).
    pub fn open(path: Option<&Path>) -> Result<Self> {
        Self::open_with_profile(path, tigrs_core::MemoryProfile::default())
    }

    /// Opens a repository at or above the given path with an explicit [`tigrs_core::MemoryProfile`].
    pub fn open_with_profile(
        path: Option<&Path>,
        profile: tigrs_core::MemoryProfile,
    ) -> Result<Self> {
        let (mut repo, info) = discover_repository(path)?;

        configure_repo_caches(
            &mut repo,
            profile.object_cache_bytes(),
            profile.delta_pack_cache_bytes(),
        );

        // Reftable repositories must shell out for ref resolution. Doing it once
        // here keeps the startup path to a single `git` spawn instead of one per
        // query, which is the dominant fixed cost of time-to-first-frame.
        let cached_head = if info.is_reftable {
            let dir = info.work_dir.as_deref().unwrap_or(&info.git_dir);
            Some(resolve_head_state_via_cli(dir)?)
        } else {
            None
        };

        let invalidator = crate::invalidation::CacheInvalidator::new();
        let pack_dir = info.common_dir.join("objects").join("pack");
        let _ = invalidator.record_pack_stamps(&pack_dir);

        let graph_table = Arc::new(RwLock::new(None));

        let cache = Arc::new(crate::cache::GitLruCache::with_profile(profile));

        Ok(Self {
            repo: PooledRepo(Some(repo)),
            info,
            cached_head,
            invalidator,
            graph_table,
            cache,
            worker_repo_pool: Arc::new(parking_lot::Mutex::new(Vec::with_capacity(
                MAX_POOLED_WORKER_REPOS,
            ))),
            memory_profile: Arc::new(parking_lot::Mutex::new(profile)),
            is_pooled_clone: false,
            read_only: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    fn with_graph_table<R>(&self, f: impl FnOnce(&crate::graph_table::GraphOidTable) -> R) -> R {
        {
            let guard = self.graph_table.read();
            if let Some(ref table) = *guard {
                return f(table);
            }
        }
        let opened = crate::graph_table::GraphOidTable::open(&self.info.common_dir);
        let mut guard = self.graph_table.write();
        let table = guard.get_or_insert(opened);
        f(table)
    }

    /// Returns the active [`tigrs_core::MemoryProfile`].
    #[must_use]
    pub fn memory_profile(&self) -> tigrs_core::MemoryProfile {
        *self.memory_profile.lock()
    }

    /// Updates the [`tigrs_core::MemoryProfile`] at runtime, resizing LRU budgets and object/pack caches.
    pub fn set_memory_profile(&mut self, profile: tigrs_core::MemoryProfile) {
        *self.memory_profile.lock() = profile;
        self.cache.reconfigure(profile);
        self.worker_repo_pool.lock().clear();
        configure_repo_caches(
            &mut self.repo,
            profile.object_cache_bytes(),
            profile.delta_pack_cache_bytes(),
        );
    }

    /// Returns whether this `GitEngine` is currently locked in Read-Only Mode.
    #[inline]
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Updates the Read-Only Mode lock for this `GitEngine` and all of its clones.
    #[inline]
    pub fn set_read_only(&self, read_only: bool) {
        self.read_only
            .store(read_only, std::sync::atomic::Ordering::Relaxed);
    }

    /// Verifies that the repository is currently in Update Mode (`read_only == false`),
    /// returning [`TigError::ReadOnly`] if Read-Only Mode is active.
    #[inline]
    pub fn ensure_writable(&self, action_desc: &str) -> Result<()> {
        if self.is_read_only() {
            return Err(TigError::ReadOnly(action_desc.to_string()));
        }
        Ok(())
    }

    /// Returns the repository metadata and detected layout.
    pub fn info(&self) -> &RepoInfo {
        &self.info
    }

    /// Resolves the commit `ObjectId` pointed to by `HEAD`.
    ///
    /// Reftable repositories answer from the value cached at open time.
    pub fn head_commit_id(&self) -> Result<ObjectId> {
        self.head_commit_id_opt()?
            .ok_or_else(|| TigError::Git(UNBORN_HEAD_MESSAGE.to_string()))
    }

    /// Resolves `HEAD`, returning `None` when the branch is unborn.
    ///
    /// Callers that can render an empty history should prefer this over
    /// [`Self::head_commit_id`], which treats a commit-less repository as an error.
    pub fn head_commit_id_opt(&self) -> Result<Option<ObjectId>> {
        if let Some(head) = &self.cached_head {
            return Ok(head.commit);
        }

        let head = self
            .repo
            .head()
            .map_err(|err| TigError::Git(format!("Failed to read HEAD: {err}")))?;

        Ok(head.id().map(gix::Id::detach))
    }

    /// Resolves the human-readable branch or ref name currently checked out.
    pub fn current_branch(&self) -> Result<String> {
        if let Some(head) = &self.cached_head {
            return Ok(head.branch.clone());
        }

        let head = self
            .repo
            .head()
            .map_err(|err| TigError::Git(format!("Failed to read HEAD: {err}")))?;

        match head.kind {
            gix::head::Kind::Symbolic(reference) => {
                let full_name = reference.name.as_bstr().to_string();
                let short_name = full_name.strip_prefix("refs/heads/").unwrap_or(&full_name);
                Ok(tigrs_core::ansi::strip_control_chars(short_name).into_owned())
            }
            gix::head::Kind::Detached { target, .. } => {
                let hex = target.to_hex().to_string();
                Ok(hex[..7.min(hex.len())].to_string())
            }
            gix::head::Kind::Unborn(name) => {
                let full = name.as_bstr().to_string();
                let short = full.strip_prefix("refs/heads/").unwrap_or(&full);
                let clean = tigrs_core::ansi::strip_control_chars(short);
                Ok(format!("{clean} (unborn)"))
            }
        }
    }

    /// Streams commits starting from `start_id` (or `HEAD` if None) in chunked batches.
    pub fn stream_commits(
        &self,
        start_id: Option<ObjectId>,
        chunk_size: Option<usize>,
        cancel: CancellationToken,
    ) -> Result<impl Iterator<Item = Result<Vec<CommitSummary>>> + '_> {
        let root_id = match start_id {
            Some(id) => id,
            None => self.head_commit_id()?,
        };
        let batch_size = chunk_size.unwrap_or_else(|| self.memory_profile().revwalk_chunk_size());
        crate::revwalk::stream_commit_chunks(&self.repo, root_id, batch_size, cancel)
    }

    /// Returns `spec` with its implicit starting tip resolved.
    ///
    /// A spec naming no revisions means "walk from `HEAD`". That default is
    /// applied here rather than inside the revwalk because only the engine can
    /// read `HEAD` from a reftable repository, where `gix` cannot instantiate
    /// the reference and would otherwise abort the walk before it starts.
    ///
    /// An unborn `HEAD` leaves the spec without tips, which walks nothing
    /// instead of failing, so a fresh repository renders an empty log.
    pub fn resolve_spec_tips(&self, mut spec: crate::RevwalkSpec) -> Result<crate::RevwalkSpec> {
        if spec.included.is_empty()
            && let Some(head) = self.head_commit_id_opt()?
        {
            spec.included.push(head);
        }
        Ok(spec)
    }

    /// Streams commits matching a structured [`crate::RevwalkSpec`] (ranges, exclusions, pathspecs).
    pub fn stream_commits_spec(
        &self,
        spec: crate::RevwalkSpec,
        chunk_size: Option<usize>,
        cancel: CancellationToken,
    ) -> Result<impl Iterator<Item = Result<Vec<CommitSummary>>> + '_> {
        let spec = self.resolve_spec_tips(spec)?;
        let batch_size = chunk_size.unwrap_or_else(|| self.memory_profile().revwalk_chunk_size());
        crate::revwalk::stream_commit_chunks_with_spec(&self.repo, spec, batch_size, cancel)
    }

    /// Counts the commits a walk over `spec` will yield.
    ///
    /// This is the denominator behind the main view's "N% commits loaded"
    /// progress readout. It delegates to `git rev-list --count`, which applies
    /// the same reachability, exclusion and pathspec rules as
    /// [`Self::stream_commits_spec`], so the total matches what streaming will
    /// eventually deliver. Git answers in a single pass, instantly when a
    /// commit-graph is present.
    ///
    /// Never call this on the startup path: on a large repository without a
    /// commit-graph it costs a full history traversal. Run it on a background
    /// thread and let the result arrive whenever it is ready.
    pub fn count_commits_for_spec(&self, spec: &crate::RevwalkSpec) -> Result<usize> {
        // An empty tip list walks nothing, and `git rev-list` with no revision
        // arguments is an error rather than an empty walk.
        if spec.included.is_empty() {
            return Ok(0);
        }

        let mut cmd = crate::path_security::safe_git_command(self.work_dir());
        cmd.args(["rev-list", "--count"]);
        for id in &spec.included {
            cmd.arg(id.to_string());
        }
        for id in &spec.excluded {
            cmd.arg(format!("^{id}"));
        }
        if !spec.pathspecs.is_empty() {
            cmd.arg("--");
            for path in &spec.pathspecs {
                cmd.arg(path);
            }
        }

        let output = cmd
            .output()
            .map_err(|e| TigError::Git(format!("Failed to run 'git rev-list --count': {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TigError::Git(format!(
                "git rev-list --count failed: {stderr}"
            )));
        }

        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<usize>()
            .map_err(|e| TigError::Git(format!("Unparseable 'git rev-list --count' output: {e}")))
    }

    /// Returns how many commits the commit-graph holds, if one is in use.
    ///
    /// Free to call: the graph is memory-mapped at [`Self::open`] and the count
    /// is a header field, so this is the only total available without walking
    /// history. It counts every commit in the graph rather than those reachable
    /// from one tip, and a graph written before the most recent commits will
    /// undercount, so treat the result as an estimate and only apply it to a
    /// full-history walk ([`crate::RevwalkSpec::is_full_history`]).
    #[must_use]
    pub fn commit_graph_commit_count(&self) -> Option<usize> {
        self.with_graph_table(|table| table.is_commitgraph().then(|| table.len()))
    }

    /// Returns a fast (`< 2 ms`) estimate of the repository's total commit count without walking history.
    ///
    /// Checks the memory-mapped `commit-graph` header first ([`Self::commit_graph_commit_count`]);
    /// when no `commit-graph` is present (the default for fresh `git clone` repositories), falls
    /// back to [`Self::estimate_pack_commit_count`], which binary-searches the contiguous
    /// `OBJ_COMMIT` region in `.git/objects/pack/pack-*.idx` (and `pack-*.rev` when available).
    #[must_use]
    pub fn fast_commit_count_estimate(&self) -> Option<usize> {
        self.commit_graph_commit_count()
            .or_else(|| self.estimate_pack_commit_count())
    }

    /// Estimates the total number of commits across all packfiles in `.git/objects/pack` in `< 2 ms`.
    ///
    /// Git's `pack-objects` places `OBJ_COMMIT` (`type == 1`) contiguously in pack-offset order.
    /// When a `pack-*.rev` reverse index is present alongside `pack-*.idx` and `pack-*.pack`,
    /// this probes and binary-searches the `.rev` table directly (`~99.9%` accuracy). When no
    /// `.rev` file exists, it reads and sorts the first `16,384` `.idx` offsets (a uniform
    /// cryptographic-hash sample of the pack), binary-searches the sample's contiguous commit
    /// span, and scales by `total_objects / sample_size` (`~99.3%` accuracy).
    #[must_use]
    pub fn estimate_pack_commit_count(&self) -> Option<usize> {
        let pack_dir = self.info.common_dir.join("objects").join("pack");
        let entries = std::fs::read_dir(&pack_dir).ok()?;
        let hash_len = self.repo.object_hash().len_in_bytes() as u64;

        let mut total_commits = 0usize;
        let mut inspected_any = false;

        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if !name.starts_with("pack-")
                || !std::path::Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("idx"))
            {
                continue;
            }
            if let Some(pack_commits) = estimate_single_pack_commits(&path, hash_len) {
                total_commits = total_commits.saturating_add(pack_commits);
                inspected_any = true;
            }
        }

        (inspected_any && total_commits > 0).then_some(total_commits)
    }

    /// Parses revision, range, exclusion and path arguments into a [`crate::RevwalkSpec`].
    pub fn parse_rev_args(&self, raw_args: &[String]) -> Result<crate::RevwalkSpec> {
        let mut included = Vec::new();
        let mut excluded = Vec::new();
        let mut pathspecs = Vec::new();
        let mut title_parts = Vec::new();

        let dashdash_idx = raw_args.iter().position(|a| a == "--");
        let (rev_tokens, path_tokens) = if let Some(idx) = dashdash_idx {
            (&raw_args[..idx], &raw_args[idx + 1..])
        } else {
            (raw_args, &[][..])
        };

        for p in path_tokens {
            pathspecs.push(p.clone());
        }

        for arg in rev_tokens {
            title_parts.push(arg.clone());

            // Symmetric difference A...B
            if let Some((left, right)) = arg.split_once("...") {
                let left_rev = if left.is_empty() { "HEAD" } else { left };
                let right_rev = if right.is_empty() { "HEAD" } else { right };
                let left_id = self.resolve_revision(left_rev)?;
                let right_id = self.resolve_revision(right_rev)?;
                included.push(left_id);
                included.push(right_id);
                continue;
            }

            // Revision range A..B
            if let Some((left, right)) = arg.split_once("..") {
                let left_rev = if left.is_empty() { "HEAD" } else { left };
                let right_rev = if right.is_empty() { "HEAD" } else { right };
                let left_id = self.resolve_revision(left_rev)?;
                let right_id = self.resolve_revision(right_rev)?;
                excluded.push(left_id);
                included.push(right_id);
                continue;
            }

            // Exclusion ^ref
            if let Some(rev) = arg.strip_prefix('^') {
                let id = self.resolve_revision(rev)?;
                excluded.push(id);
                continue;
            }

            // Revision resolution or bare path check
            match self.resolve_revision(arg) {
                Ok(id) => {
                    included.push(id);
                }
                Err(rev_err) => {
                    if dashdash_idx.is_none() && self.work_dir().join(arg).exists() {
                        pathspecs.push(arg.clone());
                    } else {
                        return Err(TigError::Argument(format!(
                            "ambiguous argument '{arg}': unknown revision or path not in the working tree.\nUse '--' to separate paths from revisions, like this:\n'tigrs [<revision>...] -- [<file>...]'\nDetails: {rev_err}"
                        )));
                    }
                }
            }
        }

        let default_title = if pathspecs.is_empty() {
            self.current_branch().unwrap_or_else(|_| "HEAD".to_string())
        } else {
            let branch = self.current_branch().unwrap_or_else(|_| "HEAD".to_string());
            format!("{branch} -- {}", pathspecs.join(" "))
        };

        let display_title = if title_parts.is_empty() {
            default_title
        } else if !pathspecs.is_empty() && dashdash_idx.is_some() {
            format!("{} -- {}", title_parts.join(" "), pathspecs.join(" "))
        } else {
            title_parts.join(" ")
        };

        Ok(crate::RevwalkSpec {
            included,
            excluded,
            pathspecs,
            display_title,
        })
    }

    /// Computes the complete diff for the specified commit against its parent.
    ///
    /// Results are cached in an in-memory LRU cache to make historical backtracking instant.
    pub fn compute_commit_diff(&self, commit_id: ObjectId) -> Result<crate::diff::CommitDiff> {
        self.compute_commit_diff_cancellable(commit_id, &CancellationToken::none())
    }

    /// Cancellable variant of [`Self::compute_commit_diff`].
    pub fn compute_commit_diff_cancellable(
        &self,
        commit_id: ObjectId,
        cancel: &CancellationToken,
    ) -> Result<crate::diff::CommitDiff> {
        cancel.check_cancelled()?;
        if let Some(cached) = self.cache.get_diff(&commit_id) {
            return Ok((*cached).clone());
        }

        let diff = crate::diff::compute_commit_diff_cancellable(&self.repo, commit_id, cancel)?;
        self.cache.insert_diff(commit_id, Arc::new(diff.clone()));
        Ok(diff)
    }

    /// Computes and returns an `Arc<CommitDiff>`, reusing the LRU cache without cloning.
    pub fn compute_commit_diff_cached(
        &self,
        commit_id: ObjectId,
    ) -> Result<Arc<crate::diff::CommitDiff>> {
        self.compute_commit_diff_cached_cancellable(commit_id, &CancellationToken::none())
    }

    /// Cancellable variant of [`Self::compute_commit_diff_cached`].
    pub fn compute_commit_diff_cached_cancellable(
        &self,
        commit_id: ObjectId,
        cancel: &CancellationToken,
    ) -> Result<Arc<crate::diff::CommitDiff>> {
        cancel.check_cancelled()?;
        if let Some(cached) = self.cache.get_diff(&commit_id) {
            return Ok(cached);
        }

        let diff = Arc::new(crate::diff::compute_commit_diff_cancellable(
            &self.repo, commit_id, cancel,
        )?);
        self.cache.insert_diff(commit_id, diff.clone());
        Ok(diff)
    }

    /// Checks the LRU cache for a previously computed commit diff without performing
    /// any git object lookups or Myers diff calculations.
    pub fn get_cached_diff(&self, commit_id: &ObjectId) -> Option<Arc<crate::diff::CommitDiff>> {
        self.cache.get_diff(commit_id)
    }

    /// Resolves a revision specification (e.g. "HEAD", "HEAD~1", branch name, or commit hash) to an `ObjectId`.
    pub fn resolve_revision(&self, rev: &str) -> Result<ObjectId> {
        if (rev == "HEAD" || rev == "@")
            && let Some(ref head) = self.cached_head
            && let Some(oid) = head.commit
        {
            return Ok(oid);
        }
        if self.info.is_reftable {
            let dir = self.info.work_dir.as_deref().unwrap_or(&self.info.git_dir);
            crate::reftable::resolve_rev_via_cli(dir, rev)
        } else {
            self.repo
                .rev_parse_single(rev)
                .map(gix::Id::detach)
                .map_err(|err| TigError::Git(format!("Failed to resolve revision '{rev}': {err}")))
        }
    }

    /// Lists all branches (local and remote) and tags in the repository.
    pub fn list_refs(&self) -> Result<Vec<crate::RefEntry>> {
        let output = crate::path_security::safe_git_command(self.work_dir())
            .args([
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname)%00%(if)%(*objectname)%(then)%(*objectname)%(else)%(objectname)%(end)%00%(committerdate:raw)%00%(authorname)%00%(subject)",
            ])
            .output()
            .map_err(|e| TigError::Git(format!("Failed to run 'git for-each-ref': {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TigError::Git(format!("git for-each-ref failed: {stderr}")));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut entries = Vec::new();

        for line in stdout.lines() {
            let parts: Vec<&str> = line.split('\0').collect();
            if parts.len() >= 5 {
                let full_name = tigrs_core::ansi::strip_control_chars(parts[0]).into_owned();
                let (kind, short_name) = if let Some(sub) = full_name.strip_prefix("refs/heads/") {
                    (crate::RefKind::LocalBranch, sub.to_string())
                } else if let Some(sub) = full_name.strip_prefix("refs/remotes/") {
                    (crate::RefKind::RemoteBranch, sub.to_string())
                } else if let Some(sub) = full_name.strip_prefix("refs/tags/") {
                    (crate::RefKind::Tag, sub.to_string())
                } else if full_name == "refs/stash" {
                    (crate::RefKind::Stash, "stash".to_string())
                } else {
                    continue;
                };

                let Ok(commit_id) = ObjectId::from_hex(parts[1].as_bytes()) else {
                    continue;
                };

                let time_secs = parts[2]
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0);

                let author_name = tigrs_core::ansi::strip_control_chars(parts[3]).into_owned();
                let summary = tigrs_core::ansi::strip_control_chars(parts[4]).into_owned();

                entries.push(crate::RefEntry {
                    full_name,
                    name: short_name,
                    kind,
                    commit_id,
                    summary,
                    author_name,
                    author_time_secs: time_secs,
                });
            }
        }

        Ok(entries)
    }

    /// Lists stashes from `refs/stash` in descending order (`stash@{0}`, etc.).
    pub fn list_stashes(&self) -> Result<Vec<crate::StashEntry>> {
        let output = crate::path_security::safe_git_command(self.work_dir())
            .args(["stash", "list", "--pretty=format:%H%x00%ct%x00%gs"])
            .output()
            .map_err(|e| TigError::Git(format!("Failed to run 'git stash list': {e}")))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut stashes = Vec::new();

        for (idx, line) in stdout.lines().enumerate() {
            let parts: Vec<&str> = line.split('\0').collect();
            if parts.len() >= 3
                && let Ok(commit_id) = ObjectId::from_hex(parts[0].as_bytes())
            {
                let time_secs = parts[1].parse::<i64>().unwrap_or(0);
                let summary = tigrs_core::ansi::strip_control_chars(parts[2]).into_owned();
                stashes.push(crate::StashEntry {
                    index: idx,
                    commit_id,
                    summary,
                    time_secs,
                });
            }
        }

        Ok(stashes)
    }

    /// Reads reflog entries for `ref_name` (e.g. "HEAD" or branch name).
    pub fn read_reflog(&self, ref_name: &str) -> Result<Vec<crate::ReflogEntry>> {
        if ref_name.starts_with('-') {
            return Err(TigError::Security(format!(
                "Ref name cannot start with dash: '{ref_name}'"
            )));
        }
        let output = crate::path_security::safe_git_command(self.work_dir())
            .args([
                "log",
                "-g",
                "-n",
                "200",
                "--no-ext-diff",
                "--no-textconv",
                "--pretty=format:%H%x00%ct%x00%an%x00%gs",
                "--end-of-options",
                ref_name,
            ])
            .output()
            .map_err(|e| TigError::Git(format!("Failed to run 'git log -g': {e}")))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut entries = Vec::new();

        for (idx, line) in stdout.lines().enumerate() {
            let parts: Vec<&str> = line.split('\0').collect();
            if parts.len() >= 4
                && let Ok(new_id) = ObjectId::from_hex(parts[0].as_bytes())
            {
                let time_secs = parts[1].parse::<i64>().unwrap_or(0);
                let committer_name = tigrs_core::ansi::strip_control_chars(parts[2]).into_owned();
                let message = tigrs_core::ansi::strip_control_chars(parts[3]).into_owned();
                entries.push(crate::ReflogEntry {
                    index: idx,
                    old_id: new_id,
                    new_id,
                    committer_name,
                    time_secs,
                    message,
                });
            }
        }

        Ok(entries)
    }

    /// Loads repository status (staged, unstaged, untracked, unmerged) using
    /// two-tier scanning with cancellation support.
    pub fn load_status(&self, cancel: &CancellationToken) -> Result<crate::status::StatusReport> {
        let branch = self.current_branch().unwrap_or_else(|_| "HEAD".to_string());
        let head_commit = self.head_commit_id().ok();
        crate::status::scan_status(&self.repo, &self.info, branch, head_commit, cancel)
    }

    /// Computes a viewable [`CommitDiff`](crate::diff::CommitDiff) for a selected [`StatusItem`](crate::status::StatusItem).
    pub fn compute_status_item_diff(
        &self,
        item: &crate::status::StatusItem,
    ) -> Result<crate::diff::CommitDiff> {
        crate::status::compute_status_item_diff(self.work_dir(), item)
    }

    /// Computes a viewable [`CommitDiff`](crate::diff::CommitDiff) for a selected [`StatusItem`](crate::status::StatusItem) with cancellation support.
    pub fn compute_status_item_diff_cancellable(
        &self,
        item: &crate::status::StatusItem,
        cancel: &tigrs_core::cancel::CancellationToken,
    ) -> Result<crate::diff::CommitDiff> {
        crate::status::compute_status_item_diff_cancellable(self.work_dir(), item, cancel)
    }

    /// Computes a viewable [`CommitDiff`](crate::diff::CommitDiff) covering an entire worktree section.
    ///
    /// Backs the main view's synthetic uncommitted-changes rows. `items` is only
    /// consulted for the untracked section, which has no `git diff` equivalent.
    pub fn compute_status_section_diff(
        &self,
        section: crate::status::StatusSection,
        items: &[crate::status::StatusItem],
    ) -> Result<crate::diff::CommitDiff> {
        self.compute_status_section_diff_cancellable(section, items, &CancellationToken::none())
    }

    /// Cancellable variant of [`Self::compute_status_section_diff`].
    pub fn compute_status_section_diff_cancellable(
        &self,
        section: crate::status::StatusSection,
        items: &[crate::status::StatusItem],
        cancel: &CancellationToken,
    ) -> Result<crate::diff::CommitDiff> {
        crate::status::compute_status_section_diff_cancellable(
            self.work_dir(),
            section,
            items,
            cancel,
        )
    }

    /// Returns the working directory path for this repository.
    pub fn work_dir(&self) -> &Path {
        self.info
            .work_dir
            .as_deref()
            .unwrap_or_else(|| self.info.git_dir.parent().unwrap_or(&self.info.git_dir))
    }

    /// Returns the value of `core.editor` if configured from a trusted scope.
    ///
    /// When `is_repo_trusted` is `false`, values defined in the repository-local
    /// `.git/config` (`Source::Local` / `Source::Worktree`) are ignored to prevent
    /// untrusted repositories from executing arbitrary commands on `e` / `edit`.
    pub fn core_editor(&self, is_repo_trusted: bool) -> Option<String> {
        let config = self.repo.config_snapshot();
        if is_repo_trusted {
            return config
                .string_by("core", None, "editor")
                .map(|value| value.to_string());
        }

        let mut trusted_editor = None;
        if let Some(sections) = config.plumbing().sections_by_name("core") {
            for section in sections {
                let source = section.meta().source;
                if !matches!(
                    source,
                    gix::config::Source::Local | gix::config::Source::Worktree
                ) && let Some(val) = section.value("editor")
                {
                    trusted_editor = Some(val.to_string());
                }
            }
        }
        trusted_editor
    }

    /// Stages an entire file (unstaged or untracked) using raw OS path bytes.
    pub fn stage_file_os(
        &self,
        path: &std::ffi::OsStr,
        old_path: Option<&std::ffi::OsStr>,
    ) -> Result<()> {
        self.ensure_writable("stage_file")?;
        crate::stage::stage_file_os(self.work_dir(), path, old_path)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Stages an entire file (unstaged or untracked).
    pub fn stage_file(&self, path: &str, old_path: Option<&str>) -> Result<()> {
        self.ensure_writable("stage_file")?;
        crate::stage::stage_file(self.work_dir(), path, old_path)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Unstages an entire file from the index using raw OS path bytes.
    pub fn unstage_file_os(
        &self,
        path: &std::ffi::OsStr,
        old_path: Option<&std::ffi::OsStr>,
    ) -> Result<()> {
        self.ensure_writable("unstage_file")?;
        crate::stage::unstage_file_os(self.work_dir(), path, old_path)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Unstages an entire file from the index.
    pub fn unstage_file(&self, path: &str, old_path: Option<&str>) -> Result<()> {
        self.ensure_writable("unstage_file")?;
        crate::stage::unstage_file(self.work_dir(), path, old_path)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Discards unstaged modifications in the working tree for a tracked file using raw OS path bytes.
    pub fn discard_file_changes_os(
        &self,
        path: &std::ffi::OsStr,
        old_path: Option<&std::ffi::OsStr>,
    ) -> Result<()> {
        self.ensure_writable("discard_file_changes")?;
        crate::stage::discard_file_changes_os(self.work_dir(), path, old_path)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Discards unstaged modifications in the working tree for a tracked file.
    pub fn discard_file_changes(&self, path: &str, old_path: Option<&str>) -> Result<()> {
        self.ensure_writable("discard_file_changes")?;
        crate::stage::discard_file_changes(self.work_dir(), path, old_path)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Discards an untracked file or directory using raw OS path bytes.
    pub fn discard_untracked_file_os(&self, path: &std::ffi::OsStr) -> Result<()> {
        self.ensure_writable("discard_untracked_file")?;
        crate::stage::discard_untracked_file_os(self.work_dir(), path)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Discards an untracked file or directory.
    pub fn discard_untracked_file(&self, path: &str) -> Result<()> {
        self.ensure_writable("discard_untracked_file")?;
        crate::stage::discard_untracked_file(self.work_dir(), path)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Stages a single diff hunk into the index using raw path bytes.
    pub fn stage_hunk_bytes(&self, raw_path: &[u8], hunk: &crate::diff::DiffHunk) -> Result<()> {
        self.ensure_writable("stage_hunk")?;
        crate::stage::stage_hunk_bytes(self.work_dir(), raw_path, hunk)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Stages a single diff hunk into the index.
    pub fn stage_hunk(&self, path: &str, hunk: &crate::diff::DiffHunk) -> Result<()> {
        self.ensure_writable("stage_hunk")?;
        crate::stage::stage_hunk(self.work_dir(), path, hunk)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Unstages a single diff hunk from the index using raw path bytes.
    pub fn unstage_hunk_bytes(&self, raw_path: &[u8], hunk: &crate::diff::DiffHunk) -> Result<()> {
        self.ensure_writable("unstage_hunk")?;
        crate::stage::unstage_hunk_bytes(self.work_dir(), raw_path, hunk)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Unstages a single diff hunk from the index.
    pub fn unstage_hunk(&self, path: &str, hunk: &crate::diff::DiffHunk) -> Result<()> {
        self.ensure_writable("unstage_hunk")?;
        crate::stage::unstage_hunk(self.work_dir(), path, hunk)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Stages one or more lines within a hunk into the index using raw path bytes.
    pub fn stage_lines_bytes(
        &self,
        raw_path: &[u8],
        hunk: &crate::diff::DiffHunk,
        line_indices: &[usize],
    ) -> Result<()> {
        self.ensure_writable("stage_lines")?;
        crate::stage::stage_lines_bytes(self.work_dir(), raw_path, hunk, line_indices)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Stages one or more lines within a hunk into the index.
    pub fn stage_lines(
        &self,
        path: &str,
        hunk: &crate::diff::DiffHunk,
        line_indices: &[usize],
    ) -> Result<()> {
        self.ensure_writable("stage_lines")?;
        crate::stage::stage_lines(self.work_dir(), path, hunk, line_indices)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Unstages one or more lines within a hunk from the index using raw path bytes.
    pub fn unstage_lines_bytes(
        &self,
        raw_path: &[u8],
        hunk: &crate::diff::DiffHunk,
        line_indices: &[usize],
    ) -> Result<()> {
        self.ensure_writable("unstage_lines")?;
        crate::stage::unstage_lines_bytes(self.work_dir(), raw_path, hunk, line_indices)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Unstages one or more lines within a hunk from the index.
    pub fn unstage_lines(
        &self,
        path: &str,
        hunk: &crate::diff::DiffHunk,
        line_indices: &[usize],
    ) -> Result<()> {
        self.ensure_writable("unstage_lines")?;
        crate::stage::unstage_lines(self.work_dir(), path, hunk, line_indices)?;
        self.invalidator.bump_generation();
        Ok(())
    }

    /// Stages a single line within a hunk into the index.
    pub fn stage_line(
        &self,
        path: &str,
        hunk: &crate::diff::DiffHunk,
        line_idx: usize,
    ) -> Result<()> {
        self.stage_lines(path, hunk, &[line_idx])
    }

    /// Unstages a single line within a hunk from the index.
    pub fn unstage_line(
        &self,
        path: &str,
        hunk: &crate::diff::DiffHunk,
        line_idx: usize,
    ) -> Result<()> {
        self.unstage_lines(path, hunk, &[line_idx])
    }

    /// Reads a directory tree at the specified subpath for a given commit.
    pub fn read_tree(&self, commit_oid: ObjectId, path: &str) -> Result<crate::tree::TreeListing> {
        self.read_tree_cancellable(commit_oid, path, &CancellationToken::none())
    }

    /// Cancellable variant of [`Self::read_tree`].
    pub fn read_tree_cancellable(
        &self,
        commit_oid: ObjectId,
        path: &str,
        cancel: &CancellationToken,
    ) -> Result<crate::tree::TreeListing> {
        cancel.check_cancelled()?;
        if let Some(cached) = self.cache.get_tree(&commit_oid, path) {
            return Ok((*cached).clone());
        }

        let tree =
            crate::tree::read_tree_at_path_cancellable(&self.repo, commit_oid, path, cancel)?;
        self.cache
            .insert_tree(commit_oid, path, Arc::new(tree.clone()));
        Ok(tree)
    }

    /// Reads a blob object by its OID.
    pub fn read_blob(&self, oid: ObjectId, path: &str) -> Result<crate::tree::BlobContent> {
        if let Some(cached) = self.cache.get_blob(&oid) {
            let mut content = (*cached).clone();
            content.path = path.to_string();
            return Ok(content);
        }

        let blob = crate::tree::read_blob(&self.repo, oid, path)?;
        self.cache.insert_blob(oid, Arc::new(blob.clone()));
        Ok(blob)
    }

    /// Reads a blob object at a specific path and commit.
    pub fn read_blob_at_commit_path(
        &self,
        commit_oid: ObjectId,
        path: &str,
    ) -> Result<crate::tree::BlobContent> {
        crate::tree::read_blob_at_commit_path(&self.repo, commit_oid, path)
    }

    /// Reads a blob object by its OID and returns raw decoded lines for diff context expansion.
    pub fn read_blob_raw_lines(&self, oid: ObjectId) -> Result<Arc<[Arc<str>]>> {
        if let Some(cached) = self.cache.get_raw_blob_lines(&oid) {
            return Ok(cached);
        }
        let lines = crate::tree::read_blob_raw_lines(&self.repo, oid)?;
        self.cache.insert_raw_blob_lines(oid, Arc::clone(&lines));
        Ok(lines)
    }

    /// Returns a reference to the underlying `gix::Repository`.
    #[inline]
    pub fn repository(&self) -> &gix::Repository {
        &self.repo
    }

    /// Retrieves a cached `BlameResult` if available without running a blame walk.
    #[must_use]
    pub fn get_cached_blame(
        &self,
        commit_oid: ObjectId,
        path: &str,
    ) -> Option<Arc<crate::blame::BlameResult>> {
        self.cache.get_blame(&commit_oid, path)
    }

    /// Computes blame annotation for a file at a specific commit.
    pub fn blame_file(
        &self,
        commit_oid: ObjectId,
        path: &str,
    ) -> Result<crate::blame::BlameResult> {
        self.blame_file_cancellable(commit_oid, path, &CancellationToken::none())
    }

    /// Cancellable variant of [`Self::blame_file`].
    pub fn blame_file_cancellable(
        &self,
        commit_oid: ObjectId,
        path: &str,
        cancel: &CancellationToken,
    ) -> Result<crate::blame::BlameResult> {
        if let Some(cached) = self.cache.get_blame(&commit_oid, path) {
            return Ok((*cached).clone());
        }
        let res = crate::blame::compute_blame_cancellable(
            &self.repo,
            Some(self.work_dir()),
            commit_oid,
            path,
            cancel,
        )?;
        self.cache
            .insert_blame(commit_oid, path, Arc::new(res.clone()));
        Ok(res)
    }

    /// Starts a repository filesystem watcher monitoring repository references,
    /// index, and packfiles, with an mtime heartbeat fallback.
    pub fn start_watcher(&self) -> Result<crate::watcher::RepoWatcher> {
        crate::watcher::RepoWatcher::start(&self.info.git_dir, &self.info.common_dir)
    }

    /// Purges in-process repository caches including content-addressed LRUs and increments the generation counter.
    pub fn purge_caches(&mut self) -> Result<u64> {
        self.cache.clear();
        self.worker_repo_pool.lock().clear();
        self.purge_worktree_caches()
    }

    /// Purges worktree/index-dependent caches and increments the generation counter while preserving
    /// content-addressed commit diffs, trees, blobs, and blames (P3-27).
    pub fn purge_worktree_caches(&mut self) -> Result<u64> {
        if self.info.is_reftable {
            let dir = self.info.work_dir.as_deref().unwrap_or(&self.info.git_dir);
            if let Ok(head) = resolve_head_state_via_cli(dir) {
                self.cached_head = Some(head);
            }
        }
        self.cache.purge_worktree();
        let generation = self.invalidator.purge_caches(&mut self.repo)?;
        let pack_dir = self.info.common_dir.join("objects").join("pack");
        if !self
            .invalidator
            .verify_pack_stamps(&pack_dir)
            .unwrap_or(false)
        {
            let _ = self.invalidator.record_pack_stamps(&pack_dir);
            self.worker_repo_pool.lock().clear();
            *self.graph_table.write() = None;
        }

        Ok(generation)
    }

    /// Returns the current cache invalidation generation counter.
    #[inline]
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.invalidator.current_generation()
    }

    /// Returns a reference to the cache invalidator.
    #[inline]
    #[must_use]
    pub fn invalidator(&self) -> &crate::invalidation::CacheInvalidator {
        &self.invalidator
    }

    /// Verifies whether all mapped packfiles have unchanged inode and mtime stamps.
    pub fn verify_pack_stamps(&self) -> Result<bool> {
        let pack_dir = self.info.common_dir.join("objects").join("pack");
        self.invalidator.verify_pack_stamps(&pack_dir)
    }

    /// Returns `true` if a valid commit-graph file is backing commit OID lookups.
    #[must_use]
    pub fn is_commitgraph(&self) -> bool {
        self.with_graph_table(crate::graph_table::GraphOidTable::is_commitgraph)
    }

    /// Adjusts the in-memory object cache size, shedding excess cache pages.
    pub fn shrink_object_cache(&mut self, limit_bytes: usize) {
        self.repo.object_cache_size(limit_bytes);
    }
}

/// Estimates the commit count in a single `pack-*.idx` file using `.rev` (if present) or sorted `.idx` offset sampling.
fn estimate_single_pack_commits(idx_path: &Path, hash_len: u64) -> Option<usize> {
    use std::os::unix::fs::FileExt as _;

    let idx_file = std::fs::File::open(idx_path).ok()?;
    let pack_file = std::fs::File::open(idx_path.with_extension("pack")).ok()?;

    let mut hdr = [0u8; 8];
    idx_file.read_exact_at(&mut hdr, 0).ok()?;
    if hdr[0..4] != [0xff, 0x74, 0x4f, 0x63]
        || u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) != 2
    {
        return None;
    }

    let mut n_buf = [0u8; 4];
    idx_file.read_exact_at(&mut n_buf, 1028).ok()?;
    let n = u32::from_be_bytes(n_buf);
    if n == 0 {
        return Some(0);
    }

    let ofs4_base = 1032u64 + u64::from(n) * (hash_len + 4);
    let ofs8_base = ofs4_base + u64::from(n) * 4;

    let read_idx_offset = |idx_pos: u32| -> Option<u64> {
        if idx_pos >= n {
            return None;
        }
        let mut buf4 = [0u8; 4];
        idx_file
            .read_exact_at(&mut buf4, ofs4_base + u64::from(idx_pos) * 4)
            .ok()?;
        let v = u32::from_be_bytes(buf4);
        if v & 0x8000_0000 == 0 {
            Some(u64::from(v))
        } else {
            let large_idx = u64::from(v & 0x7fff_ffff);
            let mut buf8 = [0u8; 8];
            idx_file
                .read_exact_at(&mut buf8, ofs8_base + large_idx * 8)
                .ok()?;
            Some(u64::from_be_bytes(buf8))
        }
    };

    let is_commit_at_pack_ofs = |mut pack_ofs: u64| -> Option<bool> {
        for _ in 0..64 {
            let mut buf = [0u8; 24];
            let n_read = pack_file.read_at(&mut buf, pack_ofs).ok()?;
            if n_read == 0 {
                return None;
            }
            match (buf[0] >> 4) & 0x07 {
                1 => return Some(true),
                6 => {
                    let mut i = 0usize;
                    while *buf.get(i)? & 0x80 != 0 {
                        i += 1;
                    }
                    i += 1;
                    let mut b = *buf.get(i)?;
                    let mut base_rel = u64::from(b & 0x7f);
                    while b & 0x80 != 0 {
                        i += 1;
                        b = *buf.get(i)?;
                        base_rel = ((base_rel + 1) << 7) | u64::from(b & 0x7f);
                    }
                    pack_ofs = pack_ofs.checked_sub(base_rel)?;
                }
                _ => return Some(false),
            }
        }
        Some(false)
    };

    // Method A: Use `pack-*.rev` reverse index when available (`~99.9%` accuracy).
    let rev_path = idx_path.with_extension("rev");
    if let Ok(rev_file) = std::fs::File::open(&rev_path)
        && let Ok(meta) = rev_file.metadata()
        && meta.len() >= 12 + u64::from(n) * 4
    {
        let mut rev_hdr = [0u8; 12];
        if rev_file.read_exact_at(&mut rev_hdr, 0).is_ok()
            && &rev_hdr[0..4] == b"RIDX"
            && u32::from_be_bytes([rev_hdr[4], rev_hdr[5], rev_hdr[6], rev_hdr[7]]) == 1
        {
            let is_commit_rev = |pos: usize| -> Option<bool> {
                let mut idx_buf = [0u8; 4];
                rev_file
                    .read_exact_at(&mut idx_buf, 12 + (pos as u64) * 4)
                    .ok()?;
                let idx_pos = u32::from_be_bytes(idx_buf);
                let pack_ofs = read_idx_offset(idx_pos)?;
                is_commit_at_pack_ofs(pack_ofs)
            };
            if let Some(commits) = estimate_contiguous_commits(n as usize, is_commit_rev) {
                return Some(commits);
            }
        }
    }

    // Method B: Fallback when no `.rev` file exists — sample and sort up to 16,384 `.idx` offsets (`~99.3%` accuracy).
    let sample_len = (n as usize).min(16_384);
    let mut raw_ofs4 = vec![0u8; sample_len * 4];
    idx_file.read_exact_at(&mut raw_ofs4, ofs4_base).ok()?;
    let mut ofs_list = Vec::with_capacity(sample_len);
    for &chunk in raw_ofs4.as_chunks::<4>().0 {
        let v = u32::from_be_bytes(chunk);
        let pack_ofs = if v & 0x8000_0000 == 0 {
            u64::from(v)
        } else {
            let large_idx = u64::from(v & 0x7fff_ffff);
            let mut buf8 = [0u8; 8];
            idx_file
                .read_exact_at(&mut buf8, ofs8_base + large_idx * 8)
                .ok()?;
            u64::from_be_bytes(buf8)
        };
        ofs_list.push(pack_ofs);
    }
    ofs_list.sort_unstable();

    let is_commit_sample = |pos: usize| -> Option<bool> {
        let pack_ofs = *ofs_list.get(pos)?;
        is_commit_at_pack_ofs(pack_ofs)
    };
    let sample_commits = estimate_contiguous_commits(sample_len, is_commit_sample)?;
    let scaled = ((sample_commits as u64) * u64::from(n) / (sample_len as u64)) as usize;
    Some(scaled)
}

/// Counts `OBJ_COMMIT` entries across `0..total` in pack-offset order using exact scan for small packs
/// and two-sided binary search over the contiguous commit region for large packs.
fn estimate_contiguous_commits(
    total: usize,
    is_commit: impl Fn(usize) -> Option<bool>,
) -> Option<usize> {
    if total == 0 {
        return Some(0);
    }
    if total <= 256 {
        let mut count = 0usize;
        for pos in 0..total {
            if is_commit(pos)? {
                count += 1;
            }
        }
        return Some(count);
    }

    let mut probes = Vec::with_capacity(80);
    let front_limit = (total / 20).max(16).min(total);
    for i in 0..16 {
        probes.push(i * front_limit / 16);
    }
    for i in 0..64 {
        probes.push(i * total / 64);
    }
    probes.sort_unstable();
    probes.dedup();

    let mut first_hit_idx = None;
    let mut last_hit_idx = None;
    for (idx, &p) in probes.iter().enumerate() {
        if is_commit(p)? {
            if first_hit_idx.is_none() {
                first_hit_idx = Some(idx);
            }
            last_hit_idx = Some(idx);
        } else if first_hit_idx.is_some() {
            break;
        }
    }

    let (Some(fi), Some(li)) = (first_hit_idx, last_hit_idx) else {
        return Some(0);
    };

    let mut lo = if fi > 0 { probes[fi - 1] + 1 } else { 0 };
    let mut hi = probes[fi];
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if is_commit(mid)? {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let start_pos = lo;

    let mut lo = probes[li] + 1;
    let mut hi = if li + 1 < probes.len() {
        probes[li + 1]
    } else {
        total
    };
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if is_commit(mid)? {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let end_pos = lo;

    Some(end_pos.saturating_sub(start_pos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn create_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .expect("failed to run git command");
            assert!(status.success(), "git {args:?} failed");
        };

        run(&["init"]);
        run(&["config", "user.name", "Tigrs Tester"]);
        run(&["config", "user.email", "tester@example.com"]);
        run(&["commit", "--allow-empty", "-m", "Initial commit"]);
        run(&["commit", "--allow-empty", "-m", "Second commit"]);

        dir
    }

    #[test]
    fn test_git_engine_open_and_revwalk() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("failed to open test repo");

        assert!(engine.info().work_dir.is_some());
        assert!(!engine.info().is_bare);

        let head_id = engine.head_commit_id().expect("failed to resolve head");
        let branch = engine.current_branch().expect("failed to resolve branch");
        assert!(!branch.is_empty());

        let (_src, token) = CancellationToken::new();
        let stream = engine
            .stream_commits(Some(head_id), Some(10), token)
            .expect("failed stream");
        let batches: Vec<Vec<CommitSummary>> = stream.map(|res| res.unwrap()).collect();

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 2);
        assert_eq!(&*batches[0][0].summary, "Second commit");
        assert_eq!(&*batches[0][0].author_name, "Tigrs Tester");
        assert_eq!(&*batches[0][1].summary, "Initial commit");
    }

    #[test]
    fn test_count_commits_for_spec_matches_the_walk() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("failed to open test repo");

        // The count is a progress denominator, so what matters is that it agrees
        // with the number of commits streaming actually delivers.
        let spec = engine
            .resolve_spec_tips(crate::RevwalkSpec::default())
            .expect("resolve tips");
        let (_src, token) = CancellationToken::new();
        let streamed: usize = engine
            .stream_commits_spec(spec.clone(), Some(10), token)
            .expect("failed stream")
            .map(|batch| batch.unwrap().len())
            .sum();

        assert_eq!(
            engine.count_commits_for_spec(&spec).expect("count"),
            streamed
        );
        assert_eq!(streamed, 2);
    }

    #[test]
    fn test_count_commits_for_spec_honours_exclusions() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("failed to open test repo");

        let head = engine.head_commit_id().expect("head");
        let parent = engine.resolve_revision("HEAD~1").expect("parent");

        let spec = crate::RevwalkSpec {
            included: vec![head],
            excluded: vec![parent],
            ..Default::default()
        };
        assert_eq!(engine.count_commits_for_spec(&spec).expect("count"), 1);
    }

    #[test]
    fn test_count_commits_for_spec_without_tips_is_zero() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("failed to open test repo");

        // An unresolved spec walks nothing; `git rev-list` with no revision
        // arguments would fail, so this case must be answered without spawning.
        let spec = crate::RevwalkSpec::default();
        assert_eq!(engine.count_commits_for_spec(&spec).expect("count"), 0);
    }

    #[test]
    fn test_commit_graph_commit_count_tracks_the_graph_file() {
        let repo_dir = create_test_repo();

        // A freshly created repository has no commit-graph, so there is no free
        // total available and the caller must fall back to counting.
        let engine = GitEngine::open(Some(repo_dir.path())).expect("failed to open test repo");
        assert_eq!(engine.commit_graph_commit_count(), None);

        // `git commit-graph write` silently does nothing (warning, exit 0) when
        // `core.commitGraph` is disabled, which it may well be globally.
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "core.commitGraph", "true"])
            .current_dir(repo_dir.path())
            .status()
            .expect("failed to enable core.commitGraph");
        assert!(status.success());

        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit-graph", "write", "--reachable"])
            .current_dir(repo_dir.path())
            .status()
            .expect("failed to write commit-graph");
        assert!(status.success());
        assert!(
            repo_dir
                .path()
                .join(".git/objects/info/commit-graph")
                .exists()
        );

        // Reopened so the engine maps the newly written graph.
        let engine = GitEngine::open(Some(repo_dir.path())).expect("failed to reopen test repo");
        let spec = engine
            .resolve_spec_tips(crate::RevwalkSpec::default())
            .expect("resolve tips");
        assert_eq!(
            engine.commit_graph_commit_count(),
            Some(engine.count_commits_for_spec(&spec).expect("count"))
        );
    }

    #[test]
    fn test_estimate_pack_commit_count_without_commit_graph() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("failed to open test repo");

        // Unpacked loose objects only -> None
        assert_eq!(engine.estimate_pack_commit_count(), None);

        // Method A: Pack objects with a `.rev` reverse index file (`pack.writeReverseIndex=true`)
        // without writing a commit-graph.
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["-c", "pack.writeReverseIndex=true", "repack", "-ad", "-q"])
            .current_dir(repo_dir.path())
            .status()
            .expect("failed to repack with rev-index");
        assert!(status.success());
        assert_eq!(engine.commit_graph_commit_count(), None);
        assert_eq!(engine.estimate_pack_commit_count(), Some(2));
        assert_eq!(engine.fast_commit_count_estimate(), Some(2));

        // Method B: Remove any `.rev` files in `.git/objects/pack/` to force fallback to
        // `.idx` v2 offset-table sampling.
        let pack_dir = repo_dir.path().join(".git/objects/pack");
        for entry in std::fs::read_dir(&pack_dir)
            .expect("read pack dir")
            .flatten()
        {
            if entry.path().extension().and_then(|e| e.to_str()) == Some("rev") {
                std::fs::remove_file(entry.path()).expect("remove .rev file");
            }
        }
        assert_eq!(engine.estimate_pack_commit_count(), Some(2));
        assert_eq!(engine.fast_commit_count_estimate(), Some(2));
    }

    #[test]
    fn test_estimate_contiguous_commits_binary_search() {
        // Commit region at the very start of the pack: 0..1_850 out of 10_000 objects
        assert_eq!(
            estimate_contiguous_commits(10_000, |pos| Some(pos < 1_850)),
            Some(1_850)
        );
        // Commit region preceded by incremental trees/deltas: 420..2_420 out of 10_000 objects
        assert_eq!(
            estimate_contiguous_commits(10_000, |pos| Some((420..2_420).contains(&pos))),
            Some(2_000)
        );
        // Primary commit region (100..1_600) followed by blobs/deltas and a secondary commit island (7_500..7_900)
        assert_eq!(
            estimate_contiguous_commits(10_000, |pos| {
                Some((100..1_600).contains(&pos) || (7_500..7_900).contains(&pos))
            }),
            Some(1_500)
        );
        // Empty commit region
        assert_eq!(
            estimate_contiguous_commits(10_000, |_| Some(false)),
            Some(0)
        );
    }

    #[test]
    fn test_git_engine_cancellation() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("failed to open test repo");

        let (src, token) = CancellationToken::new();
        src.cancel();

        let stream = engine
            .stream_commits(None, Some(10), token)
            .expect("stream init ok");
        let results: Vec<_> = stream.collect();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Err(TigError::Cancelled)));
    }

    /// Repeated ref queries must not re-spawn `git` on the reftable path.
    #[test]
    fn test_repeated_ref_queries_are_consistent() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("failed to open test repo");

        let a = engine.head_commit_id().unwrap();
        let b = engine.head_commit_id().unwrap();
        assert_eq!(a, b);
        assert_eq!(
            engine.current_branch().unwrap(),
            engine.current_branch().unwrap()
        );
    }

    #[test]
    fn test_git_engine_resolve_revision() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("failed to open test repo");

        let head_id = engine.head_commit_id().unwrap();
        let resolved_head = engine.resolve_revision("HEAD").unwrap();
        assert_eq!(head_id, resolved_head);

        let resolved_parent = engine.resolve_revision("HEAD~1").unwrap();
        assert_ne!(head_id, resolved_parent);

        let hex = head_id.to_hex().to_string();
        let resolved_hex = engine.resolve_revision(&hex).unwrap();
        assert_eq!(head_id, resolved_hex);
    }

    #[test]
    fn test_git_engine_invalidation_and_watcher() {
        let repo_dir = create_test_repo();
        let mut engine = GitEngine::open(Some(repo_dir.path())).expect("open test repo");

        let gen_before = engine.generation();
        assert_eq!(gen_before, 1);

        let gen_after = engine.purge_caches().expect("purge caches");
        assert_eq!(gen_after, 2);
        assert_eq!(engine.generation(), 2);

        let watcher = engine.start_watcher().expect("start watcher");
        assert!(watcher.drain_events().is_empty());
    }

    #[test]
    fn test_parse_rev_args_and_stream_spec() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("open test repo");

        // 1. Empty args -> HEAD
        let spec = engine.parse_rev_args(&[]).unwrap();
        assert!(spec.included.is_empty());
        assert!(spec.excluded.is_empty());
        assert!(spec.pathspecs.is_empty());

        // 2. Single ref
        let spec = engine.parse_rev_args(&["HEAD".to_string()]).unwrap();
        assert_eq!(spec.included.len(), 1);
        assert!(spec.excluded.is_empty());

        // 3. Range A..B (HEAD~1..HEAD)
        let spec = engine
            .parse_rev_args(&["HEAD~1..HEAD".to_string()])
            .unwrap();
        assert_eq!(spec.included.len(), 1);
        assert_eq!(spec.excluded.len(), 1);

        let (_src, token) = CancellationToken::new();
        let stream = engine
            .stream_commits_spec(spec, Some(10), token)
            .expect("stream range ok");
        let batches: Vec<Vec<CommitSummary>> = stream.map(|res| res.unwrap()).collect();
        assert_eq!(batches.len(), 1);
        // Only 1 commit in HEAD~1..HEAD
        assert_eq!(batches[0].len(), 1);
        assert_eq!(&*batches[0][0].summary, "Second commit");

        // 4. Invalid revision -> ambiguous argument error
        let err = engine.parse_rev_args(&["nonexistent_ref_12345".to_string()]);
        assert!(err.is_err());
        let err_msg = err.unwrap_err().to_string();
        assert!(err_msg.contains("ambiguous argument 'nonexistent_ref_12345'"));
    }

    #[test]
    fn test_parse_rev_args_symmetric_difference_and_partial_ranges() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("open test repo");

        // Symmetric difference A...B
        let spec = engine
            .parse_rev_args(&["HEAD~1...HEAD".to_string()])
            .expect("parse symmetric difference");
        assert_eq!(spec.included.len(), 2);
        assert!(spec.excluded.is_empty());

        // Partial symmetric difference ...HEAD and HEAD...
        let spec_left = engine
            .parse_rev_args(&["...HEAD".to_string()])
            .expect("parse ...HEAD");
        assert_eq!(spec_left.included.len(), 2);

        let spec_right = engine
            .parse_rev_args(&["HEAD...".to_string()])
            .expect("parse HEAD...");
        assert_eq!(spec_right.included.len(), 2);

        // Partial range ..HEAD and HEAD..
        let spec_range_left = engine
            .parse_rev_args(&["..HEAD".to_string()])
            .expect("parse ..HEAD");
        assert_eq!(spec_range_left.included.len(), 1);
        assert_eq!(spec_range_left.excluded.len(), 1);

        let spec_range_right = engine
            .parse_rev_args(&["HEAD..".to_string()])
            .expect("parse HEAD..");
        assert_eq!(spec_range_right.included.len(), 1);
        assert_eq!(spec_range_right.excluded.len(), 1);
    }

    #[test]
    fn test_parse_rev_args_exclusion_and_pathspecs() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("open test repo");

        // Exclusion ^ref
        let spec = engine
            .parse_rev_args(&["HEAD".to_string(), "^HEAD~1".to_string()])
            .expect("parse exclusion");
        assert_eq!(spec.included.len(), 1);
        assert_eq!(spec.excluded.len(), 1);

        // Explicit pathspecs separated by --
        let spec_paths = engine
            .parse_rev_args(&[
                "HEAD".to_string(),
                "--".to_string(),
                "file.txt".to_string(),
                "src/".to_string(),
            ])
            .expect("parse pathspecs");
        assert_eq!(spec_paths.included.len(), 1);
        assert_eq!(spec_paths.pathspecs, vec!["file.txt", "src/"]);
        assert!(spec_paths.display_title.contains("HEAD -- file.txt src/"));

        // Only pathspecs after -- with default HEAD
        let spec_only_paths = engine
            .parse_rev_args(&["--".to_string(), "file.txt".to_string()])
            .expect("parse only paths");
        assert!(spec_only_paths.included.is_empty());
        assert_eq!(spec_only_paths.pathspecs, vec!["file.txt"]);
    }

    #[test]
    fn test_parse_rev_args_bare_path_fallback() {
        let repo_dir = create_test_repo();
        let path = repo_dir.path();
        let engine = GitEngine::open(Some(path)).expect("open test repo");

        // Create a working tree file that is not a git revision
        std::fs::write(path.join("working_file.txt"), "hello").expect("write file");

        // Pass the bare filename without --
        let spec = engine
            .parse_rev_args(&["working_file.txt".to_string()])
            .expect("parse bare path");
        assert!(spec.included.is_empty());
        assert_eq!(spec.pathspecs, vec!["working_file.txt"]);
    }

    #[test]
    fn test_stream_commits_with_pathspec_filter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .expect("failed git command");
            assert!(status.success());
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.name", "Tigrs Tester"]);
        run(&["config", "user.email", "tester@example.com"]);

        std::fs::write(path.join("file.txt"), "content 1\n").expect("write file");
        run(&["add", "file.txt"]);
        run(&["commit", "-m", "commit 1: file.txt"]);

        std::fs::write(path.join("other.txt"), "other content\n").expect("write other");
        run(&["add", "other.txt"]);
        run(&["commit", "-m", "commit 2: other.txt"]);

        let engine = GitEngine::open(Some(path)).expect("open test repo");

        // Filter for file.txt (should match only commit 1)
        let spec = engine
            .parse_rev_args(&["HEAD".to_string(), "--".to_string(), "file.txt".to_string()])
            .expect("parse spec");
        let (_src, token) = CancellationToken::new();
        let stream = engine
            .stream_commits_spec(spec, Some(10), token)
            .expect("stream ok");
        let commits: Vec<CommitSummary> = stream.flat_map(|res| res.unwrap()).collect();
        assert_eq!(commits.len(), 1);
        assert_eq!(&*commits[0].summary, "commit 1: file.txt");

        // Filter for a path that was never touched
        let spec_empty = engine
            .parse_rev_args(&[
                "HEAD".to_string(),
                "--".to_string(),
                "nonexistent_file.rs".to_string(),
            ])
            .expect("parse spec");
        let (_src, token) = CancellationToken::new();
        let stream_empty = engine
            .stream_commits_spec(spec_empty, Some(10), token)
            .expect("stream ok");
        let commits_empty: Vec<CommitSummary> = stream_empty.flat_map(|res| res.unwrap()).collect();
        assert_eq!(commits_empty.len(), 0);
    }

    #[test]
    fn test_list_refs_and_stashes() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("open test repo");

        let refs = engine.list_refs().expect("list refs");
        assert!(!refs.is_empty());
        assert!(refs.iter().any(|r| r.name == "master" || r.name == "main"));

        let stashes = engine.list_stashes().expect("list stashes");
        assert!(stashes.is_empty());
    }

    /// Initializes a repository with an explicit ref backend, returning `None`
    /// when the local `git` does not support that format.
    fn create_repo_with_format(format: &str, commits: &[&str]) -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        let init = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "-b", "main", &format!("--ref-format={format}")])
            .current_dir(path)
            .output()
            .expect("failed to run git init");
        if !init.status.success() {
            eprintln!("git does not support --ref-format={format}, skipping");
            return None;
        }

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .expect("failed to run git command");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["config", "user.name", "Tigrs Tester"]);
        run(&["config", "user.email", "tester@example.com"]);
        for message in commits {
            run(&["commit", "--allow-empty", "-m", message]);
        }

        Some(dir)
    }

    /// The implicit `HEAD` tip must be resolved by the engine for every ref
    /// backend; `gix` alone cannot read `HEAD` from reftable storage.
    #[test]
    fn test_resolve_spec_tips_defaults_to_head_on_every_ref_backend() {
        for format in ["files", "reftable"] {
            let Some(repo_dir) =
                create_repo_with_format(format, &["First commit", "Second commit"])
            else {
                continue;
            };
            let engine = GitEngine::open(Some(repo_dir.path())).expect("open test repo");

            let spec = engine.parse_rev_args(&[]).expect("parse empty args");
            assert!(spec.included.is_empty(), "{format}: no explicit tips");

            let resolved = engine.resolve_spec_tips(spec).expect("resolve tips");
            assert_eq!(
                resolved.included,
                vec![engine.head_commit_id().expect("head")],
                "{format}: the implicit tip must be HEAD"
            );

            let (_src, token) = CancellationToken::new();
            let commits: Vec<CommitSummary> = engine
                .stream_commits_spec(resolved, Some(10), token)
                .expect("stream")
                .flat_map(|batch| batch.expect("batch"))
                .collect();
            assert_eq!(commits.len(), 2, "{format}: full history must be walked");
        }
    }

    #[test]
    fn test_resolve_spec_tips_preserves_explicit_tips() {
        let repo_dir = create_test_repo();
        let engine = GitEngine::open(Some(repo_dir.path())).expect("open test repo");

        let spec = engine
            .parse_rev_args(&["HEAD~1..HEAD".to_string()])
            .expect("parse range");
        let expected = spec.clone();

        let resolved = engine.resolve_spec_tips(spec).expect("resolve tips");
        assert_eq!(resolved, expected, "an explicit range must pass through");
    }

    /// A repository with no commits renders an empty log instead of erroring.
    #[test]
    fn test_unborn_repository_resolves_to_no_tips() {
        for format in ["files", "reftable"] {
            let Some(repo_dir) = create_repo_with_format(format, &[]) else {
                continue;
            };
            let engine = GitEngine::open(Some(repo_dir.path())).expect("open unborn repo");

            assert_eq!(
                engine.head_commit_id_opt().expect("head lookup"),
                None,
                "{format}: unborn HEAD has no commit"
            );
            assert!(engine.head_commit_id().is_err(), "{format}");

            let spec = engine
                .resolve_spec_tips(engine.parse_rev_args(&[]).expect("parse args"))
                .expect("resolve tips on unborn HEAD");
            assert!(spec.included.is_empty(), "{format}");

            let (_src, token) = CancellationToken::new();
            let commits: Vec<CommitSummary> = engine
                .stream_commits_spec(spec, Some(10), token)
                .expect("stream")
                .flat_map(|batch| batch.expect("batch"))
                .collect();
            assert!(commits.is_empty(), "{format}");
        }
    }

    #[test]
    fn test_git_engine_lru_cache_hit_and_purge() {
        let repo_dir = create_test_repo();
        let mut engine = GitEngine::open(Some(repo_dir.path())).expect("open repo");
        let head_id = engine.head_commit_id().expect("head id");

        // First call computes diff and caches it
        let diff1 = engine.compute_commit_diff_cached(head_id).expect("diff 1");

        // Second call should return identical Arc pointer from cache
        let diff2 = engine.compute_commit_diff_cached(head_id).expect("diff 2");
        assert!(Arc::ptr_eq(&diff1, &diff2));

        // Worktree purge advances generation but preserves content-addressed commit diff (P3-27)
        let _ = engine.purge_worktree_caches().expect("purge worktree");
        let diff_after_worktree = engine
            .compute_commit_diff_cached(head_id)
            .expect("diff after worktree");
        assert!(Arc::ptr_eq(&diff1, &diff_after_worktree));

        // Full purge clears in-memory LRU
        let _ = engine.purge_caches().expect("purge");
        let diff3 = engine.compute_commit_diff_cached(head_id).expect("diff 3");
        assert_eq!(*diff1, *diff3);
        // After full purge, a fresh Arc is produced
        assert!(!Arc::ptr_eq(&diff1, &diff3));
    }

    #[test]
    fn test_git_engine_core_editor() {
        let repo_dir = create_test_repo();

        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "core.editor", "test-editor-custom"])
            .current_dir(repo_dir.path())
            .status()
            .expect("git config core.editor");
        assert!(status.success());

        let engine = GitEngine::open(Some(repo_dir.path())).expect("reopen repo");
        // Untrusted repository MUST reject repository-local .git/config core.editor
        assert_ne!(
            engine.core_editor(false).as_deref(),
            Some("test-editor-custom")
        );
        // Explicitly trusted repository accepts local core.editor
        assert_eq!(
            engine.core_editor(true).as_deref(),
            Some("test-editor-custom")
        );
    }

    #[test]
    fn test_git_engine_reflog_and_stashes_and_refs() {
        let repo_dir = create_test_repo();
        let path = repo_dir.path();
        let engine = GitEngine::open(Some(path)).expect("open repo");

        // Reflog
        let reflog = engine.read_reflog("HEAD").expect("read reflog");
        assert!(!reflog.is_empty());
        assert_eq!(reflog[0].index, 0);
        assert!(!reflog[0].message.is_empty());

        // Stash
        std::fs::write(path.join("stash_me.txt"), "stash content\n").expect("write");
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", "stash_me.txt"])
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["stash", "push", "-m", "my-test-stash"])
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success());

        let stashes = engine.list_stashes().expect("list stashes");
        assert_eq!(stashes.len(), 1);
        assert!(stashes[0].summary.contains("my-test-stash"));

        // Create tag and remote ref
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["tag", "v1.0.0"])
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success());

        let head = engine.head_commit_id().unwrap();
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "update-ref",
                "refs/remotes/origin/main",
                &head.to_hex().to_string(),
            ])
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success());

        let refs = engine.list_refs().expect("list refs");
        assert!(
            refs.iter()
                .any(|r| r.kind == crate::RefKind::Tag && r.name == "v1.0.0")
        );
        assert!(
            refs.iter()
                .any(|r| r.kind == crate::RefKind::RemoteBranch && r.name == "origin/main")
        );
        assert!(refs.iter().any(|r| r.kind == crate::RefKind::Stash));

        // Engine helpers
        assert!(engine.verify_pack_stamps().is_ok());
        let _ = engine.invalidator();
        let _ = engine.is_commitgraph();
        let mut eng_mut = engine;
        eng_mut.shrink_object_cache(1024 * 1024);
    }

    #[test]
    fn test_git_engine_detached_head_and_pathspecs() {
        let repo_dir = create_test_repo();
        let path = repo_dir.path();

        // Detach HEAD
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["checkout", "--detach", "HEAD"])
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success());

        let engine = GitEngine::open(Some(path)).expect("open detached");
        let branch = engine.current_branch().expect("current branch detached");
        let head = engine.head_commit_id().unwrap();
        assert_eq!(branch, head.to_hex().to_string()[..7]);

        // Resolve revision
        let resolved = engine.resolve_revision("HEAD").expect("resolve HEAD");
        assert_eq!(resolved, head);
        assert!(engine.resolve_revision("nonexistent_rev_xyz_123").is_err());

        // Count with pathspecs
        let head = engine.head_commit_id().unwrap();
        let spec = crate::RevwalkSpec {
            included: vec![head],
            pathspecs: vec!["nonexistent_file.txt".to_string()],
            ..Default::default()
        };
        let count = engine
            .count_commits_for_spec(&spec)
            .expect("count with pathspecs");
        assert_eq!(count, 0);

        // Discard untracked file via engine: default Update Mode is false; set_read_only(true) blocks, set_read_only(false) allows
        std::fs::write(path.join("to_discard.txt"), "hello").unwrap();
        assert!(path.join("to_discard.txt").exists());
        assert!(!engine.is_read_only());
        engine.set_read_only(true);
        assert!(engine.is_read_only());
        assert!(matches!(
            engine.discard_untracked_file("to_discard.txt"),
            Err(tigrs_core::error::TigError::ReadOnly(_))
        ));
        assert!(
            path.join("to_discard.txt").exists(),
            "file must remain untouched in read-only mode"
        );

        engine.set_read_only(false);
        assert!(!engine.is_read_only());
        engine
            .discard_untracked_file("to_discard.txt")
            .expect("engine discard untracked in update mode");
        assert!(!path.join("to_discard.txt").exists());
    }
}
