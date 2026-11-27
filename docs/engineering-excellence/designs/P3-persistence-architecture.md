# P3 — Persistence and Recovery Architecture (Design)

> **Status:** COMPLETE (all sections drafted; honest coverage limits in §6).
> **Author:** P3 design author (glm-5.3-w4a8), read-only for `crates/**`.
> **Base:** branch `refactor/engineering-excellence`, HEAD
> `bceb3faed42a4124bc615916b4a352cdf2d1979a` (verified `git rev-parse HEAD`,
> 2026-11-26 22:41:24 -0500, "integrate p1-b05 …").
> **Scope per DIRECTIVE §7 P3:** A13, A22 (persist half), B02, B03, B04
> (persist half — slice side already fixed, see FINDINGS), B12, B13,
> NEW-PERSIST-04/05. A01 persist half (module split) is folded into A13's
> module plan. Non-goals listed in §5.

## 0. Method and evidence discipline

Every load-bearing claim below is verified against source at HEAD
`bceb3fa` with exact `file:line` citations. Large files (`journal.rs`,
`local.rs`, `reclaim.rs`) are navigated by grep + bounded reads, not
whole-file reads. Where the audit's line refs disagreed with HEAD, the
source read governs (FINDINGS.md verdict-provenance rule).

Sections:
1. Invariant and ownership map (current seams, target seams).
2. Per-finding design sections (B02, B03, B13, A13, A22-persist, plus
   B04/B12 coverage notes).
3. Staged implementation plan (lanes, risk class, count pins).
4. Measurement plan (charter §8.2 persistence row).
5. Non-goals.
6. Honest coverage limits and unresolved questions.

---

## 1. Invariant and ownership map

### 1.1 The invariants (what any P3 change must preserve)

Verified against source at HEAD `bceb3fa`; every claim carries its citation.

| Invariant | Owner today | Source |
|---|---|---|
| **Canonical medium** — one CAS chain per campaign, append-only, parent-linked, epoch-progressing, idempotency-key-unique; `Accepted` only after the provider's full documented sync sequence | `LocalFileMedium` (9-step append, HEAD.tmp+fsync+rename), `SqliteMedium` (BEGIN IMMEDIATE + COMMIT = linearization) | local.rs:1-14 mod doc, :660-687 `publish_head`; sqlite.rs:1-9 mod doc |
| **Commit verification** — a bare `Accepted` is never confirmation; the read-back walk + per-key exact provenance (coverage, producing transaction, effect commitment) decide | `CommitVerifier` | pipeline.rs:1377-1439 `confirm` |
| **Journal (T1)** — append-only hash-chained frames; torn-tail truncation never touches committed history; committed-history corruption fails closed; exactly one fsync per committed transaction; duplicate transaction id is idempotent no-op | `T1Journal` + free functions | journal.rs:1-113 mod doc, :1244 `frame_end`, :1252-1316 `classify_from`, :2465-2474 `resolve_tail` |
| **Publication authority** — the sidecar lock file (never on the data file), `PublicationPermit`, single-writer discipline | `T1Journal` + `hold_publication_lock` | journal.rs:1602-1613 (lock placement rationale), :2522-2575 |
| **Checkpoints / ancestry / reclaim** — verify-before-delete, MK skip tables, idempotent restartable phases, complete-manifest completion check; the journal generations' ownership is the rotation lineage, never a filename pattern | `checkpoint.rs`, `ancestry_*`, `reclaim.rs`, `rotation.rs` | reclaim.rs mod doc; rotation.rs mod doc; crossing.rs:4590-4619 `reclaimable_journal_files` |
| **Authority ordering (rotation)** — pointer flip is the linearization; failed/unresolved rotation fails every mutation closed (R5 repair); worker poison fails everything closed | `Crossing`, `JournalAuthority` | crossing.rs:1241-1286 `assert_journal_authority` |
| **Recovery** — deterministic function of (anchor, journal, confirmed chain); exactly-once by identity; fail-closed on divergence/corruption; reconstructed state must pass the strict save/restore contract before progression | `recovery.rs` (pure), crossing recovery driver | recovery.rs mod doc; crossing.rs:3395-3460 |
| **Orchestration** — maintenance under exclusive lease with FIFO barrier; optional publications nonblocking (try_send), required ops blocking send; never acknowledge before durable | `SessionOrchestrator`, `Crossing` | orchestration.rs:765-772, :837-886, :1200-1212 maintenance doc |
| **Worker transport** — single FIFO worker, bounded queue (capacity 64, provisional), health/poison admission gate; reply loss after append admission = `Unknown{WorkerReplyLost}` never definite failure | `WorkerMedium`, `IoWorker` | worker.rs:44-46, :621-650 `request`, :704-712 |
| **Provider distinctions** — local (process-crash tier on Windows) vs SQLite (`TransactionalWalFull`, contention ≠ stale-parent); migration tombstones; no generic provider hierarchy beyond `CanonicalMedium`/`BeginReads` | medium.rs trait + two providers | medium.rs:60-79 `UnknownReason`, :120-208 `MediumError` |

### 1.2 Module ownership today (engine-persist, 34 modules, ~46.4k prod lines per E-5)

| Module (lines) | Owns | Change reasons mixed today |
|---|---|---|
| `local.rs` (12,993) | LocalFileMedium: layout, OS lock, HEAD protocol, trusted-cursor walks, orphan protocol, reclaim execution, timings | **A01/A13: 6+ reasons in one file** — pure walks interleaved with lock/IO (validate_observed :714, validate :1656) |
| `sqlite.rs` (8,274) | SqliteMedium: rows, transactions, walks, backup (VACUUM INTO) | same walk/provider mixing as local |
| `recovery_classification.rs` (7,771), `ancestry_witness.rs` (7,238), `reclaim.rs` (6,939), `ancestry_accounting.rs` (5,688), `ancestry_history.rs` (2,030) | V2X-CP3 retained-evidence classifiers, ancestry codecs/walkers, reclamation ledger + plan + destructive phases | reclaim mixes codec + discovery + publication + execution (A01) |
| `pipeline.rs` (3,636) | CommitAuthority (journal half) + CommitVerifier (medium half) + facade | **already split by ownership** (:659-707); residual: clone amplification (B03), O(history) scans (A22) |
| `journal.rs` (3,575) | T1 frame format, classification, lock, bound journals | whole-file read (B02); format + IO + scan in one file but cohesive |
| `range_installation.rs` (2,903), `range_completion.rs` (2,672), `batch_completion.rs` (2,321) | V2 `/2`,`/3` marker codecs + read-only observation verifiers | pure, well-bounded (owner-ratified C1/C3/C5a scopes) |
| `recovery.rs` (2,321), `recovery_discovery.rs` (1,979) | recovery pure function; no-skip observation walk | already pure/reader-generic |
| `composite_reader.rs` (549), `composite_manifest.rs` (628), `composite_append.rs` (367) | the DARK `/2` composite read/codec/admission | dark by construction (mod docs); B03 representation question |
| `medium.rs` (912), `record.rs` (521), `envelope.rs` (1,440), `consequence.rs` (1,244), `hash.rs` (137), `checkpoint.rs` (787), `rotation.rs` (886), `encounter.rs` (1,016), `session_anchor.rs` (413), `migrate.rs` (604), `fault.rs` (140) | contracts, codecs, rotation, encounter machinery, anchor decode, offline migration, fault seams | session_anchor bakes core demo constants (REGION_COUNT/NPC_COUNT — the B12/P2 seam, §2.7) |

### 1.3 Where the seams blur (measured, with the target seam)

1. **Journal reads are whole-file** (`read_journal_bytes`, journal.rs:170-189, ≤4 GiB single Vec) even where only the fixed ~100-byte bound header is consulted (a length-framed 36-byte domain + schema byte + two 32-byte hashes, journal.rs:1030-1042; reclaim.rs:2449, :2628-2633; crossing.rs:1010-1016) — and the session-snapshot path re-reads/re-decodes the entire journal although the pipeline holds the same decoded state (orchestration.rs:808 vs pipeline.rs:1689 `journal()`). **Target seam:** a bounded-read journal surface — header-only prefix read, in-memory frontier derivation, streaming frame classification for full scans.
2. **Recovery double-decodes** — crossing.rs:3192-3238 runs `scan_bound_file`/`scan_file` (full decode #1, `JournalScan`) and then `T1Journal::open_bound`/`open` (full decode #2, `JournalState`), both alive simultaneously at :3239. **Target seam:** one decode per file per process boundary, the second view derived from the first.
3. **Copy amplification in commit/reclaim** — pipeline.rs:1213 `keys.clone()`, :1334 per-key `coverage.clone()`, :1373 `attempt.clone()` (three key-set copies per commit); reclaim.rs:3452-3454 reads a record and discards it (`let _ = &head_record;`) then re-reads at :3467; :3483 `bytes.clone()` for a domain tag readable from a borrow; :3476/:3498 linear `objects.contains`. **Target seam:** one owned copy where durability demands retention (the pending attempt), borrows/Arc elsewhere; hash-set membership.
4. **Pure walks interleaved with provider I/O** (A13) — local.rs `validate_observed` :714 / `validate` :1656 do lock()…walk…unlock() in one impl; sqlite.rs mirrors it. The V2X-CP2 `BeginReads` reader-generic pattern (reclaim.rs:675-705) already exists as the proven extraction shape. **Target seam:** walks behind `BeginReads` shared by both providers; provider files keep only layout/lock/IO.
5. **Unbounded in-RAM consequence tracking** (A22-persist) — `CommitAuthority.keys` (pipeline.rs:664, seeded :719-727, grows via `stage` :750, only InMemory-phase removals :773) and `CommitVerifier.confirmed` (:677, rebuilt at open, never pruned); `collect_actionable` scans ALL journal transactions per commit (:895-934). **Target seam:** settled-transaction watermarks + detail shedding at the rotation/reclaim compaction points.
6. **Presentation-thread blocking** (B13) — every `WorkerMedium` method is send-then-`slot.recv()` on the caller's thread (worker.rs:646, :698, :756, :803, :848, :888, :932); `commit_canonical` additionally fsyncs the journal on the caller thread (orchestration.rs:762-764 doc); `capture_snapshot` reads the whole journal directly (orchestration.rs:808) plus a worker head round-trip; the interactive demo runs all of this on the render thread (persist_demo.rs:2598-2612 renderer + :1421 commit in the same loop). **Target seam:** the T1 capture stays synchronous on the authority thread (durability the caller waits for); the canonical confirmation becomes deferrable/pollable exactly like `submit_publication`; snapshot capture loses its I/O.

The audit's verdict labels (persist.md) were reconciled per FINDINGS.md: located facts (line numbers, census) verified against HEAD where cited above; two line-ref sets were stale at HEAD (`confirmed_history` no longer exists — see §2.5; `read_journal_bytes` caller list now enumerated exactly in §2.1).

---

## 2. Per-finding design sections

### 2.1 B02 — journal working set: whole-file reads, caller trace, duplicate decodes

**State:** CONFIRMED (FINDINGS). **Charter text:** DIRECTIVE §6 B02.

#### 2.1.1 Current shape (verified at HEAD)

`read_journal_bytes` (journal.rs:170-189) opens the file, enforces
`MAX_JOURNAL_FILE_BYTES = 1 << 32` (4 GiB, journal.rs:153), reserves the full
length (`try_reserve_exact`, :177-179) and reads the whole file into one
`Vec<u8>` via `take(MAX_JOURNAL_FILE_BYTES).read_to_end` (:180-183), plus the
grow-during-scan ceiling check (:184-187). The comment at :150-152 honestly
documents "the scan reads it once per open/recovery — bounded, never in a hot
path". **Complete production caller census at HEAD** (grep-verified; the
audit's list ":1115…3428" reconciled — :3409/:3428 are `#[cfg(test)]`):

*Inside engine-persist (journal.rs):*
- `scan_bound_file` :1109-1115 → full read + `classify_bound` (all frames decoded).
- `T1Journal::open_with` :1587-1615 (production `open` :1581) → full read + classify.
- `initialize_bound_at` :1884-1892 (via `open_bound`/`open_bound_creating`) → full read.
- `T1Journal::scan_file` :1988-1990 → full read + `classify_from`.
- `refresh_from_disk` :2407-2408 (the ambiguous-write reread from `reconcile_write` :2264) → full read + strict classify.

*Inside engine-persist (reclaim.rs):*
- :2447-2454 bootstrap-evidence check — reads the **whole file** only to run `read_bound_header` (a fixed ~100-byte prefix comparison: the length-framed 36-byte domain + schema + two 32-byte hashes, journal.rs:1030-1093). Result discarded beyond the header match.
- :2610-2618 `scan_bound_file` twice for identity — full read + **full decode**, results discarded (`?` then `Ok(Vec::new())`/continue).
- :2628 stale-generation identity — whole file per generation, header-only use.

*In engine-slice:*
- `backup.rs` :345 `scan_last_committed` (via `T1Journal::scan_file`) — full decode to obtain **only the last committed transaction id**; :448 and :575 read the whole **captured copy** to hash it (legitimately whole-file — the manifest binds the journal's hash).
- `crossing.rs` :808, :950 `scan_bound_file` identity checks (results discarded); :963 `open_bound`; :1010-1016 residue header check (**whole file for a prefix**); :1025 residue classification (needs the empty-history verdict — committed/cancellations/tail, a full-scan need); :2380-2424 rotation re-open; :3192-3238 the recovery double read (below); :4608 active-must-scan-green before stale-generation removal (full scan, result discarded).
- `orchestration.rs` :627 session `open`; **:808 `capture_snapshot`** — full read + full decode per snapshot capture, only to collect `journal_frontier` (the set of committed transaction ids, :809-813).
- `persist_demo.rs` :1117 campaign open.

**Peak memory and duplicate decoded objects (the charter's questions):**

1. Per whole-file call, peak = file bytes (one Vec, exact-reserved) **plus** the decoded `JournalState` (every `CommittedTransaction` with full envelopes, sub-spans, and plan bytes — plans are bounded at 1 MiB each, journal.rs:148; envelopes at 65,536 per transaction, :143). Decoded size is a multiple of file size for envelope-heavy journals.
2. **Recovery double-decode:** crossing.rs:3192-3238 classifies the active journal into a `JournalScan` (`committed`, `cancellations`, used at :3221) and *then* opens `T1Journal::open_bound`/`open` (:3232-3237) which classifies the same bytes again into a second, richer `JournalState` — both fully decoded sets are live simultaneously when `Self::open_at(source, journal, campaign)` runs (:3239). Two full file reads (the second re-reads from the OS cache but re-allocates and re-decodes), two decoded object graphs.
3. **Snapshot-path duplicate:** `capture_snapshot` (orchestration.rs:780-825) re-reads and re-decodes the entire active journal although the crossing's pipeline already owns the decoded `T1Journal` (`pipeline.journal().transactions()`, pipeline.rs:1689) — the frontier it computes (:809-813: `transaction.id` for every committed transaction) is exactly `pipeline.journal().transactions().iter().map(|t| t.id)` modulo the pointer-vs-handle window, which the existing capture-validity generation check (:823 `generation`, refused at :844-846 when a flip intervenes) already guards.
4. `benchmark.rs` (fixture harness) opens journals repeatedly; not production.

The audit's "recovery paths (e.g., `recover_memory_lane`)" claim refines to:
`recover_memory_lane` itself (npc_memory_lane.rs:1420-1519) iterates
`pipeline.journal().transactions()` — no extra file read; the cost lands in the
pipeline **open** that precedes it.

#### 2.1.2 Options considered

**Option A — caller-side fixes only (no reader change).** Add a bounded
prefix read for header-only uses; derive the snapshot frontier and the recovery
scan from already-decoded state. Leaves `open`/`scan_file` whole-file.

- *Pros:* smallest diff; no reader-internals risk; immediately removes the
  per-snapshot full re-read and the header-only whole reads.
- *Cons:* open/recovery peak stays O(file + decoded); the 4 GiB ceiling still
  admits an enormous single allocation on constrained machines.

**Option B — incremental framed streaming reader.** Replace the
whole-file `Vec` in `open_with`/`scan_file`/`scan_bound_file`/`refresh_from_disk`
with a `BufReader`-backed frame iterator: read the 8-byte length prefix
incrementally, `frame_end` (journal.rs:1244-1250) already validates the
declared length against *remaining input* — re-expressed as
"declared length vs. actual bytes until EOF" it preserves the exact torn-tail
classification; each record's bytes land in a bounded per-frame buffer; the
hash chain (`decode_record` :879, `expected_prev` :1259, :1315) is sequential
and unchanged. Peak becomes O(decoded state) + O(max frame) instead of
O(file) + O(decoded).

- *Pros:* bounds peak memory by the *semantic* content, not the file size;
  `classify_from`'s walk (:1268-1314) is already a sequential cursor over
  `bytes` — the transformation is mechanical; torn-tail, zero-fill, chain
  mismatch, and undecodable-damage handling (:1270-1313) are all local to the
  frame boundary and survive verbatim.
- *Cons:* the classification must now distinguish "torn frame" (EOF mid-frame)
  from "declared length exceeds file" — today both come from
  `frame_end` returning None; a streaming reader must keep both mapping to
  `TailKind::TornFrame` exactly. `read_journal_bytes` stays for the
  whole-file-hash uses (backup.rs:448/575) — the API surface grows by one
  reader type.

**Option C — verified on-disk index / checkpoint for O(1) open.** A
sidecar (sequence→offset, transaction-id→offset) hash-chained to the journal,
or a checkpoint header, so `open` verifies the index tip instead of walking
frames.

- *Pros:* O(index) open regardless of history length.
- *Cons:* **new durable format surface and a new trust question** (the index
  must be authenticated against the chain it indexes — the journal's design
  deliberately derives everything from the single append-only file). The
  program already owns the bounded-history mechanism: PP-B3 **rotation**
  (rotation.rs mod doc: surviving transactions re-anchored into a fresh
  journal, fully scan-verified before the flip) plus reclamation of stale
  generations — a journal that would grow to gigabytes is a *cadence* failure
  (rotation not invoked), not a missing format feature. Adding a second
  authenticated structure duplicates rotation's job at higher risk.

**Recommendation: A then B; C rejected.** Stage the caller-side wins first
(header-only reads; frontier from memory; recovery single-decode) — they are
independently valuable and zero-format-risk — then land the streaming reader
with a differential oracle (§2.1.4). C is rejected on the trust-surface and
redundancy-with-rotation grounds; the 4 GiB cap remains a fail-closed ceiling
(journal.rs:150-153), never presented as a working-set budget, and the
*policy* answer to growth is the existing rotation/reclaim cadence made
explicit (§2.5).

**Migration/compatibility:** no byte-format change. `journal/1`,
`bound-journal/1`, frame domains, and every refusal string stay identical;
`read_journal_bytes` remains public (backup hashing). The streaming reader
must produce byte-for-byte identical `JournalState`/`JournalScan` and
identical `JournalError` values on identical inputs — enforced by the
differential test, not by review.

#### 2.1.3 What each caller keeps (no semantics lost)

- Header-only callers (reclaim :2449/:2610/:2618/:2628, crossing :1010):
  `read_bound_header`'s torn-prefix / mismatch / empty-file classification
  (journal.rs:1069-1092) is a pure prefix function — a bounded
  `read_prefix(path, bound_header_len)` must return the *same*
  `BoundHeaderState`/errors, including the existing-empty-file `Fresh` case
  and the shorter-than-header `TornCreation` prefix rule.
- `capture_snapshot`: frontier becomes `pipeline.journal().transactions()`
  ids + the flip generation; the pointer-vs-handle window is closed by the
  generation check already at submit time (orchestration.rs:844-846). The
  active-journal-pointer resolution (:807) is kept for the *missing pointer
  fails closed* rule — the refusal must still fire when the pointer names a
  missing journal, which the in-memory derivation cannot see; so the path
  resolution stays, the file read goes.
- Recovery (crossing :3192-3238): one classification, both views derived from
  it — `JournalScan` (committed/cancellations/tail) is a subset of
  `JournalState`; either scan-then-open-from-scan (add a constructor that
  adopts a verified state) or open-then-derive-scan. The bound-identity
  branches, the `NotFound && !pointer_present` empty-scan fallback (:3209-3217),
  and every `TransitionError` detail string (:3195-3218) must be preserved
  exactly.

#### 2.1.4 Test / evidence plan

- **Must hold unchanged:** b3_matrix.rs / b3_process.rs / journal_process.rs
  (torn-tail, zero-fill, chain-mismatch, torn-creation, grow-during-scan
  ceilings); v2w_f4_bound_lifecycle.rs (bound open/refusal matrices);
  pp_b3_rotation.rs (the fully-scan-verified flip); b4_process.rs/b5_lifecycle
  (slice recovery); the S6 fuzz gate (npc_memory_lane.rs:2163 — values
  unchanged).
- **New discriminating tests:**
  1. Differential oracle: generate adversarial journals (valid, torn at every
     frame boundary, zero-filled, chain-broken mid-file, header-torn,
     exactly-at-4-GiB-cap mocked via the length check) and assert
     streaming-classifier output == current-whole-file output (state +
     errors). The old classifier stays in-tree as the test oracle until the
     differential is green over the corpus, then is removed.
  2. Header-prefix reader: same corpus, prefix path vs `read_bound_header`
     over full bytes.
  3. Recovery single-decode: existing recovery matrices re-run (they are the
     real evidence) + one test asserting the recovered crossing's
     `transactions()` equals the pre-change scan (no behavioral drift).
  4. Snapshot frontier: a rotation-between-capture-and-submit test already
     exists in spirit (generation refusal); add one asserting the frontier
     from memory equals `scan_file`'s on the same file.
- **Measurement (§4):** cold/warm open and recovery at journal growth scales
  (1k / 10k / 100k transactions, realistic envelope mix), reporting peak
  working set and bytes read per open — before/after on the same fixture
  generator, disposable saves only.

### 2.2 B03 — copy amplification: composite reader memo, reclaim unit collection, pipeline attempts

**State:** CONFIRMED (folds NEW-PERSIST-01/04). **Charter text:** §6 B03.

#### 2.2.1 Current shape (verified at HEAD)

**(a) Composite reader** (composite_reader.rs:416-579
`read_composite_closure_with_memo`):
- Fresh path: `source.read_object` returns owned `Vec` (:467-477); the payload
  is decoded as a borrow; `payload.to_vec()` (:536) materializes an owned
  copy; the memo insert **clones it again** (:544 `payload: payload.clone()`);
  `chunks.push(VerifiedChunk { payload })` (:568-572) moves the third
  reference. So a fresh chunk = read bytes (1 alloc) + `to_vec` (1) + memo
  clone (1) = the payload exists **3×**; a memo hit adds `payload.clone()`
  (:463) = **2× live** per reuse, plus `tag.to_owned()`.
- `VerifiedComposite::composed_payloads` (:263) concatenates another full Vec.
- **Scope honesty:** the module is DARK by construction (mod doc :1-9: "no
  production read, write, or reclamation path depends on this module"); the
  `/2` root writer (`session_chunks::try_root_manifest`) is test-only at HEAD
  (grep: callers only in tests). Production snapshot publication is the
  legacy single-object `session-snapshot/1` (crossing.rs:1877-1909
  `publish_snapshot`). The one engine-persist production reader that can
  expand `/2` roots is `dark_reachability::live_set` (dark_reachability.rs:351-375),
  which only encounters `/2` roots if some future writer publishes them.

**(b) Reclaim unit collection** (reclaim.rs:3448-3505):
- `collect_unit_objects` (:3452-3454) reads the record **and discards it**
  (`let _ = &head_record;`) before `collect_unit_objects_via` re-reads it
  through the reader (:3467) — one redundant hash-verified record read + decode
  per unit, per call site (:3259, :3400, :3857, :4115).
- :3483 `StoredObject::new(bytes.clone())` clones the **entire object** to
  read a domain tag; `StoredObject::new` itself only decodes the length-
  prefixed tag (medium.rs:308-315) — a borrow-based domain peek (`Decoder::
  read_str` over `&bytes`) yields the identical tag/error with zero copy.
- `objects: Vec<Hash>` with `objects.contains` linear membership (:3476,
  :3498) — duplicate-within-unit refuses (the ratified set rule, :3444-3447),
  across-unit sharing is legitimate, so the vector can reach
  `MAX_HASH_LIST_LEN = 65,536` entries (record.rs:29) per unit: worst case
  ~2.1e9 comparisons.
- Every `reads.read_object_bytes(reference)` (:3475) is a full hash-verified
  object read; through `WorkerMedium` each is also a worker round-trip
  (worker.rs:951-957) — the read amplification is queue round-trips too.

**(c) Pipeline attempt clones** (pipeline.rs):
- Per `commit_canonical` with a nonempty batch: `collect_actionable` clones
  every actionable key's `sub_spans` into `keys` (:927-930); `submit_keys`
  passes `keys.clone()` into `append_for` (:1213) because the stale-parent
  path needs the set again; `append_for` clones each coverage again into
  `BatchEntry`s (:1334) and `ticks.clone()` (:1368); the attempt is cloned
  into the pending slot (:1373 `self.pending = Some(attempt.clone())`). Net:
  **three key-set copies** (collect→submit, submit→append_for's BatchEntries
  coverage, attempt→pending) plus the retained local `attempt` consumed by
  `confirm` (:1383).
- `reconcile`/`retry` clone the pending attempt (:1091, :1128) so the slot
  survives failure — the retention discipline ("write-at-uncertainty-or-
  acceptance", :1370-1373; "the ORIGINAL candidate stays in the slot through
  every pre-submission fallible step", :1117-1126) is *correct*; the clone
  count is the only issue.
- `transaction_ticks` (:941-957) does a linear `.find` over
  `journal.transactions()` per key although `JournalState.committed_index`
  (journal.rs:1147, used by `is_committed` :2310-2312) already maps
  `TransactionId → index` — an A16-class residual (O(keys × transactions)).

#### 2.2.2 Options considered

**For (c) pipeline clones:**
- **Option 1 — restructure to single retention copy.** `append_for` moves
  `keys` into the returned attempt (no `keys.clone()` at :1213 — the
  stale-parent path re-derives `remaining` from the *pending* attempt it
  already retains, which `retry` demonstrates is sufficient, :1132-1133);
  `confirm` runs on `&PendingAttempt` (borrow) with the slot cleared only on
  success — `confirm_retained` (:1285-1313) already encodes exactly this
  clearing rule. `BatchEntry.coverage` stays owned (canonical encoding
  consumes it, and the batch object must exist independently). Expected: 3 →
  1 key-set copies + owned batch entries.
- **Option 2 — `Arc` the per-key evidence.** `AttemptedKey = (Arc<[ClassifiedSpan]>, TransactionId, Hash)`; pending, request, and entries share. 
  *Cons:* the `BatchEntry` needs the spans for canonical bytes; sharing an Arc
  into the batch encoding is fine, but `ConsequenceBatch` is a *wire* type
  whose equality/decode produce owned Vecs — Arc would fork the type into
  owned/Arc variants or convert at the boundary; more churn for little gain
  at current batch sizes (memory-lane flushes are tens of keys, bounded by
  65,536).
- **Recommendation: Option 1** (move + borrow; no type forks), plus
  `transaction_ticks` via `committed_index` (expose a
  `T1Journal::transaction_position(id)`). Option 2 recorded as the
  escalations path only if measured batch sizes make the remaining owned
  copies matter.

**For (b) reclaim:**
- **Option 1 — mechanical dedupe:** delete the discarded pre-read (:3452-3454);
  borrow-based domain peek replacing `StoredObject::new(bytes.clone())`;
  `objects` becomes `HashSet<Hash>` (identity 32 B; the within-unit duplicate
  refusal becomes a containment check on insert, same rule, same error string
  :3477-3480); manifest-chunk sharing stays allowed by inserting without
  refusing when already present (:3498's `!contains` shape).
- **Option 2 — full `Arc` shared backing of read objects across a reclaim
  pass:** overkill — the bytes are consumed per-unit for verification only;
  no cross-unit reuse exists in one call.
- **Recommendation: Option 1.** All three changes preserve the exact refusal
  strings and the hash-verified-read discipline (`BeginReads` contract,
  reclaim.rs:672-680: "Every implementation MUST hash-verify").

**For (a) composite reader:**
- **Option 1 — fix now:** memo stores `Arc<[u8]>` (+
  tag), `VerifiedChunk.payload: Arc<[u8]>`; the fresh path moves the read
  bytes into one Arc (payload exists 1× + Arc refcounts); memo hits clone the
  Arc only; `composed_payloads`/`reconstruct_legacy_bytes` concatenate from
  `&[u8]` borrows unchanged (session_chunks.rs:748-756 already borrows).
  Entry-level coherence (:554-567) and the full hash re-verification
  (:530-534) are untouched — verification *scope* is unchanged; only
  ownership of the verified bytes is shared.
- **Option 2 — defer to K2-C activation:** the module is dark; no production
  cost exists today.
- **Recommendation: Option 1**, because the change is small, the dark tests
  (composite_reader.rs, k2b_reader.rs, dark_reachability.rs — ~40 memo tests)
  pin the behavior, and doing it at activation time would mix a
  representation change into the authorization checkpoint. The trust
  boundary concern in the charter — "do not reuse stale verification across a
  medium mutation" — is already structurally answered: the memo is
  per-operation (`VerificationMemo::new()` per call site, doc :408-415), and
  the design keeps it that way; **no cross-operation memo is proposed** (that
  would require content-addressed immutability guarantees the providers
  don't expose for reclaim-deleted chunks).

#### 2.2.3 Migration/compatibility

No wire-format change anywhere in B03: `ConsequenceBatch`, reclamation
ledger, manifest, and checkpoint encodings are untouched; the pending-slot
retention rule (the crash-safety semantics) is preserved bit-for-bit —
`retry`/`reconcile` behavior and the `Unknown{WorkerReplyLost}` mapping
(pipeline.rs:1295-1299) must be indistinguishable. The `Arc` change in
composite_reader is an internal type change; `VerifiedComposite`'s public
fields change `Vec<u8>` → `Arc<[u8]>` — a public-API change in a dark module
consumed only by tests and `session_chunks::reconstruct_legacy_bytes`
(borrow-compatible). Public-API changes in engine-persist are recorded in the
lane report per program discipline; this one is pre-activation by design.

#### 2.2.4 Test / evidence plan

- **Must hold:** pp_b3_reclaim.rs (3,698 lines — the reclamation matrices),
  retirement_model.rs, admission_model.rs, b3 batteries, pp_b4_pending.rs
  (the pending-attempt retention battery), the S6 fuzz gate.
- **New discriminating tests:** (1) duplicate-within-unit refusal preserved
  under the HashSet (a unit with a repeated ref — existing coverage to
  confirm, plus a >2-entry case); (2) a reclaim pass over a many-ref unit
  (≥1,000 refs) asserting identical output to the pre-change vector path and
  a bounded-time assertion (the quadratic membership was the risk);
  (3) pending retention on readback failure unchanged (existing pp_b4 tests
  are the evidence — re-run, no edit); (4) memo-hit equality: the ~40
  existing memo tests already assert deterministic-with/without-memo results.
- **Measurement:** allocation-count and bytes-cloned counters on the commit
  path (test-instrumented, disposable fixtures) before/after; reclaim pass
  wall time at unit sizes {10, 100, 1k, 10k refs}.

### 2.3 B13 — worker round-trips: caller-path map, queue wait vs service, partitioning question

**State:** CONFIRMED HIGH (FINDINGS; folds NEW-PERSIST-05). **Charter text:** §6 B13.

#### 2.3.1 Current shape (verified at HEAD)

- **The transport:** `WorkerMedium::request` (worker.rs:621-650) health-checks,
  allocates a capacity-1 completion slot (:184 `sync_channel(1)`), does a
  **blocking bounded send** (`SyncSender::send` — waits when the 64-slot
  queue is full, DEFAULT_QUEUE_CAPACITY :44-46), then **blocks on
  `slot.recv()`**. Every `CanonicalMedium` method rides this
  (`head`/`append_admitted`/`read_record`/`read_object`/`classify_object`/
  `validate`/`reconcile_unknown`/reclaim-family/`checkpoint`) — the recv sites
  at :646, :698, :756, :803, :848, :888, :932. `submit_required` (:1222-1250)
  is the blocking-send admission returning the receiver to the caller;
  `try_submit` (:1194-1212) is the nonblocking optional admission;
  `submit_probe` (:1167) is `#[doc(hidden)]` **test-only** (the register's
  ":1174" entry is this probe, not a production path).
- **The uncertain-class discipline:** a lost reply after append admission is
  `AppendOutcome::Unknown{WorkerReplyLost}` (:704-712) — never a definite
  failure; the pipeline's retained pending attempt reconciles it. This is the
  invariant any B13 change must not touch: **never acknowledge a commit
  before durable**.
- **Caller-path map (who blocks, on what):**

| Caller path | Thread | Blocks for | Citation |
|---|---|---|---|
| `commit_canonical` | authority/caller thread (the interactive demo's **render thread**) | journal fsync (capture already done at the deactivation boundary) + queue wait + walk service (`read_record`/`classify` per suffix link) + append (provider fsync/COMMIT) + read-back walk | orchestration.rs:765-772, pipeline.rs:1762-1788, walk :1513-1585 |
| `capture_snapshot` | caller thread (&self) | **direct whole-journal scan_file** (not even via the worker) + one worker head round-trip (`observe_head`) | orchestration.rs:780-825 |
| `submit_publication` / `poll_publication` | caller thread | **nonblocking by design** (try_send + try_recv) | orchestration.rs:837-904 |
| `wait_publication` | caller thread | blocking recv (explicit opt-in) | :907-919 |
| maintenance (`publish_checkpoint`/`rotate_journal`/`reclaim_checkpoint`) | caller thread under exclusive lease | FIFO barrier drain + compound (full scans, re-opens, provider reclamation) | :1140-1212 |
| `close` | caller thread | final snapshot via submit_required + wait | :1412 |
| interactive demo loop | **the presentation thread** — renderer + session on one thread | all of the above at their cadences | persist_demo.rs:2598-2612 (ShellApp+Renderer) with :1421 `commit_canonical` in the tick loop |

- **Measured cost (E-8 baseline, rig 9800X3D):** T1 journal transaction
  median 7.78 ms; canonical confirmation 5.97 ms (SQLite 1.78); snapshot
  publication median 14.46 / p99 19.80 ms (SQLite 12.53/19.69). At a 60 Hz
  frame budget (16.67 ms) a p99 publication on the render thread is a full
  frame; a confirmation is a third of one. Cadence matters: `commit_canonical`
  fires only on capture-bearing transitions (persist_demo.rs:1415-1421),
  snapshots at snapshot cadence — per-frame blocking is bounded to event
  boundaries today, and the demo's own overlay *measures* the stall
  (`confirmation_ms` :1422).
- **Queue wait vs service time:** currently unmeasured as separate quantities.
  The worker has fault seams (`StallEach`, worker.rs:414) for tests; the
  local provider keeps `timings` (e.g. `reconcile_nanos`, local.rs:1674-1678)
  but nothing separates admission wait from execution at the `Job` level.

#### 2.3.2 Options considered

**Option 1 — status quo + documentation.** Transitions are rare; the stall is
observable (`confirmation_ms`); the T1-before-confirmation ordering is the
accepted discipline.

- *Pros:* zero risk; the five-state model already makes the canonical
  confirmation *deferrable in effect* (JournaledT1 is durable; `Unknown`
  outcomes reconcile).
- *Cons:* leaves a 6-20 ms p99 render-thread stall on transition frames with
  a growth slope (walk service grows with suffix length; queue saturation
  under burst backups the send); the charter explicitly asks for the
  queue-wait/service separation and the deferral question to be answered.

**Option 2 — deferred completion for the canonical confirmation.** Keep the
T1 capture synchronous at the deactivation boundary (that fsync is the
durability the boundary promises). Make the *canonical confirmation*
submit-and-pollable exactly like `submit_publication`: nonblocking admission
(try_submit under the fairness gate), FIFO execution against the captured
verified head, poll returns `CommitOutcome` only after the verified read-back
walk (the existing `confirm`, pipeline.rs:1383-1439). The blocking
`commit_canonical` remains (tests, close, and callers who want the receipt).

- *Pros:* reuses the *proven* publication machinery (fairness gate,
  `required_waiting`, generation guard — orchestration.rs:837-886); no new
  threads; exactly-once is already guaranteed by the idempotency-key +
  pending-attempt discipline, which is *why* deferral is safe; the
  presentation thread's worst stall becomes the T1 fsync at the capture
  boundary only.
- *Cons:* a real API/behavior addition (a new orchestration state family);
  completion lands a frame or more later, so the demo overlay's per-transition
  label must poll (a presentation change, contained in persist_demo);
  the required-op fairness (a waiting `commit_canonical` blocks optional
  publications, :847-848) needs an equivalent for the async form.

**Option 3 — partitioned reads / multiple workers.** Split read ops
(`read_record`/`read_object`/`classify`) onto parallel workers to cut walk
service time.

- *Cons:* the single FIFO worker *is* the serialization point that makes the
  authority ordering and the FIFO barrier cheap to reason about; reads through
  a second worker would need their own provider coordination (the providers'
  own locks make this safe but the shared-queue FIFO guarantees — "every
  earlier admitted publication settles BEFORE the rotation", :1153-1156 —
  would need re-proof across two queues); service time is fsync/SQLite-bound,
  not CPU-bound, so parallelism buys little; the charter: "avoid extra
  threads if overhead outweighs benefit."
- **Recommendation: rejected** at current scale; revisit only if §4
  measurement shows walk service (not fsync) dominating confirmation latency
  at target scales.

**Recommendation: Option 2 + instrumentation, Option 1 as the fallback.**
Land the queue-wait/service-time separation first (measurement, below), then
the deferred-completion variant behind the existing fairness gate. If
measurement at the §4 workloads shows transition-frame stalls already inside
the demo's frame budget with margin at target scales, the deferral lane stops
at the instrumentation + documentation stage and the blocking path stays —
that is an evidence-backed no-change per the charter, not a skip.

#### 2.3.3 Migration/compatibility

No persistence-format or authority change. The worker transport, poison
gates, uncertain-class mapping, and maintenance envelope are untouched. The
new API is additive (a `submit_commit`/`poll_commit` pair mirroring the
publication pair); the blocking form keeps its exact semantics for every
existing caller and test. Presentation-only behavior change (completion label
timing) is confined to persist_demo's overlay and its evidence collector.

#### 2.3.4 Test / evidence plan

- **Must hold:** pp_b4_worker.rs / pp_b4_pending.rs / pp_b4_maintenance.rs /
  pp_b4_orchestrated.rs (the worker transport batteries: reply loss, poison,
  queue saturation, FIFO barrier), s6_burst.rs (burst backpressure), S6 fuzz
  gate.
- **New discriminating tests:** (1) async-commit completion equals blocking
  commit outcome for the same fixture sequence (differential over a scripted
  transition sequence incl. stale-parent and Unknown injections);
  (2) fairness: a waiting required async commit still refuses optional
  publications (mirror of the existing `required_waiting` tests);
  (3) queue-wait instrumentation: a `StallEach`-faulted worker shows the wait
  in the stats while service stays near-zero (the separation is real);
  (4) never-acknowledge-before-durable: an armed `LoseAppendReply` under the
  async path must surface `Unknown` on poll, never `Confirmed` (the existing
  uncertain-class tests re-targeted at the new entry point).

### 2.4 A13 — pure validation/encoding/planning vs backend I/O and orchestration

**State:** CONFIRMED (auditor label corrected by orchestrator, FINDINGS).
**Charter text:** §5 A13 + §7 P3 ("factor common pure operations while
leaving actual backend durability differences explicit … do not turn every
provider into a generic trait hierarchy").

#### 2.4.1 Current shape (verified at HEAD)

The crate is **not** unlayered — the split exists in the newest modules and
blurs in the oldest:

- Already separated: `CommitAuthority` (journal-only, no medium I/O,
  pipeline.rs:659-665) vs `CommitVerifier` (medium-only, journal-blind,
  :667-692) with the facade composing them (:694-707) — the module doc
  explicitly reserves the future actor split; the V2X-CP2 `BeginReads`
  reader-generic walk pattern (reclaim.rs:675-705) with provider adapters
  (`MediumReads`) and test readers; `recovery_discovery`/`recovery_classification`
  read-only and reader-generic; `range_*`/`batch_completion` pure codecs with
  owner-ratified scopes; `recovery.rs` a deterministic pure function.
- Blurred (the A13 evidence): `LocalFileMedium::validate_observed`
  (local.rs:714-749) interleaves the trusted-cursor decision logic with
  `walk_suffix`/`cold_walk_from`/`absorb_suffix` **inside lock-held provider
  methods**; `validate` (:1656-1671) is lock()→cold-walk()→unlock() in the
  trait impl; the same walk logic is duplicated in sqlite.rs's provider impl
  (its own cursor/suffix machinery). `local.rs` at 12,993 lines and
  `sqlite.rs` at 8,274 carry layout + lock protocol + HEAD publication +
  walks + orphan protocol + reclaim execution + timings in one file each
  (A01 persist half).
- The audit's "Refuted" label was wrong on its own evidence (FINDINGS
  reconciliation): the mixing is real; what the auditor actually showed is
  that *validation functions exist as identifiable units* — which is the
  extraction point, not a refutation.

#### 2.4.2 Options considered

**Option 1 — full provider trait-generalization** (a
`ChainWalkProvider` abstraction unifying local/sqlite behind generic
walkers, providers reduced to trait impls). **Rejected:** the charter
explicitly forbids it; the providers' *distinctions* are the contract
(SQLite `CommitUncertain`/`RollbackUncertain`/`Contention` vs the local
nine-step fsync ladder; SyncTier per platform). `CanonicalMedium` +
`BeginReads` are already the right and only shared boundaries.

**Option 2 — extract the shared walk core, split the provider files by
change reason.**
- A `walks.rs` (engine-persist-internal): the pure, reader-generic
  suffix/cold/gap-aware chain-walk machinery (`walk_suffix`,
  `cold_walk_from`, `absorb_suffix`, divergence classification, the
  skip-table descent already proven in reclaim/pipeline), expressed over
  `BeginReads` exactly as reclaim's V2X-CP2 walks are. Both providers keep
  their locks and call the shared core; the duplication between local and
  sqlite walk logic is removed (the charter's "reduce unnecessary
  duplication and representation churn").
- `local.rs` → `local/{mod,layout,head_protocol,orphan,reclaim_exec,timings}.rs`
  (paths+dirs; HEAD.tmp/rename/lock discipline; the v1.4 orphan protocol; the
  destructive reclaim under HEAD.lock; the diagnostics counters) — pub paths
  preserved by re-export so `engine_persist::local::LocalFileMedium` and
  every test import stay valid.
- `sqlite.rs` → `sqlite/{mod,rows,walks-caller,backup,meta}.rs` on the same
  principle; `reclaim.rs` → `reclaim/{ledger,discovery,plan,execution}.rs`
  (codec; reader-generic discovery; MK-bound plan publication; Phase A/B
  execution).
- *Pros:* change reasons become file boundaries (A01's actual test);
  the walk core gets one auditable home; provider files shrink to provider
  truth; no public-API churn (re-exports); no behavior change by
  construction (moves + one deduped call path).
- *Cons:* the local↔sqlite walk unification is semantics-bearing (divergence
  handling, cursor invalidation rules) — the risk is a subtly different
  refusal string or walk bound in the unified core; needs the differential
  battery below. Large diff; must come **after** the B02/B03 semantic lanes
  to avoid moving code twice.

**Option 3 — no structural change; document the layering.** Rejected as the
whole answer: the 13k-line file is a maintainability finding (A01) with a
confirmed mixing pattern; but *parts* of Option 2 can be staged so each step
is independently reviewable.

**Recommendation: Option 2, staged last** (§3 lane L7), orchestrator-reviewed
with the walk unification orchestrator-implemented. The `CommitAuthority`/
`CommitVerifier` seam stays as-is — it is the charter's requested shape
already; no actor/thread split is added in P3 (that is the B13 deferral
lane's business, and only where measurement warrants).

#### 2.4.3 Migration/compatibility

No wire formats, no public-API removals (re-exports keep paths). The one
semantic risk — unified walk core behavior — is bounded by differential
tests: for a generated corpus of chains (valid, divergent, gapped,
corrupt-referenced, unfounded), the unified core's outputs (summaries,
errors, cursor state) must equal the pre-split providers' outputs. Refusal
rule strings (`MediumError::Chain { rule }`) are part of the observable
contract and are frozen.

#### 2.4.4 Test / evidence plan

Must hold: the full engine-persist battery (b3, process, admission_model,
retirement_model, pp_b1/b3/b4 families) with zero test edits; fmt/clippy;
count pins unchanged (moves add no tests). New: the walk differential corpus
test (one new test file, deliberately added, count-pin documented).

### 2.5 A22-persist — retention/compaction lifecycle for consequence-tracking state

**State:** CONFIRMED (persist half). **Charter text:** §5 A22 + §7 P3
("evaluate historical growth of dedup keys and confirmation evidence").

#### 2.5.1 Current shape (verified at HEAD — register correction included)

The register/audit cite `confirmed_history` at local.rs:3015-3028; **that
symbol does not exist at HEAD** (grep: zero matches crate-wide), and
local.rs:3013-3018 is now inside `#[cfg(test)] mod exclusion_tests` — the
audit's line refs were stale. The real persist-half growth surfaces at HEAD:

1. `CommitAuthority.keys: BTreeMap<ConsequenceId, KeyStatus>`
   (pipeline.rs:664): seeded at open from **every** committed transaction's
   envelopes (:719-727), extended by `stage` (:750); entries are removed only
   for InMemory-phase discards (:767-776). Journaled/Confirmed phases are
   retained **forever** — this is the identity/dedup evidence (conflicting-
   reuse detection at :744-749).
2. `CommitVerifier.confirmed: BTreeMap<ConsequenceId, ConfirmedEntry>`
   (:677): rebuilt at open by the session-open chain walk
   (`CommitPipeline::open` :1676-1684, "O(chain), documented honestly"),
   extended by each completed walk, **never pruned** (no removal sites).
   `ConfirmedEntry` carries full `coverage: Vec<ClassifiedSpan>` (:594-600).
3. `T1Journal.state.committed` — every transaction fully decoded (envelopes,
   plan bytes ≤1 MiB each) for the journal handle's lifetime
   (journal.rs:1144-1164); `acknowledged` is session-scoped (:1623).
4. `collect_actionable` (pipeline.rs:895-934) scans **all** transactions ×
   envelopes on **every** commit — O(total journal history) per commit, with
   the confirmed-lookup skip but no settled-transaction watermark.
5. File-level lifecycle exists and is sound: rotation (rotation.rs) re-anchors
   surviving transactions into a fresh journal; reclamation deletes stale
   generations after checkpointing; the pipeline reopens over the new
   generation (orchestration.rs:1153-1166) — so **rotation+reclaim is already
   the compaction point for 1-3** (the reopened pipeline rebuilds from the
   smaller journal and the chain). What is missing: (a) an explicit cadence
   policy (rotation is maintenance-invoked, not threshold-driven); (b) the
   per-commit O(history) scan; (c) per-entry detail (coverage) retained where
   only membership is needed.

#### 2.5.2 Options considered

**Option 1 — lifecycle documentation + cadence policy only.** Declare
rotation+reclaim the compaction point; add a documented size/age trigger the
application layer (slice) reads; no engine change.

- *Pros:* honest and cheap; the mechanism exists and is tested (pp_b3
  rotation/reclaim batteries).
- *Cons:* does not bound the *within-generation* growth (a long-lived
  generation still grows keys/confirmed/scan cost monotonically); the
  charter asks for bounded **active RAM** with history preserved.

**Option 2 — incremental settled-transaction watermark + detail shedding.**
- Watermark: the authority tracks per-transaction settledness (all envelopes
  confirmed or the transaction canceled → skip in `collect_actionable`).
  Appends only add transactions and promotions only settle them, so a
  monotone scan cursor + a settled-bitmap is sound; canceled transactions are
  already skipped via `is_actionable` (journal.rs:2320-2322) but still
  visited. Per-commit work becomes O(unsettled + new) instead of O(total).
- Detail shedding: `ConfirmedEntry.coverage` is consulted only against
  journal envelopes (`collect_actionable` :908-923, `foreign_receipt`,
  `remaining_keys`); once the producing transaction is settled (or rotated
  away), coverage is dead weight — shed it (keep id → revision/transaction),
  re-derivable from the chain by a walk if ever needed again. `keys` keeps
  phase only (it already does — KeyStatus is small; the growth driver is
  entry count, which is the dedup evidence and must stay).
- *Pros:* bounded active RAM proportional to *unsettled* work + a compact
  confirmed membership; history stays durable (journal + chain); no format
  change.
- *Cons:* semantics-heavy (the settledness rule interacts with
  foreign-writer confirmations — another writer's batch can settle our keys,
  `remaining_keys` :987-1008); needs careful ordering proof: promotions run
  only inside completed walks (:687-691 `walk_rounds` discipline).

**Option 3 — summarize confirmed membership (id-set only) and re-derive
detail on demand.** More aggressive shedding than 2; trades a re-walk for
coverage checks. Rejected as the first step: the foreign-confirmation
equivalence check (coverage/transaction/effect, :916-922) is on the
exactly-once path — re-deriving it from the chain per check would put
O(chain) walks back on the commit path.

**Recommendation: Option 1 + Option 2 staged** — the cadence policy and
lifecycle documentation first (with the demo/soak exercising rotation at a
documented threshold), then the watermark + shedding as an
orchestrator-implemented lane with the ordering proof written into the module
doc. **The identity/dedup evidence stays complete and queryable** (keys
membership is never dropped — that is the "no lost consequences" line); what
gets bounded is per-entry detail and scan work, plus the documented rotation
cadence that bounds generation size.

#### 2.5.3 Migration/compatibility

No durable-format change: keys/confirmed are in-RAM session state rebuilt
from journal+chain at open. The watermark is derived state. Behavior
observable through the public API (commit outcomes, refusals, recoveries)
is unchanged; the only observable differences are timing/memory.

#### 2.5.4 Test / evidence plan

- **Must hold:** the S6 fuzz gate values (exactly-once is what it measures),
  b4/b5 batteries, pp_b4_pending (retry identity), pp_b3 rotation/reclaim
  (the compaction points), npc_memory_lane recovery tests.
- **New discriminating tests:** (1) a long-journal fixture (e.g. 5,000
  settled transactions) asserting per-commit *scan count* (instrumented
  counter) is O(new) — the exact regression the watermark prevents;
  (2) foreign-writer settlement: a batch confirmed by another writer still
  settles our keys and never re-appears as actionable (existing semantics,
  now also asserted at scale); (3) shed-coverage correctness: a
  conflicting-reuse stage of a settled id still refuses ConflictingReuse
  (membership retained) while a provenance query on a settled id re-derives
  or refuses exactly as before; (4) rotation cadence: a generation exceeding
  the documented threshold with maintenance available rotates (and without
  maintenance available, the growth is reported in stats, not silently
  accepted).

### 2.6 B04 (persist half) — transaction state machine: RESOLVED, no further change

The slice-side lifecycle was fixed and integrated (`e31dc61`, FINDINGS). The
persist half — `PendingAttempt` retention and the retry/reconcile/confirm
state machine — was re-read at HEAD for this design: the write-at-uncertainty
retention (pipeline.rs:1364-1374), the exception-safe `confirm_retained`
(:1285-1313, Unknown-mapping at :1295-1299), `reconcile` (:1090-1108) and
`retry` (:1127-1144) with the original-candidate-stays-in-slot rule
(:1117-1126) are coherent, and the classification-only flush path retains the
same TransactionId through retries (the audit's :1278-1298 area, verified as
`submit_keys`/`append_for` idempotency-key = batch content hash :1345-1360).
**Recommendation: no change** beyond the B03 clone reduction (§2.2), which
touches the same functions without touching the retention rules. Evidence:
the S6 fuzz gate (2×200 ops, double-run compare, npc_memory_lane.rs:2163-2205)
and pp_b4_pending.rs are the standing pins; they gate the B03 lane too.

### 2.7 B12 — persistent identity integration: interface verified, ownership stays with P2

Persist-side facts at HEAD: the pipeline's durable identity is the structured
`ConsequenceId` (consequence.rs mod doc: schema u8, region u32, episode u64,
npc u32 — **no fixed population baked into the persist contracts**); the
memory-lane recovery proves the round-trip through actual authoritative
storage (npc_memory_lane.rs:1420-1519: plan decode → identity cross-check →
ledger recovery → canonical confirmation phases). The one genuine coupling:
`session_anchor.rs` decodes the director save section using engine-core's
`REGION_COUNT`/`NPC_COUNT`/`REGION_GRID` constants (:45-46, :233-350) — a
ratified, documented shared-wire-format interpretation (mod doc: "ONE
interpretation of those wire formats' anchor-relevant parts"), but it is
exactly the seam P2's world-representation generalization (A05/B12) must
move. **P3 recommendation: no persist-side change; record the coordination
point** — P2's identity/world work owns `npc_world_save` + `NpcIdentity`
callers, and when it generalizes the save sections, `session_anchor` (and
its bounded-count validations) moves in the same change with a versioned
save-format migration per the charter's compatibility rules. P3's lanes
deliberately avoid touching that module.

---

## 3. Staged implementation plan

Lanes in dependency order. Each is independently reviewable and lands on
`refactor/engineering-excellence` through the program's review discipline.
Risk classes follow the standing rules (FINDINGS calibration record):
**semantics-heavy durability work is orchestrator-implemented**; mechanical
changes with tight contracts and named discriminating tests may be delegated
with strict briefs; every completion claim verified against the diff.

| Lane | Scope (one responsibility) | Findings | Risk class | Depends on | Count-pin expectation |
|---|---|---|---|---|---|
| **P3-L1 journal read-surface callers** | Bounded prefix read for header-only uses (reclaim.rs:2449/:2610/:2618/:2628, crossing.rs:1010); `capture_snapshot` frontier from in-memory journal state (orchestration.rs:807-813) keeping the pointer-resolution refusal | B02 | **Delegable** (mechanical, tight contract: identical `BoundHeaderState`/refusals; discriminating tests named) | — | engine-persist +2..3 (prefix-reader unit + frontier-equality), engine-slice +1 (frontier test) |
| **P3-L2 reclaim unit-collection dedupe** | Remove the discarded pre-read (reclaim.rs:3452-3454); borrow-based domain peek replacing `StoredObject::new(bytes.clone())` (:3483, and the same pattern is *not* touched elsewhere — see honest limits); `HashSet` membership (:3476/:3498) | B03(b) | **Delegable** | — | engine-persist +2 (duplicate-ref refusal at >2 entries; large-unit equivalence) |
| **P3-L3 pipeline clone reduction** | Single retention copy for the pending attempt (move at :1213, borrow-confirm); `transaction_ticks` via `committed_index` (expose `T1Journal::transaction_position`) | B03(c), B04-adjacent | **Delegable with strict brief** — the retention rules are the crash-safety semantics, so the brief must name pp_b4_pending as the gate and forbid touching :1364-1374/:1285-1313 rules | L1 (journal API addition) | engine-persist +1 (pending-retention on readback failure re-asserted at the new call shape) |
| **P3-L4 recovery single-decode** | crossing.rs:3192-3238: one classification, both views derived (scan-from-state or state-adopting open); identical `TransitionError` details | B02 | **Orchestrator-implemented** (recovery semantics) | L1 | engine-slice +1 (recovered-transactions equality test); recovery matrices re-run green |
| **P3-L5 streaming journal reader** | `read_journal_bytes`-replacement frame iterator for open/scan/refresh with bounded buffers; differential oracle vs the whole-file classifier; old classifier retained as test oracle then removed | B02 | **Orchestrator-implemented** (torn-tail/chain semantics) | L4 (recovery already single-decode, so the blast radius is contained) | engine-persist +1 test file (differential corpus — the corpus generator + equivalence assertions); b3/journal_process batteries unchanged |
| **P3-L6 A22 retention: watermark + shedding + cadence** | Settled-transaction watermark in `collect_actionable`; coverage shedding for settled/rotated-away ids; documented rotation cadence policy + stats surface | A22-persist | **Orchestrator-implemented** (exactly-once path) | L3 (the scan it optimizes is post-clone-reduction) | engine-persist +3..4 (scan-count O(new) assertion; foreign-settlement at scale; shed-coverage conflicting-reuse; cadence stats) |
| **P3-L7 B13 deferred completion + instrumentation** | Queue-wait vs service-time separation in worker stats (bounded, A21 discipline); `submit_commit`/`poll_commit` nonblocking confirmation behind the fairness gate; blocking path unchanged; persist_demo overlay polls | B13, NEW-PERSIST-05 | **Orchestrator-implemented** (authority ordering; never-ack-before-durable) — **gated on §4 measurement**: if transition-frame stalls are inside budget with margin at target scales, this lane stops at instrumentation + documented budget (evidence-backed no-change) | L6 (commit path post-watermark, so the deferral is measured on the final shape) | engine-slice +3..4 (differential async-vs-blocking; fairness mirror; Unknown-on-poll; stats separation) |
| **P3-L8 composite Arc backing** | `VerificationMemo`/`VerifiedChunk` share `Arc<[u8]>`; fresh path 3×→1×; memo hit clone→refcount | B03(a), NEW-PERSIST-04 | **Delegable** (dark module, ~40 pinning tests) | — (independent) | engine-persist ±0 (existing tests retargeted to the new field type — representation change inside a dark module; if public-field tests need edits, the exact mapping is documented in the lane report per program rules) |
| **P3-L9 A13 module split by change reason** | Shared reader-generic walk core (local+sqlite dedupe); `local/`, `sqlite/`, `reclaim/` module splits with re-exports; walk differential corpus | A13, A01-persist | **Split risk class:** walk unification = orchestrator-implemented; file moves = delegable | L4, L5, L6 (semantic lanes first — no double-moving) | ±0 tests (moves only) +1 (walk differential) |

Sequencing rationale: L1/L2/L8 are independent quick wins; L3 precedes L6 so
the scan is measured post-clone-reduction; L4 precedes L5 so the streaming
reader's integration surface is minimal; L7 is measurement-gated last among
semantic lanes; L9 is deliberately last (moves everything else first, then
the files move once).

---

## 4. Measurement plan (charter §8.2, persistence row)

**Existing baseline (E-8, rig 9800X3D, release, tree `263532f`):** snapshot
publication 14.46/19.80 ms median/p99 (SQLite 12.53/19.69); T1 transaction
7.78 ms; canonical confirmation 5.97 ms (SQLite 1.78); recovery 6.16 ms
(SQLite 6.24); store growth ~230,747 B/snapshot; lifetime growth ~31.0 B per
execution; compaction 1.16 ms. These are the "before" numbers; every P3 lane
reports against them on the same rig, serially, release mode.

**Workloads (all disposable fixture saves in `std::env::temp_dir()` — the
`s7-*` pattern the batteries already use; never owner data):**

1. **Journal growth scales:** generated campaigns at {1k, 10k, 100k}
   transactions with the realistic envelope mix (memory-lane plans + frontier
   entries), both providers. Cold open (fresh process), warm re-open,
   recovery, and snapshot-capture path. Metrics: wall time, **bytes read per
   open** (counted at the read sites), peak working set (process
   PeakWorkingSet64 via the harness; the pod runs it, the rig confirms),
   decoded-object count. Before/after per lane L1/L4/L5.
2. **Transaction mix:** append / verify / confirm / read / recover with
   injected failures — the existing fault.rs seams and the
   `crash-test-seams` process-kill matrix already implement this; the lanes
   add no new fault points, they re-run the batteries (b3_process,
   journal_process, b4_process) as evidence.
3. **Batch-size sweep for clone reduction:** commit batches at
   {1, 10, 100, 1k, 10k} keys — allocation count + cloned bytes (test-side
   counters), commit wall time. Lane L3.
4. **Queue wait vs service time (B13):** the new worker stats separate
   admission-wait from execution per job kind; workloads: idle, ordinary
   transition cadence, s6_burst saturation, `StallEach`-faulted service
   (the fault seam makes the separation observable deterministically).
   Lane L7's gate quantity: render-thread stall budget on
   transition/snapshot frames at 60 Hz and the demo's target scale.
5. **Retention/long-run (A22):** sustained multi-hour-equivalent logical
   history — the soak scenario's tick volume with per-commit scan-count and
   keys/confirmed RAM reported as growth curves (the charter's "a bounded
   backlog over a short run is not proof"); rotation at the documented
   cadence vs never-invoked, showing the compaction point's effect on active
   RAM. Lane L6.
6. **Reclaim pass at unit sizes** {10, 100, 1k, 10k refs}: wall time,
   object-read round-trips (before/after L2; the quadratic membership is the
   specific risk).

**Fixture discipline:** every save/campaign lives under the temp-dir
fixtures; artifacts (raw logs, per-run summaries) go to the approved scratch
root `Temp\opencode\ee\artifacts\` with command+exit+tree SHA recorded in
EVIDENCE.md per the established format. No benchmark substitutes debug
timings; release only. Fsync/SQLite-transaction/CPU-hash time are reported
separately (the local provider's timings counters + SQLite's own) so
"durability cost" is never conflated with CPU cost. Peak RSS is reported with
the measurement's own overhead noted. Provider-specific guarantees stay
labeled (never averaged across local vs SQLite).

---

## 5. Non-goals (explicit)

1. **No hosting/authority policy change.** Single-writer fail-closed
   authority, the sidecar-lock publication discipline, the Option-B
   conservative recovery policies, and the worker poison gates are
   preserved verbatim; no multi-writer, no seamless-crash-migration, no
   hosting model work (DIRECTIVE §0 preserved-requirements list).
2. **No provider genericization.** Exactly two providers behind the existing
   `CanonicalMedium` + `BeginReads` boundaries; no trait hierarchy, no
   provider-agnostic "backend" abstraction (charter §7 P3 last sentence).
   The SQLite-vs-local production decision (D-PP-1) is not reopened.
3. **No format changes without explicit versioned migration.** `journal/1`,
   `bound-journal/1`, `revision-record/1`, `envelope/1`, `consequence-id/1`,
   reclamation/checkpoint/ancestry formats are untouched by every
   recommended lane. The rejected B02 Option C (on-disk index) is rejected
   *because* it would add a format surface. Any future format need surfaces
   as a versioned-migration proposal for owner decision, never a silent
   change.
4. **No unrelated rewrite.** The commit state machine (B04), the recovery
   contracts, the encounter machinery, the V2X-CP3 retained-evidence
   classifiers, and the migration tooling are out of scope beyond the cited
   clone/dedupe touches. The `/2` composite stays **dark** (no activation —
   that is a separate owner authorization); P3-L8 only fixes its internal
   representation.
5. **No new threads by default.** Partitioned reads / multiple workers are
   rejected pending §4 evidence that walk service (not fsync) dominates at
   target scale; the deferred-completion lane reuses the existing single
   worker and queue.
6. **No test-count inflation.** Count pins move only through deliberately
   added discriminating tests, each documented in the lane report per the
   E-6 discipline; existing pins (main shard, heavy shard, S6 fuzz values,
   recovery matrices) must hold without edits.

---

## 6. Honest coverage limits

- **Not line-read:** the entirety of local.rs/sqlite.rs/recovery_classification.rs/
  ancestry_witness.rs (navigated by grep + targeted slices around every
  cited symbol); the V2X-CP3 modules beyond their module docs and the
  reclaim call paths cited; engine-slice's soak_scenario internals;
  sqlite.rs's row layer.
- **Audit line-ref reconciliation:** persist.md's `confirmed_history`
  (A22-persist) and its B02 caller list were stale at HEAD; both were
  re-derived here (§2.1.1, §2.5.1). The audit's B03 `reclaim.rs:5686` cite
  resolved to the `collect_unit_objects` block at :3448-3505 (the ~5686
  lines are `#[cfg(test)]` clones).
- **Not measured here:** this is a design document; no benchmarks were run
  (read-only mandate). E-8's numbers are the quoted baseline; every lane's
  claims about improvements are *expectations to be measured*, marked as
  such.
- **Measurement-gated decisions:** the B13 deferral (P3-L7) and any
  partitioned-read revisit are explicitly contingent on §4 evidence; if the
  evidence favors no-change, the no-change is recorded with the data.
- **Unresolved questions for the orchestrator/review:**
  1. P3-L7's presentation contract: is a completion label landing one frame
     later acceptable to the demo overlay contract, or must the first poll
     after a transition frame be guaranteed same-frame when the queue is
     empty? (Presentation judgment, not durability.)
  2. P3-L6's cadence policy numbers (rotation threshold in transactions or
     bytes) need an owner-rig data point at the intended first-game scale —
     the design supplies the mechanism, not the threshold.
  3. P3-L8's public-field type change (`Vec<u8>` → `Arc<[u8]>` in
     `VerifiedChunk`) is inside a dark module but is API-visible; the
     orchestrator may prefer deferring to the K2-C authorization checkpoint
     — recorded as Option 2 in §2.2.2 with the tradeoff stated.
  4. The `capture_snapshot` frontier derivation (L1) assumes the pipeline's
     journal and the pointer-resolved active journal coincide except across
     flips, which the generation check refuses; the rotation code's re-open
     path (crossing.rs:2422-2424) was read to confirm the pipeline reopens on
     the new generation — if a rotation path exists that leaves the pipeline
     holding a *stale-but-unflipped* journal, the derivation must keep the
     file scan for that window (the lane brief must re-verify this invariant
     at implementation time against the then-current rotation code).

*Design complete at HEAD `bceb3faed42a4124bc615916b4a352cdf2d1979a`; all
citations verified against that tree.*
