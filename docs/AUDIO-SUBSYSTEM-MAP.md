# AUDIO SUBSYSTEM MAP — the groundwork document

> **Status:** Research + map v1.0 · 2026-11-24 · **UNIMPLEMENTED DESIGN —
> nothing here is ratified, nothing here authorizes code or a dependency.**
> This is the recursive queue's third item: "Audio groundwork — the
> subsystem map." It maps what the engine's audio surface must be, which
> layer could come from where, and the gates any implementation must pass —
> ending in an owner-decision list. The registry (`DECISIONS.md`, T-06's
> audio clause) owns the decision.
>
> **Ownership:** this file maps the audio subsystem's boundaries. It
> **consumes** (never redefines): [`DECISIONS.md`](DECISIONS.md) (T-04 SDL3;
> T-06 audio among the standing deferrals; V-02 RCT-class efficiency; V-05
> strategic reuse; E-05 the functional-editor-first preference), the
> **built engine** (the vendored-static SDL 3.4.18 — T-09 — whose audio
> subsystem is ALREADY IN THE BINARY; the spine's event model; the
> presentation-never-enters-the-spine precedent), and
> [`CUSTOM-ENGINE-RESEARCH.md`](CUSTOM-ENGINE-RESEARCH.md) §3's principles
> (advertised features are claims; equivalent-workload measurement; the
> evidence key below).
>
> **Evidence key:** **[DOCUMENTED]** = fetched and read this pass (dated
> 2026-11-24, §6); **[PROJECT EVIDENCE]** = demonstrated in this repository;
> **[MEASURED-PENDING]** = the gate any implementation must run;
> **[OPEN]** = the owner's decision.

---

## 0. What audio the first game needs (the requirement shape)

From the ratified vision and the built scene, the honest workload list:

| Need | Source | Character |
|---|---|---|
| The living city's AMBIENCE — rain, distant crowds, traffic hum, the day/night cycle's palette | V-03; the corner's `--day-night` | Continuous beds, weather/time-driven crossfades |
| EVENT sounds — footsteps (the walking crowd!), vehicle passes, combat hits, UI | G-02; the Era-2 walking crowd | One-shots, positionally placed, LOD-gated |
| Music | the RPG baseline | Streamed, mixed under the beds |
| Positional/3D placement | the 3D world | Listener-relative panning/attenuation (stereo at minimum) |
| Voice/dialogue audio | DESIGN-BIBLE (quest content) | Later; the map must not foreclose it |
| Editor/tooling audition | E-05 | Later; the map must not foreclose it |

The crowd connection is the near-term driver: the Era-2 walking population
and the NPC AI design pass's behavior layer produce DETERMINISTIC activity
events — footsteps are the audible projection of the very events the spine
already schedules. Audio is the next presentation surface after rendering,
and it composes with the same tiering (a dormant NPC makes no sound).

## 1. The architecture constraints (the engine's law, applied to audio)

1. **Audio is PRESENTATION — it never enters the spine's fingerprinted
   state** (the free-look precedent, stated as law for this subsystem):
   no audio state, buffer, or timing enters a fingerprint, a gate, or a
   measured determinism claim. What IS deterministic: WHICH sounds fire —
   sound triggers derive from spine events and render-side observations of
   deterministic state, so the same campaign produces the same trigger
   STREAM; the audible rendering (mixing, resampling, device timing) is
   outside the contract. This split is the same two-lane shape as the
   physics decision doc's, and for the same reason.
2. **The audio thread is a real-time thread with the strictest
   allocation discipline in the engine**: no allocation, no locks on
   contended paths, no unbounded work in the callback (the classic
   audio-thread law — a stronger form of the renderer's
   zero-per-frame-allocation gate). The gate form: underrun/xrun counters
   (fail-visible, like the truncation counters) and a mix-cost budget.
3. **p99 is the metric** (V-02): an audio glitch is a tail event — the
   gates count underruns and measure worst-case callback time, never
   average throughput alone.
4. **Memory discipline**: sound residency is explicit (resident beds vs
   streamed music vs one-shot pools), bounded, and counted — the renderer's
   `allocations()` counter pattern extended to audio state.
5. **Cross-platform**: the CI trio (Windows x64, Linux x64, macOS ARM64)
   plus the existing headless/offscreen discipline — the engine runs where
   no audio device exists (the dev pod!), so audio initializes lazily and
   DEGRADES to silence typed-refused, never crashes or blocks the frame
   (the dummy-driver precedent from the shell).

## 2. The map — five layers, each with its candidate set

### 2.1 Device I/O (the callback boundary to the OS)

The layer with the clearest shortlist, because **SDL3's audio subsystem is
already in the binary** [PROJECT EVIDENCE — the vendored-static SDL 3.4.18,
T-09]:

- **SDL3 audio** [DOCUMENTED — SDL3 wiki, CategoryAudio, fetched
  2026-11-24]: everything revolves around `SDL_AudioStream` — playback and
  recording, format conversion mid-stream, mixing of all bound streams by
  the device, app-provided get-callbacks, per-stream gain and frequency
  ratio, channel maps, a postmix callback, `SDL_LoadWAV` built in;
  **logical devices** with automatic migration when the system default
  changes (headphones plugged in — "the app doesn't even have to know it
  happened"); no compressed decoding (the app provides it). Zero new
  dependency, zero new build system, one already-audited vendored tree.
- **cpal** [DOCUMENTED — github.com/RustAudio/cpal, fetched 2026-11-24]:
  Apache-2.0, pure Rust, 4k stars; Windows WASAPI (+ optional ASIO), Linux
  ALSA (+ optional JACK/PipeWire/PulseAudio), macOS/iOS CoreAudio, Android
  AAudio, WASM; MSRV 1.85 (the pinned 1.99.1 toolchain clears it);
  `realtime`/`realtime-dbus` features for RT-priority promotion; documented
  Linux sound-server pitfalls (PipeWire/PulseAudio hold the ALSA default;
  native features avoid the bridge glitches). Costs: a SECOND audio stack
  beside SDL in the same binary, system dev-dependencies on Linux CI
  (`libasound2-dev`), and a second device-enumeration surface to gate.

**The map's position**: SDL3 audio is the default device layer — it is
already paid for, already cross-platform, already vendored-audited; cpal is
the alternative IF finer backend control (ASIO-class latency, JACK routing)
becomes a measured need. [OPEN] for the owner; the first implementation
package should not add a dependency the binary already carries.

### 2.2 Decode/asset ingest (compressed → PCM)

- **Symphonia** [DOCUMENTED — github.com/pdeljanov/Symphonia, fetched
  2026-11-24]: pure 100%-safe Rust, MPL-2.0, 3.5k stars, MSRV 1.53 (the
  crate manifest's `rust-version`); FLAC /
  MP3 / PCM / Vorbis / WAV at "Excellent" status (compliance-tested or
  reference-matched over a large corpus), gapless playback supported,
  feature-flag codec selection (royalty-free defaults), fuzz-tested,
  ±15% of FFMpeg decode performance claimed (their benchmark, not ours —
  [MEASURED-PENDING] if it matters). **Licensing note for the owner**:
  MPL-2.0 is file-level copyleft — lawful for a commercial release as an
  unmodified crate, but it is the workspace's first MPL dependency and
  deserves an explicit owner ratification (V-05's discipline).
- **Build-time cooking** (the engine's own future asset pipeline): decode
  ONCE at cook time into the engine's packed PCM format (the
  content-addressed store's shape) — Symphonia (or any decoder) then lives
  at BUILD time only; the shipped binary and the runtime dependency graph
  stay decode-free. `SDL_LoadWAV` [DOCUMENTED] is the zero-dependency floor
  for WAV-only content in the interim.

**The map's position**: cook-time decoding into an owned packed format,
  with the runtime carrying no decoder at all — the RCT-class instinct
  (pay once at build, carry nothing at runtime), the same shape as the
  renderer's build-time SPIR-V. The interim floor is `SDL_LoadWAV`.

### 2.3 Mixing and spatialization (the sample graph)

- **Owned** (the map's default): a small mixer inside the engine —
  per-source gain/pan/attenuation, beds with crossfades, one-shot voice
  pooling — running on SDL3's device thread through the stream/get-callback
  model. The ambition is RCT-class: a mixer is arithmetic over bounded
  buffers, fully ownable, and the audio-thread allocation law is OURS to
  enforce either way. Stereo panning + distance attenuation covers the
  first game's floor; HRTF-class 3D audio is a later, separately-ratified
  ambition [OPEN].
- **A game-audio engine** (kira-class candidates, rodio as the cpal-side
  playback layer): not fetched this pass — honestly labeled **[OPEN]**.
  The trade to document when fetched: feature breadth (effect graphs,
  tweening) vs framework gravity vs the allocation discipline the engine
  must impose on whatever runs on its audio thread.

### 2.4 Scheduling (deterministic triggers → audible stream)

Sound triggers derive from the spine's event stream and the render-side
observations of deterministic state — the same events the NPC AI design
pass §5's perception consumes (a footstep event, a vehicle-pass event, a
combat-hit event). The LOD tiering composes for free: a dormant/far NPC
fires no events, so it makes no sound; the corner's near field carries the
audible crowd. The engine primitive: an event→trigger projection with a
bounded one-shot pool and LOD gating; the policy: which events carry which
sounds. **Presentation never enters the spine** (§1) — the trigger
projection is a READER of the spine, exactly like the pose evaluation.

### 2.5 The tooling surface (later, deliberately)

Audition (hear a sound asset in place), the mix graph's knobs, and the
editor integration are E-05's functional-editor-first territory — mapped
here only so the runtime design does not foreclose them (a mixer whose
parameters are data, not code, is the forward-compatible shape — the same
"schedules are data" instinct).

## 3. The layered picture

```text
  Spine events + render-side observation (deterministic, LOD-gated)
        │  the trigger projection (a READER — presentation never
        │  enters the spine)
        ▼
  Sound-trigger queue (bounded, lock-free handoff to the audio thread)
        ▼
  The mixer (owned): beds + one-shot voices, gain/pan/attenuation,
        allocation-free on the callback thread
        ▼
  SDL3 AudioStream layer (already vendored) ── logical devices,
        default-device migration, postmix ──► the OS audio stack
  Cook-time: Symphonia (build-only) ──► the engine's packed PCM format
```

## 4. The gates (any implementation's merge-blocking shape)

1. **The underrun gate**: zero underruns/xruns across warmed frames at the
   corner scene with the crowd's footsteps active (counted fail-visibly —
   the truncation-counter pattern; the dev pod has no audio device, so the
   gate degrades to the dummy/silent path there and executes on CI's
   device-bearing runners or the owner's rig — rig claims labeled pending).
2. **The callback-cost gate**: worst-case (p99) mix callback time under the
   populated scene, budgeted against the device period.
3. **The allocation gate**: the audio thread allocates nothing — enforced
   structurally (the counting allocator's thread attribution, the spine's
   existing machinery) and by inspection.
4. **The residency gate**: sound memory bounded and counted per class
   (beds/streams/one-shot pools), constant across warmed frames.
5. **The determinism-of-triggers gate**: the same campaign produces the
   same sound-trigger STREAM (digest-pinned, headless, no device needed —
   the pod's own gate).
6. **The degradation gate**: no audio device ⇒ typed silence, the frame
   path unaffected (the dummy-driver precedent).

## 5. The owner's decision list

1. **The device layer**: SDL3 audio (already vendored — the map's default)
   vs adding cpal for finer backend control. [OPEN]
2. **MPL-2.0 in the dependency tree**: Symphonia as the cook-time decoder —
   the workspace's first MPL dependency; lawful for commercial use as an
   unmodified crate, but V-05's discipline asks for an explicit GO.
   [OPEN]
3. **The mix layer**: owned small mixer (the map's default) vs a fetched
   and evaluated game-audio engine (kira-class — a follow-up research pass
   if the owner wants the comparison). [OPEN]
4. **The 3D audio ambition**: stereo pan + distance attenuation (game one's
   floor) vs HRTU/spatial-audio class later — a product ambition call.
   [OPEN]
5. **The first implementation package's scope**: the trigger projection +
   the owned mixer + SDL3 streams + WAV floor (the smallest complete
   vertical slice — the crowd's footsteps as the audible proof), gated by
   §4. [OPEN — the GO is the owner's]

## 6. Sources (fetched and read 2026-11-24)

- SDL3 audio: `https://wiki.libsdl.org/SDL3/CategoryAudio` — the
  SDL_AudioStream model (playback/recording, conversion, bound-stream
  mixing, callbacks, gain, frequency ratio, channel maps, postmix),
  logical devices with automatic default-device migration,
  `SDL_LoadWAV`, the channel-layout tables, the full function list.
- cpal: `https://github.com/RustAudio/cpal` — Apache-2.0; the
  platform/backend matrix (WASAPI/ASIO, ALSA/JACK/PipeWire/PulseAudio,
  CoreAudio, AAudio, WASM); MSRV 1.85; the `realtime`/`realtime-dbus`
  features; the Linux sound-server pitfalls and buffer-size disciplines.
- Symphonia: `https://github.com/pdeljanov/Symphonia` — MPL-2.0; pure
  safe Rust; MSRV 1.53 (the crate manifest's `rust-version` — corrected
  from this doc's first draft, which transposed cpal's 1.85); the codec/format status tables (FLAC/MP3/PCM/
  Vorbis/WAV Excellent, gapless); feature-flag codec selection; the
  fuzz/DoS safety aims; the ±15%-of-FFMpeg performance claim (theirs,
  [MEASURED-PENDING] if load-bearing).
- The engine's own facts [PROJECT EVIDENCE]: the vendored-static SDL 3.4.18
  (T-09) — the audio subsystem already compiled into every build; the
  shell's dummy-driver degradation precedent; the renderer's
  allocation-counter and truncation-counter gate patterns; the spine's
  event stream (the trigger source); the NPC AI design pass's perception
  layer (the shared event vocabulary).

*No borrowed benchmarks are carried as evidence — §4's gates are the
engine's own. Nothing here authorizes code or a dependency; the registry
owns every decision.*
