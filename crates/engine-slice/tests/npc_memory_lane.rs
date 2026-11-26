//! The memory-lane integration battery (Era-2 package A part 7, stage
//! 2 — the capture-lane gates' driven half): the composed world's
//! known answers, the I2 tier gate through the real census, the held
//! lane, the byte-identical rebuild across deactivate/reactivate
//! cycles, the composed pending-until-classified reputation, the REAL
//! pipeline ride (confirmation, the dropped-ack fault, the recovery
//! read), THE ERA-2 AUDIT'S FINDING 1–3 DECIDING GATES (the
//! self-sufficient retry with a transient commit failure, the
//! fresh-world cold reopen comparing individual canonical records,
//! phases and classified score, and the one retention order through
//! the composed world's held-event round trip), and THE S6 FUZZ GATE
//! — the design pass §8.4 item 4's "the S6 fuzz discipline, on the
//! real pipeline" (the S6-lite harness's op distribution and fault
//! classes, over the real spine, the real census, and the real
//! CommitPipeline).

use engine_core::clock::Tick;
use engine_core::fingerprint::Fingerprinter;
use engine_core::lod::LodLevel;
use engine_core::npc_memory::{
    CaptureOutcome, EpisodicMemory, MemoryKey, MemoryKind, MemoryRecord,
};
use engine_core::spine::Spine;
use engine_core::spine::World;
use engine_persist::consequence::ConsequenceId;
use engine_persist::fault::{FaultAction, FaultControl, FaultPoint};
use engine_persist::journal::T1Journal;
use engine_persist::local::LocalFileMedium;
use engine_persist::medium::{AppendOutcome, CanonicalMedium};
use engine_persist::pipeline::CommitPipeline;
use engine_persist::record::RevisionRecord;
use engine_slice::npc_memory_lane::{
    ENCOUNTER_TABLE, EncounterKind, MemoryCommitBook, MemoryLaneWorld, PLACES, POPULATION,
    REGION_NPCS, commit_witness_captures, install_recovered_lane, recover_memory_lane,
    schedule_encounter, set_region_level,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

/// A fresh unique temp dir per test (the b3_matrix TestDir shape).
struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "postRust-memory-lane-{}-{}-{}",
            tag,
            std::process::id(),
            n
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("the temp dir creates");
        TestDir(dir)
    }

    fn store(&self) -> PathBuf {
        self.0.join("store")
    }

    fn journal(&self) -> PathBuf {
        self.0.join("journal.t1")
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Idempotently founds the campaign store (the b3_matrix recipe
/// verbatim — the application's setup act, never a per-commit
/// decision; the founding instance drops before the pipeline opens
/// its own medium).
fn ensure_founded(dir: &TestDir) {
    let mut medium = LocalFileMedium::open(dir.store()).unwrap();
    if medium.head().unwrap().is_some() {
        return;
    }
    let founding = RevisionRecord::founding(
        engine_persist::record::AuthorityMode::Standalone,
        "memory-lane-campaign",
        0,
    );
    match medium.append(&founding, &[], None).unwrap() {
        AppendOutcome::Accepted { .. } => {}
        other => panic!("the founding must accept: {other:?}"),
    }
}

fn fresh_pipeline(dir: &TestDir) -> CommitPipeline<LocalFileMedium> {
    ensure_founded(dir);
    let journal = T1Journal::open(dir.journal()).unwrap();
    CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal).unwrap()
}

fn make_spine() -> Spine<MemoryLaneWorld> {
    Spine::with_reservation(MemoryLaneWorld::new(), 200, 60)
}

/// The whole-campaign conservation identity, asserted on the real
/// spine (the s6_burst discipline).
fn assert_spine_conservation(spine: &Spine<MemoryLaneWorld>, where_: &str) {
    let stats = spine.stats();
    let outstanding = spine.pending_count() + spine.blocked_count() + spine.held_count();
    assert_eq!(
        stats.scheduled,
        stats.executed + outstanding as u64,
        "spine conservation at {where_}"
    );
}

/// The per-witness I1 orphan identity for every population member.
fn assert_lane_conservation(spine: &Spine<MemoryLaneWorld>, where_: &str) {
    for record in POPULATION.iter() {
        let (produced, pending, confirmed) = spine.world().lane.witness_conservation(record.id);
        assert_eq!(
            produced as usize,
            pending + confirmed,
            "lane conservation for witness {} at {where_}",
            record.id
        );
    }
}

/// THE DRIVEN KNOWN ANSWERS: a Greeting at region 0's place reaches
/// exactly witnesses {0,1,2,3} (the authored distances 0.5, ≈1.06,
/// 1.5, ≈1.77 — all within the radius 2.0; ids 4 and 5 at 2.5 and
/// 3.0 are outside), each holding EXACTLY one episodic record (the
/// Greeting table has no relationship, no candidate), the authored
/// actor rule's subject, and the conservation identities on both
/// sides.
#[test]
fn witness_census_and_records_known_answers() {
    let mut spine = make_spine();
    schedule_encounter(&mut spine, 1, EncounterKind::Greeting, 0, 0, Tick(5)).unwrap();
    spine.run_ticks(5);

    let world = spine.world();
    assert_eq!(
        world.executed_encounters, 1,
        "the encounter fired at its due tick"
    );
    assert_eq!(world.unknown_payloads, 0);
    assert_eq!(world.encounter_log.len(), 1);

    let logged = &world.encounter_log[0];
    assert_eq!(logged.event, 1);
    assert_eq!(logged.kind, EncounterKind::Greeting);
    assert_eq!(
        logged.witnesses,
        vec![0, 1, 2, 3],
        "the hand-computed census"
    );

    // The authored actor rule: event 1 at region 0 → NPC 1.
    let actor = POPULATION[1].id;
    for witness in [0u32, 1, 2, 3] {
        let live = world.lane.live(witness).expect("the witness is active");
        assert_eq!(live.episodic().len(), 1, "one episodic record per witness");
        assert_eq!(
            live.episodic()[0].trace.perceived,
            actor,
            "the authored actor"
        );
        assert_eq!(live.episodic()[0].trace.observer, witness);
        assert_eq!(
            live.episodic()[0].trace.tick,
            Tick(5),
            "the logical execution tick"
        );
        assert_eq!(
            live.episodic()[0].trace.class,
            u32::from(EncounterKind::Greeting.tag())
        );
        assert!(
            live.relationships().is_empty(),
            "Greeting authors no relationship"
        );
        assert!(
            live.candidates().is_empty(),
            "Greeting authors no candidate"
        );
        let (produced, pending, confirmed) = world.lane.witness_conservation(witness);
        assert_eq!((produced, pending, confirmed), (1, 1, 0));
    }
    // The excluded pair holds nothing.
    for witness in [4u32, 5] {
        let (produced, _, _) = world.lane.witness_conservation(witness);
        assert_eq!(
            produced, 0,
            "witness {witness} is outside the Greeting radius"
        );
    }

    assert_spine_conservation(&spine, "the known-answer campaign");
    assert_lane_conservation(&spine, "the known-answer campaign");
}

/// THE I2 GATE, DRIVEN: region 0's NPC 5 sits exactly 3.0 from region
/// 1's place — geometrically inside the Theft radius 4.0. With region
/// 0 DORMANT the census excludes it (the real tier gate: a dormant
/// region's NPCs witness nothing); with region 0 FULL a new encounter
/// includes it. The tier gate is the REAL enumerate_witnesses
/// discipline, exercised through the world's own execution.
#[test]
fn i2_dormant_witnesses_nothing() {
    let mut spine = make_spine();
    set_region_level(&mut spine, 0, LodLevel::Dormant);

    // A Theft at region 1's place while region 0 is dormant.
    schedule_encounter(&mut spine, 1, EncounterKind::Theft, 1, 1, Tick(5)).unwrap();
    spine.run_ticks(5);

    let world = spine.world();
    let logged = &world.encounter_log[0];
    assert_eq!(
        logged.witnesses,
        vec![6, 7, 8, 9, 10, 11],
        "region 1's own six witness; region 0's NPC 5 (geometrically inside) does NOT"
    );
    let (produced, _, _) = world.lane.witness_conservation(5);
    assert_eq!(produced, 0, "I2: the dormant witness records nothing");

    // Activate region 0; a NEW encounter at the same place includes
    // NPC 5 (the tier gate's positive half).
    set_region_level(&mut spine, 0, LodLevel::Full);
    schedule_encounter(&mut spine, 2, EncounterKind::Theft, 1, 1, Tick(10)).unwrap();
    spine.run_ticks(5);

    let world = spine.world();
    let logged = &world.encounter_log[1];
    assert_eq!(
        logged.witnesses,
        vec![5, 6, 7, 8, 9, 10, 11],
        "the active witness is back in the census"
    );
    let (produced, _, _) = world.lane.witness_conservation(5);
    assert_eq!(
        produced, 3,
        "episodic + relationship + candidate for the Theft"
    );
    assert_lane_conservation(&spine, "the I2 campaign");
}

/// THE HELD LANE: an encounter authored into a DORMANT region enters
/// the held set (schedule_held — new work targeting dormant regions
/// never fires unnoticed); it fires on activation (the identity-
/// preserving resume), and the records land exactly once.
#[test]
fn held_encounters_fire_on_activation() {
    let mut spine = make_spine();
    set_region_level(&mut spine, 2, LodLevel::Dormant);

    schedule_encounter(&mut spine, 1, EncounterKind::Altercation, 2, 2, Tick(10)).unwrap();
    assert_eq!(spine.held_count(), 1, "the encounter is held, not queued");

    // Fifteen ticks pass: nothing fires (I5's held-lane discipline).
    spine.run_ticks(15);
    assert_eq!(spine.world().executed_encounters, 0);
    assert_eq!(spine.held_count(), 1);

    // Activation: the held event resumes and fires (late — its due
    // was 10, the clock is 15; the lateness is the spine's own
    // honest record, never an erasure).
    set_region_level(&mut spine, 2, LodLevel::Full);
    spine.run_ticks(1);
    let world = spine.world();
    assert_eq!(world.executed_encounters, 1, "the resumed event fired");
    assert_eq!(
        world.encounter_log[0].witnesses.len(),
        6,
        "region 2's own six"
    );

    // Altercation at region 2's place: every same-region NPC within
    // 3.0 inclusive (id 17 exactly ON the boundary at distance 3.0 —
    // the inclusive pin through the real path).
    assert_eq!(
        world.encounter_log[0].witnesses,
        vec![12, 13, 14, 15, 16, 17]
    );
    for witness in [12u32, 13, 14, 15, 16, 17] {
        let (produced, _, _) = world.lane.witness_conservation(witness);
        assert_eq!(
            produced, 3,
            "episodic + relationship + candidate per witness"
        );
    }
    assert_spine_conservation(&spine, "the held-lane campaign");
}

/// THE C1 REBUILD: a mixed encounter history, then deactivate →
/// duplicate-deliver (the C2 fault — the lane digest must not move)
/// → activate → the rebuilt live digests are byte-identical to the
/// pre-deactivation digests, the classification included. Twice.
#[test]
fn deactivate_reactivate_rebuild_byte_identical() {
    let mut spine = make_spine();

    // A Theft at region 1 (witnesses: region 1's six + region 0's
    // NPC 5) and a Greeting at region 0.
    schedule_encounter(&mut spine, 1, EncounterKind::Theft, 1, 1, Tick(5)).unwrap();
    schedule_encounter(&mut spine, 2, EncounterKind::Greeting, 0, 0, Tick(6)).unwrap();
    spine.run_ticks(6);

    // Classify one candidate (witness 6's Theft candidate) — the
    // rebuild must carry the decision through.
    let cand_key = MemoryKey {
        event: 1,
        witness: 6,
        kind: MemoryKind::ReputationCandidate,
    };
    spine.world_mut().lane.classify(&cand_key, true);

    // The pre-deactivation snapshots: every REGION 1 witness's live
    // digest (region 0's representations are not touched by region
    // 1's deactivation — the lane's per-witness discard).
    let before: BTreeMap<u32, u64> = (6..12u32)
        .map(|w| (w, spine.world().lane.live(w).unwrap().digest()))
        .collect();
    let lane_before = spine.world().lane.digest();

    for cycle in 0..2 {
        set_region_level(&mut spine, 1, LodLevel::Dormant);
        for w in 6..12u32 {
            assert!(
                spine.world().lane.live(w).is_none(),
                "cycle {cycle}: discarded"
            );
        }

        // THE C2 FAULT mid-cycle: re-deliver event 1's records for
        // witness 6 (the same construction the world used). Every
        // capture must no-op and the lane digest must not move.
        let digest_now = spine.world().lane.digest();
        let tick = spine.world().encounter_log[0].tick;
        let actor = POPULATION[1].id;
        for kind in [
            MemoryKind::Episodic,
            MemoryKind::Relationship,
            MemoryKind::ReputationCandidate,
        ] {
            let delta = match kind {
                MemoryKind::Episodic => {
                    engine_core::npc_memory::MemoryDelta::episodic(6, actor, tick, 2)
                }
                MemoryKind::Relationship => engine_core::npc_memory::MemoryDelta::Relationship {
                    actor,
                    delta: ENCOUNTER_TABLE[1].relationship.unwrap(),
                },
                MemoryKind::ReputationCandidate => {
                    let (class, weight) = ENCOUNTER_TABLE[1].candidate.unwrap();
                    engine_core::npc_memory::MemoryDelta::ReputationCandidate {
                        actor,
                        class,
                        weight,
                    }
                }
            };
            let record = MemoryRecord {
                key: MemoryKey {
                    event: 1,
                    witness: 6,
                    kind,
                },
                delta,
                tick,
            };
            let outcome = spine
                .world_mut()
                .lane
                .capture_and_apply(record, 32)
                .unwrap();
            assert!(
                outcome != CaptureOutcome::Captured,
                "cycle {cycle}: the duplicate re-delivery is a no-op"
            );
        }
        assert_eq!(
            spine.world().lane.digest(),
            digest_now,
            "cycle {cycle}: the lane digest survived the duplicate delivery"
        );

        // Activation: the rebuilt digests equal the pre-deactivation
        // snapshots (the classification re-applied — the candidate
        // stays Accepted, never Pending).
        set_region_level(&mut spine, 1, LodLevel::Full);
        for (w, digest) in &before {
            assert_eq!(
                spine.world().lane.live(*w).unwrap().digest(),
                *digest,
                "cycle {cycle}: witness {w}'s rebuild is byte-identical"
            );
        }
        let candidate = spine
            .world()
            .lane
            .live(6)
            .unwrap()
            .candidates()
            .get(&cand_key);
        assert_eq!(
            candidate.unwrap().phase,
            engine_core::npc_memory::CandidatePhase::Accepted,
            "cycle {cycle}: the classification survived the cycle"
        );
    }
    // The lane digest is unchanged by the whole double cycle (the
    // representation rebuild is a pure function of the ledger).
    assert_eq!(spine.world().lane.digest(), lane_before);
    assert_lane_conservation(&spine, "the rebuild campaign");
}

/// THE COMPOSED PENDING-UNTIL-CLASSIFIED GATE (§8.4 item 4's second
/// clause): Theft candidates are captured but the shared reputation
/// stays 0 until the classification; the authored review rule
/// (accept iff weight ≤ −4 — the demo's POLICY, [OPEN] §9.2) accepts
/// exactly the Theft weights; a rejection never feeds the score; a
/// deactivation cycle leaves the score byte-identical.
#[test]
fn reputation_pending_until_classified_composed() {
    let mut spine = make_spine();

    // A Theft at region 3 (witnesses: region 3's six + region 2's
    // NPC 17) and an Altercation at region 0 (six witnesses).
    schedule_encounter(&mut spine, 1, EncounterKind::Theft, 3, 3, Tick(5)).unwrap();
    schedule_encounter(&mut spine, 2, EncounterKind::Altercation, 0, 0, Tick(6)).unwrap();
    spine.run_ticks(6);

    // Every candidate is pending: the shared score is 0.
    assert_eq!(spine.world().lane.reputation_score().unwrap(), 0);

    // The review rule applied to witness 18's and 19's Theft
    // candidates (weight −5 ≤ −4 → accepted).
    for witness in [18u32, 19] {
        let key = MemoryKey {
            event: 1,
            witness,
            kind: MemoryKind::ReputationCandidate,
        };
        assert_eq!(
            spine
                .world_mut()
                .lane
                .classify(&key, ENCOUNTER_TABLE[1].candidate.unwrap().1 <= -4),
            engine_core::npc_memory::ClassifyOutcome::Classified
        );
    }
    assert_eq!(spine.world().lane.reputation_score().unwrap(), -10);

    // Re-classification is refused; the score does not move.
    let key = MemoryKey {
        event: 1,
        witness: 18,
        kind: MemoryKind::ReputationCandidate,
    };
    assert_eq!(
        spine.world_mut().lane.classify(&key, false),
        engine_core::npc_memory::ClassifyOutcome::AlreadyClassified
    );
    assert_eq!(spine.world().lane.reputation_score().unwrap(), -10);

    // An Altercation candidate (weight −3) classified REJECTED by the
    // rule: never a score contribution.
    let key = MemoryKey {
        event: 2,
        witness: 0,
        kind: MemoryKind::ReputationCandidate,
    };
    assert_eq!(
        spine
            .world_mut()
            .lane
            .classify(&key, ENCOUNTER_TABLE[0].candidate.unwrap().1 <= -4),
        engine_core::npc_memory::ClassifyOutcome::Classified
    );
    assert_eq!(spine.world().lane.reputation_score().unwrap(), -10);

    // The never-classified candidates (witnesses 20-23, 17) stay
    // pending: no contribution, ever.
    assert_eq!(spine.world().lane.reputation_score().unwrap(), -10);

    // The M-08 durability: a full deactivation cycle of every
    // involved region leaves the score byte-identical (the fold
    // reads the ledger + the decisions, never the representations).
    for region in [0usize, 2, 3] {
        set_region_level(&mut spine, region, LodLevel::Dormant);
    }
    assert_eq!(spine.world().lane.reputation_score().unwrap(), -10);
    for region in [0usize, 2, 3] {
        set_region_level(&mut spine, region, LodLevel::Full);
    }
    assert_eq!(spine.world().lane.reputation_score().unwrap(), -10);
}

/// THE REAL PIPELINE RIDE: the CommitPipeline over a founded
/// LocalFileMedium and T1 journal — the staged-commit order (stage →
/// the fsynced T1 capture → mark_applied → commit_canonical), the
/// canonical confirmation mapping back to the lane's per-record
/// confirms, the DROPPED-ACK fault (claimed-but-pending, pending-live
/// — the rebuild unaffected), the C2 re-delivery (a later commit's
/// actionable set confirms the dropped episode), and the RECOVERY
/// READ (a fresh pipeline over the same store+journal restores the
/// confirmed view — the counted consequence is canonical across
/// sessions, the signal-lane precedent).
#[test]
fn the_pipeline_ride_confirms_and_recovers() {
    let dir = TestDir::new("pipeline-ride");
    let mut spine = make_spine();
    let mut pipeline = fresh_pipeline(&dir);
    let mut book = MemoryCommitBook::default();

    // An Altercation at region 0: witnesses 0-5, three records each.
    schedule_encounter(&mut spine, 1, EncounterKind::Altercation, 0, 0, Tick(5)).unwrap();
    spine.run_ticks(5);

    // The full commit for witness 0: the keys confirm in the lane,
    // the consequence confirms in the pipeline.
    let keys = commit_witness_captures(&mut spine, &mut pipeline, &mut book, 0, true).unwrap();
    assert_eq!(
        keys.len(),
        3,
        "the episodic + relationship + candidate keys"
    );
    assert_eq!(
        spine.world().lane.witness_conservation(0),
        (3, 0, 3),
        "the lane's records confirmed"
    );
    let id0 = ConsequenceId::new(0, 0, 0);
    assert!(
        pipeline.is_confirmed(id0),
        "the pipeline's canonical confirmation"
    );
    assert_eq!(
        pipeline
            .confirmed_coverage(id0)
            .map(|spans| spans.iter().map(|s| s.occurrences()).sum::<u64>())
            .unwrap_or(0),
        3,
        "the coverage count equals the claimed records"
    );

    // THE DROPPED-ACK FAULT for witness 1: journaled and claimed but
    // never canonically committed — pending-live.
    let keys = commit_witness_captures(&mut spine, &mut pipeline, &mut book, 1, false).unwrap();
    assert_eq!(keys.len(), 3);
    assert_eq!(
        spine.world().lane.witness_conservation(1),
        (3, 3, 0),
        "the dropped-ack keys stay pending in the lane"
    );
    let id1 = ConsequenceId::new(0, 1, 1);
    assert!(
        !pipeline.is_confirmed(id1),
        "the pipeline holds it unconfirmed"
    );
    // Pending-live: the witness's live view is unaffected (the
    // confirmation is not the application).
    assert_eq!(spine.world().lane.live(1).unwrap().episodic().len(), 1);

    // THE C2 RE-DELIVERY: a later full commit (witness 2) confirms
    // the WHOLE actionable set — witness 1's dropped episode included.
    commit_witness_captures(&mut spine, &mut pipeline, &mut book, 2, true).unwrap();
    assert_eq!(
        spine.world().lane.witness_conservation(1),
        (3, 0, 3),
        "the dropped-ack episode confirmed by the later commit's actionable set"
    );
    assert!(pipeline.is_confirmed(id1));

    // An empty commit is a clean no-op.
    let keys = commit_witness_captures(&mut spine, &mut pipeline, &mut book, 0, true).unwrap();
    assert!(keys.is_empty(), "every key already claimed");

    // THE RECOVERY READ: a fresh pipeline over the same store and
    // journal restores the confirmed view (the session-open chain
    // walk — the counted consequence is canonical across sessions).
    drop(pipeline);
    let fresh = fresh_pipeline(&dir);
    assert!(fresh.is_confirmed(id0));
    assert!(fresh.is_confirmed(id1));
    let confirmed = fresh.confirmed_keys();
    assert!(confirmed.contains(&id0) && confirmed.contains(&id1));
}

/// THE FINDING 1 DECIDING GATE (the audit's own script): the last
/// unacknowledged capture retries ON ITS OWN — the same witness after
/// a withheld submission, with ZERO new events, then a transient
/// commit failure retried again in a FRESH session over the same
/// durable state. Exactly one durable consequence per episode (never
/// one per retry), all original keys confirmed, unchanged reputation.
#[test]
fn the_last_unacknowledged_capture_retries_on_its_own() {
    let dir = TestDir::new("self-retry");
    let mut spine = make_spine();
    let mut pipeline = fresh_pipeline(&dir);
    let mut book = MemoryCommitBook::default();

    // An Altercation at region 0: witness 0 holds three records.
    schedule_encounter(&mut spine, 1, EncounterKind::Altercation, 0, 0, Tick(5)).unwrap();
    spine.run_ticks(5);

    // THE WITHHELD SUBMISSION (the dropped-ack fault): journaled,
    // claimed, never canonically committed.
    let keys = commit_witness_captures(&mut spine, &mut pipeline, &mut book, 0, false).unwrap();
    assert_eq!(
        keys.len(),
        3,
        "the episodic + relationship + candidate keys"
    );
    let id0 = ConsequenceId::new(0, 0, 0);
    assert!(
        !pipeline.is_confirmed(id0),
        "the dropped ack is unconfirmed"
    );
    assert_eq!(
        spine.world().lane.witness_conservation(0),
        (3, 3, 0),
        "claimed-but-pending in the lane"
    );
    let reputation_before = spine.world().lane.reputation_score().unwrap();

    // THE SELF-SUFFICIENT RETRY: the SAME witness, submit = true, ZERO
    // new events. Under the defective bridge this call returned early
    // (every key already claimed) and the final episode stayed pending
    // until some OTHER witness committed fresh keys; the flush half
    // now drives the canonical commit on its own.
    let keys = commit_witness_captures(&mut spine, &mut pipeline, &mut book, 0, true).unwrap();
    assert!(
        keys.is_empty(),
        "the retry stages no new episode — it flushes the existing one"
    );
    assert_eq!(
        spine.world().lane.witness_conservation(0),
        (3, 0, 3),
        "all ORIGINAL keys confirmed by the retry"
    );
    assert!(pipeline.is_confirmed(id0));
    assert_eq!(
        spine.world().lane.reputation_score().unwrap(),
        reputation_before,
        "the retry changes no classification"
    );
    // EXACTLY ONE durable consequence — the retry created no episode.
    assert_eq!(pipeline.confirmed_keys(), vec![id0]);

    // THE TRANSIENT COMMIT FAILURE: a second encounter (witness 1)
    // committed through a medium whose append fails BEFORE the lock —
    // a clean, retry-safe injection (nothing durable was written by
    // the failed attempt; the journal transaction and the claims
    // stand).
    schedule_encounter(&mut spine, 2, EncounterKind::Altercation, 0, 0, Tick(10)).unwrap();
    spine.run_ticks(5);
    drop(pipeline);
    let mut faulted = CommitPipeline::open(
        LocalFileMedium::open_with(
            dir.store(),
            Some(FaultControl::new().arm(FaultPoint::BeforeLock, FaultAction::InjectError)),
        )
        .unwrap(),
        T1Journal::open(dir.journal()).unwrap(),
    )
    .unwrap();
    let result = commit_witness_captures(&mut spine, &mut faulted, &mut book, 1, true);
    assert!(
        result.is_err(),
        "the transient canonical failure propagates"
    );
    assert_eq!(
        spine.world().lane.witness_conservation(1),
        (6, 6, 0),
        "claimed-but-pending survives the transient failure (both encounters' records)"
    );
    drop(faulted);

    // The retry AFTER the transient failure, in a FRESH session over
    // the same store and journal (the fault is gone): still zero new
    // events — the flush finishes the episode.
    let mut pipeline = fresh_pipeline(&dir);
    let keys = commit_witness_captures(&mut spine, &mut pipeline, &mut book, 1, true).unwrap();
    assert!(keys.is_empty(), "still no new episode");
    assert_eq!(
        spine.world().lane.witness_conservation(1),
        (6, 0, 6),
        "the failed canonical commit is finished by the plain retry"
    );
    let mut confirmed = pipeline.confirmed_keys();
    confirmed.sort_unstable();
    assert_eq!(
        confirmed,
        vec![ConsequenceId::new(0, 0, 0), ConsequenceId::new(0, 1, 1)],
        "exactly two durable consequences — one per EPISODE, none per retry"
    );
    assert_lane_conservation(&spine, "the self-retry campaign");
}

/// THE FINDING 20 DECIDING GATE (the sign-off review's own script):
/// a definite journal rejection leaves the episode's envelope
/// discarded and its identity recorded in the book; the retry
/// completes the SAME episode — never a second envelope for the same
/// records (the defective bridge consumed a fresh episode number and
/// journaled the leftover envelope under the new plan: two
/// consequences for one memory set, and a cold reopen that refused
/// the journal at the identity cross-check). One envelope, one plan,
/// one consequence; all original records confirmed; the cold reopen
/// succeeds; the recovered lane materializes its requested tiers.
#[test]
fn a_rejected_journal_append_retries_the_same_episode() {
    let dir = TestDir::new("f20-rejected-append");
    let mut spine = make_spine();
    let pipeline = fresh_pipeline(&dir);
    let mut book = MemoryCommitBook::default();

    // An Altercation at region 0: witness 0 holds three records.
    schedule_encounter(&mut spine, 1, EncounterKind::Altercation, 0, 0, Tick(5)).unwrap();
    spine.run_ticks(5);

    // THE REJECTED JOURNAL APPEND: the journal's pre-window fault —
    // the capture is definitely not committed (nothing written).
    drop(pipeline);
    let journal = engine_persist::journal::T1Journal::open_with(
        dir.journal(),
        Some(engine_persist::journal::JournalFaultControl::new().arm(
            engine_persist::journal::JournalFaultPoint::BeforeCaptures,
            FaultAction::InjectError,
        )),
    )
    .unwrap();
    let mut faulted =
        CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal).unwrap();
    let result = commit_witness_captures(&mut spine, &mut faulted, &mut book, 0, true);
    assert!(result.is_err(), "the rejected journal append propagates");

    // THE STABLE IDENTITY: the book holds the unresolved episode —
    // nothing claimed (nothing journaled), the membership recorded.
    let unresolved = book
        .unresolved
        .as_ref()
        .expect("the rejected append records the unresolved episode");
    assert_eq!(unresolved.episode, 0, "the first episode identity");
    assert_eq!(unresolved.keys.len(), 3, "the staged membership");
    assert!(!unresolved.journaled, "nothing is durable for it");
    assert!(book.claimed.is_empty(), "no key is claimed");
    assert_eq!(
        book.episodes, 1,
        "the episode counter advanced exactly once"
    );
    assert_eq!(
        spine.world().lane.witness_conservation(0),
        (3, 3, 0),
        "the records stay pending-unclaimed in the lane"
    );
    drop(faulted);

    // THE RETRY (the fault gone, a fresh pipeline over the same
    // store): completes the SAME episode identity — the defective
    // bridge staged a SECOND envelope here, journaling the leftover
    // under a new plan.
    let mut pipeline = fresh_pipeline(&dir);
    let keys = commit_witness_captures(&mut spine, &mut pipeline, &mut book, 0, true).unwrap();
    assert!(
        keys.is_empty(),
        "no NEW episode — the completion is the unresolved one"
    );
    assert!(book.unresolved.is_none(), "the completion cleared it");
    assert_eq!(
        spine.world().lane.witness_conservation(0),
        (3, 0, 3),
        "all ORIGINAL keys confirmed by the completion's flush"
    );
    assert!(
        pipeline.is_confirmed(ConsequenceId::new(0, 0, 0)),
        "exactly the first episode identity confirmed"
    );
    assert_eq!(
        pipeline.confirmed_keys(),
        vec![ConsequenceId::new(0, 0, 0)],
        "ONE durable consequence — the retry created no episode"
    );

    // THE COLD REOPEN: the journal is internally consistent (one
    // envelope under its own plan — the identity cross-check the
    // defective bridge's double-staging failed).
    drop(pipeline);
    let reopened = fresh_pipeline(&dir);
    let recovered = recover_memory_lane(&reopened).unwrap();
    assert_eq!(
        recovered.book.episodes, 1,
        "the recovered book's episode counter"
    );
    let mut fresh_spine = make_spine();
    let recovered_book = install_recovered_lane(&mut fresh_spine, recovered);
    // THE TIER MATERIALIZATION (finding 21's install half): the
    // recovered lane was all-dormant; the fresh world requests Full
    // everywhere; the install ACTIVATES the region's witnesses —
    // the live view exists NOW (the defective install left the
    // witnesses dormant behind a Full region until an unrelated
    // tier transition).
    assert_eq!(
        fresh_spine.world().lane.witness_conservation(0),
        (3, 0, 3),
        "the recovered witness is materialized ACTIVE with its confirmed records"
    );
    drop(reopened);
    drop(recovered_book);
}

/// THE FINDING 20 AMBIGUOUS-ACK GATE: a canonical submission that
/// commits durably but never acknowledges (the medium's
/// post-linearization fault) leaves the lane pending; a later no-op
/// retry on a FRESH session reconciles from the canonical chain —
/// the defective bridge's NothingPending path skipped the
/// confirmation and the episode stayed pending forever.
#[test]
fn an_ambiguous_canonical_ack_reconciles_from_the_chain() {
    let dir = TestDir::new("f20-ambiguous-ack");
    let mut spine = make_spine();
    let pipeline = fresh_pipeline(&dir);
    let mut book = MemoryCommitBook::default();

    schedule_encounter(&mut spine, 1, EncounterKind::Altercation, 0, 0, Tick(5)).unwrap();
    spine.run_ticks(5);

    // THE AMBIGUOUS CANONICAL APPEND: the journal capture succeeds;
    // the canonical submission's write is durable but the
    // acknowledgment is lost (the medium's post-root-sync fault).
    drop(pipeline);
    let mut faulted = CommitPipeline::open(
        LocalFileMedium::open_with(
            dir.store(),
            Some(FaultControl::new().arm(FaultPoint::AfterRootDirSync, FaultAction::InjectError)),
        )
        .unwrap(),
        T1Journal::open(dir.journal()).unwrap(),
    )
    .unwrap();
    let keys = commit_witness_captures(&mut spine, &mut faulted, &mut book, 0, true).unwrap();
    assert_eq!(keys.len(), 3, "the episode staged and journaled");
    // The ambiguous outcome is NOT an error — the episode is
    // journaled and claimed; the lane still shows it pending (this
    // session's view never learned the chain's answer).
    assert_eq!(
        spine.world().lane.witness_conservation(0),
        (3, 3, 0),
        "claimed-but-pending after the lost acknowledgment"
    );
    drop(faulted);

    // THE NO-OP RETRY on a FRESH session (the fault gone): the
    // flush's confirmed-identity sweep reads the chain — the
    // ambiguous submission IS the chain's head — and confirms the
    // episode WITHOUT staging anything (the defective bridge
    // returned NothingPending and left the lane pending forever).
    let mut pipeline = fresh_pipeline(&dir);
    let keys = commit_witness_captures(&mut spine, &mut pipeline, &mut book, 0, true).unwrap();
    assert!(keys.is_empty(), "zero new events");
    assert_eq!(
        spine.world().lane.witness_conservation(0),
        (3, 0, 3),
        "the chain-held episode reconciled and confirmed"
    );
    assert_eq!(
        pipeline.confirmed_keys(),
        vec![ConsequenceId::new(0, 0, 0)],
        "exactly one durable consequence"
    );
    assert_lane_conservation(&spine, "the ambiguous-ack campaign");
}

/// THE FINDING 21 DECIDING GATE (the sign-off review's own script):
/// a classification decision taken AFTER the witness's final record
/// episode is flushed through its OWN journal transaction when
/// submit observes the mismatch — the decision and the classified
/// score survive the cold reopen (the defective bridge skipped plan
/// creation with no fresh records, and the decision vanished), and
/// a NEW encounter for the same witness on the recovered session
/// behaves as an uninterrupted one's.
#[test]
fn a_late_classification_survives_the_reopen() {
    let dir = TestDir::new("f21-late-classification");
    let mut spine = make_spine();
    let mut pipeline = fresh_pipeline(&dir);
    let mut book = MemoryCommitBook::default();

    // Witness 0's Altercation: three records (episodic +
    // relationship + candidate), committed and confirmed (the
    // candidate UNCLASSIFIED at episode time — the shared score 0).
    schedule_encounter(&mut spine, 1, EncounterKind::Altercation, 0, 0, Tick(5)).unwrap();
    spine.run_ticks(5);
    commit_witness_captures(&mut spine, &mut pipeline, &mut book, 0, true).unwrap();
    assert_eq!(
        spine.world().lane.witness_conservation(0),
        (3, 0, 3),
        "the record episode confirmed"
    );
    assert_eq!(spine.world().lane.reputation_score().unwrap(), 0);

    // THE LATE CLASSIFICATION: the candidate is accepted AFTER the
    // final record episode — a live decision the journal does not
    // hold (the plan's sync section was written before it).
    let candidate_key = MemoryKey {
        event: 1,
        witness: 0,
        kind: MemoryKind::ReputationCandidate,
    };
    assert_eq!(
        spine.world_mut().lane.classify(&candidate_key, true),
        engine_core::npc_memory::ClassifyOutcome::Classified,
        "the live decision applies"
    );
    let live_score = spine.world().lane.reputation_score().unwrap();
    assert_eq!(
        live_score, -3,
        "the accepted Altercation candidate weighs -3"
    );

    // THE CLASSIFICATION FLUSH: submit with ZERO new events observes
    // the sync mismatch and journals the decision as its own
    // transaction (the defective bridge skipped plan creation here —
    // the decision was durable only in the live lane).
    let keys = commit_witness_captures(&mut spine, &mut pipeline, &mut book, 0, true).unwrap();
    assert!(keys.is_empty(), "no new records — the flush only");
    let mut confirmed = pipeline.confirmed_keys();
    confirmed.sort_unstable();
    assert_eq!(
        confirmed,
        vec![ConsequenceId::new(0, 0, 0), ConsequenceId::new(0, 1, 0)],
        "TWO durable consequences: the record episode AND the classification-sync episode"
    );

    // THE COLD REOPEN: the decision and the score SURVIVE (the
    // defective bridge's reopen rebuilt from the last record plan —
    // the candidate returned unclassified, the score 0).
    drop(pipeline);
    let mut reopened = fresh_pipeline(&dir);
    let recovered = recover_memory_lane(&reopened).unwrap();
    let mut fresh_spine = make_spine();
    let mut recovered_book = install_recovered_lane(&mut fresh_spine, recovered);
    assert_eq!(
        fresh_spine.world().lane.reputation_score().unwrap(),
        live_score,
        "the late classification survives the reopen"
    );
    assert_eq!(
        fresh_spine.world().lane.witness_conservation(0),
        (3, 0, 3),
        "the recovered witness is materialized ACTIVE with its confirmed records"
    );

    // THE CONTINUATION: a NEW encounter for the same ACTIVE witness
    // on the recovered session — the captures land in the live view
    // immediately and confirm through the recovered book, exactly as
    // an uninterrupted session's would.
    schedule_encounter(
        &mut fresh_spine,
        2,
        EncounterKind::Altercation,
        0,
        0,
        Tick(10),
    )
    .unwrap();
    fresh_spine.run_ticks(10);
    let new_keys = commit_witness_captures(
        &mut fresh_spine,
        &mut reopened,
        &mut recovered_book,
        0,
        true,
    )
    .unwrap();
    assert_eq!(new_keys.len(), 3, "the new encounter's records staged");
    assert_eq!(
        fresh_spine.world().lane.witness_conservation(0),
        (6, 0, 6),
        "the recovered session continues as an uninterrupted one"
    );
    assert_eq!(
        fresh_spine.world().lane.reputation_score().unwrap(),
        live_score,
        "the continuation's unclassified candidate contributes nothing new"
    );
    drop(reopened);
    drop(recovered_book);
}

/// THE FINDING 20 JOURNALED ARM: an episode whose journal capture
/// SUCCEEDED but whose apply failed (or whose session died between
/// the two) is completed by the apply only — the retry never
/// re-stages or re-journals it. The state is constructed exactly as
/// the bridge leaves it (the envelope staged, the capture done, the
/// book's unresolved at the journaled phase), then the next call
/// finishes it.
#[test]
fn a_journaled_unresolved_episode_completes_with_the_apply_only() {
    let dir = TestDir::new("f20-journaled-unresolved");
    let mut spine = make_spine();
    let mut pipeline = fresh_pipeline(&dir);
    let mut book = MemoryCommitBook::default();

    schedule_encounter(&mut spine, 1, EncounterKind::Altercation, 0, 0, Tick(5)).unwrap();
    spine.run_ticks(5);

    // THE INTERRUPTED APPLY: the bridge stages and journals the
    // episode exactly as the staging half does, then the apply
    // fails — the book records the unresolved episode at the
    // journaled phase (the transaction IS durable; the keys are
    // claimed-journaled).
    let keys: Vec<MemoryKey> = spine
        .world()
        .lane
        .ledger()
        .pending_keys(0)
        .into_iter()
        .collect();
    assert_eq!(keys.len(), 3);
    let episode = book.episodes;
    book.episodes += 1;
    let consequence = ConsequenceId::new(0, episode, 0);
    let position = engine_persist::envelope::SourcePosition {
        head: None,
        tick: spine.world().last_tick.as_u64(),
    };
    let span = engine_persist::consequence::OccurrenceSpan::new(0, keys.len() as u64).unwrap();
    let envelope = engine_persist::envelope::ConsequenceEnvelope::capture(
        consequence,
        vec![engine_persist::envelope::ClassifiedSpan::new(
            span,
            engine_persist::envelope::DurabilityClass::DurableEligible,
        )],
        engine_persist::envelope::SignalLane::Captured,
        engine_persist::envelope::CounterValues::default(),
        engine_persist::envelope::SourceBaseline {
            counters: engine_persist::envelope::CounterValues::default(),
            award: engine_persist::consequence::AwardCheckpoint {
                durable: engine_persist::consequence::Coverage::new(),
                provisional: engine_persist::consequence::Coverage::new(),
            },
        },
        position,
    )
    .unwrap();
    pipeline.stage(envelope).unwrap();
    let records: Vec<MemoryRecord> = {
        let lane = &spine.world().lane;
        keys.iter()
            .map(|key| *lane.ledger().record(key).unwrap())
            .collect()
    };
    let plan = engine_slice::npc_memory_lane::MemoryPlan {
        witness: 0,
        episode,
        records,
        classifications: spine.world().lane.witness_classifications(0),
    }
    .encode()
    .unwrap();
    pipeline
        .capture(
            position,
            engine_persist::envelope::DurabilityClass::DurableEligible,
            Vec::new(),
            vec![engine_persist::journal::FrontierEntry { region: 0, episode }],
            plan,
        )
        .unwrap();
    // The apply NEVER runs — the book records the journaled
    // unresolved state exactly as the bridge's failure path does.
    for key in &keys {
        book.claimed.insert(*key);
    }
    book.unresolved = Some(engine_slice::npc_memory_lane::UnresolvedEpisode {
        witness: 0,
        episode,
        consequence,
        keys: keys.clone(),
        journaled: true,
    });

    // THE COMPLETION: the next call finishes the apply only — no new
    // envelope, no new journal transaction, the same consequence.
    let staged = commit_witness_captures(&mut spine, &mut pipeline, &mut book, 0, true).unwrap();
    assert!(staged.is_empty(), "no new episode");
    assert!(book.unresolved.is_none(), "the completion cleared it");
    assert_eq!(
        spine.world().lane.witness_conservation(0),
        (3, 0, 3),
        "the journaled episode applied and confirmed"
    );
    assert_eq!(
        pipeline.confirmed_keys(),
        vec![consequence],
        "exactly ONE durable consequence — the completion re-journaled nothing"
    );

    // THE COLD REOPEN stays consistent.
    drop(pipeline);
    let reopened = fresh_pipeline(&dir);
    let recovered = recover_memory_lane(&reopened).unwrap();
    let mut fresh_spine = make_spine();
    let recovered_book = install_recovered_lane(&mut fresh_spine, recovered);
    assert_eq!(
        fresh_spine.world().lane.witness_conservation(0),
        (3, 0, 3),
        "the reopen reconstructs the journaled-and-confirmed episode"
    );
    drop(reopened);
    drop(recovered_book);
}

/// THE FINDING 2 DECIDING GATE (the audit's own script): a FRESH
/// world, lane, book and pipeline reopened over the same store and
/// journal — the recovery reconstructs INDIVIDUAL canonical records
/// (not counts), per-key phases, the applicable classification
/// history and the classified score; it terminates at the journaled
/// boundary (an uncommitted witness is honestly absent) and the
/// dropped-ack boundary (journaled-but-unconfirmed recovers pending);
/// and the recovered book is USABLE (the recovered lane finishes the
/// dropped-ack episode with zero new events — findings 1 and 2
/// composed).
#[test]
fn cold_reopen_reconstructs_individual_records() {
    let dir = TestDir::new("cold-reopen");
    let mut spine = make_spine();
    let mut pipeline = fresh_pipeline(&dir);
    let mut book = MemoryCommitBook::default();

    // A mixed campaign: a Theft at region 3 (witnesses 17-23) and an
    // Altercation at region 0 (witnesses 0-5).
    schedule_encounter(&mut spine, 1, EncounterKind::Theft, 3, 3, Tick(5)).unwrap();
    schedule_encounter(&mut spine, 2, EncounterKind::Altercation, 0, 0, Tick(6)).unwrap();
    spine.run_ticks(6);

    // Classify BEFORE the commits (the decisions ride the episodes'
    // classification-sync sections): the authored review rule, accept
    // iff weight ≤ −4 — witnesses 18/19's Theft weights accepted,
    // witness 0's Altercation weight rejected.
    let review = |weight: i32| weight <= -4;
    for (event, witness, weight) in [
        (1u32, 18u32, ENCOUNTER_TABLE[1].candidate.unwrap().1),
        (1, 19, ENCOUNTER_TABLE[1].candidate.unwrap().1),
        (2, 0, ENCOUNTER_TABLE[0].candidate.unwrap().1),
    ] {
        let key = MemoryKey {
            event: u64::from(event),
            witness,
            kind: MemoryKind::ReputationCandidate,
        };
        assert_eq!(
            spine.world_mut().lane.classify(&key, review(weight)),
            engine_core::npc_memory::ClassifyOutcome::Classified
        );
    }

    // Commit: witnesses 0 and 18 FULL; witness 19 DROPPED-ACK (the
    // journaled-but-unconfirmed boundary); witness 20 NEVER COMMITTED
    // (the unjournaled boundary).
    commit_witness_captures(&mut spine, &mut pipeline, &mut book, 0, true).unwrap();
    commit_witness_captures(&mut spine, &mut pipeline, &mut book, 18, true).unwrap();
    commit_witness_captures(&mut spine, &mut pipeline, &mut book, 19, false).unwrap();

    // The expected state, captured BEFORE the reopen: per witness,
    // the INDIVIDUAL canonical records with their per-key phases.
    let snapshot =
        |spine: &Spine<MemoryLaneWorld>, witness: u32| -> Vec<(MemoryKey, Vec<u8>, bool)> {
            spine
                .world()
                .lane
                .ledger()
                .witness_records(witness)
                .iter()
                .map(|record| {
                    (
                        record.key,
                        record.canonical_bytes(),
                        spine.world().lane.ledger().is_confirmed(&record.key),
                    )
                })
                .collect()
        };
    let expected: BTreeMap<u32, Vec<(MemoryKey, Vec<u8>, bool)>> =
        [0u32, 18, 19].map(|w| (w, snapshot(&spine, w))).into();
    let score_before = spine.world().lane.reputation_score().unwrap();
    let classifications_before: BTreeMap<u32, Vec<(MemoryKey, bool)>> = [0u32, 18, 19]
        .map(|w| (w, spine.world().lane.witness_classifications(w)))
        .into();
    let claimed_before = book.claimed.clone();
    let episodes_before = book.episodes;

    // THE COLD REOPEN: a fresh pipeline over the same store and
    // journal, then a fresh lane and book from the journal's plans
    // and the chain's confirmed identities — nothing in memory.
    drop(pipeline);
    let mut fresh = fresh_pipeline(&dir);
    let recovered = recover_memory_lane(&fresh).unwrap();

    // INDIVIDUAL canonical records and per-key phases, not counts.
    for (witness, records) in &expected {
        let recovered_records: Vec<(MemoryKey, Vec<u8>, bool)> = recovered
            .lane
            .ledger()
            .witness_records(*witness)
            .iter()
            .map(|record| {
                (
                    record.key,
                    record.canonical_bytes(),
                    recovered.lane.ledger().is_confirmed(&record.key),
                )
            })
            .collect();
        assert_eq!(
            &recovered_records, records,
            "witness {witness}'s individual canonical records and phases survive the reopen"
        );
    }
    // The applicable classification history and the classified score.
    for (witness, decisions) in &classifications_before {
        assert_eq!(
            &recovered.lane.witness_classifications(*witness),
            decisions,
            "witness {witness}'s classification history survives the reopen"
        );
    }
    assert_eq!(
        recovered.lane.reputation_score().unwrap(),
        score_before,
        "the classified score survives the reopen"
    );
    // The commit book: the claimed set and the next episode number.
    assert_eq!(recovered.book.claimed, claimed_before);
    assert_eq!(recovered.book.episodes, episodes_before);
    // The journaled boundary: witness 20's uncommitted captures are
    // honestly absent (the pipeline's InMemory discipline).
    assert_eq!(
        recovered.lane.witness_conservation(20),
        (0, 0, 0),
        "an unjournaled capture recovers nothing"
    );
    // The dropped-ack boundary: witness 19's keys recover PENDING.
    let (pending, confirmed_count) = recovered.lane.ledger().witness_counts(19);
    assert_eq!((pending, confirmed_count), (3, 0));
    // Every recovered witness starts dormant (the materialization is
    // the activate rebuild, never a silent partial live view).
    assert!(recovered.lane.is_dormant(18) && recovered.lane.live(18).is_none());

    // THE RECOVERED BOOK IS USABLE: install the recovered lane in a
    // fresh world and flush witness 19's dropped-ack episode with
    // ZERO new events.
    let mut world = MemoryLaneWorld::new();
    world.lane = recovered.lane;
    let mut spine2 = Spine::with_reservation(world, 200, 60);
    let mut book2 = recovered.book;
    let keys = commit_witness_captures(&mut spine2, &mut fresh, &mut book2, 19, true).unwrap();
    assert!(keys.is_empty(), "the recovered state needs no new episode");
    assert_eq!(
        spine2.world().lane.witness_conservation(19),
        (3, 0, 3),
        "the recovered lane finishes the dropped-ack episode"
    );
    assert!(fresh.is_confirmed(ConsequenceId::new(3, 2, 19)));
}

/// THE FINDING 3 DECIDING GATE (the composed half — the lane-level
/// reverse-ids/equal-ticks/duplicate gate is npc_memory's test 20):
/// a held encounter released LATE fires after larger event ids
/// already executed; the one retention order (canonical event-key
/// order, maintained by the live bounded view TOO) keeps the same
/// episodes before and after the dormancy round trip. Under the
/// defective bridge the live view kept the late-fired SMALL id
/// (execution order) while the rebuild kept the larger ids (key
/// order) — the round trip changed which episode the witness
/// remembers.
#[test]
fn the_one_retention_order_through_the_composed_world() {
    let mut spine = make_spine();

    // Region 1 (witnesses 6-11) at Full: two Greetings, event 30 at
    // tick 4 and event 20 at tick 5 (REVERSE IDS — the higher id
    // executes first).
    schedule_encounter(&mut spine, 30, EncounterKind::Greeting, 1, 1, Tick(4)).unwrap();
    schedule_encounter(&mut spine, 20, EncounterKind::Greeting, 1, 1, Tick(5)).unwrap();
    spine.run_ticks(5);

    // EQUAL TICKS at region 2 (witnesses 12-17): events 40 and 35 both
    // due at tick 20, delivered in the spine's own order — the one
    // retention order makes the live view independent of it.
    schedule_encounter(&mut spine, 40, EncounterKind::Greeting, 2, 2, Tick(20)).unwrap();
    schedule_encounter(&mut spine, 35, EncounterKind::Greeting, 2, 2, Tick(20)).unwrap();

    // Dormancy for region 1: the live view is discarded, and a new
    // encounter (event 10) targeting the dormant region is HELD.
    set_region_level(&mut spine, 1, LodLevel::Dormant);
    schedule_encounter(&mut spine, 10, EncounterKind::Greeting, 1, 1, Tick(8)).unwrap();
    assert_eq!(
        spine.held_count(),
        1,
        "the dormant region's encounter is held"
    );

    // Reactivation at EVENT-DRIVEN (budget 2): the rebuild keeps the
    // two largest keys {20, 30}; the held event then fires LATE (its
    // due was 8) and its SMALL id 10 is the one the budget evicts —
    // the canonical order, never the execution order.
    set_region_level(&mut spine, 1, LodLevel::EventDriven);
    spine.run_ticks(20);
    let live = spine.world().lane.live(6).unwrap();
    assert_eq!(
        live.episodic().iter().map(|e| e.event).collect::<Vec<_>>(),
        vec![20, 30],
        "the late-held event 10 (the smallest key) is evicted, never 20 or 30"
    );

    // THE ROUND TRIP: the actual retained traces, compared before and
    // after deactivation/reactivation at the same tier.
    let before: Vec<EpisodicMemory> = live.episodic().to_vec();
    set_region_level(&mut spine, 1, LodLevel::Dormant);
    set_region_level(&mut spine, 1, LodLevel::EventDriven);
    assert_eq!(
        spine.world().lane.live(6).unwrap().episodic(),
        before.as_slice(),
        "the dormancy round trip keeps the same episodes"
    );

    // The equal-tick pair at region 2: whatever order the spine
    // executed them in, the live view holds the canonical order.
    spine.run_ticks(1);
    let live = spine.world().lane.live(12).unwrap();
    assert_eq!(
        live.episodic().iter().map(|e| e.event).collect::<Vec<_>>(),
        vec![35, 40],
        "equal ticks tie-break by event id in the live view"
    );
    let before: Vec<EpisodicMemory> = live.episodic().to_vec();
    set_region_level(&mut spine, 2, LodLevel::Dormant);
    set_region_level(&mut spine, 2, LodLevel::Full);
    assert_eq!(
        spine.world().lane.live(12).unwrap().episodic(),
        before.as_slice(),
        "the equal-tick round trip keeps the same episodes"
    );

    assert_lane_conservation(&spine, "the one-order campaign");
}

// ---------------------------------------------------------------------------
// THE S6 FUZZ GATE — the design pass §8.4 item 4's "the S6 fuzz
// discipline, on the real pipeline": the S6-lite harness's seeded op
// distribution and fault classes over the REAL machinery (the spine,
// the census, the lane, the CommitPipeline), with the per-op
// conservation identities, the C1 rebuild checks at every
// activation, the C2 exactly-once faults, the independent ORACLE
// (deliberately separate code), and the double-run byte-identity.
// ---------------------------------------------------------------------------

/// The seeded SplitMix64 (the npc.rs mix64 precedent's generator
/// form): violations reproduce deterministically from the seed.
struct FuzzRng(u64);

impl FuzzRng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// THE ORACLE — deliberately separate code (the S6-lite Oracle
/// precedent): its own census loop, its own tier logic, its own
/// dedup log. No lane, no ledger, no spine.
struct Oracle {
    levels: [LodLevel; 4],
    seen: BTreeSet<u32>,
    /// The (event, witness) pairs already applied — the exactly-once
    /// unit, mirroring the lane's key shape.
    seen_pairs: BTreeSet<(u32, u32)>,
    /// event → the first execution's witness set (the oracle's OWN
    /// census, in its own id order).
    witnesses: BTreeMap<u32, Vec<u32>>,
    /// witness → the record keys it must hold: (event, kind tag).
    records: BTreeMap<u32, BTreeSet<(u32, u8)>>,
    /// (witness, actor) → the accumulated relationship sum.
    relationships: BTreeMap<(u32, u32), i64>,
    /// (event, witness) → (class, weight, decided, accepted).
    candidates: BTreeMap<(u32, u32), (u32, i32, bool, bool)>,
}

impl Oracle {
    fn new() -> Self {
        Oracle {
            levels: [LodLevel::Full; 4],
            seen: BTreeSet::new(),
            seen_pairs: BTreeSet::new(),
            witnesses: BTreeMap::new(),
            records: BTreeMap::new(),
            relationships: BTreeMap::new(),
            candidates: BTreeMap::new(),
        }
    }

    /// The oracle's OWN census: its own distance loop over the
    /// population with its own tier gate.
    fn census(&self, place: usize, radius: f32) -> Vec<u32> {
        let r2 = radius * radius;
        let mut out = Vec::new();
        for record in POPULATION.iter() {
            if self.levels[record.id as usize / REGION_NPCS] == LodLevel::Dormant {
                continue; // its own tier logic
            }
            let dx = record.pos[0] - PLACES[place][0];
            let dy = record.pos[1] - PLACES[place][1];
            if dx * dx + dy * dy <= r2 {
                out.push(record.id);
            }
        }
        out
    }

    /// Applies a log entry: the exactly-once unit is the (event,
    /// witness) PAIR — the lane's key shape. A replay re-delivering
    /// an event to a census that gained witnesses (the levels
    /// changed between executions) genuinely records for the NEW
    /// witnesses (their first delivery of that event); a re-delivery
    /// to an already-seen pair is the no-op the lane performs. The
    /// per-event FIRST witness set is kept separately (the C4
    /// witness-set comparison uses the first execution's census).
    fn apply(&mut self, event: u32, kind: EncounterKind, place: u8, region: u8) {
        let spec = &ENCOUNTER_TABLE[kind as usize - 1];
        let witnesses = self.census(place as usize, spec.radius);
        if self.seen.insert(event) {
            self.witnesses.insert(event, witnesses.clone());
        }
        let actor = POPULATION[(region as usize * REGION_NPCS) + (event as usize % REGION_NPCS)].id;
        for witness in &witnesses {
            if !self.seen_pairs.insert((event, *witness)) {
                continue; // the exactly-once unit: already delivered
            }
            let keys = self.records.entry(*witness).or_default();
            keys.insert((event, MemoryKind::Episodic.tag()));
            if let Some(delta) = spec.relationship {
                keys.insert((event, MemoryKind::Relationship.tag()));
                *self.relationships.entry((*witness, actor)).or_insert(0) += i64::from(delta);
            }
            if let Some((class, weight)) = spec.candidate {
                keys.insert((event, MemoryKind::ReputationCandidate.tag()));
                self.candidates
                    .insert((event, *witness), (class, weight, false, false));
            }
        }
    }

    /// The oracle's shared reputation: accepted candidates only.
    fn score(&self) -> i64 {
        self.candidates
            .values()
            .filter(|(_, _, decided, accepted)| *decided && *accepted)
            .map(|(_, weight, _, _)| i64::from(*weight))
            .sum()
    }

    /// The oracle's final-state digest (the double-run comparison).
    fn digest(&self) -> u64 {
        let mut fp = Fingerprinter::new();
        for event in &self.seen {
            fp.write_u32(*event);
        }
        for (witness, keys) in &self.records {
            fp.write_u32(*witness);
            for (event, tag) in keys {
                fp.write_u32(*event);
                fp.write_u8(*tag);
            }
        }
        for ((witness, actor), sum) in &self.relationships {
            fp.write_u32(*witness);
            fp.write_u32(*actor);
            fp.write_bytes(&sum.to_le_bytes());
        }
        for ((event, witness), (class, weight, decided, accepted)) in &self.candidates {
            fp.write_u32(*event);
            fp.write_u32(*witness);
            fp.write_u32(*class);
            fp.write_bytes(&weight.to_le_bytes());
            fp.write_u8(u8::from(*decided));
            fp.write_u8(u8::from(*accepted));
        }
        fp.finish()
    }
}

/// One fuzz campaign's collectable outcome (the double-run compare).
struct CampaignOutcome {
    lane_digest: u64,
    world_fingerprint: u64,
    spine_fingerprint: u64,
    oracle_digest: u64,
    confirmed_keys: Vec<ConsequenceId>,
    ops_by_class: [u64; 8],
    encounters: u64,
    records: u64,
    confirmed_records: usize,
    score: i64,
}

/// Runs the whole 200-op campaign (the S6-lite distribution, adapted
/// to the real machinery). Every fault class fires; every invariant
/// is asserted WHERE IT HOLDS (per-op conservation, activation-time
/// rebuild equality, duplicate-time digest immobility).
fn run_campaign(tag: &str, seed: u64) -> CampaignOutcome {
    const OPS: usize = 200;
    let dir = TestDir::new(tag);
    let mut spine = make_spine();
    let mut pipeline = fresh_pipeline(&dir);
    let mut book = MemoryCommitBook::default();
    let mut oracle = Oracle::new();
    let mut rng = FuzzRng(seed);
    let mut tick: u64 = 0;
    let mut next_event: u32 = 1;
    let mut oracle_log_index: usize = 0;
    let mut ops = [0u64; 8];
    // witness → (the level at deactivation, the live digest) — the
    // C1 expectation. A same-level activation must reproduce the
    // digest byte-identically; a tier-changing activation must keep
    // exactly the LAST `budget` episodic records (the retention
    // reconvergence, derived here independently from the ledger).
    let mut expected_rebuild: BTreeMap<u32, (LodLevel, u64)> = BTreeMap::new();

    // Drains the world's new log entries into the oracle (the
    // replays dedup by event id inside the oracle).
    fn drain_oracle(spine: &Spine<MemoryLaneWorld>, oracle: &mut Oracle, from: &mut usize) {
        for logged in spine.world().encounter_log.iter().skip(*from) {
            oracle.apply(logged.event, logged.kind, logged.place, logged.region);
            *from += 1;
        }
    }

    for op in 0..OPS {
        let choice = rng.below(100);
        if choice < 30 {
            // Advance.
            ops[0] += 1;
            let steps = 1 + rng.below(8);
            spine.run_ticks(steps);
            tick += steps;
            drain_oracle(&spine, &mut oracle, &mut oracle_log_index);
        } else if choice < 50 {
            // Presence transition.
            ops[1] += 1;
            let region = rng.below(4) as usize;
            let level = match rng.below(4) {
                0 => LodLevel::Full,
                1 => LodLevel::Simplified,
                2 => LodLevel::EventDriven,
                _ => LodLevel::Dormant,
            };
            let was = spine.world().levels[region];
            if level == LodLevel::Dormant && was != LodLevel::Dormant {
                // The C1 snapshot: every region witness with a live
                // representation records its digest.
                for record in POPULATION
                    .iter()
                    .take((region + 1) * REGION_NPCS)
                    .skip(region * REGION_NPCS)
                {
                    if let Some(live) = spine.world().lane.live(record.id) {
                        expected_rebuild.insert(record.id, (was, live.digest()));
                    }
                }
            }
            set_region_level(&mut spine, region, level);
            oracle.levels[region] = level;
            if level != LodLevel::Dormant && was == LodLevel::Dormant {
                // THE C1 CHECK at activation: the rebuilt digests
                // equal the snapshotted expectations.
                for record in POPULATION
                    .iter()
                    .take((region + 1) * REGION_NPCS)
                    .skip(region * REGION_NPCS)
                {
                    if let Some((snap_level, snap_digest)) = expected_rebuild.remove(&record.id) {
                        let live =
                            spine.world().lane.live(record.id).unwrap_or_else(|| {
                                panic!("op {op}: the activated witness is live")
                            });
                        if snap_level == level {
                            assert_eq!(
                                live.digest(),
                                snap_digest,
                                "C1 (I1): op {op} — witness {}'s same-level rebuild is byte-identical",
                                record.id
                            );
                        } else {
                            // The tier changed across the dormant
                            // window: the depth is the CURRENT tier's.
                            // The rebuild must keep exactly the LAST
                            // `budget` episodic records — derived here
                            // independently from the ledger's record
                            // sequence (not by re-running the fold).
                            let budget =
                                engine_core::npc_perception::perception_budget(match level {
                                    LodLevel::Full => 0,
                                    LodLevel::Simplified => 1,
                                    LodLevel::EventDriven => 2,
                                    LodLevel::Dormant => 3,
                                });
                            let episodic: Vec<EpisodicMemory> = spine
                                .world()
                                .lane
                                .ledger()
                                .witness_records(record.id)
                                .iter()
                                .filter_map(|r| match r.delta {
                                    engine_core::npc_memory::MemoryDelta::Episodic { trace } => {
                                        Some(EpisodicMemory {
                                            event: r.key.event,
                                            trace,
                                        })
                                    }
                                    _ => None,
                                })
                                .collect();
                            let keep = budget.min(episodic.len());
                            assert_eq!(
                                live.episodic(),
                                &episodic[episodic.len() - keep..],
                                "C1 (I1): op {op} — witness {}'s tier-changing rebuild keeps exactly the last {keep} episodic records",
                                record.id
                            );
                        }
                    }
                }
            }
        } else if choice < 62 {
            // Encounter.
            ops[2] += 1;
            let region = rng.below(4) as u8;
            let kind = match rng.below(3) {
                0 => EncounterKind::Altercation,
                1 => EncounterKind::Theft,
                _ => EncounterKind::Greeting,
            };
            let event = next_event;
            next_event += 1;
            schedule_encounter(&mut spine, event, kind, region, region, Tick(tick + 1)).unwrap();
        } else if choice < 70 {
            // Commit (full).
            ops[3] += 1;
            let witness = POPULATION[rng.below(24) as usize].id;
            commit_witness_captures(&mut spine, &mut pipeline, &mut book, witness, true).unwrap();
        } else if choice < 77 {
            // Commit with the dropped-ack fault.
            ops[4] += 1;
            let witness = POPULATION[rng.below(24) as usize].id;
            commit_witness_captures(&mut spine, &mut pipeline, &mut book, witness, false).unwrap();
        } else if choice < 85 {
            // Duplicate record delivery (C2).
            ops[5] += 1;
            let log_len = spine.world().encounter_log.len();
            let logged = if log_len == 0 {
                None
            } else {
                spine
                    .world()
                    .encounter_log
                    .get(rng.below(log_len as u64) as usize)
                    .cloned()
            };
            if let Some(logged) = logged {
                let digest_now = spine.world().lane.digest();
                let actor = POPULATION[(logged.region as usize * REGION_NPCS)
                    + (logged.event as usize % REGION_NPCS)]
                    .id;
                let spec = &ENCOUNTER_TABLE[logged.kind as usize - 1];
                for witness in &logged.witnesses {
                    for kind in [
                        MemoryKind::Episodic,
                        MemoryKind::Relationship,
                        MemoryKind::ReputationCandidate,
                    ] {
                        let authored = match kind {
                            MemoryKind::Episodic => {
                                Some(engine_core::npc_memory::MemoryDelta::episodic(
                                    *witness,
                                    actor,
                                    logged.tick,
                                    u32::from(logged.kind.tag()),
                                ))
                            }
                            MemoryKind::Relationship => spec.relationship.map(|d| {
                                engine_core::npc_memory::MemoryDelta::Relationship {
                                    actor,
                                    delta: d,
                                }
                            }),
                            MemoryKind::ReputationCandidate => spec.candidate.map(|(c, w)| {
                                engine_core::npc_memory::MemoryDelta::ReputationCandidate {
                                    actor,
                                    class: c,
                                    weight: w,
                                }
                            }),
                        };
                        if let Some(delta) = authored {
                            let record = MemoryRecord {
                                key: MemoryKey {
                                    event: u64::from(logged.event),
                                    witness: *witness,
                                    kind,
                                },
                                delta,
                                tick: logged.tick,
                            };
                            let outcome = spine
                                .world_mut()
                                .lane
                                .capture_and_apply(record, 32)
                                .unwrap();
                            assert!(
                                outcome != CaptureOutcome::Captured,
                                "C2 (I1): op {op} — the duplicate delivery of event {} witness {witness} is a no-op",
                                logged.event
                            );
                        }
                    }
                }
                assert_eq!(
                    spine.world().lane.digest(),
                    digest_now,
                    "C2 (I1): op {op} — the lane digest survived the duplicate delivery"
                );
            }
        } else if choice < 92 {
            // Event replay (C2): a NEW spine event carrying an
            // ALREADY-USED encounter identity.
            ops[6] += 1;
            let log_len = spine.world().encounter_log.len();
            let logged = if log_len == 0 {
                None
            } else {
                spine
                    .world()
                    .encounter_log
                    .get(rng.below(log_len as u64) as usize)
                    .cloned()
            };
            if let Some(logged) = logged {
                schedule_encounter(
                    &mut spine,
                    logged.event,
                    logged.kind,
                    logged.place,
                    logged.region,
                    Tick(tick + 1),
                )
                .unwrap();
            }
        } else {
            // Classification review (the authored rule: accept iff
            // weight ≤ −4).
            ops[7] += 1;
            let pending: Vec<(u32, u32)> = oracle
                .candidates
                .iter()
                .filter(|(_, (_, _, decided, _))| !*decided)
                .take(3)
                .map(|((event, witness), _)| (*event, *witness))
                .collect();
            for (event, witness) in pending {
                let (class, weight, _, _) = oracle.candidates[&(event, witness)];
                let accept = weight <= -4;
                let key = MemoryKey {
                    event: u64::from(event),
                    witness,
                    kind: MemoryKind::ReputationCandidate,
                };
                let outcome = spine.world_mut().lane.classify(&key, accept);
                assert_eq!(
                    outcome,
                    engine_core::npc_memory::ClassifyOutcome::Classified,
                    "M-08: op {op} — the review classifies the pending candidate (class {class})"
                );
                if let Some(entry) = oracle.candidates.get_mut(&(event, witness)) {
                    entry.2 = true;
                    entry.3 = accept;
                }
            }
        }

        // PER-OP CONSERVATION (both identities, every op).
        assert_lane_conservation(&spine, &format!("op {op}"));
        assert_spine_conservation(&spine, &format!("op {op}"));
        drain_oracle(&spine, &mut oracle, &mut oracle_log_index);
    }

    // The final state: activate every region (the comparable state).
    for region in 0..4usize {
        set_region_level(&mut spine, region, LodLevel::Full);
        oracle.levels[region] = LodLevel::Full;
    }
    spine.run_ticks(1);
    drain_oracle(&spine, &mut oracle, &mut oracle_log_index);
    assert_lane_conservation(&spine, "the campaign end");
    assert_spine_conservation(&spine, "the campaign end");

    // ---- THE C4 ORACLE COMPARISON (the independent reference) ----
    let world = spine.world();
    for record in POPULATION.iter() {
        // The record keys.
        let lane_keys: BTreeSet<(u32, u8)> = world
            .lane
            .ledger()
            .witness_records(record.id)
            .iter()
            .map(|r| (r.key.event as u32, r.key.kind.tag()))
            .collect();
        let oracle_keys = oracle.records.get(&record.id).cloned().unwrap_or_default();
        assert_eq!(
            lane_keys, oracle_keys,
            "C4: witness {}'s record keys match the oracle",
            record.id
        );
        // The relationship sums.
        let live_rel = world
            .lane
            .live(record.id)
            .expect("every witness is active at the end")
            .relationships()
            .clone();
        let mut oracle_rel: BTreeMap<u32, i64> = BTreeMap::new();
        for ((witness, actor), sum) in &oracle.relationships {
            if *witness == record.id {
                oracle_rel.insert(*actor, *sum);
            }
        }
        assert_eq!(
            live_rel, oracle_rel,
            "C4: witness {}'s relationship sums match the oracle",
            record.id
        );
        // The candidate phases.
        let live_cand = world.lane.live(record.id).unwrap().candidates().clone();
        for ((event, witness), (class, weight, decided, accepted)) in &oracle.candidates {
            if *witness != record.id {
                continue;
            }
            let key = MemoryKey {
                event: u64::from(*event),
                witness: *witness,
                kind: MemoryKind::ReputationCandidate,
            };
            let live = live_cand.get(&key).unwrap_or_else(|| {
                panic!("C4: witness {witness}'s candidate for event {event} is live")
            });
            assert_eq!((live.class, live.weight), (*class, *weight));
            let expected_phase = match (decided, accepted) {
                (false, _) => engine_core::npc_memory::CandidatePhase::Pending,
                (true, true) => engine_core::npc_memory::CandidatePhase::Accepted,
                (true, false) => engine_core::npc_memory::CandidatePhase::Rejected,
            };
            assert_eq!(
                live.phase, expected_phase,
                "C4/M-08: witness {witness}'s candidate {event} phase matches the oracle"
            );
        }
    }
    // The witness sets (the oracle's OWN census per FIRST execution
    // — a replay's log entry carries its own census, compared only
    // when it IS the first entry for its event id).
    let mut first_entries: BTreeSet<u32> = BTreeSet::new();
    for logged in world.encounter_log.iter() {
        if !first_entries.insert(logged.event) {
            continue; // a replay entry — not the first execution
        }
        let oracle_set = oracle
            .witnesses
            .get(&logged.event)
            .unwrap_or_else(|| panic!("C4: event {} is in the oracle", logged.event));
        assert_eq!(
            &logged.witnesses, oracle_set,
            "C4: event {}'s first-execution witness set matches the oracle's own census",
            logged.event
        );
    }
    // The shared reputation.
    assert_eq!(
        world.lane.reputation_score().unwrap(),
        oracle.score(),
        "C4/M-08: the shared reputation matches the oracle"
    );

    // The collectable outcome.
    let mut fp = Fingerprinter::new();
    spine.world().fingerprint(&mut fp);
    let records: u64 = POPULATION
        .iter()
        .map(|r| spine.world().lane.witness_conservation(r.id).0)
        .sum();
    let confirmed_records: usize = POPULATION
        .iter()
        .map(|r| spine.world().lane.witness_conservation(r.id).2)
        .sum();
    let mut confirmed_keys = pipeline.confirmed_keys();
    confirmed_keys.sort_unstable();
    CampaignOutcome {
        lane_digest: world.lane.digest(),
        world_fingerprint: fp.finish(),
        spine_fingerprint: spine.lane_state_fingerprint(),
        oracle_digest: oracle.digest(),
        confirmed_keys,
        ops_by_class: ops,
        encounters: world.executed_encounters,
        records,
        confirmed_records,
        score: world.lane.reputation_score().unwrap(),
    }
}

/// THE S6 FUZZ GATE (§8.4 item 4): 200 seeded ops — the S6-lite
/// distribution (advance 30%, presence 20%, encounter 12%, commit
/// 8%, dropped-ack 7%, duplicate delivery 8%, event replay 7%,
/// review 8%) — over the real spine, the real census, and the real
/// CommitPipeline. I1 (the per-witness conservation, every op; the
/// C1 rebuild equality at every activation; the duplicate-delivery
/// digest immobility), I2 (the census's tier gate, through the
/// world's own execution), I3/C4 (the independent oracle's full
/// agreement), M-08 (the pending-until-classified score, the review
/// rule), and the double-run byte-identity (two whole campaigns from
/// the same seed: the lane digest, the world and spine fingerprints,
/// the oracle's final state, and the pipeline's confirmed set all
/// equal — violations reproduce deterministically from the seed).
#[test]
fn s6_fuzz_memory_lane_i1_through_i3_exactly_once() {
    const SEED: u64 = 0xC0FF_EE01;

    let started = Instant::now();
    let first = run_campaign("s6-fuzz-run-1", SEED);
    let second = run_campaign("s6-fuzz-run-2", SEED);
    let elapsed = started.elapsed();

    assert_eq!(
        first.lane_digest, second.lane_digest,
        "I1/double-run: the lane digests are byte-identical"
    );
    assert_eq!(
        first.world_fingerprint, second.world_fingerprint,
        "double-run: the world fingerprints are byte-identical"
    );
    assert_eq!(
        first.spine_fingerprint, second.spine_fingerprint,
        "double-run: the spine lane-state fingerprints are byte-identical"
    );
    assert_eq!(
        first.oracle_digest, second.oracle_digest,
        "C4/double-run: the oracle's final states are byte-identical"
    );
    assert_eq!(
        first.confirmed_keys, second.confirmed_keys,
        "double-run: the pipelines' confirmed sets are byte-identical"
    );
    assert_eq!(
        first.ops_by_class, second.ops_by_class,
        "double-run: the op streams are identical"
    );
    assert_eq!(
        first.encounters, second.encounters,
        "double-run: the encounter counts are identical"
    );

    // The pod-observational summary (labeled; the rig is the
    // owner's).
    println!(
        "s6 fuzz gate: 2x200 ops [advances {}, presence {}, encounters {}, commits {}, dropped-acks {}, duplicates {}, replays {}, reviews {}] — {} encounters executed, {} records captured ({} pipeline-confirmed), shared reputation {}, seed {SEED:#x}, {:.2?} for both campaigns (pod-observational)",
        first.ops_by_class[0],
        first.ops_by_class[1],
        first.ops_by_class[2],
        first.ops_by_class[3],
        first.ops_by_class[4],
        first.ops_by_class[5],
        first.ops_by_class[6],
        first.ops_by_class[7],
        first.encounters,
        first.records,
        first.confirmed_records,
        first.score,
        elapsed,
    );
}
