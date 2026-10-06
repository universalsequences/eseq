//! The native neural network kinds' setters (kind-bindings spec §14.2q):
//! `set-neural` (`:network-id`, `:field`, `:value`; `:neuron` for a neuron's
//! field, `:track-id` for its route; `:from` and `:to` for one weight cell).
//!
//! A network is named by its id among the current scene's networks, a
//! neuron by its index below the network's neuron count, a track by its
//! stable `TrackId`, all resolved when the command lands; a network the
//! current scene no longer holds or a neuron past the count is an error.
//! Edits act only where the value differs and write the field the legacy
//! `neural-set` / `neural-neuron` / `neural-weight(s)` natives write, through
//! `App::apply_neural_network_edit`: one undo entry each, per field
//! ([`NeuralSlot`]: undo restores that field as it was, leaving every other
//! edit of the network alone). Values follow the value rule ([`SetValue`]):
//! where a native clamps (a threshold below 0, an energy decay past 1, …)
//! the setter rejects, so every value it writes is one the native would have
//! kept; a label among its options (case-insensitive), a number finite and
//! in range, an integer whole; anything else is an error that changes
//! nothing. Gestures as [`super::ScriptEdit`]: a numeric field's `set!`s
//! while the pointer is down join one entry per field.

use super::graphs::timebase;
use super::track_settings::{command_track, SetValue};
use crate::*;
use sequencer::neural::{NeuralMaxPolySelection, NeuralSlot, NeuronField, ProjectNeuralNetwork};
use std::collections::HashMap;

pub(super) const COMMANDS: &[&str] = &["set-neural"];

type Payload = HashMap<String, Rc<RefCell<Value>>>;

/// The largest finite `f32` (the model's numbers are `f32`s).
const F32_MAX: f64 = f32::MAX as f64;

/// The current scene's network `:network-id` names.
fn network(app: &app::App, map: &Payload) -> Result<ProjectNeuralNetwork, String> {
    let id = SetValue::of(map, "network-id", "network-id").id("a network id")?;
    (app.state.current_neural_networks().into_iter())
        .find(|network| network.id == id)
        .ok_or_else(|| "the network is gone".to_string())
}

/// A neuron index (`:neuron`, or `:from` / `:to` for a cell) below the
/// network's neuron count.
fn neuron_index(network: &ProjectNeuralNetwork, map: &Payload, key: &str) -> Result<usize, String> {
    match network.num_neurons {
        0 => Err("the network has no neurons".to_string()),
        count => SetValue::of(map, key, key).integer(0, count - 1),
    }
}

/// A weight matrix: `size` rows of `size` finite numbers (the
/// `neural-weights` native's parse), named as §14.2c names a wrong value.
fn matrix(value: &SetValue<'_>, size: usize) -> Result<Vec<Vec<f32>>, String> {
    sequencer::lisp_host::parse_neural_weight_matrix(value.value(), size)
        .or_else(|_| value.fail(&format!("{size} lists of {size} finite numbers")))
}

/// A neuron field's slot: (field, whether a drag moves it). The arms are
/// `NeuronField::NAMES` (the history's merge keys name a field by it): the
/// parsed field must carry the name it was asked by (checked in debug
/// builds; a host kinds test sets every name).
fn neuron_field(
    app: &app::App,
    field: &str,
    value: &SetValue<'_>,
    map: &Payload,
) -> Result<(NeuronField, bool), String> {
    let number = |min: f64, max: f64| value.number(min, max).map(|v| v as f32);
    let parsed = match field {
        "route" => {
            let track = SetValue::of(map, "track-id", "route").id_or_nil("a track or nil")?;
            let route = match track {
                None => None,
                Some(_) => Some(command_track(app, map)?),
            };
            (NeuronField::Route(route), false)
        }
        "resolution" => {
            let label = value.label()?;
            (
                NeuronField::Resolution(timebase(field, label)? as u8),
                false,
            )
        }
        "quantize" => {
            let label = value.label()?;
            let quantize = if label.eq_ignore_ascii_case("off") {
                None
            } else {
                Some(timebase(field, label)? as u8)
            };
            (NeuronField::Quantize(quantize), false)
        }
        "delay" => {
            let delay = value.integer(0, u32::MAX as usize)? as u32;
            (NeuronField::Delay(delay), true)
        }
        "threshold" => (NeuronField::Threshold(number(0.0, F32_MAX)?), true),
        "transpose" => (NeuronField::Transpose(number(-F32_MAX, F32_MAX)?), true),
        "dampening-amount" => (NeuronField::DampeningAmount(number(0.0, 1.0)?), true),
        "dampening-recovery" => (NeuronField::DampeningRecovery(number(0.0, 1.0)?), true),
        other => return Err(format!("unknown field '{other}'")),
    };
    debug_assert_eq!(
        parsed.0.name(),
        field,
        "NeuronField::NAMES and the parser agree"
    );
    Ok(parsed)
}

/// What `set-neural` asks for: the slot to write and whether a drag moves
/// it; `None` when the network already is so.
fn neural_request(
    app: &app::App,
    network: &ProjectNeuralNetwork,
    map: &Payload,
) -> Result<Option<(NeuralSlot, bool)>, String> {
    let (field, value) = SetValue::field(map)?;
    let (slot, continuous) = match field.as_str() {
        "name" => (NeuralSlot::Name(value.name()?.to_string()), false),
        "enabled" => (NeuralSlot::Enabled(value.flag()?), false),
        "reset-bars" => (
            NeuralSlot::ResetBars(value.number(0.25, F32_MAX)? as f32),
            true,
        ),
        "energy-decay" => (
            NeuralSlot::EnergyDecay(value.number(0.0, 1.0)? as f32),
            true,
        ),
        "max-poly" => (
            NeuralSlot::MaxPoly(value.integer(1, u32::MAX as usize)? as u32),
            true,
        ),
        "max-poly-selection" => {
            let names = NeuralMaxPolySelection::ALL.map(NeuralMaxPolySelection::as_str);
            let selection = NeuralMaxPolySelection::ALL[value.choice(&names)?];
            (NeuralSlot::MaxPolySelection(selection), false)
        }
        "weights" => (
            NeuralSlot::Weights(matrix(&value, network.num_neurons)?),
            true,
        ),
        "weight" => {
            let from = neuron_index(network, map, "from")?;
            let to = neuron_index(network, map, "to")?;
            let v = value.number(-F32_MAX, F32_MAX)? as f32;
            (NeuralSlot::Weight { from, to, value: v }, true)
        }
        field => {
            let (field, continuous) = neuron_field(app, field, &value, map)?;
            let index = neuron_index(network, map, "neuron")?;
            (NeuralSlot::Neuron { index, field }, continuous)
        }
    };
    if slot.read(network) == slot {
        return Ok(None);
    }
    Ok(Some((slot, continuous)))
}

/// Apply `set-neural` as a script edit.
fn set_neural(app: &mut app::App, ctx: &mut LoopCtx<'_>, map: &Payload) -> Result<(), String> {
    let network = network(app, map)?;
    let Some((slot, continuous)) = neural_request(app, &network, map)? else {
        return Ok(());
    };
    super::ScriptEdit::run(app, ctx, continuous, |app| {
        app.apply_neural_network_edit(network.id, slot)
    })
    .map(|_| ())
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let Value::Map(map) = &payload else {
        editor.handle_host_event(HostEvent::Error(format!(
            "{name}: the payload is not a dict"
        )));
        return;
    };
    if let Err(message) = set_neural(app, ctx, map) {
        editor.handle_host_event(HostEvent::Error(format!("{name}: {message}")));
    }
}
