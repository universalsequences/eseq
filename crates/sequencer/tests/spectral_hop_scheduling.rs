use sequencer::lisp_host::{compile_and_load, render_loaded_effect_for_test, EffectRenderOptions};

fn options(block_size: usize) -> EffectRenderOptions {
    EffectRenderOptions {
        sample_rate: 48_000,
        block_size,
        frames: 16_384,
        param_overrides: Vec::new(),
        param_events: Vec::new(),
        tensor_overrides: Vec::new(),
        input_overrides: vec![(0, 0.0), (1, 0.0)],
        input_tones: vec![(0, 997.0, 0.2), (1, 1409.0, 0.13)],
    }
}

// Regression for eseq-yxau, fixed in DGenLisp v0.1.9. Keep the host block
// independent of the FFT hop instead of avoiding multi-hop process calls.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "eseq-wci0: Linux compiler pin predates this fix")]
fn stereo_stft_shared_output_history_is_independent_of_host_block_size() {
    let source = include_str!("fixtures/effects/stereo-stft-shared-output-history.lisp");
    let compiled = compile_and_load(source, 48_000).expect("compile shared-history regression");
    let mut reference: Option<Vec<f32>> = None;
    let mut errors = Vec::new();
    for block in [64, 128, 256, 512] {
        let report = render_loaded_effect_for_test(
            &compiled.manifest, &compiled.lib, &options(block),
        ).expect("render shared-history regression");
        assert!(report.samples.iter().all(|s| s.is_finite()));
        let error = reference.as_ref().map_or(0.0, |samples| {
            samples.iter().zip(&report.samples)
                .map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max)
        });
        eprintln!("shared output history: block={block}, L={}, R={}, error={error:e}",
            report.left_rms, report.right_rms);
        errors.push((block, error));
        reference.get_or_insert(report.samples);
    }
    assert!(errors.iter().all(|(_, error)| *error < 2.0e-5),
        "shared output history changes with host block size: {errors:?}");
}

#[test]
fn stereo_hann_round_trip_is_independent_of_host_block_size() {
    for n in [512, 1024, 2048] {
        let hop = n / 4;
        let source = format!(r#"
            (def left (in 1 @name Left))
            (def right (in 2 @name Right))
            (def win (hann {n}))
            (def frame-l (* (reshape (buffer left {n} {hop}) @shape [{n}]) win))
            (def frame-r (* (reshape (buffer right {n} {hop}) @shape [{n}]) win))
            (def (re-l im-l) (fft frame-l @N {n} @backend accelerated))
            (def (re-r im-r) (fft frame-r @N {n} @backend accelerated))
            (def time-l (ifft re-l im-l @N {n} @backend accelerated))
            (def time-r (ifft re-r im-r @N {n} @backend accelerated))
            (out (/ (overlap-add (* time-l win) {hop}) 1.5) 1 @name Left)
            (out (/ (overlap-add (* time-r win) {hop}) 1.5) 2 @name Right)
        "#);
        let compiled = compile_and_load(&source, 48_000).expect("compile stereo identity");
        let mut reference: Option<Vec<f32>> = None;
        for block in [64, 128, 256, 512] {
            let report = render_loaded_effect_for_test(
                &compiled.manifest, &compiled.lib, &options(block),
            ).expect("render stereo identity");
            assert!(report.samples.iter().all(|s| s.is_finite()));
            // The buffer/overlap-add pair delays by N-1 samples. Check each
            // channel against its own independent analytic input, not just
            // against another rendering that might share a scheduling bug.
            let mut error = [0.0f32; 2];
            for frame in (2 * n)..report.frames {
                let t = (frame - (n - 1)) as f32 / 48_000.0;
                for (channel, frequency, amplitude) in [(0, 997.0, 0.2), (1, 1409.0, 0.13)] {
                    let expected = amplitude * (2.0 * std::f32::consts::PI * frequency * t).sin();
                    error[channel] = error[channel].max((report.samples[frame * 2 + channel] - expected).abs());
                }
            }
            let block_error = reference.as_ref().map_or(0.0, |samples| {
                samples.iter().zip(&report.samples)
                    .map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max)
            });
            eprintln!("N={n}, hop={hop}, block={block}: identity error={error:?}, block error={block_error:e}");
            assert!(error.iter().all(|e| *e < 2.0e-5), "N={n}, block={block}: {error:?}");
            assert!(block_error < 2.0e-5, "N={n}, block={block}: {block_error}");
            reference.get_or_insert(report.samples);
        }
    }
}
