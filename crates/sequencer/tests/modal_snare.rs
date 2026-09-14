use sequencer::lisp_host::{self, InstrumentParamEvent, InstrumentRenderOptions};

#[test]
fn modal_coefficients_follow_parameter_events_without_delay() {
    let source = lisp_host::load_instrument_source("factory:Drums/Modal Snare").unwrap();
    // Observe the pure coefficient path, independently of the chaotic wire
    // recurrence. A freshly initialized voice supplies each expected value.
    let audio = "(out (* dcy level-v vel-gain) 1 @name audio)";
    assert_eq!(source.matches(audio).count(), 1);
    let source = source.replace(audio, "\
(def coefficient-check (+ (/ (sum bat-r) 1000) (/ (sum r-bat) 72)
  (/ (sum r-res) 72) (/ (sum spread) 72) (/ (sum bright-w) 1000)
  r2s r3s spread1 spread2 spread3 bright-norm rb1 rb2 rb3 rr1 rr2 rr3))
(out coefficient-check 1 @name audio)");
    let compiled = lisp_host::compile_and_load_instrument(&source, 48_000).unwrap();
    let render = |overrides, events, block_size, voice_index| {
        let report = lisp_host::render_loaded_instrument_for_test(
            &compiled.manifest, &compiled.lib, &InstrumentRenderOptions {
                sample_rate: 48_000, block_size, frames: 32,
                midi_note: 69.0, velocity: 1.0, gate_frames: 32, voice_index,
                param_overrides: overrides, param_events: events, input_overrides: Vec::new(),
            },
        ).unwrap();
        assert_eq!(report.non_finite_samples, 0);
        assert_eq!(report.non_finite_state_slots, 0);
        report.first_samples
    };
    for (name, lo, hi) in [
        ("stretch", 0.4, 1.6), ("split", 0.0, 4.0),
        ("tilt", 0.0, 2.5), ("visc", 0.0, 2.0),
        ("release", 20.0, 3000.0), ("release2", 20.0, 3000.0),
        ("tip", 0.03, 0.45), ("bright", 0.0, 2.5),
    ] {
        let low = render(vec![(name.into(), lo)], Vec::new(), 32, 0)[0];
        let high = render(vec![(name.into(), hi)], Vec::new(), 32, 0)[0];
        assert!((high - low).abs() > 0.0001, "{name} must affect the observed coefficients");
        let events: Vec<_> = [(1, hi), (2, lo), (7, hi), (8, lo), (16, hi), (31, lo)]
            .into_iter().map(|(frame, value)| InstrumentParamEvent {
                frame, name: name.into(), value,
            }).collect();
        for block_size in [1, 7, 32] {
            // Also exercise a separate host voice's initialization and cache.
            for voice in [0, 5] {
                let samples = render(vec![(name.into(), lo)], events.clone(), block_size, voice);
                let mut expected = low;
                for (frame, actual) in samples.into_iter().enumerate() {
                    if let Some(event) = events.iter().find(|event| event.frame == frame) {
                        expected = if event.value == hi { high } else { low };
                    }
                    assert!((actual - expected).abs() < 0.00001,
                        "{name}, voice {voice}, block {block_size}, frame {frame}: {actual} != {expected}");
                }
            }
        }
    }
}

#[test]
fn rim_pitch_tracks_played_notes() {
    let source = lisp_host::load_instrument_source("factory:Drums/Modal Snare").unwrap();
    let compiled = lisp_host::compile_and_load_instrument(&source, 48_000).unwrap();
    let render = |note, tracking, pitch, block_size| {
        let report = lisp_host::render_loaded_instrument_for_test(
            &compiled.manifest, &compiled.lib, &InstrumentRenderOptions {
                sample_rate: 48_000, block_size, frames: 9_600,
                midi_note: note, velocity: 1.0, gate_frames: 9_600, voice_index: 0,
                param_overrides: vec![
                    ("stroke".into(), 1.0), ("rim_pitch".into(), pitch),
                    ("rim_track".into(), tracking),
                    // Keep stochastic contact out of the pitch comparison. The
                    // production rim, head mix and output shaper still render.
                    ("scrape".into(), 0.0), ("snares".into(), 0.0),
                    ("rim_drive".into(), 0.0),
                ],
                param_events: Vec::new(), input_overrides: Vec::new(),
            },
        ).unwrap();
        assert_eq!(report.non_finite_samples, 0);
        assert_eq!(report.non_finite_state_slots, 0);
        assert!(report.peak > 0.01 && report.rms > 0.001);
        report
    };

    // At full tracking, one octave of played pitch doubles/halves the
    // rim frequency; half tracking moves six semitones. A4 is the anchor.
    for (note, tracking, expected_hz) in [
        (57.0, 0.0, 1200.0), (57.0, 0.5, 848.52814), (57.0, 1.0, 600.0),
        (69.0, 0.0, 1200.0), (69.0, 0.5, 1200.0), (69.0, 1.0, 1200.0),
        (81.0, 0.0, 1200.0), (81.0, 0.5, 1697.0563), (81.0, 1.0, 2400.0),
    ] {
        let fixed = render(note, 0.0, expected_hz, 128);
        for block_size in [64, 128] {
            let tracked = render(note, tracking, 1200.0, block_size);
            for (a, b) in tracked.first_samples.iter().zip(&fixed.first_samples) {
                assert!((a - b).abs() < 0.00001,
                    "note {note}, tracking {tracking}, block {block_size}: {a} != {b}");
            }
            assert!((tracked.rms - fixed.rms).abs() < fixed.rms * 0.001,
                "note {note}, tracking {tracking}: rim decay differs from fixed tuning");
        }
    }

    let fixed = render(81.0, 0.0, 1200.0, 128);
    let tracked = render(81.0, 1.0, 1200.0, 128);
    assert!(fixed.first_samples.iter().zip(tracked.first_samples)
        .any(|(a, b)| (a - b).abs() > 0.01), "rim tracking must audibly change the rimshot");
}
