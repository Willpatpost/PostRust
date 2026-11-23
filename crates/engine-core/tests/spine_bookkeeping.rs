//! The F4 scheduler-bookkeeping structural regression (Cycle 6-C):
//! pins the algorithmic properties that make the deferred-clause
//! cost linear and one-shot — the properties the measured-negligible
//! finding rests on. The wall-time evidence lives in the decision
//! record; THIS test pins what makes it true structurally, so a
//! regression back to quadratic or re-registration churn fails here
//! even on a machine where the timing difference is invisible.

use engine_core::fingerprint::Fingerprinter;
use engine_core::{EventClass, EventSpec, Schedule, Spine, Tick, World};

struct CountingWorld {
    executions: u64,
}

impl World for CountingWorld {
    fn execute(&mut self, _event: &engine_core::Event, _ctx: &mut engine_core::ExecCtx) {
        self.executions += 1;
    }
    fn fingerprint(&self, fp: &mut Fingerprinter) {
        fp.write_str("6c/structural/1");
        fp.write_u64(self.executions);
    }
}

/// Each due-but-blocked event parks EXACTLY ONCE across arbitrarily
/// many waiting ticks — `blocked_transitions` increments once per
/// blocked event, never once per waiting tick (the O(1) `last()`
/// check on the waiter lists plus the keep-existing-registrations
/// invariant). A regression to per-tick re-registration would show
/// blocked_transitions growing with the tick count.
#[test]
fn blocked_events_park_exactly_once_across_waiting_ticks() {
    let mut spine = Spine::new(CountingWorld { executions: 0 }, 1100);
    // One prerequisite at tick 50 (run_tick increments first, so
    // tick 50 executes when clock reaches 50).
    let prereq = spine
        .schedule(EventSpec::new(
            EventClass::new(1),
            Schedule::AtLogicalTime { due: Tick(50) },
            1,
        ))
        .expect("valid");
    // 1,000 dependents at tick 1.
    for i in 0..1000u64 {
        spine
            .schedule(
                EventSpec::new(
                    EventClass::new(2),
                    Schedule::AtLogicalTime { due: Tick(1) },
                    i,
                )
                .with_dependencies(vec![prereq]),
            )
            .expect("valid");
    }

    // 49 waiting ticks (clock 1-49): the dependents park at tick 1
    // and are re-checked on every subsequent drain, but the
    // blocked_transitions counter must NOT increment again.
    for _ in 0..49 {
        spine.run_tick();
    }
    assert_eq!(
        spine.stats().blocked_transitions,
        1000,
        "each blocked event parks exactly once — not once per tick"
    );
    assert_eq!(spine.stats().executed, 0);
    assert_eq!(spine.blocked_count(), 1000);

    // The release: tick 50 executes the prerequisite and all
    // dependents (budget 1100 > 1001).
    spine.run_tick();
    assert_eq!(spine.stats().executed, 1001);
    assert_eq!(spine.blocked_count(), 0);
    assert_eq!(
        spine.stats().blocked_transitions,
        1000,
        "the release adds no parking transitions"
    );
}

/// The waiter-list size per prerequisite is exactly the number of
/// dependents on it (the dedup invariant) — the structure the O(1)
/// `last()` check rests on. With 3 prerequisites and 300 dependents
/// (100 on each), after all dependents have parked and after release
/// all lists are empty (the complete-drain invariant).
#[test]
fn waiter_lists_deduplicate_and_release_completely() {
    let mut spine = Spine::new(CountingWorld { executions: 0 }, 400);
    let prereqs: Vec<_> = (0..3)
        .map(|i| {
            spine
                .schedule(EventSpec::new(
                    EventClass::new(1),
                    Schedule::AtLogicalTime { due: Tick(10) },
                    i as u64,
                ))
                .expect("valid")
        })
        .collect();
    // 100 dependents per prerequisite.
    for i in 0..300u64 {
        let dep = prereqs[(i % 3) as usize];
        spine
            .schedule(
                EventSpec::new(
                    EventClass::new(2),
                    Schedule::AtLogicalTime { due: Tick(1) },
                    i,
                )
                .with_dependencies(vec![dep]),
            )
            .expect("valid");
    }

    // Tick 1: all 300 dependents park.
    spine.run_tick();
    assert_eq!(spine.blocked_count(), 300);
    // Ticks 2-9: waiting (clock 2-9), structure stable.
    for _ in 0..8 {
        spine.run_tick();
    }
    assert_eq!(spine.blocked_count(), 300);
    assert_eq!(
        spine.stats().blocked_transitions,
        300,
        "parked once across 9 ticks"
    );

    // Tick 10: prerequisites fire, all 300 release (budget 400 > 303).
    spine.run_tick();
    assert_eq!(spine.stats().executed, 303);
    assert_eq!(spine.blocked_count(), 0);

    // The world executed everything exactly once.
    let world = spine.world();
    assert_eq!(world.executions, 303);
}

/// The parking work is proportional to the NUMBER OF BLOCKED EVENTS,
/// not to (blocked x ticks): running 10x more waiting ticks with
/// the same blocked population leaves blocked_transitions unchanged
/// (each event parked once, regardless of how long it waits). This
/// is the structural linearity property — the measured-negligible
/// finding's foundation.
#[test]
fn parking_cost_is_per_event_not_per_event_per_tick() {
    for &ticks in &[10u64, 100, 1000] {
        let mut spine = Spine::new(CountingWorld { executions: 0 }, 600);
        let prereq = spine
            .schedule(EventSpec::new(
                EventClass::new(1),
                Schedule::AtLogicalTime {
                    due: Tick(ticks + 1),
                },
                1,
            ))
            .expect("valid");
        for i in 0..500u64 {
            spine
                .schedule(
                    EventSpec::new(
                        EventClass::new(2),
                        Schedule::AtLogicalTime { due: Tick(1) },
                        i,
                    )
                    .with_dependencies(vec![prereq]),
                )
                .expect("valid");
        }
        for _ in 0..ticks {
            spine.run_tick();
        }
        assert_eq!(
            spine.stats().blocked_transitions,
            500,
            "500 events, {ticks} waiting ticks — parked exactly once each"
        );
        assert_eq!(spine.stats().executed, 0);
    }
}
