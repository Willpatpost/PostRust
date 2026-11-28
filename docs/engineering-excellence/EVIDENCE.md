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
*(Superseded for current-tree dependency data by E-11's B15 closure matrix —
core 0 external, persist 19, renderer 15, shell 5, slice 35 unique normal
deps; the manifest edges above verified current at `1d4ecc9`.)*

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

## E-12 P5 R0 — the census harness rows actually run (2026-11-27, lane worktree tree `b229e7f`, branch `ee/p5-r0-census-harness`)

The R0 lane's three runnable rows (the §9.2 CPU matrix pod-valid by
design; the §9.4/§9.5 rows run on the rig's own hardware — the RTX
5070 Ti box, release, real windows, the E-7 discipline). All
commands below exit 0. The §9.1 VRAM census and the §9.6
composition curves are the remaining rig rows (24 + 4 windows
including 4K — the serial archived runs; the batteries are
`tests/census_vram.rs` / `tests/census_composition.rs`, run the
same way).

**§9.2 — the repack matrix (BEFORE R2, CPU, release, 9800X3D):**
`cargo test -p engine-slice --test census_repack --release --
--ignored --nocapture --test-threads=1` → 1 passed / 0 failed.
Per-cell median µs/frame (instance-repack / pose-eval / slot-write;
p99 in the battery's printed row; 200 samples after 200 warmup;
the 0%-dirty instance and write columns are also the between-tick
unchanged-population cost — render-then-step's before-R2 shape):

| population | dirty | repack | pose | write | inst B | pose B |
|---|---|---:|---:|---:|---:|---:|
| garage (6+1) | 0% | 0.10 | 0.30 | 0.00 | 672 | 4,224 |
| garage (6+1) | mix (4/2) | 0.10 | 1.60 | 0.10 | 672 | 4,224 |
| garage (6+1) | 100% | 0.10 | 2.30 | 0.00 | 672 | 4,224 |
| corner (24+2) | 0% | 0.30 | 0.50 | 0.10 | 2,496 | 16,896 |
| corner (24+2) | mix (16/8) | 0.30 | 5.70 | 0.20 | 2,496 | 16,896 |
| corner (24+2) | 100% | 0.30 | 8.20 | 0.10 | 2,496 | 16,896 |
| 1k | 0% | 11.60 | 10.00 | 10.80 | 96,000 | 704,000 |
| 1k | mix (664/336) | 11.60 | 228.30 | 11.00 | 96,000 | 704,000 |
| 1k | 100% | 12.10 | 353.30 | 11.70 | 96,000 | 704,000 |
| 4k (at cap) | 0% | 49.10 | 39.90 | 110.00 | 393,216 | 2,883,584 |
| 4k (at cap) | mix (2728/1368) | 48.70 | 946.10 | 107.60 | 393,216 | 2,883,584 |
| 4k (at cap) | 100% | 47.40 | 1,373.00 | 100.30 | 393,216 | 2,883,584 |

Shape findings (the row's purpose): the instance repack and slot
write are FLAT across the dirty axis (the whole-set before-R2
cost); the pose evaluation scales with walkers (~0.34 µs/walker;
standing NPCs ~10 ns/block through the A17 cache); the §9.3 carve
context prints demand-independent 8,320,000 B. Boundary leg: 4k
packs, 4k+1 refuses (both kinds, typed).

**§9.4 — the attach/upload census (BEFORE R3, rig GPU, release):**
`cargo test -p engine-slice --test census_attach --release --
--ignored --nocapture --test-threads=1` → 1 passed / 0 failed
(twice-reproduced; counters identical across runs, wall time
varies 6.9–47 ms with first-run driver caching):

| scene | attach-wall-ms | submits (attach/construction) | blocking-wait-ms | staged-pairs | tail-B | tex-mem | tex-bytes | dev-mem |
|---|---:|---|---:|---:|---:|---:|---:|---:|
| corner | 6.92 | 5 (4/1) | 0.65 | 1 | 794,624 | 4 | 1,589,248 | 10 |
| garage | 7.28 | 5 (4/1) | 1.24 | 1 | 794,624 | 4 | 1,589,248 | 10 |

The `texture_bytes` 1,589,248 cross-checks E-7's tuple exactly; the
NEW `texture_staging_tail_bytes` shows the permanent tails are
794,624 B = 50.0% of the texture residency (the R3 retirement's
whole prize). The frame-path freeze assertion passed: the four
staging counters do not move across warmed dry frames.

**§9.5 — the ripple rows (BEFORE R4, rig GPU, release):**
`cargo test -p engine-slice --test census_ripple --release --
--ignored --nocapture --test-threads=1` → 1 passed / 0 failed
(120 samples per cell after 4 warm; the field state is the only
variable — no rain instances):

| state | extent | gpu-ms p50/p95/p99/max | live rings |
|---|---|---|---:|
| empty | 1280×720 | 0.032/0.036/0.037/0.070 | 0 |
| steady | 1280×720 | 0.031/0.037/0.042/0.044 | 1 |
| saturated | 1280×720 | 0.042/0.047/0.048/0.053 | 32 |
| empty | 2560×1440 | 0.073/0.082/0.086/0.089 | 0 |
| steady | 2560×1440 | 0.071/0.078/0.085/0.086 | 1 |
| saturated | 2560×1440 | 0.108/0.118/0.275/0.293 | 32 |

Saturation costs +31% (720p) / +48% (1440p) over empty on p50 —
the honest before-R4 magnitude (the finding's named worst case is
real but small at these extents, consistent with the design's
§5.2(a) boundary). The steady cell's instantaneous live count read
1 at the measured window (the ~3.7 is the time average; the count
oscillates 0–8 with the 1.5 rings/s admission and 2.5 s lifetime).

**Lane gates (the tree above):** `cargo fmt --all -- --check`
exit 0; `cargo clippy -p engine-renderer -p engine-slice
--all-targets -- -D warnings` exit 0; `cargo test -p
engine-renderer` 190 passed / 0 failed (debug AND release — the
pin unchanged, correct for a counter-only lane); `cargo test -p
engine-slice --lib` 156 passed / 0 failed (= 143 baseline + 13
census); the batteries' non-ignored portions: census_repack 2/1i,
census_vram 1/1i, census_ripple 1/1i, census_attach 0/1i,
census_composition 0/1i — all ok.

## E-13 NEW-REND-11 — the exact SDL pin + fresh-consumer identity demonstration (2026-11-27, lane worktree tree `1d4ecc9`, branch `ee/shell-sdl-exact-pin`)

Owner-approved per D-EE-10 item 3. Four manifest lines changed (version
requirement strings only — no code, no features, no new dependencies):
engine-shell and engine-renderer `sdl3` "0.20" → "=0.20.0" (both
`[dependencies]`) and `sdl3-sys` "0.7.2" → "=0.7.2" (both
`[target.'cfg(unix)'.dependencies]` — the only sdl3-sys requirements in
the workspace; the contract sweep covered all six repo manifests, the
other four are SDL-free, engine-slice reaches SDL only transitively).
Pins match the committed lock (sdl3 0.20.0 · sdl3-sys 0.7.2+SDL-3.4.18 ·
sdl3-src 3.4.18); sdl3-sys 0.7.2's own upstream manifest pins
`sdl3-src = "=3.4.18"`, so the vendored native source is transitively
exact. Inside vs outside workspace control: the facade sdl3 0.20.0's own
sdl3-sys requirements (`^0.7.1`, normal + build copies) and the optional
satellite crates (sdl3-image/mixer/ttf-sys and their -src crates —
features off, locked but never compiled) are upstream; the workspace
controls exactly its own four requirements, all four now exact.

Fresh-consumer demonstration (B15 §6.1 method, throwaway consumers at
`Temp\opencode\ee\q1-minimal-consumer\{shell-before,shell-after}\`,
path dep on the lane tree's engine-shell, fresh locks, own target dirs;
every command exit 0):

- BEFORE (pre-pin manifests): `cargo generate-lockfile` → sdl3-sys
  0.7.3+SDL-3.4.20 + sdl3-src 3.4.20, sdl3 0.20.0, 20 packages (F-01
  reproduced on the lane tree); `cargo run -q` → "input state
  constructed; linked SDL version Version { major: 3, minor: 4,
  patch: 20 }".
- AFTER (post-pin manifests): `cargo generate-lockfile` → sdl3-sys
  0.7.2+SDL-3.4.18 + sdl3-src 3.4.18, sdl3 0.20.0, 20 packages — cargo
  prints "available: v0.7.3+SDL-3.4.20" / "available: v3.4.20" and does
  not select them; `cargo run -q` → "input state constructed; linked SDL
  version Version { major: 3, minor: 4, patch: 18 }";
  `cargo tree -i sdl3-sys -e normal,build` → sdl3-sys v0.7.2+SDL-3.4.18
  ← sdl3 v0.20.0 ← engine-shell (path) ← consumer, the build-graph copy
  unified at the same 0.7.2; closure 15 unique (`--edges normal,build
  --prefix none`) = the B15 shell consumer's §6.3 figure — identity
  changed, shape unchanged. Both consumer runs print the known benign
  LNK4098 (F-10 — outside the workspace's `.cargo/config.toml` CRT
  toolchain scope; cosmetic). Post-pin re-resolve in the BEFORE
  directory DOWNGRADES its existing lock 0.7.3 → 0.7.2 ("Downgrading
  sdl3-sys v0.7.3+SDL-3.4.20 -> v0.7.2+SDL-3.4.18"): the enforcement is
  manifest-side, reaches existing consumers' locks, and `cargo update`
  can no longer silently drift the native SDL.

Lane gates (post-pin tree; all exit 0): `cargo fmt --all -- --check`;
`cargo check --workspace --all-targets` (45.33 s — `git status`: only
the two manifests modified, `Cargo.lock` byte-identical, i.e. the pins
enforce exactly what the lock already resolved); `cargo test -p
engine-shell` — lib 23 passed/0 failed, tests\audio 1/0,
tests\input_dispatch 13/0, tests\lifecycle 2/0 (incl.
`linked_sdl_version_is_the_vendored_3_4_18`), tests\soak 0 passed/1
ignored (the opt-in 60 s measurement), doc-tests 0 — **TRUE total 39
passed / 0 failed / 1 ignored**; `cargo clippy -p engine-shell -p
engine-renderer --all-targets -- -D warnings` (the D-EE-03
touched-crates discipline). No behavior change: no `.rs` file touched,
lock untouched, dependency closure shape unchanged.
## E-14 P5 R4 — A19 the ripple ring loop localized by the field's active extent (2026-11-27, lane worktree tree at commit, branch `ee/p5-r4-ring-localization`, rig GPU)

The A19 implementation lane (the design §5.3's recommendation (c)):
the versioned rows[2] bound claim — the conservative bounding circle
over the live rings, `max_i(|center_i − bc| + radius_i + width)`
padded by the ULP travel budget (a relative 2⁻¹⁹) — packed by
`pack_ripple_field` through the ONE shared `planar::
ripple_field_bound`, read by the fragment shader's early-out gate
(`bound_dist <= bound.z` runs the loop verbatim; outside takes the
identity path directly — bit-identical by the shader's own
documented identity contract). RIPPLE_UNIFORM_ROWS 34 → 35; the ring
rows shifted rows[2 + i] → rows[3 + i] in the same versioned change
(packer + WGSL struct + stride arithmetic + the planar oracle + the
gate-side `oracle_rows` layout companion in engine-slice's
render_weather — no gate value or assertion changed, only the row
encoding). (b) the CPU-side receiver-rect culling: DEFERRED per the
design's optional-tail provision (the packer↔oracle↔shader mirror
would all need the rect, and every shipped receiver is the full ~80 m
ground plate — the cull drops nothing; trigger recorded at the
packer). (d) tile/emitter-localized processing: deferred with
trigger in the shader header.

**§9.5 — the ripple rows (AFTER R4, rig GPU, release, the same
battery/command as E-12):** `cargo test -p engine-slice --test
census_ripple --release -- --ignored --nocapture --test-threads=1`
→ 1 passed / 0 failed. BEFORE (this lane's own base run, same
session/hardware) → AFTER:

| state | extent | gpu-ms p50 BEFORE | p50 AFTER | p95 BEFORE | p95 AFTER |
|---|---|---:|---:|---:|---:|
| empty | 1280×720 | 0.033 | 0.029 | 0.036 | 0.036 |
| steady | 1280×720 | 0.033 | 0.031 | 0.036 | 0.036 |
| saturated | 1280×720 | 0.043 | 0.034 | 0.047 | 0.042 |
| empty | 2560×1440 | 0.072 | 0.076 | 0.242 | 0.093 |
| steady | 2560×1440 | 0.070 | 0.075 | 0.075 | 0.085 |
| saturated | 2560×1440 | 0.104 | 0.077 | 0.114 | 0.082 |

The saturation overhead over empty (p50, the stable metric — the
session-to-session empty row itself moves ±0.004 ms): 720p
+0.010 → +0.005 ms (halved; E-12's +31% → ~+17%); 1440p +0.032 →
+0.001 ms (E-12's +48% → ~+1% — eliminated: the saturated field's
~13.4 m bound circle leaves most of the visible ground plate
outside, and those fragments now pay one sqrt instead of the
32-ring loop). The steady rows are unchanged within rig noise
(the ~3.7-ring field was never the cost). Full p99/max columns in
the archived logs (`Temp\opencode\ee\r4-logs\{before,after}-census.log`)
— the tails are rig-noisy and carry no signal.

**The ink gates (the acceptance bar, BEFORE → AFTER, same
session/hardware, exit 0 both):**
`cargo test -p engine-slice --test render_weather -- --ignored
--nocapture --test-threads=1` → 11 passed / 0 failed BOTH runs,
digit-for-digit: rain ink 17041 differing bytes; broadside ink
3482 px / 2162 upper-third; the AllocationStats tuple
(19/4/1589248/3/7/2); the ripple reflection ink 236114 differing
bytes / 98131 differing pixels / 10187 displayed-RGB-qualified;
the ripple sweep 22/255 at ground center [-1.929102, 0.0,
-2.6461658]; the young-field ink census 22/22, best (577, 350,
22); the measured sweep left 22 / right −21 / plateau −1; the
natural sequence's landings and term counts; the term separations
**96548/67535/7625 blend-only and 27149/27149/2538
displacement-only — E-7's pinned values, reproduced exactly**.
`cargo test -p engine-slice --test render_reflection -- --ignored
--nocapture --test-threads=1` → 7 passed / 0 failed AFTER (the
BEFORE run was killed mid-battery by a session timeout after 5/7
legs ok — every leg carries `WetFrame { ripple: None }`, the
rainless wet path, so the battery is structurally invisible to the
A19 change: the empty uniform's count gate skips the early-out;
the (7, 2, 8, 2) allocation delta and the 60/1653/2619/5551
known-answer patches reproduced identically in both runs).

**The new unit battery (engine-renderer, runs in the standard
gates):** `the_field_extent_bound_never_culls_a_banded_fragment`
(the no-false-negative property — adversarial layouts: the newborn
disk, near-lifetime rings, the bbox-diagonal pair, the four-corner
square, 32 coincident centers, the axis-aligned
triangle-equality pair, the 10⁴-scale ULP-travel field, and seeded
random fields at 1/5/32 rings × three policies; every probe where
the verbatim loop yields a non-identity contribution must sit
inside the bound, measured through the shader's own dist form);
`the_early_out_is_value_transparent_against_the_verbatim_loop`
(the honest-bound oracle equals the infinite-bound verbatim-loop
oracle at every probe, bit-for-bit — the identity contract made
executable); `the_ripple_uniform_layout_agrees_with_the_shader_struct`
(the versioned-row claim's cross-side agreement — the WGSL struct's
member order and the 3 + RIPPLE_POOL_BOUND row count).

**Lane gates (this lane's tree):** `cargo fmt --all -- --check`
exit 0; `cargo clippy -p engine-renderer --all-targets --
-D warnings` exit 0; `cargo test -p engine-renderer` **193
passed / 0 failed (debug AND release** = the 190-pin + the three
A19 tests); `cargo test -p engine-slice --lib` **157 passed / 0
failed** (the lane's base count — untouched); the batteries'
non-ignored portions: census_ripple 1/1i, render_weather 1/11i,
render_reflection 0/7i — all ok.

## E-15 P5 R1 — the demand-sized arenas measured (2026-11-27, lane worktree tree `1ea3e05`+R1, branch `ee/p5-r1-arenas-budgets`; rig GPU RTX 5070 Ti, release, real windows, the E-7/E-12 discipline)

A11/A23's §1.3 landing: the three mapped carves (instance, pose,
region) size at demand under configured caps (RenderConfig's three
budget fields, defaults = today's constants); the typed refusals
byte-identical at the CAP (the cap, never the carve, is the
boundary); growth = one `device_wait_idle` re-creation per carve
(counted in `RenderStats::arena_growth_events`, the initial
demand-sized carve is not an event). All commands below exit 0
unless noted.

**§1.5 — the residency scaling rows (rig GPU, release; the battery
`census_vram::arena_residency_scaling_rows`):** resident mapped
bytes per carve, BEFORE (the constant carves) vs AFTER (the
demand-sized carves, warmed frames at each population):

| population | inst supplied/carved | pose supplied/carved | instance B before/after | pose B before/after | events |
|---|---|---|---|---|---:|
| garage (6+1) | 7/64 | 6/64 | 786,432/12,288 | 5,767,168/90,112 | 0 |
| corner (24+2) | 26/64 | 24/64 | 786,432/12,288 | 5,767,168/90,112 | 0 |
| 1k | 1,002/1,024 | 1,000/1,024 | 786,432/196,608 | 5,767,168/1,441,792 | 0 |
| 4k (at cap) | 4,096/4,096 | 4,096/4,096 | 786,432/786,432 | 5,767,168/5,767,168 | 0 |

The design's named asymmetry measured: the corner's pose carve
drops 5,767,168 → 90,112 B (1.56% of the before carve — the 64-
block demand quantum's floor against 24 blocks used), the garage's
to the same floor; the 1k rows land within 2× of the used bytes
(the pow2 quantum); the 4k rows sit AT the default caps, so the
at-cap batteries' pins hold by construction (before == after).

**§1.5 — the growth gate boundary (rig GPU, release; the battery
`census_vram::arena_growth_gate_boundary`, 4 legs, 1 passed):**
(1) 100 → 300 instances/poses under the 4,096 cap: each carve
grows ONCE (128 → 512; `arena_growth_events` 0 → 2; exactly +1
device_memory per carve after the capture path is warmed — the
a6e5adc discipline applied to the arena), the growth frame's
capture is BYTE-IDENTICAL to a from-frame-1 reference renderer's,
and the same population again allocates nothing (steady state).
(2) a 200-instance configured cap: 201 refuses with exactly
`InstancePackError::Capacity { capacity: 200, requested: 201 }`
(the CAP, never the carve, is the boundary); 150 renders (carve
200). (3) a 500-vertex configured region budget: 612 supplied
after a 100-vertex first frame grows the carve 128 → 500 (one
event) AND truncates exactly 112 — the F-03 counter reports the
truncation the same-frame growth did NOT mask (the counter is
never cleared). (4) the pose growth under the SHADOWED family
(the heaviest sync path — the retained buffer re-created while
the per-generation shadowed sets bind it): validation 0 across
the growth and the following shadowed frames (the stale-binding
rewrite is clean).

**§9.1/§9.4/§9.5/§9.6/§9.2 re-runs (rig GPU + CPU, release):**
the full §9.1 VRAM census table (24 cells, 3 passed — every
family × extent tuple matches E-12's counter shape: the demand
sizing changes carve SIZES, never allocation COUNTS); the §9.4
attach census unchanged (corner/garage: 5 submits, 1 staged pair,
794,624 B tails, tex-bytes 1,589,248 — the attach path touches no
arena); the §9.5 ripple rows within noise of E-12 (saturation
+39% 720p / +51% 1440p over empty on p50 this run); the §9.6
maximal-composition curves validation 0/0 at all four extents
(median gpu-ms 0.065/0.115/0.184/0.365 at 720p/1080p/1440p/4K);
the §9.2 repack matrix byte columns identical (672/4,224 /
2,496/16,896 / 96,000/704,000 / 393,216/2,883,584 B — the repack
bytes are unchanged by the bounded forms).

**The population gates re-run on the rig GPU (release, serial):**
render_instanced 8/8 (the N-vs-1 pixel equivalence, the indirect
mechanism's +1 memory pin, the replace-pipelines lifecycle, and
the 4,200-frame wake campaign's `renderer.allocations()` constancy
— the 26-instance population's carve is stable, no growth events
mid-campaign); render_post 7/7 (the (8,4,16,0) composition delta
— the lazy families' one-time allocation COUNTS unchanged by the
demand sizing); render_reflection 7/7, render_shadows 4/4,
render_corner 1/1, render_garage 1/1, render_sky 1/1,
render_weather 11/11 (the rain warmed-constancy leg re-based per
the a6e5adc discipline: one pool-bound 1,024-streak warm frame
sizes the instance carve past the field's churn before the
baseline — see the D-EE-06 map in the lane handoff),
render_glyph_atlas 1/1, render_encounter 1/1.

**Lane gates:** `cargo fmt --all -- --check` exit 0; `cargo
clippy -p engine-renderer -p engine-slice --all-targets --
-D warnings` exit 0; `cargo test -p engine-renderer` 195 passed /
0 failed debug AND release (= 190 baseline + 5 new: the budget
defaults pin, the arena quantum table, the §1.5 scaling
predictions, the configured-cap refusals both kinds);
`cargo test -p engine-slice --lib` 157 passed / 0 failed (the
base tree's own count — this lane adds no slice lib test); `cargo check --workspace
--all-targets` exit 0. OBSERVED (pre-existing, NOT this lane —
reproduced on the base tree `1ea3e05` via stash): the
`census_repack` non-ignored smoke's `median_us > 0.0` assert
flakes in RELEASE on this machine (the 672 B garage slot-write
median reads 0.00 µs — timing granularity); the debug-profile run
(the gate profile) passes 2/1i. The four offscreen-gated smokes
(render_overflow, render_orientation, render_scene_separation,
s6_evidence's device leg) require SDL_VIDEODRIVER=offscreen +
headless-surface Vulkan, unavailable in this loader — the
orchestrator's rig bar.

## E-16 P5 R2 — A18 the dirty-generation protocol + the stander pose-dedup (2026-11-28, lane worktree branch `ee/p5-r2-dirty-generation`, base `30126fa`; CPU release on this machine, the E-12 §9.2 discipline)

Owner-authorized per D-EE-10 item 1C (generation-aware preparation
ACROSS ALL APPLICABLE ROUTES; the full-repack fallback + an
independent reference path retained). Design §3 (P5-resource-
residency.md), executed as specified: (c) the caller-owned
generation + per-slot stamps as the protocol, (e) the stander
pose-dedup in the same lane, (d) dirty-ranges deferred (the §9.3
measured trigger — the matrix below is that measurement; the
0%-row saving makes per-instance ranges the third-order refinement
the design's own boundary states).

**The protocol (engine-renderer):** `ProxyPopulation` grows
`instance_generation`/`pose_generation` (u64; **0 = always-dirty**,
today's exact behavior for every un-opted caller — the versioned
public-API change; ~35 test literals mechanically updated to 0,
disclosed below). TWO independent levels, each pure and unit-tested
alone: (1) the CPU half — `repack_populations_generation` /
`repack_poses_generation` validate ALWAYS (the typed refusals stay
per-frame — the D2 discipline) and gate only the byte regeneration
on the caller's declared generation against the retained buffers'
records (`PopulationGenerations`, renderer-owned, reset to unknown
by every generation-0 submission; the count guard is the packed
footprint `buffer.len() == total × stride`); (2) the write half —
`PopulationStamps` in `FrameState` (instance/pose/indirect per
frame slot) gate the mapped writes in `upload_frame_data` through
the ONE public decision function `generation_write_needed` (a slot's
stamp records what it last consumed — the ping-pong correctness by
construction; a generation-0 write resets the stamp so an opted-in
generation can never skip against another caller's bytes). The
indirect command block rides the INSTANCE generation (its bytes are
a pure function of the per-kind counts — NEW-REND-08's
cached-indirect observation closed). **Resource-replacement
invalidation:** `recreate_swapchain` clears every stamp (the
replacement pipeline state re-creates the instance/indirect buffers
with no content copy) and the frame flow's arena-growth delta
(the R1 `arena_growth_events` counter) clears them on any carve
growth — no stamp can vouch for a replaced buffer's content.
**The missed-bump debug verify mode** (§3.3's mandated mitigation,
the independent reference path): `verify_population_generation` /
`verify_pose_generation` re-pack through the FULL-REPACK FALLBACK
and byte-compare, panicking "MISSED BUMP" on divergence — invoked
by the submission adapter on every reuse under
`cfg(any(debug_assertions, test))` (both arms behind one
dispatcher signature so the release-production build compiles the
call site with zero cost and zero warnings). **The full-repack
fallback itself is unchanged** — `repack_populations_bounded` /
`repack_poses_bounded` byte-identical, exactly what every gate miss
and every generation-0 caller runs.

**The all-routes adoption map (the owner's 1C ruling — every route
that presents a population across frames):** the corner crowd's
BOTH routes — scripted `run` and interactive free-look — through
`LocomotionCrowd`'s counters (instance bumped on every crowd tick's
re-pack AND every rain rebuild; pose bumped on every tick's
re-evaluation; between ticks — free-look's accumulator shape at a
presentation rate above the 60 Hz sim clock — neither moves and the
frame is free); the garage route (the design's own split: instance
generation constant 1 — the static placement never rewrites its
half after both slots are stamped; pose generation = presented+1,
bumping with every evaluated frame); the census composition route
(through the same crowd machinery); the rain layer's standalone
population (`RainLayer::instance_generation`, bumped per
`update_and_build` — the weather gates' route). sim_demo presents
NO population half (region-only rendering — verified: no
`ProxyPopulation` anywhere in its path); every un-opted submission
(the lavapipe ink gates' own constructions, generation 0) keeps
today's always-repack/always-write behavior EXACTLY — the CI ink
gates' submitted bytes are unchanged by construction.

**The stander pose-dedup (§3.2(e), the shared animation palette):**
`stander_dedup_block_indices` — ONE shared rule over stander flags
(standers → palette block 0, the ONE cached rest block; walker j →
1+j; no standers → the identity mapping, byte-identical submission
for all-walking populations). Adopted by the phase-column evaluator
(`evaluate_population_poses` — the garage and census path), the
locomotion evaluator + instance bridge (`corner_routes`), and both
population tables' constructors (`corner_population()` /
`garage_population()` — the authored tables stay hand data, the
remap is mechanical at construction). **The skin-matrix identity
proof** (the pixel-compatibility witness): `stander_dedup_
preserves_every_npcs_skin_matrices` (corner, 6 frames) and
`locomotion_dedup_preserves_every_npcs_skin_matrices` (128
advancing steps) assert, per npc, that the palette block its remapped
index selects is BIT-IDENTICAL to the pre-dedup per-npc evaluation
— same skin matrices per npc ⟹ same skinned vertices ⟹ identical
PIXELS while the submitted bytes change (fewer blocks: corner 24→17,
garage 6→5, any all-standing population →1). The ink gates
(pixel-level, CI) hold digit-for-digit by this proof; the pod
cannot run them (no Vulkan loader — E-15's standing note).

**§9.2 — the repack matrix, BEFORE vs the protocol AFTER (CPU,
release, this machine; 200 samples after 200 warmup; the same
harness, matched cells; median µs/frame — repack / pose / write):
BEFORE = the always-repack fallback; AFTER = the §3.3 gates:**

| population | dirty | BEFORE rep/pose/wr | AFTER rep/pose/wr | skips |
|---|---|---|---|---:|
| garage (6+1) | 0% | 0.20 / 0.30 / 0.00 | 0.10 / 0.00 / 0.00 | 1992 |
| garage (6+1) | mix (4/2) | 0.10 / 1.60 / 0.00 | 0.00 / 1.60 / 0.00 | 1195 |
| garage (6+1) | 100% | 0.20 / 2.90 / 0.10 | 0.10 / 3.00 / 0.10 | 1195 |
| corner (24+2) | 0% | 0.50 / 0.30 / 0.00 | 0.20 / 0.00 / 0.00 | 1992 |
| corner (24+2) | mix (16/8) | 0.40 / 5.70 / 0.10 | 0.10 / 5.90 / 0.10 | 1195 |
| corner (24+2) | 100% | 0.50 / 9.80 / 0.20 | 0.20 / 10.00 / 0.10 | 1195 |
| 1k | 0% | 17.10 / 0.50 / 0.90 | 4.30 / 0.00 / 0.00 | 1992 |
| 1k | mix (664/336) | 19.70 / 269.60 / 8.40 | 5.10 / 276.40 / 6.70 | 1195 |
| 1k | 100% | 17.60 / 404.50 / 11.40 | 5.00 / 410.20 / 9.90 | 1195 |
| 4k (at cap) | 0% | 55.60 / 1.30 / 5.20 | 16.50 / 0.00 / 0.00 | 1992 |
| 4k (at cap) | mix (2728/1368) | 62.00 / 1120.90 / 77.20 | 20.10 / 1193.30 / 67.20 | 1195 |
| 4k (at cap) | 100% | 77.50 / 1675.20 / 109.70 | 19.60 / 1796.20 / 101.50 | 1195 |

The 0% row IS the between-tick unchanged-population frame (the
render-then-step shape): its pose-evaluation and slot-write costs go
to ZERO (nothing regenerated, nothing evaluated, nothing copied —
the design's "~the submission-plumbing floor"), and the instance
repack drops to the ALWAYS-RUN validation floor (~3.5× at 1k/4k —
the D2 fail-visible discipline deliberately kept: a declared-
identical population that no longer validates still refuses). The
mix/100% rows keep their pose work (the walkers advance — the dirty
axis is honest) while the instance repack and the instance/indirect
writes skip (the static placement's own saving, the garage route's
shape). The pose-bytes column shows the dedup palette (corner mix
16,896→11,968 B; any 0% cell →704 B — one shared rest block). The
AFTER pose medians at mix/full run ~2-6% above BEFORE (the
generation record's bookkeeping on the always-miss path) — honest,
measured, and second-order against the 0% row's elimination.
Cross-run note: the BEFORE leg is the unchanged R0 code; its
medians vary with machine state against E-12's archived row (this
run's 0% cells: 0.20/0.50/17.10/55.60 vs E-12's 0.10/0.30/11.60/
49.10 — same shape, same flat-across-dirty instance finding; the
run-internal BEFORE/AFTER comparison is the row's point).
`generation_skips` (the gated-out operations per cell: 5/frame at
0% after both slots stamp, 3/frame at mix/full) is assertable in
the harness's own lib test — the protocol's observable saving.

**§3.5's mandatory tests, all green:** the equivalence — the
adapter-level oracle (`generation_gated_adaptation_matches_the_
always_repack_oracle`: a 16-frame fuzzed (generation, population)
sequence, the gated path vs the always-dirty oracle, both buffers
byte-identical every frame) and the write-half oracle
(`population_generation_protocol_matches_the_always_write_oracle`
in frame.rs's policy_tests class: 48 frames over the frame_slot
ping-pong with an interleaved always-dirty caller, every slot's
instance/pose/indirect regions equal the always-write bytes at
every frame); the missed-bump red-then-green
(`missed_bump_panics_the_verify` / `pose_missed_bump_panics_the_
verify` — #[should_panic] "MISSED BUMP", running in BOTH debug and
release test profiles since the verify functions are always
compiled; the green halves verify honest declarations clean and
stale-but-invalid inputs refuse loudly); the ping-pong shape
(`stable_population_writes_each_slot_once_per_generation` — one
write per slot per generation, the bump rewrites both, a
generation-0 write breaks the stamp's vouching); the replacement
invalidation (`stamp_clear_forces_rewrites_after_replacement` —
the state contract both wiring sites rely on; the wiring itself is
`recreate_swapchain`'s clear and the frame flow's growth-delta
clear, exercised on device by the existing lifecycle/recreation
batteries in CI); generation-0 byte-identity (the oracle's
always-dirty arm, frame-by-frame). Double-run determinism: the
pose/table FNV pins re-measured and frozen in one run, re-asserted
green in the gate re-runs.

**Conscious pin movements (all measured, all documented at the
constant):** corner_population's packed-table FNV
16_877_662_039_878_331_722 → **11_575_762_803_608_534_537** (the
dedup's versioned claim on the params vec4's x — the pose_index
remap); corner_population's pose-bytes FNV frames 0/33
5_757_258_358_703_043_002 / 12_735_933_771_296_667_791 →
**14_796_718_041_729_273_370 / 2_475_754_115_185_997_039** (the
17-block palette); corner_routes' pose-bytes FNV frames 0/33/300
3_836_824_830_651_730_592 / 14_640_098_374_474_969_127 /
15_375_200_366_290_321_505 → **17_667_891_731_665_696_896 /
6_063_901_512_547_130_183 / 17_999_475_867_621_276_545** (the
locomotion palette). NO simulation-side digest moved (the rain/
ripple fold states, the behavior stream, the locomotion campaign —
untouched paths); no renderer-side digest moved (the proxy_instances
fixture pin `9913137345556047595`, the animation pose-sequence
digest `2_694_074_108_253_436_588` — the packing math is
untouched). The census §9.2 byte columns move with the dedup
palette (documented in the harness's own tests).

**Lane gates (all exit 0):** `cargo fmt --all -- --check`;
`cargo clippy -p engine-renderer -p engine-slice --all-targets --
-D warnings`; `cargo test -p engine-renderer` **224 passed / 0
failed, debug AND release** (= 212 at the lane base + 12 new: the
gate decision table, the instance/pose decision sequences, the
atomic refusals, the two missed-bump reds + two greens, the adapter
oracle, the write-half oracle, the ping-pong shape, the stamp
clear); `cargo test -p engine-slice --lib` **173 passed / 0
failed** (= 169 at the lane base + 4 new: the two skin-matrix
identity proofs, the mapping pins, the after-leg skip test);
`cargo test -p engine-slice --test census_repack` (the battery's
gate leg, debug) 2 passed / 1 ignored; the census_repack release
measurement run `--ignored --nocapture --test-threads=1` 1 passed /
0 failed (the archived table above); `cargo check --workspace
--all-targets`. OBSERVED (pre-existing per E-15, NOT this lane):
the census_repack smoke's `median_us > 0.0` assert flakes in
RELEASE on this machine (timing granularity); the debug-profile
gate run passes. CI count pins: engine-renderer moves 212→224
(release battery pin at engine-ci.yml:883) and the main-shard
expected count moves with the +16 lib tests — per D-EE-06 the
orchestrator corrects the pins from these measured actuals at the
integration point; this lane touches no workflow file.

## Measurement baselines


Established: E-8 is the baseline battery (2026-11-26, tree `263532f` — the
"before" numbers the P2/P3/P5 packages are judged against, with the B10/B11
methodology corrections at E-8's tail), E-9 the owner's maximal-composition
walk, E-12 the P5 R0 census rows. The workload matrix continues per charter
§8.2 (the E-8 extension rows named in the P5 design; the maximal-composition
frame curves re-run after every P5 lane). Rig GPU numbers remain the owner's;
lavapipe = correctness only. No invented metrics.
