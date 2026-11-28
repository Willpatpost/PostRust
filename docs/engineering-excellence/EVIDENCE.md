# Evidence Manifest

Every entry: what, exact command(s), exit status, tree SHA, artifact path.
Raw large outputs lived outside the source tree at
`C:\Users\Willp\AppData\Local\Temp\opencode\ee\artifacts\` — **cleaned
2026-11-27 per the owner's house-cleaning directive** (with the three
worktrees and 14.4 GiB of build targets); the summarized results, every
command, and every exit status are committed here and in the commit
messages, which stand as the durable record. A re-verification re-runs the
commands below; the raw logs were scratch by design.

## Environment (measured 2026-11-26, session start)

- OS: Windows 11, PowerShell (pwsh). Repo: `C:\Users\Willp\Code\GitHub\PostRust`, clean tree, no submodules, no stashes.
- Toolchain: rustup active `1.99.1-x86_64-pc-windows-msvc` (pinned by `rust-toolchain.toml`); cargo 1.99.1 (5f94df478 2026-08-27), rustc 1.99.1 (58cf8f9c3 2026-10-13).
- CPU: AMD Ryzen 7 9800X3D 8-core. GPU: NVIDIA GeForce RTX 5070 Ti, 16,303 MiB, driver 617.42 (via nvidia-smi; exit 0). Second adapter: AMD Radeon(TM) iGPU.
- RAM: 31.2 GB usable. Disk: C: 3,400.5 GB free, D: 32.0 GB free.
- `target/` freshly cleaned at session start — first full build ~10–20 min.
- WSL Ubuntu-24.04 present (user willp); Docker inside WSL EMPTY (0 images) —
  parity pod not yet rebuilt; lavapipe gates currently unavailable (BLOCKED
  until rebuilt, ~15 min when needed; recipe in CURRENT-STATUS/handoff).

## E-1 Baseline tag + branch (all exit 0, 2026-11-26)

```
git fetch origin main                     -> origin/main = 5b9316742b2f9fe4d0fca2a9743275f91a2ff6aa (exact audited baseline)
git tag -a pre-excellence-5b93167 5b9316742b2f9fe4d0fca2a9743275f91a2ff6aa -m "PostRust baseline before engineering excellence program"
git push origin refs/tags/pre-excellence-5b93167:refs/tags/pre-excellence-5b93167   -> * [new tag]
git ls-remote origin 'refs/tags/pre-excellence-5b93167' 'refs/tags/pre-excellence-5b93167^{}'
  -> 4ca7846f6a5453da0afe2e82d204246cbf6a7180  refs/tags/pre-excellence-5b93167        (tag object)
  -> 5b9316742b2f9fe4d0fca2a9743275f91a2ff6aa  refs/tags/pre-excellence-5b93167^{}   (peeled commit = baseline)
git switch -c refactor/engineering-excellence 5b9316742b2f9fe4d0fca2a9743275f91a2ff6aa
git push -u origin refactor/engineering-excellence             -> * [new branch]
```

Pre-checks: local tags = 4 `archive/*` (local-only); remote tags = NONE;
remote heads = `main` only. No name collisions; nothing moved or deleted.

## E-2 Source census (rough, 2026-11-26, baseline tree)

Whole-file `.rs` line counts via PowerShell `Measure-Object -Line` on
`git ls-files '*.rs'` — **orientation only, NOT production size**: inline
`#[cfg(test)]` modules are counted as production here; the P0 auditors produce
the real prod/test split. Totals: 334 tracked files, 232 `.rs`, 15 shader
sources (`.wgsl`/`.glsl`).

| Crate | files | whole-file lines |
|---|---:|---:|
| engine-core | 40 | ~33,000 |
| engine-persist | 44 | ~92,277 |
| engine-renderer | 32 | ~26,662 |
| engine-shell | 10 | ~2,919 |
| engine-slice | 106 | ~104,634 |

`cargo metadata --no-deps`: 5 workspace members, DAG
core → {persist? no} (actual dependency edges to be mapped by P0 auditors:
slice depends on all; renderer/shell/persist on core per manifests).

## E-3 Model fleet (2026-11-26; labels corrected 2026-11-27 per the audit's IA-14)

`tools.opencode.models` (odu provider) LISTED the twelve directive-named chat
models (a catalog discovery — individual callability is evidenced only by the
lanes that subsequently ran on each model, not by the listing itself):
glm-5.3-w4a8, qwen3-coder-next, gpt-oss-120b, MiMo-V2.6-Pro-RL, qwen36-35b,
qwen38-27b, qwen3-8-flash-next, gpt-oss-20b, gemma-4-31b, gemma-4-12b,
gemma-4-e4b, diffusiongemma-26b.

## E-4 Seed-finding verification (2026-11-26, baseline tree, `git grep` exit 0)

- A16: `crates/engine-persist/src/pipeline.rs:1394` — `batch.entries.iter().find(|entry| entry.id == *id)` inside `for (id, ...) in &attempt.keys`. CONFIRMED.
- B09: `crates/engine-renderer/src/vulkan.rs:1142` — `"VK_KHR_metal_surface"` in `PLATFORM_SURFACE_EXTENSIONS`; correct `VK_EXT_metal_surface` used at :5407,:5424,:5531,:5568,:5630,:5640. CONFIRMED.
- A17: `crates/engine-renderer/src/animation.rs:420` `skin_matrices` recomputes `rest_pose`/`rest_worlds` per call (rest worlds at :429); `crates/engine-slice/src/corner_routes.rs:617` `evaluate_population_poses_locomotion`. CONFIRMED.

## E-5 Production vs test line split (heuristic, 2026-11-26, baseline tree)

PowerShell brace-depth heuristic: lines inside `#[cfg(test)]` blocks in
`src/` files counted as inline-test; `tests|benches|examples/` directories
excluded (counted in E-2). Heuristic limitations: `#[cfg(test)]` on bare fns
still counted (correct for census), brace-in-string edge cases unhandled
(negligible here). Whole-file totals reconcile with E-2.

| Crate | src prod lines | src inline-test lines | tests/ dir lines (E-2) |
|---|---:|---:|---:|
| engine-core | ~14,144 | ~15,932 | ~0 |
| engine-persist | ~46,379 | ~39,946 | ~8,684 |
| engine-renderer | ~21,384 | ~6,312 | ~0 |
| engine-shell | ~1,854 | ~705 | ~498 |
| engine-slice | ~43,976 | ~6,397 | ~56,475 |

**Workspace production ≈ 128k lines; test material ≈ 134,949 lines by the
table's own mixed conventions** (inline + tests/ dirs) — a test-heavy codebase
by design; per-file prod counts feed A01 (split by change reason) and B15
(footprint claims must cite these, not whole-file sizes).

**IA-13 regeneration (2026-11-27, one convention: nonempty lines, current
branch head `0874dee`; E-2/E-5 superseded by this table):**

| Crate | src prod | src inline-test | tests/ dir |
|---|---:|---:|---:|
| engine-core | 13,751 | 16,085 | 4,471 |
| engine-persist | 45,831 | 39,754 | 8,684 |
| engine-renderer | 22,320 | 6,970 | 0 |
| engine-shell | 1,784 | 672 | 498 |
| engine-slice | 42,797 | 6,478 | 57,374 |

**Totals: production ≈ 126,483; test material ≈ 134,308 inline + 71,027
tests-dir = 205,335.** Shader inputs: 15 WGSL + 4 GLSL (`overlay.frag/.vert`,
`scene.frag/.vert` — enumerated from the tracked tree, 19 total). The
earlier E-5's physical-line convention and its mixed totals are superseded;
this table is the single-convention record. (The baseline-tag census differs
only by the program's additions — the +1,467 S2, +569 S3, +1,799 L5, and the
fix-wave lines.)

## E-6 First integration-batch gate run (2026-11-26, Windows rig, tree `3439633`)

Full serial batch on the integrated tree (A16+A17+B09+correctives), log at
`Temp\opencode\ee\artifacts\gates-batch1.log` (exact commands + exit codes):

| Gate | Result | Note |
|---|---|---|
| `cargo fmt --all -- --check` | exit 0 | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 | clean |
| main shard (`--workspace --exclude engine-slice --features engine-persist/composite-test-fixture`) | **1369/2/33, exit 0** | = pin 1368 + 1 (the B09 platform-surface test, platform-uniform pure logic) |
| renderer release (`-p engine-renderer --release`) | **173 passed / 2 result lines, exit 0** | = pin 172 + 1 (same test; the pin's "/2" is RESULT LINES, not ignored — corrected our own earlier misreading) |
| heavy shard (`-p engine-slice --features crash-test-seams`) | **650/46/64, exit 0** | unchanged — heavy carries engine-slice only |

Pin corrections from these measured actuals (workflow lines ~599/722/732):
main 1419→**1420** Linux / 1368→**1369** Windows; renderer release 172→**173**.
Linux values are the platform-uniform inference (+1), to be confirmed by the
candidate's own CI run per the repo's established practice. No test weakened;
the count moved because a corrective ADDED a test.

## E-7 A24 rig verification (2026-11-26, Windows rig, RTX 5070 Ti, tree `1884582`; labels corrected 2026-11-27 per the audit's IA-14)

`cargo test -p engine-slice --test render_weather -- --ignored --nocapture
--test-threads=1` (real GPU, real windows, validation on, no offscreen driver —
the rig discipline): **11 passed / 0 failed, exit 0, 7.29 s**. The gates'
MEASURED observations (the ink counts and term-separation counts as printed);
their ASSERTIONS are acceptance floors (e.g. ink ≥ 200, term separation ≥ its
floor) — the floors are the pinned contract, the printed values the measured
evidence; within-build double-run equality is what the byte-equality gates
prove (no cross-build comparison is claimed by this run). Measured values
from the log: rain ink 17041 differing bytes; broadside ink 3482 px / 2162
upper-third; allocation constancy 12 distinct captures with the
AllocationStats tuple (19/4/1589248/3/7/2); ripple sweep 22/255 at the
recorded ground center; term separation 96548/67535/7625 blend-only and
27149/27149/2538 displacement-only; the composition smokes ok. Log:
`Temp\opencode\ee\artifacts\gates-a24-render-weather-rig.log`.
This is the A24 equivalence-bar proof: the scripted route's weather campaign
reproduces exactly on the real GPU through the explicit-dt plumbing.

## E-8 Baseline measurement battery (2026-11-26, rig CPU 9800X3D, release, tree `263532f`)

Seven harnesses run serially on the integrated tree; raw log (every command,
exit code, and full output) at `Temp\opencode\ee\artifacts\baseline-battery.log`.
These are the "before" numbers the P2/P3/P5 packages are judged against.

**Persistence (Windows rig, LocalFile + SQLite, n=10–20):**
- snapshot serialization (no I/O): median 0.129 ms / p99 0.240
- snapshot publication (append incl. fsync): median 14.46 / p99 19.80 ms (SQLite: 12.53/19.69)
- T1 journal transaction: median 7.78 ms; canonical confirmation 5.97 ms (SQLite 1.78); recovery 6.16 ms (SQLite 6.24)
- store growth ~230,747 B per snapshot; lifetime growth ~31.0 B per execution (~23 exec/tick fixture)
- compaction (150-tick history): 1.16 ms; compacted snapshot 124,968 B vs full 220,438 B (retain 100)
- **PP-B3 retention section PANICS on Windows** — see NEW-SLICE-06 (the tier-gated reclamation-begin legitimately refuses on `ProcessCrashOnly` Windows; the harness unwraps it where local.rs's own tests cfg-gate). PP-B3 baselines remain pod-only.

**Entity-scale (600 ticks × {1k,5k,20k} × {full,mixed,event-driven}, all DOUBLE-RUN IDENTICAL) —**
**IA-12 CORRECTION (2026-11-27, per the independent audit): the 5k and 20k
rows are WITHDRAWN as evidence of continuously-active workloads at those
scales — the harness's `routine_payload_parts` codec rejects identities ≥
`NPC_COUNT` (1000), so only the first 1,000 identities run recurring chains
(the same read-through defect CURRENT-STATUS documented historically; the
archived logs confirm 1,000 pending at all three scales). The measurements
stand under their ACTUAL workload description: 1,000 recurring chains amid
{5k, 20k} initial dispatches and full-population fingerprinting. A
scale-capable versioned benchmark is queued (the sustained-load harness
already uses its own codec and retains its evidentiary basis):**
- 1k: full median 0.019/p99 0.034 · mixed 0.010/0.017 · event-driven 0.002/0.005 ms/tick
- 5k: full 0.019/0.049 · mixed 0.011/0.031 · event-driven 0.002/0.011
- 20k: full 0.019/0.065 · mixed 0.011/0.061 · event-driven 0.002/0.048
- fingerprints recorded per cell in the log (e.g. 1k-full `04b35e00f34836b3`)

**Sustained-load composition (the F-08/F-12 matrix):** D_base 154.1/84.1/13.7
per tick at 1k full/mixed/event-driven; 200.0/200.0/69.2 at 5k (full and mixed
saturate the 200 budget — the documented shape); per-lane backlog slopes in
the log.

**Behavior / locomotion / render / instrumentation:** sections complete in
the log (exit 0 each); the §8.5-class numbers (witness census cost, E-09
crossover, E-08 skip decomposition, per-NPC memory, CPU render path) are
carried there — the E-8 register cites the artifact rather than duplicating
every line.

**B10/B11 slice-measurement methodology corrections (2026-11-27, lane
`ee/slice-b10-b11-methodology`, base `72d636b`):**

- **B10 decision — option (a), the audit's own disposition, with the audit's
  premises adjudicated by direct read first.** The audit's core premise (the
  `synthesis_once` arm "uses `NpcWorld::restore`, reconstructing state from
  save bytes" while "replay replays from the last checkpoint") is FALSE for
  this harness: no restore/save/checkpoint call exists in
  `behavior_measurement.rs` (the dormancy span is quiet — nothing armed, no
  held events; the replay arm re-executes the missed occurrences through the
  real queue, the synthesis arm runs the REAL `evaluate_wake`/`next_occurrence`
  per NPC on freshly constructed routines and arms the live chains on a
  pre-constructed spine). The arms' different work IS the measured quantity —
  which reactivation policy is cheaper at which miss count — so the correction
  is labeling, not re-timing: the activation section now prints a methodology
  block naming each arm's timed window (replay = arm 250·k past-due events +
  the queue drain; synthesis = one evaluate_wake per NPC + the re-arm jumps +
  arming the 250 live chains; no restore-from-save in EITHER arm), beside the
  existing outcome-divergence and post-activation-queue reports.
- **The cadence_hz=0 question: the shipped E-08 skip decomposition was NOT
  produced under irregular cadence.** `--benchmark-behavior` is
  argument-free; the skip-cost cells arm events at authored dues and drain by
  logical `run_ticks` — no frame cadence exists in the harness (the section's
  output now says so). The `cadence_hz=0` irregular pattern (including the
  2 s stall exceeding the 8-tick per-frame catch-up bound) is the `--sim`
  route's driver pattern (`sim_demo::irregular_frame`), whose determinism
  `tests/regions.rs` covers. The audit's genuine residual — no measurement of
  the worst-case pending accumulation under that stall shape — is closed by a
  NEW behavior-measurement section (the stall worst case) driving the real
  SimDriver + NpcWorld Legacy workload (1,000 NPCs, period 6 + seeded jitter,
  budget 200 / near reserve 60, 10 Hz, catch-up bound 8) through a 120-tick
  headless warm plus 12 cycles of the demo's own irregular pattern, with the
  no-discard invariant (clock + outstanding == ticks due) and the spine
  conservation asserted at every frame and the full per-frame census
  double-run-identical. Measured (release, this lane's tree): **worst-case
  retained tick obligation 12** (every cycle's stall: the 2 s stall's 20 due
  ticks minus the 8-tick per-frame bound), draining to 0 inside every cycle
  (no cross-cycle accumulation); **worst-case queue pending 1,000 — exactly
  the pre-stall steady armed population** (one pending follow-up per NPC; the
  stall does not grow the queue: arrivals come only from budget-bounded
  executions, so unbounded accumulation requires demand > budget — the
  sustained matrix's overload class, measured separately);
  cumulative-from-construction late arrivals 0 / late executions 169 /
  budget-exhausted ticks 6, ALL from the headless warm ramp (the initial
  1,000-event schedule spread's synchrony) — the 144 irregular-cadence frames
  add ZERO budget exhaustion (asserted); per-frame cost p50/p99/max
  0.000/0.174/0.278 ms (the max is the 8-tick catch-up batch). Lib-side
  guard: `stall_worst_case_double_run_and_pin` (pins 12 / 1,000 / (0, 169, 6)
  plus the empty-increment attribution law).
- **B11 — the ripple diagnostic's census relabeled; one genuinely-wrong
  counter corrected.** The module doc's "admitted/visible ring centers" and
  "the frames whose displacement moves displayed rgb near a live ring (the
  term-separation census)" conflated field-level counts with rendered-pixel
  claims (the term-separation census belongs to the render_weather gates,
  E-7 — the phrase was a phantom in this diagnostic). The output now defines
  its terms before the arms: contacts = kill-plane ground landings; admitted
  = rings GENERATED into the field (at the shipped 320:1 the other ~319/320
  landings are NEVER GENERATED — decimated before admission, not occluded);
  live / frames-with-a-live-ring = FIELD censuses (a frame counts EXISTENCE
  — whether the displacement pass actually moved displayed rgb is not
  measured here). **The genuinely-wrong counter:** "contacts" summed the
  particle report's all-cause `deaths` (kill-plane landings AND midair
  lifetime expiries) while labeled "the physical ground contacts" — the
  layer's own census comment separates the two classes, and ripple.rs's
  measured demo-campaign context is 18 kill deaths vs 5,286 lifetime deaths.
  It now reads the layer's public kill-plane census (`kill_deaths()` delta,
  the same delta discipline as the neighboring counters). At this harness's
  fixed-camera volume the two counts coincide numerically (pre- and post-edit
  runs both print contacts 4,809; the three-arm arithmetic floor(4809/320)=15,
  /160=30, /80=60 matches the admitted counts exactly), so the shipped
  numbers' values are unchanged and their meaning is now correct;
  admitted/refused/live counts are untouched (counting logic otherwise
  identical, per the audit's labeling-only scope).
- Gates (this lane's tree): `cargo fmt --all -- --check` exit 0;
  `cargo clippy -p engine-slice --all-targets -- -D warnings` exit 0;
  `cargo test -p engine-slice --lib` **144 passed / 0 failed** (baseline 143
  — one NEW test, no existing test touched, no pin weakened); `--test
  regions` 18/0 and `--test sim_determinism` 8/0 (the SimDriver machinery
  the new section exercises); `--benchmark-behavior` and
  `--benchmark-ripple-visibility` both exit 0 (logs at
  `Temp\opencode\ee\b10b11-artifacts\`).
- Provenance note: the raw `baseline-battery.log` this record cites is no
  longer present at `Temp\opencode\ee\artifacts\` (only the later gate logs
  survive). The stall-section and ripple numbers above were measured on
  this lane's tree (the logs just cited).

## E-9 The owner's maximal-composition walk (2026-11-27, rig, tree `85a83b2`)

The owner ran the maximal composition route (`--corner --populated --behavior
--shadows --weather rain --wet --day-night 20 --post --free-look --audio`) on
the integrated tree and judged **"Everything looks right to me"** — the visual
non-regression acceptance for the entire landed program to date (all 15+
findings' changes compose: the crowd, per-NPC shadows, rain, wet reflections
with ripples, day/night, post, audio). The exit summary: 2,537 presented
frames, validation warnings/errors **0/0**, 1 swapchain recreation (the
resize), the audio device healthy (4,644 callbacks, 0 write rejections, worst
callback 19.7 µs, 0 triggers dropped), 26 instances in 2 indirect draw calls,
and the full allocation tuple printed. The owner's judgment is the standing
authority for visual claims; the byte-equality and pixel gates carry the
deterministic evidence beneath it.

## E-10 The IA-09 fixture repair rig-verified (2026-11-27, rig, tree `435fc60`)

The repaired `render_post` battery run in full on the real GPU (no name
filter needed — the poison cascade is gone): **7 passed / 0 failed, exit 0,
11.26 s.** Both depth fallbacks pass all 61 post-resize probes at **max error
0/255** (D24_S8 and D16), the post-lifecycle reference probes green at the
original extent, the pixel/discriminator/tiny-extent gates green, and the S1g
`submit_frame` byte-equality gate re-confirmed in the same run (262,144-byte
captures, 190,668 differing-bytes composed ink, allocations constant). The
extent assertion is fail-visible in the gate. Log:
`Temp\opencode\ee\artifacts\gates-ia09-rig.log`. The historical rig-pending
clause is discharged: the oracle was always right; the capture extent was
wrong.

## E-11 B15 workspace footprint measurement (2026-11-27, rig CPU 9800X3D, worktree tree `72c91d3`, branch `ee/b15-workspace-footprint`)

The B15 lane's full record: `docs/engineering-excellence/FOOTPRINT.md`
(every table's command, method, and finding lives there; raw trees/logs at
`Temp\opencode\ee\b15-artifacts\` — scratch by the E-manifest convention).
All commands below exit 0 unless stated.

- Closure matrix (`cargo tree -p <crate> --edges {normal | normal,build |
  normal,build,dev} --prefix none`, markers stripped, unique counted):
  core 1/1/1 (zero deps), persist 20/25/25, renderer 16/53/57, shell
  6/14/14, slice 36/70/70 unique incl. self. Plain `cargo tree -p
  engine-renderer` (no `--edges`) = 57 — the default includes dev+build.
- Binaries: `cargo build -p engine-slice` → debug exe 16,371,200 B;
  `cargo build -p engine-slice --release` (cold release dir, timed
  48.0 s) → release exe 10,509,824 B. Release rlibs: core 3,543,938 ·
  persist 10,454,706 · renderer 2,829,696 · shell 508,042 · slice
  12,298,198 B. Stripped sizes NOT measured — no strip binary in the
  pinned `minimal` toolchain profile (recorded, not worked around).
- Minimal consumers (throwaway projects outside the repo, fresh locks,
  own target dirs, `Measure-Command` around `cargo build` after an
  untimed `cargo fetch`; all `cargo run` green): core 2.9 s / closure 2;
  persist 9.0 s / 26 (SQLite C compile isolated at 3.5 s via
  `cargo clean -p libsqlite3-sys` + rebuild); shell 44.5 s / 15 (SDL
  static isolated at 38.4 s via `cargo clean -p sdl3-sys` + rebuild);
  renderer 41.9 s / 54; slice 52.6 s / 71.
- Key outcomes registered: B15 → MEASURED (the rend-half note's
  "nalgebra" corrected — no nalgebra in any closure); NEW-REND-11 (the
  sdl3-sys caret drift: fresh consumers resolve SDL 3.4.20 vs the
  workspace's pinned 3.4.18); dev-dep isolation verified closure-neutral;
  rusqlite-unconditional and the renderer's absent types-only surface
  recorded as the two owner-decision levers (FOOTPRINT.md §8).
- Lane gates (untouched tree): `cargo fmt --all -- --check` exit 0;
  `cargo check --workspace --all-targets` exit 0.

## Measurement baselines

(To be established in P0: first full build of the integration tree, then the
workload matrix per charter §8.2. Rig GPU numbers remain the owner's;
lavapipe = correctness only. No invented metrics.)
