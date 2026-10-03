// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Revision graph DAG layout engine, box-drawing glyphs, and multi-color lane palette.
//!
//! Provides 100% fidelity with Tig's `graph-v2` layout algorithm in safe Rust:
//! - 1-to-1 table row mapping for responsive TUI navigation
//! - UTF-8 Unicode curved box-drawing glyphs (`∙`, `●─╮`, ` │`, `─╯`, ` ├`, `─│`, `──`, `◎`, `◯`)
//! - ASCII fallback mode (`*`, `M-.`, ` |`, `-'`, ` +`, `-|`, `--`, ` I`, ` o`)
//! - 14-color cyclic ANSI palette for distinct visual branch trails
//! - Memory-bounded 16-byte `CompactGraphRow` representation

use crate::options::LineGraphics;

use tigrs_git::ObjectId;

/// Maximum number of active lanes supported in the compact graph row.
pub const MAX_GRAPH_LANES: usize = 16;

/// 14-color ANSI palette for branch lanes matching Tig's `palette-0` through `palette-13`.
pub const GRAPH_PALETTE_ANSI: [&str; 14] = [
    "\x1b[35m",   // 0: Magenta
    "\x1b[33m",   // 1: Yellow
    "\x1b[36m",   // 2: Cyan
    "\x1b[32m",   // 3: Green
    "\x1b[34m",   // 4: Blue
    "\x1b[37m",   // 5: White
    "\x1b[31m",   // 6: Red
    "\x1b[35;1m", // 7: Bright Magenta
    "\x1b[33;1m", // 8: Bright Yellow
    "\x1b[36;1m", // 9: Bright Cyan
    "\x1b[32;1m", // 10: Bright Green
    "\x1b[34;1m", // 11: Bright Blue
    "\x1b[90m",   // 12: Grey / Bright Black
    "\x1b[31;1m", // 13: Bright Red
];

/// ANSI color sequence for commit markers (`LINE_GRAPH_COMMIT`).
pub const COMMIT_COLOR_ANSI: &str = "\x1b[36;1m"; // Bright Cyan

/// ANSI color sequence for merge markers.
pub const MERGE_COLOR_ANSI: &str = "\x1b[33;1m"; // Bright Yellow

/// ANSI color reset.
pub const RESET_ANSI: &str = "\x1b[0m";

/// Individual visual symbol in a graph cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum GraphGlyph {
    /// Empty column (`"  "`).
    #[default]
    Empty = 0,
    /// Standard commit node (`" ∙"` / `" *"`).
    Commit = 1,
    /// Merge commit node (`" ●"` / `" M"`).
    Merge = 2,
    /// Merge commit with horizontal connector to the right (`"●─"` / `"M-"`).
    MergeFork = 3,
    /// Vertical branch line (`" │"` / `" |"`).
    Vertical = 4,
    /// Branch joining into a lane to the left (`"─╯"` / `"-'"`).
    Join = 5,
    /// Branch merging from the right (`"─╮"` / `"-."`).
    BranchMerge = 6,
    /// Branch forking from an active lane (`" ├"` / `" +"`).
    Fork = 7,
    /// New branch turning down (`" ╭"` / `" ."`).
    TurnDown = 8,
    /// Horizontal bridge (`"──"` / `"--"`).
    Horizontal = 9,
    /// Horizontal connector crossing over an active vertical lane (`"─│"` / `"-|"`).
    CrossOver = 10,
    /// Cross merge (`"─┼"` / `"-+"`).
    CrossMerge = 11,
    /// Multi-branch join (`"─┴"` / `"-+"`).
    MultiBranch = 12,
    /// Multi-merge connector (`"─┬"` / `"-+"`).
    MultiMerge = 13,
    /// Initial / root commit node (`" ◎"` / `" I"`).
    Initial = 14,
    /// Boundary commit node (`" ◯"` / `" o"`).
    Boundary = 15,
}

impl GraphGlyph {
    /// Converts a raw 4-bit integer into a [`GraphGlyph`].
    #[inline]
    #[must_use]
    pub const fn from_u8(val: u8) -> Self {
        match val & 0x0F {
            1 => Self::Commit,
            2 => Self::Merge,
            3 => Self::MergeFork,
            4 => Self::Vertical,
            5 => Self::Join,
            6 => Self::BranchMerge,
            7 => Self::Fork,
            8 => Self::TurnDown,
            9 => Self::Horizontal,
            10 => Self::CrossOver,
            11 => Self::CrossMerge,
            12 => Self::MultiBranch,
            13 => Self::MultiMerge,
            14 => Self::Initial,
            15 => Self::Boundary,
            _ => Self::Empty,
        }
    }

    /// Returns the 2-character visual representation for the given [`LineGraphics`] mode.
    #[inline]
    #[must_use]
    pub const fn as_str(self, style: LineGraphics) -> &'static str {
        match style {
            LineGraphics::Utf8 => match self {
                Self::Empty => "  ",
                Self::Commit => " ∙",
                Self::Merge => " ●",
                Self::MergeFork => "●─",
                Self::Vertical => " │",
                Self::Join => "─╯",
                Self::BranchMerge => "─╮",
                Self::Fork => " ├",
                Self::TurnDown => " ╭",
                Self::Horizontal => "──",
                Self::CrossOver => "─│",
                Self::CrossMerge => "─┼",
                Self::MultiBranch => "─┴",
                Self::MultiMerge => "─┬",
                Self::Initial => " ◎",
                Self::Boundary => " ◯",
            },
            LineGraphics::Ascii => match self {
                Self::Empty => "  ",
                Self::Commit => " *",
                Self::Merge => " M",
                Self::MergeFork => "M-",
                Self::Vertical => " |",
                Self::Join => "-'",
                Self::BranchMerge => "-.",
                Self::Fork => " +",
                Self::TurnDown => " .",
                Self::Horizontal => "--",
                Self::CrossOver => "-|",
                Self::CrossMerge | Self::MultiBranch | Self::MultiMerge => "-+",
                Self::Initial => " I",
                Self::Boundary => " o",
            },
        }
    }

    /// Returns `true` if this glyph represents a commit node.
    #[inline]
    #[must_use]
    pub const fn is_commit_node(self) -> bool {
        matches!(
            self,
            Self::Commit
                | Self::Merge
                | Self::MergeFork
                | Self::Join
                | Self::Initial
                | Self::Boundary
        )
    }
}

/// Compact 16-byte representation of a single commit row's graph layout.
///
/// Encodes up to 16 lanes as packed 4-bit glyph indices with active lane bitmask
/// for extreme cache locality and zero heap allocations across millions of commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactGraphRow {
    /// Index of the lane containing this commit's primary node.
    pub lane_idx: u8,
    /// Total number of rendered columns for this row.
    pub num_lanes: u8,
    /// Whether this commit is a merge commit.
    pub is_merge: bool,
    /// Reserved flags for future extensions.
    pub flags: u8,
    /// 16-bit active lane bitmask.
    pub active_mask: u16,
    /// Packed 4-bit glyph indicators for up to 16 columns (16 * 4 = 64 bits).
    pub glyphs: u64,
}

impl CompactGraphRow {
    /// Creates a new compact graph row with packed glyph data.
    #[inline]
    #[must_use]
    pub const fn new(
        lane_idx: u8,
        num_lanes: u8,
        is_merge: bool,
        active_mask: u16,
        glyphs: u64,
    ) -> Self {
        Self {
            lane_idx,
            num_lanes,
            is_merge,
            flags: 0,
            active_mask,
            glyphs,
        }
    }

    /// Retrieves the [`GraphGlyph`] at column index `lane`.
    #[inline]
    #[must_use]
    pub fn glyph_at(&self, lane: usize) -> GraphGlyph {
        if lane >= MAX_GRAPH_LANES {
            GraphGlyph::Empty
        } else {
            let shift = lane * 4;
            let val = ((self.glyphs >> shift) & 0x0F) as u8;
            GraphGlyph::from_u8(val)
        }
    }

    /// Appends the row's plain visual characters to `out`.
    ///
    /// Prefer this over [`Self::to_graph_string`] in render loops: it reuses the
    /// caller's buffer instead of allocating a `String` per commit row.
    pub fn render_into(&self, out: &mut String, style: LineGraphics) {
        let count = (self.num_lanes as usize).clamp(1, MAX_GRAPH_LANES);
        out.reserve(count * 2);
        for i in 0..count {
            out.push_str(self.glyph_at(i).as_str(style));
        }
    }

    /// Appends the row's ANSI multi-color styled characters to `out`.
    ///
    /// Buffer-reusing counterpart of [`Self::to_colored_graph_string`].
    pub fn render_colored_into(&self, out: &mut String, style: LineGraphics) {
        let count = (self.num_lanes as usize).clamp(1, MAX_GRAPH_LANES);
        out.reserve(count * 16);
        let mut current_color = None;

        for i in 0..count {
            let glyph = self.glyph_at(i);
            if glyph == GraphGlyph::Empty {
                if current_color.is_some() {
                    out.push_str(RESET_ANSI);
                    current_color = None;
                }
                out.push_str("  ");
                continue;
            }

            let text = glyph.as_str(style);
            let color = if glyph == GraphGlyph::Merge || glyph == GraphGlyph::MergeFork {
                MERGE_COLOR_ANSI
            } else if glyph.is_commit_node() {
                COMMIT_COLOR_ANSI
            } else {
                GRAPH_PALETTE_ANSI[i % GRAPH_PALETTE_ANSI.len()]
            };

            if current_color != Some(color) {
                out.push_str(color);
                current_color = Some(color);
            }
            out.push_str(text);
        }

        if current_color.is_some() {
            out.push_str(RESET_ANSI);
        }
    }

    /// Appends the row's palette-themed characters to `out`, using `palette.graph_lanes`
    /// when a named theme is active (or `GRAPH_PALETTE_ANSI` in adaptive default mode)
    /// and restoring `reset_sgr` after colored segments.
    pub fn render_palette_into(
        &self,
        out: &mut String,
        style: LineGraphics,
        palette: &crate::ui_theme::UiPalette,
        reset_sgr: &str,
    ) {
        if palette.id == tigrs_core::config_enums::UiThemeId::Default && reset_sgr == RESET_ANSI {
            self.render_colored_into(out, style);
            return;
        }
        let count = (self.num_lanes as usize).clamp(1, MAX_GRAPH_LANES);
        out.reserve(count * 16);
        let mut current_color = None;

        for i in 0..count {
            let glyph = self.glyph_at(i);
            if glyph == GraphGlyph::Empty {
                if current_color.is_some() {
                    out.push_str(reset_sgr);
                    current_color = None;
                }
                out.push_str("  ");
                continue;
            }

            let text = glyph.as_str(style);
            let color = if glyph == GraphGlyph::Merge || glyph == GraphGlyph::MergeFork {
                palette.commit_id_fg
            } else if glyph.is_commit_node() {
                palette.ref_head_fg
            } else {
                palette.graph_lanes[i % palette.graph_lanes.len()]
            };

            if current_color != Some(color) {
                crate::ui_theme::UiPalette::write_fg_sgr(out, color);
                current_color = Some(color);
            }
            out.push_str(text);
        }

        if current_color.is_some() {
            out.push_str(reset_sgr);
        }
    }

    /// Formats the row into plain visual characters without ANSI color styling.
    #[must_use]
    pub fn to_graph_string(&self, style: LineGraphics) -> String {
        let mut s = String::new();
        self.render_into(&mut s, style);
        s
    }

    /// Formats the row into ANSI multi-color styled characters using [`GRAPH_PALETTE_ANSI`].
    #[must_use]
    pub fn to_colored_graph_string(&self, style: LineGraphics) -> String {
        let mut s = String::new();
        self.render_colored_into(&mut s, style);
        s
    }
}

/// Helper to build packed glyph rows during commit history ingestion.
pub struct GraphRowBuilder {
    active_lanes: Vec<Option<ObjectId>>,
}

impl Default for GraphRowBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphRowBuilder {
    /// Creates a new graph row builder tracking active parent lanes.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active_lanes: Vec::with_capacity(MAX_GRAPH_LANES),
        }
    }

    /// Ingests a commit and its parent IDs, producing a [`CompactGraphRow`].
    pub fn process_commit(&mut self, commit_id: ObjectId, parents: &[ObjectId]) -> CompactGraphRow {
        let is_root = parents.is_empty();
        let is_merge = parents.len() > 1;

        // 1. Locate or allocate primary lane for this commit
        let lane_idx =
            if let Some(pos) = self.active_lanes.iter().position(|l| *l == Some(commit_id)) {
                pos
            } else if let Some(pos) = self.active_lanes.iter().position(Option::is_none) {
                self.active_lanes[pos] = Some(commit_id);
                pos
            } else if self.active_lanes.len() < MAX_GRAPH_LANES {
                self.active_lanes.push(Some(commit_id));
                self.active_lanes.len() - 1
            } else {
                self.active_lanes[MAX_GRAPH_LANES - 1] = Some(commit_id);
                MAX_GRAPH_LANES - 1
            };

        let mut pre_existing_mask: u16 = 0;
        for (i, lane) in self.active_lanes.iter().enumerate().take(MAX_GRAPH_LANES) {
            if lane.is_some() && i != lane_idx {
                pre_existing_mask |= 1 << i;
            }
        }

        // 2. Allocate or find lanes for secondary parents (merges)
        let mut merge_lanes = Vec::new();
        if is_merge {
            for &p in &parents[1..] {
                if let Some(pos) = self.active_lanes.iter().position(|l| *l == Some(p)) {
                    merge_lanes.push(pos);
                } else if let Some(pos) = self.active_lanes.iter().position(Option::is_none) {
                    self.active_lanes[pos] = Some(p);
                    merge_lanes.push(pos);
                } else if self.active_lanes.len() < MAX_GRAPH_LANES {
                    self.active_lanes.push(Some(p));
                    merge_lanes.push(self.active_lanes.len() - 1);
                }
            }
        }

        // 3. Compute active mask and maximum rendered column
        let mut active_mask: u16 = 0;
        let mut max_col = lane_idx;
        for &m in &merge_lanes {
            max_col = max_col.max(m);
        }
        for (i, lane) in self.active_lanes.iter().enumerate().take(MAX_GRAPH_LANES) {
            if lane.is_some() || i == lane_idx {
                active_mask |= 1 << i;
                max_col = max_col.max(i);
            }
        }

        let num_lanes = (max_col + 1).min(MAX_GRAPH_LANES);
        let min_merge = merge_lanes.iter().copied().min().unwrap_or(lane_idx);
        let max_merge = merge_lanes.iter().copied().max().unwrap_or(lane_idx);

        // 4. Assign precise glyph to each column
        let mut glyphs: u64 = 0;
        for i in 0..num_lanes {
            let glyph = if i == lane_idx {
                if is_root {
                    GraphGlyph::Initial
                } else if merge_lanes.iter().any(|&m| m > lane_idx) {
                    GraphGlyph::MergeFork
                } else if is_merge {
                    GraphGlyph::Merge
                } else {
                    GraphGlyph::Commit
                }
            } else if merge_lanes.contains(&i) {
                if i < lane_idx {
                    if (pre_existing_mask & (1 << i)) != 0 {
                        GraphGlyph::Fork
                    } else if i > min_merge {
                        GraphGlyph::MultiMerge
                    } else {
                        GraphGlyph::TurnDown
                    }
                } else if i < max_merge {
                    GraphGlyph::MultiMerge
                } else {
                    GraphGlyph::BranchMerge
                }
            } else if !merge_lanes.is_empty()
                && i > lane_idx.min(min_merge)
                && i < lane_idx.max(max_merge)
            {
                if self.active_lanes.get(i).and_then(|&l| l).is_some() {
                    GraphGlyph::CrossOver
                } else {
                    GraphGlyph::Horizontal
                }
            } else if self.active_lanes.get(i).and_then(|&l| l).is_some() {
                GraphGlyph::Vertical
            } else {
                GraphGlyph::Empty
            };

            glyphs |= ((glyph as u64) & 0x0F) << (i * 4);
        }

        // 5. Advance lane tracking to parents for subsequent commits
        if is_root {
            if lane_idx < self.active_lanes.len() {
                self.active_lanes[lane_idx] = None;
            }
        } else {
            let first_parent = parents[0];
            // Check if first parent is already active in another lane (branch join)
            if let Some(target) = self
                .active_lanes
                .iter()
                .position(|l| *l == Some(first_parent))
            {
                if target == lane_idx {
                    self.active_lanes[lane_idx] = Some(first_parent);
                } else {
                    if !is_merge && target < lane_idx {
                        // Connect intermediate lanes between target and lane_idx horizontally
                        for i in (target + 1)..lane_idx {
                            if i < MAX_GRAPH_LANES {
                                let conn_glyph =
                                    if self.active_lanes.get(i).and_then(|&l| l).is_some() {
                                        GraphGlyph::CrossOver as u64
                                    } else {
                                        GraphGlyph::Horizontal as u64
                                    };
                                let mask = !(0x0F << (i * 4));
                                glyphs = (glyphs & mask) | (conn_glyph << (i * 4));
                            }
                        }
                        // Update glyph at lane_idx to indicate branch join to the left
                        let join_val = GraphGlyph::Join as u64;
                        let mask = !(0x0F << (lane_idx * 4));
                        glyphs = (glyphs & mask) | (join_val << (lane_idx * 4));
                    } else if !is_merge && target > lane_idx && target < MAX_GRAPH_LANES {
                        // Connect intermediate lanes between lane_idx and target horizontally
                        for i in (lane_idx + 1)..target {
                            if i < MAX_GRAPH_LANES {
                                let conn_glyph =
                                    if self.active_lanes.get(i).and_then(|&l| l).is_some() {
                                        GraphGlyph::CrossOver as u64
                                    } else {
                                        GraphGlyph::Horizontal as u64
                                    };
                                let mask = !(0x0F << (i * 4));
                                glyphs = (glyphs & mask) | (conn_glyph << (i * 4));
                            }
                        }
                        let target_val = GraphGlyph::CrossOver as u64;
                        let mask = !(0x0F << (target * 4));
                        glyphs = (glyphs & mask) | (target_val << (target * 4));
                    }
                    self.active_lanes[lane_idx] = None;
                }
            } else if lane_idx < self.active_lanes.len() {
                self.active_lanes[lane_idx] = Some(first_parent);
            }
        }

        // Trim trailing None entries
        while self.active_lanes.last() == Some(&None) {
            self.active_lanes.pop();
        }

        CompactGraphRow::new(
            lane_idx as u8,
            num_lanes as u8,
            is_merge,
            active_mask,
            glyphs,
        )
    }

    /// Clears internal state and frees allocated memory.
    pub fn clear(&mut self) {
        self.active_lanes.clear();
    }

    /// Shrinks internal vectors to fit current contents.
    pub fn shrink_to_fit(&mut self) {
        self.active_lanes.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_row_size_efficiency() {
        assert!(std::mem::size_of::<CompactGraphRow>() <= 16);
    }

    #[test]
    fn test_glyph_conversions_utf8_and_ascii() {
        assert_eq!(GraphGlyph::Commit.as_str(LineGraphics::Utf8), " ∙");
        assert_eq!(GraphGlyph::Commit.as_str(LineGraphics::Ascii), " *");

        assert_eq!(GraphGlyph::Merge.as_str(LineGraphics::Utf8), " ●");
        assert_eq!(GraphGlyph::Merge.as_str(LineGraphics::Ascii), " M");

        assert_eq!(GraphGlyph::MergeFork.as_str(LineGraphics::Utf8), "●─");
        assert_eq!(GraphGlyph::MergeFork.as_str(LineGraphics::Ascii), "M-");

        assert_eq!(GraphGlyph::Vertical.as_str(LineGraphics::Utf8), " │");
        assert_eq!(GraphGlyph::Vertical.as_str(LineGraphics::Ascii), " |");

        assert_eq!(GraphGlyph::Join.as_str(LineGraphics::Utf8), "─╯");
        assert_eq!(GraphGlyph::Join.as_str(LineGraphics::Ascii), "-'");

        assert_eq!(GraphGlyph::BranchMerge.as_str(LineGraphics::Utf8), "─╮");
        assert_eq!(GraphGlyph::BranchMerge.as_str(LineGraphics::Ascii), "-.");

        assert_eq!(GraphGlyph::Fork.as_str(LineGraphics::Utf8), " ├");
        assert_eq!(GraphGlyph::Fork.as_str(LineGraphics::Ascii), " +");

        assert_eq!(GraphGlyph::CrossOver.as_str(LineGraphics::Utf8), "─│");
        assert_eq!(GraphGlyph::CrossOver.as_str(LineGraphics::Ascii), "-|");

        assert_eq!(GraphGlyph::Initial.as_str(LineGraphics::Utf8), " ◎");
        assert_eq!(GraphGlyph::Initial.as_str(LineGraphics::Ascii), " I");
    }

    #[test]
    fn test_linear_history_layout() {
        let mut builder = GraphRowBuilder::new();
        let c1 = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let c2 = ObjectId::from_hex(b"2222222222222222222222222222222222222222").unwrap();
        let c3 = ObjectId::from_hex(b"3333333333333333333333333333333333333333").unwrap();

        let row3 = builder.process_commit(c3, &[c2]);
        assert_eq!(row3.to_graph_string(LineGraphics::Utf8), " ∙");
        assert_eq!(row3.to_graph_string(LineGraphics::Ascii), " *");

        let row2 = builder.process_commit(c2, &[c1]);
        assert_eq!(row2.to_graph_string(LineGraphics::Utf8), " ∙");
        assert_eq!(row2.to_graph_string(LineGraphics::Ascii), " *");

        let row1 = builder.process_commit(c1, &[]);
        assert_eq!(row1.to_graph_string(LineGraphics::Utf8), " ◎");
        assert_eq!(row1.to_graph_string(LineGraphics::Ascii), " I");
    }

    #[test]
    fn test_render_into_appends_without_allocating_a_string() {
        let mut builder = GraphRowBuilder::new();
        let c1 = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let c2 = ObjectId::from_hex(b"2222222222222222222222222222222222222222").unwrap();
        let row2 = builder.process_commit(c2, &[c1]);
        let row1 = builder.process_commit(c1, &[]);

        // Appending consecutive rows into one buffer equals concatenating the
        // owned-`String` renderings, and existing content is preserved.
        let mut buf = String::from("prefix:");
        row2.render_into(&mut buf, LineGraphics::Utf8);
        row1.render_into(&mut buf, LineGraphics::Utf8);
        assert_eq!(
            buf,
            format!(
                "prefix:{}{}",
                row2.to_graph_string(LineGraphics::Utf8),
                row1.to_graph_string(LineGraphics::Utf8)
            )
        );

        // The colored variant keeps the same contract, trailing reset included.
        let mut colored = String::from("prefix:");
        row2.render_colored_into(&mut colored, LineGraphics::Utf8);
        assert_eq!(
            colored,
            format!(
                "prefix:{}",
                row2.to_colored_graph_string(LineGraphics::Utf8)
            )
        );
    }

    #[test]
    fn test_merge_and_join_history_layout() {
        let mut builder = GraphRowBuilder::new();
        let root = ObjectId::from_hex(b"0000000000000000000000000000000000000001").unwrap();
        let branch_a = ObjectId::from_hex(b"0000000000000000000000000000000000000002").unwrap();
        let branch_b = ObjectId::from_hex(b"0000000000000000000000000000000000000003").unwrap();
        let merge = ObjectId::from_hex(b"0000000000000000000000000000000000000004").unwrap();

        // Merge commit merging branch_a (lane 0) and branch_b (lane 1)
        let row_m = builder.process_commit(merge, &[branch_a, branch_b]);
        assert_eq!(row_m.to_graph_string(LineGraphics::Utf8), "●──╮");
        assert_eq!(row_m.to_graph_string(LineGraphics::Ascii), "M--.");

        // Commit on branch_a
        let row_a = builder.process_commit(branch_a, &[root]);
        assert_eq!(row_a.to_graph_string(LineGraphics::Utf8), " ∙ │");
        assert_eq!(row_a.to_graph_string(LineGraphics::Ascii), " * |");

        // Commit on branch_b joining into root
        let row_b = builder.process_commit(branch_b, &[root]);
        assert_eq!(row_b.to_graph_string(LineGraphics::Utf8), " │─╯");
        assert_eq!(row_b.to_graph_string(LineGraphics::Ascii), " |-'");

        // Root commit
        let row_root = builder.process_commit(root, &[]);
        assert_eq!(row_root.to_graph_string(LineGraphics::Utf8), " ◎");
        assert_eq!(row_root.to_graph_string(LineGraphics::Ascii), " I");
    }

    #[test]
    fn test_crossover_layout() {
        let mut builder = GraphRowBuilder::new();
        let c_m = ObjectId::from_hex(b"0000000000000000000000000000000000000010").unwrap();
        let p_1 = ObjectId::from_hex(b"0000000000000000000000000000000000000011").unwrap();
        let p_2 = ObjectId::from_hex(b"0000000000000000000000000000000000000012").unwrap();
        let p_3 = ObjectId::from_hex(b"0000000000000000000000000000000000000013").unwrap();

        // Seed lane 1 with p_2
        builder.active_lanes.push(Some(c_m)); // lane 0
        builder.active_lanes.push(Some(p_2)); // lane 1 (active pass-through)

        // c_m merges p_1 (lane 0) and p_3 (lane 2) crossing over lane 1
        let row = builder.process_commit(c_m, &[p_1, p_3]);
        assert_eq!(row.to_graph_string(LineGraphics::Utf8), "●──│─╮");
        assert_eq!(row.to_graph_string(LineGraphics::Ascii), "M--|-.");
    }

    #[test]
    fn test_colored_graph_string_visible_width() {
        let row = CompactGraphRow::new(0, 2, true, 0b11, 0x63); // MergeFork at 0, BranchMerge at 1
        let colored = row.to_colored_graph_string(LineGraphics::Utf8);
        let plain = row.to_graph_string(LineGraphics::Utf8);

        assert_eq!(plain, "●──╮");
        assert!(colored.contains("\x1b["));
        assert_eq!(tigrs_core::ansi::visible_width(&colored), 4);
    }

    #[test]
    fn test_octopus_merge_layout() {
        let mut builder = GraphRowBuilder::new();
        let c_m = ObjectId::from_hex(b"0000000000000000000000000000000000000020").unwrap();
        let p1 = ObjectId::from_hex(b"0000000000000000000000000000000000000021").unwrap();
        let p2 = ObjectId::from_hex(b"0000000000000000000000000000000000000022").unwrap();
        let p3 = ObjectId::from_hex(b"0000000000000000000000000000000000000023").unwrap();

        let row = builder.process_commit(c_m, &[p1, p2, p3]);
        assert_eq!(row.num_lanes, 3);
        assert!(row.is_merge);
        assert_eq!(row.to_graph_string(LineGraphics::Utf8), "●──┬─╮");
        assert_eq!(row.to_graph_string(LineGraphics::Ascii), "M--+-.");
    }

    #[test]
    fn test_boundary_commit_glyph() {
        assert_eq!(GraphGlyph::Boundary.as_str(LineGraphics::Utf8), " ◯");
        assert_eq!(GraphGlyph::Boundary.as_str(LineGraphics::Ascii), " o");
        assert!(GraphGlyph::Boundary.is_commit_node());
    }

    #[test]
    fn test_lane_overflow_capping() {
        let mut builder = GraphRowBuilder::new();
        // Generate 20 dummy commits to exceed MAX_GRAPH_LANES (16)
        let mut parents = Vec::new();
        for i in 0..20 {
            let mut hex = [b'0'; 40];
            hex[38] = b'0' + (i / 10) as u8;
            hex[39] = b'0' + (i % 10) as u8;
            parents.push(ObjectId::from_hex(&hex).unwrap());
        }

        let child = ObjectId::from_hex(b"ffffffffffffffffffffffffffffffffffffffff").unwrap();
        let row = builder.process_commit(child, &parents);

        assert!(row.num_lanes as usize <= MAX_GRAPH_LANES);
        let s = row.to_graph_string(LineGraphics::Utf8);
        assert_eq!(
            tigrs_core::ansi::visible_width(&s),
            (row.num_lanes as usize) * 2
        );
    }

    #[test]
    fn test_builder_clear_and_shrink() {
        let mut builder = GraphRowBuilder::new();
        let c1 = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let c2 = ObjectId::from_hex(b"2222222222222222222222222222222222222222").unwrap();
        let _ = builder.process_commit(c2, &[c1]);
        assert!(!builder.active_lanes.is_empty());

        builder.clear();
        assert!(builder.active_lanes.is_empty());
        builder.shrink_to_fit();
    }

    #[test]
    fn test_graph_additional_coverage() {
        // 1. GraphGlyph::from_u8 for all variants
        for i in 0..=16 {
            let glyph = GraphGlyph::from_u8(i);
            let _ = glyph.as_str(LineGraphics::Utf8);
            let _ = glyph.as_str(LineGraphics::Ascii);
        }

        // Specific glyph representations
        assert_eq!(GraphGlyph::Empty.as_str(LineGraphics::Utf8), "  ");
        assert_eq!(GraphGlyph::Empty.as_str(LineGraphics::Ascii), "  ");
        assert_eq!(GraphGlyph::TurnDown.as_str(LineGraphics::Utf8), " ╭");
        assert_eq!(GraphGlyph::TurnDown.as_str(LineGraphics::Ascii), " .");
        assert_eq!(GraphGlyph::Horizontal.as_str(LineGraphics::Utf8), "──");
        assert_eq!(GraphGlyph::Horizontal.as_str(LineGraphics::Ascii), "--");
        assert_eq!(GraphGlyph::CrossMerge.as_str(LineGraphics::Utf8), "─┼");
        assert_eq!(GraphGlyph::MultiBranch.as_str(LineGraphics::Utf8), "─┴");

        // 2. Default for GraphRowBuilder
        let builder = GraphRowBuilder::default();
        assert!(builder.active_lanes.is_empty());

        // 3. CompactGraphRow glyph_at out of bounds
        let row = CompactGraphRow::new(0, 1, false, 1, 0);
        assert_eq!(row.glyph_at(20), GraphGlyph::Empty);

        // 4. to_colored_graph_string with empty glyph clearing active color
        let glyphs = 1u64;
        let row2 = CompactGraphRow::new(0, 2, false, 0b11, glyphs);
        let rendered = row2.to_colored_graph_string(LineGraphics::Utf8);
        assert!(rendered.contains("  "));
    }

    #[test]
    fn test_graph_octopus_merge_and_lane_overflow_and_disjoint_root() {
        let oid = |n: u8| {
            let mut hex = [b'0'; 40];
            hex[38] = b'0' + (n / 10);
            hex[39] = b'0' + (n % 10);
            ObjectId::from_hex(&hex).unwrap()
        };

        let mut builder = GraphRowBuilder::new();
        // 1. Seed lane 0 and lane 1, then retire lane 0 via a disjoint root commit
        let r0 = oid(1);
        let m_right = oid(2);
        let p_first = oid(3);
        let p_second_left = oid(4);
        let _ = builder.process_commit(oid(5), &[r0, m_right]);
        // r0 is a root commit in lane 0 -> frees lane 0 while lane 1 stays active with m_right
        let row_disjoint_root = builder.process_commit(r0, &[]);
        assert_eq!(row_disjoint_root.glyph_at(0), GraphGlyph::Initial);
        assert_eq!(row_disjoint_root.glyph_at(1), GraphGlyph::Vertical);

        // 2. Merge commit in lane 1 (`m_right`) allocating secondary parent `p_second_left` into free lane 0 (`i < lane_idx`)
        let row_rtl_merge = builder.process_commit(m_right, &[p_first, p_second_left]);
        assert_eq!(row_rtl_merge.glyph_at(0), GraphGlyph::TurnDown);
        assert_eq!(row_rtl_merge.glyph_at(1), GraphGlyph::Merge);
        assert_eq!(row_rtl_merge.to_graph_string(LineGraphics::Utf8), " ╭ ●");
        assert_eq!(row_rtl_merge.to_graph_string(LineGraphics::Ascii), " . M");

        // 3. Merge commit in lane 1 where secondary parent `p_second_left` is ALREADY active in lane 0 (`Fork`)
        let next_in_lane1 = oid(6);
        builder.active_lanes[1] = Some(next_in_lane1);
        let row_rtl_fork = builder.process_commit(next_in_lane1, &[p_first, p_second_left]);
        assert_eq!(row_rtl_fork.glyph_at(0), GraphGlyph::Fork);
        assert_eq!(row_rtl_fork.glyph_at(1), GraphGlyph::Merge);
        assert_eq!(row_rtl_fork.to_graph_string(LineGraphics::Utf8), " ├ ●");
        assert_eq!(row_rtl_fork.to_graph_string(LineGraphics::Ascii), " + M");

        // 4. Non-merge commit in lane 0 joining into `p_first` in lane 1 (`target > lane_idx`)
        let row_join_right = builder.process_commit(p_second_left, &[p_first]);
        assert_eq!(row_join_right.glyph_at(0), GraphGlyph::Commit);
        assert_eq!(row_join_right.glyph_at(1), GraphGlyph::CrossOver);
    }
}
