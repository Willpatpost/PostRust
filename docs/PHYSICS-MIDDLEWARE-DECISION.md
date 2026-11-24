# PHYSICS MIDDLEWARE DECISION — Jolt vs Rapier (T-06's standing deferral, resolved into a recommendation)

> **Status:** Research + recommendation v1.0 · 2026-11-24 · **UNIMPLEMENTED
> DECISION INPUT — nothing here is ratified.** This is the recursive queue's
> second item: the physics decision research doc the mission names ("Jolt vs
> Rapier, ending in a recommendation for the owner — T-06's standing
> deferral"). It ends in a recommendation and an owner-decision list; the
> registry (`DECISIONS.md` T-06) remains the authority.
>
> **Ownership:** this file compares the two shortlisted production physics
> backends against THIS engine's ratified constraints and recommends. It
> **consumes** (never redefines): [`DECISIONS.md`](DECISIONS.md) (T-01 Rust +
> Cargo; T-06 the deferral itself; V-02 RCT-class efficiency; V-05 strategic
> reuse; E-03 determinism as working hypothesis; G-02 systemic combat; G-03
> traversal incl. eventual unrestricted flight), the **built engine** (the
> player prototype's `CollisionWorld` contract — `engine-core/src/player.rs`,
> whose module docs state the contract is "deliberately narrow so an
> established physics library can slot behind the same contract in a later
> ratified cycle (Rapier was the evaluated candidate)"; the spine's
> single-writer host-authoritative shape M-28), and
> [`CUSTOM-ENGINE-RESEARCH.md`](CUSTOM-ENGINE-RESEARCH.md) §3's research
> principles (compare at equivalent workload; treat advertised features as
> claims, not evidence; the evidence key below).
>
> **Evidence key:** **[DOCUMENTED]** = the engine's own documentation,
> fetched and read this pass (dated 2026-11-24, URLs in §7); **[PROJECT
> EVIDENCE]** = demonstrated in this repository; **[MEASURED-PENDING]** = the
> A/B this doc requires before final ratification; **[OPEN]** = the owner's
> decision.

---

## 0. The question, precisely

T-06 (2026-10-19) deferred "production physics middleware (Jolt/Rapier
shortlist stands)". The deferral was correct then — no workload demanded
physics beyond the player prototype's kinematic mechanics. Era 2 changes the
pressure: the living world's next packages (the NPC AI design pass's
behavior layer; combat-adjacent encounters; the sedan/hovercar traffic)
approach the boundary where "kinematic mechanics + hand-written queries"
stops being the honest tool. This doc answers: **which backend, behind
which contract, gated by which measurements — and what the engine owes
determinism either way.**

The comparison is NOT "which engine is better" (both are excellent and
lawful). It is "which fits THIS engine's law": Rust+Cargo (T-01), byte-exact
cross-platform determinism as the house discipline (E-03, made binding for
fingerprinted state by every gate the repo has ever merged), RCT-class
efficiency with p99 as the metric (V-02), zero framework gravity, safe
boundaries (unsafe confined to the vulkan module's invariants manifest
today), and scalability to 1000×.

## 1. What physics game one actually needs (the requirement shape)

From the ratified requirements, the honest workload list — each item mapped
to whether it is DETERMINISTIC-LANE (fingerprinted, replayed, replicated) or
PRESENTATION-LANE (visual only, never fingerprinted):

| Need | Source | Lane | Notes |
|---|---|---|---|
| Player movement + collision | G-03, built (Stage 3/`--player`) | Deterministic | Already served by the kinematic prototype behind `CollisionWorld` — the contract a backend must slot behind |
| NPC locomotion | Era-2 NPC AI design pass §4.3 | Deterministic | Kinematic, route-following — queries only, no solver |
| Combat hit detection, knockback, debris | G-02 | Deterministic (outcomes) | Solver results that change authoritative state |
| Ragdolls (NPC death, G-08) | DESIGN-BIBLE | Presentation (usually) | The moment of death is deterministic; the settling ragdoll is cosmetic |
| Vehicles (sedans now, hovercars per VISION) | VISION §2, the corner's 2 sedans | Mixed | Hovering/floating is force-driven or kinematic-rail; collisions with the world are query-shaped |
| Eventual unrestricted flight | G-03 | Deterministic | Movement, not solver-heavy |
| Cloth/soft props, GPU hair | visual ambition | Presentation | Never enters the spine |

The load-bearing observation: **the deterministic lane's physics is
QUERY-shaped and OUTCOME-shaped** — collision queries, sweep/depenetration,
and discrete event outcomes (hit landed, object came to rest at X). The
SOLVER-heavy, high-body-count, continuous simulation (ragdoll piles, debris
sprays, cloth) is overwhelmingly presentation. This asymmetry is the
decision's spine: the two lanes have different determinism requirements and
different performance ceilings — and they may not even want the same
backend.

## 2. The determinism requirement (the hard constraint)

The engine's law (E-03 + the repo's gate culture): fingerprinted state is
byte-exact, cross-platform, forever. Anything inside the spine's
authoritative state must reproduce bit-identically on the host and every
guest/CI platform (Windows x64, Linux x64, macOS ARM64 — the existing
three-platform CI).

Physics is the classic determinism hazard: floating-point variance across
platforms (libm differences, FMA contraction, SIMD width) makes naive
float physics diverge. Both shortlist engines now document a cross-platform
answer — this is the single most important finding of this pass:

### Jolt's contract [DOCUMENTED — Jolt docs, "Deterministic Simulation"]

- Same-binary determinism is a design property ("The simulation runs
  deterministically. You can replicate a simulation to a remote client by
  merely replicating the inputs"), including under multithreading.
- **`CROSS_PLATFORM_DETERMINISTIC`** (a CMake build option): "approximately
  8% slower" but deterministic regardless of compiler (MSVC2022, clang,
  gcc, emscripten), configuration (Debug/Release/Distribution), OS
  (Windows/macOS/Linux), architecture (x86, ARM, RISC-V, PowerPC,
  LoongArch), and word size — **verified in Jolt's CI across multiple
  architecture/compiler combinations per build**.
- Documented limits: the SAME source and the same defines on every
  platform; **broadphase queries are not deterministic** (multithreaded
  bounding-box widening — a documented workaround exists: re-query actual
  bounds in a custom collector and order results); narrowphase query and
  callback ORDER is not deterministic (must sort before consuming);
  `GetActiveBodies` order is not deterministic.

### Rapier's contract [DOCUMENTED — Rapier user guide, "Determinism"]

- Default is **locally deterministic** only: same machine, same Rapier
  version, same Rust compiler version ⇒ identical results; cross-machine
  "may result in completely different results".
- **`enhanced-determinism`** (a cargo feature): cross-platform deterministic
  across "all 32-bit and 64-bit platforms that implement the IEEE
  754-2008 standard strictly", including WASM. Conditions: every
  transcendental used to build simulation inputs must come from nalgebra's
  `ComplexField`/`RealField` traits (software implementations — **the exact
  discipline this engine calls libm-free**); `simd8` is a separate
  determinism domain (mutually exclusive); **`parallel` is compatible**
  ("the results of the parallel solver are identical to the results of the
  sequential one, and don't depend on the number of threads").

### What the contracts mean for PostRust

Both engines can serve the deterministic lane — with their mode ON, their
documented disciplines followed (call-order identity — the spine's existing
law; sorted consumption of unordered query/callback output — the
fingerprinter's existing law), and a **verification gate the engine itself
owns** (§5). Neither engine's claim is accepted as evidence: the repo's
standard is a double-run byte-exact digest executed on all three CI
platforms, on OUR scenes, pinned. [MEASURED-PENDING for both]

## 3. The comparison

| Axis | Jolt Physics | Rapier |
|---|---|---|
| **Language / integration** | C++17, no RTTI/exceptions, STL-only; via CMake; Rust through third-party bindings (jolt-rust / JoltC) — an FFI seam with unsafe and build integration | Pure Rust, a Cargo dependency (`rapier3d`); no FFI, safe boundary, workspace-native |
| **License** | MIT | Apache-2.0 (both lawful for commercial Steam release and standalone-engine reuse) |
| **Provenance** | Horizon Forbidden West, Death Stranding 2; GDC 2022 architecture talk; multicore-scaling published comparisons | Dimforge (an open-source company); broad Rust/Bevy ecosystem adoption; robotics use |
| **Cross-platform determinism** | `CROSS_PLATFORM_DETERMINISTIC` mode, ~8% cost, CI-verified across compilers/OSes/architectures [DOCUMENTED] | `enhanced-determinism` feature, software transcendentals, IEEE 754-2008 strict targets [DOCUMENTED] |
| **Multithreading** | Designed for concurrent queries + background body streaming; deterministic under threads | `parallel` (rayon); parallel ≡ sequential results; heterogeneous-core caveat documented (dedicated thread pool) |
| **SIMD** | SSE2 min; SSE4.1/4.2/AVX/AVX2/AVX512 compile options | Always-on 4-lane solver (the `wide` crate); `simd8` optional (a separate determinism domain) |
| **Feature breadth** | CCD, ragdolls, character (rigid + virtual), **vehicles (wheeled/tracked/motorcycle)**, soft bodies, GPU hair, buoyancy, sensors, **large-world double precision** | Rigid bodies, joints, soft bodies (incl. a FEM option), character controller, sensors, events, queries, **snapshotting/serialization** |
| **Versioning posture** | Shipped-AAA release discipline; release notes + API-change documents | **Pre-1.0** ("strongly recommended to always use its latest published version, though you may encounter breaking changes") — pin + deliberate upgrades |
| **f64 option** | Build-time double-precision mode (large worlds) | Separate crates (`rapier3d-f64`) |
| **Maintenance model** | Single primary author + contributor agreement; CLA | Company-backed; discloses AI-assisted implementation for some crates with human review |

(All table cells above are [DOCUMENTED] from the sources in §7, fetched
2026-11-24 — none is a benchmark claim about OUR workloads; performance
comparisons between the engines exist in Jolt's published multicore study
but the research principles forbid accepting them as evidence for this
engine's scenes: **[MEASURED-PENDING]** — §5's A/B.)

## 4. The architecture fit (where the backend meets the spine)

The engine's shape constrains the integration regardless of backend:

1. **The deterministic lane steps INSIDE the spine's tick** (single-writer,
   host-authoritative M-28): the physics step is budgeted work in the near
   lane, its authoritative outcomes (contacts that change state, rest
   positions that persist) are spine events/deltas, and its state hashes
   under a new versioned FNV domain composed into the campaign fingerprint.
   Cross-platform byte-exactness is REQUIRED here — with the backend's
   determinism mode ON and the engine's own gate proving it (§5).
2. **The presentation lane steps OUTSIDE the fingerprint** (ragdolls,
   debris, cloth — cosmetic): determinism optional, performance primary.
   This lane may even choose a different tool later without touching the
   spine (the same engine-primitive/policy split the NPC AI design pass
   states).
3. **The slot-in contract already exists** [PROJECT EVIDENCE]: the player
   prototype's `CollisionWorld` (box overlap, segment cast via the exact
   slab method, axis sweep, depenetration with a deterministic tie rule) —
   "deliberately narrow so an established physics library can slot behind
   the same contract". Rapier's query surface maps onto it directly (both
   are Rust query APIs); Jolt's maps through the FFI seam.
4. **Unsafe containment**: today project-owned unsafe lives only in the
   vulkan module (behind its invariants manifest) plus documented seams.
   A Jolt integration adds a permanent unsafe FFI boundary (bindings
   quality becomes an audit surface); Rapier adds none.
5. **Build integration**: the workspace already vendors SDL3 through CMake
   (T-09) — a CMake dependency is NOT foreign; but a second vendored
   C++ tree (with per-platform cross-compilation, defines discipline
   (`JPH_DOUBLE_PRECISION` etc. must match everywhere — Jolt's own caveat),
   and binding regeneration on API drift) is a standing cost vs one
   `cargo add` line.

## 5. The verification gate (what must be measured before final ratification)

The repo's discipline applied to this decision — the A/B that outranks
every table cell above:

1. **The determinism gate** (both candidates): a fixed scripted scene
   (falling stacks, a vehicle on a rail, a hit-detection exchange) stepped
   N ticks, state digested per tick — double-run byte-exact LOCALLY, and
   byte-exact ACROSS the three CI platforms (Windows x64 MSVC, Linux x64
   gcc/clang, macOS ARM64). With `enhanced-determinism` /
   `CROSS_PLATFORM_DETERMINISTIC` ON, and again with each engine's
   documented query-order disciplines applied (sorted consumption). A
   divergence is an investigation — exactly the spine's law.
2. **The cost gate**: the same scenes at the corner scale and at 1,000-body
   scale, measuring **p99 step time** (not the average — V-02's tail
   discipline), single-threaded and with each engine's parallel mode, on
   the dev pod AND the owner's rig (rig-dependent claims labeled pending).
3. **The integration gate**: the slot-in behind the existing
   `CollisionWorld` contract — the player prototype's trace fingerprints
   (`engine-core/player-trace/1`) must remain byte-identical when the
   backend swaps in behind the same queries.
4. **The teardown/leak gate**: the renderer's allocation-counter discipline
   extended to the physics state (constant across warmed frames).

## 6. THE RECOMMENDATION

**Rapier (`rapier3d`, with `enhanced-determinism` ON, exact version pinned)
as the leading candidate for the deterministic lane — the first
integration package, behind the existing `CollisionWorld` contract, gated
by §5's A/B before any final ratification.** The reasoning, in the
engine's own priority order:

1. **Determinism fit**: Rapier's cross-platform mode is philosophically
   IDENTICAL to the house discipline — software transcendentals over
   platform libm (the `sin_cos_turn`/libm-free rule), strict-IEEE targets,
   documented feature interactions (`simd8` exclusion, `parallel`
   equivalence). Jolt's mode is equally serious and more broadly
   CI-verified, but it lives behind a C++ build where the engine cannot
   read it — the engine's own gate is the proof either way, and Rapier's
   is the mode the engine can most confidently hold to its law.
2. **Integration gravity** (the lightweight/modular value): a Cargo
   dependency with no FFI, no unsafe, no second vendored C++ tree, no
   binding-drift maintenance. The workspace's entire dependency history
   (ash, sdl3, sha2, bytemuck, font8x8) is Cargo-native; Rapier continues
   it. The player prototype's contract was SHAPED for it (its own module
   docs name Rapier as the evaluated candidate).
3. **The workload fit**: the deterministic lane is query-shaped and
   outcome-shaped (§1) — squarely Rapier's competence; the AAA-scale
   solver-heavy scenarios where Jolt's breadth (vehicles, GPU hair,
   buoyancy, double-precision large worlds) is decisive live mostly in the
   PRESENTATION lane, where a later, separately-ratified choice can adopt
   Jolt WITHOUT touching the spine (the two-lane split is the
   architectural hedge).
4. **The honest costs, stated**: Rapier is pre-1.0 (pin exact versions;
   upgrade deliberately with digest re-pins — the repo already pins naga
   =29.0.4 for cause, the discipline transfers); its `parallel` uses rayon
   (an extra dependency; the heterogeneous-core caveat documented); its
   feature breadth is narrower (no built-in vehicle subsystem — a
   hovercar package would compose constraints, which is ENGINE work either
   way since hovering is game policy).

**Jolt remains the shortlisted alternative** — the performance-ceiling
candidate and the presentation-lane's natural choice if measured need
demands AAA-scale continuous simulation — with its integration cost (the
FFI seam, the C++ build, the defines discipline) paid only when a measured
workload justifies it (the no-speculative-abstraction rule).

**The recommendation to the owner, concretely:** close T-06's "shortlist
stands" clause by ratifying (a) Rapier as the deterministic lane's leading
candidate with `enhanced-determinism` mandatory and the version pinned,
(b) §5's A/B as the merge-blocking gate for the first physics package, and
(c) the two-lane split (deterministic inside the spine, presentation
outside) as the standing architecture — OR direct otherwise. The A/B runs
on request; nothing here authorizes the dependency.

## 7. Sources (all fetched and read 2026-11-24)

- Jolt Physics README + architecture docs:
  `https://github.com/jrouwe/JoltPhysics` (license MIT; 12.4k stars;
  1,838 commits; C++17; platform and CPU-feature lists; feature list;
  Rust bindings listed: SecondHalfGames/jolt-rust) and
  `https://jrouwe.github.io/JoltPhysics/` — the **Deterministic
  Simulation** section (same-binary determinism;
  `CROSS_PLATFORM_DETERMINISTIC` ~8% cost, compiler/config/OS/arch/
  word-size invariance, CI verification across architectures; the
  broadphase/query/callback ordering caveats and their workarounds).
- Rapier README + user guide: `https://github.com/dimforge/rapier`
  (Apache-2.0; 6.1k stars; 1,389 commits; pure Rust; SIMD-always-on;
  `parallel`; the AI-coding disclosure) and
  `https://www.rapier.rs/docs/` + the Rust guide's **Getting started**
  (feature list, `enhanced-determinism`, `simd8` exclusivity,
  heterogeneous-core note) and **Determinism** pages (local vs
  cross-platform contracts, nalgebra `ComplexField`/`RealField` rule,
  parallel ≡ sequential, pre-1.0 versioning advice). Current guide
  versions: Rust 0.36, bevy plugin 0.37.
- The engine's own contracts [PROJECT EVIDENCE]: `engine-core/src/
  player.rs` (the `CollisionWorld` slot-in contract and its Rapier note),
  the spine's fingerprint domains and double-run gates (CURRENT-STATUS
  §4's pin history), T-09's vendored-SDL3 CMake precedent.

*No benchmark numbers from either project's published comparisons are
carried as evidence for this engine — the research principles require
equivalent-workload A/B ([MEASURED-PENDING] §5). Nothing here merges
without the owner's GO; the registry owns the decision.*
