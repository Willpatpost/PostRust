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
| A03 | REND | **FIXED-AWAITING-RE-AUDIT (all six stages)** | S1 `954f843`, S2 `f8f308c`, S3 `70b876d`, S4 `46f29e3`, S5 `585187d`, S6 `7e4c8f8` | the staged migration COMPLETE: the submission contract (S1, rig-verified byte-equality), the scene extraction (S2), the FramePlan coordinator (S3), the vulkan.rs split (S4, pure moves multiset-proven), the shader composition (S5, 19/19 SPIR-V byte-identical), and S6 the adapter/shim/preference deletions + the Garage second scene through the public API only (closes R06); the rig-found null-pipeline defect fixed in the S6 lane and GPU-proven across every route |
| A04 | REND | CONFIRMED (S3 partial: 10→4 on the ensure ladder) | — | render_frame 19 params (incl. wet; charter said ~18) — vulkan.rs:3974-3994; the full 19→6 collapse deferred with S3's record; queued |
| A05 | CORE | IN-PROGRESS | lane `ee/p2-l8-world-config` RUNNING (base `88628bd`) | lib.rs:97-111 constants hard-coded and baked into the save format (npc_world_save fixed-population arrays); the P2-L8 lane: the WorldConfig boundary + `npc-world-save/2` versioning + v1 retained + the identity integration |

| A15 | SLICE | IN-PROGRESS (the second scene landed) | garage `7e4c8f8` | wire discipline is correct (subsystems import only public APIs); the monolithic `corner_demo` dispatch + `main.rs` CLI cascade — the garage route (`--scene garage`) landed as the second authored scene through the public API with its own demo module; the full dispatch extraction per the module classification map remains P6 |
| A06 | REND/SLICE | CONFIRMED | — | animation core is pure but lives INSIDE the renderer crate — placement is the P4 question (auditor read the purity as fine; the crate-coupling issue stands); slice half verified: `corner_routes.rs` imports `engine_renderer::animation` directly (audits/slice.md) |
| A07 | CORE | CONFIRMED | — | npc_perception.rs:83-94 linear population walk; npc_execution.rs:283-306 enumerate_witnesses rides it (audits/core.md) |
| A08 | CORE | **FIXED-AWAITING-RE-AUDIT (predicate-order half)** | `77568dc` | the IA wave closed the divergence: canonical (due,seq) visitation restored on the public API (the in-place sort before the partition, no scratch, save/restore untouched); both live-and-restored witnesses red-then-green; pure-predicate byte-identity re-proven across three trees; the honest timing cost recorded (the no-match 32x win traded to 2.4x per the audit disposition own trade) |
| A09 | CORE | **FIXED-AWAITING-RE-AUDIT** | integrated `88628bd` (lane `c88eb7d`) | the derived witness→keys index LANDED: witness_index (BTreeMap<u32, BTreeSet<MemoryKey>>, phase-blind — confirm moves records between phases without touching the key set) + the measured-in classification_index, both maintained at the single key-set mutation site; the phase merge O(log L + own records) with zero sorting; the equivalence battery byte-identical against separate scan oracles (5 shapes × 2 seeds × both construction paths, mutation-verified); the recovery replay rebuilding identical indexes + digests (wire-invisible); scaling ~97× at 100k records (witness_records 2,563→26.5 µs), classifications ~76×, activate flat; candidate_records stays scan+sort by design (§5.3 boundary, trigger recorded) |
| A10 | CORE | UNTRIAGED | — | pending auditor-core |
| A11 | REND | CONFIRMED | — | MAX_INSTANCES=4096 = MAX_POSE_BLOCKS; region capacity per FRAMES_IN_FLIGHT (scene.rs:319) |
| A12 | REND | **FIXED-AWAITING-RE-AUDIT** | integrated `585187d` (S5 lane `ef75b11`) | the shader build-time composition LANDED: the 9 variant-pair monoliths decomposed into 21 shared cores (182 lines, the 66-line lighting equation that existed ×3 among them) + 9 shells composed at build time by the byte-level splice at anchor lines; pinned naga untouched; **all 19 .spv artifacts SHA-256-identical to the pre-composition baseline** (byte-identity the migration proof); the layout-pin mechanism prototyped (build.rs resolving host-mirrored struct offsets through the same IR the spv writer consumes, cross-variant agreement + the 112-byte prefix invariant asserted, host pins via offset_of); all 5 rig batteries green through the composed shaders at `8864a88`; renderer 191 (188+3) |
| A13 | PERSIST | CONFIRMED | — | **auditor labeled "Refuted" but its own evidence confirms the mixing** (validate_observed local.rs:714, validate local.rs:1656 interleaved with append/commit in the same modules; no dedicated pure-validation layer) — label corrected by orchestrator |
| A16 | PERSIST | FIXED-AWAITING-REVIEW | integrated `f9901f3` | lane `118e193` reviewed: ordered merge verified sound (decode enforces ascending-unique at pipeline.rs:208-212; error semantics traced identical in all divergence cases); gates green, zero test changes → no pin movement; awaits batch gate run |
| A17 | REND/SLICE | FIXED-AWAITING-REVIEW | integrated `3439633` | lane `3409117` + orchestrator corrective `56205f1`: rest_pose_block cached (OnceLock, once_cell dep removed on review) + SkinBind/skin_matrices_bound (the primary item the lane dropped — bind once per batch at both production sites); pins named green; renderer 172 / slice lib 118 baseline-exact; batch gates green |
| A18 | REND | CONFIRMED | — | repack_populations proxy_instances.rs:699-706 + repack_poses animation.rs:729 per frame; folds NEW-REND-08 (static-population caching = P5 dirty-generation question) |
| A19 | REND | CONFIRMED | — | ground_reflect.wgsl:144-166 per-fragment loop over all rings; folds NEW-REND-09 |
| A20 | REND | CONFIRMED | — | per-static-upload staging alloc/copy/submit/fence (vulkan.rs:3035-3199, textures.rs:61-202); folds NEW-REND-03 (no staging pool — same finding, more detail) |
| A21 | CORE | FIXED-AWAITING-REVIEW | integrated `a213563` | orchestrator-implemented `3084a8a`: bounded retention (default 4096, amortized half-drain, lifetime `dropped_total`, NonZeroUsize capacity, opt-in echo documented); caller inventory showed all production paths run `Level::Off` — latent consumer defect closed, zero current behavior change; `&[Record]` API unchanged, `spine_main_path_allocations_are_pinned` green; +3 tests → main pin pending measured shard |
| A22 | CORE/PERSIST | **FIXED-AWAITING-RE-AUDIT (both halves)** | core integrated `4dd78e3` (lane `032bbf6`), persist integrated `42ac117` (lane `94d97a9`) | register correction stands (no `confirmed_history` at HEAD); BOTH halves LANDED — persist: the settled-transaction watermark (monotone prefix mark + out-of-order bitmap, advance = the actionable scan's own settlement proof; per-commit envelope-scan delta ≤2 vs the pre-watermark quadratic 7,260 at 120 transactions) + the bounded coverage shedding (1,024 retained FIFO, authority never dropped, empty-coverage export refused) + the reported-never-auto-triggered cadence; core: the RetentionPolicy{retain,chunk} opt-in cadence wrapping the existing compact (no second mechanism; 3×300 policy firings ≡ one explicit 900 cut byte-identical, pinned), firing at the single end-of-run_tick checkpoint, hysteresis gate, the sustained-load bound demonstrated (50,000 executions, ledger ≤428 at every boundary); the two [ASSUMPTION] thresholds (1,024/4,096; retain/chunk defaults) await the owner-rig data point |
| A23 | REND/SHELL | CONFIRMED (rend half) / MEASURED (shell half) | — | pose storage dynamic-offset per slot (skinned_draw.rs:949-950, POSE_STRIDE=704); shell half MEASURED by the thorough audit: device layer ≈434 KiB (voice pool 64×32B, ring 256×72B, scratch 24 KiB, resident PCM 399,360 B) + cue surface ≈326 KiB, all construction-time with residency pinned — no blanket replacement warranted; the open question is the two coexisting surfaces (NEW-SHELL-3, design review) |
| A24 | SLICE | VERIFIED-CLOSED | integrated `1884582` | **rig-verified 2026-11-26:** all 11 `render_weather` Vulkan gates green on the RTX 5070 Ti (exit 0, 7.29 s) with the pinned values reproducing digit-for-digit (ink 17041, broadside 3482/2162, the allocation tuple, sweep 22/255, the term-separation counts) — the scripted route's census equivalence proven on the real GPU; heavy 664/46/64 measured (pin corrected); scripted route frame-indexed by design, free-look on one coherent accumulator (crowd+rain+day/night, monotone cycle, minimized accrual, dt bit-identity pinned `0x3c888889`); two defective lane rounds caught at review, final surgery by orchestrator |
| A25 | SHELL | FIXED-AWAITING-REVIEW | integrated `7c30aee` | **auditor PASS verdict REFUTED by direct read** (start_voice ran inside the lock at audio.rs:912 — the auditor's "releases before heavy work" was backwards); orchestrator-implemented fix `fa13e26`: copy-out under try-lock → guard dropped → voice-start unheld; pairing preserved; shell battery green, zero test changes |
| B01 | CORE | CONFIRMED | — | spine.rs:1767-1786 drain_due counts executions only (blocked moves don't count); charter's structural-gap framing stands (audits/core.md) |
| B02 | PERSIST | **FIXED-AWAITING-RE-AUDIT** | L1 `19fe59d`, L4 `f437bd7`, L5 `79b2fde` | all three halves LANDED: the bounded header-prefix reads (L1), the recovery single-decode (L4 — the open-then-derive shape removing the two-unguarded-reads coincidence, legacy 2→1 full decodes, bound 3→2 + a ~100-byte prefix read), and the streaming framed reader (L5 — FrameReader replacing all six whole-read sites, working-set O(decoded)+one frame against the old O(file) exact-reserved vec up to 4 GiB, the differential oracle 3,480 comparisons with three seeded regressions caught, teeth proven) |
| B03 | PERSIST | **FIXED-AWAITING-RE-AUDIT (all three halves)** | reclaim `c084230`, pipeline `8636d89`/`54cc9e9`, composite Arc `23609bd` (lane `9f41f9e`) | reclaim dedupe LANDED (with the inversion corrective + teeth-proven test); pipeline copies LANDED (the commit path 3 key-set copies → 1); the composite Arc backing LANDED (the design's Option 1: `MemoEntry.payload`/`VerifiedChunk.payload` → `Arc<[u8]>`, payload 1× + refcounts, memo hits cloning the refcount only; the sharing proven by Arc::ptr_eq, the trust boundary restated — the memo stays per-operation, the medium-mutation test failing closed on the fresh memo; no wire change; the module stays dark so the K2-C activation never mixes a representation change into the authorization checkpoint) |
| B04 | SLICE/PERSIST | **FIXED-AWAITING-RE-AUDIT** | `a8e8d04` | the IA wave closed both boundary holes: the three-phase RetryPhase state machine (Unjournaled / Indeterminate(retained attempt) / Journaled) with exact-attempt reconciliation before any fresh staging (byte-identical envelope+plan, fail-closed on divergence), the atomic marker restoration, and the IA-05 live-handle completion on AttemptUnresolved; all three fault-sequence tests red-then-green; fuzz campaign byte-identical; heavy 667/47/64 measured |
| B10 | SLICE | CONFIRMED | — | replay/synthesis asymmetry verified in structure (audits/slice.md; line refs approximate) — measurement-quality fix alongside the measurement owner |
| B11 | SLICE | CONFIRMED (LOW) | — | occupancy-vs-visibility terminology (audits/slice.md) |
| B05 | CORE | **FIXED-AWAITING-RE-AUDIT** | `1815a2d` | the IA wave closed both escapes: the checked update (nonfinite integrated state = deterministic death, the census identity mechanical, the impossibility argument documented) and the already-below arm totality (per-axis post-else-pre); both audit witnesses red-then-green; the trajectory digest pin green; main 1385 measured at the fix |
| B06 | CORE | **FIXED-AWAITING-RE-AUDIT (both halves)** | tie `356ef92`, combat `d489b7f` (lane `60d170c`) | the tie conformance fix (orchestrator `9a064c8`) + the combat broad phase LANDED: the CombatIndex (persistent uniform hash grid over a roster's hitboxes, the StaticGrid pattern — lookup-only, candidates re-sorted to slice position, the shared narrow phase verbatim through consider_combatant; the public signature/allocation contract unchanged, the frozen fingerprint pin passing); gate 128 with every existing scenario linear (real counts two orders below); the equivalence battery bit-compares by f32 pattern (7 rows × 3 seeds × 2 wall worlds × 48 queries + the permutation arm); scaling ~600× at 32k for the typical 30 m shot, the honest diagonal waste row documenting the DDA trigger; select_target stays linear with measured evidence (154 ns @4 targets, input-edge frequency); the engine-slice adoption seam recorded (FireContext carrying Option<&CombatIndex> — the integrator's wiring) |
| B07 | CORE | RESOLVED — NO-CHANGE (with evidence; census CORRECTED 2026-11-27 per the audit's IA-07) | — | the no-change decision HOLDS for the shipped workload (the corner constructs ONE rule; validation is an allocation-free optional-index check; the wake selector short-circuits on the due commitment; a public mutable vector can change contents without changing its address — per-call validation IS the safe boundary for raw `&[Rule]`). **The census correction:** the earlier "every repository caller passes at most one rule" was FALSE — two-rule tables reach both evaluators in npc_execution's test fixtures and the shared campaign builds three rules; the one-rule bound holds for the shipped corner and the named measurement fixtures only. The design's "measured" cost claim is corrected to an operation count (no benchmark ran). Revisit triggers unchanged |
| B08 | SHELL | CONFIRMED (deferred to measurement) | — | **auditor PASS verdict REFUTED by direct read** (mem::take at input.rs:199-203 moves the buffers out, leaving capacity-0 accumulators that reallocate on the next input-bearing frame — "reused via take" was backwards); disposition: the churn is human-scale at current workloads (edges are rare, idle frames allocate nothing) — the caller-owned-snapshot API change needs an input-event allocation census to justify; queued for the P0 measurement matrix |
| B09 | REND | FIXED-AWAITING-REVIEW | integrated `9dce061` + corrective | lane `2ddef05` reviewed: metal literal fixed + never-existed display_surface removed + one focused test; **orchestrator corrective restored `VK_MVK_macos_surface`** (deprecated but REAL — legacy MoltenVK loaders advertise it; its removal re-opened the exact misclassification class the fix closes); lane citation inaccuracy recorded (VK_KHR_display does provide vkCreateDisplayPlaneSurfaceKHR); +1 test → pin correction at batch gate close; awaits batch gate run |
| B12 | CORE | IN-PROGRESS | lane `ee/p2-l8-world-config` RUNNING (with A05) | the codec exists (npc_behavior.rs); the identity integration is the L8 lane's third deliverable (NpcIdentity round-tripped through the authoritative storage) |
| B13 | SLICE/PERSIST | IN-PROGRESS | lane `ee/p3-l7-b13-worker-instrumentation` RUNNING (base `a13b85a`) | verified: `worker.rs` send-then-`slot.recv()` round-trip at :635,687,745,792,837,877,921,1174; charter-known blocking bounded send on `submit_required`; the L7 lane: the queue-wait/service counters in the never-serialized class + the zero-perturbation proof + the observational data table drafting the deferred-confirmation answer the release rig finalizes |
| B14 | REND | CONFIRMED | — | full-size RGBA16F reflection targets per slot + HDR/bloom targets; P5 measurement package |
| B15 | WORKSPACE | PARTIAL (rend half) | — | renderer dep closure tight (ash/bytemuck/nalgebra + engine-core); full workspace measurement pending P0/P6 |
| B16 | ALL | UNTRIAGED | — | pending all auditors |

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
| R01 | major | SceneDescription::validate misses inverted draw ranges | **CONFIRMED by direct read**: [5,3) passes contiguity (end==next.start holds both) and underflows end−start downstream |
| R02 | major | Scene bounds validation passes infinite endpoints | **CONFIRMED**: partial_cmp returns Some for infinities — [-inf,0] passes min<=max |
| R03 | major | the accumulator iterates all ticks before the cap | **CONFIRMED**: O(elapsed/dt) iterations before MAX_TICKS_PER_FRAME applies — exact bounded arithmetic required |
| R04 | major | deferred presentation skips crowd ticks the accumulator consumed | **CONFIRMED**: advance() consumes; weather ticks pre-submission; crowd steps post-presentation — the coherence gap |
| R05 | minor | app.rs unbounded frame-interval Vec (NEW-SHELL-04 elevated) | ACCEPTED (registered P6; elevated priority) |
| R06 | major | The scene interface's fixed material/texture/reflection prototype assumptions left as transitional-permanent | **CLOSED-BY-S6** (integrated `7e4c8f8`, lane `2e69793`) — all six migration stages landed: S1 the submission contract, S2 the scene extraction, S3 the FramePlan coordinator, S4 the vulkan split, S5 the shader composition, S6 the deletions + the second scene; the four render_corner* entries, the sticky setters, the corner_content shim, and the Corner preference arm deleted (removal conditions grep-proven); the Garage (a genuinely non-corner scene) rendered entirely through attach_scene + submit_frame with its own materials/bounds/wet roles and the texel-for-texel shadow known-answer; the rig-found null-pipeline defect fixed and GPU-proven; the pass_family_material_count generalization the documented deferred boundary (fail-visible at attach) |
| R07 | minor | The memory transaction lifecycle remains structurally complicated and copy-heavy | ACCEPTED (investigate safe consolidation) |

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
