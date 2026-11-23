//! # AUTONOMOUS-CYCLE PROTOCOL — the rules for unattended development
//!
//! **Status:** v1.0 · 2026-11-23 · The standing rules for any
//! autonomous (unsupervised) development run on this repository —
//! first use: the 2026-11-23 overnight run. An autonomous agent that
//! cannot follow this protocol must stop and leave a PR comment
//! saying so, not improvise.
//!
//! **The one-sentence summary:** you may push branches and open PRs
//! all night; you may never merge, never weaken a pin, and never
//! touch the deterministic contract — the owner's GO, the CI's exact
//! counts, and the morning audit are the walls that make your
//! autonomy safe.

## 0. The hard limits (violating any of these is a failed run)

1. **Never merge.** No PR is ever merged by you — not your own, not
   anyone's. Merges happen only on the owner's explicit GO. Open PRs
   as DRAFT while working; mark ready when you believe the package
   is complete.
2. **Never touch `main`** (no direct commits, no force-push, no
   branch deletion anywhere, no workflow edits, no branch-protection
   changes).
3. **Never weaken a test, a pin, or a gate.** An exact-count pin
   that fails is an INVESTIGATION, never an adjustment: find the
   cause, document it in the PR, and either fix the cause or leave
   the failing pin with a full explanation. A CI gate that fails is
   information, not an obstacle. (History: run 45097154875's flake
   was root-caused for hours before any line moved — that is the
   standard.)
4. **Never alter determinism.** Fingerprints stay byte-identical;
   `engine-core` is out of scope for autonomous runs unless a
   package brief explicitly includes it; no `unsafe` without a
   documented proof; libm-free arithmetic on any byte-packed path
   (see `proxy_instances.rs` for the pattern and the reason).
5. **One PR per package, the brief's scope only.** No scope creep,
   no drive-by refactors, no "while I was here" formatting of files
   the package doesn't own. If something adjacent is wrong, note it
   in a PR comment — don't fix it in this PR.
6. **Run the local gates before every push:** `cargo fmt`, `cargo
   clippy --workspace --all-targets --release -- -D warnings`, and
   `cargo test --workspace --exclude engine-slice --features
   engine-persist/composite-test-fixture` (headless; no GPU needed).
   CI runs on every pull_request — a red push is a wasted CI cycle
   and a noise signal the owner reads in the morning.

## 1. The working method (the pattern every package follows)

The proven shape (6-A, 6-B, and the 6-E scaffold all follow it):

1. **Deterministic core first** — pure data + math + oracle, fully
   tested, no Vulkan, no I/O. If the package lacks one, write it
   first (or extend the existing one).
2. **The oracle pins the semantics** — a CPU reference the GPU path
   must reproduce (`lit_surface_color` is the model; the pixel-
   equivalence capture test is the enforcement).
3. **Then the plumbing** — the ash/Vulkan wiring, with unsafe
   confined to the existing `vulkan` module's invariants manifest.
4. **Then the evidence** — the D3 dual regime: live `TimingSamples`
   AND exact capture-based percentiles; claims cite exact numbers,
   and rig-dependent claims are marked "awaiting the owner's rig
   run" (never presented as measured when they weren't).
5. **Then the PR** — title, body, and commit messages carry the
   evidence and the reasoning (the repo's commit messages are its
   history: read a few before writing yours).

**Known toolchain facts you will need** (probed, pinned, do not
rediscover): the pinned naga 29.0.4's wgsl-in has no
`var<push_constant>` (uniform buffers instead — see
`lit_scene.wgsl`); its glsl-in cannot express sampled textures (new
shaders are WGSL); `compile_wgsl`'s binding map covers bindings
0–3; WGSL has no `countTrailingZeros` (write the loop — see
`lit_scene.wgsl`); one module carries both entry points.

## 2. The ladder (tonight's work order)

Work the packages IN ORDER. Each is one branch + one PR. Do not
start the next until the current one is pushed with its PR open
(and ideally CI-green) — but do not merge.

| # | Package | Entry point | Notes |
|---|---|---|---|
| 1 | **6-E: instanced draws** | `docs/CYCLE6-6E-BRIEF.md` + the scaffold already on `main` (`proxy_instances.rs`, `proxy_mesh.rs`, `instanced_proxy.wgsl`) | The night's primary package. The brief's five acceptance criteria are merge-blocking. |
| 2 | **6-F: directional shadows** | No scaffold exists. FOLLOW §1: write the deterministic core first — the light-space orthographic projection + depth-bias math + cascade bounds as pure tested functions (the `proxy_instances.rs` pattern: libm-free where bytes are packed, fixed operation order) — then a depth-only shader variant, then the pipeline work. One cascade, the corner scene, no framework. | The visual-fidelity package. The equivalence gate: shadowed vs unshadowed pixel comparison must be a pinned assertion, and the shadow map's depth values need a CPU-side known-answer check. |
| 3 | **Corner-scene population** | Rides on 6-E landing: the deterministic population table (24 NPCs + 2 vehicles placed around the intersection — pure data, fixed or seeded, documented), drawn through the instanced path. | Composition work; small if 6-E is clean. |
| 4 | **6-E2: indirect draws (stretch)** | Only if 1–3 are done and CI-green. | GPU-side instance counts via `vkCmdDrawIndirect`. |

**If a package is DONE and CI-green before the next begins: start
the next. The run should still be working when the owner wakes.**

## 3. Blocked? Park, don't thrash

If one wall resists **more than ~60 minutes** of genuine effort:

1. Commit the work-in-progress state (compiling, tests green or
   explicitly documented as red).
2. Write a PR comment: the wall, what you tried, what you believe
   the cause is, what evidence would resolve it.
3. Move to the next ladder item.

A parked, well-documented wall is a GOOD outcome (the morning audit
reads it as information). An all-night thrash on one wall — or worse,
a "fix" that weakens a gate to get past it — is the failure mode
this section exists to prevent. **Never debug by changing tests.**

### 3.1 Runner flakes (known, on record, not yours to fix)

A CI failure in a test **your diff does not touch**, which has passed
in other runs of the same code, is a runner flake. Three are on
record from 2026-11-23 alone: the spine-allocation pin (libtest's
main-thread mpmc waker — fixed by thread attribution), the persist
setup helper (transient filesystem `Unknown` — fixed by
reconciliation), and the `s6_soak` late-window fsync race (a
726 ms runner stall letting the WAL fsync land before the hard exit
— a wall-clock race in the test's setup, open). The protocol for a
suspected flake: **rerun the failed job once**
(`gh run rerun --failed -R Willpatpost/PostRust <run-id>`), document
the occurrence and your diff-relevance analysis in the PR, and if it
fails twice leave it red with the analysis — never modify the test,
never weaken the gate, never "fix" engine code a flake implicated
without the full investigation discipline (the allocation flake's
root-cause record in `spine_allocations.rs`'s module docs is the
standard your analysis should aim for).

## 4. What the morning audit checks (write for it)

Every PR is audited against: scope (the diff is the brief, nothing
else); pin history (every count change traces to a measured cause,
documented in the commit); the equivalence gates (real assertions,
not vacuous ones); the allocation gates (`renderer.allocations()`
constant across warmed frames — asserted, not assumed); unsafe (any
new unsafe has a proof); and the evidence claims (exact numbers with
sources; rig claims marked pending). Write your PR bodies so this
audit is fast: link the evidence, show the numbers, state the
conventions you chose and why. An auditor who can verify quickly is
an owner who can GO quickly.

## 5. Communication

- PR titles/bodies/commit messages are the channel to the owner and
  the auditor. They are read; make them worth reading.
- Never claim a measurement you did not take. The rig (RTX 5070 Ti)
  is the owner's; lavapipe/CI is yours. Label accordingly.
- If everything is done and CI-green, improve the weakest evidence
  in the open PRs (more samples, tighter assertions, better
  documentation) — not new scope.

### 5.1 The diminishing-returns stop rule (the 2026-11-23 postmortem)

"Improve the weakest evidence" has a FLOOR. If the ladder is
complete, every PR is green, and the remaining improvements are
re-measurements of already-triple-verified gates or documentation
polish beyond what a PR body needs — create the DONE marker and
stop. A night that finishes early and idles is a SUCCESS; a night
that spins re-verification loops is a failure mode. Concretely: no
gate gets re-run more than twice on the same tree; no doc gets more
than one post-completion revision; when in doubt, DONE.

### 5.2 Push as you go (the 2026-11-23 postmortem)

Never stage a delivery. The moment a package compiles and its gates
pass locally, push its branch and open the draft PR — CI judges it
overnight, and the owner merges plain green PRs in the morning. No
push watchers, no handoff kits, no payload twins. If pushing fails
on credentials, park the package, document the blocker on the
branch (a commit message), and continue with the next package.

---

*This protocol binds autonomous runs. It does not change any
ratified decision, gate, or owner authority; where it is silent, the
repo's standing discipline governs (see `docs/DECISIONS.md`,
`docs/CURRENT-STATUS.md`, and the workflow definitions).*
