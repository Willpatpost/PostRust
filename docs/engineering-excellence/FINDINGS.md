# Finding Register

Stable IDs and states. Seed list from charter §5/§6 (A01–A25, B01–B16);
specialists add `NEW-<area>-<n>` entries. States: UNTRIAGED, CONFIRMED,
HYPOTHESIS, DESIGN-RISK, IN-PROGRESS, FIXED-AWAITING-REVIEW, VERIFIED-CLOSED,
NOT-APPLICABLE (with proof), BLOCKED (with cause). Full descriptions of A*/B*
live in `DIRECTIVE.md` §5/§6 — this table carries only state + owner + note.

Area abbreviations: CORE=engine-core, PERSIST=engine-persist, REND=engine-renderer,
SHELL=engine-shell, SLICE=engine-slice.

| ID | Area | State | Owner/ Lane | Note |
|---|---|---|---|---|
| A01 | PERSIST/REND/CORE | UNTRIAGED | — | god-module splits |
| A02 | CORE/REND | UNTRIAGED | — | historical commentary → present-tense contracts |
| A03 | REND | UNTRIAGED | — | render_corner* combinatorial entry points |
| A04 | REND | UNTRIAGED | — | render_frame ~18 params |
| A05 | CORE | UNTRIAGED | — | demo workload constants in engine core |
| A06 | REND/SLICE | UNTRIAGED | — | animation representation vs GPU submission |
| A07 | CORE | UNTRIAGED | — | perception census full-population iteration |
| A08 | CORE | UNTRIAGED | — | spine suspend/reclassify heap churn |
| A09 | CORE | UNTRIAGED | — | npc_memory event-first ledger scans |
| A10 | CORE | UNTRIAGED | — | collision linear traversal |
| A11 | REND | UNTRIAGED | — | MAX_INSTANCES/MAX_POSE_BLOCKS demo caps |
| A12 | REND | UNTRIAGED | — | shader variant duplication |
| A13 | PERSIST | UNTRIAGED | — | pure vs I/O separation |
| A16 | PERSIST | CONFIRMED | ee/p1-a16-ordered-confirm | verified at `pipeline.rs:1394` on baseline (2026-11-26); O(N²) find per key, N ≤ 65,536 |
| A17 | REND/SLICE | CONFIRMED | ee/p1-a17-rest-caching | verified `animation.rs:420` recomputes rest pose per call; `corner_routes.rs:617` regenerates stationary pose |
| A18 | REND | UNTRIAGED | — | full repack per frame |
| A19 | REND | UNTRIAGED | — | ripple loop over all rings per fragment |
| A20 | REND | UNTRIAGED | — | per-asset staging sync; device idle on resize |
| A21 | CORE | UNTRIAGED | — | unbounded Diagnostics Vec + sync stderr |
| A22 | CORE/PERSIST | UNTRIAGED | — | ledger growth without lifecycle policy |
| A23 | REND/SHELL | UNTRIAGED | — | fixed slot arrays, measure before changing |
| A24 | SLICE | UNTRIAGED | — | 1/60 hardwired to present cadence |
| A25 | SHELL | UNTRIAGED | — | command mutex held during voice start |
| B01 | CORE | UNTRIAGED | — | scheduler budget counts executions only |
| B02 | PERSIST | UNTRIAGED | — | journal whole-file read, 4 GiB cap |
| B03 | PERSIST | UNTRIAGED | — | composite memo copy amplification |
| B04 | SLICE/PERSIST | UNTRIAGED | — | memory-lane transaction bridge duplication |
| B05 | CORE | UNTRIAGED | — | particle dt/extent domain contract |
| B06 | CORE | UNTRIAGED | — | interaction/combat candidate scans |
| B07 | CORE | UNTRIAGED | — | rule table validation per wake |
| B08 | SHELL | UNTRIAGED | — | input buffer mem::take churn |
| B09 | REND | CONFIRMED | ee/p1-b09-metal-surface | verified `vulkan.rs:1142` `VK_KHR_metal_surface` wrong literal in `PLATFORM_SURFACE_EXTENSIONS`; rest of file uses correct `VK_EXT_metal_surface` |
| B10 | SLICE | UNTRIAGED | — | activation crossover harness asymmetry |
| B11 | SLICE | UNTRIAGED | — | ripple diagnostic overstates metric |
| B12 | CORE | UNTRIAGED | — | identity persistence not integrated |
| B13 | SLICE/PERSIST | UNTRIAGED | — | worker backpressure/HOL mapping |
| B14 | REND | UNTRIAGED | — | reflection/post target cost review |
| B15 | WORKSPACE | UNTRIAGED | — | API/build footprint measurement |
| B16 | ALL | UNTRIAGED | — | uncovered areas mandatory audit |

## Discovered findings (NEW-*)

(none yet — auditors append here with exact source locations and evidence)

<!-- Entry format:
| NEW-CORE-1 | CORE | CONFIRMED | — | one-line description; source `path:line`; evidence |
-->
