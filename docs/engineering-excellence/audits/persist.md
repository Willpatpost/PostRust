# engine-persist audit (P0, read‑only)
- **Auditor:** gpt-oss-120b, date 2026-11-26, base `00a637c` (= baseline `5b93167` for crates/)

## Seed verification table
| ID | Verdict | Current location(s) | Evidence note |
|----|---------|----------------------|---------------|
| A01 | Partial | `src/local.rs` (~12.9k lines), `src/sqlite.rs` (~8.3k lines), `src/reclaim.rs` (~6.9k lines), `src/pipeline.rs` (~3.6k lines) – each mixes storage, validation, reclamation, orchestration. Test modules (`#[cfg(test)]`) appear throughout (45 matches across src). | Line counts from PowerShell (`Get‑Content | Measure‑Object`). |
| A13 | Refuted | Validation functions (`validate_observed` at `src/local.rs:714`, `validate` at `src/local.rs:1656`) are interleaved with backend actions (e.g. `append`, `commit`) in the same modules. No dedicated pure‑validation layer. | Presence of both validation and I/O/orchestration code in same files. |
| A22 | Confirmed | Confirmation/history cache grows without eviction (`src/local.rs` lines 3015‑3028 add to `confirmed_history`). No lifecycle policy found. | No pruning logic in the module. |
| B02 | Confirmed | `src/journal.rs:170‑189` (`read_journal_bytes`) reads whole journal into memory (capacity up to 4 GiB). Call sites at lines 1115, 1615, 1713, 1892, 1990, 2408, 3409, 3428 trigger full reads during recovery. | Reads up to 4 GiB in a single allocation; callers are recovery paths (e.g., `recover_memory_lane` in `engine-slice`). |
| B03 | Confirmed | `src/composite_reader.rs:416‑444` allocates manifest then clones payloads (`payload.clone()` at 463‑464, 544). `src/reclaim.rs` clones stored objects (`bytes.clone()` at ~5686). `src/pipeline.rs` clones keys/coverage (`PendingAttempt` clones). | Evident payload copies and memo clones in hot paths. |
| B04 | Confirmed | Transaction‑lifecycle state‑machine is consolidated in `src/pipeline.rs` (e.g., `PendingAttempt` handling lines 1088‑1313). Classification‑only flush path retry retains same `TransactionId` (see `src/pipeline.rs:1278‑1298`). | State‑machine logic centralised; retry identity preserved. |
| B13 | Confirmed | Worker back‑pressure appears in `src/medium.rs` (errors `WorkerUnavailable`, `WorkerReplyLost` at lines 71‑78, 194‑205) and is propagated through `src/pipeline.rs` (handling at 1088‑1313). Authority ordering enforced via `MediumError` contracts. | Worker failures block append paths; authority ordering enforced via error handling. |

## Subsystem boundary map (module → responsibility → seams)
- `local.rs` – local storage I/O, HEAD management, validation, cache; seams: mixes validation with orchestration.
- `sqlite.rs` – SQLite backend persistence, encoding/decoding, also performs validation steps.
- `reclaim.rs` – garbage collection, duplicate detection, object eviction; interacts with storage modules.
- `pipeline.rs` – commit pipeline, transaction state‑machine, worker coordination; bridges orchestration and storage.
- `medium.rs` – canonical I/O worker abstraction, back‑pressure handling.
- `journal.rs` – journal file format, full‑file reads, checksum verification.
- `composite_reader.rs` – manifest decoding, payload verification, memoisation.
- `ancestry_*` modules – ancestry accounting, classification, checkpoint handling.
- `consequence.rs` – durability classification, replay logic.
- `checkpoint.rs` – checkpoint creation and verification.
- `record.rs`, `hash.rs` – low‑level data structures, hashing utilities.

## Per‑file coverage table (selected highlights)
| File | Primary responsibilities | Prod‑vs‑test line split (approx) | Hot‑path allocations / copies |
|------|--------------------------|-------------------------------|------------------------------|
| `src/local.rs` | HEAD lock, append, validation, confirmation cache | ~13 000 total, ~1 200 test lines | Large `Vec<u8>` for journal reads, payload clones during verification. |
| `src/sqlite.rs` | SQLite backend reads/writes, encoding/decoding | ~8 300 total, ~1 400 test lines | Allocates full rows, clones bytes for objects. |
| `src/reclaim.rs` | Duplicate detection, object reclamation, cache eviction | ~6 900 total, ~300 test lines | `bytes.clone()` per object, linear `contains` scans. |
| `src/pipeline.rs` | Transaction orchestration, worker interaction, retry logic | ~3 600 total, ~50 test lines | Clones of `PendingAttempt`, `Coverage`, `TransactionId`. |
| `src/medium.rs` | Worker proxy, back‑pressure signalling | ~912 total, ~20 test lines | Minimal allocations; error objects only. |
| `src/journal.rs` | Journal file format, whole‑file read | ~5 500 total, ~150 test lines | `Vec<u8>` up to 4 GiB allocated in `read_journal_bytes`. |
| `src/composite_reader.rs` | Manifest decode, memoised payload verification | ~2 400 total, ~80 test lines | Payload `clone()` for memoised entries; manifests allocated per read. |
| `src/ancestry_*` | Classification, checkpoint, ancestry accounting | ~4 000 total, ~120 test lines | Moderate allocations for index structures. |

## NEW findings
| ID | Severity | File:line | Description | Suggested disposition |
|----|----------|----------|-------------|-----------------------|
| NEW‑PERSIST‑01 | Medium | `src/reclaim.rs:5686` | Linear `objects.contains` on a growing vector leads to O(n) duplicate detection; could degrade with large journals. | Consider using a hash set for O(1) membership checks. |
| NEW‑PERSIST‑02 | High | `src/journal.rs:170‑189` | Whole‑journal read allocates up to 4 GiB; recovery paths may trigger OOM on constrained nodes. | Add streaming verification path or chunked processing for large journals. |
| NEW‑PERSIST‑03 | Low | `src/local.rs:3015‑3028` | Confirmation/history cache grows without size bound; no eviction policy. | Document cache growth expectations; optionally add size‑based pruning. |
| NEW‑PERSIST‑04 | Medium | `src/composite_reader.rs:463‑464` | Payload cloned for memoised entries even when identical; unnecessary copy on cache hit. | Store `Arc<[u8]>` or reference‑counted payload to avoid cloning. |
| NEW‑PERSIST‑05 | Medium | `src/medium.rs:71‑78` | Worker‑unavailable error propagates synchronously, potentially blocking the calling thread for extended periods. | Introduce async channel with back‑pressure buffer and timeout handling. |

## Unexamined / limitations
- Did not perform exhaustive line‑by‑line static analysis of every `.rs` file (≈44 files, ~92 k lines) due to time constraints.
- Memory / CPU profiling was not possible; findings are based on code inspection only.
- Did not analyze `engine-slice` integration beyond the referenced worker back‑pressure paths.
- No runtime tests were executed; all observations are from source.

---
*This audit was performed read‑only; no source files were modified.*
