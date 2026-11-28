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

use crate::config::{AdmissionReason, DynamicClamp, dynamic_geometry_clamp};

#[derive(Clone, Copy)]
pub(super) enum LayerPriority {
    First,
    Second,
}

/// One layer's share of the two-layer admission plan (T04): the
/// vertices it requested, the whole triangles admitted, the vertices
/// omitted (the capacity cut plus its rejected malformed tail), and
/// that tail itself. Every count is reported — nothing is silently
/// dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct LayerShare {
    /// Vertices the layer supplied (requested).
    pub requested: u32,
    /// Vertices admitted — always a whole number of triangles.
    pub admitted: u32,
    /// Vertices omitted from this layer (`requested − admitted`).
    pub omitted: u32,
    /// The layer's incomplete-triangle tail, rejected whole.
    pub malformed_tail: u32,
}

/// The ONE admission plan for one dynamic vertex buffer's two layers,
/// computed pure BEFORE any upload byte moves (T04/D-EE-10 §1E): the
/// capacity's triangle floor, the foreground-then-world (or
/// HUD-then-debug) deterministic partition by complete units, and
/// the combined [`DynamicClamp`] record the outcome and stats
/// consume. The priority layer (first under [`LayerPriority::First`]
/// — the overlay's HUD; second under [`LayerPriority::Second`] — the
/// scene's foreground) is admitted first as whole triangles; the
/// residual layer takes what remains of the floor, so its admission
/// boundary lands between triangles — the pre-T04 split could leave
/// a non-triangle-aligned world residual behind a prioritized
/// foreground.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LayerAdmission {
    /// The combined record (both layers): requested/admitted/omitted
    /// vertices AND units, capacity + floor, malformed tail, reason.
    pub record: DynamicClamp,
    /// The first-supplied layer's share (world / HUD).
    pub first: LayerShare,
    /// The second-supplied layer's share (foreground / debug).
    pub second: LayerShare,
}

/// Computes the plan. Pure and unit-testable without a device; the
/// exact fast path (both layers well-formed and inside the floor)
/// reproduces the pre-T04 copy counts exactly.
pub(super) fn admit_layers(
    first_len: usize,
    second_len: usize,
    capacity: u32,
    priority: LayerPriority,
) -> LayerAdmission {
    // The per-layer triangle split comes from the single-batch
    // contract (one source of the floor/tail arithmetic).
    let first_clamp = dynamic_geometry_clamp(first_len, capacity);
    let second_clamp = dynamic_geometry_clamp(second_len, capacity);
    let floor = first_clamp.capacity_floor;
    let complete = |clamp: DynamicClamp| clamp.requested_units * 3;
    let first_complete = complete(first_clamp);
    let second_complete = complete(second_clamp);
    // The deterministic partition: the priority layer's complete
    // triangles first, the residual layer's against the remainder of
    // the floor — both admissions are multiples of 3 by construction.
    let (priority_complete, residual_complete) = match priority {
        LayerPriority::First => (first_complete, second_complete),
        LayerPriority::Second => (second_complete, first_complete),
    };
    let priority_admitted = priority_complete.min(floor);
    let residual_admitted = residual_complete.min(floor - priority_admitted);
    let (first_admitted, second_admitted) = match priority {
        LayerPriority::First => (priority_admitted, residual_admitted),
        LayerPriority::Second => (residual_admitted, priority_admitted),
    };
    let share = |requested_len: u32, admitted: u32, clamp: DynamicClamp| LayerShare {
        requested: requested_len,
        admitted,
        omitted: requested_len - admitted,
        malformed_tail: clamp.malformed_tail,
    };
    let supplied = first_len.saturating_add(second_len).min(u32::MAX as usize) as u32;
    let drawn = first_admitted + second_admitted;
    let requested_units = first_clamp.requested_units + second_clamp.requested_units;
    let malformed_tail = first_clamp.malformed_tail + second_clamp.malformed_tail;
    LayerAdmission {
        record: DynamicClamp {
            supplied,
            capacity,
            drawn,
            truncated: supplied - drawn,
            requested_units,
            admitted_units: drawn / 3,
            omitted_units: requested_units - drawn / 3,
            malformed_tail,
            capacity_floor: floor,
            reason: match (first_complete + second_complete > drawn, malformed_tail > 0) {
                (true, true) => AdmissionReason::CapacityAndMalformedTail,
                (true, false) => AdmissionReason::Capacity,
                (false, true) => AdmissionReason::MalformedTail,
                (false, false) => AdmissionReason::Fits,
            },
        },
        first: share(first_clamp.supplied, first_admitted, first_clamp),
        second: share(second_clamp.supplied, second_admitted, second_clamp),
    }
}

/// Copies consecutive triangle batches into one fenced, persistently
/// mapped slot. Both copy lengths and draws consume the same
/// admission plan ([`admit_layers`] — the F-03 single-decision
/// discipline under T04's complete-primitive contract: only whole
/// triangles are ever copied or drawn). Returns the combined record
/// and the number of copied vertices from the first batch.
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
    let plan = admit_layers(first.len(), second.len(), capacity, priority);
    unsafe {
        let dest = mapped.cast::<T>().add(slot * capacity as usize);
        std::ptr::copy_nonoverlapping(first.as_ptr(), dest, plan.first.admitted as usize);
        std::ptr::copy_nonoverlapping(
            second.as_ptr(),
            dest.add(plan.first.admitted as usize),
            plan.second.admitted as usize,
        );
    }
    (plan.record, plan.first.admitted)
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

    #[test]
    fn admission_plan_reports_per_layer_shares_and_unit_accounting() {
        // The plan's reported-metrics shape (T04): each layer's
        // requested/admitted/omitted vertices with its malformed
        // tail, the deterministic foreground-first partition, and the
        // combined record's unit accounting — every omission typed
        // and countable at the layer it happened.
        use super::LayerShare;
        // The region shape at the audit's split capacity: world 5
        // triangles, foreground 1, capacity 8 (floor 6).
        let plan = super::admit_layers(15, 3, 8, LayerPriority::Second);
        assert_eq!(
            plan.first,
            LayerShare {
                requested: 15,
                admitted: 3,
                omitted: 12,
                malformed_tail: 0,
            },
            "the world layer's share"
        );
        assert_eq!(
            plan.second,
            LayerShare {
                requested: 3,
                admitted: 3,
                omitted: 0,
                malformed_tail: 0,
            },
            "the foreground layer is admitted whole first"
        );
        assert_eq!(
            (
                plan.record.supplied,
                plan.record.drawn,
                plan.record.truncated
            ),
            (18, 6, 12)
        );
        assert_eq!(
            (
                plan.record.requested_units,
                plan.record.admitted_units,
                plan.record.omitted_units
            ),
            (6, 2, 4)
        );
        assert_eq!(plan.record.capacity_floor, 6);
        assert_eq!(plan.record.reason, super::AdmissionReason::Capacity);
        // A malformed WORLD tail with room to spare: the tail is
        // rejected and reported at its layer, the complete prefix
        // admits.
        let plan = super::admit_layers(5, 0, 4, LayerPriority::Second);
        assert_eq!(
            plan.first,
            LayerShare {
                requested: 5,
                admitted: 3,
                omitted: 2,
                malformed_tail: 2,
            }
        );
        assert_eq!(plan.record.malformed_tail, 2);
        assert_eq!(plan.record.reason, super::AdmissionReason::MalformedTail);
        // The exact fast path: both layers whole, inside an aligned
        // capacity — reason Fits, nothing omitted anywhere.
        let plan = super::admit_layers(3, 3, 6, LayerPriority::First);
        assert_eq!(plan.first.admitted, 3);
        assert_eq!(plan.second.admitted, 3);
        assert_eq!(plan.record.truncated, 0);
        assert_eq!(plan.record.omitted_units, 0);
        assert_eq!(plan.record.reason, super::AdmissionReason::Fits);
    }
}
