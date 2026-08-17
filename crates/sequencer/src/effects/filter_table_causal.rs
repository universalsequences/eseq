//! Causal minimum-phase renderer for the Filter Table effect (eseq-dtx.2).
//!
//! Feasibility probes first: the causal path replaces the signal STFT with a
//! per-sample sliding-window FIR whose kernel is rebuilt at hop rate from the
//! same magnitude derivation the spectral mode uses. These probes establish
//! that the dgen primitives involved (frame-rate `buffer` windows, reductions
//! against a mutable kernel tensor, hop-rate kernel updates) compile, behave
//! causally, and run at acceptable cost before the full DSP is built.

#[cfg(test)]
mod probes {
    use crate::effects::filter_table::tests::render_lock;
    use crate::lisp_host::{render_effect_source_for_test, EffectRenderOptions};

    const TAPS: usize = 256;

    fn probe_a_source() -> String {
        format!(
            r#"
(def in-l (in 1 @name left))
(def in-r (in 2 @name right))
(param output @min 0 @max 2 @default 1)
(def kernel (tensor-param @shape [{taps}] @name fir_taps))
(def win-l (reshape (buffer in-l {taps}) @shape [{taps}]))
(def win-r (reshape (buffer in-r {taps}) @shape [{taps}]))
(def y-l (sum (* win-l kernel)))
(def y-r (sum (* win-r kernel)))
(out (* output y-l) 1 @name left)
(out (* output y-r) 2 @name right)
"#,
            taps = TAPS
        )
    }

    fn render_with_kernel(kernel: Vec<f32>, frames: usize) -> crate::lisp_host::EffectRenderReport {
        render_effect_source_for_test(
            &probe_a_source(),
            &EffectRenderOptions {
                sample_rate: 44_100,
                block_size: 512,
                frames,
                param_overrides: Vec::new(),
                param_events: Vec::new(),
                input_tones: Vec::new(),
                tensor_overrides: vec![("fir_taps".to_string(), kernel)],
                input_overrides: Vec::new(),
            },
        )
        .expect("render probe A")
    }

    /// Kernel with a single unit tap. Establishes (a) the window's time
    /// orientation and (b) that a unit kernel is a bit-exact passthrough at
    /// the expected delay — i.e. the conv path is causal and zero-latency.
    #[test]
    fn probe_a_sliding_fir_is_causal_identity() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let frames = 4096;
        // Try the unit tap at both ends to learn window orientation.
        for (tap, label) in [(TAPS - 1, "last"), (0, "first")] {
            let mut kernel = vec![0.0f32; TAPS];
            kernel[tap] = 1.0;
            let report = render_with_kernel(kernel, frames);
            let first_nonzero = report.first_nonzero_frame;
            eprintln!(
                "probe A tap={label}: peak={} rms={} first_nonzero={:?} first_samples={:?}",
                report.peak,
                report.rms,
                first_nonzero,
                &report.first_samples[..8.min(report.first_samples.len())],
            );
        }
        // Orientation-independent assertion: one of the two unit taps must be
        // an instant passthrough (impulse at frame 0 -> nonzero frame 0).
        let mut causal_hit = false;
        for tap in [0usize, TAPS - 1] {
            let mut kernel = vec![0.0f32; TAPS];
            kernel[tap] = 1.0;
            let report = render_with_kernel(kernel, frames);
            if report.first_nonzero_frame == Some(0) {
                causal_hit = true;
            }
        }
        assert!(
            causal_hit,
            "neither end tap produced a zero-latency passthrough; window semantics differ from expectations"
        );
    }

    /// A unit tap k slots away from the passthrough tap must delay the signal
    /// by exactly k samples: conv taps map to integer delays.
    #[test]
    fn probe_a_taps_map_to_integer_delays() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let frames = 2048;
        // Find the passthrough tap first.
        let passthrough_tap = [0usize, TAPS - 1]
            .into_iter()
            .find(|&tap| {
                let mut kernel = vec![0.0f32; TAPS];
                kernel[tap] = 1.0;
                render_with_kernel(kernel, frames).first_nonzero_frame == Some(0)
            })
            .expect("no passthrough tap found");
        let delay_dir: isize = if passthrough_tap == 0 { 1 } else { -1 };

        for delay in [1usize, 7, 64, 200] {
            let tap = (passthrough_tap as isize + delay_dir * delay as isize) as usize;
            let mut kernel = vec![0.0f32; TAPS];
            kernel[tap] = 1.0;
            let report = render_with_kernel(kernel, frames);
            assert_eq!(
                report.first_nonzero_frame,
                Some(delay),
                "unit tap at {tap} (passthrough {passthrough_tap}) should delay by exactly {delay}"
            );
        }
    }

    /// Probe C: the entire causal chain — a host-writable magnitude curve,
    /// cepstral minimum-phase conversion at hop rate, and the per-sample
    /// sliding-window FIR from probe A. N=512 spectrum -> 256-tap kernel.
    ///
    /// Chain: mirror half-spectrum -> log -> ifft (real cepstrum) -> causal
    /// fold (double 1..N/2-1, keep 0 and N/2, zero the rest) -> fft ->
    /// exp/cos/sin (complex exp) -> ifft -> first 256 taps, reversed to match
    /// the window's newest-sample-last orientation.
    fn probe_c_source() -> String {
        r#"
(def in-l (in 1 @name left))
(def in-r (in 2 @name right))
(param gain @min 0 @max 4 @default 1)
(param output @min 0 @max 2 @default 1)

(def mags (tensor-param @shape [257] @name mag_curve))

(def g-h (hop-hold (clip gain 0 4) 512))
(def fold-index (min (iota 512) (- 512 (iota 512))))
(def full-mag (gather (* mags g-h) fold-index))

(def logm (log (max full-mag 0.000001)))
(def c-re (ifft logm (* logm 0) @N 512 @backend accelerated))

(def idx (iota 512))
(def fold-w (+ (+ (eq idx 0) (eq idx 256))
               (* 2 (* (gte idx 1) (lte idx 255)))))
(def folded (* c-re fold-w))

(def (l-re l-im) (fft folded @N 512 @backend accelerated))
(def hmag (exp l-re))
(def h-re (* hmag (cos l-im)))
(def h-im (* hmag (sin l-im)))
(def ir-td (ifft h-re h-im @N 512 @backend accelerated))

; The hop-gated kernel chain must cross to frame rate before the conv:
; without a latch, the multiply-reduce below executes only on hop frames
; (output zero elsewhere). latch with a frame-rate cond re-emits the held
; kernel every frame.
(def ir-held (latch ir-td 1))
(def kernel (gather ir-held (- 255 (iota 256))))

(def win-l (reshape (buffer in-l 256) @shape [256]))
(def win-r (reshape (buffer in-r 256) @shape [256]))
(def y-l (sum (* win-l kernel)))
(def y-r (sum (* win-r kernel)))

(out (* output y-l) 1 @name left)
(out (* output y-r) 2 @name right)
"#
        .to_string()
    }

    fn render_probe_c(
        mags: Vec<f32>,
        frames: usize,
        tones: Vec<(usize, f32, f32)>,
        events: Vec<crate::lisp_host::InstrumentParamEvent>,
    ) -> crate::lisp_host::EffectRenderReport {
        render_effect_source_for_test(
            &probe_c_source(),
            &EffectRenderOptions {
                sample_rate: 44_100,
                block_size: 512,
                frames,
                param_overrides: Vec::new(),
                param_events: events,
                input_tones: tones,
                tensor_overrides: vec![("mag_curve".to_string(), mags)],
                input_overrides: Vec::new(),
            },
        )
        .expect("render probe C")
    }

    /// Steady-state gain of a sine probe, measured over the tail half of the
    /// render to skip kernel warmup.
    fn tail_rms(samples: &[f32], channel: usize) -> f32 {
        let frames = samples.len() / 2;
        let start = frames / 2;
        let mut acc = 0.0f64;
        let mut count = 0usize;
        for frame in start..frames {
            let value = samples[frame * 2 + channel] as f64;
            acc += value * value;
            count += 1;
        }
        (acc / count.max(1) as f64).sqrt() as f32
    }

    /// Flat unit magnitudes must produce a unity minimum-phase kernel (a
    /// delta), so a sine probe passes at unity gain.
    #[test]
    fn probe_c_flat_curve_is_unity() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let report = render_probe_c(
            vec![1.0; 257],
            8192,
            vec![(0, 441.0, 0.3), (1, 441.0, 0.3)],
            Vec::new(),
        );
        let gain = tail_rms(&report.samples, 0) / (0.3 / std::f32::consts::SQRT_2);
        eprintln!("probe C flat-curve unity gain: {gain}");
        assert!(
            (gain - 1.0).abs() < 0.05,
            "flat magnitude curve should pass a sine at unity, got gain {gain}"
        );
    }

    /// A smooth lowpass magnitude curve must pass low sines and attenuate
    /// high ones by roughly the curve's target amount.
    #[test]
    fn probe_c_lowpass_curve_shapes_spectrum() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        // Smooth rolloff centered near bin 32 (~2.76 kHz at N=512/44.1k).
        let mags: Vec<f32> = (0..257)
            .map(|bin| {
                let x = (bin as f32 - 32.0) / 8.0;
                (1.0 / (1.0 + x.exp())).max(1.0e-4)
            })
            .collect();

        let bin_hz = 44_100.0 / 512.0;
        let low = render_probe_c(
            mags.clone(),
            8192,
            vec![(0, 8.0 * bin_hz, 0.3), (1, 8.0 * bin_hz, 0.3)],
            Vec::new(),
        );
        let high = render_probe_c(
            mags,
            8192,
            vec![(0, 96.0 * bin_hz, 0.3), (1, 96.0 * bin_hz, 0.3)],
            Vec::new(),
        );
        let low_gain = tail_rms(&low.samples, 0) / (0.3 / std::f32::consts::SQRT_2);
        let high_gain = tail_rms(&high.samples, 0) / (0.3 / std::f32::consts::SQRT_2);
        eprintln!("probe C lowpass: low-bin gain {low_gain}, high-bin gain {high_gain}");
        assert!(
            (low_gain - 1.0).abs() < 0.1,
            "passband sine should be ~unity, got {low_gain}"
        );
        assert!(
            high_gain < 0.05,
            "stopband sine should be strongly attenuated, got {high_gain}"
        );
    }

    /// The kernel is rebuilt at hop rate: a gain param event mid-render must
    /// change the output level within a few hops.
    #[test]
    fn probe_c_kernel_updates_at_hop_rate() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let frames = 16_384;
        let report = render_probe_c(
            vec![1.0; 257],
            frames,
            vec![(0, 441.0, 0.3), (1, 441.0, 0.3)],
            vec![crate::lisp_host::InstrumentParamEvent {
                frame: 8192,
                name: "gain".to_string(),
                value: 0.25,
            }],
        );
        let rms_of = |start: usize, end: usize| {
            let mut acc = 0.0f64;
            for frame in start..end {
                let value = report.samples[frame * 2] as f64;
                acc += value * value;
            }
            ((acc / (end - start) as f64).sqrt()) as f32
        };
        let before = rms_of(4096, 8192);
        // Skip a generous settling window after the event (a few hops).
        let after = rms_of(12_288, 16_384);
        eprintln!("probe C hop update: rms before {before}, after {after}");
        assert!(
            (after / before - 0.25).abs() < 0.05,
            "gain event should scale the kernel by 0.25: before {before}, after {after}"
        );
    }

    /// Wall-clock cost of the full causal chain (min-phase rebuild each hop +
    /// per-sample conv) for 1s stereo.
    #[test]
    fn probe_c_cost() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let _ = render_probe_c(vec![1.0; 257], 512, Vec::new(), Vec::new());
        let start = std::time::Instant::now();
        let _ = render_probe_c(vec![1.0; 257], 44_100, Vec::new(), Vec::new());
        eprintln!("probe C cost: full causal chain {:?} for 1s stereo", start.elapsed());
    }

    /// Wall-clock cost of the per-sample 256-tap conv, versus the shipping
    /// spectral DSP rendering the same duration. Not an assertion-driven test
    /// -- prints numbers for the go/no-go decision.
    #[test]
    fn probe_a_cost_vs_spectral() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let frames = 44_100; // one second
        let mut kernel = vec![0.0f32; TAPS];
        kernel[0] = 1.0;
        // Warm the dylib cache before timing.
        let _ = render_with_kernel(kernel.clone(), 512);
        let start = std::time::Instant::now();
        let report = render_with_kernel(kernel, frames);
        let causal_elapsed = start.elapsed();

        let spectral_source = crate::effects::filter_table::dsp_source();
        let spectral_options = EffectRenderOptions {
            sample_rate: 44_100,
            block_size: 512,
            frames,
            param_overrides: Vec::new(),
            param_events: Vec::new(),
            input_tones: Vec::new(),
            tensor_overrides: Vec::new(),
            input_overrides: Vec::new(),
        };
        let _ = render_effect_source_for_test(
            spectral_source,
            &EffectRenderOptions { frames: 512, ..spectral_options.clone() },
        )
        .expect("warm spectral dylib");
        let start = std::time::Instant::now();
        let _ = render_effect_source_for_test(spectral_source, &spectral_options)
            .expect("render spectral DSP");
        let spectral_elapsed = start.elapsed();

        eprintln!(
            "probe A cost: causal 256-tap conv {causal_elapsed:?} vs spectral STFT {spectral_elapsed:?} for 1s stereo (causal peak {})",
            report.peak
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::effects::filter_table::{
        causal_dsp_source, dsp_source, tests::render_lock, TableEngine, FRAMES, NBINS, TABLE_LEN,
    };
    use crate::lisp_host::{render_effect_source_for_test, EffectRenderOptions};

    fn base_options(frames: usize) -> EffectRenderOptions {
        EffectRenderOptions {
            sample_rate: 44_100,
            block_size: 512,
            frames,
            param_overrides: Vec::new(),
            param_events: Vec::new(),
            input_tones: Vec::new(),
            tensor_overrides: Vec::new(),
            input_overrides: Vec::new(),
        }
    }

    /// Cutoff that makes the table read stride exactly 1 (reference harmonic
    /// 24 pinned to its own bin): 24 * samplerate / N.
    const IDENTITY_CUTOFF: f32 = 24.0 * 44_100.0 / 2048.0;

    fn flat_table() -> Vec<f32> {
        vec![1.0; TABLE_LEN]
    }

    /// Same smooth lowpass row in every frame: sigmoid rolloff around
    /// `edge_bin`, floored at 1e-4 like imported tables.
    fn lowpass_table(edge_bin: f32) -> Vec<f32> {
        let row: Vec<f32> = (0..NBINS)
            .map(|bin| {
                let x = (bin as f32 - edge_bin) / 24.0;
                (1.0 / (1.0 + x.exp())).max(1.0e-4)
            })
            .collect();
        let mut table = Vec::with_capacity(TABLE_LEN);
        for _ in 0..FRAMES {
            table.extend_from_slice(&row);
        }
        table
    }

    fn tail_rms(samples: &[f32], channel: usize) -> f32 {
        let frames = samples.len() / 2;
        let start = frames / 2;
        let mut acc = 0.0f64;
        for frame in start..frames {
            let value = samples[frame * 2 + channel] as f64;
            acc += value * value;
        }
        ((acc / (frames - start) as f64).sqrt()) as f32
    }

    fn steady_gain(source: &str, table: Vec<f32>, cutoff: f32, freq: f32) -> f32 {
        let amp = 0.2f32;
        let report = render_effect_source_for_test(
            source,
            &EffectRenderOptions {
                param_overrides: vec![
                    ("mix".to_string(), 1.0),
                    ("resonance".to_string(), 0.0),
                    ("frame".to_string(), 0.0),
                    ("cutoff".to_string(), cutoff),
                ],
                input_tones: vec![(0, freq, amp), (1, freq, amp)],
                tensor_overrides: vec![("table_magnitudes".to_string(), table)],
                ..base_options(16_384)
            },
        )
        .expect("render filter table engine");
        tail_rms(&report.samples, 0) / (amp / std::f32::consts::SQRT_2)
    }

    #[test]
    fn engine_refs_round_trip() {
        use crate::effects::filter_table::{compose_engine_ref, split_engine_ref};
        // Default engine composes to the bare reference (existing projects
        // stay byte-identical) and unknown or absent suffixes decode as it.
        assert_eq!(compose_engine_ref("fltab:glass-comb", TableEngine::Spectral), "fltab:glass-comb");
        assert_eq!(
            split_engine_ref("fltab:glass-comb"),
            ("fltab:glass-comb", TableEngine::Spectral)
        );
        assert_eq!(
            split_engine_ref("fltab:glass-comb#ft-engine=warp"),
            ("fltab:glass-comb#ft-engine=warp", TableEngine::Spectral)
        );
        // Causal round-trips, including stacked on an analysis-mode suffix.
        let composed = compose_engine_ref("kick#ft-mode=wavetable", TableEngine::Causal);
        assert_eq!(composed, "kick#ft-mode=wavetable#ft-engine=causal");
        assert_eq!(
            split_engine_ref(&composed),
            ("kick#ft-mode=wavetable", TableEngine::Causal)
        );
    }

    #[test]
    fn engine_identified_from_retained_source() {
        use crate::effects::filter_table::engine_for_source;
        assert_eq!(engine_for_source(dsp_source()), Some(TableEngine::Spectral));
        assert_eq!(
            engine_for_source(causal_dsp_source()),
            Some(TableEngine::Causal)
        );
        assert_eq!(engine_for_source("(out (in 1) 1)"), None);
    }

    #[test]
    fn latency_is_engine_dependent() {
        assert_eq!(
            TableEngine::Spectral.latency_samples(),
            crate::effects::filter_table::N
        );
        assert_eq!(TableEngine::Causal.latency_samples(), 0);
        assert_eq!(TableEngine::default(), TableEngine::Spectral);
        assert_eq!(TableEngine::from_tag("causal"), Some(TableEngine::Causal));
        assert_eq!(TableEngine::from_tag("spectral"), Some(TableEngine::Spectral));
        assert_eq!(TableEngine::Causal.toggled(), TableEngine::Spectral);
    }

    /// A flat table is a unity response: the causal engine must pass the
    /// harness impulse at frame 0 (zero latency, wet-only) at full amplitude.
    #[test]
    fn causal_engine_is_zero_latency_unity_on_flat_table() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let report = render_effect_source_for_test(
            causal_dsp_source(),
            &EffectRenderOptions {
                param_overrides: vec![
                    ("mix".to_string(), 1.0),
                    ("cutoff".to_string(), IDENTITY_CUTOFF),
                ],
                tensor_overrides: vec![("table_magnitudes".to_string(), flat_table())],
                ..base_options(8192)
            },
        )
        .expect("render causal engine");
        assert_eq!(
            report.first_nonzero_frame,
            Some(0),
            "causal engine must be zero-latency"
        );
        // Harness impulse is 0.45 on the left channel at frame 0.
        let first = report.first_samples[0];
        assert!(
            (first - 0.45).abs() < 0.05,
            "flat table should be near-unity at t=0, got {first}"
        );
    }

    /// Both engines derive the response from the same head, so their
    /// steady-state magnitudes must agree in the passband and both must
    /// attenuate the stopband hard. (The causal engine's 256-tap truncation
    /// smooths the edge, so parity is asserted loosely and away from it.)
    #[test]
    fn causal_matches_spectral_steady_state() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let bin_hz = 44_100.0 / 2048.0;
        let table = || lowpass_table(300.0); // edge ~6.5 kHz
        let pass_freq = 100.0 * bin_hz; // ~2.15 kHz
        let stop_freq = 700.0 * bin_hz; // ~15 kHz

        let causal_pass = steady_gain(causal_dsp_source(), table(), IDENTITY_CUTOFF, pass_freq);
        let spectral_pass = steady_gain(dsp_source(), table(), IDENTITY_CUTOFF, pass_freq);
        let causal_stop = steady_gain(causal_dsp_source(), table(), IDENTITY_CUTOFF, stop_freq);
        let spectral_stop = steady_gain(dsp_source(), table(), IDENTITY_CUTOFF, stop_freq);
        eprintln!(
            "parity: pass causal={causal_pass} spectral={spectral_pass}, stop causal={causal_stop} spectral={spectral_stop}"
        );
        assert!(
            (causal_pass / spectral_pass - 1.0).abs() < 0.25,
            "passband gains should agree: causal {causal_pass}, spectral {spectral_pass}"
        );
        assert!(
            causal_stop < 0.02 && spectral_stop < 0.02,
            "both engines should attenuate the stopband: causal {causal_stop}, spectral {spectral_stop}"
        );
    }

    /// Cutoff must translate the causal response along the frequency axis
    /// exactly as in spectral mode: halving cutoff halves the edge frequency.
    #[test]
    fn causal_cutoff_translates_response() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let bin_hz = 44_100.0 / 2048.0;
        let table = || lowpass_table(232.0); // edge ~5 kHz at identity cutoff
        let probe = 160.0 * bin_hz; // ~3.4 kHz

        let open = steady_gain(causal_dsp_source(), table(), IDENTITY_CUTOFF, probe);
        let closed = steady_gain(causal_dsp_source(), table(), IDENTITY_CUTOFF * 0.5, probe);
        eprintln!("causal cutoff translate: open gain {open}, closed gain {closed}");
        assert!(open > 0.7, "probe should pass at identity cutoff, got {open}");
        assert!(
            closed < open * 0.25,
            "halving cutoff should attenuate the probe well below the open gain: open {open}, closed {closed}"
        );
    }

    /// Cost comparison of the two full engines, printed for profiling.
    #[test]
    fn engine_cost_comparison() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let mut timings = Vec::new();
        for (label, source) in [("spectral", dsp_source()), ("causal", causal_dsp_source())] {
            let options = EffectRenderOptions {
                tensor_overrides: vec![("table_magnitudes".to_string(), flat_table())],
                ..base_options(44_100)
            };
            let _ = render_effect_source_for_test(
                source,
                &EffectRenderOptions { frames: 512, ..options.clone() },
            )
            .expect("warm dylib");
            let start = std::time::Instant::now();
            let _ = render_effect_source_for_test(source, &options).expect("render engine");
            timings.push((label, start.elapsed()));
        }
        eprintln!("engine cost for 1s stereo: {timings:?}");
    }
}

#[cfg(test)]
mod click_tests {
    use crate::effects::filter_table::{causal_dsp_source, tests::render_lock, FRAMES, NBINS};
    use crate::lisp_host::{render_effect_source_for_test, EffectRenderOptions, InstrumentParamEvent};

    /// Frame-varying table: a lowpass whose edge sweeps from bin 60 to bin 460
    /// across the 64 frames, so morphing `frame` is a large timbral change.
    fn morph_table() -> Vec<f32> {
        let mut table = Vec::with_capacity(FRAMES * NBINS);
        for frame in 0..FRAMES {
            let edge = 60.0 + (frame as f32 / (FRAMES - 1) as f32) * 400.0;
            for bin in 0..NBINS {
                let x = (bin as f32 - edge) / 24.0;
                table.push((1.0 / (1.0 + x.exp())).max(1.0e-4));
            }
        }
        table
    }

    /// Worst absolute sample-to-sample jump on the left channel, skipping the
    /// startup region. Clicks are exactly large first differences a smooth
    /// signal cannot produce.
    fn max_delta(samples: &[f32], skip_frames: usize) -> f32 {
        let frames = samples.len() / 2;
        let mut worst = 0.0f32;
        for frame in (skip_frames + 1)..frames {
            let delta = (samples[frame * 2] - samples[(frame - 1) * 2]).abs();
            worst = worst.max(delta);
        }
        worst
    }

    fn render_frame_sweep(source: &str) -> (f32, f32) {
        let frames = 88_200; // 2 s
        // A knob-drag-shaped frame sweep: many small hop-misaligned steps.
        let events: Vec<InstrumentParamEvent> = (0..400)
            .map(|step| InstrumentParamEvent {
                frame: 4_000 + step * 200,
                name: "frame".to_string(),
                value: (step as f32 / 399.0).min(1.0),
            })
            .collect();
        let report = render_effect_source_for_test(
            source,
            &EffectRenderOptions {
                sample_rate: 44_100,
                block_size: 512,
                frames,
                param_overrides: vec![
                    ("mix".to_string(), 1.0),
                    ("resonance".to_string(), 0.0),
                    ("frame".to_string(), 0.0),
                    ("cutoff".to_string(), 24.0 * 44_100.0 / 2048.0),
                ],
                param_events: events,
                input_tones: vec![(0, 441.0, 0.3), (1, 441.0, 0.3)],
                tensor_overrides: vec![("table_magnitudes".to_string(), morph_table())],
                input_overrides: Vec::new(),
            },
        )
        .expect("render frame sweep");
        let peak = report
            .samples
            .iter()
            .step_by(2)
            .fold(0.0f32, |acc, &v| acc.max(v.abs()));
        (max_delta(&report.samples, 4_000), peak)
    }

    /// Morphing `frame` must not click: the per-tap kernel slew turns each
    /// hop's kernel step into an exponential glide. Rendered against a no-slew
    /// variant (slew time 0 -> alpha 1 -> instant swap) to prove the test
    /// detects the artifact the slew removes.
    #[test]
    fn frame_morph_is_click_free_via_kernel_slew() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let slewed_source = causal_dsp_source();
        assert!(
            slewed_source.contains("(def KERNEL-SLEW-MS 8)"),
            "test assumes the shipped slew constant"
        );
        let no_slew_source =
            slewed_source.replace("(def KERNEL-SLEW-MS 8)", "(def KERNEL-SLEW-MS 0)");

        let (slewed_delta, slewed_peak) = render_frame_sweep(slewed_source);
        let (raw_delta, raw_peak) = render_frame_sweep(&no_slew_source);
        // 441 Hz sine: natural per-sample delta is peak * 2*pi*441/44100 ~ 0.063*peak.
        let natural = slewed_peak * 2.0 * std::f32::consts::PI * 441.0 / 44_100.0;
        eprintln!(
            "frame morph clicks: slewed max-delta {slewed_delta} (peak {slewed_peak}, natural {natural}), no-slew max-delta {raw_delta} (peak {raw_peak})"
        );
        assert!(
            slewed_delta < natural * 1.6,
            "slewed kernel should produce no jumps beyond the sine's own slope: max-delta {slewed_delta}, natural {natural}"
        );
        assert!(
            slewed_delta < raw_delta * 0.7,
            "slew should measurably reduce hop-boundary discontinuities: slewed {slewed_delta}, raw {raw_delta}"
        );
    }
}

/// Temporary cost-attribution probes for the causal-engine CPU regression
/// investigation. Each variant strips or restructures one stage of the causal
/// tail so timing deltas attribute cycles to that stage.
#[cfg(test)]
mod cost_probes {
    use crate::effects::filter_table::{
        causal_dsp_source, dsp_source, tests::render_lock, TABLE_LEN,
    };
    use crate::lisp_host::{render_effect_source_for_test, EffectRenderOptions};

    const KTARGET_LINE: &str =
        "(def kernel-target (latch (* (gather ir-mp rev-idx) tap-fade) (eq hop-phase-next 1)))";
    const SLEW_KERNEL_DEF: &str = "(def kernel
  (write-history kern-slew
    (+ (* (- 1 kern-seeded) kernel-target)
       (* kern-seeded (+ kern-prev (* slew-alpha (- kernel-target kern-prev)))))))";
    const CONV_L: &str = "(def wet-l (sum (* win-l kernel)))";
    const CONV_R: &str = "(def wet-r (sum (* win-r kernel)))";

    /// The pre-optimization structure: latch the full [2048] IR at frame rate,
    /// gather+fade downstream (per frame). Kept as the equivalence reference
    /// and to keep the ~24% win measurable.
    fn big_latch_source() -> String {
        let src = causal_dsp_source();
        assert!(src.contains(KTARGET_LINE), "shipped tail changed; update cost_probes");
        src.replace(
            KTARGET_LINE,
            "(def ir-held (latch ir-mp (eq hop-phase-next 1)))\n(def kernel-target (* (gather ir-held rev-idx) tap-fade))",
        )
    }

    fn variants() -> Vec<(&'static str, String)> {
        let full = causal_dsp_source().to_string();
        vec![
            ("spectral", dsp_source().to_string()),
            ("causal", full.clone()),
            // Pre-optimization structure: [2048] frame-rate latch.
            ("big-latch", big_latch_source()),
            // One conv removed: delta = cost of a single 256-tap conv path.
            ("one-conv", full.replace(CONV_R, "(def wet-r wet-l)")),
            // No convs at all (kernel reduced instead): keeps ring buffers out.
            (
                "no-conv",
                full.replace(CONV_L, "(def wet-l (sum kernel))")
                    .replace(CONV_R, "(def wet-r wet-l)"),
            ),
            // Slew bypassed: kernel switches instantly at hop boundaries.
            ("no-slew", full.replace(SLEW_KERNEL_DEF, "(def kernel kernel-target)")),
            // Kernel is a constant: no latch; note the cepstral hop chain is
            // NOT dead-code eliminated, so this still includes head + FFTs.
            (
                "const-kernel",
                full.replace(KTARGET_LINE, "(def kernel-target (* (iota 256) 0.001))"),
            ),
        ]
    }

    #[test]
    fn causal_cost_attribution() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }
        let options = EffectRenderOptions {
            sample_rate: 44_100,
            block_size: 512,
            frames: 44_100,
            param_overrides: vec![("mix".to_string(), 1.0)],
            param_events: Vec::new(),
            input_tones: vec![(0, 441.0, 0.3), (1, 441.0, 0.3)],
            tensor_overrides: vec![("table_magnitudes".to_string(), vec![1.0; TABLE_LEN])],
            input_overrides: Vec::new(),
        };
        let mut rows = Vec::new();
        for (label, source) in variants() {
            let _ = render_effect_source_for_test(
                &source,
                &EffectRenderOptions { frames: 512, ..options.clone() },
            )
            .expect("warm dylib");
            let mut best = std::time::Duration::MAX;
            for _ in 0..3 {
                let start = std::time::Instant::now();
                let _ = render_effect_source_for_test(&source, &options).expect("render variant");
                best = best.min(start.elapsed());
            }
            rows.push((label, best));
        }
        eprintln!("cost attribution (1s stereo, best of 3):");
        for (label, time) in rows {
            eprintln!("  {label:<12} {time:?}");
        }
    }

    /// The shipped small-latch structure must be output-equivalent to the
    /// original big-latch tail (same capture frames, same taps), including
    /// under hop-misaligned frame modulation.
    #[test]
    fn small_latch_matches_shipped_output() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }
        let options = EffectRenderOptions {
            sample_rate: 44_100,
            block_size: 512,
            frames: 8_192,
            param_overrides: vec![
                ("mix".to_string(), 1.0),
                ("cutoff".to_string(), 24.0 * 44_100.0 / 2048.0),
            ],
            param_events: (0..40)
                .map(|step| crate::lisp_host::InstrumentParamEvent {
                    frame: 500 + step * 180,
                    name: "frame".to_string(),
                    value: (step as f32 / 39.0).min(1.0),
                })
                .collect(),
            input_tones: vec![(0, 441.0, 0.3), (1, 441.0, 0.3)],
            tensor_overrides: vec![(
                "table_magnitudes".to_string(),
                (0..TABLE_LEN)
                    .map(|i| 1.0 / (1.0 + (((i % 1025) as f32 - 200.0) / 24.0).exp()))
                    .collect(),
            )],
            input_overrides: Vec::new(),
        };
        let a = render_effect_source_for_test(causal_dsp_source(), &options)
            .expect("render shipped");
        let b = render_effect_source_for_test(&big_latch_source(), &options)
            .expect("render big-latch");
        let worst = a
            .samples
            .iter()
            .zip(&b.samples)
            .fold(0.0f32, |acc, (x, y)| acc.max((x - y).abs()));
        eprintln!("small-latch vs shipped worst sample delta: {worst}");
        assert!(worst < 1.0e-4, "small-latch variant diverged: {worst}");
    }
}
