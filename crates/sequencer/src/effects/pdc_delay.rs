//! Plugin-delay-compensation node: a stereo integer-sample delay inserted at
//! graph join points so parallel paths with different effect latencies sum in
//! phase. The delay amount is host-written (via `write_node_state` at
//! [`PDC_PARAM_DELAY`]) whenever the graph's latency plan is recomputed; the
//! ring capacity is fixed at init so delay changes never allocate on the audio
//! thread.

use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const STATE_DELAY: usize = 0;
const STATE_CAPACITY: usize = 1;
const STATE_WRITE_POS: usize = 2;
const STATE_RING: usize = 3;

/// Maximum compensable delay in samples per node. Latency plans that exceed
/// this are clamped (and logged by the planner); at 44.1k this is ~0.37s,
/// far above any current builtin effect latency (Filter Table: 2048).
pub const PDC_MAX_DELAY_SAMPLES: usize = 16384;

pub const PDC_CHANNELS: usize = 2;

/// State float count: header + interleaved-per-channel rings.
pub const PDC_DELAY_STATE_SIZE: usize = STATE_RING + PDC_CHANNELS * PDC_MAX_DELAY_SAMPLES;

/// Float offset of the delay amount inside the node state, for
/// `write_node_state` updates from the latency planner.
pub const PDC_PARAM_DELAY: usize = STATE_DELAY;

unsafe extern "C" fn pdc_init(
    state: *mut c_void,
    _sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    let s = state as *mut f32;
    // State memory arrives zeroed; only the header needs explicit values.
    let initial_delay = if initial_state.is_null() {
        0.0
    } else {
        *(initial_state as *const f32)
    };
    *s.add(STATE_DELAY) = initial_delay;
    *s.add(STATE_CAPACITY) = PDC_MAX_DELAY_SAMPLES as f32;
    *s.add(STATE_WRITE_POS) = 0.0;
}

unsafe extern "C" fn pdc_reset(state: *mut c_void) {
    let s = state as *mut f32;
    *s.add(STATE_WRITE_POS) = 0.0;
    std::ptr::write_bytes(
        s.add(STATE_RING),
        0,
        PDC_CHANNELS * PDC_MAX_DELAY_SAMPLES,
    );
}

unsafe extern "C" fn pdc_migrate(new_state: *mut c_void, old_state: *const c_void) {
    // Same layout on both sides: preserve the ring so a hot swap does not
    // truncate in-flight compensated audio.
    std::ptr::copy_nonoverlapping(
        old_state as *const f32,
        new_state as *mut f32,
        PDC_DELAY_STATE_SIZE,
    );
}

unsafe extern "C" fn pdc_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let capacity = PDC_MAX_DELAY_SAMPLES;
    let delay = (*s.add(STATE_DELAY)).max(0.0).round() as usize;
    let delay = delay.min(capacity - 1);
    let mut write_pos = (*s.add(STATE_WRITE_POS)) as usize % capacity;

    if delay == 0 {
        for ch in 0..PDC_CHANNELS {
            let src = *inp.add(ch);
            let dst = *out.add(ch);
            std::ptr::copy_nonoverlapping(src, dst, nframes as usize);
        }
        // Keep the ring warm so raising the delay later replays real history
        // instead of a burst of zeros followed by a jump.
        for i in 0..nframes as usize {
            for ch in 0..PDC_CHANNELS {
                let ring = s.add(STATE_RING + ch * capacity);
                *ring.add(write_pos) = *(*inp.add(ch)).add(i);
            }
            write_pos = (write_pos + 1) % capacity;
        }
        *s.add(STATE_WRITE_POS) = write_pos as f32;
        return;
    }

    for i in 0..nframes as usize {
        let read_pos = (write_pos + capacity - delay) % capacity;
        for ch in 0..PDC_CHANNELS {
            let ring = s.add(STATE_RING + ch * capacity);
            *ring.add(write_pos) = *(*inp.add(ch)).add(i);
            *(*out.add(ch)).add(i) = *ring.add(read_pos);
        }
        write_pos = (write_pos + 1) % capacity;
    }
    *s.add(STATE_WRITE_POS) = write_pos as f32;
}

/// Add a zero-delay PDC node to a live graph. Returns the node id, or a
/// negative id on queue failure (callers treat that like other add_node
/// failures).
pub fn add_pdc_node(lg: *mut crate::audiograph::LiveGraph, name: &str) -> i32 {
    let node_name = std::ffi::CString::new(name.to_string()).unwrap_or_default();
    let initial_delay = 0.0f32;
    unsafe {
        crate::audiograph::add_node(
            lg,
            pdc_delay_vtable(),
            PDC_DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
            node_name.as_ptr(),
            PDC_CHANNELS as i32,
            PDC_CHANNELS as i32,
            (&initial_delay as *const f32).cast(),
            std::mem::size_of::<f32>(),
        )
    }
}

pub fn pdc_delay_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(pdc_process),
        init: Some(pdc_init),
        reset: Some(pdc_reset),
        migrate: Some(pdc_migrate),
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_block(state: &mut [f32], input: &[Vec<f32>], nframes: usize) -> Vec<Vec<f32>> {
        let mut inputs: Vec<Vec<f32>> = input.to_vec();
        let mut outputs: Vec<Vec<f32>> = vec![vec![0.0; nframes]; PDC_CHANNELS];
        let in_ptrs: Vec<*mut f32> = inputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let out_ptrs: Vec<*mut f32> = outputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        unsafe {
            pdc_process(
                in_ptrs.as_ptr(),
                out_ptrs.as_ptr(),
                nframes as c_int,
                state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }
        outputs
    }

    fn init_state(delay: f32) -> Vec<f32> {
        let mut state = vec![0.0f32; PDC_DELAY_STATE_SIZE];
        unsafe {
            pdc_init(
                state.as_mut_ptr().cast(),
                44_100,
                64,
                (&delay as *const f32).cast(),
            );
        }
        state
    }

    #[test]
    fn zero_delay_is_a_bit_exact_passthrough() {
        let mut state = init_state(0.0);
        let input: Vec<Vec<f32>> = (0..PDC_CHANNELS)
            .map(|ch| (0..64).map(|i| (i as f32) + 100.0 * ch as f32).collect())
            .collect();
        let out = run_block(&mut state, &input, 64);
        assert_eq!(out, input);
    }

    #[test]
    fn impulse_is_delayed_by_exactly_the_configured_samples() {
        let delay = 37usize;
        let mut state = init_state(delay as f32);
        let mut input = vec![vec![0.0f32; 64]; PDC_CHANNELS];
        input[0][3] = 1.0;
        input[1][5] = -1.0;
        let first = run_block(&mut state, &input, 64);
        let silence = vec![vec![0.0f32; 64]; PDC_CHANNELS];
        let second = run_block(&mut state, &silence, 64);

        let mut expected_l = vec![0.0f32; 128];
        expected_l[3 + delay] = 1.0;
        let mut expected_r = vec![0.0f32; 128];
        expected_r[5 + delay] = -1.0;
        let got_l: Vec<f32> = first[0].iter().chain(second[0].iter()).copied().collect();
        let got_r: Vec<f32> = first[1].iter().chain(second[1].iter()).copied().collect();
        assert_eq!(got_l, expected_l);
        assert_eq!(got_r, expected_r);
    }

    #[test]
    fn delay_spanning_multiple_blocks_stays_sample_exact() {
        let delay = 200usize;
        let mut state = init_state(delay as f32);
        let mut collected = Vec::new();
        for block in 0..6 {
            let mut input = vec![vec![0.0f32; 64]; PDC_CHANNELS];
            if block == 0 {
                input[0][0] = 1.0;
            }
            let out = run_block(&mut state, &input, 64);
            collected.extend(out[0].iter().copied());
        }
        let hit: Vec<usize> = collected
            .iter()
            .enumerate()
            .filter(|(_, v)| **v != 0.0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(hit, vec![delay]);
    }

    #[test]
    fn raising_delay_mid_stream_replays_ring_history() {
        let mut state = init_state(0.0);
        let ramp: Vec<Vec<f32>> = (0..PDC_CHANNELS)
            .map(|_| (0..64).map(|i| i as f32 + 1.0).collect())
            .collect();
        let _ = run_block(&mut state, &ramp, 64);
        state[PDC_PARAM_DELAY] = 10.0;
        let silence = vec![vec![0.0f32; 64]; PDC_CHANNELS];
        let out = run_block(&mut state, &silence, 64);
        // The first 10 output samples replay the tail of the previous block
        // (samples 55..64 of the ramp), not zeros.
        let expected_head: Vec<f32> = (55..65).map(|v| v as f32).collect();
        assert_eq!(&out[0][..10], expected_head.as_slice());
        assert!(out[0][10..].iter().all(|v| *v == 0.0));
    }
}
