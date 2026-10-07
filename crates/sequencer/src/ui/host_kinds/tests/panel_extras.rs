//! Stage 7b-4: what the device panels still read from the legacy panel
//! dicts (sampler media, modulators, sound binding, param UI metadata,
//! effect tables, the meter selector), scene macro config and
//! `step.variant`.

use super::*;

const REFER_EXTRAS: &str = "(import eseq.kinds :refer (track tracks macros scenes \
                            device-param stamp-variant! selection))";

impl Harness {
    fn eval_x(&mut self, code: &str) -> Value {
        let source = format!("{REFER_EXTRAS}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }
}

// ── scene macro config ─────────────────────────────────────────────────

/// The first macro's scene config and its diff count, from the model.
fn scene_config(h: &Harness) -> (sequencer::macro_engine::SceneMacroConfig, Value) {
    let id = h.app.macro_engine.macros()[0].id;
    let config = h.app.macro_engine.scene_config(id).unwrap().clone();
    let diffs = h.app.scene_macro_diff_count(&config) as f64;
    (config, Value::Number(diffs))
}

#[test]
fn scene_macro_config_reads_like_the_model_and_sets_through_history() {
    let (mut h, slot) = Harness::with_devices();
    // A second scene whose Filter cutoff differs from the first's.
    h.command("clone-pattern", Value::Nil);
    h.eval("(host-command \"switch-pattern\" (dict :idx 0 :quantize \"off\"))");
    h.drain();
    h.shared.state.pattern.effect_chains[0][slot]
        .defaults
        .set(2, 300.0);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    let command = app::AppCommand::MacroCreateScene {
        name: "Push".to_string(),
        target_scene: 1,
    };
    app::apply_command(&mut h.app, command);
    app::apply_command(
        &mut h.app,
        app::AppCommand::MacroCreate {
            name: "Plain".to_string(),
        },
    );
    h.sync();
    h.eval_x("(def m (first (macros))) (def plain (nth (macros) 1))");
    // diff-count is computed while observed.
    h.eval_x(r#"(effect-buffer "*diffs*" (label (str m.diff-count plain.diff-count)))"#);
    h.show_all();
    h.sync();
    // As the model holds the config.
    let (config, diffs) = scene_config(&h);
    assert_eq!(h.eval_x("m.type"), s("scene"));
    assert_eq!(
        h.eval_x("m.target-scene"),
        h.eval_x("(nth (scenes) 1)"),
        "the scene instance"
    );
    assert_eq!(config.target_scene, 1);
    assert_eq!(h.eval_x("m.morph-params"), Value::Bool(config.morph_params));
    assert_eq!(
        h.eval_x("m.steal-patterns"),
        Value::Bool(config.steal_patterns)
    );
    assert_eq!(h.eval_x("m.quantize"), s(config.quantize.label()));
    assert_eq!(h.eval_x("m.diff-count"), diffs);
    assert!(
        matches!(h.eval_x("m.diff-count"), Value::Number(n) if n >= 1.0),
        "the cutoff differs"
    );
    assert_eq!(config.track_mask, None);
    assert_eq!(h.eval_x("m.tracks"), h.eval_x("(tracks)"), "every track");
    // A mapped macro reads empty and takes no config.
    assert_eq!(
        h.eval_x("(list plain.target-scene plain.morph-params plain.quantize plain.tracks plain.diff-count)"),
        h.eval_x(r#"(list nil false "" (list) 0)"#)
    );
    h.rejects_in(
        REFER_EXTRAS,
        "(set! plain.morph-params true)",
        "a mapped macro has no morph-params",
        true,
    );
    // Each set! is one undo entry; the current value is a no-op.
    let before = h.app.history.undo_len();
    h.eval_x(r#"(set! m.quantize "sixteenth") (set! m.steal-patterns true)"#);
    h.drain_and_sync();
    assert_eq!(h.app.history.undo_len(), before + 2);
    assert_eq!(
        h.eval_x("(list m.quantize m.steal-patterns)"),
        h.eval_x(r#"(list "sixteenth" true)"#)
    );
    h.eval_x("(set! m.quantize m.quantize) (set! m.tracks m.tracks) (set! m.target-scene m.target-scene)");
    h.drain_and_sync();
    assert_eq!(h.app.history.undo_len(), before + 2, "no-ops");
    // Tracks: by stable id; one track masks the others.
    h.eval_x("(set! m.tracks (list (track 1)))");
    h.drain_and_sync();
    assert_eq!(h.eval_x("m.tracks"), h.eval_x("(list (track 1))"));
    let (config, diffs) = scene_config(&h);
    assert_eq!(config.track_mask, Some(vec![false, true, false]));
    assert_eq!(h.eval_x("m.diff-count"), diffs);
    assert_eq!(
        h.eval_x("m.diff-count"),
        Value::Number(0.0),
        "track 0's cutoff is masked out"
    );
    // The scene: by position.
    h.eval_x("(set! m.target-scene (first (scenes))) (set! m.morph-params false)");
    h.drain_and_sync();
    assert_eq!(h.eval_x("m.target-scene"), h.eval_x("(first (scenes))"));
    assert_eq!(h.eval_x("m.morph-params"), Value::Bool(false));
    assert_eq!(h.app.history.undo_len(), before + 5);
    for _ in 0..5 {
        h.undo();
    }
    h.sync();
    assert_eq!(
        h.eval_x("(list m.quantize m.steal-patterns m.morph-params m.target-scene m.tracks)"),
        h.eval_x(r#"(list "bar" false true (nth (scenes) 1) (tracks))"#)
    );
    // The value rule.
    h.rejects_in(REFER_EXTRAS, r#"(set! m.quantize "1/3")"#, "one of", true);
    h.rejects_in(
        REFER_EXTRAS,
        r#"(host-command "set-macro" (dict :macro-id m.mid :field "morph-params" :value 1))"#,
        "true or false",
        true,
    );
    h.rejects_in(
        REFER_EXTRAS,
        r#"(host-command "set-macro" (dict :macro-id m.mid :field "target-scene" :value 5))"#,
        "an integer from 0 to 1",
        true,
    );
    h.rejects_in(
        REFER_EXTRAS,
        r#"(host-command "set-macro" (dict :macro-id m.mid :field "tracks" :value (list 999)))"#,
        "the track is gone",
        true,
    );
    // diff-count is not recomputed while nothing moved.
    let diffs = h.frame.host_kinds.macros.diff_syncs;
    h.sync();
    h.sync();
    assert_eq!(h.frame.host_kinds.macros.diff_syncs, diffs);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.frame.host_kinds.macros.diff_syncs, diffs + 1);
}

#[test]
fn an_unobserved_diff_count_costs_no_walk_until_observed() {
    let (mut h, slot) = Harness::with_devices();
    h.command("clone-pattern", Value::Nil);
    h.eval("(host-command \"switch-pattern\" (dict :idx 0 :quantize \"off\"))");
    h.drain();
    h.shared.state.pattern.effect_chains[0][slot]
        .defaults
        .set(2, 300.0);
    let command = app::AppCommand::MacroCreateScene {
        name: "Push".to_string(),
        target_scene: 1,
    };
    app::apply_command(&mut h.app, command);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_x("(def m (first (macros)))");
    // Unobserved: no walk, whatever moves.
    for _ in 0..2 {
        h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        h.sync();
    }
    assert_eq!(h.frame.host_kinds.macros.diff_syncs, 0);
    // A new observer gets the count at once, though nothing moved.
    h.eval_x(r#"(effect-buffer "*diff*" (label (str m.diff-count)))"#);
    h.show_all();
    h.sync();
    assert_eq!(h.frame.host_kinds.macros.diff_syncs, 1);
    assert_eq!(h.eval_x("m.diff-count"), scene_config(&h).1);
    assert!(matches!(h.eval_x("m.diff-count"), Value::Number(n) if n >= 1.0));
    h.sync();
    assert_eq!(h.frame.host_kinds.macros.diff_syncs, 1, "nothing moved");
}

// ── step.variant ───────────────────────────────────────────────────────

#[test]
fn step_variant_names_the_variant_instance_the_step_plays() {
    let (mut h, slot) = Harness::with_devices();
    // Steps 2 and 4 share one lock set (A), step 6 another (B).
    for (step, value) in [(2, 500.0), (4, 500.0), (6, 800.0)] {
        h.lock_effect(slot, step, 2, value);
    }
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    h.eval_x(
        "(def t0 (track 0)) (def s2 (nth t0.steps 2)) (def s3 (nth t0.steps 3)) \
         (def s6 (nth t0.steps 6))",
    );
    // A cold read registers the track's variants.
    assert_eq!(h.eval_x("s2.variant"), h.eval_x("(first t0.variants)"));
    assert_eq!(h.eval_x("s6.variant"), h.eval_x("(nth t0.variants 1)"));
    assert_eq!(h.eval_x("s3.variant"), Value::Nil);
    // Legacy parity: the registry's assignment of each step.
    let assignments = h.shared.state.reconcile_plock_variant_registry_for_track(0);
    for step in 0..8 {
        let label = assignments
            .get(step)
            .cloned()
            .flatten()
            .map_or(Value::Nil, |assignment| s(&assignment.label));
        let code =
            format!("(let ((st (nth t0.steps {step}))) (if st.variant st.variant.label nil))");
        assert_eq!(h.eval_x(&code), label, "step {step}");
    }
    // Observed: pushed by the step diff when the track's p-locks moved.
    h.eval_x(
        r#"(def a (first t0.variants)) (def b (nth t0.variants 1))
           (effect-buffer "*step-variant*" (label (str s2.variant s6.variant)))"#,
    );
    h.show_all();
    h.sync();
    let computed = h.computed(f::STEP_VARIANT);
    h.sync();
    h.sync();
    assert_eq!(h.computed(f::STEP_VARIANT), computed, "nothing moved");
    h.eval_x("(stamp-variant! t0 (list s2) b)");
    h.drain_and_sync();
    assert_eq!(h.eval_x("s2.variant"), h.eval_x("b"));
    // B's last step cleared: B goes, its steps play none.
    h.eval_x("(stamp-variant! t0 (list s2 s6) nil)");
    h.drain_and_sync();
    assert_eq!(
        h.eval_x("(list s2.variant s6.variant)"),
        h.eval_x("(list nil nil)")
    );
    assert_eq!(h.eval_x("b.label"), s(""), "B's handle is stale");
    assert_eq!(
        h.eval_x("(let ((s4 (nth t0.steps 4))) s4.variant)"),
        h.eval_x("a")
    );
}

// ── the panel header: display name, sound binding, meter, modulators ──

/// `flt` (track 0's Filter; the track has no instrument) and `inst`, the
/// sampler on track 2.
const DEVICES_X: &str = "(def t0 (track 0)) (def flt (first t0.devices)) \
                         (def t2 (track 2)) (def inst (first t2.devices))";

/// The legacy instrument panel's entry for `track`.
fn legacy_instrument(h: &Harness, track: usize) -> Value {
    let panel = build_instrument_panel_value(&h.app, track, &h.shared.selected_steps);
    items(&panel).remove(0)
}

/// The legacy effects panel's slot maps of `track`.
fn legacy_effects(h: &Harness, track: usize) -> Vec<Value> {
    let descriptors = &h.app.graph.effect_descriptors;
    items(&build_effects_value(
        &h.shared.state,
        track,
        descriptors,
        &h.shared.selected_steps,
    ))
}

#[test]
fn the_panel_header_reads_like_the_legacy_panel_dicts() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_x(DEVICES_X);
    // The sound binding is computed while observed ("" until then).
    assert_eq!(h.eval_x("inst.sound-binding"), s(""));
    h.eval_x(r#"(effect-buffer "*binding*" (label inst.sound-binding))"#);
    h.show_all();
    h.sync();
    for (device, track) in [("inst", 2)] {
        let legacy = legacy_instrument(&h, track);
        let expected = instrument_panel_display_name(&h.app, track);
        assert_eq!(h.eval_x(&format!("{device}.display-name")), s(&expected));
        if get(&legacy, "display-name") != Value::Nil {
            assert_eq!(s(&expected), get(&legacy, "display-name"));
        }
        let binding = match get(&legacy, "sound-binding") {
            Value::Nil => s(""),
            label => label,
        };
        assert_eq!(h.eval_x(&format!("{device}.sound-binding")), binding);
        assert_eq!(h.eval_x(&format!("{device}.meter")), get(&legacy, "meter"));
    }
    let effects = legacy_effects(&h, 0);
    let filter = &effects[slot];
    assert_eq!(h.eval_x("flt.meter"), get(filter, "meter"));
    assert_eq!(h.eval_x("flt.display-name"), get(filter, "name"));
    assert_eq!(h.eval_x("flt.sound-binding"), s(""));
    // The fixed modulators: the descriptor's, as the panels list them.
    let desc = h.app.graph.effect_descriptors[0][slot].clone();
    assert!(
        !desc.instrument_modulators.is_empty(),
        "the Filter has some"
    );
    let read = items(
        &h.eval_x("(map (lambda (m) (list m.device m.index m.slot m.label)) flt.modulators)"),
    );
    assert_eq!(read.len(), desc.instrument_modulators.len());
    for (at, (row, modulator)) in read.iter().zip(&desc.instrument_modulators).enumerate() {
        let row = items(row);
        assert_eq!(row[0], h.eval_x("flt"));
        assert_eq!(row[1], Value::Number(at as f64));
        assert_eq!(row[2], Value::Number(modulator.slot as f64));
        assert_eq!(row[3], s(&modulator.label));
    }
    // A relabelled modulator is a descriptor change: fresh instances.
    h.eval_x("(def m0 (first flt.modulators))");
    h.app.graph.effect_descriptors[0][slot].instrument_modulators[0].label = "LFO".to_string();
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_x("m0.label"), s(""), "the old handle is stale");
    assert_eq!(
        h.eval_x("(let ((m (first flt.modulators))) m.label)"),
        s("LFO")
    );
    let legacy = legacy_instrument(&h, 2);
    let modulators = items(&get(&legacy, "modulators"));
    assert_eq!(
        h.eval_x("(len inst.modulators)"),
        Value::Number(modulators.len() as f64)
    );
    for (at, modulator) in modulators.iter().enumerate() {
        let code = format!("(let ((m (nth inst.modulators {at}))) (list m.slot m.label))");
        let row = items(&h.eval_x(&code));
        assert_eq!(row[0], get(modulator, "slot"));
        assert_eq!(row[1], get(modulator, "label"));
    }
    // A drum rack slot: its own meter and display name.
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_x("(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))");
    let legacy = legacy_instrument(&h, 2);
    let selected = get(&legacy, "selected-instrument");
    assert_eq!(h.eval_x("rs.meter"), get(&selected, "meter"));
    assert_eq!(h.eval_x("rs.display-name"), get(&selected, "display-name"));
    assert_eq!(h.eval_x("rk.display-name"), get(&legacy, "display-name"));
}

#[test]
fn a_modulators_envelope_reads_the_meter_cache_while_observed() {
    let (mut h, _) = Harness::with_devices();
    h.eval_x(DEVICES_X);
    h.meters.cached_modulator_phases = vec![0.0, 0.0, 0.25];
    h.meters.cached_modulator_levels = vec![0.0, 0.0, 0.5];
    h.sync();
    // Cold reads.
    assert_eq!(
        h.eval_x("(list inst.modulator-phase inst.modulator-level flt.modulator-phase)"),
        h.eval_x("(list 0.25 0.5 0)")
    );
    assert!(!h.frame.host_kinds.wants_modulator_meters());
    // Legacy parity (the modulator phase field).
    let rt = h.editor.runtime_mut();
    sync_modulator_phase_fields(rt, &[0.0, 0.0, 0.25]);
    let legacy = rt
        .reactive_field_value("SEQ", &modulator_phase_field(2))
        .cloned();
    assert_eq!(legacy, Some(Value::Number(0.25)));
    // Observed: computed per tick; only a modulator track's instrument
    // keeps the cache polled.
    h.eval_x("(def phase #'inst.modulator-phase)");
    h.sync();
    assert!(
        !h.frame.host_kinds.wants_modulator_meters(),
        "a sampler has no envelope to poll"
    );
    h.meters.cached_modulator_phases = vec![0.0, 0.0, 0.75];
    h.sync();
    assert_eq!(h.slot("phase"), 0.75);
    h.app
        .graph_controller()
        .add_modulator_track()
        .expect("modulator track");
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_x(
        "(def t3 (track 3)) (def lfo (first t3.devices)) (def lfo-phase #'lfo.modulator-phase)",
    );
    h.meters.cached_modulator_phases = vec![0.0, 0.0, 0.75, 0.5];
    h.sync();
    assert!(h.frame.host_kinds.wants_modulator_meters());
    assert_eq!(h.slot("lfo-phase"), 0.5);
    let computed = h.computed(f::DEVICE_MODULATOR_LEVEL);
    h.sync();
    assert_eq!(
        h.computed(f::DEVICE_MODULATOR_LEVEL),
        computed,
        "level unobserved"
    );
}

// ── param UI metadata ──────────────────────────────────────────────────

#[test]
fn param_ui_metadata_reads_like_the_legacy_param_maps() {
    use sequencer::effects::{ParamAssetOptions, ParamUiMetadata};
    let (mut h, slot) = Harness::with_devices();
    h.eval_x(DEVICES_X);
    h.eval_x(r#"(def old (device-param flt "cutoff"))"#);
    assert_eq!(
        h.eval_x("(list old.group old.env old.role old.display-name old.asset-options)"),
        h.eval_x(r#"(list "" "" "" "" nil)"#)
    );
    let cutoff = 2;
    let metadata = ParamUiMetadata {
        group: Some("tone".to_string()),
        env: Some("amp".to_string()),
        role: Some("attack".to_string()),
        tags: Vec::new(),
        asset_options: Some(ParamAssetOptions {
            tensor: "modes".to_string(),
            file: "modes.lisp".to_string(),
            key: "names".to_string(),
            asset_base: None,
        }),
        display_name: Some("Cut".to_string()),
    };
    h.app.graph.effect_descriptors[0][slot].params[cutoff].ui_metadata = Some(metadata.clone());
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    // A metadata change is a descriptor change: fresh params.
    assert_eq!(h.eval_x("old.name"), s(""), "the old handle is stale");
    h.eval_x(r#"(def p (device-param flt "cutoff"))"#);
    assert_eq!(
        h.eval_x("(list p.group p.env p.role p.display-name)"),
        h.eval_x(r#"(list "tone" "amp" "attack" "Cut")"#)
    );
    let options = metadata.asset_options.as_ref().unwrap();
    assert_eq!(
        h.eval_x("p.asset-options"),
        param_asset_options_value(options)
    );
    // Legacy parity (the effects panel's param map).
    let effects = legacy_effects(&h, 0);
    let param = items(&get(&effects[slot], "params"))
        .into_iter()
        .find(|param| get(param, "idx") == Value::Number(cutoff as f64))
        .expect("the cutoff map");
    for (field, key) in [
        ("group", "group"),
        ("env", "env"),
        ("role", "role"),
        ("display-name", "display-name"),
        ("asset-options", "options"),
    ] {
        assert_eq!(h.eval_x(&format!("p.{field}")), get(&param, key), "{field}");
    }
}

// ── effect tables ──────────────────────────────────────────────────────

#[test]
fn effect_tables_read_the_node_registries_while_observed() {
    use sequencer::effects::filter_table;
    let (mut h, slot) = Harness::with_devices();
    let reverb = h.add_effect(0, "Reverb");
    // Stand-ins for a Filter Table and a Convolution Reverb: what the
    // fields read is the descriptor's name and the node's registries.
    h.app.graph.effect_descriptors[0][slot].name = filter_table::NAME.to_string();
    h.app.graph.effect_descriptors[0][reverb].name =
        sequencer::effects::conv_reverb::NAME.to_string();
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_x("(def t0 (track 0)) (def tbl (first t0.devices)) (def ir (nth t0.devices 1))");
    let node = h.filter_slot(slot).node_id.load(Ordering::Relaxed) as i32;
    assert!(node > 0, "the effect has a node");
    let reference =
        filter_table::encode_table_ref("tables/pad.wav", filter_table::recommend_mode(64));
    let table = Arc::new(filter_table::default_table());
    filter_table::record_prepared_table(node, &reference, "Pad", table);
    let fields = "(list tbl.table-name tbl.table-mode tbl.table-engine tbl.table-data-key \
                  tbl.table-options tbl.ir-name)";
    let read = items(&h.eval_x(fields));
    let effects = legacy_effects(&h, 0);
    let legacy = &effects[slot];
    assert_eq!(read[0], get(legacy, "table-name"));
    assert_eq!(read[0], s("Pad"));
    assert_eq!(read[1], get(legacy, "table-mode"));
    assert_eq!(read[2], get(legacy, "table-engine"));
    assert_eq!(read[3], get(legacy, "table-data-key"));
    assert_eq!(read[4], get(legacy, "table-options"));
    assert_eq!(read[5], s(""), "a Filter Table has no IR");
    assert_eq!(h.eval_x("ir.ir-name"), get(&effects[reverb], "ir-name"));
    assert_eq!(h.eval_x("ir.ir-name"), s("No IR"));
    assert_eq!(
        h.eval_x("(list ir.table-name ir.table-options)"),
        h.eval_x(r#"(list "" (list))"#)
    );
    // Observed: per tick, the asset listing once per epoch.
    h.eval_x(r#"(effect-buffer "*tables*" (label (str tbl.table-name tbl.table-options)))"#);
    h.show_all();
    h.sync();
    let listings = h.frame.host_kinds.shared.borrow().table_listings;
    let pushes = h.frame.host_kinds.shared.borrow().table_option_pushes;
    let computed = h.computed(f::DEVICE_TABLE_NAME);
    h.sync();
    h.sync();
    assert_eq!(h.computed(f::DEVICE_TABLE_NAME), computed + 2);
    assert_eq!(h.frame.host_kinds.shared.borrow().table_listings, listings);
    assert_eq!(
        h.frame.host_kinds.shared.borrow().table_option_pushes,
        pushes,
        "idle ticks push no table-options"
    );
    assert_eq!(h.eval_x("tbl.table-options"), get(legacy, "table-options"));
    // A moved epoch lists them again and pushes them once.
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.sync();
    let shared = h.frame.host_kinds.shared.borrow();
    assert_eq!(shared.table_listings, listings + 1);
    assert_eq!(shared.table_option_pushes, pushes + 1);
    drop(shared);
    filter_table::record_prepared_table(
        node,
        &reference,
        "Pad 2",
        Arc::new(filter_table::default_table()),
    );
    h.sync();
    assert_eq!(h.eval_x("tbl.table-name"), s("Pad 2"));
}

// ── sampler media ──────────────────────────────────────────────────────

/// A mono 16-bit WAV of `frames` frames at 44.1 kHz in the temp folder.
fn wav_fixture(name: &str, frames: usize) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{name}-{}-{unique}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("create the fixture");
    for frame in 0..frames {
        writer
            .write_sample(((frame % 100) as i16 - 50) * 100)
            .expect("write a frame");
    }
    writer.finalize().expect("finalize the fixture");
    path
}

/// The media pass's (sample, analysis, slice) computations.
fn media_reads(h: &Harness) -> [u64; 3] {
    let media = &h.frame.host_kinds.media;
    [media.sample_reads, media.analysis_reads, media.slice_reads]
}

#[test]
fn sampler_media_read_like_the_sampler_panel_while_observed() {
    use sequencer::instruments::sampler::{SLOT_PARAM_SLICE_MODE, SLOT_PARAM_SLICE_SENSITIVITY};
    let (mut h, _) = Harness::with_devices();
    h.eval_x(DEVICES_X);
    // Unobserved, a device reads the no-media defaults from its
    // registration.
    assert_eq!(
        h.eval_x("(list inst.sample-buffer inst.sample-duration inst.slices inst.analysis-status inst.downbeat-time flt.onsets)"),
        h.eval_x(r#"(list nil 1 (list) "none" -1 (list))"#)
    );
    let fields = "(list inst.sample-buffer inst.sample-duration inst.start-time inst.end-time \
                  inst.slices inst.slice-active inst.onsets inst.analysis-status \
                  inst.analysis-message inst.analysis-bpm inst.analysis-confidence \
                  inst.downbeat-time)";
    h.eval_x(&format!(
        r#"(effect-buffer "*media*" (label (str {fields})))"#
    ));
    h.show_all();
    h.sync();
    // No sample yet: empty media, the selection over a 1 s default.
    let read = items(&h.eval_x(fields));
    assert_eq!(read[0], Value::Nil);
    assert_eq!(read[1], Value::Number(1.0));
    assert_eq!(read[7], s("none"));
    assert_eq!(read[11], Value::Number(-1.0));
    // A loaded, analysed sample in slice mode.
    let path = wav_fixture("kinds-media", 44_100);
    let buffer = h.app.graph.track_buffer_ids[2];
    h.app
        .register_loaded_sample_path("kinds-media", buffer, path.clone());
    let result = sequencer::analysis::AnalysisResult {
        buffer_id: buffer,
        bpm: 120.0,
        bpm_confidence: 0.75,
        onsets_frames: vec![0, 11_025, 22_050, 33_075],
        downbeat_frame: Some(11_025),
    };
    h.app
        .sample_analysis
        .cache()
        .insert_ready(result, 44_100, 44_100);
    let state = h.shared.state.clone();
    let instrument = &state.pattern.instrument_slots[2];
    instrument.defaults.set(SLOT_PARAM_SLICE_MODE, 1.0);
    instrument.defaults.set(2, 0.25);
    let reads = media_reads(&h);
    h.sync();
    assert_eq!(media_reads(&h), [reads[0] + 1, reads[1] + 1, reads[2] + 1]);
    let read = items(&h.eval_x(fields));
    let legacy = items(&build_sampler_panel_value(
        &h.app,
        2,
        &h.shared.selected_steps,
    ))
    .remove(0);
    for (at, key) in [
        (0, "buffer"),
        (1, "duration"),
        (2, "start-time"),
        (3, "end-time"),
        (4, "slices"),
        (5, "slice-active"),
        (6, "onsets"),
        (7, "analysis-status"),
        (8, "analysis-message"),
        (9, "analysis-bpm"),
        (10, "analysis-confidence"),
        (11, "downbeat-time"),
    ] {
        assert_eq!(read[at], get(&legacy, key), "{key}");
    }
    assert!(!items(&read[4]).is_empty(), "slices in slice mode");
    assert!(
        (num(read[1].clone()) - 1.0).abs() < 1e-6,
        "a one second sample"
    );
    // The slice sensitivity moves the slices only: no sample load, no
    // analysis read.
    let reads = media_reads(&h);
    instrument.defaults.set(SLOT_PARAM_SLICE_SENSITIVITY, 0.9);
    h.sync();
    assert_eq!(media_reads(&h), [reads[0], reads[1], reads[2] + 1]);
    // The sample rate moves the analysis' seconds (and the slices).
    assert_eq!(items(&h.eval_x("inst.onsets"))[1], Value::Number(0.25));
    h.app.graph.sample_rate = 22_050;
    h.sync();
    assert_eq!(media_reads(&h), [reads[0], reads[1] + 1, reads[2] + 2]);
    assert_eq!(items(&h.eval_x("inst.onsets"))[1], Value::Number(0.5));
    assert_eq!(h.eval_x("inst.downbeat-time"), Value::Number(0.5));
    h.app.graph.sample_rate = 44_100;
    h.sync();
    assert_eq!(items(&h.eval_x("inst.onsets"))[1], Value::Number(0.25));
    // A start drag moves no media key: the selection only.
    let reads = media_reads(&h);
    instrument.defaults.set(2, 0.5);
    h.sync();
    h.sync();
    assert_eq!(media_reads(&h), reads);
    assert_eq!(h.eval_x("inst.start-time"), Value::Number(0.5));
    // The analysis failing moves it (and the slices it derives), not the
    // sample.
    h.app
        .sample_analysis
        .cache()
        .insert_failed(buffer, "no onsets");
    h.sync();
    assert_eq!(media_reads(&h), [reads[0], reads[1] + 1, reads[2] + 1]);
    assert_eq!(
        h.eval_x("(list inst.analysis-status inst.analysis-message inst.analysis-bpm inst.onsets)"),
        h.eval_x(r#"(list "failed" "no onsets" 0 (list))"#)
    );
    // An effect reads empty media.
    h.eval_x(r#"(effect-buffer "*flt-media*" (label (str flt.sample-buffer flt.slices flt.analysis-status flt.sample-duration)))"#);
    h.show_all();
    h.sync();
    assert_eq!(
        h.eval_x("(list flt.sample-buffer flt.slices flt.analysis-status flt.sample-duration)"),
        h.eval_x(r#"(list nil (list) "none" 1)"#)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_rack_slot_samplers_selection_reads_like_the_rack_panel() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    assert!(h
        .app
        .state
        .update_rack_slot_in_all_pattern_snapshots(2, 0, |slot| {
            slot.instrument_slot.defaults[2] = 0.25;
            slot.instrument_slot.defaults[3] = 0.75;
        }));
    h.eval_x("(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))");
    h.eval_x(
        r#"(effect-buffer "*rack-media*"
             (label (str rs.start-time rs.end-time rs.sample-duration rs.slices)))"#,
    );
    h.show_all();
    h.sync();
    let legacy = legacy_instrument(&h, 2);
    let selected = get(&legacy, "selected-instrument");
    for (field, key) in [
        ("start-time", "start-time"),
        ("end-time", "end-time"),
        ("sample-duration", "duration"),
        ("slices", "slices"),
        ("slice-active", "slice-active"),
    ] {
        assert_eq!(
            h.eval_x(&format!("rs.{field}")),
            get(&selected, key),
            "{field}"
        );
    }
    assert_eq!(h.eval_x("rs.start-time"), Value::Number(0.25));
    // The rack edit moves the selection.
    assert!(h
        .app
        .state
        .update_rack_slot_in_all_pattern_snapshots(2, 0, |slot| {
            slot.instrument_slot.defaults[2] = 0.5;
        }));
    h.sync();
    assert_eq!(h.eval_x("rs.start-time"), Value::Number(0.5));
}
