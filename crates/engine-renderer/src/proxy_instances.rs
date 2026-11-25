//! The instanced-draw deterministic core (the Cycle 6-E scaffold —
//! see `docs/CYCLE6-6E-BRIEF.md`): the per-instance transform math,
//! the byte-exact instance-buffer packing, and the capacity
//! discipline for proxy instancing — ONE draw call for N proxies
//! instead of N vertex rebuilds (the O(entities) CPU ceiling from
//! the original renderer audit).
//!
//! THE 6-A PATTERN, APPLIED AGAIN: everything deterministic and pure
//! lives here, fully tested — the transform model, the packing
//! format, and the overflow refusal. The implementer wires the GPU
//! side (the instance-rate vertex input, the instance buffer, the
//! single indexed/instanced draw) and proves output equivalence
//! against the un-instanced path through the capture harness.
//!
//! DETERMINISM CONTRACT: the packed instance bytes are byte-identical
//! on every platform. This is why orientation is a `u16` TURN FRACTION
//! (1/65536 of a full rotation) through [`sin_cos_turn`] — a
//! fixed-order fixed-constant Taylor series over pure IEEE f32
//! arithmetic — and NOT an `f32` radian through `f32::sin`/`cos`:
//! the system libm's trig bits vary across platforms (the repo's
//! standing libm-free discipline), which would make the packed bytes
//! platform-dependent. All packing uses `f32::to_bits` — the exact
//! IEEE bit patterns, little-endian, in a FIXED field order.
//!
//! CAPACITY DISCIPLINE (the D2 ratified policy applied to instances):
//! no partial instance buffers — an over-capacity pack is a TYPED
//! refusal produced BEFORE any byte exists, never a truncated write.

/// The per-draw instance bound. The sustained-load matrix's
/// demonstrated entity range is 1,000–5,000; 4,096 is the initial
/// per-draw bound (one power-of-two above the demonstrated midpoint).
/// Revisitable only with rig evidence, never silently.
pub const MAX_INSTANCES: usize = 4096;

/// One instance's packed size: 24 little-endian f32s — four matrix
/// columns (64 B), one tint vec4 (16 B), and one params vec4 (16 B:
/// x = the POSE BLOCK INDEX — the Era-2 skinning claim on the
/// former forward-compatibility slot, a versioned layout change
/// documented here and in the packed-bytes pin's history; y/z/w stay
/// reserved zeros, claimed only by another versioned change).
pub const INSTANCE_STRIDE: usize = 96;

/// One proxy's placement: a world position, a yaw as a turn fraction
/// (`turn` = units of 1/65536 of a full rotation; 0 faces +X, 16384
/// faces −Z per the pinned convention below), and a uniform scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstanceTransform {
    /// The instance origin in world space (the proxy mesh's local
    /// origin — the feet for the humanoid proxy, the ground-center
    /// for the vehicle proxy).
    pub pos: [f32; 3],
    /// The yaw as a turn fraction (see the struct docs).
    pub turn: u16,
    /// The uniform scale (must be finite and strictly positive — a
    /// degenerate zero-scale instance is a validation refusal, not a
    /// silently invisible proxy).
    pub scale: f32,
}

/// One drawable proxy instance: the transform plus a per-instance
/// albedo tint (multiplied into the sampled albedo BEFORE the
/// lighting equation — the composition that keeps the 6-B CPU
/// oracle [`crate::lit_scene::lit_surface_color`] directly
/// applicable with `albedo * tint` as its albedo argument).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProxyInstance {
    pub transform: InstanceTransform,
    /// The per-instance albedo tint, each channel in [0, 1].
    pub tint: [f32; 3],
    /// The per-instance ALPHA — the tint vec4's `.a` slot, formerly
    /// the packing's hard-coded 1.0. A VERSIONED-LAYOUT claim of the
    /// Era-2 package C (rain) stage, documented here and in the
    /// packed-bytes pins' history: 1.0 for every OPAQUE kind (the
    /// existing npc/vehicle populations' bytes UNCHANGED — the
    /// re-run byte gates re-verify), the translucent rain kind's
    /// per-streak coverage otherwise. Validated in [0, 1] (never
    /// negative, never non-finite).
    pub alpha: f32,
    /// The instance's POSE BLOCK INDEX into the frame's pose buffer
    /// (the Era-2 skinning path): npc instance i's skin matrices
    /// live at `pose_index * POSE_STRIDE`. Sequential for the
    /// shipped population (one pose block per NPC); the INDEX form
    /// is what makes pose-block DEDUP a later non-breaking change
    /// (several instances sharing one evaluated pose). Vehicles
    /// carry 0 — the plain vehicle pipeline never reads the slot.
    /// Validated at the renderer's populated submit against the
    /// supplied pose bytes (a typed refusal, never a silent clamp).
    pub pose_index: u32,
}

/// The typed pack refusal (the D2 no-partial-overflow discipline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstancePackError {
    /// More instances than the capacity allows. No bytes were
    /// produced.
    Capacity { capacity: usize, requested: usize },
    /// An instance failed validation at `index`. No bytes were
    /// produced. `field` names the refusing field.
    Invalid { index: usize, field: &'static str },
}

/// The fixed half-turn constant (the nearest f32 to π/2 — a fixed
/// bit pattern, identical on every platform).
const FRAC_PI_2: f32 = core::f32::consts::FRAC_PI_2;

/// The deterministic sine/cosine of a turn fraction. Quadrant
/// reduction plus fixed-order fixed-constant Taylor series (Horner
/// evaluation, pure IEEE f32 arithmetic — no libm, no platform
/// variation). Documented precision: absolute error ≤ 2e-6 over the
/// full turn — ample for proxy orientation, where the contract is
/// BIT-IDENTITY across platforms, not libm-grade accuracy.
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

/// The instance transform as four COLUMN-MAJOR matrix columns (the
/// WGSL `mat4x4` constructor's order — the same convention the
/// shader's `mat4x4<f32>(i_col0, i_col1, i_col2, i_col3)` consumes).
/// The composition is T·R·S: scale first, then the yaw about +Y,
/// then the translation. THE PINNED CONVENTION: at a quarter turn
/// (turn = 16384), +X maps to −Z (right-handed, Y-up; the known-
/// answer test pins it).
pub fn instance_columns(transform: &InstanceTransform) -> [[f32; 4]; 4] {
    // sin_cos_turn returns (sin, cos) — mind the binding order.
    let (s, c) = sin_cos_turn(transform.turn);
    let k = transform.scale;
    [
        [c * k, 0.0, -s * k, 0.0],
        [0.0, k, 0.0, 0.0],
        [s * k, 0.0, c * k, 0.0],
        [transform.pos[0], transform.pos[1], transform.pos[2], 1.0],
    ]
}

impl ProxyInstance {
    /// The R4-style input contract: finite position/scale, strictly
    /// positive scale, tint channels in [0, 1], alpha finite in
    /// [0, 1]. Returns the refusing field name, or `None` when
    /// valid.
    pub fn validate(&self) -> Option<&'static str> {
        let t = &self.transform;
        if !t.pos.iter().all(|v| v.is_finite()) {
            return Some("pos");
        }
        if !t.scale.is_finite() || t.scale <= 0.0 {
            return Some("scale");
        }
        if !self
            .tint
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        {
            return Some("tint");
        }
        if !self.alpha.is_finite() || !(0.0..=1.0).contains(&self.alpha) {
            return Some("alpha");
        }
        None
    }
}

/// Appends one instance's bytes in the fixed field order (the four
/// matrix columns, tint rgb + the instance alpha, then the params
/// vec4: the pose block index as an f32 bit pattern followed by
/// three reserved zero f32s — the Era-2 claim on the former
/// all-zero slot, a versioned layout change never a silent reuse).
/// The single packing implementation both the allocating and the
/// retained forms share — the byte layout exists exactly once.
fn append_instance_bytes(bytes: &mut Vec<u8>, instance: &ProxyInstance) {
    for column in instance_columns(&instance.transform) {
        for value in column {
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    }
    for value in [
        instance.tint[0],
        instance.tint[1],
        instance.tint[2],
        instance.alpha,
    ] {
        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    // The params vec4: x = the pose index (exact in f32 up to 2^24
    // blocks — far beyond MAX_INSTANCES), y/z/w reserved zeros.
    let pose_index = instance.pose_index as f32;
    bytes.extend_from_slice(&pose_index.to_bits().to_le_bytes());
    bytes.extend_from_slice(&[0u8; 12]);
}

/// Packs `instances` into the instance-buffer bytes (little-endian
/// f32 bit patterns, the fixed field order: column 0, 1, 2, 3, tint
/// rgb + the instance alpha, then the params vec4 — pose index +
/// three reserved zero f32s). Byte-identical on
/// every platform for the same input — the packing is pure IEEE
/// arithmetic and `to_bits`. Validation and capacity refusal happen
/// BEFORE any byte exists (no partial buffers, ever).
pub fn pack_instances(instances: &[ProxyInstance]) -> Result<Vec<u8>, InstancePackError> {
    pack_instances_bounded(instances, MAX_INSTANCES)
}

/// The bounded form: the caller-supplied capacity is the draw's
/// instance-buffer capacity (a typed refusal above it, never a
/// truncation).
pub fn pack_instances_bounded(
    instances: &[ProxyInstance],
    capacity: usize,
) -> Result<Vec<u8>, InstancePackError> {
    if instances.len() > capacity {
        return Err(InstancePackError::Capacity {
            capacity,
            requested: instances.len(),
        });
    }
    for (index, instance) in instances.iter().enumerate() {
        if let Some(field) = instance.validate() {
            return Err(InstancePackError::Invalid { index, field });
        }
    }
    let mut bytes = Vec::with_capacity(instances.len() * INSTANCE_STRIDE);
    for instance in instances {
        append_instance_bytes(&mut bytes, instance);
    }
    Ok(bytes)
}

/// Repacks the three proxy populations — `npc` then `vehicle` then
/// `rain`, the FIXED per-frame buffer order (the Era-2 package C
/// rain kind appends LAST, after vehicle, never between) — INTO
/// `buffer` in place (the F-05 retained-buffer discipline applied to
/// the instance bytes: the Vec is cleared and refilled, so its
/// capacity from the first populated frames is reused every frame —
/// no per-frame allocation or growth; the byte content is identical
/// to the allocating form concatenated in the same order). All three
/// populations are validated BEFORE the clear: a typed refusal
/// leaves the previous frame's bytes intact — an atomic repack,
/// never a partial buffer.
///
/// The bound is [`MAX_INSTANCES`] TOTAL across all three kinds (the
/// sustained-load entity range the constant documents: one frame's
/// whole population, not per kind), because one slot region of the
/// GPU instance buffer holds exactly that many packed instances.
pub fn repack_populations(
    buffer: &mut Vec<u8>,
    npc: &[ProxyInstance],
    vehicle: &[ProxyInstance],
    rain: &[ProxyInstance],
) -> Result<(), InstancePackError> {
    let total = npc.len() + vehicle.len() + rain.len();
    if total > MAX_INSTANCES {
        return Err(InstancePackError::Capacity {
            capacity: MAX_INSTANCES,
            requested: total,
        });
    }
    for (index, instance) in npc.iter().chain(vehicle).chain(rain).enumerate() {
        if let Some(field) = instance.validate() {
            return Err(InstancePackError::Invalid { index, field });
        }
    }
    buffer.clear();
    buffer.reserve(total * INSTANCE_STRIDE);
    for instance in npc.iter().chain(vehicle).chain(rain) {
        append_instance_bytes(buffer, instance);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The known answers at the cardinal turns. Tolerance 1e-5
    /// covers the documented Taylor truncation (≤ 2e-6) with margin.
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

    /// The pinned matrix convention: identity at zero turn/scale 1;
    /// the quarter turn maps +X to −Z (the documented right-handed
    /// Y-up convention); scale multiplies; translation adds.
    #[test]
    fn instance_columns_identity_quarter_scale_translate() {
        let identity = InstanceTransform {
            pos: [0.0, 0.0, 0.0],
            turn: 0,
            scale: 1.0,
        };
        let cols = instance_columns(&identity);
        assert_eq!(cols[0], [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(cols[1], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(cols[2], [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(cols[3], [0.0, 0.0, 0.0, 1.0]);

        // The quarter turn: +X -> -Z, +Z -> +X (columns are the
        // basis images; tolerance covers the Taylor truncation).
        let quarter = InstanceTransform {
            pos: [0.0; 3],
            turn: 16384,
            scale: 1.0,
        };
        let cols = instance_columns(&quarter);
        assert!(cols[0][0].abs() < 1e-5, "col0.x ~ 0");
        assert!(
            (cols[0][2] + 1.0).abs() < 1e-5,
            "+X maps to -Z: {:?}",
            cols[0]
        );
        assert!(
            (cols[2][0] - 1.0).abs() < 1e-5,
            "+Z maps to +X: {:?}",
            cols[2]
        );

        // Scale doubles the basis columns; translation lands in
        // column 3 untouched.
        let scaled = InstanceTransform {
            pos: [10.0, 20.0, 30.0],
            turn: 0,
            scale: 2.0,
        };
        let cols = instance_columns(&scaled);
        assert_eq!(cols[0], [2.0, 0.0, 0.0, 0.0]);
        assert_eq!(cols[3], [10.0, 20.0, 30.0, 1.0]);
    }

    /// Determinism and order sensitivity: the same list packs
    /// byte-identically; a reordered list packs differently (the
    /// buffer order IS the draw order — caller-owned, documented).
    #[test]
    fn pack_instances_deterministic_and_order_sensitive() {
        let a = ProxyInstance {
            transform: InstanceTransform {
                pos: [1.0, 0.0, 2.0],
                turn: 1000,
                scale: 1.0,
            },
            tint: [0.5, 0.5, 0.5],
            alpha: 1.0,
            pose_index: 7,
        };
        let b = ProxyInstance {
            transform: InstanceTransform {
                pos: [-3.0, 0.0, 4.0],
                turn: 40000,
                scale: 2.0,
            },
            tint: [1.0, 0.0, 0.0],
            alpha: 1.0,
            pose_index: 0,
        };
        let first = pack_instances(&[a, b]).unwrap();
        let second = pack_instances(&[a, b]).unwrap();
        assert_eq!(first, second, "byte-identical across calls");
        assert_eq!(first.len(), 2 * INSTANCE_STRIDE);
        let reordered = pack_instances(&[b, a]).unwrap();
        assert_ne!(first, reordered, "the instance order is significant");
    }

    /// The capacity discipline: at-capacity packs; over-capacity is
    /// the TYPED refusal (never truncation); an invalid instance is
    /// the typed per-field refusal. Both refusals produce zero bytes.
    #[test]
    fn pack_instances_bounded_typed_refusals() {
        let valid = ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0; 3],
                turn: 0,
                scale: 1.0,
            },
            tint: [1.0; 3],
            alpha: 1.0,
            pose_index: 0,
        };
        let at_capacity: Vec<_> = vec![valid; 8];
        assert!(pack_instances_bounded(&at_capacity, 8).is_ok());
        let over: Vec<_> = vec![valid; 9];
        assert_eq!(
            pack_instances_bounded(&over, 8),
            Err(InstancePackError::Capacity {
                capacity: 8,
                requested: 9
            })
        );
        let mut invalid = valid;
        invalid.tint = [f32::NAN, 0.0, 0.0];
        assert_eq!(
            pack_instances(&[valid, invalid]),
            Err(InstancePackError::Invalid {
                index: 1,
                field: "tint"
            })
        );
        let mut zero_scale = valid;
        zero_scale.transform.scale = 0.0;
        assert_eq!(
            pack_instances(&[zero_scale]),
            Err(InstancePackError::Invalid {
                index: 0,
                field: "scale"
            })
        );
        // The Era-2 package C alpha claim: the per-instance alpha is
        // validated like the tint channels — outside [0, 1] and
        // non-finite are typed refusals, never a silent clamp.
        let mut alpha_high = valid;
        alpha_high.alpha = 1.5;
        assert_eq!(
            pack_instances(&[alpha_high]),
            Err(InstancePackError::Invalid {
                index: 0,
                field: "alpha"
            })
        );
        let mut alpha_nan = valid;
        alpha_nan.alpha = f32::NAN;
        assert_eq!(
            pack_instances(&[alpha_nan]),
            Err(InstancePackError::Invalid {
                index: 0,
                field: "alpha"
            })
        );
    }

    /// The alpha slot's own pin (the Era-2 package C claim on the
    /// tint vec4's `.a`): a NON-1.0 alpha lands in the packed bytes
    /// at the former hard-coded-1.0 offset (76..80 — the tint vec4's
    /// tail), two instances differing ONLY in alpha pack
    /// differently, and the opaque 1.0 default reproduces the
    /// former bytes exactly (the existing populations' pin).
    #[test]
    fn alpha_packs_into_the_tint_slot() {
        let instance = |alpha: f32| ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0; 3],
                turn: 0,
                scale: 1.0,
            },
            tint: [1.0; 3],
            alpha,
            pose_index: 0,
        };
        let bytes = pack_instances(&[instance(0.5)]).unwrap();
        let packed_alpha = f32::from_le_bytes(bytes[76..80].try_into().unwrap());
        assert_eq!(packed_alpha, 0.5, "the alpha rides the tint vec4's .a");
        assert_eq!(
            bytes[80..INSTANCE_STRIDE].len(),
            16,
            "the params vec4 tail is untouched by the claim"
        );
        assert_ne!(
            pack_instances(&[instance(0.5)]).unwrap(),
            pack_instances(&[instance(0.25)]).unwrap(),
            "the alpha is significant in the bytes"
        );
        // The opaque default: alpha 1.0 reproduces the former
        // hard-coded slot bit-for-bit.
        assert_eq!(
            f32::from_le_bytes(
                pack_instances(&[instance(1.0)]).unwrap()[76..80]
                    .try_into()
                    .unwrap()
            ),
            1.0
        );
    }

    /// THE CROSS-PLATFORM BYTE PIN: a fixed instance list packs to a
    /// byte sequence whose FNV-1a hash is this pinned u64 (measured
    /// actual; Linux/macOS CI asserting the same value IS the
    /// cross-platform determinism proof — the fingerprint-pin
    /// discipline applied to the instance buffer). Spot-bytes pin
    /// the field layout for diagnosability.
    #[test]
    fn packed_bytes_fnv_pin() {
        let instances = [
            ProxyInstance {
                transform: InstanceTransform {
                    pos: [12.5, 0.0, -7.25],
                    turn: 16384,
                    scale: 1.0,
                },
                tint: [0.9, 0.8, 0.7],
                alpha: 1.0,
                pose_index: 3,
            },
            ProxyInstance {
                transform: InstanceTransform {
                    pos: [-1.0, 0.0, 3.0],
                    turn: 49152,
                    scale: 2.0,
                },
                tint: [0.1, 0.2, 0.3],
                alpha: 1.0,
                pose_index: 1,
            },
            ProxyInstance {
                transform: InstanceTransform {
                    pos: [0.0, 0.0, 0.0],
                    turn: 0,
                    scale: 0.5,
                },
                tint: [1.0, 1.0, 1.0],
                alpha: 1.0,
                pose_index: 0,
            },
        ];
        let bytes = pack_instances(&instances).unwrap();
        // FNV-1a over the packed bytes.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in &bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(
            hash, PINNED_FNV,
            "the packed-bytes hash moved: a packing or math change — an \
             investigation, never a silent repin"
        );
        // The layout spot-check: instance 0's column 3 (translation)
        // sits at offset 48..60; the tint alpha is 1.0 at offset
        // 76..80 (tint rgb + alpha occupy 64..80); the params vec4's
        // x is the pose index (3.0f for this fixture) at 80..84 and
        // the remaining 12 bytes stay reserved zeros.
        let pos_x = f32::from_le_bytes(bytes[48..52].try_into().unwrap());
        assert_eq!(pos_x, 12.5);
        let alpha = f32::from_le_bytes(bytes[76..80].try_into().unwrap());
        assert_eq!(alpha, 1.0);
        let pose_index = f32::from_le_bytes(bytes[80..84].try_into().unwrap());
        assert_eq!(pose_index, 3.0, "the params vec4's x is the pose index");
        assert!(
            bytes[84..96].iter().all(|&b| b == 0),
            "the params vec4's y/z/w stay reserved zeros"
        );
    }

    /// The measured-actual FNV-1a of the pinned instance list above.
    /// History: the original placeholder-pin run caught the swapped
    /// sin/cos binding and the quadrant-1 sign bug before any of it
    /// landed; the value 14524014449239426066 carried the all-zero
    /// reserved slot. REPINNED in the Era-2 skinning package (the
    /// documented versioned claim: the params vec4's x now carries
    /// the pose block index — 3/1/0 for this fixture — and the pin
    /// fired on the change exactly as designed, the cause traced to
    /// the claim, not an accident). UNCHANGED through the Era-2
    /// package C (rain) alpha claim: this fixture's instances carry
    /// `alpha: 1.0`, which packs to the identical bits the former
    /// hard-coded 1.0 produced — the pin re-ran green on the claim's
    /// first test, the opaque populations' byte-compatibility proof.
    const PINNED_FNV: u64 = 9913137345556047595;

    /// Three small fixture populations (varied turns, scales, tints
    /// — enough shape to make an ordering or field bug visible in
    /// the bytes). The rain population carries NON-1.0 alphas (the
    /// translucent kind's own claim on the tint vec4's .a).
    fn fixture_populations() -> (Vec<ProxyInstance>, Vec<ProxyInstance>, Vec<ProxyInstance>) {
        let instance =
            |pos: [f32; 3], turn: u16, scale: f32, tint: [f32; 3], alpha: f32, pose_index: u32| {
                ProxyInstance {
                    transform: InstanceTransform { pos, turn, scale },
                    tint,
                    alpha,
                    pose_index,
                }
            };
        (
            vec![
                instance([1.0, 0.0, -2.0], 0, 1.0, [0.9, 0.9, 0.9], 1.0, 0),
                instance([-3.0, 0.0, 4.0], 16384, 1.1, [0.8, 0.6, 0.5], 1.0, 1),
            ],
            vec![
                instance([7.0, 0.0, 1.0], 40000, 1.0, [0.2, 0.4, 0.9], 1.0, 0),
                instance([-8.0, 0.0, -1.0], 8192, 0.9, [0.9, 0.2, 0.2], 1.0, 0),
            ],
            vec![
                instance([2.0, 6.0, 0.0], 0, 1.0, [0.6, 0.7, 0.8], 0.5, 0),
                instance([-4.0, 5.0, 3.0], 16384, 1.0, [0.5, 0.6, 0.75], 0.35, 0),
                instance([0.0, 7.0, -3.0], 32768, 1.0, [0.55, 0.65, 0.85], 0.65, 0),
            ],
        )
    }

    /// The retained repack produces byte-identical content to the
    /// allocating form concatenated in the fixed per-frame order (npc
    /// first, vehicle after, rain LAST — the Era-2 package C fixed
    /// kind order), at the pinned stride math — the F-05 reuse
    /// changes WHERE the bytes live, never WHAT they are.
    #[test]
    fn repack_matches_allocating_form_in_kind_order() {
        let (npc, vehicle, rain) = fixture_populations();
        let mut buffer = Vec::new();
        repack_populations(&mut buffer, &npc, &vehicle, &rain).unwrap();
        let mut expected = pack_instances(&npc).unwrap();
        expected.extend_from_slice(&pack_instances(&vehicle).unwrap());
        expected.extend_from_slice(&pack_instances(&rain).unwrap());
        assert_eq!(buffer, expected);
        assert_eq!(
            buffer.len(),
            (npc.len() + vehicle.len() + rain.len()) * INSTANCE_STRIDE
        );
    }

    /// The repack refusal is ATOMIC: an over-capacity or invalid
    /// input leaves the previous frame's bytes untouched (a failed
    /// frame never corrupts the buffer content; the chained index
    /// crosses the kind boundaries — npc.len() + vehicle.len() + 0
    /// for the rain kind's first instance).
    #[test]
    fn repack_refusal_preserves_the_prior_frame() {
        let (npc, vehicle, rain) = fixture_populations();
        let mut buffer = Vec::new();
        repack_populations(&mut buffer, &npc, &vehicle, &rain).unwrap();
        let prior = buffer.clone();
        let mut bad = rain.clone();
        bad[0].alpha = 2.0; // outside [0, 1]
        assert_eq!(
            repack_populations(&mut buffer, &npc, &vehicle, &bad),
            Err(InstancePackError::Invalid {
                index: npc.len() + vehicle.len(),
                field: "alpha"
            })
        );
        assert_eq!(buffer, prior, "the refusal left the prior bytes intact");
        let over: Vec<_> = vec![npc[0]; MAX_INSTANCES];
        assert_eq!(
            repack_populations(&mut buffer, &over, &vehicle, &rain),
            Err(InstancePackError::Capacity {
                capacity: MAX_INSTANCES,
                requested: MAX_INSTANCES + vehicle.len() + rain.len()
            })
        );
        assert_eq!(buffer, prior);
    }

    /// The retained buffer reuses its capacity across frames (the
    /// F-05 discipline made assertable): repacking the same or a
    /// smaller population never reallocates once warmed.
    #[test]
    fn repack_reuses_capacity_without_growth() {
        let (npc, vehicle, rain) = fixture_populations();
        let mut buffer = Vec::new();
        repack_populations(&mut buffer, &npc, &vehicle, &rain).unwrap();
        let warmed = buffer.capacity();
        assert!(warmed >= (npc.len() + vehicle.len() + rain.len()) * INSTANCE_STRIDE);
        for _ in 0..3 {
            repack_populations(&mut buffer, &npc, &vehicle, &rain).unwrap();
            assert_eq!(buffer.capacity(), warmed, "no reallocation once warmed");
        }
        repack_populations(&mut buffer, &npc, &vehicle, &[]).unwrap();
        assert_eq!(
            buffer.capacity(),
            warmed,
            "a smaller population keeps the capacity"
        );
        assert_eq!(buffer.len(), (npc.len() + vehicle.len()) * INSTANCE_STRIDE);
    }
}
