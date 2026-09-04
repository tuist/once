//! The sound of your build.
//!
//! With `--sound`, `once` runs a continuous evolving cinematic bed while a
//! command works, and layers a distinct gesture on top for each event. The
//! bed is not a fixed chord: it walks a slow I - V - vi - IV progression, a
//! quiet arpeggio picks through the current chord, and five LFOs at
//! incommensurate rates keep the timbre drifting so nothing ever repeats
//! exactly. Events do more than trigger a hit — each one persistently
//! reshapes the pad through four dimensions that decay slowly:
//! `brightness` (raised by cache hits), `warmth` (raised by fresh action
//! executions), `depth` (raised by failures), and `density` (raised by any
//! activity; also speeds the arpeggio). The result is that a run's audible
//! shape follows the shape of the work: a cache-rich build glows and rings,
//! a build full of fresh work grows warm and thick, and a failing build
//! sinks low.
//!
//! When the whole command finishes, the pad does not release immediately:
//! the closing BRAAAM and chord swell in while the bed holds for ~2s, then
//! everything releases together over ~4s. That way the ending feels like a
//! resolution rather than a cut.
//!
//! Off by default; a headless audio backend degrades to a silent no-op.

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rodio::source::Source;

// -------- Frequencies (Hz), equal-tempered ----------------------------

const C1: f32 = 32.70;
const C2: f32 = 65.41;
const F2: f32 = 87.31;
const G2: f32 = 98.00;
const A2: f32 = 110.00;
const B2: f32 = 123.47;
const C3: f32 = 130.81;
const D3: f32 = 146.83;
const E3: f32 = 164.81;
const G3: f32 = 196.00;
const A3: f32 = 220.00;
const C5: f32 = 523.25;
const E5: f32 = 659.25;
const G5: f32 = 783.99;
const C6: f32 = 1046.50;

// -------- Levels and timing --------------------------------------------

const OUTPUT_GAIN: f32 = 0.22;

/// Absolute cap on how long the audio thread will wait for the synth to
/// finish releasing before it gives up and lets the stream drop. In practice
/// the wait is bounded by `MASTER_RELEASE_SECONDS` plus the exit threshold
/// below, not by this limit; the cap only exists so a stuck audio backend
/// cannot stall CLI shutdown forever.
const RELEASE_LIMIT: Duration = Duration::from_secs(30);

/// Extra grace after the synth signals it is done, so buffered samples reach
/// the speakers before the output stream is dropped.
const POST_FINISH_GRACE: Duration = Duration::from_millis(250);

const MASTER_ATTACK_SECONDS: f32 = 1.8;
/// Release time constant. The envelope decays exponentially toward zero
/// with this half-time. Combined with `MASTER_EXIT_THRESHOLD` and the "tie
/// gestures to master" behavior below, the whole ending fades in about
/// 4-5 × this value and then cleanly stops on its own.
const MASTER_RELEASE_SECONDS: f32 = 4.0;

/// Amplitude below which we consider the pad inaudible; the synth returns
/// `None` at this point and the audio thread stops. Roughly -42 dB, well
/// under any speaker's audible floor at listening volume.
const MASTER_EXIT_THRESHOLD: f32 = 8e-3;

/// Small grace at the start of the closing phase so the swelling BRAAAM /
/// bell chord has room to begin before the pad starts descending. Short on
/// purpose — the slow release does most of the "hold" work implicitly.
const CLOSING_HOLD_SECONDS: f32 = 1.0;

// -------- Control-state enums (encoded as u8 in atomics) ---------------

const MASTER_OFF: u8 = 0;
const MASTER_SUSTAIN: u8 = 1;
const MASTER_RELEASE: u8 = 2;
const MASTER_CLOSING: u8 = 3;

const CHORD_MAJOR: u8 = 0;
const CHORD_MINOR: u8 = 1;

const PULSE_SHIMMER: u8 = 0;
const PULSE_WARM: u8 = 1;
const PULSE_LOW: u8 = 2;
const PULSE_RESOLVE: u8 = 3;

const GESTURE_SLOTS: usize = 12;

// -------- Chord progression -------------------------------------------

/// Root frequencies used to build the mid-pad chord positions. Each build
/// draws one of these four progressions from its seed, so different targets
/// each hear their own harmonic movement. All four are in C major so they
/// stay tonal after the seed's optional pitch transposition is applied.
const PROGRESSIONS_MAJOR: [[[f32; 3]; 4]; 4] = [
    // I - V - vi - IV (pop / cinematic default)
    [[C3, E3, G3], [G2, B2, D3], [A2, C3, E3], [F2, A2, C3]],
    // I - vi - IV - V (50s doo-wop, still uplifting)
    [[C3, E3, G3], [A2, C3, E3], [F2, A2, C3], [G2, B2, D3]],
    // ii - V - I - vi (jazz-adjacent, more motion)
    [[D3, F2, A2], [G2, B2, D3], [C3, E3, G3], [A2, C3, E3]],
    // I - IV - vi - V (bright, moving)
    [[C3, E3, G3], [F2, A2, C3], [A2, C3, E3], [G2, B2, D3]],
];

/// Minor mode: one progression each variant, mirroring the major set so a
/// build that has a "flavor" keeps a consistent voice across success and
/// failure runs.
const PROGRESSIONS_MINOR: [[[f32; 3]; 4]; 4] = [
    [[A2, C3, E3], [E3, G3, B2], [F2, A3, C3], [G2, B2, D3]],
    [[A2, C3, E3], [F2, A2, C3], [G2, B2, D3], [E3, G3, B2]],
    [[D3, F2, A2], [A2, C3, E3], [E3, G3, B2], [F2, A2, C3]],
    [[A2, C3, E3], [D3, F2, A2], [F2, A3, C3], [E3, G3, B2]],
];

const PROGRESSION_STEP_SECONDS: f32 = 11.0;

fn progression_table(variant: u8, chord_kind: u8) -> &'static [[f32; 3]; 4] {
    let idx = (variant as usize) % PROGRESSIONS_MAJOR.len();
    if chord_kind == CHORD_MINOR {
        &PROGRESSIONS_MINOR[idx]
    } else {
        &PROGRESSIONS_MAJOR[idx]
    }
}

fn transposed_chord(chord: &[f32; 3], multiplier: f32) -> [f32; 3] {
    [chord[0] * multiplier, chord[1] * multiplier, chord[2] * multiplier]
}

// -------- Arpeggiator --------------------------------------------------

/// Arpeggio period at rest vs when density is at 1.0. Density is bumped by
/// every action event and decays, so a busy build audibly quickens.
const ARP_PERIOD_SECONDS_IDLE: f32 = 3.2;
const ARP_PERIOD_SECONDS_ACTIVE: f32 = 1.4;
const ARP_NOTE_DECAY_SECONDS: f32 = 2.4;
/// Pattern indices into the current chord — steps 0, 1, 2, 1 give a nicer
/// non-monotonic motion than a straight ascending sweep.
const ARP_PATTERN: [usize; 4] = [0, 1, 2, 1];

// -------- Dimension memory --------------------------------------------

/// Half-lives for the four persistent dimensions. Bumped by events, decays
/// exponentially to zero. Longer decay means the pad remembers activity for
/// longer.
const BRIGHTNESS_DECAY_SECONDS: f32 = 18.0;
const WARMTH_DECAY_SECONDS: f32 = 22.0;
const DEPTH_DECAY_SECONDS: f32 = 14.0;
const DENSITY_DECAY_SECONDS: f32 = 16.0;

// -------- Public API ---------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Started,
    ActionCacheHit,
    ActionExecuted,
    ActionFailed,
    Finished,
    Failed,
}

static PLAYER: OnceLock<Mutex<Option<SoundPlayer>>> = OnceLock::new();

pub fn init(enabled: bool) {
    let slot = PLAYER.get_or_init(|| Mutex::new(None));
    if !enabled {
        return;
    }
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.is_none() {
        *guard = Some(SoundPlayer::start());
    }
}

pub fn emit(event: Event) {
    let Some(slot) = PLAYER.get() else { return };
    let guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(player) = guard.as_ref() {
        player.emit(event);
    }
}

/// Seed the synth's initial parameters (root, progression, arpeggio tempo,
/// shimmer register) from a build fingerprint. Call once, from `dispatch`,
/// before the `Started` event fires. No-op when the sink is not installed;
/// no-op if called after the synth has already picked up an earlier seed.
pub fn seed(seed: u64) {
    let Some(slot) = PLAYER.get() else { return };
    let guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(player) = guard.as_ref() {
        player.seed(seed);
    }
}

pub fn wait_for_tail() {
    let Some(slot) = PLAYER.get() else { return };
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(player) = guard.take() {
        player.stop_and_join();
    }
}

// -------- SoundPlayer wrapping the audio thread -----------------------

struct SoundPlayer {
    tx: Option<mpsc::Sender<Event>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<SynthShared>,
}

impl SoundPlayer {
    fn start() -> Self {
        let (tx, rx) = mpsc::channel::<Event>();
        let shared = Arc::new(SynthShared::default());
        let audio_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("once-sound".to_string())
            .spawn(move || audio_loop(&rx, audio_shared))
            .ok();
        Self {
            tx: Some(tx),
            thread,
            shared,
        }
    }

    fn emit(&self, event: Event) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(event);
        }
    }

    fn seed(&self, seed: u64) {
        self.shared.seed.store(seed, Ordering::Relaxed);
    }

    fn stop_and_join(mut self) {
        drop(self.tx.take());
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

// -------- Shared control surface (atomic; cheap to read per sample) ---

#[derive(Default)]
struct SynthShared {
    master_target: AtomicU8,
    chord_kind: AtomicU8,
    pulse_seq: AtomicU32,
    pulse_kind: AtomicU8,
    /// Set by the synth when it has decayed to inaudibility. The audio
    /// thread polls this to decide when it is safe to drop the output
    /// stream, so the tail always plays to completion instead of getting
    /// cut off by a fixed timeout.
    finished: AtomicBool,
    /// A build fingerprint the synth picks up when it transitions from OFF
    /// to SUSTAIN. Bits select the root of the chord progression, the
    /// progression variant, the arpeggio's base tempo, and the shimmer
    /// register. Same command → same seed → same musical identity every
    /// time; different commands each get their own piece.
    seed: AtomicU64,
}

fn audio_loop(events: &mpsc::Receiver<Event>, shared: Arc<SynthShared>) {
    let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
        drain(events);
        return;
    };
    let sample_rate = 48_000;
    let synth = AmbientSynth::new(Arc::clone(&shared), sample_rate);
    if handle.play_raw(synth).is_err() {
        drain(events);
        return;
    }
    while let Ok(event) = events.recv() {
        apply_event(&shared, event);
    }
    // If nobody has already asked us to close or release, treat the CLI
    // exiting as an implicit release. But do not stomp on a closing hold
    // that is already in progress — that hold's whole job is to give the
    // finish gesture room to breathe.
    let current = shared.master_target.load(Ordering::Relaxed);
    if current == MASTER_SUSTAIN || current == MASTER_OFF {
        shared.master_target.store(MASTER_RELEASE, Ordering::Relaxed);
    }
    // Wait for the synth to signal it has fully decayed to inaudibility,
    // rather than sleeping for a fixed duration and then dropping the
    // output stream on top of a still-ringing tail. Capped by
    // `RELEASE_LIMIT` so a stuck backend cannot stall shutdown forever.
    let deadline = Instant::now() + RELEASE_LIMIT;
    while !shared.finished.load(Ordering::Relaxed) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    // Extra grace so the last buffered samples reach the device before we
    // let `_stream` drop and take the mixer with it.
    std::thread::sleep(POST_FINISH_GRACE);
}

fn drain(events: &mpsc::Receiver<Event>) {
    while events.recv().is_ok() {}
}

fn apply_event(shared: &SynthShared, event: Event) {
    match event {
        Event::Started => {
            shared.chord_kind.store(CHORD_MAJOR, Ordering::Relaxed);
            shared.master_target.store(MASTER_SUSTAIN, Ordering::Relaxed);
        }
        Event::ActionExecuted => trigger_pulse(shared, PULSE_SHIMMER),
        Event::ActionCacheHit => trigger_pulse(shared, PULSE_WARM),
        Event::ActionFailed => trigger_pulse(shared, PULSE_LOW),
        Event::Finished => {
            shared.chord_kind.store(CHORD_MAJOR, Ordering::Relaxed);
            trigger_pulse(shared, PULSE_RESOLVE);
            shared.master_target.store(MASTER_CLOSING, Ordering::Relaxed);
        }
        Event::Failed => {
            shared.chord_kind.store(CHORD_MINOR, Ordering::Relaxed);
            trigger_pulse(shared, PULSE_LOW);
            shared.master_target.store(MASTER_CLOSING, Ordering::Relaxed);
        }
    }
}

fn trigger_pulse(shared: &SynthShared, kind: u8) {
    shared.pulse_kind.store(kind, Ordering::Relaxed);
    shared.pulse_seq.fetch_add(1, Ordering::Relaxed);
}

// -------- Gestures (transient cinematic voices) -----------------------

/// A single evolving voice: pitch that bends toward a target, an amplitude
/// envelope that ramps toward `peak` then decays exponentially, and up to
/// four summed harmonics plus a slightly detuned copy so the timbre reads as
/// brass or bell rather than as a pure sine. Voices that need a punchy hit
/// start with `amp == peak` and a tiny attack constant; voices that need to
/// swell (the closing BRAAAM, closing chord) start at `amp == 0` and use a
/// slower attack so the finish emerges rather than lands.
#[derive(Clone, Copy)]
struct Gesture {
    phase: f32,
    phase_detune: f32,
    freq: f32,
    freq_target: f32,
    freq_slew: f32,
    amp: f32,
    peak: f32,
    attack: f32,
    decay: f32,
    detune_ratio: f32,
    harmonic_gain: [f32; 4],
}

impl Gesture {
    const fn silent() -> Self {
        Self {
            phase: 0.0,
            phase_detune: 0.0,
            freq: 0.0,
            freq_target: 0.0,
            freq_slew: 0.0,
            amp: 0.0,
            peak: 0.0,
            attack: 1.0,
            decay: 1.0,
            detune_ratio: 1.0,
            harmonic_gain: [0.0; 4],
        }
    }
}

fn punchy(peak: f32, sample_rate: u32) -> (f32, f32, f32) {
    (0.0, peak, slew_coeff(sample_rate, 0.005))
}

fn swelling(peak: f32, sample_rate: u32, seconds: f32) -> (f32, f32, f32) {
    (0.0, peak, slew_coeff(sample_rate, seconds))
}

fn brass_stab(freq: f32, sample_rate: u32) -> Gesture {
    let (amp, peak, attack) = punchy(0.55, sample_rate);
    Gesture {
        phase: 0.0,
        phase_detune: 0.0,
        freq: freq * 1.015,
        freq_target: freq,
        freq_slew: slew_coeff(sample_rate, 0.25),
        amp,
        peak,
        attack,
        decay: slew_decay(sample_rate, 0.9),
        detune_ratio: 1.008,
        harmonic_gain: [0.55, 0.30, 0.18, 0.09],
    }
}

fn bell(freq: f32, sample_rate: u32) -> Gesture {
    let (amp, peak, attack) = punchy(0.42, sample_rate);
    Gesture {
        phase: 0.0,
        phase_detune: 0.0,
        freq,
        freq_target: freq,
        freq_slew: 0.0,
        amp,
        peak,
        attack,
        decay: slew_decay(sample_rate, 5.5),
        detune_ratio: 1.004,
        harmonic_gain: [0.45, 0.12, 0.30, 0.18],
    }
}

fn sparkle(freq: f32, sample_rate: u32) -> Gesture {
    let (amp, peak, attack) = punchy(0.22, sample_rate);
    Gesture {
        phase: 0.0,
        phase_detune: 0.0,
        freq,
        freq_target: freq,
        freq_slew: 0.0,
        amp,
        peak,
        attack,
        decay: slew_decay(sample_rate, 1.8),
        detune_ratio: 1.002,
        harmonic_gain: [0.55, 0.0, 0.12, 0.0],
    }
}

fn low_thump(freq: f32, sample_rate: u32) -> Gesture {
    let (amp, peak, attack) = punchy(0.6, sample_rate);
    Gesture {
        phase: 0.0,
        phase_detune: 0.0,
        freq: freq * 1.02,
        freq_target: freq,
        freq_slew: slew_coeff(sample_rate, 0.12),
        amp,
        peak,
        attack,
        decay: slew_decay(sample_rate, 0.7),
        detune_ratio: 1.010,
        harmonic_gain: [0.60, 0.35, 0.10, 0.05],
    }
}

/// BRAAAM: the deep low brass hit at the end. Swells in slowly and its tail
/// keeps ringing well past the master release, so the pad fades into the
/// BRAAAM rather than dropping out from under it.
fn braaam(freq: f32, sample_rate: u32) -> Gesture {
    let (amp, peak, attack) = swelling(0.72, sample_rate, 2.2);
    Gesture {
        phase: 0.0,
        phase_detune: 0.0,
        freq: freq * 1.02,
        freq_target: freq,
        freq_slew: slew_coeff(sample_rate, 1.2),
        amp,
        peak,
        attack,
        decay: slew_decay(sample_rate, 6.5),
        detune_ratio: 1.006,
        harmonic_gain: [0.55, 0.35, 0.20, 0.10],
    }
}

fn brass_chord(freq: f32, sample_rate: u32) -> Gesture {
    let (amp, peak, attack) = swelling(0.35, sample_rate, 1.8);
    Gesture {
        phase: 0.0,
        phase_detune: 0.0,
        freq,
        freq_target: freq,
        freq_slew: 0.0,
        amp,
        peak,
        attack,
        decay: slew_decay(sample_rate, 6.5),
        detune_ratio: 1.006,
        harmonic_gain: [0.50, 0.25, 0.15, 0.08],
    }
}

// -------- AmbientSynth (the source rodio plays continuously) ----------

struct AmbientSynth {
    shared: Arc<SynthShared>,
    sample_rate: u32,

    // Master envelope state.
    master_level: f32,
    closing_hold_samples_left: u32,
    closing_primed: bool,

    // Continuous drone oscillator phases.
    drone_phase: [f32; 4],

    // Mid pad chord oscillator phases; chord tones glide toward `chord_target`.
    chord_phase: [f32; 3],
    chord_freq: [f32; 3],
    chord_target: [f32; 3],

    // Slow chord progression state.
    progression_index: usize,
    progression_samples_left: u32,

    // Shimmer oscillator (high, airy) and its independent slow LFOs.
    shimmer_phase: f32,
    shimmer_lfo_phase: f32,

    // Arpeggiator (quiet melodic layer stepping through chord tones an
    // octave above the mid pad).
    arp_phase: f32,
    arp_freq: f32,
    arp_amp: f32,
    arp_pattern_step: usize,
    arp_samples_left: u32,

    // Free-running LFOs at incommensurate rates. Their interference is what
    // keeps the pad from ever repeating exactly the same texture.
    breath_lfo_phase: f32,        // 0.08 Hz — drone amplitude
    mid_lfo_phase: f32,           // 0.055 Hz — mid pad amplitude
    shimmer_amp_lfo_phase: f32,   // 0.19 Hz — shimmer amplitude
    detune_lfo_phase: f32,        // 0.028 Hz — drone detune
    wow_lfo_phase: f32,           // 0.4 Hz — tape-like pitch wow on drone

    // Persistent dimensions: each event bumps one or more of these, and they
    // decay slowly. They reshape the pad's overall character over the run.
    brightness: f32,
    warmth: f32,
    depth: f32,
    density: f32,

    // Gesture ring.
    gestures: [Gesture; GESTURE_SLOTS],
    next_gesture: usize,
    last_pulse_seq: u32,

    // Time (in samples, saturating) since the last cache-hit bell chord
    // rang. Used to detect the fast-cache-hit-then-finish path and skip the
    // BRAAAM in that case, so the reward bells actually get to be the
    // resolution instead of being buried by a booming close.
    samples_since_cache_hit: u32,

    // Build fingerprint applied on the SUSTAIN transition. `pitch_multiplier`
    // transposes every oscillator by a whole tonic-preserving interval;
    // `progression_variant` picks one of a handful of chord walks;
    // `arp_period_idle_seconds` shifts the arpeggio's base tempo;
    // `shimmer_root` picks C5 vs G5 for the airy top layer.
    seed_applied: bool,
    pitch_multiplier: f32,
    progression_variant: u8,
    arp_period_idle_seconds: f32,
    shimmer_root: f32,
}

impl AmbientSynth {
    fn new(shared: Arc<SynthShared>, sample_rate: u32) -> Self {
        let last_pulse_seq = shared.pulse_seq.load(Ordering::Relaxed);
        let initial_chord = PROGRESSIONS_MAJOR[0][0];
        Self {
            shared,
            sample_rate,
            master_level: 0.0,
            closing_hold_samples_left: 0,
            closing_primed: false,
            drone_phase: [0.0; 4],
            chord_phase: [0.0; 3],
            chord_freq: initial_chord,
            chord_target: initial_chord,
            progression_index: 0,
            progression_samples_left: samples_for(sample_rate, PROGRESSION_STEP_SECONDS),
            shimmer_phase: 0.0,
            shimmer_lfo_phase: 0.0,
            arp_phase: 0.0,
            arp_freq: initial_chord[0] * 2.0,
            arp_amp: 0.0,
            arp_pattern_step: 0,
            arp_samples_left: samples_for(sample_rate, ARP_PERIOD_SECONDS_IDLE),
            breath_lfo_phase: 0.0,
            mid_lfo_phase: 0.0,
            shimmer_amp_lfo_phase: 0.0,
            detune_lfo_phase: 0.0,
            wow_lfo_phase: 0.0,
            brightness: 0.0,
            warmth: 0.0,
            depth: 0.0,
            density: 0.0,
            gestures: [Gesture::silent(); GESTURE_SLOTS],
            next_gesture: 0,
            last_pulse_seq,
            samples_since_cache_hit: u32::MAX,
            seed_applied: false,
            pitch_multiplier: 1.0,
            progression_variant: 0,
            arp_period_idle_seconds: ARP_PERIOD_SECONDS_IDLE,
            shimmer_root: C5,
        }
    }

    /// Apply the shared seed on the first transition into SUSTAIN. Bits are
    /// carved up so each parameter draws from an independent slice: uniform
    /// mixing across builds without collapsing to a single "flavor".
    fn apply_seed(&mut self) {
        let seed = self.shared.seed.load(Ordering::Relaxed);
        // Bits 0-2: root offset — one of the tonally friendly degrees of C
        // major (I, ii, iii, IV, V) as semitone shifts.
        const ROOT_STEPS: [u32; 5] = [0, 2, 4, 5, 7];
        let root_semis = ROOT_STEPS[(seed & 0x7) as usize % ROOT_STEPS.len()];
        self.pitch_multiplier = 2f32.powf(root_semis as f32 / 12.0);
        // Bits 3-4: progression variant.
        self.progression_variant = ((seed >> 3) & 0x3) as u8;
        // Bits 5-10: arpeggio period, 2.4 - 4.0 seconds.
        let arp_bits = ((seed >> 5) & 0x3F) as f32 / 63.0;
        self.arp_period_idle_seconds = 2.4 + arp_bits * 1.6;
        // Bit 11: shimmer register.
        self.shimmer_root = if (seed >> 11) & 0x1 == 0 { C5 } else { G5 };

        // Reseed the initial chord and arp so we don't need to wait a full
        // progression step for the seed to take effect.
        let table = progression_table(self.progression_variant, CHORD_MAJOR);
        let initial = transposed_chord(&table[0], self.pitch_multiplier);
        self.chord_freq = initial;
        self.chord_target = initial;
        self.progression_samples_left = samples_for(self.sample_rate, PROGRESSION_STEP_SECONDS);
        self.progression_index = 0;
        self.arp_freq = initial[0] * 2.0;
    }

    fn read_controls(&mut self) -> (u8, u8) {
        let target = self.shared.master_target.load(Ordering::Relaxed);
        let chord = self.shared.chord_kind.load(Ordering::Relaxed);
        let pulse_seq = self.shared.pulse_seq.load(Ordering::Relaxed);
        if pulse_seq != self.last_pulse_seq {
            self.last_pulse_seq = pulse_seq;
            let kind = self.shared.pulse_kind.load(Ordering::Relaxed);
            self.spawn_for_pulse(kind);
        }
        (target, chord)
    }

    fn spawn_for_pulse(&mut self, kind: u8) {
        // Every event also bumps the persistent dimensions that shape the
        // pad, so the sound remembers what has been happening.
        self.density = (self.density + 0.10).min(1.0);
        match kind {
            PULSE_WARM => {
                // Cache hit: the moment worth celebrating. A small bright
                // bell chord plus a high sparkle, and a big brightness bump
                // so subsequent seconds ring on top of a shinier pad.
                self.spawn(bell(C5, self.sample_rate));
                self.spawn(bell(E5, self.sample_rate));
                self.spawn(bell(G5, self.sample_rate));
                self.spawn(sparkle(C6, self.sample_rate));
                self.brightness = (self.brightness + 0.35).min(1.0);
                self.warmth = (self.warmth + 0.05).min(1.0);
                self.samples_since_cache_hit = 0;
            }
            PULSE_SHIMMER => {
                // Fresh action: a mid brass stab, warms the pad.
                self.spawn(brass_stab(D3, self.sample_rate));
                self.warmth = (self.warmth + 0.25).min(1.0);
                self.brightness = (self.brightness + 0.05).min(1.0);
            }
            PULSE_LOW => {
                // Failure: low thump, sinks the pad darker and deeper.
                self.spawn(low_thump(C2, self.sample_rate));
                self.depth = (self.depth + 0.35).min(1.0);
                self.brightness = (self.brightness * 0.6).max(0.0);
            }
            PULSE_RESOLVE => {
                // If a cache-hit bell chord rang in the last two seconds,
                // let those bells *be* the resolution: skip the BRAAAM and
                // the big triad so nothing buries the reward moment. The
                // pad will still enter its closing hold and release, so the
                // bells ring on over the fade.
                if self.samples_since_cache_hit < self.sample_rate * 2 {
                    self.brightness = (self.brightness + 0.10).min(1.0);
                } else {
                    // Full BRAAAM plus a swelling major triad.
                    self.spawn(braaam(C1, self.sample_rate));
                    self.spawn(brass_chord(C3, self.sample_rate));
                    self.spawn(brass_chord(E3, self.sample_rate));
                    self.spawn(brass_chord(G3, self.sample_rate));
                    self.brightness = (self.brightness + 0.25).min(1.0);
                    self.warmth = (self.warmth + 0.25).min(1.0);
                }
            }
            _ => {}
        }
    }

    fn spawn(&mut self, gesture: Gesture) {
        let start = self.next_gesture;
        let mut target = start;
        for offset in 0..GESTURE_SLOTS {
            let idx = (start + offset) % GESTURE_SLOTS;
            if self.gestures[idx].amp < 1e-3 && self.gestures[idx].peak < 1e-3 {
                target = idx;
                break;
            }
        }
        self.gestures[target] = gesture;
        self.next_gesture = (target + 1) % GESTURE_SLOTS;
    }

    fn advance_progression(&mut self, chord_kind: u8, in_sustain: bool) {
        // Only advance the progression while sustaining. Once we hit the
        // closing hold, the chord settles wherever it is so the finish
        // resolves on the current tone rather than mid-step.
        if in_sustain {
            self.progression_samples_left = self.progression_samples_left.saturating_sub(1);
            if self.progression_samples_left == 0 {
                let table = progression_table(self.progression_variant, chord_kind);
                self.progression_index = (self.progression_index + 1) % table.len();
                self.progression_samples_left =
                    samples_for(self.sample_rate, PROGRESSION_STEP_SECONDS);
            }
        }
        let table = progression_table(self.progression_variant, chord_kind);
        self.chord_target = transposed_chord(&table[self.progression_index], self.pitch_multiplier);
    }
}

impl Iterator for AmbientSynth {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        // Tick the cache-hit timer forward before we read controls, so a
        // PULSE_RESOLVE that comes in this sample sees the up-to-date age
        // of any prior cache-hit bell chord.
        self.samples_since_cache_hit = self.samples_since_cache_hit.saturating_add(1);
        let (target, chord_kind) = self.read_controls();

        // --- Master envelope with closing-hold state -------------------
        //
        // MASTER_CLOSING keeps master_level pinned at 1.0 for a couple of
        // seconds, so the closing BRAAAM and chord can fully swell in
        // before the pad starts fading. Once the hold expires, we fall
        // through to the same release ramp as MASTER_RELEASE.
        let (target_level, is_releasing) = match target {
            MASTER_SUSTAIN => {
                self.closing_primed = false;
                if !self.seed_applied {
                    self.seed_applied = true;
                    self.apply_seed();
                }
                (1.0, false)
            }
            MASTER_CLOSING => {
                if !self.closing_primed {
                    self.closing_primed = true;
                    self.closing_hold_samples_left =
                        samples_for(self.sample_rate, CLOSING_HOLD_SECONDS);
                }
                if self.closing_hold_samples_left > 0 {
                    self.closing_hold_samples_left -= 1;
                    (1.0, false)
                } else {
                    (0.0, true)
                }
            }
            MASTER_RELEASE => {
                self.closing_primed = false;
                (0.0, true)
            }
            _ => {
                self.closing_primed = false;
                (0.0, false)
            }
        };

        let coeff = if target_level > self.master_level {
            slew_coeff(self.sample_rate, MASTER_ATTACK_SECONDS)
        } else {
            slew_coeff(self.sample_rate, MASTER_RELEASE_SECONDS)
        };
        self.master_level += (target_level - self.master_level) * coeff;

        // Everything (bed AND gestures) is scaled by the master envelope
        // further down, so the whole mix fades to zero together and nothing
        // gets stranded ringing over silence. Once the envelope is below
        // audibility we can signal the audio thread that we're done and
        // stop producing samples.
        if is_releasing && self.master_level < MASTER_EXIT_THRESHOLD {
            self.shared.finished.store(true, Ordering::Relaxed);
            return None;
        }

        // --- Chord progression -----------------------------------------
        let in_sustain = target == MASTER_SUSTAIN;
        self.advance_progression(chord_kind, in_sustain);
        let glide = slew_coeff(self.sample_rate, 0.9);
        for i in 0..3 {
            self.chord_freq[i] += (self.chord_target[i] - self.chord_freq[i]) * glide;
        }

        let step = TAU / self.sample_rate as f32;

        // --- Free-running LFOs at incommensurate rates -----------------
        advance(&mut self.breath_lfo_phase, step * 0.08);
        advance(&mut self.mid_lfo_phase, step * 0.055);
        advance(&mut self.shimmer_amp_lfo_phase, step * 0.19);
        advance(&mut self.detune_lfo_phase, step * 0.028);
        advance(&mut self.wow_lfo_phase, step * 0.4);
        let breath = 0.80 + 0.20 * self.breath_lfo_phase.sin();
        let mid_swell = 0.70 + 0.30 * self.mid_lfo_phase.sin();
        let shimmer_swell = 0.55 + 0.45 * self.shimmer_amp_lfo_phase.sin();
        let detune = 1.005 + 0.003 * self.detune_lfo_phase.sin();
        // Very small pitch wobble on the drone, ~0.4% peak, so the low end
        // feels living rather than static.
        let wow = 1.0 + 0.004 * self.wow_lfo_phase.sin();

        // --- Continuous bed --------------------------------------------
        let drone_base_gain = 0.30 + 0.25 * self.depth;
        let pm = self.pitch_multiplier;
        advance(&mut self.drone_phase[0], step * C2 * pm * wow);
        advance(&mut self.drone_phase[1], step * (C2 * pm * detune * wow));
        advance(&mut self.drone_phase[2], step * G2 * pm);
        advance(&mut self.drone_phase[3], step * C3 * pm);
        let drone = (self.drone_phase[0].sin() * 0.55
            + self.drone_phase[1].sin() * 0.35
            + self.drone_phase[2].sin() * 0.30
            + self.drone_phase[3].sin() * 0.18)
            * drone_base_gain
            * breath;

        let mid_base_gain = 0.10 + 0.14 * self.warmth;
        advance(&mut self.chord_phase[0], step * self.chord_freq[0]);
        advance(&mut self.chord_phase[1], step * self.chord_freq[1]);
        advance(&mut self.chord_phase[2], step * self.chord_freq[2]);
        let mid = (self.chord_phase[0].sin() * 0.55
            + self.chord_phase[1].sin() * 0.48
            + self.chord_phase[2].sin() * 0.55)
            * mid_base_gain
            * mid_swell;

        let shimmer_base_gain = 0.04 + 0.20 * self.brightness;
        advance(&mut self.shimmer_phase, step * self.shimmer_root * pm);
        advance(&mut self.shimmer_lfo_phase, step * 0.35);
        let tremolo = 0.5 + 0.5 * self.shimmer_lfo_phase.sin();
        let shimmer = self.shimmer_phase.sin() * shimmer_base_gain * tremolo * shimmer_swell;

        // --- Arpeggio: melodic sparkle stepping through the chord ------
        //
        // Rate is modulated by density so a busy build audibly quickens.
        // Arpeggio is silenced during the closing hold so it doesn't fight
        // the resolution.
        let arp = if in_sustain {
            let period_seconds = self.arp_period_idle_seconds
                + (ARP_PERIOD_SECONDS_ACTIVE - self.arp_period_idle_seconds) * self.density;
            let period_samples = samples_for(self.sample_rate, period_seconds).max(1);
            if self.arp_samples_left == 0 {
                let step_idx = ARP_PATTERN[self.arp_pattern_step];
                self.arp_freq = self.chord_target[step_idx] * 2.0;
                self.arp_amp = 0.20;
                self.arp_phase = 0.0;
                self.arp_pattern_step = (self.arp_pattern_step + 1) % ARP_PATTERN.len();
                self.arp_samples_left = period_samples;
            } else {
                self.arp_samples_left -= 1;
            }
            advance(&mut self.arp_phase, step * self.arp_freq);
            let sample = self.arp_phase.sin() * self.arp_amp;
            self.arp_amp *= slew_decay(self.sample_rate, ARP_NOTE_DECAY_SECONDS);
            sample * (0.6 + 0.5 * self.density)
        } else {
            // Arpeggio winds down gently rather than snapping off, so the
            // last plucked note gets to ring into the closing atmosphere.
            self.arp_amp *= slew_decay(self.sample_rate, 3.0);
            advance(&mut self.arp_phase, step * self.arp_freq);
            self.arp_phase.sin() * self.arp_amp
        };

        let bed = (drone + mid + shimmer + arp) * self.master_level * 0.55;

        // --- Gesture layer ---------------------------------------------
        let mut gesture_sum = 0.0;
        for gesture in &mut self.gestures {
            if gesture.amp < 1e-4 && gesture.peak < 1e-4 {
                continue;
            }
            gesture.freq += (gesture.freq_target - gesture.freq) * gesture.freq_slew;
            advance(&mut gesture.phase, step * gesture.freq);
            advance(&mut gesture.phase_detune, step * (gesture.freq * gesture.detune_ratio));
            let mut voice = 0.0;
            for h in 0..4 {
                let n = (h as f32) + 1.0;
                let gain = gesture.harmonic_gain[h];
                if gain.abs() < 1e-6 {
                    continue;
                }
                voice += gain * (gesture.phase * n).sin();
                voice += gain * (gesture.phase_detune * n).sin() * 0.7;
            }
            gesture_sum += voice * gesture.amp;
            if gesture.amp + 1e-4 < gesture.peak {
                gesture.amp += (gesture.peak - gesture.amp) * gesture.attack;
            } else {
                gesture.peak = 0.0;
                gesture.amp *= gesture.decay;
            }
        }

        // --- Dimension decay -------------------------------------------
        self.brightness *= slew_decay(self.sample_rate, BRIGHTNESS_DECAY_SECONDS);
        self.warmth *= slew_decay(self.sample_rate, WARMTH_DECAY_SECONDS);
        self.depth *= slew_decay(self.sample_rate, DEPTH_DECAY_SECONDS);
        self.density *= slew_decay(self.sample_rate, DENSITY_DECAY_SECONDS);

        // Both the bed and the gesture layer are already scaled by the bed
        // multiplier, but only the bed multiplied by `master_level` above.
        // Pull the gestures under the same envelope here so cache-hit bells,
        // BRAAAM, and every action stab fade with the pad instead of ringing
        // over silence.
        let mix = (bed + gesture_sum * self.master_level) * OUTPUT_GAIN;
        Some(mix.tanh() * 0.85)
    }
}

impl Source for AmbientSynth {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        1
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

// -------- Utility -----------------------------------------------------

fn advance(phase: &mut f32, delta: f32) {
    *phase += delta;
    if *phase >= TAU {
        *phase -= TAU;
    }
}

fn slew_coeff(sample_rate: u32, seconds: f32) -> f32 {
    1.0 - (-1.0 / (sample_rate as f32 * seconds)).exp()
}

fn slew_decay(sample_rate: u32, seconds: f32) -> f32 {
    (-1.0 / (sample_rate as f32 * seconds)).exp()
}

fn samples_for(sample_rate: u32, seconds: f32) -> u32 {
    (sample_rate as f32 * seconds).max(1.0) as u32
}
