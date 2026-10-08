//! The step panel's p-lock table (eseq-0l17.74): `selection.plock-rows`,
//! one `plock-row` per p-lock at the selected step (a selected neural
//! neuron's output overrides instead while one of the current scene is
//! selected; a previewed variant's locks while a chip previews one with no
//! step selected), and `selection.plock-variant`, the variant chip the
//! table lights.
//!
//! The rows are the legacy table's (`build_track_plocks_value*`, which the
//! kind reads instead of `SEQ.track-plocks`), built while the fields are
//! observed and only when what they read may have moved ([`RowsKey`]: the
//! [`ModelRevision`] (epochs, history, scenes, song mirror, sound binding),
//! the track's [`PlockKey`], the selection, the neuron selection, the
//! preview); an unobserved table costs a query, and a by-value read of it
//! sees the last build until the next tick observes it. A row is keyed by what it
//! locks, so an edit that keeps the row set keeps the instances and pushes
//! only the fields that changed. A lock of a device param or of a drum rack
//! macro carries its instance (`row.param`, `row.rack-macro`): the table
//! binds that instance's `value`, live through a drag, and edits it with
//! `lock-param!` / `lock-rack-macro!`; such a row speaks the param's display
//! units, as its knob does. The other rows (track settings, sends, step
//! params, neuron overrides, previews) keep their legacy values and the
//! table's host commands, addressed by `row.address`.

use super::*;
use sequencer::effects::{ParamDescriptor, ParamKind};
use sequencer::lisp_host::SelectedNeuralNeuron;
use std::collections::BTreeSet;

/// What the rows were last built from; `None` forces a rebuild.
#[derive(Clone, PartialEq)]
struct RowsKey {
    model: ModelRevision,
    track: usize,
    step: Option<usize>,
    plock: PlockKey,
    neural: BTreeSet<SelectedNeuralNeuron>,
    preview: Option<String>,
    devices: u64,
    selection: InstanceId,
}

/// The table's sync state (in [`HostKinds`]).
#[derive(Default)]
pub(crate) struct PlockRowState {
    rows: KeyedRows,
    seen: Option<RowsKey>,
    /// The variant chip a click previews while no step is selected (the
    /// tick's `GestureState::preview_plock_variant`: track, label).
    preview: Option<(usize, String)>,
    /// Row rebuilds, for tests.
    pub(crate) builds: u64,
}

impl PlockRowState {
    /// Rebuild at the next tick (a schema change, a hot reload).
    pub(super) fn invalidate(&mut self) {
        self.seen = None;
    }

    pub(super) fn representative(&self) -> Option<&InstanceId> {
        self.rows.representative()
    }
}

fn field_number(row: &HashMap<String, Rc<RefCell<Value>>>, name: &str) -> Option<f64> {
    match plock_row_field(row, name) {
        Value::Number(n) => Some(n),
        _ => None,
    }
}

/// The device a lock row's param belongs to, on the current track; `None`
/// for any other target.
fn row_device(target: &str, slot: Option<usize>, rack_slot: Option<usize>) -> Option<DeviceSlot> {
    Some(match target {
        "instrument" => DeviceSlot::Instrument,
        "effect" => DeviceSlot::Effect(slot?),
        "midi-fx" => DeviceSlot::MidiFx(slot?),
        "rack-effect" => DeviceSlot::RackEffect {
            rack_slot: rack_slot?,
            slot: slot?,
        },
        _ => return None,
    })
}

/// A legacy row value in its param's display units: the instrument rows
/// already speak them, the effect families' stored units.
fn display_units(target: &str, pdesc: &ParamDescriptor, legacy: f64) -> f64 {
    match target {
        "instrument" => legacy,
        _ => f64::from(DeviceSlot::to_user(pdesc, legacy as f32)),
    }
}

impl HostKinds {
    /// The variant chip a click previews (track, label), as the tick's
    /// gesture state holds it; the table shows its locks while no step is
    /// selected.
    pub(crate) fn set_plock_preview(&mut self, preview: Option<&(usize, String)>) {
        if self.plock_rows.preview.as_ref() != preview {
            self.plock_rows.preview = preview.cloned();
        }
    }

    /// `selection.plock-rows` and `selection.plock-variant` when what they
    /// read moved (see the module docs).
    pub(super) fn sync_plock_rows(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        model: &ModelRevision,
    ) {
        let Some(selection) = pusher.singleton(SELECTION) else {
            return;
        };
        let fields = [f::SELECTION_PLOCK_ROWS.1, f::SELECTION_PLOCK_VARIANT.1];
        if pusher.rt.host_fields_observed(selection, &fields) == 0 {
            // Built anew once observed again.
            self.plock_rows.seen = None;
            return;
        }
        let sources = pusher.sources;
        let track = sources.current_track.load(Ordering::Relaxed);
        let exists = sources.track_exists(track) && track < app.tracks.len();
        let step = selected_plock_step(&sources.selected_steps);
        let preview = (self.plock_rows.preview.as_ref())
            .filter(|(at, _)| *at == track && step.is_none())
            .map(|(_, label)| label.clone());
        let key = RowsKey {
            model: model.clone(),
            track,
            step,
            plock: sources.plock_key(track),
            neural: sources.selected_neural_neurons.lock().unwrap().clone(),
            preview,
            devices: pusher.shared.borrow().devices_generation,
            selection,
        };
        if self.plock_rows.seen.as_ref() == Some(&key) {
            return;
        }
        self.plock_rows.builds += 1;
        let state = &sources.state;
        let rows = match (&key.preview, exists) {
            (_, false) => Value::List(Vec::new()),
            (Some(label), true) => {
                build_track_plocks_value_for_variant_label(app, state, track, label)
            }
            (None, true) => build_track_plocks_value_with_neural_selection(
                app,
                state,
                track,
                &sources.selected_steps,
                Some(&key.neural),
            ),
        };
        let rows: Vec<HashMap<String, Rc<RefCell<Value>>>> = match rows {
            Value::List(items) => (items.iter())
                .filter_map(|item| match &*item.borrow() {
                    Value::Map(map) => Some(map.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut counts: HashMap<String, usize> = HashMap::new();
        let keys: Vec<String> = (rows.iter())
            .map(|row| {
                let base =
                    ["source", "label"]
                        .into_iter()
                        .map(|name| plock_row_text(row, name))
                        .chain(PLOCK_ROW_ADDRESS.iter().map(|name| {
                            match plock_row_field(row, name) {
                                Value::Number(n) => n.to_string(),
                                Value::String(text) => text,
                                _ => String::new(),
                            }
                        }))
                        .chain([plock_row_text(row, "name")])
                        .collect::<Vec<_>>()
                        .join("|");
                let seen = counts.entry(base.clone()).or_default();
                *seen += 1;
                format!("{base}#{seen}")
            })
            .collect();
        let ids = self.plock_rows.rows.reconcile(pusher, PLOCK_ROW, &keys);
        let track_id = self.track_ids.get(track).copied().flatten();
        let mut resolved = true;
        for (index, (row, id)) in rows.iter().zip(&ids).enumerate() {
            let Some(id) = *id else { continue };
            resolved &= push_plock_row(pusher, app, (track, track_id), id, index, row);
        }
        pusher.push(selection, f::SELECTION_PLOCK_ROWS, listed_instances(&ids));
        let chip = match exists {
            true => plock_variant_chip(state, track, step, key.preview.as_deref()),
            false => String::new(),
        };
        pusher.push(selection, f::SELECTION_PLOCK_VARIANT, Value::String(chip));
        // A lock whose param or rack macro has no instance yet (its device
        // registers at a later sync) is built again next tick, so no row
        // keeps the legacy units and commands for good.
        self.plock_rows.seen = resolved.then_some(key);
    }
}

/// One row's fields from its legacy map. Returns whether a lock of a
/// device param or rack macro at the selected step found its instance.
fn push_plock_row(
    pusher: &mut Pusher<'_>,
    app: &app::App,
    (track, track_id): (usize, Option<InstanceId>),
    id: InstanceId,
    index: usize,
    row: &HashMap<String, Rc<RefCell<Value>>>,
) -> bool {
    let target = plock_row_text(row, "target");
    let (source, name) = plock_row_source_and_name(row);
    let index_of = |name| field_number(row, name).map(|n| n as usize);
    let at_step = source == "step";
    // The step instance of a lock at the selected step.
    let step = (at_step.then(|| index_of("step-idx")).flatten())
        .zip(track_id)
        .and_then(|(step, track_id)| {
            let num_steps = pusher.sources.num_steps(track);
            step_of(&mut *pusher.rt, track_id, num_steps, step)
        });
    let param_idx = index_of("param-idx");
    // A device param lock's param, with its descriptor.
    let device =
        row_device(&target, index_of("slot-idx"), index_of("rack-slot")).filter(|_| step.is_some());
    let param = (device.zip(track_id).zip(param_idx)).and_then(|((device, track_id), index)| {
        let device_id = (pusher.rt).keyed_instance(DEVICE, &[track_id, device.did(app, track)])?;
        if !pusher.shared.borrow().param_devices.contains(&device_id) {
            let params = register_params(&mut *pusher.rt, pusher.shared, device_id)?;
            pusher.push(device_id, f::DEVICE_PARAMS, instance_list(params));
        }
        let source = pusher.shared.borrow().devices.get(&device_id).cloned()?;
        let pdesc = source.params().get(index)?.clone();
        let param = pusher
            .rt
            .keyed_instance(PARAM, &[device_id, index as u64])?;
        Some((param, pdesc))
    });
    let rack_macro = (target == "rack-macro" && step.is_some())
        .then(|| {
            let instrument = pusher.rt.keyed_instance(DEVICE, &[track_id?, 0])?;
            pusher
                .rt
                .keyed_instance(RACK_MACRO, &[instrument, param_idx? as u64])
        })
        .flatten();
    let options = match plock_row_field(row, "options") {
        Value::List(items) => Value::List(items),
        _ => Value::List(Vec::new()),
    };
    let mut value = field_number(row, "value").unwrap_or(0.0);
    let mut default = field_number(row, "default").unwrap_or(0.0);
    let mut min = field_number(row, "min").unwrap_or(0.0);
    let mut max = field_number(row, "max").unwrap_or(0.0);
    let mut text = plock_row_text(row, "text-value");
    let mut default_text = plock_row_text(row, "default-text");
    if let Some((_, pdesc)) = &param {
        [value, default, min, max] =
            [value, default, min, max].map(|n| display_units(&target, pdesc, n));
        if matches!(pdesc.kind, ParamKind::Continuous { .. }) {
            text = format!("{value:.2}");
            default_text = format!("{default:.2}");
        }
    }
    let wants_handle = step.is_some() && (device.is_some() || target == "rack-macro");
    let resolved = !wants_handle || param.is_some() || rack_macro.is_some();
    let address = PLOCK_ROW_ADDRESS
        .iter()
        .filter_map(|name| {
            let value = plock_row_field(row, name);
            (value != Value::Nil).then_some((*name, value))
        })
        .collect::<Vec<_>>();
    pusher.push(id, f::PLOCK_ROW_INDEX, number(index as f64));
    pusher.push(id, f::PLOCK_ROW_TARGET, Value::String(target));
    pusher.push(
        id,
        f::PLOCK_ROW_DOMAIN,
        Value::String(plock_row_text(row, "domain")),
    );
    pusher.push(id, f::PLOCK_ROW_SOURCE, Value::String(source.to_string()));
    pusher.push(id, f::PLOCK_ROW_NAME, Value::String(name));
    pusher.push(id, f::PLOCK_ROW_VALUE, number(value));
    pusher.push(id, f::PLOCK_ROW_TEXT, Value::String(text));
    pusher.push(id, f::PLOCK_ROW_DEFAULT, number(default));
    pusher.push(id, f::PLOCK_ROW_DEFAULT_TEXT, Value::String(default_text));
    pusher.push(id, f::PLOCK_ROW_MIN, number(min));
    pusher.push(id, f::PLOCK_ROW_MAX, number(max));
    pusher.push(id, f::PLOCK_ROW_OPTIONS, options);
    pusher.push(id, f::PLOCK_ROW_STEP, instance_or_nil(step));
    pusher.push(
        id,
        f::PLOCK_ROW_PARAM,
        instance_or_nil(param.map(|(param, _)| param)),
    );
    pusher.push(id, f::PLOCK_ROW_RACK_MACRO, instance_or_nil(rack_macro));
    pusher.push(id, f::PLOCK_ROW_ADDRESS, map_value(address));
    resolved
}
