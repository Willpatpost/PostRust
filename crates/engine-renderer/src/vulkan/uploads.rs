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
        // pointer arithmetic and F-03 decision used by both vertex buffers.
        let mut storage = [0u32; 12];
        for (first, second, expected, first_count, omitted) in [
            (&[10, 11][..], &[20, 21, 22][..], [10, 11, 20, 21], 2, 1),
            (&[1, 2, 3, 4, 5], &[8], [1, 2, 3, 4], 4, 2),
            (&[], &[3, 4], [3, 4, 0, 0], 0, 0),
        ] {
            storage.fill(0);
            let (decision, split) = unsafe {
                super::upload_layers(
                    storage.as_mut_ptr().cast(),
                    1,
                    4,
                    first,
                    second,
                    LayerPriority::First,
                )
            };
            assert_eq!(&storage[4..8], &expected);
            assert_eq!(&storage[..4], &[0; 4]);
            assert_eq!(&storage[8..], &[0; 4]);
            assert_eq!(split, first_count);
            assert_eq!(decision.truncated, omitted);
            assert_eq!(decision.supplied, decision.drawn + omitted);
        }
        for (foreground, expected, world_count) in [
            (&[7, 8, 9][..], [1, 7, 8, 9], 1),
            (&[7, 8, 9, 10, 11][..], [7, 8, 9, 10], 0),
            (&[][..], [1, 2, 3, 4], 4),
        ] {
            let world = [1u32, 2, 3, 4, 5];
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
            assert_eq!(&storage[8..], &[0; 4]);
            assert_eq!(split, world_count);
            assert_eq!(
                decision.truncated as usize,
                world.len() + foreground.len() - 4
            );
        }
    }
}
