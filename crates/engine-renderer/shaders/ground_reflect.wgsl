// The wet-ground reflection shader (Era 1 package 7 — see
// docs/CYCLE6-ERA1-BRIEF.md): the ground re-draw over itself,
// sampling the planar-reflection target at the SAME screen
// position (the mirrored view-projection maps every ground point
// to exactly its true clip position — the planar property
// planar.rs's unit test pins), plus the rain-ripple perturbation's
// PROJECTED displacement (Era-2 finding 19's corrective — see the
// projection block in fs_main), blended by Schlick's
// angle-based factor — planar.rs :: reflection_blend mirrored
// term-for-term:
//   b(n_dot_v) = 0.02 + 0.98 * (1 - n_dot_v)^5
// with the fifth power by repeated multiplication (no pow, the
// libm-free discipline) and n_dot_v the clamped dot of the
// surface normal and the unit view direction. The blend rides
// the FIXED-FUNCTION alpha stage (SRC_ALPHA / ONE_MINUS_SRC_ALPHA):
//   out = reflection.rgb * b + destination * (1 - b)
// exactly the CPU composite, with the destination being the
// already-rendered lit ground.
//
// TRANSPORT NOTES (the pinned toolchain facts): no
// var<push_constant> in wgsl-in (the frame block is a uniform);
// WGSL binding 1 remaps onto descriptor 0 (the 6-A atlas share),
// so the sampler rides binding 2 (post.wgsl's own convention) and
// the frame block binding 3. The frame block is THE CORNER'S OWN
// FrameParams (binding 2 in lit_scene.wgsl's layout — renumbered
// here because this shader's binding 2 is the sampler): the same
// per-slot region the corner mesh draws just read (view_proj +
// the eye in the spare w lanes — the same spare-lane convention).
//
// BINDINGS: 0 = the reflection target (sampled image, the
// per-frame-slot view), 2 = the sampler (linear, CLAMP_TO_EDGE),
// 3 = the FrameParams uniform (the corner's own buffer, dynamic).

@group(0) @binding(0)
var reflection_tex: texture_2d<f32>;
@group(0) @binding(2)
var reflection_sampler: sampler;

struct FrameParams {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,   // xyz = toward light, w = eye.x
    light_color: vec4<f32>, // rgb irradiance, w = eye.y
    ambient: vec4<f32>,     // rgb ambient, w = eye.z
};
@group(0) @binding(3)
var<uniform> frame: FrameParams;

// THE RIPPLE FIELD (Era-2 package C recursive item 1 — the rain
// ripples on the wet ground): the live ring set as a uniform block
// at binding 4 (the binding map's free slot — 0/2/3 are the
// reflection view, sampler, and frame block; binding 1 remaps onto
// descriptor 0 and is unusable, the header's transport note).
// LAYOUT (must match the CPU packer in reflection_draw.rs
// term-for-term): params0 = (live count, expansion speed m/s,
// band width m, tangent-displacement gain m); params1 = (roughen
// gain, TEST-ONLY debug mode, 0, 0); rings[i] = (center_x,
// center_z, age s, current amplitude).
// THE EMPTY FIELD IS A NO-OP BY IEEE IDENTITY (the identity gate's
// proof): count 0 skips the loop AND the projection, uv_offset
// stays (0,0) and screen_uv + (0,0) is bit-identical for the
// positive screen_uv values, roughen stays 0.0 and blend * (1.0 -
// 0.0 * g) == blend exactly — the rainless wet path renders Era-1's
// wet frame byte-for-byte. Pixels inside a live field but OUTSIDE
// every ring's band gate keep the same identity value-exactly: the
// zero world_offset projects to a (0, -0.0) uv difference and
// screen_uv + (0, -0.0) == screen_uv in IEEE arithmetic (x + (-0.0)
// is x for every finite x, +0.0 included).
struct RippleField {
    params0: vec4<f32>,
    params1: vec4<f32>,
    rings: array<vec4<f32>, 32>,
};
@group(0) @binding(4)
var<uniform> ripple_field: RippleField;

// The position output is `@invariant` (Era-1 review finding 19):
// this re-draw depth-tests EQUAL (no depth write) against the depth
// lit_scene.wgsl / lit_scene_shadowed.wgsl wrote for the SAME
// ground vertices — equal algebra in separately compiled pipelines
// does not establish bit-identical clip positions (the Vulkan
// invariance rules), and a divergent rasterized depth silently
// rejects the wet fragments on oblique ground. The Invariant
// decoration — carried by ALL THREE shaders — is the correction;
// vulkan.rs::position_invariance pins the emitted SPIR-V
// decorations from the pinned naga.
struct VertexOut {
    @invariant @builtin(position) position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>,
           @location(1) normal: vec3<f32>,
           @location(2) uv: vec2<f32>)
    -> VertexOut {
    var out: VertexOut;
    out.position = frame.view_proj * vec4<f32>(pos, 1.0);
    out.world_normal = normal;
    out.world_pos = pos;
    return out;
}

@fragment
fn fs_main(@location(0) world_normal: vec3<f32>,
           @location(1) world_pos: vec3<f32>,
           @builtin(position) frag: vec4<f32>)
    -> @location(0) vec4<f32> {
    // The reflection at THIS pixel: the mirrored view-projection
    // renders the mirrored world with the ground fixed, so the
    // same screen texel holds the reflected scene behind this
    // ground point (1:1 — the target is the frame's extent).
    let extent = vec2<f32>(textureDimensions(reflection_tex));
    let screen_uv = frag.xy / extent;
    // THE RIPPLE PERTURBATION (the design seed's classic water
    // distortion): each live ring's triangle-bump GRADIENT at this
    // ground point displaces the reflection SAMPLE in world units,
    // and the bump's height slightly roughens the Schlick blend (a
    // bumpy surface mirrors less). The band gate `band < width`
    // coincides EXACTLY with the bump's support (the bump is zero
    // at the edges), so the gate never cuts a nonzero height — the
    // one deliberate stylization is the GRADIENT's edge step (a
    // triangle bump's gradient is piecewise-constant; the stylized
    // hard ring edge every game water carries — documented, visual
    // policy).
    var uv_offset = vec2<f32>(0.0, 0.0);
    var roughen = 0.0f;
    let ripple_count = i32(ripple_field.params0.x);
    if (ripple_count > 0) {
        // THE TEST-ONLY DEBUG MODE (params1.y — renderer.rs's
        // RippleDebugMode, the Era-2 audit finding 15b separation
        // fixtures): 0 = the shipped path (the production packing
        // always writes 0), 1 = the UV/displacement term suppressed
        // (blend only), 2 = the blend/roughen term suppressed
        // (displacement only). The knobs exist so the ink gate can
        // isolate each term's visible contribution; they NEVER
        // appear in a production frame.
        let debug_mode = i32(ripple_field.params1.y);
        // THE WORLD TANGENT DISPLACEMENT (finding 19's corrective):
        // the gradient's push stays in WORLD meters — a radial
        // tangent vector at this ground point — NEVER in screen
        // units. The projection below converts it.
        var world_offset = vec2<f32>(0.0, 0.0);
        for (var i = 0; i < ripple_count; i = i + 1) {
            let ring = ripple_field.rings[i];
            let delta = vec2<f32>(world_pos.x - ring.x, world_pos.z - ring.y);
            let dist = sqrt(dot(delta, delta));
            let radius = ripple_field.params0.y * ring.z;
            let band = abs(dist - radius);
            let width = ripple_field.params0.z;
            if (band < width) {
                // the signed slope: +amp/width inside the ring line,
                // - outside (one subtraction and a sign — fixed order)
                let slope = ring.w / width;
                let signed = select(slope, -slope, dist > radius);
                // the radial direction (dist == 0 has none: zero
                // contribution — the exact center of a ring)
                let dir = select(vec2<f32>(0.0, 0.0), delta / dist, dist > 0.0);
                if (debug_mode != 1) {
                    world_offset = world_offset + dir * (signed * ripple_field.params0.w);
                }
                if (debug_mode != 2) {
                    roughen = roughen + ring.w * (1.0 - band / width);
                }
            }
        }
        // THE PROJECTION INTO SCREEN UV (finding 19's corrective):
        // the ground point P and the displaced point P + d (d stays
        // in the ground plane — a tangent displacement, world_pos.y
        // untouched) both project through the frame's TRUE
        // view-projection, the w-divide lands them in authored NDC,
        // and the difference — with the established Y convention
        // (authored NDC +y is screen-UP; screen_uv's frag.xy is
        // framebuffer y-DOWN, the vertex-boundary epilogue's flip
        // replayed here: u = (ndc_x + 1)/2, v = (1 - ndc_y)/2) — is
        // the sample offset in the SAME units screen_uv lives in.
        // This is the camera-correctness: turning the camera 180°
        // flips the screen direction of the same world push, pitch
        // foreshortens it, and perspective scales it by distance.
        // WHY THE TRUE VP IS "THE SAME VP THE REFLECTION TARGET WAS
        // RENDERED WITH": the target renders through the mirrored
        // view-projection (view_proj × M), and the mirror fixes the
        // ground plane — every point ON the plane (P and P + d both,
        // d being tangent) maps to BIT-IDENTICAL clip positions
        // under either matrix (planar.rs's
        // mirrored_view_projection_fixes_the_ground_plane pins it),
        // so the frame block's true view_proj (binding 3, the same
        // uniform the vertex stage already reads) needs NO new
        // uniform lane. The zero world_offset projects to exactly
        // (0, -0.0) — value-identical sampling (the header's
        // identity note).
        let clip_p = frame.view_proj * vec4<f32>(world_pos, 1.0);
        let clip_q = frame.view_proj * vec4<f32>(
            world_pos.x + world_offset.x,
            world_pos.y,
            world_pos.z + world_offset.y,
            1.0,
        );
        let ndc_p = clip_p.xy / clip_p.w;
        let ndc_q = clip_q.xy / clip_q.w;
        uv_offset = vec2<f32>(0.5, -0.5) * (ndc_q - ndc_p);
    }
    let reflection = textureSample(
        reflection_tex,
        reflection_sampler,
        screen_uv + uv_offset,
    ).rgb;
    // Schlick's blend by view angle — planar.rs ::
    // reflection_blend, term-for-term (the eye from the frame
    // block's spare lanes, the same convention lit_scene carries).
    let eye = vec3<f32>(frame.light_dir.w, frame.light_color.w, frame.ambient.w);
    let view = eye - world_pos;
    let view_length = sqrt(dot(view, view));
    let n_dot_v = clamp(dot(normalize(world_normal), view / view_length), 0.0, 1.0);
    let one_minus = 1.0 - n_dot_v;
    let pow5 = one_minus * one_minus * one_minus * one_minus * one_minus;
    let blend = 0.02 + 0.98 * pow5;
    // The roughening: the ripple peaks dim the mirror (blend scaled
    // toward zero by the bounded roughen coverage; min clamps the
    // overlap of many rings; the empty field leaves blend exactly
    // untouched — 1.0 - 0.0 * g == 1.0, blend * 1.0 == blend).
    let roughened = blend * (1.0 - min(roughen, 1.0) * ripple_field.params1.x);
    return vec4<f32>(reflection, roughened);
}
