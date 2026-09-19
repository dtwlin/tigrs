// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Asynchronous worker task payloads, request slots, and cancellation management.

use super::layout::ViewKind;
use crate::view::{ChangesKind, DiffView};
use std::sync::Arc;
use tigrs_core::cancel::{CancellationSource, CancellationToken};
use tigrs_core::error::Result;

/// Target of a debounced or background diff computation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiffRequestTarget {
    /// Commit diff by object ID.
    Commit(tigrs_git::ObjectId),
    /// Working-tree status section diff (`Staged`, `Unstaged`, or `Untracked`).
    Changes(ChangesKind),
    /// Individual working-tree status item diff in `StatusView`.
    StatusItem(tigrs_git::StatusItem),
}

impl DiffRequestTarget {
    /// Returns the commit ID if this target is a `DiffRequestTarget::Commit`.
    #[inline]
    #[must_use]
    pub const fn commit_oid(&self) -> Option<tigrs_git::ObjectId> {
        match self {
            Self::Commit(oid) => Some(*oid),
            Self::Changes(_) | Self::StatusItem(_) => None,
        }
    }

    /// Returns the `ChangesKind` if this target is a `DiffRequestTarget::Changes`.
    #[inline]
    #[must_use]
    pub const fn changes_kind(&self) -> Option<ChangesKind> {
        match self {
            Self::Changes(kind) => Some(*kind),
            Self::Commit(_) | Self::StatusItem(_) => None,
        }
    }

    /// Returns the `StatusItem` if this target is a `DiffRequestTarget::StatusItem`.
    #[inline]
    #[must_use]
    pub const fn status_item(&self) -> Option<&tigrs_git::StatusItem> {
        match self {
            Self::StatusItem(item) => Some(item),
            Self::Commit(_) | Self::Changes(_) => None,
        }
    }
}

impl PartialEq<tigrs_git::ObjectId> for DiffRequestTarget {
    fn eq(&self, other: &tigrs_git::ObjectId) -> bool {
        matches!(self, Self::Commit(oid) if oid == other)
    }
}

/// Result of an asynchronous commit or working-tree diff computation.
#[derive(Debug)]
pub struct DiffWorkerResponse {
    /// Algebraic target that produced this diff (`Commit`, `Changes`, or `StatusItem`).
    pub target: DiffRequestTarget,
    /// Render key used when building `precomputed_view`, for `DiffDocumentCache` insertion.
    pub render_key: Option<crate::diff::DiffRenderKey>,
    /// Whether this response originated from background speculative prefetch (`true`) rather than an active UI request.
    pub is_prefetch: bool,
    /// Generation counter when `Changes` or `StatusItem` was computed, for cache insertion.
    pub generation: u64,
    /// Monotonic request ID for epoch tracking.
    pub request_id: u64,
    /// Resulting diff (`Arc<CommitDiff>`) or engine error.
    pub result: Result<Arc<tigrs_git::CommitDiff>>,
    /// Optional pre-built `DiffView` constructed off-thread on the worker pool.
    pub precomputed_view: Option<DiffView>,
}

/// Navigation context for an asynchronous file blame computation.
#[derive(Debug, Clone)]
pub enum BlameNavKind {
    /// Initial blame load or direct open.
    Initial,
    /// Navigating to parent commit (`Parent` / `,` action) with target cursor row.
    Parent {
        /// Target parent commit object ID.
        commit_id: tigrs_git::ObjectId,
        /// Repository-relative file path at the parent revision.
        path: String,
        /// Target 0-based cursor row to focus after loading.
        cursor: usize,
        /// Current viewport height in rows for scroll clamping.
        visible_height: usize,
    },
    /// Navigating back in history (`Back` action) to a previous history entry.
    Back {
        /// Previous blame navigation stack entry to restore.
        prev_entry: crate::view::BlameHistoryEntry,
        /// Current viewport height in rows for scroll clamping.
        visible_height: usize,
    },
}

/// Result of an asynchronous file blame computation.
#[derive(Debug)]
pub struct BlameWorkerResponse {
    /// Target commit object ID (if any).
    pub commit_id: Option<tigrs_git::ObjectId>,
    /// Target file path.
    pub path: String,
    /// Monotonic request ID for epoch tracking.
    pub request_id: u64,
    /// Navigation context (initial, parent, or back).
    pub nav: BlameNavKind,
    /// Resulting blame or engine error.
    pub result: Result<tigrs_git::BlameResult>,
}

/// Result batch from asynchronous `LogView` diffstat streaming.
#[derive(Debug)]
pub struct LogWorkerResponse {
    /// Monotonic request ID for epoch tracking.
    pub request_id: u64,
    /// Batch of computed commit diffs and optional ref names.
    pub diffs: Vec<(Arc<tigrs_git::CommitDiff>, Option<Vec<String>>)>,
}

/// Single-slot cancellation handle that automatically cancels any superseded
/// task when reset or cleared.
#[derive(Default)]
pub struct CancelSlot(Option<CancellationSource>);

impl CancelSlot {
    /// Cancels any active task in this slot and returns a fresh [`CancellationToken`].
    #[inline]
    pub fn reset(&mut self) -> CancellationToken {
        self.cancel();
        let (src, tok) = CancellationToken::new();
        self.0 = Some(src);
        tok
    }

    /// Cancels any active task in this slot without allocating a new token.
    #[inline]
    pub fn cancel(&mut self) {
        if let Some(old) = self.0.take() {
            old.cancel();
        }
    }
}

/// Encapsulates channel sender, monotonic request ID, and cancellation source
/// for an asynchronous background task domain.
pub struct AsyncTaskSlot<T> {
    /// Channel sender used by background workers to deliver results back to the UI loop.
    pub tx: Option<crossbeam_channel::Sender<T>>,
    /// Monotonically increasing epoch ID for the currently active request.
    pub active_request_id: u64,
    /// Active cancellation slot for aborting an in-flight worker task.
    pub cancel: CancelSlot,
}

impl<T> Default for AsyncTaskSlot<T> {
    fn default() -> Self {
        Self {
            tx: None,
            active_request_id: 0,
            cancel: CancelSlot::default(),
        }
    }
}

impl<T> AsyncTaskSlot<T> {
    /// Returns whether a channel sender is configured for this slot.
    #[inline]
    #[must_use]
    pub fn has_sender(&self) -> bool {
        self.tx.is_some()
    }

    /// Cancels any active request in this slot, increments the monotonic request ID,
    /// and returns `(request_id, cancellation_token, sender)`.
    pub fn begin_request(
        &mut self,
    ) -> (u64, CancellationToken, Option<crossbeam_channel::Sender<T>>) {
        self.cancel_in_flight();
        let tok = self.cancel.reset();
        (self.active_request_id, tok, self.tx.clone())
    }

    /// Cancels any active request in this slot and returns a fresh `CancellationToken`
    /// without incrementing `active_request_id`.
    #[inline]
    pub fn reset_cancel(&mut self) -> CancellationToken {
        self.cancel.reset()
    }

    /// Cancels any in-flight request in this slot and invalidates its `request_id`.
    #[inline]
    pub fn cancel_in_flight(&mut self) {
        self.cancel.cancel();
        self.active_request_id = self.active_request_id.wrapping_add(1);
    }

    /// Returns whether `request_id` matches the currently active request in this slot.
    #[inline]
    #[must_use]
    pub fn is_current(&self, request_id: u64) -> bool {
        self.active_request_id == request_id
    }
}

/// Manages active [`CancelSlot`] handles per background task domain,
/// automatically cancelling superseded tasks when a newer request is dispatched.
#[derive(Default)]
pub struct ActiveTaskCancels {
    /// Active cancellation slot for commit or working-tree diff computation.
    pub diff_cancel: CancelSlot,
    /// Active cancellation slot for file blame computation.
    pub blame_cancel: CancelSlot,
    /// Active cancellation slot for working-tree status scanning.
    pub status_cancel: CancelSlot,
    /// Active cancellation slot for `LogView` diffstat streaming.
    pub log_cancel: CancelSlot,
    /// Active cancellation slot for idle speculative diff prefetching.
    pub prefetch_cancel: CancelSlot,
    /// Active cancellation slot for background uncommitted-section diff warmup.
    pub changes_warmup_cancel: CancelSlot,
}

impl ActiveTaskCancels {
    /// Cancels any previous status scan and returns a fresh `CancellationToken`.
    #[inline]
    pub fn reset_status(&mut self) -> CancellationToken {
        self.status_cancel.reset()
    }

    /// Cancels any previous speculative prefetch and returns a fresh `CancellationToken`.
    #[inline]
    pub fn reset_prefetch(&mut self) -> CancellationToken {
        self.prefetch_cancel.reset()
    }

    /// Cancels any previous uncommitted-section diff warmup and returns a fresh `CancellationToken`.
    #[inline]
    pub fn reset_changes_warmup(&mut self) -> CancellationToken {
        self.changes_warmup_cancel.reset()
    }

    /// Cancels any in-flight speculative prefetch.
    #[inline]
    pub fn cancel_prefetch(&mut self) {
        self.prefetch_cancel.cancel();
    }

    /// Cancels background tasks associated with a specific view kind when closed.
    pub fn cancel_for_view(&mut self, kind: ViewKind) {
        match kind {
            ViewKind::Diff => self.diff_cancel.cancel(),
            ViewKind::Blame => self.blame_cancel.cancel(),
            ViewKind::Status => self.status_cancel.cancel(),
            ViewKind::Log => self.log_cancel.cancel(),
            _ => {}
        }
    }

    /// Cancels all active background task sources.
    pub fn cancel_all(&mut self) {
        self.diff_cancel.cancel();
        self.blame_cancel.cancel();
        self.status_cancel.cancel();
        self.log_cancel.cancel();
        self.prefetch_cancel.cancel();
        self.changes_warmup_cancel.cancel();
    }
}
