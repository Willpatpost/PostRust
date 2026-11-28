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
| A01 | PERSIST/REND/CORE | **FIXED-AWAITING-RE-AUDIT (rend + persist halves)** | rend S4 `46f29e3`, persist L9 `622623b` (lane `3f050bc`) | the rend half: vulkan.rs 6,122 → root + 7 modules + 8 pass files (pure moves, multiset-proven, the visibility inventory in the commit); the persist half: local 13,204 → root+6, sqlite 8,439 → root+5, reclaim 7,258 → root+4 (change-reason splits, the token-multiset proof absent=0, lib.rs zero changes, persist 720/763 identical, main ±0 measured); renderer.rs's entry collapse via S1/S3/A04; the walk unification (L9b) in flight; CORE half open — the core audit landed (`audits/core.md`, spine read DEEP) but the spine split itself remains unlanded; the Sol + owner re-audits retain A01 |
| A02 | CORE/REND | **FIXED-AWAITING-RE-AUDIT (core half); renderer half open** | core `72c91d3` (lane `07c9c02`) | the core half LANDED (the historical-commentary compression across the seven largest engine-core files, comment-only, mechanically proven — contract stock kept, commit-narrative compressed to present-tense contracts); the renderer half remains: the charter's `scene_record.rs`/`frame_prepare.rs` targets were restructured by S2/S3 into `scene_description.rs`/`frame_prepare.rs`, and the commentary pass on those files has not been run |
| A03 | REND | **FIXED-AWAITING-RE-AUDIT (all six stages)** | S1 `954f843`, S2 `f8f308c`, S3 `70b876d`, S4 `46f29e3`, S5 `585187d`, S6 `7e4c8f8` | the staged migration COMPLETE: the submission contract (S1, rig-verified byte-equality), the scene extraction (S2), the FramePlan coordinator (S3), the vulkan.rs split (S4, pure moves multiset-proven), the shader composition (S5, 19/19 SPIR-V byte-identical), and S6 the adapter/shim/preference deletions + the Garage second scene through the public API only (closes R06); the rig-found null-pipeline defect fixed in the S6 lane and GPU-proven across every route |
| A04 | REND | **FIXED-AWAITING-RE-AUDIT** | integrated `9e17516` (lane `b8e5609`) | the §3.4 collapse LANDED: render_frame 19→6, the prepare pair 10→6, recreate_swapchain 8→6 — FramePolicy (Copy) + InternalFrame (the validated submission + the single-derived FramePlan); the post-S6 adaptations documented at the types; the zero-cost proof (the Copy pin + byte-identical batteries); §5.4's named phases landed as the honest remainder; ALL EIGHT rig batteries green at `df508da`; renderer 190 (189+1) |
| A05 | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `1396ba6` (lane `5995026`) | the WorldConfig boundary (fail-closed validated, V1_FORMAT parity) + `npc-world-save/2` (config as data, region-major, the structural dormant shape — 35 B/identity occupied-only) + v1 retained loadable with the spine domain tag the never-silent dispatch + B12's identity round-trip through real storage + the 64-region/10k-identity multi-scale demonstration (continuation-invisible, no dense-grid term); main 1517/1466 pinned at landing |

| A15 | SLICE | IN-PROGRESS (the second scene landed) | garage `7e4c8f8` | wire discipline is correct (subsystems import only public APIs); the monolithic `corner_demo` dispatch + `main.rs` CLI cascade — the garage route (`--scene garage`) landed as the second authored scene through the public API with its own demo module; the full dispatch extraction per the module classification map remains P6 |
| A06 | REND/SLICE | **THE MIGRATION COMPLETE (E-S1..E-S4: `f4ce19b`/`5afce2d`/`fcecedd`/`915e09c`, FIXED-AWAITING-RE-AUDIT)** | lane `c9700eb` → integrated `f4ce19b` + the inventory-guard companion `d9c3f24` | the new `engine-animation` crate (deps bytemuck ONLY per the ratified §4 manifest line, `#![deny(unsafe_code)]`) owning the pure pose core incl. SkinnedVertex (the decision record's own placement — the brief had drifted, the lane corrected it with the cycle argument, ratified at review) and sin_cos_turn re-homed (proxy_instances re-exporting); the GPU packing layer (repack family, MAX_POSE_BLOCKS, PosePackError) staying renderer-side per the owner clause; every existing import path stable through the shim (zero consumer edits — lib.rs root block byte-stable); ALL equivalence bars reproduced (pose digest 2_694_074_108_253_436_588 within one crate now, instance FNV, rest-identity, 396-vertex mesh); test conservation exact 211+21=232; the CI pin movements measured by the candidate run (renderer-release 232→211, main result lines +2 expected); E-S2 slice rewire, E-S3 shim decision, E-S4 B15 re-measure queued |
| A07 | CORE | CONFIRMED | — | npc_perception.rs:83-94 linear population walk; npc_execution.rs:283-306 enumerate_witnesses rides it (audits/core.md) |
| A08 | CORE | **FIXED-AWAITING-RE-AUDIT (predicate-order half)** | `77568dc` | the IA wave closed the divergence: canonical (due,seq) visitation restored on the public API (the in-place sort before the partition, no scratch, save/restore untouched); both live-and-restored witnesses red-then-green; pure-predicate byte-identity re-proven across three trees; the honest timing cost recorded (the no-match 32x win traded to 2.4x per the audit disposition own trade); the owner re-audit retains the **regional scheduler locality** remainder (D-EE-10 §6) |
| A09 | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `88628bd` (lane `c88eb7d`) | the derived witness→keys index LANDED: witness_index (BTreeMap<u32, BTreeSet<MemoryKey>>, phase-blind — confirm moves records between phases without touching the key set) + the measured-in classification_index, both maintained at the single key-set mutation site; the phase merge O(log L + own records) with zero sorting; the equivalence battery byte-identical against separate scan oracles (5 shapes × 2 seeds × both construction paths, mutation-verified); the recovery replay rebuilding identical indexes + digests (wire-invisible); scaling ~97× at 100k records (witness_records 2,563→26.5 µs), classifications ~76×, activate flat; candidate_records stays scan+sort by design (§5.3 boundary, trigger recorded) |
| A10 | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `11cf669` (P2-L5) | the StaticGrid broad phase LANDED (occupied-only hash grid, lookup-only, fail-closed domains, flat-in-population proven — the foundation B06 and the wall queries ride); the earlier "pending auditor-core" note was stale (the core audit landed, `audits/core.md`); Q04 (production adoption at the composition boundaries) and Q09 (spatial-query allocations/sorting/dedup) are the owner re-audit's retained follow-ups |
| A11 | REND | **FIXED-AWAITING-RE-AUDIT (the P5 resource block COMPLETE: R0–R5)** | design `ab7cf2a`, R0 `1ea3e05`, R1 `4d77c69`, R2 `0b1a60e`+corrective `145391c`+`0a72854`, R3 `59eac30`, R4 `d6d7254`, R5 `624488a` (night close `78b0664`) | demand-sized arenas under configured caps LANDED (E-15: corner/garage instance two-slot mapped storage 786,432→12,288 B, poses 5,767,168→90,112 B at admitted demand); the dirty-generation protocol + pose dedup (R2), the staging pool (R3), the ring localization (R4), the depth-pair aliasing decision (R5) all landed with the rig batteries green; the strict audit IV retains the E-15-residual follow-ups as T17/T18/T23 (arena idle-wait/high-water, whole-set dirty, pool net residency) |
| A12 | REND | **FIXED-AWAITING-RE-AUDIT** | integrated `585187d` (S5 lane `ef75b11`) | the shader build-time composition LANDED: the 9 variant-pair monoliths decomposed into 21 shared cores (182 lines, the 66-line lighting equation that existed ×3 among them) + 9 shells composed at build time by the byte-level splice at anchor lines; pinned naga untouched; **all 19 .spv artifacts SHA-256-identical to the pre-composition baseline** (byte-identity the migration proof); the layout-pin mechanism prototyped (build.rs resolving host-mirrored struct offsets through the same IR the spv writer consumes, cross-variant agreement + the 112-byte prefix invariant asserted, host pins via offset_of); all 5 rig batteries green through the composed shaders at `8864a88`; renderer 191 (188+3) |
| A13 | PERSIST | **FIXED-AWAITING-RE-AUDIT (both halves landed)** | L9 `622623b`, L9b `10d3d13` (lanes `3f050bc` + `2ecadf5`/`59aec7e`) | the split + the walk unification: the ~560-line shared chain_walk core with the refusal rules centralized, both providers as 120/145-line adapters, net -1082 lines; the battery-first differential (11 tests, green on the untouched tree, seeded regressions 3-for-3 + 1); the one corner divergence resolved on the documented R15 discipline with byte-identity pinned; persist 731/774; the L9b walk unification LANDED at `10d3d13` (the earlier "in flight" note was stale — the T38 reconciliation) |
| A14 | REND | **CONFIRMED (open — register row restored 2026-11-28, the Q11 reconciliation; the seed had no row)** | — | the charter §5 seed (renderer resource creation/replacement/destruction: explicit ownership, partial-failure cleanup, deferred retirement by completion, device-lost/resize lifecycle, scoped synchronization) never landed: the P4 design's decisive find — device-lost has ZERO typed handling (every vk::Result flattened to `Vulkan(String)`; grep-verified still true at HEAD `1d4ecc9`) — had its typed arm designed at S1 and deferred to the orchestrator, never taken; Q10 (the S6 attach-path partial-failure surface) is the re-audit's slice of the same umbrella; the Sol + owner re-audits both retain A14 |
| A16 | PERSIST | FIXED-AWAITING-REVIEW | integrated `f9901f3` | lane `118e193` reviewed: ordered merge verified sound (decode enforces ascending-unique at pipeline.rs:208-212; error semantics traced identical in all divergence cases); gates green, zero test changes → no pin movement; the batch gate run since landed (E-6, tree `3439633`, exit 0) |
| A17 | REND/SLICE | FIXED-AWAITING-REVIEW | integrated `3439633` | lane `3409117` + orchestrator corrective `56205f1`: rest_pose_block cached (OnceLock, once_cell dep removed on review) + SkinBind/skin_matrices_bound (the primary item the lane dropped — bind once per batch at both production sites); pins named green; renderer 172 / slice lib 118 baseline-exact; the batch gate run E-6 green |
| A18 | REND | **FIXED-AWAITING-RE-AUDIT** | integrated `0b1a60e` + the corrective `145391c` + the zero-alloc mapping | the dirty-generation protocol + the pose-dedup + the palette-semantics battery re-basing + the mapping zero-alloc (the vec form own 2-per-frame cost found by the corrective, fixed, the pins re-based); all eight rig batteries green; main 1625/1574, renderer 228 |
| A19 | REND | CONFIRMED | — | ground_reflect.wgsl:144-166 per-fragment loop over all rings; folds NEW-REND-09 |
| A20 | REND | **FIXED-AWAITING-RE-AUDIT** | integrated `59eac30` (lane `576783c`) | the staging pool (the replay-log growth, the actual-dependency sync, the bounded backpressure, the tails retired - texture bytes halved); every E-7 ink pin digit-for-digit; the in-flight safety fault-proven |
| A21 | CORE | FIXED-AWAITING-REVIEW | integrated `a213563` | orchestrator-implemented `3084a8a`: bounded retention (default 4096, amortized half-drain, lifetime `dropped_total`, NonZeroUsize capacity, opt-in echo documented); caller inventory showed all production paths run `Level::Off` — latent consumer defect closed, zero current behavior change; `&[Record]` API unchanged, `spine_main_path_allocations_are_pinned` green; +3 tests → the main shard since measured at every integration (the current pins 1529/1478 carry them); the same A21 shape landed in the shell as R05 (`e2aa89e`) |
| A22 | CORE/PERSIST | **FIXED-AWAITING-RE-AUDIT (both halves)** | core integrated `4dd78e3` (lane `032bbf6`), persist integrated `42ac117` (lane `94d97a9`) | register correction stands (no `confirmed_history` at HEAD); BOTH halves LANDED — persist: the settled-transaction watermark (monotone prefix mark + out-of-order bitmap, advance = the actionable scan's own settlement proof; per-commit envelope-scan delta ≤2 vs the pre-watermark quadratic 7,260 at 120 transactions) + the bounded coverage shedding (1,024 retained FIFO, authority never dropped, empty-coverage export refused) + the reported-never-auto-triggered cadence; core: the RetentionPolicy{retain,chunk} opt-in cadence wrapping the existing compact (no second mechanism; 3×300 policy firings ≡ one explicit 900 cut byte-identical, pinned), firing at the single end-of-run_tick checkpoint, hysteresis gate, the sustained-load bound demonstrated (50,000 executions, ledger ≤428 at every boundary); the two [ASSUMPTION] thresholds (1,024/4,096; retain/chunk defaults) await the owner-rig data point |
| A23 | REND/SHELL | CONFIRMED (rend half) / MEASURED (shell half) | — | pose storage dynamic-offset per slot (skinned_draw.rs:949-950, POSE_STRIDE=704); shell half MEASURED by the thorough audit: device layer ≈434 KiB (voice pool 64×32B, ring 256×72B, scratch 24 KiB, resident PCM 399,360 B) + cue surface ≈326 KiB, all construction-time with residency pinned — no blanket replacement warranted; the open question is the two coexisting surfaces (NEW-SHELL-3, design review) |
| A24 | SLICE | VERIFIED-CLOSED | integrated `1884582` | **rig-verified 2026-11-26:** all 11 `render_weather` Vulkan gates green on the RTX 5070 Ti (exit 0, 7.29 s) with the pinned values reproducing digit-for-digit (ink 17041, broadside 3482/2162, the allocation tuple, sweep 22/255, the term-separation counts) — the scripted route's census equivalence proven on the real GPU; heavy 664/46/64 measured (pin corrected); scripted route frame-indexed by design, free-look on one coherent accumulator (crowd+rain+day/night, monotone cycle, minimized accrual, dt bit-identity pinned `0x3c888889`); two defective lane rounds caught at review, final surgery by orchestrator |
| A25 | SHELL | FIXED-AWAITING-REVIEW | integrated `7c30aee` | **auditor PASS verdict REFUTED by direct read** (start_voice ran inside the lock at audio.rs:912 — the auditor's "releases before heavy work" was backwards); orchestrator-implemented fix `fa13e26`: copy-out under try-lock → guard dropped → voice-start unheld; pairing preserved; shell battery green, zero test changes |
| B01 | CORE | CONFIRMED | — | spine.rs:1767-1786 drain_due counts executions only (blocked moves don't count); charter's structural-gap framing stands (audits/core.md) |
| B02 | PERSIST | **FIXED-AWAITING-RE-AUDIT** | L1 `19fe59d`, L4 `f437bd7`, L5 `79b2fde` | all three halves LANDED: the bounded header-prefix reads (L1), the recovery single-decode (L4 — the open-then-derive shape removing the two-unguarded-reads coincidence, legacy 2→1 full decodes, bound 3→2 + a ~100-byte prefix read), and the streaming framed reader (L5 — FrameReader replacing all six whole-read sites, working-set O(decoded)+one frame against the old O(file) exact-reserved vec up to 4 GiB, the differential oracle 3,480 comparisons with three seeded regressions caught, teeth proven) |
| B03 | PERSIST | **FIXED-AWAITING-RE-AUDIT (all three halves)** | reclaim `c084230`, pipeline `8636d89`/`54cc9e9`, composite Arc `23609bd` (lane `9f41f9e`) | reclaim dedupe LANDED (with the inversion corrective + teeth-proven test); pipeline copies LANDED (the commit path 3 key-set copies → 1); the composite Arc backing LANDED (the design's Option 1: `MemoEntry.payload`/`VerifiedChunk.payload` → `Arc<[u8]>`, payload 1× + refcounts, memo hits cloning the refcount only; the sharing proven by Arc::ptr_eq, the trust boundary restated — the memo stays per-operation, the medium-mutation test failing closed on the fresh memo; no wire change; the module stays dark so the K2-C activation never mixes a representation change into the authorization checkpoint) |
| B04 | SLICE/PERSIST | **FIXED-AWAITING-RE-AUDIT** | `a8e8d04` | the IA wave closed both boundary holes: the three-phase RetryPhase state machine (Unjournaled / Indeterminate(retained attempt) / Journaled) with exact-attempt reconciliation before any fresh staging (byte-identical envelope+plan, fail-closed on divergence), the atomic marker restoration, and the IA-05 live-handle completion on AttemptUnresolved; all three fault-sequence tests red-then-green; fuzz campaign byte-identical; heavy 667/47/64 measured |
| B10 | SLICE | **FIXED-AWAITING-RE-AUDIT** | integrated `63b01d4` (lane `87fe1aa`) | both audit premises refuted by direct read (no restore in either arm; no frame cadence in the harness — the arms different work IS the measured quantity); the methodology labeling + the new section-4b stall worst case measured release with frozen pins (outstanding 12 / pending 1,000 / lateness 0/169/6); the cadence-0 assessment: the shipped E-8 skip decomposition was NOT produced under cadence 0 (structural refutation) |
| B11 | SLICE | **FIXED-AWAITING-RE-AUDIT** | integrated `63b01d4` (lane `87fe1aa`) | the labels corrected (admitted = generated with the 320:1 decimation meaning never-generated not occluded; existence never a rendered-pixel claim) + one genuinely-wrong counter corrected with the owner-revert note (contacts had summed all-cause deaths; no shipped number moved — both runs 4,809) |
| B05 | CORE | **FIXED-AWAITING-RE-AUDIT** | `1815a2d` | the IA wave closed both escapes: the checked update (nonfinite integrated state = deterministic death, the census identity mechanical, the impossibility argument documented) and the already-below arm totality (per-axis post-else-pre); both audit witnesses red-then-green; the trajectory digest pin green; main 1385 measured at the fix |
| B06 | CORE | **FIXED-AWAITING-RE-AUDIT (both halves)** | tie `356ef92`, combat `d489b7f` (lane `60d170c`) | the tie conformance fix (orchestrator `9a064c8`) + the combat broad phase LANDED: the CombatIndex (persistent uniform hash grid over a roster's hitboxes, the StaticGrid pattern — lookup-only, candidates re-sorted to slice position, the shared narrow phase verbatim through consider_combatant; the public signature/allocation contract unchanged, the frozen fingerprint pin passing); gate 128 with every existing scenario linear (real counts two orders below); the equivalence battery bit-compares by f32 pattern (7 rows × 3 seeds × 2 wall worlds × 48 queries + the permutation arm); scaling ~600× at 32k for the typical 30 m shot, the honest diagonal waste row documenting the DDA trigger; select_target stays linear with measured evidence (154 ns @4 targets, input-edge frequency); the engine-slice adoption seam recorded (FireContext carrying Option<&CombatIndex> — the integrator's wiring) |
| B07 | CORE | RESOLVED — NO-CHANGE (with evidence; census CORRECTED 2026-11-27 per the audit's IA-07) | — | the no-change decision HOLDS for the shipped workload (the corner constructs ONE rule; validation is an allocation-free optional-index check; the wake selector short-circuits on the due commitment; a public mutable vector can change contents without changing its address — per-call validation IS the safe boundary for raw `&[Rule]`). **The census correction:** the earlier "every repository caller passes at most one rule" was FALSE — two-rule tables reach both evaluators in npc_execution's test fixtures and the shared campaign builds three rules; the one-rule bound holds for the shipped corner and the named measurement fixtures only. The design's "measured" cost claim is corrected to an operation count (no benchmark ran). Revisit triggers unchanged |
| B08 | SHELL | **FIXED-AWAITING-RE-AUDIT** | integrated `6cf1a0d` (the codex lane `95408c9`) | the input edge-buffer reuse: the red census measured (up to 18,944 B/frame on edge-bearing frames), the caller-owned reuse (frame_into returning the cleared buffers, one vector per kind retaining high-water capacity), adopted in run + all 11 slice loops, the green census all-zero, +2 shell tests; the codex independent turn first implementation |
| B09 | REND | FIXED-AWAITING-REVIEW | integrated `9dce061` + corrective | lane `2ddef05` reviewed: metal literal fixed + never-existed display_surface removed + one focused test; **orchestrator corrective restored `VK_MVK_macos_surface`** (deprecated but REAL — legacy MoltenVK loaders advertise it; its removal re-opened the exact misclassification class the fix closes); lane citation inaccuracy recorded (VK_KHR_display does provide vkCreateDisplayPlaneSurfaceKHR); +1 test → the pin correction landed with E-6 (renderer 173 measured, exit 0) |
| B12 | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `1396ba6` (with A05, lane `5995026`) | the identity integration LANDED: NpcIdentity round-tripped through real authoritative storage (the section bytes + the full spine save/restore — not a codec self-test), the v1 golden independently authored; the earlier "lane RUNNING" note was stale — the L8 lane landed and the main pins moved at `23842cb` |
| B13 | SLICE/PERSIST | **CLOSED — the evidence-backed no-change CONFIRMED by the rig data point** | the L7 instrumentation `3057f66` + the release confirmation `096f8d9` (the rig run) | the sustained-commit-cadence release capture on the rig: commit-stall mean 8.83 ms / max 10.66 ms against the 16.67 ms budget — INSIDE at an honestly re-stated ~1.6x margin (the E-8 draft 5.97 median was another session; both inside budget); the wait/service split confirming the draft decomposition (the observation stream p99 4.8 ms waiting behind the append fsync service mean 5.13 ms); the trigger sharpened: the margin closing below ~1.3x or sustained-load growth → Option 2 (the submit/poll deferral) the ready mechanism; the release-mechanism itself stays unbuilt per the no-change |
| B14 | REND | **FIXED-AWAITING-RE-AUDIT (the split decision)** | integrated `624488a` (lane `1f7fadc`) | the depth pair ALIASED (~15% target residency, the iGPU census measured on the AMD adapter, the ink gates identical incl. the aliased-composition pixel gate); the color pair NO-CHANGE with the forensic reasoning and trigger; the record E-18 |
| B15 | WORKSPACE | **MEASURED (workspace half; report `FOOTPRINT.md`)** | lane `ee/b15-workspace-footprint` | the full workspace measurement LANDED (docs/engineering-excellence/FOOTPRINT.md, evidence E-11): core **zero-dep confirmed** (0 normal/build/dev), persist 19/24/24 external, renderer 15/52/56, shell 5/13/13, slice 35/69/69 unique-closure; demo binary 15.61 MiB debug / 10.02 MiB release, cold full release build 48 s; the minimal-consumer battery: core 2.9 s/2 crates, persist 9.0 s/26, shell 44.5 s/15 (SDL static = 38.4 s of it), renderer 41.9 s/54, slice 52.6 s/71 — all five run through public API only; **the earlier rend-half note's "nalgebra" was WRONG** (no nalgebra anywhere in the closure — math is hand-rolled; corrected by measurement); headline owner decisions measured: rusqlite unconditional in persist (sqlite feature-gate lever, local-only consumer pays 3.5 s SQLite C compile + 10 crates), renderer has NO types-only surface (plain FrameCamera/SceneBounds still compiles SDL+ash+naga), sdl3-sys caret drift → NEW-REND-11; dev-dep isolation verified closure-neutral (slice dev delta ZERO crates; composite-test-fixture/crash-test-seams pull nothing); stripped-size measurement BLOCKED by the pinned minimal toolchain (no strip binary) — recorded as method boundary |
| B16 | ALL | IN-PROGRESS (open mandate) | — | all five area audits LANDED (`audits/*.md`) with coverage limits recorded honestly (core 7 DEEP / 26 SKIM; the slice audit's SKIM/UNEXAMINED overlap); the B16 uncovered-areas mandate itself remains open per the Sol + owner re-audits (the integration modules — crossing, session_chunks, orchestration — the GLSL inputs, long-run history, resource teardown) |

| Q01 | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `bce11d3` (lane `e4d5c38`) | the bounded cell construction: GRID_OVERSIZED_CELL_LIMIT 64 + the overflow-oversized list on both grids, one entry never amplification, every query consulting before the same re-sort discipline - bit-identity and the order tie proven, the whole-domain collider one entry where the old loop was a hang |
| Q02 | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `bce11d3` (lane `e4d5c38`) | the roster/ownership contract: CombatIndex borrows the roster (the CensusIndex precedent) - the wrong-roster case unrepresentable, the old wrong answer reconstructed and pinned, the full B06 battery re-proven under the new contract |
| Q03 | CORE | CONFIRMED (owner re-audit) | — | scheduler housekeeping accounting permits arbitrarily large atomic waiter-release overshoot — establish the actual bounded-work contract preserving authoritative semantics |
| Q04 | CORE | CONFIRMED (owner re-audit) | — | NPC perception + combat broad-phase lack complete production adoption — retain small-workload linear paths while integrating indexed paths at composition boundaries (the adoption seam B06/L4 recorded) |
| Q05 | CORE | CONFIRMED (owner re-audit) | — | history retention conditional on configured cadence, requires restoration of configuration, may copy substantial retained history during compaction — verify consequence eligibility before automatic compaction |
| Q06 | REND | CONFIRMED (owner re-audit) | — | scene generalization incomplete despite the second scene — fixed 3-material tables, procedural texture dimensions, fixed reflection ranges, one-scene-per-renderer-lifetime; reusable resource + scene contracts needed (the PASS_FAMILY boundary S6 documented + beyond) |
| Q07 | CORE | CONFIRMED (owner re-audit) | — | configurable npc-world decoding resource admission, cumulative count validation, scoped identity architecture (the L8 boundary follow-up) |
| Q08 | PERSIST | CONFIRMED (owner re-audit) | — | historical persistence metadata resident despite the watermark + shedding — proportional active working sets + authenticated cold historical retrieval |
| Q09 | CORE | **FIXED (measured NO-CHANGE)** | integrated `bce11d3` (lane `e4d5c38`) | the allocation census measured: the linear arms and the census warm path exactly zero (pinned), the mover arms 1 alloc/32 B against the 100-585x win, the adversarial rows honest with the DDA trigger; the scratch-handle trigger named at the Q04 adoption seam |
| Q10 | REND | CONFIRMED (owner re-audit) | — | scene-attachment failure atomicity when resources attach but pipeline creation fails (the S6 ensure path's partial-failure surface) |
| Q11 | DOCS | CONFIRMED (owner re-audit) | — | the program README, package DAG, and finding register vs the actual tree — one authoritative current-status representation |
### The Codex full-engine audit (2026-11-28, at `95408c9` — `docs/engineering-excellence/audits/full-95408c9/`)

An additional independent audit (23 findings, two executable witnesses). The four
highest-impact claims were verified by direct reproduction before acceptance
(CA-04 both halves, CA-16, CA-05, CA-06); the remainder are registered
audit-claimed with verification-first mandated in each fix lane's brief.

| ID | Area | State | Note |
|---|---|---|---|
| CA-01 | REND | **FIXED-AWAITING-RE-AUDIT** | integrated `483de34` (lane `bf04768`) | the CornerPipelineGroup guard + the attach rollback + the together-publishing; the worse-under-it finding (the step-by-step live PipelineState writes) also closed; 13/13 fault sites retry-attaching validation-clean on the rig |
| CA-02 | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `b9709c6` (lane `9dce696`) | the decode admission: SectionDecodeLimits + the four pre-reservation gates + fallible try_reserve (the typed Overflow where the process aborted); the ~62x precision recorded; the counting-allocator census the documented follow-up |
| CA-03 | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `b9709c6` (lane `9dce696`) | the HousekeepingStats observability + the atomic release/order contract documented before any cursor; the no-partial-release proof surface; the backlog slope published |
| CA-04 (F04) | CI | **FIXED (verified before fix, `3311fd9`)** | the main result-line check 33 vs the tree's 34 + the heavy expected=674 glued into a comment (Linux comparing 674 actual against the stale 549) — both corrected |
| CA-05 (F05) | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `838a304` (lane `c4d993c`, rows registered `bf2c6dd`) | RotatedCollider's geometry fields made PRIVATE with the validated rebuild-on-mutation surface (the stale-cached-bounds class closed; collision.rs:54-56 documents the F05 contract; the strict audit IV §2 confirms the correction) — the earlier "CONFIRMED (witness reproduced)" row was stale, corrected by the T38 reconciliation |
| CA-06 (F06) | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `838a304` (lane `c4d993c`, rows registered `bf2c6dd`) | the v2 section's in-memory validate() now inspects active positions (the shared validation direction with restore; the (4,0) round-trip witness green) — the earlier "CONFIRMED (witness reproduced)" row was stale, corrected by the T38 reconciliation |
| CA-07 | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `b9709c6` (lane `9dce696`) | the ConsequenceEligibility token + the refusal arm with lag; the equivalence pinned with unresolved consequences; the in-place drain (zero-allocation sustained loop); the per-firing operation-count table |
| CA-08 | CORE | **FIXED-AWAITING-RE-AUDIT (the honest qualification recorded)** | integrated `f8de20f` (lane `60abe93`) | the class-keyed derived index (lazily materialized, wire-invisible, bit-identical three-twin equivalence); held flat 490x, the dormant common case constant, the with-members queue side 3.8x with the keyed-lane boundary documented |
| CA-09 (F09) | CORE/SLICE | **FIXED-AWAITING-RE-AUDIT** | integrated `30126fa` (lane `e22f610`) | the indexed-query production adoption boundary LANDED: the FireRoster seam (Option<CombatIndex> built in the roster's scope, the roster/query gates 2048/32), every live boundary measured LINEAR at the demo scale (the honest no-boundary-adopts answer with the crossover table), the census boundary wired with CensusIndexScratch; bit-identity proven at three roster sizes × three shapes × the corrupted entries × the demo worlds — the earlier "REGISTERED (audit-claimed)" row was stale, corrected by the T38 reconciliation |
| CA-10 | REND | **FIXED-AWAITING-RE-AUDIT** | integrated `483de34` (lane `bf04768`) | the deferred fence commit + dispose_failed_acquisition (the WSI-canonical release, the fence heal, the drain proof, the semaphore replacement, the typed FrameSlotUnrecoverable); 4/4 boundaries retrying in 6-26 ms where the strand was one 2-second timeout; the battery catching two draft designs at the device level |
| CA-11 | REND | **FIXED-AWAITING-RE-AUDIT** | integrated `483de34` (lane `bf04768`) | the ScopedMappedBuffer guard at all three growth sites; 3/3 paths double-failed then retry-grown with the census pairs pinned |
| CA-12 | REND | **FIXED-AWAITING-RE-AUDIT** | integrated `a45a60e` (lane `21a173d`) | the shared representable capacity (the 2^31 ceiling, the pose device facts, the checked round-up, the admitted-bytes backstop - the release wrap-to-zero-carve hazard typed) |
| CA-13 | REND | **FIXED-AWAITING-RE-AUDIT (A14 CLOSED)** | integrated `a45a60e` (lane `21a173d`) | the typed DeviceLost at the vk() choke point + the first-observation-wins latch gated on every device-touching entry; the injection seam proving the unarmed second submit refuses |
| CA-14 | REND | **STAGE-1 LANDED + THE DESIGN RECORD** | integrated `a45a60e` (lane `21a173d`) | the scene_contract_design.rs record with the four-step migration; the typed TextureExtent + DrawTable first step; the generalization steps designed with per-step acceptance tests |
| CA-15 | REND | **FIXED-AWAITING-RE-AUDIT** | integrated `a45a60e` (lane `21a173d`) | the triangle-boundary checks at admission + the per-range validation with the misaligned witnesses |
| CA-16 (F16) | PERSIST/CI | **FIXED (verified before fix, `3311fd9`)** | the crash-seam facade regression from the L9 split — the exact CI heavy command E0432; the two names restored |
| CA-17 | PERSIST | **FIXED-AWAITING-RE-AUDIT (stage 1; the eviction designed + gated)** | integrated `5b36258` (lane `c2663d9`) | the cold retrieval landed (`cold_confirmed`, both providers, the fail-closed arms); the residency measured (linear ~195-216 B/identity); stage 2 (the rotation-authored ledger + eviction) gated on the measured cold-latency rows with the [ASSUMPTION] owner decisions |
| CA-18 | SLICE/PERSIST | **FIXED-AWAITING-RE-AUDIT** | integrated `5b36258` (lane `a6084cd`) | the streaming hash (peak 182-184 B vs 1-32 MiB whole-file); the manifest wire unchanged |
| CA-19 | PERSIST | **FIXED-AWAITING-RE-AUDIT** | integrated `5b36258` (lane `abeb2ed`) | the native sqlite3_backup path (one lossless boundary, the overwrite typed) |
| CA-20 | SLICE | **FIXED-AWAITING-RE-AUDIT** | integrated `5b36258` (lane `a757ebc`) | the durability tier published + made real (the probe proving the old idiom failed 100% on Windows); the typed propagation before durable completion |
| CA-21 | SHELL | **FIXED-AWAITING-RE-AUDIT** | integrated `42ff2a1` (lane `ce2dab5`) | the producer lock bounded (the A25 mirror; red demonstrated); the admitted set, FIFO, pairing, refused total provably identical |
| CA-22 | SHELL | **FIXED-AWAITING-RE-AUDIT** | integrated `42ff2a1` (lane `ce2dab5`) | the cue backpressure (the explicit budget + refusal telemetry; the mixer-voice routing left as the owner NEW-SHELL-3 consolidation); the 800-push burst refusing 687 fail-visibly |
| CA-23 | SHELL | **FIXED-AWAITING-RE-AUDIT** | integrated `42ff2a1` (lane `ce2dab5`) | repeats a mapped-key count (the intent contract); red demonstrated |
### The Strict Audit IV (2026-11-28, at `096f8d9` — `audits/strict-audit-IV-096f8d9.md`, the owner-delivered comprehensive report)

40 numbered findings with corrective designs, closure matrices for every prior register, the mandatory
acceptance-measurement matrix (§7), the source-coverage ledger (§8), the five-wave execution plan (§9),
and the final acceptance criteria (§10 — the hard-blocker list governs P7). Register intake only —
every T-row awaits the morning's independent verification-before-acceptance per the owner directive.
Classes: CONFIRMED (source-derived), HYPOTHESIS (reproduce-or-refute mandated), OBLIGATION
(architectural), INVESTIGATION (measurement-gated), REPORTED (integrated, needs exact-head evidence).

| ID | Class | Cross-links | Note (one line) |
|---|---|---|---|
| T01 | CONFIRMED (remote) | F04/CA04 | **VERIFIED-CLOSED (2026-11-28): run `46127751223` at exact head `9690284` — ALL FIVE JOBS GREEN** (heavy ubuntu+windows, main ubuntu+windows, macos ARM64; steps after the previously failing check executed green; the new count-gate self-test step green on every main job; the echo/enforced/actual triples agree at 720/60/72 both platforms — logs preserved `~/postrust-logs/t01-evidence/`). The repair `9690284`; any later code change requalifies on its own run |
| T02 | HYPOTHESIS (strong) | CA11/A18 | **IMPLEMENTED (L4 `e348850`)**: the carve allocation_epoch with (epoch, generation) stamps — the fragile invariant eliminated at the resource identity (a replaced carve's fresh epoch can never equal a surviving stamp's); composes with T05's identity clears (neither subsumes the other); the delta clear a documented belt; the interrupted-growth retry battery rig-deferred (written, named) |
| T03 | CONFIRMED ordering + HYPOTHESIS error | Q10/CA01 | **IMPLEMENTED (L3 `c5b4c4b`)**: drain BEFORE publish; non-loss drain failure → the ticket-tied quarantine (budget freed, handle unpublished, retry via step-0 reconciliation); loss latched; the in-flight-destination rule at every InlineWait caller (the textures.rs hazard fixed); the idle audit classified with diagnostics; the drain-timeout quarantine batteries rig-deferred (written, named) |
| T04 | CONFIRMED (S0, owner contract) | A11/CA15 | **LANDED `1e998d4`** (lane `482d469` red witness + `15c5307` fix + `9c304cb` re-pin, FIXED-AWAITING-RE-AUDIT): the whole-triangle admission unit, the triangle-floor draw bound, malformed-tail rejection, the `admit_layers` one-plan-before-upload with per-layer shares and the foreground-then-world complete-unit partition, the typed unit/vertex reporting with `omitted_units·3 + malformed_tail == truncated`; the red witness demonstrated the audit's defect verbatim; the render_overflow smoke rig-deferred (no Vulkan loader on this pod, evidenced — CI-Linux mesa the authority); the authored whole-object admission documented as caller-side by contract |
| T05 | CONFIRMED + strong hypothesis | Q02-analog/A18 | **LANDED `fd3f7da`** (lane `902865d`, FIXED-AWAITING-RE-AUDIT): the hypothesis CONFIRMED by four red witnesses (the colliding-generation Reused with stale bytes; the byte-identical partition arm; the missed-bump debug panic as the only tripwire; the slot/indirect gates skipping); InstanceSourceToken/PoseSourceToken renderer-assigned in adapt_frame (private fields, gate-only metadata, never in packed bytes/digests), token mismatch → generation record reset → the R2 core gate re-admits; indirect-count identity separated from payload identity; slot invalidation at submission time; pose-index validation at admission (the T18 direction); the residual address-recycling hazard documented with the PopulationHandle future hardening; no caller adaptation; no digest pin moved |
| T06 | CONFIRMED latch gap | CA10/CA13 | **IMPLEMENTED (L1 `0238b86` + L5 `e688c01`)**: the RendererLifecycle substrate (any-FSU poisoning incl. the release-half arm, drop observe-and-record, the one-get_fence_status classification at all four bounded waits) + the first-use two-arm release with the minimal UNDEFINED→PRESENT_SRC transition; the render_layers latch + clean-drop and the first-use validation batteries rig-deferred (written, named — the deciding WSI run) |
| T07 | CONFIRMED (S0 for consequences) | Q05/CA07 | **CI-VERIFIED GREEN** (run `46131503857` at `4d17f14`, all five jobs — the +12 pin arithmetic confirmed on both platforms) — **IMPLEMENTED `c1d9b1a`** (lane `6a51ea0`+`25108d6`, FIXED-AWAITING-RE-AUDIT): the two-class eligibility (ConsequenceFreedom + the deprecated alias), the per-consumer monotone eligible-prefix frontier with typed late-join refusal, the clamped cut `min(excess, chunk, eligible − summarized)`, observable lag/refusal counters (never shedding), the logical head + amortized compact preserving every public slice contract; BOTH red witnesses captured pre-fix (the standing token cutting the 30 records a would-be consumer needed; the drain memmove); the four measurement obligations discharged (stride asserted 32 B by measurement, the amortized bound witnessed across three regimes, the sustained loop zero-allocation, the main-path pins unmoved); every integration target unchanged; the design reconciliations documented not silent |
| T08 | CONFIRMED | B01/Q03/CA03 | **IMPLEMENTED** (design `ba0a089`; lane `e63f7fa`+`903d437`+`505b0ae`): the charge identity closed, the three admission caps before any mutation (D/F/K with typed refusals pre-seq-allocation), the twelve batteries incl. the closed-form ceiling witness and the 100k drain; B01/Q03/CA03 implemented-PENDING-MEASUREMENT (the four obligations in `505b0ae`'s message — B01 not claimed closed by observations); the allocation pins unmoved; +12 lib tests |
| T09 | CONFIRMED | A08/CA08 | **INVESTIGATED — PARKED WITH TRIGGER (the measured no-change, the a17/b07/r07 precedent shape)**: the stage-1 census (lane `5b1afa7`) measured the residual real (linear, CA-08's 3.8× reproduced, entries=N exactly) but the workload warrant FAILING — the region director the only production caller, compile-time-capped ≤~875 unrelated queued at transition, the entity-scale harnesses never calling the class-keyed APIs, the cadence ≤0.3% of a 60 Hz frame; the indexed lane fully designed and parked behind the reopen trigger (≥5k unrelated queued at transition in a production/rig workload — the landed batteries the ready-made slope evidence); A08's queued half remains the honest documented gap |
| T10 | CONFIRMED | Q01/A10/CA05 | oversized lane bounded per object, not localized over many |
| T11 | CONFIRMED | Q04/CA09/B06 | spatial adoption conditional; large live composition unproven |
| T12 | CONFIRMED (deferred stages) | R06/Q06/CA14 | scene generalization — CA14's stages 2-4 |
| T13 | CONFIRMED | Q08/CA17/A22/R07 | historical persistence residency linear |
| T14 | CONFIRMED code + integrity hypotheses | CA14-adjacent | cold lookup work/trust boundary not scalable |
| T15 | CONFIRMED | A20 | staging budget a fixed append frontier, not pressure-aware streaming |
| T16 | CONFIRMED (S0 with T06) | CA13 | **IMPLEMENTED (L2 `de8bedd`)**: typed StagingAvailability (only unsupported-memory sticky), the classified ordered-prefix poll (loss never masked), the UploadTicket/FlushDisposition RAII, teardown idle classified and recorded; the classified-prefix-on-live-queue battery rig-deferred (written, named) |
| T17 | CONFIRMED growth waits | A11/A23/CA12 | arena growth waits whole device; residency high-water |
| T18 | CONFIRMED | A18 | whole-set generation only coarse dirty tracking |
| T19 | CONFIRMED | A06 | **COMPLETE (E-S1..E-S4 all landed and verified)** — the extraction with every equivalence bar held and the E-S4 re-measure confirming the ratified near-core closure (8 unique / the bytemuck chain, −46 crates vs the renderer, digest reproduced standalone); T32's render-types half remains (the FrameCamera/SceneBounds types-only surface) |
| T20 | CONFIRMED direct paths | R02/A19 | numerical admission inconsistent frame/asset boundaries |
| T21 | CONFIRMED | Q07/CA02 | **LANDED** (lane `adaab2e`+`51006f0`+`a6dfe3a`, FIXED-AWAITING-RE-AUDIT): the total decode-transient budget with every reservation charged at 64-B-granular actual capacity before the request; the whole-input minimum proof (typed, never rejecting valid encodings); the seen-ID BTreeSet replaced by the bounded k-way merge (no dense bitset, no ID-policy change); the public `decode_reservation_bytes` mirror with the off-by-one boundary; v1 fallible reserves; the default's transient arithmetic stated (~1.8× input + ~54 KiB vs the pre-gate ~62×); the red witness inverted (the 5.6×-budget regions reservation now refusing typed before it happens) |
| T22 | OBLIGATION (open) | A14 | distributed fallible bare-handle ownership ladders |
| T23 | CONFIRMED-REPORTED | A20/B14/B15 | pool net residency + measurement narrative |
| T24 | CONFIRMED | A19 | ripple one global circle, not per-region culling |
| T25 | REPORTED | B14 | target sharing needs workload-specific matched proof |
| T26 | OBLIGATION (open) | A26-class | CPU whole-scene/proxy capacity architecture |
| T27 | INVESTIGATION | A21/R05 | frame stats/timestamp bounded meaningful semantics |
| T28 | CONFIRMED | B13 | worker measurement does not resolve authority-thread blocking |
| T29 | CONFIRMED structure | B02/B03 | **IMPLEMENTED (commits 1–2; commit 3 PARKED as DECISION-T29-prefix-summary)**: the census measured (the 2·o_max 11.01 MiB peak → the 65,536 B window; the churn → pins zero; whole-read volume 0 B after); the streaming verified reads with the L5 oracle discipline (3 seeded regressions caught); the borrow-not-clone fixes; the equivalence proof green both providers over the full corpus; the rusqlite blob finding handled via the sanctioned substr fallback (no manifest change); the wall-time trade honest (~15–35% cold open — the parked summary the asymptotic fix) |
| T30 | REPORTED/DESIGN | R07/B04 | transaction state-machine consolidation staged |
| T31 | CONFIRMED improvements + proof needed | CA18-20 | backup/restore durability + path portability cross-provider |
| T32 | CONFIRMED | B15/A06/F03 | animation/render contract footprint outstanding |
| T33 | IMPLEMENTED (evidence due) | NEW-REND-11/F02 | SDL pin + sqlite follow-through — exact-head evidence |
| T34 | REPORTED | B08/A25/CA21-23 | input/audio sustained burst/latency review |
| T35 | SOURCE-HISTORY | R03/R04 | free-look timestep extreme + composition order |
| T36 | CONFIRMED | A01/A02/A13 | large files/change responsibilities remain |
| T37 | INVESTIGATION | Q09/A10/B06 | spatial foundation vs avoidable duplication |
| T38 | CONFIRMED (recurred) | Q11/B16 | **RECONCILIATION PASS LANDED 2026-11-28** (`ee/overnight-t38-register`): the six stale RUNNING rows (registry + README + FINDINGS: p5-r1/r4, l9b, sdl-pin, q01-q02-q09, q11, ca05/ca06, ca09, the A06 ratification tail, the CI pins block) corrected against the actual git history; the durable maintenance obligation (every landing updates register + README together) restated below the README's authoritative-status header |
| T39 | CONFIRMED (S0-gating) | B10 | exact-tree workload-matched measurement methodology |
| T40 | OBLIGATION (open) | B16 | whole-source coverage + threat/usage ledger |
### The overnight-review corrective findings (2026-11-29, at `2639ec2` — R1–R6, confirm-or-refute with witnesses)

| ID | Linked | State | Note |
|---|---|---|---|
| R1 | T05 | REGISTERED | the pointer-derived source token admitting allocator address-recycling collisions (lifetime-scoped identity required); the submission.rs ordering (instance generation updated through repacking BEFORE the pose-index admission check) — the regression witness: invalid changed instance population, unchanged pose source/generation, initial refusal, identical retry; an unsuccessful admission must not establish a reusable validated generation |
| R2 | T07 | REGISTERED | acknowledgments beyond executed history; ordinary explicit compaction bypassing the consumer frontier; incomplete consumer registration; authenticated consumer progress vs arbitrary numeric acks; safe compaction vs explicitly authorized administrative operations |
| R3 | T08 | REGISTERED | the follow-up cap cropping excess events and publishing telemetry AFTER handler execution — can mandatory consequences be lost following a world mutation? The typed admission/transactional reservation/deterministic deferral contract; the restored legacy graph exception and runtime-bound changes — no universal work-ceiling claims while admitted existing graphs can exceed them |
| R4 | T04 | REGISTERED | complete triangles necessary but not always sufficient — explicit engine-owned admission units and batch/object atomicity so callers don't independently prevent partial authored geometry |
| R5 | T21 | REGISTERED | the 64-byte-granularity accounting a reservation estimate not a universal allocator bound; the actual production save/session restore paths traced; finite aggregate admission budgets where external/untrusted input enters; wire compatibility without conflating compatibility limits with affordable execution limits |
| R6 | T39 | REGISTERED | the CI timing-threshold corrections reviewed collectively; deterministic work counts/allocation censuses/complexity witnesses preferred for algorithmic CI gates; precise latency reserved for repeated controlled measurements; no progressive relaxation of regression discrimination for runner variability |

## The Sol re-audit (2026-11-27, the continuation mandate at `8d34771`)

**NOT complete — final integration NOT authorized.** Scores: cleanliness 7.4,
modularity 7.2, scalability 6.0, maintainability 7.0, optimality 7.7,
lightweight 8.3, speed 7.5 (provisional), documentation 7.4 — every category
must independently reach 9/10 under the original directive. The verified
improvements are acknowledged and retained; the internal corrective-audit
closure is an intermediate milestone, NOT P7 acceptance. The outstanding
original findings (per the reviewer): A05, A07, A08 (remaining), A09, A10/B06,
B01, A03/A04/A06/A15 (migration incomplete), A11/A18/A19/A20/A23/B14, A22/B03
(remaining), B08/B13, B10/B11/B12, A01/A02/A12/A13/A14/B15/B16. **The largest
outstanding objective: engine work and resident memory proportional to
relevant workload rather than global population, historical state, or
demonstration-sized fixed capacities.**

| R-ID | Sev | Summary | Orchestrator adjudication |
|---|---|---|---|
| R01 | major | SceneDescription::validate misses inverted draw ranges | **FIXED-AWAITING-RE-AUDIT** - integrated `3a426b9` (lane `343824b`): the admission invariants landed with the Sol re-audit wave (the row had carried the stale pre-landing disposition - corrected by the Q11 corrective) |
| R02 | major | Scene bounds validation passes infinite endpoints | **FIXED-AWAITING-RE-AUDIT** - integrated `3a426b9` (lane `343824b`, the same landing as R01) |
| R03 | major | the accumulator iterates all ticks before the cap | **FIXED-AWAITING-RE-AUDIT** - integrated `bb38504` (lane `0462cfe`): the exact bounded arithmetic landed on the free-look timing lane |
| R04 | major | deferred presentation skips crowd ticks the accumulator consumed | **FIXED-AWAITING-RE-AUDIT** - integrated `bb38504` (lane `0462cfe`, the same landing as R03): the presentation coherence closed |
| R05 | minor | app.rs unbounded frame-interval Vec (NEW-SHELL-04 elevated) | ACCEPTED (registered P6; elevated priority) |
| R06 | major | The scene interface's fixed material/texture/reflection prototype assumptions left as transitional-permanent | **CLOSED-BY-S6** (integrated `7e4c8f8`, lane `2e69793`) — all six migration stages landed: S1 the submission contract, S2 the scene extraction, S3 the FramePlan coordinator, S4 the vulkan split, S5 the shader composition, S6 the deletions + the second scene; the four render_corner* entries, the sticky setters, the corner_content shim, and the Corner preference arm deleted (removal conditions grep-proven); the Garage (a genuinely non-corner scene) rendered entirely through attach_scene + submit_frame with its own materials/bounds/wet roles and the texel-for-texel shadow known-answer; the rig-found null-pipeline defect fixed and GPU-proven; the pass_family_material_count generalization the documented deferred boundary (fail-visible at attach) |
| R07 | minor | The memory transaction lifecycle remains structurally complicated and copy-heavy | **INVESTIGATED — NO-CHANGE AT MEASURED SCALES (with evidence; the census landed; trigger recorded)** — lane `ee/r07-transaction-consolidation`, the design record `INVESTIGATION-R07-transaction-consolidation.md` (the residency.rs precedent shape): the surface map measured (the census, 4 new `RetentionStats` residency estimates + the structural test — journal ≈ 281 B/identity, tracked ≈ 131, confirmed 195–216 (CA-17), shed 48, staged/pending 0 at fixed live obligations, ≈ 649 B/identity total at the 8,192 scale); the decisive find — the journal-side vs chain-side duplication is LOAD-BEARING (the exact-provenance confirmation compares the two independent derivations; a shared record vacates the check) and the authority/verifier half-split is the documented actor-composition architecture (the keyed-lane option rejected on it); the copy path already consolidated where it paid (B03: 3→1 key-set copies, the composite Arc); THE RECOMMENDATION no consolidation at the measured scales (the A17/B07 precedent shape), the shed-discriminator fold folded into CA-17 stage-2's design, the journal-side settlement trim (the shed's mirror on the largest term, Q08's journal half) designed and gated on the owner-rig census data point; persist 746/650-lib both feature sets green (+1 census test) |

**Standing orders from the re-audit:** reconcile the register/README with
the actual HEAD; prioritize P2 spatial, P3 remaining, P4 migration, P5
GPU/resource scalability; correct underlying mechanisms, not isolated guards;
a design document or green battery is not evidence an unimplemented
improvement is complete; continue autonomously; no merge until the full
directive + lead + owner accept.

## Independent audit (astra, 2026-11-27, INDEPENDENT-AUDIT-be910bf.md)

**Withholds acceptance at `be910bf` — accepted.** Every IA finding below is
registered with its disposition; the affected program findings are REOPENED.
The audit's overall assessment stands as recorded: the verification system
catches real errors; the closure decisions needed the independent check.

| ID | Sev | Summary | Disposition |
|---|---|---|---|
| IA-01 | **blocker** | A classification retry can make a valid memory-lane journal impossible to reopen (a reread error after a durable write escapes with episode 1 staged; the retry captures both envelopes under plan 2 → `plan-identity-mismatch` on cold recovery) — REPRODUCED on the public path with fault injection | **ACCEPTED — B04 REOPENED.** The fix traced the failure paths I named; the reread-error-after-durable-write path was outside my trace. The IA-01 fault sequence becomes the discriminating test |
| IA-02 | major | A stateful `FnMut` predicate observes heap-buffer visitation order, and save→restore of the SAME spine changes its selection (pure predicates verified byte-identical, SHA-256 on record) — the old pop loop visited canonically | **ACCEPTED — A08's predicate-order half REOPENED.** Fix shape: canonical visitation on the public API (sort the taken vec before the partition), the bulk path kept for order-independent internal callers; the live-vs-restored test added |
| IA-03 | major | Five finite ticks put +∞ in a LIVE particle (admission bounds speed·life but position accumulation rounds independently of life subtraction; life 5.96e-8 with X = inf) — REPRODUCED | **ACCEPTED — B05 REOPENED** (the finite-survivor guarantee). Fix: a checked update or deterministic refusal on actual integrated state, preserving the campaign digests + the five-tick witness |
| IA-04 | major | The kill-plane observer receives `[inf, 0, 0]` from the ALREADY-BELOW arm (my totality guard covered only the straddling branch) — REPRODUCED | **ACCEPTED — B05 REOPENED** (the observer totality). Fix: both branches finite |
| IA-05 | major | A live classification submit cannot finish its own uncertain canonical commit (`AttemptUnresolved` loop; the bridge handles only `Confirmed`) — REPRODUCED | **ACCEPTED — B04's commit-confirmation half REOPENED.** Fix: surface the pending outcome / drive reconcile before fresh submission; the live-handle recovery pinned |
| IA-06 | minor | `reconstruct_held` can introduce duplicate identities (pre-existing, requires inconsistent input) | ACCEPTED — queued (typed validation at the reconstruction boundary) |
| IA-07 | minor | B07's caller census overstated (2-rule test tables and a 3-rule campaign reach the evaluators; "measured" without a benchmark) | **ACCEPTED — the no-change decision HOLDS, the record corrected** (one-rule bound stated for the shipped corner; larger tables enumerated; cost in operations until timed) |
| IA-08 | minor | Two workflow annotations misdescribe (P2-L2's third test is lane-shape not allocation; the B05 flake cause is a hypothesis); the S1g lane's raw heavy logs were deleted | ACCEPTED — annotations corrected; the raw-artifact-or-hash rule standing for future pins |
| IA-09 | major | RIG-PENDING-01 is a FIXTURE DEFECT, not a driver difference: the test requests 264×264 but the Win32 surface stays 256×256 — the oracle and capture differ in extent. Astra instrumented it: with an actual SDL resize, **BOTH D24_S8 and D16 pass all 61 probes at 0/255** | **ACCEPTED — RIG-PENDING-01 reframed: the demonstrated repair is resizing the drawable and verifying the capture extent; the owner decision collapses to confirming the fixture fix.** My rig-pending framing was wrong |
| IA-10 | minor | The re-materialized renderer.md is NOT verbatim (edited during recovery) while labeled verbatim; one seed verdict describes a manifest that doesn't exist | **ACCEPTED — my provenance note corrected** (annotated edition, separately labeled; the original message's SHA-256 recorded) |
| IA-11 | minor | D-EE-09's narrative over-generalizes (only L1 switched in the integration checkout; L2 switched correctly in its worktree) | **ACCEPTED — the incident record corrected** with the message IDs and patch hashes astra recovered |
| IA-12 | major | E-8's 5k/20k entity rows sustain only 1,000 routine chains (the `NPC_COUNT` codec limit — the SAME read-through defect CURRENT-STATUS documented historically, which I failed to carry into E-8's interpretation) | **ACCEPTED — the rows withdrawn as scale evidence**, preserved under their actual workload description; the scale-capable codec versioned benchmark queued |
| IA-13 | minor | The census omits core `tests/` (4,471 lines) and the four GLSL inputs; the test-cell sum is 134,949 not 155k+ | ACCEPTED — E-2/E-5 to regenerate from the tag with one line convention |
| IA-14 | minor | E-3 "verified callable" overstates (catalog listing ≠ exercised); E-7 conflates thresholds with measured counts | ACCEPTED — labels corrected |

**The program's own corrections from this audit** (the honesty items it was
asked to check, and it caught the orchestrator's records wanting): the
renderer.md provenance (IA-10), the incident narrative (IA-11), the E-8
workload interpretation (IA-12), the census arithmetic (IA-13), the evidence
labels (IA-14), and the RIG-PENDING-01 framing (IA-09). All accepted and
corrected — the audit did exactly what it exists to do.

## Discovered findings (NEW-*)

Entries below are deduplicated: auditor items that restate a charter seed
finding fold into that seed's row above (noted there). Items refuted on
orchestrator verification are kept with their NOT-APPLICABLE proof — the
refutation is evidence too.

### Package designs (the architecture decision records)

- **P2-simulation-spatial.md** (glm-5.3, 1,056 lines, citations verified at
  `c7de072`): query inventory = 6 primitives in 3 families (D dynamic-actor,
  S-box static, S-ray static); **NO universal index** — two family-scoped
  broad phases (persistent uniform grid in CollisionWorld over both collider
  lists; per-tick counting-sort census grid at the composition layer), linear
  paths DEFAULT below measured gates, hash grid lookup-only-never-iterated +
  candidates re-sorted to insertion order = bit-identical results incl. ties;
  B01 = housekeeping cap as a second drain_due loop bound + derived
  remaining-dependency counts (try_unblock O(1)), no spine replacement; A09 =
  derived witness→keys index + phase merge (MemoryKey is event-first —
  witnesses are not contiguous; the wire order preserved); the spine's
  derived/canonical split makes counters wire-invisible by construction;
  **pin correction verified: perception digest is `10766371085850726021`**
  (npc_perception.rs:545 — CURRENT-STATUS:315 dropped the final digit, a doc
  typo queued for the P6 pass); B06's exact-zero tie divergence confirmed
  with the contract decision below. Lanes L1–L8 (L1–L3 orchestrator — spine/
  semantics; L4–L6 lane-able with tight briefs; L7–L8 policy/config).
- **P4-renderer-architecture.md** (glm-5.3, 1,026 lines, citations verified
  at `7961cb2`/`a972f3e`): the submission contract — one public
  `Renderer::submit_frame(FrameSubmission)` (borrow-only: typed camera,
  dynamic layer batches, optional `SceneFrame` pass-requirements
  declaration where each field names a pass and `None` means it doesn't
  run) with authored scene content behind an attach-time
  `SceneDescription`/`SceneHandle`; the 19-parameter render_frame
  collapses to 6 with the four owning states kept separate; sky/post/wet
  migrate from sticky settings to per-frame fields; adapters keep the
  demo byte-identical until S6 deletes them. Staged DAG S1–S6 (S1
  contract+adapters, S2 scene extraction with the shim-equality gate, S3
  the FramePlan coordinator + prepare triplication fix, S4 the vulkan.rs
  split as pure moves, S5 shader build-time composition — naga 29.0.4
  has no WGSL includes so build.rs-level composition with layout pins,
  S6 adapter removal + the second scene), every stage's gate named (the
  lavapipe batteries, allocation tuples, the E-7 rig pins).
  **Decisive finds:** the four entry points duplicate ~90 lines each
  verbatim; `ensure_frame_targets_and_resources` re-derives the implicit
  uses-declaration at three sites; **device-lost has ZERO typed handling
  anywhere** (every vk::Result flattened to Vulkan(String)) — the design
  adds a typed arm, fail-visible, no auto-recovery; the wet reflection's
  corner-coupling is via receiver_range()/reflected_ranges() (the
  generalization: role-tagged mesh ranges). **THE PHANTOM: the design
  caught that audits/renderer.md was never committed** — re-materialized
  from the auditor's message with a provenance note. Unresolved: the
  second scene's CLI placement, DeviceLost payload shape at S1, A06's
  animation crate extraction deferred with a recorded revisit trigger.
- **P3-persistence-architecture.md** (glm-5.3, 943 lines, citations verified
  at `bceb3fa`): B02 caller-side wins first (bounded header-prefix reads,
  frontier-from-decoded-state) then recovery single-decode then a streaming
  framed reader — **on-disk index REJECTED** (duplicates rotation's job,
  adds a trust surface); B03 reclaim dedupe + Arc backing + one retained
  pending copy (the 3× payload and 3 key-set copies traced to exact lines);
  B13 queue-wait/service instrumentation + measurement-gated deferred
  confirmation (render-thread blocking PROVEN: persist_demo commits on the
  render thread, E-8 confirmation 5.97 ms / publication p99 19.8 ms vs the
  16.67 ms frame budget) — partitioned reads/multi-worker REJECTED; A13
  walk-core extraction + module split by change reason, NO provider trait
  hierarchy; A22-corrected (above); B04-persist NO-CHANGE; B12 recorded as
  P2's coordination point. Lane plan L1–L9 (L1/L2/L3/L8 delegable, L4–L7
  orchestrator, L9 last). Unresolved: L7's same-frame-first-poll contract,
  L6's owner-rig cadence data, L8's dark-module Arc authorization.


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
| NEW-SLICE-01 | SLICE | HYPOTHESIS (MED) | — | no per-frame stall budget in demo render loops (render_demo/player_demo/playable/combat/encounter; auditor locations approximate/`likely` — verify before acting); P6 robustness |
| NEW-SLICE-02 | SLICE | CONFIRMED (LOW) | — | shared `DEMO_TICK_HZ=60` (player_demo.rs:49, grep-verified) vs sim `DEFAULT_TICK_RATE_HZ` — document/configure the relationship; P6 |
| NEW-SLICE-03 | SLICE | HYPOTHESIS (LOW) | — | evidence-collector loss semantics correct-by-design but undocumented; P6 doc note |
| NEW-SLICE-04 | SLICE | HYPOTHESIS (LOW) | — | per-demo collision-world builders duplicate the pattern (acceptable demo isolation per auditor); P4/P6 shared-utility decision |
| NEW-SLICE-05 | SLICE | CONFIRMED (LOW) | — | main.rs monolithic CLI dispatch cascade; P6 split candidate (cli.rs) |
| NEW-SLICE-06 | SLICE | CONFIRMED (LOW, harness portability) | — | `--benchmark-persistence`'s PP-B3 retention section panics on Windows (benchmark.rs:193 unwraps the reclamation-begin that local.rs:2735 legitimately refuses on the `ProcessCrashOnly` Windows tier — the codebase's own tests cfg-gate this arm, the harness doesn't); disposition: fail-visible skip or cfg-gate in the harness (P6-grade); PP-B3 baselines are pod-only until then; found during the E-8 baseline battery |
| RIG-PENDING-02 | SLICE/REND | **VERIFIED-CLOSED (the owner's decision, option 1)** | closed at current head | the shadows battery's first rig run measured hardware depth rounding (max 4.26e-6, every mismatch 1–2 ulps, the census shape identical to lavapipe's) against the lavapipe-pinned 3.0e-7; **the owner chose option 1: one tolerance sized for both platforms — 1e-5**, ~2,000× tighter than any real geometry error and ~2.3× above the rig's measured tail, with the full measured evidence and the authorization recorded in the tolerance's comment block. The rig re-run: **the entire render_shadows battery 4/4, exit 0** (the census reproducing digit-for-digit, 0 mismatches) — all five rig batteries now green on the current tree; the poison-cascade note retired |
| RIG-PENDING-01 | SLICE/REND | **VERIFIED-CLOSED** | fixed `435fc60` | the fixture defect REPAIRED and rig-verified: the gate now resizes the actual drawable (the render_encounter pattern — set_size+sync before the recreation-triggering render) with a fail-visible extent assertion; the rig run post-fix: **the ENTIRE render_post battery 7/7, exit 0** — both D24_S8 and D16 pass all 61 post-resize probes at max error 0/255, the poison cascade gone, the S1g byte-equality gate re-confirmed green in the same run. The historical rig-pending clause is discharged by the fixture repair (the oracle was always right; the capture extent was wrong). Sister latent shape flagged: render_reflection's step-2 wet-only tuple-only resize — currently harmless (extent-agnostic checks), the same divergence class, queued as a P6 note |
| NEW-SHELL-1 | SHELL | CONFIRMED (LOW) | — | cue-surface push path has no in-flight-sample bound (SDL_PutAudioStreamData buffers unconditionally — verified in vendored SDL 3.4.18); P6 doc/budget note to match the device layer's refusal discipline |
| NEW-SHELL-2 | SHELL | CONFIRMED (LOW, grep-verified) | — | lib.rs:13-15 stale scope docs name audio as excluded while `pub mod audio` is first-class; P6 one-paragraph fix |
| NEW-SHELL-3 | SHELL | CONFIRMED (design review) | — | two coexisting audio surfaces (cue push ~326 KiB vs callback mixer ~434 KiB, each its own SDL device) — an UNDECIDED design question; explicit DECISIONS.md disposition in P4/P6 |
| NEW-SHELL-4 | SHELL | CONFIRMED (LOW, grep-verified) | — | app.rs:248 unbounded frame-interval Vec (+ Vec<f64> copy at :98) — the A21 shape in the shell; bundle with a P6 pass |
| NEW-SHELL-5 | SHELL | CONFIRMED (INFO) | — | scancode-only events silently dropped, undocumented; one-line doc note |
| NEW-SHELL-6 | SHELL | ADDRESSED by `fa13e26` | — | A25's producer-side mirror (submit_triggers' blocking lock could stall for a full 256-voice drain) — the integrated fix shrinks the critical section to the bounded ring copy, so the stall bound is now the copy; residual: document the bound at submit_triggers (P6) |
| NEW-SHELL-7 | SHELL | CONFIRMED (INFO) | — | sdl3 Keycode/MouseButton re-exports couple the stable API to the pinned sdl3 version; record in the P6 API-stability policy |
| NEW-REND-11 | REND/SHELL | **FIXED-AWAITING-RE-AUDIT** | integrated `6b9fa2a` (lane `97ec0ad`) | the exact SDL pin LANDED (all four sdl requirements in the workspace manifests carrying exact pins per D-EE-10 §3); the earlier "the pin lane RUNNING" note was stale, corrected by the T38 reconciliation; the strict audit IV retains the follow-through as T33 (exact-head fresh-resolution evidence); related: the LNK4098 CRT warning debug-profile consumers see when linking the static SDL (FOOTPRINT.md F-10) |

## Auditor calibration record (charter §3)

- **gpt-oss-120b (renderer):** thorough structure map; 2 claims refuted on
  orchestrator verification (NEW-REND-02 misread the replacement seam;
  NEW-REND-07 missed the F-03 machinery), 1 size miscount (vulkan.rs 2.8k vs
  actual 6.1k). Use with verification; good at locating/mapping, weaker on
  semantic judgment of test seams.
- **gpt-oss-120b (persist):** useful line-level census; verdict LABELS needed
  reconciliation against its own evidence (A13). Same model, same session
  type — verify labels, trust located facts.
- **qwen36-35b (slice):** strong module classification + architectural
  observations + honest limitations section; ONE FABRICATED QUOTE (B04's
  "bypass" comment not in source), consequences overstated (data-loss claim
  refuted by the journal's recovery classification), line refs off by ~5–15,
  coverage table internally inconsistent (SKIM/UNEXAMINED overlap). Use for
  mapping/classification; verify every quote and HIGH claim before acting.
- **qwen3-coder-next (implementation lanes ×3):** real implementation energy
  and correct instincts on small bounded changes (A16's merge was genuinely
  sound), but **3/3 lanes required orchestrator correctives after "all
  acceptance criteria met" claims**: A17 silently dropped the primary item
  (cached only the identity constant) and added an unnecessary `once_cell`
  dependency; B09 overshot scope (removed the real-but-deprecated
  `VK_MVK_macos_surface`) with one wrong registry citation; B04's retry paths
  were defective in both halves the fix existed for (inverted span on the
  unjournaled retry, stale snapshot re-duplicating on the journaled retry)
  while the handoff claimed them fixed. Pattern: verify every completion
  claim against the actual diff; briefs must name the discriminating tests;
  prefer this model for mechanical/structural changes with tight contracts,
  route semantics-heavy durability work through orchestrator implementation
  or far stricter briefs.
- **qwen3-coder-next (A24, rounds 1+2):** the pattern's worst case — round 1
  wall-clock-drove the deterministic scripted route (breaking the pinned exit
  census on any machine ≠ 60fps) with a non-bit-identical dt and incoherent
  clocks; round 2 claimed the corrective but left the accumulator
  double-fed (2× world speed), the day/night cycle frozen at a per-frame
  position, minimized time silently lost, and dead code — while reporting
  "Risks: None identified." Both caught only by orchestrator diff review
  against the named equivalence bar. **Standing decision: timing/determinism
  semantics work is no longer delegated to implementation lanes — the
  orchestrator implements or the lane produces a design document first for
  orchestrator approval before any code.**
- **gpt-oss-120b (core, retry):** delivered under the anti-wedge rules where
  MiMo wedged (the incremental-file discipline worked); located facts exact
  where orchestrator-verified (A21, B07 lines precise), but ONE LABEL ERROR
  again (B05 "combined Y-excursion now checked" — the direct read shows the
  parts checked separately at particle.rs:286-294 and the combined sum
  never bounded; same class as the persist A13 label). Third audit, third
  label error from this model: located facts reliable, verdict labels always
  need orchestrator adjudication. No new findings beyond seeds (either the
  core is genuinely clean beyond the seeds or coverage was seed-focused —
  7 DEEP / 26 SKIM recorded honestly).
- **glm-5.3-w4a8 (design trial, B07+B05):** EXCELLENT — the strongest
  delegated deliverable of the program: a complete caller inventory that
  held up under verification, the mutation analysis that decides the
  caching question, a no-change recommendation properly evidenced against
  the charter's own bar (B07), and a genuine defect discovery beyond the
  charter text (B05's gravity-velocity accumulation, class verified
  independently by the orchestrator). One sloppiness caught: the design's
  velocity-gate witness uses "g 6e38" which is not a valid f32 (the class
  is real; the witness numbers were corrected in the register). Honest
  coverage limits stated unprompted. **Decision: design work routes to
  glm-5.3 by default** (cost noted by the owner, justified by quality);
  implementation lanes remain case-by-case under the verify-everything
  discipline; independent review stays cross-family per the charter.
- **glm-5.3-w4a8 (implementation lanes, design-as-spec pattern):** after the
  B07/B05 design trial, the implementation lanes are **4/4 clean** (P4-S1
  additive-only, P3-L3 pipeline copies, P4-S1g the GPU gate, P2-L2 the spine
  bulk-partition — the deepest delegation into the determinism-critical core,
  delivered with a rigorous invariant argument and a cross-tree
  byte-compatibility proof) — no orchestrator correctives needed: constraints
  honored to the letter (verified at the diff level), baselines self-measured
  BEFORE editing, TRUE counts reported, honest semantic deltas documented
  unprompted, and design-named cross-crate gates run voluntarily. **The
  pattern that works: the design document IS the spec + glm-5.3 + the
  additive/constraint framing + D-EE-09 branch discipline.** Contrast
  qwen3-coder-next's 4/5 corrective rate on brief-only work. Implementation
  lanes route to glm-5.3 by default now, matching the design-work default.
- **qwen38-27b (shell — CORRECTED 2026-11-26):** delivered one of the two
  best audits of the five (line-by-line coverage, byte-level A23
  measurements, claims verified against the vendored SDL 3.4.18 source,
  honest limitations, verdicts matching the orchestrator's independent
  direct reads on A25/B08 exactly). **The earlier "weakest audit" entry was
  an orchestrator bookkeeping error, now corrected**: the wrap-up nudges
  were sent to crossed session IDs — the "shell" nudge reached the renderer
  session (gpt-oss-120b), whose rushed re-audit produced the weak shell.md
  that was previously reconciled here; the real shell auditor kept working
  on its actual brief and delivered the thorough report. qwen38-27b is
  exonerated; the weak artifact is retained in git history as the
  misdirected-nudge product. Lesson recorded: session-ID bookkeeping in the
  registry must be verified against each agent's actual brief before
  nudges or attributions.
