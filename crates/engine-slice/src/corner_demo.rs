//! One Corner of the City. One deterministic pose per presented frame.
use crate::math::{Mat4, camera_view, mul, percentile, perspective};
use engine_core::timing::TimingSamples;
use engine_renderer::config::FRAMES_IN_FLIGHT;
use engine_renderer::lit_scene::{RouteWaypoint, camera_route, day_preset, night_preset};
use engine_renderer::proxy_instances::{InstanceTransform, ProxyInstance};
use engine_renderer::{
    DemoScenePreference, PresentModePreference, ProxyDrawMode, ProxyPopulation, RenderConfig,
    Renderer, ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig, pace_until};
use std::time::{Duration, Instant};

const STEPS: usize = 120;
const WARMUP: usize = 60;

/// The proxy materials the demo population draws with (the caller
/// policy `ProxyPopulation` carries): NPCs with the concrete facade
/// set (broad dim specular — matte clothing), vehicles with the dark
/// stone set (tighter, slightly brighter — paint sheen). Documented
/// choice, not engine policy.
const NPC_MATERIAL: u32 = 1;
const VEHICLE_MATERIAL: u32 = 2;

/// The demo's deterministic proxy population — a FIXED TABLE (not
/// seeded), hand-authored in the intersection's road-cross frame:
/// NPCs stand on the sidewalk ring (|x| or |z| = 5, between the 4 m
/// road edge and the 6 m building line), vehicles rest in the road
/// lanes clear of the camera route (the route runs z≈0 westbound
/// then x≈0 northbound; the vehicles sit in the opposite lanes).
/// Feet at y = 0.02 — on the ground, off its exact plane (the
/// coplanar underside/ground seam). Pure data, byte-stable; the
/// ladder's next package replaces this with the full 24-NPC +
/// 2-vehicle placement table.
fn demo_population() -> (Vec<ProxyInstance>, Vec<ProxyInstance>) {
    let npc = |pos: [f32; 3], turn: u16, tint: [f32; 3]| ProxyInstance {
        transform: InstanceTransform {
            pos,
            turn,
            scale: 1.0,
        },
        tint,
    };
    let vehicle = |pos: [f32; 3], turn: u16, tint: [f32; 3]| ProxyInstance {
        transform: InstanceTransform {
            pos,
            turn,
            scale: 1.0,
        },
        tint,
    };
    (
        vec![
            // West sidewalk (x = -5), facing the road (+X).
            npc([-5.0, 0.02, -6.0], 0, [0.85, 0.70, 0.55]),
            npc([-5.0, 0.02, -2.0], 0, [0.20, 0.45, 0.75]),
            npc([-5.0, 0.02, 3.0], 0, [0.90, 0.35, 0.20]),
            npc([-5.0, 0.02, 7.0], 0, [0.35, 0.75, 0.30]),
            // East sidewalk (x = 5), facing the road (-X).
            npc([5.0, 0.02, -7.0], 32768, [0.75, 0.30, 0.60]),
            npc([5.0, 0.02, 2.0], 32768, [0.95, 0.80, 0.40]),
            npc([5.0, 0.02, 6.0], 32768, [0.15, 0.55, 0.85]),
            // North sidewalk (z = 5), facing the road (-Z).
            npc([-3.0, 0.02, 5.0], 49152, [0.60, 0.60, 0.65]),
            npc([4.0, 0.02, 5.0], 49152, [0.80, 0.50, 0.30]),
            // South sidewalk (z = -5), facing the road (+Z).
            npc([-8.0, 0.02, -5.0], 16384, [0.45, 0.80, 0.75]),
            npc([1.0, 0.02, -5.0], 16384, [0.70, 0.20, 0.20]),
            // Crossing the west crosswalk, mid-stride north.
            npc([-4.5, 0.02, 0.5], 16384, [0.90, 0.75, 0.60]),
        ],
        vec![
            // Westbound lane (z = +2), stopped short of the cross.
            vehicle([-10.0, 0.0, 2.0], 16384, [0.75, 0.18, 0.15]),
            // Northbound lane (x = +2), north of the cross.
            vehicle([2.0, 0.0, 9.0], 0, [0.15, 0.25, 0.70]),
        ],
    )
}

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

pub fn run(
    night: bool,
    fps: u32,
    present: PresentModePreference,
    validation: ValidationPreference,
    populated: bool,
    proxy_reference: bool,
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
        demo_population()
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
    // The equivalence gate's reference mode (one draw per instance —
    // the pre-6-E submission shape), for before/after measurement;
    // production never sets it.
    renderer.set_proxy_draw_mode(if proxy_reference {
        ProxyDrawMode::PerInstance
    } else {
        ProxyDrawMode::Instanced
    });
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
            if proxy_reference {
                "per-object reference mode"
            } else {
                "instanced mode"
            }
        );
    }
    if validation == ValidationPreference::On && !stats.validation_active {
        return Err("requested validation layer is unavailable".into());
    }
    let aspect = size.0 as f32 / size.1 as f32;
    let light = if night { night_preset() } else { day_preset() };
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
        let frame = match population.as_ref() {
            Some(population) => {
                renderer.render_corner_populated(&matrix, pose.position, light, population, size)
            }
            None => renderer.render_corner(&matrix, pose.position, light, size),
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
