//! Uploads (P4 S4 — design §5.1's uploads row): the mapped-slot vertex
//! upload discipline — `upload_layers`, where both copy lengths and
//! draws consume the same F-03 clamp decision, and the layer priority
//! that orders world/foreground versus hud/debug-text batches. The
//! per-family mapped writes (the proxy instance bytes, the pose bytes,
//! the indirect command blocks, the overlay slot reserve) stay in their
//! owning modules and the frame flow — extracting those inline sites
//! from `render_frame`'s body is a structural change beyond a pure
//! move, so it is out of S4's scope (noted in the lane handoff).
//! Pure move from vulkan.rs (stage S4); no logic changed — only
//! location and visibility.

use crate::config::{DynamicClamp, dynamic_geometry_clamp};

#[derive(Clone, Copy)]
pub(super) enum LayerPriority {
    First,
    Second,
}

/// Copies consecutive triangle batches into one fenced, persistently mapped
/// slot. Both copy lengths and draws consume the same F-03 clamp decision.
/// Returns the number of copied vertices from the first batch.
///
/// # Safety
/// `mapped` must be the live host-coherent mapping for the buffer of `T`
/// vertices, with at least `slot + 1` slots each holding `capacity` vertices.
/// The caller must own this slot (its preceding GPU submission has completed).
pub(super) unsafe fn upload_layers<T: bytemuck::Pod>(
    mapped: *mut core::ffi::c_void,
    slot: usize,
    capacity: u32,
    first: &[T],
    second: &[T],
    priority: LayerPriority,
) -> (DynamicClamp, u32) {
    let clamp = dynamic_geometry_clamp(first.len().saturating_add(second.len()), capacity);
    let first_len = match priority {
        LayerPriority::First => first.len().min(clamp.drawn as usize),
        LayerPriority::Second => (clamp.drawn as usize).saturating_sub(second.len()),
    };
    let second_len = clamp.drawn as usize - first_len;
    unsafe {
        let dest = mapped.cast::<T>().add(slot * capacity as usize);
        std::ptr::copy_nonoverlapping(first.as_ptr(), dest, first_len);
        std::ptr::copy_nonoverlapping(second.as_ptr(), dest.add(first_len), second_len);
    }
    (clamp, first_len as u32)
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn layered_upload_preserves_order_truncation_and_neighboring_slots() {
        // Use canaries in the surrounding slots; this exercises the exact
        // pointer arithmetic and admission decision used by both vertex
        // buffers. T04 (D-EE-10 §1E): every admitted count is a whole
        // number of triangles — the capacity floor is `capacity −
        // capacity mod 3`, each layer's incomplete tail is refused, and
        // the priority layer (first under `First`, second under
        // `Second`) is admitted before the residual layer, so the
        // residual's admission boundary lands BETWEEN triangles.
        let mut storage = [0u32; 24];
        // `expected` is padded to one fixed width (7) so the row tuple
        // has a single array type; the comparison slices it to THIS
        // row's capacity.
        for (first, second, capacity, expected, first_count, drawn, omitted) in [
            // The exact fast path: both layers well-formed and inside
            // an aligned capacity — byte-identical to the pre-contract
            // copy.
            (
                &[10, 11, 12][..],
                &[20, 21, 22][..],
                6,
                [10, 11, 12, 20, 21, 22, 0],
                3,
                6,
                0,
            ),
            // A NON-ALIGNED capacity (7 → floor 6): the pre-T04 clamp
            // copied 7 priority vertices — the seventh sat one vertex
            // into the third triangle. The contract admits 6.
            (
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12][..],
                &[],
                7,
                [1, 2, 3, 4, 5, 6, 0],
                6,
                6,
                6,
            ),
            // A malformed PRIORITY layer (2 vertices — no complete
            // triangle): refused whole, the well-formed second layer
            // takes the slot. The pre-T04 clamp drew the 2-vertex
            // prefix plus a partial second triangle (5 vertices).
            (
                &[10, 11][..],
                &[20, 21, 22][..],
                6,
                [20, 21, 22, 0, 0, 0, 0],
                0,
                3,
                2,
            ),
            // A malformed RESIDUAL layer: the layer admits whole, the
            // 2-vertex tail is refused (the pre-T04 clamp drew all 5).
            (
                &[10, 11, 12][..],
                &[20, 21][..],
                6,
                [10, 11, 12, 0, 0, 0, 0],
                3,
                3,
                2,
            ),
        ] {
            storage.fill(0);
            let (decision, split) = unsafe {
                super::upload_layers(
                    storage.as_mut_ptr().cast(),
                    1,
                    capacity,
                    first,
                    second,
                    LayerPriority::First,
                )
            };
            // The slot window is computed from THIS row's capacity
            // (slot 1 spans [capacity, 2·capacity)); the canaries on
            // both sides must stay untouched.
            let start = capacity as usize;
            assert_eq!(
                &storage[start..start + capacity as usize],
                &expected[..capacity as usize]
            );
            assert!(
                storage[..start].iter().all(|&value| value == 0),
                "the preceding slot is untouched"
            );
            assert!(
                storage[start + capacity as usize..]
                    .iter()
                    .all(|&value| value == 0),
                "the following slot is untouched"
            );
            assert_eq!(split, first_count);
            assert_eq!(decision.drawn, drawn);
            assert_eq!(decision.drawn % 3, 0, "never a partial primitive");
            assert_eq!(decision.truncated, omitted);
            assert_eq!(decision.supplied, decision.drawn + omitted);
        }
        // The region shape (`Second`: world first, foreground
        // second, FOREGROUND admitted first). Capacity 4 → floor 3.
        for (foreground, world_count, expected, drawn, omitted) in [
            // Foreground 1 triangle: admitted whole; the world's
            // 5-vertex supply (1 triangle + a 2-vertex tail) admits
            // nothing — floor 3 minus foreground 3 leaves no seat.
            // The pre-T04 clamp drew world 1, foreground 3 (the
            // one-vertex world residual WAS the mid-primitive cut).
            (&[7, 8, 9][..], 0, [7, 8, 9, 0], 3, 5),
            // A malformed foreground (5 = 1 triangle + 2): the tail is
            // refused; the world admits nothing (the pre-T04 clamp
            // drew foreground 4 — one vertex into the second
            // triangle).
            (&[7, 8, 9, 10, 11][..], 0, [7, 8, 9, 0], 3, 7),
            // No foreground: the world's 5-vertex supply admits its
            // one complete triangle, the 2-vertex tail is refused
            // (the pre-T04 clamp drew 4 of 5 — mid-primitive).
            (&[][..], 3, [1, 2, 3, 0], 3, 2),
        ] {
            let world = [1u32, 2, 3, 4, 5];
            storage.fill(0);
            let (decision, split) = unsafe {
                super::upload_layers(
                    storage.as_mut_ptr().cast(),
                    1,
                    4,
                    &world,
                    foreground,
                    LayerPriority::Second,
                )
            };
            assert_eq!(&storage[4..8], &expected);
            assert_eq!(&storage[..4], &[0; 4]);
            assert_eq!(&storage[8..], &[0; 16]);
            assert_eq!(split, world_count);
            assert_eq!(decision.drawn, drawn);
            assert_eq!(decision.drawn % 3, 0, "never a partial primitive");
            assert_eq!(decision.truncated, omitted);
            assert_eq!(decision.supplied as usize, world.len() + foreground.len());
        }
    }

    #[test]
    fn foreground_priority_leaves_a_triangle_aligned_world_residual() {
        // T04's headline shape, at scale: a capacity that splits a
        // triangle (8 → floor 6) with foreground priority — the
        // residual WORLD admission must land between triangles. The
        // pre-T04 clamp drew 8 (foreground 3 + world 5 — the world
        // residual one vertex into its second triangle); the contract
        // draws 6 (foreground 3 + world 3), omitting and reporting the
        // other 12 supplied vertices.
        let mut storage = [0u32; 24];
        let world: Vec<u32> = (1..=15).collect(); // 5 triangles
        let foreground: Vec<u32> = (100..=102).collect(); // 1 triangle
        let (decision, world_drawn) = unsafe {
            super::upload_layers(
                storage.as_mut_ptr().cast(),
                1,
                8,
                &world,
                &foreground,
                LayerPriority::Second,
            )
        };
        assert_eq!(&storage[8..16], &[1, 2, 3, 100, 101, 102, 0, 0]);
        assert_eq!(&storage[..8], &[0; 8]);
        assert_eq!(&storage[16..], &[0; 8]);
        assert_eq!(world_drawn, 3, "the world residual is triangle-aligned");
        assert_eq!(decision.drawn, 6);
        assert_eq!(decision.drawn % 3, 0);
        assert_eq!(decision.truncated, 12);
        assert_eq!(decision.supplied, 18);
    }
}
