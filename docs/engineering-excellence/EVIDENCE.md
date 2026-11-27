# Evidence Manifest

Every entry: what, exact command(s), exit status, tree SHA, artifact path.
Raw large outputs live outside the source tree at
`C:\Users\Willp\AppData\Local\Temp\opencode\ee\artifacts\` (scratch, pre-approved);
summaries and hashes are committed here so important results survive loss.

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

**IA-13 CORRECTION (2026-11-27, per the independent audit):** this census has
known defects — it omits `crates/engine-core/tests/` entirely (11 tracked
files, 4,471 nonempty lines), the shader glob (15 WGSL) misses the four
compiled GLSL inputs (`.vert`/`.frag`, enumerated by build.rs), and the
earlier "155k+" total overstated the table's own mixed-convention sum
(134,949; adding the omitted core tests alone yields 139,420). A regeneration
from the tag with ONE line-count convention is queued (P6); until then the
per-crate PRODUCTION figures remain the useful column.

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

## Measurement baselines

(To be established in P0: first full build of the integration tree, then the
workload matrix per charter §8.2. Rig GPU numbers remain the owner's;
lavapipe = correctness only. No invented metrics.)
