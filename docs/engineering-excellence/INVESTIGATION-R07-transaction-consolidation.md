# R07 — The Memory-Transaction Consolidation Investigation

**Design record + census, the `residency.rs` precedent shape.** Lane
`ee/r07-transaction-consolidation` (base `30126fa`, the CA-09
integration). The finding: Sol re-audit (2026-11-27, at `8d34771`),
R07, minor — *"The memory transaction lifecycle remains structurally
complicated and copy-heavy"* — adjudicated **ACCEPTED (investigate
safe consolidation)** and never dispatched until this lane.

What "memory-transaction consolidation" names: whether the
transaction-bookkeeping surfaces — the authority's tracked-key map,
the verifier's confirmed map, the pending slots, the receipts, the
watermark state, the shed sets, and the journal's decoded history —
can or should consolidate their memory representations. This record
is the investigation; §7 lands its bounded first step (the census),
following the CA-17 discipline: the measurement first, the decision
on the measurement.

Related, already landed (the architecture this investigation reads
against): **A22** (the settled-transaction watermark + coverage
shedding — WORK bounded, not residency), **B03** (the commit path's
key-set copies 3 → 1; the composite reader's `Arc<[u8]>` backing —
the one consolidation that measurably paid), **B02** (the streaming
framed open — the open-path working set), **CA-17** (`residency.rs`:
the confirmed-identity residency design record, `cold_confirmed`, and
the first measurement surface). Related, open: **Q08** (the owner
re-audit's proportional-active-working-set mandate — this record and
CA-17 are its machinery), **CA-17 stage 2** (the gated eviction).

---

## 1. The finding, verified against source

Every claim below was read against `crates/engine-persist/src` at the
lane base before any disposition. The lifecycle IS structurally
complicated — deliberately: five per-key phases across two
deliberately split halves (the authority journal-side, the verifier
chain-side), a retained-attempt state machine with
write-at-uncertainty-or-acceptance semantics, a settlement watermark
with an out-of-order bitmap, and a coverage shed with an eligibility
queue. And it IS copy-heavy in places — measurably less so than the
audit saw: the B03 lane already removed the two redundant key-set
copies on the commit path. What remains is mapped in §4.

The question is not whether the complexity is real but whether
**consolidating the memory representations** is warranted at the
measured scales — the A17/B07 question shape, answered the same way:
measure, then decide.

## 2. The surface map

Every transaction-bookkeeping surface, its memory shape, lifetime,
invariants, measured residency, and overlap. The measured numbers are
this lane's census (§8; local file medium, 256-identity batches, 8
fixed live-unsettled envelopes, x64 debug build — the CA-17 fixture
discipline; the formulas live at the cited code sites).

| # | Surface (site) | Shape | Lifetime | Measured |
|---|---|---|---|---|
| 1 | Journal decoded history (`T1Journal.state.committed`, `cancellations`, `committed_index`, `canceled_ids`; `T1Journal.acknowledged`) | `Vec<CommittedTransaction>` — per transaction the skeleton (id 32 B, sequence/count 16 B, digest 32 B, position, classification) + **complete envelopes** (fixed ≈ 249 B + sub-span heap 24 B/span + dependency heap) + plan + frontier + causal deps; plus per-transaction index/canceled/acknowledged nodes ≈ 88–128 B | session; **re-derived at every open** (the whole journal decoded; B02-L5 bounds the open's working set, not this state); grows within the generation; **bounded generationally by the rotation** (the reopened journal decodes the smaller file) | **≈ 281–282 B/identity** |
| 2 | Tracked-key map (`CommitAuthority.keys: BTreeMap<ConsequenceId, KeyStatus>`) | per identity: key 16 B + `KeyStatus` 67 B (phase + `Option<TransactionId>` 33 + `Option<Hash>` 33) + BTree node ≈ 48 B | session; re-seeded at open from the journal; bounded generationally by the rotation | **≈ 131 B/identity** |
| 3 | Confirmed map (`CommitVerifier.confirmed: BTreeMap<ConsequenceId, ConfirmedEntry>`) | per identity: key 16 B + entry 128 B fixed (coverage `Vec` 24 + transaction 32 + effect 32 + revision 32 + epoch 8) + node ≈ 48 B + coverage heap 24 B/span | session; rebuilt at open (O(chain) walk); **insert-only** (a conflicting claim fails closed); membership NEVER dropped (the L6 contract) — the CA-17 finding; bounded only by chain-reachable history | **195–216 B/identity** (CA-17 rows, reproduced at the lane base) |
| 4 | Shed sets (`CommitVerifier.shed_eligible: VecDeque<ConsequenceId>`, `shed_coverage: HashSet<ConsequenceId>`) | queue: 16 B × ≤ 1,024 (bounded, `SETTLED_COVERAGE_RETAINED`); discriminator: ≈ 48 B per shed identity, **unbounded in the shed population** | session; the discriminator lives as long as the confirmed map | **48 B/shed identity** (0 below the bound; 36,864 B at 2,048; 331,776 B at 8,192) |
| 5 | Watermark state (`CommitAuthority.settled_watermark`, `settled_above`, `newly_settled_keys`, scan counters) | O(1) mark + ≈ 80 B per out-of-order-settled transaction (absorbed as the mark advances) + the drain vec (one commit's settlements) | session-derived; lazily re-proven at the first actionable scan (O(journal) once) | **O(1) at fixed live obligations** (the watermark working as designed) |
| 6 | Staged accrual (`CommitAuthority.staged: Vec<ConsequenceEnvelope>`) | full envelopes (fixed ≈ 249 B + heaps) | **boundary-scoped** — drained by the capture; a crash loses it honestly (P0-5) | **0 post-capture** (bounded by one boundary's batch, never by history) |
| 7 | Pending slot (`CommitVerifier.pending: Option<PendingAttempt>`) | at most ONE: record (≈ 184 B + idempotency/content-ref/dependency heaps) + batch hash 32 + keys map (per key ≈ 152 B + coverage heap) + ticks map (≈ 88 B/transaction) | **attempt-scoped** — written at uncertainty-or-acceptance, cleared only by verified confirmation; exception-safe restore on every failure arm | **0 on the clean path**; the single retained copy (P3-L3) |
| 8 | Receipts (`CaptureReceipt`, `ConfirmationReceipt`, `JournalReceipt`) | 16 B/key in the confirmation receipt's `keys` vec; the rest O(1) Copy fields | **caller-scoped** — returned, never retained by the pipeline (the honesty model: receipts report, they are not cached truth) | transient |
| 9 | Walk-local memo (`walk_chain`'s `classified: HashMap<Hash, ObjectClass>`) | per examined object ≈ 72 B | **walk-scoped** — dies with the walk (the S6-C4 corruption-between-walks guarantee) | transient |
| 10 | Medium side (trusted cursor, provider stats; the composite memo; the worker ring) | O(1) cursor (the ten-field L9b core); the memo per-operation with `Arc<[u8]>` sharing (B03); the ring bounded 256 slots (B13) | session / operation / bounded | bounded by construction |

**The total, at fixed live obligations: ≈ 649 B/identity** across the
four history-bearing surfaces (1)+(2)+(3)+(4) at the 8,192-identity
census scale — 5,319,513 B ≈ 5.07 MiB. At the shipped corner scale
(the E-8 memory-lane flushes are **tens of keys** per commit against
the 65,536 `MAX_BATCH_ENTRIES` ceiling; sessions of hundreds to low
thousands of identities), the total bookkeeping is **tens to hundreds
of KiB**.

## 3. The overlap analysis — which duplication is load-bearing

For one settled-confirmed identity, four in-memory records coexist:
the journal envelope (§2.1), the `KeyStatus` (§2.2), the
`ConfirmedEntry` (§2.3), and the shed discriminator slot (§2.4). Three
fields repeat across them: the producing `TransactionId` (in 1, 2, 3),
the coverage (in 1 and 3), the confirming revision (in 2 and 3). Is
this waste?

**No — and this is the investigation's decisive find.** The journal
envelope and the `ConfirmedEntry` are **independently derived from
different authoritative sources**: the envelope is decoded from the
T1 journal file; the entry is decoded from the medium's hash-verified
content objects through the chain walk. The exact-provenance
confirmation rule **compares them** — `collect_actionable`:
`entry.transaction == transaction.id && entry.coverage ==
envelope.sub_spans && entry.effect == effect` (likewise
`remaining_keys` and `confirm`'s per-key validation). The agreement of
two separately-derived records IS the confirmation proof — the
architecture's own rule: *"never a bare `Accepted`, never inferred
from time, local receipts, counter matches, T1 fsync, or caches."*
Consolidating the two records into one shared entry would make the
comparison vacuous — the check would compare a record against itself.
**The duplication is not waste; it is the two-source witness.** (The
same argument holds for the read-back `confirm`: the batch is
re-decoded from the medium, never aliased from the sender's buffer.)

The `KeyStatus` map (§2.2) is the bridge: the phase cache the
promotion loop advances and `status()` queries read. It duplicates
state derivable from the journal (the producing transaction) and the
confirmed map (the revision) — ≈ 131 B/identity, the price of O(log n)
per-key status queries **and** of the half-split itself: the authority
owns it precisely so the verifier never needs journal access (the
`CommitRequest` freezing discipline). The half-split is not an
accident to be consolidated away — it is the documented
actor-composition architecture: *"This is the half a future
verification actor leaves on the authority thread"* /
*"...the half a future verification actor owns on its own thread"*;
*"A future verification actor replaces this composition's middle step
with a queue and a receipt; the halves themselves are unchanged."* A
keyed-lane consolidation across the halves couples them and mixes the
trust domains (journal-derived vs chain-derived), regressing that
designed future.

## 4. The copy path (the "copy-heavy" half, censused)

Per canonical commit, per actionable key:

1. `collect_actionable` clones the envelope's `sub_spans` into the
   `AttemptedKey` (the `CommitRequest`'s frozen evidence — deliberate:
   the request is the immutable handoff to the verifier, journal-blind
   by construction).
2. `append_for` clones the coverage again into the `BatchEntry` — the
   copy that feeds `canonical_bytes`, the durable artifact's encode
   input.
3. The encoded batch bytes (transient) and the `StoredObject`.
4. `confirm` re-decodes the batch **from the medium** (the read-back
   rule — a fresh derivation, never the sender's memory) and moves the
   entries into the confirmed map.

**Already consolidated where it paid (B03, P3-L3):** the commit path's
key-set copies went 3 → 1 — `keys` MOVES into the retained
`PendingAttempt` (the single copy the confirmation borrows; the
stale-parent re-derivation reads it back from the slot), and the
composite reader's payloads share one `Arc<[u8]>` backing
(`MemoEntry.payload`/`VerifiedChunk.payload` — NEW-PERSIST-04's
disposition). What remains: two bounded coverage clones per actionable
key per commit (1) and (2) — at the shipped flush scale (tens of
keys, ≈ 24 B/span) that is **single-digit KiB of transient churn per
commit**, inherent to the freeze-then-encode-then-read-back
verification discipline. The `walk_chain` descent also materializes
the walked suffix's records (`chain: Vec<(Hash, RevisionRecord)>`)
before the reverse collection pass — O(new revisions) within a
session, O(chain) once at open (the documented honest cost).

## 5. The options assessed

**(a) No consolidation now (no-change-with-trigger).** The A17/B07
precedent disposition. For: the measured totals (§2) are immaterial
at the shipped scales; the dominant growth terms are bounded
generationally (the rotation reseeds surfaces 1–3 from the smaller
journal) or designed-bounded (CA-17 stage 2 for surface 3–4); the
duplication is load-bearing (§3); the copy path already had its
measured win (B03). Against: none at the measured scales.

**(b) The discriminator fold** — `shed_coverage: HashSet<ConsequenceId>`
(§2.4's unbounded 48 B/shed-identity term) folded into a flag on
`ConfirmedEntry` (the entry has the information; the set exists only
because an empty coverage `Vec` is legitimate for span-less movements
— a `shed` flag distinguishes exactly as well). Saves the unbounded
term and makes CA-17 stage-2's eviction trivially consistent (the
discriminator dies with its entry). Costs: `ConfirmedEntry` is public
(returned by `cold_confirmed`), `RetentionStats.shed_residency_bytes`
loses its subject, and it restructures the stage-2 substrate **mid-
gate** — stage 2 is designed and gated on the owner-rig cold-latency
rows; folding now churns the exact surface that lane will rebuild.
**Disposition: fold it INTO CA-17 stage 2** (a design note there, not
a standalone change). Trigger: stage 2 lands, or the census shows the
shed term material at the owner-rig scale.

**(c) The keyed-lane consolidation** — one `ConsequenceId`-keyed map
of phase-tagged records spanning the authority and verifier halves.
**Rejected on the architecture:** couples the actor halves (§3),
vacates the two-source exact-provenance witness (the confirmation
proof compares journal-side vs chain-side records — sharing one
record makes the check vacuous), and churns every call site
(`stage`, `collect_actionable`, `remaining_keys`, `promote_from`,
`confirmed_evidence`, the rotation's classification, the recovery
composition) to save ≈ 131–216 B/identity of ≈ 649. The B07 lesson
applies: per-call validation against the raw source IS the safe
boundary; the maps are caches over separately-owned truths.

**(d) The journal-side settlement trim** — the coverage shed's mirror
on surface 1: drop settled transactions' envelope payloads from the
decoded state, keeping the skeletons (id, sequence, digest, position).
The eligibility proof is the shed's own (settled = never actionable
again; the watermark's scan never revisits; cancellations are
append-only; confirmation monotone) — and surface 1 is the LARGEST
per-identity term (≈ 281 B). This is a real future option and the
honest answer to Q08's journal half. Costs: the `journal()` public
accessor exposes `transactions()` to the rotation's classification,
the recovery composition, and the diagnostics — a full **consumer
census of every `transactions()` reader** is the mandatory first step
(the R07 lane did not run it; it is recorded as option (d)'s gate);
the on-disk journal remains the authority, so a re-read (the B02-L5
frame reader) re-derives any trimmed detail. **Disposition: designed
here, gated on the same owner-rig data point as CA-17 stage 2/3** —
the census (§7) is its observable. At the rotation-cadence bound
(4,096 transactions × tens of keys ≈ 82k identities ≈ 23 MB of
envelopes) it would matter; under maintenance rotation it does not.

**(e) Arena/Arc backing for the coverage Vecs** (the composite
reader's B03 precedent applied to `sub_spans`/`coverage`). The
equality comparisons are value equality — sharing does not vacate
them, and the sources differ, so the witness argument does not apply.
But the win is the per-span heap only (≈ 24 B × spans × 2–3 copies ≈
48–72 B/identity), against public-type churn (`envelope.sub_spans`,
`ConfirmedEntry.coverage`, `AttemptedKey` are API shapes) and
capacity divergence between the surfaces. **Disposition: micro; not
warranted at any measured scale.** Recorded as the technique of
record IF (d) or stage 2 ever restructures these types.

## 6. THE RECOMMENDATION

**No consolidation at the measured scales — the no-change-with-
trigger disposition (the A17/B07 precedent shape).** The reasoning,
in order of weight:

1. **The measured scales don't justify the churn.** ≈ 649 B/identity
   total across all bookkeeping surfaces; tens-to-hundreds of KiB at
   the shipped corner; ≈ 5 MiB at the 8,192-identity census scale —
   against per-option churn measured in public-API ripples and
   battery re-proofs.
2. **The dominant duplication is load-bearing** (§3): the journal-side
   vs chain-side records are independent witnesses whose agreement is
   the confirmation proof; the half-split is the documented
   actor-composition architecture.
3. **The growth is already bounded or designed-bounded:** the
   watermark bounds WORK; the rotation bounds the generation
   (surfaces 1–3 reseed from the smaller journal); CA-17 stage 2
   (gated) owns surfaces 3–4 with the discriminator fold (option b)
   noted for it; option (d) is the designed journal-half answer to
   Q08, gated on the same data point.
4. **The copy path already had its measured win** (B03: 3 → 1); the
   remaining clones are the freeze/encode/read-back discipline's own
   requirement.

**[ASSUMPTION]** (owner decisions this record defers): the trigger
threshold — the total-bookkeeping budget at the intended first-game
scale (the census is the observable; the number is the owner's); the
census formulas' node-overhead constants (48 B BTree / 32 B HashSet
amortized shares — std's exact occupancy is its own business; the
CA-17 estimate discipline); option (d)'s per-generation envelope
budget if ever taken.

**Owner:** the persist area (this record's options (b)/(d) fold into
CA-17 stage 2/3's existing owner decisions).

**The trigger (revisit when ANY fires):**
- the owner-rig residency data point shows the census total exceeding
  the documented budget at the intended first-game scale (the same
  data point `SETTLED_COVERAGE_RETAINED` and CA-17 stage 2 await);
- CA-17 stage 2 lands (take the discriminator fold with it);
- a verification-actor composition lands (re-assess the half-split's
  memory shapes against the queue/receipt boundary it introduces);
- a multi-writer or long-generation workload pushes the journal past
  the rotation cadence without maintenance (take option (d)).

**Mandatory tests if any option is implemented:**
- (b): the L6 shed battery re-proven (the boundary test's 376-row
  shape), `confirmed_coverage`/`confirmed_evidence` refusal arms, the
  residency formulas' structural linearity, the CA-17 cold/hot
  equivalence battery unchanged.
- (c): **not implementable without vacating the exact-provenance
  witness — the recorded refutation is the finding**; if attempted,
  the provenance-mismatch fault tests (`ConfirmedProvenanceMismatch`,
  `CanonicalIdentityConflict`) must still fail closed against
  two-source divergence, which a shared record cannot express.
- (d): the consumer census of every `journal().transactions()` reader
  first; then the rotation battery, the recovery composition
  equivalence (fresh-open re-derivation), the L6 watermark battery,
  and the census test's journal row re-pinned with the trimmed shape.
- (e): the B03 `Arc::ptr_eq` sharing proof + the full
  equality-comparison battery (envelope ↔ entry, attempt ↔ entry).

## 7. The bounded first step (LANDED, this lane)

The census — the residency.rs stage-1 precedent shape (a measurement
surface + structural tests, zero behavior change to any existing
path):

- `RetentionStats` gains four estimated-residency fields
  (`tracked_residency_bytes`, `staged_residency_bytes`,
  `pending_residency_bytes`, `journal_residency_bytes`), the CA-17
  formula discipline (fixed struct + heap capacities + key + amortized
  node share), documented at the fields and the estimators
  (`CommitAuthority::tracked_residency_bytes` /
  `staged_residency_bytes`,
  `CommitVerifier::pending_residency_bytes`,
  `T1Journal::decoded_residency_bytes`).
- The census test
  (`residency::tests::r07_bookkeeping_census_grows_linearly_at_fixed_live_obligations`)
  pins the structural claims: the history-bearing surfaces linear at
  fixed live obligations; the boundary-scoped surface (staged) and
  the attempt-scoped surface (pending) at their fixed values; the
  shed term silent below the retention bound and fired past it.

No existing path changed behavior; no wire format touched; the stats
remain read-only derived state (never serialized, never a fingerprint
input). Gates: fmt clean, clippy `-D warnings` clean, the persist
battery green both feature sets (746 total / 650 lib with sqlite;
621 total / 525 lib local-only — the +1 census test each), the
workspace `cargo check --workspace --all-targets` clean.

## 8. The measured rows (this lane's run, x64 debug, local medium)

```
r07 census row: history 512  — journal 146,870 B (~282 B/id), tracked  68,120 B (~131 B/id),
                confirmed 110,592 B (~216 B/id), shed 0 B, staged 0 B, pending 0 B   → 325,582 B
r07 census row: history 2048 — journal 579,482 B (~281 B/id), tracked 269,336 B (~131 B/id),
                confirmed 423,936 B (~207 B/id), shed 36,864 B, staged 0 B, pending 0 B → 1,309,618 B
r07 census row: history 8192 — journal 2,309,953 B (~281 B/id), tracked 1,074,200 B (~131 B/id),
                confirmed 1,603,584 B (~195 B/id), shed 331,776 B, staged 0 B, pending 0 B → 5,319,513 B
```

(Per-identity columns divide by history + 8 for the journal and
tracked surfaces — they carry the fixed live obligation's identities
too. The confirmed-map latency pair, same fixture: cold lookup
6.7 / 22.3 / 81.1 ms; 1,000 hot lookups 231 / 237 / 286 µs — the
CA-17 stage-2 gate input, unchanged by this lane.)

Printed, never pinned (the pod-observational discipline); the
structural claims are the assertions, the numbers are the design
input.
