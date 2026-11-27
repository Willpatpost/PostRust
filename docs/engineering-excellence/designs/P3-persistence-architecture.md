# P3 — Persistence and Recovery Architecture (Design)

> **Status:** IN PROGRESS (incrementally appended; partial sections are draft).
> **Author:** P3 design author (glm-5.3-w4a8), read-only for `crates/**`.
> **Base:** branch `refactor/engineering-excellence`, HEAD
> `bceb3faed42a4124bc615916b4a352cdf2d1979a` (verified `git rev-parse HEAD`,
> 2026-11-26 22:41:24 -0500, "integrate p1-b05 …").
> **Scope per DIRECTIVE §7 P3:** A13, A22 (persist half), B02, B03, B04
> (persist half — slice side already fixed, see FINDINGS), B12, B13,
> NEW-PERSIST-04/05. A01 persist half (module split) is folded into A13's
> module plan. Non-goals listed in §9.

## 0. Method and evidence discipline

Every load-bearing claim below is verified against source at HEAD
`bceb3fa` with exact `file:line` citations. Large files (`journal.rs`,
`local.rs`, `reclaim.rs`) are navigated by grep + bounded reads, not
whole-file reads. Where the audit's line refs disagreed with HEAD, the
source read governs (FINDINGS.md verdict-provenance rule).

Sections:
1. Invariant and ownership map (current seams, target seams).
2. Per-finding design sections (B02, B03, B13, A13, A22-persist, plus
   B04/B12 coverage notes).
3. Staged implementation plan (lanes, risk class, count pins).
4. Measurement plan (charter §8.2 persistence row).
5. Non-goals.

---
