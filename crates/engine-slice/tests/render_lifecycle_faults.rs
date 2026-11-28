//! THE CA-01/CA-10/CA-11 LIFECYCLE FAULT BATTERY (the renderer
//! lifecycle/sync trio — the Codex full-engine audit's
//! highest-severity cluster): the three injected-failure
//! discriminating tests the fix lane's brief mandated, driven
//! through the renderer's public fault-injection seam
//! ([`Renderer::inject_lifecycle_fault`] — the target-fault
//! machinery's own one-shot plan/ordinal discipline, extended to
//! the lifecycle steps).
//!
//! - **CA-01** (`attach_failures_are_atomic_and_retryable`): a
//!   failure injected after EVERY creation step in the attach path
//!   (the five corner-resource steps, the eight pipeline-group
//!   steps) must leave the renderer exactly as before — no handle
//!   published, the staged pipeline objects released (the census
//!   pair equal), the RETRY of the same description succeeding and
//!   rendering validation-clean. The pre-fix defect: the retry was
//!   REFUSED ("a scene is already attached") with the resources
//!   installed behind a `None` scene.
//! - **CA-10** (`post_acquire_failures_release_the_slot`): a
//!   failure injected at each of the four post-acquire boundaries
//!   (upload, present-semaphore, record, submit) must never strand
//!   the frame slot — the immediate retries present with NO
//!   2-second fence timeout (the strand's signature), and the
//!   release's empty submit + image return stay validation-clean.
//! - **CA-11** (`growth_idle_failures_release_the_staged_carve`):
//!   a failure injected at each arena growth's idle window (after
//!   the new carve's allocation, before the old carve's
//!   retirement) must release the staged replacement (the census
//!   pair equal, the old bindings intact, the capacity unchanged)
//!   and the retry must grow and land.
//! - **CA-13** (`device_loss_is_typed_and_latched` — the
//!   CA-12..CA-15 config/scene lane's FAULT-SEAM EXTENSION of this
//!   battery): an injected `VK_ERROR_DEVICE_LOST` at the acquire
//!   site must surface as the TYPED
//!   `RendererError::DeviceLost` (the operation context riding the
//!   variant), and the renderer's latch must refuse every later
//!   frame/attach/capture entry WITHOUT touching the dead device
//!   (the unarmed second submit is the proof: reaching the device
//!   would present a real frame).
//!
//! `#[ignore]`d like the binary's other lavapipe/rig gates: it
//! requires a real Vulkan device and windows. THE RIG RUN (the
//! E-8 discipline — release, serial, archived output):
//!
//! ```sh
//! cargo test -p engine-slice --test render_lifecycle_faults --release -- \
//!     --ignored --nocapture --test-threads=1
//! ```

use std::time::Instant;

use engine_renderer::LifecycleFaultSite;
use engine_renderer::lit_scene::day_preset;
use engine_renderer::scene::{SceneVertex, push_box};
use engine_renderer::{ProxyPopulation, RenderConfig, Renderer, ValidationPreference};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};
use engine_slice::census::repack::synthetic_population;
use engine_slice::corner_demo::{corner_submission, route_poses, view_projection};
use engine_slice::corner_scene::{attach_corner_scene, corner_bounds};

fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// One fresh rig renderer, NO scene attached yet (the CA-01 leg
/// attaches — and retries the attach — itself; the CA-10/CA-11 legs
/// attach through the same public step). Validation ON: every leg
/// asserts the release paths stay validation-clean, which is the
/// legality proof for the empty release submission and the
/// no-wait image return.
fn lifecycle_renderer(title: &str) -> (ShellApp, Renderer) {
    lifecycle_renderer_with(title, RenderConfig::default())
}

/// The configured construction (the CA-11 region leg's own cap —
/// see the leg below).
fn lifecycle_renderer_with(title: &str, config: RenderConfig) -> (ShellApp, Renderer) {
    let app = ShellApp::new(ShellConfig {
        title: title.into(),
        width: 256,
        height: 256,
        target_fps: 60,
        driver: DriverPreference::Real,
    })
    .expect("the rig window constructs");
    let renderer = Renderer::new(
        app.window(),
        RenderConfig {
            demo_scene: engine_renderer::DemoScenePreference::Omitted,
            validation: ValidationPreference::On,
            ..config
        },
    )
    .expect("the rig renderer constructs");
    (app, renderer)
}

/// The battery's camera (the route's first waypoint at the window's
/// own aspect).
fn lifecycle_matrix(size: (u32, u32)) -> engine_slice::math::Mat4 {
    view_projection(
        route_poses()
            .next()
            .expect("the route has a first waypoint"),
        size.0 as f32 / size.1 as f32,
    )
}

/// Submits one plain scene-backed corner frame (no population — the
/// full phase ladder, none of the populated families).
fn submit_plain_scene_frame(
    renderer: &mut Renderer,
    matrix: &engine_slice::math::Mat4,
    size: (u32, u32),
) -> engine_renderer::FrameResult {
    renderer
        .submit_frame(corner_submission(
            matrix,
            [0.0, 1.7, 15.0],
            day_preset(),
            None,
            corner_bounds(),
            None,
            None,
            false,
            None,
            size,
        ))
        .expect("the plain scene frame renders")
}

/// Submits one populated corner frame at `instances`/`blocks` (the
/// census battery's own shape — deterministic, the repack's own
/// validation the proof).
fn submit_population(
    renderer: &mut Renderer,
    matrix: &engine_slice::math::Mat4,
    instances: &[engine_renderer::proxy_instances::ProxyInstance],
    blocks: &[[[[f32; 4]; 4]; engine_renderer::animation::HUMANOID_JOINTS]],
    size: (u32, u32),
) -> engine_renderer::FrameResult {
    let population = ProxyPopulation {
        instance_generation: 0,
        pose_generation: 0,
        npc: instances,
        npc_poses: blocks,
        npc_material: 1,
        vehicle: &[],
        vehicle_material: 2,
        rain: &[],
        rain_material: 0,
    };
    renderer
        .submit_frame(corner_submission(
            matrix,
            [0.0, 1.7, 15.0],
            day_preset(),
            None,
            corner_bounds(),
            Some(population),
            None,
            false,
            None,
            size,
        ))
        .expect("the populated frame renders")
}

/// CA-01: EVERY creation step in the attach path, injected
/// individually — the failed attach must leave the renderer exactly
/// as before (no handle, the staged pipeline objects released, the
/// retry clean), and the retry must succeed and render. One fresh
/// renderer per site (a successful attach consumes the one-scene
/// budget by design).
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn attach_failures_are_atomic_and_retryable() {
    let _guard = sdl_lock();
    let attach_sites = [
        // The corner-resource half (CornerResources' own build).
        LifecycleFaultSite::SceneVertices,
        LifecycleFaultSite::SceneTexture,
        LifecycleFaultSite::SceneSampler,
        LifecycleFaultSite::SceneUniforms,
        LifecycleFaultSite::SceneDescriptors,
        // The pipeline-group half (the staged CornerPipelineGroup).
        LifecycleFaultSite::LitModule,
        LifecycleFaultSite::LitLayout,
        LifecycleFaultSite::LitPipeline,
        LifecycleFaultSite::ProxyModule,
        LifecycleFaultSite::ProxyLayout,
        LifecycleFaultSite::ProxyPipeline,
        LifecycleFaultSite::RainModule,
        LifecycleFaultSite::RainPipeline,
    ];
    for site in attach_sites {
        let (mut app, mut renderer) = lifecycle_renderer("CA-01 attach fault");
        let _ = app.frame();
        let size = app.window().size_in_pixels();
        let matrix = lifecycle_matrix(size);
        // The failed attach: the injected fault fires with the step's
        // objects created, so the rollback must clean them up.
        renderer.inject_lifecycle_fault(site, 1);
        match attach_corner_scene(&mut renderer) {
            Err(engine_renderer::RendererError::InjectedFault(_)) => {}
            other => panic!("{site:?}: expected the injected attach refusal, got {other:?}"),
        }
        // No handle published — the renderer reports no scene.
        assert!(
            renderer.scene().is_none(),
            "{site:?}: the failed attach published no handle"
        );
        // The staged pipeline objects all released — the census
        // pair equal (created then released). For the
        // corner-resource sites the group never ran, so the pair
        // reads 0/0; for the pipeline sites every object the
        // partial group created was released by the scoped owner.
        let allocations = renderer.allocations();
        assert_eq!(
            allocations.corner_pipeline_objects, allocations.corner_pipeline_object_frees,
            "{site:?}: a failed attach releases every staged pipeline object"
        );
        // THE RETRY (the discriminating assertion — the pre-fix
        // renderer refused it with \"a scene is already attached\"):
        // the same description attaches cleanly on the same
        // renderer.
        let handle = attach_corner_scene(&mut renderer)
            .unwrap_or_else(|error| panic!("{site:?}: the retry attach must succeed: {error:?}"));
        // The corner table's own material count (the population
        // validation bound — the handle is the real one).
        assert_eq!(
            handle.material_count as usize,
            engine_renderer::lit_scene::materials().len(),
            "{site:?}: the retried attach's handle"
        );
        assert!(renderer.scene().is_some(), "{site:?}: the handle is live");
        // The retried renderer RENDERS: three scene-backed frames
        // (past both frame slots), validation-clean — the group
        // rebuild is complete (a partial group would bind a null
        // pipeline, the S6 rig-found access-violation class).
        for _ in 0..3 {
            let outcome = submit_plain_scene_frame(&mut renderer, &matrix, size);
            assert!(outcome.presented, "{site:?}: the retried frame presents");
        }
        let stats = renderer.stats();
        assert_eq!(
            stats.validation_errors, 0,
            "{site:?}: {:?}",
            stats.recent_validation
        );
        drop(renderer);
        drop(app);
        println!("CA-01 {site:?}: refused, rolled back, retried, rendered");
    }
}

/// CA-10: a failure at EACH post-acquire boundary (upload,
/// present-semaphore, record, submit) must never strand the frame
/// slot. The retry and the two frames after it (which revisit the
/// failed slot — FRAMES_IN_FLIGHT is 2) must present with NO
/// 2-second fence timeout: the pre-fix strand's signature was the
/// NEXT frame on the slot blocking the full WAIT_TIMEOUT_NS and
/// failing with the slot-fence timeout error. The validation-clean
/// assertion is the legality proof for the release's empty
/// submission (the acquire-semaphore consume, the fence heal) and
/// the no-wait image return.
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn post_acquire_failures_release_the_slot() {
    let _guard = sdl_lock();
    let boundaries = [
        LifecycleFaultSite::FrameUpload,
        LifecycleFaultSite::FramePresentSemaphore,
        LifecycleFaultSite::FrameRecord,
        LifecycleFaultSite::FrameSubmit,
    ];
    for site in boundaries {
        let (mut app, mut renderer) = lifecycle_renderer("CA-10 frame fault");
        let _ = app.frame();
        let size = app.window().size_in_pixels();
        let matrix = lifecycle_matrix(size);
        attach_corner_scene(&mut renderer).expect("the scene attaches");
        // Warm past both frame slots so every boundary's failure
        // lands on a slot with real previous submissions.
        for _ in 0..3 {
            let outcome = submit_plain_scene_frame(&mut renderer, &matrix, size);
            assert!(outcome.presented, "the warm frame presents");
        }
        // The failed frame: the boundary's injected fault.
        renderer.inject_lifecycle_fault(site, 1);
        let failed = renderer.submit_frame(corner_submission(
            &matrix,
            [0.0, 1.7, 15.0],
            day_preset(),
            None,
            corner_bounds(),
            None,
            None,
            false,
            None,
            size,
        ));
        match failed {
            Err(engine_renderer::RendererError::InjectedFault(_)) => {}
            other => panic!("{site:?}: expected the injected frame refusal, got {other:?}"),
        }
        // THE RETRY plus the two frames after it: all three present
        // (the third revisits the failed slot), and the whole retry
        // sequence stays well under the 2-second wait bound — the
        // strand would cost exactly one WAIT_TIMEOUT_NS (2 s) at the
        // first revisit and fail the frame with the slot-fence
        // timeout.
        let retry_started = Instant::now();
        for frame in 0..3 {
            let outcome = submit_plain_scene_frame(&mut renderer, &matrix, size);
            assert!(
                outcome.presented,
                "{site:?}: retry frame {frame} presents (the slot is reusable)"
            );
        }
        let retry_elapsed = retry_started.elapsed();
        assert!(
            retry_elapsed.as_millis() < 2_000,
            "{site:?}: the retry sequence took {retry_elapsed:?} — the \
             2-second slot-fence timeout is the strand's signature"
        );
        let stats = renderer.stats();
        assert_eq!(
            stats.validation_errors, 0,
            "{site:?}: the release (empty submit + image return) is validation-clean: {:?}",
            stats.recent_validation
        );
        drop(renderer);
        drop(app);
        println!("CA-10 {site:?}: failed, released, retried in {retry_elapsed:?} (3 frames)");
    }
}

/// CA-11: a failure at EACH arena growth's idle window (after the
/// new carve's allocation, before the old carve's retirement) must
/// release the staged replacement — the census pair equal (staged
/// then released, the pre-fix leak invisible to every behavioral
/// observable), the old bindings intact (the capacity unchanged,
/// no growth event), and the retry must grow and land. Two failed
/// attempts per path (re-armed), then the clean retry.
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn growth_idle_failures_release_the_staged_carve() {
    let _guard = sdl_lock();

    // ---- The REGION carve (the layers-only growth path; the census
    // leg 3's own configuration — the 500-vertex budget so the
    // oversize demand lands ON the cap, the at-cap discipline) ----
    {
        let (mut app, mut renderer) = lifecycle_renderer_with(
            "CA-11 region growth fault",
            RenderConfig {
                region_capacity: engine_renderer::RegionCapacity(500),
                ..RenderConfig::default()
            },
        );
        let _ = app.frame();
        let size = app.window().size_in_pixels();
        attach_corner_scene(&mut renderer).expect("the scene attaches");
        let small_region: Vec<SceneVertex> = (0..100)
            .map(|i| SceneVertex {
                pos: [i as f32 * 0.1, 0.0, 0.0],
                col: [0.2, 0.45, 0.85],
            })
            .collect();
        let outcome = renderer
            .render_layers(
                &lifecycle_matrix(size),
                &small_region,
                None,
                &[],
                None,
                size,
            )
            .expect("the small region frame renders");
        assert!(outcome.presented);
        assert_eq!(renderer.stats().region_capacity_vertices, 128);
        // The oversize supply that demands the growth (17 boxes ×
        // 36 verts = 612 — the census leg's own arithmetic).
        let mut oversize_region = Vec::new();
        for index in 0..17_u32 {
            push_box(
                &mut oversize_region,
                [
                    -8.0 + (index % 6) as f32 * 2.0,
                    0.0,
                    -8.0 + (index / 6) as f32 * 2.0,
                ],
                0.5,
                1.0,
                [0.85, 0.45, 0.20],
            );
        }
        assert_eq!(oversize_region.len(), 612, "17 boxes x 36 verts");
        // TWO failed growths (re-armed per attempt — the plan is
        // one-shot): each stages a replacement then fails the idle
        // wait; the staged pair must be released every time.
        for attempt in 0..2 {
            renderer.inject_lifecycle_fault(LifecycleFaultSite::RegionGrowthIdle, 1);
            match renderer.render_layers(
                &lifecycle_matrix(size),
                &oversize_region,
                None,
                &[],
                None,
                size,
            ) {
                Err(engine_renderer::RendererError::InjectedFault(_)) => {}
                other => {
                    panic!("region attempt {attempt}: expected the injected refusal, got {other:?}")
                }
            }
            let stats = renderer.stats();
            assert_eq!(
                stats.region_capacity_vertices, 128,
                "region attempt {attempt}: the old carve is intact"
            );
            assert_eq!(
                stats.arena_growth_events, 0,
                "region attempt {attempt}: the growth did not land"
            );
            let allocations = renderer.allocations();
            assert_eq!(
                allocations.arena_replacements, allocations.arena_replacement_frees,
                "region attempt {attempt}: every staged replacement was released (the leak proof)"
            );
        }
        // The clean retry: the growth lands, the carve reaches the
        // budget cap, the frame presents.
        let outcome = renderer
            .render_layers(
                &lifecycle_matrix(size),
                &oversize_region,
                None,
                &[],
                None,
                size,
            )
            .expect("the retried oversize frame renders");
        assert!(outcome.presented);
        let stats = renderer.stats();
        assert_eq!(
            stats.region_capacity_vertices, 500,
            "the region carve grew to the configured cap on the retry"
        );
        assert_eq!(stats.arena_growth_events, 1, "the growth event landed");
        assert_eq!(
            renderer.allocations().arena_replacements,
            renderer.allocations().arena_replacement_frees + 1,
            "the committed replacement is the one live staged pair"
        );
        assert_eq!(stats.validation_errors, 0, "{:?}", stats.recent_validation);
        drop(renderer);
        drop(app);
        println!("CA-11 RegionGrowthIdle: failed twice, released twice, grew on the retry");
    }

    // ---- The INSTANCE and POSE carves (the populated growth
    // paths — one battery leg for both, the census scaling rows'
    // own shape: small population carves 128, the grown population
    // demands 512) ----
    {
        let (mut app, mut renderer) = lifecycle_renderer("CA-11 populated growth fault");
        let _ = app.frame();
        let size = app.window().size_in_pixels();
        let matrix = lifecycle_matrix(size);
        attach_corner_scene(&mut renderer).expect("the scene attaches");
        let small = synthetic_population(100);
        let small_poses: Vec<_> = (0..100)
            .map(|_| engine_renderer::animation::rest_pose_block())
            .collect();
        for _ in 0..3 {
            let outcome = submit_population(&mut renderer, &matrix, &small, &small_poses, size);
            assert!(outcome.presented);
        }
        let stats = renderer.stats();
        assert_eq!((stats.instance_capacity, stats.pose_capacity), (128, 128));
        assert_eq!(stats.validation_errors, 0, "{:?}", stats.recent_validation);
        // The grown population (300 instances / 300 blocks — the
        // census leg's own demand, both carves must grow to 512).
        let mut grown = synthetic_population(300);
        for (i, instance) in grown.iter_mut().enumerate() {
            instance.pose_index = (i % 300) as u32;
        }
        let grown_poses: Vec<_> = (0..300)
            .map(|_| engine_renderer::animation::rest_pose_block())
            .collect();
        // The instance growth's idle failure first: the INSTANCE
        // carve's ensure runs first in the populated ladder, so an
        // instance-idle fault fires there — the staged replacement
        // must be released and nothing live added (the pair equal).
        // NOTE the stats discipline: `RenderStats`' capacity fields
        // refresh only on SUCCESSFUL frames (`finish_frame`), so the
        // mid-failure proofs are the error shape and the allocation
        // census pair (the LIVE context counters); the landed-growth
        // checks live after the successful retry below.
        renderer.inject_lifecycle_fault(LifecycleFaultSite::InstanceGrowthIdle, 1);
        match renderer.submit_frame(corner_submission(
            &matrix,
            [0.0, 1.7, 15.0],
            day_preset(),
            None,
            corner_bounds(),
            Some(ProxyPopulation {
                instance_generation: 0,
                pose_generation: 0,
                npc: &grown,
                npc_poses: &grown_poses,
                npc_material: 1,
                vehicle: &[],
                vehicle_material: 2,
                rain: &[],
                rain_material: 0,
            }),
            None,
            false,
            None,
            size,
        )) {
            Err(engine_renderer::RendererError::InjectedFault(_)) => {}
            other => panic!("instance growth: expected the injected refusal, got {other:?}"),
        }
        {
            let allocations = renderer.allocations();
            assert_eq!(
                allocations.arena_replacements, allocations.arena_replacement_frees,
                "the staged instance replacement was released (nothing live staged)"
            );
        }
        // The pose growth's idle failure: this same grown frame
        // retries the instance growth first (clean — no plan armed
        // for it, so it COMMITS and stays live) and the pose fault
        // fires at the pose ensure below it. The pair arithmetic
        // distinguishes the outcomes: two staged-and-released (the
        // failed instance attempt, the failed pose attempt) plus ONE
        // live committed (the instance replacement) — a pose growth
        // that had wrongly committed would make it +2.
        renderer.inject_lifecycle_fault(LifecycleFaultSite::PoseGrowthIdle, 1);
        match renderer.submit_frame(corner_submission(
            &matrix,
            [0.0, 1.7, 15.0],
            day_preset(),
            None,
            corner_bounds(),
            Some(ProxyPopulation {
                instance_generation: 0,
                pose_generation: 0,
                npc: &grown,
                npc_poses: &grown_poses,
                npc_material: 1,
                vehicle: &[],
                vehicle_material: 2,
                rain: &[],
                rain_material: 0,
            }),
            None,
            false,
            None,
            size,
        )) {
            Err(engine_renderer::RendererError::InjectedFault(_)) => {}
            other => panic!("pose growth: expected the injected refusal, got {other:?}"),
        }
        {
            let allocations = renderer.allocations();
            assert_eq!(
                allocations.arena_replacements,
                allocations.arena_replacement_frees + 1,
                "two staged-and-released attempts plus the ONE live committed \
                 instance replacement"
            );
        }
        // The clean retry: both carves at 512, the frame presents,
        // validation-clean — and exactly TWO growth events landed in
        // total (the instance growth on the pose-fault frame, the
        // pose growth here; a growth that had wrongly landed on a
        // failed frame would count three).
        let outcome = submit_population(&mut renderer, &matrix, &grown, &grown_poses, size);
        assert!(outcome.presented);
        let stats = renderer.stats();
        assert_eq!(
            (stats.instance_capacity, stats.pose_capacity),
            (512, 512),
            "both carves grew on the retry"
        );
        assert_eq!(
            stats.arena_growth_events, 1,
            "the stats accumulator sees only the RETRY frame's growth event — the \
             instance growth that landed on the pose-fault frame belongs to a FAILED \
             frame, whose outcome (and delta) never reaches finish_frame; the monotone \
             context counter has both, and the capacities + census pair below are the \
             live proofs"
        );
        assert_eq!(
            renderer.allocations().arena_replacements,
            renderer.allocations().arena_replacement_frees + 2,
            "the two committed replacements are the live staged pairs"
        );
        assert_eq!(stats.validation_errors, 0, "{:?}", stats.recent_validation);
        drop(renderer);
        drop(app);
        println!("CA-11 Instance/PoseGrowthIdle: released, grew on the retry");
    }
}

/// CA-13 (F13/A14 — the device-loss lane's rig leg, added by the
/// CA-12..CA-15 config/scene lane as a FAULT-SEAM EXTENSION of this
/// battery): the injected `VK_ERROR_DEVICE_LOST` at the acquire site
/// must surface as the TYPED `RendererError::DeviceLost` (the
/// operation context riding the variant), and the renderer's LATCH
/// must prevent every later device-touching entry — the second,
/// UNARMED submit refuses with the same typed error WITHOUT
/// reaching the acquire (an unarmed second call that touched the
/// device would acquire a real image and present), and the attach
/// and capture families refuse the same way. The knob is
/// `Renderer::inject_device_loss` (the one-shot
/// `set_acquire_device_lost` cell — the acquire-out-of-date knob's
/// own discipline); real device removal stays the separate owner
/// validation question the audit names.
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn device_loss_is_typed_and_latched() {
    let _guard = sdl_lock();
    let (mut app, mut renderer) = lifecycle_renderer("CA-13 device loss");
    let _ = app.frame();
    let size = app.window().size_in_pixels();
    let matrix = lifecycle_matrix(size);
    attach_corner_scene(&mut renderer).expect("the scene attaches");
    // Warm past both frame slots so the loss lands on a slot with
    // real previous submissions.
    for _ in 0..3 {
        let outcome = submit_plain_scene_frame(&mut renderer, &matrix, size);
        assert!(outcome.presented, "the warm frame presents");
    }
    // The injected loss: the next frame's FIRST acquire returns
    // VK_ERROR_DEVICE_LOST (the faithful driver contract — no image
    // was acquired).
    renderer.inject_device_loss();
    match renderer.submit_frame(corner_submission(
        &matrix,
        [0.0, 1.7, 15.0],
        day_preset(),
        None,
        corner_bounds(),
        None,
        None,
        false,
        None,
        size,
    )) {
        Err(engine_renderer::RendererError::DeviceLost { operation }) => assert_eq!(
            operation, "acquire_next_image",
            "the operation context rides the typed variant"
        ),
        other => panic!("expected the typed device-loss refusal, got {other:?}"),
    }
    // THE LATCH: the second submit is UNARMED — if it reached the
    // device it would acquire a real image and present; the latch
    // must refuse with the same typed error and the FIRST
    // observation's context, without touching the dead device.
    let second_started = Instant::now();
    match renderer.submit_frame(corner_submission(
        &matrix,
        [0.0, 1.7, 15.0],
        day_preset(),
        None,
        corner_bounds(),
        None,
        None,
        false,
        None,
        size,
    )) {
        Err(engine_renderer::RendererError::DeviceLost { operation }) => assert_eq!(
            operation, "acquire_next_image",
            "the first observation's context is stable"
        ),
        other => panic!("the latched refusal, got {other:?}"),
    }
    assert!(
        second_started.elapsed().as_millis() < 2_000,
        "the latched refusal returns without device work (no acquire timeout)"
    );
    // Attach refuses the same way (the uploads and pipeline group
    // are device work).
    match attach_corner_scene(&mut renderer) {
        Err(engine_renderer::RendererError::DeviceLost { .. }) => {}
        other => panic!("the latched attach refusal, got {other:?}"),
    }
    // The capture family refuses (the request would never complete;
    // the take's fence wait is device work).
    match renderer.request_frame_capture() {
        Err(engine_renderer::RendererError::DeviceLost { .. }) => {}
        other => panic!("the latched capture-request refusal, got {other:?}"),
    }
    match renderer.take_frame_capture() {
        Err(engine_renderer::RendererError::DeviceLost { .. }) => {}
        other => panic!("the latched capture-take refusal, got {other:?}"),
    }
    match renderer.request_shadow_map_capture() {
        Err(engine_renderer::RendererError::DeviceLost { .. }) => {}
        other => panic!("the latched shadow-request refusal, got {other:?}"),
    }
    match renderer.take_shadow_map_capture() {
        Err(engine_renderer::RendererError::DeviceLost { .. }) => {}
        other => panic!("the latched shadow-take refusal, got {other:?}"),
    }
    // Nothing above touched the device validation-wise (the losses
    // are the injection's, not a real driver error).
    let stats = renderer.stats();
    assert_eq!(stats.validation_errors, 0, "{:?}", stats.recent_validation);
    drop(renderer);
    drop(app);
    println!("CA-13 device loss: typed at the boundary, latched across every entry");
}

/// T06/L1 (the lifecycle substrate's rig leg — the design's
/// LIFECYCLE-T02-T03-T06-T16 §6 L1 row): the CA-13 latch absorbed
/// into the `RendererLifecycle` state machine keeps every refusal
/// surface — including `render_layers`, the frame entry the CA-13
/// battery's submit-only leg did not exercise — AND the renderer
/// with a latched loss DROPS cleanly (the §4.2 disposal discipline:
/// the lifecycle enters `Destroying`, the idle's error is observed
/// rather than discarded, and the destroy sequence proceeds on the
/// loss-abandon semantics — `vkDestroy*`/`vkFree*` remain the
/// supported teardown path after `ERROR_DEVICE_LOST`). The
/// loss-from-`get_fence_status` arms (§4.3's poll classification on
/// a REAL loss) are owner-validation territory per the design's own
/// honest limit §7.2: `inject_device_loss` models only the acquire
/// site, so those arms are pinned by the CPU classification tables
/// (`the_fence_poll_classifications_table_every_arm`) until a real
/// loss is observed on the rig.
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn lifecycle_latch_covers_render_layers_and_the_loss_renderer_drops_cleanly() {
    let _guard = sdl_lock();
    let (mut app, mut renderer) = lifecycle_renderer("T06 L1 lifecycle substrate");
    let _ = app.frame();
    let size = app.window().size_in_pixels();
    attach_corner_scene(&mut renderer).expect("the scene attaches");
    for _ in 0..3 {
        let outcome = submit_plain_scene_frame(&mut renderer, &lifecycle_matrix(size), size);
        assert!(outcome.presented, "the warm frame presents");
    }
    // The injected loss at the acquire site (the one seam that
    // models the terminal outcome faithfully).
    renderer.inject_device_loss();
    match renderer.submit_frame(corner_submission(
        &lifecycle_matrix(size),
        [0.0, 1.7, 15.0],
        day_preset(),
        None,
        corner_bounds(),
        None,
        None,
        false,
        None,
        size,
    )) {
        Err(engine_renderer::RendererError::DeviceLost { operation }) => assert_eq!(
            operation, "acquire_next_image",
            "the lifecycle's DeviceLost state carries CA-13's first context"
        ),
        other => panic!("the typed loss refusal, got {other:?}"),
    }
    // THE RENDER_LAYERS GATE (the frame entry the CA-13 battery did
    // not exercise): refused with the same first observation's
    // context, without touching the dead device (an unarmed call
    // that reached the device would present a real frame).
    let refused = renderer.render_layers(
        &engine_slice::math::identity(),
        &[],
        None,
        &[],
        None,
        size,
    );
    match refused {
        Err(engine_renderer::RendererError::DeviceLost { operation }) => assert_eq!(
            operation, "acquire_next_image",
            "render_layers refuses through the same lifecycle gate"
        ),
        other => panic!("the render_layers latched refusal, got {other:?}"),
    }
    // Validation clean before the drop (the losses are the
    // injection's, not a real driver error).
    let stats = renderer.stats();
    assert_eq!(stats.validation_errors, 0, "{:?}", stats.recent_validation);
    // THE §4.2 DISPOSAL: the latched renderer drops cleanly — the
    // lifecycle's Destroying observability state records the idle's
    // result instead of discarding it, and the ordered teardown
    // (frames → swapchain → pipes → context) completes on the
    // loss-abandon semantics. A panic or a wedged drop fails the
    // test right here.
    drop(renderer);
    drop(app);
    println!("T06 L1: the lifecycle latch covers render_layers; the loss renderer drops cleanly");
}
