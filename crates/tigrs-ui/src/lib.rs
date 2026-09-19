// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Terminal user interface, dual-stream TTY controller, event dispatch,
//! and views for tigrs.

#![forbid(unsafe_code)]

pub mod app;
pub mod diff;
pub mod editor;
pub mod graph;
pub mod headless;
pub mod highlight;
pub mod keymap;
pub mod options;
pub mod prompt;
pub mod search;
pub mod signal;
pub mod term_cap;
pub mod tty;
pub mod ui_theme;
pub mod view;

pub use app::{
    AppState, Flow, PaneLayout, ViewKind, ViewLayout, ViewManager, execute_parsed_command,
    handle_event, handle_event_with_dimensions, parse_git_grep_output, render_active, run_app,
    run_blame_app, run_blob_app, run_diff_app, run_git_grep, run_grep_app, run_help_app,
    run_log_app, run_pager_app, run_reflog_app, run_refs_app, run_stash_app, run_status_app,
    run_tree_app, set_debug_frame_stats,
};
pub use editor::{
    EditRefusal, EditTarget, EditorInvocation, build_editor_command_line, hunk_new_lineno,
    resolve_edit_target, resolve_editor,
};
pub use graph::{
    CompactGraphRow, GRAPH_PALETTE_ANSI, GraphGlyph, GraphRowBuilder, MAX_GRAPH_LANES,
};
pub use headless::{Cell, CellAttrs, Color, HeadlessTerminal};
pub use highlight::highlight_code_with_profile;
pub use keymap::{
    Action, Key, KeySequence, KeymapEngine, KeymapLookupResult, KeymapScope, ParseKeyError,
    RunCommand, RunFlags,
};
pub use options::{
    AuthorFormat, CommitOrder, DIFF_CONTEXT_FULL, DateFormat, DiffIndicator, DiffLayout,
    DiffPresentation, GraphDisplay, IgnoreSpace, LineGraphics, MenuAction, OptionId,
    OptionMenuState, TOGGLE_MENU_ITEMS, ToggleMenuItem, UiThemeId, ViewOptions, abbreviate_author,
    author_email_prefix, format_timestamp,
};
pub use prompt::{ParsedCommand, PromptKind, PromptResult, PromptState};
pub use search::{ActiveSearch, SearchDirection, SearchPattern, SearchResult};
pub use signal::{SignalCoordinator, UiSignal, record_exit_signal, take_last_exit_signal};
pub use term_cap::{
    ColorProfile, TerminalCapabilities, ansi256_to_ansi16, downgrade_ansi, rgb_to_ansi16,
    rgb_to_ansi256,
};
pub use tty::{
    InputReader, TtyController, TtyHandoverGuard, assert_fd_hygiene, install_panic_hook,
};
pub use ui_theme::{ThemeCategory, UiPalette};
pub use view::{
    BlameHistoryEntry, BlameView, BlobView, ChangesKind, ChangesRow, DiffLineType, DiffView,
    DiffViewLine, GrepMatch, GrepView, HelpRow, HelpView, LogLine, LogLineKind, LogView, MainRow,
    MainView, PagerView, ReflogView, RefsView, StashView, StatusRow, StatusView, TreeRow, TreeView,
};
