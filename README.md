# PostRust

A deterministic game engine in Rust, built around one idea:
**RollerCoaster-Tycoon-class CPU efficiency as a first-class
architectural property** — a fixed-tick simulation core whose per-tick
cost is zero-allocation, cache-disciplined, and proportionate to live
work, with Vulkan presentation on top.

- **Deterministic simulation spine** — a logical campaign clock, a
  `(due, seq)` event queue with cross-class causal dependencies, FNV-1a
  fingerprints, and byte-identical replay: every scenario trace,
  fingerprint, and save round-trip is pinned in CI and compared
  exactly on Linux, Windows, and macOS ARM64.
- **Zero-allocation main path** — gated by a counting-allocator test
  (events *and* bytes, cold and warm windows), not just code review.
- **Production persistence** — a SHA-256 content-addressed revision
  store, a durable hash-chained journal, a commit pipeline with
  canonical confirmation, and crash-recovery proven by real
  process-kill test matrices.
- **Vulkan renderer (ash)** and an SDL3 shell (vendored SDL 3.4.18,
  statically linked) — build-time shader compilation, GPU timestamps,
  p50/p95/p99 frame statistics as first-class output.
- **Exact-count CI gates** — five jobs (Linux, Windows, macOS ARM64)
  with pinned test counts; a count move is an investigation, never a
  silent repin.

## Status — read this first

[`docs/CURRENT-STATUS.md`](docs/CURRENT-STATUS.md) is the maintained,
always-current index of what the engine actually is: integrated
capabilities, verified test coverage, real performance evidence, and
known limitations. The stage-by-stage workspace narrative (Stages 0–5)
is retained as history at
[`docs/ENGINE-WORKSPACE-NARRATIVE.md`](docs/ENGINE-WORKSPACE-NARRATIVE.md);
the architecture blueprint and the decision ledger live at
[`docs/TECH-ARCHITECTURE.md`](docs/TECH-ARCHITECTURE.md) and
[`docs/DECISIONS.md`](docs/DECISIONS.md).

## Layout

| Path | Contents |
|---|---|
| `crates/engine-core` | The deterministic simulation spine (events, clock, fingerprints, player/combat/collision/adversary modules) |
| `crates/engine-persist` | The persistence stack (revision store, journal, commit pipeline, recovery) |
| `crates/engine-renderer` | The Vulkan backend (ash) |
| `crates/engine-shell` | The SDL3 application shell |
| `crates/engine-slice` | The integration/demo driver and test batteries |
| `docs/` | Architecture, decisions, and acceptance records |
| `experiments/` | The pre-engine research harnesses that informed the architecture |

Historical records reference the `engine/` prefix from the
Sektor7-era layout; PostRust was extracted from
[Sektor7](https://github.com/Willpatpost/Sektor7) with full history,
and the workspace now lives at the repository root.

## Running

```bash
cargo test --workspace --exclude engine-slice --features engine-persist/composite-test-fixture  # the main shard
cargo test -p engine-slice --release --features crash-test-seams                              # the heavy battery
cargo run --release -p engine-slice -- --help                                                 # the demo driver
```

The pinned toolchain installs automatically via `rust-toolchain.toml`;
the vendored SDL builds through the normal `cargo build` flow.

## License

Dual-licensed under [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option (`MIT OR Apache-2.0`).
SDL 3.4.18 is vendored and statically linked through the
zlib-licensed `sdl3-src` crate dependency.
