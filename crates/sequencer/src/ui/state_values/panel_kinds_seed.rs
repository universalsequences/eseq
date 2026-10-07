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
    let published = |rt: &Runtime, field: Option<String>| match field
        .and_then(|field| rt.reactive_field_value("SEQ", &field).cloned())
    {
        Some(Value::Number(n)) => Some(n),
        Some(Value::Bool(b)) => Some(if b { 1.0 } else { 0.0 }),
        _ => None,
    };
    for &(index, dict) in &indexed {
        let param = params[index];
        let value = published(rt, dict_string(dict, "value-field"))
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
                let slot = published(rt, dict_string(lane, "source-value-field"))
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
                    let value = published(rt, dict_string(lane, "depth-value-field"))
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

/// A rack panel dict's macros (`:macros`, their `:mappings` onto the
/// slots' instrument and effect params) as the rack device's
/// rack-macros.
pub(super) fn seed_panel_rack_macros(
    rt: &mut Runtime,
    inst: &Value,
    rack: eseqlisp::vm::InstanceId,
    slots: &[eseqlisp::vm::InstanceId],
) {
    let ids = |rt: &Runtime, id: eseqlisp::vm::InstanceId, field: &str| match rt
        .instance_field(id, field)
    {
        Ok(Value::List(items)) => items
            .iter()
            .filter_map(|item| match &*item.borrow() {
                Value::Instance(id) => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let macros: Vec<_> = dict_items(dict_value(inst, "macros"))
        .iter()
        .map(|rack_macro| {
            let index = dict_number(rack_macro, "id").unwrap_or(0.0);
            let id = rt
                .register_keyed_instance("eseq.kinds:rack-macro", &[rack, index as u64])
                .unwrap();
            set_field(rt, id, "device", Value::Instance(rack));
            set_field(rt, id, "index", Value::Number(index));
            let mappings: Vec<_> = dict_items(dict_value(rack_macro, "mappings"))
                .iter()
                .enumerate()
                .map(|(position, mapping)| {
                    let mapping_id = rt
                        .register_keyed_instance("eseq.kinds:macro-mapping", &[id, position as u64])
                        .unwrap();
                    set_field(rt, mapping_id, "rack-macro", Value::Instance(id));
                    set_field(rt, mapping_id, "index", Value::Number(position as f64));
                    let slot = dict_number(mapping, "rack-slot")
                        .and_then(|slot| slots.get(slot as usize).copied());
                    let device = match dict_string(mapping, "kind").as_deref() {
                        Some("rack-slot-instrument") => slot,
                        Some("rack-slot-effect") => slot.and_then(|slot| {
                            let effect_slot = dict_number(mapping, "effect-slot")?;
                            ids(rt, slot, "devices").into_iter().find(|&effect| {
                                rt.instance_field(effect, "slot") == Ok(Value::Number(effect_slot))
                            })
                        }),
                        _ => None,
                    };
                    let target = device.and_then(|device| {
                        let idx = dict_number(mapping, "param-idx")? as usize;
                        ids(rt, device, "params").get(idx).copied()
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

/// The legacy p-lock lists a test published (`SEQ.track-plocks`, the
/// selected step's lock rows; `SEQ.track-plock-any`, the params locked on
/// any step; `SEQ.track-plock-variants`, the variant chips) as the
/// seeded params' `locked` / `base` / `value` / `has-locks` and the
/// track's variants.
pub(super) fn seed_panel_locks(rt: &mut Runtime, track: eseqlisp::vm::InstanceId) {
    let ids = |value: Value| -> Vec<eseqlisp::vm::InstanceId> {
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
    };
    let row_param = |rt: &Runtime, row: &Value| -> Option<eseqlisp::vm::InstanceId> {
        let target = dict_string(row, "target")?;
        let idx = dict_number(row, "param-idx")? as usize;
        let (list, slot) = match target.as_str() {
            "instrument" => ("devices", -1.0),
            "effect" => ("devices", dict_number(row, "slot-idx")?),
            "midi-fx" => ("midi-devices", dict_number(row, "slot-idx")?),
            _ => return None,
        };
        let device = ids(rt.instance_field(track, list).ok()?)
            .into_iter()
            .find(|&device| rt.instance_field(device, "slot") == Ok(Value::Number(slot)))?;
        ids(rt.instance_field(device, "params").ok()?)
            .get(idx)
            .copied()
    };
    let published =
        |rt: &Runtime, field: &str| dict_items(rt.reactive_field_value("SEQ", field).cloned());
    // The lists say which params are locked: every other one is not.
    let devices: Vec<_> = ["devices", "midi-devices"]
        .into_iter()
        .flat_map(|list| ids(rt.instance_field(track, list).unwrap_or(Value::Nil)))
        .collect();
    for device in devices {
        for param in ids(rt.instance_field(device, "params").unwrap_or(Value::Nil)) {
            set_field(rt, param, "locked", Value::Bool(false));
            set_field(rt, param, "has-locks", Value::Bool(false));
        }
    }
    for row in published(rt, "track-plocks") {
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
    for row in published(rt, "track-plock-any") {
        if let Some(param) = row_param(rt, &row) {
            set_field(rt, param, "has-locks", Value::Bool(true));
        }
    }
    let variants: Vec<_> = published(rt, "track-plock-variants")
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

/// An instrument dict's key locks (`:key-locks` per param, the locked
/// notes, the key-lock variants and the notes stamped with them) as the
/// instrument device's and its params' fields.
pub(super) fn seed_panel_key_locks(
    rt: &mut Runtime,
    inst: &Value,
    track: eseqlisp::vm::InstanceId,
    device: eseqlisp::vm::InstanceId,
) {
    let Ok(Value::List(params)) = rt.instance_field(device, "params") else {
        return;
    };
    let params: Vec<_> = params
        .iter()
        .filter_map(|param| match &*param.borrow() {
            Value::Instance(id) => Some(*id),
            _ => None,
        })
        .collect();
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
    let active = match rt.reactive_field_value("SEQ", "track-active-notes") {
        Some(Value::List(tracks)) => tracks.first().map(|notes| notes.borrow().clone()),
        _ => None,
    };
    if let Some(Value::List(notes)) = active {
        let rows = notes
            .iter()
            .map(|note| {
                test_list(vec![
                    note.borrow().clone(),
                    Value::Number(1.0),
                    Value::Number(0.0),
                ])
            })
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
    let published =
        |rt: &Runtime, field: &str| dict_items(rt.reactive_field_value("SEQ", field).cloned());
    let mut devices = Vec::new();
    if let Some(inst) = published(rt, "instrument-panel")
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
                        effect
                    })
                    .collect();
                set_field(rt, device, "devices", instance_list(effects));
                device
            })
            .collect();
        set_field(rt, rack, "devices", instance_list(slots.iter().copied()));
        seed_panel_rack_macros(rt, &inst, rack, &slots);
        devices.push(rack);
    } else if let Some(inst) = published(rt, "instrument-panel").into_iter().next() {
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
        devices.push(device);
    }
    for (position, fx) in published(rt, "effects").iter().enumerate() {
        let slot = dict_number(fx, "slot-idx").unwrap_or(position as f64);
        let name = dict_string(fx, "name").unwrap_or_default();
        let dicts = device_param_dicts(fx);
        devices.push(seed_panel_device(
            rt,
            &mut kinds,
            track,
            1000 + slot as u64,
            slot,
            &name,
            &dicts,
        ));
    }
    set_field(rt, track, "devices", instance_list(devices));
    let midi: Vec<_> = published(rt, "midi-effects")
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
    let bus_effects = published(rt, "bus-effects");
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
                    device
                })
                .collect();
            set_field(rt, *bus, "devices", instance_list(devices));
        }
    }
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
        ("process-clamped", Value::Number(n)) => Value::Bool(n > 0.5),
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
    let ids = |value: Value| -> Vec<eseqlisp::vm::InstanceId> {
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
    };
    let device = ids(rt.instance_field(track, "devices").unwrap())
        .into_iter()
        .find(|&device| rt.instance_field(device, "slot") == Ok(Value::Number(slot)))
        .unwrap_or_else(|| panic!("no seeded device at slot {slot}"));
    ids(rt.instance_field(device, "params").unwrap())[idx]
}

/// Param `idx` of rack slot `slot` of the seeded current track's drum
/// rack ([`seed_panel_kinds`]).
pub(super) fn seeded_rack_slot_param(
    editor: &Editor,
    slot: usize,
    idx: usize,
) -> eseqlisp::vm::InstanceId {
    let rt = editor.runtime();
    let ids = |id: eseqlisp::vm::InstanceId, field: &str| match rt.instance_field(id, field) {
        Ok(Value::List(items)) => items
            .iter()
            .filter_map(|item| match &*item.borrow() {
                Value::Instance(id) => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
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
