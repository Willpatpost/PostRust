//! §9.1's feature families and §9.6's combined-scene curves — the
//! harness half that needs a DEVICE (the rig runs it; the batteries
//! in `tests/census_vram.rs` and `tests/census_composition.rs`
//! drive this module).
//!
//! THE FAMILIES (§9.1): {dry, +shadows, +post, +wet, +wet+post, the
//! maximal composition} as corner-submission flag sets. Every family
//! renders the SAME populated corner base (the 24+2 crowd — so the
//! per-family deltas are exactly the pass chains' target
//! allocations, attributable by construction); the maximal family
//! is E-9's owner-walked flag set (populated + behavior + shadows +
//! rain + wet + day-night + post + sky — free-look and audio are
//! interactive/presentation-only and stay off the scripted census
//! route, a documented deviation with no residency or timing
//! consequence: the free-look camera and the audio device touch no
//! renderer allocation and no submission byte).
//!
//! THE CURVES (§9.6): [`run_maximal_composition`] runs the maximal
//! route at a given extent through the DEMO'S OWN crowd machinery
//! (`corner_demo::LocomotionCrowd`, `pub(crate)` for exactly this
//! purpose — never a near-copy), printing frame p50/p95/p99/max,
//! GPU render ms, presented count, and validation. Re-run after
//! EVERY lane: the no-regression bar (a small-workload regression
//! cannot hide behind a large-workload win).

use std::time::{Duration, Instant};

use engine_renderer::shadows::ShadowBias;
use engine_renderer::{Renderer, ValidationPreference};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

use crate::census::TimingStats;
use crate::corner_demo::{LocomotionCrowd, corner_submission, demo_bias, route_poses};
use crate::corner_scene::{attach_corner_scene, corner_bounds};
use crate::corner_timing::DEMO_CLOCK_DT_F32;

/// One §9.1 feature family: the corner-submission flag set the
/// census battery warms per extent. `population` is the plain
/// 24+2 crowd for every family (the attributable-delta choice);
/// the maximal family swaps in the behavior+rain crowd (E-9's set).
#[derive(Debug, Clone, Copy)]
pub struct FamilySpec {
    /// The table's row label.
    pub name: &'static str,
    pub shadows: bool,
    /// The plain populated crowd (all families — the constant base).
    pub population: bool,
    pub sky: bool,
    pub post: bool,
    pub wet: bool,
    /// The maximal composition's behavior+rain crowd.
    pub maximal_crowd: bool,
}

impl FamilySpec {
    /// The chains this family activates (the computed-target
    /// columns the battery prints per row).
    pub fn chains(&self) -> (&'static str, &'static str, &'static str) {
        (
            if self.shadows { "shadow" } else { "-" },
            if self.wet { "reflection" } else { "-" },
            if self.post { "post" } else { "-" },
        )
    }
}

/// The §9.1 family axis, in the design's order.
pub const CENSUS_FAMILIES: [FamilySpec; 6] = [
    FamilySpec {
        name: "dry",
        shadows: false,
        population: true,
        sky: false,
        post: false,
        wet: false,
        maximal_crowd: false,
    },
    FamilySpec {
        name: "+shadows",
        shadows: true,
        population: true,
        sky: false,
        post: false,
        wet: false,
        maximal_crowd: false,
    },
    FamilySpec {
        name: "+post",
        shadows: false,
        population: true,
        sky: false,
        post: true,
        wet: false,
        maximal_crowd: false,
    },
    FamilySpec {
        name: "+wet",
        shadows: false,
        population: true,
        sky: false,
        post: false,
        wet: true,
        maximal_crowd: false,
    },
    FamilySpec {
        name: "+wet+post",
        shadows: false,
        population: true,
        sky: false,
        post: true,
        wet: true,
        maximal_crowd: false,
    },
    FamilySpec {
        name: "maximal composition",
        shadows: true,
        population: true,
        sky: true,
        post: true,
        wet: true,
        maximal_crowd: true,
    },
];

/// The E-9 day/night period (the owner walk's `--day-night 20`).
const DAY_NIGHT_PERIOD: f32 = 20.0;

/// One census cell's frame loop outcome — what the batteries print
/// (and what §9.6's row records per extent).
#[derive(Debug, Clone)]
pub struct RouteRow {
    /// The actual swapchain extent (asserted equal to the request).
    pub extent: (u32, u32),
    pub presented: u64,
    /// Wall frame time (submit + present round trip per frame).
    pub frame_ms: TimingStats,
    /// GPU render time (the timestamp query's percentiles).
    pub gpu_ms: Option<TimingStats>,
    pub validation_errors: u64,
    pub validation_warnings: u64,
}

/// Builds the app + renderer + attached corner scene at `extent`
/// (the battery's per-cell construction — one fresh renderer per
/// cell so the monotonic allocation counters read as the live set).
pub fn corner_renderer_at(title: &str, extent: (u32, u32)) -> Result<(ShellApp, Renderer), String> {
    let app = ShellApp::new(ShellConfig {
        title: title.into(),
        width: extent.0,
        height: extent.1,
        target_fps: 60,
        driver: DriverPreference::Real,
    })?;
    let mut renderer = Renderer::new(
        app.window(),
        engine_renderer::RenderConfig {
            validation: ValidationPreference::On,
            demo_scene: engine_renderer::DemoScenePreference::Omitted,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    attach_corner_scene(&mut renderer).map_err(|e| e.to_string())?;
    let actual = renderer.stats().extent;
    if actual != extent {
        return Err(format!(
            "the display gave {actual:?} for the requested {extent:?} — the census row \
             must run at the requested extent (DPI scaling or capability clamp)"
        ));
    }
    Ok((app, renderer))
}

/// The frame loop shared by the census batteries: `warmup` +
/// `measured` submissions of ONE corner frame shape (the family's
/// flags over the advancing route), render-then-step exactly the
/// way the demo route runs, `lag` drain frames for the timestamp
/// queries. Returns the row; every frame must present.
/// `pub(crate)`: takes the demo's own (crate-private) crowd.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_family_frames(
    app: &mut ShellApp,
    renderer: &mut Renderer,
    family: &FamilySpec,
    warmup: usize,
    measured: usize,
    mut crowd: Option<&mut LocomotionCrowd>,
    day_night: Option<f32>,
) -> Result<RouteRow, String> {
    let size = renderer.stats().extent;
    let aspect = size.0 as f32 / size.1 as f32;
    let corner_bounds = corner_bounds();
    let bias: ShadowBias = demo_bias();
    let mut light = engine_renderer::lit_scene::day_preset();
    let mut sky_frame = None;
    let mut poses = route_poses().peekable();
    let mut pose = *poses.peek().unwrap();
    let total = warmup + measured;
    let lag = engine_renderer::config::FRAMES_IN_FLIGHT as usize;
    let mut frame_times: Vec<Duration> = Vec::with_capacity(measured);
    let mut gpu_times: Vec<Duration> = Vec::with_capacity(measured);
    let mut presented: u64 = 0;

    for i in 0..total + lag {
        let frame_start = Instant::now();
        let input = app.frame();
        if input.quit_requested || input.minimized || app.window().size_in_pixels() != size {
            return Err("route interrupted by close, minimize or resize".into());
        }
        // The route's waypoint for this frame (the demo's own
        // pattern: advance inside the measured window, hold the
        // exit pose through the drain — the checked error keeps a
        // mis-sized battery fail-visible instead of panicking).
        if i < total {
            pose = poses
                .next()
                .ok_or("the census route exceeded the scripted route's frames")?;
        }
        // The day/night cycle drives the light AND the sky (the
        // demo route's own wiring, at the E-9 period).
        if let Some(period) = day_night {
            let atmosphere = engine_renderer::sky::atmosphere_at(
                engine_renderer::sky::cycle_position(i as f32 / 60.0, period),
            );
            light = atmosphere.light;
            sky_frame = Some(engine_renderer::SkyFrame {
                palette: atmosphere.sky,
                disc_dir: atmosphere.disc_dir,
            });
        }
        // THE RAIN TICK PRECEDES THE SUBMISSION (finding 18's
        // ordering discipline — the demo route's own shape).
        if let Some(crowd) = crowd.as_deref_mut() {
            let offset = std::array::from_fn(|j| pose.look_at[j] - pose.position[j]);
            let (yaw, _pitch) = crate::corner_demo::waypoint_orientation(offset);
            let rain_t = day_night
                .map(|period| engine_renderer::sky::cycle_position(i as f32 / 60.0, period))
                .unwrap_or(0.0);
            crowd.update_rain(pose.position, yaw, rain_t, DEMO_CLOCK_DT_F32);
        }
        let wet_frame = family.wet.then(|| engine_renderer::WetFrame {
            ripple: crowd
                .as_deref()
                .and_then(|c| c.rain.as_ref().map(|r| r.ripple_field())),
        });
        let frame_population = family
            .population
            .then(|| crowd.as_deref().map(|c| c.population()))
            .flatten();
        let matrix = crate::corner_demo::view_projection(pose, aspect);
        let frame = renderer
            .submit_frame(corner_submission(
                &matrix,
                pose.position,
                light,
                family.shadows.then_some(bias),
                corner_bounds,
                frame_population,
                sky_frame,
                family.post,
                wet_frame,
                size,
            ))
            .map_err(|e| e.to_string())?;
        if !frame.presented || frame.recreated || frame.suboptimal {
            return Err(format!(
                "route presentation interrupted at submission {i}: {frame:?}"
            ));
        }
        presented += 1;
        // THE CROWD'S TICK (render-then-step).
        if let Some(crowd) = crowd.as_deref_mut() {
            crowd.step(DEMO_CLOCK_DT_F32);
        }
        if (warmup..total).contains(&i) {
            let elapsed = frame_start.elapsed();
            frame_times.push(elapsed);
            if let Some(ms) = frame.gpu_render_ms {
                gpu_times.push(Duration::from_secs_f64(ms / 1000.0));
            }
        }
    }
    let stats = renderer.stats();
    if !stats.gpu_timestamps_supported && !gpu_times.is_empty() {
        return Err("timestamps reported without support".into());
    }
    if stats.gpu_timestamps_supported && gpu_times.len() != measured {
        return Err(format!(
            "incomplete GPU timestamp capture: {} of {measured}",
            gpu_times.len()
        ));
    }
    Ok(RouteRow {
        extent: size,
        presented,
        frame_ms: crate::census::timing_stats(&mut frame_times),
        gpu_ms: (!gpu_times.is_empty()).then(|| crate::census::timing_stats(&mut gpu_times)),
        validation_errors: stats.validation_errors,
        validation_warnings: stats.validation_warnings,
    })
}

/// §9.1's per-cell warm loop: the family's frames warmed past both
/// frame slots and every lazy chain build, then the allocation
/// tuple read. Returns the row's presented count for the battery's
/// presentation assertion.
pub fn warm_family(
    app: &mut ShellApp,
    renderer: &mut Renderer,
    family: &FamilySpec,
) -> Result<u64, String> {
    // The maximal family's crowd: E-9's behavior+weather arm (the
    // demo's own constructors — behavior spine + the warm-started
    // rain layer, the rain assigned exactly the way run() does).
    let mut maximal_crowd = family.maximal_crowd.then(|| {
        let mut crowd = LocomotionCrowd::with_behavior();
        crowd.rain = Some(crate::corner_weather::RainLayer::new([-5.0, 1.7, 13.0]));
        crowd
    });
    // The plain families' crowd: the locomotion population without
    // behavior or rain (the constant populated base).
    let mut plain_crowd = (!family.maximal_crowd && family.population).then(LocomotionCrowd::new);
    let crowd = maximal_crowd
        .as_mut()
        .or(plain_crowd.as_mut())
        .map(|crowd| &mut *crowd);
    let day_night = family.maximal_crowd.then_some(DAY_NIGHT_PERIOD);
    let row = run_family_frames(app, renderer, family, 4, 2, crowd, day_night)?;
    Ok(row.presented)
}

/// §9.6's combined-scene curves row: the maximal composition (E-9's
/// flag set) at `extent`, `measured` route frames after warmup.
/// Prints the row and returns it (the battery asserts validation 0
/// and completeness; the rig archives the printed numbers per the
/// E-8 discipline).
pub fn run_maximal_composition(
    extent: (u32, u32),
    warmup: usize,
    measured: usize,
) -> Result<RouteRow, String> {
    println!(
        "P5 §9.6 — the maximal-composition curves at {extent:?} (E-9's flag set; \
         {warmup} warmup + {measured} measured frames)"
    );
    let (mut app, mut renderer) = corner_renderer_at("P5 census composition", extent)?;
    let family = &CENSUS_FAMILIES[5];
    let mut crowd = LocomotionCrowd::with_behavior();
    crowd.rain = Some(crate::corner_weather::RainLayer::new([-5.0, 1.7, 13.0]));
    let row = run_family_frames(
        &mut app,
        &mut renderer,
        family,
        warmup,
        measured,
        Some(&mut crowd),
        Some(DAY_NIGHT_PERIOD),
    )?;
    let walking = crowd.states().iter().filter(|s| s.speed > 0.0).count();
    println!(
        "  population: {} npc + {} vehicle instances ({walking} walking); frame us: \
         median {:.0} / p95 {:.0} / p99 {:.0} / max {:.0}; presented {}; validation {}/{}",
        crowd.instances.len(),
        crowd.vehicles.len(),
        row.frame_ms.median_us,
        row.frame_ms.p95_us,
        row.frame_ms.p99_us,
        row.frame_ms.max_us,
        row.presented,
        row.validation_warnings,
        row.validation_errors
    );
    match &row.gpu_ms {
        Some(gpu) => println!(
            "  gpu render ms: median {:.3} / p95 {:.3} / p99 {:.3} / max {:.3} ({} samples)",
            gpu.median_us / 1000.0,
            gpu.p95_us / 1000.0,
            gpu.p99_us / 1000.0,
            gpu.max_us / 1000.0,
            gpu.samples
        ),
        None => println!("  gpu render ms: timestamps unsupported on this device"),
    }
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The family axis is the design's six, uniquely labeled, the
    /// maximal family carrying every chain (the battery's table
    /// structure leg — device-free).
    #[test]
    fn family_axis_is_the_design_six() {
        assert_eq!(CENSUS_FAMILIES.len(), 6);
        let names: Vec<_> = CENSUS_FAMILIES.iter().map(|f| f.name).collect();
        assert_eq!(
            names,
            vec![
                "dry",
                "+shadows",
                "+post",
                "+wet",
                "+wet+post",
                "maximal composition"
            ]
        );
        let maximal = CENSUS_FAMILIES[5];
        assert!(maximal.shadows && maximal.wet && maximal.post && maximal.maximal_crowd);
        assert_eq!(maximal.chains(), ("shadow", "reflection", "post"));
        // The intermediate families each add exactly their chain.
        assert_eq!(CENSUS_FAMILIES[1].chains(), ("shadow", "-", "-"));
        assert_eq!(CENSUS_FAMILIES[2].chains(), ("-", "-", "post"));
        assert_eq!(CENSUS_FAMILIES[3].chains(), ("-", "reflection", "-"));
        assert_eq!(CENSUS_FAMILIES[4].chains(), ("-", "reflection", "post"));
        assert_eq!(CENSUS_FAMILIES[0].chains(), ("-", "-", "-"));
        // Every family keeps the populated base (the
        // attributable-delta choice).
        assert!(CENSUS_FAMILIES.iter().all(|f| f.population));
    }
}
