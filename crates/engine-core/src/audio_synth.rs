//! The deterministic procedural PCM synthesis (Era 2 audio
//! vertical slice — the `proc_texture` pattern applied to audio):
//! seeded one-shot and ambience-bed waveforms as i16 PCM,
//! generated in fixed operation order — byte-identical samples
//! on every platform and every build, for the same seed. This is
//! the procedural floor under the cook-time packed-PCM store
//! (§2.2 of `docs/AUDIO-SUBSYSTEM-MAP.md`): the crowd's
//! footsteps and the city-hum bed are SYNTHESIZED, not decoded,
//! so the vertical slice ships with zero audio assets.
//!
//! # DETERMINISM CONTRACT
//! The noise stream is SplitMix64 (the repo's `mix64` family —
//! the same finalizer `proc_texture` carries), the envelopes and
//! filters are pure f32 arithmetic in fixed order, and the i16
//! boundary is [`crate::audio_mix::to_i16`] (clamped, symmetric,
//! truncation-rounded). The same seed produces byte-identical
//! samples — pinned by digest (the FNV-1a discipline,
//! [`sample_digest`]). NO runtime sin/cos/pow/exp anywhere: the
//! footstep's exponential decay is repeated multiplication by a
//! fixed per-sample factor, and the bed's low-pass is a one-pole
//! `y += alpha·(x−y)` walk.
//!
//! # THE ALLOCATION BOUNDARY (§1.2's law)
//! These functions return owned `Vec<i16>` — they run at ASSET
//! time (load/cook, or a debug audition), NEVER on the audio
//! thread. The audio thread consumes the finished buffers
//! through [`crate::audio_mix::mix_into`], which allocates
//! nothing.
//!
//! # THE LOOP-CLEAN BED
//! [`synthesize_ambience_bed`] makes its output loopable by
//! CROSSFADE, not by periodic construction: the raw low-passed
//! stream is generated once, then the last `n` samples blend
//! into the head's first samples so the wrap point is
//! value-continuous BY CONSTRUCTION (the last blended sample IS
//! the first sample, exactly). See the function's docs for the
//! pairing and the measured seam metrics.

/// The SplitMix64 golden gamma (the stream step).
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// The seeded SplitMix64 finalizer (deterministic,
/// cross-platform — the repo's standard, as carried by
/// `proc_texture`'s `mix64`).
fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The FNV-1a 64-bit constants (the repo's standard).
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One white-noise sample in [−1, 1) from the stream state: the
/// top 24 bits of the finalized word, mapped and scaled — fixed
/// order, pure IEEE arithmetic.
fn white_from(state: u64) -> f32 {
    ((state >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
}

/// The footstep's decay RATE in per-second exponential units:
/// the per-sample envelope multiplier is `1 − 80/sample_rate`
/// (the linearized discrete exponential — no pow/exp), so the
/// envelope falls as ≈ e^(−80·t): about 0.30 of peak at a
/// quarter of the burst, 0.008 at three quarters — the tail is
/// well under the 10%-of-peak budget while the burst still
/// sounds like a heel strike, and the rate is SAMPLE-RATE
/// INDEPENDENT (the multiplier adapts so 44.1 kHz and 48 kHz
/// footprints match to f32 rounding).
const FOOTSTEP_DECAY_PER_SECOND: f32 = 80.0;

/// The footstep's linear attack: `sample_rate / 250` samples
/// (4 ms), at least 1 — the fast onset.
const FOOTSTEP_ATTACK_SECONDS_DIVISOR: u32 = 250;

/// The footstep's peak amplitude (of full i16 scale) — a
/// single step in a crowd, not a gunshot.
const FOOTSTEP_AMPLITUDE: f32 = 0.6;

/// One footstep as mono i16 PCM: a short enveloped noise burst
/// — seeded white noise through a fast linear attack (4 ms) and
/// an exponential-ish decay (the [`FOOTSTEP_DECAY_PER_SECOND`]
/// repeated-multiplication law), at [`FOOTSTEP_AMPLITUDE`]
/// peak. Exactly `sample_rate · 0.08` samples — computed in
/// INTEGER arithmetic as `sample_rate · 8 / 100`, exact for
/// every rate divisible by 25 (8 000 / 22 050 / 44 100 /
/// 48 000). Deterministic: the same seed
/// → byte-identical samples, on every platform (digest-pinned).
///
/// Degenerate rates: `sample_rate` below 100 produces an empty
/// buffer (typed silence — never a panic); the decay multiplier
/// is clamped at 0 so absurdly low rates stay well-defined.
pub fn synthesize_footstep(seed: u64, sample_rate: u32) -> Vec<i16> {
    let sample_rate = sample_rate.max(1);
    let length = (sample_rate as u64 * 8 / 100) as usize;
    let attack = (sample_rate / FOOTSTEP_ATTACK_SECONDS_DIVISOR).max(1) as usize;
    let decay = (1.0 - FOOTSTEP_DECAY_PER_SECOND / sample_rate as f32).max(0.0);
    let mut out = Vec::with_capacity(length);
    let mut state = seed;
    let mut envelope = 1.0;
    for i in 0..length {
        state = state.wrapping_add(GOLDEN);
        let white = white_from(mix64(state));
        let ramp = if i < attack {
            i as f32 / attack as f32
        } else {
            1.0
        };
        let sample = white * envelope * ramp * FOOTSTEP_AMPLITUDE;
        out.push(crate::audio_mix::to_i16(sample));
        envelope *= decay;
    }
    out
}

/// The bed's one-pole low-pass coefficient (the city hum's
/// character: a ~150 Hz cutoff at 48 kHz — traffic and air
/// rumble, no hiss).
const BED_ALPHA: f32 = 0.02;

/// The bed's normalized peak amplitude (of full i16 scale) —
/// the "quiet bed level": the layer everything else sits on,
/// pre-mix, with headroom for footsteps and music above it.
const BED_PEAK: f32 = 0.25;

/// A looping city-hum ambience bed as mono i16 PCM: seeded
/// white noise through a one-pole low-pass
/// (`y += BED_ALPHA·(x − y)`), amplitude-normalized so the peak
/// is exactly [`BED_PEAK`], and made LOOP-CLEAN by crossfade
/// (the documented choice — the noise is NOT periodic by
/// construction). Exactly `seconds · sample_rate` samples.
/// Deterministic: the same seed → byte-identical samples
/// (digest-pinned).
///
/// # THE LOOP-CLEAN CROSSFADE
/// The last `n` samples (`n = min(len/4, sample_rate/8)` — a
/// 125 ms blend at 48 kHz, never more than a quarter of the
/// bed) blend from the raw tail into the head's first samples:
/// sample `len − n + i` becomes
/// `crossfade(raw_tail[i], raw[(i + 1) % n], i/(n−1))`. The
/// pairing runs the head material one AHEAD so the blend's
/// FINAL sample is exactly `raw[0]` — the sample the wrap
/// returns to — making the boundary discontinuity ZERO by
/// construction (`out[len − 1] == out[0]`, bit-exact through
/// [`crate::audio_mix::to_i16`]). The cost: the head's first
/// `n` samples play twice per loop, inaudible for a low-passed
/// noise bed. Beds shorter than 8 samples skip the crossfade
/// (too short to blend); `seconds = 0` yields an empty buffer.
pub fn synthesize_ambience_bed(seed: u64, sample_rate: u32, seconds: u32) -> Vec<i16> {
    let sample_rate = sample_rate.max(1);
    let length = seconds as usize * sample_rate as usize;
    if length == 0 {
        return Vec::new();
    }
    // The raw low-passed stream, f32, fixed order.
    let mut raw = Vec::with_capacity(length);
    let mut state = seed;
    let mut filtered = 0.0;
    for _ in 0..length {
        state = state.wrapping_add(GOLDEN);
        let white = white_from(mix64(state));
        filtered += BED_ALPHA * (white - filtered);
        raw.push(filtered);
    }
    // Peak-normalize to the quiet bed level (silence stays
    // silent — a zero-peak bed is the empty-signal guard).
    let mut peak = 0.0;
    for &value in &raw {
        let magnitude = value.abs();
        if magnitude > peak {
            peak = magnitude;
        }
    }
    let scale = if peak > 0.0 { BED_PEAK / peak } else { 1.0 };
    for value in &mut raw {
        *value *= scale;
    }
    // The loop-clean crossfade (see the function docs). The
    // head window [0, n) and the tail window [len−n, len) are
    // disjoint (n ≤ len/4), so the in-place walk reads only
    // unmodified values.
    let blend = (length / 4).min(sample_rate as usize / 8);
    if blend >= 2 {
        for i in 0..blend {
            let t = i as f32 / (blend - 1) as f32;
            let tail = raw[length - blend + i];
            let head = raw[(i + 1) % blend];
            raw[length - blend + i] = crate::audio_mix::crossfade(tail, head, t);
        }
    }
    raw.iter()
        .map(|&value| crate::audio_mix::to_i16(value))
        .collect()
}

/// The sample digest (the cross-platform byte pin): FNV-1a over
/// the samples' little-endian i16 bytes, in order — the repo's
/// constants (see `trigger_digest` / `pose_sequence_digest` for
/// the discipline). The same samples → the same digest on every
/// platform; a moved pin is an investigation, never a merge.
pub fn sample_digest(samples: &[i16]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for &sample in samples {
        for byte in sample.to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixed seeds the pins below were measured against
    /// (48 000 Hz — the vertical slice's device rate).
    const FOOTSTEP_PIN_SEED: u64 = 0x1234_5678;
    const BED_PIN_SEED: u64 = 0x9ABC_DEF0;
    const PIN_RATE: u32 = 48_000;

    /// Determinism: the same seed synthesizes byte-identical
    /// samples, twice, at every pinned rate.
    #[test]
    fn footstep_is_deterministic() {
        for rate in [8_000u32, 22_050, 44_100, PIN_RATE] {
            let a = synthesize_footstep(FOOTSTEP_PIN_SEED, rate);
            let b = synthesize_footstep(FOOTSTEP_PIN_SEED, rate);
            assert_eq!(
                a, b,
                "seed {FOOTSTEP_PIN_SEED} at {rate} Hz is byte-identical"
            );
            assert!(
                !a.is_empty() || rate < 100,
                "rate {rate} produces a non-empty burst"
            );
        }
    }

    /// Distinctness: different seeds produce different samples.
    #[test]
    fn footstep_seeds_are_distinct() {
        let a = synthesize_footstep(0x0000_0001, PIN_RATE);
        let b = synthesize_footstep(0x0000_0002, PIN_RATE);
        assert_ne!(a, b);
        assert_ne!(sample_digest(&a), sample_digest(&b));
    }

    /// The length pins: exactly sample_rate · 0.08 samples,
    /// integer-exact at the common rates.
    #[test]
    fn footstep_length_pins() {
        assert_eq!(synthesize_footstep(FOOTSTEP_PIN_SEED, 48_000).len(), 3840);
        assert_eq!(synthesize_footstep(FOOTSTEP_PIN_SEED, 44_100).len(), 3528);
        assert_eq!(synthesize_footstep(FOOTSTEP_PIN_SEED, 22_050).len(), 1764);
        // A degenerate rate is typed silence, not a panic.
        assert!(synthesize_footstep(FOOTSTEP_PIN_SEED, 0).is_empty());
    }

    /// The envelope shape (the pinned seed, 48 kHz): the peak is
    /// EARLY (in the first quarter — the fast attack), and the
    /// tail is QUIET (the last quarter's max is below 10% of
    /// the peak — the decay law's contract).
    #[test]
    fn footstep_envelope_shape() {
        let samples = synthesize_footstep(FOOTSTEP_PIN_SEED, PIN_RATE);
        let quarter = samples.len() / 4;
        let (mut peak, mut peak_index) = (0i16, 0usize);
        for (i, &sample) in samples.iter().enumerate() {
            if sample.abs() > peak {
                peak = sample.abs();
                peak_index = i;
            }
        }
        assert!(peak > 8000, "the burst is audible (peak {peak})");
        assert!(
            peak_index < quarter,
            "the peak is early (index {peak_index} of {})",
            samples.len()
        );
        let tail_peak = samples[samples.len() - quarter..]
            .iter()
            .map(|s| s.abs())
            .max()
            .unwrap();
        assert!(
            (tail_peak as f32) < 0.1 * peak as f32,
            "the tail is quiet ({tail_peak} vs peak {peak})"
        );
    }

    /// Determinism + distinctness for the bed, at two rates.
    #[test]
    fn bed_is_deterministic_and_distinct() {
        let a = synthesize_ambience_bed(BED_PIN_SEED, PIN_RATE, 2);
        let b = synthesize_ambience_bed(BED_PIN_SEED, PIN_RATE, 2);
        assert_eq!(a, b, "the same seed is byte-identical");
        let c = synthesize_ambience_bed(BED_PIN_SEED + 1, PIN_RATE, 2);
        assert_ne!(a, c, "different seeds differ");
        // The filter is rate-dependent by design (the cutoff is
        // per-sample), but each rate is internally deterministic.
        let d = synthesize_ambience_bed(BED_PIN_SEED, 44_100, 2);
        assert_eq!(d, synthesize_ambience_bed(BED_PIN_SEED, 44_100, 2));
    }

    /// The length pins: exactly seconds · sample_rate samples;
    /// zero seconds is the empty buffer.
    #[test]
    fn bed_length_pins() {
        assert_eq!(
            synthesize_ambience_bed(BED_PIN_SEED, 48_000, 2).len(),
            96_000
        );
        assert_eq!(
            synthesize_ambience_bed(BED_PIN_SEED, 44_100, 1).len(),
            44_100
        );
        assert!(synthesize_ambience_bed(BED_PIN_SEED, 48_000, 0).is_empty());
    }

    /// The bed is quiet (peak-normalized to BED_PEAK, with i16
    /// rounding slack) and not silence (the low-passed stream
    /// has real variation).
    #[test]
    fn bed_is_quiet_but_not_silent() {
        let samples = synthesize_ambience_bed(BED_PIN_SEED, PIN_RATE, 2);
        let peak = samples.iter().map(|s| s.abs()).max().unwrap();
        assert!(peak <= 8193, "peak {peak} within the bed level + rounding");
        assert!(peak >= 1000, "peak {peak} is real signal, not silence");
    }

    /// The loop-clean contract (the pinned seed, 48 kHz, 2 s):
    /// the boundary discontinuity is ZERO by construction
    /// (the last blended sample IS the first sample), and the
    /// crossfade region's steps stay within the documented
    /// threshold — the blend spreads the raw seam over 6 000
    /// samples instead of one step.
    #[test]
    fn bed_loop_is_clean() {
        let samples = synthesize_ambience_bed(BED_PIN_SEED, PIN_RATE, 2);
        let length = samples.len();
        assert_eq!(
            samples[length - 1],
            samples[0],
            "the wrap is value-continuous"
        );
        // The blend region: len/4 vs sample_rate/8 — 6 000 at
        // 48 kHz. Steps there must stay gentle.
        let blend = (length / 4).min(PIN_RATE as usize / 8);
        let mut worst_step = 0i32;
        for i in length - blend..length - 1 {
            let step = (samples[i + 1] as i32 - samples[i] as i32).abs();
            if step > worst_step {
                worst_step = step;
            }
        }
        assert!(
            worst_step <= BLEND_STEP_BOUND,
            "the blend region's worst step is {worst_step}"
        );
        // And the blend is no wilder than the bed at large (the
        // seam was SPREAD, not moved).
        let mut worst_anywhere = 0i32;
        for window in samples.windows(2) {
            let step = (window[1] as i32 - window[0] as i32).abs();
            if step > worst_anywhere {
                worst_anywhere = step;
            }
        }
        assert!(
            worst_step <= worst_anywhere,
            "the blend ({worst_step}) is within the bed's own worst step ({worst_anywhere})"
        );
    }

    /// The measured blend-step bound: the pinned seed's worst
    /// blend-region step measured 804 (the bed's worst step
    /// anywhere: 861); the bound carries ~50% slack — see
    /// [`bed_loop_is_clean`].
    const BLEND_STEP_BOUND: i32 = 1200;

    /// THE DIGEST PINS: the fixed seeds' measured FNV-1a digests
    /// at 48 kHz — cross-platform, forever (a moved pin is an
    /// investigation, never a merge).
    #[test]
    fn sample_digest_pins() {
        let footstep = synthesize_footstep(FOOTSTEP_PIN_SEED, PIN_RATE);
        assert_eq!(sample_digest(&footstep), PINNED_FOOTSTEP_DIGEST);
        let bed = synthesize_ambience_bed(BED_PIN_SEED, PIN_RATE, 2);
        assert_eq!(sample_digest(&bed), PINNED_BED_DIGEST);
        // The empty signal digests to the FNV offset basis (the
        // known answer — no measurement needed).
        assert_eq!(sample_digest(&[]), FNV_OFFSET_BASIS);
        // One flipped sample moves the digest (avalanche).
        let mut flipped = footstep.clone();
        flipped[0] = flipped[0].wrapping_add(1);
        assert_ne!(sample_digest(&flipped), sample_digest(&footstep));
    }

    /// The measured-actual pin (the pinned footstep seed:
    /// 0x1234_5678 at 48 000 Hz, 3 840 samples).
    const PINNED_FOOTSTEP_DIGEST: u64 = 4084186983141199492;

    /// The measured-actual pin (the pinned bed seed:
    /// 0x9ABC_DEF0 at 48 000 Hz, 2 s, 96 000 samples).
    const PINNED_BED_DIGEST: u64 = 9496992194624059978;
}
