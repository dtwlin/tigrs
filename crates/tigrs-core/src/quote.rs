// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Shell quoting and macro interpolation engine.
//!
//! Provides safe quoting for external shell command execution, completely
//! neutralizing command injection vulnerabilities in untrusted inputs
//! such as branch names, commit hashes, filenames, and commit messages.

use crate::error::{Result, TigError};
use std::borrow::Cow;
use std::collections::HashMap;

/// Trait for macro variable lookup during command template interpolation.
pub trait MacroLookup {
    /// Look up a macro key (e.g. `"commit"`, `"file"`).
    fn lookup(&self, key: &str) -> Option<Cow<'_, str>>;
}

impl<F> MacroLookup for F
where
    F: Fn(&str) -> Option<Cow<'static, str>>,
{
    fn lookup(&self, key: &str) -> Option<Cow<'_, str>> {
        self(key)
    }
}

impl<S: std::hash::BuildHasher> MacroLookup for HashMap<&str, &str, S> {
    fn lookup(&self, key: &str) -> Option<Cow<'_, str>> {
        self.get(key).copied().map(Cow::Borrowed)
    }
}

impl<S: std::hash::BuildHasher> MacroLookup for HashMap<String, String, S> {
    fn lookup(&self, key: &str) -> Option<Cow<'_, str>> {
        self.get(key).map(|s| Cow::Borrowed(s.as_str()))
    }
}

impl<S: std::hash::BuildHasher> MacroLookup for HashMap<&str, String, S> {
    fn lookup(&self, key: &str) -> Option<Cow<'_, str>> {
        self.get(key).map(|s| Cow::Borrowed(s.as_str()))
    }
}

/// Tracks the inner subshell quoting state inside a backtick command substitution (`` `...` ``).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InnerQuote {
    /// Outside any inner quotes within the backtick subshell.
    #[default]
    Unquoted,
    /// Inside single quotes `'...'` within the backtick subshell.
    SingleQuoted,
    /// Inside double quotes `"..."` within the backtick subshell.
    DoubleQuoted,
}

impl InnerQuote {
    const fn toggle_single(self) -> Self {
        match self {
            Self::Unquoted => Self::SingleQuoted,
            Self::SingleQuoted => Self::Unquoted,
            Self::DoubleQuoted => Self::DoubleQuoted,
        }
    }

    const fn toggle_double(self) -> Self {
        match self {
            Self::Unquoted => Self::DoubleQuoted,
            Self::DoubleQuoted => Self::Unquoted,
            Self::SingleQuoted => Self::SingleQuoted,
        }
    }
}

/// Tracks the POSIX shell quoting state at a given position in a command template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteState {
    /// Outside any quotes.
    Unquoted,
    /// Outside any quotes, immediately after an unescaped `$`.
    UnquotedDollar,
    /// Inside single quotes `'...'`.
    SingleQuoted,
    /// Inside double quotes `"..."`.
    DoubleQuoted,
    /// Inside double quotes `"..."`, immediately after an unescaped `$`.
    DoubleQuotedDollar,
    /// Inside a `$(...)` command substitution (either unquoted or inside double quotes),
    /// where POSIX shell resets the quoting context independently of any outer `"..."`.
    CommandSub {
        /// Whether the outermost `$(...)` was opened inside `"..."`.
        outer_double_quoted: bool,
        /// Current parenthesis nesting depth (`>= 1`).
        depth: u32,
        /// Bitmask stack of enclosing `InnerQuote` states (`1` = `DoubleQuoted`, `0` = `Unquoted`)
        /// for each nested `(` / `$(` level so closing `)` restores the exact enclosing quote state.
        parent_double_mask: u64,
        /// Inner quote state if the cursor is currently inside a backtick `` `...` `` within the subshell.
        in_backtick: Option<InnerQuote>,
        /// Active quote state inside the current `$(...)` subshell level (outside any backtick).
        inner: InnerQuote,
        /// Whether the immediately preceding character inside the subshell was an unescaped `$`.
        inner_dollar: bool,
    },
    /// Inside unquoted backtick command substitution `` `...` `` with inner subshell quote state.
    Backtick(InnerQuote),
    /// Inside backtick command substitution within double quotes `` "`...`" `` with inner subshell quote state.
    DoubleQuotedBacktick(InnerQuote),
}

impl QuoteState {
    /// Advances the quote state across a template substring slice.
    pub fn advance(&mut self, s: &str) {
        let mut escaped = false;
        for ch in s.chars() {
            if escaped {
                match *self {
                    Self::UnquotedDollar => *self = Self::Unquoted,
                    Self::DoubleQuotedDollar => *self = Self::DoubleQuoted,
                    Self::CommandSub {
                        outer_double_quoted,
                        depth,
                        parent_double_mask,
                        in_backtick,
                        inner,
                        ..
                    } => {
                        let next_bt = match in_backtick {
                            Some(bt_inner) if inner == InnerQuote::DoubleQuoted && ch == '"' => {
                                Some(bt_inner.toggle_double())
                            }
                            other => other,
                        };
                        *self = Self::CommandSub {
                            outer_double_quoted,
                            depth,
                            parent_double_mask,
                            in_backtick: next_bt,
                            inner,
                            inner_dollar: false,
                        };
                    }
                    Self::DoubleQuotedBacktick(inner) if ch == '"' => {
                        // Inside `"`...`"`, `\"` escapes `"` for the outer double quote and delivers
                        // a literal `"` to the inner backtick subshell.
                        *self = Self::DoubleQuotedBacktick(inner.toggle_double());
                    }
                    _ => {}
                }
                escaped = false;
                continue;
            }
            match *self {
                Self::Unquoted => match ch {
                    '\\' => escaped = true,
                    '\'' => *self = Self::SingleQuoted,
                    '"' => *self = Self::DoubleQuoted,
                    '$' => *self = Self::UnquotedDollar,
                    '`' => *self = Self::Backtick(InnerQuote::Unquoted),
                    _ => {}
                },
                Self::UnquotedDollar => match ch {
                    '(' => {
                        *self = Self::CommandSub {
                            outer_double_quoted: false,
                            depth: 1,
                            parent_double_mask: 0,
                            in_backtick: None,
                            inner: InnerQuote::Unquoted,
                            inner_dollar: false,
                        };
                    }
                    '\\' => {
                        *self = Self::Unquoted;
                        escaped = true;
                    }
                    '\'' => *self = Self::SingleQuoted,
                    '"' => *self = Self::DoubleQuoted,
                    '$' => *self = Self::UnquotedDollar,
                    '`' => *self = Self::Backtick(InnerQuote::Unquoted),
                    _ => *self = Self::Unquoted,
                },
                Self::SingleQuoted => {
                    if ch == '\'' {
                        *self = Self::Unquoted;
                    }
                }
                Self::DoubleQuoted => match ch {
                    '\\' => escaped = true,
                    '"' => *self = Self::Unquoted,
                    '$' => *self = Self::DoubleQuotedDollar,
                    '`' => *self = Self::DoubleQuotedBacktick(InnerQuote::Unquoted),
                    _ => {}
                },
                Self::DoubleQuotedDollar => match ch {
                    '(' => {
                        *self = Self::CommandSub {
                            outer_double_quoted: true,
                            depth: 1,
                            parent_double_mask: 0,
                            in_backtick: None,
                            inner: InnerQuote::Unquoted,
                            inner_dollar: false,
                        };
                    }
                    '\\' => {
                        *self = Self::DoubleQuoted;
                        escaped = true;
                    }
                    '"' => *self = Self::Unquoted,
                    '$' => *self = Self::DoubleQuotedDollar,
                    '`' => *self = Self::DoubleQuotedBacktick(InnerQuote::Unquoted),
                    _ => *self = Self::DoubleQuoted,
                },
                Self::CommandSub {
                    outer_double_quoted,
                    depth,
                    parent_double_mask,
                    in_backtick,
                    inner,
                    inner_dollar,
                } => {
                    if let Some(bt_inner) = in_backtick {
                        match ch {
                            '\\' => {
                                escaped = true;
                            }
                            '`' => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner,
                                    inner_dollar: false,
                                };
                            }
                            '\'' => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: Some(bt_inner.toggle_single()),
                                    inner,
                                    inner_dollar: false,
                                };
                            }
                            '"' => {
                                if inner == InnerQuote::DoubleQuoted {
                                    *self = Self::CommandSub {
                                        outer_double_quoted,
                                        depth,
                                        parent_double_mask,
                                        in_backtick: None,
                                        inner: InnerQuote::Unquoted,
                                        inner_dollar: false,
                                    };
                                } else {
                                    *self = Self::CommandSub {
                                        outer_double_quoted,
                                        depth,
                                        parent_double_mask,
                                        in_backtick: Some(bt_inner.toggle_double()),
                                        inner,
                                        inner_dollar: false,
                                    };
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if inner_dollar && ch == '(' && inner != InnerQuote::SingleQuoted {
                        let parent_bit = u64::from(inner == InnerQuote::DoubleQuoted);
                        *self = Self::CommandSub {
                            outer_double_quoted,
                            depth: depth.saturating_add(1),
                            parent_double_mask: (parent_double_mask << 1) | parent_bit,
                            in_backtick: None,
                            inner: InnerQuote::Unquoted,
                            inner_dollar: false,
                        };
                        continue;
                    }
                    match inner {
                        InnerQuote::Unquoted => match ch {
                            '\\' => {
                                escaped = true;
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner,
                                    inner_dollar: false,
                                };
                            }
                            '`' => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: Some(InnerQuote::Unquoted),
                                    inner,
                                    inner_dollar: false,
                                };
                            }
                            '\'' => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner: InnerQuote::SingleQuoted,
                                    inner_dollar: false,
                                };
                            }
                            '"' => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner: InnerQuote::DoubleQuoted,
                                    inner_dollar: false,
                                };
                            }
                            '$' => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner,
                                    inner_dollar: true,
                                };
                            }
                            '(' => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth: depth.saturating_add(1),
                                    parent_double_mask: parent_double_mask << 1,
                                    in_backtick: None,
                                    inner,
                                    inner_dollar: false,
                                };
                            }
                            ')' => {
                                if depth <= 1 {
                                    *self = if outer_double_quoted {
                                        Self::DoubleQuoted
                                    } else {
                                        Self::Unquoted
                                    };
                                } else {
                                    let restored_inner = if (parent_double_mask & 1) != 0 {
                                        InnerQuote::DoubleQuoted
                                    } else {
                                        InnerQuote::Unquoted
                                    };
                                    *self = Self::CommandSub {
                                        outer_double_quoted,
                                        depth: depth - 1,
                                        parent_double_mask: parent_double_mask >> 1,
                                        in_backtick: None,
                                        inner: restored_inner,
                                        inner_dollar: false,
                                    };
                                }
                            }
                            _ => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner,
                                    inner_dollar: false,
                                };
                            }
                        },
                        InnerQuote::SingleQuoted => {
                            if ch == '\'' {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner: InnerQuote::Unquoted,
                                    inner_dollar: false,
                                };
                            }
                        }
                        InnerQuote::DoubleQuoted => match ch {
                            '\\' => {
                                escaped = true;
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner,
                                    inner_dollar: false,
                                };
                            }
                            '`' => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: Some(InnerQuote::Unquoted),
                                    inner,
                                    inner_dollar: false,
                                };
                            }
                            '"' => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner: InnerQuote::Unquoted,
                                    inner_dollar: false,
                                };
                            }
                            '$' => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner,
                                    inner_dollar: true,
                                };
                            }
                            _ => {
                                *self = Self::CommandSub {
                                    outer_double_quoted,
                                    depth,
                                    parent_double_mask,
                                    in_backtick: None,
                                    inner,
                                    inner_dollar: false,
                                };
                            }
                        },
                    }
                }
                Self::Backtick(inner) => match ch {
                    '\\' => escaped = true,
                    '`' => *self = Self::Unquoted,
                    '\'' => *self = Self::Backtick(inner.toggle_single()),
                    '"' => *self = Self::Backtick(inner.toggle_double()),
                    _ => {}
                },
                Self::DoubleQuotedBacktick(inner) => match ch {
                    '\\' => escaped = true,
                    '`' => *self = Self::DoubleQuoted,
                    '\'' => *self = Self::DoubleQuotedBacktick(inner.toggle_single()),
                    '"' => *self = Self::Unquoted,
                    _ => {}
                },
            }
        }
    }
}

fn push_backtick_escaped(
    result: &mut String,
    quoted: &str,
    inner: InnerQuote,
    outer_double_quoted: bool,
) {
    let subshell_fragment = match inner {
        InnerQuote::Unquoted => quoted.to_string(),
        InnerQuote::SingleQuoted => format!("'{quoted}'"),
        InnerQuote::DoubleQuoted => format!("\"{quoted}\""),
    };
    for ch in subshell_fragment.chars() {
        if matches!(ch, '\\' | '$' | '`') || (outer_double_quoted && ch == '"') {
            result.push('\\');
        }
        result.push(ch);
    }
}

fn neutralize_trailing_backslashes(result: &mut String) {
    let trailing_backslashes = result.chars().rev().take_while(|&c| c == '\\').count();
    if trailing_backslashes % 2 == 1 {
        result.push('\\');
    }
}

/// Appends a shell-quoted value into `result`, safely adapting to the active [`QuoteState`]
/// so that single quotes are never interpreted literally inside double quotes (`"..."`),
/// never prematurely terminate single-quoted templates (`'...'`), never have their opening
/// quote escaped by a trailing backslash (`\`) from the preceding template prefix, and never
/// break out of `$()` or backtick command substitutions (`` `...` ``), including nested quotes
/// inside subshells.
pub fn append_quoted_in_context(result: &mut String, val: &str, quote_state: QuoteState) {
    let quoted = shell_quote(val);
    match quote_state {
        QuoteState::Unquoted => {
            neutralize_trailing_backslashes(result);
            result.push_str(&quoted);
        }
        QuoteState::UnquotedDollar => {
            // Separate trailing `$` from `'...'` with `""` so `$` + `'...'` cannot form
            // an ANSI-C `$'...'` string literal in shells that interpret `\'` inside `$'...'`.
            neutralize_trailing_backslashes(result);
            result.push_str("\"\"");
            result.push_str(&quoted);
        }
        QuoteState::DoubleQuoted | QuoteState::DoubleQuotedDollar => {
            // Ensure an odd number of trailing backslashes in `result` does not escape our closing `"`
            neutralize_trailing_backslashes(result);
            result.push('"');
            result.push_str(&quoted);
            result.push('"');
        }
        QuoteState::SingleQuoted => {
            result.push('\'');
            result.push_str(&quoted);
            result.push('\'');
        }
        QuoteState::CommandSub {
            in_backtick,
            inner,
            inner_dollar,
            ..
        } => {
            if let Some(bt_inner) = in_backtick {
                if bt_inner != InnerQuote::SingleQuoted {
                    neutralize_trailing_backslashes(result);
                }
                push_backtick_escaped(result, &quoted, bt_inner, inner == InnerQuote::DoubleQuoted);
            } else {
                match inner {
                    InnerQuote::Unquoted => {
                        // Inside `$(...)` (even when `$(...)` is enclosed in outer double quotes),
                        // POSIX shells start a fresh unquoted command parser context where `'...'`
                        // single-quotes literally and emitting `"` would open a new inner double quote.
                        neutralize_trailing_backslashes(result);
                        if inner_dollar {
                            result.push_str("\"\"");
                        }
                        result.push_str(&quoted);
                    }
                    InnerQuote::SingleQuoted => {
                        result.push('\'');
                        result.push_str(&quoted);
                        result.push('\'');
                    }
                    InnerQuote::DoubleQuoted => {
                        neutralize_trailing_backslashes(result);
                        result.push('"');
                        result.push_str(&quoted);
                        result.push('"');
                    }
                }
            }
        }
        QuoteState::Backtick(inner) => {
            if inner != InnerQuote::SingleQuoted {
                neutralize_trailing_backslashes(result);
            }
            push_backtick_escaped(result, &quoted, inner, false);
        }
        QuoteState::DoubleQuotedBacktick(inner) => {
            if inner != InnerQuote::SingleQuoted {
                neutralize_trailing_backslashes(result);
            }
            push_backtick_escaped(result, &quoted, inner, true);
        }
    }
}

/// Finds the matching closing parenthesis index for an opened `%(`, accounting for nested parentheses.
#[must_use]
pub fn find_matching_close_paren(s: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (idx, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

/// Escapes and quotes a string for safe usage in POSIX shell commands.
///
/// Wraps the input in single quotes `'...'` and escapes any internal single quotes
/// as `'\''`, which terminates the quoted string, inserts a literal escaped single quote,
/// and reopens the single-quoted string.
///
/// In POSIX shells (bash, sh, dash, zsh), single quotes disable all parameter expansion,
/// command substitution, arithmetic expansion, and process substitution.
///
/// # Examples
///
/// ```
/// use tigrs_core::quote::shell_quote;
///
/// assert_eq!(shell_quote(""), "''");
/// assert_eq!(shell_quote("main"), "'main'");
/// assert_eq!(shell_quote("feature/branch-1"), "'feature/branch-1'");
/// assert_eq!(shell_quote("it's dangerous"), "'it'\\''s dangerous'");
/// assert_eq!(shell_quote("; rm -rf ~"), "'; rm -rf ~'");
/// assert_eq!(shell_quote("$(reboot)"), "'$(reboot)'");
/// ```
pub fn shell_quote(input: &str) -> String {
    let clean = crate::ansi::strip_control_chars(input);
    if clean.is_empty() {
        return "''".to_string();
    }

    let cap = clean
        .len()
        .saturating_add(2)
        .saturating_add(clean.matches('\'').count().saturating_mul(3));
    let mut quoted = String::with_capacity(cap);
    quoted.push('\'');
    for ch in clean.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// Interpolates `%(name)` macro tokens in a command string with shell-quoted values.
///
/// Each macro of the form `%(<key>)` in `template` is replaced with a context-aware
/// shell-quoted value (`shell_quote(value)`). If a macro key is not found in `macros`, an error
/// is returned to prevent incomplete or malformed command execution.
///
/// # Errors
///
/// Returns [`TigError::Command`] if an unrecognized macro token is encountered.
pub fn interpolate_command<M: MacroLookup + ?Sized>(template: &str, macros: &M) -> Result<String> {
    let mut result = String::with_capacity(template.len() + 64);
    let mut rest = template;
    let mut quote_state = QuoteState::Unquoted;

    while let Some(start_idx) = rest.find("%(") {
        let prefix = &rest[..start_idx];
        quote_state.advance(prefix);
        result.push_str(prefix);
        let after_start = &rest[start_idx + 2..];

        if let Some(end_idx) = find_matching_close_paren(after_start) {
            let key = &after_start[..end_idx];
            if let Some(val) = macros.lookup(key) {
                append_quoted_in_context(&mut result, &val, quote_state);
            } else {
                return Err(TigError::Command(format!(
                    "Unknown macro '%({key})' in command template: '{template}'"
                )));
            }
            rest = &after_start[end_idx + 1..];
        } else {
            // Unclosed '%(', treat remainder as literal
            result.push_str(&rest[start_idx..]);
            rest = "";
            break;
        }
    }

    result.push_str(rest);
    Ok(result)
}

/// Verifies that an argument contains only safe POSIX shell literal characters.
///
/// Returns `Ok(())` if the argument is non-empty and consists only of ASCII
/// alphanumeric characters, `_`, `-`, `.`, `/`, `@`, `:`.
///
/// Returns an error if any shell metacharacters (such as `;`, `&`, `|`, `$`, `` ` ``,
/// `(`, `)`, `<`, `>`, newlines, carriage returns, quotes, or wildcards) are present.
pub fn verify_shell_argument_safety(arg: &str) -> Result<()> {
    if arg.is_empty() {
        return Err(TigError::Command(
            "Empty argument is not a valid shell token".to_string(),
        ));
    }
    for b in arg.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'/' | b'@' | b':' => {}
            _ => {
                return Err(TigError::Command(format!(
                    "Argument contains unsafe shell character: {:?}",
                    b as char
                )));
            }
        }
    }
    Ok(())
}

fn verify_no_control_chars(cmd: &str) -> Result<()> {
    for &b in cmd.as_bytes() {
        // Reject dangerous ASCII control characters in commands
        if (b < 0x20 && b != b'\t' && b != b'\n') || b == 0x7f {
            return Err(TigError::Command(format!(
                "Command contains forbidden control character (byte 0x{b:02x})"
            )));
        }
    }
    for ch in cmd.chars() {
        if crate::ansi::is_dangerous_unicode_char(ch) {
            return Err(TigError::Command(format!(
                "Command contains forbidden Unicode control/BiDi character (U+{:04X})",
                ch as u32
            )));
        }
    }
    Ok(())
}

/// Verifies that a full command string is syntactically well-formed for POSIX shell execution.
///
/// Checks that:
/// - There are no unescaped control characters (`\0`, `\r`, `\x1b`, etc.).
/// - Single-quoted strings are properly closed.
/// - Double-quoted strings are properly closed.
/// - Backtick command substitutions (`` `...` ``) are properly closed and their unescaped
///   subshell commands are themselves syntactically valid.
/// - Trailing backslashes are not left dangling.
pub fn verify_shell_safety(cmd: &str) -> Result<()> {
    verify_no_control_chars(cmd)?;
    if !cmd.contains('`') && !cmd.contains("$(") {
        if shlex::split(cmd).is_none() {
            return Err(TigError::Command(
                "Command contains unbalanced quotes or trailing backslash".to_string(),
            ));
        }
        return Ok(());
    }

    // Decompose outer command and any unquoted or double-quoted `$()` or backtick subshells,
    // verifying both the outer shell tokens and each subshell recursively.
    let mut outer_normalized = String::with_capacity(cmd.len());
    let mut subshell_buf = String::new();
    let mut state = QuoteState::Unquoted;
    let mut escaped = false;
    for ch in cmd.chars() {
        if escaped {
            match state {
                QuoteState::Unquoted
                | QuoteState::UnquotedDollar
                | QuoteState::DoubleQuoted
                | QuoteState::DoubleQuotedDollar => {
                    if state == QuoteState::UnquotedDollar {
                        state = QuoteState::Unquoted;
                    } else if state == QuoteState::DoubleQuotedDollar {
                        state = QuoteState::DoubleQuoted;
                    }
                    outer_normalized.push('\\');
                    outer_normalized.push(ch);
                }
                QuoteState::CommandSub {
                    outer_double_quoted,
                    depth,
                    parent_double_mask,
                    in_backtick,
                    inner,
                    ..
                } => {
                    subshell_buf.push('\\');
                    subshell_buf.push(ch);
                    state = QuoteState::CommandSub {
                        outer_double_quoted,
                        depth,
                        parent_double_mask,
                        in_backtick,
                        inner,
                        inner_dollar: false,
                    };
                }
                QuoteState::Backtick(_) => {
                    if matches!(ch, '\\' | '$' | '`') {
                        subshell_buf.push(ch);
                    } else {
                        subshell_buf.push('\\');
                        subshell_buf.push(ch);
                    }
                }
                QuoteState::DoubleQuotedBacktick(_) => {
                    if matches!(ch, '\\' | '$' | '`' | '"') {
                        subshell_buf.push(ch);
                    } else {
                        subshell_buf.push('\\');
                        subshell_buf.push(ch);
                    }
                }
                QuoteState::SingleQuoted => unreachable!(),
            }
            escaped = false;
            continue;
        }

        match state {
            QuoteState::Unquoted => match ch {
                '\\' => escaped = true,
                '\'' => {
                    state = QuoteState::SingleQuoted;
                    outer_normalized.push(ch);
                }
                '"' => {
                    state = QuoteState::DoubleQuoted;
                    outer_normalized.push(ch);
                }
                '$' => {
                    state = QuoteState::UnquotedDollar;
                }
                '`' => {
                    state = QuoteState::Backtick(InnerQuote::Unquoted);
                    subshell_buf.clear();
                    outer_normalized.push_str("__bt__");
                }
                _ => outer_normalized.push(ch),
            },
            QuoteState::UnquotedDollar => match ch {
                '(' => {
                    state = QuoteState::CommandSub {
                        outer_double_quoted: false,
                        depth: 1,
                        parent_double_mask: 0,
                        in_backtick: None,
                        inner: InnerQuote::Unquoted,
                        inner_dollar: false,
                    };
                    subshell_buf.clear();
                    outer_normalized.push_str("__cs__");
                }
                '\\' => {
                    outer_normalized.push('$');
                    state = QuoteState::Unquoted;
                    escaped = true;
                }
                '\'' => {
                    outer_normalized.push('$');
                    state = QuoteState::SingleQuoted;
                    outer_normalized.push(ch);
                }
                '"' => {
                    outer_normalized.push('$');
                    state = QuoteState::DoubleQuoted;
                    outer_normalized.push(ch);
                }
                '$' => {
                    outer_normalized.push('$');
                    state = QuoteState::UnquotedDollar;
                }
                '`' => {
                    outer_normalized.push('$');
                    state = QuoteState::Backtick(InnerQuote::Unquoted);
                    subshell_buf.clear();
                    outer_normalized.push_str("__bt__");
                }
                _ => {
                    outer_normalized.push('$');
                    outer_normalized.push(ch);
                    state = QuoteState::Unquoted;
                }
            },
            QuoteState::SingleQuoted => {
                if ch == '\'' {
                    state = QuoteState::Unquoted;
                }
                outer_normalized.push(ch);
            }
            QuoteState::DoubleQuoted => match ch {
                '\\' => escaped = true,
                '"' => {
                    state = QuoteState::Unquoted;
                    outer_normalized.push(ch);
                }
                '$' => {
                    state = QuoteState::DoubleQuotedDollar;
                }
                '`' => {
                    state = QuoteState::DoubleQuotedBacktick(InnerQuote::Unquoted);
                    subshell_buf.clear();
                    outer_normalized.push_str("__bt__");
                }
                _ => outer_normalized.push(ch),
            },
            QuoteState::DoubleQuotedDollar => match ch {
                '(' => {
                    state = QuoteState::CommandSub {
                        outer_double_quoted: true,
                        depth: 1,
                        parent_double_mask: 0,
                        in_backtick: None,
                        inner: InnerQuote::Unquoted,
                        inner_dollar: false,
                    };
                    subshell_buf.clear();
                    outer_normalized.push_str("__cs__");
                }
                '\\' => {
                    outer_normalized.push('$');
                    state = QuoteState::DoubleQuoted;
                    escaped = true;
                }
                '"' => {
                    outer_normalized.push('$');
                    state = QuoteState::Unquoted;
                    outer_normalized.push(ch);
                }
                '$' => {
                    outer_normalized.push('$');
                    state = QuoteState::DoubleQuotedDollar;
                }
                '`' => {
                    outer_normalized.push('$');
                    state = QuoteState::DoubleQuotedBacktick(InnerQuote::Unquoted);
                    subshell_buf.clear();
                    outer_normalized.push_str("__bt__");
                }
                _ => {
                    outer_normalized.push('$');
                    outer_normalized.push(ch);
                    state = QuoteState::DoubleQuoted;
                }
            },
            QuoteState::CommandSub {
                outer_double_quoted,
                depth,
                parent_double_mask,
                in_backtick,
                inner,
                inner_dollar,
            } => {
                if in_backtick.is_none() && inner == InnerQuote::Unquoted && ch == ')' && depth <= 1
                {
                    if subshell_buf.contains("\"'$(") || subshell_buf.contains("\"'`") {
                        return Err(TigError::Command(
                            "Subshell contains unsafe double-quoted command substitution"
                                .to_string(),
                        ));
                    }
                    verify_shell_safety(&subshell_buf)?;
                    state = if outer_double_quoted {
                        QuoteState::DoubleQuoted
                    } else {
                        QuoteState::Unquoted
                    };
                } else {
                    subshell_buf.push(ch);
                    let mut next_state = QuoteState::CommandSub {
                        outer_double_quoted,
                        depth,
                        parent_double_mask,
                        in_backtick,
                        inner,
                        inner_dollar,
                    };
                    let mut tmp = [0u8; 4];
                    next_state.advance(ch.encode_utf8(&mut tmp));
                    state = next_state;
                }
            }
            QuoteState::Backtick(inner) => match ch {
                '\\' => escaped = true,
                '`' => {
                    if shlex::split(&subshell_buf).is_none() {
                        return Err(TigError::Command(
                            "Backtick subshell contains unbalanced quotes or trailing backslash"
                                .to_string(),
                        ));
                    }
                    state = QuoteState::Unquoted;
                }
                _ => {
                    subshell_buf.push(ch);
                    let mut next_state = QuoteState::Backtick(inner);
                    let mut tmp = [0u8; 4];
                    next_state.advance(ch.encode_utf8(&mut tmp));
                    state = next_state;
                }
            },
            QuoteState::DoubleQuotedBacktick(inner) => match ch {
                '\\' => escaped = true,
                '`' => {
                    if shlex::split(&subshell_buf).is_none() {
                        return Err(TigError::Command(
                            "Double-quoted backtick subshell contains unbalanced quotes or trailing backslash"
                                .to_string(),
                        ));
                    }
                    state = QuoteState::DoubleQuoted;
                }
                '"' => {
                    return Err(TigError::Command(
                        "Unescaped double quote inside double-quoted backtick subshell".to_string(),
                    ));
                }
                _ => {
                    subshell_buf.push(ch);
                    let mut next_state = QuoteState::DoubleQuotedBacktick(inner);
                    let mut tmp = [0u8; 4];
                    next_state.advance(ch.encode_utf8(&mut tmp));
                    state = next_state;
                }
            },
        }
    }

    if state == QuoteState::UnquotedDollar {
        outer_normalized.push('$');
        state = QuoteState::Unquoted;
    }

    if escaped || state != QuoteState::Unquoted || shlex::split(&outer_normalized).is_none() {
        return Err(TigError::Command(
            "Command contains unbalanced quotes, unclosed subshell, or trailing backslash"
                .to_string(),
        ));
    }

    Ok(())
}

/// Splits a command string into individual shell arguments according to POSIX shell word splitting rules.
///
/// Rejects unescaped control characters and uses [`shlex::split`] for standard POSIX
/// word tokenization and quote stripping in a single pass.
///
/// # Errors
///
/// Returns [`TigError::Command`] if the command contains unclosed quotes, dangling backslashes,
/// or forbidden control characters.
pub fn split_shell_words(cmd: &str) -> Result<Vec<String>> {
    verify_no_control_chars(cmd)?;
    shlex::split(cmd).ok_or_else(|| {
        TigError::Command(format!(
            "Failed to parse shell command into words (syntax error or unclosed quote): '{cmd}'"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_quote_empty() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn test_shell_quote_alphanumeric() {
        assert_eq!(shell_quote("master"), "'master'");
        assert_eq!(shell_quote("HEAD~1"), "'HEAD~1'");
        assert_eq!(shell_quote("abc1234567890"), "'abc1234567890'");
    }

    #[test]
    fn test_quote_state_machine_and_subshell_backtick_execution() {
        let payload = "a'b\"c$(id)`id`$VAR\\end";
        let mut map: HashMap<&str, &str> = HashMap::new();
        map.insert("branch", payload);

        let templates = [
            "printf '%s' %(branch)",
            "printf '%s' $%(branch)",
            "printf '%s' \"%(branch)\"",
            "printf '%s' \"$%(branch)\"",
            "printf '%s' '%(branch)'",
            "printf '%s' $(printf '%s' %(branch))",
            "printf '%s' $(printf '%s' $%(branch))",
            "printf '%s' $(printf '%s' '%(branch)')",
            "printf '%s' $(printf '%s' \"%(branch)\")",
            "printf '%s' \"$(printf '%s' \"$(printf '%s' %(branch))\")\"",
            "printf '%s' `printf '%s' %(branch)`",
            "printf '%s' `printf '%s' '%(branch)'`",
            "printf '%s' `printf '%s' \"%(branch)\"`",
            "printf '%s' \"`printf '%s' %(branch)`\"",
            "printf '%s' \"`printf '%s' '%(branch)'`\"",
            "printf '%s' \"`printf '%s' \\\"%(branch)\\\"`\"",
            "printf '%s' $(printf '%s' `printf '%s' %(branch)`)",
            "printf '%s' $(printf '%s' \"`printf '%s' %(branch)`\")",
            "printf '%s' $(printf '%s' \"`printf '%s' '%(branch)'`\")",
            "printf '%s' $(printf '%s' \"`printf '%s' \\\"%(branch)\\\"`\")",
        ];

        for tmpl in templates {
            let expanded = interpolate_command(tmpl, &map)
                .unwrap_or_else(|e| panic!("interpolate failed for {tmpl}: {e}"));
            let out = std::process::Command::new("sh")
                .args(["-c", &expanded])
                .output()
                .expect("sh execution");
            assert!(
                out.status.success(),
                "sh failed for template `{tmpl}` -> `{expanded}`: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.ends_with(payload),
                "command injection or quote corruption in `{tmpl}` -> `{expanded}`: got `{stdout}`, expected literal suffix `{payload}`"
            );
        }
    }

    #[test]
    fn test_quote_state_edge_transitions_and_verify_shell_safety_errors() {
        // Test Dollar transitions followed by \, ', ", $, `, and normal chars
        let mut st = QuoteState::Unquoted;
        st.advance("$\\a$'x'$\"y\"$$$`echo hi`$( (echo a) )");
        assert_eq!(st, QuoteState::Unquoted);

        let mut st_dq = QuoteState::Unquoted;
        st_dq.advance("\"$\\a$\"\"$$\"\"$`echo hi`\"");
        assert_eq!(st_dq, QuoteState::Unquoted);

        // Trailing backslash neutralization
        let mut buf = String::from("echo \\");
        append_quoted_in_context(&mut buf, "val", QuoteState::Unquoted);
        assert_eq!(buf, "echo \\\\'val'");

        // Error branches in verify_shell_safety
        assert!(verify_shell_safety("echo `unclosed 'quote`").is_err());
        assert!(verify_shell_safety("echo \"`unclosed 'quote`\"").is_err());
        assert!(verify_shell_safety("echo \"`unescaped \" quote`\"").is_err());
        assert!(verify_shell_safety("echo $(echo \"'$(id)\")").is_err());
        assert!(verify_shell_safety("echo $(echo unclosed").is_err());
        assert!(verify_shell_safety("echo $").is_ok());
    }

    #[test]
    fn test_shell_quote_spaces_and_tabs() {
        assert_eq!(shell_quote("feature branch"), "'feature branch'");
        assert_eq!(
            shell_quote("with\ttabs\tand\nnewlines"),
            "'with\ttabs\tand\nnewlines'"
        );
    }

    #[test]
    fn test_shell_quote_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("'"), "''\\'''");
        assert_eq!(shell_quote("''"), "''\\'''\\'''");
        assert_eq!(shell_quote("foo 'bar' baz"), "'foo '\\''bar'\\'' baz'");
    }

    #[test]
    fn test_shell_quote_injection_attacks() {
        // Semicolon chaining
        assert_eq!(shell_quote("; rm -rf ~"), "'; rm -rf ~'");
        // Command substitution with $()
        assert_eq!(shell_quote("$(echo pwned)"), "'$(echo pwned)'");
        // Command substitution with backticks
        assert_eq!(shell_quote("`id`"), "'`id`'");
        // Variable expansion
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        // Piping and redirection
        assert_eq!(
            shell_quote("| cat /etc/passwd > /tmp/pwn"),
            "'| cat /etc/passwd > /tmp/pwn'"
        );
        // Ampersand backgrounding
        assert_eq!(shell_quote("& disown"), "'& disown'");
        // Logical operators
        assert_eq!(
            shell_quote("test && shutdown -h now"),
            "'test && shutdown -h now'"
        );
    }

    #[test]
    fn test_interpolate_command_basic() {
        let mut macros = HashMap::new();
        macros.insert("commit", "a1b2c3d4e5f6");
        macros.insert("file", "src/main.rs");

        let cmd = "git show %(commit) -- %(file)";
        let interpolated = interpolate_command(cmd, &macros).expect("interpolation failed");
        assert_eq!(interpolated, "git show 'a1b2c3d4e5f6' -- 'src/main.rs'");
    }

    #[test]
    fn test_interpolate_command_hostile_values() {
        let mut macros = HashMap::new();
        macros.insert("branch", "fix; reboot; #");
        macros.insert("commit", "$(touch /tmp/pwned)");

        let cmd = "git checkout %(branch) && git log %(commit)";
        let interpolated = interpolate_command(cmd, &macros).expect("interpolation failed");
        assert_eq!(
            interpolated,
            "git checkout 'fix; reboot; #' && git log '$(touch /tmp/pwned)'"
        );
    }

    #[test]
    fn test_interpolate_command_unknown_macro() {
        let macros: HashMap<&str, &str> = HashMap::new();
        let cmd = "git log %(unknown)";
        let err = interpolate_command(cmd, &macros).unwrap_err();
        assert!(err.to_string().contains("Unknown macro '%(unknown)'"));
    }

    #[test]
    fn test_interpolate_command_unclosed_macro() {
        let macros: HashMap<&str, &str> = HashMap::new();
        let cmd = "git log %(incomplete";
        let interpolated = interpolate_command(cmd, &macros).expect("should handle unclosed");
        assert_eq!(interpolated, "git log %(incomplete");
    }

    #[test]
    fn test_verify_shell_argument_safety() {
        assert!(verify_shell_argument_safety("master").is_ok());
        assert!(verify_shell_argument_safety("origin/main").is_ok());
        assert!(verify_shell_argument_safety("feature_branch-1.0@user:test").is_ok());

        assert!(verify_shell_argument_safety("").is_err());
        assert!(verify_shell_argument_safety("branch; rm -rf").is_err());
        assert!(verify_shell_argument_safety("$(reboot)").is_err());
        assert!(verify_shell_argument_safety("foo | bar").is_err());
        assert!(verify_shell_argument_safety("it's").is_err());
    }

    #[test]
    fn test_verify_shell_safety() {
        assert!(verify_shell_safety("git log 'main'").is_ok());
        assert!(verify_shell_safety("git checkout -b \"feature-x\"").is_ok());
        assert!(verify_shell_safety("echo 'it'\\''s fine'").is_ok());
        assert!(verify_shell_safety("git log --grep=\"foo \\\"bar\\\"\"").is_ok());

        // Unclosed quotes
        assert!(verify_shell_safety("git log 'unclosed").is_err());
        assert!(verify_shell_safety("git log \"unclosed").is_err());

        // Dangling backslash
        assert!(verify_shell_safety("git log \\").is_err());

        // Control characters
        assert!(verify_shell_safety("git log \0payload").is_err());
        assert!(verify_shell_safety("git log \r\n").is_err());
        assert!(verify_shell_safety("git log \x1b[2J").is_err());
    }

    #[test]
    fn test_interpolate_command_macro_lookup_closure() {
        let lookup = |key: &str| -> Option<Cow<'static, str>> {
            match key {
                "branch" => Some(Cow::Borrowed("feature/zero-alloc")),
                "commit" => Some(Cow::Owned("1234567".to_string())),
                _ => None,
            }
        };
        let cmd = "git checkout %(branch) && git reset --hard %(commit)";
        let res = interpolate_command(cmd, &lookup).expect("closure lookup should succeed");
        assert_eq!(
            res,
            "git checkout 'feature/zero-alloc' && git reset --hard '1234567'"
        );
    }

    #[test]
    fn test_interpolate_command_string_hashmap() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("file".to_string(), "crates/core.rs".to_string());
        let res = interpolate_command("cat %(file)", &map).expect("hashmap string lookup");
        assert_eq!(res, "cat 'crates/core.rs'");

        let mut map2: HashMap<&str, String> = HashMap::new();
        map2.insert("commit", "abc1234".to_string());
        let res2 =
            interpolate_command("git show %(commit)", &map2).expect("hashmap &str string lookup");
        assert_eq!(res2, "git show 'abc1234'");
    }

    #[test]
    fn test_split_shell_words() {
        // Plain arguments
        let words = split_shell_words("git log --oneline -n 10").unwrap();
        assert_eq!(words, vec!["git", "log", "--oneline", "-n", "10"]);

        // Single quoted arguments with spaces
        let words = split_shell_words("git commit -m 'Initial commit for project'").unwrap();
        assert_eq!(
            words,
            vec!["git", "commit", "-m", "Initial commit for project"]
        );

        // Double quoted arguments with spaces and escaped quotes
        let words = split_shell_words("git log --grep=\"feature release\"").unwrap();
        assert_eq!(words, vec!["git", "log", "--grep=feature release"]);

        // Mixed quotes
        let words = split_shell_words("cmd 'single' \"double\" unquoted").unwrap();
        assert_eq!(words, vec!["cmd", "single", "double", "unquoted"]);

        // Empty string
        let words = split_shell_words("").unwrap();
        assert_eq!(words, Vec::<String>::new());

        // Error cases: unclosed single quote
        assert!(split_shell_words("git log 'unclosed").is_err());

        // Error cases: unclosed double quote
        assert!(split_shell_words("git log \"unclosed").is_err());

        // Error cases: trailing backslash
        assert!(split_shell_words("git log \\").is_err());

        // Error cases: control character injection
        assert!(split_shell_words("git log \x1b[2J").is_err());
        assert!(split_shell_words("git log \0injection").is_err());
    }

    #[test]
    fn test_interpolate_command_quoted_context_prevents_injection() {
        let mut map: HashMap<&str, &str> = HashMap::new();
        map.insert("branch", "$(echo INJECTED)");
        map.insert("prompt", "foo; echo INJECTED; 'bar'");

        // 1. Macro inside double quotes
        let double_quoted_cmd =
            interpolate_command("printf '%s' \"Branch: %(branch)!\"", &map).unwrap();
        assert_eq!(
            double_quoted_cmd,
            "printf '%s' \"Branch: \"'$(echo INJECTED)'\"!\""
        );
        let output = std::process::Command::new("sh")
            .args(["-c", &double_quoted_cmd])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "Branch: $(echo INJECTED)!"
        );

        // 2. Macro inside single quotes
        let single_quoted_cmd =
            interpolate_command("printf '%s' 'Prompt: %(prompt)!'", &map).unwrap();
        let output_sq = std::process::Command::new("sh")
            .args(["-c", &single_quoted_cmd])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output_sq.stdout),
            "Prompt: foo; echo INJECTED; 'bar'!"
        );
    }
}
