# Changelog

All notable changes to `tigrs` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-09-27

### Added
- **Two-Tier Hybrid Git Engine (`tigrs-git`)**: In-process `gix` object graph traversal, tree/blob diffing (`imara-diff`), and blame combined with hardened, sandboxed Git CLI plumbing for index/worktree mutations and `reftable` repositories.
- **Non-Blocking Streaming TUI (`tigrs-ui`)**: Damage-tracked terminal renderer (`HeadlessTerminal` + `ScreenGrid` cell diffing with DEC 2026 synchronized output), dual-pane horizontal/vertical split views, and curved Unicode box-drawing revision graph DAG (`CompactGraphRow`).
- **Complete 14-View Ecosystem**: `MainView`, `DiffView` (unified & side-by-side), `StatusView`, `StageView`, `LogView`, `BlobView`, `BlameView`, `TreeView`, `RefsView`, `StashView`, `ReflogView`, `GrepView`, `PagerView`, and `HelpView`, plus the interactive `[o]` Options Panel and `[t]` UI Theme Picker.
- **Interactive Staging & Worktree Operations**: File, hunk, and single/multi-line staging, unstaging, and discarding (`u`, `!`) preserving CRLF line endings and raw non-UTF-8 OS paths.
- **Untrusted Repository RCE Sandboxing & Read-Only Mode (`[RO]`)**: Neutralization of repository-local `core.fsmonitor`, `core.hooksPath`, `core.editor`, `core.pager`, `diff.external`, `filter.*`, `textconv`, and path-traversal symlinks, plus one-key Read-Only lock (`--read-only` / `:ro`).
- **TOML Configuration (`tigrs-core`)**: Configurable keybindings, view options, custom user commands with shell-quoted `%(commit)`/`%(file)`/`%(branch)` macros, and 6 built-in UI color schemes (`default`, `calm-dark`, `high-contrast`, `solarized-dark`, `light`, `mono`).
