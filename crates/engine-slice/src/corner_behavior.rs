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
/// CROSSERS 8-9 (the pacers, routes 8/9): TWO entries each (the pacing alternation).
///   Crosser 8: entry 0 start=30, entry 1 start=90
///   Crosser 9: entry 0 start=90, entry 1 start=150
///   Both entries: cadence every(120), kind 200→201 alternating
///   The seam maps kind 200→waypoint 1 (far end), 201→waypoint 0 (back).
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
    for (npc, start0, start1) in [(8u32, 30u64, 90u64), (9, 90, 150)] {
        let schedule = NpcSchedule {
            entries: vec![
                Commitment {
                    start: Tick(start0),
                    cadence: Cadence::every(120).expect("period 120 is nonzero"),
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
                    start: Tick(start1),
                    cadence: Cadence::every(120).expect("period 120 is nonzero"),
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
/// 200 → the far pacing end (waypoint 1), 201 → back (waypoint 0);
/// strollers: kind 300 → LOOPING (the resume — no redraw, no phase
/// reset, continuing from the current leg_distance), 301 → the rest
/// waypoint W_k (1 for even k, 3 for odd — the pauses spread over
/// both lanes). Returns the `set_destination` outcome: false is the
/// authored refusal, NEVER expected on this table — the world counts
/// it and the campaign gate asserts zero.
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
        200 => set_destination(route, state, 1),
        201 => set_destination(route, state, 0),
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

/// The composed crowd's spine world: the 24 schedules/routines/rngs
/// (one per NPC), the locomotion states and present mask from part
/// 4's constructor, and the behavior stream (PRE-ALLOCATED — the
/// composed per-tick path allocates nothing; the capacity bound is
/// derived below). `execute` IS the policy seam: a wake event
/// evaluates (schedule authority) and retargets the walker.
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
    /// The behavior stream (capacity bound documented in the module).
    pub stream: Vec<StreamEntry>,
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
}

impl CornerCrowdWorld {
    /// The composed crowd constructor.
    ///
    /// The stream Vec's capacity is 256 (663 ticks × the cadence
    /// arithmetic give ≈ 70 entries total — waiter 2-3 fires each,
    /// crossers ~11 each, strollers ~5-6 each ≈ 24 + 22 + 76 ≈ 122;
    /// 256 carries 2× headroom; the allocation gate MEASURES it).
    pub fn new(states: Vec<NpcLocomotion>, present: Vec<bool>) -> Self {
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
        }];
        let rates = [0.001; MAX_NEEDS];
        let stream = Vec::with_capacity(256);
        let routes = corner_routes();
        CornerCrowdWorld {
            schedules,
            routines,
            rngs,
            last_decay_tick,
            rules,
            rates,
            stream,
            states,
            present,
            routes,
            wake_executions: 0,
            retarget_refusals: 0,
            arrivals: 0,
            stepped: 0,
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
    /// - The stream capacity is construction-time (256)
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
        self.stream.push(StreamEntry {
            tick: ctx.executed_tick(),
            npc,
            activity: outcome.activity,
        });
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
        routines: Vec<engine_core::npc_behavior::NpcRoutine>,
        wake_executions: u64,
        retarget_refusals: u64,
        arrivals: u64,
        fingerprint: u64,
    }

    /// Runs the composed campaign for `ticks` from a fresh double
    /// construction; returns the terminal readout (frame `ticks`).
    fn run_campaign(ticks: u64) -> CampaignSnapshot {
        let (_, states, present) = corner_population_locomotion();
        let world = CornerCrowdWorld::new(states, present);
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
            routines: world.routines.clone(),
            wake_executions: world.wake_executions,
            retarget_refusals: world.retarget_refusals,
            arrivals: world.arrivals,
            fingerprint: fp.finish(),
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

        // Crosser 8: 2 entries (30/90, kinds 200/201, every 120).
        let s = &schedules[8];
        assert_eq!(s.entries.len(), 2, "crosser 8 has 2 entries");
        assert_eq!(s.entries[0].start.as_u64(), 30, "crosser 8 entry 0 start");
        assert_eq!(s.entries[0].activity.kind, 200, "crosser 8 entry 0 kind");
        assert_eq!(s.entries[1].start.as_u64(), 90, "crosser 8 entry 1 start");
        assert_eq!(s.entries[1].activity.kind, 201, "crosser 8 entry 1 kind");
        assert_eq!(
            s.entries[0].cadence,
            Cadence::every(120).unwrap(),
            "crosser 8 cadence"
        );

        // Crosser 9: 2 entries (90/150).
        let s = &schedules[9];
        assert_eq!(s.entries.len(), 2, "crosser 9 has 2 entries");
        assert_eq!(s.entries[0].start.as_u64(), 90, "crosser 9 entry 0 start");
        assert_eq!(s.entries[0].activity.kind, 200, "crosser 9 entry 0 kind");
        assert_eq!(s.entries[1].start.as_u64(), 150, "crosser 9 entry 1 start");
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
    /// - crosser 8: entry 0 at 30+120k (30,150,270,390,510,630 = 6),
    ///   entry 1 at 90+120k (90,210,330,450,570 = 5) = 11;
    /// - crosser 9: entry 0 at 90+120k (5), entry 1 at 150+120k
    ///   (150,270,390,510,630 = 5) = 10;
    /// - strollers: entry 0 at 60+17k+240m, entry 1 at +120 — npc 10
    ///   fires 6 (60,300,540 + 180,420,660), npcs 11-17 fire 5 each,
    ///   npcs 18-23 fire 4 each = 6 + 35 + 24 = 65.
    ///
    /// TOTAL 13 + 11 + 10 + 65 = 99 wake executions.
    ///
    /// The arrival count's crosser portion is derived (each pacing
    /// retarget walks ~47 ticks and arrives, except a fire too late in
    /// the window); the stroller portion depends on where the rests
    /// find them (a rest may be interrupted by the next stroll before
    /// reaching W) — the total is a MEASURED pin (the placeholder-pin
    /// discipline: 25 at this tree).
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

        // THE COUNTER PINS (the derivation above; the arrivals measured).
        assert_eq!(
            a.wake_executions, 99,
            "the wake count matches the cadence arithmetic"
        );
        assert_eq!(a.retarget_refusals, 0, "the seam never refuses");
        assert_eq!(a.arrivals, 25, "the measured arrival count");

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
        // The MEASURED pins (this tree's own run; movement is an
        // investigation, never a silent repin).
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
    /// mode's — no wake fires before tick 30); frame 33's likewise (the
    /// retarget at tick 30 changes only the destination, not the motion
    /// before the first arrival); frame 0's execution digest is the FNV
    /// offset basis (the empty stream — no wake has fired yet).
    const PIN_LOCO_0: u64 = 5_555_165_838_415_353_039;
    const PIN_LOCO_33: u64 = 15_857_914_442_065_508_438;
    const PIN_LOCO_300: u64 = 178_212_510_757_263_954;
    const PIN_LOCO_662: u64 = 9_730_262_620_159_629_689;
    const PIN_EXEC_0: u64 = 14_695_981_039_346_656_037;
    const PIN_EXEC_33: u64 = 14_402_972_430_116_642_247;
    const PIN_EXEC_300: u64 = 16_534_949_987_853_281_436;
    const PIN_EXEC_662: u64 = 12_924_330_556_365_912_741;

    /// THE SEAM RETARGET KNOWN ANSWERS (the REAL seam — the same
    /// function `execute` drives, never a test copy).
    #[test]
    fn seam_retarget_known_answers() {
        let routes = corner_routes();

        // The pacer's alternation: standing at waypoint 0, kind 200
        // walks to the far end (waypoint 1); after the ARRIVAL (the
        // snap-and-stand shape: leg = destination, speed = 0), kind
        // 201 walks back (waypoint 0).
        let mut state = start_route(8, &routes[8], 200, 0, 0).expect("crosser start");
        assert!(seam_retarget(8, 200, &mut state, &routes));
        assert_eq!(state.destination, 1, "kind 200 → the far pacing end");
        assert!(state.speed > 0.0, "kind 200 restarts the walk");
        state.leg = 1; // the arrival's snap-and-stand (advance_batch's own shape)
        state.speed = 0.0;
        assert!(seam_retarget(8, 201, &mut state, &routes));
        assert_eq!(state.destination, 0, "kind 201 → back");
        assert!(state.speed > 0.0, "kind 201 restarts the walk (leg 1 → 0)");

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
}
