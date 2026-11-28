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
  commit, pushed, synced with origin at `9690284` — the T01 CI repair; this
  reconciliation lands next). Lane branches `ee/<package>`
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
    block (2026-11-27/28, registered `1d4ecc9` — BINDING):** four decisions
    ruled (the P5 resource architecture conditionally approved; the A06
    animation crate extraction APPROVED, sequenced after P5 R1/R2 — both
    landed, the window open; the SDL exact pin APPROVED — landed `6b9fa2a`;
    SQLite feature isolation APPROVED default-on — landed `3a419a7`) plus the
    rendering-contract/types surface investigation authorized. Continue
    autonomously; every category ≥ 9/10; stop only at the final owner
    integration gate.
  - **The Strict Audit IV (2026-11-28, `audits/strict-audit-IV-096f8d9.md`,
    the owner-delivered comprehensive report at `096f8d9`) is the current
    governing campaign mandate:** NOT ACCEPTED — provisional scores 8.0/8.2/
    7.3/7.6/8.2/8.6/7.8/7.3, every category below the 9/10 bar; 40 findings
    T01–T40 registered with the five-wave execution plan (§9), the mandatory
    acceptance-measurement matrix (§7) and the source-coverage ledger (§8);
    the overnight directive (`OVERNIGHT-DIRECTIVE.md`) is its execution brief.
    Wave 0 (T01 CI repair + this T38 reconciliation) executing 2026-11-28 in
    the ODU orchestrator session.

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

| Package | Scope (charter IDs) | Status at head `9690284` |
|---|---|---|
| P0 | tag/branch, census, coverage ledger, baseline measurements, architecture map | **COMPLETE** — tag/branch E-1; all five crate audits landed (`audits/*.md`, incl. the core retry after the MiMo wedge); census E-2/E-5 superseded by the IA-13 single-convention table; first gate baseline E-6; measurement battery E-8 (the "before" numbers); the owner's maximal-composition walk E-9; the B15 footprint E-11; the P5 census harness E-12 |
| P1 | A16 A17 A21 A25 B05 B07 B08 B09 B04(partial) | **COMPLETE (all nine dispositioned)** — A16 `f9901f3`, A17 `3439633`, B09 `9dce061`+corrective, B04 `e31dc61`+corrective, A21 `a213563`, A25 `7c30aee`, B05 `1815a2d`→`bceb3fa`, B07 RESOLVED no-change (design evidence), B08 `6cf1a0d` (the codex lane) |
| P2 | A05 A07–A10 A22 A24 B01 B06 B12 | **all lanes landed** — design `c7de072`; L1 tie `356ef92`, L2 bulk-partition `e1031e5`, L3 housekeeping `34975f5`, L5 StaticGrid `11cf669`, L6 census grid `40e0828`/`7af1f54`, L4 witness index `c79a5c4`, B06 combat broad phase `d489b7f`, L7 A22-core `4dd78e3`, L8 A05/B12 world-config `1396ba6`, A24 `1884582` rig-verified, Q01+Q02+Q09 `bce11d3` (lane `e4d5c38`). Remaining per strict audit IV: T08/T09 (B01's bounded-work contract + queued locality), T10/T11/T37 |
| P3 | A13 A22 B02 B03 B04 B12 B13 | **all core lanes landed (L9b complete)** — design `bceb3fa`; L1 `19fe59d`, L2 `c084230`, L3 `8636d89`, L4 `f437bd7`, L5 `79b2fde`, L6 A22-persist `42ac117`, L7 B13 `3057f66`, L8 B03-Arc `23609bd`, L9 module split `622623b`, L9b walk unification `10d3d13`; the SQLite feature gate LANDED `3a419a7` (D-EE-10 §4). Remaining per strict audit IV: T13/T14 (Q08), T29/T30, T31, T28 |
| P4 | A01 A03–A06 A11 A12 A14 A15 B14 B15 | **migration complete (S1–S6)** — S1 `954f843`, S2 `f8f308c`, S3 `70b876d`, S4 `46f29e3`, S5 `585187d`, S6 `7e4c8f8` + rig corrective `a6e5adc` (R06 closed); A04 `9e17516` (device bar `df508da`); A02-core `72c91d3`; the A06 decision record `064f866`; B15 measured `72d636b`; CA-05/CA-06 `838a304`, CA-12..15 `a45a60e`, CA-01/10/11 `483de34`, R01/R02 `3a426b9`. Remaining per strict audit IV: T02/T03/T05/T06/T16/T22 (the renderer lifecycle team), T12 (Q06), A02's renderer half |
| P5 | A11 A17–A20 A23 B14 | **COMPLETE (R0–R5)** — design `ab7cf2a`; R0 census `1ea3e05` (E-12), R1 arenas `4d77c69`, R2 dirty-generation `0b1a60e`+correctives `145391c`/`0a72854`, R3 staging `59eac30`, R4 ring `d6d7254`, R5 aliasing `624488a` (night close `78b0664`, the renderer-release/main pins measured); E-15..E-18. Remaining per strict audit IV: T15/T17/T18/T23/T24/T25/T26 |
| P6 | A01–A06 A12–A15 B15 B16 | **incremental** — A02-core landed; B10/B11 `63b01d4`; the SDL exact pin LANDED `6b9fa2a` (NEW-REND-11); the SQLite feature gate landed `3a419a7`; the A06 extraction IN FLIGHT (T19, branch `ee/overnight-t19-animation`, base `9690284`); T32 (render-types footprint) follows T19 |
| P7 | full re-audit, rubric scoring, final report + PR | NOT STARTED — only after every category independently reaches the bar |

## Current tree facts (the measured state; census at `1d4ecc9`, pins at `9690284`)

- **Census (IA-13 single convention, nonempty lines; supersedes E-2/E-5):**
  production ≈ 126,483 lines; test material ≈ 134,308 inline + 71,027
  tests-dir = 205,335. Per crate (src prod / src inline-test / tests dir):
  core 13,751/16,085/4,471 · persist 45,831/39,754/8,684 · renderer
  22,320/6,970/0 · shell 1,784/672/498 · slice 42,797/6,478/57,374. Shader
  inputs 19 (15 WGSL + 4 GLSL).
- **CI pins (`.github/workflows/engine-ci.yml`, corrected from measured
  actuals at each integration per D-EE-06; the T01 repair `9690284`
  centralized them as per-shard expectation variables):** main shard
  1629 Linux / 1578 Windows passed / 2 ignored / 34 result lines; renderer
  release 232 passed / 2 result lines; heavy shard (engine-slice) 720
  Linux / 720 Windows / 60 ignored / 72 result lines (platform-uniform,
  dual-platform CI-measured; the census: 68 targets + 4 crash-seams child
  lines). The count-gate self-test (`scripts/ci-count-gate-selftest.sh`)
  runs in the main matrix job on every push. All eight rig gate sets green
  on the RTX 5070 Ti at `df508da` (render_post 7/7, render_instanced 8/8,
  render_shadows 4/4, render_reflection 7/7, render_weather 11/11,
  render_garage 1/1, render_corner 1/1, render_sky 1/1).
- **Footprint (B15, `FOOTPRINT.md` / E-11):** slice exe 16.4 MiB debug /
  10.5 MiB release (−36%); cold full-workspace release build 48 s; minimal
  consumers on fresh locks: core 2.9 s / 2 crates, persist 9.0 s / 26, shell
  44.5 s / 15 (SDL static 38.4 s of it), renderer 41.9 s / 54, slice 52.6 s /
  71 — all five through public API only.
- **In flight right now (the live lanes, ODU orchestrator session
  2026-11-28):** `ee/overnight-t19-animation` (worktree t19 — the A06
  E-S1 extraction, subordinate agent), this T38 reconciliation
  (`ee/overnight-t38-register`), and the T01 CI verification run
  `46127751223` at exact head `9690284` (in progress). All prior
  "RUNNING" registry rows were stale — those six lanes landed
  (`4d77c69`, `d6d7254`, `10d3d13`, `6b9fa2a`, `bce11d3`, `0439894`).

## Next actions (the strict-audit-IV wave plan, reconciled with the register)

1. **Wave 0 (executing):** T01's exact-head CI verification run
   `46127751223` at `9690284` — all five jobs green closes the gate;
   T38 this reconciliation pass; the durable overnight log
   (`OVERNIGHT-LOG.md`) now maintained per landing.
2. **Wave 1 (next, CPU-verifiable first):** T19 (the A06 extraction,
   IN FLIGHT); T07 (the retention safe moving cut — the S0 headline,
   spine.rs); T04 (complete-primitive admission); T05 (generation
   identity contracts); T21 (decode transients); then the renderer
   lifecycle team T02/T03/T06/T16 as ONE coordinated stream.
3. **Wave 2:** T08/T09 (scheduler bounding + queued locality — the
   spine trio serialized with T07), T10/T11/T37 (spatial), T13/T14/T29/T30
   (persistence residency).
4. **Wave 3:** T12 (scene generalization stages), T15/T17/T18/T20,
   T32 (render types, after T19), T24/T25/T26.
5. **Wave 4:** T23/T27/T28/T31/T33/T34/T35/T36, T39's measurement
   discipline applied throughout, T40's coverage ledger.
6. **Wave 5 / P7:** freeze, exact-tree measurement, independent audit —
   only after every category independently reaches the bar. No main
   merge before the owner's explicit GO.

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
| `OVERNIGHT-LOG.md` | the ODU orchestrator's durable engineering log (per-landing records, STATE AT DAWN handoffs) |
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
