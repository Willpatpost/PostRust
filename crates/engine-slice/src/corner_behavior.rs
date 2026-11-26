//! The composed-crowd policy module (Era 2 package A part 5 — the composed
//! gates, `docs/ERA2-NPC-AI-DESIGN-PASS.md` §8.4's allocation-counting gate):
//! the 24 NPC schedules, the spine world, the policy seam, and the composed
//! tick — ALL pure, fully tested, no I/O, no Vulkan, no new dependencies.
//!
//! # DETERMINISM CONTRACT
//! Every answer this module produces is byte-identical on every platform:
//! schedules are pure authored data (fixed tick values, integer cadences,
//! fixed activity kinds and targets), the world's RNG stream follows the
//! per-NPC seed family (part 4's `seed_for`), the execution layer's
//! wake/witness evaluators are pure, and the digest uses FNV-1a over
//! little-endian fixed-width fields. A fixed campaign over a fixed schedule
//! produces the same activity stream forever — pinned by digest.

use crate::corner_routes::corner_routes;
use engine_core::clock::Tick;
use engine_core::event::{Event, EventClass, Field};
use engine_core::npc_behavior::{ActivityKind, MAX_NEEDS, NpcId, NpcRng, Rule};
use engine_core::npc_execution::{StreamEntry, evaluate_wake, wake_payload, wake_payload_parts};
use engine_core::npc_locomotion::{LOOPING, NpcLocomotion, Route, set_destination};
use engine_core::npc_schedule::{ActivityDescriptor, Cadence, Commitment, NpcSchedule, Target};
use engine_core::spine::{ExecCtx, World};

// ------------------------------------------------------- policy constants

/// The demo's composed-wake class (0xC5). The engine-core test fixture uses
/// 0xC4 — class bytes are deployment-local policy.
pub const WAKE_CLASS: EventClass = EventClass::new(0xC5);

// ------------------------------------------------------- the schedules

/// One NPC's appearance/locomotion seed from its index (NPC_TABLE order).
/// Waiter i → 100+i, crosser 8/9 → 200/201, stroller 10+k → 300+k.
/// This matches part 4's `start_route` seed stream inside
/// `corner_population_locomotion`.
fn seed_for(npc: u32) -> u32 {
    if npc < 8 {
        // Waiters 0-7: appearance_seed 100+i
        100 + npc
    } else if npc < 10 {
        // Crossers 8-9: appearance_seed 200/201
        200 + (npc - 8)
    } else {
        // Strollers 10-23: appearance_seed 300+k where k = npc-10
        300 + (npc - 10)
    }
}

/// The 24 NPC schedules, one per NPC in NPC_TABLE order (waiters 0-7,
/// crossers 8-9, strollers 10-23).
///
/// WAITERS 0-7 (standing, byte-frozen locomotion): ONE entry.
///   start: Tick(120 + 40*i) (i = the waiter's index: 120, 160, ..., 400)
///   cadence: every(360)
///   activity: kind 100 (waiter idle re-arm — NO locomotion retarget)
///   target: { npc: i, anchor: (0, 0) }
///
/// CROSSERS 8-9 (the full crossings, routes 8/9): TWO entries each (the crossing alternation).
///   Crosser 8: entry 0 start=90, entry 1 start=510, BOTH Cadence::every(840)
///   Crosser 9: entry 0 start=90, entry 1 start=510, BOTH Cadence::every(840)
///   Both entries: kind 200/201, Field::Near, target npc.
///   The seam maps kind 200→waypoint 2 (the far curb — across the road),
///   kind 201→waypoint 1 (the near curb — back).
///
/// STROLLERS 10-23 (the flank loops, routes 0-7): TWO entries each (the stroll/rest pair).
///   Let k = i - 10 (0..13)
///   Entry 0 (stroll): start=60+17*k, cadence every(240), kind 300
///   Entry 1 (rest): start=60+17*k+120, cadence every(240), kind 301
///   The seam maps kind 300→LOOPING (resume from current leg_distance),
///   kind 301→W_k where W_k=1 for even k, 3 for odd k (the rest waypoints).
pub fn corner_schedules() -> Vec<NpcSchedule> {
    let mut schedules = Vec::with_capacity(24);

    // -- Waiters 0-7
    for i in 0..8 {
        let start = Tick(120 + 40 * i as u64);
        let schedule = NpcSchedule {
            entries: vec![Commitment {
                start,
                cadence: Cadence::every(360).expect("period 360 is nonzero"),
                activity: ActivityDescriptor {
                    kind: 100,
                    field: Field::Near,
                },
                target: Target {
                    npc: i as u32,
                    anchor: (0, 0),
                },
            }],
        };
        schedules.push(schedule);
    }

    // -- Crossers 8-9
    for npc in [8u32, 9u32] {
        let schedule = NpcSchedule {
            entries: vec![
                Commitment {
                    start: Tick(90),
                    cadence: Cadence::every(840).expect("period 840 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 200,
                        field: Field::Near,
                    },
                    target: Target {
                        npc,
                        anchor: (0, 0),
                    },
                },
                Commitment {
                    start: Tick(510),
                    cadence: Cadence::every(840).expect("period 840 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 201,
                        field: Field::Near,
                    },
                    target: Target {
                        npc,
                        anchor: (0, 0),
                    },
                },
            ],
        };
        schedules.push(schedule);
    }

    // -- Strollers 10-23
    for i in 10..24 {
        let k = (i - 10) as u64;
        let stroll_start = Tick(60 + 17 * k);
        let rest_start = Tick(60 + 17 * k + 120);
        let schedule = NpcSchedule {
            entries: vec![
                Commitment {
                    start: stroll_start,
                    cadence: Cadence::every(240).expect("period 240 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 300,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: i as u32,
                        anchor: (0, 0),
                    },
                },
                Commitment {
                    start: rest_start,
                    cadence: Cadence::every(240).expect("period 240 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 301,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: i as u32,
                        anchor: (0, 0),
                    },
                },
            ],
        };
        schedules.push(schedule);
    }

    schedules
}

// ------------------------------------------------------- the policy seam

/// The commitment → locomotion retarget (§7's game-side seam, the
/// campaign's POLICY comment made into the demo's authored table):
/// waiters (kind 100) never retarget — byte-frozen; crossers: kind
/// 200 → the far curb (waypoint 2, across the road), 201 → the near curb
/// (waypoint 1, back); strollers: kind 300 → LOOPING (the resume — no
/// redraw, no phase reset, continuing from the current leg_distance),
/// 301 → the rest waypoint W_k (1 for even k, 3 for odd — the pauses
/// spread over both lanes). Returns the `set_destination` outcome: false
/// is the authored refusal, NEVER expected on this table — the world
/// counts it and the campaign gate asserts zero.
fn seam_retarget(
    npc: u32,
    commitment_kind: u16,
    state: &mut NpcLocomotion,
    routes: &[Route],
) -> bool {
    if npc < 8 {
        // The waiters stand — world-placed policy, never retargeted.
        return true;
    }
    let Some(route) = routes.get(state.route as usize) else {
        return false;
    };
    match commitment_kind {
        200 => set_destination(route, state, 2),
        201 => set_destination(route, state, 1),
        300 => set_destination(route, state, LOOPING),
        301 => {
            let k = npc - 10;
            let rest_waypoint = if k.is_multiple_of(2) { 1 } else { 3 };
            set_destination(route, state, rest_waypoint)
        }
        // The waiters' kind 100 (and any unknown kind): no retarget.
        _ => true,
    }
}

// ------------------------------------------------------- the spine world

/// The behavior-stream recording mode (the audit's finding 5
/// correction). `Full` retains every entry in `stream` — the
/// fixtures that pin the complete stream byte-for-byte and hash it
/// with `execution_digest`. `Rolling` retains NOTHING per entry: the
/// rolling FNV-1a digest and the counters carry the record, so a
/// session that outlives any reservation (the running demo) grows
/// nothing — the old wake log pushed one entry per wake into a
/// 256-reserved Vec forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    /// Retain every entry (the fixture mode: the complete stream is
    /// the pinned record).
    Full,
    /// Retain nothing; fold each entry into the rolling digest (the
    /// running demo's bounded record).
    Rolling,
}

/// Folds ONE stream entry into a running FNV-1a hash — EXACTLY
/// `execution_digest`'s per-entry byte sequence (tick u64 LE, npc
/// u32 LE, rule u32 LE, kind tag byte, the kind's params as LE u32s
/// in decision_digest's order), starting from the FNV offset basis.
/// Folding every executed entry in order makes the rolling digest
/// EQUAL `execution_digest` over the equivalent retained stream at
/// every prefix — the fixture campaign below pins that equality, so
/// the two encodings cannot drift apart.
fn fold_stream_entry(mut hash: u64, entry: &StreamEntry) -> u64 {
    let mut fold = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    fold(&entry.tick.as_u64().to_le_bytes());
    fold(&entry.npc.to_le_bytes());
    fold(&entry.activity.rule.to_le_bytes());
    let tag = match entry.activity.kind {
        ActivityKind::Idle => 0,
        ActivityKind::MoveTo { .. } => 1,
        ActivityKind::Fulfill { .. } => 2,
        ActivityKind::React { .. } => 3,
    };
    fold(&[tag]);
    match entry.activity.kind {
        ActivityKind::Idle => {}
        ActivityKind::MoveTo { route, waypoint } => {
            fold(&route.to_le_bytes());
            fold(&waypoint.to_le_bytes());
        }
        ActivityKind::Fulfill { commitment } => {
            fold(&commitment.to_le_bytes());
        }
        ActivityKind::React { class, subject } => {
            fold(&class.to_le_bytes());
            fold(&subject.0.to_le_bytes());
        }
    }
    hash
}

/// The composed crowd's spine world: the 24 schedules/routines/rngs
/// (one per NPC), the locomotion states and present mask from part
/// 4's constructor, and the behavior stream (PRE-ALLOCATED in the
/// Full fixture mode — the composed per-tick path allocates nothing;
/// in the Rolling demo mode nothing is retained at all). `execute`
/// IS the policy seam: a wake event evaluates (schedule authority)
/// and retargets the walker.
pub struct CornerCrowdWorld {
    /// The 24 schedules (one per NPC).
    pub schedules: Vec<NpcSchedule>,
    /// The 24 routines (one per NPC).
    pub routines: Vec<engine_core::npc_behavior::NpcRoutine>,
    /// The 24 RNGs (one per NPC), seeded from the same stream as part 4.
    pub rngs: Vec<NpcRng>,
    /// Last decay tick per NPC.
    pub last_decay_tick: Vec<Tick>,
    /// The shared rule table: one idle-baseline rule.
    pub rules: Vec<Rule>,
    /// Need decay rates.
    pub rates: [f32; MAX_NEEDS],
    /// The behavior stream (Full mode only; the capacity bound is
    /// documented in the constructors).
    pub stream: Vec<StreamEntry>,
    /// The stream recording mode (finding 5's correction).
    pub stream_mode: StreamMode,
    /// The rolling stream digest: FNV-1a folded per executed entry
    /// (both modes — the fixture cross-check pins it against
    /// `execution_digest` over the retained stream).
    pub stream_digest: u64,
    /// The number of entries folded into `stream_digest`.
    pub stream_entries: u64,
    /// Per-NPC locomotion states from part 4.
    pub states: Vec<NpcLocomotion>,
    /// Per-NPC present mask.
    pub present: Vec<bool>,
    /// The route table (cloned once at construction).
    pub routes: Vec<Route>,
    /// Wake execution counter.
    pub wake_executions: u64,
    /// The seam's authored refusals (NEVER expected — the campaign
    /// gate asserts zero; a nonzero count is an authoring defect).
    pub retarget_refusals: u64,
    /// Arrival counter (from batch summary).
    pub arrivals: u64,
    /// Stepped counter (from batch summary).
    pub stepped: u64,
    /// Crossing-guard settlements (from batch summary; the authored
    /// table's ordinary trajectories never fire it — nonzero is an
    /// authoring defect, counted fail-visibly).
    pub guard_settles: u64,
    /// Stride-phase clamps (from batch summary; same discipline as
    /// `guard_settles`).
    pub phase_clamps: u64,
}

impl CornerCrowdWorld {
    /// The composed crowd constructor, FIXTURE mode: the stream
    /// retains every entry (capacity 256 — 663 ticks × the cadence
    /// arithmetic give ≈ 70 entries total — waiter 2-3 fires each,
    /// crossers ~11 each, strollers ~5-6 each ≈ 24 + 22 + 76 ≈ 122;
    /// 256 carries 2× headroom; the fixtures that run past it pay
    /// the Vec growth, which is the fixture's business — never the
    /// demo's per-frame path).
    pub fn new(states: Vec<NpcLocomotion>, present: Vec<bool>) -> Self {
        Self::build(states, present, StreamMode::Full)
    }

    /// The running-demo constructor (finding 5's correction): the
    /// same schedules, routines, rngs, states, and routes, with the
    /// behavior stream in the Rolling mode — no per-entry retention,
    /// no reservation. A session that outgrows any reservation grows
    /// nothing; the rolling digest and the counters carry the record.
    pub fn new_rolling(states: Vec<NpcLocomotion>, present: Vec<bool>) -> Self {
        Self::build(states, present, StreamMode::Rolling)
    }

    fn build(states: Vec<NpcLocomotion>, present: Vec<bool>, mode: StreamMode) -> CornerCrowdWorld {
        let schedules = corner_schedules();
        let routines = vec![
            engine_core::npc_behavior::NpcRoutine {
                current: engine_core::npc_behavior::Activity {
                    kind: ActivityKind::Idle,
                    rule: u32::MAX,
                },
                needs: [1.0; MAX_NEEDS],
                route_progress: 0,
            };
            24
        ];
        let rngs = (0..24)
            .map(|i| NpcRng::for_npc(NpcId(seed_for(i as u32))))
            .collect();
        let last_decay_tick = vec![Tick(0); 24];
        let rules = vec![Rule {
            activity: ActivityKind::Idle,
            need: None,
            base_priority: 0,
            applies_below: 0.0,
            witness: None,
        }];
        let rates = [0.001; MAX_NEEDS];
        let stream = if mode == StreamMode::Full {
            Vec::with_capacity(256)
        } else {
            Vec::new()
        };
        let routes = corner_routes();
        CornerCrowdWorld {
            schedules,
            routines,
            rngs,
            last_decay_tick,
            rules,
            rates,
            stream,
            stream_mode: mode,
            stream_digest: 0xcbf2_9ce4_8422_2325,
            stream_entries: 0,
            states,
            present,
            routes,
            wake_executions: 0,
            retarget_refusals: 0,
            arrivals: 0,
            stepped: 0,
            guard_settles: 0,
            phase_clamps: 0,
        }
    }

    /// One composed tick: SPINE FIRST (the wake may retarget
    /// mid-tick), then the batch, then the arrival mirror. The demo
    /// drives this and reads the states out for the render.
    ///
    /// The composed step's allocation profile:
    /// - `advance_batch` is the existing engine-core path (no alloc)
    /// - the mirror is field writes only
    /// - `corner_instances_into` reuses its output buffer
    /// - `evaluate_population_poses_locomotion` reuses its output buffer
    /// - The stream record is construction-time only: the Full
    ///   fixture mode's 256-entry reservation, or the Rolling demo
    ///   mode's nothing-at-all (finding 5's correction — the wake
    ///   log can no longer grow past any boundary)
    pub fn composed_tick(&mut self, dt: f32) -> engine_core::npc_locomotion::BatchSummary {
        // The batch step (the demo's dt = 1/60 — the same deterministic
        // clock as part 4).
        let summary = engine_core::npc_locomotion::advance_batch(
            &self.routes,
            &mut self.states,
            &self.present,
            dt,
            None,
            0.3,
            1.75,
        );
        self.stepped += u64::from(summary.stepped);
        self.arrivals += u64::from(summary.arrivals);
        // The pathological-tick counters, fail-visible (both stay
        // zero on the authored table — nonzero is an authoring
        // defect the demo's exit summary reports).
        self.guard_settles += u64::from(summary.guard_settles);
        self.phase_clamps += u64::from(summary.phase_clamps);

        // THE ARRIVAL MIRROR (idempotent at stands): any present WALKER
        // (npc 8-23 — the waiters are world-placed policy whose
        // route_progress is inert) standing at a waypoint (speed == 0.0 —
        // a retarget's destination reached, or the already-there case)
        // carries the stood waypoint into its committed location. The
        // write is idempotent: once mirrored, leg == route_progress and
        // no further tick writes; a walker mid-stride (speed > 0) never
        // mirrors — the committed location stays coarse until the next
        // stand (part 3's position bridge).
        for i in 8..24 {
            if self.present[i]
                && self.states[i].speed == 0.0
                && self.routines[i].route_progress != self.states[i].leg
            {
                self.routines[i].route_progress = self.states[i].leg;
            }
        }

        summary
    }

    /// THE VERSIONED DIAGNOSTIC DIGEST (finding 7's correction — a
    /// diagnostic BESIDE the compatibility `fingerprint` the World
    /// trait carries below, never a replacement: the spine's
    /// fingerprint contract is untouched, and every existing pin
    /// over it stands). The compatibility fingerprint folds the
    /// kind DISCRIMINANT only and omits needs, RNG position, decay
    /// anchors, and locomotion — two worlds fulfilling different
    /// commitments could hash alike at that boundary. This digest
    /// folds the complete future-behavior state, versioned like
    /// R1's journal (a future field list is a new version tag,
    /// never a silent move): per NPC in order — the current
    /// activity WITH its variant payload (route/waypoint,
    /// commitment, class+subject), every need's bits,
    /// route_progress, the RNG stream position, the decay anchor,
    /// and the full locomotion state (position, facing, route, leg,
    /// leg_distance, destination, speed, speed_scale, lateral,
    /// stride_phase) — then the present mask and the counters.
    /// Pure, fixed order, read-only.
    pub fn diagnostic_digest(&self) -> u64 {
        let mut fp = engine_core::fingerprint::Fingerprinter::new();
        fp.write_str("engine-slice/corner-behavior-diagnostic/1");
        for i in 0..24 {
            fp.write_u32(self.routines[i].current.rule);
            match self.routines[i].current.kind {
                ActivityKind::Idle => fp.write_u8(0),
                ActivityKind::MoveTo { route, waypoint } => {
                    fp.write_u8(1);
                    fp.write_u32(route);
                    fp.write_u32(waypoint);
                }
                ActivityKind::Fulfill { commitment } => {
                    fp.write_u8(2);
                    fp.write_u32(commitment);
                }
                ActivityKind::React { class, subject } => {
                    fp.write_u8(3);
                    fp.write_u32(class);
                    fp.write_u32(subject.0);
                }
            }
            for need in self.routines[i].needs {
                fp.write_u32(need.to_bits());
            }
            fp.write_u32(self.routines[i].route_progress);
            fp.write_u64(self.rngs[i].state_bits());
            fp.write_tick(self.last_decay_tick[i]);
            let state = self.states[i];
            fp.write_u32(state.position[0].to_bits());
            fp.write_u32(state.position[1].to_bits());
            fp.write_u32(u32::from(state.facing));
            fp.write_u32(state.route);
            fp.write_u32(state.leg);
            fp.write_u32(state.leg_distance.to_bits());
            fp.write_u32(state.destination);
            fp.write_u32(state.speed.to_bits());
            fp.write_u32(state.speed_scale.to_bits());
            fp.write_u32(state.lateral.to_bits());
            fp.write_u32(state.stride_phase.to_bits());
            fp.write_u8(u8::from(self.present[i]));
        }
        fp.write_u64(self.wake_executions);
        fp.write_u64(self.arrivals);
        fp.write_u64(self.stepped);
        fp.finish()
    }
}

impl World for CornerCrowdWorld {
    fn execute(&mut self, event: &Event, ctx: &mut ExecCtx) {
        // Ignore non-wake classes.
        if event.class != WAKE_CLASS {
            return;
        }

        let Some((npc, commitment)) = wake_payload_parts(event.payload) else {
            return;
        };

        // Out-of-range NPC or commitment.
        if npc >= 24 {
            return;
        }
        let npc_idx = npc as usize;
        if commitment as usize >= self.schedules[npc_idx].entries.len() {
            return;
        }

        // Copy-in/copy-out (NpcRoutine and NpcRng are Copy) so the
        // borrowed schedule and the state never alias.
        let mut rng = self.rngs[npc_idx];
        let routine = self.routines[npc_idx];
        let Some(outcome) = evaluate_wake(
            &self.schedules[npc_idx],
            commitment,
            ctx.executed_tick(),
            self.last_decay_tick[npc_idx],
            &routine,
            &self.rules,
            &mut rng,
            &self.rates,
        ) else {
            return;
        };

        self.routines[npc_idx] = outcome.routine_after;
        self.rngs[npc_idx] = rng;
        self.last_decay_tick[npc_idx] = ctx.executed_tick();
        // THE STREAM RECORD (finding 5's correction): every mode
        // folds the entry into the rolling digest (the fixture
        // cross-check pins the fold against `execution_digest` over
        // the retained stream); only the Full fixture mode retains
        // the entry itself — the Rolling demo mode keeps nothing per
        // entry, so the log cannot grow past any reservation.
        let entry = StreamEntry {
            tick: ctx.executed_tick(),
            npc,
            activity: outcome.activity,
        };
        self.stream_digest = fold_stream_entry(self.stream_digest, &entry);
        self.stream_entries += 1;
        if self.stream_mode == StreamMode::Full {
            self.stream.push(entry);
        }
        self.wake_executions += 1;

        // THE SEAM (the authored table above): the commitment's kind
        // drives the retarget; a refusal is counted, never silent.
        let commitment_kind = self.schedules[npc_idx].entries[commitment as usize]
            .activity
            .kind;
        if !seam_retarget(
            npc,
            commitment_kind,
            &mut self.states[npc_idx],
            &self.routes,
        ) {
            self.retarget_refusals += 1;
        }

        // THE HONEST RE-ARM: the same commitment index re-arms.
        if let Some(next) = outcome.next_wake {
            let payload = wake_payload(npc, commitment).expect("the authored index round-trips");
            ctx.schedule_at(next, WAKE_CLASS, payload, Vec::new());
        }
    }

    /// The canonical logical record: per NPC in order, the routine
    /// lane (current kind discriminant encoding, route_progress), then
    /// the counters (wake_executions, arrivals, stepped). The
    /// locomotion states are NOT in this fingerprint (the locomotion
    /// digest owns them — the separate pin).
    fn fingerprint(&self, fp: &mut engine_core::fingerprint::Fingerprinter) {
        fp.write_str("engine-slice/corner-behavior-world/1");
        for i in 0..24 {
            // routine.current.kind's discriminant-safe encoding (follow
            // LocoWorld's fingerprint shape at npc_locomotion.rs lines
            // ~2095-2112).
            fp.write_u32(self.routines[i].current.rule);
            fp.write_u32(match self.routines[i].current.kind {
                ActivityKind::Idle => 0,
                ActivityKind::MoveTo { .. } => 1,
                ActivityKind::Fulfill { .. } => 2,
                ActivityKind::React { .. } => 3,
            });
            fp.write_u32(self.routines[i].route_progress);
        }
        fp.write_u64(self.wake_executions);
        fp.write_u64(self.retarget_refusals);
        fp.write_u64(self.arrivals);
        fp.write_u64(self.stepped);
    }
}

// ------------------------------------------------------- the lib tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corner_routes::corner_population_locomotion;
    use engine_core::npc_execution::execution_digest;
    use engine_core::npc_locomotion::LocomotionSample;
    use engine_core::npc_locomotion::{locomotion_digest, start_route};
    use engine_core::spine::Spine;

    /// Arms every schedule entry at its own first occurrence (the 40
    /// initial events: waiters 8, crossers 4, strollers 28) — the
    /// corrected rule; each entry then self-re-arms at its period.
    fn arm_all_entries(spine: &mut Spine<CornerCrowdWorld>) {
        let world = spine.world();
        let mut specs: Vec<engine_core::event::EventSpec> = Vec::new();
        for npc in 0..24 {
            let schedule = &world.schedules[npc];
            for (entry_idx, entry) in schedule.entries.iter().enumerate() {
                let payload =
                    wake_payload(npc as u32, entry_idx as u32).expect("the entry index packs");
                specs.push(engine_core::event::EventSpec {
                    class: WAKE_CLASS,
                    schedule: engine_core::event::Schedule::AtLogicalTime { due: entry.start },
                    payload,
                    field: entry.activity.field,
                    depends_on: Vec::new(),
                });
            }
        }
        for spec in specs {
            spine.schedule(spec).expect("arming is legal");
        }
    }

    /// The campaign's terminal readout (frame `ticks`).
    struct CampaignSnapshot {
        states: Vec<NpcLocomotion>,
        stream: Vec<StreamEntry>,
        stream_digest: u64,
        stream_entries: u64,
        routines: Vec<engine_core::npc_behavior::NpcRoutine>,
        wake_executions: u64,
        retarget_refusals: u64,
        arrivals: u64,
        guard_settles: u64,
        phase_clamps: u64,
        fingerprint: u64,
        /// The versioned diagnostic digest (finding 7) — beside the
        /// compatibility fingerprint, never part of it.
        diagnostic: u64,
    }

    /// Runs the composed campaign for `ticks` from a fresh double
    /// construction; returns the terminal readout (frame `ticks`).
    fn run_campaign(ticks: u64) -> CampaignSnapshot {
        let (_, states, present) = corner_population_locomotion();
        run_world_campaign(CornerCrowdWorld::new(states, present), ticks)
    }

    /// The same campaign in the ROLLING stream mode (the demo's
    /// construction since finding 5's correction) — identical
    /// schedules, wakes, and states; the behavior stream is folded,
    /// not retained.
    fn run_campaign_rolling(ticks: u64) -> CampaignSnapshot {
        let (_, states, present) = corner_population_locomotion();
        run_world_campaign(CornerCrowdWorld::new_rolling(states, present), ticks)
    }

    /// The shared driver: arm every entry, run `ticks` composed
    /// ticks, read the world out through the spine.
    fn run_world_campaign(world: CornerCrowdWorld, ticks: u64) -> CampaignSnapshot {
        let mut spine = Spine::with_reservation(world, 64, 32);
        arm_all_entries(&mut spine);
        for _ in 0..ticks {
            spine.run_tick();
            spine.world_mut().composed_tick(1.0 / 60.0);
        }
        let world = spine.world();
        let mut fp = engine_core::fingerprint::Fingerprinter::new();
        world.fingerprint(&mut fp);
        CampaignSnapshot {
            states: world.states.clone(),
            stream: world.stream.clone(),
            stream_digest: world.stream_digest,
            stream_entries: world.stream_entries,
            routines: world.routines.clone(),
            wake_executions: world.wake_executions,
            retarget_refusals: world.retarget_refusals,
            arrivals: world.arrivals,
            guard_settles: world.guard_settles,
            phase_clamps: world.phase_clamps,
            fingerprint: fp.finish(),
            diagnostic: world.diagnostic_digest(),
        }
    }

    /// THE SCHEDULES ARE PINNED: 24 schedules; the entry counts
    /// (1/2/2 per role), the starts/cadences/kinds spot-pinned.
    #[test]
    fn schedules_are_pinned() {
        let schedules = corner_schedules();
        assert_eq!(schedules.len(), 24, "24 schedules");

        // Waiters 0-7: 1 entry each (start 120+40i, every 360, kind 100).
        for (i, s) in schedules.iter().enumerate().take(8) {
            assert_eq!(s.entries.len(), 1, "waiter {i} has 1 entry");
            let e = &s.entries[0];
            assert_eq!(e.start.as_u64(), 120 + 40 * i as u64, "waiter {i} start");
            assert_eq!(
                e.cadence,
                Cadence::every(360).unwrap(),
                "waiter {i} cadence"
            );
            assert_eq!(e.activity.kind, 100, "waiter {i} kind");
            assert_eq!(e.target.npc, i as u32, "waiter {i} target npc");
            assert_eq!(
                e.activity.field,
                Field::Near,
                "waiter {i} rides the near lane"
            );
        }

        // Crosser 8: 2 entries (90/510, kinds 200/201, every 840).
        let s = &schedules[8];
        assert_eq!(s.entries.len(), 2, "crosser 8 has 2 entries");
        assert_eq!(s.entries[0].start.as_u64(), 90, "crosser 8 entry 0 start");
        assert_eq!(s.entries[0].activity.kind, 200, "crosser 8 entry 0 kind");
        assert_eq!(s.entries[1].start.as_u64(), 510, "crosser 8 entry 1 start");
        assert_eq!(s.entries[1].activity.kind, 201, "crosser 8 entry 1 kind");
        assert_eq!(
            s.entries[0].cadence,
            Cadence::every(840).unwrap(),
            "crosser 8 cadence"
        );

        // Crosser 9: 2 entries (90/510).
        let s = &schedules[9];
        assert_eq!(s.entries.len(), 2, "crosser 9 has 2 entries");
        assert_eq!(s.entries[0].start.as_u64(), 90, "crosser 9 entry 0 start");
        assert_eq!(s.entries[0].activity.kind, 200, "crosser 9 entry 0 kind");
        assert_eq!(s.entries[1].start.as_u64(), 510, "crosser 9 entry 1 start");
        assert_eq!(s.entries[1].activity.kind, 201, "crosser 9 entry 1 kind");

        // Stroller 10 (k=0): entries at 60 and 180, kinds 300/301, every 240.
        let s = &schedules[10];
        assert_eq!(s.entries.len(), 2, "stroller 10 has 2 entries");
        assert_eq!(s.entries[0].start.as_u64(), 60, "stroller 10 entry 0 start");
        assert_eq!(s.entries[0].activity.kind, 300, "stroller 10 entry 0 kind");
        assert_eq!(
            s.entries[1].start.as_u64(),
            180,
            "stroller 10 entry 1 start"
        );
        assert_eq!(s.entries[1].activity.kind, 301, "stroller 10 entry 1 kind");
        assert_eq!(
            s.entries[0].cadence,
            Cadence::every(240).unwrap(),
            "stroller 10 cadence"
        );

        // Stroller 23 (k=13): entries at 60+17·13 = 281 and 401.
        let s = &schedules[23];
        assert_eq!(s.entries.len(), 2, "stroller 23 has 2 entries");
        assert_eq!(
            s.entries[0].start.as_u64(),
            60 + 17 * 13,
            "stroller 23 entry 0 start"
        );
        assert_eq!(s.entries[0].activity.kind, 300, "stroller 23 entry 0 kind");
        assert_eq!(
            s.entries[1].start.as_u64(),
            60 + 17 * 13 + 120,
            "stroller 23 entry 1 start"
        );
        assert_eq!(s.entries[1].activity.kind, 301, "stroller 23 entry 1 kind");
    }

    /// THE COMPOSED CAMPAIGN (§8.4's gate, the package's core pin): a
    /// double-constructed composed crowd run 663 ticks (the demo's full
    /// window) is byte-identical in the locomotion states, the behavior
    /// stream, and the world's fingerprint; the digests pin at sampled
    /// ticks; the wake-execution count matches the CADENCE ARITHMETIC
    /// (the hand-derived table below); the seam NEVER refuses; and the
    /// waiters stay byte-frozen (their locomotion is world-placed
    /// policy the spine never touches).
    ///
    /// THE WAKE-COUNT DERIVATION (hand-computed, 663 ticks = due at
    /// 0..=662; a wake due exactly at 662 executes):
    /// - waiters: entry start 120+40i, period 360 → waiters 0-4 fire
    ///   twice (120/480, 160/520, 200/560, 240/600, 280/640), waiters
    ///   5-7 once (320, 360, 400) = 13;
    /// - crossers 8-9: entry 0 at 90+840k (only 90), entry 1 at 510+840k
    ///   (only 510) = 4 (2 each);
    /// - strollers: entry 0 at 60+17k+240m, entry 1 at +120 — npc 10
    ///   fires 6 (60,300,540 + 180,420,660), npcs 11-17 fire 5 each,
    ///   npcs 18-23 fire 4 each = 6 + 35 + 24 = 65.
    ///
    /// TOTAL 13 + 4 + 65 = 82 wake executions.
    ///
    /// The arrival count's crosser portion is derived: the first
    /// crossing is under way from frame 0 (the retarget at 90
    /// re-affirms it), and L0 + L1 (2.2 + 7.4 m) at the measured draws
    /// (1.3704/1.3231 m/s) lands each crosser at its far curb at
    /// ~420/~435 — INSIDE the window, one arrival each; the return
    /// crossings (retargets at 510) arrive at ~835/~846, outside — so
    /// the crossers contribute exactly 2 arrivals. The stroller
    /// portion depends on where the rests find them (a rest may be
    /// interrupted by the next stroll before reaching W) — the total
    /// is a MEASURED pin (the placeholder-pin discipline: 9 at this
    /// tree = the crossers' 2 + the strollers' 7).
    #[test]
    fn composed_campaign_double_run() {
        let a = run_campaign(663);
        let b = run_campaign(663);

        // THE DOUBLE-RUN: byte-identical states, stream, fingerprint.
        assert_eq!(a.states, b.states, "the states are byte-identical");
        assert_eq!(a.stream, b.stream, "the stream is byte-identical");
        assert_eq!(
            a.fingerprint, b.fingerprint,
            "the world fingerprint is byte-identical"
        );
        // ... and the versioned diagnostic digest (finding 7) beside
        // it: the complete future-behavior state is byte-identical
        // too.
        assert_eq!(
            a.diagnostic, b.diagnostic,
            "the diagnostic digest is byte-identical"
        );
        // THE ROLLING-FOLD CROSS-CHECK (finding 5's correction): the
        // digest the demo mode carries equals execution_digest over
        // the retained stream — the two encodings cannot drift.
        assert_eq!(
            a.stream_digest,
            execution_digest(&a.stream),
            "the rolling fold equals execution_digest over the retained stream"
        );
        assert_eq!(a.stream_entries, a.stream.len() as u64);

        // THE COUNTER PINS (the derivation above; the arrivals measured).
        assert_eq!(
            a.wake_executions, 82,
            "the wake count matches the cadence arithmetic"
        );
        assert_eq!(a.retarget_refusals, 0, "the seam never refuses");
        assert_eq!(a.arrivals, 9, "the measured arrival count");

        // THE DIGEST PINS: the locomotion digest over the 24 states at
        // sampled ticks and the execution digest over the stream.
        let digest_at = |tick: u64| -> (u64, u64) {
            let snap = run_campaign(tick);
            let samples: Vec<LocomotionSample> = snap
                .states
                .iter()
                .enumerate()
                .map(|(i, s)| LocomotionSample {
                    npc: i as u32,
                    position: s.position,
                    facing: s.facing,
                    stride_phase: s.stride_phase,
                    leg: s.leg,
                })
                .collect();
            (locomotion_digest(&samples), execution_digest(&snap.stream))
        };
        let (loco_0, exec_0) = digest_at(0);
        let (loco_33, exec_33) = digest_at(33);
        let (loco_300, exec_300) = digest_at(300);
        let (loco_662, exec_662) = digest_at(662);
        println!(
            "composed digests: loco 0/33/300/662 = {loco_0}/{loco_33}/{loco_300}/{loco_662}; \
             exec 0/33/300/662 = {exec_0}/{exec_33}/{exec_300}/{exec_662}"
        );
        // The MEASURED pins (the consts' doc carries the derivation).
        assert_eq!(loco_0, PIN_LOCO_0, "frame 0's locomotion digest moved");
        assert_eq!(loco_33, PIN_LOCO_33, "frame 33's locomotion digest moved");
        assert_eq!(
            loco_300, PIN_LOCO_300,
            "frame 300's locomotion digest moved"
        );
        assert_eq!(
            loco_662, PIN_LOCO_662,
            "frame 662's locomotion digest moved"
        );
        assert_eq!(exec_0, PIN_EXEC_0, "frame 0's execution digest moved");
        assert_eq!(exec_33, PIN_EXEC_33, "frame 33's execution digest moved");
        assert_eq!(exec_300, PIN_EXEC_300, "frame 300's execution digest moved");
        assert_eq!(exec_662, PIN_EXEC_662, "frame 662's execution digest moved");

        // THE WAITERS STAY BYTE-FROZEN (the seam never retargets them —
        // the static gates' waiter pixels hold through the composed mode).
        let (_, construction_states, _) = corner_population_locomotion();
        for (i, construction) in construction_states.iter().enumerate().take(8) {
            assert_eq!(
                a.states[i], *construction,
                "waiter {i} is byte-frozen across the composed campaign"
            );
        }
    }

    /// The measured-actual digest pins (the placeholder-pin discipline:
    /// the first run measures, the pins follow, any later movement is
    /// an investigation). Frame 0's locomotion digest is part 4's own
    /// frame-0 pin (the composed mode's first frames equal the plain
    /// mode's — no wake fires before tick 90); frame 33's likewise (the
    /// retarget at tick 90 re-affirms the crossing already under way,
    /// so the composed states at tick 33 are the plain mode's own —
    /// the SAME pin value the plain campaign carries); frame 0's and
    /// frame 33's execution digests are the FNV offset basis (the
    /// empty stream — the first wake fires at tick 90).
    const PIN_LOCO_0: u64 = 5_555_165_838_415_353_039;
    const PIN_LOCO_33: u64 = 15_857_914_442_065_508_438;
    const PIN_LOCO_300: u64 = 2_194_641_527_225_081_114;
    const PIN_LOCO_662: u64 = 765_261_949_917_836_765;
    const PIN_EXEC_0: u64 = 14_695_981_039_346_656_037;
    const PIN_EXEC_33: u64 = 14_695_981_039_346_656_037;
    const PIN_EXEC_300: u64 = 17_642_371_054_831_568_059;
    const PIN_EXEC_662: u64 = 2_580_761_500_374_851_303;

    /// THE DIAGNOSTIC DIGEST'S DISCRIMINANTS (finding 7's deciding
    /// test): vary ONLY a commitment payload, a need bit, an RNG
    /// stream position, or a locomotion phase — the compatibility
    /// fingerprint (deliberately discriminant-only, preserved
    /// untouched) stays IDENTICAL while the versioned diagnostic
    /// digest distinguishes the worlds. Each perturbation is the
    /// audit's own witness shape: two worlds fulfilling different
    /// commitments hashed alike at the compatibility boundary. The
    /// perturbation happens at the TERMINAL read (after behavior is
    /// done) so exactly one field differs between the paired worlds.
    #[test]
    fn diagnostic_digest_distinguishes_what_the_fingerprint_omits() {
        fn run_then(mutate: &dyn Fn(&mut CornerCrowdWorld)) -> (u64, u64) {
            let (_, states, present) = corner_population_locomotion();
            let mut spine = Spine::with_reservation(CornerCrowdWorld::new(states, present), 64, 32);
            arm_all_entries(&mut spine);
            for _ in 0..663 {
                spine.run_tick();
                spine.world_mut().composed_tick(1.0 / 60.0);
            }
            mutate(spine.world_mut());
            let world = spine.world();
            let mut fp = engine_core::fingerprint::Fingerprinter::new();
            world.fingerprint(&mut fp);
            (fp.finish(), world.diagnostic_digest())
        }
        let no_op = |_: &mut CornerCrowdWorld| {};
        let baseline = run_then(&no_op);
        let repeat = run_then(&no_op);
        assert_eq!(
            baseline, repeat,
            "the unperturbed double-run: both digests identical"
        );

        // (a) ONLY a commitment payload: the same Fulfill
        // discriminant, a different commitment index (the audit's
        // "two worlds fulfilling different commitments").
        let payload_a = run_then(&|world: &mut CornerCrowdWorld| {
            world.routines[5].current = engine_core::npc_behavior::Activity {
                kind: ActivityKind::Fulfill { commitment: 0 },
                rule: 5,
            };
        });
        let payload_b = run_then(&|world: &mut CornerCrowdWorld| {
            world.routines[5].current = engine_core::npc_behavior::Activity {
                kind: ActivityKind::Fulfill { commitment: 1 },
                rule: 5,
            };
        });
        assert_eq!(
            payload_a.0, payload_b.0,
            "the compatibility fingerprint cannot see the payload (the audit's \
             witness — preserved untouched)"
        );
        assert_ne!(
            payload_a.1, payload_b.1,
            "the diagnostic distinguishes the commitment payload"
        );

        // (b) ONLY a need bit (the decay lane the fingerprint omits).
        let need_a = run_then(&|world: &mut CornerCrowdWorld| {
            world.routines[5].needs[2] = 0.5;
        });
        let need_b = run_then(&|world: &mut CornerCrowdWorld| {
            world.routines[5].needs[2] = 0.25;
        });
        assert_eq!(
            need_a.0, need_b.0,
            "the compatibility fingerprint cannot see the needs"
        );
        assert_ne!(
            need_a.1, need_b.1,
            "the diagnostic distinguishes the need bit"
        );

        // (c) ONLY an RNG stream position (one extra draw consumed).
        let rng_a = run_then(&no_op);
        let rng_b = run_then(&|world: &mut CornerCrowdWorld| {
            world.rngs[5].next_unit();
        });
        assert_eq!(
            rng_a.0, rng_b.0,
            "the compatibility fingerprint cannot see the RNG position"
        );
        assert_ne!(
            rng_a.1, rng_b.1,
            "the diagnostic distinguishes the RNG position"
        );

        // (d) ONLY a locomotion phase (the locomotion state the
        // fingerprint delegates to the separate locomotion pin).
        let phase_a = run_then(&no_op);
        let phase_b = run_then(&|world: &mut CornerCrowdWorld| {
            world.states[5].stride_phase = (world.states[5].stride_phase + 0.5) % 1.0;
        });
        assert_eq!(
            phase_a.0, phase_b.0,
            "the compatibility fingerprint cannot see the locomotion phase"
        );
        assert_ne!(
            phase_a.1, phase_b.1,
            "the diagnostic distinguishes the locomotion phase"
        );
    }

    /// THE ROLLING STREAM CAMPAIGN (the audit's finding 5 deciding
    /// test, the determinism half; the allocation half lives in the
    /// lavapipe composed-gate tests, `render_instanced.rs`): the
    /// running demo's bounded record — no per-entry retention, the
    /// rolling digest and counters only — equals the fixture mode's
    /// complete-stream execution digest, byte-identical across
    /// double construction, driven PAST the old wake log's 256-entry
    /// reservation and its 512 growth boundary with real wakes.
    ///
    /// THE WAKE-COUNT DERIVATION at 4200 ticks (hand-computed like
    /// the 663-tick table above, then MEASURED — the placeholder-pin
    /// discipline): waiters 92 (start 120+40i, every 360: waiters
    /// 0-3 fire 12× — waiter 3's 12th is due exactly at 4200 and
    /// executes — waiters 4-7 fire 11×); crossers 20 (each of the
    /// two crossers' two entries fires 5×); strollers 477 (entry 0
    /// at 60+17k+240m: k=0-3 fire 18×, k=4-13 fire 17× = 242; entry
    /// 1 at 180+17k+240m: k=0-10 fire 17×, k=11-13 fire 16× = 235)
    /// — total 589, comfortably past the old 512 boundary.
    #[test]
    fn rolling_campaign_digest_equals_full_stream_past_boundaries() {
        let full = run_campaign(4200);
        let rolling = run_campaign_rolling(4200);
        let rolling_b = run_campaign_rolling(4200);

        // Past BOTH old boundaries with real wakes (the measured
        // count; the derivation above).
        assert_eq!(
            full.wake_executions, 589,
            "the 4200-tick cadence arithmetic"
        );
        assert!(
            full.wake_executions > 512,
            "past the old 256-entry reservation AND its 512 growth point"
        );

        // THE ROLLING RECORD EQUALS THE COMPLETE STREAM: the fold is
        // execution_digest, entry for entry.
        assert_eq!(
            rolling.stream_digest, full.stream_digest,
            "the rolling digest equals the full mode's"
        );
        assert_eq!(
            rolling.stream_digest,
            execution_digest(&full.stream),
            "the rolling digest equals execution_digest over the retained stream"
        );
        assert_eq!(rolling.stream_entries, full.stream.len() as u64);
        assert_eq!(rolling.wake_executions, full.wake_executions);

        // The bounded record: nothing retained, byte-identical
        // across double construction, the trajectories themselves
        // mode-independent.
        assert!(
            rolling.stream.is_empty(),
            "the rolling mode retains no per-entry history"
        );
        assert_eq!(
            rolling.stream_digest, rolling_b.stream_digest,
            "the rolling record is byte-identical across construction"
        );
        assert_eq!(rolling.fingerprint, rolling_b.fingerprint);
        assert_eq!(rolling.states, rolling_b.states);
        assert_eq!(
            rolling.states, full.states,
            "the trajectories are mode-independent"
        );

        // The authored table never fires the pathological guards
        // (finding 4's counters stay zero on ordinary trajectories).
        assert_eq!(rolling.guard_settles, 0, "no crossing-guard settlement");
        assert_eq!(rolling.phase_clamps, 0, "no stride-phase clamp");
        assert_eq!(full.guard_settles, 0);
        assert_eq!(full.phase_clamps, 0);
        assert_eq!(rolling.retarget_refusals, 0, "the seam never refuses");
    }

    /// THE SEAM RETARGET KNOWN ANSWERS (the REAL seam — the same
    /// function `execute` drives, never a test copy).
    #[test]
    fn seam_retarget_known_answers() {
        let routes = corner_routes();

        // The crosser's alternation: standing at waypoint 0, kind 200
        // walks to the far curb (waypoint 2); after the ARRIVAL (the
        // snap-and-stand shape: leg = destination, speed = 0), kind
        // 201 walks to the near curb (waypoint 1).
        let mut state = start_route(8, &routes[8], 200, 0, 0).expect("crosser start");
        assert!(seam_retarget(8, 200, &mut state, &routes));
        assert_eq!(state.destination, 2, "kind 200 → the far curb");
        assert!(state.speed > 0.0, "kind 200 restarts the walk");
        state.leg = 2; // the arrival's snap-and-stand (advance_batch's own shape)
        state.speed = 0.0;
        assert!(seam_retarget(8, 201, &mut state, &routes));
        assert_eq!(state.destination, 1, "kind 201 → the near curb");
        assert!(state.speed > 0.0, "kind 201 restarts the walk (leg 2 → 1)");

        // The stroller's MID-LEG resume: a walking state mid-leg with a
        // forged phase, kind 300 → LOOPING keeps leg_distance and the
        // phase EXACTLY (no redraw, no reset — byte-equal continuation).
        let mut state = start_route(0, &routes[0], 300, 1, LOOPING).expect("stroller start");
        state.leg_distance = 2.5;
        state.stride_phase = 0.4;
        assert!(seam_retarget(10, 300, &mut state, &routes));
        assert_eq!(state.destination, LOOPING, "kind 300 → LOOPING");
        assert_eq!(
            state.leg_distance, 2.5,
            "the mid-leg distance is kept exactly"
        );
        assert_eq!(state.stride_phase, 0.4, "the phase is kept exactly");

        // The rest heading: kind 301 → W_k (npc 10, k=0 even → 1;
        // npc 11, k=1 odd → 3).
        let mut state = start_route(0, &routes[0], 300, 0, LOOPING).expect("stroller start");
        assert!(seam_retarget(10, 301, &mut state, &routes));
        assert_eq!(state.destination, 1, "stroller 10 rests at waypoint 1");
        let mut state = start_route(1, &routes[1], 301, 0, LOOPING).expect("stroller start");
        assert!(seam_retarget(11, 301, &mut state, &routes));
        assert_eq!(state.destination, 3, "stroller 11 rests at waypoint 3");

        // The waiter: kind 100 changes NOTHING (byte-frozen).
        let mut state = start_route(0, &routes[0], 100, 0, 0).expect("standing fixture");
        let before = state;
        assert!(seam_retarget(5, 100, &mut state, &routes));
        assert_eq!(state, before, "the waiter's kind 100 is byte-frozen");
    }

    /// THE ARRIVAL MIRROR (part 3's position bridge, composed): over
    /// the campaign, every standing walker's committed location equals
    /// its stood waypoint (the mirror is idempotent — the final world
    /// satisfies it for every walker), and the waiters' route_progress
    /// stays 0 (world-placed policy, inert).
    #[test]
    fn arrival_mirror_follows_the_rests() {
        let snap = run_campaign(663);
        // The stream is nonempty and every entry carries a real tick.
        assert!(
            !snap.stream.is_empty(),
            "the campaign produced a behavior stream"
        );
        // Every standing walker mirrors its waypoint.
        for i in 8..24 {
            if snap.states[i].speed == 0.0 {
                assert_eq!(
                    snap.routines[i].route_progress, snap.states[i].leg,
                    "standing walker {i}'s committed location mirrors its waypoint"
                );
            }
        }
        // The waiters' committed location is inert.
        for i in 0..8 {
            assert_eq!(snap.routines[i].route_progress, 0, "waiter {i} inert");
        }
        // The terminal states still satisfy the pairwise-clearance
        // discipline (the composed crowd never collides — the same
        // 0.75 walker margin part 4's battery pins).
        for i in 0..24 {
            for j in (i + 1)..24 {
                let dx = snap.states[i].position[0] - snap.states[j].position[0];
                let dz = snap.states[i].position[1] - snap.states[j].position[1];
                let distance = (dx * dx + dz * dz).sqrt();
                if snap.states[i].speed > 0.0 || snap.states[j].speed > 0.0 {
                    assert!(
                        distance >= 0.75,
                        "composed walkers {i} and {j} closed to {distance}"
                    );
                }
            }
        }
    }

    /// THE COMPOSED CROSSINGS CLEAR THE CAMERA ON THE SHARED CLOCK: the
    /// composed campaign (the spine-driven crowd, the same 663-tick
    /// window as the campaign pin) measured against the camera's
    /// same-frame position at EVERY tick — >= 0.8, the measured minimum
    /// pinned. The demo's pairing is the plain gate's own (crowd step k
    /// with camera pose clamp(k - 60, 0, 600)); the composed mode's
    /// first ~420 ticks are the plain mode's trajectory (the retarget
    /// at 90 re-affirms the crossing already under way), so the
    /// measured minimum EQUALS the plain gate's own pin — a
    /// cross-check between the two gates. The curb stands (from the
    /// far-curb arrivals ~420/~435 to the retargets at 510) hold the
    /// crossers at the far curbs — crosser 9 standing at the west
    /// curb (-3.7, 5.0) while the camera passes the north crosswalk
    /// (pose 380, its z = 5 line) 3.7 m east; the return crossings
    /// (from 510) cross the camera's line at ticks ~672/~678, after
    /// the camera has parked at [0, 1.7, 38] (pose 600 = tick 660).
    /// The pairwise discipline is NOT claimed beyond the demo's 663
    /// presented frames.
    #[test]
    fn composed_crossings_clear_the_camera_on_the_shared_clock() {
        use crate::corner_routes::corner_population_locomotion;

        // Materialize the route poses once (601 entries).
        let poses: Vec<_> = crate::corner_demo::route_poses().collect();

        // Build two spines with identical world states
        let (_routes, states, present) = corner_population_locomotion();
        let world_a = CornerCrowdWorld::new(states.clone(), present.clone());
        let mut spine_a = engine_core::spine::Spine::with_reservation(world_a, 64, 32);
        let world_b = CornerCrowdWorld::new(states, present);
        let mut spine_b = engine_core::spine::Spine::with_reservation(world_b, 64, 32);

        // Arm every schedule entry at its first occurrence (same as run_campaign)
        let arm_entries = |spine: &mut engine_core::spine::Spine<CornerCrowdWorld>| {
            let world = spine.world();
            let mut specs: Vec<engine_core::event::EventSpec> = Vec::new();
            for npc in 0..24 {
                let schedule = &world.schedules[npc];
                for (entry_idx, entry) in schedule.entries.iter().enumerate() {
                    let payload =
                        engine_core::npc_execution::wake_payload(npc as u32, entry_idx as u32)
                            .expect("the entry index packs");
                    specs.push(engine_core::event::EventSpec {
                        class: WAKE_CLASS,
                        schedule: engine_core::event::Schedule::AtLogicalTime { due: entry.start },
                        payload,
                        field: entry.activity.field,
                        depends_on: Vec::new(),
                    });
                }
            }
            let _world = world;
            for spec in specs {
                spine.schedule(spec).expect("arming is legal");
            }
        };
        arm_entries(&mut spine_a);
        arm_entries(&mut spine_b);

        let mut global_min = f32::INFINITY;
        let mut global_min_tick = 0u64;

        // THE FRAME-k CONVENTION (the demo's render-then-step, the plain
        // gate's own): presented frame k renders the crowd after k
        // composed ticks with camera pose clamp(k - 60, 0, 600) — the
        // measurement reads the state BEFORE the tick's spine+batch
        // step, exactly the state the demo renders at that frame.
        for k in 0..=662u64 {
            let world_a = spine_a.world();
            let world_b = spine_b.world();

            // Double-run equality for states[8]/states[9]
            for i in 8..10 {
                assert_eq!(
                    (
                        world_a.states[i].position,
                        world_a.states[i].facing,
                        world_a.states[i].leg,
                        world_a.states[i].stride_phase
                    ),
                    (
                        world_b.states[i].position,
                        world_b.states[i].facing,
                        world_b.states[i].leg,
                        world_b.states[i].stride_phase
                    ),
                    "the composed double-run diverged at tick {k}, npc {i}"
                );
            }

            // THE CAMERA MINIMUM
            let pose_idx = ((k as i64 - 60).max(0) as u64).min(600);
            let pose = &poses[pose_idx as usize];
            let camera_pos = [pose.position[0], pose.position[2]];

            let mut min_dist = f32::INFINITY;
            for i in 0..24 {
                let npc_pos = world_a.states[i].position;
                let dx = npc_pos[0] - camera_pos[0];
                let dz = npc_pos[1] - camera_pos[1];
                let dist = (dx * dx + dz * dz).sqrt();
                min_dist = min_dist.min(dist);
            }
            if min_dist < global_min {
                global_min = min_dist;
                global_min_tick = k;
            }
            assert!(
                min_dist >= 0.8,
                "composed crowd and camera too close at tick {k}: {min_dist} m"
            );
            // THE TICK'S STEP (after the measurement — the state the
            // next frame renders).
            spine_a.run_tick();
            spine_b.run_tick();
            spine_a.world_mut().composed_tick(1.0 / 60.0);
            spine_b.world_mut().composed_tick(1.0 / 60.0);
        }

        println!("composed crossings camera minimum: {global_min} at tick {global_min_tick}");
        // THE MEASURED PIN: the composed mode's first ~420 ticks are the
        // plain mode's own (the retarget at 90 re-affirms the crossing
        // already under way; the first stand begins at the far-curb
        // arrival ~420), so the minimum is the PLAIN gate's own value
        // at its own frame — a genuine cross-check between the two
        // gates (movement is an investigation, never a silent repin).
        assert_eq!(
            global_min, 1.8529123,
            "the composed same-frame camera minimum moved (the plain gate's own pin)"
        );
        assert_eq!(
            global_min_tick, 178,
            "the composed same-frame camera minimum's tick moved (the plain gate's own frame)"
        );
    }
}
