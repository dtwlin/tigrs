// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Domain types for Git objects, commits, and repository metadata.

use gix::ObjectId;
use smallvec::SmallVec;
use std::path::PathBuf;
use std::sync::Arc;

/// Parent object IDs of a commit, stored inline for the common arity.
///
/// Root commits have zero parents, ordinary commits one, and conventional
/// merges two; octopus merges (three or more) are vanishingly rare. Sizing the
/// inline buffer at two therefore keeps parent storage entirely within the
/// [`CommitSummary`] itself for effectively the whole of a repository's
/// history, while still degrading gracefully to the heap for octopus merges.
pub type ParentIds = SmallVec<[ObjectId; 2]>;

/// Summary information for a single Git commit, optimized for TUI list views.
///
/// # Memory layout
///
/// Instances of this type are retained for the entire loaded history, so the
/// footprint is kept deliberately small:
///
/// - `author_name` is an [`Arc<str>`] interned across the whole revwalk. Large
///   repositories have orders of magnitude more commits than distinct authors
///   (the Linux kernel has ~1.34M commits but only ~30k authors), so interning
///   collapses the dominant per-commit string cost into one shared allocation.
/// - `summary` is a [`Box<str>`], saving the capacity word a `String` carries.
/// - `parents` is a [`ParentIds`] with inline capacity for two entries, so
///   streaming a large history performs no per-commit heap allocation for
///   parent IDs at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSummary {
    /// Object ID (hash) of the commit.
    pub id: ObjectId,

    /// Parent commit IDs (used for graph rendering and merge detection).
    pub parents: ParentIds,

    /// Sanitized author name, interned and shared across commits.
    pub author_name: Arc<str>,

    /// Author timestamp in seconds since Unix epoch.
    pub author_time_secs: i64,

    /// Sanitized first line of commit message (subject).
    pub summary: Box<str>,
}

impl CommitSummary {
    /// Formats the abbreviated 7-character hexadecimal hash into `buf` with zero heap allocations.
    #[inline]
    pub fn short_id_buf<'a>(&self, buf: &'a mut [u8; 7]) -> &'a str {
        const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
        let bytes = self.id.as_bytes();
        if bytes.len() >= 4 {
            buf[0] = HEX_CHARS[(bytes[0] >> 4) as usize];
            buf[1] = HEX_CHARS[(bytes[0] & 0x0f) as usize];
            buf[2] = HEX_CHARS[(bytes[1] >> 4) as usize];
            buf[3] = HEX_CHARS[(bytes[1] & 0x0f) as usize];
            buf[4] = HEX_CHARS[(bytes[2] >> 4) as usize];
            buf[5] = HEX_CHARS[(bytes[2] & 0x0f) as usize];
            buf[6] = HEX_CHARS[(bytes[3] >> 4) as usize];
            std::str::from_utf8(buf).unwrap_or("")
        } else {
            ""
        }
    }

    /// Returns the abbreviated 7-character hexadecimal hash.
    #[inline]
    pub fn short_id(&self) -> String {
        let mut buf = [0u8; 7];
        self.short_id_buf(&mut buf).to_string()
    }

    /// Returns `true` if this commit has more than one parent (a merge commit).
    #[inline]
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    /// Formats the commit timestamp into a human-readable relative date string.
    #[inline]
    pub fn format_relative_date(&self, now_secs: i64) -> String {
        format_relative_date(self.author_time_secs, now_secs)
    }
}

/// Formats `time_secs` relative to `now_secs` as a human-readable age string.
///
/// Used both for commit rows and for synthetic rows (such as the uncommitted
/// changes entries in the main view) that are not backed by a [`CommitSummary`].
#[must_use]
pub fn format_relative_date(time_secs: i64, now_secs: i64) -> String {
    let diff = now_secs - time_secs;
    if diff < 0 {
        return "in the future".to_string();
    }
    if diff < 60 {
        return format!("{diff}s ago");
    }
    let minutes = diff / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{days}d ago");
    }
    let months = days / 30;
    if months < 12 {
        return format!("{months}mo ago");
    }
    let years = days / 365;
    format!("{years}y ago")
}

/// Formats a Unix epoch timestamp (seconds) into `YYYY-MM-DD` in a 10-byte stack buffer with zero heap allocations.
#[allow(clippy::similar_names)]
pub fn format_epoch_date_buf(secs: i64, buf: &mut [u8; 10]) -> &str {
    let days = (secs / 86_400) + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    let y_clamped = y.clamp(0, 9999) as u32;
    buf[0] = b'0' + ((y_clamped / 1000) % 10) as u8;
    buf[1] = b'0' + ((y_clamped / 100) % 10) as u8;
    buf[2] = b'0' + ((y_clamped / 10) % 10) as u8;
    buf[3] = b'0' + (y_clamped % 10) as u8;
    buf[4] = b'-';
    buf[5] = b'0' + ((m / 10) % 10) as u8;
    buf[6] = b'0' + (m % 10) as u8;
    buf[7] = b'-';
    buf[8] = b'0' + ((d / 10) % 10) as u8;
    buf[9] = b'0' + (d % 10) as u8;
    std::str::from_utf8(buf).unwrap_or("1970-01-01")
}

/// Converts a timestamp in seconds to formatted date string (`YYYY-MM-DD` or `YYYY-MM-DD HH:MM`).
///
/// Uses [`format_epoch_date_buf`] for standard short civil dates (`YYYY-MM-DD`)
/// and [`gix::date::Time`] for ISO 8601 formatting.
#[must_use]
pub fn format_timestamp(secs: i64, iso: bool) -> String {
    if iso {
        let time = gix::date::Time::new(secs.max(0), 0);
        let formatted = time.format_or_unix(gix::date::time::format::ISO8601);
        if formatted.len() >= 16 {
            formatted[..16].to_string()
        } else {
            formatted
        }
    } else {
        let mut buf = [0u8; 10];
        format_epoch_date_buf(secs.max(0), &mut buf).to_string()
    }
}

/// Metadata and path configuration for an opened Git repository.
#[derive(Debug, Clone)]
pub struct RepoInfo {
    /// The `.git` directory path (or gitfile target).
    pub git_dir: PathBuf,

    /// The common git directory path (holding objects, config, packed-refs).
    pub common_dir: PathBuf,

    /// The working tree root directory, if non-bare.
    pub work_dir: Option<PathBuf>,

    /// True if the repository format is reftable (Git >= 2.45).
    pub is_reftable: bool,

    /// True if the repository is bare (no working tree).
    pub is_bare: bool,
}

/// Kind of Git reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// Local branch under refs/heads/.
    LocalBranch,
    /// Remote tracking branch under refs/remotes/.
    RemoteBranch,
    /// Tag under refs/tags/.
    Tag,
    /// Stash under refs/stash.
    Stash,
    /// Other or custom reference.
    Other,
}

/// A Git reference entry for listing in the refs view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefEntry {
    /// Full ref path (e.g. `refs/heads/main`).
    pub full_name: String,
    /// Display name (e.g. `main` or `origin/main` or `v1.0`).
    pub name: String,
    /// Kind of reference.
    pub kind: RefKind,
    /// Target commit object ID.
    pub commit_id: ObjectId,
    /// First line of commit message.
    pub summary: String,
    /// Commit author name.
    pub author_name: String,
    /// Author timestamp.
    pub author_time_secs: i64,
}

/// A stash entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// 0-based stash index (`stash@{0}`, `stash@{1}`, etc.).
    pub index: usize,
    /// Commit object ID of the stash.
    pub commit_id: ObjectId,
    /// Description message of the stash.
    pub summary: String,
    /// Timestamp of the stash commit.
    pub time_secs: i64,
}

/// A reflog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    /// 0-based index (`HEAD@{0}`, `HEAD@{1}`, etc.).
    pub index: usize,
    /// Old commit ID before the transition.
    pub old_id: ObjectId,
    /// New commit ID after the transition.
    pub new_id: ObjectId,
    /// Committer name who caused the transition.
    pub committer_name: String,
    /// Timestamp of transition.
    pub time_secs: i64,
    /// Reflog message describing the action.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy(ts: i64) -> CommitSummary {
        CommitSummary {
            id: ObjectId::empty_tree(gix::hash::Kind::Sha1),
            parents: ParentIds::new(),
            author_name: Arc::from("Tester"),
            author_time_secs: ts,
            summary: Box::from("test"),
        }
    }

    #[test]
    fn test_format_relative_date() {
        let now = 1_000_000;
        assert_eq!(dummy(now - 15).format_relative_date(now), "15s ago");
        assert_eq!(dummy(now - 180).format_relative_date(now), "3m ago");
        assert_eq!(dummy(now - 7200).format_relative_date(now), "2h ago");
        assert_eq!(dummy(now - 86400 * 3).format_relative_date(now), "3d ago");
        assert_eq!(dummy(now - 86400 * 65).format_relative_date(now), "2mo ago");
        assert_eq!(dummy(now - 86400 * 400).format_relative_date(now), "1y ago");
        assert_eq!(dummy(now + 10).format_relative_date(now), "in the future");
    }

    #[test]
    fn test_short_id() {
        let c = dummy(0);
        let full_hex = c.id.to_hex().to_string();
        assert_eq!(c.short_id(), &full_hex[..7]);

        let mut buf = [0u8; 7];
        let buf_str = c.short_id_buf(&mut buf);
        assert_eq!(buf_str, &full_hex[..7]);
        assert_eq!(buf_str, c.short_id().as_str());
    }

    #[test]
    fn test_is_merge() {
        let oid = ObjectId::empty_tree(gix::hash::Kind::Sha1);
        let mut c = dummy(0);
        assert!(!c.is_merge());
        c.parents = smallvec::smallvec![oid];
        assert!(!c.is_merge());
        c.parents = smallvec::smallvec![oid, oid];
        assert!(c.is_merge());
    }

    /// Guards the per-commit footprint, which is multiplied by the full history
    /// length. A regression here directly translates into hundreds of megabytes
    /// of RSS on large repositories.
    ///
    /// Current layout: `ObjectId` 24 (21 padded to pointer alignment) +
    /// `ParentIds` 56 (8 capacity word + 48 inline buffer for two padded
    /// `ObjectId`s) + `Arc<str>` 16 + `i64` 8 + `Box<str>` 16 = 120.
    ///
    /// The inline parent buffer is larger than the 16-byte `Box<[ObjectId]>`
    /// it replaced, but it is a net win: the boxed slice still had to store the
    /// same parent IDs on the heap, where a one-parent commit consumed a
    /// 32-byte malloc bucket plus allocator bookkeeping. Inlining trades that
    /// for zero allocator traffic across the entire revwalk.
    #[test]
    fn test_commit_summary_is_compact() {
        let size = std::mem::size_of::<CommitSummary>();
        assert!(
            size <= 120,
            "CommitSummary grew to {size} bytes; it is retained once per commit \
             across the entire history, so keep it compact"
        );
    }

    /// The inline capacity is the entire point of [`ParentIds`]: commits with
    /// up to two parents must not touch the allocator.
    #[test]
    fn test_parent_ids_stay_inline_up_to_two_parents() {
        let oid = ObjectId::empty_tree(gix::hash::Kind::Sha1);

        let mut parents = ParentIds::new();
        assert!(!parents.spilled(), "empty parent list spilled to the heap");
        parents.push(oid);
        assert!(
            !parents.spilled(),
            "single-parent commit spilled to the heap"
        );
        parents.push(oid);
        assert!(!parents.spilled(), "two-parent merge spilled to the heap");

        // Octopus merges are rare enough that spilling is the correct tradeoff.
        parents.push(oid);
        assert!(parents.spilled());
        assert_eq!(parents.len(), 3);
    }

    #[test]
    fn test_format_timestamp() {
        // Unix epoch timestamp: 1726214745 (2024-09-13 08:05:45 UTC)
        assert_eq!(format_timestamp(1_726_214_745, false), "2024-09-13");
        assert_eq!(format_timestamp(1_726_214_745, true), "2024-09-13 08:05");

        // Epoch 0
        assert_eq!(format_timestamp(0, false), "1970-01-01");
        assert_eq!(format_timestamp(0, true), "1970-01-01 00:00");

        // Negative clamp to 0
        assert_eq!(format_timestamp(-1234, false), "1970-01-01");
        assert_eq!(format_timestamp(-1234, true), "1970-01-01 00:00");
    }

    #[test]
    fn test_format_epoch_date_buf() {
        let mut buf = [0u8; 10];
        assert_eq!(format_epoch_date_buf(0, &mut buf), "1970-01-01");
        assert_eq!(format_epoch_date_buf(1_726_214_745, &mut buf), "2024-09-13");
        // Leap day 2024-02-29 (1709164800)
        assert_eq!(format_epoch_date_buf(1_709_164_800, &mut buf), "2024-02-29");
    }
}
