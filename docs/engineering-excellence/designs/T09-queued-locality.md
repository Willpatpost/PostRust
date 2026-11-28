# T09 — Localized Class-Keyed Queued Operations: Design

**Author:** T09 design lane (glm-5.3-w4a8 subagent, subordinate to the ODU
orchestrator session)
**Status:** COMPLETE — source-cited design for orchestrator review; no source
edits made (read-only lane).
**Read base:** worktree `/home/coder/PostRust-worktrees/t09`, branch
`ee/overnight-t09-queued-design`, base `c2701ee`. **All spine.rs line numbers
below cite this tree** (the audit's `2387–2419, 2483–2555` were taken at
`096f8d9`; the tree has moved substantially since — the T08 admission caps
(`903d437`/`505b0ae`), the T07 retention frontier, and the T29 design docs all
landed — and every cited region was re-resolved individually for every claim).
`c2701ee` is the integration head at dispatch, so this design composes with the
T08 admission bounds and the T07 frontier exactly as they exist now.
**Scope:** strict-audit-IV finding T09 (S1; A08/CA08) — regional queued
operations still traverse unrelated heap elements.
**Constraints honored:** DESIGN ONLY — no file under `crates/` was touched;
no cargo commands run (read-only analysis; cost claims are data-structure
reasoning over the cited code, labeled as estimates where they are
estimates — the R07/CA-17 convention); no new dependencies; no spine
replacement (the lane representation changes *inside* `spine.rs`; `drain_due`,
`execute_now`, the admission checks, and every public contract keep their
shapes); the stateful `FnMut` generic predicate stays canonical (IA-02
canonical visitation preserved on every path); wire byte-identity for identical
operation sequences (no version bump, no owner decision required by the
recommended mechanism — the one contingency that would need the owner is
PARKED in §3.5 with the decision-record shape).

---

## 0. Thesis

CA-08 gave the spine a lazily-materialized class-keyed *membership* index
(`class_index: Option<ClassIndex>`, :1334; the helpers :1489–1571; the
materialization gate :3047–3077) and with it truly localized **held** and
**blocked** operations — the held gathering measured 490× flat at 100k
(`f8de20f`). But the **queued** half of a class-keyed transition still
touches every entry in the lane: `suspend_class`/`reclassify_class`'s queue
arm (`suspend_class_queue` :3177–3205, `reclassify_class` :3294–3299) does
`into_vec` → `extract_if` over the **whole** lane buffer with a per-entry
`BTreeSet` membership lookup → `BinaryHeap::from` re-heapify of the **whole**
remainder. The canonical sort and the per-entry predicate calls are what the
index removed (the measured 3.8× — honest, not flattened); the O(lane) scan
plus re-heapify is the residual, and it is exactly what the audit names:
*for a nonempty class the entire heap converts to Vec, `extract_if` scans all
entries, re-heapifies the whole remainder*. The predicate forms
(`suspend_matching`'s `suspend_from_queue` :2892–2930,
`try_reconstruct_held`'s `partition_queue` :2756–2772) carry the same shape
plus the O(N log N) canonical sort the stateful-`FnMut` visitation contract
(IA-02) requires — correctly, and not this finding's target.

The reason the residual exists is structural, not incidental: **`BinaryHeap`
admits no keyed removal** (the CA-08 landing's own qualification). The
obligation is a lane representation that supports removing a *named* entry in
O(log N) without touching any other. This design compares the audit's three
candidate shapes — the indexed priority queue with handle/position updates,
per-class heaps with a global minimum structure, and measured lazy tombstones
with bounded rebuild and deterministic merge — **plus** the keyed-lane
representation CA-08 deliberately deferred (`f8de20f`: "an allocation-pin and
drain-determinism project, not a lane"), on complexity, cache behavior,
allocation against the spine allocation pins, (due,seq) canonical-order
preservation, save/restore byte-identity, small-workload cost, and the T08-cap
and lazy-materialization interactions.

**The recommendation:** the indexed priority queue, in its dual-representation
form — the lane field becomes `enum LaneQueue { Plain(BinaryHeap<Reverse<Pending>>), Keyed(IndexedLaneHeap) }`,
with `Plain` the literal std path a spine keeps until its first class-keyed
call (the CA-08 lazy gate extended from the index to the lane buffers) and
`Keyed` a hand-rolled, safe-Rust binary heap over the same contiguous
`Vec<Reverse<Pending>>` buffer plus a `seq → slot` positions map maintained at
every sift. A never-materializing spine — the `spine_allocations` pins'
scenario, and every workload whose driver never calls a class-keyed API —
executes the **byte-identical std code path** (pins unmoved by construction,
not by argument). A materialized spine gains O(log N) keyed extraction:
`suspend_class`'s queue arm becomes O(m log N) in the class's own membership
m — **flat in the unrelated population** — with the drain's pop path keeping
the contiguous-buffer sift locality it has today (one added L1-resident hash
probe per sift swap). The (due,seq) canonical pop order, the save bytes, the
fingerprints, and every statistic are unchanged: heap buffer layout is
already rebuild-invisible (the `restore_queue` argument :2774–2787; save
:5445–5462 and both fingerprint sections :4103–4120, :4194–4211 sort lanes
canonically), so a representation that preserves the per-lane multiset
preserves every observable. The stage-1 gate is **measurement first**: the
audit's exact slope (one active class fixed, unrelated population
1k/5k/20k/100k — the CA-08 battery measured 1k/10k/100k only, pod-observational)
plus the workload-composition warrant (do region-director deactivations at
production scale actually carry large unrelated queued populations?), with
PARK as the honest outcome if the warrant fails. The audit's explicit warning
is honored throughout: the held-side 490× evidence does **not** generalize to
queued transitions — nothing in this design claims it does.

---

## 1. The current-cost census (the queued path's actual work at `c2701ee`)

Every claim below was re-resolved at the current head. Sizes used in the
arithmetic, all **[estimate]** labels per the R07/CA-17 convention:
`Pending` ≈ 80 B (`due` 8 + `seq` 8 + `Event` ≈ 64: `EventId` 8,
`EventClass` 2 (+pad), `Schedule` 16, `payload` 8, `Field` 1 (+pad),
`depends_on: Vec` 24 — `event.rs`/`clock.rs` field census); a `BTreeSet` node
share ≈ 48 B/entry; a `HashMap<u64,u32>` entry with the FNV builder ≈ 28–48 B
inclusive of load-factor slack; `(due, seq)` is unique per lane under dense
seq allocation, so seq alone is a sound lane-local key.

### 1.1 The audit's scenario, named precisely

One active class owning **m** members queued in a lane (the audit's battery
fixes m = 40; the CA-08 fixture used the same), while the **unrelated** queued
population in that lane grows **N** = 1k / 5k / 20k / 100k. The workload
composition that makes this real: the region director's deactivation
(`engine-slice/src/region.rs:1033–1047`) calls `suspend_class(class)` between
ticks while other regions' obligations queue unrelated in both lanes.

### 1.2 `suspend_class` / `reclassify_class` — the indexed queue arm (the target)

`suspend_class_queue` (:3177–3205) and the `reclassify_class` queue arm
(:3294–3299), per lane, when the class has members in it:

| step | work | at N=100k, m=40 [estimate] |
|---|---|---|
| membership take | `mem::take` of the class's lane set | O(1) |
| `into_vec` | O(1), buffer reused, **zero allocation** | — |
| `extract_if` scan | visits **all N** lane entries; each runs `BTreeSet::contains(&(due,seq))` — O(log m) compares, ~1–3 cache misses (B-tree depth 2–3 at m ≤ ~1000) | ~100k lookups; ~100k–300k cache misses on the set; the buffer walk itself is ~100k × 80 B ≈ 8 MiB sequential |
| `BinaryHeap::from` | heapify O(N): ~N compares, ~2N `Pending` moves | ~100k compares; up to ~200k × 80 B ≈ 16 MiB memcpy traffic |

Net: **O(N + N·log m)** — linear in the unrelated population, dominated by
the scan's set lookups and the heapify's element moves. The empty-membership
early exit (:3185–3189) keeps the dormant-region common case O(1) — measured
~200 ns against 2.51 ms scan at 100k (`f8de20f`) — so the census's target is
strictly the with-members arm. The measured with-members residual is the
CA-08 landing's honest 3.8×-not-flattened: the sort (§1.3) is gone, the O(N)
scan and re-heapify are not.

### 1.3 `suspend_matching` / `suspend_ids` — the predicate form

`suspend_from_queue` (:2892–2930) per lane: `into_vec` → the **canonical
`(due,seq)` sort** (IA-02 — the stateful `FnMut` predicate must observe
candidates in canonical order, never the heap's rebuild-dependent buffer
order; this sort is contractual, not incidental) → `extract_if` with a
per-entry **predicate call** (arbitrary user code, not just a set lookup) →
`BinaryHeap::from`. Net O(N log N): at N=100k roughly 1.7M compare/swap
steps moving ~2 × 80 B each ≈ 270 MiB of memcpy traffic [estimate]. This is
the audit's original citation shape; the design does **not** propose to
remove its sort (the predicate contract keeps it — §4.6).

### 1.4 `try_reconstruct_held` — the recovery snapshot arm

`partition_queue` (:2756–2772): the §1.3 shape per lane (sort + partition),
plus `restore_queue`'s re-heapify (:2780–2787) on refusal or
`BinaryHeap::from` on success. The removal half consumes only the dropped
entries' `(class, due, seq)` — keys the membership already holds — a fact
§3.3's tombstone analysis turns on. Recovery path only; low frequency by
construction (crash windows), so it is censused but not the driver of the
recommendation.

### 1.5 `resume_class` — the already-local arm (NOT the target)

:3216–3262: the held map's keyed removal is O(log H) per member; the
re-queue pushes are O(log N) each — the canonical insert cost, correctly
paid. Flat in the unrelated held population (the 490× measurement). The
audit's instruction is honored: this arm's evidence is **not** cited as
generalizing to the queued side.

### 1.6 `collect_matching` — the read-only preparation query

:2341–2366: iterates **both whole heaps** (O(N) predicate calls) and sorts
the matches. There is no indexed queued read: the class membership holds
`(due, seq)` keys, and a `BinaryHeap` admits no keyed lookup, so even
read-only class-keyed gathering cannot resolve members to entries today.
(The region director avoids this by suspending first, then reading held —
`region.rs:1037–1047`.) The keyed representation closes this too (§4.7,
stage 3, measured-warranted).

### 1.7 Steady-state maintenance (the index-live tax, already landed)

Every schedule push, execution pop, park, release, and suspension move
performs one `BTreeSet` insert/remove in the class's own membership —
O(log m) — behind the `if let Some(index)` gate; the unmaterialized path
pays one `None` check. Whole-workload overhead measured 1.53–1.61× at
2k/20k (`f8de20f`). This is the baseline the positions map's upkeep (§4.4)
must be measured **beside**, not instead of.

### 1.8 The T08 composition facts (what changed at the head, what didn't)

The T08 admission caps (`903d437`: D/F/K; `declared_fan_in` :1372 with
`DECLARED_FAN_IN_RESERVE` :1596; `HousekeepingAdmission` :1379) bound
per-tick work at admission — they do not touch, charge, or bound the
between-ticks class-keyed transitions (the region director runs them outside
`run_tick`; they are not housekeeping units). The cost-model table landed at
`505b0ae` states plainly that T09 is "explicitly not claimed fixed" — this
design is that row's resolution. The closed-form per-tick ceiling's `log n`
term is the drain's pop cost; §4's mechanism leaves its class unchanged.
The T07 retention frontier is ledger/history-side — **zero interaction
sites** with the lane representation (the T08 design's own composition
finding, re-verified here by reading the frontier's regions :363–566).

---

## 2. What "truly localized" must preserve (the invariant set)

Any mechanism that claims the obligation must hold all of these, because
each is pinned by an existing battery or a recorded contract:

1. **The (due,seq) canonical pop order.** A min-heap over unique keys pops
   ascending; the execution ledger, lateness recording, and the fairness
   batteries pin it. A representation change must preserve the *per-lane
   multiset* exactly — pop order then follows from the key set, not the
   layout.
2. **Heap-layout invisibility.** Save (:5445–5462) and both fingerprint
   sections (:4103–4120, :4194–4211) sort lanes canonically before writing;
   `restore_queue`'s refusal path (:2774–2787) already rebuilds the heap from
   a snapshot multiset and claims every observable unchanged. Layout
   differences between representations are therefore *already* an accepted
   invisibility class — the design extends it, never relies on breaking it.
3. **IA-02 canonical visitation for the stateful predicate.** The predicate
   forms' pre-partition sort stays, on every representation, verbatim.
4. **Wire byte-identity.** Identical operation sequences produce identical
   save bytes; restore rebuilds canonical structures and every derived index
   starts `None`/`Plain`; the first class-keyed call rebuilds (the
   rebuild-equals-maintained pin, :10950–10977 shape).
5. **The allocation pins (18,6880)/(9,3024).** The pinned scenario
   (`spine_allocations.rs`) never makes a class-keyed call — the lazy gate
   is pinned as its own assertion (:11274–11280). Any mechanism whose
   never-materialized path is not *literally* today's code must argue the
   pins hold by measurement; the recommended mechanism argues it by
   construction instead (§4.3).
6. **The small-workload rule.** The audit's own boundary: don't force a more
   expensive scheduler on tiny workloads. The lazy gate is the mechanism's
   answer: a spine that never asks for class-keyed transitions must pay
   nothing beyond a discriminant check.
7. **The T08 caps and charge identity.** `waiter_visits == release_decrements
   + release_unblocks + release_not_blocked` and the admission refusals are
   untouched by any candidate below (none touches the drain's charging or
   the admission checks — verified per candidate in §3).

---

## 3. The mechanism comparison

Four shapes: the audit's three, plus CA-08's deferred keyed-lane. Axes per
the brief: complexity, cache behavior, allocation vs the pins, (due,seq)
canonical-order preservation, save/restore byte-identity, small-workload
cost, T08-caps interaction, lazy-materialization interaction.

### 3.1 The indexed priority queue with handle/position updates

**Shape.** Keep the contiguous `Vec<Reverse<Pending>>` buffer and the binary
heap over it; add `positions: HashMap<u64, u32, RemainingDepsHasherBuilder>`
(seq → slot), maintained at every sift swap. Keyed extraction: look up the
slot, swap with the last element, pop, sift-down then sift-up, updating the
positions of every moved entry — O(log N) per member, O(m log N) per
class-keyed transition. std's `BinaryHeap` exposes no sift hooks, so this is
hand-rolled — **safe Rust entirely** (swap loops with map upkeep; no
`unsafe`, no new deps).

Two implementation sub-shapes:

- **(1a) Always-custom.** Replace `BinaryHeap` outright; one code path, one
  sift implementation, the simplest invariants. Risk: the
  never-materialized path's allocation *sequence* and drain *time* now
  depend on our heap matching std's behavior. The pins measure allocation
  events/bytes of a scenario that pushes and pops through the lanes — our
  `Vec` growth policy would have to match `BinaryHeap`'s exactly (they are
  both `Vec`-backed with push-doubling, so it *should* hold — but "should"
  is a measured gate, and a pin movement is an investigation, never a silent
  repin).
- **(1b) Dual-representation (RECOMMENDED).** `enum LaneQueue { Plain(
  BinaryHeap<Reverse<Pending>>), Keyed(IndexedLaneHeap) }`. `Plain` is the
  literal std field every spine starts with and keeps until its first
  class-keyed call; `materialize_class_index` (:3047) additionally converts
  both lanes `Plain → Keyed` (`into_vec` adopts the buffer as-is — it is
  already a valid heap array — and builds the positions map in one O(N)
  pass, **at the same moment the index is already being built**, so no new
  laziness rule is invented). Conversion is one-way, like the index's
  materialization. Every queue operation site (the census: ~30 production
  sites among the 75 `near_queue`/`far_queue` references, §4.5) gains a
  `match` arm.

**Complexity.** Push O(log N) + map upkeep; pop O(log N) + map upkeep;
peek O(1); keyed removal O(log N); class-keyed transition O(m log N) —
**flat in N** (log factor only); bulk predicate passes keep today's shape
plus one O(N) positions rebuild folded into the same pass (§4.6). At the
audit's shape (N=100k, m=40): ~40 × 17 ≈ 680 swap+probe operations against
today's ~100k set lookups + ~16 MiB heapify traffic — roughly two orders of
magnitude less work [estimate; the landing measures].

**Cache behavior.** The buffer stays one contiguous `Vec<Pending>` — the
drain's sift-down walks the same few cache lines it walks today. Each sift
swap gains one hash probe (seq → u32, FNV over 8 bytes — the
`RemainingDepsHasherBuilder` precedent :1598–1640, adopted for exactly this
"cheap lookup over engine-authored integer keys" reason); the map is
~28–48 B/entry, L1/L2-resident at small m, and probed ~log N times per
operation. The pop path's locality is **identical in class** to std's; the
class-keyed extraction touches m entries' slots plus the sift path — no
unrelated entry is visited at all.

**Allocation vs the pins.** The `Plain` arm *is* std — for a
never-materializing spine the pinned scenario executes byte-identical code,
so the pins hold **by construction**, not by a growth-policy argument; the
lazy-gate assertion extends to cover the lane representation (no class-keyed
call → lanes stay `Plain` AND the index stays `None`). Once `Keyed`: the
positions map grows structurally with the live queued population — the same
price class as the class index's `BTreeSet` nodes, which CA-08 already
recorded as "the price of class-keyed transitions that touch only their own
members"; the map is pre-reserved at adoption (the
`DECLARED_FAN_IN_RESERVE` precedent — adoption happens at the first
class-keyed call, outside every measured window), grows amortized beyond
reserve, and shrinks as entries leave. The transition batteries measure the
index-live workload's allocations beside the plain one (§6, test 7).

**(due,seq) canonical order.** The heap invariant is defined on the same
keys; pops ascend canonically in both arms; keyed removal removes exactly
the named member, preserving the survivor multiset — so the execution
sequence, lateness recording, and fairness are bit-identical. Pinned by the
differential oracle (§6, test 5), not asserted.

**Save/restore byte-identity.** The per-lane multiset is identical across
representations for identical operation sequences; save and the
fingerprints sort canonically; restore always rebuilds `Plain` with `None`
index; the first class-keyed call re-adopts `Keyed` and rebuilds both the
membership and the positions map — the existing
rebuild-equals-maintained pin extended. **No wire change; no version bump.**

**Small-workload cost.** Never-materializing spines: one discriminant check
per queue op (the `class_index` `None`-check precedent — CA-08 proved this
class of check keeps the pins). Materialized small spines: sift-time map
upkeep — measured beside the 1.53–1.61× index overhead (§6, test 6); the
expectation is the same multiple class (a sift performs ~log N swaps; the
probe is one hash of a u64), stated as an expectation, gated as a bound.

**T08-caps interaction.** None. The admission checks, the four maintenance
sites of `declared_fan_in`, the charge identity, and the drain's housekeeping
accounting are untouched; the closed form's `log n` term keeps its class
(pop gains a constant-factor probe, not an asymptotic term).

**Lazy-materialization interaction.** Native: the same gate, the same
discipline, one more structure adopted at the same instant. The CA-08
equivalence battery's twins extend to drive `Plain` vs `Keyed` at identical
logical states.

**Risks.** (i) Hand-rolled sift correctness — mitigated by the
std-differential oracle (random op sequences, pop-order + multiset +
save-bytes equality; the L5 seeded-regression discipline with teeth proven
pre-landing), never erased. (ii) The dual representation is permanent
`spine.rs` complexity (~30 sites) — the price of bit-identical
never-materialized behavior; §8 records it as a standing cost. (iii) Bulk
predicate passes on a `Keyed` lane rebuild the map (O(N) added to an already
O(N log N) pass — constant-class, honest, measured).

### 3.2 Per-class heaps with a global minimum structure

**Shape.** Each (class, lane) owns its own heap; a global structure (a heap
of class minima, or an indexed heap over them) tracks each class's current
minimum so the drain still pops the true (due,seq) minimum.

**Complexity.** Push O(log C + log m); pop O(log C + log m) through the
global structure; class-keyed transition O(m log m) — localized, comparable
to §3.1's. But the global minimum structure needs *keyed updates* when a
class's minimum changes — the same problem §3.1 solves, recursed one level
down — or lazy stale minima that must be skipped (extra pops, and the skip
loop lives on the hot path).

**(due,seq) order and fairness.** Preservable in principle (the global
compare is the full key), but only through the stale-minima machinery or the
recursive indexed structure — more moving parts between every pop and its
answer than §3.1 needs for the same guarantee.

**Cache and allocation.** Many small per-class `Vec`s fragment the lane's
working set; every pop touches the global heap plus the class's Vec — worse
locality than one contiguous buffer. Allocation is the disqualifier: **one
`Vec` per (class, lane), allocated on first push** — the pinned scenario
schedules three classes inside its measured window, so eager adoption moves
the pins (18,6880)/(9,3024) for *every* workload; lazy adoption (convert at
first class-keyed call) avoids that but then the *adopted* hot path taxes
every pop with the global structure forever — including spines whose
drivers never asked for class-keyed operations after the first one — a
permanent small-workload regression against a today-cheap pop.

**Save/restore.** Multiset-per-lane preservable; restore rebuilds; same
byte-identity argument as §3.1. No advantage.

**Verdict: REJECTED.** It matches §3.1's localized-transition asymptotics
while being strictly worse on the pins (moves them eagerly, or dual-shapes
the hot path with a heavier adopted form), worse on pop cache behavior, and
worse on the small-workload rule. Nothing it wins isn't won by §3.1.

### 3.3 Measured lazy tombstones with bounded rebuild and deterministic merge

**Shape.** Keep `BinaryHeap` untouched; a class-keyed removal marks the
members' keys in a tombstone set; pops peek-and-skip tombstoned heads; a
bounded rebuild (into_vec + retain + heapify) fires when the dead fraction
crosses a threshold, amortizing removal to O(log N); "deterministic merge"
covers lane-to-lane moves.

**The load-bearing finding — the fit problem.** The spine's dominant
class-keyed transitions are **value-carrying relocations**: `suspend_class`
must insert the *`Pending` itself* into `held`, and `reclassify_class` must
re-push it into the target lane — **synchronously, between ticks** — because
the region director reads `held_class_view` immediately after suspending
(`region.rs:1037–1047`, the six-phase protocol's preparation query), and the
held count/conservation/save observables must reflect the move before the
next tick. A tombstone marks without extracting; the entry's value is
available only when it physically pops (the *next* drain, a tick later) or
at rebuild. Deferring the held insertion changes the between-ticks
observable contract — the preparation query would lie, the conservation
identity would break at the boundary, and save would need to filter
tombstoned queue entries while the held set is missing members (a wire-visible
state, or a filtered wire that diverges from the live structures). To move
the value *now*, something must find the entry in the buffer — which is the
indexed-heap problem §3.1 already solved, at which point the tombstone adds
nothing.

**Where it does fit.** Removal-shaped transitions that need only *keys*:
`try_reconstruct_held`'s queue arm consumes `(class, due, seq)` from its
dropped half (:2624–2649) — all three live in the membership — so tombstones
with a bounded rebuild could amortize the recovery path. It is a
crash-recovery path, low-frequency by construction; §1.4 censused it and
found it not the driver. A future *cancel* API (pure removal, no
relocation) would also fit.

**Hot-path cost.** `drain_due`'s peek-based due check (:4490–4496) needs
tombstone-skip logic on the head — complexity on the hottest loop, paid by
every workload the moment any tombstone exists, for a mechanism that cannot
serve the primary transitions.

**Verdict: REJECTED as the primary mechanism** — it does not implement the
obligation (localized *relocation*), only a relative of it. Recorded as the
measured contingency for the removal-shaped arm (§7, stage 3, only if
stage-1 measurements show the recovery path mattering — they are not
expected to).

### 3.4 The keyed-lane representation (CA-08's deferred option)

**Shape.** Replace the lanes with `BTreeMap<(due,seq), Pending>` (or
equivalent): push/pop/keyed-removal all O(log N); canonical iteration free;
class-keyed transitions O(m log N) — localized; **one** representation, no
dual shape, the simplest invariant story of the four. This is the shape
CA-08 named and deliberately did not take ("an allocation-pin and
drain-determinism project, not a lane" — `f8de20f`).

**Why it stays deferred.** (i) **Allocation**: every push inserts a B-tree
node share — the pinned scenario pushes through its measured window, so the
pins move for *every* workload, including the never-class-keyed ones — a
structural violation of the small-workload rule, not a tunable one. Lazy
adoption avoids it only by re-introducing the dual shape §3.1 already has,
with a worse adopted hot path. (ii) **Cache**: B-tree pop is pointer-chasing
with node merges/rebalances against the contiguous sift — a drain-p99
regression risk at large N [estimate 2–4× per pop; must be measured, not
assumed — and this design refuses to claim it cheap]. (iii) It buys over
§3.1 only the single-representation simplicity — a code-shape benefit, not
a workload benefit.

**Verdict: PARKED as the contingency** (§3.5). CA-08's deferral was correct;
this design keeps it parked and defines exactly what would reopen it.

### 3.5 The contingency and its decision-record shape (PARKED, decided nothing)

If stage-2 measurements show the positions-map upkeep multiple exceeding
the declared bound at production workloads, or the keyed extraction's win
below the warrant threshold at the region director's real cadence and
population, the honest next step is **not** to force §3.1 through: it is the
keyed-lane with pin renegotiation, taken to the owner as
**decision-t09-keyed-lane**, the decision-record shape (the T29 precedent):
1. **Durable contents** — none (the wire is unchanged in every candidate;
   the decision is engine-internal). 2. **The trade** — the allocation pins
   (18,6880)/(9,3024) renegotiated (per-push node allocation) against
   single-representation simplicity and free canonical iteration; the
   measured pop-p99 delta. 3. **The drain posture** — the hot path's
   representation permanently changes for all workloads. 4. **The activation
   policy** — eager vs lazy adoption and what the lazy gate then means.
   5. **The warrant threshold** — the stage-1/2 measured numbers that would
   justify it, stated before the fact. Per the standing rule this section
   decides **nothing**; it exists so the contingency is not improvised.

---

## 4. The recommended mechanism, specified

### 4.1 The lane type

```rust
enum LaneQueue {
    /// The literal std heap every spine starts with — kept until the first
    /// class-keyed call adopts `Keyed` (the CA-08 lazy gate, extended).
    Plain(BinaryHeap<Reverse<Pending>>),
    /// The same contiguous buffer, plus seq → slot positions maintained at
    /// every sift — O(log N) keyed extraction, no unrelated traversal.
    Keyed(IndexedLaneHeap),
}

struct IndexedLaneHeap {
    buffer: Vec<Reverse<Pending>>,          // a valid heap array at all times
    positions: HashMap<u64, u32, RemainingDepsHasherBuilder>, // seq → slot
}
```

`RemainingDepsHasherBuilder` is reused verbatim (:1598–1640) — the map is
lookup-only, never iterated for a decision, so the hash choice is
behavior-invisible by the same recorded argument. `positions` is pre-reserved
at adoption (`POSITIONS_RESERVE`, the `DECLARED_FAN_IN_RESERVE` precedent —
sized by the first stage-1 measurement, placeholder 16).

### 4.2 The sift core (safe Rust, no new deps)

`push` (sift-up), `pop` (sift-down), `remove_by_seq` (swap-with-last, pop,
sift-down then sift-up), `peek`, `len`, `iter` — each maintains `positions`
at every swap: two map updates per swap (the swapped pair), one insert per
push, one remove per pop/extraction. The comparison is the existing `Ord for
Pending` (:1099–1103) — `(due, seq)` — unchanged. No `unsafe`; no allocation
beyond the map's structural growth and the buffer's existing push-doubling.

### 4.3 Adoption and the pins, by construction

`materialize_class_index` (:3047–3077) extends: after building the index it
converts both lanes — `into_vec` (O(1), the buffer is already a valid heap
array), build `positions` in one O(N) pass, adopt `Keyed`. One-way, like the
index. The `spine_allocations` scenario never calls a class-keyed API, so it
runs `Plain` end to end — **the std code path literally** — and the pins
hold by construction. The lazy-gate assertion (:11274–11280) extends to
`matches!(self.near_queue, LaneQueue::Plain)` after the workload. Any pin
movement at landing is a stop-and-investigate, never a repin; the design
knows of no mechanism for one (the enum wraps the std heap; allocation
happens inside std on the `Plain` arm).

### 4.4 The indexed transitions, rewired

- `suspend_class`'s queue arm (:3119–3134 → `suspend_class_queue`
  :3177–3205): the empty-membership early exit stays; the with-members arm
  becomes, per member in the taken set, `remove_by_seq` → the `Pending` in
  hand → the existing held insertion sequence verbatim (:3195–3201). O(m log N).
- `reclassify_class`'s queue arm (:3294–3299): the same extraction; the
  re-lane push (:3302–3313) targets the other lane's representation
  (`Plain::push` or `Keyed::push` — the match arm), identity-preserving as
  today.
- `resume_class` (:3216–3262): unchanged except the re-queue push dispatches
  through the lane enum.
- The blocked and held arms of every transition: unchanged (already keyed
  and flat).

### 4.5 The maintenance-site census (the serialization cost, stated)

From the 75 `near_queue`/`far_queue` references, the production sites the
enum must serve: push (:2214/2215, :2838/2839, :2980/2981, :3238/3239,
:3307/3308, :4860/4861), peek/pop in `drain_due` (:4490–4499), the bulk
partitions and rebuilds (:2398/2405, :2559/2560, :2614/2615, :2650/2651,
:2961/2968, :3120/3128, :3291–3299), iteration for
collect/fingerprint/save/materialize (:2346/2352, :3052/3061, :4107–4109,
:4197–4198, :4369/4370, :4588/4592, :5416–5421, :5458, :5502), the
read-only lengths/emptiness (:1841, :1846, :1942, :4867, :4878, :4559/4560),
and the constructors/restore points (:1785/1786, :5302/5303). Read-only
sites dispatch trivially; mutation sites gain the `Keyed` arm; bulk sites
follow §4.6. This is the honest size of the change: ~30 production sites in
one file, one lane, serialized after T08 (which is landed).

### 4.6 The bulk-predicate rule (the stateful `FnMut` contract preserved)

On a `Keyed` lane, the predicate forms (`suspend_from_queue`,
`partition_queue`, `reclassify_queue`, `restore_queue`) keep their exact
shape — `into_vec`, the canonical `(due,seq)` sort, `extract_if`,
re-heapify — operating on the `Keyed`'s own buffer, then **rebuild
`positions` in one O(N) pass folded into the same re-heapify** (clear the
map, walk the buffer). No per-entry upkeep during the pass; the IA-02
canonical visitation is untouched; the added O(N) sits inside an existing
O(N log N) pass (constant-class, measured at stage 2). The stateful
predicate stays canonical — the design touches only the *indexed* paths'
locality.

### 4.7 The queued read form (stage 3, measured-warranted)

With positions, a `queued_class_view`/`collect_class` becomes possible —
membership seqs → slots → `&Pending` — O(m log m), closing §1.6's gap (the
read side cannot localize today either). Parked behind the stage-2
measurements: only if the region director's composition (or another
consumer) shows the read path hot does it land; the audit's obligation is
the removal path.

---

## 5. Preserved guarantees (the compat bar, battery by battery)

- **Every existing battery green, unchanged**: the spine's contracts,
  canonical orders, fingerprints, save bytes, and statistics are untouched;
  the only behavior delta is *which unrelated entries a class-keyed
  transition visits* — an unobservable (layout invisibility, §2.2).
- **The CA-08 equivalence battery extended, not weakened**: the five fixture
  shapes × three byte-identical twins (generic predicate / same with index
  live / indexed) gain a fourth twin-driving style — index live with `Keyed`
  lanes — asserting identical counts, canonical save bytes, all three
  fingerprint domains, suspension and housekeeping stats, and
  maintained-index-equals-rebuild, across save/restore boundaries with
  re-adoption.
- **The T08 twelve batteries**: untouched — the admission checks and their
  maintenance sites never touch the lanes' representation; the
  `declared_fan_in` recount (:2723–2733) iterates the lanes through the
  enum's read-only arm.
- **The T07 frontier**: zero interaction sites (§1.8).
- **The allocation pins**: unmoved by construction (§4.3); the extended
  lazy-gate assertion pins it.
- **No wire change**: save/restore byte-identity for identical operation
  sequences; a restored spine resumes `Plain` + `None` and re-adopts on
  demand — the retention-policy/config-class discipline (derived state never
  rides the wire).

---

## 6. The test plan (the audit's list, made concrete)

1. **The 100k unrelated-population slope** (the audit's exact sizes
   1k/5k/20k/100k — CA-08 measured 1k/10k/100k; 5k and 20k are added): one
   active class, m = 40 members fixed, unrelated queued population grown per
   size, both the one-lane and both-lanes variants. Measures: (a) **entries
   touched** — an inline `#[cfg(test)]` probe counter (the debug-tripwire
   precedent; zero production surface — the CA-08 batteries are inline in
   spine.rs's own test module) incremented in the extraction paths and the
   `extract_if` closures, asserting the with-members indexed transition
   touches exactly m entries post-stage-2 (N + m today); (b) heap-rebuild
   work — the probe counts sift swaps and heapify passes; (c) median and p99
   transition latency per size (the `class_median_ns` harness); (d) the
   slope assertions in the CA-08 shape — scan forms grow ≥ linearly
   (≥10–15× across the range), indexed forms flat-to-logarithmic
   (≤3× + headroom), including the with-members arm joining the flat set
   (today it is pinned only to "never regress against the scan form",
   :11187–11197 — the design's headline assertion replaces that honest
   residual pin).
2. **Mixed predicate/class operations and fairness**: interleaved
   `suspend_matching`/`suspend_class`/`reclassify`/`resume`/`collect` on one
   spine; answers asserted equal to the scan forms' (the equivalence
   discipline); the stateful predicate's visitation order canonical on both
   representations (IA-02 — a save→restore twin mid-cycle, re-adopting,
   must select identically); the execution ledger's (due,seq) order
   identical across representations through randomized op sequences.
3. **The differential oracle (the sift-correctness teeth)**: seeded random
   op sequences (push / pop / keyed-remove / bulk partition / restore)
   against a `std::BinaryHeap` reference twin, asserting pop-order equality,
   multiset equality, and save-bytes equality; ≥3 seeded regressions caught
   during bring-up (the L5 discipline — teeth proven before landing, the
   regressions kept as permanent cases).
4. **Save/restore/digest identity**: §5's extended equivalence battery; the
   double-run-identical discipline (E-8) over a full mixed workload with
   mid-cycle adoption; the rebuild-equals-maintained pin extended to
   `positions` (restored spine's first class-keyed call rebuilds the exact
   map the maintained one holds).
5. **Index construction and steady maintenance**: the adoption pass timed at
   each size (the one-time O(N) price, beside the index build it joins); the
   update battery (`run_class_workload` :11211) extended to the `Keyed`
   epoch — schedule/execute/park workload with lanes `Keyed` vs `Plain`,
   asserting the declared bound (today ≤6× + headroom at 2k/20k; the design
   expects the same class and keeps the battery's bound honest rather than
   loosening it silently).
6. **Allocation accounting**: a dedicated integration binary reusing the
   `spine_allocations` counting-allocator harness (its own binary — the
   global allocator is process-wide) measuring the index-live workload's
   events/bytes against the plain one at 2k/20k (the positions map's
   structural growth, named, not hidden); the pins themselves re-run green
   unmoved with the extended lazy-gate assertion.
7. **The T08 caps composition**: the admission batteries re-run green
   (they must — no interaction — but the landing proves it, not the
   argument); the charge identity asserted at storm boundaries with lanes
   `Keyed`.
8. **The audit's fairness clause**: no wall-clock-based canonical decisions
   (all timing assertions are observational witnesses with robust
   statistics — the pod-observational labels preserved; the rig obligations
   named in §8).

---

## 7. The staged landing plan (serialized on spine.rs; T08 and T07 landed)

**Stage 1 — the census and the batteries (zero production change).** The
slope battery at the audit's four sizes with the touched-counter probe; the
allocation-harness update measurement; the module cost-model doc's T09 row
gains the measured census (the `505b0ae` doc's "explicitly not claimed
fixed" note resolved to numbers). **Gates**: every existing battery and both
allocation pins green unmoved; the slope numbers recorded pod-observational;
the **warrant decision recorded** — proceed to stage 2 only if the
with-members residual at the rig-relevant shapes is material (it grows
linearly — structurally certain) AND the workload composition exercises it
(the region director's deactivation cadence against realistic unrelated
queued populations; if production-shaped workloads never carry large
unrelated queues at deactivation time, the honest outcome is PARK with the
census recorded — the audit's own small-workload rule read in the owner's
direction). 

**Stage 2 — the indexed lane.** §4 in full: the enum, the sift core, the
adoption, the indexed transitions rewired, the maintenance census, the bulk
rule; the differential oracle and the extended equivalence battery.
**Gates**: both allocation pins green unmoved (by construction — and any
movement stops the landing); the slope battery's with-members arm flat
(≤3× + headroom across 1k→100k); the update battery within its declared
bound with `Keyed` lanes; save/restore/digest byte-identity across the
extended battery; the oracle's seeded regressions caught and kept; fmt /
clippy `-D warnings` / full `engine-core` battery green with the count
movement being exactly the new batteries (the house CI-pin discipline —
measured companion corrections, never silent).

**Stage 3 — the reconciliation (measured-warranted items only).**
`queued_class_view` if the stage-2 measurements warrant (§4.7); the
`try_reconstruct_held` tombstone arm only if the stage-1 census showed the
recovery path mattering (not expected — §3.3); the module docs updated (the
cost-model table's queued-transition row, the CA-08 qualification paragraph
rewritten from "3.8× not flattened, keyed-lane deliberately not taken" to
the measured post-stage-2 truth); the register row moved to
implemented-pending-measurement with the rig obligations named (§8).

**The pins, stated once more**: untouchable throughout. The `Plain` arm is
the by-construction guarantee; the one consciously-argued exception path
(keyed-lane pin renegotiation) is §3.5's PARKED contingency, owner-gated,
decided nothing.

---

## 8. Honest limits (what only measurement settles, and what this design refuses to claim)

1. **Every number here is an estimate or a pod-observational prior.** The
   arithmetic in §1 is data-structure reasoning over cited code, labeled
   [estimate]; the CA-08 measurements (3.8×, 490×, 200 ns, 1.53–1.61×) are
   pod numbers in the test profile on shared runners. The landing measures;
   the rig confirms (the b01/q03 company: production-scale composition, the
   region director's real cadence and populations, the sustained matrix).
2. **The held-side 490× does not generalize to queued transitions** — the
   audit's explicit instruction, honored: the queued-side win is a different
   mechanism class (extraction, not keyed map lookup) and is claimed only
   through the stage-2 slope battery.
3. **The positions map is a real memory price once live**: ~28–48 B per
   queued entry [estimate] beside the class index's ~48 B/entry — the
   class-keyed price roughly doubles for spines that use class-keyed
   transitions; bounded by live obligations, vanishing with them; paid by
   the workloads that benefit, never by the ones that don't (the lazy gate).
4. **The dual representation is permanent complexity** (~30 production
   sites, two sift implementations to keep honest). It is the price of
   bit-identical never-materialized behavior; a future consolidation to a
   single keyed representation is §3.5's owner decision, not this design's
   outcome.
5. **The hand-rolled sift risk is mitigated, not erased.** The differential
   oracle and the seeded-regression discipline are the strongest teeth the
   house has, and the landing must prove them (regressions caught
   pre-landing); residual risk is a stage-2 review obligation.
6. **The workload-composition honesty**: if region deactivations at
   production scale are rare, small-population, or carried at moments when
   the unrelated queues are drained, the whole mechanism is insurance —
   which is why stage 1's warrant gate exists and PARK is a legitimate,
   recorded outcome, not a failure.
7. **No wire change is claimed beyond proof**: byte-identity for identical
   operation sequences is asserted through the extended battery, and the
   one candidate that would touch the pins (keyed-lane) is PARKED with the
   decision-record shape — decided nothing, per the standing rule.
