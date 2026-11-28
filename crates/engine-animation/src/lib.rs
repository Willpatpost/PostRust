//! The deterministic animation core (Stage 1, Era-2 package — the
//! living world's skeletal foundation): the pure-data clip model, the
//! libm-free rotation machinery ([`sin_cos_turn`]), the joint
//! hierarchy's matrix composition, and the skinning equation — ALL
//! pure, fully tested, NO Vulkan.
//!
//! EXTRACTED from engine-renderer at E-S1 (DECISION-A06 §4,
//! owner-approved per D-EE-10 §2): this crate is the canonical home
//! of the pose/skeleton/sampling machinery and the single-block pose
//! byte serialization ([`pack_pose_bytes`]); engine-renderer's
//! `animation` module is the compatibility shim re-exporting this
//! surface beside the GPU packing layer (the retained repack, the
//! capacity refusal, the generation gate) that stays renderer-side.
//! [`sin_cos_turn`] moved here from engine-renderer's
//! proxy_instances (which re-exports it for its existing consumers),
//! and bytemuck is this crate's one dependency, carried by
//! [`SkinnedVertex`]'s Pod derives — the byte-boundary type whose
//! pure CPU skinning reference [`skin_vertex`] evaluates.
//!
//! # DETERMINISM CONTRACT
//! Every byte produced by this module is byte-identical on every
//! platform. This is why rotations use [`sin_cos_turn`]
//! turn fractions (1/65536 of a full rotation) through fixed-order
//! Taylor series — pure IEEE f32 arithmetic, no libm, no platform
//! variation. All packing uses `f32::to_bits` — the exact IEEE bit
//! patterns, little-endian, in a FIXED field order.
//!
//! # THE ROTATION CONVENTION (the pinned reference)
//! A joint's local rotation is THREE u16 turn fractions (pitch, yaw,
//! roll) composed in the FIXED order: **R = R_yaw · R_pitch · R_roll**.
//! This means: start with the identity, then apply R_roll, then R_pitch,
//! then R_yaw — the resulting matrix transforms a point as if it were
//! rotated by roll, then pitch, then yaw in world space.
//!
//! The axis conventions follow the right-handed coordinate system with
//! Y-up: +Y is up, +Z is forward, +X is right.
//!
//! ## Pitch (about +X): positive rotates +Y toward +Z
//! The spec states: "a point at (0,-1,0) (a hanging limb) moves toward
//! -Z (backward, since the humanoid's local forward is +Z)".
//! At a quarter pitch (turn = 16384), +Y -> +Z, +Z -> -Y.
//!
//! ## Yaw (about +Y): positive rotates +X toward -Z
//! This matches the proxy_instances convention: at a quarter turn
//! (turn = 16384), +X -> -Z (right-handed Y-up).
//!
//! ## Roll (about +Z): positive rotates +X toward +Y
//! At a quarter roll (turn = 16384), +X -> +Y, +Y -> -X.
//!
//! # THE SKINNING EQUATION (the CPU reference the WGSL will mirror)
//! `skinned_pos = Σ w_i · (M_i · [pos; 1])` (left-to-right, weighted sum)
//! `skinned_normal = normalize(Σ w_i · (N_i · normal))` where N_i is the
//! upper 3x3 of M_i.
//!
//! # THE REST-IDENTITY PROOF (the pixel-perfect promise)
//! The skin_matrices function produces matrices M_j such that when the
//! pose is all zeros (rest), every M_j is BIT-EXACT identity. This is
//! because:
//!
//!   J_j = translate(bind_world_j) at rest (no rotation, no delta_pos)
//!   M_j = J_j · translate(-bind_world_j) = translate(bind_world_j) · translate(-bind_world_j)
//!
//! And translate(w) · translate(-w) = translate(w - w) = translate(0)
//! which is the identity matrix. Every element is bit-exact because
//! f32 addition/subtraction is IEEE-754 compliant (the commutative
//! property holds for finite reals, and 0 is exact).
//!
//! This means the lit-scene oracle can use the unskinned mesh directly
//! for the rest pose — NO special handling needed — the vertex shader
//! will compute the same position as the unskinned path.

#![deny(unsafe_code)]

use std::sync::OnceLock;

/// The fixed half-turn constant (the nearest f32 to π/2 — a fixed
/// bit pattern, identical on every platform).
const FRAC_PI_2: f32 = core::f32::consts::FRAC_PI_2;

/// The deterministic sine/cosine of a turn fraction. Quadrant
/// reduction plus fixed-order fixed-constant Taylor series (Horner
/// evaluation, pure IEEE f32 arithmetic — no libm, no platform
/// variation). Documented precision: absolute error ≤ 2e-6 over the
/// full turn — ample for proxy orientation, where the contract is
/// BIT-IDENTITY across platforms, not libm-grade accuracy.
///
/// Moved here from engine-renderer's proxy_instances at E-S1 (the
/// animation core's rotations and the instance packing share the ONE
/// libm-free trigonometry — the convention exists exactly once;
/// proxy_instances re-exports this definition for its existing
/// consumers, and its known-answer tests moved with it).
pub fn sin_cos_turn(turn: u16) -> (f32, f32) {
    let quadrant = (turn >> 14) & 3;
    // The within-quadrant angle: [0, π/2).
    let x = (turn & 0x3FFF) as f32 * (FRAC_PI_2 / 16384.0);
    let x2 = x * x;
    // sin(x): x − x³/3! + x⁵/5! − x⁷/7! + x⁹/9! − x¹¹/11! (Horner).
    let sin_x = x
        * (1.0
            + x2 * (-1.0 / 6.0
                + x2 * (1.0 / 120.0
                    + x2 * (-1.0 / 5040.0 + x2 * (1.0 / 362880.0 + x2 * (-1.0 / 39916800.0))))));
    // cos(x): 1 − x²/2! + x⁴/4! − x⁶/6! + x⁸/8! − x¹⁰/10! (Horner).
    let cos_x = 1.0
        + x2 * (-1.0 / 2.0
            + x2 * (1.0 / 24.0
                + x2 * (-1.0 / 720.0 + x2 * (1.0 / 40320.0 + x2 * (-1.0 / 3628800.0)))));
    match quadrant {
        0 => (sin_x, cos_x),
        1 => (cos_x, -sin_x),
        2 => (-sin_x, -cos_x),
        _ => (-cos_x, sin_x),
    }
}

/// The number of joints in the v2 humanoid skeleton.
pub const HUMANOID_JOINTS: usize = 11;

/// The cached rest pose block - computed once at first call and reused.
/// The rest pose is a pure function of the immutable skeleton, so we cache it
/// once to avoid recomputing for every stationary NPC on every frame.
/// (`std::sync::OnceLock` — no external dependency for this.)
static REST_POSE_BLOCK_CACHE: OnceLock<[[[f32; 4]; 4]; HUMANOID_JOINTS]> = OnceLock::new();

/// The pose stride: 11 column-major mat4s of f32, 64 bytes each = 704
/// bytes. The per-joint 64 B IS one column-major mat4: 16 f32 × 4 B —
/// four columns of four f32 (`[[f32; 4]; 4]`, packed column 0's
/// x/y/z/w then column 1, ..., per [`pack_pose_bytes`]) — the exact
/// `mat4x4<f32>` column-major layout the shaders read, so pose
/// blocks upload with no conversion.
pub const POSE_STRIDE: usize = HUMANOID_JOINTS * 64;

/// A joint in the skeleton: the parent reference and the bind pivot in
/// MESH space (the anatomical pivot, NOT the box center).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Joint {
    /// The parent joint index, or None for the root (pelvis).
    pub parent: Option<usize>,
    /// The pivot point in mesh-local coordinates (the anatomical pivot).
    pub pivot: [f32; 3],
}

/// The v2 humanoid skeleton: 11 joints in this order:
/// 0: pelvis (root)
/// 1: torso
/// 2: head
/// 3: upper_arm_l
/// 4: upper_arm_r
/// 5: lower_arm_l
/// 6: lower_arm_r
/// 7: upper_leg_l
/// 8: upper_leg_r
/// 9: lower_leg_l
/// 10: lower_leg_r
pub fn humanoid_skeleton() -> [Joint; HUMANOID_JOINTS] {
    [
        // 0: pelvis (root, no parent), pivot at hips
        Joint {
            parent: None,
            pivot: [0.0, 0.98, 0.0],
        },
        // 1: torso (parent: pelvis), waist pivot
        Joint {
            parent: Some(0),
            pivot: [0.0, 1.06, 0.0],
        },
        // 2: head (parent: torso), neck pivot
        Joint {
            parent: Some(1),
            pivot: [0.0, 1.60, 0.0],
        },
        // 3: upper_arm_l (parent: torso), left shoulder
        Joint {
            parent: Some(1),
            pivot: [-0.21, 1.58, 0.0],
        },
        // 4: upper_arm_r (parent: torso), right shoulder
        Joint {
            parent: Some(1),
            pivot: [0.21, 1.58, 0.0],
        },
        // 5: lower_arm_l (parent: upper_arm_l), left elbow
        Joint {
            parent: Some(3),
            pivot: [-0.21, 1.18, 0.0],
        },
        // 6: lower_arm_r (parent: upper_arm_r), right elbow
        Joint {
            parent: Some(4),
            pivot: [0.21, 1.18, 0.0],
        },
        // 7: upper_leg_l (parent: pelvis), left hip
        Joint {
            parent: Some(0),
            pivot: [-0.10, 0.90, 0.0],
        },
        // 8: upper_leg_r (parent: pelvis), right hip
        Joint {
            parent: Some(0),
            pivot: [0.10, 0.90, 0.0],
        },
        // 9: lower_leg_l (parent: upper_leg_l), left knee
        Joint {
            parent: Some(7),
            pivot: [-0.10, 0.45, 0.0],
        },
        // 10: lower_leg_r (parent: upper_leg_r), right knee
        Joint {
            parent: Some(8),
            pivot: [0.10, 0.45, 0.0],
        },
    ]
}

/// Mapping from mesh box emission order (from humanoid_mesh) to joint id.
/// The v2 humanoid mesh emits boxes in this order:
/// 0: head, 1: torso, 2: pelvis, 3: upper_arm_l, 4: upper_arm_r,
/// 5: lower_arm_l, 6: lower_arm_r, 7: upper_leg_l, 8: upper_leg_r,
/// 9: lower_leg_l, 10: lower_leg_r
/// Each box binds to the joint whose pivot is at that box's anatomical
/// anchor (head box → head joint, torso box → torso joint, etc.).
pub const BOX_JOINT: [usize; HUMANOID_JOINTS] = [2, 1, 0, 3, 4, 5, 6, 7, 8, 9, 10];

/// The bounding boxes for each of the 11 mesh boxes, in emission order
/// (head, torso, pelvis, upper_arm_l, upper_arm_r, lower_arm_l, lower_arm_r,
/// upper_leg_l, upper_leg_r, lower_leg_l, lower_leg_r).
/// Values copied EXACTLY from humanoid_mesh's box_faces calls.
pub const HUMANOID_BOXES: [([f32; 3], [f32; 3]); HUMANOID_JOINTS] = [
    // 0: head [-0.11, 1.60, -0.11] to [0.11, 1.80, 0.11]
    ([-0.11, 1.60, -0.11], [0.11, 1.80, 0.11]),
    // 1: torso [-0.21, 1.06, -0.12] to [0.21, 1.60, 0.12]
    ([-0.21, 1.06, -0.12], [0.21, 1.60, 0.12]),
    // 2: pelvis [-0.19, 0.90, -0.11] to [0.19, 1.06, 0.11]
    ([-0.19, 0.90, -0.11], [0.19, 1.06, 0.11]),
    // 3: upper_arm_l [-0.33, 1.18, -0.06] to [-0.21, 1.58, 0.06]
    ([-0.33, 1.18, -0.06], [-0.21, 1.58, 0.06]),
    // 4: upper_arm_r [0.21, 1.18, -0.06] to [0.33, 1.58, 0.06]
    ([0.21, 1.18, -0.06], [0.33, 1.58, 0.06]),
    // 5: lower_arm_l [-0.31, 0.78, -0.05] to [-0.21, 1.18, 0.05]
    ([-0.31, 0.78, -0.05], [-0.21, 1.18, 0.05]),
    // 6: lower_arm_r [0.21, 0.78, -0.05] to [0.31, 1.18, 0.05]
    ([0.21, 0.78, -0.05], [0.31, 1.18, 0.05]),
    // 7: upper_leg_l [-0.17, 0.45, -0.08] to [-0.03, 0.90, 0.08]
    ([-0.17, 0.45, -0.08], [-0.03, 0.90, 0.08]),
    // 8: upper_leg_r [0.03, 0.45, -0.08] to [0.17, 0.90, 0.08]
    ([0.03, 0.45, -0.08], [0.17, 0.90, 0.08]),
    // 9: lower_leg_l [-0.15, 0.00, -0.08] to [-0.03, 0.45, 0.12]
    ([-0.15, 0.00, -0.08], [-0.03, 0.45, 0.12]),
    // 10: lower_leg_r [0.03, 0.00, -0.08] to [0.15, 0.45, 0.12]
    ([0.03, 0.00, -0.08], [0.15, 0.45, 0.12]),
];

/// A single joint's pose delta from rest: position offset and rotation
/// as turn fractions (pitch, yaw, roll).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointPose {
    /// Delta position from the rest pose, in mesh-local coordinates.
    pub delta_pos: [f32; 3],
    /// Delta rotation as SIGNED turn fractions (1/65536 of a full
    /// rotation, the unit [`sin_cos_turn`]'s turn scale pins): pitch
    /// (about +X), yaw (about +Y), roll (about +Z). SIGNED because
    /// joint swings CROSS ZERO every half cycle — an unsigned wrap
    /// would force keyframe interpolation the long way around the
    /// turn circle (a half-rotation limb sweep between adjacent
    /// keyframes; the zero-crossing pin in the tests exists to
    /// catch exactly that). The [`sin_cos_turn`] boundary takes the
    /// two's-complement `as u16` image — a negative turn and its
    /// u16 wrap are the same angle modulo a full rotation, which is
    /// all a rotation matrix ever sees.
    pub delta_turns: [i16; 3],
}

/// One keyframe in a clip: a time in seconds and the full pose at that time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keyframe {
    /// Time in seconds, monotonically increasing within the clip.
    pub time: f32,
    /// The full joint pose at this keyframe.
    pub pose: [JointPose; HUMANOID_JOINTS],
}

/// A clip: a duration and a sequence of keyframes.
/// The clip loops: the last keyframe's pose is cross-faded to the first
/// keyframe's pose over the remaining time to duration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clip {
    /// The clip duration in seconds.
    pub duration: f32,
    /// The keyframes, sorted by time, first at 0.0, last < duration.
    /// The wrap is handled by the sampler: for t after the last keyframe,
    /// interpolate from the last keyframe to the first over the remaining time.
    pub keyframes: &'static [Keyframe],
}

/// The rotation matrix for a single turn fraction around a given axis.
/// Returns the 3x3 rotation matrix as 9 f32s in column-major order.
/// The function is libm-free, using [`sin_cos_turn`] for the trigonometry.
fn rotation_matrix_for_axis(turn: u16, axis: usize) -> [f32; 9] {
    let (s, c) = sin_cos_turn(turn);
    match axis {
        0 => {
            // Pitch: rotation about +X
            // Positive pitch rotates +Y toward +Z (right-hand rule).
            // At 90°: Y -> Z, Z -> -Y
            // Column 0 (X basis): [1, 0, 0]
            // Column 1 (Y basis): [0, c, s]
            // Column 2 (Z basis): [0, -s, c]
            [
                1.0, 0.0, 0.0, // col 0: X -> X
                0.0, c, s, // col 1: Y -> Y·c + Z·s
                0.0, -s, c, // col 2: Z -> -Y·s + Z·c
            ]
        }
        1 => {
            // Yaw: rotation about +Y
            // +X -> +X·c + +Z·s, +Z -> -X·s + +Z·c
            // Column 0 (X basis): [c, 0, -s]
            // Column 1 (Y basis): [0, 1, 0]
            // Column 2 (Z basis): [s, 0, c]
            [
                c, 0.0, -s, // col 0: X -> X·c - Z·s
                0.0, 1.0, 0.0, // col 1: Y -> Y
                s, 0.0, c, // col 2: Z -> X·s + Z·c
            ]
        }
        2 => {
            // Roll: rotation about +Z
            // +X -> +X·c - +Y·s, +Y -> +X·s + +Y·c
            // Column 0 (X basis): [c, s, 0]
            // Column 1 (Y basis): [-s, c, 0]
            // Column 2 (Z basis): [0, 0, 1]
            [
                c, s, 0.0, // col 0: X -> X·c + Y·s
                -s, c, 0.0, // col 1: Y -> -X·s + Y·c
                0.0, 0.0, 1.0, // col 2: Z -> Z
            ]
        }
        _ => unreachable!(),
    }
}

/// Multiplies two column-major 3x3 matrices: out = A · B. The entry
/// out[col·3 + row] = Σᵢ a[i·3 + row] · b[col·3 + i] — column `col`
/// of the product is A applied to B's column `col` (the same
/// column-image rule [`matrix_multiply_4x4`] pins at 4x4). Pure f32
/// in the fixed i-then-row order. Public here and re-exported
/// crate-visibly by engine-renderer's animation shim (E-S1:
/// `shadows.rs`'s populated oracle composes the same-convention
/// rotations through this ONE implementation — the convention
/// exists exactly once).
pub fn mat3_multiply(a: [f32; 9], b: [f32; 9]) -> [f32; 9] {
    let mut out = [0.0; 9];
    for col in 0..3 {
        for row in 0..3 {
            out[col * 3 + row] =
                a[row] * b[col * 3] + a[3 + row] * b[col * 3 + 1] + a[6 + row] * b[col * 3 + 2];
        }
    }
    out
}

/// Composes a 4x4 transform from a column-major 3x3 rotation and a
/// translation, in THE REPO'S COLUMN-MAJOR [[f32; 4]; 4] CONVENTION
/// (the one the renderer's instance packing pins and
/// the WGSL `mat4x4<f32>(c0, c1, c2, c3)` constructor consumes):
/// the outer index is the COLUMN, columns 0..2 are the rotation's
/// basis images, and the translation is column 3's xyz with w = 1.
/// A vector applies as clip = Σ columns[i] · p[i] (the form
/// [`skin_vertex`] evaluates). Writing the rotation's rows as
/// columns here is the transpose bug the 6-F build once shipped —
/// the end-to-end quarter-pitch test below exists to catch it.
fn compose_transform_matrix(rotation: [f32; 9], translation: [f32; 3]) -> [[f32; 4]; 4] {
    // rotation is column-major: rotation[col*3 + row].
    [
        [rotation[0], rotation[1], rotation[2], 0.0], // column 0: the X basis image
        [rotation[3], rotation[4], rotation[5], 0.0], // column 1: the Y basis image
        [rotation[6], rotation[7], rotation[8], 0.0], // column 2: the Z basis image
        [translation[0], translation[1], translation[2], 1.0], // column 3: the translation
    ]
}

/// Composes a joint's local transform matrix from its pose.
/// The local transform is: translate(bind_local_pivot_relative + delta_pos)
/// then rotate (the fixed Euler order R = R_yaw · R_pitch · R_roll).
/// Takes the whole skeleton (the parent's pivot feeds the bind-local
/// offset) — one pure walk per call, no per-joint reconstruction.
fn compose_local_matrix(
    skeleton: &[Joint; HUMANOID_JOINTS],
    joint: &Joint,
    pose: &JointPose,
) -> [[f32; 4]; 4] {
    // Compute the local pivot (relative to parent for non-root, absolute for root).
    let bind_local_pivot = if let Some(parent_idx) = joint.parent {
        let parent = &skeleton[parent_idx];
        [
            joint.pivot[0] - parent.pivot[0],
            joint.pivot[1] - parent.pivot[1],
            joint.pivot[2] - parent.pivot[2],
        ]
    } else {
        joint.pivot
    };

    // Compute the translation: bind_local_pivot + delta_pos.
    let translation = [
        bind_local_pivot[0] + pose.delta_pos[0],
        bind_local_pivot[1] + pose.delta_pos[1],
        bind_local_pivot[2] + pose.delta_pos[2],
    ];

    // Compose the rotation: R = R_yaw · R_pitch · R_roll (fixed order —
    // the roll applies to the point first, the yaw last).
    // The signed turns take their two's-complement u16 image at the
    // sin_cos_turn boundary (the JointPose docs' angle equivalence).
    let rot_roll = rotation_matrix_for_axis(pose.delta_turns[2] as u16, 2);
    let rot_pitch = rotation_matrix_for_axis(pose.delta_turns[0] as u16, 0);
    let rot_yaw = rotation_matrix_for_axis(pose.delta_turns[1] as u16, 1);

    // R = R_yaw · (R_pitch · R_roll), column-major 3x3s.
    let rotation = mat3_multiply(rot_yaw, mat3_multiply(rot_pitch, rot_roll));

    compose_transform_matrix(rotation, translation)
}

/// Composes the world transform matrices for all joints given a pose.
/// The joint hierarchy's matrix composition: parents always have lower
/// indices (asserted in tests), so we can compose in joint index order.
/// Returns one 4x4 matrix per joint (COLUMN-MAJOR [[f32;4];4]).
pub fn compose_joint_matrices(
    skeleton: &[Joint; HUMANOID_JOINTS],
    pose: &[JointPose; HUMANOID_JOINTS],
) -> [[[f32; 4]; 4]; HUMANOID_JOINTS] {
    let mut result = [[[0.0; 4]; 4]; HUMANOID_JOINTS];

    for (idx, joint) in skeleton.iter().enumerate() {
        let local_matrix = compose_local_matrix(skeleton, joint, &pose[idx]);

        if let Some(parent_idx) = joint.parent {
            // Chain-compose: world = parent_world · local
            let parent_world = result[parent_idx];
            result[idx] = matrix_multiply_4x4(parent_world, local_matrix);
        } else {
            // Root: world = local
            result[idx] = local_matrix;
        }
    }

    result
}

/// Multiplies two 4x4 matrices in THE REPO'S COLUMN-MAJOR CONVENTION:
/// out = A · B, where column `j` of the product is A applied to B's
/// column `j` — out[j][row] = Σᵢ a[i][row] · b[j][i]. This is the
/// form that composes `parent_world · local` correctly (the child's
/// local space maps through the parent's full transform) and that
/// [`skin_vertex`] applies to vectors. The transposed form (row
/// images instead of column images) silently inverts every rotation
/// — the end-to-end convention test exists to catch it.
fn matrix_multiply_4x4(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            out[col][row] = a[0][row] * b[col][0]
                + a[1][row] * b[col][1]
                + a[2][row] * b[col][2]
                + a[3][row] * b[col][3];
        }
    }
    out
}

/// The precomputed skin bind data for one skeleton: the per-joint
/// inverse-bind matrices `translate(-bind_world_j)` derived from the
/// chain-composed REST pose (the all-zero `JointPose`). A pure function
/// of the immutable skeleton — construct once per evaluation batch (or
/// once per process for a fixed skeleton) and share across every
/// [`skin_matrices_bound`] call, instead of recomposing the rest chain
/// per skinned NPC per frame.
pub struct SkinBind {
    inv_bind: [[[f32; 4]; 4]; HUMANOID_JOINTS],
}

impl SkinBind {
    /// Computes the bind exactly as [`skin_matrices`] does internally —
    /// the same fixed operation order, so the rest-identity bit-exactness
    /// contract carries over unchanged.
    pub fn from_skeleton(skeleton: &[Joint; HUMANOID_JOINTS]) -> Self {
        let rest_pose = [JointPose {
            delta_pos: [0.0; 3],
            delta_turns: [0; 3],
        }; HUMANOID_JOINTS];
        let rest_worlds = compose_joint_matrices(skeleton, &rest_pose);
        let mut inv_bind = [[[0.0; 4]; 4]; HUMANOID_JOINTS];
        for idx in 0..HUMANOID_JOINTS {
            // Extract the bind world translation from the rest world matrix.
            let bind_world_translation = [
                rest_worlds[idx][3][0],
                rest_worlds[idx][3][1],
                rest_worlds[idx][3][2],
            ];
            // Build the translation matrix for -bind_world.
            inv_bind[idx] = compose_transform_matrix(
                [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], // identity rotation
                [
                    -bind_world_translation[0],
                    -bind_world_translation[1],
                    -bind_world_translation[2],
                ],
            );
        }
        SkinBind { inv_bind }
    }
}

/// Computes the skin matrices from precomputed bind data: `M_j = J_j ·
/// inv_bind_j` per joint — the final multiply of [`skin_matrices`], the
/// rest-chain cost already paid once at [`SkinBind::from_skeleton`].
pub fn skin_matrices_bound(
    joint_worlds: &[[[f32; 4]; 4]; HUMANOID_JOINTS],
    bind: &SkinBind,
) -> [[[f32; 4]; 4]; HUMANOID_JOINTS] {
    let mut result = [[[0.0; 4]; 4]; HUMANOID_JOINTS];
    for idx in 0..HUMANOID_JOINTS {
        // M_j = J_j · inv_bind
        result[idx] = matrix_multiply_4x4(joint_worlds[idx], bind.inv_bind[idx]);
    }
    result
}

/// Computes the skin matrices from the joint world matrices and the skeleton.
/// M_j = J_j · translate(-bind_world_j) where bind_world_j is the chain-composed
/// REST translation of joint j (compose the rest locals — all-zero JointPose —
/// and take the translation column).
/// This makes the rest pose produce BIT-EXACT identity skin matrices.
///
/// Delegates to [`SkinBind::from_skeleton`] + [`skin_matrices_bound`] with the
/// identical fixed operation order, so every digest pin carries over. Hot
/// paths (per-NPC-per-frame pose evaluation) should construct the bind once
/// per batch and call [`skin_matrices_bound`]; this from-skeleton form stays
/// for oracles, one-shot use, and any future non-humanoid skeleton.
pub fn skin_matrices(
    joint_worlds: &[[[f32; 4]; 4]; HUMANOID_JOINTS],
    skeleton: &[Joint; HUMANOID_JOINTS],
) -> [[[f32; 4]; 4]; HUMANOID_JOINTS] {
    skin_matrices_bound(joint_worlds, &SkinBind::from_skeleton(skeleton))
}

/// A skinned vertex: the same structure as ProxyVertex plus skinning info.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SkinnedVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u32; 4],  // Up to 4 joint indices
    pub weights: [f32; 4], // Up to 4 weights (sum to 1.0)
}

/// Skins one vertex using the skinning equation.
/// `skinned_pos = Σ w_i · (M_i · [pos; 1])`
/// `skinned_normal = normalize(Σ w_i · (N_i · normal))` where N_i is the upper 3x3.
pub fn skin_vertex(
    vertex: &SkinnedVertex,
    mats: &[[[f32; 4]; 4]; HUMANOID_JOINTS],
) -> ([f32; 3], [f32; 3]) {
    let mut skinned_pos = [0.0; 3];
    let mut skinned_normal = [0.0; 3];

    // The skinning equation: weighted sum of transformed positions and normals.
    for i in 0..4 {
        let weight = vertex.weights[i];
        if weight == 0.0 {
            continue; // Zero-weight terms vanish exactly in IEEE.
        }

        let joint_idx = vertex.joints[i] as usize;
        let mat = &mats[joint_idx];

        // Transform the position: [x, y, z, 1] * mat (column-major).
        let pos_h = [
            vertex.pos[0] * mat[0][0]
                + vertex.pos[1] * mat[1][0]
                + vertex.pos[2] * mat[2][0]
                + mat[3][0],
            vertex.pos[0] * mat[0][1]
                + vertex.pos[1] * mat[1][1]
                + vertex.pos[2] * mat[2][1]
                + mat[3][1],
            vertex.pos[0] * mat[0][2]
                + vertex.pos[1] * mat[1][2]
                + vertex.pos[2] * mat[2][2]
                + mat[3][2],
            vertex.pos[0] * mat[0][3]
                + vertex.pos[1] * mat[1][3]
                + vertex.pos[2] * mat[2][3]
                + mat[3][3],
        ];

        // Scale by weight and accumulate.
        skinned_pos[0] += weight * pos_h[0];
        skinned_pos[1] += weight * pos_h[1];
        skinned_pos[2] += weight * pos_h[2];

        // Transform the normal: [x, y, z, 0] * mat (upper 3x3 only).
        let normal_h = [
            vertex.normal[0] * mat[0][0]
                + vertex.normal[1] * mat[1][0]
                + vertex.normal[2] * mat[2][0],
            vertex.normal[0] * mat[0][1]
                + vertex.normal[1] * mat[1][1]
                + vertex.normal[2] * mat[2][1],
            vertex.normal[0] * mat[0][2]
                + vertex.normal[1] * mat[1][2]
                + vertex.normal[2] * mat[2][2],
        ];

        // Scale by weight and accumulate.
        skinned_normal[0] += weight * normal_h[0];
        skinned_normal[1] += weight * normal_h[1];
        skinned_normal[2] += weight * normal_h[2];
    }

    // Normalize the skinned normal (IEEE sqrt is allowed).
    let len = (skinned_normal[0] * skinned_normal[0]
        + skinned_normal[1] * skinned_normal[1]
        + skinned_normal[2] * skinned_normal[2])
        .sqrt();

    if len > 0.0 {
        skinned_normal[0] /= len;
        skinned_normal[1] /= len;
        skinned_normal[2] /= len;
    }

    (skinned_pos, skinned_normal)
}

/// Computes the world-space AABB of a mesh box under the skin matrices.
/// Transforms the box's 8 corners through M_{BOX_JOINT[box_index]}.
pub fn posed_box_bounds(
    mats: &[[[f32; 4]; 4]; HUMANOID_JOINTS],
    box_index: usize,
) -> ([f32; 3], [f32; 3]) {
    let joint_idx = BOX_JOINT[box_index];
    let mat = &mats[joint_idx];

    let (lo, hi) = HUMANOID_BOXES[box_index];

    // Transform the 8 corners.
    let corners = [
        [lo[0], lo[1], lo[2], 1.0],
        [lo[0], lo[1], hi[2], 1.0],
        [lo[0], hi[1], lo[2], 1.0],
        [lo[0], hi[1], hi[2], 1.0],
        [hi[0], lo[1], lo[2], 1.0],
        [hi[0], lo[1], hi[2], 1.0],
        [hi[0], hi[1], lo[2], 1.0],
        [hi[0], hi[1], hi[2], 1.0],
    ];

    let mut transformed = [0.0; 8 * 3];
    for (i, corner) in corners.iter().enumerate() {
        let t = &mut transformed[i * 3..(i + 1) * 3];
        t[0] = corner[0] * mat[0][0] + corner[1] * mat[1][0] + corner[2] * mat[2][0] + mat[3][0];
        t[1] = corner[0] * mat[0][1] + corner[1] * mat[1][1] + corner[2] * mat[2][1] + mat[3][1];
        t[2] = corner[0] * mat[0][2] + corner[1] * mat[1][2] + corner[2] * mat[2][2] + mat[3][2];
    }

    // Compute min/max.
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];

    for i in 0..8 {
        for j in 0..3 {
            let v = transformed[i * 3 + j];
            if v < min[j] {
                min[j] = v;
            }
            if v > max[j] {
                max[j] = v;
            }
        }
    }

    (min, max)
}

/// Packs a pose block into bytes: 11 matrices, column-major, f32 bits,
/// little-endian, joint order 0..11, fixed field order (column 0 x,y,z,w
/// then column 1... per matrix).
pub fn pack_pose_bytes(mats: &[[[f32; 4]; 4]; HUMANOID_JOINTS]) -> [u8; POSE_STRIDE] {
    let mut bytes = [0u8; POSE_STRIDE];
    let mut offset = 0;

    for mat in mats.iter() {
        for col in mat.iter() {
            for &f in col.iter() {
                let bits = f.to_le_bytes();
                bytes[offset..offset + 4].copy_from_slice(&bits);
                offset += 4;
            }
        }
    }

    bytes
}

/// Returns the rest pose block: the skin matrices at the all-zero pose.
/// These are BIT-EXACT identity matrices (the rest-identity proof).
/// Uses a cached value computed once at first call.
pub fn rest_pose_block() -> [[[f32; 4]; 4]; HUMANOID_JOINTS] {
    *REST_POSE_BLOCK_CACHE.get_or_init(|| {
        let skeleton = humanoid_skeleton();
        let rest_pose = [JointPose {
            delta_pos: [0.0; 3],
            delta_turns: [0; 3],
        }; HUMANOID_JOINTS];
        let joint_worlds = compose_joint_matrices(&skeleton, &rest_pose);
        skin_matrices(&joint_worlds, &skeleton)
    })
}

/// Samples a pose from a clip at time t (modulo duration).
pub fn sample_pose(clip: &Clip, t: f32) -> [JointPose; HUMANOID_JOINTS] {
    // Reduce t modulo duration.
    let t = t.rem_euclid(clip.duration);

    if clip.keyframes.is_empty() {
        // Empty clip: return zero pose.
        return [JointPose {
            delta_pos: [0.0; 3],
            delta_turns: [0; 3],
        }; HUMANOID_JOINTS];
    }

    // Find the bracketing keyframe pair.
    // For t after the last keyframe, interpolate from the last to the first.
    let n = clip.keyframes.len();

    if n == 1 {
        // Single keyframe: return it.
        return clip.keyframes[0].pose;
    }

    // Linear scan in fixed order.
    let mut t0_idx = 0;
    let mut t1_idx = 1;

    if t >= clip.keyframes[n - 1].time {
        // After the last keyframe: wrap to first.
        t0_idx = n - 1;
        t1_idx = 0;
    } else {
        // Find the pair.
        for i in 0..n - 1 {
            if t >= clip.keyframes[i].time && t < clip.keyframes[i + 1].time {
                t0_idx = i;
                t1_idx = i + 1;
                break;
            }
        }
    }

    let kf0 = &clip.keyframes[t0_idx];
    let kf1 = &clip.keyframes[t1_idx];

    let t0 = kf0.time;
    let t1 = kf1.time;

    // Compute alpha.
    let alpha = if t1_idx == 0 && t0_idx == n - 1 {
        // Wrap case: t1 is first, t0 is last.
        (t - t0) / (clip.duration - t0)
    } else {
        (t - t0) / (t1 - t0)
    };

    // Interpolate each joint's pose.
    let mut result = [JointPose {
        delta_pos: [0.0; 3],
        delta_turns: [0; 3],
    }; HUMANOID_JOINTS];

    for (j, result_j) in result.iter_mut().enumerate() {
        let p0 = &kf0.pose[j];
        let p1 = &kf1.pose[j];

        // delta_pos: component-wise lerp.
        result_j.delta_pos = [
            p0.delta_pos[0] + (p1.delta_pos[0] - p0.delta_pos[0]) * alpha,
            p0.delta_pos[1] + (p1.delta_pos[1] - p0.delta_pos[1]) * alpha,
            p0.delta_pos[2] + (p1.delta_pos[2] - p0.delta_pos[2]) * alpha,
        ];

        // delta_turns: SIGNED lerp as f32 (crossing zero the short way
        // — the JointPose docs' zero-crossing contract), then round to
        // the nearest integer turn with ties away from zero (f32::
        // round's documented semantics), clamped to the i16 range.
        for k in 0..3 {
            let v0: f32 = p0.delta_turns[k].into();
            let v1: f32 = p1.delta_turns[k].into();
            let lerped = v0 + (v1 - v0) * alpha;
            let rounded = lerped.round();
            result_j.delta_turns[k] = if rounded > 32767.0 {
                32767
            } else if rounded < -32768.0 {
                -32768
            } else {
                rounded as i16
            };
        }
    }

    result
}

/// The FNV-1a 64-bit constants (the repo's standard).
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Samples the clip at `samples` points and computes the pose sequence digest.
/// This is the cross-platform byte pin (the FNV discipline).
pub fn pose_sequence_digest(
    clip: &Clip,
    skeleton: &[Joint; HUMANOID_JOINTS],
    samples: usize,
) -> u64 {
    let mut hasher = FNV_OFFSET_BASIS;

    for i in 0..samples {
        let t = (i as f32) * clip.duration / (samples as f32);
        let pose = sample_pose(clip, t);
        let joint_worlds = compose_joint_matrices(skeleton, &pose);
        let skin_mats = skin_matrices(&joint_worlds, skeleton);
        let bytes = pack_pose_bytes(&skin_mats);

        // Fold with FNV-1a.
        for &byte in bytes.iter() {
            hasher ^= byte as u64;
            hasher = hasher.wrapping_mul(FNV_PRIME);
        }
    }

    hasher
}

/// The walk clip: one walking stride, duration 1.1 s, 8 keyframes at
/// t = k · 0.1375 (k = 0..7) — the stride phase φ = k · 45°. The
/// sampler's seam interpolates keyframe 7 back to keyframe 0 over
/// the remaining 0.1375 s, closing the loop.
///
/// AUTHORING METHOD: every field is a fixed sinusoid of the stride
/// phase, SAMPLED at the eight 45° marks and frozen as integer turn
/// data — the runtime path never evaluates a transcendental (the
/// libm-free discipline; the values were computed offline from the
/// formulas in each field's comment). The signed-turn helper
/// truncates toward zero; the amplitude pins in the tests carry the
/// ≤ 1-unit rounding slack.
///
/// THE GAIT MODEL (φ measured from the left leg's heel strike;
/// "forward" is local +Z, so a FORWARD limb reach is NEGATIVE pitch
/// — positive pitch swings a hanging limb backward, the pinned
/// convention in this module's header):
/// - upper_leg_l pitch = −0.055·sin(φ): the left leg swings FORWARD
///   over φ ∈ (0°, 180°), trails over (180°, 360°); upper_leg_r is
///   the exact anti-phase (+0.055·sin(φ)).
/// - lower_leg_l (the knee) = +0.09·max(0, sin(φ)): the knee flexes
///   ONLY during its own leg's forward swing (max flexion at
///   mid-swing, φ = 90°), straight at heel strike and through
///   stance — the knee never hyper-extends (positive-only).
///   lower_leg_r mirrors it on the other half-stride.
/// - upper_arm_l pitch = +0.045·sin(φ) — ANTI-PHASE to the same-side
///   leg (the left arm reaches forward while the left leg trails).
/// - lower arms: a slight standing elbow flexion (+0.03) modulated
///   ±0.01 with the arm's swing (more flexion at the forward reach).
/// - pelvis: Δy = −0.02·cos(2φ) — the body VAULTS over the stance
///   leg: lowest at double support (the heel strikes, φ = 0°/180°),
///   highest at mid-stance (φ = 90°/270°), two bobs per stride.
///   Yaw sway +0.008·sin(φ): the left hip advances with the left
///   leg's forward swing.
/// - torso: +0.01·cos(2φ) counter-lean, paired with the bob (double
///   frequency).
/// - head: all zeros — the steady head (the documented choice).
pub fn walk_clip() -> Clip {
    // sin(45°) = √2/2 — the offline-sampled constant the 45°-mark
    // keyframes carry (the only non-trivial sine the table needs).
    const SIN_45: f32 = core::f32::consts::FRAC_1_SQRT_2;

    /// The authoring helper: a turn FRACTION of a full rotation to
    /// its signed i16 units (truncation toward zero — deterministic,
    /// and the amplitude pins carry the slack).
    const fn turn_i16(fraction: f32) -> i16 {
        (fraction * 65536.0) as i16
    }

    // The eight keyframes, phase k · 45°. Each field's comment is
    // its formula AT that phase — the value is the frozen sample.
    const WALK_KEYFRAMES: [Keyframe; 8] = [
        // Keyframe 0: t = 0.0 (φ = 0°, the left leg's heel strike —
        // double support, the body at its lowest).
        Keyframe {
            time: 0.0,
            pose: [
                // 0: pelvis — Δy = −0.02·cos(0) = −0.02; yaw = 0.008·sin(0) = 0.
                JointPose {
                    delta_pos: [0.0, -0.02, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 1: torso — +0.01·cos(0) = +0.01.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.01), 0, 0],
                },
                // 2: head — steady.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 3: upper_arm_l — +0.045·sin(0) = 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 4: upper_arm_r — −0.045·sin(0) = 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 5: lower_arm_l — 0.03 − 0.01·sin(0) = 0.03.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03), 0, 0],
                },
                // 6: lower_arm_r — 0.03 + 0.01·sin(0) = 0.03.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03), 0, 0],
                },
                // 7: upper_leg_l — −0.055·sin(0) = 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 8: upper_leg_r — +0.055·sin(0) = 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 9: lower_leg_l — 0.09·max(0, sin(0)) = 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 10: lower_leg_r — 0.09·max(0, −sin(0)) = 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
            ],
        },
        // Keyframe 1: t = 0.1375 (φ = 45° — the left leg's forward
        // swing begins; the left knee starts to flex).
        Keyframe {
            time: 0.1375,
            pose: [
                // 0: pelvis — Δy = −0.02·cos(90°) = 0; yaw = 0.008·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, turn_i16(0.008 * SIN_45), 0],
                },
                // 1: torso — +0.01·cos(90°) = 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 2: head — steady.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 3: upper_arm_l — +0.045·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.045 * SIN_45), 0, 0],
                },
                // 4: upper_arm_r — −0.045·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.045 * SIN_45), 0, 0],
                },
                // 5: lower_arm_l — 0.03 − 0.01·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03 - 0.01 * SIN_45), 0, 0],
                },
                // 6: lower_arm_r — 0.03 + 0.01·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03 + 0.01 * SIN_45), 0, 0],
                },
                // 7: upper_leg_l — −0.055·sin(45°) (forward swing).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.055 * SIN_45), 0, 0],
                },
                // 8: upper_leg_r — +0.055·sin(45°) (trailing, stance).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.055 * SIN_45), 0, 0],
                },
                // 9: lower_leg_l — 0.09·sin(45°) (flexing).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.09 * SIN_45), 0, 0],
                },
                // 10: lower_leg_r — 0 (the right knee is straight in stance).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
            ],
        },
        // Keyframe 2: t = 0.275 (φ = 90° — mid-swing: the left leg at
        // its forward reach, the left knee at max flexion, the body
        // vaulting high over the straight right stance leg).
        Keyframe {
            time: 0.275,
            pose: [
                // 0: pelvis — Δy = −0.02·cos(180°) = +0.02 (mid-stance
                // high); yaw = 0.008·sin(90°) = +0.008.
                JointPose {
                    delta_pos: [0.0, 0.02, 0.0],
                    delta_turns: [0, turn_i16(0.008), 0],
                },
                // 1: torso — +0.01·cos(180°) = −0.01.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.01), 0, 0],
                },
                // 2: head — steady.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 3: upper_arm_l — +0.045·sin(90°) = +0.045.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.045), 0, 0],
                },
                // 4: upper_arm_r — −0.045.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.045), 0, 0],
                },
                // 5: lower_arm_l — 0.03 − 0.01 = 0.02.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.02), 0, 0],
                },
                // 6: lower_arm_r — 0.03 + 0.01 = 0.04.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.04), 0, 0],
                },
                // 7: upper_leg_l — −0.055 (max forward reach).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.055), 0, 0],
                },
                // 8: upper_leg_r — +0.055 (max trail, stance).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.055), 0, 0],
                },
                // 9: lower_leg_l — 0.09 (max knee flexion).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.09), 0, 0],
                },
                // 10: lower_leg_r — 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
            ],
        },
        // Keyframe 3: t = 0.4125 (φ = 135° — the swing decelerates;
        // the knee extends toward heel strike).
        Keyframe {
            time: 0.4125,
            pose: [
                // 0: pelvis — Δy = −0.02·cos(270°) = 0; yaw = 0.008·sin(135°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, turn_i16(0.008 * SIN_45), 0],
                },
                // 1: torso — +0.01·cos(270°) = 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 2: head — steady.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 3: upper_arm_l — +0.045·sin(135°) = +0.045·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.045 * SIN_45), 0, 0],
                },
                // 4: upper_arm_r — −0.045·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.045 * SIN_45), 0, 0],
                },
                // 5: lower_arm_l — 0.03 − 0.01·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03 - 0.01 * SIN_45), 0, 0],
                },
                // 6: lower_arm_r — 0.03 + 0.01·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03 + 0.01 * SIN_45), 0, 0],
                },
                // 7: upper_leg_l — −0.055·sin(45°) (forward, settling).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.055 * SIN_45), 0, 0],
                },
                // 8: upper_leg_r — +0.055·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.055 * SIN_45), 0, 0],
                },
                // 9: lower_leg_l — 0.09·sin(45°) (extending).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.09 * SIN_45), 0, 0],
                },
                // 10: lower_leg_r — 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
            ],
        },
        // Keyframe 4: t = 0.55 (φ = 180° — the right leg's heel
        // strike; double support, the body low again).
        Keyframe {
            time: 0.55,
            pose: [
                // 0: pelvis — Δy = −0.02·cos(360°) = −0.02; yaw = 0.
                JointPose {
                    delta_pos: [0.0, -0.02, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 1: torso — +0.01·cos(360°) = +0.01.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.01), 0, 0],
                },
                // 2: head — steady.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 3: upper_arm_l — 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 4: upper_arm_r — 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 5: lower_arm_l — 0.03.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03), 0, 0],
                },
                // 6: lower_arm_r — 0.03.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03), 0, 0],
                },
                // 7: upper_leg_l — 0 (the left leg planted under the body).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 8: upper_leg_r — 0 (the right leg at heel strike).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 9: lower_leg_l — 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 10: lower_leg_r — 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
            ],
        },
        // Keyframe 5: t = 0.6875 (φ = 225° — the right leg's forward
        // swing begins; the right knee starts to flex).
        Keyframe {
            time: 0.6875,
            pose: [
                // 0: pelvis — Δy = 0; yaw = 0.008·sin(225°) = −0.008·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, turn_i16(-0.008 * SIN_45), 0],
                },
                // 1: torso — 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 2: head — steady.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 3: upper_arm_l — +0.045·sin(225°) = −0.045·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.045 * SIN_45), 0, 0],
                },
                // 4: upper_arm_r — −0.045·sin(225°) = +0.045·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.045 * SIN_45), 0, 0],
                },
                // 5: lower_arm_l — 0.03 − 0.01·sin(225°) = 0.03 + 0.01·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03 + 0.01 * SIN_45), 0, 0],
                },
                // 6: lower_arm_r — 0.03 + 0.01·sin(225°) = 0.03 − 0.01·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03 - 0.01 * SIN_45), 0, 0],
                },
                // 7: upper_leg_l — −0.055·sin(225°) = +0.055·sin(45°) (trailing, stance).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.055 * SIN_45), 0, 0],
                },
                // 8: upper_leg_r — +0.055·sin(225°) = −0.055·sin(45°) (forward swing).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.055 * SIN_45), 0, 0],
                },
                // 9: lower_leg_l — 0 (the left knee straight in stance).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 10: lower_leg_r — 0.09·sin(45°) (flexing).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.09 * SIN_45), 0, 0],
                },
            ],
        },
        // Keyframe 6: t = 0.825 (φ = 270° — the right leg's mid-swing;
        // the body vaults high over the straight left stance leg).
        Keyframe {
            time: 0.825,
            pose: [
                // 0: pelvis — Δy = +0.02 (mid-stance high); yaw = −0.008.
                JointPose {
                    delta_pos: [0.0, 0.02, 0.0],
                    delta_turns: [0, turn_i16(-0.008), 0],
                },
                // 1: torso — −0.01.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.01), 0, 0],
                },
                // 2: head — steady.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 3: upper_arm_l — −0.045.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.045), 0, 0],
                },
                // 4: upper_arm_r — +0.045.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.045), 0, 0],
                },
                // 5: lower_arm_l — 0.04.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.04), 0, 0],
                },
                // 6: lower_arm_r — 0.02.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.02), 0, 0],
                },
                // 7: upper_leg_l — +0.055 (max trail, stance).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.055), 0, 0],
                },
                // 8: upper_leg_r — −0.055 (max forward reach).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.055), 0, 0],
                },
                // 9: lower_leg_l — 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 10: lower_leg_r — 0.09 (max knee flexion).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.09), 0, 0],
                },
            ],
        },
        // Keyframe 7: t = 0.9625 (φ = 315° — the right swing
        // decelerates toward the seam's heel strike at t = 1.1 ≡ 0).
        Keyframe {
            time: 0.9625,
            pose: [
                // 0: pelvis — Δy = 0; yaw = −0.008·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, turn_i16(-0.008 * SIN_45), 0],
                },
                // 1: torso — 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 2: head — steady.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 3: upper_arm_l — −0.045·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.045 * SIN_45), 0, 0],
                },
                // 4: upper_arm_r — +0.045·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.045 * SIN_45), 0, 0],
                },
                // 5: lower_arm_l — 0.03 + 0.01·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03 + 0.01 * SIN_45), 0, 0],
                },
                // 6: lower_arm_r — 0.03 − 0.01·sin(45°).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.03 - 0.01 * SIN_45), 0, 0],
                },
                // 7: upper_leg_l — +0.055·sin(45°) (trailing, settling).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.055 * SIN_45), 0, 0],
                },
                // 8: upper_leg_r — −0.055·sin(45°) (forward, settling).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(-0.055 * SIN_45), 0, 0],
                },
                // 9: lower_leg_l — 0.
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [0, 0, 0],
                },
                // 10: lower_leg_r — 0.09·sin(45°) (extending).
                JointPose {
                    delta_pos: [0.0, 0.0, 0.0],
                    delta_turns: [turn_i16(0.09 * SIN_45), 0, 0],
                },
            ],
        },
    ];

    Clip {
        duration: 1.1,
        keyframes: &WALK_KEYFRAMES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The known answers at the cardinal turns. Tolerance 1e-5
    /// covers the documented Taylor truncation (≤ 2e-6) with margin.
    /// (Moved with `sin_cos_turn` from engine-renderer's
    /// proxy_instances at E-S1.)
    #[test]
    fn sin_cos_turn_known_answers() {
        let (s, c) = sin_cos_turn(0);
        assert!(
            (s - 0.0).abs() < 1e-5 && (c - 1.0).abs() < 1e-5,
            "zero turn: ({s}, {c})"
        );
        let (s, c) = sin_cos_turn(16384);
        assert!(
            (s - 1.0).abs() < 1e-5 && c.abs() < 1e-5,
            "quarter: ({s}, {c})"
        );
        let (s, c) = sin_cos_turn(32768);
        assert!(s.abs() < 1e-5 && (c + 1.0).abs() < 1e-5, "half: ({s}, {c})");
        let (s, c) = sin_cos_turn(49152);
        assert!(
            (s + 1.0).abs() < 1e-5 && c.abs() < 1e-5,
            "three-quarter: ({s}, {c})"
        );
    }

    /// Opposite turns negate both components (the half-turn
    /// symmetry), and every output stays within [-1, 1] + tolerance.
    /// (Moved with `sin_cos_turn` at E-S1.)
    #[test]
    fn sin_cos_turn_half_turn_symmetry_and_range() {
        for turn in [
            0u16, 1, 8191, 8192, 8193, 16383, 16384, 30000, 32767, 49151, 60000,
        ] {
            let (s, c) = sin_cos_turn(turn);
            let (s_opp, c_opp) = sin_cos_turn(turn.wrapping_add(32768));
            assert!((s + s_opp).abs() < 2e-5, "turn {turn}: {s} vs {s_opp}");
            assert!((c + c_opp).abs() < 2e-5, "turn {turn}: {c} vs {c_opp}");
            assert!((-1.0 - 1e-5..=1.0 + 1e-5).contains(&s));
            assert!((-1.0 - 1e-5..=1.0 + 1e-5).contains(&c));
        }
    }

    /// The zero-pose fixture (rest) — used by many tests.
    fn rest_pose() -> [JointPose; HUMANOID_JOINTS] {
        [JointPose {
            delta_pos: [0.0; 3],
            delta_turns: [0; 3],
        }; HUMANOID_JOINTS]
    }

    /// Skeleton structure: parents precede children; pivots lie on
    /// their boxes' boundaries where claimed; BOX_JOINT maps every
    /// box to a joint whose pivot is at that box's anatomical anchor.
    #[test]
    fn skeleton_structure() {
        let skeleton = humanoid_skeleton();

        // Parents precede children (the compose loop's invariant).
        for (idx, joint) in skeleton.iter().enumerate() {
            if let Some(parent_idx) = joint.parent {
                assert!(
                    parent_idx < idx,
                    "parent {} must precede child {}",
                    parent_idx,
                    idx
                );
            }
        }

        // Pivots at the documented anatomical anchors.
        assert!(
            (skeleton[1].pivot[1] - 1.06).abs() < 1e-5,
            "torso pivot at the waist (pelvis top / torso bottom)"
        );
        assert!(
            (skeleton[2].pivot[1] - 1.60).abs() < 1e-5,
            "head pivot at the neck"
        );
        assert!(
            (skeleton[3].pivot[0] + 0.21).abs() < 1e-5,
            "upper_arm_l pivot at the left shoulder"
        );
        assert!(
            (skeleton[4].pivot[0] - 0.21).abs() < 1e-5,
            "upper_arm_r pivot at the right shoulder"
        );

        // BOX_JOINT maps every mesh box to a joint whose pivot lies
        // inside (or on the boundary of) that box — the rigid
        // binding's sanity.
        for (box_idx, &joint_idx) in BOX_JOINT.iter().enumerate() {
            assert!(joint_idx < HUMANOID_JOINTS);
            let (box_lo, box_hi) = HUMANOID_BOXES[box_idx];
            let pivot = skeleton[joint_idx].pivot;
            for axis in 0..3 {
                assert!(
                    pivot[axis] >= box_lo[axis] - 1e-3 && pivot[axis] <= box_hi[axis] + 1e-3,
                    "box {box_idx} -> joint {joint_idx}: pivot[{axis}] = {} outside [{}, {}]",
                    pivot[axis],
                    box_lo[axis],
                    box_hi[axis]
                );
            }
        }
    }

    /// The rotation axes' known answers on the RAW matrices (the
    /// column-major basis images): quarter pitch maps +Y to +Z;
    /// quarter yaw maps +X to −Z (the instance_columns convention);
    /// quarter roll maps +X to +Y.
    #[test]
    fn rotation_axes_known_answers() {
        let pitch = rotation_matrix_for_axis(16384, 0);
        assert!(
            pitch[3].abs() < 1e-5 && pitch[4].abs() < 1e-5 && (pitch[5] - 1.0).abs() < 1e-5,
            "quarter pitch: column 1 (the +Y image) is (0, 0, 1)"
        );
        let yaw = rotation_matrix_for_axis(16384, 1);
        assert!(
            yaw[0].abs() < 1e-5 && yaw[1].abs() < 1e-5 && (yaw[2] + 1.0).abs() < 1e-5,
            "quarter yaw: column 0 (the +X image) is (0, 0, -1)"
        );
        let roll = rotation_matrix_for_axis(16384, 2);
        assert!(
            roll[0].abs() < 1e-5 && (roll[1] - 1.0).abs() < 1e-5 && roll[2].abs() < 1e-5,
            "quarter roll: column 0 (the +X image) is (0, 1, 0)"
        );
    }

    /// THE END-TO-END CONVENTION PIN — the test the transpose bug
    /// exists to fail. A quarter pitch at the left hip, through the
    /// FULL public chain (compose → skin matrices → skin_vertex),
    /// must swing a point hanging below the pivot BACKWARD (−Z:
    /// the humanoid's local forward is +Z, positive pitch trails a
    /// hanging limb). A transposed matrix core flips this to +Z; a
    /// wrong apply flips it silently everywhere else too.
    #[test]
    fn quarter_pitch_swings_a_hanging_limb_backward() {
        let skeleton = humanoid_skeleton();
        let mut pose = rest_pose();
        pose[7].delta_turns = [16384, 0, 0]; // quarter pitch, left hip.
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let skin_mats = skin_matrices(&joint_worlds, &skeleton);

        // A vertex 0.30 m below the left-hip pivot, rigid to joint 7.
        let pivot = skeleton[7].pivot;
        let hang = [pivot[0], pivot[1] - 0.30, pivot[2]];
        let vertex = SkinnedVertex {
            pos: hang,
            normal: [0.0, -1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [7, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        let (pos, normal) = skin_vertex(&vertex, &skin_mats);
        // Quarter pitch maps (0, -1, 0) to (0, 0, -1): the point
        // lands 0.30 m BEHIND the pivot, at pivot height.
        assert!(
            (pos[0] - pivot[0]).abs() < 2e-5,
            "x stays at the pivot: {pos:?}"
        );
        assert!(
            (pos[1] - pivot[1]).abs() < 2e-5,
            "the limb swings level: {pos:?}"
        );
        assert!(
            (pos[2] - (pivot[2] - 0.30)).abs() < 2e-5,
            "positive pitch trails the limb (−Z): {pos:?} — a transposed \
             matrix core lands it at +Z instead"
        );
        // The hanging normal swings to −Z as well.
        assert!(
            normal[0].abs() < 2e-5 && normal[1].abs() < 2e-5 && (normal[2] + 1.0).abs() < 2e-5,
            "the skinned normal is the rotated hang: {normal:?}"
        );

        // The NEGATIVE pitch swings it forward (+Z).
        pose[7].delta_turns = [-16384, 0, 0];
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let skin_mats = skin_matrices(&joint_worlds, &skeleton);
        let (pos, _) = skin_vertex(&vertex, &skin_mats);
        assert!(
            (pos[2] - (pivot[2] + 0.30)).abs() < 2e-5,
            "negative pitch reaches the limb forward (+Z): {pos:?}"
        );

        // The same convention at the left shoulder (an arm swing).
        pose[7].delta_turns = [0, 0, 0];
        pose[3].delta_turns = [16384, 0, 0];
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let skin_mats = skin_matrices(&joint_worlds, &skeleton);
        let shoulder = skeleton[3].pivot;
        let vertex = SkinnedVertex {
            pos: [shoulder[0], shoulder[1] - 0.30, shoulder[2]],
            normal: [0.0, -1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [3, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        let (pos, _) = skin_vertex(&vertex, &skin_mats);
        assert!(
            (pos[2] - (shoulder[2] - 0.30)).abs() < 2e-5,
            "the arm trails under positive pitch: {pos:?}"
        );
    }

    /// The Euler order pin: with pitch AND yaw both at a quarter,
    /// R = R_yaw · R_pitch maps +Y → +X (pitch first sends +Y to +Z,
    /// yaw then sends +Z to +X). The composed local matrix's column
    /// 1 (the +Y basis image) is the answer; a different composition
    /// order (or a transposed core) moves it.
    #[test]
    fn euler_order_yaw_then_pitch_is_pinned() {
        let skeleton = humanoid_skeleton();
        let mut pose = rest_pose();
        pose[0].delta_turns = [16384, 16384, 0]; // pitch quarter + yaw quarter.
        let mats = compose_joint_matrices(&skeleton, &pose);
        let col1 = mats[0][1]; // the +Y basis image.
        assert!(
            (col1[0] - 1.0).abs() < 2e-5 && col1[1].abs() < 2e-5 && col1[2].abs() < 2e-5,
            "R_yaw · R_pitch maps +Y to +X: {col1:?} (a different Euler order \
             or a transposed core moves this)"
        );
    }

    /// sample_pose: t=0 returns keyframe 0's pose exactly; t at a
    /// keyframe returns it exactly; determinism.
    #[test]
    fn sample_pose_keyframe_exact() {
        let clip = walk_clip();
        let pose = sample_pose(&clip, 0.0);
        assert_eq!(pose, clip.keyframes[0].pose, "t=0 returns keyframe 0");
        for (i, kf) in clip.keyframes.iter().enumerate() {
            let pose = sample_pose(&clip, kf.time);
            assert_eq!(pose, kf.pose, "t={} returns keyframe {}", kf.time, i);
        }
        let a = sample_pose(&clip, 0.5);
        let b = sample_pose(&clip, 0.5);
        assert_eq!(a, b, "determinism");
    }

    /// The seam: the loop is CONTINUOUS through the wrap — the left
    /// limit (t → duration⁻, where the kf7→kf0 interpolation is
    /// nearly complete) and the right limit (t → 0⁺) both converge
    /// to keyframe 0's pose, within the interpolation's rounding.
    #[test]
    fn sample_pose_seam_continuity() {
        let clip = walk_clip();
        // ε = 1e-6 s: the residual interpolation left in the wrap is
        // under 0.02 turn units at the table's largest swing (2085),
        // so the rounded sample equals keyframe 0 exactly.
        let before = sample_pose(&clip, 1.1 - 1e-6);
        let after = sample_pose(&clip, 1e-6);
        let kf0 = clip.keyframes[0].pose;
        for j in 0..HUMANOID_JOINTS {
            for k in 0..3 {
                assert!(
                    (before[j].delta_turns[k] - kf0[j].delta_turns[k]).abs() <= 1,
                    "seam-before at joint {j} axis {k}: the left limit converges to keyframe 0"
                );
                assert!(
                    (after[j].delta_turns[k] - kf0[j].delta_turns[k]).abs() <= 1,
                    "seam-after at joint {j} axis {k}"
                );
            }
            for axis in 0..3 {
                assert!(
                    (before[j].delta_pos[axis] - kf0[j].delta_pos[axis]).abs() < 1e-3
                        && (after[j].delta_pos[axis] - kf0[j].delta_pos[axis]).abs() < 1e-3,
                    "seam delta_pos at joint {j} axis {axis}"
                );
            }
        }
    }

    /// Turn interpolation on a synthetic two-keyframe clip: the
    /// midpoint is the rounded midpoint; quarter points; ties round
    /// AWAY from zero; sign crossings go through zero the SHORT way.
    #[test]
    fn turn_interpolation_known_answers() {
        let kf = |time: f32, pitch: i16| {
            let mut pose = rest_pose();
            pose[7].delta_turns = [pitch, 0, 0];
            Keyframe { time, pose }
        };
        let keyframes: &'static [Keyframe] =
            Box::leak([kf(0.0, 0), kf(1.0, 16384)].to_vec().into_boxed_slice());
        let clip = Clip {
            duration: 2.0,
            keyframes,
        };
        // t=0.5: alpha 0.5 → the rounded midpoint 8192.
        assert_eq!(sample_pose(&clip, 0.5)[7].delta_turns[0], 8192);
        // t=0.25: alpha 0.25 → 4096.
        assert_eq!(sample_pose(&clip, 0.25)[7].delta_turns[0], 4096);
        // t=1.5 (the wrap bracket, kf1 → kf0): alpha 0.5 → 8192.
        assert_eq!(sample_pose(&clip, 1.5)[7].delta_turns[0], 8192);

        // Ties away from zero: 0 → 1 over alpha 0.5 lands at 0.5,
        // which rounds to 1.
        let keyframes: &'static [Keyframe] =
            Box::leak([kf(0.0, 0), kf(1.0, 1)].to_vec().into_boxed_slice());
        let clip = Clip {
            duration: 2.0,
            keyframes,
        };
        assert_eq!(sample_pose(&clip, 0.5)[7].delta_turns[0], 1);

        // A sign crossing goes through zero (the SHORT way — the
        // zero-crossing contract's unit form).
        let keyframes: &'static [Keyframe] =
            Box::leak([kf(0.0, -10), kf(1.0, 10)].to_vec().into_boxed_slice());
        let clip = Clip {
            duration: 2.0,
            keyframes,
        };
        assert_eq!(sample_pose(&clip, 0.5)[7].delta_turns[0], 0);
        assert_eq!(sample_pose(&clip, 0.25)[7].delta_turns[0], -5);
    }

    /// THE ZERO-CROSSING PIN: over the whole walk cycle at fine
    /// resolution, every swing's turn stays within its authored
    /// amplitude — a wrapped-value lerp (the unsigned-encoding bug)
    /// sweeps sign crossings the long way around the turn circle
    /// and blows these bounds out by an order of magnitude.
    #[test]
    fn swing_turns_stay_small_across_the_cycle() {
        let clip = walk_clip();
        const ULL_BOUND: i32 = 3605; // 0.055 · 65536 = 3604.5 + rounding slack.
        const UAL_BOUND: i32 = 2950; // 0.045 · 65536 = 2949.1.
        const YAW_BOUND: i32 = 525; // 0.008 · 65536 = 524.3.
        const KNEE_BOUND: i32 = 5900; // 0.09 · 65536 = 5898.2.
        for i in 0..220 {
            let t = i as f32 * 1.1 / 220.0;
            let pose = sample_pose(&clip, t);
            let turn = |j: usize, k: usize| i32::from(pose[j].delta_turns[k]);
            assert!(
                turn(7, 0).abs() <= ULL_BOUND && turn(8, 0).abs() <= ULL_BOUND,
                "t={t}: an upper-leg swing left its amplitude ({} / {}) — \
                 the interpolation is sweeping the long way around",
                turn(7, 0),
                turn(8, 0)
            );
            assert!(
                turn(3, 0).abs() <= UAL_BOUND && turn(4, 0).abs() <= UAL_BOUND,
                "t={t}: an arm swing left its amplitude"
            );
            assert!(
                turn(0, 1).abs() <= YAW_BOUND,
                "t={t}: the pelvis yaw swayed out"
            );
            for knee in [9, 10] {
                let value = turn(knee, 0);
                assert!(
                    (0..=KNEE_BOUND).contains(&value),
                    "t={t}: knee {knee} turn {value} outside [0, {KNEE_BOUND}] — \
                     knees flex positive-only within the amplitude"
                );
            }
        }
    }

    /// The gait's left/right anti-phase and the knee complement: at
    /// every keyframe the left and right swings are exact negatives,
    /// and the right knee at keyframe k is the left knee at k+4
    /// (the half-stride image).
    #[test]
    fn gait_anti_phase_and_knee_complement() {
        let clip = walk_clip();
        let n = clip.keyframes.len();
        for (k, kf) in clip.keyframes.iter().enumerate() {
            let pairs = [(7usize, 8usize), (3usize, 4usize)];
            for (l, r) in pairs {
                assert!(
                    (kf.pose[l].delta_turns[0] + kf.pose[r].delta_turns[0]).abs() <= 1,
                    "keyframe {k}: joints {l}/{r} are not anti-phase"
                );
            }
            let half = clip.keyframes[(k + 4) % n];
            assert_eq!(
                kf.pose[10].delta_turns[0], half.pose[9].delta_turns[0],
                "keyframe {k}: the right knee is not the left knee's half-stride image"
            );
        }
    }

    /// The knee phase: the left knee flexes only on the first
    /// half-stride (keyframes 1-3), the right knee only on the
    /// second (5-7), both zero at the heel strikes (0, 4) — each
    /// knee bends during ITS OWN leg's forward swing.
    #[test]
    fn knee_bends_only_during_its_legs_swing() {
        let clip = walk_clip();
        for (k, kf) in clip.keyframes.iter().enumerate() {
            let left = kf.pose[9].delta_turns[0];
            let right = kf.pose[10].delta_turns[0];
            match k {
                1..=3 => {
                    assert!(left > 0, "keyframe {k}: the left knee flexes mid-stride");
                    assert_eq!(
                        right, 0,
                        "keyframe {k}: the right knee is straight in stance"
                    );
                }
                5..=7 => {
                    assert!(right > 0, "keyframe {k}: the right knee flexes mid-stride");
                    assert_eq!(left, 0, "keyframe {k}: the left knee is straight in stance");
                }
                _ => {
                    assert_eq!(left, 0, "keyframe {k}: the planted knee is straight");
                    assert_eq!(right, 0, "keyframe {k}: the planted knee is straight");
                }
            }
        }
    }

    /// compose_joint_matrices at rest: every joint's world matrix is
    /// a PURE TRANSLATION to its pivot — the rotation columns are
    /// bit-exact identity images, and the translation column equals
    /// the chain-composed pivot.
    #[test]
    fn compose_rest_is_pure_translation_to_the_pivots() {
        let skeleton = humanoid_skeleton();
        let mats = compose_joint_matrices(&skeleton, &rest_pose());
        for (idx, mat) in mats.iter().enumerate() {
            assert_eq!(
                mat[0].map(f32::to_bits),
                [1.0f32, 0.0, 0.0, 0.0].map(f32::to_bits),
                "joint {idx}: column 0 is the +X image, bit-exact"
            );
            assert_eq!(
                mat[1].map(f32::to_bits),
                [0.0f32, 1.0, 0.0, 0.0].map(f32::to_bits),
                "joint {idx}: column 1 is the +Y image, bit-exact"
            );
            assert_eq!(
                mat[2].map(f32::to_bits),
                [0.0f32, 0.0, 1.0, 0.0].map(f32::to_bits),
                "joint {idx}: column 2 is the +Z image, bit-exact"
            );
            assert_eq!(mat[3][3], 1.0);
            let pivot = skeleton[idx].pivot;
            for axis in 0..3 {
                assert!(
                    (mat[3][axis] - pivot[axis]).abs() < 1e-5,
                    "joint {idx}: the rest translation is the pivot"
                );
            }
        }
        // Two chain-derived pivots (the compose order's arithmetic):
        // the torso's world pivot is the pelvis pivot + the local
        // waist offset; the head's adds the neck offset.
        assert!((mats[1][3][1] - 1.06).abs() < 1e-5, "torso pivot y");
        assert!((mats[2][3][1] - 1.60).abs() < 1e-5, "head pivot y");
    }

    /// THE CHAIN-ORDER PIN: parent_first composition. A quarter
    /// pitch at the TORSO rotates the head's offset from the torso
    /// (0, 0.54, 0) to (0, 0, 0.54) — the head's world translation
    /// becomes the torso pivot plus that rotated offset. A
    /// local-first multiply order instead rotates the torso's own
    /// offset too and moves the head somewhere else entirely.
    #[test]
    fn chain_composes_parent_then_local() {
        let skeleton = humanoid_skeleton();
        let mut pose = rest_pose();
        pose[1].delta_turns = [16384, 0, 0]; // quarter pitch at the torso.
        let mats = compose_joint_matrices(&skeleton, &pose);
        let head = mats[2][3];
        assert!(
            head[0].abs() < 2e-5 && (head[1] - 1.06).abs() < 2e-5 && (head[2] - 0.54).abs() < 2e-5,
            "the head after a torso pitch: {head:?} — expected the torso pivot \
             (0, 1.06, 0) plus the rotated (0, 0.54, 0) offset, i.e. (0, 1.06, 0.54)"
        );

        // A quarter yaw at the pelvis swings the left shoulder
        // (pivot offset (-0.21, 0.60, 0) from the pelvis) to
        // (0, 0.60, +0.21): +X maps to −Z, so −X maps to +Z.
        let mut pose = rest_pose();
        pose[0].delta_turns = [0, 16384, 0];
        let mats = compose_joint_matrices(&skeleton, &pose);
        let shoulder = mats[3][3];
        assert!(
            shoulder[0].abs() < 2e-5
                && (shoulder[1] - 1.58).abs() < 2e-5
                && (shoulder[2] - 0.21).abs() < 2e-5,
            "the left shoulder after a pelvis yaw: {shoulder:?}"
        );
    }

    /// THE REST-IDENTITY PIN (the pixel-perfect promise): at the
    /// zero pose every skin matrix is BIT-EXACT identity. This is
    /// the property that lets the skinned path render the authored
    /// mesh byte-identically at rest — the Stage-2 gates lean on it.
    #[test]
    fn skin_matrices_rest_identity_is_bit_exact() {
        let skeleton = humanoid_skeleton();
        let joint_worlds = compose_joint_matrices(&skeleton, &rest_pose());
        let skin_mats = skin_matrices(&joint_worlds, &skeleton);
        let identity = [
            [1.0f32, 0.0, 0.0, 0.0],
            [0.0f32, 1.0, 0.0, 0.0],
            [0.0f32, 0.0, 1.0, 0.0],
            [0.0f32, 0.0, 0.0, 1.0],
        ];
        for (idx, mat) in skin_mats.iter().enumerate() {
            for col in 0..4 {
                assert_eq!(
                    mat[col].map(f32::to_bits),
                    identity[col].map(f32::to_bits),
                    "skin matrix {idx} column {col} is not bit-exact identity — \
                     the rest-pose pixel-identity promise is broken"
                );
            }
        }
    }

    /// The blended form of the equation: a vertex split between two
    /// differently-rotated joints lands at the weighted sum of the
    /// two rigid results, evaluated in the same fixed term order.
    #[test]
    fn skin_vertex_blended_is_the_weighted_sum() {
        let skeleton = humanoid_skeleton();
        let mut pose = rest_pose();
        pose[7].delta_turns = [16384, 0, 0]; // left hip quarter pitch.
        pose[8].delta_turns = [-8192, 0, 0]; // right hip eighth back.
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let skin_mats = skin_matrices(&joint_worlds, &skeleton);

        let pos = [-0.02, 0.60, 0.03]; // between the two hips, below them
        let rigid = |joint: u32| SkinnedVertex {
            pos,
            normal: [0.0, -1.0, 0.0],
            uv: [0.0; 2],
            joints: [joint, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        let (p7, _) = skin_vertex(&rigid(7), &skin_mats);
        let (p8, _) = skin_vertex(&rigid(8), &skin_mats);
        let blended = SkinnedVertex {
            pos,
            normal: [0.0, -1.0, 0.0],
            uv: [0.0; 2],
            joints: [7, 8, 0, 0],
            weights: [0.25, 0.75, 0.0, 0.0],
        };
        let (pb, _) = skin_vertex(&blended, &skin_mats);
        for axis in 0..3 {
            let expected = 0.25 * p7[axis] + 0.75 * p8[axis];
            assert!(
                (pb[axis] - expected).abs() < 1e-6,
                "axis {axis}: the blend {pb:?} is the weighted sum of {p7:?} and {p8:?}"
            );
        }
        // And it lies between the two rigid results on each axis
        // where they differ.
        assert!(pb[2] >= p7[2].min(p8[2]) - 1e-6 && pb[2] <= p7[2].max(p8[2]) + 1e-6);
    }

    /// posed_box_bounds: at rest, every box's bounds equal its
    /// authored HUMANOID_BOXES entry; under a quarter pelvis yaw the
    /// torso box's world bounds SWAP its x and z extents (the yaw
    /// exchanges the horizontal axes; the box's center sits on the
    /// rotation axis).
    #[test]
    fn posed_box_bounds_rest_and_under_rotation() {
        let skeleton = humanoid_skeleton();
        let joint_worlds = compose_joint_matrices(&skeleton, &rest_pose());
        let skin_mats = skin_matrices(&joint_worlds, &skeleton);
        for (box_idx, &(expected_lo, expected_hi)) in HUMANOID_BOXES.iter().enumerate() {
            let (lo, hi) = posed_box_bounds(&skin_mats, box_idx);
            for axis in 0..3 {
                assert!(
                    (lo[axis] - expected_lo[axis]).abs() < 1e-5
                        && (hi[axis] - expected_hi[axis]).abs() < 1e-5,
                    "box {box_idx} axis {axis}: rest bounds are the authored box"
                );
            }
        }

        let mut pose = rest_pose();
        pose[0].delta_turns = [0, 16384, 0];
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let skin_mats = skin_matrices(&joint_worlds, &skeleton);
        // The torso box (emission index 1): x ∈ [−0.21, 0.21],
        // z ∈ [−0.12, 0.12] at rest. After the quarter yaw the
        // horizontal extents exchange: x ∈ [−0.12, 0.12],
        // z ∈ [−0.21, 0.21]; y is untouched (the yaw axis).
        let (lo, hi) = posed_box_bounds(&skin_mats, 1);
        assert!(
            (lo[0] + 0.12).abs() < 2e-5 && (hi[0] - 0.12).abs() < 2e-5,
            "x extent swaps"
        );
        assert!(
            (lo[2] + 0.21).abs() < 2e-5 && (hi[2] - 0.21).abs() < 2e-5,
            "z extent swaps"
        );
        assert!(
            (lo[1] - 1.06).abs() < 2e-5 && (hi[1] - 1.60).abs() < 2e-5,
            "y untouched"
        );
    }

    /// rest_pose_block: equals skin_matrices at the zero pose,
    /// bit-exact.
    #[test]
    fn rest_pose_block_equals_skin_matrices() {
        let skeleton = humanoid_skeleton();
        let joint_worlds = compose_joint_matrices(&skeleton, &rest_pose());
        let expected = skin_matrices(&joint_worlds, &skeleton);
        assert_eq!(rest_pose_block(), expected);
    }

    /// walk_clip structure: duration 1.1; 8 keyframes; strictly
    /// increasing times, first at 0.0, last < duration; the pelvis
    /// bobs at double frequency (low at the heel strikes, high at
    /// mid-stance); the head is steady; and the amplitude formulas
    /// are pinned at their 45° marks (the table-is-the-formula
    /// check — a mis-slotted keyframe fails here).
    #[test]
    fn walk_clip_structure() {
        let clip = walk_clip();
        assert!((clip.duration - 1.1).abs() < 1e-5);
        assert_eq!(clip.keyframes.len(), 8);
        for i in 1..clip.keyframes.len() {
            assert!(
                clip.keyframes[i].time > clip.keyframes[i - 1].time,
                "times strictly increasing"
            );
        }
        assert_eq!(clip.keyframes[0].time, 0.0);
        assert!(clip.keyframes[7].time < clip.duration);

        // The pelvis vault: low (−0.02) at the heel strikes (φ = 0,
        // 180), high (+0.02) at mid-stance (φ = 90, 270).
        for k in [0, 4] {
            assert_eq!(
                clip.keyframes[k].pose[0].delta_pos[1], -0.02,
                "heel-strike low"
            );
        }
        for k in [2, 6] {
            assert_eq!(
                clip.keyframes[k].pose[0].delta_pos[1], 0.02,
                "mid-stance high"
            );
        }
        for k in [1, 3, 5, 7] {
            assert_eq!(
                clip.keyframes[k].pose[0].delta_pos[1], 0.0,
                "the zero crossing"
            );
        }

        // The head is steady everywhere.
        for kf in clip.keyframes.iter() {
            assert_eq!(
                kf.pose[2],
                JointPose {
                    delta_pos: [0.0; 3],
                    delta_turns: [0; 3]
                }
            );
        }

        // The formula pins at the 45° marks: the left leg's swing is
        // −0.055·sin(45°), the left knee +0.09·sin(45°), the pelvis
        // yaw +0.008·sin(45°), the left arm +0.045·sin(45°).
        let sin45 = core::f32::consts::FRAC_1_SQRT_2;
        let kf = &clip.keyframes[1];
        let expect = |fraction: f32| (fraction * 65536.0) as i16;
        assert_eq!(
            kf.pose[7].delta_turns[0],
            expect(-0.055 * sin45),
            "the 45° leg swing"
        );
        assert_eq!(
            kf.pose[9].delta_turns[0],
            expect(0.09 * sin45),
            "the 45° knee"
        );
        assert_eq!(
            kf.pose[0].delta_turns[1],
            expect(0.008 * sin45),
            "the 45° pelvis yaw"
        );
        assert_eq!(
            kf.pose[3].delta_turns[0],
            expect(0.045 * sin45),
            "the 45° arm swing"
        );
        // The mid-swing extremes at φ = 90°.
        let kf = &clip.keyframes[2];
        assert_eq!(
            kf.pose[7].delta_turns[0],
            expect(-0.055),
            "the max forward reach"
        );
        assert_eq!(
            kf.pose[9].delta_turns[0],
            expect(0.09),
            "the max knee flexion"
        );
        // The heel strikes are the all-zero limb pose.
        for k in [0, 4] {
            let kf = &clip.keyframes[k];
            for j in [3, 4, 7, 8, 9, 10] {
                assert_eq!(
                    kf.pose[j].delta_turns,
                    [0, 0, 0],
                    "heel strike {k} joint {j}"
                );
            }
        }
    }

    /// pose_sequence_digest: deterministic across calls.
    #[test]
    fn pose_sequence_digest_determinism() {
        let clip = walk_clip();
        let skeleton = humanoid_skeleton();
        assert_eq!(
            pose_sequence_digest(&clip, &skeleton, 64),
            pose_sequence_digest(&clip, &skeleton, 64)
        );
    }

    /// THE CROSS-PLATFORM POSE DIGEST PIN: the walk clip's sampled
    /// pose sequence hashes to this MEASURED u64 (the placeholder-pin
    /// discipline: the first run measures, the pin follows, and any
    /// later movement is an investigation — a math, table, or
    /// packing change — never a silent repin). Linux and Windows CI
    /// asserting the same value IS the cross-platform determinism
    /// proof (the FNV discipline applied to the pose bytes).
    #[test]
    fn pose_sequence_digest_pin() {
        let clip = walk_clip();
        let skeleton = humanoid_skeleton();
        let digest = pose_sequence_digest(&clip, &skeleton, 64);
        // MEASURED at this tree (the convention repair + the signed
        // turn model + the corrected gait table — the placeholder
        // pin's first real value; the delegate build's digest was
        // never pinned, so this is the pin's origin, not a repin).
        const PINNED_FNV: u64 = 2_694_074_108_253_436_588;
        assert_eq!(
            digest, PINNED_FNV,
            "the walk clip's pose digest moved: a math, table, or packing change — \
             an investigation, never a silent repin"
        );
    }
}
