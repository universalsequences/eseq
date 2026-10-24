//! Silence propagation between graph kernels (see `ap_inputs_silent` in
//! graph_engine.c). A kernel whose inputs are all declared silent may skip
//! its DSP, provided it declares its outputs silent too, so chains of idle
//! buses, sends and meters stop costing a buffer pass per node (eseq-v6te).
//! Outside a graph kernel nothing reads as silent and nothing is declared.

use std::os::raw::c_int;

extern "C" {
    fn ap_inputs_silent() -> c_int;
    fn ap_input_silent(port: c_int) -> c_int;
    fn ap_current_node_ninputs() -> c_int;
    fn ap_current_node_noutputs() -> c_int;
    fn ap_current_node_state_writes() -> u32;
    fn ap_set_output_silent(port: c_int);
    fn ap_output_was_silent(port: c_int) -> c_int;
    fn ap_emit_silence(out: *const *mut f32, nframes: c_int);
}

/// Every input of the running kernel reads silence this block.
pub fn inputs_silent() -> bool {
    unsafe { ap_inputs_silent() != 0 }
}

/// Input `port` of the running kernel reads silence this block (unconnected,
/// or its source declared it). False outside a graph kernel.
pub fn input_silent(port: usize) -> bool {
    unsafe { ap_input_silent(port as c_int) != 0 }
}

/// `(inputs, outputs)` the running kernel's node was built with; `(0, 0)`
/// outside a graph kernel.
pub fn node_ports() -> (usize, usize) {
    unsafe {
        (
            ap_current_node_ninputs().max(0) as usize,
            ap_current_node_noutputs().max(0) as usize,
        )
    }
}

/// Changes whenever a param or bulk state write lands in the running node's
/// state. 0 outside a graph kernel.
pub fn node_state_writes() -> u32 {
    unsafe { ap_current_node_state_writes() }
}

/// Bypass copy of inputs `0..ports` to the same outputs that keeps silence
/// flowing: a port whose input is declared silent is emitted silent (its
/// buffer holds zeros, so the output is identical), any other is copied.
/// Outside a graph this is a plain copy.
///
/// # Safety
/// `inp` and `out` must hold `ports` pointers, each valid for `nframes`
/// reads (inputs) or writes (outputs); null pairs are skipped and a port may
/// be in place (input and output the same buffer).
pub unsafe fn passthrough(inp: *const *mut f32, out: *const *mut f32, ports: usize, nframes: c_int) {
    let nf = nframes.max(0) as usize;
    for port in 0..ports {
        let (src, dst) = (*inp.add(port), *out.add(port));
        if src.is_null() || dst.is_null() {
            continue;
        }
        if input_silent(port) {
            emit_port(port, std::slice::from_raw_parts_mut(dst, nf));
        } else if src != dst {
            // `copy`, not `copy_nonoverlapping`: the graph ABI allows in-place.
            std::ptr::copy(src as *const f32, dst, nf);
        }
    }
}

/// [`passthrough`] that also replaces non-finite samples with 0, for an
/// effect whose dry path has always sanitized its input.
///
/// # Safety
/// As for [`passthrough`].
pub unsafe fn passthrough_sanitized(inp: *const *mut f32, out: *const *mut f32, ports: usize, nframes: c_int) {
    let nf = nframes.max(0) as usize;
    for port in 0..ports {
        let (src, dst) = (*inp.add(port), *out.add(port));
        if src.is_null() || dst.is_null() {
            continue;
        }
        if input_silent(port) {
            emit_port(port, std::slice::from_raw_parts_mut(dst, nf));
        } else {
            // Raw reads and writes: the graph ABI allows in-place buffers.
            for frame in 0..nf {
                let x = *src.add(frame);
                *dst.add(frame) = if x.is_finite() { x } else { 0.0 };
            }
        }
    }
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

/// Like [`emit`] over every output port the running node was built with, for
/// a kernel that cannot trust its own idea of its port count. Outside a graph
/// this does nothing.
///
/// # Safety
/// `out` must be the running kernel's output pointers, each valid for
/// `nframes` writes.
pub unsafe fn emit_node_outputs(out: *const *mut f32, nframes: c_int) {
    ap_emit_silence(out, nframes);
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
        let render = |mode: u32, frames: usize| {
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

    /// 0 = declared silence, 1 = constant 0.5 (undeclared), 2 = 1.0 at frame 0.
    static FEED_MODE: AtomicU32 = AtomicU32::new(0);
    /// Whether the observer's inputs read silent on its last block.
    static OBSERVED_SILENT: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn feed_process(
        _inp: *const *mut f32,
        out: *const *mut f32,
        nframes: c_int,
        _state: *mut c_void,
        _buffers: *mut c_void,
    ) {
        for port in 0..2 {
            let lane = std::slice::from_raw_parts_mut(*out.add(port), nframes as usize);
            match FEED_MODE.load(Ordering::Relaxed) {
                0 => emit_port(port, lane),
                1 => lane.fill(0.5),
                _ => {
                    lane.fill(0.0);
                    lane[0] = 1.0;
                }
            }
        }
    }

    /// Copies input 0 to its output and records whether its inputs read silent.
    unsafe extern "C" fn observer_process(
        inp: *const *mut f32,
        out: *const *mut f32,
        nframes: c_int,
        _state: *mut c_void,
        _buffers: *mut c_void,
    ) {
        OBSERVED_SILENT.store(inputs_silent() as u32, Ordering::Relaxed);
        std::ptr::copy(*inp as *const f32, *out, nframes as usize);
    }

    struct Rig {
        lg: *mut graph::LiveGraph,
        observer: i32,
    }

    impl Rig {
        fn new(label: &str) -> Self {
            graph::initialize_engine_for_test(64, 48_000);
            let label = CString::new(label).unwrap();
            let lg = unsafe { graph::create_live_graph(32, 64, label.as_ptr(), 1) };
            let mut rig = Rig { lg, observer: 0 };
            rig.observer = rig.add(
                graph::NodeVTable { process: Some(observer_process), ..graph::NodeVTable::default() },
                4, 2, 1, "observer",
            );
            unsafe { assert!(graph::graph_connect(lg, rig.observer, 0, 0, 0)) };
            rig
        }
        fn add(&self, vtable: graph::NodeVTable, state_bytes: usize, inputs: i32, outputs: i32, name: &str) -> i32 {
            let name = CString::new(name).unwrap();
            let id = unsafe {
                graph::add_node(self.lg, vtable, state_bytes, name.as_ptr(), inputs, outputs, std::ptr::null(), 0)
            };
            assert!(id > 0);
            id
        }
        fn feed(&self) -> i32 {
            self.add(
                graph::NodeVTable { process: Some(feed_process), ..graph::NodeVTable::default() },
                4, 0, 2, "feed",
            )
        }
        /// Render one block; returns the output and whether the observer's
        /// inputs read declared silence.
        fn render(&self) -> (Vec<f32>, bool) {
            let mut output = vec![f32::NAN; 64];
            unsafe { graph::process_next_block(self.lg, output.as_mut_ptr(), 64) };
            (output, OBSERVED_SILENT.load(Ordering::Relaxed) == 1)
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            unsafe { graph::destroy_live_graph(self.lg) };
        }
    }

    #[test]
    fn bypass_passthrough_keeps_silence_flowing_and_copies_signal() {
        let rig = Rig::new("silence-bypass-passthrough");
        let feed = rig.feed();
        let filter = rig.add(
            crate::effects::filter::filter_vtable(),
            crate::effects::filter::FILTER_STATE_SIZE * 4, 6, 2, "filter",
        );
        unsafe {
            assert!(graph::graph_connect(rig.lg, feed, 0, filter, 0));
            assert!(graph::graph_connect(rig.lg, feed, 1, filter, 1));
            assert!(graph::graph_connect(rig.lg, filter, 0, rig.observer, 0));
            assert!(graph::graph_connect(rig.lg, filter, 1, rig.observer, 1));
            assert!(graph::params_push_wrapper(rig.lg, graph::ParamMsg {
                idx: crate::effects::filter::FILTER_PARAM_ENABLED,
                logical_id: filter as u64,
                fvalue: 0.0,
            }));
        }
        FEED_MODE.store(0, Ordering::Relaxed);
        for _ in 0..3 {
            let (output, silent) = rig.render();
            assert!(output.iter().all(|v| *v == 0.0));
            assert!(silent, "a bypassed effect must pass declared silence on");
        }
        FEED_MODE.store(1, Ordering::Relaxed);
        let (output, silent) = rig.render();
        assert!(output.iter().all(|v| *v == 0.5), "bypass must copy the signal exactly");
        assert!(!silent);
        FEED_MODE.store(0, Ordering::Relaxed);
        let (output, silent) = rig.render();
        assert!(output.iter().all(|v| *v == 0.0) && silent, "stale signal leaked: {output:?}");
    }

    #[test]
    fn idle_sampler_voice_declares_silence() {
        use crate::instruments::sampler;
        let rig = Rig::new("silence-idle-sampler");
        let sample = vec![0.5f32; 8192];
        let buffer = unsafe { graph::create_buffer(rig.lg, 4096, 2, sample.as_ptr()) };
        assert!(buffer >= 0);
        let voice = sampler::create_sampler_node(rig.lg, buffer, 48_000, "idle-voice").unwrap();
        unsafe {
            assert!(graph::graph_connect(rig.lg, voice.node_id, 0, rig.observer, 0));
            assert!(graph::graph_connect(rig.lg, voice.node_id, 1, rig.observer, 1));
        }
        for _ in 0..4 {
            let (output, silent) = rig.render();
            assert!(output.iter().all(|v| *v == 0.0));
            assert!(silent, "a sampler voice with no triggers must declare silence");
        }
    }

    #[test]
    fn settled_bypassed_chorus_declares_silence_and_copies_signal() {
        let rig = Rig::new("silence-bypassed-chorus");
        let feed = rig.feed();
        let chorus = rig.add(
            crate::effects::chorus::vtable(),
            crate::effects::chorus::state_size(48_000.0),
            2 + crate::instruments::voice_modulator::SLOT_COUNT as i32, 2, "chorus",
        );
        unsafe {
            assert!(graph::graph_connect(rig.lg, feed, 0, chorus, 0));
            assert!(graph::graph_connect(rig.lg, feed, 1, chorus, 1));
            assert!(graph::graph_connect(rig.lg, chorus, 0, rig.observer, 0));
            assert!(graph::graph_connect(rig.lg, chorus, 1, rig.observer, 1));
            assert!(graph::params_push_wrapper(rig.lg, graph::ParamMsg {
                idx: 0, logical_id: chorus as u64, fvalue: 0.0,
            }));
        }
        // The disable slews the wet out first (about 0.25 s), then bypass skips.
        FEED_MODE.store(1, Ordering::Relaxed);
        for _ in 0..400 {
            rig.render();
        }
        let (output, silent) = rig.render();
        assert!(output.iter().all(|v| *v == 0.5), "settled bypass must pass the dry signal: {output:?}");
        assert!(!silent);
        FEED_MODE.store(0, Ordering::Relaxed);
        for _ in 0..2 {
            let (output, silent) = rig.render();
            assert!(output.iter().all(|v| *v == 0.0) && silent);
        }
    }
}
