# Engineering Excellence Program — Control Plane

**Charter:** `DIRECTIVE.md` (verbatim owner directive, 2026-11-26). Read §0 and §4
before any package work. This file is the concise index; details live in the
linked files. One status page, no monolithic log.

- **Baseline:** `main` @ `5b9316742b2f9fe4d0fca2a9743275f91a2ff6aa` = tag
  `pre-excellence-5b93167` (tag object `4ca7846f6a5453da0afe2e82d204246cbf6a7180`,
  verified on origin by `git ls-remote` — see EVIDENCE.md E-1).
- **Integration branch:** `refactor/engineering-excellence` (from the tagged
  commit, pushed). Lane branches `ee/<package>` merge into it under the
  orchestrator's review; **main is never touched, no merges into main until the
  owner's explicit final GO**.
- **Program law:** the repo's standing discipline governs where the charter is
  silent — `docs/AUTONOMOUS-CYCLE-PROTOCOL.md` (never merge, never weaken a
  pin, run local gates before push, one package per branch, no unsafe without
  proof, determinism guarded) plus `docs/CURRENT-STATUS.md`'s update rule.
  Charter §1 grants exactly one exception: the baseline preservation tag.

## The eight rubrics (each ≥ 9/10, independently reviewed — charter §9)

cleanliness · modularity · scalability · maintainability ·
optimality/efficiency/performance · lightweight · speed · documentation

Scoring is a substantiated engineering judgment by independent reviewers with
source evidence; the orchestrator recommends, never self-certifies. Baseline
scores are NOT yet assessed — P0 establishes them.

## Package DAG (charter §7; status lives here)

```
P0 baseline/census/measurement ──┬──> P1 direct improvements ──┐
                                 ├──> P2 simulation/spatial ────┤
                                 ├──> P3 persistence/recovery ──┼──> P6 cleanliness/API/docs (incremental, alongside)
                                 └──> P4 renderer separation ───┤
                                          └──> P5 GPU/anim/mem ┘──> P7 integrated acceptance + independent audit
```

| Package | Scope (charter IDs) | Status |
|---|---|---|
| P0 | tag/branch done; census, coverage ledger, baseline measurements, architecture map | IN-PROGRESS (auditors dispatched, first lanes started) |
| P1 | A16 A17 A21 A25 B05 B07 B08 B09 B04(partial) B03(obvious) | IN-PROGRESS (A16/A17/B09 lanes dispatched) |
| P2 | A05 A07–A10 A22 A24 B01 B06 B12 | NOT STARTED (waits on P0 audit) |
| P3 | A13 A22 B02 B03 B04 B12 B13 | NOT STARTED (waits on P0 audit) |
| P4 | A01 A03–A06 A11 A12 A14 A15 B14 B15 | NOT STARTED (waits on P0 audit) |
| P5 | A11 A17–A20 A23 B14 | NOT STARTED (waits on P4 interfaces) |
| P6 | A01–A06 A12–A15 B15 B16 | INCREMENTAL (runs alongside) |
| P7 | full re-audit, rubric scoring, final report + PR | NOT STARTED |

## File map

| File | Purpose |
|---|---|
| `DIRECTIVE.md` | the charter, verbatim |
| `FINDINGS.md` | the finding register (A01–A25, B01–B16, NEW-*), stable IDs + states |
| `EVIDENCE.md` | evidence manifest: environment, commands, artifacts, hashes |
| `DECISIONS.md` | program-level accepted decisions (D-EE-*) |
| `AGENTS.md` | session/agent registry: role, model, base, worktree, status |
| `briefs/` | per-package implementation briefs (each lane's contract) |
| `audits/` | independent audit reports (per area), committed as program evidence |

## Standing constraints (every lane brief inherits)

1. Determinism: fingerprints/digests byte-identical unless a package brief
   explicitly defines a migration; libm-free fixed-order arithmetic on
   byte-packed paths stays libm-free.
2. No test weakening; count pins corrected only from measured actuals, batched
   at integration, never silently.
3. Lane owns its listed files only; anything adjacent found wrong gets a
   FINDINGS.md entry, not a drive-by fix.
4. Local gates before push: `cargo fmt --all -- --check`, clippy
   `-D warnings` on touched crates, targeted tests per brief; full workspace
   gates run by the orchestrator at integration points.
5. Evidence: every claim carries command + exit status + tree SHA. Rig
   (RTX 5070 Ti) numbers are the owner's; lavapipe/CI correctness only.
6. Commit messages: long, lowercase, evidence-carrying (house style — read
   `git log` before writing).

## Next actions

1. Five read-only area auditors (core / persist / renderer+shaders / shell /
   slice) → `audits/*.md`, seed FINDINGS.md states from measured current source.
2. Three P1 implementation lanes (A16, A17, B09) in disjoint worktrees.
3. Baseline measurement battery (P0) on the integration tree after the first
   full build (target dir is fresh — first build ~10–20 min).
4. Integration + full local gate run when lanes land; FINDINGS/AGENTS updated.
