# PostRust — Current Status Index

**Purpose:** the maintained, always-current index of what the engine actually
is — integrated, tested, measured, and pending. The stage records in
`docs/ENGINE-WORKSPACE-NARRATIVE.md` and `docs/` are **historical acceptance evidence, retained
as history and never rewritten**; this file is the live layer on top of them.
A fresh development or review agent should be able to determine the actual
current status, active authority, source of truth, known limitations, and next
work from THIS file alone.

- **Extended for the owner's sign-off walk on 2026-11-27:** the
  owner ran the maximal composition route (`--corner --populated
  --behavior --shadows --weather rain --wet --day-night 20 --post
  --free-look --audio` — every corner feature live at once) and the
  era's separate visual routes: the composition runs clean (the
  owner's judgment: "it looks fine"), and **the audit's held-open
  visual question is ANSWERED: natural-intensity ripples are NOT
  owner-visible on bare wet asphalt** — consistent with the
  audit's own prediction (~0.19 expected simultaneously-visible
  impacts at the corner's ring budget; the forced-field gates
  prove the displacement reaches the framebuffer at ±22/255
  measured sweep, but a ~2 px sample shift at campaign ring
  density sits below the perceptual threshold without a dark
  water patch and a tracked reflected feature to frame it).
  Puddles do not exist yet (the prototype was the audit's one
  endorsed-but-unbuilt item). **The puddle package is now the
  foregrounded next work item** — the audit's question-7
  specification (6–10 authored standing-water patches, the
  still-water composition, ring confinement, and its acceptance
  table) is the path to readable impacts; the sign-off walk's
  negative result is the package's motivating evidence.
- **Era 2 CLOSED here (the owner's completion directive, 2026-11-27):**
  the owner reviewed the density question, walked the composition
  again ("I have just now seen ripples for the very first time" —
  the flat-daylight variant), and directed completion: every
  finding of both independent audits (the Era-2 audit's 1–19 and
  the sign-off review's 20–28) is remediated on `main`, every
  corrective green on all five CI jobs, and the one open
  presentation question — the ripple density — is resolved as a
  WALK-TIME TOOL, not a unilateral policy change:
  `--corner --weather rain --ripple-decimation <n>` swaps ONLY the
  admission rate (the shipped 320:1 policy and its exact pins stay
  the default — MEASURED on the rig: the default route's census
  spawned=17, the 80:1 arm spawned=71 with 15 active at exit, the
  typed refusals loud). The owner's future walk decides the re-pin
  (40/80/160) vs the puddle package; the density A/B
  (`--benchmark-ripple-visibility`) and the knob are the decision
  instruments. **Era 2 is code-complete, owner-walked, and
  remediation-verified — the sign-off review's own closure
  criteria met (all three blocker packages landed: findings 20–21
  via PR #68, finding 22 via #69, findings 23–26 via #70, the
  reporting items 27–28 and the diagnostic in the same); the
  puddle prototype, the warmed combined-view allocation scope,
  and the owner decision lists (NPC-AI §9, T-06 physics, the
  audio ambitions) are ERA 3 PLANNING INPUTS, not Era 2 work.**
  The next era begins with the owner's plans.
- **Reconciled for the sign-off review remediation's completion on
  2026-11-27:** the independent sign-off review at `ec4eb3c`
  (`docs/ERA2-REMEDIATION-SIGNOFF-ec4eb3c.md`) held Era-2 sign-off
  across findings 20–28; all are now remediated on `main` through
  three corrective PRs (plus the R10 fold-in), each green on all
  five CI jobs with its measured evidence in its commits: **R7**
  (PR #68, findings 20–21 — the memory lane's stable episode
  identity across definite journal rejections and mid-flight apply
  failures, the chain-authoritative reconciliation sweep, the
  scoped confirmation, the late classification's own journal
  transaction, and the recovered lane's tier materialization; four
  deciding gates), **R8** (PR #69, finding 22 — both host seams
  count `realloc`, and the composed wake gate attributes the world
  callback through a matched control running the identical wake
  path with the recording skipped: the differential zero on every
  frame, the deliberate-callback-allocation negative control
  proving the teeth), **R9** (PR #70, findings 23–26 — the headless
  extension prerequisite and the platform-surface condition, the
  both-predictions receiver-order discrimination (the correct
  order's interval vs the reversed order's exact prediction, both
  computed at the measured witness; the poison-fix exposure of the
  windowed-surface extent clamp fixed beside it), the natural
  birth's term separation (the displacement-only pair must carry
  its own ink, with the inverse-ray support test), and the finite
  kinematic extent at emitter admission), and **R10** (folded into
  PR #70, findings 27–28 — the calibration-history amendment below,
  the per-NPC drain census closing the redistribution escape, the
  synthesis digests labeled predictions, and the
  `--benchmark-ripple-visibility` pre-puddle density A/B: the
  shipped 320:1 arm measured 15 admissions per 10 s window = the
  1.5/s × 2.5 s ≈ 3.8 steady-state live rings, 160:1 doubling and
  80:1 quadrupling the opportunity with zero refusals — the
  opportunity is continuous; the per-ring movement is the
  readability question, the owner's higher-admission walk decides
  between a decimation re-pin and the puddle package). This pass
  settles the §4 table at the PR #70 endpoint `ce55627` — main
  **1419/2/33 Linux, 1368/2/33 Windows**, renderer release
  **172/2**, heavy **651/46/64 Linux, 650/46/64 Windows** — every
  count enforced by the runs on the exact trees (R7's `45910242798`
  green at 649/648 heavy, R8's `45911022713` green at 651/650, the
  R9+R10 runs `45914046788` and `45914624711` green with no count
  moved — every deciding check in R9/R10 lives inside existing
  test functions). STILL OPEN, by design: the puddle prototype
  (question 7 — the density A/B printed above is the decision
  input; the owner's higher-admission walk decides), the
  cross-package warmed combined-view allocation scope (R8's
  counters now make it measurable), and the owner decision lists
  (NPC-AI §9, T-06 physics, the audio ambitions). Sign-off returns
  to the owner and the next audit.
- **Amended per the sign-off review's finding 27 (2026-11-27):**
  the remediation records below called the R3/R4 calibration
  rounds "never a test weakened" — that phrase overstated what the
  calibration history preserved, and the retained evidence now
  carries the honest form (old predicate, measured counterexample,
  replacement property, per substantive change):
  - **R4's forced-field floor (round 6):** the first candidate
    required a displayed-RGB MAJORITY of the differing pixels;
    Ubuntu measured 12,087 of 97,961 = 12% (the darker lavapipe
    mirror gives the UV displacement less rgb to move) while
    Windows passed with a majority — the counterexample that
    falsified the majority as a cross-platform property. The
    replacement: the absolute floor of 5,000 displayed-RGB-
    qualified pixels (2.4× margin under Ubuntu's measurement,
    ~10× under Windows's) — a weaker predicate than the majority on
    bright mirrors, a stronger one on dark ones, chosen for
    cross-platform validity with the margin recorded. The majority
    remains enforced where it is the acceptance property (the
    rain broadside gate's upper-third majority — the R3 addition).
  - **R3's receiver-blend witness floors (rounds 4–7):** the
    original signed-dry PRESENCE floor became ABSOLUTE presence
    (|wet−dry| ≥ 2 — a streak under bright ground dims it, which is
    still presence), and the qualification floors fell
    (wet 165→25, presence 15→3, contrast 50→8) — each fall pinned
    from a measured near-miss on a platform the old floor excluded,
    with the flip asserts carrying the discrimination. The R9
    corrective (PR #70) then replaced the sign-flip predicate
    itself with the both-predictions interval form — the review's
    finding 24 demonstrated the flip's vacuity at dimming
    witnesses (both orders dim), and the interval form decides at
    any witness with the separation recorded.
  - **R3's ripple campaign pin:** the exact production campaign
    pin was re-measured for the documented contact-geometry change
    (the world receiver plane — finding 12's correction moved the
    landings); the old pin is historical, the new pin exact, and
    the CHANGE is recorded as a production-policy change with its
    cause, never as a gate weakening.
  - **The count-only `ci:` corrections** (d35917a, 9c1d646, the
    R6/R7/R8/R9 pins): test-count pins corrected from measured
    actuals — a different class from predicate changes; no gate
    floor, discriminator, or coverage moved in any of them.
  The exact-count history and every red run's actuals remain
  separately traceable in the commits; this amendment records the
  predicate history the "never weakened" phrase collapsed.
- **Reconciled for the Era-2 audit remediation's completion on
  2026-11-26:** the independent audit at `959889d`
  (`docs/ERA2-LIVING-WORLD-AUDIT-959889d.md`) held Era-2 sign-off
  open across nineteen findings; all nineteen are now remediated on
  `main` through six corrective PRs, each green on all five CI jobs
  and each carrying its own measured evidence in its commits:
  R1 = findings 1–3 (the memory lane's self-sufficient retries, the
  reconstructable recovery proof, the retention-order contract; PR
  #60), R2 = findings 4–5 (the bounded locomotion tick, the bounded
  wake log; PR #61), R3 = findings 12/13/14/18 (the world receiver
  plane, the billboard turn, the ordered blend, the one-tick
  presentation; PR #62), R5 = findings 9–11 (the atomic
  trigger+listener publication, the committed-phase footsteps, the
  honest callback stats; PR #63), R4 = findings 15/19 (the
  camera-correct ripple displacement and the gate truth; PR #64 —
  whose final round found and fixed a genuine cross-platform test
  defect: `gate_pixel_ray`'s missing ASPECT factor, which had
  compressed every inverse-projected ground point 1.778× toward the
  screen-center column, landing authored ripple rings at the wrong
  world centers on BOTH platforms — the shader, packing, upload,
  and forward projection were correct all along; the root-cause
  chain, traced with an instrumented ladder through the WSL
  lavapipe parity pod, is recorded in PR #64's commits), and
  R6 = findings 6/7/8/16/17 (the witness matcher, the identity wire
  seam and the versioned diagnostics, the paired E-09, the census
  identities, the validation refusals; PR #65). This pass settles
  the §4 table at the R6 endpoint `b4da1b9` — main
  **1418/2/33 Linux, 1367/2/33 Windows**, renderer release
  **172/2**, heavy **645/46/64 Linux, 644/46/64 Windows** — every
  count ENFORCED by the exact-count gates of the runs on the exact
  trees (R4's `45900716161` on `cba6827`, R6's `45901849045` on
  `8002dad`, both green on all five jobs; both merges
  fast-forward-shaped — the merge trees are exactly the branch
  trees those runs validated). STILL OPEN from the audit, by
  design: the puddle prototype (question 7 — endorsed by the audit
  once the world-space landings and layer order were correct, both
  now landed; the package itself awaits the owner's visual
  acceptance walk), the finding-20 branch-protection process
  recommendation (an owner settings decision, never made by an
  agent), and the cross-package warmed combined-view allocation
  scope (the audit's warm-coverage note — a candidate for the next
  measurement package). Sign-off returns to the owner and the next
  audit.
- **Maintained since:** 2026-11-14 (the post-audit remediation cycle, F-13);
  reconciled for Cycle 1 on 2026-11-15.
- **Reconciled for the PostRust extraction on 2026-11-20:** the engine moved
  from the Sektor7 game repository to this repository
  (https://github.com/Willpatpost/PostRust) with full history; the workspace
  now lives at the repository ROOT (no `engine/` prefix — historical records
  below still use it). This pass also brought the §4 counts and the
  capability matrix current through Cycle 5 + perf-hardening batch 1; the
  extraction baseline is `main` @ `b19d210`, five-job CI run `44827915274`.
- **Reconciled for the Era-2 landing on 2026-11-24:** the owner's GO wave
  (PRs #33–#36, 23:45Z) merged the whole Era-2 mission — package 1 plus the
  three recursive-phase documents; this pass settles the §4 table at
  `e4842d6`, marks the merged state throughout, and records the package's
  post-merge second-device evidence.
- **Reconciled for the Era-2 package C landing on 2026-11-25:** the
  owner's GO (21:53:23Z, `04b09b9`) merged the whole particles +
  weather package — the deterministic cores (digest
  `16840467954493462700`), the renderer's third populated kind,
  the demo rain layer with the gate battery, and the strengthening
  fold-ins; this pass settles the §4 table at the merge endpoint
  (main 1371/1320, renderer release 163/2, heavy 625/624 with
  ignored 39 — all enforced by the runs' own gates), carries the
  cores' records into §2, the arc into §9, the matrix row, and
  records the two-seat build (the restart's orphaned session and
  the supervisor's session, one canonical core).
- **Reconciled for the Era-2 package A landing on 2026-11-25:** the
  owner's GO wave (18:26:20–18:37:23Z) took the whole eleven-PR stack
  terminal — #38, #41–#47 merged (four real merge commits carrying the
  linear chain, four already-incorporated markings), #39/#40 closed as
  incorporated (their heads landed on `main` through #41's merge), and
  #48 merged last (branch updated to `2fd72b9` first, run `45675511392`
  green, then the GO via `d782a27`); this pass settles the §4 table at
  `d782a27` — `main` itself, the first settlement whose tree is a merge
  endpoint rather than a candidate branch — marks the merged state
  throughout, and records the wave's merge mechanics for the morning
  audit.
- **Extended for the Era-2 package A part-1 landing on 2026-11-25:** the
  owner's GO merged PR #37 (the NPC AI deterministic cores) at 02:02Z —
  after this reconciliation's branch was cut, racing its push by 45
  minutes; the extension re-settles the §4 table at `d9a034d` and carries
  the package's record into §2, §9, and the capability matrix, so the
  index is current at merge time rather than born one landing behind.
- **Extended for the Era-2 package A part 8 delivery on 2026-11-25:** the
  composed-crowd camera crossing (part 4's documented deferral closed — the
  full crosswalk crossings on the shared clock) delivered on
  `era2/npc-ai-composed-crowd`, stacked on the part-7 tip; this pass
  settles the §4 table at that tree (main 1313/2/33 unchanged and heavy
  620/33/63, both Linux MEASURED with the workflow's own awk) and carries
  the package's record into §2, §9, and the matrix.
- **Extended for the Era-2 package A part 7 delivery on 2026-11-25:** the
  capture-lane gates (§8.4 item 4, the last unlanded design-pass gate)
  delivered on `era2/npc-ai-capture-lane`, stacked on the s6-soak-seam
  tip; this pass settles the §4 table at that tree (main 1313/2/33 and
  heavy 618/33/63, both Linux MEASURED with the workflow's own awk) and
  carries the package's record into §2, §9, and the matrix.
- **Update rule:** every change that alters capabilities, test counts,
  performance evidence, or priorities updates this file in the same change.

---

## 1. Historical stage-completion records (where the history lives)

| Record | Location |
|---|---|
| The stage-by-stage workspace narrative (Stages 0–5, the stage banner) | `docs/ENGINE-WORKSPACE-NARRATIVE.md` — historical, retained |
| The architecture decision ledger (T-01…, accepted and deferred) | `docs/DECISIONS.md` |
| Stage 4 physical acceptance (the owner's rig) | `docs/STAGE4-WINDOWS-ACCEPTANCE-RECORD-2026-10-21.md` |
| Stage 5 Batch 1–5 acceptance records | `docs/STAGE5-B1-WIRE-FORMAT.md` … `STAGE5-B5-LIFECYCLE-RECORD.md` |
| Slice 0 integration acceptance | `docs/SLICE0-INTEGRATION-ACCEPTANCE-RECORD-2026-10-26.md` |
| Production-persistence line records (PP-B1…B4) | `docs/PP-B1-SQLITE-PERSISTENCE-RECORD.md` … `PP-B4-ORCHESTRATION-VALIDATION-RECORD.md` |
| Stage 6 evidence schema, burst validation, soak runbooks | `docs/STAGE6-*.md` |
| The Era-2 recursive-phase research set — the NPC AI design pass, the physics middleware decision, the audio subsystem map | `docs/ERA2-NPC-AI-DESIGN-PASS.md`, `docs/PHYSICS-MIDDLEWARE-DECISION.md`, `docs/AUDIO-SUBSYSTEM-MAP.md` (PRs #34/#35/#36, merged 2026-11-24) |
| The game-side vision and design bible | `VISION.md`, `DESIGN-BIBLE.md` in the **Sektor7 game repository** (https://github.com/Willpatpost/Sektor7 — they moved with the game at the extraction); `docs/TECH-ARCHITECTURE.md` here |

These are point-in-time acceptance records. Later work does not edit them to
appear further along; later state lives in this index.

## 2. Current integrated engine capabilities (what `main` actually carries)

Five-crate workspace at the repository root (`crates/`), all cross-platform (Linux, Windows, macOS
ARM64) with a five-job CI:

- **`engine-core`** — the deterministic simulation spine: `(due, seq)` event
  queue, cross-class causal dependencies, per-tick budget, execution ledger
  with first-class lateness, the safe `processed_through` watermark, FNV-1a
  fingerprints, canonical wire primitives + full-fidelity spine round-trip;
  plus the Cycle-1 **player movement/collision prototype module**
  (kinematic mechanics, std-only, behind a narrow `CollisionWorld` query
  contract — §8 scope); plus the perf-hardening **zero-allocation main
  path** (2026-11-20): diagnostics construction is threshold-gated
  (`Diagnostics::would_log` — a suppressed record allocates nothing),
  handler follow-ups collect into a reused scratch, partially-released
  blocked events keep their registrations on the remaining prerequisites,
  and the unmet-dependency paths are allocation-free — gated by the
  `spine_allocations` integration test (a counting global allocator
  pinning BOTH allocation events and bytes for a multi-dependency
  scenario, engine-side attribution only: cold window 18 events /
  6880 bytes on a fresh spine, warm window 9 / 3024 in steady state,
  against 87 / 12990 for the SAME scenario measured on the
  pre-optimization implementation; every fingerprint and scenario pin
  byte-identical, and the measured sustained-load execution cost
  dropped p50 1.24 -> 0.51 ms/window with identical trace digests); plus
  the Era-2 **NPC AI deterministic cores** (package A part 1, PR #37,
  MERGED 2026-11-25 — the design pass's §3–§6 as three additive pure
  engine-core modules, every existing test untouched): `npc_behavior.rs`
  (the two-lane record — `NpcIdentity` the durable lane, `NpcRoutine`
  the reconstructible; the needs counters with fixed-order clamped
  decay; the pure rule-table selector with needs-ranked scoring,
  schedule authority, and per-NPC RNG-stream tie-breaks — a fixed rule
  table over a fixed record produces the same activity sequence
  forever, the 24-decision FNV digest pinned at
  1277463877274275604), `npc_schedule.rs` (the calendar layer:
  authored `Commitment`/`Cadence` pure data with integer u64 tick
  arithmetic and checked overflow — no floats anywhere; the
  fixed-order due iteration and the fast-forward `next_due` for
  time-skip; the due-sequence digest pinned at 11670746710459133860),
  and `npc_perception.rs` (the inverted-query census — WHICH NPCs lie
  within a radius of a point, squared f32 distances in fixed
  operation order with NO sqrt; the fail-closed admissible gate — a
  non-finite coordinate can never authorize a witness, the gate that
  caught a real overflow bug during its own gating; the four-tier
  perception budget table; the durable trace record's canonical
  little-endian encoding, its digest pinned at 10766371085850726021).
  The engine/policy boundary held throughout: the selector MACHINERY,
  the calendar queries, and the census mechanism are engine; the rule
  tables, schedule content, and perception classes are policy authored
  above. The **spine integration** (package A part 2, `npc_execution.rs`
  — §6.2's contract landed on the real queue, additive, the spine
  itself untouched): the **event payload codecs** (tags 0xA1/0xA2 in
  bits 63..56, the full-32-bit npc field, the 24-bit arg with the
  TYPED pack refusal — an unrepresentable arg is `None`, never a
  silent truncation; the namespace-collision proof against the
  routine layout pinned by test); `evaluate_wake` (the per-event
  LAZY need decay — cost per event, not per tick, §4.2's EventDriven
  shape; the schedule-authority selection from the EVENT's own
  payload, never a calendar re-derivation; and the HONEST RE-ARM
  anchored at actual execution — a late wake skips missed
  occurrences, never fires them in a burst); `evaluate_witness`
  (§4.1's perception selection boundary — the fired perceptions
  offered to the rule table, no schedule authority);
  `enumerate_witnesses` (the §5.1 inverted census into a
  caller-owned cleared scratch — population order, the LANDED
  four-tier budget table as the single tier authority, the
  fail-closed radii); `retain_trace` (§5.2's retention: oldest-first
  eviction at the tier budget, the zero-budget append refused);
  and `execution_digest` (the §8.4 gate-2 byte pin over (tick, npc,
  activity) in `decision_digest`'s exact field order) — pinned by a
  14-test battery whose five INTEGRATION tests drive a real
  `Spine::with_reservation` through a test-side `World` (game
  policy by placement): the seeded double-run byte-identity (stream
  digest AND world fingerprint), the measured stream-digest pin
  10778018066104731562, the E-10 synchrony cluster (40 same-tick
  near commitments absorbed by the reservation, none dropped), the
  late-arrival honest skip (intended 5, executed 9 under a held
  reservation — the re-arm jumps the missed 8-occurrence to 11, the
  chain on time from the shift onward), and the witness census with
  `depends_on` causality (the dormant NPC inside the radius
  witnesses nothing). `npc_schedule`'s `Commitment::next_occurrence`
  made public as the per-entry re-arm anchor (the private logic,
  byte-identical). The **locomotion batch** (package A part 3,
  `npc_locomotion.rs` — §4.3's near-lane batch + §4.2's Full-tier
  row landed, additive, the spine and every existing module
  untouched): **routes as pure data** (`RouteLeg { to, turn }` from
  an authored origin — per-leg heading TURNS AUTHORED in u16
  turn units, the placement-table pattern extended to paths: no
  runtime atan2/sin/cos exists anywhere; leg lengths/units/perps
  precomputed at `Route::new`, the module's only sqrt,
  IEEE-correctly-rounded at authoring; zero-length legs,
  non-finite fields, unclosed loops, and non-positive
  speeds/strides are typed refusals); **the hand-spread draws**
  (speed scale [0.9, 1.1), lateral fraction [-1, 1] × the route's
  authored width, and the initial stride phase — three draws in
  fixed order from the appearance seed's OWN stream, never the
  selector's, so locomotion reconstruction never depends on
  selector replay order); **the position bridge** (continuous f32
  in fixed operation order; the committed location stays coarse —
  `leg` mirrored into `route_progress` at arrival, the
  `committed_cell` quantizer onto the (u8, u8) region grid); the
  **identity-ordered zero-allocation batch step** (`advance_batch`
  — fixed-order per-NPC arithmetic: the crossing loop with
  remainder carry bounded by MAX_CROSSINGS_PER_TICK, arrival snaps
  to the AUTHORED waypoint bytes, loop wraps, the shortest-way
  INTEGER turn blend at an authored rate, the smoothstep lateral
  fade with the no-pop-at-corners window, the stride phase from
  DISTANCE); the **pinned-blocked collision composition** (the
  player's `depenetration` contract, near tier only,
  ground-constrained — a blocked tick reverts the along-leg
  distance and freezes the phase: no foot slide, byte-idempotent
  across blocked ticks, the walk resuming the instant the path
  clears); the **speed-band clip selection** (`clip_for` —
  policy's threshold); and `locomotion_digest` (the FNV byte pin
  over npc/position-bits/facing/phase-bits/leg) — pinned by an
  18-test battery whose two CAMPAIGN gates drive a real
  `Spine::with_reservation` (a commitment-driven MoveTo through
  `evaluate_wake`, a LOOPING stroll, a standing waiter, an absent
  far-tier NPC): the double-run byte-identity (locomotion digest
  AND activity stream) and the measured locomotion-digest pin
  852983358677453085. The §8.5 measurement discharged on this pod
  (`--benchmark-locomotion`, release, 2,048 samples per config):
  the corner's 24-walker shape p99 **0.0047 ms/tick** (the
  µs-scale [ASSUMPTION] confirmed), the 1,000-walker pathological
  all-present case p99 **0.0707 ms/tick** with the collision query
  (0.42% of a 60 Hz frame), the all-blocked worst case p99
  0.0497 ms/tick, and the no-query arithmetic p99 0.0184 ms/tick
  — every configuration double-run identical. The rendering bridge
  has LANDED as package A part 4 (the corner demo's populated route
  on the locomotion state — see `engine-slice`'s corner_routes
  paragraph); the composed gates remain as package A's final part.
  The **memory-lane core** (package A part 7, `npc_memory.rs` — the
  design pass §5.2/§5.3 + §8.4 item 4's engine half, additive, the
  spine and every existing module untouched): the idempotency-keyed
  capture ledger (MemoryKey = the EVENT's key × witness × kind, the
  S6-lite harness's mem:/rel:/cand: family; MemoryDelta = §5.3's
  three writes — the episodic PerceptionTrace reused verbatim, the
  witness→actor relationship delta, the reputation candidate) with
  capture-at-production (I1 by construction: the discard can never
  precede the capture), first-capture-wins idempotence, the async
  confirm transition, and the canonical-global-key-order
  witness_records merge (both phases, never phase-partitioned — the
  rebuild's retention depends on the event order); the live
  representation (WitnessMemory: the bounded episodic Vec through
  the EXISTING retain_trace, the checked relationship accumulators,
  the candidate phases); the transition surface (capture_and_apply
  — the single-sourced exactly-once event path with the
  active/dormant materialization gate: a dormant witness's records
  accumulate ledger-only; deactivate/activate — the discard and the
  canonical-order rebuild with the durable classification decisions
  re-applied, fail-closed overflow); and the M-08 shared campaign
  reputation (`reputation_score` — one shared fold over the LEDGER's
  accepted candidates only, representation-independent so a
  deactivation cannot move it) — pinned by an 18-test battery
  (main-shard platform-uniform) including the phase-interleave pin,
  the inactive-witness gate, the retention-bounded rebuild, and the
  accepted-only fold with the cross-cycle score durability. Plus
  the Era-2 **package C particle + weather cores** (`particle.rs`
  + `weather.rs`, MERGED with PR #51, 2026-11-25 — the animation
  core's discipline applied to unbound particles): the per-emitter
  **seeded SplitMix64 spawn stream** (the `NpcRng` family's
  arithmetic verbatim, seven fixed-order draws per spawn) — a
  fixed seed's trajectory sequence byte-identical forever,
  cross-platform, pure IEEE f32, **no transcendental anywhere**;
  **the pinned integration order** (gravity → drag the per-step
  multiply → position → life → the death test, stable compaction
  — spawn order among survivors is part of the byte contract);
  the fixed-array bounded pool (1024, active prefix, zero
  tick-path allocation) with the **typed, counted, per-tick-
  reported overflow refusal** (`TickReport` must be observed —
  the `audio_triggers` discipline) and the accumulator clamp;
  `Emitter::set_volume` (the mid-campaign volume refresh —
  validates, replaces only the config, preserves the stream);
  `fold_state` (the FNV-1a fold over position+life bits) with
  **the trajectory digest pin `16840467954493462700`** (seed
  `0xC0FFEE`, the rain-like campaign, 600 ticks at dt = 1/60 —
  measured from the run, held byte-exact through every merge and
  both seats' independent runs); the weather model as pure data
  (`RainPolicy` caller-supplied — the engine carries no rain
  numbers; `rain_emitter_config` over the OBSERVED volume, the
  listener-relative discipline, `None` on the dry path;
  `Weather::rain_validated` the fail-closed policy form beside
  the clamping blend form); and the **N-emitter composition
  equivalence gate** (four emitters together vs alone —
  byte-identical, no shared state exists to introduce silently).
- **`engine-persist`** — the persistence stack: SHA-256 content-addressed
  revision store; `CanonicalMedium` boundary with **two production mediums
  carrying distinct durability contracts** — SQLiteMedium
  `TransactionalWalFull` (WAL + `synchronous=FULL`, both verified by
  querying the actual configuration at open) and LocalFileMedium
  `FullPosixSequence` on supported POSIX / `ProcessCrashOnly` on Windows
  (see §5 and §8); the durable T1 journal (hash-chained transaction
  frames); the `CommitPipeline`
  (single-authority per-key lattice, canonical confirmation); the
  recovery machinery (strict temporal boundaries, authenticated ancestry
  witnesses, recovery-state classification, degradation matrices); the
  epoch-checked record constructors.
- **`engine-renderer`** — the Vulkan backend (ash): swapchain with recreation
  handling, present-mode preference with FIFO fallback, depth-correct scene,
  the dynamic region-scene vertex path (the F-02 caller-held scratch form),
  the retained overlay-vertex buffer (F-05), GPU timestamps, validation-layer
  routing, the debug bitmap overlay, the Cycle-1 **dynamic-geometry
  truncation counters** (F-03 Step 1: per-frame region/overlay
  omitted-vertex counts with supplied/capacity context, reported in every
  run summary — fail-visible over the former silent clamps), and the
  Cycle 6-E **instanced proxy path**: the instance-rate vertex input
  (mesh binding + packed-instance binding), the lazily-created
  per-frame-slot mapped instance buffer and per-kind mesh VBOs
  (zero per-frame allocation — the counters stay constant across
  warmed populated frames), one draw call per populated proxy kind,
  and the per-object reference submission mode kept as the
  equivalence gate's N-draws side; the Cycle 6-E2 **indirect
  submission** on the same path: one `vkCmdDrawIndirect` per
  populated kind with the instance counts read from GPU memory
  (the lazily-created per-frame-slot mapped command buffer, the
  byte-pinned pure command packing, byte-identical output to the
  direct instanced draw — the mechanism the deferred GPU-culling
  package authors counts through). The Era-1 **population v2**: the
  shipped proxy meshes are the v2 silhouettes (`populated_meshes` —
  the eleven-box humanoid, the sedan with libm-free cylinder
  wheels; the instance transform contract unchanged, origin at
  feet/ground-center).
  Cycle 6-F **directional shadow path**: the deterministic light-space
  cascade core, the per-slot shadow maps behind a depth-only pass
  (an external subpass dependency orders the writes before the main
  pass's fragment reads), the shadowed lit pipeline sampling the map
  (Era-1 **PCF soft shadows** — the 3×3 `shadow_filter` tap table
  over the validated map, a compare-time analytical bias shared
  verbatim with the CPU oracle, both sides carrying the same
  out-of-bounds-reads-clear convention), the known-answer map
  readback, and the `--corner --shadows` route — all lazily created
  (the unshadowed path's allocation pins untouched, its renders
  pixel-identical). The Era-1 **sky layer** (`--corner --sky`, the
  sticky `set_sky` setting): the fullscreen-triangle sky pass drawn
  first behind the scene with no depth interaction (`sky.wgsl`
  mirroring `sky.rs`'s equation; the palette and disc direction from
  `atmosphere_at`; the ray reconstructed PER-FRAGMENT from the
  CPU-computed inverse view-projection — the package-6 first-run
  correction: the original per-vertex form affine-interpolated
  denormalized directions and never rendered the disc) — lazily
  created, and a fully-covered frame stays byte-identical with the
  sky on and off. The Era-1 **day/night cycle** (`--corner
  --day-night <seconds>`): the triangle-wave clock
  (`sky::cycle_position`) advances the atmosphere with the demo
  clock — the lit scene's light parameters and the sky layer ride
  the same `atmosphere_at` frame each frame (the free-look path
  uses the wall clock; the scripted route the presented-frame count
  at its reference 60 Hz, deterministic). The Era-1 **post chain**
  (`--corner --post`, the sticky `set_post` setting; package 6, the
  ladder's architectural step): the HDR render-target chain — the
  scene pass into an offscreen R16G16B16A16_SFLOAT target (+ its
  depth attachment, per frame slot), the 1/4-res bloom mip chain
  (the bright-passed 2×2 bilinear-footprint box downsample, then
  the separable [1,4,6,4,1]/16 blur, `bloom.wgsl` mirroring
  `post.rs`'s chain), and the composite pass (post.wgsl's ACES
  tonemap + the bloom add-back at `post.rs::BLOOM_STRENGTH`) into
  the swapchain — textureLoad-exact through the whole chain (the
  lavapipe gate pins presented pixels against post.rs's reference
  at 0/255 measured), lazily created with per-frame-slot
  ping-pong images (the shadow-map discipline) and NO per-frame
  uniforms (the PostParams block is extent-static), composes with
  every corner flag, and the postless path stays byte-identical.
  The **shadowed-HDR populated variant** (`era2/shadowed-hdr-populated`,
  2026-11-25 — Era-2 package 1's documented follow-up, landed): the
  HDR scene pass's shadowed arm now draws the population through
  the shadowed-populated state's lazily-ensured HDR pipeline
  variants (`ShadowedProxyResources::ensure_hdr` — the same shader
  modules, layouts, and sets as the main-pass pipelines, only the
  HDR render pass differs; the `skinned.ensure_hdr` precedent,
  per-frame idempotent so pipeline replacement re-ensures) —
  closing the last corner-flag composition gap: `--corner
  --populated --shadows --post` previously REFUSED on its first
  frame ("populated route drew no proxy calls" — the HDR arm
  passed `None` for the shadowed proxy pipelines and the
  recorder's belt-check skipped the population; MEASURED on the
  pod before the fix). The composition gate
  (`shadowed_populated_composes_through_the_hdr_chain`,
  render_post): the defect pin (2 draws), the presence ink (34166
  bytes vs the unpopulated frame), the one-time populated
  families' delta (6, 4, 16, 0) then constant with the warmed
  frame byte-identical, and the decisive full-chain known answers
  — a light-facing torso wall deep in a building's shadow
  rendering exactly `albedo·ambient` through bloom+ACES (57,600
  pixels, max 1/255) and the same wall in open sun rendering the
  full lit equation (57,600 pixels, max 1/255; the equations
  differ by 0.360 linear — the fixture pair discriminates both an
  always-ambient and a map-ignoring pipeline).
  The Era-2 **animation + skinning + proxy-shadows package**
  (PR #33, MERGED 2026-11-24 — the owner's GO; the branch deleted on
  merge): the **deterministic animation core** (`animation.rs`
  — the pure-data clip model, joint rotations through the existing
  libm-free `sin_cos_turn` machinery, signed i16 turn
  interpolation, the column-major hierarchy composition, and the
  skinning equation `skin_vertex` with its bit-exact rest-identity
  proof; a fixed clip's poses pinned by FNV-1a byte digests — the
  walk clip at 64 samples hashes to 2694074108253436588); the
  **skinning equation's WGSL mirror** (five modules — the
  unshadowed and shadowed npc lit paths, the shadowed vehicle lit
  path, and the two depth-cast paths — mirroring `skin_vertex`
  term-for-term, the UV passing through UNSKINNED as authored); the
  **pose storage buffer and the per-kind draw split** (the
  lazily-created per-frame-slot mapped pose buffer at binding 5,
  `POSE_STRIDE` 704 bytes with the alignment proof, the npc kind
  through the skinned pipeline with two dynamic offsets and the
  vehicle kind unchanged, the pose-index claim on the instance
  params vec4's x — a versioned layout change both packed-bytes
  FNV pins fired on and were repinned from); the **shadowed
  populated path** (stage 4b: both kinds CAST — the npc kind
  through the skinned depth pipeline, the vehicle kind through the
  plain one — and both RECEIVE through their shadowed lit
  pipelines, the lazily-created `ShadowedProxyResources` with
  caller-supplied per-family bind structs so the binding-2
  dynamic-offset stride is never derived inside the draw;
  `--corner --populated --shadows` works, the Era-1 typed refusal
  retired); and the **populated raycaster oracle** (`shadows.rs` —
  the structured Frame3 with the exact transpose inverse,
  `NpcOccluder`/`VehicleOccluder` with per-instance world-AABB
  bounds culling measured at ≈8 skips per texel,
  `map_texel_depth_populated` and `is_shadowed_populated`). The
  stage-5 **gates**: the skinning CPU-vs-GPU oracle (one walking
  npc's torso face, per-pixel against `skin_vertex` through a
  dead-on camera derived from the skinned rectangle, with the
  rest-pose differential), the populated shadow map vs the
  raycaster at rest and at frame-33 walk under the 6-F discipline
  (the per-class census printed for the tolerance's first
  device-measured evidence), the allocation counters (Vulkan AND
  host) constant across warmed WALKING frames with per-frame pose
  digests proving the crowd moves, and the animated N-vs-1
  submission equivalence.
- **`engine-shell`** — the SDL3 application shell (statically-linked vendored
  SDL 3.4.18; real window + offscreen/dummy modes).
- **`engine-slice`** — the integration/demo driver: the stdin CLI, `--window`,
  `--render` (with `--regions` LOD visualization), `--sim` (headless
  deterministic), `--demo-persist` (the integrated authoritative campaign
  behind the render loop, with the soak/evidence machinery), the session
  crossing (founded-and-anchored or recovered, the Space action, the durable
  close), the benchmark harnesses
  (`--benchmark-persistence`, `--benchmark-instrumentation`,
  `--benchmark-entity-scale`, `--benchmark-render`), and the Cycle-1
  additions: **`--benchmark-sustained-load`** (the F-08/F-12 sustained-scale
  demonstration command) with **`--sustained-load-full-matrix`** (the full
  reference matrix; the raw evidence record is archived in the
  coordination ledger — §7) and **`--player` / `--player-headless`** (the
  player prototype's interactive and deterministic paths), and the Era-1
  **`--corner --free-look`** eyeball camera (the Stage-2 6-DoF machinery
  wired to the shell's input intents, clamped to the corner scene's world
  AABB — presentation only: the interactive camera never enters the spine,
  a fingerprint, or any measured claim, and the scripted route's
  measurement series print only with the mode off), and the Era-2
  **animated population** (merged with the renderer's Era-2 paragraph
  in PR #33): the placement table's PHASE column (the 8
  crosswalk waiters stand, the 16 crossers and strollers walk at
  hand-spread u16 phase fractions of the 1.1 s stride — no
  lockstep marching), the per-frame walk evaluation
  (`corner_population::evaluate_population_poses` — the frame
  counter reduces modulo 66 IN INTEGERS before any float forms, so
  the walk loop closes bit-exactly at stride boundaries, pinned),
  writing into a retained buffer the renderer repacks per frame
  (zero per-frame allocation on both sides), and the walking
  `--corner --populated` route (plus `--populated --shadows`
  together through the shadowed populated path). Era 2 package A
  part 4 REPLACED the demo route's source with BEHAVIOR STATE: the
  `corner_routes` module (the corner's walking lanes as pure data —
  eight far-segment flank loops with lanes exactly 0.75 m apart at
  dyadic 4.375/5.125, two crosswalk pacers at the original
  crossers' spots, the waiters byte-identical; the swept-validity
  battery measuring every sample unconditionally with the margins
  as measured minimums; the 663-frame temporal campaign pinning
  the double-run byte-identity, the locomotion digests, and the
  pairwise clearance — walker pairs ≥ 0.75 with the measured
  minimum EXACTLY the lane separation, waiter pairs ≥ 0.7 the
  original cardinal rule) and the demo's `LocomotionCrowd`
  (render-then-step at the deterministic clock: `advance_batch`
  at dt = 1/60, `corner_instances_into` into the retained instance
  buffer, the locomotion pose evaluation into the retained pose
  buffer — 0 host allocations per warmed moving frame,
  `renderer.allocations()` constant, both MEASURED). The
  phase-column path remains pub where gates pin it (the skinning
  oracle, the populated shadow-map gate). Era 2 package A part 5
  composed the BEHAVIOR SPINE into that route (`corner_behavior`:
  the 24 authored schedules, the `CornerCrowdWorld` spine world
  with the policy seam, `--corner --populated --behavior`), and
  part 6 added the **measurement surface** (`--benchmark-behavior`,
  `behavior_measurement.rs` — §8.4 item 7 + §8.5's remaining
  measurements over the real machinery: the behavior-workload
  sustained matrix, the witness enumeration cost, the E-09
  activation crossover, the E-08 skip-cost curves, memory-per-NPC
  by tier; pod-observational with the double-run determinism
  asserted in every section, its 5-test lib battery pinning the
  workload digests — §9's arc record carries the measured numbers).
  The **memory-lane composition** (package A part 7,
  `npc_memory_lane.rs` + `tests/npc_memory_lane.rs` — §8.4 item 4's
  driven half, the corner_behavior precedent: game policy by
  placement): the authored policy table (three encounter kinds with
  the radii/relationship-deltas/candidate classes, the 24-NPC
  population with the measured adjacency minimums, the payload
  codec with the namespace-collision proof against the wake/witness
  tags), the composed world (impl World — the REAL
  enumerate_witnesses census tier-gated by the region levels, the
  fixed-order per-witness captures through capture_and_apply, the
  encounter log, the versioned fingerprint domain
  engine-slice/memory-lane-world/1), the LOD transitions over the
  REAL spine machinery (suspend_ids/resume_matching/schedule_held,
  with the BETWEEN-LEVEL immediate retention re-application — the
  memory depth follows the tier now, the fuzz gate's own catch),
  and the REAL pipeline ride (commit_witness_captures in the
  crossing's staged-commit order: stage → the fsynced T1 journal
  capture → mark_applied → commit_canonical, the dropped-ack fault
  as the withheld submit, the global actionable-set confirmation
  mapping back to the lane's per-record confirms). The gates: the
  driven census known answers, the I2 dormant gate (the
  geometrically-inside witness excluded and re-included through the
  world's own execution), the held lane, the byte-identical rebuild
  with the mid-cycle C2 duplicate, the composed
  pending-until-classified reputation with the M-08 cross-cycle
  durability, the real pipeline ride (confirmation, the
  dropped-ack, the C2 re-delivery, the fresh-open recovery read),
  and THE S6 FUZZ GATE — 2×200 seeded ops in the S6-lite
  distribution with all fault classes (duplicate delivery, dropped
  acks, event replay, reviews) over the real spine/census/
  CommitPipeline, the per-op conservation identities (lane AND
  spine), the C1 rebuild checks at every activation (same-level
  byte-identity; tier-changing cycles keep exactly the LAST budget
  episodic records, derived independently), the C4 independent
  oracle, and the double-run byte-identity — MEASURED on this pod:
  38 encounters executed, 431 records captured (188
  pipeline-confirmed), the shared reputation −105 (21 accepted
  Theft candidates at −5), 310 ms for both campaigns (tmpfs-backed
  temp dirs; the durability semantics are the machinery's, pinned
  by the b3/b4 batteries on real mediums) — a NORMAL test, no
  #[ignore].
  The **composed-crowd camera crossing** (package A part 8,
  `corner_routes`/`corner_behavior`, 2026-11-25 — part 4's
  documented deferral closed): the two crosswalk PACERS became
  full-crossing CROSSERS, each walking the whole road arm through
  its crosswalk (curb-to-curb minus the waiter standoff — the
  turn-arounds at ±3.7 are the furthest points that clear the
  standing waiters by the walker-waiter margin; f32 measures
  0.79999995, 3.7 is not dyadic), crossing the camera's road line
  BEHIND the camera on the SHARED CLOCK. The three-leg loop shapes
  (west: origin [−5.0, 1.5] north to 3.7, the full southbound
  crossing to −3.7, the full northbound crossing back to the
  origin — closing with no turn at the origin; north: the mirror)
  keep the ORIGINAL crossers' exact positions, facings, and seed
  draws — frame 0 is byte-identical to the pacers' own, and the
  part-4 pose-bytes pins at frames 0/33/300 hold UNCHANGED (the
  stride-phase stream depends on the walking speed and draws, not
  the route geometry). THE TIMING (authored against the camera's
  schedule — SCENE_EXTENT 40: the camera passes the west crosswalk
  line x=−5 at pose ~116.5 and the north crosswalk line z=5 at pose
  380, parking at [0, 1.7, 38] beyond pose 600): crosser 8 crosses
  z=0 at steps 259/583/907/1231 (poses 199/523/847/1171), crosser 9
  crosses x=0 at steps 268/604/939/1275 — every line-crossing
  outside the camera's own crosswind-pass windows, the measured
  same-frame minimum **1.8529123 m at step 178** (crosser 8
  southbound mid-crosswalk, the camera at x=−4.57 exiting the west
  arm) against the 0.8 gate. THE GATES: the two TEMPORAL CAMERA
  GATES (the package's headline) — the plain crowd over 1280 steps
  (the demo's full 663-frame presented sequence plus both crossers'
  second-loop crossings against the parked camera) and the composed
  campaign over its 663 ticks, both under the DEMO'S OWN PAIRING
  (the crowd steps from the timing warmup's first loop iteration
  while the camera's pose advances only in the measured window —
  the owner sees crowd step k with camera pose clamp(k−60, 0, 600)),
  every frame's whole population ≥ 0.8 from the camera's same-frame
  position, the measured minimum pinned in both (the composed gate
  measures the plain gate's own value — a cross-check between the
  gates); the semantic timing bounds in pose-index terms (crosser
  8's first crossing past the camera's west-arm exit; none of
  crosser 9's crossings inside the camera's ±1.5 m north-crosswalk
  window [370, 390]); the sweep battery restructured (the geometric
  camera_min scoped to the flank routes — the crossers cross the
  spline BY DESIGN, the temporal gate owns their camera safety —
  measuring 4.375 exactly over the flanks; the crossers'
  crosswalk-strip band membership over the full crossing span, 0
  violations; cross_route_min re-measured 1.3768167 — crosser 8's
  west band edge vs the E-W north-west flank's road-side lane;
  waiter_min 0.79999995 — the standoff itself; sedan_min unchanged
  1.395); the composed schedules re-authored (crossers 8/9: kind
  200 → the far curb (waypoint 2) at tick 90, kind 201 → the near
  curb (waypoint 1) at tick 510, both every(840) — every retarget
  lands after a curb arrival, never mid-crossing: the crossing
  takes ≤ 457 frames from frame 0 at the slowest draw, so 510
  always fires after the far-curb stand begins; the curb stands are
  the pedestrian waiting on the far side), the wake count 99 → 82
  by the new cadence arithmetic, the arrivals re-measured 9 (the
  crossers' 2 + the strollers' 7), the composed digest pins at
  300/662 re-measured (ticks 0/33 EQUAL the plain campaign's own
  pins — no wake fires before 90); the pairwise walker discipline
  stays the 663-frame campaign test's own scope (the same-loop
  stroller drift bound is a 663-frame design bound — the temporal
  gates' 1280-step window explicitly does NOT claim it). Every
  existing gate re-fired green (render_instanced 7/7 and
  render_shadows 4/4 on the pod's lavapipe; the routes ran
  end-to-end: `--populated`, `--populated --behavior` (the exit
  summary reproducing 82 wakes / 9 arrivals digit-for-digit), and
  `--populated --shadows`, all 601 frames with validation 0/0).
  The **Era-2 package C weather layer** (2026-11-25, PR #51's
  stage 4+5, `corner_weather.rs`): the `--corner --populated
  --weather rain` route — the deterministic rain field over the
  living corner. The demo's `RainLayer` (the LocomotionCrowd
  pattern): the engine-core `Emitter` (the weather model's
  `rain_emitter_config` over the demo's authored `RainPolicy`:
  rate 480/s, terminal fall ~9 m/s via gravity 9.81 + drag
  0.982, lifetimes 0.9–1.4 s, the kill plane at the ground) with
  the warm start (132 ticks — the first presented frame is
  already wet), the per-frame listener-following spawn box
  (x/z-quantized to 4 m cells so the field never slides with the
  camera, refreshed through the ADDITIVE `Emitter::set_volume` —
  the immutable-config gap the stage closed, validated exactly as
  construction), the render-then-step fixed clock (dt = 1/60),
  and the billboard streaks (the turn derived PHYSICALLY as
  t = −yaw·65536/τ — `instance_columns` maps the mesh's +Z
  normal to (sin t, 0, cos t); the camera at yaw θ looks along
  (sin θ, 0, −cos θ); no quarter-turn offset — the first draft's
  +16384 turned every streak edge-on, caught in the line review),
  the tint riding the day/night cycle's `atmosphere_at` ambient,
  `RAIN_MATERIAL` 0 (the asphalt noise texture), `RAIN_ALPHA`
  0.35. The exit summary's census line: MEASURED
  spawned=6360 died=6049 refused=0 final-active=311
  volume-refreshes=21 over the 601-frame route (the conservation
  exact, the pool-bound proof at the demo boundary). The stage-5
  gates (`tests/render_weather.rs`, the five #[ignore]d lavapipe
  gates + the packer's typed-refusal lib test): the ink-count
  (MEASURED 492 differing bytes vs the rainless baseline, floor
  200), the warmed both-counter allocation constancy (12
  distinct captures — the field moves), the double-run
  byte-equality, the rain-kind N-vs-1, and the populated-shadowed
  composition smoke — all green on the pod's lavapipe. The
  coordination record (four actors, two sessions, the contract
  consolidation) lives in PR #51's trail and the pod-root
  OVERNIGHT-STATUS.md. The **strengthening fold-ins** (`7749ebd`,
  the package's final commit): the **N-emitter composition
  equivalence gate** (four emitters, distinct seeds/volumes/rates,
  128 ticks together in fixed Vec order vs each alone —
  byte-identical alive prefixes; the gate exists to catch any
  future shared-state introduction), the **broadside ink gate at
  the presentation extent** (the pixel metric over the full
  1280×720, the 60-frame steady state, the dry double-run
  baseline, and the sky-region check — MEASURED 1660 pixels
  total, 1350 in the upper third: rain provably IN FRONT of the
  background, the depth interaction's visible half; floors
  1200/900, ~7× above the edge-on collapse), and
  **`Weather::rain_validated`** (the fail-closed policy form —
  finite and inside [0,1] or a typed refusal; the clamping
  `rain()` kept and documented as the interpolation/blend form).
  The renderer's own half (the third populated kind —
  `particle.wgsl` unlit tinted alpha-blended, depth test-on/
  write-off, `ProxyInstance.alpha` in the tint vec4's `.a` with
  the existing packed-bytes pins UNCHANGED, the third indirect
  command, no rain in the shadow map) is recorded in the
  engine-renderer section above; the whole ladder's evidence and
  the two-seat record live in PR #51's body and commits.

The **integrated campaign demo** (`--demo-persist`) is the vertical slice:
one authoritative, persistently-orchestrated 1000-NPC / 8-region campaign
with LOD, crash recovery, soak evidence, and rendering together.

## 3. Production mechanisms vs demo-only implementations

| Layer | Class |
|---|---|
| `engine-core` spine, `engine-persist` store/journal/pipeline/recovery, `engine-renderer` Vulkan backend, `engine-shell` | **Engine libraries** — production mechanisms (no demo coupling) |
| `engine-slice` scenarios (`stage6-v1`), the CLI modes, the benchmark harnesses, the debug overlay content | **Demo/driver layer** — deliberately not the game |
| `composite-test-fixture` (engine-persist), `crash-test-seams` (engine-slice) | **Test-only features** — compile-time isolated, never in production builds (CI re-proves the feature-disabled production path) |
| The Vulkan renderer's current shape (static scene buffers, host-visible persistently-mapped dynamic buffers, 2 frames in flight, bounded region-vertex capacity with **fail-visible truncation counters** — F-03 Step 1 delivered; the Step-2 capacity/refusal policy deferred) | **Prototype-class** rendering design — correct and validated, not the final architecture (see §8) |
| The Cycle-1 player prototype (`engine-core` player module + the `--player` demo layer) | **Prototype-class** kinematic mechanics — reusable engine surface, reversible, explicitly not a ratified production physics backend |

### November 21 audit and corrective candidate

The audit examined `219f800`. Its fixes landed directly on `main` as
`a4eb372`, spanning 34 files. The change covered journal recovery and
reclamation, scheduler and player input, drawable sizing and geometry
capacity, whole-run timing, the CLI and documentation. Four existing
ignored Vulkan smoke tests were run separately. Three gained assertions.
The old `overlay_slot_regions_are_disjoint` test covered offsets in a
shared mapped buffer. Per-slot overlay buffers retired that layout, and
the replacement test covers on-demand capacity growth.

The review candidate combines `a4eb372` with Batch 3 (`e0b52e8`).
It retains the pipeline cache, hybrid frame pacing, validation gating and
release shader-orientation check. The validation queue updates its pending
flag under the same mutex as its messages. Resize hints trigger a drawable-pixel
extent check. CPU and GPU percentiles both use whole-run `TimingSamples`.
The journal sidecar now allows a short writer handoff and reports typed
contention after two seconds. Tests exercise double-open and release, frame
end arithmetic at the integer boundary, and capture reconciliation after
an operating-system sync error.

## 4. Current automated test coverage

The main and heavy CI jobs check the number of passes, ignored tests and
result lines. The table gives the local Linux and Windows measurements
for the combined candidate.

| CI partition | Linux | Windows |
|---|---:|---:|
| Main shard (`--workspace --exclude engine-slice`) | 1418 | 1367 |
| Heavy shard (`-p engine-slice --features crash-test-seams`) | 645 | 644 |
| **Combined** | **2063** | **2011** |

The macOS job has its own partition. Main has two ignored tests and 33
result lines. Heavy has forty-six ignored tests and 64 result
lines. The table is settled at the landed `main` `b4da1b9`
(2026-11-26, PR #65's merge — the Era-2 audit remediation's
endpoint): main **1418/2/33 Linux, 1367/2/33 Windows**, renderer
release **172/2**, heavy **645/46/64 Linux, 644/46/64 Windows** —
every count ENFORCED by the exact-count gates of the runs on the
exact trees: R4's run `45900716161` on `cba6827` (PR #64 — the
aspect-factor corrective; its pins main 1406/1355 and renderer
release 172 measured by that run and by run `45877898296`'s
zero-failure counts, the stale-163/39/40 failure messages corrected
to name the actual pins) and R6's run `45901849045` on `8002dad`
(PR #65 — the polish package; main 1418/1367 and heavy 645/644,
Windows main MEASURED locally with the step's own awk at 1367/2/33
on the exact tree, the Linux counts platform-uniform inferences
CONFIRMED by the run itself) — both green on all five jobs, both
merges fast-forward-shaped (the merge trees are exactly the branch
trees those runs validated). The remediation wave's count history
is on record in each corrective's own `ci:`-prefixed commits
(every red run's actuals recorded, never a test weakened); R4's
calibration history — six forced-field floor rounds, then the
aspect-factor root cause traced through the WSL lavapipe parity
pod — is on record in PR #64's trail. The prior settlement,
superseded: the landed `main` `04b09b9`
(2026-11-25, PR #51's merge — the package C endpoint): main
**1371/2/33**, renderer release **163/2**, heavy **625/39/64** —
every count ENFORCED by the exact-count gates of the runs on the
exact trees: the branch's final run `45702469099` on `7749ebd`
(the merge was fast-forward-shaped — `04b09b9`'s tree is EXACTLY
`7749ebd`'s) green on all five jobs with its own lines: `main
shard: 1371 passed, 2 ignored, 33` and `renderer release: 163
passed, 2` and `heavy shard: 625 passed, 39 ignored, 64` on
ubuntu-latest; `main shard: 1320 passed, 2 ignored, 33` and
`heavy shard: 624 passed, 39 ignored, 64` on windows-latest — the
Windows platform-uniform inferences CONFIRMED by the run itself;
the merge-endpoint run `45708420586` on `04b09b9` re-enforces the
same counts on `main`. The package's own count history is on
record in its commits: the red run `45693271859` (the stale-pin
push — stage 3's tests never absorbed by the then-pin 1339)
corrected from actuals at `8128ae2` (1369/1318, heavy 625/624
with ignored 38), then the strengthening fold-ins at `7749ebd`
(+2 main: the N-emitter composition gate and the validated
intensity battery, 1371/1320; +1 ignored heavy: the broadside ink
gate, 39) — never a test weakened. The prior settlement,
superseded: the landed `main` `d782a27`
(2026-11-25, PR #48's merge — the evening GO wave's endpoint; the
FIRST settlement whose tree is a merge commit on `main` itself
rather than a candidate branch): main **1313/2/33**, renderer release
**160/2**, heavy **620/33/63** — all three Linux shards MEASURED
with the workflow's own awk on this exact tree (the settlement
checkout, code-identical to `d782a27`; zero failures anywhere);
Windows **1262** main / **619** heavy = the platform-uniform
deltas, **CONFIRMED by the endpoint run's own exact-count gates**
(run `45677048777` on `d782a27`: `main shard: 1313 passed, 2
ignored, 33 test-result lines` and `renderer release: 160 passed,
2` and `heavy shard: 620 passed, 33 ignored, 63` on ubuntu-latest;
`main shard: 1262 passed, 2 ignored, 33` and `heavy shard: 619
passed, 33 ignored, 63` on windows-latest). No count moved anywhere
in the wave — the stack was one linear commit chain, so every wave
merge was fast-forward-shaped and each merge commit's tree was
EXACTLY the PR-head tree its own green run had already validated
(the four push runs `45675162950` / `45675252175` / `45675323330` /
`45675390139`, all green on all five jobs; PR #48's `45675511392`
green on `2fd72b9`, whose tree is exactly `9c8bcd1`'s — the tree
run `45658412570` validated). The prior settlement, superseded: the
composed-crowd tree
(`era2/npc-ai-composed-crowd`, 2026-11-25, stacked on the part-7 tip —
the composed-crowd camera crossing, part 4's documented deferral
closed): main **1313/2/33 UNCHANGED (engine-core untouched) and heavy
620/33/63 both Linux MEASURED with the workflow's own awk on this
exact tree** (heavy 618 + the two platform-uniform temporal camera
gates: corner_routes' `full_crossings_clear_the_camera_on_the_shared_clock`
— the plain crowd's 1280-step window against the camera's same-frame
position under the demo's own warmup-lead pairing (crowd step k with
camera pose clamp(k−60, 0, 600); the measured minimum 1.8529123 at
step 178, both crossers' line-crossing steps pinned: 259/583/907/1231
and 268/604/939/1275) — and corner_behavior's
`composed_crossings_clear_the_camera_on_the_shared_clock` — the
composed campaign's 663 ticks against the same pairing, measuring the
plain gate's own minimum as its cross-check); Windows 619 = 617 + the
same 2 (the platform-uniform inference — pure spine/IEEE arithmetic
plus the route_poses f32 lerp, no cfg gates, no Vulkan) —
CONFIRMED by this candidate's own CI run 45637296212 (green on all
five jobs, first push — the run's own exact-count gates enforced the
measured counts on both platforms: `heavy shard: 620 passed, 33
ignored, 63` on ubuntu-latest and `heavy shard: 619 passed, 33
ignored, 63` on windows-latest, `main shard: 1313 passed, 2 ignored,
33` on both); renderer release 160/2 UNCHANGED (engine-renderer
untouched) (main 1295 + the
npc_memory battery's 18 platform-uniform engine-core lib tests;
heavy 608 + the memory-lane composition's 10 platform-uniform — the
npc_memory_lane lib battery's three and the new
tests/npc_memory_lane.rs binary's seven, its result line 62 → 63;
ignored 33 UNCHANGED — the S6 fuzz gate is a NORMAL test, 310 ms
measured on the pod, no #[ignore]); Windows 1262/617 = the same
platform-uniform deltas, **CONFIRMED by this candidate's own CI run
45625263643** (green on all five jobs, first push — the run's own
exact-count gates enforced the measured counts on both platforms:
`main shard: 1313 passed, 2 ignored, 33` and `heavy shard: 618
passed, 33 ignored, 63` on ubuntu-latest, `main shard: 1262 passed,
2 ignored, 33` and `heavy shard: 617 passed, 33 ignored, 63` on
windows-latest, the S6 fuzz gate `s6_fuzz_memory_lane_i1_through_i3_exactly_once
... ok` as an ordinary test on the runner); renderer release 160/2
UNCHANGED (engine-renderer untouched by the package). The prior settlement,
superseded: the shadowed-HDR populated-variant tree
(`era2/shadowed-hdr-populated`, 2026-11-25, stacked on the part-6 tip
— the composition gate): heavy **608 Linux MEASURED with the
workflow's own awk on this exact tree** (608 passed / 33 ignored / 62
result lines, zero failures — the part-6 settlement's counts with the
ignored count 32 → 33 by the ONE new #[ignore]d render_post gate; the
passed count and result lines UNCHANGED — the gate rides the existing
binary); Windows 607 passed UNCHANGED with ignored 33 (the
platform-uniform inference — the gate is lavapipe-class, executed by
the Linux Post-gates step, ignored identically on both platforms; to
be confirmed by this candidate's CI run, the count corrected from
actuals on a mismatch, never a test weakened); main **1295/2/33
MEASURED unchanged** (engine-renderer gains no lib tests); renderer
release 160/2 UNCHANGED. The prior settlement, superseded: the
Era-2 package A part-6 tree
(`era2/npc-ai-measurement-surface`, 2026-11-25, stacked on part 5 — the
measurement surface): heavy **608 Linux MEASURED with the workflow's own
awk on that tree** (608 passed / 32 ignored / 62 result lines, zero
failures — the part-5 settlement's 603 plus the behavior_measurement
battery's 5 platform-uniform engine-slice lib tests); Windows **607** =
602 + the same 5, **CONFIRMED by that candidate's own CI run
45587893215** (green on all five jobs — both heavy-shard exact-count
gates enforced the measured counts); ignored 32 and result lines 62 (lib
tests only, no new binaries — the benchmark flag is test-free harness
code); main **1295/2/33 MEASURED unchanged** (engine-core untouched by
part 6); renderer release 160/2 UNCHANGED. The prior settlements,
superseded in order: the part-5 tree
(`era2/npc-ai-composed-gates`, 2026-11-25, stacked on part 4 — the
composed gates): heavy 603 Linux MEASURED / 602 Windows (the +4
corner_behavior battery, ignored 31 → 32 by the new #[ignore]d composed
lavapipe gate, CI run 45579355303 green on all five jobs at the tip
`d5b2736`); the part-4 tree
(`era2/npc-ai-rendering-bridge`, 2026-11-25 — the rendering bridge):
heavy 599 Linux MEASURED / 598 Windows (the +8 corner_routes battery),
main 1295/2/33 unchanged, ignored 31 / result lines 62 (CI run
45572074231 green on all five jobs, both shard counts measured by the
run's own logs); the part-3 tree (`era2/npc-ai-locomotion-batch`,
2026-11-25 — the locomotion batch): the WORKFLOW's pins moved to main
1295/1244 with heavy 591/590 unchanged (engine-slice gained only the
test-free benchmark route), Linux 1295/2/33 MEASURED with the
workflow's own awk on that tree — but the §4 TABLE's part-2 row (main
1277/1226) was left stale by that package and was corrected at the
part-4 settlement from the measured actuals; the part-2 tree
(`era2/npc-ai-spine-integration`, 2026-11-25, stacked on the #38
reconciliation — `d9a034d`'s code plus this package): main **1277
Linux MEASURED with the workflow's own awk on this exact tree**
(1277 passed / 2 ignored / 33 result lines, zero failures — the
part-1 baseline 1263 plus npc_execution's 14 platform-uniform
engine-core lib tests); Windows **1226** = 1212 + the same 14 (the
platform-uniform inference — pure spine/IEEE arithmetic, no cfg
gates, no Vulkan; to be confirmed by that candidate's CI run, the
count corrected from actuals on a mismatch, never a test weakened);
renderer release 160/2 and heavy 591 Linux / 590 Windows with 31
ignored / 62 result lines UNCHANGED (the package touches engine-core
only), `d9a034d`
(2026-11-25, PR #37's merge — the Era-2 package A part-1 landing,
the owner's GO at 02:02Z; the merge-commit run `45530057475` green
on all five jobs enforcing main 1263/1212, its own push history the
pin discipline on record: the first candidate `d47620e` failed the
main-shard exact-count gate — run `45528096205`: expected 1237, the
tree measured 1263 — and the correction commit `d104094` re-pinned
from the measured actual on both platforms, run `45529070104`
green), `e4842d6` (2026-11-24, the Era-2
mission's merged endpoint — PR #33's merge commit `5d991dd`, where
the green five-job run `45512576593` enforced main 1237/1186 with
renderer release 160/2 and heavy 591/590, followed by the three
docs-only merges #34/#35/#36 under the workflow's path filter), the
Era-1 ladder's endpoint at `406aceb` (PR #28's reconciliation: main
1188/1137, renderer release 111/2, heavy 582/581 with 17 ignored /
62 result lines), then the eight-PR corrective union's landing
through PR #29 (main 1208/2/33, renderer release 131/2, heavy
588/28/62 — the union rehearsal's measured counts), on which #33's
tree built.
The per-package growth history that produced those endpoint counts
(the 6-E scaffold through the Era-1 ladder, each measured at its own
integrated tree): the Cycle 6-E implementation candidate's Linux
counts were MEASURED
locally with the workflow's own awk (1142 main = the 6-E scaffold's
1138 + 4 platform-uniform engine-renderer tests; heavy passed counts
unchanged, its new `render_instanced` binary contributing +1 ignored
/+1 result line); Windows = the prior 1087/569 + the same platform-
uniform deltas (inference, confirmed by the candidate's CI run — a
mismatch is corrected from actuals, never by weakening a test). The
Cycle 6-E ladder's population package (2026-11-23, riding the 6-E
branch) grew the heavy shard's passed count 570 -> 576 (the
`corner_population` module's six-test placement battery, within the
existing lib result line) and its ignored count 10 -> 11 (the second
`render_instanced` gate — the full 24-NPC + 2-vehicle table's N-vs-1 —
in the existing binary, serialized on the sdl_lock pattern); Linux
576/11/58 MEASURED with the workflow's own awk, Windows 575 = the
prior 569 + the same 6 (platform-uniform inference: the battery's
pins are margin-based comparisons and the byte pin is pure-IEEE
packing — to be confirmed by the candidate's CI run). Main is
untouched (engine-renderer untouched). The
Cycle 6-E2 indirect-draws package (2026-11-23, riding the population
branch) grew the main shard 1142 -> 1146 and the renderer release
battery 65 -> 69 (the `proxy_indirect` command core's four
platform-uniform tests: the packing determinism/wire layout, the
command-bytes FNV cross-platform pin, the empty-kind zero counts,
the slot-region math — pure integer packing, no cfg gates) and the
heavy shard's ignored count 11 -> 12 (the third `render_instanced`
gate — the indirect submission's three-way byte-equality, the
one-time command-buffer allocation pin, an oracle wall through the
indirect submission — in the same binary); Linux 1146/69 and
576/12/58 MEASURED locally with the workflow's own awk, Windows
1095/69 and 575/12/58 = the same platform-uniform deltas (to be
confirmed by the candidate's CI run). The Era-1 population-v2
package (2026-11-23): the v2 silhouettes (`populated_meshes` — the
eleven-box humanoid at 396 vertices, the sedan with cylinder wheels
at 552) become the population path's shipped meshes; the
`proxy_indirect` command pins and both packed-bytes FNV pins
re-measured for the new counts and the three stroller placements
nudged for the true 0.36 m footprint (every count UNCHANGED — the
repins are values inside existing tests, no new test anywhere).
  The Era-1 post package (2026-11-23): the main shard grew 1179 ->
  1187 and the renderer release battery 102 -> 110 (post.rs's chain
  reference - eight platform-uniform tests: the exactly-zero bright
  pass below one, the floor-division mip extent, the bilinear-
  footprint downsample box, the per-channel knee feed, the clamped
  blur taps and uniform-field preservation, the bilinear upsample's
  exact weights and border clamp, the composite equation, and the
  full-chain hand-computed 8x8 scene); the heavy shard's ignored
  count 15 -> 16 and result lines 60 -> 61 (the new
  tests/render_post.rs binary's ONE #[ignore]d gate - the
  known-answer pixels vs post.rs's chain at 0/255 MEASURED on
  lavapipe, the bloom-ring lift, the postless byte-identity, the
  one-time allocation delta, the composition smokes). The Era-1
  gates' FIRST-RUN CORRECTIONS (2026-11-23, the post package's
  session): every Era-1 lavapipe gate (soft shadows, sky,
  population-v2's re-pins) had never executed anywhere - their CI
  steps live in the parked workflow commits and the prior sessions
  measured shard counts only. First execution on the re-provisioned
  pod exposed and fixed: the sky pass's affine-interpolated
  per-vertex rays (the disc never rendered; now reconstructed
  per-fragment, 0/255 against the CPU rays), the sky gate's
  occluded sun/moon poses (the street-level origin aims the sun
  INTO the southwest building's silhouette; the poses moved above
  the masses), atmosphere_at's unnormalized night disc direction
  (0.9055 long, violating the documented unit contract - the moon
  never met the disc threshold), the sky gate's capture-buffer
  allocation pin (+1 memory, the first capture's lazy destination),
  the two render_shadows tests' parallel SDL init race (the
  sdl_lock serialization, render_instanced's pattern),
  population-v2's MISSED re-pins in the integration battery (the
  indirect oracle wall still carried the v1 rectangle 0.24 x
  [0.85, 1.45] - its frame includes the v2 pelvis/torso dead zone;
  the command-word defense-in-depth copy still carried the v1
  108/72 mesh counts), and post.wgsl's binding-1 mip aliasing the
  scene target through the build's atlas-share binding map (moved
  to binding 4, the 6-F shadow map's convention). Every Era-1
  graphical gate now executes and passes on the pod's lavapipe.
  The Era-1 wet-ground reflection package (2026-11-23, the
  ladder's last rung): the planar pass — the corner mesh's
  building masses re-rendered MIRRORED (planar.rs's
  ground_reflection_matrix composed into the view-projection,
  the winding flipped to CLOCKWISE, the ground itself never
  drawn in the mirror — it would occlude its own reflection)
  into per-frame-slot RGBA16F targets riding the post state's
  HDR render pass object (the wet mode carries the post state,
  never necessarily its chain) — and the ground re-draw:
  the ground quad re-rasterized inside whichever scene pass
  runs (EQUAL depth, no write) sampling the reflection at the
  same screen position (the planar property: the mirror fixes
  the ground plane, so the texel IS the reflected scene behind
  the pixel), Schlick-blended on the alpha stage (planar.rs's
  reflection_blend, r0 = 0.02). --corner --wet, composes with
  every corner flag; the mirrored scene is the buildings (the
  sky and the proxies are the documented follow-ups). The gates
  (tests/render_reflection.rs): the known-answer chain — the
  pixel's ray to its ground hit, the reflected ray raycast into
  the boxes, the lit equation at the hit (the mirror
  invariance: the mirrored render's shading at the mirrored
  point IS the real equation at the real point from the real
  eye), the Schlick blend over the dry pixel — MEASURED 1/255
  at grazing (60 pixels, blend factors 0.43-0.46) and overhead
  (3249 pixels, wet ~ dry at ~r0); the dry path byte-identical;
  the one-time allocation delta pinned with its decomposition;
  the composition smokes (wet+post, wet+post+sky,
  wet+shadows, wet+populated) all clean. The heavy shard's
  ignored count 16 -> 17 and result lines 61 -> 62 (the new
  binary's one #[ignore]d gate); the main shard and renderer
  release +1 each (planar.rs's mirrored-view-projection
  contract test). The package's own first-run catches (all
  fixed, the evidence in the commit): the color-write-mask
  default is ZERO (a legal, validation-silent no-op pipeline —
  the re-draw recorded, rasterized, and wrote nothing), the
  ground re-draw initially recorded OUTSIDE the HDR scene pass
  (a lavapipe segfault), and the mirrored sets' per-material
  indexing was off by one (the buildings rendered with the
  ground's asphalt texture).
Cycle 6-F candidate's Linux counts were MEASURED locally with the
workflow's own awk (1146 main = the pre-6-F 1138 + 8 platform-uniform
engine-renderer tests — the shadow core's full battery; heavy passed
counts unchanged, its new `render_shadows` binary contributing +1
ignored / +1 result line); Windows = the prior 1087/569 + the same
platform-uniform deltas (inference, to be confirmed by the candidate's
CI run — a mismatch is corrected from actuals, never by weakening a
test). The COMBINED tree (the full 6-E stack — 6-E + population +
6-E2 — merged with 6-F, 2026-11-23) carries all three additions:
main 1138 + 4 + 4 + 8 = 1154, heavy 576 passed with the three
`render_instanced` gates + the `render_shadows` gate (9 -> 13
ignored) and both new binaries (57 -> 59 result lines); Linux
MEASURED with the workflow's own awk on the merged tree, Windows
1103/575 = the platform-uniform sums (inference, to be confirmed by
the candidate's CI run). The
pre-6-E-implementation workflow pins (1138/1087 main, 570/569 heavy
with 9 ignored / 57 lines) carried the 6-A/6-B/6-E-scaffold history;
the workflow's comment chains remain the per-batch measurement record.
The Era-1 free-look package (2026-11-23) grew the heavy shard's passed
count 576 -> 582 (the `corner_demo` camera battery's six
platform-uniform tests — the route-entry matrix identity, the
world-AABB/eye-band clamp, the Stage-2-speed basis movement, the
runaway-input clamp, the shared look-mapping composition, the pitch
pin — within the existing lib result line; ignored and result lines
unchanged); Linux 582/13/59 MEASURED locally with the workflow's own
awk, Windows 581 = the prior 575 + the same 6 (platform-uniform
inference — pure f32 arithmetic and struct equality, no cfg gates; to
be confirmed by the candidate's CI run). The main-shard table row is
settled above at `main` @ `406aceb` — the scaffold-era flag (the row
carrying pre-scaffold 1154/1103 against the workflow's then-pin
1173/1122, deferred to "the scaffold's own obligation" rather than
folded into that package's diff) is discharged by this file's
2026-11-24 reconciliation; the flag's history lives in git.

**The Era-1 corrective cycle (2026-11-23/24, the overnight autonomous
run; the spec is Astra's 20-finding review,
`docs/ERA1-RENDERING-REVIEW-286d3e3.md`, merged as #20 — findings 1,
10 and 11 pre-verified by the home team):** every one of the 20
findings' corrections AND verification tests is landed across seven
five-job CI-green PRs, every one since MERGED under the owner's GO
(the only merge authority — the union landing PR #29 carried the
sequence; the merge-order arithmetic lived in the workflow's comment
chains and the PR bodies):

- **#21 `corrective/capture-trust`** — findings 1+2: the
  device-to-host visibility barrier and producing-submission/buffer-
  generation binding on both capture paths; the third
  `render_shadows` gate
  (`shadow_readback_belongs_to_the_shadowed_frame_and_its_buffer`)
  exercising interposition and replacement.
  Heavy ignored 17→18 at its tip; no other count moves.
- **#22 `corrective/frame-preparation`** — findings 3–9: pre-acquire
  target preparation, the negotiated depth format (post fallbacks),
  the current camera before either lighting branch, the generation
  cache key, immediate handle registration, the fault-injection seam
  with its count census, and explicitly-enabled synchronization
  validation with a negative control. At its tip: main 1195 Linux /
  1144 Windows, renderer release 118/2, heavy 582/21/62.
- **#23 `corrective/allocations-geometry-gates`** — findings 10–19:
  the host-allocation counter beside the Vulkan one (warmed direct,
  indirect and shadowed frames all pinned at zero), the owned wheel
  winding with the oblique-view gate, the full-vertex FNV digest pins,
  the lit-transform oracle, the shadow-map ground/NaN hardening, the
  reachable-anchor PCF boundaries, the captured-lift bloom
  measurement, the shader guard mirrors, the virtual-eye reflection
  oracle, and `@invariant` position outputs SPIR-V-verified with wet
  coverage bands. At its tip: main 1197/2/33 Linux (1146 Windows),
  renderer release 120/2, heavy 588/17/62 Linux (587 Windows).
- **#24 `corrective/architecture-split`** — finding 20's phased
  `frame_prepare` extraction and corner-scene recorder split (phases
  A/B/C plus the coordinator flag contract at the phase-A seam): a
  pure refactor, no test count moves.
- **#25 `corrective/sky-offaxis-coverage`** — the gate-table's sky
  extension: the off-axis disc edge at 264×200 (aspect 1.32) inside
  the existing sky gate; no count moves.
- **#26 `corrective/wet-depth-coverage`** — finding 19's
  fallback-format half (D24_S8) on the #22+#23 union tree. At its
  tip: main 1208/2/33, renderer release 131/2, heavy 588/24/62.
- **#27 `corrective/post-edge-coverage`** — the cycle-closing audit's
  three unlanded check-halves: findings 16+17's GPU-side
  discriminators (the asymmetric-disc mirror discriminator, the
  1×256/2×256 tiny-extent ambient feed) and finding 4's wet D16
  cell. At its tip (the largest stacked tree): main 1208 Linux /
  1157 Windows (2/33), renderer release 131/2, heavy 588 Linux /
  587 Windows (26 ignored / 62 result lines) — MEASURED with the
  workflow's own awk, Windows confirmed by the exact-count CI gate.
- **#30 `corrective/acquire-out-of-date-coverage`** (the recursive
  phase's next item, 2026-11-24) — finding 6's out-of-date half: the
  acquire-out-of-date recreation path's first-frame verification,
  the one review-proposed check #22 had honestly documented as
  driver-forced and not exercisable from the test API. A one-shot
  diagnostic injection (the `inject_target_fault` seam precedent)
  replaces the first `vkAcquireNextImageKHR` with the injected
  `ERROR_OUT_OF_DATE_KHR` — the driver contract modeled exactly (no
  image, semaphore untouched), the arm's retry acquire structurally
  real — and the new `#[ignore]d` render_reflection gate
  (`first_frame_after_out_of_date_recreation_keeps_every_layer`)
  proves the FIRST presented frame after that recreation carries
  sky/wet/post by the pending-recreate sibling's own differential
  method, plus the one-shot consumption proof. Stacks on #27's tip;
  at its tree: main 1208/2/33 and renderer release 131/2 unchanged,
  heavy 588/27/62 (ignored 26 → 27, the one new gate) — MEASURED
  with the workflow's own awk, the reflection battery 6/6 on the
  pod's lavapipe. At the full union the heavy ignored pin is 28
  (#21's capture gate + 27 here) — MEASURED on the assembled
  eight-PR union by the extended rehearsal (the paragraph below),
  the exact-count gate enforcing it through CI on PR #29.

The full union's counts are MEASURED (the 2026-11-24 union
  rehearsal, branch `corrective/union-rehearsal` — EIGHT `--no-ff`
  merges in the ladder order over `main` @ `406aceb`: #21–#27,
  then #30's acquire-out-of-date gate as the eighth, the end state
  the owner's merge sequence produces — the full local gate battery
  with the workflow's own awk): main **1208/2/33** (Windows 1157),
  renderer release **131/2**, heavy **588/28/62** (Windows 587 —
  the seven-PR union's measured 27 plus #30's one gate, the
  arithmetic #30's body documented, MEASURED on the assembled
  tree). The eighth merge is the one the seven-PR rehearsal could
  not cover (#30 stacks on #27's tip, so its one-shot injection
  consume site — vulkan.rs's first acquire — had never met #24's
  `prepare_after_recreation` restructure of that exact region):
  its ONE conflict was the workflow's heavy-ignored pin block,
  resolved additively, and the linchpin gate re-fired on the merged
  tree with every measured line identical (the injection survived
  the split — re-verified on the pod's lavapipe AND in CI's own
  Reflection-gates step); draft PR #29 (the
  rehearsal's DO-NOT-MERGE verification PR — GitHub's
  `pull_request` CI is the only mechanism that runs the exact-count
  gates on the union tree, the push trigger being branch-filtered)
  carries it through the five-job CI — run `45392386762` green on
  the seven-PR tree, run `45406550126` green on the eight-PR tree
  (the Windows 1157/587/28 exact-count pins' first execution on
  the end state). The rehearsal also caught the union's two
  merge-time obligations — both at the #21×#23 seam, correct on
  every branch's own tree and invisible to GitHub's conflict
  detection: `compare_map`'s 4→7-tuple (four destructure sites in
  #21's capture gate) and `intersection_boxes()`'s Vec→`&'static
  [SceneBox]` slice change (the same four sites' `&boxes` becomes
  a needless borrow under clippy `-D warnings`) — plus #22's
  accidentally-committed `vulkan.rs.backup` (a stale 5083-line
  copy of `vulkan.rs`, removed on that branch in `5a4cbd2` with a
  diff-identity proof of gate inertness). The full record lives in
  PR #28's comment trail; the fixes are carried verbatim on the
  rehearsal branch for the owner's merge sequence. Every rig-dependent clause (the discrete-GPU
capture-visibility behavior, driver-forced fallback paths) is
documented as rig-pending in the PR bodies, awaiting the owner's
RTX 5070 Ti procedure — never presented as measured. The cycle's own
closing re-audit (2026-11-24, independently re-verified against
source) found the recursive queue empty: no unlanded correction, no
unlanded verification test, no NOTE/IMPROVEMENT finding uncovered.
The NEXT cycle's re-check (2026-11-24, the supervisor cycling the
agent as the protocol directs) re-audited that emptiness claim
against the source with a second independent read-only pass and
found exactly one remainder: finding 6's out-of-date half, the
check #22 had documented as not exercisable — landed as #30 above
(the cycle's own 5/3 seam precedent applied to its last documented
limitation). The queue is empty again: every remaining clause is
either landed with a CI-visible gate or documented as rig-pending
in the PR bodies.

**The Era-2 package 1 record (2026-11-24, branch
`era2/animation-skinning-proxy-shadows` — the animation + skinning +
proxy-shadows ladder, all five stages on one branch; MERGED as PR #33
at 2026-11-24T23:45Z (the owner's GO), the merge-commit run
`45512576593` green on all five jobs at `5d991dd` with these counts
enforced, the branch deleted on merge; every count below MEASURED on
the branch's stage-5 tree with the workflow's own awk, cross-checked
against CI runs 45471375791 / 45472428113 / 45478600773 on the staged
tips):** the
count arithmetic, stage by stage — stage 1 (the deterministic
animation core, `b675377` + the `6b95875` verification repair) adds
22 platform-uniform engine-renderer lib tests (main AND renderer
release); stages 2a+2b (the pose-index claim + the five WGSL
modules, `862e003`) add NO tests (the packed-bytes pins are value
repins inside existing tests); stages 2c+4a (the skinned pipeline
wiring + the populated raycaster oracle, `036d88a`) add 7
platform-uniform engine-renderer lib tests (main AND renderer
release); stage 3 (the animated population, `16ec1a5`) adds 3
engine-slice lib tests (the HEAVY shard's passed count, within the
existing lib result line); stage 4b (the shadowed populated path,
`ba58c96`) adds no tests (wiring); stage 5 (the gates + the pin
reconciliation) adds 3 `#[ignore]`d lavapipe gates in existing
binaries (the heavy shard's IGNORED count only). The reconciled
pins: main 1208+29 = **1237 Linux / 1186 Windows** (2 ignored / 33
result lines), renderer release 131+29 = **160/2**, heavy
588+3 = **591 Linux / 590 Windows** with **31 ignored / 62 result
lines** — the union's 28 ignored plus stage 5's three gates
(render_instanced 4→6: the skinning oracle and the animated N-vs-1;
render_shadows 3→4: the populated shadow-map gate; the full-table
gate's animated allocation regime extends an existing test, no
count). The stage-5 gates' first device execution is the push's own
CI run (the dev pod carries no Vulkan loader): the skinning
oracle's per-pixel expectations, the populated map's 2.68e-7
tolerance, and the differentials are all CPU-derived in the gates'
comments, with the per-class census prints as the first run's
measured evidence (the CPU-side expectation census at rest, the
comparator's own post-skip bucketing: clear 511,688 / ground
28,758 / building 495,081 / npc 1 / vehicle 442 compared, 12,606
edge-skipped at the 1.5-texel margin; at frame-33 walk: npc 1,
and 73 texels differ between the two expectation maps — the
recorded density lesson: a 0.42 m figure spans ≈4 texels of the
0.110 m map, so the npc class is a presence proof with the
73-texel differential carrying the casts' coverage). **The first
device execution's corrective cycle (2026-11-24/25 overnight — three
red runs, one repair in three commits, the full record in PR #33's
comment trail):** run 45485839516 caught
`VUID-vkCmdBindDescriptorSets-pDescriptorSets-06715` (the pose
buffer's binding-5 descriptor written `range(VK_WHOLE_SIZE)` —
illegal with any nonzero dynamic offset; slot 0's zero offset is
legal, which is how it survived every CPU-side gate); run
45488497488 then executed the indirect gate's replacement assertions
for the first time on an Era-2 tree and measured the skinned
family's per-replacement re-ensure churn (+1 pool / +3 sets per
forced replacement — the counters are cumulative creation totals;
the 6-E2-era pins held them flat because no populated-path family
then owned descriptor-class resources); run 45491858460 reproduced
those numbers digit-for-digit; run 45493567733 (the post-repair
first-green attempt) passed the instanced gates and executed the
shadowed-populated gates for real — the census exactly as pinned
(1,035,970 compared / 12,606 edge-skipped, npc 1 / vehicle 442),
with 31 of 1,035,970 building texels at the float-epsilon tail just
past the placeholder tolerance (the placeholder-pin discipline's
anticipated correction: `POPULATED_MAP_TOLERANCE` 2.68e-7 → 3.0e-7
from that run's measured per-class census — the instance classes an
order under at npc 5.96e-8 / vehicle 1.79e-7). The repair commits:
the WHOLE_SIZE ranges (all three binding-5 writes → the concrete
`SLOT_STRIDE`), the never-written shadowed sets (both families, per
material × slot), the skinned depth set rebuilt to its shader's own
declarations, the vehicle shadowed pipeline's wrong shader module,
the four shadowed-family builders ignoring their `layout` parameter
(now structural: `pipeline_from_module` takes the layout and a
`ProxyVertexForm` as explicit parameters), the plain vehicle
pipelines' skinned 64-byte vertex input against the 32-byte sedan
VBO, and the replacement gates' exact per-replacement cost pin
(measured twice digit-for-digit; the pass-independent ownership
transfer that would restore the flat tuple is the recorded
renderer-architecture follow-up). Every pinned count unchanged
through the whole cycle. One pre-existing macOS-only failure surfaced in
the same runs (CI run 45478600773:
`s6_soak::deterministic_late_window_hard_exit_control_recovers` —
the in-flight fsync commits before the hard exit on the M1 runner,
so the relaunch recovers at tick 3300 where the test pins 3054;
the soak machinery is untouched by this branch; the §3.1 rerun
PASSED — the documented fsync race, closed as a one-time flake) —
reported to the supervisor, outside this package's scope.
The package's post-merge second-device evidence (2026-11-24, the
23:50 UTC queue-empty session — the full record in PR #33's comment
trail): the pod's re-provisioned lavapipe (Mesa 25.2.8 / LLVM 20.1.2
— no pod Vulkan loader existed during the package's own development)
executed both new gate batteries green, rc=0 first try
(`render_instanced` 6/6 — the skinning oracle max RGB error 1/255
with its −10.9/255 self-discrimination control, the animated N-vs-1
byte-equal with 1,447 pixels moved by the walk, 8 distinct pose
digests across the 8 measured frames with host allocations 0;
`render_shadows` 4/4 — the populated map's census reproducing CI run
`45493567733` digit-for-digit, max |Δdepth| 2.68e-7 under the
corrected 3.00e-7 tolerance, the static 6-F value reproducing
digit-for-digit), and the full `--corner --populated --shadows`
route ran its FIRST EXECUTION ANYWHERE — 601 frames, validation
warnings/errors 0/0. Raw logs on the pod:
`~/push-handoff/era2-second-device/`.

(At `a4eb372`, Windows counts were measured before landing; Linux
counts were inferred at landing and subsequently confirmed by GitHub
run `44937616390`.)

(The extraction baseline was measured at `main` @
`b19d210` by five-job CI run `44827915274` (and identical at the Sektor7
pre-extraction mainline `2640b64`, run `44825693818`): the Cycle-3-era
values this table carried before the 2026-11-20 reconciliation (1011/960 +
498/497 at Cycle 3, then 1050/999 + 512/513) were superseded by Cycle 4
(+46 main platform-uniform across its batches), Cycle 5 (+2 main, +12 heavy
+1 ignored), and perf-hardening batch 1 (+2 main, +1 result line — the
spine allocation gate). Every step was measured at its integrated tree;
the history is preserved in the workflow's comment chains and the cycle
commit messages.)

**The Cycle-1 candidate** (was `s7-cycle1-integration`;
**merged to `main` 2026-11-15**, superseded by the Cycle-2 merge below)
added +32 main / +16 heavy platform-uniform tests and three integration
binaries, including the full corrective round (R4/R4b player boundary
and exact-accounting regressions, R2's gradual-growth regression).
Measured at the R4b-merged tree (the workflow's own awk semantics):
main **987 / 1 ignored / 30 result lines**; heavy **483 / 4 ignored / 49
result lines**. Windows expectations 936 / 482 (platform-uniform
inference) are confirmed by the candidate CI run; the workflow pins are
updated in the same change (the F2 maintenance obligation). All
determinism pins below are byte-identical at the candidate, plus the new
pinned player trace `engine-core/player-trace/1` class
(`8513d40792ea88f0`, the libm-free script class — cross-platform by
construction; divergence is an investigation, never a silent re-pin).
The formerly ignored oversize smoke is now ALSO executed by an enforced
Linux CI step (the R1 corrective) with its evidence lines in the log.

**Cycle 2** (built on the Cycle-1 candidate; **merged to `main`
2026-11-16**, superseded by the orientation merge below) adds C2-P1's
+19 main (11 player mechanics: step-up, the exact-slab segment query,
camera obstruction; 6 interaction-framework tests; the
corrective/closeout rounds' progression and interaction-validity
regressions) and +11 heavy (`playable_world`, a new integration
binary, including the corrective rounds and the connected live-frame
route) plus C2-P2's +2 heavy envelope regressions, with a second frozen
scenario trace (`8f6b490648326d26`, the libm-free gatehouse course).
Both Cycle-1 frozen traces and every determinism pin remained
byte-identical through the merge.

**The orientation corrective** (2026-11-17, PR #4 — `main` =
`15b8109e0cca4760c1128b2bc61bf59b3f4fdce5`, post-merge CI
`44014349602` success ×5) fixed the owner-observed upside-down world:
the single Vulkan Y-axis conversion (the naga shader-boundary flip)
with `perspective()` un-negated, both pipelines `front_face
CLOCKWISE` (the winding audit), and the bounded frame-capture readback
with pixel-level orientation regressions (`render_orientation`, an
enforced Linux CI step). It also made the overlay actually render for
the first time (the glyph quads had been back-facing/culled under the
old shared COUNTER_CLOCKWISE state since Stage 2 — measured pixel
evidence; no earlier test ever asserted overlay pixels).

**The post-orientation hardening candidate** (2026-11-17,
`s7-post-orientation-hardening`, awaiting review) fixes the
owner-observed inverted horizontal mouse look (mouse-X negated at the
input mappings across all four camera paths — the playable, player,
and the shared free-flight render/persist cameras; the vertical
mapping byte-identical) and hardens the capture API: an explicit
4-byte RGBA8/BGRA8 capture-format contract with typed refusals before
any GPU copy (an 8-byte fallback surface format can no longer
under-allocate the destination), checked sizing, the fallible capture
preparation moved BEFORE the frame-fence reset and image acquire (a
refused capture can never poison frame synchronization — the frame
renders normally and the refusal is observable), and the bounded
single-flight request lifecycle (a second request while one is
outstanding is refused with a typed error — never overwriting an
unconsumed readback). The final cache-validation corrective on the
same candidate adds the cache-reuse full-extent validation (a
1280×720 destination never serves a 960×960 swapchain, same pixel
count notwithstanding) and the transfer-source-capability check
independent of destination reusability (at preparation AND at the
record-time guard). Adds +4 main (the H3 capture contract units and
the two cache-validation regressions) and +3 heavy (the H2
mouse-look direction regressions across the player, playable, and
free-flight paths — asserting the resulting world-space look
direction through the real mappings and engine math). Measured at
the candidate: main **1011 / 1 / 30**; heavy **498 / 5 / 51 result
lines** (Windows 960 / 497, platform-uniform). Every determinism pin
and both frozen traces byte-identical at the candidate.

- **Determinism pins (never change):** engine `6504123a47cab66f`, state
  `937bd3b6f56f07e0`, save `len=1209 / fnv=c6c1b46c7d21843b`, the Stage 3
  and Stage 4 scenario fingerprints — all double-run and byte-compared in CI.
- **Smokes:** fingerprint/save self-tests (double-run), headless simulation,
  the three presence scenarios, the Vulkan offscreen render smokes with the
  active validation layer incl. swapchain recreation and pipeline
  replacement (Linux/lavapipe — correctness only), the bounded integrated
  soak, and the F-06 release render-path canary (distributions printed, the
  A/B geometry equality asserted, no timing thresholds).
- **Counts describe the current CI configurations** — the specific commands
  in the workflow, not every platform or feature combination. The historical
  count narratives inside `docs/ENGINE-WORKSPACE-NARRATIVE.md` describe
  earlier states and are retained as history.

## 5. Accepted architecture decisions

The ledger is `docs/DECISIONS.md`; the load-bearing accepted decisions:

- Stage-gated minimal scope with per-mechanism tests (the anti-abstraction
  ledger, TECH-ARCHITECTURE §8).
- Deterministic-first simulation (the spine owns logical time; rendering and
  pacing never enter logical state).
- Content-addressed storage with structural domain separation; the
  single-authority commit model (canonical confirmation = verified chain
  inclusion AND per-key validation); no parallel canonical writes.
- The T1 journal and the two-contract recovery (pure-function
  reconciliation; strict temporal boundaries at every begin).
- The two production canonical mediums with their **distinct durability
  contracts** (the provider-selection gate closed through the PP line):
  SQLiteMedium **`TransactionalWalFull`** (WAL + `synchronous=FULL`, both
  verified by querying the actual configuration at open — PP-B1/D-PP-1,
  subject to its documented configuration/storage assumptions);
  LocalFileMedium **`FullPosixSequence`** on supported POSIX (the full
  fsync sequence; the macOS std-semantics note applies) and
  **`ProcessCrashOnly` on Windows** — the process-crash tier, insufficient
  for the ratified destructive-reclamation durability requirement (the
  OC-P2 tier gates refuse the destructive arms on that platform).
- SDL3 vendored-static; the engine never links system SDL.
- Production-persistence decisions: `docs/PRODUCTION-PERSISTENCE-DECISIONS.md`.

## 6. Owner-ratified decisions and deferred design items

The following three behavioral and measurement decisions were
**ratified by the owner on 2026-11-22** (the decision ledger:
`docs/DECISIONS.md`, entries T-16/T-17/T-18). They are accepted
behavior, not open questions — future changes require an explicit
tradeoff analysis and owner approval.

- **Jump input (ratified).** The first executed simulation tick
  consumes a queued press; an airborne player must press again
  after landing. Future gameplay polish may propose configurable,
  bounded jump buffering (with coyote-time) under the ratified
  requirements in the decision entry — none is implemented.
- **Render overflow (ratified).** At the shared scene-vertex cap,
  foreground draws (player-visible geometry, weapon/viewmodel
  presentation) take priority and world geometry truncates first.
  The permanent geometry-capacity policy (complete primitives,
  scalable overflow handling) remains renderer-development backlog
  work — the fixed current cap is a prototype constraint, not
  endorsed architecture.
- **Timing percentiles (ratified).** Live CPU/GPU diagnostics use
  the bounded histogram (bucket-upper-bound percentiles, exact
  maxima, documented ≤ ~1.6% relative error). Engineering
  benchmarks that justify optimization or architecture claims must
  use exact measurements or rigorous A/B methodology — approximate
  histogram percentiles are never presented as exact observations.

The following audit finding remains genuinely open:

- **Scheduler budget (audit F4).** The per-tick limit counts executed
  events. Registering blocked events and waking their dependents still
  costs CPU outside that limit. **CLOSED 2026-11-22 (Cycle 6-C,
  decision T-18): MEASURED NEGLIGIBLE — no bounded budget needed.**
  The per-tick bookkeeping cost is linear at ~1 ns per blocked event
  per tick (under 0.1% of a 60 Hz frame at 10,000 blocked events —
  10x the demonstrated sustained-load scale); each event parks exactly
  once regardless of waiting duration (pinned structurally by
  `tests/spine_bookkeeping.rs`). The finding reopens only if a
  10,000+ blocked-event-per-tick workload is demonstrated.

- **F-03** — the region-render vertex-capacity policy: **Step 1 is
  DELIVERED in Cycle 1** (the fail-visible truncation counters — §2 —
  measured zero-truncation on every ordinary run, and the oversize
  lavapipe smoke proves the exact counting through the real render path);
  The current renderer gives foreground and gameplay HUD vertices priority
  within the existing caps and allocates overlay storage on demand.
  **Step 2** (constants-derived capacity + typed refusal at the
  `render_frame` boundary) remains deferred awaiting the owner's separate
  decision — the technical recommendation with alternatives and tradeoffs
  is delivered with the Cycle-1 P1 package (`GPU-CAPACITY-SYNC-ANALYSIS.md`
  §4 Step 2 + the delivery report).
- **F-04** — GPU asset streaming / upload architecture: the measured-
  hypothesis plan is **DESIGN READY** (the same document §2.2/§3 — gated on
  the rig measurements H1/H4/H5; no refactor justified by current
  evidence).
- **F-08 / F-12** — the sustained-load scale methodology is
  **IMPLEMENTED in Cycle 1** (the `--benchmark-sustained-load`
  demonstration + `--sustained-load-full-matrix` full harness: the six
  workload classes, twelve measurement dimensions, the measured
  composition table, the honest effective matrix — see §7), **corrected
  in the R2/R3 round** (per-logical-tick slope units; recovery horizons
  derived from measured drains with explicit bounded-observation wording
  for non-draining compositions); remaining follow-ups are documented
  limitations (§8: the class-6 director-protocol anchor is
  harness-driven, and the sustained codec's identity-capacity
  tightening), not missing methodology.
- **The F-05 overlay throttle** — deliberately NOT implemented (the measured
  costs are negligible at target frame budgets; revisit only with rig
  evidence).
- Compaction follow-ups and the multiplayer **implementation** —
  `docs/MULTIPLAYER-SPEC.md`'s player-facing multiplayer and governance
  rules are **ratified product requirements**; the in-session
  architecture boundaries are **designed** (unratified) in
  `docs/CYCLE6-6D-SESSION-REPLICATION-BOUNDARIES.md` (Cycle 6-D,
  2026-11-23 — sim placement, input ordering, replication scope,
  divergence detection, session lifecycle, with the owner decision
  list in its §9); what remains open beyond it is the owner's §9
  rulings, the transport/provider selection (T-06 deferral), and all
  implementation (owner-gated batches, its §10).

## 7. Actual performance evidence (and what is NOT evidence)

- **CPU render path (pod, `--benchmark-render`):** the F-02 scratch form
  p95 0.081 → 0.035 ms/frame (−57%), byte-identical geometry; the F-05
  overlay breakdown (glyph vertex rebuild 0.090–0.131 ms dominant; format
  chain and percentiles negligible). Pod CPU evidence only.
- **Entity scale (pod, `--benchmark-entity-scale`):** 1k / 5k / 20k
  entities × three LOD scenarios, bounded event execution per tick with
  double-run determinism. Bounded-cost evidence, **not** sustained-load
  proof (F-12/F-08 open). **Read-through correction (Cycle 1, measured
  2026-11-14):** at the 5k and 20k points the benchmark sustains only
  **1,000 live routine chains** — `npc::routine_payload_parts` rejects
  identities ≥ `NPC_COUNT` (1,000), so entities above the first thousand
  execute their initial event and never chain (measured: ≈154–162
  executions/tick at ALL three populations; pending stays exactly 1,000).
  The historical at-scale numbers are one-1000-chain steady states, not
  scaled-chain evidence; the sustained-load harness below realizes full
  chains at every population with its own codec.
- **Sustained load (pod, Cycle-1; corrected per the R2/R3 corrective):**
  the F-08/F-12 reference matrix — **`--sustained-load-full-matrix`** runs
  it; **`--benchmark-sustained-load`** is the bounded demonstration
  command. The raw measurement record (every cell, double-run, with
  environment parameters and reason-coded N/A lines) is archived at the
  coordination ledger as
  `cycle1/evidence/P2-sustained-load-full-matrix-R2R3.output` — this
  summary qualifies it and replaces no raw result. **Overload onset is a
  function of population × LOD composition, not population alone**
  (measured base demand per logical tick against the 200/tick budget):
  at 1k, no scenario's base saturates (full 154.1, mixed 84.1,
  event-driven 13.7); at 5k, full and mixed saturate the budget while
  event-driven (69.2) stays sustainable; at 20k all three saturate. The
  overloaded cells measure the honest "fast-because-falling-behind"
  (per-logical-tick backlog slopes — the R2 units corrective — e.g.
  +57.7/tick at 5k-full, +199.7/tick at 1k-event-driven-overlaid; the
  gradual-growth regression pins +1/tick so slow unsustainability cannot
  masquerade as sustainable). **Recovery horizons are derived from
  measured drains** (the R3 corrective: the 10× rule over observed drain
  times — 1k drains 100–150 ticks → 1000–1500-tick horizons, one
  resource-capped at 1200, stated); at 5k/20k no complete drain occurs
  within the bounded probe, so those cells are explicit bounded
  observation windows with backlog/age trends reported and **no
  complete-recovery claim**. Mixed-LOD splits lane-vs-lane (near
  overloaded / far designed cadence). Compacted-representation variants
  show the ledger downgrade. Pod CPU evidence only, logical-time
  determinism throughout.
- **Player prototype (pod, Cycle-1 `--player-headless` / offscreen):**
  collide-what-you-see physics verified end-to-end (the scripted walk
  stops against the visible cube at the exact SKIN offset); the
  libm-free trace class is cross-platform bit-stable (pinned
  `8513d40792ea88f0`, reproduced at the integrated three-package tree).
  **No human-played run has occurred** — offscreen runs are correctness
  smokes only.
- **Persistence (pod, the F-07 harness):** ~1295 sequential ops/s, ~9%
  worker-queue overhead, contention p99 128 ms, cold recovery ~34.7 ms,
  ~19× head-of-line amplification — pod measurements of the approved
  topology, not universal guarantees; the actionable interpretation is
  delivered
  (`docs/F07-PERSISTENCE-OPTIMIZATION-ANALYSIS-2026-11-14.md`, §9).
- **Real GPU (the only real-GPU evidence):** the Stage 4 Windows acceptance
  record (2026-10-21, the owner's RTX 5070 Ti rig — e.g. 1744 fps avg
  uncapped/immediate on the Stage 4 scene). **Standing rule: no CI or pod
  number is GPU evidence** (llvipe/lavapipe are correctness smoke only).
  The current-scene hardware baseline procedure is
  `docs/WINDOWS-GPU-PROFILING-KIT.md` (F-09-corrected), awaiting the owner's
  rig.

## 8. Known limitations (the honest list)

- **LocalFileMedium on Windows is `ProcessCrashOnly`** — the documented std
  Windows sequence (file fsync + rename ordering) establishes
  acknowledged-revision persistence across process crash only, NOT OS
  crash or power loss, and it is insufficient for the ratified
  destructive-reclamation durability requirement (the OC-P2 tier gates
  refuse the destructive arms on that platform). Production Windows
  deployments use the SQLite medium (`TransactionalWalFull`).
- The renderer is prototype-class: static scene buffers, host-visible
  persistently-mapped dynamic buffers, 2 frames in flight, and a **bounded
  region-vertex capacity** (F-03 — the truncation is now COUNTED and
  reported, measured zero on every ordinary run, but the capacity limit
  itself stands; the Step-2 refusal policy is deferred by design).
- Player movement uses kinematic box collision. The playable gatehouse and
  courtyard encounter use a constrained third-person camera, and the
  encounter includes rotated cover. Controller state remains transient and
  movement runs beside the spine. The interactive demos provide rebindable
  controls and adjustable mouse sensitivity, and the encounter also provides
  user-controlled HUD scaling. See [Player controls](PLAYER-INPUT.md).
- The sustained-load harness's documented Cycle-1 limitations: class 6's
  RegionDirector-protocol anchor is harness-driven (the director's own
  transition semantics stay covered by the regions/s6 suites, not the
  matrix), and the sustained codec's 16-bit identity field contradicts
  its 2^24 restore bound (latent — no delivered path triggers it; a
  follow-up tightening).
- The entity-scale benchmark's 5k/20k dead-chain read-through (§7): the
  historical at-scale evidence is one-1000-chain steady state.
- **The s6_soak late-window fsync race (flake C, 2026-11-23, macOS run
  45104773498) — CLOSED 2026-11-25 (the
  `corrective/s6-soak-late-window-seam` package):**
  `deterministic_late_window_hard_exit_control_recovers`
  assumed the tick-3300 publication's WAL fsync cannot complete within
  the five sim ticks before the controlled hard exit ("the pure-sim
  ticks between always outrun the fsync") — but a 726 ms runner stall
  on the sim path let the fsync land first (the campaign recovered at
  tick 3300, not the asserted 3054). A genuine wall-clock race in the
  test's SETUP, not an engine defect (recovery, conservation, and
  close integrity all held). **The principled fix LANDED** (the
  shape §8 specified, one refinement: the seam sits at the demo's
  publication boundary, not inside engine-persist — the worker's
  EXISTING `ParkExecutingAppend` fault is the deterministic barrier,
  so the production persistence stack is untouched): the demo's
  hidden `--park-publication-at-tick` arms the I/O worker's append
  park AT the scripted publication's own scenario tick (before its
  append job is submitted), so the worker parks INSIDE the job's
  dispatch — the publication admitted and started (past the poison
  and cancellation boundaries), its store execution never begun,
  nothing of its transaction durable — and the paired hard exit at
  3305 kills the parked worker with the process. The in-flight state
  is now PROVABLE on every runner (the safety argument: the
  original test's own passing state held the worker busy inside
  that same append for the whole 3301-3305 window — those five
  ticks need no blocking worker dependency, so the parked case
  cannot deadlock). The test's child arms the seam at 3300; the
  3054 recovery and every close assertion unchanged; three
  consecutive runs green on this pod (69.6/69.5/69.7 s) plus the
  full s6_soak battery 34/34; no test-count movement anywhere (the
  field/flag is test-free harness code). The rerun-once protocol
  (AUTONOMOUS-CYCLE-PROTOCOL §3.1) remains for any OTHER runner
  flake, but this one is retired by construction.
- Sustained-load scale is QUALIFIED with composition-dependent
  interpretations (sustainable at 1k across scenarios; at 5k the full and
  mixed compositions genuinely overload the 200/tick budget while
  event-driven stays sustainable; at 20k all compositions overload —
  measured, per-tick units), but production-scale claims remain out; the
  reference matrix is pod CPU evidence with logical-time determinism,
  not a universal throughput guarantee, and the 5k/20k recovery cells
  are bounded observation windows, not complete drains.
- No GPU asset streaming (F-04); dynamic content moves through per-frame
  host-visible copies.
- No networking, replication, procedural generation, asset pipeline, or
  editor systems (§10 — not yet designed, not hidden). Combat, adversary,
  and encounter prototypes landed through Cycles 2–5 (§2, the capability
  matrix) — game-scale gameplay on top of them remains undesigned; the
  physics backend remains the reversible Cycle-1 prototype.
- The engine is a **development foundation + demo**, not a production game
  engine; nothing in the current evidence supports production-readiness
  claims for the complete game.

## 9. Active development priorities

The post-audit remediation cycle (2026-11-14) **is merged** — `main` @
`900b0ea` carries F-09 GPU-kit/CLI correctness, F-11, F-14, F-13, and the
Milestone-2 analyses (`docs/SUSTAINED-LOAD-METHODOLOGY.md`,
`docs/F07-PERSISTENCE-OPTIMIZATION-ANALYSIS-2026-11-14.md`,
`docs/GPU-CAPACITY-SYNC-ANALYSIS.md`,
`docs/F10-FAULT-SEAM-COVERAGE-MATRIX-2026-11-14.md`).

**Cycle 1 (2026-11-14/15, the owner's GO directive preserved verbatim with
SHA-256 at the coordination ledger, §10)** — three packages delivered,
independently cross-reviewed (all ACCEPT), consolidated on
`s7-cycle1-integration`: **P1** renderer overflow visibility (F-03 Step 1,
merged `359fbc6`), **P3** the player movement/collision prototype (merged
`7101433`), **P2** the sustained-load verification harness (F-08/F-12,
merged `0940a62`), the §8 reconciliation (`0eeba4c`), and the **ChatGPT
corrective round** (the HOLD review, hash-verified at the ledger): R1 the
enforced CI execution of the oversize regression (`47c7f8e`), R2/R3 the
per-logical-tick slope units + derived recovery horizons (merged
`2b620eb`), R4 the player-input boundary contracts (merged `ee23927`) —
each corrective independently verified by a non-author reviewer (all
ACCEPT; verdicts archived in `cycle1/reviews/`).

The **corrected Cycle-1 candidate** (full ancestry from `900b0ea`) measured
at its exact tree: fmt/clippy clean, main **984/1/30**, heavy **483/4/49**,
every determinism pin byte-identical, the P3 frozen trace reproducing, and
the cross-package combined-semantics check green (the player proxy through
the truncation counters — 0 omitted). The five-job `workflow_dispatch`
candidate CI (including the enforced R1 execution step) is dispatched at
this exact SHA; per-job conclusions are recorded in the cycle report.

**Cycle 2 (2026-11-15/16, the GO directive hash-verified at the ledger)** —
two implementation packages plus the F-03 Step-2 recommendation
(owner-gated, §6), consolidated on `s7-cycle2-integration` (built on the
Cycle-1 candidate):

- **C2-P1 — the playable-world interaction loop** (the gatehouse
  courtyard): step-up navigation over multi-box geometry, the exact
  ray–AABB `segment_hit` powering both TP-camera obstruction pull-in and
  line-of-sight interaction filtering, the minimal reusable interaction
  framework (engine-core `interaction.rs`; game policy in the demo),
  visible feedback through the existing overlay + dynamic scene, and the
  `--playable` / `--playable-headless` deterministic twin (the frozen
  scenario trace `8f6b490648326d26`; the full event log incl. the
  occluded→clear LOS moment and the TP pull-in sample). All scenario state
  **transient**; controls documented for the owner's hardware procedure
  (the C2-P3 draft in the cycle report).
- **C2-P2 — simulation scalability**: the measured **capacity envelope**
  (budget sweep 200–3200, release profile, real chains, double-run): 1k
  compositions sustainable from budget 200–400; **5k-full from 1600**,
  **5k-mixed from 800**, 5k-event-driven from 200; **20k-full NOT
  sustained within the sweep** (the honest limit at 3200), 20k-mixed from
  3200, 20k-event-driven from 800. Spine isolation **0.278–0.517
  µs/event release** (≈4× the debug profile); work rates 1,044–1,482
  events/ms — **the budget binds, not the CPU**. The envelope
  regressions pin sustainability at the measured budget (5k-mixed
  800/240) and near-lane starvation-freedom under far flood at the
  raised budget. The default-budget change is **owner-gated** (the
  measured recommendation delivered; nothing silently re-defaulted). No
  direct allocation/locality instrument was added — C4's drop rests on
  the isolation's cost-flatness; a bounded counting-allocator remains a
  future-cycle option (with the `unsafe_code=deny` interaction that
  would need surfacing). The near-lane per-event-cost lead (~2×) is a
  documented lead, not a conclusion (work/ms at sustaining budgets is
  similar across compositions: 5k-full@1600 ≈ 1,301 vs 5k-mixed@800 ≈
  1,280 events/ms).

Both packages independently cross-reviewed (ACCEPT; verdicts archived in
`cycle2/reviews/`), the ChatGPT review chain closed (**CYCLE 2
CLOSED-PASS**), and merged to `main` 2026-11-16; the orientation
corrective followed via **PR #4** (2026-11-17, `main` =
`15b8109e0cca4760c1128b2bc61bf59b3f4fdce5`, CI `44014349602` success
×5, the owner's physical right-side-up verification on the RTX 5070
Ti). The post-orientation hardening candidate (§4) is the active
review cycle. Standing owner decisions: the F-03 Step-2 policy
ratification and the simulation default-budget change.

**Cycle 3 (2026-11-17/18, the GO directive hash-verified at the ledger)**
— six parallel implementation seats + three independent reviewer seats
(the owner-authorized parallel expansion), all six packages
review-accepted and integrated on `s7-cycle3-integration` (the
candidate documented in §4's pins): **P1a** the reusable combat
mechanics (`engine-core/combat.rs`: the pure `resolve_attack` with
exact-slab occlusion + the Health/Defeated domain + the 0xC0AB codec);
**P1b** the adversary behavior (`engine-core/adversary.rs`: the
six-state cadence-driven machine over the spine, LOS/hysteresis
perception, the 0xB00B namespace proof); **P2** the event-driven SDL3
audio (`engine-shell/audio.rs`: callback-free cue streams, procedural
placeholders, zero per-frame API, observable silent degradation); **P3**
the scene separation (the Stage-2 test scene now EXPLICIT opt-in —
`DemoScenePreference::Omitted` the default; the renderer imposes no
demo content on game scenes); **P4** the macOS exclusion-test flake fix
(the deterministic complete witness; the failure independently
replicated at P = 1/185.1); **P6** the integrated playable combat
encounter (`engine-slice/combat_demo.rs` + the `combat_world` battery:
the winnable encounter + the reproducible failure scenario, the
per-tick combat transition, B's adversary through the spine, the audio
cues at the observation points, the NEW `engine-core/combat-world/1`
trace domain `d2ef688081dbc659`). All combat/adversary/encounter state
TRANSIENT (round resets, honestly labeled — no checkpoint claims);
provisional values are placeholders. The review chain: every package
independently reviewed (G/H/I), every MAJOR corrective landed and
confirmed (B's snap-branch blocking check; F's per-tick pacing + the
B-seam swap), plus the orchestrator's composition fix (combat_demo's
RenderConfig gained D's demo_scene field). The candidate CI at the
exact SHA is the closing gate; the owner's playtest commands:
`--combat` (the encounter) and `--playable` (the gatehouse, unchanged).

**The Era-1 corrective cycle (2026-11-23/24, the overnight autonomous
run).** The spec is Astra's 20-finding Era-1 rendering review
(`docs/ERA1-RENDERING-REVIEW-286d3e3.md`, merged as #20; findings 1,
10 and 11 pre-verified by the home team). The ladder's four packages
landed first — capture-trust (#21), frame-preparation (#22),
allocations-geometry-gates (#23), architecture-split (#24) — then the
recursive phase per the owner's directive: the sky off-axis extension
(#25), finding 19's fallback-format half (#26), and the closing
check-halves audit (#27). All seven PRs are merged under the owner's
GO (the only merge authority; the count evidence and
merge-order notes are in §4 and the PR bodies). The cycle's closing
re-audit (2026-11-24, independently re-verified against source) found
the recursive queue empty. The merge GOs were given (2026-11-24): the
union landing PR #29 carried the eight-package sequence with the two
#21×#23 merge-time fixes and the measured union counts (the
2026-11-24 union rehearsal, `corrective/union-rehearsal` / PR #28's
comment trail), with #30–#32's stacked coverage and PR #28's own
reconciliation following. The rig run for the rig-pending clauses
(the discrete-GPU capture-visibility behavior and the driver-forced
depth-fallback paths — the RTX 5070 Ti procedure,
`docs/WINDOWS-GPU-PROFILING-KIT.md`) remains the owner's.

**The Era-2 mission (2026-11-24, the overnight autonomous run) —
MERGED whole.** Package 1 (the animation + skinning + proxy-shadows
ladder, PR #33) and the three recursive-phase documents (the NPC AI
design pass #34, the physics middleware decision #35, the audio
subsystem map #36) all took the owner's GO in one wave
(2026-11-24T23:45Z), the #33 merge-commit CI green on all five jobs
with the exact-count pins enforced. What awaits the owner now
(settled at the package-C landing, 2026-11-25T21:53Z): the rig
commands — `cargo run -p engine-slice --release -- --corner
--populated --shadows` and its `--free-look` variant walk the moving
corner (24 NPCs, 16 walking at hand-spread phases, per-NPC soft
shadows that walk with them, 2 sedans), and
**`--corner --free-look --populated --shadows --day-night 60
--weather rain` walks the living corner IN THE RAIN** (the
package C acceptance — ~300 warm streaks falling through the
observed volume, dying at the ground, tinted by the sky;
composing with `--post`, `--behavior`, and `--audio`); the
Era-1 rig-pending clauses
above; the documented follow-ups (~~the shadowed-HDR variant~~ —
CLOSED 2026-11-25 by the `era2/shadowed-hdr-populated` package,
MERGED as PR #44 in the evening GO wave: the
three-way `--post --populated --shadows` composition now works
end-to-end, gated by the full-chain known answers — the rig command
`--corner --populated --shadows --post` walks the HDR-shadowed
corner, composing with `--behavior` and `--free-look`; ~~the
skinned family's pass-independent ownership transfer that would
restore the flat replacement tuple~~ — CLOSED 2026-11-25 by the
`era2/skinned-retained-ownership` package, MERGED as PR #48 — the
wave's last merge, `d782a27` (the pose storage, pool, sets, and
pipeline layout moved to the Vulkan context at device lifetime,
INHERITED across pipeline replacements — the flat replacement tuple
re-pinned in `render_instanced`'s two lifecycle gates, PR #33's
renderer-architecture follow-up executed as recorded); ~~the s6_soak
fsync-race test-only seam~~ — CLOSED 2026-11-25 by the
`corrective/s6-soak-late-window-seam` package, MERGED as PR #45 in
the evening GO wave, §8's record); and the
three documents' owner
decisions (the NPC AI design pass's unratified boundaries, T-06's
physics middleware recommendation awaiting ratification in
`docs/DECISIONS.md`, the audio subsystem map's groundwork
priorities).

**The Era-2 package A arc (2026-11-25, the overnight continuation
beyond the mission file's terminal queue — the design pass as the
implementation brief): MERGED WHOLE — the owner's GO wave
2026-11-25T18:26:20–18:37:23Z took the eleven-PR stack #38–#48
terminal (part 1 by PR #37's GO at 02:02Z, parts 2-8 plus the three
follow-up packages by the evening wave; every rung had been
CI-green on its own head, #38 docs-only by the workflow's path
filter). The wave's merge mechanics, on record for the audit: the
chain was strictly linear by commits, so the owner's click order
(#41, #38, #43, #42, #45, #44, #47, #46) let four REAL merge
commits carry the whole content — `2c456b9` (#41: parts 2-4 and
the #38 reconciliation's docs, fast-forward-shaped onto `d9a034d`),
`7da236d` (#43: parts 5-6), `7bfb49a` (#45: the shadowed-HDR and
s6-soak packages), `7d07c2e` (#47: parts 7-8) — while #38, #42,
#44 and #46 registered through GitHub's already-incorporated path
(their heads were ancestors of `main` the moment the preceding
merge landed; each shows its own head SHA as its merge commit and
created no new commit, no duplicate run); #39 and #40 were CLOSED
by the owner as incorporated (their heads `d6aedbe`/`1257f2d` had
landed on `main` through #41's merge — closing, not merging, is the
honest disposition for a rung whose content is already in); and #48
merged LAST (`d782a27`, 18:37:23Z) after the owner updated its
branch to `2fd72b9` (merging the wave's `main` into it — a
tree-clean merge: main's side was tree-identical to the merge base
`4c7d351`, so `2fd72b9`'s tree is EXACTLY `9c8bcd1`'s, the tree run
`45658412570` had already validated) and its re-run `45675511392`
went green on all five jobs. The endpoint run on the settled head:
`45677048777` on `d782a27`.** Part 1: the NPC AI deterministic cores (PR #37, the
owner's GO at 02:02Z; the design pass's §3–§6 as three additive
pure engine-core modules — §2's record carries the contracts and
pins). Part 2: the spine integration (`era2/npc-ai-spine-integration`,
stacked on the #38 reconciliation — §6.2's contract on the real
queue, the spine untouched; §2's record carries the machinery, the
battery, and the measured pins). Part 3: the locomotion batch
(`era2/npc-ai-locomotion-batch`, stacked on part 2 — §4.3's
near-lane batch + §4.2's Full-tier locomotion integration; §2's
record carries the machinery, the battery, the measured digest
pin, and the §8.5 measurement). Part 4: the rendering bridge
(`era2/npc-ai-rendering-bridge`, stacked on part 3 — §7's contract
landed: the corner demo's populated route reads BEHAVIOR STATE,
the locomotion population stepped per presented frame at the
deterministic clock (render-then-step, dt = 1/60) with the
instances and pose blocks re-evaluated into retained buffers —
zero per-frame allocation, MEASURED: 0 host allocations per warmed
moving frame and `renderer.allocations()` constant across the
warmed animated window; the corner's walking lanes as pure data —
eight far-segment flank loops with lanes exactly 0.75 m apart (the
different-NPC margin met by construction, dyadic coordinates), two
crosswalk pacers at the original crossers' exact spots, the
waiters byte-identical; the swept-validity battery repaired to
measure every sample unconditionally (the WIP's min-coord filter
silently dropped the road-side lanes and every pacer sample — the
weakened-gate class, barred) and the 663-frame temporal campaign
pinned (double-run byte-identical, the locomotion digest at
0/33/66/300/600/662, pairwise walker clearance ≥ 0.75 — the
measured minimum EXACTLY the lane separation — and waiter-waiter
≥ 0.7, the original table's cardinal rule); the render_instanced
gates re-pinned on the locomotion source (the moving-crowd 26-vs-2
byte equality, the frame-0 static fixtures, the allocation
constancy under motion); heavy shard 591 → 599 Linux MEASURED
with the workflow's own awk, main shard untouched at 1295/2/33).
Part 5: the composed gates (`era2/npc-ai-composed-gates`, stacked
on part 4 — §8.4's named allocation-counting gate made possible:
the `--behavior` flag composes the behavior spine INTO the demo's
populated crowd — the 24 schedules (the waiters' idle re-arms, the
crossers' alternating pacing commitments, the strollers'
stroll/rest pairs), the `CornerCrowdWorld` spine world (evaluate
at the wake, the authored seam `set_destination` table, the
arrival mirror, the pre-allocated stream), one composed tick per
presented frame — spine first, then the batch; the campaign
pinned: 99 wake executions over the 663-tick window (the cadence
arithmetic hand-derived and asserted), 25 arrivals (measured),
double-run byte-identical (states, stream, world fingerprint) with
digest pins at sampled ticks — frame 0's locomotion digest EQUALS
part 4's (the composed mode's first frames are the plain mode's);
the waiters byte-frozen through the composed campaign; the
allocation-counting gate with the spine-churn attribution clause
(the investigation's finding: `Spine::schedule`'s unfinished-index
node churn allocates on an amortized cadence — engine-core
internals, documented, out of scope; a composed frame with NO wake
execution measures EXACTLY zero host allocations, `renderer.
allocations()` constant; the demo's own exit summary measured 99
wakes / 25 arrivals over the route); heavy shard 599 → 603 Linux
MEASURED, ignored 31 → 32 (the new composed lavapipe gate), main
shard untouched at 1295/2/33). Part 6: the measurement surface
(`era2/npc-ai-measurement-surface`, stacked on part 5 — §8.4's item 7
and §8.5's remaining measurements as `--benchmark-behavior`, one new
engine-slice module over the REAL machinery, engine-core untouched):
the **behavior-workload sustained matrix** (12 cells: populations
{125, 1000} × demand classes {low 100, near 200, overload 400
events/tick vs budget 200/reserve 60} × synchrony {staggered,
aligned}, authored cadence arithmetic with the class-target assert;
per-cell 600 warm + 30×60 measured ticks, double-run digest +
fingerprint asserted, spine conservation asserted — MEASURED on this
pod: staggered low/near deliver their authored rate at p50 ≈ 2.0/4.1
ms per 60-tick window with ZERO lateness and ZERO exhaustion; the
aligned variants reproduce the E-10 cluster signature (low-demand
aligned: 214k late executions, 1_072 exhausted ticks of 2_400 —
budgets sized to cluster PEAKS, not averages); the overload cells
expose the honest re-arm's self-limiting shape — delivered caps at
the budget with the backlog bounded at the ENTRY count (2_000
pending, every tick exhausted, 480k late executions over 2_400
ticks: per (npc, commitment) the chain is one-deep, so sustained
authored overload CANNOT pile unbounded work — the E-10 acceptance
holds by construction); cost tracks the event rate, not the
population (125 and 1000 at equal rate within noise, ≈ 0.33 µs per
wake event end-to-end); the **witness enumeration cost** (census +
the full per-significant-event path at {24, 125, 1000} × {all-Full,
mixed, all-Dormant}: the census is linear in population — p50 0.092/
0.271/1.263 µs — and tier-gating is free; the full event at
1_000-scale all-Full admits 81 witnesses at p50 4.9 µs; the
all-Dormant census walks the same 1.26 µs and admits NOTHING —
I2/I3's cost shape); the **activation crossover** (E-09 on the real
machinery: 125 NPCs × 2 entries, k ∈ 1…64 missed occurrences —
replay re-executes the missed events through the real queue
(late-arrival arm + drain to the reactivation point, executed ==
250k asserted), synthesis is the production shape (one
`evaluate_wake` per NPC — the single lazy decay over the whole span
— plus the `next_occurrence` re-arm jumps, asserted at (k+1)·600
and 300+k·600); MEASURED: replay grows linearly ≈ 0.094 → 9.96 ms
p50 across k, synthesis is SPAN-INVARIANT at ≈ 0.028 ms (the decay
is O(1) in the dormancy span) — the crossover lands at k = 1, BELOW
the harness's 7.6–12.4 (the real machinery's synthesis is cheaper
than the harness's: the adaptive policy's threshold is moot here,
replay never wins in the measured range; §9.1's launch-policy
question stands, now with the real number); the **skip-cost curves**
(E-08 on the real queue: seven (duration, events) cells at D ∈
{10⁴, 10⁵, 10⁶} with the pair structure isolating events from
elapsed, every armed event's honest re-arm authored beyond the
window so executed == armed exactly — MEASURED decomposition ≈
35.8 ns/tick + ≈ 0.336 µs/event: the event term dominates at
game-relevant densities, and the E-08 pair's reading (same elapsed,
different event counts) shows the DELTA tracking events with the
tick floor common to both sides; the dormant contrast: 1_000 NPCs'
retained calendars re-derive in 0.025 µs — the quiet skip is free);
and **memory-per-NPC by tier** (the accounting over the real structs
at entries/NPC ∈ {2, 16}: Full 840/1_288 B, Simplified 316/764 B,
Event-driven 196/644 B, Dormant 20 B (the record + next_event —
schedules are authored shared data, not per-NPC resident); the
measured composed world 12_916 B total / 538 B per NPC, locomotion
8.2% — monotonicity and component sums pinned by the lib battery).
The battery (5 platform-uniform lib tests) pins the matrix
workload's stream digest (12_420_752_226_621_264_573) and the
witness census digest (16_487_293_382_058_425_681), both baked from
measured runs stable across independent executions. The design
pass's §9 open-questions list still awaits the owner.

**Part 7: the capture-lane gates** (`era2/npc-ai-capture-lane`,
2026-11-25, stacked on the s6-soak-seam tip — §8.4 item 4, the LAST
unlanded §8.4 gate, delivered): the durable perception memory lane
through the real pipeline, deactivate/reactivate exactly-once, the
S6 fuzz discipline, and reputation candidates
pending-until-classified — the design pass's §5.3 lane shape (the
episodic entry, the relationship delta, the reputation candidate —
all captured at production, pending-live in both phases, riding the
commit pipeline to canonical confirmation asynchronously, never
blocking a transition) landed as engine-core's `npc_memory` core
(the 18-test battery) and the `npc_memory_lane` composition over
the real spine/census/CommitPipeline (the seven-gate battery with
THE S6 fuzz gate — the S6-lite op distribution and fault classes,
the per-op conservations, the C1 rebuild checks, the C2 faults, the
independent oracle, the double-run byte-identity; MEASURED: 38
encounters, 431 records, 188 pipeline-confirmed, the shared
reputation −105, 310 ms both campaigns). The fuzz gate caught and
the package fixed two real defects before landing (the lazy
tier-change retention re-application; the confirmation loop's
claimed-set ordering). The §5.3-listed composition follow-ups
remain the owner's: the memory depth and reputation classes are
[OPEN] §9.2 (the engine ships the knobs; the demo authors the
policy table); the M-08 shared-reputation game composition and any
ledger-content compaction are measured-trigger decisions (the
full-fidelity ledger is the shipped shape — the S3-lite cost
lesson recorded; compaction only if measurements demand it).

**Part 8: the composed-crowd camera crossing** (`era2/npc-ai-composed-crowd`,
2026-11-25, stacked on the part-7 tip — part 4's documented deferral
closed, the honest queue's next item): the full crosswalk crossings on
the shared clock. The pacers became crossers walking the whole road arm
(three-leg loops, the ±3.7 waiter standoff documented, frame 0
byte-identical), the crossing timing authored against the camera's own
schedule and gated by THE TEMPORAL CAMERA GATES — the plain 1280-step
window and the composed 663-tick campaign, both under the demo's own
warmup-lead pairing (crowd step k with camera pose clamp(k−60, 0, 600)
— the pairing verified in corner_demo's route loop, WARMUP = 60),
every frame's whole population ≥ 0.8 from the camera's same-frame
position with the measured minimum 1.8529123 at step 178 pinned in
both gates (the composed gate measuring the plain gate's own value as
its cross-check — the composed trajectory is the plain one through
the first curb stand). The composed schedules re-authored (the
crossing commitments at 90/510, cadence 840 — every retarget after a
curb arrival, the far-curb stands the pedestrian's wait), the wake
count 99 → 82, the arrivals 9 (measured; the crossers' 2 + the
strollers' 7), the digests re-pinned (ticks 0/33 equal the plain
campaign's own pins). The implementation went to the coder subagent
(the mission's delegation rule); the round's audit (this orchestrator)
found and repaired three defects before landing: the semantic
crossing bounds derived from a WRONG SCENE EXTENT (the spec's author
— this orchestrator — had modeled SCENE_EXTENT as 16; the probe of
the real route_poses proved 40, moving the camera's crosswalk passes
to poses ~116.5/~380 and the ±1.5 m window to [370, 390] — the
bounds re-derived in pose-index terms), the composed gate's
off-by-one (measuring after the tick's step — one frame of mismatch
against the demo's render-then-step; restructured to measure before),
and an unrequested extra test correctly removed — with one repair of
the repair itself: the round's `pose_bytes_fnv_pins` deletion
initially took PART 4'S LANDED TEST with it (recognized in the count
reconciliation: the lib battery measured 97 against the predicted
98, the inventory diff showing the baseline already carried it since
df06b22) — restored verbatim from the tip, its pins holding
UNCHANGED (the pose-phase stream is route-geometry-independent, a
verification in itself). The counts: heavy 618 → 620 Linux MEASURED
with the workflow's own awk (+2 platform-uniform lib tests), Windows
619 inferred, main 1313/2/33 and renderer release 160/2 UNCHANGED;
every existing gate re-fired green (render_instanced 7/7,
render_shadows 4/4 on the pod's lavapipe; the three routes
end-to-end with validation 0/0; the composed exit summary
reproducing the lib pins digit-for-digit).

**The shadowed-HDR populated variant (2026-11-25, the follow-on
session's package — Era-2 package 1's documented follow-up closed,
branch `era2/shadowed-hdr-populated`, stacked on the part-6 tip):**
the last corner-flag composition gap. The defect, MEASURED before
the fix on the pod's lavapipe: `--corner --populated --shadows
--post` REFUSED on its first frame — "error: populated route drew
no proxy calls" — because the post chain's HDR scene pass passed
`None` for the shadowed proxy pipelines (PR #33's documented
follow-up; its body's "draws the proxies unshadowed-in-HDR"
understated the shipped behavior — the recorder's belt-check
skipped the population entirely and the demo's fail-visible
populated-route check errored the route). The fix, the
`SkinnedProxyResources::ensure_hdr` precedent: two lazily-created
HDR pipeline variants on the shadowed-populated state (the same
shader modules, pipeline layouts, and descriptor sets as the
main-pass receiving pipelines — only the HDR render pass differs,
so the one-time cost is two pipeline objects, invisible to
AllocationStats), ensured in the frame flow's post family-growth
block at the first populated shadowed post frame (per-frame
idempotent — the pipeline-replacement path re-ensures against the
new pass, the finding-A discipline), supplied through the HDR
scene pass's `SceneTargetPipelines` (the recorder's belt-check
stays as the lazy-window guard, the sky_hdr shape). The gate
(`shadowed_populated_composes_through_the_hdr_chain`, the heavy
shard's ignored 32 → 33): the defect pin (the full 24+2 rest table
through the three-way path, 2 draws), the presence ink (34,166
bytes vs the unpopulated shadowed frame), the one-time
populated-families' delta (6 device_memory / 4 pools / 16 sets / 0
samplers — the instance buffer + the two mesh VBOs + their staging
memories + the pose storage; the skinned pool + the
shadowed-populated state's three; the 3 + 13 sets) then CONSTANT
across warmed composed frames with the warmed frame
byte-identical, and the decisive full-chain known answers — a
light-facing NPC torso wall (the rotated-oracle orientation, N·L ≈
0.707) placed by deterministic CPU search DEEP inside a building's
shadow (the raycaster verifying five probes + ±0.3 m margins, the
footprint and camera corridor clear) rendering EXACTLY
`albedo·ambient` through the bloom + ACES chain (57,600 pixels,
max 1/255), and the same wall in open sun rendering the FULL lit
equation (57,600 pixels, max 1/255 — no self-shadow acne: the
wall's self-depth-plus-bias compare holds exactly as the 6-F lit
ground patches prove, at the same light-space slope); the two
equations differ by 0.360 linear at the wall center, so the
fixture pair discriminates both an always-ambient and a
map-ignoring pipeline. Every existing gate re-fired green
(render_post 6/6, render_shadows 4/4, render_instanced 7/7 — the
postless paths byte-identical); the routes ran end-to-end
(`--populated --shadows --post`: 601 frames, 0/0 validation; the
composed `--behavior` variant: 99 wakes / 25 arrivals, part 5's
pins reproducing); main 1295/2/33 MEASURED unchanged, heavy
608/33/62 MEASURED with the workflow's own awk (the ignored pin
corrected from the measured actual). The implementation went to
the coder subagent (the mission's delegation rule) and its round's
audit found the decisive fixtures missing (the spec's text pasted
as comments — the overclaim class PR #38 records); the home team
implemented the fixtures and repaired the round's working halves
(the hand-rolled bias copy → the demo's own `demo_bias()`, the
allocation delta re-measured and re-pinned from (3,3,16,1) to the
actual (6,4,16,0) with the staging-memory decomposition).

**The s6_soak late-window seam (2026-11-25, the follow-on package —
§8's open flake fix closed, branch
`corrective/s6-soak-late-window-seam`, stacked on the shadowed-HDR
tip):** the third recorded 2026-11-23 runner-timing flake retired by
construction. The demo's hidden `--park-publication-at-tick <t>`
(#[doc(hidden)], the `--abort-after-tick`/`--poison-at-tick`
precedent — engine-slice's demo-layer seam class, not the B4
crossing's cfg-gated statics) arms the I/O worker's EXISTING
`ParkExecutingAppend` fault at the scripted publication's own
scenario tick, just before its append job is submitted: the worker
parks inside the job's dispatch (admitted and started, past the
poison and cancellation boundaries, no store execution — nothing
of the transaction durable), and the paired hard exit kills the
parked worker with the process. The production persistence stack is
untouched (the fix's one refinement over §8's sketched shape: the
seam sits at the demo's publication boundary, not inside
engine-persist — the barrier machinery already existed in the
worker). The §7D control test arms it at 3300; the 3054 recovery
and every close assertion unchanged; MEASURED on this pod: three
consecutive green runs (69.6/69.5/69.7 s — the determinism proof)
plus the full s6_soak battery 34/34, fmt/clippy clean, the main
shard 1295/2/33 unchanged, and NO count movement anywhere (the
field/flag is test-free harness code; no workflow pin touched).
The transcription round went to the coder subagent (the mission's
delegation rule) and verified clean against the spec on the first
read — including the one construction site the orchestrator's own
pre-check missed (s6_evidence.rs).

**The skinned family's pass-independent ownership transfer
(2026-11-25, the follow-on package — PR #33's recorded
renderer-architecture follow-up closed, branch
`era2/skinned-retained-ownership`, stacked on the part-8 tip;
MERGED as PR #48 — the evening GO wave's last merge, 18:37:23Z via
`d782a27`, the owner's branch update `2fd72b9` first with its
five-job run `45675511392` green on the exact tree):**
the flat replacement tuple restored. The defect, on record since
PR #33's repair chain: `SkinnedProxyResources` mixed pass-AGNOSTIC
resources (the mapped pose storage, the skinned set layout/pool,
the three per-material sets, the pipeline layout — objects that
bind no render pass, only context-owned corner resources plus the
family's own pose buffer) with the pass-BOUND pipelines under
`PipelineState` ownership, so every forced pipeline replacement
re-created the pool and sets — the churn measured at +1 pool / +3
sets per replacement (CI run 45488497488, the pin this package
retires). The fix, the recorded design executed as written: the
split — `SkinnedRetained` (everything pass-agnostic) now lives on
`VulkanContext` at device lifetime, created once at the FIRST
populated frame ever (`ensure_skinned_retained`, idempotent, the
corner-typed refusal preserved) and INHERITED by every pipeline
generation (the `pipeline_cache` precedent); the per-state
`SkinnedProxyResources` shrinks to the main-pass pipeline + the
lazy HDR variant, rebuilt per generation by the finding-A
replacement discipline on the RETAINED layout; `ctx.destroy`
releases the retained state FIRST (reverse creation order; the
Renderer::drop pipes→ctx ordering is the proof in release's
SAFETY). The shadowed-populated family is deliberately NOT
transferred (its sets bind the re-created shadow state's per-slot
map views — the recorded reason the transfer does not generalize);
it keeps the finding-A re-ensure and reaches the pose buffer
through the context. The re-pin (in place, no count movement):
`render_instanced`'s two lifecycle blocks assert the 4-tuple
(texture_memory, pools, sets, samplers) EXACTLY EQUAL to the warmed
tuple — the successor of the +2 pools / +6 sets churn pin and
STRONGER (any per-replacement re-creation in the family now fails
the gate outright). MEASURED on this pod's lavapipe: render_instanced
7/7 — warmed pools 3 / sets 7, after BOTH forced replacements pools
3 / sets 7 FLAT (device_memory 19→39 / 20→42, the replacement's own
reallocs plus the re-ensured proxy family's, printed not pinned),
double-run digit-for-digit; every byte-equality, draw-call,
oracle, and host-allocation-zero assertion green; render_shadows
4/4; render_post 6/6 — the shadowed-populated HDR composition
gate's one-time delta (6/4/16/0) UNCHANGED (the first-frame
creation cost is identical, only the churn retired), the torso
walls at max 1/255; the populated route end-to-end 601 frames
validation 0/0; the CI resize smoke green. The shards with the
workflow's own awk, ALL UNCHANGED: main 1313/2/33, renderer
release 160/2, heavy 620/33/63 Linux and 1262/619 Windows — both
platforms MEASURED, the Windows inferences confirmed by the
candidate's own first CI run (45656765496, all five jobs green on
the exact tip 1a38f9a, first push, every exact-count gate
enforcing the measured counts); fmt and clippy -D warnings
clean in both forms. The delegation record, honestly: two rounds
to the coder subagent produced analysis only (the tree verified
clean after each); the orchestrator implemented the spec itself
and audited every hunk before the gates.

**The Era-2 package C arc (2026-11-25, particles + weather — rain
over the corner, deterministic to the byte; MERGED as PR #51 at
21:53:23Z via `04b09b9`, the final run `45702469099` green on all
five jobs with every exact-count gate enforcing the measured
counts):** the five-stage ladder — the deterministic cores
(`0c64601`: the seeded spawn stream, the pinned integration
order, the typed overflow, **the digest pin
`16840467954493462700`**), the renderer's third populated kind
(`dc7eadc`: the unlit blended `particle.wgsl`, the alpha claim on
the tint vec4's `.a` with the existing packed-bytes pins
UNCHANGED, the third indirect command, the `ccc5b50` placeholder
retired per the no-new-pipeline-class law), the verifier-found
corrective (`ee8b907`: the render_post populated-HDR delta
re-pinned +2 device_memory — the rain kind's third mesh VBO and
its staging), the demo rain layer + the gate battery (`8128ae2`),
the final main merges (`89ae9dd`/`b6b3f1b` — the audio
compositions absorbed, the three-way `--populated --weather rain
--audio` composition), and the strengthening fold-ins (`7749ebd`:
the N-emitter composition gate, the broadside ink gate measuring
1660/1350, the validated intensity constructor). **The two-seat
record, honestly:** the server restart orphaned the 17:07
implementation seat while the supervisor spawned the 19:07 seat —
both held the package C mandate, and a genuine duplication
crisis (two parallel cores with diverging digest pins in the
making) was resolved through the repo-as-shared-state
coordination (the pushed canonical core, the worktree note, the
OVERNIGHT-STATUS records, the PR comment trail): the cores and
the renderer kind from the implementation seat, the demo layer
and gates from the build seat, each verified by the other (the
build seat's line review caught the render_post pin the
implementation seat's round missed; the implementation seat's
full-device-surface runs verified the build seat's delivery).
The parallel drafts survive as mining branches
(`era2/pc-wip-billboard-renderer`, `era2/pc-impl-seat-stage45`);
the digest-pin-once argument held — one canonical core, one
canonical pin. The package's count history includes the red run
`45693271859` (the stale-pin push, corrected from actuals at
`8128ae2` — the exact-count gates doing exactly their job) and
the measured landings 1369/1318 → 1371/1320 (main) and 625/624
with ignored 38 → 39 (heavy). The recursive follow-ups: the
wet-ground rain ripples (in flight on `era2/rain-ripples*`,
stacked through the package branch) and the particle LOD tiers.

## 10. The authoritative coordination ledger

- **Location:** `/home/coder/sektor7-coordination/` on the development pod
  (the live operational ledger since the 2026-11-14 clean-slate restart —
  the legacy `~/.qwen/sektor7-coordination/STATUS.md` location was wiped in
  that day's workspace purge and is obsolete) — **outside the Git
  repository, by charter**. It holds the owner's directives preserved
  verbatim with SHA-256 (`owner-directives/`, verified against
  `HASHES.sha256`), the cycle charters/matrices/challenges/reviews/deliveries
  (`cycle1/`), and the relay log (`sessions/registry.md`).
- **Ownership:** the **GLM orchestrator session** (`coder-1e`) is the sole
  writer, under the standing chain of command — Owner → ChatGPT (independent
  technical lead) → orchestrator → engineering seats. Seats receive
  provenanced relays (owner-directive + hash references, relay IDs
  registered in the ledger). The owner's authorizations are the preserved
  directive files, never paraphrase.
- **Durability:** pod-local operational storage (not committed, not on
  GitHub). It is deliberately independent of the repo's branches and
  worktrees, so branch/worktree cleanup never invalidates references to it.
  After a coordinator-session reset it is rebuildable from the preserved
  directive files plus the repository's own records (commit messages and
  `docs/`); this index carries the technical status, the ledger carries the
  campaign/operational state.

**Present state (verified 2026-11-24, the post-merge session):** the
ledger directory is absent from the current pod provisioning
(`/home/coder/sektor7-coordination/` does not exist — the pod was
re-provisioned, taking the directive files with it). The durability
clause above anticipated the reset half: this index and the
repository's own records (commit messages, `docs/`, the PR trails on
GitHub) are the surviving technical authority — matching the standing
mission rule that all durable state lives in the repo, branches, and
PRs. The overnight supervisor's operational records live at the pod
root (`OVERNIGHT-STATUS.md`, outside the repo by the same charter).

---

## The capability-readiness matrix

Classification per the owner's directive (2026-11-14 §10): **Implemented and
validated** / **partially implemented** / **prototype only** / **designed but
not implemented** / **not yet designed**.

| Capability | Classification | Basis |
|---|---|---|
| Deterministic simulation & scheduling | Implemented and validated (demo scale; sustained-scale measured at the covered populations — §7/§8) | `engine-core` spine; the determinism pins; the CI battery; the entity-scale benchmark; the sustained-load reference matrix |
| Persistence & recovery | Implemented and validated | `engine-persist` store/journal/pipeline; the crash/recovery matrices; real process-kill proofs; cross-platform CI; the per-platform durability tiers (§5/§8) |
| Rendering (Vulkan presentation) | Partially implemented (prototype-class design) | `engine-renderer`; lavapipe + rig validation; F-03 Step 1 delivered (fail-visible truncation counters), Step-2 policy + F-04 open |
| Shader/material architecture | Prototype only | Hand-written build-time naga shaders; no material system |
| GPU resource streaming | Not yet designed | F-04 deferred; host-visible copies only |
| Particles + weather | **Implemented (Era 2 package C, MERGED #51)** | The deterministic particle core (engine-core `particle.rs`: the seeded per-emitter SplitMix64 spawn stream, the pinned integration order gravity→drag→position→life→death-test with stable compaction, the fixed-array bounded pool with the typed counted overflow refusal, the trajectory digest pin `16840467954493462700`, the N-emitter composition gate) + the weather model (`weather.rs`: pure state, caller-supplied `RainPolicy`, the observed-volume `rain_emitter_config`, the validated policy form) + the renderer's THIRD populated kind (rain through the existing instanced path — the unlit tinted alpha-blended `particle.wgsl`, depth test-on/write-off, `ProxyInstance.alpha` with the existing packed-bytes pins unchanged, the third indirect command, no rain in the shadow map) + the demo rain layer (`corner_weather.rs`: warm-started, the 4 m-quantized listener-following spawn box, render-then-step at dt=1/60, the physically-derived billboard turn, the exit census with the conservation exact) — gated by the lavapipe battery (the ink-count both metrics, the both-counter allocation constancy, the double-run byte-equality, the rain-kind N-vs-1, the typed overflow surfaced). `--corner --free-look --populated --shadows --day-night 60 --weather rain` walks the living corner in the rain; composes with `--post`, `--behavior`, and `--audio`. The wet-ground ripple extension and the particle LOD tiers are the package's recursive follow-ups |
| Instanced/indirect draws | **Implemented (6-E + 6-E2)** | The Cycle 6-E implementation landed 2026-11-23 on the scaffold's deterministic core (the libm-free turn-fraction transforms, the byte-pinned packing, the proxy meshes, the `instanced_proxy.wgsl` shader): the instance-rate vertex input, the lazily-created zero-per-frame-allocation instance buffer (allocations constant across warmed populated frames), one draw call per populated kind (the populated corner route: **26 instances — the full 24-NPC + 2-vehicle placement table, the ladder's population package `corner_population`, pure data with a placement-validity battery and its own packed-bytes FNV pin — in 2 calls**), the N-vs-1 pixel-equivalence gate (`tests/render_instanced.rs`, byte-equal frames for BOTH the 13-instance fixture and the shipped 24+2 table, per-pixel 6-B oracle checks at 0/255 max error on lavapipe), the draw-call pins, and the `--corner --populated` demo mode — see `docs/CYCLE6-6E-BRIEF.md`. The Cycle 6-E2 package (2026-11-23, riding the population branch) added the indirect submission: `ProxyDrawMode::Indirect` — one `vkCmdDrawIndirect` per populated kind with the counts read from GPU memory, the pure byte-pinned `proxy_indirect` command core (FNV cross-platform pin, `firstVertex`/`firstInstance` pinned zero — no `drawIndirectFirstInstance` feature), a lazily-created per-frame-slot mapped command buffer (one-time +1 device_memory, pinned; the direct path's counters untouched), byte-equal frames across per-object/direct/indirect submissions (the third `render_instanced` gate, with an oracle wall through the indirect path), and the populated route's default submission now indirect (`--proxy-instanced` selects the direct 6-E shape). GPU-side count AUTHORSHIP (compute/culling writing the counts) remains deferred to the GPU-culling package. The rig measurement commands live in the 6-E/6-E2 PRs (lavapipe is correctness evidence only) |
| Directional shadows | **Implemented (6-F + Era-1 PCF + Era-2 proxy shadows)** | The Cycle 6-F implementation landed 2026-11-23: the deterministic core (the light-space orthographic cascade, the compare-time slope-scaled bias model, the CPU raycaster oracle — pure IEEE, unit-pinned) + the depth-only pass + the shadowed lit pipeline sampling the map (one cascade, the corner scene — PCF-softened since Era-1 package 2). The gates: the map's known-answer (every texel vs the raycaster, max |Δdepth| 2.7e-7 on lavapipe) and the pixel equivalence (a shadowed ground patch renders exactly `albedo·ambient`, a lit patch the unshadowed equation, the unshadowed path pixel-identical) — `tests/render_shadows.rs`, executed by the CI's Linux Vulkan step. The `--corner --shadows` demo mode runs the full route. PCF softening (Era-1 package 2, the 3×3 `shadow_filter` tap table shared verbatim with the CPU oracle) and proxy shadows (Era-2, PR #33 — the populated path casting AND receiving, the skinned per-NPC depth casts) are landed; cascaded splits remain open |
| NPC/world representation (LOD) | Partially implemented | The region director, four LOD levels, activation/reconstruction — validated at 1000 NPCs / 8 regions; the NPC AI design pass merged (#34) with its deterministic cores landed (PR #37: the behavior selector machinery, the schedule layer, the perception layer — pure, pinned), the spine integration landed (package A part 2: commitment wakes as spine events, the witness census, the honest re-arm — pinned by the integration battery), and the locomotion batch landed (package A part 3: routes as pure data with authored turns, the hand-spread draws, the position bridge, the identity-ordered zero-allocation batch step, the pinned-blocked collision composition — pinned by the 18-test battery with the campaign gates over a real spine, §8.5's p99 measurement discharged: 4.7 µs/tick at the corner's 24 walkers, 70.7 µs/tick at the 1,000-walker pathological case on this pod); the rendering bridge, the composed gates, and the measurement surface landed (package A parts 4-6, MERGED in the 2026-11-25 evening GO wave: the behavior-state walking crowd at zero per-frame allocation, the behavior-spine composition with the allocation-counting gate, and §8.4/§8.5's measured evidence — the sustained matrix, the witness cost, the E-09 crossover at k=1 with span-invariant synthesis, the E-08 skip decomposition, memory-per-NPC by tier); the capture-lane gates landed (package A part 7, MERGED in the wave: the durable perception memory lane — the keyed capture ledger with capture-at-production I1, the canonical-order rebuild, the M-08 shared reputation folding accepted candidates only, and THE S6 FUZZ GATE over the real spine/census/CommitPipeline with the per-op conservations, the C1/C2 checks, the independent oracle, and the double-run byte-identity); the composed-crowd camera crossing landed (package A part 8, MERGED in the wave: the full crosswalk crossings on the shared clock — the pacers became crossers walking the whole road arm behind the camera, gated by the two temporal camera gates under the demo's own warmup-lead pairing, the measured same-frame minimum 1.8529123 pinned in both, the composed crossing commitments with the far-curb stands) |
| Physics and collision | Prototype only | The Cycle-1 kinematic player prototype (`engine-core` player module: swept-AABB collision, fixed-step movement, FP/TP cameras) — headless/offscreen validated, collide-what-you-see; reversible and explicitly NOT a ratified production physics backend (§8) |
| Combat and gameplay systems | Partially implemented (demo-scale prototype, validated) | The Cycle 2–5 line: `engine-core` combat resolution (`resolve_attack`, pure, allocation-free) + the adversary cadence model + the encounter composition with cover/HUD/cues in `engine-slice` — pinned by the combat_world (21) and encounter_world (30) batteries with byte-identical scenario traces; game-scale gameplay systems (progression, economy, AI beyond the cadence prototype) remain undesigned |
| Networking and replication | In-session boundaries **designed (unratified)** | The player-facing multiplayer and governance rules are **ratified** in `docs/MULTIPLAYER-SPEC.md`; the campaign-level authority/persistence research lives in `docs/MULTIPLAYER-ARCHITECTURE-RESEARCH.md` (unapproved); the **in-session architecture** — sim placement, input ordering, replication scope, divergence detection, session lifecycle — is designed in `docs/CYCLE6-6D-SESSION-REPLICATION-BOUNDARIES.md` (**unratified**; owner decision list §9); transport/provider selection remains undesigned (T-06 deferral) |
| Listen-server authority | Designed (unratified) | The 6-D design's S3 direction — host-authoritative canonical spine with optional non-authoritative client mirror — is a PROPOSAL; no engine system |
| Host handoff | Designed (unratified) | Planned-required per the ratified multiplayer requirements; the 6-D design maps the handoff onto the research §5 lease/epoch machinery over `engine-persist`; no engine system |
| Co-op roster governance | Not yet designed | The player-facing governance rules (roster democracy, unanimity requirements) are **ratified** in `docs/MULTIPLAYER-SPEC.md`; no engine system |
| Procedural world generation | Not yet designed | — |
| Dense city streaming | Not yet designed | F-08 deferred |
| Assets and content pipeline | Not yet designed | — |
| Editor/creation tools | Not yet designed | — |
| Performance instrumentation | Implemented and validated | The benchmark flags; the run summaries; the F-06 canary; the GPU kit; the Cycle-1 sustained-load reference matrix and the renderer truncation counters |
| Production deployment | Not yet designed | The CLI is a development driver, not a product |
