//! F-09 CLI regressions: `--fps 0` is refused at the top-level mode
//! dispatch with a visible nonzero error directing to
//! `--fps uncapped` — the numeric 0 previously clamped to a silent
//! 1 FPS, so a documented "uncapped" measurement run was actually
//! paced at 1 FPS (the GPU kit and the CI render smokes both carried
//! the defect). These spawn the real binary; the refusal happens
//! during argument parsing, before any SDL/Vulkan or campaign
//! initialization, so the checks are GPU-free and deterministic on
//! every platform.

use std::process::Command;

fn run_binary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_engine-slice"))
        .args(args)
        .output()
        .expect("spawn the engine-slice binary")
}

#[test]
fn render_rejects_fps_zero_directing_to_uncapped() {
    let output = run_binary(&["--render", "--fps", "0"]);
    assert!(
        !output.status.success(),
        "the binary must exit nonzero on --fps 0"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--fps uncapped"),
        "stderr must direct to uncapped: {stderr}"
    );
    assert!(
        stderr.contains("1 FPS"),
        "stderr must state the silent 1 FPS clamp it refuses: {stderr}"
    );
}

#[test]
fn demo_persist_rejects_fps_zero_directing_to_uncapped() {
    // The campaign directory is never touched — the refusal exits
    // during argument parsing, before any campaign or GPU work.
    let output = run_binary(&["--demo-persist", "unused-campaign-dir", "--fps", "0"]);
    assert!(
        !output.status.success(),
        "the binary must exit nonzero on --fps 0"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--fps uncapped"),
        "stderr must direct to uncapped: {stderr}"
    );
    assert!(
        stderr.contains("1 FPS"),
        "stderr must state the silent 1 FPS clamp it refuses: {stderr}"
    );
}

#[test]
fn help_lists_demo_modes_and_exits() {
    let output = run_binary(&["--encounter", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("PostRust engine demos and benchmarks"));
    assert!(stdout.contains("--hud-scale"));
    assert!(!stdout.contains("Type 'help'"));
}

#[test]
fn encounter_rejects_invalid_hud_scale_before_opening_a_window() {
    for value in ["NaN", "0", "5"] {
        let output = run_binary(&["--encounter", "--hud-scale", value]);
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("--hud-scale requires a number"), "{stderr}");
    }
}
