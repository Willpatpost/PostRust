## Cycle 6-E2: indirect draws — one `vkCmdDrawIndirect` per proxy kind (GPU-side counts)

The ladder's stretch package (AUTONOMOUS-CYCLE-PROTOCOL §2 item 4), riding
the population branch (this PR's base): the populated corner's proxy
submission stops carrying the instance counts in the command stream — one
`vkCmdDrawIndirect` per populated kind reads them from GPU memory at draw
time. The draw-call headline is unchanged (**26 instances stay 2 draws**);
what changes is WHERE N lives — exactly the property the deferred
GPU-culling package needs (a GPU pass rewriting counts without re-recording
anything). This package's count author is the CPU (the frame flow, after
the slot's fence wait); the mechanism — `vkCmdDrawIndirect` reading a
buffer — is the deliverable. GPU-side count AUTHORSHIP stays deferred.

**Branch tip note:** the branch carries the package commit `6ee89b4` plus
`3522e55`, the run-wide handoff doc (`docs/OVERNIGHT-HANDOFF-2026-11-23.md`,
byte-identical on all four parked branches, committed by the final session
of the run). Against this PR's base (which carries the same doc) the diff
cancels to **the package commit only**.

### What changed

- **`crates/engine-renderer/src/proxy_indirect.rs` (new)** — the
  deterministic command core (the §1 pattern, `proxy_instances`' shape):
  the packed `VkDrawIndirectCommand` bytes — four `u32` fields
  (`vertexCount`, `instanceCount`, `firstVertex`, `firstInstance`) in
  fixed order, `to_le_bytes`, no padding, no floats, no libm —
  byte-identical on every platform. **`firstVertex`/`firstInstance` are
  PINNED ZERO**: each kind draws its own bound VBO at offset 0 and the
  per-kind instance range arrives through the instance-rate vertex-binding
  OFFSET (the 6-E mechanism, unchanged), so the optional
  `drawIndirectFirstInstance` feature is deliberately NOT required — the
  path runs on every conformant implementation including lavapipe (the CI
  gate's rasterizer). One fixed 32-byte block per frame slot (npc command
  at +0, vehicle at +16 — the repack's kind order). Four platform-uniform
  tests: the packing determinism with the pinned 108/72 mesh vertex
  counts; the command-bytes FNV-1a cross-platform pin (the shipped 24+2
  population's block hashes to `5608403408697128027` — measured actual);
  the empty-kind zero-count commands; the slot-region math.
- **`proxy_draw.rs`** — `IndirectResources`: ONE persistently mapped
  HOST_COHERENT buffer with INDIRECT_BUFFER usage
  (VUID-vkCmdDrawIndirect-buffer-02709), one disjoint 32-byte region per
  frame slot, written only inside the frame flow that first waited that
  slot's fence (the instance buffer's exact discipline). **Created LAZILY
  at the first INDIRECT-mode populated frame** — one level lazier than
  the proxy resources, so the direct instanced path's allocation pins
  (the 6-E gates) pay nothing for a mechanism it never touches; pinned by
  the gate: device_memory 18 → 19, one time, every other counter
  unchanged. `write_slot`'s unsafe carries the fence-before-write caller
  contract (the instance buffer's own proof); release runs only at
  device idle.
- **`renderer.rs` / `vulkan.rs`** — `ProxyDrawMode::Indirect` (the third
  submission mode; **the API default stays `Instanced`** — the 6-E
  contract, zero drift for the stacked PRs); the upload carries the mode;
  `proxy_draw_call_count` takes the mode (indirect records the SAME
  one-per-kind count as the direct mode — the GPU-side count changes
  where N lives, never the submission count; the policy test extended).
  The frame flow packs this slot's commands immediately before recording
  one `vkCmdDrawIndirect` per populated kind (drawCount 1, stride 16 —
  no multiDrawIndirect feature; the kinds bind different mesh VBOs); the
  HOST_COHERENT write is visible to the submission's draws by
  queue-submit ordering. The pipeline-replacement lifecycle re-ensures
  the command buffer (the finding-A discipline). A
  structurally-unreachable missing state falls back to the direct draw
  under a `debug_assert` (fail-safe, never fail-silent-wrong; the gate
  runs debug, so the assert is the proof the indirect path executed).
- **`engine-slice`** — the populated route's DEFAULT submission is now
  the indirect path (the mechanism exercised at route scale);
  `--proxy-instanced` selects the direct 6-E submission and
  `--proxy-reference` the per-object shape for the rig's before/after
  runs.
- **`tests/render_instanced.rs`** — a third `#[ignore]`d lavapipe gate,
  `indirect_draws_match_direct_and_per_object_pixels`: the SHIPPED table
  through ALL THREE submissions. Three SDL tests serialize on the same
  `sdl_lock` (the CI step's command is unchanged).

### The evidence (MEASURED on this pod's user-local lavapipe — mesa 25.2.8; software-rasterizer correctness evidence, the perf CLAIM is the owner's rig run)

Raw logs preserved at `~/verify6-6e2/` on the pod (`/tmp/opencode/gate-6e2-*`
are the in-place originals; `/tmp` may not survive a pod restart).

- **The three-way gate**: 26 instances (24 npc + 2 vehicle) — per-object
  **26 draws** == direct instanced **2** == indirect **2 draws**,
  **byte-equal captured frames**; 2,745 of 65,536 pixels changed by the
  population (the same ink as the direct gate); the indirect mechanism's
  one-time cost **device_memory 18 → 19** (the command buffer's single
  host-visible memory; every other counter unchanged); warmed
  indirect-path allocations constant
  (`19/4/1589248/2/4/2`); the replacement lifecycle re-verified
  (device_memory 41 after, delta 22, the command buffer re-ensured);
  **oracle wall 0/255 measured through the indirect submission** (the
  6-B CPU reference — the independent pin, not just A/B equality; the
  gate's assertion bound is within 2/255). The two existing gates
  re-verified green in the same run (the 13-instance fixture's
  byte-equality, oracle walls 0/255 both materials; the full-table
  gate's 26-vs-2 byte-equality).
- **The populated route** (`--corner --populated`, 60 warmup + 601 route
  frames + 2 drain, validation warnings/errors 0/0): **26 instances
  (24 npc + 2 vehicle), 2 draw calls per frame (indirect mode)**; exit
  allocations device_memory 18; peak process resident 107,136 KiB.
- **Local gates**: fmt clean; clippy `--workspace --all-targets
  --release -D warnings` clean; main shard **1146 passed / 0 failed /
  2 ignored / 33 result lines** (the population branch's 1142 + the four
  `proxy_indirect` tests, platform-uniform); renderer release battery
  **69 / 2 result lines** (the prior 65 + the same four); heavy shard
  **576 passed / 12 ignored / 58 result lines** (the prior 576/11/58 +
  the third gate's +1 ignored in the same binary); `render_corner`
  regression gate green.

### The rig run (the CLAIM — lavapipe numbers above are correctness evidence only)

```sh
# AFTER (the shipped population, one indirect draw per kind — the default):
cargo run --release -p engine-slice -- --corner --populated --fps uncapped --validation on
# The direct 6-E submission (the same population, direct instanced):
cargo run --release -p engine-slice -- --corner --populated --proxy-instanced --fps uncapped --validation on
# BEFORE (the same population, one draw per instance):
cargo run --release -p engine-slice -- --corner --populated --proxy-reference --fps uncapped --validation on
```

Each prints the dual-regime series (live TimingSamples bucket bounds AND
exact nearest-rank percentiles, n=601). Lavapipe reference from the
preserved route log (`~/verify6-6e2/gate-6e2-package-route.log`,
2026-11-23 09:23 UTC): exact CPU frame work p50/p95/p99 =
**4.295790 / 40.630449 / 48.340273 ms** — this shared pod's tail is
contended and NOT the claim. (The package commit message quotes
6.05/45.76/52.65 ms from an earlier route run on the same tree whose log
did not survive; the surviving log is the citable source. Either way the
number is software-rasterizer reference only.) **The draw-call count
(26→2) and the three-way byte-equality are the pinned headline; the
CPU-timing claim is rig-pending.**

### Conventions chosen (and why)

- **`firstVertex`/`firstInstance` pinned zero** — each kind draws its own
  bound VBO and takes its instance range through the instance-rate
  binding offset (the 6-E mechanism); a nonzero `firstInstance` would
  require the optional `drawIndirectFirstInstance` feature — deliberately
  not required, so the path runs on lavapipe (the CI gate's rasterizer)
  and every conformant implementation.
- **Lazy creation at the first INDIRECT-mode populated frame** — one
  level lazier than the proxy resources: the direct path's allocation
  pins (the 6-E gates) pay nothing for a mechanism it never touches;
  the one-time +1 device_memory is itself pinned by the gate.
- **The CPU writes the counts this package** — the mechanism is the
  deliverable; GPU-side count AUTHORSHIP (a culling/compute pass
  rewriting counts without re-recording) is the successor package's
  scope, on this PR's deferral list.
- **The API default stays `Instanced`** — the 6-E contract; zero drift
  for the stacked PRs. The DEMO's populated route defaults to indirect
  (the mechanism exercised at route scale); the submission is explicit
  on the command line.
- **drawCount 1 per kind** — no multiDrawIndirect feature (the kinds
  bind different mesh VBOs, so a single multi-draw would buy nothing).
- **The fail-safe `debug_assert` fallback** — a structurally-unreachable
  missing indirect state falls back to the direct draw under an assert
  (never fail-silent-wrong; the gate runs debug, so the assert is the
  proof the indirect path executed).

### CI pins (never weakened — every delta traces to a measured cause)

- Main shard 1142 → **1146** (Windows 1091 → **1095**: the same four
  platform-uniform `proxy_indirect` tests — an inference per the
  platform-uniform convention, **to be confirmed by this candidate's CI
  run**; a mismatch is corrected from actuals, never by weakening a
  test).
- Renderer release battery 65 → **69** (the same four).
- Heavy shard ignored 11 → **12**, result lines stay 58 (the third gate
  runs in the SAME existing enforced Linux step — three SDL tests
  serialize on the `sdl_lock`, the command is unchanged).
- The populated-route step's grep now pins **"(indirect mode)"** (the
  binary it runs evolved; the comment chain carries it).
- `docs/CURRENT-STATUS.md` updated per its rule (the matrix row, the
  test-count table).

### Deferred (untouched)

GPU-side count AUTHORSHIP (the GPU-culling package writes the counts
through this mechanism), animation, crowd LOD, mesh merging,
multiDrawIndirect, per-instance indirect commands. engine-core untouched;
fingerprints and selftest pins unaffected.

*Prepared by the run's delivery sessions (2026-11-23); body evidence
re-verified against the raw logs at `~/verify6-6e2/` by the 10:13 UTC
session before arming the four-branch delivery.*

### Merge-order dry-run (the four-way union — 2026-11-23, the 10:13 UTC session, verified)

This PR completes the stack, so the merge that matters to it is
6-F-vs-the-FULL-stack (the owner's order: the stack PRs ff-merge,
then 6-F is the one true three-way merge). The four-way census
(`git merge-tree 3522e55 4b5b228`): **8 files / 31 conflict hunks —
the SAME file set and per-file distribution as the three-way case**
(this package's indirect arm adds NO new conflict region against
6-F's shadow arm; both sides' insertions sit at the same anchors
only because both diff against the common base). The resolution
(sim commit `9c0cdb8`, tree `f02ddef`): verify5's own gated
three-way resolution patches (`5c9690d → 76a673e`) applied
**CLEANLY** onto this stack for `renderer.rs`/`vulkan.rs`; the rest
hand-united on the same conventions (`render_frame`'s three Option
slots; `--populated --shadows` a typed upfront refusal — proxy
shadows are on both PRs' deferral lists). **One code-level finding,
morning-relevant: the union's `corner_demo::run` crosses clippy's
`too_many_arguments` (8/7 — each branch individually was at or under
7; the union carries both feature axes). Resolved with the repo's
own `#[allow]` pattern (five precedents, `render_frame` itself
included), with the union arithmetic documented in a comment at the
site — a flag-struct regroup is the owner's call, not a merge
resolution's.** The combined tree, FULLY gated on the pod (every
number MEASURED 2026-11-23 10:27–10:36 UTC): fmt/clippy clean; main
**1154/2/33**, release **77/2**, heavy **576/13/59** — every pin
exactly the projected union arithmetic; every lavapipe gate green
with its evidence lines IDENTICAL to each branch's own runs (the
determinism contract holding across the union): the three-way gate
(26 == 2 == 2 draws, byte-equal, ink 2,745, one-time device_memory
18→19, warmed 19/4/1589248/2/4/2, replacement delta 22, oracle
0/255), the shadows known-answer (1,006,779 compared / 41,797
edge-skipped / 0 mismatches / max |Δdepth| 2.68e-7, identical after
the forced replacement), and all four regression binaries; all
routes green in release (populated-indirect / -instanced /
-reference, shadows, plain day+night) plus the combination refusal.
The resolution patch and bundle live at `~/verify7-union/` on the
pod — both round-trip PROVEN (the patch applied `--index` to a
fresh `3522e55` checkout and the bundle fetched into a clean repo
holding only main + the 6-F tip each reconstruct tree `f02ddef`
byte-identically).

---

### Session-4/5 payload sync — the body brought level with the branch, the repo made its single source (2026-11-23, the 11:15 UTC session)

This body was assembled at 10:38 UTC, before session 4 committed
the per-branch verification addenda (10:57–11:04); the push
watcher's payloads therefore lagged the branches by one addendum.
Session 5 folded the gap in and committed the result as this file —
the canonical PR body. The watcher payloads and the `~/push-handoff`
copies (on the pod) are regenerated from this file byte-for-byte —
their README pins the sha256 — and the run handoff's morning recipe
now points here (`--body-file docs/PR-BODY-CYCLE6-6E2.md`) instead
of at the commit message.

**The opening Branch-tip-note paragraph above is superseded** (it
describes the 10:13 state): the tips have since moved — session 4's
per-branch addenda (10:57–11:04) and session 5's payload-sync
commits — and the four branches' handoff docs are no longer
byte-identical (each branch's session-4 addendum is branch-
specific). The current state: this branch's head = the package
commit `6ee89b4` + the run-wide handoff doc (`3522e55`) + session
4's addendum (`9f40161`) + session 5's payload-sync commit (the one
adding this file). Against this PR's base (the population branch's
tip, same structure) the diff is **the package commit `6ee89b4`
plus the handoff-doc addendum delta between the two branches** —
the doc commits no longer cancel exactly, and the delta is
documentation only: `git diff --name-only 6ee89b4..HEAD` lists only
`docs/OVERNIGHT-HANDOFF-2026-11-23.md` and this file; no code, no
workflow, no pins.

**The canonical rig block** (`docs/WINDOWS-GPU-PROFILING-KIT.md`
§1–2: release build, every run at least twice with both reported,
`--present immediate --fps uncapped` for the ceiling attempt, never
conflate pacing / presentation / GPU time / presented frames).
PowerShell, from the repo root:

```powershell
git checkout cycle6/6e2-indirect-draws
cargo build --release
cargo run --release -p engine-slice -- --corner --populated --present immediate --fps uncapped
cargo run --release -p engine-slice -- --corner --populated --proxy-instanced --present immediate --fps uncapped
cargo run --release -p engine-slice -- --corner --populated --proxy-reference --present immediate --fps uncapped
# THE three-way: 26 instances == 2 indirect draws == 2 direct draws
# (vs 26 per-object); the indirect submission's CPU cost vs the
# direct one on real hardware is the claim.
cargo run --release -p engine-slice -- --corner --populated --validation on
```

(The populated route's DEFAULT submission on this branch is the
indirect path — the first command exercises it; `--proxy-instanced`
selects the direct 6-E submission, `--proxy-reference` the
per-object shape.)

`--present immediate` is prefer-when-supported (`config.rs`'s
`PresentModePreference::Immediate`): the pod's offscreen lavapipe
surface falls back to fifo — the banner prints the EFFECTIVE mode —
while the rig's swapchain selects immediate.

**Session 5 pre-flighted this exact block verbatim** on this
branch's tip (2026-11-23 11:14 UTC, release, pod-local lavapipe,
mesa 25.2.8): **rc=0 on all four runs**, headline lines reproduced
exactly — `26 instances (24 npc + 2 vehicle), 2 draw calls per
frame (indirect mode)` / `(instanced mode)` / `26 draw calls per
frame (per-object reference mode)` / validation warnings/errors 0/0
on all four. Logs:
`~/verify8-session5/session5-preflight-6e2.log` on the pod (lavapipe
numbers are correctness evidence only, never the perf claim).

**Session 4's verification, folded in** (the full 65-step table is
the branch's addendum — the docs commit between the handoff doc and
this one): every CI step no prior session had run locally, executed
green — the THREE `render_instanced` gates (the 26 == 2 == 2
three-way byte-equality, both prior gates re-verified in the same
run), the populated route in indirect mode (26 instances, 2 draws,
validation 0/0), the placement battery 6/6, the proxy battery 17/17
(13 + this package's 4), and the renderer release battery
**69/2** matching the pin.

---

### Session 6 — the heavy shard's direct clean-environment run (2026-11-23, the ~11:30 UTC session)

The one second-run gap this branch still carried: the heavy shard
had session 3's single direct measurement (576/12/58), taken
before the clean-environment discipline the 6-F lane's polluted-env
FAIL established — the main shard already had session 4's direct
clean-environment run on this tree (1146/2/33). Session 6 closed
the heavy gap: the CI command verbatim (`cargo test -p
engine-slice --features engine-slice/crash-test-seams`), the
workflow's own awk counting, in a shell with no SDL/VK variables,
directly on this branch's tip (`ae7bcf5` at run time; the code tree
is exactly `6ee89b4`'s — every commit above the package commit is
documentation only):

| Step (MEASURED this session, this tip) | Result |
|---|---|
| `cargo fmt --all -- --check`; clippy `-D warnings` in both forms (the CI debug form, the standing-order release form) | clean |
| **the full heavy shard** | **576 passed / 12 ignored / 58 result lines, rc=0, zero failures — the branch pin, its first post-clean-discipline direct run (session 3's 576/12/58 re-confirmed on this tree)** |

Raw log: `/tmp/opencode/session6-6e2-heavy.log` on the pod (a /tmp
path — may not survive a pod restart; the numbers here are the
durable record). The watcher payload and its `~/push-handoff` twin
were regenerated from this file byte-for-byte after this section
landed.

**The tip statement (supersedes the Branch tip note above):** the
pushed tip is this branch's head = the package commit `6ee89b4` +
the run-wide handoff doc (`3522e55`) + session 4's addendum
(`9f40161`) + session 5's payload-sync commit (`ae7bcf5`) +
session 6's heavy-shard verification addendum (the commit adding
this section). Every commit beyond `6ee89b4` is documentation only
— `git diff --name-only 6ee89b4..HEAD` lists only
`docs/OVERNIGHT-HANDOFF-2026-11-23.md` and this file; no code, no
workflow, no pins. This PR's base is the population branch's tip,
which carries the same doc line; the doc commits' evolution between
the branches remains documentation only.
