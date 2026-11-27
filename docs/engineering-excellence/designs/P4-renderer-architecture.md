# P4 — Renderer Architecture and Separation (DESIGN)

**Status:** IN PROGRESS — being written incrementally (subagent design-p4).
**Base for all source citations:** `refactor/engineering-excellence` @
`7961cb290cc5222db88d222333fc4de8f4183925` (working tree HEAD at dispatch;
crates/** read-only throughout).
**Charter:** `DIRECTIVE.md` §7 P4 paragraph + §4 (engineering standards);
finding rows A01(rend), A03, A04, A06, A11, A12, A14, A15, B14, B15 in
`FINDINGS.md`.

> Sections below are being appended in reading order; "cited at" line numbers
> refer to the exact SHA above. This box will be replaced by the final
> summary when the document is complete.

## 0. Source-reading log (coverage + honesty)

- READ: DIRECTIVE §4/§7-P4, FINDINGS rows (A01 rend, A03–A06, A11, A12, A14,
  A15, B14, B15, NEW-REND-*), `audits/slice.md` §2 module classification map.
- **COVERAGE GAP (recorded, not silently skipped):** `audits/renderer.md` is
  referenced by `FINDINGS.md` (rows A03/A04/A12 cite it; the calibration
  record describes its structure map and shader duplication matrix) but the
  file does NOT exist at HEAD — `git ls-files` shows only core/persist/shell/
  slice under `docs/engineering-excellence/audits/`, and `git log --all --
  <path>` finds no commit that ever touched it. The renderer audit's facts
  therefore had to be re-derived from source in this session; every
  load-bearing claim below carries its own direct citation instead.
- (in progress) renderer.rs / config.rs / vulkan.rs / scene_record.rs /
  frame_prepare.rs / shader pairs / draw modules — see per-section citations.

## 1. Evidence base — what the source actually shows

(being filled; see §2–§6)
