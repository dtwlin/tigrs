# AI Coding Agent Guide (`tigrs`)

This document provides context, architectural invariants, security boundaries, and implementation guidelines for AI coding agents working on **`tigrs`**—a **secure** and **blazing fast** Git front end for the text terminal written in 100% safe Rust.

---

## 1. Core Project Invariants

1. **100% Safe Rust (`#![forbid(unsafe_code)]`)**:
   - Every crate in the workspace enforces `#![forbid(unsafe_code)]`.
   - Never introduce `unsafe` blocks or raw `libc` FFI calls. Use `rustix` and `signal-hook` for POSIX terminal, signal, pipe, and process operations.
2. **Zero C Dependencies**:
   - Default builds must compile with zero C dependencies (no `libgit2`, `ncurses`, `openssl`, or `libmimalloc-sys`).
3. **100% Public Documentation (`missing_docs = "warn"`)**:
   - Every public struct, enum, variant, struct field, trait, constant, and function must have clear, active-voice Rustdoc comments.
   - Verify with `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`.
4. **SPDX & Copyright Attribution**:
   - Every `.rs` source file must begin with the two-line header:
     ```rust
     // SPDX-License-Identifier: MIT OR Apache-2.0
     // Copyright (C) 2026 David Lin <dtwlin@gmail.com>
     ```
5. **Open-Source Data Hygiene**:
   - Never include proprietary or internal company paths, hostnames, URLs, bug IDs, or real employee names/emails in code, comments, or tests.
   - Always use RFC 2606 synthetic test data (e.g., `Alice Developer <alice@example.com>`, `https://ci.example.com/...`, `Issue-Id: 100200`).

---

## 2. Workspace Architecture

The Cargo workspace consists of four crates with strict unidirectional dependencies (`tigrs-core` ← `tigrs-git` ← `tigrs-ui` ← `tigrs-cli`):

```
tigrs/
├── crates/
│   ├── tigrs-core/   # Arena allocators, ANSI/shell sanitizers, cancellation, TOML config
│   ├── tigrs-git/    # Two-tier hybrid Git engine (in-process gix + hardened git CLI boundary)
│   ├── tigrs-ui/     # 14 interactive views, double-buffered cell renderer, diff layout engine
│   └── tigrs-cli/    # CLI entry point, crossbeam event loop, background worker orchestration
├── assets/           # Generated shell completions and UNIX man page (tigrs.1)
├── docs/screenshots/ # Headless-rendered SVG and GIF terminal demonstrations
└── config.toml.example
```

### 2.1 `tigrs-core` (`crates/tigrs-core`)
- **`APP_VERSION`** (`lib.rs`): Single source of truth for the release version (`env!("CARGO_PKG_VERSION")`).
- **ANSI & Control Character Sanitization** (`ansi.rs`):
  - `strip_control_chars`: Strips all CSI/OSC escape sequences, C0/C1 control codes (`U+0080..=U+009F`), `\r`, and Unicode BiDi overrides (`U+202A..=U+202E`, `U+2066..=U+2069`).
  - `filter_sgr_only`: Preserves only safe SGR color sequences (`\x1b[...m`) when consuming piped colored output.
- **Context-Aware Shell Quoting** (`quote.rs`, `macro_ctx.rs`):
  - `QuoteState` tracks unquoted, single-quoted (`'`), double-quoted (`"`), backtick subshell (`` ` ``), and nested `$()` command-substitution contexts when expanding runtime macros (`%(commit)`, `%(file)`, `%(lineno)`, `%(prompt)`), preventing shell command injection.
- **Configuration, Memory Profiles & Surgical TOML Persistence** (`config.rs`, `config_enums.rs`, `security.rs`):
  - Parses `~/.config/tigrs/config.toml` (or `$XDG_CONFIG_HOME/tigrs/config.toml`) with flexible kebab-case and snake_case option aliases, enforcing absolute path checks (`path.is_absolute()`) on `HOME` / `XDG_CONFIG_HOME` / `TIGRS_CONFIG`, `read_only: bool` (default `false`), `memory_profile: MemoryProfile` (`"greedy"` default, `"balanced"`, `"lean"`), Diff Header Redesign settings (`diff_presentation: DiffPresentation` defaulting to `Banner`, `diff_sticky_header`, `diff_collapse_generated`, `diff_hints: DiffHintsMode`), Main View Color Highlighting settings (`main_author_color: AuthorColorMode`, `main_spotlight: SpotlightMode`, `main_spotlight_dim_others`, `main_dim_unreachable`, `main_push_status`, `main_date_heat`, `main_subject_rules`, `main_unique_prefix`, `main_dim_merges`), `[colors.authors]` overrides, and `[[main.subject-rules]]` regex rules with zero repository allowlists.
  - `Config::render_merged_toml` and `Config::save_to_path` surgically merge modified `[general]`, `[view]`, and `[performance]` settings while preserving user comments, `[security]`, `[keybindings]`, `[colors]`, `[colors.authors]`, and `[[main.subject-rules]]`.

### 2.2 `tigrs-git` (`crates/tigrs-git`)
Implements a **Two-Tier Hybrid Git Architecture**:
- **Read-Only Enforcement (`engine.rs`)**:
  - `GitEngine` holds `read_only: Arc<AtomicBool>` (default `false`), exposing `is_read_only()`, `set_read_only(bool)`, and `ensure_writable(&str)` (returning `Err(TigError::ReadOnly(_))` across all 10 mutating engine methods: `stage_file`, `unstage_file`, `discard_file_changes`, `discard_untracked_file`, `stage_hunk`, `unstage_hunk`, `stage_line`, `unstage_line`, `stage_lines`, `unstage_lines`).
- **Tier 1 — In-Process `gix` + `imara-diff` (~95% of operations)**:
  - **Repository Discovery (`discovery.rs`)**: Opens via `gix::ThreadSafeRepository::discover_opts` with `filter_config_section` rejecting `filter`, `diff`, and `merge` driver sections from all sources and `verify_untrusted_core_worktree` preventing out-of-tree `core.worktree` escapes.
  - **Revwalk, Graph Table & Pack Commit Estimator** (`revwalk.rs`, `graph_table.rs`, `engine.rs`): Streams commits in profile-sized chunks (`MemoryProfile::revwalk_chunk_size()`, `5,000` in `greedy`, `2,000` in `balanced`, `250` in `lean`) using zero-copy `gix-commitgraph` mmap traversal when available, sub-millisecond `fast_commit_count_estimate()` via pack `.rev`/`.idx` binary search when no commit-graph exists, and Merkle tree-diff pruning for pathspecs.
  - **Diff Engine & Gitattributes** (`diff.rs`, `userdiff.rs`, `userdiff_table.rs`, `worddiff.rs`): Computes commit, range, and working-tree diffs in-process with all 28 builtin Git `userdiff` language drivers, intra-line token diffing, and in-process `.gitattributes` scanning for `linguist-generated` / `generated` attributes (`FileDiff::is_generated`).
  - **Blame, Tree & Blob** (`blame.rs`, `tree.rs`): In-process file annotation and commit tree/blob inspection backed by thread-safe byte-budgeted LRU caches (`cache.rs`).
- **Tier 2 — Hardened Subprocess Boundary (~5% of operations)**:
  - **Untrusted Repo Sandbox** (`path_security.rs`): All external `git` calls (`status.rs`, `stage.rs`, `reftable.rs`) **must** be constructed via `safe_git_command` (which also passes `--no-optional-locks`, `-c gc.auto=0`, `-c maintenance.auto=false`, and `-c fetch.writeCommitGraph=false` so background `git` commands never create lockfiles or mutate `.git/`).
  - **Interactive Staging & Patch Generation** (`stage.rs`, `patch.rs`): Generates byte-exact Git patches for file, hunk (`@`), and single-line (`1`) staging/unstaging via `git apply --cached`.
  - **Reftable Support** (`reftable.rs`): Automatically detects `extensions.refstorage = reftable` (where `.git/HEAD` contains `ref: refs/heads/.invalid`) and routes ref resolution to the hardened Tier-2 CLI while keeping object/diff/revwalk reads in `gix`.

### 2.3 `tigrs-ui` (`crates/tigrs-ui`)
- **State, Read-Only Guard, & View Management** (`app/mod.rs`, `app/dispatch.rs`, `app/commands.rs`, `app/layout.rs`, `app/options_panel.rs`):
  - `AppState` owns `ViewManager` (managing all 14 canonical views: `Main`, `Diff`, `Log`, `Status`, `Stage`, `Tree`, `Blob`, `Blame`, `Refs`, `Stash`, `Grep`, `Reflog`, `Help`, `Pager`), `ViewOptions`, `Keymap`, a profile-sized LRU `DiffDocumentCache` (`1,024` entries in `greedy`), deferred input-burst refresh coalescing (`defer_view_refresh` / `flush_deferred_view_refresh`), and monotonic `AsyncTaskSlot` cancellation handles.
  - `AppState::check_read_only_blocked` blocks all mutating actions when Read-Only Mode is enabled (`options.read_only == true`) and sets `status_message` to `READ_ONLY_WARNING_MSG`. Runtime toggles (`:set read-only = true/false`, `:read-only`, `:update-mode`, `:rw`, `:ro`, `:set noro`, `:toggle read-only`) synchronize `options.read_only`, `config.general.read_only`, and `engine.set_read_only(...)` via `AppState::sync_read_only_state`.
- **Double-Buffered Cell Renderer & Unified Overlay Compositing** (`app/render.rs`, `headless.rs`, `term_cap.rs`, `ui_theme.rs`):
  - Views and bottom overlays (including the 44-item context-aware Interactive Options & Config Panel `o` with 4 task-oriented tabs and inline `/` search filter, the Diff file-details card `i`, and the status/prompt bar) render into `ScreenGrid` cells via `composite_overlay_into_front_grid`, styled by 14 built-in UI color themes (`UiThemeId` across Adaptive, Dark, Light, and WCAG AAA High-Contrast families), with `[RO]` stamped in the title bar when `options.read_only` is active.
  - `present_frame_to_spool` downsamples colors to the detected `ColorProfile` (`TrueColor`, `Ansi256`, `Ansi16`, `Monochrome`), diffs row-by-row against `back_grid`, and emits damaged rows in a single synchronized (`\x1b[?2026h` / `\x1b[?2026l`) write.
- **Main View Color Highlighting (`P0`–`P11`)** (`view/main_view.rs`, `ui_theme.rs`):
  - Implements deterministic 10-hue FNV-1a author coloring (`main_author_color = "hash" | "me" | "off"`, hotkey `a`, plus `[colors.authors]` overrides), interactive same-author and branch-ancestry spotlighting (`main_spotlight = "off" | "author" | "ancestry"`, hotkeys `*` and `&` with `compute_ancestry_lineage`), current-user `"me"` badge (`● `), background-tinted cursor row (`cursor_row_bg`) preserving multi-hue foreground tokens on named themes, unreachable reflog commit dimming (`main_dim_unreachable`), unpushed commit SHA indicator (`↑`, `main_push_status`, hotkey `U`), 6-bucket relative date heatmap (`main_date_heat`), co-located ref badge consolidation (`[main ⇄ origin]`, `[v1.0 +2 tags]`), semantic subject rules (`fixup!`, `Revert`, `WIP`, `feat!:`, `#123` issues, `[[main.subject-rules]]`, `main_dim_merges`), and shortest unique SHA prefix bolding (`main_unique_prefix`).
- **Diff Presentation Pipeline (`P1`–`P11`)** (`diff/`):
  - Supports `Banner` (default 2-line box-drawn header with `[MOD]`/`[ADD]`/`[DEL]`/`[REN]`/`[CPY]`/`[CHM]`/`[BIN]`/`[SUB]` badges, directory/basename contrast, and proportional `━━───` diffstat bar), `Fancy` (`diff-so-fancy` style), and `Classic` (`diff --git`) presentations (`diff/paint.rs`, `F`), sticky file header pinning at row 0 while scrolling (`diff_sticky_header`), auto-collapsed `linguist-generated` lockfiles (`diff_collapse_generated`), `i` file details popover (`Action::ToggleDiffFileDetails`), `y` raw Git header copy (`Action::YankDiffText`), `Unified` and Gerrit-style `SideBySide` dual-pane layouts with token-similarity alignment (`diff/align.rs`) and dynamic line-number gutter widths (`max_lineno_digits`), collapsible file sections (`za`, `zM`, `zR`) and diffstat quick-jump (`Enter`), soft line wrapping (`wrap_lines`, `zW`) with continuation markers (`↪`), moved-block detection (`color_moved`, `M`), interactive per-hunk context expansion (`+` / `=` / `_`) alongside global blob context splicing (`]` / `[`, `diff/expand.rs`), `Arc<CommitDiff>` sharing, speculative background `DiffDocument` prefetching, and external diff formatter piping (`diff/formatter.rs`, gated on `!options.read_only`).
- **TTY & Editor Handover** (`tty.rs`, `editor.rs`):
  - Temporarily leaves the alternate screen and raw mode to run `$EDITOR` (`e`) or `!` shell commands in `tigrs`'s own foreground process group (when Update Mode is active), then bumps the alternate-screen generation counter on return to guarantee a full screen repaint.

### 2.4 `tigrs-cli` (`crates/tigrs-cli`)
- **CLI Flags & Event Loop** (`args.rs`, `main.rs`):
  - Launches in Update Mode by default (`config.general.read_only = false`), supporting `--read-only` (`--ro`) to lock into Read-Only Mode (`[RO]`) and `--update-mode` to override `general.read_only = true` in `config.toml`.
  - Uses a dedicated input-reader thread and `crossbeam_channel::select!` over `{commits, diff, blame, status, fs_watcher, signals, input}` so background data streams never stall on terminal polling.

---

## 3. Security Boundaries (Protecting Users from Malicious Repos)

When modifying `tigrs-git` or `tigrs-ui`, preserve these defenses tested in `crates/tigrs-ui/tests/security_rce_prevention.rs` and `crates/tigrs-ui/tests/security_coverage_matrix.rs`:
1. **Untrusted by Default, Zero Allowlists & Read-Only Mode Guard (`--read-only` / `[RO]`)**:
   - Every repository is unconditionally untrusted by default; never introduce a `trusted_repos` allowlist.
   - All mutating UI actions must check `app.check_read_only_blocked()` and all mutating `GitEngine` methods must call `self.ensure_writable(...)`.
2. **Always use `safe_git_command` (`crates/tigrs-git/src/path_security.rs`)**:
   - Never call `std::process::Command::new("git")` in production code.
   - `safe_git_command` strips dangerous environment variables (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_EXTERNAL_DIFF`, `GIT_INDEX_FILE`), sets `--no-optional-locks`, `GIT_LITERAL_PATHSPECS=1`, and `GIT_ATTR_SOURCE` (empty tree), pins `core.fsmonitor=false`, `core.hooksPath=/dev/null`, `gc.auto=0`, `maintenance.auto=false`, and `protocol.allow=never`, and dynamically scans `.git/info/attributes` + `.git/config` to neutralize all `filter.*`, `diff.*`, and `merge.*` drivers via `GIT_CONFIG_COUNT`.
3. **Enforce Worktree & Symlink Bounds**:
   - Validate all repository-relative paths with `verify_relative_path` (rejecting `\0`, absolute paths, `..` escapes, pathspec magic, and `.git` components) and `verify_worktree_path_safety` (recursive component-by-component symlink check preventing symlink-before-directory escapes when staging, discarding, or editing files).
   - Always pass file/path arguments after `--` when invoking subprocesses.
4. **Never Trust Repository `core.editor` or `.tigrc`**:
   - Editor resolution in `crates/tigrs-ui/src/editor.rs` skips repository-local `.git/config` (`git config --global` only) so a cloned repository cannot hijack `e` (`edit`) to execute arbitrary commands.

---

## 4. Critical Implementation Gotchas

1. **Commit Message Parsing (`commit.message_raw()`, NOT `commit.message().body()`)**:
   - In `gix`, `MessageRef::body(&self)` returns a `BodyRef` whose `Deref`/`as_bytes()` returns `self.body_without_trailer`—silently stripping the entire trailing paragraph if it contains `Key: Value` or `Key=Value` trailers (`Signed-off-by:`, `Change-Id:`, `Tested:`, etc.)—and `MessageRef.title` splits at `\n\n` rather than `\n`.
   - Always use `parse_commit_title_and_body(commit.message_raw()?.as_ref())` in `crates/tigrs-git/src/diff.rs` so `title` is strictly line 1 and `body` preserves 100% of the commit message and trailers.
2. **CRLF Preservation in Diff Tokenization**:
   - `gix`'s default `ByteLinesWithoutTerminator` strips `\r` along with `\n`, masking pure LF↔CRLF line-ending commits. Always build `InternedInput` by splitting on `\n` only (`crates/tigrs-git/src/diff.rs`).
3. **Syntax Highlighting & Full-Context Expansion Budgets on Multi-File Diffs**:
   - `build_diff_document_cancellable_with_state` (`crates/tigrs-ui/src/diff/layout.rs`) enforces a commit-wide highlight budget (`MemoryProfile::max_diff_highlight_total_lines() = 200` lines at standard context `<= 3`, scaling to `MAX_HIGHLIGHT_LINES = 2,000` when context is expanded) and bounds full-file (`DIFF_CONTEXT_FULL`) blob splicing by a commit-wide `diff_context_full_max_lines` (`50,000` lines, falling back to `20` context lines around each hunk on larger files) so `syntect` regex tokenization never stalls the UI thread on large commits or continuous `]` / `[` presses.
4. **CLI Asset Synchronization**:
   - Whenever CLI flags or version strings in `crates/tigrs-cli/src/args.rs` change, regenerate `assets/completions/*` (`tigrs completions <shell>`) and `assets/man/tigrs.1` (`tigrs man`).

---

## 5. Standard Verification Workflow

Before committing any change, run all four checks from the workspace root:

```bash
# 1. Check formatting
cargo fmt --all -- --check

# 2. Run Clippy with all warnings denied across all targets
cargo clippy --workspace --all-targets -- -D warnings

# 3. Verify 100% public Rustdoc coverage and intra-doc links
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# 4. Run the full unit, differential, PTY, and security test suite
cargo test --workspace
```
