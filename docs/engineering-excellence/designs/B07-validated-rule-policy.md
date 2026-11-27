# B07 — Rule-Table Validation on the Hot Evaluation Path: Design

**Finding:** B07 (DIRECTIVE §6) — `evaluate_wake`/`evaluate_witness` call
`validate_rule_table` on every invocation.
**State (FINDINGS.md):** CONFIRMED — npc_execution.rs:164 and :246,
orchestrator grep-verified.
**Status of this document:** DESIGN ONLY. No implementation in this task.
Read-only over `crates/**`; this file is the deliverable.
**Author:** design subagent (glm-5.3-w4a8), 2026-11-26, branch
`refactor/engineering-excellence`.
**Sources read:** `crates/engine-core/src/npc_behavior.rs`,
`crates/engine-core/src/npc_execution.rs`,
`crates/engine-core/src/npc_locomotion.rs` (test module),
`crates/engine-slice/src/corner_behavior.rs`,
`crates/engine-slice/src/behavior_measurement.rs`,
`crates/engine-slice/tests/render_instanced.rs`,
`crates/engine-core/tests/spine_allocations.rs` — all citations below are
line-exact against the working tree at the time of reading.

---

## 1. The finding, restated precisely

Both public evaluation entry points validate the caller's rule table on
every call, before any evaluation work:

- `crates/engine-core/src/npc_execution.rs:164` — inside `evaluate_wake`
  (signature at :147-156): `if validate_rule_table(rules).is_err() {
  return None; }` — the fail-closed admission refusal.
- `crates/engine-core/src/npc_execution.rs:246` — inside `evaluate_witness`
  (signature at :235-243): `validate_rule_table(rules).ok()?;`.

The adjudicated question (DIRECTIVE §6 B07): a validated immutable policy
handle may move invariant checks out of hot evaluation — *compare
validation cost, allocation behavior, API usability, and
mutation/version handling*, and **do not introduce a complicated
type-state architecture solely to remove one tiny loop unless it also
clarifies contracts** (the charter bar, restated in §8 below).

## 2. What `validate_rule_table` actually costs — by direct read

The function (`crates/engine-core/src/npc_behavior.rs:278-287`):

```rust
pub fn validate_rule_table(rules: &[Rule]) -> Result<(), &'static str> {
    for rule in rules {
        if let Some(need) = rule.need
            && need as usize >= MAX_NEEDS
        {
            return Err("npc/rule-need-index-out-of-range");
        }
    }
    Ok(())
}
```

Measured by reading (no benchmark run — this task is design-only; the
operation count is small enough to establish by inspection):

- **Operations per rule:** one `Option<u8>` discriminant test, one
  `u8 → usize` widening, one integer compare against `MAX_NEEDS = 8`
  (`npc_behavior.rs:161`). Early return on the first offense.
- **Allocation behavior: ZERO.** No heap traffic, no hashing, no sorting,
  no RNG draws, no formatting. The `&'static str` error is a constant.
  This matters against the repo's allocation gates: the spine main-path
  pin (`crates/engine-core/tests/spine_allocations.rs:178,186` —
  `(18, 6880)` cold / `(9, 3024)` warm) counts every engine-side
  allocation event; `validate_rule_table` contributes zero events on
  every path it touches.
- **Table sizes in every real workload: ZERO OR ONE RULE.** See the
  caller inventory (§3). A one-rule table is ~28-32 bytes (`Rule` is
  `Copy`: `ActivityKind` enum ≈ 12 B + `Option<u8>` 2 B + `u8` 1 B +
  `f32` 4 B + `Option<WitnessMatcher>` 8 B, with padding — fields at
  `npc_behavior.rs:188-201`) — one cache line touched, one loop
  iteration. The check is a low-single-digit-nanosecond operation.
- **Comparison against the work around it:** the selector's own
  per-rule work is strictly larger — `select_activity`
  (`npc_behavior.rs:377-445`) walks the table calling
  `Rule::applicable` (:218-239: a bounds-checked `needs.get`, a
  `partial_cmp` on f32, a witness scan) plus an f32 score
  (:403), and walks the table a SECOND time on ties (:426-437).
  `decay_needs` runs 8 f32 multiply-adds per evaluation
  (`npc_behavior.rs:469`). Removing the validation loop from a
  one-rule-table wake removes nanoseconds from a path that then still
  performs decay, view construction, RNG seeding state copies, and
  spine bookkeeping around it.

**One structural observation that cuts BOTH ways:** on the wake path,
`evaluate_wake` constructs the view with
`due_commitment: Some(commitment)` (`npc_execution.rs:186-189`), and
`select_activity` short-circuits on a due commitment BEFORE walking the
table (`npc_behavior.rs:388-393` — "a due commitment always wins"). So
today the validation loop is the **only** table iteration the wake path
performs. Removing it would make the wake path table-free; but that
"win" removes exactly one iteration over a one-rule table in every
shipped workload. The witness path does walk the table (no due
commitment), so there the validation is one extra pass beside the
selector's one-or-two passes — again at one rule per real table.

## 3. The contract inventory — every caller and what it passes

Complete caller census (grep across `crates/` for
`evaluate_wake|evaluate_witness`, then read of each site):

| Caller | Site | Table passed | Built | Mutated after? |
|---|---|---|---|---|
| `CornerCrowdWorld::execute` (the demo's behavior-spine crowd, 24 NPCs) | `crates/engine-slice/src/corner_behavior.rs:542-553` passes `&self.rules` | `Vec<Rule>` field, **`pub`** (`corner_behavior.rs:284`) | once, at world construction (`corner_behavior.rs:360-366`) — a **single idle rule** (`need: None`) | never (no assignment/push/clear anywhere — see §4) |
| `BehaviorScaleWorld::execute` (the behavior bench's wake loop) | `crates/engine-slice/src/behavior_measurement.rs:243-254` passes `&self.rules` | `Vec<Rule>` field (`behavior_measurement.rs:180`) | once (`behavior_measurement.rs:209`) — `vec![IDLE_RULE]`, **one rule** | never |
| The bench's witness-event measurement | `crates/engine-slice/src/behavior_measurement.rs:566-575` passes `&rules` | local `let rules = vec![IDLE_RULE];` (`behavior_measurement.rs:517`) — **one rule**, constructed once per measurement block | per block | n/a (local) |
| `synthesis_once` (the crossover-synthesis measurement) | `crates/engine-slice/src/behavior_measurement.rs:881-892` passes `&rules` | `let rules = [IDLE_RULE];` (`behavior_measurement.rs:866`) — **one rule**, a stack array constructed per call | **per call** | n/a (local) |
| `LocoWorld::execute` | `crates/engine-core/src/npc_locomotion.rs:2169-2180` passes `&self.rules` | `Vec<Rule>` field (`npc_locomotion.rs:2098`) | once (`npc_locomotion.rs:2129-2135`) — **one rule** | never |
| Crowd world in the render gate | `crates/engine-slice/tests/render_instanced.rs:3345-3354` passes `&self.rules` | `Vec<Rule>` field (`render_instanced.rs:3318`), cloned from the reference crowd at :3485 — **one rule** | once per control-world construction (a clone) | never |
| `evaluate_wake`/`evaluate_witness` unit tests | `npc_execution.rs:591, 632, 674, 696, 741, 764, 784, 829, 901, 935, 968, 982, 1227, 1277` — all inside `#[cfg(test)] mod tests` (module opens at `npc_execution.rs:458-459`) | small inline literals — including the **empty table** `let rules = [];` (:628) and single-rule tables | per test | n/a |

Notes drawn from the inventory:

1. **`npc_locomotion.rs:2169` is test-only.** The module's test module
   opens at `npc_locomotion.rs:900-901`; the `LocoWorld` caller and its
   `rules` field live inside it. It is not a production caller.
2. **Every authored table in the repo is one rule** (or zero, in one
   unit test). No caller constructs a table per *wake*; the only
   per-call construction is the bench's `synthesis_once` stack array,
   which is one `Rule` and free.
3. `validate_rule_table` has no other callers: the only non-test
   references are the two evaluation sites themselves
   (`npc_execution.rs:41` import, :164, :246); the remaining matches are
   `npc_behavior.rs`'s own unit tests (:637, :645, :668, :672) and doc
   links.
4. The empty table is valid (`Ok(())` — the loop body never runs); the
   selector's no-applicable-rule floor returns the idle baseline
   (`npc_behavior.rs:412-419`). So "zero rules" is a legal policy.

## 4. Can a caller mutate the table between calls?

**In the current tree: no caller ever does.** A grep for
`.rules =` / `.rules.push` / `.rules.clear` / `.rules.insert` /
`.rules.remove` across `crates/` returns **zero matches**. Every table
is written once at construction and only borrowed (`&self.rules`,
`&rules`) thereafter.

**But the type system does not enforce it, in two independent ways:**

- `CornerCrowdWorld.rules` is a **`pub` field**
  (`corner_behavior.rs:284`). Any code holding `&mut CornerCrowdWorld`
  can overwrite or mutate the `Vec` between evaluations.
- `evaluate_wake`/`evaluate_witness` take `rules: &[Rule]` — a public
  API accepting any borrowed slice. A caller may construct a fresh
  table per call, mutate a shared one between calls, or alias the same
  memory with different contents across calls (a dropped-and-recreated
  `Vec` can even land at the same address).

**Consequence for design (b) (caching):** a cache keyed on pointer
identity or a version counter is **not sound** against the API as it
stands — content mutation at a stable address, or fresh tables at
recycled addresses, defeats both. Making it sound requires *changing
the API first* (freeze the table behind an immutable handle), at which
point the cache is superseded by option (a)'s construction-time
validation anyway.

## 5. The safety boundary that already exists (defense in depth)

The charter requires retaining "a safe boundary for externally supplied
raw policy." Two layers exist today:

1. **Admission refusal (typed, fail-closed):** the per-call
   `validate_rule_table` in both evaluators — an out-of-range need
   index refuses the evaluation (`None`), pinned by
   `both_evaluators_fail_closed_on_an_out_of_range_need_index`
   (`npc_execution.rs:958-994`).
2. **Per-rule fail-closed inside the selector (defense in depth):**
   `Rule::need_level` (`npc_behavior.rs:210-213`) uses
   `routine.needs.get(need as usize)` — an out-of-range index yields
   `None`, and `Rule::applicable` then treats the rule as inapplicable
   (`npc_behavior.rs:227-230`: "Out-of-range need index: fail closed,
   never a panic"). The module docs state the layering explicitly
   (`npc_behavior.rs:270-277`): the selector "additionally fails closed
   per-rule so an unvalidated table can never crash, only mis-serve."

So the per-call validation is not a memory-safety boundary — it is an
**admission-quality boundary** that upgrades would-be mis-service into
a typed, fail-visible refusal. Any design that removes it from the
`&[Rule]` entry points must preserve an equivalent refusal somewhere,
or accept a documented semantic change (mis-serve instead of refuse)
for raw tables — which would move the pinned None-contract tests.

## 6. Design options

### Option (a) — `ValidatedRuleTable` newtype; evaluate_* takes it

Shape: `pub struct ValidatedRuleTable { rules: Vec<Rule> }` constructed
via `ValidatedRuleTable::new(rules) -> Result<Self, &'static str>`
(runs `validate_rule_table` once), `Deref<Target = [Rule]>` for reads,
no `&mut` access. `evaluate_wake`/`evaluate_witness` grow (or switch
to) `rules: &ValidatedRuleTable` forms.

- **API surface:** one new public type plus either (i) **duplicate**
  entry points (validated + raw) — two names, two doc contracts, two
  test matrices forever; or (ii) a **breaking switch** — every caller
  in §3 changes: the demo world's `pub rules: Vec<Rule>` field becomes
  the newtype (an engine-slice public-API change), the bench's
  per-call stack array becomes a per-call construction (fine — one
  rule), the test worlds all change. The raw form must be RETAINED
  regardless (the charter's safe boundary for externally supplied raw
  policy), so the dual surface is unavoidable in practice.
- **Allocation behavior:** if the newtype owns a `Vec<Rule>`,
  construction allocates once at admission — outside every hot window,
  harmless (and the bench's `synthesis_once` per-call array becomes a
  per-call heap allocation where today it is a stack array — a
  measurable regression in that measurement path, visible to any
  allocation census around it). A borrowing newtype
  (`struct Validated<'a> { rules: &'a [Rule] }`) avoids the allocation
  but pushes a lifetime parameter through `CornerCrowdWorld`,
  `BehaviorScaleWorld`, and both evaluators — signature churn in three
  crates to delete one loop iteration.
- **Safety boundary:** preserved — raw forms still validate per call;
  validated forms validate at construction. Sound under mutation
  (the newtype freezes its contents).
- **What it buys:** the wake path's only table walk
  (§2's short-circuit observation) disappears for validated callers.
  At one rule per real table: single-digit nanoseconds per wake.

**Verdict:** real, permanent API cost (dual entry surface or breaking
change + lifetime/ownership churn across engine-core, engine-slice,
and gates) to remove a one-iteration, allocation-free loop. Fails the
charter bar unless it also clarifies contracts — and the contract it
would "clarify" (admission-time refusal instead of per-call refusal)
is already documented, tested, and deliberately shaped
(`npc_behavior.rs:270-277`, `npc_execution.rs:136-140, 230-233`).

### Option (b) — cached validation with identity/version check

Shape: memoize "this `&[Rule]` validated" keyed on (pointer, length)
or a policy version counter; evaluate_* consults the cache.

- **API surface:** no signature change (the appeal), but a new piece of
  hidden global or per-world state, a cache-invalidation protocol, and
  a new class of bug (stale-cache acceptance of a mutated table —
  precisely the corruption the validation exists to refuse).
- **Allocation behavior:** the cache itself allocates (a map or the
  counter storage); for one-rule tables a hash of the contents costs
  more than the scan it replaces.
- **Safety boundary:** **broken** as analyzed in §4 — `pub` field
  mutation and `&[Rule]` aliasing make identity/version unsound
  without a prior API freeze; with the freeze, option (a) dominates.

**Verdict: unsound or slower. Reject.**

### Option (c) — no-change, with the measured argument

- **API surface:** unchanged (zero churn across three crates and two
  gate files).
- **Allocation behavior:** already zero (§2).
- **Safety boundary:** unchanged and already two-layer (§5).
- **Mutation handling:** irrelevant — no state introduced that
  mutation could invalidate.
- **The measured argument:** the loop is O(n_rules) with n ∈ {0, 1} in
  every workload in the repository (§3); its per-rule work is a subset
  of one selector pass's per-rule work (§2); it allocates nothing
  (§2); it is not on any allocation-pinned window's hot list
  (spine_allocations' `ScenarioWorld` at
  `spine_allocations.rs:208-235` does not even call the evaluators —
  the crowd-path allocation pins in `render_instanced.rs` are the
  relevant gates and are untouched). The cost is bounded by table
  length, which is policy data; if a future policy ships a
  pathologically large table, the selector's own passes scale with the
  same n and dominate the validation long before the validation
  matters — the right fix then is at the policy-authoring boundary,
  not in the evaluator.

**Verdict: the honest one.** The charter's own bar (§8 below) is
satisfied by leaving the loop in place with this evidence.

## 7. Recommendation

**NO-CHANGE**, on the evidence in §2-§6:

1. The measured cost is negligible in every shipped workload (zero or
   one rule per table; no allocation; nanoseconds against a path that
   immediately performs decay arithmetic, view construction, and spine
   bookkeeping).
2. The per-call validation **is** the charter's required safe boundary
   for externally supplied raw policy, and it composes with the
   selector's per-rule fail-closed as deliberate defense in depth
   (documented at `npc_behavior.rs:270-277`).
3. The only sound way to remove it (option a) costs a dual public API
   surface or a breaking change across three crates, plus lifetime or
   allocation churn — to delete one loop iteration. Option (b) is
   unsound against today's mutation possibilities (§4).
4. Revisit triggers, recorded for the future: (i) a policy table
   large enough that the validation scan shows in a profile (then
   reconsider option (a) as an *admission-time* API for that policy's
   own crate, not a core API change); (ii) a measured wake-rate
   workload (e.g. thousands of wakes per tick sustained) where the
   cumulative validation cost is demonstrated by profile, not
   inspection.

One **documentation-only** improvement is recommended inside no-change:
the `evaluate_wake`/`evaluate_witness` doc comments already state the
refusal contract; add one sentence each noting that the validation is
O(table length), allocation-free, and the wake path's only table walk
(because the due-commitment short-circuit precedes the selector's
scan) — so a future reader does not re-derive this whole analysis to
ask the same question. That is a comment edit, not a behavior edit,
and belongs to a doc-hygiene package (A02's lane), not a behavior
change.

## 8. The charter bar, answered directly

> "Do not introduce a complicated type-state architecture solely to
> remove one tiny loop unless it also clarifies contracts."

The loop IS tiny: one `Option<u8>` check per rule, zero or one rules
per real table, no allocation. The contracts are already clear and
pinned: admission refusal (typed `None`) plus per-rule fail-closed
inside the selector. A `ValidatedRuleTable` would not clarify the
contract — it would *relocate* it (from per-call to construction-time)
at the price of a dual API or a breaking change, and the raw form
would still carry the per-call check for untrusted policy, so the
"hot" cost the change targets survives on exactly the boundary the
charter says must stay safe. **Recommend no-change with evidence.**

## 9. Test / evidence plan (for the no-change disposition)

Pins that must hold (they will — nothing changes):

- `both_evaluators_fail_closed_on_an_out_of_range_need_index`
  (`npc_execution.rs:958-994`) — the None-contract for both evaluators.
- The `validate_rule_table` unit tests (`npc_behavior.rs:637-672`).
- The selector test battery (`npc_behavior.rs:578-954`) — untouched.
- The behavior digests/census pins that transit through
  `evaluate_wake`: `render_instanced.rs`'s crowd gates (the
  composition-census and stream-digest pins around :3308-3600) and
  `behavior_measurement.rs`'s replay arms.
- The spine allocation pins (`spine_allocations.rs:178,186`) —
  unrelated windows, but the baseline that any future attempt in this
  area must not move.

Evidence a future change proposal would owe (recorded here as the
discriminating bar):

- A profile showing `validate_rule_table` as a measurable fraction of
  a real workload's wake path at a table size a real policy uses
  (not a synthetic 10⁶-rule table).
- An allocation census around the proposed construction path
  (admission-time and steady-state) demonstrating no new per-tick or
  per-wake allocation.
- A mutation-possibility audit of the new API (can any caller still
  hand the evaluator an unvalidated or mutated table?).

No new test is required for no-change; this document is the evidence.

## 10. Coverage limits (honest)

- No benchmark was run (design-only task): the cost claims rest on
  operation counting by direct read (§2), which the charter permits
  when the loop is this small, and on the caller census (§3) — not on
  hardware measurement. If the orchestrator wants a number, a
  one-off `#[bench]`-style harness around `evaluate_wake` with a
  one-rule table vs. a validation-stubbed variant would quantify it;
  the prediction is single-digit nanoseconds.
- The caller census is complete for the current tree (grep + read of
  every match), but a future crate could add callers; the design's
  revisit triggers (§7.4) are the guard.
- `enumerate_witnesses` and the perception census (A07) interact with
  `evaluate_witness`'s callers but are out of scope here.
