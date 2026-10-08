//! Stage 7b-3: device panel extras (param placement and modulation lanes,
//! modulation display, process mapping, key locks, base note, tensors,
//! p-lock variants, project and rack macros, the neural selection's
//! override), their setters, and the cold `device.playhead` read.

use super::*;

/// The sampler's `start` (a percent param: stored 0–1, shown 0–100) and
/// the Filter's `cutoff` (Hz).
const START: usize = 2;
const CUTOFF: usize = 2;

const REFER_PANEL: &str = "(import eseq.kinds :refer (track tracks macros device-param \
                           lock-param! unlock-param! set-tensor-cell! stamp-variant! stamp-key-variant! \
                           lock-rack-macro! unlock-rack-macro! selection))";

impl Harness {
    fn eval_panel(&mut self, code: &str) -> Value {
        let source = format!("{REFER_PANEL}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    fn panel_scans(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().panel_scans
    }

    fn sampler_refreshes(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().sampler_refreshes
    }
}

fn index_of(value: &Value) -> Option<usize> {
    match get(value, "idx") {
        Value::Number(idx) => Some(idx as usize),
        _ => None,
    }
}

fn close(value: Value, expected: f64) -> bool {
    (num(value) - expected).abs() < 1e-3
}

const SAMPLER: &str = "(def t0 (track 0)) (def flt (first t0.devices)) \
                       (def cutoff (device-param flt \"cutoff\")) \
                       (def t2 (track 2)) (def inst (first t2.devices)) \
                       (def start (device-param inst \"start\"))";

#[test]
fn param_placement_and_lanes_follow_the_sampler_descriptor() {
    let (mut h, _) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    // Literal expectations for the builtin sampler (its descriptor's layout
    // as the panel shows it), so a placement rule change shows up here.
    let read = |h: &mut Harness, idx: usize| {
        let code = format!(
            "(let ((p (nth inst.params {idx}))) (list p.section p.label p.mod-slot p.visible))"
        );
        items(&h.eval_panel(&code))
    };
    let row = |section: &str, label: &str, slot: f64, visible: bool| {
        vec![s(section), s(label), Value::Number(slot), Value::Bool(visible)]
    };
    assert_eq!(h.eval_panel("(len inst.params)"), Value::Number(145.0));
    let main = [
        "attack", "release", "start", "end", "enabled", "reverse", "loop", "xfade", "sr",
        "warp", "mode", "bpm", "speed", "scrub",
    ];
    for (idx, name) in main.iter().enumerate() {
        assert_eq!(read(&mut h, idx), row("main", name, 0.0, true), "param {idx}");
    }
    // The tail: warp settings and the host-only slice controls (the slice
    // mode, its sensitivity and base note, eseq-0l17.82's re-added check).
    let tail = ["smooth", "preserve", "fill", "decay", "slice", "sens", "slice base"];
    for (at, name) in tail.iter().enumerate() {
        assert_eq!(read(&mut h, 134 + at), row("main", name, 0.0, true), "param {}", 134 + at);
    }
    // Each source's type param (labelled `type`) and the settings its type
    // uses at the defaults: Mod 1 an LFO (its division only while synced),
    // Mod 2 an envelope, Mod 3 random, Mod 4 drift.
    let settings = [
        "rate", "sync", "division", "shape", "pulse width", "retrigger", "attack", "decay",
        "sustain", "release", "rate", "sync", "division", "slew", "rate", "sync", "division",
    ];
    let shown: [&[usize]; 4] = [&[0, 1, 3, 4, 5], &[6, 7, 8, 9], &[10, 11, 13], &[14, 15]];
    for slot in 0..4 {
        let base = 14 + slot * 18;
        let source = (slot + 1) as f64;
        assert_eq!(read(&mut h, base), row("source", "type", source, true), "Mod {source}'s type");
        for (at, label) in settings.iter().enumerate() {
            let visible = shown[slot].contains(&at);
            assert_eq!(
                read(&mut h, base + 1 + at),
                row("source", label, source, visible),
                "Mod {source}'s setting {at}"
            );
        }
    }
    // Each LFO's phase, after the tail: shown for Mod 1's LFO only.
    for slot in 0..4 {
        let visible = slot == 0;
        assert_eq!(read(&mut h, 141 + slot), row("source", "phase", (slot + 1) as f64, visible));
    }
    // The lane params, under the mods editor.
    assert_eq!(read(&mut h, 86), row("mod", "speed src", 0.0, true));
    assert_eq!(read(&mut h, 133), row("mod", "end lane 4 amt", 0.0, true));
    // The lanes onto a param: (source, depth, depth-min, depth-max, unit,
    // slot), four each.
    let lanes = |h: &mut Harness, idx: usize| {
        let code = format!(
            "(let ((p (nth inst.params {idx}))) \
             (map (lambda (mt) (list (if mt.source mt.source.index -1) mt.depth.index \
             mt.depth-min mt.depth-max mt.unit mt.param.index mt.slot)) p.mod-targets))"
        );
        items(&h.eval_panel(&code))
    };
    for (idx, first, range, unit) in [
        (2, 118, 100.0, "%"),
        (3, 126, 100.0, "%"),
        (8, 102, 42100.0, "Hz"),
        (11, 110, 380.0, "bpm"),
        (12, 86, 8.0, ""),
        (13, 94, 100.0, "%"),
    ] {
        let read = lanes(&mut h, idx);
        assert_eq!(read.len(), 4, "param {idx}'s lanes");
        for (lane, kind) in read.iter().enumerate() {
            let source = (first + 2 * lane) as f64;
            assert_eq!(
                items(kind),
                vec![
                    Value::Number(source),
                    Value::Number(source + 1.0),
                    Value::Number(-range),
                    Value::Number(range),
                    s(unit),
                    Value::Number(idx as f64),
                    Value::Number(0.0),
                ],
                "param {idx}'s lane {lane}"
            );
        }
    }
    for idx in [0, 1, 4, 5, 6, 7, 9, 10, 14, 86] {
        assert!(lanes(&mut h, idx).is_empty(), "param {idx} has no lanes");
    }}

#[test]
fn param_visible_follows_the_source_type_and_is_computed_only_while_observed() {
    let (mut h, _) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    let params = h.app.graph.instrument_descriptors[2].params.clone();
    let named = |name: &str| params.iter().position(|p| p.name == name);
    let (Some(source), Some(rate), Some(attack)) = (
        named("mod1_source"),
        named("mod1_lfo_rate"),
        named("mod1_env_attack"),
    ) else {
        panic!("the sampler's first modulation source");
    };
    h.eval_panel(&format!(
        "(def rp (nth inst.params {rate})) (def ap (nth inst.params {attack})) \
         (def rate-shown #'rp.visible) (def attack-shown #'ap.visible)"
    ));
    assert_eq!(h.computed(f::PARAM_VISIBLE), 2, "the bindings' seeds");
    let sequencer::effects::ParamKind::Enum { labels } = params[source].kind.clone() else {
        panic!("the source type is an enum");
    };
    let type_of = |name: &str| labels.iter().position(|label| label == name).unwrap() as f32;
    let state = h.shared.state.clone();
    let slot = &state.pattern.instrument_slots[2];
    slot.defaults.set(source, type_of("lfo"));
    h.sync();
    assert_eq!((h.slot("rate-shown"), h.slot("attack-shown")), (1.0, 0.0));
    slot.defaults.set(source, type_of("env"));
    h.sync();
    assert_eq!((h.slot("rate-shown"), h.slot("attack-shown")), (0.0, 1.0));
    // A p-lock at the shown step picks the type there.
    slot.plocks.set(3, source, type_of("lfo"));
    h.shared.selected_steps.lock().unwrap().insert(3);
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.sync();
    assert_eq!((h.slot("rate-shown"), h.slot("attack-shown")), (1.0, 0.0));
    // Released: nothing computes it.
    h.eval_panel("(set! rate-shown nil) (set! attack-shown nil)");
    h.sync();
    h.sync();
    let after = h.computed(f::PARAM_VISIBLE);
    h.sync();
    h.sync();
    assert_eq!(h.computed(f::PARAM_VISIBLE), after, "unobserved: no work");
}

#[test]
fn modulation_display_reads_the_tick_sample_in_display_units() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    // Nothing observes a modulation field yet: the tick need not sample.
    h.sync();
    assert!(!h.frame.host_kinds.wants_mod_display());
    assert_eq!(h.computed(f::PARAM_MOD_OFFSET), 0);
    h.eval_panel(
        "(def start-offset #'start.mod-offset) (def start-mod #'start.mod-value) \
         (def cutoff-offset #'cutoff.mod-offset) (def cutoff-mod #'cutoff.mod-value) \
         (def cutoff-scale #'cutoff.mod-scale)",
    );
    h.sync();
    assert!(
        h.frame.host_kinds.wants_mod_display(),
        "an observed field keeps it polled"
    );
    // Unsampled: no offset, the shown value, scale 1.
    let base = f64::from(h.filter_slot(slot).defaults.get(CUTOFF));
    assert_eq!(h.slot("cutoff-offset"), 0.0);
    assert!((h.slot("cutoff-mod") - base).abs() < 1e-3);
    assert_eq!(h.slot("cutoff-scale"), 1.0);
    // A sample: the instrument's in display units, the effect's (by its
    // node) in stored units, as the legacy fields carry them.
    let node = h.filter_slot(slot).node_id.load(Ordering::Relaxed) as i32;
    assert!(node > 0, "the Filter has a graph node");
    let sample = ModDisplayValues {
        effects: vec![EffectModValues {
            node_id: node,
            values: vec![ParamModValue {
                param_idx: CUTOFF,
                offset: 250.0,
                value: base + 250.0,
                scale: 1.25,
            }],
            slot_phases: [0.25, -1.0, -1.0, -1.0],
        }],
        instrument: Some(InstrumentModValues {
            track: 2,
            values: vec![ParamModValue {
                param_idx: START,
                offset: 10.0,
                value: 35.0,
                scale: 1.0,
            }],
            slot_phases: [0.5, 0.75, -1.0, -1.0],
        }),
        rack_slot: None,
    };
    h.meters.cached_mod_display_values = sample.clone();
    h.sync();
    assert_eq!(h.slot("start-offset"), 10.0);
    assert_eq!(h.slot("start-mod"), 35.0);
    assert_eq!(h.slot("cutoff-offset"), 250.0);
    assert!((h.slot("cutoff-mod") - (base + 250.0)).abs() < 1e-3);
    assert_eq!(h.slot("cutoff-scale"), 1.25);
    assert_eq!(
        h.eval_panel("inst.mod-phases"),
        h.eval_panel("(list 0.5 0.75 -1 -1)")
    );
    // An observed phase list is pushed per tick while it moves.
    h.eval_panel(r#"(effect-buffer "*phases*" (label (str inst.mod-phases)))"#);
    h.show_all();
    h.sync();
    h.meters
        .cached_mod_display_values
        .instrument
        .as_mut()
        .unwrap()
        .slot_phases[0] = 0.625;
    h.sync();
    assert_eq!(
        h.rt()
            .instance_field(h.instance_of(DEVICE, &[h.track_id(2), 0]), "mod-phases")
            .unwrap(),
        h.eval_panel("(list 0.625 0.75 -1 -1)")
    );
    h.eval_panel(r#"(effect-buffer "*phases*" (label "gone"))"#);
    h.show_all();
    assert_eq!(
        h.eval_panel("flt.mod-phases"),
        h.eval_panel("(list 0.25 -1 -1 -1)")
    );
    // Released: the tick may stop sampling.
    h.eval_panel(
        "(set! start-offset nil) (set! start-mod nil) (set! cutoff-offset nil) \
         (set! cutoff-mod nil) (set! cutoff-scale nil)",
    );
    h.sync();
    h.sync();
    assert!(!h.frame.host_kinds.wants_mod_display());
}

#[test]
fn process_mapping_follows_the_track_process_chain() {
    let (mut h, _) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    h.eval_panel(
        "(def mapped #'start.process-mapped) (def written #'start.process-value) \
         (def clamped #'start.process-clamped)",
    );
    h.sync();
    let stored = h.shared.state.pattern.instrument_slots[2]
        .defaults
        .get(START);
    let base = f64::from(stored) * 100.0;
    assert_eq!(h.slot("mapped"), 0.0);
    assert!(
        (h.slot("written") - base).abs() < 1e-3,
        "unmapped: the shown value"
    );
    let chain = |enabled: bool| {
        let start = sequencer::process::ParamTarget::InstrumentParam {
            param: "start".to_string(),
            param_id: None,
        };
        one_slot_chain(start, enabled)
    };
    // A process chain edit moves the track's p-lock key, as the legacy
    // commands' invalidation does.
    let edit = |h: &mut Harness, enabled: bool| {
        assert!(h.shared.state.set_track_process_chain(2, chain(enabled)));
        let invalidation = UiInvalidation::ProcessChain { track: 2 };
        h.shared.ui_invalidations.push(invalidation);
        h.sync();
    };
    edit(&mut h, true);
    assert_eq!(h.slot("mapped"), 1.0);
    // The scheduler's write, in display units (a percent param ×100).
    h.shared.state.publish_process_effective_params(
        2,
        &[sequencer::process::ProcessEffectiveParam {
            param_idx: START,
            base: 0.0,
            value: 0.6,
            clamped: true,
        }],
    );
    h.sync();
    assert!((h.slot("written") - 60.0).abs() < 1e-3);
    assert_eq!(h.slot("clamped"), 1.0);
    // The bound set is read once per p-lock key, not per tick.
    let scans = h.panel_scans();
    h.sync();
    h.sync();
    assert_eq!(h.panel_scans(), scans);
    edit(&mut h, false);
    assert_eq!(h.slot("mapped"), 0.0, "a disabled slot writes nothing");
    assert!((h.slot("written") - base).abs() < 1e-3);
    assert_eq!(h.slot("clamped"), 0.0);
}

#[test]
fn key_locks_read_after_sync_and_scan_once_per_plock_key() {
    let (mut h, _) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    assert_eq!(
        h.eval_panel("inst.key-locked-notes"),
        h.eval_panel("(list)")
    );
    h.eval_panel(
        r#"(effect-buffer "*observe*" (label (str inst.key-locked-notes start.key-locks)))"#,
    );
    h.show_all();
    h.sync();
    let computed = h.computed(f::DEVICE_KEY_LOCKED_NOTES);
    h.sync();
    h.sync();
    assert_eq!(
        h.computed(f::DEVICE_KEY_LOCKED_NOTES),
        computed,
        "observed, recomputed only when the p-lock key moved"
    );
    for (note, value) in [(60, 0.5), (64, 0.25)] {
        let command = app::AppCommand::SetInstrumentKeyLock {
            track: 2,
            note,
            param_idx: START,
            value,
        };
        app::apply_command(&mut h.app, command);
    }
    // The key-lock commands move the fx and UI epochs.
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(
        h.eval_panel("inst.key-locked-notes"),
        h.eval_panel("(list 60 64)")
    );
    assert_eq!(
        h.eval_panel("start.key-locks"),
        h.eval_panel("(list (list 60 50) (list 64 25))")
    );
    assert_eq!(h.eval_panel("cutoff.key-locks"), h.eval_panel("(list)"));
    assert_eq!(h.eval_panel("flt.key-locked-notes"), h.eval_panel("(list)"));
    // The panel's derivation (shared) agrees.
    let slot = &h.shared.state.pattern.instrument_slots[2];
    let params = &h.app.graph.instrument_descriptors[2].params;
    let locks = instrument_key_locks(slot, params);
    assert_eq!(locks.notes, vec![60, 64]);
    assert_eq!(locks.by_param[START], vec![(60, 50.0), (64, 25.0)]);
    // Read once per p-lock key.
    let scans = h.panel_scans();
    h.sync();
    h.sync();
    assert_eq!(h.panel_scans(), scans);
}

#[test]
fn base_note_sets_through_history_and_rejects_bad_values() {
    let (mut h, _) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    h.eval_panel("(def note #'inst.base-note)");
    h.sync();
    assert_eq!(h.slot("note"), 0.0);
    assert_eq!(h.eval_panel("flt.base-note"), Value::Number(0.0));
    let offset = |h: &Harness| {
        let offsets = &h.shared.state.pattern.instrument_base_note_offsets;
        f32::from_bits(offsets[2].load(Ordering::Relaxed))
    };
    let before = h.app.history.undo_len();
    h.eval_panel("(set! inst.base-note 12)");
    h.drain_and_sync();
    assert_eq!(offset(&h), 12.0);
    assert_eq!(h.slot("note"), 12.0);
    assert_eq!(h.app.history.undo_len(), before + 1);
    // Absolute: the same value is no new entry.
    h.eval_panel("(set! inst.base-note 12)");
    h.drain_and_sync();
    assert_eq!(h.app.history.undo_len(), before + 1);
    h.undo();
    h.sync();
    assert_eq!(offset(&h), 0.0);
    assert_eq!(h.slot("note"), 0.0);
    h.rejects_in(
        REFER_PANEL,
        "(set! inst.base-note 60)",
        "from -48 to 48",
        false,
    );
    h.rejects_in(
        REFER_PANEL,
        "(set! flt.base-note 3)",
        "has no base note",
        false,
    );
    assert_eq!(offset(&h), 0.0);
}

impl Harness {
    /// Track 2's sampler gets a 2×2 tensor (cells 0.1–0.4, range 0–1).
    fn with_tensor(&mut self) {
        let mut desc = self.app.graph.instrument_descriptors[2].clone();
        desc.tensor_params
            .push(sequencer::effects::TensorParamDescriptor {
                name: "mask".to_string(),
                shape: vec![2, 2],
                cell_offset: 4096,
                default: vec![0.1, 0.2, 0.3, 0.4],
                min: 0.0,
                max: 1.0,
            });
        let slot = &self.shared.state.pattern.instrument_slots[2];
        slot.tensor_params.apply_descriptor(&desc.tensor_params);
        self.app.graph.instrument_descriptors[2] = desc;
        self.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
        self.sync();
    }

    fn tensor_base(&self) -> Vec<f32> {
        let slot = &self.shared.state.pattern.instrument_slots[2];
        slot.tensor_params.default_values(0).unwrap()
    }

    fn cells(&mut self, code: &str) -> Vec<f64> {
        items(&self.eval_panel(code)).into_iter().map(num).collect()
    }
}

fn same_cells(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6)
}

#[test]
fn tensors_register_with_their_device_and_set_through_history() {
    let (mut h, _) = Harness::with_devices();
    h.with_tensor();
    h.eval_panel(SAMPLER);
    h.eval_panel("(def mask (first inst.tensors))");
    assert_eq!(h.eval_panel("(len inst.tensors)"), Value::Number(1.0));
    assert_eq!(h.eval_panel("(len flt.tensors)"), Value::Number(0.0));
    assert_eq!(
        h.eval_panel(
            "(list mask.device mask.index mask.name mask.rows mask.cols mask.min mask.max)"
        ),
        h.eval_panel(r#"(list inst 0 "mask" 2 2 0 1)"#)
    );
    let values = h.cells("mask.values");
    assert!(same_cells(&values, &[0.1, 0.2, 0.3, 0.4]), "{values:?}");
    assert_eq!(h.eval_panel("mask.locked"), Value::Bool(false));
    h.eval_panel("(def shown #'mask.locked)");
    // Set a cell: one undo entry, never a p-lock.
    let before = h.app.history.undo_len();
    h.eval_panel("(set-tensor-cell! mask 2 0.9)");
    h.drain_and_sync();
    assert!((h.tensor_base()[2] - 0.9).abs() < 1e-6);
    assert_eq!(h.app.history.undo_len(), before + 1);
    let base = h.cells("mask.base");
    assert!(same_cells(&base, &[0.1, 0.2, 0.9, 0.4]), "{base:?}");
    // A p-lock at the shown step shows over the base.
    let command = app::AppCommand::SetInstrumentTensorPlockCellMulti {
        track: 2,
        steps: vec![5],
        tensor_idx: 0,
        cell_idx: 0,
        value: 0.7,
    };
    app::apply_command(&mut h.app, command);
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.shared.selected_steps.lock().unwrap().insert(5);
    h.sync();
    let values = h.cells("mask.values");
    assert!(same_cells(&values, &[0.7, 0.2, 0.9, 0.4]), "{values:?}");
    assert_eq!(h.slot("shown"), 1.0);
    h.undo();
    h.undo();
    h.shared.selected_steps.lock().unwrap().clear();
    h.sync();
    assert!(
        (h.tensor_base()[2] - 0.3).abs() < 1e-6,
        "undo restores the cell"
    );
    assert_eq!(h.slot("shown"), 0.0);
    // The value rule.
    h.rejects_in(
        REFER_PANEL,
        "(set-tensor-cell! mask 4 0.5)",
        "an integer from 0 to 3",
        false,
    );
    h.rejects_in(
        REFER_PANEL,
        "(set-tensor-cell! mask 0 1.5)",
        "a number from 0 to 1",
        false,
    );
    h.rejects_in(
        REFER_PANEL,
        "(set-tensor-cell! (dict :device flt :index 0) 0 0.5)",
        "no such tensor",
        false,
    );
    // A descriptor change replaces the tensors: old handles go stale.
    h.eval_panel("(def old-mask mask)");
    let mut desc = h.app.graph.instrument_descriptors[2].clone();
    desc.tensor_params[0].shape = vec![1, 4];
    h.app.graph.instrument_descriptors[2] = desc;
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_ne!(
        h.eval_panel("(first inst.tensors)"),
        h.eval_panel("old-mask")
    );
    assert_eq!(
        h.eval_panel("(let ((mask (first inst.tensors))) mask.rows)"),
        Value::Number(1.0)
    );
    // A tensor with no cells takes no cell.
    let mut desc = h.app.graph.instrument_descriptors[2].clone();
    desc.tensor_params
        .push(sequencer::effects::TensorParamDescriptor {
            name: "empty".to_string(),
            shape: vec![0],
            cell_offset: 8192,
            default: Vec::new(),
            min: 0.0,
            max: 1.0,
        });
    h.app.graph.instrument_descriptors[2] = desc;
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_panel("(len inst.tensors)"), Value::Number(2.0));
    let before = h.app.history.undo_len();
    h.rejects_in(
        REFER_PANEL,
        "(set-tensor-cell! (nth inst.tensors 1) 0 0.5)",
        "the tensor has no cells",
        false,
    );
    assert_eq!(h.app.history.undo_len(), before);
}

#[test]
fn step_variants_list_the_chips_and_stamp_through_history() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    // Steps 2 and 4 share one lock set (A), step 6 another (B).
    for (step, value) in [(2, 500.0), (4, 500.0), (6, 800.0)] {
        h.lock_effect(slot, step, CUTOFF, value);
    }
    // The knob's gesture ends with its release.
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    h.eval_panel("(def chips t0.variants)");
    assert_eq!(
        h.eval_panel("(map (lambda (v) v.label) chips)"),
        h.eval_panel(r#"(list "A" "B")"#)
    );
    // The registry's chips, as the legacy strip showed them.
    let registry = h.shared.state.plock_variant_registry_snapshot(0);
    assert_eq!(registry.entries.len(), 2);
    for (at, entry) in registry.entries.iter().enumerate() {
        let chip = VariantChip::of(entry);
        let code =
            format!("(let ((v (nth chips {at}))) (list v.label v.name v.count v.track v.device))");
        let read = items(&h.eval_panel(&code));
        assert_eq!(read[0], Value::String(chip.label));
        assert_eq!(read[1], Value::String(chip.name));
        assert_eq!(read[2], Value::Number(chip.count as f64));
        assert_eq!(read[3], h.eval_panel("t0"));
        assert_eq!(read[4], Value::Nil);
        let color = h.cells(&format!("(let ((v (nth chips {at}))) (rest v.color))"));
        assert!(same_cells(&color, &chip.color.map(f64::from)), "{color:?}");
    }
    // current follows the selected step.
    h.eval_panel(
        "(def a (first chips)) (def b (nth chips 1)) (def a-current #'a.current) \
         (def b-current #'b.current)",
    );
    h.sync();
    assert_eq!(h.slot("a-current"), 0.0);
    let scans = |h: &Harness| h.frame.host_kinds.shared.borrow().variant_current_scans;
    let before = scans(&h);
    h.shared.selected_steps.lock().unwrap().insert(4);
    h.sync();
    assert_eq!((h.slot("a-current"), h.slot("b-current")), (1.0, 0.0));
    // The selected step's variant is read once per track per tick, however
    // many variants observe it; not at all while nothing moved.
    assert_eq!(scans(&h), before + 1);
    h.sync();
    h.sync();
    assert_eq!(scans(&h), before + 1);
    // Stamping A onto steps 7 and 2: one undo entry for the step that
    // differs.
    let before = h.app.history.undo_len();
    h.eval_panel("(stamp-variant! t0 (list (nth t0.steps 7) (nth t0.steps 2)) a)");
    h.drain_and_sync();
    assert_eq!(h.app.history.undo_len(), before + 1);
    assert_eq!(h.filter_slot(slot).plocks.get(7, CUTOFF), Some(500.0));
    assert_eq!(h.eval_panel("(first t0.variants)"), h.eval_panel("a"));
    // nil clears the steps' variant locks.
    h.eval_panel("(stamp-variant! t0 (list (nth t0.steps 6)) nil)");
    h.drain_and_sync();
    assert_eq!(h.filter_slot(slot).plocks.get(6, CUTOFF), None);
    assert_eq!(h.app.history.undo_len(), before + 2);
    assert_eq!(h.eval_panel("(len t0.variants)"), Value::Number(1.0));
    assert_eq!(
        h.eval_panel("b.label"),
        s(""),
        "B is gone: its handle is stale"
    );
    h.undo();
    assert_eq!(h.filter_slot(slot).plocks.get(6, CUTOFF), Some(800.0));
    h.undo();
    h.sync();
    assert_eq!(h.filter_slot(slot).plocks.get(7, CUTOFF), None);
    assert_eq!(h.filter_slot(slot).plocks.get(6, CUTOFF), Some(800.0));
    h.rejects_in(
        REFER_PANEL,
        r#"(stamp-variant! t0 (list (nth t0.steps 1)) (dict :label "Z"))"#,
        "the track has no variant Z",
        false,
    );
    h.rejects_in(
        REFER_PANEL,
        "(let ((t1 (track 1))) (stamp-variant! t0 (list (nth t1.steps 1)) a))",
        "steps must be steps of the track",
        false,
    );
    // The registry is read once per p-lock key.
    let scans = h.panel_scans();
    h.sync();
    h.sync();
    assert_eq!(h.panel_scans(), scans);
}

#[test]
fn a_held_variant_handle_goes_stale_rather_than_naming_a_later_variant() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    for (step, value) in [(2, 500.0), (6, 800.0)] {
        h.lock_effect(slot, step, CUTOFF, value);
    }
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    // Held, but the list is never observed nor read again.
    h.eval_panel("(def b (nth t0.variants 1))");
    assert_eq!(h.eval_panel("b.label"), s("B"));
    h.eval_panel("(stamp-variant! t0 (list (nth t0.steps 6)) nil)");
    h.drain_and_sync();
    // Another lock set takes the free label B.
    h.lock_effect(slot, 8, CUTOFF, 900.0);
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    let labels = h.eval_panel("(map (lambda (v) v.label) t0.variants)");
    assert_eq!(labels, h.eval_panel(r#"(list "A" "B")"#));
    assert_ne!(h.eval_panel("(nth t0.variants 1)"), h.eval_panel("b"));
    assert_eq!(h.eval_panel("b.label"), s(""), "the old handle is stale");
}

#[test]
fn key_lock_variants_list_their_keys_and_stamp_through_history() {
    let (mut h, _) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    let command = app::AppCommand::SetInstrumentKeyLockMulti {
        track: 2,
        notes: vec![60, 62],
        param_idx: START,
        value: 0.5,
    };
    app::apply_command(&mut h.app, command);
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_panel("(def kv (first inst.variants))");
    assert_eq!(
        h.eval_panel(
            "(list (len inst.variants) kv.label kv.count kv.notes kv.device kv.track kv.current)"
        ),
        h.eval_panel(r#"(list 1 "A" 1 (list 60 62) inst t2 false)"#)
    );
    assert_eq!(
        h.eval_panel("(len t2.variants)"),
        Value::Number(0.0),
        "no step variants"
    );
    let before = h.app.history.undo_len();
    h.eval_panel("(stamp-key-variant! inst (list 62 64) kv)");
    h.drain_and_sync();
    assert_eq!(h.app.history.undo_len(), before + 1);
    let key_lock = |h: &Harness, note: u8| {
        let slot = &h.shared.state.pattern.instrument_slots[2];
        slot.key_locks.get(note, START)
    };
    assert_eq!(key_lock(&h, 64), Some(0.5));
    assert_eq!(h.eval_panel("kv.notes"), h.eval_panel("(list 60 62 64)"));
    h.eval_panel("(stamp-key-variant! inst (list 60) nil)");
    h.drain_and_sync();
    assert_eq!(key_lock(&h, 60), None);
    h.undo();
    h.undo();
    h.sync();
    assert_eq!(key_lock(&h, 60), Some(0.5));
    assert_eq!(key_lock(&h, 64), None);
    h.rejects_in(
        REFER_PANEL,
        "(stamp-key-variant! flt (list 60) kv)",
        "has no key-lock variants",
        false,
    );
    h.rejects_in(
        REFER_PANEL,
        "(stamp-key-variant! inst (list 200) kv)",
        "200 is no MIDI note",
        false,
    );
}

#[test]
fn project_macros_read_set_and_keep_their_identity() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    let create = |h: &mut Harness, name: &str| {
        let command = app::AppCommand::MacroCreate {
            name: name.to_string(),
        };
        app::apply_command(&mut h.app, command);
        h.app.macro_engine.macros().last().unwrap().id
    };
    let first = create(&mut h, "Sweep");
    let target = sequencer::process::ParamTarget::EffectParam {
        slot,
        effect: "Filter".to_string(),
        param: "cutoff".to_string(),
        param_id: None,
    };
    let command = app::AppCommand::MacroMapParam {
        id: first,
        track: 0,
        target,
    };
    app::apply_command(&mut h.app, command);
    h.sync();
    h.eval_panel("(def m (first (macros))) (def mm (first m.mappings))");
    assert_eq!(
        h.eval_panel(
            "(list (len (macros)) m.index m.mid m.name m.type m.script-key (len m.mappings))"
        ),
        h.eval_panel(&format!(r#"(list 1 0 {first} "Sweep" "mapped" "" 1)"#))
    );
    assert_eq!(
        h.eval_panel("mm.target"),
        h.eval_panel("cutoff"),
        "the target is the param instance"
    );
    assert_eq!(
        h.eval_panel("(list mm.macro mm.rack-macro mm.index m.target-scene)"),
        h.eval_panel("(list m nil 0 nil)")
    );
    // The mapping table's columns, as the model shows them.
    let model = h.app.macro_engine.macros()[0].clone();
    let mapping = &model.mappings[0];
    let (path, param, min, max, ..) = macro_mapping_display_metadata(&h.app, mapping);
    let label = format!("{path} · {}", process_param_target_label(&mapping.target));
    assert_eq!(h.eval_panel("mm.label"), s(&label));
    assert_eq!(h.eval_panel("mm.path"), s(&path));
    assert_eq!(h.eval_panel("mm.param-label"), s(&param));
    assert_eq!(h.eval_panel("mm.min"), Value::Number(f64::from(min)));
    assert_eq!(h.eval_panel("mm.max"), Value::Number(f64::from(max)));
    assert_eq!(h.eval_panel("mm.curve"), s(mapping.curve.label()));
    assert_eq!(h.eval_panel("mm.suspended"), Value::Bool(mapping.suspended));
    assert_eq!(
        h.eval_panel("m.value"),
        Value::Number(f64::from(model.value))
    );
    // The value: a performance control (no undo entry), compared per tick
    // with no structure sync.
    let syncs = h.frame.host_kinds.macros.syncs;
    let before = h.app.history.undo_len();
    h.eval_panel("(set! m.value 0.5)");
    h.drain_and_sync();
    assert_eq!(h.app.macro_engine.macros()[0].value, 0.5);
    assert_eq!(h.eval_panel("m.value"), Value::Number(0.5));
    assert_eq!(
        h.frame.host_kinds.macros.syncs, syncs,
        "a value moves no structure"
    );
    assert_eq!(h.app.history.undo_len(), before);
    // The engaged macro shows in the param it drives.
    let effective = h.app.effective_slot_param_value(0, slot, CUTOFF).unwrap();
    assert!(close(h.eval_panel("cutoff.value"), f64::from(effective)));
    // The name and the mapping: through history, undone.
    h.eval_panel(r#"(set! m.name "Open")"#);
    h.drain_and_sync();
    assert_eq!(h.eval_panel("m.name"), s("Open"));
    assert_eq!(h.app.history.undo_len(), before + 1);
    h.eval_panel(r#"(set! mm.max 4000) (set! mm.curve "exp")"#);
    h.drain_and_sync();
    let mapping = h.app.macro_engine.macros()[0].mappings[0].clone();
    assert_eq!(mapping.range_max, 4000.0);
    assert_eq!(mapping.curve, sequencer::macro_engine::MacroCurve::Exp);
    assert_eq!(
        h.eval_panel("(list mm.max mm.curve)"),
        h.eval_panel(r#"(list 4000 "exp")"#)
    );
    for _ in 0..3 {
        h.undo();
    }
    h.sync();
    assert_eq!(
        h.eval_panel("(list m.name mm.curve)"),
        h.eval_panel(r#"(list "Sweep" "linear")"#)
    );
    h.rejects_in(
        REFER_PANEL,
        "(set! mm.max 99999)",
        "a number from 20 to 20000",
        false,
    );
    h.rejects_in(REFER_PANEL, r#"(set! mm.curve "wobbly")"#, "one of", false);
    // Every curve the kind shows sets back (log-domain included).
    let undo_len = h.app.history.undo_len();
    h.eval_panel(r#"(set! mm.curve "log-domain")"#);
    h.drain_and_sync();
    use sequencer::macro_engine::MacroCurve;
    let curve = h.app.macro_engine.macros()[0].mappings[0].curve;
    assert_eq!(curve, MacroCurve::LogDomain);
    assert_eq!(h.eval_panel("mm.curve"), s("log-domain"));
    h.eval_panel("(set! mm.curve mm.curve)");
    h.drain_and_sync();
    assert_eq!(
        h.app.history.undo_len(),
        undo_len + 1,
        "the same curve: no entry"
    );
    h.undo();
    h.sync();
    for curve in [
        MacroCurve::Linear,
        MacroCurve::Exp,
        MacroCurve::Log,
        MacroCurve::LogDomain,
    ] {
        assert_eq!(MacroCurve::from_label(curve.label()), Some(curve));
    }
    use sequencer::sequencer::RackMacroCurve;
    for curve in [
        RackMacroCurve::Linear,
        RackMacroCurve::Exp,
        RackMacroCurve::Log,
    ] {
        assert_eq!(RackMacroCurve::from_label(curve.label()), Some(curve));
    }
    assert_eq!(RackMacroCurve::from_label("log-domain"), None);
    h.rejects_in(
        REFER_PANEL,
        r#"(set! m.name "  ")"#,
        "a non-empty name",
        false,
    );
    // Identity: a second macro, then the first deleted: the second keeps
    // its instance (re-keyed).
    let second = create(&mut h, "Other");
    h.sync();
    h.eval_panel("(def m2 (nth (macros) 1))");
    app::apply_command(&mut h.app, app::AppCommand::MacroDelete { id: first });
    h.sync();
    assert_eq!(h.eval_panel("(first (macros))"), h.eval_panel("m2"));
    assert_eq!(
        h.eval_panel("(list m2.index m2.mid)"),
        h.eval_panel(&format!("(list 0 {second})"))
    );
    assert_eq!(
        h.eval_panel("m.name"),
        s(""),
        "the deleted macro's handle is stale"
    );
}

impl Harness {
    /// A drum rack on track 2 whose macro 1 maps onto its first slot's
    /// `start` (stored 0–0.5); `rk` the rack, `rs` the slot, `rm` the
    /// macro, `rmm` its mapping.
    fn mapped_rack_macro(&mut self) -> sequencer::sequencer::RackMacroId {
        self.rack_track();
        self.sync();
        let id = sequencer::sequencer::RackMacroId::from_index(1).unwrap();
        let mapping = sequencer::sequencer::RackMacroMapping {
            target: sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                slot: 0,
                param: "start".to_string(),
                param_index: START,
            },
            range_min: 0.0,
            range_max: 0.5,
            curve: sequencer::sequencer::RackMacroCurve::Linear,
        };
        self.app
            .map_rack_macro(2, id, mapping)
            .expect("map the macro");
        self.sync();
        self.eval_panel(
            "(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices)) \
             (def rm (nth rk.macros 1)) (def rmm (first rm.mappings))",
        );
        id
    }

    /// Track 2's rack macro 1: (name, base, mapping range, curve label),
    /// asserting the live rack and the effective pattern agree.
    fn rack_macro_1(&self) -> (String, f32, (f32, f32), &'static str) {
        let read = |rack_macro: &sequencer::sequencer::RackMacro| {
            let mapping = &rack_macro.mappings[0];
            (
                rack_macro.name.clone(),
                rack_macro.value,
                (mapping.range_min, mapping.range_max),
                mapping.curve.label(),
            )
        };
        let live = read(
            &self
                .shared
                .state
                .live_rack_track_snapshot(2)
                .unwrap()
                .macros[1],
        );
        let stored = self.app.state.with_project_scenes(|scenes| {
            let pattern = scenes.effective_pattern_id(2).unwrap();
            read(&scenes.track_pools[2].rack_macros(pattern).unwrap()[1])
        });
        assert_eq!(live, stored, "the live rack and the pattern agree");
        live
    }
}

#[test]
fn rack_macros_read_set_and_map_onto_the_slot_params() {
    let mut h = Harness::new();
    let id = h.mapped_rack_macro();
    assert_eq!(h.eval_panel("(len rk.macros)"), Value::Number(8.0));
    assert_eq!(
        h.eval_panel("(list rm.device rm.index rm.stable-key rm.name (len rm.mappings))"),
        h.eval_panel(r#"(list rk 1 "macro_2" "Macro 2" 1)"#)
    );
    assert_eq!(
        h.eval_panel("rmm.target"),
        h.eval_panel(r#"(device-param rs "start")"#)
    );
    assert_eq!(
        h.eval_panel("(list rmm.macro rmm.rack-macro rmm.min rmm.max rmm.curve)"),
        h.eval_panel(r#"(list nil rm 0 50 "linear")"#)
    );
    assert_eq!(
        h.eval_panel("(len rs.macros)"),
        Value::Number(0.0),
        "only the rack's device"
    );
    // Live: the shown value, the base, the lock flags.
    h.eval_panel(
        "(def shown #'rm.value) (def own #'rm.base) (def locked #'rm.locked) \
         (def any #'rm.has-locks)",
    );
    h.sync();
    let read = |h: &mut Harness| {
        (
            h.slot("shown"),
            h.slot("own"),
            h.slot("locked"),
            h.slot("any"),
        )
    };
    assert_eq!(read(&mut h), (0.0, 0.0, 0.0, 0.0));
    h.eval_panel("(set! rm.base 0.75)");
    h.drain_and_sync();
    assert_eq!(read(&mut h), (0.75, 0.75, 0.0, 0.0));
    let command = app::AppCommand::SetRackMacroPlockMulti {
        track: 2,
        steps: vec![3],
        macro_idx: 1,
        value: 0.25,
    };
    app::apply_command(&mut h.app, command);
    // A first lock on the displayed step moves the UI epoch (as the rack
    // panel's command): `has-locks` follows the track's p-lock key.
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.shared.selected_steps.lock().unwrap().insert(3);
    h.sync();
    assert_eq!(read(&mut h), (0.25, 0.75, 1.0, 1.0));
    // Four observed fields of this macro and another's value: one rack
    // lock per tick.
    h.eval_panel("(def rm3 (nth rk.macros 2)) (def other #'rm3.value)");
    h.sync();
    let locks = h.frame.host_kinds.macros.rack_live_locks;
    h.sync();
    assert_eq!(h.frame.host_kinds.macros.rack_live_locks, locks + 1);
    h.sync();
    assert_eq!(h.frame.host_kinds.macros.rack_live_locks, locks + 2);
    // A rename is the rack panel's live text edit; a mapping's range is in
    // its target's display units.
    h.eval_panel(r#"(set! rm.name "Tone") (set! rmm.max 80)"#);
    h.drain_and_sync();
    assert_eq!(
        h.eval_panel("(list rm.name rmm.max)"),
        h.eval_panel(r#"(list "Tone" 80)"#)
    );
    let rack = h.shared.state.live_rack_track_snapshot(2).unwrap();
    assert!((rack.macros[1].mappings[0].range_max - 0.8).abs() < 1e-6);
    // A base edit (the rack revision moves) syncs no macro structure.
    let syncs = h.frame.host_kinds.macros.rack_syncs;
    h.eval_panel("(set! rm.base 0.5)");
    h.drain_and_sync();
    assert_eq!(h.frame.host_kinds.macros.rack_syncs, syncs);
    h.rejects_in(
        REFER_PANEL,
        "(set! rm.base 2)",
        "a number from 0 to 1",
        false,
    );
    h.rejects_in(REFER_PANEL, r#"(set! rmm.curve "wobbly")"#, "one of", false);
    h.rejects_in(
        REFER_PANEL,
        "(set! rmm.max 300)",
        "a number from 0 to 100",
        false,
    );
    // A bound outside the target's range (an older mapping) sets back as
    // it reads: the no-op comes before the range check.
    let target = h.app.rack_macro_mapping_target(2, id, 0).unwrap();
    let range = sequencer::sequencer::RackMacroField::Range {
        target,
        min: 0.0,
        max: 2.0,
    };
    h.app
        .apply_rack_macro_edit(2, id, range)
        .expect("widen the range");
    h.drain_and_sync();
    assert_eq!(h.eval_panel("rmm.max"), Value::Number(200.0));
    h.editor.minibuffer = None;
    h.eval_panel("(set! rmm.max rmm.max)");
    h.drain();
    assert_eq!(h.error(), "", "the current value always works");
}

#[test]
fn rack_macro_setters_record_one_entry_each_and_a_drag_joins_one() {
    let mut h = Harness::new();
    h.mapped_rack_macro();
    let start = h.rack_macro_1();
    assert_eq!(start, ("Macro 2".to_string(), 0.0, (0.0, 0.5), "linear"));
    let undo = h.app.history.undo_len();
    h.eval_panel(r#"(set! rm.base 0.75) (set! rm.name "Tone") (set! rmm.max 80)"#);
    h.eval_panel(r#"(set! rmm.curve "exp")"#);
    h.drain_and_sync();
    let edited = ("Tone".to_string(), 0.75, (0.0, 0.8), "exp");
    assert_eq!(h.rack_macro_1(), edited);
    assert_eq!(h.app.history.undo_len(), undo + 4, "one entry each");
    // Absolute: the values it holds add no entry.
    h.eval_panel(r#"(set! rm.base 0.75) (set! rm.name "Tone") (set! rmm.curve "exp")"#);
    h.drain_and_sync();
    assert_eq!(h.app.history.undo_len(), undo + 4);
    // Undo walks back one field at a time; redo replays them.
    let steps = [
        ("Tone".to_string(), 0.75, (0.0, 0.8), "linear"),
        ("Tone".to_string(), 0.75, (0.0, 0.5), "linear"),
        ("Macro 2".to_string(), 0.75, (0.0, 0.5), "linear"),
        start.clone(),
    ];
    for expected in &steps {
        h.undo();
        assert_eq!(&h.rack_macro_1(), expected);
    }
    for _ in &steps {
        app::edit::redo(&mut h.app);
    }
    assert_eq!(h.rack_macro_1(), edited);
    h.sync();
    assert_eq!(
        h.eval_panel("(list rm.name rm.base rmm.max rmm.curve)"),
        h.eval_panel(r#"(list "Tone" 0.75 80 "exp")"#)
    );
    // A drag view: base and min set!s while the pointer is down join one
    // entry per field, which undoes to the value before the drag.
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for value in [0.5, 0.25, 0.1] {
        h.eval_panel(&format!("(set! rm.base {value})"));
        h.drain();
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.rack_macro_1().1, 0.1);
    assert_eq!(h.app.history.undo_len(), undo + 1, "the drag is one entry");
    h.gesture.pointer_down = true;
    for min in [10, 20, 30] {
        h.eval_panel(&format!("(set! rmm.min {min})"));
        h.drain();
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.rack_macro_1().2, (0.3, 0.8));
    assert_eq!(h.app.history.undo_len(), undo + 2);
    h.undo();
    assert_eq!(h.rack_macro_1().2, (0.0, 0.8));
    h.undo();
    assert_eq!(h.rack_macro_1(), edited);
    // A bad value is an error that records nothing.
    h.rejects_in(
        REFER_PANEL,
        "(set! rm.base 2)",
        "a number from 0 to 1",
        true,
    );
}

/// Rack macro 1's own value in the Patch track 2's `pattern` plays, and in
/// the live rack.
fn rack_macro_1_values(
    state: &sequencer::sequencer::SequencerState,
    pattern: sequencer::sequencer::PatternId,
) -> (f32, f32) {
    let stored = state.with_project_scenes(|scenes| {
        let patch = scenes.track_pools[2].patch(pattern).unwrap();
        patch.rack_track.as_ref().unwrap().macros[1].value
    });
    (
        stored,
        state.live_rack_track_snapshot(2).unwrap().macros[1].value,
    )
}

/// A pattern of track 2 copied from `from`, with a Patch of its own or
/// (`shared`) sharing `from`'s.
fn rack_pattern_copy(
    state: &sequencer::sequencer::SequencerState,
    from: sequencer::sequencer::PatternId,
    shared: bool,
) -> sequencer::sequencer::PatternId {
    state.with_scenes_mut(|scenes| {
        let pool = &mut scenes.track_pools[2];
        let data = pool.get(from).unwrap();
        match shared {
            true => {
                let refs = pool.refs(from).unwrap();
                pool.insert_with_refs(data, refs)
            }
            false => pool.insert(data),
        }
    })
}

#[test]
fn rack_macro_edits_follow_the_patch_the_live_rack_mirrors() {
    use sequencer::sequencer::RackMacroField;
    let mut h = Harness::new();
    let id = h.mapped_rack_macro();
    let state = h.app.state.clone();
    let scene = state.with_project_scenes(|scenes| scenes.effective_pattern_id(2).unwrap());
    // A bound take with a Patch of its own: the edit lands in the copy the
    // panel shows (the take's), and its undo there.
    let take = rack_pattern_copy(&state, scene, false);
    let data = state.with_project_scenes(|scenes| scenes.track_pools[2].get(take).unwrap());
    assert!(state.borrow_track_device_state(2, take, &data));
    h.app
        .apply_rack_macro_edit(2, id, RackMacroField::Value(0.5))
        .unwrap();
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(rack_macro_1_values(&state, take), (0.5, 0.5));
    assert_eq!(
        rack_macro_1_values(&state, scene).0,
        0.0,
        "the scene's Patch is untouched"
    );
    h.undo();
    assert_eq!(rack_macro_1_values(&state, take), (0.0, 0.0));
    state.release_bound_device_state();
    // A pattern sharing the scene pattern's Patch: an undo after switching
    // to it still reaches the live rack.
    h.app
        .apply_rack_macro_edit(2, id, RackMacroField::Value(0.75))
        .unwrap();
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(rack_macro_1_values(&state, scene), (0.75, 0.75));
    let sibling = rack_pattern_copy(&state, scene, true);
    state.with_scenes_mut(|scenes| scenes.track_overrides[2] = Some(sibling));
    h.undo();
    assert_eq!(rack_macro_1_values(&state, sibling), (0.0, 0.0));
    assert_eq!(rack_macro_1_values(&state, scene).0, 0.0);
}

#[test]
fn a_rack_macro_drag_across_a_pattern_switch_keeps_one_entry_per_pattern() {
    use sequencer::sequencer::RackMacroField;
    let mut h = Harness::new();
    let id = h.mapped_rack_macro();
    let state = h.app.state.clone();
    let scene = state.with_project_scenes(|scenes| scenes.effective_pattern_id(2).unwrap());
    let other = rack_pattern_copy(&state, scene, false);
    let undo = h.app.history.undo_len();
    for value in [0.2, 0.4] {
        h.app
            .apply_rack_macro_edit(2, id, RackMacroField::Value(value))
            .unwrap();
    }
    // The pattern switches under the open drag.
    state.with_scenes_mut(|scenes| scenes.track_overrides[2] = Some(other));
    for value in [0.6, 0.8] {
        h.app
            .apply_rack_macro_edit(2, id, RackMacroField::Value(value))
            .unwrap();
    }
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), undo + 2, "one entry per pattern");
    assert_eq!(rack_macro_1_values(&state, scene).0, 0.4);
    assert_eq!(rack_macro_1_values(&state, other), (0.8, 0.8));
    h.undo();
    assert_eq!(rack_macro_1_values(&state, other).0, 0.0, "its own before");
    assert_eq!(rack_macro_1_values(&state, scene).0, 0.4);
    h.undo();
    assert_eq!(rack_macro_1_values(&state, scene).0, 0.0);
}

#[test]
fn a_rack_macro_mapping_edit_names_its_mapping_by_target() {
    use sequencer::sequencer::{RackMacroCurve, RackMacroField, RackMacroMapping, RackMacroTarget};
    let mut h = Harness::new();
    let id = h.mapped_rack_macro();
    let gain = RackMacroTarget::SlotParam {
        slot: 0,
        param: "gain".to_string(),
    };
    let mapping = RackMacroMapping {
        target: gain.clone(),
        range_min: 0.0,
        range_max: 1.0,
        curve: RackMacroCurve::Linear,
    };
    h.app.map_rack_macro(2, id, mapping).expect("map the gain");
    let range = |h: &Harness| {
        let rack = h.shared.state.live_rack_track_snapshot(2).unwrap();
        let mappings = &rack.macros[1].mappings;
        let (index, mapping) = (mappings.iter().enumerate())
            .find(|(_, mapping)| mapping.target == gain)
            .unwrap();
        (index, mapping.range_min, mapping.range_max, mappings.len())
    };
    let edit = RackMacroField::Range {
        target: gain.clone(),
        min: 0.25,
        max: 0.75,
    };
    h.app.apply_rack_macro_edit(2, id, edit).unwrap();
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(range(&h), (1, 0.25, 0.75, 2));
    // Unmapping the earlier mapping moves the gain's to position 0: undo
    // still restores the gain's range.
    assert!(h.app.unmap_rack_macro(2, id, 0));
    h.undo();
    assert_eq!(range(&h), (0, 0.0, 1.0, 1));
    // With the gain unmapped too, redo has no mapping to write: an error
    // that changes nothing.
    assert!(h.app.unmap_rack_macro(2, id, 0));
    let replay = app::edit::redo(&mut h.app);
    assert!(
        matches!(replay, app::history::HistoryReplay::Failed(_)),
        "{replay:?}"
    );
    let rack = h.shared.state.live_rack_track_snapshot(2).unwrap();
    assert!(rack.macros[1].mappings.is_empty());
}

#[test]
fn rack_macro_locks_set_and_clear_the_steps_that_differ_through_history() {
    let mut h = Harness::new();
    h.mapped_rack_macro();
    h.eval_panel("(def s2 (nth t2.steps 2)) (def s5 (nth t2.steps 5)) (def s7 (nth t2.steps 7))");
    let locks = |h: &Harness| {
        let rack = h.shared.state.live_rack_track_snapshot(2).unwrap();
        [2, 5, 7].map(|step| rack.macros[1].plocks[step])
    };
    // The rack is the current track, its step 2 shown.
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.shared.selected_steps.lock().unwrap().insert(2);
    h.sync();
    let before = h.app.history.undo_len();
    h.eval_panel("(lock-rack-macro! rm (list s2 s5) 0.25)");
    h.drain_and_sync();
    assert_eq!(
        h.app.history.undo_len(),
        before + 1,
        "one entry for both steps"
    );
    assert_eq!(locks(&h), [Some(0.25), Some(0.25), None]);
    assert_eq!(
        h.eval_panel("(list rm.value rm.base rm.locked rm.has-locks)"),
        h.eval_panel("(list 0.25 0 true true)")
    );
    // Steps already holding the lock, or none to clear, are left alone.
    h.eval_panel("(lock-rack-macro! rm (list s2 s5) 0.25) (unlock-rack-macro! rm (list s7))");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 1, "nothing to do");
    h.eval_panel("(lock-rack-macro! rm (list s5 s7) 0.5)");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 2);
    assert_eq!(locks(&h), [Some(0.25), Some(0.5), Some(0.5)]);
    // Clearing is one entry; undo puts the locks back.
    h.eval_panel("(unlock-rack-macro! rm (list s2 s5 s7))");
    h.drain_and_sync();
    assert_eq!(h.app.history.undo_len(), before + 3);
    assert_eq!(locks(&h), [None, None, None]);
    assert_eq!(
        h.eval_panel("(list rm.value rm.locked rm.has-locks)"),
        h.eval_panel("(list 0 false false)")
    );
    h.undo();
    assert_eq!(locks(&h), [Some(0.25), Some(0.5), Some(0.5)]);
    h.undo();
    assert_eq!(locks(&h), [Some(0.25), Some(0.25), None]);
    // The value rule: a number in 0–1, steps of the rack's track.
    h.rejects_in(
        REFER_PANEL,
        "(lock-rack-macro! rm (list s2) 2)",
        "a number from 0 to 1",
        true,
    );
    h.rejects_in(
        REFER_PANEL,
        "(let ((t0 (track 0))) (lock-rack-macro! rm (list (nth t0.steps 2)) 0.5))",
        "steps of the device's track",
        true,
    );
    h.rejects_in(
        REFER_PANEL,
        "(let ((t0 (track 0))) (unlock-rack-macro! rm (list (nth t0.steps 2))))",
        "steps of the device's track",
        true,
    );
    assert_eq!(locks(&h), [Some(0.25), Some(0.25), None]);
}

/// A script drag of `lock-rack-macro!` on one step is one undo entry until
/// the release; another step starts the next (eseq-0l17.58).
#[test]
fn a_script_drag_of_rack_macro_locks_joins_one_entry_per_step() {
    let mut h = Harness::new();
    h.mapped_rack_macro();
    h.eval_panel("(def s2 (nth t2.steps 2)) (def s5 (nth t2.steps 5))");
    let locks = |h: &Harness| {
        let rack = h.shared.state.live_rack_track_snapshot(2).unwrap();
        [2, 5].map(|step| rack.macros[1].plocks[step])
    };
    let before = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for value in [0.25, 0.5, 0.75] {
        h.eval_panel(&format!("(lock-rack-macro! rm (list s2) {value})"));
        h.drain();
    }
    h.eval_panel("(lock-rack-macro! rm (list s5) 0.5)");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 1, "step 2's entry");
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    h.gesture.script_param_gesture = None;
    assert_eq!(h.app.history.undo_len(), before + 2, "one entry per step");
    assert_eq!(locks(&h), [Some(0.75), Some(0.5)]);
    h.undo();
    assert_eq!(locks(&h), [Some(0.75), None]);
    h.undo();
    assert_eq!(locks(&h), [None, None]);
    // No pointer: an entry per call.
    h.eval_panel("(lock-rack-macro! rm (list s2) 0.25)");
    h.drain();
    h.eval_panel("(lock-rack-macro! rm (list s2) 0.5)");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 2);
}

#[test]
fn a_selected_neurons_override_shows_in_the_param_value() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    h.eval_panel(
        "(def value #'cutoff.value) (def locked #'cutoff.locked) \
         (def overridden #'cutoff.overridden)",
    );
    h.sync();
    assert_eq!(h.slot("overridden"), 0.0);
    let base = f64::from(h.filter_slot(slot).defaults.get(CUTOFF));
    assert!((h.slot("value") - base).abs() < 1e-3);
    let param_id = h.filter_slot(slot).param_node_id(CUTOFF);
    let param_id = param_id.expect("a live node identity");
    h.shared
        .state
        .edit_current_neural_networks(|networks| {
            let mut network = sequencer::neural::ProjectNeuralNetwork {
                id: 11,
                name: "router".to_string(),
                num_neurons: 1,
                ..sequencer::neural::ProjectNeuralNetwork::default()
            };
            network.neurons[0].output_overrides.effects =
                vec![sequencer::neural::ProjectEffectParamOverride {
                    target_track: 0,
                    slot_index: slot,
                    param_id,
                    param_index: CUTOFF,
                    value: 1234.0,
                }];
            networks.push(network);
            Ok(())
        })
        .unwrap();
    h.sync();
    assert!((h.slot("value") - base).abs() < 1e-3, "no neuron selected");
    let neuron = sequencer::lisp_host::SelectedNeuralNeuron {
        pattern_idx: 0,
        network_id: 11,
        neuron_idx: 0,
    };
    h.shared
        .selected_neural_neurons
        .lock()
        .unwrap()
        .insert(neuron);
    h.sync();
    assert_eq!(h.slot("value"), 1234.0);
    // overridden says why; locked keeps meaning a p-lock supplies it.
    assert_eq!(h.slot("overridden"), 1.0);
    assert_eq!(h.slot("locked"), 0.0);
    h.lock_effect(slot, 5, CUTOFF, 900.0);
    h.shared.selected_steps.lock().unwrap().insert(5);
    h.sync();
    assert_eq!(h.slot("value"), 1234.0);
    assert_eq!((h.slot("locked"), h.slot("overridden")), (1.0, 1.0));
    h.shared.selected_steps.lock().unwrap().clear();
    h.sync();
    h.shared.selected_neural_neurons.lock().unwrap().clear();
    h.sync();
    assert!((h.slot("value") - base).abs() < 1e-3);
    assert_eq!(h.slot("overridden"), 0.0);
}

#[test]
fn a_cold_playhead_read_follows_a_voice_rebuild_without_a_model_sync() {
    let (mut h, _) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    h.sync();
    let refreshes = h.sampler_refreshes();
    let syncs = h.model_syncs();
    h.sync();
    assert_eq!(h.sampler_refreshes(), refreshes, "unchanged: nothing");
    // A voice rebuild (other sampler nodes) moves no model counter.
    let ids = &mut h.app.graph.track_node_ids[2].sampler_ids;
    assert!(!ids.is_empty(), "the sampler has voices");
    ids.push(-1);
    h.sync();
    assert_eq!(h.model_syncs(), syncs, "no model sync");
    assert_eq!(h.sampler_refreshes(), refreshes + 1);
    // The cold read samples the current voices (none plays: 0).
    assert_eq!(h.eval_panel("inst.playhead"), Value::Number(0.0));
    h.sync();
    assert_eq!(h.sampler_refreshes(), refreshes + 1);
}

/// What the step panel's table holds while it shows (the rows are built
/// while observed).
const OBSERVE_TABLE: &str = r#"(effect-buffer "*plock-table*"
    (label (str (len selection.plock-rows) selection.plock-variant)))"#;

/// eseq-0l17.74: the step panel's p-lock table is `selection.plock-rows`, a
/// `plock-row` per lock at the first selected step. A device param's lock
/// carries its param and step (the table binds the param's value and edits
/// through `lock-param!` / `unlock-param!`), in the param's display units;
/// a lock edit keeps the row's instance, and nothing rebuilds while nothing
/// moved.
#[test]
fn plock_rows_list_the_selected_steps_locks_and_carry_their_param() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    h.eval_panel(OBSERVE_TABLE);
    h.show_all();
    h.lock_effect(slot, 2, CUTOFF, 900.0);
    h.sync();
    assert_eq!(
        h.eval_panel("(len selection.plock-rows)"),
        Value::Number(0.0)
    );
    assert_eq!(h.eval_panel("selection.plock-variant"), s("def"));
    h.shared.selected_steps.lock().unwrap().insert(2);
    h.sync();
    h.eval_panel("(def rows selection.plock-rows) (def r (first rows))");
    assert_eq!(h.eval_panel("(len rows)"), Value::Number(1.0));
    let read = items(&h.eval_panel(
        "(list r.target r.domain r.source r.name r.text (= r.param cutoff) \
         (= r.step (nth t0.steps 2)) r.rack-macro r.index)",
    ));
    assert_eq!(
        read,
        vec![
            s("effect"),
            s("fx"),
            s("step"),
            s("cutoff"),
            s("900.00"),
            Value::Bool(true),
            Value::Bool(true),
            Value::Nil,
            Value::Number(0.0),
        ]
    );
    assert!(close(h.eval_panel("r.value"), 900.0));
    assert_eq!(h.eval_panel("selection.plock-variant"), s("A"));
    let row = h.eval_panel("r");
    let builds = h.frame.host_kinds.plock_rows.builds;
    h.sync();
    assert_eq!(
        h.frame.host_kinds.plock_rows.builds, builds,
        "nothing moved"
    );
    // The table's edit: the row stays, its value moves.
    h.eval_panel("(lock-param! r.param (list r.step) 1200)");
    h.drain_and_sync();
    assert_eq!(h.eval_panel("(first selection.plock-rows)"), row);
    assert!(close(h.eval_panel("r.value"), 1200.0));
    assert!(close(h.eval_panel("r.param.value"), 1200.0));
    h.eval_panel("(unlock-param! r.param (list r.step))");
    h.drain_and_sync();
    assert_eq!(
        h.eval_panel("(len selection.plock-rows)"),
        Value::Number(0.0)
    );
    assert_eq!(h.eval_panel("selection.plock-variant"), s("def"));
}

/// eseq-0l17.74: a chip clicked with no step selected previews its variant
/// (the gesture state the tick hands the host kinds): the table lists the
/// variant's locks, read-only, and lights its chip; a selection replaces
/// the preview with the selected step's locks.
#[test]
fn plock_rows_preview_a_variant_while_no_step_is_selected() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    h.eval_panel(OBSERVE_TABLE);
    h.show_all();
    for (step, value) in [(2, 500.0), (6, 800.0)] {
        h.lock_effect(slot, step, CUTOFF, value);
    }
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    h.command("preview-plock-variant", map_value([("label", s("B"))]));
    h.sync();
    h.eval_panel("(def r (first selection.plock-rows))");
    let read = items(&h.eval_panel("(list (len selection.plock-rows) r.source r.param r.step)"));
    assert_eq!(
        read,
        vec![Value::Number(1.0), s("preview"), Value::Nil, Value::Nil]
    );
    assert!(close(h.eval_panel("r.value"), 800.0));
    assert_eq!(h.eval_panel("selection.plock-variant"), s("B"));
    // A selected step shows its own locks (the tick drops the preview).
    h.shared.selected_steps.lock().unwrap().insert(2);
    h.sync();
    h.eval_panel("(def r (first selection.plock-rows))");
    assert_eq!(h.eval_panel("r.source"), s("step"));
    assert!(close(h.eval_panel("r.value"), 500.0));
    assert_eq!(h.eval_panel("selection.plock-variant"), s("A"));
}

/// eseq-0l17.74: while a print latch holds a param, its value follows the
/// hand (the latched value), not the step the playhead last printed.
#[test]
fn a_param_held_by_the_print_latch_shows_the_latched_value() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    h.eval_panel("(def shown #'cutoff.value) (def printing #'cutoff.printing)");
    h.sync();
    let rest = h.slot("shown");
    let target = PrintTarget::Effect {
        slot_idx: slot,
        param_idx: CUTOFF,
    };
    h.shared.step_print.lock().unwrap().latch(0, target, 321.0);
    h.sync();
    assert_eq!(h.slot("shown"), rest, "only while playing and recording");
    h.set_playing(true);
    h.shared.recording.store(true, Ordering::Relaxed);
    h.sync();
    assert_eq!((h.slot("shown"), h.slot("printing")), (321.0, 1.0));
    h.shared.step_print.lock().unwrap().latch(0, target, 654.0);
    h.sync();
    assert_eq!(h.slot("shown"), 654.0, "it follows every move of the hand");
    h.shared.step_print.lock().unwrap().disarm();
    h.sync();
    assert_eq!((h.slot("shown"), h.slot("printing")), (rest, 0.0));
}

/// eseq-0l17.74 review: the table rebuilds when anything its rows read
/// moves, not only the track's p-lock key: a neuron override edit (history),
/// the song's row mirror (a pattern launch) and a sound binding loan (the
/// DEF column) each move the model revision.
#[test]
fn plock_rows_follow_neuron_edits_and_the_model_revision() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    h.eval_panel(OBSERVE_TABLE);
    h.show_all();
    let param_id = h.filter_slot(slot).param_node_id(CUTOFF);
    let param_id = param_id.expect("a live node identity");
    h.shared
        .state
        .edit_current_neural_networks(|networks| {
            let mut network = sequencer::neural::ProjectNeuralNetwork {
                id: 11,
                name: "router".to_string(),
                num_neurons: 1,
                ..sequencer::neural::ProjectNeuralNetwork::default()
            };
            network.neurons[0].output_overrides.effects =
                vec![sequencer::neural::ProjectEffectParamOverride {
                    target_track: 0,
                    slot_index: slot,
                    param_id,
                    param_index: CUTOFF,
                    value: 1234.0,
                }];
            networks.push(network);
            Ok(())
        })
        .unwrap();
    let neuron = sequencer::lisp_host::SelectedNeuralNeuron {
        pattern_idx: 0,
        network_id: 11,
        neuron_idx: 0,
    };
    h.shared
        .selected_neural_neurons
        .lock()
        .unwrap()
        .insert(neuron);
    h.sync();
    h.eval_panel("(def r (first selection.plock-rows))");
    assert_eq!(
        h.eval_panel("(list r.source r.name)"),
        h.eval_panel(r#"(list "neuron" "N1 cutoff")"#)
    );
    assert!(close(h.eval_panel("r.value"), 1234.0));
    // The knob's edit writes the selected neuron's override.
    let payload = map_value([
        ("slot-idx", Value::Number(slot as f64)),
        ("param-idx", Value::Number(CUTOFF as f64)),
        ("value", Value::Number(2000.0)),
    ]);
    h.command("set-effect-param", payload);
    h.sync();
    assert!(close(h.eval_panel("r.value"), 2000.0));
    h.shared.selected_neural_neurons.lock().unwrap().clear();

    // A lock written with no counter but the song's row mirror moving.
    h.shared.selected_steps.lock().unwrap().insert(4);
    h.sync();
    assert_eq!(
        h.eval_panel("(len selection.plock-rows)"),
        Value::Number(0.0)
    );
    h.filter_slot(slot).set_plock(4, CUTOFF, 700.0);
    let builds = h.frame.host_kinds.plock_rows.builds;
    h.sync();
    assert_eq!(
        h.frame.host_kinds.plock_rows.builds, builds,
        "nothing moved yet"
    );
    h.app.song_row_mirror_epoch += 1;
    h.sync();
    h.eval_panel("(def r (first selection.plock-rows))");
    assert!(close(h.eval_panel("r.value"), 700.0));
    // The base value behind DEF, with only a sound binding loan moving.
    h.filter_slot(slot).defaults.set(CUTOFF, 3000.0);
    h.app.sound_binding_epoch += 1;
    h.sync();
    assert_eq!(h.eval_panel("r.default-text"), s("3000.00"));
}

/// eseq-0l17.74 review: the table is built only while something shows it:
/// a lock drag with the step panel hidden builds nothing, and the first
/// observation builds it.
#[test]
fn plock_rows_build_only_while_observed() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    h.shared.selected_steps.lock().unwrap().insert(2);
    h.sync();
    let builds = h.frame.host_kinds.plock_rows.builds;
    for value in [500.0, 600.0, 700.0] {
        h.lock_effect(slot, 2, CUTOFF, value);
        h.sync();
    }
    assert_eq!(
        h.frame.host_kinds.plock_rows.builds, builds,
        "hidden: no build"
    );
    h.eval_panel(OBSERVE_TABLE);
    h.show_all();
    h.sync();
    assert_eq!(h.frame.host_kinds.plock_rows.builds, builds + 1);
    h.eval_panel("(def r (first selection.plock-rows))");
    assert!(close(h.eval_panel("r.value"), 700.0));
}

/// eseq-0l17.74 review: a drum rack macro's lock row carries its rack macro
/// (display units, `lock-rack-macro!`) even when no rack panel ever showed
/// the macros.
#[test]
fn a_rack_macro_row_carries_its_macro_before_the_rack_panel_shows() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    let command = app::AppCommand::SetRackMacroPlockMulti {
        track: 2,
        steps: vec![3],
        macro_idx: 1,
        value: 0.25,
    };
    app::apply_command(&mut h.app, command);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.shared.selected_steps.lock().unwrap().insert(3);
    h.eval_panel(OBSERVE_TABLE);
    h.show_all();
    h.sync();
    h.eval_panel("(def r (first selection.plock-rows))");
    let read = items(&h.eval_panel("(list r.target r.rack-macro.index r.step.index)"));
    assert_eq!(
        read,
        vec![s("rack-macro"), Value::Number(1.0), Value::Number(3.0)]
    );
    assert!(close(h.eval_panel("r.rack-macro.value"), 0.25));
}

/// The tick samples the modulation display (and holds the modulators on the
/// audio graph's watchlist) only while a kind field reads the sample
/// (docs/kind-bindings-spec.md D3, eseq-0l17.79).
#[test]
fn the_tick_samples_modulation_only_while_a_kind_field_observes_it() {
    let (mut h, _) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    let unpolled = ModDisplayValues {
        instrument: Some(InstrumentModValues {
            track: 99,
            values: Vec::new(),
            slot_phases: [9.0; 4],
        }),
        ..ModDisplayValues::default()
    };
    h.meters.cached_mod_display_values = unpolled.clone();
    h.meters.last_meter_poll_at = Instant::now() - METER_POLL_INTERVAL * 2;
    h.tick();
    h.tick();
    assert_eq!(h.meters.cached_mod_display_values, unpolled, "nothing observed, nothing sampled");
    assert_eq!(h.meters.mod_display_poll_track, None);
    assert!(h.meters.watched_display_modulators.is_empty());
    // Observed: sampled at once (the cadence is not due), for the current
    // track.
    h.eval_panel("(def cutoff-offset #'cutoff.mod-offset) (def start-offset #'start.mod-offset)");
    h.meters.last_meter_poll_at = Instant::now() + Duration::from_secs(3600);
    h.tick();
    h.tick();
    assert_ne!(h.meters.cached_mod_display_values, unpolled, "observed, sampled");
    assert_eq!(h.meters.mod_display_poll_track, Some(0));
    // Released: the sample stops and the watchlist is released.
    h.eval_panel("(set! cutoff-offset nil) (set! start-offset nil)");
    h.tick();
    h.tick();
    assert_eq!(h.meters.mod_display_poll_track, None);
    assert!(h.meters.watched_display_modulators.is_empty());
    h.meters.cached_mod_display_values = unpolled.clone();
    h.meters.last_meter_poll_at = Instant::now() - METER_POLL_INTERVAL * 2;
    h.tick();
    assert_eq!(h.meters.cached_mod_display_values, unpolled, "released, nothing sampled");
}
