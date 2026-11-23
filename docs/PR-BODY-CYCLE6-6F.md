## Cycle 6-F: directional shadows — one cascade, the corner scene, the deterministic core first

Implements ladder item 2 (`docs/AUTONOMOUS-CYCLE-PROTOCOL.md` §2): the light-space orthographic projection, the depth-bias math, and the cascade bounds as **pure, unit-pinned functions first** (the `proxy_instances.rs` pattern), then the depth-only shader, then the pipeline, then the gates. **The shadow map is pinned texel-by-texel against a CPU raycaster at float epsilon, and the shadowed pixel output is pinned against the 6-B CPU oracle.**

### The deterministic core (`engine-renderer/src/shadows.rs`)

- `cascade_for` — the light-space basis (deterministic reference-axis flip for near-vertical lights), the scene AABB's 8 corners projected in fixed order, the world→light-clip matrix. **Depth convention: closest-to-light = 0, clear = 1.0** (camera-like LESS). The *inverted* form was this build's one real convention bug — it made the ground win the depth test over every roof; a vertical-light probe caught it (the whole map read the ground's depth) and the fixed form is pinned by the matrix-coverage and synthetic-cube unit tests.
- `depth_bias` — the compare-time slope-scaled bias (the `vkCmdSetDepthBias` equation's software form), applied at **compare** time in both the shader and the CPU oracle from the same FrameParams-carried constants; the depth-only pass writes **unbiased** depths so every texel stays CPU-predictable.
- The CPU raycaster (`is_shadowed` — the shadow-*testing* oracle, ground ignored as an occluder, self-hit epsilon) and **`map_texel_depth`** (the *map* oracle: the texel's ray cast from the far plane down along −light — the first front-facing surface, the ground quad included, corner-graze and far-plane-roof cases handled).

**8 unit tests** (all 8 platform-uniform and all counted by CI — the main and release shards each grow by the same 8; the workflow's comment chain documents the arithmetic): basis orthonormality, matrix coverage, the synthetic cube (hand-computed roof depth 0.25), the corner census, the raycaster's synthetic geometry and the corner's **signature shadow** (the day 45° sun with the 22 m SW mass shadows the cross center), the texel round-trip, the bias model.

### The GPU path

- `shaders/shadow_depth.wgsl` — depth-only: the corner mesh through the light matrix, no color attachments, no write-time bias.
- `shaders/lit_scene_shadowed.wgsl` — lit_scene's equation with the direct terms gated by a **hard 0/1 shadow factor from an exact `textureLoad`** (no sampler, no compare-op magic — the compare is plain arithmetic mirroring the CPU oracle, bias included; the map's y-flip and edge clamping accounted and mirrored).
- `shadow_draw.rs` — the lazy shadow state: **per-slot ping-pong maps** (D32_SFLOAT; slot S's depth pass runs only inside the frame that waited slot S's fence), the depth-only render pass with the **external subpass dependency** (LATE_FRAGMENT_TESTS/DEPTH_WRITE → FRAGMENT_SHADER/SHADER_READ) ordering the map's writes before the main pass's reads, the shadowed lit pipeline, per-material×slot descriptor sets (binding 4 = the slot's map view), the mapped extended-FrameParams uniform, and the **known-answer readback** (CopyImageToBuffer on the depth aspect; the single-flight handshake lives in FrameState so a request survives pipeline replacements — found by the gate itself).
- The frame flow records the depth pass before the main pass; the unshadowed path is untouched — `render_corner`'s pixels and allocation pins byte-identical (the regression pin below).

### The gates — measured on this pod's user-local lavapipe (mesa 25.2.8, the CI runner's noble-updates set; correctness evidence, the rig is authoritative for perf)

**The map known-answer** (`tests/render_shadows.rs`, `#[ignore]`d lavapipe class + the new enforced Linux CI step): **1,006,779 of 1,048,576 texels compared, 0 mismatches, max |Δdepth| 2.68e-7** (float epsilon); 41,797 texels (3.99%) edge-skipped — the perimeter and corner-graze bands where rasterization coverage is implementation territory; 536,888 texels hold occluders (the vacuous-gate guard); **the identical result after a forced pipeline replacement** (the finding-A re-ensure).

**The pixel equivalence:** a fully-shadowed ground patch (the cross center — inside the SW mass's signature shadow; the fixture self-validates all five probes through the CPU oracle before asserting) renders **exactly `albedo · ambient`: max RGB error 0/255 over 57,600 pixels**; a fully-lit patch renders the full unshadowed equation through the shadowed pipeline: **max error 1/255**; the shadowed-vs-unshadowed frame differ: all 65,536 pixels; the unshadowed path **byte-identical** before and after everything.

**Zero per-frame allocation:** warmed shadowed frames hold the counters exactly (device_memory 17 = the corner's 13 incl. capture + 4: two maps, the extended uniform, the readback); the unshadowed corner's pins (4, 2, 4, 2) hold **before** the first shadowed frame (the lazy discipline).

**The route:** `--corner --shadows` — 601 frames, validation warnings/errors 0/0, no teardown leaks (the depth pass's render-pass release was the one leak the first run's exit validation caught). Rig commands: `cargo run --release -p engine-slice -- --corner --shadows --fps uncapped --validation on` (vs the same without `--shadows`).

### Conventions chosen (and why)

- **Compare-time analytical bias, not write-time `vkCmdSetDepthBias`** — keeps the map exactly CPU-predictable (the known-answer gate) and the bias expression shared verbatim by shader and oracle.
- **Hard shadows, one 1024² cascade, `textureLoad`** — the 6-F scope; PCF/softening, cascaded splits, and proxy shadows are open follow-ups (proxy shadows ride on the population package).
- **D32_SFLOAT required** — one f32 per texel, no UNORM quantization between the rasterized depth and the compared/read-back value.
- **The demo bias** (constant 0.0015 / slope 0.002) documented on `corner_demo::demo_bias` — roughly two texel quanta of floor; the gate's margins measure both acne and peter-panning.
- **Branch baseline:** pins computed from `main` @ `1fe89b7` (the 6-E scaffold baseline, before the 6-E implementation PR's +4). If 6-E lands first, the rebase adds its delta (1150/1099 main, 73 release, heavy ignored 11/lines 59) — the workflow's comment chains state both branches' arithmetic.

### CI

Main shard 1138→**1146** (Windows 1087→**1095**, platform-uniform +8); release battery 61→**69** (the same 8); heavy passed unchanged (570/569) with `render_shadows` adding +1 ignored (9→**10**) / +1 result line (57→**58**); the new enforced Linux gate step; the shadowed route joins the day/night measurement step (n=601 greps + the cascade line). CURRENT-STATUS.md updated per its rule (the capability bullet, the matrix row, the test table).

**Never merged by the agent — awaiting the owner's GO.**

---

### Re-verification (fresh, 2026-11-23 07:15 UTC, tree 96dee7c)

A later overnight session re-ran the full gate set on this exact
parked tree (pod-local, same user-local lavapipe, mesa 25.2.8):
fmt clean; clippy `-D warnings` clean; **main 1146 / 2 / 33**;
**release battery 69 / 2**; **heavy 570 / 10 / 58** (all three
exactly the branch's pins); the `render_shadows` known-answer gate
identical to the original run (**1,006,779 compared / 41,797
edge-skipped (3.99%) / 0 mismatches / max |Δdepth| 2.68e-7**, and
identical again after the forced replacement); both pixel oracles
(shadowed 0/255, lit 1/255, each over 57,600 pixels); the
shadowed-vs-unshadowed frame differ 65,536/65,536; warmed shadowed
allocations device_memory 17 (constant); `render_corner`'s
unshadowed path green with its own pins; the `--corner --shadows`
route 601 frames, validation warnings/errors 0/0, device_memory 16
at exit. Every number above matches the original measurement — the
payload's claims are now double-run evidence.

### Pre-audit correction (07:18 UTC session, before this PR opened)

The CI paragraph and the core's test-count parenthetical above
carried stale arithmetic from an earlier draft (main 1140 / Windows
1089 / +2, release 63, post-6-E-rebase 1144/1093/65). Corrected to
the measured values this morning, before the PR was created: main
**1146 / 1095** (+8), release **69** (the same +8), post-6-E-rebase
**1150 / 1099 / 73**. The shipped commit's workflow pins and comment
chains, the fresh gate logs, and the re-verification section above
all carried the correct +8 throughout — only this body text was
stale. (The 6-E body's Windows line also gained its missing "to
be": that CI run is pending, not confirming.)

### Third verification (fresh, 2026-11-23 07:39 UTC, tree 96dee7c)

A third overnight pass (after the pre-audit corrections) re-ran
the battery on this exact tree: fmt clean; clippy `-D warnings`
clean; main **1146 / 2 / 33**; release **69 / 2**; heavy
**570 / 10 / 58** — all exactly the pins; the known-answer gate
**identical to both prior runs** (1,006,779 compared / 41,797
edge-skipped (3.99%) / 0 mismatches / max |Δdepth| 2.68e-7,
identical after the forced replacement; both pixel oracles 0/255
and 1/255; 65,536/65,536 frame differ; warmed device_memory 17);
all six other gate binaries green. Zero drift — the claims above
are triple-run evidence.

### Rig-command pre-flight (verbatim release runs, 2026-11-23 08:03 UTC, tree 96dee7c)

The 07:57 overnight pass (the fifth, after the 07:50 delivery
rehearsal) executed the rig command above VERBATIM on this exact
tree through the pod's user-local lavapipe — `cargo run --release
-p engine-slice -- --corner --shadows --fps uncapped --validation
on`, release profile confirmed in the banner (debug_assertions=
false; the night's earlier route logs were debug builds — this
closes that gap): the full 60 warmup + 601-frame route, the
"directional shadows: one cascade (1024 texels), bias constant
0.0015 / slope 0.0020" banner, validation warnings/errors 0/0,
exit device_memory 16 (this payload's own pinned exit count).
The comparison side (the same command without `--shadows`) ran
clean too: no shadows banner, validation 0/0, exit device_memory
12 — the lazy discipline holding on the unshadowed path (no
shadow resources allocated). Copy-paste-and-go. (Raw logs:
`~/verify4-rig/` on the overnight pod.)

### Merge-order dry-run (2026-11-23, a later session — the combined tree, verified)

No prior verification pass had tested whether the three parked branches
merge with each other. `git merge-tree` + a full working-tree
resolution (in a standalone clone — the real repo's object DB and
branches untouched) found this PR's branch conflicts with the 6-E stack (6-E + the population package) in either merge order: **8 files / 31 conflict hunks, and
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
parked branches — `4b5b228` here). It is documentation only — no
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
now points here (`--body-file docs/PR-BODY-CYCLE6-6F.md`) instead
of at the commit message.

**THE COMMIT-MESSAGE CORRECTIVE (read this body, not the message's
CI paragraph):** `96dee7c`'s message says "the main shard 1138 →
1140 (Windows 1087 → 1089, the same platform-uniform 2); the
release battery 61 → 63". That paragraph is STALE — it was written
before the `shadows.rs` battery grew from 2 to 8 tests during the
INVERTED-depth convention-bug hunt the same message describes, and
it was never refreshed; its numbers were never the measured state
of any tree on this branch. The measured tip state (session 4,
clean environment, the workflow's own awk): **main 1146 passed /
2 ignored / 33 result lines, rc=0; renderer release battery 69/2;
targeted shadows battery 8/8** — exactly the branch's workflow pins
and comment chain ("Linux 1146 = 1138 + the same 8; Windows 1095 =
1087 + the same 8", "61 → 69"). This body's own CI section was
corrected in place at 07:18 UTC (the pre-audit correction below);
the commit message retains the stale paragraph. Full record: the
6-F branch's session-4 addendum commit (`1b05252`).

**The canonical rig block** (`docs/WINDOWS-GPU-PROFILING-KIT.md`
§1–2: release build, every run at least twice with both reported,
`--present immediate --fps uncapped` for the ceiling attempt, never
conflate pacing / presentation / GPU time / presented frames).
PowerShell, from the repo root:

```powershell
git checkout cycle6/6f-directional-shadows
cargo build --release
cargo run --release -p engine-slice -- --corner --shadows --present immediate --fps uncapped
cargo run --release -p engine-slice -- --corner --present immediate --fps uncapped
# THE A/B: the depth pass + shadowed-lit cost vs the unshadowed route.
cargo run --release -p engine-slice -- --corner --shadows --validation on
```

`--present immediate` is prefer-when-supported (`config.rs`'s
`PresentModePreference::Immediate`): the pod's offscreen lavapipe
surface falls back to fifo — the banner prints the EFFECTIVE mode —
while the rig's swapchain selects immediate.

**Session 5 pre-flighted this exact block verbatim** on this
branch's tip (2026-11-23 11:13 UTC, release, pod-local lavapipe,
mesa 25.2.8): **rc=0 on all three runs**, headline lines reproduced
exactly — `directional shadows: one cascade (1024 texels), bias
constant 0.0015 / slope 0.0020` on both shadowed runs, validation
warnings/errors 0/0 on all three. Logs:
`~/verify8-session5/session5-preflight-6f.log` on the pod (lavapipe
numbers are correctness evidence only, never the perf claim).

**Session 4's verification, folded in** (the full table is the
branch's addendum — the docs commit between the handoff doc and
this one): every CI step no prior session had run locally, executed
with zero code failures and zero flakes — the clean-environment
full main shard **1146/2/33 rc=0**, the renderer release battery
**69/2**, the targeted shadows battery **8/8**, the `render_shadows`
gate (map known-answer + pixel equivalence) and the shadowed route
re-runs. One lane FAIL, root-caused per §3.1 discipline and NOT a
code fault: the main-shard step initially failed inside the
exported lavapipe environment — engine-shell's relative-mouse test
(a package this diff does not touch) cannot capture relative mouse
under `SDL_VIDEODRIVER=offscreen`; the clean re-run is the
CI-parity record, and CI is unaffected (the workflow never sets
`SDL_VIDEODRIVER` on the Tests step). The operational lesson (CPU
shards in a clean env, lavapipe vars only for graphical commands)
is on record in the addendum.

**The tip statement (supersedes the Branch tip note above):** the
pushed tip is this branch's head = the package commit `96dee7c` +
the run-wide handoff doc (`4b5b228`) + session 4's addendum
(`1b05252`) + session 5's payload-sync commit (the one adding this
file). Every commit beyond `96dee7c` is documentation only —
`git diff --name-only 96dee7c..HEAD` lists only
`docs/OVERNIGHT-HANDOFF-2026-11-23.md` and this file; no code, no
workflow, no pins. This PR's base is `main`, so the doc commits
appear in the diff (nothing cancels); they are the run's audit
record, not scope creep.

---

### Session 6 — the heavy shard's direct clean-environment run (2026-11-23, the ~11:30 UTC session)

The one second-run gap this branch still carried: the heavy shard
had session 3's single direct measurement (570/10/58), taken
before the clean-environment discipline THIS lane's own polluted-env
FAIL established (session 4's first main-shard attempt here failed
inside the exported lavapipe environment — engine-shell's
relative-mouse test, a package this diff does not touch — and the
root-cause produced the run-wide operational lesson; the clean
re-run 1146/2/33 is the record). Session 6 closed the heavy gap the
same way the lesson prescribes: the CI command verbatim (`cargo
test -p engine-slice --features engine-slice/crash-test-seams`),
the workflow's own awk counting, in a shell with no SDL/VK
variables, directly on this branch's tip (`d53fa2e` at run time;
the code tree is exactly `96dee7c`'s — every commit above the
package commit is documentation only):

| Step (MEASURED this session, this tip) | Result |
|---|---|
| `cargo fmt --all -- --check`; clippy `-D warnings` in both forms (the CI debug form, the standing-order release form) | clean |
| **the full heavy shard** | **570 passed / 10 ignored / 58 result lines, rc=0, zero failures — the branch pin, its first post-clean-discipline direct run (session 3's 570/10/58 re-confirmed on this tree)** |

Raw log: `/tmp/opencode/session6-6f-heavy.log` on the pod (a /tmp
path — may not survive a pod restart; the numbers here are the
durable record). The watcher payload and its `~/push-handoff` twin
were regenerated from this file byte-for-byte after this section
landed.

**The tip statement (supersedes the one above):** the pushed tip is
this branch's head = the package commit `96dee7c` + the run-wide
handoff doc (`4b5b228`) + session 4's addendum (`1b05252`) +
session 5's payload-sync commit (`d53fa2e`) + session 6's
heavy-shard verification addendum (the commit adding this section).
Every commit beyond `96dee7c` is documentation only — `git diff
--name-only 96dee7c..HEAD` lists only
`docs/OVERNIGHT-HANDOFF-2026-11-23.md` and this file; no code, no
workflow, no pins.

---

### Session 7 — the linchpin gate's third direct run (2026-11-23, the ~11:54 UTC session)

The evidence table's one remaining asymmetry after session 6: this
branch's graphical linchpin (`render_shadows`, the known-answer +
pixel-equivalence gate) had two direct measurements (session 3's
final-tree run, session 4's tip re-run) while the population
branch's linchpin carried three. Session 7 closed the asymmetry —
the gate run a third time, directly on this branch's tip (`e858f40`
at run time; the code tree is exactly `96dee7c`'s — every commit
above the package commit is documentation only), in the scoped
environment (the lavapipe variables on the gate command only, per
session 4's operational lesson — the lesson this branch's own lane
established):

| Assertion (MEASURED this session, this tip) | Result |
|---|---|
| `render_shadows --ignored` | **1 passed, rc=0 — every pinned evidence line reproduced exactly** |
| the map known-answer | 1048576 texels, 1006779 compared, 41797 edge-skipped (3.99%), **0 mismatches, max \|Δdepth\| 2.68e-7** — against the CPU raycaster |
| the pixel oracles | ground patch [0.0, 0.0] (shadowed): 57600 pixels, max RGB error 0/255; ground patch [12.0, 3.0] (lit): 57600 pixels, max RGB error 1/255 |
| the visible-change guard | 65536 of 65536 frame pixels changed by the shadow (both directions guarded) |
| the allocation gate | warmed `AllocationStats { device_memory: 17, … }` constant across warmed frames |
| the replacement lifecycle | the known-answer re-verified after pipeline replacement: 1006779 / 41797 / 0 / 2.68e-7, identical |

Raw log: `/tmp/opencode/session7-6f-gate.log` on the pod (a /tmp
path — may not survive a pod restart; the numbers here are the
durable record). The watcher payload and its `~/push-handoff` twin
were regenerated from this file byte-for-byte after this section
landed. The linchpin now holds three direct measurements on this
branch's code tree, matching the population branch's count.

**The tip statement (supersedes the one above):** the pushed tip is
this branch's head = the package commit `96dee7c` + the run-wide
handoff doc (`4b5b228`) + session 4's addendum (`1b05252`) +
session 5's payload-sync commit (`d53fa2e`) + session 6's
heavy-shard verification addendum + session 7's linchpin
verification addendum (the commit adding this section). Every
commit beyond `96dee7c` is documentation only — `git diff
--name-only 96dee7c..HEAD` lists only
`docs/OVERNIGHT-HANDOFF-2026-11-23.md` and this file; no code, no
workflow, no pins.
