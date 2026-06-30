//! Content-keyed glyph (galley) + image-texture caches for the grid painter.
//!
//! `GalleyCache` memoises single-glyph egui galleys (keyed by char + colour +
//! pass + style) so the per-cell grid paint never re-shapes a glyph it drew last
//! frame; `ImageTextureCache` memoises uploaded inline-image textures. Both prune
//! what they did not touch each frame. Extracted from the `egui_app` god-module;
//! re-exported via `pub(crate) use glyph_cache::*`. Behaviour unchanged.

use std::collections::{HashMap, HashSet};

use eframe::egui;

use super::grid::PaneId;

/// Which of a grid row's up-to-three painted galleys this cache entry holds: the
/// crisp main pass in the runs' real colours, or one of the two pure-channel
/// chromatic-aberration ghost passes. Each pass for a row is keyed separately so
/// the ghost galleys (drawn in a single override colour) never collide with the
/// crisp galley (drawn in the runs' real per-cell colours).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RowPass {
    /// The crisp main pass — each run keeps its real SGR colour.
    Main,
    /// The pure-red ghost (chromatic aberration), shifted left.
    GhostRed,
    /// The pure-blue ghost (chromatic aberration), shifted right.
    GhostBlue,
}

/// Content-keyed single-glyph galley cache for [`paint_grid_native`]. The grid
/// is painted one glyph per CELL, each positioned at its computed `col * cw`
/// origin — so layout is FONT-ADVANCE-INDEPENDENT: a wide or fallback glyph can
/// never shift another cell, and there is no "scattered text under a proportional
/// font" failure mode (the reason the per-run approach was reverted). The cache
/// is keyed purely by glyph CONTENT (char + colour + pass + style), so the same
/// glyph reused across many cells shares ONE laid-out galley (the galley is
/// position-independent — painted at each cell's origin). Pruned to the glyphs
/// seen this frame so the map stays bounded by distinct on-screen glyph+colour
/// combinations (small). Cleared wholesale on a font re-install (the cached
/// galleys reference the old font atlas).
#[derive(Default)]
pub(crate) struct GalleyCache {
    /// content key -> laid-out single-glyph galley.
    glyphs: HashMap<u64, std::sync::Arc<egui::Galley>>,
    /// Content keys drawn THIS frame, for the end-of-frame prune.
    seen_this_frame: HashSet<u64>,
}

impl GalleyCache {
    /// Lay out a single glyph (content `key`), reusing the cached galley when the
    /// glyph+colour+style is unchanged. `build` constructs the
    /// [`egui::text::LayoutJob`] on a miss. Records the key as seen this frame.
    pub(crate) fn glyph(
        &mut self,
        painter: &egui::Painter,
        key: u64,
        build: impl FnOnce() -> egui::text::LayoutJob,
    ) -> std::sync::Arc<egui::Galley> {
        self.seen_this_frame.insert(key);
        if let Some(galley) = self.glyphs.get(&key) {
            return galley.clone();
        }
        let galley = painter.layout_job(build());
        self.glyphs.insert(key, galley.clone());
        galley
    }

    /// Drop glyph galleys NOT drawn this frame (glyphs that scrolled off / a
    /// closed pane) and reset the per-frame seen set. Called once at the end of
    /// [`C0pl4ndApp::grid_ui`].
    pub(crate) fn prune_unseen(&mut self) {
        if self.seen_this_frame.is_empty() {
            // Nothing painted this frame (e.g. every pane errored): keep entries
            // so a transient empty frame does not evict the whole cache.
            return;
        }
        self.glyphs.retain(|k, _| self.seen_this_frame.contains(k));
        self.seen_this_frame.clear();
    }

    /// Drop every entry — used when the font stack is re-installed (the cached
    /// galleys reference the previous font atlas and must be relaid).
    pub(crate) fn clear(&mut self) {
        self.glyphs.clear();
        self.seen_this_frame.clear();
    }
}

/// Content cache key for one painted glyph: the char, its RGB colour, the pass
/// (crisp / chromatic-ghost), and the shared per-frame style bits (font size +
/// fallback fg). Two cells with the same glyph+colour+style share one galley.
///
/// INVARIANT: the key must capture EVERY input that changes the laid-out galley
/// produced by [`build_glyph_job`]. Today that function renders only `char` +
/// `font` (size via `style_key`) + `color` — SGR attributes (bold / italic /
/// underline) are intentionally NOT rendered, so they are correctly absent here.
/// If [`build_glyph_job`] is ever extended to honour `CellFlags`, those flag bits
/// MUST be added to this key, or two visually-different cells (e.g. bold vs
/// regular `a`) would collide on one cached galley.
pub(crate) fn glyph_cache_key(c: char, rgb: (u8, u8, u8), pass: RowPass, style_key: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    style_key.hash(&mut h);
    c.hash(&mut h);
    rgb.hash(&mut h);
    (pass as u8).hash(&mut h);
    h.finish()
}

/// Build the [`egui::text::LayoutJob`] for a SINGLE glyph in `color`. Used on a
/// glyph-cache miss; the resulting galley is painted at the cell's `col * cw`
/// origin.
pub(crate) fn build_glyph_job(
    c: char,
    font: &egui::FontId,
    color: egui::Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let mut buf = [0u8; 4];
    job.append(
        c.encode_utf8(&mut buf),
        0.0,
        egui::text::TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    );
    job
}

/// Texture-cache key for an inline image: `(pane, abs line, col, width, height)`.
/// Stable across a scrollback-view scroll (so a visible image uploads once), and
/// distinct per pane so two panes' images never collide.
pub(crate) type ImageKey = (PaneId, usize, usize, usize, usize);

/// GPU-texture cache for inline images (Sixel / Kitty graphics), mirroring
/// [`GalleyCache`]'s seen-set + end-of-frame prune so textures for images that
/// scrolled off (or a closed pane) are dropped and GPU memory cannot grow
/// without bound. An `egui::TextureHandle` frees its GPU texture on drop, so
/// pruning an entry releases the texture.
#[derive(Default)]
pub(crate) struct ImageTextureCache {
    map: HashMap<ImageKey, egui::TextureHandle>,
    seen_this_frame: HashSet<ImageKey>,
}

impl ImageTextureCache {
    /// Return the texture id for `key`, marking it seen this frame. On a cache
    /// HIT the cached id is returned and `fetch` is NOT called. On a MISS `fetch`
    /// is invoked to produce `(width, height, rgba)`, the texture is uploaded
    /// once, and its id returned — so pixel bytes are copied at most once per
    /// uploaded texture (never on a hit). `None` when `fetch` yields nothing
    /// (e.g. the image was evicted between the metadata sweep and the fetch).
    pub(crate) fn get_or_upload(
        &mut self,
        ctx: &egui::Context,
        key: ImageKey,
        fetch: impl FnOnce() -> Option<(usize, usize, Vec<u8>)>,
    ) -> Option<egui::TextureId> {
        self.seen_this_frame.insert(key);
        if let Some(tex) = self.map.get(&key) {
            return Some(tex.id());
        }
        let (w, h, rgba) = fetch()?;
        // NEAREST keeps pixel art (sixel) crisp; the source is already RGBA.
        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        let tex = ctx.load_texture(
            format!("c0pl4nd-img-{}-{}-{}", key.0 .0, key.1, key.2),
            image,
            egui::TextureOptions::NEAREST,
        );
        let id = tex.id();
        self.map.insert(key, tex);
        Some(id)
    }

    /// Drop textures not touched this frame (images scrolled off / closed pane)
    /// and reset the seen set. Called once at the end of `grid_ui`.
    pub(crate) fn prune_unseen(&mut self) {
        self.map.retain(|k, _| self.seen_this_frame.contains(k));
        self.seen_this_frame.clear();
    }
}
