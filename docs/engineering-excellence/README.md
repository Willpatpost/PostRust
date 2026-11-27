# Engineering Excellence Program — Control Plane

**Charter:** `DIRECTIVE.md` (verbatim owner directive, 2026-11-26). Read §0 and §4
before any package work. This file is the concise index; details live in the
linked files. One status page, no monolithic log.

- **Baseline:** `main` @ `5b9316742b2f9fe4d0fca2a9743275f91a2ff6aa` = tag
  `pre-excellence-5b93167` (tag object `4ca7846f6a5453da0afe2e82d204246cbf6a7180`,
  verified on origin).
- **Integration branch:** `refactor/engineering-excellence` (from the tagged
  commit, pushed). Lane branches `ee/<package>` merge into it under the
  orchestrator's review; **main is never touched, no merges into main until
  the owner's explicit final GO**.
- **Program law:** the repo's standing discipline governs where the charter is
  silent — `docs/AUTONOMOUS-CYCLE-PROTOCOL.md` plus `docs/CURRENT-STATUS.md`'s
  update rule (which carries a branch notice pointing readers here).
  Charter §1 grants exactly one exception: the baseline preservation tag.
- **THE SOL RE-AUDIT (2026-11-27, `600bff2`'s FINDINGS section) is the
  governing continuation mandate:** the program is NOT complete (scores
  6.0–8.3 against the 9/10 bar); the verified improvements are retained as
  intermediate progress; the outstanding work is P2 spatial, P3 remaining,
  P4 migration S4–S6, P5 GPU/resource scalability, P6 alongside; the
  largest objective is workload-proportional engine work and resident
  memory. No merge until the full directive + independent lead + owner
  accept.

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
| P0 | tag/branch done; census, coverage ledger, baseline measurements, architecture map | IN-PROGRESS — 4/5 area audits landed+verified (renderer, persist, slice, shell pending… core pending); census E-2/E-5 done; first gate baseline E-6 done; measurement matrix NOT STARTED |
| P1 | A16 A17 A21 A25 B05 B07 B08 B09 B04(partial) B03(obvious) | 4 DONE integrated+gated (A16 `f9901f3`, A17 `3439633`, B09 `9dce061`+corrective, B04 `e31dc61`+corrective); A21/A25/B07/B08/B05 awaiting the core/shell audit reads |
| P2 | A05 A07–A10 A22 A24 B01 B06 B12 | design LANDED (`designs/P2-simulation-spatial.md`); L1 (B06 tie) FIXED-AWAITING-REVIEW `356ef92`; L2/L3 spine lanes queued (orchestrator); L4–L6 lane-able; L7/L8 policy/config |
| P3 | A13 A22 B02 B03 B04 B12 B13 | design LANDED (`designs/P3-persistence-architecture.md`); L1/L2 FIXED-AWAITING-REVIEW (`19fe59d`/`c084230`); L3 RUNNING (glm lane); L4–L7 orchestrator; L8/L9 queued |
| P4 | A01 A03–A06 A11 A12 A14 A15 B14 B15 | design LANDED (`designs/P4-renderer-architecture.md`, the submission contract + S1–S6 DAG); S1 RUNNING (glm lane, additive-only); S2–S6 queued |
| P5 | A11 A17–A20 A23 B14 | NOT STARTED (waits on P4 interfaces) |
| P6 | A01–A06 A12–A15 B15 B16 | INCREMENTAL (LOW doc notes registered: NEW-REND-04/05/06, NEW-SLICE-02/03/05) |
| P7 | full re-audit, rubric scoring, final report + PR | NOT STARTED |

## Next actions (the continuation mandate)

1. RUNNING: the R-scenes (R01+R02 admission invariants), R-timing (R03+R04
   accumulator arithmetic + presentation coherence), and P2-L5 (the static
   broad phase — the re-audit's highest priority) lanes.
2. P2 remaining: L6 (the census grid — A07's composition-layer index), L3
   (B01's housekeeping accounting), L7 (A22-core ledger policy), L8 (A05/B12
   the world-configuration boundary with the old-save story).
3. P4 migration: S4 (the vulkan.rs split), S5 (shader composition), S6 (the
   adapter/shim deletion + the second scene — closing R06's transitional
   constraints).
4. P5: the resource-residency/dirty-range/batched-upload work on P4's
   interfaces (A11/A18/A19/A20/A23/B14).
5. P3 remaining: L6–L9 (A22 watermark, B13 instrumentation, the Arc backing,
   the A13 module split — R07's consolidation investigation feeds L9).
6. The measurement-methodology items (B10/B11/B12) and R05's bounded
   interval retention.
7. P7 only after every category independently reaches the bar.

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

## File map
