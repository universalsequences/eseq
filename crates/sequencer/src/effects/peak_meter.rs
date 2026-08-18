//! Lightweight stereo peak-meter sink for graph tap points.
//!
//! The node has no outputs and exists solely to expose post-routing audio
//! levels through the audiograph watchlist. Keeping its state small avoids
//! snapshotting the large delay rings owned by PDC nodes.

use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

pub const STATE_PEAK_L: usize = 0;
pub const STATE_PEAK_R: usize = 1;
pub const PEAK_METER_STATE_SIZE: usize = 2;

unsafe extern "C" fn peak_meter_init(
    state: *mut c_void,
    _sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    std::ptr::write_bytes(state as *mut f32, 0, PEAK_METER_STATE_SIZE);
}

unsafe extern "C" fn peak_meter_process(
    inp: *const *mut f32,
    _out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let left = *inp.add(0);
    let right = *inp.add(1);
    let mut peak_l = 0.0_f32;
    let mut peak_r = 0.0_f32;
    for frame in 0..nframes as usize {
        peak_l = peak_l.max((*left.add(frame)).abs());
        peak_r = peak_r.max((*right.add(frame)).abs());
    }
    *s.add(STATE_PEAK_L) = peak_l.max(*s.add(STATE_PEAK_L) * 0.92);
    *s.add(STATE_PEAK_R) = peak_r.max(*s.add(STATE_PEAK_R) * 0.92);
}

pub fn peak_meter_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(peak_meter_process),
        init: Some(peak_meter_init),
        ..NodeVTable::default()
    }
}

pub fn add_peak_meter_node(lg: *mut crate::audiograph::LiveGraph, name: &str) -> i32 {
    let node_name = std::ffi::CString::new(name).unwrap_or_default();
    unsafe {
        crate::audiograph::add_node(
            lg,
            peak_meter_vtable(),
            PEAK_METER_STATE_SIZE * std::mem::size_of::<f32>(),
            node_name.as_ptr(),
            2,
            0,
            std::ptr::null(),
            0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_stereo_block_peaks_and_decays_previous_peak() {
        let mut state = [0.0_f32; PEAK_METER_STATE_SIZE];
        unsafe {
            peak_meter_init(state.as_mut_ptr().cast(), 48_000, 64, std::ptr::null());
        }
        let mut left = [0.25_f32, -0.75, 0.5];
        let mut right = [-0.2_f32, 0.4, 0.9];
        let inputs = [left.as_mut_ptr(), right.as_mut_ptr()];
        unsafe {
            peak_meter_process(
                inputs.as_ptr(),
                std::ptr::null(),
                left.len() as c_int,
                state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }
        assert_eq!(state, [0.75, 0.9]);

        left.fill(0.0);
        right.fill(0.0);
        unsafe {
            peak_meter_process(
                inputs.as_ptr(),
                std::ptr::null(),
                left.len() as c_int,
                state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }
        assert_eq!(state, [0.75 * 0.92, 0.9 * 0.92]);
    }
}
