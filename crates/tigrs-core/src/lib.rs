// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Core foundational utilities, domain errors, security sanitizers,
//! and concurrency primitives for tigrs.

#![forbid(unsafe_code)]

/// Canonical release version string of `tigrs`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod ansi;
pub mod cancel;
pub mod cgroup;
pub mod config;
pub mod config_enums;
pub mod error;
pub mod history;
pub mod line_buffer;
pub mod macro_ctx;
pub mod pool;
pub mod quote;
pub mod security;

pub use ansi::{
    filter_sgr_only, sanitize_string, strip_control_chars, truncate_visible_width, visible_width,
};
pub use cancel::{CancellationSource, CancellationToken, GenerationCounter};
pub use cgroup::{
    MemoryPressureLevel, MemoryUsage, evaluate_memory_pressure, read_cgroup_memory_usage,
};
pub use config::{
    ColorSpec, Config, GeneralConfig, MainSubjectRuleConfig, PerformanceConfig, SecuritySettings,
    ViewConfig, expand_tilde,
};
pub use config_enums::{
    AuthorFormat, CommitOrder, DateFormat, DiffHintsMode, DiffIndicator, DiffLayout,
    DiffPresentation, GraphDisplay, IgnoreSpace, LineGraphics, MainAuthorColor, MainSpotlight,
    MemoryProfile, ParseConfigEnumError, UiThemeId, WordDiffPairing,
};
pub use error::{Result, TigError};
pub use history::{DEFAULT_HISTORY_SIZE, HistoryManager};
pub use line_buffer::{LineBuffer, LineBufferIter};
pub use macro_ctx::MacroContext;
pub use pool::{
    ComputePool, default_compute_threads, global_compute_pool, init_global_compute_pool,
    send_or_yield,
};
pub use quote::{
    MacroLookup, interpolate_command, shell_quote, split_shell_words, verify_shell_argument_safety,
    verify_shell_safety,
};
pub use security::SecurityConfig;
