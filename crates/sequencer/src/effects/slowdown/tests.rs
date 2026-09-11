use super::*;
use std::ptr;

struct Harness(Vec<f64>);
impl Harness {
    fn new(sr: i32) -> Self {
        let mut h = Self(vec![0.0; state_size(sr as f32).div_ceil(8)]);
        unsafe {
            init(h.ptr(), sr, 512, ptr::null());
        }
        h
    }
    fn ptr(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }
    fn state(&mut self) -> &mut State {
        unsafe { &mut *self.ptr().cast::<State>() }
    }
    fn run(&mut self, input: &[[f32; 2]], block: usize) -> Vec<[f32; 2]> {
        self.run_with_sources(input, block, [0.0; MOD_SLOTS])
    }
    fn run_with_sources(
        &mut self,
        input: &[[f32; 2]],
        block: usize,
        sources: [f32; MOD_SLOTS],
    ) -> Vec<[f32; 2]> {
        let mut result = Vec::new();
        for chunk in input.chunks(block) {
            let mut left: Vec<_> = chunk.iter().map(|x| x[0]).collect();
            let mut right: Vec<_> = chunk.iter().map(|x| x[1]).collect();
            // The graph permits in-place buffers; explicitly exercise that ABI.
            let mut mods: [Vec<f32>; MOD_SLOTS] =
                std::array::from_fn(|slot| vec![sources[slot]; chunk.len()]);
            let buffers = [
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                mods[0].as_mut_ptr(),
                mods[1].as_mut_ptr(),
                mods[2].as_mut_ptr(),
                mods[3].as_mut_ptr(),
            ];
            unsafe {
                process(
                    buffers.as_ptr(),
                    buffers.as_ptr(),
                    chunk.len() as i32,
                    self.ptr(),
                    ptr::null_mut(),
                );
            }
            result.extend(left.into_iter().zip(right).map(|(l, r)| [l, r]));
        }
        result
    }
}

fn sine(n: usize, sr: f32, frequency: f32) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let x = (std::f32::consts::TAU * frequency * i as f32 / sr).sin() * 0.5;
            [x, -x]
        })
        .collect()
}

#[test]
fn descriptor_registers_persistent_modulation_controls() {
    let desc = EffectDescriptor::builtin_insert("slowdown").unwrap();
    assert_eq!(desc.name, "Slowdown");
    assert_eq!(desc.input_channels, 2 + MOD_SLOTS);
    assert_eq!(desc.instrument_modulators.len(), MOD_SLOTS);
    assert_eq!(
        desc.instrument_modulation_targets.len(),
        MOD_SLOTS * MOD_TARGETS.len()
    );
    assert_eq!(desc.output_channels, 2);
    assert!(super::super::builtin_effect_names().contains(&"Slowdown"));
    assert_eq!(
        super::super::builtin_effect_project_name("slowdown").as_deref(),
        Some("builtin:Slowdown")
    );
    assert_eq!(
        super::super::builtin_effect_name_from_project_name("builtin:Slowdown"),
        Some("Slowdown")
    );
    let slot = super::super::EffectSlotSnapshot::new_default_with_modulator(&desc, 1, 0);
    let mut h = Harness::new(48000);
    for (i, p) in desc.params.iter().enumerate() {
        if !voice_modulator::is_source_param(p.node_param_idx) {
            assert!((p.node_param_idx as usize) < DEPTH_BASE + MOD_TARGETS.len() * MOD_SLOTS);
            let stored = unsafe { *h.ptr().cast::<f32>().add(p.node_param_idx as usize) };
            assert_eq!(slot.defaults[i], stored);
        }
        assert!(p.default >= p.min && p.default <= p.max);
        assert_eq!(
            p.is_host_modulatable(),
            MOD_TARGETS.iter().any(|target| target.param == i)
        );
    }
    assert_eq!(desc.bpm_param_idx(), Some(PARAM_BPM));
    for target in &desc.instrument_modulation_targets {
        let depth = &desc.params[target.depth_param_idx];
        assert_eq!(
            depth.stored_to_user(0.25),
            0.25,
            "depth values and bounds share native units"
        );
        assert_eq!(target.mod_mode, ModulationMode::Additive);
    }
}

#[test]
fn every_modulator_input_drives_all_six_destinations_without_writing_base_values() {
    let input = sine(12000, 48000.0, 731.0);
    for slot in 0..MOD_SLOTS {
        let mut h = Harness::new(48000);
        h.state().params[SYNC] = 0.0;
        h.state().params[TONE] = 1000.0;
        h.state().params[MIX] = 0.5;
        let base = h.state().params;
        for (index, depth) in [0.1, 500.0, 1.0, 20.0, 1000.0, 0.1].into_iter().enumerate() {
            h.state().mod_depths[index][slot] = depth;
        }
        let mut source = [0.0; MOD_SLOTS];
        source[slot] = 1.0;
        let output = h.run_with_sources(&input, 127, source);
        let s = h.state();
        assert_eq!(s.params, base);
        assert!((s.speed - 0.6).abs() < 1e-6);
        assert!((s.wet - 0.6).abs() < 1e-6);
        assert!((s.tone - 2000.0).abs() < 1e-6);
        assert_eq!(s.period, 48000);
        assert_eq!(s.fade_length, 1920);
        assert_eq!(s.effective_params(source)[BEATS], 2.0);
        assert!(output.iter().flatten().all(|x| x.is_finite()));
        let mut reference = Harness::new(48000);
        reference.state().params = base;
        let reference = reference.run(&input, 127);
        assert!(output
            .iter()
            .zip(reference)
            .any(|(a, b)| (a[0] - b[0]).abs() > 0.1));
    }
}

#[test]
fn real_effect_modulator_node_drives_each_connected_slot() {
    use crate::audiograph as ag;
    use std::ffi::CString;
    struct Graph(ag::LiveGraphPtr);
    impl Drop for Graph {
        fn drop(&mut self) {
            unsafe {
                ag::destroy_live_graph(self.0 .0);
            }
        }
    }
    ag::initialize_engine_for_test(64, 8000);
    let label = CString::new("slowdown-modulation-audio").unwrap();
    let graph = Graph(ag::LiveGraphPtr(unsafe {
        ag::create_live_graph(32, 64, label.as_ptr(), 2)
    }));
    assert!(!graph.0 .0.is_null());
    let desc = descriptor();
    let effect = unsafe {
        ag::add_node(
            graph.0 .0,
            vtable(),
            state_size(8000.0),
            label.as_ptr(),
            desc.input_channels as i32,
            2,
            ptr::null(),
            0,
        )
    };
    let source = unsafe {
        ag::add_node(
            graph.0 .0,
            voice_modulator::effect_modulator_vtable(),
            voice_modulator::STATE_SIZE * 4,
            label.as_ptr(),
            voice_modulator::INPUT_COUNT as i32,
            MOD_SLOTS as i32,
            ptr::null(),
            0,
        )
    };
    assert!(effect > 0 && source > 0);
    let push = |node, idx, value| unsafe {
        ag::params_push_wrapper(
            graph.0 .0,
            ag::ParamMsg {
                logical_id: node as u64,
                idx: idx as u64,
                fvalue: value,
            },
        );
    };
    for slot in 0..MOD_SLOTS {
        assert!(unsafe {
            ag::graph_connect(graph.0 .0, source, slot as i32, effect, (2 + slot) as i32)
        });
        push(effect, DEPTH_BASE + slot, 0.2);
    }
    assert!(unsafe { ag::add_node_to_watchlist(graph.0 .0, effect) });
    let mut output = [0.0f32; 128];
    let mut captured = Harness::new(8000);
    for slot in 0..MOD_SLOTS {
        for source_slot in 0..MOD_SLOTS {
            push(
                source,
                voice_modulator::slot_source_param_idx(source_slot),
                if source_slot == slot { 1.0 } else { 0.0 },
            );
            push(
                source,
                voice_modulator::slot_param_idx(source_slot, voice_modulator::PARAM_LFO_RATE_HZ),
                5.0,
            );
        }
        let mut min = 1.0f64;
        let mut max = 0.0f64;
        for _ in 0..32 {
            for _ in 0..4 {
                unsafe {
                    graph.0.process_next_block(output.as_mut_ptr(), 64);
                }
            }
            let mut written = 0;
            if unsafe {
                ag::get_node_state_into(
                    graph.0 .0,
                    effect,
                    captured.ptr(),
                    state_size(8000.0),
                    &mut written,
                )
            } {
                let value = captured.state().speed;
                min = min.min(value);
                max = max.max(value);
            }
        }
        assert!(
            max - min > 0.15,
            "Mod {} did not reach the DSP: {min}..{max}",
            slot + 1
        );
    }
}

#[test]
fn playback_speed_tracks_fractional_ratio_and_preserves_stereo() {
    let sr = 48000.0;
    let input = sine(48000, sr, 1000.0);
    for speed in [0.25, 0.5, 0.73, 1.0] {
        let mut h = Harness::new(sr as i32);
        h.state().params[SPEED] = speed;
        h.state().params[SYNC] = 0.0;
        h.state().params[TIME] = 4000.0;
        unsafe {
            reset(h.ptr());
        }
        let output = h.run(&input, 137);
        let segment = &output[12000..44000];
        let crossings = segment
            .windows(2)
            .filter(|w| w[0][0] <= 0.0 && w[1][0] > 0.0)
            .count();
        let frequency = crossings as f32 * sr / segment.len() as f32;
        assert!(
            (frequency - 1000.0 * speed).abs() < 3.0,
            "speed={speed}, measured={frequency}"
        );
        let rms = (segment.iter().map(|x| x[0] * x[0]).sum::<f32>() / segment.len() as f32).sqrt();
        assert!(rms > 0.3 && rms < 0.36, "speed={speed}, rms={rms}");
        for frame in output {
            assert_eq!(frame[0], -frame[1]);
        }
    }
}

#[test]
fn block_partition_reset_and_migration_are_deterministic() {
    let input = sine(62000, 48000.0, 317.0);
    let mut a = Harness::new(48000);
    let mut b = Harness::new(48000);
    assert_eq!(a.run(&input, 1), b.run(&input, 511));
    let mut migrated = Harness::new(48000);
    unsafe {
        migrate(migrated.ptr(), a.ptr());
    }
    assert_eq!(a.run(&input[..4000], 64), migrated.run(&input[..4000], 99));
    unsafe {
        reset(a.ptr());
    }
    let mut fresh = Harness::new(48000);
    assert_eq!(a.run(&input[..8000], 113), fresh.run(&input[..8000], 211));
    let mut changed_rate = Harness::new(96000);
    unsafe {
        migrate(changed_rate.ptr(), b.ptr());
    }
    assert_eq!(changed_rate.state().sample_rate, 96000.0);
    assert_eq!(changed_rate.state().age, 0);
    assert!(changed_rate
        .run(&vec![[0.0; 2]; 1024], 64)
        .iter()
        .all(|x| *x == [0.0; 2]));
}

#[test]
fn tempo_and_length_changes_latch_without_interrupting_a_crossfade() {
    let mut h = Harness::new(48000);
    h.state().params[PARAM_BPM as usize] = 60.0;
    h.state().params[BEATS] = 0.125;
    h.run(&vec![[0.2; 2]; 3000], 71);
    assert_eq!(h.state().period, 6000);
    h.state().params[PARAM_BPM as usize] = 120.0;
    h.state().params[SMOOTH] = 100.0;
    h.run(&vec![[0.2; 2]; 3000], 127);
    assert_eq!(h.state().period, 6000);
    h.run(&[[0.2; 2]], 1);
    assert_eq!(h.state().period, 3000);
    assert_eq!(h.state().fade_length, 1500);
    h.state().params[TIME] = 40.0;
    h.state().params[SYNC] = 0.0;
    h.run(&vec![[0.2; 2]; 1500], 39);
    assert_eq!(h.state().period, 3000);
    assert_eq!(h.state().fade_length, 1500);
}

#[test]
fn modulation_is_finite_bounded_and_bypass_settles_to_exact_dry() {
    let mut h = Harness::new(48000);
    let input = sine(512, 48000.0, 89.0);
    for k in 0..160 {
        let s = h.state();
        s.params[SPEED] = if k % 2 == 0 { 0.25 } else { 1.0 };
        s.params[TIME] = if k % 3 == 0 { 4000.0 } else { 40.0 };
        s.params[SYNC] = 0.0;
        s.params[SMOOTH] = if k % 2 == 0 { 1.0 } else { 100.0 };
        s.params[TONE] = if k % 5 == 0 {
            f32::NAN
        } else {
            200.0 + k as f32 * 100.0
        };
        let out = h.run(&input, 32);
        assert!(out.iter().flatten().all(|v| v.is_finite() && v.abs() < 0.7));
        assert!(h.state().delay < (h.state().capacity - TAPS) as f64);
    }
    h.state().params[ENABLED] = 0.0;
    h.run(&vec![[0.0; 2]; 12000], 512);
    assert_eq!(h.run(&input, 64), input);
    for p in &mut h.state().params {
        *p = f32::INFINITY;
    }
    assert!(h.run(&input, 64).iter().flatten().all(|v| v.is_finite()));
}

#[test]
fn interpolation_wraps_preserves_dc_and_rejects_large_images() {
    let mut h = Harness::new(48000);
    let s = h.state();
    for weights in &s.table {
        assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }
    let capacity = s.capacity;
    unsafe {
        let buffer = ring(h.ptr());
        for i in 0..capacity {
            *buffer.add(i) = 0.25;
            *buffer.add(capacity + i) = -0.75;
        }
        h.state().write = 3;
        for delay in [17.0, 17.25, 19.73, 96000.5] {
            let sample = h.state().read(buffer, delay);
            assert!((sample[0] - 0.25).abs() < 1e-6);
            assert!((sample[1] + 0.75).abs() < 1e-6);
        }
        // At half speed the unwanted image of a 0.3 cycles/sample source is
        // at 0.35 cycles/output sample; a two-point reader leaks it badly.
        for i in 0..capacity {
            *buffer.add(i) = (std::f64::consts::TAU * 0.3 * i as f64).sin() as f32;
        }
        h.state().write = 20000;
        let mut wanted = [0.0f64; 2];
        let mut image = [0.0f64; 2];
        for i in 0..4000 {
            let sample = h.state().read(buffer, 10000.0 - i as f64 * 0.5)[0] as f64;
            for (acc, frequency) in [(&mut wanted, 0.15), (&mut image, 0.35)] {
                let phase = std::f64::consts::TAU * frequency * i as f64;
                acc[0] += sample * phase.cos();
                acc[1] += sample * phase.sin();
            }
        }
        assert!(image[0].hypot(image[1]) / wanted[0].hypot(wanted[1]) < 0.001);
    }
}

#[test]
fn automated_restarts_are_continuous_and_smoothers_reach_exact_endpoints() {
    let mut h = Harness::new(48000);
    h.state().params[SYNC] = 0.0;
    let input = sine(96000, 48000.0, 10.0);
    let mut output = Vec::new();
    for (i, chunk) in input.chunks(257).enumerate() {
        let s = h.state();
        s.params[SPEED] = if i % 2 == 0 { 0.25 } else { 1.0 };
        s.params[TIME] = [40.0, 60.0, 90.0][i % 3];
        s.params[SMOOTH] = if i % 2 == 0 { 1.0 } else { 100.0 };
        s.params[MIX] = if i % 3 == 0 { 0.0 } else { 1.0 };
        output.extend(h.run(chunk, 64));
    }
    let maximum_jump = output
        .windows(2)
        .map(|w| (w[1][0] - w[0][0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        maximum_jump < 0.035,
        "unexpected discontinuity: {maximum_jump}"
    );
    h.state().params[SPEED] = 1.0;
    h.state().params[MIX] = 1.0;
    h.run(&vec![[0.25; 2]; 12000], 128);
    assert_eq!(h.state().speed, 1.0);
    assert_eq!(h.state().wet, 1.0);
    let dc = h.run(&vec![[0.25; 2]; 12000], 64);
    assert!(dc.iter().flatten().all(|x| (*x - 0.25).abs() < 1e-5));
}

#[test]
fn history_capacity_covers_extreme_timing_at_all_sample_rates() {
    for sr in [8000, 44100, 48000, 96000, 192000, 384000] {
        let mut h = Harness::new(sr);
        h.state().params[BEATS] = 4.0;
        h.state().params[PARAM_BPM as usize] = 20.0;
        h.state().params[SMOOTH] = 100.0;
        let params = h.state().effective_params([0.0; MOD_SLOTS]);
        h.state().latch_cycle(&params);
        let s = h.state();
        let maximum_delay = BASE_DELAY + 0.75 * (s.period + s.fade_length) as f64;
        assert!(maximum_delay + (TAPS as f64) < s.capacity as f64);
    }
}

#[test]
fn transport_phase_param_is_registered_and_lands_after_the_depth_block() {
    let desc = descriptor();
    assert_eq!(
        desc.transport_phase_param_idx(),
        Some(PARAM_TRANSPORT_BEAT_PHASE as u32)
    );
    assert!(
        desc.params
            .iter()
            .all(|p| p.node_param_idx != PARAM_TRANSPORT_BEAT_PHASE as u32),
        "the transport phase input is hidden, not a user parameter"
    );
    let mut h = Harness::new(48_000);
    unsafe {
        *h.ptr()
            .cast::<f32>()
            .add(PARAM_TRANSPORT_BEAT_PHASE as usize) = 3.25;
    }
    assert_eq!(h.state().transport_phase, 3.25);
    assert_eq!(
        h.state().params,
        DEFAULTS,
        "the phase slot must not alias a parameter"
    );
    assert!(h.state().mod_depths.iter().flatten().all(|d| *d == 0.0));
}

/// Drive the effect the way the audio callback does: one block-start beat phase
/// per block, derived from an absolute transport sample position.
fn run_transport(
    h: &mut Harness,
    start_sample: u64,
    blocks: usize,
    block: usize,
    bpm: f64,
    playing: bool,
) -> Vec<usize> {
    let sr = 48_000.0;
    let input = sine(block, sr as f32, 220.0);
    let mut restarts = Vec::new();
    for b in 0..blocks {
        let total = start_sample + (b * block) as u64;
        let beats = if playing {
            total as f64 * bpm / (60.0 * sr)
        } else {
            0.0
        };
        h.state().transport_phase = super::super::dj_mixer::transport_beat_phase(beats);
        h.run(&input, block);
        // A restart at frame 0 leaves age == block, so use <= and let callers
        // warm the state past the first block before probing.
        let age = h.state().age;
        if age <= block {
            restarts.push((b + 1) * block - age);
        }
    }
    restarts
}

#[test]
fn synced_restarts_lock_to_the_transport_bar_grid_after_a_seek() {
    let mut h = Harness::new(48_000);
    h.state().params[SYNC] = 1.0;
    h.state().params[BEATS] = 4.0;
    let bar = 96_000u64; // four beats at 120 BPM, 48 kHz
    let seek = 7_000u64; // start mid-bar, not block aligned
                         // Warm up undriven so the seek's immediate restart is observable.
    h.run(&sine(4096, 48_000.0, 220.0), 512);
    let restarts = run_transport(&mut h, seek, 600, 512, 120.0, true);
    assert_eq!(restarts[0], 0, "a seek restarts the capture immediately");
    assert!(restarts.len() >= 4, "{restarts:?}");
    for (n, local) in restarts[1..].iter().enumerate() {
        let expected = (bar * (n as u64 + 1) - seek) as usize;
        assert!(
            local.abs_diff(expected) <= 1,
            "restart {n} at local {local}, expected bar boundary {expected}: {restarts:?}"
        );
    }
}

#[test]
fn stopped_transport_free_runs_and_play_resumes_the_grid() {
    let mut h = Harness::new(48_000);
    h.state().params[SYNC] = 1.0;
    h.state().params[BEATS] = 1.0;
    // Stopped: the host keeps pushing phase 0, so the tempo-derived period rules.
    let stopped = run_transport(&mut h, 0, 200, 512, 120.0, false);
    assert!(
        stopped.windows(2).all(|w| w[1] - w[0] == 24_000),
        "free-run cadence while stopped: {stopped:?}"
    );
    // Play from sample 0: the first block cannot be told apart from stopped,
    // then every restart sits on a beat in transport samples.
    let played = run_transport(&mut h, 0, 200, 512, 120.0, true);
    for local in &played[1..] {
        let off = local % 24_000;
        assert!(off <= 1 || off >= 23_999, "{played:?}");
    }
    assert!(played.len() >= 3, "{played:?}");
}
