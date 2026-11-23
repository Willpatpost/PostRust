## Cycle 6-E ladder package 3: the corner-scene population — the full 24-NPC + 2-vehicle placement table through the instanced path

Rides on the 6-E branch (this PR's base): the instanced path is 6-E's;
this package replaces the demo's 12-NPC + 2-vehicle fixed table with
the ladder's full deterministic placement table — pure data, documented
per entry, pinned every way the repo knows how.

### What changed

- **`crates/engine-slice/src/corner_population.rs` (new)** — the
  placement table: 24 NPCs + 2 vehicles as two const arrays (fixed
  order: npc then vehicle, the repack's buffer order), a one-line
  accessor, and a six-test validity battery. NOT seeded (the corner
  scene's own no-seeding discipline); hand-authored placements in the
  intersection's road-cross frame with per-entry comments.
- **The facing convention, derived instead of labeled**: the proxy
  meshes' local forward is local +Z (the axis the 6-E oracle walls
  face); the instance matrix's third column is (sin, 0, cos), so turn
  0 → +Z (north), 16384 → +X (east), 32768 → −Z (south), 49152 → −X
  (west). Every entry's comment states its facing from this table.
  **The replaced 12-NPC table's facing labels did not all hold under
  any single mesh-forward choice** (e.g. its "North sidewalk … facing
  the road (−Z)" entries carry turn 49152, which faces +Z; its
  "westbound" vehicle carries turn 16384, which faces east) — this
  package derives each facing instead of asserting it by vibe.
- **`corner_demo.rs`** — `demo_population` deleted; `--corner
  --populated` draws `corner_population()` through the unchanged
  instanced path. The help text names the 24+2 population.
- **`tests/render_instanced.rs`** — a second `#[ignore]`d lavapipe
  gate: the shipped table through BOTH submission modes. The binary
  now carries two SDL-touching tests, so it gains the engine-shell
  test binaries' `sdl_lock` serialization pattern (the sdl3 crate
  declares one main thread per process; two parallel SDL inits in one
  process collide — the guard drops after the app, so SDL fully quits
  between tests; the CI step's command is unchanged).

### The placement rules (each pinned by a unit test)

- Pedestrians on the sidewalk ring (the 2 m band around the 8 m road
  cross) or mid-crosswalk; the footprint circle (r = 0.28 m, the NPC
  proxy's worst corner) stays fully off the road for non-crossers,
  inside the crosswalk bands for the two crossers; feet at y = 0.02
  (the 6-E seam convention); uniform scale 1.0 (a placement table,
  not a height sampler); cardinal facings only.
- Vehicles in their right-hand-traffic lanes (westbound keeps the
  north half z ∈ (0,4); northbound the east half x ∈ (0,4)), clear of
  the intersection box, the buildings, the pedestrians, and each
  other (≥ 0.5 m footprint-inclusive, exact axis-aligned math).
- **Every placement clears the camera route by ≥ 1.0 m,
  footprint-inclusive** (pedestrians as footprint circles, vehicles
  as their cardinal-turn road boxes, against the route's own waypoint
  polyline — exact point/segment and segment/segment distances). The
  replaced table's tightest approach was 0.26 m; this table's
  tightest is 1.1 m (the departed northbound sedan beside the exit
  leg).
- Pedestrians never overlap: ≥ 0.7 m center-to-center against the
  0.56 m bounding-circle contact bound.
- **The placement-table byte pin**: the full population's packed
  bytes (through `repack_populations` — which validates every
  instance before a byte exists) hash to FNV-1a
  `9133392768426866649` (measured actual; pure-IEEE packing, so every
  platform's CI asserts the same value — the scaffold's
  `packed_bytes_fnv_pin` discipline applied to the table: any hand
  edit, even one tint channel, is a conscious repin).

### The evidence (MEASURED on this pod's user-local lavapipe — mesa 25.2.8; software-rasterizer correctness evidence, the perf CLAIM is the owner's rig run)

- **The full-table gate** (`full_corner_population_matches_per_object_pixels`,
  executed by the existing enforced Linux CI step): 26 instances
  (24 npc + 2 vehicle) — per-object **26 draws** vs instanced **2
  draws**, **byte-equal captured frames** (all 65,536 bytes,
  repeat-stable); 2,745 of 65,536 pixels changed vs the unpopulated
  base (the ink control, floor pinned at 1,300); **allocations
  constant across warmed populated frames in both modes**
  (device_memory 18 with the capture buffer — the same total as the
  13-instance fixture: the population's size changes no allocation);
  validation errors 0.
- **The existing 6-E gate re-verified green, byte-for-byte the same
  behavior** (13 instances, 3,728 ink pixels, oracle walls 0/255 max
  error, replacement lifecycle) — untouched by this diff.
- **The populated route** (`--corner --populated`, 601 frames,
  validation warnings/errors 0/0): **26 instances in 2 draw calls per
  frame**; allocations device_memory 17 at exit. The reference mode
  (`--proxy-reference`) runs the identical route at **26 calls** with
  the identical allocation set. Lavapipe exact CPU p50/p95/p99:
  instanced 4.30/31.19/40.82 ms vs per-object 4.81/22.08/34.22 ms —
  **this shared pod's p95/p99 are contended tonight and NOT the
  claim**; the draw-call count (26 → 2) is the pinned headline, the
  CPU-timing claim is rig-pending.
- **The pure-model pin now matches the shipped table**:
  `proxy_draw_call_count(false, 24, 2) == 2` / `(true, 24, 2) == 26`
  was already pinned in engine-renderer's policy tests at the 6-E
  implementation — this table makes the demo live up to it.

### The rig measurement commands (the CLAIM is the owner's run)

```
# AFTER (the instanced path, the full population):
cargo run --release -p engine-slice -- --corner --populated --fps uncapped --validation on
# BEFORE (the same population, one draw per instance):
cargo run --release -p engine-slice -- --corner --populated --proxy-reference --fps uncapped --validation on
```

### CI pins (measured, never weakened)

- Main shard: **1142 / 2 / 33 — unchanged** (engine-renderer is
  untouched by this diff; MEASURED locally with the workflow's own
  awk).
- Heavy shard: **576 / 11 / 58** (the prior 570/10/58 + the
  six-test placement battery in the existing lib result line, and
  the second `render_instanced` gate's +1 ignored; result lines
  unchanged — the same binary). MEASURED locally with the workflow's
  own awk; Windows 575 = the prior 569 + the same 6 (platform-
  uniform inference: margin-based pins, pure-IEEE byte pin — to be
  confirmed by this candidate's CI run; a mismatch is corrected from
  actuals, never by weakening a test).

### Scope

One package, the ladder item only: engine-slice (the table module,
the demo wiring, the second gate + the sdl_lock pattern), the CI
heavy-shard pins + comment chains, and CURRENT-STATUS (the update
rule). engine-renderer, engine-core, and every fingerprint untouched.

**Every existing gate holds** — MEASURED fresh on this exact tree
(2026-11-23, pod-local): `cargo fmt` clean; `cargo clippy
--workspace --all-targets --release -- -D warnings` clean; the main
shard **1142 / 2 / 33** (unchanged — engine-renderer untouched);
the release renderer battery **65 / 2**; the heavy shard **576 /
11 / 58**; and EVERY prior ignored graphical gate re-run green on
the pod's lavapipe stack (render_corner, render_glyph_atlas,
render_orientation, render_overflow, render_scene_separation,
render_encounter — the unpopulated paths are byte-for-byte the same
code this diff leaves alone). engine-core untouched; fingerprints
and selftest pins unaffected.

Deferred, untouched: indirect draws (6-E2 — gated on 1–3 CI-green),
GPU culling, animation, crowd LOD. **Never merged by the agent —
awaiting the owner's GO.**

### Third verification (fresh, 2026-11-23 07:34 UTC, routes to 07:45, tree 5c9690d)

A third overnight pass re-ran the battery on this exact tree: fmt
clean; clippy `-D warnings` clean; main **1142 / 2 / 33**
(unchanged); release **65 / 2**; heavy **576 / 11 / 58** — all
exactly the pins; BOTH instanced gates green with identical
pinned numbers (the 13-instance fixture: 13 vs 2 draws, ink
3,728; the full table: 26 instances, 26 vs 2 draws, byte-equal,
ink 2,745; warmed device_memory 18 in both modes); all six other
gate binaries green; both routes fresh (`--corner --populated`:
"26 instances (24 npc + 2 vehicle), 2 draw calls per frame", 601
frames, validation 0/0; `--proxy-reference`: "26 draw calls per
frame", validation 0/0 — the N-vs-1 headline confirmed at route
level in both directions). Zero drift against both prior runs —
the claims above are triple-run evidence.

### Rig-command pre-flight (verbatim release runs, 2026-11-23 08:03 UTC, tree 5c9690d)

The 07:57 overnight pass (the fifth, after the 07:50 delivery
rehearsal) re-audited the rig commands against the logs of the
third verification pass and found them ALREADY verbatim-release
evidence: the `Running` lines in `~/verify3/pop-route.log` and
`pop-route-ref.log` match the two commands above character for
character (`cargo run --release`, debug_assertions=false), both
completing 60 warmup + 601 frames with validation 0/0 and the
pinned population lines ("26 instances (24 npc + 2 vehicle),
2 draw calls per frame" / "26 draw calls per frame"). The tree
is byte-identical to that pass's HEAD (5c9690d — identical
inputs, identical results), so no re-run was needed; the same
pass closed the remaining gap on the OTHER two payloads' trees
(see their pre-flight sections). Copy-paste-and-go. (Raw logs:
`~/verify3/pop-route*.log` on the overnight pod.)

### Merge-order dry-run (2026-11-23, a later session — the combined tree, verified)

No prior verification pass had tested whether the three parked branches
merge with each other. `git merge-tree` + a full working-tree
resolution (in a standalone clone — the real repo's object DB and
branches untouched) found this PR's stack (it rides on the 6-E branch) conflicts with the 6-F branch in either merge order: **8 files / 31 conflict hunks, and
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
parked branches — `cb92088` here). It is documentation only — no
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
now points here (`--body-file docs/PR-BODY-CORNER-POPULATION.md`)
instead of at the commit message.

**The canonical rig block** (`docs/WINDOWS-GPU-PROFILING-KIT.md`
§1–2: release build, every run at least twice with both reported,
`--present immediate --fps uncapped` for the ceiling attempt, never
conflate pacing / presentation / GPU time / presented frames).
PowerShell, from the repo root:

```powershell
git checkout cycle6/corner-population
cargo build --release
cargo run --release -p engine-slice -- --corner --populated --present immediate --fps uncapped
cargo run --release -p engine-slice -- --corner --populated --proxy-reference --present immediate --fps uncapped
# THE A/B: 26 instances (24 npc + 2 vehicle) in 2 draw calls
# (instanced) vs 26 draw calls (reference); the CPU p50/p95/p99
# lines (live AND exact) are the claim.
cargo run --release -p engine-slice -- --corner --populated --validation on
```

`--present immediate` is prefer-when-supported (`config.rs`'s
`PresentModePreference::Immediate`): the pod's offscreen lavapipe
surface falls back to fifo — the banner prints the EFFECTIVE mode —
while the rig's swapchain selects immediate.

**Session 5 pre-flighted this exact block verbatim** on this
branch's tip (2026-11-23 11:12 UTC, release, pod-local lavapipe,
mesa 25.2.8): **rc=0 on all three runs**, headline lines reproduced
exactly — `26 instances (24 npc + 2 vehicle), 2 draw calls per
frame (instanced mode)` / `26 draw calls per frame (per-object
reference mode)` / validation warnings/errors 0/0. Logs:
`~/verify8-session5/session5-preflight-population.log` on the pod
(lavapipe numbers are correctness evidence only, never the perf
claim).

**Session 4's verification, folded in** (the full 65-step table is
the branch's addendum — the docs commit between the handoff doc and
this one): every CI step no prior session had run locally, executed
green — including the full main shard in a CLEAN environment
(**1142/2/33 rc=0**, measured on the 6-E tree whose main-shard test
set this branch shares exactly — this package adds no main-shard
tests), the renderer release battery **65/2**, the placement
battery 6/6, BOTH `render_instanced` gates (the 13-instance fixture
and this package's full 24+2 table, 26-vs-2 byte-equal), the
populated route (26 instances, 2 draws, validation 0/0), and every
pinned selftest byte-identical, double-run equal.

**The tip statement (supersedes the Branch tip note above):** the
pushed tip is this branch's head = the package commit `5c9690d` +
the run-wide handoff doc (`cb92088`) + session 4's addendum
(`4c5edbf`) + session 5's payload-sync commit (the one adding this
file). Every commit beyond `5c9690d` is documentation only —
`git diff --name-only 5c9690d..HEAD` lists only
`docs/OVERNIGHT-HANDOFF-2026-11-23.md` and this file; no code, no
workflow, no pins. This PR's base is the 6-E branch's tip, so the
diff is the population package commit plus the handoff-doc
evolution between the two branches' addenda (documentation only —
the 6-E code itself sits in the base and is not re-diffed here).
