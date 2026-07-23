/*!
Argument parsing for the `neural-*` authoring natives.

The builtin neural sequencer is edited from lisp via natives like
`neural-create`, `neural-set`, `neural-neuron`, `neural-weight`, and
`neural-plock-*`. This module defines the intermediate edit types those
natives produce — `NeuralCreateOptions`, `NeuralSetEdits`,
`NeuralNeuronEdits`, `NeuralWeightEdit`, plus `NeuralNetworkRef` for
name-or-id lookups — and the `parse_neural_*` functions that build them from
raw `EValue` args. The natives themselves are registered in
`sequencer_natives`, which applies these edits to the project's
`ProjectNeuralNetwork` list.
*/

use super::super::*;

#[derive(Clone, Debug)]
pub(in crate::lisp_host) enum NeuralNetworkRef {
    Id(u64),
    Name(String),
}

#[derive(Clone, Debug)]
pub(in crate::lisp_host) struct NeuralCreateOptions {
    pub(in crate::lisp_host) name: String,
    pub(in crate::lisp_host) num_neurons: usize,
    pub(in crate::lisp_host) enabled: bool,
    pub(in crate::lisp_host) weights: Option<Vec<Vec<f32>>>,
}

#[derive(Clone, Debug, Default)]
pub(in crate::lisp_host) struct NeuralSetEdits {
    pub(in crate::lisp_host) name: Option<String>,
    pub(in crate::lisp_host) reset_interval_bars: Option<f32>,
    pub(in crate::lisp_host) energy_decay: Option<f32>,
    pub(in crate::lisp_host) max_poly: Option<u32>,
    pub(in crate::lisp_host) max_poly_selection: Option<NeuralMaxPolySelection>,
}

#[derive(Clone, Debug, Default)]
pub(in crate::lisp_host) struct NeuralNeuronEdits {
    pub(in crate::lisp_host) route: Option<Option<usize>>,
    pub(in crate::lisp_host) resolution: Option<Timebase>,
    pub(in crate::lisp_host) threshold: Option<f32>,
    pub(in crate::lisp_host) delay_steps: Option<u32>,
    pub(in crate::lisp_host) quantize: Option<Option<Timebase>>,
    pub(in crate::lisp_host) transpose: Option<f32>,
    pub(in crate::lisp_host) dampening_amount: Option<f32>,
    pub(in crate::lisp_host) dampening_recovery: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::lisp_host) struct NeuralWeightEdit {
    pub(in crate::lisp_host) from: usize,
    pub(in crate::lisp_host) to: usize,
    pub(in crate::lisp_host) value: f32,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::lisp_host) struct NeuralResetStepEdit {
    pub(in crate::lisp_host) track: usize,
    pub(in crate::lisp_host) step: usize,
    pub(in crate::lisp_host) enabled: bool,
}

pub(in crate::lisp_host) fn parse_neural_network_ref(value: &EValue) -> Result<NeuralNetworkRef, String> {
    match value {
        EValue::Number(id) if id.is_finite() && *id >= 0.0 && id.fract() == 0.0 => {
            Ok(NeuralNetworkRef::Id(*id as u64))
        }
        EValue::String(name) | EValue::Keyword(name) | EValue::Symbol(name) => {
            let name = name.trim().to_string();
            if name.is_empty() {
                Err("neural network name cannot be empty".to_string())
            } else {
                Ok(NeuralNetworkRef::Name(name))
            }
        }
        _ => Err("expected neural network id or name".to_string()),
    }
}

pub(in crate::lisp_host) fn neural_network_index(
    networks: &[ProjectNeuralNetwork],
    reference: &NeuralNetworkRef,
) -> Result<usize, String> {
    match reference {
        NeuralNetworkRef::Id(id) => networks
            .iter()
            .position(|network| network.id == *id)
            .ok_or_else(|| format!("unknown neural network id {id}")),
        NeuralNetworkRef::Name(name) => {
            let matches = networks
                .iter()
                .enumerate()
                .filter(|(_, network)| network.name.eq_ignore_ascii_case(name))
                .map(|(idx, _)| idx)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [idx] => Ok(*idx),
                [] => Err(format!("unknown neural network '{name}'")),
                _ => Err(format!("ambiguous neural network name '{name}'")),
            }
        }
    }
}

pub(in crate::lisp_host) fn next_neural_network_id(networks: &[ProjectNeuralNetwork]) -> u64 {
    networks
        .iter()
        .map(|network| network.id)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
        .max(1)
}

pub(in crate::lisp_host) fn parse_nonnegative_usize(value: &EValue, label: &str) -> Result<usize, String> {
    match value {
        EValue::Number(value)
            if value.is_finite()
                && *value >= 0.0
                && value.fract() == 0.0
                && *value <= usize::MAX as f64 =>
        {
            Ok(*value as usize)
        }
        _ => Err(format!("{label} must be a non-negative integer")),
    }
}

pub(in crate::lisp_host) fn parse_positive_neuron_count(value: &EValue) -> Result<usize, String> {
    let count = parse_nonnegative_usize(value, "neuron count")?;
    if count == 0 || count > NUM_NEURONS {
        return Err(format!("neuron count must be 1..={NUM_NEURONS}"));
    }
    Ok(count)
}

pub(in crate::lisp_host) fn parse_u32_value(value: &EValue, label: &str) -> Result<u32, String> {
    match value {
        EValue::Number(value)
            if value.is_finite()
                && *value >= 0.0
                && value.fract() == 0.0
                && *value <= u32::MAX as f64 =>
        {
            Ok(*value as u32)
        }
        _ => Err(format!("{label} must be a non-negative integer")),
    }
}

pub(in crate::lisp_host) fn parse_f32_value(value: &EValue, label: &str) -> Result<f32, String> {
    match value {
        EValue::Number(value)
            if value.is_finite() && *value >= f32::MIN as f64 && *value <= f32::MAX as f64 =>
        {
            Ok(*value as f32)
        }
        _ => Err(format!("{label} must be finite numeric")),
    }
}

pub(in crate::lisp_host) fn parse_bool_value(value: &EValue, label: &str) -> Result<bool, String> {
    match value {
        EValue::Bool(value) => Ok(*value),
        EValue::Nil => Ok(false),
        EValue::Number(value) if value.is_finite() => Ok(*value != 0.0),
        _ => Err(format!("{label} expects a boolean")),
    }
}

pub(in crate::lisp_host) fn parse_timebase_value(value: &EValue) -> Result<Timebase, String> {
    parse_timebase_arg(std::slice::from_ref(value), 0)
}

pub(in crate::lisp_host) fn parse_neural_max_poly_selection(value: &EValue) -> Result<NeuralMaxPolySelection, String> {
    let name = match value {
        EValue::String(value) | EValue::Keyword(value) | EValue::Symbol(value) => {
            value.trim().trim_start_matches(':').to_ascii_lowercase()
        }
        _ => {
            return Err(
                "max-poly selection expects deterministic, propagation, or random".to_string(),
            );
        }
    };
    match name.as_str() {
        "deterministic" | "ordered" | "first" => Ok(NeuralMaxPolySelection::Deterministic),
        "propagation" | "propagate" | "productive" | "impact" => {
            Ok(NeuralMaxPolySelection::Propagation)
        }
        "random" | "rand" => Ok(NeuralMaxPolySelection::Random),
        "loudest" | "loud" | "velocity" => Ok(NeuralMaxPolySelection::Loudest),
        "lowest-transpose" | "lowest" | "low-transpose" => {
            Ok(NeuralMaxPolySelection::LowestTranspose)
        }
        "highest-transpose" | "highest" | "high-transpose" => {
            Ok(NeuralMaxPolySelection::HighestTranspose)
        }
        "seed-first" | "seed" => Ok(NeuralMaxPolySelection::SeedFirst),
        _ => Err(
            "max-poly selection expects deterministic, propagation, random, loudest, \
             lowest-transpose, highest-transpose, or seed-first"
                .to_string(),
        ),
    }
}

pub(in crate::lisp_host) fn neural_attr_name(value: &EValue) -> Option<String> {
    match value {
        EValue::Keyword(name) => Some(name.to_ascii_lowercase()),
        EValue::Symbol(name) | EValue::String(name)
            if name.starts_with(':') || name.starts_with('@') =>
        {
            Some(
                name.trim_start_matches(':')
                    .trim_start_matches('@')
                    .to_ascii_lowercase(),
            )
        }
        _ => None,
    }
}

pub(in crate::lisp_host) fn parse_neural_create_args(args: &[EValue]) -> Result<NeuralCreateOptions, String> {
    let mut name: Option<String> = None;
    let mut num_neurons: Option<usize> = None;
    let mut enabled = true;
    let mut weights_value: Option<EValue> = None;
    let mut idx = 0;
    while idx < args.len() {
        let attr = neural_attr_name(&args[idx])
            .ok_or_else(|| "neural-create expects keyword arguments".to_string())?;
        idx += 1;
        let value = args
            .get(idx)
            .ok_or_else(|| format!("neural-create :{attr} expects a value"))?;
        match attr.as_str() {
            "name" => {
                name = match value {
                    EValue::String(value) | EValue::Keyword(value) | EValue::Symbol(value) => {
                        Some(value.trim().to_string())
                    }
                    _ => return Err("neural-create :name expects string/symbol".to_string()),
                };
            }
            "neurons" | "num-neurons" => num_neurons = Some(parse_positive_neuron_count(value)?),
            "enabled" => enabled = parse_bool_value(value, "neural-create :enabled")?,
            "weights" => weights_value = Some(value.clone()),
            other => return Err(format!("neural-create unknown argument :{other}")),
        }
        idx += 1;
    }
    let name = name.ok_or_else(|| "neural-create requires :name".to_string())?;
    if name.is_empty() {
        return Err("neural-create :name cannot be empty".to_string());
    }
    let num_neurons = num_neurons.ok_or_else(|| "neural-create requires :neurons".to_string())?;
    let weights = weights_value
        .as_ref()
        .map(|value| parse_neural_weight_matrix(value, num_neurons))
        .transpose()?;
    Ok(NeuralCreateOptions {
        name,
        num_neurons,
        enabled,
        weights,
    })
}

pub(in crate::lisp_host) fn parse_neural_set_args(args: &[EValue]) -> Result<NeuralSetEdits, String> {
    let mut edits = NeuralSetEdits::default();
    let mut idx = 0;
    while idx < args.len() {
        let attr = neural_attr_name(&args[idx])
            .ok_or_else(|| "neural-set expects keyword arguments".to_string())?;
        idx += 1;
        let value = args
            .get(idx)
            .ok_or_else(|| format!("neural-set :{attr} expects a value"))?;
        match attr.as_str() {
            "name" => {
                edits.name = match value {
                    EValue::String(value) | EValue::Keyword(value) | EValue::Symbol(value) => {
                        Some(value.trim().to_string())
                    }
                    _ => return Err("neural-set :name expects string/symbol".to_string()),
                };
            }
            "reset-bars" | "reset-interval-bars" => {
                edits.reset_interval_bars = Some(parse_f32_value(value, "reset bars")?)
            }
            "energy-decay" => edits.energy_decay = Some(parse_f32_value(value, "energy decay")?),
            "max-poly" => edits.max_poly = Some(parse_u32_value(value, "max-poly")?),
            "max-poly-selection" | "max-poly-mode" | "poly-selection" | "poly-mode" => {
                edits.max_poly_selection = Some(parse_neural_max_poly_selection(value)?)
            }
            other => return Err(format!("neural-set unknown argument :{other}")),
        }
        idx += 1;
    }
    Ok(edits)
}

pub(in crate::lisp_host) fn parse_neural_neuron_args(args: &[EValue]) -> Result<NeuralNeuronEdits, String> {
    let mut edits = NeuralNeuronEdits::default();
    let mut idx = 0;
    while idx < args.len() {
        let attr = neural_attr_name(&args[idx])
            .ok_or_else(|| "neural-neuron expects keyword arguments".to_string())?;
        idx += 1;
        let value = args
            .get(idx)
            .ok_or_else(|| format!("neural-neuron :{attr} expects a value"))?;
        match attr.as_str() {
            "route" => {
                edits.route = Some(match value {
                    EValue::Nil | EValue::Bool(false) => None,
                    _ => Some(parse_nonnegative_usize(value, "route")?),
                });
            }
            "resolution" | "clock" => edits.resolution = Some(parse_timebase_value(value)?),
            "threshold" => edits.threshold = Some(parse_f32_value(value, "threshold")?.max(0.0)),
            "delay" | "delay-steps" => edits.delay_steps = Some(parse_u32_value(value, "delay")?),
            "quantize" => {
                edits.quantize = Some(match value {
                    EValue::Nil | EValue::Bool(false) => None,
                    _ => Some(parse_timebase_value(value)?),
                });
            }
            "transpose" => edits.transpose = Some(parse_f32_value(value, "transpose")?),
            "dampening" | "dampening-amount" => {
                edits.dampening_amount = Some(parse_f32_value(value, "dampening")?)
            }
            "dampening-recovery" | "recovery" => {
                edits.dampening_recovery = Some(parse_f32_value(value, "dampening recovery")?)
            }
            other => return Err(format!("neural-neuron unknown argument :{other}")),
        }
        idx += 1;
    }
    Ok(edits)
}

pub(in crate::lisp_host) fn parse_neural_weight_args(args: &[EValue]) -> Result<NeuralWeightEdit, String> {
    let mut from = None;
    let mut to = None;
    let mut value = None;
    let mut idx = 0;
    while idx < args.len() {
        let attr = neural_attr_name(&args[idx])
            .ok_or_else(|| "neural-weight expects keyword arguments".to_string())?;
        idx += 1;
        let arg = args
            .get(idx)
            .ok_or_else(|| format!("neural-weight :{attr} expects a value"))?;
        match attr.as_str() {
            "from" => from = Some(parse_nonnegative_usize(arg, "from")?),
            "to" => to = Some(parse_nonnegative_usize(arg, "to")?),
            "value" | "amount" => value = Some(parse_f32_value(arg, "weight")?),
            other => return Err(format!("neural-weight unknown argument :{other}")),
        }
        idx += 1;
    }
    Ok(NeuralWeightEdit {
        from: from.ok_or_else(|| "neural-weight requires :from".to_string())?,
        to: to.ok_or_else(|| "neural-weight requires :to".to_string())?,
        value: value.ok_or_else(|| "neural-weight requires :value".to_string())?,
    })
}

pub(in crate::lisp_host) fn parse_neural_reset_step_args(args: &[EValue]) -> Result<NeuralResetStepEdit, String> {
    if args.len() == 3 && matches!(args[0], EValue::Number(_)) {
        return Ok(NeuralResetStepEdit {
            track: parse_nonnegative_usize(&args[0], "track")?,
            step: parse_step_arg(args, 1)?,
            enabled: parse_bool_value(&args[2], "neural-reset-step")?,
        });
    }

    let mut track = None;
    let mut step = None;
    let mut enabled = None;
    let mut idx = 0;
    while idx < args.len() {
        if let Some(attr) = neural_attr_name(&args[idx]) {
            idx += 1;
            let value = args
                .get(idx)
                .ok_or_else(|| format!("neural-reset-step :{attr} expects a value"))?;
            match attr.as_str() {
                "track" => track = Some(parse_nonnegative_usize(value, "track")?),
                "step" => step = Some(parse_step_arg(args, idx)?),
                "enabled" | "value" => {
                    enabled = Some(parse_bool_value(value, "neural-reset-step")?)
                }
                other => return Err(format!("neural-reset-step unknown argument :{other}")),
            }
            idx += 1;
        } else {
            enabled = Some(parse_bool_value(&args[idx], "neural-reset-step")?);
            idx += 1;
        }
    }
    Ok(NeuralResetStepEdit {
        track: track.ok_or_else(|| "neural-reset-step requires :track".to_string())?,
        step: step.ok_or_else(|| "neural-reset-step requires :step".to_string())?,
        enabled: enabled.ok_or_else(|| "neural-reset-step requires enabled bool".to_string())?,
    })
}

pub(in crate::lisp_host) fn parse_neural_weight_matrix(
    value: &EValue,
    expected_size: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let EValue::List(rows) = value else {
        return Err("neural weight matrix must be a list of rows".to_string());
    };
    let mut matrix = Vec::with_capacity(rows.len());
    for row in rows {
        let row = row.borrow();
        let EValue::List(cells) = &*row else {
            return Err("neural weight matrix rows must be lists".to_string());
        };
        let mut parsed_row = Vec::with_capacity(cells.len());
        for cell in cells {
            let cell = cell.borrow();
            match &*cell {
                EValue::Number(value)
                    if value.is_finite()
                        && *value >= f32::MIN as f64
                        && *value <= f32::MAX as f64 =>
                {
                    parsed_row.push(*value as f32)
                }
                _ => return Err("neural weight matrix cells must be numbers".to_string()),
            }
        }
        matrix.push(parsed_row);
    }
    validate_neural_matrix_shape(&matrix, expected_size)?;
    Ok(matrix)
}

pub(in crate::lisp_host) fn validate_neural_matrix_shape(matrix: &[Vec<f32>], expected_size: usize) -> Result<(), String> {
    if matrix.len() != expected_size {
        return Err(format!(
            "neural weight matrix must have {expected_size} rows"
        ));
    }
    if matrix.iter().any(|row| row.len() != expected_size) {
        return Err(format!(
            "neural weight matrix must be {expected_size}x{expected_size}"
        ));
    }
    Ok(())
}

pub(in crate::lisp_host) fn normalize_project_neural_network_shape(
    network: &mut ProjectNeuralNetwork,
) -> Result<(), String> {
    if network.num_neurons == 0 || network.num_neurons > NUM_NEURONS {
        return Err(format!("neural network size must be 1..={NUM_NEURONS}"));
    }
    network
        .neurons
        .resize_with(network.num_neurons, ProjectNeuron::default);
    network.neurons.truncate(network.num_neurons);
    if network.weights.len() != network.num_neurons
        || network
            .weights
            .iter()
            .any(|row| row.len() != network.num_neurons)
    {
        let mut normalized = vec![vec![0.0; network.num_neurons]; network.num_neurons];
        for (row_idx, row) in network.weights.iter().enumerate().take(network.num_neurons) {
            for (col_idx, value) in row.iter().enumerate().take(network.num_neurons) {
                normalized[row_idx][col_idx] = *value;
            }
        }
        network.weights = normalized;
    }
    Ok(())
}

pub(in crate::lisp_host) fn apply_neural_set_edits(
    network: &mut ProjectNeuralNetwork,
    edits: &NeuralSetEdits,
) -> Result<(), String> {
    if let Some(name) = &edits.name {
        if name.is_empty() {
            return Err("neural-set :name cannot be empty".to_string());
        }
        network.name = name.clone();
    }
    if let Some(reset_interval_bars) = edits.reset_interval_bars {
        network.reset_interval_bars = reset_interval_bars.max(0.25);
    }
    if let Some(energy_decay) = edits.energy_decay {
        network.energy_decay = energy_decay.clamp(0.0, 1.0);
    }
    if let Some(max_poly) = edits.max_poly {
        network.max_poly = max_poly.max(1);
    }
    if let Some(max_poly_selection) = edits.max_poly_selection {
        network.max_poly_selection = max_poly_selection;
    }
    Ok(())
}

pub(in crate::lisp_host) fn apply_neural_neuron_edits(
    neuron: &mut ProjectNeuron,
    edits: &NeuralNeuronEdits,
    track_count: usize,
) -> Result<(), String> {
    if let Some(route) = edits.route {
        if let Some(track) = route {
            if track >= track_count {
                return Err("route track out of range".to_string());
            }
        }
        neuron.route = route;
    }
    if let Some(resolution) = edits.resolution {
        neuron.resolution = resolution as u8;
    }
    if let Some(threshold) = edits.threshold {
        neuron.threshold = threshold.max(0.0);
    }
    if let Some(delay_steps) = edits.delay_steps {
        neuron.delay_steps = delay_steps;
    }
    if let Some(quantize) = edits.quantize {
        neuron.quantize = quantize.map(|timebase| timebase as u8);
    }
    if let Some(transpose) = edits.transpose {
        neuron.transpose = transpose;
    }
    if let Some(dampening_amount) = edits.dampening_amount {
        neuron.dampening_amount = dampening_amount.clamp(0.0, 1.0);
    }
    if let Some(dampening_recovery) = edits.dampening_recovery {
        neuron.dampening_recovery = dampening_recovery.clamp(0.0, 1.0);
    }
    Ok(())
}

pub(in crate::lisp_host) fn neural_instrument_param_id(
    state: &crate::sequencer::SequencerState,
    track: usize,
    param_idx: usize,
) -> Result<ParamNodeId, String> {
    if track >= state.active_track_count() {
        return Err("target track out of range".to_string());
    }
    let slot = state
        .pattern
        .instrument_slots
        .get(track)
        .ok_or_else(|| "target track instrument slot out of range".to_string())?;
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    if param_idx >= num_params {
        return Err("instrument param out of range".to_string());
    }
    slot.param_node_id(param_idx)
        .ok_or_else(|| "instrument param has no live node identity".to_string())
}

pub(in crate::lisp_host) fn neural_effect_param_id(
    state: &crate::sequencer::SequencerState,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
) -> Result<ParamNodeId, String> {
    if track >= state.active_track_count() {
        return Err("target track out of range".to_string());
    }
    let slot = state
        .pattern
        .effect_chains
        .get(track)
        .and_then(|chain| chain.get(slot_idx))
        .ok_or_else(|| "effect slot out of range".to_string())?;
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    if param_idx >= num_params {
        return Err("effect param out of range".to_string());
    }
    slot.param_node_id(param_idx)
        .ok_or_else(|| "effect param has no live node identity".to_string())
}

pub(in crate::lisp_host) fn neural_neuron_mut(
    network: &mut ProjectNeuralNetwork,
    neuron_idx: usize,
) -> Result<&mut ProjectNeuron, String> {
    normalize_project_neural_network_shape(network)?;
    if neuron_idx >= network.num_neurons {
        return Err("neuron index out of range".to_string());
    }
    network
        .neurons
        .get_mut(neuron_idx)
        .ok_or_else(|| "neuron index out of range".to_string())
}

pub(in crate::lisp_host) fn upsert_neural_instrument_plock(
    network: &mut ProjectNeuralNetwork,
    neuron_idx: usize,
    target_track: usize,
    param_index: usize,
    param_id: ParamNodeId,
    value: f32,
) -> Result<(), String> {
    let network_id = network.id;
    let network_name = network.name.clone();
    let neuron = neural_neuron_mut(network, neuron_idx)?;
    if let Some(existing) = neuron
        .output_overrides
        .instrument
        .iter_mut()
        .find(|entry| entry.target_track == target_track && entry.param_index == param_index)
    {
        existing.param_id = param_id;
        existing.value = value;
    } else {
        neuron
            .output_overrides
            .instrument
            .push(ProjectParamOverride {
                target_track,
                param_id,
                param_index,
                value,
            });
    }
    eprintln!(
        "[neural-plock] instrument network={network_id} name={network_name:?} neuron={neuron_idx} target_track={target_track} param={param_index} logical_id={} node_param_idx={} value={value}",
        param_id.logical_id, param_id.node_param_idx,
    );
    Ok(())
}

pub(in crate::lisp_host) fn upsert_neural_effect_plock(
    network: &mut ProjectNeuralNetwork,
    neuron_idx: usize,
    target_track: usize,
    slot_index: usize,
    param_index: usize,
    param_id: ParamNodeId,
    value: f32,
) -> Result<(), String> {
    let network_id = network.id;
    let network_name = network.name.clone();
    let neuron = neural_neuron_mut(network, neuron_idx)?;
    if let Some(existing) = neuron.output_overrides.effects.iter_mut().find(|entry| {
        entry.target_track == target_track
            && entry.slot_index == slot_index
            && entry.param_index == param_index
    }) {
        existing.param_id = param_id;
        existing.value = value;
    } else {
        neuron
            .output_overrides
            .effects
            .push(ProjectEffectParamOverride {
                target_track,
                slot_index,
                param_id,
                param_index,
                value,
            });
    }
    eprintln!(
        "[neural-plock] effect network={network_id} name={network_name:?} neuron={neuron_idx} target_track={target_track} slot={slot_index} param={param_index} logical_id={} node_param_idx={} value={value}",
        param_id.logical_id, param_id.node_param_idx,
    );
    Ok(())
}

pub(in crate::lisp_host) fn clear_neural_instrument_plock(
    network: &mut ProjectNeuralNetwork,
    neuron_idx: usize,
    target_track: usize,
    param_index: usize,
) -> Result<bool, String> {
    let neuron = neural_neuron_mut(network, neuron_idx)?;
    let before = neuron.output_overrides.instrument.len();
    neuron
        .output_overrides
        .instrument
        .retain(|entry| !(entry.target_track == target_track && entry.param_index == param_index));
    Ok(neuron.output_overrides.instrument.len() != before)
}

pub(in crate::lisp_host) fn clear_neural_effect_plock(
    network: &mut ProjectNeuralNetwork,
    neuron_idx: usize,
    target_track: usize,
    slot_index: usize,
    param_index: usize,
) -> Result<bool, String> {
    let neuron = neural_neuron_mut(network, neuron_idx)?;
    let before = neuron.output_overrides.effects.len();
    neuron.output_overrides.effects.retain(|entry| {
        !(entry.target_track == target_track
            && entry.slot_index == slot_index
            && entry.param_index == param_index)
    });
    Ok(neuron.output_overrides.effects.len() != before)
}

pub fn clear_neural_instrument_plock_by_network_id(
    state: &crate::sequencer::SequencerState,
    network_id: u64,
    neuron_idx: usize,
    target_track: usize,
    param_idx: usize,
) -> Result<bool, String> {
    state.edit_current_neural_networks(|networks| {
        let Some(network) = networks.iter_mut().find(|network| network.id == network_id) else {
            return Err("selected neural network was not found in the current pattern".to_string());
        };
        clear_neural_instrument_plock(network, neuron_idx, target_track, param_idx)
    })
}

pub fn clear_neural_effect_plock_by_network_id(
    state: &crate::sequencer::SequencerState,
    network_id: u64,
    neuron_idx: usize,
    target_track: usize,
    slot_idx: usize,
    param_idx: usize,
) -> Result<bool, String> {
    state.edit_current_neural_networks(|networks| {
        let Some(network) = networks.iter_mut().find(|network| network.id == network_id) else {
            return Err("selected neural network was not found in the current pattern".to_string());
        };
        clear_neural_effect_plock(network, neuron_idx, target_track, slot_idx, param_idx)
    })
}

pub fn set_selected_neural_instrument_plocks(
    state: &crate::sequencer::SequencerState,
    selection: &BTreeSet<SelectedNeuralNeuron>,
    target_track: usize,
    param_idx: usize,
    value: f32,
) -> Result<bool, String> {
    let current_pattern = state.current_scene_index();
    let selected = selection
        .iter()
        .copied()
        .filter(|selected| selected.pattern_idx == current_pattern)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(false);
    }
    let param_id = neural_instrument_param_id(state, target_track, param_idx)?;
    let applied = state.edit_current_neural_networks(|networks| {
        let mut applied = 0_usize;
        for selected in &selected {
            let Some(network) = networks
                .iter_mut()
                .find(|network| network.id == selected.network_id)
            else {
                continue;
            };
            upsert_neural_instrument_plock(
                network,
                selected.neuron_idx,
                target_track,
                param_idx,
                param_id,
                value,
            )?;
            applied += 1;
        }
        Ok(applied)
    })?;
    if applied == 0 {
        return Err("selected neural network was not found in the current pattern".to_string());
    }
    Ok(true)
}

pub fn set_selected_neural_effect_plocks(
    state: &crate::sequencer::SequencerState,
    selection: &BTreeSet<SelectedNeuralNeuron>,
    target_track: usize,
    slot_idx: usize,
    param_idx: usize,
    value: f32,
) -> Result<bool, String> {
    let current_pattern = state.current_scene_index();
    let selected = selection
        .iter()
        .copied()
        .filter(|selected| selected.pattern_idx == current_pattern)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(false);
    }
    let param_id = neural_effect_param_id(state, target_track, slot_idx, param_idx)?;
    let applied = state.edit_current_neural_networks(|networks| {
        let mut applied = 0_usize;
        for selected in &selected {
            let Some(network) = networks
                .iter_mut()
                .find(|network| network.id == selected.network_id)
            else {
                continue;
            };
            upsert_neural_effect_plock(
                network,
                selected.neuron_idx,
                target_track,
                slot_idx,
                param_idx,
                param_id,
                value,
            )?;
            applied += 1;
        }
        Ok(applied)
    })?;
    if applied == 0 {
        return Err("selected neural network was not found in the current pattern".to_string());
    }
    Ok(true)
}

pub fn selected_neural_instrument_plock_value(
    state: &crate::sequencer::SequencerState,
    selection: &BTreeSet<SelectedNeuralNeuron>,
    target_track: usize,
    param_idx: usize,
) -> Option<f32> {
    let current_pattern = state.current_scene_index();
    let param_id = neural_instrument_param_id(state, target_track, param_idx).ok()?;
    let networks = state.current_neural_networks();
    selection
        .iter()
        .filter(|selected| selected.pattern_idx == current_pattern)
        .find_map(|selected| {
            networks
                .iter()
                .find(|network| network.id == selected.network_id)
                .and_then(|network| network.neurons.get(selected.neuron_idx))
                .and_then(|neuron| {
                    neuron.output_overrides.instrument.iter().find_map(|entry| {
                        (entry.target_track == target_track
                            && entry.param_index == param_idx
                            && entry.param_id == param_id)
                            .then_some(entry.value)
                    })
                })
        })
}

pub fn selected_neural_effect_plock_value(
    state: &crate::sequencer::SequencerState,
    selection: &BTreeSet<SelectedNeuralNeuron>,
    target_track: usize,
    slot_idx: usize,
    param_idx: usize,
) -> Option<f32> {
    let current_pattern = state.current_scene_index();
    let param_id = neural_effect_param_id(state, target_track, slot_idx, param_idx).ok()?;
    let networks = state.current_neural_networks();
    selection
        .iter()
        .filter(|selected| selected.pattern_idx == current_pattern)
        .find_map(|selected| {
            networks
                .iter()
                .find(|network| network.id == selected.network_id)
                .and_then(|network| network.neurons.get(selected.neuron_idx))
                .and_then(|neuron| {
                    neuron.output_overrides.effects.iter().find_map(|entry| {
                        (entry.target_track == target_track
                            && entry.slot_index == slot_idx
                            && entry.param_index == param_idx
                            && entry.param_id == param_id)
                            .then_some(entry.value)
                    })
                })
        })
}

pub(in crate::lisp_host) fn neural_network_to_value(network: &ProjectNeuralNetwork) -> EValue {
    let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
    map.insert("id".to_string(), lisp_number(network.id as f64));
    map.insert("name".to_string(), lisp_string(network.name.clone()));
    map.insert("enabled".to_string(), lisp_bool(network.enabled));
    map.insert(
        "num-neurons".to_string(),
        lisp_number(network.num_neurons as f64),
    );
    map.insert(
        "reset-bars".to_string(),
        lisp_number(network.reset_interval_bars as f64),
    );
    map.insert(
        "energy-decay".to_string(),
        lisp_number(network.energy_decay as f64),
    );
    map.insert("max-poly".to_string(), lisp_number(network.max_poly as f64));
    map.insert(
        "max-poly-selection".to_string(),
        lisp_string(network.max_poly_selection.as_str().to_string()),
    );
    map.insert(
        "weights".to_string(),
        lisp_value(lisp_list(
            network
                .weights
                .iter()
                .map(|row| {
                    lisp_list(
                        row.iter()
                            .map(|value| EValue::Number(*value as f64))
                            .collect(),
                    )
                })
                .collect(),
        )),
    );
    map.insert(
        "neurons".to_string(),
        lisp_value(lisp_list(
            network
                .neurons
                .iter()
                .enumerate()
                .map(|(idx, neuron)| neural_neuron_to_value(idx, neuron))
                .collect(),
        )),
    );
    EValue::Map(map)
}

pub fn selected_neural_neurons_to_value(selection: &BTreeSet<SelectedNeuralNeuron>) -> EValue {
    lisp_list(
        selection
            .iter()
            .map(|selected| {
                let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
                map.insert(
                    "pattern".to_string(),
                    lisp_number(selected.pattern_idx as f64),
                );
                map.insert(
                    "network-id".to_string(),
                    lisp_number(selected.network_id as f64),
                );
                map.insert(
                    "neuron".to_string(),
                    lisp_number(selected.neuron_idx as f64),
                );
                EValue::Map(map)
            })
            .collect(),
    )
}

pub(in crate::lisp_host) fn neural_instrument_overrides_to_value(overrides: &[ProjectParamOverride]) -> EValue {
    lisp_list(
        overrides
            .iter()
            .map(|override_param| {
                let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
                map.insert(
                    "target-track".to_string(),
                    lisp_number(override_param.target_track as f64),
                );
                map.insert(
                    "param".to_string(),
                    lisp_number(override_param.param_index as f64),
                );
                map.insert(
                    "value".to_string(),
                    lisp_number(override_param.value as f64),
                );
                map.insert(
                    "logical-id".to_string(),
                    lisp_number(override_param.param_id.logical_id as f64),
                );
                map.insert(
                    "node-param-idx".to_string(),
                    lisp_number(override_param.param_id.node_param_idx as f64),
                );
                EValue::Map(map)
            })
            .collect(),
    )
}

pub(in crate::lisp_host) fn neural_effect_overrides_to_value(overrides: &[ProjectEffectParamOverride]) -> EValue {
    lisp_list(
        overrides
            .iter()
            .map(|override_param| {
                let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
                map.insert(
                    "target-track".to_string(),
                    lisp_number(override_param.target_track as f64),
                );
                map.insert(
                    "slot".to_string(),
                    lisp_number(override_param.slot_index as f64),
                );
                map.insert(
                    "param".to_string(),
                    lisp_number(override_param.param_index as f64),
                );
                map.insert(
                    "value".to_string(),
                    lisp_number(override_param.value as f64),
                );
                map.insert(
                    "logical-id".to_string(),
                    lisp_number(override_param.param_id.logical_id as f64),
                );
                map.insert(
                    "node-param-idx".to_string(),
                    lisp_number(override_param.param_id.node_param_idx as f64),
                );
                EValue::Map(map)
            })
            .collect(),
    )
}

pub(in crate::lisp_host) fn neural_neuron_to_value(idx: usize, neuron: &ProjectNeuron) -> EValue {
    let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
    map.insert("index".to_string(), lisp_number(idx as f64));
    map.insert(
        "route".to_string(),
        lisp_value(
            neuron
                .route
                .map(|track| EValue::Number(track as f64))
                .unwrap_or(EValue::Nil),
        ),
    );
    map.insert(
        "resolution".to_string(),
        lisp_value(EValue::Keyword(
            neuron.resolution_timebase().label().to_string(),
        )),
    );
    map.insert("delay".to_string(), lisp_number(neuron.delay_steps as f64));
    map.insert(
        "threshold".to_string(),
        lisp_number(neuron.threshold as f64),
    );
    map.insert(
        "transpose".to_string(),
        lisp_number(neuron.transpose as f64),
    );
    map.insert(
        "quantize".to_string(),
        lisp_value(
            neuron
                .quantize_timebase()
                .map(|timebase| EValue::Keyword(timebase.label().to_string()))
                .unwrap_or(EValue::Nil),
        ),
    );
    map.insert(
        "dampening".to_string(),
        lisp_number(neuron.dampening_amount as f64),
    );
    map.insert(
        "dampening-recovery".to_string(),
        lisp_number(neuron.dampening_recovery as f64),
    );
    map.insert(
        "instrument-plocks".to_string(),
        lisp_value(neural_instrument_overrides_to_value(
            &neuron.output_overrides.instrument,
        )),
    );
    map.insert(
        "effect-plocks".to_string(),
        lisp_value(neural_effect_overrides_to_value(
            &neuron.output_overrides.effects,
        )),
    );
    EValue::Map(map)
}
