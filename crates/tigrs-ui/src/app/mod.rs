// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Main TUI application lifecycle, event loop, and render coordination.
//!
//! The loop multiplexes three independent sources - streamed commits, POSIX
//! signals and terminal input - over a single [`crossbeam_channel::select`].
//!
//! An earlier revision polled the terminal with a timeout and checked the other
//! sources around it. That serialised the sources: a commit batch arriving just
//! after the loop parked in `event::poll` was not drawn until the poll expired,
//! adding up to a full frame interval of latency per batch and roughly 33 ms to
//! time-to-first-frame. Blocking on all sources at once removes that coupling
//! and still leaves the process at 0% CPU while idle.

use crate::keymap::{Key, KeymapEngine, KeymapLookupResult, KeymapScope, RunCommand, RunFlags};
use crate::options::ViewOptions;
use crate::prompt::{ParsedCommand, PromptKind, PromptResult, PromptState};
use crate::search::{ActiveSearch, SearchDirection};
use crate::signal::{SignalCoordinator, UiSignal};
use crate::tty::TtyController;
use crate::view::{
    BlameView, BlobView, ChangesKind, ChangesRow, DiffView, GrepView, HelpView, LogView, MainView,
    PagerView, ReflogView, RefsView, StashView, StatusView, TreeView, View,
};
use crossbeam_channel::{Receiver, Sender, bounded, select};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tigrs_core::cancel::{CancellationSource, CancellationToken};
use tigrs_core::error::Result;
use tigrs_core::history::HistoryManager;
use tigrs_core::macro_ctx::MacroContext;
use tigrs_git::{CommitSummary, GitEngine, StatusItem};

/// Target frame duration when coalescing repaints during streaming (~60 FPS).
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Debounce window for asynchronous commit diff computation during navigation (~30 ms).
pub const DIFF_DEBOUNCE_INTERVAL: Duration = Duration::from_millis(30);

/// Debounce window for filesystem watcher events (~150 ms) to coalesce rapid bursts.
pub const WATCHER_DEBOUNCE_INTERVAL: Duration = Duration::from_millis(150);

/// Speculative prefetch interval before triggering background diff prefetch on idle (~50 ms).
pub const SPECULATIVE_PREFETCH_INTERVAL: Duration = Duration::from_millis(50);

/// Sleep used when nothing is pending. Long enough to be effectively idle; any
/// of the three sources wakes the loop immediately regardless.
const IDLE_WAIT: Duration = Duration::from_secs(3600);

pub mod actions;
pub mod commands;
pub mod dispatch;
pub mod layout;
pub mod render;
pub mod tasks;
pub mod view_manager;

#[cfg(test)]
mod tests;

pub use actions::*;
pub use commands::execute_parsed_command;
pub use commands::*;
pub use dispatch::*;
pub use layout::*;
pub use render::*;
pub use tasks::*;
pub use view_manager::*;

/// Application state holding active views and Git engine.
#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppState {
    /// Owned manager for all 13 canonical views, view navigation stack, and split-window layout flags.
    pub views: ViewManager,
    /// Git engine for resolving commit diffs and status.
    pub engine: Option<GitEngine>,
    /// Keymap engine managing keybindings.
    pub keymap: KeymapEngine,
    /// Pending key sequence currently being typed.
    pub pending_keys: Vec<Key>,
    /// Active prompt line state machine, if a prompt is open.
    pub prompt: Option<PromptState>,
    /// Persistent command and search history manager.
    pub history: HistoryManager,
    /// Last status message displayed on the status bar.
    pub status_message: Option<String>,
    /// Active search state containing query pattern and direction.
    pub active_search: Option<ActiveSearch>,
    /// Display options and configuration toggles.
    pub options: ViewOptions,
    /// Negotiated terminal capabilities.
    pub caps: crate::term_cap::TerminalCapabilities,
    /// Pending external command requiring terminal handover execution.
    pub pending_handover: Option<(String, RunCommand)>,
    /// Async task slot for background blame computations.
    pub blame_task: AsyncTaskSlot<BlameWorkerResponse>,
    /// Async task slot for background commit diff computations.
    pub diff_task: AsyncTaskSlot<DiffWorkerResponse>,
    /// Async task slot for background log diffstat streaming.
    pub log_task: AsyncTaskSlot<LogWorkerResponse>,
    /// Active cancellation sources per background task category.
    pub cancels: ActiveTaskCancels,
    /// Pending commit or section diff target awaiting debounce expiry: (`target`, deadline, `request_id`).
    pub pending_diff_target: Option<(DiffRequestTarget, Instant, u64)>,
    /// Most recent worktree status scan, used to build the main view's
    /// uncommitted-changes rows.
    ///
    /// Cached so that toggling `show-changes` or `show-untracked` can rebuild
    /// the rows without paying for another scan.
    pub changes_report: Option<tigrs_git::StatusReport>,
    /// Generation-scoped cache of computed uncommitted section diffs and their `DiffView`s.
    pub changes_diff_cache:
        std::collections::HashMap<(ChangesKind, u64), (Arc<tigrs_git::CommitDiff>, DiffView)>,
    /// Generation-scoped cache of computed per-file `StatusItem` `DiffView`s for instant `StatusView` navigation.
    pub status_item_diff_cache: std::collections::HashMap<
        (tigrs_git::StatusItem, u64, crate::diff::DiffRenderKey),
        DiffView,
    >,
    /// Bounded LRU cache of laid-out `Arc<DiffDocument>`s keyed by `(ObjectId, DiffRenderKey)`.
    pub diff_document_cache: std::cell::RefCell<crate::diff::DiffDocumentCache>,
    /// Stateful double-buffered terminal renderer (eliminating per-frame grid allocations).
    pub renderer: std::cell::RefCell<TerminalRenderer>,
    /// Infallible atomic screen-invalidation flag that guarantees full-screen repaint
    /// even if `invalidate_screen()` is called while `renderer` is borrowed.
    pub screen_dirty: std::sync::atomic::AtomicBool,
    /// Whether debug frame statistics are logged.
    pub debug_frame_stats: bool,
    /// Pending speculative diff prefetch target awaiting idle debounce expiry: (`commit_id`, deadline).
    pub pending_speculative_prefetch: Option<(tigrs_git::ObjectId, Instant)>,
    /// Last terminal size observed by the event loop, used to resolve the pane
    /// geometry (and therefore the scroll height) of views other than the active one.
    pub last_terminal_size: Option<(u16, u16)>,
    /// Pending external editor invocation requiring terminal handover execution.
    pub pending_editor: Option<crate::editor::EditorInvocation>,
    /// Session-scoped runtime override that disables line numbers if an editor fails.
    pub editor_line_number_disabled: bool,
    /// Value of `core.editor` cached from git config at startup.
    pub core_editor: Option<String>,
    /// Security trust policy governing repository-local execution boundaries.
    pub security: tigrs_core::SecurityConfig,
    /// Snapshot of `ViewOptions` as last loaded or saved to `config.toml` (for `●` unsaved indicators).
    pub saved_options_baseline: ViewOptions,
    /// Active target `config.toml` path for saving configuration (`None` falls back to `Config::default_config_path()`).
    pub config_file_path: Option<std::path::PathBuf>,
    /// Deserialized `Config` retaining `[security]`, `[keybindings]`, and `[colors]` sections across saves.
    pub loaded_config: tigrs_core::Config,
    /// When `true` (while draining queued input bursts in the event loop), `OptionEffect::RefreshDiff`
    /// and `OptionEffect::RefreshSyntax` set pending flags instead of rebuilding synchronously on every keystroke.
    pub defer_view_refresh: bool,
    /// Coalesced flag indicating `refresh_diff_view()` must be executed before rendering or non-option actions.
    pub pending_diff_refresh: bool,
    /// Coalesced flag indicating `blob_view` / `blame_view` syntax highlighting must be refreshed.
    pub pending_syntax_refresh: bool,
    /// Pending `Ctrl-Z` / `suspend` action requiring job-control stop on the UI thread.
    pub pending_suspend: bool,
    /// Explicit CLI `--read-only` / `--update-mode` override preserved across `:source`.
    pub cli_read_only_override: Option<bool>,
}

/// Status bar warning displayed when the user attempts a repository-modifying action in Read-Only Mode.
pub const READ_ONLY_WARNING_MSG: &str = "Read-only mode: repository modifications are disabled. Run ':set read-only = false' (or ':update-mode') to enable updates.";

impl AppState {
    /// Returns `true` when the application is currently in Read-Only Mode (`options.read_only == true`).
    #[inline]
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.options.read_only
    }

    /// Synchronizes the `GitEngine` atomic `read_only` flag with `self.options.read_only`.
    pub fn sync_read_only_state(&mut self) {
        if let Some(ref eng) = self.engine {
            eng.set_read_only(self.options.read_only);
        }
    }

    /// Checks whether the application is in Read-Only Mode. If so, sets [`READ_ONLY_WARNING_MSG`]
    /// on the status bar, invalidates the screen, and returns `true` (indicating the action must be blocked).
    pub fn check_read_only_blocked(&mut self, _action_desc: &str) -> bool {
        if self.is_read_only() {
            self.status_message = Some(READ_ONLY_WARNING_MSG.to_string());
            self.invalidate_screen();
            true
        } else {
            false
        }
    }

    /// Returns `true` if `candidate` resolves inside the active repository's working tree or `.git` metadata directory,
    /// even when `candidate` or its parent directories do not yet exist on disk.
    #[must_use]
    pub fn is_path_inside_repo(&self, candidate: &std::path::Path) -> bool {
        let Some(ref eng) = self.engine else {
            return false;
        };
        let work_dir = eng.work_dir();
        let git_dir = &eng.info().git_dir;
        let work_canon = work_dir
            .canonicalize()
            .unwrap_or_else(|_| work_dir.to_path_buf());
        let git_canon = git_dir.canonicalize().unwrap_or_else(|_| git_dir.clone());

        let cand_canon = canonicalize_existing_prefix(candidate);
        if cand_canon.starts_with(&work_canon) || cand_canon.starts_with(&git_canon) {
            return true;
        }
        if candidate.is_relative() {
            let rel_to_work = canonicalize_existing_prefix(&work_dir.join(candidate));
            if rel_to_work.starts_with(&work_canon) || rel_to_work.starts_with(&git_canon) {
                return true;
            }
        }
        false
    }

    /// Returns `true` if and only if BOTH the working tree directory AND every active
    /// Git metadata directory (`git_dir`, `.git` gitfile targets, `commondir`, `GIT_DIR`,
    /// and `GIT_COMMON_DIR`) satisfy the user's `security` policy.
    #[must_use]
    pub fn is_current_repo_trusted(&self, work_dir: &std::path::Path) -> bool {
        if !self.security.is_repo_trusted(work_dir) {
            return false;
        }
        if let Some(ref eng) = self.engine
            && (!self.security.is_repo_trusted(eng.work_dir())
                || !self.security.is_repo_trusted(&eng.info().git_dir))
        {
            return false;
        }
        let meta_dirs = tigrs_git::discover_git_metadata_dirs(work_dir);
        !meta_dirs.is_empty() && meta_dirs.iter().all(|d| self.security.is_repo_trusted(d))
    }

    /// Returns an immutable reference to the owned [`ViewManager`] component.
    #[inline]
    #[must_use]
    pub fn views(&self) -> &ViewManager {
        &self.views
    }

    /// Returns a mutable reference to the owned [`ViewManager`] component.
    #[inline]
    #[must_use]
    pub fn views_mut(&mut self) -> &mut ViewManager {
        &mut self.views
    }

    /// Returns the (primary, secondary) pair displayed in a split layout.
    #[inline]
    #[must_use]
    pub fn split_views(&self) -> Option<(ViewKind, ViewKind)> {
        self.views.split_views()
    }

    /// Drops stack entries whose views are gone and seeds an empty stack.
    #[inline]
    pub fn ensure_view_stack(&mut self) {
        self.views.ensure_view_stack();
    }

    /// Returns the active view of `kind` as a polymorphic trait object.
    #[inline]
    #[must_use]
    pub fn view_ref(&self, kind: ViewKind) -> Option<&dyn View> {
        self.views.view_ref(kind)
    }

    /// Returns the active view of `kind` as a mutable polymorphic trait object.
    #[inline]
    #[must_use]
    pub fn view_mut(&mut self, kind: ViewKind) -> Option<&mut dyn View> {
        self.views.view_mut(kind)
    }

    /// Synchronizes cache capacities in `GitEngine` and `DiffDocumentCache` with `options.memory_profile`.
    pub fn sync_memory_profile(&mut self) {
        let profile = self.options.memory_profile;
        self.diff_document_cache.borrow_mut().reconfigure(profile);
        if let Some(ref mut eng) = self.engine {
            eng.set_memory_profile(profile);
        }
    }

    /// Applies the view refresh side effect required after an option modification.
    pub fn apply_option_effect(&mut self, effect: crate::options::OptionEffect) {
        use crate::options::OptionEffect;
        self.sync_memory_profile();
        match effect {
            OptionEffect::None => {}
            OptionEffect::RefreshChanges => {
                self.apply_changes_rows();
            }
            OptionEffect::RefreshDiff => {
                if self.defer_view_refresh {
                    self.pending_diff_refresh = true;
                } else {
                    self.refresh_diff_view();
                }
            }
            OptionEffect::RefreshSyntax => {
                self.invalidate_screen();
                if self.defer_view_refresh {
                    self.pending_diff_refresh = true;
                    self.pending_syntax_refresh = true;
                } else {
                    self.refresh_diff_view();
                    if let Some(ref mut blob) = self.views.blob_view {
                        blob.refresh_highlighting(&self.options);
                    }
                    if let Some(ref mut blame) = self.views.blame_view {
                        blame.refresh_highlighting(&self.options);
                    }
                }
            }
        }
    }

    /// Flushes any coalesced `RefreshDiff` or `RefreshSyntax` side effects accumulated
    /// during an input drain batch (`]` / `[` key-repeat bursts).
    pub fn flush_deferred_view_refresh(&mut self) {
        let do_diff = std::mem::take(&mut self.pending_diff_refresh);
        let do_syntax = std::mem::take(&mut self.pending_syntax_refresh);
        if do_diff {
            self.refresh_diff_view();
        }
        if do_syntax {
            if let Some(ref mut blob) = self.views.blob_view {
                blob.refresh_highlighting(&self.options);
            }
            if let Some(ref mut blame) = self.views.blame_view {
                blame.refresh_highlighting(&self.options);
            }
        }
    }

    /// Applies an option menu entry activation, setting the status message and
    /// refreshing only the affected views.
    pub fn apply_menu_action(&mut self, action: crate::options::MenuAction) {
        use crate::options::MenuAction;

        let (msg, effect) = self.options.apply_menu_action(action);
        self.sync_read_only_state();
        if matches!(action, MenuAction::ResetAll) {
            self.apply_changes_rows();
        }
        if matches!(
            action,
            MenuAction::Toggle(id) | MenuAction::Prev(id) | MenuAction::Reset(id)
                if id.descriptor().invalidates_screen
        ) || matches!(action, MenuAction::ResetAll)
        {
            self.invalidate_screen();
        }
        self.apply_option_effect(effect);
        self.status_message = Some(msg);
    }

    /// Saves the current runtime options to a TOML configuration file (`custom_path` or active `config.toml`),
    /// preserving `[security]`, `[keybindings]`, and `[colors]` sections.
    pub fn save_config_to_toml(&mut self, custom_path: Option<&str>, minimal: bool) {
        let resolved_path = if let Some(raw_p) = custom_path
            && !raw_p.trim().is_empty()
        {
            Some(tigrs_core::config::expand_tilde(raw_p.trim()))
        } else {
            self.config_file_path
                .clone()
                .or_else(tigrs_core::Config::default_config_path)
        };

        let Some(target_path) = resolved_path else {
            self.status_message = Some(
                "Failed to save config: no target path specified and $HOME is unset".to_string(),
            );
            return;
        };

        if self.is_read_only() && self.is_path_inside_repo(&target_path) {
            self.status_message = Some(READ_ONLY_WARNING_MSG.to_string());
            self.invalidate_screen();
            return;
        }

        self.options.write_into_config(&mut self.loaded_config);
        match self
            .loaded_config
            .save_to_path(&tigrs_core::Config::default(), &target_path, minimal)
        {
            Ok(count) => {
                self.saved_options_baseline = self.options.clone();
                self.config_file_path = Some(target_path.clone());
                let mode_str = if minimal { "modified" } else { "total" };
                self.status_message = Some(format!(
                    "Saved {count} {mode_str} option(s) to {}",
                    target_path.display()
                ));
            }
            Err(err) => {
                self.status_message = Some(format!("Failed to save config: {err}"));
            }
        }
    }

    /// Reloads runtime options and keybindings from the specified TOML file path (`:source <path>`).
    pub fn source_config_from_toml(&mut self, path_str: &str) {
        let target_path = tigrs_core::config::expand_tilde(path_str.trim());
        match tigrs_core::Config::load_from_file(&target_path) {
            Ok(cfg) => {
                let mut new_opts = ViewOptions::default();
                new_opts.apply_config(&cfg);
                new_opts = new_opts.with_capabilities(&self.caps);
                if let Some(ro_override) = self.cli_read_only_override {
                    new_opts.read_only = ro_override;
                }
                let _ = self.keymap.apply_config(&cfg.keybindings);
                self.options = new_opts.clone();
                self.saved_options_baseline = new_opts;
                self.loaded_config = cfg;
                self.config_file_path = Some(target_path.clone());
                self.sync_read_only_state();
                self.invalidate_screen();
                self.apply_changes_rows();
                self.apply_option_effect(crate::options::OptionEffect::RefreshSyntax);
                self.status_message = Some(format!(
                    "Sourced configuration from {}",
                    target_path.display()
                ));
            }
            Err(err) => {
                self.status_message = Some(format!("Failed to source config: {err}"));
            }
        }
    }

    /// Toggles an option by name and sets the status message, refreshing only affected views.
    pub fn toggle_option(&mut self, name: &str) {
        match self.options.toggle_by_name_with_effect(name) {
            Ok((msg, effect)) => {
                self.sync_read_only_state();
                self.invalidate_screen();
                self.status_message = Some(msg);
                self.apply_option_effect(effect);
            }
            Err(err) => {
                self.status_message = Some(err);
            }
        }
    }

    /// Invokes `f` with a repository blob line provider closure if `self.engine` is present.
    fn with_blob_provider<R>(
        &self,
        f: impl FnOnce(Option<&dyn Fn(tigrs_git::ObjectId) -> Option<Arc<[Arc<str>]>>>) -> R,
    ) -> R {
        if let Some(ref engine) = self.engine {
            let provider = |oid: tigrs_git::ObjectId| -> Option<Arc<[Arc<str>]>> {
                engine.read_blob_raw_lines(oid).ok()
            };
            f(Some(&provider))
        } else {
            f(None)
        }
    }

    /// Returns the active [`crate::diff::DiffRenderKey`] for `self.options` and `self.caps`.
    #[inline]
    #[must_use]
    pub fn current_diff_render_key(&self) -> crate::diff::DiffRenderKey {
        crate::diff::DiffRenderKey::from_options(&self.options, self.caps.color_profile)
    }

    /// Creates a new `DiffView` using current options, consulting `diff_document_cache` first
    /// to avoid repeating `syntect` highlighting and `word_diff` layout on visited/prefetched commits.
    pub fn create_diff_view(&self, diff: impl Into<Arc<tigrs_git::CommitDiff>>) -> DiffView {
        let diff_arc: Arc<tigrs_git::CommitDiff> = diff.into();
        let commit_id = diff_arc.commit_id;
        let render_key = self.current_diff_render_key();

        if !commit_id.is_null()
            && commit_id != tigrs_git::ObjectId::empty_tree(commit_id.kind())
            && let Some((cached_doc, is_ext)) = self
                .diff_document_cache
                .borrow_mut()
                .get(commit_id, &render_key)
        {
            return DiffView::from_arc_and_document(diff_arc, cached_doc, None, is_ext);
        }

        let (doc, is_ext) = self.with_blob_provider(|provider| {
            DiffView::build_documents(&diff_arc, &self.options, &self.caps, provider)
        });
        let doc_arc = Arc::new(doc);

        if !commit_id.is_null() && commit_id != tigrs_git::ObjectId::empty_tree(commit_id.kind()) {
            self.diff_document_cache.borrow_mut().insert(
                commit_id,
                render_key,
                Arc::clone(&doc_arc),
                is_ext,
            );
        }

        DiffView::from_arc_and_document(diff_arc, doc_arc, None, is_ext)
    }

    /// Creates a new status `DiffView` using current options and repository blob provider.
    pub fn create_status_diff_view(
        &self,
        diff: impl Into<Arc<tigrs_git::CommitDiff>>,
        item: StatusItem,
    ) -> DiffView {
        let diff_arc: Arc<tigrs_git::CommitDiff> = diff.into();
        let (doc, is_ext) = self.with_blob_provider(|provider| {
            DiffView::build_documents(&diff_arc, &self.options, &self.caps, provider)
        });
        DiffView::from_arc_and_document(diff_arc, Arc::new(doc), Some(item), is_ext)
    }

    /// Rebuilds the structured diff document in the current diff view (if any)
    /// using updated view options (e.g. diff-presentation, diff-layout, diff-context),
    /// reusing `diff_document_cache` in `< 20 µs` when toggling back to a previously rendered key.
    pub fn refresh_diff_view(&mut self) {
        self.changes_diff_cache.clear();
        if let Some(mut diff_view) = self.views.diff_view.take() {
            if diff_view.has_custom_fold_or_hunk_state() {
                let options = self.options.clone();
                self.with_blob_provider(|provider| {
                    diff_view.refresh(&options, provider);
                });
                self.views.diff_view = Some(diff_view);
                return;
            }

            let commit_id = diff_view.commit_id();
            let render_key = self.current_diff_render_key();
            let is_commit_diff = diff_view.status_item().is_none()
                && !commit_id.is_null()
                && commit_id != tigrs_git::ObjectId::empty_tree(commit_id.kind());

            if is_commit_diff
                && let Some((cached_doc, is_ext)) = self
                    .diff_document_cache
                    .borrow_mut()
                    .get(commit_id, &render_key)
            {
                diff_view.refresh_with_document(cached_doc, is_ext);
                self.views.diff_view = Some(diff_view);
                return;
            }

            let diff_arc = Arc::clone(diff_view.diff_arc());
            let (new_doc, is_ext) = self.with_blob_provider(|provider| {
                DiffView::build_documents(&diff_arc, &self.options, &self.caps, provider)
            });
            let doc_arc = Arc::new(new_doc);
            if is_commit_diff {
                self.diff_document_cache.borrow_mut().insert(
                    commit_id,
                    render_key,
                    Arc::clone(&doc_arc),
                    is_ext,
                );
            }
            diff_view.refresh_with_document(doc_arc, is_ext);
            self.views.diff_view = Some(diff_view);
        }
    }

    /// Rebuilds the main view's uncommitted-changes rows from the cached status
    /// report, honoring the current `show-changes` / `show-untracked` options.
    ///
    /// Returns `true` if the displayed rows actually changed.
    pub fn apply_changes_rows(&mut self) -> bool {
        let rows = self
            .changes_report
            .as_ref()
            .map_or_else(Vec::new, |report| {
                changes_rows_from_status(report, &self.options)
            });

        let Some(ref mut main) = self.views.main_view else {
            return false;
        };
        if main.changes() == rows.as_slice() {
            return false;
        }
        main.set_changes(rows);
        true
    }

    /// Pushes a view kind onto the stack.
    pub fn push_view(&mut self, kind: ViewKind) {
        self.pending_speculative_prefetch = None;
        self.cancels.cancel_prefetch();
        self.views.push_view(kind);
        self.invalidate_screen();
    }

    /// Cancels background tasks associated with a specific view kind when closed.
    pub fn cancel_for_view(&mut self, kind: ViewKind) {
        match kind {
            ViewKind::Diff => {
                self.pending_diff_target = None;
                self.diff_task.cancel_in_flight();
            }
            ViewKind::Blame => self.blame_task.cancel_in_flight(),
            ViewKind::Log => self.log_task.cancel_in_flight(),
            _ => {}
        }
        self.cancels.cancel_for_view(kind);
    }

    /// Cancels all active background tasks across all slots and cancel sources.
    pub fn cancel_all(&mut self) {
        self.diff_task.cancel_in_flight();
        self.blame_task.cancel_in_flight();
        self.log_task.cancel_in_flight();
        self.cancels.cancel_all();
    }

    /// Pops the active view from the stack and clears its field.
    pub fn pop_active_view(&mut self) -> Option<ViewKind> {
        self.pending_speculative_prefetch = None;
        self.cancels.cancel_prefetch();
        let popped = self.views.pop_active_view();
        if let Some(p) = popped {
            self.cancel_for_view(p);
        }
        self.invalidate_screen();
        popped
    }

    /// Computes the on-screen geometry of every currently displayed pane.
    ///
    /// Returns `None` when no view is active. The returned rectangles are the
    /// authoritative description of the frame: rendering draws into them and
    /// mouse events are routed by hit-testing against them.
    #[must_use]
    pub fn view_layout(&self, width: u16, height: u16) -> Option<ViewLayout> {
        let active = self.active_view()?;

        if !self.views.maximized
            && self.views.view_stack.len() >= 2
            && let Some((primary, secondary)) = self.split_views()
        {
            let vertical = self.options.vertical_split;
            let can_split = if vertical {
                width >= VSPLIT_MIN_WIDTH && height >= VSPLIT_MIN_HEIGHT
            } else {
                width >= HSPLIT_MIN_WIDTH && height >= HSPLIT_MIN_HEIGHT
            };
            if can_split {
                let (primary, secondary) = if vertical {
                    // One column is reserved between the panes for the separator.
                    let left_w = width.saturating_sub(1) / 2;
                    let right_w = width.saturating_sub(left_w).saturating_sub(1);
                    (
                        PaneLayout {
                            kind: primary,
                            x: 0,
                            y: 0,
                            width: left_w,
                            height,
                        },
                        PaneLayout {
                            kind: secondary,
                            x: left_w.saturating_add(1),
                            y: 0,
                            width: right_w,
                            height,
                        },
                    )
                } else {
                    let top_h = height / 2;
                    let bottom_h = height.saturating_sub(top_h);
                    (
                        PaneLayout {
                            kind: primary,
                            x: 0,
                            y: 0,
                            width,
                            height: top_h,
                        },
                        PaneLayout {
                            kind: secondary,
                            x: 0,
                            y: top_h,
                            width,
                            height: bottom_h,
                        },
                    )
                };
                return Some(ViewLayout::Split {
                    primary,
                    secondary,
                    vertical,
                });
            }
        }

        Some(ViewLayout::Single(PaneLayout {
            kind: active,
            x: 0,
            y: 0,
            width,
            height,
        }))
    }

    /// Discards the damage-tracking cache so the next paint emits every row.
    ///
    /// Rendering only writes rows that differ from the previously painted
    /// frame. That is valid exactly as long as the physical screen still shows
    /// that frame. Whenever the alternate screen buffer is re-entered — after
    /// an editor or external command handover, or a `SIGTSTP`/`SIGCONT`
    /// cycle — the screen comes back *blank* while the cache still describes
    /// the old frame, so an unchanged view would be diffed down to zero bytes
    /// and the user would be left looking at an empty terminal.
    pub fn invalidate_screen(&self) {
        self.screen_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut renderer) = self.renderer.try_borrow_mut() {
            renderer.invalidate();
        }
    }

    /// Returns the visible content height (in rows) for the currently active view,
    /// accounting for dual split-window layouts in either orientation.
    #[must_use]
    pub fn active_view_visible_height(&self, width: u16, height: u16) -> usize {
        let fallback = (height as usize).saturating_sub(VIEW_CHROME_ROWS);
        let Some(active) = self.active_view() else {
            return fallback;
        };
        self.view_layout(width, height)
            .and_then(|layout| layout.pane_for(active))
            .map_or(fallback, |pane| pane.visible_height())
    }

    /// Returns the currently active view at the top of the stack.
    pub fn active_view(&self) -> Option<ViewKind> {
        self.views.active_view()
    }

    /// Returns whether the view of `kind` is currently instantiated.
    #[must_use]
    pub fn has_view(&self, kind: ViewKind) -> bool {
        self.views.has_view(kind)
    }

    /// Returns the *parent* of the active view: the view it was opened from.
    ///
    /// This is upstream Tig's `view->parent` link. The view stack is maintained
    /// in open order by [`Self::push_view`], so the parent is simply the
    /// nearest live entry beneath the active one. It is what `next` / `previous`
    /// steer, so that pressing `J` in a diff opened from the log advances the
    /// *log* selection rather than scrolling the diff body.
    ///
    /// Returns `None` for a root view, which correctly makes `next` / `previous`
    /// fall back to moving the active view's own cursor.
    #[must_use]
    pub fn parent_view(&self) -> Option<ViewKind> {
        self.views.parent_view()
    }

    /// Returns the visible content height of `kind`'s pane, falling back to
    /// `fallback` when the view is not on screen or the terminal size is unknown.
    ///
    /// Cursor motions must be scroll-adjusted against the height of the pane
    /// that actually owns the cursor. Using the active pane's height for a
    /// *parent* pane desynchronizes scrolling in split layouts, where the two
    /// panes have different heights.
    #[must_use]
    pub fn visible_height_for(&self, kind: ViewKind, fallback: usize) -> usize {
        self.last_terminal_size
            .and_then(|(width, height)| self.view_layout(width, height))
            .and_then(|layout| layout.pane_for(kind))
            .map_or(fallback, |pane| pane.visible_height())
    }

    /// Returns the formatted pending key sequence (e.g. "g-") or empty string.
    pub fn pending_keys_str(&self) -> String {
        if self.pending_keys.is_empty() {
            String::new()
        } else {
            let mut s = String::new();
            for k in &self.pending_keys {
                s.push_str(&k.to_string());
            }
            s.push('-');
            s
        }
    }

    /// Extracts the current contextual variables for macro expansion, prioritizing
    /// the currently focused view (`active_view()`).
    pub fn macro_context(&self) -> MacroContext {
        let mut ctx = MacroContext::new();

        // 1. Commit & Head (prioritize active view first)
        if let Some(id) = self
            .active_view()
            .and_then(|k| self.view_ref(k))
            .and_then(View::selected_commit_id)
        {
            ctx.commit = Some(id.to_string());
        } else if let Some(ref diff) = self.views.diff_view {
            ctx.commit = Some(diff.commit_id().to_string());
        } else if let Some(ref main) = self.views.main_view {
            if let Some(c) = main.selected_commit() {
                ctx.commit = Some(c.id.to_string());
            }
        } else if let Some(ref blame) = self.views.blame_view
            && let Some(l) = blame.selected_line()
        {
            ctx.commit = Some(l.commit_id.to_string());
        }

        if let Some(ref engine) = self.engine {
            if let Ok(head_id) = engine.head_commit_id() {
                ctx.head = Some(head_id.to_string());
            }
            if let Ok(branch) = engine.current_branch() {
                ctx.branch = Some(branch);
            }
            ctx.repo_git_dir = Some(engine.info().git_dir.display().to_string());
            ctx.repo_worktree = engine
                .info()
                .work_dir
                .as_ref()
                .map(|p| p.display().to_string());
        }

        // 2. Blob
        if let Some(ref blob) = self.views.blob_view {
            ctx.blob = Some(blob.blob_oid().to_string());
        }

        // 3. File, Directory & Line (prioritize active view via polymorphic View::populate_macro_context,
        //    falling back to open background views if the active view doesn't supply file context)
        let mut populated_from_active = false;
        if let Some(active_kind) = self.active_view()
            && matches!(
                active_kind,
                ViewKind::Status
                    | ViewKind::Tree
                    | ViewKind::Blame
                    | ViewKind::Blob
                    | ViewKind::Diff
            )
            && let Some(v) = self.view_ref(active_kind)
        {
            v.populate_macro_context(&mut ctx);
            populated_from_active = true;
        }

        if !populated_from_active {
            for fallback_kind in [
                ViewKind::Diff,
                ViewKind::Status,
                ViewKind::Tree,
                ViewKind::Blame,
                ViewKind::Blob,
            ] {
                if let Some(v) = self.view_ref(fallback_kind) {
                    v.populate_macro_context(&mut ctx);
                    break;
                }
            }
        }

        ctx
    }

    /// Constructs a new `AppState` configured with `engine` and optional `config`.
    #[must_use]
    pub fn with_engine_and_config(
        mut engine: Option<GitEngine>,
        config: Option<&tigrs_core::Config>,
    ) -> Self {
        let mut options = ViewOptions::default();
        let mut keymap = KeymapEngine::default();
        let mut keymap_warnings = Vec::new();
        let security = tigrs_core::SecurityConfig;
        let cli_read_only_override = config.and_then(|c| c.cli_read_only_override);
        if let Some(cfg) = config {
            options.apply_config(cfg);
            keymap_warnings = keymap.apply_config(&cfg.keybindings);
        }
        if let Some(ro_override) = cli_read_only_override {
            options.read_only = ro_override;
        }
        if let Some(ref mut eng) = engine {
            let effective_ro = options.read_only || eng.is_read_only();
            options.read_only = effective_ro;
            eng.set_read_only(effective_ro);
            eng.set_memory_profile(options.memory_profile);
        }
        // A TUI cannot print to stderr without corrupting the screen, so bad
        // config is reported through the status line instead.
        let status_message = keymap_warnings.first().map(|first| {
            let extra = keymap_warnings.len() - 1;
            if extra == 0 {
                format!("Config warning: {first}")
            } else {
                format!("Config warning: {first} (+{extra} more)")
            }
        });
        let saved_options_baseline = options.clone();
        let loaded_config = config.cloned().unwrap_or_default();
        let config_file_path = config
            .and_then(|c| c.loaded_path.clone())
            .or_else(tigrs_core::Config::default_config_path);
        let diff_document_cache = std::cell::RefCell::new(
            crate::diff::DiffDocumentCache::with_profile(options.memory_profile),
        );
        Self {
            engine,
            keymap,
            status_message,
            options,
            diff_document_cache,
            security,
            saved_options_baseline,
            config_file_path,
            loaded_config,
            cli_read_only_override,
            ..Default::default()
        }
    }
}

/// Canonicalizes the longest existing ancestor prefix of `path` and appends
/// any remaining non-existent path components, normalizing `.` and `..`.
fn canonicalize_existing_prefix(path: &std::path::Path) -> std::path::PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut cur: &std::path::Path = &abs;
    let mut tail = Vec::new();
    loop {
        if let Ok(canon) = cur.canonicalize() {
            let mut result = canon;
            for comp in tail.into_iter().rev() {
                match comp {
                    std::path::Component::Normal(c) => result.push(c),
                    std::path::Component::ParentDir => {
                        let _ = result.pop();
                    }
                    _ => {}
                }
            }
            return result;
        }
        if let Some(parent) = cur.parent()
            && parent != cur
        {
            if let Some(comp) = cur.components().next_back() {
                tail.push(comp);
            }
            cur = parent;
        } else {
            return abs;
        }
    }
}

/// Outcome of handling a single terminal event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Continue running the interactive event loop.
    Continue,
    /// Terminate the interactive event loop and exit cleanly.
    Quit,
}

fn run_standalone_view(app: AppState, cancel_source: &CancellationSource) -> Result<()> {
    let never_rx = crossbeam_channel::never::<Vec<CommitSummary>>();
    run_app_with_state(app, &never_rx, cancel_source)
}

/// Runs the interactive main view application loop until the user exits.
pub fn run_app(
    main_view: MainView,
    commit_rx: &Receiver<Vec<CommitSummary>>,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.main_view = Some(main_view);
    run_app_with_state(app, commit_rx, cancel_source)
}

/// Runs the interactive standalone diff view loop for a single commit.
pub fn run_diff_app(
    diff_view: DiffView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.caps = crate::term_cap::TerminalCapabilities::detect();
    app.options = app.options.with_capabilities(&app.caps);
    app.views.diff_view = Some(diff_view);
    app.refresh_diff_view();
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone status view loop for working tree inspection.
pub fn run_status_app(
    status_view: StatusView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.status_view = Some(status_view);
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone tree view loop for repository directory inspection.
pub fn run_tree_app(
    tree_view: TreeView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.tree_view = Some(tree_view);
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone blob view loop for inspecting file content.
pub fn run_blob_app(
    blob_view: BlobView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.blob_view = Some(blob_view);
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone blame view loop for inspecting file annotations.
pub fn run_blame_app(
    blame_view: BlameView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.blame_view = Some(blame_view);
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone references view loop.
pub fn run_refs_app(
    refs_view: RefsView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.refs_view = Some(refs_view);
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone stash view loop.
pub fn run_stash_app(
    stash_view: StashView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.stash_view = Some(stash_view);
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone reflog view loop.
pub fn run_reflog_app(
    reflog_view: ReflogView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.reflog_view = Some(reflog_view);
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone log view loop.
pub fn run_log_app(
    log_view: LogView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.log_view = Some(log_view);
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone grep view loop.
pub fn run_grep_app(
    grep_view: GrepView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.grep_view = Some(grep_view);
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone help view loop.
pub fn run_help_app(
    help_view: HelpView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.help_view = Some(help_view);
    run_standalone_view(app, cancel_source)
}

/// Runs the interactive standalone pager view loop.
pub fn run_pager_app(
    pager_view: PagerView,
    cancel_source: &CancellationSource,
    engine: Option<GitEngine>,
    config: Option<&tigrs_core::Config>,
) -> Result<()> {
    let mut app = AppState::with_engine_and_config(engine, config);
    app.views.pager_view = Some(pager_view);
    run_standalone_view(app, cancel_source)
}

/// The terminal-side collaborators the event loop needs when an event turns
/// into a child-process handover (`!` commands and `e`/`edit`).
struct TerminalContext<'a> {
    tty: &'a mut TtyController,
    input_reader: &'a crate::tty::InputReader,
    signals: &'a SignalCoordinator,
}

fn process_ui_event(
    event: &crossterm::event::Event,
    app: &mut AppState,
    term: &mut TerminalContext<'_>,
    cancel_source: &CancellationSource,
    (width, height): (&mut u16, &mut u16),
    needs_render: &mut bool,
) -> Flow {
    let tty = &mut *term.tty;
    let input_reader = term.input_reader;
    let signals = term.signals;
    match handle_event_with_dimensions(event, app, *width, *height) {
        Flow::Quit => {
            cancel_source.cancel();
            return Flow::Quit;
        }
        Flow::Continue => *needs_render = true,
    }

    if app.options.mouse != tty.is_mouse_enabled() {
        let _ = tty.set_mouse_capture(app.options.mouse);
    }

    if app.pending_suspend {
        app.pending_suspend = false;
        if let Err(err) = tty.suspend_for_job_control(input_reader) {
            app.status_message = Some(format!("Suspend failed: {err}"));
        }
        after_handover(tty, signals, (width, height));
        app.invalidate_screen();
        *needs_render = true;
    }

    if let Some((cmd_line, run_cmd)) = app.pending_handover.take() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut shell_cmd = std::process::Command::new(&shell);
        shell_cmd.arg("-c").arg(&cmd_line);
        if let Some(work_dir) = app.engine.as_ref().map(|e| e.work_dir().to_path_buf()) {
            shell_cmd.current_dir(&work_dir);
            if !app.is_current_repo_trusted(&work_dir) {
                tigrs_git::apply_untrusted_repo_env(&mut shell_cmd, &work_dir);
            }
        }
        let mut guard = match crate::tty::TtyHandoverGuard::enter(tty, Some(input_reader)) {
            Ok(g) => g,
            Err(err) => {
                app.status_message = Some(format!("Handover error: {err}"));
                return Flow::Continue;
            }
        };
        let res = guard.execute_command(shell_cmd);
        let _ = guard.reclaim();
        drop(guard);
        after_handover(tty, signals, (width, height));

        let is_status = app.active_view() == Some(ViewKind::Status);
        let mut status_report = None;
        if let Some(ref mut eng) = app.engine {
            let _ = eng.purge_worktree_caches();
            if is_status {
                let (_src, token) = CancellationToken::new();
                status_report = eng.load_status(&token).ok();
            }
        }
        if let Some(report) = status_report {
            let old_cursor = app
                .views
                .status_view
                .as_ref()
                .map_or(0, StatusView::cursor_index);
            let visible = app.active_view_visible_height(*width, *height);
            let mut new_status = StatusView::new(report);
            new_status.set_cursor(old_cursor, visible);
            app.views.status_view = Some(new_status);
        }
        *needs_render = true;

        match res {
            Ok(status) => {
                if !status.success() {
                    app.status_message = Some(format!("Command exited with {status}"));
                }
                if run_cmd.flags.contains(RunFlags::EXIT) {
                    cancel_source.cancel();
                    return Flow::Quit;
                }
            }
            Err(err) => {
                app.status_message = Some(format!("Command error: {err}"));
            }
        }
    }

    if let Some(inv) = app.pending_editor.take() {
        if tty.is_piped() {
            app.status_message = Some("Cannot open editor in piped mode".to_string());
            return Flow::Continue;
        }

        let mut guard = match crate::tty::TtyHandoverGuard::enter(tty, Some(input_reader)) {
            Ok(g) => g,
            Err(err) => {
                app.status_message = Some(format!("Handover error: {err}"));
                return Flow::Continue;
            }
        };

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(&inv.command_line);
        if let Some(ref eng) = app.engine {
            let work_dir = eng.work_dir();
            cmd.current_dir(work_dir);
            if !app.is_current_repo_trusted(work_dir) {
                tigrs_git::apply_untrusted_repo_env(&mut cmd, work_dir);
            }
        }

        let res = guard.execute_command(cmd);
        let _ = guard.reclaim();
        drop(guard);
        after_handover(tty, signals, (width, height));

        refresh_after_editor(app, (*width, *height));
        *needs_render = true;

        match res {
            Ok(status) => {
                if !status.success() {
                    if inv.used_line_number {
                        app.editor_line_number_disabled = true;
                        app.status_message = Some(
                            "Editor failed; line numbers disabled for this session (set editor-line-number = no to persist)".to_string(),
                        );
                    } else {
                        app.status_message = Some(format!("Editor exited with {status}"));
                    }
                }
            }
            Err(err) => {
                app.status_message = Some(format!("Editor error: {err}"));
            }
        }
    }

    Flow::Continue
}

/// Re-synchronises the UI with the terminal after an interactive child process
/// handed the terminal back.
///
/// The child ran in tigrs' process group, so any Ctrl-C or Ctrl-Z the user
/// aimed at it was delivered to tigrs too and is still sitting in the signal
/// channel; replaying those would quit or suspend tigrs for no reason. The
/// terminal may also have been resized while the child owned the screen, and
/// the `SIGWINCH` that reported it is discarded along with the rest, so the
/// size is re-queried directly instead.
fn after_handover(
    tty: &TtyController,
    signals: &SignalCoordinator,
    (width, height): (&mut u16, &mut u16),
) {
    signals.drain();
    if let Ok((w, h)) = tty.size() {
        *width = w;
        *height = h;
    }
}
fn refresh_after_editor(app: &mut AppState, (width, height): (u16, u16)) {
    let visible = app.active_view_visible_height(width, height);
    if let Some(ref mut eng) = app.engine {
        let _ = eng.purge_worktree_caches();
    }

    match app.active_view() {
        Some(ViewKind::Status) => {
            if let Some(ref eng) = app.engine {
                let (_src, token) = CancellationToken::new();
                if let Ok(report) = eng.load_status(&token) {
                    let old_cursor = app
                        .views
                        .status_view
                        .as_ref()
                        .map_or(0, StatusView::cursor_index);
                    let mut new_status = StatusView::new(report);
                    new_status.set_cursor(old_cursor, visible);
                    app.views.status_view = Some(new_status);
                }
            }
        }
        Some(ViewKind::Diff) => {
            if let Some(ref eng) = app.engine {
                let status_item = app
                    .views
                    .diff_view
                    .as_ref()
                    .and_then(|d| d.status_item().cloned());
                if let Some(item) = status_item
                    && let Ok(diff_data) = eng.compute_status_item_diff(&item)
                {
                    let old_cursor = app
                        .views
                        .diff_view
                        .as_ref()
                        .map_or(0, DiffView::cursor_index);
                    let mut new_diff = app.create_status_diff_view(diff_data, item);
                    new_diff.set_cursor(old_cursor, visible);
                    app.views.diff_view = Some(new_diff);
                }
            }
        }
        Some(ViewKind::Blame) => {
            let blame_info = app
                .views
                .blame_view
                .as_ref()
                .map(|b| (b.commit_oid(), b.path().to_string(), b.cursor()));
            if let (Some((commit_id, path, cursor)), Some(eng)) = (blame_info, &app.engine)
                && let Ok(res) = eng.blame_file(commit_id, &path)
            {
                let mut bv = BlameView::from_result(res);
                bv.refresh_highlighting(&app.options);
                bv.set_cursor(cursor, visible);
                app.views.blame_view = Some(bv);
            }
        }
        Some(ViewKind::Blob) => {
            let blob_info = app
                .views
                .blob_view
                .as_ref()
                .map(|b| (b.commit_oid(), b.path().to_string(), b.cursor()));
            if let (Some((commit_oid, path, cursor)), Some(eng)) = (blob_info, &app.engine)
                && let Ok(blob) = eng.read_blob_at_commit_path(commit_oid, &path)
            {
                let mut bv = BlobView::new_with_options(commit_oid, blob, &app.options);
                bv.set_cursor(cursor, visible);
                app.views.blob_view = Some(bv);
            }
        }
        _ => {}
    }

    if app.active_view() != Some(ViewKind::Status)
        && let Some(ref mut status) = app.views.status_view
        && let Some(ref eng) = app.engine
    {
        let (_src, token) = CancellationToken::new();
        if let Ok(report) = eng.load_status(&token) {
            status.refresh(report);
        }
    }
}

/// Builds the main view's uncommitted-changes rows from a worktree status scan.
///
/// Ordering and wording follow upstream Tig: untracked, then unstaged, then
/// staged, top to bottom, with empty sections omitted. Unmerged (conflicted)
/// paths are folded into the unstaged section, also matching upstream.
fn changes_rows_from_status(
    report: &tigrs_git::StatusReport,
    options: &ViewOptions,
) -> Vec<ChangesRow> {
    if !options.show_changes {
        return Vec::new();
    }

    let mut rows = Vec::with_capacity(3);

    if options.show_untracked && !report.untracked.is_empty() {
        rows.push(ChangesRow::new(
            ChangesKind::Untracked,
            report.untracked.len(),
        ));
    }

    let unstaged = report.unstaged.len() + report.unmerged.len();
    if unstaged > 0 {
        rows.push(ChangesRow::new(ChangesKind::Unstaged, unstaged));
    }

    if !report.staged.is_empty() {
        rows.push(ChangesRow::new(ChangesKind::Staged, report.staged.len()));
    }

    rows
}

/// Spawns a background worktree status scan feeding the main view's changes rows.
///
/// The scan never runs on the startup path: it stats the whole worktree and
/// would delay time-to-first-frame, so results are folded in after the first
/// paint and on subsequent filesystem events.
fn spawn_changes_scan(
    engine: &GitEngine,
    tx: &Sender<tigrs_git::StatusReport>,
    token: CancellationToken,
) {
    let eng = engine.clone();
    let tx = tx.clone();
    tigrs_core::global_compute_pool().spawn(move || {
        if token.is_cancelled() {
            return;
        }
        if let Ok(report) = eng.load_status(&token) {
            if token.is_cancelled() {
                return;
            }
            // A full channel means a newer scan is already queued; dropping
            // this result is correct, not an error.
            let _ = tx.try_send(report);
        }
    });
}

/// Primary interactive application runner managing TTY lifecycle, multiplexed channels, and active views.
pub fn run_app_with_state(
    mut app: AppState,
    commit_rx: &Receiver<Vec<CommitSummary>>,
    cancel_source: &CancellationSource,
) -> Result<()> {
    let mut tty = TtyController::enter_with_options(app.options.mouse)?;
    let signals = SignalCoordinator::new()?;
    let input_reader = crate::tty::InputReader::spawn();
    let input_rx = input_reader.receiver();
    let (mut width, mut height) = tty.size()?;

    app.caps = crate::term_cap::TerminalCapabilities::detect();
    app.options = app.options.with_capabilities(&app.caps);
    app.ensure_view_stack();

    if app.core_editor.is_none()
        && let Some(ref eng) = app.engine
    {
        let is_trusted = app.is_current_repo_trusted(eng.work_dir());
        app.core_editor = eng.core_editor(is_trusted);
    }

    // Populate ref badges on main view asynchronously after first paint so time-to-first-frame is never blocked
    let (refs_tx, refs_rx) = bounded::<Vec<tigrs_git::RefEntry>>(1);

    let mut is_streaming = app.views.main_view.is_some();
    let mut active_commit_rx = commit_rx.clone();
    let mut needs_render = true;
    let mut pending_input_render = false;
    let mut painted_content = false;
    // Which alternate-screen incarnation the damage-tracking cache describes.
    // Zero never matches a live controller, so the first pass through the loop
    // always starts from a clean cache.
    let mut painted_generation = 0u64;
    let mut last_render = Instant::now();
    let (blame_tx, blame_rx) = bounded::<BlameWorkerResponse>(16);
    app.blame_task.tx = Some(blame_tx.clone());

    let (diff_tx, diff_rx) = bounded::<DiffWorkerResponse>(16);
    app.diff_task.tx = Some(diff_tx.clone());

    let (log_tx, log_rx) = bounded::<LogWorkerResponse>(16);
    app.log_task.tx = Some(log_tx.clone());

    // Capacity of one: filesystem event bursts collapse into a single pending
    // scan result instead of queueing up stale ones.
    let (changes_tx, changes_rx) = bounded::<tigrs_git::StatusReport>(1);
    let mut post_paint_init_done = false;
    let mut watcher_debounce = Debounce::new(WATCHER_DEBOUNCE_INTERVAL);
    let mut last_input_at: Option<Instant> = None;
    let never_commit_rx = crossbeam_channel::never::<Vec<CommitSummary>>();
    let never_refs_rx = crossbeam_channel::never::<Vec<tigrs_git::RefEntry>>();
    let never_changes_rx = crossbeam_channel::never::<tigrs_git::StatusReport>();

    let mut watcher: Option<tigrs_git::RepoWatcher> = None;
    let (never_watcher_tx, never_watcher_rx) = bounded::<tigrs_git::RepoChangeEvent>(0);
    let _ = never_watcher_tx;

    if let Some(ref b) = app.views.blame_view
        && b.is_loading()
        && let Some(engine) = &app.engine
    {
        let cid = b.commit_oid();
        let path = b.path().to_string();
        let eng = engine.clone();
        if let (req_id, token, Some(tx)) = app.blame_task.begin_request() {
            tigrs_core::global_compute_pool().spawn(move || {
                if token.is_cancelled() {
                    return;
                }
                let res = eng.blame_file_cancellable(cid, &path, &token);
                if token.is_cancelled() {
                    return;
                }
                let _ = tigrs_core::send_or_yield(
                    &tx,
                    BlameWorkerResponse {
                        commit_id: Some(cid),
                        path,
                        request_id: req_id,
                        nav: BlameNavKind::Initial,
                        result: res,
                    },
                    &token,
                );
            });
        }
    }

    loop {
        if cancel_source.is_cancelled() {
            app.cancel_all();
            return Ok(());
        }

        // Drain any pending input events with highest priority before rendering or waiting,
        // coalescing repeated diff/syntax rebuilds (e.g., holding `]` or `[`) into a single
        // rebuild per frame and capping the batch so continuous key-repeat bursts never starve rendering.
        app.defer_view_refresh = true;
        let mut drained_events = 0usize;
        while drained_events < 32
            && let Ok(event) = input_rx.try_recv()
        {
            drained_events += 1;
            last_input_at = Some(Instant::now());
            pending_input_render = true;
            if process_ui_event(
                &event,
                &mut app,
                &mut TerminalContext {
                    tty: &mut tty,
                    input_reader: &input_reader,
                    signals: &signals,
                },
                cancel_source,
                (&mut width, &mut height),
                &mut needs_render,
            ) == Flow::Quit
            {
                app.defer_view_refresh = false;
                app.cancel_all();
                return Ok(());
            }
        }
        app.defer_view_refresh = false;
        app.flush_deferred_view_refresh();

        // ---- Render phase -------------------------------------------------
        // A child process (editor, `!` command) or a SIGTSTP/SIGCONT cycle
        // leaves and re-enters the alternate screen, which comes back blank.
        // Drop the damage-tracking cache first, otherwise an unchanged view
        // diffs down to zero bytes and the user is left with a blank screen.
        if tty.screen_generation() != painted_generation {
            painted_generation = tty.screen_generation();
            app.invalidate_screen();
            needs_render = true;
        }

        // The first frame that actually carries content bypasses the coalescing
        // cap; it is the frame the user perceives as startup latency.
        if needs_render && tty.is_active() {
            let has_content = app
                .active_view()
                .and_then(|k| app.view_ref(k))
                .is_some_and(View::has_content);

            let first_content = !painted_content && has_content;
            let cap_elapsed = !is_streaming || last_render.elapsed() >= FRAME_INTERVAL;

            if first_content || pending_input_render || cap_elapsed {
                if let Err(err) =
                    render_active(&app, tty.writer(), width, height).and_then(|()| tty.flush())
                {
                    if let tigrs_core::TigError::Io(ref io_err) = err
                        && (io_err.kind() == std::io::ErrorKind::BrokenPipe
                            || io_err.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()))
                    {
                        cancel_source.cancel();
                        app.cancel_all();
                        return Ok(());
                    }
                    return Err(err);
                }
                needs_render = false;
                pending_input_render = false;
                last_render = Instant::now();
                painted_content |= has_content;
            }
        }

        // Kick off the repository watcher, ref badge loader, and first worktree
        // scan only once a real content frame has been painted (or streaming ended),
        // so they never compete with time-to-first-frame.
        if !post_paint_init_done && (painted_content || !is_streaming) {
            post_paint_init_done = true;
            if let Some(ref eng) = app.engine {
                watcher = eng.start_watcher().ok();
                if app.views.main_view.is_some() {
                    let eng_refs = eng.clone();
                    let tx = refs_tx.clone();
                    tigrs_core::global_compute_pool().spawn(move || {
                        if let Ok(refs) = eng_refs.list_refs() {
                            let _ = tx.send(refs);
                        }
                    });
                    let token = app.cancels.reset_status();
                    spawn_changes_scan(eng, &changes_tx, token);
                }
            }
        }
        let watcher_rx = watcher
            .as_ref()
            .map_or(&never_watcher_rx, tigrs_git::RepoWatcher::receiver);

        // If a repaint is still owed, wake when the frame budget expires.
        // Otherwise block until one of the sources produces something.
        let mut wait = if needs_render {
            FRAME_INTERVAL.saturating_sub(last_render.elapsed())
        } else {
            IDLE_WAIT
        };

        // During an active key-repeat burst (< 60ms since last input event), defer background
        // revwalk batch absorption, ref badge updates, and initial worktree status row insertion
        // as long as MainView already has plenty of commits loaded ahead of the cursor. This
        // eliminates mid-scroll layout shifts, full-screen repaints, and batch-ingestion pauses.
        let input_burst_quiet_window = Duration::from_millis(60);
        let has_scroll_headroom = app
            .views
            .main_view
            .as_ref()
            .is_some_and(|m| m.commit_count() > m.cursor_index().saturating_add(64));
        let input_burst_active = has_scroll_headroom
            && if let Some(t) = last_input_at {
                let elapsed = t.elapsed();
                if elapsed < input_burst_quiet_window {
                    wait = wait.min(input_burst_quiet_window.saturating_sub(elapsed));
                    true
                } else {
                    false
                }
            } else {
                false
            };
        let eff_commit_rx = if input_burst_active {
            &never_commit_rx
        } else {
            &active_commit_rx
        };
        let eff_refs_rx = if input_burst_active {
            &never_refs_rx
        } else {
            &refs_rx
        };
        let eff_changes_rx = if input_burst_active {
            &never_changes_rx
        } else {
            &changes_rx
        };

        // Dispatch debounced commit or working-tree diff computation if the debounce timer has expired
        if let Some((target, deadline, req_id)) = app.pending_diff_target.clone() {
            let now = Instant::now();
            if now >= deadline {
                app.pending_diff_target = None;
                if let (Some(engine), Some(tx)) = (&app.engine, &app.diff_task.tx) {
                    let eng = engine.clone();
                    let worker_tx = tx.clone();
                    let opts = app.options.clone();
                    let caps = app.caps.clone();
                    let render_key = app.current_diff_render_key();
                    let generation = engine.generation();
                    let changes_items = match &target {
                        DiffRequestTarget::Changes(kind) => app.changes_report.as_ref().map(|r| {
                            let (sec, slice) = match kind {
                                ChangesKind::Untracked => {
                                    (tigrs_git::StatusSection::Untracked, r.untracked.clone())
                                }
                                ChangesKind::Unstaged => {
                                    (tigrs_git::StatusSection::Unstaged, r.unstaged.clone())
                                }
                                ChangesKind::Staged => {
                                    (tigrs_git::StatusSection::Staged, r.staged.clone())
                                }
                            };
                            (*kind, sec, slice)
                        }),
                        DiffRequestTarget::Commit(_) | DiffRequestTarget::StatusItem(_) => None,
                    };
                    let token = app.diff_task.reset_cancel();
                    tigrs_core::global_compute_pool().spawn(move || {
                        if token.is_cancelled() {
                            return;
                        }
                        let res = match &target {
                            DiffRequestTarget::Commit(oid) => {
                                eng.compute_commit_diff_cached_cancellable(*oid, &token)
                            }
                            DiffRequestTarget::Changes(_kind) => {
                                let Some((_, sec, items)) = changes_items else {
                                    return;
                                };
                                eng.compute_status_section_diff_cancellable(sec, &items, &token)
                                    .map(Arc::new)
                            }
                            DiffRequestTarget::StatusItem(item) => eng
                                .compute_status_item_diff_cancellable(item, &token)
                                .map(Arc::new),
                        };
                        if token.is_cancelled() {
                            return;
                        }
                        let status_item_opt = match &target {
                            DiffRequestTarget::StatusItem(item) => Some(item.clone()),
                            _ => None,
                        };
                        let precomputed_view = if let Ok(ref diff_data) = res {
                            let blob_provider =
                                |oid: tigrs_git::ObjectId| eng.read_blob_raw_lines(oid).ok();
                            DiffView::new_cancellable_with_caps(
                                Arc::clone(diff_data),
                                status_item_opt,
                                &opts,
                                Some(&caps),
                                Some(&blob_provider),
                                &token,
                            )
                        } else {
                            None
                        };
                        if token.is_cancelled() {
                            return;
                        }
                        let _ = tigrs_core::send_or_yield(
                            &worker_tx,
                            DiffWorkerResponse {
                                target,
                                render_key: Some(render_key),
                                is_prefetch: false,
                                generation,
                                request_id: req_id,
                                result: res,
                                precomputed_view,
                            },
                            &token,
                        );
                    });
                }
            } else {
                let remaining = deadline.saturating_duration_since(now);
                wait = wait.min(remaining);
            }
        }

        // Dispatch debounced status scan if watcher debounce timer has expired
        let now = Instant::now();
        if watcher_debounce.poll(now).is_some() {
            if let Some(ref eng) = app.engine {
                let token = app.cancels.reset_status();
                spawn_changes_scan(eng, &changes_tx, token);
            }
        } else if let Some(remaining) = watcher_debounce.time_until_fire(now) {
            wait = wait.min(remaining);
        }

        // Dispatch speculative commit diff + DiffDocument prefetch if idle timer has expired
        if let Some((target_oid, deadline)) = app.pending_speculative_prefetch {
            let now = Instant::now();
            if now >= deadline {
                app.pending_speculative_prefetch = None;
                let render_key = app.current_diff_render_key();
                let radius = app.options.memory_profile.prefetch_window_radius();
                let mut prefetch_oids = Vec::with_capacity(radius * 2 + 1);
                {
                    let mut doc_cache = app.diff_document_cache.borrow_mut();
                    if doc_cache.get(target_oid, &render_key).is_none() {
                        prefetch_oids.push(target_oid);
                    }
                    if let Some(ref main) = app.views.main_view {
                        let commits = main.commits();
                        let (_, sel_oid) = dispatch::main_selection(&app);
                        if let Some(center_oid) = sel_oid
                            && let Some(center) = main.commit_index_for_id(&center_oid)
                        {
                            for dist in 1..=radius {
                                if let Some(next_c) = commits.get(center + dist)
                                    && next_c.id != target_oid
                                    && doc_cache.get(next_c.id, &render_key).is_none()
                                {
                                    prefetch_oids.push(next_c.id);
                                }
                                if let Some(prev_idx) = center.checked_sub(dist)
                                    && let Some(prev_c) = commits.get(prev_idx)
                                    && prev_c.id != target_oid
                                    && doc_cache.get(prev_c.id, &render_key).is_none()
                                {
                                    prefetch_oids.push(prev_c.id);
                                }
                            }
                        }
                    }
                }
                if !prefetch_oids.is_empty()
                    && let Some(ref engine) = app.engine
                {
                    let eng = engine.clone();
                    let worker_tx = app.diff_task.tx.clone();
                    let opts = app.options.clone();
                    let caps = app.caps.clone();
                    let generation = engine.generation();
                    let token = app.cancels.reset_prefetch();
                    tigrs_core::global_compute_pool().spawn(move || {
                        let blob_provider =
                            |oid: tigrs_git::ObjectId| eng.read_blob_raw_lines(oid).ok();
                        for oid in prefetch_oids {
                            if token.is_cancelled() {
                                return;
                            }
                            let res = eng.compute_commit_diff_cached_cancellable(oid, &token);
                            if token.is_cancelled() {
                                return;
                            }
                            if let (Ok(diff_data), Some(tx)) = (&res, &worker_tx) {
                                let precomputed_view = DiffView::new_cancellable_with_caps(
                                    Arc::clone(diff_data),
                                    None,
                                    &opts,
                                    Some(&caps),
                                    Some(&blob_provider),
                                    &token,
                                );
                                if !token.is_cancelled() {
                                    let _ = tigrs_core::send_or_yield(
                                        tx,
                                        DiffWorkerResponse {
                                            target: DiffRequestTarget::Commit(oid),
                                            render_key: Some(render_key.clone()),
                                            is_prefetch: true,
                                            generation,
                                            request_id: 0,
                                            result: res,
                                            precomputed_view,
                                        },
                                        &token,
                                    );
                                }
                            }
                        }
                    });
                }
            } else {
                let remaining = deadline.saturating_duration_since(now);
                wait = wait.min(remaining);
            }
        }

        select! {
            recv(eff_refs_rx) -> msg => {
                if let Ok(refs) = msg
                    && let Some(ref mut main) = app.views.main_view {
                        main.set_ref_badges(&refs);
                        needs_render = true;
                    }
            },

            recv(diff_rx) -> msg => {
                if let Ok(resp) = msg {
                    needs_render |= on_diff_result(&mut app, resp);
                }
            },

            recv(log_rx) -> msg => {
                if let Ok(resp) = msg {
                    needs_render |= on_log_batch(&mut app, resp);
                }
            },

            recv(eff_changes_rx) -> msg => {
                if let Ok(report) = msg {
                    needs_render |= on_changes_result(&mut app, report, width, height);
                }
            },

            recv(blame_rx) -> msg => {
                if let Ok(resp) = msg {
                    needs_render |= on_blame_result(&mut app, resp);
                }
            },

            recv(eff_commit_rx) -> msg => if let Ok(batch) = msg {
                let absorb_limit = app.options.memory_profile.revwalk_absorb_batches();
                if let Some(ref mut main) = app.views.main_view {
                    main.append_commits(batch);
                    // Absorb up to a bounded number of batches per tick,
                    // breaking immediately if user input is pending so keystrokes remain instantaneous.
                    for _ in 0..absorb_limit {
                        if !input_rx.is_empty() {
                            break;
                        }
                        match active_commit_rx.try_recv() {
                            Ok(more) => main.append_commits(more),
                            Err(_) => break,
                        }
                    }
                    if app.active_view() == Some(ViewKind::Main) {
                        needs_render = true;
                    }
                }
            } else {
                active_commit_rx = crossbeam_channel::never();
                if is_streaming {
                    is_streaming = false;
                    if let Some(ref mut main) = app.views.main_view {
                        main.set_finished();
                    }
                    if app.active_view() == Some(ViewKind::Main) {
                        needs_render = true;
                    }
                }
            },

            recv(signals.receiver()) -> msg => {
                if let Ok(sig) = msg {
                    match handle_signal(sig, &mut app, &mut tty, &input_reader, &signals, &mut width, &mut height)? {
                        Flow::Quit => {
                            cancel_source.cancel();
                            app.cancel_all();
                            return Ok(());
                        }
                        Flow::Continue => needs_render = true,
                    }
                } else {
                    cancel_source.cancel();
                    app.cancel_all();
                    return Ok(());
                }
            },

            recv(watcher_rx) -> msg => {
                if let Ok(_change) = msg {
                    on_watcher_event(&mut app, watcher.as_ref(), &mut watcher_debounce);
                }
            },

            recv(input_rx) -> msg => {
                if let Ok(event) = msg {
                    last_input_at = Some(Instant::now());
                    pending_input_render = true;
                    if process_ui_event(
                        &event,
                        &mut app,
                        &mut TerminalContext {
                            tty: &mut tty,
                            input_reader: &input_reader,
                            signals: &signals,
                        },
                        cancel_source,
                        (&mut width, &mut height),
                        &mut needs_render,
                    ) == Flow::Quit
                    {
                        app.cancel_all();
                        return Ok(());
                    }
                } else {
                    // The reader thread died (e.g. controlling terminal hangup);
                    // record any pending exit signal (or SIGHUP = 1) and shut down cleanly.
                    let sig = signals
                        .exit_signal()
                        .unwrap_or(signal_hook::consts::SIGHUP);
                    crate::signal::record_exit_signal(sig);
                    cancel_source.cancel();
                    app.cancel_all();
                    return Ok(());
                }
            },

            default(wait) => {}
        }
    }
}

/// Generic debounce timer that tracks a pending payload and its deadline.
#[derive(Debug, Clone)]
pub struct Debounce<T> {
    pending: Option<(T, Instant)>,
    delay: Duration,
}

impl<T> Debounce<T> {
    /// Creates a new disarmed `Debounce` with the given delay duration.
    #[must_use]
    pub const fn new(delay: Duration) -> Self {
        Self {
            pending: None,
            delay,
        }
    }

    /// Arms or resets the debounce deadline with `item`.
    pub fn arm(&mut self, item: T) {
        self.pending = Some((item, Instant::now() + self.delay));
    }

    /// Disarms the debounce timer, discarding any pending item.
    pub fn disarm(&mut self) {
        self.pending = None;
    }

    /// Returns `Some(T)` if the timer has expired as of `now`, disarming it.
    pub fn poll(&mut self, now: Instant) -> Option<T> {
        if let Some((_, deadline)) = &self.pending
            && now >= *deadline
        {
            return self.pending.take().map(|(item, _)| item);
        }
        None
    }

    /// Returns the remaining duration until the timer fires, if armed.
    #[must_use]
    pub fn time_until_fire(&self, now: Instant) -> Option<Duration> {
        self.pending
            .as_ref()
            .map(|(_, deadline)| deadline.saturating_duration_since(now))
    }
}

fn on_diff_result(app: &mut AppState, resp: DiffWorkerResponse) -> bool {
    match &resp.target {
        DiffRequestTarget::Commit(commit_id) => {
            if let (Some(render_key), Some(view)) = (&resp.render_key, &resp.precomputed_view)
                && view.status_item().is_none()
                && !commit_id.is_null()
                && *commit_id != tigrs_git::ObjectId::empty_tree(commit_id.kind())
            {
                app.diff_document_cache.borrow_mut().insert(
                    *commit_id,
                    render_key.clone(),
                    Arc::clone(view.document_arc()),
                    view.is_external_formatter(),
                );
            }
        }
        DiffRequestTarget::Changes(kind) => {
            if let (Ok(diff_data), Some(view)) = (&resp.result, &resp.precomputed_view) {
                app.changes_diff_cache.insert(
                    (*kind, resp.generation),
                    (Arc::clone(diff_data), view.clone()),
                );
            }
        }
        DiffRequestTarget::StatusItem(item) => {
            if let (Some(render_key), Some(view)) = (&resp.render_key, &resp.precomputed_view) {
                app.status_item_diff_cache.insert(
                    (item.clone(), resp.generation, render_key.clone()),
                    view.clone(),
                );
            }
        }
    }
    if resp.is_prefetch {
        return false;
    }
    if !app.views.view_stack.contains(&ViewKind::Diff) || !app.diff_task.is_current(resp.request_id)
    {
        return false;
    }
    if let Some(view) = resp.precomputed_view {
        app.views.diff_view = Some(view);
        return true;
    }
    if let Ok(diff_data) = resp.result {
        let view = match resp.target {
            DiffRequestTarget::StatusItem(item) => {
                let v = app.create_status_diff_view(Arc::clone(&diff_data), item.clone());
                app.status_item_diff_cache.insert(
                    (item, resp.generation, app.current_diff_render_key()),
                    v.clone(),
                );
                v
            }
            DiffRequestTarget::Changes(kind) => {
                let v = app.create_diff_view(Arc::clone(&diff_data));
                app.changes_diff_cache
                    .insert((kind, resp.generation), (diff_data, v.clone()));
                v
            }
            DiffRequestTarget::Commit(_) => app.create_diff_view(Arc::clone(&diff_data)),
        };
        app.views.diff_view = Some(view);
        return true;
    }
    false
}

fn on_log_batch(app: &mut AppState, resp: LogWorkerResponse) -> bool {
    if app.log_task.is_current(resp.request_id)
        && let Some(ref mut log) = app.views.log_view
    {
        for (diff, refs) in resp.diffs {
            log.fill_commit_diff(&diff, refs.as_deref());
        }
        if app.active_view() == Some(ViewKind::Log) {
            return true;
        }
    }
    false
}

fn on_changes_result(
    app: &mut AppState,
    report: tigrs_git::StatusReport,
    width: u16,
    height: u16,
) -> bool {
    let mut needs_render = false;
    if let Some(ref eng) = app.engine {
        let generation = eng.generation();
        app.changes_diff_cache.retain(|&(_, g), _| g == generation);
        app.status_item_diff_cache
            .retain(|&(_, g, _), _| g == generation);
    } else {
        app.changes_diff_cache.clear();
        app.status_item_diff_cache.clear();
    }
    let mut sections_to_warm = Vec::with_capacity(3);
    if app.options.memory_profile != tigrs_core::MemoryProfile::Lean && app.options.show_changes {
        let generation = app.engine.as_ref().map_or(0, GitEngine::generation);
        if app.options.show_untracked
            && !report.untracked.is_empty()
            && !app
                .changes_diff_cache
                .contains_key(&(ChangesKind::Untracked, generation))
        {
            sections_to_warm.push((
                ChangesKind::Untracked,
                tigrs_git::StatusSection::Untracked,
                report.untracked.clone(),
            ));
        }
        if !report.unstaged.is_empty()
            && !app
                .changes_diff_cache
                .contains_key(&(ChangesKind::Unstaged, generation))
        {
            sections_to_warm.push((
                ChangesKind::Unstaged,
                tigrs_git::StatusSection::Unstaged,
                report.unstaged.clone(),
            ));
        }
        if !report.staged.is_empty()
            && !app
                .changes_diff_cache
                .contains_key(&(ChangesKind::Staged, generation))
        {
            sections_to_warm.push((
                ChangesKind::Staged,
                tigrs_git::StatusSection::Staged,
                report.staged.clone(),
            ));
        }
    }
    if app.active_view() == Some(ViewKind::Status) {
        let old_cursor = app
            .views
            .status_view
            .as_ref()
            .map_or(0, StatusView::cursor_index);
        let visible = app.active_view_visible_height(width, height);
        let mut new_status = StatusView::new(report.clone());
        new_status.set_cursor(old_cursor, visible);
        app.views.status_view = Some(new_status);
        needs_render = true;
    }
    app.changes_report = Some(report);
    if app.apply_changes_rows() && app.active_view() == Some(ViewKind::Main) {
        needs_render = true;
    }
    if !sections_to_warm.is_empty()
        && let (Some(engine), Some(worker_tx)) = (app.engine.clone(), app.diff_task.tx.clone())
    {
        let generation = engine.generation();
        let eng = engine;
        let opts = app.options.clone();
        let caps = app.caps.clone();
        let render_key = app.current_diff_render_key();
        let token = app.cancels.reset_changes_warmup();
        tigrs_core::global_compute_pool().spawn(move || {
            let blob_provider = |oid: tigrs_git::ObjectId| eng.read_blob_raw_lines(oid).ok();
            for (kind, section, items) in sections_to_warm {
                if token.is_cancelled() {
                    break;
                }
                let res = eng
                    .compute_status_section_diff(section, &items)
                    .map(Arc::new);
                if let Ok(ref diff_data) = res {
                    let precomputed_view = DiffView::new_cancellable_with_caps(
                        Arc::clone(diff_data),
                        None,
                        &opts,
                        Some(&caps),
                        Some(&blob_provider),
                        &token,
                    );
                    let _ = tigrs_core::send_or_yield(
                        &worker_tx,
                        DiffWorkerResponse {
                            target: DiffRequestTarget::Changes(kind),
                            render_key: Some(render_key.clone()),
                            is_prefetch: true,
                            generation,
                            request_id: 0,
                            result: res,
                            precomputed_view,
                        },
                        &token,
                    );
                }
            }
        });
    }
    needs_render
}

fn on_blame_result(app: &mut AppState, resp: BlameWorkerResponse) -> bool {
    if !app.blame_task.is_current(resp.request_id) {
        return false;
    }
    match resp.result {
        Ok(res) => {
            if let Some(ref mut blame) = app.views.blame_view {
                match resp.nav {
                    BlameNavKind::Initial => {
                        blame.apply_blame_result(res);
                    }
                    BlameNavKind::Parent {
                        commit_id,
                        path,
                        cursor,
                        visible_height,
                    } => {
                        blame.navigate_to(commit_id, path, cursor);
                        blame.apply_blame_result(res);
                        blame.set_cursor(cursor, visible_height);
                    }
                    BlameNavKind::Back {
                        prev_entry,
                        visible_height,
                    } => {
                        let _ = blame.pop_history();
                        blame.navigate_back_to(prev_entry, res, visible_height);
                    }
                }
                blame.refresh_highlighting(&app.options);
                return true;
            }
        }
        Err(err) => {
            if let Some(ref mut blame) = app.views.blame_view {
                blame.set_status_message(format!("Blame error: {err}"));
                return true;
            }
        }
    }
    false
}

fn on_watcher_event(
    app: &mut AppState,
    watcher: Option<&tigrs_git::RepoWatcher>,
    watcher_debounce: &mut Debounce<()>,
) {
    if let Some(w) = watcher {
        let _ = w.drain_events();
    }
    if let Some(ref mut eng) = app.engine {
        let _ = eng.purge_worktree_caches();
    }
    watcher_debounce.arm(());

    let should_shed = match app.options.memory_profile {
        tigrs_core::MemoryProfile::Lean => {
            tigrs_core::evaluate_memory_pressure() >= tigrs_core::MemoryPressureLevel::High
        }
        tigrs_core::MemoryProfile::Balanced | tigrs_core::MemoryProfile::Greedy => {
            tigrs_core::read_cgroup_memory_usage().is_some_and(|u| {
                u.limit_bytes.is_some()
                    && u.pressure_level() >= tigrs_core::MemoryPressureLevel::Critical
            })
        }
    };
    if should_shed {
        if let Some(ref mut main) = app.views.main_view {
            main.shrink_to_fit();
        }
        if let Some(ref mut eng) = app.engine {
            eng.shrink_object_cache(8 * 1024 * 1024);
        }
    }
}

/// Applies a POSIX signal to the terminal state.
fn handle_signal(
    sig: UiSignal,
    app: &mut AppState,
    tty: &mut TtyController,
    input_reader: &crate::tty::InputReader,
    signals: &SignalCoordinator,
    width: &mut u16,
    height: &mut u16,
) -> Result<Flow> {
    match sig {
        UiSignal::Resize => {
            if let Ok((w, h)) = tty.size() {
                *width = w;
                *height = h;
            }
            // Terminals reflow (and often clear) the alternate screen when the
            // window changes size, so nothing previously painted can be
            // assumed to still be there.
            app.invalidate_screen();
        }
        UiSignal::Quit => {
            if let Some(signo) = signals.exit_signal() {
                crate::signal::record_exit_signal(signo);
            }
            return Ok(Flow::Quit);
        }
        UiSignal::Suspend => {
            if let Err(err) = tty.suspend_for_job_control(input_reader) {
                app.status_message = Some(format!("Suspend failed: {err}"));
            }
            signals.drain();
            if let Ok((w, h)) = tty.size() {
                *width = w;
                *height = h;
            }
            app.invalidate_screen();
        }
        UiSignal::Resume => {
            tty.force_resume()?;
            input_reader.resume();
            if let Ok((w, h)) = tty.size() {
                *width = w;
                *height = h;
            }
            app.invalidate_screen();
        }
    }
    Ok(Flow::Continue)
}

/// Applies a terminal event to the active view, reporting whether the app should exit.
///
/// Mouse events are routed to the focused pane. Prefer
/// [`handle_event_with_dimensions`], which knows the terminal size and can route
/// the wheel to whichever pane the pointer is over.
pub fn handle_event(event: &Event, app: &mut AppState, visible_height: usize) -> Flow {
    handle_event_impl(event, app, visible_height, None)
}

/// Applies a terminal event to the active view using terminal dimensions to derive
/// pane geometry, so the visible height and mouse targeting match what is on screen.
pub fn handle_event_with_dimensions(
    event: &Event,
    app: &mut AppState,
    width: u16,
    height: u16,
) -> Flow {
    app.last_terminal_size = Some((width, height));
    let visible = app.active_view_visible_height(width, height);
    handle_event_impl(event, app, visible, Some((width, height)))
}

/// Shared event handling. `dimensions` carries the terminal size when known,
/// enabling pointer-accurate mouse routing; `None` falls back to the focused pane.
fn handle_event_impl(
    event: &Event,
    app: &mut AppState,
    visible_height: usize,
    dimensions: Option<(u16, u16)>,
) -> Flow {
    app.ensure_view_stack();

    match event {
        Event::Key(key) => {
            // Key events repeat on release under the Kitty protocol; only act on press.
            if key.kind == KeyEventKind::Release {
                return Flow::Continue;
            }

            // If prompt is active, intercept all key events
            if let Some(ref mut prompt) = app.prompt {
                let result = prompt.handle_key(key, &mut app.history);
                match result {
                    PromptResult::None => return Flow::Continue,
                    PromptResult::Cancel => {
                        app.prompt = None;
                        return Flow::Continue;
                    }
                    PromptResult::Confirm(confirmed) => {
                        let Some(prompt_obj) = app.prompt.take() else {
                            return Flow::Continue;
                        };
                        if let PromptKind::Confirm {
                            expanded_command,
                            run_command,
                            ..
                        } = prompt_obj.kind
                            && confirmed
                        {
                            return execute_run_command(app, &expanded_command, &run_command);
                        }
                        return Flow::Continue;
                    }
                    PromptResult::OptionSelected(menu_action) => {
                        app.prompt = None;
                        app.apply_menu_action(menu_action);
                        return Flow::Continue;
                    }
                    PromptResult::OptionChanged(menu_action) => {
                        app.apply_menu_action(menu_action);
                        return Flow::Continue;
                    }
                    PromptResult::SaveConfig { path, minimal } => {
                        app.save_config_to_toml(path.as_deref(), minimal);
                        return Flow::Continue;
                    }
                    PromptResult::Submit(text) => {
                        let Some(prompt_obj) = app.prompt.take() else {
                            return Flow::Continue;
                        };
                        if !text.is_empty() {
                            app.history.add(&text);
                            let history_inside_repo = app
                                .history
                                .path()
                                .is_some_and(|p| app.is_path_inside_repo(p));
                            if !(app.is_read_only() && history_inside_repo)
                                && let Err(err) = app.history.save()
                            {
                                app.status_message = Some(format!("Failed to save history: {err}"));
                            }
                        }
                        match prompt_obj.kind {
                            PromptKind::OptionMenu(_) => {
                                app.toggle_option(&text);
                                return Flow::Continue;
                            }
                            PromptKind::Command => {
                                let parsed = ParsedCommand::parse(&text);
                                return execute_parsed_command(app, parsed, visible_height);
                            }
                            PromptKind::SearchForward => {
                                execute_search(
                                    app,
                                    &text,
                                    SearchDirection::Forward,
                                    visible_height,
                                );
                                return Flow::Continue;
                            }
                            PromptKind::SearchBackward => {
                                execute_search(
                                    app,
                                    &text,
                                    SearchDirection::Backward,
                                    visible_height,
                                );
                                return Flow::Continue;
                            }
                            PromptKind::InteractiveMacro {
                                template,
                                answers,
                                run_command,
                                ..
                            } => {
                                return handle_interactive_macro_submit(
                                    app,
                                    &template,
                                    answers,
                                    &text,
                                    run_command.as_deref(),
                                );
                            }
                            PromptKind::Confirm { .. } => return Flow::Continue,
                        }
                    }
                }
            }

            // Global quit via Ctrl-C
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                app.pending_keys.clear();
                return Flow::Quit;
            }

            // If user presses Escape while multi-key sequence is pending, cancel the pending sequence
            if key.code == KeyCode::Esc && !app.pending_keys.is_empty() {
                app.pending_keys.clear();
                return Flow::Continue;
            }

            let key_entry = Key::from(*key);
            app.pending_keys.push(key_entry);

            let scope = app
                .active_view()
                .map_or(KeymapScope::Generic, KeymapScope::from);

            match app.keymap.lookup(scope, &app.pending_keys) {
                KeymapLookupResult::Prefix => Flow::Continue,
                KeymapLookupResult::Match(action) | KeymapLookupResult::Ambiguous(action) => {
                    app.pending_keys.clear();
                    execute_action(app, &action, visible_height)
                }
                KeymapLookupResult::NoMatch => {
                    app.pending_keys.clear();
                    Flow::Continue
                }
            }
        }

        Event::Mouse(mouse) => {
            if app.prompt.is_some() {
                return Flow::Continue;
            }

            // Scroll whichever pane the pointer is over, not merely the focused
            // one: in a split layout the user expects the wheel to act on the
            // pane under the cursor. Focus deliberately stays where it is.
            let pointed = dimensions
                .and_then(|(width, height)| app.view_layout(width, height))
                .and_then(|layout| layout.pane_at(mouse.column, mouse.row));
            let (target, pane_height) = match pointed {
                Some(pane) => (Some(pane.kind), pane.visible_height()),
                None => (app.active_view(), visible_height),
            };
            let Some(target) = target else {
                return Flow::Continue;
            };

            match mouse.kind {
                crossterm::event::MouseEventKind::ScrollDown => {
                    scroll_view_down(app, target, MOUSE_SCROLL_LINES, pane_height);
                    sync_split_views_after_scroll(app, target);
                    Flow::Continue
                }
                crossterm::event::MouseEventKind::ScrollUp => {
                    scroll_view_up(app, target, MOUSE_SCROLL_LINES, pane_height);
                    sync_split_views_after_scroll(app, target);
                    Flow::Continue
                }
                _ => Flow::Continue,
            }
        }

        _ => Flow::Continue,
    }
}
