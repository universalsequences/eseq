//! Silence propagation between graph kernels (see `ap_inputs_silent` in
//! graph_engine.c). A kernel whose inputs are all declared silent may skip
//! its DSP, provided it declares its outputs silent too, so chains of idle
//! buses, sends and meters stop costing a buffer pass per node (eseq-v6te).
//! Outside a graph kernel nothing reads as silent and nothing is declared.

use std::os::raw::c_int;

extern "C" {
    fn ap_inputs_silent() -> c_int;
    fn ap_set_output_silent(port: c_int);
    fn ap_output_was_silent(port: c_int) -> c_int;
}

/// Every input of the running kernel reads silence this block.
pub fn inputs_silent() -> bool {
    unsafe { ap_inputs_silent() != 0 }
}

/// Zero outputs `0..ports` unless already zero, and declare them silent.
/// Outside a graph this still zeroes (nothing is ever known silent there).
///
/// # Safety
/// `out` must hold `ports` output pointers (null ones are skipped), each
/// valid for `nframes` writes.
pub unsafe fn emit(out: *const *mut f32, ports: usize, nframes: c_int) {
    for port in 0..ports {
        let lane = *out.add(port);
        if !lane.is_null() {
            emit_port(port, std::slice::from_raw_parts_mut(lane, nframes.max(0) as usize));
        }
    }
}

/// Zero output `port` unless it already holds zeros over all of `lane`, and
/// declare it silent.
///
/// # Safety
/// `lane` must be the running kernel's buffer for output `port`.
pub unsafe fn emit_port(port: usize, lane: &mut [f32]) {
    if ap_output_was_silent(port as c_int) == 0 {
        lane.fill(0.0);
    }
    ap_set_output_silent(port as c_int);
}

/// Declare output `port` silent after the caller made it all zeros.
pub fn declare_port(port: usize) {
    unsafe { ap_set_output_silent(port as c_int) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiograph as graph;
    use std::ffi::{c_void, CString};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 0 = declared silence, 1 = constant 0.5 (undeclared), 2 = impulse at frame 0.
    static SOURCE_MODE: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn source_process(
        _inp: *const *mut f32,
        out: *const *mut f32,
        nframes: c_int,
        _state: *mut c_void,
        _buffers: *mut c_void,
    ) {
        let lane = std::slice::from_raw_parts_mut(*out, nframes as usize);
        match SOURCE_MODE.load(Ordering::Relaxed) {
            0 => emit_port(0, lane),
            1 => lane.fill(0.5),
            _ => {
                lane.fill(0.0);
                lane[0] = 1.0;
            }
        }
    }

    /// 0 = declared silence, 1 = constant 0.5 (undeclared).
    static VARIABLE_SOURCE_MODE: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn variable_source_process(
        _inp: *const *mut f32,
        out: *const *mut f32,
        nframes: c_int,
        _state: *mut c_void,
        _buffers: *mut c_void,
    ) {
        let lane = std::slice::from_raw_parts_mut(*out, nframes as usize);
        match VARIABLE_SOURCE_MODE.load(Ordering::Relaxed) {
            0 => emit_port(0, lane),
            _ => lane.fill(0.5),
        }
    }

    #[test]
    fn short_silent_pass_does_not_vouch_for_a_longer_one() {
        graph::initialize_engine_for_test(64, 48_000);
        let label = CString::new("silence-variable-pass").unwrap();
        let lg = unsafe { graph::create_live_graph(32, 64, label.as_ptr(), 1) };
        let source_name = CString::new("source").unwrap();
        let source = unsafe {
            graph::add_node(
                lg,
                graph::NodeVTable { process: Some(variable_source_process), ..graph::NodeVTable::default() },
                4, source_name.as_ptr(), 0, 1, std::ptr::null(), 0,
            )
        };
        assert!(source > 0);
        let gain_name = CString::new("gain").unwrap();
        let gain = unsafe { graph::add_gain_node(lg, 2.0, gain_name.as_ptr()) };
        unsafe {
            assert!(graph::graph_connect(lg, source, 0, gain, 0));
            assert!(graph::graph_connect(lg, gain, 0, 0, 0));
        }
        let mut render = |mode: u32, frames: usize| {
            VARIABLE_SOURCE_MODE.store(mode, Ordering::Relaxed);
            let mut output = vec![f32::NAN; frames];
            unsafe { graph::process_next_block(lg, output.as_mut_ptr(), frames as c_int) };
            output
        };
        assert!(render(1, 64).iter().all(|v| *v == 1.0), "gain 2 x 0.5");
        // A short silent pass clears only its own frames, so the rest of each
        // edge still holds the signal above.
        assert!(render(0, 16).iter().all(|v| *v == 0.0));
        let full = render(0, 64);
        assert!(full.iter().all(|v| *v == 0.0), "stale signal leaked: {full:?}");
        // Once a full pass is cleared, shorter and full silent passes stay clean.
        assert!(render(0, 16).iter().all(|v| *v == 0.0));
        assert!(render(0, 64).iter().all(|v| *v == 0.0));
        unsafe { graph::destroy_live_graph(lg) };
    }

    #[test]
    fn silence_never_masks_signal_and_delays_drain_before_going_silent() {
        graph::initialize_engine_for_test(64, 48_000);
        let label = CString::new("silence-propagation").unwrap();
        let lg = unsafe { graph::create_live_graph(32, 64, label.as_ptr(), 1) };
        let add = |vtable: graph::NodeVTable, state: usize, inputs: i32, outputs: i32, name: &str| {
            let name = CString::new(name).unwrap();
            let id = unsafe {
                graph::add_node(lg, vtable, state * 4, name.as_ptr(), inputs, outputs, std::ptr::null(), 0)
            };
            assert!(id > 0);
            id
        };
        let source = add(
            graph::NodeVTable { process: Some(source_process), ..graph::NodeVTable::default() },
            1, 0, 1, "source",
        );
        let gain_name = CString::new("gain").unwrap();
        let gain = unsafe { graph::add_gain_node(lg, 2.0, gain_name.as_ptr()) };
        let pdc = add(
            crate::effects::pdc_delay::pdc_delay_vtable(),
            crate::effects::pdc_delay::PDC_DELAY_STATE_SIZE, 2, 2, "pdc",
        );
        unsafe {
            assert!(graph::graph_connect(lg, source, 0, gain, 0));
            assert!(graph::graph_connect(lg, gain, 0, pdc, 0));
            assert!(graph::graph_connect(lg, pdc, 0, 0, 0));
            assert!(graph::params_push_wrapper(lg, graph::ParamMsg {
                idx: crate::effects::pdc_delay::PDC_PARAM_DELAY as u64,
                logical_id: pdc as u64,
                fvalue: 100.0,
            }));
        }
        let mut rendered = Vec::new();
        let mut render = |mode: u32, blocks: usize| {
            SOURCE_MODE.store(mode, Ordering::Relaxed);
            for _ in 0..blocks {
                let mut output = vec![f32::NAN; 64];
                unsafe { graph::process_next_block(lg, output.as_mut_ptr(), 64) };
                rendered.extend(output);
            }
        };
        render(0, 4); // settle into declared silence end to end
        render(1, 4); // undeclared signal must pass straight through
        render(0, 4); // back to silence: the 100-frame tail drains first
        render(2, 1); // an impulse from silence
        render(0, 4);
        unsafe { graph::destroy_live_graph(lg) };

        let block = |b: usize| &rendered[b * 64..(b + 1) * 64];
        assert!((0..4).all(|b| block(b).iter().all(|v| *v == 0.0)));
        // Signal enters at block 4 and appears 100 frames later (frame 356).
        let signal_start = 4 * 64 + 100;
        assert!(rendered[4 * 64..signal_start].iter().all(|v| *v == 0.0));
        assert!(rendered[signal_start..8 * 64].iter().all(|v| *v == 1.0), "gain 2 x 0.5");
        // Silence at block 8: 100 more frames of delayed signal still drain.
        assert!(rendered[8 * 64..8 * 64 + 100].iter().all(|v| *v == 1.0), "tail must drain");
        assert!(rendered[8 * 64 + 100..12 * 64].iter().all(|v| *v == 0.0));
        // Impulse at block 12 frame 0 arrives at frame 100 after it, as 2.0.
        let impulse = 12 * 64 + 100;
        assert_eq!(rendered[impulse], 2.0);
        assert!(rendered[12 * 64..].iter().enumerate()
            .all(|(i, v)| if 12 * 64 + i == impulse { true } else { *v == 0.0 }));
    }
}
