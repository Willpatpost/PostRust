# P1/P2-A24 — Fixed-timestep decoupling for the corner demo's interactive loops

**Package:** A24 · **Lane branch:** `ee/p2-a24-fixed-timestep` ·
**Base:** integration head at worktree setup (`e31dc61` or later — record it).
**Role:** focused Rust implementation, design-note-first. **One
responsibility:** stop advancing the corner demo's interactive subsystems at
`1.0/60.0` per PRESENTED frame; drive them by accumulated wall time at a fixed
tick — nothing else.

## Problem (verified: slice audit + orchestrator grep, 2026-11-26)

- `crates/engine-slice/src/corner_demo.rs:293` (`step_plain`) and `:308`
  (`step_composed`) advance at hardcoded `1.0 / 60.0` per presented frame;
  the free-look loop (corner_demo.rs ~1100+) runs camera + crowd + weather at
  render cadence with NO catch-up — variable framerate/minimize/stall changes
  the world's advancement rate.
- `crates/engine-slice/src/corner_weather.rs:267,282,384,393`: the rain
  emitter and ripple field tick at hardcoded `1.0/60.0` per frame.

## Required outcome

1. **Read first:** `crates/engine-slice/src/frame_timing.rs` (the existing
   sim-vs-camera time-splitting discipline) and how `sim.rs` /
   `window_demo.rs` use it. The charter is explicit: integrate the EXISTING
   timing mechanism, do not invent a second incompatible one.
2. Extract a fixed-timestep accumulator (the frame_timing pattern) driving
   the corner demo's interactive loops: accumulate presented-frame wall time,
   step the weather/ripple/camera/crowd at exactly 1/60 per accumulated
   1/60 s, with BOUNDED catch-up (a documented cap on backlog steps per
   frame; beyond the cap, drop the excess time VISIBLY — a counter or log
   line, never silently) and pause/minimize handling per the existing
   discipline (minimized windows still advance the campaign clock per the
   no-silent-discard rule).
3. **At exactly 60 fps the behavior is byte-identical:** one tick per
   presented frame, the same warm-start census, the same exit summaries, the
   same day/night progression per frame. This is the equivalence bar.
4. **Determinism boundary — the headless campaigns are UNTOUCHED:** the
   pinned gates (render_weather, the ripple census spawned=6360 died=6049,
   the 663-frame campaigns, pose pins) drive frames BY INDEX through
   deterministic paths — they must not change AT ALL. This package changes
   only the interactive (windowed, wall-clock) route. No spine, no
   fingerprint, no digest changes anywhere. If any pinned value moves, STOP
   and report — that is a blocker.
5. No duplicate input edges or audio triggers under catch-up stepping (the
   accumulator steps the WORLD, never re-polls input).

## Constraints

- Allowed edit paths: `crates/engine-slice/src/corner_demo.rs`,
  `corner_weather.rs`, `frame_timing.rs` (ONLY to extend/reuse, not to change
  its existing consumers' semantics), and at most a small new
  `corner_timing.rs` if the extraction demands it. Forbidden: everything
  else, including engine-core/engine-renderer/engine-shell.
- Gates (record commands + exit status): `cargo fmt --all -- --check`;
  `cargo clippy -p engine-slice --all-targets -- -D warnings`;
  `cargo test -p engine-slice --lib` (the corner batteries);
  `cargo test -p engine-slice --test render_weather -- --nocapture
  --test-threads=1` is the rig battery — SKIP the #[ignore]d Vulkan gates
  (no GPU in your worktree); run the NON-ignored tests of that binary.
- Design note in the commit message: the accumulator shape, the catch-up cap
  and its justification, and the 60fps-equivalence argument.
- Commit style: long, lowercase, evidence-carrying. No push, no merge, no
  main.

## Handoff (required, structured)

Exact commit SHA + base; changed files; the design note; the 60fps
equivalence argument; which pinned gates/campaigns were re-run and their
values (unchanged = stated); commands + exit status; risks; unresolved
issues; integration requirements.
