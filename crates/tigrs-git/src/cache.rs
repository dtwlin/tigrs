// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Bounded in-memory LRU caching for diffs, tree listings, and blobs.
//!
//! Provides instant backtracking when navigating history (e.g. child -> parent -> child)
//! and prevents redundant Myers diff recomputation. Cache entries are keyed by Git object ID
//! and invalidated whenever the repository generation counter advances.

use crate::blame::BlameResult;
use crate::diff::CommitDiff;
use crate::tree::{BlobContent, TreeListing};
use clru::CLruCache;
use gix::ObjectId;
use parking_lot::Mutex;
use std::borrow::Borrow;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Arc;
use tigrs_core::MemoryProfile;

/// Key for the tree listing and blame caches: a commit plus a path within its tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TreeCacheKey(pub ObjectId, pub Box<str>);

/// Borrowable lookup trait enabling zero-allocation `(ObjectId, &str)` probes against [`TreeCacheKey`].
pub trait TreeKey {
    /// Returns the commit or tree object ID component of the cache key.
    fn oid(&self) -> &ObjectId;
    /// Returns the repository-relative path component of the cache key.
    fn path(&self) -> &str;
}

impl TreeKey for TreeCacheKey {
    fn oid(&self) -> &ObjectId {
        &self.0
    }
    fn path(&self) -> &str {
        &self.1
    }
}

impl TreeKey for (&ObjectId, &str) {
    fn oid(&self) -> &ObjectId {
        self.0
    }
    fn path(&self) -> &str {
        self.1
    }
}

impl Hash for dyn TreeKey + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.oid().hash(state);
        self.path().hash(state);
    }
}

impl PartialEq for dyn TreeKey + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.oid() == other.oid() && self.path() == other.path()
    }
}

impl Eq for dyn TreeKey + '_ {}

impl<'a> Borrow<dyn TreeKey + 'a> for TreeCacheKey {
    fn borrow(&self) -> &(dyn TreeKey + 'a) {
        self
    }
}

/// Bounded LRU cache enforcing both an entry-count ceiling and a cumulative byte budget.
#[derive(Debug)]
struct ByteBudgetedLru<K: Hash + Eq, V> {
    lru: Option<CLruCache<K, (V, usize)>>,
    used_bytes: usize,
    max_bytes: usize,
}

impl<K: Hash + Eq, V: Clone> ByteBudgetedLru<K, V> {
    fn new(capacity: usize, max_bytes: usize) -> Self {
        let lru = NonZeroUsize::new(capacity)
            .filter(|_| max_bytes > 0)
            .map(CLruCache::new);
        Self {
            lru,
            used_bytes: 0,
            max_bytes,
        }
    }

    fn get<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.lru.as_mut()?.get(key).map(|(v, _)| v.clone())
    }

    fn put(&mut self, key: K, value: V, entry_bytes: usize) {
        let Some(ref mut lru) = self.lru else {
            return;
        };
        if let Some((_, old_bytes)) = lru.pop(&key) {
            self.used_bytes = self.used_bytes.saturating_sub(old_bytes);
        }
        if entry_bytes > self.max_bytes {
            return;
        }
        while !lru.is_empty()
            && (lru.len() >= lru.capacity() || self.used_bytes + entry_bytes > self.max_bytes)
        {
            if let Some((_, (_, evicted_bytes))) = lru.pop_back() {
                self.used_bytes = self.used_bytes.saturating_sub(evicted_bytes);
            } else {
                break;
            }
        }
        lru.put(key, (value, entry_bytes));
        self.used_bytes += entry_bytes;
    }

    fn reconfigure(&mut self, capacity: usize, max_bytes: usize) {
        self.max_bytes = max_bytes;
        let Some(non_zero_cap) = NonZeroUsize::new(capacity).filter(|_| max_bytes > 0) else {
            self.lru = None;
            self.used_bytes = 0;
            return;
        };
        if let Some(ref mut lru) = self.lru {
            while !lru.is_empty() && (lru.len() > capacity || self.used_bytes > self.max_bytes) {
                if let Some((_, (_, evicted_bytes))) = lru.pop_back() {
                    self.used_bytes = self.used_bytes.saturating_sub(evicted_bytes);
                } else {
                    break;
                }
            }
            lru.resize(non_zero_cap);
        } else {
            self.lru = Some(CLruCache::new(non_zero_cap));
            self.used_bytes = 0;
        }
    }

    fn clear(&mut self) {
        if let Some(ref mut lru) = self.lru {
            lru.clear();
        }
        self.used_bytes = 0;
    }
}

fn approx_diff_bytes(diff: &CommitDiff) -> usize {
    let mut bytes = 256 + diff.title.len() + diff.body.as_deref().map_or(0, str::len);
    for file in &diff.files {
        bytes += 160 + file.path.len();
        for hunk in &file.hunks {
            bytes += 96 + hunk.func_context.as_deref().map_or(0, str::len);
            for line in &hunk.lines {
                bytes += 48 + line.content.len();
            }
        }
    }
    bytes
}

fn approx_tree_bytes(tree: &TreeListing) -> usize {
    128 + tree.path.len() + tree.entries.len() * 96
}

fn approx_blob_bytes(blob: &BlobContent) -> usize {
    128 + blob.path.len() + blob.size + blob.lines.len() * 32
}

fn approx_raw_lines_bytes(lines: &[Arc<str>]) -> usize {
    64 + lines.iter().map(|s| 32 + s.len()).sum::<usize>()
}

fn approx_blame_bytes(blame: &BlameResult) -> usize {
    128 + blame.path.len() + blame.lines.len() * 240
}

/// Unified Git LRU cache holding diffs, trees, blobs, raw blob lines, and blames with fine-grained per-domain locks.
#[derive(Debug)]
pub struct GitLruCache {
    diffs: Mutex<ByteBudgetedLru<ObjectId, Arc<CommitDiff>>>,
    trees: Mutex<ByteBudgetedLru<TreeCacheKey, Arc<TreeListing>>>,
    blobs: Mutex<ByteBudgetedLru<ObjectId, Arc<BlobContent>>>,
    raw_blob_lines: Mutex<ByteBudgetedLru<ObjectId, Arc<[Arc<str>]>>>,
    blames: Mutex<ByteBudgetedLru<TreeCacheKey, Arc<BlameResult>>>,
}

impl Default for GitLruCache {
    fn default() -> Self {
        Self::new()
    }
}

impl GitLruCache {
    /// Creates a new `GitLruCache` initialized with baseline capacities (`DEFAULT_*_CACHE_CAPACITY`).
    #[must_use]
    pub fn new() -> Self {
        Self::with_profile(MemoryProfile::Lean)
    }

    /// Creates a new `GitLruCache` sized according to `profile`.
    #[must_use]
    pub fn with_profile(profile: MemoryProfile) -> Self {
        Self {
            diffs: Mutex::new(ByteBudgetedLru::new(
                profile.diff_cache_capacity(),
                profile.diff_cache_byte_budget(),
            )),
            trees: Mutex::new(ByteBudgetedLru::new(
                profile.tree_cache_capacity(),
                profile.tree_cache_byte_budget(),
            )),
            blobs: Mutex::new(ByteBudgetedLru::new(
                profile.blob_cache_capacity(),
                profile.blob_cache_byte_budget(),
            )),
            raw_blob_lines: Mutex::new(ByteBudgetedLru::new(
                profile.blob_cache_capacity(),
                profile.blob_cache_byte_budget(),
            )),
            blames: Mutex::new(ByteBudgetedLru::new(
                profile.blame_cache_capacity(),
                profile.blame_cache_byte_budget(),
            )),
        }
    }

    /// Reconfigures capacities and byte budgets in-place for a new [`MemoryProfile`].
    pub fn reconfigure(&self, profile: MemoryProfile) {
        self.diffs.lock().reconfigure(
            profile.diff_cache_capacity(),
            profile.diff_cache_byte_budget(),
        );
        self.trees.lock().reconfigure(
            profile.tree_cache_capacity(),
            profile.tree_cache_byte_budget(),
        );
        self.blobs.lock().reconfigure(
            profile.blob_cache_capacity(),
            profile.blob_cache_byte_budget(),
        );
        self.raw_blob_lines.lock().reconfigure(
            profile.blob_cache_capacity(),
            profile.blob_cache_byte_budget(),
        );
        self.blames.lock().reconfigure(
            profile.blame_cache_capacity(),
            profile.blame_cache_byte_budget(),
        );
    }

    /// Retrieves a cached `CommitDiff` if available.
    pub fn get_diff(&self, id: &ObjectId) -> Option<Arc<CommitDiff>> {
        self.diffs.lock().get(id)
    }

    /// Inserts a `CommitDiff` into the cache.
    pub fn insert_diff(&self, id: ObjectId, diff: Arc<CommitDiff>) {
        let bytes = approx_diff_bytes(&diff);
        self.diffs.lock().put(id, diff, bytes);
    }

    /// Retrieves a cached `TreeListing` if available without allocating a heap string for `path`.
    pub fn get_tree(&self, commit_oid: &ObjectId, path: &str) -> Option<Arc<TreeListing>> {
        let lookup = (commit_oid, path);
        self.trees.lock().get(&lookup as &dyn TreeKey)
    }

    /// Inserts a `TreeListing` into the cache.
    pub fn insert_tree(&self, commit_oid: ObjectId, path: &str, tree: Arc<TreeListing>) {
        let bytes = approx_tree_bytes(&tree);
        self.trees
            .lock()
            .put(TreeCacheKey(commit_oid, Box::from(path)), tree, bytes);
    }

    /// Retrieves a cached `BlobContent` if available.
    pub fn get_blob(&self, oid: &ObjectId) -> Option<Arc<BlobContent>> {
        self.blobs.lock().get(oid)
    }

    /// Inserts a `BlobContent` into the cache.
    pub fn insert_blob(&self, oid: ObjectId, blob: Arc<BlobContent>) {
        let bytes = approx_blob_bytes(&blob);
        self.blobs.lock().put(oid, blob, bytes);
    }

    /// Retrieves cached sanitized blob lines (`Arc<[Arc<str>]>`) used for diff context expansion.
    pub fn get_raw_blob_lines(&self, oid: &ObjectId) -> Option<Arc<[Arc<str>]>> {
        self.raw_blob_lines.lock().get(oid)
    }

    /// Inserts sanitized blob lines (`Arc<[Arc<str>]>`) into the cache.
    pub fn insert_raw_blob_lines(&self, oid: ObjectId, lines: Arc<[Arc<str>]>) {
        let bytes = approx_raw_lines_bytes(&lines);
        self.raw_blob_lines.lock().put(oid, lines, bytes);
    }

    /// Retrieves a cached `BlameResult` if available without allocating a heap string for `path`.
    pub fn get_blame(&self, commit_oid: &ObjectId, path: &str) -> Option<Arc<BlameResult>> {
        let lookup = (commit_oid, path);
        self.blames.lock().get(&lookup as &dyn TreeKey)
    }

    /// Inserts a `BlameResult` into the cache.
    pub fn insert_blame(&self, commit_oid: ObjectId, path: &str, blame: Arc<BlameResult>) {
        let bytes = approx_blame_bytes(&blame);
        self.blames
            .lock()
            .put(TreeCacheKey(commit_oid, Box::from(path)), blame, bytes);
    }

    /// Clears worktree-dependent caches while preserving content-addressed diffs, trees, blobs, and blames.
    pub fn purge_worktree(&self) {
        // Content-addressed items are preserved across worktree updates.
    }

    /// Clears all cached items unconditionally.
    pub fn clear(&self) {
        self.diffs.lock().clear();
        self.trees.lock().clear();
        self.blobs.lock().clear();
        self.raw_blob_lines.lock().clear();
        self.blames.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(byte: u8) -> ObjectId {
        ObjectId::from_bytes_or_panic(&[byte; 20])
    }

    fn blob(text: &str) -> Arc<BlobContent> {
        Arc::new(BlobContent {
            oid: oid(0),
            path: text.to_string(),
            size: text.len(),
            is_binary: false,
            lines: vec![text.to_string()].into(),
        })
    }

    /// The blob cache must evict strictly by recency of *access*, not of
    /// insertion: re-reading an entry has to protect it from the next eviction.
    #[test]
    fn test_cache_evicts_least_recently_used_entry() {
        let cache = GitLruCache::new();
        let cap = MemoryProfile::Lean.blob_cache_capacity();

        for i in 0..cap {
            cache.insert_blob(oid(u8::try_from(i).unwrap()), blob("x"));
        }

        // Touch the oldest entry so the second-oldest becomes the LRU victim.
        assert!(cache.get_blob(&oid(0)).is_some());

        let overflow = u8::try_from(cap).unwrap();
        cache.insert_blob(oid(overflow), blob("new"));

        assert!(
            cache.get_blob(&oid(0)).is_some(),
            "recently read entry was evicted"
        );
        assert!(
            cache.get_blob(&oid(1)).is_none(),
            "least recently used entry survived"
        );
        assert!(cache.get_blob(&oid(overflow)).is_some());
    }

    /// Re-inserting an existing key must update it in place rather than
    /// consuming another capacity slot.
    #[test]
    fn test_cache_reinsert_updates_in_place() {
        let cache = GitLruCache::new();
        let id = oid(7);

        cache.insert_blob(id, blob("first"));
        cache.insert_blob(id, blob("second"));

        let cached = cache.get_blob(&id).expect("entry present");
        assert_eq!(cached.path, "second");
    }

    /// Tree listings are keyed by both commit and path, so the same path under
    /// two different commits must not alias.
    #[test]
    fn test_tree_cache_is_keyed_by_commit_and_path() {
        let cache = GitLruCache::new();
        let listing = |name: &str| {
            Arc::new(TreeListing {
                commit_oid: oid(0),
                path: name.to_string(),
                parent_path: None,
                entries: Vec::new(),
            })
        };

        cache.insert_tree(oid(1), "src", listing("a"));
        cache.insert_tree(oid(2), "src", listing("b"));

        assert_eq!(cache.get_tree(&oid(1), "src").unwrap().path, "a");
        assert_eq!(cache.get_tree(&oid(2), "src").unwrap().path, "b");
        assert!(cache.get_tree(&oid(1), "docs").is_none());
    }

    #[test]
    fn test_git_lru_cache_content_addressed_persistence_and_clear() {
        let cache = GitLruCache::new();
        let oid1 = ObjectId::empty_tree(gix::hash::Kind::Sha1);

        let diff = Arc::new(CommitDiff {
            commit_id: oid1,
            parent_ids: Vec::new(),
            author_name: Arc::from("Alice"),
            author_email: Arc::from("alice@example.com"),
            author_date: "2026-09-14".to_string(),
            committer_name: Arc::from("Alice"),
            committer_email: Arc::from("alice@example.com"),
            committer_date: "2026-09-14".to_string(),
            title: Arc::from("Initial commit"),
            body: None,
            files: Vec::new(),
            stats: crate::diff::DiffSummaryStats::default(),
        });

        cache.insert_diff(oid1, diff.clone());
        assert!(cache.get_diff(&oid1).is_some());

        // Explicit clear must wipe entries
        cache.clear();
        assert!(cache.get_diff(&oid1).is_none());
    }
}
