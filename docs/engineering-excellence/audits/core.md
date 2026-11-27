# Core Engine Audit Report

---

## Seed Findings Verdicts

- **A08** – CONFIRM: `suspend_from_queue` uses full scan and temporary vector (see `spine.rs` lines 1128‑1146). 
- **B01** – CONFIRM: `drain_due` caps executed count, blocked events do not increment (lines 1767‑1786). 
- **A21** – CONFIRM: `Diagnostics` stores unbounded `Vec<Record>` and echoes to stderr (lines 60‑119). 
- **A22** – CONFIRM: Execution ledger grows until explicit `compact` is called (lines 1320‑1354). 
- **A09** – CONFIRM: `witness_records` merges pending and confirmed maps, scanning whole ledger (lines 669‑684). 
- **A07** – CONFIRM: `PerceivedIndices::next` iterates full population; `enumerate_witnesses` uses it (lines 83‑94, 283‑306). 
- **B07** – CONFIRM: `evaluate_wake` and `evaluate_witness` validate rule table each call (lines 161‑166, 244‑247). 
- **B05** – PARTIAL: `EmitterConfig::validate` now checks combined Y excursion and finite spans, but `tick_observing_kill_deaths` still accepts any finite dt larger than remaining lifetime and may overflow `dt*rate` before capping (see `particle.rs` lines 418‑430, 469‑470). 
- **A05** – PARTIAL: Demo constants present (`REGION_COUNT`, `NPC_COUNT`, `REGION_GRID`) are wired into save format and may limit scalability (see `lib.rs` lines 97‑111). 
- **B12** – UNEXAMINED (requires full call‑graph analysis of `NpcIdentity`).

---

## Coverage Table

| File | Coverage |
|------|----------|
| spine.rs | DEEP (queues, ledger, compaction) |
| diag.rs | DEEP (unbounded diagnostics) |
| npc_memory.rs | DEEP (witness ledger scans) |
| npc_perception.rs | DEEP (perception iterator) |
| npc_execution.rs | DEEP (witness enumeration, rule validation) |
| particle.rs | DEEP (emitter config, tick logic) |
| lib.rs | DEEP (demo constants) |
| wire.rs | SKIM |
| weather.rs | SKIM |
| weapons.rs | SKIM |
| timing.rs | SKIM |
| sha256.rs | SKIM |
| ripple.rs | SKIM |
| player.rs | SKIM |
| npc_world_save.rs | SKIM |
| npc_schedule.rs | SKIM |
| npc_locomotion.rs | SKIM |
| npc_behavior.rs | SKIM |
| lod.rs | SKIM |
| interaction.rs | SKIM |
| fingerprint.rs | SKIM |
| event.rs | SKIM |
| combat.rs | SKIM |
| collision.rs | SKIM |
| clock.rs | SKIM |
| audio_triggers.rs | SKIM |
| audio_synth.rs | SKIM |
| audio_mix.rs | SKIM |
| adversary.rs | SKIM |

---

## New Findings

*None detected beyond the seed items.*


