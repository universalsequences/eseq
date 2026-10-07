//! The addressable fields of a native neural network (kind-bindings spec
//! §14.2q): what a host kind setter edits and its history records
//! (`NeuralNetworkPatch`). Each slot is one field of one network (a
//! network setting, one weight cell or the whole matrix, one field of one
//! neuron, or every neuron's threshold at once), so undo and redo restore exactly the edited field and leave
//! every other edit of the network alone (an unrecorded legacy `neural-*`
//! write included).

use super::{NeuralMaxPolySelection, ProjectNeuralNetwork, ProjectNeuron};

/// One field of a neuron, with its value.
#[derive(Clone, Debug, PartialEq)]
pub enum NeuronField {
    /// The track position it plays, or none.
    Route(Option<usize>),
    /// A `Timebase` index.
    Resolution(u8),
    Delay(u32),
    Threshold(f32),
    Transpose(f32),
    /// A `Timebase` index, or none (off).
    Quantize(Option<u8>),
    DampeningAmount(f32),
    DampeningRecovery(f32),
}

impl NeuronField {
    /// Every field's name (`neuron.<name>` in `eseq.kinds`, the `set-neural`
    /// `:field`), in variant order: [`Self::name`] reads it.
    pub const NAMES: [&'static str; 8] = [
        "route",
        "resolution",
        "delay",
        "threshold",
        "transpose",
        "quantize",
        "dampening-amount",
        "dampening-recovery",
    ];

    pub fn name(&self) -> &'static str {
        Self::NAMES[match self {
            Self::Route(_) => 0,
            Self::Resolution(_) => 1,
            Self::Delay(_) => 2,
            Self::Threshold(_) => 3,
            Self::Transpose(_) => 4,
            Self::Quantize(_) => 5,
            Self::DampeningAmount(_) => 6,
            Self::DampeningRecovery(_) => 7,
        }]
    }

    /// The same field with `neuron`'s value.
    fn read(&self, neuron: &ProjectNeuron) -> Self {
        match self {
            Self::Route(_) => Self::Route(neuron.route),
            Self::Resolution(_) => Self::Resolution(neuron.resolution),
            Self::Delay(_) => Self::Delay(neuron.delay_steps),
            Self::Threshold(_) => Self::Threshold(neuron.threshold),
            Self::Transpose(_) => Self::Transpose(neuron.transpose),
            Self::Quantize(_) => Self::Quantize(neuron.quantize),
            Self::DampeningAmount(_) => Self::DampeningAmount(neuron.dampening_amount),
            Self::DampeningRecovery(_) => Self::DampeningRecovery(neuron.dampening_recovery),
        }
    }

    fn write(&self, neuron: &mut ProjectNeuron) {
        match *self {
            Self::Route(route) => neuron.route = route,
            Self::Resolution(resolution) => neuron.resolution = resolution,
            Self::Delay(delay) => neuron.delay_steps = delay,
            Self::Threshold(threshold) => neuron.threshold = threshold,
            Self::Transpose(transpose) => neuron.transpose = transpose,
            Self::Quantize(quantize) => neuron.quantize = quantize,
            Self::DampeningAmount(amount) => neuron.dampening_amount = amount,
            Self::DampeningRecovery(recovery) => neuron.dampening_recovery = recovery,
        }
    }
}

/// One field of a network, with its value.
#[derive(Clone, Debug, PartialEq)]
pub enum NeuralSlot {
    Name(String),
    Enabled(bool),
    ResetBars(f32),
    EnergyDecay(f32),
    MaxPoly(u32),
    MaxPolySelection(NeuralMaxPolySelection),
    /// The whole matrix, rows from-neuron, columns to-neuron.
    Weights(Vec<Vec<f32>>),
    /// One matrix cell.
    Weight {
        from: usize,
        to: usize,
        value: f32,
    },
    Neuron {
        index: usize,
        field: NeuronField,
    },
    /// Every neuron's threshold, by neuron: one edit, so a control that sets
    /// them all records one entry and undo restores each as it was.
    Thresholds(Vec<f32>),
}

impl NeuralSlot {
    /// Which field it is (its value left out): a gesture's merge key part.
    pub fn address(&self) -> String {
        match self {
            Self::Name(_) => "name".to_string(),
            Self::Enabled(_) => "enabled".to_string(),
            Self::ResetBars(_) => "reset-bars".to_string(),
            Self::EnergyDecay(_) => "energy-decay".to_string(),
            Self::MaxPoly(_) => "max-poly".to_string(),
            Self::MaxPolySelection(_) => "max-poly-selection".to_string(),
            Self::Weights(_) => "weights".to_string(),
            Self::Weight { from, to, .. } => format!("weight:{from}:{to}"),
            Self::Neuron { index, field } => format!("neuron:{index}:{}", field.name()),
            Self::Thresholds(_) => "thresholds".to_string(),
        }
    }

    /// The same field with `network`'s value (a cell or a neuron past the
    /// stored shape reads as a fresh one: 0, the neuron defaults).
    pub fn read(&self, network: &ProjectNeuralNetwork) -> Self {
        match self {
            Self::Name(_) => Self::Name(network.name.clone()),
            Self::Enabled(_) => Self::Enabled(network.enabled),
            Self::ResetBars(_) => Self::ResetBars(network.reset_interval_bars),
            Self::EnergyDecay(_) => Self::EnergyDecay(network.energy_decay),
            Self::MaxPoly(_) => Self::MaxPoly(network.max_poly),
            Self::MaxPolySelection(_) => Self::MaxPolySelection(network.max_poly_selection),
            Self::Weights(_) => Self::Weights(network.shaped_weights()),
            Self::Weight { from, to, .. } => Self::Weight {
                from: *from,
                to: *to,
                value: (network.weights.get(*from))
                    .and_then(|row| row.get(*to))
                    .copied()
                    .unwrap_or(0.0),
            },
            Self::Neuron { index, field } => Self::Neuron {
                index: *index,
                field: field.read(&network.neurons.get(*index).cloned().unwrap_or_default()),
            },
            Self::Thresholds(_) => Self::Thresholds(
                (0..network.num_neurons)
                    .map(|index| {
                        (network.neurons.get(index))
                            .map_or(ProjectNeuron::default().threshold, |neuron| {
                                neuron.threshold
                            })
                    })
                    .collect(),
            ),
        }
    }

    /// Write the value into `network` (its shape normalized first, as the
    /// natives do before a cell or neuron edit). A neuron or cell index past
    /// the network's neuron count, or a matrix of another size, is an error
    /// that changes nothing (a replay onto a network that changed shape).
    pub fn write(&self, network: &mut ProjectNeuralNetwork) -> Result<(), String> {
        let size = network.num_neurons;
        let in_range = |index: usize| {
            (index < size)
                .then_some(())
                .ok_or_else(|| format!("neuron {index} is past the network's {size} neurons"))
        };
        match self {
            Self::Name(name) => network.name.clone_from(name),
            Self::Enabled(enabled) => network.enabled = *enabled,
            Self::ResetBars(bars) => network.reset_interval_bars = *bars,
            Self::EnergyDecay(decay) => network.energy_decay = *decay,
            Self::MaxPoly(poly) => network.max_poly = *poly,
            Self::MaxPolySelection(selection) => network.max_poly_selection = *selection,
            Self::Weights(weights) => {
                if weights.len() != size || weights.iter().any(|row| row.len() != size) {
                    return Err(format!("the weights are not {size} by {size}"));
                }
                network.weights.clone_from(weights);
                network.normalize_shape();
            }
            Self::Weight { from, to, value } => {
                in_range(*from)?;
                in_range(*to)?;
                network.normalize_shape();
                network.weights[*from][*to] = *value;
            }
            Self::Neuron { index, field } => {
                in_range(*index)?;
                network.normalize_shape();
                field.write(&mut network.neurons[*index]);
            }
            Self::Thresholds(thresholds) => {
                if thresholds.len() != size {
                    return Err(format!("the thresholds are not {size}"));
                }
                network.normalize_shape();
                for (neuron, threshold) in network.neurons.iter_mut().zip(thresholds) {
                    neuron.threshold = *threshold;
                }
            }
        }
        Ok(())
    }

    pub fn retained_bytes(&self) -> usize {
        match self {
            Self::Name(name) => name.capacity(),
            Self::Weights(rows) => rows
                .iter()
                .map(|row| row.capacity() * std::mem::size_of::<f32>())
                .sum(),
            Self::Thresholds(thresholds) => thresholds.capacity() * std::mem::size_of::<f32>(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(size: usize) -> ProjectNeuralNetwork {
        ProjectNeuralNetwork {
            num_neurons: size,
            weights: vec![vec![0.0; size]; size],
            neurons: vec![ProjectNeuron::default(); size],
            ..ProjectNeuralNetwork::default()
        }
    }

    #[test]
    fn writes_past_the_networks_shape_fail_and_change_nothing() {
        let mut net = network(2);
        let before = net.clone();
        let cell = NeuralSlot::Weight {
            from: 0,
            to: 2,
            value: 1.0,
        };
        assert!(cell.write(&mut net).is_err());
        let neuron = NeuralSlot::Neuron {
            index: 2,
            field: NeuronField::Delay(3),
        };
        assert!(neuron.write(&mut net).is_err());
        assert!(NeuralSlot::Weights(vec![vec![1.0; 3]; 3])
            .write(&mut net)
            .is_err());
        assert!(NeuralSlot::Thresholds(vec![0.5; 3])
            .write(&mut net)
            .is_err());
        assert_eq!(net, before);
        let cell = NeuralSlot::Weight {
            from: 1,
            to: 0,
            value: 0.5,
        };
        assert!(cell.write(&mut net).is_ok());
        assert_eq!(net.weights[1][0], 0.5);
    }

    #[test]
    fn neuron_field_names_are_distinct() {
        let mut names = NeuronField::NAMES.to_vec();
        names.dedup();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), NeuronField::NAMES.len());
    }
}
