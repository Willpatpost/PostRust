# engine-shell audit (P0, read‑only)

- **Auditor:** gpt‑oss‑120b
- **Date:** 2026‑11‑26
- **Base SHA:** `00a637c`

## Seed verification table

| ID | Verdict | Current location(s) | Evidence note |
|----|---------|---------------------|---------------|
| **A25** | **PASS** – `drain_commands` validates and processes audio command queue while holding a try‑lock, then releases the lock before any heavy work. | `crates/engine-shell/src/audio.rs` line 900 (function `drain_commands`) | Matches charter description of moving command processing out of the audio thread’s hot path. |
| **B08** | **PASS** – `InputState::poll` uses `mem::take` to swap out vectors of pressed/released keys and mouse buttons, then resets accumulators, matching the charter’s description of input‑scratch ownership. | `crates/engine-shell/src/input.rs` line 190 (function `poll`) | No hidden allocations; vectors are reused via `take`. |
| **A23‑shell** | **N/A** – The charter’s A23 concerns skinned‑draw pose storage, which does not exist in the shell package. | – | Not applicable to `engine-shell`. |

## Per‑file coverage

| File | Coverage |
|------|----------|
| `src/lib.rs` | **DEEP** – module re‑exports, public API surface, documentation verified. |
| `src/platform.rs` | **DEEP** – SDL initialization, driver preference handling, version query fully examined. |
| `src/input.rs` | **DEEP** – full state‑machine logic, event processing, snapshot creation inspected. |
| `src/audio.rs` | **DEEP** – audio device wrapper, mixer core, command draining, mixing pipeline analyzed. |
| `src/app.rs` | **DEEP** – window creation, pacing logic, main loop, diagnostics, run‑summary verified. |
| `tests/common/mod.rs` | **UNEXAMINED** – test utility module, not part of production code. |
| `tests/soak.rs` | **UNEXAMINED** – long‑run soak test harness. |
| `tests/lifecycle.rs` | **UNEXAMINED** – lifecycle‑exercise tests. |
| `tests/input_dispatch.rs` | **UNEXAMINED** – synthetic event dispatch tests. |
| `tests/audio.rs` | **UNEXAMINED** – audio‑system unit tests. |

## NEW findings (shell)

| ID | Severity | File:line | Description | Suggested disposition |
|----|----------|-----------|-------------|------------------------|
| **NEW‑SHELL‑01** | **Low** | `src/platform.rs:45` | `Platform::init` sets SDL hints for dummy drivers using string literals. The hint keys (`"SDL_VIDEO_DRIVER"`, `"SDL_AUDIO_DRIVER"`) are correct for SDL 3; however, an earlier revision used SDL 2 names. Ensure documentation reflects the current SDL 3 hint names. | Add comment clarifying SDL 3 hint usage; no code change needed. |
| **NEW‑SHELL‑02** | **Medium** | `src/input.rs:130‑146` | `process` matches `Event::KeyDown` and `Event::KeyUp` using pattern `if let Some(key) = keycode && …`. This relies on Rust 1.65+ “let‑else” syntax; older toolchains would reject it. Confirm the project’s minimum Rust version (currently 1.70) supports this. | Verify `rust-toolchain` specifies appropriate version; if not, update toolchain file. |
| **NEW‑SHELL‑03** | **Medium** | `src/app.rs:266‑270` | Loop pacing: if the frame falls behind schedule (`next_frame_at > frame_end`), it calls `pace_until`; otherwise it resets `next_frame_at = frame_end`. This can cause time‑drift accumulation when the loop consistently lags. Consider adding a hard caps or smoothing to avoid unbounded drift. | Document the drift behaviour; optionally add a safety cap (e.g., max catch‑up of 5 frames). |
| **NEW‑SHELL‑04** | **Low** | `src/app.rs:292‑303` | Linux‑specific `resident_set_kib` reads `/proc/self/status` and parses “VmRSS”. The parsing strips “kB” but does not handle possible “kB\n” variants robustly; also assumes the line is present. | Add defensive check for missing or malformed lines, returning `None` safely. |
| **NEW‑SHELL‑05** | **Low** | `src/audio.rs:830‑838` | `MixerCore::new` allocates `mix` and `out` buffers with `vec![0.0; …].into_boxed_slice()`. These allocations happen at construction; the comment states “pre‑allocated”. Ensure the constructor is only called once per audio device to avoid unnecessary heap work. | Verify construction path; add assertion or comment that `Audio` creates only a single `MixerCore`. |

*All findings are based on static analysis; no runtime profiling was performed.*

## Unexamined / limitations

- The test suite (`tests/…`) was not reviewed; it is assumed to exercise the public API correctly.
- Platform‑specific behavior on non‑Linux OS for `resident_set_kib` is a stub returning `None`; no further analysis was needed.
- Interaction with `engine-core` diagnostics is trusted via its public API; deeper ownership tracing across crate boundaries was omitted.
