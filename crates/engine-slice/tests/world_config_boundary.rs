//! P2 L8 (A05/B12): the world-configuration boundary, from the DEMO'S
//! side — the tests that pin the demo's authorship of its own
//! workload values and the live world's state flowing through the
//! configured `npc-world-save/2` format.
//!
//! * **The parity pin:** the demo's authored constants equal the v1
//!   wire format's fixed interpretation (`WorldConfig::V1_FORMAT`) —
//!   the byte-compatibility contract that keeps every historical
//!   save loading and every frozen fingerprint identical while the
//!   engine stops exporting the demo's numbers as the only truth.
//! * **The configured round-trip:** a LIVE `NpcWorld`'s authoritative
//!   state (real executed steps, milestones, credits, dormant
//!   sentinels after a far-presence deactivation) assembled into a v2
//!   section at the corner's configured values, saved and restored
//!   through the configured format — including the record-only
//!   dormant blocks.
//! * **The domain story:** the demo's production saves stay v1
//!   (`engine-slice/npc-world-save/1`) and the v2 fixed world refuses
//!   them by domain — the explicit dispatch, never a silent format
//!   change.

use engine_core::npc_behavior::{NpcId, NpcIdentity};
use engine_core::{
    Decoder, Diagnostics, Encoder, Level, LodLevel, NpcRegionBlock, NpcSaveWorldV2,
    NpcWorldSectionV2, PresenceSample, SaveError, Spine, WorldConfig,
};
use engine_slice::npc::{
    NPC_COUNT, NpcConfig, NpcWorld, REGION_COUNT, REGION_GRID, install_initial_schedules,
};
use engine_slice::region::RegionConfig;
use engine_slice::sim::{SimConfig, SimDriver};

/// The demo's authored values ARE the v1 wire format's fixed
/// interpretation — the byte-compatibility contract (A05's "the corner
/// keeps its current numbers"): the demo's saves stay v1-shaped and
/// load exactly as before, while the same values flow into the
/// configured format as configuration.
#[test]
fn the_demo_authors_the_v1_format_values() {
    assert_eq!(
        WorldConfig::V1_FORMAT,
        WorldConfig {
            region_count: REGION_COUNT as u32,
            npc_count: NPC_COUNT as u32,
            region_grid: REGION_GRID,
        },
        "the demo's authored workload equals the v1 wire interpretation"
    );
    // The engine no longer claims these as the only truth: the same
    // configured format admits other workloads (pinned in engine-core's
    // own battery at 24, 300, and 10,000 identities).
    assert!(WorldConfig::V1_FORMAT.validated().is_ok());
}

/// Assembles the LIVE world's authoritative state into a v2 section at
/// the demo's configured values: region-major blocks by the demo's own
/// partition rule (`npc % REGION_COUNT`), the identity table with
/// deterministic durable facets, and the STRUCTURAL dormant shape — a
/// Dormant region's block carries only the record columns (the live
/// world's discard sentinels become the format's omitted
/// representation).
fn configured_section(world: &NpcWorld) -> NpcWorldSectionV2 {
    let lods = world.region_lods();
    let mut regions = Vec::new();
    for (region, &lod) in lods.iter().enumerate() {
        let members = (0..NPC_COUNT)
            .filter(|npc| npc % REGION_COUNT == region)
            .collect::<Vec<_>>();
        // The durable facets: the stable id, an on-grid home cell, the
        // region membership, a derived appearance seed (the
        // placement-table pattern — deterministic, never wall-clock).
        let identities = members
            .iter()
            .map(|&npc| NpcIdentity {
                id: NpcId(npc as u32),
                home_cell: (
                    (npc % usize::from(REGION_GRID)) as u8,
                    ((npc / usize::from(REGION_GRID)) % usize::from(REGION_GRID)) as u8,
                ),
                region: region as u8,
                appearance_seed: (npc as u32).wrapping_mul(2_654_435_761),
            })
            .collect::<Vec<_>>();
        let record = |column: &[u64]| members.iter().map(|&npc| column[npc]).collect::<Vec<_>>();
        let block = NpcRegionBlock {
            near: world.config().near_regions[region],
            lod,
            identities,
            steps: record(world.steps()),
            milestones: record(world.milestones()),
            credits: record(world.credits()),
            routine_cursor: if lod == LodLevel::Dormant {
                Vec::new()
            } else {
                record(world.routine_cursor())
            },
            positions: if lod == LodLevel::Dormant {
                Vec::new()
            } else {
                members
                    .iter()
                    .map(|&npc| world.positions()[npc])
                    .collect::<Vec<_>>()
            },
            represented_through: if lod == LodLevel::Dormant {
                Vec::new()
            } else {
                members
                    .iter()
                    .map(|&npc| world.represented_through()[npc])
                    .collect::<Vec<_>>()
            },
            pending_signals: if lod == LodLevel::Dormant {
                Vec::new()
            } else {
                record(world.pending_signals())
            },
        };
        regions.push(block);
    }
    NpcWorldSectionV2 {
        seed: world.config().seed,
        activity_period: world.config().activity_period,
        simplify_factor: world.config().simplify_factor,
        commitment_window: world.config().commitment_window,
        region_lods_enabled: world.config().region_lods_enabled,
        config: WorldConfig {
            region_count: REGION_COUNT as u32,
            npc_count: NPC_COUNT as u32,
            region_grid: REGION_GRID,
        },
        regions,
    }
}

#[test]
fn the_live_world_state_round_trips_through_the_configured_v2_section() {
    // The demo's REAL state through the configured format: active
    // accrual (executed steps, milestone crossings — the consequential
    // arrays nonzero on the wire), then a far-presence deactivation
    // (capture-before-discard — the scratch captured, the
    // reconstructible representation discarded), each snapshot
    // assembled into a v2 section and round-tripped exactly.
    let near = PresenceSample {
        position: (100.0, 0.0),
        velocity: (0.0, 0.0),
    };
    let far = PresenceSample {
        position: (2000.0, 2000.0),
        velocity: (0.0, 0.0),
    };
    let mut driver = SimDriver::with_regions(
        NpcConfig::default(),
        SimConfig::default(),
        RegionConfig::default(),
        &[near],
    )
    .expect("the driver constructs");
    driver.run_headless(150);
    let active_world = driver.spine().world();
    assert!(
        active_world.steps().iter().any(|step| *step > 0),
        "the active regions really executed"
    );
    assert!(
        active_world.milestones().iter().any(|m| *m > 0),
        "the fixture crossed milestone boundaries"
    );

    // The ACTIVE snapshot: mixed blocks (the player-proximate regions
    // Full, the distant ones dormant since construction — their
    // record-only blocks carry the sentinel-equivalent omission).
    let active_section = configured_section(active_world);
    assert_eq!(active_section.config, WorldConfig::V1_FORMAT);
    active_section
        .validate()
        .expect("the live section is consistent");
    let mut enc = Encoder::new();
    active_section.save(&mut enc);
    let bytes = enc.into_bytes();
    let mut dec = Decoder::new(&bytes);
    let restored = NpcWorldSectionV2::restore(&mut dec).unwrap();
    dec.finish().expect("the section consumed exactly");
    assert_eq!(restored, active_section);
    // The live per-identity state survived the configured format
    // exactly (spot-check across regions).
    let flattened = restored
        .regions
        .iter()
        .flat_map(|block| {
            block
                .identities
                .iter()
                .zip(&block.steps)
                .map(|(identity, step)| (identity.id.0, *step))
        })
        .collect::<Vec<_>>();
    for npc in [0usize, 1, 7, 123, 500, 999] {
        let step = flattened
            .iter()
            .find(|&&(id, _)| id == npc as u32)
            .map(|&(_, step)| step);
        assert_eq!(step, Some(active_world.steps()[npc]));
    }

    // The DORMANT snapshot: everything deactivated (the scratch
    // captured and cleared, the reconstructible representation
    // discarded) — every block record-only, and the consequential
    // record columns still on the wire.
    driver.set_presence(vec![far]);
    driver.run_headless(5);
    let dormant_world = driver.spine().world();
    assert_eq!(
        dormant_world.pending_signals().iter().sum::<u64>(),
        0,
        "the scratch was captured and cleared"
    );
    let dormant_section = configured_section(dormant_world);
    dormant_section
        .validate()
        .expect("the dormant section is consistent");
    assert!(
        dormant_section
            .regions
            .iter()
            .all(|block| block.lod == LodLevel::Dormant)
    );
    let mut enc = Encoder::new();
    dormant_section.save(&mut enc);
    let bytes = enc.into_bytes();
    let mut dec = Decoder::new(&bytes);
    let restored = NpcWorldSectionV2::restore(&mut dec).unwrap();
    dec.finish().expect("the section consumed exactly");
    assert_eq!(restored, dormant_section);
    for block in &restored.regions {
        // The structural dormant shape: the record columns survive,
        // the reconstructible columns are gone.
        assert!(block.routine_cursor.is_empty());
        assert!(block.positions.is_empty());
        assert!(block.represented_through.is_empty());
        assert_eq!(block.milestones.len(), block.identities.len());
    }
    // The once-active regions' irreversible consequences survived the
    // deactivation and the configured format (the regions dormant
    // since construction honestly carry zero milestones).
    assert!(
        restored
            .regions
            .iter()
            .any(|block| block.milestones.iter().any(|milestone| *milestone > 0))
    );
}

#[test]
fn the_production_save_domain_stays_v1_and_the_v2_world_refuses_it() {
    // The domain story from the demo's side: the production world's
    // saves remain v1 (byte-compatible — the demo authors the v1
    // interpretation's values), they load through the production
    // world exactly as before, and the v2 fixed world REFUSES them by
    // domain — the explicit dispatch, never a silent reinterpretation.
    let config = NpcConfig {
        region_lods_enabled: true,
        ..NpcConfig::default()
    };
    let mut spine = Spine::with_reservation(NpcWorld::new(config.clone()), 200, 60);
    install_initial_schedules(&mut spine, &config);
    spine.run_ticks(150);
    let bytes = spine.save_bytes();
    let restored = Spine::<NpcWorld>::restore(&bytes, Diagnostics::new(Level::Off))
        .expect("the v1 save loads");
    assert_eq!(restored.save_bytes(), bytes);
    let error = Spine::<NpcSaveWorldV2>::restore(&bytes, Diagnostics::new(Level::Off))
        .err()
        .expect("the v2 world refuses a v1 save");
    assert!(matches!(
        error,
        SaveError::Domain {
            expected: "engine-slice/npc-world-save/2",
            ..
        }
    ));
}
