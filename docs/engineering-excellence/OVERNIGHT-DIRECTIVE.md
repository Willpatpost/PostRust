# THE OVERNIGHT DIRECTIVE — the Strict Audit IV campaign
**For the overnight coding workspace. Written by the GLM orchestrator at the owner's direction, 2026-11-28.**

You are an autonomous engineering agent taking over the PostRust engineering-excellence
program for one overnight session. This directive is your complete brief. It is long
because the program is deep — read it in order, then work the waves. The repo you have
is your primary source; this document orients you and tells you what to do with it.

---

## PART 0 — WHAT THIS IS

**PostRust** is a five-crate Rust game engine (workspace root = the repo you have):

| Crate | Role | External prod deps |
|---|---|---|
| `engine-core` | the deterministic simulation spine + NPC systems + spatial machinery | **0** (measured hard zero — never break it) |
| `engine-persist` | journal/commit pipeline, two providers (local file + SQLite), reclaim/recovery | 19 default, ~5 with `--no-default-features` |
| `engine-renderer` | Vulkan (ash) renderer, naga WGSL, animation/pose machinery | 15 |
| `engine-shell` | SDL3 window/loop/audio/input | 5 |
| `engine-slice` | the composition crate + demo app | 35 |

The owner chartered a formal program (read **`docs/engineering-excellence/DIRECTIVE.md`**
— the controlling document) to bring every one of eight engineering categories to
**≥9/10 independently substantiated, by evidence, never by claim**. The program has run
for days under a GLM orchestrator with implementation lanes; you are the overnight
continuation. The current governing audit is the **Strict Audit IV**
(`docs/engineering-excellence/audits/strict-audit-IV-096f8d9.md`) — 40 findings
(T01–T40), five execution waves, an acceptance-measurement matrix, and a coverage
ledger. **Read that audit document in full before touching anything.** This directive
supplements it with the orchestrator's codebase knowledge and the overnight scoping.

### The current state (at the branch head you should start from)

- **Branch:** `refactor/engineering-excellence` (NEVER touch `main` — it sits at
  `5b93167`, tagged `pre-excellence-5b93167`). Work lands on the engineering branch
  or topic branches off it. Fetch and check `git log --oneline -5` first — if work
  has progressed past what this directive describes, reconcile per PART 2 §2.6.
- **CI pins (measured actuals):** main shard **1629 (Linux) / 1578 (Windows)** passed,
  2 ignored, 34 result lines; renderer release **232** / 2 lines; heavy shard
  **716/715** passed, **59** ignored. The pins live in
  `.github/workflows/engine-ci.yml` (~line 747 main, ~line 883 renderer release,
  ~line 2095+ heavy).
- **What's landed:** the entire P5 resource block (R0 census harness through R5
  aliasing), the persist module split + walk-core unification, the renderer
  submission migration (S1–S6, two scenes), the shader composition, the sqlite
  feature gate (default-on), the SDL exact pin, A04's collapse, the typed
  device-loss latch, the dirty-generation protocol, the staging pool, the depth-pair
  aliasing. Every prior audit's findings are dispositioned in
  `docs/engineering-excellence/FINDINGS.md` — the register. Read it.
- **In flight when the day ended:** the A06 animation-crate extraction (T19) was
  re-dispatched and cancelled by a machine wake-up during its reading phase — zero
  work lost, its branch `ee/a06-animation-extraction` exists at a clean base with
  its full reading phase banked (the agent had read D-EE-10, the A06 record §4,
  animation.rs completely, and the consumer map). Same for A02's renderer half
  (`ee/rend-a02-commentary`).

### The control plane (read these first, in order)

1. `docs/engineering-excellence/DIRECTIVE.md` — the owner's charter
2. `docs/engineering-excellence/audits/strict-audit-IV-096f8d9.md` — the governing audit
3. `docs/engineering-excellence/FINDINGS.md` — the register (every finding, every state)
4. `docs/engineering-excellence/DECISIONS.md` — D-EE-01..10 (item 10 = the owner's
   latest authorizations — READ IT, it pre-authorizes most of what you'll do)
5. `docs/engineering-excellence/README.md` — the status index
6. `docs/engineering-excellence/EVIDENCE.md` — E-1..E-18, the measured-evidence manifest
7. `docs/engineering-excellence/designs/` — P2/P3/P4/P5 design docs
8. `docs/engineering-excellence/audits/` — every prior audit (IA, Codex, Sol)

---

## PART 1 — THE NON-NEGOTIABLE RULES

Breaking any of these is worse than finding nothing. The program's history includes
multiple instances of each violation class being caught and corrected — the record
is honest about them; keep it that way.

1. **NEVER touch `main`.** Never commit to it, never merge into it, never rebase it.
   Final integration happens only at the owner's explicit GO. You do not have that GO.
2. **NEVER weaken a test or a pin.** If your change moves a pinned value, that is a
   finding to investigate — never a pin to edit. Legitimate count corrections happen
   ONLY from measured actuals (you run the suite, record command + exit status + the
   TRUE counts) documented in the commit message. If a timing tolerance must change,
   it's a measured-actuals correction with the numbers in the test comment.
3. **Determinism is byte-identical.** Campaign digest pins, save round-trips,
   double-run byte-equality gates. If a change of yours moves any digest, you broke
   something — investigate, don't re-pin.
4. **Every claim carries evidence**: the command, its exit status, the tree SHA.
   Commit messages are long, lowercase, evidence-carrying records (read
   `git log --oneline -10` then one full `git show` to see the house style).
5. **Verify before accepting — including your own conclusions.** The program's
   history includes audit findings that were WRONG and were caught by direct source
   reads. For every finding you work: read the cited source, construct the failure
   if it's a hypothesis, reproduce it red before fixing it green. A refutation must
   identify WHY the proposed failure is impossible — cite the mechanism, not merely
   a passing test that doesn't exercise the condition.
6. **No silent anything.** No silent truncation, no silent downgrade, no silent
   fallback, no silent acceptance. Every refusal is typed and loud.
7. **Preserve verified improvements.** Do not restart or rewrite working subsystems.
   Do not preserve inferior architecture merely because it exists. Both directions
   require evidence.
8. **The charter's fidelity rule:** never lower fidelity to pass a metric. Full-
   resolution reflection is the default (owner decision D-EE-10 §1F).
9. **The commit discipline:** one logical change per commit, on a topic branch, with
   the evidence-carrying message. Never commit conflict markers (it has happened —
   always `grep -P '<<<<<<<|>>>>>>>|=======' <files>` after any manual merge
   resolution). Never commit scratch files.
10. **Scalability is NOT code-complete** (the owner's explicit instruction). Do not
    characterize any of it as done. Implement mechanisms; do not close findings
    through documentation or design records alone.

---

## PART 2 — THE PROCEDURES

### 2.1 The verification-before-acceptance discipline (for every finding)

1. **Read the audit's claim** (the strict-audit-IV document) and the register row
   (`FINDINGS.md`) with its cross-links.
2. **Read the cited source** at the current head. Line numbers in the audit refer to
   `096f8d9`; the head may have drifted — find the mechanism, not the line.
3. **Classify the claim**: CONFIRMED (the mechanism is visible), HYPOTHESIS (a
   proposed failure you must construct), OBLIGATION (architectural work), or
   INVESTIGATION (measurement-gated).
4. **For hypotheses: build the red witness FIRST** — a test or standalone probe that
   demonstrates the failure. If you cannot construct it, either the hypothesis is
   refuted (document WHY with the specific mechanism) or your witness is wrong
   (get a second opinion from the source before claiming refutation).
5. **Only then fix.** The fix carries the discriminating test (red-then-green where
   practical), the equivalence battery (the existing gates prove you didn't break
   anything), and the measured evidence.
6. **Record the disposition** in the register with the commit SHA.

### 2.2 The lane lifecycle (how work is organized)

- **One lane = one branch = one worktree = one agent.** Never share a writable
  worktree. The orchestrator pre-creates branches (D-EE-09); if you are alone
  overnight, you may create your own topic branches (`ee/overnight-<topic>`) off the
  integration head — but NEVER run branch commands in a worktree another process
  might be using, and never commit to the integration branch directly from a lane.
- **Lane brief discipline:** before each lane, write yourself a one-page brief:
  the finding(s), the allowed edit paths, the invariant, the gates, the exit
  criteria. Keep it in a scratch file if needed; the commit message carries the
  durable record.
- **Allowed edit paths per lane:** keep them NARROW. If a fix requires touching
  outside its natural scope, document why in the commit.
- **Merge discipline (if you integrate your own lanes):** review the exact diff
  (`git diff base..branch`) before every merge; check for conflict markers after
  manual resolutions; never merge a lane whose gates you haven't run.

### 2.3 The gates (what to run, when)

Per lane, at minimum:
- `cargo fmt --all -- --check` — clean
- `cargo clippy -p <touched crates> --all-targets -- -D warnings` — clean
- `cargo test -p <touched crates>` — the full battery, counts recorded
- `cargo check --workspace --all-targets` — the downstream compile

The full-suite commands:
- **Main shard:** `cargo test --workspace --exclude engine-slice --features engine-persist/composite-test-fixture`
- **Slice suite:** `cargo test -p engine-slice` (its `#[ignore]`d tests are GPU gates)
- **Local-only persist:** `cargo test -p engine-persist --no-default-features`
- **Renderer release:** `cargo test -p engine-renderer --release`

**Count pins move ONLY from measured actuals**, in a dedicated pin-correction commit
that maps every movement to its tests. The workflow file is YAML — after ANY edit to
it, run a real YAML parse (python `yaml.safe_load`) — the program has twice broken CI
with unvalidated workflow edits (a glued comment, a lost indent; both recorded in git
history — read `096f8d9`'s message).

### 2.4 The environment: what you can and cannot verify overnight

Your workspace is NOT the development rig (an RTX 5070 Ti Windows box). Before
planning work, detect your environment:

```sh
# What Vulkan do you have?
# (a GPU-less Linux container: no Vulkan loader at all)
ls /usr/share/vulkan/icd.d/ 2>/dev/null || echo "no Vulkan ICD directory"
# lavapipe (software) may be present on Linux — check for:
ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null
```

- **If you have no GPU and no lavapipe:** you CANNOT run the `#[ignore]`d rig
  batteries (render_post, render_instanced, etc. — they need real windows/SDL).
  Your overnight scope is the CPU-side findings and the pure-mechanism work
  (PART 5 marks each T-finding's overnight suitability). GPU-dependent
  verification is DEFERRED, not skipped: the lane's handoff must say exactly
  which rig gates remain.
- **If you have lavapipe:** you can run the lavapipe-class smokes
  (`SDL_VIDEODRIVER=offscreen cargo test -p engine-slice --test render_orientation
  -- --ignored ...`) — but NOT the perf-sensitive gates (software rendering
  makes timing gates meaningless).
- **The `render_post` poison cascade:** one failing SDL test poisons the binary's
  shared lock — run batteries unfiltered to see true counts, or per-test to isolate.
- **Windows vs Linux:** the pins are platform-specific (the workflow's
  `expected=` splits on `uname`). Your local counts are YOUR platform's — record
  them as such; never claim the other platform's number without its run.

### 2.5 The reporting discipline (how to leave the morning handoff)

Keep a running `docs/engineering-excellence/OVERNIGHT-LOG.md` — append per lane:
- The finding(s) worked, the verification verdict (confirmed/refuted/partial + the
  specific mechanism), the fix as landed, the tests added, the gates run (commands +
  exit + TRUE counts), the count movements, the unresolved remainders.
- Every commit on the integration branch (or topic branches left for the
  orchestrator to review-merge — PREFER topic branches + a merge request in the log
  if you are at all unsure of a change).
- The final section: "STATE AT DAWN" — the head SHA, the pins, the open items, the
  rig-dependent verifications owed.

### 2.6 If work has progressed past this directive

Fetch; check `git log --oneline` against the SHAs this document names. If the head
moved, read the new commits' messages (they are complete records) and reconcile each
finding you planned to work against the newer tree before starting. Do not assume a
finding is still open — check the register.

---

## PART 3 — THE COMPLETE FINDING LEDGER (every audit, every item)

Status legend: ✅ closed with evidence · 🔶 partial (mechanism landed, residual
named) · ⏳ in flight · ❌ open · 🔁 recurred

### The original P0 audits (A01–A25, B01–B16)

| ID | Item | Status | Where / residual |
|---|---|---|---|
| A01 | file splits | 🔶 | vulkan (S4) ✓, persist (L9) ✓; spine.rs + renderer residuals → T36 |
| A02 | commentary | 🔶 | core ✓ (git `72c91d3`); renderer half = the cancelled lane, branch `ee/rend-a02-commentary` → T36 |
| A03 | demo entry points | ✅ | the S1–S6 migration; R06 closed (git `7e4c8f8`) |
| A04 | 19-param frame | ✅ | the collapse (git `9e17516`) |
| A05 | world constants | ✅ | L8 WorldConfig + v2 (git `1396ba6`) |
| A06 | animation placement | ⏳ | T19 — the extraction, branch `ee/a06-animation-extraction` |
| A07 | perception walk | ✅ | L6 census grid (git `40e0828`) |
| A08 | scheduler queue | 🔶 | CA-08 class index (git `f8de20f`) — held/blocked flat; queue-side residual → T09 |
| A09 | witness memory | ✅ | L4 index (git `88628bd`) |
| A10 | collision scan | ✅ | L5 broad phase (git `11cf669`) |
| A11 | fixed capacity | ✅ | R1 arenas (git `4d77c69`) — residuals → T04/T17 |
| A12 | shader duplication | ✅ | S5 composition (git `585187d`) — final equivalence evidence → audit matrix row |
| A13 | persist mixing | ✅ | L9 + L9b (git `622623b`, `10d3d13`) |
| A14 | GPU lifetime | 🔶 | typed loss + latch (git `a45a60e`) → T02/T06/T16/T22 |
| A15 | demo org | 🔶 | garage ✓; dispatch extraction → T36 |
| A16 | quadratic confirm | ✅ | (pre-program history) |
| A17 | inverse-bind cache | ✅ | (pre-program history; re-verified twice) |
| A18 | per-frame repack | ✅ | R2 (git `0b1a60e`) + corrective + zero-alloc mapping — residuals → T02/T05/T18 |
| A19 | ripple scan | ✅ | R4 (git `d6d7254`) — per-region residual → T24 |
| A20 | staging | ✅ | R3 (git `59eac30`) — residuals → T03/T15/T16/T23 |
| A21 | diagnostics bounds | ✅ | (pre-program history; R05 shell collector) |
| A22 | history growth | 🔶 | L7 + L6 + CA-17 stage 1 → T13/T14 eviction |
| A23 | fixed buffers | ✅ | R1 + shell no-change |
| A24 | frame-driven sim | ✅ | fixed timestep (rig-verified) — T35 extremes review |
| A25 | audio lock | ✅ | (pre-program history) |
| B01 | housekeeping | 🔶 | L3 + CA-03 → T08 |
| B02 | journal reads | ✅ | L1/L4/L5 streaming |
| B03 | copy amplification | ✅ | L2/L3/L8-Arc |
| B04 | retry machine | ✅ | IA corrective |
| B05 | particle domain | ✅ | (pre-program) — T40 re-audit |
| B06 | combat scan | ✅ | broad phase + tie — T10/T11 large-scale |
| B07 | rule validation | ✅ | no-change (measured) — reopen on scale change |
| B08 | input churn | ✅ | the Codex fix (git `6cf1a0d`) — T34 sustained review |
| B09 | SDL literal | ✅ | (pre-program) |
| B10 | methodology | 🔶 | labels ✓ → T39 exact-tree methodology |
| B11 | nomenclature | ✅ | B11 (git `63b01d4`) |
| B12 | identity codec | ✅ | L8 → T21 scope |
| B13 | worker stalls | ✅ | instrumentation + rig no-change (git `1caf860`) → T28 |
| B14 | render targets | ✅ | R5 split decision (git `624488a`) → T25 matched proof |
| B15 | footprint | ✅ | B15 + SDL pin + sqlite gate → T32/T33 |
| B16 | coverage | ❌ | → T40 |

### The astra IA wave — ✅ all 14 (IA01–IA14, the fix wave complete; the audit's §6.3 mandates rechecking touched paths at final head)

### The Sol re-audit (R01–R07)

R01–R04 ✅ · R05 ✅ · R06 ✅ (S6) · R07 🔣 investigated no-change → T13/T30

### The owner Q-findings (Q01–Q11)

Q01 ✅ · Q02 ✅ · Q03 🔶→T08 · Q04 🔶→T11 · Q05 🔶→T07 · Q06 🔣→T12 ·
Q07 🔶→T21 · Q08 🔣→T13/T14 · Q09 ✅(measured)→T37 · Q10 ✅→T03 residual · Q11 🔁→T38

### The Codex audit (CA01–CA23) — ✅ all dispositioned (CA-14 and CA-17 at stage 1; the audit's §6.3 governs their reopening)

---

## PART 4 — THE T-WAVE: THE OVERNIGHT CAMPAIGN

The strict audit's five waves. **Your overnight scope is marked per finding.**
Priority order for the night: Wave 0 → Wave 1's CPU-verifiable findings →
Wave 2's CPU work → the A06 extraction (T19) — see PART 6 for the schedule.

### T01 — exact-head CI verification · CONFIRMED (remote) · Wave 0 · ⚡ DO FIRST
**Where:** `.github/workflows/engine-ci.yml`; the repair at git `096f8d9` fixed a
YAML break (a comment line at column 0). A run was triggered on that SHA.
**What to do:** `gh run list --workflow engine-ci.yml --limit 3` (if you have gh
credentials; else check the GitHub web UI if accessible; else NOTE it as
owner-verifiable and move on). If the run completed: read every job's result;
any count mismatch is corrected from the run's actuals (the Linux numbers are
inferences that have never run — mismatches are EXPECTED, correct them one by
one in a pin-correction commit mapping each movement). If the run failed on
something other than counts (a compile error on Linux, a feature combination),
that's a real finding: fix it.
**Overnight:** ✅ fully doable (CLI/web only).

### T02 — generation-stamp invalidation too late on interrupted preparation · HYPOTHESIS · Wave 1 · ✅ CPU-verifiable
**Where:** `crates/engine-renderer/src/vulkan/frame.rs` — the R2 lane's
`PopulationStamps` in `FrameState`, consumed in `upload_frame_data` via
`generation_write_needed`; the invalidation arms (the swapchain recreation
clearing all stamps; the arena-growth delta clearing).
**The hypothesis:** a frame that fails BETWEEN the stamp consumption (the slot
records "I consumed generation G") and the actual write completion can leave the
stamp ahead of the slot's true state — the next frame at generation G skips a
write that never happened.
**Verification:** read the exact order — when does the stamp update relative to
the fallible upload steps? Construct the interrupted case with the
`LifecycleFaultSite` machinery (the fault-injection seam in the same file —
extend it if no site covers the window). Red witness first.
**Fix if confirmed:** move the stamp commit to after the successful write (the
CA-10 deferred-commit discipline — the same pattern the fence reset fix used).
**Tests:** the fault-injected interruption then retry writing correctly; the
missed-bump verify (already exists, debug-mode) as the backstop.
**Overnight:** the code analysis and fix are CPU-doable; the fault-injection
witness needs the GPU — write it, mark it rig-deferred, ship the fix with the
debug-verify evidence.

### T03 — attach publishes before deferred upload readiness · CONFIRMED ordering + HYPOTHESIS error · Wave 1 · ⚠ GPU-verification-heavy
**Where:** `crates/engine-renderer/src/renderer.rs` `attach_scene` → the R3 lane's
deferred flush: `CornerResources::build` flushes deferred, the drain at
`attach_scene`'s end. The R3 lane documented "publish-then-drain" as deliberate
(post-commit drain failure = device health, not attach atomicity).
**The audit reopens it:** the error outcome is unresolved — if the drain fails
(stuck fence → the typed timeout), the scene is published with possibly
un-drained uploads; a first frame could read unwritten buffers.
**Verification:** trace the exact publish/drain order and every drain-failure
path. Can a submitted frame observe un-drained staging regions? (The staging
readiness tracking says regions release only on their own fence — but a FAILED
drain means the fence never signaled, the regions still bound, the data maybe
never arrived.)
**Fix direction:** latch the drain failure into the renderer state (refuse
subsequent frames with the typed error — the device-loss latch pattern), or
reorder the publish after the drain.
**Overnight:** the code analysis and the latch fix are CPU-doable; the
end-to-end proof needs the rig. Do the analysis + fix; mark the rig gates.

### T04 — dynamic geometry violates complete-primitive admission · CONFIRMED (S0, owner contract) · Wave 1 · ✅ CPU-verifiable
**Where:** the region-geometry upload path — `crates/engine-renderer/src/vulkan/
pipelines.rs` `ensure_region_capacity` + the layered upload in the frame path
(the F-03 clamp machinery, `config.rs`'s region clamp/counter).
**The owner's D-EE-10 §1E ruling:** complete-primitive admission, no silent
truncation, requested/admitted/omitted distinct.
**What's wrong:** the clamp truncates at BYTE capacity — mid-primitive (a
SceneVertex-stride boundary, not a triangle boundary). R1's growth makes it
rarer but the clamp still exists for over-cap demand.
**Fix:** the admission rounds the truncation down to the last complete primitive
(the CA-15 `triangle_list_boundary` discipline applied to the dynamic path);
the counters report requested/admitted/omitted PRIMITIVES (not vertices);
over-cap demand that fits no primitive refuses typed rather than drawing
partial geometry.
**Tests:** the over-cap fixture (the existing render_overflow battery is the
shape — it currently pins the byte-truncation behavior! read it first) with the
primitive-boundary admission; the omitted-primitive counter exact.
**Overnight:** code + CPU tests doable; render_overflow is lavapipe-class —
if you lack lavapipe, the unit-level admission tests carry the night and the
battery is rig-deferred.

### T05 — raw caller generations not robust identity contracts · CONFIRMED + hypothesis · Wave 1 · ✅ CPU-verifiable
**Where:** `crates/engine-renderer/src/proxy_instances.rs` — the R2 lane's
`instance_generation`/`pose_generation` u64 fields on `ProxyPopulation`; the
retained `PopulationGenerations` in the renderer; the debug-verify.
**The hypothesis:** colliding generation spaces across population SOURCES (two
callers both starting at 1, same counts) can skip writes wrongly. The debug
verify catches divergence; release builds have no protection.
**Verification:** construct the collision (two sources, colliding generations,
different content, same count). Does the stamp gate skip the second source's
write because the (generation, count) matches?
**Fix direction (the audit's):** a robust identity — an epoch/fingerprint
component beside the generation (a cheap content-class token: the source's
pointer-derived or caller-declared identity), checked with the generation.
The Q02 borrowed-roster contract is the precedent shape.
**Overnight:** fully CPU-doable (the packing and stamps are pure CPU; the
debug-verify battery is unit-level).

### T06 — error disposition can lose terminal device state · CONFIRMED latch gap · Wave 1 · ⚠ GPU-verification-heavy
**Where:** `crates/engine-renderer/src/vulkan.rs` (`RendererError::DeviceLost`,
the `DeviceLossLatch`) + `vulkan/frame.rs`'s `dispose_failed_acquisition` +
every device-touching public entry's observe-on-exit/gate-on-entry.
**The gap:** paths that error WITHOUT observing the latch (a disposal that
itself fails, a stuck-fence timeout that's device-loss masquerading as
timeout — T16's interaction).
**Fix:** the latch observation at EVERY exit including disposals; the stuck-fence
classification consulting device loss first.
**Overnight:** analysis + the code fix doable; the proof is fault-injection on
the GPU — rig-deferred.

### T07 — retention acknowledgment is a permanent assertion, not a safe moving cut · CONFIRMED (S0 for consequences) · Wave 1 · ✅ FULLY CPU — THE NIGHT'S HEADLINE FINDING
**Where:** `crates/engine-core/src/spine.rs` — the CA-07 lane's
`ConsequenceEligibility` token + `set_retention_policy` + the end-of-run_tick
checkpoint; the L7 lane's original policy.
**What's wrong:** the token is constructed once by the composition; once armed,
the cadence cuts forever — even if NEW consequence consumers appear later
(a new lane reading the ledger's history would find its prefix gone). The
acknowledgment is "permanent assertion," not a living contract.
**The audit's corrective direction:** a MOVING safe-cut frontier — the
composition declares its consequence consumers (or their oldest-needed
position); a cut beyond the oldest consumer's need is refused; new consumers
extend the frontier. Alternative shapes: (a) a consumer-registry on the spine
(the cut never passes the minimum registered need), (b) the eligibility
re-established per firing (the token carries a frontier the composition
advances explicitly), (c) a lag/refusal window (the cut trails the confirmed
consumption position by a configurable margin).
**Your judgment from the code** — read how the consequence consumers actually
read the ledger (the crossing's `compact_session`, the memory lane, the
recovery paths) and pick the shape that matches reality. The design decision
gets documented ON the type; the mechanism gets a discriminating test:
a cut attempt beyond a registered consumer's need REFUSES (counted, lag
exposed), the consumer then reads its history intact, the frontier advances
as consumers release.
**Preserve:** the L7 sustained-load bound battery, the restore→reconfigure→
next-compact equivalence (they must stay green — the moving cut composes with
the existing cadence).
**Overnight:** fully CPU (engine-core, the spine's own batteries).

### T08 — housekeeping observation does not bound work · CONFIRMED · Wave 2 · ✅ CPU-verifiable
**Where:** `crates/engine-core/src/spine.rs` — the CA-03 lane's
`HousekeepingStats` + the E-10 atomic release at `execute_now` (all-or-nothing
per execution, the documented contract).
**What's wrong:** the observation counts fan-out but nothing BOUNDS it — a
single tick can release an arbitrary waiter batch (the ninety-waiter storm
published as "worst tick").
**The audit's direction:** the resumable release cursor the CA-03 lane decided
and documented but did not implement: releases in bounded batches across
ticks, preserving registration visit order, cursors suspending only at
execution boundaries, `visits == decrements + unblocks + no-ops` at every
boundary.
**This is determinism-core work** — read the CA-03 lane's documented contract
(the comment block at `execute_now`) and implement exactly it. The equivalence
battery: existing execution histories + canonical saves unchanged under high
fan-in, partial release, both lanes, save/restore at each phase boundary.
**Overnight:** fully CPU. High care — the spine is the determinism core; run
the FULL engine-core battery after every step.

### T09 — queued ops traverse unrelated heap · CONFIRMED · Wave 2 · ✅ CPU — heavy design work
**Where:** `crates/engine-core/src/spine.rs` — the lane heaps (`BinaryHeap`)
that `suspend_matching`/`reclassify_matching` drain entirely to scan (the
CA-08 lane's documented residual: "the keyed lane representation — an
allocation-pin and drain-determinism project").
**The audit promotes it.** The design questions: a secondary index (class →
lane positions) beside the heap, or a different queue structure entirely.
The determinism constraints: the pop order must stay canonical ((due,seq)
ascending); the wire must stay byte-identical; the spine_allocations pins
(18/6880 cold, 9/3024 warm) must not move without measured cause.
**Overnight approach:** DESIGN FIRST (a document: the options with their
allocation/determinism arithmetic), then implement the chosen shape if the
design is clean. If the design reveals the change is bigger than a night,
land the design record + the first bounded step. Do NOT rush the determinism
core — a partial careful step beats a complete hasty one.

### T10 — oversized lane not localized over many objects · CONFIRMED · Wave 2 · ✅ CPU-verifiable
**Where:** `crates/engine-core/src/player.rs` + `combat.rs` — the Q01 lane's
`GRID_OVERSIZED_CELL_LIMIT` + the oversized list (one entry per object,
consulted by every query).
**What's wrong:** many oversized objects = the oversized list itself is a
linear scan per query.
**Fix direction:** a coarse-bounds index over the oversized entries (sort by
an axis, binary-search the query extent) — the same conservative-correctness
discipline (bit-identity preserved, the re-sort-to-insertion-order).
**Tests:** the many-oversized fixture (100+ oversized colliders) with the
query-cost measurement (flat vs the list scan); the bit-identity battery.
**Overnight:** fully CPU.

### T11 — large live composition unproven · CONFIRMED · Wave 2 · ✅ CPU-workload work
**Where:** the test fixtures — no composition exists at 1k/5k/20k/100k with
census-verified REAL populations. The adoption gates (CA-09's FireRoster,
the census index) are wired but unproven at scale.
**What to do:** build the scaled-workload fixtures per the audit's §7
scheduler/spatial rows: real armed/queued obligations (census-verified —
the audit explicitly warns a large `--population` argument doesn't create
actors without a census of executed/armed/visible counts), the measurements
at each scale, the adoption gates' behavior at scale (do they fire? does the
indexed path win?).
**Overnight:** fully CPU (simulation-side scale; the census batteries are
release-runnable without GPU).

### T12 — scene generalization stages 2–4 · CONFIRMED (deferred stages) · Wave 3 · ⚠ GPU-verification-heavy
**Where:** `crates/engine-renderer/src/scene_contract_design.rs` — the CA-14
lane's design record with the four-step migration (dynamic material count,
per-pass negotiation, the second resource topology, per-set extents).
**Overnight:** the design is done; the implementation steps are renderer work
whose proof is GPU-heavy. Assess step 2 (the dynamic material count) for
CPU-testability (the admission/validation logic is pure); if the acceptance
tests can run headless, proceed; otherwise leave detailed and defer.

### T13 — historical persistence residency eviction · CONFIRMED · Wave 2 · ✅ CPU — the design exists
**Where:** `crates/engine-persist/src/residency.rs` — the CA-17 stage-1 design
record (the rotation-authored ledger recommendation, the staged plan, the
[ASSUMPTION] markers) + `pipeline.rs`'s confirmed map.
**What to do:** implement stage 2 per the design — the rotation-authored
ledger + the confirmed-entry eviction, preserving the preservation duties the
design lists (duplicate detection, retry identity, provenance across both
providers). The cold-lookup latency rows are on record (6.55/21.5/83.4 ms) —
the eviction must not regress them past the design's bound.
**Overnight:** fully CPU (persist batteries both feature sets).

### T14 — cold-lookup trust/work boundary · CONFIRMED code + hypotheses · Wave 2 · ✅ CPU — follows T13
**Where:** the same `residency.rs` cold_confirmed path. The integrity
hypotheses (the trust boundary of the cold walk) verify alongside T13's
eviction — the eviction is what makes the cold path load-bearing.

### T15 — staging not pressure-aware · CONFIRMED · Wave 3 · ⚠ GPU
The pool is a bump allocator with a fixed ceiling — region reuse (the
freed-by-fence ranges) is tracked but not REUSED. The fix (a free-list over
released regions) is renderer work with GPU verification. Design-level
overnight work at most.

### T16 — staging errors can mask device loss · CONFIRMED (S0 with T06) · Wave 1 · ⚠ GPU — pairs with T06
The staging timeout/capacity errors must classify device loss first. The code
fix pairs with T06's latch work; the proof is fault-injection on the GPU.

### T17 — arena whole-device waits + high-water retention · CONFIRMED · Wave 3 · ⚠ GPU
The growth paths use `device_wait_idle`; retention never shrinks. The
fence-scoped retirement (releasing under the owning slot's fence instead of
the whole device) is renderer/sync work — GPU verification. Design overnight.

### T18 — whole-set generation only coarse dirty · CONFIRMED · Wave 3 · ✅ CPU-designable
The per-instance sparse dirty tracking (the design's deferred third-order).
The packing math is CPU; the benefit proof is the repack matrix (CPU release).
Design + implement the sparse scope if the night allows after the Wave 1 CPU
findings.

### T19 — the animation extraction · ⏳ IN FLIGHT · Wave 3 · ✅ FULLY CPU
**This is the A06 lane.** Its branch exists (`ee/a06-animation-extraction`)
with the full reading phase banked. The migration plan is
`DECISION-A06-animation-crate.md` §4 (E-S1..S4: the additive crate + shim →
the slice rewire → the shim decision → the re-measure). The authorization is
D-EE-10 item 2 (bytemuck placement judgment, closure minimized, fingerprints
preserved). **This is the highest-confidence overnight lane** — pure
mechanical migration with exact equivalence bars. Dispatch it EARLY.

### T20 — numerical admission boundaries · CONFIRMED · Wave 3 · ✅ CPU
**Where:** the submission-path numeric domains (frame inputs — camera,
bounds, wet params) vs the static scene validation (R02 fixed static). The
frame-domain checks for non-finite/extreme values. The admission shape: the
typed refusals at the submission boundary (the CA-12 admitted_bytes pattern).
**Overnight:** CPU (the submission validation is pure).

### T21 — decode bounds exclude transients · CONFIRMED · Wave 1 · ✅ CPU
**Where:** `crates/engine-core/src/npc_world_save.rs` — the CA-02 lane's
`SectionDecodeLimits` gate the column reservations but not the TRANSIENT
decode structures (the per-region maps during restore, the index memory).
**Fix:** the full resource budget (the transients counted against the
caller's limit); the default documented as the wire maximum with the
transient arithmetic.
**Overnight:** fully CPU (engine-core).

### T22 — bare-handle ownership ladders · OBLIGATION · Waves 1–3 · architectural
The scoped-owner discipline (ScopedMappedBuffer, CornerPipelineGroup)
generalized. This is the audit's architectural umbrella over T02/T06/T16/T17
— treat it as the DESIGN LENS for those fixes, not a separate lane.

### T23 — pool net residency narrative · REPORTED · Wave 4 · measurement
The census rows exist; the audit wants the net-residency story (the pool's
own 1 MiB counted). A documentation + census-column task. Low priority.

### T24 — ripple per-region culling · CONFIRMED · Wave 3 · GPU
The R4 trigger (a strict-subset receiver scene) hasn't fired — the audit
wants the per-region field culling regardless. Design-level overnight.

### T25 — target-sharing matched proof · REPORTED · Wave 4 · GPU
The R5 depth-aliasing needs the workload-specific matched A/B (the §7 GPU
memory row). Rig work.

### T26 — CPU whole-scene capacity architecture · OBLIGATION · Wave 3 · architectural
The renderer's whole-scene/proxy orientation (every frame considers the
whole scene). The long-term architecture (visibility clustering etc.) —
design obligation, not an overnight lane.

### T27 — frame-stats semantics · INVESTIGATION · Wave 4 · CPU
The bounded/meaningful semantics for the frame stats and timestamps. An
investigation + design record. CPU-doable.

### T28 — authority-thread blocking · CONFIRMED · Wave 4 · CPU-measurable
The B13 no-change was confirmed on the rig — the audit says the worker
measurement doesn't resolve the AUTHORITY-thread blocking (the caller-side
stall decomposition). The §7 persistence row's sustained workloads. CPU
measurement work.

### T29 — walker startup/suffix residency · CONFIRMED structure · Wave 2 · CPU
The unified chain_walk's startup/suffix residency bounds (the cold-open O(journal)
and the suffix retention). Measure + bound per the audit. CPU (persist).

### T30 — transaction consolidation · REPORTED/DESIGN · Wave 2 · CPU
The R07 design record's options — the audit keeps it staged. No overnight
action beyond what T13/T29 naturally cover.

### T31 — backup cross-provider proof · CONFIRMED improvements + proof · Wave 4 · CPU
The CA-18/19/20 fixes need the cross-provider fault battery (every publication
boundary faulted, both providers). CPU (persist + slice backup tests).

### T32 — animation/render contract footprint · CONFIRMED · Wave 3–4 · follows T19
The types-surface investigation — AFTER the extraction lands, measure the
independent consumer closures (the B15 method). Follows T19 naturally.

### T33 — SDL/sqlite exact-head evidence · IMPLEMENTED · Wave 4 · CI-dependent
The evidence is the exact-head CI run (T01). No separate overnight action.

### T34 — input/audio sustained review · REPORTED · Wave 4 · partially CPU
The sustained burst/latency reviews (the §7 shell row). The dummy-driver
smokes are CPU-runnable on Linux (SDL dummy); the real-device parts defer.

### T35 — free-look extremes · SOURCE-HISTORY · Wave 4 · CPU-review
The R03/R04 fixes' extreme-case review (the accumulator at extreme dt, the
composition order). A review + edge-fixture task. CPU.

### T36 — remaining large files · CONFIRMED · Wave 4 · CPU
spine.rs (~5,300 lines production) unsplit; the renderer's residuals. The
A01 pattern (change-reason splits, pure moves). CPU — but coordinate with
T07/T08/T09 (all touch spine.rs — SEQUENCE: the semantic work first, the
splits after, or you'll move everything twice).

### T37 — spatial foundation vs duplication · INVESTIGATION · Wave 4 · CPU
The Q09 revisit: the shared-surface promotion (combat.rs's local twin of
player.rs's cell machinery). An investigation + the promotion decision.
CPU.

### T38 — control plane stale again · CONFIRMED (recurred) · Wave 0 · ✅ DO EARLY
The register/README drift after the night's landings. The discipline: every
integration updates the register rows AND the README together. Do a
reconciliation pass EARLY (the state this directive describes is current as
of its writing — your landings will drift it again; reconcile at dawn too).

### T39 — exact-tree measurement methodology · CONFIRMED (S0-gating) · Wave 4 · cross-cutting
The §7 matrix discipline: matched workloads, census-verified populations,
exact-tree SHAs, real samples. This is the METHODOLOGY for every measurement
you take overnight — apply it as you go (record the admitted domain, the
actual counts, the command, the SHA), and the dawn report follows it.

### T40 — whole-source coverage ledger · OBLIGATION · Wave 4 · cross-cutting
The §8 coverage ledger (the 14 review groups). An overnight start: the
file-level inventory from the exact tree (`git ls-files '*.rs'` — 196 source
files at the audited snapshot) marked with coverage state. Do NOT claim
coverage without the actual reads.

---

## PART 5 — THE OVERNIGHT SCHEDULE

Ordered by (CPU-verifiable × priority × confidence):

**First hour — Wave 0:**
1. T01: the CI run check (or its notation as owner-verifiable)
2. T38: the register reconciliation baseline
3. Read the strict audit + this directive's PART 4 findings you'll work

**The night's main lanes (dispatch in this order, serially or in parallel
worktrees if your environment supports it):**
1. **T19 — the A06 extraction** (the banked reading phase; the highest-confidence
   complete-able lane; fully CPU)
2. **T07 — the retention safe moving cut** (the S0 headline; fully CPU; engine-core)
3. **T04 — the complete-primitive admission** (the S0 owner contract; CPU code +
   unit tests; the lavapipe battery if available)
4. **T05 — the generation identity contracts** (CPU; composes with T04's crate)
5. **T08 — the housekeeping bounding** (CPU; determinism core — high care)
6. **T21 — the decode transients** (CPU; engine-core; independent of the above)
7. **T02/T03/T06/T16 — the renderer lifecycle cluster** (the code analysis and
   fixes CPU-doable; the proofs rig-deferred — coordinate as ONE team per the
   audit's warning; if the night is short, do the ANALYSIS + the latch/reorder
   fixes and leave the fault-injection witnesses as rig-deferred)

**If time remains:**
- T10 (the oversized index), T11 (the scaled fixtures), T13/T14 (the eviction),
  T18 (the sparse dirty design), T20 (the numeric admission), T29 (the walker
  residency), T31 (the backup fault battery), T35 (the extremes review)

**Do NOT attempt overnight** (GPU-dependent or architectural): T12's
implementation steps, T15/T17/T24/T25/T26 — design records at most.

**The A02 renderer-commentary lane** (`ee/rend-a02-commentary`): dispatch ONLY
if the renderer files aren't being touched by the lifecycle cluster (they will
be — SKIP A02 overnight; the lifecycle work will churn the same files).

---

## PART 6 — THE MEASUREMENT OBLIGATIONS (the §7 matrix, overnight-applicable rows)

Every measurement you take: name the exact admitted domain, the actual
population/records/bytes (census-verified, not label-verified), the outcome
counters, the command + exit + SHA, and repeat runs distinguishing median
from tails. The CPU-runnable rows:

- **Scheduler work** (T08/T11): 1k/5k/20k/100k REAL active/queued obligations —
  census the armed/visible counts; the locality, backlog slope, latency.
- **Spatial** (T10): the crossover, the many-oversized, the oracle equivalence.
- **Persistence** (T13/T29/T31): the history scales at fixed live keys; the
  cold misses; the fault boundaries — both providers.
- **Wire/resource admission** (T04/T21): the extremes, the typed refusals
  before unbounded allocation, the actual peak bytes.
- **Renderer generations** (T05): the sparse/change/all-dirty matrix at the
  pack level (CPU — the GPU readback oracle is rig-deferred).
- **Animation** (T19): the equivalence bars (the pose digest, the FNV pins,
  rest-identity, mesh bit-identity) + the consumer closure re-measure.

---

## PART 7 — WHAT NOT TO DO

1. No `main` touches. No P7 claims. No "code-complete" characterizations.
2. No pin weakening; no digest re-pinning; no test deletion.
3. No unvalidated workflow edits (YAML-parse after EVERY change).
4. No conflict markers committed (grep after every manual resolution).
5. No rushing the determinism core (T08/T09): a partial careful step beats a
   complete hasty one; the full engine-core battery after every step.
6. No GPU-gate claims without the GPU (mark rig-deferred explicitly).
7. No silent scope expansion (document every out-of-path touch).
8. No re-litigating D-EE-10 (the authorizations stand — bytemuck judgment,
   extraction, growth, all of it).
9. No discarding the in-flight branches (T19's reading phase is banked value).
10. If genuinely blocked on an owner-level decision (a NEW
    authority/durability/semantic contract), write the decision record with
    alternatives and continue other work — do not stop.

---

## PART 8 — THE DAWN HANDOFF

End the night with:
1. `docs/engineering-excellence/OVERNIGHT-LOG.md` complete (per PART 2.5)
2. Every lane either merged (with its gates run) or left on its branch with
   the merge request in the log
3. The register rows updated for everything you touched
4. The "STATE AT DAWN" section: head SHA, pins, open items, the rig-dependent
   verifications owed (each named with its battery)
5. A one-paragraph summary the owner can read in thirty seconds

The orchestrator resumes in the morning, reviews your diffs against your
claims, runs the rig gates, and integrates. That's normal, not distrust.

**The bar is high and the evidence discipline is real. That is the point.**

---

## PART 9 — THE CODEBASE MAP (where everything lives, for the findings you'll work)

### engine-core (the determinism core — the highest-care crate)

| Module | What it is | Relevant findings |
|---|---|---|
| `src/spine.rs` (~5,300 lines) | THE spine: the event lanes (near/far BinaryHeaps), the blocked/held sets, drain_due (the four phases), the housekeeping charge model, the retention policy + compaction, the class index (CA-08), the fingerprints, save/restore | T07 T08 T09 T36 |
| `src/world_config.rs` | WorldConfig + the validation bounds + SectionDecodeLimits docs | T21 |
| `src/npc_world_save.rs` | the v1/v2 wire codecs, validate/restore, save_validated, the admission gates | T21 |
| `src/npc_schedule.rs`, `npc_behavior.rs`, `npc_execution.rs`, `npc_perception.rs`, `npc_locomotion.rs`, `npc_memory.rs` | the NPC systems (schedule/behavior/execution/perception/locomotion/memory — each with its digest pins and campaign batteries) | T11 (the scale fixtures) |
| `src/collision.rs`, `player.rs`, `combat.rs`, `interaction.rs`, `weapons.rs` | the spatial machinery (RotatedCollider with the validated replacements, StaticGrid + the oversized list, CombatIndex with the borrowed roster, the census) | T10 T11 T37 |
| `src/particle.rs`, `ripple.rs`, `weather.rs`, `audio_mix.rs`, `audio_synth.rs`, `audio_triggers.rs` | the gameplay math (finite domains, deterministic paths) | T40 review |
| `src/wire.rs`, `sha256.rs`, `fingerprint.rs`, `diag.rs` | serialization support | — |
| `tests/` | determinism, lanes, lod, save_corruption, save_roundtrip, spatial_query_allocations, spine_allocations (THE allocation pins: 18/6880 cold, 9/3024 warm — never move unmeasured), spine_bookkeeping, spine_contracts, spine_history, suspension | the batteries |

**The spine's critical invariants** (read before touching): canonical (due,seq)
pop order; the wire byte-identical; the three fingerprint domains; the
housekeeping allowance (1.5× [ASSUMPTION]); the retention eligibility token; the
class index's lazy materialization (the allocation pins depend on it).

### engine-persist

| Module | What it is | Relevant findings |
|---|---|---|
| `src/pipeline.rs` (~4,500) | CommitAuthority/CommitVerifier, the pending slots, the settled watermark, the shed sets, RetentionStats + the residency estimates (R07) | T13 T14 T29 T30 |
| `src/residency.rs` | the CA-17 stage-1 design record + cold_confirmed | T13 T14 |
| `src/chain_walk.rs` (~560) | the unified walk core (the L9b differential battery pins it) | T29 |
| `src/journal.rs` | the framed journal, the streaming hash (CA-18), the ceiling discipline | T29 T31 |
| `src/local.rs` + `local/*` | the file provider (layout, head_protocol, walks, orphan, reclaim_exec, timings) | — |
| `src/sqlite.rs` + `sqlite/*` | the SQLite provider (feature-gated: `sqlite` default-on; `--no-default-features` = local-only with the typed SqliteNotBuilt refusal) | T33 |
| `src/composite_reader.rs`, `composite_manifest.rs`, `checkpoint.rs`, `reclaim/*`, `dark_reachability.rs`, `recovery*.rs` | the canonical mechanisms | T40 review |
| `src/migrate.rs`, the backup paths | the provider migration + backup | T31 |

**The persist batteries:** `cargo test -p engine-persist` (default) AND
`--features composite-test-fixture` AND `--no-default-features` — all three,
every time you touch this crate.

### engine-renderer

| Module | What it is | Relevant findings |
|---|---|---|
| `src/renderer.rs` | the public API (submit_frame, attach_scene with the CA-01 rollback, RenderStats) | T03 T06 |
| `src/submission.rs`, `frame_plan.rs`, `frame_prepare.rs` | the safe layer (the validated submission, the plan's uses bits + demand counts, the extent-target ensures) | T04 T05 T20 |
| `src/scene_description.rs` | the scene contract (validate, the typed TextureExtent/DrawTable from CA-14 stage 1) | T04 T12 T20 |
| `src/config.rs` | RenderConfig + the budget newtypes + ArenaBudgets::validate (CA-12) + the F-03 region clamp | T04 T17 |
| `src/proxy_instances.rs`, `animation.rs` | the population packing + the pose machinery (the R2 generations; animation.rs is the extraction target T19) | T05 T18 T19 |
| `src/vulkan/context.rs`, `frame.rs`, `pipelines.rs`, `resources.rs`, `swapchain.rs`, `staging.rs`, `capture.rs` | the device layer (the ownership guards, the fence discipline, the staging pool, the fault seams: TargetFaultSite + LifecycleFaultSite + inject_device_loss) | T02 T03 T06 T15 T16 T17 T22 |
| `src/vulkan/passes/*` | corner, proxy_draw, skinned_draw, reflection_draw (the SharedDepthTargets from R5), post_draw, shadow_draw | T03 T16 T24 |
| `src/shaders/` | the WGSL: `core/` (21 shared fragments) + `variants/` (9 shells, composed at build) + the direct-compiled (ground_reflect with the R4 early-out) | T24 T40 review |
| `build.rs` | the composition + the layout pins | — |

**The renderer batteries:** `cargo test -p engine-renderer` debug AND release
(the release count is CI-pinned: 232). The rig batteries (engine-slice's
`#[ignore]`d set) are NOT runnable without the GPU.

### engine-shell

`src/app.rs` (the loop, the bounded interval collector from R05), `input.rs`
(the B08 reuse + the repeat contract), `audio.rs` (the A25/CA-21 lock discipline,
the CA-22 cue backpressure). Relevant: T34. The shell battery:
`cargo test -p engine-shell` (41+ passed, the soak measurement opt-in).

### engine-slice (the composition + demo)

| Module | What it is | Relevant findings |
|---|---|---|
| `src/main.rs` + the demo modules (corner_demo, garage_demo, encounter_demo, combat_demo, playable_demo, persist_demo, render_demo, player_demo, sim_demo) | the routes (the all-routes R2 adoption) | T11 T36 |
| `src/corner_population.rs`, `corner_routes.rs`, `corner_scene.rs` | the corner's population machinery (the dedup mapping — the zero-alloc three-way rule) | T05 T18 |
| `src/region.rs`, `crossing.rs`, `npc_memory_lane.rs`, `session*.rs`, `store.rs`, `backup.rs`, `worker.rs` | the composition layer | T11 T28 T31 |
| `src/census/*` | the P5 measurement harness (repack, attach, ripple, vram, composition — your CPU measurement instruments) | T11 T39 |
| `tests/` | the rig batteries (render_*) + the census batteries + the persist suites (pp_b*) + the lifecycle fault battery | the gates |

### The workflow and CI

`.github/workflows/engine-ci.yml` (~2,350 lines): the main/renderer/heavy pins,
the local-only leg, the result-line counts. **YAML-parse after every edit.**
The heavy command: `cargo test -p engine-slice --features
engine-slice/crash-test-seams` (default profile, NOT --release --ignored —
that's the rig-gates form; the distinction is recorded in git `44da040`).

### The interaction matrix (what touches what — the parallelization guide)

| Finding | Files touched | Conflicts with |
|---|---|---|
| T07 | spine.rs (retention region) | T08/T09 (spine.rs other regions — SEQUENCE or carefully disjoint) |
| T08 | spine.rs (execute_now/drain) | T07/T09 — same file |
| T09 | spine.rs (the lane heaps) + DESIGN | T07/T08 — same file |
| T04 | config.rs, pipelines.rs, frame_prepare.rs, submission.rs | T05 (proxy_instances) is disjoint; T03 (renderer.rs/frame.rs) partially |
| T05 | proxy_instances.rs, renderer.rs (the retained records) | T04 partially, T18 (same machinery) |
| T02 | frame.rs (the stamps) | T03/T06 (frame.rs) — ONE TEAM |
| T03 | renderer.rs, corner.rs, staging.rs | T02/T06/T16 — ONE TEAM |
| T06/T16 | vulkan.rs, frame.rs, staging.rs | T02/T03 — ONE TEAM |
| T19 | the new crate + animation.rs + the consumers | disjoint from everything (the extraction is additive-first) |
| T21 | npc_world_save.rs, world_config.rs | disjoint |
| T10 | player.rs, combat.rs | disjoint |
| T13/T14 | persist pipeline.rs, residency.rs | disjoint from core/renderer |
| T11 | the slice fixtures + census | touches test files broadly — run AFTER the code lanes |

**The rule:** one agent per file-set at a time. The spine trio (T07/T08/T09)
and the renderer lifecycle quartet (T02/T03/T06/T16) each serialize internally.

---

## PART 10 — THE EVIDENCE ARCHIVE (what already exists, for your baselines)

- `Temp\opencode\ee\artifacts\` (if your workspace has the same filesystem —
  otherwise the EVIDENCE.md rows cite the numbers): gates-*.log for every
  landing, the census before/afters, the witness reproductions.
- EVIDENCE.md E-1..E-18: the measured manifest — E-12 (the R0 census), E-16
  (R3's staging), E-17 (R2's dirty-gen), E-18 (R5's aliasing) are your
  before-pictures for the renderer findings.
- The rig-dependent verifications the dawn owes (keep this list current in
  your log): every battery your GPU-deferred lanes name.

*End of the directive. Work the waves. Leave the dawn handoff.*
