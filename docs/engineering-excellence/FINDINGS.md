# Finding Register

Stable IDs and states. Seed list from charter §5/§6 (A01–A25, B01–B16);
specialists add `NEW-<area>-<n>` entries. States: UNTRIAGED, CONFIRMED,
HYPOTHESIS, DESIGN-RISK, IN-PROGRESS, FIXED-AWAITING-REVIEW, VERIFIED-CLOSED,
NOT-APPLICABLE (with proof), BLOCKED (with cause). Full descriptions of A*/B*
live in `DIRECTIVE.md` §5/§6 — this table carries only state + owner + note.

Area abbreviations: CORE=engine-core, PERSIST=engine-persist, REND=engine-renderer,
SHELL=engine-shell, SLICE=engine-slice.

Verdict provenance: `audits/*.md` are the auditors' raw reports; where the
orchestrator's verification disagrees with an auditor's label, the note says so
and the register carries the reconciled state (auditor evidence cited, label
corrected).

| ID | Area | State | Owner/ Lane | Note |
|---|---|---|---|---|
| A01 | PERSIST/REND/CORE | CONFIRMED (persist+rend halves) | — | persist: local.rs ~12.9k, sqlite.rs ~8.3k, reclaim.rs ~6.9k, pipeline.rs ~3.6k whole-file (audits/persist.md); rend: vulkan.rs **6,122** lines (orchestrator-measured; auditor's 2.8k claim wrong), renderer.rs combinatorial entries ~620–998. CORE half pending auditor-core |
| A02 | CORE/REND | UNTRIAGED | — | pending auditor-core (renderer half: scene_record/frame_prepare per charter) |
| A03 | REND | CONFIRMED | — | four render_corner* entry points + DemoScenePreference (audits/renderer.md) |
| A04 | REND | CONFIRMED | — | render_frame 19 params (incl. wet; charter said ~18) — vulkan.rs:3974-3994 |
| A05 | CORE | UNTRIAGED | — | pending auditor-core |
| A06 | REND/SLICE | CONFIRMED | — | animation core is pure but lives INSIDE the renderer crate — placement is the P4 question (auditor read the purity as fine; the crate-coupling issue stands) |
| A07 | CORE | UNTRIAGED | — | pending auditor-core |
| A08 | CORE | UNTRIAGED | — | pending auditor-core |
| A09 | CORE | UNTRIAGED | — | pending auditor-core |
| A10 | CORE | UNTRIAGED | — | pending auditor-core |
| A11 | REND | CONFIRMED | — | MAX_INSTANCES=4096 = MAX_POSE_BLOCKS; region capacity per FRAMES_IN_FLIGHT (scene.rs:319) |
| A12 | REND | CONFIRMED | — | 1:1 shadowed/skinned shader variant pairs + per-variant descriptor layouts (audits/renderer.md duplication matrix) |
| A13 | PERSIST | CONFIRMED | — | **auditor labeled "Refuted" but its own evidence confirms the mixing** (validate_observed local.rs:714, validate local.rs:1656 interleaved with append/commit in the same modules; no dedicated pure-validation layer) — label corrected by orchestrator |
| A16 | PERSIST | CONFIRMED | ee/p1-a16-ordered-confirm | verified `pipeline.rs:1394`; lane IN-PROGRESS |
| A17 | REND/SLICE | CONFIRMED | ee/p1-a17-rest-caching | verified `animation.rs:420` (+ usages shadow_draw.rs:928, reflection_draw.rs:1402), `corner_routes.rs:617`; lane IN-PROGRESS |
| A18 | REND | CONFIRMED | — | repack_populations proxy_instances.rs:699-706 + repack_poses animation.rs:729 per frame; folds NEW-REND-08 (static-population caching = P5 dirty-generation question) |
| A19 | REND | CONFIRMED | — | ground_reflect.wgsl:144-166 per-fragment loop over all rings; folds NEW-REND-09 |
| A20 | REND | CONFIRMED | — | per-static-upload staging alloc/copy/submit/fence (vulkan.rs:3035-3199, textures.rs:61-202); folds NEW-REND-03 (no staging pool — same finding, more detail) |
| A21 | CORE | UNTRIAGED | — | pending auditor-core |
| A22 | CORE/PERSIST | CONFIRMED (persist half) | — | confirmed_history grows unbounded, local.rs:3015-3028, no pruning; CORE half pending |
| A23 | REND/SHELL | CONFIRMED (rend half) | — | pose storage dynamic-offset per slot (skinned_draw.rs:949-950, POSE_STRIDE=704); SHELL half pending auditor-shell |
| A24 | SLICE | UNTRIAGED | — | pending auditor-slice |
| A25 | SHELL | UNTRIAGED | — | pending auditor-shell |
| B01 | CORE | UNTRIAGED | — | pending auditor-core |
| B02 | PERSIST | CONFIRMED | — | journal.rs:170-189 whole-file read ≤4 GiB; auditor's caller list (journal.rs:1115…3428) to be re-verified in P3 (line refs ambiguous across modules) |
| B03 | PERSIST | CONFIRMED | — | composite_reader.rs:416-464,544 payload clones; reclaim.rs ~5686 bytes.clone(); pipeline.rs PendingAttempt clones; folds NEW-PERSIST-01/04 |
| B04 | SLICE/PERSIST | PARTIAL | — | persist half: core state machine IS consolidated in pipeline.rs (PendingAttempt 1088-1313, retry identity preserved) — the charter's hypothesis concerns the SLICE bridge (npc_memory_lane.rs); verdict awaits auditor-slice |
| B05 | CORE | UNTRIAGED | — | pending auditor-core |
| B06 | CORE | UNTRIAGED | — | pending auditor-core |
| B07 | CORE | UNTRIAGED | — | pending auditor-core |
| B08 | SHELL | UNTRIAGED | — | pending auditor-shell |
| B09 | REND | CONFIRMED | ee/p1-b09-metal-surface | verified `vulkan.rs:1142`; lane IN-PROGRESS; auditor cross-checked the REST of PLATFORM_SURFACE_EXTENSIONS clean |
| B10 | SLICE | UNTRIAGED | — | pending auditor-slice |
| B11 | SLICE | UNTRIAGED | — | pending auditor-slice |
| B12 | CORE | UNTRIAGED | — | pending auditor-core |
| B13 | SLICE/PERSIST | PARTIAL | — | persist half: WorkerUnavailable/WorkerReplyLost propagation mapped (medium.rs:71-78,194-205); the blocking-send/HOL caller map is slice/worker.rs — awaits auditor-slice |
| B14 | REND | CONFIRMED | — | full-size RGBA16F reflection targets per slot + HDR/bloom targets; P5 measurement package |
| B15 | WORKSPACE | PARTIAL (rend half) | — | renderer dep closure tight (ash/bytemuck/nalgebra + engine-core); full workspace measurement pending P0/P6 |
| B16 | ALL | UNTRIAGED | — | pending all auditors |

## Discovered findings (NEW-*)

Entries below are deduplicated: auditor items that restate a charter seed
finding fold into that seed's row above (noted there). Items refuted on
orchestrator verification are kept with their NOT-APPLICABLE proof — the
refutation is evidence too.

| ID | Area | State | Owner | Note |
|---|---|---|---|---|
| NEW-REND-01 | REND | CLOSED-DUP | — | restates B09 (auditor listed it independently) |
| NEW-REND-02 | REND | NOT-APPLICABLE | — | REFUTED on verification: vulkan.rs:4959 `replace_pipelines \|\| new_format != swapchain.format` — pipelines replace on format change only (required correctness); the flag is the documented forced-replacement test seam (render_instanced lifecycle gates), default false; no "every recreation" stall exists |
| NEW-REND-03 | REND | CLOSED-FOLD | — | folds into A20 (same finding: no reusable staging pool) |
| NEW-REND-04 | REND | CONFIRMED (LOW) | — | proxy_instances.rs:258 total-across-kinds bound is hand-summed; future-kind hazard — A11-adjacent doc/type hardening, P6 |
| NEW-REND-05 | REND | CONFIRMED (LOW) | — | scene.rs:319 undocumented +512 margin — P6 doc note |
| NEW-REND-06 | REND | CONFIRMED (LOW) | — | POSE_STRIDE comment could name mat4 = 4×4×f32 (11×64=704 ✓) — P6 doc note |
| NEW-REND-07 | REND | NOT-APPLICABLE | — | REFUTED on verification: the F-03 fail-visible truncation machinery (region decision.truncated/omitted counters; layered-upload test vulkan.rs:5228 `layered_upload_preserves_order_truncation_and_neighboring_slots`) already bounds the upload — no OOB path |
| NEW-REND-08 | REND | CLOSED-FOLD | — | folds into A18 (per-frame repack is the render-then-step design; static-population caching is the P5 dirty-generation package) |
| NEW-REND-09 | REND | CLOSED-FOLD | — | folds into A19 |
| NEW-REND-10 | REND | CLOSED-FOLD | — | folds into A11/A23 (pose-capacity mirroring MAX_INSTANCES) |
| NEW-PERSIST-01 | PERSIST | CLOSED-FOLD | — | folds into B03 (reclaim linear membership) |
| NEW-PERSIST-02 | PERSIST | CLOSED-FOLD | — | folds into B02 |
| NEW-PERSIST-03 | PERSIST | CLOSED-FOLD | — | folds into A22 persist half |
| NEW-PERSIST-04 | PERSIST | CONFIRMED (MED) | — | composite_reader memo hit still clones payload — B03 disposition candidate (Arc/shared verified backing); P3 |
| NEW-PERSIST-05 | PERSIST | CONFIRMED (MED) | — | worker-unavailable propagates synchronously through append paths — B13 sub-item; P3 |

## Auditor calibration record (charter §3)

- **gpt-oss-120b (renderer):** thorough structure map; 2 claims refuted on
  orchestrator verification (NEW-REND-02 misread the replacement seam;
  NEW-REND-07 missed the F-03 machinery), 1 size miscount (vulkan.rs 2.8k vs
  actual 6.1k). Use with verification; good at locating/mapping, weaker on
  semantic judgment of test seams.
- **gpt-oss-120b (persist):** useful line-level census; verdict LABELS needed
  reconciliation against its own evidence (A13). Same model, same session
  type — verify labels, trust located facts.
