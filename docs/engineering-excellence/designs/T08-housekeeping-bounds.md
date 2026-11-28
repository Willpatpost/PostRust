# T08 — Bounding Scheduler Housekeeping Work: Design

**Author:** T08 design lane (glm-5.3-w4a8 subagent, subordinate to the ODU
orchestrator session)
**Status:** COMPLETE — source-cited design for orchestrator review; no source
edits made (read-only lane).
**Read base:** worktree `/home/coder/PostRust-worktrees/t08`, branch
`ee/overnight-t08-housekeeping-design`, base `5b3cbb5`. **All spine.rs line
numbers below cite this tree** (the audit's numbers were taken at `096f8d9`;
the tree has moved since — T07's frontier machinery landed — and every region
was re-resolved individually for every claim). `5b3cbb5` is the main
checkout's head at dispatch, so this design composes with the T07 frontier
(:3084-3160), the logical-head storage (:937-948), and the
CA-08 class index exactly as they exist now.
**Scope:** strict-audit-IV finding T08 (S1; B01/Q03/CA03) — housekeeping
observations do not bound scheduler work.
**Constraints honored:** DESIGN ONLY — no file under `crates/` was touched;
no cargo commands run (read-only analysis; cost claims are data-structure
reasoning over the cited code, labeled as estimates where they are
estimates); no new dependencies; no spine replacement (the audit's "don't
replace the event spine wholesale on fashion" — `drain_due`, `execute_now`,
and `try_unblock` keep their shapes; nothing is suspended that runs today).

---

## 0. Thesis

The landed B01 machinery (P2 design §3, lane P2-L3, integrated `34975f5`)
gave the drain a per-phase housekeeping *allowance* and CA-03 gave it
class-separated *observability*, but the allowance counts units that do not
bound work: a park charges one unit while registering up to
`depends_on.len()` edges; a prerequisite's synchronous release visits its
entire waiter list — of unbounded length — and charges *after* the loop, so
one execution can overshoot any allowance by any amount; `NotBlocked`
visits charge nothing; each of `run_tick`'s four drain phases gets a fresh
allowance; and handler follow-up admission is unbounded. The fix this design
recommends is **bounding admission before mutations** (the audit's first
acceptable shape), not a resumable cursor (its second): three configurable
admission caps — **D**, the per-event dependency-declaration length,
refused at `schedule` before the seq is allocated; **F**, the
per-prerequisite *declared*-dependent count, refused at `schedule` through
a derived, wire-invisible, pre-reserved `declared_fan_in` map whose
registered-waiter subset is exactly what the atomic release walks, so the
E-10 all-or-nothing release (:4076-4091, the CA-03 recorded contract) is
**preserved unchanged and simply bounded** (≤ F visits per execution)
rather than suspended; and **K**, the per-execution follow-up admission
count, cropped at emission with a typed observational counter, never
silently. With the caps, every charged unit bounds a closed-form amount of
engine work and the whole tick is bounded:
`O(B·(F + K·(D + log n)) + hk·D·log n)` with `hk ≤ 4.5·B + 4F`. The
accounting corrections (NotBlocked charged; per-phase allowances retained
with the whole-tick ceiling documented; the backlog slope published
per-tick) make the bound observable. Within-cap graphs — every existing
battery — behave bit-for-bit as today: the caps are refusals at admission,
not deferrals in the drain, so no canonical order, no event id, no
held/blocked/unfinished semantic, no wire byte, and no allocation pin
moves.

---

## 1. The defect, precisely (evidence at 5b3cbb5)

Every claim re-read at the current head; the audit's line numbers were not
reused.

1. **The charge is after the work, and the work per unit is unbounded in
   the graph.** `drain_due` (:3876-3969) loops
   `while executed < cap && housekeeping < hk_cap` (:3880). The execution
   arm calls `execute_now` and adds its return *after* the fact (:3908):
   `execute_now`'s synchronous waiter release (:4092-4108) drains the
   prerequisite's entire `waiting_on` list inside the call — the loop runs
   to exhaustion by contract (:4076-4091) — and returns the summed units.
   A prerequisite with W registered waiters costs W `try_unblock` visits
   in one atomic burst regardless of the allowance; the bound gates only
   the *next* loop iteration (:3903-3907 admits the overshoot by design).
   W is unbounded in the admitted graph: nothing stops 100k events from
   declaring one prerequisite.
2. **A park charges one unit independent of its registration fan-out.**
   The park arm (:3910-3943) charges exactly one unit (:3942) while
   `register_waiters_for_unmet` (:4144-4153) inserts one `waiting_on`
   edge per *unique* unmet dependency — O(deps · log n) map work and
   `dependency_registrations` (:729) publishes the fan-out CA-03 added
   precisely because the blend hides it. The unmet scan itself
   (`has_unmet_dependencies`, :4118-4120) is O(deps) `BTreeSet` lookups.
   `depends_on` length is unbounded at admission.
3. **`NotBlocked` visits charge nothing.** The release loop's
   `ReleaseAttempt::NotBlocked` arm (:4105) performs a real
   `remaining_deps` lookup and branch (:4170-4174) but does not increment
   `released`, so the blended total undercounts the loop's true iteration
   count (`waiter_visits`, :733). The arm is defensively unreachable
   through the public API today (the lifecycle keeps registrations inside
   the blocked set), which is why no counter pin moves when it is fixed —
   but an uncharged visit class is exactly the "observations do not bound
   work" shape the audit names.
4. **Each drain phase gets a fresh allowance.** `run_tick` (:2803-2814)
   calls `drain_due` four times — near guaranteed (`near_reserve`), far
   guaranteed (`budget − near_reserve`), near extra, far extra — and each
   call computes `housekeeping_cap(cap)` fresh (:3879). No analysis exists
   of the whole tick's total: Σ phase caps can reach `3·budget` (budget
   for the two guaranteed phases, up to `2·budget` across the two extra
   phases), so the blended whole-tick ceiling is `4.5·budget` *plus the
   per-execution overshoot of item 1, which is unbounded*. CA-03's
   `worst_tick_housekeeping` (:741-744) publishes the total; nothing
   bounds it.
5. **Follow-up admission is unbounded.** A handler may emit any number of
   follow-ups through `ExecCtx` (the scratch buffer, :4038-4065); each is
   scheduled atomically with the execution (:4051-4064 — deferral would
   reassign `next_seq`, correctly forbidden). Engine cost per admission is
   O(deps) validation (:1656-1658) + a heap push; the *count* per handler
   is handler-chosen and nothing bounds it, so one tick's engine-side
   follow-up work is unbounded even though executions are capped.

What already landed and stays: the allowance loop itself, the 1.5×
`[ASSUMPTION]` multiple (:1289-1302), the O(1) `try_unblock` through the
derived remaining-dependency counts (:4161-4168 — the P2-L3 landing), the
`waiting_on` waiter index (the P2 design §3.3(3) no-change finding), the
exhaustion signal and backlog peak (:3945-3968, :2818-2853), and the
CA-03 counter class (:723-752). This design adds what the audit's fuller
requirement demands on top of that substrate; per the audit's explicit
instruction, B01 is **not** claimed closed by any of the landed
observations, and this design alone does not close it — the landed bounds
plus the §8 measurements do.

---

## 2. The cost model (what the engine bounds vs external handler cost)

The audit demands this be explicit. With execution budget `B`, caps
`D`/`F`/`K` (§4), and `n` the pending population:

**Engine-bounded per tick (the guarantee this design closes):**

| Work class | Per-unit engine cost | Per-tick count bound |
|---|---|---|
| Execution (the drain's execute arm) | ledger push (amortized O(1)), `unfinished` remove O(log n), stats | ≤ B (:2817) |
| — its atomic waiter release | one O(1) `try_unblock` per registered waiter (:4169-4208) | ≤ B · **F** (registered ⊆ declared, §4.2) |
| — its follow-up admissions | O(D) validation + O(log n) push + O(D²) in-place dedup scan (bounded by D itself) | ≤ B · **K** |
| Park (the drain's park arm) | O(D) unmet scan + ≤ D `waiting_on` inserts (O(log n) each) + O(1) hashed counter insert + O(log n) blocked insert | ≤ Σ allowances ≤ 4.5·B |
| Heap churn | O(log n) per push/pop | ≤ (B + parks + unblocks) · O(log n) |
| Declared-fan-in upkeep | O(1) hashed entry op per site (§4.2) | ≤ B + parks, allocation-free in reserve |
| Stale entries | `NotBlocked` charged (§5.1); lifecycle keeps the arm unreachable | 0 in admitted graphs |
| Compaction (T07, already landed) | per firing: chunk-bounded SHA-256 + head bump; amortized ≤ one move per discarded record; no-policy per tick: one comparison | policy-bounded (cited, not re-designed) |
| Class index (CA-08, already landed) | O(log n) membership update per transition, riding the same counts | ≤ the counts above; lazy materialization unchanged |

**Whole-tick engine ceiling (the closed form):**
`O(B·(F + K·(D + log n)) + hk·D·log n)` with `hk ≤ 4.5·B + 4F`
(Σ phase caps ≤ 3·B, §5.3; the +4F is the per-phase terminal overshoot,
now bounded because the release is bounded). The `log n` term is the
pending population's heap/B-tree depth — the engine's per-tick work is
*population-log*, never population-linear: 100k queued unrelated
obligations cost `log(100k) ≈ 17` per pop, not a scan (the T09
regional-locality finding is a different defect and stays open).

**External handler cost (NOT engine-bounded, stated plainly):**

- **Handler bodies.** `World::execute` runs arbitrary user code inside
  `execute_now` (:4048). The engine bounds its *own* bookkeeping around
  the handler, never the handler. This is the audit's requested split,
  made explicit.
- **Follow-up count beyond K.** The handler chooses how many follow-ups
  to *attempt*; the engine admits ≤ K per execution and crops the rest
  with a typed counter (§4.3) — the handler's emission effort is its own
  cost; the engine's exposure is capped and the crop is fail-visible.
- **Driver-side costs:** world fingerprinting, enabled diagnostics (the
  `would_log` guards keep suppressed records allocation-free, :3917,
  :4016-4030), the driver's own scheduling and retention-consumer
  policy (T07's trust boundary, §8 of that design).

---

## 3. The bounding mechanism — comparison and ONE recommendation

The audit offers two acceptable shapes. Both are compared on complexity,
cache behavior, and determinism; the P2 prior art is evaluated against the
audit's fuller requirement; one is recommended.

### 3.1 Option A — bound admission before mutations (RECOMMENDED)

Three caps, every check *before any graph mutation*:

- **D — per-event dependency-declaration length**, checked in `schedule`
  beside the existing unknown-dependency validation (:1656-1658), which
  already precedes the seq allocation: `spec.depends_on.len() > D` →
  typed `ScheduleError::DependencyFanOutExceeded { observed, cap }`. The
  raw length is O(1) to check (the *unique* count is what registration
  dedups later; raw ≥ unique, so the raw cap bounds the unique fan-out).
  Before mutations means literally: no seq consumed, no queue push, no
  `unfinished` insert, no counter moves — the refused spec leaves zero
  trace (the existing `UnknownDependency` arm's shape).
- **F — per-prerequisite declared-dependent count**, checked in
  `schedule` per *unique* dependency P through the derived
  `declared_fan_in: HashMap<seq, u32>` map (§3.1.1):
  `declared_fan_in[P] + 1 > F` → typed
  `ScheduleError::PrerequisiteFanInExceeded { prerequisite, observed,
  cap }`, before the seq allocation. Declarations on *executed*
  prerequisites are harmless (a declarer whose deps are met executes
  directly and never parks — the deps-met gate :3902) and are not
  counted: the map holds an entry for P exactly while P is unfinished,
  so the check skips them naturally.
- **K — per-execution follow-up admissions**, cropped at *emission* in
  `ExecCtx` (§4.3): the ctx carries a remaining-admission counter
  initialized to K; `follow_up` beyond it is not buffered, not
  scheduled, and counted (`HousekeepingStats::follow_ups_cropped`).
  Cropping at emission (not at the drain site) also bounds the scratch
  buffer's steady-state capacity at K.

**The load-bearing proof — why F bounds the atomic release without
touching it.** Registered waiters on P (the `waiting_on[P]` list the
release loop walks, :4093) are events that *parked* while P was unmet;
every parked event declared its dependencies at schedule time and was
unfinished when it parked. So `registered(P) ⊆ declared(P) ⊆ F` at every
stable boundary, and the E-10 all-or-nothing release (:4076-4091) — the
CA-03 *recorded* contract, which explicitly reserves that any future
resumable cursor must suspend only at execution boundaries and never
inside a release loop — is preserved **verbatim**: it is bounded, not
suspended. No partial-release semantics are invented (the audit's
"no partial-release semantics invented just to satisfy a counter" —
refusal at admission invents nothing inside the drain).

**The upkeep proof (why the map has only four sites).** A declarer W of
P can leave `unfinished` only by executing, and W executes only when all
its dependencies are met — so P executed first, and P's entry was
already removed at P's execution. Therefore, while P's entry exists, the
count of W's never decreases: the sites are exactly

1. `schedule`: +1 per unique dep with a live entry (the unique scan is
   an O(D²) in-place pass, bounded by the D cap itself — no set
   allocation);
2. `execute_now`, beside the `unfinished.remove` at :4012 (before the
   handler runs, so follow-ups declaring on their just-executing
   producer skip the check — the producer's entry is already gone and
   the declaration is harmless): remove the executed event's own entry;
3. `reconstruct_held`'s replacement path (:2140-2160 area — the sites
   that already recount `remaining_deps` and `dependency_registrations`
   at :2154, :2279, :2674): the replaced identity's declarations
   transfer to the replacement's spec, through the same recount;
4. `restore`: one rebuild pass over queued+blocked+held specs
   (unique deps where the dep is unfinished), the `held_index`
   derived-rebuild precedent (:4279-4284).

Suspension is invariant by construction (a held declarer stays
unfinished; resume re-parks through the normal path) — *fewer*
maintenance sites than `waiting_on` itself. The map is derived (a pure
function of canonical specs + `unfinished`), never iterated for any
decision (point lookups/removes only — so no deterministic-iteration
requirement), never fingerprinted, never serialized: wire-invisible by
construction, the `remaining_deps` precedent (:4281).

**A rejected sub-variant, recorded with its reasoning.** Capping fan-in
at *park* time (defer the over-cap park: the event stays queued under
its original `(due, seq)`) was analyzed and rejected: the deferred event
re-pushes with unchanged priority and re-pops at the head of its lane;
if the prerequisite sits behind it in `(due, seq)` order — the exact
adversarial shape, the whole point of the cap — the lane livelocks
(pops, defers, re-pushes, forever; the prerequisite never executes, the
waiter never parks). Bounding at admission avoids the livelock class
entirely: the over-cap event never exists.

### 3.1.1 Allocation-pin safety (the constraint that shaped the map)

The pins (spine_allocations.rs:178, :186 — (18, 6880) cold / (9, 3024)
warm) are untouchable. Read in full, the windows open *inside*
`run_wave` (:271) **after** spine construction (:331) and contain one
dependency-declaring schedule (the two-dependency blocked event, :285)
plus 40 ticks. Therefore:

- `declared_fan_in` is pre-reserved at **construction**
  (`DECLARED_FAN_IN_RESERVE = 16`, the `REMAINING_DEPS_RESERVE`
  precedent at :1211-1222 — the reservation moves the structural cost
  outside every measured window, the pin module's own documented
  discipline). The scenario touches 2 distinct unfinished prerequisites
  — within the reserve, so no window allocation, cold or warm.
- The steady-state ops (entry insert/remove/lookup) are allocation-free
  within reserve, the `RemainingDepsHasherBuilder` reuse (:1224-1266).
- The D check reads `spec.depends_on.len()` — O(1), no allocation. The
  F unique-scan is in-place. The K crop is an integer counter.
- `ExecCtx` is constructed only at spine.rs:4041 across the workspace
  (grep-verified at this base) — adding the remaining-admission field
  is compile-checked; if an external construction site ever appears, it
  fails to build loudly rather than silently mis-defaulting.

### 3.2 Option B — deterministic resumable continuation cursor (analyzed, REJECTED)

Shape: the release loop suspends at `(prerequisite seq, waiter-list
index)` when the allowance trips; pending releases drain (a) within the
tick while allowance remains (same-tick readiness for healthy graphs)
and (b) across ticks otherwise. Evaluated against the audit's fuller
requirement:

- **It does not bound the park side at all.** A single park's
  registration burst (≤ D edge inserts) is one uninterrupted mutation
  batch; making *it* resumable means half-registered parks
  (`remaining_deps` seeded late, `waiting_on` partial), and a restore
  mid-partial-park strands edges. The audit's "bound atomic fan-out …
  before mutations" still requires an admission cap for declarations —
  **D is needed under either option**; the cursor only replaces F's
  function. The real comparison is F-at-admission vs
  release-cursor-for-fan-in alone.
- **It versions a recorded contract.** spine.rs:4076-4091 records, as
  *decided* contract, that the release is all-or-nothing per execution
  and that any future resumable cursor suspends only at execution
  boundaries — never inside a release loop. Option B suspends inside
  the release loop by definition; the `waiter_visits ==
  decrements + unblocks + not-blocked` identity becomes per-suspension.
- **Save/restore semantics fork.** The cursor cannot ride the wire
  (byte-identity for identical canonical state is the ratified
  discipline). Completing pending releases at the *save* boundary
  mutates queues/blocked before encoding; completing them at *restore*
  re-derives counts that the live event-driven decrements already
  consumed (the double-decrement hazard: the re-derived
  `remaining_deps` already excludes executed prerequisites, so
  replaying a suspended release would decrement again — the eager
  completion must re-validate, not resume). Either fork breaks the
  "save→restore→continue == uninterrupted control" pairing unless the
  test oracles carry the save schedule explicitly — a widening of the
  determinism surface.
- **Cache behavior:** the waiter list is a `Vec<u64>` walked
  sequentially; a cursor resumption re-walks cold memory next tick.
  Option A never creates the over-cap list at all — the refused
  declarer never enters the structure. Roughly a wash on cache, A wins
  on structure count (no pending-release queue, no cursor field).
- **Determinism:** B changes the execution order of over-allowance
  release storms (the `adversarial_release_storm_is_atomic_bounded_and_
  deferred` battery, :5960, re-pins); A changes only the admission of
  over-cap graphs — at the defaults (§4) **no existing battery trips**.

**Recommendation: Option A.** It is the audit's own first-listed shape,
needs no new loop state, no wire surface, no contract versioning, keeps
the E-10 release atomic and bounded rather than suspended and
interruptible, and leaves every existing battery green at the defaults.
B's single advantage — admitting arbitrarily concentrated fan-in without
refusal — is precisely the admission the audit asks the engine to bound;
the typed refusal surfaces it to the driver at the producer, where the
driver can raise F (a config) or restructure its graph, instead of the
engine silently rate-limiting releases across ticks.

### 3.3 The P2 prior art (§3 of `P2-simulation-spatial.md`) against the fuller requirement

The P2 design's B01 section delivered: (1) the second loop bound
(`while executed < cap && housekeeping < hk_cap`) with the 1.5×
`[ASSUMPTION]` allowance — **kept**; (2) the derived
remaining-dependency counts making `try_unblock` O(1) — **kept**, and
load-bearing here (it is what makes F bound *work*, not just visits);
(3) the `waiting_on` index kept as the irreducible per-waiter release
path — **kept**; (4) partitioned queues/timing wheels rejected —
**unchanged**. What the prior art explicitly did not claim — and the
audit correctly refuses to call B01 closed over — is the bound itself:
the allowance counts parks and *charged* attempts, but the units
undersample real work (items §1.1-1.5 above). This design keeps the
prior art's accounting substrate and adds the admission bounds that
make each unit's work closed-form. The P2 design's own §3.3(2) derivation
("the healthy dependency-graph ratio at roughly one unit per event")
remains the reason the 1.5× multiple and the one-unit park granularity
must not change (§5.2).

---

## 4. The recommended mechanism, specified

### 4.1 Configuration surface

`HousekeepingAdmission { max_dependencies: NonZeroU32,
max_declared_dependents: NonZeroU32, max_follow_ups: NonZeroU32 }` with
defaults **D = 64** (raw declaration length), **F = 4096** (declared
dependents per prerequisite), **K = 1024** (follow-up admissions per
execution). Config-class state in the retention-policy tradition
(:4289-4293): set through an additive `set_admission_bounds(&mut self,
…)` (no constructor churn — zero call-site movement), never
fingerprinted, never serialized; a restored spine resumes at the
**defaults** and the driver re-configures — the documented
retention-policy precedent, with the same one-paragraph caveat in the
public docs (a world saved under non-default bounds and restored without
re-configuring behaves under the defaults afterward; the divergence is
observable in world content, never in spine fingerprints, because config
was never fingerprinted).

Default justification (measured-actual discipline): every existing
fixture's observed maximum sits comfortably under each default — the
largest declared fan-in in the workspace is 2048 (tests/lanes.rs:294-330,
the P2-L3-corrected herd at budget 2100/2050), the largest dependency
list is 3 raw entries (the same fixture's `[first, last, first]`), the
largest per-execution follow-up count is 3 (the allocation scenario's
fanout class, spine_allocations.rs:222-224). D=64/F=4096/K=1024 leave
2-500× headroom over every battery while keeping the §2 ceiling
closed-form; re-pinning is a `[ASSUMPTION]`-protocol measurement
obligation (§8), never a silent change.

### 4.2 `declared_fan_in` — the derived map

`HashMap<u64 seq, u32 count>` with `RemainingDepsHasherBuilder`, reserve
`DECLARED_FAN_IN_RESERVE = 16` pre-reserved at construction (§3.1.1).
Invariant (proven §3.1): at every stable boundary, `declared_fan_in[P]`
= the number of unfinished events whose `depends_on` declares P,
for every unfinished P; entries for executed P do not exist; the map is
suspension-invariant. Upkeep sites: the four of §3.1. A
`declared_fan_in_matches_revalidation(&spine)` test helper (the
`remaining_counts_match_full_revalidation` precedent, :5899) asserts the
invariant by full re-derivation at every battery boundary.

### 4.3 The follow-up crop

`ExecCtx` gains `admissions_remaining: u32` and `crops: u32` (private,
compile-checked per §3.1.1); `follow_up` decrements/buffers while
admissions remain, else increments `crops`. `execute_now` adds `crops` to
`HousekeepingStats::follow_ups_cropped` after the drain. The crop is a
distinct class from `invalid_dropped` (fingerprinted Stats, :4056 —
dependency-shape refusals the handler could have fixed) : a crop is a
*config* outcome, so it belongs to the observational class (the P2
design §3.4's own recommendation that new counters stay out of
fingerprinted Stats to avoid touching digest pins). Never silent: the
counter plus an `Error`-level diagnostic line under the `would_log`
guard.

---

## 5. The accounting corrections

### 5.1 `NotBlocked` charged as a visit

The release loop's `NotBlocked` arm increments `released` (and a new
observational `HousekeepingStats::release_not_blocked`). Every waiter
visit then charges exactly one unit, making the CA-03 identity —
`waiter_visits == release_decrements + release_unblocks +
release_not_blocked` — the *charge* identity, mechanical at every
boundary (spine.rs:4086-4091 names exactly this surface). No existing
fixture reaches the arm (it is defensively unreachable through the
public API; every current battery's visits already equal decrements +
unblocks — e.g. :6458-6468, :6646-6648), so no counter pin moves; the
implementation verifies this by running the full battery before
committing (the investigation-not-silent-repin rule).

### 5.2 The park charge stays one unit — the conscious rejection of per-edge charging

Charging registration edges as units would trip the healthy shape: the
`healthy_ratio_never_trips_the_allowance` battery (:6027-6058) parks
1000 single-dependency waiters in one tick at allowance 1650 — per-edge
charging makes that 2000 units and the battery's pinned
`blocked_count == 1000` fails. The one-unit granularity is the P2
design's own healthy-ratio derivation (:1281-1287) and **stays**. What
changes is the unit's *meaning*: under D, one park unit bounds
`O(D·log n)` registration work — a closed form, no longer a hope. The
per-edge truth stays published (`dependency_registrations`, :729) and
gains a per-tick worst (`worst_tick_dependency_registrations`,
mirroring :741-744) so the edge slope is observable without tripping
anything.

### 5.3 Per-phase allowances retained; the whole-tick ceiling documented

A single whole-tick housekeeping allowance would let one lane's storm
consume the other lane's guaranteed housekeeping — the same fairness
argument that keeps the execution reservation per-phase (:1303-1307).
**Per-phase stays.** What is added is the analysis the audit demands:
Σ phase caps = `budget + 2(budget − e_guaranteed) − e_extra ≤ 3·budget`,
so the blended whole-tick housekeeping is bounded by `4.5·budget + 4F`
(the terminal overshoot per phase, bounded because the release is), and
— with the caps — the whole-tick *engine work* is the §2 closed form.
`worst_tick_housekeeping` (:741) now measures a quantity that is
actually bounded; the doc comment on `housekeeping_cap` (:1268-1310)
gains the ceiling paragraph and the version note that the bound's
meaning strengthened (each unit now bounds bounded work) while its
number and granularity did not.

### 5.4 Backlog-slope observability

Today only the cumulative `exhaustion_backlog_peak` (:745-751) exists;
the slope requires per-tick sampling of `near_pending_count`/
`far_pending_count` (the :6613-6635 batteries do exactly that by hand).
`LaneReport` (the struct at :660-674) gains `housekeeping_backlog_near:
u64` and `housekeeping_backlog_far: u64` — the lane queue lengths recorded at each
exhaustion break, per-tick reset, observational class (never
fingerprinted, never serialized — the :6391-6403 restore-defaults proof
extends verbatim). Additive fields; the implementation greps for
exhaustive `LaneReport` struct literals outside `run_tick` (none known;
the batteries read named fields) before landing.

---

## 6. Preserved guarantees (the untouchable-invariants argument)

1. **Canonical `(due, seq)` order.** Nothing in the drain's pop order,
   park priority, release order (registration order = the `(due, seq)`
   pop order at park time, :4079-4082), or queue discipline changes.
   Within-cap graphs are bit-for-bit identical — the compat proof is
   *every existing battery green unchanged* at the defaults. Over-cap
   graphs are a new admission contract (point 8).
2. **Event IDs / dense `next_seq`.** The D and F refusals sit beside the
   unknown-dependency validation (:1656-1658), *before* the seq
   allocation — a refused spec consumes nothing. The K crop never
   reaches `schedule`. No partial identity is ever minted.
3. **Held/blocked/unfinished semantics; conservation.** Untouched: the
   caps act at admission, so the `scheduled == executed + queued +
   blocked + held` identity (:4348-4350) holds at every boundary — a
   refused or cropped event simply never enters `scheduled`.
4. **Save/restore byte-identity.** No wire change; `declared_fan_in` is
   derived and rebuilt at restore through the canonical-spec pass
   (:4279-4284 class); the config is never serialized (the
   retention-policy precedent :4289-4293); the restored spine's
   rebuild-equals-maintained invariant is pinned by the §7 round-trip
   battery. `spine-history/1/2`, every domain gate, and every digest
   pin are untouched by construction.
5. **The housekeeping allowance semantics.** The 1.5× `[ASSUMPTION]`
   multiple (:1289-1302), the one-unit park/release granularity, the
   exhaustion signal class (:6337-6404), and the zero-cap-phase no-op
   are all unchanged — the design *strengthens* what a unit bounds
   without changing what a unit costs. The version note in §5.3 records
   exactly this, so the change is conscious, not silent.
6. **The E-10 atomic release / the CA-03 recorded contract.** Preserved
   verbatim (:4076-4091): the release stays all-or-nothing per
   execution, in registration order, with no observable state between
   iterations — bounded by F at admission rather than suspended by a
   cursor. The `waiter_visits` identity becomes the charge identity
   (§5.1) and is asserted at every battery boundary.
7. **Near/far fairness.** The three-phase reservation arithmetic
   (:2803-2814) and the per-phase allowances are untouched; the caps are
   lane-blind (a prerequisite's declared dependents count across both
   lanes — the cross-lane battery pins it).
8. **The admission contract change, versioned.** Over-cap graphs —
   `depends_on.len() > D`, declared dependents > F, follow-ups > K per
   execution — move from "silently admitted, unbounded engine work" to
   "typed refusal / typed crop, counted and logged." This is the
   audit-sanctioned shape ("for admitted graphs … bound … before
   mutations"), documented in the public `schedule`/`follow_up` contract
   with the config knobs and their defaults, and pinned by new
   batteries that prove the exact accepted execution sequence of the
   within-cap graph and the exact typed outcomes of the over-cap graph.
   No wall-clock input anywhere: every decision is a pure function of
   canonical state + config (§7.12).

Compositions preserved: **T07 frontier** — compaction fires at the tick
boundary (:2882-2891) after every drain phase; the admission caps are
scheduler-side config in the same never-serialized class; zero
interaction (the retained ledger grows only through executions, already
bounded by B). **CA-08 class index** — the caps act at `schedule` (an
event that never exists never touches the index) and at `ExecCtx`
emission (before any scheduling); no new membership sites; lazy
materialization unchanged. **No new dependencies** — std only.

---

## 7. The test plan

Every test names its oracle; all in the spine.rs inline module (the
CA-07/B01 precedent) except where noted. The compat bar — **every
existing battery green unchanged** — is itself test item 0:

0. **The compat bar (untouchable, named):** the eleven inline
   housekeeping/derived-count batteries —
   `adversarial_parking_storm_is_housekeeping_bounded` (:5868),
   `adversarial_release_storm_is_atomic_bounded_and_deferred` (:5960),
   `healthy_ratio_never_trips_the_allowance` (:6027),
   `duplicate_and_held_dependencies_count_correctly` (:6070),
   `release_fanout_stays_once_and_in_order_under_the_allowance` (:6163),
   `save_restore_rebuilds_remaining_counts_byte_identically` (:6224),
   `derived_counts_survive_suspend_resume_and_reconstruction` (:6273),
   `housekeeping_exhaustion_is_observational_only` (:6345),
   `housekeeping_counters_separate_parks_registrations_and_releases`
   (:6417),
   `blocked_work_survives_phase_boundary_save_restore_with_identical_
   counters` (:6490),
   `housekeeping_publishes_worst_tick_work_and_the_exhaustion_backlog`
   (:6584) — plus tests/spine_bookkeeping.rs (all three),
   tests/lanes.rs `shared_prerequisites_release_fanout_once_in_order`
   (the 2100/2050 conscious pin), tests/determinism.rs,
   tests/save_roundtrip.rs, tests/spine_history.rs, tests/suspension.rs,
   tests/spine_contracts.rs, and the spine_allocations pins
   ((18, 6880)/(9, 3024), cold and warm). None of these changes source
   or pin values.
1. **Fan-out refusal (D):** `dep_spec` with D+1 dependencies → typed
   `DependencyFanOutExceeded`; **before-mutations proof**: `next_seq`,
   both queues, `unfinished`, `stats.scheduled`, every
   HousekeepingStats counter, and `declared_fan_in` (via the
   revalidation helper) unchanged by the refused call; at exactly D →
   admitted. Duplicates: 3 raw entries with 2 unique at D=2 → admitted
   (raw-length check, unique registration — the lanes.rs fixture's
   shape).
2. **Fan-in refusal (F):** F+1 declarers on one prerequisite → the
   (F+1)-th `schedule` returns `PrerequisiteFanInExceeded`; the first F
   admitted; before-mutations proof as in 1. **The bound proven:**
   configure F small (e.g. 8) with a herd — `waiter_visits == F`
   exactly at the prerequisite's execution, `worst_tick_housekeeping`
   reflects ≤ F + parks, no overshoot beyond F anywhere (the audit's
   per-tick maximum work, pinned as an exact integer, not a
   wall-clock).
3. **Declarations on executed prerequisites are free:** schedule
   declarers on an already-executed event (the follow-up-on-producer
   shape, :3997-3999) at F=1 — no refusal, they execute directly, never
   park; `declared_fan_in` stays empty.
4. **Held and cross-lane counting:** a held declarer counts (a
   prerequisite's entry survives suspension of its declarers); declarers
   in both lanes count on the same prerequisite (the cross-lane oracle);
   a declarer resumed from held re-parks through the normal path with
   the count unchanged throughout (suspension-invariance pinned).
5. **The map invariant:** `declared_fan_in_matches_revalidation`
   asserted at every boundary of batteries 1-4 and the storms
   (the `remaining_counts_match_full_revalidation` discipline, :5899).
6. **`reconstruct_held` transfer:** the replacement identity's
   declarations recounted (the :2140-2160 site); the revalidation helper
   green across the reconstruction.
7. **Save/restore:** mid-herd save → restore → byte-identical save
   bytes; the rebuilt map equals the maintained map (revalidation); the
   restored spine (config at defaults) continues the herd to completion
   with the canonical order preserved (the :6224/:6490 disciplines
   extended); a spine configured under non-default bounds documents the
   config-reset behavior (world-content divergence observable, spine
   fingerprints identical).
8. **Follow-up crop (K):** a handler emitting K+1 follow-ups → K
   admitted (the world's stream reflects exactly K children),
   `follow_ups_cropped == 1`, `invalid_dropped == 0` (the classes
   discriminate); the scratch buffer's steady capacity stays ≤ K across
   a sustained run (the warm-window discipline); a handler emitting ≤ K
   (SustainedWorld's 1-per-wake, the allocation scenario's 3) — zero
   crops, every existing sustained battery green.
9. **Blocked storms / cycles / refusals:** cycles remain structurally
   impossible (the unknown-dependency gate — forward references
   refused, typed); a blocked-only graph under held roots with over-cap
   declarers: the refused declarers never exist, the admitted graph's
   `run_to_completion` behavior unchanged (the :4221-4223
   no-progressible-work rule); the 600-storm and 90-herd batteries
   unchanged at default F (compat bar 0) and re-run at configured F=256
   with the exact new accepted sequence pinned (admission-order
   refusal points, release order, execution order — integers, no
   wall-clock).
10. **100k unrelated obligations at fixed budget:** 100k independent
    events, budget 200 — drains over ≥ 500 ticks; per-tick
    `worst_tick_housekeeping ≤ 4.5·budget`; `near_pending_count`
    monotone decreasing (the slope oracle); zero parks, zero
    registrations, zero refusals (unrelated ⇒ no caps touched — the
    audit's fixed-budget case, proving the caps cost nothing when
    unused). The per-tick `LaneReport` backlog fields carry the slope
    without hand-sampling.
11. **The composite per-tick maximum:** a storm + herd +
    follow-up-emitter world at small configured caps — after every
    tick, the published tick housekeeping ≤ the §2 ceiling evaluated at
    (B, D, F, K) — the closed form pinned as an integer bound on a
    real adversarial composition.
12. **No wall-clock canonical decisions:** every battery asserts exact
    accepted sequences / exact typed outcomes; the design adds no
    clock read beyond the existing due comparisons; wall-clock appears
    only in the §8 measurement obligations, as evidence, never as a
    canonical decision (the E-8 discipline).

---

## 8. Honest limits (what only measurement at implementation time settles)

- **The 1.5× multiple remains `[ASSUMPTION]`** (:1289-1302). The §2
  ceiling is closed-form in the caps, but whether 1.5× keeps healthy
  sustained shapes untripped at production fan-in mixes is a
  measurement: the sustained matrix + adversarial graphs re-measure the
  housekeeping-per-execution ratio and re-pin the multiple under the
  placeholder protocol. This design deliberately does not touch it.
- **The default caps are judgment, not measurement.** D=64/F=4096/K=1024
  exceed every workspace fixture's observed maximum with headroom, but
  whether production worlds fit under them — or prefer tighter caps with
  visible refusals — is driver policy the observability surfaces
  (refusal counts, crop counts, backlog slope) make conscious. The
  defaults must be re-examined at the landing measurement.
- **F caps *declared* fan-in, not registered.** The declared count
  upper-bounds the parked waiter list conservatively: a declarer that
  executes before ever parking (its prerequisite executed first) still
  consumed admission budget while both were unfinished. Whether the
  conservative gap matters in real workloads is measurable
  (`dependency_registrations` against the map's counts); a
  registered-count cap would reintroduce the park-time livelock class
  (§3.1) and is rejected pending evidence it is needed.
- **Handler bodies remain external and unbounded** — by design and by
  the audit's own split. K bounds the engine's exposure and the crop is
  fail-visible, but a handler that builds a giant structure before
  emitting one follow-up is the driver's cost, not the spine's.
- **Restore of legacy over-F saves.** The restore rebuild is faithful
  to canonical state and does not enforce the caps (restore is
  fail-closed on *corruption*, not config); a pre-cap save with a
  >F waiter list performs the legacy atomic release once, bounded by
  the saved graph's actual fan-in. Documented at the restore contract;
  a restore-time enforcement was considered and rejected (it would
  either strand waiters or mutate canonical state at load).
- **`NotBlocked` is defensively unreachable through the public API
  today** — the charge correction is accounting completeness; no
  black-box battery can exercise the arm. The identity assertion
  (§5.1) covers it structurally; if a future path makes it reachable,
  its charge is already correct.
- **The whole-tick ceiling is a bound, not an observed actual.** The
  batteries pin actuals at fixture scales (≤ 600-herd, 100k unrelated);
  production-scale adversarial composites at the ceiling are a
  rig/measurement obligation, not a test-bed claim.
- **T09 (regional queued-operation locality) is not addressed here and
  must not be assumed fixed by T08** — the 100k battery's per-tick
  bound is population-*log*; the class-keyed removal path's
  whole-heap behavior is a separate finding with its own design.
- **The `LaneReport` additive fields and the `ExecCtx` private fields
  carry small compile/migration risk** (exhaustive struct literals,
  external constructions) that the landing verifies by build and grep —
  estimated zero sites at this base, verified, not assumed.

---

## 9. The staged landing plan

Serialized on spine.rs after the T07 landing (same file; the orchestrator
sequences any in-flight spine.rs lanes — T09 if dispatched — before or
after these stages, never interleaved). Base `5b3cbb5`. Three commits,
each with its own gate; a stage gate failure blocks the next stage.

**Stage 1 — the accounting corrections (pure accounting + observability,
zero behavior change).** NotBlocked charged + `release_not_blocked`;
`worst_tick_dependency_registrations`; the `LaneReport` at-break backlog
fields; the `housekeeping_cap` doc gains the whole-tick ceiling paragraph
and the version note. Coverage extends *in place* (the counters battery
:6417 gains the three-way identity assertion; the worst-tick battery
:6584 gains the per-tick report fields' assertions) — no new test
functions, so even the CI shard-count pins stay untouched.
**Gate:** the full compat bar (§7.0) green unchanged; both allocation
pins byte-exact (u64 writes only); engine-core test count unchanged.

**Stage 2 — the admission bounds.** D at `schedule` (typed error, raw
length); F through the derived pre-reserved `declared_fan_in` map (typed
error, the four upkeep sites of §3.1, the revalidation helper); K at
`ExecCtx` emission (`follow_ups_cropped`, the diagnostic line);
`set_admission_bounds` config; the §2 cost-model table into the module
docs. New batteries §7.1-§7.12 (new test functions — the main-shard
count pins move; the companion correction commit records the measured
actuals with each new battery named, the lifecycle lane's 1654→1661
discipline).
**Gate:** the full compat bar (§7.0) green **unchanged at the defaults**
— the storm, herd, healthy-ratio, and lanes-fixture batteries are the
load-bearing witnesses that within-cap graphs are bit-for-bit identical;
both allocation pins byte-exact (§3.1.1's construction-reserve and
steady-state arguments, verified by the run); the revalidation helper
green at every new battery boundary; fmt/clippy clean.

**Stage 3 — reconciliation and measurement obligations.** The register
rows (T08; B01/Q03/CA03 rows updated to *implemented-pending-measurement*
— B01 is **not** claimed closed by observations, per the audit's explicit
instruction; it closes only when the landed bounds plus the §8
measurements exist); README; this design's status flip; the measurement
obligations named for the next rig window (the sustained matrix +
adversarial graphs re-pinning the 1.5× `[ASSUMPTION]`, the default-cap
headroom audit, the composite per-tick actual at production scale).
**Gate:** docs-only diff; the count-gate selftest green (the T01 lane's
control plane).

**Untouchable, named once more:** the spine allocation pins
(18, 6880)/(9, 3024) cold/warm (spine_allocations.rs:178/:186) and every
battery in §7.0. The lanes.rs 2100/2050 budget is itself a *pin* of the
landed B01 semantics — Stage 2's defaults exceed its 2048 declared
fan-in precisely so it never moves again.

---

*Prepared for the ODU orchestrator session. Every spine.rs citation above
was resolved by direct read at base `5b3cbb5` in
`/home/coder/PostRust-worktrees/t08`; the audit's 096f8d9-based line
numbers were not reused without re-resolution. Costs reasoned from the
cited data structures are estimates until the §8 measurements run.*
