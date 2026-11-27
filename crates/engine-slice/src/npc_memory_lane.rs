//! The composed memory-lane world (the Era-2 NPC-AI design pass §5.1/§5.3's
//! composition, part 7 stage 2 — the capture-lane gates' driven half).
//!
//! # THE ERA-2 AUDIT'S CORRECTIVE (findings 1–3, Astra's
//! # highest-value recommendation)
//! - **Finding 1:** [`commit_witness_captures`] separates STAGING
//!   fresh records from FLUSHING existing actionable episodes — the
//!   last unacknowledged capture retries on its own, with zero new
//!   events, in this session or a fresh one.
//! - **Finding 2:** the journal plan is VERSIONED (v2,
//!   [`MEMORY_PLAN_DOMAIN`]/[`MEMORY_PLAN_SCHEMA`]) and carries the
//!   episode's ordered records and the witness's classification sync;
//!   [`recover_memory_lane`] rebuilds a fresh lane and book from the
//!   journal and the canonical chain — individual records, phases and
//!   the classified score, terminating at the journaled, applied and
//!   confirmed boundaries. The pipeline's own wire and save formats
//!   are untouched (the plan is the lane's private, opaque-to-the-
//!   pipeline blob).
//! - **Finding 3:** episodic retention is ONE order — canonical
//!   event-key order — maintained by the live bounded view too
//!   (engine-core `npc_memory`'s `EpisodicMemory`), so a dormancy
//!   round trip cannot change which episode a witness remembers.
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
use engine_core::npc_memory::{
    CaptureOutcome, ClassifyOutcome, ConfirmOutcome, MemoryDelta, MemoryKey, MemoryKind,
    MemoryLane, MemoryRecord,
};
use engine_core::npc_perception::NpcRecord;
use engine_core::npc_perception::perception_budget;
use engine_core::spine::{ExecCtx, ObligationView, ScheduleError, Spine, World};
use engine_core::wire::{Decoder, Encoder};
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

/// One unresolved episode (the sign-off review's finding 20
/// corrective): a definite capture failure or a mid-flight apply
/// failure left an episode incomplete. The book records the
/// identity and membership; the next call completes IT before any
/// new episode is staged — one record set always maps to one
/// envelope, one plan and one consequence, on this session's
/// pipeline or a fresh one's (an unjournaled episode is honestly
/// absent from a recovered book, so a fresh session simply stages
/// it anew under a fresh identity — nothing durable existed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedEpisode {
    pub witness: u32,
    pub episode: u64,
    pub consequence: ConsequenceId,
    /// The episode's keys in envelope order (the membership the
    /// completion's plan must carry exactly).
    pub keys: Vec<MemoryKey>,
    /// How far the episode got: `false` = the journal append was
    /// definitely rejected (the envelope was discarded, nothing is
    /// durable — re-stage and capture); `true` = the capture
    /// SUCCEEDED and the apply failed (the transaction is journaled
    /// — complete the apply and the claim only).
    pub journaled: bool,
}

/// The commit bookkeeping: per witness, the keys already claimed by a
/// pipeline episode (in-flight or confirmed — a dropped ack leaves
/// them claimed-but-pending in the lane, re-submittable), the
/// episode counter (each commit's fresh ConsequenceId episode), the
/// single unresolved episode (finding 20), the per-episode
/// membership for scoped confirmation (finding 20's confirmation
/// scope — the confirmed receipt names episode identities, never
/// "everything claimed"), and the classification sync as of each
/// witness's last journaled episode (finding 21 — a decision taken
/// after the final record episode is flushed by its own journal
/// transaction when submit observes the mismatch).
#[derive(Debug, Clone, Default)]
pub struct MemoryCommitBook {
    pub claimed: std::collections::BTreeSet<MemoryKey>,
    pub episodes: u64,
    pub unresolved: Option<UnresolvedEpisode>,
    /// episode → (identity, keys) for the episodes this session
    /// staged or completed (the recovery rebuilds it from the plans).
    pub episode_keys: std::collections::BTreeMap<u64, (ConsequenceId, Vec<MemoryKey>)>,
    /// witness → the classification sync as of its last journaled
    /// episode (the recovery rebuilds it from the plans).
    pub journaled_classifications: std::collections::BTreeMap<u32, Vec<(MemoryKey, bool)>>,
}

/// The memory-lane journal plan's domain — the EXPLICIT VERSION
/// MARKER of the lane's own recovery material (the blob the pipeline
/// carries transparently inside the T1 transaction; the pipeline's
/// own wire and save formats are untouched by this section).
pub const MEMORY_PLAN_DOMAIN: &str = "sektor7/engine-slice/memory-lane-plan";

/// The plan's schema version. **v2 (the Era-2 audit's finding 2
/// corrective):** the plan carries the ordered records' full canonical
/// bytes and the witness's durable classification decisions — episode
/// identity, membership and the applicable classification history are
/// recoverable from the journal alone. v1 (the defective build's
/// unversioned 12 bytes: a witness id and a count) carried none of
/// it — two different sets of three memories produced the same plan
/// bytes. The v2 reader refuses v1 bytes fail-closed at the domain
/// gate; no v1 journal exists outside test directories (the bridge
/// never shipped beyond them), so there is no compatibility surface
/// to carry — the version marker is the migration boundary.
pub const MEMORY_PLAN_SCHEMA: u8 = 2;

/// One plan's documented maximum record count (bounded decode).
pub const MEMORY_PLAN_MAX_RECORDS: u64 = 1 << 16;
/// One plan's documented maximum classification-sync count (bounded
/// decode).
pub const MEMORY_PLAN_MAX_CLASSIFICATIONS: u64 = 1 << 16;
/// One journaled record's documented maximum byte length (the largest
/// canonical record is 13 + 8 + 21 bytes; the bound is generous, the
/// strict decode enforces the exact shape).
pub const MEMORY_PLAN_MAX_RECORD_BYTES: u64 = 4096;

/// One journaled episode's recovery material (the T1 transaction's
/// plan, version 2): the witness, the episode number, the ordered
/// claimed records' canonical bytes (identity AND membership AND the
/// payloads — the material a fresh session needs), and the
/// classification-sync section — the witness's durable decisions as
/// of this episode (later episodes re-carry earlier decisions, so the
/// sync heals forward; a decision taken after the witness's FINAL
/// journaled episode is durable only in the live lane — the recovery
/// terminates at the journaled, applied and confirmed boundaries,
/// never beyond them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPlan {
    pub witness: u32,
    pub episode: u64,
    pub records: Vec<MemoryRecord>,
    pub classifications: Vec<(MemoryKey, bool)>,
}

impl MemoryPlan {
    /// The canonical encode: the domain tag, the schema byte, the
    /// witness, the episode, then the length-prefixed records and
    /// classification-sync entries (the `wire.rs` primitives —
    /// fixed-width little-endian, bounded on decode).
    pub fn encode(&self) -> Result<Vec<u8>, SaveError> {
        let mut enc = Encoder::new();
        enc.write_str(MEMORY_PLAN_DOMAIN);
        enc.write_u8(MEMORY_PLAN_SCHEMA);
        enc.write_u32(self.witness);
        enc.write_u64(self.episode);
        enc.write_count(self.records.len());
        for record in &self.records {
            enc.write_bytes(&record.canonical_bytes());
        }
        enc.write_count(self.classifications.len());
        for (key, accepted) in &self.classifications {
            enc.write_bytes(&key.canonical_bytes());
            enc.write_u8(u8::from(*accepted));
        }
        Ok(enc.into_bytes())
    }

    /// The strict decode (fail-closed: the domain gate refuses a v1
    /// plan or any foreign blob, the schema must be exactly v2, every
    /// count is bounded, every record decodes through
    /// [`MemoryRecord::decode`]'s own strict reader, no trailing
    /// bytes).
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        const SECTION: &str = "memory-lane-plan";
        let mut dec = Decoder::new(bytes);
        dec.read_domain(MEMORY_PLAN_DOMAIN)?;
        let schema = dec.read_u8()?;
        if schema != MEMORY_PLAN_SCHEMA {
            return Err(SaveError::Version {
                observed: u32::from(schema),
                supported: u32::from(MEMORY_PLAN_SCHEMA),
            });
        }
        let witness = dec.read_u32()?;
        let episode = dec.read_u64()?;
        let record_count = dec.read_count(MEMORY_PLAN_MAX_RECORDS, SECTION, "record-count")?;
        let mut records = Vec::with_capacity(record_count as usize);
        for _ in 0..record_count {
            let record_bytes = dec.read_bytes(MEMORY_PLAN_MAX_RECORD_BYTES, SECTION)?;
            records.push(MemoryRecord::decode(record_bytes)?);
        }
        let classification_count = dec.read_count(
            MEMORY_PLAN_MAX_CLASSIFICATIONS,
            SECTION,
            "classification-count",
        )?;
        let mut classifications = Vec::with_capacity(classification_count as usize);
        for _ in 0..classification_count {
            let key_bytes = dec.read_bytes(MemoryKey::KEY_STRIDE as u64, SECTION)?;
            let key = MemoryKey::decode(key_bytes)?;
            let accepted = dec.read_u8()? != 0;
            classifications.push((key, accepted));
        }
        dec.finish()?;
        Ok(MemoryPlan {
            witness,
            episode,
            records,
            classifications,
        })
    }
}

/// Commit one witness's unclaimed pending records through the REAL
/// pipeline (the crossing's staged-commit shape, verbatim in order):
/// stage the capture envelope → the T1 journal CAPTURE (the fsynced
/// transaction — THE durable boundary; a key is journaled before any
/// live claim, the pipeline's AppliedBeforeJournal refusal enforces
/// it) → mark_applied (the live claim) → commit_canonical (the
/// canonical confirmation — on Confirmed, each claimed key confirms
/// in the lane).
///
/// **STAGING IS SEPARATE FROM FLUSHING (the Era-2 audit's finding 1
/// corrective):** staging creates a new episode ONLY for this
/// witness's fresh (unclaimed) pending records; the flush then runs
/// whenever `submit` is true — REGARDLESS of whether this call staged
/// anything — driving `commit_canonical` over the pipeline's WHOLE
/// actionable set (every previously claimed-but-unconfirmed episode,
/// this witness's dropped acks and any other's). A withheld
/// submission therefore retries ON ITS OWN: a second call with
/// `submit == true` and zero new events flushes the same episode; a
/// failed canonical commit after claiming (the claims are already
/// established) has the identical retry path — the final
/// unacknowledged capture never waits for a fresh record to arrive.
/// `submit == false` is the DROPPED-ACK fault: stage, journal-capture
/// and mark_applied but NO canonical commit — the keys stay
/// claimed-but-pending in the lane (pending-live: the rebuild and the
/// reputation are unaffected; the C2 re-delivery). Returns ONLY THIS
/// CALL'S freshly staged keys (a retry's empty return is the
/// no-new-episode shape, never a refusal — the flush still happened).
///
/// **The plan blob (v2 — the finding 2 corrective):** the episode's
/// journaled plan carries the ordered records' canonical bytes and
/// the witness's classification-sync section, so a fresh session
/// recovers episode identity, membership and the applicable
/// classification history from the journal alone
/// ([`recover_memory_lane`]).
pub fn commit_witness_captures<M: CanonicalMedium>(
    spine: &mut Spine<MemoryLaneWorld>,
    pipeline: &mut CommitPipeline<M>,
    book: &mut MemoryCommitBook,
    witness: u32,
    submit: bool,
) -> Result<Vec<MemoryKey>, PipelineError> {
    let region = (witness as usize) / REGION_NPCS;

    // ---- THE UNRESOLVED COMPLETION (the sign-off review's finding
    // 20 corrective): a definite journal rejection or a mid-flight
    // apply failure left one episode incomplete, and the book holds
    // its stable identity. The completion runs BEFORE any new
    // episode is staged — the defective bridge consumed a fresh
    // episode number on retry and staged a SECOND envelope for the
    // same records, journaling the leftover under the new plan (two
    // consequences for one memory set, and a cold reopen that
    // refuses the journal at the identity cross-check). The
    // completion reuses the recorded identity exactly; an
    // unjournaled failure re-stages the SAME envelope (the failed
    // attempt's was discarded before the error propagated — nothing
    // is double-staged on this pipeline, and a fresh pipeline has
    // nothing staged at all).
    if let Some(unresolved) = book.unresolved.take() {
        complete_unresolved_episode(spine, pipeline, book, unresolved)?;
    }

    // ---- THE STAGING HALF: fresh (unclaimed) pending records only. A
    // previously claimed key's transaction is already durable —
    // re-staging it would be the pipeline's conflicting reuse, and a
    // second episode for the same keys would double-count; the retry
    // is the FLUSH below, never a re-stage.
    let keys: Vec<MemoryKey> = spine
        .world_mut()
        .lane
        .ledger()
        .pending_keys(witness)
        .into_iter()
        .filter(|k| !book.claimed.contains(k))
        .collect();

    if !keys.is_empty() {
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

        // THE PLAN BLOB v2 (the journal transaction's plan, opaque to
        // the pipeline): the episode's identity and membership — the
        // ordered claimed records' CANONICAL BYTES (the key, the tick,
        // the delta — the material a fresh session decodes), plus the
        // witness's classification-sync section (the durable decisions
        // as of this episode).
        let records: Vec<MemoryRecord> = {
            let lane = &spine.world().lane;
            keys.iter()
                .map(|key| {
                    *lane
                        .ledger()
                        .record(key)
                        .expect("a pending key's record is in the ledger")
                })
                .collect()
        };
        let classifications = spine.world().lane.witness_classifications(witness);
        let plan = MemoryPlan {
            witness,
            episode,
            records,
            classifications: classifications.clone(),
        }
        .encode()
        .map_err(PipelineError::Format)?;

        // THE T1 CAPTURE — the fsynced journal transaction over the
        // staged envelope (the durable boundary; the CaptureReceipt is
        // post-fsync by contract). WITHOUT THIS the key is InMemory and
        // mark_applied rightly refuses (AppliedBeforeJournal —
        // durable-before-live). A DEFINITE REJECTION (CaptureAborted:
        // the journal's pre-window failure, or the ambiguous-write
        // reconciliation establishing NotCommitted) is the finding 20
        // boundary: the envelope is DISCARDED here (nothing durable
        // exists for it) and the episode is recorded UNRESOLVED in the
        // book — the retry completes the same identity, never a second
        // envelope. Any other error (a conflicting payload, a hard
        // journal corruption) is the pipeline's fail-closed class:
        // propagated as-is, no retry state (the pipeline's own
        // contract forbids retrying those as the same attempt).
        if let Err(error) = pipeline.capture(
            position,
            DurabilityClass::DurableEligible,
            Vec::new(),
            vec![FrontierEntry {
                region: region as u32,
                episode,
            }],
            plan,
        ) {
            if matches!(error, PipelineError::CaptureAborted) {
                pipeline.discard_staged(&[consequence]);
                book.unresolved = Some(UnresolvedEpisode {
                    witness,
                    episode,
                    consequence,
                    keys: keys.clone(),
                    journaled: false,
                });
            }
            return Err(error);
        }

        // The claim is established AT the journal boundary (the
        // transaction is durable — the keys are claimed-journaled
        // even if the apply below fails; the finding 20 completion
        // finishes the apply for exactly this case, and a process
        // death between the two is healed by the recovery's own
        // claim-from-plan).
        for key in &keys {
            book.claimed.insert(*key);
        }
        book.episode_keys
            .insert(episode, (consequence, keys.clone()));
        book.journaled_classifications
            .insert(witness, classifications);

        // THE APPLY (the live-authoritative boundary). A failure here
        // leaves the transaction JOURNALED and the keys claimed — the
        // episode is recorded unresolved at the journaled phase; the
        // retry completes the apply only (never a second envelope).
        if let Err(error) = pipeline.mark_applied(&[consequence]) {
            book.unresolved = Some(UnresolvedEpisode {
                witness,
                episode,
                consequence,
                keys: keys.clone(),
                journaled: true,
            });
            return Err(error);
        }
    }

    // ---- THE CLASSIFICATION FLUSH (the sign-off review's finding 21
    // corrective, extended to the retry discipline): a classification
    // decision taken AFTER the witness's final record episode is
    // durable only in the live lane — a cold reopen rebuilds from the
    // last journaled plan and the decision vanishes (the reported
    // score and the decision itself). Submit now observes the mismatch
    // between the lane's current classification sync and the book's
    // last-journaled snapshot, and flushes the decisions through their
    // OWN journal transaction — a classification-sync episode (an
    // empty record set, the unit-marker span, the current sync as the
    // plan). This follows the same retry discipline as the record
    // path: failure after stage records an unresolved episode so the
    // retry completes the same identity, never a second envelope.
    if submit && keys.is_empty() && book.unresolved.is_none() {
        let current = spine.world().lane.witness_classifications(witness);
        if !current.is_empty() && book.journaled_classifications.get(&witness) != Some(&current) {
            flush_witness_classifications(spine, pipeline, book, region, witness, current)?;
        }
    }

    // ---- THE FLUSH HALF (the finding 1 corrective): submit drives the
    // canonical commit over the whole actionable set even when THIS
    // call staged nothing — the last unacknowledged capture's own
    // retry, with zero new events. A transient commit failure
    // propagates (the claims and the journal transaction stand;
    // nothing is rolled back); the next submit call — this session's
    // or a recovered one's — re-derives the actionable set from the
    // journal and finishes it.
    if submit {
        // THE CONFIRMED-IDENTITY SWEEP (finding 20's reconciliation
        // half): the canonical chain is the authority. An ambiguous
        // submission (Unknown) or a later NothingPending can leave a
        // transaction the chain verifiably holds while the lane still
        // shows it pending — the sweep confirms every tracked episode
        // the chain already confirmed, whatever THIS commit call's
        // own outcome turns out to be.
        confirm_chain_held_episodes(spine, pipeline, book);

        // THE SCOPED CONFIRMATION (finding 20's confirmation scope):
        // the receipt names the confirmed episode IDENTITIES — only
        // their keys confirm in the lane. The defective bridge
        // confirmed every claimed key after any successful batch (the
        // pipeline filters cancelled transactions; a blanket confirm
        // could confirm a key whose episode was cancelled).
        if let CommitOutcome::Confirmed(receipt) = pipeline.commit_canonical()? {
            for id in &receipt.keys {
                if let Some((_, episode_keys)) = book.episode_keys.get(&id.episode) {
                    for key in episode_keys {
                        spine.world_mut().lane.confirm(key);
                    }
                    book.episode_keys.remove(&id.episode);
                }
            }
            // A canonical confirmation can also settle episodes the
            // receipt's identity list carries indirectly (the whole
            // actionable set commits as one revision) — re-sweep so
            // the chain's own view is reflected before returning.
            confirm_chain_held_episodes(spine, pipeline, book);
        }
    }
    // NothingPending / Unknown / StaleParent (or the dropped-ack
    // fault's withheld submit): the pipeline's own outcome
    // discipline — the sweep above already healed anything the chain
    // verifiably holds; the rest stays claimed and pending-live; the
    // caller re-commits (the honest retry — now self-sufficient:
    // zero new events suffice).
    Ok(keys)
}

/// Complete one unresolved episode (finding 20's corrective — the
/// stable identity the next call finishes before staging anything
/// new). An unjournaled failure re-stages the SAME envelope (its
/// failed attempt's was discarded; nothing is double-staged) and
/// captures the plan over the recorded membership; a journaled
/// failure completes the apply only. Both paths converge on the
/// claim and the book's episode tracking.
fn complete_unresolved_episode<M: CanonicalMedium>(
    spine: &mut Spine<MemoryLaneWorld>,
    pipeline: &mut CommitPipeline<M>,
    book: &mut MemoryCommitBook,
    unresolved: UnresolvedEpisode,
) -> Result<(), PipelineError> {
    let UnresolvedEpisode {
        witness,
        episode,
        consequence,
        keys,
        journaled,
    } = unresolved;
    let region = (witness as usize) / REGION_NPCS;

    if !journaled {
        // The rejected append: re-stage the identity's envelope and
        // journal it (the plan over the SAME keys — their records are
        // still pending-unclaimed in the ledger — plus the CURRENT
        // classification sync, which heals forward exactly as a
        // fresh episode's plan would).
        let span = OccurrenceSpan::new(0, keys.len() as u64)
            .map_err(|_| PipelineError::Format(SaveError::Integrity("memory-lane/capture-span")))?;
        let position = SourcePosition {
            head: None,
            tick: spine.world().last_tick.as_u64(),
        };
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

        let records: Vec<MemoryRecord> = {
            let lane = &spine.world().lane;
            keys.iter()
                .map(|key| {
                    *lane
                        .ledger()
                        .record(key)
                        .expect("an unresolved episode's key is in the ledger")
                })
                .collect()
        };
        let classifications = spine.world().lane.witness_classifications(witness);
        let plan = MemoryPlan {
            witness,
            episode,
            records,
            classifications: classifications.clone(),
        }
        .encode()
        .map_err(PipelineError::Format)?;

        // The same definite-rejection boundary as the staging half:
        // a second rejection re-records the unresolved episode (the
        // identity stays stable across any number of failures).
        if let Err(error) = pipeline.capture(
            position,
            DurabilityClass::DurableEligible,
            Vec::new(),
            vec![FrontierEntry {
                region: region as u32,
                episode,
            }],
            plan,
        ) {
            if matches!(error, PipelineError::CaptureAborted) {
                pipeline.discard_staged(&[consequence]);
                book.unresolved = Some(UnresolvedEpisode {
                    witness,
                    episode,
                    consequence,
                    keys,
                    journaled: false,
                });
            }
            return Err(error);
        }
        for key in &keys {
            book.claimed.insert(*key);
        }
        book.episode_keys
            .insert(episode, (consequence, keys.clone()));
        book.journaled_classifications
            .insert(witness, classifications);
    }

    // The apply: both paths converge (a journaled failure's
    // transaction is already durable; the claim and the episode
    // tracking were established at its capture).
    pipeline.mark_applied(&[consequence])?;
    if journaled {
        // The journaled failure's capture already established the
        // claim and tracking — restore them (the book recorded the
        // episode unresolved AFTER capturing them).
        for key in &keys {
            book.claimed.insert(*key);
        }
        book.episode_keys
            .insert(episode, (consequence, keys.clone()));
    }
    Ok(())
}

/// Flush dirty classification decisions through their OWN journal
/// transaction (finding 21's corrective, extended to the retry
/// discipline): a classification-sync episode — an empty record set
/// with the unit-marker span (the envelope's occurrence space is
/// non-empty by contract; the marker is the recovery's classification-
/// sync shape), the current sync as the plan. The recovery applies
/// the decisions and terminates at the same journaled/applied/
/// confirmed boundaries. The retry discipline mirrors the record
/// path: failure after stage records an unresolved episode so the
/// retry completes the same identity, never a second envelope.
fn flush_witness_classifications<M: CanonicalMedium>(
    spine: &mut Spine<MemoryLaneWorld>,
    pipeline: &mut CommitPipeline<M>,
    book: &mut MemoryCommitBook,
    region: usize,
    witness: u32,
    current: Vec<(MemoryKey, bool)>,
) -> Result<(), PipelineError> {
    let episode = book.episodes;
    book.episodes += 1;
    let consequence = ConsequenceId::new(region as u32, episode, witness);
    let position = SourcePosition {
        head: None,
        tick: spine.world().last_tick.as_u64(),
    };
    // The unit-marker span: the envelope's occurrence space is
    // non-empty by the pipeline's contract; the classification-sync
    // episode covers exactly one marker occurrence and no records
    // (the recovery's membership rule pairs the two).
    let span = OccurrenceSpan::new(0, 1)
        .map_err(|_| PipelineError::Format(SaveError::Integrity("memory-lane/flush-span")))?;
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
    if let Err(error) = pipeline.stage(envelope) {
        // A stage failure is like CaptureAborted for records: nothing
        // is durable, so journaled=false and the retry re-stages.
        book.unresolved = Some(UnresolvedEpisode {
            witness,
            episode,
            consequence,
            keys: Vec::new(),
            journaled: false,
        });
        return Err(error);
    }

    let plan = MemoryPlan {
        witness,
        episode,
        records: Vec::new(),
        classifications: current.clone(),
    }
    .encode()
    .map_err(PipelineError::Format)?;
    if let Err(error) = pipeline.capture(
        position,
        DurabilityClass::DurableEligible,
        Vec::new(),
        vec![FrontierEntry {
            region: region as u32,
            episode,
        }],
        plan,
    ) {
        if matches!(error, PipelineError::CaptureAborted) {
            pipeline.discard_staged(&[consequence]);
            book.unresolved = Some(UnresolvedEpisode {
                witness,
                episode,
                consequence,
                keys: Vec::new(),
                journaled: false,
            });
        }
        return Err(error);
    }

    if let Err(error) = pipeline.mark_applied(&[consequence]) {
        // The transaction is journaled at this point, so journaled=true
        // and the retry completes the apply only.
        book.unresolved = Some(UnresolvedEpisode {
            witness,
            episode,
            consequence,
            keys: Vec::new(),
            journaled: true,
        });
        return Err(error);
    }
    book.episode_keys.insert(episode, (consequence, Vec::new()));
    book.journaled_classifications.insert(witness, current);
    Ok(())
}

/// Confirm every tracked episode the canonical chain already holds
/// (finding 20's reconciliation half — the authority is the chain,
/// never the last submission's outcome).
fn confirm_chain_held_episodes<M: CanonicalMedium>(
    spine: &mut Spine<MemoryLaneWorld>,
    pipeline: &CommitPipeline<M>,
    book: &mut MemoryCommitBook,
) {
    let held: Vec<u64> = book
        .episode_keys
        .iter()
        .filter(|(_, (id, _))| pipeline.is_confirmed(*id))
        .map(|(episode, _)| *episode)
        .collect();
    for episode in held {
        if let Some((_, keys)) = book.episode_keys.remove(&episode) {
            for key in keys {
                spine.world_mut().lane.confirm(&key);
            }
        }
    }
}

/// The recovered lane and commit book (a fresh session's state,
/// rebuilt from the journal and the canonical chain alone).
pub struct RecoveredMemoryLane {
    pub lane: MemoryLane,
    pub book: MemoryCommitBook,
}

/// THE COLD-REOPEN RECOVERY (the Era-2 audit's finding 2 corrective):
/// rebuild a FRESH lane and commit book from (a) the T1 journal's
/// committed transactions — each plan v2's ordered records and
/// classification sync — and (b) the canonical chain's confirmed
/// identities (the pipeline's own session-open walk). Every witness
/// starts DORMANT (the session-reopen shape: the materialization is
/// the [`MemoryLane::activate`] rebuild, never a silent partial live
/// view). TERMINATES AT THE JOURNALED, APPLIED AND CONFIRMED
/// BOUNDARIES: an unjournaled capture is honestly absent (the
/// pipeline's InMemory discipline), a journaled-but-unconfirmed
/// episode recovers PENDING (its keys claimed, re-submittable by
/// [`commit_witness_captures`]'s flush half), a chain-confirmed
/// episode recovers CONFIRMED. Every cross-check fails closed — a
/// plan that disagrees with its transaction's consequence identity,
/// frontier or membership is corruption, never a guess.
pub fn recover_memory_lane<M: CanonicalMedium>(
    pipeline: &CommitPipeline<M>,
) -> Result<RecoveredMemoryLane, PipelineError> {
    let integrity = |gate: &'static str| PipelineError::Format(SaveError::Integrity(gate));
    let mut lane = MemoryLane::new();
    let mut book = MemoryCommitBook::default();

    for transaction in pipeline.journal().transactions() {
        // The plan is this bridge's own material; a foreign or v1
        // blob is refused at the domain gate (the journal belongs to
        // the memory-lane campaign — fail closed, never skipped).
        let plan = MemoryPlan::decode(&transaction.plan).map_err(PipelineError::Format)?;

        // THE IDENTITY CROSS-CHECK: the plan's (witness, episode)
        // must equal the transaction's own consequence identity —
        // the envelope's ConsequenceId and the frontier's (region,
        // episode). Membership: the envelope's occurrence span
        // covers exactly the plan's record count.
        let Some(envelope) = transaction.envelopes.first() else {
            return Err(integrity("memory-lane/plan-empty-transaction"));
        };
        let Some(frontier) = transaction.frontier.first() else {
            return Err(integrity("memory-lane/plan-missing-frontier"));
        };
        if frontier.episode != plan.episode
            || frontier.region as usize >= MEMORY_REGIONS
            || envelope.id != ConsequenceId::new(frontier.region, plan.episode, plan.witness)
        {
            return Err(integrity("memory-lane/plan-identity-mismatch"));
        }
        let span_count = envelope
            .sub_spans
            .first()
            .map(|classified| classified.span.hi - classified.span.lo)
            .unwrap_or(0);
        if plan.records.is_empty() {
            // THE CLASSIFICATION-SYNC EPISODE (finding 21's
            // corrective): an empty record set with the unit-marker
            // span — the membership rule pairs the two exactly.
            if span_count != 1 || plan.classifications.is_empty() {
                return Err(integrity("memory-lane/plan-flush-membership-mismatch"));
            }
        } else if span_count != plan.records.len() as u64 {
            return Err(integrity("memory-lane/plan-membership-mismatch"));
        }

        // The journaled boundary: every record recovers into the
        // ledger (dormant — counted, never materialized). A key
        // claimed by two episodes is a conflicting reuse the pipeline
        // itself refuses; a duplicate plan record is corruption.
        let mut plan_keys = Vec::with_capacity(plan.records.len());
        for record in &plan.records {
            if record.key.witness != plan.witness {
                return Err(integrity("memory-lane/plan-record-witness-mismatch"));
            }
            match lane.capture_dormant(*record) {
                Ok(CaptureOutcome::Captured) => {}
                Ok(_) => return Err(integrity("memory-lane/plan-duplicate-record")),
                Err(_) => return Err(integrity("memory-lane/plan-record-refused")),
            }
            book.claimed.insert(record.key);
            plan_keys.push(record.key);
        }

        // The applicable classification history: the sync section's
        // decisions re-record durably (a decision re-carried by a
        // later episode is the AlreadyClassified no-op — first
        // decision wins, exactly the live discipline).
        for (key, accepted) in &plan.classifications {
            if key.witness != plan.witness {
                return Err(integrity(
                    "memory-lane/plan-classification-witness-mismatch",
                ));
            }
            match lane.classify(key, *accepted) {
                ClassifyOutcome::Classified | ClassifyOutcome::AlreadyClassified => {}
                ClassifyOutcome::Unknown => {
                    return Err(integrity("memory-lane/plan-unknown-classification"));
                }
            }
        }

        // The confirmed boundary: the canonical chain's own view (the
        // fresh pipeline's session-open walk) decides the phase.
        if pipeline.is_confirmed(envelope.id) {
            for record in &plan.records {
                match lane.confirm(&record.key) {
                    ConfirmOutcome::Confirmed | ConfirmOutcome::AlreadyConfirmed => {}
                    ConfirmOutcome::Unknown => {
                        return Err(integrity("memory-lane/plan-confirm-unknown"));
                    }
                }
            }
            // A chain-confirmed episode is settled: the recovered
            // book does not track it for the flush (the
            // journaled-but-unconfirmed ones stay tracked — the
            // flush's reconciliation sweep and scoped confirmation
            // consume them).
        } else {
            book.episode_keys
                .insert(plan.episode, (envelope.id, plan_keys.clone()));
        }

        // The book's classification snapshot: the LAST journaled
        // plan per witness wins (journal order; later episodes
        // re-carry earlier decisions, so the final sync is the
        // current one — the classification flush's mismatch
        // detection starts from here).
        book.journaled_classifications
            .insert(plan.witness, plan.classifications.clone());

        // The next episode number: one past the highest journaled
        // episode (the frontier's monotonic contract).
        book.episodes = book.episodes.max(plan.episode.saturating_add(1));
    }

    Ok(RecoveredMemoryLane { lane, book })
}

/// Install a recovered lane into a fresh world AND materialize the
/// requested tiers (the sign-off review's finding 21 corrective).
/// The recovered lane starts all-dormant (the session-reopen shape
/// — the materialization is the [`MemoryLane::activate`] rebuild,
/// never a silent partial live view), but the fresh world's
/// requested levels are NOT all dormant, and
/// [`set_region_level`]'s same-level early return would leave the
/// mismatch standing: a recovered witness's region says Full while
/// its memory stays dormant, so captures reach its ledger without
/// ever materializing the live view — until some unrelated tier
/// transition happens to fire. The install materializes every
/// active region's witnesses through the same activation path a
/// Dormant→active transition takes (the budget-consistent rebuild,
/// byte-identical to an uninterrupted session's).
pub fn install_recovered_lane(
    spine: &mut Spine<MemoryLaneWorld>,
    recovered: RecoveredMemoryLane,
) -> MemoryCommitBook {
    let RecoveredMemoryLane { lane, book } = recovered;
    spine.world_mut().lane = lane;
    for region in 0..MEMORY_REGIONS {
        let level = spine.world().levels[region];
        if level != LodLevel::Dormant {
            let budget = perception_budget(tier_level(level));
            for record in POPULATION
                .iter()
                .take((region + 1) * REGION_NPCS)
                .skip(region * REGION_NPCS)
            {
                spine.world_mut().lane.activate(record.id, budget).expect(
                    "the rebuild's checked accumulation cannot overflow at authored deltas",
                );
            }
        }
    }
    book
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
