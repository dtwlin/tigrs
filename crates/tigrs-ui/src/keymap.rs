// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Keymap engine and multi-key sequence dispatcher.
//!
//! Provides pure-Rust, zero-unsafe key event mapping, key sequence parsing,
//! prefix trie matching, view-scoped keymaps with fallback to generic keymaps,
//! and complete default Tig keybindings.

use crate::app::ViewKind;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use indexmap::IndexMap;
use smallvec::SmallVec;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Errors that can occur when parsing key names or sequences.
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub enum ParseKeyError {
    /// The input string was empty.
    #[error("empty key string")]
    Empty,
    /// An unclosed angle bracket token was encountered, e.g. `<Ctrl-`.
    #[error("unclosed angle bracket: '{0}'")]
    UnclosedAngleBracket(String),
    /// An unrecognized key name was found inside `<...>`.
    #[error("unknown key name: '{0}'")]
    UnknownKeyName(String),
    /// An unrecognized modifier was found.
    #[error("unknown modifier: '{0}'")]
    UnknownModifier(String),
    /// An unrecognized action name was found.
    #[error("unknown action: '{0}'")]
    UnknownAction(String),
    /// An unrecognized keymap scope was found.
    #[error("unknown keymap scope: '{0}'")]
    UnknownScope(String),
}

/// A single normalized keypress, composed of a key code and active modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// Normalized terminal key code.
    pub code: KeyCode,
    /// Active modifier bitflags (`CONTROL`, `ALT`, `SHIFT`).
    pub modifiers: KeyModifiers,
}

impl Key {
    /// Creates a normalized key instance.
    ///
    /// Normalizes Ctrl keys to lowercase chars (since Ctrl keybindings in Tig
    /// are case-insensitive), and removes the Shift modifier from uppercase
    /// character keys to ensure cross-terminal consistency.
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        let (code, modifiers) = match code {
            KeyCode::Char(c) if modifiers.contains(KeyModifiers::CONTROL) => {
                (KeyCode::Char(c.to_ascii_lowercase()), modifiers)
            }
            KeyCode::Char(c) if c.is_ascii_uppercase() => {
                (KeyCode::Char(c), modifiers & !KeyModifiers::SHIFT)
            }
            _ => (code, modifiers),
        };
        Self { code, modifiers }
    }

    /// Creates a key with the Ctrl modifier and a character code.
    pub fn ctrl(c: char) -> Self {
        Self::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Creates a simple character key with no modifiers.
    pub fn from_char(c: char) -> Self {
        Self::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Creates a key from a code with no modifiers.
    pub fn from_code(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::NONE)
    }
}

impl From<char> for Key {
    fn from(c: char) -> Self {
        Self::from_char(c)
    }
}

impl From<KeyCode> for Key {
    fn from(code: KeyCode) -> Self {
        Self::from_code(code)
    }
}

impl From<(KeyCode, KeyModifiers)> for Key {
    fn from((code, modifiers): (KeyCode, KeyModifiers)) -> Self {
        Self::new(code, modifiers)
    }
}

impl Key {
    /// Parses a single key token string, e.g. `<Enter>`, `<Ctrl-N>`, `<Down>`, `q`.
    pub fn parse(s: &str) -> Result<Self, ParseKeyError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ParseKeyError::Empty);
        }

        if s.starts_with('<') && s.ends_with('>') {
            let inner = &s[1..s.len() - 1];

            // 1. Check for modifiers like <Ctrl-X>, <C-X>, <Alt-X>, <M-X>, <Shift-X>
            if let Some(rest) = inner
                .strip_prefix("Ctrl-")
                .or_else(|| inner.strip_prefix("C-"))
            {
                let sub_key = Self::parse_single_token(rest)?;
                return Ok(Self::new(
                    sub_key.code,
                    sub_key.modifiers | KeyModifiers::CONTROL,
                ));
            }
            if let Some(rest) = inner
                .strip_prefix("Alt-")
                .or_else(|| inner.strip_prefix("M-"))
            {
                let sub_key = Self::parse_single_token(rest)?;
                return Ok(Self::new(
                    sub_key.code,
                    sub_key.modifiers | KeyModifiers::ALT,
                ));
            }
            if let Some(rest) = inner
                .strip_prefix("Shift-")
                .or_else(|| inner.strip_prefix("S-"))
            {
                let sub_key = Self::parse_single_token(rest)?;
                return Ok(Self::new(
                    sub_key.code,
                    sub_key.modifiers | KeyModifiers::SHIFT,
                ));
            }

            Self::parse_single_token(inner)
        } else {
            // Single UTF-8 character or raw string
            let mut chars = s.chars();
            let first = chars.next().ok_or(ParseKeyError::Empty)?;
            if chars.next().is_none() {
                Ok(Self::from_char(first))
            } else {
                // If multiple characters without brackets, try matching named keys like "Enter"
                Self::parse_single_token(s)
            }
        }
    }

    fn parse_single_token(name: &str) -> Result<Self, ParseKeyError> {
        let lower = name.to_ascii_lowercase();
        let key = match lower.as_str() {
            "enter" | "return" => KeyCode::Enter,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "tab" => KeyCode::Tab,
            "backtab" | "shifttab" => KeyCode::BackTab,
            "esc" | "escape" => KeyCode::Esc,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "ins" | "insert" => KeyCode::Insert,
            "del" | "delete" => KeyCode::Delete,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pgup" | "pageup" | "scrollback" | "sback" => KeyCode::PageUp,
            "pgdown" | "pagedown" | "scrollfwd" | "sfwd" => KeyCode::PageDown,
            "hash" => KeyCode::Char('#'),
            "lessthan" | "lt" => KeyCode::Char('<'),
            "singlequote" => KeyCode::Char('\''),
            "doublequote" => KeyCode::Char('"'),
            "shiftleft" => return Ok(Key::new(KeyCode::Left, KeyModifiers::SHIFT)),
            "shiftright" => return Ok(Key::new(KeyCode::Right, KeyModifiers::SHIFT)),
            "shiftdelete" | "shiftdel" => {
                return Ok(Key::new(KeyCode::Delete, KeyModifiers::SHIFT));
            }
            "shifthome" => return Ok(Key::new(KeyCode::Home, KeyModifiers::SHIFT)),
            "shiftend" => return Ok(Key::new(KeyCode::End, KeyModifiers::SHIFT)),
            f if f.starts_with('f') && f.len() >= 2 => {
                if let Ok(num) = f[1..].parse::<u8>() {
                    KeyCode::F(num)
                } else {
                    return Err(ParseKeyError::UnknownKeyName(name.to_string()));
                }
            }
            _ if name.chars().count() == 1 => {
                let Some(ch) = name.chars().next() else {
                    return Err(ParseKeyError::UnknownKeyName(name.to_string()));
                };
                KeyCode::Char(ch)
            }
            _ => return Err(ParseKeyError::UnknownKeyName(name.to_string())),
        };
        Ok(Key::from_code(key))
    }
}

impl From<KeyEvent> for Key {
    fn from(event: KeyEvent) -> Self {
        let mods =
            event.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        Self::new(event.code, mods)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let has_ctrl = self.modifiers.contains(KeyModifiers::CONTROL);
        let has_alt = self.modifiers.contains(KeyModifiers::ALT);
        let has_shift = self.modifiers.contains(KeyModifiers::SHIFT);

        if has_ctrl || has_alt || has_shift {
            write!(f, "<")?;
            if has_ctrl {
                write!(f, "Ctrl-")?;
            }
            if has_alt {
                write!(f, "Alt-")?;
            }
            if has_shift {
                write!(f, "Shift-")?;
            }
            match self.code {
                KeyCode::Char(c) => write!(f, "{}", c.to_ascii_uppercase())?,
                KeyCode::Enter => write!(f, "Enter")?,
                KeyCode::Backspace => write!(f, "Backspace")?,
                KeyCode::Tab => write!(f, "Tab")?,
                KeyCode::BackTab => write!(f, "BackTab")?,
                KeyCode::Esc => write!(f, "Esc")?,
                KeyCode::Left => write!(f, "Left")?,
                KeyCode::Right => write!(f, "Right")?,
                KeyCode::Up => write!(f, "Up")?,
                KeyCode::Down => write!(f, "Down")?,
                KeyCode::Home => write!(f, "Home")?,
                KeyCode::End => write!(f, "End")?,
                KeyCode::PageUp => write!(f, "PageUp")?,
                KeyCode::PageDown => write!(f, "PageDown")?,
                KeyCode::Delete => write!(f, "Delete")?,
                KeyCode::Insert => write!(f, "Insert")?,
                KeyCode::F(num) => write!(f, "F{num}")?,
                _ => write!(f, "?")?,
            }
            write!(f, ">")
        } else {
            match self.code {
                KeyCode::Char(' ') => write!(f, "<Space>"),
                KeyCode::Char('<') => write!(f, "<Lt>"),
                KeyCode::Char('#') => write!(f, "<Hash>"),
                KeyCode::Char(c) => write!(f, "{c}"),
                KeyCode::Enter => write!(f, "<Enter>"),
                KeyCode::Backspace => write!(f, "<Backspace>"),
                KeyCode::Tab => write!(f, "<Tab>"),
                KeyCode::BackTab => write!(f, "<BackTab>"),
                KeyCode::Esc => write!(f, "<Esc>"),
                KeyCode::Left => write!(f, "<Left>"),
                KeyCode::Right => write!(f, "<Right>"),
                KeyCode::Up => write!(f, "<Up>"),
                KeyCode::Down => write!(f, "<Down>"),
                KeyCode::Home => write!(f, "<Home>"),
                KeyCode::End => write!(f, "<End>"),
                KeyCode::PageUp => write!(f, "<PageUp>"),
                KeyCode::PageDown => write!(f, "<PageDown>"),
                KeyCode::Delete => write!(f, "<Delete>"),
                KeyCode::Insert => write!(f, "<Insert>"),
                KeyCode::F(num) => write!(f, "<F{num}>"),
                _ => write!(f, "<?>"),
            }
        }
    }
}

/// Backing storage for a [`KeySequence`].
///
/// Essentially every binding is one key (`j`) or two (`gg`, `<C-W><C-W>`), so
/// two elements of inline capacity keep key lookup entirely allocation-free on
/// the input hot path. Longer sequences remain expressible and simply spill.
pub type KeySequenceKeys = SmallVec<[Key; 2]>;

/// A sequence of one or more keys pressed in succession.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct KeySequence(pub KeySequenceKeys);

impl KeySequence {
    /// Creates a key sequence from an existing vector of keys.
    ///
    /// The vector's allocation is reused as-is rather than copied.
    pub fn new(keys: Vec<Key>) -> Self {
        Self(SmallVec::from_vec(keys))
    }

    /// Creates a key sequence with a single key.
    pub fn single(key: Key) -> Self {
        Self(smallvec::smallvec![key])
    }

    /// Number of keys in the sequence.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the key sequence is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns a slice of keys.
    pub fn as_slice(&self) -> &[Key] {
        &self.0
    }

    /// Parses a sequence string like `"qa"`, `"gg"`, `"<Esc>q"`, `"<C-W><C-W>"`.
    pub fn parse(s: &str) -> Result<Self, ParseKeyError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ParseKeyError::Empty);
        }

        let mut keys = KeySequenceKeys::new();
        let mut chars = s.chars().peekable();

        while let Some(&ch) = chars.peek() {
            if ch == '<' {
                chars.next();
                let mut token = String::from('<');
                let mut found_end = false;
                for c in chars.by_ref() {
                    token.push(c);
                    if c == '>' {
                        found_end = true;
                        break;
                    }
                }
                if !found_end {
                    return Err(ParseKeyError::UnclosedAngleBracket(token));
                }
                keys.push(Key::parse(&token)?);
            } else {
                chars.next();
                keys.push(Key::from_char(ch));
            }
        }

        if keys.is_empty() {
            return Err(ParseKeyError::Empty);
        }
        Ok(Self(keys))
    }
}

impl fmt::Display for KeySequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for k in &self.0 {
            write!(f, "{k}")?;
        }
        Ok(())
    }
}

bitflags::bitflags! {
    /// Flags associated with user-defined shell or prompt commands.
    ///
    /// These correspond to the prefix sigils Tig accepts in a `bind` command's
    /// action, e.g. `bind main B ?git checkout -b %(prompt)`.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct RunFlags: u8 {
        /// Run silently in background without status feedback (`@`).
        const SILENT = 1 << 0;
        /// Prompt for confirmation before running (`?`).
        const CONFIRM = 1 << 1;
        /// Exit Tig after running command (`<`).
        const EXIT = 1 << 2;
        /// Internal Tig prompt command (`:`).
        const INTERNAL = 1 << 3;
        /// Echo command output in the status bar (`+`).
        const ECHO = 1 << 4;
        /// Quick command execution (`>`).
        const QUICK = 1 << 5;
    }
}

/// User-defined command definition (e.g. from `bind main B ?git checkout -b %(prompt)`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RunCommand {
    /// Execution prefix flags (`!`, `@`, `?`, `<`, `>`).
    pub flags: RunFlags,
    /// Unexpanded shell command template containing optional `%(...)` macros.
    pub command: String,
}

/// Canonical Tig request actions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    // View switching
    /// Open or switch focus to the given view (`view-main`, `view-diff`, ...).
    OpenView(ViewKind),
    /// Open or switch focus to the staging view (`view-stage`).
    ///
    /// Staging is not a [`ViewKind`] of its own: it re-targets the diff view at
    /// the selected working-tree change, so it keeps a dedicated variant.
    ViewStage,

    // View manipulation
    /// Drill down into the selected item (`enter`).
    Enter,
    /// Navigate backwards in blame history or directory hierarchy (`back`).
    Back,
    /// Move to the next item in the parent view (`next`).
    Next,
    /// Move to the previous item in the parent view (`previous`).
    Previous,
    /// Navigate to parent commit or directory (`parent`).
    Parent,
    /// Cycle focus to the next split pane (`view-next`).
    ViewNext,
    /// Reload and refresh the active view (`refresh`).
    Refresh,
    /// Toggle between split-view and full-screen layout (`maximize`).
    Maximize,
    /// Close the active view pane (`view-close`).
    ViewClose,
    /// Quit the application (`quit`).
    Quit,

    // View-specific actions
    /// Stage or unstage the selected status item (`status-update`).
    StatusUpdate,
    /// Revert working tree changes for the selected status item (`status-revert`).
    StatusRevert,
    /// Launch merge tool for conflicted file (`status-merge`).
    StatusMerge,
    /// Stage or unstage single diff line (`stage-update-line`).
    StageUpdateLine,
    /// Stage or unstage diff hunk (`stage-update-part`).
    StageUpdatePart,
    /// Split current diff hunk into smaller chunks (`stage-split-chunk`).
    StageSplitChunk,
    /// Jump to the next diff hunk header (`next-hunk`).
    NextHunk,
    /// Jump to the previous diff hunk header (`prev-hunk`).
    PrevHunk,
    /// Jump to the next file header in diff (`next-file`).
    NextFile,
    /// Jump to the previous file header in diff (`prev-file`).
    PrevFile,
    /// Toggle fold/collapse state of the current file in diff view (`toggle-file-fold`).
    ToggleFileFold,
    /// Fold/collapse all files in diff view (`fold-all`).
    FoldAllFiles,
    /// Unfold/expand all files in diff view (`unfold-all`).
    UnfoldAllFiles,
    /// Expand context around the current diff hunk (`expand-hunk-context`).
    ExpandHunkContext,
    /// Shrink extra context around the current diff hunk (`shrink-hunk-context`).
    ShrinkHunkContext,
    /// Toggle the on-demand File Details popover (`i` in diff/stage views).
    ToggleDiffFileDetails,
    /// Yank/copy the current diff line or real `diff --git` / `@@` header (`y` in diff/stage views).
    YankDiffText,

    // Cursor navigation
    /// Move cursor up one line (`move-up`).
    MoveUp,
    /// Move cursor down one line (`move-down`).
    MoveDown,
    /// Move cursor up one page (`move-page-up`).
    MovePageUp,
    /// Move cursor down one page (`move-page-down`).
    MovePageDown,
    /// Move cursor up half a page (`move-half-page-up`).
    MoveHalfPageUp,
    /// Move cursor down half a page (`move-half-page-down`).
    MoveHalfPageDown,
    /// Jump cursor to the first line (`move-first-line`).
    MoveFirstLine,
    /// Jump cursor to the last line (`move-last-line`).
    MoveLastLine,

    // Scrolling
    /// Scroll viewport up one line without moving cursor (`scroll-line-up`).
    ScrollLineUp,
    /// Scroll viewport down one line without moving cursor (`scroll-line-down`).
    ScrollLineDown,
    /// Scroll viewport up one page (`scroll-page-up`).
    ScrollPageUp,
    /// Scroll viewport down one page (`scroll-page-down`).
    ScrollPageDown,
    /// Scroll viewport horizontally to column 0 (`scroll-first-col`).
    ScrollFirstCol,
    /// Scroll viewport left (`scroll-left`).
    ScrollLeft,
    /// Scroll viewport right (`scroll-right`).
    ScrollRight,

    // Searching
    /// Open forward search prompt (`search`).
    Search,
    /// Open backward search prompt (`search-back`).
    SearchBack,
    /// Jump to next search match (`find-next`).
    FindNext,
    /// Jump to previous search match (`find-prev`).
    FindPrev,

    // Options & toggles
    /// Open interactive option toggle menu (`options`).
    Options,
    /// Toggle sort order ascending/descending (`toggle-sort-order`).
    ToggleSortOrder,
    /// Cycle sort field (`toggle-sort-field`).
    ToggleSortField,
    /// Toggle a specific display option by `OptionId`.
    ToggleOption(crate::options::OptionId),
    /// Adjust diff context line count by delta (`toggle-diff-context`).
    ToggleDiffContext(i8),
    /// Jump cursor to `HEAD` commit (`goto-head`).
    GotoHead,

    // Misc
    /// Open selected file at current line in external editor (`edit`).
    Edit,
    /// Open command-line prompt (`:`).
    Prompt,
    /// Force full screen clear and redraw (`screen-redraw`).
    ScreenRedraw,
    /// Stop background streaming tasks (`stop-loading`).
    StopLoading,
    /// Display version information in status bar (`show-version`).
    ShowVersion,
    /// Suspend tigrs and return to the parent shell (`suspend` / `Ctrl-Z`).
    Suspend,
    /// No-op unbound action (`none`).
    None,

    // User-defined run command
    /// Execute user-defined external or internal command.
    Run(Arc<RunCommand>),
}

impl Action {
    /// Returns the canonical Tig action name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::OpenView(kind) => kind.action_name(),
            Self::ViewStage => "view-stage",
            Self::Enter => "enter",
            Self::Back => "back",
            Self::Next => "next",
            Self::Previous => "previous",
            Self::Parent => "parent",
            Self::ViewNext => "view-next",
            Self::Refresh => "refresh",
            Self::Maximize => "maximize",
            Self::ViewClose => "view-close",
            Self::Quit => "quit",
            Self::StatusUpdate => "status-update",
            Self::StatusRevert => "status-revert",
            Self::StatusMerge => "status-merge",
            Self::StageUpdateLine => "stage-update-line",
            Self::StageUpdatePart => "stage-update-part",
            Self::StageSplitChunk => "stage-split-chunk",
            Self::NextHunk => "next-hunk",
            Self::PrevHunk => "prev-hunk",
            Self::NextFile => "next-file",
            Self::PrevFile => "prev-file",
            Self::ToggleFileFold => "toggle-file-fold",
            Self::FoldAllFiles => "fold-all",
            Self::UnfoldAllFiles => "unfold-all",
            Self::ExpandHunkContext => "expand-hunk-context",
            Self::ShrinkHunkContext => "shrink-hunk-context",
            Self::ToggleDiffFileDetails => "toggle-file-details",
            Self::YankDiffText => "yank-diff",
            Self::MoveUp => "move-up",
            Self::MoveDown => "move-down",
            Self::MovePageUp => "move-page-up",
            Self::MovePageDown => "move-page-down",
            Self::MoveHalfPageUp => "move-half-page-up",
            Self::MoveHalfPageDown => "move-half-page-down",
            Self::MoveFirstLine => "move-first-line",
            Self::MoveLastLine => "move-last-line",
            Self::ScrollLineUp => "scroll-line-up",
            Self::ScrollLineDown => "scroll-line-down",
            Self::ScrollPageUp => "scroll-page-up",
            Self::ScrollPageDown => "scroll-page-down",
            Self::ScrollFirstCol => "scroll-first-col",
            Self::ScrollLeft => "scroll-left",
            Self::ScrollRight => "scroll-right",
            Self::Search => "search",
            Self::SearchBack => "search-back",
            Self::FindNext => "find-next",
            Self::FindPrev => "find-prev",
            Self::Options => "options",
            Self::ToggleSortOrder => "toggle-sort-order",
            Self::ToggleSortField => "toggle-sort-field",
            Self::ToggleOption(id) => id.action_name(),
            Self::ToggleDiffContext(n) if *n < 0 => "diff-context-down",
            Self::ToggleDiffContext(_) => "diff-context-up",
            Self::GotoHead => "goto-head",
            Self::Edit => "edit",
            Self::Prompt => "prompt",
            Self::ScreenRedraw => "screen-redraw",
            Self::StopLoading => "stop-loading",
            Self::ShowVersion => "show-version",
            Self::Suspend => "suspend",
            Self::None => "none",
            Self::Run(_) => "run",
        }
    }

    /// Parses an action string (e.g. `"move-down"`, `"quit"`, `":toggle line-number"`).
    pub fn parse(s: &str) -> Result<Self, ParseKeyError> {
        let s = s.trim();
        let lower = s.to_ascii_lowercase();

        // Check OPTIONS_REGISTRY first for any option toggle action or `:toggle <option>`
        if let Some(target) = lower
            .strip_prefix("toggle-")
            .or_else(|| lower.strip_prefix(":toggle "))
            .map(str::trim)
            && let Some(desc) = crate::options::find_descriptor(target)
        {
            return Ok(Self::ToggleOption(desc.id));
        }
        for desc in crate::options::OPTIONS_REGISTRY {
            if lower == desc.id.action_name() {
                return Ok(Self::ToggleOption(desc.id));
            }
        }

        if let Some(kind) = ViewKind::from_action_name(&lower) {
            return Ok(Self::OpenView(kind));
        }

        let action = match lower.as_str() {
            "view-stage" => Self::ViewStage,
            "enter" => Self::Enter,
            "back" => Self::Back,
            "next" => Self::Next,
            "previous" => Self::Previous,
            "parent" => Self::Parent,
            "view-next" => Self::ViewNext,
            "refresh" => Self::Refresh,
            "maximize" => Self::Maximize,
            "view-close" => Self::ViewClose,
            "quit" => Self::Quit,
            "status-update" => Self::StatusUpdate,
            "status-revert" => Self::StatusRevert,
            "status-merge" => Self::StatusMerge,
            "stage-update-line" => Self::StageUpdateLine,
            "stage-update-part" => Self::StageUpdatePart,
            "stage-split-chunk" => Self::StageSplitChunk,
            "next-hunk" | ":/^@@" => Self::NextHunk,
            "prev-hunk" | ":?^@@" => Self::PrevHunk,
            "next-file" => Self::NextFile,
            "prev-file" => Self::PrevFile,
            "toggle-file-fold" | "toggle-fold" => Self::ToggleFileFold,
            "fold-all" | "fold-all-files" => Self::FoldAllFiles,
            "unfold-all" | "unfold-all-files" => Self::UnfoldAllFiles,
            "expand-hunk-context" | "expand-hunk" => Self::ExpandHunkContext,
            "shrink-hunk-context" | "shrink-hunk" => Self::ShrinkHunkContext,
            "toggle-file-details" | "file-details" | "diff-file-details" => {
                Self::ToggleDiffFileDetails
            }
            "yank-diff" | "copy-diff" | "yank-header" => Self::YankDiffText,
            "move-up" => Self::MoveUp,
            "move-down" => Self::MoveDown,
            "move-page-up" => Self::MovePageUp,
            "move-page-down" => Self::MovePageDown,
            "move-half-page-up" => Self::MoveHalfPageUp,
            "move-half-page-down" => Self::MoveHalfPageDown,
            "move-first-line" => Self::MoveFirstLine,
            "move-last-line" => Self::MoveLastLine,
            "scroll-line-up" => Self::ScrollLineUp,
            "scroll-line-down" => Self::ScrollLineDown,
            "scroll-page-up" => Self::ScrollPageUp,
            "scroll-page-down" => Self::ScrollPageDown,
            "scroll-first-col" => Self::ScrollFirstCol,
            "scroll-left" => Self::ScrollLeft,
            "scroll-right" => Self::ScrollRight,
            "search" => Self::Search,
            "search-back" => Self::SearchBack,
            "find-next" => Self::FindNext,
            "find-prev" => Self::FindPrev,
            "options" => Self::Options,
            "edit" => Self::Edit,
            "prompt" => Self::Prompt,
            "screen-redraw" => Self::ScreenRedraw,
            "stop-loading" => Self::StopLoading,
            "show-version" => Self::ShowVersion,
            "suspend" => Self::Suspend,
            "none" => Self::None,
            "toggle-sort-order" | ":toggle sort-order" => Self::ToggleSortOrder,
            "toggle-sort-field" | ":toggle sort-field" => Self::ToggleSortField,
            ":goto head" | "goto-head" => Self::GotoHead,
            ":toggle diff-context -1" | "diff-context-down" => Self::ToggleDiffContext(-1),
            ":toggle diff-context +1"
            | "diff-context-up"
            | "toggle-diff-context"
            | ":toggle diff-context" => Self::ToggleDiffContext(1),
            other => {
                // If it starts with command flags or shell / prompt prefixes
                if other.starts_with('!')
                    || other.starts_with('?')
                    || other.starts_with('@')
                    || other.starts_with(':')
                {
                    let mut flags = RunFlags::empty();
                    let mut cmd = s;
                    loop {
                        // `:` (internal prompt command) and `!` (external shell
                        // command) are terminal sigils: they select the command
                        // kind, and everything after them is the command text.
                        let (flag, terminal) = match cmd.chars().next() {
                            Some(':') => (RunFlags::INTERNAL, true),
                            Some('!') => (RunFlags::empty(), true),
                            Some('@') => (RunFlags::SILENT, false),
                            Some('?') => (RunFlags::CONFIRM, false),
                            Some('<') => (RunFlags::EXIT, false),
                            Some('+') => (RunFlags::ECHO, false),
                            Some('>') => (RunFlags::QUICK, false),
                            _ => break,
                        };
                        flags |= flag;
                        cmd = &cmd[1..];
                        if terminal {
                            break;
                        }
                    }
                    Self::Run(Arc::new(RunCommand {
                        flags,
                        command: cmd.trim().to_string(),
                    }))
                } else {
                    return Err(ParseKeyError::UnknownAction(s.to_string()));
                }
            }
        };

        Ok(action)
    }
}

/// View or context scope for keybindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeymapScope {
    /// Generic fallback keybindings shared across all views.
    Generic,
    /// Commit log / main view scope.
    Main,
    /// Commit diff view scope.
    Diff,
    /// Working tree status view scope.
    Status,
    /// Staging view scope.
    Stage,
    /// Directory tree view scope.
    Tree,
    /// Raw file blob view scope.
    Blob,
    /// File blame annotations view scope.
    Blame,
    /// Active search prompt scope.
    Search,
    /// Arbitrary text pager view scope.
    Pager,
    /// Rich revision log view scope.
    Log,
    /// Git reflog view scope.
    Reflog,
    /// Branches, tags, and remotes references view scope.
    Refs,
    /// Keybinding help view scope.
    Help,
    /// Git stash stack view scope.
    Stash,
    /// Text search grep view scope.
    Grep,
}

impl KeymapScope {
    /// Parses a keymap scope name string (case-insensitive).
    pub fn parse(s: &str) -> Result<Self, ParseKeyError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "generic" => Ok(Self::Generic),
            "main" => Ok(Self::Main),
            "diff" => Ok(Self::Diff),
            "status" => Ok(Self::Status),
            "stage" => Ok(Self::Stage),
            "tree" => Ok(Self::Tree),
            "blob" => Ok(Self::Blob),
            "blame" => Ok(Self::Blame),
            "search" => Ok(Self::Search),
            "pager" => Ok(Self::Pager),
            "log" => Ok(Self::Log),
            "reflog" => Ok(Self::Reflog),
            "refs" => Ok(Self::Refs),
            "help" => Ok(Self::Help),
            "stash" => Ok(Self::Stash),
            "grep" => Ok(Self::Grep),
            _ => Err(ParseKeyError::UnknownScope(s.to_string())),
        }
    }
}

impl From<ViewKind> for KeymapScope {
    fn from(kind: ViewKind) -> Self {
        match kind {
            ViewKind::Main => Self::Main,
            ViewKind::Diff => Self::Diff,
            ViewKind::Status => Self::Status,
            ViewKind::Tree => Self::Tree,
            ViewKind::Blob => Self::Blob,
            ViewKind::Blame => Self::Blame,
            ViewKind::Help => Self::Help,
            ViewKind::Refs => Self::Refs,
            ViewKind::Stash => Self::Stash,
            ViewKind::Grep => Self::Grep,
            ViewKind::Reflog => Self::Reflog,
            ViewKind::Log => Self::Log,
            ViewKind::Pager => Self::Pager,
        }
    }
}

/// Internal trie node lookup outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
enum NodeLookupResult {
    /// Exact match with no longer extensions.
    Exact(Action),
    /// Current sequence matches an action, but has longer extensions.
    Ambiguous(Action),
    /// Sequence is an incomplete prefix with no action yet.
    Prefix,
    /// Path diverges; sequence not found.
    NotFound,
}

/// Prefix tree storing multi-key sequence bindings.
#[derive(Clone, Debug, Default)]
struct KeymapTrie {
    action: Option<Action>,
    children: IndexMap<Key, KeymapTrie>,
}

impl KeymapTrie {
    fn insert(&mut self, keys: &[Key], action: Action) {
        if keys.is_empty() {
            self.action = Some(action);
            return;
        }
        self.children
            .entry(keys[0])
            .or_default()
            .insert(&keys[1..], action);
    }

    fn lookup(&self, keys: &[Key]) -> NodeLookupResult {
        if keys.is_empty() {
            if let Some(ref action) = self.action {
                if self.children.is_empty() {
                    NodeLookupResult::Exact(action.clone())
                } else {
                    NodeLookupResult::Ambiguous(action.clone())
                }
            } else if !self.children.is_empty() {
                NodeLookupResult::Prefix
            } else {
                NodeLookupResult::NotFound
            }
        } else if let Some(child) = self.children.get(&keys[0]) {
            child.lookup(&keys[1..])
        } else {
            NodeLookupResult::NotFound
        }
    }

    fn find_action_key(&self, target: &Action, current_path: &mut Vec<Key>) -> Option<Vec<Key>> {
        if self.action.as_ref() == Some(target) {
            return Some(current_path.clone());
        }
        for (key, child) in &self.children {
            current_path.push(*key);
            if let Some(res) = child.find_action_key(target, current_path) {
                return Some(res);
            }
            current_path.pop();
        }
        None
    }
}

/// Result of looking up a key sequence in the keymap engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeymapLookupResult {
    /// Complete exact match found.
    Match(Action),
    /// Sequence is an incomplete prefix of one or more bindings.
    Prefix,
    /// Exact match exists, but longer multi-key extensions also exist.
    Ambiguous(Action),
    /// No binding matches this sequence.
    NoMatch,
}

/// Keymap engine managing view-scoped and generic keymaps.
#[derive(Clone, Debug)]
pub struct KeymapEngine {
    tables: HashMap<KeymapScope, KeymapTrie>,
    reverse: HashMap<(KeymapScope, Action), KeySequence>,
}

impl Default for KeymapEngine {
    fn default() -> Self {
        Self::default_tig()
    }
}

impl KeymapEngine {
    /// Creates an empty keymap engine.
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    /// Binds a key sequence to an action in a given scope.
    #[allow(clippy::needless_pass_by_value)]
    pub fn bind(&mut self, scope: KeymapScope, sequence: KeySequence, action: Action) {
        self.reverse
            .retain(|(s, _), seq| !(*s == scope && *seq == sequence));
        if action != Action::None {
            self.reverse
                .entry((scope, action.clone()))
                .or_insert_with(|| sequence.clone());
        }
        self.tables
            .entry(scope)
            .or_default()
            .insert(sequence.as_slice(), action);
    }

    /// Binds using string representations, e.g. `bind_str("generic", "gg", "move-first-line")`.
    pub fn bind_str(
        &mut self,
        scope_str: &str,
        keys_str: &str,
        action_str: &str,
    ) -> Result<(), ParseKeyError> {
        let scope = KeymapScope::parse(scope_str)?;
        let seq = KeySequence::parse(keys_str)?;
        let action = Action::parse(action_str)?;
        self.bind(scope, seq, action);
        Ok(())
    }

    /// Applies custom keybindings parsed from `config.toml`.
    ///
    /// Bindings are applied on a best-effort basis in declaration order: a
    /// malformed entry is skipped and described in the returned warnings rather
    /// than silently discarded.
    #[must_use]
    pub fn apply_config(
        &mut self,
        keybindings: &indexmap::IndexMap<String, String>,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        for (key_spec, action_str) in keybindings {
            let Some((scope, key)) = key_spec.split_once('.') else {
                warnings.push(format!(
                    "keybinding \"{key_spec}\": expected \"<scope>.<keys>\", for example \"generic.gg\""
                ));
                continue;
            };
            if let Err(err) = self.bind_str(scope, key, action_str) {
                warnings.push(format!("keybinding \"{key_spec} = {action_str}\": {err}"));
            }
        }
        warnings
    }

    /// Looks up a key sequence in the given scope, with fallback to `Generic` (unless `Search`).
    pub fn lookup(&self, scope: KeymapScope, keys: &[Key]) -> KeymapLookupResult {
        if keys.is_empty() {
            return KeymapLookupResult::NoMatch;
        }

        // 1. Check scope-specific table
        if let Some(table) = self.tables.get(&scope) {
            match table.lookup(keys) {
                NodeLookupResult::Exact(action) => {
                    let generic_has_children =
                        if scope != KeymapScope::Generic && scope != KeymapScope::Search {
                            self.tables.get(&KeymapScope::Generic).is_some_and(|gt| {
                                matches!(
                                    gt.lookup(keys),
                                    NodeLookupResult::Prefix | NodeLookupResult::Ambiguous(_)
                                )
                            })
                        } else {
                            false
                        };

                    if action == Action::None {
                        if generic_has_children {
                            return KeymapLookupResult::Prefix;
                        }
                        return KeymapLookupResult::NoMatch;
                    }
                    if generic_has_children {
                        return KeymapLookupResult::Ambiguous(action);
                    }
                    return KeymapLookupResult::Match(action);
                }
                NodeLookupResult::Ambiguous(action) => {
                    if action == Action::None {
                        return KeymapLookupResult::Prefix;
                    }
                    return KeymapLookupResult::Ambiguous(action);
                }
                NodeLookupResult::Prefix => {
                    return KeymapLookupResult::Prefix;
                }
                NodeLookupResult::NotFound => {}
            }
        }

        // 2. Search scope never inherits generic keys
        if scope == KeymapScope::Search {
            return KeymapLookupResult::NoMatch;
        }

        // 3. Fallback to Generic keymap
        if let Some(generic_table) = self.tables.get(&KeymapScope::Generic) {
            match generic_table.lookup(keys) {
                NodeLookupResult::Exact(action) => {
                    if action == Action::None {
                        return KeymapLookupResult::NoMatch;
                    }
                    return KeymapLookupResult::Match(action);
                }
                NodeLookupResult::Ambiguous(action) => {
                    if action == Action::None {
                        return KeymapLookupResult::Prefix;
                    }
                    return KeymapLookupResult::Ambiguous(action);
                }
                NodeLookupResult::Prefix => {
                    return KeymapLookupResult::Prefix;
                }
                NodeLookupResult::NotFound => {
                    return KeymapLookupResult::NoMatch;
                }
            }
        }

        KeymapLookupResult::NoMatch
    }

    /// Finds the primary key combination bound to `action` in `scope` (or `Generic`).
    pub fn get_key_for_action(&self, scope: KeymapScope, action: &Action) -> Option<KeySequence> {
        if let Some(seq) = self.reverse.get(&(scope, action.clone())) {
            return Some(seq.clone());
        }
        let mut path = Vec::new();
        if let Some(table) = self.tables.get(&scope)
            && let Some(keys) = table.find_action_key(action, &mut path)
        {
            return Some(KeySequence::new(keys));
        }
        if scope != KeymapScope::Search {
            if let Some(seq) = self.reverse.get(&(KeymapScope::Generic, action.clone())) {
                return Some(seq.clone());
            }
            path.clear();
            if let Some(table) = self.tables.get(&KeymapScope::Generic)
                && let Some(keys) = table.find_action_key(action, &mut path)
            {
                return Some(KeySequence::new(keys));
            }
        }
        None
    }
}

/// Declarative helper macro for batch-binding single-key actions into a [`KeymapEngine`].
macro_rules! bind_keys {
    ($engine:expr, $scope:expr, [ $( $key:expr => $action:expr ),* $(,)? ]) => {
        $(
            $engine.bind($scope, KeySequence::single(Key::from($key)), $action);
        )*
    };
}

impl KeymapEngine {
    /// Initializes a keymap engine with complete default Tig keybindings.
    pub fn default_tig() -> Self {
        let mut engine = Self::new();

        // --------------------------------------------------------------------
        // Generic View Switching, Manipulation, Navigation, and Toggles
        // --------------------------------------------------------------------
        bind_keys!(engine, KeymapScope::Generic, [
            // View Switching
            'm' => Action::OpenView(ViewKind::Main),
            'd' => Action::OpenView(ViewKind::Diff),
            'l' => Action::OpenView(ViewKind::Log),
            'L' => Action::OpenView(ViewKind::Reflog),
            't' => Action::OpenView(ViewKind::Tree),
            'f' => Action::OpenView(ViewKind::Blob),
            'b' => Action::OpenView(ViewKind::Blame),
            'r' => Action::OpenView(ViewKind::Refs),
            'p' => Action::OpenView(ViewKind::Pager),
            'h' => Action::OpenView(ViewKind::Help),
            's' => Action::OpenView(ViewKind::Status),
            'S' => Action::OpenView(ViewKind::Status),
            'c' => Action::ViewStage,
            'y' => Action::OpenView(ViewKind::Stash),
            'g' => Action::OpenView(ViewKind::Grep),

            // View Manipulation
            KeyCode::Enter => Action::Enter,
            '<' => Action::Back,
            KeyCode::Backspace => Action::Back,
            ',' => Action::Parent,
            Key::ctrl('n') => Action::Next,
            'J' => Action::Next,
            Key::ctrl('p') => Action::Previous,
            'K' => Action::Previous,
            KeyCode::Tab => Action::ViewNext,
            'R' => Action::Refresh,
            KeyCode::F(5) => Action::Refresh,
            'O' => Action::Maximize,
            'q' => Action::ViewClose,
            KeyCode::Esc => Action::ViewClose,
            'Q' => Action::Quit,
            Key::ctrl('c') => Action::Quit,

            // Cursor Navigation
            'j' => Action::MoveDown,
            KeyCode::Down => Action::MoveDown,
            'k' => Action::MoveUp,
            KeyCode::Up => Action::MoveUp,
            Key::ctrl('d') => Action::MoveHalfPageDown,
            Key::ctrl('u') => Action::MoveHalfPageUp,
            KeyCode::PageDown => Action::MovePageDown,
            ' ' => Action::MovePageDown,
            Key::ctrl('f') => Action::MovePageDown,
            KeyCode::PageUp => Action::MovePageUp,
            '-' => Action::MovePageUp,
            Key::ctrl('b') => Action::MovePageUp,
            KeyCode::Home => Action::MoveFirstLine,
            KeyCode::End => Action::MoveLastLine,

            // Scrolling
            '|' => Action::ScrollFirstCol,
            KeyCode::Left => Action::ScrollLeft,
            KeyCode::Right => Action::ScrollRight,
            KeyCode::Insert => Action::ScrollLineUp,
            Key::ctrl('y') => Action::ScrollLineUp,
            KeyCode::Delete => Action::ScrollLineDown,
            Key::ctrl('e') => Action::ScrollLineDown,

            // Searching
            '/' => Action::Search,
            '?' => Action::SearchBack,
            'n' => Action::FindNext,
            'N' => Action::FindPrev,

            // Options and Toggles
            'o' => Action::Options,
            'I' => Action::ToggleSortOrder,
            'i' => Action::ToggleSortField,
            '#' => Action::ToggleOption(crate::options::OptionId::LineNumber),
            '.' => Action::ToggleOption(crate::options::OptionId::LineNumber),
            'D' => Action::ToggleOption(crate::options::OptionId::Date),
            'A' => Action::ToggleOption(crate::options::OptionId::Author),
            'T' => Action::ToggleOption(crate::options::OptionId::Committer),
            '~' => Action::ToggleOption(crate::options::OptionId::LineGraphics),
            'F' => Action::ToggleOption(crate::options::OptionId::FileName),
            'W' => Action::ToggleOption(crate::options::OptionId::IgnoreSpace),
            'w' => Action::ToggleOption(crate::options::OptionId::WordDiff),
            'X' => Action::ToggleOption(crate::options::OptionId::Id),
            '$' => Action::ToggleOption(crate::options::OptionId::CommitTitleOverflow),
            '%' => Action::ToggleOption(crate::options::OptionId::FileFilter),
            '^' => Action::ToggleOption(crate::options::OptionId::RevFilter),
            '*' => Action::ToggleOption(crate::options::OptionId::FileSize),
            '&' => Action::ToggleOption(crate::options::OptionId::MainSpotlight),

            // Misc
            'e' => Action::Edit,
            ':' => Action::Prompt,
            Key::ctrl('l') => Action::ScreenRedraw,
            Key::ctrl('z') => Action::Suspend,
            'z' => Action::StopLoading,
            'v' => Action::ShowVersion,
        ]);

        // --------------------------------------------------------------------
        // Main View Specific
        // --------------------------------------------------------------------
        bind_keys!(engine, KeymapScope::Main, [
            '*' => Action::ToggleOption(crate::options::OptionId::MainSpotlight),
        ]);

        // --------------------------------------------------------------------
        // Status View Specific
        // --------------------------------------------------------------------
        bind_keys!(engine, KeymapScope::Status, [
            'u' => Action::StatusUpdate,
            '!' => Action::StatusRevert,
            'M' => Action::StatusMerge,
        ]);

        // --------------------------------------------------------------------
        // Stage View Specific
        // --------------------------------------------------------------------
        bind_keys!(engine, KeymapScope::Stage, [
            'u' => Action::StatusUpdate,
            '1' => Action::StageUpdateLine,
            '2' => Action::StageUpdatePart,
            '!' => Action::StatusRevert,
            '\\' => Action::StageSplitChunk,
            '@' => Action::NextHunk,
            ')' => Action::NextHunk,
            '(' => Action::PrevHunk,
            '}' => Action::NextFile,
            '{' => Action::PrevFile,
            '[' => Action::ToggleDiffContext(-1),
            ']' => Action::ToggleDiffContext(1),
            '+' => Action::ExpandHunkContext,
            '=' => Action::ExpandHunkContext,
            '_' => Action::ShrinkHunkContext,
            '0' => Action::ScrollFirstCol,
            'v' => Action::ToggleOption(crate::options::OptionId::DiffLayout),
            'M' => Action::ToggleOption(crate::options::OptionId::ColorMoved),
            'S' => Action::ToggleOption(crate::options::OptionId::SyntaxHighlighting),
            'T' => Action::ToggleOption(crate::options::OptionId::SyntaxTheme),
            'i' => Action::ToggleDiffFileDetails,
            'y' => Action::YankDiffText,
            'z' => Action::None,
        ]);

        // --------------------------------------------------------------------
        // Diff View Specific
        // --------------------------------------------------------------------
        bind_keys!(engine, KeymapScope::Diff, [
            '@' => Action::NextHunk,
            ')' => Action::NextHunk,
            '(' => Action::PrevHunk,
            ']' => Action::ToggleDiffContext(1),
            '[' => Action::ToggleDiffContext(-1),
            '+' => Action::ExpandHunkContext,
            '=' => Action::ExpandHunkContext,
            '_' => Action::ShrinkHunkContext,
            '0' => Action::ScrollFirstCol,
            '}' => Action::NextFile,
            '{' => Action::PrevFile,
            'v' => Action::ToggleOption(crate::options::OptionId::DiffLayout),
            'M' => Action::ToggleOption(crate::options::OptionId::ColorMoved),
            'S' => Action::ToggleOption(crate::options::OptionId::SyntaxHighlighting),
            'T' => Action::ToggleOption(crate::options::OptionId::SyntaxTheme),
            'u' => Action::StatusUpdate,
            '1' => Action::StageUpdateLine,
            '!' => Action::StatusRevert,
            '.' => Action::Next,
            'i' => Action::ToggleDiffFileDetails,
            'y' => Action::YankDiffText,
            'z' => Action::None,
        ]);

        for scope in [KeymapScope::Diff, KeymapScope::Stage] {
            let _ = engine.bind_str(
                match scope {
                    KeymapScope::Diff => "diff",
                    _ => "stage",
                },
                "za",
                "toggle-file-fold",
            );
            let _ = engine.bind_str(
                match scope {
                    KeymapScope::Diff => "diff",
                    _ => "stage",
                },
                "zM",
                "fold-all",
            );
            let _ = engine.bind_str(
                match scope {
                    KeymapScope::Diff => "diff",
                    _ => "stage",
                },
                "zR",
                "unfold-all",
            );
            let _ = engine.bind_str(
                match scope {
                    KeymapScope::Diff => "diff",
                    _ => "stage",
                },
                "zW",
                "toggle-wrap-lines",
            );
            let _ = engine.bind_str(
                match scope {
                    KeymapScope::Diff => "diff",
                    _ => "stage",
                },
                "zz",
                "stop-loading",
            );
            let _ = engine.bind_str(
                match scope {
                    KeymapScope::Diff => "diff",
                    _ => "stage",
                },
                "zj",
                "next-hunk",
            );
            let _ = engine.bind_str(
                match scope {
                    KeymapScope::Diff => "diff",
                    _ => "stage",
                },
                "zk",
                "prev-hunk",
            );
            let _ = engine.bind_str(
                match scope {
                    KeymapScope::Diff => "diff",
                    _ => "stage",
                },
                "zn",
                "next-file",
            );
            let _ = engine.bind_str(
                match scope {
                    KeymapScope::Diff => "diff",
                    _ => "stage",
                },
                "zp",
                "prev-file",
            );
        }

        // --------------------------------------------------------------------
        // Blob & Blame View Specific
        // --------------------------------------------------------------------
        bind_keys!(engine, KeymapScope::Blob, [
            'S' => Action::ToggleOption(crate::options::OptionId::SyntaxHighlighting),
            'T' => Action::ToggleOption(crate::options::OptionId::SyntaxTheme),
        ]);
        bind_keys!(engine, KeymapScope::Blame, [
            'S' => Action::ToggleOption(crate::options::OptionId::SyntaxHighlighting),
            'T' => Action::ToggleOption(crate::options::OptionId::SyntaxTheme),
        ]);

        // --------------------------------------------------------------------
        // Main View Specific
        // --------------------------------------------------------------------
        bind_keys!(engine, KeymapScope::Main, [
            'H' => Action::GotoHead,
            'G' => Action::ToggleOption(crate::options::OptionId::CommitTitleGraph),
            'F' => Action::ToggleOption(crate::options::OptionId::CommitTitleRefs),
        ]);

        // --------------------------------------------------------------------
        // Search Keymap (does not inherit generic)
        // --------------------------------------------------------------------
        bind_keys!(engine, KeymapScope::Search, [
            KeyCode::Down => Action::FindNext,
            Key::ctrl('n') => Action::FindNext,
            Key::ctrl('j') => Action::FindNext,
            KeyCode::Up => Action::FindPrev,
            Key::ctrl('p') => Action::FindPrev,
            Key::ctrl('k') => Action::FindPrev,
            Key::ctrl('c') => Action::ViewClose,
            KeyCode::Esc => Action::ViewClose,
        ]);

        engine
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_parsing_and_formatting() {
        let enter = Key::parse("<Enter>").unwrap();
        assert_eq!(enter.code, KeyCode::Enter);
        assert_eq!(enter.to_string(), "<Enter>");

        let ctrl_n = Key::parse("<Ctrl-n>").unwrap();
        assert_eq!(ctrl_n.code, KeyCode::Char('n'));
        assert!(ctrl_n.modifiers.contains(KeyModifiers::CONTROL));
        assert_eq!(ctrl_n.to_string(), "<Ctrl-N>");

        let c_d = Key::parse("<C-d>").unwrap();
        assert_eq!(c_d.code, KeyCode::Char('d'));
        assert!(c_d.modifiers.contains(KeyModifiers::CONTROL));

        let esc = Key::parse("<Esc>").unwrap();
        assert_eq!(esc.code, KeyCode::Esc);
        assert_eq!(esc.to_string(), "<Esc>");

        let down = Key::parse("<Down>").unwrap();
        assert_eq!(down.code, KeyCode::Down);
        assert_eq!(down.to_string(), "<Down>");

        let f5 = Key::parse("<F5>").unwrap();
        assert_eq!(f5.code, KeyCode::F(5));
        assert_eq!(f5.to_string(), "<F5>");

        let space = Key::parse("<Space>").unwrap();
        assert_eq!(space.code, KeyCode::Char(' '));
        assert_eq!(space.to_string(), "<Space>");

        let lt = Key::parse("<Lt>").unwrap();
        assert_eq!(lt.code, KeyCode::Char('<'));
        assert_eq!(lt.to_string(), "<Lt>");

        let char_q = Key::parse("q").unwrap();
        assert_eq!(char_q.code, KeyCode::Char('q'));
        assert_eq!(char_q.to_string(), "q");
    }

    #[test]
    fn test_key_sequence_parsing_and_formatting() {
        let seq = KeySequence::parse("qa").unwrap();
        assert_eq!(seq.len(), 2);
        assert_eq!(seq.0[0], Key::from_char('q'));
        assert_eq!(seq.0[1], Key::from_char('a'));
        assert_eq!(seq.to_string(), "qa");

        let gg = KeySequence::parse("gg").unwrap();
        assert_eq!(gg.len(), 2);
        assert_eq!(gg.0[0], Key::from_char('g'));
        assert_eq!(gg.0[1], Key::from_char('g'));
        assert_eq!(gg.to_string(), "gg");

        let bracket_seq = KeySequence::parse("<Esc>q").unwrap();
        assert_eq!(bracket_seq.len(), 2);
        assert_eq!(bracket_seq.0[0], Key::from_code(KeyCode::Esc));
        assert_eq!(bracket_seq.0[1], Key::from_char('q'));
        assert_eq!(bracket_seq.to_string(), "<Esc>q");
    }

    #[test]
    fn test_action_parsing() {
        use crate::options::OptionId;
        assert_eq!(
            Action::parse("view-main").unwrap(),
            Action::OpenView(ViewKind::Main)
        );
        assert_eq!(Action::parse("quit").unwrap(), Action::Quit);
        assert_eq!(
            Action::parse("status-update").unwrap(),
            Action::StatusUpdate
        );
        assert_eq!(
            Action::parse(":toggle line-number").unwrap(),
            Action::ToggleOption(OptionId::LineNumber)
        );
        assert_eq!(
            Action::parse("toggle-lineno").unwrap(),
            Action::ToggleOption(OptionId::LineNumber)
        );
        assert_eq!(
            Action::parse(":toggle diff-context +1").unwrap(),
            Action::ToggleDiffContext(1)
        );
        assert_eq!(
            Action::parse(":toggle diff-context -1").unwrap(),
            Action::ToggleDiffContext(-1)
        );
        assert_eq!(
            Action::parse(":toggle diff-layout").unwrap(),
            Action::ToggleOption(OptionId::DiffLayout)
        );
        assert_eq!(
            Action::parse("toggle-diff-layout").unwrap(),
            Action::ToggleOption(OptionId::DiffLayout)
        );
        assert_eq!(
            Action::parse(":toggle layout").unwrap(),
            Action::ToggleOption(OptionId::DiffLayout)
        );
        assert_eq!(
            Action::parse(":toggle side-to-side").unwrap(),
            Action::ToggleOption(OptionId::DiffLayout)
        );
        assert_eq!(
            Action::parse(":toggle single").unwrap(),
            Action::ToggleOption(OptionId::DiffLayout)
        );
        assert_eq!(
            Action::parse(":toggle diff-presentation").unwrap(),
            Action::ToggleOption(OptionId::DiffPresentation)
        );
        assert_eq!(
            Action::parse("toggle-diff-presentation").unwrap(),
            Action::ToggleOption(OptionId::DiffPresentation)
        );
        assert_eq!(Action::parse("none").unwrap(), Action::None);

        let run = Action::parse("?git checkout -b %(prompt)").unwrap();
        if let Action::Run(cmd) = run {
            assert!(cmd.flags.contains(RunFlags::CONFIRM));
            assert_eq!(cmd.command, "git checkout -b %(prompt)");
        } else {
            panic!("Expected Action::Run");
        }
    }

    #[test]
    fn test_multi_key_sequence_trie_matching() {
        let mut engine = KeymapEngine::new();
        engine.bind(
            KeymapScope::Generic,
            KeySequence::parse("gg").unwrap(),
            Action::MoveFirstLine,
        );
        engine.bind(
            KeymapScope::Generic,
            KeySequence::parse("qa").unwrap(),
            Action::Quit,
        );
        engine.bind(
            KeymapScope::Generic,
            KeySequence::parse("q").unwrap(),
            Action::ViewClose,
        );

        // Lookup 'g' -> Prefix
        let g = vec![Key::from_char('g')];
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &g),
            KeymapLookupResult::Prefix
        );

        // Lookup 'gg' -> Match(MoveFirstLine)
        let gg = vec![Key::from_char('g'), Key::from_char('g')];
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &gg),
            KeymapLookupResult::Match(Action::MoveFirstLine)
        );

        // Lookup 'gx' -> NoMatch
        let gx = vec![Key::from_char('g'), Key::from_char('x')];
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &gx),
            KeymapLookupResult::NoMatch
        );

        // Lookup 'q' -> Ambiguous(ViewClose) because 'qa' exists!
        let q = vec![Key::from_char('q')];
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &q),
            KeymapLookupResult::Ambiguous(Action::ViewClose)
        );

        // Lookup 'qa' -> Match(Quit)
        let qa = vec![Key::from_char('q'), Key::from_char('a')];
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &qa),
            KeymapLookupResult::Match(Action::Quit)
        );
    }

    #[test]
    fn test_scope_override_and_none_masking() {
        let mut engine = KeymapEngine::new();
        // Generic binding: 'q' -> Quit
        engine.bind(
            KeymapScope::Generic,
            KeySequence::parse("q").unwrap(),
            Action::Quit,
        );

        // In Main view, mask 'q' to None
        engine.bind(
            KeymapScope::Main,
            KeySequence::parse("q").unwrap(),
            Action::None,
        );

        // In Diff view, override 'q' with ViewClose
        engine.bind(
            KeymapScope::Diff,
            KeySequence::parse("q").unwrap(),
            Action::ViewClose,
        );

        // Main view: 'q' is masked to NoMatch
        let q = vec![Key::from_char('q')];
        assert_eq!(
            engine.lookup(KeymapScope::Main, &q),
            KeymapLookupResult::NoMatch
        );

        // Diff view: 'q' resolves to ViewClose
        assert_eq!(
            engine.lookup(KeymapScope::Diff, &q),
            KeymapLookupResult::Match(Action::ViewClose)
        );

        // Status view (no override): falls back to Generic -> Quit
        assert_eq!(
            engine.lookup(KeymapScope::Status, &q),
            KeymapLookupResult::Match(Action::Quit)
        );
    }

    #[test]
    fn test_default_tig_bindings() {
        use crate::options::OptionId;
        let engine = KeymapEngine::default_tig();

        let j = vec![Key::from_char('j')];
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &j),
            KeymapLookupResult::Match(Action::MoveDown)
        );

        let k = vec![Key::from_char('k')];
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &k),
            KeymapLookupResult::Match(Action::MoveUp)
        );

        let u_diff = vec![Key::from_char('u')];
        assert_eq!(
            engine.lookup(KeymapScope::Diff, &u_diff),
            KeymapLookupResult::Match(Action::StatusUpdate)
        );

        let h_main = vec![Key::from_char('H')];
        assert_eq!(
            engine.lookup(KeymapScope::Main, &h_main),
            KeymapLookupResult::Match(Action::GotoHead)
        );

        // Parity test: 'd' opens ViewDiff, 'l' opens ViewLog, 'g' opens ViewGrep, 'm' opens ViewMain
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &[Key::from_char('d')]),
            KeymapLookupResult::Match(Action::OpenView(ViewKind::Diff))
        );
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &[Key::from_char('l')]),
            KeymapLookupResult::Match(Action::OpenView(ViewKind::Log))
        );
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &[Key::from_char('g')]),
            KeymapLookupResult::Match(Action::OpenView(ViewKind::Grep))
        );
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &[Key::from_char('m')]),
            KeymapLookupResult::Match(Action::OpenView(ViewKind::Main))
        );

        // 'F' in generic toggles file name; in main it toggles commit title refs
        assert_eq!(
            engine.lookup(KeymapScope::Generic, &[Key::from_char('F')]),
            KeymapLookupResult::Match(Action::ToggleOption(OptionId::FileName))
        );
        assert_eq!(
            engine.lookup(KeymapScope::Main, &[Key::from_char('F')]),
            KeymapLookupResult::Match(Action::ToggleOption(OptionId::CommitTitleRefs))
        );
        assert_eq!(
            engine.lookup(KeymapScope::Main, &[Key::from_char('G')]),
            KeymapLookupResult::Match(Action::ToggleOption(OptionId::CommitTitleGraph))
        );
    }

    #[test]
    fn test_scope_parsing_and_action_as_str() {
        for scope in [
            "generic", "main", "diff", "status", "stage", "tree", "blob", "blame", "search",
            "pager", "log", "reflog", "refs", "help", "stash", "grep",
        ] {
            assert!(KeymapScope::parse(scope).is_ok());
        }
        assert!(KeymapScope::parse("invalid_scope_name").is_err());

        // Action name
        assert_eq!(Action::MoveDown.name(), "move-down");
        assert_eq!(Action::MoveUp.name(), "move-up");
        assert_eq!(Action::MoveHalfPageDown.name(), "move-half-page-down");
        assert_eq!(Action::MoveHalfPageUp.name(), "move-half-page-up");
        assert_eq!(Action::ScrollLineDown.name(), "scroll-line-down");
        assert_eq!(Action::ScrollLineUp.name(), "scroll-line-up");
        assert_eq!(Action::Search.name(), "search");
        assert_eq!(Action::SearchBack.name(), "search-back");
        assert_eq!(Action::FindNext.name(), "find-next");
        assert_eq!(Action::FindPrev.name(), "find-prev");
        assert_eq!(Action::Options.name(), "options");
        assert_eq!(Action::Edit.name(), "edit");
        assert_eq!(Action::Prompt.name(), "prompt");
        assert_eq!(Action::Quit.name(), "quit");
        assert_eq!(Action::None.name(), "none");

        // Action flags parsing
        let run_flags = Action::parse("@<+>!git commit").unwrap();
        if let Action::Run(cmd) = run_flags {
            assert!(cmd.flags.contains(RunFlags::SILENT));
            assert!(cmd.flags.contains(RunFlags::EXIT));
            assert!(cmd.flags.contains(RunFlags::ECHO));
            assert!(cmd.flags.contains(RunFlags::QUICK));
            assert_eq!(cmd.command, "git commit");
        } else {
            panic!("Expected Action::Run");
        }

        let internal_cmd = Action::parse(":echo hello").unwrap();
        if let Action::Run(cmd) = internal_cmd {
            assert!(cmd.flags.contains(RunFlags::INTERNAL));
            assert_eq!(cmd.command, "echo hello");
        } else {
            panic!("Expected Action::Run");
        }
    }

    #[test]
    fn test_bind_str_and_get_key_for_action() {
        let mut engine = KeymapEngine::new();
        engine.bind_str("main", "x", "view-diff").unwrap();
        assert_eq!(
            engine.lookup(KeymapScope::Main, &[Key::from_char('x')]),
            KeymapLookupResult::Match(Action::OpenView(ViewKind::Diff))
        );

        // get_key_for_action
        let seq = engine
            .get_key_for_action(KeymapScope::Main, &Action::OpenView(ViewKind::Diff))
            .unwrap();
        assert_eq!(seq.to_string(), "x");

        // apply_config: valid entries bind, invalid entries are reported in declaration order
        let mut map = indexmap::IndexMap::new();
        map.insert("main.y".to_string(), "quit".to_string());
        assert!(engine.apply_config(&map).is_empty());
        assert_eq!(
            engine.lookup(KeymapScope::Main, &[Key::from_char('y')]),
            KeymapLookupResult::Match(Action::Quit)
        );

        map.insert("no-scope-separator".to_string(), "quit".to_string());
        map.insert("main.z".to_string(), "not-a-real-action".to_string());
        map.insert("bogus-scope.q".to_string(), "quit".to_string());
        let warnings = engine.apply_config(&map);
        assert_eq!(warnings.len(), 3, "got: {warnings:?}");
        // Preserves declaration order from IndexMap.
        assert!(warnings[0].contains("no-scope-separator"));
        assert!(warnings[1].contains("main.z"));
        assert!(warnings[2].contains("bogus-scope.q"));
        // The one valid entry in the same batch is still applied.
        assert_eq!(
            engine.lookup(KeymapScope::Main, &[Key::from_char('y')]),
            KeymapLookupResult::Match(Action::Quit)
        );
    }

    #[test]
    fn test_keymap_formatting_and_errors() {
        use crate::options::OptionId;
        // 1. KeySequence::is_empty
        assert!(KeySequence::default().is_empty());
        let seq = KeySequence::parse("a").unwrap();
        assert!(!seq.is_empty());

        // 2. ParseKeyError variants
        assert!(matches!(KeySequence::parse(""), Err(ParseKeyError::Empty)));
        assert!(matches!(
            KeySequence::parse("<unclosed"),
            Err(ParseKeyError::UnclosedAngleBracket(_))
        ));
        assert!(matches!(
            KeySequence::parse("<Fabc>"),
            Err(ParseKeyError::UnknownKeyName(_))
        ));
        assert!(matches!(
            KeySequence::parse("<NoSuchKey>"),
            Err(ParseKeyError::UnknownKeyName(_))
        ));

        // 3. Display formatting of various keys
        let test_keys = [
            (Key::new(KeyCode::Enter, KeyModifiers::NONE), "<Enter>"),
            (
                Key::new(KeyCode::Backspace, KeyModifiers::NONE),
                "<Backspace>",
            ),
            (Key::new(KeyCode::Tab, KeyModifiers::NONE), "<Tab>"),
            (Key::new(KeyCode::BackTab, KeyModifiers::NONE), "<BackTab>"),
            (Key::new(KeyCode::Esc, KeyModifiers::NONE), "<Esc>"),
            (Key::new(KeyCode::Left, KeyModifiers::NONE), "<Left>"),
            (Key::new(KeyCode::Right, KeyModifiers::NONE), "<Right>"),
            (Key::new(KeyCode::Up, KeyModifiers::NONE), "<Up>"),
            (Key::new(KeyCode::Down, KeyModifiers::NONE), "<Down>"),
            (Key::new(KeyCode::Home, KeyModifiers::NONE), "<Home>"),
            (Key::new(KeyCode::End, KeyModifiers::NONE), "<End>"),
            (Key::new(KeyCode::PageUp, KeyModifiers::NONE), "<PageUp>"),
            (
                Key::new(KeyCode::PageDown, KeyModifiers::NONE),
                "<PageDown>",
            ),
            (Key::new(KeyCode::Delete, KeyModifiers::NONE), "<Delete>"),
            (Key::new(KeyCode::Insert, KeyModifiers::NONE), "<Insert>"),
            (Key::new(KeyCode::F(5), KeyModifiers::NONE), "<F5>"),
            (Key::new(KeyCode::Char('#'), KeyModifiers::NONE), "<Hash>"),
            (Key::new(KeyCode::Char('a'), KeyModifiers::ALT), "<Alt-A>"),
            (
                Key::new(KeyCode::Char('b'), KeyModifiers::SHIFT),
                "<Shift-B>",
            ),
            (Key::new(KeyCode::Enter, KeyModifiers::ALT), "<Alt-Enter>"),
        ];
        for (k, expected) in test_keys {
            assert_eq!(k.to_string(), expected);
        }

        // 4. Action name() coverage
        let actions = [
            Action::OpenView(ViewKind::Main),
            Action::OpenView(ViewKind::Diff),
            Action::OpenView(ViewKind::Log),
            Action::OpenView(ViewKind::Reflog),
            Action::OpenView(ViewKind::Tree),
            Action::OpenView(ViewKind::Blob),
            Action::OpenView(ViewKind::Blame),
            Action::OpenView(ViewKind::Refs),
            Action::ViewStage,
            Action::OpenView(ViewKind::Status),
            Action::OpenView(ViewKind::Help),
            Action::OpenView(ViewKind::Pager),
            Action::OpenView(ViewKind::Stash),
            Action::OpenView(ViewKind::Grep),
            Action::Enter,
            Action::Back,
            Action::Next,
            Action::Previous,
            Action::Parent,
            Action::ViewNext,
            Action::Refresh,
            Action::Maximize,
            Action::ViewClose,
            Action::StatusUpdate,
            Action::StatusRevert,
            Action::StatusMerge,
            Action::StageUpdateLine,
            Action::StageUpdatePart,
            Action::StageSplitChunk,
            Action::NextHunk,
            Action::PrevHunk,
            Action::NextFile,
            Action::PrevFile,
            Action::MovePageUp,
            Action::MovePageDown,
            Action::MoveFirstLine,
            Action::MoveLastLine,
            Action::ScrollPageUp,
            Action::ScrollPageDown,
            Action::ScrollFirstCol,
            Action::ScrollLeft,
            Action::ScrollRight,
            Action::ToggleSortOrder,
            Action::ToggleSortField,
            Action::ToggleOption(OptionId::LineNumber),
            Action::ToggleOption(OptionId::Date),
            Action::ToggleOption(OptionId::Author),
            Action::ToggleOption(OptionId::Committer),
            Action::ToggleOption(OptionId::LineGraphics),
            Action::ToggleOption(OptionId::FileName),
            Action::ToggleOption(OptionId::IgnoreSpace),
            Action::ToggleOption(OptionId::WordDiff),
            Action::ToggleOption(OptionId::Id),
            Action::ToggleOption(OptionId::CommitTitleOverflow),
            Action::ToggleOption(OptionId::FileFilter),
            Action::ToggleOption(OptionId::RevFilter),
            Action::ToggleDiffContext(1),
            Action::ToggleOption(OptionId::DiffLayout),
            Action::ToggleOption(OptionId::DiffPresentation),
            Action::ToggleOption(OptionId::CommitTitleGraph),
            Action::ToggleOption(OptionId::CommitTitleRefs),
            Action::ToggleOption(OptionId::FileSize),
            Action::ToggleOption(OptionId::CommitOrder),
            Action::ToggleOption(OptionId::ShowChanges),
            Action::ToggleOption(OptionId::StatusShowUntrackedDirs),
            Action::ToggleOption(OptionId::VerticalSplit),
            Action::GotoHead,
            Action::ScreenRedraw,
            Action::StopLoading,
            Action::ShowVersion,
        ];
        for a in actions {
            assert!(!a.name().is_empty());
        }

        // 5. Deterministic lookup in get_key_for_action (Generic fallback & rebind fallback)
        let mut engine = KeymapEngine::default();
        let key_for_quit = engine
            .get_key_for_action(KeymapScope::Diff, &Action::Quit)
            .expect("should find Quit key");
        assert_eq!(key_for_quit.to_string(), "Q");

        let key_for_close = engine
            .get_key_for_action(KeymapScope::Diff, &Action::ViewClose)
            .expect("should find ViewClose key");
        assert_eq!(key_for_close.to_string(), "q");

        // Rebind 'Q' in Generic scope to something else; Quit should deterministically fall back to <Ctrl-c>
        engine.bind(
            KeymapScope::Generic,
            KeySequence::parse("Q").unwrap(),
            Action::Refresh,
        );
        let fallback_quit = engine
            .get_key_for_action(KeymapScope::Diff, &Action::Quit)
            .expect("should fall back to <Ctrl-C>");
        assert_eq!(fallback_quit.to_string(), "<Ctrl-C>");
    }

    #[test]
    fn test_key_helpers_and_from_conversions() {
        let k_char = Key::from('x');
        assert_eq!(k_char.code, KeyCode::Char('x'));
        assert_eq!(k_char.modifiers, KeyModifiers::NONE);

        let k_code = Key::from(KeyCode::Enter);
        assert_eq!(k_code.code, KeyCode::Enter);
        assert_eq!(k_code.modifiers, KeyModifiers::NONE);

        let k_ctrl = Key::ctrl('c');
        assert_eq!(k_ctrl.code, KeyCode::Char('c'));
        assert!(k_ctrl.modifiers.contains(KeyModifiers::CONTROL));

        let k_pair = Key::from((KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(k_pair.code, KeyCode::Left);
        assert!(k_pair.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn test_action_name_roundtrip_diff_context_and_toggles() {
        let up = Action::ToggleDiffContext(1);
        let down = Action::ToggleDiffContext(-1);
        assert_eq!(up.name(), "diff-context-up");
        assert_eq!(down.name(), "diff-context-down");
        assert_eq!(Action::parse(up.name()).unwrap(), up);
        assert_eq!(Action::parse(down.name()).unwrap(), down);

        for desc in crate::options::OPTIONS_REGISTRY {
            let toggle = Action::ToggleOption(desc.id);
            assert_eq!(Action::parse(toggle.name()).unwrap(), toggle);
        }
    }

    #[test]
    fn test_cross_scope_exact_vs_generic_prefix_disambiguation() {
        let mut engine = KeymapEngine::new();
        let move_first = Action::parse("move-first-line").unwrap();
        let view_grep = Action::parse("view-grep").unwrap();
        engine.bind(
            KeymapScope::Generic,
            KeySequence::parse("gg").unwrap(),
            move_first.clone(),
        );
        engine.bind(
            KeymapScope::Main,
            KeySequence::parse("g").unwrap(),
            view_grep.clone(),
        );

        assert_eq!(
            engine.lookup(KeymapScope::Main, &[Key::from('g')]),
            KeymapLookupResult::Ambiguous(view_grep)
        );
        assert_eq!(
            engine.lookup(KeymapScope::Main, &[Key::from('g'), Key::from('g')]),
            KeymapLookupResult::Match(move_first)
        );
    }
}
