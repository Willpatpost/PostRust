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
//!
//! ---
//!
//! ## SESSION 4 ADDENDUM — the verification session (2026-11-23,
//! later the same day)
//!
//! The fourth session inherited the mandate this document gives
//! future sessions: the ladder complete, nothing pushed, "improve
//! the weakest evidence in the existing packages — never new
//! scope; do not re-implement anything." Nothing below
//! re-implements anything; every change this session commits is
//! documentation.
//!
//! ### The push blocker, re-probed live — unchanged, still the one
//! blocker
//!
//! `git push --dry-run` fails exactly as session 3 recorded: the
//! code-server askpass sockets are dead files
//! (`ECONNREFUSED /tmp/vscode-git-*.sock`) and the remote answers
//! `No anonymous write access.` (exit 128). No `gh`, no
//! `GITHUB_TOKEN`, no `~/.git-credentials`, no `~/.netrc`, no SSH
//! keys, no credential helper — re-verified individually. Anonymous
//! READ still works: `git ls-remote origin` shows `origin/main` at
//! `1fe89b7` — the exact merge base of all four ladder branches,
//! with only the seven historical PR refs beside it — so nothing
//! has drifted underneath the branches. They remain one
//! authenticated push away; the morning commands above stand
//! unchanged.
//!
//! ### The session-4 audit (the morning audit's checklist, pre-run
//! by an independent session)
//!
//! - **Scope** — each package commit's diff is its brief and
//!   nothing else: 6-E touches 11 files (engine-renderer,
//!   engine-slice, the workflow, CURRENT-STATUS), 6-F 14, the
//!   population 7, 6-E2 10. `engine-core` is untouched in all
//!   four; AGENTS.md untouched; no drive-by refactors in any
//!   diffstat.
//! - **The equivalence gates are real assertions** — byte-equality
//!   with first-mismatch pixel reports; ink controls that reject
//!   vacuous comparisons (`render_instanced`'s changed-pixel
//!   floor, `render_shadows`' occluder-coverage and
//!   visible-change guards in BOTH directions); allocation
//!   constancy asserted on the FULL allocations struct across
//!   warmed frames in EVERY submission mode; the 6-B CPU-oracle
//!   cross-checks; the shadow map's known-answer against the CPU
//!   raycaster with the mismatch count pinned at 0.
//! - **Unsafe carries its proofs** — every new unsafe block
//!   documents its caller contract (`# Safety` + inline SAFETY
//!   notes: `proxy_draw`'s `write_slot`/`draw`, `proxy_indirect`'s
//!   `write_slot`, `shadow_draw`'s readback) inside the vulkan
//!   module's invariants manifest; the crate-level
//!   `#![deny(unsafe_code)]` still bounds everything outside it.
//! - **The determinism contract held** — every pinned selftest
//!   re-executed this session on this branch (the table below):
//!   the Stage 0 engine/state fingerprints, the canonical save
//!   bytes, the Stage 3 lane fingerprints, the Stage 4 presence
//!   fingerprints — byte-identical, double-run equal.
//!
//! ### THE RIG MEASUREMENT COMMANDS — written down at last
//!
//! The 6-E brief's acceptance criterion 5 was delivered circular:
//! the commit messages said "the rig commands are in the PR body"
//! while this handoff's PR-creation recipe uses the commit
//! message AS the body — the commands existed nowhere. They exist
//! now, in this section (carry the relevant block into each PR
//! body when creating the PRs). The discipline is
//! `docs/WINDOWS-GPU-PROFILING-KIT.md` §1–2: release build,
//! `--present immediate --fps uncapped` for the ceiling attempt,
//! every run at least twice with both reported, PresentMon
//! optional, never conflate pacing / presentation / GPU time /
//! presented frames. Every run below exits itself (the corner
//! route's fixed 601-frame regime) and prints the engine's own
//! run summary (live + exact `TimingSamples` percentiles, the
//! draw-call headline, peak RSS).
//!
//! PowerShell, from the repo root:
//!
//! ```powershell
//! # 6-E (and the population package — the same flags, the 24+2 table):
//! git checkout cycle6/6e-instanced-draws   # or cycle6/corner-population
//! cargo build --release
//! cargo run --release -p engine-slice -- --corner --populated --present immediate --fps uncapped
//! cargo run --release -p engine-slice -- --corner --populated --proxy-reference --present immediate --fps uncapped
//! # THE A/B: 26 instances in 2 draw calls (instanced) vs 26 draw calls
//! # (reference); the CPU p50/p95/p99 lines (live AND exact) are the claim.
//! cargo run --release -p engine-slice -- --corner --populated --validation on
//! # one validation-active correctness run (the CI gate's rig equivalent)
//!
//! # 6-F:
//! git checkout cycle6/6f-directional-shadows
//! cargo build --release
//! cargo run --release -p engine-slice -- --corner --shadows --present immediate --fps uncapped
//! cargo run --release -p engine-slice -- --corner --present immediate --fps uncapped
//! # THE A/B: the depth pass + shadowed-lit cost vs the unshadowed route.
//! cargo run --release -p engine-slice -- --corner --shadows --validation on
//!
//! # 6-E2:
//! git checkout cycle6/6e2-indirect-draws
//! cargo build --release
//! cargo run --release -p engine-slice -- --corner --populated --present immediate --fps uncapped
//! cargo run --release -p engine-slice -- --corner --populated --proxy-instanced --present immediate --fps uncapped
//! cargo run --release -p engine-slice -- --corner --populated --proxy-reference --present immediate --fps uncapped
//! # THE three-way: 26 == 2 == 2 draws; the indirect submission's CPU cost
//! # vs the direct one on real hardware.
//! cargo run --release -p engine-slice -- --corner --populated --validation on
//! ```
//!
//! Lavapipe numbers (the timing lines in every commit message)
//! remain correctness evidence only — no measurement in any of
//! these packages was taken on the owner's rig; these commands
//! are how that evidence gets taken.
//!
//! ### Corrective — the 6-F commit message's stale CI paragraph
//!
//! `96dee7c`'s message says "the main shard 1138 -> 1140 (Windows
//! 1087 -> 1089, the same platform-uniform 2); the release
//! battery 61 -> 63". That paragraph was written before the
//! `shadows.rs` battery grew from 2 tests to 8 during the
//! INVERTED-depth convention-bug hunt the same message describes,
//! and it was never refreshed. The tree's actual pins — the
//! workflow the 6-F branch carries, its comment chain ("Linux
//! 1146 = 1138 + the same 8; Windows 1095 = 1087 + the same 8",
//! "61 -> 69"), and session 3's handoff record — all agree; the
//! paragraph's numbers were never the measured state of any tree
//! on that branch. Session 4 independently re-measured the 6-F
//! tree's shards in a clean environment — main 1146/2/33 rc=0,
//! renderer release 69/2, the targeted shadows battery 8/8 (the
//! 6-F branch's own session-4 addendum carries the full record).
//! When creating the 6-F PR, append this corrective to the body.
//!
//! ### This branch's session-4 verification table (the corner
//! population, tip `cb92088` + this addendum commit)
//!
//! All steps below ran this session on this branch's tip, 65/65
//! green, zero flakes. These are the CI steps NO prior session
//! had executed locally plus targeted re-verification of this
//! package line's own linchpins:
//!
//! | Step | Result |
//! |---|---|
//! | fmt `--check`; clippy `-D warnings` (CI form AND the standing-order release form) | clean / clean |
//! | activation + crash-seam isolation (nm on the feature-disabled rlibs) | activation 0, adversarial 0, authorized read-only surfaces present, seam arms 0 |
//! | workspace inventory guard | exactly the five authorized packages |
//! | fingerprint selftest ×2 | double-run equal; `engine=6504123a47cab66f`, `state=937bd3b6f56f07e0` |
//! | spine-save selftest ×2 | double-run equal; `save-len=1209`, `save-fnv=c6c1b46c7d21843b` |
//! | Stage 3 sim smoke ×2 | double-run equal; `executed 45717`, `engine-lanes=5591d3eb05643e78`, `state-lanes=9d90cb3bbd45b032` |
//! | Stage 4 presence ×3 ×2 | stationary `da1b2d1a7e5cd49e`, walk `1df391ad488848bf`, tour `495ab27b37549e93` — all pinned values, double-run equal |
//! | feature-disabled production-path (B-P1, B-R1a2) | green |
//! | proxy unit battery (debug) | 13 passed (the scaffold's 9 + the 6-E implementation's 4) |
//! | `corner_population` placement battery | 6 passed |
//! | renderer release battery | MEASURED 65/2 (the branch pin 65/2) |
//! | F-06 benchmark canary | green (region scene median 0.048 ms/frame; overlay-on extra 0.030 ms/frame) |
//! | the five `--render` offscreen smokes | green |
//! | `render_scene_separation` + `render_encounter` `--ignored` | green |
//! | playable-world smoke | green |
//! | plain day/night corner routes (601 frames, every CI grep) | green — first local execution of these steps |
//! | populated route | 26 instances (24 npc + 2 vehicle), **2 draw calls per frame (instanced mode)**, validation 0/0 |
//! | `render_instanced --ignored` | BOTH gates green — the 13-instance fixture AND the full 24+2 table's N-vs-1 byte-equality re-confirmed |
//! | **the full main shard** (session 4's own run, CLEAN environment — measured on the 6-E tree, whose main-shard test set this branch shares exactly: this package adds no main-shard tests) | **MEASURED 1142 passed / 2 ignored / 33 result lines, rc=0, zero failures — the branch pin, a second independent full-shard confirmation of session 3's record** |
//! | S6-C3 soak smoke + fresh-process recovery | evidence package verified complete; workload gate PASS |
//!
//! Session 3's shard measurements for this branch (main
//! 1142/2/33, heavy 576/11/58, renderer release 65/2) are the
//! immutable-tree records; session 4's own full main-shard run
//! above (via the shared 6-E test set) and its targeted re-runs
//! of every test this package line added agree with them (13
//! proxy + 6 placement + 2 gates + 65/2 release).
//!
//! (Operational note for future pod sessions, from the 6-F
//! lane's one root-caused FAIL: run the CPU shards in a CLEAN
//! environment — `SDL_VIDEODRIVER=offscreen` breaks engine-shell's
//! relative-mouse test; export the lavapipe variables only for
//! the graphical commands, exactly as CI scopes them. The full
//! record is on the 6-F branch's addendum.)
