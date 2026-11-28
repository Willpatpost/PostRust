//! §9.2's REPACK MATRIX (A18/R2's before/after row) — the per-frame
//! CPU cost of the three operations `adapt_frame` runs on EVERY
//! populated submission today (submission.rs's repack sequence):
//!
//! - **instance repack** — [`engine_renderer::proxy_instances::
//!   repack_populations`]: validate-all, clear, reserve, regenerate
//!   every instance's 96 B (two fixed-order Taylor series per
//!   instance plus the `to_bits` packing).
//! - **pose evaluation** — [`crate::corner_population::
//!   evaluate_population_poses`]: per NPC, `sample_pose` +
//!   `compose_joint_matrices` + `skin_matrices_bound` (the walkers;
//!   standers read the A17-landed cached rest block).
//! - **slot write** — the per-frame mapped-region memcpy
//!   `upload_frame_data` performs after the slot's fence wait
//!   (frame.rs: the instance slice + the pose slice + the 48-B
//!   indirect block). Measured here as a CPU-side SINK-BUFFER copy
//!   of the exact same byte counts (the memcpy the mapped regions
//!   receive; the rig re-run can additionally measure through the
//!   real device path — the byte counts, not the destination, are
//!   the row's variable).
//!
//! The axes (§9.2): populations {6 (garage), 24 (corner), 1k, 4k
//! (the cap)} × dirty fraction {0% (all standing), the stander mix
//! (each table's OWN authored mix — the garage's 2 stand / 4 walk,
//! the corner's 8 stand / 16 walk; the design's "~40%" label is a
//! rounding of the stander share, the tables' actual counts are
//! what run), 100% (all walking)}. BEFORE R2 the instance repack
//! and the slot write are WHOLE-SET regardless of dirtiness — the
//! flat columns across the dirty axis ARE the finding; the 0%
//! instance-repack cell is also the RENDER-THEN-STEP between-tick
//! frame's cost (an unchanged population still pays the full
//! repack — §9.2's second half, measured).
//!
//! §9.7's boundary leg lives here too: 4k is EXACTLY the cap
//! (MAX_INSTANCES = MAX_POSE_BLOCKS = 4096); cap+1 must refuse with
//! the typed capacity error, never a partial buffer (the D2
//! discipline — pinned by the tests below, re-used by the battery's
//! printed boundary line).

use std::time::{Duration, Instant};

use engine_renderer::animation::{MAX_POSE_BLOCKS, POSE_STRIDE, repack_poses};
use engine_renderer::proxy_indirect::INDIRECT_SLOT_BYTES;
use engine_renderer::proxy_instances::{
    INSTANCE_STRIDE, InstanceTransform, MAX_INSTANCES, ProxyInstance, repack_populations,
};

use crate::corner_population::{Block, Phase, evaluate_population_poses};

/// The REAL proxy mesh vertex counts (the same public mesh builders
/// ProxyResources reads — `skinned_humanoid_mesh` / `sedan_mesh` /
/// `rain_streak_mesh`): the indirect block's pack + copy per frame
/// uses exactly these numbers in production, so the census's
/// slot-write leg pays the identical arithmetic. Read once (the
/// meshes are deterministic tables).
fn proxy_mesh_vertex_counts() -> (u32, u32, u32) {
    (
        engine_renderer::animation::skinned_humanoid_mesh().len() as u32,
        engine_renderer::populated_meshes::sedan_mesh().len() as u32,
        engine_renderer::populated_meshes::rain_streak_mesh().len() as u32,
    )
}

/// One matrix axis point: a population's instances plus its OWN
/// authored stander mix (the dirty axis's middle cell).
pub struct PopulationSpec {
    /// The row label ("garage (6+1)", "corner (24+2)", "1k", "4k").
    pub label: &'static str,
    /// The npc kind's instances (pose_index sequential — the
    /// shipped population's pin).
    pub npc: Vec<ProxyInstance>,
    /// The vehicle kind's instances (empty at the synthetic scales —
    /// the axis is the NPC population).
    pub vehicles: Vec<ProxyInstance>,
    /// The population's own stander mix (the authored phase column;
    /// tiled for the synthetic scales).
    pub stander_mix: Vec<Phase>,
}

/// The dirty-fraction axis (§9.2's three cells).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyFraction {
    /// 0% — every NPC stands (the cached rest block every frame;
    /// the between-tick unchanged-population shape).
    Zero,
    /// The population's own authored stander mix.
    StanderMix,
    /// 100% — every NPC walks at spread phase offsets.
    Full,
}

impl DirtyFraction {
    /// The table's column label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Zero => "0% (all stand)",
            Self::StanderMix => "stander mix (authored)",
            Self::Full => "100% (all walk)",
        }
    }
}

/// One matrix row: a (population × dirty fraction) cell's three
/// measured operations plus its byte context.
pub struct MatrixRow {
    pub population: &'static str,
    pub dirty: &'static str,
    pub walkers: usize,
    pub standers: usize,
    pub instance_repack: TimingStats,
    pub pose_evaluation: TimingStats,
    pub slot_write: TimingStats,
    /// The frame's packed instance bytes (npc + vehicles).
    pub instance_bytes: usize,
    /// The frame's packed pose bytes (one block per npc).
    pub pose_bytes: usize,
}

pub use crate::census::{TimingStats, timing_stats};

/// The corner table's authored stander mix (8 stand / 16 walk of
/// 24) — the tiling pattern for the synthetic scales, so every
/// "stander mix" cell keeps the corner's measured ratio.
fn corner_mix_pattern() -> Vec<Phase> {
    crate::corner_population::corner_population_phases()
}

/// Builds the phase list for one cell: the dirty fraction applied
/// over `count` NPCs (the stander-mix cell tiles the population's
/// own authored column; Full spreads distinct walk offsets the way
/// the corner table does — pure data, deterministic).
fn phases_for(count: usize, fraction: DirtyFraction, authored: &[Phase]) -> Vec<Phase> {
    match fraction {
        DirtyFraction::Zero => vec![Phase::Stand; count],
        DirtyFraction::StanderMix => (0..count).map(|i| authored[i % authored.len()]).collect(),
        DirtyFraction::Full => (0..count)
            .map(|i| Phase::Walk(((i as u32 * 40_503) % 65_536) as u16))
            .collect(),
    }
}

/// A deterministic synthetic population (the 1k / 4k axis points):
/// a placement grid over the ±60 m field, spread cardinal-biased
/// facings, the corner palette's tints cycled, pose_index
/// sequential. Valid by construction (finite positions, unit
/// scale, unit alpha, in-range tints — the repack's own validation
/// is the proof when the battery runs).
pub fn synthetic_population(count: usize) -> Vec<ProxyInstance> {
    let palette = crate::corner_population::corner_npc_tints();
    (0..count)
        .map(|i| {
            let ring = (i / 8) as f32 + 1.0;
            let spoke = i % 8;
            let angle = (spoke as f32 / 8.0) * core::f32::consts::TAU;
            ProxyInstance {
                transform: InstanceTransform {
                    pos: [angle.cos() * 6.0 * ring, 0.02, angle.sin() * 6.0 * ring],
                    turn: ((spoke % 4) * 16_384) as u16,
                    scale: 1.0,
                },
                tint: palette[i % palette.len()],
                alpha: 1.0,
                pose_index: i as u32,
            }
        })
        .collect()
}

/// The §9.2 default axis: garage (6+1, the garage's own tables),
/// corner (24+2, the corner's own tables), 1k, 4k (the cap).
pub fn census_populations() -> Vec<PopulationSpec> {
    let mut specs = Vec::new();
    {
        let (npc, vehicles) = crate::garage_scene::garage_population();
        specs.push(PopulationSpec {
            label: "garage (6+1)",
            npc,
            vehicles,
            stander_mix: crate::garage_scene::garage_phases(),
        });
    }
    {
        let (npc, vehicles) = crate::corner_population::corner_population();
        specs.push(PopulationSpec {
            label: "corner (24+2)",
            npc,
            vehicles,
            stander_mix: corner_mix_pattern(),
        });
    }
    for count in [1000_usize, MAX_INSTANCES] {
        specs.push(PopulationSpec {
            label: if count == MAX_INSTANCES {
                "4k (at cap)"
            } else {
                "1k"
            },
            npc: synthetic_population(count),
            vehicles: Vec::new(),
            stander_mix: corner_mix_pattern(),
        });
    }
    specs
}

/// Runs the whole §9.2 matrix and prints the table (µs/frame per
/// cell; the §0.1 carve row printed once as the §9.3 context).
/// `warmup` frames then `samples` measured frames per cell, the
/// `benchmark::report` convention. Returns the rows for callers
/// that assert over them (the lib tests' structure legs).
pub fn run_repack_matrix(
    populations: &[PopulationSpec],
    warmup: usize,
    samples: usize,
) -> Vec<MatrixRow> {
    println!("P5 R0 §9.2 — the repack matrix (BEFORE R2): CPU us/frame per operation");
    println!(
        "  carve context (§9.3 BEFORE — demand-independent): instance {} B / pose {} B / region {} B = {} B total",
        crate::census::carve::mapped_carves().instance_bytes,
        crate::census::carve::mapped_carves().pose_bytes,
        crate::census::carve::mapped_carves().region_bytes,
        crate::census::carve::mapped_carves().total_bytes,
    );
    println!(
        "  per-cell: instance-repack / pose-evaluation / slot-write (sink copy of the frame's exact bytes) — {samples} samples after {warmup} warmup"
    );
    let mut rows = Vec::new();
    for spec in populations {
        for fraction in [
            DirtyFraction::Zero,
            DirtyFraction::StanderMix,
            DirtyFraction::Full,
        ] {
            rows.push(run_cell(spec, fraction, warmup, samples));
        }
    }
    println!(
        "  boundary (§9.7): repack at {} ok; {} refused; poses at {} ok; {} refused",
        MAX_INSTANCES,
        MAX_INSTANCES + 1,
        MAX_POSE_BLOCKS,
        MAX_POSE_BLOCKS + 1
    );
    rows
}

/// One cell: the three operations measured over the advancing demo
/// clock (frame k — the walkers' clip times advance exactly the way
/// the composed route's do; standing NPCs read the cached block).
fn run_cell(
    spec: &PopulationSpec,
    fraction: DirtyFraction,
    warmup: usize,
    samples: usize,
) -> MatrixRow {
    let phases = phases_for(spec.npc.len(), fraction, &spec.stander_mix);
    let walkers = phases
        .iter()
        .filter(|phase| matches!(phase, Phase::Walk(_)))
        .count();
    let standers = phases.len() - walkers;

    // The retained buffers (the F-05 discipline the frame path
    // itself uses — clear + reserve + refill, capacity reused).
    let mut instance_bytes: Vec<u8> = Vec::new();
    let mut poses: Vec<Block> = Vec::new();
    let mut pose_bytes: Vec<u8> = Vec::new();
    // The slot-write sinks: carve-sized destinations the frame's
    // bytes copy into (the mapped regions' byte counts).
    let mut instance_sink = vec![0_u8; MAX_INSTANCES * INSTANCE_STRIDE];
    let mut pose_sink = vec![0_u8; MAX_POSE_BLOCKS * POSE_STRIDE];
    let mut indirect_sink = [0_u8; INDIRECT_SLOT_BYTES];
    let (npc_vertices, vehicle_vertices, rain_vertices) = proxy_mesh_vertex_counts();

    let mut instance_times: Vec<Duration> = Vec::with_capacity(warmup + samples);
    let mut pose_times: Vec<Duration> = Vec::with_capacity(warmup + samples);
    let mut write_times: Vec<Duration> = Vec::with_capacity(warmup + samples);
    for frame in 0..warmup + samples {
        // Operation 1 — the instance repack (whole-set, always).
        let started = Instant::now();
        repack_populations(&mut instance_bytes, &spec.npc, &spec.vehicles, &[])
            .expect("the census populations are valid by construction");
        instance_times.push(started.elapsed());
        // Operation 2 — the pose evaluation (standers read the
        // cached rest block; walkers pay the full sample+compose).
        let started = Instant::now();
        evaluate_population_poses(frame as u64, &phases, &mut poses);
        pose_times.push(started.elapsed());
        repack_poses(&mut pose_bytes, &poses)
            .expect("the census populations are within the pose cap");
        // Operation 3 — the slot write: the frame's exact byte
        // counts copied into the sink regions (the memcpy shape
        // upload_frame_data performs per populated frame), plus the
        // 48-B indirect block's pack + copy (NEW-REND-08's
        // cached-indirect observation — the block is a pure
        // function of the per-kind counts, repacked every frame).
        // The sinks pass through `black_box` FIRST: the production
        // write copies into Vulkan-mapped memory the compiler
        // cannot see, and an eliminator would silently delete a
        // local dead store — the opaque handle is what makes the
        // sink the honest surrogate.
        let started = Instant::now();
        let instance_region =
            std::hint::black_box(instance_sink.as_mut_slice())[..instance_bytes.len()].as_mut();
        instance_region.copy_from_slice(&instance_bytes);
        let pose_region =
            std::hint::black_box(pose_sink.as_mut_slice())[..pose_bytes.len()].as_mut();
        pose_region.copy_from_slice(&pose_bytes);
        let block = engine_renderer::proxy_indirect::pack_indirect_block(
            npc_vertices,
            spec.npc.len() as u32,
            vehicle_vertices,
            spec.vehicles.len() as u32,
            rain_vertices,
            0,
        );
        std::hint::black_box(indirect_sink.as_mut_slice()).copy_from_slice(&block);
        write_times.push(started.elapsed());
    }
    instance_times.drain(..warmup);
    pose_times.drain(..warmup);
    write_times.drain(..warmup);

    let row = MatrixRow {
        population: spec.label,
        dirty: fraction.label(),
        walkers,
        standers,
        instance_repack: timing_stats(&mut instance_times),
        pose_evaluation: timing_stats(&mut pose_times),
        slot_write: timing_stats(&mut write_times),
        instance_bytes: instance_bytes.len(),
        pose_bytes: pose_bytes.len(),
    };
    println!(
        "  {} x {}: {}/{} walk/stand — repack {:.2} / pose {:.2} / write {:.2} us \
         (p99 {:.2}/{:.2}/{:.2}; inst {} B, pose {} B)",
        row.population,
        row.dirty,
        row.walkers,
        row.standers,
        row.instance_repack.median_us,
        row.pose_evaluation.median_us,
        row.slot_write.median_us,
        row.instance_repack.p99_us,
        row.pose_evaluation.p99_us,
        row.slot_write.p99_us,
        row.instance_bytes,
        row.pose_bytes
    );
    row
}

/// §9.2's default populations at the rig's sample counts (200
/// warmup + 200 measured — the E-8 baseline battery's convention
/// for CPU-side rows; the 4k pose cells run ~ms per sample in
/// release, so the whole table is a few seconds).
pub fn run_default_matrix() -> Vec<MatrixRow> {
    run_repack_matrix(&census_populations(), 200, 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The byte-count pins: every population's packed bytes are
    /// exactly (npc + vehicles) × 96 instance bytes and npc × 704
    /// pose bytes — the slot-write row's variables, and the §0.1
    /// arithmetic's demand-side numbers (the corner's ~19 KB of
    /// regenerated bytes vs the 6.7 MB carve).
    #[test]
    fn cell_byte_counts_are_the_packed_strides() {
        let populations = census_populations();
        let row = run_repack_matrix(&populations, 1, 2);
        assert_eq!(row.len(), 4 * 3, "4 populations x 3 dirty fractions");
        let garage = &row[0];
        assert_eq!(garage.instance_bytes, 7 * INSTANCE_STRIDE, "6 npc + 1 van");
        assert_eq!(garage.pose_bytes, 6 * POSE_STRIDE);
        let corner = &row[3];
        assert_eq!(
            corner.instance_bytes,
            26 * INSTANCE_STRIDE,
            "24 npc + 2 vehicles"
        );
        assert_eq!(corner.pose_bytes, 24 * POSE_STRIDE);
        let four_k = &row[9];
        assert_eq!(four_k.instance_bytes, MAX_INSTANCES * INSTANCE_STRIDE);
        assert_eq!(four_k.pose_bytes, MAX_POSE_BLOCKS * POSE_STRIDE);
    }

    /// The dirty axis is honest: the stander-mix cells carry each
    /// table's OWN authored counts (garage 2/4, corner 8/16 — the
    /// design's "~40%" label is the stander share rounded; the
    /// tables' actual counts are what run), and the 0%/100% cells
    /// are all-stand/all-walk.
    #[test]
    fn dirty_fractions_are_the_authored_mixes() {
        let populations = census_populations();
        let rows = run_repack_matrix(&populations, 1, 1);
        let garage_mix = &rows[1];
        assert_eq!((garage_mix.standers, garage_mix.walkers), (2, 4));
        let corner_mix = &rows[4];
        assert_eq!((corner_mix.standers, corner_mix.walkers), (8, 16));
        assert_eq!(rows[0].walkers, 0, "the 0% cell is all standing");
        assert_eq!(rows[2].walkers, 6, "the garage 100% cell is all walking");
        assert_eq!(rows[5].standers, 0, "the 100% cell is all walking");
    }

    /// §9.7's boundary leg: 4k is EXACTLY the cap and cap+1 refuses
    /// with the typed errors, before any byte exists (the D2
    /// no-partial-overflow discipline; the same legs the existing
    /// capacity-refusal tests pin, restated at the census's axis
    /// point so the battery's printed boundary line is backed by an
    /// assertion).
    #[test]
    fn at_cap_repacks_and_above_cap_refuses() {
        let mut buffer = Vec::new();
        let at_cap = synthetic_population(MAX_INSTANCES);
        repack_populations(&mut buffer, &at_cap, &[], &[]).expect("exactly the cap packs");
        assert_eq!(buffer.len(), MAX_INSTANCES * INSTANCE_STRIDE);
        let above = synthetic_population(MAX_INSTANCES + 1);
        match repack_populations(&mut buffer, &above, &[], &[]) {
            Err(engine_renderer::proxy_instances::InstancePackError::Capacity {
                capacity,
                requested,
            }) => {
                assert_eq!(capacity, MAX_INSTANCES);
                assert_eq!(requested, MAX_INSTANCES + 1);
            }
            other => panic!("the typed capacity refusal, not {other:?}"),
        }
        assert_eq!(
            buffer.len(),
            MAX_INSTANCES * INSTANCE_STRIDE,
            "the refusal left the previous frame's bytes intact (atomic repack)"
        );
        let rest = engine_renderer::animation::rest_pose_block();
        let blocks: Vec<Block> = (0..MAX_POSE_BLOCKS).map(|_| rest).collect();
        let mut pose_buffer = Vec::new();
        repack_poses(&mut pose_buffer, &blocks).expect("exactly the cap packs");
        let above_blocks: Vec<Block> = (0..MAX_POSE_BLOCKS + 1).map(|_| rest).collect();
        match repack_poses(&mut pose_buffer, &above_blocks) {
            Err(engine_renderer::animation::PosePackError::Capacity {
                capacity,
                requested,
            }) => {
                assert_eq!(capacity, MAX_POSE_BLOCKS);
                assert_eq!(requested, MAX_POSE_BLOCKS + 1);
            }
            other => panic!("the typed pose capacity refusal, not {other:?}"),
        }
    }

    /// The synthetic populations are VALID (the repack's own
    /// validation accepts every axis point — the battery's
    /// `.expect` is backed by this pin).
    #[test]
    fn synthetic_populations_validate() {
        let mut buffer = Vec::new();
        for count in [6_usize, 24, 1000, MAX_INSTANCES] {
            let population = synthetic_population(count);
            repack_populations(&mut buffer, &population, &[], &[])
                .unwrap_or_else(|e| panic!("the {count}-npc synthetic population is valid: {e:?}"));
            assert_eq!(buffer.len(), count * INSTANCE_STRIDE);
        }
    }
}
