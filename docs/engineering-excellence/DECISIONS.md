# Program Decisions (D-EE-*)

Accepted program-level decisions with rationale. Charter governs; these record
choices the charter leaves open. Irreversible/owner-gated items are NOT decided
here (repository kept, formats, main merges, workflow edits).

| ID | Decision | Rationale |
|---|---|---|
| D-EE-01 | Keep the existing repository and history. | Charter §1 mandates; no technical case exists for a restart. |
| D-EE-02 | Integration = `refactor/engineering-excellence`; lanes on `ee/<package>` branches in worktrees under `C:\Users\Willp\AppData\Local\Temp\opencode\ee\`; lanes merge to integration under orchestrator review; no PRs to main until final owner GO. | Charter §1/§7; worktree isolation avoids overlapping writers; Temp\opencode is pre-approved scratch. |
| D-EE-03 | Gates strategy: lanes run fmt + clippy (touched crates) + brief-specified targeted tests; the orchestrator runs the full workspace gates (fmt, clippy --workspace --all-targets -- -D warnings, main-shard/heavy/renderer batteries) on the integrated tree at each integration point, serially. | Protocol §0.6 requires local gates before push; per-lane full workspace gates would thrash build time; benchmark runs serial per charter §3.1. |
| D-EE-04 | Each lane worktree gets its own `target/` dir; at most 2 concurrent cargo invocations machine-wide; benchmarks serial only. | Shared target dirs across worktrees force full rebuilds (path-dependent package ids) and lock contention; disk is plentiful (3.4 TB); protects timings. |
| D-EE-05 | Charter copied verbatim to `docs/engineering-excellence/DIRECTIVE.md` (harness attachment had no filesystem copy; provenance note inside). | Charter §2: no chat memory as sole authority. |
| D-EE-06 | Test-count pins: if program work adds/removes tests, the `.github/workflows/engine-ci.yml` pins are corrected from measured actuals in batched `ci:` commits at integration points, per the repo's standing law; never a silent or unilateral gate change; final PR discloses every pin delta. | Repo law (CURRENT-STATUS §0) supersedes convenience; charter §8 requires no unilateral workflow edits — pin corrections from measured actuals are the repo's own established, evidence-carrying practice, kept narrow and documented. |
| D-EE-07 | engine-core changes require a brief-scoped determinism argument in the lane report: which byte-packed/fingerprint paths are touched and why digests cannot move; any digest movement = explicit migration entry in the closure report. | Protocol §0.4; charter §4. |
| D-EE-08 | Lavapipe parity pod stays un-rebuilt until a gate actually requires it (cross-platform visual gates or CI-mirror verification); rebuild recipe on record. | ~15 min rebuild + Docker maintenance cost; not needed for P1 lanes whose evidence is CPU-side; pod is currently empty by owner cleanup. |
| D-EE-09 | **Lane branch discipline (the contamination incident, 2026-11-26; narrative CORRECTED 2026-11-27 per the audit's IA-11):** the orchestrator pre-creates every lane branch IN THE WORKTREE before dispatch; implementation agents NEVER run branch-creation commands. THE CORRECTED NARRATIVE: the P3-L1 agent ran its branch switch in the orchestrator's integration checkout (the session cwd default) and made its edits there; the P3-L2 agent switched ITS branch correctly in the a16 worktree but its reclaim.rs edits strayed into the orchestrator checkout as well (per the tool records `msg_21b264a4e001...` and `msg_21b27ffa600...` — the original D-EE-09 text over-generalized to "both agents switched in the orchestrator checkout"). The quarantine preserved all identifiable work (the WIP patches survive at the scratch artifacts with SHA-256 prefixes `5b03b5489847` and `d9fc4ca3085c`; the stray design commit was fast-forwarded onto the integration branch; both lane commits were rebuilt at the correct base). Also standing: lane handoff test counts are re-measured by the orchestrator. | The session-cwd default is an environment property agents cannot be trusted to override; pre-creation by the coordinator is one command and removes the failure mode entirely. |

## D-EE-10 — the owner authorization block (2026-11-27/28, at `083f617`)

The owner ruled on the four pending decisions and added architectural
authorization. Verbatim effect (the full text lives in the session
record; this is the register摘要 with the operative clauses):

1. **P5 resource architecture** — (A) render-target aliasing APPROVED
   CONDITIONALLY (the 10% threshold a heuristic not an eligibility
   gate; evaluate absolute savings, peak residency, resolution
   scaling, sync cost, GPU overlap, shared-memory hardware —
   integrated GPUs IN SCOPE; no fidelity reduction, no unsafe
   overlap, no unnecessary serialization; explicit lifetimes,
   frame-slot ownership, discriminating correctness evidence).
   (B) arenas APPROVED (64 min / pow2 / geometric; the 4096s default
   budgets never permanent limits; configurable; safe growth,
   retirement, saturation, memory-pressure; no oscillation; routine
   tuning owner-unburdened). (C) dirty-generation ADOPTED ACROSS ALL
   APPLICABLE ROUTES (not just the two demos; the full-repack
   fallback + independent reference path retained). (D) staging
   APPROVED (inline-wait acceptable first; the objective reusable
   memory, batched, readiness-tracked, async-safe, bounded
   backpressure; sync follows actual dependencies NOT frame-slot
   fences; attach-time insufficient to dismiss streaming). (E) region
   growth APPROVED (prefer pre-submission when demand known;
   complete-primitive admission; requested/admitted/omitted
   distinct; no silent truncation). (F) full-resolution reflections
   stay DEFAULT (lower-res only as an explicit quality option;
   matched quality in all comparisons).
2. **A06 — extraction APPROVED** (overriding the record's Option 2
   recommendation): a dedicated engine-animation crate, sequenced
   AFTER P5 R1/R2 in the P6 window; pure pose/skeleton/sampling
   separated from GPU packing; closure minimized (bytemuck placement
   evaluated); engine_animation canonical with compat re-exports;
   numerical behavior + fingerprints preserved; re-measured. A06
   closes on implementation + independent verification.
3. **NEW-REND-11 — the exact SDL pin APPROVED** (verify the complete
   resolution contract; demonstrate fresh consumers resolve the
   intended identity; upgrades deliberate).
4. **SQLite feature isolation APPROVED** (feature-gated, ENABLED BY
   DEFAULT; both configurations verified; no weakening — a stronger
   durability tier fails visibly when no eligible provider; measure
   the differences).
5. **A rendering-contract/types surface investigation AUTHORIZED**
   (crate or feature isolation where justified; no crate-count
   theater).
6. **The owner's independent re-audit at `083f617`** (scores
   8.0/8.1/7.2/7.7/8.1/8.4/7.8-provisional/7.6) — substantial but
   NOT acceptance; **scalability not code-complete**; eleven new
   findings Q01-Q11 registered below; A08 regional scheduler
   locality, R07, B08/B13, and the coverage/performance obligations
   retained. Continue autonomously past the queued P5 lanes; every
   category ≥9/10; stop only at the final owner integration gate.
