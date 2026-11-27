//! Minimal GLSL-to-SPIR-V compilation path (T-05, Stage 2): pure-Rust
//! naga compilation at build time — no external shader tools required on
//! the dev pod, CI runners, or the lead's rig. The permanent
//! shader-toolchain decision remains deferred.
//!
//! P4 S5 — shader build-time composition (design §6.3): the pinned
//! naga's wgsl-in has no include/preprocessor facility, so the nine
//! variant-pair sources (the lit / instanced-proxy / skinned-proxy /
//! shadow-depth families) are composed HERE, at text level: shared
//! fragments under `shaders/core/*.wgsl` are spliced into
//! `shaders/variants/*.wgsl` shells at `// @core:<name>` anchor lines
//! (a byte-level string splice — no templating language, no new
//! dependencies). The composed byte stream is, by construction, the
//! exact stream the pre-S5 monolithic sources held (the migration
//! verified every variant round-trips byte-identically), so the
//! emitted SPIR-V is byte-identical through the same pinned naga.
//! Static specialization is untouched: the emitted module set and
//! every `out_name` are unchanged (pipeline assembly never knows), one
//! specialized .spv per variant, zero runtime branching. Every
//! composed source is dumped to `OUT_DIR/composed/<variant>.wgsl` —
//! always inspectable, greppable, diffable (the design's debug-dump
//! requirement, always on rather than behind an env var).
//!
//! P4 S5 — layout pins (design §6.4): while compiling the composed
//! variants, the host-mirrored uniform structs (FrameParams plain and
//! shadowed, MaterialParams, LightMatrix) have their layouts resolved
//! through the same parsed IR — member offsets from naga's
//! `StructMember::offset` (the exact values the SPIR-V writer emits as
//! the members' `Offset` decorations), sizes through naga's WGSL
//! memory-layout table. The pins are cross-checked for agreement
//! across EVERY variant consumer before `OUT_DIR/layout_pins.rs` is
//! emitted; `vulkan::shader_layout_pins` includes that file and the
//! Rust mirrors assert against it, so a layout drift between the Rust
//! packing and the WGSL consumption fails the build on whichever side
//! moved.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn compile_shader(stage: &str, file: &str, out_name: &str) {
    let shader_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("shaders");
    let src_path = shader_dir.join(file);
    println!("cargo:rerun-if-changed={}", src_path.display());
    let source = fs::read_to_string(&src_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", src_path.display()));

    let naga_stage = match stage {
        "vertex" => naga::ShaderStage::Vertex,
        "fragment" => naga::ShaderStage::Fragment,
        _ => panic!("unknown shader stage {stage}"),
    };
    let mut frontend = naga::front::glsl::Frontend::default();
    let module = frontend
        .parse(&naga::front::glsl::Options::from(naga_stage), &source)
        .unwrap_or_else(|error| panic!("GLSL parse {file}: {error:?}"));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("GLSL validation {file}: {error:?}"));
    let words =
        naga::back::spv::write_vec(&module, &info, &naga::back::spv::Options::default(), None)
            .unwrap_or_else(|error| panic!("SPIR-V generation {file}: {error:?}"));

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join(out_name);
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in &words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    fs::write(&out, bytes).unwrap_or_else(|error| panic!("write {out:?}: {error}"));
}

fn main() {
    compile_shader("vertex", "scene.vert", "scene.vert.spv");
    compile_shader("fragment", "scene.frag", "scene.frag.spv");
    compile_shader("vertex", "overlay.vert", "overlay.vert.spv");
    compile_shader("fragment", "overlay.frag", "overlay.frag.spv");
    // The Cycle 6-A atlas overlay shaders (see docs/CYCLE6-6A-BRIEF.md):
    // WGSL, because the pinned naga's glsl-in cannot express sampled
    // textures (the probed finding in the shader's header). ONE module
    // carries BOTH entry points (vs_main / fs_main); the overlay pipeline
    // creates the two shader modules from these same bytes, naming the
    // entry points.
    compile_wgsl("overlay_atlas.wgsl", "overlay_atlas.spv");
    // The nine P4 S5 composed variants (design §6.3): the lit /
    // instanced / skinned / shadow families whose duplicated
    // lighting/shadow/layout semantics now live once in shaders/core/.
    // The comments below are each family's original documentation; the
    // compile calls are the SAME outputs under the SAME out_names.
    let mut pins = LayoutPins::default();
    // The Cycle 6-B lit-scene shaders (see docs/CYCLE6-6B-BRIEF.md):
    // the same one-module/both-entry-points form.
    compile_wgsl_composed("lit_scene.wgsl", "lit_scene.spv", &mut pins);
    // The Cycle 6-E instanced-proxy shaders (see
    // docs/CYCLE6-6E-BRIEF.md): the same one-module/both-entry-
    // points form, the same binding set as lit_scene (0-3).
    compile_wgsl_composed("instanced_proxy.wgsl", "instanced_proxy.spv", &mut pins);
    // The Era-2 package C particle shader (the rain kind — the third
    // populated kind): unlit tinted blended quads, the same binding
    // set as the lit family (0-2 declared; the header documents the
    // choice and the depth/blend contract). Not a composed variant:
    // no duplicated semantics (the unlit equation exists only here).
    compile_wgsl("particle.wgsl", "particle.spv");
    // The Cycle 6-F shadow shaders (see
    // docs/AUTONOMOUS-CYCLE-PROTOCOL.md §2 item 2): the depth-only
    // pass (one uniform: the light matrix) and the shadowed lit
    // variant (lit_scene's bindings plus the depth map at 4).
    compile_wgsl_composed("shadow_depth.wgsl", "shadow_depth.spv", &mut pins);
    compile_wgsl_composed(
        "lit_scene_shadowed.wgsl",
        "lit_scene_shadowed.spv",
        &mut pins,
    );
    // The Era-1 atmosphere shaders (see docs/CYCLE6-ERA1-BRIEF.md):
    // the sky (equation-mirroring sky.rs) and the post pass (post.rs).
    compile_wgsl("sky.wgsl", "sky.spv");
    compile_wgsl("post.wgsl", "post.spv");
    // The Era-1 package 6 bloom chain (see docs/CYCLE6-ERA1-BRIEF.md):
    // the bright-passed 1/4 downsample + the separable blur feeding
    // post.wgsl's composite — post.rs's chain, mirrored term-for-term.
    compile_wgsl("bloom.wgsl", "bloom.spv");
    // The Era-1 package 7 wet-ground reflection (see
    // docs/CYCLE6-ERA1-BRIEF.md): the ground re-draw sampling the
    // planar-reflection target, Schlick-blended — planar.rs's
    // reflection_blend mirrored term-for-term.
    compile_wgsl("ground_reflect.wgsl", "ground_reflect.spv");
    // The Era-2 stage 2B animation + skinning + proxy shadows modules
    // (five new WGSL shaders, binding 5 for the pose storage buffer).
    compile_wgsl_composed(
        "instanced_proxy_skinned.wgsl",
        "instanced_proxy_skinned.spv",
        &mut pins,
    );
    compile_wgsl_composed(
        "instanced_proxy_skinned_shadowed.wgsl",
        "instanced_proxy_skinned_shadowed.spv",
        &mut pins,
    );
    compile_wgsl_composed(
        "instanced_proxy_shadowed.wgsl",
        "instanced_proxy_shadowed.spv",
        &mut pins,
    );
    compile_wgsl_composed(
        "shadow_depth_skinned.wgsl",
        "shadow_depth_skinned.spv",
        &mut pins,
    );
    compile_wgsl_composed(
        "shadow_depth_instanced.wgsl",
        "shadow_depth_instanced.spv",
        &mut pins,
    );
    // The layout pins land only after every composed variant recorded
    // its structs (the cross-variant agreement is the point).
    pins.finish(&PathBuf::from(env::var("OUT_DIR").unwrap()));
}

/// The WGSL compile pipeline (Cycle 6-A): parse + validate + emit ONE
/// SPIR-V module containing every entry point in the source. Same
/// pinned naga, same pure-Rust discipline as the GLSL path.
fn compile_wgsl_source(label: &str, source: &str, out_name: &str) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("WGSL parse {label}: {}", error));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("WGSL validation {label}: {error:?}"));
    // WGSL requires distinct bindings. Vulkan permits separate OpTypeImage
    // and OpTypeSampler variables to alias the same combined descriptor.
    let mut options = naga::back::spv::Options {
        fake_missing_bindings: false,
        ..Default::default()
    };
    // Binding map: the 6-B descriptor foundation is reused as-is.
    // Binding 5 (the Era-2 skinning pose storage buffer) passes
    // through unchanged — the same binding number in SPIR-V.
    for binding in [0, 1, 2, 3, 4, 5] {
        options.binding_map.insert(
            naga::ResourceBinding { group: 0, binding },
            naga::back::spv::BindingInfo {
                // Texture + sampler share descriptor 0. The two uniform
                // blocks must retain their distinct SPIR-V decorations.
                // Binding 4 (the 6-F shadow map, a texture_depth_2d read
                // with textureLoad) keeps its own descriptor.
                // Binding 5 (the 2-B pose storage buffer) keeps its own
                // descriptor (the Era-2 skinning convention).
                binding: if binding == 1 { 0 } else { binding },
                ..Default::default()
            },
        );
    }
    let words = naga::back::spv::write_vec(&module, &info, &options, None)
        .unwrap_or_else(|error| panic!("SPIR-V generation {label}: {error:?}"));

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join(out_name);
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in &words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    fs::write(&out, bytes).unwrap_or_else(|error| panic!("write {out:?}: {error}"));
}

/// The direct WGSL path: one authored source file, unchanged since
/// Cycle 6-A (the non-variant shaders — atlas, particle, sky, post,
/// bloom, ground reflection).
fn compile_wgsl(file: &str, out_name: &str) {
    let shader_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("shaders");
    let src_path = shader_dir.join(file);
    println!("cargo:rerun-if-changed={}", src_path.display());
    let source = fs::read_to_string(&src_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", src_path.display()));
    compile_wgsl_source(file, &source, out_name);
}

/// The composed variant path (P4 S5, design §6.3): splice the shell's
/// `// @core:` anchors with the shared fragments, dump the composed
/// text to `OUT_DIR/composed/` (always inspectable), record the layout
/// pins from the parsed module, then run the same compile pipeline
/// under the SAME out_name the pre-S5 monolithic file used — pipeline
/// assembly (the include_bytes! consumers) is untouched.
fn compile_wgsl_composed(variant: &'static str, out_name: &str, pins: &mut LayoutPins) {
    let (bytes, deps) = compose(variant);
    for dep in &deps {
        println!("cargo:rerun-if-changed={}", dep.display());
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let composed_dir = out_dir.join("composed");
    fs::create_dir_all(&composed_dir)
        .unwrap_or_else(|error| panic!("mkdir {}: {error}", composed_dir.display()));
    fs::write(composed_dir.join(variant), &bytes)
        .unwrap_or_else(|error| panic!("dump composed {variant}: {error}"));
    let source =
        std::str::from_utf8(&bytes).unwrap_or_else(|error| panic!("compose {variant}: {error}"));
    // The pin recording parses the composed source a second time (the
    // compile pipeline below parses again) — build-time cost only, and
    // it keeps the compile path byte-for-byte the Cycle 6-A shape.
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("WGSL parse variants/{variant}: {}", error));
    pins.record(variant, &module);
    compile_wgsl_source(&format!("variants/{variant} (composed)"), source, out_name);
}

/// The composition splice (design §6.3): reads
/// `shaders/variants/<variant>`, replaces each `// @core:<name>`
/// anchor LINE with the verbatim bytes of `shaders/core/<name>.wgsl`,
/// and returns the assembled source plus every file read (for
/// rerun-if-changed). Byte-level on purpose: the composed stream is
/// the exact byte sequence the pre-S5 monolithic file held (the
/// migration verified each variant round-trips byte-identically),
/// which is what makes the emitted SPIR-V byte-identical — the splice
/// never normalizes line endings or rewrites text. A missing core
/// file is a build failure (every anchor must resolve).
fn compose(variant: &str) -> (Vec<u8>, Vec<PathBuf>) {
    let shader_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("shaders");
    let shell_path = shader_dir.join("variants").join(variant);
    let shell = fs::read(&shell_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", shell_path.display()));
    let mut deps = vec![shell_path];
    let mut source = Vec::with_capacity(shell.len() * 2);
    for line in shell.split_inclusive(|byte| *byte == b'\n') {
        let body = line.strip_suffix(&b"\n"[..]).unwrap_or(line);
        let body = body.strip_suffix(&b"\r"[..]).unwrap_or(body);
        if let Some(name) = body.strip_prefix(b"// @core:") {
            let name = std::str::from_utf8(name)
                .unwrap_or_else(|error| panic!("compose {variant}: anchor name: {error}"))
                .trim();
            let core_path = shader_dir.join("core").join(format!("{name}.wgsl"));
            let core = fs::read(&core_path).unwrap_or_else(|error| {
                panic!(
                    "compose {variant}: core {name} unreadable ({error}) — every anchor must \
                     resolve to shaders/core/{name}.wgsl"
                )
            });
            deps.push(core_path);
            source.extend_from_slice(&core);
        } else {
            source.extend_from_slice(line);
        }
    }
    (source, deps)
}

/// One uniform struct's WGSL-side layout: the resolved byte size plus
/// each member's byte offset (naga's `StructMember::offset` — the
/// exact value the SPIR-V writer emits as the member's `Offset`
/// decoration, so these pins describe the emitted words, not just the
/// source).
#[derive(Clone, PartialEq, Debug)]
struct StructPin {
    size: u32,
    members: Vec<(String, u32)>,
}

/// Resolves `struct_name`'s layout through the parsed module (the
/// name is unique per variant source — each uniform block appears
/// once).
fn struct_pin(module: &naga::Module, struct_name: &str) -> Option<StructPin> {
    for (handle, ty) in module.types.iter() {
        if ty.name.as_deref() != Some(struct_name) {
            continue;
        }
        let naga::TypeInner::Struct { members, .. } = &ty.inner else {
            continue;
        };
        let mut layouter = naga::proc::Layouter::default();
        layouter
            .update(module.to_ctx())
            .unwrap_or_else(|error| panic!("layout {struct_name}: {error:?}"));
        return Some(StructPin {
            size: layouter[handle].size,
            members: members
                .iter()
                .map(|member| {
                    let name = member
                        .name
                        .clone()
                        .unwrap_or_else(|| panic!("{struct_name}: unnamed member"));
                    (name, member.offset)
                })
                .collect(),
        });
    }
    None
}

/// The P4 S5 layout pins (design §6.4): the WGSL-side uniform-struct
/// layouts collected from every composed variant, asserted for
/// cross-variant agreement, and emitted as `OUT_DIR/layout_pins.rs` —
/// the single checked source the Rust mirrors assert against. The
/// expected consumer counts are the design's variant matrix: the
/// plain FrameParams block in the three unshadowed lit-family
/// variants, the extended block in the three shadowed ones,
/// MaterialParams in all six, LightMatrix in the three depth-only
/// passes.
#[derive(Default)]
struct LayoutPins {
    frame_params: Vec<(&'static str, StructPin)>,
    frame_params_shadowed: Vec<(&'static str, StructPin)>,
    material_params: Vec<(&'static str, StructPin)>,
    light_matrix: Vec<(&'static str, StructPin)>,
}

impl LayoutPins {
    /// Records the pin-able uniform structs of one composed variant.
    fn record(&mut self, variant: &'static str, module: &naga::Module) {
        if let Some(pin) = struct_pin(module, "FrameParams") {
            if pin
                .members
                .iter()
                .any(|(name, _)| name == "light_view_proj")
            {
                self.frame_params_shadowed.push((variant, pin));
            } else {
                self.frame_params.push((variant, pin));
            }
        }
        if let Some(pin) = struct_pin(module, "MaterialParams") {
            self.material_params.push((variant, pin));
        }
        if let Some(pin) = struct_pin(module, "LightMatrix") {
            self.light_matrix.push((variant, pin));
        }
    }

    /// Asserts the cross-variant agreement (and the plain/shadowed
    /// prefix invariant), then writes `layout_pins.rs`.
    fn finish(self, out_dir: &Path) {
        let frame = agreed(&self.frame_params, "FrameParams (plain block)", 3);
        let shadowed = agreed(
            &self.frame_params_shadowed,
            "FrameParams (shadowed block)",
            3,
        );
        let material = agreed(&self.material_params, "MaterialParams", 6);
        let light = agreed(&self.light_matrix, "LightMatrix", 3);
        // The prefix invariant (the design's "the unshadowed shader
        // reads only the block's 112-byte prefix"): the shadowed
        // block's base members must pin-identically to the plain
        // block's — the same bytes, the same offsets.
        assert_eq!(
            &shadowed.members[..frame.members.len()],
            &frame.members[..],
            "FrameParams: the shadowed block's base members must match the plain block's"
        );

        let variants = |group: &[(&'static str, StructPin)]| {
            group
                .iter()
                .map(|(variant, _)| *variant)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut file = String::new();
        file.push_str(
            "// GENERATED by engine-renderer/build.rs (P4 S5 layout pins, design §6.4) — \
             DO NOT EDIT.\n\
             // The WGSL-side uniform struct layouts, resolved through the pinned naga\n\
             // (=29.0.4) from the COMPOSED variant sources: member offsets from the\n\
             // parsed IR (the same values the SPIR-V writer emits as OpMemberDecorate\n\
             // Offset), sizes through naga's WGSL memory-layout table. Cross-variant\n\
             // agreement was asserted at build time before this file was written; the\n\
             // host mirrors assert against these constants (vulkan/passes/corner.rs,\n\
             // vulkan/passes/shadow_draw.rs) — a drift on either side fails the build.\n",
        );
        file.push_str(&format!(
            "// FrameParams (plain block, binding 2) — agreed across: {}.\n",
            variants(&self.frame_params)
        ));
        file.push_str(&format!(
            "pub const FRAME_PARAMS_SIZE: usize = {};\n",
            frame.size
        ));
        for (name, offset) in &frame.members {
            file.push_str(&format!(
                "pub const FRAME_PARAMS_{}_OFFSET: usize = {};\n",
                name.to_uppercase(),
                offset
            ));
        }
        file.push_str(&format!(
            "// FrameParams (shadowed extension; the base members pin-identically to the\n\
             // plain block's — the unshadowed shader reads the same 112-byte prefix) —\n\
             // agreed across: {}.\n",
            variants(&self.frame_params_shadowed)
        ));
        file.push_str(&format!(
            "pub const SHADOW_FRAME_PARAMS_SIZE: usize = {};\n",
            shadowed.size
        ));
        for (name, offset) in &shadowed.members[frame.members.len()..] {
            file.push_str(&format!(
                "pub const SHADOW_FRAME_PARAMS_{}_OFFSET: usize = {};\n",
                name.to_uppercase(),
                offset
            ));
        }
        file.push_str(&format!(
            "// MaterialParams (binding 3) — agreed across: {}.\n",
            variants(&self.material_params)
        ));
        file.push_str(&format!(
            "pub const MATERIAL_PARAMS_SIZE: usize = {};\n",
            material.size
        ));
        for (name, offset) in &material.members {
            file.push_str(&format!(
                "pub const MATERIAL_PARAMS_{}_OFFSET: usize = {};\n",
                name.to_uppercase(),
                offset
            ));
        }
        file.push_str(&format!(
            "// LightMatrix (the depth-only pass's binding 0) — agreed across: {}.\n",
            variants(&self.light_matrix)
        ));
        file.push_str(&format!(
            "pub const LIGHT_MATRIX_SIZE: usize = {};\n",
            light.size
        ));
        for (name, offset) in &light.members {
            file.push_str(&format!(
                "pub const LIGHT_MATRIX_{}_OFFSET: usize = {};\n",
                name.to_uppercase(),
                offset
            ));
        }
        fs::write(out_dir.join("layout_pins.rs"), file)
            .unwrap_or_else(|error| panic!("write layout_pins.rs: {error}"));
    }
}

/// The agreement check behind every pin group: exactly `want`
/// consumers found, and every one of them byte-equal to the first.
fn agreed<'a>(group: &'a [(&'static str, StructPin)], what: &'a str, want: usize) -> &'a StructPin {
    assert_eq!(
        group.len(),
        want,
        "{what}: expected {want} variant consumers, found [{}]",
        group
            .iter()
            .map(|(variant, _)| *variant)
            .collect::<Vec<_>>()
            .join(", ")
    );
    for (variant, pin) in &group[1..] {
        assert_eq!(
            pin, &group[0].1,
            "{what}: {variant} disagrees with {} — the shared core drifted",
            group[0].0
        );
    }
    &group[0].1
}
