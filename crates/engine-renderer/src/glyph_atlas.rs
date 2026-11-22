//! The glyph atlas: font8x8 bitmap data packed into one R8 texture and
//! the UV table + quad emitter that replace the per-pixel overlay text
//! rasterization (the Cycle 6-A foundation — see
//! `docs/CYCLE6-6A-BRIEF.md`).
//!
//! THE COST BEING REMOVED (measured, `render_benchmark` section D):
//! the former `push_text_px` emitted ONE QUAD PER SET FONT PIXEL —
//! ~750 glyphs x ~20 set pixels x 6 vertices ~= 90,000 vertices
//! (~2.2 MB) rebuilt on the CPU EVERY FRAME. The atlas form emits ONE
//! QUAD PER GLYPH (~4,500 vertices for the same text, a ~20x
//! reduction in both CPU rasterization and upload bytes).
//!
//! DIVISION OF THIS MODULE (the 6-A scaffold boundary):
//! - DETERMINISTIC PURE (implemented here, fully tested): the atlas
//!   bitmap construction from `font8x8::legacy::BASIC_LEGACY`, the
//!   packed glyph-atlas layout, the UV lookup table, and the
//!   glyph-quad emitter. No Vulkan, no allocation beyond the returned
//!   arrays, bit-stable and cross-platform identical.
//! - VULKAN UPLOAD (stubbed for the implementer): turning
//!   [`GlyphAtlas::atlas_bitmap`] into a DEVICE_LOCAL R8 image with an
//!   immutable sampler — `textures.rs` + the integration points named
//!   in the brief.
//!
//! DETERMINISM CONTRACT: the atlas layout and UV table are pure
//! functions of the font8x8 data (version-pinned in Cargo.lock) —
//! identical on every platform and every build. The quad emitter
//! reproduces the legacy metrics EXACTLY (9.0-px advance, 10.0-px
//! line height, the same MAX_GLYPHS cap, the same non-finite-input
//! refusal) so the on-screen text geometry is unchanged; only the
//! rendering mechanism changes (per-pixel quads -> sampled atlas
//! quads).
//!
//! FONT COVERAGE: `BASIC_LEGACY` carries the 128 ASCII code points —
//! the atlas is a 16x8 cell grid (128x64 texels). `uv_rect` folds any
//! u8 with `& 0x7F` so it is TOTAL over u8 (the emitter maps
//! non-ASCII characters to `?` exactly as the legacy form did before
//! the atlas is ever consulted).

/// The atlas grid: 16 columns x 8 rows of 8x8-texel glyphs.
pub const CELL_COLS: u32 = 16;
pub const CELL_ROWS: u32 = 8;
pub const GLYPH_SIZE: u32 = 8;
/// The atlas bitmap extent: 128 x 64 texels, R8 (255 = lit, 0 = empty).
pub const ATLAS_WIDTH: u32 = CELL_COLS * GLYPH_SIZE;
pub const ATLAS_HEIGHT: u32 = CELL_ROWS * GLYPH_SIZE;

/// One glyph quad for the atlas-rendered overlay: pixel-space
/// position rect (the SAME coordinates the legacy per-pixel form
/// emitted, pre-normalization) plus the atlas UV rect. Color carries
/// through for tinting (the atlas is luminance; the fragment shader
/// multiplies by this color).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphQuad {
    /// Pixel-space destination rect: [x0, y0, x1, y1] — UNNORMALIZED
    /// (the pipeline maps to NDC exactly as the legacy form did).
    pub dst_px: [f32; 4],
    /// Atlas UV rect: [u0, v0, u1, v1] — normalized within the atlas.
    pub uv: [f32; 4],
    /// The tint color, identical semantics to the legacy vertex color.
    pub color: [f32; 4],
}

/// The deterministic glyph atlas: the packed bitmap and the pure UV
/// math. Construction is cheap (8 KiB) and happens once at renderer
/// init; the bitmap uploads to a single R8 image exactly once.
pub struct GlyphAtlas {
    /// The packed 128x64 R8 bitmap (row-major, y * ATLAS_WIDTH + x).
    atlas_bitmap: Vec<u8>,
}

impl GlyphAtlas {
    /// Builds the atlas from the pinned font8x8 legacy font. Pure and
    /// deterministic: the same bytes on every platform.
    pub fn new() -> GlyphAtlas {
        let mut atlas_bitmap = vec![0u8; (ATLAS_WIDTH * ATLAS_HEIGHT) as usize];
        for code in 0..128u8 {
            let glyph = font8x8::legacy::BASIC_LEGACY[code as usize];
            let (cell_x, cell_y) = ((code % 16) as u32, (code / 16) as u32);
            for (row, row_bits) in glyph.iter().enumerate() {
                for bit in 0..8 {
                    if row_bits & (1 << bit) != 0 {
                        let x = cell_x * GLYPH_SIZE + bit as u32;
                        let y = cell_y * GLYPH_SIZE + row as u32;
                        atlas_bitmap[(y * ATLAS_WIDTH + x) as usize] = 255;
                    }
                }
            }
        }
        GlyphAtlas { atlas_bitmap }
    }

    /// The packed bitmap for the one-time GPU upload (R8, row-major,
    /// ATLAS_WIDTH x ATLAS_HEIGHT).
    pub fn atlas_bitmap(&self) -> &[u8] {
        &self.atlas_bitmap
    }

    /// The atlas UV rect for a code point. TOTAL over u8: the code is
    /// folded with `& 0x7F` (the 128-entry ASCII font; the emitter
    /// maps non-ASCII characters to `?` before this is consulted, and
    /// the fold makes the function total for any direct caller).
    /// The cell extent is exact — adjacent glyph cells are
    /// zero-filled, so nearest sampling cannot bleed.
    pub fn uv_rect(&self, code: u8) -> [f32; 4] {
        let code = code & 0x7F;
        let (cell_x, cell_y) = ((code % 16) as u32, (code / 16) as u32);
        let x0 = cell_x * GLYPH_SIZE;
        let y0 = cell_y * GLYPH_SIZE;
        [
            x0 as f32 / ATLAS_WIDTH as f32,
            y0 as f32 / ATLAS_HEIGHT as f32,
            (x0 + GLYPH_SIZE) as f32 / ATLAS_WIDTH as f32,
            (y0 + GLYPH_SIZE) as f32 / ATLAS_HEIGHT as f32,
        ]
    }
}

impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new()
    }
}

/// The 6-A quad emitter: the atlas form of the legacy
/// `push_text_px`. Emits ONE [`GlyphQuad`] per character — the same
/// pixel-space geometry (9.0-px advance, 10.0-px newline height, the
/// same 6,000-glyph cap, non-finite inputs refused) so the rendered
/// text position/size is IDENTICAL to the legacy form; the shader
/// samples the atlas instead of the CPU rasterizing each texel.
///
/// The implementer wires this into the overlay path behind the
/// pixel-equivalence gate (the brief's acceptance criterion): the
/// capture comparison against the legacy output before the live path
/// switches.
pub fn push_glyph_quads(
    quads: &mut Vec<GlyphQuad>,
    origin: [f32; 2],
    scale: f32,
    text: &str,
    color: [f32; 4],
    atlas: &GlyphAtlas,
) {
    if !(scale > 0.0 && scale.is_finite() && origin.iter().chain(&color).all(|v| v.is_finite())) {
        return;
    }
    let [mut glyph_x, mut glyph_y] = origin;
    let mut glyph_count = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            glyph_x = origin[0];
            glyph_y += 10.0 * scale;
            continue;
        }
        let ascii = if ch.is_ascii() { ch as u8 } else { b'?' };
        quads.push(GlyphQuad {
            dst_px: [
                glyph_x,
                glyph_y,
                glyph_x + 8.0 * scale,
                glyph_y + 8.0 * scale,
            ],
            uv: atlas.uv_rect(ascii),
            color,
        });
        glyph_x += 9.0 * scale;
        glyph_count += 1;
        if glyph_count >= crate::overlay::MAX_GLYPHS {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The atlas is a pure function of the pinned font: 128x64, and
    /// every lit font texel lands in its mapped cell.
    #[test]
    fn atlas_layout_maps_every_code_point_deterministically() {
        let atlas = GlyphAtlas::new();
        assert_eq!(
            atlas.atlas_bitmap().len(),
            (ATLAS_WIDTH * ATLAS_HEIGHT) as usize
        );
        for code in 0..128u8 {
            let glyph = font8x8::legacy::BASIC_LEGACY[code as usize];
            let (cell_x, cell_y) = ((code % 16) as u32, (code / 16) as u32);
            for row in 0..8u32 {
                for bit in 0..8u32 {
                    let lit = glyph[row as usize] & (1 << bit) != 0;
                    let idx = ((cell_y * GLYPH_SIZE + row) * ATLAS_WIDTH
                        + cell_x * GLYPH_SIZE
                        + bit) as usize;
                    assert_eq!(
                        atlas.atlas_bitmap()[idx],
                        if lit { 255 } else { 0 },
                        "code {code} texel ({row},{bit})"
                    );
                }
            }
        }
    }

    /// The UV rects tile the atlas exactly: the 16x8 cells partition
    /// [0,1]x[0,1] with no overlap and no gap; the fold is total over
    /// u8.
    #[test]
    fn uv_rects_partition_the_atlas_exactly() {
        let atlas = GlyphAtlas::new();
        for code in 0..128u8 {
            let [u0, v0, u1, v1] = atlas.uv_rect(code);
            let (cx, cy) = (code % 16, code / 16);
            assert_eq!(u0, cx as f32 / 16.0);
            assert_eq!(v0, cy as f32 / 8.0);
            assert_eq!(u1, (cx as f32 + 1.0) / 16.0);
            assert_eq!(v1, (cy as f32 + 1.0) / 8.0);
        }
        // The fold: codes above the font's 128 entries map into the
        // grid (total over u8, mirroring the legacy '?' discipline).
        assert_eq!(atlas.uv_rect(0x80), atlas.uv_rect(0x00));
        assert_eq!(atlas.uv_rect(0xFF), atlas.uv_rect(0x7F));
    }

    /// The quad emitter reproduces the legacy text metrics: one quad
    /// per character, the 9.0-px advance, the 10.0-px newline height,
    /// the cap, and the non-finite refusal.
    #[test]
    fn glyph_quads_reproduce_the_legacy_text_metrics() {
        let atlas = GlyphAtlas::new();
        let color = [1.0, 1.0, 1.0, 1.0];
        let mut quads = Vec::new();
        push_glyph_quads(&mut quads, [10.0, 20.0], 2.0, "A\nB", color, &atlas);
        assert_eq!(quads.len(), 2, "one quad per character, newline excluded");
        // 'A' at origin, 8x8 texels x scale 2 -> 16x16 px.
        assert_eq!(quads[0].dst_px, [10.0, 20.0, 26.0, 36.0]);
        // 'B' on the next line: y advanced by 10.0 * scale = 20.
        assert_eq!(quads[1].dst_px, [10.0, 40.0, 26.0, 56.0]);
        // Distinct code points map to distinct cells.
        assert_ne!(quads[0].uv, quads[1].uv);
        // Non-finite inputs are refused (the legacy contract).
        let mut none = Vec::new();
        push_glyph_quads(&mut none, [f32::NAN, 0.0], 2.0, "A", color, &atlas);
        assert!(none.is_empty());
        // The cap holds.
        let text = "A".repeat(6_500);
        let mut capped = Vec::new();
        push_glyph_quads(&mut capped, [0.0, 0.0], 1.0, &text, color, &atlas);
        assert_eq!(capped.len(), crate::overlay::MAX_GLYPHS);
        // Non-ASCII maps to '?' (the legacy discipline).
        let mut question = Vec::new();
        push_glyph_quads(&mut question, [0.0, 0.0], 1.0, "\u{e9}", color, &atlas);
        assert_eq!(question[0].uv, atlas.uv_rect(b'?'));
    }
}
