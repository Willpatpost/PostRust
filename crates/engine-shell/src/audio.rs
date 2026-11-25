//! Cycle 3 P2 — the event-driven audio capability (owner GO
//! 2026-11-17, §2 P2; Stage 1's audio exclusion is superseded for this
//! module by that authorization).
//!
//! A small, reusable, ONE-SHOT CUE surface over SDL3's callback-free
//! audio-stream model. Design consequences of the charter's
//! performance-first standards:
//!
//! - **Event-driven, never polled:** there is no per-frame API at all.
//!   Cues are pushed as complete prebuilt waveforms through
//!   `SDL_PutAudioStreamData` — a non-blocking buffered copy ("this
//!   call simply copies the unconverted data for later", per the SDL3
//!   documentation) executed by SDL's own audio thread. When idle,
//!   nothing in the engine calls into audio: the bound streams simply
//!   run dry.
//! - **No callback, no runtime synthesis, no per-event allocation:**
//!   the cue waveforms are procedurally generated ONCE at
//!   initialization (pure functions of the sample index — original
//!   placeholder audio, no external assets, no licensing surface);
//!   each playback is one SDL call plus SDL's internal copy.
//! - **One stream per cue, mixed by SDL:** each cue owns a stream
//!   bound to the shared device; SDL mixes concurrently-bound streams,
//!   so distinct cues overlap naturally.
//! - **Audio is a presentation consumer, never an authority:** the
//!   deterministic headless paths never construct this type at all;
//!   interactive paths route gameplay events through [`Audio::play`]
//!   only. No simulation outcome can depend on audio.
//! - **Graceful, observable degradation:** a missing audio device (or
//!   any open failure) degrades to the [`AudioStatus::Silent`] state
//!   with the reason queryable through [`Audio::status`] — never an
//!   error; a mid-run playback failure degrades the same way. The
//!   explicit [`Audio::disabled`] headless mode never touches SDL
//!   audio at all.
//!
//! # Era 2 package B part 2 — the SDL3 AUDIO DEVICE LAYER
//! (the vertical slice's §2.1 device I/O, under the map's law)
//!
//! [`AudioDevice`] (below the cue surface) is the deterministic audio
//! cores' device boundary: the trigger projection's output
//! (`engine_core::audio_triggers::SoundTrigger`) becomes audible
//! through an owned mixer running on SDL3's audio thread. The design
//! follows `docs/AUDIO-SUBSYSTEM-MAP.md` exactly:
//!
//! - **Stream get-callback, not push:** the device opens through
//!   SDL3's `SDL_OpenAudioDeviceStream` callback model (the sdl3
//!   crate's `AudioSubsystem::open_playback_stream` +
//!   [`sdl3::audio::AudioCallback`] safe wrapper — the workspace
//!   denies `unsafe_code`, and this layer owns none). SDL calls the
//!   mixer on ITS audio thread with the exact amount it needs; the
//!   application never pushes or polls. The app-side stream format is
//!   48 kHz stereo S16 — the documented constants matching
//!   `audio_synth`'s pinned i16 output and `audio_mix::to_i16`'s
//!   boundary (SDL converts to whatever the hardware provides).
//! - **The audio-thread allocation law (§1.2):** the callback mixes
//!   into PRE-ALLOCATED buffers only — the fixed voice pool (the
//!   trigger projection's `TRIGGER_POOL_BOUND`), the f32 mix
//!   scratch, the i16 output scratch, and the resident bed PCM are
//!   all built at construction (asset time, where allocation is
//!   legal). The callback allocates nothing, waits on no lock (the
//!   command cell is drained with `try_lock` — a failed attempt
//!   defers the drain by one device period; it never blocks), and
//!   does bounded work (at most the fixed ring's worth of voice
//!   starts plus a fixed chunk of mixing per invocation).
//! - **Fail-visible counters (§4):** underruns (a refused put — SDL
//!   plays silence for the shortfall), worst callback cost in
//!   nanoseconds, the live voice count, voice steals, and dropped
//!   triggers are Relaxed atomics — the truncation-counter pattern;
//!   [`AudioStats`] reads them.
//! - **Lazy + typed degradation (§1.5, §4.6):** [`AudioDevice::try_new`]
//!   runs when the application first asks for audio — never at
//!   startup — and a machine with no playback device gets
//!   [`AudioError::NoDevice`]: typed silence, no crash, no frame-path
//!   impact (the shell's dummy-driver precedent).

use std::fmt;
use std::mem::size_of;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use engine_core::audio_mix::{attenuation, mix_into, pan_from_listener, pan_gains, to_i16};
use engine_core::audio_synth::{synthesize_ambience_bed, synthesize_footstep};
use engine_core::audio_triggers::{SoundClass, SoundEmitter, SoundTrigger, TRIGGER_POOL_BOUND};
use engine_core::lod::LodLevel;

use crate::platform::Platform;
use sdl3::audio::{
    AudioCallback, AudioDevice as SdlAudioDevice, AudioFormat, AudioSpec, AudioStream,
    AudioStreamOwner, AudioStreamWithCallback,
};

/// The gameplay cue surface — the narrow event→sound mapping the
/// application layer wires (firing, impact, damage, interaction,
/// encounter completion). The placeholder synthesis behind each cue
/// is original and procedural; final soundtrack, voice, and spatial
/// audio are out of scope for this milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cue {
    /// A weapon firing.
    Fire,
    /// A projectile or body impact.
    Impact,
    /// Damage applied to a combatant.
    Damage,
    /// An interaction confirmed (switch, pickup).
    Interact,
    /// An encounter completed.
    EncounterComplete,
    /// A weapon selection change (the Cycle-4 F proposal, approved by
    /// the orchestrator 2026-11-18: a short mechanical click — the
    /// selection is a control event, not an interaction-confirmed).
    WeaponSwitch,
    /// A single adversary defeated — the partial-victory beat (the
    /// Cycle-4 F proposal, approved: a brief descending sting,
    /// distinct from the encounter's completion).
    AdversaryDefeated,
    /// The player defeated — the encounter's failure terminal (the
    /// Cycle-4 F proposal, approved: a longer descending resolution,
    /// the failure counterpart of `EncounterComplete`'s rising
    /// arpeggio).
    Defeat,
}

/// Every cue, in the stable index order used internally.
pub const CUES: [Cue; 8] = [
    Cue::Fire,
    Cue::Impact,
    Cue::Damage,
    Cue::Interact,
    Cue::EncounterComplete,
    Cue::WeaponSwitch,
    Cue::AdversaryDefeated,
    Cue::Defeat,
];

impl Cue {
    fn index(self) -> usize {
        match self {
            Cue::Fire => 0,
            Cue::Impact => 1,
            Cue::Damage => 2,
            Cue::Interact => 3,
            Cue::EncounterComplete => 4,
            Cue::WeaponSwitch => 5,
            Cue::AdversaryDefeated => 6,
            Cue::Defeat => 7,
        }
    }
}

/// The observable audio state — the run summary prints this so an
/// unavailable device is VISIBLE, not silent-by-accident.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioStatus {
    /// A playback device is open; cues play through it.
    Active { driver: String, device: String },
    /// No audio: the reason is always carried (headless mode, no
    /// device, an open failure, or a mid-run playback failure).
    Silent { reason: String },
}

impl AudioStatus {
    /// True when cues play.
    pub fn is_active(&self) -> bool {
        matches!(self, AudioStatus::Active { .. })
    }
}

/// The audio capability: a default playback device plus one bound
/// stream per cue, carrying prebuilt procedural waveforms. Drop
/// follows Rust's declaration-order field drops: the device closes
/// first — `SDL_CloseAudioDevice` unbinds the still-bound streams
/// without destroying them (SDL 3.4.18, `DestroyLogicalAudioDevice`)
/// — then each stream destroys itself, and the refcounted SDL audio
/// subsystem quits only after the last stream's refcount clone drops
/// (every stream owner holds one). The reverse order is equally
/// SDL-safe: the mid-run failure path in `play` releases the streams
/// before the device.
pub struct Audio {
    status: AudioStatus,
    /// The open device — Rust drops fields in declaration order, so
    /// this closes FIRST; `SDL_CloseAudioDevice` unbinds the
    /// still-bound streams without destroying them, so the later
    /// stream drops operate on already-unbound streams.
    device: Option<SdlAudioDevice>,
    /// One stream per cue, bound to the device (SDL mixes them).
    streams: Option<Vec<AudioStreamOwner>>,
    /// The prebuilt cue waveforms (48 kHz mono f32), indexed by cue.
    samples: Vec<Vec<f32>>,
}

/// The cue sample rate (Hz).
const SAMPLE_RATE: i32 = 48_000;

impl Audio {
    /// The explicit silent/headless mode: no SDL audio is touched at
    /// all (no subsystem init, no device, no threads), `play` is a
    /// no-op, and the carried reason is observable in the status.
    pub fn disabled(reason: &str) -> Audio {
        Audio {
            status: AudioStatus::Silent {
                reason: reason.to_string(),
            },
            device: None,
            streams: None,
            samples: Vec::new(),
        }
    }

    /// Initializes the default playback device and the per-cue
    /// streams, synthesizing the cue waveforms once. ANY failure —
    /// the audio subsystem, the device open, the stream creation or
    /// binding — degrades to the observable silent state with the
    /// reason: audio is never a gameplay prerequisite.
    pub fn initialize(platform: &Platform) -> Audio {
        let subsystem = match platform.sdl().audio() {
            Ok(subsystem) => subsystem,
            Err(error) => {
                return Audio::disabled(&format!("the audio subsystem: {error}"));
            }
        };
        // The device spec is a REQUEST; SDL3 converts every bound
        // stream into whatever the device actually provides.
        let device_spec = AudioSpec {
            freq: Some(SAMPLE_RATE),
            channels: Some(2),
            format: Some(AudioFormat::F32LE),
        };
        let device = match subsystem.open_playback_device(&device_spec) {
            Ok(device) => device,
            Err(error) => {
                return Audio::disabled(&format!("no audio playback device: {error}"));
            }
        };
        // The application-side cue format: mono f32 at the synthesis
        // rate; SDL's stream converts (and upmixes) to the device.
        let cue_spec = AudioSpec {
            freq: Some(SAMPLE_RATE),
            channels: Some(1),
            format: Some(AudioFormat::F32LE),
        };
        let mut streams = Vec::with_capacity(CUES.len());
        for _ in CUES {
            match subsystem.new_playback_stream(&cue_spec, None) {
                Ok(stream) => streams.push(stream),
                Err(error) => {
                    return Audio::disabled(&format!("the cue stream: {error}"));
                }
            }
        }
        let bound: Vec<&sdl3::audio::AudioStream> =
            streams.iter().map(|stream| &**stream).collect();
        if let Err(error) = device.bind_streams(&bound) {
            return Audio::disabled(&format!("binding the cue streams: {error}"));
        }
        let samples: Vec<Vec<f32>> = CUES.iter().map(|cue| cue_samples(*cue)).collect();
        let driver = subsystem.current_audio_driver().to_string();
        let device_name = device
            .name()
            .map(|name| name.to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        Audio {
            status: AudioStatus::Active {
                driver,
                device: device_name,
            },
            device: Some(device),
            streams: Some(streams),
            samples,
        }
    }

    /// The observable status (print it in the run summary).
    pub fn status(&self) -> &AudioStatus {
        &self.status
    }

    /// Plays one cue: a single non-blocking push of the cue's
    /// prebuilt waveform into its stream (SDL's audio thread consumes
    /// it; no allocation, no synthesis, no polling). A no-op in the
    /// silent state; a mid-run playback failure degrades to the
    /// observable silent state and releases the audio resources.
    pub fn play(&mut self, cue: Cue) {
        let Some(streams) = self.streams.as_mut() else {
            return;
        };
        let index = cue.index();
        if let Err(error) = streams[index].put_data_f32(&self.samples[index]) {
            self.status = AudioStatus::Silent {
                reason: format!("playback failed mid-run: {error}"),
            };
            self.streams = None;
            self.device = None;
            self.samples.clear();
        }
    }
}

/// Deterministic per-index pseudo-noise (a pure splitmix-style integer
/// hash — no RNG state, no wall-clock, byte-identical on every
/// platform and every run).
fn deterministic_noise(index: u32) -> f32 {
    let mut z = index.wrapping_mul(0x9E37_79B1).wrapping_add(0x632B_E597);
    z = (z ^ (z >> 16)).wrapping_mul(0x85EB_CA6B);
    z = (z ^ (z >> 13)).wrapping_mul(0xC2B2_AE35);
    z ^= z >> 16;
    (z >> 8) as f32 / 16_777_216.0 - 0.5
}

/// One pure-function sample generator per cue (t = seconds since the
/// cue start). Original placeholder synthesis: envelopes over sine
/// tones and the deterministic noise — no external assets.
fn fire_sample(t: f32, index: u32) -> f32 {
    // A short sharp noise burst with a fast exponential decay.
    let envelope = (-t * 45.0).exp();
    deterministic_noise(index) * envelope * 0.7
}

fn impact_sample(t: f32, _index: u32) -> f32 {
    // A descending pitch thump: 170 Hz falling to 70 Hz over the cue.
    let sweep = 170.0 - 90.0 * (t / 0.13);
    let tone = (std::f32::consts::TAU * sweep * t).sin();
    let envelope = (-t * 20.0).exp();
    tone * envelope * 0.8
}

fn damage_sample(t: f32, index: u32) -> f32 {
    // A harsh mid-range burst: a 210 Hz near-square with noise and a
    // fast attack, medium decay.
    let square = if (t * 210.0).fract() < 0.5 { 1.0 } else { -1.0 };
    let envelope = (-t * 14.0).exp() * (t * 300.0).min(1.0);
    (square * 0.5 + deterministic_noise(index) * 0.3) * envelope * 0.6
}

fn interact_sample(t: f32, _index: u32) -> f32 {
    // A soft two-tone confirmation blip: 660 Hz then 880 Hz.
    let (freq, local) = if t < 0.055 {
        (660.0, t)
    } else {
        (880.0, t - 0.055)
    };
    let envelope = (-local * 30.0).exp();
    (std::f32::consts::TAU * freq * t).sin() * envelope * 0.45
}

fn complete_sample(t: f32, _index: u32) -> f32 {
    // A rising three-note arpeggio: C5, E5, G5, each ~150 ms.
    let notes = [523.25, 659.25, 783.99];
    let note = (t / 0.15).floor() as usize;
    let local = t - note as f32 * 0.15;
    let freq = notes[note.min(2)];
    let envelope = (-local * 12.0).exp() * (local * 200.0).min(1.0);
    (std::f32::consts::TAU * freq * t).sin() * envelope * 0.5
}

fn weapon_switch_sample(t: f32, index: u32) -> f32 {
    // A short mechanical click: a brief high tick over a low thock —
    // the two-layer selection sound (a control event, ~70 ms).
    let tick = (std::f32::consts::TAU * 1250.0 * t).sin() * (-t * 90.0).exp() * 0.35;
    let thock = (std::f32::consts::TAU * 165.0 * t).sin() * (-t * 38.0).exp() * 0.4;
    let latch = deterministic_noise(index) * (-t * 70.0).exp() * 0.15;
    tick + thock + latch
}

fn adversary_defeated_sample(t: f32, _index: u32) -> f32 {
    // A brief descending sting: 400 Hz falling to 200 Hz over the
    // cue — the partial-victory beat, distinct from the encounter's
    // completion arpeggio.
    let sweep = 400.0 - 200.0 * (t / 0.2);
    let tone = (std::f32::consts::TAU * sweep * t).sin();
    let envelope = (-t * 9.0).exp() * (t * 250.0).min(1.0);
    tone * envelope * 0.55
}

fn defeat_sample(t: f32, _index: u32) -> f32 {
    // A longer descending resolution: A4, F4, D4, each ~170 ms — the
    // failure counterpart of the completion arpeggio.
    let notes = [440.0, 349.23, 293.66];
    let note = (t / 0.17).floor() as usize;
    let local = t - note as f32 * 0.17;
    let freq = notes[note.min(2)];
    let envelope = (-local * 10.0).exp() * (local * 180.0).min(1.0);
    (std::f32::consts::TAU * freq * t).sin() * envelope * 0.5
}

/// Synthesizes one cue's waveform — a pure function of the cue,
/// deterministic on every run. Cross-platform float bit-identity is not
/// asserted (libm `sin`/`exp` are not guaranteed bit-portable); audio sits
/// outside every determinism pin, so only per-run determinism is claimed.
fn cue_samples(cue: Cue) -> Vec<f32> {
    let (duration_s, generator): (f32, fn(f32, u32) -> f32) = match cue {
        Cue::Fire => (0.09, fire_sample),
        Cue::Impact => (0.13, impact_sample),
        Cue::Damage => (0.18, damage_sample),
        Cue::Interact => (0.11, interact_sample),
        Cue::EncounterComplete => (0.45, complete_sample),
        Cue::WeaponSwitch => (0.07, weapon_switch_sample),
        Cue::AdversaryDefeated => (0.2, adversary_defeated_sample),
        Cue::Defeat => (0.51, defeat_sample),
    };
    let count = (duration_s * SAMPLE_RATE as f32) as usize;
    (0..count)
        .map(|index| generator(index as f32 / SAMPLE_RATE as f32, index as u32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The explicit silent mode: no SDL audio is touched, play is a
    /// no-op, and the observable cost of a refused cue is effectively
    /// zero (the no-op branch — no SDL call at all).
    #[test]
    fn disabled_audio_is_silent_and_cheap() {
        let mut audio = Audio::disabled("headless test mode");
        assert_eq!(
            audio.status(),
            &AudioStatus::Silent {
                reason: "headless test mode".to_string()
            }
        );
        assert!(!audio.status().is_active());
        let start = std::time::Instant::now();
        for _ in 0..100_000 {
            audio.play(Cue::Fire);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 100,
            "100k refused cues stay cheap: {elapsed:?}"
        );
    }

    /// The cue waveforms are pure: identical bytes on regeneration,
    /// every sample finite, every length bounded, and every cue
    /// carries actual energy (never digital silence).
    #[test]
    fn cue_waveforms_are_deterministic_finite_bounded_and_audible() {
        for cue in CUES {
            let first = cue_samples(cue);
            let second = cue_samples(cue);
            assert_eq!(first, second, "{cue:?} regenerates identically");
            assert!(
                first.len() < SAMPLE_RATE as usize,
                "{cue:?} is bounded under one second ({} samples)",
                first.len()
            );
            assert!(
                first.iter().all(|s| s.is_finite()),
                "{cue:?} has no non-finite samples"
            );
            let energy: f32 = first.iter().map(|s| s * s).sum();
            assert!(
                energy > 1.0,
                "{cue:?} carries energy (sum of squares {energy})"
            );
        }
    }
}

// ===========================================================================
// Era 2 package B part 2 — the SDL3 AUDIO DEVICE LAYER (the module
// header's second section; everything above is the Cycle-3 cue surface).
// ===========================================================================

/// The device sample rate (Hz) — the documented constant: the rate
/// `audio_synth`'s digest pins and `audio_mix`'s i16 boundary are
/// measured at, so the synthesized PCM reaches the device unresampled
/// when the hardware matches (SDL's stream model converts otherwise).
pub const DEVICE_SAMPLE_RATE: i32 = 48_000;

/// The device channel count: stereo (the pan law's two outputs).
pub const DEVICE_CHANNELS: i32 = 2;

/// The vertical slice's sound-class vocabulary — the POLICY shared
/// between the demo's `ClassMapping` table (`audio_triggers`'s
/// engine/policy boundary: the engine carries classes as data) and
/// this layer's class→source table below.
pub const SOUND_CLASS_FOOTSTEP: SoundClass = 1;

/// The looping city-hum ambience bed's class.
pub const SOUND_CLASS_AMBIENCE: SoundClass = 2;

/// The footstep's synthesis seed (`audio_synth`'s digest-pin seed —
/// the same campaign sounds the same, forever).
const FOOTSTEP_SEED: u64 = 0x1234_5678;

/// The bed's synthesis seed (`audio_synth`'s digest-pin seed).
const BED_SEED: u64 = 0x9ABC_DEF0;

/// The bed's length in seconds (the pinned 2 s loop-clean bed).
const BED_SECONDS: u32 = 2;

/// The fixed voice count — the trigger projection's pool bound: at
/// most [`TRIGGER_POOL_BOUND`] sounds are ever concurrently audible
/// (bounded work BY CONSTRUCTION). A pool under pressure STEALS the
/// round-robin victim voice, counted fail-visibly — never a silent
/// drop, never unbounded growth.
const VOICE_COUNT: usize = TRIGGER_POOL_BOUND;

/// The trigger ring's capacity: four ticks at the pool bound. The
/// callback drains the whole ring every device period (tens of
/// milliseconds) while the projection fires at tick cadence, so a
/// full ring means the audio thread stopped draining — refused
/// fail-visibly in [`AudioStats::triggers_dropped`], never a silent
/// drop.
const TRIGGER_QUEUE_CAPACITY: usize = TRIGGER_POOL_BOUND * 4;

/// The mix chunk: the fixed frame count one callback pass mixes per
/// iteration. Any SDL request size is served in whole chunks of at
/// most this many frames — the callback's work is bounded no matter
/// what the device asks for.
const MIX_CHUNK_FRAMES: usize = 2048;

/// One resident synthesized source (the class→source table's target —
/// POLICY data: which sound class plays which PCM).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceSlot {
    Footstep,
    AmbienceBed,
}

impl SourceSlot {
    fn index(self) -> usize {
        match self {
            SourceSlot::Footstep => 0,
            SourceSlot::AmbienceBed => 1,
        }
    }
}

/// The class→source mapping (the vertical slice's POLICY table: the
/// footstep class plays the procedural footstep, the ambience class
/// plays the looping bed). Unmapped classes are silence — the
/// policy's table is authoritative (the trigger core's own law).
const CLASS_TO_SOURCE: [(SoundClass, SourceSlot); 2] = [
    (SOUND_CLASS_FOOTSTEP, SourceSlot::Footstep),
    (SOUND_CLASS_AMBIENCE, SourceSlot::AmbienceBed),
];

fn source_for_class(class: SoundClass) -> Option<SourceSlot> {
    CLASS_TO_SOURCE
        .iter()
        .find(|&&(mapped, _)| mapped == class)
        .map(|&(_, slot)| slot)
}

/// The listener snapshot the mixer pans and attenuates against —
/// supplied by the application (the camera/pose layer's data; the
/// `pan_from_listener` contract: PRE-NORMALIZED, ORTHOGONAL `forward`
/// and `right` — the caller owns normalization, it may use sqrt
/// there).
///
/// The falloff parameters are the slice's spatial POLICY, carried per
/// snapshot so the application owns them. The defaults: reference
/// distance 4 (a crowd-conversation reach) and rolloff 1 (the
/// inverse-distance law's classic slope — gain exactly 0.5 at double
/// the reference distance).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub right: [f32; 3],
    pub ref_distance: f32,
    pub rolloff: f32,
}

impl Default for Listener {
    fn default() -> Listener {
        Listener {
            position: [0.0, 0.0, 0.0],
            // The animation core's convention: +Z forward, +X right.
            forward: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            ref_distance: 4.0,
            rolloff: 1.0,
        }
    }
}

/// The typed device-layer refusal (the degradation gate's shape):
/// audio is NEVER a prerequisite — every variant degrades to silence,
/// and `NoDevice` is the deviceless-machine path the CI pod runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    /// No playback device exists (SDL enumerates zero, or the audio
    /// subsystem refuses to start — the deviceless pod's shape). The
    /// caller's answer is typed silence: print it, keep the frame.
    NoDevice,
    /// A device exists but the callback stream could not be opened or
    /// started (SDL's reason carried — a rig/configuration failure
    /// worth seeing; degrades identically).
    StreamOpen(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoDevice => write!(f, "no audio playback device"),
            AudioError::StreamOpen(reason) => write!(f, "the audio stream: {reason}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// The fail-visible diagnostics the §4 gates read (the
/// truncation-counter pattern: every anomaly is COUNTED, never
/// swallowed). All counters are Relaxed atomics written on the audio
/// thread and read here — diagnostics, never synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioStats {
    /// Device underruns: callback invocations where SDL refused the
    /// mixed chunk (the device then plays silence for the shortfall).
    /// Zero across warmed frames is the §4.1 gate.
    pub underruns: u64,
    /// The worst (maximum) callback wall cost in nanoseconds — the
    /// §4.2 gate's tail metric, budgeted against the device period.
    pub worst_callback_ns: u64,
    /// Voices currently audible (the one-shot pool plus the bed, if
    /// playing).
    pub voices_active: u32,
    /// One-shot triggers that arrived while the voice pool was full —
    /// each stole the pool's next round-robin voice (bounded work
    /// guaranteed; the steal is the audible anomaly to see).
    pub voices_stolen: u64,
    /// Triggers refused because the ring was full — the audio thread
    /// had stopped draining (an engine bug, not a workload).
    pub triggers_dropped: u64,
}

/// The command cell's payload: the listener snapshot plus the bounded
/// trigger ring (fixed slots, head/count indices — no allocation on
/// either side; the slots are overwritten in place).
struct SharedCommands {
    listener: Listener,
    ring: Box<[SoundTrigger; TRIGGER_QUEUE_CAPACITY]>,
    /// The oldest queued trigger's slot.
    head: usize,
    /// The queued trigger count (`head + count` is the next write).
    count: usize,
}

/// The audio-thread counters (Relaxed — fail-visible diagnostics,
/// never synchronization).
struct AudioCounters {
    underruns: AtomicU64,
    worst_callback_ns: AtomicU64,
    voices_active: AtomicU32,
    voices_stolen: AtomicU64,
    triggers_dropped: AtomicU64,
}

/// The ONE shared cell between the main thread (producer: triggers,
/// listener) and the audio thread (consumer: the callback): the
/// command mutex plus the counter block. The SPSC shape is by
/// construction — [`AudioDevice`] is the single producer and SDL's
/// single callback thread is the single consumer.
struct SharedAudio {
    commands: Mutex<SharedCommands>,
    counters: AudioCounters,
}

impl SharedAudio {
    fn new() -> SharedAudio {
        // The ring's placeholder slot: an unmapped class (0), so even
        // a never-written slot drained by a bug is silence, not a
        // crash.
        let placeholder = SoundTrigger {
            class: 0,
            emitter: SoundEmitter::World(0),
            position: [0.0; 3],
            tier: LodLevel::Full,
        };
        SharedAudio {
            commands: Mutex::new(SharedCommands {
                listener: Listener::default(),
                ring: Box::new([placeholder; TRIGGER_QUEUE_CAPACITY]),
                head: 0,
                count: 0,
            }),
            counters: AudioCounters {
                underruns: AtomicU64::new(0),
                worst_callback_ns: AtomicU64::new(0),
                voices_active: AtomicU32::new(0),
                voices_stolen: AtomicU64::new(0),
                triggers_dropped: AtomicU64::new(0),
            },
        }
    }

    /// The producer side (main thread): appends triggers to the ring,
    /// bounded, counting refusals fail-visibly. Mutex poisoning is
    /// recovered by taking the cell's payload anyway — the slots are
    /// plain Copy data, structurally valid through any panic (and
    /// the callback cannot panic: every path is a bounds-checked walk
    /// over fixed buffers, no unwrap, no division).
    fn submit(&self, triggers: &[SoundTrigger]) {
        let mut commands = self
            .commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for &trigger in triggers {
            if commands.count >= TRIGGER_QUEUE_CAPACITY {
                self.counters
                    .triggers_dropped
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let slot = (commands.head + commands.count) % TRIGGER_QUEUE_CAPACITY;
            commands.ring[slot] = trigger;
            commands.count += 1;
        }
    }

    /// The producer side: publishes the listener snapshot the next
    /// drain will pan against.
    fn set_listener(&self, listener: Listener) {
        self.commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .listener = listener;
    }
}

/// One pooled voice: a resident source, a play cursor, and the
/// precomputed per-channel gains (pan × attenuation, derived AT THE
/// MIXER from the listener snapshot — the trigger core's
/// "derive LATER" contract).
#[derive(Debug, Clone, Copy)]
struct Voice {
    source: SourceSlot,
    cursor: usize,
    gain: (f32, f32),
    looping: bool,
    active: bool,
}

impl Voice {
    const IDLE: Voice = Voice {
        source: SourceSlot::Footstep,
        cursor: 0,
        gain: (0.0, 0.0),
        looping: false,
        active: false,
    };
}

/// The Euclidean distance (the attenuation input). `f32::sqrt` is
/// IEEE-754 correctly-rounded — bit-identical on every platform, so
/// unlike the pan law's cos/sin (not bit-portable, hence audio_mix's
/// offline-sampled table) it needs no sampling; and the mixer pays it
/// once per voice START (at most the pool bound per callback), never
/// per sample.
fn euclidean(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// The asset-time i16→f32 conversion (the synth's PCM into the
/// mixer's f32 domain): `sample / 32767` — the exact inverse scale of
/// `to_i16`'s symmetric [−32767, +32767] range, so a round trip
/// i16 → f32 → i16 is the identity to rounding. Asset time only,
/// never the audio thread.
fn to_f32(samples: &[i16]) -> Vec<f32> {
    samples
        .iter()
        .map(|&sample| f32::from(sample) / 32767.0)
        .collect()
}

/// The owned mixer (§2.3): the voice pool, the resident sources, the
/// mix scratch — ALL pre-allocated at construction. This struct is
/// moved into SDL's callback box; every method that runs on the audio
/// thread (the drain, the voice start, the render, the callback
/// itself) allocates nothing and waits on nothing.
struct MixerCore {
    voices: [Voice; VOICE_COUNT],
    /// Round-robin search/steal cursor (the fair steal: the next
    /// voice after the last one handed out).
    voice_hint: usize,
    /// The resident sources, indexed by [`SourceSlot::index`] —
    /// synthesized ONCE at construction and converted to f32 there
    /// (asset time; the vertical slice ships zero audio assets — the
    /// procedural floor).
    sources: [Vec<f32>; 2],
    /// Stereo interleaved f32 mix scratch ([`MIX_CHUNK_FRAMES`]
    /// frames).
    mix: Box<[f32]>,
    /// Stereo interleaved i16 output scratch (the device boundary).
    out: Box<[i16]>,
    /// The shared command cell + counters (Arc-shared with the
    /// device wrapper).
    shared: Arc<SharedAudio>,
}

impl MixerCore {
    /// Builds the core and synthesizes the resident sources (the
    /// allocation-legal asset pass — never on the audio thread). The
    /// synth's i16 PCM is converted to f32 HERE, once: the mixer is
    /// f32 end-to-end (`audio_mix`'s law — the i16 boundary is the
    /// single final `to_i16`), so the audio thread never converts.
    fn new(shared: Arc<SharedAudio>) -> MixerCore {
        let sources = [
            to_f32(&synthesize_footstep(
                FOOTSTEP_SEED,
                DEVICE_SAMPLE_RATE as u32,
            )),
            to_f32(&synthesize_ambience_bed(
                BED_SEED,
                DEVICE_SAMPLE_RATE as u32,
                BED_SECONDS,
            )),
        ];
        MixerCore {
            voices: [Voice::IDLE; VOICE_COUNT],
            voice_hint: 0,
            sources,
            mix: vec![0.0; MIX_CHUNK_FRAMES * 2].into_boxed_slice(),
            out: vec![0; MIX_CHUNK_FRAMES * 2].into_boxed_slice(),
            shared,
        }
    }

    /// The audio state's resident bytes (§4.4's counted, constant
    /// bound: sources + pool + scratch — fixed after construction).
    fn resident_bytes(&self) -> u64 {
        let sources: usize = self.sources.iter().map(|s| s.len() * 4).sum();
        (sources
            + VOICE_COUNT * size_of::<Voice>()
            + self.mix.len() * size_of::<f32>()
            + self.out.len() * size_of::<i16>()) as u64
    }

    /// THE DRAIN (the callback's first act, once per invocation —
    /// never per trigger): takes the command cell with `try_lock`. A
    /// failed attempt (the producer mid-submit, or the poison the
    /// callback cannot cause) DEFERS the drain by one device period —
    /// the audio thread never waits on a lock (§1.2's "no locks on
    /// contended paths" read as no BLOCKING; the producer's critical
    /// section is a bounded copy of Copy slots, microseconds).
    fn drain_commands(&mut self) {
        // The Arc clone releases the self borrow so `start_voice`
        // can take `&mut self` while the guard lives (a refcount
        // bump — no allocation).
        let shared = Arc::clone(&self.shared);
        let Ok(mut commands) = shared.commands.try_lock() else {
            return;
        };
        let listener = commands.listener;
        while commands.count > 0 {
            let trigger = commands.ring[commands.head];
            commands.head = (commands.head + 1) % TRIGGER_QUEUE_CAPACITY;
            commands.count -= 1;
            self.start_voice(trigger, listener);
        }
    }

    /// Maps one drained trigger to a pooled voice: the class→source
    /// table (policy), the listener-relative gains (the mixer math,
    /// AT the mixer — §2.4's "derive LATER" contract), and the pool's
    /// free-slot-or-steal handout. Unmapped classes are silence (the
    /// policy table is authoritative — the trigger core's own law).
    fn start_voice(&mut self, trigger: SoundTrigger, listener: Listener) {
        let Some(slot) = source_for_class(trigger.class) else {
            return;
        };
        let looping = slot == SourceSlot::AmbienceBed;
        // The bed is IDEMPOTENT and resident: one bed voice, ever —
        // an ambience trigger on an already-playing bed is the
        // policy's no-op (the bed loops until the device drops).
        if looping
            && self
                .voices
                .iter()
                .any(|voice| voice.active && voice.looping)
        {
            return;
        }
        let gain = if looping {
            // The ambient bed: equal-power center, NO distance falloff
            // (a district bed is everywhere, not at a point).
            pan_gains(0.0)
        } else {
            let pan = pan_from_listener(
                trigger.position,
                listener.position,
                listener.forward,
                listener.right,
            );
            let reach = attenuation(
                euclidean(trigger.position, listener.position),
                listener.ref_distance,
                listener.rolloff,
            );
            let (left, right) = pan_gains(pan);
            (left * reach, right * reach)
        };
        // The handout: the first idle voice from the hint (round
        // robin), else the hint's voice is stolen — bounded work,
        // counted fail-visibly, never a silent drop.
        let mut index = self.voice_hint;
        for _ in 0..VOICE_COUNT {
            if !self.voices[index].active {
                break;
            }
            index = (index + 1) % VOICE_COUNT;
        }
        self.voice_hint = (index + 1) % VOICE_COUNT;
        if self.voices[index].active {
            self.shared
                .counters
                .voices_stolen
                .fetch_add(1, Ordering::Relaxed);
        }
        self.voices[index] = Voice {
            source: slot,
            cursor: 0,
            gain,
            looping,
            active: true,
        };
    }

    /// Mixes `frames` stereo frames into the i16 scratch (clamped to
    /// the scratch capacity — the panic-free guarantee): the f32
    /// accumulation pass (the bed and the one-shots through
    /// `mix_into`, the looping bed wrapping at its loop-clean
    /// boundary), then the single clamp/convert boundary (`to_i16`).
    /// Fixed order: pool order, then each voice's own sample order.
    /// Allocation-free and panic-free (the shorter-length-wins kernel
    /// bounds every slice).
    fn render_chunk(&mut self, frames: usize) {
        let frames = frames.min(MIX_CHUNK_FRAMES);
        let samples = frames * 2;
        self.mix[..samples].fill(0.0);
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            let source = &self.sources[voice.source.index()];
            if source.is_empty() {
                voice.active = false;
                continue;
            }
            let mut written = 0;
            while written < frames {
                let left = source.len() - voice.cursor;
                if left == 0 {
                    if voice.looping {
                        voice.cursor = 0;
                        continue;
                    }
                    voice.active = false;
                    break;
                }
                let segment = left.min(frames - written);
                mix_into(
                    &mut self.mix[written * 2..][..segment * 2],
                    &source[voice.cursor..][..segment],
                    voice.gain,
                );
                voice.cursor += segment;
                written += segment;
            }
        }
        for (out, &mix) in self.out[..samples]
            .iter_mut()
            .zip(self.mix[..samples].iter())
        {
            *out = to_i16(mix);
        }
    }
}

impl AudioCallback<i16> for MixerCore {
    /// THE AUDIO THREAD (§1.2's strictest discipline): drains the
    /// command cell (try_lock — never waits), mixes the requested
    /// sample count in bounded chunks through the pre-allocated
    /// scratch, hands each chunk to SDL, and publishes the counters.
    /// No allocation (everything touched is pre-allocated; SDL's own
    /// internal put handling is SDL's, inside its locked queue), no
    /// blocking, no unbounded walk (chunk ≤ [`MIX_CHUNK_FRAMES`];
    /// drain ≤ the fixed ring). SDL requests whole stereo frames —
    /// an odd trailing sample count (never observed) is ignored
    /// rather than mixed into a half frame.
    fn callback(&mut self, stream: &mut AudioStream, requested: i32) {
        let started = Instant::now();
        self.drain_commands();
        let mut underruns: u64 = 0;
        let mut remaining = requested.max(0) as usize;
        while remaining >= 2 {
            let frames = (remaining / 2).min(MIX_CHUNK_FRAMES);
            let samples = frames * 2;
            self.render_chunk(frames);
            if stream.put_data_i16(&self.out[..samples]).is_err() {
                // SDL refused the chunk: the device plays silence for
                // the shortfall — the underrun, counted fail-visibly.
                underruns += 1;
                break;
            }
            remaining -= samples;
        }
        let counters = &self.shared.counters;
        counters.underruns.fetch_add(underruns, Ordering::Relaxed);
        counters
            .worst_callback_ns
            .fetch_max(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let active = self.voices.iter().filter(|voice| voice.active).count() as u32;
        counters.voices_active.store(active, Ordering::Relaxed);
    }
}

/// The SDL3 audio device wrapper (package B part 2's surface): the
/// default playback device opened through the sdl3 crate's SAFE
/// stream-callback API at 48 kHz stereo S16, carrying the owned mixer
/// on SDL's audio thread. LAZY by design: the application constructs
/// this on first audio use — nothing here runs at startup.
///
/// Drop order is declaration order: the stream drops FIRST — SDL
/// destroys the stream (closing the logical device it opened) under
/// its internal device lock, which guarantees the callback is not
/// running when the destroy returns (SDL 3.4.18's documented destroy
/// path — the wrapper's ownership model relies on it) — and only then
/// does the callback box (and its `Arc` to the shared cell) release,
/// before the wrapper's own `Arc` handles.
pub struct AudioDevice {
    /// The callback-carrying stream (dropped first — see the struct
    /// docs). Ownership-only: holding it keeps the device open and
    /// the mixer's callback running; its Drop is the read.
    #[expect(dead_code)]
    stream: AudioStreamWithCallback<MixerCore>,
    /// The producer/diagnostics side of the shared cell.
    shared: Arc<SharedAudio>,
    /// SDL's audio driver name (the run summary prints it).
    driver: String,
    /// The opened device's name.
    device_name: String,
    /// The audio state's counted residency (constant after
    /// construction — §4.4).
    resident_bytes: u64,
}

impl AudioDevice {
    /// Opens the DEFAULT playback device at 48 kHz stereo S16 and
    /// starts the mixer on SDL's audio thread (the stream get-
    /// callback model — see the module header's part-2 section for
    /// why the callback shape was chosen over the push shape). The
    /// typed refusals: [`AudioError::NoDevice`] when the machine has
    /// no playback device (or the audio subsystem refuses to start —
    /// the deviceless pod's path: typed silence, the frame
    /// unaffected), [`AudioError::StreamOpen`] when a device exists
    /// but the callback stream refuses to open or start. Never
    /// blocks, never crashes — the degradation gate.
    pub fn try_new(platform: &Platform) -> Result<AudioDevice, AudioError> {
        let subsystem = platform.sdl().audio().map_err(|_| AudioError::NoDevice)?;
        // The deviceless refusal: zero enumerated playback devices —
        // checked BEFORE the open, so absence (the pod) is NoDevice,
        // distinct from a stream failure on a present device (the
        // rig's configuration problem). An enumeration refusal is
        // read the same way: no devices to see.
        let has_device = subsystem
            .audio_playback_device_ids()
            .map(|ids| !ids.is_empty())
            .unwrap_or(false);
        if !has_device {
            return Err(AudioError::NoDevice);
        }
        let shared = Arc::new(SharedAudio::new());
        let core = MixerCore::new(Arc::clone(&shared));
        let resident_bytes =
            core.resident_bytes() + (TRIGGER_QUEUE_CAPACITY * size_of::<SoundTrigger>()) as u64;
        let spec = AudioSpec {
            freq: Some(DEVICE_SAMPLE_RATE),
            channels: Some(DEVICE_CHANNELS),
            format: Some(AudioFormat::s16_sys()),
        };
        let stream = subsystem
            .open_playback_stream(&spec, core)
            .map_err(|error| AudioError::StreamOpen(error.to_string()))?;
        stream
            .resume()
            .map_err(|error| AudioError::StreamOpen(error.to_string()))?;
        let device_name = stream
            .device_name()
            .unwrap_or_else(|| "unknown".to_string());
        Ok(AudioDevice {
            stream,
            shared,
            driver: subsystem.current_audio_driver().to_string(),
            device_name,
            resident_bytes,
        })
    }

    /// The trigger consumer (the main thread, once per tick): appends
    /// the projection's bounded output to the ring the callback
    /// drains at ITS start. Bounded and fail-visible — refusals land
    /// in [`AudioStats::triggers_dropped`].
    pub fn submit_triggers(&mut self, triggers: &[SoundTrigger]) {
        self.shared.submit(triggers);
    }

    /// Publishes the listener snapshot the next drain pans and
    /// attenuates against (the camera/pose layer's data — the gains
    /// are derived AT the callback, never in the trigger core).
    pub fn set_listener(&mut self, listener: Listener) {
        self.shared.set_listener(listener);
    }

    /// The fail-visible diagnostics (§4's gates): underruns, the
    /// worst callback cost, the live voice count, and the two
    /// refusals (steals, drops). Relaxed reads — diagnostics, never
    /// synchronization.
    pub fn stats(&self) -> AudioStats {
        let counters = &self.shared.counters;
        AudioStats {
            underruns: counters.underruns.load(Ordering::Relaxed),
            worst_callback_ns: counters.worst_callback_ns.load(Ordering::Relaxed),
            voices_active: counters.voices_active.load(Ordering::Relaxed),
            voices_stolen: counters.voices_stolen.load(Ordering::Relaxed),
            triggers_dropped: counters.triggers_dropped.load(Ordering::Relaxed),
        }
    }

    /// The audio state's counted residency in bytes (sources + voice
    /// pool + scratch + ring): constant after construction — §4.4's
    /// gate reads it across warmed frames and expects no movement.
    pub fn residency_bytes(&self) -> u64 {
        self.resident_bytes
    }

    /// The SDL audio driver behind the device (the run summary).
    pub fn driver(&self) -> &str {
        &self.driver
    }

    /// The opened device's name (the run summary).
    pub fn device_name(&self) -> &str {
        &self.device_name
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;
    use engine_core::npc_behavior::NpcId;

    fn core() -> (MixerCore, Arc<SharedAudio>) {
        let shared = Arc::new(SharedAudio::new());
        (MixerCore::new(Arc::clone(&shared)), shared)
    }

    fn footstep_at(position: [f32; 3]) -> SoundTrigger {
        SoundTrigger {
            class: SOUND_CLASS_FOOTSTEP,
            emitter: SoundEmitter::Npc(NpcId(7)),
            position,
            tier: LodLevel::Full,
        }
    }

    fn ambience() -> SoundTrigger {
        SoundTrigger {
            class: SOUND_CLASS_AMBIENCE,
            emitter: SoundEmitter::World(0),
            position: [0.0; 3],
            tier: LodLevel::Full,
        }
    }

    /// Renders `frames` frames through the chunked path and collects
    /// the interleaved i16 output (tests may allocate — the audio
    /// thread never does).
    fn render_collect(mixer: &mut MixerCore, frames: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(frames * 2);
        let mut left = frames;
        while left > 0 {
            let chunk = left.min(MIX_CHUNK_FRAMES);
            mixer.render_chunk(chunk);
            out.extend_from_slice(&mixer.out[..chunk * 2]);
            left -= chunk;
        }
        out
    }

    /// Silence before any trigger: the mixer's resting output is
    /// digital silence (i16 zero).
    #[test]
    fn idle_output_is_silence() {
        let (mut mixer, _shared) = core();
        mixer.drain_commands();
        let out = render_collect(&mut mixer, 256);
        assert!(out.iter().all(|&sample| sample == 0));
    }

    /// The footstep path end to end (no device needed): submit →
    /// drain → render produces real energy, and a HARD-RIGHT source
    /// is silent in the LEFT channel (the pan law's known answer —
    /// pan +1 is the (0, 1) gain pair).
    #[test]
    fn footstep_renders_panned_energy() {
        let (mut mixer, shared) = core();
        shared.submit(&[footstep_at([5.0, 0.0, 0.0])]);
        mixer.drain_commands();
        let out = render_collect(&mut mixer, 256);
        let left: i64 = (0..256).map(|frame| out[frame * 2].abs() as i64).sum();
        let right: i64 = (0..256).map(|frame| out[frame * 2 + 1].abs() as i64).sum();
        assert_eq!(left, 0, "hard right is left-silent");
        assert!(right > 1000, "the burst is audible (energy {right})");
    }

    /// The listener snapshot publishes: moving the listener pans the
    /// same world source the other way (the `pan_from_listener`
    /// offset known answer, through the device path).
    #[test]
    fn the_listener_snapshot_repanns() {
        let (mut mixer, shared) = core();
        shared.set_listener(Listener {
            position: [10.0, 0.0, 0.0],
            ..Listener::default()
        });
        shared.submit(&[footstep_at([0.0, 0.0, 0.0])]);
        mixer.drain_commands();
        let out = render_collect(&mut mixer, 256);
        // The source at the origin is hard LEFT of the moved listener.
        let left: i64 = (0..256).map(|frame| out[frame * 2].abs() as i64).sum();
        let right: i64 = (0..256).map(|frame| out[frame * 2 + 1].abs() as i64).sum();
        assert_eq!(right, 0, "hard left is right-silent");
        assert!(left > 1000, "the burst is audible (energy {left})");
    }

    /// The distance attenuation's exact half: the SAME seed's burst
    /// at double the reference distance (dead ahead, so pan 0 on both)
    /// plays at EXACTLY half the samples of the burst at the
    /// reference — the gain pair 0.7071 vs 0.35355 is a power-of-two
    /// ratio, exact in f32 and preserved through the i16 boundary to
    /// rounding.
    #[test]
    fn double_reference_distance_is_exactly_half_as_loud() {
        let (mut near, near_shared) = core();
        let (mut far, far_shared) = core();
        near_shared.submit(&[footstep_at([0.0, 0.0, 4.0])]); // at the reference
        far_shared.submit(&[footstep_at([0.0, 0.0, 8.0])]); // double, dead ahead
        near.drain_commands();
        far.drain_commands();
        let near_out = render_collect(&mut near, 3840); // the whole burst
        let far_out = render_collect(&mut far, 3840);
        for i in 0..near_out.len() {
            let doubled = i32::from(far_out[i]) * 2;
            let reference = i32::from(near_out[i]);
            assert!(
                (doubled - reference).abs() <= 2,
                "sample {i}: far {doubled}/2 vs near {reference}"
            );
        }
    }

    /// The resident bed: one ambience trigger starts ONE looping
    /// voice (a second trigger is the policy's idempotent no-op); the
    /// bed is channel-symmetric (equal-power center) and wraps
    /// seamlessly — rendered across TWICE the bed's length, every
    /// window carries real energy (the loop-clean crossfade's
    /// boundary, audible through the device path).
    #[test]
    fn the_bed_is_resident_idempotent_and_loop_clean() {
        let (mut mixer, shared) = core();
        shared.submit(&[ambience(), ambience()]);
        mixer.drain_commands();
        let beds = mixer
            .voices
            .iter()
            .filter(|voice| voice.active && voice.looping)
            .count();
        assert_eq!(beds, 1, "the bed is idempotent");
        let bed_len = mixer.sources[SourceSlot::AmbienceBed.index()].len();
        let out = render_collect(&mut mixer, bed_len * 2);
        assert_eq!(out.len(), bed_len * 4);
        for frame in 0..out.len() / 2 {
            assert_eq!(
                out[frame * 2],
                out[frame * 2 + 1],
                "equal-power center: both channels identical (frame {frame})"
            );
        }
        for window in out.chunks(MIX_CHUNK_FRAMES * 2) {
            let peak = window.iter().map(|s| s.abs()).max().unwrap_or(0);
            assert!(
                peak > 50,
                "the bed never falls silent across the wrap (window peak {peak})"
            );
        }
        // The looping voice never retires.
        assert!(
            mixer
                .voices
                .iter()
                .any(|voice| voice.active && voice.looping)
        );
    }

    /// The pool bound: more one-shot triggers than voices steal the
    /// round-robin victim, counted fail-visibly — exactly the overflow
    /// count — and the active count never exceeds the pool.
    #[test]
    fn the_voice_pool_is_bounded_and_steals_are_counted() {
        let (mut mixer, shared) = core();
        let triggers: Vec<SoundTrigger> = (0..VOICE_COUNT + 6)
            .map(|i| footstep_at([i as f32, 0.0, 0.0]))
            .collect();
        shared.submit(&triggers);
        mixer.drain_commands();
        let active = mixer.voices.iter().filter(|voice| voice.active).count();
        assert_eq!(active, VOICE_COUNT, "the pool is the bound");
        assert_eq!(
            shared.counters.voices_stolen.load(Ordering::Relaxed),
            6,
            "the six above the bound stole, fail-visible"
        );
        assert_eq!(
            shared.counters.triggers_dropped.load(Ordering::Relaxed),
            0,
            "the ring held them all"
        );
    }

    /// The ring bound: triggers beyond four pool bounds (an audio
    /// thread that stopped draining — the only way it fills) are
    /// refused fail-visibly, never silently, never fatally; the
    /// subsequent drain processes exactly the ring's capacity.
    #[test]
    fn ring_overflow_is_counted() {
        let (mut mixer, shared) = core();
        let triggers: Vec<SoundTrigger> = (0..TRIGGER_QUEUE_CAPACITY + 44)
            .map(|i| footstep_at([i as f32, 0.0, 0.0]))
            .collect();
        shared.submit(&triggers);
        assert_eq!(
            shared.counters.triggers_dropped.load(Ordering::Relaxed),
            44,
            "the 44 above the ring are counted, never silently dropped"
        );
        mixer.drain_commands();
        let active = mixer.voices.iter().filter(|voice| voice.active).count();
        assert_eq!(active, VOICE_COUNT);
        assert_eq!(
            shared.counters.voices_stolen.load(Ordering::Relaxed),
            (TRIGGER_QUEUE_CAPACITY - VOICE_COUNT) as u64
        );
    }

    /// Residency is constant across warmed renders (§4.4): the
    /// counted bytes do not move while voices come and go.
    #[test]
    fn residency_is_constant_across_renders() {
        let (mut mixer, shared) = core();
        let before = mixer.resident_bytes();
        assert!(before > 0);
        shared.submit(&[footstep_at([1.0, 0.0, 1.0]), ambience()]);
        mixer.drain_commands();
        render_collect(&mut mixer, 8192);
        assert_eq!(mixer.resident_bytes(), before);
    }

    /// Unmapped classes are silence (the policy table is
    /// authoritative), not an error and not a voice.
    #[test]
    fn unmapped_classes_are_silence() {
        let (mut mixer, shared) = core();
        shared.submit(&[SoundTrigger {
            class: 999,
            ..footstep_at([1.0, 0.0, 0.0])
        }]);
        mixer.drain_commands();
        assert!(mixer.voices.iter().all(|voice| !voice.active));
        let out = render_collect(&mut mixer, 64);
        assert!(out.iter().all(|&sample| sample == 0));
    }
}
