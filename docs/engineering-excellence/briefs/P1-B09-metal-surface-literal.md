# P1-B09 — Metal surface extension literal in `engine-renderer`

**Package:** P1-B09 · **Lane branch:** `ee/p1-b09-metal-surface` ·
**Base:** the `refactor/engineering-excellence` head at worktree creation.
**Role:** focused Rust implementation. **One responsibility:** fix the
platform-surface extension classification defect — nothing else.

## Problem (verified on the baseline tree, 2026-11-26)

`crates/engine-renderer/src/vulkan.rs`, `InstanceExtensionPlan::select`:
`PLATFORM_SURFACE_EXTENSIONS` (line ~1136) contains `"VK_KHR_metal_surface"`
(line ~1142) — **this extension does not exist**; the real macOS window
surface extension is `VK_EXT_metal_surface`
(https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_metal_surface.html).
The rest of the same file already uses the correct literal (lines ~5407, 5424,
5531, 5568, 5630, 5640 — the platform-condition tests).

Effect of the defect: on a macOS SDL windowed instance list advertising
`VK_EXT_metal_surface`, `has_platform_surface` evaluates false, so the
headless-fallback condition (`!has_platform_surface && loader advertises
headless && KHR_surface available`) can inappropriately ADD
`VK_EXT_headless_surface` to a windowed list. (Whether this fires depends on
the loader; the misclassification itself is confirmed.)

## Required outcome

1. Replace the wrong literal with `VK_EXT_metal_surface` in
   `PLATFORM_SURFACE_EXTENSIONS`. Review the remaining entries of that list
   against the Khronos registry for the same class of defect
   (`VK_KHR_display_surface`, `VK_MVK_macos_surface`, `VK_GGP_stream_descriptor_surface`,
   `VK_NN_vi_surface`, `VK_FUCHSIA_imagepipe_surface`, `VK_QNX_screen_surface`,
   win32/xlib/xcb/wayland/android) — report findings; change only verified
   defects.
2. Prefer ash's generated extension-name constants over ad hoc string
   literals **where the existing code style already imports ash names**; if
   the module's convention is string literals (check neighboring code), keep
   the literal fix minimal and consistent — do not restyle the list.
3. Tests: locate the existing platform-condition tests (~lines 5407–5640,
   `VK_EXT_metal_surface` mentions) and confirm they now exercise the fixed
   classification; if a test today encodes the WRONG expectation (metal not
   recognized), correct it to assert the fixed behavior — this is a bug fix,
   not a weakening; document any expectation change in the commit message.
   If coverage lacks a macOS-windowed-list case (metal present → NO headless
   fallback), add ONE focused test in the existing test module following the
   neighboring tests' pattern.
4. No behavior change for Windows/Linux paths (their literals are correct).

## Constraints

- Allowed edit paths: `crates/engine-renderer/src/vulkan.rs` (incl. its test
  module). Forbidden: everything else.
- No new dependencies; no public API change.
- `cargo fmt --all -- --check`; `cargo clippy -p engine-renderer --all-targets
  -- -D warnings`; `cargo test -p engine-renderer` (lib battery; note: the
  renderer release battery is run by the orchestrator at integration).
- No Vulkan hardware needed: `InstanceExtensionPlan::select` is pure logic
  with existing unit tests.

## Handoff (required, structured)

Exact commit SHA + base; changed files; the registry citations for any other
list entry you changed or cleared; commands + exit status; test results;
whether the macOS-windowed case previously misclassified ( reasoned, not
claimed measured — no macOS here); risks; unresolved issues. Do NOT push, do
NOT merge, do NOT touch main. Commit on your lane branch and stop.
