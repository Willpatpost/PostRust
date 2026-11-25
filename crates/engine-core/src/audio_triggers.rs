//! The deterministic audio trigger core (Era 2 package B — see
//! `docs/AUDIO-SUBSYSTEM-MAP.md` §2.4, the constraint law §1.1):
//! the sound-trigger projection — a READER of the spine's event
//! stream and the render-side observations of deterministic state,
//! never a writer. Presentation never enters the spine: no audio
//! state, buffer, or timing enters a fingerprint. What IS
//! deterministic: WHICH sounds fire — the same campaign produces
//! the same trigger stream, digest-pinned, provable headless with
//! no audio device at all (the pod's own gate, §4.5).
//!
//! # DETERMINISM CONTRACT
//! The projection is a pure function over its inputs: the observed
//! activity events (which emitters did what this tick, in fixed
//! NPC-table order), the LOD tiers, and the listener snapshot.
//! The output trigger list is fixed-order, bounded (a typed refusal
//! above the pool bound — never a silent drop), and byte-identical
//! across platforms. The digest pins a sampled trigger sequence.
//!
//! # THE ENGINE/POLICY BOUNDARY (the map's rule)
//! The engine ships the projection machinery: the LOD gating (a
//! tier table), the bounded pool, the fixed ordering, the digest.
//! The POLICY ships the class vocabulary (which activity class maps
//! to which sound class) — data above the engine, never code in it.

use crate::lod::LodLevel;

/// A sound class — policy-defined vocabulary (footstep, vehicle
/// pass, ambience bed crossfade, UI). The engine only carries it.
pub type SoundClass = u32;

/// The emitter of a sound: an NPC (by stable id) or a world source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundEmitter {
    Npc(crate::npc_behavior::NpcId),
    /// A world-positioned source (a vehicle, a district bed).
    World(u32),
}

/// One projected sound trigger: the pure data the mixer consumes.
/// Carries NO timing (the audible rendering owns when it plays);
/// carries the class, the emitter, the world position, and the
/// LOD tier at projection time (the tier gates the pool).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundTrigger {
    pub class: SoundClass,
    pub emitter: SoundEmitter,
    /// The emitter's world position at the event (listener-relative
    /// pan/attenuation derive LATER, at the mixer, from the listener
    /// snapshot — never baked here).
    pub position: [f32; 3],
    /// The LOD tier that projected this trigger (diagnostic).
    pub tier: LodLevel,
}

/// The maximum triggers one tick may project (the pool bound —
/// a typed refusal above it, never a silent drop).
pub const TRIGGER_POOL_BOUND: usize = 64;

/// The typed pool overflow (fail-visible, the truncation-counter
/// discipline: the caller MUST observe it, never swallow it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerOverflow {
    /// How many triggers were dropped above the bound.
    pub dropped: u32,
}

/// One observed activity event — the projection's input, supplied
/// by the behavior/locomotion layer in FIXED emitter order (the
/// NPC-table order; the projection never sorts).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActivityObservation {
    pub emitter: SoundEmitter,
    /// The policy's activity class (the vocabulary shared with the
    /// behavior layer — e.g. the footstep-edge class).
    pub activity_class: u32,
    pub position: [f32; 3],
    pub tier: LodLevel,
}

/// The class mapping: which activity classes project to which
/// sound classes (POLICY data — a fixed table the engine walks).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassMapping {
    pub activity_class: u32,
    pub sound_class: SoundClass,
}

/// The tier gate: which LOD tiers may project triggers at all.
/// Dormant NEVER projects (the design's "a dormant NPC makes no
/// sound" — by construction, not by filtering after the fact).
pub fn tier_projects(tier: LodLevel) -> bool {
    !matches!(tier, LodLevel::Dormant)
}

/// THE PROJECTION (§2.4): observations + the class table + the
/// tier gates → the bounded trigger list, in the observations' own
/// fixed order. Pure; allocation-free (writes into the caller's
/// buffer); overflow is the typed refusal the caller must observe.
pub fn project_triggers(
    observations: &[ActivityObservation],
    mappings: &[ClassMapping],
    out: &mut Vec<SoundTrigger>,
) -> Result<(), TriggerOverflow> {
    out.clear();
    let mut dropped: u32 = 0;
    for observation in observations {
        if !tier_projects(observation.tier) {
            continue;
        }
        // The class table walk: fixed order, the first mapping wins
        // (the policy's table is authoritative data).
        let Some(mapping) = mappings
            .iter()
            .find(|mapping| mapping.activity_class == observation.activity_class)
        else {
            continue;
        };
        if out.len() < TRIGGER_POOL_BOUND {
            out.push(SoundTrigger {
                class: mapping.sound_class,
                emitter: observation.emitter,
                position: observation.position,
                tier: observation.tier,
            });
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        return Err(TriggerOverflow { dropped });
    }
    Ok(())
}

/// The trigger-stream digest (the cross-platform byte pin — §4.5's
/// gate): FNV-1a over a sampled trigger sequence's canonical bytes
/// (class, emitter tag + id, position to_bits, tier discriminant),
/// in fixed order.
pub fn trigger_digest(triggers: &[SoundTrigger]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for trigger in triggers {
        feed(&trigger.class.to_le_bytes());
        match trigger.emitter {
            SoundEmitter::Npc(id) => {
                feed(&[0]);
                feed(&id.0.to_le_bytes());
            }
            SoundEmitter::World(id) => {
                feed(&[1]);
                feed(&id.to_le_bytes());
            }
        }
        for value in trigger.position {
            feed(&value.to_bits().to_le_bytes());
        }
        feed(&[tier_tag(trigger.tier)]);
    }
    hash
}

/// The tier's canonical discriminant byte (fixed across platforms).
fn tier_tag(tier: LodLevel) -> u8 {
    match tier {
        LodLevel::Full => 0,
        LodLevel::Simplified => 1,
        LodLevel::EventDriven => 2,
        LodLevel::Dormant => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::npc_behavior::NpcId;

    fn observation(
        emitter_id: u32,
        activity_class: u32,
        position: [f32; 3],
        tier: LodLevel,
    ) -> ActivityObservation {
        ActivityObservation {
            emitter: SoundEmitter::Npc(NpcId(emitter_id)),
            activity_class,
            position,
            tier,
        }
    }

    /// Dormant emitters are silent BY CONSTRUCTION (the tier gate
    /// refuses them before the pool — not a filter after it).
    #[test]
    fn dormant_tiers_never_project() {
        let mappings = [ClassMapping {
            activity_class: 7,
            sound_class: 100,
        }];
        let observations = [
            observation(1, 7, [0.0; 3], LodLevel::Dormant),
            observation(2, 7, [1.0; 3], LodLevel::Full),
        ];
        let mut out = Vec::new();
        project_triggers(&observations, &mappings, &mut out).unwrap();
        assert_eq!(out.len(), 1, "only the full-tier emitter fired");
        assert_eq!(out[0].emitter, SoundEmitter::Npc(NpcId(2)));
    }

    /// The fixed order: the output preserves the observations' own
    /// order (never sorted by position, class, or emitter).
    #[test]
    fn the_output_order_is_the_input_order() {
        let mappings = [ClassMapping {
            activity_class: 1,
            sound_class: 10,
        }];
        let observations = [
            observation(30, 1, [9.0; 3], LodLevel::Full),
            observation(10, 1, [1.0; 3], LodLevel::Full),
            observation(20, 1, [5.0; 3], LodLevel::Simplified),
        ];
        let mut out = Vec::new();
        project_triggers(&observations, &mappings, &mut out).unwrap();
        let ids: Vec<u32> = out
            .iter()
            .map(|t| match t.emitter {
                SoundEmitter::Npc(id) => id.0,
                SoundEmitter::World(id) => id,
            })
            .collect();
        assert_eq!(ids, [30, 10, 20], "the input order preserved");
    }

    /// Unmapped activity classes project nothing (the policy's
    /// table is authoritative — an unmapped class is silence, not
    /// an error).
    #[test]
    fn unmapped_classes_are_silence() {
        let mappings = [ClassMapping {
            activity_class: 5,
            sound_class: 50,
        }];
        let observations = [observation(1, 99, [0.0; 3], LodLevel::Full)];
        let mut out = Vec::new();
        project_triggers(&observations, &mappings, &mut out).unwrap();
        assert!(out.is_empty());
    }

    /// The pool bound: above 64 triggers, the typed refusal carries
    /// the dropped count — fail-visible, never a silent drop.
    #[test]
    fn the_pool_bound_is_a_typed_refusal() {
        let mappings = [ClassMapping {
            activity_class: 1,
            sound_class: 10,
        }];
        let observations: Vec<_> = (0..70u32)
            .map(|i| observation(i, 1, [i as f32; 3], LodLevel::Full))
            .collect();
        let mut out = Vec::new();
        let overflow = project_triggers(&observations, &mappings, &mut out).unwrap_err();
        assert_eq!(overflow.dropped, 6, "70 - 64 bound");
        assert_eq!(out.len(), TRIGGER_POOL_BOUND);
        // At the bound exactly: no refusal.
        let at_bound: Vec<_> = observations[..64].to_vec();
        project_triggers(&at_bound, &mappings, &mut out).unwrap();
        assert_eq!(out.len(), TRIGGER_POOL_BOUND);
    }

    /// World emitters carry their own tag in the digest (the NPC
    /// and world namespaces never collide).
    #[test]
    fn world_emitters_digest_differently_from_npc() {
        let npc = [SoundTrigger {
            class: 1,
            emitter: SoundEmitter::Npc(NpcId(5)),
            position: [0.0; 3],
            tier: LodLevel::Full,
        }];
        let world = [SoundTrigger {
            class: 1,
            emitter: SoundEmitter::World(5),
            position: [0.0; 3],
            tier: LodLevel::Full,
        }];
        assert_ne!(trigger_digest(&npc), trigger_digest(&world));
    }

    /// THE DIGEST PIN: a fixed observation sequence (the walking
    /// crowd's first steps — three emitters, mixed tiers, fixed
    /// positions) projects to this FNV digest, cross-platform,
    /// forever (the §4.5 gate; a move is an investigation).
    #[test]
    fn trigger_digest_pin() {
        let mappings = [
            ClassMapping {
                activity_class: 7,
                sound_class: 100,
            },
            ClassMapping {
                activity_class: 8,
                sound_class: 200,
            },
        ];
        let observations = [
            observation(0, 7, [1.0, 0.0, 2.0], LodLevel::Full),
            observation(1, 7, [-3.0, 0.0, 4.0], LodLevel::Simplified),
            observation(2, 8, [0.5, 0.0, -0.5], LodLevel::Full),
            observation(3, 7, [9.0, 0.0, 9.0], LodLevel::Dormant),
            observation(4, 7, [2.0, 0.0, 2.0], LodLevel::EventDriven),
        ];
        let mut out = Vec::new();
        project_triggers(&observations, &mappings, &mut out).unwrap();
        assert_eq!(out.len(), 4, "the dormant emitter gated");
        assert_eq!(trigger_digest(&out), PINNED_DIGEST);
    }

    /// The measured-actual pin (the four projected triggers above).
    const PINNED_DIGEST: u64 = 1590023602593914298;
}
