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

use std::cmp::Reverse;
use std::collections::BinaryHeap;

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
    ///
    /// T21 (the transient arithmetic stated): the v1 workload is
    /// FIXED — 1,000 identities, eight regions — so this walk's
    /// total transient reservation is a constant (seven 1,000-element
    /// columns, 8,000 bytes each: 56,000 nominal), reserved fallibly
    /// (`try_reserve` — an allocation failure is the typed
    /// [`SaveError::Overflow`] refusal, never an abort) and needing
    /// no limits type (the documented CA-02 decision: affordability
    /// is definitionally granted by the fixed v1 interpretation;
    /// callers decoding less-trusted v1 bytes get their admission at
    /// the byte framing the spine and the anchor own).
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
        let mut steps = reserved_column(NPC_COUNT, SECTION)?;
        for _ in 0..NPC_COUNT {
            steps.push(dec.read_u64()?);
        }
        let mut routine_cursor = reserved_column(NPC_COUNT, SECTION)?;
        for _ in 0..NPC_COUNT {
            routine_cursor.push(dec.read_u64()?);
        }
        // Grid positions carry the dormant-discard sentinel
        // (`POSITION_DISCARDED` — the reconstructible representation
        // is GONE, not merely flagged); every other position must lie
        // on the region-local grid (the bounds invariant).
        let mut positions = reserved_column(NPC_COUNT, SECTION)?;
        for _ in 0..NPC_COUNT {
            let x = dec.read_u8()?;
            let y = dec.read_u8()?;
            if (x, y) != POSITION_DISCARDED && (x >= REGION_GRID || y >= REGION_GRID) {
                return Err(SaveError::Integrity("npc-world-save/position-bounds"));
            }
            positions.push((x, y));
        }
        let mut represented_through = reserved_column(NPC_COUNT, SECTION)?;
        for _ in 0..NPC_COUNT {
            represented_through.push(dec.read_tick()?);
        }
        let mut milestones = reserved_column(NPC_COUNT, SECTION)?;
        for _ in 0..NPC_COUNT {
            milestones.push(dec.read_u64()?);
        }
        let mut pending_signals = reserved_column(NPC_COUNT, SECTION)?;
        for _ in 0..NPC_COUNT {
            pending_signals.push(dec.read_u64()?);
        }
        let mut credits = reserved_column(NPC_COUNT, SECTION)?;
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
/// (CA-02/F02, Q07, T21 — the resource-admission half of the L8
/// boundary): what the DECODER may spend materializing a section, as
/// a CALLER decision distinct from what the WIRE admits.
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
/// **What `max_reserve_bytes` bounds (T21 — the TOTAL, not the
/// columns):** the ledger charges EVERY transient heap reservation
/// the decode makes, each at its actual requested capacity rounded
/// up to a documented allocation granularity
/// ([`ALLOCATION_GRANULARITY`]):
///
/// * the per-block column vectors (`count × size_of::<T>()` — the
///   exact capacity `try_reserve` requests, observable as
///   `capacity() == len()` on every decoded column);
/// * the regions vector itself (`region_count ×
///   size_of::<NpcRegionBlock>()` — the block-vector transient
///   CA-02 left as a plain reservation);
/// * the cross-region uniqueness merge's cursor scratch (at most
///   one twelve-byte cursor per region — the seen-id `BTreeSet`
///   node stream CA-02 left unaccounted is gone entirely; see
///   [`NpcWorldSectionV2::restore_with_limits`]).
///
/// A budget covering exactly the charge is computable WITHOUT
/// decoding, from the section itself:
/// [`NpcWorldSectionV2::decode_reservation_bytes`] is the same
/// arithmetic the decoder's ledger runs, so a composition caller
/// can configure the budget that admits precisely the section it
/// authored — and one byte less refuses it, typed.
///
/// The granularity rounding is the documented conservative
/// per-allocation overhead: std reserves "at least" the requested
/// capacity, and the allocator may round a request into a larger
/// size class — 64-byte granularity covers the common small-block
/// classes (glibc's 16-byte-aligned chunks with header, jemalloc's
/// 16-byte spacing, the Windows CRT's small-block heap) with
/// margin. Allocators with coarser classes are honestly OUTSIDE
/// this accounting: the budget is an admission contract over
/// requested bytes plus this documented overhead, and
/// allocator-level telemetry remains the counting-allocator census
/// follow-up (the note below).
///
/// **The scoped/global identity policy is NOT this type's decision**
/// (a separate owner question, deliberately out of scope): the v2
/// format's identity table is globally scoped — ids unique across
/// the whole section, blocks positional and region-major — and
/// whether a future format scopes identities per region is a
/// format-versioning decision, never a decode-limit decision.
///
/// **The production-entry boundary (the R5 census, 2026-11-28):**
/// every PRODUCTION world-section decode entry in this tree flows
/// TRUSTED-LOCAL saves, and none decodes v2 bytes at all yet:
///
/// * the application's [`crate::SaveWorld`] implementation for its
///   `NpcWorld` (engine-slice's `npc` module) delegates to the V1
///   [`NpcWorldSection::restore`] — reached only through
///   `Spine::<NpcWorld>::restore` inside engine-slice's
///   `restore_session`, whose bytes are the session snapshot the
///   process itself serialized (the capture-validation round trip)
///   or the persist medium's hash-verified, content-addressed,
///   single-writer LOCAL storage (the recovery's HEAD-descent walk
///   reads every object through `read_object`'s SHA-256
///   verification against the requested identity);
/// * the persist rotation anchor (`engine-persist`'s
///   `session_anchor`) validates the newest snapshot through
///   `Spine::<NpcSaveWorld>::restore` — the same fixed v1 walk,
///   validation only;
/// * the v2 codec has ZERO production consumers: `NpcSaveWorldV2`
///   exists for the future authenticated v2 anchor (the P3
///   coordination recorded at its doc), and
///   [`NpcWorldSectionV2::restore_with_limits`] is called today
///   only by tests.
///
/// The v1 walk needs no limits type (its identity set is the fixed
/// 1,000-record v1 interpretation — transients a 56,000-byte-nominal
/// constant, reserved fallibly; affordability is definitionally
/// granted), and the [`crate::SaveWorld::restore`] trait signature
/// carries no limits parameter BY the documented CA-02 owner
/// decision — affordability belongs to the entry that owns the
/// bytes, never to the generic codec. No network/co-op transport
/// exists anywhere in the tree (networking is on the
/// anti-abstraction ledger of both engine-core and engine-persist);
/// the reviewer's "network/untrusted load" is the FIRST GAME's
/// design goal, not landed code. WHEN such an entry lands — or any
/// boundary restoring bytes it did not author — it must call
/// [`NpcWorldSectionV2::restore_with_limits`] with a FINITE budget
/// ([`Self::for_config`] derives one from the composition's own
/// authored [`WorldConfig`]), never [`Self::WIRE_MAXIMUM`]: the wire
/// maximum is a COMPATIBILITY limit (what a well-formed section may
/// declare), never an affordability claim (what this decoder can
/// spend materializing it).
///
/// The default ([`Self::WIRE_MAXIMUM`]) is exactly the wire's own
/// validity domain, so the plain [`NpcWorldSectionV2::restore`]
/// entry keeps today's acceptance byte for byte; composition
/// callers that decode less-trusted bytes pass smaller limits
/// through [`NpcWorldSectionV2::restore_with_limits`] and every
/// over-budget input becomes a typed [`SaveError::Limit`] refusal
/// BEFORE any reservation is made.
///
/// **The default's transient arithmetic (T21, stated):** at
/// [`Self::WIRE_MAXIMUM`] the byte budget is not enforced
/// (`u64::MAX`), and the decode's transients are bounded by the
/// whole-input minimum-size gate instead: a section payload of `S`
/// bytes admits at most `(S − 10·R) / 35` identities (each
/// identity's dormant wire minimum is 35 bytes, each block's
/// framing 10), so the column reservations are at most
/// `62/35 · S ≈ 1.8 × S` (the active in-memory stride over the
/// dormant wire minimum) plus the fixed partition transients — the
/// block vector at most `256 × size_of::<NpcRegionBlock>()`, the
/// merge cursors at most `256 × 12` bytes, and the per-column
/// granularity slack — roughly `1.8 ×` the input plus a ~54 KiB
/// constant. CA-02's pre-gate shape reserved ~62 × the input; the
/// whole-input gate is what the wire maximum now means.
///
/// **The allocation-census boundary (an honest note):** the
/// thread-attributed counting-allocator census — the
/// `tests/spine_allocations.rs` / `tests/spatial_query_allocations.rs`
/// precedent — measures the decode path only from a dedicated
/// integration binary (a `#[global_allocator]` is one per binary),
/// which this codec's owning lanes did not add; the inline battery
/// proves the same property structurally (the typed pre-reservation
/// refusals plus the exact-capacity reservations — the reservation
/// sizes are observable on every decoded section, and the ledger's
/// charge is published by
/// [`NpcWorldSectionV2::decode_reservation_bytes`]), and the census
/// binary remains the natural follow-up if the composition boundary
/// adopting `restore_with_limits` wants allocator-level telemetry.
/// **What the census battery would carry (the designed shape, R5 —
/// deliberately NOT implemented here):** `tests/npc_decode_allocations.rs`,
/// one `#[global_allocator]` per binary, pinning for a fixed section
/// under [`Self::WIRE_MAXIMUM`] and under [`Self::for_config`]
/// budgets that the decode's allocation EVENT count equals the
/// ledger's reservation count, that the allocation BYTES are at most
/// the ledger's charge under the platform's real size classes (the
/// 64-byte estimate's honesty made measurable, per-thread attributed
/// the spine-allocation precedent's waker-registration lesson), and
/// that every typed refusal path makes ZERO allocation events. It is
/// not implemented on this lane by design: the counting allocator
/// is engine-core's SECOND `unsafe` `GlobalAlloc` exception under
/// the workspace's `unsafe_code = "deny"` (the first,
/// `tests/spine_allocations.rs`, was admitted by an owner ruling
/// with its own safety exception doc) — an unsafe-exception policy
/// decision belonging to a lane brief carrying that authority, not
/// to a docs-and-wiring lane under a no-unsafe invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionDecodeLimits {
    /// The maximum total identity records (the declared
    /// `config.npc_count`) this decoder will materialize: the
    /// aggregate over-budget refusal. Must not exceed
    /// [`MAX_NPC_COUNT`] to have any effect (the config gate refuses
    /// larger declarations first).
    pub max_identities: u64,
    /// The maximum cumulative in-memory bytes the decode may reserve
    /// across EVERY transient it allocates (T21's total budget: the
    /// per-block column vectors, the regions vector, and the
    /// uniqueness-merge scratch — each charged at its requested
    /// capacity rounded up to the documented allocation granularity;
    /// see the type doc for the full accounting and
    /// [`NpcWorldSectionV2::decode_reservation_bytes`] for the exact
    /// charge a given section will incur).
    pub max_reserve_bytes: u64,
}

impl SectionDecodeLimits {
    /// The wire-valid maximum: the identity codec's full domain and
    /// an unenforced byte budget — exactly the acceptance the plain
    /// [`NpcWorldSectionV2::restore`] entry has always had (the
    /// default's transient arithmetic is stated in the type doc:
    /// the whole-input minimum-size gate bounds the decode to
    /// ~1.8× the input plus the fixed partition transients).
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

    /// The FINITE composition budget derived from the caller's own
    /// authored configuration (R5 — the config-class affordability
    /// decision, the budget a boundary restoring LESS-TRUSTED bytes
    /// configures instead of [`Self::WIRE_MAXIMUM`]): `max_identities`
    /// is exactly the authored population, and `max_reserve_bytes`
    /// is the closed-form WORST-CASE charge any section carrying a
    /// workload at or below that configuration can incur — the
    /// partition transients (the block vector and the
    /// uniqueness-merge scratch at the authored region count) plus
    /// every column at the authored population in the ALL-ACTIVE
    /// shape (the dormant shape is its subset), each column's
    /// per-block charge sum bounded by `npc_count × stride +
    /// (granularity − 1) × region_count` (the split's rounding
    /// slack: a concentrated split pays the nominal, a
    /// one-identity-per-region split pays the per-block rounding —
    /// the bound covers both).
    ///
    /// The budget is a SAFE UPPER BOUND, never the minimal charge:
    /// a known section's exact charge is
    /// [`NpcWorldSectionV2::decode_reservation_bytes`], and this
    /// budget is at or above it for every section whose declared
    /// population is at most `config.npc_count` and whose partition
    /// fits the authored region count — so restoring the
    /// composition's own saves NEVER refuses (the gates never reject
    /// a valid encoding), while a section declaring MORE than the
    /// authored workload refuses TYPED ([`SaveError::Limit`]) — more
    /// identities at the aggregate identity gate, a wider partition
    /// at the reserve ledger — BEFORE the reservation it refuses.
    /// The wire's own validity domain and this budget are the two
    /// contracts the R5 census keeps distinct: the wire maximum is a
    /// COMPATIBILITY limit (what a well-formed section may declare),
    /// never an affordability claim (what this decoder can spend
    /// materializing it).
    ///
    /// Fail-closed on a configuration outside [`WorldConfig`]'s
    /// domain (exactly [`WorldConfig::validated`]'s gates), never
    /// repaired.
    pub fn for_config(config: WorldConfig) -> Result<Self, SaveError> {
        const SECTION: &str = "section-decode-limits";
        config
            .validated()
            .map_err(|_| SaveError::Integrity("section-decode-limits/world-config"))?;
        let region_count = u64::from(config.region_count);
        let npc_count = u64::from(config.npc_count);
        // The partition transients at the authored region count: the
        // block vector and the uniqueness-merge scratch.
        let block_vector = region_count
            .checked_mul(std::mem::size_of::<NpcRegionBlock>() as u64)
            .ok_or(SaveError::Overflow { section: SECTION })?;
        let mut total = allocation_charge(block_vector)?;
        let scratch = region_count
            .checked_mul(std::mem::size_of::<UniquenessCursor>() as u64)
            .ok_or(SaveError::Overflow { section: SECTION })?;
        total = total
            .checked_add(allocation_charge(scratch)?)
            .ok_or(SaveError::Overflow { section: SECTION })?;
        // Every column at the authored population in the ALL-ACTIVE
        // shape, with the per-block split's rounding slack bounded
        // by one granularity per non-empty block: the sum over
        // blocks of ceil64(count × stride) is at most
        // npc_count × stride + (granularity − 1) × region_count
        // (the one-count-governs rule makes the nominal terms
        // split-independent).
        let slack = (ALLOCATION_GRANULARITY - 1)
            .checked_mul(region_count)
            .ok_or(SaveError::Overflow { section: SECTION })?;
        for stride in ACTIVE_COLUMN_STRIDES {
            let nominal = npc_count
                .checked_mul(stride)
                .ok_or(SaveError::Overflow { section: SECTION })?;
            let bound = nominal
                .checked_add(slack)
                .ok_or(SaveError::Overflow { section: SECTION })?;
            total = total
                .checked_add(bound)
                .ok_or(SaveError::Overflow { section: SECTION })?;
        }
        Ok(SectionDecodeLimits {
            max_identities: npc_count,
            max_reserve_bytes: total,
        })
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
    /// whole-input minimum-size gate and the minimum-bytes gates
    /// below still apply, so no reservation ever exceeds what the
    /// wire could actually supply for that count; see
    /// [`SectionDecodeLimits`] for the default's transient
    /// arithmetic).
    pub fn restore(dec: &mut Decoder) -> Result<Self, SaveError> {
        Self::restore_with_limits(dec, &SectionDecodeLimits::WIRE_MAXIMUM)
    }

    /// Restores the v2 section under CALLER-CONFIGURED admission
    /// limits (CA-02/F02, T21 — see [`SectionDecodeLimits`]): the
    /// same fail-closed walk as [`Self::restore`], with the decoder's
    /// resource admission decided by the caller rather than the
    /// wire's validity domain alone. Five gates close the
    /// reservation-abort shape, each BEFORE the reservation it
    /// guards:
    ///
    /// * **The aggregate identity budget**: a declared population
    ///   above `limits.max_identities` is refused before any region
    ///   decodes (the over-budget workload refusal).
    /// * **The whole-input minimum-size proof (T21)**: before ANY
    ///   significant allocation, `region_count` blocks × 10 framing
    ///   wire bytes + `npc_count` identities × 35 dormant-minimum
    ///   wire bytes must fit in the bytes actually remaining — a
    ///   truncated or huge-declaring input refuses here, typed,
    ///   before the block vector or any column reserves. The gate
    ///   never rejects a valid encoding (a complete section's
    ///   payload is exactly its blocks' bytes, at least every
    ///   block's dormant minimum). For a COMPLETE section it
    ///   subsumes the per-column stride gate below — with the
    ///   remaining-identity budget bounding every block count by
    ///   the unclaimed population, each column's stride is payable
    ///   from the dormant minima of the identities the section
    ///   still owes; a TRUNCATED input whose active blocks sit
    ///   above the dormant minimum is exactly what the stride gates
    ///   (kept as defense in depth) still catch, column-named.
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
    /// * **The total reserve-bytes budget (T21)**: EVERY transient
    ///   heap reservation — the regions vector, each column at its
    ///   actual requested capacity, the uniqueness-merge scratch,
    ///   each rounded up to the documented allocation granularity —
    ///   is charged against `limits.max_reserve_bytes` before it is
    ///   requested (the `regions-reserve`, `reserve-bytes`, and
    ///   `uniqueness-scratch` refusals; the exact charge for a
    ///   known section is published by
    ///   [`Self::decode_reservation_bytes`]).
    ///
    /// All count arithmetic is checked, every reservation is
    /// fallible (`try_reserve` — an allocation failure is a typed
    /// [`SaveError::Overflow`] refusal, never an abort), and the
    /// cross-region uniqueness proof is a bounded k-way merge over
    /// the per-block identity tables (each already strictly
    /// ascending, wire-enforced) instead of the per-ID `BTreeSet`
    /// node stream CA-02 left unaccounted: at most one cursor per
    /// region, charged and fallibly reserved before the merge runs,
    /// never a dense bitset over the sparse u32 id universe. The
    /// duplicate refusal stays the typed [`SaveError::Duplicate`]
    /// naming the id, and still fires BEFORE the population-sum
    /// integrity gate (the pre-T21 error preference); a
    /// compound-malformed input may now name its other defect
    /// first, since the merge runs after the walk (the variant and
    /// the named id are unchanged).
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
        // THE WHOLE-INPUT MINIMUM-SIZE PROOF (T21): before ANY
        // significant allocation, the declared partition and
        // population must be payable at each identity's DORMANT
        // minimum — region_count blocks of 10 framing wire bytes
        // (near, lod, the fixed-width identity count) plus npc_count
        // identities of 35 wire bytes (the 11-byte identity record
        // and the three eight-byte record columns) must fit in the
        // bytes actually remaining. A complete section's payload is
        // exactly its blocks' bytes, at least every block's dormant
        // minimum, so the gate never rejects a valid encoding; a
        // truncated or huge-declaring input refuses HERE, typed,
        // before the block vector or any column reserves. This
        // subsumes the per-column wire-stride gates below (kept as
        // defense in depth): the remaining-identity budget bounds
        // every block count by the unclaimed population, so each
        // column's stride is payable from the dormant minima of the
        // identities the section still owes.
        let framing_minimum = region_count
            .checked_mul(BLOCK_FRAMING_WIRE_BYTES)
            .ok_or(SaveError::Overflow { section: SECTION })?;
        let identity_minimum = npc_count
            .checked_mul(DORMANT_IDENTITY_WIRE_BYTES)
            .ok_or(SaveError::Overflow { section: SECTION })?;
        let section_minimum = framing_minimum
            .checked_add(identity_minimum)
            .ok_or(SaveError::Overflow { section: SECTION })?;
        let remaining = dec.remaining() as u64;
        if section_minimum > remaining {
            return Err(SaveError::Limit {
                section: SECTION,
                kind: "section-minimum-bytes",
                value: section_minimum,
                max: remaining,
            });
        }
        // The cumulative in-memory reservation across EVERY
        // transient the decode allocates (T21's total budget: the
        // block vector, the columns, the uniqueness scratch — each
        // charged at the granularity-rounded request).
        let mut reserved: u64 = 0;
        // THE REGIONS VECTOR (T21): charged against the budget and
        // reserved fallibly. The block vector is bounded by the
        // constant region-field width (256 blocks), but under a
        // caller's small budget it is transient heap the ledger must
        // see BEFORE it is requested — the red witness's shape (a
        // decode admitted at the nominal column budget while the
        // block vector alone reserved ~5.6× it).
        let region_slots =
            usize::try_from(region_count).map_err(|_| SaveError::Overflow { section: SECTION })?;
        charge_reserve(
            region_count
                .checked_mul(std::mem::size_of::<NpcRegionBlock>() as u64)
                .ok_or(SaveError::Overflow { section: SECTION })?,
            "regions-reserve",
            limits,
            &mut reserved,
        )?;
        let mut regions = Vec::new();
        regions
            .try_reserve(region_slots)
            .map_err(|_| SaveError::Overflow { section: SECTION })?;
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
                // block — the dense positional discipline, made
                // wire-checkable, and the sortedness the post-walk
                // cross-region uniqueness merge rides (T21: global
                // uniqueness is proven by the bounded merge, not by
                // per-ID index nodes).
                if let Some(prior) = prev_id
                    && identity.id.0 <= prior
                {
                    return Err(SaveError::Order { section: SECTION });
                }
                prev_id = Some(identity.id.0);
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
        // THE CROSS-REGION UNIQUENESS MERGE (T21): the seen-id
        // `BTreeSet` is gone — its per-ID tree nodes were transient
        // index memory no budget saw (at least one fixed-layout
        // allocation per eleven identities, unbounded by any caller
        // limit). Every block's table is strictly ascending (the
        // wire-enforced canonical order above), so a bounded k-way
        // merge proves GLOBAL uniqueness: at most one cursor per
        // region (never a dense bitset over the sparse u32 id
        // universe), O(n log R), and the cursor scratch is charged
        // and fallibly reserved before the merge runs. The duplicate
        // refusal is still the typed `SaveError::Duplicate` naming
        // the id, and still fires BEFORE the population-sum
        // integrity gate (the pre-T21 error preference).
        charge_reserve(
            region_count
                .checked_mul(std::mem::size_of::<UniquenessCursor>() as u64)
                .ok_or(SaveError::Overflow { section: SECTION })?,
            "uniqueness-scratch",
            limits,
            &mut reserved,
        )?;
        let mut cursors: Vec<Reverse<UniquenessCursor>> = Vec::new();
        cursors
            .try_reserve(region_slots)
            .map_err(|_| SaveError::Overflow { section: SECTION })?;
        let mut heap = BinaryHeap::from(cursors);
        assert_cross_region_uniqueness(&regions, &mut heap)?;
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

    /// The exact total this section's encoding will charge against
    /// [`SectionDecodeLimits::max_reserve_bytes`] at decode (T21 —
    /// the composition caller's affordability arithmetic): the
    /// regions vector, every column at its actual requested
    /// capacity, and the uniqueness-merge scratch, each rounded up
    /// to the documented allocation granularity — the SAME ledger
    /// [`Self::restore_with_limits`] accumulates, computed without
    /// decoding. A budget at or above this value admits the
    /// section's own encoding; a budget one byte below refuses it,
    /// typed, at the charge that crosses (the off-by-one admission
    /// boundary, pinned by test). The charge follows the section's
    /// SHAPE — the `lod`-selected dormant/active column set, exactly
    /// as the wire walk reads it; `validate` the section first if
    /// the caller wants the canonical shape.
    pub fn decode_reservation_bytes(&self) -> Result<u64, SaveError> {
        const SECTION: &str = "npc-world-save-v2";
        let region_count = u64::from(self.config.region_count);
        let block_bytes = region_count
            .checked_mul(std::mem::size_of::<NpcRegionBlock>() as u64)
            .ok_or(SaveError::Overflow { section: SECTION })?;
        let mut total = allocation_charge(block_bytes)?;
        for block in &self.regions {
            let count = u64::try_from(block.identities.len())
                .expect("the identity count is bounded by the configured population");
            // The column strides the walk charges for this block's
            // shape: the identity table, steps, milestones, and
            // credits always; the four reconstructible columns only
            // in the active shape (the structural dormant rule).
            // The SHARED stride tables — for_config's worst case
            // derives from the same numbers, never a second copy.
            let strides: &[u64] = if block.lod == LodLevel::Dormant {
                &DORMANT_COLUMN_STRIDES
            } else {
                &ACTIVE_COLUMN_STRIDES
            };
            for stride in strides {
                let nominal = count
                    .checked_mul(*stride)
                    .ok_or(SaveError::Overflow { section: SECTION })?;
                let charge = allocation_charge(nominal)?;
                total = total
                    .checked_add(charge)
                    .ok_or(SaveError::Overflow { section: SECTION })?;
            }
        }
        let scratch = region_count
            .checked_mul(std::mem::size_of::<UniquenessCursor>() as u64)
            .ok_or(SaveError::Overflow { section: SECTION })?;
        total
            .checked_add(allocation_charge(scratch)?)
            .ok_or(SaveError::Overflow { section: SECTION })
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
            }
            total_identities = total_identities
                .checked_add(u64::try_from(count).expect("counts are bounded by the identity set"))
                .ok_or(SaveError::Overflow { section: SECTION })?;
        }
        // THE CROSS-REGION UNIQUENESS MERGE (T21 — the same bounded
        // merge the wire walk runs): the per-block ascending order
        // was checked in the loop above, so the merge over the
        // in-memory tables proves global uniqueness with at most one
        // cursor per region (the scratch is bounded by the constant
        // region-field width here — no budget applies to validate,
        // and 256 twelve-byte cursors is affordable by definition).
        let cursors: Vec<Reverse<UniquenessCursor>> = Vec::with_capacity(self.regions.len());
        let mut heap = BinaryHeap::from(cursors);
        assert_cross_region_uniqueness(&self.regions, &mut heap)?;
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

/// The documented conservative per-allocation overhead (T21): every
/// decode reservation is charged at its requested byte count rounded
/// UP to this granularity. `Vec::try_reserve` requests exactly the
/// capacity it is asked for (observable on every decoded column:
/// `capacity() == len()`), but std reserves "at least" the requested
/// capacity and the allocator may round the request into a larger
/// size class — 64-byte granularity covers the common small-block
/// classes (glibc's 16-byte-aligned chunks with their header,
/// jemalloc's 16-byte spacing, the Windows CRT's small-block heap)
/// with margin. Allocators with coarser classes are honestly outside
/// this accounting: the budget is an admission contract over
/// requested bytes plus this documented overhead, and allocator-level
/// telemetry remains the counting-allocator census follow-up (see
/// [`SectionDecodeLimits`]'s census note).
const ALLOCATION_GRANULARITY: u64 = 64;

/// The per-identity IN-MEMORY column strides the v2 walk reserves
/// for the DORMANT (record-only) block shape: the identity table,
/// steps, milestones, and credits. THE ONE stride table shared by
/// [`NpcWorldSectionV2::decode_reservation_bytes`] (the exact-charge
/// mirror) and [`SectionDecodeLimits::for_config`] (the config-class
/// budget's worst case) — never two tables that could drift.
const DORMANT_COLUMN_STRIDES: [u64; 4] = [
    std::mem::size_of::<NpcIdentity>() as u64,
    std::mem::size_of::<u64>() as u64,
    std::mem::size_of::<u64>() as u64,
    std::mem::size_of::<u64>() as u64,
];

/// The per-identity IN-MEMORY column strides of the ACTIVE block
/// shape: the dormant record columns PLUS the four reconstructible
/// columns (routine cursor, positions, represented-through,
/// pending signals). The active set is the SUPERSET — the worst
/// case every aggregate budget must cover (see
/// [`DORMANT_COLUMN_STRIDES`] for the one-table discipline).
const ACTIVE_COLUMN_STRIDES: [u64; 8] = [
    std::mem::size_of::<NpcIdentity>() as u64,
    std::mem::size_of::<u64>() as u64,
    std::mem::size_of::<u64>() as u64,
    std::mem::size_of::<(u8, u8)>() as u64,
    std::mem::size_of::<Tick>() as u64,
    std::mem::size_of::<u64>() as u64,
    std::mem::size_of::<u64>() as u64,
    std::mem::size_of::<u64>() as u64,
];

/// The wire bytes every region block costs before any identity
/// (T21's whole-input minimum gate): the near flag, the LOD
/// discriminant, and the identity count — a fixed-width u64 through
/// `Encoder::write_count`.
const BLOCK_FRAMING_WIRE_BYTES: u64 = 10;

/// The minimum wire bytes ONE identity can occupy in a complete
/// section (T21's whole-input minimum gate): the DORMANT record-only
/// shape — the eleven-byte identity record plus the three record
/// columns (steps, milestones, credits — eight wire bytes each). An
/// active identity costs more (61); the dormant minimum is what the
/// gate may assume.
const DORMANT_IDENTITY_WIRE_BYTES: u64 = NpcIdentity::WIRE_STRIDE as u64 + 24;

/// Rounds a nominal reservation up to the allocation granularity
/// (checked; the T21 per-allocation overhead — see
/// [`ALLOCATION_GRANULARITY`]).
fn allocation_charge(nominal: u64) -> Result<u64, SaveError> {
    const SECTION: &str = "npc-world-save-v2";
    let padded = nominal
        .checked_add(ALLOCATION_GRANULARITY - 1)
        .ok_or(SaveError::Overflow { section: SECTION })?;
    Ok(padded / ALLOCATION_GRANULARITY * ALLOCATION_GRANULARITY)
}

/// Charges one transient reservation (nominal bytes) against the
/// caller's total budget, accumulating the section ledger (T21): the
/// charge is the granularity-rounded request; the refusal is a typed
/// `Limit` naming the caller's budget, BEFORE the reservation is
/// made.
fn charge_reserve(
    nominal: u64,
    kind: &'static str,
    limits: &SectionDecodeLimits,
    reserved: &mut u64,
) -> Result<(), SaveError> {
    const SECTION: &str = "npc-world-save-v2";
    let charge = allocation_charge(nominal)?;
    *reserved = reserved
        .checked_add(charge)
        .ok_or(SaveError::Overflow { section: SECTION })?;
    if *reserved > limits.max_reserve_bytes {
        return Err(SaveError::Limit {
            section: SECTION,
            kind,
            value: *reserved,
            max: limits.max_reserve_bytes,
        });
    }
    Ok(())
}

/// One cursor in the cross-region uniqueness merge (T21): the next
/// unconsumed identity of ONE region's table. `Ord` is by id first
/// — the merge extracts the minimum — with region and index only as
/// deterministic tie-breakers (a tie on id is exactly the duplicate
/// the merge exists to catch).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct UniquenessCursor {
    id: u32,
    region: u8,
    index: u32,
}

/// The cross-region uniqueness proof (T21): a bounded k-way merge
/// over the per-block identity tables. Every block's table MUST
/// already be strictly ascending (the wire-enforced canonical order;
/// `validate` checks it in its own loop) — the merge pops the global
/// minimum id repeatedly and refuses the first repeat with the typed
/// `SaveError::Duplicate` naming it. The caller supplies the heap
/// with capacity pre-reserved for one cursor per region (the merge
/// never holds more), so no merge-step allocation occurs: std pushes
/// allocate only at `len == capacity`.
fn assert_cross_region_uniqueness(
    regions: &[NpcRegionBlock],
    heap: &mut BinaryHeap<Reverse<UniquenessCursor>>,
) -> Result<(), SaveError> {
    const SECTION: &str = "npc-world-save-v2";
    for (region, block) in regions.iter().enumerate() {
        if let Some(first) = block.identities.first() {
            let region =
                u8::try_from(region).expect("the region index is bounded by MAX_REGION_COUNT");
            heap.push(Reverse(UniquenessCursor {
                id: first.id.0,
                region,
                index: 0,
            }));
        }
    }
    let mut prev: Option<u32> = None;
    while let Some(Reverse(cursor)) = heap.pop() {
        // Each block is ascending and the heap yields the global
        // minimum, so the merged sequence is nondecreasing and only
        // a repeat can violate it (the `<=` is defensive).
        if prev.is_some_and(|prior| cursor.id <= prior) {
            return Err(SaveError::Duplicate {
                section: SECTION,
                seq: u64::from(cursor.id),
            });
        }
        prev = Some(cursor.id);
        let block = &regions[usize::from(cursor.region)];
        let next = u64::from(cursor.index) + 1;
        if next < block.identities.len() as u64 {
            heap.push(Reverse(UniquenessCursor {
                id: block.identities[usize::try_from(next).expect("bounded by the block length")]
                    .id
                    .0,
                region: cursor.region,
                index: u32::try_from(next).expect("bounded by the block length"),
            }));
        }
    }
    Ok(())
}

/// A fallibly reserved column (T21 — fallible allocation wherever
/// practical): `try_reserve` makes a decode allocation failure a
/// typed [`SaveError::Overflow`] refusal rather than a process
/// abort. The v1 walk's columns are the fixed 1,000-identity
/// workload's (affordable by definition — the documented CA-02
/// decision that v1 needs no limits type); the v2 walk reserves the
/// same way after its budget gates.
fn reserved_column<T>(capacity: usize, section: &'static str) -> Result<Vec<T>, SaveError> {
    let mut column = Vec::new();
    column
        .try_reserve(capacity)
        .map_err(|_| SaveError::Overflow { section })?;
    Ok(column)
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
///
/// T21 note: for a COMPLETE section this stride gate is subsumed by
/// the whole-input minimum-size proof (see
/// [`NpcWorldSectionV2::restore_with_limits`]) — with every block
/// count bounded by the unclaimed population, each column's stride
/// is payable from the dormant minima of the identities still owed,
/// so no valid encoding is refused here. It is kept as defense in
/// depth, and it is LIVE for truncated inputs whose active blocks
/// sit above the dormant minimum (the whole-input gate cannot see a
/// section's actual shape, only its floor): the column-named
/// refusal below is what catches them, before the reservation.
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
    charge_reserve(mem_needed, "reserve-bytes", limits, reserved)
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

    /// THE WHOLE-INPUT MINIMUM-SIZE PROOF (T21): a declaration within
    /// every per-field bound (1 region, 8 identities, a full block
    /// header, 20 bytes of "records") is refused BEFORE any
    /// reservation — the section's own minimum (1 × 10 framing +
    /// 8 × 35 dormant-identity wire bytes = 290) exceeds the 30
    /// bytes actually remaining, a typed `Limit` naming both. This
    /// gate subsumes CA-02's per-column stride witness (the same
    /// input, 8 identities × 11-byte stride vs 20 bytes, refused
    /// pre-T21 at the identity column): the refusal now fires even
    /// earlier — before the block vector, not just before the
    /// columns. The stride gates remain in the walk as defense in
    /// depth, unchanged in kind.
    #[test]
    fn truncated_declarations_fail_at_the_whole_input_minimum_gate() {
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
        // The whole-input proof: 10 + 8 × 35 = 290 wire bytes are the
        // least a complete 1-region 8-identity section could occupy;
        // 30 remain (the block header plus the twenty bytes).
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(
                &mut Decoder::new(&bytes),
                &SectionDecodeLimits::WIRE_MAXIMUM
            ),
            Err(SaveError::Limit {
                kind: "section-minimum-bytes",
                value: 290,
                max: 30,
                ..
            })
        ));
        // A huge declaration over a large-but-finite remainder
        // refuses the same way, still before any reservation: 100
        // declared identities with 100 bytes remaining (the generic
        // one-byte-per-element count bound passes) against a
        // minimum of 10 + 100 × 35 = 3,510.
        let mut enc = Encoder::new();
        enc.write_u64(1337);
        enc.write_u64(6);
        enc.write_u64(4);
        enc.write_u64(12);
        enc.write_u8(1);
        enc.write_count(1); // region count
        enc.write_count(100); // npc count: within the u32 domain
        enc.write_u8(4); // region grid
        for _ in 0..100 {
            enc.write_u8(0xAA); // one hundred bytes actually present
        }
        let bytes = enc.into_bytes();
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(
                &mut Decoder::new(&bytes),
                &SectionDecodeLimits::WIRE_MAXIMUM
            ),
            Err(SaveError::Limit {
                kind: "section-minimum-bytes",
                value: 3_510,
                max: 100,
                ..
            })
        ));
        // And a truncation the minimum gate cannot see (an ACTIVE
        // section sits above its all-dormant minimum, so the
        // declaration's minimum fits while the input is 3 bytes
        // short of the section's actual shape) still refuses typed —
        // here at the stride gate, the defense-in-depth arm the
        // whole-input minimum cannot subsume: region 1's credits
        // column (2 identities x 8 wire bytes, 13 present) refuses
        // with the column-naming Limit before its reservation.
        let section = scaled_section(2, 4, 4, 1);
        let mut bytes = encode_v2(&section);
        bytes.truncate(bytes.len() - 3);
        assert!(matches!(
            NpcWorldSectionV2::restore(&mut Decoder::new(&bytes)),
            Err(SaveError::Limit {
                kind: "credits-count",
                ..
            })
        ));
    }

    /// THE TOTAL RESERVE-BYTES BUDGET (CA-02, re-derived under T21's
    /// total accounting): every transient reservation — the block
    /// vector, each column at its requested capacity, the uniqueness
    /// scratch, each at the allocation granularity — is checked
    /// against the caller's configured byte budget before it is
    /// requested. The pre-T21 test pinned the nominal column sum
    /// (48 identity bytes then 80 cumulative); the total ledger now
    /// refuses earlier and larger, and the exact admitting charge is
    /// [`NpcWorldSectionV2::decode_reservation_bytes`] — the
    /// off-by-one boundary, both directions.
    #[test]
    fn the_reserve_bytes_budget_bounds_every_decode_transient() {
        let section = scaled_section(3, 12, 4, 1);
        let bytes = encode_v2(&section);
        // A budget below the block vector's own charge refuses at the
        // FIRST reservation of the walk (the regions vector), the
        // ledger reading the granularity-rounded 3 × block bytes.
        let regions_charge = allocation_charge(3 * std::mem::size_of::<NpcRegionBlock>() as u64)
            .expect("the block-vector charge is representable");
        let limits = SectionDecodeLimits::new(12, 50);
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(&mut Decoder::new(&bytes), &limits),
            Err(SaveError::Limit {
                kind: "regions-reserve",
                value,
                max: 50,
                ..
            }) if value == regions_charge
        ));
        // A budget covering the block vector but not the first column
        // (region 0's four-identity identity table: 4 × 12 = 48
        // nominal, 64 charged at the granularity) refuses at the
        // column, the cumulative ledger naming both charges.
        let first_column_cumulative = regions_charge + allocation_charge(4 * 12).unwrap();
        let limits = SectionDecodeLimits::new(12, first_column_cumulative - 1);
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(&mut Decoder::new(&bytes), &limits),
            Err(SaveError::Limit {
                kind: "reserve-bytes",
                value,
                max,
                ..
            }) if value == first_column_cumulative && max == first_column_cumulative - 1
        ));
        // The exact charge admits the section's own encoding, and
        // one byte below refuses at the walk's last charge (the
        // uniqueness scratch): the mirror and the ledger are the
        // same arithmetic, pinned at the boundary.
        let admitting = section
            .decode_reservation_bytes()
            .expect("the charge is representable");
        let mut dec = Decoder::new(&bytes);
        let restored = NpcWorldSectionV2::restore_with_limits(
            &mut dec,
            &SectionDecodeLimits::new(12, admitting),
        )
        .expect("the exact charge admits the section");
        dec.finish().unwrap();
        assert_eq!(restored, section);
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(
                &mut Decoder::new(&bytes),
                &SectionDecodeLimits::new(12, admitting - 1)
            ),
            Err(SaveError::Limit {
                kind: "uniqueness-scratch",
                ..
            })
        ));
    }

    /// THE T21 WITNESS, POST-FIX (the pre-fix red run is recorded in
    /// the lane's first commit `adaab2e`: the same input decoded Ok
    /// at this budget while the block vector alone reserved
    /// 256 × `size_of::<NpcRegionBlock>()` bytes against it — the
    /// nominal column ledger admitted a decode whose transients
    /// exceeded the caller's configured limit ~5.6×). The total
    /// budget now charges the block vector first: the same input
    /// under the same 9,216-byte budget refuses typed at the
    /// regions-reserve gate BEFORE the reservation is made, and the
    /// exact charge ([`NpcWorldSectionV2::decode_reservation_bytes`])
    /// is the off-by-one admission boundary — the budget that admits
    /// precisely this section, one byte less refusing typed at the
    /// charge that crosses.
    #[test]
    fn small_configured_budgets_refuse_the_decode_transients_before_reserving() {
        let section = scaled_section(256, 256, 16, 0);
        let bytes = encode_v2(&section);
        // The pre-fix nominal column arithmetic: 256 dormant
        // identities × 36 in-memory bytes = 9,216 — the budget the
        // pre-fix ledger stopped at.
        let nominal_columns: u64 = 256 * 36;
        assert_eq!(nominal_columns, 9_216);
        let limits = SectionDecodeLimits::new(256, nominal_columns);
        // The refusal fires at the block vector's charge, before the
        // reservation: kind `regions-reserve`, the ledger's reading
        // (the granularity-rounded 256 × block bytes), the caller's
        // budget as the max.
        let expected = allocation_charge(256 * std::mem::size_of::<NpcRegionBlock>() as u64)
            .expect("the block-vector charge is representable");
        assert!(
            expected > nominal_columns,
            "the block-vector charge ({expected}) must exceed the pre-fix nominal budget"
        );
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(&mut Decoder::new(&bytes), &limits),
            Err(SaveError::Limit {
                kind: "regions-reserve",
                value,
                max,
                ..
            }) if value == expected && max == nominal_columns
        ));
        // The off-by-one admission boundary: the exact charge admits
        // the section's own encoding...
        let admitting = section
            .decode_reservation_bytes()
            .expect("the charge is representable");
        let mut dec = Decoder::new(&bytes);
        let restored = NpcWorldSectionV2::restore_with_limits(
            &mut dec,
            &SectionDecodeLimits::new(256, admitting),
        )
        .expect("the exact charge admits the section");
        dec.finish().unwrap();
        assert_eq!(restored, section);
        // The transient is now BUDGETED, not banned: the block
        // vector still reserves its 256 slots under the admitting
        // budget, exactly as the charge said it would.
        assert_eq!(restored.regions.capacity(), 256);
        // ...and one byte below refuses, typed, at the charge that
        // crosses — the uniqueness scratch is the walk's last charge
        // for this shape, so that is the refusal that fires.
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(
                &mut Decoder::new(&bytes),
                &SectionDecodeLimits::new(256, admitting - 1)
            ),
            Err(SaveError::Limit {
                kind: "uniqueness-scratch",
                max,
                ..
            }) if max == admitting - 1
        ));
    }

    /// THE CONFIG-DERIVED ADMISSION BUDGET (R5 — the production-entry
    /// boundary's config-class lever): `SectionDecodeLimits::for_config`
    /// derives a FINITE budget from the composition's own authored
    /// configuration, the budget a boundary restoring less-trusted
    /// bytes configures instead of the wire maximum. The
    /// never-refuses property on every shape a configuration can
    /// carry: the concentrated split (every identity in one block —
    /// the nominal maximum), the spread split (one identity per
    /// block — the per-block rounding maximum), the round-robin
    /// fixture partition, mixed dormant/active, all-dormant, the
    /// minimal world, and the widest admitted partition — each
    /// section's own bytes restore under its own configuration's
    /// budget, and the exact charge
    /// (`decode_reservation_bytes`) is at or below the budget (the
    /// gates never reject a valid encoding; the bound covers both
    /// split extremes).
    #[test]
    fn the_config_derived_budget_admits_every_section_its_config_carries() {
        let base = scaled_section(1, 1, 16, 1);
        let shaped = |splits: Vec<Vec<u32>>, grid: u8, dormant_from: usize| {
            let regions = splits
                .iter()
                .enumerate()
                .map(|(region, ids)| {
                    region_block(
                        region as u8,
                        grid,
                        ids,
                        if region >= dormant_from {
                            LodLevel::Dormant
                        } else {
                            LodLevel::Full
                        },
                        region < 2,
                    )
                })
                .collect::<Vec<_>>();
            let npc_count: u32 = splits.iter().map(|ids| ids.len() as u32).sum();
            NpcWorldSectionV2 {
                regions,
                config: WorldConfig {
                    region_count: splits.len() as u32,
                    npc_count,
                    region_grid: grid,
                },
                ..base.clone()
            }
        };
        // The concentrated split: all nine identities in region 0,
        // two empty blocks after.
        let concentrated = shaped(vec![vec![0, 1, 2, 3, 4, 5, 6, 7, 8], vec![], vec![]], 16, 9);
        // The spread split: one identity per block across five
        // regions (four identities ride the last block).
        let spread = shaped(
            vec![vec![0], vec![1], vec![2], vec![3], vec![4, 5, 6, 7]],
            16,
            3,
        );
        let mut battery: Vec<NpcWorldSectionV2> = vec![concentrated, spread];
        // The fixture scales: the corner's configured values (both
        // shapes), the memory-lane scale, the 10k fixture, the
        // minimal world, the widest partition.
        for (regions, npcs, grid, active) in [
            (8, 24, 16, 8),
            (8, 1000, 16, 8),
            (8, 1000, 16, 3),
            (64, 10_000, 16, 4),
            (1, 1, 1, 1),
            (256, 300, 255, 0),
        ] {
            battery.push(scaled_section(regions, npcs, grid, active));
        }
        for section in battery {
            section
                .validate()
                .expect("the battery section is consistent");
            let limits = SectionDecodeLimits::for_config(section.config)
                .expect("the battery configuration is in the domain");
            assert!(
                limits.max_identities == u64::from(section.config.npc_count),
                "the identity budget is exactly the authored population"
            );
            let exact = section
                .decode_reservation_bytes()
                .expect("the charge is representable");
            assert!(
                exact <= limits.max_reserve_bytes,
                "the exact charge ({exact}) must fit the config-derived budget ({})",
                limits.max_reserve_bytes
            );
            let bytes = encode_v2(&section);
            let mut dec = Decoder::new(&bytes);
            let restored = NpcWorldSectionV2::restore_with_limits(&mut dec, &limits)
                .expect("the config-derived budget never refuses the config's own section");
            dec.finish().unwrap();
            assert_eq!(restored, section);
            assert_eq!(encode_v2(&restored), bytes);
        }
    }

    /// THE CONFIG-DERIVED ADMISSION BUDGET, refusal half (R5): a
    /// section declaring MORE than the authored workload refuses
    /// TYPED, before the reservation it refuses — more identities at
    /// the aggregate identity gate (the over-budget workload
    /// refusal), a wider partition at the reserve ledger's FIRST
    /// charge (the block vector). The wire maximum keeps today's
    /// acceptance of the same bytes: the limits change admission,
    /// never the walk.
    #[test]
    fn the_config_derived_budget_refuses_declarations_beyond_the_authored_workload() {
        // The identity axis: a 3-region/12-identity section under a
        // budget authored for 11 identities — the aggregate gate
        // fires before the whole-input proof or any reservation.
        let section = scaled_section(3, 12, 4, 1);
        let bytes = encode_v2(&section);
        let limits = SectionDecodeLimits::for_config(WorldConfig {
            region_count: 3,
            npc_count: 11,
            region_grid: 4,
        })
        .expect("the authored configuration is in the domain");
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(&mut Decoder::new(&bytes), &limits),
            Err(SaveError::Limit {
                kind: "npc-count",
                value: 12,
                max: 11,
                ..
            })
        ));
        // The partition axis: a 256-region section under a budget
        // authored for one region — the block vector's charge alone
        // (256 x size_of::<NpcRegionBlock>(), granularity-rounded)
        // exceeds the whole derived budget, so the refusal fires at
        // the walk's FIRST charge, before the vector reserves.
        let wide = scaled_section(256, 1, 16, 0);
        let wide_bytes = encode_v2(&wide);
        let narrow = SectionDecodeLimits::for_config(WorldConfig {
            region_count: 1,
            npc_count: 1,
            region_grid: 16,
        })
        .expect("the authored configuration is in the domain");
        let block_vector = allocation_charge(256 * std::mem::size_of::<NpcRegionBlock>() as u64)
            .expect("the block-vector charge is representable");
        assert!(
            block_vector > narrow.max_reserve_bytes,
            "the wide partition's block vector ({block_vector}) must exceed the narrow budget ({})",
            narrow.max_reserve_bytes
        );
        assert!(matches!(
            NpcWorldSectionV2::restore_with_limits(&mut Decoder::new(&wide_bytes), &narrow),
            Err(SaveError::Limit {
                kind: "regions-reserve",
                value,
                max,
                ..
            }) if value == block_vector && max == narrow.max_reserve_bytes
        ));
        // The wire maximum keeps today's acceptance of both inputs
        // exactly (the compatibility limit, distinct from the
        // affordability contract).
        let mut dec = Decoder::new(&bytes);
        assert!(
            NpcWorldSectionV2::restore_with_limits(&mut dec, &SectionDecodeLimits::WIRE_MAXIMUM)
                .is_ok()
        );
        let mut dec = Decoder::new(&wide_bytes);
        assert!(
            NpcWorldSectionV2::restore_with_limits(&mut dec, &SectionDecodeLimits::WIRE_MAXIMUM)
                .is_ok()
        );
    }

    /// The budget derivation is fail-closed on the configuration's
    /// own domain (R5): a zero partition, a partition beyond the
    /// identity codec's region field, a zero population, or a zero
    /// grid side refuses at the derivation — never a repaired or
    /// defaulted budget.
    #[test]
    fn an_invalid_configuration_is_refused_at_the_budget_derivation() {
        let base = WorldConfig {
            region_count: 3,
            npc_count: 9,
            region_grid: 16,
        };
        for invalid in [
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
                matches!(
                    SectionDecodeLimits::for_config(invalid),
                    Err(SaveError::Integrity("section-decode-limits/world-config"))
                ),
                "the configuration {invalid:?} must refuse at the derivation"
            );
        }
    }

    /// THE CROSS-REGION UNIQUENESS MERGE (T21): the per-ID
    /// `BTreeSet` node stream is gone; global uniqueness is a
    /// bounded k-way merge over the per-block ascending tables. The
    /// battery: valid interleaved ids across regions restore (no
    /// FALSE duplicate), an adversarial duplicate in NON-adjacent
    /// regions with smaller ids merged first is refused naming the
    /// id, a three-way duplicate refuses on the first repeat, a
    /// duplicate spanning the first and last of many regions refuses
    /// (the merge's long-range case), and the wire gate and the
    /// in-memory `validate` gate agree on every case (the F06
    /// discipline extended to the merge).
    #[test]
    fn cross_region_uniqueness_is_the_bounded_merge_on_every_shape() {
        let base = scaled_section(1, 1, 16, 1);
        let section_with = |regions: Vec<NpcRegionBlock>, npc_count: u32| NpcWorldSectionV2 {
            regions,
            config: WorldConfig {
                region_count: 3,
                npc_count,
                region_grid: 16,
            },
            ..base.clone()
        };
        // Valid interleaved ids across three regions (ids 10..70
        // spread so no two adjacent merge outputs share a block):
        // restores, validates, and re-encodes byte-identically.
        let interleaved = section_with(
            vec![
                region_block(0, 16, &[10, 30, 50], LodLevel::Full, true),
                region_block(1, 16, &[20, 40, 60], LodLevel::Full, false),
                region_block(2, 16, &[15, 25, 70], LodLevel::Dormant, false),
            ],
            9,
        );
        interleaved.validate().expect("interleaved ids are unique");
        let bytes = encode_v2(&interleaved);
        let mut dec = Decoder::new(&bytes);
        let restored =
            NpcWorldSectionV2::restore_with_limits(&mut dec, &SectionDecodeLimits::WIRE_MAXIMUM)
                .expect("no false duplicate from interleaved ids");
        dec.finish().unwrap();
        assert_eq!(restored, interleaved);
        assert_eq!(encode_v2(&restored), bytes);
        // The adversarial shapes: a duplicate id in NON-adjacent
        // regions with smaller ids merged before it (50 rides
        // regions 0 and 2, ids 20/25/30/40 merged first).
        let duplicate = section_with(
            vec![
                region_block(0, 16, &[10, 50], LodLevel::Full, true),
                region_block(1, 16, &[20, 60], LodLevel::Full, false),
                region_block(2, 16, &[50, 70], LodLevel::Dormant, false),
            ],
            6,
        );
        // Both gates refuse identically (the F06 discipline
        // extended to the merge): the wire walk and the in-memory
        // validator name the same duplicated id.
        let wire_refusal = NpcWorldSectionV2::restore(&mut Decoder::new(&encode_v2(&duplicate)));
        assert!(
            matches!(wire_refusal, Err(SaveError::Duplicate { seq: 50, .. })),
            "restore names the duplicated id"
        );
        assert!(
            matches!(
                duplicate.validate(),
                Err(SaveError::Duplicate { seq: 50, .. })
            ),
            "validate names the duplicated id"
        );
        // A three-way duplicate (50 in every region): the first
        // repeat refuses, naming the id.
        let three_way = section_with(
            vec![
                region_block(0, 16, &[50], LodLevel::Full, true),
                region_block(1, 16, &[50], LodLevel::Full, false),
                region_block(2, 16, &[50], LodLevel::Dormant, false),
            ],
            3,
        );
        assert!(matches!(
            NpcWorldSectionV2::restore(&mut Decoder::new(&encode_v2(&three_way))),
            Err(SaveError::Duplicate { seq: 50, .. })
        ));
        assert!(matches!(
            three_way.validate(),
            Err(SaveError::Duplicate { .. })
        ));
        // The long-range case: a duplicate spanning the first and
        // last of eight regions, six blocks of smaller ids between.
        let mut regions = Vec::new();
        regions.push(region_block(0, 16, &[100], LodLevel::Full, true));
        for region in 1u32..7 {
            regions.push(region_block(
                region as u8,
                16,
                &[region],
                LodLevel::Dormant,
                false,
            ));
        }
        regions.push(region_block(7, 16, &[100], LodLevel::Dormant, false));
        let long_range = NpcWorldSectionV2 {
            regions,
            config: WorldConfig {
                region_count: 8,
                npc_count: 8,
                region_grid: 16,
            },
            ..base.clone()
        };
        assert!(matches!(
            NpcWorldSectionV2::restore(&mut Decoder::new(&encode_v2(&long_range))),
            Err(SaveError::Duplicate { seq: 100, .. })
        ));
        assert!(matches!(
            long_range.validate(),
            Err(SaveError::Duplicate { .. })
        ));
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
