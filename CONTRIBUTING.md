# Contributing to `tigrs`

Thank you for your interest in contributing to `tigrs`! This document outlines the development workflow, coding standards, testing requirements, and licensing guidelines for the repository.

---

## Getting Started

### Prerequisites

- **Rust Toolchain:** Rust `1.88` or newer (Rust 2024 Edition) with `rustfmt` and `clippy`.
- **Git:** Standard `git` CLI (`>= 2.28`, or `>= 2.45` for `reftable` repository tests) for running differential parity tests and staging integration tests.

### Building & Running

```bash
# Build workspace in release mode
cargo build --workspace --release

# Run tigrs on the current repository
cargo run -p tigrs --
```

---

## Verification & Quality Gates

Every pull request must pass the full verification suite with zero warnings:

```bash
# 1. Code formatting check
cargo fmt --all -- --check

# 2. Strict Clippy lints (workspace enforces warn-by-default pedantic/safety lints)
cargo clippy --workspace --all-targets -- -D warnings

# 3. Public API Rustdoc validation
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# 4. Unit, differential, and headless TUI integration test suite
cargo test --workspace
```

---

## Coding Standards & Architectural Invariants

For a detailed guide to the crate layout, concurrency model, and rendering architecture, see [AGENT.md](AGENT.md). Key conventions include:

1. **Pure-Rust & Zero Unsafe Policy**
   - `#![forbid(unsafe_code)]` is strictly enforced across all four workspace crates (`tigrs-core`, `tigrs-git`, `tigrs-ui`, and `tigrs-cli`) with zero `unsafe` blocks.
   - Use `rustix` and `signal-hook` for POSIX terminal, process-group, pipe, and signal operations.
   - Do not introduce C/OpenSSL dependencies; keep `gix` configured with `zlib-rs`.

2. **Error Handling & Panic Freedom**
   - Production library and UI code must never crash on malformed repository data or user input.
   - `unwrap_used` and `expect_used` are lint-checked across the workspace; propagate errors with `Result` and `thiserror` or handle fallback states cleanly.

3. **Security, Read-Only Mode & Untrusted Repository Sandboxing**
   - Every repository is treated as untrusted for RCE sandboxing and launches in **Update Mode** (`read_only = false`) by default. Users can lock any session into **Read-Only Mode** (`read_only = true`, displayed as `[RO]` in the title bar) via `--read-only` (`--ro`) on the CLI, `general.read_only = true` in `config.toml` (overridable via `--update-mode`), `[R] read-only` in the Options Panel (`o`), or `:read-only` (`:ro`) at runtime. All mutating UI actions must check `AppState::check_read_only_blocked()` and all mutating `GitEngine` methods must call `GitEngine::ensure_writable()`.
   - Never invoke `std::process::Command::new("git")` directly in `tigrs-git` or `tigrs-ui`. Always use `safe_git_command` (`crates/tigrs-git/src/path_security.rs`) and `apply_untrusted_repo_env` so untrusted `.git/config` or `.git/info/attributes` files cannot execute arbitrary commands (`core.fsmonitor`, `core.hooksPath`, `diff.external`, `filter.*`, `textconv`, etc.).
   - All repository strings rendered to the terminal must pass through `strip_control_chars` or `filter_sgr_only` (`crates/tigrs-core/src/ansi.rs`).

4. **Documentation (`missing_docs`)**
   - All public structs, enums, functions, methods, and modules across the workspace must have clear Rustdoc comments (`///` or `//!`).

5. **Synthetic Test Data Only**
   - All unit and integration tests must construct synthetic repositories in `tempfile::TempDir` using generic RFC 2606 example identities (`Alice Developer <alice@example.com>`).

---

## Licensing & Intellectual Property Boundaries

`tigrs` is dual-licensed under **`MIT OR Apache-2.0`** ([`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE)).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in `tigrs` by you, as defined in the Apache-2.0 license, shall be dual-licensed under `MIT OR Apache-2.0`, without any additional terms or conditions.

To preserve clear provenance and clean-room boundaries across all components:

1. **Clean-Room Rust Implementation (`tigrs-core`, `tigrs-git`, `tigrs-ui`, `tigrs-cli`)**
   - All components—including the configuration engine (`crates/tigrs-core/src/config.rs`), builtin language function-header drivers (`crates/tigrs-git/src/userdiff_table.rs`), TUI state machine, damage-tracked renderer, and DAG arena—are original pure-Rust implementations written from public specifications rather than translated from C source files.
2. **Dependency Licenses**
   - All third-party Cargo dependencies must use permissive or weak-copyleft licenses compatible with `MIT OR Apache-2.0` redistribution, as documented in [THIRD_PARTY.md](THIRD_PARTY.md).
