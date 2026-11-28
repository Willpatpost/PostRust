//! T05 (strict audit IV) — the population-identity collision witnesses:
//! the RED-side demonstration of the raw-generation gate's hazard, at
//! the PUBLIC core (`repack_populations_generation`), plus the
//! same-source stability pin (the R2 reuse the identity layer must
//! preserve). The adapter-level defenses (the identity records beside
//! the generation, the stamp invalidation, the indirect-count
//! identity) are proven in-crate — `submission.rs` and
//! `vulkan/frame.rs`'s test modules — because `adapt_frame` and
//! `PopulationStamps` are `pub(crate)`.
//!
//! THE DEMONSTRATED MECHANISM (why the audit is right): the core
//! gate's reuse condition is `generation != 0 && buffer_generation ==
//! generation && buffer.len() == total * INSTANCE_STRIDE` — a RAW
//! caller u64 plus a byte length. Two independently valid population
//! sources, each with its own generation space starting at 1, can
//! present the SAME generation at the SAME total count with
//! DIFFERENT content; the gate cannot tell them apart, returns
//! `Reused`, and the retained buffer keeps the FIRST source's bytes.
//! In debug/test builds the adapter's missed-bump verify trips on
//! exactly this (fail-visible); in a release production frame
//! nothing re-packs and nothing compares — the stale bytes render.
//! The first test below reproduces the skip through the public API
//! with NO verify in the path (the release shape); it is a keeper —
//! it pins the CORE contract (the generation space belongs to the
//! population source; a collision there is a caller defect the
//! RENDERER-side identity layer must defend against, which is what
//! T05 landed; see `submission.rs`'s identity records).

use engine_renderer::proxy_instances::{
    GenerationDecision, INSTANCE_STRIDE, InstanceTransform, MAX_INSTANCES, ProxyInstance,
    repack_populations, repack_populations_generation,
};

/// Two fixture populations from two DIFFERENT sources (their own
/// storage — the way two independently owned crowds exist), with
/// COLLIDING generation spaces: same per-kind counts (2 npc, 1
/// vehicle, no rain — so the same packed byte length), different
/// content, both declaring generation 5.
fn source_a() -> Vec<ProxyInstance> {
    let instance = |x: f32, turn: u16, pose_index: u32| ProxyInstance {
        transform: InstanceTransform {
            pos: [x, 0.02, -2.0],
            turn,
            scale: 1.0,
        },
        tint: [0.8, 0.7, 0.6],
        alpha: 1.0,
        pose_index,
    };
    vec![
        instance(1.0, 1000, 0),
        instance(-3.0, 40000, 1),
        instance(7.0, 8192, 0),
    ]
}

fn source_b() -> Vec<ProxyInstance> {
    let instance = |x: f32, turn: u16, pose_index: u32| ProxyInstance {
        transform: InstanceTransform {
            pos: [x, 0.5, 4.0],
            turn,
            scale: 2.0,
        },
        tint: [0.1, 0.9, 0.3],
        alpha: 1.0,
        pose_index,
    };
    vec![
        instance(12.0, 2000, 0),
        instance(-8.0, 30000, 1),
        instance(9.0, 4096, 0),
    ]
}

/// THE COLLISION, DEMONSTRATED (the audit's hypothesis, release
/// shape): source A is admitted at generation 5; source B — a
/// different population at the same counts from its own storage,
/// its own generation space also at 5 — submits, and the core gate
/// RETURNS REUSED while the buffer still holds A's bytes. No verify
/// runs on this path (the verify is the adapter's debug/test
/// dispatch, absent here exactly as in a release production frame):
/// the skip is silent, and the renderer would draw A's crowd. This
/// is the hazard the renderer-side identity records (T05,
/// `submission.rs`) exist to make unrepresentable for two LIVE
/// sources; the core function's own contract deliberately stays the
/// R2 shape (caller-owned generation space, `census/repack.rs`'s
/// single-spec route depends on it) — the defense lives one level
/// up, where the renderer can see the submission's provenance.
#[test]
fn core_gate_reuses_a_colliding_generation_from_a_second_source() {
    let a = source_a();
    let b = source_b();
    let mut buffer = Vec::new();
    let mut buffer_generation = 0_u64;
    // Source A admitted: repacked, generation recorded.
    assert_eq!(
        repack_populations_generation(
            &mut buffer,
            &a[..2],
            &a[2..],
            &[],
            MAX_INSTANCES,
            5,
            &mut buffer_generation
        )
        .unwrap(),
        GenerationDecision::Repacked
    );
    let a_bytes = buffer.clone();
    assert_eq!(buffer_generation, 5);
    // Source B at its own generation 5, same counts: the gate hits.
    let decision = repack_populations_generation(
        &mut buffer,
        &b[..2],
        &b[2..],
        &[],
        MAX_INSTANCES,
        5,
        &mut buffer_generation,
    )
    .unwrap();
    // The demonstrated skip: REUSED against the colliding space...
    assert_eq!(
        decision,
        GenerationDecision::Reused,
        "the raw-u64 gate cannot distinguish the two sources — the \
         demonstrated T05 hazard (the defense is renderer-side)"
    );
    // ...and the retained bytes are still source A's — the renderer
    // would draw the WRONG crowd (in the adapter this exact state is
    // what the debug missed-bump verify panics on; release renders
    // it, which is why the identity layer exists).
    assert_eq!(buffer, a_bytes);
    assert_eq!(buffer.len(), 3 * INSTANCE_STRIDE);
    // The oracle both sources individually pack to (what a correct
    // renderer must end up holding after B's submission):
    let mut b_expected = Vec::new();
    repack_populations(&mut b_expected, &b[..2], &b[2..], &[]).unwrap();
    assert_ne!(
        buffer, b_expected,
        "the buffer must NOT hold B's bytes here — \
                                    if it did, the demonstration would be vacuous"
    );
}

/// The same-generation partition collision (the byte-only oracle's
/// blind spot, demonstrated at the core gate): source A splits its
/// three instances 2 npc + 1 vehicle; source C submits the SAME
/// three instances in the SAME buffer order split 1 npc + 2 vehicle
/// at the same generation — the concatenated instance bytes are
/// IDENTICAL (the payload gate reuses, and even the adapter's debug
/// byte-verify passes), but the per-kind counts differ, so the
/// INDIRECT command block (a pure function of the per-kind counts)
/// and the per-kind draw splits differ. A byte-length oracle cannot
/// see this shape change; the indirect-count identity (T05,
/// `vulkan/frame.rs`'s per-kind count records) is what catches it.
#[test]
fn core_gate_reuses_an_identical_byte_payload_across_a_different_partition() {
    let a = source_a();
    let mut buffer = Vec::new();
    let mut buffer_generation = 0_u64;
    repack_populations_generation(
        &mut buffer,
        &a[..2],
        &a[2..],
        &[],
        MAX_INSTANCES,
        5,
        &mut buffer_generation,
    )
    .unwrap();
    let a_bytes = buffer.clone();
    // The same instances, partitioned 1 npc + 2 vehicle (the second
    // npc becomes a vehicle — its pose slot is unread by the vehicle
    // pipeline, and its pose index stays in range for the ONE npc).
    let decision = repack_populations_generation(
        &mut buffer,
        &a[..1],
        &a[1..],
        &[],
        MAX_INSTANCES,
        5,
        &mut buffer_generation,
    )
    .unwrap();
    assert_eq!(decision, GenerationDecision::Reused);
    // Byte-identical payload: the reuse is CORRECT for the instance
    // buffer itself (nothing stale in those bytes) — the hazard is
    // that the per-kind counts moved from (2, 1, 0) to (1, 2, 0)
    // while every byte-length oracle stays green. The indirect
    // block and the per-kind draws must follow the NEW counts; the
    // stamp-level defense is proven in `vulkan/frame.rs`'s tests.
    assert_eq!(buffer, a_bytes);
}
