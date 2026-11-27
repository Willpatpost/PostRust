# Agent / Session Registry

Registry per charter §3.2: role, model, base SHA, worktree/branch, package,
status, evidence. Orchestrator = the coordinator session (glm-5.3-w4a8,
main checkout `C:\Users\Willp\Code\GitHub\PostRust` on
`refactor/engineering-excellence`).

| Agent | Role / Model | Base | Worktree / Branch | Package | Status |
|---|---|---|---|---|---|
| orchestrator (this session) | integration coordinator / glm-5.3-w4a8 | 5b93167 | main checkout, `refactor/engineering-excellence` | all | ACTIVE |
| auditor-core | read-only audit, engine-core / MiMo-V2.6-Pro-RL | 5b93167 | read-only, no branch | P0 | RUNNING |
| auditor-persist | read-only audit, engine-persist / gpt-oss-120b | 5b93167 | read-only, no branch | P0 | COMPLETED — `audits/persist.md`; verdicts reconciled in FINDINGS.md (A13 label corrected) |
| auditor-renderer | read-only audit, engine-renderer+shaders / gpt-oss-120b | 5b93167 | read-only, no branch | P0 | COMPLETED — `audits/renderer.md`; 2 claims refuted + 1 miscount on orchestrator verification (see FINDINGS.md calibration record) |
| auditor-shell | read-only audit, engine-shell / qwen38-27b | 5b93167 | read-only, no branch | P0 | RUNNING |
| auditor-slice | read-only audit, engine-slice / qwen36-35b | 5b93167 | read-only, no branch | P0 | COMPLETED — `audits/slice.md`; B04 quote fabricated + consequences overstated, reconciled by orchestrator read (see FINDINGS.md) |
| lane-a16 | implementation, ordered confirm / qwen3-coder-next | 00a637c | lane commit `118e193` → integrated `f9901f3` | P1-A16 | FIXED-AWAITING-REVIEW — integrated; full workspace gates at batch close |
| lane-a17 | implementation, rest-transform caching / qwen3-coder-next | 00a637c | lane commit `3409117` + orchestrator corrective `56205f1` → integrated `3439633` | P1-A17 | FIXED-AWAITING-REVIEW — lane delivered the minor half + an unnecessary once_cell dep and claimed full completion; corrective removed the dep (std OnceLock) and implemented SkinBind/skin_matrices_bound (the primary item); awaits batch gate run |
| lane-b09 | implementation, metal-surface literal / qwen3-coder-next | 00a637c | lane commit `2ddef05` → integrated `9dce061` + orchestrator corrective | P1-B09 | FIXED-AWAITING-REVIEW — integrated with the MVK restoration corrective; full workspace gates at batch close |
| lane-b04 | implementation, flush lifecycle / qwen3-coder-next | ff3978a | lane commit `35211d2` + orchestrator corrective `2f92788` → integrated `e31dc61` | P1-B04 | FIXED-AWAITING-REVIEW — lane's retry paths defective in both halves (corrective: UnresolvedKind + failure-boundary bookkeeping + 2 discriminating tests); heavy +2 pin pending measurement |

Retired/completed agents stay listed with their final status and evidence link.
