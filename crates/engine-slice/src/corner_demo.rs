//! One Corner of the City. One deterministic pose per presented frame.
use crate::corner_population::corner_population;
use crate::math::{Mat4, camera_view, mul, percentile, perspective};
use engine_core::timing::TimingSamples;
use engine_renderer::config::FRAMES_IN_FLIGHT;
use engine_renderer::lit_scene::{RouteWaypoint, camera_route, day_preset, night_preset};
use engine_renderer::{
    DemoScenePreference, PresentModePreference, ProxyDrawMode, ProxyPopulation, RenderConfig,
    Renderer, ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig, pace_until};
use std::time::{Duration, Instant};

const STEPS: usize = 120;
const WARMUP: usize = 60;

/// The demo's proxy materials (the caller policy `ProxyPopulation`
/// carries): NPCs with the concrete facade set (broad dim specular
/// — matte clothing), vehicles with the dark stone set (tighter,
/// slightly brighter — paint sheen). Documented choice, not engine
/// policy.
const NPC_MATERIAL: u32 = 1;
const VEHICLE_MATERIAL: u32 = 2;

/// Two seconds per segment at the reference 60 Hz. Uncapped runs present
/// the identical sequence. Shared waypoints appear once, including both ends.
pub fn route_poses() -> impl ExactSizeIterator<Item = RouteWaypoint> {
    let route = camera_route();
    (0..(route.len() - 1) * STEPS + 1).map(move |frame| {
        let start = route[frame / STEPS];
        let Some(end) = route.get(frame / STEPS + 1) else {
            return start;
        };
        let t = (frame % STEPS) as f32 / STEPS as f32;
        let lerp = |a: [f32; 3], b: [f32; 3]| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        RouteWaypoint {
            position: lerp(start.position, end.position),
            look_at: lerp(start.look_at, end.look_at),
        }
    })
}

pub fn view_projection(pose: RouteWaypoint, aspect: f32) -> Mat4 {
    let [x, y, z] = std::array::from_fn(|i| pose.look_at[i] - pose.position[i]);
    let view = camera_view(pose.position, (-x).atan2(-z), y.atan2(x.hypot(z)), 0.);
    mul(&perspective(65_f32.to_radians(), aspect, 0.1, 150.), &view)
}

struct Distribution {
    live: TimingSamples,
    exact_ms: Vec<f64>,
}

impl Distribution {
    fn new(frames: usize) -> Self {
        Self {
            live: TimingSamples::default(),
            exact_ms: Vec::with_capacity(frames),
        }
    }

    fn record(&mut self, ms: f64) {
        self.exact_ms.push(ms);
        self.live.record(Duration::from_secs_f64(ms / 1000.));
    }

    fn report(&self, label: &str) {
        let n = self.exact_ms.len();
        if n == 0 {
            println!("{label}: unavailable (0 samples)");
            return;
        }
        let [p50, p95, p99] = [0.5, 0.95, 0.99].map(|q| self.live.percentile_ms(q));
        println!(
            "live {label} p50/p95/p99 ms (bucket upper bounds, n={n}): {p50:.6} / {p95:.6} / {p99:.6}"
        );
        let [p50, p95, p99] = [0.5, 0.95, 0.99].map(|q| percentile(&self.exact_ms, q));
        println!(
            "exact {label} p50/p95/p99 ms (nearest rank, n={n}): {p50:.6} / {p95:.6} / {p99:.6}"
        );
    }
}

// Read the OS process high-water mark once, after the final submission.
#[cfg(target_os = "linux")]
fn peak_resident_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(windows)]
fn peak_resident_kib() -> Option<u64> {
    let query = format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id());
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &query])
        .output()
        .ok()?;
    let bytes: u64 = std::str::from_utf8(&out.stdout).ok()?.trim().parse().ok()?;
    Some(bytes / 1024)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn peak_resident_kib() -> Option<u64> {
    None
}

/// The demo's shadow-bias policy (documented per the API contract):
/// roughly two texel quanta of constant floor plus a slope term —
/// the corner scene's axis-aligned faces under the day/night lights
/// stay acne-free at the 1024² cascade while the peter-panning stays
/// under one texel on the ground (the equivalence gate's margins
/// measure both).
pub fn demo_bias() -> engine_renderer::shadows::ShadowBias {
    engine_renderer::shadows::ShadowBias {
        constant: 0.0015,
        slope_scale: 0.002,
    }
}

// The four-way union's arg arithmetic: each branch's run() stayed at
// or under clippy's 7 (the 6-E stack's 7 with the 6-E2 proxy-instanced
// flag; 6-F's 5); the UNION carries both feature axes and crosses to
// 8. The flat flag list is the CLI's own shape (one bool per demo
// mode, main.rs passes them straight through); grouping them into a
// config struct would restructure both packages' code inside a merge
// resolution — the render_frame precedent (vulkan.rs) is the allow.
#[allow(clippy::too_many_arguments)]
pub fn run(
    night: bool,
    fps: u32,
    present: PresentModePreference,
    validation: ValidationPreference,
    populated: bool,
    proxy_reference: bool,
    proxy_instanced: bool,
    shadows: bool,
) -> Result<(), String> {
    let mut app = ShellApp::new(ShellConfig {
        title: "One Corner of the City".into(),
        width: 1280,
        height: 720,
        target_fps: fps.max(1),
        driver: DriverPreference::Real,
    })?;
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            present_mode: present,
            validation,
            demo_scene: DemoScenePreference::Corner,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let (npc, vehicle) = if populated {
        corner_population()
    } else {
        (Vec::new(), Vec::new())
    };
    let population = if populated {
        Some(ProxyPopulation {
            npc: &npc,
            npc_material: NPC_MATERIAL,
            vehicle: &vehicle,
            vehicle_material: VEHICLE_MATERIAL,
        })
    } else {
        None
    };
    // The submission mode, in evidence order: the per-object
    // reference (one draw per instance — the pre-6-E shape, the
    // before/after measurement mode), the direct 6-E instanced
    // submission (--proxy-instanced, the indirect path's A/B
    // partner), and — the populated route's DEFAULT since 6-E2 —
    // the indirect submission (one vkCmdDrawIndirect per kind, the
    // instance counts read from GPU memory; the mechanism the
    // deferred GPU-culling package builds on). The default exercises
    // the shipped mechanism at route scale; the renderer API's own
    // default remains the direct instanced mode (the 6-E contract).
    let proxy_mode = if proxy_reference {
        ProxyDrawMode::PerInstance
    } else if proxy_instanced {
        ProxyDrawMode::Instanced
    } else {
        ProxyDrawMode::Indirect
    };
    renderer.set_proxy_draw_mode(proxy_mode);
    // The combination refusal (the union's policy, from the verified
    // three-way resolution): proxy shadows (a populated scene drawn
    // through the shadowed path) are a deferred package on both the
    // population and the 6-F deferral lists, so asking for both is a
    // typed upfront refusal — never a silent mode drop.
    if populated && shadows {
        return Err(
            "--populated and --shadows together is not implemented (proxy shadows are a deferred package)"
                .into(),
        );
    }
    let stats = renderer.stats();
    let size = stats.extent;
    let preset = if night { "night" } else { "day" };
    println!(
        "One Corner of the City | {preset} | {} | Vulkan {} | {}x{} native | {} | present={} | fps={} (0=uncapped) | validation={} | debug_assertions={}",
        stats.gpu_name,
        stats.api_version,
        size.0,
        size.1,
        stats.surface_format,
        stats.present_mode,
        fps,
        stats.validation_active,
        cfg!(debug_assertions)
    );
    if populated {
        let mode_label = match proxy_mode {
            ProxyDrawMode::PerInstance => "per-object reference mode",
            ProxyDrawMode::Instanced => "instanced mode",
            ProxyDrawMode::Indirect => "indirect mode",
        };
        println!(
            "proxy population: {} instances ({} npc + {} vehicle), {} draw calls per frame ({})",
            npc.len() + vehicle.len(),
            npc.len(),
            vehicle.len(),
            if proxy_reference {
                (npc.len() + vehicle.len()) as u32
            } else {
                u32::from(!npc.is_empty()) + u32::from(!vehicle.is_empty())
            },
            mode_label
        );
    }
    if validation == ValidationPreference::On && !stats.validation_active {
        return Err("requested validation layer is unavailable".into());
    }
    if shadows {
        let bias = demo_bias();
        println!(
            "directional shadows: one cascade ({} texels), bias constant {:.4} / slope {:.4}",
            engine_renderer::shadows::SHADOW_MAP_EXTENT,
            bias.constant,
            bias.slope_scale
        );
    }
    let aspect = size.0 as f32 / size.1 as f32;
    let light = if night { night_preset() } else { day_preset() };
    let bias = demo_bias();
    let pace = (fps > 0).then(|| Duration::from_secs_f64(1. / f64::from(fps)));
    let mut poses = route_poses().peekable();
    let count = poses.len();
    let mut pose = *poses.peek().unwrap();
    let measured = WARMUP..WARMUP + count;
    let lag = FRAMES_IN_FLIGHT as usize;
    let mut cpu = Distribution::new(count);
    let mut intervals = Distribution::new(count);
    let mut gpu = Distribution::new(count);
    let mut previous = Instant::now();
    for i in 0..measured.end + lag {
        let start = Instant::now();
        let input = app.frame();
        if input.quit_requested || input.minimized || app.window().size_in_pixels() != size {
            return Err("route interrupted by close, minimize or resize".into());
        }
        if measured.contains(&i) {
            pose = poses.next().unwrap();
        }
        let matrix = view_projection(pose, aspect);
        // Submission selection: the shadowed route (6-F) or the
        // populated route (6-E/6-E2, the mode set above); the
        // combination was refused above (proxy shadows are the
        // deferred package).
        let frame = if shadows {
            renderer.render_corner_shadowed(&matrix, pose.position, light, bias, size)
        } else if let Some(population) = population.as_ref() {
            renderer.render_corner_populated(&matrix, pose.position, light, population, size)
        } else {
            renderer.render_corner(&matrix, pose.position, light, size)
        }
        .map_err(|e| e.to_string())?;
        let end = Instant::now();
        if !frame.presented || frame.recreated || frame.suboptimal {
            return Err(format!(
                "route presentation interrupted at submission {i}: {frame:?}"
            ));
        }
        if populated && frame.geometry.proxy_draw_calls == 0 {
            return Err("populated route drew no proxy calls".into());
        }
        if measured.contains(&i) {
            cpu.record((end - start).as_secs_f64() * 1000.);
            intervals.record((end - previous).as_secs_f64() * 1000.);
        }
        // Query results belong to the previous submission in this slot.
        // Hold the exit pose for two drain submissions outside the CPU capture.
        if (measured.start + lag..measured.end + lag).contains(&i)
            && let Some(ms) = frame.gpu_render_ms
        {
            gpu.record(ms);
        }
        previous = end;
        if let Some(period) = pace {
            pace_until(start + period);
        }
    }
    if renderer.stats().gpu_timestamps_supported && gpu.exact_ms.len() != count {
        return Err(format!(
            "incomplete GPU timestamp capture: {} of {count}",
            gpu.exact_ms.len()
        ));
    }
    let stats = renderer.stats();
    if stats.validation_errors > 0 {
        return Err(format!(
            "{} validation errors: {:?}",
            stats.validation_errors, stats.recent_validation
        ));
    }
    println!(
        "{WARMUP} warmup + {count} route frames + {lag} timestamp drain; validation warnings/errors {}/{}",
        stats.validation_warnings, stats.validation_errors
    );
    let peak = peak_resident_kib().map_or_else(|| "unavailable".into(), |v| format!("{v} KiB"));
    cpu.report("CPU frame work (poll through present, includes Vulkan waits)");
    intervals.report("frame interval (consecutive present returns, includes pacing)");
    gpu.report("GPU render pass");
    println!(
        "peak process resident memory: {peak}; Vulkan allocations: {:?}",
        renderer.allocations()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn route_visits_each_waypoint_and_keeps_its_target_in_frame() {
        let poses: Vec<_> = route_poses().collect();
        assert_eq!(poses.len(), 601);
        assert_eq!(
            poses.iter().step_by(STEPS).copied().collect::<Vec<_>>(),
            camera_route()
        );
        for pose in poses {
            let matrix = view_projection(pose, 16. / 9.);
            let [x, y, z] = pose.look_at;
            let clip: [f32; 4] = std::array::from_fn(|i| {
                matrix[0][i] * x + matrix[1][i] * y + matrix[2][i] * z + matrix[3][i]
            });
            assert!(clip[3] > 0. && clip[2] > 0. && clip[2] < clip[3]);
            assert!(clip[0].abs() < 1e-4 && clip[1].abs() < 1e-4, "{clip:?}");
        }
    }
}
