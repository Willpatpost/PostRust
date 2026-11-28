use engine_core::npc_behavior::{NpcId, NpcIdentity};
use engine_core::npc_world_save::{NpcRegionBlock, NpcWorldSectionV2};
use engine_core::{Decoder, Encoder, LodLevel, Tick, WorldConfig};

fn main() {
    let mut world = NpcWorldSectionV2 {
        seed: 1,
        activity_period: 6,
        simplify_factor: 4,
        commitment_window: 12,
        region_lods_enabled: true,
        config: WorldConfig {
            region_count: 1,
            npc_count: 1,
            region_grid: 4,
        },
        regions: vec![NpcRegionBlock {
            near: true,
            lod: LodLevel::Full,
            identities: vec![NpcIdentity {
                id: NpcId(0),
                home_cell: (0, 0),
                region: 0,
                appearance_seed: 1,
            }],
            steps: vec![0],
            routine_cursor: vec![0],
            positions: vec![(0, 0)],
            represented_through: vec![Tick(0)],
            milestones: vec![0],
            pending_signals: vec![0],
            credits: vec![0],
        }],
    };
    for position in [(0, 0), (4, 0)] {
        world.regions[0].positions[0] = position;
        let admitted = world.validate().is_ok();
        let mut bytes = Encoder::new();
        world.save(&mut bytes);
        let restored = NpcWorldSectionV2::restore(&mut Decoder::new(bytes.as_bytes()));
        println!(
            "position={position:?} admitted={admitted} restore={:?}",
            restored.as_ref().err()
        );
        assert_eq!(
            admitted,
            restored.is_ok(),
            "the in-memory and wire gates disagree"
        );
    }
}
