# P2 — Simulation and Spatial Scalability: Design

**Author:** design author, package P2 (glm-5.3-w4a8 subagent)
**Status:** IN PROGRESS — query inventory verified against source; design
sections appended incrementally.
**Read base:** branch `refactor/engineering-excellence`, HEAD
`c7de072b0a90e73a70cd1d554797c1462df71efa` ("p0 measurement baseline (e-8)",
2026-11-26). All line numbers below cite this tree.
**Scope (charter §7 P2):** A05, A07–A10, A22(core), B01, B06, B12 — spatial
query semantics, scheduler accounting, spine region ops, witness-oriented
memory, ledger retention, world configuration/identity, staged lanes,
measurement plan.
**Constraints honored:** read-only for `crates/**`; this file is the only
write. No builds, no commits.

> The charter's hardest bar, quoted for every section that follows:
> "Do not implement a universal index before identifying real differing
> query needs. … Retain simple small-workload fast path where faster. …
> don't replace the event spine wholesale on fashion."
> (DIRECTIVE §7, P2 paragraph, every sentence a requirement.)

---

## 0. The E-8 reality this design must respect

E-8 (rig CPU 9800X3D, release, tree `263532f`; EVIDENCE.md E-8) measured the
entity-scale battery: 600 ticks × {1k, 5k, 20k} × {full, mixed, event-driven},
**median tick wall flat at 0.019 / 0.010 / 0.002 ms per tick at ALL THREE
scales** (p99 up to 0.065). The sustained-load matrix shows why: D_base
executions per tick are 154.1 (1k full) → **200.0 (5k full, saturated)** —
the execution budget binds before the CPU does, at every scale measured.
Consequence, stated once and carried through every section: **the linear
scans are not today's bottleneck at the measured workloads**; they are the
*scaling hazard* (cost grows with population while the budget does not) and
the *galaxy-scale blocker* (a 1M-NPC world cannot census per event over a
dense array even at 2 µs per 20k walk — and the census population slice is
the caller's ACTIVE census, already tier-gated). This design therefore earns
every mechanism against (a) the scaling slope the E-8 flat line hides, (b)
the charter's stated future workloads (galaxy-scale, densely authored
cities), and (c) the 9.3 rubric's "no hidden fixed demo population" bar —
not against invented present-day hotspots. Where the evidence says the
current mechanism is already the right cost/benefit at its measured
workload, the recommendation is NO-CHANGE with the boundary stated (§1.7).

---

## 1. The query inventory (verified against source, cited)

*(populated incrementally; each entry: semantics, complexity, population
touched, call frequency, ordering contract, E-8 cost evidence)*

### 1.1 Q-A — Perception census (A07): `perceive` / `enumerate_witnesses`

- **Source:** `crates/engine-core/src/npc_perception.rs:80-114`
  (`PerceivedIndices::next` walk + `record_in_radius`),
  `:142-164` (`perceive` construction, fail-closed radius gate);
  `crates/engine-core/src/npc_execution.rs:299-320` (`enumerate_witnesses`
  composes `perceive` with the four-tier budget filter).
- **Semantics:** the INVERTED QUERY — which NPCs are within `radius` of
  `center`. Squared-distance f32, FIXED operation order (dx, dy, dx·dx+dy·dy),
  INCLUSIVE boundary (`≤`, pinned by the 3-4-5 known-answer test at
  npc_perception.rs:346-363). Fail-closed: negative/NaN/Inf/square-overflow
  radius perceives nobody (`:147-156`); non-finite positions compare false
  (`:109-114`). `enumerate_witnesses` excludes tiers whose
  `perception_budget` is 0 (Dormant) — npc_execution.rs:307-319.
- **Complexity today:** O(population-slice) per query, zero allocation (a
  lazily advanced cursor — npc_perception.rs:63-78); `enumerate_witnesses`
  writes indices into caller-owned scratch (`out.clear()` then push,
  npc_execution.rs:306).
- **Population touched:** the caller-presented census slice — in the slice
  composition this is the active-region population (the module doc,
  npc_perception.rs:22-34, states tier gating is the caller's composition:
  "a dormant/far NPC is not in any active region's census").
- **Frequency:** per SIGNIFICANT EVENT commit step (event-side, not
  NPC-side — npc_perception.rs:22-27), not per tick.
- **Ordering contract:** FIXED POPULATION ORDER (population-slice order,
  which the caller presents in identity order — §8.3 discipline). Pinned by
  `perceive_order_is_population_order_not_distance_order`
  (npc_perception.rs:371-395). Any index must preserve exactly this order.
- **E-8 evidence:** event-driven tick medians 0.002 ms at 1k→20k — census
  cost is inside noise at these scales; the flat line is the budget cap, not
  census cheapness. No per-census isolate was measured at 20k+ (gap §8).

### 1.2 Q-B — Locomotion near-lane depenetration: `advance_batch` step 7

- **Source:** `crates/engine-core/src/npc_locomotion.rs:601-797`;
  collision composition at `:764-797` (step 7).
- **Semantics:** per PRESENT NPC with speed > 0, one
  `CollisionWorld::depenetration` call over the NPC's box at the candidate
  position (npc_locomotion.rs:776). A push moves x/z only (ground-constrained),
  reverts the (leg, leg_distance) pair (the honest no-progress revert,
  `:779-791`), freezes stride phase. `collision: Option<&CollisionWorld>` —
  None is a valid no-collision mode (tests run it, e.g. npc_locomotion.rs:2289).
- **Complexity today:** O(collider count) per moving NPC per tick (see Q-E);
  the batch itself allocates nothing (`:554-560`).
- **Population touched:** near-tier (Full) PRESENT NPCs — 24–1000+ per tick
  in the current demo scale; the E-8 "full" rows are this lane.
- **Frequency:** PER TICK per moving NPC (the only per-tick-per-actor query
  in the inventory — everything else is per event or per input).
- **Ordering contract:** identity (index) order within the batch
  (`:618-620`); depenetration's own tie semantics = first strictly-smaller
  candidate wins, iteration order of the collider list (Q-E).
- **E-8 evidence:** full-lane tick medians 0.019 ms at 1k with D_base
  154 executions; 5k saturates the 200 budget at 0.019 ms — the lane cost
  is budget-flat today. Collider count per world in the demo is small
  (tens — see §1.7 for the exact census); the hazard is (NPCs × colliders)
  product growth.

### 1.3 Q-C — Player kinematics: `step_player` axis sweeps

- **Source:** `crates/engine-core/src/player.rs:824-955` — one
  `depenetration` (`:882`), then three `sweep_axis` calls (Y `:892`, X
  `:909`, Z `:933`), plus `clamp_to_bounds` (`:955`); the step-up path adds
  `box_overlaps` + 2 sweeps when `step_height > 0` (`:793-803`; default 0.0
  disables it, player.rs:152-156).
- **Semantics:** swept-AABB per axis with the SKIN-margin discipline
  (`sweep_axis` doc, player.rs:347-354: perpendicular axes use SKIN overlap
  margin; clamp targets sit SKIN short of each face — "tunneling impossible
  at any speed"). Tie-breaking: the *first* collider achieving the binding
  face wins (loop order = insertion order, axis-aligned list then rotated,
  player.rs:374-414).
- **Complexity today:** 4 linear collider-list scans per player tick (7 with
  step-up), each O(collider count) — plus rotated narrow phases.
- **Population touched / frequency:** the STATIC world (boxes + rotated
  prisms); per player tick (the fixed 60 Hz sim clock; 1 player).
- **Ordering contract:** tie-break by collider iteration order (see Q-E).
- **E-8 evidence:** no player-query isolate in the battery; the
  locomotion isolate brackets the same query family (Q-B). Total player
  cost is invisible in the 0.019 ms tick medians.

### 1.4 Q-D — Line of sight / camera / occlusion: `segment_hit`

- **Source:** `player.rs:298-345` (exact ray–AABB slab per collider,
  margin-expanded, smallest `t` in [0,1], `Some(0.0)` inside, ground and
  bounds NOT consulted); rotated arm `collision.rs:468-565`
  (`segment_rotated_hit`: local-frame slab, `t` invariant under the rigid
  transform, corner ties by axis order X→Y→Z — collision.rs:451-467).
- **Callers (production):** camera obstruction pull-in
  (`engine-slice/src/playable_demo.rs:454,507` — **per frame**;
  `encounter_frame.rs:333,349`), combat occlusion inside `resolve_attack`
  (`combat.rs:328`, per attack), adversary line-of-sight
  (`adversary.rs:565,607`).
- **Complexity today:** O(collider count) per segment; conservative
  broad-phase pretest already exists for the ROTATED arm only
  (segment-AABB vs margin-expanded world AABB, collision.rs:474-499 — the
  C4-R1 corrective; the axis-aligned arm has no pretest, it is the exact
  slab itself).
- **Frequency:** per frame (camera) + per attack (input edge).
- **Ordering contract:** smallest `t` wins; equal `t` resolves by
  iteration order (first list wins) — but note the AABB arm's tie is over
  *enter-times from different colliders*, practically distinct.
- **E-8 evidence:** none isolated; per-frame camera cost sits inside the
  render-path medians (region scene 0.022 ms/frame, E-8 log).

### 1.5 Q-E — Static-world membership/push: `box_overlaps`, `depenetration`

- **Source:** `player.rs:282-287` (strict overlap, SKIN contact clear);
  `player.rs:428-470` (minimal-axis push; **deterministic tie rule:
  "the first strictly-smaller candidate wins, so mathematically equal
  pushes resolve by iteration order"** — player.rs:421-427; rotated
  candidates after axis-aligned, "a mathematical tie keeps the
  axis-aligned push").
- **Population/frequency:** static world; `depenetration` per moving NPC
  per tick (Q-B) + once per player tick; `box_overlaps` in spawn/step-up
  validation.
- **E-8 evidence:** locomotion isolate — 1000 walkers with queries that
  MISS: p50 0.0088 ms/batch (0.0156 µs per NPC p99) vs **0.0038 ms with
  `collision: None`** (0.0078 µs per NPC) — the collision queries are
  ~half the near lane's cost at 1000 NPCs against the bench's small world;
  1000-blocked: 0.0141 ms (E-8 log, `--benchmark-locomotion`). The bench
  world is small (the pinned corner-blocked lane; colliders are few), so
  this is a per-NPC constant today — it scales as NPCs × colliders.

### 1.6 Q-F — Interaction selection (B06): `select_target`

- **Source:** `interaction.rs:69-144`. Nearest **surface distance**
  (extent-discounted) within reach and facing cone; ties break to the
  **lowest id** (`:133-138`); invalid query/eye/forward refuses (`None`);
  invalid targets skipped. Per target: one sqrt (distance, `:110`), one
  normalize (`:120-124`), dot, compare — the B06 sqrt+normalize note.
- **THE ZERO-DISTANCE TIE DEVIATION (B06's question, answered by
  source):** the `distance == 0.0` branch (`:111-118`) takes
  `(target.id, 0.0)` and displaces only on `candidate.1 < best_d` — **it
  omits the `target.id < best_id` tie clause the normal path has**
  (`:136`). Two targets at exact distance zero therefore resolve by
  SLICE ORDER, contradicting the module's own contract ("ties break to
  the lowest id … iteration order is the caller's slice",
  interaction.rs:11-13, and "a pure function of (targets, eye, forward,
  query)", `:11-12`). The zero-distance target also bypasses the cone
  check by design ("dead-ahead by definition", `:112-113`) — that part is
  documented intent; the tie omission is not.
- **Population/frequency:** caller's interaction-target inventory
  (playable demo: a handful); per interact **input edge**
  (playable_demo.rs:502), not per frame.
- **Complexity:** O(targets) with sqrt per target — trivial at demo scale;
  the B06 interest is contract + the shared-foundation question.
- **E-8 evidence:** none (human-frequency input path).

### 1.7 Q-G — Combat trace (B06): `resolve_attack`

- **Source:** `combat.rs:305-371`. One `segment_hit` wall query
  (`:328`, Q-D), then a linear scan of caller-supplied `Combatant`
  hitboxes (`:331-352`) via exact `segment_box_hit`; **nearest distance
  wins, equal distances break to the LOWEST id, the wall wins ties
  (target must be STRICTLY nearer)** — combat.rs:298-302, `:340`, `:346`.
  No sqrt in the hot compare (t-scaled distances).
- **Population/frequency:** the combatant slice (encounter demo: small);
  per attack input (encounter_frame.rs:147, combat_demo.rs:966,
  adversary.rs:660).
- **E-8 evidence:** none isolated; input-frequency path.

### 1.8 Q-H — Scheduler "queries" (B01/A08 context, not spatial)

- `drain_due` (spine.rs:1767-1809): heap peek/pop per due event;
  blocked parking does a `BTreeMap::insert` ×2 (blocked, waiter
  registrations) and an O(deps) unmet scan — **not counted against the
  execution cap** (B01, confirmed core.md).
- `execute_now` (spine.rs:1824-1913): O(1) ledger append + unfinished
  remove + handler + follow-up scheduling + `waiting_on.remove(&seq)` →
  `try_unblock` per waiter (synchronous fan-out).
- `try_unblock` (spine.rs:1953-1974): O(deps) revalidation per attempt.
- `suspend_from_queue`/`reclassify_queue` (spine.rs:1128-1149,
  1206-1227): full heap drain to a temp `Vec` + filter + re-push each —
  O(N log N) + allocation per region transition.
- `resume_matching` (spine.rs:1097-1122): full held scan + seq Vec +
  per-seq revalidation.
- `collect_matching` (spine.rs:861-886): iterate both heaps + blocked,
  filter, sort by (due, seq) — O(N log N) + alloc; called by region
  planning (region.rs), crossing (crossing.rs:1569), soak/sustained
  harnesses.
- **E-8 evidence:** the skip decomposition (E-8 log):
  ≈ 12.7 ns/tick + ≈ 0.163 µs/event; the event term dominates at
  ≥ 0.1 events/tick. The sustained matrix: aligned synchrony drives
  late executions and budget-exhausted ticks (1072–2400 per window) —
  the budget machinery is exercised, and D_base saturates at 200 at 5k.

### 1.9 Q-I — NPC memory witness access (A09)

- **Source:** `npc_memory.rs:675-685` (`witness_records`: full scan of
  BOTH maps + filter + `sort_unstable_by_key` — O(ledger) + O(result log
  result) + alloc, **per call**); `:691-697` (`pending_keys` calls
  `witness_records` then re-filters); `:701-713` (`witness_counts`: two
  full scans); `:911-917` (`witness_classifications`: full
  classifications scan); `:869-905` (`activate`: rebuild folds
  `witness_records` — O(ledger) per witness activation).
- **THE KEY STRUCTURAL FACT (correcting a tempting shortcut):**
  `MemoryKey` derives `Ord` over **(event, witness, kind)**
  (npc_memory.rs:89-99) — the canonical order is EVENT-FIRST (the
  charter's phrase "canonical event-first wire order"). A witness's
  records are therefore NOT a contiguous range of the pending/confirmed
  `BTreeMap`s; a range query cannot replace the scan. The needed
  structure is a derived witness→keys index (§5).
- **Population/frequency:** the whole two-phase ledger (grows with
  captured history; unbounded — A22's memory half); per witness
  activation (region activation batches), per journal flush claim
  (`pending_keys`), per conservation check.
- **E-8 evidence:** no memory-access isolate; the E-8 memory table gives
  resident sizes (Full 840 B/NPC, Dormant record-only 20 B/NPC at 2
  entries; 1288/764/644/20 at 16 entries) — the LEDGER is the growth
  term the table does not capture.

### 1.10 The census composition today (who presents what to Q-A)

- `npc_memory_lane.rs` (the demo's memory lane): census population is a
  **const 24-record table** (`MEMORY_POPULATION = 24`, :70; `POPULATION`,
  :149), 4 regions × 6 NPCs, tiers from region levels (`:373-376`), one
  `enumerate_witnesses` per ENCOUNTER_CLASS event (`:380`).
- `corner_behavior.rs`/`corner_demo.rs`: the 1000-NPC world composes
  locomotion + schedules; the E-8 behavior harness measured census
  populations 24/125/1000 directly (`behavior_measurement.rs:533-591`).
- So the census' *measured* per-event cost today: **0.000 µs (24) /
  0.100 µs (125) / 0.403 µs (1000) p50 per significant event** (E-8 log,
  §8.5 witness enumeration; double-run identical). The full event
  (census + witness evaluations) is 0.4–1.8 µs at these scales.

### 1.11 Inventory summary — how many DISTINCT query shapes exist?

**Six engine query primitives, three families by population and
contract:**

| Family | Queries | Population | Ordered output? | Frequency |
|---|---|---|---|---|
| **D — dynamic-actor proximity** | Q-A census (2D disc), Q-F interaction candidates (3D range+cone), Q-G combatant trace (3D ray) | moving NPCs / targets | YES — population/identity order (Q-A); nearest+lowest-id (Q-F/G) | per event / per input |
| **S-box — static world, box queries** | Q-B/Q-C/Q-E depenetration, sweep_axis, box_overlaps | static colliders (never moves) | tie-by-iteration-order (insertion order) | per tick per mover / per player tick |
| **S-ray — static world, segment queries** | Q-D segment_hit | static colliders | smallest-t, tie by list order | per frame (camera) / per attack |

Family D members do **not** share a broad phase for free: the census
enumerates ALL witnesses in a disc in identity order; interaction/combat
want the nearest along a cone/ray with lowest-id ties — the same
candidate set, different reduction. A shared dynamic-actor candidate
generator (disc/range broad phase) can serve all three IF it yields
conservative candidates that the existing exact reductions then filter —
which is precisely the charter's "share candidate discovery when it
makes sense, not every query's narrow phase". Families S-box and S-ray
share one population (the static collider list) and CAN share one static
broad phase, with per-query narrow phases unchanged.

---

## 2. The spatial foundation decision

### 2.1 Options vs the actual query set

**(a) Uniform grid / hash grid.** Cell size ~ query extent; insert =
compute cell; query = walk cells under the query's bounding shape.
- D family: per-tick rebuild (counting sort over the active population)
  or incremental move; candidates re-sorted to population order.
- S families: built once at world construction; box queries walk the
  cells under the (swept) box; segment queries walk the cells under the
  segment's conservative AABB (or a DDA).
- Determinism: SAFE if the grid is a **lookup structure only** — never
  iterate the map itself; walk cells in a canonical order derived from
  the query (row-major over the query's cell span), collect candidates,
  restore the contract order (population index / collider index) before
  the unchanged narrow phase. A `HashMap<CellKey, …>` is then
  order-invisible. This is the whole trick that makes a grid compatible
  with the spine's fixed-order discipline.
- Weakness: single cell size serves one extent scale; pathological when
  actor extents or query radii span orders of magnitude (not the case
  today — §2.3).

**(b) Hierarchical grid.** Multiple cell levels for mixed extents.
- No current workload has mixed extents: NPC boxes ~0.6–1.2 m, census
  radii from the encounter table (meters), player reach 2.5 m
  (interaction.rs:49-53), weapon ranges (combat profiles). All within
  ~one decade. **No measured need; defer** with an explicit trigger
  (§2.6).

**(c) BVH.** Ray/box traversal over a static hierarchy; the classic
city-scale answer for family S.
- Strength: large static worlds, rays, no cell-size tuning.
- Cost: build complexity, deterministic-build requirements (must derive
  from a canonical input order — Morton sort), and per-query traversal
  overhead that LOSES to a linear scan at demo scale (tens of
  colliders). The charter: "Favor simple, well-understood algorithms
  until scale justifies complexity."
- The uniform grid delivers the same O(touched) bound for boxes and
  conservative-AABB segment walks with far less machinery. **Defer BVH**
  behind the same evidence trigger as the hierarchy (§2.6).

**(d) Sorted cell arrays (compact/compactfied grid).** Flat
cell-sorted index + per-cell offsets; query = binary search.
- For family D this is the natural per-tick REBUILD form: counting sort
  the active population by cell (O(N + cells), identity-stable), then
  each census walks its cells' slices. No per-move incremental cost —
  attractive because locomotion moves 24–1000+ NPCs per tick (E-8 full
  lane), so incremental remove/insert per mover would cost O(movers)
  anyway; a rebuild costs O(N) once and serves every query that tick.
- For family S (static), a sorted array is equivalent to (a) with better
  locality; build once.

**(e) Hybrids.** The honest reading of the inventory: D and S are
different populations with different lifecycles (movers vs never-moves)
and different order contracts (identity order vs insertion order) —
**one universal index would couple them for no query's benefit**. The
charter's bar is explicit: no universal index before identifying real
differing needs; the needs differ.

### 2.2 The recommendation

**Two family-scoped broad phases, three exact narrow phases unchanged,
and the linear path retained as the default below measured crossovers.**

1. **Family S — a persistent, lazily-built uniform grid inside
   `CollisionWorld`** (engine-core, `player.rs`/`collision.rs`), over
   BOTH collider lists keyed by their world AABBs (the rotated list
   already caches one — collision.rs:71-77):
   - Activated by a collider-count threshold (initial gate 128;
     §9 measures the true crossover — the demo worlds are far below it,
     so **today's worlds keep the exact current linear paths,
     bit-identical**; the threshold is a build-time/measurement-pinned
     constant, not fashion).
   - Every query (`box_overlaps`, `segment_hit`, `sweep_axis`,
     `depenetration`, and the rotated pretests) first gathers candidate
     collider INDICES from the cells under the query's conservative
     extent, **sorts them back into original insertion order** (small
     sets; the sort is over indices), partitions axis-aligned vs
     rotated, and runs the EXISTING narrow-phase loops over those
     ordered slices. Because the cell walk is conservative (a superset
     of touching colliders — cell size ≥ nothing special is required,
     only that insertion uses the same AABB the pretests use) and the
     order is restored, **every result and every tie is bit-identical to
     the linear path** — the same argument collision.rs:31-39 makes for
     the empty-rotated-list guarantee.
   - Segment queries walk the cells under the segment's raw AABB
     (conservative; the rotated pretest's enclosure discipline,
     collision.rs:474-499, is the precedent). A DDA refinement is
     deferred until a measured workload shows AABB-walk waste
     (long diagonal rays in large worlds).
   - Cost model: build O(C) once per world mutation (worlds are built
     once; `with_*` builders); query O(cells walked + candidates).
     Memory O(C) indices.
2. **Family D — a per-tick rebuilt compact cell index over the census
   population, owned by the composition layer** (the `World`
   implementation that owns the population — `npc_memory_lane.rs`'s
   successor; NOT inside `perceive`):
   - `engine-core` keeps `perceive`/`enumerate_witnesses` EXACTLY as
     the linear primitive (they are the contract and the fast path),
     and gains a sibling `indexed` census primitive with the IDENTICAL
     output contract (population-ordered indices, inclusive boundary,
     fail-closed radius — npc_perception.rs:116-141) taking a built
     index + query. The composition chooses per workload; both paths
     remain permanent (charter: "Retain simple small-workload fast path
     where faster").
   - The index: counting-sort of `(cell, population_index)` over the
     census population per tick (or per population-change batch),
     identity-stable within cells; census = walk the disc's cell span
     row-major, collect, run the UNCHANGED `record_in_radius` on each
     candidate (fixed op order preserved — npc_perception.rs:103-114),
     then stable-sort the survivors by population index (or emit via a
     seen-bitmap in population order — cheaper for large results).
     Fail-closed and boundary semantics come free because the exact
     test is the same code.
   - Q-F/Q-G may later ride the same index for candidate generation
     (conservative disc/box cells around the eye/origin), with their
     exact nearest/lowest-id reductions unchanged — but only after the
     census lane proves the pattern (§7 lanes).
3. **Small-workload fast path — the crossover is a MEASURED GATE, not a
   constant.** From E-8: census at 1000 population = 0.403 µs p50
   (linear); the rebuild alone costs ≥ one population pass (~the same
   0.4 µs) plus bucket writes. The index pays only when
   (queries per tick) × (linear cost − indexed cost) > rebuild cost —
   i.e. roughly ≥ 2–3 census queries per tick at 1000 population, fewer
   at higher N (linear grows, rebuild grows identically — the RATIO
   improves with query count, not population). At the demo's 24-record
   census the linear path is sub-timer-resolution and **wins forever**.
   Initial gates (to be replaced by §9 measurements): population ≥ 512
   AND expected census queries per tick ≥ 4. Below either: linear.
   Family S gate: collider count ≥ 128 (static build amortizes over the
   per-tick per-NPC depenetration load — at 1000 movers, 1000+
   box queries per tick, the grid pays at far smaller collider counts
   than a single-query world would; the E-8 locomotion isolate shows
   the collision share is already ~½ of the near lane at 1000 NPCs vs a
   tiny world).
4. **Stable IDs and result ordering** — the discipline, made mechanical:
   every indexed path restores the contract order (population index for
   family D, collider insertion index for family S) BEFORE the exact
   narrow phase or the consumer. The five campaign digest pins are the
   enforcement: decision `1277463877274275604` (npc_behavior.rs:967),
   schedule `11670746710459133860` (npc_schedule.rs:559), perception
   trace `10766371085850726021` (npc_perception.rs:545 — NOTE: the
   brief's `1076637108585072602` drops the final digit; the source pin
   is the 20-digit value), execution stream `10778018066104731562`
   (npc_execution.rs:1725), locomotion `852983358677453085`
   (npc_locomotion.rs:2381). All must hold unchanged through every P2
   lane (they exercise the census, the selectors, and the locomotion
   batch end-to-end).
5. **Update/removal costs for moving actors** (the locomotion batch
   moves 24–1000+ per tick): family D pays O(N + cells) per rebuild —
   bounded, allocation-stable with caller scratch, and CHEAPER than
   1000 incremental hash remove+inserts (which would also churn). No
   per-actor removal API exists to get wrong; dormant/absent NPCs are
   simply not in the census population (the tier gate already excludes
   them — npc_perception.rs:30-34). Family S has NO updates (static).
6. **Multi-region / large-coordinate boundaries**: cell keys are
   `i32` pairs computed as `floor(coord / cell_size)`; the index build
   VALIDATES the domain fail-closed (all finite, |cell| < 2^21 — an
   explicit numerical-domain contract per charter §4, mirroring
   `perceive`'s radius gate). World-space f32 coordinates with
   region-local saves (REGION_GRID u8, npc_world_save.rs:109-111)
   already establish the local-origin discipline; the scalable form
   (§7) is per-region indexes with boundary queries consulting the
   adjacent region's index — the census composition already presents
   per-active-region populations, so the boundary rule is a composition
   concern, documented and tested at region edges (§9). "Multi-scale
   synthetic worlds must not require allocating a full dense grid for
   the galaxy" — a hash/compact grid over OCCUPIED cells only satisfies
   this by construction (sparse keys, no dense allocation); the dense
   `REGION_GRID` array remains the per-region save encoding (bounded
   16×16 per region), never a world-space dense grid.

### 2.3 Why not one shared broad phase for D and S?

The census (Q-A) queries the NPC population; the player/NPC kinematics
(Q-B/C/E) query the static world. A mover-vs-mover query family does not
exist yet (NPCs don't collide with each other — locomotion is
kinematic-vs-static only, npc_locomotion.rs:562-566). Building a unified
actor+static index now would be speculative generality with zero
current queries — exactly the charter's "universal index before
identifying real differing query needs". The design leaves the seam
(the candidate-generator contract, §2.2.2) so a future mover-vs-mover
family can adopt it with evidence.

### 2.4 Tradeoff table

| Option | Query cost (touched) | Build/update | Memory | Determinism risk | Fits | Verdict |
|---|---|---|---|---|---|---|
| Linear scan (status quo) | O(N) or O(C) | none | none | none | all small workloads | **RETAINED as default + fast path** |
| Uniform/hash grid (S family) | O(cells+cand) | O(C) once | O(C) | order restoration required (solved §2.2.1) | box + segment queries, static | **ADOPT (gated)** |
| Compact sorted grid (D family) | O(cells+cand+sort) | O(N+cells)/tick | O(N)+cells | order restoration (solved §2.2.2) | census, per-tick movers | **ADOPT (gated)** |
| Hierarchical grid | better mixed extents | 2–3× grid | 2–3× grid | same + level order | extents vary ≫1 decade | DEFER (trigger §2.6) |
| BVH | O(log C · hits) | O(C log C) | O(C) | deterministic build discipline | huge static, rays | DEFER (trigger §2.6) |
| Universal index (D+S+scheduler) | — | — | — | couples all contracts | no query needs it | **REJECTED (charter bar)** |

### 2.5 What this decision does NOT change

`perceive`, `record_in_radius`, `enumerate_witnesses`,
`CollisionWorld::{segment_hit, sweep_axis, depenetration, box_overlaps}`,
`select_target`, `resolve_attack`, `segment_box_hit` keep their
signatures, exact arithmetic, and tie rules — the narrow phases are the
contract; only candidate SOURCES change, behind measured gates. The
interaction zero-distance tie deviation (§1.6) is a separate
contract-fix lane (§7 L1), decided on the module's own documented
contract, not on optimization grounds.

### 2.6 Deferred-mechanism triggers (recorded, not fashionable)

- Hierarchical grid: any family-D query set whose extents/radii span
  > ~16× (e.g. building-scale crowds + vehicle-scale perception).
- BVH: static collider count where grid cell-walk waste (empty-cell
  walking in sparse large worlds) measurably dominates — the §9 slope
  curves decide.
- DDA segment walk: measured AABB-walk cell waste > ~4× the ray's true
  cell crossings on a real workload.
- Mover-vs-mover broad phase: the first authoritative NPC-NPC collision
  feature (none exists today — cite before building).

---

## 3. B01 — Scheduler housekeeping accounting

### 3.1 The confirmed gap, precisely

`drain_due` (spine.rs:1767-1809) loops `while executed < cap`. A due
event with unmet dependencies is parked (`blocked.insert` +
`register_waiters_for_unmet`, spine.rs:1804-1805) **without incrementing
`executed`** — correct per the documented intent ("parked without
consuming budget (no work was done)", spine.rs:1763-1766), but it means
one tick's housekeeping work (park + waiter registration, each O(deps)
with `BTreeMap` inserts) is bounded only by the number of due-blocked
events. `execute_now` (spine.rs:1824-1913) releases all waiters of a
completed event synchronously (`:1907-1912`), each `try_unblock`
(spine.rs:1953-1974) re-scanning the waiter's FULL dependency list
(`has_unmet_dependencies`, `:1921-1926`). Fan-in/fan-out multiply both.
The E-8 skip decomposition (≈ 0.163 µs/event + 12.7 ns/tick) measured
the *healthy* shape; the adversarial shape (deep dependency graphs,
thundering herds on one prerequisite) is unbounded per tick today.

### 3.2 What must NOT change (the invariants)

- The `(due, sequence)` total order governs all observable ordering
  (pop order, fingerprints, save canonical order — spine.rs:2041-2047).
- The guarantee on ACTUAL executions (budget = executions; blocked
  park is free — that semantic stays).
- Held/blocked/unfinished semantics; conservation; `stats.scheduled`
  never decremented by suspension (spine.rs:888-894).
- Near/far fairness: the three-phase reservation
  (run_tick, spine.rs:1242-1251: near guaranteed `near_reserve`, far
  guaranteed remainder, then near-extra, far-extra).
- Save/restore: authoritative = clock, next_seq, budget/reserve, stats,
  queues (canonical order), blocked (seq order), held (seq order),
  ledger. **Derived and rebuilt on load: `waiting_on`, `held_index`,
  `unfinished` complement machinery, `max_executed_index`**
  (spine.rs:2041-2052). Any new counter/index that is derivable from
  canonical state joins the derived class → wire-compatible BY
  CONSTRUCTION, no format version bump. This is the load-bearing fact
  for the whole section.
- Budget exhaustion must be observable and make progress (charter §4):
  `budget_exhausted_ticks`, `blocked_transitions`, late counters
  already exist (spine.rs:1792, 1257-1265).

### 3.3 The accounting design

**(1) A housekeeping budget as a second loop bound — not a new
scheduler.** `drain_due` gains a counted housekeeping allowance:

- Each unit charged: one blocked park (the pop + unmet scan +
  registration), one waiter registration, one unblock revalidation,
  one queue push from unblock. NOT charged: peek (cheap), the pop of an
  event that executes (that's the execution's cost, already inside the
  execution cap's accounting), handler follow-up scheduling (part of
  the executed event's consequence — deferring it would reassign
  `next_seq` and change observable order, so it stays atomic with the
  execution).
- Loop form: `while executed < cap && housekeeping < hk_cap`. On hk
  exhaustion with due work remaining: break, and record it — a new
  `stats.housekeeping_exhausted_ticks` counter plus the existing
  `budget_exhausted_ticks` semantics extended to cover it (the
  observable-exhaustion discipline). The un-drained due events REMAIN
  IN THE QUEUE with their `(due, seq)` untouched — identical to
  execution-budget exhaustion semantics today (late execution on a
  later tick; the events are never dropped).
- **Progress guarantee:** each tick performs ≥ min(cap, hk_cap) units
  of classified work; a head-of-queue permanently-blocked event
  consumes at most one pop+scan per tick before being parked (park is
  one hk unit) — after parking it costs nothing until unblocked (the
  `blocked` map is passive). The pathological repeat cost exists only
  for events that stay queued-because-unbudgeted, and each retry is a
  counted hk unit, so the per-tick bound holds by construction. No
  starvation deadlock: unblocking is driven by executions (capped but
  progressing) and by `resume`; a zero-execution, all-blocked tick is
  already representable today and stays observable via counters.
- **Default value:** hk_cap = a small multiple of the execution budget
  (initial `budget + budget/2`; §9 measures the real
  housekeeping-per-execution ratio on the sustained matrix and the
  adversarial graphs — the default is a measurement placeholder, marked
  `[ASSUMPTION]` like `PERCEPTION_BUDGETS`).
- Near/far fairness is preserved because the bound lives INSIDE
  `drain_due`, which the three-phase `run_tick` already calls per lane
  with per-phase caps — the reservation arithmetic is untouched.

**(2) Remaining-dependency counts — the charter's suggested
intermediate, ADOPTED.** A derived `BTreeMap<u64 seq, u32 remaining>`
over BLOCKED events only:

- Computed once at park time (count of UNIQUE unmet dependencies — the
  same dedup `register_waiters_for_unmet_into` performs,
  spine.rs:435-444; duplicates in `depends_on` must count once).
- Decremented in the release path: when event X executes, for each
  waiter W registered on X (the `waiting_on` list already being
  drained, spine.rs:1908-1911), decrement W's remaining; at zero, W
  unblocks — O(1) instead of `try_unblock`'s O(deps) re-scan.
- Lifecycle follows the blocked map exactly: park creates, unblock
  consumes, suspend-of-a-blocked-event drops (with its `waiting_on`
  registrations — spine.rs:896-901 already removes those; the counter
  dies with the entry), resume revalidates and recounts
  (resume_matching already re-runs the dependency check,
  spine.rs:1108-1116).
- Derived from canonical state (deps + unfinished) → rebuilt on load,
  wire-invisible (the `held_index` precedent, spine.rs:415-420).
- Determinism: pure integer counters driven by the same ordered
  release sequence; no iteration-order dependence (the release order is
  the `waiting_on` Vec order, unchanged).
- Edge cases to pin in tests: duplicate dependency ids count once;
  a dependency that is HELD (unfinished, not executed) keeps remaining
  > 0 (held is not executed — spine.rs:371-383's complement rule); a
  dependency executed while the waiter was PARKED vs QUEUED; re-park
  after resume; the counter surviving save/restore round-trip as the
  same integer.

**(3) Indexed waiters — already present, kept.** `waiting_on:
BTreeMap<u64, Vec<u64>>` (spine.rs:369-370) IS the waiter index; the
fan-out iteration is genuinely O(waiters-on-X) and irreducible (each
waiter truly needs its release attempt). With (2), each attempt is
O(1). No new structure needed — this is a no-change sub-finding WITH
evidence: the index exists, is maintained at exactly the right sites,
and is derived/wire-invisible.

**(4) NOT adopted (and why):** partitioned queues / timing wheels —
the E-8 skip decomposition shows the event term (0.163 µs/event)
dominates only at ≥ 0.1 events/tick and the heap's 12.7 ns/tick term is
noise; a timing wheel's payoff window (many long-delay homogeneous
events) is not a measured workload, and it would fork the (due, seq)
discipline into a second representation — the charter's "don't replace
the event spine wholesale on fashion". Revisit trigger: a measured
world whose per-tick due-pop count (not event count) dominates tick
cost at 100k+ scale.

### 3.4 Save/restore and observability

No wire change: both (1)'s counter is a `Stats` field (Stats IS
authoritative on the wire — spine.rs:2043; adding a field is a
versioned Stats extension, same class as any past counter addition —
the save decoder gates it explicitly, never silently) and (2)'s map is
derived-rebuilt. Digests: engine/state fingerprints hash Stats
(spine.rs:1394-1400) — a new counter changes `engine_fingerprint`
values; the campaign digests that must hold (decision/schedule/
perception/execution/locomotion, §2.2.4) do NOT hash spine Stats —
verified: they hash behavior streams, not spine fingerprints. The
spine-fingerprint test pins will need conscious re-pinning under the
repo's investigation-not-silent-repin discipline if the counter is
added to Stats (prefer: keep the new counter OUT of fingerprinted
Stats, in the observational `LaneReport`/`SuspensionStats` class —
spine.rs:2048-2052 documents that class as never-affecting
scheduling/fingerprints; **recommendation: put housekeeping
exhaustion in the observational class** to avoid touching any pin).

---

## 4. A08 — Spine suspend/reclassify/resume as localized operations

### 4.1 The current cost shape (cited)

- `suspend_from_queue` (spine.rs:1128-1149): full heap pop loop into a
  temp `Vec::with_capacity(queue.len())` + filter + re-push each —
  O(N log N) + one allocation per lane per call.
- `reclassify_queue` (spine.rs:1206-1227): same shape.
- `resume_matching` (spine.rs:1097-1122): full `held` scan collecting
  seqs into a Vec, then per-seq remove + O(deps) revalidation.
- `suspend_matching`'s blocked arm (spine.rs:921-926): scan + collect
  Vec.
- `collect_matching` (spine.rs:861-886): iterate both heaps + blocked,
  collect, sort — O(N log N) + alloc.
- Callers: region transitions only (region.rs:975, 1019, 1201-1203 —
  per LOD change, not per tick), crossing/soak harnesses. So the
  absolute cost today is small; the finding is the SCALING shape
  (per-transition O(queued) work as worlds grow) and the allocation
  churn.

### 4.2 The design — three steps, in evidence order

**Step 1 (the charter's named intermediate, adopted): linear bulk
partition + heapify, allocation-free.**
`suspend_from_queue`/`reclassify_queue` become:

1. `let mut items = queue.into_vec()` — consumes the heap, REUSING its
   buffer (no allocation; `BinaryHeap::into_vec` is O(1)).
2. Stable partition in one linear pass: matching items out (to
   `held`/`moved`), retained items kept IN THE SAME BUFFER
   (a `Vec::retain`-style compaction preserves relative order — order
   within the heap layout is rebuild-invisible anyway, spine.rs:1124-1127,
   but stability costs nothing and keeps the argument simple).
3. `*queue = BinaryHeap::from(items)` — heapify, O(N), no allocation
   (the buffer transfers).
4. The held/moved side: `BTreeMap` inserts (O(M log H)) — unchanged
   semantics.

Net: O(N) + O(M log H) per call, ZERO allocations (vs O(N log N) +
1–2 Vecs). Observable behavior identical: the heap is an
implementation detail whose layout never reaches a fingerprint
(spine.rs:1124-1127, 2041-2047 — canonical order only). The blocked-arm
scan and `resume_matching` similarly drop their temp Vecs:
`BTreeMap::retain` with split borrows (the
`register_waiters_for_unmet_into` precedent for free-function split
borrows, spine.rs:435+, 1945-1951) lets resume move matching held
entries out and push them to the lanes in one in-place pass.

**Step 2 (the endgame, GATED on Step-1 measurements): a per-class
derived index.** The production predicates are all
`view.class == EventClass(region)` (region.rs:975, 1019; crossing.rs).
A derived `BTreeMap<EventClass, BTreeSet<u64 seq>>` (or counting
structure) maintained at the six sites where an obligation's location
changes (schedule / park / unblock / execute / suspend / resume) makes
every region operation O(class members + log) instead of O(total
queued). Cost: one O(log) set op per transition site on the hot path
(≈ 0.163 µs/event today — a `BTreeSet` insert adds ~10–20 ns, ~10% of
the event term; the sustained matrix re-measures this). Wire-invisible
(derived from canonical queues/blocked/held). **Adopt only if** the
Step-1 world at target scale (100k obligations, 8–64 regions,
transition-heavy presence movement) still shows transition cost
dominating a measured frame budget — otherwise Step 1 is the honest
stop. This is exactly the charter's sequencing: intermediates first,
indexed representation when the workload warrants.

**Step 3 (not adopted): a partitioned-by-class queue structure.**
Would replace the two-lane heap with per-class heaps and a merge — a
wholesale spine replacement. Rejected per the charter bar; the (due,
seq) heap + reservation is the spine's identity.

### 4.3 Invariants preserved (checklist for the lane)

- Identity-preserving relocation: `(due, seq)`, schedule, payload,
  lane, dependencies all kept (spine.rs:888-894, 1151-1158);
  `stats.scheduled` never decremented.
- Queued-suspend keeps `waiting_on` intact; blocked-suspend drops it
  (spine.rs:896-901); resume revalidates through the normal path
  (spine.rs:1108-1116).
- `held` keyed by seq (canonical), `held_index` (due, seq) maintained
  at the same sites — Step 1/2 must update it identically
  (spine.rs:415-420's O(log n) watermark requirement).
- Save/restore: byte-compatible with NO migration — queues serialize
  in canonical (due, seq) order regardless of internal layout
  (spine.rs:2041-2047); held in seq order. Step 2's index is
  derived-rebuilt.
- Discriminating tests: suspension round-trip through save/load;
  the (due, seq) output of `collect_matching` unchanged; the existing
  suspension/hold test battery; sustained-matrix fingerprints
  (E-8 cells double-run identical).

---

## 5. A09 — Witness-oriented memory access

### 5.1 The structural fact that shapes everything

`MemoryKey` orders **(event, witness, kind)** (npc_memory.rs:89-99) —
the canonical event-first order the charter names. The pending and
confirmed `BTreeMap`s therefore iterate in EVENT order; a witness's
records are scattered, which is why `witness_records`
(npc_memory.rs:675-685) full-scans both maps, filters, and
SORTS — O(ledger) + O(result · log result) + an allocation, per call.
`pending_keys` (`:691-697`) multiplies it (calls `witness_records`,
re-filters); `witness_counts` (`:701-713`) scans twice more;
`activate` (`:869-905`) pays it per witness activation;
`witness_classifications` (`:911-917`) scans the classifications map.

### 5.2 The design — a derived witness→keys index + phase merge

- **Index:** `witness_index: BTreeMap<u32 witness, BTreeSet<MemoryKey>>`
  (or per-witness sorted `Vec<MemoryKey>` for locality), maintained at
  the ONE insertion site — `MemoryLedger::capture` (`:637-649`) — and
  at any future removal site. `confirm` moves a record BETWEEN phases
  without changing the key set → **index untouched** (a load-bearing
  simplification: the index is phase-blind). `classify` writes the
  separate classifications map → untouched.
- **`witness_records(w)`** becomes: iterate `witness_index[w]` (a
  per-witness `BTreeSet<MemoryKey>` iterates in canonical MemoryKey
  order — (event, kind) for fixed witness — EXACTLY the order the
  current sort produces), and for each key take
  `pending.get(k).or_else(|| confirmed.get(k))` — the same
  phase-precedence the current `record()` exposes (`:663-667`), fused
  as a two-phase merge. Output: the identical `Vec<&MemoryRecord>` in
  the identical order. Cost: O(log L + result) with zero sorting.
- **`pending_keys`/`witness_counts`**: same index walk with phase
  lookups — O(log L + result).
- **`witness_classifications`**: an analogous derived
  `BTreeMap<u32, BTreeSet<MemoryKey>>` over `classifications`, or (if
  classification volume stays small) keep the scan and document the
  boundary — measured decision, lane-gated.
- **`activate`**: folds the index walk — the rebuild becomes
  O(log L + witness's own records), the "avoid O(global history)
  activation" requirement. **Batch regional activation** is then the
  composition looping the region's witnesses (region.rs activation
  path) with per-witness indexed folds — no new engine API needed
  unless measurement shows per-witness O(log) overhead dominating
  (then a `activate_batch(&[u32])` sharing one scratch).
- **Wire/digest compatibility:** the index is DERIVED (rebuilt from
  the canonical maps on load — the spine's derived-class precedent).
  The canonical maps, `digest()` (`:733-746`), the journal plans, and
  the episodic retention order (`retain_episodic`,
  npc_memory.rs:438-459 — event-ordered insert/evict) are untouched.
  Memory-lane digests and the E-8 fingerprints must hold unchanged.
- **Bounded resident caches with durable history:** the `live`
  BTreeMap is bounded by ACTIVE witnesses (the materialization gate,
  `:831-839`); the growth term is the LEDGER. Retention design
  (composition-level, coordinates with P3/B04's journal machinery):
  a per-witness **checkpoint** — a serialized `WitnessMemory`
  snapshot at a key boundary + the post-boundary key tail; activation
  folds checkpoint + tail range (still canonical order); compaction
  evicts pre-boundary CONFIRMED records from RAM while they remain in
  durable journal history. Checkpoint format is versioned
  (memory-lane wire change, explicit migration, never silent); the
  boundary policy (records per witness / bytes) is configuration with
  observable eviction counters. This is a P3-coordinated lane (the
  journal owns durable history); P2 fixes the access shape and the
  index that makes bounded RAM possible.

### 5.3 What stays linear (on purpose)

Single-record `record()`, `is_captured`, `confirm`, `classify` are
already O(log) map ops — no change. The global `candidate_records`
fold (npc_memory.rs:752-759) is lane-scope and rare; index later only
if measured.

---

## 6. A22-core — Execution-ledger retention as a lifecycle policy

### 6.1 Current state (cited)

- The ledger is a `Vec<ExecutionRecord>` appended at every execution
  (spine.rs:1839) and grown until the EXPLICIT, versioned
  `compact(retain)` (spine.rs:1345-1375) — authenticated SHA-256 prefix
  summary, `spine-history/2` save form, eligibility left to the
  application driver (spine.rs:1338-1344).
- **The identity/dedup evidence is NOT the ledger.** Executed-membership
  is the `unfinished`-complement (`seq < next_seq && !unfinished`,
  spine.rs:371-383); exactly-once and conservation hang off
  `next_seq`/`unfinished`/`waiting_on`; `max_executed_due` is maintained
  incrementally precisely so nothing scans the ledger per frame
  (spine.rs:391-399, 1843-1846). The ledger is HISTORY: audit trail,
  late-accounting source, and the compaction digest's input. This
  distinction — the charter's "required identity/dedup evidence vs
  diagnostic history" — is already structurally true; the design makes
  it a DOCUMENTED contract and polices it with tests (no future code
  path may consult the ledger for a correctness decision).
- E-8: lifetime store growth ~31.0 B per execution (wire RECORD_BYTES
  = 31, spine.rs:2059-2062; ~23 exec/tick fixture); compaction of a
  150-tick history = 1.16 ms, compacted snapshot 124,968 B vs 220,438 B
  (retain 100).

### 6.2 The design — compaction as POLICY, bounded active RAM

- **A configured retention policy** (composition-level config, not
  engine constants): `retain_records: NonZeroU64` OR a byte budget
  (records × ~40 B resident — measure the in-memory stride in §9;
  wire is 31). Policy fires the EXISTING `compact()` — no new
  compaction mechanism, no second summary domain.
- **When it fires:** between ticks (never mid-drain — `compact` is a
  between-ticks operation by its own doc, spine.rs:1344's "explicit
  operation"), when `ledger.len() > retain + chunk` — **chunked** so
  each invocation's SHA-256 work is bounded (the 1.16 ms / 150-tick
  measurement extrapolates linearly; a 100k-record cut would be ~ms
  scale — chunk caps it, e.g. ≤ 4,096 records per invocation, making
  compaction cost observable and bounded per tick like every other
  budget).
- **Observability:** compaction events, chunk sizes, and cumulative
  summarized counts join the observational stats class (NOT
  fingerprinted Stats — §3.4's recommendation), so no pin moves.
- **Eligibility stays the application driver's** (spine.rs:1338-1344):
  the engine policy bounds RAM; the driver decides when consequence
  lanes tolerate a prefix cut. A driver that never calls it keeps
  today's exact behavior (default config = never — **no silent
  behavior change**; the demo opts in explicitly).
- **Save/restore:** already versioned (`spine-history/2` exists,
  `HistoryKind`, `try_engine_fingerprint` vs `engine_fingerprint`
  split — spine.rs:1379-1384). Auto-policy changes WHEN compaction
  happens, not WHAT it is → no format change, no migration. The
  full-history APIs keep their loud guards.
- **No-change sub-finding (evidence):** the compaction MECHANISM
  (authenticated chain, prefix discipline, versioned save form) is
  sound and ratified (PP-B2/D-PP-3, spine.rs:1320-1336); P2 adds only
  the driver policy + chunk bound + counters.

---

## 7. A05/B12 — World configuration and canonical identity

### 7.1 Current state (cited)

- `REGION_COUNT = 8`, `NPC_COUNT = 1000`, `REGION_GRID = 16`,
  `POSITION_DISCARDED` live as engine-core constants
  (lib.rs:97-115) explicitly documented as "the shared wire-format
  bound" — every layer reads ONE constant.
- `NpcWorldSection` (npc_world_save.rs:60-101) encodes the count and
  dense per-NPC arrays (`write_count(NPC_COUNT)` at `:116`; restore
  REFUSES any other count — "a different count is an incompatible
  workload, rejected at restore, never a resized restore", `:61-65`,
  `:177-178`). Region mask is `REGION_COUNT`-bit (`:71-73`, `:233-235`);
  positions are region-local `(u8, u8)` on the `REGION_GRID` grid with
  the discard sentinel (`:87-89`, `:197`).
- `NpcIdentity` (npc_behavior.rs:85-142): an 11-byte codec
  (id u32, home_cell 2×u8, region u8, appearance_seed u32) with an
  asymmetric golden record — but the SAVE FORMAT does not use it
  (B12: no caller beyond module/docs; the save's identity is the
  array POSITION, id == index).

### 7.2 The design — a configuration boundary with explicit format versioning

**The compatibility adapter vs final mechanism distinction (charter §7
P2's own words) is the lane structure:**

- **Final mechanism — `WorldComposition` config:** a plain-data
  configuration (region_count, npc_count, region_grid, plus the
  behavioral knobs the section already carries) supplied by the
  application at world construction. Engine-core STOPS exporting the
  demo's specific values as the only truth; the constants move to the
  demo/slice layer as its authored workload (the A05 required outcome:
  "Move demo workload policy out of engine-wide constants"). The save
  section carries its own shape: **`npc-world-save/2`** — a new domain
  string (the v1 discipline: domains are compatibility boundaries,
  npc_world_save.rs:56-58) that writes region_count / npc_count /
  region_grid as DATA, with per-region sub-sections.
- **Old saves — retained valid, never silently migrated:** a v1 save
  (domain `engine-slice/npc-world-save/1`, count 1000) loads through
  the v1 decoder exactly as today, materializing the fixed workload it
  encodes; a v2 save loads its own shape. The loader dispatches on the
  domain byte-for-byte as it already does for every other versioned
  domain (the repo's standing save discipline). NO v1→v2 rewrite
  happens implicitly; an explicit `migrate` utility (opt-in, reported)
  may transcode later if the owner wants one. This satisfies "Retain
  valid old saves or explicit migration — never silent."
- **Scalable identity (B12):** v2 carries a per-region identity table
  encoded with the EXISTING `NpcIdentity::wire_bytes` codec — making
  the round-trip "through actual authoritative storage" (B12's
  requirement) a testable property instead of a codec self-test. The
  dense per-NPC arrays REMAIN positional (id == index within the
  region block) — dense positional state is compact,
  cache-friendly, and deterministic; the identity table is the durable
  lane (home cell, region membership, appearance seed) that survives
  representation discard. Non-dense id spaces stay out of scope until
  a feature needs them (recorded as the boundary).
- **Region-partitioned encoding (galaxy rule):** v2 writes
  region-major blocks; a DORMANT region's block carries only the
  dormant record columns (the tier memory table's Dormant shape — 20 B
  record-only per NPC, E-8) using the EXISTING discard sentinels
  (POSITION_DISCARDED, routine-cursor sentinels). "Multi-scale
  synthetic worlds must not require allocating a full dense grid for
  the galaxy": the world-space representation is per-region sparse
  (occupied regions only); the per-region 16×16 grid stays bounded BY
  DESIGN (it is region-local, not world-global). Loading a v2 world
  materializes only the regions the anchor needs active — the
  LOD/dormancy machinery (region.rs) already defines what "active"
  means; the save format stops contradicting it.
- **Renderer scene capacities (A05's third clause)** are P4/P5
  territory (A11); this design only requires that no engine-core
  constant weld the SIMULATION shape to the renderer's (the
  configuration seam above provides the boundary).

### 7.3 What this is NOT

Not a rewrite of `NpcWorld`'s arrays, not a streaming-world feature,
not NPC-NPC collision. The v1 format, the fixed 1000-NPC demo, and its
fingerprints stay untouched; v2 is additive with its own tests. The
lane is orchestrator-implemented or design-first (format/durability
semantics class — the standing rule from the A24 lesson,
FINDINGS.md calibration record).

---

## 8. The staged lane plan

Dependency order; risk classes per the standing rule (FINDINGS.md
calibration record: "timing/determinism semantics work is no longer
delegated … the orchestrator implements or the lane produces a design
document first"). Every lane's acceptance includes the five campaign
digest pins (§2.2.4) UNCHANGED plus its own discriminating tests.

| Lane | Content | Class | Depends on | Pins that must hold | Discriminating tests |
|---|---|---|---|---|---|
| **L1 — B06 contract fix** | `select_target` zero-distance branch gets the documented lowest-id tie (interaction.rs:111-118 vs `:133-138`); NO broad-phase change | semantics (orchestrator-implemented; tiny diff) | none | interaction tests updated deliberately (this lane CHANGES an edge-case behavior to match the module's own documented contract — the pin movement is conscious, documented, approved per charter §8 test-representation rules) | two targets at exact distance 0 → lowest id wins regardless of slice order; zero-distance vs surface-0.0 tie; purity across slice permutations |
| **L2 — A08a spine bulk ops** | `into_vec` + stable partition + heapify for suspend/reclassify; `retain`-based allocation-free resume/blocked-suspend; split-borrow free functions | spine/determinism (orchestrator-implemented per standing rule) | none | decision, schedule, execution, locomotion digests; suspension battery; save/load round-trip | (due,seq) collect_matching output identical pre/post; suspension→save→restore→resume equivalence; sustained-matrix fingerprints |
| **L3 — B01 accounting** | housekeeping cap in `drain_due` (observational counter, NOT fingerprinted Stats); remaining-dependency counts (derived map) | spine/determinism (orchestrator) | L2 (same file, sequence to avoid overlap) | all five digests; sustained-matrix cells identical (same executions, same dues) | adversarial fan-in/fan-out graph: housekeeping bounded per tick, no deadlock, counters move; duplicate-dep counts once; held-dependency stays unmet; save/restore rebuilds counts |
| **L4 — A09 index** | witness_index + phase merge for witness_records/pending_keys/witness_counts (+ classifications if measured); activate folds | engine-core pure, output-identical (lane-implementable with tight contract) | none (disjoint from L2/L3 files? same crate, different module — sequence with L2/L3 for review load only) | memory-lane digests; B04 flush gates; E-8 behavior fingerprints | order-equivalence vs old path on randomized ledgers (property test); activate rebuild identical; index rebuilt on load == incrementally maintained |
| **L5 — S-family broad phase** | CollisionWorld uniform grid, count-gated (128), candidate order restoration, all five query arms | determinism-critical (player traces!) (design-approved lane or orchestrator) | L1 (tie contract settled) | frozen player trace fingerprints; encounter/combat/collision batteries | exhaustive small-world equivalence: for every query arm, grid path == linear path bit-for-bit over randomized worlds straddling the gate; tie cases (equal pushes, equal t) explicit |
| **L6 — D-family census index** | counting-sort compact grid + indexed census primitive; composition adoption behind population×frequency gates | determinism-critical (census order) | L1 | perception trace digest; execution stream digest; census double-run assertions | boundary/on-radius/region-edge cases; candidate-set superset property; output order == population order (property test); crossover measurement gates the default |
| **L7 — A22-core policy** | chunked auto-compaction driver config + observational counters | spine (orchestrator) | L3 (accounting shape settled) | spine-history/2 round-trips; compaction digest chain tests | chunk bound; never mid-tick; driver-opt-out keeps byte-identical saves; counter observability |
| **L8 — A05/B12 v2 format** | WorldComposition config + npc-world-save/2 region-partitioned section + identity tables + explicit domain dispatch | format/durability (orchestrator or design-first lane) | L4 (memory index stabilizes what "region block" means for memory lanes); P3 coordination for anchor/journal | ALL v1 behavior and saves byte-identical (v1 domain untouched) | v1 save loads as v1; v2 round-trip; identity table golden; dormant-region block sizes; refusal of mismatched counts |
| **M-P2 — measurement** | §9 matrix, before/after each lane, serial on the rig | measurement owner | runs alongside | E-8 cells re-run identical for no-behavior lanes | the matrix itself |

Sequencing note: L2+L3 touch `spine.rs` (sequence them); L4 touches
`npc_memory.rs` (parallel-safe); L5 touches `player.rs`/`collision.rs`;
L6 touches `npc_perception.rs`/`npc_execution.rs` + slice composition.
No two lanes share a file concurrently — the charter's conflict-free
ownership rule.

---

## 9. The measurement plan (charter §8.2, spatial row)

**Baseline anchors (already measured, E-8):** census 0.000/0.100/0.403
µs at 24/125/1000; locomotion lane 0.0088 ms/1000 with queries vs
0.0038 without; skip decomposition 12.7 ns/tick + 0.163 µs/event;
memory-per-tier table; entity-scale tick medians flat (budget-bound).
These are the BEFORE numbers; every lane re-runs the relevant harness
serially on the rig (charter §8.1: matched conditions, release, warm-up
explicit, raw samples archived with commands and exit codes).

**The spatial matrix (per family, per lane):**

1. **Densities:** census population {24, 125, 1k, 5k, 20k, 100k (index
   only — the linear arm at 100k is the point of the slope)} ×
   distribution {uniform, clustered (all-in-one-cell), ring (boundary
   saturation)} × candidate fraction {1%, 50%, 100%}.
2. **Index build/update/query slopes:** per density, measure (a)
   per-tick rebuild cost, (b) per-query cost, (c) linear query cost —
   plot all three vs N; report FITTED SLOPES (µs per 1k records), not
   single speedups. The crossover surface is
   queries-per-tick × population where (a) + q·(b) < q·(c) — report it
   as a curve, and set the runtime gates from it (§2.2's 512/4 and 128
   are placeholders until this runs).
3. **Moving actors:** the locomotion bench (E-8's
   `--benchmark-locomotion` shape) extended with the D-index
   maintenance: 24/1000 walkers × miss/hit/blocked worlds; per-NPC
   p99 must not regress above the no-index arm at sub-gate scales
   (the fast-path guarantee, measured).
4. **Static family:** collider count {8, 64, 128, 1k, 10k, 100k
   clustered city block} × each query arm {box_overlaps,
   segment_hit (axis-aligned/diagonal/long), sweep_axis ×3,
   depenetration}; grid vs linear slopes; the long-diagonal-ray
   AABB-walk waste measurement (decides the DDA trigger, §2.6);
   build cost and memory (bytes per collider).
5. **Boundary cases (correctness, not just cost):** records exactly ON
   the radius (the 3-4-5 pin class); records on cell edges and corners;
   queries spanning region boundaries (the multi-region rule, §2.2.6);
   non-finite coordinates (fail-closed); the largest-coordinate domain
   the index admits (validated, documented).
6. **Exactness evidence:** for every configuration, grid-path ==
   linear-path BIT-FOR-BIT on identical inputs (an assertion harness
   shipped as a test, not just a bench) — including the tie classes
   (equal depenetration pushes, equal t, zero-distance ties).
7. **B01 accounting:** the sustained matrix re-run (aligned/staggered,
   populations 125/1000, budgets 200) — executions, dues, digests
   IDENTICAL (the accounting must not change behavior) — plus
   adversarial dependency graphs (fan-in 1000 on one prerequisite,
   chains 1000 deep, wide diamonds): per-tick housekeeping-op counts,
   tick wall, backlog slope before/after; the hk-cap default chosen
   from the healthy ratio.
8. **A08:** region-transition cost at queued-obligation counts
   {2k, 20k, 200k} before/after L2 (O(N log N) → O(N) must show as
   slope change, and allocation counts → 0); transition-heavy presence
   movement at 8/64 regions for the L2b gate decision.
9. **A09:** ledger sizes {1k, 10k, 100k records × witnesses 24/1000}:
   witness_records/activate before/after (O(ledger) → O(log + own));
   the E-09 activation crossover re-measured through the indexed
   activate (the k=1 crossover is the accepted approximate-policy
   boundary — unchanged policy, cheaper exact path).
10. **A22-core:** compaction chunk cost curve (records cut × ms) and
    resident-ledger bytes vs records (the in-memory stride), feeding
    the retain/chunk defaults.
11. **End-to-end:** the full E-8 battery re-run per landed lane —
    every no-behavior-change lane must reproduce the E-8 cell
    fingerprints exactly (the double-run-identical discipline), and
    tick medians/p99 must not regress (charter §8.3: a small-workload
    regression cannot hide behind a large-workload win — the sub-gate
    fast paths are how we keep that promise).

**Reporting discipline:** exact commands, exit codes, tree SHAs,
artifact paths in EVIDENCE.md; slopes with sample counts and noise
floor; no speedup claims below measured noise.

---

## 10. No-change recommendations (evidence per charter §2)

The charter requires technical evidence for no-change dispositions,
not postponement. Three are made here:

1. **`waiting_on` waiter index — already adequate (B01 sub-item).**
   It exists (`BTreeMap<u64, Vec<u64>>`, spine.rs:369-370), is
   maintained at exactly the park/release sites, is derived and
   wire-invisible, and its iteration cost equals the genuine release
   work. The fan-out COST problem is the per-waiter O(deps)
   revalidation — fixed by remaining-dependency counts (§3.3), not by
   reindexing.
2. **The heap itself — keep (charter's "don't replace the event spine
   wholesale").** E-8's decomposition (12.7 ns/tick queue term +
   0.163 µs/event) shows the heap is not the cost; the (due, seq)
   BinaryHeap with the three-phase reservation IS the spine's
   identity, and every alternative (timing wheel, partitioned queues,
   B-tree queue) would fork the canonical-order discipline for an
   unmeasured win. Revisit trigger recorded (§3.3(4)).
3. **Compact sorted-grid vs incremental hash moves for family D —
   rebuild chosen, per-move updates rejected.** Evidence: the
   locomotion batch moves EVERY present NPC every tick (E-8 full
   lane; 24–1000+ movers), so incremental maintenance costs O(movers)
   hash remove+inserts with allocator/BTree churn, while the rebuild
   is one O(N + cells) counting sort serving all queries that tick —
   strictly less work at the measured motion density, and
   allocation-stable with caller scratch (the F-02 discipline).

Also recorded (charter §2's boundary requirement): the LINEAR paths
are not merely retained but DEFAULT below the measured gates (§2.2.3)
— the E-8 census numbers (0.000–0.403 µs at 24–1000) show the linear
scan is the cost/benefit optimum at every current workload; the
indexed paths exist for the scaling slopes the flat E-8 line hides.

---

## 11. Open questions for the orchestrator / owner

1. **L1 semantics sign-off:** the zero-distance tie fix aligns the
   code with the module's documented contract (lowest id,
   interaction.rs:11-13) — no existing test pins the current
   deviation (verified: no multi-target zero-distance test exists,
   interaction.rs:146-388). Confirm the documented contract is the
   intended one (it also matches combat's tie rule, combat.rs:346).
2. **The hk-cap default and whether housekeeping exhaustion belongs in
   fingerprinted `Stats` or the observational class** — this design
   recommends observational (§3.4) to avoid moving spine-fingerprint
   pins; the orchestrator may prefer the louder Stats route and accept
   a conscious re-pin.
3. **A22-core retention defaults** (retain records / chunk size /
   whether the demo opts in) are `[ASSUMPTION]` until §9.10 measures
   the resident stride and compaction cost curve.
4. **The v2 save domain's region-partitioned shape needs P3
   sign-off** (anchor/journal surface) before L8 starts — the memory
   checkpoint format (§5.2) likewise coordinates with the B04-era
   journal machinery.
5. **Gate constants (population 512 × 4 queries; colliders 128)** are
   placeholders pinned by measurement (§9.2/§9.4); the orchestrator
   should treat the measured crossover, not these numbers, as the
   contract.

---

## 12. Coverage limits (honest)

- Read fully: `npc_perception.rs`, `collision.rs` (production +
  tests), `npc_execution.rs` (production + digest area),
  `npc_locomotion.rs` (advance_batch region), `player.rs`
  (CollisionWorld + step_player region), `interaction.rs`,
  `combat.rs` (resolve_attack region), `spine.rs` (struct, queues/
  blocked/held/waiting, run_tick, drain/execute/unblock, suspension/
  reclassify/collect, compact, save/load commentary),
  `npc_memory.rs` (ledger/lane/index-relevant regions),
  `npc_world_save.rs` (section + encoder/decoder),
  `npc_behavior.rs` (NpcIdentity), `lib.rs` (constants),
  `region.rs` (transition region), `npc_memory_lane.rs` (census
  composition region).
- Read partially / not verified line-by-line: the spine save/load
  byte encoders beyond the commentary block (wire details trusted from
  the documented derived/canonical split, spine.rs:2033-2052);
  `crossing.rs`/`soak_scenario.rs`/`sustained_load.rs` (callers
  located by grep only); `behavior_measurement.rs` (harness, read via
  the E-8 log + grep); the slice demos' full census compositions
  beyond the cited lines; E-8 artifact sections taken from the
  archived log without re-running (no builds allowed in this task).
- Not examined: renderer-side spatial structures (P4/P5 scope);
  persist-side ledger growth beyond E-8's numbers (P3 scope); the
  `adversary.rs` harness beyond its query calls; WGSL/shaders.
- Claims resting on the E-8 log inherit its "pod results,
  observational" caveat — rig confirmation is the §9 plan's job.
- The brief's perception pin (`1076637108585072602`) is a
  transcription of the source pin `10766371085850726021`
  (npc_perception.rs:545); the source value governs (flagged in
  §2.2.4).

*— design complete for orchestrator review; lanes L1–L8 + M-P2 above
are the independently reviewable delivery units.*
