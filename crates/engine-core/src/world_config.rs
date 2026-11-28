//! The world-configuration boundary (P2 L8, A05): the population and
//! partition parameters a world carries as CONFIGURATION — plain data
//! authored by the application — rather than engine-wide constants.
//!
//! **Why this module exists (the A05 finding, verified by grep):** the
//! demo's specific values (`REGION_COUNT = 8`, `NPC_COUNT = 1000`,
//! `REGION_GRID = 16`) lived as engine-core constants documented as
//! "the shared wire-format bound", so every layer read ONE constant —
//! and a world at any other scale was unrepresentable. The audit of
//! this tree found the engine MECHANISMS already consume configured
//! values: the spine is world-agnostic (event classes are `u16`
//! policy, never partitioned by these constants), the perception
//! census walks the caller-presented population slice, the memory
//! lane indexes witness ids — none of them reads the constants. The
//! only engine-core consumers were the v1 save codec
//! (`npc_world_save.rs`) and, through re-exports, the demo itself.
//! The boundary work is therefore the FORMAT (a versioned section
//! that carries the parameters as data — [`crate::NpcWorldSectionV2`])
//! and the DEMO'S AUTHORSHIP (engine-slice's `npc` module defines its
//! own values, equal to the v1 interpretation so every historical
//! save stays byte-compatible).
//!
//! The constants at the crate root (`REGION_COUNT`, `NPC_COUNT`,
//! `REGION_GRID`, `POSITION_DISCARDED`) remain exported,
//! re-documented as the **v1 wire formats' fixed interpretation** — what a v1 save
//! (`engine-slice/npc-world-save/1`) and the v1 region-director
//! section encode BY DEFINITION, and what the persistence layer's
//! session-anchor decode validates against. They are wire-format
//! data, not world policy; [`WorldConfig::V1_FORMAT`] names the same
//! triple as a configuration value for parity pinning.
//!
//! Validation is fail-closed (the numerical-domain contract the
//! charter requires of every boundary): a zero region count, a
//! partition wider than the identity codec's region field, a zero
//! population, or a zero grid side is refused, never repaired.

/// The maximum number of regions a [`WorldConfig`] admits. The bound
/// is the existing `NpcIdentity` wire codec's own: its `region` field
/// is a `u8` (B12's durable-lane codec, which the v2 save format
/// carries verbatim), so a region index lives in `0..=255` and the
/// partition at most 256 regions. A wider partition needs a
/// versioned identity codec — recorded here as the boundary, never
/// repaired by truncation.
pub const MAX_REGION_COUNT: u32 = 256;

/// The maximum population a [`WorldConfig`] admits: the `NpcId`
/// identity space (`u32`), since ids are dense over
/// `0..npc_count`. The wire reader bounds every per-region block
/// count by the remaining bytes as well, so a corrupted count is
/// rejected before any allocation trusts it.
pub const MAX_NPC_COUNT: u32 = u32::MAX;

/// The world's population and partition parameters as plain data —
/// the configuration the application authors at world construction
/// and the `npc-world-save/2` section carries on the wire (A05: the
/// demo workload policy moved out of engine-wide constants). The
/// behavioral knobs (seed, activity period, cadence factors) were
/// always wire data, never constants; they stay section fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldConfig {
    /// The number of logical regions in the partition. Bounded by
    /// [`MAX_REGION_COUNT`] (the identity codec's region field).
    pub region_count: u32,
    /// The number of stable NPC identities — the dense identity set
    /// the per-region state columns are positional over. Bounded by
    /// [`MAX_NPC_COUNT`] (the `NpcId` space).
    pub npc_count: u32,
    /// The side length of the per-NPC region-local position grid.
    /// The grid is region-local and bounded BY DESIGN (never a
    /// world-space dense allocation); the dormant-discard sentinel
    /// `POSITION_DISCARDED = (u8::MAX, u8::MAX)` stays strictly
    /// off-grid at every admitted size.
    pub region_grid: u8,
}

/// Rejection reasons for an invalid [`WorldConfig`] (fail-closed; the
/// numerical-domain contract at the configuration boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldConfigError {
    /// `region_count` was zero — a world with no regions is not a
    /// workload, and refusing beats inventing one.
    RegionCountZero,
    /// `region_count` exceeded [`MAX_REGION_COUNT`] — the identity
    /// codec's `u8` region field cannot address the partition.
    RegionCountTooLarge,
    /// `npc_count` was zero — the dense identity set is empty, which
    /// no section column layout represents.
    NpcCountZero,
    /// `region_grid` was zero — no on-grid position exists, so the
    /// bounds invariant would be vacuous.
    RegionGridZero,
}

impl WorldConfig {
    /// The v1 wire format's fixed interpretation as a configuration
    /// value: eight regions, 1,000 stable identities, a 16×16
    /// region-local grid. This is what a v1 save
    /// (`engine-slice/npc-world-save/1`) materializes BY DEFINITION —
    /// wire-format data, not world policy. The demo authors the same
    /// triple as its own workload values; the parity is pinned by the
    /// engine-slice boundary tests so every historical save stays
    /// byte-compatible.
    pub const V1_FORMAT: WorldConfig = WorldConfig {
        region_count: crate::REGION_COUNT as u32,
        npc_count: crate::NPC_COUNT as u32,
        region_grid: crate::REGION_GRID,
    };

    /// Validates the parameter set, fail-closed: non-zero region
    /// count within the identity codec's region-field bound, non-zero
    /// population within the `NpcId` space, non-zero grid side. A
    /// configuration outside the domain is refused, never repaired.
    pub fn validated(self) -> Result<Self, WorldConfigError> {
        if self.region_count == 0 {
            return Err(WorldConfigError::RegionCountZero);
        }
        if self.region_count > MAX_REGION_COUNT {
            return Err(WorldConfigError::RegionCountTooLarge);
        }
        if self.npc_count == 0 {
            return Err(WorldConfigError::NpcCountZero);
        }
        if self.region_grid == 0 {
            return Err(WorldConfigError::RegionGridZero);
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_v1_interpretation_is_the_documented_triple() {
        // The parity anchor: the configuration view of the v1 wire
        // format equals the constants the v1 codec (and the persist
        // side's anchor decode) read — one triple, two views.
        assert_eq!(
            WorldConfig::V1_FORMAT.region_count,
            crate::REGION_COUNT as u32
        );
        assert_eq!(WorldConfig::V1_FORMAT.npc_count, crate::NPC_COUNT as u32);
        assert_eq!(WorldConfig::V1_FORMAT.region_grid, crate::REGION_GRID);
        assert_eq!(
            WorldConfig::V1_FORMAT,
            WorldConfig {
                region_count: 8,
                npc_count: 1000,
                region_grid: 16,
            }
        );
        assert!(WorldConfig::V1_FORMAT.validated().is_ok());
    }

    #[test]
    fn the_admitted_domain_is_bounded_on_every_axis() {
        // The edges: one region / one NPC / grid side one is a
        // minimal world; the identity-codec bounds (256 regions, the
        // u32 id space) and the widest grid side are admitted.
        let minimal = WorldConfig {
            region_count: 1,
            npc_count: 1,
            region_grid: 1,
        };
        assert_eq!(minimal.validated(), Ok(minimal));
        let widest_partition = WorldConfig {
            region_count: MAX_REGION_COUNT,
            npc_count: 1,
            region_grid: 1,
        };
        assert_eq!(widest_partition.validated(), Ok(widest_partition));
        let widest_grid = WorldConfig {
            region_count: 1,
            npc_count: 1,
            region_grid: u8::MAX,
        };
        assert_eq!(widest_grid.validated(), Ok(widest_grid));
        let largest_population = WorldConfig {
            region_count: 1,
            npc_count: MAX_NPC_COUNT,
            region_grid: 1,
        };
        assert_eq!(largest_population.validated(), Ok(largest_population));
    }

    #[test]
    fn invalid_configurations_are_refused_never_repaired() {
        let base = WorldConfig::V1_FORMAT;
        for config in [
            WorldConfig {
                region_count: 0,
                ..base
            },
            WorldConfig {
                region_count: MAX_REGION_COUNT + 1,
                ..base
            },
            WorldConfig {
                npc_count: 0,
                ..base
            },
            WorldConfig {
                region_grid: 0,
                ..base
            },
        ] {
            assert!(
                config.validated().is_err(),
                "the configuration {config:?} is outside the admitted domain"
            );
        }
        assert_eq!(
            WorldConfig {
                region_count: 0,
                ..base
            }
            .validated(),
            Err(WorldConfigError::RegionCountZero)
        );
        assert_eq!(
            WorldConfig {
                region_count: MAX_REGION_COUNT + 1,
                ..base
            }
            .validated(),
            Err(WorldConfigError::RegionCountTooLarge)
        );
        assert_eq!(
            WorldConfig {
                npc_count: 0,
                ..base
            }
            .validated(),
            Err(WorldConfigError::NpcCountZero)
        );
        assert_eq!(
            WorldConfig {
                region_grid: 0,
                ..base
            }
            .validated(),
            Err(WorldConfigError::RegionGridZero)
        );
    }
}
