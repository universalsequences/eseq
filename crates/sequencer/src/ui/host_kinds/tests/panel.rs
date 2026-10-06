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
                           lock-param! set-tensor-cell! stamp-variant! stamp-key-variant! \
                           selection))";

impl Harness {
    fn eval_panel(&mut self, code: &str) -> Value {
        let source = format!("{REFER_PANEL}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// `code` reports an error containing `expected`.
    fn rejects_7b3(&mut self, code: &str, expected: &str) {
        self.editor.minibuffer = None;
        self.eval_panel(code);
        self.drain();
        assert!(self.error().contains(expected), "{code}: {}", self.error());
    }

    /// Undo the last entry, with the resync the undo command's epoch bump
    /// brings.
    fn undo(&mut self) {
        app::edit::undo(&mut self.app);
        self.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        self.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    }

    fn panel_scans(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().panel_scans
    }

    fn sampler_refreshes(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().sampler_refreshes
    }
}

/// A map's field.
fn get(value: &Value, key: &str) -> Value {
    match value {
        Value::Map(map) => map
            .get(key)
            .map_or(Value::Nil, |cell| cell.borrow().clone()),
        _ => Value::Nil,
    }
}

fn items(value: &Value) -> Vec<Value> {
    match value {
        Value::List(items) => items.iter().map(|item| item.borrow().clone()).collect(),
        _ => Vec::new(),
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
fn param_placement_and_lanes_match_the_sampler_panel() {
    let (mut h, _) = Harness::with_devices();
    h.eval_panel(SAMPLER);
    let panel = build_instrument_panel_value(&h.app, 2, &h.shared.selected_steps);
    let panel = items(&panel).remove(0);
    let fields = |h: &mut Harness, idx: usize| {
        let code = format!(
            "(let ((p (nth inst.params {idx}))) (list p.section p.label p.mod-slot p.visible))"
        );
        items(&h.eval_panel(&code))
    };
    let mut seen = HashSet::new();
    let mut lanes_seen = 0;
    for (list, section) in [("synth", "main"), ("mod", "mod")] {
        for param in items(&get(&panel, list)) {
            let Some(idx) = index_of(&param) else {
                continue; // the base note row
            };
            seen.insert(idx);
            let read = fields(&mut h, idx);
            assert_eq!(read[0], s(section), "param {idx} in {list}");
            assert_eq!(read[1], get(&param, "name"), "param {idx}'s label");
            assert_eq!(read[3], Value::Bool(true));
            // The lanes onto it, as the panel lists them.
            let lanes = items(&get(&param, "mod-targets"));
            let code = format!(
                "(let ((p (nth inst.params {idx}))) \
                 (map (lambda (mt) (list (if mt.source mt.source.index -1) mt.depth.index \
                 mt.depth-min mt.depth-max mt.unit mt.param.index mt.slot)) p.mod-targets))"
            );
            let kinds = items(&h.eval_panel(&code));
            assert_eq!(kinds.len(), lanes.len(), "param {idx}'s lanes");
            lanes_seen += lanes.len();
            for (lane, kind) in lanes.iter().zip(&kinds) {
                let kind = items(kind);
                let source = match get(lane, "source-idx") {
                    Value::Nil => Value::Number(-1.0),
                    source => source,
                };
                assert_eq!(kind[0], source);
                assert_eq!(kind[1], get(lane, "depth-idx"));
                assert_eq!(kind[2], get(lane, "depth-min"));
                assert_eq!(kind[3], get(lane, "depth-max"));
                let unit = get(lane, "depth-unit");
                assert_eq!(kind[4], if unit == Value::Nil { s("") } else { unit });
                assert_eq!(kind[5], Value::Number(idx as f64));
                if get(lane, "source-idx") == Value::Nil {
                    assert_eq!(kind[6], get(lane, "source-slot"));
                }
            }
        }
    }
    assert!(lanes_seen > 0, "the sampler declares modulation lanes");
    // The modulation sources: each slot's type param and the settings its
    // type uses.
    for section in items(&get(&panel, "sources")) {
        let slot = num(get(&section, "slot"));
        let listed = std::iter::once(get(&section, "source-param"))
            .filter(|param| *param != Value::Nil)
            .chain(items(&get(&section, "params")));
        for param in listed {
            let idx = index_of(&param).expect("a source param has an index");
            seen.insert(idx);
            let read = fields(&mut h, idx);
            assert_eq!(read[0], s("source"), "source param {idx}");
            assert_eq!(read[1], get(&param, "name"));
            assert_eq!(read[2], Value::Number(slot));
            assert_eq!(read[3], Value::Bool(true), "source param {idx} shows");
        }
    }
    // Everything else is hidden plumbing or a setting its source's type
    // does not use.
    let count = h.app.graph.instrument_descriptors[2].params.len();
    let mut unseen = 0;
    for idx in (0..count).filter(|idx| !seen.contains(idx)) {
        unseen += 1;
        let read = fields(&mut h, idx);
        let hidden = read[0] == s("hidden");
        let unused_source = read[0] == s("source") && read[3] == Value::Bool(false);
        assert!(hidden || unused_source, "param {idx}: {read:?}");
    }
    assert!(unseen > 0, "the sampler has hidden or unused params");
}

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
    // The legacy instrument field shows the same sample.
    let rt = h.editor.runtime_mut();
    sync_instrument_mod_offset_field_delta(rt, None, sample.instrument.as_ref());
    let legacy = rt.reactive_field_value("SEQ", &instrument_mod_offset_field(2, START));
    assert_eq!(legacy.cloned(), Some(Value::Number(10.0)));
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
    // The legacy panel marks the same param.
    let panel = build_instrument_panel_value(&h.app, 2, &h.shared.selected_steps);
    let synth = items(&get(&items(&panel)[0], "synth"));
    let start = synth.iter().find(|p| index_of(p) == Some(START)).unwrap();
    assert_eq!(get(start, "process-mapped"), Value::Bool(true));
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
    // The legacy field shows it too.
    let rt = h.editor.runtime_mut();
    sync_instrument_base_note_value_field(rt, &h.app, 2);
    let legacy = rt.reactive_field_value("SEQ", &instrument_base_note_value_field(2));
    assert_eq!(legacy.cloned(), Some(Value::Number(12.0)));
    // Absolute: the same value is no new entry.
    h.eval_panel("(set! inst.base-note 12)");
    h.drain_and_sync();
    assert_eq!(h.app.history.undo_len(), before + 1);
    h.undo();
    h.sync();
    assert_eq!(offset(&h), 0.0);
    assert_eq!(h.slot("note"), 0.0);
    h.rejects_7b3("(set! inst.base-note 60)", "from -48 to 48");
    h.rejects_7b3("(set! flt.base-note 3)", "has no base note");
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
    h.rejects_7b3("(set-tensor-cell! mask 4 0.5)", "an integer from 0 to 3");
    h.rejects_7b3("(set-tensor-cell! mask 0 1.5)", "a number from 0 to 1");
    h.rejects_7b3(
        "(set-tensor-cell! (dict :device flt :index 0) 0 0.5)",
        "no such tensor",
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
    h.rejects_7b3(
        "(set-tensor-cell! (nth inst.tensors 1) 0 0.5)",
        "the tensor has no cells",
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
    // Legacy parity (SEQ.track-plock-variants; its "def" chip first).
    let legacy = build_track_plock_variants_value(&h.shared.state, 0, &h.shared.selected_steps);
    let legacy: Vec<Value> = items(&legacy).into_iter().skip(1).collect();
    assert_eq!(legacy.len(), 2);
    for (at, chip) in legacy.iter().enumerate() {
        let code =
            format!("(let ((v (nth chips {at}))) (list v.label v.name v.count v.track v.device))");
        let read = items(&h.eval_panel(&code));
        assert_eq!(read[0], get(chip, "label"));
        assert_eq!(read[1], get(chip, "display"));
        assert_eq!(read[2], get(chip, "count"));
        assert_eq!(read[3], h.eval_panel("t0"));
        assert_eq!(read[4], Value::Nil);
        let color = h.cells(&format!("(let ((v (nth chips {at}))) (rest v.color))"));
        let legacy_color = ["color-r", "color-g", "color-b"].map(|key| num(get(chip, key)));
        assert!(same_cells(&color, &legacy_color), "{color:?}");
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
    h.rejects_7b3(
        r#"(stamp-variant! t0 (list (nth t0.steps 1)) (dict :label "Z"))"#,
        "the track has no variant Z",
    );
    h.rejects_7b3(
        "(let ((t1 (track 1))) (stamp-variant! t0 (list (nth t1.steps 1)) a))",
        "steps must be steps of the track",
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
    h.rejects_7b3(
        "(stamp-key-variant! flt (list 60) kv)",
        "has no key-lock variants",
    );
    h.rejects_7b3(
        "(stamp-key-variant! inst (list 200) kv)",
        "200 is no MIDI note",
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
    // Legacy parity (SEQ.macros).
    let legacy = items(&build_macros_value(&h.app)).remove(0);
    let mapping = items(&get(&legacy, "mappings")).remove(0);
    assert_eq!(h.eval_panel("mm.label"), get(&mapping, "target-label"));
    assert_eq!(h.eval_panel("mm.min"), get(&mapping, "display-min"));
    assert_eq!(h.eval_panel("mm.max"), get(&mapping, "display-max"));
    assert_eq!(h.eval_panel("mm.curve"), get(&mapping, "curve"));
    assert_eq!(h.eval_panel("mm.suspended"), get(&mapping, "suspended"));
    assert_eq!(h.eval_panel("m.value"), get(&legacy, "value"));
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
    h.rejects_7b3("(set! mm.max 99999)", "a number from 20 to 20000");
    h.rejects_7b3(r#"(set! mm.curve "wobbly")"#, "one of");
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
    h.rejects_7b3(r#"(set! m.name "  ")"#, "a non-empty name");
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

#[test]
fn rack_macros_read_set_and_map_onto_the_slot_params() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
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
    h.app.map_rack_macro(2, id, mapping).expect("map the macro");
    h.sync();
    h.eval_panel(
        "(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices)) \
         (def rm (nth rk.macros 1)) (def rmm (first rm.mappings))",
    );
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
    let legacy_value = |h: &mut Harness| {
        let step = selected_plock_step(&h.shared.selected_steps);
        let step = displayed_plock_step(&h.app.state, 2, step);
        let rt = h.editor.runtime_mut();
        sync_rack_macro_value_field(rt, &h.app, 2, id, step);
        let field = rack_macro_value_field(2, 1);
        rt.reactive_field_value("SEQ", &field).cloned()
    };
    assert_eq!(legacy_value(&mut h), Some(Value::Number(0.75)));
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
    assert_eq!(legacy_value(&mut h), Some(Value::Number(0.25)));
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
    h.rejects_7b3("(set! rm.base 2)", "a number from 0 to 1");
    h.rejects_7b3(r#"(set! rmm.curve "wobbly")"#, "one of");
    h.rejects_7b3("(set! rmm.max 300)", "a number from 0 to 100");
    // A bound outside the target's range (an older mapping) sets back as
    // it reads: the no-op comes before the range check.
    assert!(h.app.set_rack_macro_mapping_range(2, id, 0, 0.0, 2.0));
    h.drain_and_sync();
    assert_eq!(h.eval_panel("rmm.max"), Value::Number(200.0));
    h.editor.minibuffer = None;
    h.eval_panel("(set! rmm.max rmm.max)");
    h.drain();
    assert_eq!(h.error(), "", "the current value always works");
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
    // The legacy field shows the same.
    let selection = h.shared.selected_neural_neurons.lock().unwrap().clone();
    let rt = h.editor.runtime_mut();
    let selection = Some(&selection);
    sync_track_effect_param_value_field_with_neural_selection(
        rt, &h.app, 0, slot, CUTOFF, None, selection,
    );
    let field = track_effect_param_value_field(0, slot, CUTOFF, "cutoff");
    let legacy = rt.reactive_field_value("SEQ", &field);
    assert_eq!(legacy.cloned(), Some(Value::Number(1234.0)));
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
