# Workspace API and Build Footprint (B15)

The B15 measurement lane's deliverable: the measured dependency closure per
crate, the workspace binary/library sizes, and the minimal-consumer battery —
every number carries its command. Measured 2026-11-27 on the Windows rig
(AMD Ryzen 7 9800X3D, 8c/16t; rustc/cargo 1.99.1 pinned by
`rust-toolchain.toml`; cmake + MSVC present) in the B15 worktree, branch
`ee/b15-workspace-footprint`, base tree `72c91d3` (the integrated program
HEAD at lane start; no source file was touched by this lane — the diff is
this report plus the register/evidence updates only).

**Boundary statement (what this report is NOT):** runtime startup, resident
memory under load, and workload-scaling numbers belong to the E-8 baseline
battery and the P0/P5 matrix — this report covers the *build-time and
adoption* footprint: what compiles, how big it is, what a consumer must
build to use each crate's public API. No production code, manifest, or lock
file was changed.

## 1. Method

All commands run in the worktree root
(`C:\Users\Willp\AppData\Local\Temp\opencode\ee\a17`) unless a consumer
directory is named. Exit status recorded for every invocation; every command
below exited 0 unless a non-zero exit is explicitly printed.

- **Dependency closure**: `cargo tree -p <crate> --edges <set> --prefix
  none`, one line per crate, deduplication markers (` (*)`) stripped, unique
  lines counted. The `--edges` flag is ALWAYS passed explicitly — the
  default `cargo tree` edge set includes dev AND build edges (measured:
  plain `cargo tree -p engine-renderer --prefix none` yields 57 unique
  crates vs the production-only 53), so a naive "production footprint"
  measurement overstates by the dev/test closure. Counting convention:
  "unique" includes the queried crate itself; "external" = unique − 1.
- **Host caveat**: measured on `windows-msvc`; the manifests'
  `target.'cfg(unix)'` dependencies (the `sdl-unix-console-build` enabler on
  engine-shell and engine-renderer) are invisible in these trees by cargo's
  target filtering. They are documented from the manifests (both are
  sdl3-sys feature-only — no additional crates beyond the sdl3 chain
  already counted).
- **Lock caveat**: workspace trees resolve against the committed
  `Cargo.lock`. The consumer battery (§6) generates FRESH locks — the
  version-drift finding F-01 comes from exactly that difference.
- **Sizes**: `(Get-Item <path>).Length` on the built artifacts, bytes exact,
  MiB = bytes/1,048,576.
- **Timings**: single cold runs on this rig (`Measure-Command` around the
  cargo invocation), dependency download excluded via a prior untimed
  `cargo fetch`. They are rig-specific order-of-magnitude evidence, not
  pinned contract values.
- **Minimal consumers**: throwaway projects OUTSIDE the repo at
  `C:\Users\Willp\AppData\Local\Temp\opencode\ee\b15-minimal-consumer\{core,
  persist,renderer,shell,slice}` — each a standalone crate with a `path`
  dependency on exactly one engine crate, its own `Cargo.lock` and its own
  `target/` (never committed; the full reproduction recipe is §6.2).

## 2. Dependency closure per crate

Command shape (per crate, three edge sets):
`cargo tree -p <crate> --edges normal --prefix none` (and `normal,build`,
`normal,build,dev`) — all exit 0. Raw trees at
`Temp\opencode\ee\b15-artifacts\tree-<crate>-<edges>.txt`.

| Crate | Prod unique (normal) | Prod ext. | +build unique | +build ext. | +dev unique | +dev ext. |
|---|---:|---:|---:|---:|---:|---:|
| engine-core | 1 | **0** | 1 | 0 | 1 | 0 |
| engine-persist | 20 | 19 | 25 | 24 | 25 | 24 |
| engine-renderer | 16 | 15 | 53 | 52 | 57 | 56 |
| engine-shell | 6 | 5 | 14 | 13 | 14 | 13 |
| engine-slice | 36 | 35 | 70 | 69 | 70 | 69 |

Reading of the table:

- **engine-core is exactly zero-dependency** — no normal, build, or dev
  dependencies at any depth (the lightweight anchor the architecture
  claims; confirmed at the closure level, not just the manifest level).
- **engine-persist** has NO dev-dependencies (24 external with build);
  the +5 from build edges is entirely `libsqlite3-sys`'s C toolchain (cc,
  find-msvc-tools, pkg-config, shlex, vcpkg).
- **engine-renderer** carries the heaviest build-side closure: the build
  dependency `naga =29.0.4` (glsl-in/spv-out/wgsl-in — the build-time
  shader composition, S5) contributes ~30 crates; the SDL static native
  chain (sdl3 → sdl3-sys → cmake/cc/sdl3-src/pkg-config/rpkg-config/
  vcpkg/find-msvc-tools/shlex) the rest. Its dev-only delta is exactly 4
  crates (petgraph + fixedbitset + hashbrown 0.15.5 + foldhash 0.1.5 —
  all from the test-side `naga` spv-in re-parse).
- **engine-shell** adds nothing beyond the SDL chain + engine-core; no
  dev-dependencies.
- **engine-slice** (the integration driver) subsumes all four: 69 external
  production crates. **Its dev-dependencies are closure-neutral** — the
  measured dev delta over production is ZERO crates (the dev rusqlite is
  the same crate the production engine-persist closure already contains;
  the `engine-persist/composite-test-fixture` dev feature enables no
  crates). "The production binary never sees it" (the manifest's own
  claim) is confirmed at the resolution level, not just the linkage level.

### 2.1 The per-crate dep lists (production, normal+build, with versions)

**engine-core** — none. (Command: `cargo tree -p engine-core --edges
normal,build --prefix none` → the single line `engine-core v0.1.0`.)

**engine-shell** (14 unique / 13 external): engine-core (path) · sdl3
v0.20.0 · sdl3-sys v0.7.2+SDL-3.4.18 · sdl3-src v3.4.18 (vendored SDL
source, build-side) · bitflags v2.13.2 · libc v0.2.191 · cmake v0.1.58 ·
cc v1.9.0 · find-msvc-tools v0.1.15 · pkg-config v0.3.34 · rpkg-config
v0.1.2 · shlex v2.0.1 · vcpkg v0.2.15.

**engine-persist** (25 unique / 24 external): engine-core (path) · rusqlite
v0.40.2 [`bundled`, `limits`] → bitflags · fallible-iterator v0.3.0 ·
fallible-streaming-iterator v0.1.9 · hashlink v0.12.2 → hashbrown v0.17.1
→ foldhash v0.2.0 · libsqlite3-sys v0.38.2 (build: cc, find-msvc-tools,
pkg-config, shlex, vcpkg) · smallvec v1.16.4; sha2 v0.11.1 → cfg-if
v1.0.5 · cpufeatures v0.3.1 · digest v0.11.3 → block-buffer v0.12.1 ·
hybrid-array v0.4.15 · typenum v1.20.1 · const-oid v0.10.2 · crypto-common
v0.2.2.

**engine-renderer** (53 unique / 52 external): engine-core (path) · ash
v0.38.0+1.3.281 (features `debug`,`loaded`,`std`; `loaded` → libloading
v0.8.9, windows-link v0.2.1) · bytemuck v1.25.2 [`derive`] →
bytemuck_derive → proc-macro2 v1.0.107 · quote v1.0.47 · syn v3.0.9 ·
unicode-ident v1.0.27 · font8x8 v0.3.1 · sdl3 v0.20.0 [`ash`,
`build-from-source-static`] → sdl3-sys v0.7.2+SDL-3.4.18 → sdl3-src
v3.4.18 · cmake · cc · find-msvc-tools · pkg-config · rpkg-config · shlex
· vcpkg · bitflags · libc; BUILD-ONLY: naga v=29.0.4 [`glsl-in`,
`spv-out`, `wgsl-in`] → arrayvec v0.7.8 · bit-set v0.9.1 · bit-vec v0.9.1 ·
codespan-reporting v0.13.1 · equivalent v1.0.3 · half v2.7.1 · hashbrown
v0.16.1 · hexf-parse v0.2.1 · indexmap v2.14.2 · libm v0.2.16 · log
v0.4.34 · num-traits v0.2.19 · once_cell v1.21.4 · pp-rs v0.2.1 ·
rustc-hash v1.1.0 · spirv v0.4.0 · thiserror v2.0.21 (+ thiserror-impl,
syn v2.0.119) · unicode-width v0.2.2 · unicode-xid v0.2.6 · zerocopy
v0.8.63 (+ zerocopy-derive) · cfg_aliases v0.2.2 · autocfg v1.5.1 ·
foldhash v0.2.0 (shared names above appear once). DEV-ONLY (+4): petgraph
v0.8.3 · fixedbitset v0.5.9 · hashbrown v0.15.5 · foldhash v0.1.5.

**engine-slice** (70 unique / 69 external): the union of the four above —
all four engine crates, the SDL chain, the rusqlite/SQLite chain, the sha2
chain, the naga build chain, the ash/bytemuck/font8x8 chain (full flat
list: §Appendix A).

### 2.2 Version duplication inside the closure

Measured in the engine-renderer and engine-slice production trees
(`cargo tree -p <crate> --edges normal,build --prefix none`, exit 0):

| Crate | Versions in one build | Pulled by |
|---|---|---|
| hashbrown | 0.16.1, 0.17.1 (+0.15.5 dev-only) | 0.16.1 ← indexmap ← naga (build); 0.17.1 ← hashlink ← rusqlite; 0.15.5 ← petgraph (dev) |
| syn | 2.0.119, 3.0.9 | 2.x ← thiserror-impl/zerocopy-derive; 3.x ← bytemuck_derive |
| foldhash | 0.2.0, 0.1.5 (dev-only) | the two hashbrowns |

Ecosystem-typical semver-major duplication; no manifest-level fix exists
without upstream upgrades, and the naga `=29.0.4` pin (required: 30.0.1's
glsl-in defect, per the manifest's own comment) freezes its subtree.
Recorded, no action.

**ash is compiled TWICE per profile** (measured in `target\release\deps`:
`libash-0c867908420c339b.rlib` 72,586,152 B and
`libash-3dead265cb78e1f0.rlib` 68,353,816 B). Cause, verified by
`cargo tree -p engine-renderer -i ash -e features` (exit 0): sdl3-sys's
**build script** depends on sdl3 itself for metadata generation, and that
build-graph copy of sdl3 activates ash with a different feature set than
engine-renderer's normal-graph copy (`use-ash-v0-38` + metadata features
vs ash's defaults `debug`/`loaded`/`std`) — cargo compiles one ash for the
normal graph and one for the build-script graph. Inherent to sdl3-sys's
packaging; not fixable from this workspace's manifests. Cost: one extra
ash compile per profile (~68–73 MB rlib each).

## 3. Feature-flag closure

Commands: `cargo tree -p <crate> -e features --prefix none` for persist,
renderer, slice (exit 0; raw at `Temp\opencode\ee\b15-artifacts\features-*.txt`).

- **engine-persist's `composite-test-fixture` and `crash-test-seams` pull
  ZERO crates** — both are empty feature arrays (`= []`) whose only effect
  is `#[cfg(feature)]`-gating code and `[[test]] required-features`. The
  K2-C1/RMD-03 isolation discipline (test facilities compile-time excluded
  from production builds) is verified at the RESOLUTION level: enabling
  them adds no dependency, and the CI-side claim (feature-disabled
  artifact carries zero activation symbols) is consistent with the
  closure measurement.
- **engine-slice's `crash-test-seams` forwards to
  `engine-persist/crash-test-seams`** — likewise zero crates.
- **The features that DO pull weight**: `rusqlite/bundled` →
  `libsqlite3-sys/bundled` + `cc` (the SQLite C compile); sdl3-sys's
  `build-from-source-static` → `sdl3-src` + `cmake` + `cc` + the
  locator chain (pkg-config/rpkg-config/vcpkg); `bytemuck/derive` →
  bytemuck_derive + syn; ash default `loaded` → libloading (runtime
  Vulkan-loader dlopen — the standard choice; the static-link
  alternative is a platform policy, not a footprint fix).
- **rusqlite's DEFAULT features include `ffi-sqlite-wasm-rs`** →
  `dep:sqlite-wasm-rs` — a wasm32-target-only dependency (target-gated in
  rusqlite's own manifest at `cfg(all(target_family = "wasm",
  target_os = "unknown"))`). On Windows it is DOWNLOADED at `cargo
  fetch` (observed: `Downloaded sqlite-wasm-rs v0.5.5`) but never
  compiled — it appears in fresh `Cargo.lock`s (persist consumer lock:
  43 packages vs 26 built) without entering the build. Harmless; worth
  knowing when reading a consumer's lock.

## 4. dev-only vs production, and optional-dep candidates

Dev/build/production classification (measured deltas, §2):

- **Dev-only in the workspace**: exactly the renderer's `naga[spv-in]`
  re-parse tree (petgraph family, 4 crates) and engine-slice's
  dev-declarations (0 new crates). Nothing else — core, persist, and
  shell declare no dev-dependencies at all.
- **Build-only** (compiled for build scripts, never linked into the
  binary): the naga tree (~30 crates, engine-renderer's shader
  compilation), the SDL/SQLite C toolchains (cmake, cc, pkg-config,
  rpkg-config, vcpkg, find-msvc-tools, shlex), and sdl3-src (source
  only).
- **Production deps that could be optional** (owner decisions — this
  lane measures, it does not change):
  1. **rusqlite in engine-persist** — unconditional. A local-medium-only
     consumer still compiles bundled SQLite: measured isolated cost
     3.5 s (§6.3) and 10 of persist's 24 external crates. A `sqlite`
     feature gate is the obvious candidate — but SqliteMedium is the
     documented production fallback (the Windows durability position,
     engine-persist's own lib header) and `migrate_local_to_sqlite` is
     public API; gating changes the crate's packaging contract and is
     the owner's call, registered as the §8 lever.
  2. **font8x8 in engine-renderer** — public-domain bitmap font data for
     the debug overlay only. Gating it behind a feature is possible but
     the overlay is integral to the renderer's diagnostics surface; low
     value (one small pure-data crate), noted for completeness.
  3. **Nothing in engine-shell** (sdl3 is the crate's purpose) and
     **nothing in engine-core** (zero deps).

## 5. Binary and library sizes

Commands: `cargo build -p engine-slice` (exit 0), `cargo build -p
engine-slice --release` (exit 0, cold for this worktree's target —
`target/release` did not exist before; the full log with every
"Compiling" line is at `Temp\opencode\ee\b15-artifacts\build-release-slice.log`).
Sizes via `(Get-Item <path>).Length`. The workspace's ONLY binary target is
`engine-slice.exe` (src/main.rs; no `[[bin]]`/`[[example]]`/`[[bench]]`
anywhere — `Select-String -Path crates/*/Cargo.toml -Pattern '\[\[bin'
→ no matches, exit 0).

| Artifact | Bytes | MiB |
|---|---:|---:|
| engine-slice.exe, debug profile | 16,371,200 | 15.61 |
| engine-slice.exe, release profile (default, unstripped) | 10,509,824 | 10.02 |

Release rlibs (`target\release\deps\libengine_*.rlib`):

| Crate | Bytes | MiB |
|---|---:|---:|
| engine-core | 3,543,938 | 3.38 |
| engine-persist | 10,454,706 | 9.97 |
| engine-renderer | 2,829,696 | 2.70 |
| engine-shell | 508,042 | 0.48 |
| engine-slice | 12,298,198 | 11.73 |

Debug rlibs (same command shape, `target\debug\deps`; two hashes exist for
persist/shell/slice — the second variant comes from the feature-diverged
builds the test shards produce, e.g. `composite-test-fixture`):

| Crate | Bytes (variant range) | MiB |
|---|---|---:|
| engine-core | 24,267,708 | 23.14 |
| engine-persist | 86,206,634 – 86,223,994 | ~82.22 |
| engine-renderer | 23,911,746 | 22.80 |
| engine-shell | 3,673,456 – 3,676,756 | ~3.50 |
| engine-slice | 102,410,836 – 103,147,606 | ~98.04 |

- **Debug-vs-release for the demo binary: 15.61 → 10.02 MiB (−36%)**;
  the delta is debuginfo plus optimization. The statically-linked SDL and
  SQLite are inside both.
- **Stripped sizes: NOT MEASURED — blocked by the pinned toolchain.**
  The `rust-toolchain.toml` profile is `minimal`; `strip`/`rust-strip`/
  `objcopy` are absent from both PATH and the sysroot bin (`where.exe
  strip` → not found; `Get-ChildItem <sysroot>\bin -Filter *strip*` →
  empty). Per the charter's own rule ("evaluate build configuration only
  after checking current pinned toolchain/platform behavior") this is
  recorded as the method boundary, not worked around by installing
  components on the rig. The release PE carries no cargo debuginfo by
  default profile anyway (debug = 0); a stripped-vs-unstripped comparison
  would quantify the MSVC symbol tables — owner may add `llvm-tools` or
  set `strip = true` in a profile as a P6 configuration decision.
- **Cold full-workspace release build (this rig): 48.0 s**
  (`Measure-Command { cargo build -p engine-slice --release }`,
  RELEASE_BUILD_EXIT=0) — every dependency, the SDL CMake build, the
  bundled SQLite compile, and the naga shader compilation included. The
  worktree target after full debug + release builds: 12.47 GB
  (`Get-ChildItem target -Recurse -File | Measure-Object Length -Sum`).

## 6. The minimal-consumer battery

### 6.1 Method (reproduction recipe)

For each engine crate C ∈ {core, persist, renderer, shell, slice}, at
`Temp\opencode\ee\b15-minimal-consumer\<C>\`:

1. Create `Cargo.toml` with exactly one dependency:
   `C = { path = "<worktree>/crates/C" }` (no default features changed,
   no extras), and a `src/main.rs` calling the minimal public surface
   (the exact programs are quoted in §6.2).
2. `cargo fetch` (untimed; downloads + fresh `Cargo.lock`).
3. `Measure-Command { cargo build }` — the COLD compile time (fresh
   target dir, dev profile — what a consumer hits first).
4. `cargo run -q` — the consumer must RUN, not just compile.
5. `cargo tree --edges normal,build,dev --prefix none` — the closure the
   consumer pulls.

### 6.2 The minimal public surfaces exercised

- **engine-core**: implement `World` (execute + fingerprint), `Spine::new`,
  `schedule(EventSpec::new(EventClass::new(1),
  Schedule::AtLogicalTime { due: Tick::ZERO }, 42))`, `run_tick()`,
  `spine.world()` fingerprint through `Fingerprinter`. Program and run
  output: `tick=1 executed=1 digest=0x89cd31291d2aefa4`.
- **engine-persist**: `LocalFileMedium::open(temp dir)` → creates the
  objects/quarantine/HEAD.lock layout, validates, drops cleanly. Output:
  `opened local store at ...` / `closed cleanly`.
- **engine-renderer**: the S1 submission contract's plain-data types with
  NO device — `FrameCamera`, `OutputTarget`, `SceneBounds`,
  `DynamicLayers::default()`. Output: `camera eye=[0.0, 1.0, 5.0]
  target=1280x720 bounds=SceneBounds { ... } world batch len=0`.
- **engine-shell**: `InputState::new()` + `linked_sdl_version()` (the
  SDL call with no window and no audio device). Output: `input state
  constructed; linked SDL version Version { major: 3, minor: 4, patch:
  20 }` — see F-01 for the version drift this printed.
- **engine-slice**: one pure library helper, `engine_slice::math::
  camera_forward(0.0, 0.0, 0.0)` — no demo mode entered. Output:
  `camera forward at yaw=pitch=roll=0: [-0.0, -0.0, -1.0]`.

### 6.3 Results (all exit 0; single cold runs, this rig)

| Consumer | Cold `cargo build` | Closure (unique, all edges) | Fresh lock packages | Runs? |
|---|---:|---:|---:|---|
| engine-core | **2.9 s** | **2** (self + core) | 2 | yes |
| engine-persist | 9.0 s | 26 | 43 | yes |
| engine-shell | 44.5 s | 15 | 21 | yes |
| engine-renderer | 41.9 s | 54 | 61 | yes |
| engine-slice | 52.6 s | 71 | 87 | yes |

Isolation measurements (cost attribution, `cargo clean -p <crate>` +
timed rebuild, exit 0):

- **Bundled SQLite C compile** (`libsqlite3-sys`, persist consumer):
  **3.5 s** of the 9.0 s cold build.
- **Vendored SDL static build** (`sdl3-sys` cmake, shell consumer):
  **38.4 s** of the 44.5 s cold build — **~86% of the shell consumer's
  build cost is the SDL native build**, by design (the Stage 1
  native-library strategy: identical vendored SDL everywhere, no DLL
  redistribution).

The slice consumer's own binary: 6,655,488 B (6.35 MiB, debug profile —
it links the whole workspace statically minus the demo's main.rs).

### 6.4 Consumability verdicts (public API without internal knowledge)

- **engine-core: excellent.** A spine + tick consumer compiles 2.9 s with
  ZERO registry dependencies and needs only the lib.rs docs (World trait,
  EventSpec, Spine). No internal knowledge required.
- **engine-persist: good, one caveat.** `LocalFileMedium::open` is
  self-explanatory and works; but the consumer cannot avoid compiling the
  SQLite medium it never uses (F-02).
- **engine-renderer: the plain-data types work without a device** (the S1
  contract delivers what it promises at the TYPE level), but the crate
  offers no surface that avoids the native stack: using only
  `FrameCamera`/`SceneBounds` still compiles SDL statically, ash, and the
  naga build-time shader pipeline (F-03). No headless/types-only split
  exists — that is a P4/P6 API-architecture question, not a manifest fix.
- **engine-shell: fine for what it is** — the SDL boundary is the
  crate's stated purpose; `linked_sdl_version` links the native library
  without a window. Debug-profile consumers see a benign
  `LNK4098: defaultlib 'MSVCRTD' conflicts` linker warning (the
  statically-built SDL's CRT choice vs rustc's debug CRT) — cosmetic,
  but the first thing a new consumer sees.
- **engine-slice: consumable as a library** (pure helpers work), but any
  use compiles the entire workspace — it is the integration/demo driver
  by design; the charter's "application demos/benchmarks should not be
  required for library use" is satisfied for the four library crates and
  structurally N/A for slice itself.

## 7. Findings

- **F-01 (MED — native-version drift): fresh consumers resolve a
  different SDL than the workspace.** The shell/renderer/slice consumers'
  fresh locks picked **sdl3-sys 0.7.3 + vendored SDL 3.4.20** (run output
  `Version { 3, 4, 20 }`), while the workspace lock pins sdl3-sys
  0.7.2 + SDL 3.4.18. The manifests request `sdl3-sys = "0.7.2"` — a
  caret range — so the "identical native version on the dev pod, CI
  runners, and the rig" invariant (the Stage 1 lead decision) is enforced
  ONLY by the committed `Cargo.lock`, not by the manifests. Any external
  consumer, or a `cargo update`, silently changes the linked native SDL.
  One-line fix exists (`=0.7.2` pin) but is an owner manifest decision —
  this lane changed no manifests.
  **DISPOSITION (2026-11-27, lane NEW-REND-11, owner-approved per
  D-EE-10 item 3): FIXED — every SDL requirement in the workspace
  manifests now carries an exact pin** (the resolution-contract sweep:
  the repo's six manifests contain SDL requirements only in engine-shell
  and engine-renderer). Pin map (nominal → lock-carried → pinned):
  `sdl3` "0.20" → 0.20.0 → `=0.20.0` in both crates' `[dependencies]`;
  `sdl3-sys` "0.7.2" → 0.7.2+SDL-3.4.18 → `=0.7.2` in both crates'
  `cfg(unix)` console-build sections (the only sdl3-sys requirements in
  the workspace). The pins match what the committed lock already
  resolves, so the workspace build and `Cargo.lock` are byte-identical
  before/after (the manifest now enforces what the lock carried). The
  facade pin closes the same caret aperture one level up — without it a
  future sdl3 0.20.x would drift fresh consumers again while the
  workspace lock held 0.20.0. Native identity is transitively exact:
  sdl3-sys 0.7.2's own manifest pins `sdl3-src = "=3.4.18"` (upstream,
  outside workspace control, but anchored by the sys pin). Fresh-consumer
  demonstration (the §6.1 method, throwaway dirs outside the repo,
  recorded in EVIDENCE.md E-13): pre-pin fresh lock resolved
  sdl3-sys 0.7.3+SDL-3.4.20 + sdl3-src 3.4.20 and ran
  `Version { 3, 4, 20 }`; post-pin fresh lock resolves
  sdl3-sys 0.7.2+SDL-3.4.18 + sdl3-src 3.4.18 (cargo notes the newer
  versions "available" and does not select them) and runs
  `Version { 3, 4, 18 }`; closure shape unchanged (15 unique, the §6.3
  shell-consumer figure). Any future SDL upgrade is now a deliberate
  manifest edit, per the owner's clause.
- **F-02 (MED → RESOLVED 2026-11-28, D-EE-10 item 4 — the owner
  APPROVED the `sqlite` feature gate): rusqlite WAS unconditional
  in engine-persist.** The original finding: local-only consumers
  paid 3.5 s of SQLite C compile + 10 external crates they never
  call; the counterweights (SqliteMedium the documented production
  fallback, `migrate_local_to_sqlite` public API, engine-slice's
  test-harness dev-dep) made it the owner's call. **The
  disposition (measured at this lane's tree, the
  ee/persist-sqlite-feature-gate worktree):** rusqlite is now
  `optional = true` behind engine-persist's `sqlite` feature —
  **ENABLED BY DEFAULT** (ordinary consumers retain the full
  functionality: the provider, the §6.1 migration, the backup);
  engine-slice threads its own `sqlite` feature (default ON,
  `default-features = false` on the persist edge) so the
  integration tier is genuinely local-only under
  `--no-default-features`. The measured fresh-minimal-consumer
  differences (the §6.1 method, throwaway dirs outside the repo):
  the **lock resolves 42 → 12 packages**; the unique closure
  (`--edges normal,build,dev`) **26 → 12 real packages** — the
  disappeared set is exactly the SQLite chain: rusqlite,
  libsqlite3-sys, fallible-iterator, fallible-streaming-iterator,
  hashlink, hashbrown 0.17.2, foldhash, smallvec, bitflags (linked)
  plus the C toolchain cc, find-msvc-tools, pkg-config, shlex,
  vcpkg (build-only) — **14 crates** (9 on the normal edge alone);
  the **cold consumer build 16.9 s → 10.4 s (−6.5 s, −38%,
  including the bundled SQLite C compile)**; the **consumer binary
  2,378,240 → 658,944 B (−72%)**; the cold target dir 440.11 →
  345.02 MiB. Both consumers RUN identically (the local medium's
  open/validate/drop lifecycle). The no-weakening rule is enforced
  at the store-selection seam: a `StoreLocation::Database`
  selection in a local-only build refuses with the typed
  `MediumError::SqliteNotBuilt` naming the missing feature — never
  a silent local downgrade — and the CI carries the local-only leg
  (the exclusion-map gate: 125 Windows / 124 Linux sqlite-gated
  tests excluded, both batteries green).
- **F-03 (design question, no quick fix): the renderer has no
  types-only surface.** Plain-data submission types (no device) still
  pull the full SDL static + ash + naga shader compile (41.9 s cold).
  This is the measured form of B15's "ensure minimal consumer does not
  pull unnecessary graphics/audio/storage" for the renderer: today it
  cannot avoid it. A submission-types split would be an API architecture
  change (P4/P6); the closure discipline itself is tight (16 normal
  unique — the FINDINGS B15 rend-half claim holds for the full crate).
- **F-04 (LOW): ash compiles twice per profile** (72.6 MB + 68.4 MB
  rlibs) — sdl3-sys's build-script metadata tree carries its own
  feature-diverged ash copy. Inherent to sdl3-sys packaging; no
  workspace-level fix; recorded so nobody "fixes" it by de-duplicating
  manifests.
- **F-05 (LOW): duplicate versions** (hashbrown ×2–3, syn ×2, foldhash
  ×2, §2.2) — ecosystem-typical, semver-major, no action.
- **F-06 (verification): the test-facility isolation features are
  closure-clean.** composite-test-fixture / crash-test-seams pull zero
  crates; engine-slice's dev-deps pull zero NEW crates (rusqlite already
  production). The manifest-level isolation claims are confirmed at the
  resolution level.
- **F-07 (method hazard): plain `cargo tree` includes dev+build edges** —
  57 vs 53 unique for engine-renderer. Any future footprint claim must
  pass `--edges` explicitly (this report's convention).
- **F-08 (by-design note): two SHA-256 implementations coexist.**
  engine-core carries a private hand-rolled `mod sha256` (the
  zero-dependency discipline; save-format hashing) while engine-persist
  uses the audited `sha2` crate (content addressing with domain
  separation). Semantically necessary per the architecture's own rules —
  consolidating would either break core's zero-dep invariant or replace
  the audited path; the charter's "do not replace SHA" applies. No
  action; recorded because the charter asks the duplicate-hashing
  question explicitly.
- **F-09 (INFO): rusqlite's default `ffi-sqlite-wasm-rs` downloads
  sqlite-wasm-rs into every fresh lock** but never compiles it on
  Windows (target-gated in rusqlite). Lock-file readers should expect
  it.
- **F-10 (INFO): the LNK4098 CRT warning** greets debug-profile consumers
  of the statically-linked SDL (shell/renderer/slice). Benign; a
  consumer-doc note candidate.

## 8. Lightweight score implications (Sol's 8.3)

What the measurements say moves "lightweight" (and by how much, measured):

1. **The isolation discipline is already excellent** — core at ZERO
   deps, dev-deps closure-neutral in slice, feature-gated test fixtures
   pulling nothing. This is why 8.3 is already high; the residual cost is
   concentrated, not diffuse.
2. **The sqlite feature gate (F-02) — IMPLEMENTED 2026-11-28 (D-EE-10
   item 4, the owner's APPROVAL).** The prediction below held and is
   now the measured record (F-02's disposition above): a local-only
   persist consumer resolves **9 external crates** (the sha2 chain +
   core remain; 14 fewer on the build edge — the C toolchain), builds
   **6.5 s faster cold**, and links a **72% smaller binary**. The
   production-fallback story is preserved by the default-ON feature
   and the fail-visible `SqliteNotBuilt` refusal — never a silent
   downgrade. (The pre-implementation estimate was 24 external +
   9.0 s → ~14 external + ~5 s; the measured actuals are in F-02.)
3. **A renderer types-only surface (F-03)** would take a
   submission-types consumer from 41.9 s / 54 crates to near
   engine-core's 2.9 s / 2 — but it is an API architecture project
   (P4/P6), not a packaging tweak, and the S6 Garage second scene proved
   the current public API works; the cost is adoption friction, not
   correctness.
4. **Pinning sdl3-sys exactly (F-01)** costs nothing and protects the
   native-identity invariant the architecture already claims.
5. **Not levers**: the SDL static build time (38.4 s) is the accepted
   Stage 1 strategy (identical native version everywhere, no DLL hell);
  the naga build chain is the S5 shader-composition architecture; the
   version duplications are upstream. Chasing these would trade stated
   architectural guarantees for build seconds — the score should not
   move via those.

## Appendix A — engine-slice production closure (flat, 70 unique)

arrayvec 0.7.8 · ash 0.38.0+1.3.281 · autocfg 1.5.1 · bit-set 0.9.1 ·
bit-vec 0.9.1 · bitflags 2.13.2 · block-buffer 0.12.1 · bytemuck 1.25.2 ·
bytemuck_derive 1.12.1 (proc-macro) · cc 1.9.0 · cfg_aliases 0.2.2 ·
cfg-if 1.0.5 · cmake 0.1.58 · codespan-reporting 0.13.1 · const-oid
0.10.2 · cpufeatures 0.3.1 · crypto-common 0.2.2 · digest 0.11.3 ·
engine-core / engine-persist / engine-renderer / engine-shell /
engine-slice (path, v0.1.0) · equivalent 1.0.3 · fallible-iterator 0.3.0 ·
fallible-streaming-iterator 0.1.9 · find-msvc-tools 0.1.15 · foldhash
0.2.0 · font8x8 0.3.1 · half 2.7.1 · hashbrown 0.16.1 · hashbrown 0.17.1 ·
hashlink 0.12.2 · hexf-parse 0.2.1 · hybrid-array 0.4.15 · indexmap
2.14.2 · libc 0.2.191 · libloading 0.8.9 · libm 0.2.16 · libsqlite3-sys
0.38.2 · log 0.4.34 · naga 29.0.4 · num-traits 0.2.19 · once_cell 1.21.4 ·
pkg-config 0.3.34 · pp-rs 0.2.1 · proc-macro2 1.0.107 · quote 1.0.47 ·
rpkg-config 0.1.2 · rusqlite 0.40.2 · rustc-hash 1.1.0 · sdl3 0.20.0 ·
sdl3-src 3.4.18 · sdl3-sys 0.7.2+SDL-3.4.18 · sha2 0.11.1 · shlex 2.0.1 ·
smallvec 1.16.4 · spirv 0.4.0+sdk-1.4.341.0 · syn 2.0.119 · syn 3.0.9 ·
thiserror 2.0.21 · thiserror-impl 2.0.21 (proc-macro) · typenum 1.20.1 ·
unicode-ident 1.0.27 · unicode-width 0.2.2 · unicode-xid 0.2.6 · vcpkg
0.2.15 · windows-link 0.2.1 · zerocopy 0.8.63 · zerocopy-derive 0.8.63
(proc-macro).

(engine-renderer's 53-unique list is this minus the persist/sqlite/sha2
chain: block-buffer, const-oid, cpufeatures, crypto-common, digest,
engine-persist, engine-shell, engine-slice, fallible-iterator,
fallible-streaming-iterator, hashlink, hybrid-array, libsqlite3-sys,
rusqlite, sha2, smallvec, typenum — verified by `Compare-Object` of the
two sorted tree outputs.)

## Appendix B — gates on the lane tree (untouched source)

- `cargo fmt --all -- --check` → **exit 0**
- `cargo check --workspace --all-targets` → **exit 0** (`Finished dev
  profile target(s) in 5.23s`; log at `Temp\opencode\ee\b15-artifacts\
gate-check.log`)

No file under `crates/` was modified by this lane (`git status` clean
before the doc edits; the committed diff is docs-only).
