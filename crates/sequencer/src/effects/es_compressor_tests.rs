use super::*;
use crate::lisp_host::{EffectRenderOptions, EffectRenderReport, InstrumentParamEvent};

fn options(rate: u32, frames: usize, params: &[(&str, f32)]) -> EffectRenderOptions {
    EffectRenderOptions {
        sample_rate: rate,
        block_size: 128,
        frames,
        param_overrides: params.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        param_events: Vec::new(),
        tensor_overrides: Vec::new(),
        input_overrides: vec![(0, 0.7), (1, -0.35)],
        input_tones: Vec::new(),
    }
}

fn render(source: &str, options: &EffectRenderOptions) -> EffectRenderReport {
    // Missing compiler/toolchain is a hard error, never a silent skip.
    let report = crate::lisp_host::render_effect_source_for_test(source, options)
        .expect("compile/load/render ES Compressor");
    assert!(
        report.samples.iter().all(|sample| sample.is_finite()),
        "nonfinite DSP output"
    );
    report
}

fn observation(left: &str, right: &str) -> String {
    // Expose controller nodes only in the test program; the public effect
    // remains stereo audio, with no debug parameters or alternate code paths.
    let (body, _) = dsp_source()
        .split_once("\n(out (finish compressed-l")
        .expect("stereo output boundary");
    format!("{body}\n(out {left} 1)\n(out {right} 2)\n")
}

#[test]
fn manifest_declares_modes_and_fixed_low_latency_without_fft() {
    for forbidden in ["(fft ", "(ifft ", "(buffer ", "@file", "tensor-param"] {
        assert!(!dsp_source().contains(forbidden), "unexpected {forbidden}");
    }
    for (rate, expected) in [
        (8000, 2),
        (44100, 11),
        (48000, 12),
        (96000, 24),
        (384000, 96),
    ] {
        assert_eq!(
            crate::lisp_host::declared_effect_latency_samples(dsp_source(), rate).unwrap(),
            Some(expected)
        );
    }
    let json = crate::lisp_host::compile_lisp(dsp_source(), 48000).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&json).unwrap();
    let params = manifest["params"].as_array().unwrap();
    let mode = params.iter().find(|p| p["name"] == "mode").unwrap();
    assert_eq!(mode["default"], 0);
    assert_eq!(
        mode["options"]["labels"],
        serde_json::json!(["Punch", "Level", "Sustain"])
    );
    assert_eq!(params.len(), 10);
    // Prevent an accidental return to large per-instance FFT workspaces.
    assert!(manifest["totalMemorySlots"].as_u64().unwrap() < 32_768);
}

fn gated_observation(left: &str, right: &str, seconds: f64) -> String {
    observation(left, right).replace(
        "(def left (in 1 @name Left))",
        &format!("(def raw-left (in 1 @name Left))\n\
            (make-history test-clock)\n\
            (def test-frame (read-history test-clock))\n\
            (write-history test-clock (+ test-frame 1))\n\
            (def left (* raw-left (lt test-frame (* samplerate {seconds}))))"),
    )
}

fn tone_options(rate: u32, frames: usize, params: &[(&str, f32)], hz: f32, peak: f32) -> EffectRenderOptions {
    let mut o = options(rate, frames, params);
    o.input_overrides = vec![(0, 0.0), (1, 0.0)];
    o.input_tones = vec![(0, hz, peak)];
    o
}

#[test]
fn punch_memory_envelope_matches_double_precision_reference() {
    let source = gated_observation("punch-target", "punch-gr", 0.25);
    for (rate, attack, release) in [(8000, 1.0, 20.0), (48000, 40.0, 120.0), (384000, 200.0, 2000.0)] {
        let out = render(&source, &tone_options(rate, rate as usize * 2,
            &[("attack", attack), ("release", release)], 1000.0, 0.7));
        let mut memory = 0.0f64;
        let mut gr = 0.0f64;
        for (i, frame) in out.samples.chunks_exact(2).enumerate() {
            // Observe the gain computer separately, then integrate the authored
            // envelope in f64. Static/detector behavior has its own tests below.
            let target = frame[0] as f64;
            let memory_ms = release as f64 * if target > memory { 1.4787 } else { 2.4024 };
            memory += (target - memory) * -(-1000.0 / (rate as f64 * memory_ms)).exp_m1();
            let (aim, tau) = if target > gr {
                (target, 1.0475 * attack as f64)
            } else {
                ((target + 0.15335 * (memory - target).max(0.0)).min(gr), 0.8444 * release as f64)
            };
            gr += (aim - gr) * -(-1000.0 / (rate as f64 * tau)).exp_m1();
            assert!((frame[1] as f64 - gr).abs() < 0.003,
                "rate={rate} frame={i}: {} vs {gr}", frame[1]);
            assert!(frame[1] >= 0.0 && frame[1] <= 40.0);
        }
    }
}

#[test]
fn level_gain_populations_match_double_precision_and_keep_bounds() {
    let source = gated_observation("level-target", "level-attenuation", 0.25);
    // At 384 kHz / Release 2000, the slow population has an 86.7-second
    // time constant. This also guards the split accumulator under fast-math.
    for (rate, attack, release) in [(8000, 1.0, 20.0), (48000, 40.0, 120.0), (384000, 200.0, 2000.0)] {
        let out = render(&source, &tone_options(rate, rate as usize * 2,
            &[("attack", attack), ("release", release)], 1000.0, 0.7));
        let mut fast = 1.0f64;
        let mut slow = 1.0f64;
        for (i, frame) in out.samples.chunks_exact(2).enumerate() {
            let reduction = frame[0] as f64;
            let target = 10.0f64.powf(-reduction / 20.0);
            let scale = 1.0 + 17.0739 / (1.0 + (reduction / 4.26775).powf(7.53749));
            for (state, a, r) in [(&mut fast, 0.0817833, 2.30597), (&mut slow, 0.901311, 43.3357)] {
                let tau = if target < *state { a * attack as f64 * scale } else { r * release as f64 };
                *state += (target - *state) * -(-1000.0 / (rate as f64 * tau)).exp_m1();
            }
            let expected = 0.815406 * fast + 0.184594 * slow;
            assert!((frame[1] as f64 - expected).abs() < 0.00001,
                "rate={rate} frame={i}: {} vs {expected}", frame[1]);
            assert!(frame[1] > 0.0 && frame[1] <= 1.000001);
        }
    }
}

#[test]
fn sustain_controller_preserves_existing_equations() {
    let rate = 48000.0;
    let out = render(
        &observation("gr", "makeup"),
        &options(rate as u32, 12000, &[]),
    );
    let u = 0.5f64;
    let threshold = -36.0 - 30.0 * u;
    let ceiling = 8.0 * 2.0f64.powf(5.0 * u);
    let curve = |over: f64| ceiling * (1.0 - (-0.75 * over / ceiling).exp());
    let level = 20.0 * (0.7f32 as f64).log10();
    let target = curve(level - threshold);
    let makeup = curve(-23.0 - threshold);
    let mut history = [0.0; 48];
    let mut previous = 0.0f64;
    for (i, frame) in out.samples.chunks_exact(2).enumerate() {
        let error = target - history.iter().sum::<f64>() / 48.0;
        let overshoot = 1.5 + 5.5 * u * u;
        let heat = u * u * ((level + 20.0) / 12.0).clamp(0.0, 1.0);
        let depth_speed = (1.0 + overshoot) * (-5.0 * heat).exp();
        let tau = if error >= 0.0 {
            (40.0 * depth_speed * (-error.abs() / 4.0).exp()).max(0.05)
        } else {
            (120.0 * depth_speed * (-error.abs() / 8.0).exp()).max(20.0 - 19.0 * heat)
        };
        let aim = (target + (overshoot * error).clamp(-18.0, 18.0)).max(0.0);
        previous = aim + (-1000.0 / (tau * rate)).exp() * (previous - aim);
        history[i % 48] = previous;
        assert!(
            (frame[0] as f64 - previous).abs() < 0.003,
            "frame {i}: {} vs {previous}",
            frame[0]
        );
        assert!((frame[1] as f64 - makeup).abs() < 0.0001);
    }
}

#[test]
fn modes_have_distinct_linked_responses_and_true_parallel_mix() {
    let mut wet = Vec::new();
    for mode in 0..3 {
        let mut o = options(48000, 16384, &[("mode", mode as f32)]);
        // Constant signals below the shaper knee isolate gain behavior.
        o.input_overrides = vec![(0, 0.1), (1, -0.05)];
        let full = render(dsp_source(), &o);
        for frame in full.samples.chunks_exact(2) {
            assert!((frame[0] + 2.0 * frame[1]).abs() < 0.00001);
        }
        o.param_overrides.push(("mix".into(), 0.0));
        let dry = render(dsp_source(), &o);
        for (i, frame) in dry.samples.chunks_exact(2).enumerate() {
            assert_eq!(frame, if i < 12 { &[0.0, 0.0] } else { &[0.1, -0.05] });
        }
        o.param_overrides.last_mut().unwrap().1 = 0.5;
        let half = render(dsp_source(), &o);
        for ((h, w), d) in half.samples.iter().zip(&full.samples).zip(&dry.samples) {
            assert!((h - 0.5 * (w + d)).abs() < 0.00001);
        }
        wet.push(full.samples);
    }
    for a in 0..3 {
        for b in a + 1..3 {
            // Compare trajectories, not just their final DC gain: distinct
            // controllers can legitimately settle to similar steady levels.
            let error: f64 = wet[a]
                .iter()
                .zip(&wet[b])
                .map(|(x, y)| (*x as f64 - *y as f64).powi(2))
                .sum();
            let energy: f64 = wet[a].iter().map(|x| (*x as f64).powi(2)).sum();
            assert!(
                (error / energy).sqrt() > 0.01,
                "indistinguishable modes {a}/{b}"
            );
        }
    }
}

#[test]
fn output_trim_covers_the_declared_gain_range() {
    for db in [-48.0_f32, 0.0, 6.0, 12.0, 18.0] {
        let mut o = options(48000, 128, &[("mix", 0.0), ("output-db", db)]);
        o.input_overrides = vec![(0, 0.1), (1, -0.05)];
        let report = render(dsp_source(), &o);
        let gain = 10.0_f32.powf(db / 20.0);
        for (i, frame) in report.samples.chunks_exact(2).enumerate() {
            let expected = if i < 12 { [0.0, 0.0] } else { [0.1 * gain, -0.05 * gain] };
            for channel in 0..2 {
                assert!((frame[channel] - expected[channel]).abs() < 0.00001,
                    "output trim {db} dB, frame {i}: {frame:?} vs {expected:?}");
            }
        }
    }
}

#[test]
fn automation_is_partition_invariant_and_extremes_are_finite() {
    for rate in [8000, 44100, 48000, 96000, 192000, 384000] {
        let mut o = options(
            rate,
            rate as usize,
            &[
                ("amount", 100.0),
                ("attack", 1.0),
                ("release", 20.0),
                ("tone", 100.0),
                ("drive", 24.0),
                ("input-db", 24.0),
                ("output-db", 18.0),
                ("detector-db", 36.0),
            ],
        );
        o.input_tones = vec![(0, 71.0, 0.7), (1, 997.0, 0.5)];
        for (fraction, mode) in [(0.1, 2.0), (0.3, 1.0), (0.6, 0.0), (0.8, 2.0)] {
            o.param_events.push(InstrumentParamEvent {
                frame: (rate as f64 * fraction) as usize + 7,
                name: "mode".into(),
                value: mode,
            });
        }
        for (name, value) in [
            ("release", 2000.0),
            ("attack", 200.0),
            ("tone", 0.0),
            ("mix", 0.3),
            ("input-db", -24.0),
            ("output-db", -48.0),
            ("detector-db", -36.0),
            ("amount", 0.0),
        ] {
            o.param_events.push(InstrumentParamEvent {
                frame: rate as usize / 2 + 3,
                name: name.into(),
                value,
            });
        }
        let a = render(dsp_source(), &o);
        o.block_size = 31;
        let b = render(dsp_source(), &o);
        assert_eq!(a.samples, b.samples, "block partition at {rate}");
        // +24 dB input and +18 dB output allow large dry peaks; this is not
        // a brickwall limiter. Keep the bound consistent with those trims.
        assert!(
            a.peak.is_finite() && a.peak < 128.0,
            "bounded driven audio at {rate}: {}",
            a.peak
        );
    }
}

#[test]
fn mode_changes_crossfade_warm_controllers_without_resetting_them() {
    let mut o = options(48000, 12000, &[]);
    o.input_overrides = vec![(0, 0.1), (1, -0.05)];
    let mut fixed = Vec::new();
    for mode in 0..3 {
        o.param_overrides = vec![("mode".into(), mode as f32)];
        let out = render(dsp_source(), &o);
        assert!(out.peak < 0.75, "fixture must stay below the shaper knee");
        fixed.push(out.samples);
    }
    o.param_overrides = vec![("mode".into(), 0.0)];
    for (frame, mode) in [(2003, 2.0), (5007, 1.0), (8009, 0.0)] {
        o.param_events.push(InstrumentParamEvent {
            frame,
            name: "mode".into(),
            value: mode,
        });
    }
    let changed = render(dsp_source(), &o);
    let pole = (-100.0f64 / 48000.0).exp();
    let mut weights = [1.0f64, 0.0, 0.0];
    let mut mode = 0;
    for (i, sample) in changed.samples.chunks_exact(2).enumerate() {
        if i == 2003 {
            mode = 2;
        }
        if i == 5007 {
            mode = 1;
        }
        if i == 8009 {
            mode = 0;
        }
        for (j, w) in weights.iter_mut().enumerate() {
            let target = if j == mode { 1.0 } else { 0.0 };
            *w = target + pole * (*w - target);
        }
        for channel in 0..2 {
            let expected: f64 = (0..3)
                .map(|j| weights[j] * fixed[j][2 * i + channel] as f64)
                .sum();
            assert!(
                (sample[channel] as f64 - expected).abs() < 0.00001,
                "warm mode transition at {i}: {} vs {expected}",
                sample[channel]
            );
        }
    }
}

#[test]
fn controllers_release_after_a_burst_and_silence_stays_silent() {
    // Entirely synthetic excitation; no private samples or reference renders.
    let input = "(def raw-left (in 1 @name Left))\n\
        (make-history test-clock)\n\
        (def test-frame (read-history test-clock))\n\
        (write-history test-clock (+ test-frame 1))\n\
        (def left (* raw-left (lt test-frame samplerate)))";
    let source =
        observation("punch-gain", "level-attenuation").replace("(def left (in 1 @name Left))", input);
    // Level has AC input coupling; DC is not a sustained compression fixture.
    let o = tone_options(48000, 48000 * 20, &[("amount", 80.0)], 1000.0, 0.7);
    let out = render(&source, &o);
    let end_burst = &out.samples[2 * 47999..2 * 48000];
    let recovered = &out.samples[out.samples.len() - 2..];
    for c in 0..2 {
        assert!(
            end_burst[c] < 0.5,
            "controller {c} must compress: {end_burst:?}"
        );
        assert!(
            recovered[c] > 0.99,
            "controller {c} must recover: {recovered:?}"
        );
    }
    for mode in 0..3 {
        let mut o = options(
            48000,
            48000,
            &[
                ("mode", mode as f32),
                ("amount", 100.0),
                ("drive", 24.0),
                ("tone", 100.0),
            ],
        );
        o.input_overrides = vec![(0, 0.0), (1, 0.0)];
        assert_eq!(render(dsp_source(), &o).peak, 0.0);
    }
}

fn steady_reduction(hz: f32, peak_db: f32, amount: f32, detector_db: f32) -> [f64; 2] {
    let out = render(&observation("punch-gr", "level-gr"), &tone_options(
        48000, 96000, &[("amount", amount), ("detector-db", detector_db)],
        hz, 10.0f32.powf(peak_db / 20.0)));
    let mut mean = [0.0; 2];
    for frame in out.samples[2 * 72000..].chunks_exact(2) {
        for c in 0..2 { mean[c] += frame[c] as f64 / 24000.0; }
    }
    mean
}

#[test]
fn amount_and_detector_engage_meaningful_compression_at_musical_levels() {
    let quiet = steady_reduction(1000.0, -24.0, 50.0, 0.0);
    let normal = steady_reduction(1000.0, -12.0, 50.0, 0.0);
    // Functional voicing bounds, not copied reference code or sample fixtures.
    assert!((2.0..4.0).contains(&quiet[0]), "Punch must engage quiet material: {quiet:?}");
    assert!((8.0..11.0).contains(&normal[0]), "Punch reduction: {normal:?}");
    assert!((0.8..2.5).contains(&normal[1]), "Level reduction: {normal:?}");
    let strong = steady_reduction(1000.0, -24.0, 100.0, 0.0);
    let driven = steady_reduction(1000.0, -12.0, 50.0, 12.0);
    let off = steady_reduction(1000.0, -6.0, 0.0, 0.0);
    for c in 0..2 {
        assert!(strong[c] > quiet[c] + 3.0, "Amount must increase reduction: {quiet:?} -> {strong:?}");
        assert!(driven[c] > normal[c] + 5.0, "Detector must drive compression: {normal:?} -> {driven:?}");
        assert!(off[c].abs() < 0.0001, "Amount 0 disables reduction, not Level's nominal gain: {off:?}");
    }
}

#[test]
fn level_detector_weights_bass_and_treble_without_tone_coloration() {
    let bass = steady_reduction(60.0, -12.0, 50.0, 0.0);
    let mid = steady_reduction(1000.0, -12.0, 50.0, 0.0);
    let high = steady_reduction(8000.0, -12.0, 50.0, 0.0);
    assert!(bass[1] > mid[1] + 2.0, "Level bass sensitivity: {bass:?} vs {mid:?}");
    assert!(high[1] > mid[1] + 4.0, "Level treble sensitivity: {high:?} vs {mid:?}");
    assert!((bass[0] - mid[0]).abs() < 1.5, "Punch must not inherit Level's detector: {bass:?} vs {mid:?}");
}

#[test]
fn level_recovery_remembers_exposure_and_timing_controls_remain_effective() {
    let mut tails = Vec::new();
    for seconds in [0.02, 2.0] {
        let out = render(&gated_observation("punch-gr", "level-gr", seconds),
            &tone_options(48000, ((seconds + 3.0) * 48000.0) as usize, &[], 1000.0, 0.5));
        let frame = ((seconds + 1.0) * 48000.0) as usize;
        tails.push(out.samples[frame * 2 + 1]);
        assert!(out.samples[out.samples.len() - 1] < tails.last().copied().unwrap());
    }
    assert!(tails[1] > tails[0] + 0.35, "Long exposure must leave a longer recovery: {tails:?}");
    let source = gated_observation("punch-gr", "level-gr", 0.02);
    let fast = render(&source, &tone_options(48000, 96000, &[("attack", 1.0)], 1000.0, 0.5));
    let slow = render(&source, &tone_options(48000, 96000, &[("attack", 200.0)], 1000.0, 0.5));
    for c in 0..2 {
        assert!(fast.samples[959 * 2 + c] > slow.samples[959 * 2 + c] + 2.0,
            "Attack must change transient passage for mode {c}");
    }
    let source = gated_observation("punch-gr", "level-gr", 0.2);
    let fast = render(&source, &tone_options(48000, 96000, &[("release", 20.0)], 1000.0, 0.5));
    let slow = render(&source, &tone_options(48000, 96000, &[("release", 2000.0)], 1000.0, 0.5));
    for c in 0..2 {
        assert!(slow.samples[57600 * 2 + c] > fast.samples[57600 * 2 + c] + 0.5,
            "Release must change recovery for mode {c}");
    }
}

#[test]
fn tone_is_optional_and_attenuates_high_frequencies() {
    let mut o = options(48000, 8192, &[("amount", 0.0), ("mode", 0.0)]);
    o.input_tones = vec![(0, 18000.0, 0.2), (1, 1000.0, 0.2)];
    let flat = render(dsp_source(), &o);
    o.param_overrides.push(("tone".into(), 100.0));
    let dark = render(dsp_source(), &o);
    assert!(dark.left_rms < flat.left_rms * 0.1);
    assert!(dark.right_rms > flat.right_rms * 0.9);
    assert!(dark.right_rms < flat.right_rms * 1.2);
}
