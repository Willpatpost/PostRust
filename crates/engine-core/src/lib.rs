//! Stage 0 reusable engine core for the Sektor7 original engine.
//!
//! Scope (`docs/TECH-ARCHITECTURE.md` §6, Stage 0 — lead authorization
//! 2026-10-19): the canonical clock seed, the event spine carrying the three
//! contract semantics the architecture review required, deterministic
//! logical-state fingerprints, and basic diagnostics.
//!
//! Implemented contract semantics (`docs/TECH-ARCHITECTURE.md` §3.2, v0.2):
//!
//! * **Safe watermark** — [`Spine::processed_through`] is distinct from
//!   [`Spine::max_executed_due`] and never claims completeness while earlier
//!   required events remain queued, executing, or blocked by unresolved
//!   causal dependencies; it returns `None` when nothing is provably
//!   complete (including a pending event at tick 0). It is computed live,
//!   may retreat when late events are scheduled, and is a progress measure
//!   — not irreversible finality.
//! * **Causal dependencies and ordering** — within a class, `(due, seq)`
//!   defines deterministic queue priority (minimum key among currently
//!   queued, due events) — not a globally monotonic dequeue or execution
//!   guarantee; the ledger records deviations without identifying their
//!   causes (see [`EventClass`]). Across classes, ordering exists only
//!   through explicit `depends_on` links. Dependency ids must reference
//!   already-scheduled or already-executed events (unknown ids are rejected
//!   at schedule time), which makes dependency cycles structurally
//!   impossible.
//! * **Event vs execution timestamps** — [`Schedule::AtLogicalTime`] fixes
//!   the due tick at schedule time; [`Schedule::ExecutionRelative`] anchors
//!   a follow-up to the tick at which its scheduling event actually
//!   executed. Lateness (`executed_tick > intended_due`) is first-class
//!   ledger data.
//!
//! Deliberately absent (anti-abstraction ledger, §8): regions, LOD levels,
//! persistence, gating, networking, rendering, threading, and any
//! subsystem not exercised by this crate's own tests or the `engine-slice`
//! application. (Near/far lanes arrived with Stage 3 — §2.4's three-phase
//! budget policy — while regions and LOD levels remain gated.)
//!
//! Determinism rules: fingerprints use FNV-1a (stable across platforms and
//! compiler versions — `std`'s hashers are not); fingerprinted state never
//! derives from unordered iteration; wall-clock time never enters logical
//! state.
//!
//! Cycle 1 P3 (2026-11-14) adds the player movement/collision prototype —
//! kinematic, std-only mechanics (integration, collision queries, camera
//! modes, a fixed-step driver), exercised by its own tests and the
//! engine-slice demo application.

pub mod adversary;
pub mod audio_mix;
pub mod audio_synth;
pub mod audio_triggers;
pub mod clock;
pub mod collision;
pub mod combat;
pub mod diag;
pub mod event;
pub mod fingerprint;
pub mod interaction;
pub mod lod;
pub mod npc_behavior;
pub mod npc_execution;
pub mod npc_locomotion;
pub mod npc_memory;
pub mod npc_perception;
pub mod npc_schedule;
pub mod npc_world_save;
pub mod particle;
pub mod player;
pub mod ripple;
pub mod spine;
pub mod timing;
pub mod weapons;
pub mod weather;
pub mod wire;
pub mod world_config;

mod sha256;

pub use clock::Tick;
pub use collision::{RotatedCollider, segment_rotated_hit};
pub use combat::{
    AttackProfile, AttackRefusal, AttackRequest, COMBAT_CLASS, COMBAT_FINGERPRINT_DOMAIN,
    COMBAT_MAGIC, COMBAT_PAYLOAD_ID_BOUND, CombatEventKind, Combatant, CombatantId, DamageOutcome,
    HealOutcome, Health, HealthError, HitResolution, MissKind, ProfileField, combat_payload,
    combat_payload_parts, health_full, resolve_attack, segment_box_hit,
};
pub use diag::{Diagnostics, Level, Record};
pub use event::{
    Event, EventClass, EventId, EventSpec, ExecutionRecord, Field, Schedule, ScheduleKind,
};
pub use fingerprint::Fingerprinter;
pub use interaction::{InteractionQuery, InteractionTarget, select_target};
pub use player::{
    CAMERA_BACKOFF, CAMERA_MARGIN, CameraMode, CameraPose, CollisionWorld, DriveStep, MoveIntent,
    PlayerConfig, PlayerDriver, PlayerState, StaticCollider, StepOutcome, TraceEntry, camera_pose,
    camera_pose_obstructed, look_direction, player_box, step_player,
};
/// The v1 wire formats' fixed region layout: eight logical regions.
/// Authoritative in engine-core as **wire-format data** (P2 L8,
/// A05's configuration boundary): a v1 save
/// (`engine-slice/npc-world-save/1`) and the v1 region-director
/// section encode eight regions BY DEFINITION, and the persistence
/// layer's shared session-snapshot anchor decoding validates against
/// this one constant. This is NOT world policy — the world's live
/// partition is configuration ([`WorldConfig`], carried as data by
/// the `npc-world-save/2` section); the application authors its own
/// values (engine-slice's `npc` module), equal to this
/// interpretation so every historical save stays byte-compatible.
pub const REGION_COUNT: usize = 8;

/// The v1 wire formats' fixed workload: 1,000 stable NPC identities
/// (the shared wire-format bound — see [`REGION_COUNT`]). Wire-format
/// data, not world policy: a v1 save materializes exactly this
/// population; a `npc-world-save/2` save carries its own
/// ([`WorldConfig::npc_count`]).
pub const NPC_COUNT: usize = 1000;

/// The v1 wire formats' fixed side length of the per-NPC region-local
/// position grid (the shared wire-format bound — see
/// [`REGION_COUNT`]). Wire-format data, not world policy: the v2
/// section carries the grid side as data
/// ([`WorldConfig::region_grid`]).
pub const REGION_GRID: u8 = 16;

/// The discarded-position sentinel (the shared wire-format value —
/// see [`REGION_COUNT`]). Format data in both versions: the one
/// off-grid exemption every position-bounds gate sanctions.
pub const POSITION_DISCARDED: (u8, u8) = (u8::MAX, u8::MAX);

pub use lod::{LodConfig, LodConfigError, LodController, LodLevel, PresenceSample};
pub use npc_world_save::{
    NPC_WORLD_SAVE_DOMAIN, NPC_WORLD_SAVE_DOMAIN_V2, NpcRegionBlock, NpcSaveWorld, NpcSaveWorldV2,
    NpcWorldSection, NpcWorldSectionV2,
};
pub use spine::{
    CompactionError, ExecCtx, FollowUpAdmission, HistoryKind, HousekeepingAdmission, LaneReport,
    LaneTotals, ObligationLocation, ObligationView, ReconstructionError, ScheduleError, Spine,
    Stats, SuspensionStats, World,
};
pub use weapons::{
    Arsenal, FireContext, FireOutcome, FireRefusal, FiringPattern, MAX_SPREAD_RAYS, Shot,
    SpreadField, WEAPON_COUNT, WeaponId, WeaponSet, WeaponSpec, try_fire,
};
pub use wire::{Decoder, Encoder, SaveError, SaveWorld};
pub use world_config::{MAX_NPC_COUNT, MAX_REGION_COUNT, WorldConfig, WorldConfigError};
