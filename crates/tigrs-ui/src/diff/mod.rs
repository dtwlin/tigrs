// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Diff presentation, layout, formatting, and rendering subsystem.

pub mod align;
pub mod document;
pub mod expand;
pub mod formatter;
pub mod layout;
pub mod paint;
pub mod theme;

pub use align::{AlignPolicy, align_runs};
pub use document::{DiffDocument, DiffLineType, HunkLocation, LineMarker, RowCell, RowPair};
pub use expand::{ExpandedItem, ExpandedLine, expand_file_items, expand_file_lines};
pub use formatter::run_external_formatter;
pub use layout::{
    BlobLineProvider, build_diff_document, build_diff_document_with_state,
    classify_commit_body_lines, detect_moved_blocks, expand_tabs, is_commit_trailer,
};
pub use paint::{DiffViewport, paint_diff, style_for_type, write_style};
pub use theme::{Attrs, DiffStyle, DiffTheme, style_from_spec};

use crate::options::{
    DiffIndicator, DiffLayout, DiffPresentation, IgnoreSpace, UiThemeId, ViewOptions,
    WordDiffPairing,
};
use crate::term_cap::ColorProfile;
use clru::CLruCache;
use std::borrow::Borrow;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Arc;
use tigrs_core::MemoryProfile;
use tigrs_git::ObjectId;

/// Composite cache key capturing every option that shapes a [`DiffDocument`] layout.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(clippy::struct_excessive_bools)]
pub struct DiffRenderKey {
    /// Header presentation mode (`Fancy` vs `Classic`).
    pub presentation: DiffPresentation,
    /// Layout mode (`Unified` vs `SideBySide`).
    pub layout: DiffLayout,
    /// Line indicator mode (`Sign`, `Strip`, `Column`).
    pub indicator: DiffIndicator,
    /// Intra-line word-diff pairing heuristic.
    pub word_diff_pairing: WordDiffPairing,
    /// Whether `word-diff` is active.
    pub word_diff: bool,
    /// Whether moved-block detection (`color-moved`) is active.
    pub color_moved: bool,
    /// Number of context lines around hunks (`-U<N>`).
    pub diff_context: usize,
    /// Whitespace ignore setting.
    pub ignore_space: IgnoreSpace,
    /// Tab stop width.
    pub tab_size: usize,
    /// Whether syntax highlighting is enabled.
    pub syntax_highlighting: bool,
    /// Active syntax theme name.
    pub syntax_theme: String,
    /// Active UI view color theme.
    pub ui_theme: UiThemeId,
    /// External diff formatter command, if any.
    pub diff_formatter: String,
    /// Active terminal color gamut.
    pub color_profile: ColorProfile,
    /// Line graphics mode (`Utf8` vs `Ascii`).
    pub line_graphics: tigrs_core::LineGraphics,
    /// Action hint chips mode (`Auto`, `Always`, `Never`).
    pub diff_hints: tigrs_core::DiffHintsMode,
    /// Auto-collapse generated files (`linguist-generated`).
    pub diff_collapse_generated: bool,
}

impl DiffRenderKey {
    /// Builds a [`DiffRenderKey`] from the active [`ViewOptions`] and [`ColorProfile`].
    #[must_use]
    pub fn from_options(opts: &ViewOptions, color_profile: ColorProfile) -> Self {
        Self {
            presentation: opts.diff_presentation,
            layout: opts.diff_layout,
            indicator: opts.diff_indicator,
            word_diff_pairing: opts.word_diff_pairing,
            word_diff: opts.word_diff,
            color_moved: opts.color_moved,
            diff_context: opts.diff_context,
            ignore_space: opts.ignore_space,
            tab_size: opts.tab_size,
            syntax_highlighting: opts.syntax_highlighting,
            syntax_theme: opts.syntax_theme.clone(),
            ui_theme: opts.ui_theme,
            diff_formatter: if opts.read_only {
                String::new()
            } else {
                opts.diff_formatter.clone()
            },
            color_profile,
            line_graphics: opts.line_graphics,
            diff_hints: opts.diff_hints,
            diff_collapse_generated: opts.diff_collapse_generated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DiffDocCacheKey(ObjectId, DiffRenderKey);

trait DiffDocLookupKey {
    fn oid(&self) -> &ObjectId;
    fn render_key(&self) -> &DiffRenderKey;
}

impl DiffDocLookupKey for DiffDocCacheKey {
    fn oid(&self) -> &ObjectId {
        &self.0
    }
    fn render_key(&self) -> &DiffRenderKey {
        &self.1
    }
}

impl DiffDocLookupKey for (&ObjectId, &DiffRenderKey) {
    fn oid(&self) -> &ObjectId {
        self.0
    }
    fn render_key(&self) -> &DiffRenderKey {
        self.1
    }
}

impl Hash for dyn DiffDocLookupKey + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.oid().hash(state);
        self.render_key().hash(state);
    }
}

impl PartialEq for dyn DiffDocLookupKey + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.oid() == other.oid() && self.render_key() == other.render_key()
    }
}

impl Eq for dyn DiffDocLookupKey + '_ {}

impl<'a> Borrow<dyn DiffDocLookupKey + 'a> for DiffDocCacheKey {
    fn borrow(&self) -> &(dyn DiffDocLookupKey + 'a) {
        self
    }
}

fn approx_doc_bytes(doc: &DiffDocument) -> usize {
    256 + doc.rows.len() * 448
}

/// Bounded hash-indexed LRU cache of laid-out [`DiffDocument`] instances keyed by `(ObjectId, DiffRenderKey)`.
#[derive(Debug)]
pub struct DiffDocumentCache {
    lru: CLruCache<DiffDocCacheKey, (Arc<DiffDocument>, bool, usize)>,
    used_bytes: usize,
    max_bytes: usize,
}

impl Default for DiffDocumentCache {
    fn default() -> Self {
        Self::with_profile(MemoryProfile::default())
    }
}

impl DiffDocumentCache {
    /// Creates a new [`DiffDocumentCache`] with the given maximum entry count and a default byte budget.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).expect("non-zero capacity");
        Self {
            lru: CLruCache::new(cap),
            used_bytes: 0,
            max_bytes: MemoryProfile::Greedy.diff_document_cache_byte_budget(),
        }
    }

    /// Creates a new [`DiffDocumentCache`] sized according to `profile`.
    #[must_use]
    pub fn with_profile(profile: MemoryProfile) -> Self {
        let cap = NonZeroUsize::new(profile.diff_document_cache_capacity().max(1))
            .expect("non-zero capacity");
        Self {
            lru: CLruCache::new(cap),
            used_bytes: 0,
            max_bytes: profile.diff_document_cache_byte_budget(),
        }
    }

    /// Reconfigures the capacity and byte budget for a new [`MemoryProfile`].
    pub fn reconfigure(&mut self, profile: MemoryProfile) {
        let cap = profile.diff_document_cache_capacity().max(1);
        self.max_bytes = profile.diff_document_cache_byte_budget();
        while !self.lru.is_empty() && (self.lru.len() > cap || self.used_bytes > self.max_bytes) {
            if let Some((_, (_, _, evicted_bytes))) = self.lru.pop_back() {
                self.used_bytes = self.used_bytes.saturating_sub(evicted_bytes);
            } else {
                break;
            }
        }
        if let Some(nz) = NonZeroUsize::new(cap) {
            self.lru.resize(nz);
        }
    }

    /// Looks up a cached `(Arc<DiffDocument>, is_external_formatter)` by `(commit_id, key)`,
    /// promoting the entry to most-recently-used on hit without allocating.
    pub fn get(
        &mut self,
        commit_id: ObjectId,
        key: &DiffRenderKey,
    ) -> Option<(Arc<DiffDocument>, bool)> {
        let lookup = (&commit_id, key);
        self.lru
            .get(&lookup as &dyn DiffDocLookupKey)
            .map(|(doc, ext, _)| (Arc::clone(doc), *ext))
    }

    /// Inserts a laid-out `(Arc<DiffDocument>, is_external_formatter)` into the cache,
    /// evicting the least-recently-used entries when at capacity or byte budget.
    pub fn insert(
        &mut self,
        commit_id: ObjectId,
        key: DiffRenderKey,
        doc: Arc<DiffDocument>,
        is_external_formatter: bool,
    ) {
        let entry_bytes = approx_doc_bytes(&doc);
        let full_key = DiffDocCacheKey(commit_id, key);
        if let Some((_, _, old_bytes)) = self.lru.pop(&full_key) {
            self.used_bytes = self.used_bytes.saturating_sub(old_bytes);
        }
        if entry_bytes > self.max_bytes {
            return;
        }
        while !self.lru.is_empty()
            && (self.lru.len() >= self.lru.capacity()
                || self.used_bytes + entry_bytes > self.max_bytes)
        {
            if let Some((_, (_, _, evicted_bytes))) = self.lru.pop_back() {
                self.used_bytes = self.used_bytes.saturating_sub(evicted_bytes);
            } else {
                break;
            }
        }
        self.lru
            .put(full_key, (doc, is_external_formatter, entry_bytes));
        self.used_bytes += entry_bytes;
    }

    /// Clears all cached documents.
    pub fn clear(&mut self) {
        self.lru.clear();
        self.used_bytes = 0;
    }

    /// Returns the number of cached documents currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lru.len()
    }

    /// Returns `true` if no documents are currently cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lru.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigrs_core::DiffHintsMode;

    #[test]
    fn test_diff_render_key_distinguishes_all_header_and_layout_options() {
        let mut opts = ViewOptions::default();
        let base_key = DiffRenderKey::from_options(&opts, ColorProfile::TrueColor);

        // 1. diff_presentation
        opts.diff_presentation = DiffPresentation::Fancy;
        assert_ne!(
            base_key,
            DiffRenderKey::from_options(&opts, ColorProfile::TrueColor)
        );
        opts.diff_presentation = DiffPresentation::Banner;

        // 2. diff_hints
        opts.diff_hints = DiffHintsMode::Never;
        assert_ne!(
            base_key,
            DiffRenderKey::from_options(&opts, ColorProfile::TrueColor)
        );
        opts.diff_hints = DiffHintsMode::Auto;

        // 3. diff_collapse_generated
        opts.diff_collapse_generated = false;
        assert_ne!(
            base_key,
            DiffRenderKey::from_options(&opts, ColorProfile::TrueColor)
        );
        opts.diff_collapse_generated = true;

        // 4. line_graphics
        opts.line_graphics = tigrs_core::LineGraphics::Ascii;
        assert_ne!(
            base_key,
            DiffRenderKey::from_options(&opts, ColorProfile::TrueColor)
        );
        opts.line_graphics = tigrs_core::LineGraphics::Utf8;

        // 5. color_profile
        assert_ne!(
            base_key,
            DiffRenderKey::from_options(&opts, ColorProfile::Monochrome)
        );
    }

    #[test]
    fn test_diff_document_cache_lru_and_capacity_resizing() {
        let mut cache = DiffDocumentCache::new(2);
        assert!(cache.is_empty());

        let oid1 = ObjectId::from_bytes_or_panic(&[0x01; 20]);
        let oid2 = ObjectId::from_bytes_or_panic(&[0x02; 20]);
        let oid3 = ObjectId::from_bytes_or_panic(&[0x03; 20]);
        let key = DiffRenderKey::from_options(&ViewOptions::default(), ColorProfile::TrueColor);
        let empty_doc = Arc::new(DiffDocument::default());

        cache.insert(oid1, key.clone(), Arc::clone(&empty_doc), false);
        cache.insert(oid2, key.clone(), Arc::clone(&empty_doc), true);
        assert_eq!(cache.len(), 2);

        // Promote oid1 to MRU so oid2 is evicted next
        assert!(!cache.get(oid1, &key).unwrap().1);
        cache.insert(oid3, key.clone(), Arc::clone(&empty_doc), false);
        assert_eq!(cache.len(), 2);
        assert!(cache.get(oid1, &key).is_some());
        assert!(cache.get(oid2, &key).is_none());
        assert!(cache.get(oid3, &key).is_some());

        // Reconfiguring to Greedy preserves valid entries
        cache.reconfigure(MemoryProfile::Greedy);
        assert_eq!(cache.len(), 2);

        cache.clear();
        assert!(cache.is_empty());
    }
}
