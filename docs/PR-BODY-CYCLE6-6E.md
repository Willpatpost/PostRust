## Cycle 6-E: instanced draws — one draw call per proxy kind

Implements `docs/CYCLE6-6E-BRIEF.md` on the merged scaffold. **The O(entities) CPU ceiling closes: the populated corner scene submits N proxy draws → ≤ 2 (one per populated kind), pixel-identical.**

### The five acceptance criteria, with evidence

**1. The equivalence gate (the linchpin).** `crates/engine-slice/tests/render_instanced.rs` (new, `#[ignore]`d lavapipe class, executed by the new enforced Linux CI step). One fixed population — 10 NPCs + 3 vehicles with varied turns (all four quadrants), scales, tints, a 3-NPC depth stack along the sight line — captured twice through the SAME renderer:

- `ProxyDrawMode::PerInstance`: N single-instance `vkCmdDraw`s, a per-draw byte offset into the same slot region (the pre-6-E per-object submission shape)
- `ProxyDrawMode::Instanced`: one `vkCmdDraw(…, instanceCount = N)` per populated kind

**Measured locally on lavapipe (this pod's user-local ICD, mesa 25.2.8 — the CI runner's noble-updates set): byte-equal captured frames, all 65,536 bytes × both repeats.** Pinned assertions: full-frame byte equality with a first-mismatch report, the per-object/instanced call counts (13 vs 2 — the pinned integers), the ink control (3,728 of 65,536 pixels changed vs the unpopulated base — the comparison is not vacuous), and capture determinism (a repeat instanced capture reproduces the identical bytes).

**The shared-bug hole an A/B gate alone leaves open is closed independently:** two single-instance captures under orthographic cameras (NPC torso +Z wall, vehicle body +Z wall) are checked per-pixel against the 6-B CPU oracle `lit_surface_color(albedo * tint, …)` — the render_corner discipline applied to the instance transform conventions. **Measured: max RGB error 0/255 over 57,600 interior pixels on EACH wall** (a wrong matrix/turn/scale convention would move the wall's screen rectangle and fail position AND color together). The typed refusals (over-capacity total, out-of-table material) are asserted to reach the caller with no poisoned frame state.

**2. Draw-call count.** The pinned-integer headline, in three layers: the pure model `proxy_draw::proxy_draw_call_count` (unit-pinned: 24+2 → 2 instanced / 26 per-object), the integration pins (13 vs 2 in the gate test), and the demo route — `--corner --populated` prints and the new CI step greps: **14 instances (12 npc + 2 vehicle) → 2 draw calls per frame (instanced mode)**; `--proxy-reference` prints 14. `GeometryStats.proxy_draw_calls` surfaces the recorded count per frame (the record step's own return, `debug_assert`-co-pinned against the pure model).

**3. Zero per-frame allocation.** The instance bytes are repacked into the renderer's RETAINED `Vec` (`repack_populations` — the F-05 overlay-vertex pattern, unit-pinned for capacity reuse) and copied into ONE persistently mapped instance buffer with a disjoint region per frame slot (the region/overlay discipline: slot S written only after slot S's fence wait). The mesh VBOs (NPC 108 verts, vehicle 72) are device-local, staged-uploaded once at the FIRST populated frame — lazy, the region-buffer precedent: **the unpopulated corner path pays nothing (render_corner's existing allocation pins are unchanged and re-verified locally)**. **Measured: `renderer.allocations()` constant across warmed populated frames in both modes** (device_memory 18 = the corner's 13 incl. the capture buffer + 5 proxy memory allocations: 2 mesh + 2 staging + 1 instance), and constant again after a forced pipeline replacement (the finding-A re-ensure rebuilds the proxy set before installation; the corner's static 4-tuple survives — asserted; the measured replacement delta is printed, cumulative counters never decrement).

**4. Every existing gate holds.** Local (this pod): `cargo fmt` clean; `cargo clippy --workspace --all-targets --release -- -D warnings` clean; the main shard measured **1142 passed / 2 ignored / 33 result lines** with the workflow's own awk (the 6-E scaffold's 1138 + this PR's 4 platform-uniform engine-renderer tests); the release renderer battery **65 / 2** (61 + the same 4). The heavy shard's passed counts are unchanged (570/569); its new `render_instanced` binary adds +1 ignored / +1 result line (pins 9→10, 57→58). All prior ignored graphical gates re-run green locally on the pod's lavapipe stack (render_corner, render_glyph_atlas, render_orientation, render_overflow, render_scene_separation, render_encounter). engine-core is untouched; fingerprints and selftest pins are unaffected (no sim/persist path changes). Windows = 1087 + the same platform-uniform 4 (inference, to be confirmed by this candidate's CI run).

**5. The rig measurement commands (the CLAIM is the owner's run — lavapipe is a software rasterizer, correctness evidence only, never performance evidence):**

```
# AFTER (the instanced path):
cargo run --release -p engine-slice -- --corner --populated --fps uncapped --validation on
# BEFORE (the same population, one draw per instance):
cargo run --release -p engine-slice -- --corner --populated --proxy-reference --fps uncapped --validation on
# Baseline (unpopulated corner, for context):
cargo run --release -p engine-slice -- --corner --fps uncapped --validation on
```

Each prints the dual-regime series (live TimingSamples bucket bounds AND exact nearest-rank percentiles, n=601). Lavapipe reference numbers from this pod (labeled software-rasterizer, NOT the claim): instanced CPU p50/p95/p99 4.23/35.60/41.28 ms vs per-object 4.27/27.76/39.96 ms — the p50 delta is within this shared pod's noise; **the draw-call count (14→2) is the pinned headline; the CPU-timing claim is rig-pending.**

### Conventions chosen (and why)

- **`MAX_INSTANCES` bounds the TOTAL population per frame (both kinds), not per kind** — the constant documents the sustained-load entity range (1,000–5,000 scene entities); one slot region of the GPU instance buffer holds exactly one bounded population.
- **Fixed per-frame buffer order: npc then vehicle** — each kind's instances are contiguous, so a kind's draw binds one offset; byte-pinned by `repack_matches_allocating_form_in_kind_order`.
- **Proxy resources live in `PipelineState`, lazily** (region precedent), re-ensured on replacement (finding A); the proxy pipeline itself is created in the corner branch of pipeline assembly (every Corner renderer, like the lit pipeline).
- **Per-kind material indices are CALLER policy** (`ProxyPopulation` carries them; validated twice — safe layer and frame flow before any sync state is touched, the capture-refusal discipline). The demo uses facade (1) for NPCs, dark stone (2) for vehicles.
- **`ProxyDrawMode::PerInstance` is public API** — the equivalence gate's N-draws side and the before/after evidence need it from the integration test; production never sets it (documented on the type).
- **Proxy feet at y = 0.02** — on the ground, off its exact plane (the coplanar underside/ground seam); the corner ground draws first in BOTH modes and Vulkan's primitive order (instance order within a draw) keeps every depth decision identical — the byte-equality basis.
- **The demo's population is a fixed 12+2 table** (sidewalk ring + road lanes, clear of the camera route) — pure data, documented per entry. The ladder's next package (corner-scene population) replaces it with the full 24-NPC + 2-vehicle placement table.

### Scope

One package, the brief only: engine-renderer (the instance core's retained repack, the proxy GPU state, the pipeline, the frame-flow wiring, the public API), engine-slice (the demo mode, the gate test, the CLI), the CI pins + two new Linux steps, and CURRENT-STATUS (the update rule). Deferred, untouched: indirect draws (6-E2), GPU culling, animation, crowd LOD. **Never merged by the agent — awaiting the owner's GO.**

---

### Re-verification (fresh, 2026-11-23, via the population branch's tree)

A later overnight session re-ran the gate set on a tree carrying
this commit exactly (the population package's branch, 5c9690d —
its parent is 511b157): fmt clean; clippy `-D warnings` clean;
**main 1142 / 2 / 33**; **release battery 65 / 2**; heavy 576 /
11 / 58 (this PR's 570/10/58 + the population package's +6 passed /
+1 ignored in the existing binaries); the `render_instanced`
equivalence gate green with its pinned evidence lines reproduced
(13 vs 2 draws, byte-equal frames, 3,728 ink pixels, oracle walls
0/255, the replacement lifecycle, warmed allocations constant); the
populated route 601 frames validation 0/0; `render_corner` green
(oracle walls 0/255, the warmed/allocation pins). The claims above
are double-run evidence; the population PR (based on this branch)
carries the fresh numbers.

### Third verification (fresh, 2026-11-23 07:44 UTC, tree 511b157)

A third overnight pass (after the pre-audit corrections, before
any PR opened) re-ran the battery on this exact tree: fmt clean;
clippy `-D warnings` clean; main **1142 / 2 / 33**; release
**65 / 2**; heavy **570 / 10 / 58** — all exactly the pins; the
equivalence gate green with the identical pinned numbers (13 vs 2
draws, byte-equal frames, ink 3,728, oracle walls 0/255 on both
materials, warmed device_memory 18); render_corner and
render_orientation green. Zero drift against both prior runs —
the claims above are triple-run evidence.

### Rig-command pre-flight (verbatim release runs, 2026-11-23 08:03 UTC, tree 511b157)

The 07:57 overnight pass (the fifth, after the 07:50 delivery
rehearsal) executed every rig command above VERBATIM on this
exact tree through the pod's user-local lavapipe — `cargo run
--release -p engine-slice` with the exact flags, release profile
confirmed in the banner (debug_assertions=false; the night's
earlier route logs were debug builds — this closes that gap).
All three completed the full 60 warmup + 601-frame route with
validation warnings/errors 0/0: `--corner --populated` printed
the pinned "14 instances (12 npc + 2 vehicle), 2 draw calls per
frame (instanced mode)" and exited at device_memory 17;
`--corner --populated --proxy-reference` printed "14 draw calls
per frame (per-object reference mode)" (the N side, the same
exit state); the unpopulated baseline printed no population line
and exited at device_memory 12. The rig commands are
copy-paste-and-go. (Raw logs: `~/verify4-rig/` on the overnight
pod.)

### Merge-order dry-run (2026-11-23, a later session — the combined tree, verified)

No prior verification pass had tested whether the three parked branches
merge with each other. `git merge-tree` + a full working-tree
resolution (in a standalone clone — the real repo's object DB and
branches untouched) found this PR's branch conflicts with the 6-F branch in either merge order: **8 files / 31 conflict hunks, and
no conflict-free order exists** (the branches genuinely intersect).
Every hunk is a mechanical union — 6-F was designed against the 6-E
frame-flow shape (its side already passed `None` for the proxy slot),
and the one design point beyond text (`render_frame` carrying BOTH
`Option<&ProxyFrameUpload>` and `Option<&ShadowFrameParams>`, a frame
never carrying both) preserves each mode's exactly-tested path.

The resolved combined tree was FULLY GATED on the pod's user-local
lavapipe (mesa 25.2.8): fmt clean; clippy `--workspace --all-targets
--release -D warnings` clean; actionlint + PyYAML clean on the merged
workflow; **main 1150/2/33, release 73/2, heavy 576/12/59 — 0 failed
anywhere** (the union arithmetic this PR's CI paragraph projected,
now MEASURED on the resolved tree; the Windows pins 1099/575 remain
platform-uniform inferences per convention); ALL lavapipe gate
binaries green with their pinned evidence lines IDENTICAL to each
branch's own runs (both instanced gates — 13-vs-2 and 26-vs-2 draws,
byte-equal frames, ink 3,728/2,745, oracle walls 0/255, warmed
device_memory 18, the replacement lifecycle; the shadows known-answer
— 1,006,779 compared / 41,797 edge-skipped / 0 mismatches /
max |Δdepth| 2.68e-7, stable across the forced replacement; the
shadowed/lit patches 0/255 and 1/255; render_corner's oracles); and
the four release routes green (populated: 26 instances in 2 draw
calls, n=601, validation 0/0; --proxy-reference: 26 draw calls;
--shadows: the cascade banner, n=601, validation 0/0; plain: no
banners, device_memory 12). `--populated --shadows` together is a
typed upfront refusal in the resolution (proxy shadows are the
deferred package on both PRs' deferral lists — never a silent mode
drop).

A verified candidate resolution (patch + git bundle), the merge-tree
census, and every raw gate/route log live at `~/verify5-merge/` on
the pod. The simulation repo: `/tmp/opencode/merge-dryrun` (commit
`76a673e`, based on the 6-E stack at `5c9690d`).

Artifact round-trip (eleventh pass, 2026-11-23 08:49 UTC): the
`resolution.bundle` was fetched into a clean repo holding ONLY its
stated prerequisites (`96dee7c` + `main`) — self-contained, the 6-E
stack's objects ride inside the bundle — and `resolution.patch` was
applied `--index` to a fresh `5c9690d` checkout: both reconstruct
tree `d5dafe8` byte-identically to the sim repo's `76a673e`. (The
sim-repo pointer above lives under `/tmp` and may not survive a pod
restart; the bundle in `~/verify5-merge/` is the durable
reconstruction path — now proven.)

### Branch tip note (added by the 10:13 UTC session, before any PR opened)

The pushed branch tip carries one commit beyond this package: the
run-wide handoff doc (`docs/OVERNIGHT-HANDOFF-2026-11-23.md`, added by
the run's final session at 10:06 UTC, byte-identical on all four
parked branches — `3e5a811` here). It is documentation only — no
code, no pins, no workflow edits — and every measurement in this body
was taken on the package tree below it, which the commit does not
touch. It appears in this PR's diff because the base (`main`) does
not carry it; the stacked PRs' bases do, so it cancels there.

### The four-way union (2026-11-23, the 10:13 UTC session — the 6-E2 stack × 6-F, verified)

The stack gained a member after the dry-run above (6-E2, the indirect
package, riding the population branch) — and the owner's merge order
(stack PRs first, 6-F last) makes 6-F-vs-the-FULL-stack the merge that
matters. The four-way census: **the SAME 8 files / 31 hunks** (the
6-E2 package adds no new conflict region — its indirect arm and 6-F's
shadow arm touch disjoint frame-flow regions). The combined tree
(stack tip `3522e55` ∪ 6-F `4b5b228`, sim commit `9c0cdb8`) was fully
gated on the pod: fmt/clippy clean, **main 1154/2/33, release 77/2,
heavy 576/13/59 — every pin exactly the projected union arithmetic**
(verify5's three-way measured 1150/73/576-12-59; the +4s are 6-E2's
platform-uniform battery), every lavapipe gate green with its
evidence lines IDENTICAL to each branch's own runs (the three-way
26==2==2 byte-equality; the shadows known-answer 1,006,779/41,797/0/
2.68e-7), all routes green in release including the
`--populated --shadows` typed refusal. **One code-level finding,
morning-relevant: the union's `corner_demo::run` crosses clippy's
`too_many_arguments` (8/7 — each branch was at or under 7); resolved
with the repo's own allow pattern (five precedents, `render_frame`
included), documented at the site.** Raw evidence, the resolution
patch and bundle (both round-trip PROVEN to reconstruct tree
`f02ddef` byte-identically): `~/verify7-union/` on the pod.

---

### Session-4/5 payload sync — the body brought level with the branch, the repo made its single source (2026-11-23, the 11:15 UTC session)

This body was assembled at 10:38 UTC, before session 4 committed
the per-branch verification addenda (10:57–11:04); the push
watcher's payloads therefore lagged the branches by one addendum.
Session 5 folded the gap in and committed the result as this file —
the canonical PR body. The watcher payloads and the `~/push-handoff`
copies (on the pod) are regenerated from this file byte-for-byte —
their README pins the sha256 — and the run handoff's morning recipe
now points here (`--body-file docs/PR-BODY-CYCLE6-6E.md`) instead
of at the commit message.

**The canonical rig block** (`docs/WINDOWS-GPU-PROFILING-KIT.md`
§1–2: release build, every run at least twice with both reported,
`--present immediate --fps uncapped` for the ceiling attempt, never
conflate pacing / presentation / GPU time / presented frames).
PowerShell, from the repo root:

```powershell
git checkout cycle6/6e-instanced-draws
cargo build --release
cargo run --release -p engine-slice -- --corner --populated --present immediate --fps uncapped
cargo run --release -p engine-slice -- --corner --populated --proxy-reference --present immediate --fps uncapped
# THE A/B: 14 instances in 2 draw calls (instanced) vs 14 draw calls
# (reference) on THIS branch's 12-NPC + 2-vehicle demo table; the CPU
# p50/p95/p99 lines (live AND exact) are the claim. (The population
# branch runs the identical flags on the full 24+2 table — its PR
# carries that block.)
cargo run --release -p engine-slice -- --corner --populated --validation on
```

`--present immediate` is prefer-when-supported (`config.rs`'s
`PresentModePreference::Immediate`): the pod's offscreen lavapipe
surface falls back to fifo — the banner prints the EFFECTIVE mode —
while the rig's swapchain selects immediate.

**Session 5 pre-flighted this exact block verbatim** on this
branch's tip (2026-11-23 11:12 UTC, release, pod-local lavapipe,
mesa 25.2.8): **rc=0 on all three runs**, headline lines reproduced
exactly — `14 instances (12 npc + 2 vehicle), 2 draw calls per
frame (instanced mode)` / `14 draw calls per frame (per-object
reference mode)` / validation warnings/errors 0/0. Logs:
`~/verify8-session5/session5-preflight-6e.log` on the pod (lavapipe
numbers are correctness evidence only, never the perf claim).

**Session 4's verification, folded in** (the full 64-step table is
the branch's addendum — the docs commit between the handoff doc and
this one): every CI step no prior session had run locally, executed
green — including **the full main shard in a CLEAN environment:
1142 passed / 2 ignored / 33 result lines, rc=0** (the branch pin,
a second independent confirmation of session 3's record), the
renderer release battery **65/2**, the 13-instance
`render_instanced` gate, the populated route (14 instances, 2
draws, validation 0/0), the proxy battery 13/13, and every pinned
selftest byte-identical, double-run equal.

**The tip statement (supersedes the Branch tip note above):** the
pushed tip is this branch's head = the package commit `511b157` +
the run-wide handoff doc (`3e5a811`) + session 4's addendum
(`14d30f9`) + session 5's payload-sync commit (the one adding this
file). Every commit beyond `511b157` is documentation only —
`git diff --name-only 511b157..HEAD` lists only
`docs/OVERNIGHT-HANDOFF-2026-11-23.md` and this file; no code, no
workflow, no pins. This PR's base is `main`, so the doc commits
appear in the diff (nothing cancels); they are the run's audit
record, not scope creep.

---

### Session 6 — the heavy shard's direct clean-environment run (2026-11-23, the ~11:30 UTC session)

The one second-run gap this branch still carried: the heavy shard
had session 3's single direct measurement (570/10/58), taken
before the clean-environment discipline the 6-F lane's polluted-env
FAIL established — the main shard already had session 4's direct
clean-environment run on this tree. Session 6 closed the heavy gap:
the CI command verbatim (`cargo test -p engine-slice --features
engine-slice/crash-test-seams`), the workflow's own awk counting,
in a shell with no SDL/VK variables, directly on this branch's tip
(`f38c1c2` at run time; the code tree is exactly `511b157`'s —
every commit above the package commit is documentation only):

| Step (MEASURED this session, this tip) | Result |
|---|---|
| `cargo fmt --all -- --check`; clippy `-D warnings` in both forms (the CI debug form, the standing-order release form) | clean |
| **the full heavy shard** | **570 passed / 10 ignored / 58 result lines, rc=0, zero failures — the branch pin, its first post-clean-discipline direct run (session 3's 570/10/58 re-confirmed on this tree)** |

Raw log: `/tmp/opencode/session6-6e-heavy.log` on the pod (a /tmp
path — may not survive a pod restart; the numbers here are the
durable record). The watcher payload and its `~/push-handoff` twin
were regenerated from this file byte-for-byte after this section
landed.

**The tip statement (supersedes the one above):** the pushed tip is
this branch's head = the package commit `511b157` + the run-wide
handoff doc (`3e5a811`) + session 4's addendum (`14d30f9`) +
session 5's payload-sync commit (`f38c1c2`) + session 6's
heavy-shard verification addendum (the commit adding this section).
Every commit beyond `511b157` is documentation only — `git diff
--name-only 511b157..HEAD` lists only
`docs/OVERNIGHT-HANDOFF-2026-11-23.md` and this file; no code, no
workflow, no pins.
