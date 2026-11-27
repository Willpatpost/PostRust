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

## E-3 Model fleet (verified callable, 2026-11-26)

`tools.opencode.models` (odu provider): glm-5.3-w4a8, qwen3-coder-next,
gpt-oss-120b, MiMo-V2.6-Pro-RL, qwen36-35b, qwen38-27b, qwen3-8-flash-next,
gpt-oss-20b, gemma-4-31b, gemma-4-12b, gemma-4-e4b, diffusiongemma-26b —
all active chat models. (Non-chat: embedding/ASR/TTS/audio per charter §3,
not used for source judgment.)

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

**Workspace production ≈ 128k lines; test material ≈ 155k+ lines** (inline
+ tests/ dirs) — a test-heavy codebase by design; per-file prod counts feed
A01 (split by change reason) and B15 (footprint claims must cite these, not
whole-file sizes).

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

## Measurement baselines

(To be established in P0: first full build of the integration tree, then the
workload matrix per charter §8.2. Rig GPU numbers remain the owner's;
lavapipe = correctness only. No invented metrics.)
