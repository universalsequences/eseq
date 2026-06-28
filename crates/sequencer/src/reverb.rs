use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

// Attribution:
// This reverb algorithm is based on Sapphire Galaxy for VCV Rack by Don Cross (cosinekitty)
// which is itself based on Airwindows Galactic by Chris Johnson.

// ── Constants ──

const NDELAYS: usize = 13;

/// Delay buffer sizes (stereo frames) — relatively prime to minimize periodic artifacts.
const DELAY_BUF_SIZES: [usize; NDELAYS] = [
    9700, 6000, 2320, 940, // Bank 0 (indices 0-3)
    15220, 8460, 4540, 3200, // Bank 1 (indices 4-7)
    6480, 3660, 1720, 680,  // Bank 2 (indices 8-11)
    3111, // Chorus delay (index 12)
];

/// Tank sizes for delay length calculation based on "bigness" knob.
const TANK_SIZES: [usize; 12] = [
    4801, 2909, 1153, 461, 7607, 4217, 2269, 1597, 3407, 1823, 859, 331,
];

// ── State layout indices (f32 slots) ──

const ST_REPLACE: usize = 0;
const ST_BRIGHT: usize = 1;
const ST_DETUNE: usize = 2;
const ST_BIGNESS: usize = 3;
const ST_MIX: usize = 4;
const ST_FPD0: usize = 5;
const ST_FPD1: usize = 6;
const ST_QUALITY: usize = 7;
const ST_VIBM: usize = 8;
const ST_OLDFPD: usize = 9;
const ST_CYCLE: usize = 10;
const ST_SAMPLE_RATE: usize = 11;
const ST_DELAY_META: usize = 12; // 13 delays × 2 (count, length) = 26 floats
const ST_FEEDBACK: usize = 38; // 4 stereo frames = 8 floats
const ST_IIR_A_L: usize = 46;
const ST_IIR_A_R: usize = 47;
const ST_IIR_B_L: usize = 48;
const ST_IIR_B_R: usize = 49;
const ST_LASTREF: usize = 50; // 5 stereo frames = 10 floats
const ST_BUFS: usize = 60; // Delay buffer data starts here

/// Total number of f32 slots for all delay buffers (stereo = 2 floats per frame).
const fn total_buf_floats() -> usize {
    let mut total = 0;
    let mut i = 0;
    while i < NDELAYS {
        total += DELAY_BUF_SIZES[i] * 2;
        i += 1;
    }
    total
}

const ST_ENABLED: usize = ST_BUFS + total_buf_floats();
pub const REVERB_STATE_SIZE: usize = ST_ENABLED + 1;

// Public param indices for UI control
pub const REVERB_PARAM_REPLACE: u64 = ST_REPLACE as u64;
pub const REVERB_PARAM_BRIGHT: u64 = ST_BRIGHT as u64;
pub const REVERB_PARAM_SIZE: u64 = ST_BIGNESS as u64;
pub const REVERB_PARAM_ENABLED: u64 = ST_ENABLED as u64;

/// Pre-computed buffer offsets (from index 0 of state array).
const fn buf_offsets() -> [usize; NDELAYS] {
    let mut offsets = [0usize; NDELAYS];
    let mut offset = ST_BUFS;
    let mut i = 0;
    while i < NDELAYS {
        offsets[i] = offset;
        offset += DELAY_BUF_SIZES[i] * 2;
        i += 1;
    }
    offsets
}

const BUF_OFFSETS: [usize; NDELAYS] = buf_offsets();

// ── Helper functions ──

#[inline(always)]
fn square(x: f32) -> f32 {
    x * x
}

#[inline(always)]
fn cube(x: f32) -> f32 {
    x * x * x
}

#[inline(always)]
fn clamp4(x: f32) -> f32 {
    x.clamp(-4.0, 4.0)
}

#[inline(always)]
fn clamp1(x: f32) -> f32 {
    x.clamp(-1.0, 1.0)
}

#[inline(always)]
fn fast_sin(x: f32) -> f32 {
    // Use standard sin — the Swift version uses a lookup table but std sin
    // is fast enough for 4x sub-sampled processing.
    x.sin()
}

// ── Init ──

unsafe extern "C" fn reverb_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;

    // Zero entire state
    for i in 0..REVERB_STATE_SIZE {
        *s.add(i) = 0.0;
    }

    // Hardcoded defaults for send-return usage
    *s.add(ST_REPLACE) = 0.3;
    *s.add(ST_BRIGHT) = 0.8;
    *s.add(ST_DETUNE) = 0.1;
    *s.add(ST_BIGNESS) = 0.2;
    *s.add(ST_MIX) = 1.0; // Pure wet (send return)

    // LFSR seeds
    *s.add(ST_FPD0) = f32::from_bits(2756923396u32);
    *s.add(ST_FPD1) = f32::from_bits(2341963165u32);

    // Low quality mode (4x sub-sampling)
    *s.add(ST_QUALITY) = 2.0;

    *s.add(ST_VIBM) = 3.0;
    *s.add(ST_OLDFPD) = 429496.7295;
    *s.add(ST_CYCLE) = 0.0;
    *s.add(ST_SAMPLE_RATE) = sample_rate as f32;

    // Initialize delay metadata: count=0, length=bufSize/2
    for i in 0..NDELAYS {
        let meta = ST_DELAY_META + i * 2;
        *s.add(meta) = 0.0; // count
        *s.add(meta + 1) = (DELAY_BUF_SIZES[i] / 2) as f32; // initial length
    }
    *s.add(ST_ENABLED) = 1.0;
}

// ── Process ──

unsafe extern "C" fn reverb_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let nf = nframes as usize;

    let in_l = *inp.add(0);
    let in_r = *inp.add(1);
    let out0 = *out.add(0); // L output
    let out1 = *out.add(1); // R output

    if *s.add(ST_ENABLED) <= 0.5 {
        std::ptr::copy_nonoverlapping(in_l as *const f32, out0, nf);
        std::ptr::copy_nonoverlapping(in_r as *const f32, out1, nf);
        return;
    }

    // Read sample rate
    let mut sample_rate = *s.add(ST_SAMPLE_RATE);
    if sample_rate < 8000.0 || sample_rate > 192000.0 {
        sample_rate = 44100.0;
    }
    let overallscale = sample_rate / 44100.0;

    // Quality mode: 2 = low (4x sub-sampling)
    let quality_mode = (*s.add(ST_QUALITY)) as i32;
    let quality_mult = 1 << quality_mode;
    let base_cycle_end = (overallscale.floor() as i32).clamp(1, 4);
    let cycle_end = base_cycle_end * quality_mult;

    // Read mutable state
    let mut vib_m = *s.add(ST_VIBM);
    let mut oldfpd = *s.add(ST_OLDFPD);
    let mut cycle = (*s.add(ST_CYCLE)) as i32;
    let mut fpd0 = (*s.add(ST_FPD0)).to_bits();
    let _fpd1 = (*s.add(ST_FPD1)).to_bits();

    if fpd0 == 0 {
        fpd0 = 2756923396;
    }

    // IIR states
    let mut iir_a_l = *s.add(ST_IIR_A_L);
    let mut iir_a_r = *s.add(ST_IIR_A_R);
    let mut iir_b_l = *s.add(ST_IIR_B_L);
    let mut iir_b_r = *s.add(ST_IIR_B_R);

    // Feedback (4 stereo frames)
    let mut fb0_l = *s.add(ST_FEEDBACK);
    let mut fb0_r = *s.add(ST_FEEDBACK + 1);
    let mut fb1_l = *s.add(ST_FEEDBACK + 2);
    let mut fb1_r = *s.add(ST_FEEDBACK + 3);
    let mut fb2_l = *s.add(ST_FEEDBACK + 4);
    let mut fb2_r = *s.add(ST_FEEDBACK + 5);
    let mut fb3_l = *s.add(ST_FEEDBACK + 6);
    let mut fb3_r = *s.add(ST_FEEDBACK + 7);

    // LastRef (5 stereo frames for interpolation)
    let mut lr0_l = *s.add(ST_LASTREF);
    let mut lr0_r = *s.add(ST_LASTREF + 1);
    let mut lr1_l = *s.add(ST_LASTREF + 2);
    let mut lr1_r = *s.add(ST_LASTREF + 3);
    let mut lr2_l = *s.add(ST_LASTREF + 4);
    let mut lr2_r = *s.add(ST_LASTREF + 5);
    let mut lr3_l = *s.add(ST_LASTREF + 6);
    let mut lr3_r = *s.add(ST_LASTREF + 7);
    let mut lr4_l = *s.add(ST_LASTREF + 8);
    let mut lr4_r = *s.add(ST_LASTREF + 9);

    // Delay counts and lengths (13 each)
    let mut counts = [0i32; NDELAYS];
    let mut lengths = [0i32; NDELAYS];
    for i in 0..NDELAYS {
        counts[i] = (*s.add(ST_DELAY_META + i * 2)) as i32;
        lengths[i] = (*s.add(ST_DELAY_META + i * 2 + 1)) as i32;
    }

    // Read parameters
    let replace_knob = *s.add(ST_REPLACE);
    let bright_knob = *s.add(ST_BRIGHT);
    let detune_knob = *s.add(ST_DETUNE);
    let bigness_knob = *s.add(ST_BIGNESS);
    let mix_knob = *s.add(ST_MIX);

    // Derived values
    let regen = 0.0625 + ((1.0 - replace_knob) * 0.0625);
    let attenuate = (1.0 - (regen / 0.125)) * 1.333;
    let sqrt_scale = overallscale.max(0.1).sqrt();
    let lowpass = square(1.00001 - (1.0 - bright_knob)) / sqrt_scale;
    let drift = cube(detune_knob) * 0.001;
    let size = (bigness_knob * 1.77) + 0.1;
    let one_minus_lp = 1.0 - lowpass;

    // Update tank sizes
    for i in 0..12 {
        lengths[i] = ((TANK_SIZES[i] as f32 * size) as i32)
            .max(1)
            .min(DELAY_BUF_SIZES[i] as i32 - 1);
    }
    lengths[12] = 256; // Chorus delay fixed

    // Vibrato phase increment
    let vib_increment = oldfpd * drift;
    let two_pi = std::f32::consts::PI * 2.0;
    let pi_over_2 = std::f32::consts::PI * 0.5;

    // Inline write_head and read_tail closures operate on the state pointer `s`
    // They are unsafe and expect valid indices.

    for i in 0..nf {
        let dry_l = *in_l.add(i);
        let dry_r = *in_r.add(i);

        // Update vibrato modulation
        vib_m += vib_increment;
        if vib_m > two_pi {
            vib_m -= two_pi;
            // LFSR update
            fpd0 ^= fpd0 << 13;
            fpd0 ^= fpd0 >> 17;
            fpd0 ^= fpd0 << 5;
            oldfpd = 0.4294967295 + fpd0 as f32 * 0.0000000000618;
        }

        // Write attenuated input to chorus delay (delay 12)
        {
            let ofs = BUF_OFFSETS[12];
            let buf_size = DELAY_BUF_SIZES[12] as i32;
            let write_pos = counts[12].max(0).min(buf_size - 1) as usize;
            let idx = ofs + write_pos * 2;
            *s.add(idx) = clamp4(dry_l * attenuate);
            *s.add(idx + 1) = clamp4(dry_r * attenuate);
            counts[12] += 1;
            if counts[12] < 0 || counts[12] > lengths[12] {
                counts[12] = 0;
            }
        }

        // Read from chorus delay with vibrato-modulated interpolation
        let sin_l = fast_sin(vib_m);
        let sin_r = fast_sin(vib_m + pi_over_2);
        let ofs_l = (sin_l + 1.0) * 127.0;
        let ofs_r = (sin_r + 1.0) * 127.0;
        let i_ofs_l = ofs_l as i32;
        let i_ofs_r = ofs_r as i32;
        let frc_l = ofs_l - i_ofs_l as f32;
        let frc_r = ofs_r - i_ofs_r as f32;

        let count12 = counts[12];
        let len12 = lengths[12].max(1);
        let buf12 = DELAY_BUF_SIZES[12] as i32;
        let ofs12 = BUF_OFFSETS[12];

        // Helper: reverse lookup for chorus delay
        let rev12 = |pos: i32| -> usize {
            let rev_ofs = if pos > len12 { len12 + 1 } else { 0 };
            (pos - rev_ofs).max(0).min(buf12 - 1) as usize
        };

        // Left channel interpolation
        let base_l = count12 + i_ofs_l;
        let idx1_l = rev12(base_l);
        let idx2_l = rev12(base_l + 1);
        let v1_l = *s.add(ofs12 + idx1_l * 2);
        let v2_l = *s.add(ofs12 + idx2_l * 2);
        let phasor_l = v1_l + (v2_l - v1_l) * frc_l;

        // Right channel interpolation
        let base_r = count12 + i_ofs_r;
        let idx1_r = rev12(base_r);
        let idx2_r = rev12(base_r + 1);
        let v1_r = *s.add(ofs12 + idx1_r * 2 + 1);
        let v2_r = *s.add(ofs12 + idx2_r * 2 + 1);
        let phasor_r = v1_r + (v2_r - v1_r) * frc_r;

        // IIR lowpass A
        iir_a_l = iir_a_l * one_minus_lp + phasor_l * lowpass;
        iir_a_r = iir_a_r * one_minus_lp + phasor_r * lowpass;
        let sample_l = iir_a_l;
        let sample_r = iir_a_r;

        // Sub-sampled processing
        cycle += 1;
        if cycle >= cycle_end {
            // Macro for write_head
            macro_rules! write_head {
                ($di:expr, $val_l:expr, $val_r:expr) => {{
                    let ofs = BUF_OFFSETS[$di];
                    let buf_sz = DELAY_BUF_SIZES[$di] as i32;
                    let wp = counts[$di].max(0).min(buf_sz - 1) as usize;
                    let idx = ofs + wp * 2;
                    *s.add(idx) = clamp4($val_l);
                    *s.add(idx + 1) = clamp4($val_r);
                    counts[$di] += 1;
                    if counts[$di] < 0 || counts[$di] > lengths[$di] {
                        counts[$di] = 0;
                    }
                }};
            }

            // Macro for read_tail
            macro_rules! read_tail {
                ($di:expr) => {{
                    let ofs = BUF_OFFSETS[$di];
                    let buf_sz = DELAY_BUF_SIZES[$di] as i32;
                    let cnt = counts[$di];
                    let len = lengths[$di];
                    if len <= 0 {
                        (0.0f32, 0.0f32)
                    } else {
                        let rev_ofs = if cnt > len { len + 1 } else { 0 };
                        let rp = (cnt - rev_ofs).max(0).min(buf_sz - 1) as usize;
                        let idx = ofs + rp * 2;
                        (*s.add(idx), *s.add(idx + 1))
                    }
                }};
            }

            // Write to bank 2 (delays 8-11) with cross-channel flipped feedback
            write_head!(8, sample_l + fb0_r * regen, sample_r + fb0_l * regen);
            write_head!(9, sample_l + fb1_r * regen, sample_r + fb1_l * regen);
            write_head!(10, sample_l + fb2_r * regen, sample_r + fb2_l * regen);
            write_head!(11, sample_l + fb3_r * regen, sample_r + fb3_l * regen);

            // Read bank 2, Hadamard stir, write to bank 0
            let (t8_l, t8_r) = read_tail!(8);
            let (t9_l, t9_r) = read_tail!(9);
            let (t10_l, t10_r) = read_tail!(10);
            let (t11_l, t11_r) = read_tail!(11);

            let sum2_l = t8_l + t9_l + t10_l + t11_l;
            let sum2_r = t8_r + t9_r + t10_r + t11_r;

            write_head!(0, t8_l + t8_l - sum2_l, t8_r + t8_r - sum2_r);
            write_head!(1, t9_l + t9_l - sum2_l, t9_r + t9_r - sum2_r);
            write_head!(2, t10_l + t10_l - sum2_l, t10_r + t10_r - sum2_r);
            write_head!(3, t11_l + t11_l - sum2_l, t11_r + t11_r - sum2_r);

            // Read bank 0, Hadamard stir, write to bank 1
            let (t0_l, t0_r) = read_tail!(0);
            let (t1_l, t1_r) = read_tail!(1);
            let (t2_l, t2_r) = read_tail!(2);
            let (t3_l, t3_r) = read_tail!(3);

            let sum0_l = t0_l + t1_l + t2_l + t3_l;
            let sum0_r = t0_r + t1_r + t2_r + t3_r;

            write_head!(4, t0_l + t0_l - sum0_l, t0_r + t0_r - sum0_r);
            write_head!(5, t1_l + t1_l - sum0_l, t1_r + t1_r - sum0_r);
            write_head!(6, t2_l + t2_l - sum0_l, t2_r + t2_r - sum0_r);
            write_head!(7, t3_l + t3_l - sum0_l, t3_r + t3_r - sum0_r);

            // Read bank 1 for feedback and output
            let (f0_l, f0_r) = read_tail!(4);
            let (f1_l, f1_r) = read_tail!(5);
            let (f2_l, f2_r) = read_tail!(6);
            let (f3_l, f3_r) = read_tail!(7);

            let sum1_l = f0_l + f1_l + f2_l + f3_l;
            let sum1_r = f0_r + f1_r + f2_r + f3_r;

            // Update feedback with stir + soft clip
            fb0_l = clamp4(f0_l + f0_l - sum1_l);
            fb0_r = clamp4(f0_r + f0_r - sum1_r);
            fb1_l = clamp4(f1_l + f1_l - sum1_l);
            fb1_r = clamp4(f1_r + f1_r - sum1_r);
            fb2_l = clamp4(f2_l + f2_l - sum1_l);
            fb2_r = clamp4(f2_r + f2_r - sum1_r);
            fb3_l = clamp4(f3_l + f3_l - sum1_l);
            fb3_r = clamp4(f3_r + f3_r - sum1_r);

            // Output from bank 1 sum
            let sum_l = sum1_l * 0.125;
            let sum_r = sum1_r * 0.125;

            // Update lastRef interpolation (cycleEnd=4 for low quality at 44.1k)
            match cycle_end {
                4 => {
                    lr0_l = lr4_l;
                    lr0_r = lr4_r;
                    lr2_l = (lr0_l + sum_l) * 0.5;
                    lr2_r = (lr0_r + sum_r) * 0.5;
                    lr1_l = (lr0_l + lr2_l) * 0.5;
                    lr1_r = (lr0_r + lr2_r) * 0.5;
                    lr3_l = (lr2_l + sum_l) * 0.5;
                    lr3_r = (lr2_r + sum_r) * 0.5;
                    lr4_l = sum_l;
                    lr4_r = sum_r;
                }
                3 => {
                    lr0_l = lr3_l;
                    lr0_r = lr3_r;
                    lr2_l = (lr0_l + lr0_l + sum_l) / 3.0;
                    lr2_r = (lr0_r + lr0_r + sum_r) / 3.0;
                    lr1_l = (lr0_l + sum_l + sum_l) / 3.0;
                    lr1_r = (lr0_r + sum_r + sum_r) / 3.0;
                    lr3_l = sum_l;
                    lr3_r = sum_r;
                }
                2 => {
                    lr0_l = lr2_l;
                    lr0_r = lr2_r;
                    lr1_l = (lr0_l + sum_l) * 0.5;
                    lr1_r = (lr0_r + sum_r) * 0.5;
                    lr2_l = sum_l;
                    lr2_r = sum_r;
                }
                _ => {
                    lr0_l = sum_l;
                    lr0_r = sum_r;
                }
            }

            cycle = 0;
        }

        // Get interpolated output based on cycle position
        let (wet_l, wet_r) = match cycle {
            0 => (lr0_l, lr0_r),
            1 => (lr1_l, lr1_r),
            2 => (lr2_l, lr2_r),
            3 => (lr3_l, lr3_r),
            _ => (lr4_l, lr4_r),
        };

        // Final IIR lowpass B
        iir_b_l = iir_b_l * one_minus_lp + wet_l * lowpass;
        iir_b_r = iir_b_r * one_minus_lp + wet_r * lowpass;

        // Wet/dry mix
        let wet = 1.0 - cube(1.0 - mix_knob);
        let out_l = iir_b_l * wet + dry_l * (1.0 - wet);
        let out_r = iir_b_r * wet + dry_r * (1.0 - wet);

        // Soft clamp and write output
        *out0.add(i) = clamp1(out_l);
        *out1.add(i) = clamp1(out_r);
    }

    // Write back mutable state
    *s.add(ST_VIBM) = vib_m;
    *s.add(ST_OLDFPD) = oldfpd;
    *s.add(ST_CYCLE) = cycle as f32;
    *s.add(ST_FPD0) = f32::from_bits(fpd0);

    *s.add(ST_IIR_A_L) = iir_a_l;
    *s.add(ST_IIR_A_R) = iir_a_r;
    *s.add(ST_IIR_B_L) = iir_b_l;
    *s.add(ST_IIR_B_R) = iir_b_r;

    *s.add(ST_FEEDBACK) = fb0_l;
    *s.add(ST_FEEDBACK + 1) = fb0_r;
    *s.add(ST_FEEDBACK + 2) = fb1_l;
    *s.add(ST_FEEDBACK + 3) = fb1_r;
    *s.add(ST_FEEDBACK + 4) = fb2_l;
    *s.add(ST_FEEDBACK + 5) = fb2_r;
    *s.add(ST_FEEDBACK + 6) = fb3_l;
    *s.add(ST_FEEDBACK + 7) = fb3_r;

    *s.add(ST_LASTREF) = lr0_l;
    *s.add(ST_LASTREF + 1) = lr0_r;
    *s.add(ST_LASTREF + 2) = lr1_l;
    *s.add(ST_LASTREF + 3) = lr1_r;
    *s.add(ST_LASTREF + 4) = lr2_l;
    *s.add(ST_LASTREF + 5) = lr2_r;
    *s.add(ST_LASTREF + 6) = lr3_l;
    *s.add(ST_LASTREF + 7) = lr3_r;
    *s.add(ST_LASTREF + 8) = lr4_l;
    *s.add(ST_LASTREF + 9) = lr4_r;

    // Write back delay counts and lengths
    for di in 0..NDELAYS {
        *s.add(ST_DELAY_META + di * 2) = counts[di] as f32;
        *s.add(ST_DELAY_META + di * 2 + 1) = lengths[di] as f32;
    }
}

pub fn reverb_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(reverb_process),
        init: Some(reverb_init),
        reset: None,
        migrate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn init_state() -> Vec<f32> {
        let mut state = vec![0.0; REVERB_STATE_SIZE];
        unsafe {
            reverb_init(state.as_mut_ptr().cast(), 44_100, 64, ptr::null());
        }
        state
    }

    #[test]
    fn disabled_reverb_passes_stereo_inputs_independently() {
        let mut state = init_state();
        state[ST_ENABLED] = 0.0;

        let mut in_l = [0.25, -0.5, 0.75, -1.0];
        let mut in_r = [-0.125, 0.375, -0.625, 0.875];
        let inputs = [in_l.as_mut_ptr(), in_r.as_mut_ptr()];
        let mut out_l = [0.0; 4];
        let mut out_r = [0.0; 4];
        let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];

        unsafe {
            reverb_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                in_l.len() as c_int,
                state.as_mut_ptr().cast(),
                ptr::null_mut(),
            );
        }

        assert_eq!(out_l, in_l);
        assert_eq!(out_r, in_r);
    }
}
