//! The per-voice modulation kernel eseq's voice modulator and the exported
//! plugin both run: four slots of LFO / envelope / random / drift / ext over a
//! flat `f32` state (layout constants below), driven by gate, trigger and the
//! transport bar clock. Pure: no allocation, no globals, no app types. The
//! graph node (`sequencer::instruments::voice_modulator`) adds voice
//! identity, slot leases, silence declarations and stats around it.
#![allow(clippy::missing_safety_doc, clippy::too_many_arguments)]

/// Beats per cycle of each tempo-sync division, by division index (the order
/// of `sequencer::effects::SyncDivision::ALL`; a sequencer test pins it).
pub const DIVISION_BEATS: [f32; 14] = [
    0.125,
    0.25,
    1.0 / 6.0,
    0.5,
    1.0 / 3.0,
    0.75,
    1.0,
    2.0 / 3.0,
    1.5,
    2.0,
    4.0,
    8.0,
    16.0,
    32.0,
];

/// Beats of division `index`, clamped to the table like `SyncDivision::from_index`.
pub fn division_beats(index: usize) -> f32 {
    DIVISION_BEATS[index.min(DIVISION_BEATS.len() - 1)]
}

/// State cells the kernel itself reads and writes (a host may keep more after
/// them, as the graph node does for its output-lane cache).
pub const KERNEL_STATE_SIZE: usize = STATE_DISPLAY_SLOT_PHASE + SLOT_COUNT;

/// Zero `s[..KERNEL_STATE_SIZE]`, seed each slot's RNG and set the sample
/// rate: a fresh voice, before its params are written.
pub unsafe fn init_state(s: *mut f32, sample_rate: f32) {
    unsafe {
        for i in 0..KERNEL_STATE_SIZE {
            *s.add(i) = 0.0;
        }
        for slot in 0..SLOT_COUNT {
            *s.add(slot_state_idx(slot, IDX_RNG)) = 0x1234_5678u32.wrapping_add(slot as u32) as f32;
        }
        *s.add(IDX_SAMPLE_RATE) = sample_rate;
    }
}

pub const SLOT_COUNT: usize = 4;

pub const NUM_OUTPUTS: usize = SLOT_COUNT;

pub const INPUT_GATE: usize = 0;

pub const INPUT_PITCH: usize = 1;

pub const INPUT_VELOCITY: usize = 2;

pub const INPUT_TRIGGER: usize = 3;

pub const INPUT_EXT_BASE: usize = 4;

pub const INPUT_TRANSPORT_BAR_PHASE: usize = 8;

pub const INPUT_TRANSPORT_BAR_PHASE_INC: usize = 9;

pub const INPUT_COUNT: usize = 10;

pub const EXT_INPUT_COUNT: usize = 4;

pub const SLOT_STATE_STRIDE: usize = 11;
pub const IDX_SLOT_STATE_BASE: usize = 0;
pub const IDX_LFO_PHASE: usize = 0;
pub const IDX_ENV: usize = 1;
pub const IDX_RAND_PHASE: usize = 2;
pub const IDX_DRIFT: usize = 3;
pub const IDX_RNG: usize = 4;
pub const IDX_RAND_HOLD: usize = 5;
pub const IDX_RAND_SMOOTH: usize = 6;
pub const IDX_ENV_STAGE: usize = 7;
/// Drift: 0..1 progress from the `from` value (`IDX_DRIFT`) to the next random
/// target (`IDX_DRIFT_TARGET`); a wrap picks a new target.
pub const IDX_DRIFT_PHASE: usize = 8;
pub const IDX_DRIFT_TARGET: usize = 9;
/// Synced LFO + retrigger: the bar-derived phase captured at the last note-on,
/// subtracted so the cycle restarts at the note instead of staying locked to
/// the bar. Ignored while retrigger is off, so toggling it off re-locks.
pub const IDX_LFO_SYNC_ANCHOR: usize = 10;

pub const IDX_PREV_GATE: usize = IDX_SLOT_STATE_BASE + SLOT_STATE_STRIDE * SLOT_COUNT;
pub const IDX_LAST_RESET_COUNTER: usize = IDX_PREV_GATE + 1;
pub const IDX_SAMPLE_RATE: usize = IDX_LAST_RESET_COUNTER + 1;
pub const IDX_CUSTOM_ENGINE_ID: usize = IDX_SAMPLE_RATE + 1;
pub const IDX_CUSTOM_VOICE_IDX: usize = IDX_CUSTOM_ENGINE_ID + 1;
pub const IDX_SAMPLER_TRACK_IDX: usize = IDX_CUSTOM_VOICE_IDX + 1;
pub const IDX_SAMPLER_VOICE_IDX: usize = IDX_SAMPLER_TRACK_IDX + 1;
pub const IDX_TRANSPORT_CLOCK_SOURCE: usize = IDX_SAMPLER_VOICE_IDX + 1;
pub const IDX_EVENT_SLICE_START: usize = IDX_TRANSPORT_CLOCK_SOURCE + 1;
/// The transport only reports a phase *within* the bar, so multi-bar sync
/// divisions need the bar wraps counted here: `IDX_BAR_COUNT` advances every
/// time the bar phase wraps while the clock is advancing, and resets with the
/// mod reset counter (transport start / bar resync). Every process call
/// advances it, including the idle early returns, so a voice that sat silent
/// for a few bars counts the same wraps as one that kept sounding.
pub const IDX_PREV_BAR_PHASE: usize = IDX_EVENT_SLICE_START + 1;
pub const IDX_BAR_COUNT: usize = IDX_PREV_BAR_PHASE + 1;
pub const _: () = assert!(IDX_BAR_COUNT < PARAM_SLOT_BASE);

pub const PARAM_SLOT_BASE: usize = 64;
pub const PARAM_SLOT_STRIDE: usize = 18;
pub const PARAM_SLOT_SOURCE: usize = 0;
pub const PARAM_LFO_RATE_HZ: usize = 1;
pub const PARAM_LFO_SYNC: usize = 2;
pub const PARAM_LFO_DIV: usize = 3;
pub const PARAM_LFO_SHAPE: usize = 4;
pub const PARAM_LFO_PW: usize = 5;
pub const PARAM_LFO_RETRIGGER: usize = 6;
pub const PARAM_ENV_ATTACK_MS: usize = 7;
pub const PARAM_ENV_DECAY_MS: usize = 8;
pub const PARAM_ENV_SUSTAIN: usize = 9;
pub const PARAM_ENV_RELEASE_MS: usize = 10;
pub const PARAM_RAND_RATE_HZ: usize = 11;
pub const PARAM_RAND_SYNC: usize = 12;
pub const PARAM_RAND_DIV: usize = 13;
pub const PARAM_RAND_SLEW: usize = 14;
pub const PARAM_DRIFT_RATE: usize = 15;
pub const PARAM_DRIFT_SYNC: usize = 16;
pub const PARAM_DRIFT_DIV: usize = 17;

pub const PARAM_BPM: usize = PARAM_SLOT_BASE + PARAM_SLOT_STRIDE * SLOT_COUNT;
pub const PARAM_TRANSPORT_BAR_PHASE: usize = PARAM_BPM + 1;
pub const PARAM_TRANSPORT_BAR_PHASE_INC: usize = PARAM_TRANSPORT_BAR_PHASE + 1;
pub const PARAM_RESET_COUNTER: usize = PARAM_TRANSPORT_BAR_PHASE_INC + 1;
/// Per-slot LFO phase offset in degrees, one param per slot at
/// `PARAM_LFO_PHASE_BASE + slot`. It lives *after* the slot block rather than
/// inside it on purpose: the slot stride and every existing node param index
/// are part of the saved-project contract (`ProjectEffectSlot` remaps by node
/// index, and `EffectSlotSnapshot::sync_to_descriptor` preserves values
/// positionally), so a param inserted mid-slot would silently shift every
/// later slot's saved values onto the wrong control.
pub const PARAM_LFO_PHASE_BASE: usize = PARAM_RESET_COUNTER + 1;

/// Display tail (eseq-dtx.13). `voice_modulator_process` stores the block's
/// last output value per slot here so a UI-thread poller can read the live
/// modulation signal back off the audiograph watchlist and render effective
/// (post-modulation) parameter values. Four plain `f32` stores per block on
/// the audio thread — no allocation, no locking, no atomics needed because
/// the watchlist snapshot is taken between blocks, and a torn read would at
/// worst show one stale display value.
pub const STATE_DISPLAY_SLOT_VALUE: usize = PARAM_LFO_PHASE_BASE + SLOT_COUNT;

/// Display tail, second row: the slot's cycle position for the source editor's
/// waveform marker. LFO slots publish their effective phase (after the phase
/// offset param), rand and drift slots the 0..1 progress towards their next
/// random value, and every other source `-1.0` (no marker). Same contract as
/// `STATE_DISPLAY_SLOT_VALUE`: plain per-block stores, read back off the
/// watchlist between blocks.
pub const STATE_DISPLAY_SLOT_PHASE: usize = STATE_DISPLAY_SLOT_VALUE + SLOT_COUNT;

/// No marker: the slot's source has no cycle to place one on.
pub const DISPLAY_PHASE_NONE: f32 = -1.0;

/// The `@mod-mode additive` contract that `(mod name)` compiles to inside the
/// DGen engine (and that the hand-written Rust built-ins implement inline):
/// the base cell plus, when the destination's `__dgen_mod_active__` flag is
/// set, the sum of every depth lane scaled by its modulator slot's 0..1
/// output. Dual-maintained with the DGenLisp compiler's `mod` expansion; the
/// UI reuses it to draw the effective value without touching the engine.
pub fn additive_modulated_value(
    base: f32,
    active: bool,
    depths: &[f32; SLOT_COUNT],
    slot_values: &[f32; SLOT_COUNT],
) -> f32 {
    if !active {
        return base;
    }
    let mut value = base;
    for slot in 0..SLOT_COUNT {
        value += depths[slot] * slot_values[slot].clamp(0.0, 1.0);
    }
    value
}

pub const SHAPE_TRIANGLE: usize = 0;
pub const SHAPE_SINE: usize = 1;
pub const SHAPE_PULSE: usize = 2;
pub const SHAPE_SAW: usize = 3;

/// Drift picks a new random target every `1 / rate` seconds and glides to it
/// with a raised-cosine, so the output covers the full range at every rate
/// (the old one-pole design scaled its target by 0.08 and barely moved).
/// Envelope stage ceiling. Was 2 s attack / 4 s decay+release, which the
/// user hit constantly; exponential scaling keeps the low end fine-grained.
pub const ENV_TIME_MAX_MS: f32 = 20_000.0;
pub const DRIFT_RATE_MIN_HZ: f32 = 0.02;
pub const DRIFT_RATE_MAX_HZ: f32 = 8.0;

pub const SOURCE_OFF: usize = 0;
pub const SOURCE_LFO: usize = 1;
pub const SOURCE_ENV: usize = 2;
pub const SOURCE_RAND: usize = 3;
pub const SOURCE_DRIFT: usize = 4;
pub const SOURCE_EXT1: usize = 5;
pub const SOURCE_EXT2: usize = 6;
pub const SOURCE_EXT3: usize = 7;
pub const SOURCE_EXT4: usize = 8;

pub const ENV_STAGE_IDLE: f32 = 0.0;
pub const ENV_STAGE_ATTACK: f32 = 1.0;
pub const ENV_STAGE_DECAY: f32 = 2.0;
pub const ENV_STAGE_SUSTAIN: f32 = 3.0;
pub const ENV_STAGE_RELEASE: f32 = 4.0;

pub fn slot_state_idx(slot: usize, offset: usize) -> usize {
    IDX_SLOT_STATE_BASE + slot * SLOT_STATE_STRIDE + offset
}

pub fn slot_param_idx(slot: usize, offset: usize) -> usize {
    PARAM_SLOT_BASE + slot * PARAM_SLOT_STRIDE + offset
}

pub fn slot_source_param_idx(slot: usize) -> usize {
    slot_param_idx(slot, PARAM_SLOT_SOURCE)
}

pub fn slot_lfo_phase_param_idx(slot: usize) -> usize {
    PARAM_LFO_PHASE_BASE + slot
}

pub fn next_rand(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    let v = ((*state >> 8) as f32) / ((u32::MAX >> 8) as f32);
    v * 2.0 - 1.0
}

/// Triangle whose peak sits at `peak` (0..1) of the cycle: 0.5 is symmetric,
/// lower values ramp up fast and fall slowly, higher values the reverse. The
/// same `lfo_pw` param drives it, so the pulse's width and the triangle's
/// skew share one control ("pw" / "peak" in the editor).
pub fn triangle(phase: f32, peak: f32) -> f32 {
    let peak = peak.clamp(0.05, 0.95);
    if phase < peak {
        -1.0 + 2.0 * phase / peak
    } else {
        1.0 - 2.0 * (phase - peak) / (1.0 - peak)
    }
}

pub fn shape_value(shape: usize, phase: f32, pulse_width: f32) -> f32 {
    match shape {
        SHAPE_SINE => (std::f32::consts::TAU * phase).sin(),
        SHAPE_PULSE => {
            if phase < pulse_width.clamp(0.05, 0.95) {
                1.0
            } else {
                -1.0
            }
        }
        SHAPE_SAW => phase * 2.0 - 1.0,
        _ => triangle(phase, pulse_width),
    }
}

/// Phase offset param (degrees) applied on top of the running LFO phase.
pub fn lfo_effective_phase(phase: f32, offset_degrees: f32) -> f32 {
    normalize_phase(phase + offset_degrees.clamp(0.0, 360.0) / 360.0)
}

/// Raised-cosine glide from `from` to `target` over `phase` in 0..1: smooth at
/// both ends, so consecutive targets join without a kink.
pub fn drift_value(from: f32, target: f32, phase: f32) -> f32 {
    let t = (1.0 - (std::f32::consts::PI * phase.clamp(0.0, 1.0)).cos()) * 0.5;
    (from + (target - from) * t).clamp(-1.0, 1.0)
}

pub fn bipolar_to_unipolar(value: f32) -> f32 {
    ((value.clamp(-1.0, 1.0) + 1.0) * 0.5).clamp(0.0, 1.0)
}

pub fn synced_rate_hz(div_idx: usize, bpm: f32) -> f32 {
    let beats = division_beats(div_idx);
    (bpm.max(20.0) / 60.0) / beats.max(0.0001)
}

pub fn normalize_phase(value: f32) -> f32 {
    if value.is_finite() {
        value - value.floor()
    } else {
        0.0
    }
}

pub fn transport_bar_phase_for_frame(base_phase: f32, phase_inc: f32, frame: usize) -> f32 {
    normalize_phase(base_phase + phase_inc * frame as f32)
}

/// Where one block reads its transport clock from: the per-frame input buses
/// when the node is input-clocked, else the block-start params extrapolated
/// by the event-slice offset.
pub struct BlockTransportClock {
    pub phase_in: *const f32,
    pub phase_inc_in: *const f32,
    pub param_phase: f32,
    pub param_phase_inc: f32,
    pub event_slice_start: usize,
}

/// Running bar counter behind `IDX_PREV_BAR_PHASE` / `IDX_BAR_COUNT`.
#[derive(Clone, Copy)]
pub struct BarTracker {
    pub prev_bar_phase: f32,
    pub bar_count: f32,
}

/// Cycle phase for a synced division. Divisions up to one bar derive it from
/// the bar phase alone; longer ones fold the running bar count in, so a
/// 4-bar LFO no longer restarts at every bar line.
pub fn synced_phase_from_bar_phase(div_idx: usize, bar_phase: f32, bar_count: f32) -> f32 {
    synced_phase_from_bar_phase_beats(
        division_beats(div_idx),
        bar_phase,
        bar_count,
    )
}

pub fn synced_phase_from_bar_phase_beats(beats: f32, bar_phase: f32, bar_count: f32) -> f32 {
    let cycle_bars = beats / 4.0;
    if cycle_bars <= 1.0 {
        return normalize_phase(normalize_phase(bar_phase) * 4.0 / beats.max(0.0001));
    }
    let bar_in_cycle = bar_count.max(0.0).rem_euclid(cycle_bars);
    normalize_phase((bar_in_cycle + normalize_phase(bar_phase)) / cycle_bars)
}

pub unsafe fn slot_source(s: *const f32, slot: usize) -> usize {
    let source = (*s.add(slot_source_param_idx(slot))).round() as usize;
    if source <= SOURCE_EXT4 {
        source
    } else {
        SOURCE_OFF
    }
}

/// Copy the block's last output frame into the display tail
/// (`STATE_DISPLAY_SLOT_VALUE`). Bounded, allocation-free, lock-free; see the
/// constant's docs.
pub unsafe fn publish_slot_display_values(s: *mut f32, out: *const *mut f32, nf: usize) {
    if out.is_null() || nf == 0 {
        return;
    }
    for slot in 0..NUM_OUTPUTS {
        let out_slot = *out.add(slot);
        let value = if out_slot.is_null() {
            0.0
        } else {
            *out_slot.add(nf - 1)
        };
        *s.add(STATE_DISPLAY_SLOT_VALUE + slot) = value;
        *s.add(STATE_DISPLAY_SLOT_PHASE + slot) = slot_display_phase(s, slot);
    }
}

/// Cycle position for the source editor's waveform marker; see
/// `STATE_DISPLAY_SLOT_PHASE`.
pub unsafe fn slot_display_phase(s: *const f32, slot: usize) -> f32 {
    match slot_source(s, slot) {
        SOURCE_LFO => lfo_effective_phase(
            *s.add(slot_state_idx(slot, IDX_LFO_PHASE)),
            *s.add(slot_lfo_phase_param_idx(slot)),
        ),
        SOURCE_RAND => (*s.add(slot_state_idx(slot, IDX_RAND_PHASE))).clamp(0.0, 1.0),
        SOURCE_DRIFT => (*s.add(slot_state_idx(slot, IDX_DRIFT_PHASE))).clamp(0.0, 1.0),
        _ => DISPLAY_PHASE_NONE,
    }
}

/// Frames per pre-pass chunk in `render_frames`; bounds its stack arrays.
pub const RENDER_CHUNK: usize = 128;

/// Per-frame facts shared by every slot of one chunk.
pub struct ChunkFrames {
    pub len: usize,
    /// `false` for a sampler voice still waiting for its first gate: every
    /// output is zero and no slot state advances.
    pub active: [bool; RENDER_CHUNK],
    pub note_on: [bool; RENDER_CHUNK],
    pub gate: [f32; RENDER_CHUNK],
    pub bar_phase: [f32; RENDER_CHUNK],
    pub clock_advancing: [bool; RENDER_CHUNK],
    pub bar_count: [f32; RENDER_CHUNK],
    /// Every frame active, none a note-on: lets steady envelopes fill.
    pub steady: bool,
    pub all_gate_high: bool,
}

/// Render every non-Off slot for the block. The gate/clock pre-pass runs once
/// per chunk, then each slot runs its own loop with its params hoisted and its
/// state in locals. Slots share no state, so this is equivalent to the
/// frame-major reference loop (pinned bit-for-bit by
/// `block_renderer_matches_per_sample_reference`).
#[allow(clippy::too_many_arguments)]
pub unsafe fn render_frames(
    s: *mut f32,
    out: *const *mut f32,
    ext_inputs: &[*mut f32; EXT_INPUT_COUNT],
    slot_sources: &[usize; SLOT_COUNT],
    gate_in: *const f32,
    trigger_in: *const f32,
    clock: &BlockTransportClock,
    bars: &mut BarTracker,
    prev_gate: &mut f32,
    sampler_voice_started: &mut bool,
    is_sampler: bool,
    nf: usize,
    sample_rate: f32,
    bpm: f32,
) {
    let mut frames = ChunkFrames {
        len: 0,
        active: [false; RENDER_CHUNK],
        note_on: [false; RENDER_CHUNK],
        gate: [0.0; RENDER_CHUNK],
        bar_phase: [0.0; RENDER_CHUNK],
        clock_advancing: [false; RENDER_CHUNK],
        bar_count: [0.0; RENDER_CHUNK],
        steady: true,
        all_gate_high: true,
    };
    // Only a synced LFO reads the per-frame transport clock; the bar counter
    // itself still advances on every frame below.
    let needs_clock = slot_sources.iter().enumerate().any(|(slot, source)| {
        *source == SOURCE_LFO && *s.add(slot_param_idx(slot, PARAM_LFO_SYNC)) > 0.5
    });
    let mut start = 0;
    while start < nf {
        let len = (nf - start).min(RENDER_CHUNK);
        frames.len = len;
        frames.steady = true;
        frames.all_gate_high = true;
        for j in 0..len {
            let i = start + j;
            let gate = (*gate_in.add(i)).clamp(0.0, 1.0);
            let trigger = (*trigger_in.add(i)).max(0.0);
            // Bar wraps are counted before the not-yet-started skip below, so a
            // sampler voice waiting for its first gate stays on the bar grid.
            let (transport_bar_phase, transport_clock_advancing) = clock.frame(i);
            bars.note_frame(transport_bar_phase, transport_clock_advancing);
            if !*sampler_voice_started && gate <= 0.5 && trigger <= 0.5 {
                frames.active[j] = false;
                frames.steady = false;
                *prev_gate = gate;
                continue;
            }
            if is_sampler && (gate > 0.5 || trigger > 0.5) {
                *sampler_voice_started = true;
            }
            frames.active[j] = true;
            frames.note_on[j] = (gate > 0.5 && *prev_gate <= 0.5) || trigger > 0.5;
            frames.steady &= !frames.note_on[j];
            frames.all_gate_high &= gate > 0.5;
            frames.gate[j] = gate;
            if needs_clock {
                frames.bar_phase[j] = transport_bar_phase;
                frames.clock_advancing[j] = transport_clock_advancing;
                frames.bar_count[j] = bars.bar_count;
            }
            *prev_gate = gate;
        }
        for (slot, source) in slot_sources.iter().copied().enumerate() {
            if source == SOURCE_OFF {
                continue;
            }
            let out_slot = std::slice::from_raw_parts_mut((*out.add(slot)).add(start), len);
            render_slot_block(s, ext_inputs, slot, source, start, &frames, out_slot, sample_rate, bpm);
        }
        start += len;
    }
}

/// Reproduces the reference's per-sample `u32 -> f32` RNG state round trip:
/// the state lives in an `f32` slot, so every stored value is rounded.
pub struct SlotRng {
    pub stored: f32,
    pub seed: u32,
}

impl SlotRng {
    pub unsafe fn load(s: *const f32, slot: usize) -> Self {
        Self {
            stored: *s.add(slot_state_idx(slot, IDX_RNG)),
            seed: 0x1234_5678u32.wrapping_add(slot as u32),
        }
    }

    /// The state one sample sees on entry.
    pub fn current(&self) -> u32 {
        let state = self.stored as u32;
        if state == 0 {
            self.seed
        } else {
            state
        }
    }

    pub fn finish_sample(&mut self, state: u32) {
        self.stored = state as f32;
    }

    pub unsafe fn store(&self, s: *mut f32, slot: usize) {
        *s.add(slot_state_idx(slot, IDX_RNG)) = self.stored;
    }
}

#[allow(clippy::too_many_arguments)]
pub unsafe fn render_slot_block(
    s: *mut f32,
    ext_inputs: &[*mut f32; EXT_INPUT_COUNT],
    slot: usize,
    source: usize,
    start: usize,
    frames: &ChunkFrames,
    out: &mut [f32],
    sample_rate: f32,
    bpm: f32,
) {
    let len = frames.len;
    if (SOURCE_EXT1..=SOURCE_EXT4).contains(&source) {
        let ptr = ext_inputs[source - SOURCE_EXT1];
        for j in 0..len {
            out[j] = if !frames.active[j] || ptr.is_null() {
                0.0
            } else {
                (*ptr.add(start + j)).clamp(0.0, 1.0)
            };
        }
        return;
    }
    let param = |offset: usize| *s.add(slot_param_idx(slot, offset));
    let state = |offset: usize| s.add(slot_state_idx(slot, offset));
    let mut rng = SlotRng::load(s, slot);
    let mut rendered = false;

    match source {
        SOURCE_LFO => {
            let mut phase = *state(IDX_LFO_PHASE);
            let mut anchor = *state(IDX_LFO_SYNC_ANCHOR);
            let sync = param(PARAM_LFO_SYNC) > 0.5;
            let div = param(PARAM_LFO_DIV).round() as usize;
            let beats = division_beats(div);
            let retrigger = param(PARAM_LFO_RETRIGGER) > 0.5;
            let rate = if sync {
                synced_rate_hz(div, bpm)
            } else {
                param(PARAM_LFO_RATE_HZ).clamp(0.01, 20.0)
            };
            let increment = rate / sample_rate;
            let shape = param(PARAM_LFO_SHAPE).round() as usize;
            let pw = param(PARAM_LFO_PW).clamp(0.05, 0.95);
            let offset_degrees = *s.add(slot_lfo_phase_param_idx(slot));
            for j in 0..len {
                if !frames.active[j] {
                    out[j] = 0.0;
                    continue;
                }
                rendered = true;
                let note_on = frames.note_on[j];
                if sync && frames.clock_advancing[j] {
                    let bar_derived = synced_phase_from_bar_phase_beats(
                        beats,
                        frames.bar_phase[j],
                        frames.bar_count[j],
                    );
                    phase = if retrigger {
                        if note_on {
                            anchor = bar_derived;
                        }
                        normalize_phase(bar_derived - anchor)
                    } else {
                        bar_derived
                    };
                } else {
                    if note_on && retrigger {
                        phase = 0.0;
                    }
                    phase = normalize_phase(phase + increment);
                }
                let effective = lfo_effective_phase(phase, offset_degrees);
                out[j] = bipolar_to_unipolar(shape_value(shape, effective, pw)).clamp(0.0, 1.0);
            }
            *state(IDX_LFO_PHASE) = phase;
            *state(IDX_LFO_SYNC_ANCHOR) = anchor;
            let current = rng.current();
            rng.finish_sample(current);
        }
        SOURCE_ENV => {
            let mut env = *state(IDX_ENV);
            let mut stage = *state(IDX_ENV_STAGE);
            let attack = 1.0
                / (param(PARAM_ENV_ATTACK_MS).clamp(1.0, ENV_TIME_MAX_MS) * 0.001 * sample_rate);
            let decay = 1.0
                / (param(PARAM_ENV_DECAY_MS).clamp(5.0, ENV_TIME_MAX_MS) * 0.001 * sample_rate);
            let sustain = param(PARAM_ENV_SUSTAIN).clamp(0.0, 1.0);
            let release = 1.0
                / (param(PARAM_ENV_RELEASE_MS).clamp(5.0, ENV_TIME_MAX_MS) * 0.001 * sample_rate);
            // Held sustain and idle are fixed points of the per-frame stage
            // machine below; fill them instead of stepping every frame.
            let fixed = frames.steady
                && ((stage == ENV_STAGE_SUSTAIN && frames.all_gate_high) || stage == ENV_STAGE_IDLE);
            if fixed {
                if stage == ENV_STAGE_SUSTAIN {
                    env = sustain;
                }
                env = env.clamp(0.0, 1.0);
                out.fill(env);
                rendered = len > 0;
            }
            for j in 0..if fixed { 0 } else { len } {
                if !frames.active[j] {
                    out[j] = 0.0;
                    continue;
                }
                rendered = true;
                let gate = frames.gate[j];
                if frames.note_on[j] {
                    env = 0.0;
                    stage = ENV_STAGE_ATTACK;
                }
                if gate <= 0.5 && stage != ENV_STAGE_IDLE && stage != ENV_STAGE_RELEASE {
                    stage = ENV_STAGE_RELEASE;
                }
                if stage == ENV_STAGE_ATTACK {
                    env = (env + attack).min(1.0);
                    if env >= 0.999 {
                        env = 1.0;
                        stage = ENV_STAGE_DECAY;
                    }
                } else if stage == ENV_STAGE_DECAY {
                    env += (sustain - env) * decay;
                    if (env - sustain).abs() <= 0.001 {
                        env = sustain;
                        stage = if gate > 0.5 {
                            ENV_STAGE_SUSTAIN
                        } else {
                            ENV_STAGE_RELEASE
                        };
                    }
                } else if stage == ENV_STAGE_SUSTAIN {
                    env = sustain;
                    if gate <= 0.5 {
                        stage = ENV_STAGE_RELEASE;
                    }
                } else if stage == ENV_STAGE_RELEASE {
                    env += (0.0 - env) * release;
                    if env <= 0.0005 {
                        env = 0.0;
                        stage = ENV_STAGE_IDLE;
                    }
                }
                env = env.clamp(0.0, 1.0);
                out[j] = env;
            }
            *state(IDX_ENV) = env;
            *state(IDX_ENV_STAGE) = stage;
            let current = rng.current();
            rng.finish_sample(current);
        }
        SOURCE_RAND => {
            let mut phase = *state(IDX_RAND_PHASE);
            let mut hold = *state(IDX_RAND_HOLD);
            let mut smooth = *state(IDX_RAND_SMOOTH);
            let rate = if param(PARAM_RAND_SYNC) > 0.5 {
                synced_rate_hz(param(PARAM_RAND_DIV).round() as usize, bpm)
            } else {
                param(PARAM_RAND_RATE_HZ).clamp(0.01, 20.0)
            };
            let increment = rate / sample_rate;
            let slew = param(PARAM_RAND_SLEW).clamp(0.0, 0.999);
            for j in 0..len {
                if !frames.active[j] {
                    out[j] = 0.0;
                    continue;
                }
                rendered = true;
                let mut rng_state = rng.current();
                if frames.note_on[j] {
                    hold = next_rand(&mut rng_state);
                }
                let prev_phase = phase;
                phase = (phase + increment).fract();
                if phase < prev_phase {
                    hold = next_rand(&mut rng_state);
                }
                smooth += (hold - smooth) * (1.0 - slew);
                rng.finish_sample(rng_state);
                out[j] = bipolar_to_unipolar(smooth).clamp(0.0, 1.0);
            }
            *state(IDX_RAND_PHASE) = phase;
            *state(IDX_RAND_HOLD) = hold;
            *state(IDX_RAND_SMOOTH) = smooth;
        }
        SOURCE_DRIFT => {
            let mut from = *state(IDX_DRIFT);
            let mut target = *state(IDX_DRIFT_TARGET);
            let mut phase = *state(IDX_DRIFT_PHASE);
            let rate = if param(PARAM_DRIFT_SYNC) > 0.5 {
                synced_rate_hz(param(PARAM_DRIFT_DIV).round() as usize, bpm)
            } else {
                param(PARAM_DRIFT_RATE).clamp(DRIFT_RATE_MIN_HZ, DRIFT_RATE_MAX_HZ)
            };
            let increment = rate / sample_rate;
            for j in 0..len {
                if !frames.active[j] {
                    out[j] = 0.0;
                    continue;
                }
                rendered = true;
                let mut rng_state = rng.current();
                phase += increment;
                if phase >= 1.0 {
                    phase = normalize_phase(phase);
                    from = target;
                    target = next_rand(&mut rng_state);
                }
                rng.finish_sample(rng_state);
                out[j] = bipolar_to_unipolar(drift_value(from, target, phase)).clamp(0.0, 1.0);
            }
            *state(IDX_DRIFT) = from;
            *state(IDX_DRIFT_TARGET) = target;
            *state(IDX_DRIFT_PHASE) = phase;
        }
        _ => {
            for value in out.iter_mut() {
                *value = 0.0;
            }
            return;
        }
    }
    if rendered {
        rng.store(s, slot);
    }
}

impl BlockTransportClock {
    /// `(bar phase, clock advancing)` for frame `i` of the block.
    pub unsafe fn frame(&self, i: usize) -> (f32, bool) {
        let phase = if !self.phase_in.is_null() {
            normalize_phase(*self.phase_in.add(i))
        } else {
            transport_bar_phase_for_frame(
                self.param_phase,
                self.param_phase_inc,
                self.event_slice_start + i,
            )
        };
        let advancing = if !self.phase_inc_in.is_null() {
            *self.phase_inc_in.add(i) > 0.0
        } else {
            self.param_phase_inc > 0.0
        };
        (phase, advancing)
    }

}

impl BarTracker {
    pub unsafe fn load(s: *const f32) -> Self {
        Self {
            prev_bar_phase: *s.add(IDX_PREV_BAR_PHASE),
            bar_count: *s.add(IDX_BAR_COUNT),
        }
    }

    pub unsafe fn store(&self, s: *mut f32) {
        *s.add(IDX_PREV_BAR_PHASE) = self.prev_bar_phase;
        *s.add(IDX_BAR_COUNT) = self.bar_count;
    }

    /// A mod resync lands at a bar line, but the host clock's wrap and the
    /// scheduler's bar change are not sample-aligned: the reset frame may sit
    /// just before the wrap (phase near 1) or just after it (phase near 0).
    /// Latching the reset frame's phase stops the stale pre-reset phase from
    /// reading as a wrap, and a reset that arrives ahead of the wrap starts
    /// at -1 so the imminent wrap lands on bar 0 instead of bar 1.
    pub fn reset(&mut self, bar_phase: f32) {
        self.prev_bar_phase = bar_phase;
        self.bar_count = if bar_phase > 0.5 { -1.0 } else { 0.0 };
    }

    pub fn note_frame(&mut self, bar_phase: f32, clock_advancing: bool) {
        if clock_advancing && bar_phase < self.prev_bar_phase - 0.5 {
            self.bar_count += 1.0;
        }
        self.prev_bar_phase = bar_phase;
    }

}

