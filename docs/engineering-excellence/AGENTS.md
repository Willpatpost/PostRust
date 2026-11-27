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
| auditor-slice | read-only audit, engine-slice / qwen36-35b | 5b93167 | read-only, no branch | P0 | RUNNING |
| lane-a16 | implementation, ordered confirm / qwen3-coder-next | 00a637c | `Temp\opencode\ee\a16`, `ee/p1-a16-ordered-confirm` | P1-A16 | RUNNING |
| lane-a17 | implementation, rest-transform caching / qwen3-coder-next | 00a637c | `Temp\opencode\ee\a17`, `ee/p1-a17-rest-caching` | P1-A17 | RUNNING |
| lane-b09 | implementation, metal-surface literal / qwen3-coder-next | 00a637c | `Temp\opencode\ee\b09`, `ee/p1-b09-metal-surface` | P1-B09 | RUNNING |

Retired/completed agents stay listed with their final status and evidence link.
