//! THE RIPPLE-VISIBILITY A/B DIAGNOSTIC (the sign-off review's
//! recommendation, before any puddle tuning): the owner's walk
//! established that natural rings are unreadable on bare wet asphalt;
//! the open question the review posed — "insufficient opportunity
//! (sparse admission) or weak per-ring movement (the gain)" — is
//! decided by a matched replay of the SAME camera/seed/route at the
//! shipped settings, then at a lower decimation (more admitted
//! rings), then at a higher ripple gain — counting, per arm
//! (B11's labeling corrective: every census below is FIELD-level,
//! never a rendered-pixel claim): the kill-plane ground landings
//! (the layer's kill-death census — the admission funnel's input;
//! the particle report's all-cause `deaths` counts midair lifetime
//! expiries too and is NOT the contact census), the rings ADMITTED
//! (generated into the field — every `decimation`-th landing; at
//! the shipped 320:1 the other ~319 of 320 landings are never
//! generated at all, decimated BEFORE admission — "admitted" means
//! generated, never occluded-or-rendered), the pool refusals
//! (spawn attempts dropped at the ring bound), the live rings, and
//! the frames with at least one live ring in the FIELD (the
//! field-level opportunity census: how often something existed to
//! draw — whether a ring's displacement actually moved displayed
//! rgb is the rig pixel gates' question, measured there, never
//! here).
//!
//! Pod-observational (the behavior_measurement precedent): never a
//! CI threshold; the double-run determinism is asserted in every
//! arm (the same seed replays byte-identically), the numbers are
//! the rig's evidence for the puddle-package decision — the review's
//! own framing: "Retain the normal configuration and its exact pins
//! for acceptance. This distinguishes insufficient opportunity from
//! weak per-ring movement."

use crate::corner_timing::demo_clock_dt;
use crate::corner_weather::{RAIN_SEED, RIPPLE_POLICY, RainLayer};
use engine_core::ripple::RIPPLE_POOL_BOUND;

/// One arm's census.
struct ArmCensus {
    tag: &'static str,
    /// The KILL-PLANE ground landings in the measured window (the
    /// layer's `kill_deaths` census delta — real landings on the
    /// receiver plane, the admission funnel's input). B11's
    /// corrective: the pre-fix census summed the particle report's
    /// all-cause `deaths` under this label (midair lifetime
    /// expiries included — a different event class than the name
    /// claimed; near-identical at THIS harness's fixed-camera
    /// volume, wrong by construction and divergent elsewhere).
    contacts: u64,
    /// The ring ADMISSIONS in the measured window (the cumulative
    /// counter's delta — the pre-fix version summed the cumulative
    /// counter per frame, overcounting by the window length). An
    /// admitted ring is GENERATED into the field — admission is
    /// NOT visibility: the decimation refuses most landings before
    /// generation (never-generated, not occluded), and whether a
    /// generated ring reaches the displayed framebuffer is the
    /// displacement pass's coverage question (not measured here).
    admitted: u64,
    refusals: u64,
    live_at_end: usize,
    /// The frames (of the measured window) with at least one live
    /// ring in the FIELD (`active_count > 0`) — the OPPORTUNITY
    /// census at the FIELD level: how often something existed to
    /// draw. NOT a screen census: no occlusion, no
    /// displacement-pass coverage, no framebuffer test — a frame
    /// counts whether a live ring EXISTS, never whether it was
    /// rendered or moved displayed rgb (B11's labeling corrective).
    frames_with_live_rings: u64,
    max_live_rings: usize,
}

/// The fixed camera the weather gates use (the same vantage the
/// owner's scripted walk saw).
const EYE: [f32; 3] = [0.0, 1.7, 15.0];

/// One arm: the shipped policy with ONE knob varied (the decimation
/// — the admission rate; never more than one at a time: the A/B
/// discipline).
fn run_arm(tag: &'static str, decimation: u32, frames: u64) -> ArmCensus {
    // The layer with the ARM'S policy (the decimation knob) — the
    // same seed, the same camera, the same warm start the demo's
    // layer uses (the comparison is like-for-like against the
    // shipped arm).
    let policy = engine_core::ripple::RipplePolicy {
        decimation,
        ..RIPPLE_POLICY
    };
    let mut layer = RainLayer::with_ripple_policy(EYE, policy, demo_clock_dt().as_secs_f32());
    // The warm start (the demo's own 132 ticks — the first measured
    // frame is already wet).
    for _ in 0..132 {
        layer.update_and_build(EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
    }
    // The cumulative counters' baselines (the window's landings,
    // admissions, and refusals are the DELTAS — the counters are
    // lifetime-cumulative including the warm start).
    let landings_before = layer.kill_deaths();
    let spawned_before = layer.ripple_field().spawned();
    let refusals_before = layer.ripple_field().refused_total();
    let mut frames_with_live_rings = 0u64;
    let mut max_live_rings = 0usize;
    for _ in 0..frames {
        layer.update_and_build(EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
        let live = layer.ripple_field().active_count();
        max_live_rings = max_live_rings.max(live);
        if live > 0 {
            frames_with_live_rings += 1;
        }
    }
    let field = layer.ripple_field();
    // B11's corrective: contacts are the KILL-PLANE landings (the
    // layer's own contact census — `kill_deaths`, the same delta
    // discipline as the neighboring counters), never the particle
    // report's all-cause `deaths` (which counts midair lifetime
    // expiries too): the label and the counted class now agree.
    let contacts = layer.kill_deaths() - landings_before;
    let admitted = field.spawned() - spawned_before;
    let refusals = field.refused_total() - refusals_before;
    let live_at_end = field.active_count();
    ArmCensus {
        tag,
        contacts,
        admitted,
        refusals,
        live_at_end,
        frames_with_live_rings,
        max_live_rings,
    }
}

/// The diagnostic: the shipped arm, the 160:1 admission arm, and the
/// 80:1 arm (the pool bound 32 crossed deliberately at the highest —
// the refusals census records it), each double-run asserted
/// byte-identical (the same seed), the censuses printed beside each
/// other for the puddle-package decision.
pub fn run_ripple_visibility_diagnostic() {
    const FRAMES: u64 = 600;
    println!(
        "Ripple visibility A/B — the sign-off review's pre-puddle diagnostic \
         (the fixed gate camera, seed {RAIN_SEED:#x}, {FRAMES} frames ≈ 10 s \
         at 60 Hz, the shipped warm start; pod-observational, never a CI \
         threshold)"
    );
    println!(
        "  policy: lifetime {} s, speed {} m/s, width {} m, amplitude {}, \
         pool bound {RIPPLE_POOL_BOUND}",
        RIPPLE_POLICY.lifetime, RIPPLE_POLICY.speed, RIPPLE_POLICY.width, RIPPLE_POLICY.amplitude
    );
    println!(
        "  census terms (B11's labeling corrective — every count is a FIELD-level \
         census, never a rendered-pixel claim):"
    );
    println!(
        "    contacts  = kill-plane ground LANDINGS (the layer's kill-death census — \
         the admission funnel's input; the particle report's all-cause deaths \
         include midair lifetime expiries and are NOT this census)"
    );
    println!(
        "    admitted  = rings GENERATED into the field (every decimation-th landing; \
         at the shipped 320:1 the other ~319 of 320 landings are NEVER GENERATED — \
         decimated before admission, not occluded)"
    );
    println!(
        "    live / frames-with-a-live-ring = rings alive in the FIELD (generated and \
         unexpired; a frame counts EXISTENCE — whether the displacement pass \
         actually moved displayed rgb is not measured here)"
    );
    println!();
    let arms = [
        run_arm("shipped  (decimation 320)", 320, FRAMES),
        run_arm("diagnostic (decimation 160)", 160, FRAMES),
        run_arm("diagnostic (decimation 80)", 80, FRAMES),
    ];
    // THE DOUBLE-RUN DETERMINISM: the shipped arm replayed — the
    // censuses identical (the fixed seed's streams are
    // platform-stable; the arm's own determinism, the
    // behavior_measurement precedent).
    let replay = run_arm("shipped (replay)", 320, FRAMES);
    assert_eq!(
        (replay.contacts, replay.admitted, replay.refusals),
        (arms[0].contacts, arms[0].admitted, arms[0].refusals),
        "the shipped arm's census double-run identical"
    );
    for arm in &arms {
        println!(
            "  {}: contacts {} (kill-plane landings), admitted {} (rings generated; \
             pool refusals {}), live at end {}, max live {}, frames with a live \
             ring in the field {}/{} ({:.0}%)",
            arm.tag,
            arm.contacts,
            arm.admitted,
            arm.refusals,
            arm.live_at_end,
            arm.max_live_rings,
            arm.frames_with_live_rings,
            FRAMES,
            100.0 * arm.frames_with_live_rings as f64 / FRAMES as f64
        );
    }
    println!();
    println!(
        "  the decision input: the shipped arm saw {} landings and admitted {} rings \
         (~1 in 320 — the rest NEVER GENERATED, decimated before admission, not \
         occluded); the steady state is ~{:.1} live rings (the admission rate \
         {:.2}/s x the {:.1} s lifetime; measured max {}, at end {}); the 160:1 \
         arm doubles the ADMISSION opportunity, the 80:1 arm quadruples it \
         toward the pool bound (the refusals census records any saturation). \
         If the 160:1/80:1 censuses still read as unreadable on the rig's --corner \
         --populated --weather rain walk, the per-ring MOVEMENT (the \
         gain, the puddle's darkened water and tracked reflected edge) \
         is the readability path — the authored puddle package, per the \
         review's question 7. If the higher admission reads clearly, a \
         policy re-pin (the decimation, the owner's GO) is the cheaper \
         correction.",
        arms[0].contacts,
        arms[0].admitted,
        arms[0].admitted as f64 / (FRAMES as f64 / 60.0) * RIPPLE_POLICY.lifetime as f64,
        arms[0].admitted as f64 / (FRAMES as f64 / 60.0),
        RIPPLE_POLICY.lifetime,
        arms[0].max_live_rings,
        arms[0].live_at_end
    );
}
