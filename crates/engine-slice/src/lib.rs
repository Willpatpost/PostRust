//! Interactive-slice application library (Stage 0).
//!
//! The binary is a thin stdin loop; all logic lives here so tests can drive
//! the same code path without spawning a process.

// (D-EE-10 item 4) The PP-B1 backup tool captures and verifies the
// SQLITE campaign database — the module exists only under
// engine-slice's `sqlite` feature (DEFAULT ON; a
// --no-default-features build is local-only and carries no backup
// surface — a database campaign cannot exist in such a build).
#[cfg(feature = "sqlite")]
pub mod backup;
pub mod behavior_measurement;
pub mod benchmark;
pub mod census;
pub mod combat_adoption;
pub mod combat_demo;
pub mod corner_behavior;
pub mod corner_demo;
pub mod corner_population;
pub mod corner_routes;
pub mod corner_scene;
pub mod corner_timing;
pub mod corner_weather;
pub mod crossing;
pub mod encounter_benchmark;
pub mod encounter_demo;
pub mod encounter_events;
pub mod encounter_frame;
mod encounter_hud;
pub mod encounter_visuals;
pub mod frame_timing;
pub mod garage_demo;
pub mod garage_scene;
pub mod math;
pub mod npc;
pub mod npc_memory_lane;
pub mod orchestration;
pub mod perf_sweep;
pub mod persist_demo;
pub mod playable_demo;
mod player_controls;
pub mod player_demo;
pub mod region;
pub mod region_render;
pub mod render_benchmark;
pub mod render_demo;
pub mod ripple_visibility;
pub mod save_demo;
pub mod session;
pub mod session_chunks;
pub mod sim;
pub mod sim_demo;
pub mod soak_evidence;
pub mod soak_scenario;
pub mod store;
pub mod sustained_load;
pub mod window_demo;
pub mod worker;
pub mod world;
