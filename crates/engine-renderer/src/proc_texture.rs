//! The deterministic procedural textures (the Cycle 6-B scaffold —
//! see `docs/CYCLE6-6B-BRIEF.md`): seeded value-noise RGBA8 textures
//! for the corner scene's materials, generated in fixed operation
//! order — identical bytes on every platform and every build (the
//! 6-A atlas discipline applied to material albedo).
//!
//! 6-B's three textures: (0) wet asphalt — dark, low-frequency
//! variation with tight bright speckle (the specular comes from the
//! MATERIAL parameters, not the texture); (1) concrete facade —
//! mid-gray with block course lines; (2) dark stone base — cool,
//! high-frequency grain.

/// One texture's dimensions (square, power-of-two).
pub const TEXTURE_SIZE: u32 = 256;
/// The number of textures the corner scene defines.
pub const TEXTURE_COUNT: u32 = 3;

/// The seeded SplitMix64 finalizer (deterministic, cross-platform).
fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// A seeded hash for a lattice point: returns [0, 1).
fn hash01(seed: u64, x: i32, y: i32) -> f32 {
    let h = mix64(seed ^ ((x as u64) << 32) ^ (y as u64 & 0xFFFF_FFFF));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Smoothstep interpolation between lattice values (the value-noise
/// kernel — the standard 3t²−2t³ form, fixed order).
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Bilinear value noise at (fx, fy) over an integer lattice of the
/// given frequency (cells per texture). FIXED operation order.
fn value_noise(seed: u64, fx: f32, fy: f32, freq: u32) -> f32 {
    let x = fx * freq as f32;
    let y = fy * freq as f32;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let tx = smooth(x - x0 as f32);
    let ty = smooth(y - y0 as f32);
    let v00 = hash01(seed, x0, y0);
    let v10 = hash01(seed, x0 + 1, y0);
    let v01 = hash01(seed, x0, y0 + 1);
    let v11 = hash01(seed, x0 + 1, y0 + 1);
    let a = v00 + (v10 - v00) * tx;
    let b = v01 + (v11 - v01) * tx;
    a + (b - a) * ty
}

/// Generates texture `index` (0..TEXTURE_COUNT) as RGBA8, row-major,
/// TEXTURE_SIZE² pixels. Pure: the same bytes for the same index on
/// every platform.
pub fn procedural_texture(index: u32) -> Vec<u8> {
    assert!(index < TEXTURE_COUNT);
    let mut data = Vec::with_capacity((TEXTURE_SIZE * TEXTURE_SIZE * 4) as usize);
    // Distinct seed per texture.
    let seed = 0x6B_0000_0000 ^ (index as u64);
    for y in 0..TEXTURE_SIZE {
        for x in 0..TEXTURE_SIZE {
            let fx = x as f32 / TEXTURE_SIZE as f32;
            let fy = y as f32 / TEXTURE_SIZE as f32;
            let (r, g, b) = match index {
                // 0: wet asphalt — dark blue-gray base, low-freq
                // puddle variation, sparse bright speckle.
                0 => {
                    let base = 0.05 + 0.05 * value_noise(seed, fx, fy, 4);
                    let grain = value_noise(seed ^ 1, fx, fy, 64);
                    let speckle = if grain > 0.985 { 0.25 } else { 0.0 };
                    let v = base + speckle;
                    (v * 0.9, v * 0.95, v)
                }
                // 1: concrete facade — mid gray, block course lines
                // every 32 px, coarse noise.
                1 => {
                    let noise = value_noise(seed, fx, fy, 16);
                    let course = if (y % 32) < 2 { 0.85 } else { 1.0 };
                    let v = (0.42 + 0.14 * noise) * course;
                    (v, v, v * 1.02)
                }
                // 2: dark stone — cool, high-frequency grain.
                _ => {
                    let grain = value_noise(seed, fx, fy, 48);
                    let v = 0.18 + 0.10 * grain;
                    (v * 0.95, v, v * 1.1)
                }
            };
            data.extend_from_slice(&[
                (r.clamp(0.0, 1.0) * 255.0) as u8,
                (g.clamp(0.0, 1.0) * 255.0) as u8,
                (b.clamp(0.0, 1.0) * 255.0) as u8,
                255,
            ]);
        }
    }
    data
}

/// A scene's albedo set (P4 S2 — the attach-time description's
/// texture half): the RGBA8 images a
/// [`crate::scene_description::SceneDescription`] attaches, in
/// TEXTURE-INDEX order (a [`crate::lit_scene::Material`]'s
/// `texture` field selects from this list). Each image is
/// row-major [`TEXTURE_SIZE`]² pixels — the generator's own byte
/// contract, enforced at attach
/// ([`SceneDescription::validate`](crate::scene_description::SceneDescription::validate)):
/// a malformed length is a typed refusal, never a runtime shape
/// guess at upload.
///
/// Built by ANY consumer from [`procedural_texture`] (the corner's
/// set is indices `0..[`TEXTURE_COUNT`]` — see the migration shim,
/// `corner_content.rs`); a future scene may attach any subset or
/// reorder, as long as every material's index selects a supplied
/// texture.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProcTextureSet {
    /// One RGBA8 image per texture, in texture-index order.
    pub textures: Vec<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Determinism: the same index yields byte-identical textures.
    #[test]
    fn procedural_textures_are_deterministic() {
        for index in 0..TEXTURE_COUNT {
            let a = procedural_texture(index);
            let b = procedural_texture(index);
            assert_eq!(a, b, "texture {index} byte-identical across calls");
            assert_eq!(
                a.len(),
                (TEXTURE_SIZE * TEXTURE_SIZE * 4) as usize,
                "texture {index} size"
            );
        }
    }

    /// Distinctness: different indices yield different bytes.
    #[test]
    fn procedural_textures_are_distinct() {
        let a = procedural_texture(0);
        let b = procedural_texture(1);
        let c = procedural_texture(2);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    /// Sanity: fully opaque (alpha 255 everywhere), and each texture
    /// has non-trivial variation (not a flat color).
    #[test]
    fn procedural_textures_are_opaque_and_varied() {
        for index in 0..TEXTURE_COUNT {
            let data = procedural_texture(index);
            for px in data.as_chunks::<4>().0 {
                assert_eq!(px[3], 255, "opaque");
            }
            let lum: Vec<u8> = data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|px| ((px[0] as u32 + px[1] as u32 + px[2] as u32) / 3) as u8)
                .collect();
            let min = *lum.iter().min().unwrap() as u32;
            let max = *lum.iter().max().unwrap() as u32;
            assert!(max - min >= 8, "texture {index} varies: [{min},{max}]");
        }
    }
}
