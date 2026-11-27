# P1-A17 — Cache immutable rest/bind transforms (animation + stationary poses)

**Package:** P1-A17 · **Lane branch:** `ee/p1-a17-rest-caching` ·
**Base:** the `refactor/engineering-excellence` head at worktree creation.
**Role:** focused Rust implementation. **One responsibility:** stop
recomputing skeleton-constant rest data per call/per stationary NPC — nothing
else.

## Problem (verified on the baseline tree, 2026-11-26)

1. `crates/engine-renderer/src/animation.rs::skin_matrices` (~line 420):
   every call constructs the all-zero `rest_pose` and runs
   `compose_joint_matrices(skeleton, &rest_pose)` (~line 429) to extract the
   bind world translations — pure functions of the IMMUTABLE skeleton,
   recomputed per evaluation. The walk loop calls this per walking NPC per
   frame; the demo's pose evaluation calls it for every NPC.
2. `crates/engine-slice/src/corner_routes.rs::evaluate_population_poses_locomotion`
   (~line 617): regenerates the rest pose block per STATIONARY NPC (waiters)
   — the same rest block for every stationary NPC on every frame.

## Required outcome

- Cache the skeleton-constant data ONCE at authoring/construction (where the
  `Skeleton` is built, or a cached table keyed by the skeleton the animation
  module already owns — read the actual ownership first and choose the
  placement with the fewest boundaries/copies; do not add a new public
  machinery layer for this).
- Share the stationary/rest pose block in
  `evaluate_population_poses_locomotion` so identical rest blocks are
  computed once and referenced/copied per NPC as the retained pose buffer's
  layout requires (read `repack_poses`/the pose buffer contract first; keep
  slot ownership and byte layout EXACTLY as-is).
- **Bit-exactness is the acceptance bar:** the rest-identity proof
  (`skin_matrices_rest_identity_is_bit_exact`, animation.rs ~1818), every
  packed-bytes FNV pose pin, the walk-clip digest pins, and the
  `pose_bytes_fnv_pins` class in engine-slice must reproduce UNCHANGED. The
  cached value must be the same pure computation's result — identical
  arithmetic, computed once.
- Zero per-frame allocation on the warmed path must be preserved (the
  allocation-constancy gates pin it). A one-time allocation at skeleton
  authoring is fine; document it.
- Assess (report only, no implementation): where else rest/bind data is
  recomputed (e.g. inverse bind matrices, shadowed depth paths, the CPU
  skinning oracle) — a FINDINGS note for the orchestrator, not drive-by edits.

## Constraints

- Allowed edit paths: `crates/engine-renderer/src/animation.rs` (incl. tests),
  `crates/engine-slice/src/corner_routes.rs` (incl. its test module),
  and — ONLY if the ownership analysis demands it — the minimal construction
  site of the demo's skeleton/pose tables in engine-slice. Forbidden:
  engine-core, engine-persist, engine-shell, vulkan.rs, the workflow.
- Do not change `POSE_STRIDE`, the pose buffer layout, the packed-bytes
  format, or any digest pin VALUE. If a pin cannot hold, STOP and report —
  that is a blocker, not a re-pin.
- Tests: run `cargo test -p engine-renderer` and the engine-slice corner
  batteries relevant to poses
  (`cargo test -p engine-slice --lib corner_routes` and the pose-pin tests by
  name). Report exact pass counts.
- `cargo fmt --all -- --check`; `cargo clippy -p engine-renderer -p
  engine-slice --all-targets -- -D warnings`.
- Performance evidence: the corner demo's 24-NPC population is small — give
  a reasoned cost statement (work removed per frame: N_walk × rest-compose
  cost, measured via a micro-benchmark ONLY if straightforward; otherwise the
  operation-count argument with the pure-function cost measured once in a
  test). No invented numbers.

## Handoff (required, structured)

Exact commit SHA + base; changed files; where the cache lives and why that
placement; which pins were re-run and their values (unchanged = stated);
commands + exit status; risks (especially any lifetime/ownership subtlety of
the cached table); unresolved issues; the A17 "assess elsewhere" note. Do NOT
push, do NOT merge, do NOT touch main. Commit on your lane branch and stop.
