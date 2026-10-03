// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Main commit log view with virtual scrolling and column layout.

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Color, SetAttribute};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tigrs_git::{CommitSummary, ObjectId, RefEntry, RefKind};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::graph::{COMMIT_COLOR_ANSI, CompactGraphRow, GraphRowBuilder, RESET_ANSI};
use crate::view::ViewportCursor;

/// Abbreviated object ID displayed for synthetic uncommitted-changes rows.
///
/// Matches upstream Tig, which renders the null object ID for these rows.
const CHANGES_SHORT_ID: &str = "0000000";

/// Author name displayed for synthetic uncommitted-changes rows.
///
/// Matches upstream Tig's `unknown_ident` (`src/util.c`).
const CHANGES_AUTHOR: &str = "Not Committed Yet";

/// History size above which the loading header reports a percentage instead of
/// an exact commit count.
///
/// A small history streams in faster than the eye can follow, so a running
/// count is both readable and informative there. Past this point the count is
/// just a blur of digits, and "how far along am I" is the only question worth
/// answering.
pub const LARGE_HISTORY_THRESHOLD: usize = 10_000;

/// Sentinel stored in [`MainView`]'s shared total meaning "not counted yet".
const TOTAL_UNKNOWN: usize = 0;

/// Category of an uncommitted-changes row shown above the commit history.
///
/// Ordering of the variants matches the top-to-bottom display order used by
/// upstream Tig: untracked, then unstaged, then staged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangesKind {
    /// Files present in the worktree but not tracked by Git.
    Untracked,
    /// Tracked files modified in the worktree but not staged.
    Unstaged,
    /// Changes staged in the index, ready to be committed.
    Staged,
}

impl ChangesKind {
    /// Returns the row title, matching upstream Tig's wording.
    #[inline]
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Untracked => "Untracked changes",
            Self::Unstaged => "Unstaged changes",
            Self::Staged => "Staged changes",
        }
    }
}

/// A synthetic main-view row summarizing one category of uncommitted changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangesRow {
    /// Which category of changes this row represents.
    pub kind: ChangesKind,
    /// Number of files in this category (always greater than zero for displayed rows).
    pub file_count: usize,
}

impl ChangesRow {
    /// Creates a changes row for `kind` covering `file_count` files.
    #[inline]
    #[must_use]
    pub fn new(kind: ChangesKind, file_count: usize) -> Self {
        Self { kind, file_count }
    }
}

/// A single logical row of the main view: either a synthetic changes row or a commit.
#[derive(Clone, Copy, Debug)]
pub enum MainRow<'a> {
    /// An uncommitted-changes row rendered above the commit history.
    Changes(&'a ChangesRow),
    /// A commit from the revision walk.
    Commit(&'a CommitSummary),
}

/// Stable identity of the selected row, captured before the changes prefix is
/// replaced so the cursor can be re-derived against the new row indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectionAnchor {
    /// The cursor was on the changes row of this kind.
    Changes(ChangesKind),
    /// The cursor was on the commit at this index into `commits`.
    Commit(usize),
}

/// Extracts a 64-bit hash from a cryptographic Git `ObjectId`.
#[inline]
fn oid_hash64(id: &ObjectId) -> u64 {
    let bytes = id.as_bytes();
    let mut buf = [0u8; 8];
    let len = bytes.len().min(8);
    buf[..len].copy_from_slice(&bytes[..len]);
    u64::from_ne_bytes(buf)
}

/// Checks whether a commit `ObjectId` matches a hex prefix string with zero heap allocation.
#[inline]
fn oid_matches_hex_prefix(oid: &ObjectId, hex_prefix: &str) -> bool {
    let bytes = oid.as_bytes();
    if hex_prefix.is_empty() || hex_prefix.len() > bytes.len() * 2 {
        return false;
    }
    for (i, ch) in hex_prefix.bytes().enumerate() {
        let nibble = match ch {
            b'0'..=b'9' => ch - b'0',
            b'a'..=b'f' => ch - b'a' + 10,
            b'A'..=b'F' => ch - b'A' + 10,
            _ => return false,
        };
        let byte = bytes[i / 2];
        let target_nibble = if i % 2 == 0 { byte >> 4 } else { byte & 0x0F };
        if nibble != target_nibble {
            return false;
        }
    }
    true
}

/// Bitflag: commit is reachable from `HEAD` (`P5`).
pub const FLAG_REACHABLE_FROM_HEAD: u8 = 1 << 0;
/// Bitflag: commit is ahead of upstream (`↑` unpushed, `P6`).
pub const FLAG_UNPUSHED: u8 = 1 << 1;
/// Bitflag: commit is on a non-HEAD branch not yet merged into upstream (`P6`).
pub const FLAG_UNMERGED: u8 = 1 << 2;
/// Bitflag: commit was authored by the current Git user (`"me"`, `P3`).
pub const FLAG_IS_ME: u8 = 1 << 3;
/// Bitflag: commit subject contains an issue/PR reference (`#123` or `GH-123`, `P9`).
pub const FLAG_HAS_ISSUE_REF: u8 = 1 << 4;

/// Precomputed semantic classification of a commit subject line (`P9` & `P11`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum SubjectRuleKind {
    /// Standard commit subject without special prefix.
    #[default]
    Normal = 0,
    /// `fixup!`, `squash!`, or `amend!` interactive rebase commit (`P9`).
    FixupSquash = 1,
    /// `Revert "..."` or `revert:` commit (`P9`).
    Revert = 2,
    /// `WIP`, `wip:`, or `DO NOT MERGE` commit (`P9`).
    Wip = 3,
    /// Conventional commit with breaking change marker `type(scope)!:` (`P9`).
    BreakingConventional = 4,
    /// Conventional commit prefix `type(scope):` or `type:` (`P9`).
    Conventional = 5,
    /// Merge commit boilerplate `Merge branch ...` / `Merge pull request ...` (`P11`).
    MergeCommit = 6,
}

/// Compact 4-byte per-commit highlight metadata stored in a parallel side-table in [`MainView`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommitHighlightMeta {
    /// Interned author ID resolved via [`MainView::author_entry`] ($O(1)$ integer equality for same-author spotlight).
    pub author_id: u16,
    /// Precomputed semantic subject rule classification (`P9` & `P11`).
    pub subject_kind: SubjectRuleKind,
    /// Bitset flags (`FLAG_REACHABLE_FROM_HEAD`, `FLAG_UNPUSHED`, `FLAG_UNMERGED`, `FLAG_IS_ME`, `FLAG_HAS_ISSUE_REF`).
    pub flags: u8,
}

/// Interned author metadata entry in [`MainView`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorEntry {
    /// Canonical author name.
    pub name: Arc<str>,
    /// Deterministic 0..9 hue index from [`fnv1a_author_hash`].
    pub hue_idx: u8,
    /// Total number of loaded commits by this author ($O(1)$ status bar readout in `P2`).
    pub commit_count: u32,
    /// Whether this author matches the current Git user (`P3`).
    pub is_me: bool,
}

/// Computes a deterministic 0..9 hue bucket index for `author` using 64-bit FNV-1a (`P1`).
#[must_use]
pub fn fnv1a_author_hash(author: &str) -> usize {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in author.trim().bytes() {
        h ^= u64::from(b.to_ascii_lowercase());
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    (h as usize) % 10
}

/// Maps a commit timestamp (`commit_time_secs`) relative to `now_secs` into one of 6 age heat buckets (`P7`):
/// `0`: `< 1h`, `1`: `< 24h` (today), `2`: `< 7d` (this week), `3`: `< 30d` (this month), `4`: `< 1y` (this year), `5`: `>= 1y` (older).
#[must_use]
pub const fn date_heat_bucket(commit_time_secs: i64, now_secs: i64) -> usize {
    let age = now_secs.saturating_sub(commit_time_secs);
    if age < 3_600 {
        0
    } else if age < 86_400 {
        1
    } else if age < 604_800 {
        2
    } else if age < 2_592_000 {
        3
    } else if age < 31_536_000 {
        4
    } else {
        5
    }
}

/// Classifies a commit subject line into a [`SubjectRuleKind`] and whether it contains a `#123` issue reference (`P9`, `P11`).
#[must_use]
pub fn classify_subject(summary: &str, is_merge: bool) -> (SubjectRuleKind, bool) {
    let trimmed = summary.trim_start();
    let has_issue = contains_issue_token(trimmed);

    if trimmed.starts_with("fixup!")
        || trimmed.starts_with("squash!")
        || trimmed.starts_with("amend!")
    {
        return (SubjectRuleKind::FixupSquash, has_issue);
    }
    if trimmed.starts_with("Revert \"") || trimmed.starts_with("revert:") {
        return (SubjectRuleKind::Revert, has_issue);
    }
    if trimmed.starts_with("WIP:")
        || trimmed.starts_with("WIP ")
        || trimmed == "WIP"
        || trimmed.starts_with("wip:")
        || trimmed.starts_with("DO NOT MERGE")
    {
        return (SubjectRuleKind::Wip, has_issue);
    }
    if is_merge
        || trimmed.starts_with("Merge branch ")
        || trimmed.starts_with("Merge pull request ")
        || trimmed.starts_with("Merge remote-tracking ")
        || trimmed.starts_with("Merge tag ")
    {
        return (SubjectRuleKind::MergeCommit, has_issue);
    }
    if let Some((prefix_len, is_breaking)) = conventional_prefix_len(trimmed)
        && prefix_len > 0
    {
        return (
            if is_breaking {
                SubjectRuleKind::BreakingConventional
            } else {
                SubjectRuleKind::Conventional
            },
            has_issue,
        );
    }
    (SubjectRuleKind::Normal, has_issue)
}

/// Returns `Some((colon_inclusive_len, is_breaking))` if `s` begins with a Conventional Commit prefix (`feat(scope)!:`).
#[must_use]
pub fn conventional_prefix_len(s: &str) -> Option<(usize, bool)> {
    const TYPES: &[&str] = &[
        "feat", "fix", "refactor", "perf", "docs", "test", "chore", "build", "ci", "style",
    ];
    let colon_pos = s.find(':')?;
    if colon_pos > 28 {
        return None;
    }
    let head = &s[..colon_pos];
    let (head_no_bang, is_breaking) = if let Some(stripped) = head.strip_suffix('!') {
        (stripped, true)
    } else {
        (head, false)
    };
    let type_part = if let Some(paren_idx) = head_no_bang.find('(') {
        if !head_no_bang.ends_with(')') {
            return None;
        }
        &head_no_bang[..paren_idx]
    } else {
        head_no_bang
    };
    if TYPES.iter().any(|&t| t.eq_ignore_ascii_case(type_part)) {
        Some((colon_pos + 1, is_breaking))
    } else {
        None
    }
}

fn contains_issue_token(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'#' && bytes[i + 1].is_ascii_digit() {
            return true;
        }
        i += 1;
    }
    false
}

/// State and layout coordinator for the main commit view.
pub struct MainView {
    /// Synthetic uncommitted-changes rows rendered *before* [`Self::commits`].
    ///
    /// Kept separate from `commits` so that `graph_rows` remains positionally
    /// index-aligned with `commits`; display indices are mapped through
    /// [`MainView::row`] and [`MainView::commit_index`].
    changes: Vec<ChangesRow>,
    commits: Vec<CommitSummary>,
    /// Compact 4-byte per-commit highlight metadata side-table aligned 1:1 with `commits`.
    highlight_meta: Vec<CommitHighlightMeta>,
    /// Interned author table indexed by `CommitHighlightMeta::author_id`.
    authors: Vec<AuthorEntry>,
    /// Fast lookup from author name to interned `author_id`.
    author_lookup: HashMap<Arc<str>, u16>,
    /// Current user name for `"me"` commit detection (`P3`).
    current_user_name: String,
    /// Current user email for `"me"` commit detection (`P3`).
    current_user_email: String,
    /// Explicit or ref-inferred `HEAD` commit ID (`P5`).
    head_commit_id: Option<ObjectId>,
    /// Explicit or ref-inferred upstream tracking commit ID (`P6`).
    upstream_commit_id: Option<ObjectId>,
    /// Rolling frontier of commit IDs reachable from `HEAD` (`P5`).
    reachable_frontier: std::collections::HashSet<ObjectId>,
    /// Set of commit IDs reachable from `upstream_commit_id` (`P6`).
    upstream_reachable: std::collections::HashSet<ObjectId>,
    /// Whether `HEAD` reachability was explicitly configured (`set_head_and_upstream` or multi-branch refs).
    has_explicit_reachability: bool,
    /// Cached ancestry lineage commit indices `(cursor_commit_idx, total_commits, lineage_set)` for `P2`.
    ancestry_cache: std::cell::RefCell<Option<(usize, usize, std::collections::HashSet<usize>)>>,
    /// Index-only hash table storing 4-byte `u32` indices into `commits` (`Phase A Item A3`).
    /// Achieves O(1) commit lookups without duplicating 20-byte `ObjectId` keys (~40 MB saved).
    commit_index_by_id: hashbrown::HashTable<u32>,
    /// 65,536-bucket head indices (`u32::MAX` = empty) keyed by the upper 16 bits (first 4 hex nibbles)
    /// of each commit OID for $O(1)$ [`MainView::unique_prefix_len`] lookup.
    prefix16_heads: Box<[u32]>,
    /// Intrusive next-commit index chain aligned 1:1 with `commits` for 16-bit OID prefix buckets.
    prefix16_next: Vec<u32>,
    graph_rows: Vec<CompactGraphRow>,
    graph_builder: GraphRowBuilder,
    ref_badges: HashMap<ObjectId, Vec<(String, Color)>>,
    /// Raw ref names per commit preserved even when co-located local+remote badges are merged (`P8`).
    raw_ref_names: HashMap<ObjectId, Vec<String>>,
    /// Selection cursor and vertical scroll viewport.
    nav: ViewportCursor,
    branch_name: String,
    is_loading: bool,
    /// Total commits the walk is expected to yield, or [`TOTAL_UNKNOWN`].
    ///
    /// Counting the history is far too slow to block the first frame on, so the
    /// total is filled in by a background thread through the handle returned by
    /// [`MainView::total_commits_handle`]. Shared rather than sent over a
    /// channel because it is write-once, read-during-render state that no other
    /// part of the app needs to react to.
    total_commits: Arc<AtomicUsize>,
    now_secs: i64,
    /// Optional 0-based initial target line from CLI `+N` argument, applied once enough rows stream in.
    initial_target_line: Option<usize>,
}

impl MainView {
    /// Creates a new main view with the specified active branch name.
    pub fn new(branch_name: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);

        let current_user_name = std::env::var("GIT_AUTHOR_NAME").unwrap_or_default();
        let current_user_email = std::env::var("GIT_AUTHOR_EMAIL").unwrap_or_default();

        Self {
            changes: Vec::new(),
            commits: Vec::with_capacity(1024),
            highlight_meta: Vec::with_capacity(1024),
            authors: Vec::with_capacity(64),
            author_lookup: HashMap::with_capacity(64),
            current_user_name,
            current_user_email,
            head_commit_id: None,
            upstream_commit_id: None,
            reachable_frontier: std::collections::HashSet::new(),
            upstream_reachable: std::collections::HashSet::new(),
            has_explicit_reachability: false,
            ancestry_cache: std::cell::RefCell::new(None),
            commit_index_by_id: hashbrown::HashTable::with_capacity(1024),
            prefix16_heads: vec![u32::MAX; 65_536].into_boxed_slice(),
            prefix16_next: Vec::with_capacity(1024),
            graph_rows: Vec::with_capacity(1024),
            graph_builder: GraphRowBuilder::new(),
            ref_badges: HashMap::new(),
            raw_ref_names: HashMap::new(),
            nav: ViewportCursor::new(),
            branch_name,
            is_loading: true,
            total_commits: Arc::new(AtomicUsize::new(TOTAL_UNKNOWN)),
            now_secs: now,
            initial_target_line: None,
        }
    }

    /// Sets a 0-based initial target line (from CLI `+N`), jumped to once enough commits stream in.
    pub fn set_initial_target_line(&mut self, line: usize) {
        self.initial_target_line = Some(line);
    }

    /// Configures the current Git user identity (`user.name` and `user.email`) for `"me"` commit highlighting (`P3`).
    pub fn set_current_user(&mut self, name: impl Into<String>, email: impl Into<String>) {
        self.current_user_name = name.into();
        self.current_user_email = email.into();
        let uname = self.current_user_name.trim();
        let uemail = self.current_user_email.trim();
        let email_prefix = uemail.split('@').next().unwrap_or("");
        for (author_idx, entry) in self.authors.iter_mut().enumerate() {
            let is_me = Self::matches_user(&entry.name, uname, uemail, email_prefix);
            entry.is_me = is_me;
            let aid = author_idx as u16;
            for meta in &mut self.highlight_meta {
                if meta.author_id == aid {
                    if is_me {
                        meta.flags |= FLAG_IS_ME;
                    } else {
                        meta.flags &= !FLAG_IS_ME;
                    }
                }
            }
        }
    }

    fn matches_user(author: &str, uname: &str, uemail: &str, email_prefix: &str) -> bool {
        let a = author.trim();
        if a.is_empty() {
            return false;
        }
        if !uname.is_empty() && a.eq_ignore_ascii_case(uname) {
            return true;
        }
        if !uemail.is_empty() && a.eq_ignore_ascii_case(uemail) {
            return true;
        }
        if !email_prefix.is_empty() && a.eq_ignore_ascii_case(email_prefix) {
            return true;
        }
        false
    }

    /// Configures `HEAD` and `upstream` (`origin/<branch>`) commit IDs for reachability dimming (`P5`)
    /// and push-status SHA highlighting (`P6`), recomputing flags across all loaded commits.
    pub fn set_head_and_upstream(
        &mut self,
        head_id: Option<ObjectId>,
        upstream_id: Option<ObjectId>,
    ) {
        self.head_commit_id = head_id;
        self.upstream_commit_id = upstream_id;
        self.has_explicit_reachability = head_id.is_some();
        self.recompute_reachability_and_push_status();
    }

    /// Returns the compact highlight metadata for the commit at `commit_idx`.
    #[must_use]
    pub fn highlight_meta(&self, commit_idx: usize) -> Option<CommitHighlightMeta> {
        self.highlight_meta.get(commit_idx).copied()
    }

    /// Returns the interned [`AuthorEntry`] for `author_id`.
    #[must_use]
    pub fn author_entry(&self, author_id: u16) -> Option<&AuthorEntry> {
        self.authors.get(author_id as usize)
    }

    /// Computes the shortest unique hex prefix length (`4..=7`) for `oid` among loaded commits (`P10`).
    #[must_use]
    pub fn unique_prefix_len(&self, oid: &ObjectId) -> usize {
        let target_bytes = oid.as_bytes();
        let p16 = usize::from(u16::from_be_bytes([target_bytes[0], target_bytes[1]]));
        let mut min_nibbles: usize = 4;
        let mut cur = self.prefix16_heads[p16];
        while cur != u32::MAX {
            let idx = cur as usize;
            cur = self.prefix16_next.get(idx).copied().unwrap_or(u32::MAX);
            let Some(c) = self.commits.get(idx) else {
                continue;
            };
            if c.id == *oid {
                continue;
            }
            let other_bytes = c.id.as_bytes();
            let mut common = 4;
            while common < 7 {
                let b1 = target_bytes[common / 2];
                let b2 = other_bytes[common / 2];
                let n1 = if common % 2 == 0 { b1 >> 4 } else { b1 & 0x0F };
                let n2 = if common % 2 == 0 { b2 >> 4 } else { b2 & 0x0F };
                if n1 == n2 {
                    common += 1;
                } else {
                    break;
                }
            }
            if common + 1 > min_nibbles {
                min_nibbles = (common + 1).min(7);
                if min_nibbles == 7 {
                    break;
                }
            }
        }
        min_nibbles
    }

    fn intern_author(&mut self, author_name: &Arc<str>) -> (u16, bool) {
        if let Some(&id) = self.author_lookup.get(author_name) {
            let idx = id as usize;
            self.authors[idx].commit_count = self.authors[idx].commit_count.saturating_add(1);
            return (id, self.authors[idx].is_me);
        }
        let id = u16::try_from(self.authors.len()).unwrap_or(u16::MAX);
        let hue_idx = fnv1a_author_hash(author_name) as u8;
        let uname = self.current_user_name.trim();
        let uemail = self.current_user_email.trim();
        let email_prefix = uemail.split('@').next().unwrap_or("");
        let is_me = Self::matches_user(author_name, uname, uemail, email_prefix);
        if (id as usize) < usize::from(u16::MAX) {
            self.authors.push(AuthorEntry {
                name: Arc::clone(author_name),
                hue_idx,
                commit_count: 1,
                is_me,
            });
            self.author_lookup.insert(Arc::clone(author_name), id);
        }
        (id, is_me)
    }

    fn recompute_reachability_and_push_status(&mut self) {
        self.reachable_frontier.clear();
        self.upstream_reachable.clear();

        let effective_head = self
            .head_commit_id
            .or_else(|| self.commits.first().map(|c| c.id));
        if let Some(hid) = effective_head {
            self.reachable_frontier.insert(hid);
        }
        if let Some(uid) = self.upstream_commit_id {
            self.upstream_reachable.insert(uid);
        }

        let has_upstream = self.upstream_commit_id.is_some();
        for (idx, commit) in self.commits.iter().enumerate() {
            let is_head_reach = self.reachable_frontier.remove(&commit.id);
            if is_head_reach {
                for &p in &commit.parents {
                    self.reachable_frontier.insert(p);
                }
            }
            let is_up_reach = self.upstream_reachable.remove(&commit.id);
            if is_up_reach {
                for &p in &commit.parents {
                    self.upstream_reachable.insert(p);
                }
            }

            if let Some(meta) = self.highlight_meta.get_mut(idx) {
                meta.flags &= !(FLAG_REACHABLE_FROM_HEAD | FLAG_UNPUSHED | FLAG_UNMERGED);
                if is_head_reach {
                    meta.flags |= FLAG_REACHABLE_FROM_HEAD;
                    if has_upstream && !is_up_reach {
                        meta.flags |= FLAG_UNPUSHED;
                    }
                } else if has_upstream && !is_up_reach {
                    meta.flags |= FLAG_UNMERGED;
                }
            }
        }
    }

    /// Returns a handle a background counter uses to publish the walk's total.
    ///
    /// Storing `TOTAL_UNKNOWN` (or never storing at all) simply leaves the
    /// header reporting an exact commit count, so a counter that fails or never
    /// finishes degrades to the previous behaviour rather than breaking.
    #[must_use]
    pub fn total_commits_handle(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.total_commits)
    }

    /// Publishes the walk's expected total commit count.
    pub fn set_total_commits(&self, total: usize) {
        self.total_commits.store(total, Ordering::Relaxed);
    }

    /// Returns the walk's expected total commit count once it is known.
    #[must_use]
    pub fn total_commits(&self) -> Option<usize> {
        match self.total_commits.load(Ordering::Relaxed) {
            TOTAL_UNKNOWN => None,
            total => Some(total),
        }
    }

    /// Returns loading progress as a percentage, when one is worth showing.
    #[must_use]
    pub fn load_percent(&self) -> Option<u8> {
        if !self.is_loading {
            return None;
        }
        let total = self.total_commits()?;
        if total < LARGE_HISTORY_THRESHOLD {
            return None;
        }
        let loaded = self.commits.len() as u64;
        let percent = loaded * 100 / total as u64;
        Some(u8::try_from(percent).unwrap_or(99).min(99))
    }

    /// Sets the repository ref badges mapping commit IDs to badge labels and colors,
    /// performing co-located local+remote branch deduplication (`P8`) and updating
    /// `HEAD` / `upstream` reachability (`P5`, `P6`).
    pub fn set_ref_badges(&mut self, refs: &[RefEntry]) {
        self.ref_badges.clear();
        self.raw_ref_names.clear();

        let mut inferred_head = None;
        let mut inferred_upstream = None;
        let expected_remote = format!("origin/{}", self.branch_name);
        let mut local_branches_count = 0usize;

        let mut by_commit: HashMap<ObjectId, Vec<&RefEntry>> = HashMap::new();
        for r in refs {
            self.raw_ref_names
                .entry(r.commit_id)
                .or_default()
                .push(r.name.clone());
            by_commit.entry(r.commit_id).or_default().push(r);
            match r.kind {
                RefKind::LocalBranch => {
                    local_branches_count += 1;
                    if r.name == self.branch_name {
                        inferred_head = Some(r.commit_id);
                    }
                }
                RefKind::RemoteBranch if r.name == expected_remote => {
                    inferred_upstream = Some(r.commit_id);
                }
                _ => {}
            }
        }

        for (cid, commit_refs) in by_commit {
            let mut merged_remotes = std::collections::HashSet::new();
            let mut badges = Vec::with_capacity(commit_refs.len());

            for r in &commit_refs {
                if r.kind == RefKind::LocalBranch {
                    // Check if there is a co-located remote tracking branch `<remote>/<r.name>` on this exact commit (`P8`)
                    let suffix = format!("/{}", r.name);
                    let matching_remote = commit_refs.iter().find(|cand| {
                        cand.kind == RefKind::RemoteBranch && cand.name.ends_with(&suffix)
                    });
                    if let Some(rem) = matching_remote {
                        let remote_host = &rem.name[..rem.name.len() - suffix.len()];
                        merged_remotes.insert(rem.full_name.as_str());
                        badges.push((format!("[{} ⇄ {remote_host}]", r.name), Color::Green));
                    } else {
                        badges.push((format!("[{}]", r.name), Color::Green));
                    }
                }
            }

            for r in &commit_refs {
                match r.kind {
                    RefKind::LocalBranch => {}
                    RefKind::RemoteBranch => {
                        if !merged_remotes.contains(r.full_name.as_str()) {
                            badges.push((format!("[{}]", r.name), Color::Red));
                        }
                    }
                    RefKind::Tag => badges.push((format!("<{}>", r.name), Color::Yellow)),
                    RefKind::Stash => badges.push(("{stash}".to_string(), Color::Magenta)),
                    RefKind::Other => badges.push((format!("({})", r.name), Color::Cyan)),
                }
            }
            self.ref_badges.insert(cid, badges);
        }

        if self.head_commit_id.is_none()
            && let Some(hid) = inferred_head
        {
            self.head_commit_id = Some(hid);
            if local_branches_count > 1 {
                self.has_explicit_reachability = true;
            }
        }
        if self.upstream_commit_id.is_none()
            && let Some(uid) = inferred_upstream
        {
            self.upstream_commit_id = Some(uid);
        }
        if self.head_commit_id.is_some() || self.upstream_commit_id.is_some() {
            self.recompute_reachability_and_push_status();
        }
    }

    /// Returns a map of commit IDs to plain reference names extracted from loaded ref badges.
    #[must_use]
    pub fn ref_names_by_commit(&self) -> HashMap<ObjectId, Vec<String>> {
        if !self.raw_ref_names.is_empty() {
            return self.raw_ref_names.clone();
        }
        self.ref_badges
            .iter()
            .map(|(&oid, badges)| {
                let names = badges
                    .iter()
                    .map(|(b, _)| {
                        b.trim_matches(|c| {
                            matches!(c, '[' | ']' | '<' | '>' | '(' | ')' | '{' | '}')
                        })
                        .to_string()
                    })
                    .collect();
                (oid, names)
            })
            .collect()
    }

    /// Appends a new batch of streamed commits to the view.
    pub fn append_commits(&mut self, batch: Vec<CommitSummary>) {
        if let Some(total) = self.total_commits()
            && self.commits.capacity() < total
        {
            let additional = total.saturating_sub(self.commits.len());
            self.commits.reserve(additional);
            self.highlight_meta.reserve(additional);
            self.prefix16_next.reserve(additional);
            self.graph_rows.reserve(additional);
            let commits_ref = &self.commits;
            self.commit_index_by_id
                .reserve(additional, |&existing_idx| {
                    oid_hash64(&commits_ref[existing_idx as usize].id)
                });
        }
        let base_idx = self.commits.len();
        if base_idx == 0
            && self.head_commit_id.is_none()
            && let Some(first) = batch.first()
        {
            self.reachable_frontier.insert(first.id);
        }
        let has_upstream = self.upstream_commit_id.is_some();

        for (offset, commit) in batch.iter().enumerate() {
            let commit_idx = (base_idx + offset) as u32;
            let is_merge = commit.is_merge();

            let graph_row = self
                .graph_builder
                .process_commit(commit.id, &commit.parents);
            self.graph_rows.push(graph_row);

            let (author_id, is_me) = self.intern_author(&commit.author_name);
            let (subject_kind, has_issue) = classify_subject(&commit.summary, is_merge);
            let is_head_reach = self.reachable_frontier.remove(&commit.id);
            if is_head_reach {
                for &p in &commit.parents {
                    self.reachable_frontier.insert(p);
                }
            }
            let is_up_reach = self.upstream_reachable.remove(&commit.id);
            if is_up_reach {
                for &p in &commit.parents {
                    self.upstream_reachable.insert(p);
                }
            }
            let mut flags = 0u8;
            if is_head_reach {
                flags |= FLAG_REACHABLE_FROM_HEAD;
                if has_upstream && !is_up_reach {
                    flags |= FLAG_UNPUSHED;
                }
            } else if has_upstream && !is_up_reach {
                flags |= FLAG_UNMERGED;
            }
            if is_me {
                flags |= FLAG_IS_ME;
            }
            if has_issue {
                flags |= FLAG_HAS_ISSUE_REF;
            }
            self.highlight_meta.push(CommitHighlightMeta {
                author_id,
                subject_kind,
                flags,
            });

            let oid_bytes = commit.id.as_bytes();
            let p16 = usize::from(u16::from_be_bytes([oid_bytes[0], oid_bytes[1]]));
            let prev_head = self.prefix16_heads[p16];
            self.prefix16_next.push(prev_head);
            self.prefix16_heads[p16] = commit_idx;

            let hash = oid_hash64(&commit.id);
            let commits_slice = &self.commits;
            let batch_slice = &batch[..offset];
            self.commit_index_by_id
                .insert_unique(hash, commit_idx, |&existing_idx| {
                    let existing_usize = existing_idx as usize;
                    let existing_oid = if existing_usize < commits_slice.len() {
                        &commits_slice[existing_usize].id
                    } else {
                        &batch_slice[existing_usize - base_idx].id
                    };
                    oid_hash64(existing_oid)
                });
        }
        self.commits.extend(batch);
        *self.ancestry_cache.borrow_mut() = None;
        if let Some(target_line) = self.initial_target_line {
            let len = self.changes.len() + self.commits.len();
            if len > target_line {
                self.nav.set_cursor(target_line, len, 24);
                self.initial_target_line = None;
            }
        }
    }

    /// Looks up a commit index in $O(1)$ time using the index-only hash table.
    #[inline]
    #[must_use]
    pub fn commit_index_for_id(&self, target_id: &ObjectId) -> Option<usize> {
        let hash = oid_hash64(target_id);
        self.commit_index_by_id
            .find(hash, |&idx| {
                self.commits
                    .get(idx as usize)
                    .is_some_and(|c| c.id == *target_id)
            })
            .map(|&idx| idx as usize)
    }

    /// Maps a commit `ObjectId` directly to its display row index in $O(1)$ time.
    #[inline]
    #[must_use]
    pub fn row_for_commit_id(&self, target_id: &ObjectId) -> Option<usize> {
        self.commit_index_for_id(target_id)
            .map(|idx| self.row_for_commit(idx))
    }

    /// Finds the display row index of the first commit matching a hex prefix without heap allocation.
    #[must_use]
    pub fn find_row_by_hex_prefix(&self, hex_prefix: &str) -> Option<usize> {
        self.commits
            .iter()
            .position(|c| oid_matches_hex_prefix(&c.id, hex_prefix))
            .map(|idx| self.row_for_commit(idx))
    }

    /// Trims unused heap capacity across commit, index, and graph buffers.
    pub fn shrink_to_fit(&mut self) {
        self.commits.shrink_to_fit();
        self.prefix16_next.shrink_to_fit();
        self.commit_index_by_id.shrink_to_fit(|&idx| {
            self.commits
                .get(idx as usize)
                .map_or(0, |c| oid_hash64(&c.id))
        });
        self.graph_rows.shrink_to_fit();
        self.graph_builder.shrink_to_fit();
    }

    /// Marks that commit streaming has finished.
    pub fn set_finished(&mut self) {
        self.is_loading = false;
        if let Some(target_line) = self.initial_target_line.take() {
            let len = self.changes.len() + self.commits.len();
            if len > 0 {
                self.nav.set_cursor(target_line.min(len - 1), len, 24);
            }
        }
    }

    /// Returns the total number of loaded commits.
    #[inline]
    pub fn commit_count(&self) -> usize {
        self.commits.len()
    }

    /// Returns the slice of commit summaries currently in the view.
    #[inline]
    pub fn commits(&self) -> &[CommitSummary] {
        &self.commits
    }

    /// Returns the synthetic uncommitted-changes rows shown above the history.
    #[inline]
    #[must_use]
    pub fn changes(&self) -> &[ChangesRow] {
        &self.changes
    }

    /// Returns the number of synthetic rows preceding the first commit.
    #[inline]
    #[must_use]
    pub fn changes_len(&self) -> usize {
        self.changes.len()
    }

    /// Returns the total number of displayed rows (changes rows plus commits).
    #[inline]
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.changes.len() + self.commits.len()
    }

    /// Returns the logical row at display index `idx`, if it exists.
    #[must_use]
    pub fn row(&self, idx: usize) -> Option<MainRow<'_>> {
        if let Some(change) = self.changes.get(idx) {
            return Some(MainRow::Changes(change));
        }
        self.commits
            .get(idx - self.changes.len())
            .map(MainRow::Commit)
    }

    /// Returns the logical row under the cursor, if any.
    #[inline]
    #[must_use]
    pub fn selected_row(&self) -> Option<MainRow<'_>> {
        self.row(self.nav.cursor)
    }

    /// Maps a display row index to an index into [`Self::commits`].
    ///
    /// Returns `None` for synthetic changes rows.
    #[inline]
    #[must_use]
    pub fn commit_index(&self, row_idx: usize) -> Option<usize> {
        row_idx
            .checked_sub(self.changes.len())
            .filter(|i| *i < self.commits.len())
    }

    /// Maps an index into [`Self::commits`] to its display row index.
    #[inline]
    #[must_use]
    pub fn row_for_commit(&self, commit_idx: usize) -> usize {
        self.changes.len() + commit_idx
    }

    /// Replaces the uncommitted-changes rows, preserving the current selection.
    ///
    /// The cursor follows the same logical content across the update: a selected
    /// commit stays selected even though its display index shifts, and a selected
    /// changes section stays selected as long as it still exists. The viewport is
    /// shifted by the same delta so the screen does not jump.
    pub fn set_changes(&mut self, changes: Vec<ChangesRow>) {
        if self.changes == changes {
            return;
        }

        // A view pinned to the very top stays pinned, so rows inserted above the
        // history become visible instead of being scrolled off. This also makes
        // the first changes row the initial selection, matching upstream Tig.
        if self.nav.cursor == 0 && self.nav.scroll_offset == 0 {
            self.changes = changes;
            self.clamp_viewport();
            return;
        }

        let anchor = self.selection_anchor();
        let old_prefix = self.changes.len();
        self.changes = changes;
        let new_prefix = self.changes.len();

        self.nav.cursor = match anchor {
            Some(SelectionAnchor::Commit(commit_idx)) => new_prefix + commit_idx,
            // If the selected section disappeared, fall through to the first commit.
            Some(SelectionAnchor::Changes(kind)) => self
                .changes
                .iter()
                .position(|c| c.kind == kind)
                .unwrap_or(new_prefix),
            None => 0,
        };

        self.nav.scroll_offset = if new_prefix >= old_prefix {
            self.nav.scroll_offset + (new_prefix - old_prefix)
        } else {
            self.nav
                .scroll_offset
                .saturating_sub(old_prefix - new_prefix)
        };

        self.clamp_viewport();
    }

    /// Captures the stable identity of the selected row before a prefix update.
    fn selection_anchor(&self) -> Option<SelectionAnchor> {
        match self.row(self.nav.cursor)? {
            MainRow::Changes(c) => Some(SelectionAnchor::Changes(c.kind)),
            MainRow::Commit(_) => Some(SelectionAnchor::Commit(
                self.nav.cursor - self.changes.len(),
            )),
        }
    }

    /// Clamps the cursor and scroll offset to the current row count.
    fn clamp_viewport(&mut self) {
        let max_idx = self.row_count().saturating_sub(1);
        self.nav.cursor = self.nav.cursor.min(max_idx);
        self.nav.scroll_offset = self.nav.scroll_offset.min(max_idx);
    }

    /// Returns the currently selected commit summary.
    ///
    /// Returns `None` when the cursor rests on a synthetic changes row, which
    /// lets commit-oriented actions fail safe rather than acting on the wrong
    /// commit.
    pub fn selected_commit(&self) -> Option<&CommitSummary> {
        self.commit_index(self.nav.cursor)
            .and_then(|i| self.commits.get(i))
    }

    /// Returns the index of the currently selected row.
    #[inline]
    pub fn cursor_index(&self) -> usize {
        self.nav.cursor
    }

    /// Explicitly sets the selection cursor and adjusts scroll viewport.
    pub fn set_cursor(&mut self, cursor: usize, visible_height: usize) {
        let total = self.row_count();
        self.nav.set_cursor(cursor, total, visible_height);
    }

    /// Moves the selection cursor down by `n` rows, updating the scroll viewport.
    pub fn move_down(&mut self, n: usize, visible_height: usize) {
        let total = self.row_count();
        self.nav.move_down_by(n, total, visible_height);
    }

    /// Moves the selection cursor up by `n` rows, updating the scroll viewport.
    pub fn move_up(&mut self, n: usize, visible_height: usize) {
        self.nav.move_up_by(n, visible_height);
    }

    /// Moves the selection down by one screen page.
    pub fn page_down(&mut self, visible_height: usize) {
        let step = visible_height.saturating_sub(2).max(1);
        self.move_down(step, visible_height);
    }

    /// Moves the selection up by one screen page.
    pub fn page_up(&mut self, visible_height: usize) {
        let step = visible_height.saturating_sub(2).max(1);
        self.move_up(step, visible_height);
    }

    /// Jumps directly to the very first row (top of the view).
    pub fn home(&mut self) {
        self.nav.scroll_top();
    }

    /// Jumps directly to the very last displayed row.
    pub fn end(&mut self, visible_height: usize) {
        let total = self.row_count();
        self.nav.scroll_bottom(total, visible_height);
    }

    /// Returns the current vertical scroll offset.
    #[inline]
    #[must_use]
    pub fn scroll_offset(&self) -> usize {
        self.nav.scroll_offset
    }

    /// Scrolls the viewport down by `n` lines, clamping to available rows
    /// and keeping the cursor within the visible viewport.
    pub fn scroll_line_down(&mut self, n: usize, visible_height: usize) {
        let total = self.row_count();
        self.nav.scroll_line_down(n, total, visible_height);
    }

    /// Scrolls the viewport up by `n` lines, clamping to 0
    /// and keeping the cursor within the visible viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.nav.scroll_line_up(n, visible_height);
    }

    /// Computes (or returns cached) set of commit indices belonging to the selected commit's
    /// DAG ancestry or descendant lineage (`P2` `MainSpotlight::Ancestry`).
    fn ancestry_lineage_set(&self, selected_commit_idx: usize) -> std::collections::HashSet<usize> {
        let total = self.commits.len();
        if let Some((cached_cursor, cached_total, ref set)) = *self.ancestry_cache.borrow()
            && cached_cursor == selected_commit_idx
            && cached_total == total
        {
            return set.clone();
        }

        let mut set = std::collections::HashSet::new();
        let Some(selected_commit) = self.commits.get(selected_commit_idx) else {
            return set;
        };
        set.insert(selected_commit_idx);

        // 1. Walk downward through ancestors (commit_idx >= selected_commit_idx in topological order, or via O(1) commit_index_for_id)
        let mut ancestor_oids = std::collections::HashSet::new();
        for &p in &selected_commit.parents {
            ancestor_oids.insert(p);
        }
        for idx in selected_commit_idx..total {
            let c = &self.commits[idx];
            if idx == selected_commit_idx || ancestor_oids.contains(&c.id) {
                set.insert(idx);
                for &p in &c.parents {
                    ancestor_oids.insert(p);
                }
            }
        }

        // 2. Walk upward through descendants (0..selected_commit_idx) in reverse order
        let mut descendant_oids = std::collections::HashSet::new();
        descendant_oids.insert(selected_commit.id);
        for idx in (0..selected_commit_idx).rev() {
            let c = &self.commits[idx];
            if c.parents.iter().any(|p| descendant_oids.contains(p)) {
                set.insert(idx);
                descendant_oids.insert(c.id);
            }
        }

        *self.ancestry_cache.borrow_mut() = Some((selected_commit_idx, total, set.clone()));
        set
    }

    /// Renders the entire view onto the provided buffered writer using default options.
    pub fn render<W: Write>(&self, w: &mut W, width: u16, height: u16) -> std::io::Result<()> {
        self.render_with_options(w, width, height, &crate::options::ViewOptions::default())
    }

    /// Renders the entire view onto the provided buffered writer using specified options.
    pub fn render_with_options<W: Write>(
        &self,
        w: &mut W,
        width: u16,
        height: u16,
        options: &crate::options::ViewOptions,
    ) -> std::io::Result<()> {
        if width < 10 || height < 3 {
            return Ok(());
        }

        let width = width as usize;
        let height = height as usize;
        let content_height = height.saturating_sub(2); // 1 line for header, 1 for status bar
        let palette = options.ui_palette();

        // 1. Render Header Line
        queue!(w, MoveTo(0, 0), SetAttribute(Attribute::Bold))?;
        let header_text = if let Some(percent) = self.load_percent() {
            format!(" [main] {} - {percent}% commits loaded", self.branch_name)
        } else {
            format!(
                " [main] {} - {} commits loaded{}",
                self.branch_name,
                self.commits.len(),
                if self.is_loading { " (loading...)" } else { "" }
            )
        };
        write_padded_line(w, &header_text, width, false)?;
        queue!(w, SetAttribute(Attribute::Reset))?;

        // Precompute spotlight state (`P2`)
        let selected_commit_idx = self.commit_index(self.nav.cursor);
        let selected_author_id = selected_commit_idx
            .and_then(|i| self.highlight_meta.get(i))
            .map(|m| m.author_id);
        let ancestry_set = if options.main_spotlight
            == tigrs_core::config_enums::MainSpotlight::Ancestry
            && let Some(sel_idx) = selected_commit_idx
        {
            Some(self.ancestry_lineage_set(sel_idx))
        } else {
            None
        };

        // 2. Render Virtual Viewport Rows
        if self.row_count() == 0 {
            for row in 0..content_height {
                queue!(w, MoveTo(0, (row + 1) as u16))?;
                if row == 0 {
                    let msg = if self.is_loading {
                        "  Loading commits..."
                    } else {
                        "  No commits found."
                    };
                    write_padded_line(w, msg, width, false)?;
                } else {
                    write_padded_line(w, "", width, false)?;
                }
            }
        } else {
            let mut line_buf = String::with_capacity(width + 64);
            for row in 0..content_height {
                let row_idx = self.nav.scroll_offset + row;
                queue!(w, MoveTo(0, (row + 1) as u16))?;

                match self.row(row_idx) {
                    Some(main_row) => {
                        self.render_row(
                            w,
                            main_row,
                            row_idx,
                            width,
                            options,
                            &palette,
                            selected_author_id,
                            ancestry_set.as_ref(),
                            &mut line_buf,
                        )?;
                    }
                    None => write_padded_line(w, "", width, false)?,
                }
            }
        }

        // 3. Render Status Bar (`P2` Spotlight readout)
        queue!(
            w,
            MoveTo(0, (height - 1) as u16),
            SetAttribute(Attribute::Reverse)
        )?;
        let row_count = self.row_count();
        let status_pct = ((self.nav.cursor + 1) * 100)
            .checked_div(row_count)
            .unwrap_or(100);
        let spotlight_suffix = match options.main_spotlight {
            tigrs_core::config_enums::MainSpotlight::Off => String::new(),
            tigrs_core::config_enums::MainSpotlight::Author => {
                if let Some(aid) = selected_author_id
                    && let Some(entry) = self.author_entry(aid)
                {
                    format!(
                        " | Spotlight: {} ({} of {} loaded)",
                        entry.name,
                        entry.commit_count,
                        self.commits.len()
                    )
                } else {
                    String::new()
                }
            }
            tigrs_core::config_enums::MainSpotlight::Ancestry => {
                let count = ancestry_set
                    .as_ref()
                    .map_or(0, std::collections::HashSet::len);
                format!(
                    " | Spotlight: Ancestry ({count} of {} loaded)",
                    self.commits.len()
                )
            }
        };
        let status_text = format!(
            " [{}] line {} of {} ({}%){} - press 'q' to quit, 'j'/'k' to move",
            self.branch_name,
            if row_count == 0 {
                0
            } else {
                self.nav.cursor + 1
            },
            row_count,
            status_pct,
            spotlight_suffix
        );
        write_padded_line(w, &status_text, width, true)?;
        queue!(w, SetAttribute(Attribute::Reset))?;

        Ok(())
    }

    /// Renders one content row (a changes row or a commit) as a full padded line.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn render_row<W: Write>(
        &self,
        w: &mut W,
        main_row: MainRow<'_>,
        row_idx: usize,
        width: usize,
        options: &crate::options::ViewOptions,
        palette: &crate::ui_theme::UiPalette,
        selected_author_id: Option<u16>,
        ancestry_set: Option<&std::collections::HashSet<usize>>,
        line: &mut String,
    ) -> std::io::Result<()> {
        let is_selected = row_idx == self.nav.cursor;
        let use_color = !options.no_color;
        // `P4`: When a named theme (or explicit cursor bg) is active, use a colored background
        // cursor bar so per-column foreground colors remain vibrant on the selected row!
        let use_colored_cursor = is_selected && use_color && palette.cursor_row.bg.is_some();

        let mut row_base_sgr = String::new();
        if is_selected {
            if use_colored_cursor {
                row_base_sgr.push_str(RESET_ANSI);
                if let Some(bg) = palette.cursor_row.bg {
                    crate::ui_theme::UiPalette::write_bg_sgr(&mut row_base_sgr, bg);
                }
                row_base_sgr.push_str("\x1b[1m");
                if let Some(fg) = palette.cursor_row.fg {
                    crate::ui_theme::UiPalette::write_fg_sgr(&mut row_base_sgr, fg);
                }
            } else {
                queue!(w, SetAttribute(Attribute::Reverse))?;
            }
        }
        let reset_sgr = if use_colored_cursor {
            row_base_sgr.as_str()
        } else if is_selected {
            "\x1b[0;7m"
        } else {
            RESET_ANSI
        };
        let paint_columns = use_color
            && (!is_selected
                || (use_colored_cursor
                    && !matches!(
                        palette.category,
                        crate::ui_theme::ThemeCategory::HighContrastDark
                            | crate::ui_theme::ThemeCategory::HighContrastLight
                    )));

        let commit_idx = self.commit_index(row_idx);
        let meta = commit_idx
            .and_then(|i| self.highlight_meta.get(i))
            .copied()
            .unwrap_or_default();
        let author_entry = self.author_entry(meta.author_id);

        // Determine Spotlight (`P2`) and Reachability (`P5`) state for this row
        let (is_spotlight_match, is_spotlight_dimmed) = match options.main_spotlight {
            tigrs_core::config_enums::MainSpotlight::Off => (false, false),
            tigrs_core::config_enums::MainSpotlight::Author => {
                let matched = commit_idx.is_some() && selected_author_id == Some(meta.author_id);
                (matched, !matched && options.main_spotlight_dim_others)
            }
            tigrs_core::config_enums::MainSpotlight::Ancestry => {
                let matched = commit_idx
                    .and_then(|i| ancestry_set.map(|s| s.contains(&i)))
                    .unwrap_or(false);
                (matched, !matched && options.main_spotlight_dim_others)
            }
        };
        let is_unreachable_dimmed = commit_idx.is_some()
            && options.main_dim_unreachable
            && self.has_explicit_reachability
            && (meta.flags & FLAG_REACHABLE_FROM_HEAD) == 0;
        let is_row_dimmed = (is_spotlight_dimmed || is_unreachable_dimmed) && !is_selected;

        let fields = row_fields(main_row, self.now_secs);
        line.clear();
        if use_colored_cursor {
            line.push_str(&row_base_sgr);
        }

        // `P2`: Left-edge spotlight gutter indicator (`▎` in UTF-8, `|` in ASCII)
        if options.main_spotlight != tigrs_core::config_enums::MainSpotlight::Off {
            if is_spotlight_match {
                let bar = match options.line_graphics {
                    crate::options::LineGraphics::Utf8 => "▎",
                    crate::options::LineGraphics::Ascii => "|",
                };
                if paint_columns {
                    let bar_color = author_entry.map_or(palette.ref_head_fg, |e| {
                        palette.author_hues[e.hue_idx as usize % 10]
                    });
                    crate::ui_theme::UiPalette::write_fg_sgr(line, bar_color);
                    line.push_str(bar);
                    line.push_str(reset_sgr);
                } else {
                    line.push_str(bar);
                }
            } else {
                line.push(' ');
            }
        }

        if is_row_dimmed {
            line.push_str("\x1b[2m");
        }

        // Optional Commit ID (`P0`, `P6` Push-Status, `P10` Unique Prefix)
        if options.commit_id {
            let is_unpushed = options.main_push_status && (meta.flags & FLAG_UNPUSHED) != 0;
            let is_unmerged = options.main_push_status && (meta.flags & FLAG_UNMERGED) != 0;
            let sha_color = if is_unpushed {
                palette.sha_unpushed_fg
            } else if is_unmerged {
                palette.sha_unmerged_fg
            } else {
                palette.commit_id_fg
            };
            let arrow = if is_unpushed {
                match options.line_graphics {
                    crate::options::LineGraphics::Utf8 => "↑",
                    crate::options::LineGraphics::Ascii => "^",
                }
            } else {
                ""
            };
            if let MainRow::Commit(commit) = main_row
                && options.main_unique_prefix
                && paint_columns
                && !is_selected
            {
                let uniq_len = self
                    .unique_prefix_len(&commit.id)
                    .min(fields.short_id.len());
                let (uniq_part, tail_part) = fields.short_id.split_at(uniq_len);
                if paint_columns {
                    crate::ui_theme::UiPalette::write_fg_sgr(line, sha_color);
                }
                line.push_str("\x1b[1m");
                line.push_str(arrow);
                line.push_str(uniq_part);
                line.push_str("\x1b[22;2m");
                line.push_str(tail_part);
                line.push_str(reset_sgr);
                let used_w = UnicodeWidthStr::width(arrow) + fields.short_id.len();
                for _ in used_w..8 {
                    line.push(' ');
                }
                if is_row_dimmed {
                    line.push_str("\x1b[2m");
                }
            } else {
                let display_id = if arrow.is_empty() {
                    fields.short_id.clone()
                } else {
                    format!("{arrow}{}", fields.short_id)
                };
                if paint_columns {
                    crate::ui_theme::UiPalette::write_fg_sgr(line, sha_color);
                }
                append_column(line, &display_id, 8);
                if paint_columns && options.date_format == crate::options::DateFormat::Off {
                    line.push_str(reset_sgr);
                }
            }
        }

        // Date column (`P0` & `P7` 6-Bucket Relative Date Heatmap)
        if options.date_format != crate::options::DateFormat::Off {
            if paint_columns && !is_row_dimmed {
                let date_color = if options.main_date_heat {
                    let bucket = date_heat_bucket(fields.time_secs, self.now_secs);
                    palette.date_heat[bucket]
                } else {
                    palette.date_fg
                };
                crate::ui_theme::UiPalette::write_fg_sgr(line, date_color);
            }
            match options.date_format {
                crate::options::DateFormat::Relative => {
                    let date_str = tigrs_git::format_relative_date(fields.time_secs, self.now_secs);
                    append_column(line, &date_str, 10);
                    line.push(' ');
                }
                crate::options::DateFormat::Short => {
                    let date_str = crate::options::format_timestamp(fields.time_secs, false);
                    append_column(line, &date_str, 10);
                    line.push(' ');
                }
                crate::options::DateFormat::Iso => {
                    let date_str = crate::options::format_timestamp(fields.time_secs, true);
                    append_column(line, &date_str, 16);
                    line.push(' ');
                }
                crate::options::DateFormat::Off => {}
            }
            if paint_columns
                && !is_row_dimmed
                && options.author_format == crate::options::AuthorFormat::Off
            {
                line.push_str(reset_sgr);
            }
        }

        // Author column (`P0`, `P1` Deterministic 10-Hue Hash, `P3` "me" Marker)
        if options.author_format != crate::options::AuthorFormat::Off {
            let is_me = (meta.flags & FLAG_IS_ME) != 0
                && options.main_author_color != tigrs_core::config_enums::MainAuthorColor::Off;
            if paint_columns && !is_row_dimmed {
                let ac = Self::resolve_author_color(fields.author, author_entry, options, palette);
                crate::ui_theme::UiPalette::write_fg_sgr(line, ac);
            }
            if paint_columns && (is_me || is_spotlight_match) {
                line.push_str("\x1b[1m");
            }
            let raw_author = match options.author_format {
                crate::options::AuthorFormat::Full => std::borrow::Cow::Borrowed(fields.author),
                crate::options::AuthorFormat::Abbreviated => {
                    std::borrow::Cow::Owned(crate::options::abbreviate_author(fields.author))
                }
                crate::options::AuthorFormat::Email => {
                    std::borrow::Cow::Owned(crate::options::author_email_prefix(fields.author))
                }
                crate::options::AuthorFormat::Off => std::borrow::Cow::Borrowed(""),
            };
            if is_me {
                let me_glyph = match options.line_graphics {
                    crate::options::LineGraphics::Utf8 => "● ",
                    crate::options::LineGraphics::Ascii => "* ",
                };
                let decorated = format!("{me_glyph}{raw_author}");
                append_column(line, &decorated, 15);
            } else {
                append_column(line, &raw_author, 15);
            }
            let author_wrote_sgr = paint_columns && (!is_row_dimmed || is_me || is_spotlight_match);
            // Coalesce SGR reset when unselected default graph column immediately follows
            let graph_follows_with_ansi = !is_selected
                && paint_columns
                && !is_row_dimmed
                && !is_me
                && !is_spotlight_match
                && options.commit_title_graph != crate::options::GraphDisplay::No
                && palette.id == tigrs_core::config_enums::UiThemeId::Default;
            if !graph_follows_with_ansi && author_wrote_sgr {
                line.push_str(reset_sgr);
                if is_row_dimmed {
                    line.push_str("\x1b[2m");
                }
            }
            line.push(' ');
        }

        // Graph marker with DAG lanes (`P0`)
        if options.commit_title_graph != crate::options::GraphDisplay::No {
            self.append_graph_column(
                line,
                main_row,
                row_idx,
                is_selected && !use_colored_cursor,
                !paint_columns,
                options,
                palette,
                reset_sgr,
            );
            if is_row_dimmed {
                line.push_str("\x1b[2m");
            }
            line.push(' ');
        }

        // `P6`: Compact unpushed `↑` indicator when `id` column is hidden
        if !options.commit_id && options.main_push_status && (meta.flags & FLAG_UNPUSHED) != 0 {
            let up_glyph = match options.line_graphics {
                crate::options::LineGraphics::Utf8 => "↑",
                crate::options::LineGraphics::Ascii => "^",
            };
            if paint_columns {
                crate::ui_theme::UiPalette::write_fg_sgr(line, palette.sha_unpushed_fg);
                line.push_str("\x1b[1m");
                line.push_str(up_glyph);
                line.push_str(reset_sgr);
                if is_row_dimmed {
                    line.push_str("\x1b[2m");
                }
            } else {
                line.push_str(up_glyph);
            }
            line.push(' ');
        }

        // Ref badges (`P0` & `P8` Co-Located Ref Merging, e.g. `[main ⇄ origin]`, `<v1.0>`)
        if let MainRow::Commit(commit) = main_row
            && options.commit_title_refs
            && let Some(badges) = self.ref_badges.get(&commit.id)
        {
            for (badge, color) in badges {
                if paint_columns {
                    let badge_fg = match color {
                        Color::Red | Color::DarkRed => palette.ref_remote_fg,
                        Color::Yellow | Color::DarkYellow => palette.ref_tag_fg,
                        Color::Magenta | Color::DarkMagenta => palette.ref_stash_fg,
                        Color::Cyan | Color::DarkCyan => palette.ref_head_fg,
                        _ => palette.ref_branch_fg,
                    };
                    crate::ui_theme::UiPalette::write_fg_sgr(line, badge_fg);
                    line.push_str(badge);
                    line.push_str(reset_sgr);
                    if is_row_dimmed {
                        line.push_str("\x1b[2m");
                    }
                } else {
                    line.push_str(badge);
                }
                line.push(' ');
            }
        }

        let prefix_width = tigrs_core::ansi::visible_width(line);
        let remaining_width = width.saturating_sub(prefix_width);
        Self::append_styled_subject(
            line,
            fields.title,
            remaining_width,
            options,
            palette,
            meta,
            is_selected && !use_colored_cursor,
            !paint_columns,
            matches!(main_row, MainRow::Changes(_)),
            is_row_dimmed,
            reset_sgr,
        );
        if is_row_dimmed && !line.ends_with(RESET_ANSI) {
            line.push_str(RESET_ANSI);
        }

        write_padded_line(w, line, width, is_selected)?;

        if is_selected {
            queue!(w, SetAttribute(Attribute::Reset))?;
        }

        Ok(())
    }

    fn resolve_author_color(
        author: &str,
        entry: Option<&AuthorEntry>,
        options: &crate::options::ViewOptions,
        palette: &crate::ui_theme::UiPalette,
    ) -> crate::headless::Color {
        // 1. Check explicit `[colors.authors]` override first (`P1`)
        if !options.author_colors.is_empty() {
            for (key, spec_str) in &options.author_colors {
                if author.eq_ignore_ascii_case(key) || author.contains(key.as_str()) {
                    let first_tok = spec_str.split_whitespace().next().unwrap_or(spec_str);
                    if let Some(c) = crate::diff::style_from_spec(&tigrs_core::ColorSpec::String(
                        first_tok.to_string(),
                    ))
                    .fg
                    {
                        return c;
                    }
                }
            }
        }
        // 2. Apply `main_author_color` mode (`Hash`, `Me`, `Off`)
        match options.main_author_color {
            tigrs_core::config_enums::MainAuthorColor::Off => palette.author_fg,
            tigrs_core::config_enums::MainAuthorColor::Me => {
                if entry.is_some_and(|e| e.is_me) {
                    palette.ref_head_fg
                } else {
                    palette
                        .muted_style
                        .fg
                        .unwrap_or(crate::headless::Color::BrightBlack)
                }
            }
            tigrs_core::config_enums::MainAuthorColor::Hash => {
                if let Some(e) = entry {
                    if e.is_me {
                        palette.ref_head_fg
                    } else {
                        palette.author_hues[e.hue_idx as usize % 10]
                    }
                } else {
                    palette.author_hues[fnv1a_author_hash(author)]
                }
            }
        }
    }

    /// Builds the graph cell for a row.
    #[allow(clippy::too_many_arguments)]
    fn append_graph_column(
        &self,
        line: &mut String,
        main_row: MainRow<'_>,
        row_idx: usize,
        is_reverse_selected: bool,
        no_color: bool,
        options: &crate::options::ViewOptions,
        palette: &crate::ui_theme::UiPalette,
        reset_sgr: &str,
    ) {
        match main_row {
            MainRow::Changes(_) => {
                append_changes_graph_marker(
                    line,
                    options.line_graphics,
                    is_reverse_selected || no_color,
                    reset_sgr,
                );
            }
            MainRow::Commit(commit) => {
                match self
                    .commit_index(row_idx)
                    .and_then(|i| self.graph_rows.get(i))
                {
                    Some(graph) => {
                        if is_reverse_selected || no_color {
                            graph.render_into(line, options.line_graphics);
                        } else {
                            graph.render_palette_into(
                                line,
                                options.line_graphics,
                                palette,
                                reset_sgr,
                            );
                        }
                    }
                    None => line.push(if commit.is_merge() { 'M' } else { '*' }),
                }
            }
        }
    }

    /// Appends the commit subject with `P9` semantic token rules, `P11` merge dimming,
    /// and `commit-title-overflow` coloring.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    fn append_styled_subject(
        out: &mut String,
        title: &str,
        max_width: usize,
        options: &crate::options::ViewOptions,
        palette: &crate::ui_theme::UiPalette,
        meta: CommitHighlightMeta,
        is_reverse_selected: bool,
        no_color: bool,
        is_changes: bool,
        is_row_dimmed: bool,
        reset_sgr: &str,
    ) {
        if is_reverse_selected || no_color {
            append_truncated(out, title, max_width);
            return;
        }

        if is_changes {
            crate::ui_theme::UiPalette::write_fg_sgr(out, palette.status_untracked_fg);
            append_truncated(out, title, max_width);
            out.push_str(reset_sgr);
            return;
        }

        // If `commit-title-overflow` is set, preserve exact overflow red coloring past `limit`
        if let Some(limit) = options.commit_title_overflow {
            let mut total = 0;
            let mut in_overflow = false;
            for ch in title.chars() {
                let w = ch.width().unwrap_or(0);
                if total + w > max_width {
                    break;
                }
                if total >= limit && !in_overflow {
                    crate::ui_theme::UiPalette::write_fg_sgr(out, palette.title_overflow_fg);
                    in_overflow = true;
                }
                out.push(ch);
                total += w;
            }
            if in_overflow {
                out.push_str(reset_sgr);
            }
            return;
        }

        // Check custom `[[main.subject-rules]]` first (`P9`)
        if options.main_subject_rules && !options.custom_subject_rules.is_empty() {
            for rule in &options.custom_subject_rules {
                if !rule.pattern.is_empty() && title.contains(rule.pattern.as_str()) {
                    if rule.bold {
                        out.push_str("\x1b[1m");
                    }
                    if rule.dim {
                        out.push_str("\x1b[2m");
                    }
                    if let Some(c) = crate::diff::style_from_spec(&tigrs_core::ColorSpec::String(
                        rule.fg.clone(),
                    ))
                    .fg
                    {
                        crate::ui_theme::UiPalette::write_fg_sgr(out, c);
                    }
                    append_truncated(out, title, max_width);
                    out.push_str(reset_sgr);
                    return;
                }
            }
        }

        // `P11`: Merge commit dimming
        if options.main_dim_merges && meta.subject_kind == SubjectRuleKind::MergeCommit {
            out.push_str("\x1b[2m");
            append_truncated(out, title, max_width);
            out.push_str(reset_sgr);
            return;
        }

        // `P9`: Built-in semantic subject rules
        if options.main_subject_rules && !is_row_dimmed {
            match meta.subject_kind {
                SubjectRuleKind::FixupSquash => {
                    let prefix_end = title.find('!').map_or(title.len(), |i| i + 1);
                    let (prefix, rest) = title.split_at(prefix_end);
                    let p_trunc = tigrs_core::ansi::truncate_display_width(prefix, max_width);
                    let p_w = UnicodeWidthStr::width(p_trunc);
                    crate::ui_theme::UiPalette::write_fg_sgr(out, palette.title_overflow_fg);
                    out.push_str("\x1b[1m");
                    out.push_str(p_trunc);
                    out.push_str(reset_sgr);
                    if p_w < max_width {
                        append_subject_with_issue_tokens(
                            out,
                            rest,
                            max_width - p_w,
                            (meta.flags & FLAG_HAS_ISSUE_REF) != 0,
                            palette.ref_head_fg,
                            reset_sgr,
                        );
                    }
                    return;
                }
                SubjectRuleKind::Revert => {
                    let prefix_end = if title.starts_with("Revert") { 6 } else { 7 };
                    let (prefix, rest) = title.split_at(prefix_end.min(title.len()));
                    let p_trunc = tigrs_core::ansi::truncate_display_width(prefix, max_width);
                    let p_w = UnicodeWidthStr::width(p_trunc);
                    crate::ui_theme::UiPalette::write_fg_sgr(out, palette.ref_remote_fg);
                    out.push_str("\x1b[1m");
                    out.push_str(p_trunc);
                    out.push_str(reset_sgr);
                    if p_w < max_width {
                        append_subject_with_issue_tokens(
                            out,
                            rest,
                            max_width - p_w,
                            (meta.flags & FLAG_HAS_ISSUE_REF) != 0,
                            palette.ref_head_fg,
                            reset_sgr,
                        );
                    }
                    return;
                }
                SubjectRuleKind::Wip => {
                    crate::ui_theme::UiPalette::write_fg_sgr(out, palette.commit_id_fg);
                    out.push_str("\x1b[1m");
                    append_truncated(out, title, max_width);
                    out.push_str(reset_sgr);
                    return;
                }
                SubjectRuleKind::BreakingConventional | SubjectRuleKind::Conventional => {
                    let is_breaking = meta.subject_kind == SubjectRuleKind::BreakingConventional;
                    if (is_breaking || palette.id != tigrs_core::config_enums::UiThemeId::Default)
                        && let Some(colon_idx) = title.find(':')
                    {
                        let (prefix, rest) = title.split_at(colon_idx + 1);
                        let p_trunc = tigrs_core::ansi::truncate_display_width(prefix, max_width);
                        let p_w = UnicodeWidthStr::width(p_trunc);
                        if is_breaking {
                            crate::ui_theme::UiPalette::write_fg_sgr(
                                out,
                                palette.title_overflow_fg,
                            );
                        }
                        out.push_str("\x1b[1m");
                        out.push_str(p_trunc);
                        out.push_str(reset_sgr);
                        if p_w < max_width {
                            append_subject_with_issue_tokens(
                                out,
                                rest,
                                max_width - p_w,
                                (meta.flags & FLAG_HAS_ISSUE_REF) != 0,
                                palette.ref_head_fg,
                                reset_sgr,
                            );
                        }
                        return;
                    } else if (meta.flags & FLAG_HAS_ISSUE_REF) != 0 {
                        append_subject_with_issue_tokens(
                            out,
                            title,
                            max_width,
                            true,
                            palette.ref_head_fg,
                            reset_sgr,
                        );
                        return;
                    }
                }
                SubjectRuleKind::Normal | SubjectRuleKind::MergeCommit => {
                    if (meta.flags & FLAG_HAS_ISSUE_REF) != 0 {
                        append_subject_with_issue_tokens(
                            out,
                            title,
                            max_width,
                            true,
                            palette.ref_head_fg,
                            reset_sgr,
                        );
                        return;
                    }
                }
            }
        }

        append_truncated(out, title, max_width);
    }
}

fn append_subject_with_issue_tokens(
    out: &mut String,
    s: &str,
    max_width: usize,
    highlight_issues: bool,
    issue_color: crate::headless::Color,
    reset_sgr: &str,
) {
    let truncated = tigrs_core::ansi::truncate_display_width(s, max_width);
    if !highlight_issues {
        out.push_str(truncated);
        return;
    }
    let bytes = truncated.as_bytes();
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            out.push_str(&truncated[last..i]);
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            crate::ui_theme::UiPalette::write_fg_sgr(out, issue_color);
            out.push_str("\x1b[4m");
            out.push_str(&truncated[start..i]);
            out.push_str("\x1b[24m");
            out.push_str(reset_sgr);
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&truncated[last..]);
}

impl super::View for MainView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Main
    }

    fn line_count(&self) -> usize {
        self.row_count()
    }

    fn nav(&self) -> &ViewportCursor {
        &self.nav
    }

    fn nav_mut(&mut self) -> &mut ViewportCursor {
        &mut self.nav
    }

    fn render(
        &self,
        mut writer: &mut dyn std::io::Write,
        width: u16,
        height: u16,
        options: &crate::options::ViewOptions,
    ) -> tigrs_core::error::Result<()> {
        self.render_with_options(&mut writer, width, height, options)?;
        Ok(())
    }

    fn page_down(&mut self, visible_height: usize) {
        self.page_down(visible_height);
    }

    fn page_up(&mut self, visible_height: usize) {
        self.page_up(visible_height);
    }

    fn matches_search(&self, index: usize, pat: &crate::search::SearchPattern) -> bool {
        match self.row(index) {
            Some(MainRow::Commit(c)) => {
                pat.is_match(&c.summary) || pat.is_match(&c.author_name) || pat.is_match_oid(&c.id)
            }
            Some(MainRow::Changes(ch)) => pat.is_match(ch.kind.title()),
            None => false,
        }
    }

    fn selected_commit_id(&self) -> Option<tigrs_git::ObjectId> {
        self.selected_commit().map(|c| c.id)
    }

    fn has_content(&self) -> bool {
        self.commit_count() > 0
    }
}

/// Column values for one rendered row, unifying commits and changes rows.
struct RowFields<'a> {
    /// Abbreviated object ID, or the null ID for changes rows.
    short_id: String,
    /// Timestamp used by the date column.
    time_secs: i64,
    /// Author name displayed in the author column.
    author: &'a str,
    /// Row title displayed after all fixed-width columns.
    title: &'a str,
}

/// Extracts the shared column values for `main_row`.
fn row_fields(main_row: MainRow<'_>, now_secs: i64) -> RowFields<'_> {
    match main_row {
        MainRow::Commit(commit) => RowFields {
            short_id: commit.short_id(),
            time_secs: commit.author_time_secs,
            author: &commit.author_name,
            title: &commit.summary,
        },
        MainRow::Changes(change) => RowFields {
            short_id: CHANGES_SHORT_ID.to_string(),
            time_secs: now_secs,
            author: CHANGES_AUTHOR,
            title: change.kind.title(),
        },
    }
}

/// Renders the two-column lane-0 node glyph used by changes rows.
///
/// Colors are omitted on the selected row so the reverse-video highlight is not
/// broken by an embedded reset sequence.
fn append_changes_graph_marker(
    out: &mut String,
    style: crate::options::LineGraphics,
    is_selected: bool,
    reset_sgr: &str,
) {
    let glyph = match style {
        crate::options::LineGraphics::Utf8 => " ○",
        crate::options::LineGraphics::Ascii => " o",
    };
    if is_selected {
        out.push_str(glyph);
    } else {
        out.push_str(COMMIT_COLOR_ANSI);
        out.push_str(glyph);
        out.push_str(reset_sgr);
    }
}

/// Appends characters of `s` to `out` until visual column budget `max_width` is reached.
fn append_truncated(out: &mut String, s: &str, max_width: usize) {
    out.push_str(tigrs_core::ansi::truncate_display_width(s, max_width));
}

/// Appends `s` to `out` formatted to exactly `target_width` columns, truncating or right-padding with spaces.
fn append_column(out: &mut String, s: &str, target_width: usize) {
    let current_width = UnicodeWidthStr::width(s);
    if current_width > target_width {
        append_truncated(out, s, target_width);
    } else {
        out.push_str(s);
        for _ in 0..(target_width - current_width) {
            out.push(' ');
        }
    }
}

/// Writes a string to the output writer and clears remainder of `width` columns using spaces or `\x1b[K`.
fn write_padded_line<W: Write>(
    w: &mut W,
    s: &str,
    width: usize,
    pad_with_spaces: bool,
) -> std::io::Result<()> {
    static SPACES: &[u8; 256] = &[b' '; 256];
    let s_width = tigrs_core::ansi::visible_width(s);
    w.write_all(s.as_bytes())?;
    if pad_with_spaces {
        if s_width < width {
            let mut rem = width - s_width;
            while rem > 0 {
                let chunk = rem.min(SPACES.len());
                w.write_all(&SPACES[..chunk])?;
                rem -= chunk;
            }
        }
    } else {
        w.write_all(b"\x1b[K")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix::ObjectId;
    use std::sync::Arc;

    fn make_test_commit(summary: &str) -> CommitSummary {
        CommitSummary {
            id: ObjectId::empty_tree(gix::hash::Kind::Sha1),
            parents: tigrs_git::ParentIds::new(),
            author_name: Arc::from("Author"),
            author_time_secs: 1_000_000,
            summary: Box::from(summary),
        }
    }

    #[test]
    fn test_main_view_navigation() {
        let mut view = MainView::new("main".to_string());
        let batch: Vec<_> = (0..50)
            .map(|i| make_test_commit(&format!("commit {i}")))
            .collect();
        view.append_commits(batch);
        view.set_finished();

        assert_eq!(view.commit_count(), 50);
        assert_eq!(view.nav.cursor, 0);

        view.move_down(10, 20);
        assert_eq!(view.nav.cursor, 10);

        view.move_up(3, 20);
        assert_eq!(view.nav.cursor, 7);

        view.home();
        assert_eq!(view.nav.cursor, 0);

        view.end(20);
        assert_eq!(view.nav.cursor, 49);
    }

    #[test]
    fn test_append_column_and_truncated() {
        let mut buf = String::new();
        append_column(&mut buf, "abc", 6);
        assert_eq!(buf, "abc   ");

        buf.clear();
        append_column(&mut buf, "overlength", 4);
        assert_eq!(buf, "over");

        buf.clear();
        append_truncated(&mut buf, "日本語テスト", 6);
        // Each full-width character is 2 visual columns, so 3 characters = 6 visual columns
        assert_eq!(buf, "日本語");
    }

    #[test]
    fn test_render_buffer() {
        let mut view = MainView::new("main".to_string());
        let batch: Vec<_> = (0..5)
            .map(|i| make_test_commit(&format!("commit {i}")))
            .collect();
        view.append_commits(batch);
        view.set_finished();

        let mut output = Vec::new();
        view.render(&mut output, 80, 24).expect("render failed");
        assert!(!output.is_empty());
    }

    #[test]
    fn test_main_view_commit_storage_and_shrink_to_fit() {
        let mut view = MainView::new("main".to_string());
        assert_eq!(view.commit_count(), 0);

        let batch: Vec<_> = (0..10)
            .map(|i| make_test_commit(&format!("commit {i}")))
            .collect();
        view.append_commits(batch);

        assert_eq!(view.commit_count(), 10);
        assert!(!view.commits()[0].is_merge());

        view.shrink_to_fit();
        assert_eq!(view.commit_count(), 10);
    }

    #[test]
    fn test_main_view_graph_integration() {
        let mut view = MainView::new("main".to_string());
        let c1 = make_test_commit("root commit");
        let mut c2 = make_test_commit("child commit");
        c2.parents = smallvec::smallvec![c1.id];

        view.append_commits(vec![c2, c1]);
        assert_eq!(view.graph_rows.len(), 2);

        // Child commit (linear)
        assert_eq!(
            view.graph_rows[0].to_graph_string(crate::options::LineGraphics::Utf8),
            " ∙"
        );
        assert_eq!(
            view.graph_rows[0].to_graph_string(crate::options::LineGraphics::Ascii),
            " *"
        );

        // Root commit
        assert_eq!(
            view.graph_rows[1].to_graph_string(crate::options::LineGraphics::Utf8),
            " ◎"
        );
        assert_eq!(
            view.graph_rows[1].to_graph_string(crate::options::LineGraphics::Ascii),
            " I"
        );

        // Test rendering with UTF-8 line graphics
        let mut opts = crate::options::ViewOptions {
            line_graphics: crate::options::LineGraphics::Utf8,
            ..Default::default()
        };
        let mut out = Vec::new();
        view.render_with_options(&mut out, 80, 24, &opts)
            .expect("render");
        let rendered = String::from_utf8_lossy(&out);
        assert!(rendered.contains(" ∙") || rendered.contains(" ◎"));

        // Test rendering with ASCII line graphics
        opts.line_graphics = crate::options::LineGraphics::Ascii;
        out.clear();
        view.render_with_options(&mut out, 80, 24, &opts)
            .expect("render");
        let rendered_ascii = String::from_utf8_lossy(&out);
        assert!(rendered_ascii.contains(" *") || rendered_ascii.contains(" I"));
    }
    fn changes_view(n_commits: usize, kinds: &[ChangesKind]) -> MainView {
        let mut view = MainView::new("main".to_string());
        let batch: Vec<_> = (0..n_commits)
            .map(|i| make_test_commit(&format!("commit {i}")))
            .collect();
        view.append_commits(batch);
        view.set_finished();
        view.set_changes(kinds.iter().map(|k| ChangesRow::new(*k, 1)).collect());
        view
    }

    #[test]
    fn test_changes_rows_map_to_display_indices() {
        let view = changes_view(3, &[ChangesKind::Unstaged, ChangesKind::Staged]);

        assert_eq!(view.changes_len(), 2);
        assert_eq!(view.commit_count(), 3);
        assert_eq!(view.row_count(), 5);

        assert!(matches!(
            view.row(0),
            Some(MainRow::Changes(c)) if c.kind == ChangesKind::Unstaged
        ));
        assert!(matches!(
            view.row(1),
            Some(MainRow::Changes(c)) if c.kind == ChangesKind::Staged
        ));
        assert!(matches!(view.row(2), Some(MainRow::Commit(_))));
        assert!(matches!(view.row(4), Some(MainRow::Commit(_))));
        assert!(view.row(5).is_none());

        // Synthetic rows have no commit index; commits are offset by the prefix.
        assert_eq!(view.commit_index(0), None);
        assert_eq!(view.commit_index(1), None);
        assert_eq!(view.commit_index(2), Some(0));
        assert_eq!(view.commit_index(4), Some(2));
        assert_eq!(view.commit_index(5), None);
        assert_eq!(view.row_for_commit(0), 2);
    }

    #[test]
    fn test_selected_commit_is_none_on_changes_row() {
        let mut view = changes_view(3, &[ChangesKind::Untracked]);

        assert_eq!(view.cursor_index(), 0);
        assert!(view.selected_commit().is_none());
        assert!(matches!(view.selected_row(), Some(MainRow::Changes(_))));

        view.move_down(1, 10);
        assert_eq!(view.cursor_index(), 1);
        let commit = view.selected_commit().expect("first commit selected");
        assert_eq!(&*commit.summary, "commit 0");
    }

    #[test]
    fn test_navigation_spans_changes_and_commits() {
        let mut view = changes_view(3, &[ChangesKind::Unstaged, ChangesKind::Staged]);

        view.end(10);
        assert_eq!(view.cursor_index(), 4);

        view.home();
        assert_eq!(view.cursor_index(), 0);

        // Cursor must not run past the last commit.
        view.move_down(100, 10);
        assert_eq!(view.cursor_index(), 4);
    }

    #[test]
    fn test_scroll_accounts_for_changes_rows() {
        // 4 rows total, viewport of 3 => max scroll offset of 1.
        let mut view = changes_view(3, &[ChangesKind::Staged]);
        view.scroll_line_down(10, 3);
        assert_eq!(view.scroll_offset(), 1);
    }

    #[test]
    fn test_set_changes_preserves_commit_selection() {
        let mut view = changes_view(5, &[ChangesKind::Unstaged]);

        // Select the third commit (display row 3).
        view.set_cursor(3, 10);
        assert_eq!(
            &*view.selected_commit().expect("commit").summary,
            "commit 2"
        );

        // Staging a file adds a row above; the same commit must stay selected.
        view.set_changes(vec![
            ChangesRow::new(ChangesKind::Unstaged, 1),
            ChangesRow::new(ChangesKind::Staged, 1),
        ]);
        assert_eq!(view.cursor_index(), 4);
        assert_eq!(
            &*view.selected_commit().expect("commit").summary,
            "commit 2"
        );

        // Committing everything removes both rows; selection still follows.
        view.set_changes(Vec::new());
        assert_eq!(view.cursor_index(), 2);
        assert_eq!(
            &*view.selected_commit().expect("commit").summary,
            "commit 2"
        );
    }

    #[test]
    fn test_set_changes_preserves_section_selection() {
        let mut view = changes_view(3, &[ChangesKind::Unstaged, ChangesKind::Staged]);

        // Select the "Staged changes" row.
        view.set_cursor(1, 10);

        // The unstaged section disappears; staged stays selected at its new index.
        view.set_changes(vec![ChangesRow::new(ChangesKind::Staged, 2)]);
        assert_eq!(view.cursor_index(), 0);
        assert!(matches!(
            view.selected_row(),
            Some(MainRow::Changes(c)) if c.kind == ChangesKind::Staged
        ));
    }

    #[test]
    fn test_set_changes_falls_through_when_section_vanishes() {
        let mut view = changes_view(3, &[ChangesKind::Untracked, ChangesKind::Staged]);
        // Select "Staged changes" so the pinned-to-top fast path does not apply.
        view.set_cursor(1, 10);

        view.set_changes(vec![ChangesRow::new(ChangesKind::Untracked, 1)]);

        // The selected section is gone, so the cursor lands on the first commit.
        assert_eq!(view.cursor_index(), 1);
        assert_eq!(
            &*view.selected_commit().expect("commit").summary,
            "commit 0"
        );
    }

    #[test]
    fn test_set_changes_keeps_view_pinned_to_top() {
        let mut view = changes_view(3, &[]);
        assert_eq!(view.cursor_index(), 0);
        assert_eq!(view.scroll_offset(), 0);

        view.set_changes(vec![
            ChangesRow::new(ChangesKind::Unstaged, 1),
            ChangesRow::new(ChangesKind::Staged, 1),
        ]);

        // The new rows must be visible and selected, not scrolled past.
        assert_eq!(view.scroll_offset(), 0);
        assert_eq!(view.cursor_index(), 0);
        assert!(matches!(
            view.selected_row(),
            Some(MainRow::Changes(c)) if c.kind == ChangesKind::Unstaged
        ));
    }

    #[test]
    fn test_set_changes_shifts_viewport_when_scrolled() {
        let mut view = changes_view(20, &[]);
        view.set_cursor(10, 5);
        let scroll_before = view.scroll_offset();
        assert!(scroll_before > 0);

        view.set_changes(vec![ChangesRow::new(ChangesKind::Unstaged, 1)]);

        // Both cursor and viewport shift by the prefix delta, so the same
        // commits stay on screen in the same positions.
        assert_eq!(view.cursor_index(), 11);
        assert_eq!(view.scroll_offset(), scroll_before + 1);
        assert_eq!(
            &*view.selected_commit().expect("commit").summary,
            "commit 10"
        );
    }

    #[test]
    fn test_set_changes_clamps_when_no_commits_remain() {
        let mut view = MainView::new("main".to_string());
        view.set_finished();
        view.set_changes(vec![ChangesRow::new(ChangesKind::Unstaged, 1)]);
        assert_eq!(view.cursor_index(), 0);

        view.set_changes(Vec::new());
        assert_eq!(view.row_count(), 0);
        assert_eq!(view.cursor_index(), 0);
        assert_eq!(view.scroll_offset(), 0);
        assert!(view.selected_row().is_none());
    }

    #[test]
    fn test_set_changes_is_a_noop_when_unchanged() {
        let mut view = changes_view(5, &[ChangesKind::Unstaged]);
        view.set_cursor(4, 10);
        let before = view.cursor_index();

        view.set_changes(vec![ChangesRow::new(ChangesKind::Unstaged, 1)]);
        assert_eq!(view.cursor_index(), before);
    }

    #[test]
    fn test_changes_rows_render_with_tig_parity_fields() {
        let view = changes_view(
            2,
            &[
                ChangesKind::Untracked,
                ChangesKind::Unstaged,
                ChangesKind::Staged,
            ],
        );

        // The ID column is off by default (tig parity), so enable it explicitly
        // to assert the null-ID rendering.
        let opts = crate::options::ViewOptions {
            commit_id: true,
            ..Default::default()
        };
        let mut out = Vec::new();
        view.render_with_options(&mut out, 100, 10, &opts)
            .expect("render");
        let rendered = String::from_utf8_lossy(&out);

        assert!(rendered.contains("Untracked changes"));
        assert!(rendered.contains("Unstaged changes"));
        assert!(rendered.contains("Staged changes"));
        assert!(rendered.contains(CHANGES_SHORT_ID));
        // The author column is 15 wide, so the 17-character name is truncated
        // just like any other long author name.
        assert!(rendered.contains(tigrs_core::ansi::truncate_display_width(CHANGES_AUTHOR, 15)));

        // The header still reports only real commits...
        assert!(rendered.contains("2 commits loaded"));
        // ...while the status bar counts every reachable row.
        assert!(rendered.contains("line 1 of 5"));
    }

    #[test]
    fn test_changes_graph_marker_respects_line_graphics() {
        let marker = |style, selected| {
            let mut out = String::new();
            append_changes_graph_marker(&mut out, style, selected, RESET_ANSI);
            out
        };

        let utf8 = marker(crate::options::LineGraphics::Utf8, false);
        assert!(utf8.contains(" ○"));
        assert!(utf8.starts_with(COMMIT_COLOR_ANSI));

        let ascii = marker(crate::options::LineGraphics::Ascii, false);
        assert!(ascii.contains(" o"));

        // The selected row is reverse-video, so no embedded color is emitted.
        assert_eq!(marker(crate::options::LineGraphics::Utf8, true), " ○");

        // Both variants occupy exactly two visible columns, like commit lanes.
        assert_eq!(tigrs_core::ansi::visible_width(&utf8), 2);
        assert_eq!(tigrs_core::ansi::visible_width(&ascii), 2);
    }

    #[test]
    fn test_changes_rows_render_without_commits() {
        let mut view = MainView::new("main".to_string());
        view.set_finished();
        view.set_changes(vec![ChangesRow::new(ChangesKind::Unstaged, 3)]);

        let mut out = Vec::new();
        view.render_with_options(&mut out, 100, 10, &crate::options::ViewOptions::default())
            .expect("render");
        let rendered = String::from_utf8_lossy(&out);

        // The empty-state message must not win over a real changes row.
        assert!(rendered.contains("Unstaged changes"));
        assert!(!rendered.contains("No commits found."));
    }

    #[test]
    fn test_main_view_frame_byte_budget_guardrail() {
        let mut view = MainView::new("main".to_string());
        let commits: Vec<CommitSummary> = (0..50)
            .map(|i| {
                let mut c = make_test_commit(&format!("feat: commit {i} with message description"));
                c.author_name = Arc::from(format!("Dev {i}"));
                c.author_time_secs = 1_700_000_000 - (i64::from(i) * 3600);
                c
            })
            .collect();
        view.append_commits(commits);

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        let bytes_emitted = buf.len();
        println!("80x24 MainView frame bytes: {bytes_emitted}");

        // 80x24 terminal frame should emit well under 2,500 bytes (with EL optimization, typically ~1,400 - 1,800 bytes).
        // This acts as a guardrail against regression into byte-ballooning.
        assert!(
            bytes_emitted < 2500,
            "MainView 80x24 frame byte budget exceeded: {bytes_emitted} bytes >= 2500"
        );
    }

    /// Builds a loading view holding `loaded` commits out of an expected `total`.
    fn loading_view(loaded: usize, total: usize) -> MainView {
        let mut view = MainView::new("main".to_string());
        view.append_commits(
            (0..loaded)
                .map(|i| make_test_commit(&format!("c{i}")))
                .collect(),
        );
        view.set_total_commits(total);
        view
    }

    #[test]
    fn test_total_commits_handle_publishes_from_another_thread() {
        // Mirrors how the CLI hands the counter off: the view is moved into the
        // app while a background thread still holds a handle to its total.
        let mut view = MainView::new("main".to_string());
        view.append_commits(
            (0..2_500)
                .map(|i| make_test_commit(&format!("c{i}")))
                .collect(),
        );
        let handle = view.total_commits_handle();

        assert_eq!(view.load_percent(), None);

        std::thread::spawn(move || handle.store(10_000, Ordering::Relaxed))
            .join()
            .unwrap();

        assert_eq!(view.total_commits(), Some(10_000));
        assert_eq!(view.load_percent(), Some(25));
    }

    #[test]
    fn test_load_percent_only_applies_to_a_large_streaming_history() {
        // No total counted yet: nothing to take a percentage of.
        let mut view = MainView::new("main".to_string());
        view.append_commits(
            (0..100)
                .map(|i| make_test_commit(&format!("c{i}")))
                .collect(),
        );
        assert_eq!(view.total_commits(), None);
        assert_eq!(view.load_percent(), None);

        // Total known but the history is small, so an exact count still reads well.
        let small = loading_view(100, LARGE_HISTORY_THRESHOLD - 1);
        assert_eq!(small.load_percent(), None);

        // Large history mid-stream.
        let large = loading_view(1_500, 10_000);
        assert_eq!(large.load_percent(), Some(15));

        // Streaming finished: the final count is exact and worth showing.
        let mut done = loading_view(1_500, 10_000);
        done.set_finished();
        assert_eq!(done.load_percent(), None);
    }

    #[test]
    fn test_load_percent_is_capped_below_completion() {
        // The total is only an estimate, so it can be met or overshot while the
        // walk is still running. Progress must never claim to be complete.
        let exact = loading_view(10_000, 10_000);
        assert_eq!(exact.load_percent(), Some(99));

        let overshot = loading_view(12_000, 10_000);
        assert_eq!(overshot.load_percent(), Some(99));

        // Nothing loaded yet is a legitimate 0%.
        let empty = loading_view(0, 10_000);
        assert_eq!(empty.load_percent(), Some(0));
    }

    #[test]
    fn test_header_reports_percentage_while_loading_large_history() {
        let view = loading_view(1_500, 10_000);
        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        let rendered = String::from_utf8_lossy(&buf);

        assert!(
            rendered.contains("[main] main - 15% commits loaded"),
            "expected a percentage readout, got: {rendered}"
        );
        assert!(
            !rendered.contains("1500 commits loaded"),
            "the running commit count must not be shown alongside the percentage"
        );
    }

    #[test]
    fn test_header_keeps_exact_count_without_a_counted_total() {
        // A counter that never reports leaves the previous behaviour intact.
        let mut view = MainView::new("main".to_string());
        view.append_commits((0..3).map(|i| make_test_commit(&format!("c{i}"))).collect());

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        let rendered = String::from_utf8_lossy(&buf);
        assert!(rendered.contains("3 commits loaded (loading...)"));

        view.set_finished();
        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        let rendered = String::from_utf8_lossy(&buf);
        assert!(rendered.contains("3 commits loaded"));
        assert!(!rendered.contains("loading..."));
    }

    #[test]
    fn test_main_view_commit_title_overflow_coloring() {
        let mut view = MainView::new("main".to_string());
        view.append_commits(vec![
            make_test_commit("First commit with short message"),
            make_test_commit("0123456789OVERFLOW_SECTION"),
        ]);
        view.set_finished();

        // Cursor is on row 0 (first commit), so row 1 (second commit) is unselected.
        let opts = crate::options::ViewOptions {
            commit_title_overflow: Some(10),
            ..Default::default()
        };
        let mut buf = Vec::new();
        view.render_with_options(&mut buf, 80, 24, &opts).unwrap();
        let rendered = String::from_utf8_lossy(&buf);

        // The overflow text should be wrapped with red ANSI escape \x1b[31m
        assert!(
            rendered.contains("\x1b[31mOVERFLOW_SECTION\x1b[0m"),
            "Expected red overflow coloring, got:\n{rendered}"
        );
    }

    #[test]
    fn test_main_view_ref_badges_colored() {
        let mut view = MainView::new("main".to_string());
        let commit = make_test_commit("Ref badge test commit");
        let cid = commit.id;
        view.append_commits(vec![
            make_test_commit("First commit to hold cursor"),
            commit,
        ]);
        view.set_finished();

        view.set_ref_badges(&[
            RefEntry {
                full_name: "refs/heads/main".to_string(),
                name: "main".to_string(),
                commit_id: cid,
                kind: RefKind::LocalBranch,
                summary: "Summary".to_string(),
                author_name: "Author".to_string(),
                author_time_secs: 1_000_000,
            },
            RefEntry {
                full_name: "refs/tags/v1.0".to_string(),
                name: "v1.0".to_string(),
                commit_id: cid,
                kind: RefKind::Tag,
                summary: "Summary".to_string(),
                author_name: "Author".to_string(),
                author_time_secs: 1_000_000,
            },
        ]);

        let opts = crate::options::ViewOptions::default();
        let mut buf = Vec::new();
        view.render_with_options(&mut buf, 80, 24, &opts).unwrap();
        let rendered = String::from_utf8_lossy(&buf);

        // [main] should be Green (\x1b[32m) and <v1.0> should be Yellow (\x1b[33m)
        assert!(
            rendered.contains("\x1b[32m[main]\x1b[0m"),
            "Expected green badge, got:\n{rendered}"
        );
        assert!(
            rendered.contains("\x1b[33m<v1.0>\x1b[0m"),
            "Expected yellow badge, got:\n{rendered}"
        );
    }

    fn oid_with_prefix(b0: u8, b1: u8) -> ObjectId {
        let mut raw = [0u8; 20];
        raw[0] = b0;
        raw[1] = b1;
        ObjectId::from_bytes_or_panic(&raw)
    }

    #[test]
    fn test_p1_author_hash_determinism_and_color_overrides() {
        // FNV-1a case-insensitive stability
        assert_eq!(
            fnv1a_author_hash("Alice Developer"),
            fnv1a_author_hash("  alice developer ")
        );

        let mut view = MainView::new("main".to_string());
        let mut c0 = make_test_commit("First commit");
        c0.author_name = Arc::from("Alice Developer");
        let mut c1 = make_test_commit("Second commit");
        c1.author_name = Arc::from("Bob Maintainer");
        view.append_commits(vec![c0, c1]);
        view.set_finished();

        assert_eq!(view.authors.len(), 2);
        assert_eq!(view.authors[0].commit_count, 1);
        assert_eq!(view.authors[1].commit_count, 1);
        assert!(view.authors[0].hue_idx < 10);
        assert!(view.authors[1].hue_idx < 10);
    }

    #[test]
    fn test_p2_spotlight_author_and_ancestry_lineage() {
        let mut view = MainView::new("main".to_string());
        let id_root = oid_with_prefix(0x10, 0x00);
        let id_mid = oid_with_prefix(0x20, 0x00);
        let id_side = oid_with_prefix(0x30, 0x00);

        let mut c_side = make_test_commit("Side branch commit");
        c_side.id = id_side;
        c_side.author_name = Arc::from("Bob Maintainer");

        let mut c_mid = make_test_commit("Lineage head commit");
        c_mid.id = id_mid;
        c_mid.parents = smallvec::smallvec![id_root];
        c_mid.author_name = Arc::from("Alice Developer");

        let mut c_root = make_test_commit("Root ancestor commit");
        c_root.id = id_root;
        c_root.author_name = Arc::from("Alice Developer");

        view.append_commits(vec![c_mid, c_side, c_root]);
        view.set_finished();

        // 1. Author spotlight on cursor row 0 ("Alice Developer"): matches rows 0 and 2 (2 of 3)
        let mut opts = crate::options::ViewOptions {
            main_spotlight: tigrs_core::MainSpotlight::Author,
            main_spotlight_dim_others: true,
            ..Default::default()
        };
        let mut buf = Vec::new();
        view.render_with_options(&mut buf, 100, 12, &opts).unwrap();
        let rendered = String::from_utf8_lossy(&buf);
        assert!(
            rendered.contains("▎"),
            "Expected spotlight gutter bar ▎, got:\n{rendered}"
        );
        assert!(
            rendered.contains("Spotlight: Alice Developer (2 of 3 loaded)"),
            "Expected author spotlight count in status bar, got:\n{rendered}"
        );
        assert!(
            rendered.contains("\x1b[2m"),
            "Expected dim SGR on non-matching row when main_spotlight_dim_others is true"
        );

        // 2. Ancestry spotlight on cursor row 0 (c_mid -> parent c_root): matches rows 0 and 2
        opts.main_spotlight = tigrs_core::MainSpotlight::Ancestry;
        buf.clear();
        view.render_with_options(&mut buf, 100, 12, &opts).unwrap();
        let rendered_anc = String::from_utf8_lossy(&buf);
        assert!(
            rendered_anc.contains("Spotlight: Ancestry (2 of 3 loaded)"),
            "Expected ancestry spotlight readout, got:\n{rendered_anc}"
        );
    }

    #[test]
    fn test_p3_current_user_me_detection_and_mode() {
        let mut view = MainView::new("main".to_string());
        let mut c0 = make_test_commit("Cursor commit");
        c0.author_name = Arc::from("Bob Maintainer");
        let mut c1 = make_test_commit("My commit");
        c1.author_name = Arc::from("Alice Developer");
        view.append_commits(vec![c0, c1]);
        view.set_finished();
        view.set_current_user("Alice Developer", "alice@example.com");

        assert!(view.authors[1].is_me);
        assert!(!view.authors[0].is_me);

        let opts = crate::options::ViewOptions {
            main_author_color: tigrs_core::MainAuthorColor::Me,
            ..Default::default()
        };
        let mut buf = Vec::new();
        view.render_with_options(&mut buf, 100, 10, &opts).unwrap();
        let rendered = String::from_utf8_lossy(&buf);
        assert!(
            rendered.contains("● Alice Develop"),
            "Expected ● marker prefix for current user author, got:\n{rendered}"
        );
    }

    #[test]
    fn test_p4_colored_cursor_row_preserves_foreground_hues() {
        let mut view = MainView::new("main".to_string());
        let mut c0 = make_test_commit("Selected commit on Mocha theme");
        c0.author_name = Arc::from("Alice Developer");
        view.append_commits(vec![c0]);
        view.set_finished();

        let opts = crate::options::ViewOptions {
            ui_theme: tigrs_core::UiThemeId::CatppuccinMocha,
            ..Default::default()
        };
        let mut buf = Vec::new();
        view.render_with_options(&mut buf, 100, 10, &opts).unwrap();
        let rendered = String::from_utf8_lossy(&buf);

        // Cursor row in CatppuccinMocha uses surface1 background (\x1b[48;2;69;71;90m)
        // and switches foreground colors mid-row without resetting background before EOL
        assert!(
            rendered.contains("\x1b[48;2;69;71;90m"),
            "Expected Mocha cursor surface1 background SGR, got:\n{rendered}"
        );
    }

    #[test]
    fn test_p5_p6_reachability_dimming_and_push_status_indicator() {
        let mut view = MainView::new("main".to_string());
        let id_head = oid_with_prefix(0xAA, 0x01);
        let id_upstream = oid_with_prefix(0xBB, 0x02);
        let id_unreachable = oid_with_prefix(0xCC, 0x03);

        let mut c_head = make_test_commit("Local unpushed commit");
        c_head.id = id_head;
        c_head.parents = smallvec::smallvec![id_upstream];

        let mut c_upstream = make_test_commit("Shared upstream commit");
        c_upstream.id = id_upstream;

        let mut c_unreach = make_test_commit("Dangling tag commit");
        c_unreach.id = id_unreachable;

        view.append_commits(vec![c_head, c_upstream, c_unreach]);
        view.set_finished();
        view.set_head_and_upstream(Some(id_head), Some(id_upstream));

        // Commit 0 (id_head) is reachable from HEAD and unpushed (not reachable from upstream)
        assert_ne!(view.highlight_meta[0].flags & FLAG_REACHABLE_FROM_HEAD, 0);
        assert_ne!(view.highlight_meta[0].flags & FLAG_UNPUSHED, 0);

        // Commit 1 (id_upstream) is reachable from both HEAD and upstream (pushed)
        assert_ne!(view.highlight_meta[1].flags & FLAG_REACHABLE_FROM_HEAD, 0);
        assert_eq!(view.highlight_meta[1].flags & FLAG_UNPUSHED, 0);

        // Commit 2 (id_unreachable) is not reachable from HEAD
        assert_eq!(view.highlight_meta[2].flags & FLAG_REACHABLE_FROM_HEAD, 0);

        // Move cursor to row 1 so row 0 (unpushed) and row 2 (unreachable) render unselected
        view.set_cursor(1, 10);
        let opts = crate::options::ViewOptions {
            commit_id: true,
            main_push_status: true,
            main_dim_unreachable: true,
            ..Default::default()
        };
        let mut buf = Vec::new();
        view.render_with_options(&mut buf, 100, 10, &opts).unwrap();
        let rendered = String::from_utf8_lossy(&buf);

        assert!(
            rendered.contains("↑aa01"),
            "Expected ↑ push-status prefix on unpushed commit SHA, got:\n{rendered}"
        );
    }

    #[test]
    fn test_p7_relative_date_heatmap_buckets() {
        let now = 2_000_000_000;
        assert_eq!(date_heat_bucket(now - 1800, now), 0); // < 1 hour
        assert_eq!(date_heat_bucket(now - 36_000, now), 1); // < 24 hours
        assert_eq!(date_heat_bucket(now - 3 * 86_400, now), 2); // < 7 days
        assert_eq!(date_heat_bucket(now - 15 * 86_400, now), 3); // < 30 days
        assert_eq!(date_heat_bucket(now - 120 * 86_400, now), 4); // < 1 year
        assert_eq!(date_heat_bucket(now - 400 * 86_400, now), 5); // >= 1 year
    }

    #[test]
    fn test_p8_colocated_ref_badge_deduplication() {
        let mut view = MainView::new("main".to_string());
        let c0 = make_test_commit("Cursor commit");
        let mut c1 = make_test_commit("Synced branch commit");
        c1.id = oid_with_prefix(0x42, 0x42);
        let cid = c1.id;
        view.append_commits(vec![c0, c1]);
        view.set_finished();

        view.set_ref_badges(&[
            RefEntry {
                full_name: "refs/heads/main".to_string(),
                name: "main".to_string(),
                commit_id: cid,
                kind: RefKind::LocalBranch,
                summary: String::new(),
                author_name: String::new(),
                author_time_secs: 0,
            },
            RefEntry {
                full_name: "refs/remotes/origin/main".to_string(),
                name: "origin/main".to_string(),
                commit_id: cid,
                kind: RefKind::RemoteBranch,
                summary: String::new(),
                author_name: String::new(),
                author_time_secs: 0,
            },
        ]);

        let opts = crate::options::ViewOptions::default();
        let mut buf = Vec::new();
        view.render_with_options(&mut buf, 100, 10, &opts).unwrap();
        let rendered = String::from_utf8_lossy(&buf);

        assert!(
            rendered.contains("[main ⇄ origin]"),
            "Expected merged co-located ref badge [main ⇄ origin], got:\n{rendered}"
        );
        assert!(
            !rendered.contains("{origin/main}"),
            "Expected redundant remote badge {{origin/main}} to be deduplicated, got:\n{rendered}"
        );
    }

    #[test]
    fn test_p9_p11_subject_semantic_rules_and_merge_dimming() {
        assert_eq!(
            classify_subject("fixup! fix parser", false),
            (SubjectRuleKind::FixupSquash, false)
        );
        assert_eq!(
            classify_subject("Revert \"feat: old\"", false),
            (SubjectRuleKind::Revert, false)
        );
        assert_eq!(
            classify_subject("WIP: half baked", false),
            (SubjectRuleKind::Wip, false)
        );
        assert_eq!(
            classify_subject("feat(api)!: breaking", false),
            (SubjectRuleKind::BreakingConventional, false)
        );

        let mut view = MainView::new("main".to_string());
        let c0 = make_test_commit("Cursor commit");
        let c_fixup = make_test_commit("fixup! address review feedback (#42)");
        let mut c_merge = make_test_commit("Merge branch 'feature' into main");
        c_merge.parents =
            smallvec::smallvec![oid_with_prefix(0x01, 0x01), oid_with_prefix(0x02, 0x02)];
        view.append_commits(vec![c0, c_fixup, c_merge]);
        view.set_finished();

        let opts = crate::options::ViewOptions {
            main_subject_rules: true,
            main_dim_merges: true,
            ..Default::default()
        };
        let mut buf = Vec::new();
        view.render_with_options(&mut buf, 100, 10, &opts).unwrap();
        let rendered = String::from_utf8_lossy(&buf);

        // Issue reference (#42) is underlined & cyan (\x1b[36m\x1b[4m#42\x1b[24m\x1b[0m)
        assert!(
            rendered.contains("#42") && rendered.contains("\x1b[4m#42\x1b[24m"),
            "Expected underlined cyan issue token #42, got:\n{rendered}"
        );
        // Merge commit is dimmed (\x1b[2mMerge branch...)
        assert!(
            rendered.contains("\x1b[2mMerge branch"),
            "Expected dimmed merge commit subject, got:\n{rendered}"
        );
    }

    #[test]
    fn test_p10_unique_sha_prefix_bolding_and_monochrome_fallback() {
        let mut view = MainView::new("main".to_string());
        let mut c0 = make_test_commit("Cursor commit");
        c0.id = ObjectId::from_bytes_or_panic(&[
            0xAB, 0xCD, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        let mut c1 = make_test_commit("Colliding prefix commit");
        c1.id = ObjectId::from_bytes_or_panic(&[
            0xAB, 0xCD, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        view.append_commits(vec![c0, c1]);
        view.set_finished();

        // Both start with "abcd", differing at 5th hex digit ('1' vs '2') -> unique prefix length = 5
        assert_eq!(view.unique_prefix_len(&view.commits[1].id), 5);

        let mut opts = crate::options::ViewOptions {
            commit_id: true,
            main_unique_prefix: true,
            ..Default::default()
        };
        let mut buf = Vec::new();
        view.render_with_options(&mut buf, 100, 10, &opts).unwrap();
        let rendered = String::from_utf8_lossy(&buf);
        assert!(
            rendered.contains("\x1b[1mabcd2\x1b[22;2m00"),
            "Expected bold unique prefix 'abcd2' followed by dimmed '00', got:\n{rendered}"
        );

        // Monochrome fallback (no_color = true): zero SGR color escapes (only reverse video \x1b[7m on cursor row)
        opts.no_color = true;
        buf.clear();
        view.render_with_options(&mut buf, 100, 10, &opts).unwrap();
        let rendered_mono = String::from_utf8_lossy(&buf);
        assert!(
            !rendered_mono.contains("\x1b[38;2;") && !rendered_mono.contains("\x1b[32m"),
            "Expected zero color SGR codes when no_color is enabled, got:\n{rendered_mono}"
        );
    }

    #[test]
    fn test_selected_row_full_width_highlight_with_spotlight_and_current_user() {
        let mut view = MainView::new("main".to_string());
        view.set_current_user("Alice Developer", "alice@example.com");
        let mut c0 = make_test_commit("feat: selected commit with spotlight #42 and long subject");
        c0.id = oid_with_prefix(0xAA, 0x01);
        c0.author_name = Arc::from("Alice Developer");
        let mut c1 = make_test_commit("fix: second commit");
        c1.id = oid_with_prefix(0xBB, 0x02);
        c1.author_name = Arc::from("Bob Reviewer");
        c0.parents = smallvec::smallvec![c1.id];
        view.append_commits(vec![c0.clone(), c1.clone()]);
        view.set_changes(vec![ChangesRow::new(ChangesKind::Unstaged, 2)]);

        // Attach ref badges and mark c0 as unpushed (upstream at c1) so all inline SGR spans
        // (ref badge, unpushed arrow, spotlight gutter, current-user marker, subject rule,
        // #42 issue token, and commit_title_overflow) are active on the selected row.
        view.set_ref_badges(&[
            RefEntry {
                name: "main".to_string(),
                full_name: "refs/heads/main".to_string(),
                commit_id: c0.id,
                kind: RefKind::LocalBranch,
                author_name: "Alice Developer".to_string(),
                author_time_secs: 1_700_000_000,
                summary: "feat".to_string(),
            },
            RefEntry {
                name: "v1.0".to_string(),
                full_name: "refs/tags/v1.0".to_string(),
                commit_id: c0.id,
                kind: RefKind::Tag,
                author_name: "Alice Developer".to_string(),
                author_time_secs: 1_700_000_000,
                summary: "feat".to_string(),
            },
        ]);
        view.set_head_and_upstream(Some(c0.id), Some(c1.id));
        view.set_finished();

        // Test both row 0 (MainRow::Changes) and row 1 (MainRow::Commit(0) with all inline spans)
        for selected_row in [0_usize, 1_usize] {
            view.set_cursor(selected_row, 10);
            let term_y = selected_row + 1;

            for spotlight in [
                tigrs_core::MainSpotlight::Author,
                tigrs_core::MainSpotlight::Ancestry,
                tigrs_core::MainSpotlight::Off,
            ] {
                // 1. Default theme (reverse-video cursor highlight): every column 0..100 must be REVERSE
                let opts = crate::options::ViewOptions {
                    main_spotlight: spotlight,
                    commit_id: true,
                    commit_title_overflow: Some(10),
                    ..Default::default()
                };
                let mut term = crate::headless::HeadlessTerminal::new(100, 10);
                view.render_with_options(&mut term, 100, 10, &opts).unwrap();
                for x in 0..100 {
                    let cell = term.cell(x, term_y).expect("cell on selected row");
                    assert!(
                        cell.attrs.contains(crate::headless::CellAttrs::REVERSE),
                        "Selected row {selected_row} lost REVERSE highlight at column {x} (ch={:?}) with spotlight={spotlight:?}",
                        cell.ch
                    );
                }

                // 2. Named theme (background-colored cursor bar): every column 0..100 must retain cursor_row.bg
                let themed_opts = crate::options::ViewOptions {
                    main_spotlight: spotlight,
                    commit_id: true,
                    commit_title_overflow: Some(10),
                    ui_theme: tigrs_core::UiThemeId::Dracula,
                    ..Default::default()
                };
                let expected_bg = themed_opts.ui_palette().cursor_row.bg;
                assert!(expected_bg.is_some());
                let mut themed_term = crate::headless::HeadlessTerminal::new(100, 10);
                view.render_with_options(&mut themed_term, 100, 10, &themed_opts)
                    .unwrap();
                for x in 0..100 {
                    let cell = themed_term.cell(x, term_y).expect("cell on selected row");
                    assert_eq!(
                        cell.bg, expected_bg,
                        "Selected row {selected_row} lost cursor_row.bg at column {x} (ch={:?}) with spotlight={spotlight:?}",
                        cell.ch
                    );
                }
            }
        }
    }

    #[test]
    fn test_unique_prefix_len_o1_prefix16_buckets_and_collisions() {
        let mut view = MainView::new("main".to_string());
        let mut c0 = make_test_commit("first commit");
        c0.id = ObjectId::from_hex(b"abcde10123456789abcdef0123456789abcdef01").unwrap();
        let mut c1 = make_test_commit("colliding 5-char prefix commit");
        c1.id = ObjectId::from_hex(b"abcde29999999999abcdef0123456789abcdef01").unwrap();
        let mut c2 = make_test_commit("distinct bucket commit");
        c2.id = ObjectId::from_hex(b"123456789abcdef0123456789abcdef012345678").unwrap();
        view.append_commits(vec![c0.clone(), c1.clone(), c2.clone()]);

        // c0 and c1 share "abcde" (5 hex nibbles), so unique_prefix_len must expand to 6
        assert_eq!(view.unique_prefix_len(&c0.id), 6);
        assert_eq!(view.unique_prefix_len(&c1.id), 6);
        // c2 has no collision in its 16-bit bucket, so it uses the minimum 4-nibble unique prefix
        assert_eq!(view.unique_prefix_len(&c2.id), 4);

        // Appending a 3rd commit in the same 0xABCD bucket that shares 6 hex nibbles with c0
        // ("abcde1") must expand both c0 and c3 to the maximum 7-nibble display prefix.
        let mut c3 = make_test_commit("colliding 6-char prefix commit");
        c3.id = ObjectId::from_hex(b"abcde1f123456789abcdef0123456789abcdef01").unwrap();
        view.append_commits(vec![c3.clone()]);
        assert_eq!(view.unique_prefix_len(&c0.id), 7);
        assert_eq!(view.unique_prefix_len(&c3.id), 7);
        assert_eq!(view.unique_prefix_len(&c1.id), 6);
    }
}
