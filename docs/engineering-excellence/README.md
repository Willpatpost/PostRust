# Engineering Excellence Program — Control Plane

**Charter:** `DIRECTIVE.md` (verbatim owner directive, 2026-11-26). Read §0 and §4
before any package work. This file is the concise index; details live in the
linked files. One status page, no monolithic log.

**AUTHORITATIVE STATUS (the Q11 reconciliation, 2026-11-28):** this README is
the ONE authoritative current-status representation of the program against the
actual tree — what has landed (with SHAs), what is in flight (with lane names),
and the next-actions queue. `FINDINGS.md` is the detail record (per-finding
states, owners, evidence); `AGENTS.md` is the lane registry; `EVIDENCE.md` is
the measurement manifest. Cross-reference discipline: every integration commit
updates the register rows for its lanes AND this page's program-state +
next-actions sections together — a landing that updates one and not the other
is incomplete, and on any drift this page governs until the correction lands.
Every state claim below carries its SHA; re-verify against
`git log --oneline` and `git worktree list` when in doubt.

- **Baseline:** `main` @ `5b9316742b2f9fe4d0fca2a9743275f91a2ff6aa` = tag
  `pre-excellence-5b93167` (tag object `4ca7846f6a5453da0afe2e82d204246cbf6a7180`,
  verified on origin).
- **Integration branch:** `refactor/engineering-excellence` (from the tagged
  commit, pushed, synced with origin at `1d4ecc9`). Lane branches `ee/<package>`
  merge into it under the orchestrator's review; **main is never touched, no
  merges into main until the owner's explicit final GO**.
- **Program law:** the repo's standing discipline governs where the charter is
  silent — `docs/AUTONOMOUS-CYCLE-PROTOCOL.md` plus `docs/CURRENT-STATUS.md`'s
  update rule (which carries a branch notice pointing readers here).
  Charter §1 grants exactly one exception: the baseline preservation tag.
- **The re-audit record (the governing continuation mandate):**
  - The Sol re-audit (2026-11-27, registered at `600bff2`): the program is NOT
    complete — scores 6.0–8.3 against the 9/10 bar; the verified improvements
    retained as intermediate progress; findings R01–R07 registered and
    adjudicated (R05 landed `e2aa89e`, R06 closed-by-S6 `7e4c8f8`, R07
    accepted-investigate).
  - **The owner's independent re-audit at `083f617` + the D-EE-10 authorization
    block (2026-11-27/28, registered `1d4ecc9`) is the current governing
    mandate:** substantial but NOT acceptance — scalability NOT code-complete.
    Scores 8.0/8.1/7.2/7.7/8.1/8.4/7.8-provisional/7.6. Eleven new findings
    Q01–Q11 registered; A08 regional scheduler locality, R07, B08/B13 and the
    coverage/performance obligations retained. Four pending decisions ruled:
    the P5 resource architecture (conditionally approved), the A06 animation
    crate extraction (APPROVED, sequenced after P5 R1/R2 in the P6 window),
    the SDL exact pin (APPROVED), SQLite feature isolation (APPROVED,
    default-on) — plus the rendering-contract/types surface investigation
    authorized. Continue autonomously; every category ≥ 9/10; stop only at the
    final owner integration gate.

## The eight rubrics (each ≥ 9/10, independently reviewed — charter §9)

cleanliness · modularity · scalability · maintainability ·
optimality/efficiency/performance · lightweight · speed · documentation

Scoring is a substantiated engineering judgment by independent reviewers with
source evidence; the orchestrator recommends, never self-certifies. Scored
twice so far, neither at the bar: the Sol re-audit 6.0–8.3 (2026-11-27), the
owner re-audit 8.0/8.1/7.2/7.7/8.1/8.4/7.8-provisional/7.6 (2026-11-27/28) —
scalability (7.2) the furthest from the bar, the workload-proportional
objective the largest outstanding item.

## Package DAG (charter §7; status lives here)

```
P0 baseline/census/measurement ──┬──> P1 direct improvements ──┐
                                  ├──> P2 simulation/spatial ────┤
                                  ├──> P3 persistence/recovery ──┼──> P6 cleanliness/API/docs (incremental, alongside)
                                  └──> P4 renderer separation ───┤
                                           └──> P5 GPU/anim/mem ┘──> P7 integrated acceptance + independent audit
```

The measured crate graph (workspace members per manifests; unique external
normal deps per the B15 closure matrix, `FOOTPRINT.md` / E-11):

```
engine-core          0 external deps (the zero-registry-dep spine)
├─ engine-persist    core + 19 external
├─ engine-renderer   core + 15 external
├─ engine-shell      core + 5 external
└─ engine-slice      all four crates + 35 external
```

Post-split structure (the A01/S4/L9 landings): `engine-renderer/src/vulkan.rs`
is now a root facade over `vulkan/` (7 module files + the `passes/`
directory); `engine-persist/src` carries `local/`, `sqlite/`, `reclaim/`
submodule trees under their root facades — pre-split "monolith" file
references in older records describe the baseline tree, not this one.

| Package | Scope (charter IDs) | Status at head `1d4ecc9` |
|---|---|---|
| P0 | tag/branch, census, coverage ledger, baseline measurements, architecture map | **COMPLETE** — tag/branch E-1; all five crate audits landed (`audits/*.md`, incl. the core retry after the MiMo wedge); census E-2/E-5 superseded by the IA-13 single-convention table; first gate baseline E-6; measurement battery E-8 (the "before" numbers); the owner's maximal-composition walk E-9; the B15 footprint E-11; the P5 census harness E-12 |
| P1 | A16 A17 A21 A25 B05 B07 B08 B09 B04(partial) | **8/9 landed** — A16 `f9901f3`, A17 `3439633`, B09 `9dce061`+corrective, B04 `e31dc61`+corrective, A21 `a213563`, A25 `7c30aee`, B05 `1815a2d`→`bceb3fa`; B07 RESOLVED no-change (design evidence); B08 deferred to measurement (retained by the owner re-audit) |
| P2 | A05 A07–A10 A22 A24 B01 B06 B12 | **all lanes landed** — design `c7de072`; L1 tie `356ef92`, L2 bulk-partition `e1031e5`, L3 housekeeping `34975f5`, L5 StaticGrid `11cf669`, L6 census grid `40e0828`/`7af1f54`, L4 witness index `c79a5c4`, B06 combat broad phase `d489b7f`, L7 A22-core `4dd78e3`, L8 A05/B12 world-config `1396ba6`, A24 `1884582` rig-verified. Remaining: Q01/Q02/Q09 (lane RUNNING), Q03/Q04/Q05/Q07, B01's bounded-work contract |
| P3 | A13 A22 B02 B03 B04 B12 B13 | **core lanes landed; L9b in flight** — design `bceb3fa`; L1 `19fe59d`, L2 `c084230`, L3 `8636d89`, L4 `f437bd7`, L5 `79b2fde`, L6 A22-persist `42ac117`, L7 B13 `3057f66`, L8 B03-Arc `23609bd`, L9 module split `622623b`; L9b walk unification RUNNING (lane commits `2ecadf5`+`59aec7e`, worktree b09). Remaining: Q08, R07 (investigation), the B13 release-rig confirmation, the SQLite feature gate (D-EE-10 §4 — approved, not started) |
| P4 | A01 A03–A06 A11 A12 A14 A15 B14 B15 | **migration complete (S1–S6)** — S1 `954f843`, S2 `f8f308c`, S3 `70b876d`, S4 `46f29e3`, S5 `585187d`, S6 `7e4c8f8` + rig corrective `a6e5adc` (R06 closed); A04 `9e17516` (device bar `df508da`); A02-core `72c91d3`; the A06 decision record `064f866` (extraction APPROVED per D-EE-10 §2); B15 measured `72d636b`. Remaining: A14 (register row restored — resource ownership/device-lost), R01/R02, Q06/Q10, A02's renderer half |
| P5 | A11 A17–A20 A23 B14 | **in flight** — design LANDED `ab7cf2a` (`designs/P5-resource-residency.md`); R0 census harness `1ea3e05` (E-12: the repack matrix, the attach census — 794,624 B staging tails = 50.0% of texture residency, the ripple rows); R1 arenas RUNNING (worktree a16); R4 ring localization RUNNING (worktree a17); R2 dirty-generation, R3 staging, R5 the gated aliasing decision queued; D-EE-10 §1 approved the architecture |
| P6 | A01–A06 A12–A15 B15 B16 | **incremental** — A02-core landed; B10/B11 `63b01d4`; the LOW doc notes registered (NEW-REND-04/05/06, NEW-SLICE-02/03/05); the A06 extraction window queued (after P5 R1/R2); the SDL exact-pin lane RUNNING (NEW-REND-11, worktree q1) |
| P7 | full re-audit, rubric scoring, final report + PR | NOT STARTED — only after every category independently reaches the bar |

## Current tree facts (the measured state at head `1d4ecc9`)

- **Census (IA-13 single convention, nonempty lines; supersedes E-2/E-5):**
  production ≈ 126,483 lines; test material ≈ 134,308 inline + 71,027
  tests-dir = 205,335. Per crate (src prod / src inline-test / tests dir):
  core 13,751/16,085/4,471 · persist 45,831/39,754/8,684 · renderer
  22,320/6,970/0 · shell 1,784/672/498 · slice 42,797/6,478/57,374. Shader
  inputs 19 (15 WGSL + 4 GLSL).
- **CI pins (`.github/workflows/engine-ci.yml`, corrected from measured
  actuals at each integration per D-EE-06):** main shard 1529 Linux / 1478
  Windows passed / 2 ignored / 33 result lines; renderer release 190 passed /
  2 result lines; heavy shard (engine-slice) 674 Linux / 673 Windows / 46
  ignored / 64 result lines. All eight rig gate sets green on the RTX 5070 Ti
  at `df508da` (render_post 7/7, render_instanced 8/8, render_shadows 4/4,
  render_reflection 7/7, render_weather 11/11, render_garage 1/1,
  render_corner 1/1, render_sky 1/1).
- **Footprint (B15, `FOOTPRINT.md` / E-11):** slice exe 16.4 MiB debug /
  10.5 MiB release (−36%); cold full-workspace release build 48 s; minimal
  consumers on fresh locks: core 2.9 s / 2 crates, persist 9.0 s / 26, shell
  44.5 s / 15 (SDL static 38.4 s of it), renderer 41.9 s / 54, slice 52.6 s /
  71 — all five through public API only.
- **In flight right now (the live worktrees):** `ee/p5-r1-arenas-budgets`
  (worktree a16), `ee/p5-r4-ring-localization` (a17),
  `ee/p3-l9b-walk-unification` (b09), `ee/shell-sdl-exact-pin` (q1),
  `ee/core-q01-q02-q09-spatial` (q2), `ee/docs-q11-status-reconciliation`
  (q3 — this lane).

## Next actions (the continuation mandate, reconciled with the register)

1. RUNNING: the six live lanes above — P5 R1 (arenas), P5 R4 (ring
   localization), P3-L9b (the walk unification), the SDL exact pin
   (NEW-REND-11 / D-EE-10 §3), Q01+Q02+Q09 (the spatial triad), and this
   Q11 documentation reconciliation.
2. P5 queued: R2 (the A18 dirty-generation + pose dedup), R3 (the A20 staging
   pool — retiring the measured 794,624 B per-texture tails), R5 (the B14
   aliasing decision, gated on the census + the owner's D-EE-10 §1 criteria).
3. The owner re-audit's Q-findings not yet in flight: Q03 (the scheduler
   housekeeping bounded-work contract), Q04 (production adoption of the
   indexed paths at composition boundaries), Q05 (compaction consequence
   eligibility), Q06 (scene generalization — the PASS_FAMILY boundary and
   beyond), Q07 (configurable npc-world decode admission), Q08 (proportional
   active working sets + authenticated cold historical retrieval), Q10
   (scene-attach failure atomicity).
4. R01-R04 LANDED long ago (the admission invariants `343824b`/`3a426b9`, the accumulator and presentation coherence `0462cfe`/`bb38504`) - the Q11 lane mis-diagnosed these as never-dispatched (the register rows carried stale pre-landing dispositions); the git history corrected it, the rows now carry their SHAs.

   earlier "RUNNING" claim here was wrong; queued).
5. Retained obligations: A08's regional scheduler locality, R07 (the memory
   transaction lifecycle consolidation investigation), B08 (input churn,
   measurement-gated), B13's release-rig confirmation of the
   deferred-confirmation draft, A14 (renderer resource ownership /
   device-lost — register row restored this pass).
6. The approved-but-not-started owner decisions: the A06 extraction window
   (after P5 R1/R2, in P6), the SQLite feature gate (D-EE-10 §4, default-on),
   the rendering-contract/types surface investigation (D-EE-10 §5).
7. P7 only after every category independently reaches the bar.

## File map

| File | Purpose |
|---|---|
| `DIRECTIVE.md` | the charter, verbatim |
| `FINDINGS.md` | the finding register (A01–A25, B01–B16, R*, IA*, Q01–Q11, NEW-*), stable IDs + states — the detail record behind this page |
| `EVIDENCE.md` | evidence manifest: environment, commands, artifacts, hashes (E-1..E-12) |
| `DECISIONS.md` | program-level accepted decisions (D-EE-01..D-EE-10) |
| `DECISION-A06-animation-crate.md` | the A06 placement adjudication (owner-ratification vehicle) |
| `FOOTPRINT.md` | the B15 workspace footprint measurement (closure matrix, sizes, consumers) |
| `INDEPENDENT-AUDIT-be910bf.md` | the astra independent audit (the IA findings) |
| `AGENTS.md` | session/agent registry: role, model, base, worktree, status |
| `briefs/` | per-package implementation briefs (each lane's contract) |
| `designs/` | the architecture decision records: B05, B07, P2, P3, P4, P5 |
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
