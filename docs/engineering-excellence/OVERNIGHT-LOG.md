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
