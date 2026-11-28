//! (V2-W F4, the R15 world-validator authority) The NPC world's save
//! section — the shared, statically linked interpretation of the
//! spine save's world-section wire format.
//!
//! **Why this module exists (the R15 closure):** the authenticated
//! rotation anchor must validate the newest session snapshot's spine
//! under the PRODUCTION world's own rules — a hash-verified object
//! with a valid director summary but an invalid world section is not
//! valid retirement evidence (the R14-A finding), and the validator
//! that authenticates retirement evidence must be the validator
//! belonging to the campaign's actual persisted world schema, never a
//! freely selected caller implementation (the R15 finding: the
//! public generic rotation entry let any external crate substitute a
//! weaker `SaveWorld` that declares the production domain — the
//! bypass was demonstrated through the real public API and is now
//! UNREPRESENTABLE there: the entry takes no world parameter, and
//! the anchor restores through [`NpcSaveWorld`], whose acceptance is
//! exactly this section codec's acceptance).
//!
//! **The parity is by construction, not by mirroring:** the
//! application's [`crate::SaveWorld`] implementation for its
//! `NpcWorld` DELEGATES its restore to
//! [`NpcWorldSection::restore`] (the same function this module
//! defines), and the anchor's spine validation runs the production
//! `Spine::<W>::restore` machinery itself with `W = NpcSaveWorld`
//! (also delegating here). One walk, two consumers — no
//! independently maintained interpretation can drift. This is the
//! same downward move the layout constants and `LodConfig::validated`
//! already made (the V2-W F4 authenticated-retention design): the
//! wire format's anchor-relevant rules live in engine-core, beside
//! the bounds (`NPC_COUNT`, `REGION_GRID`, `POSITION_DISCARDED`)
//! they parameterize; the application keeps its behavior (the
//! `World` event handling and the save-side encode) and the
//! persistence layer's anchor names only this module.
//!
//! The section is the world save's PAYLOAD codec (the bytes inside
//! the spine save's world-section framing — after the domain tag and
//! length prefix, which the spine's generic save/restore owns):
//! [`NpcWorldSection::restore`] consumes exactly the production
//! encoding or fails closed with the production error taxonomy, and
//! [`NpcWorldSection::save`] emits the same sequence (the
//! application's `NpcWorld::save` keeps its own verbatim encoder —
//! byte-identical, pinned by the round-trip parity tests — so no
//! clone is introduced on the save path).
//!
//! # npc-world-save/2 — the world-configuration boundary (P2 L8,
//! A05/B12)
//!
//! This module also owns the VERSIONED successor,
//! [`NpcWorldSectionV2`] (domain `engine-slice/npc-world-save/2`),
//! which carries the world's population/partition parameters as DATA
//! ([`crate::WorldConfig`]) instead of inheriting the v1 constants.
//! The versioning story is the repo's standing save discipline —
//! never a silent format change:
//!
//! * **v1 is retained loadable, byte-for-byte.** The codec above is
//!   untouched; a v1 save loads through any world declaring the v1
//!   domain (the demo's `NpcWorld`, the anchor's [`NpcSaveWorld`])
//!   and materializes the FIXED workload it encodes — eight regions,
//!   1,000 stable identities, a 16×16 region-local grid (the
//!   constants above, re-documented as the v1 wire interpretation —
//!   format data, not world policy).
//! * **v2 carries its own configuration.** Region-major blocks, a
//!   per-region identity table encoded with the EXISTING
//!   [`crate::npc_behavior::NpcIdentity`] wire codec (B12: the
//!   durable lane round-tripped through actual authoritative
//!   storage, not a codec self-test), and record-only dormant blocks
//!   (the occupied-only rule — no dense allocation proportional to
//!   the galaxy).
//! * **The dispatch is the domain tag**, exactly as for every other
//!   versioned domain: the spine's world-section framing reads the
//!   restoring world's `SAVE_DOMAIN` and refuses a mismatch by name.
//!   A v1 save never loads as v2 and vice versa — the forms reject
//!   one another, never silently reinterpreted.
//! * **No v1→v2 rewrite happens implicitly.** An explicit migrate
//!   utility is future opt-in work; it needs the application's
//!   region-assignment policy (the v1 section carries no per-NPC
//!   region mapping — `npc % REGION_COUNT` is demo policy, not wire
//!   data) and P3 sign-off for the anchor/journal surface that also
//!   reads world sections. Until then: load v1 as v1, v2 as v2.

use std::collections::BTreeSet;

use crate::clock::Tick;
use crate::event::Event;
use crate::fingerprint::Fingerprinter;
use crate::lod::LodLevel;
use crate::npc_behavior::NpcIdentity;
use crate::spine::{ExecCtx, World};
use crate::wire::{Decoder, Encoder, SaveError, SaveWorld};
use crate::world_config::{MAX_NPC_COUNT, MAX_REGION_COUNT, WorldConfig};
use crate::{NPC_COUNT, POSITION_DISCARDED, REGION_COUNT, REGION_GRID};

/// The NPC world save section's domain tag — versioned and stable,
/// the ONE string both the application's `NpcWorld::save` and the
/// anchor's fixed world declare (the spine's generic framing checks
/// it against the restoring world's `SAVE_DOMAIN`).
pub const NPC_WORLD_SAVE_DOMAIN: &str = "engine-slice/npc-world-save/1";

/// The NPC world's save section as plain data: the behavioral
/// configuration plus the authoritative per-NPC state (dense arrays
/// indexed by the stable identity set — ONE count governs every
/// array; the identity set is fixed, so a different count is an
/// incompatible workload, rejected at restore, never a resized
/// restore). The application's `NpcWorld` assembles itself from this
/// section; the anchor's [`NpcSaveWorld`] holds it restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpcWorldSection {
    /// Deterministic workload seed (behavioral identity).
    pub seed: u64,
    /// The near-region classification as a `REGION_COUNT`-bit mask
    /// (validated bounded at restore; the application expands it).
    pub near_mask: u64,
    /// Base activity period in ticks.
    pub activity_period: u64,
    /// The cadence multiplier at Simplified LOD.
    pub simplify_factor: u64,
    /// The commitment window at EventDriven LOD.
    pub commitment_window: u64,
    /// Whether the region-LOD handler path is enabled.
    pub region_lods_enabled: bool,
    /// Executed activity count per NPC — ACTUAL executions only.
    pub steps: Vec<u64>,
    /// Routine progression cursor per NPC (with discard sentinels
    /// while dormant).
    pub routine_cursor: Vec<u64>,
    /// Per-NPC position on the region-local grid (or the
    /// discarded-position sentinel while dormant).
    pub positions: Vec<(u8, u8)>,
    /// Per-NPC represented-through boundary.
    pub represented_through: Vec<Tick>,
    /// Real-execution milestones per NPC (irreversible
    /// consequences).
    pub milestones: Vec<u64>,
    /// Uncommitted consequential signals per NPC (scratch).
    pub pending_signals: Vec<u64>,
    /// Committed consequential signals per NPC.
    pub credits: Vec<u64>,
    /// The handler's current LOD view per region.
    pub region_lods: [LodLevel; REGION_COUNT],
}

impl NpcWorldSection {
    /// Encodes the section — the same sequence the application's
    /// `NpcWorld::save` writes (the production encoder; this mirror
    /// exists so the fixed world's [`SaveWorld::save`] is total and
    /// the section round-trips under its own codec — the parity tests
    /// pin the two encoders together).
    pub fn save(&self, enc: &mut Encoder) {
        enc.write_u64(self.seed);
        enc.write_u64(self.near_mask);
        enc.write_u64(self.activity_period);
        enc.write_u64(self.simplify_factor);
        enc.write_u64(self.commitment_window);
        enc.write_u8(u8::from(self.region_lods_enabled));
        enc.write_count(NPC_COUNT);
        for step in &self.steps {
            enc.write_u64(*step);
        }
        for cursor in &self.routine_cursor {
            enc.write_u64(*cursor);
        }
        for position in &self.positions {
            enc.write_u8(position.0);
            enc.write_u8(position.1);
        }
        for boundary in &self.represented_through {
            enc.write_tick(*boundary);
        }
        for milestone in &self.milestones {
            enc.write_u64(*milestone);
        }
        for signal in &self.pending_signals {
            enc.write_u64(*signal);
        }
        for credit in &self.credits {
            enc.write_u64(*credit);
        }
        for lod in &self.region_lods {
            enc.write_u8(match lod {
                LodLevel::Full => 0,
                LodLevel::Simplified => 1,
                LodLevel::EventDriven => 2,
                LodLevel::Dormant => 3,
            });
        }
    }

    /// Restores the section (fail-closed): the exact production walk
    /// — every count bounded before allocation, every discriminant
    /// typed, the fixed identity set, the grid-position bounds with
    /// the dormant-discard sentinel, the near-mask bound — consuming
    /// exactly the production encoding or returning the production
    /// typed error. Malformed input is never repaired.
    pub fn restore(dec: &mut Decoder) -> Result<Self, SaveError> {
        const SECTION: &str = "npc-world-save";
        let seed = dec.read_u64()?;
        let near_mask = dec.read_u64()?;
        let activity_period = dec.read_u64()?;
        let simplify_factor = dec.read_u64()?;
        let commitment_window = dec.read_u64()?;
        let region_lods_enabled = match dec.read_u8()? {
            0 => false,
            1 => true,
            discriminant => {
                return Err(SaveError::Enum {
                    section: SECTION,
                    discriminant: u64::from(discriminant),
                });
            }
        };
        // The fixed identity set: the count is bounded by the
        // documented workload size and the remaining bytes before any
        // allocation trusts it, and must equal NPC_COUNT exactly — a
        // different identity set is an incompatible workload, never a
        // silently resized restore.
        let npc_count = dec.read_count(NPC_COUNT as u64, SECTION, "npc-count")?;
        if npc_count != NPC_COUNT as u64 {
            return Err(SaveError::Integrity("npc-world-save/npc-count"));
        }
        let mut steps = Vec::with_capacity(NPC_COUNT);
        for _ in 0..NPC_COUNT {
            steps.push(dec.read_u64()?);
        }
        let mut routine_cursor = Vec::with_capacity(NPC_COUNT);
        for _ in 0..NPC_COUNT {
            routine_cursor.push(dec.read_u64()?);
        }
        // Grid positions carry the dormant-discard sentinel
        // (`POSITION_DISCARDED` — the reconstructible representation
        // is GONE, not merely flagged); every other position must lie
        // on the region-local grid (the bounds invariant).
        let mut positions = Vec::with_capacity(NPC_COUNT);
        for _ in 0..NPC_COUNT {
            let x = dec.read_u8()?;
            let y = dec.read_u8()?;
            if (x, y) != POSITION_DISCARDED && (x >= REGION_GRID || y >= REGION_GRID) {
                return Err(SaveError::Integrity("npc-world-save/position-bounds"));
            }
            positions.push((x, y));
        }
        let mut represented_through = Vec::with_capacity(NPC_COUNT);
        for _ in 0..NPC_COUNT {
            represented_through.push(dec.read_tick()?);
        }
        let mut milestones = Vec::with_capacity(NPC_COUNT);
        for _ in 0..NPC_COUNT {
            milestones.push(dec.read_u64()?);
        }
        let mut pending_signals = Vec::with_capacity(NPC_COUNT);
        for _ in 0..NPC_COUNT {
            pending_signals.push(dec.read_u64()?);
        }
        let mut credits = Vec::with_capacity(NPC_COUNT);
        for _ in 0..NPC_COUNT {
            credits.push(dec.read_u64()?);
        }
        let mut region_lods = [LodLevel::Full; REGION_COUNT];
        for lod in &mut region_lods {
            *lod = match dec.read_u8()? {
                0 => LodLevel::Full,
                1 => LodLevel::Simplified,
                2 => LodLevel::EventDriven,
                3 => LodLevel::Dormant,
                discriminant => {
                    return Err(SaveError::Enum {
                        section: SECTION,
                        discriminant: u64::from(discriminant),
                    });
                }
            };
        }
        // The near-region mask is a REGION_COUNT-bit mask — set bits
        // above it are not this workload's configuration.
        if near_mask >> REGION_COUNT != 0 {
            return Err(SaveError::Integrity("npc-world-save/near-mask"));
        }
        Ok(NpcWorldSection {
            seed,
            near_mask,
            activity_period,
            simplify_factor,
            commitment_window,
            region_lods_enabled,
            steps,
            routine_cursor,
            positions,
            represented_through,
            milestones,
            pending_signals,
            credits,
            region_lods,
        })
    }
}

/// The fixed world the authenticated anchor restores through: a
/// [`World`]/[`SaveWorld`] implementation whose section acceptance is
/// EXACTLY [`NpcWorldSection::restore`]'s (the same function the
/// application's production `NpcWorld::restore` delegates to) — so
/// `Spine::<NpcSaveWorld>::restore` accepts precisely the spine
/// saves the production `Spine::<NpcWorld>::restore` accepts, and
/// the anchor needs no caller-supplied world parameter at all. This
/// type exists for VALIDATION only: it never executes (a no-op
/// handler) and never fingerprints; it is never a live game world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpcSaveWorld(pub NpcWorldSection);

impl World for NpcSaveWorld {
    fn execute(&mut self, _event: &Event, _ctx: &mut ExecCtx) {}
    fn fingerprint(&self, _fp: &mut Fingerprinter) {}
}

impl SaveWorld for NpcSaveWorld {
    const SAVE_DOMAIN: &'static str = NPC_WORLD_SAVE_DOMAIN;

    fn save(&self, enc: &mut Encoder) {
        self.0.save(enc);
    }

    fn restore(dec: &mut Decoder) -> Result<Self, SaveError> {
        Ok(NpcSaveWorld(NpcWorldSection::restore(dec)?))
    }
}

/// The v2 NPC world save section's domain tag — the versioned
/// successor of [`NPC_WORLD_SAVE_DOMAIN`] (see the module's
/// versioning story: the two forms dispatch on this tag and reject
/// one another, never silently reinterpreted).
pub const NPC_WORLD_SAVE_DOMAIN_V2: &str = "engine-slice/npc-world-save/2";

/// Caller-configurable decode-admission limits for the v2 section
/// (CA-02/F02, Q07 — the resource-admission half of the L8 boundary):
/// what the DECODER may spend materializing a section, as a CALLER
/// decision distinct from what the WIRE admits.
///
/// **Why this type exists (the two-contracts finding):** the wire's
/// own validity bounds (`MAX_REGION_COUNT`, `MAX_NPC_COUNT` — the
/// identity codec's widths) say what a WELL-FORMED section may
/// declare; they say nothing about what THIS decoder can afford.
/// The v2 walk reserves every per-block column vector from the
/// block's identity count BEFORE reading the records, and the
/// generic count bound checks one byte per element while an identity
/// record occupies eleven wire bytes and eight in-memory columns
/// ride the same count — so a crafted section of `S` bytes could make
/// the decoder transiently reserve roughly `62 × S` bytes of heap
/// (a world section may be up to [`crate::wire::MAX_BYTE_LEN`]
/// bytes), aborting the process on an allocation failure rather
/// than refusing with a typed error. Valid wire ranges and
/// affordable decoder work are different contracts; this type is
/// the second one, authored by the composition boundary that knows
/// what it can afford.
///
/// **The scoped/global identity policy is NOT this type's decision**
/// (a separate owner question, deliberately out of scope): the v2
/// format's identity table is globally scoped — ids unique across
/// the whole section, blocks positional and region-major — and
/// whether a future format scopes identities per region is a
/// format-versioning decision, never a decode-limit decision.
///
/// The default ([`Self::WIRE_MAXIMUM`]) is exactly the wire's own
/// validity domain, so the plain [`NpcWorldSectionV2::restore`]
/// entry keeps today's acceptance byte for byte; composition
/// callers that decode less-trusted bytes pass smaller limits
/// through [`NpcWorldSectionV2::restore_with_limits`] and every
/// over-budget input becomes a typed [`SaveError::Limit`] refusal
/// BEFORE any reservation is made.
///
/// **The allocation-census boundary (an honest note):** the
/// thread-attributed counting-allocator census — the
/// `tests/spine_allocations.rs` / `tests/spatial_query_allocations.rs`
/// precedent — measures the decode path only from a dedicated
/// integration binary (a `#[global_allocator]` is one per binary),
/// which this codec's owning lanes did not add; the inline battery
/// proves the same property structurally (the typed pre-reservation
/// refusals plus the exact-capacity reservations — the reservation
/// sizes are observable on every decoded section), and the census
/// binary remains the natural follow-up if the composition boundary
/// adopting `restore_with_limits` wants allocator-level telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionDecodeLimits {
    /// The maximum total identity records (the declared
    /// `config.npc_count`) this decoder will materialize: the
    /// aggregate over-budget refusal. Must not exceed
    /// [`MAX_NPC_COUNT`] to have any effect (the config gate refuses
    /// larger declarations first).
    pub max_identities: u64,
    /// The maximum cumulative in-memory bytes the decode may reserve
    /// across the section's per-block column vectors (the transient
    /// reservation budget; the decoded section's own footprint is
    /// bounded by it by construction).
    pub max_reserve_bytes: u64,
}

impl SectionDecodeLimits {
    /// The wire-valid maximum: the identity codec's full domain and
    /// an unbounded reservation budget — exactly the acceptance the
    /// plain [`NpcWorldSectionV2::restore`] entry has always had.
    pub const WIRE_MAXIMUM: SectionDecodeLimits = SectionDecodeLimits {
        max_identities: MAX_NPC_COUNT as u64,
        max_reserve_bytes: u64::MAX,
    };

    /// The caller-configured limits from plain integers (the
    /// composition boundary's affordability decision; see the type
    /// doc for what each bounds).
    pub const fn new(max_identities: u64, max_reserve_bytes: u64) -> Self {
        SectionDecodeLimits {
            max_identities,
            max_reserve_bytes,
        }
    }
}

/// One region's block in the v2 section: the region-major unit the
/// world is partitioned into (a v2 save writes exactly
/// `config.region_count` of these, ascending region index).
///
/// The block carries the region's own configuration facet (`near`,
/// `lod`) and its identity table plus per-NPC state columns,
/// positional within the block: column `k` belongs to
/// `identities[k]`, whose global id is durable and unique across the
/// whole section. **The dormant shape is structural (the occupied-only
/// rule):** a block whose `lod` is [`LodLevel::Dormant`] encodes only
/// the record columns — `steps`, `milestones`, `credits` — and its
/// reconstructible columns (`routine_cursor`, `positions`,
/// `represented_through`, `pending_signals`) are EMPTY, on the wire
/// and in memory: the representation is GONE (the discard sentinels
/// of the v1 encoding, made structural), not merely flagged. The
/// save derives the block shape from `lod`, restore enforces the
/// invariant on every wire input, and
/// [`NpcWorldSectionV2::validate`] refuses an inconsistent in-memory
/// section before it is encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpcRegionBlock {
    /// Whether this region's events are scheduled into the near lane
    /// (v1's `REGION_COUNT`-bit near mask, per-region in v2 — no
    /// 64-region mask bound).
    pub near: bool,
    /// The handler's current LOD view for this region. `Dormant`
    /// selects the record-only block shape.
    pub lod: LodLevel,
    /// The region's identity table — the durable lane (B12),
    /// wire-encoded with the existing [`NpcIdentity`] codec. Ids are
    /// strictly ascending within the block and unique across the
    /// section; `identity.region` equals the block's region index and
    /// `identity.home_cell` is strictly on-grid (identity is durable —
    /// never the discard sentinel).
    pub identities: Vec<NpcIdentity>,
    /// Executed activity count per NPC — ACTUAL executions only.
    pub steps: Vec<u64>,
    /// Routine progression cursor per NPC (present only in the
    /// active shape; empty while dormant).
    pub routine_cursor: Vec<u64>,
    /// Per-NPC position on the region-local grid, or the
    /// discarded-position sentinel (active shape only; empty while
    /// dormant).
    pub positions: Vec<(u8, u8)>,
    /// Per-NPC represented-through boundary (active shape only;
    /// empty while dormant).
    pub represented_through: Vec<Tick>,
    /// Real-execution milestones per NPC (irreversible consequences —
    /// retained in the dormant shape).
    pub milestones: Vec<u64>,
    /// Uncommitted consequential signals per NPC — scratch, captured
    /// before discard (active shape only; empty while dormant).
    pub pending_signals: Vec<u64>,
    /// Committed consequential signals per NPC (retained in the
    /// dormant shape).
    pub credits: Vec<u64>,
}

/// The v2 NPC world's save section as plain data: the behavioral
/// configuration, the world's population/partition parameters AS DATA
/// ([`WorldConfig`] — A05's configuration boundary), and the
/// region-major blocks. See [`NpcRegionBlock`] for the block shape
/// and the module docs for the versioning/migration story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpcWorldSectionV2 {
    /// Deterministic workload seed (behavioral identity).
    pub seed: u64,
    /// Base activity period in ticks.
    pub activity_period: u64,
    /// The cadence multiplier at Simplified LOD.
    pub simplify_factor: u64,
    /// The commitment window at EventDriven LOD.
    pub commitment_window: u64,
    /// Whether the region-LOD handler path is enabled.
    pub region_lods_enabled: bool,
    /// THE CONFIGURATION — the population/partition parameters the v1
    /// format baked in as constants, carried as data (a v2 save loads
    /// its own workload; a v1 save loads the v1 interpretation).
    pub config: WorldConfig,
    /// The region-major blocks, ascending region index, exactly
    /// `config.region_count` of them.
    pub regions: Vec<NpcRegionBlock>,
}

impl NpcWorldSectionV2 {
    /// Encodes the section. The walk is the exact inverse of
    /// [`Self::restore`]: the header (the behavioral knobs, then the
    /// configuration as data), then `config.region_count` blocks in
    /// ascending region index, each writing the shape its `lod`
    /// selects — record-only for [`LodLevel::Dormant`]. Encoding an
    /// inconsistent section (a dormant block carrying non-empty
    /// reconstructible columns) writes the dormant shape: those
    /// values are gone by the dormancy discipline. This raw codec is
    /// TOTAL (the v1 mirror discipline — encoding is never repair):
    /// a caller that encodes an unvalidated section can write bytes
    /// [`Self::restore`] refuses; validate the section first
    /// ([`Self::validate`]) — or encode through the fallible
    /// validated path [`Self::save_validated`], which refuses before
    /// any byte is written.
    pub fn save(&self, enc: &mut Encoder) {
        enc.write_u64(self.seed);
        enc.write_u64(self.activity_period);
        enc.write_u64(self.simplify_factor);
        enc.write_u64(self.commitment_window);
        enc.write_u8(u8::from(self.region_lods_enabled));
        enc.write_count(self.config.region_count as usize);
        enc.write_count(self.config.npc_count as usize);
        enc.write_u8(self.config.region_grid);
        for block in &self.regions {
            enc.write_u8(u8::from(block.near));
            enc.write_u8(lod_discriminant(block.lod));
            enc.write_count(block.identities.len());
            // The identity table through the EXISTING NpcIdentity
            // codec — fixed 11-byte stride, no per-record prefix.
            for identity in &block.identities {
                for byte in identity.wire_bytes() {
                    enc.write_u8(byte);
                }
            }
            for step in &block.steps {
                enc.write_u64(*step);
            }
            if block.lod != LodLevel::Dormant {
                for cursor in &block.routine_cursor {
                    enc.write_u64(*cursor);
                }
                for position in &block.positions {
                    enc.write_u8(position.0);
                    enc.write_u8(position.1);
                }
                for boundary in &block.represented_through {
                    enc.write_tick(*boundary);
                }
                for signal in &block.pending_signals {
                    enc.write_u64(*signal);
                }
            }
            for milestone in &block.milestones {
                enc.write_u64(*milestone);
            }
            for credit in &block.credits {
                enc.write_u64(*credit);
            }
        }
    }

    /// The fallible validated encoding path for public callers (F06's
    /// wire half): validates the section — exactly
    /// [`Self::validate`]'s gates, including the shared position
    /// predicate — and only then encodes, so the bytes written
    /// through this path are exactly bytes [`Self::restore`] accepts,
    /// and a refusal happens before any byte is written. The raw
    /// [`Self::save`] remains the total mirror codec (the v1
    /// discipline: encoding is never repair, and the application's
    /// own `NpcWorld::save` keeps its verbatim encoder — byte-ident,
    /// pinned by the round-trip parity tests); a caller that saves an
    /// unvalidated section through it can still write bytes restore
    /// refuses. This path closes that hole for callers that opt in.
    pub fn save_validated(&self, enc: &mut Encoder) -> Result<(), SaveError> {
        self.validate()?;
        self.save(enc);
        Ok(())
    }

    /// Restores the v2 section (fail-closed): every count bounded
    /// before allocation trusts it, every discriminant typed, the
    /// configuration domain-validated, the identity table decoded
    /// through the existing [`NpcIdentity`] codec with its region,
    /// home-cell, ordering, and global-uniqueness gates, the
    /// grid-position bounds with the dormant-discard sentinel (active
    /// blocks), the structural dormant shape, and the ONE identity
    /// count governing every block — the block counts must sum to
    /// `config.npc_count` exactly (a different identity set is an
    /// incompatible workload, rejected at restore, never a resized
    /// restore). Malformed input is never repaired.
    ///
    /// This entry decodes under the WIRE-VALID maximum limits
    /// ([`SectionDecodeLimits::WIRE_MAXIMUM`]) — today's acceptance,
    /// byte for byte, and the bound every well-formed section
    /// satisfies. A composition boundary decoding less-trusted bytes
    /// (a session snapshot from storage it did not author) should
    /// call [`Self::restore_with_limits`] with the limits it can
    /// afford: the reservation-abort shape (a count within wire
    /// ranges whose column reservations exceed the platform's
    /// willingness) becomes a typed refusal there, and remains —
    /// under the wire maximum — bounded by the input's own size (the
    /// minimum-bytes gates below still apply, so no reservation ever
    /// exceeds what the wire could actually supply for that count).
    pub fn restore(dec: &mut Decoder) -> Result<Self, SaveError> {
        Self::restore_with_limits(dec, &SectionDecodeLimits::WIRE_MAXIMUM)
    }

    /// Restores the v2 section under CALLER-CONFIGURED admission
    /// limits (CA-02/F02 — see [`SectionDecodeLimits`]): the same
    /// fail-closed walk as [`Self::restore`], with the decoder's
    /// resource admission decided by the caller rather than the
    /// wire's validity domain alone. Three gates close the
    /// reservation-abort shape, each BEFORE the reservation it
    /// guards:
    ///
    /// * **The aggregate identity budget**: a declared population
    ///   above `limits.max_identities` is refused before any region
    ///   decodes (the over-budget workload refusal).
    /// * **The remaining identity budget**: each block's identity
    ///   count is bounded by the population REMAINING unclaimed by
    ///   earlier blocks (`npc_count − identities so far`), not the
    ///   full declared total — so no per-region reservation can
    ///   exceed the workload the section actually declares, and the
    ///   cumulative equality check at the end is an integrity gate,
    ///   never the first line of defense.
    /// * **The minimum-remaining-bytes gate**: before a column's
    ///   vector may reserve `count` elements, `count ×` the
    ///   element's wire stride must be payable from the bytes
    ///   actually remaining (the generic count bound checks one
    ///   byte per element; identity records occupy eleven wire
    ///   bytes, the u64 columns eight, positions two — a count
    ///   within the one-byte bound could reserve 8–11× what the
    ///   wire could supply for it). The gate never rejects a valid
    ///   encoding: a complete section's remaining bytes at each
    ///   column are exactly that column's bytes plus everything
    ///   after it.
    ///
    /// All count arithmetic is checked, every reservation is
    /// fallible (`try_reserve` — an allocation failure is a typed
    /// [`SaveError::Overflow`] refusal, never an abort), and the
    /// cumulative in-memory reservation is checked against
    /// `limits.max_reserve_bytes` (the `reserve-bytes` refusal). The
    /// per-region block vector itself stays a plain reservation:
    /// `region_count` is bounded by the constant
    /// [`MAX_REGION_COUNT`] (256 blocks × the block struct's size —
    /// affordable by definition, like the v1 codec's fixed 1,000
    /// identity set, which is why the v1 walk needs no limits type).
    pub fn restore_with_limits(
        dec: &mut Decoder,
        limits: &SectionDecodeLimits,
    ) -> Result<Self, SaveError> {
        const SECTION: &str = "npc-world-save-v2";
        let seed = dec.read_u64()?;
        let activity_period = dec.read_u64()?;
        let simplify_factor = dec.read_u64()?;
        let commitment_window = dec.read_u64()?;
        let region_lods_enabled = read_flag(dec, SECTION)?;
        // THE CONFIGURATION AS DATA: the partition bounded by the
        // identity codec's region field, the population by the NpcId
        // space and the bytes actually remaining — never a constant.
        let region_count = dec.read_count(u64::from(MAX_REGION_COUNT), SECTION, "region-count")?;
        if region_count == 0 {
            return Err(SaveError::Integrity("npc-world-save-v2/region-count-zero"));
        }
        let npc_count = dec.read_count(u64::from(MAX_NPC_COUNT), SECTION, "npc-count")?;
        if npc_count == 0 {
            return Err(SaveError::Integrity("npc-world-save-v2/npc-count-zero"));
        }
        let region_grid = dec.read_u8()?;
        if region_grid == 0 {
            return Err(SaveError::Integrity("npc-world-save-v2/region-grid-zero"));
        }
        let config = WorldConfig {
            region_count: u32::try_from(region_count)
                .map_err(|_| SaveError::Integrity("npc-world-save-v2/region-count"))?,
            npc_count: u32::try_from(npc_count)
                .map_err(|_| SaveError::Integrity("npc-world-save-v2/npc-count"))?,
            region_grid,
        }
        .validated()
        .map_err(|_| SaveError::Integrity("npc-world-save-v2/world-config"))?;
        // THE AGGREGATE IDENTITY BUDGET (CA-02): the declared
        // population is the workload the whole decode materializes —
        // the caller's record limit refuses it before any region
        // decodes, never after the columns are reserved.
        if u64::from(config.npc_count) > limits.max_identities {
            return Err(SaveError::Limit {
                section: SECTION,
                kind: "npc-count",
                value: u64::from(config.npc_count),
                max: limits.max_identities,
            });
        }
        // The cumulative in-memory reservation across every block's
        // columns (the `reserve-bytes` budget's ledger).
        let mut reserved: u64 = 0;
        // The regions vector: bounded by the constant region-field
        // width (256 blocks), affordable by definition.
        let mut regions = Vec::with_capacity(region_count as usize);
        let mut seen_ids: BTreeSet<u32> = BTreeSet::new();
        let mut total_identities = 0u64;
        for region_index in 0..region_count {
            let near = read_flag(dec, SECTION)?;
            let lod = read_lod(dec, SECTION)?;
            // A block's identity count is bounded by the world's own
            // population, the bytes remaining, AND the identities the
            // earlier blocks have not yet claimed (the remaining
            // identity budget — CA-02): the sum of every block's
            // reservation can never exceed the declared workload.
            let identity_budget = npc_count - total_identities;
            let identity_count = dec.read_count(identity_budget, SECTION, "identity-count")?;
            let count = usize::try_from(identity_count)
                .map_err(|_| SaveError::Overflow { section: SECTION })?;
            let mut identities = Vec::new();
            admit_column(
                identity_count,
                NpcIdentity::WIRE_STRIDE as u64,
                std::mem::size_of::<NpcIdentity>() as u64,
                dec.remaining() as u64,
                "identity-count",
                limits,
                &mut reserved,
            )?;
            identities
                .try_reserve(count)
                .map_err(|_| SaveError::Overflow { section: SECTION })?;
            let mut prev_id: Option<u32> = None;
            for _ in 0..identity_count {
                let mut record = [0u8; NpcIdentity::WIRE_STRIDE];
                for slot in record.iter_mut() {
                    *slot = dec.read_u8()?;
                }
                let identity = NpcIdentity::from_wire(&record)
                    .ok_or(SaveError::Integrity("npc-world-save-v2/identity-record"))?;
                // The identity's region membership must match the
                // block it rides in — the region-major layout is the
                // partition, not a hint.
                if identity.region != region_index as u8 {
                    return Err(SaveError::Integrity("npc-world-save-v2/identity-region"));
                }
                // The home cell is durable: strictly on-grid, never
                // the discard sentinel (dormancy discards the
                // routine representation, never the identity).
                if identity.home_cell.0 >= region_grid || identity.home_cell.1 >= region_grid {
                    return Err(SaveError::Integrity("npc-world-save-v2/identity-home-cell"));
                }
                // Canonical order: strictly ascending ids within the
                // block, unique across the whole section — the dense
                // positional discipline, made wire-checkable.
                if let Some(prior) = prev_id
                    && identity.id.0 <= prior
                {
                    return Err(SaveError::Order { section: SECTION });
                }
                prev_id = Some(identity.id.0);
                if !seen_ids.insert(identity.id.0) {
                    return Err(SaveError::Duplicate {
                        section: SECTION,
                        seq: u64::from(identity.id.0),
                    });
                }
                identities.push(identity);
            }
            total_identities = total_identities
                .checked_add(identity_count)
                .ok_or(SaveError::Overflow { section: SECTION })?;
            let mut steps = Vec::new();
            admit_column(
                identity_count,
                8,
                std::mem::size_of::<u64>() as u64,
                dec.remaining() as u64,
                "steps-count",
                limits,
                &mut reserved,
            )?;
            steps
                .try_reserve(count)
                .map_err(|_| SaveError::Overflow { section: SECTION })?;
            for _ in 0..identity_count {
                steps.push(dec.read_u64()?);
            }
            // THE STRUCTURAL DORMANT SHAPE: a Dormant block carries
            // only the record columns; the reconstructible columns are
            // absent on the wire and empty in memory (the occupied-
            // only rule — a dormant region costs its records, never
            // its representation).
            let (routine_cursor, positions, represented_through, pending_signals) =
                if lod == LodLevel::Dormant {
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new())
                } else {
                    let mut routine_cursor = Vec::new();
                    admit_column(
                        identity_count,
                        8,
                        std::mem::size_of::<u64>() as u64,
                        dec.remaining() as u64,
                        "routine-cursor-count",
                        limits,
                        &mut reserved,
                    )?;
                    routine_cursor
                        .try_reserve(count)
                        .map_err(|_| SaveError::Overflow { section: SECTION })?;
                    for _ in 0..identity_count {
                        routine_cursor.push(dec.read_u64()?);
                    }
                    // Grid positions carry the dormant-discard
                    // sentinel (the reconstructible representation is
                    // GONE, not merely flagged); every other position
                    // must lie on the region-local grid — through the
                    // ONE shared predicate (F06: this gate and
                    // `validate` answer identically by construction).
                    let mut positions = Vec::new();
                    admit_column(
                        identity_count,
                        2,
                        std::mem::size_of::<(u8, u8)>() as u64,
                        dec.remaining() as u64,
                        "positions-count",
                        limits,
                        &mut reserved,
                    )?;
                    positions
                        .try_reserve(count)
                        .map_err(|_| SaveError::Overflow { section: SECTION })?;
                    for _ in 0..identity_count {
                        let x = dec.read_u8()?;
                        let y = dec.read_u8()?;
                        if !position_admitted((x, y), region_grid) {
                            return Err(SaveError::Integrity("npc-world-save-v2/position-bounds"));
                        }
                        positions.push((x, y));
                    }
                    let mut represented_through = Vec::new();
                    admit_column(
                        identity_count,
                        8,
                        std::mem::size_of::<Tick>() as u64,
                        dec.remaining() as u64,
                        "represented-through-count",
                        limits,
                        &mut reserved,
                    )?;
                    represented_through
                        .try_reserve(count)
                        .map_err(|_| SaveError::Overflow { section: SECTION })?;
                    for _ in 0..identity_count {
                        represented_through.push(dec.read_tick()?);
                    }
                    let mut pending_signals = Vec::new();
                    admit_column(
                        identity_count,
                        8,
                        std::mem::size_of::<u64>() as u64,
                        dec.remaining() as u64,
                        "pending-signals-count",
                        limits,
                        &mut reserved,
                    )?;
                    pending_signals
                        .try_reserve(count)
                        .map_err(|_| SaveError::Overflow { section: SECTION })?;
                    for _ in 0..identity_count {
                        pending_signals.push(dec.read_u64()?);
                    }
                    (
                        routine_cursor,
                        positions,
                        represented_through,
                        pending_signals,
                    )
                };
            let mut milestones = Vec::new();
            admit_column(
                identity_count,
                8,
                std::mem::size_of::<u64>() as u64,
                dec.remaining() as u64,
                "milestones-count",
                limits,
                &mut reserved,
            )?;
            milestones
                .try_reserve(count)
                .map_err(|_| SaveError::Overflow { section: SECTION })?;
            for _ in 0..identity_count {
                milestones.push(dec.read_u64()?);
            }
            let mut credits = Vec::new();
            admit_column(
                identity_count,
                8,
                std::mem::size_of::<u64>() as u64,
                dec.remaining() as u64,
                "credits-count",
                limits,
                &mut reserved,
            )?;
            credits
                .try_reserve(count)
                .map_err(|_| SaveError::Overflow { section: SECTION })?;
            for _ in 0..identity_count {
                credits.push(dec.read_u64()?);
            }
            regions.push(NpcRegionBlock {
                near,
                lod,
                identities,
                steps,
                routine_cursor,
                positions,
                represented_through,
                milestones,
                pending_signals,
                credits,
            });
        }
        // The ONE identity count governs: the block counts must sum
        // to the configured population exactly — a different identity
        // set is an incompatible workload, never a resized restore.
        if total_identities != u64::from(config.npc_count) {
            return Err(SaveError::Integrity("npc-world-save-v2/npc-count"));
        }
        Ok(NpcWorldSectionV2 {
            seed,
            activity_period,
            simplify_factor,
            commitment_window,
            region_lods_enabled,
            config,
            regions,
        })
    }

    /// Validates an in-memory section before it is encoded (the
    /// constructor-side discipline; the wire input path enforces the
    /// same gates inline as it reads): the configuration's domain,
    /// the block count agreement, the per-block column agreement with
    /// the structural dormant shape, the identity table's region,
    /// home-cell, ordering, and global-uniqueness gates, the ACTIVE
    /// blocks' grid positions through the ONE shared position
    /// predicate (`position_admitted` — exactly restore's rule, so a
    /// section that validates is exactly a section whose encoding
    /// restores), and the population sum.
    pub fn validate(&self) -> Result<(), SaveError> {
        const SECTION: &str = "npc-world-save-v2";
        self.config
            .validated()
            .map_err(|_| SaveError::Integrity("npc-world-save-v2/world-config"))?;
        if self.regions.len() != self.config.region_count as usize {
            return Err(SaveError::Integrity("npc-world-save-v2/region-count"));
        }
        let mut seen_ids: BTreeSet<u32> = BTreeSet::new();
        let mut total_identities = 0u64;
        for (region_index, block) in self.regions.iter().enumerate() {
            let count = block.identities.len();
            if block.steps.len() != count
                || block.milestones.len() != count
                || block.credits.len() != count
            {
                return Err(SaveError::Integrity("npc-world-save-v2/column-counts"));
            }
            let dormant = block.lod == LodLevel::Dormant;
            for column_len in [
                block.routine_cursor.len(),
                block.positions.len(),
                block.represented_through.len(),
                block.pending_signals.len(),
            ] {
                if dormant && column_len != 0 {
                    return Err(SaveError::Integrity("npc-world-save-v2/dormant-shape"));
                }
                if !dormant && column_len != count {
                    return Err(SaveError::Integrity("npc-world-save-v2/column-counts"));
                }
            }
            // THE SHARED POSITION GATE (F06): every active-block
            // position faces exactly restore's predicate — the
            // on-grid-or-sentinel rule through the ONE shared
            // function — under the same production error name, so
            // the in-memory gate and the wire gate refuse
            // identically (the (4,0) witness case: previously
            // admitted here, refused at restore).
            if !dormant {
                for position in &block.positions {
                    if !position_admitted(*position, self.config.region_grid) {
                        return Err(SaveError::Integrity("npc-world-save-v2/position-bounds"));
                    }
                }
            }
            let mut prev_id: Option<u32> = None;
            for identity in &block.identities {
                if identity.region != region_index as u8 {
                    return Err(SaveError::Integrity("npc-world-save-v2/identity-region"));
                }
                if identity.home_cell.0 >= self.config.region_grid
                    || identity.home_cell.1 >= self.config.region_grid
                {
                    return Err(SaveError::Integrity("npc-world-save-v2/identity-home-cell"));
                }
                if let Some(prior) = prev_id
                    && identity.id.0 <= prior
                {
                    return Err(SaveError::Order { section: SECTION });
                }
                prev_id = Some(identity.id.0);
                if !seen_ids.insert(identity.id.0) {
                    return Err(SaveError::Duplicate {
                        section: SECTION,
                        seq: u64::from(identity.id.0),
                    });
                }
            }
            total_identities = total_identities
                .checked_add(u64::try_from(count).expect("counts are bounded by the identity set"))
                .ok_or(SaveError::Overflow { section: SECTION })?;
        }
        if total_identities != u64::from(self.config.npc_count) {
            return Err(SaveError::Integrity("npc-world-save-v2/npc-count"));
        }
        Ok(())
    }
}

/// The fixed world a v2-declaring consumer restores through: the v2
/// sibling of [`NpcSaveWorld`] — a [`World`]/[`SaveWorld`]
/// implementation whose section acceptance is EXACTLY
/// [`NpcWorldSectionV2::restore`]'s. Like `NpcSaveWorld`, it exists
/// for VALIDATION only (a no-op handler, never fingerprinting, never
/// a live game world); a future authenticated anchor that must
/// validate v2 session snapshots restores through it (P3
/// coordination — the design's open question 4). Its domain is the
/// v2 tag, so the spine's world-section framing dispatches v1 and v2
/// saves to their own codecs and the forms reject one another.
///
/// **The admission boundary (CA-02):** the [`SaveWorld::restore`]
/// trait signature carries no limits, so this path decodes under the
/// WIRE-VALID maximum ([`SectionDecodeLimits::WIRE_MAXIMUM`]) — the
/// acceptance every well-formed section satisfies. When the v2
/// anchor lands, its brief owns the affordability decision: a
/// composition boundary that restores a section from bytes it did
/// not author calls [`NpcWorldSectionV2::restore_with_limits`]
/// directly with the limits it can afford, and this validation-only
/// world stays on the wire maximum exactly as the v1 `NpcSaveWorld`
/// does (the v1 walk needs no limits — its identity set is the
/// fixed 1,000-record v1 interpretation, affordable by definition).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpcSaveWorldV2(pub NpcWorldSectionV2);

impl World for NpcSaveWorldV2 {
    fn execute(&mut self, _event: &Event, _ctx: &mut ExecCtx) {}
    fn fingerprint(&self, _fp: &mut Fingerprinter) {}
}

impl SaveWorld for NpcSaveWorldV2 {
    const SAVE_DOMAIN: &'static str = NPC_WORLD_SAVE_DOMAIN_V2;

    fn save(&self, enc: &mut Encoder) {
        self.0.save(enc);
    }

    fn restore(dec: &mut Decoder) -> Result<Self, SaveError> {
        Ok(NpcSaveWorldV2(NpcWorldSectionV2::restore(dec)?))
    }
}

/// The wire discriminant for a LOD level (the v1 section's encoding,
/// shared by the v2 blocks).
fn lod_discriminant(lod: LodLevel) -> u8 {
    match lod {
        LodLevel::Full => 0,
        LodLevel::Simplified => 1,
        LodLevel::EventDriven => 2,
        LodLevel::Dormant => 3,
    }
}

/// The ONE grid-position predicate for the v2 section's ACTIVE
/// blocks, shared by the in-memory validator and the decoder (F06:
/// the two gates disagreed — `validate` checked column lengths and
/// durable home cells but never inspected the active positions, so a
/// section could pass validation and fail its own restore with
/// `npc-world-save-v2/position-bounds`). A position is admitted
/// exactly when it is the dormant-discard sentinel
/// ([`POSITION_DISCARDED`]) or lies strictly on the region-local
/// grid — the exact-tuple rule, so a PARTIAL sentinel (one sentinel
/// coordinate, one off-grid) is an ordinary off-grid refusal, never
/// an exemption. Both [`NpcWorldSectionV2::validate`] (before
/// encoding) and [`NpcWorldSectionV2::restore`] (after) consult THIS
/// function: a section that validates is exactly a section whose
/// encoding restores.
fn position_admitted(position: (u8, u8), region_grid: u8) -> bool {
    position == POSITION_DISCARDED || (position.0 < region_grid && position.1 < region_grid)
}

/// The per-column admission gate (CA-02/F02): before a column's
/// vector may reserve `count` elements, the count must be payable in
/// WIRE bytes actually remaining (each element's minimum wire
/// stride — the generic `Decoder::read_count` bound checks one byte
/// per element, an 8–11× looseness per column this gate closes) and
/// in the caller's declared in-memory reserve budget (accumulated in
/// `reserved` across the whole section decode). Checked arithmetic
/// throughout; the refusal is a typed [`SaveError::Limit`] naming the
/// column, BEFORE any reservation is made — a truncated or crafted
/// input can no longer make the decoder reserve what the wire cannot
/// supply for the count it declared. Never rejects a valid encoding:
/// a complete section's remaining bytes at each column are exactly
/// that column's bytes plus everything after it.
fn admit_column(
    count: u64,
    wire_stride: u64,
    mem_stride: u64,
    remaining: u64,
    kind: &'static str,
    limits: &SectionDecodeLimits,
    reserved: &mut u64,
) -> Result<(), SaveError> {
    const SECTION: &str = "npc-world-save-v2";
    let wire_needed = count
        .checked_mul(wire_stride)
        .ok_or(SaveError::Overflow { section: SECTION })?;
    if wire_needed > remaining {
        return Err(SaveError::Limit {
            section: SECTION,
            kind,
            value: count,
            max: remaining / wire_stride.max(1),
        });
    }
    let mem_needed = count
        .checked_mul(mem_stride)
        .ok_or(SaveError::Overflow { section: SECTION })?;
    *reserved = reserved
        .checked_add(mem_needed)
        .ok_or(SaveError::Overflow { section: SECTION })?;
    if *reserved > limits.max_reserve_bytes {
        return Err(SaveError::Limit {
            section: SECTION,
            kind: "reserve-bytes",
            value: *reserved,
            max: limits.max_reserve_bytes,
        });
    }
    Ok(())
}

/// Reads a boolean flag (fail-closed on non-0/1 discriminants).
fn read_flag(dec: &mut Decoder, section: &'static str) -> Result<bool, SaveError> {
    match dec.read_u8()? {
        0 => Ok(false),
        1 => Ok(true),
        discriminant => Err(SaveError::Enum {
            section,
            discriminant: u64::from(discriminant),
        }),
    }
}

/// Reads a LOD level (fail-closed on unknown discriminants).
fn read_lod(dec: &mut Decoder, section: &'static str) -> Result<LodLevel, SaveError> {
    match dec.read_u8()? {
        0 => Ok(LodLevel::Full),
        1 => Ok(LodLevel::Simplified),
        2 => Ok(LodLevel::EventDriven),
        3 => Ok(LodLevel::Dormant),
        discriminant => Err(SaveError::Enum {
            section,
            discriminant: u64::from(discriminant),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_section() -> NpcWorldSection {
        let positions = (0..NPC_COUNT)
            .map(|npc| ((npc % 16) as u8, ((npc / 16) % 16) as u8))
            .collect();
        NpcWorldSection {
            seed: 1337,
            near_mask: 0b0000_0011,
            activity_period: 6,
            simplify_factor: 4,
            commitment_window: 12,
            region_lods_enabled: true,
            steps: (0..NPC_COUNT).map(|npc| npc as u64).collect(),
            routine_cursor: (0..NPC_COUNT).map(|npc| (npc / 2) as u64).collect(),
            positions,
            represented_through: vec![Tick(7); NPC_COUNT],
            milestones: (0..NPC_COUNT).map(|npc| (npc / 16) as u64).collect(),
            pending_signals: vec![0; NPC_COUNT],
            credits: (0..NPC_COUNT).map(|npc| (npc / 32) as u64).collect(),
            region_lods: [
                LodLevel::Full,
                LodLevel::Simplified,
                LodLevel::EventDriven,
                LodLevel::Dormant,
                LodLevel::Full,
                LodLevel::Simplified,
                LodLevel::EventDriven,
                LodLevel::Dormant,
            ],
        }
    }

    fn encode(section: &NpcWorldSection) -> Vec<u8> {
        let mut enc = Encoder::new();
        section.save(&mut enc);
        enc.into_bytes()
    }

    #[test]
    fn the_section_round_trips_under_its_own_codec() {
        let section = sample_section();
        let bytes = encode(&section);
        let mut dec = Decoder::new(&bytes);
        let restored = NpcWorldSection::restore(&mut dec).unwrap();
        dec.finish().unwrap();
        assert_eq!(restored, section);
    }

    #[test]
    fn the_fixed_world_declares_the_production_domain_and_delegates() {
        // The acceptance identity: NpcSaveWorld::restore is the
        // section codec, and the domain is the ONE string — the
        // spine's world-section framing accepts it exactly where the
        // production world's does.
        assert_eq!(NpcSaveWorld::SAVE_DOMAIN, NPC_WORLD_SAVE_DOMAIN);
        assert_eq!(NPC_WORLD_SAVE_DOMAIN, "engine-slice/npc-world-save/1");
        let bytes = encode(&sample_section());
        let mut dec = Decoder::new(&bytes);
        let world = NpcSaveWorld::restore(&mut dec).unwrap();
        dec.finish().unwrap();
        assert_eq!(world.0, sample_section());
    }

    #[test]
    fn a_truncated_section_fails_closed() {
        // The R15 adversarial shape: an 8-byte payload (well-formed
        // as a weak blob, structurally short for the production
        // walk) fails the section codec — the production error the
        // bypass exploited.
        let mut enc = Encoder::new();
        enc.write_u64(0);
        let bytes = enc.into_bytes();
        let mut dec = Decoder::new(&bytes);
        assert!(NpcWorldSection::restore(&mut dec).is_err());
    }

    #[test]
    fn an_incompatible_identity_count_is_refused_never_resized() {
        // The count rewrite: the production prefix with the count
        // reading NPC_COUNT + 1 — an incompatible workload, refused.
        let section = sample_section();
        let mut enc = Encoder::new();
        enc.write_u64(section.seed);
        enc.write_u64(section.near_mask);
        enc.write_u64(section.activity_period);
        enc.write_u64(section.simplify_factor);
        enc.write_u64(section.commitment_window);
        enc.write_u8(1);
        enc.write_count(NPC_COUNT + 1);
        let bytes = enc.into_bytes();
        let mut dec = Decoder::new(&bytes);
        assert!(NpcWorldSection::restore(&mut dec).is_err());
    }

    #[test]
    fn an_off_grid_position_is_refused() {
        let mut section = sample_section();
        section.positions[5] = (REGION_GRID, 0);
        let bytes = encode(&section);
        let mut dec = Decoder::new(&bytes);
        assert!(NpcWorldSection::restore(&mut dec).is_err());
        // The dormant-discard sentinel is the one off-grid exemption.
        section.positions[5] = POSITION_DISCARDED;
        let bytes = encode(&section);
        let mut dec = Decoder::new(&bytes);
        assert!(NpcWorldSection::restore(&mut dec).is_ok());
    }

    #[test]
    fn unknown_discriminants_and_mask_overflow_fail_closed() {
        let mut section = sample_section();
        section.region_lods[2] = LodLevel::Full;
        let mut bytes = encode(&section);
        // The last byte is the final region's LOD discriminant.
        *bytes.last_mut().unwrap() = 9;
        let mut dec = Decoder::new(&bytes);
        assert!(NpcWorldSection::restore(&mut dec).is_err());

        let mut section = sample_section();
        section.near_mask = 1 << REGION_COUNT;
        let bytes = encode(&section);
        let mut dec = Decoder::new(&bytes);
        assert!(NpcWorldSection::restore(&mut dec).is_err());

        // The enabled-flag discriminant.
        let section = sample_section();
        let mut bytes = encode(&section);
        // The flag is the 6th field: 5 u64s (40 bytes) then the u8.
        bytes[40] = 7;
        let mut dec = Decoder::new(&bytes);
        assert!(NpcWorldSection::restore(&mut dec).is_err());
    }

    // ---- npc-world-save/2 (P2 L8, A05/B12) -------------------------
    //
    // The mandated battery: (1) the v1-load compatibility (a golden
    // v1 save loads, semantics unchanged), (2) the v2 round-trip at
    // multiple scales, (3) the identity round-trip through real
    // storage, (4) the corner's configured values through the
    // configured format — plus the dispatch, the dormant-block
    // shapes, the count gates, the corruption corpus, and the
    // multi-scale run through the configured engine.

    use crate::diag::{Diagnostics, Level};
    use crate::event::{EventClass, EventSpec, Schedule};
    use crate::npc_behavior::NpcId;
    use crate::spine::Spine;

    /// The deterministic identity for a fixture id: home cell strictly
    /// on the given grid, region membership, a derived appearance
    /// seed — the durable-lane facets the table round-trips.
    fn fixture_identity(id: u32, region: u8, grid: u8) -> NpcIdentity {
        let side = u32::from(grid);
        NpcIdentity {
            id: NpcId(id),
            home_cell: ((id % side) as u8, ((id / side) % side) as u8),
            region,
            appearance_seed: id.wrapping_mul(2_654_435_761),
        }
    }

    /// One fixture region block: `ids` ascending, the state columns
    /// derived deterministically, and the STRUCTURAL dormant shape —
    /// a Dormant block carries only the record columns.
    fn region_block(
        region: u8,
        grid: u8,
        ids: &[u32],
        lod: LodLevel,
        near: bool,
    ) -> NpcRegionBlock {
        let identities = ids
            .iter()
            .map(|&id| fixture_identity(id, region, grid))
            .collect::<Vec<_>>();
        let steps = ids.iter().map(|&id| u64::from(id) % 97).collect();
        let milestones = ids.iter().map(|&id| u64::from(id) % 16).collect();
        let credits = ids.iter().map(|&id| u64::from(id) % 5).collect();
        if lod == LodLevel::Dormant {
            NpcRegionBlock {
                near,
                lod,
                identities,
                steps,
                milestones,
                credits,
                routine_cursor: Vec::new(),
                positions: Vec::new(),
                represented_through: Vec::new(),
                pending_signals: Vec::new(),
            }
        } else {
            NpcRegionBlock {
                near,
                lod,
                identities,
                steps,
                milestones,
                credits,
                routine_cursor: ids.iter().map(|&id| u64::from(id) / 2).collect(),
                positions: ids
                    .iter()
                    .map(|&id| {
                        (
                            (id % u32::from(grid)) as u8,
                            ((id / 7) % u32::from(grid)) as u8,
                        )
                    })
                    .collect(),
                represented_through: ids.iter().map(|&id| Tick(u64::from(id) % 31)).collect(),
                pending_signals: ids.iter().map(|&id| u64::from(id) % 3).collect(),
            }
        }
    }

    /// A configured fixture section: the region-major partition
    /// `npc % region_count` (the demo's own rule), the first
    /// `active_regions` blocks Full, the rest Dormant.
    fn scaled_section(regions: u32, npcs: u32, grid: u8, active_regions: u32) -> NpcWorldSectionV2 {
        let mut blocks = Vec::new();
        for region in 0..regions {
            let ids = (region..npcs).step_by(regions as usize).collect::<Vec<_>>();
            let lod = if region < active_regions {
                LodLevel::Full
            } else {
                LodLevel::Dormant
            };
            blocks.push(region_block(region as u8, grid, &ids, lod, region < 2));
        }
        NpcWorldSectionV2 {
            seed: 1337,
            activity_period: 6,
            simplify_factor: 4,
            commitment_window: 12,
            region_lods_enabled: true,
            config: WorldConfig {
                region_count: regions,
                npc_count: npcs,
                region_grid: grid,
            },
            regions: blocks,
        }
    }

    fn encode_v2(section: &NpcWorldSectionV2) -> Vec<u8> {
        let mut enc = Encoder::new();
        section.save(&mut enc);
        enc.into_bytes()
    }

    /// The exact v2 size formula (the occupied-only rule made
    /// checkable): a fixed 50-byte header, then per block 10 bytes of
    /// framing plus 61 bytes per ACTIVE identity (identity 11, seven
    /// state columns 50) or 35 bytes per DORMANT identity (identity
    /// 11, three record columns 24). No term is proportional to a
    /// world-space dense grid — the region-local grid never enters
    /// the size, and a dormant region pays only its records.
    fn expected_v2_len(section: &NpcWorldSectionV2) -> usize {
        let mut len = 8 * 4 + 1 + 8 + 8 + 1;
        for block in &section.regions {
            let count = block.identities.len();
            len += 10 + count * NpcIdentity::WIRE_STRIDE;
            len += count * 8; // steps
            if block.lod != LodLevel::Dormant {
                len += count * (8 + 2 + 8 + 8); // cursor, positions, boundary, pending
            }
            len += count * 8; // milestones
            len += count * 8; // credits
        }
        len
    }

    #[test]
    fn the_v1_walk_loads_an_independently_authored_golden_byte_stream() {
        // THE V1-COMPATIBILITY PIN (the lane's mandatory test 1): a
        // golden v1 save — authored here by an INDEPENDENT byte walk,
        // never through `NpcWorldSection::save` — loads through the
        // UNCHANGED v1 decoder with the semantics unchanged, at the
        // frozen v1 length. The v2 boundary does not move one v1 byte.
        let mut enc = Encoder::new();
        enc.write_u64(1337); // seed
        enc.write_u64(0b0000_0011); // near mask
        enc.write_u64(6); // activity period
        enc.write_u64(4); // simplify factor
        enc.write_u64(12); // commitment window
        enc.write_u8(1); // region-LOD path enabled
        enc.write_count(NPC_COUNT);
        for npc in 0..NPC_COUNT as u64 {
            enc.write_u64(npc); // steps
        }
        for npc in 0..NPC_COUNT as u64 {
            enc.write_u64(npc / 2); // routine cursors
        }
        for npc in 0..NPC_COUNT as u32 {
            enc.write_u8((npc % 16) as u8); // position x
            enc.write_u8(((npc / 16) % 16) as u8); // position y
        }
        for _ in 0..NPC_COUNT {
            enc.write_tick(Tick(7)); // represented-through boundaries
        }
        for npc in 0..NPC_COUNT as u64 {
            enc.write_u64(npc / 16); // milestones
        }
        for _ in 0..NPC_COUNT {
            enc.write_u64(0); // pending signals
        }
        for npc in 0..NPC_COUNT as u64 {
            enc.write_u64(npc / 32); // credits
        }
        for lod in 0..REGION_COUNT as u8 {
            enc.write_u8(lod % 4); // region LOD view
        }
        let bytes = enc.into_bytes();
        // The frozen v1 formula: 5 u64s + flag + count + the dense
        // columns (50 bytes per NPC) + the region LOD view.
        assert_eq!(bytes.len(), 8 * 5 + 1 + 8 + NPC_COUNT * 50 + REGION_COUNT);
        let mut dec = Decoder::new(&bytes);
        let restored = NpcWorldSection::restore(&mut dec).unwrap();
        dec.finish().expect("the golden stream consumed exactly");
        assert_eq!(restored, sample_section());
    }

    #[test]
    fn v2_round_trips_at_multiple_scales() {
        // The mandated multi-scale round-trip: the memory-lane scale,
        // the CORNER'S CONFIGURED VALUES (8 regions / 1,000
        // identities / 16-grid — today's workload through the
        // configured format, all-active and with dormancy), the
        // 10k-population fixture, the minimal world, and the widest
        // admitted partition with the widest grid — every scale
        // round-trips under its own carried configuration, and the
        // encoded length is exactly the occupied-only formula.
        for (regions, npcs, grid, active) in [
            (8, 24, 16, 8),
            (8, 1000, 16, 8),
            (8, 1000, 16, 3),
            (64, 10_000, 16, 4),
            (1, 1, 1, 1),
            (256, 300, 255, 0),
        ] {
            let section = scaled_section(regions, npcs, grid, active);
            section
                .validate()
                .expect("the fixture section is consistent");
            let bytes = encode_v2(&section);
            assert_eq!(bytes.len(), expected_v2_len(&section));
            let mut dec = Decoder::new(&bytes);
            let restored = NpcWorldSectionV2::restore(&mut dec).unwrap();
            dec.finish().expect("the section consumed exactly");
            assert_eq!(restored, section, "the {npcs}-identity round trip");
            assert_eq!(
                encode_v2(&restored),
                bytes,
                "the re-encode is byte-identical"
            );
        }
    }

    #[test]
    fn identity_tables_round_trip_through_real_section_storage() {
        // B12 (the lane's mandatory test 3): `NpcIdentity` round-tripped
        // through the ACTUAL authoritative storage — the v2 section's
        // identity table (the bytes the world representation persists)
        // and the full spine save/restore around it — not a codec
        // self-test. Handcrafted durable facets (ids, home cells,
        // region memberships, appearance seeds) survive exactly.
        let handcrafted: [&[u32]; 3] = [&[0, 7, 19, 44, 45], &[1, 2, 3], &[5, 700, 90_000]];
        let mut blocks = Vec::new();
        for (region, ids) in handcrafted.iter().enumerate() {
            blocks.push(region_block(
                region as u8,
                16,
                ids,
                LodLevel::Full,
                region < 2,
            ));
        }
        let section = NpcWorldSectionV2 {
            seed: 42,
            activity_period: 5,
            simplify_factor: 3,
            commitment_window: 9,
            region_lods_enabled: true,
            config: WorldConfig {
                region_count: 3,
                npc_count: 11,
                region_grid: 16,
            },
            regions: blocks,
        };
        section.validate().unwrap();
        let bytes = encode_v2(&section);
        let mut dec = Decoder::new(&bytes);
        let restored = NpcWorldSectionV2::restore(&mut dec).unwrap();
        dec.finish().unwrap();
        assert_eq!(restored, section);
        for (region, ids) in handcrafted.iter().enumerate() {
            let expected = ids
                .iter()
                .map(|&id| fixture_identity(id, region as u8, 16))
                .collect::<Vec<_>>();
            assert_eq!(restored.regions[region].identities, expected);
        }
        // Through the spine's save/restore (the storage boundary the
        // anchor and any v2 consumer restore through): the section —
        // identity tables included — survives the full framing.
        let spine = Spine::with_reservation(NpcSaveWorldV2(section.clone()), 200, 60);
        let spine_bytes = spine.save_bytes();
        let restored_spine =
            Spine::<NpcSaveWorldV2>::restore(&spine_bytes, Diagnostics::new(Level::Off)).unwrap();
        assert_eq!(restored_spine.world().0, section);

        // The identity gates, on really encoded bytes: a duplicate id
        // across blocks, descending ids within a block, a region
        // mismatch, and an off-grid home cell are all refused.
        let duplicate = NpcWorldSectionV2 {
            regions: vec![
                region_block(0, 16, &[3, 9], LodLevel::Full, true),
                region_block(1, 16, &[9, 10], LodLevel::Full, false),
            ],
            config: WorldConfig {
                region_count: 2,
                npc_count: 4,
                region_grid: 16,
            },
            ..section.clone()
        };
        assert!(matches!(
            NpcWorldSectionV2::restore(&mut Decoder::new(&encode_v2(&duplicate))),
            Err(SaveError::Duplicate { seq: 9, .. })
        ));
        let descending = NpcWorldSectionV2 {
            regions: vec![region_block(0, 16, &[10, 4], LodLevel::Full, true)],
            config: WorldConfig {
                region_count: 1,
                npc_count: 2,
                region_grid: 16,
            },
            ..section.clone()
        };
        assert!(matches!(
            NpcWorldSectionV2::restore(&mut Decoder::new(&encode_v2(&descending))),
            Err(SaveError::Order { .. })
        ));
        let mut wrong_region = scaled_section(2, 6, 16, 2);
        wrong_region.regions[0].identities[1].region = 1;
        assert!(NpcWorldSectionV2::restore(&mut Decoder::new(&encode_v2(&wrong_region))).is_err());
        let mut off_grid = scaled_section(2, 6, 16, 2);
        off_grid.regions[0].identities[1].home_cell = (16, 0);
        assert!(NpcWorldSectionV2::restore(&mut Decoder::new(&encode_v2(&off_grid))).is_err());
        // And validate() refuses the same inconsistencies in memory.
        assert!(wrong_region.validate().is_err());
        assert!(off_grid.validate().is_err());
        assert!(duplicate.validate().is_err());
        assert!(descending.validate().is_err());
    }

    #[test]
    fn dormant_blocks_are_record_only_and_occupied_sized() {
        // The occupied-only rule, on exact bytes: five regions of 50
        // identities, two active and three dormant. The dormant
        // blocks pay 35 bytes per identity (identity + the three
        // record columns) against the active 61 — the 26-byte
        // difference is precisely the four omitted reconstructible
        // columns — and NOTHING in the size grows with a world-space
        // dense grid (the region-local grid never enters the formula;
        // a dormant region costs its records, never its
        // representation).
        let section = scaled_section(5, 250, 16, 2);
        let counts = section
            .regions
            .iter()
            .map(|block| (block.lod == LodLevel::Dormant, block.identities.len()))
            .collect::<Vec<_>>();
        assert_eq!(
            counts,
            vec![(false, 50), (false, 50), (true, 50), (true, 50), (true, 50)]
        );
        let bytes = encode_v2(&section);
        assert_eq!(bytes.len(), expected_v2_len(&section));
        // The dense alternative — the v1 shape's full columns for
        // every region — and the exact savings: 26 bytes per dormant
        // identity (cursor 8 + position 2 + boundary 8 + pending 8).
        let dense_len = 50
            + counts
                .iter()
                .fold(0, |sum, &(_, count)| sum + 10 + 61 * count);
        let dormant_identities: usize = counts
            .iter()
            .filter(|&&(dormant, _)| dormant)
            .map(|&(_, count)| count)
            .sum();
        assert_eq!(dense_len - bytes.len(), 26 * dormant_identities);
        // Restore synthesizes the structural dormant shape: the
        // record columns survive exactly, the reconstructible columns
        // are gone (empty — the representation is GONE, not flagged).
        let mut dec = Decoder::new(&bytes);
        let restored = NpcWorldSectionV2::restore(&mut dec).unwrap();
        dec.finish().unwrap();
        assert_eq!(restored, section);
        for block in &restored.regions {
            if block.lod == LodLevel::Dormant {
                assert!(block.routine_cursor.is_empty());
                assert!(block.positions.is_empty());
                assert!(block.represented_through.is_empty());
                assert!(block.pending_signals.is_empty());
                assert_eq!(block.steps.len(), 50);
                assert_eq!(block.milestones.len(), 50);
                assert_eq!(block.credits.len(), 50);
            }
        }
    }

    #[test]
    fn mismatched_counts_and_configurations_are_refused_never_resized() {
        // The one-count-governs rule at the configuration boundary:
        // the block identity counts must sum to the configured
        // population EXACTLY — a doctored npc-count (either direction)
        // is an incompatible workload, refused, never a resized
        // restore. The configuration's own domain gates (zero
        // partition, zero grid) refuse too.
        let section = scaled_section(3, 12, 4, 1);
        let bytes = encode_v2(&section);
        // The v2 header: 4 u64s (32 bytes), the flag, region-count
        // (33..41), npc-count (41..49), the grid byte (49).
        const REGION_COUNT_AT: usize = 33;
        const NPC_COUNT_AT: usize = 41;
        const GRID_AT: usize = 49;
        for value in [13u64, 11] {
            let mut doctored = bytes.clone();
            doctored[NPC_COUNT_AT..NPC_COUNT_AT + 8].copy_from_slice(&value.to_le_bytes());
            assert!(
                NpcWorldSectionV2::restore(&mut Decoder::new(&doctored)).is_err(),
                "npc count {value} rejected"
            );
        }
        let mut doctored = bytes.clone();
        doctored[REGION_COUNT_AT..REGION_COUNT_AT + 8].copy_from_slice(&0u64.to_le_bytes());
        assert!(NpcWorldSectionV2::restore(&mut Decoder::new(&doctored)).is_err());
        let mut doctored = bytes.clone();
        doctored[GRID_AT] = 0;
        assert!(NpcWorldSectionV2::restore(&mut Decoder::new(&doctored)).is_err());
        // A region count beyond the identity codec's region field is
        // refused by the wire bound (the read_count maximum), and an
        // in-memory configuration outside the domain by validate().
        let mut doctored = bytes.clone();
        doctored[REGION_COUNT_AT..REGION_COUNT_AT + 8]
            .copy_from_slice(&u64::from(MAX_REGION_COUNT + 1).to_le_bytes());
        assert!(matches!(
            NpcWorldSectionV2::restore(&mut Decoder::new(&doctored)),
            Err(SaveError::Limit { .. })
        ));
        let mut too_many_regions = section.clone();
        too_many_regions.config.region_count = MAX_REGION_COUNT + 1;
        assert!(too_many_regions.validate().is_err());
    }

    #[test]
    fn the_domains_dispatch_and_reject_one_another() {
        // THE EXPLICIT DOMAIN DISPATCH (never a silent format change):
        // a v1 save loads through the v1-declaring world and is
        // REFUSED by the v2 world by domain — both tags named — and
        // vice versa; the section-level walks refuse each other's
        // bytes as well (the shapes are disjoint, never silently
        // reinterpreted).
        let mut v1_spine = Spine::with_reservation(NpcSaveWorld(sample_section()), 200, 60);
        v1_spine
            .schedule(EventSpec::new(
                EventClass::new(0),
                Schedule::AtLogicalTime { due: Tick(1) },
                42,
            ))
            .unwrap();
        v1_spine.run_ticks(3);
        let v1_bytes = v1_spine.save_bytes();
        let restored_v1 =
            Spine::<NpcSaveWorld>::restore(&v1_bytes, Diagnostics::new(Level::Off)).unwrap();
        assert_eq!(restored_v1.world().0, sample_section());
        let error = Spine::<NpcSaveWorldV2>::restore(&v1_bytes, Diagnostics::new(Level::Off))
            .err()
            .expect("the v2 world refuses a v1 save");
        assert!(matches!(
            error,
            SaveError::Domain {
                expected: NPC_WORLD_SAVE_DOMAIN_V2,
                ..
            }
        ));

        let mut v2_spine =
            Spine::with_reservation(NpcSaveWorldV2(scaled_section(3, 12, 4, 2)), 200, 60);
        v2_spine
            .schedule(EventSpec::new(
                EventClass::new(0),
                Schedule::AtLogicalTime { due: Tick(1) },
                42,
            ))
            .unwrap();
        v2_spine.run_ticks(3);
        let v2_bytes = v2_spine.save_bytes();
        let restored_v2 =
            Spine::<NpcSaveWorldV2>::restore(&v2_bytes, Diagnostics::new(Level::Off)).unwrap();
        assert_eq!(restored_v2.world().0, scaled_section(3, 12, 4, 2));
        let error = Spine::<NpcSaveWorld>::restore(&v2_bytes, Diagnostics::new(Level::Off))
            .err()
            .expect("the v1 world refuses a v2 save");
        assert!(matches!(
            error,
            SaveError::Domain {
                expected: NPC_WORLD_SAVE_DOMAIN,
                ..
            }
        ));

        // Section-level: neither walk decodes the other's payload.
        let v1_section_bytes = encode(&sample_section());
        assert!(NpcWorldSectionV2::restore(&mut Decoder::new(&v1_section_bytes)).is_err());
        let v2_section_bytes = encode_v2(&scaled_section(3, 12, 4, 2));
        assert!(NpcWorldSection::restore(&mut Decoder::new(&v2_section_bytes)).is_err());
    }

    #[test]
    fn truncated_v2_sections_fail_closed() {
        // Every prefix of a valid v2 encoding fails: the walk reads
        // exact counts and typed discriminants, so a short input
        // never yields a section.
        let bytes = encode_v2(&scaled_section(3, 12, 4, 1));
        for cut in 0..bytes.len() {
            assert!(
                NpcWorldSectionV2::restore(&mut Decoder::new(&bytes[..cut])).is_err(),
                "cut at {cut} rejected"
            );
        }
        // Trailing bytes are refused at the consumption boundary: the
        // walk itself succeeds on its own bytes, and the framing's
        // exact-consumption finish refuses the leftover (the spine's
        // world-section reader applies the same finish after every
        // section restore).
        let mut trailing = bytes.clone();
        trailing.push(0);
        let mut dec = Decoder::new(&trailing);
        assert!(NpcWorldSectionV2::restore(&mut dec).is_ok());
        assert!(dec.finish().is_err());
    }

    // ------------------ CA-02 decode-admission tests (F02, Q07's core half)

    /// THE AGGREGATE IDENTITY BUDGET (CA-02): a declared population
    /// above the caller's configured record limit is refused BEFORE
    /// any region decodes — the over-budget workload refusal, a
    /// typed `Limit` naming the caller's affordability, never a
    /// decode-then-fail or an abort. The wire-maximum control
    /// restores the same bytes: the limits change admission, never
    /// the walk.
    #[test]
    fn small_configured_limits_refuse_the_over_budget_aggregate() {
        let section = scaled_section(3, 12, 4, 1);
        let bytes = encode_v2(&section);
        let limits = SectionDecodeLimits::new(6, u64::MAX);
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(&mut Decoder::new(&bytes), &limits),
            Err(SaveError::Limit {
                section: _,
                kind: "npc-count",
                value: 12,
                max: 6
            })
        ));
        // The wire maximum keeps today's acceptance exactly.
        let mut dec = Decoder::new(&bytes);
        let restored = NpcWorldSectionV2::restore(&mut dec).unwrap();
        dec.finish().unwrap();
        assert_eq!(restored, section);
        // A limit at the declared population admits it.
        let admitting = SectionDecodeLimits::new(12, u64::MAX);
        let mut dec = Decoder::new(&bytes);
        let restored = NpcWorldSectionV2::restore_with_limits(&mut dec, &admitting).unwrap();
        dec.finish().unwrap();
        assert_eq!(restored, section);
        // The reservations are tight, never inflated: each column's
        // capacity is exactly its count (the try_reserve shape —
        // no counting allocator needed to see the reservation).
        for block in &restored.regions {
            assert_eq!(block.identities.capacity(), block.identities.len());
            assert_eq!(block.steps.capacity(), block.steps.len());
            assert_eq!(block.milestones.capacity(), block.milestones.len());
            assert_eq!(block.credits.capacity(), block.credits.len());
        }
    }

    /// THE REMAINING IDENTITY BUDGET (CA-02): each block's identity
    /// count is bounded by the population the earlier blocks have NOT
    /// yet claimed — a doctored second block re-declaring the full
    /// population is refused at the count gate (a typed `Limit`
    /// carrying the remaining budget), before any of its columns
    /// reserve. Under the pre-CA-02 bound (the full declared total),
    /// this input passed the count check and reserved eight columns
    /// against it before failing.
    #[test]
    fn the_remaining_identity_budget_bounds_each_region_before_reservation() {
        // Two regions, four identities split 2 + 2 (region 0 Full,
        // region 1 Dormant — the structural dormant shape).
        let section = scaled_section(2, 4, 4, 1);
        let bytes = encode_v2(&section);
        // The valid split restores (the budget gate never rejects a
        // valid encoding).
        let mut dec = Decoder::new(&bytes);
        assert!(
            NpcWorldSectionV2::restore_with_limits(&mut dec, &SectionDecodeLimits::WIRE_MAXIMUM)
                .is_ok()
        );
        // Region 0 (Full, 2 identities): 10 framing bytes + 2×11
        // identity + 2×(8+8+2+8+8) active columns + 2×(8+8+8) record
        // columns = 132 bytes; the region-1 block header follows at
        // 50 + 132, its identity count at offset 184.
        const REGION1_COUNT_AT: usize = 184;
        let mut doctored = bytes.clone();
        doctored[REGION1_COUNT_AT..REGION1_COUNT_AT + 8].copy_from_slice(&4u64.to_le_bytes());
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(
                &mut Decoder::new(&doctored),
                &SectionDecodeLimits::WIRE_MAXIMUM
            ),
            Err(SaveError::Limit {
                section: _,
                kind: "identity-count",
                value: 4,
                max: 2
            })
        ));
    }

    /// THE MINIMUM-REMAINING-BYTES GATE (CA-02): a count within the
    /// generic one-byte-per-element bound (8 identities, 20 bytes
    /// remaining) but unpayable at the identity record's eleven-byte
    /// wire stride (8 × 11 = 88 > 20) is refused BEFORE the
    /// reservation — a typed `Limit` naming the stride-payable
    /// maximum. Under the pre-CA-02 walk this input reserved eight
    /// columns from the count and failed only inside the read loop.
    #[test]
    fn truncated_counts_fail_at_the_minimum_wire_bytes_gate() {
        let mut enc = Encoder::new();
        enc.write_u64(1337); // seed
        enc.write_u64(6); // activity period
        enc.write_u64(4); // simplify factor
        enc.write_u64(12); // commitment window
        enc.write_u8(1); // region-LOD path enabled
        enc.write_count(1); // region count
        enc.write_count(8); // npc count
        enc.write_u8(4); // region grid
        enc.write_u8(0); // near
        enc.write_u8(0); // lod: Full
        enc.write_count(8); // identity count: eight identities claimed
        for _ in 0..20 {
            enc.write_u8(0xAA); // twenty bytes actually present
        }
        let bytes = enc.into_bytes();
        // The generic count bound passes (8 ≤ 20 remaining); the
        // stride gate refuses: 20 / 11 = 1 identity payable.
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(
                &mut Decoder::new(&bytes),
                &SectionDecodeLimits::WIRE_MAXIMUM
            ),
            Err(SaveError::Limit {
                section: _,
                kind: "identity-count",
                value: 8,
                max: 1
            })
        ));
    }

    /// THE RESERVE-BYTES BUDGET (CA-02): the cumulative in-memory
    /// reservation across the section's columns is checked against
    /// the caller's configured byte budget before each reservation —
    /// the transient heap the decode may spend is a caller decision,
    /// published as a typed refusal when the columns would exceed it.
    #[test]
    fn the_reserve_bytes_budget_bounds_the_transient_columns() {
        let section = scaled_section(3, 12, 4, 1);
        let bytes = encode_v2(&section);
        // Region 0 (Full, four identities): the identity table
        // reserves 4 × 12 = 48 in-memory bytes; the steps column
        // would push the cumulative reservation to 80.
        let limits = SectionDecodeLimits::new(12, 50);
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(&mut Decoder::new(&bytes), &limits),
            Err(SaveError::Limit {
                section: _,
                kind: "reserve-bytes",
                value: 80,
                max: 50
            })
        ));
        // A budget covering the section's whole transient reservation
        // admits it (the 4-identity Full block at 62 bytes per
        // identity + two 4-identity Dormant blocks at 36 per
        // identity: 248 + 288 = 536 in-memory column bytes).
        let admitting = SectionDecodeLimits::new(12, 536);
        let mut dec = Decoder::new(&bytes);
        assert!(NpcWorldSectionV2::restore_with_limits(&mut dec, &admitting).is_ok());
        dec.finish().unwrap();
    }

    /// THE RED WITNESS (T21, recorded pre-fix — the assertion is
    /// inverted by the fix commit): the reserve-bytes ledger counts
    /// only the nominal COLUMN bytes, so a decode under a small
    /// configured budget allocates transients far beyond it. A
    /// 256-region, one-identity-per-region all-dormant section's
    /// nominal column sum is 256 × 36 = 9,216 in-memory bytes
    /// (identity 12 + steps 8 + milestones 8 + credits 8 per
    /// identity), and a budget of exactly 9,216 admits the decode
    /// today. But the walk ALSO transiently reserves, never charging
    /// the budget:
    ///
    /// * the regions vector — `Vec::with_capacity(256)` reserves
    ///   exactly 256 × `size_of::<NpcRegionBlock>()` allocator-request
    ///   bytes (measured at runtime below; the exact request size,
    ///   not an estimate), observable as `regions.capacity() == 256`
    ///   on the restored section — on this toolchain ~5.6× the whole
    ///   configured budget by itself;
    /// * the seen-id index — std's `BTreeSet<u32>` nodes hold at
    ///   most 11 entries each, so 256 identities allocate at least
    ///   ⌈256/11⌉ = 24 fixed-layout node allocations (the node
    ///   layout is std-implementation-defined, roughly 160 bytes on
    ///   this toolchain — the counting-allocator census remains the
    ///   follow-up that would measure it; the node COUNT is the
    ///   derived lower bound).
    ///
    /// The caller configured 9,216; the decode spent 9,216 + ~51,200
    /// + ≥ 24 nodes. Post-fix, this same input under the same budget
    /// refuses typed at the regions-reserve gate BEFORE the
    /// reservation is made.
    #[test]
    fn small_configured_budgets_do_not_bound_the_decode_transients_red_witness() {
        let section = scaled_section(256, 256, 16, 0);
        let bytes = encode_v2(&section);
        // The nominal column arithmetic: 256 dormant identities × 36
        // in-memory bytes = 9,216 — the ledger's exact final reading.
        let nominal_columns: u64 = 256 * 36;
        assert_eq!(nominal_columns, 9_216);
        let limits = SectionDecodeLimits::new(256, nominal_columns);
        let mut dec = Decoder::new(&bytes);
        let restored = NpcWorldSectionV2::restore_with_limits(&mut dec, &limits)
            .expect("RED: today the decode is admitted at the nominal column budget");
        dec.finish().unwrap();
        assert_eq!(restored, section);
        // The transient the budget never saw, part 1 (the exact
        // allocator request): the block vector reserved 256 slots.
        assert_eq!(restored.regions.capacity(), 256);
        let regions_reservation = 256 * std::mem::size_of::<NpcRegionBlock>() as u64;
        assert!(
            regions_reservation > nominal_columns,
            "the regions vector alone ({regions_reservation} bytes) exceeds the whole \
             configured budget ({nominal_columns} bytes)"
        );
        // Part 2 (the derived lower bound): the seen-id index's node
        // count — at least one fixed-layout allocation per eleven
        // identities, never charged against any budget.
        assert_eq!(256u64.div_ceil(11), 24);
        // RED: the caller's configured limit was exceeded by
        // transients the ledger never charged. (Post-fix: this exact
        // call refuses with `SaveError::Limit` at the regions-reserve
        // gate, before any reservation is made.)
    }

    /// The multi-scale run fixture: a World whose authoritative state
    /// IS the v2 section (per-region steps columns), executing by the
    /// payload's (region, column) pair and chaining itself — the
    /// configured engine's world representation at any scale.
    struct ScaledFixtureWorld {
        section: NpcWorldSectionV2,
    }

    impl ScaledFixtureWorld {
        fn payload(region: usize, column: usize) -> u64 {
            ((region as u64) << 32) | column as u64
        }

        fn total_steps(&self) -> u64 {
            self.section
                .regions
                .iter()
                .flat_map(|block| block.steps.iter())
                .sum()
        }
    }

    impl World for ScaledFixtureWorld {
        fn execute(&mut self, event: &Event, ctx: &mut ExecCtx) {
            let region = (event.payload >> 32) as usize;
            let column = (event.payload & 0xFFFF_FFFF) as usize;
            if let Some(block) = self.section.regions.get_mut(region)
                && column < block.steps.len()
            {
                block.steps[column] = block.steps[column].saturating_add(1);
            }
            // The fixture chains itself: queued work survives the
            // save/restore boundary and continues (the spine's
            // discipline, at the configured scale).
            ctx.follow_up(1, event.class, event.payload, Vec::new());
        }

        fn fingerprint(&self, fp: &mut Fingerprinter) {
            fp.write_str("engine-core/npc-world-save-v2/fixture/1");
            fp.write_u64(self.section.seed);
            fp.write_u64(u64::from(self.section.config.region_count));
            fp.write_u64(u64::from(self.section.config.npc_count));
            for block in &self.section.regions {
                fp.write_usize(block.steps.len());
                for step in &block.steps {
                    fp.write_u64(*step);
                }
            }
        }
    }

    impl SaveWorld for ScaledFixtureWorld {
        const SAVE_DOMAIN: &'static str = NPC_WORLD_SAVE_DOMAIN_V2;

        fn save(&self, enc: &mut Encoder) {
            self.section.save(enc);
        }

        fn restore(dec: &mut Decoder) -> Result<Self, SaveError> {
            Ok(ScaledFixtureWorld {
                section: NpcWorldSectionV2::restore(dec)?,
            })
        }
    }

    fn fixture_spine(section: NpcWorldSectionV2) -> Spine<ScaledFixtureWorld> {
        // One chained event per ACTIVE-region identity (the dormant
        // regions hold no obligations — the occupied-only rule on the
        // execution side).
        let active = section
            .regions
            .iter()
            .enumerate()
            .filter(|(_, block)| block.lod != LodLevel::Dormant)
            .flat_map(|(region, block)| {
                (0..block.identities.len()).map(move |column| (region, column))
            })
            .collect::<Vec<_>>();
        let mut spine = Spine::with_reservation(ScaledFixtureWorld { section }, 200, 60);
        for (region, column) in active {
            spine
                .schedule(EventSpec::new(
                    EventClass::new(region as u16),
                    Schedule::AtLogicalTime {
                        due: Tick(1 + (column as u64 % 7)),
                    },
                    ScaledFixtureWorld::payload(region, column),
                ))
                .expect("the fixture schedule is dependency-free");
        }
        spine
    }

    #[test]
    fn a_configured_world_loads_and_runs_at_scale() {
        // THE MULTI-SCALE DEMONSTRATION (the lane's mandated fixture):
        // a world at a DIFFERENT scale — 64 regions, 10,000 stable
        // identities, four active — loading and RUNNING through the
        // configured engine (the spine): real executions mutate the
        // restored per-region state, the save/restore boundary is
        // continuation-invisible against a never-saved control, and
        // the world section's size is exactly the occupied-only
        // formula (no dense grid allocation proportional to the
        // galaxy — no region-count × grid² term anywhere; the 60
        // dormant regions pay 35 bytes per identity, records only).
        let section = scaled_section(64, 10_000, 16, 4);
        section.validate().unwrap();
        let active_identities = section
            .regions
            .iter()
            .filter(|block| block.lod != LodLevel::Dormant)
            .map(|block| block.identities.len())
            .sum::<usize>();
        assert_eq!(active_identities, 628, "four of sixty-four regions active");

        let mut control = fixture_spine(section.clone());
        let mut saved = fixture_spine(section);
        control.run_ticks(20);
        saved.run_ticks(20);
        assert_eq!(control.stats().executed, saved.stats().executed);
        assert!(
            saved.stats().executed > 3_000,
            "the fixture really ran (executed {})",
            saved.stats().executed
        );
        let bytes = saved.save_bytes();
        // The world section as stored: the spine framing carries the
        // section bytes verbatim, so the section's own encoding — at
        // the post-run state — is exactly the occupied-only size.
        let stored = encode_v2(&saved.world().section);
        assert_eq!(stored.len(), expected_v2_len(&saved.world().section));

        let mut restored =
            Spine::<ScaledFixtureWorld>::restore(&bytes, Diagnostics::new(Level::Off)).unwrap();
        assert_eq!(restored.world().section, saved.world().section);
        assert_eq!(restored.save_bytes(), bytes);

        // Continuation invisibility: the restored spine continues
        // exactly like the never-saved control — engine state, world
        // state (the per-region steps), and the execution count.
        control.run_ticks(25);
        restored.run_ticks(25);
        assert_eq!(
            control.state_fingerprint(),
            restored.state_fingerprint(),
            "the full logical state continues identically"
        );
        assert_eq!(control.stats().executed, restored.stats().executed);
        assert_eq!(
            control.world().total_steps(),
            restored.world().total_steps()
        );
        assert!(restored.world().total_steps() > 4_000);
    }

    #[test]
    fn active_positions_face_one_gate_in_memory_and_on_the_wire() {
        // THE F06 WITNESS AS THE CRATE'S OWN BATTERY: pre-fix, an
        // in-memory section with an off-grid active position (the
        // audit's (4,0) on a 4-grid) PASSED validate() and FAILED its
        // own restore with `npc-world-save-v2/position-bounds` — the
        // two gates disagreed (the reproduced external witness, exit
        // 101). Both gates now consult the ONE shared predicate
        // (`position_admitted`), so they answer identically BY
        // CONSTRUCTION for every position class: the ordinary cell,
        // the exact edge (grid−1 on both axes), the dormant-discard
        // sentinel, the PARTIAL sentinel (one sentinel coordinate —
        // the exact-tuple rule makes it an ordinary off-grid
        // refusal), and the witness's own (4,0) plus its mirrored
        // axis — with the same production error name on refusal.
        let mut section = scaled_section(1, 1, 4, 1);
        for (position, admitted) in [
            ((0, 0), true),                  // the ordinary cell
            ((3, 3), true),                  // the exact edge: grid−1 on both axes
            (POSITION_DISCARDED, true),      // the dormant-discard sentinel
            ((u8::MAX, 0), false),           // partial sentinel: exact-tuple rule, no exemption
            ((u8::MAX, u8::MAX - 1), false), // the mirrored partial sentinel
            ((4, 0), false),                 // THE WITNESS CASE: off-grid, non-sentinel
            ((0, 4), false),                 // the mirrored off-grid axis
        ] {
            section.regions[0].positions[0] = position;
            let validated = section.validate();
            let bytes = encode_v2(&section);
            let restored = NpcWorldSectionV2::restore(&mut Decoder::new(&bytes));
            // The witness's own assertion: the gates agree.
            assert_eq!(
                validated.is_ok(),
                restored.is_ok(),
                "position {position:?}: the in-memory and wire gates disagree"
            );
            assert_eq!(
                validated.is_ok(),
                admitted,
                "position {position:?}: admitted iff on-grid-or-sentinel"
            );
            // Refusal identity: BOTH gates name the same production
            // error (the witness's restore error, now validate's too).
            if !admitted {
                assert!(
                    matches!(
                        validated,
                        Err(SaveError::Integrity("npc-world-save-v2/position-bounds"))
                    ),
                    "position {position:?}: validate names the position-bounds error"
                );
                assert!(
                    matches!(
                        restored,
                        Err(SaveError::Integrity("npc-world-save-v2/position-bounds"))
                    ),
                    "position {position:?}: restore names the position-bounds error"
                );
            }
        }
        // The dormant shape is unaffected: a Dormant block's positions
        // are empty (the structural rule), so the gate is the active
        // blocks' alone.
        let dormant = scaled_section(2, 6, 4, 0);
        dormant.validate().expect("the dormant section validates");
    }

    #[test]
    fn the_validated_encoding_path_refuses_what_restore_would() {
        // F06's wire half: the raw `save` is total (the v1 mirror
        // discipline), so a caller that saves an UNVALIDATED section
        // can write bytes restore refuses — the same disagreement at
        // the wire. `save_validated` closes it for public callers:
        // the off-grid section is refused BEFORE any byte is written,
        // and a valid section's bytes through the validated path are
        // IDENTICAL to the raw path's (existing valid bytes
        // preserved).
        let mut off_grid = scaled_section(2, 6, 16, 2);
        off_grid.regions[0].positions[0] = (16, 0); // off-grid, non-sentinel
        let mut enc = Encoder::new();
        let refused = off_grid.save_validated(&mut enc);
        assert!(
            matches!(
                refused,
                Err(SaveError::Integrity("npc-world-save-v2/position-bounds"))
            ),
            "the validated path names the shared gate's error"
        );
        assert!(
            enc.as_bytes().is_empty(),
            "the refusal happens before any byte is written"
        );
        // The raw codec remains total — and restore still refuses
        // its bytes (the disagreement save_validated exists to close,
        // pinned here as the discriminating pair).
        let raw = encode_v2(&off_grid);
        assert!(NpcWorldSectionV2::restore(&mut Decoder::new(&raw)).is_err());
        // A valid section: byte-identical through both paths.
        let valid = scaled_section(2, 6, 16, 2);
        valid.validate().expect("the fixture section is consistent");
        let mut enc = Encoder::new();
        valid
            .save_validated(&mut enc)
            .expect("the valid section encodes");
        assert_eq!(enc.into_bytes(), encode_v2(&valid));
        // And those bytes restore (the round-trip the path promises).
        let valid_bytes = encode_v2(&valid);
        let mut dec = Decoder::new(&valid_bytes);
        let restored = NpcWorldSectionV2::restore(&mut dec).unwrap();
        dec.finish().unwrap();
        assert_eq!(restored, valid);
    }
}
