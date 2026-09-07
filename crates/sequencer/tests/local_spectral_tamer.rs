//! Opt-in coverage for the gitignored local-library effect.
use sequencer::lisp_host::{
    compile_and_load, effect_sidechain_inputs, effect_source_path,
    render_loaded_effect_for_test, EffectRenderOptions, InstrumentParamEvent,
};

fn max_difference(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
}

#[test]
#[ignore = "requires local spectral-tamer and DGenLisp >= v0.1.9"]
fn every_parameter_responds_to_all_four_modulator_inputs() {
    let source = std::fs::read_to_string(effect_source_path("spectral-tamer"))
        .expect("local spectral-tamer DSP");
    let compiled = compile_and_load(&source, 48_000).expect("compile spectral-tamer");
    assert_eq!(compiled.manifest.mod_destinations.len(), 12);
    assert_eq!(compiled.manifest.modulators.len(), 4);
    assert_eq!(effect_sidechain_inputs(&compiled.manifest).iter()
        .map(|input| input.input_channel).collect::<Vec<_>>(), vec![2, 3]);

    let options = || EffectRenderOptions {
        sample_rate: 48_000,
        block_size: 512,
        frames: 16_384,
        param_overrides: vec![],
        param_events: vec![],
        input_tones: vec![],
        input_overrides: vec![],
        tensor_overrides: vec![],
    };
    let render = |options: &EffectRenderOptions| {
        let report = render_loaded_effect_for_test(&compiled.manifest, &compiled.lib, options)
            .expect("render spectral-tamer");
        assert!(report.samples.iter().all(|sample| sample.is_finite()));
        report.samples
    };
    let unchanged = render(&options());
    for (name, target) in [
        ("amount", 1.0), ("attack", 0.0), ("release", 1.0), ("gate", 0.02),
        ("low-cut", 4000.0), ("high-cut", 500.0), ("tilt", 1.0),
        ("sidechain", 1.0), ("freeze", 1.0), ("delta", 1.0),
        ("input", 0.5), ("output", 0.5),
    ] {
        let param = compiled.manifest.params.iter().find(|p| p.name == name).unwrap();
        let mut direct_options = options();
        direct_options.param_events.push(InstrumentParamEvent {
            frame: 4096, name: name.into(), value: target,
        });
        let direct = render(&direct_options);
        assert!(max_difference(&unchanged, &direct) > 1e-7, "{name} must affect audio");
        for slot in 1..=4 {
            let mut modulated_options = options();
            // Audio ports 0/1 and sidechain ports 2/3 must remain untouched.
            modulated_options.input_overrides.push((slot + 3, 1.0));
            modulated_options.param_overrides.push((
                format!("mod {name} slot {slot} amt"), target - param.default,
            ));
            modulated_options.param_events.push(InstrumentParamEvent {
                frame: 4096, name: format!("__dgen_mod_active__{name}"), value: 1.0,
            });
            let modulated = render(&modulated_options);
            let error = max_difference(&direct, &modulated);
            assert!(error < 2e-5, "{name} slot {slot}: modulation/direct mismatch {error}");
        }
    }
}
