# P1-B04: Flush Lifecycle - Transaction Identity Consolidation

## Brief

**Package:** P1-B04  
**Lane Branch:** `ee/p1-b04-flush-lifecycle`  
**Base SHA:** `ff3978a`  
**Area:** SLICE/PERSIST  
**Finding:** B04 (confirmed)  
**Model:** `qwen3-coder-next` (implementation agent)

## Context

The record-capture lifecycle and the classification-flush lifecycle are structurally asymmetric:

### Record Path (lines 804-1034)
- Stages a new episode for pending records (lines 455-967)
- Records `UnresolvedEpisode` on `capture` failure (journaled=false) or `mark_applied` failure (journaled=true)
- Retry via `complete_unresolved_episode` reuses the same episode ID
- One logical episode = one identity = one journal transaction

### Classification Flush Path (lines 1166-1226)
- Stages a NEW episode for classification sync
- If ANY step fails (stage, capture, mark_applied), there's NO retry mechanism
- A failure leaves the classification decision durable only in the live lane
- On retry, a NEW episode ID is minted → second journal transaction for same sync

## The Problem

**Finding B04 (orchestrator verification, npc_memory_lane.rs):**

> The record path's `mark_applied` failure records `UnresolvedEpisode` (954–967, retry completes the apply, never a second envelope); the classification flush (1166–1226) has NO equivalent — failure after `stage` → retry mints a NEW episode id + a SECOND journal transaction for the same sync (retry-identity inconsistency + write amplification).

**Key defects:**
1. **Retry-identity inconsistency:** Classification flush doesn't record unresolved episodes
2. **Write amplification:** Retries create duplicate journal transactions for same logical sync
3. **Cold-reopen data loss:** Classification decisions made after final record episode are lost on cold reopen

## Design Note

### Shape Chosen: UnresolvedEpisode Extension

**Rationale:** The record path's `UnresolvedEpisode` structure already captures all necessary state:
- `witness`: The witness being flushed
- `episode`: The episode ID (assigned once, reused on retry)
- `consequence`: The full ConsequenceId (region, episode, witness)
- `journaled`: Boolean flag distinguishing unjournaled vs journaled failure

The classification flush needs the same discipline:
- Stage → Capture → MarkApplied sequence
- Record `UnresolvedEpisode` on ANY failure (journaled=false for stage/capture, journaled=true for mark_applied)
- Retry via `complete_unresolved_episode` reuses the same episode ID

### Implementation Plan

**Step 1: Modify `flush_witness_classifications` to record unresolved on failure**

The function now:
- Takes the current classification sync as input
- Records `UnresolvedEpisode` with journaled=false on stage/capture failure
- Records `UnresolvedEpisode` with journaled=true on mark_applied failure
- On success, inserts into `episode_keys` and `journaled_classifications`

**Step 2: Move classification flush to be handled by retry mechanism**

In `commit_witness_captures`:
- The unresolved completion runs BEFORE any new episode is staged
- Classification flush is triggered via the unresolved mechanism
- This ensures consistent retry identity

**Step 3: Handle classification-only unresolved episodes in `complete_unresolved_episode`**

The existing logic already handles both:
- `journaled=false`: Re-stage and recapture (empty records for classification-only)
- `journaled=true`: Apply only (the transaction is already durable)

**Step 4: Recovery support is already in place**

The `recover_memory_lane` already handles classification-sync episodes:
- Empty record set with unit-marker span (lines 1308-1317)
- Decisions applied through the `classify` path

## Requirements

1. **Happy-path byte-identity:** The normal success path must be identical (same episode numbering, same journal transactions)
2. **Failure-path changes:** Only the retry mechanism changes
3. **Test pins:** No test pin movements expected (same behavior on success, same S6 fuzz outcomes)
4. **Format:** No wire or save format changes

## Acceptance Gates

1. `cargo fmt --all -- --check` passes
2. `cargo clippy -p engine-slice --all-targets -- -D warnings` passes
3. `cargo test -p engine-slice --test npc_memory_lane` passes (14 tests)
4. No test pin changes
5. S6 fuzz gate passes with same seed outcomes

## Files to Edit

1. `crates/engine-slice/src/npc_memory_lane.rs`:
   - `flush_witness_classifications`: Add unresolved recording
   - `commit_witness_captures`: Integrate classification flush into retry loop
   - `complete_unresolved_episode`: Handle classification-only episodes

2. `crates/engine-slice/tests/npc_memory_lane.rs`:
   - No changes expected (happy path is byte-identical)

## Risks

- **Low:** The classification flush is a smaller, less-traveled path than record capture
- **Low:** The S6 fuzz gate exercises this path extensively
- **Low:** No format changes, so cold-reopen compatibility is maintained

## Deliverables

1. Commit to `ee/p1-b04-flush-lifecycle` with evidence-carrying message
2. No push, no merge, no PR
3. Handoff with exact commit SHA, changed files, and test results
