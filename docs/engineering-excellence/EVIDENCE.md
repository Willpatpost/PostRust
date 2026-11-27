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

## Measurement baselines

(To be established in P0: first full build of the integration tree, then the
workload matrix per charter §8.2. Rig GPU numbers remain the owner's;
lavapipe = correctness only. No invented metrics.)
