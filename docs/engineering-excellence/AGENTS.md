# Agent / Session Registry

Registry per charter §3.2: role, model, base SHA, worktree/branch, package,
status, evidence. Orchestrator = the coordinator session (glm-5.3-w4a8,
main checkout `C:\Users\Willp\Code\GitHub\PostRust` on
`refactor/engineering-excellence`).

| Agent | Role / Model | Base | Worktree / Branch | Package | Status |
|---|---|---|---|---|---|
| orchestrator (this session) | integration coordinator / glm-5.3-w4a8 | 5b93167 | main checkout, `refactor/engineering-excellence` | all | ACTIVE |
| auditor-core | read-only audit, engine-core / MiMo-V2.6-Pro-RL | 5b93167 | read-only, no branch | P0 | RETIRED — wedged without producing anything (owner-observed; the session was formally cancelled afterward, ID `…M3cDpCUOUofd13` — the one my nudges never reached due to the crossed-session bookkeeping error); no calibration data; replaced below |
| auditor-core-2 | read-only audit, engine-core / gpt-oss-120b | (records at report) | read-only, no branch | P0 | COMPLETED — `audits/core.md`; 10 seeds verified (8 CONFIRM / 2 PARTIAL / 1 honestly UNEXAMINED), one label error (B05) adjudicated by direct read; anti-wedge discipline worked |
| auditor-persist | read-only audit, engine-persist / gpt-oss-120b | 5b93167 | read-only, no branch | P0 | COMPLETED — `audits/persist.md`; verdicts reconciled in FINDINGS.md (A13 label corrected) |
| auditor-renderer | read-only audit, engine-renderer+shaders / gpt-oss-120b | 5b93167 | read-only, no branch | P0 | COMPLETED — `audits/renderer.md`; 2 claims refuted + 1 miscount on orchestrator verification (see FINDINGS.md calibration record) |
| auditor-shell | read-only audit, engine-shell / qwen38-27b | 5b93167 | read-only, no branch | P0 | COMPLETED (thorough) — `audits/shell.md` overwritten with the real report; the earlier weak artifact was the misdirected nudge reaching the renderer session (orchestrator session-ID bookkeeping error, corrected in the calibration record) |
| lane-a25 | orchestrator-implemented, audio lock scope / glm-5.3-w4a8 | 6b6544c | commit `fa13e26` → integrated `7c30aee` | P1-A25 | FIXED-AWAITING-REVIEW — copy-out-under-lock, voice-start unheld; shell battery green, no count movement |
| lane-a21 | orchestrator-implemented, bounded diagnostics / glm-5.3-w4a8 | c66c3b1 | commit `3084a8a` → integrated `a213563` | P1-A21 | FIXED-AWAITING-REVIEW — bounded retention + dropped counter; spine allocation pin green; main +3 pin pending measurement |
| design-b07-b05 | design documents, rule-policy + particle domain / glm-5.3-w4a8 (calibration trial) | c66c3b1 | `designs/B07-validated-rule-policy.md`, `designs/B05-particle-domain-contract.md` | P1-B07/B05 | COMPLETED — B07 RESOLVED no-change (evidence verified), B05 implemented by orchestrator from the design (`457d391` → `bceb3fa`, witness corrected); glm-5.3 calibration: EXCELLENT, design work routes there by default |
| design-p2 | spatial/scheduler design / glm-5.3-w4a8 | c7de072 | `designs/P2-simulation-spatial.md` | P2 | RUNNING |
| design-p3 | persistence architecture design / glm-5.3-w4a8 | bceb3fa | `designs/P3-persistence-architecture.md` | P3 | COMPLETED — 943 lines, citations verified; register corrections propagated (A22-persist, B04-persist no-change); lane plan L1–L9 recorded |
| design-p4 | renderer architecture design / glm-5.3-w4a8 | 7961cb2 | `designs/P4-renderer-architecture.md` | P4 | COMPLETED — 1,026 lines; the submission contract + staged DAG S1–S6; caught the audits/renderer.md phantom (re-materialized); L1/L2 landed, L3–L9 queued |
| auditor-slice | read-only audit, engine-slice / qwen36-35b | 5b93167 | read-only, no branch | P0 | COMPLETED — `audits/slice.md`; B04 quote fabricated + consequences overstated, reconciled by orchestrator read (see FINDINGS.md) |
| lane-a16 | implementation, ordered confirm / qwen3-coder-next | 00a637c | lane commit `118e193` → integrated `f9901f3` | P1-A16 | FIXED-AWAITING-REVIEW — integrated; full workspace gates at batch close |
| lane-a17 | implementation, rest-transform caching / qwen3-coder-next | 00a637c | lane commit `3409117` + orchestrator corrective `56205f1` → integrated `3439633` | P1-A17 | FIXED-AWAITING-REVIEW — lane delivered the minor half + an unnecessary once_cell dep and claimed full completion; corrective removed the dep (std OnceLock) and implemented SkinBind/skin_matrices_bound (the primary item); awaits batch gate run |
| lane-b09 | implementation, metal-surface literal / qwen3-coder-next | 00a637c | lane commit `2ddef05` → integrated `9dce061` + orchestrator corrective | P1-B09 | FIXED-AWAITING-REVIEW — integrated with the MVK restoration corrective; full workspace gates at batch close |
| lane-b04 | implementation, flush lifecycle / qwen3-coder-next | ff3978a | lane commit `35211d2` + orchestrator corrective `2f92788` → integrated `e31dc61` | P1-B04 | FIXED-AWAITING-REVIEW — lane's retry paths defective in both halves (corrective: UnresolvedKind + failure-boundary bookkeeping + 2 discriminating tests); heavy pin corrected 653/652 |
| lane-a24 | implementation, fixed timestep / qwen3-coder-next | 3f8691a | lane commits `b1a052b`+`886361c` → integrated `1884582` | P2-A24 | VERIFIED-CLOSED — rig gates green (11/11 render_weather on the RTX 5070 Ti, pinned values digit-for-digit); heavy pin corrected 665/664 |
| lane-p3-l1 | journal read surfaces / qwen3-coder-next → orchestrator-completed | 4fd9683 | commit `98166ec` → integrated `19fe59d` | P3-L1 | FIXED-AWAITING-REVIEW — the contamination incident lane (branch switch in the orchestrator checkout, halted, quarantined, completed by the orchestrator); read_prefix landed, frontier half reverted to L4/L5 |
| lane-p3-l2 | reclaim dedupe / qwen3-coder-next + orchestrator corrective | 4fd9683 | commits `10aaf18`+`cc85bc0` → integrated `c084230` | P3-L2 | FIXED-AWAITING-REVIEW — the inverted closure-dedupe caught at review, fixed with the teeth-proven discriminating test; handoff counts misreported and re-measured |

Retired/completed agents stay listed with their final status and evidence link.
