use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::neural::ParamNodeId;
use crate::sequencer::MAX_STEPS;

/// Baseline storage capacity for per-slot defaults, p-locks, and node mappings.
/// Custom instruments include generated host-modulation controls in addition to
/// their declared DGen params, so dense synths can exceed 128 parameters.
pub const MAX_SLOT_PARAMS: usize = 512;

/// Number of fixed built-in effect slots. Built-ins are now ordinary inserts,
/// so track effect chains start at slot 0.
pub const BUILTIN_SLOT_COUNT: usize = 0;

/// NaN sentinel stored as bits — means "no p-lock override".
const NAN_BITS: u32 = f32::NAN.to_bits();

// ── ParamKind ──

#[derive(Clone, Debug)]
pub enum ParamKind {
    Continuous { unit: Option<String> }, // e.g., "Hz", "ms", "%"
    Boolean,                             // 0.0 = off, 1.0 = on
    Enum { labels: Vec<String> },        // value = index as f32
}

#[derive(Clone, Debug)]
pub enum HostControl {
    FxSidechain { input_channel: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamUiMetadata {
    pub group: Option<String>,
    pub env: Option<String>,
    pub role: Option<String>,
}

impl ParamUiMetadata {
    pub fn new(group: Option<String>, env: Option<String>, role: Option<String>) -> Option<Self> {
        if group.is_none() && env.is_none() && role.is_none() {
            None
        } else {
            Some(Self { group, env, role })
        }
    }
}

#[derive(Clone, Debug)]
pub struct InstrumentModulatorDescriptor {
    pub slot: usize,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct InstrumentModulationTarget {
    pub base_param_idx: usize,
    pub source_param_idx: Option<usize>,
    pub modulator_slot: usize,
    pub depth_param_idx: usize,
    pub active_param_idx: Option<usize>,
    pub depth_min: f32,
    pub depth_max: f32,
    pub depth_unit: Option<String>,
}

// ── ParamScaling ──

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamScaling {
    Linear,
    Exponential, // log-space steps: ideal for frequency-like params
}

// ── ParamDescriptor ──

#[derive(Clone, Debug)]
pub struct ParamDescriptor {
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub kind: ParamKind,
    pub scaling: ParamScaling,
    pub node_param_idx: u32,  // index into audio node's state array
    pub node_param_span: u32, // number of contiguous state cells that share this value
    pub host_control: Option<HostControl>,
    pub ui_metadata: Option<ParamUiMetadata>,
}

impl ParamDescriptor {
    /// Step size for +/- adjustment.
    pub fn increment(&self, current_val: f32) -> f32 {
        match &self.kind {
            ParamKind::Boolean | ParamKind::Enum { .. } => 1.0,
            ParamKind::Continuous { .. } => match self.scaling {
                ParamScaling::Linear => (self.max - self.min) * 0.01,
                ParamScaling::Exponential => {
                    let step = current_val.abs() * 0.02;
                    let floor = (self.max - self.min) * 0.001;
                    step.max(floor)
                }
            },
        }
    }

    pub fn clamp(&self, val: f32) -> f32 {
        val.clamp(self.min, self.max)
    }

    /// Normalize value to 0.0..1.0 for display (linear or log-space).
    pub fn normalize(&self, val: f32) -> f32 {
        let range = self.max - self.min;
        if range <= 0.0 {
            return 0.0;
        }
        match self.scaling {
            ParamScaling::Linear => ((val - self.min) / range).clamp(0.0, 1.0),
            ParamScaling::Exponential => {
                if self.min <= 0.0 || self.max <= 0.0 {
                    return ((val - self.min) / range).clamp(0.0, 1.0);
                }
                let log_min = self.min.ln();
                let log_max = self.max.ln();
                let log_range = log_max - log_min;
                if log_range <= 0.0 {
                    return 0.0;
                }
                ((val.max(self.min).ln() - log_min) / log_range).clamp(0.0, 1.0)
            }
        }
    }

    /// Convert a normalized 0.0..1.0 control value into the stored parameter domain.
    pub fn denormalize(&self, normalized: f32) -> f32 {
        let normalized = normalized.clamp(0.0, 1.0);
        match &self.kind {
            ParamKind::Boolean => {
                if normalized >= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            ParamKind::Enum { .. } => {
                let range = self.max - self.min;
                if range <= 0.0 {
                    self.min
                } else {
                    (self.min + normalized * range)
                        .round()
                        .clamp(self.min, self.max)
                }
            }
            ParamKind::Continuous { .. } => match self.scaling {
                ParamScaling::Linear => self.min + normalized * (self.max - self.min),
                ParamScaling::Exponential => {
                    if self.min <= 0.0 || self.max <= 0.0 {
                        self.min + normalized * (self.max - self.min)
                    } else {
                        let log_min = self.min.ln();
                        let log_max = self.max.ln();
                        (log_min + normalized * (log_max - log_min)).exp()
                    }
                }
            },
        }
    }

    pub fn is_boolean(&self) -> bool {
        matches!(self.kind, ParamKind::Boolean)
    }

    pub fn is_enum(&self) -> bool {
        matches!(self.kind, ParamKind::Enum { .. })
    }

    /// Returns true if this param is displayed as percentage but stored 0.0-1.0.
    pub fn is_percent(&self) -> bool {
        matches!(&self.kind, ParamKind::Continuous { unit: Some(u) } if u == "%")
    }

    /// Convert user-entered value to stored value (handles % → /100).
    pub fn user_input_to_stored(&self, val: f32) -> f32 {
        if self.is_percent() {
            val / 100.0
        } else {
            val
        }
    }

    /// Convert a stored value to the display/user-edit domain.
    pub fn stored_to_user(&self, val: f32) -> f32 {
        if self.is_percent() {
            val * 100.0
        } else {
            val
        }
    }

    fn accepts_migrated_value_from(&self, old: &ParamDescriptor, value: f32) -> bool {
        param_kinds_are_compatible(&old.kind, &self.kind)
            && value.is_finite()
            && value >= self.min
            && value <= self.max
    }

    pub fn format_value(&self, val: f32) -> String {
        match &self.kind {
            ParamKind::Boolean => {
                if val > 0.5 {
                    "ON".to_string()
                } else {
                    "OFF".to_string()
                }
            }
            ParamKind::Enum { labels } => {
                let idx = val.round() as usize;
                labels
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(|| format!("{}", idx))
            }
            ParamKind::Continuous { unit } => {
                let display_val = self.stored_to_user(val);
                match unit.as_deref() {
                    Some("Hz") => format!("{:.0} Hz", display_val),
                    Some("ms") => format!("{:.0} ms", display_val),
                    Some("%") => format!("{:.0}%", display_val),
                    Some(u) => format!("{:.2} {}", display_val, u),
                    None => format!("{:.2}", display_val),
                }
            }
        }
    }
}

fn param_kinds_are_compatible(old: &ParamKind, new: &ParamKind) -> bool {
    match (old, new) {
        (ParamKind::Continuous { unit: old_unit }, ParamKind::Continuous { unit: new_unit }) => {
            old_unit == new_unit
        }
        (ParamKind::Boolean, ParamKind::Boolean) => true,
        (ParamKind::Enum { labels: old_labels }, ParamKind::Enum { labels: new_labels }) => {
            old_labels == new_labels
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{
        EffectDescriptor, EffectSlotSnapshot, EffectSlotState, ParamDescriptor, ParamKind,
        ParamScaling,
    };
    use crate::neural::ParamNodeId;

    #[test]
    fn denormalize_boolean_snaps_to_zero_or_one() {
        let desc = ParamDescriptor {
            name: "enabled".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            kind: ParamKind::Boolean,
            scaling: ParamScaling::Linear,
            node_param_idx: 0,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        };

        assert_eq!(desc.denormalize(0.49), 0.0);
        assert_eq!(desc.denormalize(0.5), 1.0);
    }

    #[test]
    fn denormalize_exponential_matches_midpoint_in_log_space() {
        let desc = ParamDescriptor {
            name: "cutoff".to_string(),
            min: 20.0,
            max: 20_000.0,
            default: 1_000.0,
            kind: ParamKind::Continuous {
                unit: Some("Hz".to_string()),
            },
            scaling: ParamScaling::Exponential,
            node_param_idx: 0,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        };

        let value = desc.denormalize(0.5);
        assert!((value - 632.4555).abs() < 0.1, "value was {value}");
    }

    #[test]
    fn dense_slots_preserve_params_past_legacy_capacity() {
        let params: Vec<ParamDescriptor> = (0..150)
            .map(|idx| ParamDescriptor {
                name: format!("p{idx}"),
                min: -10.0,
                max: 10.0,
                default: idx as f32,
                kind: ParamKind::Continuous { unit: None },
                scaling: ParamScaling::Linear,
                node_param_idx: 1_000 + idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            })
            .collect();
        let desc = EffectDescriptor {
            name: "dense".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params,
        };

        let slot = EffectSlotState::new(&desc, 100);
        slot.defaults.set(149, 7.5);
        slot.set_plock(3, 149, -4.25);

        assert_eq!(slot.defaults.get(149), 7.5);
        assert_eq!(slot.plocks.get(3, 149), Some(-4.25));
        assert_eq!(slot.resolve_node_idx(149), 1_149);
    }

    #[test]
    fn sync_to_descriptor_rebinds_loaded_plock_ids_to_live_node_id() {
        let desc = EffectDescriptor {
            name: "test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![ParamDescriptor {
                name: "cutoff".to_string(),
                min: 0.0,
                max: 1.0,
                default: 0.5,
                kind: ParamKind::Continuous { unit: None },
                scaling: ParamScaling::Linear,
                node_param_idx: 15,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }],
        };
        let mut snapshot = EffectSlotSnapshot {
            node_id: 10,
            modulator_node_id: 0,
            num_params: 1,
            defaults: vec![0.2],
            plocks: (0..crate::sequencer::MAX_STEPS)
                .map(|_| vec![None])
                .collect(),
            plock_param_ids: (0..crate::sequencer::MAX_STEPS)
                .map(|_| vec![None])
                .collect(),
            param_node_indices: vec![15],
            param_node_spans: vec![1],
            ir: None,
        };
        snapshot.plocks[3][0] = Some(0.9);
        snapshot.plock_param_ids[3][0] = Some(ParamNodeId {
            logical_id: 10,
            node_param_idx: 15,
        });

        snapshot.sync_to_descriptor(&desc, 42);

        assert_eq!(snapshot.plocks[3][0], Some(0.9));
        assert_eq!(
            snapshot.plock_param_ids[3][0],
            Some(ParamNodeId {
                logical_id: 42,
                node_param_idx: 15,
            })
        );
    }

    #[test]
    fn empty_slot_apply_descriptor_preserves_dense_param_mappings() {
        let params: Vec<ParamDescriptor> = (0..150)
            .map(|idx| ParamDescriptor {
                name: format!("p{idx}"),
                min: -10.0,
                max: 10.0,
                default: idx as f32,
                kind: ParamKind::Continuous { unit: None },
                scaling: ParamScaling::Linear,
                node_param_idx: 2_000 + idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            })
            .collect();
        let desc = EffectDescriptor {
            name: "dense".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params,
        };

        let slot = EffectSlotState::empty();
        slot.apply_descriptor(&desc, 100);
        slot.defaults.set(149, 6.25);
        slot.set_plock(7, 149, -3.5);

        assert_eq!(slot.defaults.get(149), 6.25);
        assert_eq!(slot.plocks.get(7, 149), Some(-3.5));
        assert_eq!(slot.resolve_node_idx(149), 2_149);
    }

    #[test]
    fn sync_descriptor_preserves_existing_defaults_and_plocks() {
        let original = EffectDescriptor {
            name: "orig".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "a".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.1,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 3,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "b".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.2,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 4,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        let rebound = EffectDescriptor {
            name: "rebound".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "a".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.9,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 10,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "b".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.8,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 11,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };

        let slot = EffectSlotState::new(&original, 100);
        slot.defaults.set(0, 0.42);
        slot.defaults.set(1, 0.73);
        slot.set_plock(3, 0, 0.33);
        slot.set_plock(4, 1, 0.66);

        slot.sync_descriptor(&rebound, 200);

        assert_eq!(slot.node_id.load(Ordering::Relaxed), 200);
        assert_eq!(slot.resolve_node_idx(0), 10);
        assert_eq!(slot.resolve_node_idx(1), 11);
        assert_eq!(slot.defaults.get(0), 0.42);
        assert_eq!(slot.defaults.get(1), 0.73);
        assert_eq!(slot.plocks.get(3, 0), Some(0.33));
        assert_eq!(slot.plocks.get(4, 1), Some(0.66));
    }

    #[test]
    fn sync_descriptor_by_param_name_preserves_reordered_compatible_values() {
        let original = EffectDescriptor {
            name: "orig".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "cutoff".to_string(),
                    min: 20.0,
                    max: 20_000.0,
                    default: 1_000.0,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: 3,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "gain".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 4,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        let rebound = EffectDescriptor {
            name: "rebound".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "gain".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.25,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 10,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "cutoff".to_string(),
                    min: 20.0,
                    max: 20_000.0,
                    default: 800.0,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: 11,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };

        let slot = EffectSlotState::new(&original, 100);
        slot.defaults.set(0, 880.0);
        slot.defaults.set(1, 0.73);
        slot.set_plock(3, 0, 440.0);
        slot.set_plock(4, 1, 0.66);

        slot.sync_descriptor_by_param_name(&original, &rebound, 200);

        assert_eq!(slot.node_id.load(Ordering::Relaxed), 200);
        assert_eq!(slot.resolve_node_idx(0), 10);
        assert_eq!(slot.resolve_node_idx(1), 11);
        assert_eq!(slot.defaults.get(0), 0.73);
        assert_eq!(slot.defaults.get(1), 880.0);
        assert_eq!(slot.plocks.get(4, 0), Some(0.66));
        assert_eq!(slot.plocks.get(3, 1), Some(440.0));
    }

    #[test]
    fn sync_descriptor_by_param_name_drops_wrong_or_out_of_range_values() {
        let original = EffectDescriptor {
            name: "orig".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "mode".to_string(),
                    min: 0.0,
                    max: 2.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 3,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "gain".to_string(),
                    min: 0.0,
                    max: 10.0,
                    default: 0.5,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 4,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        let rebound = EffectDescriptor {
            name: "rebound".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "mode".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Enum {
                        labels: vec!["x".to_string(), "y".to_string()],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 10,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "gain".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.25,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 11,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };

        let slot = EffectSlotState::new(&original, 100);
        slot.defaults.set(0, 2.0);
        slot.defaults.set(1, 7.0);
        slot.set_plock(3, 0, 1.0);
        slot.set_plock(4, 1, 8.0);

        slot.sync_descriptor_by_param_name(&original, &rebound, 200);

        assert_eq!(slot.defaults.get(0), 1.0);
        assert_eq!(slot.defaults.get(1), 0.25);
        assert_eq!(slot.plocks.get(3, 0), None);
        assert_eq!(slot.plocks.get(4, 1), None);
    }

    #[test]
    fn force_enabled_default_sets_enabled_without_disturbing_other_params() {
        let desc = EffectDescriptor {
            name: "custom".to_string(),
            input_channels: 0,
            output_channels: 1,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "cutoff".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.25,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 12,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "enabled".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::lisp_host::DGEN_ENABLED_PARAM_IDX as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        let slot = EffectSlotState::new(&desc, 100);
        slot.defaults.set(0, 0.77);
        slot.defaults.set(1, 0.0);

        assert_eq!(slot.force_enabled_default(&desc), Some(1));
        assert_eq!(slot.defaults.get(0), 0.77);
        assert_eq!(slot.defaults.get(1), 1.0);
    }

    #[test]
    fn lisp_manifest_params_address_dgen_wrapper_state() {
        let desc = EffectDescriptor::from_lisp_manifest(
            "custom",
            &[crate::lisp_host::DGenParam {
                name: "cutoff".to_string(),
                cell_id: 12,
                cell_span: 4,
                default: 1000.0,
                min: 20.0,
                max: 20_000.0,
                unit: Some("Hz".to_string()),
                hidden: false,
                group: None,
                env: None,
                role: None,
            }],
            0,
            1,
        );

        assert_eq!(
            desc.params[0].node_param_idx,
            (crate::lisp_host::HEADER_SLOTS + 12) as u32
        );
    }

    #[test]
    fn lisp_manifest_params_preserve_visible_ui_metadata_only() {
        let desc = EffectDescriptor::from_lisp_manifest(
            "custom",
            &[
                crate::lisp_host::DGenParam {
                    name: "amp_attack".to_string(),
                    cell_id: 2,
                    cell_span: 1,
                    default: 0.01,
                    min: 0.0,
                    max: 2.0,
                    unit: None,
                    hidden: false,
                    group: Some("amp".to_string()),
                    env: Some("amp_env".to_string()),
                    role: Some("attack".to_string()),
                },
                crate::lisp_host::DGenParam {
                    name: "hidden_release".to_string(),
                    cell_id: 3,
                    cell_span: 1,
                    default: 0.1,
                    min: 0.0,
                    max: 2.0,
                    unit: None,
                    hidden: true,
                    group: Some("amp".to_string()),
                    env: Some("amp_env".to_string()),
                    role: Some("release".to_string()),
                },
            ],
            0,
            1,
        );

        assert_eq!(
            desc.params.len(),
            2,
            "visible param plus generated enabled param"
        );
        let ui = desc.params[0]
            .ui_metadata
            .as_ref()
            .expect("visible DGen param keeps metadata");
        assert_eq!(ui.group.as_deref(), Some("amp"));
        assert_eq!(ui.env.as_deref(), Some("amp_env"));
        assert_eq!(ui.role.as_deref(), Some("attack"));
        assert_eq!(desc.params[1].name, "enabled");
        assert!(desc.params[1].ui_metadata.is_none());
    }

    #[test]
    fn builtin_dynamics_exposes_macro_params() {
        let desc = EffectDescriptor::builtin_444_compressor();
        let names: Vec<&str> = desc.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "mode", "amount", "attack", "release", "low cut", "drive", "input", "output",
                "mix", "enabled", "knee"
            ]
        );
        assert_eq!(desc.params[0].default, 1.0);
        assert_eq!(desc.params[1].default, 0.62);
        assert_eq!(
            desc.params[1].node_param_idx,
            crate::dynamics::DYNAMICS_PARAM_AMOUNT as u32
        );
        match &desc.params[0].kind {
            ParamKind::Enum { labels } => {
                assert_eq!(
                    labels,
                    &vec!["Glue".to_string(), "404".to_string(), "Hybrid".to_string()]
                );
            }
            other => panic!("mode should be enum, got {other:?}"),
        }
        match &desc.params[3].kind {
            ParamKind::Enum { labels } => {
                assert_eq!(
                    labels,
                    &vec![
                        "fast".to_string(),
                        "bounce".to_string(),
                        "auto".to_string(),
                        "smooth".to_string()
                    ]
                );
            }
            other => panic!("release should be enum, got {other:?}"),
        }
    }

    #[test]
    fn builtin_compressor_names_are_canonical_and_legacy_dynamics_loads() {
        assert_eq!(
            EffectDescriptor::builtin_insert_names(),
            &[
                "Filter",
                "Delay",
                "Str8 Delay",
                "DJ Mixer",
                "Reverb",
                "444 Compressor",
                "Glue Compressor",
                "Compressor",
                "Limiter",
                "Tape"
            ]
        );
        assert_eq!(
            EffectDescriptor::canonical_builtin_insert_name("404 Compressor"),
            Some("444 Compressor")
        );
        assert_eq!(
            EffectDescriptor::canonical_builtin_insert_name("Dynamics"),
            Some("444 Compressor")
        );
        assert_eq!(
            EffectDescriptor::builtin_insert("Glue Compressor")
                .unwrap()
                .params[0]
                .default,
            0.0
        );
        assert_eq!(
            EffectDescriptor::canonical_builtin_insert_name("Utility Compressor"),
            Some("Compressor")
        );
        assert_eq!(
            EffectDescriptor::canonical_builtin_insert_name("Utility Limiter"),
            Some("Limiter")
        );
        assert_eq!(
            EffectDescriptor::builtin_insert("Limiter").unwrap().params[1].default,
            -0.3
        );
        assert_eq!(
            EffectDescriptor::builtin_insert("str8 delay").unwrap().name,
            "Str8 Delay"
        );
        assert_eq!(
            EffectDescriptor::builtin_insert("dj mixer").unwrap().name,
            "DJ Mixer"
        );
    }

    #[test]
    fn default_full_chain_contains_only_empty_insert_slots() {
        let chain = EffectDescriptor::default_full_chain();
        assert_eq!(chain.len(), crate::lisp_host::MAX_CUSTOM_FX);
        assert!(chain.iter().all(|desc| desc.name.is_empty()));
        assert!(chain.iter().all(|desc| desc.params.is_empty()));
    }

    #[test]
    fn manually_inserted_filter_and_delay_default_to_enabled() {
        let filter = EffectDescriptor::builtin_insert("Filter").unwrap();
        let delay = EffectDescriptor::builtin_insert("Delay").unwrap();
        let filter_enabled = filter
            .params
            .iter()
            .find(|param| param.name == "enabled")
            .unwrap();
        let delay_enabled = delay
            .params
            .iter()
            .find(|param| param.name == "enabled")
            .unwrap();

        assert_eq!(filter_enabled.default, 1.0);
        assert_eq!(delay_enabled.default, 1.0);
    }

    #[test]
    fn builtin_filter_exposes_effect_local_cutoff_modulation() {
        let desc = EffectDescriptor::builtin_filter();

        assert_eq!(desc.input_channels, 2 + crate::voice_modulator::NUM_OUTPUTS);
        assert_eq!(
            desc.instrument_modulators
                .iter()
                .map(|modulator| (modulator.slot, modulator.label.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "Mod 1"), (2, "Mod 2"), (3, "Mod 3"), (4, "Mod 4")]
        );

        let mod1_source = desc
            .params
            .iter()
            .find(|param| param.name == "mod1_source")
            .expect("filter should expose effect-local Mod 1 source");
        assert_eq!(mod1_source.default, 0.0);
        match &mod1_source.kind {
            ParamKind::Enum { labels } => {
                assert_eq!(
                    labels.iter().map(String::as_str).collect::<Vec<_>>(),
                    vec!["off", "lfo", "env", "rand", "drift", "ext1", "ext2", "ext3", "ext4"]
                );
            }
            other => panic!("mod1_source should be enum, got {other:?}"),
        }

        let target_names = desc
            .instrument_modulation_targets
            .iter()
            .map(|target| {
                (
                    desc.params[target.base_param_idx].name.as_str(),
                    target.modulator_slot,
                    desc.params[target.depth_param_idx].name.as_str(),
                    target.depth_min,
                    target.depth_max,
                    target.depth_unit.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            target_names,
            vec![
                ("cutoff", 1, "mod cutoff slot 1 amt", -4.0, 4.0, Some("oct")),
                ("cutoff", 2, "mod cutoff slot 2 amt", -4.0, 4.0, Some("oct")),
                ("cutoff", 3, "mod cutoff slot 3 amt", -4.0, 4.0, Some("oct")),
                ("cutoff", 4, "mod cutoff slot 4 amt", -4.0, 4.0, Some("oct")),
            ]
        );
    }

    #[test]
    fn builtin_str8_delay_exposes_ableton_style_params() {
        let desc = EffectDescriptor::builtin_str8_delay();
        let names: Vec<&str> = desc.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            &names[..16],
            vec![
                "enabled",
                "wet",
                "feedback",
                "left sync",
                "left div",
                "left offset",
                "left time",
                "right sync",
                "right div",
                "right offset",
                "right time",
                "filter freq",
                "filter width",
                "mod rate",
                "mod amount",
                "mod phase",
            ]
        );
        assert_eq!(desc.input_channels, 2 + crate::voice_modulator::NUM_OUTPUTS);
        assert_eq!(
            desc.instrument_modulators
                .iter()
                .map(|modulator| (modulator.slot, modulator.label.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "Mod 1"), (2, "Mod 2"), (3, "Mod 3"), (4, "Mod 4")]
        );
        assert_eq!(desc.params[1].default, 0.5);
        assert_eq!(desc.params[3].default, 1.0);
        assert_eq!(desc.params[4].default, 6.0);
        assert_eq!(desc.params[5].min, -0.5);
        assert_eq!(desc.params[5].max, 0.5);
        assert_eq!(
            desc.params[11].node_param_idx,
            crate::str8_delay::STR8_DELAY_PARAM_FILTER_FREQ as u32
        );
        assert_eq!(desc.params[12].max, 6.0);
        assert_eq!(desc.params[12].default, 4.5);
        match &desc.params[8].kind {
            ParamKind::Enum { labels } => assert_eq!(labels[6], "1/4"),
            other => panic!("right div should be enum, got {other:?}"),
        }

        let target_names = desc
            .instrument_modulation_targets
            .iter()
            .map(|target| {
                (
                    desc.params[target.base_param_idx].name.as_str(),
                    target.modulator_slot,
                    desc.params[target.depth_param_idx].name.as_str(),
                    target.depth_min,
                    target.depth_max,
                    target.depth_unit.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            target_names,
            vec![
                (
                    "left time",
                    1,
                    "mod time slot 1 amt",
                    -1000.0,
                    1000.0,
                    Some("ms")
                ),
                (
                    "left time",
                    2,
                    "mod time slot 2 amt",
                    -1000.0,
                    1000.0,
                    Some("ms")
                ),
                (
                    "left time",
                    3,
                    "mod time slot 3 amt",
                    -1000.0,
                    1000.0,
                    Some("ms")
                ),
                (
                    "left time",
                    4,
                    "mod time slot 4 amt",
                    -1000.0,
                    1000.0,
                    Some("ms")
                ),
                ("wet", 1, "mod wet slot 1 amt", -1.0, 1.0, Some("%")),
                ("wet", 2, "mod wet slot 2 amt", -1.0, 1.0, Some("%")),
                ("wet", 3, "mod wet slot 3 amt", -1.0, 1.0, Some("%")),
                ("wet", 4, "mod wet slot 4 amt", -1.0, 1.0, Some("%")),
                ("feedback", 1, "mod feedback slot 1 amt", -0.95, 0.95, None),
                ("feedback", 2, "mod feedback slot 2 amt", -0.95, 0.95, None),
                ("feedback", 3, "mod feedback slot 3 amt", -0.95, 0.95, None),
                ("feedback", 4, "mod feedback slot 4 amt", -0.95, 0.95, None),
                (
                    "filter freq",
                    1,
                    "mod cutoff slot 1 amt",
                    -4.0,
                    4.0,
                    Some("oct")
                ),
                (
                    "filter freq",
                    2,
                    "mod cutoff slot 2 amt",
                    -4.0,
                    4.0,
                    Some("oct")
                ),
                (
                    "filter freq",
                    3,
                    "mod cutoff slot 3 amt",
                    -4.0,
                    4.0,
                    Some("oct")
                ),
                (
                    "filter freq",
                    4,
                    "mod cutoff slot 4 amt",
                    -4.0,
                    4.0,
                    Some("oct")
                ),
            ]
        );
    }

    #[test]
    fn builtin_dj_mixer_exposes_sp_style_params() {
        let desc = EffectDescriptor::builtin_dj_mixer();
        let names: Vec<&str> = desc.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["enabled", "speed", "length", "loop"]);
        assert_eq!(desc.input_channels, 2);
        assert_eq!(desc.output_channels, 2);
        assert_eq!(desc.params[0].default, 1.0);
        assert_eq!(desc.params[1].min, -1.0);
        assert_eq!(desc.params[1].max, 1.0);
        assert_eq!(desc.params[1].default, 1.0);
        assert_eq!(desc.params[2].min, 0.012);
        assert_eq!(desc.params[2].max, 0.230);
        assert_eq!(desc.params[2].default, 0.230);
        assert_eq!(desc.params[3].default, 0.0);
        assert_eq!(
            desc.params[2].node_param_idx,
            crate::dj_mixer::DJ_MIXER_PARAM_LENGTH_SEC as u32
        );
    }

    #[test]
    fn builtin_sampler_exposes_inline_modulation_metadata() {
        let desc = EffectDescriptor::builtin_sampler();
        let modulators = desc
            .instrument_modulators
            .iter()
            .map(|modulator| (modulator.slot, modulator.label.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            modulators,
            vec![(1, "Mod 1"), (2, "Mod 2"), (3, "Mod 3"), (4, "Mod 4"),]
        );

        let target_names = desc
            .instrument_modulation_targets
            .iter()
            .map(|target| {
                (
                    desc.params[target.base_param_idx].name.as_str(),
                    target
                        .source_param_idx
                        .and_then(|idx| desc.params.get(idx))
                        .map(|param| param.name.as_str())
                        .unwrap_or("<fixed>"),
                    desc.params[target.depth_param_idx].name.as_str(),
                    target.depth_min,
                    target.depth_max,
                    target.depth_unit.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            target_names.len(),
            crate::sampler::SAMPLER_MOD_LANES_PER_PARAM * 6
        );
        assert_eq!(
            &target_names[..4],
            &[
                ("speed", "mod speed src", "mod speed amt", -8.0, 8.0, None),
                (
                    "speed",
                    "mod speed lane 2 src",
                    "mod speed lane 2 amt",
                    -8.0,
                    8.0,
                    None
                ),
                (
                    "speed",
                    "mod speed lane 3 src",
                    "mod speed lane 3 amt",
                    -8.0,
                    8.0,
                    None
                ),
                (
                    "speed",
                    "mod speed lane 4 src",
                    "mod speed lane 4 amt",
                    -8.0,
                    8.0,
                    None
                ),
            ]
        );
        assert!(
            target_names.iter().any(|target| *target
                == (
                    "scrub",
                    "mod scrub src",
                    "mod scrub amt",
                    -1.0,
                    1.0,
                    Some("%")
                )),
            "sampler scrub modulation depth should use the same percent display domain as scrub: {target_names:?}"
        );
        assert!(
            target_names.iter().any(|target| *target
                == (
                    "sr",
                    "mod sr src",
                    "mod sr amt",
                    -42_100.0,
                    42_100.0,
                    Some("Hz")
                )),
            "sampler sr modulation depth should be exposed in Hz: {target_names:?}"
        );
        assert!(
            target_names.iter().any(|target| *target
                == (
                    "bpm",
                    "mod bpm src",
                    "mod bpm amt",
                    -380.0,
                    380.0,
                    Some("bpm")
                )),
            "sampler bpm modulation depth should be exposed in BPM: {target_names:?}"
        );
        assert_eq!(
            target_names.last(),
            Some(&(
                "end",
                "mod end lane 4 src",
                "mod end lane 4 amt",
                -1.0,
                1.0,
                Some("%")
            ))
        );
        let smooth = desc
            .params
            .iter()
            .find(|param| param.name == "smooth")
            .expect("sampler should expose scrub smooth time");
        assert_eq!(
            smooth.node_param_idx,
            crate::sampler::PARAM_SCRUB_SMOOTH_TIME_MS as u32
        );
        assert_eq!((smooth.min, smooth.max, smooth.default), (0.0, 250.0, 6.0));
    }
}

// ── EffectDescriptor ──

fn sampler_mod_depth_range(destination: &str) -> (f32, f32, Option<String>) {
    match destination {
        "speed" => (-8.0, 8.0, None),
        "scrub" => (-1.0, 1.0, Some("%".to_string())),
        "sr" => (-42_100.0, 42_100.0, Some("Hz".to_string())),
        "bpm" => (-380.0, 380.0, Some("bpm".to_string())),
        "start" | "end" => (-1.0, 1.0, Some("%".to_string())),
        _ => (-1.0, 1.0, None),
    }
}

#[derive(Clone, Debug)]
pub struct EffectDescriptor {
    pub name: String,
    pub params: Vec<ParamDescriptor>,
    pub input_channels: usize,
    pub output_channels: usize,
    pub instrument_modulators: Vec<InstrumentModulatorDescriptor>,
    pub instrument_modulation_targets: Vec<InstrumentModulationTarget>,
}

impl EffectDescriptor {
    pub const BUILTIN_INSERT_PREFIX: &'static str = "builtin:";

    pub fn enabled_param(node_param_idx: u32, default: f32) -> ParamDescriptor {
        ParamDescriptor {
            name: "enabled".to_string(),
            min: 0.0,
            max: 1.0,
            default,
            kind: ParamKind::Boolean,
            scaling: ParamScaling::Linear,
            node_param_idx,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        }
    }

    pub fn builtin_insert_names() -> &'static [&'static str] {
        &[
            "Filter",
            "Delay",
            "Str8 Delay",
            "DJ Mixer",
            "Reverb",
            "444 Compressor",
            "Glue Compressor",
            "Compressor",
            "Limiter",
            "Tape",
        ]
    }

    pub fn builtin_insert_project_name(name: &str) -> Option<String> {
        let canonical = Self::canonical_builtin_insert_name(name)?;
        Some(format!("{}{}", Self::BUILTIN_INSERT_PREFIX, canonical))
    }

    pub fn strip_builtin_insert_project_name(name: &str) -> Option<&str> {
        name.strip_prefix(Self::BUILTIN_INSERT_PREFIX)
            .and_then(Self::canonical_builtin_insert_name)
    }

    pub fn canonical_builtin_insert_name(name: &str) -> Option<&'static str> {
        let trimmed = name.trim();
        if trimmed.eq_ignore_ascii_case("Dynamics") {
            return Some("444 Compressor");
        }
        if trimmed.eq_ignore_ascii_case("404 Compressor") {
            return Some("444 Compressor");
        }
        if trimmed.eq_ignore_ascii_case("Utility Compressor") {
            return Some("Compressor");
        }
        if trimmed.eq_ignore_ascii_case("Utility Limiter") {
            return Some("Limiter");
        }
        Self::builtin_insert_names()
            .iter()
            .copied()
            .find(|builtin| builtin.eq_ignore_ascii_case(trimmed))
    }

    pub fn builtin_insert(name: &str) -> Option<Self> {
        match Self::canonical_builtin_insert_name(name)? {
            "Filter" => Some(Self::builtin_filter()),
            "Delay" => Some(Self::builtin_delay()),
            "Str8 Delay" => Some(Self::builtin_str8_delay()),
            "DJ Mixer" => Some(Self::builtin_dj_mixer()),
            "Reverb" => Some(Self::builtin_reverb_insert()),
            "444 Compressor" => Some(Self::builtin_444_compressor()),
            "Glue Compressor" => Some(Self::builtin_glue_compressor()),
            "Compressor" => Some(Self::builtin_compressor()),
            "Limiter" => Some(Self::builtin_limiter()),
            "Tape" => Some(Self::builtin_tape()),
            _ => None,
        }
    }

    /// Built-in filter effect descriptor.
    pub fn builtin_filter() -> Self {
        let mut desc = Self {
            name: "Filter".to_string(),
            input_channels: 2 + crate::voice_modulator::NUM_OUTPUTS,
            output_channels: 2,
            instrument_modulators: (1..=crate::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                Self::enabled_param(crate::filter::FILTER_PARAM_ENABLED as u32, 1.0),
                ParamDescriptor {
                    name: "mode".to_string(),
                    min: 0.0,
                    max: 3.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "lowpass".to_string(),
                            "highpass".to_string(),
                            "bandpass".to_string(),
                            "notch".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_MODE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "cutoff".to_string(),
                    min: 20.0,
                    max: 20000.0,
                    default: 1000.0,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::filter::FILTER_PARAM_CUTOFF as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "resonance".to_string(),
                    min: 0.5,
                    max: 10.0,
                    default: 1.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_RESONANCE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "drive".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_DRIVE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "wet".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_WET as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "lfo amt".to_string(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_LFO_AMOUNT as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "lfo rate".to_string(),
                    min: 0.01,
                    max: 40.0,
                    default: 1.0,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::filter::FILTER_PARAM_LFO_RATE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "lfo sync".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_LFO_SYNCED as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "lfo div".to_string(),
                    min: 0.0,
                    max: 10.0,
                    default: 6.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "1/32".to_string(),
                            "1/16".to_string(),
                            "1/16t".to_string(),
                            "1/8".to_string(),
                            "1/8t".to_string(),
                            "1/8.".to_string(),
                            "1/4".to_string(),
                            "1/4t".to_string(),
                            "1/4.".to_string(),
                            "1/2".to_string(),
                            "1".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_LFO_DIVISION as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "lfo wave".to_string(),
                    min: 0.0,
                    max: 5.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "sine".to_string(),
                            "tri".to_string(),
                            "saw".to_string(),
                            "ramp".to_string(),
                            "square".to_string(),
                            "s&h".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_LFO_WAVE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "lfo phase".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_LFO_PHASE_OFFSET as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "env amt".to_string(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_ENV_AMOUNT as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "env attack".to_string(),
                    min: 0.1,
                    max: 5000.0,
                    default: 5.0,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::filter::FILTER_PARAM_ENV_ATTACK_MS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "env release".to_string(),
                    min: 1.0,
                    max: 5000.0,
                    default: 120.0,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::filter::FILTER_PARAM_ENV_RELEASE_MS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "slope".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec!["12".to_string(), "24".to_string()],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_SLOPE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        desc.params
            .extend(crate::voice_modulator::effect_param_descriptors());
        let cutoff_idx = desc
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("built-in filter cutoff param should exist");
        let depth_params = [
            crate::filter::FILTER_PARAM_MOD_CUTOFF_DEPTH_1,
            crate::filter::FILTER_PARAM_MOD_CUTOFF_DEPTH_2,
            crate::filter::FILTER_PARAM_MOD_CUTOFF_DEPTH_3,
            crate::filter::FILTER_PARAM_MOD_CUTOFF_DEPTH_4,
        ];
        for (slot, node_param_idx) in depth_params.into_iter().enumerate() {
            let depth_param_idx = desc.params.len();
            desc.params.push(ParamDescriptor {
                name: format!("mod cutoff slot {} amt", slot + 1),
                min: -4.0,
                max: 4.0,
                default: 0.0,
                kind: ParamKind::Continuous {
                    unit: Some("oct".to_string()),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            });
            desc.instrument_modulation_targets
                .push(InstrumentModulationTarget {
                    base_param_idx: cutoff_idx,
                    source_param_idx: None,
                    modulator_slot: slot + 1,
                    depth_param_idx,
                    active_param_idx: None,
                    depth_min: -4.0,
                    depth_max: 4.0,
                    depth_unit: Some("oct".to_string()),
                });
        }
        desc
    }

    /// Built-in delay effect descriptor.
    pub fn builtin_delay() -> Self {
        Self {
            name: "Delay".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "wet".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::sampler::PARAM_ATTACK_SAMPLES as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "synced".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::sampler::PARAM_RELEASE_SAMPLES as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "time".to_string(),
                    min: 1.0,
                    max: 2000.0,
                    default: 250.0,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::sampler::PARAM_START_POINT as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "feedback".to_string(),
                    min: 0.0,
                    max: 0.95,
                    default: 0.3,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::sampler::PARAM_END_POINT as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "dampening".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 4,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "width".to_string(),
                    min: 0.0,
                    max: 2.0,
                    default: 1.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 5,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                Self::enabled_param(crate::delay::DELAY_PARAM_ENABLED as u32, 1.0),
            ],
        }
    }

    pub fn builtin_str8_delay() -> Self {
        let sync_labels = || {
            vec![
                "1/32".to_string(),
                "1/16".to_string(),
                "1/16t".to_string(),
                "1/8".to_string(),
                "1/8t".to_string(),
                "1/8.".to_string(),
                "1/4".to_string(),
                "1/4t".to_string(),
                "1/4.".to_string(),
                "1/2".to_string(),
                "1".to_string(),
            ]
        };
        let mut desc = Self {
            name: "Str8 Delay".to_string(),
            input_channels: 2 + crate::voice_modulator::NUM_OUTPUTS,
            output_channels: 2,
            instrument_modulators: (1..=crate::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                Self::enabled_param(crate::str8_delay::STR8_DELAY_PARAM_ENABLED as u32, 1.0),
                ParamDescriptor {
                    name: "wet".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_WET as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "feedback".to_string(),
                    min: 0.0,
                    max: 0.95,
                    default: 0.5,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_FEEDBACK as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "left sync".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_LEFT_SYNC as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "left div".to_string(),
                    min: 0.0,
                    max: 10.0,
                    default: 6.0,
                    kind: ParamKind::Enum {
                        labels: sync_labels(),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_LEFT_DIV as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "left offset".to_string(),
                    min: -0.5,
                    max: 0.5,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_LEFT_OFFSET as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "left time".to_string(),
                    min: 1.0,
                    max: 2000.0,
                    default: 250.0,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_LEFT_TIME_MS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "right sync".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_RIGHT_SYNC as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "right div".to_string(),
                    min: 0.0,
                    max: 10.0,
                    default: 6.0,
                    kind: ParamKind::Enum {
                        labels: sync_labels(),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_RIGHT_DIV as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "right offset".to_string(),
                    min: -0.5,
                    max: 0.5,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_RIGHT_OFFSET as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "right time".to_string(),
                    min: 1.0,
                    max: 2000.0,
                    default: 250.0,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_RIGHT_TIME_MS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "filter freq".to_string(),
                    min: 20.0,
                    max: 20000.0,
                    default: 1140.0,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_FILTER_FREQ as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "filter width".to_string(),
                    min: 0.25,
                    max: 6.0,
                    default: 4.5,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_FILTER_Q as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mod rate".to_string(),
                    min: 0.01,
                    max: 20.0,
                    default: 0.5,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_MOD_RATE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mod amount".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_MOD_AMOUNT as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mod phase".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::str8_delay::STR8_DELAY_PARAM_MOD_PHASE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        desc.params
            .extend(crate::voice_modulator::effect_param_descriptors());

        let time_idx = desc
            .params
            .iter()
            .position(|param| param.name == "left time")
            .expect("built-in Str8 Delay left time param should exist");
        let wet_idx = desc
            .params
            .iter()
            .position(|param| param.name == "wet")
            .expect("built-in Str8 Delay wet param should exist");
        let feedback_idx = desc
            .params
            .iter()
            .position(|param| param.name == "feedback")
            .expect("built-in Str8 Delay feedback param should exist");
        let cutoff_idx = desc
            .params
            .iter()
            .position(|param| param.name == "filter freq")
            .expect("built-in Str8 Delay filter freq param should exist");

        let mut append_depth_targets =
            |base_param_idx: usize,
             destination_name: &str,
             depth_params: [u64; crate::voice_modulator::SLOT_COUNT],
             depth_min: f32,
             depth_max: f32,
             depth_unit: Option<&str>| {
                for (slot, node_param_idx) in depth_params.into_iter().enumerate() {
                    let depth_param_idx = desc.params.len();
                    desc.params.push(ParamDescriptor {
                        name: format!("mod {destination_name} slot {} amt", slot + 1),
                        min: depth_min,
                        max: depth_max,
                        default: 0.0,
                        kind: ParamKind::Continuous {
                            unit: depth_unit.map(str::to_string),
                        },
                        scaling: ParamScaling::Linear,
                        node_param_idx: node_param_idx as u32,
                        node_param_span: 1,
                        host_control: None,
                        ui_metadata: None,
                    });
                    desc.instrument_modulation_targets
                        .push(InstrumentModulationTarget {
                            base_param_idx,
                            source_param_idx: None,
                            modulator_slot: slot + 1,
                            depth_param_idx,
                            active_param_idx: None,
                            depth_min,
                            depth_max,
                            depth_unit: depth_unit.map(str::to_string),
                        });
                }
            };

        append_depth_targets(
            time_idx,
            "time",
            [
                crate::str8_delay::STR8_DELAY_PARAM_MOD_TIME_DEPTH_1,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_TIME_DEPTH_2,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_TIME_DEPTH_3,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_TIME_DEPTH_4,
            ],
            -1000.0,
            1000.0,
            Some("ms"),
        );
        append_depth_targets(
            wet_idx,
            "wet",
            [
                crate::str8_delay::STR8_DELAY_PARAM_MOD_WET_DEPTH_1,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_WET_DEPTH_2,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_WET_DEPTH_3,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_WET_DEPTH_4,
            ],
            -1.0,
            1.0,
            Some("%"),
        );
        append_depth_targets(
            feedback_idx,
            "feedback",
            [
                crate::str8_delay::STR8_DELAY_PARAM_MOD_FEEDBACK_DEPTH_1,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_FEEDBACK_DEPTH_2,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_FEEDBACK_DEPTH_3,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_FEEDBACK_DEPTH_4,
            ],
            -0.95,
            0.95,
            None,
        );
        append_depth_targets(
            cutoff_idx,
            "cutoff",
            [
                crate::str8_delay::STR8_DELAY_PARAM_MOD_CUTOFF_DEPTH_1,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_CUTOFF_DEPTH_2,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_CUTOFF_DEPTH_3,
                crate::str8_delay::STR8_DELAY_PARAM_MOD_CUTOFF_DEPTH_4,
            ],
            -4.0,
            4.0,
            Some("oct"),
        );

        desc
    }

    pub fn builtin_dj_mixer() -> Self {
        Self {
            name: "DJ Mixer".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                Self::enabled_param(crate::dj_mixer::DJ_MIXER_PARAM_ENABLED as u32, 1.0),
                ParamDescriptor {
                    name: "speed".to_string(),
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dj_mixer::DJ_MIXER_PARAM_SPEED as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "length".to_string(),
                    min: 0.012,
                    max: 0.230,
                    default: 0.230,
                    kind: ParamKind::Continuous {
                        unit: Some("s".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dj_mixer::DJ_MIXER_PARAM_LENGTH_SEC as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "loop".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dj_mixer::DJ_MIXER_PARAM_LOOP as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        }
    }

    /// Built-in reverb as an insert effect. The DSP node is mono-in/stereo-out,
    /// so stereo predecessors are currently folded to its mono input by graph wiring.
    pub fn builtin_reverb_insert() -> Self {
        Self {
            name: "Reverb".to_string(),
            input_channels: 1,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "mix".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.35,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 4,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "size".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.2,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::reverb::REVERB_PARAM_SIZE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "brightness".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.8,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::reverb::REVERB_PARAM_BRIGHT as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "replace".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.3,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::reverb::REVERB_PARAM_REPLACE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                Self::enabled_param(crate::reverb::REVERB_PARAM_ENABLED as u32, 1.0),
            ],
        }
    }

    fn builtin_dynamics_variant(
        name: &str,
        mode: f32,
        amount: f32,
        attack: f32,
        release: f32,
        low_cut_hz: f32,
        drive: f32,
        output_db: f32,
        mix: f32,
    ) -> Self {
        Self {
            name: name.to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "mode".to_string(),
                    min: 0.0,
                    max: 2.0,
                    default: mode,
                    kind: ParamKind::Enum {
                        labels: vec!["Glue".to_string(), "404".to_string(), "Hybrid".to_string()],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dynamics::DYNAMICS_PARAM_MODE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "amount".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: amount,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dynamics::DYNAMICS_PARAM_AMOUNT as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "attack".to_string(),
                    min: 0.0,
                    max: 3.0,
                    default: attack,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "fast".to_string(),
                            "punch".to_string(),
                            "glue".to_string(),
                            "slow".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dynamics::DYNAMICS_PARAM_ATTACK as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "release".to_string(),
                    min: 0.0,
                    max: 3.0,
                    default: release,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "fast".to_string(),
                            "bounce".to_string(),
                            "auto".to_string(),
                            "smooth".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dynamics::DYNAMICS_PARAM_RELEASE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                if mode == 0.0 {
                    ParamDescriptor {
                        name: "low cut".to_string(),
                        min: 0.0,
                        max: 3.0,
                        default: low_cut_hz,
                        kind: ParamKind::Enum {
                            labels: vec![
                                "off".to_string(),
                                "60".to_string(),
                                "90".to_string(),
                                "150".to_string(),
                            ],
                        },
                        scaling: ParamScaling::Linear,
                        node_param_idx: crate::dynamics::DYNAMICS_PARAM_LOW_CUT_HZ as u32,
                        node_param_span: 1,
                        host_control: None,
                        ui_metadata: None,
                    }
                } else {
                    ParamDescriptor {
                        name: "low cut".to_string(),
                        min: 20.0,
                        max: 250.0,
                        default: low_cut_hz,
                        kind: ParamKind::Continuous {
                            unit: Some("Hz".to_string()),
                        },
                        scaling: ParamScaling::Exponential,
                        node_param_idx: crate::dynamics::DYNAMICS_PARAM_LOW_CUT_HZ as u32,
                        node_param_span: 1,
                        host_control: None,
                        ui_metadata: None,
                    }
                },
                ParamDescriptor {
                    name: "drive".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: drive,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dynamics::DYNAMICS_PARAM_DRIVE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "input".to_string(),
                    min: -12.0,
                    max: 24.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dynamics::DYNAMICS_PARAM_INPUT_DB as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "output".to_string(),
                    min: -12.0,
                    max: 12.0,
                    default: output_db,
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dynamics::DYNAMICS_PARAM_OUTPUT_DB as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mix".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: mix,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dynamics::DYNAMICS_PARAM_MIX as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                Self::enabled_param(crate::dynamics::DYNAMICS_PARAM_ENABLED as u32, 1.0),
                ParamDescriptor {
                    name: "knee".to_string(),
                    min: 0.0,
                    max: 18.0,
                    default: if mode == 0.0 { 8.0 } else { 6.0 },
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::dynamics::DYNAMICS_PARAM_KNEE_DB as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        }
    }

    /// SP-404-inspired compressor: sustain, level, and post-compression color.
    pub fn builtin_444_compressor() -> Self {
        Self::builtin_dynamics_variant("444 Compressor", 1.0, 0.62, 1.0, 2.0, 55.0, 0.32, -1.0, 1.0)
    }

    /// SSL-style bus glue: linked stereo detection, low-cut sidechain, and auto release.
    pub fn builtin_glue_compressor() -> Self {
        Self::builtin_dynamics_variant("Glue Compressor", 0.0, 0.42, 2.0, 2.0, 2.0, 0.12, 0.0, 1.0)
    }

    /// General-purpose compressor with conservative hybrid behavior.
    pub fn builtin_compressor() -> Self {
        Self {
            name: "Compressor".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "threshold".to_string(),
                    min: -60.0,
                    max: 0.0,
                    default: -18.0,
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::compressor::COMPRESSOR_PARAM_THRESHOLD_DB as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "ratio".to_string(),
                    min: 1.0,
                    max: 20.0,
                    default: 4.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::compressor::COMPRESSOR_PARAM_RATIO as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "attack".to_string(),
                    min: 0.1,
                    max: 200.0,
                    default: 10.0,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::compressor::COMPRESSOR_PARAM_ATTACK_MS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "release".to_string(),
                    min: 5.0,
                    max: 2000.0,
                    default: 120.0,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::compressor::COMPRESSOR_PARAM_RELEASE_MS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "makeup".to_string(),
                    min: -24.0,
                    max: 24.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::compressor::COMPRESSOR_PARAM_MAKEUP_DB as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mix".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::compressor::COMPRESSOR_PARAM_MIX as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                Self::enabled_param(crate::compressor::COMPRESSOR_PARAM_ENABLED as u32, 1.0),
            ],
        }
    }

    /// General-purpose lookahead limiter.
    pub fn builtin_limiter() -> Self {
        Self {
            name: "Limiter".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "input".to_string(),
                    min: -24.0,
                    max: 24.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::limiter::LIMITER_PARAM_INPUT_GAIN_DB as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "ceiling".to_string(),
                    min: -24.0,
                    max: 0.0,
                    default: -0.3,
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::limiter::LIMITER_PARAM_CEILING_DB as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "release".to_string(),
                    min: 1.0,
                    max: 2000.0,
                    default: 100.0,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::limiter::LIMITER_PARAM_RELEASE_MS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "lookahead".to_string(),
                    min: 0.0,
                    max: 20.0,
                    default: 3.0,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::limiter::LIMITER_PARAM_LOOKAHEAD_MS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                Self::enabled_param(crate::limiter::LIMITER_PARAM_ENABLED as u32, 1.0),
            ],
        }
    }

    /// Analog tape emulation built on Jiles–Atherton hysteresis.
    pub fn builtin_tape() -> Self {
        Self {
            name: "Tape".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "drive".to_string(),
                    min: -12.0,
                    max: 24.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::tape::TAPE_PARAM_DRIVE_DB as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "bias".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::tape::TAPE_PARAM_BIAS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "speed".to_string(),
                    min: 0.0,
                    max: 2.0,
                    default: 1.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "7.5 ips".to_string(),
                            "15 ips".to_string(),
                            "30 ips".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::tape::TAPE_PARAM_SPEED as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "output".to_string(),
                    min: -24.0,
                    max: 12.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::tape::TAPE_PARAM_OUTPUT_DB as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mix".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::tape::TAPE_PARAM_MIX as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "wow".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::tape::TAPE_PARAM_WOW as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "flutter".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::tape::TAPE_PARAM_FLUTTER as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "hiss".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::tape::TAPE_PARAM_HISS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                Self::enabled_param(crate::tape::TAPE_PARAM_ENABLED as u32, 1.0),
            ],
        }
    }

    /// Back-compat alias for projects created while the generic prototype existed.
    pub fn builtin_dynamics() -> Self {
        Self::builtin_444_compressor()
    }

    /// Built-in sampler instrument descriptor.
    pub fn builtin_sampler() -> Self {
        let mut params = vec![
            ParamDescriptor {
                name: "attack".to_string(),
                min: 0.0,
                max: 500.0,
                default: 0.0,
                kind: ParamKind::Continuous {
                    unit: Some("ms".to_string()),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: 0,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "release".to_string(),
                min: 0.0,
                max: 2000.0,
                default: 0.0,
                kind: ParamKind::Continuous {
                    unit: Some("ms".to_string()),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: 1,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "start".to_string(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                kind: ParamKind::Continuous {
                    unit: Some("%".to_string()),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: 2,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "end".to_string(),
                min: 0.0,
                max: 1.0,
                default: 1.0,
                kind: ParamKind::Continuous {
                    unit: Some("%".to_string()),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: 3,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            Self::enabled_param(crate::sampler::SAMPLER_PARAM_ENABLED as u32, 1.0),
            ParamDescriptor {
                name: "reverse".to_string(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                kind: ParamKind::Boolean,
                scaling: ParamScaling::Linear,
                node_param_idx: crate::sampler::PARAM_REVERSE as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "loop".to_string(),
                min: 0.0,
                max: 3.0,
                default: 1.0,
                kind: ParamKind::Enum {
                    labels: vec![
                        "one-shot".to_string(),
                        "gate".to_string(),
                        "loop".to_string(),
                        "ping-pong".to_string(),
                    ],
                },
                scaling: ParamScaling::Linear,
                node_param_idx: crate::sampler::PARAM_LOOP_MODE as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "xfade".to_string(),
                min: 0.0,
                max: 250.0,
                default: 0.0,
                kind: ParamKind::Continuous {
                    unit: Some("ms".to_string()),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: crate::sampler::PARAM_LOOP_XFADE_SAMPLES as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "sr".to_string(),
                min: 2000.0,
                max: 44100.0,
                default: 44100.0,
                kind: ParamKind::Continuous {
                    unit: Some("Hz".to_string()),
                },
                scaling: ParamScaling::Exponential,
                node_param_idx: crate::sampler::PARAM_SR_HZ as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "warp".to_string(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                kind: ParamKind::Boolean,
                scaling: ParamScaling::Linear,
                node_param_idx: crate::sampler::PARAM_WARP_ENABLED as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "mode".to_string(),
                min: 0.0,
                max: 0.0,
                default: 0.0,
                kind: ParamKind::Enum {
                    labels: vec!["transient".to_string()],
                },
                scaling: ParamScaling::Linear,
                node_param_idx: crate::sampler::PARAM_WARP_MODE as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "bpm".to_string(),
                min: 20.0,
                max: 400.0,
                default: 120.0,
                kind: ParamKind::Continuous { unit: None },
                scaling: ParamScaling::Linear,
                node_param_idx: crate::sampler::PARAM_WARP_SAMPLE_BPM as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "speed".to_string(),
                min: -4.0,
                max: 4.0,
                default: 1.0,
                kind: ParamKind::Continuous { unit: None },
                scaling: ParamScaling::Linear,
                node_param_idx: crate::sampler::PARAM_SPEED as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "scrub".to_string(),
                min: -1.0,
                max: 1.0,
                default: 0.0,
                kind: ParamKind::Continuous {
                    unit: Some("%".to_string()),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: crate::sampler::PARAM_SCRUB_OFFSET as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
        ];
        params.extend(crate::voice_modulator::ui_param_descriptors());
        let mod_source_labels: Vec<String> = std::iter::once("off".to_string())
            .chain(
                (1..=crate::voice_modulator::SLOT_COUNT)
                    .map(|slot| crate::voice_modulator::modulator_slot_label(slot, "")),
            )
            .collect();
        let mut instrument_modulation_targets = Vec::new();
        for lane in crate::sampler::SAMPLER_MOD_TARGET_PARAMS {
            let name = lane.destination;
            let base_param_idx = params
                .iter()
                .position(|p| p.name == name)
                .expect("sampler modulation target base param should exist");
            let source_param_idx = params.len();
            params.push(ParamDescriptor {
                name: if lane.lane == 1 {
                    format!("mod {name} src")
                } else {
                    format!("mod {name} lane {} src", lane.lane)
                },
                min: 0.0,
                max: crate::voice_modulator::SLOT_COUNT as f32,
                default: 0.0,
                kind: ParamKind::Enum {
                    labels: mod_source_labels.clone(),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: lane.source_param as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            });
            let depth_param_idx = params.len();
            let (depth_min, depth_max, depth_unit) = sampler_mod_depth_range(name);
            params.push(ParamDescriptor {
                name: if lane.lane == 1 {
                    format!("mod {name} amt")
                } else {
                    format!("mod {name} lane {} amt", lane.lane)
                },
                min: depth_min,
                max: depth_max,
                default: 0.0,
                kind: ParamKind::Continuous {
                    unit: depth_unit.clone(),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: lane.depth_param as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            });
            instrument_modulation_targets.push(InstrumentModulationTarget {
                base_param_idx,
                source_param_idx: Some(source_param_idx),
                modulator_slot: 0,
                depth_param_idx,
                active_param_idx: None,
                depth_min,
                depth_max,
                depth_unit,
            });
        }
        params.push(ParamDescriptor {
            name: "smooth".to_string(),
            min: 0.0,
            max: 250.0,
            default: 6.0,
            kind: ParamKind::Continuous {
                unit: Some("ms".to_string()),
            },
            scaling: ParamScaling::Linear,
            node_param_idx: crate::sampler::PARAM_SCRUB_SMOOTH_TIME_MS as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        });
        Self {
            name: "Sampler".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: (1..=crate::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets,
            params,
        }
    }
    /// Default fixed effect chain descriptors.
    pub fn default_chain() -> Vec<Self> {
        Vec::new()
    }

    /// Full default chain: MAX_CUSTOM_FX empty insert slots.
    pub fn default_full_chain() -> Vec<Self> {
        let mut chain = Self::default_chain();
        for _ in 0..crate::lisp_host::MAX_CUSTOM_FX {
            chain.push(Self::empty_custom_slot());
        }
        chain
    }

    /// Empty custom slot placeholder (name is empty, no params).
    pub fn empty_custom_slot() -> Self {
        Self {
            name: String::new(),
            params: Vec::new(),
            input_channels: 0,
            output_channels: 0,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
        }
    }

    /// Construct from a lisp effect manifest.
    pub fn from_lisp_manifest(
        name: &str,
        params: &[crate::lisp_host::DGenParam],
        input_channels: usize,
        output_channels: usize,
    ) -> Self {
        let mut descriptors: Vec<ParamDescriptor> = params
            .iter()
            .filter(|p| !p.hidden)
            .map(|p| ParamDescriptor {
                name: p.name.clone(),
                min: p.min,
                max: p.max,
                default: p.default,
                kind: ParamKind::Continuous {
                    unit: p.unit.clone(),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: (crate::lisp_host::HEADER_SLOTS + p.cell_id) as u32,
                node_param_span: p.cell_span as u32,
                host_control: None,
                ui_metadata: crate::effects::ParamUiMetadata::new(
                    p.group.clone(),
                    p.env.clone(),
                    p.role.clone(),
                ),
            })
            .collect();
        descriptors.push(Self::enabled_param(
            crate::lisp_host::DGEN_ENABLED_PARAM_IDX as u32,
            1.0,
        ));
        Self {
            name: name.to_string(),
            params: descriptors,
            input_channels,
            output_channels,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
        }
    }
}

// ── SlotPLockData (replaces EffectPLockData and LispPLockData) ──

/// Per-slot per-step parameter overrides.
/// NaN = no override (use slot default).
/// No internal clamping — callers pass clamped values.
pub struct SlotPLockData {
    data: Vec<AtomicU32>,
    id_logical_ids: Vec<AtomicU64>,
    id_node_param_indices: Vec<AtomicU32>,
    max_params: usize,
    /// Number of non-NaN cells. Lets snapshot capture/restore and per-step
    /// plock masks take O(1) fast paths for the common plock-free slot.
    plock_count: AtomicU32,
    /// Number of non-NaN cells per step. Lets per-step queries and snapshot
    /// capture/restore skip the param scan for plock-free steps.
    step_counts: Vec<AtomicU32>,
}

impl SlotPLockData {
    pub fn new(max_params: usize) -> Self {
        let size = MAX_STEPS * max_params;
        let data: Vec<AtomicU32> = (0..size).map(|_| AtomicU32::new(NAN_BITS)).collect();
        let id_logical_ids: Vec<AtomicU64> = (0..size).map(|_| AtomicU64::new(0)).collect();
        let id_node_param_indices: Vec<AtomicU32> =
            (0..size).map(|_| AtomicU32::new(u32::MAX)).collect();
        Self {
            data,
            id_logical_ids,
            id_node_param_indices,
            max_params,
            plock_count: AtomicU32::new(0),
            step_counts: (0..MAX_STEPS).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    fn index(&self, step: usize, param_idx: usize) -> usize {
        step * self.max_params + param_idx
    }

    pub fn has_any_plock(&self) -> bool {
        self.plock_count.load(Ordering::Relaxed) > 0
    }

    /// Adjust plock_count and the step's count for a cell transitioning
    /// between old and new bits.
    fn note_cell_transition(&self, step: usize, old_bits: u32, new_bits: u32) {
        let old_set = !f32::from_bits(old_bits).is_nan();
        let new_set = !f32::from_bits(new_bits).is_nan();
        if !old_set && new_set {
            self.plock_count.fetch_add(1, Ordering::Relaxed);
            if let Some(count) = self.step_counts.get(step) {
                count.fetch_add(1, Ordering::Relaxed);
            }
        } else if old_set && !new_set {
            self.plock_count.fetch_sub(1, Ordering::Relaxed);
            if let Some(count) = self.step_counts.get(step) {
                count.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    fn step_count(&self, step: usize) -> u32 {
        self.step_counts
            .get(step)
            .map(|count| count.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Clear every plock value and param id.
    pub fn clear_all(&self) {
        if !self.has_any_plock() {
            return;
        }
        for idx in 0..self.data.len() {
            self.data[idx].store(NAN_BITS, Ordering::Relaxed);
            self.id_logical_ids[idx].store(0, Ordering::Relaxed);
            self.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
        }
        self.plock_count.store(0, Ordering::Relaxed);
        for count in &self.step_counts {
            count.store(0, Ordering::Relaxed);
        }
    }

    pub fn get(&self, step: usize, param_idx: usize) -> Option<f32> {
        let idx = self.index(step, param_idx);
        if idx >= self.data.len() {
            return None;
        }
        let bits = self.data[idx].load(Ordering::Relaxed);
        let val = f32::from_bits(bits);
        if val.is_nan() {
            None
        } else {
            Some(val)
        }
    }

    pub fn set(&self, step: usize, param_idx: usize, val: f32) {
        let idx = self.index(step, param_idx);
        if idx < self.data.len() {
            let old_bits = self.data[idx].swap(val.to_bits(), Ordering::Relaxed);
            self.note_cell_transition(step, old_bits, val.to_bits());
            self.id_logical_ids[idx].store(0, Ordering::Relaxed);
            self.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
        }
    }

    pub fn set_with_id(&self, step: usize, param_idx: usize, val: f32, param_id: ParamNodeId) {
        let idx = self.index(step, param_idx);
        if idx < self.data.len() {
            let old_bits = self.data[idx].swap(val.to_bits(), Ordering::Relaxed);
            self.note_cell_transition(step, old_bits, val.to_bits());
            self.id_logical_ids[idx].store(param_id.logical_id, Ordering::Relaxed);
            self.id_node_param_indices[idx].store(param_id.node_param_idx, Ordering::Relaxed);
        }
    }

    pub fn get_id(&self, step: usize, param_idx: usize) -> Option<ParamNodeId> {
        let idx = self.index(step, param_idx);
        if idx >= self.data.len() {
            return None;
        }
        let logical_id = self.id_logical_ids[idx].load(Ordering::Relaxed);
        let node_param_idx = self.id_node_param_indices[idx].load(Ordering::Relaxed);
        if logical_id == 0 || node_param_idx == u32::MAX {
            None
        } else {
            Some(ParamNodeId {
                logical_id,
                node_param_idx,
            })
        }
    }

    pub fn clear_step(&self, step: usize) {
        if self.step_count(step) == 0 {
            return;
        }
        for p in 0..self.max_params {
            let idx = self.index(step, p);
            if idx < self.data.len() {
                let old_bits = self.data[idx].swap(NAN_BITS, Ordering::Relaxed);
                self.note_cell_transition(step, old_bits, NAN_BITS);
                self.id_logical_ids[idx].store(0, Ordering::Relaxed);
                self.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
            }
        }
    }

    pub fn clear_param(&self, step: usize, param_idx: usize) {
        let idx = self.index(step, param_idx);
        if idx < self.data.len() {
            let old_bits = self.data[idx].swap(NAN_BITS, Ordering::Relaxed);
            self.note_cell_transition(step, old_bits, NAN_BITS);
            self.id_logical_ids[idx].store(0, Ordering::Relaxed);
            self.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
        }
    }

    /// Bulk capture of all step plock values and param ids in one flat pass.
    /// Equivalent to calling `get`/`get_id` for every (step, param) but
    /// without the per-element call overhead.
    pub fn capture_rows(
        &self,
        num_params: usize,
    ) -> (Vec<Vec<Option<f32>>>, Vec<Vec<Option<ParamNodeId>>>) {
        if !self.has_any_plock() {
            // No plocks anywhere: empty row sets denote "all None" and avoid
            // allocating MAX_STEPS * num_params Options per slot.
            return (Vec::new(), Vec::new());
        }
        let read_np = num_params.min(self.max_params);
        let mut plocks = Vec::with_capacity(MAX_STEPS);
        let mut plock_param_ids = Vec::with_capacity(MAX_STEPS);
        for step in 0..MAX_STEPS {
            if self.step_count(step) == 0 {
                // Empty rows denote "all None" for this step (consumers use
                // guarded .get() access) and skip the per-param atomic scan.
                plocks.push(Vec::new());
                plock_param_ids.push(Vec::new());
                continue;
            }
            let base = step * self.max_params;
            let mut row = vec![None; num_params];
            let mut id_row = vec![None; num_params];
            for p in 0..read_np {
                let idx = base + p;
                let val = f32::from_bits(self.data[idx].load(Ordering::Relaxed));
                if !val.is_nan() {
                    row[p] = Some(val);
                }
                let logical_id = self.id_logical_ids[idx].load(Ordering::Relaxed);
                if logical_id != 0 {
                    let node_param_idx = self.id_node_param_indices[idx].load(Ordering::Relaxed);
                    if node_param_idx != u32::MAX {
                        id_row[p] = Some(ParamNodeId {
                            logical_id,
                            node_param_idx,
                        });
                    }
                }
            }
            plocks.push(row);
            plock_param_ids.push(id_row);
        }
        (plocks, plock_param_ids)
    }

    /// Bulk restore matching EffectSlotSnapshot::restore semantics: values
    /// present in the snapshot rows are written (with their param ids), `None`
    /// entries are cleared, and steps/params missing from the snapshot are
    /// left untouched.
    pub fn restore_rows(
        &self,
        plocks: &[Vec<Option<f32>>],
        plock_param_ids: &[Vec<Option<ParamNodeId>>],
        num_params: usize,
    ) {
        if plocks.is_empty() {
            // Empty row set means "no plocks at all" (see capture_rows).
            self.clear_all();
            return;
        }
        let np = num_params.min(self.max_params);
        for (step, row) in plocks.iter().enumerate().take(MAX_STEPS) {
            if row.is_empty() {
                // Empty row means "all None" at this step (see capture_rows):
                // clear it, which is a no-op when the step holds nothing.
                self.clear_step(step);
                continue;
            }
            // Skip steps where the snapshot row carries no values and the slot
            // holds none either: every cell write below would be a no-op.
            if self.step_count(step) == 0 && !row.iter().any(|value| value.is_some()) {
                continue;
            }
            let base = step * self.max_params;
            let id_row = plock_param_ids.get(step);
            for p in 0..np.min(row.len()) {
                let idx = base + p;
                match row[p] {
                    Some(val) => {
                        let param_id = id_row.and_then(|ids| ids.get(p)).copied().flatten();
                        let old_bits = self.data[idx].swap(val.to_bits(), Ordering::Relaxed);
                        self.note_cell_transition(step, old_bits, val.to_bits());
                        match param_id {
                            Some(param_id) => {
                                self.id_logical_ids[idx]
                                    .store(param_id.logical_id, Ordering::Relaxed);
                                self.id_node_param_indices[idx]
                                    .store(param_id.node_param_idx, Ordering::Relaxed);
                            }
                            None => {
                                self.id_logical_ids[idx].store(0, Ordering::Relaxed);
                                self.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
                            }
                        }
                    }
                    None => {
                        let old_bits = self.data[idx].swap(NAN_BITS, Ordering::Relaxed);
                        self.note_cell_transition(step, old_bits, NAN_BITS);
                        self.id_logical_ids[idx].store(0, Ordering::Relaxed);
                        self.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    /// OR a per-step "has any plock" bit into `mask` (one bit per step) in a
    /// single flat scan.
    pub fn or_step_plock_mask(&self, mask: &mut [u64; MAX_STEPS / 64], num_params: usize) {
        if !self.has_any_plock() {
            return;
        }
        let np = num_params.min(self.max_params);
        if np == 0 {
            return;
        }
        for step in 0..MAX_STEPS {
            if self.step_count(step) > 0 {
                mask[step / 64] |= 1u64 << (step % 64);
            }
        }
    }

    pub fn step_has_any_plock(&self, step: usize, num_params: usize) -> bool {
        if num_params.min(self.max_params) == 0 {
            return false;
        }
        self.step_count(step) > 0
    }
}

// ── SlotParamDefaults (replaces TrackEffectDefaults and LispParamDefaults) ──

pub struct SlotParamDefaults {
    data: Vec<AtomicU32>,
}

impl SlotParamDefaults {
    pub fn new_from_descriptor(desc: &EffectDescriptor) -> Self {
        let mut data: Vec<AtomicU32> = desc
            .params
            .iter()
            .map(|p| AtomicU32::new(p.default.to_bits()))
            .collect();
        data.resize_with(MAX_SLOT_PARAMS.max(data.len()), || {
            AtomicU32::new(0.0_f32.to_bits())
        });
        Self { data }
    }

    pub fn new_zeroed(count: usize) -> Self {
        let data: Vec<AtomicU32> = (0..count)
            .map(|_| AtomicU32::new(0.0_f32.to_bits()))
            .collect();
        Self { data }
    }

    pub fn get(&self, idx: usize) -> f32 {
        if idx < self.data.len() {
            f32::from_bits(self.data[idx].load(Ordering::Relaxed))
        } else {
            0.0
        }
    }

    pub fn set(&self, idx: usize, val: f32) {
        if idx < self.data.len() {
            self.data[idx].store(val.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }
}

// ── EffectSlotState (runtime state for one effect in a track's chain) ──

pub struct EffectSlotState {
    pub node_id: AtomicU32,           // audio graph node (0 = empty)
    pub modulator_node_id: AtomicU32, // optional host modulation bank node
    pub plocks: SlotPLockData,
    pub defaults: SlotParamDefaults,
    pub num_params: AtomicU32,
    pub param_node_indices: Vec<AtomicU32>, // per-param: idx field for ParamMsg
    pub param_node_spans: Vec<AtomicU32>,   // per-param: contiguous DGen cells updated by idx
}

impl EffectSlotState {
    pub fn new(desc: &EffectDescriptor, node_id: u32) -> Self {
        let num_params = desc.params.len();
        let capacity = MAX_SLOT_PARAMS.max(num_params);
        let mut param_node_indices: Vec<AtomicU32> = desc
            .params
            .iter()
            .map(|p| AtomicU32::new(p.node_param_idx))
            .collect();
        param_node_indices.resize_with(capacity, || AtomicU32::new(0));
        let mut param_node_spans: Vec<AtomicU32> = desc
            .params
            .iter()
            .map(|p| AtomicU32::new(p.node_param_span.max(1)))
            .collect();
        param_node_spans.resize_with(capacity, || AtomicU32::new(1));
        Self {
            node_id: AtomicU32::new(node_id),
            modulator_node_id: AtomicU32::new(0),
            plocks: SlotPLockData::new(capacity),
            defaults: SlotParamDefaults::new_from_descriptor(desc),
            num_params: AtomicU32::new(num_params as u32),
            param_node_indices,
            param_node_spans,
        }
    }

    /// Resolve the audio graph param index for a given param.
    pub fn resolve_node_idx(&self, param_idx: usize) -> u64 {
        if param_idx < self.param_node_indices.len() {
            self.param_node_indices[param_idx].load(Ordering::Relaxed) as u64
        } else {
            param_idx as u64
        }
    }

    pub fn resolve_node_span(&self, param_idx: usize) -> u32 {
        if param_idx < self.param_node_spans.len() {
            self.param_node_spans[param_idx]
                .load(Ordering::Relaxed)
                .max(1)
        } else {
            1
        }
    }

    pub fn param_node_id(&self, param_idx: usize) -> Option<ParamNodeId> {
        let raw_idx = self
            .param_node_indices
            .get(param_idx)?
            .load(Ordering::Relaxed);
        if raw_idx == u32::MAX {
            return None;
        }
        if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
            let logical_id = self.modulator_node_id.load(Ordering::Relaxed) as u64;
            if logical_id == 0 {
                return None;
            }
            Some(ParamNodeId {
                logical_id,
                node_param_idx: raw_idx - crate::voice_modulator::MOD_PARAM_BASE,
            })
        } else {
            let logical_id = self.node_id.load(Ordering::Relaxed) as u64;
            if logical_id == 0 {
                return None;
            }
            Some(ParamNodeId {
                logical_id,
                node_param_idx: raw_idx,
            })
        }
    }

    pub fn set_plock(&self, step: usize, param_idx: usize, val: f32) {
        if let Some(param_id) = self.param_node_id(param_idx) {
            self.plocks.set_with_id(step, param_idx, val, param_id);
        } else {
            self.plocks.set(step, param_idx, val);
        }
    }

    /// Create an empty slot (no effect loaded).
    pub fn empty() -> Self {
        Self {
            node_id: AtomicU32::new(0),
            modulator_node_id: AtomicU32::new(0),
            plocks: SlotPLockData::new(MAX_SLOT_PARAMS),
            defaults: SlotParamDefaults::new_zeroed(MAX_SLOT_PARAMS),
            num_params: AtomicU32::new(0),
            param_node_indices: (0..MAX_SLOT_PARAMS).map(|_| AtomicU32::new(0)).collect(),
            param_node_spans: (0..MAX_SLOT_PARAMS).map(|_| AtomicU32::new(1)).collect(),
        }
    }

    /// Overwrite this pre-allocated slot in-place from a descriptor and node ID.
    pub fn apply_descriptor(&self, desc: &EffectDescriptor, node_id: u32) {
        self.apply_descriptor_with_modulator(desc, node_id, 0);
    }

    pub fn apply_descriptor_with_modulator(
        &self,
        desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        self.node_id.store(node_id, Ordering::Relaxed);
        self.modulator_node_id
            .store(modulator_node_id, Ordering::Relaxed);
        self.num_params
            .store(desc.params.len() as u32, Ordering::Relaxed);
        for (i, p) in desc.params.iter().enumerate() {
            self.defaults.set(i, p.default);
            if i < self.param_node_indices.len() {
                self.param_node_indices[i].store(p.node_param_idx, Ordering::Relaxed);
            }
            if i < self.param_node_spans.len() {
                self.param_node_spans[i].store(p.node_param_span.max(1), Ordering::Relaxed);
            }
        }
    }

    /// Rebind this live slot to the current graph descriptor/node while
    /// preserving the stored defaults and p-locks as far as possible.
    pub fn sync_descriptor(&self, desc: &EffectDescriptor, node_id: u32) {
        let modulator_node_id = self.modulator_node_id.load(Ordering::Relaxed);
        self.sync_descriptor_with_modulator(desc, node_id, modulator_node_id);
    }

    pub fn sync_descriptor_with_modulator(
        &self,
        desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        let old_num_params = self.num_params.load(Ordering::Relaxed) as usize;
        let preserve = old_num_params.min(desc.params.len());

        let mut saved_defaults = Vec::with_capacity(preserve);
        for param_idx in 0..preserve {
            saved_defaults.push(self.defaults.get(param_idx));
        }

        let mut saved_plocks = Vec::with_capacity(MAX_STEPS);
        let mut saved_plock_ids = Vec::with_capacity(MAX_STEPS);
        for step in 0..MAX_STEPS {
            let mut step_plocks = Vec::with_capacity(preserve);
            let mut step_ids = Vec::with_capacity(preserve);
            for param_idx in 0..preserve {
                step_plocks.push(self.plocks.get(step, param_idx));
                step_ids.push(self.plocks.get_id(step, param_idx));
            }
            saved_plocks.push(step_plocks);
            saved_plock_ids.push(step_ids);
        }

        self.apply_descriptor_with_modulator(desc, node_id, modulator_node_id);

        for param_idx in 0..preserve {
            self.defaults.set(param_idx, saved_defaults[param_idx]);
        }
        for step in 0..MAX_STEPS {
            for param_idx in 0..preserve {
                match saved_plocks[step][param_idx] {
                    Some(value) => {
                        if let Some(param_id) = saved_plock_ids[step][param_idx] {
                            self.plocks.set_with_id(step, param_idx, value, param_id);
                        } else {
                            self.plocks.set(step, param_idx, value);
                        }
                    }
                    None => self.plocks.clear_param(step, param_idx),
                }
            }
        }
        self.recompute_modulation_active_params(desc);
    }

    /// Rebind this live slot using descriptor identity instead of param index.
    ///
    /// This is the correct path for generated instruments/effects whose param
    /// order may change between edits. Runtime values are only carried across
    /// when a parameter name is unique in both descriptors, the kind is
    /// compatible, and the old value is already valid for the new range.
    pub fn sync_descriptor_by_param_name(
        &self,
        old_desc: &EffectDescriptor,
        new_desc: &EffectDescriptor,
        node_id: u32,
    ) {
        let modulator_node_id = self.modulator_node_id.load(Ordering::Relaxed);
        self.sync_descriptor_by_param_name_with_modulator(
            old_desc,
            new_desc,
            node_id,
            modulator_node_id,
        );
    }

    pub fn sync_descriptor_by_param_name_with_modulator(
        &self,
        old_desc: &EffectDescriptor,
        new_desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        let old_num_params = self.num_params.load(Ordering::Relaxed) as usize;
        let mut migrated = Vec::new();

        for (new_idx, new_param) in new_desc.params.iter().enumerate() {
            let Some(old_idx) = unique_param_index_by_name(old_desc, &new_param.name) else {
                continue;
            };
            if unique_param_index_by_name(new_desc, &new_param.name) != Some(new_idx) {
                continue;
            }
            if old_idx >= old_num_params {
                continue;
            }
            let Some(old_param) = old_desc.params.get(old_idx) else {
                continue;
            };

            let default = self.defaults.get(old_idx);
            let default = new_param
                .accepts_migrated_value_from(old_param, default)
                .then_some(default);

            let mut plocks = Vec::new();
            for step in 0..MAX_STEPS {
                let Some(value) = self.plocks.get(step, old_idx) else {
                    continue;
                };
                if new_param.accepts_migrated_value_from(old_param, value) {
                    plocks.push((step, value));
                }
            }

            migrated.push((new_idx, default, plocks));
        }

        self.apply_descriptor_with_modulator(new_desc, node_id, modulator_node_id);
        for step in 0..MAX_STEPS {
            for param_idx in 0..MAX_SLOT_PARAMS {
                self.plocks.clear_param(step, param_idx);
            }
        }

        for (new_idx, default, plocks) in migrated {
            if let Some(value) = default {
                self.defaults.set(new_idx, value);
            }
            for (step, value) in plocks {
                self.set_plock(step, new_idx, value);
            }
        }
        self.recompute_modulation_active_params(new_desc);
    }

    pub fn recompute_modulation_active_params(&self, desc: &EffectDescriptor) {
        let mut active_indices = desc
            .instrument_modulation_targets
            .iter()
            .filter_map(|target| target.active_param_idx)
            .collect::<Vec<_>>();
        active_indices.sort_unstable();
        active_indices.dedup();

        for active_idx in active_indices {
            if active_idx >= desc.params.len() {
                continue;
            }

            let group = desc
                .instrument_modulation_targets
                .iter()
                .filter(|target| target.active_param_idx == Some(active_idx))
                .collect::<Vec<_>>();
            if group.is_empty() {
                continue;
            }

            let default_active = group
                .iter()
                .any(|target| self.defaults.get(target.depth_param_idx).abs() > f32::EPSILON);
            self.defaults
                .set(active_idx, if default_active { 1.0 } else { 0.0 });

            for step in 0..MAX_STEPS {
                let has_depth_plock = group
                    .iter()
                    .any(|target| self.plocks.get(step, target.depth_param_idx).is_some());
                if has_depth_plock {
                    let active = group.iter().any(|target| {
                        self.plocks
                            .get(step, target.depth_param_idx)
                            .unwrap_or_else(|| self.defaults.get(target.depth_param_idx))
                            .abs()
                            > f32::EPSILON
                    });
                    self.set_plock(step, active_idx, if active { 1.0 } else { 0.0 });
                } else {
                    self.plocks.clear_param(step, active_idx);
                }
            }
        }
    }

    pub fn force_enabled_default(&self, desc: &EffectDescriptor) -> Option<usize> {
        let enabled_idx = desc
            .params
            .iter()
            .position(|param| param.name.eq_ignore_ascii_case("enabled"))?;
        self.defaults.set(enabled_idx, 1.0);
        Some(enabled_idx)
    }

    /// Reset this slot to an empty state.
    pub fn clear(&self) {
        self.node_id.store(0, Ordering::Relaxed);
        self.num_params.store(0, Ordering::Relaxed);
        for i in 0..MAX_SLOT_PARAMS {
            self.defaults.set(i, 0.0);
            if i < self.param_node_indices.len() {
                self.param_node_indices[i].store(0, Ordering::Relaxed);
            }
            if i < self.param_node_spans.len() {
                self.param_node_spans[i].store(1, Ordering::Relaxed);
            }
        }
        for step in 0..MAX_STEPS {
            for param_idx in 0..MAX_SLOT_PARAMS {
                self.plocks.clear_param(step, param_idx);
            }
        }
    }

    /// Copy all runtime slot payload from another slot.
    ///
    /// Used when compacting custom FX slots after deleting one slot. This moves
    /// defaults and p-locks along with the node binding so slot-indexed
    /// automation continues to refer to the same audible effect.
    pub fn copy_from(&self, other: &EffectSlotState) {
        EffectSlotSnapshot::capture(other).restore(self);
    }
}

fn unique_param_index_by_name(desc: &EffectDescriptor, name: &str) -> Option<usize> {
    let mut matches = desc
        .params
        .iter()
        .enumerate()
        .filter(|(_, param)| param.name == name)
        .map(|(idx, _)| idx);
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

// ── EffectSlotSnapshot (for pattern save/restore) ──

#[derive(Clone, Debug)]
pub struct EffectSlotSnapshot {
    pub node_id: u32,
    pub modulator_node_id: u32,
    pub num_params: u32,
    pub defaults: Vec<f32>,
    pub plocks: Vec<Vec<Option<f32>>>,
    pub plock_param_ids: Vec<Vec<Option<ParamNodeId>>>,
    pub param_node_indices: Vec<u32>,
    pub param_node_spans: Vec<u32>,
    /// Convolution Reverb IR reference (sample hash/stem) carried through
    /// save/restore. None for every other effect.
    pub ir: Option<String>,
}

impl EffectSlotSnapshot {
    pub fn capture(slot: &EffectSlotState) -> Self {
        let node_id = slot.node_id.load(Ordering::Relaxed);
        let modulator_node_id = slot.modulator_node_id.load(Ordering::Relaxed);
        let num_params = slot.num_params.load(Ordering::Relaxed);
        let np = num_params as usize;

        let mut defaults = Vec::with_capacity(np);
        for i in 0..np {
            defaults.push(slot.defaults.get(i));
        }

        let (plocks, plock_param_ids) = slot.plocks.capture_rows(np);

        let mut param_node_indices = Vec::with_capacity(np);
        let mut param_node_spans = Vec::with_capacity(np);
        for i in 0..np {
            if i < slot.param_node_indices.len() {
                param_node_indices.push(slot.param_node_indices[i].load(Ordering::Relaxed));
            } else {
                param_node_indices.push(0);
            }
            if i < slot.param_node_spans.len() {
                param_node_spans.push(slot.param_node_spans[i].load(Ordering::Relaxed).max(1));
            } else {
                param_node_spans.push(1);
            }
        }

        Self {
            node_id,
            modulator_node_id,
            num_params,
            defaults,
            plocks,
            plock_param_ids,
            param_node_indices,
            param_node_spans,
            ir: crate::conv_reverb::ir_ref_for(node_id as i32),
        }
    }

    pub fn restore(&self, slot: &EffectSlotState) {
        slot.node_id.store(self.node_id, Ordering::Relaxed);
        slot.modulator_node_id
            .store(self.modulator_node_id, Ordering::Relaxed);
        slot.num_params.store(self.num_params, Ordering::Relaxed);
        let np = self.num_params as usize;

        for i in 0..np {
            if i < self.defaults.len() {
                slot.defaults.set(i, self.defaults[i]);
            }
            if i < slot.param_node_indices.len() {
                let idx = self.param_node_indices.get(i).copied().unwrap_or(0);
                slot.param_node_indices[i].store(idx, Ordering::Relaxed);
            }
            if i < slot.param_node_spans.len() {
                let span = self.param_node_spans.get(i).copied().unwrap_or(1).max(1);
                slot.param_node_spans[i].store(span, Ordering::Relaxed);
            }
        }

        slot.plocks
            .restore_rows(&self.plocks, &self.plock_param_ids, np);
    }

    pub fn new_default(desc: &EffectDescriptor, node_id: u32) -> Self {
        Self::new_default_with_modulator(desc, node_id, 0)
    }

    pub fn new_default_with_modulator(
        desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) -> Self {
        let np = desc.params.len();
        let defaults: Vec<f32> = desc.params.iter().map(|p| p.default).collect();
        let plocks: Vec<Vec<Option<f32>>> = (0..MAX_STEPS).map(|_| vec![None; np]).collect();
        let param_node_indices: Vec<u32> = desc.params.iter().map(|p| p.node_param_idx).collect();
        let param_node_spans: Vec<u32> = desc
            .params
            .iter()
            .map(|p| p.node_param_span.max(1))
            .collect();

        Self {
            node_id,
            modulator_node_id,
            num_params: np as u32,
            defaults,
            plocks,
            plock_param_ids: (0..MAX_STEPS).map(|_| vec![None; np]).collect(),
            param_node_indices,
            param_node_spans,
            ir: None,
        }
    }

    pub fn new_empty() -> Self {
        Self {
            node_id: 0,
            modulator_node_id: 0,
            num_params: 0,
            defaults: Vec::new(),
            plocks: (0..MAX_STEPS).map(|_| Vec::new()).collect(),
            plock_param_ids: (0..MAX_STEPS).map(|_| Vec::new()).collect(),
            param_node_indices: Vec::new(),
            param_node_spans: Vec::new(),
            ir: None,
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new_empty();
    }

    pub fn sync_to_descriptor(&mut self, desc: &EffectDescriptor, node_id: u32) {
        self.sync_to_descriptor_with_modulator(desc, node_id, self.modulator_node_id);
    }

    pub fn sync_to_descriptor_with_modulator(
        &mut self,
        desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        let new_np = desc.params.len();
        let old_defaults = self.defaults.clone();
        let old_plocks = self.plocks.clone();

        self.node_id = node_id;
        self.modulator_node_id = modulator_node_id;
        self.num_params = new_np as u32;
        self.defaults = desc.params.iter().map(|p| p.default).collect();
        self.param_node_indices = desc.params.iter().map(|p| p.node_param_idx).collect();
        self.param_node_spans = desc
            .params
            .iter()
            .map(|p| p.node_param_span.max(1))
            .collect();
        self.plocks = (0..MAX_STEPS).map(|_| vec![None; new_np]).collect();
        self.plock_param_ids = (0..MAX_STEPS).map(|_| vec![None; new_np]).collect();

        let preserve = old_defaults.len().min(new_np);
        for i in 0..preserve {
            self.defaults[i] = old_defaults[i];
        }
        for step in 0..MAX_STEPS {
            if let Some(saved_step) = old_plocks.get(step) {
                for param_idx in 0..preserve.min(saved_step.len()) {
                    self.plocks[step][param_idx] = saved_step[param_idx];
                    self.plock_param_ids[step][param_idx] = self
                        .param_node_indices
                        .get(param_idx)
                        .copied()
                        .and_then(|raw_idx| {
                            ParamNodeId::from_slot_param(node_id, modulator_node_id, raw_idx)
                        });
                }
            }
        }
        self.recompute_modulation_active_params(desc);
    }

    pub fn recompute_modulation_active_params(&mut self, desc: &EffectDescriptor) {
        let mut active_indices = desc
            .instrument_modulation_targets
            .iter()
            .filter_map(|target| target.active_param_idx)
            .collect::<Vec<_>>();
        active_indices.sort_unstable();
        active_indices.dedup();

        for active_idx in active_indices {
            if active_idx >= self.defaults.len() {
                continue;
            }

            let group = desc
                .instrument_modulation_targets
                .iter()
                .filter(|target| target.active_param_idx == Some(active_idx))
                .collect::<Vec<_>>();
            if group.is_empty() {
                continue;
            }

            let default_active = group.iter().any(|target| {
                self.defaults
                    .get(target.depth_param_idx)
                    .copied()
                    .unwrap_or(0.0)
                    .abs()
                    > f32::EPSILON
            });
            self.defaults[active_idx] = if default_active { 1.0 } else { 0.0 };

            for step in 0..MAX_STEPS {
                let Some(step_plocks) = self.plocks.get_mut(step) else {
                    continue;
                };
                if active_idx >= step_plocks.len() {
                    continue;
                }
                let has_depth_plock = group.iter().any(|target| {
                    step_plocks
                        .get(target.depth_param_idx)
                        .copied()
                        .flatten()
                        .is_some()
                });
                if has_depth_plock {
                    let active = group.iter().any(|target| {
                        step_plocks
                            .get(target.depth_param_idx)
                            .copied()
                            .flatten()
                            .unwrap_or_else(|| {
                                self.defaults
                                    .get(target.depth_param_idx)
                                    .copied()
                                    .unwrap_or(0.0)
                            })
                            .abs()
                            > f32::EPSILON
                    });
                    step_plocks[active_idx] = Some(if active { 1.0 } else { 0.0 });
                } else {
                    step_plocks[active_idx] = None;
                }
            }
        }
    }
}

// ── Sync divisions (kept — orthogonal to effect system) ──

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncDivision {
    ThirtySecond = 0,
    Sixteenth = 1,
    SixteenthTriplet = 2,
    Eighth = 3,
    EighthTriplet = 4,
    EighthDotted = 5,
    Quarter = 6,
    QuarterTriplet = 7,
    QuarterDotted = 8,
    Half = 9,
    Whole = 10,
}

impl SyncDivision {
    pub const ALL: [SyncDivision; 11] = [
        SyncDivision::ThirtySecond,
        SyncDivision::Sixteenth,
        SyncDivision::SixteenthTriplet,
        SyncDivision::Eighth,
        SyncDivision::EighthTriplet,
        SyncDivision::EighthDotted,
        SyncDivision::Quarter,
        SyncDivision::QuarterTriplet,
        SyncDivision::QuarterDotted,
        SyncDivision::Half,
        SyncDivision::Whole,
    ];

    /// Duration in beats (quarter notes).
    pub fn to_beats(self) -> f64 {
        match self {
            SyncDivision::ThirtySecond => 0.125,
            SyncDivision::Sixteenth => 0.25,
            SyncDivision::SixteenthTriplet => 1.0 / 6.0,
            SyncDivision::Eighth => 0.5,
            SyncDivision::EighthTriplet => 1.0 / 3.0,
            SyncDivision::EighthDotted => 0.75,
            SyncDivision::Quarter => 1.0,
            SyncDivision::QuarterTriplet => 2.0 / 3.0,
            SyncDivision::QuarterDotted => 1.5,
            SyncDivision::Half => 2.0,
            SyncDivision::Whole => 4.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SyncDivision::ThirtySecond => "1/32",
            SyncDivision::Sixteenth => "1/16",
            SyncDivision::SixteenthTriplet => "1/16t",
            SyncDivision::Eighth => "1/8",
            SyncDivision::EighthTriplet => "1/8t",
            SyncDivision::EighthDotted => "1/8.",
            SyncDivision::Quarter => "1/4",
            SyncDivision::QuarterTriplet => "1/4t",
            SyncDivision::QuarterDotted => "1/4.",
            SyncDivision::Half => "1/2",
            SyncDivision::Whole => "1",
        }
    }

    pub fn from_index(idx: usize) -> SyncDivision {
        SyncDivision::ALL[idx.min(SyncDivision::ALL.len() - 1)]
    }
}
