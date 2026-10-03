// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Layout builder converting a `CommitDiff` into a structured `DiffDocument`.
//!
//! Handles unified and side-by-side row generation, diff-so-fancy style headers,
//! hunk function contexts, context expansion, and intra-line word-diff pairing.

use std::sync::Arc;
use tigrs_core::ansi::truncate_display_width_ellipsis;
use tigrs_git::{
    CommitDiff, DiffHunk, DiffLineKind, FileChangeStatus, FileDiff, ObjectId, word_diff,
};

use crate::options::{DiffLayout, DiffPresentation, ViewOptions};
use crate::term_cap::TerminalCapabilities;

use super::align::align_runs;
use super::document::{DiffDocument, DiffLineType, HunkLocation, LineMarker, RowCell, RowPair};
use super::expand::{ExpandedItem, ExpandedLine, expand_file_items_with_hunk_context};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Expands tab characters in `s` to spaces using `tab_size` and Unicode display column width.
/// Skips ANSI CSI escape sequences (`\x1b[...`) when calculating column width.
#[must_use]
pub fn expand_tabs(s: &str, tab_size: usize) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let tab_size = if tab_size == 0 { 8 } else { tab_size };
    let mut out = String::with_capacity(s.len() + 16);
    let mut col = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            out.push(c);
            if let Some(bracket) = chars.next() {
                out.push(bracket);
            }
            for esc_c in chars.by_ref() {
                out.push(esc_c);
                if ('\x40'..='\x7e').contains(&esc_c) {
                    break;
                }
            }
        } else if c == '\t' {
            let spaces = tab_size - (col % tab_size);
            for _ in 0..spaces {
                out.push(' ');
            }
            col += spaces;
        } else {
            out.push(c);
            col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        }
    }
    out
}

/// Provider function mapping an object ID to loaded line slices.
pub type BlobLineProvider<'a> = &'a dyn Fn(ObjectId) -> Option<Arc<[Arc<str>]>>;

/// Builds a `DiffDocument` from a `CommitDiff` according to the provided `ViewOptions`.
#[must_use]
pub fn build_diff_document(
    diff: &CommitDiff,
    opts: &ViewOptions,
    caps: &TerminalCapabilities,
    blob_provider: Option<BlobLineProvider>,
) -> DiffDocument {
    build_diff_document_cancellable(diff, opts, caps, blob_provider, None).unwrap_or_default()
}

/// Builds a `DiffDocument` from a `CommitDiff` with explicit per-file folding and per-hunk
/// extra context state.
#[must_use]
pub fn build_diff_document_with_state(
    diff: &CommitDiff,
    opts: &ViewOptions,
    caps: &TerminalCapabilities,
    blob_provider: Option<BlobLineProvider>,
    folded_files: &BTreeSet<usize>,
    hunk_extra_context: &BTreeMap<(usize, usize), usize>,
) -> DiffDocument {
    build_diff_document_cancellable_with_state(
        diff,
        opts,
        caps,
        blob_provider,
        folded_files,
        hunk_extra_context,
        None,
    )
    .unwrap_or_default()
}

/// Builds a `DiffDocument` from a `CommitDiff` with cooperative cancellation support.
///
/// Returns `None` if `cancel` is triggered before completion.
#[must_use]
pub fn build_diff_document_cancellable(
    diff: &CommitDiff,
    opts: &ViewOptions,
    caps: &TerminalCapabilities,
    blob_provider: Option<BlobLineProvider>,
    cancel: Option<&tigrs_core::CancellationToken>,
) -> Option<DiffDocument> {
    let default_folds = default_folded_files(diff, opts);
    let empty_hunk_ctx = BTreeMap::new();
    build_diff_document_cancellable_with_state(
        diff,
        opts,
        caps,
        blob_provider,
        &default_folds,
        &empty_hunk_ctx,
        cancel,
    )
}

/// Computes the initial set of folded file indices for `diff` under `opts`.
///
/// When `opts.diff_collapse_generated` is `true`, only files marked `linguist-generated`
/// in `.gitattributes` are auto-folded on open.
#[must_use]
pub fn default_folded_files(diff: &CommitDiff, opts: &ViewOptions) -> BTreeSet<usize> {
    let mut folds = BTreeSet::new();
    if opts.diff_collapse_generated {
        for (idx, file) in diff.files.iter().enumerate() {
            if is_file_linguist_generated(&file.path, diff) {
                folds.insert(idx);
            }
        }
    }
    folds
}

/// Returns `true` if `file_path` is marked `linguist-generated` via `.gitattributes`
/// (either recorded by the `gix` attribute stack or present in a `.gitattributes` diff in `diff`).
#[must_use]
pub fn is_file_linguist_generated(file_path: &str, diff: &CommitDiff) -> bool {
    if tigrs_git::is_path_marked_linguist_generated(file_path) {
        return true;
    }
    for attr_file in &diff.files {
        if attr_file.path == ".gitattributes" || attr_file.path.ends_with("/.gitattributes") {
            for hunk in &attr_file.hunks {
                for line in &hunk.lines {
                    if matches!(
                        line.kind,
                        tigrs_git::DiffLineKind::Add | tigrs_git::DiffLineKind::Context
                    ) && gitattributes_line_marks_generated(&line.content, file_path)
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn gitattributes_line_marks_generated(line: &str, file_path: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }
    let mut parts = trimmed.split_whitespace();
    let Some(pattern) = parts.next() else {
        return false;
    };
    if !simple_gitattr_glob_matches(pattern, file_path) {
        return false;
    }
    let mut generated = false;
    for attr in parts {
        if attr == "linguist-generated"
            || attr == "linguist-generated=true"
            || attr == "linguist-generated=1"
        {
            generated = true;
        } else if attr == "-linguist-generated"
            || attr == "!linguist-generated"
            || attr == "linguist-generated=false"
        {
            generated = false;
        }
    }
    generated
}

fn simple_gitattr_glob_matches(pattern: &str, file_path: &str) -> bool {
    let pat = pattern.trim_start_matches('/');
    if pat == file_path {
        return true;
    }
    let file_name = file_path.rsplit('/').next().unwrap_or(file_path);
    if pat == file_name {
        return true;
    }
    if let Some(suffix) = pat.strip_prefix('*')
        && (file_path.ends_with(suffix) || file_name.ends_with(suffix))
    {
        return true;
    }
    if let Some(prefix) = pat.strip_suffix("/**")
        && file_path.starts_with(prefix)
    {
        return true;
    }
    if let Some(prefix) = pat.strip_suffix("/*")
        && file_path.starts_with(prefix)
    {
        return true;
    }
    false
}

/// Formats a rename or copy path pair using Git/Gerrit-style brace diffing
/// (e.g. `sound/soc/amd/acp/acp-sdw-{mach → sof-mach}.c`).
#[must_use]
pub fn format_rename_brace_diff(old_path: &str, new_path: &str, ascii: bool) -> String {
    let arrow = if ascii { " -> " } else { " → " };
    if old_path == new_path {
        return new_path.to_string();
    }
    let old_bytes = old_path.as_bytes();
    let new_bytes = new_path.as_bytes();
    let min_len = old_bytes.len().min(new_bytes.len());

    let mut prefix_len = 0;
    while prefix_len < min_len && old_bytes[prefix_len] == new_bytes[prefix_len] {
        prefix_len += 1;
    }
    while prefix_len > 0 && !old_path.is_char_boundary(prefix_len) {
        prefix_len -= 1;
    }
    // Snap prefix back to the nearest delimiter ('/', '-', '_', '.') so tokens stay whole.
    while prefix_len > 0 {
        let prev_b = old_bytes[prefix_len - 1];
        if matches!(prev_b, b'/' | b'-' | b'_' | b'.') {
            break;
        }
        prefix_len -= 1;
    }

    let mut suffix_len = 0;
    while suffix_len < (min_len - prefix_len)
        && old_bytes[old_bytes.len() - 1 - suffix_len]
            == new_bytes[new_bytes.len() - 1 - suffix_len]
    {
        suffix_len += 1;
    }
    while suffix_len > 0
        && (!old_path.is_char_boundary(old_bytes.len() - suffix_len)
            || !new_path.is_char_boundary(new_bytes.len() - suffix_len))
    {
        suffix_len -= 1;
    }
    while suffix_len > 0 {
        let next_b = old_bytes[old_bytes.len() - suffix_len];
        if matches!(next_b, b'/' | b'-' | b'_' | b'.') {
            break;
        }
        suffix_len -= 1;
    }

    if prefix_len == 0 && suffix_len == 0 {
        return format!("{old_path}{arrow}{new_path}");
    }
    let prefix = &old_path[..prefix_len];
    let suffix = &old_path[old_bytes.len() - suffix_len..];
    let old_mid = &old_path[prefix_len..old_bytes.len() - suffix_len];
    let new_mid = &new_path[prefix_len..new_bytes.len() - suffix_len];
    format!("{prefix}{{{old_mid}{arrow}{new_mid}}}{suffix}")
}

/// Middle-elides deep file paths so the leading directory and full filename remain readable.
#[must_use]
pub fn elide_middle_path(path: &str, max_width: usize, ascii: bool) -> String {
    if path.chars().count() <= max_width {
        return path.to_string();
    }
    let ellipsis = if ascii { "..." } else { "…" };
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() <= 2 {
        return path.to_string();
    }
    let first = segments[0];
    let last = segments[segments.len() - 1];
    let second_last = segments[segments.len() - 2];
    let candidate_two_tail = format!("{first}/{ellipsis}/{second_last}/{last}");
    if candidate_two_tail.chars().count() <= max_width {
        return candidate_two_tail;
    }
    format!("{first}/{ellipsis}/{last}")
}

/// Formats a compact 3-cell change-magnitude sparkline (`▏▁▏`..`▏█▉▏` in UTF-8, `[.  ]`..`[###]` in ASCII).
#[must_use]
pub const fn format_magnitude_sparkline(
    additions: usize,
    deletions: usize,
    ascii: bool,
) -> &'static str {
    let total = additions.saturating_add(deletions);
    if ascii {
        if total == 0 {
            "[   ]"
        } else if total <= 5 {
            "[.  ]"
        } else if total <= 25 {
            "[#  ]"
        } else if total <= 100 {
            "[## ]"
        } else {
            "[###]"
        }
    } else if total == 0 {
        "▏ ▏"
    } else if total <= 5 {
        "▏▁▏"
    } else if total <= 25 {
        "▏▃▏"
    } else if total <= 100 {
        "▏▅▏"
    } else if total <= 300 {
        "▏▇▏"
    } else {
        "▏█▉▏"
    }
}

/// Detects invisible or non-content changes (mode changes, CRLF/LF flips, whitespace-only edits).
#[must_use]
pub fn detect_invisible_change_badges(file: &tigrs_git::FileDiff, ascii: bool) -> Vec<String> {
    let mut badges = Vec::new();
    let arrow = if ascii { "->" } else { "→" };
    if let (Some(old_m), Some(new_m)) = (file.old_mode, file.new_mode)
        && old_m != new_m
    {
        if file.hunks.is_empty() && !file.is_binary {
            let dot = if ascii { "|" } else { "·" };
            badges.push(format!(
                "mode {old_m:06o} {arrow} {new_m:06o} {dot} no content change"
            ));
        } else {
            badges.push(format!("mode {old_m:06o} {arrow} {new_m:06o}"));
        }
    }

    if !file.hunks.is_empty() && (file.additions > 0 || file.deletions > 0) {
        let mut removed_lines = Vec::new();
        let mut added_lines = Vec::new();
        for hunk in &file.hunks {
            for line in &hunk.lines {
                match line.kind {
                    tigrs_git::DiffLineKind::Remove => removed_lines.push(line.content.as_str()),
                    tigrs_git::DiffLineKind::Add => added_lines.push(line.content.as_str()),
                    tigrs_git::DiffLineKind::Context => {}
                }
            }
        }
        if !removed_lines.is_empty() && removed_lines.len() == added_lines.len() {
            let rem_has_cr = removed_lines.iter().any(|l| l.ends_with('\r'));
            let add_has_cr = added_lines.iter().any(|l| l.ends_with('\r'));
            let cr_stripped_eq = removed_lines
                .iter()
                .zip(added_lines.iter())
                .all(|(r, a)| r.trim_end_matches('\r') == a.trim_end_matches('\r'));
            if cr_stripped_eq && rem_has_cr != add_has_cr {
                if rem_has_cr {
                    badges.push(format!("line endings CRLF {arrow} LF"));
                } else {
                    badges.push(format!("line endings LF {arrow} CRLF"));
                }
            } else {
                let ws_eq = removed_lines.iter().zip(added_lines.iter()).all(|(r, a)| {
                    r.chars()
                        .filter(|c| !c.is_whitespace())
                        .eq(a.chars().filter(|c| !c.is_whitespace()))
                });
                if ws_eq {
                    badges.push("whitespace only".to_string());
                }
            }
        }
    }
    badges
}

/// Infers a concise programming language badge from `path`.
#[must_use]
pub fn detect_file_language_badge(path: &str) -> Option<&'static str> {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    match file_name {
        "Makefile" | "GNUmakefile" => return Some("Make"),
        "Dockerfile" => return Some("Docker"),
        "Cargo.toml" => return Some("TOML"),
        _ => {}
    }
    let ext = file_name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "c" | "h" => Some("C"),
        "rs" => Some("Rust"),
        "py" => Some("Python"),
        "go" => Some("Go"),
        "js" | "mjs" | "cjs" => Some("JS"),
        "ts" | "tsx" => Some("TS"),
        "jsx" => Some("JSX"),
        "java" => Some("Java"),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => Some("C++"),
        "sh" | "bash" | "zsh" => Some("Shell"),
        "rb" => Some("Ruby"),
        "php" => Some("PHP"),
        "swift" => Some("Swift"),
        "kt" | "kts" => Some("Kotlin"),
        "scala" => Some("Scala"),
        "sql" => Some("SQL"),
        "toml" => Some("TOML"),
        "yaml" | "yml" => Some("YAML"),
        "json" => Some("JSON"),
        "md" | "markdown" => Some("Markdown"),
        "html" | "htm" => Some("HTML"),
        "css" | "scss" => Some("CSS"),
        "proto" => Some("Proto"),
        "zig" => Some("Zig"),
        "lua" => Some("Lua"),
        _ => None,
    }
}

/// Formats an enclosing function/symbol context string cleanly for hunk separators and sticky headers.
#[must_use]
pub fn format_enclosing_symbol(func_context: &str) -> String {
    let trimmed = func_context.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.chars().count() <= 48 {
        return trimmed.to_string();
    }
    if let Some((before_paren, _)) = trimmed.split_once('(') {
        let ident = before_paren
            .split_whitespace()
            .last()
            .unwrap_or(before_paren)
            .trim_start_matches('*')
            .trim_start_matches('&');
        if !ident.is_empty() {
            return format!("{ident}()");
        }
    }
    trimmed.chars().take(48).collect()
}

/// Formats the canonical Git `diff --git` multi-line header block for copy fidelity (`y`).
#[must_use]
pub fn format_real_file_header(file: &tigrs_git::FileDiff) -> String {
    let (a_path, b_path) = match &file.status {
        FileChangeStatus::Added => ("/dev/null".to_string(), format!("b/{}", file.path)),
        FileChangeStatus::Deleted => (format!("a/{}", file.path), "/dev/null".to_string()),
        FileChangeStatus::Renamed { source_path, .. }
        | FileChangeStatus::Copied { source_path, .. } => {
            (format!("a/{source_path}"), format!("b/{}", file.path))
        }
        _ => (format!("a/{}", file.path), format!("b/{}", file.path)),
    };
    let mut lines = vec![format!("diff --git {a_path} {b_path}")];
    match &file.status {
        FileChangeStatus::Added => {
            if let Some(mode) = file.new_mode {
                lines.push(format!("new file mode {mode:06o}"));
            }
        }
        FileChangeStatus::Deleted => {
            if let Some(mode) = file.old_mode {
                lines.push(format!("deleted file mode {mode:06o}"));
            }
        }
        FileChangeStatus::Renamed {
            source_path,
            similarity_pct,
        } => {
            lines.push(format!("similarity index {similarity_pct}%"));
            lines.push(format!("rename from {source_path}"));
            lines.push(format!("rename to {}", file.path));
        }
        FileChangeStatus::Copied {
            source_path,
            similarity_pct,
        } => {
            lines.push(format!("similarity index {similarity_pct}%"));
            lines.push(format!("copy from {source_path}"));
            lines.push(format!("copy to {}", file.path));
        }
        _ => {
            if let (Some(old_m), Some(new_m)) = (file.old_mode, file.new_mode)
                && old_m != new_m
            {
                lines.push(format!("old mode {old_m:06o}"));
                lines.push(format!("new mode {new_m:06o}"));
            }
        }
    }
    if let (Some(old_id), Some(new_id)) = (&file.old_id, &file.new_id) {
        let old_hex = old_id.to_hex().to_string();
        let new_hex = new_id.to_hex().to_string();
        let old_short = &old_hex[..7.min(old_hex.len())];
        let new_short = &new_hex[..7.min(new_hex.len())];
        if let Some(mode) = file.new_mode.or(file.old_mode) {
            lines.push(format!("index {old_short}..{new_short} {mode:06o}"));
        } else {
            lines.push(format!("index {old_short}..{new_short}"));
        }
    }
    lines.push(format!("--- {a_path}"));
    lines.push(format!("+++ {b_path}"));
    lines.join("\n")
}

/// Formats the canonical Git `@@ -old,len +new,len @@ func` header for copy fidelity (`y`).
#[must_use]
pub fn format_real_hunk_header(hunk: &tigrs_git::DiffHunk) -> String {
    if let Some(ref func) = hunk.func_context {
        format!(
            "@@ -{},{} +{},{} @@ {}",
            hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len, func
        )
    } else {
        format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len
        )
    }
}

#[inline]
fn push_unified_row(
    cell: RowCell,
    file_idx: Option<u32>,
    hunk_loc: Option<HunkLocation>,
    row_type: DiffLineType,
    rows: &mut Vec<RowPair>,
    line_to_file: &mut Vec<Option<u32>>,
    line_to_hunk: &mut Vec<Option<HunkLocation>>,
) {
    rows.push(RowPair::unified(cell, hunk_loc, row_type));
    line_to_file.push(file_idx);
    line_to_hunk.push(hunk_loc);
}

fn append_commit_header_rows(
    diff: &CommitDiff,
    rows: &mut Vec<RowPair>,
    line_to_file: &mut Vec<Option<u32>>,
    line_to_hunk: &mut Vec<Option<HunkLocation>>,
) {
    push_unified_row(
        RowCell::new(LineMarker::None, format!("commit {}", diff.commit_id), None),
        None,
        None,
        DiffLineType::CommitHeader,
        rows,
        line_to_file,
        line_to_hunk,
    );

    if diff.parent_ids.len() > 1 {
        let parents_str = diff
            .parent_ids
            .iter()
            .map(|p| {
                let hex = p.to_hex().to_string();
                hex[..7.min(hex.len())].to_string()
            })
            .collect::<Vec<_>>()
            .join(" ");
        push_unified_row(
            RowCell::new(LineMarker::None, format!("Merge: {parents_str}"), None),
            None,
            None,
            DiffLineType::MergeHeader,
            rows,
            line_to_file,
            line_to_hunk,
        );
    }

    push_unified_row(
        RowCell::new(
            LineMarker::None,
            format!("Author:     {} <{}>", diff.author_name, diff.author_email),
            None,
        ),
        None,
        None,
        DiffLineType::AuthorHeader,
        rows,
        line_to_file,
        line_to_hunk,
    );
    push_unified_row(
        RowCell::new(
            LineMarker::None,
            format!("AuthorDate: {}", diff.author_date),
            None,
        ),
        None,
        None,
        DiffLineType::DateHeader,
        rows,
        line_to_file,
        line_to_hunk,
    );
    push_unified_row(
        RowCell::new(
            LineMarker::None,
            format!(
                "Commit:     {} <{}>",
                diff.committer_name, diff.committer_email
            ),
            None,
        ),
        None,
        None,
        DiffLineType::CommitterHeader,
        rows,
        line_to_file,
        line_to_hunk,
    );
    push_unified_row(
        RowCell::new(
            LineMarker::None,
            format!("CommitDate: {}", diff.committer_date),
            None,
        ),
        None,
        None,
        DiffLineType::DateHeader,
        rows,
        line_to_file,
        line_to_hunk,
    );

    push_unified_row(
        RowCell::new(LineMarker::None, String::new(), None),
        None,
        None,
        DiffLineType::Empty,
        rows,
        line_to_file,
        line_to_hunk,
    );
    push_unified_row(
        RowCell::new(LineMarker::None, format!("    {}", diff.title), None),
        None,
        None,
        DiffLineType::MessageTitle,
        rows,
        line_to_file,
        line_to_hunk,
    );

    if let Some(body) = &diff.body
        && !body.trim().is_empty()
    {
        push_unified_row(
            RowCell::new(LineMarker::None, String::new(), None),
            None,
            None,
            DiffLineType::Empty,
            rows,
            line_to_file,
            line_to_hunk,
        );
        let raw_lines: Vec<&str> = body.lines().collect();
        let clean_lines: Vec<&str> = raw_lines
            .iter()
            .map(|l| l.strip_prefix("    ").unwrap_or(l))
            .collect();
        let trailer_flags = classify_commit_body_lines(&clean_lines);
        for (body_line, (clean_line, is_trailer)) in raw_lines
            .into_iter()
            .zip(clean_lines.into_iter().zip(trailer_flags))
        {
            if body_line.trim().is_empty() {
                push_unified_row(
                    RowCell::new(LineMarker::None, String::new(), None),
                    None,
                    None,
                    DiffLineType::Empty,
                    rows,
                    line_to_file,
                    line_to_hunk,
                );
            } else {
                let row_type = if is_trailer {
                    DiffLineType::CommitTrailer
                } else {
                    DiffLineType::MessageBody
                };
                push_unified_row(
                    RowCell::new(LineMarker::None, format!("    {clean_line}"), None),
                    None,
                    None,
                    row_type,
                    rows,
                    line_to_file,
                    line_to_hunk,
                );
            }
        }
    }
    push_unified_row(
        RowCell::new(LineMarker::None, String::new(), None),
        None,
        None,
        DiffLineType::Empty,
        rows,
        line_to_file,
        line_to_hunk,
    );
}

fn append_diffstat_rows(
    diff: &CommitDiff,
    rows: &mut Vec<RowPair>,
    line_to_file: &mut Vec<Option<u32>>,
    line_to_hunk: &mut Vec<Option<HunkLocation>>,
) {
    push_unified_row(
        RowCell::new(LineMarker::None, "---".to_string(), None),
        None,
        None,
        DiffLineType::Delimiter,
        rows,
        line_to_file,
        line_to_hunk,
    );

    if diff.files.is_empty() {
        push_unified_row(
            RowCell::new(LineMarker::None, " 0 files changed".to_string(), None),
            None,
            None,
            DiffLineType::StatSummary,
            rows,
            line_to_file,
            line_to_hunk,
        );
    } else {
        let max_path_len = diff
            .files
            .iter()
            .map(|f| f.path.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(10, 48);

        let max_changes = diff
            .files
            .iter()
            .map(|f| f.additions + f.deletions)
            .max()
            .unwrap_or(0);

        let max_bar_width: usize = 20;

        for (f_idx, file) in diff.files.iter().enumerate() {
            let file_idx_u32 = u32::try_from(f_idx).ok();
            let total_changes = file.additions + file.deletions;
            let (plus_count, minus_count) = if max_changes > 0 {
                let plus = if file.additions > 0 {
                    ((file.additions * max_bar_width) / max_changes.max(max_bar_width)).max(1)
                } else {
                    0
                };
                let minus = if file.deletions > 0 {
                    ((file.deletions * max_bar_width) / max_changes.max(max_bar_width)).max(1)
                } else {
                    0
                };
                (plus, minus)
            } else {
                (0, 0)
            };

            let bar = format!("{}{}", "+".repeat(plus_count), "-".repeat(minus_count));
            let path_disp = truncate_display_width_ellipsis(&file.path, max_path_len);
            let stat_line = format!(" {path_disp:<max_path_len$} | {total_changes:>4} {bar}");
            push_unified_row(
                RowCell::new(LineMarker::None, stat_line, None),
                file_idx_u32,
                None,
                DiffLineType::StatFile,
                rows,
                line_to_file,
                line_to_hunk,
            );
        }

        let file_s = if diff.stats.files_changed == 1 {
            ""
        } else {
            "s"
        };
        let ins_s = if diff.stats.insertions == 1 { "" } else { "s" };
        let del_s = if diff.stats.deletions == 1 { "" } else { "s" };
        let summary_line = format!(
            " {} file{} changed, {} insertion{}(+), {} deletion{}(-)",
            diff.stats.files_changed,
            file_s,
            diff.stats.insertions,
            ins_s,
            diff.stats.deletions,
            del_s
        );
        push_unified_row(
            RowCell::new(LineMarker::None, summary_line, None),
            None,
            None,
            DiffLineType::StatSummary,
            rows,
            line_to_file,
            line_to_hunk,
        );
    }
    push_unified_row(
        RowCell::new(LineMarker::None, String::new(), None),
        None,
        None,
        DiffLineType::Empty,
        rows,
        line_to_file,
        line_to_hunk,
    );
}

#[allow(clippy::too_many_arguments)]
fn append_file_header_rows(
    f_idx: usize,
    file: &FileDiff,
    diff: &CommitDiff,
    opts: &ViewOptions,
    is_folded: bool,
    rows: &mut Vec<RowPair>,
    line_to_file: &mut Vec<Option<u32>>,
    line_to_hunk: &mut Vec<Option<HunkLocation>>,
) {
    let cur_f = u32::try_from(f_idx).ok();
    let (a_path, b_path) = match &file.status {
        FileChangeStatus::Added => ("/dev/null".to_string(), format!("b/{}", file.path)),
        FileChangeStatus::Deleted => (format!("a/{}", file.path), "/dev/null".to_string()),
        FileChangeStatus::Renamed { source_path, .. }
        | FileChangeStatus::Copied { source_path, .. } => {
            (format!("a/{source_path}"), format!("b/{}", file.path))
        }
        _ => (format!("a/{}", file.path), format!("b/{}", file.path)),
    };

    let fold_badge = if is_folded {
        let arrow = if opts.line_graphics == crate::options::LineGraphics::Ascii {
            ">"
        } else {
            "▸"
        };
        let hunk_s = if file.hunks.len() == 1 { "" } else { "s" };
        Some((
            arrow,
            format!(
                " [folded: +{} -{}, {} hunk{}]",
                file.additions,
                file.deletions,
                file.hunks.len(),
                hunk_s
            ),
        ))
    } else {
        None
    };

    let ascii = opts.line_graphics == crate::options::LineGraphics::Ascii;
    if opts.diff_presentation == DiffPresentation::Banner {
        let total_files = diff.files.len().max(1);
        let status_chip = match &file.status {
            FileChangeStatus::Modified => "M".to_string(),
            FileChangeStatus::Added => "A".to_string(),
            FileChangeStatus::Deleted => "D".to_string(),
            FileChangeStatus::Renamed { similarity_pct, .. } => format!("R{similarity_pct}"),
            FileChangeStatus::Copied { similarity_pct, .. } => format!("C{similarity_pct}"),
            FileChangeStatus::TypeChanged => "T".to_string(),
        };
        let display_path = match &file.status {
            FileChangeStatus::Renamed { source_path, .. }
            | FileChangeStatus::Copied { source_path, .. } => {
                format_rename_brace_diff(source_path, &file.path, ascii)
            }
            _ => elide_middle_path(&file.path, 52, ascii),
        };
        let stats_part = if file.is_binary {
            "(binary)".to_string()
        } else {
            let spark = format_magnitude_sparkline(file.additions, file.deletions, ascii);
            format!("+{} -{} {spark}", file.additions, file.deletions)
        };
        let mut badges = detect_invisible_change_badges(file, ascii);
        let is_generated = is_file_linguist_generated(&file.path, diff);
        if is_generated {
            badges.push("generated (.gitattributes)".to_string());
        }
        if is_folded {
            let hunk_s = if file.hunks.len() == 1 { "" } else { "s" };
            let dot = if ascii { "|" } else { "·" };
            if is_generated {
                badges.push(format!(
                    "[folded: +{} -{}, {} hunk{hunk_s} collapsed {dot} Enter to expand]",
                    file.additions,
                    file.deletions,
                    file.hunks.len()
                ));
            } else {
                badges.push(format!(
                    "[folded: +{} -{}, {} hunk{hunk_s}]",
                    file.additions,
                    file.deletions,
                    file.hunks.len()
                ));
            }
        }
        let sep = if ascii { " | " } else { " · " };
        let badge_str = if badges.is_empty() {
            String::new()
        } else {
            format!("{sep}{}", badges.join(sep))
        };
        let hints_str = if opts.diff_hints == tigrs_core::DiffHintsMode::Never {
            String::new()
        } else {
            "  za fold  e edit  i info  y copy".to_string()
        };
        let lead = if is_folded {
            if ascii { "> --" } else { "▸ ─" }
        } else if ascii {
            "--"
        } else {
            "─"
        };
        let mid_rule = if ascii { "--" } else { "─" };
        let fill_rule = if ascii { "----" } else { "────" };
        let banner_text = format!(
            "{lead} {}/{total_files} {mid_rule} {status_chip} {mid_rule} {display_path} {fill_rule} {stats_part}{badge_str} {mid_rule}{hints_str}",
            f_idx + 1
        );
        push_unified_row(
            RowCell::new(LineMarker::None, banner_text, None),
            cur_f,
            None,
            DiffLineType::FileHeader,
            rows,
            line_to_file,
            line_to_hunk,
        );
    } else if opts.diff_presentation == DiffPresentation::Fancy {
        push_unified_row(
            RowCell::new(LineMarker::None, "─".repeat(80), None),
            cur_f,
            None,
            DiffLineType::Delimiter,
            rows,
            line_to_file,
            line_to_hunk,
        );

        let status_text = match &file.status {
            FileChangeStatus::Added => format!("added: {}", file.path),
            FileChangeStatus::Deleted => format!("deleted: {}", file.path),
            FileChangeStatus::Renamed { source_path, .. } => {
                format!("renamed: {source_path} to {}", file.path)
            }
            FileChangeStatus::Copied { source_path, .. } => {
                format!("copied: {source_path} to {}", file.path)
            }
            FileChangeStatus::Modified => format!("modified: {}", file.path),
            FileChangeStatus::TypeChanged => format!("type changed: {}", file.path),
        };
        let status_text = if file.is_binary {
            format!("{status_text} (binary)")
        } else {
            status_text
        };
        let status_text = if let Some((arrow, ref badge)) = fold_badge {
            format!("{arrow} {status_text}{badge}")
        } else {
            status_text
        };

        push_unified_row(
            RowCell::new(LineMarker::None, status_text, None),
            cur_f,
            None,
            DiffLineType::FileHeader,
            rows,
            line_to_file,
            line_to_hunk,
        );

        push_unified_row(
            RowCell::new(LineMarker::None, "─".repeat(80), None),
            cur_f,
            None,
            DiffLineType::Delimiter,
            rows,
            line_to_file,
            line_to_hunk,
        );

        if !is_folded
            && let (Some(old_m), Some(new_m)) = (file.old_mode, file.new_mode)
            && old_m != new_m
        {
            let mode_text = format!(
                "{} changed file mode from {old_m:06o} to {new_m:06o}",
                file.path
            );
            push_unified_row(
                RowCell::new(LineMarker::None, mode_text, None),
                cur_f,
                None,
                DiffLineType::ModeChange,
                rows,
                line_to_file,
                line_to_hunk,
            );
        }
    } else {
        let header_text = if let Some((arrow, ref badge)) = fold_badge {
            format!("{arrow} diff --git {a_path} {b_path}{badge}")
        } else {
            format!("diff --git {a_path} {b_path}")
        };
        push_unified_row(
            RowCell::new(LineMarker::None, header_text, None),
            cur_f,
            None,
            DiffLineType::FileHeader,
            rows,
            line_to_file,
            line_to_hunk,
        );

        if is_folded {
            return;
        }

        match &file.status {
            FileChangeStatus::Added => {
                if let Some(mode) = file.new_mode {
                    push_unified_row(
                        RowCell::new(LineMarker::None, format!("new file mode {mode:06o}"), None),
                        cur_f,
                        None,
                        DiffLineType::FileHeader,
                        rows,
                        line_to_file,
                        line_to_hunk,
                    );
                }
            }
            FileChangeStatus::Deleted => {
                if let Some(mode) = file.old_mode {
                    push_unified_row(
                        RowCell::new(
                            LineMarker::None,
                            format!("deleted file mode {mode:06o}"),
                            None,
                        ),
                        cur_f,
                        None,
                        DiffLineType::FileHeader,
                        rows,
                        line_to_file,
                        line_to_hunk,
                    );
                }
            }
            FileChangeStatus::Renamed {
                source_path,
                similarity_pct,
            } => {
                push_unified_row(
                    RowCell::new(
                        LineMarker::None,
                        format!("similarity index {similarity_pct}%"),
                        None,
                    ),
                    cur_f,
                    None,
                    DiffLineType::FileHeader,
                    rows,
                    line_to_file,
                    line_to_hunk,
                );
                push_unified_row(
                    RowCell::new(LineMarker::None, format!("rename from {source_path}"), None),
                    cur_f,
                    None,
                    DiffLineType::FileHeader,
                    rows,
                    line_to_file,
                    line_to_hunk,
                );
                push_unified_row(
                    RowCell::new(LineMarker::None, format!("rename to {}", file.path), None),
                    cur_f,
                    None,
                    DiffLineType::FileHeader,
                    rows,
                    line_to_file,
                    line_to_hunk,
                );
            }
            FileChangeStatus::Copied {
                source_path,
                similarity_pct,
            } => {
                push_unified_row(
                    RowCell::new(
                        LineMarker::None,
                        format!("similarity index {similarity_pct}%"),
                        None,
                    ),
                    cur_f,
                    None,
                    DiffLineType::FileHeader,
                    rows,
                    line_to_file,
                    line_to_hunk,
                );
                push_unified_row(
                    RowCell::new(LineMarker::None, format!("copy from {source_path}"), None),
                    cur_f,
                    None,
                    DiffLineType::FileHeader,
                    rows,
                    line_to_file,
                    line_to_hunk,
                );
                push_unified_row(
                    RowCell::new(LineMarker::None, format!("copy to {}", file.path), None),
                    cur_f,
                    None,
                    DiffLineType::FileHeader,
                    rows,
                    line_to_file,
                    line_to_hunk,
                );
            }
            _ => {}
        }

        let old_hex = file
            .old_id
            .map_or_else(|| "00000000".to_string(), |id| id.to_hex().to_string());
        let new_hex = file
            .new_id
            .map_or_else(|| "00000000".to_string(), |id| id.to_hex().to_string());
        let mode_suffix = file
            .new_mode
            .or(file.old_mode)
            .map(|m| format!(" {m:06o}"))
            .unwrap_or_default();
        push_unified_row(
            RowCell::new(
                LineMarker::None,
                format!(
                    "index {}..{}{}",
                    &old_hex[..7.min(old_hex.len())],
                    &new_hex[..7.min(new_hex.len())],
                    mode_suffix
                ),
                None,
            ),
            cur_f,
            None,
            DiffLineType::FileHeader,
            rows,
            line_to_file,
            line_to_hunk,
        );

        push_unified_row(
            RowCell::new(LineMarker::None, format!("--- {a_path}"), None),
            cur_f,
            None,
            DiffLineType::FileOld,
            rows,
            line_to_file,
            line_to_hunk,
        );
        push_unified_row(
            RowCell::new(LineMarker::None, format!("+++ {b_path}"), None),
            cur_f,
            None,
            DiffLineType::FileNew,
            rows,
            line_to_file,
            line_to_hunk,
        );
    }
}

fn format_hunk_header_text(
    f_idx: usize,
    hunk_idx: usize,
    file: &FileDiff,
    hunk_extra_context: &BTreeMap<(usize, usize), usize>,
    opts: &ViewOptions,
) -> String {
    let ascii = opts.line_graphics == crate::options::LineGraphics::Ascii;
    let hunk = &file.hunks[hunk_idx];
    let hunk_extra = hunk_extra_context
        .get(&(f_idx, hunk_idx))
        .copied()
        .unwrap_or(0);
    let extra_badge = if hunk_extra > 0 {
        format!(" [+{hunk_extra} ctx]")
    } else {
        String::new()
    };
    if opts.diff_presentation == DiffPresentation::Banner {
        let prev_end = if hunk_idx == 0 {
            1u32
        } else {
            let prev = &file.hunks[hunk_idx - 1];
            prev.new_start + prev.new_len
        };
        let unchanged = hunk
            .new_start
            .saturating_sub(prev_end)
            .saturating_sub(u32::try_from(hunk_extra).unwrap_or(u32::MAX));
        let left_part = if unchanged == 0 {
            let end_line = hunk.new_start + hunk.new_len.saturating_sub(1);
            if ascii {
                format!("-- lines {}..{} --{extra_badge}", hunk.new_start, end_line)
            } else {
                format!("── lines {}..{} ──{extra_badge}", hunk.new_start, end_line)
            }
        } else {
            let expand_hint = if hunk_idx == 0 {
                "+ expand  ] all"
            } else {
                "+ expand"
            };
            let line_word = if unchanged == 1 { "line" } else { "lines" };
            if ascii {
                format!("... {unchanged} unchanged {line_word} ...   {expand_hint}{extra_badge}")
            } else {
                format!("⋯ {unchanged} unchanged {line_word} ⋯   {expand_hint}{extra_badge}")
            }
        };
        let lang = detect_file_language_badge(&file.path);
        let sym = hunk
            .func_context
            .as_deref()
            .map(format_enclosing_symbol)
            .filter(|s| !s.is_empty());
        let right_part = match (lang, sym) {
            (Some(l), Some(s)) => {
                if ascii {
                    format!("  | {l} > {s}")
                } else {
                    format!("  ┃ {l} ▸ {s}")
                }
            }
            (None, Some(s)) => {
                if ascii {
                    format!("  | > {s}")
                } else {
                    format!("  ┃ ▸ {s}")
                }
            }
            (Some(l), None) => {
                if ascii {
                    format!("  | {l}")
                } else {
                    format!("  ┃ {l}")
                }
            }
            (None, None) => String::new(),
        };
        format!("{left_part}{right_part}")
    } else if opts.diff_presentation == DiffPresentation::Fancy {
        let first_changed = find_first_changed_lineno(hunk);
        if let Some(ref func) = hunk.func_context {
            format!(
                "@ {}:{} @ {}{}",
                file.path, first_changed, func, extra_badge
            )
        } else {
            format!("@ {}:{} @{}", file.path, first_changed, extra_badge)
        }
    } else if let Some(ref func) = hunk.func_context {
        format!(
            "@@ -{},{} +{},{} @@ {}{}",
            hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len, func, extra_badge
        )
    } else {
        format!(
            "@@ -{},{} +{},{} @@{}",
            hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len, extra_badge
        )
    }
}

fn apply_moved_block_markers(diff: &CommitDiff, rows: &mut [RowPair]) {
    let moved_anchors = detect_moved_blocks(diff);
    if moved_anchors.is_empty() {
        return;
    }
    for row in rows {
        if row.right.is_none() {
            if let Some(HunkLocation {
                file_idx,
                hunk_idx,
                line_idx: Some(line_idx),
            }) = row.anchor
                && moved_anchors.contains(&(file_idx, hunk_idx, line_idx))
                && let Some(ref mut left) = row.left
            {
                left.is_moved = true;
            }
        } else {
            if let Some(HunkLocation {
                file_idx,
                hunk_idx,
                line_idx: Some(line_idx),
            }) = row.anchor
                && moved_anchors.contains(&(file_idx, hunk_idx, line_idx))
                && let Some(ref mut left) = row.left
                && left.marker == LineMarker::Del
            {
                left.is_moved = true;
            }
            let right_anchor = row.paired_anchor.or(row.anchor);
            if let Some(HunkLocation {
                file_idx,
                hunk_idx,
                line_idx: Some(line_idx),
            }) = right_anchor
                && moved_anchors.contains(&(file_idx, hunk_idx, line_idx))
                && let Some(ref mut right) = row.right
                && right.marker == LineMarker::Add
            {
                right.is_moved = true;
            }
        }
    }
}

/// Builds a `DiffDocument` from a `CommitDiff` with per-file folding, per-hunk context overrides,
/// and cooperative cancellation support.
#[must_use]
pub fn build_diff_document_cancellable_with_state(
    diff: &CommitDiff,
    opts: &ViewOptions,
    _caps: &TerminalCapabilities,
    blob_provider: Option<BlobLineProvider>,
    folded_files: &BTreeSet<usize>,
    hunk_extra_context: &BTreeMap<(usize, usize), usize>,
    cancel: Option<&tigrs_core::CancellationToken>,
) -> Option<DiffDocument> {
    let estimated_rows: usize = diff
        .files
        .iter()
        .map(|f| f.hunks.iter().map(|h| h.lines.len() + 2).sum::<usize>() + 6)
        .sum::<usize>()
        + 16;
    let mut rows = Vec::with_capacity(estimated_rows);
    let mut file_indices = Vec::with_capacity(diff.files.len());
    let mut hunk_indices = Vec::new();
    let mut line_to_file = Vec::with_capacity(estimated_rows);
    let mut line_to_hunk = Vec::with_capacity(estimated_rows);

    append_commit_header_rows(diff, &mut rows, &mut line_to_file, &mut line_to_hunk);
    append_diffstat_rows(diff, &mut rows, &mut line_to_file, &mut line_to_hunk);

    // --- 3. Per-File Diff Generation ---
    let is_global_expanded =
        opts.diff_context > 3 || opts.is_full_context() || !hunk_extra_context.is_empty();
    let mut remaining_highlight_budget = if is_global_expanded && diff.files.len() <= 2 {
        opts.memory_profile
            .max_diff_highlight_total_lines()
            .max(4_000)
    } else {
        opts.memory_profile.max_diff_highlight_total_lines()
    };
    let mut remaining_word_diff_budget = crate::highlight::MAX_DIFF_WORD_DIFF_TOTAL_LINES;
    let mut remaining_full_context_budget = if diff.files.len() > 2 {
        opts.diff_context_full_max_lines.min(1_000)
    } else {
        opts.diff_context_full_max_lines
    };

    for (f_idx, file) in diff.files.iter().enumerate() {
        if cancel.is_some_and(tigrs_core::CancellationToken::is_cancelled) {
            return None;
        }
        let cur_f = u32::try_from(f_idx).ok();
        file_indices.push(rows.len());

        let is_folded = folded_files.contains(&f_idx);
        append_file_header_rows(
            f_idx,
            file,
            diff,
            opts,
            is_folded,
            &mut rows,
            &mut line_to_file,
            &mut line_to_hunk,
        );
        if is_folded {
            continue;
        }

        if file.is_binary {
            push_unified_row(
                RowCell::new(LineMarker::None, "Binary files differ".to_string(), None),
                cur_f,
                None,
                DiffLineType::BinaryNote,
                &mut rows,
                &mut line_to_file,
                &mut line_to_hunk,
            );
            continue;
        }

        let has_hunk_extra = hunk_extra_context.keys().any(|&(f, _)| f == f_idx);
        let blob_lines = if opts.diff_context > 3 || opts.is_full_context() || has_hunk_extra {
            let target_oid = if file.status == FileChangeStatus::Deleted {
                file.old_id
            } else {
                file.new_id
            };
            target_oid.and_then(|oid| blob_provider.as_ref().and_then(|provider| provider(oid)))
        } else {
            None
        };

        let blob_len = blob_lines.as_ref().map_or(0, |b| b.len());
        let (effective_context, effective_is_full) = if opts.is_full_context() {
            if blob_len > 0 && blob_len <= remaining_full_context_budget {
                remaining_full_context_budget =
                    remaining_full_context_budget.saturating_sub(blob_len);
                (opts.diff_context, true)
            } else {
                (20, false)
            }
        } else {
            (opts.diff_context, false)
        };

        let items = expand_file_items_with_hunk_context(
            f_idx,
            file,
            blob_lines.as_deref(),
            effective_context,
            effective_is_full,
            Some(hunk_extra_context),
        );

        let expanded_line_count = items.len();
        let mut file_hl = if opts.syntax_highlighting
            && !file.is_binary
            && remaining_highlight_budget > 0
            && expanded_line_count <= crate::highlight::MAX_HIGHLIGHT_LINES
        {
            FileSyntaxHighlighter::new(&file.path, &opts.syntax_theme)
        } else {
            None
        };

        let mut current_hunk_idx: Option<usize> = (!file.hunks.is_empty()).then_some(0);
        let mut section_lines: Vec<ExpandedLine> = Vec::new();

        let flush_section =
            |lines: &mut Vec<ExpandedLine>,
             hunk_idx: Option<usize>,
             hl: Option<&mut FileSyntaxHighlighter>,
             hl_budget: &mut usize,
             wd_budget: &mut usize,
             rows: &mut Vec<RowPair>,
             line_to_file: &mut Vec<Option<u32>>,
             line_to_hunk: &mut Vec<Option<HunkLocation>>| {
                if lines.is_empty() {
                    return;
                }
                if opts.diff_layout == DiffLayout::SideBySide {
                    build_side_by_side_lines(
                        lines,
                        cur_f,
                        hunk_idx,
                        opts,
                        hl,
                        hl_budget,
                        wd_budget,
                        rows,
                        line_to_file,
                        line_to_hunk,
                    );
                } else {
                    build_unified_lines(
                        lines,
                        cur_f,
                        hunk_idx,
                        opts,
                        hl,
                        hl_budget,
                        wd_budget,
                        rows,
                        line_to_file,
                        line_to_hunk,
                    );
                }
                lines.clear();
            };

        for item in items {
            match item {
                ExpandedItem::HunkHeader { hunk_idx } => {
                    if cancel.is_some_and(tigrs_core::CancellationToken::is_cancelled) {
                        return None;
                    }
                    flush_section(
                        &mut section_lines,
                        current_hunk_idx,
                        file_hl.as_mut(),
                        &mut remaining_highlight_budget,
                        &mut remaining_word_diff_budget,
                        &mut rows,
                        &mut line_to_file,
                        &mut line_to_hunk,
                    );

                    current_hunk_idx = Some(hunk_idx);
                    hunk_indices.push(rows.len());

                    let hunk_text =
                        format_hunk_header_text(f_idx, hunk_idx, file, hunk_extra_context, opts);
                    push_unified_row(
                        RowCell::new(LineMarker::None, hunk_text, None),
                        cur_f,
                        Some(HunkLocation {
                            file_idx: f_idx,
                            hunk_idx,
                            line_idx: None,
                        }),
                        DiffLineType::HunkHeader,
                        &mut rows,
                        &mut line_to_file,
                        &mut line_to_hunk,
                    );
                }
                ExpandedItem::Line(line) => {
                    section_lines.push(line);
                }
            }
        }

        flush_section(
            &mut section_lines,
            current_hunk_idx,
            file_hl.as_mut(),
            &mut remaining_highlight_budget,
            &mut remaining_word_diff_budget,
            &mut rows,
            &mut line_to_file,
            &mut line_to_hunk,
        );
    }

    if opts.color_moved {
        apply_moved_block_markers(diff, &mut rows);
    }

    Some(DiffDocument::new(
        rows,
        file_indices,
        hunk_indices,
        line_to_file,
        line_to_hunk,
    ))
}

/// Minimum contiguous lines required to classify a relocated run as a moved block.
const MIN_MOVED_LINES: usize = 3;
/// Minimum total alphanumeric characters across a moved block to avoid trivial brace/blank matches.
const MIN_MOVED_ALNUM_CHARS: usize = 20;

#[derive(Debug)]
struct ChangeRun<'a> {
    file_idx: usize,
    hunk_idx: usize,
    group_id: usize,
    lines: Vec<(usize, &'a str, u64)>,
}

fn hash_normalized_line(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Detects moved code blocks (`git diff --color-moved`) across a `CommitDiff`.
///
/// Returns the set of canonical `(file_idx, hunk_idx, line_idx)` anchors belonging
/// to relocated blocks of at least `MIN_MOVED_LINES` (3) contiguous lines and
/// `MIN_MOVED_ALNUM_CHARS` (20) alphanumeric characters.
#[must_use]
pub fn detect_moved_blocks(diff: &CommitDiff) -> HashSet<(usize, usize, usize)> {
    let has_enough_deletions = diff.files.iter().any(|f| {
        !f.is_binary
            && (f.deletions >= MIN_MOVED_LINES
                || f.hunks.iter().any(|h| {
                    h.lines
                        .iter()
                        .filter(|l| l.kind == DiffLineKind::Remove)
                        .count()
                        >= MIN_MOVED_LINES
                }))
    });
    if !has_enough_deletions {
        return HashSet::new();
    }
    let has_enough_additions = diff.files.iter().any(|f| {
        !f.is_binary
            && (f.additions >= MIN_MOVED_LINES
                || f.hunks.iter().any(|h| {
                    h.lines
                        .iter()
                        .filter(|l| l.kind == DiffLineKind::Add)
                        .count()
                        >= MIN_MOVED_LINES
                }))
    });
    if !has_enough_additions {
        return HashSet::new();
    }

    let mut del_runs: Vec<ChangeRun<'_>> = Vec::new();
    let mut add_runs: Vec<ChangeRun<'_>> = Vec::new();
    let mut next_group_id: usize = 0;

    for (f_idx, file) in diff.files.iter().enumerate() {
        if file.is_binary {
            continue;
        }
        for (h_idx, hunk) in file.hunks.iter().enumerate() {
            let mut l_idx = 0;
            while l_idx < hunk.lines.len() {
                if hunk.lines[l_idx].kind == DiffLineKind::Context {
                    l_idx += 1;
                    continue;
                }
                let group_id = next_group_id;
                next_group_id += 1;

                let mut del_lines = Vec::new();
                while l_idx < hunk.lines.len() && hunk.lines[l_idx].kind == DiffLineKind::Remove {
                    let norm = hunk.lines[l_idx].content.trim();
                    del_lines.push((l_idx, norm, hash_normalized_line(norm)));
                    l_idx += 1;
                }
                if del_lines.len() >= MIN_MOVED_LINES {
                    del_runs.push(ChangeRun {
                        file_idx: f_idx,
                        hunk_idx: h_idx,
                        group_id,
                        lines: del_lines,
                    });
                }

                let mut add_lines = Vec::new();
                while l_idx < hunk.lines.len() && hunk.lines[l_idx].kind == DiffLineKind::Add {
                    let norm = hunk.lines[l_idx].content.trim();
                    add_lines.push((l_idx, norm, hash_normalized_line(norm)));
                    l_idx += 1;
                }
                if add_lines.len() >= MIN_MOVED_LINES {
                    add_runs.push(ChangeRun {
                        file_idx: f_idx,
                        hunk_idx: h_idx,
                        group_id,
                        lines: add_lines,
                    });
                }
            }
        }
    }

    if del_runs.is_empty() || add_runs.is_empty() {
        return HashSet::new();
    }

    // Index 3-line hash windows across all deletion runs
    let mut trigram_index: HashMap<(u64, u64, u64), Vec<(usize, usize)>> = HashMap::new();
    for (r_idx, run) in del_runs.iter().enumerate() {
        if run.lines.len() < MIN_MOVED_LINES {
            continue;
        }
        for offset in 0..=run.lines.len() - MIN_MOVED_LINES {
            let key = (
                run.lines[offset].2,
                run.lines[offset + 1].2,
                run.lines[offset + 2].2,
            );
            trigram_index.entry(key).or_default().push((r_idx, offset));
        }
    }

    let mut moved: HashSet<(usize, usize, usize)> = HashSet::new();

    for add_run in &add_runs {
        if add_run.lines.len() < MIN_MOVED_LINES {
            continue;
        }
        for j in 0..=add_run.lines.len() - MIN_MOVED_LINES {
            let key = (
                add_run.lines[j].2,
                add_run.lines[j + 1].2,
                add_run.lines[j + 2].2,
            );
            let Some(candidates) = trigram_index.get(&key) else {
                continue;
            };
            for &(del_r_idx, i) in candidates {
                let del_run = &del_runs[del_r_idx];
                // Must be relocated from a different change group (not an in-place replacement)
                if del_run.group_id == add_run.group_id {
                    continue;
                }
                let max_len = (del_run.lines.len() - i).min(add_run.lines.len() - j);
                let mut match_len = 0;
                while match_len < max_len
                    && del_run.lines[i + match_len].1 == add_run.lines[j + match_len].1
                {
                    match_len += 1;
                }
                if match_len >= MIN_MOVED_LINES {
                    let alnum_count: usize = add_run.lines[j..j + match_len]
                        .iter()
                        .map(|(_, s, _)| s.chars().filter(|c| c.is_alphanumeric()).count())
                        .sum();
                    if alnum_count >= MIN_MOVED_ALNUM_CHARS {
                        for k in 0..match_len {
                            moved.insert((
                                del_run.file_idx,
                                del_run.hunk_idx,
                                del_run.lines[i + k].0,
                            ));
                            moved.insert((
                                add_run.file_idx,
                                add_run.hunk_idx,
                                add_run.lines[j + k].0,
                            ));
                        }
                    }
                }
            }
        }
    }

    moved
}

/// Computes the 1-based line number of the first modified line in a hunk.
fn find_first_changed_lineno(hunk: &DiffHunk) -> u32 {
    let mut cur_new = hunk.new_start;
    let mut cur_old = hunk.old_start;
    for line in &hunk.lines {
        match line.kind {
            DiffLineKind::Context => {
                cur_new = cur_new.saturating_add(1);
                cur_old = cur_old.saturating_add(1);
            }
            DiffLineKind::Add => {
                return cur_new;
            }
            DiffLineKind::Remove => {
                return cur_old;
            }
        }
    }
    hunk.new_start
}

struct FileSyntaxHighlighter {
    path: String,
    theme: String,
    new_hl: crate::highlight::IncrementalHighlighter,
    old_hl: Option<crate::highlight::IncrementalHighlighter>,
}

impl FileSyntaxHighlighter {
    fn new(path: &str, theme: &str) -> Option<Self> {
        Some(Self {
            path: path.to_string(),
            theme: theme.to_string(),
            new_hl: crate::highlight::IncrementalHighlighter::with_theme_if_supported(path, theme)?,
            old_hl: None,
        })
    }

    #[inline]
    fn highlight_with(
        hl: &mut crate::highlight::IncrementalHighlighter,
        text: &str,
        budget: &mut usize,
    ) -> Arc<[crate::highlight::SyntaxSpan]> {
        if *budget == 0 {
            return Arc::from([]);
        }
        if !text.trim().is_empty() {
            *budget -= 1;
        }
        let spans = hl.highlight_line_spans(text);
        if spans.is_empty() {
            // Preserve a zero-width sentinel span so blank/whitespace-only lines inside a
            // syntax-highlighted file still receive the syntax diff background tint (`has_syntax`).
            Arc::from([crate::highlight::SyntaxSpan {
                start: 0,
                end: 0,
                fg: crate::headless::Color::White,
                bold: false,
                italic: false,
            }])
        } else {
            Arc::from(spans)
        }
    }

    #[inline]
    fn highlight_old(
        &mut self,
        text: &str,
        budget: &mut usize,
    ) -> Arc<[crate::highlight::SyntaxSpan]> {
        if *budget == 0 {
            return Arc::from([]);
        }
        if self.old_hl.is_none() {
            self.old_hl = crate::highlight::IncrementalHighlighter::with_theme_if_supported(
                &self.path,
                &self.theme,
            );
        }
        if let Some(hl) = self.old_hl.as_mut() {
            Self::highlight_with(hl, text, budget)
        } else {
            Arc::from([])
        }
    }

    #[inline]
    fn highlight_new(
        &mut self,
        text: &str,
        budget: &mut usize,
    ) -> Arc<[crate::highlight::SyntaxSpan]> {
        Self::highlight_with(&mut self.new_hl, text, budget)
    }

    #[inline]
    fn highlight_context(
        &mut self,
        text: &str,
        budget: &mut usize,
    ) -> (
        Arc<[crate::highlight::SyntaxSpan]>,
        Arc<[crate::highlight::SyntaxSpan]>,
    ) {
        if let Some(old_hl) = self.old_hl.as_mut() {
            let _ = old_hl.highlight_line_spans(text);
        }
        let spans = Self::highlight_with(&mut self.new_hl, text, budget);
        (Arc::clone(&spans), spans)
    }
}

/// Formats unified diff rows, performing intra-line word-diff and syntax highlighting on modified runs.
#[allow(clippy::too_many_arguments)]
fn build_unified_lines(
    lines: &[ExpandedLine],
    cur_f: Option<u32>,
    hunk_idx: Option<usize>,
    opts: &ViewOptions,
    mut hl: Option<&mut FileSyntaxHighlighter>,
    hl_budget: &mut usize,
    wd_budget: &mut usize,
    rows: &mut Vec<RowPair>,
    line_to_file: &mut Vec<Option<u32>>,
    line_to_hunk: &mut Vec<Option<HunkLocation>>,
) {
    let tab_size = opts.tab_size;
    let enclosing_hunk = cur_f.zip(hunk_idx).map(|(f_u32, h_idx)| HunkLocation {
        file_idx: f_u32 as usize,
        hunk_idx: h_idx,
        line_idx: None,
    });
    let mut i = 0;

    while i < lines.len() {
        let line = &lines[i];

        if line.marker == LineMarker::Context {
            let expanded_text = expand_tabs(&line.content, tab_size);
            let mut cell = RowCell::new(
                LineMarker::Context,
                expanded_text.clone(),
                line.new_lineno.or(line.old_lineno),
            );
            if let Some(ref mut highlighter) = hl {
                cell.syntax_spans = highlighter.highlight_new(&expanded_text, hl_budget);
            }
            rows.push(RowPair::unified(
                cell,
                line.anchor,
                DiffLineType::DiffContext,
            ));
            line_to_file.push(cur_f);
            line_to_hunk.push(line.anchor.or(enclosing_hunk));

            if line.no_newline_at_eof {
                rows.push(RowPair::unified(
                    RowCell::new(LineMarker::None, "\\ No newline at end of file", None),
                    None,
                    DiffLineType::Delimiter,
                ));
                line_to_file.push(cur_f);
                line_to_hunk.push(enclosing_hunk);
            }
            i += 1;
            continue;
        }

        // Change run: collect contiguous deletions followed by additions
        let del_start = i;
        while i < lines.len() && lines[i].marker == LineMarker::Del {
            i += 1;
        }
        let del_end = i;

        let add_start = i;
        while i < lines.len() && lines[i].marker == LineMarker::Add {
            i += 1;
        }
        let add_end = i;

        let del_count = del_end - del_start;
        let add_count = add_end - add_start;

        let mut old_spans: Vec<Vec<(u32, u32)>> = vec![Vec::new(); del_count];
        let mut new_spans: Vec<Vec<(u32, u32)>> = vec![Vec::new(); add_count];

        let old_texts: Vec<String> = lines[del_start..del_end]
            .iter()
            .map(|l| expand_tabs(&l.content, tab_size))
            .collect();
        let new_texts: Vec<String> = lines[add_start..add_end]
            .iter()
            .map(|l| expand_tabs(&l.content, tab_size))
            .collect();

        if opts.word_diff && del_count > 0 && add_count > 0 && *wd_budget > 0 {
            *wd_budget = wd_budget.saturating_sub(del_count + add_count);
            let old_slices: Vec<&str> = old_texts.iter().map(String::as_str).collect();
            let new_slices: Vec<&str> = new_texts.iter().map(String::as_str).collect();

            let alignment = align_runs(&old_slices, &new_slices, opts.word_diff_pairing.into());

            for (oi_opt, ni_opt) in alignment {
                if let (Some(oi), Some(ni)) = (oi_opt, ni_opt)
                    && let Some(wd) = word_diff(&old_texts[oi], &new_texts[ni])
                {
                    old_spans[oi] = wd
                        .old
                        .iter()
                        .map(|s| (s.start as u32, s.end as u32))
                        .collect();
                    new_spans[ni] = wd
                        .new
                        .iter()
                        .map(|s| (s.start as u32, s.end as u32))
                        .collect();
                }
            }
        }

        // Emit deletions
        for (idx, line) in lines[del_start..del_end].iter().enumerate() {
            let expanded_text = &old_texts[idx];
            let mut cell = RowCell::new(LineMarker::Del, expanded_text.clone(), line.old_lineno);
            cell.emphasis.clone_from(&old_spans[idx]);
            if let Some(ref mut highlighter) = hl {
                cell.syntax_spans = highlighter.highlight_old(expanded_text, hl_budget);
            }
            rows.push(RowPair::unified(cell, line.anchor, DiffLineType::DiffDel));
            line_to_file.push(cur_f);
            line_to_hunk.push(line.anchor.or(enclosing_hunk));

            if line.no_newline_at_eof {
                rows.push(RowPair::unified(
                    RowCell::new(LineMarker::None, "\\ No newline at end of file", None),
                    None,
                    DiffLineType::Delimiter,
                ));
                line_to_file.push(cur_f);
                line_to_hunk.push(enclosing_hunk);
            }
        }

        // Emit additions
        for (idx, line) in lines[add_start..add_end].iter().enumerate() {
            let expanded_text = &new_texts[idx];
            let mut cell = RowCell::new(LineMarker::Add, expanded_text.clone(), line.new_lineno);
            cell.emphasis.clone_from(&new_spans[idx]);
            if let Some(ref mut highlighter) = hl {
                cell.syntax_spans = highlighter.highlight_new(expanded_text, hl_budget);
            }
            rows.push(RowPair::unified(cell, line.anchor, DiffLineType::DiffAdd));
            line_to_file.push(cur_f);
            line_to_hunk.push(line.anchor.or(enclosing_hunk));

            if line.no_newline_at_eof {
                rows.push(RowPair::unified(
                    RowCell::new(LineMarker::None, "\\ No newline at end of file", None),
                    None,
                    DiffLineType::Delimiter,
                ));
                line_to_file.push(cur_f);
                line_to_hunk.push(enclosing_hunk);
            }
        }
    }
}

/// Formats side-by-side dual pane diff rows with paired alignments and syntax highlighting.
#[allow(clippy::too_many_arguments)]
fn build_side_by_side_lines(
    lines: &[ExpandedLine],
    cur_f: Option<u32>,
    hunk_idx: Option<usize>,
    opts: &ViewOptions,
    mut hl: Option<&mut FileSyntaxHighlighter>,
    hl_budget: &mut usize,
    wd_budget: &mut usize,
    rows: &mut Vec<RowPair>,
    line_to_file: &mut Vec<Option<u32>>,
    line_to_hunk: &mut Vec<Option<HunkLocation>>,
) {
    let tab_size = opts.tab_size;
    let enclosing_hunk = cur_f.zip(hunk_idx).map(|(f_u32, h_idx)| HunkLocation {
        file_idx: f_u32 as usize,
        hunk_idx: h_idx,
        line_idx: None,
    });
    let mut i = 0;

    while i < lines.len() {
        let line = &lines[i];

        if line.marker == LineMarker::Context {
            let expanded_text = expand_tabs(&line.content, tab_size);
            let mut left_cell = RowCell::new(
                LineMarker::Context,
                Arc::from(expanded_text.as_str()),
                line.old_lineno,
            );
            let mut right_cell = RowCell::new(
                LineMarker::Context,
                Arc::from(expanded_text.as_str()),
                line.new_lineno,
            );
            if let Some(ref mut highlighter) = hl {
                let (old_spans, new_spans) =
                    highlighter.highlight_context(&expanded_text, hl_budget);
                left_cell.syntax_spans = old_spans;
                right_cell.syntax_spans = new_spans;
            }

            rows.push(RowPair::paired(
                Some(left_cell),
                Some(right_cell),
                line.anchor,
                DiffLineType::DiffContext,
            ));
            line_to_file.push(cur_f);
            line_to_hunk.push(line.anchor.or(enclosing_hunk));

            if line.no_newline_at_eof {
                let eof_cell = RowCell::new(LineMarker::None, "\\ No newline at end of file", None);
                rows.push(RowPair::paired(
                    Some(eof_cell.clone()),
                    Some(eof_cell),
                    None,
                    DiffLineType::Delimiter,
                ));
                line_to_file.push(cur_f);
                line_to_hunk.push(enclosing_hunk);
            }
            i += 1;
            continue;
        }

        // Change run
        let del_start = i;
        while i < lines.len() && lines[i].marker == LineMarker::Del {
            i += 1;
        }
        let del_end = i;

        let add_start = i;
        while i < lines.len() && lines[i].marker == LineMarker::Add {
            i += 1;
        }
        let add_end = i;

        let old_texts: Vec<String> = lines[del_start..del_end]
            .iter()
            .map(|l| expand_tabs(&l.content, tab_size))
            .collect();
        let new_texts: Vec<String> = lines[add_start..add_end]
            .iter()
            .map(|l| expand_tabs(&l.content, tab_size))
            .collect();

        // Pre-compute syntax spans sequentially across old and new change runs
        let old_syntax_spans: Vec<Arc<[crate::highlight::SyntaxSpan]>> =
            if let Some(ref mut highlighter) = hl {
                old_texts
                    .iter()
                    .map(|t| highlighter.highlight_old(t, hl_budget))
                    .collect()
            } else {
                vec![Arc::from([]); old_texts.len()]
            };

        let new_syntax_spans: Vec<Arc<[crate::highlight::SyntaxSpan]>> =
            if let Some(ref mut highlighter) = hl {
                new_texts
                    .iter()
                    .map(|t| highlighter.highlight_new(t, hl_budget))
                    .collect()
            } else {
                vec![Arc::from([]); new_texts.len()]
            };

        let old_slices: Vec<&str> = old_texts.iter().map(String::as_str).collect();
        let new_slices: Vec<&str> = new_texts.iter().map(String::as_str).collect();

        let alignment = align_runs(&old_slices, &new_slices, opts.word_diff_pairing.into());
        // Side-by-side mode always computes intra-line word diff on paired modification rows
        // (Gerrit-style two-tone row + word background highlighting).
        let run_word_diff =
            (opts.word_diff || opts.diff_layout == DiffLayout::SideBySide) && *wd_budget > 0;
        if run_word_diff {
            *wd_budget = wd_budget.saturating_sub(old_texts.len() + new_texts.len());
        }

        for (oi_opt, ni_opt) in alignment {
            match (oi_opt, ni_opt) {
                (Some(oi), Some(ni)) => {
                    let old_line = &lines[del_start + oi];
                    let new_line = &lines[add_start + ni];

                    let mut left_cell = RowCell::new(
                        LineMarker::Del,
                        Arc::from(old_texts[oi].as_str()),
                        old_line.old_lineno,
                    );
                    let mut right_cell = RowCell::new(
                        LineMarker::Add,
                        Arc::from(new_texts[ni].as_str()),
                        new_line.new_lineno,
                    );

                    left_cell.syntax_spans = old_syntax_spans[oi].clone();
                    right_cell.syntax_spans = new_syntax_spans[ni].clone();

                    if run_word_diff && let Some(wd) = word_diff(&old_texts[oi], &new_texts[ni]) {
                        left_cell.emphasis = wd
                            .old
                            .iter()
                            .map(|s| (s.start as u32, s.end as u32))
                            .collect();
                        right_cell.emphasis = wd
                            .new
                            .iter()
                            .map(|s| (s.start as u32, s.end as u32))
                            .collect();
                    }

                    let anchor = old_line.anchor.or(new_line.anchor);
                    let paired_anchor = if old_line.anchor.is_some() {
                        new_line.anchor
                    } else {
                        None
                    };
                    rows.push(RowPair::paired_with_anchors(
                        Some(left_cell),
                        Some(right_cell),
                        anchor,
                        paired_anchor,
                        DiffLineType::DiffAdd,
                    ));
                    line_to_file.push(cur_f);
                    line_to_hunk.push(anchor.or(enclosing_hunk));
                }
                (Some(oi), None) => {
                    let old_line = &lines[del_start + oi];
                    let mut left_cell = RowCell::new(
                        LineMarker::Del,
                        Arc::from(old_texts[oi].as_str()),
                        old_line.old_lineno,
                    );
                    left_cell.syntax_spans = old_syntax_spans[oi].clone();
                    rows.push(RowPair::paired(
                        Some(left_cell),
                        None,
                        old_line.anchor,
                        DiffLineType::DiffDel,
                    ));
                    line_to_file.push(cur_f);
                    line_to_hunk.push(old_line.anchor.or(enclosing_hunk));
                }
                (None, Some(ni)) => {
                    let new_line = &lines[add_start + ni];
                    let mut right_cell = RowCell::new(
                        LineMarker::Add,
                        Arc::from(new_texts[ni].as_str()),
                        new_line.new_lineno,
                    );
                    right_cell.syntax_spans = new_syntax_spans[ni].clone();
                    rows.push(RowPair::paired(
                        None,
                        Some(right_cell),
                        new_line.anchor,
                        DiffLineType::DiffAdd,
                    ));
                    line_to_file.push(cur_f);
                    line_to_hunk.push(new_line.anchor.or(enclosing_hunk));
                }
                (None, None) => {}
            }
        }
    }
}

/// Returns `true` if `key` is a hyphenated RFC-822 / `git interpret-trailers` token
/// (`<Seg1>-<Seg2>(-<SegN>)*`), where every `-`-delimited segment starts with an
/// ASCII alphabetic character and consists solely of ASCII alphanumeric characters.
///
/// Requiring each segment to start with a letter matches standard hyphenated trailer
/// keys (`Signed-off-by`, `Change-Id`, `Tracker-Bug-Id`, `Reviewed-on`, `Tested-with`,
/// `Co-authored-by`, `Closes-Bug`, `Depends-On`, etc.) while rejecting numbered or
/// technical prose labels (`Step-1`, `Phase-2`, `UTF-8`, `SHA-256`).
#[inline]
fn is_hyphenated_trailer_key(key: &str) -> bool {
    if key.is_empty() || key.len() > 64 || !key.contains('-') {
        return false;
    }
    key.split('-').all(|seg| {
        !seg.is_empty()
            && seg.as_bytes()[0].is_ascii_alphabetic()
            && seg.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// Returns `true` if `line` has valid RFC-822 `Token: value` syntax suitable for
/// a line inside a trailing `git interpret-trailers` block.
#[inline]
fn is_rfc822_trailer_line(line: &str) -> bool {
    let Some((key, after_colon)) = line.split_once(':') else {
        return false;
    };
    if !after_colon.trim().is_empty() && !after_colon.starts_with([' ', '\t']) {
        return false;
    }
    !key.is_empty()
        && key.len() <= 64
        && key.as_bytes()[0].is_ascii_alphabetic()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Returns `true` if `line` starts with a recognized Git commit trailer or metadata tag
/// (case-insensitive), or a hyphenated RFC-822 trailer token (`<Word>-<Word>: <value>`).
#[must_use]
pub fn is_commit_trailer(line: &str) -> bool {
    const KNOWN_TRAILER_KEYS: &[&str] = &[
        // Upstream Tig canonical trailers
        "signed-off-by",
        "acked-by",
        "reviewed-by",
        "helped-by",
        "reported-by",
        "mentored-by",
        "suggested-by",
        "cc",
        "noticed-by",
        "tested-by",
        "improved-by",
        "thanks-to",
        "based-on-patch-by",
        "contributions-by",
        "co-authored-by",
        "requested-by",
        "original-patch-by",
        "inspired-by",
        "generated-by",
        "assisted-by",
        // Standard Git / Gerrit / issue-tracker / test metadata tags
        "fixes",
        "fixed",
        "closes",
        "closed",
        "resolves",
        "resolved",
        "change-id",
        "bug",
        "bugs",
        "issue",
        "issues",
        "test",
        "tested",
        "tests",
        "testing",
        "link",
        "links",
        "see-also",
        "ref",
        "refs",
        "reference",
        "references",
        "revert",
        "reverts",
        "backport",
        "relates",
        "related",
        "commit-queue",
        "commit-queue-record",
        "tested-with",
    ];

    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed
        .get(..27)
        .is_some_and(|s| s.eq_ignore_ascii_case("(cherry picked from commit "))
        && trimmed.ends_with(')')
    {
        return true;
    }

    let Some((key, after_colon)) = trimmed.split_once(':') else {
        return false;
    };

    // A trailer colon must be followed by whitespace or end-of-line (rejecting URLs like
    // `https://...` and scoped identifiers like `std::io::Error`).
    if !after_colon.trim().is_empty() && !after_colon.starts_with([' ', '\t']) {
        return false;
    }

    if KNOWN_TRAILER_KEYS
        .iter()
        .any(|k| key.eq_ignore_ascii_case(k))
    {
        return true;
    }

    is_hyphenated_trailer_key(key)
}

/// Classifies each line of a commit message body (with the 4-space display indent stripped)
/// as `true` (`CommitTrailer`) or `false` (`MessageBody`).
///
/// Combines three rules for consistent commit message highlighting:
/// 1. Any line matching [`is_commit_trailer`] is classified as a trailer.
/// 2. Indented continuation lines (`starts_with([' ', '\t'])`) immediately following a
///    trailer line in the same paragraph inherit trailer classification (supporting
///    RFC-822 folded values and multi-line `Tested:` lists).
/// 3. Following `git interpret-trailers` semantics, if the final non-empty paragraph of
///    the commit message contains at least one recognized trailer and consists solely of
///    valid RFC-822 `Token: value` lines (plus any indented continuations or cherry-pick
///    lines), all lines in that trailing paragraph are classified as trailers.
#[must_use]
pub fn classify_commit_body_lines(clean_lines: &[&str]) -> Vec<bool> {
    let mut flags = vec![false; clean_lines.len()];
    if clean_lines.is_empty() {
        return flags;
    }

    // Pass 1: Direct trailer recognition + indented continuation lines within each paragraph.
    let mut prev_in_para_is_trailer = false;
    for (idx, &line) in clean_lines.iter().enumerate() {
        if line.trim().is_empty() {
            prev_in_para_is_trailer = false;
            continue;
        }
        let is_indented = line.starts_with([' ', '\t']);
        if (!is_indented && is_commit_trailer(line))
            || (is_indented && (prev_in_para_is_trailer || is_commit_trailer(line)))
        {
            flags[idx] = true;
            prev_in_para_is_trailer = true;
        } else {
            prev_in_para_is_trailer = false;
        }
    }

    // Pass 2: `git interpret-trailers` trailing-paragraph promotion.
    if let Some(last_non_empty) = clean_lines.iter().rposition(|l| !l.trim().is_empty()) {
        let para_start = clean_lines[..=last_non_empty]
            .iter()
            .rposition(|l| l.trim().is_empty())
            .map_or(0, |blank_idx| blank_idx + 1);

        let para = &clean_lines[para_start..=last_non_empty];
        let has_known_trailer = flags[para_start..=last_non_empty].iter().any(|&f| f);
        if has_known_trailer {
            let mut prev_valid = false;
            let all_valid_trailer_syntax = para.iter().all(|&line| {
                let is_indented = line.starts_with([' ', '\t']);
                let valid = if is_indented {
                    prev_valid || is_commit_trailer(line)
                } else {
                    is_commit_trailer(line) || is_rfc822_trailer_line(line)
                };
                prev_valid = valid;
                valid
            });

            if all_valid_trailer_syntax {
                for flag in &mut flags[para_start..=last_non_empty] {
                    *flag = true;
                }
            }
        }
    }

    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_commit_trailer() {
        assert!(is_commit_trailer(
            "Signed-off-by: Alice <alice@example.com>"
        ));
        assert!(is_commit_trailer(
            "signed-off-by: Alice <alice@example.com>"
        ));
        assert!(is_commit_trailer(
            "    Signed-off-by: Alice <alice@example.com>"
        ));
        assert!(is_commit_trailer("Reviewed-by: Bob <bob@example.com>"));
        assert!(is_commit_trailer("Acked-by: Charlie <charlie@example.com>"));
        assert!(is_commit_trailer("Co-authored-by: Dave <dave@example.com>"));
        assert!(is_commit_trailer("Cc: Eve <eve@example.com>"));
        assert!(is_commit_trailer("Fixes: #1234"));
        assert!(is_commit_trailer("Change-Id: I1234567890abcdef"));
        assert!(is_commit_trailer("Bug: 98765"));
        assert!(is_commit_trailer("Tested: cargo test --workspace"));
        assert!(is_commit_trailer("Tested:"));
        assert!(is_commit_trailer("Test: manual verification"));
        assert!(is_commit_trailer("Tracker-Bug-Id: 123456"));
        assert!(is_commit_trailer(
            "Reviewed-on: https://gerrit.example.com/c/project/+/42"
        ));
        assert!(is_commit_trailer(
            "(cherry picked from commit 0123456789abcdef0123456789abcdef01234567)"
        ));

        // Non-trailers
        assert!(!is_commit_trailer("This is just normal text"));
        assert!(!is_commit_trailer("Note: this is not a git trailer"));
        assert!(!is_commit_trailer(
            "Step-1: do not highlight numbered steps"
        ));
        assert!(!is_commit_trailer("UTF-8: support non-ascii characters"));
        assert!(!is_commit_trailer("https://example.com/path"));
        assert!(!is_commit_trailer("Fixes in this commit were verified"));
        assert!(!is_commit_trailer("Line mentioning Signed-off-by: later"));
        assert!(!is_commit_trailer(""));
    }

    #[test]
    fn test_classify_commit_body_lines_trailer_block_and_continuations() {
        let lines = vec![
            "Explain why the change is needed.",
            "Note: this prose line in an earlier paragraph is not a trailer.",
            "",
            "Tested:",
            "  - unit tests",
            "  - integration tests",
            "",
            "Tracker-Bug-Id: 424242",
            "CustomTag: custom-trailer-in-final-block",
            "Change-Id: I1234567890abcdef",
            "Signed-off-by: Alice Developer <alice@example.com>",
        ];
        let flags = classify_commit_body_lines(&lines);
        assert_eq!(
            flags,
            vec![
                false, // Explain why...
                false, // Note: this prose line...
                false, // blank
                true,  // Tested:
                true,  //   - unit tests (indented continuation)
                true,  //   - integration tests (indented continuation)
                false, // blank
                true,  // Tracker-Bug-Id: 424242
                true,  // CustomTag: promoted by trailing trailer block
                true,  // Change-Id: ...
                true,  // Signed-off-by: ...
            ]
        );
    }

    #[test]
    fn test_format_rename_brace_diff_comprehensive() {
        // Identical paths return the path unchanged
        assert_eq!(
            format_rename_brace_diff("src/lib.rs", "src/lib.rs", false),
            "src/lib.rs"
        );

        // Disjoint paths with no shared delimiter prefix/suffix
        assert_eq!(
            format_rename_brace_diff("alpha.txt", "beta.md", false),
            "alpha.txt \u{2192} beta.md"
        );
        assert_eq!(
            format_rename_brace_diff("alpha.txt", "beta.md", true),
            "alpha.txt -> beta.md"
        );

        // Shared prefix and suffix snapping to token delimiters ('/', '-', '_', '.')
        assert_eq!(
            format_rename_brace_diff(
                "sound/soc/amd/acp/acp-sdw-mach.c",
                "sound/soc/amd/acp/acp-sdw-sof-mach.c",
                false
            ),
            "sound/soc/amd/acp/acp-sdw-{mach \u{2192} sof-mach}.c"
        );
        assert_eq!(
            format_rename_brace_diff(
                "sound/soc/amd/acp/acp-sdw-mach.c",
                "sound/soc/amd/acp/acp-sdw-sof-mach.c",
                true
            ),
            "sound/soc/amd/acp/acp-sdw-{mach -> sof-mach}.c"
        );

        // Multi-byte UTF-8 characters in paths must never panic on byte boundaries
        assert_eq!(
            format_rename_brace_diff("docs/café_old.md", "docs/café_new.md", false),
            "docs/café_{old \u{2192} new}.md"
        );
    }

    #[test]
    fn test_elide_middle_path_comprehensive() {
        // Path within max_width is untouched
        assert_eq!(elide_middle_path("src/main.rs", 20, false), "src/main.rs");

        // <= 2 segments are never middle-elided even if max_width is tiny
        assert_eq!(
            elide_middle_path("src/very_long_filename.rs", 5, false),
            "src/very_long_filename.rs"
        );

        // Deep path preserving first segment, second-last segment, and filename when it fits
        let deep = "drivers/gpu/drm/amd/display/dc/dml2/dml21/src/dml2_core/dml2_core_dcn4.c";
        assert_eq!(
            elide_middle_path(deep, 42, false),
            "drivers/…/dml2_core/dml2_core_dcn4.c"
        );
        assert_eq!(
            elide_middle_path(deep, 44, true),
            "drivers/.../dml2_core/dml2_core_dcn4.c"
        );

        // Falls back to first/…/last when tighter max_width is given
        assert_eq!(
            elide_middle_path(deep, 30, false),
            "drivers/…/dml2_core_dcn4.c"
        );
        assert_eq!(
            elide_middle_path(deep, 30, true),
            "drivers/.../dml2_core_dcn4.c"
        );
    }

    #[test]
    fn test_format_magnitude_sparkline_all_buckets() {
        // UTF-8 buckets: 0, 1..=5, 6..=25, 26..=100, 101..=300, >300
        assert_eq!(format_magnitude_sparkline(0, 0, false), "▏ ▏");
        assert_eq!(format_magnitude_sparkline(3, 2, false), "▏\u{2581}▏");
        assert_eq!(format_magnitude_sparkline(10, 15, false), "▏\u{2583}▏");
        assert_eq!(format_magnitude_sparkline(50, 50, false), "▏\u{2585}▏");
        assert_eq!(format_magnitude_sparkline(200, 100, false), "▏\u{2587}▏");
        assert_eq!(
            format_magnitude_sparkline(201, 100, false),
            "▏\u{2588}\u{2589}▏"
        );
        assert_eq!(
            format_magnitude_sparkline(usize::MAX, 10, false),
            "▏\u{2588}\u{2589}▏"
        );

        // ASCII fallback buckets
        assert_eq!(format_magnitude_sparkline(0, 0, true), "[   ]");
        assert_eq!(format_magnitude_sparkline(1, 4, true), "[.  ]");
        assert_eq!(format_magnitude_sparkline(12, 13, true), "[#  ]");
        assert_eq!(format_magnitude_sparkline(60, 40, true), "[## ]");
        assert_eq!(format_magnitude_sparkline(101, 0, true), "[###]");
    }

    #[test]
    fn test_detect_invisible_change_badges_all_cases() {
        use tigrs_git::{DiffHunk, DiffLineKind, FileChangeStatus, FileDiff, HunkLine};

        // 1. Mode-only change with 0 content lines
        let mode_only = FileDiff {
            status: FileChangeStatus::Modified,
            path: "run.sh".to_string(),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_755),
            is_binary: false,
            hunks: vec![],
            old_id: None,
            new_id: None,
            additions: 0,
            deletions: 0,
        };
        assert_eq!(
            detect_invisible_change_badges(&mode_only, false),
            vec!["mode 100644 \u{2192} 100755 \u{00b7} no content change"]
        );
        assert_eq!(
            detect_invisible_change_badges(&mode_only, true),
            vec!["mode 100644 -> 100755 | no content change"]
        );

        // 2. Line-ending flip CRLF -> LF
        let crlf_to_lf = FileDiff {
            status: FileChangeStatus::Modified,
            path: "win.txt".to_string(),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            is_binary: false,
            old_id: None,
            new_id: None,
            additions: 1,
            deletions: 1,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 1,
                func_context: None,
                lines: vec![
                    HunkLine {
                        kind: DiffLineKind::Remove,
                        content: "hello world\r".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "hello world".to_string(),
                        no_newline_at_eof: false,
                    },
                ],
            }],
        };
        assert_eq!(
            detect_invisible_change_badges(&crlf_to_lf, false),
            vec!["line endings CRLF \u{2192} LF"]
        );

        // 3. Line-ending flip LF -> CRLF
        let lf_to_crlf = FileDiff {
            hunks: vec![DiffHunk {
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 1,
                func_context: None,
                lines: vec![
                    HunkLine {
                        kind: DiffLineKind::Remove,
                        content: "hello world".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "hello world\r".to_string(),
                        no_newline_at_eof: false,
                    },
                ],
            }],
            ..crlf_to_lf.clone()
        };
        assert_eq!(
            detect_invisible_change_badges(&lf_to_crlf, true),
            vec!["line endings LF -> CRLF"]
        );

        // 4. Whitespace-only edit
        let ws_only = FileDiff {
            hunks: vec![DiffHunk {
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 1,
                func_context: None,
                lines: vec![
                    HunkLine {
                        kind: DiffLineKind::Remove,
                        content: "fn  compute( x: i32 ) {".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "    fn compute(x: i32) {".to_string(),
                        no_newline_at_eof: false,
                    },
                ],
            }],
            ..crlf_to_lf
        };
        assert_eq!(
            detect_invisible_change_badges(&ws_only, false),
            vec!["whitespace only"]
        );
    }

    #[test]
    fn test_detect_file_language_badge_and_enclosing_symbol() {
        assert_eq!(detect_file_language_badge("Makefile"), Some("Make"));
        assert_eq!(detect_file_language_badge("sub/Dockerfile"), Some("Docker"));
        assert_eq!(detect_file_language_badge("Cargo.toml"), Some("TOML"));
        assert_eq!(detect_file_language_badge("src/main.rs"), Some("Rust"));
        assert_eq!(detect_file_language_badge("driver.c"), Some("C"));
        assert_eq!(detect_file_language_badge("schema.proto"), Some("Proto"));
        assert_eq!(detect_file_language_badge("UNKNOWN"), None);
        assert_eq!(detect_file_language_badge("file.unknown_ext"), None);

        assert_eq!(format_enclosing_symbol("   "), "");
        assert_eq!(format_enclosing_symbol("fn short_sig()"), "fn short_sig()");
        assert_eq!(
            format_enclosing_symbol(
                "static const struct drm_crtc_helper_funcs *amdgpu_dm_crtc_get_helper_funcs(struct drm_crtc *crtc, int idx)"
            ),
            "amdgpu_dm_crtc_get_helper_funcs()"
        );
    }

    #[test]
    fn test_format_real_file_and_hunk_headers_all_statuses() {
        use tigrs_git::{DiffHunk, FileChangeStatus, FileDiff, ObjectId};

        let oid1 = ObjectId::from_bytes_or_panic(&[0xaa; 20]);
        let oid2 = ObjectId::from_bytes_or_panic(&[0xbb; 20]);

        let added = FileDiff {
            status: FileChangeStatus::Added,
            path: "src/new.rs".to_string(),
            old_mode: None,
            new_mode: Some(0o100_644),
            is_binary: false,
            hunks: vec![],
            old_id: None,
            new_id: Some(oid2),
            additions: 5,
            deletions: 0,
        };
        let added_hdr = format_real_file_header(&added);
        assert!(added_hdr.contains("diff --git /dev/null b/src/new.rs"));
        assert!(added_hdr.contains("new file mode 100644"));

        let copied = FileDiff {
            status: FileChangeStatus::Copied {
                source_path: "src/orig.rs".to_string(),
                similarity_pct: 92,
            },
            path: "src/copy.rs".to_string(),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            is_binary: false,
            hunks: vec![],
            old_id: Some(oid1),
            new_id: Some(oid2),
            additions: 2,
            deletions: 0,
        };
        let copied_hdr = format_real_file_header(&copied);
        assert!(copied_hdr.contains("similarity index 92%"));
        assert!(copied_hdr.contains("copy from src/orig.rs"));
        assert!(copied_hdr.contains("copy to src/copy.rs"));

        let hunk_no_ctx = DiffHunk {
            old_start: 10,
            old_len: 4,
            new_start: 12,
            new_len: 6,
            func_context: None,
            lines: vec![],
        };
        assert_eq!(format_real_hunk_header(&hunk_no_ctx), "@@ -10,4 +12,6 @@");
    }
}
