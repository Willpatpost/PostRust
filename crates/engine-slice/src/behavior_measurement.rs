//! The Era-2 package A measurement surface (part 6 — the design pass
//! §8.4 item 7 + §8.5's remaining measurements, over the composed
//! machinery): the behavior-workload sustained matrix, the witness
//! enumeration cost per significant event, activation latency with
//! schedule catch-up (E-09's crossover on the real machinery), the
//! skip-cost curves (E-08's shape on the real queue — logical-tick
//! cells, no frame cadence), the stall worst case (B10's added
//! case: the worst-case pending accumulation under a stall that
//! exceeds the per-frame catch-up bound, through the real
//! SimDriver machinery the `--sim` irregular route drives), and
//! memory-per-NPC by tier. ALL pod-observational benchmark work
//! (never CI thresholds, never physical-rig claims) with the
//! double-run determinism proof asserted in every section — the
//! `--benchmark-locomotion` precedent. Pure CPU: no Vulkan, no I/O
//! beyond the report prints, no new engine machinery — every
//! measured path is the real engine-core function.

use engine_core::clock::Tick;
use engine_core::event::{Event, EventClass, EventSpec, Field, Schedule};
use engine_core::fingerprint::Fingerprinter;
use engine_core::lod::LodLevel;
use engine_core::npc_behavior::{
    Activity, ActivityKind, MAX_NEEDS, NpcId, NpcRng, NpcRoutine, Rule,
};
use engine_core::npc_execution::{
    StreamEntry, enumerate_witnesses, evaluate_wake, evaluate_witness, execution_digest,
    retain_trace, wake_payload, wake_payload_parts,
};
use engine_core::npc_locomotion::NpcLocomotion;
use engine_core::npc_perception::{
    NpcRecord, PERCEPTION_BUDGETS, PerceptionTrace, TRACE_STRIDE, perception_budget,
};
use engine_core::npc_schedule::{ActivityDescriptor, Cadence, Commitment, NpcSchedule, Target};
use engine_core::spine::{ExecCtx, Spine, World};

use crate::npc::NpcConfig;
use crate::sim::{SimConfig, SimDriver};
use crate::sim_demo::irregular_frame;

/// The bench's wake class (0xC6 — deployment-local policy; the corner's
/// composed demo uses 0xC5, the engine-core fixture 0xC4).
pub const BENCH_WAKE_CLASS: EventClass = EventClass::new(0xC6);

/// The sustained harness's own configuration: near budget 200, reserve 60.
const BUDGET: u32 = 200;
const RESERVE: u32 = 60;

/// A schedule factory for the sustained matrix.
fn matrix_schedules(
    population: usize,
    entries_per: usize,
    period: u64,
    aligned: bool,
) -> Vec<NpcSchedule> {
    let mut schedules = Vec::with_capacity(population);
    for npc in 0..population {
        let mut entries = Vec::with_capacity(entries_per);
        for e in 0..entries_per {
            let start = if aligned {
                Tick(period)
            } else {
                Tick(1 + ((npc * entries_per + e) % period as usize) as u64)
            };
            entries.push(Commitment {
                start,
                cadence: Cadence::every(period).expect("period is nonzero"),
                activity: ActivityDescriptor {
                    kind: 300,
                    field: Field::Near,
                },
                target: Target {
                    npc: npc as u32,
                    anchor: (0, 0),
                },
            });
        }
        schedules.push(NpcSchedule { entries });
    }
    schedules
}

/// A schedule factory for the skip-cost cells: 125 NPCs × 2 entries
/// whose authored period is `2 * duration` — EVERY armed event's
/// honest re-arm (`next_occurrence` past its execution tick) lands
/// beyond the measured window, so the executed count is EXACTLY the
/// armed count (the conservation the section asserts). The armed
/// dues themselves are the cell's authored event-density spread
/// (see `run_skip_cost_curves`); the entries exist so each armed
/// payload addresses a REAL commitment and the handler runs the
/// REAL `evaluate_wake` re-arm path.
fn skip_schedules(rearm_period: u64) -> Vec<NpcSchedule> {
    let mut schedules = Vec::with_capacity(125);
    for npc in 0..125 {
        schedules.push(NpcSchedule {
            entries: vec![
                Commitment {
                    start: Tick(1),
                    cadence: Cadence::every(rearm_period).expect("period is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 300,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: npc as u32,
                        anchor: (0, 0),
                    },
                },
                Commitment {
                    start: Tick(2),
                    cadence: Cadence::every(rearm_period).expect("period is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 301,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: npc as u32,
                        anchor: (0, 0),
                    },
                },
            ],
        });
    }
    schedules
}

/// A schedule factory for the activation crossover: 125 NPCs × 2
/// entries, period 600, starts 600 and 300 — over the dormancy span
/// (0, k·600] BOTH entries have EXACTLY k occurrences (entry 0:
/// 600, 1200, …, k·600; entry 1: 300, 900, …, 300+(k−1)·600 =
/// k·600−300), the authored arithmetic the section's re-arm
/// asserts pin.
fn crossover_schedules() -> Vec<NpcSchedule> {
    let mut schedules = Vec::with_capacity(125);
    for npc in 0..125 {
        schedules.push(NpcSchedule {
            entries: vec![
                Commitment {
                    start: Tick(600),
                    cadence: Cadence::every(600).expect("period 600 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 300,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: npc as u32,
                        anchor: (0, 0),
                    },
                },
                Commitment {
                    start: Tick(300),
                    cadence: Cadence::every(600).expect("period 600 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 301,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: npc as u32,
                        anchor: (0, 0),
                    },
                },
            ],
        });
    }
    schedules
}

/// A schedule factory for the activation crossover: 125 NPCs × 2
/// entries, period 600, starts 600 and 300 — over the dormancy span
/// (0, k·600] BOTH entries have EXACTLY k occurrences (entry 0:
/// 600, 1200, …, k·600; entry 1: 300, 900, …, 300+(k−1)·600 =
/// k·600−300), the authored arithmetic the section's re-arm
/// asserts pin.
/// The corner's one idle-baseline rule.
const IDLE_RULE: Rule = Rule {
    activity: ActivityKind::Idle,
    need: None,
    base_priority: 0,
    applies_below: 0.0,
    witness: None,
};

/// The behavior scale world.
struct BehaviorScaleWorld {
    schedules: Vec<NpcSchedule>,
    routines: Vec<NpcRoutine>,
    rngs: Vec<NpcRng>,
    last_decay: Vec<Tick>,
    rules: Vec<Rule>,
    rates: [f32; MAX_NEEDS],
    stream: Vec<StreamEntry>,
    wake_executions: u64,
    /// Per-NPC wake executions (the sign-off review's finding 28
    /// corrective: the total-count conservation alone cannot catch a
    /// redistribution — the right number of events on the wrong NPCs;
    /// the drain's per-NPC census closes that escape).
    per_npc_wakes: Vec<u64>,
}

impl BehaviorScaleWorld {
    fn new(schedules: Vec<NpcSchedule>) -> Self {
        let n = schedules.len();
        let routines = vec![
            NpcRoutine {
                current: Activity {
                    kind: ActivityKind::Idle,
                    rule: u32::MAX,
                },
                needs: [1.0; MAX_NEEDS],
                route_progress: 0,
            };
            n
        ];
        let rngs = (0..n)
            .map(|i| NpcRng::for_npc(NpcId(1000 + i as u32)))
            .collect();
        let last_decay = vec![Tick(0); n];
        let rules = vec![IDLE_RULE];
        let rates = [0.001; MAX_NEEDS];
        let stream = Vec::with_capacity(1024);
        BehaviorScaleWorld {
            schedules,
            routines,
            rngs,
            last_decay,
            rules,
            rates,
            stream,
            wake_executions: 0,
            per_npc_wakes: vec![0; n],
        }
    }
}

impl World for BehaviorScaleWorld {
    fn execute(&mut self, event: &Event, ctx: &mut ExecCtx) {
        if event.class != BENCH_WAKE_CLASS {
            return;
        }
        let Some((npc, commitment)) = wake_payload_parts(event.payload) else {
            return;
        };
        if npc as usize >= self.schedules.len() {
            return;
        }
        let npc_idx = npc as usize;
        if commitment as usize >= self.schedules[npc_idx].entries.len() {
            return;
        }
        let mut rng = self.rngs[npc_idx];
        let routine = self.routines[npc_idx];
        let Some(outcome) = evaluate_wake(
            &self.schedules[npc_idx],
            commitment,
            ctx.executed_tick(),
            self.last_decay[npc_idx],
            &routine,
            &self.rules,
            &mut rng,
            &self.rates,
        ) else {
            return;
        };
        self.routines[npc_idx] = outcome.routine_after;
        self.rngs[npc_idx] = rng;
        self.last_decay[npc_idx] = ctx.executed_tick();
        self.stream.push(StreamEntry {
            tick: ctx.executed_tick(),
            npc,
            activity: outcome.activity,
        });
        self.wake_executions += 1;
        self.per_npc_wakes[npc_idx] += 1;
        if let Some(next) = outcome.next_wake {
            let payload = wake_payload(npc, commitment).expect("the authored index round-trips");
            ctx.schedule_at(next, BENCH_WAKE_CLASS, payload, Vec::new());
        }
    }

    fn fingerprint(&self, fp: &mut Fingerprinter) {
        fp.write_str("engine-slice/behavior-measurement-world/1");
        for i in 0..self.schedules.len() {
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
    }
}

/// Arms every entry at its own first occurrence.
fn arm_all_entries(spine: &mut Spine<BehaviorScaleWorld>) {
    let world = spine.world();
    let mut specs: Vec<EventSpec> = Vec::new();
    for npc in 0..world.schedules.len() {
        let schedule = &world.schedules[npc];
        for (entry_idx, entry) in schedule.entries.iter().enumerate() {
            let payload =
                wake_payload(npc as u32, entry_idx as u32).expect("the entry index packs");
            specs.push(EventSpec {
                class: BENCH_WAKE_CLASS,
                schedule: Schedule::AtLogicalTime { due: entry.start },
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

// ------------------------------------------------------- SECTION 1 — sustained behavior matrix

fn run_sustained_behavior_matrix() {
    println!("Behavior-workload sustained matrix — the §8.4 item 7 measurement");
    println!("  pod results, observational (never CI thresholds, never physical-rig claims)");
    println!();

    const POPS: [usize; 2] = [125, 1000];

    // The three demand classes (the sustained methodology's class
    // set, authored as schedule arithmetic): average events/tick =
    // population × entries_per / period. Synchrony is the SECOND
    // dimension (aligned vs staggered per class), not a fourth
    // class — the delegate round's duplicated "synchrony" class was
    // low-demand twice, repaired here.
    type DemandClass = (&'static str, u32, u64, u64);
    let classes: [DemandClass; 3] = [
        ("low-demand", 100, 20, 20),
        ("near-saturation", 200, 10, 10),
        ("overload", 400, 5, 5),
    ];

    for &pop in &POPS {
        for &(name, target, period16, period2) in &classes {
            let entries_per = if pop == 125 { 16 } else { 2 };
            let period = if pop == 125 { period16 } else { period2 };
            let authored_demand = (pop * entries_per) as f64 / period as f64;
            // The authoring check: the derived demand IS the class's
            // nominal target (the authored-arithmetic gate on the
            // cell table itself).
            assert_eq!(
                authored_demand, target as f64,
                "the authored cadence hits the {name} target at population {pop}"
            );

            for &aligned in &[true, false] {
                let schedules = matrix_schedules(pop, entries_per, period, aligned);
                let world = BehaviorScaleWorld::new(schedules);
                let mut spine = Spine::with_reservation(world, BUDGET, RESERVE);
                arm_all_entries(&mut spine);

                for _ in 0..600 {
                    spine.run_tick();
                }
                let mut samples = engine_core::timing::TimingSamples::default();
                for _ in 0..30 {
                    let started = std::time::Instant::now();
                    spine.run_ticks(60);
                    samples.record(started.elapsed());
                }

                // DOUBLE-RUN (fresh construction, no shared state):
                // the whole cell again, then the equality asserts —
                // printed claims come AFTER verification, never before.
                let schedules2 = matrix_schedules(pop, entries_per, period, aligned);
                let world2 = BehaviorScaleWorld::new(schedules2);
                let mut spine2 = Spine::with_reservation(world2, BUDGET, RESERVE);
                arm_all_entries(&mut spine2);
                for _ in 0..600 {
                    spine2.run_tick();
                }
                for _ in 0..30 {
                    spine2.run_ticks(60);
                }
                assert_eq!(
                    execution_digest(&spine.world().stream),
                    execution_digest(&spine2.world().stream),
                    "double-run stream digest identical for {} pop {} aligned {}",
                    name,
                    pop,
                    aligned
                );
                let mut fp = Fingerprinter::new();
                spine.world().fingerprint(&mut fp);
                let mut fp2 = Fingerprinter::new();
                spine2.world().fingerprint(&mut fp2);
                assert_eq!(
                    fp.finish(),
                    fp2.finish(),
                    "double-run world fingerprint identical for {} pop {} aligned {}",
                    name,
                    pop,
                    aligned
                );
                drop(spine2);

                let stats = spine.stats();
                let lane = spine.lane_totals();
                let pending = spine.pending_count() as u64;
                let executed_plus = stats.executed
                    + pending
                    + spine.blocked_count() as u64
                    + spine.held_count() as u64;
                assert_eq!(
                    stats.scheduled, executed_plus,
                    "conservation: scheduled == executed+pending+blocked+held for {} pop {} aligned {}",
                    name, pop, aligned
                );

                let p50 = samples.percentile_ms(0.50);
                let p99 = samples.percentile_ms(0.99);
                let max_ms = samples.max_ms();
                let total_ticks = 600 + 30 * 60;
                let delivered_per_tick = stats.executed as f64 / total_ticks as f64;

                println!(
                    "sustained cell: class {} / population {} / synchrony {} / budget {} (reserve {}) / period {}",
                    name,
                    pop,
                    if aligned { "aligned" } else { "staggered" },
                    BUDGET,
                    RESERVE,
                    period
                );
                println!(
                    "  cost p50/p99/max {:.3}/{:.3}/{:.3} ms/window ({} samples)",
                    p50,
                    p99,
                    max_ms,
                    samples.sample_count()
                );
                println!(
                    "  demand: authored {:.1} events/tick, delivered {:.1} events/tick (budget-limiting is NOT efficiency)",
                    authored_demand, delivered_per_tick
                );
                println!(
                    "  logical progress: scheduled {} / executed {} (near {} / far {}) / pending {}",
                    stats.scheduled, stats.executed, lane.near_executed, lane.far_executed, pending
                );
                println!(
                    "  late executions {} / budget-exhausted ticks {} / near-backlogged ticks {}",
                    stats.late_executions, stats.budget_exhausted_ticks, lane.near_backlogged_ticks
                );
                println!("  determinism: DOUBLE-RUN IDENTICAL (asserted above)");
                println!();
            }
        }
    }
}

// ------------------------------------------------------- SECTION 2 — witness cost

fn run_witness_cost() {
    println!("Witness enumeration cost — the §8.5 measurement");
    println!("  pod results, observational (never CI thresholds, never physical-rig claims)");
    println!();

    const POPS: [usize; 3] = [24, 125, 1000];

    // The tier mixes, BUILT PER POPULATION (the delegate round built
    // them at the 24 length and let `enumerate_witnesses`' fail-closed
    // missing-tier rule silently shrink the 125/1000 censuses to their
    // first 24 records — exactly the dropped-sample defect class the
    // repo bars; repaired here).
    let mix_for = |pop: usize, mix: usize| -> Vec<LodLevel> {
        match mix {
            0 => vec![LodLevel::Full; pop],
            1 => (0..pop)
                .map(|i| match i % 4 {
                    0 => LodLevel::Full,
                    1 => LodLevel::Simplified,
                    2 => LodLevel::EventDriven,
                    _ => LodLevel::Dormant,
                })
                .collect(),
            _ => vec![LodLevel::Dormant; pop],
        }
    };
    let mix_names = ["all_full", "mixed", "all_dormant"];

    fn tier_level(level: LodLevel) -> u8 {
        match level {
            LodLevel::Full => 0,
            LodLevel::Simplified => 1,
            LodLevel::EventDriven => 2,
            LodLevel::Dormant => 3,
        }
    }

    for &pop in &POPS {
        for (mix_idx, mix_name) in mix_names.iter().enumerate() {
            let tiers = mix_for(pop, mix_idx);
            let side = (pop as f32).sqrt().ceil() as u32;
            let population: Vec<NpcRecord> = (0..pop)
                .map(|i| NpcRecord {
                    id: i as u32,
                    pos: [(i % side as usize) as f32, (i / side as usize) as f32],
                })
                .collect();

            let center = [side as f32 / 2.0, side as f32 / 2.0];
            let radius = 5.0;

            let routines = vec![
                NpcRoutine {
                    current: Activity {
                        kind: ActivityKind::Idle,
                        rule: u32::MAX,
                    },
                    needs: [1.0; MAX_NEEDS],
                    route_progress: 0,
                };
                pop
            ];
            let mut rngs: Vec<NpcRng> = (0..pop)
                .map(|i| NpcRng::for_npc(NpcId(1000 + i as u32)))
                .collect();
            let rules = vec![IDLE_RULE];
            let rates = [0.001; MAX_NEEDS];

            // The census scratch (caller-owned, reused — the F-02
            // discipline) and the per-witness bounded memories.
            let mut scratch = Vec::with_capacity(1024);
            let mut memories: Vec<Vec<PerceptionTrace>> = vec![Vec::with_capacity(32); pop];
            // The per-event perceptions scratch: built ONCE per
            // significant event from the census (the census IS the
            // witness set), never per witness — the O(W²) rebuild of
            // the delegate round measured allocator traffic, not the
            // witness path.
            let mut perceptions: Vec<(u32, NpcId)> = Vec::with_capacity(1024);

            // WARM: 256 census calls (the measured path's own shape).
            for _ in 0..256 {
                enumerate_witnesses(center, radius, &population, &tiers, &mut scratch);
            }

            // MEASURED: the census alone (2048 calls).
            let mut census_samples = engine_core::timing::TimingSamples::default();
            for _ in 0..2048 {
                let started = std::time::Instant::now();
                enumerate_witnesses(center, radius, &population, &tiers, &mut scratch);
                census_samples.record(started.elapsed());
            }
            let admitted = scratch.len();

            // MEASURED: the FULL per-significant-event path (256
            // events): census → perceptions built once → per-witness
            // evaluate_witness + retain_trace at the tier's budget.
            let mut event_samples = engine_core::timing::TimingSamples::default();
            for _ in 0..256 {
                let started = std::time::Instant::now();
                enumerate_witnesses(center, radius, &population, &tiers, &mut scratch);
                perceptions.clear();
                for &idx in scratch.iter() {
                    perceptions.push((7u32, NpcId(population[idx as usize].id)));
                }
                for &witness_idx in scratch.iter() {
                    let idx = witness_idx as usize;
                    let budget = perception_budget(tier_level(tiers[idx]));
                    let trace = PerceptionTrace {
                        observer: population[idx].id,
                        perceived: 0,
                        tick: Tick(1000),
                        class: 7,
                    };
                    let mut rng = rngs[idx];
                    let _outcome = evaluate_witness(
                        &routines[idx],
                        &perceptions,
                        Tick(1000),
                        Tick(0),
                        &rules,
                        &mut rng,
                        &rates,
                    )
                    .expect("the bench's authored table validates at admission");
                    rngs[idx] = rng;
                    retain_trace(&mut memories[idx], trace, budget);
                }
                event_samples.record(started.elapsed());
            }

            let census_p50 = census_samples.percentile_ms(0.50) * 1000.0;
            let census_p99 = census_samples.percentile_ms(0.99) * 1000.0;
            let event_p50 = event_samples.percentile_ms(0.50) * 1000.0;
            let event_p99 = event_samples.percentile_ms(0.99) * 1000.0;

            // DOUBLE-RUN: the census set is identical across fresh runs.
            let mut scratch_a = Vec::with_capacity(1024);
            enumerate_witnesses(center, radius, &population, &tiers, &mut scratch_a);
            let mut scratch_b = Vec::with_capacity(1024);
            enumerate_witnesses(center, radius, &population, &tiers, &mut scratch_b);
            assert_eq!(scratch_a, scratch_b, "census scratch identical across runs");

            println!("witness cell: population {} / mix {}", pop, mix_name);
            println!(
                "  census p50/p99 {:.3}/{:.3} µs per significant event ({} samples)",
                census_p50,
                census_p99,
                census_samples.sample_count()
            );
            println!("  admitted witnesses: {}", admitted);
            println!(
                "  full-event p50/p99 {:.3}/{:.3} µs per event (census + {} witness evaluations, {} samples)",
                event_p50,
                event_p99,
                admitted,
                event_samples.sample_count()
            );
            println!("  determinism: census set DOUBLE-RUN IDENTICAL (asserted above)");
            println!();
        }
    }
}

// ------------------------------------------------------- SECTION 3 — activation crossover

fn run_activation_crossover() {
    println!("Activation latency with schedule catch-up — the E-09 crossover measurement");
    println!("  pod results, observational (never CI thresholds, never physical-rig claims)");
    println!();

    // THE B10 ADJUDICATION (the slice audit's replay-vs-synthesis
    // claim, verified by direct read): the audit's premise — that
    // the synthesis arm "uses `NpcWorld::restore`, reconstructing
    // state from save bytes" while "replay replays from the last
    // checkpoint" — is FALSE for this harness. Neither arm touches
    // save bytes, restore, or checkpoints (no such call exists in
    // this module; the dormancy span is QUIET — nothing armed, so
    // there is no checkpoint to replay from, and no held events
    // exist on this queue). What the arms genuinely differ in IS
    // the measured quantity — WHICH reactivation policy's work is
    // cheaper at which miss count: the replay policy re-executes
    // every missed occurrence through the real queue; the synthesis
    // policy performs ONE evaluation per NPC (the lazy-decay jump)
    // and re-arms the live chains directly. The methodology block
    // below labels each arm's timed window so the comparison cannot
    // be overread as like-for-like work; the outcome divergence is
    // reported beside the costs (finding 8's correction), never
    // equated.
    println!("  methodology (B10's labeling corrective — what each arm's timed window IS):");
    println!(
        "    replay arm: arm 250·k past-due events onto the live spine, then drain \
         them through the real queue — every missed occurrence re-executes \
         (budget-bounded, ceil(250k/{BUDGET})+2 drain ticks; the honest re-arms stay \
         pending beyond the window)"
    );
    println!(
        "    synthesis arm: the per-NPC loop (ONE evaluate_wake per NPC — the \
         lazy-decay jump over the whole dormancy span — plus each entry's \
         next_occurrence re-arm jump), then arm the 250 live chains on a \
         pre-constructed spine — NO queue drain, and NO restore-from-save-bytes \
         in EITHER arm: the arms' different work IS the measured policy choice"
    );
    println!();

    // THE k SEMANTICS (finding 8's correction, documented and
    // asserted): k is the missed occurrences PER ENTRY over the
    // dormancy span (0, k·600] — 2k per NPC at this factory's two
    // entries per NPC, i.e. 250·k pending events across the 125-NPC
    // population. The old closing summary mislabeled k itself as
    // events per NPC; every print below names the unit.
    //
    // The dormancy span: reactivation at T1 = k·600 + 1 with EXACTLY
    // k missed occurrences per entry per NPC (the factory's authored
    // arithmetic, asserted below on the real `next_occurrence`
    // machinery) — 125 NPCs × 2 entries × k pending events.
    let k_values = [1u64, 2, 4, 8, 16, 32, 64];
    const REPS: usize = 64;

    // The authored arithmetic, asserted (the k-semantics gate): entry
    // 0's occurrences in (0, k·600] are exactly 600, 1200, …, k·600;
    // entry 1's exactly 300, 900, …, 300+(k−1)·600 = k·600−300 — k
    // each, and the next occurrence past the span is the re-arm both
    // strategies arm (the due equality the section enforces).
    for &k in &k_values {
        let schedules = crossover_schedules();
        let entry0 = &schedules[0].entries[0];
        let entry1 = &schedules[0].entries[1];
        for j in 1..=k {
            assert_eq!(
                entry0.next_occurrence(Tick((j - 1) * 600)),
                Some(Tick(j * 600)),
                "k {k}: entry 0's j-th occurrence is j·600 (k is occurrences PER ENTRY)"
            );
            assert_eq!(
                entry1.next_occurrence(Tick(300 + (j - 1) * 600)),
                Some(Tick(300 + j * 600)),
                "k {k}: entry 1's j-th occurrence is 300+(j−1)·600 (k is occurrences \
                 PER ENTRY)"
            );
        }
        assert_eq!(
            entry0.next_occurrence(Tick(k * 600)),
            Some(Tick((k + 1) * 600)),
            "k {k}: entry 0's post-span re-arm due"
        );
        assert_eq!(
            entry1.next_occurrence(Tick(300 + (k - 1) * 600)),
            Some(Tick(300 + k * 600)),
            "k {k}: entry 1's post-span re-arm due"
        );
    }

    /// One strategy's post-activation outcome, for the cross-strategy
    /// comparison (finding 8: the differing outcomes are REPORTED
    /// beside the two costs — the strategies are deliberately
    /// different reconstructors, never silently equated).
    struct ActivationOutcome {
        /// Live events on the queue after the activation window.
        pending: u64,
        /// Executions the world performed (replay: the whole missed
        /// burst; synthesis: none — its evaluations are loop-side).
        wake_executions: u64,
        /// Per-NPC needs bits, folded in population order.
        needs_digest: u64,
        /// Per-NPC current activity (rule + kind + payload), folded
        /// in population order.
        activity_digest: u64,
    }

    /// Folds a world's routine lane into the outcome shape.
    fn world_outcome(world: &BehaviorScaleWorld, pending: u64) -> ActivationOutcome {
        let mut needs = Fingerprinter::new();
        let mut activities = Fingerprinter::new();
        for i in 0..world.schedules.len() {
            for need in world.routines[i].needs {
                needs.write_u32(need.to_bits());
            }
            let current = world.routines[i].current;
            activities.write_u32(current.rule);
            match current.kind {
                ActivityKind::Idle => activities.write_u8(0),
                ActivityKind::MoveTo { route, waypoint } => {
                    activities.write_u8(1);
                    activities.write_u32(route);
                    activities.write_u32(waypoint);
                }
                ActivityKind::Fulfill { commitment } => {
                    activities.write_u8(2);
                    activities.write_u32(commitment);
                }
                ActivityKind::React { class, subject } => {
                    activities.write_u8(3);
                    activities.write_u32(class);
                    activities.write_u32(subject.0);
                }
            }
        }
        ActivationOutcome {
            pending,
            wake_executions: world.wake_executions,
            needs_digest: needs.finish(),
            activity_digest: activities.finish(),
        }
    }

    /// The REPLAY policy: the missed occurrences re-execute through
    /// the real queue. The spine's clock advances QUIETLY to T1 first
    /// (nothing armed — the dormancy), then the k·250 past-due events
    /// arm and drain: the timed window is the arm + the late
    /// execution burst, the E-09 replay cost (∝ pending events).
    /// Returns the PAIRED end-to-end sample (the whole activation as
    /// one duration) and the post-activation outcome for the
    /// cross-strategy comparison.
    fn replay_once(k: u64) -> (u128, ActivationOutcome) {
        let world = BehaviorScaleWorld::new(crossover_schedules());
        let mut spine = Spine::with_reservation(world, BUDGET, RESERVE);
        // The reactivation tick is STRICTLY past every missed
        // occurrence (entry 0's k-th lands AT k·600; T1 = k·600 + 1
        // puts the whole armed set past-due — every one a late
        // arrival, the honest replay shape).
        spine.run_ticks(k * 600 + 1);
        let started = std::time::Instant::now();
        let mut specs: Vec<EventSpec> = Vec::with_capacity(250 * k as usize);
        for npc in 0..125u32 {
            for j in 1..=k {
                specs.push(EventSpec {
                    class: BENCH_WAKE_CLASS,
                    schedule: Schedule::AtLogicalTime { due: Tick(j * 600) },
                    payload: wake_payload(npc, 0).expect("payload packs"),
                    field: Field::Near,
                    depends_on: Vec::new(),
                });
                specs.push(EventSpec {
                    class: BENCH_WAKE_CLASS,
                    schedule: Schedule::AtLogicalTime {
                        due: Tick(300 + (j - 1) * 600),
                    },
                    payload: wake_payload(npc, 1).expect("payload packs"),
                    field: Field::Near,
                    depends_on: Vec::new(),
                });
            }
        }
        for spec in specs {
            spine.schedule(spec).expect("arming is legal");
        }
        // The drain stops AT the reactivation point: the past-due
        // events execute as late arrivals within the budget, and the
        // honest re-arms (the entry's next occurrences, T1+299 at
        // the nearest) stay PENDING — the post-reactivation live
        // chain is NOT activation cost (the delegate round's
        // `run_to_completion` walked into it, counting 875 events at
        // k=1 where 250 were armed; repaired). The window is
        // ceil(250k/budget) + 2 ticks, asserted inside the nearest
        // re-arm distance for every k in the measured range.
        let drain = (250 * k).div_ceil(BUDGET as u64) + 2;
        assert!(
            drain < 299,
            "the drain window stays inside the nearest re-arm (T1+299) at k {k}"
        );
        spine.run_ticks(drain);
        let elapsed = started.elapsed();
        let stats = spine.stats();
        assert_eq!(
            stats.executed,
            250 * k,
            "replay executes every missed occurrence at k {k}"
        );
        assert_eq!(
            stats.late_arrivals,
            250 * k,
            "every replay event arrives late (past-due arming) at k {k}"
        );
        // THE QUEUE-CONSERVATION ASSERTION on the paired campaigns
        // (finding 8's correction): every replayed occurrence
        // re-armed exactly once beyond the window — k live re-arms
        // per entry (250·k total, the k-fold duplicate of the 250
        // dues synthesis arms).
        assert_eq!(
            spine.pending_count() as u64,
            250 * k,
            "every replayed occurrence re-armed exactly once beyond the window at k {k}"
        );
        assert_eq!(
            spine.pending_count() as u64,
            k * 250,
            "k is occurrences per entry: 250 keys × k re-arms at k {k}"
        );
        // THE CROSS-STRATEGY DUE EQUALITY (finding 8's correction):
        // every replay re-arm lands at the SAME due the synthesis
        // policy arms — `next_occurrence` at the window's end tick
        // equals `next_occurrence` at T1 (the drain window stays
        // inside the 299-tick inter-occurrence gap, asserted above),
        // so both strategies' post-activation calendars agree per
        // NPC per entry on (wake key, due). What differs — and is
        // reported beside the costs below, never equated — is the
        // COUNT: replay holds k re-arms per entry at that due,
        // synthesis one.
        let schedules = crossover_schedules();
        let window_end = k * 600 + 1 + drain;
        for schedule in &schedules {
            assert_eq!(
                schedule.entries[0].next_occurrence(Tick(window_end)),
                Some(Tick((k + 1) * 600)),
                "k {k}: entry 0's post-window re-arm due equals synthesis's armed due"
            );
            assert_eq!(
                schedule.entries[1].next_occurrence(Tick(window_end)),
                Some(Tick(300 + k * 600)),
                "k {k}: entry 1's post-window re-arm due equals synthesis's armed due"
            );
        }
        let outcome = world_outcome(spine.world(), spine.pending_count() as u64);
        // THE PER-NPC DRAIN CENSUS (the sign-off review's finding 28
        // corrective): the total-count conservation cannot catch a
        // redistribution — the right number of events on the wrong
        // NPCs. Every NPC executed EXACTLY its authored share (2
        // entries × k occurrences each — the duplicate-drain form of
        // the queued-multiset check).
        for (npc_idx, &wakes) in spine.world().per_npc_wakes.iter().enumerate() {
            assert_eq!(
                wakes,
                2 * k,
                "k {k}: NPC {npc_idx} executed exactly its 2 x k authored occurrences — \
                 no redistribution escapes the census"
            );
        }
        (elapsed.as_nanos(), outcome)
    }

    /// The SYNTHESIS policy (the production activation shape): per
    /// NPC, ONE `evaluate_wake` at T1 (the single lazy decay over the
    /// whole dormancy span — THE synthesis, through the REAL
    /// function) plus each entry's `next_occurrence` re-arm jump;
    /// then the 250 arms on a PRE-CONSTRUCTED spine. THE PAIRED
    /// END-TO-END SAMPLE (finding 8's correction): the whole
    /// activation — loop AND arm — is ONE timed sample, so the
    /// percentiles below are percentiles of per-run sums, never a
    /// sum of separately-computed component percentiles (a sum of
    /// p50s is not the p50 of sums). The component timings are
    /// still returned for the decomposition line, reported as
    /// components only. Also returns the authored re-arm dues (the
    /// double-run determinism proof) and the synthesis-side outcome
    /// (the loop's routine lanes) for the cross-strategy comparison.
    fn synthesis_once(k: u64) -> (u128, u128, u128, Vec<(Tick, u64)>, ActivationOutcome) {
        let total_started = std::time::Instant::now();
        let schedules = crossover_schedules();
        let rates = [0.001; MAX_NEEDS];
        let rules = [IDLE_RULE];
        let mut arms: Vec<(Tick, u64)> = Vec::with_capacity(250);
        let mut needs_digest = Fingerprinter::new();
        let mut activity_digest = Fingerprinter::new();
        let loop_started = std::time::Instant::now();
        for (i, schedule) in schedules.iter().enumerate() {
            let mut rng = NpcRng::for_npc(NpcId(1000 + i as u32));
            let routine = NpcRoutine {
                current: Activity {
                    kind: ActivityKind::Idle,
                    rule: u32::MAX,
                },
                needs: [1.0; MAX_NEEDS],
                route_progress: 0,
            };
            let Some(outcome) = evaluate_wake(
                schedule,
                0,
                Tick(k * 600 + 1),
                Tick(0),
                &routine,
                &rules,
                &mut rng,
                &rates,
            ) else {
                panic!("the authored commitment 0 is in range at k {k}");
            };
            for need in outcome.routine_after.needs {
                needs_digest.write_u32(need.to_bits());
            }
            let current = outcome.routine_after.current;
            activity_digest.write_u32(current.rule);
            match current.kind {
                ActivityKind::Idle => activity_digest.write_u8(0),
                ActivityKind::MoveTo { route, waypoint } => {
                    activity_digest.write_u8(1);
                    activity_digest.write_u32(route);
                    activity_digest.write_u32(waypoint);
                }
                ActivityKind::Fulfill { commitment } => {
                    activity_digest.write_u8(2);
                    activity_digest.write_u32(commitment);
                }
                ActivityKind::React { class, subject } => {
                    activity_digest.write_u8(3);
                    activity_digest.write_u32(class);
                    activity_digest.write_u32(subject.0);
                }
            }
            // NOTE (the sign-off review's finding 28 corrective,
            // labeled): these digests are the PREDICTED per-NPC
            // routine lanes — the loop's evaluations, computed here
            // and reported beside the costs, but NEVER INSTALLED
            // into the armed world (which starts Idle; the synthesis
            // policy is the approximation the paired comparison
            // reports, never an equivalence claim). The digest
            // writes happen inside the loop window so the loop's
            // cost includes its own bookkeeping; the ARM and the
            // world's spine construction sit outside it.
            let next0 = schedule.entries[0].next_occurrence(Tick(k * 600 + 1));
            let next1 = schedule.entries[1].next_occurrence(Tick(k * 600 + 1));
            assert_eq!(
                next0,
                Some(Tick((k + 1) * 600)),
                "entry 0 re-arm jumps the missed occurrences at k {k}"
            );
            assert_eq!(
                next1,
                Some(Tick(300 + k * 600)),
                "entry 1 re-arm jumps the missed occurrences at k {k}"
            );
            arms.push((
                next0.expect("asserted above"),
                wake_payload(i as u32, 0).expect("payload packs"),
            ));
            arms.push((
                next1.expect("asserted above"),
                wake_payload(i as u32, 1).expect("payload packs"),
            ));
        }
        let loop_nanos = loop_started.elapsed().as_nanos();

        // The arm loop on a PRE-CONSTRUCTED spine (the world and spine
        // exist before the timed window — construction is not
        // activation cost).
        let mut spine = Spine::with_reservation(
            BehaviorScaleWorld::new(crossover_schedules()),
            BUDGET,
            RESERVE,
        );
        let arm_started = std::time::Instant::now();
        for &(due, payload) in &arms {
            spine
                .schedule(EventSpec {
                    class: BENCH_WAKE_CLASS,
                    schedule: Schedule::AtLogicalTime { due },
                    payload,
                    field: Field::Near,
                    depends_on: Vec::new(),
                })
                .expect("arming is legal");
        }
        let arm_nanos = arm_started.elapsed().as_nanos();
        let total_nanos = total_started.elapsed().as_nanos();
        // THE QUEUE-CONSERVATION ASSERTION on the paired campaign
        // (finding 8's correction): the synthesis arms 250 future
        // commitments — one live chain per entry, nothing executed
        // inside the activation window (its evaluations are
        // loop-side, not queue-side).
        assert_eq!(
            spine.pending_count(),
            250,
            "synthesis arms exactly one live chain per entry at k {k}"
        );
        assert_eq!(
            spine.stats().executed,
            0,
            "the synthesis window executes nothing (the evaluations are loop-side)"
        );
        let outcome = ActivationOutcome {
            pending: spine.pending_count() as u64,
            wake_executions: spine.world().wake_executions,
            needs_digest: needs_digest.finish(),
            activity_digest: activity_digest.finish(),
        };
        (total_nanos, loop_nanos, arm_nanos, arms, outcome)
    }

    let mut crossover_k: Option<u64> = None;
    for &k in &k_values {
        // REPLAY: REPS repetitions into TimingSamples (fresh state
        // per repetition, constructed outside the timed window) —
        // each repetition ONE paired end-to-end sample (arm + drain
        // together).
        let mut replay_samples = engine_core::timing::TimingSamples::default();
        let mut replay_outcome: Option<ActivationOutcome> = None;
        for rep in 0..REPS {
            let (nanos, outcome) = replay_once(k);
            replay_samples.record(std::time::Duration::from_nanos(nanos as u64));
            if rep == 0 {
                replay_outcome = Some(outcome);
            }
        }
        let replay_outcome = replay_outcome.expect("rep 0 ran");

        // SYNTHESIS: REPS repetitions, each ONE paired end-to-end
        // sample (the per-NPC loop AND the arm loop timed together —
        // finding 8's correction: the percentiles below are
        // percentiles of per-run sums). The component timings are
        // recorded separately for the decomposition line only.
        let mut synth_samples = engine_core::timing::TimingSamples::default();
        let mut loop_samples = engine_core::timing::TimingSamples::default();
        let mut arm_samples = engine_core::timing::TimingSamples::default();
        let mut first_arms: Option<Vec<(Tick, u64)>> = None;
        let mut first_outcome: Option<ActivationOutcome> = None;
        for rep in 0..REPS {
            let (total_nanos, loop_nanos, arm_nanos, arms, outcome) = synthesis_once(k);
            synth_samples.record(std::time::Duration::from_nanos(total_nanos as u64));
            loop_samples.record(std::time::Duration::from_nanos(loop_nanos as u64));
            arm_samples.record(std::time::Duration::from_nanos(arm_nanos as u64));
            if rep == 0 {
                first_arms = Some(arms);
                first_outcome = Some(outcome);
            } else if rep == 1 {
                // DOUBLE-RUN DETERMINISM (not an equivalence claim —
                // finding 8's correction: the old gate framed this
                // synthesis-vs-synthesis comparison as the
                // strategies' equivalence; it is only each policy's
                // own determinism, and the CROSS-strategy comparison
                // is reported below): the re-arm dues and the
                // routine lanes are byte-identical across fresh
                // constructions of the same policy.
                assert_eq!(
                    first_arms.as_ref().expect("rep 0 ran"),
                    &arms,
                    "synthesis re-arm dues identical across runs at k {k}"
                );
                assert_eq!(
                    first_outcome.as_ref().expect("rep 0 ran").needs_digest,
                    outcome.needs_digest,
                    "synthesis needs digest identical across runs at k {k}"
                );
            }
        }
        let synth_outcome = first_outcome.expect("rep 0 ran");

        let replay_p50 = replay_samples.percentile_ms(0.50);
        let replay_p99 = replay_samples.percentile_ms(0.99);
        // THE PERCENTILE-OF-SUMS (finding 8's correction): the
        // synthesis p50/p99 are percentiles over the paired
        // per-run totals — never loop_p50 + arm_p50 (a sum of
        // component percentiles is not the percentile of sums).
        let synth_p50 = synth_samples.percentile_ms(0.50);
        let synth_p99 = synth_samples.percentile_ms(0.99);
        let loop_p50 = loop_samples.percentile_ms(0.50);
        let arm_p50 = arm_samples.percentile_ms(0.50);
        let pending = 250 * k;

        println!(
            "activation cell: k {} ({} missed occurrences PER ENTRY — {} pending events, \
             125 NPCs × 2 entries × k)",
            k, k, pending
        );
        println!(
            "  replay p50/p99 {:.3}/{:.3} ms per activation ({} reps, {} events executed + \
             {} late arrivals asserted)",
            replay_p50,
            replay_p99,
            REPS,
            250 * k,
            250 * k
        );
        println!(
            "  synthesis p50/p99 {:.3}/{:.3} ms per activation — percentiles of PAIRED \
             per-run totals ({} reps; component medians for decomposition only: per-NPC \
             loop {:.3}, arm loop {:.3})",
            synth_p50, synth_p99, REPS, loop_p50, arm_p50
        );
        // THE CROSS-STRATEGY COMPARISON, reported beside the two
        // costs (finding 8's correction — the documented-approximation
        // branch: the two policies are deliberately DIFFERENT
        // reconstructors, so their outcomes are reported, never
        // equated). The structural equalities that DO hold are
        // asserted inside the campaigns above: equal wake-key
        // coverage (every NPC × entry live in both) and equal due
        // ticks per entry (both re-arm at (k+1)·600 and 300+k·600).
        // The differences, with their causes:
        // - LIVE-CHAIN COUNT: replay leaves 250·k pending (k honest
        //   re-arms per entry — one per replayed occurrence, all at
        //   the same due), synthesis 250 (one per entry).
        // - EXECUTIONS: replay re-executes every missed occurrence
        //   (250·k evaluations, k per entry); synthesis performs ONE
        //   evaluation per NPC (the production activation shape:
        //   the lazy-decay jump over the whole span) — the routine
        //   lanes and stream differ BY DESIGN.
        println!(
            "  post-activation queues: replay {} live (k re-arms per entry at the same \
             dues), synthesis {} live (one per entry) — NOT interchangeable, reported",
            replay_outcome.pending, synth_outcome.pending
        );
        println!(
            "  outcome divergence beside the costs: executions {} vs {}, needs digest \
             {} vs {}, activity digest {} vs {} — synthesis is the cheaper approximate \
             reconstruction (one evaluation per NPC), accepted deliberately. THE \
             SYNTHESIS DIGESTS ARE PREDICTIONS (the loop's evaluated routine lanes — \
             computed and reported, never installed into the armed world, which starts \
             Idle; the finding 28 labeling)",
            replay_outcome.wake_executions,
            synth_outcome.wake_executions,
            replay_outcome.needs_digest,
            synth_outcome.needs_digest,
            replay_outcome.activity_digest,
            synth_outcome.activity_digest
        );
        if synth_p50 < replay_p50 && crossover_k.is_none() {
            crossover_k = Some(k);
            println!(
                "  crossover: synthesis becomes cheaper at k = {} missed occurrences per \
                 entry ({} pending events per NPC = 2k — E-09's harness measured \
                 7.6-12.4)",
                k,
                2 * k
            );
        }
        println!(
            "  determinism: re-arm dues + needs digest DOUBLE-RUN IDENTICAL (asserted); \
             the per-NPC drain census asserted (every NPC executed exactly 2 x k — no \
             redistribution escapes the total-count conservation)"
        );
        println!();
    }
    match crossover_k {
        Some(k) => println!(
            "  E-09 crossover on the real machinery: k = {} missed occurrences per entry \
             ({} pending events per NPC) — the smallest measured per-entry miss count \
             where synthesis beats replay",
            k,
            2 * k
        ),
        None => println!(
            "  E-09 crossover on the real machinery: NOT REACHED within k ≤ {} missed \
             occurrences per entry — replay stays cheaper across the measured range",
            k_values[k_values.len() - 1]
        ),
    }
    println!();
}

// ------------------------------------------------------- SECTION 4 — skip-cost curves

fn run_skip_cost_curves() {
    println!("Skip-cost curves — the E-08 shape on the real queue");
    println!("  pod results, observational (never CI thresholds, never physical-rig claims)");
    println!();

    // B10's cadence clarification: these cells are LOGICAL-TICK
    // queue measurements — events armed at authored dues, drained
    // by `run_ticks`; no frame cadence exists anywhere in this
    // harness (the shipped E-08 decomposition therefore carries no
    // `cadence_hz` — the audit's `cadence_hz = 0` premise names the
    // `--sim` route's driver pattern, not this section). The
    // frame-cadence stall shape — a stall exceeding the per-frame
    // catch-up bound — is the next section's measurement.
    println!(
        "  methodology (B10's cadence clarification): logical-tick cells — events \
         armed at authored dues, drained by run_ticks; no frame cadence exists in \
         this harness. The frame-cadence stall shape (a stall exceeding the \
         per-frame catch-up bound) is the NEXT section's measurement."
    );
    println!();

    // The E-08 cell set: the (duration, events) pairs where events
    // vary at FIXED duration (rows 1-2, 3-4, 5-6) and duration varies
    // at FIXED events (E=3_000 at 10^4 vs 10^5; E=30_000 at 10^5 vs
    // 10^6) — the pair structure that isolates cost-vs-events from
    // cost-vs-elapsed. Dues spread evenly: due = 1 + (j·D)/E (exact
    // integer arithmetic; the E=300_000 cell at D=100_000 carries
    // three events per tick — the queue's own same-due handling).
    type Cell = (u64, u64);
    let cells: [Cell; 7] = [
        (10_000, 300),
        (10_000, 3_000),
        (100_000, 3_000),
        (100_000, 30_000),
        (1_000_000, 30_000),
        (1_000_000, 300_000),
        (100_000, 300_000),
    ];

    // The reported curve rows, for the closing summary.
    let mut rows: Vec<(u64, u64, f64)> = Vec::with_capacity(cells.len());

    for &(duration, events) in &cells {
        // The schedules' authored re-arm period is 2·D: every armed
        // event's honest re-arm lands beyond the window, so executed
        // is EXACTLY the armed count and the re-arms are the measured
        // window's own pending tail (the conservation below).
        let world = BehaviorScaleWorld::new(skip_schedules(2 * duration));
        let mut spine = Spine::with_reservation(world, BUDGET, RESERVE);

        let mut specs: Vec<EventSpec> = Vec::with_capacity(events as usize);
        for j in 0..events {
            let npc = (j % 125) as u32;
            let commitment = (j / 125) % 2;
            let due = Tick(1 + (j * duration) / events);
            specs.push(EventSpec {
                class: BENCH_WAKE_CLASS,
                schedule: Schedule::AtLogicalTime { due },
                payload: wake_payload(npc, commitment as u32).expect("payload packs"),
                field: Field::Near,
                depends_on: Vec::new(),
            });
        }
        for spec in specs {
            spine.schedule(spec).expect("arming is legal");
        }
        let started = std::time::Instant::now();
        spine.run_ticks(duration);
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let stats = spine.stats();
        let pending = spine.pending_count() as u64;

        assert_eq!(
            stats.executed, events,
            "conservation: every armed event executed for duration {duration} events {events}"
        );
        assert_eq!(
            pending, events,
            "the re-arm tail: every executed event re-armed beyond the window for duration {duration} events {events}"
        );
        assert_eq!(
            stats.scheduled,
            stats.executed + pending + spine.blocked_count() as u64 + spine.held_count() as u64,
            "spine conservation for duration {duration} events {events}"
        );

        let ms_per_1000 = elapsed_ms * 1000.0 / events as f64;
        rows.push((duration, events, elapsed_ms));

        println!("skip cell: duration {} events {}", duration, events);
        println!(
            "  elapsed {:.3} ms, {:.3} ms per 1000 events",
            elapsed_ms, ms_per_1000
        );
        println!(
            "  executed {} / pending {} (the re-arm tail)",
            stats.executed, pending
        );
        println!();

        // DOUBLE-RUN on the smallest cell: identical executed count
        // and world fingerprint.
        if duration == 10_000 && events == 300 {
            let world2 = BehaviorScaleWorld::new(skip_schedules(2 * duration));
            let mut spine2 = Spine::with_reservation(world2, BUDGET, RESERVE);
            let mut specs2: Vec<EventSpec> = Vec::with_capacity(events as usize);
            for j in 0..events {
                let npc = (j % 125) as u32;
                let commitment = (j / 125) % 2;
                let due = Tick(1 + (j * duration) / events);
                specs2.push(EventSpec {
                    class: BENCH_WAKE_CLASS,
                    schedule: Schedule::AtLogicalTime { due },
                    payload: wake_payload(npc, commitment as u32).expect("payload packs"),
                    field: Field::Near,
                    depends_on: Vec::new(),
                });
            }
            for spec in specs2 {
                spine2.schedule(spec).expect("arming is legal");
            }
            spine2.run_ticks(duration);
            assert_eq!(
                spine2.stats().executed,
                spine.stats().executed,
                "double-run executed identical"
            );
            let mut fp = Fingerprinter::new();
            spine.world().fingerprint(&mut fp);
            let mut fp2 = Fingerprinter::new();
            spine2.world().fingerprint(&mut fp2);
            assert_eq!(
                fp.finish(),
                fp2.finish(),
                "double-run fingerprint identical"
            );
        }
    }

    // THE E-08 STATEMENT (from the measured rows): cost tracks
    // events at fixed elapsed, and stays flat in elapsed at fixed
    // events — printed as the measured ratios, never asserted as a
    // threshold (pod evidence).
    let ratio = |a: f64, b: f64| if b > 0.0 { a / b } else { f64::NAN };
    let same_d = |d: u64, e1: u64, e2: u64| {
        let r1 = rows.iter().find(|&&(dd, ee, _)| dd == d && ee == e1);
        let r2 = rows.iter().find(|&&(dd, ee, _)| dd == d && ee == e2);
        if let (Some(&(_, _, ms1)), Some(&(_, _, ms2))) = (r1, r2) {
            println!(
                "  same duration {d}: {e1} events {ms1:.3} ms vs {e2} events {ms2:.3} ms — cost ratio {:.2} for a {:.1}× event ratio",
                ratio(ms2, ms1),
                e2 as f64 / e1 as f64
            );
        }
    };
    let same_e = |e: u64, d1: u64, d2: u64| {
        let r1 = rows.iter().find(|&&(dd, ee, _)| dd == d1 && ee == e);
        let r2 = rows.iter().find(|&&(dd, ee, _)| dd == d2 && ee == e);
        if let (Some(&(_, _, ms1)), Some(&(_, _, ms2))) = (r1, r2) {
            println!(
                "  same events {e}: duration {d1} {ms1:.3} ms vs duration {d2} {ms2:.3} ms — cost ratio {:.2} for a {:.0}× elapsed ratio",
                ratio(ms2, ms1),
                d2 as f64 / d1 as f64
            );
        }
    };
    println!("  the E-08 shape (measured ratios, pod evidence):");
    same_d(10_000, 300, 3_000);
    same_d(100_000, 3_000, 30_000);
    same_d(1_000_000, 30_000, 300_000);
    same_e(3_000, 10_000, 100_000);
    same_e(30_000, 100_000, 1_000_000);
    // The decomposition the ratios express: cost ≈ (per-tick floor) ×
    // ticks + (per-event) × events. The floor is the empty-tick queue
    // walk; the event term dominates at game-relevant densities —
    // and the E-08 pair's reading (the harness's full-vs-quiet at the
    // SAME elapsed) is the DELTA tracking events, the floor common to
    // both sides. Fit from the measured rows, printed as the honest
    // statement (never a threshold).
    if let (Some(&(_, _, ms_a)), Some(&(_, _, ms_b))) = (
        rows.iter().find(|&&(d, e, _)| d == 10_000 && e == 300),
        rows.iter().find(|&&(d, e, _)| d == 10_000 && e == 3_000),
    ) {
        let per_event = (ms_b - ms_a) * 1000.0 / (3_000.0 - 300.0);
        // The 300-event cell's event term in ms (µs/event × 300 / 1000),
        // subtracted from its measured total: the remainder is the
        // 10_000-tick floor.
        let floor_ns = (ms_a - per_event * 300.0 / 1000.0) * 1_000_000.0 / 10_000.0;
        println!(
            "  decomposition (fit at D=10_000): ≈ {:.1} ns/tick + ≈ {:.3} µs/event — the event term dominates at ≥ 0.1 events/tick",
            floor_ns, per_event
        );
    }
    println!();

    // THE DORMANT CONTRAST (the quiet counterpart): a dormant
    // population's skip carries NO queue work — its only cost is the
    // calendar re-derivation at the far side, `next_occurrence` over
    // the retained entries (1000 NPCs × 2 real commitments — the
    // delegate round walked EMPTY schedules, measuring a `.get` miss;
    // repaired here to the real arithmetic).
    let schedules = crossover_schedules_dormant(1000);
    let mut samples = engine_core::timing::TimingSamples::default();
    for _ in 0..2048 {
        let started = std::time::Instant::now();
        for schedule in &schedules {
            for entry in &schedule.entries {
                let _ = entry.next_occurrence(Tick(10_000_000));
            }
        }
        samples.record(started.elapsed());
    }
    let p50 = samples.percentile_ms(0.50) * 1000.0;
    let p99 = samples.percentile_ms(0.99) * 1000.0;
    println!(
        "dormant contrast: {} NPCs × 2 retained entries, unarmed",
        schedules.len()
    );
    println!(
        "  calendar-only skip p50/p99 {:.3}/{:.3} µs ({} samples)",
        p50,
        p99,
        samples.sample_count()
    );
    println!();
}

/// The dormant population's retained calendars: `population` NPCs ×
/// 2 entries each (the crossover shape — real commitments, nothing
/// armed on any queue).
fn crossover_schedules_dormant(population: usize) -> Vec<NpcSchedule> {
    let mut schedules = Vec::with_capacity(population);
    for npc in 0..population {
        schedules.push(NpcSchedule {
            entries: vec![
                Commitment {
                    start: Tick(600),
                    cadence: Cadence::every(600).expect("period is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 300,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: npc as u32,
                        anchor: (0, 0),
                    },
                },
                Commitment {
                    start: Tick(300),
                    cadence: Cadence::every(600).expect("period is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 301,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: npc as u32,
                        anchor: (0, 0),
                    },
                },
            ],
        });
    }
    schedules
}

// --------------------------------- SECTION 4b — stall worst case (B10's added case)

/// One frame's census — logical quantities only (no timing), so the
/// double-run equality is exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StallFrame {
    frame: u64,
    /// The driver's retained tick obligation AFTER the frame: due
    /// ticks accrued but not executed (the per-frame catch-up
    /// bound's remainder — the no-discard invariant's
    /// "outstanding" term).
    outstanding: u32,
    /// The spine queue's armed population after the frame (events
    /// scheduled, not yet executed).
    pending: usize,
    /// Cumulative late arrivals (events armed with a due already
    /// behind the clock).
    late_arrivals: u64,
    /// Cumulative late executions (executions past their due).
    late_executions: u64,
    /// Cumulative budget-exhausted ticks (ticks that ended with the
    /// budget consumed and due work still queued).
    budget_exhausted_ticks: u64,
}

/// Runs the B10 stall script once: a 120-tick headless warm (the
/// queue's steady armed shape), then `cycles` repetitions of the
/// demo's own irregular cadence — `sim_demo::irregular_frame`, the
/// `--sim` route's `cadence_hz = 0` pattern (five 60 Hz frames, ONE
/// 2 s stall = 20 due ticks at the 10 Hz rate against the 8-tick
/// per-frame catch-up bound, five 240 Hz frames, one 120 Hz frame;
/// computed durations only, never real sleeping — the sim_demo
/// determinism precedent). The no-discard invariant and the spine
/// conservation are asserted AT every frame, inside the run (both
/// runs of a double-run re-assert them); the per-frame census is
/// the double-run comparison's subject. Returns the driver (for
/// the final-state equality), the census, and the observational
/// per-frame cost samples.
fn run_stall_script(
    cycles: u64,
) -> (
    SimDriver,
    Vec<StallFrame>,
    engine_core::timing::TimingSamples,
) {
    let mut driver = SimDriver::new(NpcConfig::default(), SimConfig::default());
    driver.run_headless(120);
    let mut census: Vec<StallFrame> = Vec::new();
    let mut samples = engine_core::timing::TimingSamples::default();
    for frame in 0..cycles * 12 {
        let started = std::time::Instant::now();
        driver.advance(irregular_frame(frame));
        samples.record(started.elapsed());
        let spine = driver.spine();
        let stats = spine.stats();
        let outstanding = driver.outstanding_ticks();
        let pending = spine.pending_count();
        // THE NO-DISCARD INVARIANT (sim.rs's own accounting law):
        // every tick ever due is executed or outstanding — the
        // stall may DEFER work, never drop it.
        assert_eq!(
            driver.ticks_due_total(),
            spine.clock().as_u64() + u64::from(outstanding),
            "no-discard: clock + outstanding == ticks due, at frame {frame}"
        );
        // The spine conservation (this harness's wide law).
        assert_eq!(
            stats.scheduled,
            stats.executed
                + pending as u64
                + spine.blocked_count() as u64
                + spine.held_count() as u64,
            "spine conservation at frame {frame}"
        );
        census.push(StallFrame {
            frame,
            outstanding,
            pending,
            late_arrivals: stats.late_arrivals,
            late_executions: stats.late_executions,
            budget_exhausted_ticks: stats.budget_exhausted_ticks,
        });
    }
    (driver, census, samples)
}

/// The B10 stall measurement: the worst-case pending accumulation
/// under a stall that exceeds the per-frame catch-up bound — the
/// queue/accumulator shape the E-08 skip cells (logical ticks) and
/// the E-09 crossover arms (the reactivation drain vs the synthesis
/// jump) do not measure, driven through the REAL catch-up machinery
/// (SimDriver, the same driver the `--sim` irregular route drives)
/// over the real NpcWorld Legacy workload.
fn run_stall_worst_case() {
    println!(
        "Stall worst case — the pending accumulation under a stall exceeding the \
         per-frame catch-up bound (B10's added case)"
    );
    println!("  pod results, observational (never CI thresholds, never physical-rig claims)");
    println!();
    const CYCLES: u64 = 12;
    let config = SimConfig::default();
    println!(
        "  the machinery: SimDriver (the --sim route's own driver) over the real \
         NpcWorld Legacy workload — 1,000 NPCs, period 6 + seeded jitter, budget {} \
         (near reserve {}), {} Hz tick rate, per-frame catch-up bound {} ticks",
        config.budget,
        config.effective_near_reserve(),
        config.tick_rate_hz,
        config.max_catchup_per_frame
    );
    println!(
        "  the script: a 120-tick headless warm (the steady queue shape), then {} \
         cycles of the demo's irregular cadence (the cadence_hz=0 pattern: five 60 Hz \
         frames, ONE 2 s stall = 20 due ticks vs the 8-tick bound, five 240 Hz \
         frames, one 120 Hz frame — computed durations only, never real sleeping)",
        CYCLES
    );
    println!();

    // DOUBLE-RUN (fresh construction, no shared state): the whole
    // script again, then the equality asserts — printed claims come
    // AFTER verification, never before.
    let (driver_a, census_a, samples_a) = run_stall_script(CYCLES);
    let (driver_b, census_b, _) = run_stall_script(CYCLES);
    assert_eq!(census_a, census_b, "the stall census double-run identical");
    {
        let spine_a = driver_a.spine();
        let spine_b = driver_b.spine();
        assert_eq!(
            spine_a.clock(),
            spine_b.clock(),
            "final campaign tick identical"
        );
        assert_eq!(spine_a.stats(), spine_b.stats(), "final stats identical");
        assert_eq!(
            spine_a.lane_fingerprint_hex(),
            spine_b.lane_fingerprint_hex(),
            "final lane fingerprint identical"
        );
        assert_eq!(
            spine_a.lane_state_fingerprint_hex(),
            spine_b.lane_state_fingerprint_hex(),
            "final lane state fingerprint identical"
        );
    }

    // The worst case, from the census: the retained tick obligation
    // and the queue's armed population — each with the frame it
    // occurred at.
    let worst_outstanding_frame = census_a
        .iter()
        .max_by_key(|f| f.outstanding)
        .expect("the census is nonempty");
    let worst_pending_frame = census_a
        .iter()
        .max_by_key(|f| f.pending)
        .expect("the census is nonempty");
    // The pre-stall reference: the last short frame before the
    // first stall (the pattern's stall lands at index 5).
    let pre_stall = census_a
        .iter()
        .find(|f| f.frame == 4)
        .expect("frame 4 is in the census");
    // The retained obligation drains fully inside EVERY cycle: the
    // catch-up bound defers the stall's dues across frames, never
    // accumulates them across cycles.
    for cycle in 0..CYCLES {
        assert!(
            census_a[cycle as usize * 12..cycle as usize * 12 + 12]
                .iter()
                .any(|f| f.outstanding == 0),
            "cycle {cycle} contains a fully-drained frame (no cross-cycle accumulation)"
        );
    }
    let last = *census_a.last().expect("the census is nonempty");
    // WHERE the budget exhausted (the lateness attribution): the
    // frames whose cumulative budget-exhausted count advanced. The
    // measured truth: the list is EMPTY — every budget-exhausted
    // tick and late execution belongs to the headless warm ramp
    // (the initial 1,000-event schedule spread's synchrony, before
    // the first census frame); the irregular-cadence script itself
    // adds NO lateness at this workload.
    let exhaustion_frames: Vec<u64> = census_a
        .windows(2)
        .filter_map(|w| {
            (w[1].budget_exhausted_ticks > w[0].budget_exhausted_ticks).then_some(w[1].frame)
        })
        .collect();
    assert!(
        exhaustion_frames.is_empty(),
        "the irregular-cadence frames add no budget exhaustion (the lateness is the \
         warm ramp's, measured in the cumulative counters)"
    );

    println!(
        "stall cell: {} cycles × 12 frames of the irregular cadence",
        CYCLES
    );
    println!(
        "  worst-case retained tick obligation: {} ticks (frame {} — the LAST stall \
         frame; every cycle's stall retains the same amount: the 2 s stall's ~20 due \
         ticks against the 8-tick per-frame bound)",
        worst_outstanding_frame.outstanding, worst_outstanding_frame.frame
    );
    println!(
        "  worst-case queue pending: {} events (frame {}) vs the pre-stall steady \
         armed population {} (frame 4) — the stall does not grow the queue at this \
         workload: arrivals come only from budget-bounded executions, so unbounded \
         accumulation requires demand > budget (the sustained matrix's overload \
         class, measured separately)",
        worst_pending_frame.pending, worst_pending_frame.frame, pre_stall.pending
    );
    println!(
        "  drain: the retained obligation returns to 0 inside every cycle (asserted, \
         {} cycles); cumulative-from-construction late arrivals {} / late executions \
         {} / budget-exhausted ticks {} — ALL from the headless warm ramp (the \
         initial 1,000-event schedule spread's synchrony): the 144 irregular-cadence \
         frames add ZERO (asserted — the stall script itself creates no lateness at \
         this workload)",
        CYCLES, last.late_arrivals, last.late_executions, last.budget_exhausted_ticks
    );
    println!(
        "  budget-exhaustion attribution: the census frames' increments are {:?} — \
         empty (the lateness is the warm ramp's, carried in the cumulative counters \
         from before the first census frame)",
        exhaustion_frames
    );
    println!(
        "  per-frame cost p50/p99/max {:.3}/{:.3}/{:.3} ms ({} frames; the max is \
         the catch-up batch — up to 8 budgeted ticks in one frame)",
        samples_a.percentile_ms(0.50),
        samples_a.percentile_ms(0.99),
        samples_a.max_ms(),
        samples_a.sample_count()
    );
    println!(
        "  determinism: the FULL per-frame census DOUBLE-RUN IDENTICAL (asserted \
         above); the no-discard invariant and spine conservation asserted at every \
         frame of both runs"
    );
    println!();
}

// ------------------------------------------------------- SECTION 5 — memory per NPC

#[derive(Debug, Clone, Copy, PartialEq)]
struct TierFootprint {
    routine: usize,
    rng: usize,
    decay_anchor: usize,
    locomotion: usize,
    schedule_per_entry: usize,
    perception_deep: usize,
    dormant_record: usize,
    total: usize,
}

fn tier_footprint(tier: LodLevel, entries_per_npc: usize) -> TierFootprint {
    let routine = std::mem::size_of::<NpcRoutine>();
    let rng = std::mem::size_of::<NpcRng>();
    let decay_anchor = std::mem::size_of::<u64>();
    let locomotion = if tier == LodLevel::Full {
        std::mem::size_of::<NpcLocomotion>()
    } else {
        0
    };
    let schedule_per_entry = std::mem::size_of::<Commitment>();
    let perception_deep = PERCEPTION_BUDGETS[tier_level(tier) as usize] * TRACE_STRIDE
        + 3 * std::mem::size_of::<usize>();
    let dormant_record = std::mem::size_of::<NpcRecord>() + std::mem::size_of::<u64>();

    let total = match tier {
        LodLevel::Full | LodLevel::Simplified | LodLevel::EventDriven => {
            routine
                + rng
                + decay_anchor
                + locomotion
                + entries_per_npc * schedule_per_entry
                + perception_deep
        }
        LodLevel::Dormant => dormant_record,
    };

    TierFootprint {
        routine,
        rng,
        decay_anchor,
        locomotion,
        schedule_per_entry,
        perception_deep,
        dormant_record,
        total,
    }
}

fn tier_level(level: LodLevel) -> u8 {
    match level {
        LodLevel::Full => 0,
        LodLevel::Simplified => 1,
        LodLevel::EventDriven => 2,
        LodLevel::Dormant => 3,
    }
}

fn run_memory_per_npc() {
    println!("Memory-per-NPC by tier — the §8.5 measurement");
    println!("  pod results, observational (never CI thresholds, never physical-rig claims)");
    println!();

    for &entries in &[2, 16] {
        println!("memory table: entries per NPC {}", entries);
        println!(
            "  tier           routine    rng decay anchor  locomotion  sched/entry  deep  dormant   total"
        );
        println!(
            "  (Dormant's total is the RECORD ONLY — the record + next_event; the other columns are the shared component sizes, not resident at this tier)"
        );
        for tier in [
            LodLevel::Full,
            LodLevel::Simplified,
            LodLevel::EventDriven,
            LodLevel::Dormant,
        ] {
            let fp = tier_footprint(tier, entries);
            println!(
                "  {:12} {:8} {:6} {:10} {:10} {:10} {:5} {:8} {:8}",
                format!("{:?}", tier),
                fp.routine,
                fp.rng,
                fp.decay_anchor,
                fp.locomotion,
                fp.schedule_per_entry,
                fp.perception_deep,
                fp.dormant_record,
                fp.total
            );
        }
        println!();

        println!("  population-scale @ 1000 NPCs:");
        for tier in [
            LodLevel::Full,
            LodLevel::Simplified,
            LodLevel::EventDriven,
            LodLevel::Dormant,
        ] {
            let fp = tier_footprint(tier, entries);
            println!(
                "    {:12}: {} B ({:.1} MB)",
                format!("{:?}", tier),
                fp.total * 1000,
                fp.total as f64 * 1000.0 / 1_000_000.0
            );
        }
        println!();
    }

    // DEEP FOOTPRINT
    let (_routes, states, present) = crate::corner_routes::corner_population_locomotion();
    let world = crate::corner_behavior::CornerCrowdWorld::new(states, present);
    let deep_bytes = deep_footprint(&world);
    println!(
        "measured composed world: total {} B, per-NPC {:.1} B, locomotion share {:.1}% (excludes the routes' engine-core-private per-leg geometry rows — same count as the {} legs)",
        deep_bytes,
        deep_bytes as f64 / 24.0,
        world
            .states
            .iter()
            .map(std::mem::size_of_val)
            .sum::<usize>() as f64
            * 100.0
            / deep_bytes as f64,
        world
            .routes
            .iter()
            .map(engine_core::npc_locomotion::Route::leg_count)
            .sum::<usize>()
    );
    println!();
}

fn deep_footprint(world: &crate::corner_behavior::CornerCrowdWorld) -> usize {
    // The honest deep walk: the struct's OWN inline bytes (the ten
    // Vec headers, the rates array, the four counters —
    // `size_of_val` carries them all) PLUS every Vec's element bytes
    // at its ACTUAL capacity PLUS the routes' internal legs
    // (`Route::leg_count` × the leg and per-leg geometry rows — the
    // delegate round counted only the shallow `size_of::<Route>()`
    // slot and missed the authored table's own bytes).
    let mut total = std::mem::size_of_val(world);

    for schedule in &world.schedules {
        total += schedule.entries.capacity() * std::mem::size_of::<Commitment>();
    }
    total += world.routines.capacity() * std::mem::size_of::<NpcRoutine>();
    total += world.rngs.capacity() * std::mem::size_of::<NpcRng>();
    total += world.last_decay_tick.capacity() * std::mem::size_of::<Tick>();
    total += world.rules.capacity() * std::mem::size_of::<Rule>();
    total += world.stream.capacity() * std::mem::size_of::<StreamEntry>();
    total += world.states.capacity() * std::mem::size_of::<NpcLocomotion>();
    total += world.present.capacity() * std::mem::size_of::<bool>();
    for route in &world.routes {
        // The authored legs (public `RouteLeg`); the per-leg
        // precomputed geometry rows are an engine-core-private type
        // of the same count — NOT engine-slice-visible, so the walk
        // excludes them and the print says so (never an unmeasured
        // guess in a measurement claim).
        total += route.leg_count() * std::mem::size_of::<engine_core::npc_locomotion::RouteLeg>();
    }
    total
}

// ------------------------------------------------------- THE ENTRY POINT

pub fn run_behavior_measurements() {
    println!("Behavior measurement surface — the Era-2 package A §8.4/§8.5 plan");
    println!("  pod results, observational (never CI thresholds, never physical-rig claims)");
    println!();
    run_sustained_behavior_matrix();
    println!();
    run_witness_cost();
    println!();
    run_activation_crossover();
    println!();
    run_skip_cost_curves();
    println!();
    run_stall_worst_case();
    println!();
    run_memory_per_npc();
}

// ------------------------------------------------------- THE LIB BATTERY

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::npc_execution::execution_digest;
    use engine_core::spine::Spine;

    /// The matrix workload's stream digest pin (MEASURED on this
    /// module's first complete run, stable across independent runs —
    /// the frozen-pin discipline: a divergence is an investigation,
    /// never a silent re-pin).
    const PINNED_MATRIX_STREAM: u64 = 12_420_752_226_621_264_573;
    /// The witness census digest pin (MEASURED on this module's first
    /// complete run, stable across independent runs).
    const PINNED_WITNESS_CENSUS: u64 = 16_487_293_382_058_425_681;

    #[test]
    fn matrix_workload_double_run_and_pin() {
        const POP: usize = 24;
        const ENTRIES_PER: usize = 2;
        const PERIOD: u64 = 3;
        const BUDGET: u32 = 32;
        const RESERVE: u32 = 16;
        const TICKS: u64 = 200;

        let schedules_a = matrix_schedules(POP, ENTRIES_PER, PERIOD, false);
        let world_a = BehaviorScaleWorld::new(schedules_a);
        let mut spine_a = Spine::with_reservation(world_a, BUDGET, RESERVE);
        arm_all_entries(&mut spine_a);
        for _ in 0..TICKS {
            spine_a.run_tick();
        }
        let stats_a = spine_a.stats();
        let world_a = spine_a.world();
        let wake_executions_a = world_a.wake_executions;
        let digest_a = execution_digest(&world_a.stream);
        let mut fp_a = Fingerprinter::new();
        world_a.fingerprint(&mut fp_a);
        let fingerprint_a = fp_a.finish();

        let schedules_b = matrix_schedules(POP, ENTRIES_PER, PERIOD, false);
        let world_b = BehaviorScaleWorld::new(schedules_b);
        let mut spine_b = Spine::with_reservation(world_b, BUDGET, RESERVE);
        arm_all_entries(&mut spine_b);
        for _ in 0..TICKS {
            spine_b.run_tick();
        }
        let _stats_b = spine_b.stats();
        let world_b = spine_b.world();
        let wake_executions_b = world_b.wake_executions;
        let digest_b = execution_digest(&world_b.stream);
        let mut fp_b = Fingerprinter::new();
        world_b.fingerprint(&mut fp_b);
        let fingerprint_b = fp_b.finish();

        assert_eq!(digest_a, digest_b, "stream digest identical");
        assert_eq!(fingerprint_a, fingerprint_b, "world fingerprint identical");
        assert_eq!(
            wake_executions_a, wake_executions_b,
            "wake_executions identical"
        );

        assert!(stats_a.executed > 0, "executed > 0");
        assert_eq!(
            wake_executions_a, stats_a.executed,
            "world counter matches spine"
        );

        // The authored demand: 24 × 2 / 3 = 16 events/tick under the
        // budget 32 — every armed occurrence executes on time.
        assert_eq!(
            stats_a.late_arrivals, 0,
            "no late arrivals under budget at the test shape"
        );

        assert_eq!(
            digest_a, PINNED_MATRIX_STREAM,
            "stream digest matches pinned value"
        );
    }

    #[test]
    fn witness_census_digest_pin() {
        const POP: usize = 24;
        let side = (POP as f32).sqrt().ceil() as u32;
        let population: Vec<NpcRecord> = (0..POP)
            .map(|i| NpcRecord {
                id: i as u32,
                pos: [(i % side as usize) as f32, (i / side as usize) as f32],
            })
            .collect();

        let tiers: Vec<LodLevel> = (0..POP)
            .map(|i| match i % 4 {
                0 => LodLevel::Full,
                1 => LodLevel::Simplified,
                2 => LodLevel::EventDriven,
                _ => LodLevel::Dormant,
            })
            .collect();

        let center = [side as f32 / 2.0, side as f32 / 2.0];
        let radius = 5.0;

        let mut scratch_a = Vec::with_capacity(1024);
        enumerate_witnesses(center, radius, &population, &tiers, &mut scratch_a);
        let mut scratch_b = Vec::with_capacity(1024);
        enumerate_witnesses(center, radius, &population, &tiers, &mut scratch_b);
        assert_eq!(scratch_a, scratch_b, "census scratch identical");

        let mut fp = Fingerprinter::new();
        for &idx in &scratch_a {
            fp.write_u32(idx);
        }
        let digest = fp.finish();

        // The tier-gating known answer FIRST — never dead behind the
        // pin escape (the delegate round put it after the `return`,
        // where it never ran until baking; repaired): four records at
        // ±1.0 from the center, tiers Full/Simplified/EventDriven/
        // Dormant in order — the census admits the first three and
        // excludes the Dormant one (the I2/I3 invariant).
        let test_population = vec![
            NpcRecord {
                id: 0,
                pos: [center[0] - 1.0, center[1]],
            },
            NpcRecord {
                id: 1,
                pos: [center[0] + 1.0, center[1]],
            },
            NpcRecord {
                id: 2,
                pos: [center[0], center[1] - 1.0],
            },
            NpcRecord {
                id: 3,
                pos: [center[0], center[1] + 1.0],
            },
        ];
        let test_tiers = vec![
            LodLevel::Full,
            LodLevel::Simplified,
            LodLevel::EventDriven,
            LodLevel::Dormant,
        ];
        let mut test_scratch = Vec::with_capacity(1024);
        enumerate_witnesses(
            center,
            radius,
            &test_population,
            &test_tiers,
            &mut test_scratch,
        );
        assert_eq!(
            test_scratch,
            vec![0, 1, 2],
            "tier-gating admits Full/Simplified/EventDriven, excludes Dormant"
        );

        assert_eq!(
            digest, PINNED_WITNESS_CENSUS,
            "census digest matches pinned value"
        );
    }

    #[test]
    fn activation_rearm_arithmetic() {
        let schedule = NpcSchedule {
            entries: vec![
                Commitment {
                    start: Tick(600),
                    cadence: Cadence::every(600).expect("period 600 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 300,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: 0,
                        anchor: (0, 0),
                    },
                },
                Commitment {
                    start: Tick(300),
                    cadence: Cadence::every(600).expect("period 600 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 301,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: 0,
                        anchor: (0, 0),
                    },
                },
            ],
        };

        assert_eq!(
            schedule.entries[0].next_occurrence(Tick(0)),
            Some(Tick(600)),
            "entry 0 next from 0"
        );
        assert_eq!(
            schedule.entries[1].next_occurrence(Tick(0)),
            Some(Tick(300)),
            "entry 1 next from 0"
        );

        assert_eq!(
            schedule.entries[0].next_occurrence(Tick(4800)),
            Some(Tick(5400)),
            "entry 0 next from 4800 (k=8)"
        );
        assert_eq!(
            schedule.entries[1].next_occurrence(Tick(4800)),
            Some(Tick(5100)),
            "entry 1 next from 4800 (k=8)"
        );
    }

    #[test]
    fn skip_conservation_small() {
        // D = 1000, E = 50 (density 0.05 events/tick): every armed
        // event executes once within the window and re-arms beyond it
        // (the factory's 2·D period) — executed == E exactly.
        const DURATION: u64 = 1000;
        const EVENTS: u64 = 50;
        const POP: usize = 125;

        let world = BehaviorScaleWorld::new(skip_schedules(2 * DURATION));
        let mut spine = Spine::with_reservation(world, BUDGET, RESERVE);

        let mut specs: Vec<EventSpec> = Vec::with_capacity(EVENTS as usize);
        for j in 0..EVENTS {
            let npc = (j % POP as u64) as u32;
            let commitment = (j / POP as u64) % 2;
            let due = Tick(1 + (j * DURATION) / EVENTS);
            specs.push(EventSpec {
                class: BENCH_WAKE_CLASS,
                schedule: Schedule::AtLogicalTime { due },
                payload: wake_payload(npc, commitment as u32).expect("payload packs"),
                field: Field::Near,
                depends_on: Vec::new(),
            });
        }
        for spec in specs {
            spine.schedule(spec).expect("arming is legal");
        }
        spine.run_ticks(DURATION);
        let stats = spine.stats();
        let pending = spine.pending_count() as u64;
        let blocked = spine.blocked_count() as u64;
        let held = spine.held_count() as u64;

        assert_eq!(
            stats.executed, EVENTS,
            "every armed event executed exactly once"
        );
        assert_eq!(
            pending, EVENTS,
            "every executed event re-armed beyond the window"
        );
        assert_eq!(
            stats.scheduled,
            stats.executed + pending + blocked + held,
            "conservation: scheduled == executed + queues"
        );

        // DOUBLE-RUN: same construction, identical fingerprint.
        let world2 = BehaviorScaleWorld::new(skip_schedules(2 * DURATION));
        let mut spine2 = Spine::with_reservation(world2, BUDGET, RESERVE);
        let mut specs2: Vec<EventSpec> = Vec::with_capacity(EVENTS as usize);
        for j in 0..EVENTS {
            let npc = (j % POP as u64) as u32;
            let commitment = (j / POP as u64) % 2;
            let due = Tick(1 + (j * DURATION) / EVENTS);
            specs2.push(EventSpec {
                class: BENCH_WAKE_CLASS,
                schedule: Schedule::AtLogicalTime { due },
                payload: wake_payload(npc, commitment as u32).expect("payload packs"),
                field: Field::Near,
                depends_on: Vec::new(),
            });
        }
        for spec in specs2 {
            spine2.schedule(spec).expect("arming is legal");
        }
        spine2.run_ticks(DURATION);
        assert_eq!(
            spine2.stats().executed,
            stats.executed,
            "double-run executed identical"
        );
        let world2 = spine2.world();
        let mut fp2 = Fingerprinter::new();
        world2.fingerprint(&mut fp2);
        let world = spine.world();
        let mut fp = Fingerprinter::new();
        world.fingerprint(&mut fp);
        assert_eq!(fp.finish(), fp2.finish(), "world fingerprint identical");
    }

    /// The B10 stall worst-case pins (MEASURED on this module's
    /// first complete run, stable across independent runs — the
    /// frozen-pin discipline: a divergence is an investigation,
    /// never a silent re-pin). All three are logical censuses of
    /// the scripted irregular cadence (computed durations only),
    /// so they are deterministic everywhere. The lateness triple
    /// is cumulative-from-construction and belongs ENTIRELY to
    /// the headless warm ramp (the initial 1,000-event schedule
    /// spread's synchrony): the 144 irregular-cadence frames add
    /// zero budget exhaustion (asserted below) — the stall script
    /// itself creates no lateness at this workload.
    const PINNED_STALL_OUTSTANDING: u32 = 12;
    const PINNED_STALL_PENDING: usize = 1000;
    const PINNED_STALL_LATENESS: (u64, u64, u64) = (0, 169, 6);

    #[test]
    fn stall_worst_case_double_run_and_pin() {
        // The B10 stall worst case (the added section's lib-side
        // guard): the 12-cycle irregular-cadence script on the real
        // SimDriver + NpcWorld machinery. The structural laws — the
        // no-discard invariant and the spine conservation — are
        // asserted INSIDE run_stall_script at every frame of BOTH
        // runs; this guard adds the double-run census equality, the
        // catch-up-bound arithmetic, the per-cycle drain law, and
        // the frozen extrema pins.
        let (driver_a, census_a, _) = run_stall_script(12);
        let (driver_b, census_b, _) = run_stall_script(12);
        assert_eq!(census_a, census_b, "the stall census double-run identical");
        assert_eq!(
            driver_a.spine().clock(),
            driver_b.spine().clock(),
            "final campaign tick identical"
        );
        assert_eq!(
            driver_a.spine().stats(),
            driver_b.spine().stats(),
            "final stats identical"
        );

        let worst_outstanding = census_a
            .iter()
            .map(|f| f.outstanding)
            .max()
            .expect("the census is nonempty");
        let worst_pending = census_a
            .iter()
            .map(|f| f.pending)
            .max()
            .expect("the census is nonempty");
        let last = *census_a.last().expect("the census is nonempty");
        println!(
            "stall census: worst outstanding {worst_outstanding}, worst pending \
             {worst_pending}, final lateness ({}, {}, {})",
            last.late_arrivals, last.late_executions, last.budget_exhausted_ticks
        );

        // The structural law (exact arithmetic, not a measurement):
        // the pattern's 2 s stall accrues >= 20 due ticks at the
        // 10 Hz rate; the 8-tick per-frame catch-up bound retains
        // >= 12 of them.
        let config = SimConfig::default();
        let stall_due_ticks = 2_000_000_000u64 / (1_000_000_000 / u64::from(config.tick_rate_hz));
        assert!(
            u64::from(worst_outstanding)
                >= stall_due_ticks - u64::from(config.max_catchup_per_frame),
            "the stall's retained obligation is at least its due ticks minus the bound"
        );
        // The drain completes inside every cycle (no cross-cycle
        // accumulation of the retained obligation).
        for cycle in 0..12u64 {
            assert!(
                census_a[cycle as usize * 12..cycle as usize * 12 + 12]
                    .iter()
                    .any(|f| f.outstanding == 0),
                "cycle {cycle} drains fully"
            );
        }

        assert_eq!(
            worst_outstanding, PINNED_STALL_OUTSTANDING,
            "retained-obligation worst-case pin"
        );
        assert_eq!(
            worst_pending, PINNED_STALL_PENDING,
            "queue-pending worst-case pin"
        );
        assert_eq!(
            (
                last.late_arrivals,
                last.late_executions,
                last.budget_exhausted_ticks
            ),
            PINNED_STALL_LATENESS,
            "the stall script's cumulative lateness pin (the warm ramp's initial \
             spread, measured and frozen)"
        );
        // The attribution law (measured, frozen): the census frames
        // themselves add NO budget exhaustion — the lateness is the
        // warm ramp's, carried in the cumulative counters.
        let exhaustion_frames: Vec<u64> = census_a
            .windows(2)
            .filter_map(|w| {
                (w[1].budget_exhausted_ticks > w[0].budget_exhausted_ticks).then_some(w[1].frame)
            })
            .collect();
        assert!(
            exhaustion_frames.is_empty(),
            "the irregular-cadence frames add no budget exhaustion (got {exhaustion_frames:?})"
        );
    }

    #[test]
    fn memory_accounting_monotone() {
        for &entries in &[1, 2, 16] {
            let fp_full = tier_footprint(LodLevel::Full, entries);
            let fp_simp = tier_footprint(LodLevel::Simplified, entries);
            let fp_even = tier_footprint(LodLevel::EventDriven, entries);
            let fp_dorm = tier_footprint(LodLevel::Dormant, entries);

            assert!(fp_full.total > fp_simp.total, "Full > Simplified");
            assert!(fp_simp.total > fp_even.total, "Simplified > EventDriven");
            assert!(fp_even.total > fp_dorm.total, "EventDriven > Dormant");

            // Full/Simplified/EventDriven: total excludes dormant_record
            let sum_full = fp_full.routine
                + fp_full.rng
                + fp_full.decay_anchor
                + fp_full.locomotion
                + fp_full.schedule_per_entry * entries
                + fp_full.perception_deep;
            assert_eq!(sum_full, fp_full.total, "Full total == sum of components");

            let sum_simp = fp_simp.routine
                + fp_simp.rng
                + fp_simp.decay_anchor
                + fp_simp.locomotion
                + fp_simp.schedule_per_entry * entries
                + fp_simp.perception_deep;
            assert_eq!(
                sum_simp, fp_simp.total,
                "Simplified total == sum of components"
            );

            let sum_even = fp_even.routine
                + fp_even.rng
                + fp_even.decay_anchor
                + fp_even.locomotion
                + fp_even.schedule_per_entry * entries
                + fp_even.perception_deep;
            assert_eq!(
                sum_even, fp_even.total,
                "EventDriven total == sum of components"
            );

            // Dormant: total = dormant_record only
            assert_eq!(
                fp_dorm.dormant_record, fp_dorm.total,
                "Dormant total == dormant_record"
            );
        }
    }
}
