# T07 — The Retention Safe Moving Cut: Design

**Author:** T07 design lane (glm-5.3-w4a8 subagent, subordinate to the ODU
orchestrator session)
**Status:** COMPLETE — source-cited design for orchestrator review; no source
edits made (read-only lane).
**Read base:** worktree `/home/coder/PostRust-worktrees/t07`, branch
`ee/overnight-t07-retention-design`, base `23c5249`. **All spine.rs line
numbers below cite this tree** (the audit's numbers were taken at `096f8d9`;
at `23c5249` the regions sit within a few lines of the audit's citations and
were re-resolved individually for every claim).
**Scope:** strict-audit-IV finding T07 (S0 for consequence-bearing
composition; Q05/CA07/A22) — the retention acknowledgment is a permanent
assertion set once at configuration time, not a safe moving cut.
**Constraints honored:** DESIGN ONLY — no file under `crates/` was touched;
no cargo commands run (read-only analysis; costs reasoned from the data
structures and the CA-07 measured operation counts, labeled as estimates
where they are estimates); no new dependencies.

---

## 0. Thesis

`ConsequenceEligibility::acknowledged()` (spine.rs:407-422) is a zero-sized
assertion that stands permanently once `set_retention_policy`
(spine.rs:2808-2815) fires it; for consequence-bearing worlds this lets the
cadence cut records whose application consumers never vouched for them
release. The fix is a **two-class eligibility model**: (1) worlds that are
*permanently consequence-free* — no application lane ever reads execution
history — keep a static, type-shaped capability (today's token, re-documented
as exactly that class and nothing more); (2) *consequence-bearing* worlds arm
the cadence through an **in-memory, per-consumer, monotone
eligible-prefix frontier** — named consumers acknowledge the prefix they can
relinquish, the spine cuts only to `min(consumer acknowledgments)` clamped by
the existing threshold/chunk math, the frontier never rides the canonical
wire (so byte-identity for identical cut sequences is preserved by
construction and the restored-process hazard stays closed), refusals and lag
are observable per consumer and never resolved by discarding data, and the
underlying storage moves from the tail-shifting `Vec::drain`
(spine.rs:2781) to a **logical head + amortized compact** — the same `Vec`,
a `head: usize` marking the already-summarized physical prefix, zero bytes
moved per firing, amortized ≤ one record-move per discarded record, zero
allocations, and the live tail stays a contiguous `&[ExecutionRecord]` so
every public slice contract, the save encoding, and the allocation pins are
preserved exactly.

---

## 1. The defect, precisely (evidence at 23c5249)

- **The token:** `ConsequenceEligibility` (spine.rs:383-422) is an opaque
  `Copy` ZST (`_asserted: ()`, :407-411); `acknowledged()` (:413-421) is its
  only constructor. It is stored once at configuration
  (`retention_eligibility: Option<ConsequenceEligibility>`, :802-808, set at
  :2814) and cleared only by `set_retention_bound` (:2827-2833, which
  re-opens the question on bound change), `clear_retention_policy`
  (:2840-2843), or restore (:4161 — configuration never rides the wire).
- **The firing:** `apply_retention_policy` (spine.rs:2915-2962), the single
  checkpoint at the end of `run_tick` (:2669-2678), checks exactly two
  things: the length threshold (`ledger_len > retain + chunk`, :2920-2923)
  and the token's presence (:2924-2939 — the CA-07 refusal arm). If the
  token is present it cuts `min(excess, chunk)` records (:2940-2947) with no
  further evidence. **Nothing between configuration and the cut consults any
  consumer, any acknowledgment state, or any consequence-resolution fact.**
- **Why the engine cannot know:** the authority (executed-membership,
  exactly-once, conservation, watermarks) hangs off `next_seq`, the
  `unfinished` complement (:752), `waiting_on`, and `max_executed_index`
  (:768) — never the ledger (the evidence/history split, spine.rs:243-252).
  A prefix cut is structurally valid for every engine invariant; the only
  eligibility that can be violated is the caller's. That is why the proof
  must come from the consumer side — and why a *standing* token is a proof
  about the past standing in for a proof about the future: the token says
  "my consumers could relinquish this prefix at configuration time," and the
  cadence then treats every FUTURE prefix as releasable until the config is
  touched again.
- **The tail-shift:** `compact` (spine.rs:2758-2791) hashes the prefix
  read-only (:2771-2776) then removes it in place with
  `self.ledger.drain(..cut)` (:2781) — no allocation, capacity retained
  (the CA-07 landing replaced a per-firing `split_off` reallocation), but
  the retained tail memmoves down: `retained × size_of::<ExecutionRecord>()`
  bytes per invocation (the ~40 B in-memory stride vs the 31 B wire record,
  spine.rs:280-282). The CA-07 cost witness (spine.rs:6905-6987) measured,
  at constant chunk 300: **200 record-moves per firing at retain 128, 400 at
  retain 300** (:6977-6984) — the moves grow with the retained window, not
  with the work performed.
- **The explicit path:** `Spine::compact` (:2758) itself delegates
  eligibility to the driver by doc (:2735-2745). One production composition
  already implements a real moving cut on top of it —
  `engine-slice/src/crossing.rs` `compact_session` (:1967-2066): the
  per-episode lane-eligibility walk (:1999-2028, an episode is summarizable
  iff none of its identities is pending and every applied identity is
  canonically confirmed) precedes the spine cut, monotone per region
  (:1943-1952). The engine-side cadence is the only path with no such
  boundary.

**What is already right and stays:** the opt-in no-default rule, the
tick-boundary firing site, the hysteresis band, the chunk-bounded SHA-256
work, the authenticated prefix-summary chain, the refusal arm with
`refused_ticks`/`retention_lag_records`, the configuration-never-rides-the-
wire rule, and the restore→reconfigure→next-compact equivalence
(:6521-6545, :6824-6903). The design changes WHO may arm the cadence and
WHAT the cut consults — not the cadence, the cut mechanics' wire form, or
the authority.

---

## 2. The eligibility model

### 2.1 Class 1 — permanently consequence-free worlds

A world is *permanently consequence-free* when no application lane ever
reads the execution ledger's per-record content: no late accounting over the
verbatim tail, no audit consumer, no history API use — the world's handlers
execute, the spine's own authority structures carry everything the engine
needs, and history is diagnostic only. For such worlds the eligible prefix
is **everything executed** — the frontier is the whole ledger, and `retain`
is a diagnostic-preference window, not a safety window.

The design keeps a static capability for this class, tightening today's
token into an honestly-named one:

```rust
/// Constructing this value asserts the WORLD CLASS: no application
/// consumer of execution-history records exists or will ever exist in
/// this composition — the eligible prefix is unbounded by construction
/// of the application, not by evidence gathered at runtime.
pub struct ConsequenceFreedom { /* opaque */ }
```

with `set_retention_policy(policy, ConsequenceFreedom::permanently_free())`
as the arming call for this class. `ConsequenceEligibility` remains as a
deprecated alias for one release (§6) so the existing test call sites
and any external caller compile unchanged while the docs stop calling the
token an "acknowledgment."

**The honest limit, stated in the type's doc:** Rust cannot prove a negative
about future callers; the capability is still an assertion. What the design
changes is that the assertion is now *scoped to a world class with an
unbounded frontier* — it no longer impersonates evidence about consumer
state. The register of evidence (which compositions asserted it) is the
commit history and the audit trail, not the type system. Where the driver
can discharge the proof structurally, it should prefer the frontier class
even for consequence-free worlds; Class 1 exists for the genuinely static
case (deterministic simulations, benchmarks like `sustained_load.rs`, the
spine's own test fixtures).

### 2.2 Class 2 — consequence-bearing worlds: the eligible-prefix frontier

For worlds whose application lanes read history (late accounting, audit,
UI trails, the crossing session's ledger-adjacent consumers), the cadence is
armed through a **consumer-acknowledgment frontier**:

```rust
pub struct RetentionConsumerId(/* small stable identity: &'static str */);

/// Arming consequence-bearing retention: the cadence fires only up to
/// the minimum consumer acknowledgment, never on standing configuration.
pub fn arm_retention_frontier(&mut self, policy: RetentionPolicy);

/// Registers a consumer. The initial acknowledgment is the CURRENT
/// summarized prefix — registering onto a compacted spine vouches for
/// the standing cut (the already-summarized prefix is unrecoverable;
/// a consumer that cannot vouch for it must not register).
pub fn register_retention_consumer(&mut self, id: RetentionConsumerId);

/// Advances a consumer's acknowledgment through global executed ordinal
/// `through` (the first `through` execution records of the lifetime are
/// releasable by this consumer's vouch). Monotone; fail-closed.
pub fn acknowledge_retention_through(
    &mut self, id: RetentionConsumerId, through: u64,
) -> Result<(), RetentionAckError>; // UnknownConsumer | Regression { .. }
```

**The frontier value:** `eligible = min over registered consumers
(ack_i)`, with the register rule `ack_i ≥ summarized_records` at all times.
The checkpoint's cut becomes:

```
cut = min(excess, chunk, eligible − summarized_records)
```

and a firing requires `cut ≥ 1` (a fully-held frontier refuses exactly like
an unarmed policy — counted, observable, never a silent skip). The
`expect` at spine.rs:2943-2947 keeps its invariant by the same
construction argument extended with the clamp: the gate fires only when the
clamped cut is ≥ 1.

**The ordinal space:** global executed ordinal — record *n* is the *n*-th
execution of the spine's lifetime. This space is already on the wire
implicitly: `summarized_records` is exactly the count of cut records in it
(the count identity `summarized + retained == executed`, gated at restore
spine.rs:4055-4070), and it survives restore unchanged, so consumer
acknowledgments remain meaningful across process boundaries. Because
execution order is deterministic (the E-8 double-run discipline), ordinal ↔
record identity is stable across identical runs. Consumers that think in
identities read `ledger_retained()` (the verbatim tail) and count what they
have absorbed; a convenience `Spine::retention_ordinal_of(seq)` (live-tail
scan, O(live), driver-cadence cost, never in the tick loop) converts if
needed.

**Model coverage (the audit's enumerated cases):**

- **Multiple consumers:** the min rule. Each consumer's acknowledgment is
  independent and monotone; the lagging consumer gates the cut. Observable
  per consumer (§4).
- **Unknown/indeterminate commit:** a consumer acknowledges only what it
  has *durably* processed. Unknown = unacknowledged = holds the frontier —
  fail-closed, the same rule as the persist side's settled-transaction
  watermark (engine-persist/src/pipeline.rs:67-132, P3-L6: a transaction is
  settled only when durably canceled or fully journaled; everything else
  keeps paying). The engine never resolves an unknown into an ack.
- **Late readers:** a late reader is a consumer that stops acknowledging at
  its read point; the frontier waits, the lag grows (§4). A consumer
  joining mid-session registers at the standing cut (the register rule) —
  it vouches for what is already unrecoverable or it does not join. A
  consumer that needs records *already summarized* was impossible to serve
  before this design and remains impossible (the summary is a derivation
  witness, never a reconstruction — spine.rs:180-189, 2726-2733); the
  register rule forces that impossibility to be confronted at join time,
  not discovered at read time.
- **Blocked/held obligations:** the engine's authority never reads the
  ledger, so a blocked waiter on a held prerequisite (the CA-07 unresolved-
  consequence fixture, spine.rs:6824-6903) does not itself hold the
  frontier — the consumer that will late-account its *future* execution
  record holds its *future* acknowledgments, which is exactly the moving
  cut: the past prefix can be released while a future consequence is
  pending. The CA-07 control test carries over with its semantics
  strengthened: the unresolved consequence now also holds a *named*
  consumer's ack below the threshold and the refusal is per-consumer
  observable.
- **Host change / checkpoint / recovery:** the frontier is in-memory and
  derived; on restore every consumer re-registers and re-establishes its
  acknowledgment **from its own durable state** (the settled-watermark
  re-derivation precedent — a fresh persist open re-derives the watermark
  lazily, pipeline.rs:86-91). No acknowledgment rides the wire, so a
  restored process can never resume cutting on a vouch a prior process
  made and this one did not re-make — the CA-07 hazard stays closed
  (spine.rs:296-314 documents exactly this why-not; the design extends it
  from the boolean token to every consumer ack).
- **Restore-config change:** a restored spine may be armed with a
  different policy and a different consumer set. The register rule makes
  the new set's standing vouch explicit (initial ack = restored
  `summarized_records`); arming with *zero* consumers never cuts (the
  empty-frontier rule: `min(∅)` is undefined, and the design defines it as
  refusal — never as permission). A driver that wants continuity of the
  cut math re-applies its policy and re-registers its consumers at its own
  configuration boundary after restore, exactly the pinned
  restore→reconfigure→next-compact equivalence shape (:6521-6545).
- **Pending records:** records appended but not yet absorbed by a
  consumer simply sit above that consumer's ack; the frontier holds. No
  special case.
- **Snapshot identity:** an acknowledgment vouches for *content the
  consumer read*, on *this* history. The spine side authenticates what it
  does with the vouch: the cut hashes exactly the records released
  (the chain over `record_wire_bytes`, :2767-2776, pinned to the save
  encoding by test :4653-4666), and the firing's evidence (consumer set,
  per-consumer acks at the cut, ordinals, tick) is recorded in the
  observational stats (§4) — an audit trail of the proof's exercise. The
  spine cannot verify the consumer *read* what it acks (honest limit,
  §8); it fabricates nothing and it never coerces a vouch.

### 2.3 What happens to the existing API surface

- `set_retention_bound` (:2827-2833) — the refusal arm — is unchanged: it
  declares the bound without any eligibility and refuses observably. It
  composes with the frontier: arming afterwards is a fresh
  `arm_retention_frontier`.
- `retention_armed` (:2856-2858) becomes "armed with *some* eligibility":
  the Class-1 capability OR a frontier with ≥ 1 registered consumer.
- `Spine::compact` (explicit, :2758) keeps its standing: the driver's
  between-ticks tool with its own eligibility discipline delegated by doc
  (:2735-2745). **Manual operator authority is retained** (the audit's
  explicit bar): the frontier polices the *automatic* path; an operator who
  explicitly cuts takes the responsibility explicitly, exactly as today.
  Where composition-level eligibility exists around the explicit path —
  `crossing.rs::compact_session` — it already precedes the cut
  (:1999-2028) and is untouched.

---

## 3. The frontier's data shape (wire analysis, both options, one recommendation)

**Where it lives:** in the `Spine` struct, beside `retention`
(:800-812): `retention_consumers: BTreeMap<RetentionConsumerId, u64>` (acks
in global-ordinal space; `BTreeMap` for deterministic iteration in the
observability report). Configuration-class state — like `retention`,
`retention_eligibility`, and `RetentionStats` (:3777-3784) — never
fingerprinted, never serialized, restored at defaults.

### Option A — in-memory derived frontier (RECOMMENDED)

The frontier never touches the encoder. Save bytes are a function of the
cut *sequence* only, and that is already the ratified identity discipline:
`policy_cadence_equals_the_equivalent_explicit_compaction`
(spine.rs:6487-6545) pins that 3×300 policy firings and one explicit
900-record cut of the same history produce **byte-identical** saves — the
prefix chain is invocation-boundary-independent, and the wire carries only
`summarized_records`, the summary, and the verbatim tail
(:4275-4316).

The frontier changes *which* cuts fire (it can only veto or clamp a
firing), never *how* a fired cut encodes. Therefore:

- **Identical cut sequences → identical wire bytes: preserved by
  construction.** The gate is upstream of the encoder; a clamped cut
  (`min(excess, chunk, eligible − summarized)`) is a different cut
  sequence, and different cut sequences were never promised identical
  bytes — the same logical state encodes differently after different
  compaction histories today (that is the `spine-history/2` form's
  existence).
- `sektor7/save/1`, `sektor7/save/2`, `engine-core/spine-history/2`
  (wire.rs:34-54), `SAVE_FORMAT_VERSION(_V2)`, every gate at
  spine.rs:3844-3851, and every digest pin: **byte-exact, zero format
  change, no migration needed.** The proof is the unchanged encoder plus
  the new tests in §5 pinning frontier-armed cadences against the
  equivalent explicit-compaction sequences byte for byte.
- The three full-logical fingerprint domains — `engine-core/spine/2`
  (:2977), `engine-core/state/2` (:3039), `engine-core/spine-lanes/1`
  (:3067) — and the compacted pair `engine-core/spine-compacted/1`
  (:3242), `engine-core/state-compacted/1` (:3320) hash logical content
  only; the frontier and (§7's) head are physical/configuration state and
  never reach any `Fingerprinter`.

### Option B — versioned durable format (`spine-history/3` carrying the frontier) — analyzed and REJECTED

A v3 would add a frontier section (consumer ids + acks + an epoch) to the
save, requiring: a new `SAVE_DOMAIN_V3` and `SPINE_HISTORY_DOMAIN_V3`
(wire.rs pattern :34-54), mutual rejection with both existing forms
(the :3862-3891 dispatch extended), restore-side gates (acks ≥
`summarized_records`, register-rule re-derivation), and a migration proof
that every existing `spine-history/2` stream loads under the new reader
with an empty frontier (refusing to cut until consumers re-register). The
analysis rejects it on three grounds, all already ratified in this
codebase:

1. **The CA-07 why-not (spine.rs:296-314):** riding the canonical save
   makes save bytes depend on non-logical consumer state — the same
   logical state would encode differently with different consumer sets,
   breaking the byte-identity discipline every digest pin enforces.
2. **The hazard itself:** persisting acknowledgments lets a restored
   process resume cutting history its consequence consumers never
   re-vouched for — the precise defect class T07 exists to close. A
   persisted ack is a standing assertion that survives the process that
   made it.
3. **The sidecar alternative** cannot authenticate into the rotation
   anchor's evidence without a new trust surface (the on-disk-index
   rejection's reasoning, cited at :303-305).

If a future owner decision ever forces Option B (e.g., an orchestrating
process that must survive restarts without consumer re-registration), the
migration obligations above are the complete list; this design's frontier
semantics transfer unchanged. **Recommendation: Option A.**

---

## 4. The lag / backpressure policy

**The rule, explicit:** the checkpoint never discards data to keep RAM
under a target. A held frontier means the ledger grows; that growth is the
honest cost of unresolved consequences, and it is *observable*, never
silent, never amortized away by an unauthorized cut.

Observability surface (all in the `RetentionStats` class — observational,
never fingerprinted, never serialized, :355-381):

- `refused_ticks` (existing, :380) — extended semantics: threshold
  exceeded with no firing eligibility *or* with a fully-held frontier.
- `frontier_hold_ticks` (new) — ticks the threshold was exceeded and the
  frontier (not the arming) was the limiter.
- `retention_lag_records` (existing, :2868-2875) — unchanged: distance
  above the resident bound.
- `retention_frontier_lag_records` (new) — `min(threshold-cut,
  eligible − summarized)` shortfall: how many records the policy could
  have cut but the consumers have not released.
- `retention_consumer_lag(id)` (new accessor) — per-consumer distance
  behind the leading acknowledgment: *which* consumer is the gate.
- The firing log line (:2952-2960) gains the frontier values when the
  frontier mode is armed.

**Backpressure:** the lag metrics are the driver's admission signal. The
engine's own levers stay what they are: the execution budget bounds
per-tick arrivals (each tick adds at most `budget` ledger records — the
existing argument at :271-282), the chunk bounds per-firing work, and the
driver may slow scheduling, drain its consumers, or shed load *at its own
authority*. The engine neither sheds history nor blocks the tick: a held
frontier costs one threshold comparison per tick. The existing
misconfiguration rule (chunk below sustained arrival rate → the ledger
nets upward, observable in stats — :276-282) extends verbatim to the
frontier: a consumer acknowledging slower than the arrival rate nets the
ledger upward, and that is the consumer's owner's decision to make
consciously, visible in `retention_consumer_lag`.

---

## 5. The test plan

Every test names its oracle. The existing matrix that must stay green
UNCHANGED (the compat bar): the nine A22/CA-07 retention tests
(spine.rs:6171, 6267, 6395, 6487, 6554, 6645, 6763, 6824, 6920) and the `spine_history.rs` PP-B2 matrix (10 tests, corruption
gates included). New tests (spine.rs inline module, the CA-07 precedent):

1. **Unresolved consequence across threshold (the audit's first case):**
   threshold exceeded, a registered consumer's ack below the cut —
   refusal; `frontier_hold_ticks` and both lag metrics advance; no cut;
   `history_kind()` stays `Full`; the ledger keeps growing past the bound
   with conservation exact at every boundary (the no-discard rule made
   mechanical).
2. **Advancing safe frontier:** the consumer acks past the threshold in
   steps — each firing cuts exactly `min(excess, chunk, eligible −
   summarized)`; the resulting save bytes are byte-identical to the
   equivalent *explicit* compaction sequence of the same cuts (the
   :6487-6545 discipline extended to the frontier mode); the count
   identity holds at every boundary.
3. **Refusal:** zero consumers (armed, threshold exceeded → refuse);
   `acknowledge_retention_through` on an unknown consumer → typed
   `UnknownConsumer`; a regressing ack → typed `Regression` (monotonicity
   is enforced, not assumed); register onto a compacted spine with an ack
   below the standing cut → typed refusal (the register rule).
4. **Rollback/restore:** the :6824-6903 control shape extended —
   mid-cadence save → restore (frontier empty, policy cleared) →
   re-register consumers with durably-derived acks → continue →
   byte-identical to the uninterrupted control *with the same
   acknowledgment schedule*; the restored process that fails to
   re-register refuses instead of cutting.
5. **Repeated compaction with identical logical state and authoritative
   history:** the twin discipline (:6267-6383) — a no-policy twin's full
   ledger is the verbatim-suffix oracle at every cut; the frontier-armed
   spine's stats/watermarks/report identical to the twin at every tick.
6. **Sustained arrivals vs chunk rate:** the `SustainedWorld` shape
   (:6603-6626) with a consumer acknowledging at rate r against chunk c:
   r > c nets the ledger upward with `retention_consumer_lag` growing
   linearly (observable, never discarded); r = c holds the bound; r < c
   converges to the band. Also the multi-consumer min: two consumers,
   the lagging one gates, `retention_consumer_lag` names it.
7. **Late join:** a consumer registering after a firing vouches the
   standing cut (register rule) and holds the next one at its own pace.
8. **Class-1 compat:** every existing `ConsequenceEligibility` test call
   site compiles and passes against the alias (§6), and a
   permanently-free-armed cadence reproduces :6267's exact sawtooth.
9. **Storage mechanics (§7):** per-firing move counts at O(1) (zero tail
   moves per firing — the :6920 witness inverted), the amortized-move
   bound (total moves ≤ discarded records over a sustained run), the
   capacity discriminator preserved (:6974), save round-trip
   byte-identity with a dead prefix present, restore tightness (head 0),
   and the sustained loop still zero-allocation.
10. **Explicit-path independence:** an explicit `compact` between
    frontier-held firings succeeds (manual authority) and the frontier
    accounting stays coherent (ack clamp measured from the new
    `summarized_records`).

---

## 6. Consumer inventory and migration (grep-verified at 23c5249)

**`ConsequenceEligibility`** — every site in the workspace:

| Site | Role |
|---|---|
| spine.rs:383-411 | definition (doc :383-405, struct :406-411) |
| spine.rs:413-421 | `acknowledged()` constructor |
| spine.rs:802-808 | field `retention_eligibility` |
| spine.rs:2811-2814 | consumed by `set_retention_policy` |
| spine.rs:2856-2858 | read by `retention_armed` |
| spine.rs:2924-2939 | read by the checkpoint's eligibility gate |
| spine.rs:6158-6160 | test shorthand `eligibility()` |
| spine.rs:6195, 6213, 6280, 6408, 6502, 6522, 6537, 6565, 6658, 6787, 6850, 6862, 6891, 6931 | the 14 test call sites |

**No production consumer outside spine.rs** — the CA-07 landing
grep-verified "zero external consumers" (commit `9dce696`, integrated
`b9709c6`), re-verified at this base: the only non-engine-core matches for
retention symbols are `engine-slice/src/worker.rs:1244` and
`engine-persist/src/residency.rs:96-150` referencing the **other**
`RetentionStats` (`CommitPipeline`'s, P3-L6 — a different type in a
different crate).

**`RetentionPolicy`** — spine.rs:223-353 (type), :800 (field), :1188
(construction default), :4161 (restore default), :2808-2849 (set/bound/
clear/get), :2915-2962 (checkpoint), :6147-6152 (test shorthand), the same
test sites above. **No external constructor or reader** outside the
engine-core tests.

**`Spine::compact` (explicit)** — production consumers:
`engine-slice/src/crossing.rs:1967-2066` (`compact_session`, with its own
composition-level eligibility :1999-2028 — the compliant precedent),
`engine-slice/src/sustained_load.rs:1215-1217` (measurement only);
`engine-core/tests/spine_history.rs` (8 sites, the PP-B2 matrix).

**What breaks:** nothing at the type level if `ConsequenceEligibility`
stays as a deprecated alias of `ConsequenceFreedom` for one release. The
14 spine-internal test call sites adapt mechanically (they assert the
consequence-free class — which is what their fixtures are: `NoopWorld`,
`SustainedWorld` never read history). The doc contract changes
(set_retention_policy's token parameter meaning narrows to Class 1);
`engine-slice` and `engine-persist` need **zero** changes (grep-verified
above; the crossing session's eligibility is internal to its own cut).

**Staged landing (three commits):**

1. **The frontier** (eligibility model): `ConsequenceFreedom` + alias,
   `arm_retention_frontier` / consumer registry / ack API / clamped cut /
   new stats + accessors / the §5 tests 1-8, 10. No storage change. Every
   existing test green unchanged (the compat proof). Count-gate pins
   updated from the measured run.
2. **The storage mechanics** (§7): logical head + amortized compact, the
   must-switch site list, tests §5.9, allocation pins re-measured (the
   sustained loop must stay zero-allocation; the main-path pins
   (18, 6880)/(9, 3024) must not move — no-policy windows are untouched).
3. **The reconciliation**: register/readme rows (T07, Q05, CA07, A22),
   the audit's §6 row updates, this design's status flip — the house rule
   that a landing updates the register AND the readme together.

---

## 7. Storage mechanics (the drain replacement)

The audit requires evaluating ring/deque/chunked segments or logical head
+ amortized compact against: bytes moved per cut, allocation behavior vs
the spine allocation pins, save/restore byte-identity, determinism.

**Ground truth (CA-07's measured table, spine.rs:6905-6987):** at constant
chunk 300, one firing = 300 SHA-256 record hashes + `retained` in-place
record moves: **200 moves at retain 128, 400 at retain 300** — moves scale
with the retained window (~40 B each — :280-282; the ~40 B is the
documented stride, an implementation-time `size_of` check belongs in the
landing). The drain is allocation-free and keeps the high-water capacity
(:6939-6944, :6974).

**Option 1 — status quo `Vec::drain` (spine.rs:2781):** moves
`retained × ~40 B` per firing — a record that stays retained across k
firings is moved k times. At the measured windows (retain ≤ 300: ≤ 16 KB
per firing) this is bounded and small; it grows linearly in `retain`, so a
large verbatim window (retain 100k → 4 MB per firing) pays memmove for
nothing. Allocation: zero; capacity retained. Wire: trivially identical.

**Option 2 — ring/deque (`VecDeque<ExecutionRecord>`):** O(1) prefix
removal, but **it breaks the public contiguous-slice contract**
`ledger_retained() -> &[ExecutionRecord]` (spine.rs:1282-1287): a wrapped
ring has no contiguous live view, so every consumer — `crossing.rs:1982`
(`spine.ledger_retained().len()`), the test oracles
(:6330-6334, :6470-6474), `try_ledger_full` (:1289-1293) — needs an API
change rippling into engine-slice. Capacity semantics differ from the
`Vec` capacity the :6974 discriminator witnesses. Growth still reallocates
(same as Vec). REJECTED: the slice contract is load-bearing and the gain
over Option 3 is nil.

**Option 3 — chunked segments (`Vec<Vec<ExecutionRecord>>`):** whole-
segment drops are O(segment), but segments allocate (violating the
zero-allocation sustained loop unless a segment pool is added — new
machinery, new pins), partial segments need per-chunk indexing, and the
same slice-contract break as Option 2. REJECTED.

**Option 4 — logical head + amortized compact (RECOMMENDED):** keep the
`Vec<ExecutionRecord>`; add `head: usize` — `[..head]` is the physically
present but already-summarized prefix, `[head..]` the live tail.

- *Per-cut cost:* the firing hashes the chunk (read-only, :2771-2776 —
  unchanged, required for chain byte-identity) then bumps `head += cut` —
  **zero bytes moved, zero allocations.** A periodic amortized compact
  (`Vec::drain(..head)` — still allocation-free, capacity retained) fires
  when `head ≥ live_len` (classic amortization): each record is physically
  moved at most once per *discard-batch*, giving **total moves ≤
  discarded-record bytes over any prefix of the run** — amortized ≤ one
  move per discarded record, versus Option 1's one move per *retained*
  record *per firing*.
- *Physical residency:* live + dead < 2 × live at the trigger — a
  documented worst case of `2 × resident_bound_records` (the price of the
  amortization), never data discarded to hit it, and the trigger is
  configuration-visible. For drivers that need the hard 1× bound, the
  trigger `head ≥ max(chunk, live)` trades a shorter slack for more
  frequent compacts — a documented knob, default `head ≥ live`.
- *Slice contract:* `&self.ledger[head..]` **is** a `&[ExecutionRecord]`
  — `ledger_retained` (:1282) returns the suffix slice; every consumer
  site is source-compatible.
- *Invariant making it safe:* `head > 0 ⟹ compacted.is_some()` — the dead
  prefix exists only after a compaction, and a `Full` spine always has
  `head == 0`, so the full-form save (:4262-4265), `ledger()`, the
  full-history fingerprints (:3017-3028, :3109-3110) and
  `try_ledger_full` (:1289) never observe a dead prefix. Restore always
  rebuilds tight (`head = 0`) — the decoder is untouched.
- *Wire identity:* the v2 section encodes only the live tail
  (:4312-4313 switched to `[head..]`); a live spine with a dead prefix and
  its restored tight image encode **identical bytes** (the dead prefix is
  pure physical slack, never logical, never serialized).
- *Determinism:* `head` is a pure function of the cut sequence
  (`head = Σ cuts since the last amortized compact`); the fingerprints
  hash the live view; the logical content is bit-identical under both
  storage shapes.
- *Allocation pins:* the main-path pins (spine_allocations.rs, no-policy
  windows) are untouched — no policy ⇒ the checkpoint is a no-op
  (:2675-2677) and `head` stays 0. The sustained loop stays
  zero-allocation (head bump and drain never allocate; the capacity
  discriminator :6974 still holds — after any firing,
  `capacity ≥ old_live = new_live + cut > new_live`).
- *The must-switch site list* (the complete `self.ledger` inventory at
  23c5249, 41 grep matches triaged): **switch to the live view** —
  :1272 (`history_kind` retained_records), :2759/:2771/:2781 (compact's
  window math, hash range, and removal — the drain becomes the head bump,
  plus the amortized-compact check), :2871 (lag), :2919 (checkpoint
  length), :3290-3291 (compacted_fingerprint), :4312-4313 (v2 save),
  :1283 (`ledger_retained`); **safe by the head>0 ⟹ compacted invariant**
  — :1262, :1293, :3017-3018, :3109-3110, :4262-4263 (Full-only paths);
  **unchanged** — :3490 (push), :4225 (restore-time max derivation,
  always tight), the test-site accessors (they use the public views).

**Recommendation: Option 4**, landed as commit 2 after the frontier — with
the honest note that at the *measured* retain windows (≤ 300 records) the
drain's absolute cost (≤ 16 KB per firing, likely below the SHA-256 cost
of the same firing — estimate, not measurement) does not make Option 1
incorrect today; Option 4 wins because it removes the linear-in-retain
term entirely at zero invariant cost, and the audit's bar is exactly that
the bytes-moved curve be measured and bounded, not merely small at the
demo scale.

---

## 8. Honest limits

- **The engine cannot verify a consumer's vouch.** An `acknowledge_retention_through` call is still a caller assertion — named, monotone, per-cut, observable, non-persisted, but not cryptographically proven against what the consumer read. Full consumer-side proof (content-bound acks hashing the tail the consumer absorbed) was considered and deferred: it adds a per-ack O(live) hash with no enforcement against a lying consumer, which is outside the engine's trust boundary. The design's claim is precisely: *no standing assertion, no fabricated proof, every cut min-vouched, every refusal observable* — not "consumers cannot lie."
- **No measurements were taken in this lane** (read-only): the ~40 B stride, the per-firing memmove/SHA-256 wall-clock ratio, and the 2× worst-case residency under Option 4's trigger are data-structure reasoning and CA-07's published operation counts. The landing must measure: `size_of::<ExecutionRecord>()` asserted, the amortized-move bound witnessed over a sustained run, the sustained loop's allocation count (must stay zero), and the main-path pins (must not move).
- **The register rule pushes the join-time impossibility to the driver:** a late-joining consumer that needed already-summarized records fails at register (typed refusal) rather than silently — but the driver must actually handle that refusal; the engine cannot make history reconstructible (by design, spine.rs:185-189).
- **The empty-frontier refusal means an armed-but-unregistered cadence never cuts** — a driver that arms the frontier and forgets to register consumers will see `frontier_hold_ticks` grow, not cuts. This is deliberate (fail-closed) and must be documented at `arm_retention_frontier`.
- **Ordinal-space acknowledgments assume the consumer counts what it reads.** The `retention_ordinal_of` convenience is O(live) and driver-cadence only; a consumer that mis-derives its ordinal over- or under-acks. Under-acking is safe (holds the frontier); over-acking is the same trust boundary as limit 1.
- **The `crossing.rs` session keeps its own independent eligibility** (:1999-2028); this design does not unify the session's episode boundary with the spine frontier (they police different prefixes — session lanes vs execution records). A future composition running both must keep them coherent itself; flagged for the orchestrator.

---

## 9. Untouchable invariants — how each is preserved

1. **Canonical (due, seq) order:** untouched — retention never reaches the
   queues, their heap layout, or the sorted lane encodings (:4288-4296);
   the ledger is execution-ordered (append at :3490) and the frontier/
   head operate on its prefix only. No ordering site changes.
2. **Wire bytes for identical cut sequences:** the encoder is unchanged
   (Option A, §3); the frontier only vetoes/clamps firings upstream of
   it; the head only changes which physical bytes are *absent*, and the
   v2 section encodes the live tail (§7) — pinned byte-identical by the
   extended :6487 discipline (test §5.2) and the dead-prefix round-trip
   test (§5.9).
3. **The fingerprint domains** (`engine-core/spine/2` :2977,
   `state/2` :3039, `spine-lanes/1` :3067, `spine-compacted/1` :3242,
   `state-compacted/1` :3320): the frontier and consumers are
   configuration-class state (never hashed — the :3777-3784 boundary);
   the head is physical layout (never hashed; the compacted domains hash
   the live view, whose logical content is identical under both storage
   shapes).
4. **The spine allocation pins** ((18, 6880)/(9, 3024) cold/warm,
   spine_allocations.rs — thread-attributed, investigation-not-repin):
   no main-path allocation is added (usize field, map allocated only at
   the configuration boundary, acks are u64 writes); the sustained
   retention loop stays zero-allocation (head bump and drain allocate
   nothing, capacity retained); the no-policy checkpoint remains a no-op.

---

*Prepared for the ODU orchestrator session. Every spine.rs citation above
was resolved by direct read at base 23c5249 in
`/home/coder/PostRust-worktrees/t07`; the audit's 096f8d9-based line
numbers were not reused without re-resolution.*
