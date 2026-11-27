# P1-B04 — Classification-flush retry lifecycle consolidation

**Package:** P1-B04 · **Lane branch:** `ee/p1-b04-flush-lifecycle` ·
**Base:** integration head at worktree setup (`ff3978a` or later — record it).
**Role:** focused Rust implementation with a design-first step. **One
responsibility:** give the classification-sync flush the same
failure/retry identity discipline the record-capture path already has —
nothing else.

## Problem (verified by the orchestrator's own read, 2026-11-26, and by the
charter's B04; the independent auditor's quote was fabricated but the
structural claim survives verification)

`crates/engine-slice/src/npc_memory_lane.rs`:

- **The record path** (`commit_witness_captures`, ~lines 940–967): on
  `mark_applied` failure it records `book.unresolved = Some(UnresolvedEpisode
  { witness, episode, consequence, keys, journaled: true })` — the next call
  runs `complete_unresolved_episode` (~1043) which completes the apply with
  the SAME identity: "the retry completes the apply only (never a second
  envelope)".
- **The classification flush** (`flush_witness_classifications`, ~1166–1226):
  allocates a fresh episode (`book.episodes += 1`), `pipeline.stage(envelope)`
  → `pipeline.capture(plan)` → `pipeline.mark_applied(&[consequence])`, then
  updates `book.episode_keys` / `book.journaled_classifications`. On ANY
  failure it returns Err with NO bookkeeping — so the retry (the flush
  condition at ~980–985 re-fires because `journaled_classifications` was not
  updated) mints a NEW episode id and a SECOND journal transaction for the
  same logical sync. Failure modes:
  - failure after `stage`/`capture` succeeded (transaction durable): the
    first transaction is chain-held but never tracked (`episode_keys` not
    updated), so `confirm_chain_held_episodes` (~1231) can't see it; the
    retry writes a duplicate transaction. Write amplification + untracked
    durable state.
  - failure at `stage` (nothing durable): retry mints a new episode id —
    consumed-id churn only, mostly harmless, but the same asymmetry.
- The flush is guarded by `book.unresolved.is_none()` at ~980 (it skips while
  a record-path unresolved episode exists) — preserve that ordering.

## Required outcome

Route the classification-sync flush through the SAME unresolved-episode
lifecycle used by record capture, so one logical sync keeps one identity
across retries:

1. **Design note first (in the commit message):** how you represent the
   classification-sync episode in `UnresolvedEpisode` — recommended shape: a
   kind marker (record-capture vs classification-sync) OR the existing
   struct with empty `keys` plus the episode/consequence identity, whichever
   requires less machinery; `complete_unresolved_episode` (~1043) already
   handles journaled vs unjournaled halves — extend it (or add a sibling
   completion fn it shares structure with) for the classification variant.
2. On `mark_applied` failure inside the flush: record the unresolved episode
   (journaled: true) so the retry completes the apply with the SAME
   consequence/episode — never a second envelope for the same sync.
3. On `stage`/`capture` failure: record unresolved (journaled: false) so the
   retry re-stages the SAME episode id — with a FRESH plan (classifications
   as of the retry; they may legitimately have changed — that is a new sync
   decision on the same identity slot, document this choice).
4. `book.episode_keys`/`journaled_classifications` updates happen exactly
   where they do today, on the success path.
5. **Semantics preserved:** the happy path is byte-identical (same episode
   numbering, same journal transactions, same plans); ONLY failure-retry
   paths change (same identity instead of a new one). The S6 fuzz gate
   (`tests/npc_memory_lane.rs`, the fault-injecting campaign) is the
   acceptance battery — if its pinned outcomes legitimately shift (e.g.
   fewer duplicate transactions under dropped-ack faults), REPORT the exact
   old/new values with the mechanism explanation; never silently re-pin.

## Constraints

- Allowed edit paths: `crates/engine-slice/src/npc_memory_lane.rs` (+ its
  test file `crates/engine-slice/tests/npc_memory_lane.rs` ONLY for
  documented pin corrections with measured values). Forbidden: engine-core,
  engine-persist, everything else.
- Do NOT change `MemoryCommitBook`'s public surface beyond what the
  lifecycle needs; do not touch the record path's semantics.
- Determinism: happy-path digests/fingerprints/campaign pins must reproduce
  UNCHANGED (run the memory-lane batteries and the fuzz gate). Failure-path
  changes are the point of the package — document them precisely.
- Gates (in your worktree, record commands + exit status): `cargo fmt --all
  -- --check`; `cargo clippy -p engine-slice --all-targets -- -D warnings`;
  `cargo test -p engine-slice --test npc_memory_lane` (the full battery incl.
  the fuzz gate); plus `cargo test -p engine-slice --lib npc_memory` if lib
  tests exist for the module. First build is slow (~10–20 min) — expected.
- Commit style: long, lowercase, evidence-carrying (read `git log --oneline
  -5`). Do NOT push, merge, or touch main.

## Handoff (required, structured)

Exact commit SHA + base; changed files; the design note (which shape you
chose and why); happy-path pin values before/after (must be equal — state
it); failure-path behavior changes with the fault scenarios you traced;
commands + exit status; test results (exact pass counts, any pin corrections
with old/new values + mechanism); risks; unresolved issues; integration
requirements.
