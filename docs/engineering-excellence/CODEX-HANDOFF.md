# The Codex handoff briefing (the owner-directed independent audit + fix turn)

You are arriving mid-program with repo access but no conversation
history. This file orients you. It was written by the orchestrator
session (glm-5.3-w4a8) at the owner's direction; the owner is handing
you the reins for one turn — a small audit plus implementable fixes.

## What this repository is

PostRust: a five-crate Rust game engine (engine-core, engine-persist,
engine-renderer, engine-shell, engine-slice) under an
engineering-excellence program chartered by the owner in
`docs/engineering-excellence/DIRECTIVE.md` (read it first — it is the
controlling document). The goal: every engineering category
independently substantiated at ≥9/10, verified by evidence, never by
claim. The program's whole history, every decision, and every piece
of evidence lives in `docs/engineering-excellence/`:

- `README.md` — the authoritative current-status representation
- `FINDINGS.md` — the finding register (every audit finding, its
  state, its landing SHA)
- `DECISIONS.md` — D-EE-01..10, the decision records (D-EE-10 is the
  owner's latest authorization block — read it before acting)
- `EVIDENCE.md` — E-1..E-13, the evidence manifest
- `designs/` — P2/P3/P4/P5, the design documents the implementation
  lanes executed from
- `FOOTPRINT.md` — the B15 workspace measurement
- `INDEPENDENT-AUDIT-be910bf.md` — the previous independent audit
  (the IA-01..14 findings, all since fixed)

## The rules you must not break (the owner's standing rules)

1. **NEVER touch `main`** (at `5b93167`, tagged `pre-excellence-5b93167`).
   All work lands on `refactor/engineering-excellence` (or a branch
   off it). The final integration is the owner's GO, not yours or mine.
2. **NEVER weaken a test or a pin.** Count corrections only from
   measured actuals (run the suite, record command + exit status +
   the TRUE counts), documented in the commit message. Timing
   tolerance changes are measured-actuals corrections with the
   numbers recorded in the test (see git log `877590f`, `a6e5adc`
   for the precedent form).
3. **Determinism is byte-identical** — the campaign digest pins, the
   save round-trips, the double-run byte-equality gates. If your
   change moves any pinned value, that is a finding to investigate,
   never a pin to edit.
4. **Every claim carries evidence**: the command, the exit status,
   the tree SHA. Commit messages are long, lowercase,
   evidence-carrying records (the house style — read a few recent
   ones).
5. **The GPU is real**: an RTX 5070 Ti. The `#[ignore]`d rig
   batteries run with `--ignored --nocapture --test-threads=1`
   serially, WITHOUT `SDL_VIDEODRIVER=offscreen` on Windows. The
   render_post battery must run unfiltered (the poison cascade).
6. Count pins live in `.github/workflows/engine-ci.yml` (~line 747
   main, ~870 renderer release, ~2070 heavy). Current: main
   **1540/1489** (Linux/Windows), renderer release **190**, heavy
   **674/673/47**.

## Where the program stands (at this writing)

The original audits (P0), the fix waves (P1, the IA wave), the
scalability/maintainability lanes (P2, P3, P4, P5-R0), the owner's
Q-findings work — all landed or in flight. The owner's latest
independent re-audit scored the candidate 8.0/8.1/7.2/7.7/8.1/8.4/
7.8/7.6 — substantial but not acceptance. The open items are the
register's Q-rows and the queue in README's next-actions.

## What the owner asked of your turn

A **small audit** (pick a surface, verify it against the source —
the standing discipline is that audit claims are verified by direct
read before acceptance; see FINDINGS.md's calibration record for the
times auditors were wrong) **plus implementable fixes** for what you
find — under the rules above, with gates run and counts measured.

## Practical notes

- `cargo test --workspace --exclude engine-slice --features
  engine-persist/composite-test-fixture` is the "main shard".
- `cargo test -p engine-slice` is the slice suite (its `#[ignore]`d
  batteries are the rig gates).
- The census batteries (`census_*`) are the P5 measurement harness —
  `census_repack`/`census_attach`/`census_ripple` run CPU/release
  anywhere; `census_vram`/`census_composition` need the display.
- Worktrees under `Temp\opencode\ee\` may hold in-flight lane work —
  check `git worktree list` and each worktree's `git status` before
  touching anything; the orchestrator's lanes commit to their own
  branches (`ee/*`), never to the integration branch directly.
- If you fix something: commit it on a branch off the integration
  head with the evidence-carrying message style; the orchestrator
  will review, gate, and pin it into the integration branch.

Welcome aboard. The bar is high and the evidence discipline is real;
that is the point.
