use crate::audiograph::*;
use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::path::Path;

// State layout indices (f32 slots)
const STATE_BUFFER_ID: usize = 0;
const STATE_PLAYHEAD: usize = 1;
const STATE_PLAYING: usize = 2;
const STATE_GAIN: usize = 3;
const STATE_VELOCITY: usize = 4;
const STATE_SPEED: usize = 5;
const STATE_GATE_SAMPLES: usize = 6; // absolute gate length in samples (computed by audio callback)
const STATE_TRANSPOSE: usize = 7;
const STATE_ATTACK_SAMPLES: usize = 8;
const STATE_RELEASE_SAMPLES: usize = 9;
const STATE_GATE_MODE: usize = 10; // 1.0=gate on, 0.0=gate off
                                   // Persistent envelope state (not settable via params)
const STATE_ENV_PHASE: usize = 11; // 0=idle, 1=attack, 2=sustain, 3=release
const STATE_ENV_LEVEL: usize = 12; // current envelope amplitude 0.0–1.0
const STATE_RELEASE_LEVEL: usize = 13; // level when release began (for linear ramp)
const STATE_GATE_COUNTER: usize = 14; // real-time sample counter for gate duration (increments by 1/sample, not by playback rate)
const STATE_LAST_OUT_L: usize = 15; // last emitted left sample, used for click-free retrigger smoothing
const STATE_LAST_OUT_R: usize = 16; // last emitted right sample, used for click-free retrigger smoothing
const STATE_RETRIGGER_OUT_L: usize = 17; // captured left sample at retrigger start
const STATE_RETRIGGER_OUT_R: usize = 18; // captured right sample at retrigger start
const SAMPLER_STATE_SIZE: usize = 19;

// Envelope phase constants
const ENV_IDLE: f32 = 0.0;
const ENV_ATTACK: f32 = 1.0;
const ENV_SUSTAIN: f32 = 2.0;
const ENV_RELEASE: f32 = 3.0;
const ENV_RETRIGGER: f32 = 4.0; // smooth fade-down before re-attack

// Minimum release to prevent clicks (in samples, ~1.5ms at 44100)
const MIN_RELEASE_SAMPLES: f32 = 64.0;
// Retrigger crossfade duration (~1ms at 44100). Fades old content to 0 before new attack.
const RETRIGGER_FADE_SAMPLES: f32 = 48.0;
// Minimum attack applied after retrigger to prevent click on ramp-up (~0.2ms at 44100).
// Fresh triggers from silence use the user's attack value directly (even if 0).
const MIN_RETRIGGER_ATTACK: f32 = 8.0;

// Param indices (match state layout for direct write)
pub const PARAM_PLAYHEAD: u64 = STATE_PLAYHEAD as u64;
pub const PARAM_TRIGGER: u64 = STATE_PLAYING as u64;
pub const PARAM_VELOCITY: u64 = STATE_VELOCITY as u64;
pub const PARAM_SPEED: u64 = STATE_SPEED as u64;
pub const PARAM_GATE_SAMPLES: u64 = STATE_GATE_SAMPLES as u64;
pub const PARAM_TRANSPOSE: u64 = STATE_TRANSPOSE as u64;
pub const PARAM_ATTACK_SAMPLES: u64 = STATE_ATTACK_SAMPLES as u64;
pub const PARAM_RELEASE_SAMPLES: u64 = STATE_RELEASE_SAMPLES as u64;
pub const PARAM_GATE_MODE: u64 = STATE_GATE_MODE as u64;
pub const PARAM_BUFFER_ID: u64 = STATE_BUFFER_ID as u64;

pub struct SamplerTrack {
    pub name: String,
    pub node_id: i32,
    pub logical_id: u64,
    pub buffer_id: i32,
}

/// extern "C" init — called by audiograph when node is created.
unsafe extern "C" fn sampler_init(
    state: *mut c_void,
    _sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    let s = state as *mut f32;
    if !initial_state.is_null() {
        let init = initial_state as *const f32;
        *s.add(STATE_BUFFER_ID) = *init.add(0);
    }
    *s.add(STATE_PLAYHEAD) = 0.0;
    *s.add(STATE_PLAYING) = 0.0;
    *s.add(STATE_GAIN) = 0.8;
    *s.add(STATE_VELOCITY) = 1.0;
    *s.add(STATE_SPEED) = 1.0;
    *s.add(STATE_GATE_SAMPLES) = f32::MAX; // ungated by default until first trigger
    *s.add(STATE_TRANSPOSE) = 0.0;
    *s.add(STATE_ATTACK_SAMPLES) = 0.0;
    *s.add(STATE_RELEASE_SAMPLES) = 0.0;
    *s.add(STATE_GATE_MODE) = 1.0; // gate on by default
    *s.add(STATE_ENV_PHASE) = ENV_IDLE;
    *s.add(STATE_ENV_LEVEL) = 0.0;
    *s.add(STATE_RELEASE_LEVEL) = 0.0;
    *s.add(STATE_GATE_COUNTER) = 0.0;
    *s.add(STATE_LAST_OUT_L) = 0.0;
    *s.add(STATE_LAST_OUT_R) = 0.0;
    *s.add(STATE_RETRIGGER_OUT_L) = 0.0;
    *s.add(STATE_RETRIGGER_OUT_R) = 0.0;
}

/// extern "C" process — reads sample data from buffer, writes to output.
///
/// Envelope state machine (persists across blocks):
///   IDLE → (trigger) → ATTACK → SUSTAIN → (gate-off) → RELEASE → IDLE
///
/// gate_samples=0 is treated as an explicit note-off regardless of gate_mode,
/// so keyboard release always triggers the release phase.
unsafe extern "C" fn sampler_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let buffer_id = (*s.add(STATE_BUFFER_ID)) as usize;
    let mut playhead = *s.add(STATE_PLAYHEAD);
    let playing = *s.add(STATE_PLAYING);
    let gain = *s.add(STATE_GAIN);
    let velocity = *s.add(STATE_VELOCITY);
    let speed = *s.add(STATE_SPEED);
    let gate_samples = *s.add(STATE_GATE_SAMPLES);
    let transpose = *s.add(STATE_TRANSPOSE);
    let attack_samples = *s.add(STATE_ATTACK_SAMPLES);
    let release_samples = *s.add(STATE_RELEASE_SAMPLES);
    let gate_mode = *s.add(STATE_GATE_MODE);
    let mut env_phase = *s.add(STATE_ENV_PHASE);
    let mut env_level = *s.add(STATE_ENV_LEVEL);
    let mut release_level = *s.add(STATE_RELEASE_LEVEL);
    let mut gate_counter = *s.add(STATE_GATE_COUNTER);
    let mut last_out_l = *s.add(STATE_LAST_OUT_L);
    let mut last_out_r = *s.add(STATE_LAST_OUT_R);
    let mut retrigger_out_l = *s.add(STATE_RETRIGGER_OUT_L);
    let mut retrigger_out_r = *s.add(STATE_RETRIGGER_OUT_R);

    let buf_desc = buffers as *const BufferDesc;
    let desc = &*buf_desc.add(buffer_id);
    let sample_data = desc.buffer;
    let sample_len = desc.size as usize;

    let out0 = *out.add(0);
    let out1 = *out.add(1);
    let nf = nframes as usize;
    let channel_count = desc.channel_count.max(1) as usize;

    if playing <= 0.0 || sample_data.is_null() || sample_len == 0 {
        for i in 0..nf {
            *out0.add(i) = 0.0;
            *out1.add(i) = 0.0;
        }
        *s.add(STATE_LAST_OUT_L) = 0.0;
        *s.add(STATE_LAST_OUT_R) = 0.0;
        return;
    }

    let effective_rate = speed * (2.0_f32).powf(transpose / 12.0);
    let amplitude = velocity * gain;
    let eff_release = release_samples.max(MIN_RELEASE_SAMPLES);
    // After a retrigger fade, use a small minimum attack to avoid click on ramp-up.
    // For fresh triggers from silence this flag stays false → attack=0 stays punchy.
    let mut post_retrigger = false;

    // ── Trigger detection ──
    // playhead==0 means params just reset it. Distinguish fresh vs retrigger:
    if playhead == 0.0 && env_phase != ENV_RETRIGGER {
        gate_counter = 0.0; // reset real-time duration counter
        if env_level > 0.001 || last_out_l.abs() > 0.000_1 || last_out_r.abs() > 0.000_1 {
            // Voice was still audible → fade the actual previous output to zero
            // before starting the new waveform attack.
            env_phase = ENV_RETRIGGER;
            env_level = 1.0;
            release_level = 1.0;
            retrigger_out_l = last_out_l;
            retrigger_out_r = last_out_r;
        } else {
            // Voice was silent → clean attack from 0
            env_phase = ENV_ATTACK;
            env_level = 0.0;
            retrigger_out_l = 0.0;
            retrigger_out_r = 0.0;
        }
    }

    // ── Pre-loop gate-off check (gate may have changed between blocks) ──
    if env_phase == ENV_ATTACK || env_phase == ENV_SUSTAIN {
        if gate_samples <= 0.0 {
            env_phase = ENV_RELEASE;
            release_level = env_level;
        } else if gate_mode > 0.5 && gate_counter >= gate_samples {
            env_phase = ENV_RELEASE;
            release_level = env_level;
        }
    }

    for i in 0..nf {
        // Past end of sample data: stop
        if playhead >= sample_len as f32 || playhead < 0.0 {
            *out0.add(i) = 0.0;
            *out1.add(i) = 0.0;
            env_phase = ENV_IDLE;
            env_level = 0.0;
            *s.add(STATE_PLAYING) = 0.0;
            for j in (i + 1)..nf {
                *out0.add(j) = 0.0;
                *out1.add(j) = 0.0;
            }
            break;
        }

        // ── Envelope state machine (per sample) ──
        // Uses chained `if` (not else-if) so phase transitions within a
        // single sample flow through immediately (e.g. retrigger→attack).

        if env_phase == ENV_RETRIGGER {
            // Fade the previously emitted sample to zero, then begin the new attack.
            // The new playhead advances underneath this silent crossfade so we do not
            // jump directly from the old waveform to the new one.
            *out0.add(i) = retrigger_out_l * env_level;
            *out1.add(i) = retrigger_out_r * env_level;
            last_out_l = *out0.add(i);
            last_out_r = *out1.add(i);

            env_level -= 1.0 / RETRIGGER_FADE_SAMPLES;
            playhead += effective_rate;
            gate_counter += 1.0;
            if env_level <= 0.0 {
                env_level = 0.0;
                env_phase = ENV_ATTACK;
                post_retrigger = true;
                last_out_l = 0.0;
                last_out_r = 0.0;
            }
            continue;
        }

        if env_phase == ENV_ATTACK {
            // After retrigger, enforce a minimum attack to prevent click
            // on the ramp back up (sample data at playhead≈48 != 0).
            // Fresh triggers from silence keep attack=0 for max punch.
            let eff_attack = if post_retrigger {
                attack_samples.max(MIN_RETRIGGER_ATTACK)
            } else {
                attack_samples
            };
            if eff_attack > 0.0 {
                env_level += 1.0 / eff_attack;
            } else {
                env_level = 1.0;
            }
            if env_level >= 1.0 {
                env_level = 1.0;
                env_phase = ENV_SUSTAIN;
            }
        }

        if env_phase == ENV_SUSTAIN {
            if gate_samples <= 0.0 {
                // Explicit note-off (keyboard release)
                env_phase = ENV_RELEASE;
                release_level = env_level;
            } else if gate_mode > 0.5 && gate_counter >= gate_samples {
                // Duration gating (real-time counter, independent of playback rate)
                env_phase = ENV_RELEASE;
                release_level = env_level;
            } else if gate_mode <= 0.5 && playhead >= (sample_len as f32 - eff_release) {
                // Auto-release near end of sample (gate off = play full sample)
                env_phase = ENV_RELEASE;
                release_level = env_level;
            }
        }

        if env_phase == ENV_RELEASE {
            if release_level > 0.0 {
                env_level -= release_level / eff_release;
            }
            if env_level <= 0.0 {
                env_level = 0.0;
                env_phase = ENV_IDLE;
                *s.add(STATE_PLAYING) = 0.0;
                *out0.add(i) = 0.0;
                *out1.add(i) = 0.0;
                last_out_l = 0.0;
                last_out_r = 0.0;
                for j in (i + 1)..nf {
                    *out0.add(j) = 0.0;
                    *out1.add(j) = 0.0;
                }
                break;
            }
        }

        // ── Read sample with linear interpolation ──

        let idx = playhead as usize;
        let frac = playhead - idx as f32;
        let sample_index = idx * channel_count;
        let next_sample_index = (idx + 1) * channel_count;
        let s0_l = if idx < sample_len {
            *sample_data.add(sample_index)
        } else {
            0.0
        };
        let s1_l = if idx + 1 < sample_len {
            *sample_data.add(next_sample_index)
        } else {
            0.0
        };
        let s0_r = if channel_count > 1 && idx < sample_len {
            *sample_data.add(sample_index + 1)
        } else {
            s0_l
        };
        let s1_r = if channel_count > 1 && idx + 1 < sample_len {
            *sample_data.add(next_sample_index + 1)
        } else {
            s1_l
        };
        let sample_l = s0_l + frac * (s1_l - s0_l);
        let sample_r = s0_r + frac * (s1_r - s0_r);
        let env_amp = amplitude * env_level;

        *out0.add(i) = sample_l * env_amp;
        *out1.add(i) = sample_r * env_amp;
        last_out_l = *out0.add(i);
        last_out_r = *out1.add(i);

        playhead += effective_rate;
        gate_counter += 1.0; // real-time counter (1 per sample, independent of transpose/speed)
    }

    // Write back persistent state
    *s.add(STATE_PLAYHEAD) = playhead;
    *s.add(STATE_ENV_PHASE) = env_phase;
    *s.add(STATE_ENV_LEVEL) = env_level;
    *s.add(STATE_RELEASE_LEVEL) = release_level;
    *s.add(STATE_GATE_COUNTER) = gate_counter;
    *s.add(STATE_LAST_OUT_L) = last_out_l;
    *s.add(STATE_LAST_OUT_R) = last_out_r;
    *s.add(STATE_RETRIGGER_OUT_L) = retrigger_out_l;
    *s.add(STATE_RETRIGGER_OUT_R) = retrigger_out_r;
}

pub fn sampler_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(sampler_process),
        init: Some(sampler_init),
        reset: None,
        migrate: None,
    }
}

/// Load a WAV file into an audiograph buffer.
/// Returns `(buffer_id, file_stem_name)`.
pub fn load_wav_buffer(lg: *mut LiveGraph, wav_path: &Path) -> Result<(i32, String), String> {
    let reader =
        hound::WavReader::open(wav_path).map_err(|e| format!("Failed to open WAV: {e}"))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;

    let samples_f32: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max_val = (1u32 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / max_val)
                .collect()
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
    };

    let stereo: Vec<f32> = if channels == 1 {
        samples_f32
            .iter()
            .flat_map(|&sample| [sample, sample])
            .collect()
    } else if channels == 2 {
        samples_f32
    } else {
        samples_f32
            .chunks(channels)
            .flat_map(|ch| {
                let left = ch.iter().step_by(2).copied().sum::<f32>()
                    / ch.iter().step_by(2).count() as f32;
                let right = ch.iter().skip(1).step_by(2).copied().sum::<f32>()
                    / ch.iter().skip(1).step_by(2).count().max(1) as f32;
                [left, right]
            })
            .collect()
    };

    // Skip leading silence: scan with 64-sample RMS windows, threshold -60dB (~0.001)
    let skip = {
        const WINDOW: usize = 64;
        const THRESHOLD: f32 = 0.001;
        let thresh_sq = THRESHOLD * THRESHOLD;
        let mut start = 0usize;
        for chunk in stereo.chunks(WINDOW * 2) {
            let frames = chunk.len() / 2;
            let sum_sq: f32 = chunk.iter().map(|s| s * s).sum();
            if frames > 0 && sum_sq / (frames * 2) as f32 > thresh_sq {
                break;
            }
            start += frames;
        }
        start.min(stereo.len() / 2)
    };
    let trimmed = &stereo[skip * 2..];

    let buffer_id = unsafe { create_buffer(lg, (trimmed.len() / 2) as c_int, 2, trimmed.as_ptr()) };
    if buffer_id < 0 {
        return Err("Failed to create buffer".to_string());
    }

    let name = wav_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sample")
        .to_string();

    Ok((buffer_id, name))
}

/// Create a sampler node from an existing buffer ID.
pub fn create_sampler_node(
    lg: *mut LiveGraph,
    buffer_id: i32,
    name: &str,
) -> Result<SamplerTrack, String> {
    let initial_state = [buffer_id as f32];
    let c_name = CString::new(name).unwrap();

    let node_id = unsafe {
        add_node(
            lg,
            sampler_vtable(),
            SAMPLER_STATE_SIZE * std::mem::size_of::<f32>(),
            c_name.as_ptr(),
            0,
            2,
            initial_state.as_ptr() as *const c_void,
            initial_state.len() * std::mem::size_of::<f32>(),
        )
    };

    if node_id < 0 {
        return Err("Failed to add sampler node".to_string());
    }

    Ok(SamplerTrack {
        name: name.to_string(),
        node_id,
        logical_id: node_id as u64,
        buffer_id,
    })
}

/// Load a WAV file, create an audiograph buffer and sampler node.
pub fn create_sampler_track(lg: *mut LiveGraph, wav_path: &Path) -> Result<SamplerTrack, String> {
    let (buffer_id, name) = load_wav_buffer(lg, wav_path)?;
    create_sampler_node(lg, buffer_id, &name)
}
