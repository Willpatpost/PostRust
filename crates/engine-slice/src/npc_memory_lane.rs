//! The composed memory-lane world (the Era-2 NPC-AI design pass §5.1/§5.3's
//! composition, part 7 stage 2 — the capture-lane gates' driven half).
//!
//! # DESIGN REFERENCE
//! - Game policy by placement (the corner_behavior precedent): the
//!   world below composes the REAL engine machinery — the spine's
//!   event execution, the inverted-query census, the LOD
//!   suspend/resume transitions, the commit pipeline — with AUTHORED
//!   POLICY (which encounter kinds are durable-significant, the
//!   radii, the relationship deltas, the reputation classes and
//!   weights — all [OPEN] design §9.2, the owner's decisions).
//! - The census is the REAL inverted query, tier-gated: a dormant
//!   region's NPCs witness nothing (I2).
//! - The LOD transitions are the REAL spine machinery
//!   (suspend/resume over the region's events, schedule_held for new
//!   work targeting dormant regions).
//! - The pipeline ride is the REAL CommitPipeline in the crossing's
//!   staged-commit order: stage → the T1 journal capture (the
//!   fsynced durable boundary) → mark_applied → commit_canonical.

use engine_core::SaveError;
use engine_core::clock::Tick;
use engine_core::event::{Event, EventClass, EventId, EventSpec, Field, Schedule};
use engine_core::fingerprint::Fingerprinter;
use engine_core::lod::LodLevel;
use engine_core::npc_execution::enumerate_witnesses;
use engine_core::npc_memory::{MemoryDelta, MemoryKey, MemoryKind, MemoryLane, MemoryRecord};
use engine_core::npc_perception::NpcRecord;
use engine_core::npc_perception::perception_budget;
use engine_core::spine::{ExecCtx, ObligationView, ScheduleError, Spine, World};
use engine_persist::CanonicalMedium;
use engine_persist::consequence::{AwardCheckpoint, ConsequenceId, Coverage, OccurrenceSpan};
use engine_persist::envelope::{
    ClassifiedSpan, ConsequenceEnvelope, CounterValues, DurabilityClass, SignalLane,
    SourceBaseline, SourcePosition,
};
use engine_persist::journal::FrontierEntry;
use engine_persist::pipeline::{CommitOutcome, CommitPipeline, PipelineError};

/// The number of memory regions (policy: 4 regions at the corner scale).
pub const MEMORY_REGIONS: usize = 4;

/// The population size: 24 NPCs, 6 per region (the corner scale —
/// design §5.1's measured-trigger clause: the linear census is exact
/// and cheap here; any future index is a measured decision).
pub const MEMORY_POPULATION: usize = 24;

/// The NPCs per region (the population's layout constant).
pub const REGION_NPCS: usize = MEMORY_POPULATION / MEMORY_REGIONS;

/// The encounter kinds (POLICY: which classes are durable-significant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncounterKind {
    Altercation = 1,
    Theft = 2,
    Greeting = 3,
}

impl EncounterKind {
    #[must_use]
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(EncounterKind::Altercation),
            2 => Some(EncounterKind::Theft),
            3 => Some(EncounterKind::Greeting),
            _ => None,
        }
    }

    #[must_use]
    pub fn tag(self) -> u8 {
        self as u8
    }
}

/// The encounter policy table (AUTHORED, pure data): the census
/// radius, the relationship delta (if the kind produces one), the
/// reputation candidate (class tag, weight — if the kind produces
/// one). All values policy, [OPEN] design §9.2.
pub struct EncounterSpec {
    pub radius: f32,
    pub relationship: Option<i32>,
    pub candidate: Option<(u32, i32)>,
}

pub const ENCOUNTER_TABLE: [EncounterSpec; 3] = [
    EncounterSpec {
        radius: 3.0,
        relationship: Some(-2),
        candidate: Some((1, -3)),
    }, // Altercation
    EncounterSpec {
        radius: 4.0,
        relationship: Some(-4),
        candidate: Some((2, -5)),
    }, // Theft
    EncounterSpec {
        radius: 2.0,
        relationship: None,
        candidate: None,
    }, // Greeting
];

/// The population: 24 authored positions (every coordinate dyadic —
/// exactly representable in f32), 6 per region at region-local
/// offsets from each region's place. Region r's NPCs carry ids
/// r*6..r*6+5 and sit at region-local coordinates
/// {(0.5,0), (0.75,0.75), (0,1.5), (-1.25,1.25), (-2.5,0), (3,0)}
/// translated by PLACES[r] — so the distances from PLACES[r] are
/// exactly {0.5, 0.75√2≈1.06, 1.5, 1.25√2≈1.77, 2.5, 3.0} (every
/// same-region NPC is within the Altercation radius 3.0 inclusive —
/// the (3,0) one exactly ON it, the inclusive-boundary pin through
/// the real path). The ADJACENCY fixture (measured, pinned by the
/// lib battery): at the Theft radius 4.0 exactly FOUR cross-region
/// NPCs are reachable — each region's (3,0)-offset NPC sits 3.0 from
/// the horizontally-adjacent place (region 0's NPC 5 from region 1's
/// place, region 2's NPC 17 from region 3's place, and the mirrored
/// pair: region 1's NPC 10 at (3.5,0) sits 3.5 from region 0's
/// place, region 3's NPC 22 at (3.5,6) sits 3.5 from region 2's
/// place) — the I2 geometry (geometrically inside, excluded when the
/// witness's region is Dormant); at the Greeting radius 2.0 NO
/// cross-region NPC is reachable; at the Altercation radius 3.0 only
/// the two exact-boundary 3.0 cases reach (inclusive), never the 3.5
/// pair.
pub const POPULATION: [NpcRecord; MEMORY_POPULATION] = [
    NpcRecord {
        id: 0,
        pos: [0.5, 0.0],
    },
    NpcRecord {
        id: 1,
        pos: [0.75, 0.75],
    },
    NpcRecord {
        id: 2,
        pos: [0.0, 1.5],
    },
    NpcRecord {
        id: 3,
        pos: [-1.25, 1.25],
    },
    NpcRecord {
        id: 4,
        pos: [-2.5, 0.0],
    },
    NpcRecord {
        id: 5,
        pos: [3.0, 0.0],
    },
    NpcRecord {
        id: 6,
        pos: [6.5, 0.0],
    },
    NpcRecord {
        id: 7,
        pos: [6.75, 0.75],
    },
    NpcRecord {
        id: 8,
        pos: [6.0, 1.5],
    },
    NpcRecord {
        id: 9,
        pos: [4.75, 1.25],
    },
    NpcRecord {
        id: 10,
        pos: [3.5, 0.0],
    },
    NpcRecord {
        id: 11,
        pos: [9.0, 0.0],
    },
    NpcRecord {
        id: 12,
        pos: [0.5, 6.0],
    },
    NpcRecord {
        id: 13,
        pos: [0.75, 6.75],
    },
    NpcRecord {
        id: 14,
        pos: [0.0, 7.5],
    },
    NpcRecord {
        id: 15,
        pos: [-1.25, 7.25],
    },
    NpcRecord {
        id: 16,
        pos: [-2.5, 6.0],
    },
    NpcRecord {
        id: 17,
        pos: [3.0, 6.0],
    },
    NpcRecord {
        id: 18,
        pos: [6.5, 6.0],
    },
    NpcRecord {
        id: 19,
        pos: [6.75, 6.75],
    },
    NpcRecord {
        id: 20,
        pos: [6.0, 7.5],
    },
    NpcRecord {
        id: 21,
        pos: [4.75, 7.25],
    },
    NpcRecord {
        id: 22,
        pos: [3.5, 6.0],
    },
    NpcRecord {
        id: 23,
        pos: [9.0, 6.0],
    },
];

/// One authored encounter place per region (dyadic f32).
pub const PLACES: [[f32; 2]; MEMORY_REGIONS] = [[0.0, 0.0], [6.0, 0.0], [0.0, 6.0], [6.0, 6.0]];

/// The encounter payload codec: magic 0x4D45 in bits 63..48, the
/// event id 24 bits (47..24), kind 8 bits (23..16), place 8 bits
/// (15..8), region 8 bits (7..0). A value that does not fit is
/// `None` — the typed refusal, never a silent truncation (the
/// wake_payload/witness_payload precedent; the magic's top byte 0x4D
/// cannot collide with the 0xA1/0xA2 wake/witness tags in bits
/// 63..56).
pub fn encounter_payload(event: u32, kind: u8, place: u8, region: u8) -> Option<u64> {
    if event >= 1 << 24 {
        return None;
    }
    Some(
        (0x4D45_u64 << 48)
            | (u64::from(event) << 24)
            | (u64::from(kind) << 16)
            | (u64::from(place) << 8)
            | u64::from(region),
    )
}

/// The decode side of [`encounter_payload`] — the magic check first
/// (a foreign payload is None, never misparsed).
pub fn encounter_payload_parts(payload: u64) -> Option<(u32, u8, u8, u8)> {
    if (payload >> 48) as u16 != 0x4D45 {
        return None;
    }
    let event = ((payload >> 24) & 0xFF_FFFF) as u32;
    let kind = ((payload >> 16) & 0xFF) as u8;
    let place = ((payload >> 8) & 0xFF) as u8;
    let region = (payload & 0xFF) as u8;
    Some((event, kind, place, region))
}

/// The encounter event class (opaque; the world's only class).
pub const ENCOUNTER_CLASS: EventClass = EventClass(0x4D45);

/// One executed encounter — the evidence lane the fuzz oracle reads
/// (the witnesses as the census produced them, in census order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encounter {
    pub event: u32,
    pub kind: EncounterKind,
    pub place: u8,
    pub region: u8,
    pub tick: Tick,
    pub witnesses: Vec<u32>,
}

/// The composed world: the memory lane (engine-core), the region
/// levels, the encounter log, and the caller-held census scratch
/// (the F-02 discipline — reused, never reallocated per event).
pub struct MemoryLaneWorld {
    pub lane: MemoryLane,
    pub levels: [LodLevel; MEMORY_REGIONS],
    pub encounter_log: Vec<Encounter>,
    census: Vec<u32>,
    pub executed_encounters: u64,
    pub unknown_payloads: u64,
    pub last_tick: Tick,
}

impl MemoryLaneWorld {
    #[must_use]
    pub fn new() -> Self {
        MemoryLaneWorld {
            lane: MemoryLane::new(),
            levels: [LodLevel::Full; MEMORY_REGIONS],
            encounter_log: Vec::new(),
            census: Vec::new(),
            executed_encounters: 0,
            unknown_payloads: 0,
            last_tick: Tick(0),
        }
    }

    /// The tier's episodic budget (the landed perception table).
    fn episodic_budget(&self, level: LodLevel) -> usize {
        perception_budget(tier_level(level))
    }
}

impl Default for MemoryLaneWorld {
    fn default() -> Self {
        Self::new()
    }
}

/// The LodLevel → level-number mapping `perception_budget` indexes
/// (PERCEPTION_BUDGETS' documented ordering: 0 = Full, 1 =
/// Simplified, 2 = EventDriven, 3 = Dormant — npc_execution's own
/// private tier_level, mirrored here at the policy layer).
fn tier_level(level: LodLevel) -> u8 {
    match level {
        LodLevel::Full => 0,
        LodLevel::Simplified => 1,
        LodLevel::EventDriven => 2,
        LodLevel::Dormant => 3,
    }
}

impl World for MemoryLaneWorld {
    fn execute(&mut self, event: &Event, ctx: &mut ExecCtx) {
        // The class filter is total: non-encounter classes are not
        // this world's (and count nowhere).
        if event.class != ENCOUNTER_CLASS {
            return;
        }

        let Some((event_id, kind_tag, place, region)) = encounter_payload_parts(event.payload)
        else {
            self.unknown_payloads += 1;
            return;
        };

        let Some(kind) = EncounterKind::from_tag(kind_tag) else {
            self.unknown_payloads += 1;
            return;
        };

        // The census inputs: the tiers from the CURRENT region levels
        // (a dormant region's NPCs are excluded from every census —
        // I2, the real tier gate).
        let mut tiers = [LodLevel::Full; MEMORY_POPULATION];
        for (i, tier) in tiers.iter_mut().enumerate() {
            *tier = self.levels[i / REGION_NPCS];
        }

        let place_pos = PLACES[place as usize];
        let radius = ENCOUNTER_TABLE[kind as usize - 1].radius;
        enumerate_witnesses(place_pos, radius, &POPULATION, &tiers, &mut self.census);

        // The authored actor rule (POLICY): the encounter's subject is
        // the region-local NPC the event id selects — deterministic,
        // never a location query.
        let actor = POPULATION
            .get((region as usize * REGION_NPCS) + ((event_id % REGION_NPCS as u32) as usize))
            .map(|r| r.id)
            .expect("the authored actor rule indexes the region's own six NPCs");

        let mut witness_ids = Vec::new();
        for &witness_idx in &self.census {
            let witness = POPULATION[witness_idx as usize].id;
            witness_ids.push(witness);

            let budget = self.episodic_budget(tiers[witness_idx as usize]);
            let tick = ctx.executed_tick();
            let event_key = u64::from(event_id);

            // The authored record kinds in FIXED ORDER: episodic
            // ALWAYS, then the relationship, then the candidate (the
            // policy table's shape). A MemoryLaneError is an
            // engine-invariant break — the fail-visible panic
            // (execute cannot return Result).
            let episodic = MemoryRecord {
                key: MemoryKey {
                    event: event_key,
                    witness,
                    kind: MemoryKind::Episodic,
                },
                delta: MemoryDelta::episodic(witness, actor, tick, u32::from(kind.tag())),
                tick,
            };
            if let Err(e) = self.lane.capture_and_apply(episodic, budget) {
                panic!("memory lane: the episodic capture failed: {e}");
            }

            if let Some(rel_delta) = ENCOUNTER_TABLE[kind as usize - 1].relationship {
                let record = MemoryRecord {
                    key: MemoryKey {
                        event: event_key,
                        witness,
                        kind: MemoryKind::Relationship,
                    },
                    delta: MemoryDelta::Relationship {
                        actor,
                        delta: rel_delta,
                    },
                    tick,
                };
                if let Err(e) = self.lane.capture_and_apply(record, budget) {
                    panic!("memory lane: the relationship capture failed: {e}");
                }
            }

            if let Some((class, weight)) = ENCOUNTER_TABLE[kind as usize - 1].candidate {
                let record = MemoryRecord {
                    key: MemoryKey {
                        event: event_key,
                        witness,
                        kind: MemoryKind::ReputationCandidate,
                    },
                    delta: MemoryDelta::ReputationCandidate {
                        actor,
                        class,
                        weight,
                    },
                    tick,
                };
                if let Err(e) = self.lane.capture_and_apply(record, budget) {
                    panic!("memory lane: the candidate capture failed: {e}");
                }
            }
        }

        self.encounter_log.push(Encounter {
            event: event_id,
            kind,
            place,
            region,
            tick: ctx.executed_tick(),
            witnesses: witness_ids,
        });
        self.executed_encounters += 1;
        self.last_tick = ctx.executed_tick();
    }

    /// The canonical logical record: the lane digest, the region
    /// levels, the counters (domain version 1, never mutated).
    fn fingerprint(&self, fp: &mut Fingerprinter) {
        fp.write_str("engine-slice/memory-lane-world/1");
        fp.write_u64(self.lane.digest());
        for level in &self.levels {
            fp.write_u8(match level {
                LodLevel::Full => 0,
                LodLevel::Simplified => 1,
                LodLevel::EventDriven => 2,
                LodLevel::Dormant => 3,
            });
        }
        fp.write_u64(self.executed_encounters);
        fp.write_u64(self.unknown_payloads);
        fp.write_usize(self.encounter_log.len());
    }
}

/// Set a region's level through the REAL spine transitions: to
/// Dormant = suspend the region's pending encounter events
/// (identity-preserving, the held set) + deactivate the region's
/// witnesses (the lane's discard); from Dormant = activate the
/// witnesses (the rebuild at the new tier's budget) + resume the
/// held events; between active levels = the level table only (the
/// census's tier gating and the next apply's budget change; the
/// representation stays). Returns the number of suspended-or-resumed
/// events.
pub fn set_region_level(spine: &mut Spine<MemoryLaneWorld>, region: usize, level: LodLevel) -> u32 {
    let current_level = spine.world().levels[region];
    if current_level == level {
        return 0;
    }

    let from_is_active = current_level != LodLevel::Dormant;
    let to_is_active = level != LodLevel::Dormant;

    // Between active levels: the table AND the immediate retention
    // re-application — the tier change re-folds each region witness's
    // representation at the NEW tier's budget (the memory depth
    // follows the tier NOW, never lazily at the next append: the
    // live view is always budget-consistent, so the rebuild at any
    // later activation is byte-identical at a fixed level).
    if from_is_active && to_is_active {
        spine.world_mut().levels[region] = level;
        let budget = perception_budget(tier_level(level));
        for record in POPULATION
            .iter()
            .take((region + 1) * REGION_NPCS)
            .skip(region * REGION_NPCS)
        {
            spine
                .world_mut()
                .lane
                .activate(record.id, budget)
                .expect("the rebuild's checked accumulation cannot overflow at authored deltas");
        }
        return 0;
    }

    // The region predicate: the class check AND the payload's region
    // field (the decode is the single source of the region's events).
    let matches_region = |view: &ObligationView| {
        view.class == ENCOUNTER_CLASS
            && encounter_payload_parts(view.payload)
                .is_some_and(|(_, _, _, r)| r as usize == region)
    };

    if !to_is_active {
        // Deactivation: suspend the region's events, then discard the
        // witnesses' representations (the capture half happened at
        // production — the ledger holds every record).
        let collected = spine.collect_matching(matches_region);
        let ids: Vec<EventId> = collected.iter().map(|v| v.id).collect();
        let suspended = spine.suspend_ids(&ids);

        for record in POPULATION
            .iter()
            .take((region + 1) * REGION_NPCS)
            .skip(region * REGION_NPCS)
        {
            spine.world_mut().lane.deactivate(record.id);
        }
        spine.world_mut().levels[region] = level;
        suspended
    } else {
        // Activation: the witnesses rebuild at the new tier's budget,
        // then the held events resume (they fire on their own due
        // ticks — the resumed-but-not-fired window is the spine's).
        spine.world_mut().levels[region] = level;
        let budget = perception_budget(tier_level(level));
        for record in POPULATION
            .iter()
            .take((region + 1) * REGION_NPCS)
            .skip(region * REGION_NPCS)
        {
            spine
                .world_mut()
                .lane
                .activate(record.id, budget)
                .expect("the rebuild's checked accumulation cannot overflow at authored deltas");
        }
        spine.resume_matching(&matches_region)
    }
}

/// Author an encounter event: into the held set if the region is
/// Dormant (schedule_held — new work targeting dormant regions never
/// fires unnoticed), else the normal schedule. Due at `due`, in the
/// Near lane (the census is near-field work).
pub fn schedule_encounter(
    spine: &mut Spine<MemoryLaneWorld>,
    event: u32,
    kind: EncounterKind,
    place: u8,
    region: u8,
    due: Tick,
) -> Result<EventId, ScheduleError> {
    let region_level = spine.world().levels[region as usize];

    let payload = encounter_payload(event, kind.tag(), place, region)
        .expect("the authored event id fits 24 bits and the tags fit their bytes");

    let spec = EventSpec {
        class: ENCOUNTER_CLASS,
        schedule: Schedule::AtLogicalTime { due },
        payload,
        field: Field::Near,
        depends_on: Vec::new(),
    };

    if region_level == LodLevel::Dormant {
        spine.schedule_held(spec)
    } else {
        spine.schedule(spec)
    }
}

/// The commit bookkeeping: per witness, the keys already claimed by a
/// pipeline episode (in-flight or confirmed — a dropped ack leaves
/// them claimed-but-pending in the lane, re-submittable), and the
/// episode counter (each commit's fresh ConsequenceId episode).
#[derive(Debug, Clone, Default)]
pub struct MemoryCommitBook {
    pub claimed: std::collections::BTreeSet<MemoryKey>,
    pub episodes: u64,
}

/// Commit one witness's unclaimed pending records through the REAL
/// pipeline (the crossing's staged-commit shape, verbatim in order):
/// stage the capture envelope → the T1 journal CAPTURE (the fsynced
/// transaction — THE durable boundary; a key is journaled before any
/// live claim, the pipeline's AppliedBeforeJournal refusal enforces
/// it) → mark_applied (the live claim) → commit_canonical (the
/// canonical confirmation — on Confirmed, each claimed key confirms
/// in the lane). `submit == false` is the DROPPED-ACK fault: stage +
/// journal-capture + mark_applied but NO canonical commit — the keys
/// stay claimed-but-pending in the lane (pending-live: the rebuild
/// and the reputation are unaffected; a later full commit re-derives
/// the actionable keys from the journal and confirms them — the
/// C2 re-delivery). Returns the keys this episode claimed.
pub fn commit_witness_captures<M: CanonicalMedium>(
    spine: &mut Spine<MemoryLaneWorld>,
    pipeline: &mut CommitPipeline<M>,
    book: &mut MemoryCommitBook,
    witness: u32,
    submit: bool,
) -> Result<Vec<MemoryKey>, PipelineError> {
    let region = (witness as usize) / REGION_NPCS;

    let keys: Vec<MemoryKey> = spine
        .world_mut()
        .lane
        .ledger()
        .pending_keys(witness)
        .into_iter()
        .filter(|k| !book.claimed.contains(k))
        .collect();

    if keys.is_empty() {
        return Ok(Vec::new());
    }

    let count = keys.len() as u64;
    let episode = book.episodes;
    book.episodes += 1;

    let position = SourcePosition {
        head: None,
        tick: spine.world().last_tick.as_u64(),
    };
    let consequence = ConsequenceId::new(region as u32, episode, witness);

    let span = OccurrenceSpan::new(0, count)
        .map_err(|_| PipelineError::Format(SaveError::Integrity("memory-lane/capture-span")))?;
    let envelope = ConsequenceEnvelope::capture(
        consequence,
        vec![ClassifiedSpan::new(span, DurabilityClass::DurableEligible)],
        SignalLane::Captured,
        CounterValues::default(),
        SourceBaseline {
            counters: CounterValues::default(),
            award: AwardCheckpoint {
                durable: Coverage::new(),
                provisional: Coverage::new(),
            },
        },
        position,
    )
    .map_err(PipelineError::Format)?;

    pipeline.stage(envelope)?;

    // THE PLAN BLOB (the journal transaction's plan, opaque to the
    // pipeline): the witness id and the claimed-record count, both
    // little-endian — the lane's decodable recovery material.
    let mut plan_bytes = Vec::with_capacity(12);
    plan_bytes.extend_from_slice(&witness.to_le_bytes());
    plan_bytes.extend_from_slice(&count.to_le_bytes());

    // THE T1 CAPTURE — the fsynced journal transaction over the
    // staged envelope (the durable boundary; the CaptureReceipt is
    // post-fsync by contract). WITHOUT THIS the key is InMemory and
    // mark_applied rightly refuses (AppliedBeforeJournal —
    // durable-before-live).
    pipeline.capture(
        position,
        DurabilityClass::DurableEligible,
        Vec::new(),
        vec![FrontierEntry {
            region: region as u32,
            episode,
        }],
        plan_bytes,
    )?;

    pipeline.mark_applied(&[consequence])?;

    // The claim is established at the episode's capture — before the
    // submission branch (the confirm loop below reads this set).
    for key in &keys {
        book.claimed.insert(*key);
    }

    if submit && let CommitOutcome::Confirmed(_) = pipeline.commit_canonical()? {
        // The canonical confirmation covers EVERY actionable journaled
        // key — this episode's AND every previously dropped-ack
        // episode's (any witness: the actionable set is the whole
        // pipeline's). Every claimed key is journaled, so every
        // claimed-but-lane-pending key maps to a pipeline-confirmed
        // consequence here — confirm them all (an already-confirmed
        // key is a no-op).
        let claimed: Vec<MemoryKey> = book.claimed.iter().copied().collect();
        for key in claimed {
            spine.world_mut().lane.confirm(&key);
        }
    }
    // NothingPending / Unknown / StaleParent (or the dropped-ack
    // fault's withheld submit): the pipeline's own outcome
    // discipline — the keys stay claimed and pending-live; the
    // caller re-commits (the honest retry).
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE CODEC ROUND-TRIP: every field survives; the magic check
    /// rejects foreign payloads; the 24-bit event field refuses an
    /// over-wide id (the typed refusal, never a truncation).
    #[test]
    fn encounter_payload_round_trips_and_refuses() {
        let payload = encounter_payload(0x00AB_CDEF, EncounterKind::Theft.tag(), 2, 3)
            .expect("the values fit their fields");
        assert_eq!(
            encounter_payload_parts(payload),
            Some((0x00AB_CDEF, EncounterKind::Theft.tag(), 2, 3))
        );
        assert_eq!(
            encounter_payload(1 << 24, 1, 0, 0),
            None,
            "a 24-bit-overflowing event id is refused"
        );
        // Foreign magic (the wake tag 0xA1 in bits 63..56 — a
        // wake-shaped payload) is None, never misparsed.
        let wake_shaped = 0xA1FF_0000_0000_0000_u64;
        assert_eq!(encounter_payload_parts(wake_shaped), None);
    }

    /// THE TAG-COLLISION PROOF: the encounter magic's top byte (0x4D)
    /// differs from the wake/witness tags' top bytes (0xA1/0xA2), so
    /// no npc_execution payload can decode as an encounter and no
    /// encounter payload can decode as a wake/witness event — the
    /// namespace is disjoint BY LAYOUT, pinned.
    #[test]
    fn encounter_magic_never_collides_with_wake_or_witness_tags() {
        let encounter = encounter_payload(1, 1, 0, 0).expect("fits");
        let top_byte = encounter >> 56;
        assert_eq!(top_byte, 0x4D);
        assert_ne!(top_byte, 0xA1);
        assert_ne!(top_byte, 0xA2);
        // And the wake/witness payloads never carry our magic.
        let wake = engine_core::npc_execution::wake_payload(1, 1).expect("fits");
        let witness = engine_core::npc_execution::witness_payload(1, 1).expect("fits");
        assert_eq!(encounter_payload_parts(wake), None);
        assert_eq!(encounter_payload_parts(witness), None);
    }

    /// THE POPULATION LAYOUT (the placement battery): every region's
    /// six NPCs sit within their own Altercation radius; the
    /// cross-region census at the Theft radius is exactly the four
    /// authored adjacency witnesses; the Greeting radius reaches no
    /// cross-region NPC — the I2 geometry, as measured minimums.
    #[test]
    fn population_layout_is_as_authored() {
        // Same-region distances: every region's six within its own
        // Altercation radius 3.0 (square 9.0), the (3,0) offset
        // exactly ON it.
        for (region, place) in PLACES.iter().enumerate() {
            for record in POPULATION
                .iter()
                .take((region + 1) * REGION_NPCS)
                .skip(region * REGION_NPCS)
            {
                let dx = record.pos[0] - place[0];
                let dy = record.pos[1] - place[1];
                assert!(
                    dx * dx + dy * dy <= 9.0,
                    "region {region}'s NPC {} is within its own Altercation radius",
                    record.id
                );
            }
        }
        // The cross-region census at the Theft radius 4.0 (square
        // 16.0): exactly the four authored adjacency witnesses.
        let mut cross: Vec<u32> = Vec::new();
        for (region, place) in PLACES.iter().enumerate() {
            for record in POPULATION.iter() {
                if record.id as usize / REGION_NPCS == region {
                    continue; // own region
                }
                let dx = record.pos[0] - place[0];
                let dy = record.pos[1] - place[1];
                if dx * dx + dy * dy <= 16.0 {
                    cross.push(record.id);
                }
            }
        }
        cross.sort_unstable();
        assert_eq!(
            cross,
            vec![5, 10, 17, 22],
            "exactly the four authored adjacency witnesses are Theft-reachable cross-region"
        );
        // The Greeting radius 2.0 (square 4.0) reaches NO cross-region
        // NPC; the Altercation radius 3.0 (square 9.0) reaches only
        // the two exact-boundary 3.0 cases.
        let mut altercation_cross: Vec<u32> = Vec::new();
        for (region, place) in PLACES.iter().enumerate() {
            for record in POPULATION.iter() {
                if record.id as usize / REGION_NPCS == region {
                    continue;
                }
                let dx = record.pos[0] - place[0];
                let dy = record.pos[1] - place[1];
                assert!(
                    dx * dx + dy * dy > 4.0,
                    "Greeting reaches no cross-region NPC ({} at region {region})",
                    record.id
                );
                if dx * dx + dy * dy <= 9.0 {
                    altercation_cross.push(record.id);
                }
            }
        }
        altercation_cross.sort_unstable();
        assert_eq!(
            altercation_cross,
            vec![5, 17],
            "Altercation reaches only the two exact-boundary cross-region witnesses (inclusive)"
        );
    }
}
