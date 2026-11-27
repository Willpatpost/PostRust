# P1-A16 — Ordered confirmation matching in `engine-persist`

**Package:** P1-A16 · **Lane branch:** `ee/p1-a16-ordered-confirm` ·
**Base:** the `refactor/engineering-excellence` head at worktree creation.
**Role:** focused Rust implementation. **One responsibility:** replace the
quadratic key matching in `CommitPipeline::confirm` with a single ordered
merge — nothing else.

## Problem (verified on the baseline tree, 2026-11-26)

`crates/engine-persist/src/pipeline.rs`, `fn confirm` (~line 1383): for every
ordered attempted key it runs `batch.entries.iter().find(|entry| entry.id == *id)`
(line ~1394). With canonical batch entries sorted and unique by decode (~line
209) and up to 65,536 entries per batch, confirmation is O(N²) in comparisons
on the hot persistence path.

## Required outcome

- Single ordered merge/zip (or justified equivalent) between `attempt.keys`
  (ordered — verify the actual container and its ordering) and
  `batch.entries` (sorted, unique — **verify the decode-side guarantee in
  source before relying on it**; if decode does not enforce sorted+unique,
  enforce/verify it explicitly in the merge with the same failure semantics).
- **Identical verification semantics, zero weakening:** same error variants,
  same error detail strings, same per-key check order (key-missing → payload
  mismatch → key-not-chain-included → key-confirmed-by-different-revision),
  same len-equality precondition, same "no unattempted keys" conclusion
  (equal lengths + uniqueness + every attempted key found).
- Complexity: O(N) key matching (remaining `self.confirmed.get(id)` map
  lookups may stay logarithmic — they are not this package's target).
- No public API change. No behavior change observable to callers. No new
  dependencies.

## Constraints

- Allowed edit paths: `crates/engine-persist/src/pipeline.rs` (and its inline
  test module). If a test helper must move to keep tests intact, keep it
  inside engine-persist. Forbidden: every other file, the workflow, docs
  outside `docs/engineering-excellence/`.
- Determinism: this is verification logic, not fingerprint arithmetic; all
  digests/pins must stay byte-identical (run the persist battery).
- No test weakening. If your change legitimately adds/renames a test, report
  the exact count delta — do not touch CI pins (the orchestrator batches pin
  corrections).
- Read `git log --oneline -10` for the house commit style: long, lowercase,
  evidence-carrying messages.

## Acceptance

1. `cargo test -p engine-persist` green (full crate, all features the crate's
   default + `composite-test-fixture` feature as used by the main shard:
   `cargo test -p engine-persist --features composite-test-fixture`).
2. `cargo fmt --all -- --check` clean; `cargo clippy -p engine-persist
   --all-targets -- -D warnings` clean.
3. A complexity statement in the commit message (before/after) + the invariant
   argument (why the merge preserves the exact error semantics).
4. A micro-evidence note: a batch-size sweep timing the confirm path before
   vs after (e.g. N ∈ {1k, 16k, 65k} synthetic ordered keys, release build)
   with command + numbers in the commit message. Keep it honest — if the
   existing test fixtures make this impractical, say so and give the
   complexity argument only.

## Handoff (required, structured)

Exact commit SHA + base; changed files; the invariant argument; commands run
with exit status; test results; risks; unresolved issues. Report unresolved
issues, not just successes. Do NOT push, do NOT merge, do NOT touch main.
Commit on your lane branch and stop.
