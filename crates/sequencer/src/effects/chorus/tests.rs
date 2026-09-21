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
    fn run(&mut self, input: &[[f32; 2]], block: usize, sources: [f32; SLOTS]) -> Vec<[f32; 2]> {
        let mut result = Vec::new();
        for chunk in input.chunks(block) {
            let mut left: Vec<_> = chunk.iter().map(|x| x[0]).collect();
            let mut right: Vec<_> = chunk.iter().map(|x| x[1]).collect();
            let mut mods: [Vec<f32>; SLOTS] =
                std::array::from_fn(|i| vec![sources[i]; chunk.len()]);
            let buffers = [
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                mods[0].as_mut_ptr(),
                mods[1].as_mut_ptr(),
                mods[2].as_mut_ptr(),
                mods[3].as_mut_ptr(),
            ];
            // Deliberately in-place, as permitted by the graph ABI.
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
fn signal(n: usize, sr: f32) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            [
                (t * 997.0 * std::f32::consts::TAU).sin() * 0.3,
                (t * 331.0 * std::f32::consts::TAU).sin() * 0.2,
            ]
        })
        .collect()
}

#[test]
fn registry_and_parameter_abi_match_all_modulation_targets() {
    let d = EffectDescriptor::builtin_insert("chorus").unwrap();
    assert!(super::super::builtin_effect_names().contains(&"Chorus"));
    assert_eq!(
        super::super::builtin_effect_project_name("Chorus").as_deref(),
        Some("builtin:Chorus")
    );
    assert_eq!(
        super::super::builtin_effect_name_from_project_name("builtin:Chorus"),
        Some("Chorus")
    );
    assert_eq!(d.input_channels, 2 + SLOTS);
    assert_eq!(d.instrument_modulation_targets.len(), COUNT * SLOTS);
    let mut h = Harness::new(48000);
    for p in &d.params {
        if voice_modulator::is_source_param(p.node_param_idx) {
            continue;
        }
        assert!((p.node_param_idx as usize) < DEPTH_BASE + COUNT * SLOTS);
        assert_eq!(
            unsafe { *h.ptr().cast::<f32>().add(p.node_param_idx as usize) },
            p.default
        );
    }
    for target in &d.instrument_modulation_targets {
        let i = target.base_param_idx - 1;
        let slot = target.modulator_slot - 1;
        let depth = &d.params[target.depth_param_idx];
        assert_eq!(depth.node_param_idx as usize, DEPTH_BASE + i * SLOTS + slot);
        assert!(d.params[target.base_param_idx].is_host_modulatable());
        assert_eq!(depth.stored_to_user(0.25), 0.25);
        let amount = (CONTROLS[i].max - CONTROLS[i].min) * 0.01;
        h.state().depths = [[0.0; SLOTS]; COUNT];
        unsafe {
            *h.ptr().cast::<f32>().add(depth.node_param_idx as usize) = amount;
        }
        let mut sources = [0.0; SLOTS];
        sources[slot] = 1.0;
        let effective = h.state().effective(sources);
        let expected = (CONTROLS[i].default + amount).clamp(CONTROLS[i].min, CONTROLS[i].max);
        assert_eq!(effective[i], expected as f64);
        assert_eq!(h.state().params[i], CONTROLS[i].default);
        let input = signal(2048, 48000.0);
        let actual = h.run(&input, 31, sources);
        let mut reference = Harness::new(48000);
        reference.state().params[i] = expected;
        assert_eq!(actual, reference.run(&input, 128, [0.0; SLOTS]));
        unsafe {
            reset(h.ptr());
        }
    }
}

#[test]
fn dry_bypass_reset_and_block_partition_are_exact() {
    for sr in [44100, 48000, 96000, 192000] {
        let input = signal(sr / 4, sr as f32);
        let mut a = Harness::new(sr as i32);
        let mut b = Harness::new(sr as i32);
        a.state().params[MIX] = 0.0;
        assert_eq!(a.run(&input, 31, [0.0; SLOTS]), input);
        a.state().enabled = 0.0;
        a.state().params[MIX] = 1.0;
        a.state().params[OUTPUT] = 12.0;
        unsafe {
            reset(a.ptr());
        }
        assert_eq!(a.run(&input, 128, [0.0; SLOTS]), input);
        a = Harness::new(sr as i32);
        let first = a.run(&input, 31, [0.0; SLOTS]);
        assert_eq!(first, b.run(&input, 128, [0.0; SLOTS]));
        unsafe {
            reset(a.ptr());
        }
        assert_eq!(first, a.run(&input, 257, [0.0; SLOTS]));
    }
}

#[test]
fn phase_relationship_and_mono_wet_excitation() {
    let input = signal(48000, 48000.0);
    let mut h = Harness::new(48000);
    h.state().params[MIX] = 1.0;
    h.state().params[PHASE] = 0.0;
    assert!(h
        .run(&input, 128, [0.0; SLOTS])
        .iter()
        .all(|x| x[0] == x[1]));
    h.state().params[PHASE] = 180.0;
    unsafe {
        reset(h.ptr());
    }
    assert!(h
        .run(&input, 128, [0.0; SLOTS])
        .iter()
        .any(|x| (x[0] - x[1]).abs() > 0.01));
    let opposite: Vec<_> = input.iter().map(|x| [x[0], -x[0]]).collect();
    unsafe {
        reset(h.ptr());
    }
    assert!(h
        .run(&opposite, 128, [0.0; SLOTS])
        .iter()
        .all(|x| *x == [0.0; 2]));
}

#[test]
fn interpolator_delay_units_and_filter_response() {
    let mut h = Harness::new(48000);
    unsafe {
        let buffer = ring(h.ptr());
        for i in 0..h.state().capacity {
            *buffer.add(i) = i as f32;
        }
        h.state().write = 1000;
        for d in [2.0, 5.25, 20.75, 999.0] {
            assert!((h.state().read(buffer, d) - (1000.0 - d)).abs() < 1e-9);
        }
    }
    assert_eq!(triangle(0.0), 0.0);
    assert_eq!(triangle(0.5), 1.0);
    for sr in [44100.0, 96000.0] {
        for highpass in [false, true] {
            let mut filter = Filter::default();
            let g = (std::f64::consts::PI * 1000.0 / sr).tan();
            let mut energy = 0.0;
            for n in 0..sr as usize {
                let x = (std::f64::consts::TAU * 1000.0 * n as f64 / sr).sin();
                let y = filter.tick(x, g, highpass);
                if n >= sr as usize / 2 {
                    energy += y * y;
                }
            }
            let rms = (energy / (sr / 2.0)).sqrt();
            assert!((rms - 0.5).abs() < 0.002, "Butterworth corner: {rms}");
        }
    }
}

#[test]
fn extreme_automation_and_nonfinite_inputs_do_not_poison_state() {
    for sr in [8000, 44100, 96000, 192000] {
        let mut h = Harness::new(sr);
        let mut input = signal(2048, sr as f32);
        input[17] = [f32::NAN, f32::INFINITY];
        for pass in 0..6 {
            for (i, c) in CONTROLS.iter().enumerate() {
                h.state().params[i] = if pass % 2 == 0 { c.min } else { c.max };
                h.state().depths[i] = [c.max - c.min; SLOTS];
            }
            if pass == 3 {
                h.state().params[BASE] = f32::NAN;
            }
            let out = h.run(&input, 31, [if pass % 2 == 0 { -1.0 } else { 1.0 }; SLOTS]);
            assert!(out
                .iter()
                .flatten()
                .all(|v| v.is_finite() && v.abs() < 10.0));
        }
        let silent = h.run(&vec![[0.0; 2]; sr as usize * 2], 128, [0.0; SLOTS]);
        assert!(silent[silent.len() - 256..]
            .iter()
            .flatten()
            .all(|v| v.abs() < 1e-10));
    }
}

#[test]
fn static_delay_is_in_milliseconds_at_every_sample_rate() {
    for sr in [44100, 48000, 96000, 192000] {
        let mut h = Harness::new(sr);
        h.state().params[MIX] = 1.0;
        h.state().params[DEPTH] = 0.0;
        h.state().params[BASE] = 10.0;
        let delay = sr as usize / 100;
        let mut input = vec![[0.0; 2]; delay + 100];
        input[0] = [1.0; 2];
        let out = h.run(&input, 31, [0.0; SLOTS]);
        assert!(out[..delay].iter().all(|x| *x == [0.0; 2]));
        assert!(out[delay][0].abs() > 1e-6);
    }
}

#[test]
fn automated_controls_are_partition_independent_and_bypass_settles_to_dry() {
    let mut a = Harness::new(48000);
    let mut b = Harness::new(48000);
    let input = signal(8192, 48000.0);
    for pass in 0..4 {
        for h in [&mut a, &mut b] {
            h.state().params[RATE] = if pass % 2 == 0 { 0.1 } else { 12.0 };
            h.state().params[BASE] = if pass % 2 == 0 { 0.5 } else { 30.0 };
            h.state().params[PHASE] = if pass % 2 == 0 { 0.0 } else { 360.0 };
            h.state().params[HP] = if pass % 2 == 0 { 20.0 } else { 12000.0 };
            h.state().params[OUTPUT] = 6.0;
        }
        assert_eq!(a.run(&input, 31, [0.0; SLOTS]), b.run(&input, 128, [0.0; SLOTS]));
    }
    a.state().enabled = 0.0;
    a.run(&signal(24000, 48000.0), 31, [0.0; SLOTS]);
    assert_eq!(a.run(&input, 128, [0.0; SLOTS]), input);
}

#[test]
fn migration_preserves_tail_and_rate_changes_preserve_controls() {
    let mut old = Harness::new(48000);
    old.state().params[RATE] = 1.2;
    let input = signal(4096, 48000.0);
    old.run(&input, 128, [0.0; SLOTS]);
    let mut new = Harness::new(48000);
    unsafe {
        migrate(new.ptr(), old.ptr());
    }
    assert_eq!(
        new.run(&input, 31, [0.0; SLOTS]),
        old.run(&input, 128, [0.0; SLOTS])
    );
    let mut changed = Harness::new(96000);
    unsafe {
        migrate(changed.ptr(), old.ptr());
    }
    assert_eq!(changed.state().params[RATE], 1.2);
    assert_eq!(changed.state().phase, 0.0);
    assert_eq!(changed.state().sample_rate, 96000.0);
    assert_eq!(
        changed.run(&[[0.0; 2]; 512], 128, [0.0; SLOTS]),
        vec![[0.0; 2]; 512]
    );
}
