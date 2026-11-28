# T29 — The Shared Chain Walker's Bounded Startup/Suffix Residency: Design

**Author:** T29 design lane (glm-5.3-w4a8 subagent, subordinate to the ODU
orchestrator session)
**Status:** COMPLETE — source-cited design for orchestrator review; no source
edits made (read-only lane).
**Read base:** worktree `/home/coder/PostRust-worktrees/t29`, branch
`ee/overnight-t29-walker-design`, base `3db8a63`. **All line numbers below cite
this tree.** The audit's citations (`chain_walk.rs:222–472,492–610` at
`096f8d9`) were re-resolved at this base: `walk_chain_excluding` sits at
:222–469 and `walk_suffix` at :492–600 — the file is the L9b landing's own
(`10d3d13`) and has not changed since; every claim below was read against the
current tree, not the audit SHA.
**Scope:** strict-audit-IV finding T29 (S2; A13/A22/B02/B03) — the shared
chain walker carries large transient memory (collected walked/suffix records,
BTree maps, reclaimed indexes), repeated reads, and O(chain) startup.
**Constraints honored:** DESIGN ONLY — no file under `crates/` was touched; no
cargo commands run (all costs reasoned from the data structures and labeled as
estimates where they are estimates, per the R07/CA-17 convention); no new
dependencies (nothing proposed below needs a crate the workspace does not
already pin: `sha2` is the existing hasher, `rusqlite` is the existing sqlite
dep); no provider trait hierarchy (the `ChainReads` seam stays the
`BeginReads` narrowness — loading operations only, never the rejected
`ChainWalkProvider`); the canonical chain stays the authority; the L9b
differential battery is extended, never weakened; **if the durable format must
change, that half is PARKED with the decision-record shape and not decided**
(§7, Appendix A).

---

## 0. Thesis

The finding's single phrase — "large transient memory" — conflates three
lifetime classes with three different dispositions, and the design's whole
structure comes from separating them:

1. **The session-resident trusted cursor** (`WalkCursor`, chain_walk.rs:119–143):
   `seen_keys`, `reachable`, `record_ids`, `reclaimed` are O(chain) **by
   design and load-bearing**. They are the substrate of the O(1) per-commit
   duplicate-key and dependency-membership checks (local/head_protocol.rs:104,
   :117; sqlite.rs:672, :680) that the §5.3.2 contract and the providers' own
   O(1)-cursor test pins promise. The audit's own instruction — "avoid
   conflating work and residency" — cuts both ways: this residency is the
   price of the per-commit work bound. **Not removable; only its constant
   factor is** (§4.5, and only behind the parked owner decision).

2. **The walk-transient materialization** — the `walked` triples
   (chain_walk.rs:237), the `positions` map (:424), the staged MK indexes
   (:242), the per-suffix decoded records (:501, :590), and above all the
   **whole-object byte buffers**: every content reference is loaded as a
   complete `Vec<u8>` (:285–293, :578–583) to hash-verify it and — for the
   first reference — to **peek a domain that lives in the first ~60 bytes**
   (:298–299, :614), and every composite chunk is loaded whole AND copied
   again inside `verify_chunk_dependency` (composite_append.rs:150,
   `bytes.to_vec()`). **Removable with no durable-format change and no proof
   traded:** the SHA-256 needs every byte but never *simultaneously* — stream
   it (the L5 `FrameReader` discipline, `79b2fde`, is the in-repo template),
   peek the domain from a bounded prefix, and decode the chunk envelope header
   from a prefix while the hash streams (§4.1).

3. **The per-validation transient** — the one the audit's line citations
   point at but never name: `validate_observed` **deep-clones the cursor's
   `seen_keys`, `record_ids`, and `reclaimed`** (chain_walk.rs:694–704) before
   *every* divergent-head suffix walk, passing them by value into a consumer
   that only reads them; the local session-open orphan sweep clones the whole
   `reachable` set the same way (local/orphan.rs:89–93). Each clone is a full
   O(chain) allocation-and-free per external append observed. **Removable for
   free:** borrow (§4.2). This is the single highest-value line-level fix in
   the finding and it changes no observable whatsoever.

The bounded design therefore lands in two commits that touch no wire format —
stream the object verification, borrow where the code clones, memoize reads
only within one coordination scope (the S6-C4 walk-local-memo precedent,
pipeline.rs:2058–2069; a **cross-scope** byte cache is refuted in §3.4: it
suppresses the referenced-corruption refusal and fails open) — and **parks the
one true asymptotic fix**, the authenticated checkpoint/suffix summary, as the
owner decision: it requires new durable store contents (a content-addressed
summary object written at compaction and referenced by the MK record), it
converts per-open prefix re-verification into publication-time verification
plus tamper-evidence (a real trust delta, §4.5.4), and the standing rule says
a design record does not decide durable-format questions. It is designed here
to the field level with its trust boundary drawn exactly — it may short-circuit
*cursor establishment*; it may **never** feed the reclaim-execution survivor
walks, deletion, or the exact-provenance confirmation — so the owner decision
is a real choice with the costs named, not a blank check.

---

## 1. The finding, verified at 3db8a63

### 1.1 What the walker materializes, where

Every item below was read in the current tree. Line citations are the
evidence.

| Shape | Site (chain_walk.rs unless noted) | Lifetime |
|---|---|---|
| `seen_ids: BTreeSet<Hash>` (becomes `record_ids`) | :227, moved at :461 | walk → cursor |
| `seen_keys: BTreeSet<Vec<u8>>` | :228 | walk → cursor |
| `reachable: BTreeSet<Hash>` | :229 | walk → cursor |
| `walked: Vec<(Hash, u64, Vec<Hash>)>` — id, epoch, `depends_on` clone per record | :237, pushed :333 | walk-transient |
| `staged_indexes: Vec<ReclaimedIndexEntry>` — every on-chain MK's full index | :242, extended :327 | walk-transient |
| `checkpoints: Vec<Hash>` | :243 | walk-transient |
| `skip_table: Option<(Hash, CheckpointMetadata)>` — the most recent MK's full metadata | :247 | walk-transient |
| per-record `Vec<u8>` from `read_record_bytes` + decoded `RevisionRecord` | :253–254 | record-transient |
| per-object `Vec<u8>` from `read_object_bytes`; `first_object` held across refs 1..n | :284–293 | object-transient (peak ≈ 2 objects) |
| composite manifest decode + per-chunk whole load + the `to_vec()` copy inside `verify_chunk_dependency` | :308–323; composite_append.rs:150 | chunk-transient (peak ≈ 2× chunk) |
| `reclaimed: BTreeMap<Hash, u64>` merge target | :392 | walk → cursor |
| `positions: BTreeMap<Hash, usize>` — the deferred ancestry index | :424–427 | walk-transient |
| suffix: `Vec<VerifiedSuffixEntry>` — a **full decoded `RevisionRecord`** per suffix record + `composite_closure: Option<Vec<Hash>>` | :161–165, :501, :590 | suffix-transient |
| **the three cursor-set deep clones** in `validate_observed` | :694–704 | per-divergent-head transient |
| **the `reachable` clone** in the local session-open sweep | local/orphan.rs:89–93 | per-open transient |
| the pipeline's own `chain: Vec<(Hash, RevisionRecord)>` at open (the verifier's confirmed-map rebuild — a second, separate O(chain) pass) | pipeline.rs:2048 | open-transient (out of this lane's scope; §8) |

### 1.2 The call-site map (work, not residency)

- **Cold open** — local `open` (local/layout.rs:76–78 → `cold_walk_from`),
  `validate()` (local.rs:301–316, cursor deliberately discarded first),
  sqlite `open` (sqlite.rs:433) and reconcile (sqlite.rs:838, :862):
  O(chain) record reads **plus O(total stored content bytes) object reads**
  (every referenced object loaded whole to hash it).
- **Per commit** — `head()` → `validate_observed` (local.rs:204–214,
  sqlite.rs:550–559): O(0) on the verified head, O(suffix) on external
  divergence, O(chain) rebuild on sound invalidation (chain_walk.rs:725–734).
  The append itself advances the cursor O(1) (chain_walk.rs:752–825) and
  checks keys/dependencies against the cursor O(1)-per-query.
- **Reconcile** — local/head_protocol.rs:407–408: `cursor = None` then a
  **full cold walk per reconcile call**. Not per tick, but per ambiguous
  attempt — an O(chain) unit the cold/per-commit analysis must carry
  (§5).
- **Reclaim execution** — sqlite/reclaim_exec.rs:555, :780: fresh
  `walk_chain_excluding`; the survivor set is the **fresh walk's** `reachable`
  (:589, :781), never the session cursor's. This is the load-bearing fact for
  the summary's trust boundary (§4.5.3).
- **The orphan sweep** — local/orphan.rs:88–106: session-open, lock-held,
  **quarantines (never deletes)** objects not in the cursor's `reachable`.

### 1.3 Repeated reads, precisely

Within one walk: a content object referenced by K records is read and
hash-verified K times (the `reachable` set dedups the *count*, not the *read*;
chain_walk.rs:285–293). `read_record_bytes` is once per record (the cycle
guard :250–252 covers re-visits). The first-reference domain peek consumes the
**whole object's bytes** for a ~60-byte prefix need. The composite closure is
verified exactly once per manifest (the R15 win, retained). Across calls:
`validate_observed`'s suffix walk re-reads records a previous cold walk
already verified — correct, different coordination scope; the reclaim walks
re-read everything — deliberate exactness. Sqlite re-fetches the full blob per
read (`sqlite/rows.rs` load fns); local re-reads the whole file per read
(`local/layout.rs:185`, `fs::read`).

---

## 2. The residency census

**The estimate discipline (R07 §2 / CA-17, inherited):** every number below is
a **labeled estimate** reasoned from the data structures; std's exact B-tree
node occupancy is its own business, so the node share is taken at the R07
convention (**≈ 48 B amortized per element**); `Vec` capacity slack is taken
at up to 2× length (doubling growth); the idempotency key length `k` is taken
at **16 B** (the `ConsequenceId` width the R07 census measured); `Hash` is a
fixed 32 B (hash.rs:21, :25). **The landing's census run (§6, commit 1)
replaces every one of these with a measured row; nothing here is a
measurement.**

Symbols: `N` retained records walked, `C` distinct reachable content objects
(including composite chunk identities), `K` distinct idempotency keys, `R`
reclaimed-index entries, `D` total `depends_on` edges, `M` on-chain MKs, `S`
suffix records, `o_max` the largest single content object's byte size,
`E` total staged index entries across all MKs.

### 2.1 Class 1 — cursor-resident (session lifetime; O(chain) by design)

| Field | Formula (estimate) | At the census example |
|---|---|---|
| `reachable` | (32 B hash + 48 B node) × (N + C) = **80·(N+C)** | 80 × 20,480 = 1.56 MiB |
| `record_ids` | 80·N (the record ids duplicate their `reachable` membership — §3.2 shows why both sets must exist) | 80 × 8,192 = 640 KiB |
| `seen_keys` | (24 B Vec header + k + 48 B node) × K ≈ **88·K** | 88 × 4,096 = 352 KiB |
| `reclaimed` | (32 + 8 + 48) × R = **88·R** | 88 × 2,048 = 176 KiB |
| fixed fields | head/epoch/founding/authority/`campaign_id` String/content_objects ≈ O(100) | — |

**Cursor total ≈ 160·N + 80·C + 88·K + 88·R B** — at the worked example
(N = 8,192 records — the R07 census scale; C = 1.5N; K = N/2; R = N/4):
**≈ 2.7 MiB (estimate)**. At the shipped corner scale (R07: sessions of
hundreds to low thousands of records) this is **tens to hundreds of KiB**.
This is the class the audit's "O(chain) memory, documented" note on
`seen_keys` (chain_walk.rs:122–124) already concedes; the census makes all
four fields' arithmetic explicit instead of one.

### 2.2 Class 2 — walk-transient (peak concurrent with the cursor being built)

| Shape | Formula (estimate) |
|---|---|
| `walked` | (40 B tuple + 24 B Vec header + 32·d_i) per record + up to 2× capacity slack ≈ **(64 + 32·d̄)·N**, slack up to 2× |
| `positions` | (32 + 8 + 48) × N = **88·N** |
| `staged_indexes` | (24 + k + 32 + 8 + Vec slack) × E ≈ **80·E** |
| `checkpoints` | 32·M |
| `skip_table` | one `CheckpointMetadata` (skips + index of the latest MK) |
| object byte buffers | **peak ≈ 2·o_max** (first_object held at :288 while refs 1..n stream through :286; each dropped after verify) |
| composite verification | peak ≈ **2× the largest chunk** (the whole load at :311 plus the `to_vec()` copy at composite_append.rs:150) |
| per-record decode | one `RevisionRecord` ≈ 160 B + 32·(refs+deps) heap, dropped per iteration |

**The dominant term is the object buffers.** With L5's derived frame maximum
(~5.25 MiB) as the stand-in for a large session-snapshot content object, the
cold walk's byte-transient peak is **~10.5 MiB (estimate, 2·o_max)** against a
cursor that is a few MiB — i.e. today the *transient* can exceed the
*resident* it is building, which is exactly the finding's "large transient
memory." After §4.1 this peak drops to **the stream chunk size (~64 KiB,
labeled) + one record decode**.

### 2.3 Class 3 — per-validation and per-open transients

| Shape | Formula (estimate) | When |
|---|---|---|
| the `validate_observed` clones | 80·N + 88·K + 88·R deep-copied **and freed** | **every** divergent-head `head()` call (chain_walk.rs:694–704) — the "repeated cursor advancement" scenario: an external writer appending between our validations makes each `head()` pay a full cursor-sized allocation churn |
| the sweep's `reachable` clone | 80·(N+C) | every local session open (orphan.rs:89–93) |
| suffix entries | ≈ (224 + 32·(refs+deps) + 32·chunks)·S | per divergent-head suffix walk — bounded by the divergence, the designed bound; S is not chain-scale |

At the worked example the clone set is **≈ 1.15 MiB allocated and freed per
divergent validation (estimate)** — pure churn, no residency, no proof value:
`walk_suffix` takes the sets by value and only reads them
(chain_walk.rs:497–499, :531, :562).

### 2.4 What the census does NOT claim

No absolute number above is a measurement; the debug-vs-release gap (R07
noted it for the journal surfaces), the allocator's behavior under the
clone-and-free churn, and sqlite's page-cache residency are landing-time
observables. The census's structural claims — linearity in N, C, K, R; the
2·o_max peak; the per-validation clone — are source-derived and are what
commit 1's census tests pin (§6.1).

---

## 3. What the proofs require — allow and prevent

The design rule the audit sets is "if proof requirements allow." Four
obligations govern the walker; each is stated with its memory consequence.

### 3.1 Storage integrity — every referenced object's bytes hash-verified

**Allows streaming.** The obligation is `SHA-256(bytes at identity) ==`
identity (the `ChainReads` contract, chain_walk.rs:96–99; hash.rs:1–12). SHA-256
is sequential (sha2's `Digest::update` is already the hasher's shape,
hash.rs:71–76) — the proof needs every byte *read*, never every byte *held*.
The domain peek (:298–299) and the chunk envelope header
(composite_append.rs:166–183: domain string, tag string, length-prefixed
payload) are **prefix-decodable** — a bounded prefix read answers them. What
streaming cannot serve: nothing found — the manifest decode (:309) needs the
root object whole, but the root is one object, the same one whose bytes are
already needed for the record's own reference; the design streams everything
else and holds at most one stream buffer plus (for composite roots) the root.

### 3.2 Key uniqueness — exact membership at append time

**Prevents cardinality reduction.** The append-side duplicate check
(head_protocol.rs:104, sqlite.rs:672) is a writer-side exact-membership query
against *all* prior keys. A digest cannot answer membership; a probabilistic
filter can only *fail closed* (a false positive refuses a valid chain — an
availability cost, never a correctness hole, so it is admissible only as a
pre-filter before an exact fallback the writer itself must own); a
Merkle-membership-proof server is the writer itself (no win); an on-disk key
index queried per append is a new durable sidecar (format change — parked with
the summary) and slower. **Conclusion: O(chain) key knowledge stays resident
in the session; only its representation constant is reducible (§4.5.5).** The
same argument holds for `record_ids`/`reclaimed` (dependency ancestry,
head_protocol.rs:117, sqlite.rs:680, chain_walk.rs:428–442): the writer needs
(hash → is-a-record, epoch) exactly.

The `reachable`/`record_ids` duplication (§2.1) is **load-bearing** the same
way R07 §3's duplications were: `record_ids` must distinguish "a revision id"
from "a content hash that happens to be reachable" — the cursor's own doc
says so (chain_walk.rs:126–130) — and `reachable` must answer quarantine and
insert-classification questions (orphan.rs:99, sqlite/rows.rs:100–102) that
don't care about record-ness. Two semantics, two sets; the record-id element
sitting in both is the price, not waste.

### 3.3 Corruption detection between walks — the S6-C4 timing guarantee

**Prevents cross-scope byte caches.** The pipeline's own corrective states the
guarantee in source (pipeline.rs:2058–2069): a corruption introduced between
two walks is detected by the second walk, because **every walk re-verifies
every object it examines**. A cache that serves previously-verified bytes
across coordination scopes suppresses exactly that: storage that has *changed*
under a live identity (the referenced-corruption refusal,
sqlite/rows.rs:99–105) would be masked by the cached good bytes — **fail
open**. Therefore (§4.3): the read memo is sound **only within one
coordination scope** — one lock hold, one transaction, one walk — where the
medium is immutable by construction. This is not a new rule; it is the S6-C4
memo's rule (walk-local, re-verify per walk) applied to the chain walker.

### 3.4 Deletion and exact-provenance authority

**Prevents the summary from ever feeding them.** The reclaim-execution
survivor set comes from fresh `walk_chain_excluding` runs
(reclaim_exec.rs:555, :780, :589, :781) — deletion decisions never consume the
session cursor, so no cursor-establishment shortcut can weaken them *by
construction*; the design pins this as an invariant test (§6.2). The
exact-provenance confirmation (R07 §3: the journal-side vs chain-side records
whose agreement *is* the proof) never reads the walker's cursor at all — the
pipeline re-derives the chain side itself (pipeline.rs:2044+). Both stay
untouched by everything in this design.

---

## 4. The bounded design

### 4.1 Streaming object validation (the L5 discipline at the `ChainReads` seam)

**The change:** `ChainReads` (chain_walk.rs:100–109) gains two narrow loading
operations alongside the existing three — still loading operations, still the
`BeginReads` narrowness, **not** a provider hierarchy; both adapters implement
them, no default logic leaks provider policy into the core:

```rust
/// Reads a bounded prefix of a content object's bytes (the domain
/// peek and the chunk-envelope header) — hash verification NOT
/// implied; prefix-only consumers must not treat this as verification.
fn read_object_prefix(&mut self, hash: &Hash, len: usize)
    -> Result<Vec<u8>, MediumError>;

/// Fully hash-verifies a content object WITHOUT materializing it:
/// streams the bytes in bounded chunks, returns the verified domain
/// peek (the prefix) — the whole-bytes form remains for the callers
/// that genuinely need the payload (the composite root).
fn verify_object_streaming(&mut self, hash: &Hash)
    -> Result<VerifiedPrefix, MediumError>;
```

- **Local adapter:** `File::open` + positioned chunked reads (~64 KiB,
  labeled) replacing `fs::read` (layout.rs:185) on the streaming path; the
  prefix op reads exactly `len` bytes.
- **Sqlite adapter:** `rusqlite`'s incremental blob I/O (`Blob` handle +
  chunked `read`) — **an existing dependency's existing API**, no new crate;
  if the pinned rusqlite version's blob behavior disappoints at landing time,
  the fallback is `substr`-windowed SELECTs, still no new dep (that
  verification is a landing obligation, §6, not an assumption).
- **The cold/suffix walks switch:** the content-refs loop
  (chain_walk.rs:285–293, :578–583) verifies via the streaming op, holding
  only the first reference's prefix; the composite closure
  (`verified_composite_closure`, :610–629) verifies each chunk by streaming
  hash + prefix-decoded envelope header + declared-length accounting, with
  the whole-bytes `verify_chunk_dependency` retained `cfg(test)`-gated as the
  oracle reference — the exact L5 pattern (`79b2fde`: the production path
  streams, the whole-read walk stays as the differential oracle).
- **The composite root stays whole** (one object): the manifest decode needs
  it, and the record's own reference to it is a legitimate whole read.
- **Effect (estimates):** the walk's byte-transient peak drops from
  ≈ 2·o_max (~10.5 MiB at the L5-max shape) to ≈ 64 KiB stream buffer + one
  record decode + (composite roots only) one root object; the same for the
  suffix walk and for every `validate_observed` rebuild. Repeated whole-file
  I/O for the domain peek becomes a prefix read.

**What it does not change:** every refusal rule, every check order, every
observable byte. The equivalence proof is the oracle (§6.2): the whole-read
reference and the streaming path compared over the full battery corpus —
states by equality, errors by full `Debug`+`Display` strings, seeded
regressions three-for-three (the L5 bar).

### 4.2 Borrow where the code clones (the zero-proof-cost fix)

- `validate_observed` (chain_walk.rs:694–713): `walk_suffix`'s signature takes
  `trusted_keys: BTreeSet<Vec<u8>>`, `trusted_record_ids: BTreeSet<Hash>`,
  `reclaimed: BTreeMap<Hash, u64>` **by value** (:497–499) while only reading
  them (:531, :562, :535–537). Change to `&` borrows; the three clones at
  :699–701 delete. **A crate-private signature change; no observable moves**
  — the battery's frozen observables (summaries, cursors, refusal strings)
  are untouched, and the census test pins the clone cost to zero (the teeth:
  an allocation-counting assertion on the divergent-head path, before/after).
- The orphan sweep (local/orphan.rs:89–93): iterate the directory and query
  `cursor.reachable.contains(&hash)` per entry under a shared borrow — the
  whole-set clone deletes. (The sweep's *semantics* — quarantine, never
  delete — are untouched and are load-bearing for §4.5.3's risk note.)
- `advance_cursor`'s `key.clone()` per append (chain_walk.rs:806–808) is
  O(k) ≈ 40 B — inherent (the key must live in the set), not targeted.

### 4.3 The within-walk bounded read memo (scope-local, byte-budgeted)

For the repeated-read case (§1.3: one object referenced by K records reads K
times): a memo of **hash-verified** object prefixes/verdicts keyed by
identity, **born and dying within a single walk** (the S6-C4 discipline,
§3.3). Policy: a byte budget (configurable, default small — the memo holds
streamed verdicts and small verified objects, never large payloads; large
objects get verdict memoization — "verified at identity X within this walk" —
which is O(1) per entry, not byte residency). The memo is a core-internal
`walk` local, never a field on the cursor, never visible across calls.
**Measurement-gated** (§6): whether real histories reference-share enough to
pay is an observable, not an assumption; the memo lands only if the census's
repeat-read row shows it does.

### 4.4 Suffix-entry slimming (optional, measurement-gated)

`VerifiedSuffixEntry` (chain_walk.rs:161–165) carries the full decoded
`RevisionRecord`; `absorb_suffix` consumes only `id`, the key,
`content_refs`, the closure, and `epoch` (:645–673), and the ancestry checks
consume `depends_on` only *during* validation (:529–543). A slimmed entry
(post-validation projection) roughly halves the per-suffix transient — worth
it only if the suffix-scale rows justify it; the battery's observables don't
see the struct either way.

### 4.5 The authenticated checkpoint/suffix summary — **DESIGNED, PARKED**

The one mechanism that attacks *both* remaining O(chain) terms — the cold
open's read work and the cursor's residency constant. It requires new durable
store contents, so per the standing rule this section designs it and
**Appendix A parks the decision**.

#### 4.5.1 The shape

A content-addressed object under a new domain
(`engine-persist/chain-prefix-summary/1`), written **by the compaction
publication** and referenced by the MK record as a second content reference
(the record wire format already carries a reference list; MK *recognition*
stays "first content object carries the checkpoint-metadata domain",
chain_walk.rs:298, :325 — the summary rides position ≥ 1, changing no
recognition rule). Fields:

- the anchor: the MK record's id and epoch (which chain position this
  describes), the founding id/authority/campaign_id (cross-checkable by one
  O(1) read of the founding record), and the accounted totals (`records`,
  `content_objects` — cross-checkable against epoch arithmetic);
- the retained-record array: sorted, deduplicated `(hash 32 B, epoch 8 B)`
  pairs — 40 B/record (estimate) — the prefix's `record_ids` + ancestry
  knowledge;
- the retained-content array: sorted `Hash` — 32 B/object (estimate) — the
  prefix's `reachable` content share;
- the retained-key array: sorted length-prefixed keys — (8 + k) B/key
  (estimate) — the prefix's `seen_keys`;
- the reclaimed index is **not duplicated**: it already lives in the
  `CheckpointMetadata` object (checkpoint.rs:123–129, :166) the walk already
  loads for the skip table; the summary and the metadata are read together.

#### 4.5.2 What authenticates it, and where it lives

It lives in the store like any content object (local: `objects/`; sqlite: the
`objects` table) under its own SHA-256 identity. The authentication chain:
(1) content addressing makes the summary's own bytes tamper-evident; (2) the
MK record *references it by hash*, and the MK record is itself a hash-verified
link on the chain reachable from HEAD through verified parents; (3) the
publication that writes it **already holds the full walk results**
(reclaim_exec.rs:555/:780 — `walk_chain_excluding` returns the complete
cursor), so the summary is a serialization of state the publisher derived by
the exact walk, at **zero additional reads**. There is no signature, no new
trust principal — SHA-256 object integrity, never signer authentication
(hash.rs:8–12).

#### 4.5.3 The trust boundary (the R07 "never trust a cache" discipline)

The summary is a **cache over the canonical chain, and the chain stays the
authority**. The boundary, drawn exactly:

- **It may short-circuit:** cursor *establishment* — cold open, reconcile,
  and the post-invalidation rebuild (chain_walk.rs:725–734) may load the
  prefix knowledge from a verified summary instead of re-walking the prefix,
  then walk and verify only head→MK (the suffix) and merge (the same merge
  `absorb_suffix` performs).
- **It may never feed:** (a) the reclaim-execution survivor walks — they keep
  their fresh full `walk_chain_excluding` (reclaim_exec.rs:555, :780), pinned
  by an invariant test that the deletion path never reads the summary;
  (b) the exact-provenance confirmation — untouched by construction (§3.4);
  (c) the public `validate()` — the explicit "verify everything now" arm
  keeps its full cold walk (local.rs:301–316) — the summary must not weaken
  the user-facing exact verification request.
- **Fail-closed rules (never accept, never merge):** any decode failure,
  unknown domain version, missing summary object, anchor mismatch (the MK
  record's id/epoch ≠ the summary's anchor), accounting mismatch
  (summary + walked-suffix totals ≠ the epoch/linkage arithmetic), unsorted
  or duplicate arrays, an epoch ≥ the MK's epoch in the retained array, a
  skip-table retained target absent from the retained array, or a
  configuration refusal → **the full O(chain) cold walk exactly as today**.
  The founding-record O(1) cross-check (read the founding record; its
  authority/campaign/epoch-0 must match the summary) runs on every
  summary-trusting open.
- **The sweep (orphan.rs) consumes the session cursor as today** — under a
  summary-built cursor its quarantine decisions inherit the summary's
  completeness. Quarantine is non-destructive and recoverable (the move
  never destroys evidence, orphan.rs:60–83), so the worst case is a
  recoverable misfiling, not data loss; the design records this as the
  summary's one consumer-side risk and requires the sweep to remain
  quarantine-only (an invariant test).

#### 4.5.4 The honest trust delta (why it is parked, not landed)

What the summary trades away: **per-open bit-rot re-verification of the
pre-MK prefix's object bytes.** Today every cold open re-hashes every
referenced object; under the summary, pre-MK object bytes are verified at
publication time and thereafter only tamper-evident *as stored content*
(content addressing detects a rewritten object only when something re-reads
it). The integrity of the *summary's claims* is chain-anchored; the freshness
of the *prefix's bytes* is not. Mitigations available at landing: `validate()`
as the explicit full arm (already exists), a periodic background
re-verification pass (work, not residency — record it as an option), and the
reclaim walks that re-verify ranges they touch. Whether this trade is
acceptable is **not a design question — it is the owner's** (Appendix A,
decision point 2).

#### 4.5.5 The residency half, and its honest ceiling

Loading the cursor from the summary permits a compact representation —
sorted arrays (40 B/record, 32 B/object, (8+k)/key — §4.5.1) instead of
BTree maps (160/80/88 — §2.1), roughly a **4× constant reduction
(estimate)** — with two implementation postures for the owner to choose
between (Appendix A, point 3): rebuild the ten-field BTree cursor from the
arrays (no §5.3.2 contract change, no battery-fingerprint change, work-win
only) or carry the arrays as the prefix representation (residency win too,
at the cost of a cursor-shape change the differential battery's fingerprint
comparison must be re-based for — field-value equality, never a weakened
comparison). **The ceiling: O(chain) knowledge residency is not removable**
(§3.2) — the summary attacks the cold-open work and the constant, never the
cardinality. Any claim beyond that would be false and is not made.

---

## 5. Cold open vs per-commit — where O(chain) is obligatory

| Path | Today | Obligatory? | After stage 2 (no format change) | After the summary (parked) |
|---|---|---|---|---|
| session open | O(chain) reads + whole-object transient peak | the exact historical verification: **yes, obligatory** absent a trusted summary — every record hash-verified, every object hashed, the cursor derived from the chain itself | same reads, **streamed** — peak ~64 KiB + root objects, no clone churn | head→MK walked and verified; prefix from the verified summary (fail-closed fallback to this column) |
| `head()` per commit, unchanged head | O(0) | — | unchanged | unchanged |
| `head()` per commit, external append | O(suffix) walk + **the three cursor-set clones** | the suffix verification: yes; the clones: no | O(suffix), zero clones | same |
| `head()`, sound invalidation | O(chain) rebuild | yes (fail-closed) | streamed rebuild | summary-assisted, same fail-closed arm |
| append | O(1) advance + O(1) membership | the membership checks need the O(chain) cursor knowledge — residency, not work | unchanged | unchanged (compact substrate, §4.5.5) |
| reconcile | **O(chain) cold walk per call** | the full-verification-before-classification rule (head_protocol.rs:396–406) — yes at the current trust model | streamed | summary-assisted |
| reclaim execution | fresh O(chain) `walk_chain_excluding` | **always, non-negotiable** — deletion authority | streamed, still fresh, still exact | **unchanged — never summary-fed** |
| public `validate()` | full cold walk | **always** — the explicit exact arm | streamed | **unchanged** |
| pipeline verifier walk (open) | second O(chain) record pass, `Vec<(Hash, RevisionRecord)>` (pipeline.rs:2048) | the exact-provenance chain-side derivation: yes; the separate *pass* is consolidable in principle (the two-source witness is journal-vs-chain, not medium-vs-pipeline) | out of this lane's scope — recorded §8 | out of scope |

**The fail-closed rules, restated as one line each:** corruption anywhere
walked → typed refusal, never acceptance (the frozen rules,
chain_walk.rs:12–26); divergent rewrite → refusal, never merge (:719–724);
unknown/failed summary → the full cold walk, never a partial trust
(§4.5.3); absent parent without a covering skip → hard corruption
(:345–364); an epoch above `u64::MAX` → refusal, never wrap (:263–275,
:552–559).

**Work vs residency, kept distinct:** stage 2 removes *transient bytes and
allocation churn* only; the cursor's resident knowledge is untouched. The
summary removes *prefix read work* and (at the owner's posture choice) the
residency constant; the knowledge cardinality remains in every variant.

---

## 6. The differential battery extension and the staged landing plan

The L9b battery (walk_differential.rs, `2ecadf5`/`59aec7e`) is **extended,
never weakened**: the frozen observables (summaries, cursors, refusal
strings; the `WalkStats` counters deliberately excluded) stand; the corner
test's strict byte-identity stands; every addition is additive. The
battery-first discipline repeats: each stage's discriminating cases exist and
are green **before** the production change they guard.

### 6.1 Commit 1 — the battery extension + the census (zero production change)

**The new oracle axes (the audit's named five, as provider-neutral corpus
cases):**

1. **Accepted/refused states** — the dispatch arms enumerated as a state set:
   each `(observed, trusted)` combination (chain_walk.rs:689–745) with its
   accepted cursor state or refusal, on both providers, byte-identical.
2. **Full error classification** — every rule in the frozen table
   (`divergence`, `epoch-regression`, `duplicate-idempotency-key`,
   `depends-on-unresolvable`, `cycle`, `kind-incoherent`,
   `founding-missing`, `checkpoint-index-incoherent`,
   `composite-closure-verification`, plus `MissingObject` and the typed
   format/referenced-corruption propagations) fired at least once per
   provider per walk form (cold, excluding, suffix), with the
   expected-rule assertions the battery already carries.
3. **Conflict ordering** — multi-violation records pinning *which* rule fires
   first, derived from the core's check sequence (cycle → decode → epoch
   linkage → duplicate key → content refs → MK/composite → parent/founding;
   suffix boundary check at `until`, ancestry deferred): e.g. a record that
   is both epoch-regressive and duplicate-keyed must refuse
   `epoch-regression`; a suffix that passes a bad key before a bad boundary
   must refuse the key. These order pins are new teeth — today the battery
   proves the rules fire, not their precedence.
4. **Stale cursor** — the external-writer family: absorbed append (cursor
   advanced, O(suffix)), divergent rewrite (refusal, cursor *kept* — the
   divergence arm does not discard, :719–724), vanished head (the
   `(None, Some)` divergence), corrupted suffix (sound invalidation: cursor
   discarded, cold rebuild re-derives the refusal), and **repeated cursor
   advancement** — N successive external appends each validated, the
   O(1)/O(suffix) accounting asserted (the providers' existing
   walk-stats pins extended to the sequence).
5. **Provider failure** — fault injection at read points (the local
   `FaultPoint` seam, the sqlite fault seam): both providers must refuse,
   typed, fail-closed — **never accept**. Honest comparison rule: the
   seam-independent classifications (`Chain { rule }`, `MissingObject`,
   `ReferencedObjectCorrupt`) compare byte-identical as the battery always
   has; the seam-specific transport variants (`Io` vs `Sqlite`) compare as
   *refused-with-typed-error on both*, because their `Debug` strings are
   legitimately provider-shaped (the battery's existing normalization
   discipline, stated for this axis rather than assumed away).

**The census (the R07 §7 shape):** a read-only estimator over the walked
chain (`walk_census`: cursor-resident, walk-transient, per-validation
formulas of §2 as documented fields — never serialized, never a fingerprint
input) + structural census tests: linearity in N/C/K/R at fixed shapes; the
2·o_max object peak at the large-object shape; **the per-validation clone
cost as a row** — the teeth for commit 2 (it must pin to zero after).
Census scales: N ∈ {512, 2048, 8192} (the R07 ladder), both providers, the
small-object and the L5-max-object shapes.

**Gates:** the persist battery green in **all three configurations** —
default (sqlite on), `--no-default-features` (local-only),
`--features composite-test-fixture` — both providers exercised in the
battery; fmt clean; clippy `-D warnings` clean; the diff touches only test
and census surfaces (the zero-production-change assertion, reviewed at the
diff level); count movements pinned with cause comments.

### 6.2 Commit 2 — the bounded mechanics (no durable format change)

The §4.1 streaming seam ops + both adapters; the walks rewired to them; the
§4.2 borrow fixes; the §4.3 memo and §4.4 slimming **only if the commit-1
census rows justify them** (measurement-gated, each independently
revertible). The whole-bytes `verify_chunk_dependency` and the whole-read
object path retained `cfg(test)`-gated as the oracle reference.

**The equivalence proof (the L5 bar):** the streaming path asserted equal to
the whole-read reference over the full battery corpus — states by equality,
errors by full `Debug`+`Display` strings; the corpus gains the
streaming-discriminating shapes (a corrupt byte in the *middle* of a large
object — the refusal must be identical to the whole-read form; a corrupt
byte in the domain prefix; an object whose declared length disagrees with
its stored size; truncations at every stream-chunk boundary — the L5
traversal pattern applied to object reads); **three seeded regressions run
then reverted, none committed** (e.g. a stream early-exit that treats
short-read-as-EOF, a prefix cache that serves stale bytes across refs, a
hash computed over the prefix only).

**Gates:** the commit-1 battery green **unchanged** (the frozen observables
byte-identical — this is the equivalence proof for the walks themselves);
the streaming oracle green both providers; the three configurations; the
census rows re-measured — the object peak row at the L5-max shape must show
~64 KiB + root terms (the measured obligation), the clone row zero; a
cold-open wall-time row at the census scales, both providers, before/after
(serial runs, the T39 methodology discipline); allocation pins where the
repo already carries them; no test weakened; count movements pinned.

### 6.3 Commit 3 — the authenticated prefix summary: **PARKED (owner-gated)**

Not scheduled. If the owner GO arrives (Appendix A), the landing order is
pre-designed: (1) the publication writes the summary (zero extra reads,
§4.5.2) with the reader still ignoring it; (2) the reader's fail-closed
summary path + the full fallback, behind the owner-chosen posture
(§4.5.5); (3) the battery's summary-vs-full-walk oracle — the whole corpus
through both paths (states equal, errors equal), plus the tamper family
(bit-flipped summary object, stale summary against a rewritten chain, a
foreign MK's summary, an unknown domain version, an anchor mismatch — each
must fall back or refuse, never accept) and the two never-feed invariant
tests (reclaim execution and `validate()` do not read the summary); (4) the
census re-measure (cold-open work, residency per posture). Each step's bar
is named now so the landing lane cannot weaken one silently.

### 6.4 The measurement obligations (consolidated)

- the census rows (§6.1) — actual B/record vs the §2 estimates, debug and
  release, both providers;
- the object-peak and clone rows before/after commit 2 (the two measured
  claims of §4.1/§4.2);
- cold-open wall time at N ∈ {512, 2048, 8192}, both providers, before and
  after streaming (serial, T39 discipline);
- the repeat-read rate on realistic histories (engine-slice sustained-load
  fixtures) — gates the §4.3 memo;
- the suffix-scale distribution at real workloads — gates the §4.4 slimming;
- rusqlite blob-read behavior on the pinned version (or the `substr`
  fallback decision) — a landing-time verification, not an assumption;
- if the summary is ever approved: the cold-open work row head→MK vs full,
  and the residency row per posture.

---

## 7. Honest limits

1. **Every number in §2 is a labeled estimate.** The node share (48 B), the
   Vec slack, k = 16 B are conventions; std's B-tree occupancy, the
   debug/release gap, and allocator behavior under churn are the landing's
   census rows, not this document's claims. The structural claims
   (linearity, the 2·o_max peak, the clones' existence and linearity) are
   source-derived and testable — commit 1 pins them.
2. **The shipped-scale magnitude is modest.** At the R07-measured session
   scales (hundreds to low thousands of records), the cursor is tens to
   hundreds of KiB and the transient peak is the object-buffer term — the
   stage-2 fixes are landed because they are *free* (no proof traded, no
   format touched), not because the corner is on fire. The asymptotic fix is
   the summary, and it is parked.
3. **The summary's trust delta is real and not design-erasable** (§4.5.4).
   Cross-checks bound it; the fail-closed fallback contains it; only the
   owner can accept it. This document does not decide it (Appendix A).
4. **O(chain) session knowledge residency is the ceiling in every variant**
   (§3.2, §4.5.5). No design in this document claims otherwise, and any
   future one that does should be treated with the R07 §3 skepticism.
5. **The pipeline's second open pass is observed, not designed here.**
   Fusing the medium's open walk with the verifier's record re-read
   (pipeline.rs:2044+) would not vacate the exact-provenance witness (the
   witness is journal-vs-chain), but the lock-per-record cost and the
   consumer census it needs are their own investigation — T30/R07-adjacent,
   recorded for that lane, not smuggled into this one.
6. **The battery's documented corpus limits are inherited, not solved:**
   the hash-fixed-point unconstructibles (a literal parent cycle, a literal
   self-dependency) remain unconstructible; the conflict-ordering pins
   (§6.1.3) cover the *reachable* orderings only.
7. **The provider-failure axis compares classifications, not transport
   strings** (§6.1.5) — an honest narrowing, documented in the battery
   rather than papered over with a fake normalization.

---

## Appendix A — the parked decision record (shape, not decided)

**DECISION-T29-prefix-summary** — the owner decision points, enumerated for
the GO. This appendix *is* the decision-record shape the standing rule
requires; nothing in it is decided.

1. **Durable-store contents.** May compaction publication write a
   content-addressed object under a new domain
   (`engine-persist/chain-prefix-summary/1`), referenced by the MK record's
   content references? (The revision wire format is unchanged — the
   reference list already exists; MK recognition rules are unchanged. What
   changes is what publication writes and what the store contains. Old
   readers ignore the extra reference as an ordinary content object; new
   readers fail closed without it.)
2. **The trust trade.** Accept publication-time verification + chain-anchored
   tamper-evidence + the fail-closed full-walk fallback (and `validate()` as
   the always-exact arm) **in place of** per-open re-hashing of pre-MK object
   bytes at cold open and reconcile? (§4.5.4 states the delta and the
   mitigations, including the optional background re-verification pass.)
3. **The cursor posture.** Rebuild the ten-field BTree cursor from the
   summary (no §5.3.2 change, work-win only) or carry the compact arrays as
   the prefix representation (residency win, cursor-shape change, the
   battery's cursor fingerprint re-based on field-value equality — never a
   weakened comparison)?
4. **Activation policy.** Always-on when a verifiable summary exists, or
   opt-in configuration with the full walk as the default?
5. **The warrant threshold.** At what measured cold-open latency or residency
   row (§6.4) does the summary become warranted at all? The census is the
   observable; the number is the owner's, per the R07 `[ASSUMPTION]`
   convention.

**Owner:** the persist area, per the D-EE-10 decision framework. **No lane
dispatches on this half without the GO** — the stage-1/2 landings of §6
require no part of it.
