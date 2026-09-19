# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Security Model: Untrusted Git Repositories

`tigrs` is designed to safely inspect **untrusted Git repositories**—such as freshly cloned third-party repositories, downloaded archives, or repositories on shared filesystems—without risking arbitrary code execution or accidental repository mutations.

Every repository is treated as **untrusted by default** with **zero repository allowlists**.

### Built-in Defenses

1. **Update Mode Default & Optional Read-Only Lock (`--read-only` / `[RO]`)**
   - `tigrs` launches in **Update Mode** (`read_only = false`) by default while keeping all untrusted-repository RCE sandboxing unconditionally active. Users can lock any session into **Read-Only Mode** (`read_only = true`, displayed as `[RO]` in the title bar) via `--read-only` (`--ro`) on the CLI, `general.read_only = true` in `config.toml` (overridable via `--update-mode`), `[R] read-only` in the Options Panel (`o`), or `:set read-only = true` (`:read-only` / `:ro`) at runtime.
   - All repository-mutating operations in `GitEngine` (`stage_file`, `unstage_file`, `discard_file_changes`, `discard_untracked_file`, `stage_hunk`, `unstage_hunk`, `stage_line`, `unstage_line`, `stage_lines`, `unstage_lines`) enforce `GitEngine::ensure_writable()` and return `TigError::ReadOnly` when locked.
   - All UI staging, reverting, `$EDITOR` (`e`), shell execution (`:!cmd`, `:+cmd`), and in-repository file write paths (`:save-config`, history persistence) enforce `AppState::check_read_only_blocked()`.
   - All background `git` commands pass `--no-optional-locks`, `-c gc.auto=0`, `-c maintenance.auto=false`, and `-c fetch.writeCommitGraph=false` so inspecting a repository never writes `.git/index.lock` or modifies `.git/`.

2. **Two-Tier Git Sandboxing (`discovery.rs` & `path_security.rs`)**
   - **Tier-1 In-Process `gix` Hardening (`crates/tigrs-git/src/discovery.rs`)**: Opens repositories with `filter_config_section` rejecting `filter`, `diff`, and `merge` driver definitions from all configuration sources, and validates `core.worktree` (`verify_untrusted_core_worktree`) so a malicious `.git/config` cannot redirect working-tree reads or writes outside the repository root.
   - **Tier-2 Subprocess Sandbox (`crates/tigrs-git/src/path_security.rs` — `safe_git_command` & `apply_untrusted_repo_env`)**:
     - Clears dangerous environment variables (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_EXTERNAL_DIFF`, `GIT_EXEC_PATH`, `GIT_TEMPLATE_DIR`) and sets `GIT_CONFIG_NOSYSTEM=1`, `GIT_PAGER=cat`, `GIT_LITERAL_PATHSPECS=1`, and `GIT_ATTR_SOURCE` to the SHA-1/SHA-256 empty tree hash.
     - Forces `-c` command-line overrides neutralizing executable directives (`core.fsmonitor=false`, `core.hooksPath=/dev/null`, `core.sshCommand=false`, `core.gitProxy=false`, `protocol.allow=never`, `gpg.program=/bin/false`, `interactive.diffFilter=`) and passes `--no-ext-diff`, `--no-textconv`, and `--literal-pathspecs`.
     - Dynamically scans `.git/config`, `.git/config.worktree`, `commondir`, nested `[include]` chains, and `$GIT_DIR/info/attributes` via `ConfigSanitizationScanner` to neutralize every `filter.*`, `diff.*`, `merge.*`, `alias.*`, `pager.*`, `credential.*`, and `remote.*` driver via `GIT_CONFIG_COUNT`.

3. **Worktree Path, Symlink, Editor, and Config Isolation**
   - `verify_relative_path` and `verify_worktree_path_safety` perform recursive component-by-component symlink resolution (`MAX_SYMLINK_DEPTH = 40`), rejecting directory traversal (`..`), `.git` path components, pathspec magic prefixes (`:(top)`), and chained symlink escapes.
   - Editor resolution (`crates/tigrs-ui/src/editor.rs`) queries only `--global` Git configuration (`GIT_EDITOR`, `VISUAL`, `EDITOR`)—never repository-local `core.editor` or `.tigrc`—and prefixes leading `-` or `+` file paths with `./` after control-character stripping.
   - Configuration and history discovery (`crates/tigrs-core/src/config.rs`, `crates/tigrs-core/src/history.rs`) require strictly absolute paths (`path.is_absolute()`) for `HOME`, `XDG_CONFIG_HOME`, `TIGRS_CONFIG`, `XDG_DATA_HOME`, and `TIG_HISTORY`, preventing relative-path config hijacking from the current working directory.

4. **Context-Aware Shell Macro Quoting (`crates/tigrs-core/src/quote.rs`, `macro_ctx.rs`)**
   - `QuoteState` tracks unquoted, single-quoted (`'`), double-quoted (`"`), backtick subshell (`` ` ``), and nested `$()` command-substitution states when expanding runtime `%(...)` macros, ensuring every token is safely POSIX-quoted.

5. **Terminal Escape & Trojan Source BiDi Sanitization (`crates/tigrs-core/src/ansi.rs`, `line_buffer.rs`)**
   - All repository strings (commit messages, author names, ref names, hunk headers, file paths, and blob/diff contents) are sanitized via `strip_control_chars` or `filter_sgr_only` before rendering, stripping C0/C1 8-bit control codes (`U+0080..=U+009F`), ANSI/CSI/OSC escape sequences (including OSC 52 clipboard writes), and Trojan Source Unicode BiDi overrides (`U+202A..=U+202E`, `U+2066..=U+2069`).

6. **File Descriptor Hygiene**
   - All repository packfiles and internal descriptors are opened with `O_CLOEXEC`, preventing file descriptor leakage when handing over the controlling TTY to `$EDITOR` or interactive shells.

## Reporting a Vulnerability

If you discover a security vulnerability in `tigrs`—especially any bypass of the untrusted-repository sandbox, read-only mode enforcement, or terminal escape sanitization—please report it privately rather than opening a public GitHub issue:

- **Contact:** David Lin (<dtwlin@gmail.com>)
- **GitHub Security Advisories:** Use the ["Report a vulnerability"](https://github.com/dtwlin/tigrs/security/advisories/new) button under the repository's **Security** tab.

Please include a minimal reproduction recipe (such as a sample `.git/config` or test script) and the output of `tigrs --version`. You can expect an initial response within 72 hours.
