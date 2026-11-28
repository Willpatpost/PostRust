# PostRust — Overnight Engineering Log (ODU orchestrator)

Durable public-safe engineering history for the ODU-hosted campaign, per the
owner handoff of 2026-11-28. The orchestrator writes this file; subordinate
agents send lane reports. Local live state: `~/postrust-logs/overnight-state/`.
Commit style: lowercase evidence-carrying records.

---

## 2026-11-28 — session start (orchestrator GLM-5.3, ODU workspace)

**Startup reconciliation (08:42Z):** accepted startup SHA `a2e6e86`
(fast-forward from the clean `057e72f` checkout; remote was 2 ahead, 0 behind;
tree `6e3d67e`; main `5b93167` and tag `pre-excellence-5b93167` verified
unchanged). Single integration writer confirmed; no `ee/*` branches exist on
origin. Registry reconciliation: all six AGENTS.md "RUNNING" rows were stale —
p3-l9b/sdl-pin/p5-r1/p5-r4/q01-q02-q09/q11 all landed (integration commits
`10d3d13`, `6b9fa2a`, `4d77c69`, `d6d7254`, `bce11d3`, `0439894`+`c1624bf`+
`42bf4ef`). The A06 animation extraction (T19/T32) has NOT begun; the prior
"banked reading phase" branch is absent from this clone — the durable base is
`DECISION-A06-animation-crate.md` §4 + the D-EE-10 §2 owner approval.

**Control-plane reading completed:** charter, DECISIONS (D-EE-01..10), strict
audit IV (T01–T40), overnight directive (all parts), registry, A06 decision
record. CI evidence preserved locally: run `46125284171` both heavy-job logs
(`~/postrust-logs/t01-evidence/`).

## T01 — the heavy-shard result-line gate repair (Wave 0)

**The defect, root-caused from git + exact-head CI logs:** run `46125284171`
(exact head `a2e6e86`, both heavy jobs red) reported
`heavy shard: 720 passed, 60 ignored, 72 test-result lines (expected 720 / 60 / 74)`
then `HEAVY SHARD RESULT-LINE MISMATCH: 72 != 64`. Three literals for one
quantity: the echo said 74, the gate enforced 64, the actual was 72 (both
platforms, identical). Git history: the gate was last correctly set to 64 at
`8128ae2` (era 2 package C, when the actual was 64); +8 result lines landed
since (the census/b08/backup/ca09/ca05 batteries and the crash-seams arming
children, mapped at the `44da040` re-measurement which recorded
"716/59/72" but updated only the passed/ignored pins); `057e72f` then updated
ignored 59→60 in the echo and **typo'd the result-line literal to 74** (its
own commit message records the intended "720 / 60 / 72"), leaving the gate at
64. Sibling in the main shard: echo printed a stale 33 while the gate enforced
34 (green in this run only because the actual is 34).

**The repair (branch `ee/overnight-t01-ci-counts`, worktree
`~/PostRust-worktrees/t01`):**

1. Heavy + main shard steps each define ONE authoritative expectation set
   (`expected`, `expected_ignored`, `expected_results`); the echo, all three
   assertions and all failure messages derive from the same variables, so the
   74/64/72 drift class is structurally impossible. Heavy pins 720/60/72
   (dual-platform CI-measured at the exact head); main pins 1629(Linux)/
   1578(Windows) passed, 2 ignored, 34 result lines. Renderer-release
   (232/2), local-only (11) and workspace-inventory (5) gates audited
   internally consistent — left untouched (narrow repair).
2. New deterministic gate-regression fixture
   `scripts/ci-count-gate-selftest.sh` (cargo-free bash/awk/grep): valid
   summary passes; wrong passed/ignored/result-line counts each fail; a
   FAILED result line is excluded from the passed sum (a failing suite
   cannot pass its own count gate); empty/zero summary parses to 0/0/0 and
   fails a nonzero gate; pipefail propagates nonzero cargo exit through tee;
   and workflow-agreement checks verify engine-ci.yml's shard echoes and
   assertions derive from the centralized variables. **Teeth proven:**
   6 failures against the pre-repair workflow, all green against the repair.
3. The fixture wired as a CI step in the main matrix job
   ("CI count-gate self-test") — runs in seconds on every push.
4. Validations: real `yaml.safe_load` parse (pyyaml 6.0.4 in an isolated
   user venv — the system python is PEP-668 managed, no pyyaml
   preinstalled); `bash -n` on all 33 extracted run-blocks; the selftest
   green locally.

**Local reproduction (native Linux, housekeeper lane `t01-ci`, default
profile, the exact CI command
`cargo test -p engine-slice --features engine-slice/crash-test-seams`):**
exit 0, wrapper exit 0, **720 passed / 60 ignored / 72 result lines** —
triple-identical with both CI heavy jobs at the exact head. Log:
`~/postrust-logs/engineering/a2e6e86-t01-heavy-20261128T084352Z.log`.

**The 72-line census (what each result line is):** 68 test targets each
print one result line — the engine-slice lib (173 passed), the
`engine_slice` bin (`src/main.rs` unittests), 65 integration binaries
under `tests/`, and the `Doc-tests engine_slice` line. Two of those
targets — `tests/pp_b3_reclaim.rs` and `tests/pp_b3_rotation.rs` —
spawn crash-test-seams CHILD processes that re-exec the same binary
filtered to one test (each printing its own additional
`test result: ok. 1 passed; … filtered out` line): pp_b3_reclaim 1
child + parent = 2 lines, pp_b3_rotation 3 children + parent = 4 lines.
68 headers − 2 multi-line headers + 6 lines from those two = **72**.
The children's `1 passed` sums into the 720 (the awk totals every ok
line, as CI measured). The historical 64→72 movement: +8 since the
`8128ae2` era — the census/b08/backup-cluster/ca09/ca05 batteries and
the crash-seams arming children mapped at the `44da040` re-measurement.

**Expected CI closure:** one dispatched run at the exact candidate SHA; all
five jobs green including steps after the previously failing check. Only an
exact-head green run closes this gate.

## T38 — the register reconciliation pass (Wave 0, branch `ee/overnight-t38-register`)

The six AGENTS.md "RUNNING" rows and their README/FINDINGS mirrors were all
stale — every named lane landed (verified against git history before any
edit): p5-r1 `4d77c69`, p5-r4 `d6d7254`, p3-l9b `10d3d13`, sdl-pin `6b9fa2a`,
q01-q02-q09 `bce11d3`, q11 `0439894`. Corrected rows, each citing its
integration SHA: A11 (P5 complete R0–R5), A13 (L9b landed), A06 (extraction
lane dispatched — T19), CA-05/CA-06 (fixed at `838a304`, verified against
collision.rs source: the F05 private-fields contract), CA-09 (landed
`30126fa`), NEW-REND-11 (landed `6b9fa2a`), the T01/T19/T38 T-rows, the
README program-state (CI pins 1629/1578/2/34 main, 232/2 renderer-release,
720/720/60/72 heavy; the strict-audit-IV governing mandate added beside the
binding D-EE-10 block; the wave-plan next actions; the stray paragraph
removed), and the AGENTS.md registry (rig orchestrator retired, the ODU
session + t01/t19/t38 lanes registered). The durable maintenance obligation
restated: every landing updates the register rows AND the README together.

## T01 — CLOSED (the exact-head green run, 2026-11-28)

**Run `46127751223` at exact head `9690284`: conclusion SUCCESS — all five
jobs green** (heavy-tests ubuntu + windows, the main matrix ubuntu-latest +
windows-latest + macos-15 ARM64). The previously failing heavy result-line
gate now passes with the corrected centralized pin — both platforms print
`heavy shard: 720 passed, 60 ignored, 72 test-result lines (expected 720 /
60 / 72)` (echo = enforced = actual). The steps AFTER the previously failing
check executed green (the release render-path canary; the heavy job's
post-Tests steps). The new CI count-gate self-test step ran green on every
main job. This is the first fully green engineering-branch run after eight
consecutive failures. Green logs preserved:
`~/postrust-logs/t01-evidence/run46127751223-{ubuntu,windows}-heavy-GREEN.log`.
Any later code change requalifies on its own exact-head run (the docs-only
T38 landing `23c5249` touches no tested surface).

## T07 — the design landed (the retention safe moving cut, S0)

The design lane (`ee/overnight-t07-retention-design`, commit `69d4c0f`, integrated
`4709abf`) delivered `designs/T07-retention-frontier.md`: the two-class eligibility
model (static `ConsequenceFreedom` for permanently-consequence-free worlds; a
per-consumer monotone eligible-prefix frontier for consequence-bearing ones), the
frontier in-memory and never on the canonical wire (byte-identity by construction),
the register rule with typed late-join refusal, observable lag/refusal counters
(never shedding), and logical-head + amortized-compact storage preserving the
`&[ExecutionRecord]` slice contract. Orchestrator verification before acceptance:
docs-only commit; zero production consumers outside spine.rs (grep); the slice
contract + drain + all eight must-switch sites verified line-exact. One
discrepancy noted (41-vs-20 `self.ledger` census — pattern scope; the
implementation re-derives the inventory fresh). Implementation queued behind the
running build lanes, staged per the design (commit 1 the frontier, commit 2 the
logical head), with the four measurement obligations named.

## T19 — the A06 E-S1 extraction landed (`f4ce19b` + the guard companion `d9c3f24`)

The lane (`ee/overnight-t19-animation`, commit `c9700eb`, base `9690284`) delivered
stage E-S1: the new `engine-animation` crate (bytemuck only, `#![deny(unsafe_code)]`)
owning the pure pose/skeleton/sampling core; the GPU packing layer staying
renderer-side; every existing import path stable through the shim (zero consumer
edits; the lib.rs root re-export block byte-stable). Orchestrator verification:
diff scope exact (8 files, no slice/core/shadows/lib.rs edits); the minimal
Cargo.lock delta ratified; gates re-run independently (renderer 211/0/0 measured;
test conservation exact 211+21=232); all four equivalence bars reproduced (pose
digest, instance FNV, rest-identity, mesh bit-identity). One deviation ratified:
SkinnedVertex moved (not kept) — matching the owner-ratified DECISION-A06 §4
verbatim ("SkinnedVertex stays in engine-animation"; the brief had drifted).
The workspace inventory guard corrected 5→6 members (`d9c3f24`, its designed
refusal honored). The CI measurement run `46129087795` dispatched at `d9c3f24`
to measure the pin movements (renderer-release 232→211, main result lines +2
expected); pins will be corrected ONLY from its measured actuals. E-S2 (slice
rewire to `engine_animation::`), E-S3 (shim deletion decision), E-S4 (B15
re-measure) queued.

## T04 — the complete-primitive admission landed (`1e998d4`)

The lane (`ee/overnight-t04-primitives`, red witness `482d469` + fix `15c5307` +
re-pin `9c304cb`) delivered the D-EE-10 §1E owner contract for dynamic geometry:
whole-triangle admission units (no semantic boundaries inferred from vertex
counts), the triangle-floor draw bound (`capacity − capacity mod 3`), malformed-tail
rejection with largest-complete-prefix admission, `admit_layers` as the one pure
admission plan before any upload byte moves, per-layer `LayerShare` records, the
deterministic foreground-then-world complete-unit partition, and the typed
unit+vertex reporting with the exact decomposition. The red witness recorded the
audit's defect verbatim (six failing assertions at the pre-fix tree). The
render_overflow smoke's execution is environment-blocked locally (no Vulkan
loader — evidenced; CI-Linux mesa remains the authority); its new pins derive
from the pinned pure function. CI pins corrected from measured actuals at
`a03f6fb` (main lines 36 both platforms — CI-measured by run `46129087795`;
renderer-release 215 — measured on the combined tree; main passed 1633/1582 —
lane-measured +4 with candidate confirmation). The confirming run `46129831117`
dispatched at `a03f6fb`.

## The T19+T04 candidate verified green (2026-11-28)

**Run `46130487361` at exact head `089741d`: conclusion SUCCESS — all five
jobs green.** The A06 E-S1 extraction and the T04 complete-primitive admission
are CI-verified together: heavy 720/60/72 (unchanged), main 1633 Linux /
1582 Windows / 2 ignored / 36 result lines, renderer-release 215/2, macOS
ARM64, and the count-gate self-test on every main job. The interim runs are
recorded honestly: `46129087795` (the T19-isolated measurement — heavy+macOS
green, the main jobs red only at the correctly-stale pins) and `46129831117`
(the first confirming run — caught the orchestrator's own missed passed-pin
edit, the a03f6fb commit having claimed the 1629→1633/1578→1582 movement in
its message while the sed applied only the result-lines and renderer-release
literals; the gate did exactly what it exists to do; the omission corrected
at `089741d` with the history in the pin's comment).

## T07 — the retention safe moving cut implemented (`c1d9b1a`, pins at `4d17f14`)

The S0 headline finding closed at the mechanism level. The lane executed the
approved design in two staged commits (`6a51ea0` the frontier, `25108d6` the
logical head), spine.rs only. Orchestrator verification: gates re-run
independently (every integration target unchanged, main-path allocation pins
unmoved); both red witnesses captured pre-fix (the standing token cutting
exactly the 30 records a would-be consumer still needed; the drain's memmove
at physical index 100); the fresh ledger census re-derived (the lane catching
its own stage-1 addition `retention_ordinal_of` as a must-switch site). The
measurement obligations discharged: stride asserted **32 B** (the design's
~40 estimate corrected by measurement), amortized bound witnessed across
three retain/chunk regimes with pure zero-move firings proven by exact
physical-index stability, sustained loop zero-allocation, pins
(18,6880)/(9,3024) unmoved. Design reconciliations documented not silent
(register signature, lag formula, zero-moves clause, stride). CI pins
corrected from lane-measured actuals: main 1645/1594 passed (+12), lines 36,
heavy/renderer unchanged; confirming run `46131503857` dispatched.

**T07 CI-verified green:** run `46131503857` at `4d17f14` — all five jobs
success; the main shard 1645/1594 pin arithmetic confirmed on both platforms.

## T05 — the population identity contracts landed (`fd3f7da`, pins at `121aa5d`)

The audit's hypothesis CONFIRMED by four distinct red witnesses captured
pre-fix (the colliding-generation `Reused` against stale bytes on the
release-shaped path; the byte-identical-payload partition change the
byte-only oracle cannot see; the missed-bump debug panic as the sole
tripwire; the slot-level and indirect gates skipping). The fix:
`InstanceSourceToken`/`PoseSourceToken` — renderer-assigned identity
derived from the submission's own borrows, private fields, gate-only
metadata never in packed bytes or digests; token mismatch resets the
half's generation record so the existing R2 gate re-admits; indirect-count
identity separated from payload identity; slot invalidation moved to
submission time; pose-index validation at admission/change (the T18
direction). No caller adaptation; no digest pin moved. The residual
allocator-address-recycling hazard documented with the revision-scoped
`PopulationHandle` as future hardening. CI pins corrected at `121aa5d`
(main 1650/1599/2/37, renderer-release 220/3) — and the pin batch caught
the orchestrator's own near-miss echo/gate drift on the renderer-release
step, prompting the class-level completion: the third shard centralized
under the same variables as heavy/main, the selftest extended to guard it
(20/20 green). The orchestrator's local gate re-run was housekeeper-refused
at 24.45/32 GiB cgroup (page-cache-heavy; no OOM events) — the lane's
recorded gates + diff review + the dispatched exact-head CI run carry the
verification; the local re-run queued for when the memory axis eases.
Confirming run `46133893849` dispatched.

**T05 CI-verified green:** run `46133893849` at `121aa5d` — all five jobs
success; the main 1650/1599/2/37 and renderer-release 220/3 pin arithmetic
confirmed on all platforms.

## The lifecycle cluster design landed (`d1ed4b2`)

`designs/LIFECYCLE-T02-T03-T06-T16.md` (lane `1629d4d`): the four findings
as one defect class — state vouching for device content advancing on call
sequence rather than resource identity. T02 adjudicated
confirmed-at-audit-SHA / refuted-at-head by the T05 mechanism (verified at
the cited lines; the epoch binding lands anyway). New findings beyond the
audit: the FSU release-half latch gap and the textures.rs InlineWait
destination-destruction hazard (verified real). The serialized staged plan
L1–L5 with the CPU/rig split named per stage. The implementation queues as
ONE lane (the audit's one-team mandate) behind the memory ceiling.
