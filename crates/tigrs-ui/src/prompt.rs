// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive prompt line state machine and command parser for tigrs.
//!
//! Handles command (`:`), search (`/`, `?`), interactive macro (`%(prompt)`),
//! and execution confirmation prompts with full cursor navigation, UTF-8 editing,
//! and persistent history navigation.

use crate::keymap::{Action, RunCommand};
use crate::options::MenuAction;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::Arc;
use tigrs_core::history::HistoryManager;
use unicode_width::UnicodeWidthStr;

/// The category of prompt currently active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// Command line prompt (`:`).
    Command,
    /// Forward search prompt (`/`).
    SearchForward,
    /// Backward search prompt (`?`).
    SearchBackward,
    /// User input prompt for `%(prompt)` macro expansion.
    InteractiveMacro {
        /// Optional prompt message shown to the user (e.g. `"Branch name: "`).
        label: String,
        /// The command template being expanded.
        template: String,
        /// Collected user inputs for each `%(prompt)` token in order.
        answers: Vec<String>,
        /// `RunCommand` descriptor if triggered by keybinding.
        run_command: Option<Arc<RunCommand>>,
    },
    /// Confirmation prompt (e.g. `"Run: git reset --hard? [y/N]"`).
    Confirm {
        /// Prompt question.
        message: String,
        /// Command to execute if confirmed.
        expanded_command: String,
        /// `RunCommand` descriptor.
        run_command: Arc<RunCommand>,
    },
    /// Interactive option toggle menu (`o`).
    OptionMenu(crate::options::OptionMenuState),
}

/// The result of processing a key event in prompt mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptResult {
    /// The key was consumed; prompt remains active.
    None,
    /// The user submitted the prompt input with Enter.
    Submit(String),
    /// The user cancelled the prompt with Esc or Ctrl-C.
    Cancel,
    /// The user responded to a confirmation prompt.
    Confirm(bool),
    /// Option menu entry activated and menu should close (`Enter` or direct hotkey).
    OptionSelected(MenuAction),
    /// Option modified in-place inside the interactive Options & Config Panel (`Space`, `[` / `]`, `r`, `R`); panel stays open.
    OptionChanged(MenuAction),
    /// User triggered config persistence from the Options & Config Panel (`p`, `Ctrl+S`, or `P` Save-As).
    SaveConfig {
        /// Optional custom target `.toml` path (`None` = default/active `config.toml`).
        path: Option<String>,
        /// True when saving only non-default / modified keys; false when saving all keys.
        minimal: bool,
    },
}

/// Parsed representation of a `:` command string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedCommand {
    /// Jump to 1-based line number (e.g. `:42`).
    LineNumber(usize),
    /// Jump to commit SHA, ref, or revision (e.g. `:2f12bcc`, `:HEAD~1`).
    Goto(String),
    /// Shell command execution (e.g. `:!git status`, `:!make`).
    Shell {
        /// Raw shell command string to execute.
        command: String,
        /// Whether the command runs interactively in the foreground terminal.
        foreground: bool,
    },
    /// Built-in Tig request action (e.g. `:quit`, `:toggle line-number`, `:view-diff`).
    Builtin(Action),
    /// Toggle option by name (e.g. `:toggle line-number`).
    Toggle(String),
    /// Set option by name and value (e.g. `:set mouse = true`).
    Set {
        /// Option name or alias.
        variable: String,
        /// Raw string value to assign.
        value: String,
    },
    /// Persist current runtime options to a TOML config file (`:save-config [path]`, `:wconfig [path]`).
    SaveConfig {
        /// Optional custom `.toml` file path (`None` = active/default `config.toml`).
        path: Option<String>,
        /// Whether to write only non-default keys (`true`) or all keys (`false`, via `:save-config!`).
        minimal: bool,
    },
    /// Reload configuration from a specified TOML file (`:source <path>`).
    SourceConfig(String),
    /// Search pattern (e.g. `:/pattern`, `:?pattern`).
    Search {
        /// Regular expression search query.
        query: String,
        /// True when searching upward (`?`), false when searching downward (`/`).
        backward: bool,
    },
    /// Status message echo (e.g. `:echo hello`).
    Echo(String),
    /// Grep search (e.g. `:grep <pattern>`).
    Grep(String),
    /// Empty command (user just pressed Enter at empty `:` prompt).
    Empty,
    /// Unrecognized command with error explanation.
    Unknown(String),
}

impl ParsedCommand {
    /// Parses a raw command line string entered at the `:` prompt.
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Self::Empty;
        }

        // 1. Line number jump: e.g. "42", ":42"
        let s = trimmed.strip_prefix(':').unwrap_or(trimmed);
        if let Ok(lineno) = s.parse::<usize>() {
            return Self::LineNumber(lineno);
        }

        // 2. Shell execution: e.g. "!git status" (foreground) or "+echo hi" (echo output)
        if let Some(cmd) = s.strip_prefix('!') {
            return Self::Shell {
                command: cmd.trim().to_string(),
                foreground: true,
            };
        }
        if let Some(cmd) = s.strip_prefix('+') {
            return Self::Shell {
                command: cmd.trim().to_string(),
                foreground: false,
            };
        }

        // 3. Search triggers: e.g. "/pattern", "?pattern"
        if let Some(pattern) = s.strip_prefix('/') {
            return Self::Search {
                query: pattern.to_string(),
                backward: false,
            };
        }
        if let Some(pattern) = s.strip_prefix('?') {
            return Self::Search {
                query: pattern.to_string(),
                backward: true,
            };
        }

        // 4. Echo command: e.g. "echo hello"
        if let Some(msg) = s.strip_prefix("echo ") {
            return Self::Echo(msg.to_string());
        }

        // 5. Goto command: e.g. "goto 2f12bcc" or "goto 42"
        if let Some(arg) = s.strip_prefix("goto ") {
            let arg = arg.trim();
            if let Ok(line) = arg.parse::<usize>() {
                return Self::LineNumber(line);
            }
            return Self::Goto(arg.to_string());
        }

        // 6. Common abbreviations
        if s == "q" || s == "quit" {
            return Self::Builtin(Action::Quit);
        }
        if s == "options" || s == "config" {
            return Self::Builtin(Action::Options);
        }
        if s == "version" {
            return Self::Echo(format!("tigrs {}", tigrs_core::APP_VERSION));
        }
        if matches!(
            s.to_ascii_lowercase().as_str(),
            "update-mode" | "update_mode" | "rw"
        ) {
            return Self::Set {
                variable: "read-only".to_string(),
                value: "false".to_string(),
            };
        }
        if matches!(
            s.to_ascii_lowercase().as_str(),
            "read-only" | "read_only" | "readonly" | "ro"
        ) {
            return Self::Set {
                variable: "read-only".to_string(),
                value: "true".to_string(),
            };
        }

        // 6a. SaveConfig and SourceConfig commands:
        //     `:save-config [path]`, `:wconfig [path]`, `:save-config! [path]`, `:wconfig! [path]`, `:source <path>`
        for (prefix, minimal) in [
            ("save-config!", false),
            ("wconfig!", false),
            ("save-config", true),
            ("wconfig", true),
        ] {
            if s == prefix {
                return Self::SaveConfig {
                    path: None,
                    minimal,
                };
            }
            if let Some(rest) = s.strip_prefix(prefix)
                && rest.starts_with(char::is_whitespace)
            {
                let arg = rest.trim().trim_matches('"').trim_matches('\'');
                return Self::SaveConfig {
                    path: if arg.is_empty() {
                        None
                    } else {
                        Some(arg.to_string())
                    },
                    minimal,
                };
            }
        }
        if let Some(rest) = s.strip_prefix("source ") {
            let arg = rest.trim().trim_matches('"').trim_matches('\'');
            if !arg.is_empty() {
                return Self::SourceConfig(arg.to_string());
            }
        }

        // 6b. Toggle command: e.g. "toggle line-number"
        if let Some(arg) = s.strip_prefix("toggle ") {
            return Self::Toggle(arg.trim().to_string());
        }

        // 6c. Set command: e.g. "set mouse = true", "set syntax-theme ?", "set syntax-theme Nord",
        //     or Vim line-number/read-only shorthands ("set number", "set nu", "set nonumber", "set nonu", "set ro", "set noro")
        if let Some(arg) = s.strip_prefix("set ") {
            let arg = arg.trim();
            match arg.to_ascii_lowercase().as_str() {
                "number" | "nu" | "line-number" | "lineno" => {
                    return Self::Set {
                        variable: "line-number".to_string(),
                        value: "yes".to_string(),
                    };
                }
                "nonumber" | "nonu" | "no-line-number" | "nolineno" => {
                    return Self::Set {
                        variable: "line-number".to_string(),
                        value: "no".to_string(),
                    };
                }
                "invnumber" | "invnu" | "number!" | "nu!" => {
                    return Self::Toggle("line-number".to_string());
                }
                "ro" | "readonly" | "read-only" | "read_only" => {
                    return Self::Set {
                        variable: "read-only".to_string(),
                        value: "true".to_string(),
                    };
                }
                "noro" | "noreadonly" | "no-read-only" | "no_read_only" | "update-mode"
                | "update_mode" | "rw" => {
                    return Self::Set {
                        variable: "read-only".to_string(),
                        value: "false".to_string(),
                    };
                }
                "invro" | "ro!" | "readonly!" | "read-only!" => {
                    return Self::Toggle("read-only".to_string());
                }
                "wrap" | "wrap-lines" | "wrap_lines" => {
                    return Self::Set {
                        variable: "wrap-lines".to_string(),
                        value: "yes".to_string(),
                    };
                }
                "nowrap" | "no-wrap" | "no-wrap-lines" | "no_wrap_lines" => {
                    return Self::Set {
                        variable: "wrap-lines".to_string(),
                        value: "no".to_string(),
                    };
                }
                "invwrap" | "wrap!" | "wrap-lines!" => {
                    return Self::Toggle("wrap-lines".to_string());
                }
                _ => {}
            }
            if let Some((var, val)) = arg.split_once('=') {
                return Self::Set {
                    variable: var.trim().to_string(),
                    value: val.trim().trim_matches('"').trim_matches('\'').to_string(),
                };
            }
            if let Some((var, val)) = arg.split_once(char::is_whitespace) {
                return Self::Set {
                    variable: var.trim().to_string(),
                    value: val.trim().trim_matches('"').trim_matches('\'').to_string(),
                };
            }
            return Self::Toggle(arg.to_string());
        }

        // 6d. Grep command: e.g. "grep <pattern>"
        if let Some(arg) = s.strip_prefix("grep ") {
            return Self::Grep(arg.trim().to_string());
        }

        // 7. Built-in action parsing (e.g. "view-main", ":toggle line-number")

        if let Ok(action) = Action::parse(s) {
            return Self::Builtin(action);
        }

        // 8. Hex SHA or Git ref pattern (7-40 hex chars or ref like HEAD~1, main)
        let is_hex_sha = s.len() >= 7 && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit());
        let is_ref = s.starts_with("HEAD")
            || s.contains('~')
            || s.contains('^')
            || s.contains('/')
            || (!s.contains(' ')
                && s.chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.'));

        if is_hex_sha || is_ref {
            return Self::Goto(s.to_string());
        }

        Self::Unknown(format!("Unknown command: '{trimmed}'"))
    }
}

/// Interactive prompt state machine managing input buffer, cursor, and history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptState {
    /// The prompt kind and associated metadata.
    pub kind: PromptKind,
    /// Character buffer of currently typed input.
    pub buffer: String,
    /// Cursor position in character (Unicode scalar value) offset.
    pub cursor: usize,
}

impl PromptState {
    /// Creates a new empty [`PromptState`] of the given kind.
    pub fn new(kind: PromptKind) -> Self {
        Self {
            kind,
            buffer: String::new(),
            cursor: 0,
        }
    }

    /// Creates a prompt initialized with pre-filled input text and cursor at the end.
    pub fn with_initial_text(kind: PromptKind, initial: &str) -> Self {
        let char_count = initial.chars().count();
        Self {
            kind,
            buffer: initial.to_string(),
            cursor: char_count,
        }
    }

    /// Returns the prompt prefix string displayed on the terminal screen.
    pub fn prompt_prefix(&self) -> &str {
        match &self.kind {
            PromptKind::Command => ":",
            PromptKind::SearchForward => "/",
            PromptKind::SearchBackward => "?",
            PromptKind::InteractiveMacro { label, .. } => {
                if label.is_empty() {
                    ":"
                } else {
                    label.as_str()
                }
            }
            PromptKind::Confirm { message, .. } => message.as_str(),
            PromptKind::OptionMenu(_) => "",
        }
    }

    /// Creates an interactive option menu prompt initialized in the `All` view.
    #[must_use]
    pub fn new_option_menu() -> Self {
        Self {
            kind: PromptKind::OptionMenu(crate::options::OptionMenuState::new()),
            buffer: String::new(),
            cursor: 0,
        }
    }

    /// Creates a context-aware interactive option menu prompt pre-selected on the category tab matching `view`.
    #[must_use]
    pub fn new_option_menu_for_view(view: Option<crate::app::layout::ViewKind>) -> Self {
        Self {
            kind: PromptKind::OptionMenu(crate::options::OptionMenuState::for_view(view)),
            buffer: String::new(),
            cursor: 0,
        }
    }

    /// Returns the number of characters in the current buffer.
    pub fn char_count(&self) -> usize {
        self.buffer.chars().count()
    }

    /// Inserts a character at the current cursor position.
    pub fn insert_char(&mut self, c: char) {
        if c.is_control() && c != '\t' {
            return;
        }
        let byte_idx = self
            .buffer
            .char_indices()
            .nth(self.cursor)
            .map_or(self.buffer.len(), |(idx, _)| idx);
        self.buffer.insert(byte_idx, c);
        self.cursor += 1;
    }

    /// Deletes the character immediately preceding the cursor (Backspace).
    pub fn delete_char_before_cursor(&mut self) {
        if self.cursor > 0 {
            let target_char = self.cursor - 1;
            let byte_idx = self
                .buffer
                .char_indices()
                .nth(target_char)
                .map(|(idx, _)| idx);
            if let Some(idx) = byte_idx {
                self.buffer.remove(idx);
                self.cursor -= 1;
            }
        }
    }

    /// Deletes the character directly under the cursor (Delete).
    pub fn delete_char_at_cursor(&mut self) {
        let byte_idx = self
            .buffer
            .char_indices()
            .nth(self.cursor)
            .map(|(idx, _)| idx);
        if let Some(idx) = byte_idx {
            self.buffer.remove(idx);
        }
    }

    /// Deletes the word preceding the cursor (`Ctrl-W`).
    pub fn delete_word_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let chars: Vec<char> = self.buffer.chars().collect();
        let mut new_cursor = self.cursor;

        // Skip trailing spaces
        while new_cursor > 0 && chars[new_cursor - 1].is_whitespace() {
            new_cursor -= 1;
        }
        // Skip word characters
        while new_cursor > 0 && !chars[new_cursor - 1].is_whitespace() {
            new_cursor -= 1;
        }

        let start_byte = self
            .buffer
            .char_indices()
            .nth(new_cursor)
            .map_or(self.buffer.len(), |(idx, _)| idx);
        let end_byte = self
            .buffer
            .char_indices()
            .nth(self.cursor)
            .map_or(self.buffer.len(), |(idx, _)| idx);

        self.buffer.drain(start_byte..end_byte);
        self.cursor = new_cursor;
    }

    /// Clears the line from the cursor position to the end (`Ctrl-K`).
    pub fn kill_to_end(&mut self) {
        let byte_idx = self
            .buffer
            .char_indices()
            .nth(self.cursor)
            .map_or(self.buffer.len(), |(idx, _)| idx);
        self.buffer.truncate(byte_idx);
    }

    /// Clears the entire prompt buffer (`Ctrl-U`).
    pub fn clear_line(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    /// Processes an incoming keyboard event, updating prompt state or returning a result.
    pub fn handle_key(&mut self, key: &KeyEvent, history: &mut HistoryManager) -> PromptResult {
        // Handle option menu prompt separately
        if let PromptKind::OptionMenu(ref mut menu) = self.kind {
            if let Some(ref mut path_buf) = menu.save_as_input {
                if key.code == KeyCode::Esc
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    menu.save_as_input = None;
                    return PromptResult::None;
                }
                if key.code == KeyCode::Enter {
                    let trimmed = path_buf.trim().to_string();
                    menu.save_as_input = None;
                    return PromptResult::SaveConfig {
                        path: if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed)
                        },
                        minimal: menu.minimal_save,
                    };
                }
                match key.code {
                    KeyCode::Backspace => {
                        path_buf.pop();
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        path_buf.clear();
                    }
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL) && !c.is_control() =>
                    {
                        path_buf.push(c);
                    }
                    _ => {}
                }
                return PromptResult::None;
            }

            if menu.filter_active {
                if key.code == KeyCode::Esc
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    menu.filter_query.clear();
                    menu.filter_active = false;
                    menu.sync_selection_to_visible();
                    return PromptResult::None;
                }
                if key.code == KeyCode::Enter {
                    menu.filter_active = false;
                    return PromptResult::None;
                }
                if key.code == KeyCode::Up {
                    menu.filter_active = false;
                    menu.prev();
                    return PromptResult::None;
                }
                if key.code == KeyCode::Down {
                    menu.filter_active = false;
                    menu.next();
                    return PromptResult::None;
                }
                match key.code {
                    KeyCode::Backspace => {
                        if menu.filter_query.pop().is_none() {
                            menu.filter_active = false;
                        }
                        menu.sync_selection_to_visible();
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        menu.filter_query.clear();
                        menu.sync_selection_to_visible();
                    }
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL) && !c.is_control() =>
                    {
                        menu.filter_query.push(c);
                        menu.sync_selection_to_visible();
                    }
                    _ => {}
                }
                return PromptResult::None;
            }

            if key.code == KeyCode::Esc {
                if !menu.filter_query.is_empty() {
                    menu.filter_query.clear();
                    menu.sync_selection_to_visible();
                    return PromptResult::None;
                }
                return PromptResult::Cancel;
            }
            if key.code == KeyCode::Char('q')
                || key.code == KeyCode::Char('o')
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                return PromptResult::Cancel;
            }
            if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return PromptResult::SaveConfig {
                    path: None,
                    minimal: menu.minimal_save,
                };
            }
            if key.code == KeyCode::Char('u') && key.modifiers.contains(KeyModifiers::CONTROL) {
                menu.page_up(5);
                return PromptResult::None;
            }
            if key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL) {
                menu.page_down(5);
                return PromptResult::None;
            }
            if key.code == KeyCode::Enter || key.code == KeyCode::Right {
                if menu.visible_indices().is_empty() {
                    return PromptResult::None;
                }
                return PromptResult::OptionChanged(menu.current_item().action);
            }
            if key.code == KeyCode::Left || key.code == KeyCode::Backspace {
                if menu.visible_indices().is_empty() {
                    return PromptResult::None;
                }
                return PromptResult::OptionChanged(menu.current_prev_action());
            }
            if key.code == KeyCode::Up || key.code == KeyCode::Char('k') {
                menu.prev();
                return PromptResult::None;
            }
            if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
                menu.next();
                return PromptResult::None;
            }
            if key.code == KeyCode::Home {
                menu.first();
                return PromptResult::None;
            }
            if key.code == KeyCode::End {
                menu.last();
                return PromptResult::None;
            }
            if key.code == KeyCode::PageUp {
                menu.page_up(5);
                return PromptResult::None;
            }
            if key.code == KeyCode::PageDown {
                menu.page_down(5);
                return PromptResult::None;
            }
            if key.code == KeyCode::Tab {
                menu.next_category_tab();
                return PromptResult::None;
            }
            if key.code == KeyCode::BackTab {
                menu.prev_category_tab();
                return PromptResult::None;
            }
            if let KeyCode::Char(c) = key.code {
                match c {
                    ' ' | 'l' | ']' => {
                        if menu.visible_indices().is_empty() {
                            return PromptResult::None;
                        }
                        return PromptResult::OptionChanged(menu.current_item().action);
                    }
                    'h' | '[' => {
                        if menu.visible_indices().is_empty() {
                            return PromptResult::None;
                        }
                        return PromptResult::OptionChanged(menu.current_prev_action());
                    }
                    'g' => {
                        menu.first();
                        return PromptResult::None;
                    }
                    'G' => {
                        menu.last();
                        return PromptResult::None;
                    }
                    '/' => {
                        menu.filter_active = true;
                        return PromptResult::None;
                    }
                    '1'..='5' => {
                        menu.set_category_tab(c as u8 - b'0');
                        return PromptResult::None;
                    }
                    'r' => {
                        if let Some(act) = menu.current_reset_action() {
                            return PromptResult::OptionChanged(act);
                        }
                        return PromptResult::None;
                    }
                    'R' => {
                        return PromptResult::OptionChanged(MenuAction::ResetAll);
                    }
                    'M' => {
                        menu.minimal_save = !menu.minimal_save;
                        return PromptResult::None;
                    }
                    'p' => {
                        return PromptResult::SaveConfig {
                            path: None,
                            minimal: menu.minimal_save,
                        };
                    }
                    'P' => {
                        let initial = tigrs_core::Config::default_config_path().map_or_else(
                            || "~/.config/tigrs/config.toml".to_string(),
                            |p| p.display().to_string(),
                        );
                        menu.save_as_input = Some(initial);
                        return PromptResult::None;
                    }
                    _ => {}
                }
                if let Some(idx) = crate::options::OptionMenuState::find_hotkey(c) {
                    menu.selected = idx;
                    return PromptResult::OptionChanged(menu.current_item().action);
                }
            }
            return PromptResult::None;
        }

        // Handle confirmation prompts separately: only Y/N/Enter/Esc
        if matches!(self.kind, PromptKind::Confirm { .. }) {
            return match key.code {
                KeyCode::Char('y' | 'Y') => PromptResult::Confirm(true),
                KeyCode::Char('n' | 'N') | KeyCode::Enter | KeyCode::Esc => {
                    PromptResult::Confirm(false)
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    PromptResult::Confirm(false)
                }
                _ => PromptResult::None,
            };
        }

        // Cancel on Esc or Ctrl-C
        if key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            history.reset_navigation();
            return PromptResult::Cancel;
        }

        // Submit on Enter
        if key.code == KeyCode::Enter {
            history.reset_navigation();
            return PromptResult::Submit(self.buffer.clone());
        }

        // Cursor movement & editing
        match key.code {
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = 0;
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.char_count();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_line();
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.kill_to_end();
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.delete_word_backward();
            }

            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor < self.char_count() {
                    self.cursor += 1;
                }
            }
            KeyCode::Home => {
                self.cursor = 0;
            }
            KeyCode::End => {
                self.cursor = self.char_count();
            }
            KeyCode::Backspace => {
                self.delete_char_before_cursor();
            }
            KeyCode::Delete => {
                self.delete_char_at_cursor();
            }

            // History navigation
            KeyCode::Up | KeyCode::Char('p')
                if key.code == KeyCode::Up || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if let Some(entry) = history.prev(&self.buffer) {
                    self.buffer = entry.to_string();
                    self.cursor = self.char_count();
                }
            }
            KeyCode::Down | KeyCode::Char('n')
                if key.code == KeyCode::Down || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if let Some(entry) = history.next() {
                    self.buffer = entry.to_string();
                    self.cursor = self.char_count();
                }
            }

            KeyCode::Char(c) => {
                self.insert_char(c);
            }

            _ => {}
        }

        PromptResult::None
    }

    /// Formats the prompt line for rendering on the screen within the given `width`.
    ///
    /// Returns a tuple `(rendered_text, screen_cursor_x)` where `screen_cursor_x`
    /// is the 0-based column index where the terminal cursor should be placed.
    pub fn render_line(&self, width: usize) -> (String, usize) {
        if let PromptKind::OptionMenu(ref menu) = self.kind {
            let rendered = menu.render_line(width);
            let len = UnicodeWidthStr::width(rendered.as_str());
            return (rendered, len.min(width.saturating_sub(1)));
        }

        let prefix_cow = tigrs_core::ansi::strip_control_chars(self.prompt_prefix());
        let prefix = prefix_cow.as_ref();
        let prefix_width = UnicodeWidthStr::width(prefix);

        if width <= prefix_width {
            return (
                tigrs_core::ansi::truncate_display_width(prefix, width).to_string(),
                0,
            );
        }

        let max_content_width = width - prefix_width;
        let safe_buf = tigrs_core::ansi::strip_control_chars(&self.buffer);
        let chars: Vec<char> = safe_buf.chars().collect();
        let char_widths: Vec<usize> = chars
            .iter()
            .map(|c| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0))
            .collect();
        let cursor_idx = self.cursor.min(chars.len());
        let width_before_cursor: usize = char_widths[..cursor_idx].iter().sum();

        let (start_char, screen_cursor) = if width_before_cursor < max_content_width {
            (0, prefix_width + width_before_cursor)
        } else {
            let cursor_cell_width = char_widths.get(cursor_idx).copied().unwrap_or(1).max(1);
            let avail_before = max_content_width.saturating_sub(cursor_cell_width);
            let mut start = cursor_idx;
            let mut acc = 0;
            while start > 0 {
                let w = char_widths[start - 1];
                if acc + w > avail_before {
                    break;
                }
                acc += w;
                start -= 1;
            }
            (start, prefix_width + acc)
        };

        let mut visible_chars = String::new();
        let mut used_width = 0;
        for (i, &ch) in chars.iter().enumerate().skip(start_char) {
            let w = char_widths[i];
            if used_width + w > max_content_width {
                break;
            }
            visible_chars.push(ch);
            used_width += w;
        }

        let mut line = format!("{prefix}{visible_chars}");
        let total_line_width = prefix_width + used_width;
        if total_line_width < width {
            let pad = " ".repeat(width - total_line_width);
            line.push_str(&pad);
        }

        (line, screen_cursor.min(width.saturating_sub(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ViewKind;
    use crate::keymap::RunFlags;
    use crate::options::OptionId;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn test_parsed_command_variants() {
        assert_eq!(ParsedCommand::parse("42"), ParsedCommand::LineNumber(42));
        assert_eq!(ParsedCommand::parse(":100"), ParsedCommand::LineNumber(100));

        assert_eq!(
            ParsedCommand::parse("!git status"),
            ParsedCommand::Shell {
                command: "git status".to_string(),
                foreground: true,
            }
        );

        assert_eq!(
            ParsedCommand::parse("/needle"),
            ParsedCommand::Search {
                query: "needle".to_string(),
                backward: false,
            }
        );
        assert_eq!(
            ParsedCommand::parse("?backneedle"),
            ParsedCommand::Search {
                query: "backneedle".to_string(),
                backward: true,
            }
        );

        assert_eq!(
            ParsedCommand::parse("q"),
            ParsedCommand::Builtin(Action::Quit)
        );
        assert_eq!(
            ParsedCommand::parse("quit"),
            ParsedCommand::Builtin(Action::Quit)
        );
        assert_eq!(
            ParsedCommand::parse("view-diff"),
            ParsedCommand::Builtin(Action::OpenView(ViewKind::Diff))
        );

        assert_eq!(
            ParsedCommand::parse("echo Hello Tig"),
            ParsedCommand::Echo("Hello Tig".to_string())
        );

        assert_eq!(
            ParsedCommand::parse("2f12bcc"),
            ParsedCommand::Goto("2f12bcc".to_string())
        );
    }

    #[test]
    fn test_prompt_character_editing() {
        let mut prompt = PromptState::new(PromptKind::Command);
        let mut hist = HistoryManager::new(10);

        prompt.handle_key(&key(KeyCode::Char('f')), &mut hist);
        prompt.handle_key(&key(KeyCode::Char('o')), &mut hist);
        prompt.handle_key(&key(KeyCode::Char('o')), &mut hist);
        assert_eq!(prompt.buffer, "foo");
        assert_eq!(prompt.cursor, 3);

        prompt.handle_key(&key(KeyCode::Left), &mut hist);
        assert_eq!(prompt.cursor, 2);

        prompt.handle_key(&key(KeyCode::Char('x')), &mut hist);
        assert_eq!(prompt.buffer, "foxo");
        assert_eq!(prompt.cursor, 3);

        prompt.handle_key(&key(KeyCode::Backspace), &mut hist);
        assert_eq!(prompt.buffer, "foo");
        assert_eq!(prompt.cursor, 2);

        prompt.handle_key(&ctrl_key('a'), &mut hist);
        assert_eq!(prompt.cursor, 0);

        prompt.handle_key(&ctrl_key('e'), &mut hist);
        assert_eq!(prompt.cursor, 3);
    }

    #[test]
    fn test_prompt_word_kill_and_clear() {
        let mut prompt = PromptState::with_initial_text(PromptKind::Command, "git commit -m");
        let mut hist = HistoryManager::new(10);

        prompt.handle_key(&ctrl_key('w'), &mut hist);
        assert_eq!(prompt.buffer, "git commit ");

        prompt.handle_key(&ctrl_key('u'), &mut hist);
        assert_eq!(prompt.buffer, "");
        assert_eq!(prompt.cursor, 0);
    }

    #[test]
    fn test_prompt_history_navigation() {
        let mut prompt = PromptState::new(PromptKind::Command);
        let mut hist = HistoryManager::new(10);
        hist.add("status");
        hist.add("diff");

        prompt.handle_key(&key(KeyCode::Up), &mut hist);
        assert_eq!(prompt.buffer, "diff");

        prompt.handle_key(&key(KeyCode::Up), &mut hist);
        assert_eq!(prompt.buffer, "status");

        prompt.handle_key(&key(KeyCode::Down), &mut hist);
        assert_eq!(prompt.buffer, "diff");

        prompt.handle_key(&key(KeyCode::Down), &mut hist);
        assert_eq!(prompt.buffer, "");
    }

    #[test]
    fn test_prompt_confirmation() {
        let mut prompt = PromptState::new(PromptKind::Confirm {
            message: "Reset? [y/N]".to_string(),
            expanded_command: "git reset".to_string(),
            run_command: Arc::new(RunCommand {
                flags: RunFlags::default(),
                command: "git reset".to_string(),
            }),
        });
        let mut hist = HistoryManager::new(10);

        let res = prompt.handle_key(&key(KeyCode::Char('y')), &mut hist);
        assert_eq!(res, PromptResult::Confirm(true));

        let res = prompt.handle_key(&key(KeyCode::Char('n')), &mut hist);
        assert_eq!(res, PromptResult::Confirm(false));
    }

    #[test]
    fn test_prompt_option_menu_interactions() {
        let mut prompt = PromptState::new_option_menu();
        let mut hist = HistoryManager::new(10);

        // Starts on item 0: UI color theme [t]
        assert!(
            prompt
                .render_line(40)
                .0
                .contains("Toggle option UI color theme [t]")
        );

        // Press Down to move to syntax theme [H]
        let res = prompt.handle_key(&key(KeyCode::Down), &mut hist);
        assert_eq!(res, PromptResult::None);
        assert!(
            prompt
                .render_line(40)
                .0
                .contains("Toggle option syntax theme [H]")
        );

        // Press Enter to cycle/toggle in-place (menu stays open!)
        let res = prompt.handle_key(&key(KeyCode::Enter), &mut hist);
        assert_eq!(
            res,
            PromptResult::OptionChanged(MenuAction::Toggle(OptionId::SyntaxTheme))
        );

        // Test direct hotkey selection: press 'W' toggles ignore-space in-place
        let mut prompt2 = PromptState::new_option_menu();
        let res = prompt2.handle_key(&key(KeyCode::Char('W')), &mut hist);
        assert_eq!(
            res,
            PromptResult::OptionChanged(MenuAction::Toggle(OptionId::IgnoreSpace))
        );

        // Test inline '/' search filter and Esc clearing filter before cancelling
        let mut prompt3 = PromptState::new_option_menu();
        assert_eq!(
            prompt3.handle_key(&key(KeyCode::Char('/')), &mut hist),
            PromptResult::None
        );
        for ch in "date".chars() {
            assert_eq!(
                prompt3.handle_key(&key(KeyCode::Char(ch)), &mut hist),
                PromptResult::None
            );
        }
        // Enter exits filter typing while keeping selection on Date
        assert_eq!(
            prompt3.handle_key(&key(KeyCode::Enter), &mut hist),
            PromptResult::None
        );
        // Pressing Enter again toggles Date in-place
        assert_eq!(
            prompt3.handle_key(&key(KeyCode::Enter), &mut hist),
            PromptResult::OptionChanged(MenuAction::Toggle(OptionId::Date))
        );
        // First Esc clears the filter query without closing the menu
        assert_eq!(
            prompt3.handle_key(&key(KeyCode::Esc), &mut hist),
            PromptResult::None
        );
        // Second Esc cancels and closes the menu
        let res = prompt3.handle_key(&key(KeyCode::Esc), &mut hist);
        assert_eq!(res, PromptResult::Cancel);
    }

    #[test]
    fn test_prompt_multibyte_utf8_and_cjk_editing() {
        let mut prompt = PromptState::new(PromptKind::Command);
        let mut hist = HistoryManager::new(10);

        // Insert CJK characters
        for c in "你好世界".chars() {
            prompt.handle_key(&key(KeyCode::Char(c)), &mut hist);
        }
        assert_eq!(prompt.buffer, "你好世界");
        assert_eq!(prompt.char_count(), 4);
        assert_eq!(prompt.cursor, 4);

        // Navigate left across multibyte character boundary
        prompt.handle_key(&key(KeyCode::Left), &mut hist);
        assert_eq!(prompt.cursor, 3);

        // Backspace deletes '世'
        prompt.handle_key(&key(KeyCode::Backspace), &mut hist);
        assert_eq!(prompt.buffer, "你好界");
        assert_eq!(prompt.cursor, 2);

        // Insert emoji '🚀'
        prompt.handle_key(&key(KeyCode::Char('🚀')), &mut hist);
        assert_eq!(prompt.buffer, "你好🚀界");
        assert_eq!(prompt.cursor, 3);

        // Delete key deletes '界'
        prompt.handle_key(&key(KeyCode::Delete), &mut hist);
        assert_eq!(prompt.buffer, "你好🚀");
        assert_eq!(prompt.cursor, 3);

        // Word kill backward with mixed ASCII and CJK
        let mut prompt2 = PromptState::with_initial_text(PromptKind::Command, "echo 你好 世界");
        prompt2.handle_key(&ctrl_key('w'), &mut hist);
        assert_eq!(prompt2.buffer, "echo 你好 ");

        // Render line with CJK and emoji doesn't panic and renders prefix
        let (rendered, cursor_col) = prompt.render_line(40);
        assert!(rendered.starts_with(':'));
        assert!(rendered.contains("你好🚀"));
        assert!(cursor_col > 0);
    }

    #[test]
    fn test_prompt_long_line_and_scrolling() {
        let long_str = "a".repeat(2000);
        let mut prompt = PromptState::with_initial_text(PromptKind::Command, &long_str);
        let mut hist = HistoryManager::new(10);

        assert_eq!(prompt.char_count(), 2000);
        assert_eq!(prompt.cursor, 2000);

        // Render in narrow terminal (width 80)
        let (rendered, col) = prompt.render_line(80);
        assert_eq!(rendered.len(), 80);
        assert!(col <= 80);

        // Backspace on long line
        prompt.handle_key(&key(KeyCode::Backspace), &mut hist);
        assert_eq!(prompt.char_count(), 1999);
    }

    #[test]
    fn test_prompt_additional_coverage() {
        // 1. ParsedCommand variants
        assert_eq!(ParsedCommand::parse(""), ParsedCommand::Empty);
        assert_eq!(ParsedCommand::parse("   "), ParsedCommand::Empty);
        assert_eq!(
            ParsedCommand::parse("goto 42"),
            ParsedCommand::LineNumber(42)
        );
        assert_eq!(
            ParsedCommand::parse("goto my_tag"),
            ParsedCommand::Goto("my_tag".to_string())
        );
        assert_eq!(
            ParsedCommand::parse("grep search_target"),
            ParsedCommand::Grep("search_target".to_string())
        );
        match ParsedCommand::parse("completely unknown command with spaces") {
            ParsedCommand::Unknown(msg) => {
                assert!(msg.contains("completely unknown command with spaces"));
            }
            other => panic!("Expected Unknown, got {other:?}"),
        }

        // 2. Prefixes
        let forward_prompt = PromptState::new(PromptKind::SearchForward);
        assert_eq!(forward_prompt.prompt_prefix(), "/");
        let backward_prompt = PromptState::new(PromptKind::SearchBackward);
        assert_eq!(backward_prompt.prompt_prefix(), "?");
        let prompt_cmd = PromptState::new(PromptKind::Command);
        assert_eq!(prompt_cmd.prompt_prefix(), ":");
        let prompt_opt = PromptState::new_option_menu();
        assert_eq!(prompt_opt.prompt_prefix(), "");

        // 3. Narrow terminal rendering (width < prefix)
        let (narrow_rendered, narrow_col) = prompt_cmd.render_line(0);
        assert_eq!(narrow_rendered, "");
        assert_eq!(narrow_col, 0);

        // 4. Ctrl-k (kill to end), Home, End
        let mut prompt = PromptState::with_initial_text(PromptKind::Command, "hello world");
        let mut hist = HistoryManager::new(10);
        // Move to start with Home
        prompt.handle_key(&key(KeyCode::Home), &mut hist);
        assert_eq!(prompt.cursor, 0);
        // Move right 5 chars
        for _ in 0..5 {
            prompt.handle_key(&key(KeyCode::Right), &mut hist);
        }
        assert_eq!(prompt.cursor, 5);
        // Ctrl-k kills to end
        prompt.handle_key(&ctrl_key('k'), &mut hist);
        assert_eq!(prompt.buffer, "hello");
        // End moves to end
        prompt.handle_key(&key(KeyCode::End), &mut hist);
        assert_eq!(prompt.cursor, 5);

        // 5. Option menu with Up arrow (menu.prev())
        let mut opt_prompt = PromptState::new_option_menu();
        let _ = opt_prompt.handle_key(&key(KeyCode::Down), &mut hist);
        let _ = opt_prompt.handle_key(&key(KeyCode::Up), &mut hist);
        assert!(
            opt_prompt
                .render_line(40)
                .0
                .contains("Toggle option UI color theme [t]")
        );

        // 6. Confirmation with Ctrl-c
        let mut confirm_prompt = PromptState::new(PromptKind::Confirm {
            message: "Sure? [y/N]".to_string(),
            expanded_command: "cmd".to_string(),
            run_command: Arc::new(RunCommand {
                flags: RunFlags::default(),
                command: "cmd".to_string(),
            }),
        });
        let res = confirm_prompt.handle_key(&ctrl_key('c'), &mut hist);
        assert_eq!(res, PromptResult::Confirm(false));
    }

    #[test]
    fn test_prompt_render_line_wide_utf8_display_columns() {
        let prompt = PromptState::with_initial_text(PromptKind::SearchForward, "測試🦀abc");
        // Prefix "/" is 1 col; "測" (2) + "試" (2) + "🦀" (2) + "abc" (3) = 9 cols -> total 10 cols
        let (rendered, cursor_x) = prompt.render_line(20);
        assert_eq!(UnicodeWidthStr::width(rendered.as_str()), 20);
        assert_eq!(cursor_x, 10);
    }
}
