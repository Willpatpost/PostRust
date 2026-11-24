# ERA2 NPC AI DESIGN PASS — behavior, perception, schedules for hundreds of persistent NPCs

> **Status:** Design pass v1.0 · 2026-11-24 · **UNIMPLEMENTED DESIGN — nothing here is
> ratified, nothing here authorizes code.** This is the Era-2 design pass the
> overnight mission's recursive queue names first: the design for the living
> crowd's *behavior, perception, and schedules*, in the research program's
> terms, grounded in the engine's actual built substrate.
>
> **Ownership:** this file designs the NPC-AI surface of the simulation layer.
> It **consumes** (never redefines): [`CUSTOM-ENGINE-RESEARCH.md`](CUSTOM-ENGINE-RESEARCH.md)
> (§0.3 the living-world mandate; §2 domain 4; §4 Stage 3 — Experiment B's
> 100/500/1,000-agent workload vocabulary), [`SIMULATION-ARCHITECTURE-RESEARCH.md`](SIMULATION-ARCHITECTURE-RESEARCH.md)
> (the tiered-hybrid hypothesis, the four LOD levels, the activation/
> deactivation contracts, invariants I1–I8, the S-experiment ledger),
> [`DECISIONS.md`](DECISIONS.md) (V-03, M-07/M-08/M-13/M-27–M-30, T-06/T-07,
> T-14/T-15, E-03, E-07–E-10 — the registry owns every decision; this pass
> *proposes*), the **built engine** (`engine-core`'s spine/LOD/suspension,
> `engine-persist`'s pipeline, `engine-renderer`'s instanced + skinned path,
> the Era-2 animation core on branch `era2/animation-skinning-proxy-shadows`,
> PR #33, awaiting the owner's GO), and [`CYCLE6-6D-SESSION-REPLICATION-BOUNDARIES.md`](CYCLE6-6D-SESSION-REPLICATION-BOUNDARIES.md)
> (the unratified session-replication boundary design an NPC-AI surface must
> compose with). It **defers to** DESIGN-BIBLE (gameplay rules — encounter,
> mortality, quest succession), MULTIPLAYER-SPEC (player-facing policy), and
> the owner (every [OPEN] item in §9).
>
> **Evidence key (the research program's):** **[PROJECT EVIDENCE]** =
> demonstrated in this repository (built, tested, measured — cited);
> **[ESTABLISHED-CLASS]** = technique class documented in public
> game-development literature; **[HYPOTHESIS]** = this pass's unproven
> composition; **[ASSUMPTION]** = labeled workload/budget guess pending
> measurement; **[OPEN]** = unresolved product decision for the owner.

---

## 0. Scope — what this pass designs, and what it refuses

The first game's living-world mandate (V-03): *authored districts, procedural
frontier, **hundreds of persistent NPCs**, dynamic time/weather, encounters*.
The Stage-3 research vocabulary (Experiment B): *schedules, goals, travel,
events, state changes, and interactions; multiple districts, near-player and
far-away agents, and persistent offscreen progression; measurable behavioral
fidelity.* Era-2 package 1 (PR #33) gave the crowd its **body** — the
deterministic animation core, the skinned instanced path, walk phases. What
the crowd still lacks is its **mind**: NPCs stand at their placement-table
positions or walk in place; nobody goes anywhere, notices anything, or keeps
a schedule. This pass designs that mind:

- **§4 Behavior** — the per-NPC decision layer: how an NPC selects what it is
  doing (its *activity*), moment to moment, deterministically, at four LOD
  tiers.
- **§5 Perception** — the sensing layer: what an NPC perceives about the
  world and the players, at what fidelity, at what cost, and which
  perceptions leave durable traces.
- **§6 Schedules** — the calendar layer: authored recurring and one-off
  commitments as pure data, executed as spine events, surviving dormancy and
  time-skips.

**Explicitly out of scope** (each deferred with its owner): combat AI and
encounter micro-state (DESIGN-BIBLE G-02, the M-30 encounter-transaction
lane — the spine's provisional class exists for it, no design here);
dialogue and quest logic (DESIGN-BIBLE); NPC mortality mechanics (G-08 — the
*detection* of a killable-NPC event is a perception input §5 covers, the
*consequences* are DESIGN-BIBLE §6 succession); networking/replication
*implementation* (CYCLE6-6D's boundaries are consumed as constraints, §8);
physics-based locomotion (T-06's deferral — the physics decision doc is the
recursive queue's next item after this one); pathfinding *algorithm choice*
(§4.4 states the contract and the measured trigger for the decision, not the
algorithm).

**The pass's own discipline:** every design element below is stated as
*engine primitive* or *game policy* (the mission's boundary rule), every
claim carries its evidence key, and every measurement claim cites its source
or is labeled [ASSUMPTION]. Nothing here weakens, reinterprets, or
pre-ratifies anything in DECISIONS.md.

## 1. The mandate in engineering terms

Three standing constraints shape every section:

1. **Determinism is the law, not a feature** (E-03 working hypothesis, made
   binding for the engine's own surfaces by the repo's discipline — every
   Era-2 gate is byte-exact). Same campaign seed + same inputs ⇒ same NPC
   behavior stream, byte-identical, cross-platform. The spine already
   guarantees this for event execution ([PROJECT EVIDENCE] — the
   double-run fingerprint tests; `--sim` headless determinism). The NPC-AI
   layers must not break it: **per-NPC RNG streams** (SIM-RESEARCH §7's
   classic divergence bug — a global RNG whose consumption order depends on
   iteration order), **fixed evaluation order**, **fixed operation order**,
   and the **libm-free discipline** on any byte-packed path (the
   `proxy_instances.rs`/`animation.rs` pattern).
2. **RCT-class efficiency** (V-02): cost scales with *meaningful change*,
   not entity count × frame rate (SIM-RESEARCH §1). A dormant NPC costs
   storage; a far NPC costs its events; a near NPC costs its batch share.
   The spine's three-phase near/far budget with the guaranteed near
   reservation ([PROJECT EVIDENCE] T-14, measured anti-starvation) is the
   substrate this layering rides on.
3. **Scalability to 1000×** (the mission's clause): the design must hold at
   hundreds of persistent NPCs *now* (the V-03 mandate) and at
   Experiment B's 1,000-agent benchmark scale — with the same architecture,
   different constants. Every data structure below is either O(1) per NPC
   per event, O(log n) in queue size, or explicitly measured.

## 2. The substrate — what is already built (all [PROJECT EVIDENCE], cited)

The design composes built machinery; it invents none of it. (API names and
line citations verified against source in the design pass's verification
appendix §10.)

- **The event spine** (`engine-core`): `(due, seq)` priority with the
  ratified in-class ordering semantics (DECISIONS §3.2 clause 3);
  cross-class causal `depends_on` blocking (park-once, T-18's measured
  ~1 ns/blocked-event/tick bookkeeping); the three-phase near/far per-tick
  budget (near reservation a guaranteed allocation, not a ceiling);
  lane-inheriting follow-ups; the execution ledger with first-class
  lateness; the safe `processed_through` watermark; FNV-1a fingerprints
  over versioned domains.
- **The LOD machinery** (`engine-core` `lod.rs` + the suspension set):
  per-boundary hysteresis, squared-distance deterministic decisions,
  velocity lookahead (receding, never counted), multi-presence
  nearest-wins; identity-preserving suspension (`suspend`/`resume` over a
  held set retaining EventId/due/lane/payload/dependencies; conservation
  `scheduled = executed + queued + blocked + held`; `schedule_held` for
  dormant-targeted work; `reclassify_matching` for lane migration;
  `follow_up_in_field` under current region policy) — all T-15-accepted
  with equivalence and boundary tests.
- **The region lifecycle** (`engine-slice` RegionDirector + the PP-B4
  integration): presence-driven tick-boundary transitions; dormant regions
  cost one distance computation per tick; **capture-before-discard** with
  an exactly-once idempotency-keyed ledger (the in-memory Stage-4 form
  hardened through the Stage-5 B3/B4 machinery into the real
  `engine-persist` CommitPipeline — pending/captured/confirmed
  distinctions, canonical confirmation, the two-contract recovery).
- **The campaign workload**: 1,000 stable-identity NPCs across 8 regions as
  application data; the sustained-load methodology (F-08/F-12: six workload
  classes, twelve measurement dimensions, the honest effective matrix); the
  measured sustained-load execution cost (p50 0.51 ms/window after the
  zero-allocation main path).
- **The animation + population substrate** (PR #33, awaiting GO): the pure
  clip model (keyframe times, joint turns, fixed-order lerp, the
  column-major chain composition), `skin_vertex`, the per-frame pose
  evaluation (`evaluate_population_poses` — the integer stride reduction
  closing the walk loop bit-exactly), the placement table with its phase
  column, the skinned instanced draw path with the pose storage buffer at
  zero per-frame allocation, and the shadowed populated path (casting and
  receiving, the per-instance bounds culling measured at ≈8 skips/texel).
- **The S-experiment evidence base** (E-07–E-10, the sim-lod-local
  harnesses — protocol-logic evidence, not production claims): the
  replay-vs-synthesis crossover in *event work* (~7.6–12.4 pending
  events/NPC, density-dependent); time-skip cost tracking events, not
  elapsed time (7 d full-world skip 87.4 ms / 27,218 events vs same-elapsed
  quiet 13.1 ms / 3,380); schedule synchrony clustering starts to 80/tick
  and fixed-time shifts spiking ~200/tick vs a 2.7/tick average (budgets
  sized to cluster peaks); queue behavior under the lead's corrected
  acceptance (measured predictable growth under sustained overload, never
  silent drops).

## 3. The NPC record — the state contract

SIM-RESEARCH §5's conceptual facet table, mapped onto the engine's real
state lanes. The **rule of thumb is already ratified shape** (M-29/M-30's
two-lane model as read by SIM-RESEARCH §5): *state that shapes future
behavior, dialogue, or world consequences persists through the commit
pipeline; state that only shapes the next few seconds of motion is
reconstructed.*

| Facet | Contents | Lane in the built engine | Class |
|---|---|---|---|
| **Identity** | Stable NPC ID (never reused — permanent deaths, M-13/G-08), home, region membership, appearance/animation seed | The campaign workload's stable identities + region tags (§10.1) | Engine: the identity machinery. Policy: who exists. |
| **Schedule** | Recurring + one-off commitments (see §6) | New — the schedule table (§6.1), pure data, wire-encodable | Engine: the table machinery + wake events. Policy: the content. |
| **Location** | Last committed position + facing | Extended — the placement table's position becomes the *committed* location; in-region micro-position is reconstructible | Engine: the commit boundary. |
| **Routine state** | Current activity, needs counters, route progress | New — the record's per-NPC block in the active region's representation (capture-before-discard applies) | Engine: the block schema. Policy: needs/decay rates. |
| **Episodic memory** | Witnessed durable-significant events (§5.3) | New — captured deltas through the existing exactly-once ledger (§10.1: the capture-ledger lanes; §10.2) | Engine: the capture mechanism + retention machinery. Policy: classes, depth (§9). |
| **Relationships** | Typed weighted edges to NPCs/factions/players | New — durable deltas, same lane as memory | Engine: the delta schema. Policy: edge types. |
| **Personal state** | Health, possessions, economy position | Future — deliberately NOT designed here (encounter/transaction lane, M-30) | Deferred. |
| **Encounter micro-state** | Targets, cooldowns, animation phase | Existing pattern: provisional/temporary, never persists (SIM-RESEARCH §5; the M-30 lane) | Deferred with it. |

**The record's representation discipline** (engine primitive, following
the wire.rs/SaveWorld precedent — §10.1's spine-round-trip discipline):
every durable facet is a
*versioned, wire-encodable, fingerprintable* domain — the spine-round-trip
discipline. A record that cannot save/load byte-exactly through the
existing canonical encoding does not ship. The record's reconstructible
facets (routine state, micro-position) are derived on activation by the
existing replay/synthesis machinery — never persisted, never faked as
durable.

## 4. Behavior — the decision layer

### 4.1 The selector is a pure function (engine primitive)

An NPC's *activity* — what it is doing and why — is selected by:

```text
select_activity(record, world_view, rng_stream) -> Activity
```

evaluated **only at event boundaries** (activity starts, activity ends,
commitment arrives, perception fires), never per frame. This is the
`proxy_instances.rs`/`animation.rs` pattern applied to decisions: the
selector is pure data transformation — same inputs, same activity,
byte-identical, cross-platform — fully tested without Vulkan, I/O, or
wall clocks. The selector's inputs:

- **record** — the §3 facets (schedule, needs, memory, relationships);
- **world_view** — a *read snapshot* of the queryable world state the
  policy grants the NPC (time of day, weather class, region occupancy,
  the perception memory — never raw spine internals);
- **rng_stream** — the NPC's own stream (§8.3), consumed in fixed order.

**[HYPOTHESIS]** The composition: an ordered rule table (policy-authored
data, like the clip table) ranked by need state and schedule pressure,
tie-broken by the rng stream. This is the schedule-driven class of game-AI
selection [ESTABLISHED-CLASS] (Radiant-AI-class schedule systems,
The-Sims-class time-budgeted agents — SIM-RESEARCH §12's citations,
class-level only) — *composed deterministically*, which is our addition and
the part E-03 still owns as hypothesis until the first implementation's
gates pin it.

**Boundary:** the engine ships the selector *machinery* — the pure
evaluation loop over an ordered rule table, the need counters' arithmetic,
the digest pinning — and the game ships the rule table content. No
cyberpunk-specific behavior lives in engine code.

### 4.2 Activities are spine events (engine primitive)

The selected activity is *executed* as the existing chained-event pattern
(the campaign workload's routine chains (§10.1)): the activity's start
event runs, its follow-ups inherit the cause's lane (T-14's lane-inheriting
follow-ups — a near-field activity's continuation stays near; a far-field
activity's stays far), and the chain's next link is scheduled at the
activity's own cadence:

| LOD tier | Activity progression | Cadence | Cost shape |
|---|---|---|---|
| **Full** (players present) | Per-tick activity steps: locomotion integration, micro-behavior (glances, waiting shuffle), interaction windows | every near tick, inside the guaranteed reservation | per present-NPC per tick |
| **Simplified** (near region, unobserved detail) | Trajectory commitments: the activity's *route* progresses between commitment points only | at commitment boundaries (arrival, waypoint) | per commitment, ~1/10–1/100 of full [ASSUMPTION — S1 measures] |
| **Event** (far field) | Calendar only: the activity exists as its next wake-up | on queue | per event (I2) |
| **Dormant** | Record + `next_event` timestamp in the suspension set | never | storage only |

The tier transition is the **existing** suspension/reclassify machinery
(T-15): activation toward full runs capture-then-replay/synthesize through
the RegionDirector; deactivation toward dormant captures durable deltas
first (I1). No new transition machinery is designed — §4's only new moving
part is *what the activity events do when they run*.

### 4.3 Locomotion and routes (the near-field batch lane)

Movement is a **near-lane batch concern**, not a per-NPC event storm: the
full-tier locomotion integration runs as the data-oriented batch step over
all present NPCs in a region (SIM-RESEARCH §3's data-oriented layer —
cache-friendly, component-owned). Routes are **pure data** — waypoint
sequences with authored widths, crossing semantics, and hand-spread
variability seeds (the placement table's phase-column pattern extended to
paths: the crowd does not march in lockstep *or* file in lockstep).

- **Route progress** is the reconstructible facet: on deactivation the
  committed position stamps; on activation the replay/synthesis derives
  where along the route the NPC now is (the E-09 crossover governs which).
- **Walk-vs-idle clip selection** is derived from the locomotion state
  (speed band → clip id; the animation core's clip model needs no new
  machinery — the (clip, phase) pair is already the instance contract
  (§10.3)).
- **Collision** composes the player prototype's `CollisionWorld` query
  contract (§10.1: player.rs CollisionWorld) — NPCs are kinematic movers against the same world
  the player collides with, at the near tier only. Far tiers do not
  collide (their trajectories are commitments, not physics).
- **The position bridge**: today's sim-side location is a discrete
  `(u8, u8)` cell on the 16×16 region grid (§10.1) while the render
  side reads continuous world-space transforms. The behavior layer
  closes that gap at the near tier — the locomotion state carries
  continuous f32 positions with fixed operation order (the animation
  core's arithmetic discipline) — and keeps the *committed* location
  coarse (the cell or waypoint anchor), so far-field/dormant state
  never pays continuous precision and the render proxies' existing
  cell→world mapping remains the far-tier fallback.

**[ASSUMPTION]** near-field locomotion at the corner scale (24 NPCs) costs
microseconds per tick; at Experiment-B scale (1,000 near NPCs — a
pathological all-present case) it is the batch step's measured p99 that
gates, not the average (the mission's p99 clause). The implementation
package's measurement plan owns the number (§8.5).

### 4.4 Pathfinding — the contract, and the measured trigger for the decision

The corner scene and the first game's authored districts need **route
following**, not pathfinding: authored waypoint graphs suffice for
schedule-driven movement (go to work, walk the district, cross the street).
**[OPEN]** — *reactive* pathfinding (around a blocked street, a fight, a
player's vehicle) is a separate decision with a measured trigger: design it
only when a workload demonstrates the need (the mission's
no-speculative-abstraction rule). When that trigger fires, the decision is
the owner's (T-06's deferral discipline — the physics decision doc is the
queue's next item; pathfinding joins that doc's decision class or gets its
own pass). The engine contract meanwhile: **routes are data the behavior
layer consumes; whatever solves routes later must remain a pure function
over the same world view** — no exception, or determinism breaks.

## 5. Perception — the sensing layer

### 5.1 The inverted query (engine primitive)

Perception is **not** an NPC-side scan (N NPCs × M events per tick is the
RCT sin). It is an **event-side witness enumeration**: when a
*durable-significant* event commits (a theft witnessed, a public
altercation, a killable-NPC death — the classes are policy, §5.2), the
event's commit step enumerates its witness set — the NPCs within the
event's perception radius — and writes each witness's memory record.
Cost: O(active NPCs in radius) per significant event, zero for the NPC
population at large. **[ESTABLISHED-CLASS]** (event-notifying interest
management) **[HYPOTHESIS]** (the composition with the capture ledger,
below).

**The witness query's index is a NEW engine primitive — and its shape is
a measured decision, not a default.** No spatial partition of any kind
exists in the simulation substrate today (§10.2 — `CollisionWorld`
iterates linearly; the raycaster's per-instance bounds cull is
render-side). The measured-trigger discipline applies verbatim (the
Era-2 shadow mission's own precedent — "a BVH only if measurements
demand it"; none did): at the corner scale (24 NPCs) a linear census
per significant event is exact and cheap; at Experiment-B scale the
census cost is the gate's to measure BEFORE any hierarchy is built.
The design constraint that holds at every scale: **iteration is
inverted** (events → witnesses, never NPCs × events) and **ordered**
(identity order, §8.3) — whatever index later accelerates it must
preserve both or determinism breaks.

Tier gating falls out of the LOD machinery: a dormant/far NPC is not in
any active region's witness census, so it witnesses nothing — exactly
SIM-RESEARCH §4's "far agents don't perceive" — and catch-up on activation
recovers only what was *represented* (the replay covers committed events;
the far NPC's memory gains what its region's committed history recorded,
never invented micro-events — I3/no-time-travel).

### 5.2 Perception classes are policy; the budget is engine

The engine ships: the witness-enumeration mechanism, the memory-record
schema (a captured delta with the event's idempotency key), the retention
machinery (bounded per-NPC memory depth, oldest-first expiry — the depth
itself [OPEN] §9), and the **perception budget** — significant-event
witness enumeration counts against the event's lane budget like any other
execution, so a perception storm (a riot) degrades to *deferred* far-field
work, never dropped consequences (E-10's corrected acceptance discipline).
The game ships: which event classes are durable-significant, the radii per
class, line-of-sight rules (a first-person raycast at the near tier
[ASSUMPTION — cost measured in the implementation package], no raycast at
far tiers), and which memory classes are *reputation-relevant* — M-08's
single shared campaign reputation is fed by classified candidates only,
and the classification is policy **[OPEN]**.

### 5.3 Durable traces (the Mara example, on the built lane)

A witnessed durable-significant event writes, through the existing
capture-before-discard/exactly-once ledger (§10.1: the capture-ledger lanes; §10.2): (a) the witness's
episodic-memory entry; (b) optional relationship deltas (witness → actor);
(c) a *reputation candidate* — pending until classified, never itself the
shared score. All three survive LOD deactivation (I1 — the ledger's local
record, pending-live), ride the commit pipeline to canonical confirmation
asynchronously, and never block a transition. The S4-lite harness
demonstrated exactly this lane shape in protocol logic (E-08); the
production lane is PP-B4's real pipeline.

## 6. Schedules — the calendar layer

### 6.1 Schedules are pure data (engine primitive)

The schedule table follows the clip model's discipline: **authored data,
versioned, wire-encodable, deterministic** — no code, no clocks, no I/O at
authoring time. Per NPC: a recurring day-pattern (commitment list with
time-of-day windows, location anchors, route bindings, variability seed)
plus committed one-offs (the durable lane). The existing seeded-schedules
pattern in the campaign workload (§10.1) is the seed of this table;
the placement table's phase-column precedent is its rendering-side sibling.

### 6.2 Commitments execute as spine events (engine primitive)

A commitment's wake-up is a spine event in the NPC's region lane — the
existing schedule machinery, no new spine surface: the event fires, the
selector (§4.1) turns the commitment into an activity, the activity's
follow-up chain (§4.2) runs it. **Synchrony is the measured hazard**
(E-10's finding: schedule synchrony clusters 80 starts/tick and fixed-time
shifts spike ~200/tick vs a 2.7/tick average — budgets sized to cluster
peaks): authored schedules carry hand-spread jitter (the phase-column
pattern — per-NPC variability seeds dispersing the windows), and the
near-lane reservation absorbs the residual clusters.

### 6.3 Time-skip is queue fast-forward; gates hold (consuming ratified policy)

M-34's unanimous time-skip is a **fast-forward of the event queue** — cost
proportional to scheduled events, not elapsed time (S4-lite measured:
7 d full-world 87.4 ms / 27,218 events vs same-elapsed quiet 13.1 ms /
3,380 — E-08). The four-lane classification (story-gate holds, voting
lane, ordinary routines run, emergent-irreversible holds city-side during
frontier-only epochs — M-07/M-29) is **not built**; this pass restates it
as the implementation constraint it already is in SIM-RESEARCH §6: the
schedule layer must classify commitments by lane *as data* at authoring
time, so the future Gate Keeper has no runtime classification to invent.
**[HYPOTHESIS]** the composition; the S4-lite harness is the evidence; the
production gate design belongs to the session/replication line
(CYCLE6-6D), consumed not redesigned here.

## 7. The rendering contract — from behavior state to walking pixels

The behavior layer's presentation surface is exactly PR #33's instance
contract, extended from static placement to *derived* placement:

- **The instance record** stays the packed (transform, params) pair; the
  pose-index claim on the params slot is unchanged (§10.3). What
  changes is the *source* of the transform: the demo's per-frame
  evaluation reads the behavior state (locomotion position, facing) and
  the (clip, phase) pair instead of the static placement + phase.
- **Presentation never enters the spine** (the free-look precedent): the
  per-frame pose evaluation remains a demo-layer reader of committed
  behavior state; no fingerprint, gate, or measured claim depends on
  render-time sampling.
- **The moving-along-paths crowd** (PR #33's documented follow-up) is this
  contract's first implementation rung: the corner NPCs walk their routes
  through the intersection — crosswalks, sidewalk loops, the waiters'
  stand-and-shift — composing with sky/day-night/post/wet/shadows
  unchanged.
- **Scale [ASSUMPTION → the implementation package's gates]**: per-kind
  instance capacity, pose-buffer capacity (704 B/pose × population), and
  the N-vs-1/indirect equalities must re-pin at the populated scale the
  behavior layer drives (the corner's 24+2 first, Experiment-B's 1,000
  far/≈dozens-near shape next). The 1000× clause: the *architecture* (one
  draw per kind, pose dedup by (clip, phase) block) holds; the *constants*
  (buffer sizes) are the measured re-pins, never silent growth.

## 8. Composition constraints — determinism, replication, verification

### 8.1 What the spine's ordering semantics mean for behavior (engine primitive)

The ratified §3.2 clause-3 semantics: in-class `(due, seq)` priority, with
causal blocking and late arrival as the two recorded deviation classes.
For the behavior layer this is a *feature*: (a) two NPCs' activity
selectors never race — their events are ordered by the queue's key;
(b) a perception-triggered follow-up *must* use the existing
`depends_on` causality when its effect requires the triggering event's
commit (never chronological inference — the registry's own warning);
(c) cross-class execution order stays unspecified except where
dependencies impose it, so the behavior layer's tests must pin outcomes
through dependencies, not observed timing.

### 8.2 Replication boundaries (consumed from CYCLE6-6D)

The session-replication boundary design (unratified, CYCLE6-6D) requires:
sim placement owned by the host's spine, input ordering at the spine
boundary, replication scope by interest, divergence detection by
fingerprint. The NPC-AI surface composes by construction: behavior state
is deterministic function output over committed inputs (so guests may
replay or receive deltas — the boundary doc's option space), perception's
durable lane is captured deltas (so replication scope covers memory the
same way it covers any durable key), and no behavior state lives outside
the record + the spine (so there is nothing hidden to replicate). **No
networking is designed here**; the constraint is recorded so the behavior
implementation cannot accidentally foreclose CYCLE6-6D's options.

### 8.3 Determinism details (engine primitive, the Era-2 discipline)

- **Per-NPC RNG streams**: seeded from (campaign seed, NPC identity) at
  activation, consumed in fixed order by the selector; stream state is
  reconstructible (replayed on activation like routine state), never a
  global RNG [ESTABLISHED-CLASS divergence hazard, SIM-RESEARCH §7].
- **Fixed evaluation order**: the batch step iterates present NPCs in
  identity order (BTreeMap-like), not pointer/allocator order.
- **The libm-free discipline** on every byte-packed path (need counters
  as integers, turn/angle math through the `sin_cos_turn` machinery,
  positions as f32 with fixed operation order — the animation core's
  precedent).
- **Digests pin the behavior stream**: a fixed campaign's activity
  sequence per NPC, and the population's pose stream, hash to FNV-1a
  byte digests (the walk-clip digest's pattern) — cross-platform,
  never re-pinned without a cause.

### 8.4 The verification strategy for the future packages (the gates, named now)

1. **The selector oracle**: known-answer tables (record+view+seed →
   activity) including tie-break and need-threshold edges; corrupted-input
   refusals (a rule table naming a nonexistent activity fails closed).
2. **The behavior digest**: a seeded campaign's activity/commitment
   stream pinned (the `--sim` double-run discipline).
3. **The witness oracle**: hand-computed witness sets for authored event
   geometry (radius, tier gating, the dormant-NPC-witnesses-nothing
   invariant I2/I3).
4. **The capture-lane gates**: memory deltas exactly-once across
   deactivate/reactivate cycles (the S6 fuzz discipline, on the real
   pipeline); reputation candidates pending-until-classified.
5. **The schedule gates**: skip-cost ∝ events (not elapsed); held lanes
   never firing (I5) with revalidate-then-present; synchrony clusters
   within the budgeted near reservation (E-10's corrected acceptance).
6. **The rendering equalities**: the moving crowd's N-vs-1/indirect
   byte-equality, allocation constancy (Vulkan AND host) across warmed
   *moving* frames, the pose digests under motion.
7. **The scale gate**: the sustained-load methodology re-run with the
   behavior workload classes (the F-08/F-12 matrix extended).

### 8.5 The measurement plan (what the implementation must measure, labeled)

Every number the future packages claim: p99 per-tick cost by tier
([ASSUMPTION] full-tier locomotion µs-scale at corner scale; the batch
step's p99 at 1,000-near pathological), witness enumeration cost per
significant event, activation latency with schedule catch-up (E-09's
crossover re-measured on the real machinery), skip cost curves (E-08's
shape on the real queue), memory-per-NPC by tier, and the allocation
counters (both) across warmed regimes. Rig-dependent claims stay
rig-pending; lavapipe/CI is the nightly evidence.

## 9. Open questions for the owner (this pass's decision list)

1. **Plausibility vs exactness on activation** (SIM-RESEARCH §11.1,
   restated): the E-09 crossover (~7.6–12.4 pending events/NPC) is
   evidence, not policy — does the owner pin a launch policy (adaptive,
   labeled) or leave it configured per campaign?
2. **Memory depth and reputation classes** (§5.2, SIM-RESEARCH §11.3):
   per-NPC episodic retention depth; which memory classes feed M-08's
   shared reputation.
3. **Far-field fidelity** (SIM-RESEARCH §11.2): the corner/first-district
   scale keeps per-NPC events far-field; the horizon-scale question
   (statistical city models from altitude) is a product ambition call.
4. **Perception geometry** (§5.2): radii per class, line-of-sight at the
   near tier (raycast cost measured in implementation), the
   first-person/hearing/behind distinctions — policy content the owner
   ratifies as game rules.
5. **Schedule authoring surface** (§6.1): hand-authored tables vs an
   editor tool later (E-05's functional-editor-first preference applies;
   no tool is designed here).
6. **The pathfinding trigger** (§4.4): the measured workload that would
   justify reactive pathfinding, and whether its decision joins the
   physics decision doc (T-06 class) or gets its own pass.
7. **Needs-vs-schedules balance** (§4.1): the first game's crowd is
   schedule-driven with needs as modifiers (the research program's
   Radiant-AI-class shape); a needs-primary (Sims-class) crowd is a
   different game — owner's call on ambition for game one.

## 10. Verification appendix — source citations for every §2 claim

*Every identifier and line below was verified against source by this
pass's inventory (two independent read-only passes; `origin/main` @
`fc6bce6` for the engine-core/slice substrate, branch
`era2/animation-skinning-proxy-shadows` @ `d69be6a` for the Era-2
machinery).*

### 10.1 The spine and the existing NPC workload

- **The spine contract** (`engine-core/src/spine.rs:1-4`): "canonical
  campaign clock, scheduling with deterministic `(due, seq)` queue
  priority, cross-class causal dependencies, per-tick budget with the
  near/far lane policy, the execution ledger, and the safe
  processed-through watermark."
- **The game-side handler surface** — what an NPC-behavior world
  implements: `World::execute(&mut self, event: &Event, ctx: &mut
  ExecCtx)` + `World::fingerprint` (`spine.rs:40-48`); "game policy
  stays outside reusable engine mechanisms by *placement*"
  (`spine.rs:35-39`).
- **The execution context** (`spine.rs:51-150`):
  `ExecCtx::follow_up(delay, class, payload, depends_on)` —
  execution-relative, "an overdue cause shifts its effects honestly"
  (`:77-80`), lane-inheriting (`:59-63`);
  `ExecCtx::schedule_at(tick, …)` — fixed logical time, late arrivals
  counted (`:103-117`, `:766-777`);
  `ExecCtx::follow_up_in_field(field, delay, …)` — the Stage-4
  explicit-lane override (`:127-145`).
- **Event data** (`engine-core/src/event.rs`): `Field::Near/Far`
  (`:17-21`, "the near lane's reservation is a guaranteed allocation,
  not a ceiling"); `EventClass(pub u16)` opaque (`:41`); `Schedule::
  AtLogicalTime/ExecutionRelative` (`:75-85`); `ExecutionRecord` with
  first-class `late` (`:175-184`).
- **Budget**: `Spine::with_reservation(world, budget, near_reserve)`
  (`spine.rs:465`); the three-phase `run_tick` (`:1242-1283`) — near
  up to the reservation, far up to the nonreserved capacity,
  remainder redistributed near-first; exhaustion counts
  `budget_exhausted_ticks`, nothing dropped (`:1257-1265`); blocked
  events park without consuming budget (`:1763-1809`; T-18's measured
  ~1 ns/blocked-event/tick bookkeeping).
- **Suspension (the LOD machinery)**: `schedule_held` (`:809-844`),
  `collect_matching` (`:861-886`), `suspend_matching` (`:907-949`),
  `reconstruct_held` (`:975-1054`), `resume_matching` (`:1097-1122`),
  `reclassify_matching` (`:1164-1200`); conservation `scheduled =
  executed + queued + blocked + held` (T-15's accepted record).
- **The clock** (`engine-core/src/clock.rs:7-15`): "The spine's tick
  loop is the only component that advances campaign time; wall-clock/
  OS time never enters simulation logic"; `Tick(pub u64)` with no
  intrinsic duration — the rate is application policy (the demo's 10
  Hz is "a demo default, not an engine-wide rate", `sim.rs:14-19`).
- **The NPC workload today** (`engine-slice/src/npc.rs:1-11`): "1,000
  stable NPC identities, each with a chained activity schedule: every
  execution advances that NPC's logical activity step and schedules
  the next activity as an execution-relative follow-up (which
  inherits the executing event's near/far lane)". `NpcWorld`'s dense
  per-NPC vectors: `steps`, `routine_cursor`, `positions: (u8, u8)`
  on a 16×16 region-local grid, `represented_through`, `milestones`
  (every 16th step — "irreversible consequences — never fabricated by
  reconstruction", `:225-228`), `pending_signals`/`credits` (the
  scratch-vs-ledger lanes, `:229-242`). `routine_payload_parts`
  (`:160-169`) decodes `magic(16)|npc(16)|cursor(32)`;
  `advance_position` (`:492-505`) is a seeded ±1-cell random walk —
  **not goal-directed locomotion**; `replay_routine_span`
  (`:410-436`) vs `synthesize_routine_span` (`:448-477`, closed-form
  O(1)); `mix64` (`:187-192`) — "the deterministic 64-bit mixer
  (SplitMix64's finalizer)".
- **The campaign**: `NpcConfig` defaults seed 1337, `activity_period`
  6, `simplify_factor` 4, `commitment_window` 12 (`npc.rs:81-96`);
  `cadence_factor` by LOD (`:104-110`); `lod_field` — Full/Simplified
  → Near, Event/Dormant → Far (`:116-121`); the 8-region ring of
  exact-rational centers (`region.rs:341-350`, "no trigonometry");
  the sustained-load harness's six workload classes and twelve
  measurement dimensions (`sustained_load.rs:332-566`), conservation
  asserted per cell (`:943-948`), double-run determinism
  (`run_cell_deterministic`, `:1005-1011`).
- **The player prototype** (`engine-core/src/player.rs`):
  `CollisionWorld` — `box_overlaps` (`:282`), `segment_hit`
  (the ray–AABB slab method, "no sampling, so no aliasing", `:298`),
  `sweep_axis` ("the box stops before crossing", `:355`),
  `depenetration` (`:428`); the fixed Y→X→Z swept order
  (`step_player`, `:891-953`); `PlayerDriver::advance_observed` —
  "the fixed-step composition seam … gameplay logic that must
  observe each logical tick's own resulting state interleaves HERE"
  (`:1140-1142`) — the natural perception-tick hook. The only
  perception-shaped primitive today: `interaction::select_target`
  (nearest-in-cone, O(n) over the caller's slice, `interaction.rs:
  57-69`) — a player-facing selection query, **not an NPC perception
  system**.
- **The determinism recipe** (a future NPC-AI gate's mechanism):
  `Fingerprinter` FNV-1a 64 (`fingerprint.rs:24-85`, "never feed data
  that derives from unordered iteration"); versioned domains —
  `"engine-slice/npc-world/1"`, `"engine-slice/npc-spatial/3"`,
  `"engine-slice/stage4-state/1"`, `"engine-slice/capture-ledger/2"`
  — "new versioned domain tag, never mutate a frozen one"
  (`npc.rs:688-692`); frozen hex pins with "a divergence is an
  investigation, never a silent re-pin" (`tests/player_movement.rs:
  113-115`); the soak scenario's per-tick purity ("a pure function of
  the scenario version, the seed, and the LOGICAL TICK NUMBER … never
  per rendered frame", `soak_scenario.rs:1-48`).
- **The wake mechanisms — exactly three exist**: `Schedule::
  AtLogicalTime` (the lane heaps until due), execution-relative
  follow-ups, and region dormancy through the held set (the
  RegionDirector's tick-boundary `apply(presence)`, `region.rs:
  939-951`). **No wall-clock timer, no sleep_until, no priority
  beyond `(due, seq)` + dependencies + the two lanes.**

### 10.2 What does NOT exist (verified by exhaustive search — the design's new-primitive list)

No navigation mesh, pathfinding, or steering anywhere in `crates/`;
no continuous NPC world positions (the `(u8, u8)` grid cell is the
whole location state) and no NPC-vs-geometry collision; **no
perception system** (nothing beyond the player's interaction cone);
**no spatial partition of any kind for simulation queries** (no BVH,
quadtree, k-d tree, or spatial hash — `CollisionWorld` iterates
linearly); **no schedule/calendar layer** (the routine is an abstract
counter with seeded jitter — no time-of-day, no goals, no
utility/behavior-tree/GOAP/FSM machinery, no data-driven behavior
format; "a future scripting layer" is explicitly deferred,
`spine.rs:38-39`); no NPC identity beyond the stable index (no names,
roles, appearance, relationships, or memory in sim state); no
per-NPC priority within a lane ("urgency must be expressed through
due times, lanes, or the near reservation"); no animation or
locomotion model in the sim (the render-side corner population is a
fixed hand-authored table, "PURE DATA, FIXED TABLE (not seeded)",
unrelated to the spine NPCs).

### 10.3 The Era-2 animation + rendering substrate (branch, PR #33)

- **The pose pipeline** (pure, no Vulkan, no new dependencies —
  `animation.rs:1-6`): `sample_pose(clip, t) -> [JointPose; 11]`
  (`:680`, fixed-order lerp; turns as signed i16 fractions of 1/65536
  rotation) → `compose_joint_matrices(skeleton, pose)` (`:372`,
  column-major, parents always lower-indexed) → `skin_matrices`
  (`:420`, `M_j = J_j · translate(−bind_world_j)`, the rest pose
  bit-exact identity) → one 704-byte block per NPC (`POSE_STRIDE`,
  `:65-66`); `skin_vertex` (`:492-570`, up to 4 influences, the
  shipped mesh rigidly bound); `repack_poses` into a retained buffer
  with the typed capacity refusal (`:641-664`); `pose_sequence_digest`
  (`:777-800`, the walk clip at 64 samples pinned to
  `2_694_074_108_253_436_588`); `posed_box_bounds` (`:573`, the
  shadow-culling input).
- **The clip model**: exactly ONE clip exists (`walk_clip`, `:840`) —
  8 keyframes of pure static data at the 45° marks of a 1.1 s stride;
  66 frames at the 60 Hz reference; per-NPC phase offsets as
  `Phase::Walk(u16)` turn-fractions of the stride
  (`corner_population.rs:160-195`); the frame counter reduces modulo
  66 IN INTEGERS before any float forms (the bit-exact loop closure,
  `corner_population.rs:242-253`). **No clip blending, no
  locomotion-driven clip selection, no per-NPC clip choice** — every
  walker plays the same clip at a phase offset.
- **The population flow**: `NPC_TABLE: [ProxyInstance; 24]` —
  positions and facings are table constants; only poses animate
  (`corner_population.rs:74`); `evaluate_population_poses(k, phases,
  out)` writes one block per entry per presented frame, in place,
  zero allocation after the first frame (`:235`); the renderer
  repacks and copies into the slot's mapped pose region after the
  fence wait (`skinned_draw.rs:477-490`).
- **The capacities (hard limits, typed refusals)**:
  `MAX_INSTANCES = 4096` total across kinds per frame
  (`proxy_instances.rs:33` — "Revisitable only with rig evidence,
  never silently"); `MAX_POSE_BLOCKS = 4096` (`animation.rs:238`);
  `INSTANCE_STRIDE = 96` bytes with the pose-index claim on the
  params vec4's x (`proxy_instances.rs:183-189`); the instance
  buffer's per-slot carve (`proxy_draw.rs:341`); the populated render
  path has never been driven above 26 instances.
- **The shadowed populated path**: `Frame3` with the exact transpose
  inverse (`shadows.rs:341-412`); `NpcOccluder`'s world AABB computed
  once per frame, never per ray (`:412-464`);
  `map_texel_depth_populated` with the bounds cull FIRST
  (`:740-776`); the cull-safety pin (`:1536-1604`); the measured ≈8
  skips per texel (the census, `:1756-1923`); the single 1024²
  cascade ≈ 0.110 m/texel — a 0.42 m figure spans ≈4 texels (the npc
  class a presence proof). **No BVH — measurements did not demand
  it.**
- **The 6-6D session-replication constraints** (unratified, consumed
  as constraints): sim placement S3 (host-authoritative spine, "no
  client-side sim authority"; `engine-core` "never learns that
  networks exist"); input ordering player-id-major then sequence "by
  rule not arrival"; replication scope by interest with **"AI
  micro-state beyond the LOD-relevant projection" never replicating**;
  divergence detection by fingerprint checkpoints (read, never
  influence); the §9 owner decisions D1 (sim placement), D2 (late-input
  policy), D4 (checkpoint cadence), D5 (absent-member avatar policy),
  D8 (mirror opt-in) all touch NPC simulation.
