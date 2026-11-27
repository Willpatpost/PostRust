# Agent / Session Registry

Registry per charter §3.2: role, model, base SHA, worktree/branch, package,
status, evidence. Orchestrator = the coordinator session (glm-5.3-w4a8,
main checkout `C:\Users\Willp\Code\GitHub\PostRust` on
`refactor/engineering-excellence`).

| Agent | Role / Model | Base | Worktree / Branch | Package | Status |
|---|---|---|---|---|---|
| orchestrator (this session) | integration coordinator / glm-5.3-w4a8 | 5b93167 | main checkout, `refactor/engineering-excellence` | all | ACTIVE |
| auditor-core | read-only audit, engine-core / MiMo-V2.6-Pro-RL | 5b93167 | read-only, no branch | P0 | DISPATCHED |
| auditor-persist | read-only audit, engine-persist / gpt-oss-120b | 5b93167 | read-only, no branch | P0 | DISPATCHED |
| auditor-renderer | read-only audit, engine-renderer+shaders / gpt-oss-120b | 5b93167 | read-only, no branch | P0 | DISPATCHED |
| auditor-shell | read-only audit, engine-shell / qwen38-27b | 5b93167 | read-only, no branch | P0 | DISPATCHED |
| auditor-slice | read-only audit, engine-slice / qwen36-35b | 5b93167 | read-only, no branch | P0 | DISPATCHED |
| lane-a16 | implementation, ordered confirm / qwen3-coder-next | (set at dispatch) | `Temp\opencode\ee\a16`, `ee/p1-a16-ordered-confirm` | P1-A16 | DISPATCHED |
| lane-a17 | implementation, rest-transform caching / qwen3-coder-next | (set at dispatch) | `Temp\opencode\ee\a17`, `ee/p1-a17-rest-caching` | P1-A17 | DISPATCHED |
| lane-b09 | implementation, metal-surface literal / qwen3-coder-next | (set at dispatch) | `Temp\opencode\ee\b09`, `ee/p1-b09-metal-surface` | P1-B09 | DISPATCHED |

Retired/completed agents stay listed with their final status and evidence link.
