//! The renderer's animation surface, post-E-S3 (DECISION-A06 §4, the
//! owner-approved A06 migration): the GPU PACKING LAYER — the module
//! that owns the renderer's pose-upload carve. The deterministic
//! pose/skeleton/sampling core lives in the `engine-animation` crate
//! (E-S1); engine-slice speaks `engine_animation::` directly (E-S2);
//! and the E-S1 compatibility shim's PUBLIC re-export block is
//! deleted HERE (E-S3) — `engine_renderer::animation::<moved
//! symbol>` is no longer a public path. Root-path consumers keep
//! their paths through the lib.rs root re-export (re-pointed at
//! `engine_animation`); the packing symbols below keep this module
//! path (they are defined here).
//!
//! What stays HERE (the GPU packing layer, per the owner's
//! "separated from GPU packing" clause): the retained-buffer repack
//! family (`repack_poses`, `repack_poses_bounded`,
//! `repack_poses_generation`, `verify_pose_generation`), the
//! capacity mirror `MAX_POSE_BLOCKS`, the typed refusal
//! `PosePackError`, and the pose source identity `PoseSourceToken`
//! — the buffer/capacity/generation machinery that owns the
//! renderer's upload carve. `skinned_humanoid_mesh` lives in
//! `populated_meshes` (it derives from that module's unskinned
//! mesh); the crate-visible alias below holds the renderer's
//! internal `crate::animation::` consumers of it.
//!
//! The crate-visible aliases below are NOT compatibility surface:
//! they are the renderer's INTERNAL wiring — the same treatment
//! `mat3_multiply` has always had. The renderer's own modules
//! (`shadows`, `renderer`, `submission`, `frame_plan`, `config`,
//! the `vulkan` tree) speak `crate::animation::<symbol>` for the
//! moved pure-core symbols; the aliases keep that path resolving
//! against the ONE engine-animation implementation — the convention
//! still exists exactly once.
//!
//! The test module at the bottom stays renderer-side ON PURPOSE:
//! those tests exercise the kept repack/generation machinery and
//! the shipped mesh derivation — the packing layer's own batteries.

use crate::proxy_instances::GenerationDecision;

pub(crate) use crate::populated_meshes::skinned_humanoid_mesh;
pub(crate) use engine_animation::{
    BOX_JOINT, HUMANOID_BOXES, HUMANOID_JOINTS, POSE_STRIDE, SkinnedVertex, pack_pose_bytes,
};

/// The pure-core symbols the renderer reaches only from its own
/// `cfg(test)` batteries — the shadow oracle's pose machinery
/// (shadows.rs), the submission/frame generation-oracle tests, and
/// this module's own tests. Crate-visible and test-gated so the
/// non-test lib build carries no unused alias.
#[cfg(test)]
pub(crate) use engine_animation::{
    JointPose, compose_joint_matrices, humanoid_skeleton, rest_pose_block, sample_pose,
    skin_matrices, skin_vertex, walk_clip,
};

/// The column-major 3x3 product the shadow oracle composes
/// same-convention rotations through — ONE implementation, now
/// `engine_animation::mat3_multiply` (public there at E-S1);
/// re-exported crate-visibly here, the pre-extraction surface
/// exactly (`shadows.rs` imports it from this module).
pub(crate) use engine_animation::mat3_multiply;

/// The typed repack refusal (the D2 no-partial-overflow discipline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosePackError {
    /// More pose blocks than the capacity allows. No bytes were produced.
    Capacity { capacity: usize, requested: usize },
}

/// T05 (strict audit IV) — the POSE population's source identity:
/// [`crate::proxy_instances::InstanceSourceToken`]'s mirror for the
/// pose-block palette (the evaluated-pose list a distinct source
/// owns). The token is the blocks slice's storage address plus its
/// length; the renderer derives and records it beside the pose
/// generation, so two LIVE pose sources with colliding generation
/// spaces cannot wrongly reuse each other's blocks, while a source
/// that re-evaluates into stable storage (the demos' retained
/// `poses` Vec, cleared and refilled in place) keeps its between-
/// tick reuse. Same residual hazard and same future hardening as
/// the instance token's docs — see there (the contract exists
/// exactly once, referenced from the pose side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PoseSourceToken {
    blocks: usize,
    blocks_count: usize,
}

impl PoseSourceToken {
    /// Derives the token from a submission's borrowed pose blocks —
    /// the one construction path (the renderer's adapter is its only
    /// caller; the fields stay private).
    pub fn of(blocks: &[[[[f32; 4]; 4]; HUMANOID_JOINTS]]) -> Self {
        Self {
            blocks: blocks.as_ptr() as usize,
            blocks_count: blocks.len(),
        }
    }
}

/// The maximum number of pose blocks (the same bound as instances).
pub const MAX_POSE_BLOCKS: usize = crate::proxy_instances::MAX_INSTANCES;

/// Repacks multiple pose blocks into a retained buffer (clear + reserve + append).
/// Capacity bound MAX_POSE_BLOCKS — typed refusal above, never truncation.
/// Takes the blocks BY VALUE slice (no caller-side reference juggling —
/// the demo's per-frame retained Vec of pose blocks feeds this directly).
pub fn repack_poses(
    buffer: &mut Vec<u8>,
    blocks: &[[[[f32; 4]; 4]; HUMANOID_JOINTS]],
) -> Result<(), PosePackError> {
    repack_poses_bounded(buffer, blocks, MAX_POSE_BLOCKS)
}

/// The bounded form (P5 §1.3, A11/R1): the caller-supplied cap is
/// the CONFIGURED budget ([`crate::config::PoseCapacity`] — default
/// [`MAX_POSE_BLOCKS`]), mirroring
/// [`crate::proxy_instances::repack_populations_bounded`]: the CAP,
/// never the current carve, is the typed refusal's boundary, and the
/// GPU pose carve grows toward this cap on demand.
pub fn repack_poses_bounded(
    buffer: &mut Vec<u8>,
    blocks: &[[[[f32; 4]; 4]; HUMANOID_JOINTS]],
    capacity: usize,
) -> Result<(), PosePackError> {
    if blocks.len() > capacity {
        return Err(PosePackError::Capacity {
            capacity,
            requested: blocks.len(),
        });
    }

    buffer.clear();
    buffer.reserve(blocks.len() * POSE_STRIDE);

    for block in blocks.iter() {
        let bytes = pack_pose_bytes(block);
        buffer.extend_from_slice(&bytes);
    }

    Ok(())
}

/// The dirty-generation protocol's pose half (P5 §3.3, A18/R2 —
/// [`crate::proxy_instances::repack_populations_generation`]'s
/// mirror): the capacity bound ALWAYS runs (the typed refusal stays
/// per-frame), and the byte regeneration is gated on the caller's
/// declared `generation` — a pose advance bumps, a static pose set
/// reuses (the garage's instance half and any between-tick frame's
/// pose half). `buffer_generation` records which generation's bytes
/// the retained `buffer` holds (0 = unknown; every generation-0
/// submission resets it). The count guard
/// (`buffer.len() == blocks.len() * POSE_STRIDE`) is the packed
/// footprint. The full-repack fallback is [`repack_poses_bounded`],
/// unchanged.
pub fn repack_poses_generation(
    buffer: &mut Vec<u8>,
    blocks: &[[[[f32; 4]; 4]; HUMANOID_JOINTS]],
    capacity: usize,
    generation: u64,
    buffer_generation: &mut u64,
) -> Result<GenerationDecision, PosePackError> {
    if blocks.len() > capacity {
        return Err(PosePackError::Capacity {
            capacity,
            requested: blocks.len(),
        });
    }
    if generation != 0
        && *buffer_generation == generation
        && buffer.len() == blocks.len() * POSE_STRIDE
    {
        return Ok(GenerationDecision::Reused);
    }
    buffer.clear();
    buffer.reserve(blocks.len() * POSE_STRIDE);
    for block in blocks.iter() {
        let bytes = pack_pose_bytes(block);
        buffer.extend_from_slice(&bytes);
    }
    *buffer_generation = generation;
    Ok(GenerationDecision::Repacked)
}

/// THE MISSED-BUMP DEBUG VERIFY's pose half (P5 §3.3 — the
/// independent reference path): re-packs `blocks` through the
/// full-repack fallback and byte-compares against `buffer`,
/// panicking loudly on divergence. The submission adapter invokes
/// this on every pose-side reuse under `cfg(any(debug_assertions,
/// test))`; always compiled so the red-then-green witness runs in
/// every test profile.
pub fn verify_pose_generation(
    buffer: &[u8],
    blocks: &[[[[f32; 4]; 4]; HUMANOID_JOINTS]],
    capacity: usize,
) -> Result<(), PosePackError> {
    let mut reference = Vec::with_capacity(buffer.len());
    repack_poses_bounded(&mut reference, blocks, capacity)?;
    assert_eq!(
        reference, buffer,
        "MISSED BUMP (P5 §3.3): a pose set declared unchanged \
         (generation reuse) repacked to different bytes — bump the \
         pose generation when the evaluated blocks change"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The zero-pose fixture (rest) — used by many tests.
    fn rest_pose() -> [JointPose; HUMANOID_JOINTS] {
        [JointPose {
            delta_pos: [0.0; 3],
            delta_turns: [0; 3],
        }; HUMANOID_JOINTS]
    }

    /// skin_vertex: a rigid vertex follows its joint's transform
    /// EXACTLY (the hand-rotated known answer), and the shipped
    /// mesh's weights sum to one. (E-S1: stayed renderer-side — it
    /// consumes the shipped `skinned_humanoid_mesh`, a renderer
    /// asset — and now reaches `skin_vertex`/`SkinnedVertex` THROUGH
    /// this shim's re-exports, the path-stability proof.)
    #[test]
    fn skin_vertex_rigid_follows_its_joint() {
        let skeleton = humanoid_skeleton();
        let mut pose = rest_pose();
        pose[7].delta_turns = [16384, 0, 0]; // quarter pitch, left hip.
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let skin_mats = skin_matrices(&joint_worlds, &skeleton);

        // A mesh vertex of the left upper-leg box (emission box 7).
        let mesh = skinned_humanoid_mesh();
        let v = &mesh[7 * 36];
        assert_eq!(v.joints[0], 7, "the vertex is rigid to joint 7");
        let (pos, _) = skin_vertex(v, &skin_mats);
        // The independent hand answer: M_7 · v = pivot + R_quarter ·
        // (v − pivot), and quarter pitch maps (x, y, z) → (x, −z, y).
        let pivot = skeleton[7].pivot;
        let d = [
            v.pos[0] - pivot[0],
            v.pos[1] - pivot[1],
            v.pos[2] - pivot[2],
        ];
        let expected = [pivot[0] + d[0], pivot[1] + (-d[2]), pivot[2] + d[1]];
        for axis in 0..3 {
            assert!(
                (pos[axis] - expected[axis]).abs() < 2e-5,
                "axis {axis}: {pos:?} vs the hand rotation {expected:?}"
            );
        }

        // At rest, the identity skinning returns the authored vertex.
        let joint_worlds = compose_joint_matrices(&skeleton, &rest_pose());
        let skin_mats = skin_matrices(&joint_worlds, &skeleton);
        for v in mesh.iter().step_by(37) {
            let (pos, normal) = skin_vertex(v, &skin_mats);
            for axis in 0..3 {
                assert!((pos[axis] - v.pos[axis]).abs() < 1e-6, "rest pos identity");
                assert!(
                    (normal[axis] - v.normal[axis]).abs() < 1e-6,
                    "rest normal identity"
                );
            }
        }
        // The weights sum to one on the whole shipped mesh.
        for (i, v) in mesh.iter().enumerate() {
            let sum: f32 = v.weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-6, "vertex {i}: weights sum to 1.0");
        }
    }

    /// pack_pose_bytes / repack_poses: determinism; the retained
    /// repack equals the concatenated per-block packs; the capacity
    /// refusal is ATOMIC (the prior frame's bytes survive a
    /// refusal); the retained buffer reuses its capacity once
    /// warmed (the F-05 discipline).
    #[test]
    fn pose_packing_discipline() {
        let skeleton = humanoid_skeleton();
        let mut pose = rest_pose();
        pose[7].delta_turns = [16384, 0, 0];
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let posed = skin_matrices(&joint_worlds, &skeleton);
        let rest = rest_pose_block();

        let bytes1 = pack_pose_bytes(&posed);
        let bytes2 = pack_pose_bytes(&posed);
        assert_eq!(bytes1, bytes2, "packing determinism");
        assert_eq!(bytes1.len(), POSE_STRIDE);

        let mut buffer = Vec::new();
        repack_poses(&mut buffer, &[rest, posed]).unwrap();
        let expected: Vec<u8> = pack_pose_bytes(&rest)
            .iter()
            .chain(pack_pose_bytes(&posed).iter())
            .copied()
            .collect();
        assert_eq!(buffer, expected, "the repack is the concatenated packs");
        assert_eq!(buffer.len(), 2 * POSE_STRIDE);

        // The refusal is atomic: over-capacity leaves the prior bytes.
        let prior = buffer.clone();
        let over: Vec<_> = vec![rest; MAX_POSE_BLOCKS + 1];
        assert_eq!(
            repack_poses(&mut buffer, &over),
            Err(PosePackError::Capacity {
                capacity: MAX_POSE_BLOCKS,
                requested: MAX_POSE_BLOCKS + 1
            })
        );
        assert_eq!(buffer, prior, "the refusal left the prior frame intact");

        // The retained buffer never grows once warmed.
        let warmed = buffer.capacity();
        for _ in 0..3 {
            repack_poses(&mut buffer, &[rest, posed]).unwrap();
            assert_eq!(buffer.capacity(), warmed, "no reallocation once warmed");
        }
        repack_poses(&mut buffer, &[rest]).unwrap();
        assert_eq!(
            buffer.capacity(),
            warmed,
            "a smaller pack keeps the capacity"
        );
        assert_eq!(buffer.len(), POSE_STRIDE);
    }

    /// P5 §1.3 (A11/R1): the CONFIGURED cap is the pose repack's
    /// typed-refusal boundary — the instance carve's mirror (a
    /// smaller budget refuses at exactly that budget; the default
    /// form is the bounded form at MAX_POSE_BLOCKS).
    #[test]
    fn pose_repack_refusal_boundary_is_the_configured_cap() {
        let rest = rest_pose_block();
        let blocks: Vec<_> = vec![rest; 24];
        let mut buffer = Vec::new();
        repack_poses_bounded(&mut buffer, &blocks, 24).unwrap();
        assert_eq!(buffer.len(), 24 * POSE_STRIDE);
        let prior = buffer.clone();
        assert_eq!(
            repack_poses_bounded(&mut buffer, &blocks, 23),
            Err(PosePackError::Capacity {
                capacity: 23,
                requested: 24
            })
        );
        assert_eq!(buffer, prior, "the refusal left the prior bytes intact");
        // The default form delegates at the constant.
        let mut bounded = Vec::new();
        repack_poses_bounded(&mut bounded, &blocks, MAX_POSE_BLOCKS).unwrap();
        let mut constant = Vec::new();
        repack_poses(&mut constant, &blocks).unwrap();
        assert_eq!(bounded, constant);
    }

    /// P5 §3.3 (A18/R2) — the pose-side generation gate: the
    /// decision sequence over a static pose set (the garage's
    /// between-advance shape and any between-tick frame's pose
    /// half). First submission repacks and records; same-generation
    /// resubmissions reuse byte-identically; a bumped generation
    /// (the pose advanced) repacks; generation 0 always repacks and
    /// resets the record; the count guard repacks on a changed block
    /// count at the same generation.
    #[test]
    fn pose_generation_gated_repack_decision_sequence() {
        let skeleton = humanoid_skeleton();
        let mut pose = rest_pose();
        pose[7].delta_turns = [16384, 0, 0];
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let posed = skin_matrices(&joint_worlds, &skeleton);
        let rest = rest_pose_block();
        let static_set = [rest, posed, rest];
        let mut buffer = Vec::new();
        let mut buffer_generation = 0_u64;
        assert_eq!(
            repack_poses_generation(
                &mut buffer,
                &static_set,
                MAX_POSE_BLOCKS,
                4,
                &mut buffer_generation
            )
            .unwrap(),
            GenerationDecision::Repacked
        );
        let first = buffer.clone();
        for _ in 0..3 {
            assert_eq!(
                repack_poses_generation(
                    &mut buffer,
                    &static_set,
                    MAX_POSE_BLOCKS,
                    4,
                    &mut buffer_generation
                )
                .unwrap(),
                GenerationDecision::Reused
            );
            assert_eq!(buffer, first);
        }
        assert_eq!(buffer_generation, 4);
        // The pose advanced: bump and repack.
        let mut pose2 = rest_pose();
        pose2[8].delta_turns = [-8192, 0, 0];
        let joint_worlds = compose_joint_matrices(&skeleton, &pose2);
        let posed2 = skin_matrices(&joint_worlds, &skeleton);
        let advanced = [rest, posed2, rest];
        assert_eq!(
            repack_poses_generation(
                &mut buffer,
                &advanced,
                MAX_POSE_BLOCKS,
                5,
                &mut buffer_generation
            )
            .unwrap(),
            GenerationDecision::Repacked
        );
        let mut expected = Vec::new();
        repack_poses(&mut expected, &advanced).unwrap();
        assert_eq!(buffer, expected, "the repacked bytes are the fallback's");
        // The count guard: same generation, different block count.
        assert_eq!(
            repack_poses_generation(
                &mut buffer,
                &[rest],
                MAX_POSE_BLOCKS,
                5,
                &mut buffer_generation
            )
            .unwrap(),
            GenerationDecision::Repacked
        );
        // Generation 0: always repack, reset the record.
        assert_eq!(
            repack_poses_generation(
                &mut buffer,
                &static_set,
                MAX_POSE_BLOCKS,
                0,
                &mut buffer_generation
            )
            .unwrap(),
            GenerationDecision::Repacked
        );
        assert_eq!(buffer_generation, 0);
        // The capacity refusal stays per-frame on the gated path and
        // is atomic against the records.
        let prior = buffer.clone();
        let over: Vec<_> = vec![rest; MAX_POSE_BLOCKS + 1];
        assert_eq!(
            repack_poses_generation(
                &mut buffer,
                &over,
                MAX_POSE_BLOCKS,
                0,
                &mut buffer_generation
            ),
            Err(PosePackError::Capacity {
                capacity: MAX_POSE_BLOCKS,
                requested: MAX_POSE_BLOCKS + 1
            })
        );
        assert_eq!(buffer, prior);
        assert_eq!(buffer_generation, 0);
    }

    /// P5 §3.3 — the pose-side missed-bump witnesses: the red half
    /// panics the verify on a changed-but-not-bumped pose set; the
    /// green half verifies an honestly-declared set clean.
    #[test]
    #[should_panic(expected = "MISSED BUMP")]
    fn pose_missed_bump_panics_the_verify() {
        let skeleton = humanoid_skeleton();
        let mut pose = rest_pose();
        pose[7].delta_turns = [16384, 0, 0];
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let posed = skin_matrices(&joint_worlds, &skeleton);
        let rest = rest_pose_block();
        let mut buffer = Vec::new();
        repack_poses(&mut buffer, &[rest, posed]).unwrap();
        // The walker advanced without a bump: the reference repack
        // diverges and the verify fails loudly.
        let mut pose2 = rest_pose();
        pose2[7].delta_turns = [8192, 0, 0];
        let joint_worlds = compose_joint_matrices(&skeleton, &pose2);
        let posed2 = skin_matrices(&joint_worlds, &skeleton);
        verify_pose_generation(&buffer, &[rest, posed2], MAX_POSE_BLOCKS).unwrap();
    }

    /// The verify's green half (see the red witness above).
    #[test]
    fn pose_verify_green() {
        let rest = rest_pose_block();
        let mut buffer = Vec::new();
        repack_poses(&mut buffer, &[rest, rest]).unwrap();
        verify_pose_generation(&buffer, &[rest, rest], MAX_POSE_BLOCKS)
            .expect("an honestly-declared pose set verifies clean");
    }
}
