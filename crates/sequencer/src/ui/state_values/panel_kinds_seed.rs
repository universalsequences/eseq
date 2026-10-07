//! The kinds behind the host-less panel harnesses (kind-bindings spec §13
//! stage 8, eseq-0l17.14): [`seed_panel_kinds`] publishes, from the panel
//! dicts a test set, the eseq.kinds instances the factory panels read.

use super::*;
use crate::host_kinds::tests::{get, items};

/// The kinds behind a host-less panel harness (kind-bindings spec §13
/// stage 8, eseq-0l17.14): eseq.effects reads every param value from an
/// eseq.kinds param, so [`seed_panel_kinds`] publishes, from the panel
/// dicts a test set (`SEQ.instrument-panel`, `SEQ.effects`,
/// `SEQ.midi-effects`), the current track, its instrument and effect
/// devices, their params (with their modulation lanes) and tensors, as
/// the host-kinds tick does. `fields` maps each dict's legacy
/// `:value-field` (and a lane's source and depth fields) to the instance
/// that replaced it, so a test drives a value with [`set_panel_param`].
#[derive(Default)]
pub(super) struct PanelKinds {
    pub(super) fields: HashMap<String, eseqlisp::vm::InstanceId>,
    /// A param's legacy modulation and process display fields, by the
    /// param field each became.
    pub(super) display_fields: HashMap<String, (eseqlisp::vm::InstanceId, &'static str)>,
}

/// Map entry `key` of `value`, if set (host_kinds' test `get`).
pub(super) fn dict_value(value: &Value, key: &str) -> Option<Value> {
    Some(get(value, key)).filter(|v| !matches!(v, Value::Nil))
}

/// The items of a list value, none for anything else (host_kinds' test
/// `items`).
pub(super) fn dict_items(value: Option<Value>) -> Vec<Value> {
    value.map(|value| items(&value)).unwrap_or_default()
}

pub(super) fn dict_number(value: &Value, key: &str) -> Option<f64> {
    match dict_value(value, key) {
        Some(Value::Number(n)) => Some(n),
        _ => None,
    }
}

pub(super) fn dict_string(value: &Value, key: &str) -> Option<String> {
    match dict_value(value, key) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

/// What a test published under legacy field `field` (`SEQ.<field>`).
fn published(rt: &Runtime, field: &str) -> Option<Value> {
    rt.reactive_field_value("SEQ", field).cloned()
}

/// What a test published under the legacy field `dict`'s `key` names.
fn published_via(rt: &Runtime, dict: &Value, key: &str) -> Option<Value> {
    published(rt, &dict_string(dict, key)?)
}

/// A published number (a flag as 0 or 1).
fn published_number(value: Option<Value>) -> Option<f64> {
    match value {
        Some(Value::Number(n)) => Some(n),
        Some(Value::Bool(b)) => Some(f64::from(u8::from(b))),
        _ => None,
    }
}

/// Whether a published flag is on (true, or a number over 0.5).
fn truthy(value: Option<Value>) -> bool {
    published_number(value).is_some_and(|n| n > 0.5)
}

/// The instances list value `value` holds.
pub(super) fn instance_ids(value: Value) -> Vec<eseqlisp::vm::InstanceId> {
    match value {
        Value::List(items) => items
            .iter()
            .filter_map(|item| match &*item.borrow() {
                Value::Instance(id) => Some(*id),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The instances instance `id`'s list field `field` holds.
pub(super) fn field_ids(
    rt: &Runtime,
    id: eseqlisp::vm::InstanceId,
    field: &str,
) -> Vec<eseqlisp::vm::InstanceId> {
    instance_ids(rt.instance_field(id, field).unwrap_or(Value::Nil))
}

/// Every param dict of a device dict: its params and synth params, its
/// modulation sources' params, the sampler's mod params.
pub(super) fn device_param_dicts(device: &Value) -> Vec<Value> {
    let mut dicts = Vec::new();
    for key in ["params", "synth", "mod"] {
        dicts.extend(dict_items(dict_value(device, key)));
    }
    for section in dict_items(dict_value(device, "sources")) {
        dicts.extend(dict_items(dict_value(&section, "params")));
        dicts.extend(dict_value(&section, "source-param"));
    }
    dicts
}

/// Register device `did` of `track` (a track or bus instance) and its
/// params from `dicts`.
pub(super) fn seed_panel_device(
    rt: &mut Runtime,
    kinds: &mut PanelKinds,
    track: eseqlisp::vm::InstanceId,
    did: u64,
    slot: f64,
    name: &str,
    dicts: &[Value],
) -> eseqlisp::vm::InstanceId {
    let device = rt
        .register_keyed_instance("eseq.kinds:device", &[track, did])
        .unwrap();
    let parent = if rt
        .instance_kind(track)
        .is_some_and(|kind| kind.ends_with(":bus"))
    {
        "bus"
    } else {
        "track"
    };
    for (field, value) in [
        (parent, Value::Instance(track)),
        ("slot", Value::Number(slot)),
        ("did", Value::Number(did as f64)),
        (
            "role",
            Value::String(if slot < 0.0 { "instrument" } else { "effect" }.into()),
        ),
        ("name", Value::String(name.into())),
        ("type", Value::String(name.into())),
    ] {
        set_field(rt, device, field, value);
    }
    let indexed: Vec<(usize, &Value)> = dicts
        .iter()
        .filter_map(|dict| dict_number(dict, "idx").map(|idx| (idx as usize, dict)))
        .collect();
    let lanes: Vec<Value> = dicts
        .iter()
        .flat_map(|dict| dict_items(dict_value(dict, "mod-targets")))
        .collect();
    let count = indexed
        .iter()
        .map(|(idx, _)| idx + 1)
        .chain(lanes.iter().flat_map(|lane| {
            ["source-idx", "depth-idx"]
                .into_iter()
                .filter_map(|key| dict_number(lane, key).map(|idx| idx as usize + 1))
        }))
        .max()
        .unwrap_or(0);
    let params: Vec<_> = (0..count)
        .map(|index| {
            let param = rt
                .register_keyed_instance("eseq.kinds:param", &[device, index as u64])
                .unwrap();
            set_field(rt, param, "device", Value::Instance(device));
            set_field(rt, param, "index", Value::Number(index as f64));
            param
        })
        .collect();
    // A value the test published under the legacy field wins over the
    // dict's own (the legacy field was what the panels read).
    for &(index, dict) in &indexed {
        let param = params[index];
        let value = published_number(published_via(rt, dict, "value-field"))
            .or_else(|| dict_number(dict, "value"))
            .unwrap_or(0.0);
        for field in ["value", "base", "process-value"] {
            set_field(rt, param, field, Value::Number(value));
        }
        set_mod_value(rt, param, Value::Number(value));
        set_field(rt, param, "mod-scale", Value::Number(1.0));
        for field in ["min", "max"] {
            if let Some(bound) = dict_number(dict, field) {
                set_field(rt, param, field, Value::Number(bound));
            }
        }
        if let Some(name) = dict_string(dict, "name") {
            set_field(rt, param, "name", Value::String(name));
        }
        if let Some(unit) = dict_string(dict, "unit") {
            set_field(rt, param, "unit", Value::String(unit));
        }
        if let Some(text) = dict_string(dict, "text-value") {
            set_field(rt, param, "text", Value::String(text));
        }
        if let Some(field) = dict_string(dict, "value-field") {
            kinds.fields.insert(field, param);
        }
        for (key, kind_field) in [
            ("mod-offset-field", "mod-offset"),
            ("mod-value-field", "mod-value"),
            ("mod-scale-field", "mod-scale"),
            ("process-value-field", "process-value"),
            ("process-clamped-field", "process-clamped"),
        ] {
            if let Some(field) = dict_string(dict, key) {
                kinds.display_fields.insert(field, (param, kind_field));
            }
        }
        if dict_value(dict, "process-mapped") == Some(Value::Bool(true)) {
            set_field(rt, param, "process-mapped", Value::Bool(true));
        }
        let lanes: Vec<_> = dict_items(dict_value(dict, "mod-targets"))
            .iter()
            .enumerate()
            .map(|(lane_index, lane)| {
                let lane_id = rt
                    .register_keyed_instance("eseq.kinds:mod-target", &[param, lane_index as u64])
                    .unwrap();
                set_field(rt, lane_id, "param", Value::Instance(param));
                set_field(rt, lane_id, "index", Value::Number(lane_index as f64));
                let slot = published_number(published_via(rt, lane, "source-value-field"))
                    .or_else(|| dict_number(lane, "source-slot"))
                    .unwrap_or(0.0);
                set_field(rt, lane_id, "slot", Value::Number(slot));
                if let Some(source) = dict_number(lane, "source-idx") {
                    let source = params[source as usize];
                    set_field(rt, lane_id, "source", Value::Instance(source));
                    set_field(rt, source, "value", Value::Number(slot));
                    if let Some(field) = dict_string(lane, "source-value-field") {
                        kinds.fields.insert(field, source);
                    }
                }
                if let Some(depth) = dict_number(lane, "depth-idx") {
                    let depth = params[depth as usize];
                    set_field(rt, lane_id, "depth", Value::Instance(depth));
                    let value = published_number(published_via(rt, lane, "depth-value-field"))
                        .or_else(|| dict_number(lane, "depth"))
                        .unwrap_or(0.0);
                    set_field(rt, depth, "value", Value::Number(value));
                    if let Some(field) = dict_string(lane, "depth-value-field") {
                        kinds.fields.insert(field, depth);
                    }
                }
                for (key, field) in [("depth-min", "depth-min"), ("depth-max", "depth-max")] {
                    if let Some(bound) = dict_number(lane, key) {
                        set_field(rt, lane_id, field, Value::Number(bound));
                    }
                }
                if let Some(unit) = dict_string(lane, "depth-unit") {
                    set_field(rt, lane_id, "unit", Value::String(unit));
                }
                lane_id
            })
            .collect();
        set_field(rt, param, "mod-targets", instance_list(lanes));
    }
    set_field(rt, device, "params", instance_list(params));
    device
}

/// Mapping `index` of macro `owner` (a `macro`, or with `rack` a
/// `rack-macro`): its `macro-mapping`, keyed (owner, index), with its owner
/// and index set.
pub(super) fn seed_mapping_row(
    rt: &mut Runtime,
    owner: eseqlisp::vm::InstanceId,
    rack: bool,
    index: usize,
) -> eseqlisp::vm::InstanceId {
    let row = rt
        .register_keyed_instance("eseq.kinds:macro-mapping", &[owner, index as u64])
        .unwrap();
    let owner_field = if rack { "rack-macro" } else { "macro" };
    set_field(rt, row, owner_field, Value::Instance(owner));
    set_field(rt, row, "index", Value::Number(index as f64));
    row
}

/// A rack panel dict's macros (`:macros`, their `:mappings` onto the
/// slots' instrument and effect params) as the rack device's
/// rack-macros.
pub(super) fn seed_panel_rack_macros(
    rt: &mut Runtime,
    kinds: &mut PanelKinds,
    inst: &Value,
    rack: eseqlisp::vm::InstanceId,
    slots: &[eseqlisp::vm::InstanceId],
) {
    let macros: Vec<_> = dict_items(dict_value(inst, "macros"))
        .iter()
        .map(|rack_macro| {
            let index = dict_number(rack_macro, "id").unwrap_or(0.0);
            let id = rt
                .register_keyed_instance("eseq.kinds:rack-macro", &[rack, index as u64])
                .unwrap();
            set_field(rt, id, "device", Value::Instance(rack));
            set_field(rt, id, "index", Value::Number(index));
            // Name, value and lock state: the dict's (a value the test
            // published under the legacy field wins), the legacy fields
            // mapped to the rack macro fields that replaced them.
            if let Some(name) = dict_string(rack_macro, "name") {
                set_field(rt, id, "name", Value::String(name));
            }
            let value = match published_via(rt, rack_macro, "value-field") {
                Some(Value::Number(n)) => n,
                _ => dict_number(rack_macro, "value").unwrap_or(0.0),
            };
            let locked = matches!(
                published_via(rt, rack_macro, "plock-active-field"),
                Some(Value::Number(n)) if n > 0.5
            );
            let base = match published_via(rt, rack_macro, "plock-default-field") {
                Some(Value::Number(n)) if locked => n,
                _ => value,
            };
            for (field, value) in [
                ("value", Value::Number(value)),
                ("base", Value::Number(base)),
                ("locked", Value::Bool(locked)),
                ("has-locks", Value::Bool(false)),
            ] {
                set_field(rt, id, field, value);
            }
            for (key, kind_field) in [
                ("value-field", "value"),
                ("plock-active-field", "locked"),
                ("plock-default-field", "base"),
            ] {
                if let Some(field) = dict_string(rack_macro, key) {
                    kinds.display_fields.insert(field, (id, kind_field));
                }
            }
            let mappings: Vec<_> = dict_items(dict_value(rack_macro, "mappings"))
                .iter()
                .enumerate()
                .map(|(position, mapping)| {
                    let mapping_id = seed_mapping_row(rt, id, true, position);
                    let slot = dict_number(mapping, "rack-slot")
                        .and_then(|slot| slots.get(slot as usize).copied());
                    let device = match dict_string(mapping, "kind").as_deref() {
                        Some("rack-slot-instrument") => slot,
                        Some("rack-slot-effect") => slot.and_then(|slot| {
                            let effect_slot = dict_number(mapping, "effect-slot")?;
                            field_ids(rt, slot, "devices").into_iter().find(|&effect| {
                                rt.instance_field(effect, "slot") == Ok(Value::Number(effect_slot))
                            })
                        }),
                        _ => None,
                    };
                    let target = device.and_then(|device| {
                        let idx = dict_number(mapping, "param-idx")? as usize;
                        field_ids(rt, device, "params").get(idx).copied()
                    });
                    if let Some(target) = target {
                        set_field(rt, mapping_id, "target", Value::Instance(target));
                    }
                    mapping_id
                })
                .collect();
            set_field(rt, id, "mappings", instance_list(mappings));
            id
        })
        .collect();
    set_field(rt, rack, "macros", instance_list(macros));
}

/// A rack panel slot dict's strip (gain, pan, base note, voices, mute,
/// solo, the delete-target highlight) as its slot device's display fields
/// (a value the test published under the dict's legacy field wins), the
/// legacy fields mapped to the device fields that replaced them.
fn seed_rack_slot_strip(
    rt: &mut Runtime,
    kinds: &mut PanelKinds,
    slot: &Value,
    device: eseqlisp::vm::InstanceId,
) {
    for (key, field_key, kind_field) in [
        ("gain", "gain-field", "gain-display"),
        ("pan", "pan-field", "pan-display"),
        ("base-note", "base-note-field", "base-note-display"),
        ("max-polyphony", "max-polyphony-field", "voices-display"),
    ] {
        let value = match published_via(rt, slot, field_key) {
            Some(Value::Number(n)) => Some(n),
            _ => dict_number(slot, key),
        };
        if let Some(value) = value {
            set_field(rt, device, kind_field, Value::Number(value));
        }
        if let Some(field) = dict_string(slot, field_key) {
            kinds.display_fields.insert(field, (device, kind_field));
        }
    }
    if let Some(voices) = dict_number(slot, "max-polyphony") {
        set_field(rt, device, "voices", Value::Number(voices));
    }
    for (key, field_key, kind_field) in [
        ("mute", "mute-field", "muted-display"),
        ("solo", "solo-field", "soloed-display"),
    ] {
        let on = match published_number(published_via(rt, slot, field_key)) {
            Some(n) => n > 0.5,
            None => truthy(dict_value(slot, key)),
        };
        set_field(rt, device, kind_field, Value::Bool(on));
        if let Some(field) = dict_string(slot, field_key) {
            kinds.display_fields.insert(field, (device, kind_field));
        }
    }
    let (track, idx) = (dict_number(slot, "track"), dict_number(slot, "idx"));
    if let (Some(track), Some(idx)) = (track, idx) {
        let field = rack_slot_delete_target_field(track as usize, idx as usize);
        let target = truthy(published(rt, &field));
        set_field(rt, device, "delete-target", Value::Bool(target));
        kinds
            .display_fields
            .insert(field, (device, "delete-target"));
    }
}

/// The rack panel's former delete-target field of slot `slot` of `track`
/// (now `device.delete-target`): a test seeds it as a legacy field
/// ([`seed_rack_slot_strip`] maps it).
pub(super) fn rack_slot_delete_target_field(track: usize, slot: usize) -> String {
    format!("rack-slot-delete-target-{track}-{slot}")
}

/// The legacy p-lock lists a test published (`SEQ.track-plocks`, the
/// selected step's lock rows; `SEQ.track-plock-any`, the params locked on
/// any step; `SEQ.track-plock-variants`, the variant chips) as the
/// seeded params' `locked` / `base` / `value` / `has-locks` and the
/// track's variants.
pub(super) fn seed_panel_locks(rt: &mut Runtime, track: eseqlisp::vm::InstanceId) {
    let row_param = |rt: &Runtime, row: &Value| -> Option<eseqlisp::vm::InstanceId> {
        let target = dict_string(row, "target")?;
        let idx = dict_number(row, "param-idx")? as usize;
        let (list, slot) = match target.as_str() {
            "instrument" => ("devices", -1.0),
            "effect" => ("devices", dict_number(row, "slot-idx")?),
            "midi-fx" => ("midi-devices", dict_number(row, "slot-idx")?),
            _ => return None,
        };
        let device = field_ids(rt, track, list)
            .into_iter()
            .find(|&device| rt.instance_field(device, "slot") == Ok(Value::Number(slot)))?;
        field_ids(rt, device, "params").get(idx).copied()
    };
    // The lists say which params are locked: every other one is not.
    let devices: Vec<_> = ["devices", "midi-devices"]
        .into_iter()
        .flat_map(|list| field_ids(rt, track, list))
        .collect();
    for device in devices {
        for param in field_ids(rt, device, "params") {
            set_field(rt, param, "locked", Value::Bool(false));
            set_field(rt, param, "has-locks", Value::Bool(false));
        }
    }
    for row in dict_items(published(rt, "track-plocks")) {
        if let Some(param) = row_param(rt, &row) {
            set_field(rt, param, "locked", Value::Bool(true));
            if let Some(base) = dict_number(&row, "default") {
                set_field(rt, param, "base", Value::Number(base));
            }
            if let Some(value) = dict_number(&row, "value") {
                set_field(rt, param, "value", Value::Number(value));
            }
        }
    }
    for row in dict_items(published(rt, "track-plock-any")) {
        if let Some(param) = row_param(rt, &row) {
            set_field(rt, param, "has-locks", Value::Bool(true));
        }
    }
    seed_rack_locks(
        rt,
        track,
        &dict_items(published(rt, "track-plocks")),
        &dict_items(published(rt, "track-plock-any")),
    );
    // Track-level locks (no param index) at the selected step:
    // track.setting-locks.
    let settings: Vec<_> = dict_items(published(rt, "track-plocks"))
        .iter()
        .filter(|row| dict_value(row, "param-idx").is_none())
        .filter_map(|row| {
            let name = dict_string(row, "target")?;
            // The row's lock value, else what the legacy display field the
            // test published shows, else the default.
            let value = (dict_value(row, "text-value").or_else(|| dict_value(row, "value")))
                .or_else(|| {
                    rt.reactive_field_value("SEQ", &format!("tp-{name}"))
                        .cloned()
                })
                .or_else(|| dict_value(row, "default"))?;
            Some(map_value([("name", Value::String(name)), ("value", value)]))
        })
        .collect();
    set_field(rt, track, "setting-locks", test_list(settings));
    let variants: Vec<_> = dict_items(published(rt, "track-plock-variants"))
        .iter()
        .filter(|chip| dict_string(chip, "kind").as_deref() == Some("variant"))
        .enumerate()
        .map(|(vid, chip)| {
            let id = rt
                .register_keyed_instance("eseq.kinds:variant", &[track, vid as u64])
                .unwrap();
            let color =
                ["color-r", "color-g", "color-b"].map(|key| dict_number(chip, key).unwrap_or(0.0));
            set_field(rt, id, "track", Value::Instance(track));
            set_field(
                rt,
                id,
                "label",
                Value::String(dict_string(chip, "label").unwrap_or_default()),
            );
            set_field(rt, id, "color", test_rgb(color));
            set_field(
                rt,
                id,
                "current",
                Value::Bool(dict_value(chip, "current") == Some(Value::Bool(true))),
            );
            id
        })
        .collect();
    if !variants.is_empty() {
        set_field(rt, track, "variants", instance_list(variants));
    }
}

/// The current track's settings a test published under their legacy
/// fields (`SEQ.tp-*`, `fts-options`) as the track's (and the project's)
/// fields, the legacy fields mapped to the ones that replaced them.
fn seed_track_settings(rt: &mut Runtime, kinds: &mut PanelKinds, track: eseqlisp::vm::InstanceId) {
    for (legacy, field) in [
        ("tp-poly", "poly"),
        ("tp-gate", "gate"),
        ("tp-supports-mono-trigger", "supports-mono-trigger"),
        ("tp-max-polyphony", "max-polyphony"),
        ("tp-num-steps", "num-steps"),
        ("tp-swing", "swing"),
        ("tp-voice-priority", "voice-priority"),
        ("tp-mono-trigger", "mono-trigger"),
        ("tp-swing-resolution", "swing-resolution"),
        ("tp-timebase", "timebase"),
        ("tp-fts", "fts"),
        ("tp-accumulator", "accumulator"),
        ("tp-accum-mode", "accum-mode"),
        ("tp-accum-limit", "accum-limit"),
    ] {
        // Only a value of the field's type (a test may publish a legacy
        // placeholder, a 0 for a label).
        let value = match (field, published(rt, legacy)) {
            ("poly" | "gate" | "supports-mono-trigger", Some(Value::Number(n))) => {
                Some(Value::Bool(n > 0.5))
            }
            ("poly" | "gate" | "supports-mono-trigger", Some(Value::Bool(b))) => {
                Some(Value::Bool(b))
            }
            ("max-polyphony" | "num-steps" | "swing" | "accum-limit", Some(Value::Number(n))) => {
                Some(Value::Number(n))
            }
            (
                "voice-priority" | "mono-trigger" | "swing-resolution" | "timebase" | "fts"
                | "accumulator" | "accum-mode",
                Some(Value::String(label)),
            ) => Some(Value::String(label)),
            _ => None,
        };
        if let Some(value) = value {
            set_field(rt, track, field, value);
        }
        kinds
            .display_fields
            .insert(legacy.to_string(), (track, field));
    }
    if let Some(Value::String(label)) = published(rt, "tp-mute-group") {
        let group = label.parse::<f64>().unwrap_or(0.0);
        set_field(rt, track, "mute-group", Value::Number(group));
    }
    if let Some(options) = published(rt, "fts-options") {
        let project = kind_singleton_rt(rt, "project");
        set_field(rt, project, "fts-options", options);
    }
    seed_track_tuning(rt, track);
    // A drum rack edits its selected slot's voices (`tp-is-rack`, its slot
    // `tp-rack-slot-idx`, the voices `tp-max-polyphony`).
    if published(rt, "tp-is-rack") == Some(Value::Bool(true)) {
        let slot = match published(rt, "tp-rack-slot-idx") {
            Some(Value::Number(n)) => n,
            _ => 0.0,
        };
        set_field(rt, track, "rack", Value::Bool(true));
        let selection = kind_singleton_rt(rt, "selection");
        set_field(rt, selection, "rack-slot", Value::Number(slot));
    }
}

/// The published legacy scale fields (`SEQ.tp-tuning-*`) as the track's
/// tuning and its degrees.
fn seed_track_tuning(rt: &mut Runtime, track: eseqlisp::vm::InstanceId) {
    let Some(Value::Bool(on)) = published(rt, "tp-tuning-on") else {
        return;
    };
    let tuning = rt
        .register_keyed_instance("eseq.kinds:tuning", &[track, 0])
        .unwrap();
    set_field(rt, tuning, "track", Value::Instance(track));
    set_field(rt, tuning, "on", Value::Bool(on));
    for (legacy, field) in [("tp-tuning-root", "root"), ("tp-tuning-mode", "mode")] {
        if let Some(Value::String(label)) = published(rt, legacy) {
            set_field(rt, tuning, field, Value::String(label));
        }
    }
    if let Some(Value::Number(percent)) = published(rt, "tp-tuning-morph") {
        set_field(rt, tuning, "morph", Value::Number(percent / 100.0));
    }
    if let Some(Value::Number(period)) = published(rt, "tp-tuning-period") {
        set_field(rt, tuning, "period", Value::Number(period));
    }
    let column = |rt: &Runtime, field: &str| dict_items(published(rt, field));
    let bases = column(rt, "tp-tuning-base");
    let offsets = column(rt, "tp-tuning-offsets");
    let pitches = column(rt, "tp-tuning-pitches");
    let enabled = column(rt, "tp-tuning-enabled");
    let labels = column(rt, "tp-tuning-labels");
    let degrees: Vec<_> = (0..bases.len())
        .map(|index| {
            let degree = rt
                .register_keyed_instance("eseq.kinds:degree", &[tuning, index as u64])
                .unwrap();
            set_field(rt, degree, "tuning", Value::Instance(tuning));
            set_field(rt, degree, "index", Value::Number(index as f64));
            for (field, values) in [
                ("base", &bases),
                ("offset", &offsets),
                ("pitch", &pitches),
                ("enabled", &enabled),
                ("label", &labels),
            ] {
                if let Some(value) = values.get(index) {
                    set_field(rt, degree, field, value.clone());
                }
            }
            degree
        })
        .collect();
    set_field(rt, tuning, "degrees", instance_list(degrees));
    set_field(rt, track, "tuning", Value::Instance(tuning));
}

/// An instrument dict's sampler media (`:buffer`, `:duration`, `:slices`,
/// `:slice-active`, the selection times; a test's published
/// `sampler-playhead` and selection time fields win) as the sampler
/// device's media fields, the legacy fields mapped to them; a modulator's
/// `:phase-field` / `:level-field` too.
fn seed_device_media(
    rt: &mut Runtime,
    kinds: &mut PanelKinds,
    inst: &Value,
    device: eseqlisp::vm::InstanceId,
) {
    if let Some(buffer) = dict_value(inst, "buffer") {
        set_field(rt, device, "sample-buffer", buffer);
    }
    for (key, field) in [
        ("duration", "sample-duration"),
        ("start-time", "start-time"),
        ("end-time", "end-time"),
    ] {
        if let Some(value) = dict_number(inst, key) {
            set_field(rt, device, field, Value::Number(value));
        }
    }
    for key in ["slices", "slice-active"] {
        if let Some(list) = dict_value(inst, key) {
            set_field(rt, device, key, list);
        }
    }
    for (key, field) in [
        ("start-time-field", "start-time"),
        ("end-time-field", "end-time"),
        ("phase-field", "modulator-phase"),
        ("level-field", "modulator-level"),
    ] {
        if let Some(Value::Number(value)) = published_via(rt, inst, key) {
            set_field(rt, device, field, Value::Number(value));
        }
        if let Some(legacy) = dict_string(inst, key) {
            kinds.display_fields.insert(legacy, (device, field));
        }
    }
    if let Some(Value::Number(playhead)) = published(rt, "sampler-playhead") {
        set_field(rt, device, "playhead", Value::Number(playhead));
    }
    kinds
        .display_fields
        .insert("sampler-playhead".to_string(), (device, "playhead"));
}

/// An effect dict's tables (a Filter Table's `:table-*`, a Convolution
/// Reverb's `:ir-name`) as its device's fields, and its response editor
/// session (`:editor`) as the table-editor singleton's.
fn seed_effect_tables(rt: &mut Runtime, fx: &Value, device: eseqlisp::vm::InstanceId) {
    for key in [
        "table-name",
        "table-mode",
        "table-engine",
        "table-data-key",
        "ir-name",
    ] {
        if let Some(Value::String(text)) = dict_value(fx, key) {
            set_field(rt, device, key, Value::String(text));
        }
    }
    if let Some(options) = dict_value(fx, "table-options") {
        set_field(rt, device, "table-options", options);
    }
    let Some(editor) = dict_value(fx, "editor") else {
        return;
    };
    let session = kind_singleton_rt(rt, "table-editor");
    set_field(rt, session, "device", Value::Instance(device));
    set_field(rt, session, "open", Value::Bool(true));
    for key in [
        "frames",
        "selected-frame",
        "selected-frame-normalized",
        "op-count",
    ] {
        if let Some(value) = dict_number(&editor, key) {
            set_field(rt, session, key, Value::Number(value));
        }
    }
    for key in ["can-undo", "can-redo", "dirty"] {
        let on = dict_value(&editor, key) == Some(Value::Bool(true));
        set_field(rt, session, key, Value::Bool(on));
    }
    if let Some(band) = dict_value(&editor, "band") {
        if let Some(kind) = dict_string(&band, "kind") {
            set_field(rt, session, "band-kind", Value::String(kind));
        }
        for (key, field) in [
            ("freq", "band-freq"),
            ("gain", "band-gain"),
            ("q", "band-q"),
        ] {
            if let Some(value) = dict_number(&band, key) {
                set_field(rt, session, field, Value::Number(value));
            }
        }
    }
}

/// The drum rack rows of the legacy p-lock lists (`rack-macro`,
/// `rack-slot-param`) as the rack's macros' `locked` / `has-locks` and its
/// slots' `strip-locks`.
fn seed_rack_locks(
    rt: &mut Runtime,
    track: eseqlisp::vm::InstanceId,
    plocks: &[Value],
    any: &[Value],
) {
    let Some(rack) = field_ids(rt, track, "devices")
        .into_iter()
        .find(|&device| rt.instance_field(device, "slot") == Ok(Value::Number(-1.0)))
    else {
        return;
    };
    let rows = |list: &[Value], target: &str| -> Vec<(usize, usize)> {
        list.iter()
            .filter(|row| dict_string(row, "target").as_deref() == Some(target))
            .filter_map(|row| {
                let idx = dict_number(row, "param-idx")? as usize;
                let slot = dict_number(row, "slot-idx").unwrap_or(0.0) as usize;
                Some((slot, idx))
            })
            .collect()
    };
    let macros = field_ids(rt, rack, "macros");
    for (_, idx) in rows(plocks, "rack-macro") {
        if let Some(&rm) = macros.get(idx) {
            set_field(rt, rm, "locked", Value::Bool(true));
        }
    }
    for (_, idx) in rows(any, "rack-macro") {
        if let Some(&rm) = macros.get(idx) {
            set_field(rt, rm, "has-locks", Value::Bool(true));
        }
    }
    let slots = field_ids(rt, rack, "devices");
    for (position, &slot) in slots.iter().enumerate() {
        let names: Vec<_> = rows(any, "rack-slot-param")
            .into_iter()
            .filter(|(at, _)| *at == position)
            .filter_map(|(_, idx)| sequencer::sequencer::RackSlotParam::ALL.get(idx))
            .map(|param| Value::String(param.name().to_string()))
            .collect();
        set_field(rt, slot, "strip-locks", test_list(names));
    }
}

/// An instrument dict's key locks (`:key-locks` per param, the locked
/// notes, the key-lock variants and the notes stamped with them) as the
/// instrument device's and its params' fields, and its track's sounding
/// notes (`:active-notes`).
pub(super) fn seed_panel_key_locks(
    rt: &mut Runtime,
    inst: &Value,
    track: eseqlisp::vm::InstanceId,
    device: eseqlisp::vm::InstanceId,
) {
    let Ok(params @ Value::List(_)) = rt.instance_field(device, "params") else {
        return;
    };
    let params = instance_ids(params);
    for (index, rows) in dict_items(dict_value(inst, "key-locks")).iter().enumerate() {
        let rows: Vec<_> = dict_items(Some(rows.clone()))
            .iter()
            .map(|row| {
                test_list(vec![
                    Value::Number(dict_number(row, "note").unwrap_or(0.0)),
                    Value::Number(dict_number(row, "value").unwrap_or(0.0)),
                ])
            })
            .collect();
        if let Some(&param) = params.get(index) {
            set_field(rt, param, "key-locks", test_list(rows));
        }
    }
    set_field(
        rt,
        device,
        "key-locked-notes",
        dict_value(inst, "key-locked-notes").unwrap_or_else(|| test_list(vec![])),
    );
    let stamped = dict_items(dict_value(inst, "key-lock-note-variants"));
    let variants: Vec<_> = dict_items(dict_value(inst, "key-lock-variants"))
        .iter()
        .filter(|chip| dict_string(chip, "kind").as_deref() == Some("variant"))
        .enumerate()
        .map(|(vid, chip)| {
            let label = dict_string(chip, "label").unwrap_or_default();
            let id = rt
                .register_keyed_instance("eseq.kinds:variant", &[device, vid as u64])
                .unwrap();
            let color =
                ["color-r", "color-g", "color-b"].map(|key| dict_number(chip, key).unwrap_or(0.0));
            let notes: Vec<_> = stamped
                .iter()
                .filter(|row| dict_string(row, "label").as_deref() == Some(label.as_str()))
                .filter_map(|row| dict_number(row, "note").map(Value::Number))
                .collect();
            for (field, value) in [
                ("track", Value::Instance(track)),
                ("device", Value::Instance(device)),
                (
                    "name",
                    Value::String(dict_string(chip, "display").unwrap_or(label.clone())),
                ),
                ("label", Value::String(label)),
                ("color", test_rgb(color)),
                ("notes", test_list(notes)),
            ] {
                set_field(rt, id, field, value);
            }
            id
        })
        .collect();
    set_field(rt, device, "variants", instance_list(variants));
    // The notes sounding on the track (`:active-notes`, a harness key of
    // the dict) as `track.active-notes` rows.
    let active = dict_items(dict_value(inst, "active-notes"));
    if !active.is_empty() {
        let rows = active
            .into_iter()
            .map(|note| test_list(vec![note, Value::Number(1.0), Value::Number(0.0)]))
            .collect();
        set_field(rt, track, "active-notes", test_list(rows));
    }
}

pub(super) fn seed_panel_kinds(editor: &mut Editor) -> PanelKinds {
    let mut kinds = PanelKinds::default();
    let rt = editor.runtime_mut();
    if !has_host_kinds(rt) {
        return kinds;
    }
    let current = match rt.reactive_field_value("SEQ", "current-track") {
        Some(Value::Number(n)) => *n as usize,
        _ => 0,
    };
    // The harness's own tracks when it publishes them (seed_kind_tracks).
    let mut tracks = project_list(rt, "tracks");
    if tracks.len() <= current {
        tracks = register_kind_range(rt, "eseq.kinds:track", current + 1);
        for (index, &track) in tracks.iter().enumerate() {
            set_field(rt, track, "index", Value::Number(index as f64));
            set_field(rt, track, "tid", Value::Number(index as f64));
            set_field(rt, track, "selected", Value::Bool(index == current));
        }
        let project = kind_singleton_rt(rt, "project");
        set_field(rt, project, "tracks", instance_list(tracks.iter().copied()));
    }
    let selection = kind_singleton_rt(rt, "selection");
    let track = tracks[current];
    set_field(rt, selection, "track", Value::Instance(track));
    let mut devices = Vec::new();
    if let Some(inst) = dict_items(published(rt, "instrument-panel"))
        .into_iter()
        .next()
        .filter(|inst| dict_value(inst, "slots").is_some())
    {
        // A drum rack: its slots, the selected slot's instrument and
        // every slot's effects, and its macros.
        let name = dict_string(&inst, "name").unwrap_or_default();
        let rack = seed_panel_device(rt, &mut kinds, track, 0, -1.0, &name, &[]);
        let selected = dict_value(&inst, "selected-instrument");
        let slots: Vec<_> = dict_items(dict_value(&inst, "slots"))
            .iter()
            .map(|slot| {
                let idx = dict_number(slot, "idx").unwrap_or(0.0);
                let selected = selected
                    .as_ref()
                    .filter(|selected| dict_number(selected, "rack-slot") == Some(idx));
                let dicts = selected.map(device_param_dicts).unwrap_or_default();
                let slot_name = dict_string(slot, "name").unwrap_or_default();
                let device = seed_panel_device(
                    rt,
                    &mut kinds,
                    track,
                    3000 + idx as u64,
                    idx,
                    &slot_name,
                    &dicts,
                );
                set_field(rt, device, "container", Value::Instance(rack));
                if let Some(selected) = selected {
                    seed_device_media(rt, &mut kinds, selected, device);
                }
                let effects: Vec<_> = dict_items(dict_value(slot, "effects"))
                    .iter()
                    .map(|fx| {
                        let effect_slot = dict_number(fx, "slot-idx").unwrap_or(0.0);
                        let fx_name = dict_string(fx, "name").unwrap_or_default();
                        let effect = seed_panel_device(
                            rt,
                            &mut kinds,
                            track,
                            4000 + (idx as u64) * 100 + effect_slot as u64,
                            effect_slot,
                            &fx_name,
                            &device_param_dicts(fx),
                        );
                        set_field(rt, effect, "container", Value::Instance(device));
                        seed_effect_tables(rt, fx, effect);
                        effect
                    })
                    .collect();
                set_field(rt, device, "devices", instance_list(effects));
                device
            })
            .collect();
        set_field(rt, rack, "devices", instance_list(slots.iter().copied()));
        for (slot, &device) in dict_items(dict_value(&inst, "slots")).iter().zip(&slots) {
            seed_rack_slot_strip(rt, &mut kinds, slot, device);
        }
        seed_panel_rack_macros(rt, &mut kinds, &inst, rack, &slots);
        devices.push(rack);
    } else if let Some(inst) = dict_items(published(rt, "instrument-panel"))
        .into_iter()
        .next()
    {
        let name = dict_string(&inst, "name").unwrap_or_default();
        let dicts = device_param_dicts(&inst);
        let device = seed_panel_device(rt, &mut kinds, track, 0, -1.0, &name, &dicts);
        let tensors: Vec<_> = dict_items(dict_value(&inst, "tensors"))
            .iter()
            .map(|tensor| {
                let index = dict_number(tensor, "idx").unwrap_or(0.0) as u64;
                let id = rt
                    .register_keyed_instance("eseq.kinds:tensor", &[device, index])
                    .unwrap();
                let cells = dict_number(tensor, "rows").unwrap_or(0.0)
                    * dict_number(tensor, "cols").unwrap_or(0.0);
                let zeros = vec![Value::Number(0.0); cells as usize];
                set_field(rt, id, "device", Value::Instance(device));
                set_field(rt, id, "index", Value::Number(index as f64));
                set_field(rt, id, "values", test_list(zeros.clone()));
                set_field(rt, id, "base", test_list(zeros));
                if let Some(field) = dict_string(tensor, "value-field") {
                    kinds.fields.insert(field, id);
                }
                id
            })
            .collect();
        set_field(rt, device, "tensors", instance_list(tensors));
        seed_panel_key_locks(rt, &inst, track, device);
        seed_device_media(rt, &mut kinds, &inst, device);
        devices.push(device);
    }
    for (position, fx) in dict_items(published(rt, "effects")).iter().enumerate() {
        let slot = dict_number(fx, "slot-idx").unwrap_or(position as f64);
        let name = dict_string(fx, "name").unwrap_or_default();
        let dicts = device_param_dicts(fx);
        let device = seed_panel_device(
            rt,
            &mut kinds,
            track,
            1000 + slot as u64,
            slot,
            &name,
            &dicts,
        );
        seed_effect_tables(rt, fx, device);
        devices.push(device);
    }
    set_field(rt, track, "devices", instance_list(devices));
    let midi: Vec<_> = dict_items(published(rt, "midi-effects"))
        .iter()
        .enumerate()
        .map(|(position, fx)| {
            let slot = dict_number(fx, "slot-idx").unwrap_or(position as f64);
            let name = dict_string(fx, "name").unwrap_or_default();
            let dicts = device_param_dicts(fx);
            seed_panel_device(
                rt,
                &mut kinds,
                track,
                2000 + slot as u64,
                slot,
                &name,
                &dicts,
            )
        })
        .collect();
    set_field(rt, track, "midi-devices", instance_list(midi));
    // Each bus's effects (SEQ.bus-effects: one list per bus), on the
    // harness's own buses when it publishes them.
    let bus_effects = dict_items(published(rt, "bus-effects"));
    if bus_effects
        .iter()
        .any(|effects| !dict_items(Some(effects.clone())).is_empty())
    {
        let mut buses = project_list(rt, "buses");
        if buses.len() < bus_effects.len() {
            buses = register_kind_range(rt, "eseq.kinds:bus", bus_effects.len());
            for (index, &bus) in buses.iter().enumerate() {
                set_field(rt, bus, "index", Value::Number(index as f64));
                set_field(rt, bus, "bid", Value::Number(index as f64));
            }
            let project = kind_singleton_rt(rt, "project");
            set_field(rt, project, "buses", instance_list(buses.iter().copied()));
        }
        for (bus, effects) in buses.iter().zip(&bus_effects) {
            let devices: Vec<_> = dict_items(Some(effects.clone()))
                .iter()
                .enumerate()
                .map(|(position, fx)| {
                    let slot = dict_number(fx, "slot-idx").unwrap_or(position as f64);
                    let name = dict_string(fx, "name").unwrap_or_default();
                    let device = seed_panel_device(
                        rt,
                        &mut kinds,
                        *bus,
                        5000 + slot as u64,
                        slot,
                        &name,
                        &device_param_dicts(fx),
                    );
                    seed_effect_tables(rt, fx, device);
                    device
                })
                .collect();
            set_field(rt, *bus, "devices", instance_list(devices));
        }
    }
    seed_track_settings(rt, &mut kinds, track);
    seed_panel_locks(rt, track);
    rt.run_reactive_cycle();
    SEEDED_PANEL_FIELDS.with(|fields| {
        let mut fields = fields.borrow_mut();
        for (name, &id) in &kinds.fields {
            fields.insert((id, "value".to_string()), name.clone());
            fields.insert((id, "values".to_string()), name.clone());
        }
        for (name, &(id, kind_field)) in &kinds.display_fields {
            fields.insert((id, kind_field.to_string()), name.clone());
        }
    });
    SEEDED_DISPLAY_FIELDS.with(|fields| {
        let mut fields = fields.borrow_mut();
        for (name, &(id, kind_field)) in &kinds.display_fields {
            fields.insert(name.clone(), (id, kind_field));
        }
        for (name, &id) in &kinds.fields {
            fields.insert(name.clone(), (id, "value"));
        }
    });
    kinds
}

/// The legacy value field a control's binding stands for: a seeded
/// param's (or tensor's) `#'` binding names the `:value-field` it
/// replaced ([`seed_panel_kinds`]); a legacy ref names its own field.
pub(super) fn bound_field(value: Option<&Value>) -> Option<String> {
    let Some(Value::ReactiveRef {
        namespace, field, ..
    }) = value
    else {
        return None;
    };
    match namespace.strip_prefix("%instance/") {
        Some(id) => {
            let id: eseqlisp::vm::InstanceId = id.parse().ok()?;
            SEEDED_PANEL_FIELDS.with(|fields| fields.borrow().get(&(id, field.clone())).cloned())
        }
        None => Some(field.clone()),
    }
}

thread_local! {
    /// [`seed_panel_kinds`]'s instances by the legacy field each
    /// replaced (the test's own thread: nextest runs one per process).
    static SEEDED_PANEL_FIELDS: RefCell<HashMap<(eseqlisp::vm::InstanceId, String), String>> =
        RefCell::new(HashMap::new());
    /// Every legacy field [`seed_panel_kinds`] replaced, by the instance
    /// field that replaced it.
    static SEEDED_DISPLAY_FIELDS: RefCell<HashMap<String, (eseqlisp::vm::InstanceId, &'static str)>> =
        RefCell::new(HashMap::new());
}

/// Push the instance field that replaced legacy field `field` (a value,
/// modulation or process display field; [`seed_panel_kinds`]), as the
/// host-kinds tick does. A value sets the param's base too.
/// A field nothing seeded (set before the panels were seeded) is still
/// the test's own `SEQ` value, which seeding reads from the dicts.
pub(super) fn set_seeded_field(editor: &mut Editor, field: &str, value: Value) {
    let Some((id, kind_field)) =
        SEEDED_DISPLAY_FIELDS.with(|fields| fields.borrow().get(field).copied())
    else {
        editor.runtime_mut().set_reactive("SEQ", field, value);
        return;
    };
    let rt = editor.runtime_mut();
    let value = match (kind_field, value) {
        (
            "process-clamped" | "locked" | "muted-display" | "soloed-display" | "delete-target",
            Value::Number(n),
        ) => Value::Bool(n > 0.5),
        (_, value) => value,
    };
    match kind_field {
        "mod-value" => set_mod_value(rt, id, value),
        "value" => {
            set_field(rt, id, "value", value.clone());
            set_field(rt, id, "base", value.clone());
            set_mod_value(rt, id, value);
        }
        _ => set_field(rt, id, kind_field, value),
    }
    rt.run_reactive_cycle();
}

/// Param `idx` of the seeded current track's device at chain position
/// `slot` (-1: the instrument; [`seed_panel_kinds`]).
pub(super) fn seeded_param(editor: &Editor, slot: f64, idx: usize) -> eseqlisp::vm::InstanceId {
    let rt = editor.runtime();
    let selection = kind_singleton_rt(rt, "selection");
    let Ok(Value::Instance(track)) = rt.instance_field(selection, "track") else {
        panic!("no seeded track");
    };
    let device = field_ids(rt, track, "devices")
        .into_iter()
        .find(|&device| rt.instance_field(device, "slot") == Ok(Value::Number(slot)))
        .unwrap_or_else(|| panic!("no seeded device at slot {slot}"));
    field_ids(rt, device, "params")[idx]
}

/// Param `idx` of rack slot `slot` of the seeded current track's drum
/// rack ([`seed_panel_kinds`]).
pub(super) fn seeded_rack_slot_param(
    editor: &Editor,
    slot: usize,
    idx: usize,
) -> eseqlisp::vm::InstanceId {
    let rt = editor.runtime();
    let ids = |id, field: &str| field_ids(rt, id, field);
    let selection = kind_singleton_rt(rt, "selection");
    let Ok(Value::Instance(track)) = rt.instance_field(selection, "track") else {
        panic!("no seeded track");
    };
    let rack = ids(track, "devices")[0];
    ids(ids(rack, "devices")[slot], "params")[idx]
}

/// Push `field` of a seeded instance as the host-kinds tick does.
pub(super) fn push_seeded(
    editor: &mut Editor,
    id: eseqlisp::vm::InstanceId,
    field: &str,
    value: Value,
) {
    let rt = editor.runtime_mut();
    set_field(rt, id, field, value);
    rt.run_reactive_cycle();
}

/// Write the source a binding reads: an instance field (pushed as the
/// host-kinds tick does) or a legacy reactive field.
pub(super) fn set_binding_source(editor: &mut Editor, namespace: &str, field: &str, value: Value) {
    let rt = editor.runtime_mut();
    match namespace
        .strip_prefix("%instance/")
        .and_then(|id| id.parse().ok())
    {
        Some(id) => {
            set_field(rt, id, field, value);
            rt.run_reactive_cycle();
        }
        None => {
            rt.set_reactive(namespace, field, value);
        }
    }
}

/// Push `kind_field` of the param that replaced legacy value field
/// `field` ([`seed_panel_kinds`]), as the host-kinds tick does.
pub(super) fn set_panel_param_field(
    editor: &mut Editor,
    kinds: &PanelKinds,
    field: &str,
    kind_field: &str,
    value: Value,
) {
    let id = *kinds
        .fields
        .get(field)
        .unwrap_or_else(|| panic!("no seeded param for {field}"));
    let rt = editor.runtime_mut();
    set_field(rt, id, kind_field, value);
    rt.run_reactive_cycle();
}

/// Drive the param (or lane source or depth) that replaced legacy value
/// field `field` ([`seed_panel_kinds`]), as the host-kinds tick does.
pub(super) fn set_panel_param(editor: &mut Editor, kinds: &PanelKinds, field: &str, value: Value) {
    let id = *kinds
        .fields
        .get(field)
        .unwrap_or_else(|| panic!("no seeded param for {field}"));
    let rt = editor.runtime_mut();
    let tensor = rt
        .instance_kind(id)
        .is_some_and(|kind| kind.ends_with(":tensor"));
    if tensor {
        set_field(rt, id, "values", value.clone());
        set_field(rt, id, "base", value);
    } else {
        for name in ["value", "base"] {
            set_field(rt, id, name, value.clone());
        }
        set_mod_value(rt, id, value);
    }
    rt.run_reactive_cycle();
}

/// Param `id`'s modulated value and its ratio form (`mod-ratio`: / 100 for
/// a percent param), as the host-kinds tick pushes them.
fn set_mod_value(rt: &mut Runtime, id: eseqlisp::vm::InstanceId, value: Value) {
    let percent = matches!(rt.instance_field(id, "percent"), Ok(Value::Bool(true)));
    let ratio = match (&value, percent) {
        (Value::Number(n), true) => Value::Number(n / 100.0),
        _ => value.clone(),
    };
    set_field(rt, id, "mod-value", value);
    set_field(rt, id, "mod-ratio", ratio);
}
