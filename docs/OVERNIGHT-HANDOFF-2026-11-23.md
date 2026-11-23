//! # OVERNIGHT HANDOFF — the 2026-11-23 autonomous cycle-6 run
//!
//! **Status:** v1.0 · written 2026-11-23 (late) by the overnight
//! agent's third session · **Read this first if you are the owner
//! or the morning auditor: everything is done and verified — but
//! nothing is pushed, and that is an environment fact, not a work
//! gap.** The one action this run needs from you is at
//! "THE ONE BLOCKER" below.
//!
//! ## What exists (the whole ladder, complete)
//!
//! Four packages, four branches, one commit each (plus this
//! handoff commit), all built and verified on this dev pod:
//!
//! | # | Package | Branch | Commit | Base |
//! |---|---------|--------|--------|------|
//! | 1 | 6-E instanced draws | `cycle6/6e-instanced-draws` | `511b157` | `main` |
//! | 2 | 6-F directional shadows | `cycle6/6f-directional-shadows` | `96dee7c` | `main` |
//! | 3 | corner-scene population | `cycle6/corner-population` | `5c9690d` | package 1 |
//! | 4 | 6-E2 indirect draws (stretch) | `cycle6/6e2-indirect-draws` | `6ee89b4` | package 3 |
//!
//! Every branch carries its full evidence record in its commit
//! message (the house style: implementation, evidence with exact
//! numbers, CI pin deltas, deferred list). The PR body for each
//! should be that commit message.
//!
//! ## THE ONE BLOCKER — no GitHub write access on this pod
//!
//! The dev pod has **no GitHub credentials**: no `gh` CLI, no
//! `~/.git-credentials`, no `~/.netrc`, no `GH_TOKEN`, no SSH
//! keys (`ssh -T git@github.com` → "Permission denied
//! (publickey)"), and the code-server askpass sockets git tried
//! (`/tmp/vscode-git-*.sock`) are dead stale files — `git push`
//! dies with `remote: No anonymous write access.` This is why
//! sessions one and two left committed-but-unpushed branches
//! (they measured their lavapipe evidence the same user-local
//! way this session rebuilt it — see the recipe below — but
//! could never push). **Per the protocol this was parked and
//! documented, not thrashed.**
//!
//! ### The exact morning commands (owner, from any machine with
//! credentials — or provision a write token on this pod and the
//! next session can do it):
//!
//! ```sh
//! cd ~/PostRust   # or your clone
//! git push origin cycle6/6e-instanced-draws cycle6/6f-directional-shadows \
//!                 cycle6/corner-population cycle6/6e2-indirect-draws
//!
//! gh pr create --draft --title "cycle 6-E: instanced draws" \
//!   --head cycle6/6e-instanced-draws --base main \
//!   --body-file <(git log -1 --format=%B 511b157)
//! gh pr create --draft --title "cycle 6-F: directional shadows" \
//!   --head cycle6/6f-directional-shadows --base main \
//!   --body-file <(git log -1 --format=%B 96dee7c)
//! gh pr create --draft --title "corner-scene population (6-E ladder package 3)" \
//!   --head cycle6/corner-population --base cycle6/6e-instanced-draws \
//!   --body-file <(git log -1 --format=%B 5c9690d)
//! gh pr create --draft --title "cycle 6-E2: indirect draws" \
//!   --head cycle6/6e2-indirect-draws --base cycle6/corner-population \
//!   --body-file <(git log -1 --format=%B 6ee89b4)
//! ```
//!
//! The stacked `--base` choices keep each PR's diff exactly its
//! package (population shows only `5c9690d`; 6-E2 only `6ee89b4`);
//! GitHub retargets a stacked PR to `main` automatically when its
//! base branch merges and is deleted. Merge order: 6-E →
//! population → 6-E2 (6-F is independent; mark-ready order is
//! yours — nothing was merged by the agent, per the protocol).
//!
//! ## What this session verified (all MEASURED here, 2026-11-23)
//!
//! Full local gate suite per branch — `cargo fmt --check`,
//! `cargo clippy --workspace --all-targets --release -- -D
//! warnings`, the main shard, the renderer release battery, the
//! heavy shard — plus **every `--ignored` lavapipe gate the CI
//! enforces**, on the final tree of each line:
//!
//! - **6-E + population** (`cycle6/corner-population`): main
//!   shard 1142/0; heavy 576/11/58; renderer release 65/2; both
//!   `render_instanced` gates — 13-instance fixture (13 vs 2
//!   draws, byte-equal frames, oracles 0/255) and the shipped
//!   24+2 table (26 vs 2, byte-equal, ink 2745, warmed
//!   allocations constant) — and the populated route (601
//!   frames, validation 0/0, "26 instances, 2 draw calls per
//!   frame (instanced mode)").
//! - **6-F** (`cycle6/6f-directional-shadows`): main 1146/0;
//!   heavy 570/10/58; renderer release 69/2; the shadow gate —
//!   1,006,779 texels compared, 0 mismatches, max |Δdepth|
//!   2.68e-7, shadowed patch 0/255, lit patch 1/255, 65536/65536
//!   frame pixels changed, warmed allocations constant, the
//!   replacement lifecycle re-verified — and the
//!   `--corner --shadows` route (601 frames, validation 0/0).
//! - **6-E2** (`cycle6/6e2-indirect-draws`): main 1146/0; heavy
//!   576/12/58; renderer release 69/2; the three-way gate —
//!   per-object 26 == direct 2 == indirect 2 draws, byte-equal
//!   frames, one-time device_memory 18→19, oracle wall 0/255 —
//!   and the populated route in indirect mode (601 frames,
//!   validation 0/0).
//! - **The pre-existing graphical gates on the final tree**
//!   (regression sweep): `render_corner`, `render_orientation`,
//!   `render_overflow`, `render_glyph_atlas` — all green.
//!
//! Every CI pin on every branch matches these measured numbers
//! (the Windows pins carry the documented platform-uniform deltas
//! and are marked "to be confirmed by the candidate's CI run" —
//! the honest form; a mismatch there is corrected from actuals,
//! never by weakening a test).
//!
//! Rig-dependent perf claims (the CPU p50/p95/p99 tables) are
//! marked rig-pending in the commit messages; lavapipe numbers
//! are labeled correctness evidence only. **No measurement in
//! any commit message was taken on the owner's rig.**
//!
//! ## The user-local lavapipe recipe (reproducible, no root)
//!
//! The pod has no Vulkan loader (verified 2026-11-14 per
//! `render_overflow`'s header, still true). Sessions one and
//! three each provisioned a user-local one from the Ubuntu noble
//! pool — extract-only, no apt install, no root:
//!
//! ```sh
//! # debs from archive.ubuntu.com noble/noble-updates (24.04):
//! #   libvulkan1 1.4.363, mesa-vulkan-drivers 25.2.8-0ubuntu0.24.04.5,
//! #   libllvm20 20.1.2-0ubuntu1~24.04.3, libdrm2+libdrm-amdgpu1 2.4.125,
//! #   libwayland-client0 1.22.0, libx11-xcb1 1.8.7, the six libxcb-*
//! #   1.15, libxshmfence1 1.3.3, vulkan-tools 1.4.341 (vulkaninfo),
//! #   vulkan-validationlayers 1.4.341 (the Khronos layer)
//! mkdir -p /tmp/opencode/vulkan
//! for d in *.deb; do dpkg-deb -x "$d" /tmp/opencode/vulkan; done
//! # point lvp_icd.json's library_path and the layer JSONs at the
//! # prefix (they default to system paths), then per shell:
//! export LD_LIBRARY_PATH=/tmp/opencode/vulkan/usr/lib/x86_64-linux-gnu
//! export VK_ICD_FILENAMES=/tmp/opencode/vulkan/usr/share/vulkan/icd.d/lvp_icd.json
//! export VK_LAYER_PATH=/tmp/opencode/vulkan/usr/share/vulkan/explicit_layer.d
//! export SDL_VULKAN_LIBRARY=$LD_LIBRARY_PATH/libvulkan.so.1
//! SDL_VIDEODRIVER=offscreen cargo test -p engine-slice --test render_instanced -- --ignored --nocapture
//! ```
//!
//! **Mesa 25.2.8 matters**: the noble-release 24.0.5 lavapipe
//! does not expose `VK_EXT_headless_surface` (the SDL offscreen
//! driver needs it); 25.2.8 does. `SDL_VULKAN_LIBRARY` matters:
//! the static SDL's `SDL_LoadObject("libvulkan.so.1")` misses
//! the prefix without it (ash's loader finds it either way).
//! `/tmp/opencode/vulkan` does not survive pod reboots — future
//! sessions re-run this recipe (it is ~10 minutes end to end).
//!
//! ## Why `~/OVERNIGHT-DONE` was NOT created
//!
//! The standing order: create it "when the entire ladder is
//! complete **and every PR is green**." The code ladder is
//! complete; the PRs do not exist (the blocker above), so "every
//! PR green" is not satisfiable from this pod tonight. Creating
//! the exit signal anyway would misreport state to the
//! supervisor; the honest form is this document plus the
//! branches. Future sessions tonight: per protocol §5, improve
//! the weakest evidence in the existing packages — never new
//! scope; do not re-implement anything listed above.
//!
//! ## Flake protocol reminder for the PRs' first CI runs
//!
//! §3.1 stands: any CI failure in a test the diff does not touch
//! that has passed elsewhere = suspected runner flake → rerun
//! the failed job once, document the diff-relevance analysis in
//! the PR, leave it red with the analysis if it fails twice.
//! Known open flake: `s6_soak`'s late-window fsync race
//! (docs/AUTONOMOUS-CYCLE-PROTOCOL.md §3.1, flake C).
