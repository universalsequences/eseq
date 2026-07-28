//! Built-in audio effects and the shared effect-chain model.

#[allow(dead_code)]
pub mod compressor;
pub mod conv_reverb;
#[allow(dead_code)]
pub(crate) mod delay;
#[allow(dead_code)]
pub(crate) mod dimension;
#[allow(dead_code)]
pub(crate) mod dj_mixer;
#[allow(dead_code)]
pub(crate) mod dynamics;
#[allow(dead_code)]
pub(crate) mod eq8;
#[allow(dead_code)]
pub(crate) mod filter;
#[allow(dead_code)]
pub mod filterbank;
#[allow(dead_code)]
pub(crate) mod gatepitch;
#[allow(dead_code)]
pub(crate) mod limiter;
pub mod multiverb;
#[allow(dead_code)]
pub mod ott;
pub(crate) mod phaser_flanger;
#[allow(dead_code)]
pub mod reverb;
#[allow(dead_code)]
pub mod roar;
#[allow(dead_code)]
pub(crate) mod space_echo;
pub mod spring;
#[allow(dead_code)]
pub mod stereo_panner;
#[allow(dead_code)]
pub(crate) mod str8_delay;
#[allow(dead_code)]
pub(crate) mod tape;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::neural::ParamNodeId;
use crate::sequencer::MAX_STEPS;

/// Baseline storage capacity for per-slot defaults, p-locks, and node mappings.
/// Custom instruments include generated host-modulation controls in addition to
/// their declared DGen params, so dense synths can exceed 128 parameters.
pub const MAX_SLOT_PARAMS: usize = 512;
pub const MAX_SLOT_TENSOR_PARAMS: usize = 16;
pub const MAX_SLOT_TENSOR_PARAM_CELLS: usize = 64;
pub const MAX_SLOT_TENSOR_CELLS: usize = MAX_SLOT_TENSOR_PARAMS * MAX_SLOT_TENSOR_PARAM_CELLS;
pub const MAX_MIDI_NOTES: usize = 128;

/// Number of fixed built-in effect slots. Built-ins are now ordinary inserts,
/// so track effect chains start at slot 0.
pub const BUILTIN_SLOT_COUNT: usize = 0;
pub const NO_TRANSPORT_PHASE_PARAM: u32 = u32::MAX;

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
    pub tags: Vec<String>,
}

impl ParamUiMetadata {
    pub fn new(group: Option<String>, env: Option<String>, role: Option<String>) -> Option<Self> {
        Self::with_tags(group, env, role, Vec::new())
    }

    pub fn with_tags(
        group: Option<String>,
        env: Option<String>,
        role: Option<String>,
        tags: Vec<String>,
    ) -> Option<Self> {
        let tags = normalized_param_tags(tags);
        if group.is_none() && env.is_none() && role.is_none() && tags.is_empty() {
            None
        } else {
            Some(Self {
                group,
                env,
                role,
                tags,
            })
        }
    }
}

fn normalized_param_tags(tags: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for tag in tags {
        let normalized = tag.trim_start_matches(':').to_ascii_lowercase();
        if !normalized.is_empty() && !out.contains(&normalized) {
            out.push(normalized);
        }
    }
    out
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
    pub fn has_tag_or_name(&self, tag_or_name: &str) -> bool {
        let needle = tag_or_name.trim_start_matches(':').to_ascii_lowercase();
        if self.name.eq_ignore_ascii_case(&needle) {
            return true;
        }
        self.ui_metadata.as_ref().is_some_and(|metadata| {
            metadata
                .role
                .as_ref()
                .is_some_and(|role| role.trim_start_matches(':').eq_ignore_ascii_case(&needle))
                || metadata
                    .tags
                    .iter()
                    .any(|tag| tag.eq_ignore_ascii_case(&needle))
        })
    }

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
    use std::collections::BTreeMap;
    use std::sync::atomic::Ordering;

    use super::{
        tensor_param_descriptors_from_manifest, EffectDescriptor, EffectSlotSnapshot,
        EffectSlotState, HostControl, ParamDescriptor, ParamKind, ParamScaling,
        TensorParamDescriptor,
    };
    use crate::lisp_host::{TensorInit, TensorMeta};
    use crate::neural::ParamNodeId;

    fn tensor_meta(name: &str, cell_offset: usize, shape: Vec<usize>, mutable: bool) -> TensorMeta {
        TensorMeta {
            name: name.to_string(),
            cell_offset,
            shape,
            kind: "f32".to_string(),
            mutable,
            source_file: None,
            source_sample_rate: None,
        }
    }

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
            tensor_params: Vec::new(),
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
    fn tensor_param_descriptors_expose_small_named_mutable_tensors() {
        let tensors = vec![
            tensor_meta("strike_mask", 64, vec![2, 2], true),
            tensor_meta("", 80, vec![2, 2], true),
            tensor_meta("immutable", 96, vec![2, 2], false),
            tensor_meta("rank3", 112, vec![2, 2, 2], true),
        ];
        let init = vec![TensorInit {
            offset: 64,
            data: vec![-0.25, 0.25, 1.5, f32::NAN, 0.75],
        }];

        let descriptors = tensor_param_descriptors_from_manifest(&tensors, &init);

        assert_eq!(descriptors.len(), 1);
        let tensor = &descriptors[0];
        assert_eq!(tensor.name, "strike_mask");
        assert_eq!(tensor.shape, vec![2, 2]);
        assert_eq!(tensor.cell_offset, 64);
        assert_eq!(tensor.rows(), 2);
        assert_eq!(tensor.cols(), 2);
        assert_eq!(tensor.default, vec![0.0, 0.25, 1.0, 0.0]);
        assert_eq!((tensor.min, tensor.max), (0.0, 1.0));
    }

    #[test]
    fn tensor_param_descriptors_hide_large_mutable_tensors() {
        let tensors = vec![tensor_meta("ir_buffer", 128, vec![128, 1024], true)];

        let descriptors = tensor_param_descriptors_from_manifest(&tensors, &[]);

        assert!(
            descriptors.is_empty(),
            "large mutable IR-style tensors must not become user parameters"
        );
    }

    #[test]
    fn slot_tensor_params_capture_restore_defaults_and_whole_matrix_plocks() {
        let desc = EffectDescriptor {
            name: "tensor instrument".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: vec![TensorParamDescriptor {
                name: "strike_mask".to_string(),
                shape: vec![2, 2],
                cell_offset: 64,
                default: vec![0.1, 0.2, 0.3, 0.4],
                min: 0.0,
                max: 1.0,
            }],
            params: Vec::new(),
        };
        let slot = EffectSlotState::new(&desc, 10);

        let edited = slot
            .tensor_params
            .set_plock_cell(7, 0, 2, 0.95)
            .expect("tensor p-lock edit");

        assert_eq!(edited, vec![0.1, 0.2, 0.95, 0.4]);
        assert_eq!(
            slot.tensor_params.default_values(0).unwrap(),
            vec![0.1, 0.2, 0.3, 0.4]
        );
        assert_eq!(slot.tensor_params.plock_values(7, 0).unwrap(), edited);

        let snapshot = EffectSlotSnapshot::capture(&slot);
        assert_eq!(snapshot.tensor_params.len(), 1);
        assert_eq!(snapshot.tensor_params[0].default, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(
            snapshot.tensor_params[0].plocks[7],
            Some(vec![0.1, 0.2, 0.95, 0.4])
        );

        let mut destination = snapshot.clone();
        destination.tensor_params[0].default = vec![0.0; 4];
        destination.tensor_params[0].plocks[7] = Some(vec![0.8, 0.7, 0.6, 0.5]);
        destination.copy_base_values_from(&snapshot);
        assert_eq!(
            destination.tensor_params[0].default,
            vec![0.1, 0.2, 0.3, 0.4]
        );
        assert_eq!(
            destination.tensor_params[0].plocks[7],
            Some(vec![0.8, 0.7, 0.6, 0.5]),
            "copying scene defaults must preserve the destination scene's tensor p-locks"
        );

        let restored = EffectSlotState::empty();
        snapshot.restore(&restored);
        assert_eq!(
            restored.tensor_params.default_values(0).unwrap(),
            vec![0.1, 0.2, 0.3, 0.4]
        );
        assert_eq!(
            restored.tensor_params.plock_values(7, 0).unwrap(),
            vec![0.1, 0.2, 0.95, 0.4]
        );
    }

    #[test]
    fn device_value_snapshot_retains_prepared_convolution_ir_for_fileless_redo() {
        let descriptor = EffectDescriptor::builtin_filter();
        let slot = EffectSlotState::new(&descriptor, 424_242);
        let prepared = std::sync::Arc::new(crate::effects::conv_reverb::StereoIr {
            left: crate::effects::conv_reverb::ChannelIr {
                re: vec![1.0, -0.25],
                im: vec![0.0, 0.5],
            },
            right: crate::effects::conv_reverb::ChannelIr {
                re: vec![0.75, -0.5],
                im: vec![0.25, 0.0],
            },
        });
        crate::effects::conv_reverb::record_prepared_ir(
            424_242,
            "plate-a",
            "Plate A",
            prepared.clone(),
        );

        let snapshot = EffectSlotSnapshot::capture_authoring_values(&slot);

        assert_eq!(snapshot.ir.as_deref(), Some("plate-a"));
        assert!(std::sync::Arc::ptr_eq(
            snapshot.prepared_ir.as_ref().expect("prepared IR memento"),
            &prepared,
        ));
        assert!(snapshot.bit_exact_eq(&snapshot.clone()));
    }

    #[test]
    fn sync_to_descriptor_rebinds_loaded_plock_and_key_lock_ids_to_live_node_id() {
        let desc = EffectDescriptor {
            name: "test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
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
            key_locks: BTreeMap::from([(69, vec![Some(0.7)])]),
            key_lock_param_ids: BTreeMap::from([(
                69,
                vec![Some(ParamNodeId {
                    logical_id: 10,
                    node_param_idx: 15,
                })],
            )]),
            param_node_indices: vec![15],
            param_node_spans: vec![1],
            transport_phase_param_idx: crate::effects::NO_TRANSPORT_PHASE_PARAM,
            tensor_params: Vec::new(),
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
        assert_eq!(snapshot.key_locks[&69][0], Some(0.7));
        assert_eq!(
            snapshot.key_lock_param_ids[&69][0],
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
            tensor_params: Vec::new(),
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
            tensor_params: Vec::new(),
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
            tensor_params: Vec::new(),
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
            tensor_params: Vec::new(),
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
            tensor_params: Vec::new(),
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
            tensor_params: Vec::new(),
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
            tensor_params: Vec::new(),
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
            tensor_params: Vec::new(),
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
            crate::effects::dynamics::DYNAMICS_PARAM_AMOUNT as u32
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
    fn builtin_roar_exposes_stage_params_within_state_bounds() {
        let descriptor = EffectDescriptor::builtin_roar();
        // The roar panel (ui/effects/builtin/roar.lisp) looks these up by
        // name; renaming any of them breaks the custom UI.
        for name in [
            "enabled",
            "drive",
            "tone",
            "tone freq",
            "tone mode",
            "routing",
            "blend",
            "xover low",
            "xover high",
            "fb mode",
            "fb time",
            "fb div",
            "fb amount",
            "fb invert",
            "fb duck",
            "fb freq",
            "fb width",
            "compress",
            "sc hpf",
            "output",
            "dry/wet",
        ] {
            assert!(
                descriptor.params.iter().any(|param| param.name == name),
                "Roar descriptor should expose {name:?}"
            );
        }
        for prefix in ["s1", "s2", "s3"] {
            for field in [
                "shaper", "amount", "bias", "level", "filter", "freq", "res", "pre",
            ] {
                let name = format!("{prefix} {field}");
                assert!(
                    descriptor.params.iter().any(|param| param.name == name),
                    "Roar descriptor should expose {name:?}"
                );
            }
        }
        // 21 globals + 24 stage params + the shared modulator block + 4
        // targets × 4 mod-depth slots.
        let modulator_params = crate::instruments::voice_modulator::effect_param_descriptors().len();
        assert_eq!(descriptor.params.len(), 21 + 24 + modulator_params + 16);
        assert_eq!(descriptor.instrument_modulation_targets.len(), 16);
        for (idx, param) in descriptor.params.iter().enumerate() {
            // The shared voice-modulator block (indices 45..45+block) writes
            // into the modulator node's state, not Roar's.
            if idx < 45 || param.name.ends_with(" amt") {
                assert!(
                    (param.node_param_idx as usize) < crate::effects::roar::ROAR_STATE_SIZE,
                    "param {:?} writes outside the Roar state array",
                    param.name
                );
            }
            assert!(
                param.default >= param.min && param.default <= param.max,
                "param {:?} default out of range",
                param.name
            );
        }
        let routing = descriptor
            .params
            .iter()
            .find(|param| param.name == "routing")
            .unwrap();
        match &routing.kind {
            ParamKind::Enum { labels } => assert_eq!(
                labels,
                &vec![
                    "single".to_string(),
                    "serial".to_string(),
                    "parallel".to_string(),
                    "multi band".to_string(),
                    "mid side".to_string(),
                    "feedback".to_string(),
                    "delay".to_string(),
                ]
            ),
            other => panic!("routing should be enum, got {other:?}"),
        }
    }

    #[test]
    fn builtin_ott_exposes_per_band_dynamics_params_within_state_bounds() {
        let descriptor = EffectDescriptor::builtin_ott();
        // The multiband panel (ui/effects/builtin/multiband.lisp) looks
        // these up by name; renaming any of them breaks the custom UI.
        for prefix in ["low", "mid", "high"] {
            for field in [
                "below thr",
                "below ratio",
                "above thr",
                "above ratio",
                "attack",
                "release",
                "input",
                "output",
                "on",
                "solo",
            ] {
                let name = format!("{prefix} {field}");
                assert!(
                    descriptor.params.iter().any(|param| param.name == name),
                    "OTT descriptor should expose {name:?}"
                );
            }
        }
        for name in [
            "low split",
            "high split",
            "xover low",
            "xover high",
            "soft knee",
            "rms",
            "output",
            "time",
            "amount",
            "enabled",
        ] {
            assert!(
                descriptor.params.iter().any(|param| param.name == name),
                "OTT descriptor should expose {name:?}"
            );
        }
        assert_eq!(descriptor.params.len(), 40);
        for param in &descriptor.params {
            assert!(
                (param.node_param_idx as usize) < crate::effects::ott::OTT_STATE_SIZE,
                "param {:?} writes outside the OTT state array",
                param.name
            );
            assert!(
                param.default >= param.min && param.default <= param.max,
                "param {:?} default out of range",
                param.name
            );
        }
    }

    #[test]
    fn builtin_multiverb_pins_param_order_within_state_bounds() {
        let descriptor = EffectDescriptor::builtin_insert("Multiverb").unwrap();
        assert_eq!(
            descriptor.input_channels,
            2 + crate::instruments::voice_modulator::NUM_OUTPUTS
        );
        assert_eq!(descriptor.output_channels, 2);
        // Param order is append-only (plocks persist by descriptor index) —
        // this list may only ever grow at the end.
        let names: Vec<&str> = descriptor
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect();
        let base_param_count = 14;
        assert_eq!(
            names[..base_param_count],
            [
                "mode",
                "decay",
                "size",
                "predelay",
                "damp",
                "bass",
                "diffusion",
                "mod rate",
                "mod depth",
                "mod shape",
                "era",
                "width",
                "mix",
                "enabled",
            ]
        );
        let source_descriptors = crate::instruments::voice_modulator::effect_param_descriptors();
        let source_names = source_descriptors
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names[base_param_count..base_param_count + source_names.len()],
            source_names,
            "standard effect-modulator source controls must follow the locked base params"
        );
        assert_eq!(
            names[base_param_count + source_names.len()..],
            [
                "mod decay slot 1 amt",
                "mod decay slot 2 amt",
                "mod decay slot 3 amt",
                "mod decay slot 4 amt",
                "mod size slot 1 amt",
                "mod size slot 2 amt",
                "mod size slot 3 amt",
                "mod size slot 4 amt",
                "mod depth slot 1 amt",
                "mod depth slot 2 amt",
                "mod depth slot 3 amt",
                "mod depth slot 4 amt",
                "mod mix slot 1 amt",
                "mod mix slot 2 amt",
                "mod mix slot 3 amt",
                "mod mix slot 4 amt",
            ]
        );
        assert_eq!(
            descriptor.instrument_modulators.len(),
            crate::instruments::voice_modulator::SLOT_COUNT
        );
        assert_eq!(
            descriptor.instrument_modulation_targets.len(),
            crate::instruments::voice_modulator::SLOT_COUNT * 4
        );
        for param in &descriptor.params {
            if !crate::instruments::voice_modulator::is_source_param(param.node_param_idx) {
                assert!(
                    (param.node_param_idx as usize)
                        < crate::effects::multiverb::MULTIVERB_STATE_SIZE,
                    "param {:?} writes outside the Multiverb state array",
                    param.name
                );
            }
            assert!(
                param.default >= param.min && param.default <= param.max,
                "param {:?} default out of range",
                param.name
            );
        }
    }

    #[test]
    fn builtin_compressor_names_are_canonical_and_legacy_dynamics_loads() {
        assert_eq!(
            EffectDescriptor::builtin_insert_names(),
            &[
                "Filter",
                "EQ8",
                "Delay",
                "Str8 Delay",
                "Space Echo",
                "Dimension",
                "Phaser-Flanger",
                "Roar",
                "DJ Mixer",
                "Reverb",
                "Multiverb",
                "444 Compressor",
                "Glue Compressor",
                "Compressor",
                "OTT",
                "Limiter",
                "Tape",
                "Filterbank"
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
        assert_eq!(EffectDescriptor::builtin_insert("eq8").unwrap().name, "EQ8");
    }

    #[test]
    fn builtin_eq8_exposes_expected_band_params_and_defaults() {
        let desc = EffectDescriptor::builtin_insert("EQ8").unwrap();
        assert_eq!(desc.input_channels, 2);
        assert_eq!(desc.output_channels, 2);
        assert_eq!(
            desc.params.len(),
            1 + crate::effects::eq8::EQ8_NUM_BANDS * 5
        );
        assert_eq!(desc.params[0].name, "enabled");
        assert_eq!(desc.params[0].default, 1.0);
        assert_eq!(
            desc.params[0].node_param_idx,
            crate::effects::eq8::EQ8_PARAM_ENABLED as u32
        );

        let names: Vec<&str> = desc
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect();
        assert_eq!(
            &names[1..6],
            vec!["b1 enabled", "b1 type", "b1 freq", "b1 gain", "b1 q"]
        );
        assert_eq!(
            desc.params[1].node_param_idx,
            crate::effects::eq8::eq8_band_enabled_param_idx(0) as u32
        );
        assert_eq!(
            desc.params[2].node_param_idx,
            crate::effects::eq8::eq8_band_type_param_idx(0) as u32
        );
        assert_eq!(desc.params[3].default, 80.0);
        assert_eq!(desc.params[4].default, 0.0);
        assert_eq!(desc.params[5].default, 0.707);
        assert_eq!(desc.params[21].default, 0.0);
        assert_eq!(desc.params[21].name, "b5 enabled");
        assert_eq!(desc.params[23].default, 1500.0);

        match &desc.params[2].kind {
            ParamKind::Enum { labels } => {
                assert_eq!(
                    labels.iter().map(String::as_str).collect::<Vec<_>>(),
                    vec!["lowshelf", "bell", "highshelf"]
                );
            }
            other => panic!("EQ8 band type should be enum, got {other:?}"),
        }
        assert_eq!(
            EffectDescriptor::builtin_insert_project_name("EQ8").as_deref(),
            Some("builtin:EQ8")
        );
        assert_eq!(
            EffectDescriptor::strip_builtin_insert_project_name("builtin:EQ8"),
            Some("EQ8")
        );
    }

    #[test]
    fn builtin_reverb_insert_is_stereo_in_and_stereo_out() {
        let desc = EffectDescriptor::builtin_insert("Reverb").unwrap();
        assert_eq!(desc.input_channels, 2);
        assert_eq!(desc.output_channels, 2);
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

        assert_eq!(desc.input_channels, 2 + crate::instruments::voice_modulator::NUM_OUTPUTS);
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
    fn builtin_space_echo_exposes_re201_params() {
        let desc = EffectDescriptor::builtin_space_echo();
        let names: Vec<&str> = desc.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            &names[..16],
            vec![
                "enabled",
                "mode",
                "repeat rate",
                "sync",
                "sync div",
                "sync offset",
                "intensity",
                "bass",
                "treble",
                "echo volume",
                "reverb volume",
                "dry",
                "input drive",
                "wow/flutter",
                "tape age",
                "mod1_source",
            ]
        );
        assert_eq!(desc.input_channels, 2 + crate::instruments::voice_modulator::NUM_OUTPUTS);
        assert_eq!(desc.instrument_modulators.len(), 4);
        // Spring tension, spring type, and stereo width ride at the end (after
        // the modulator params) so stored plock param indices stay stable.
        let n = desc.params.len();
        let tension = &desc.params[n - 3];
        assert_eq!(tension.name, "tension");
        assert_eq!(tension.default, 0.5);
        let spring_type = &desc.params[n - 2];
        assert_eq!(spring_type.name, "spring type");
        match &spring_type.kind {
            ParamKind::Enum { labels } => {
                assert_eq!(labels.len(), 2);
                assert_eq!(labels[1], "King Tubby");
            }
            other => panic!("spring type should be enum, got {other:?}"),
        }
        let width = &desc.params[n - 1];
        assert_eq!(width.name, "stereo width");
        assert_eq!(width.default, 0.7);
        match &desc.params[1].kind {
            ParamKind::Enum { labels } => {
                assert_eq!(labels.len(), 12);
                assert_eq!(labels[11], "12: reverb only");
            }
            other => panic!("mode should be enum, got {other:?}"),
        }
        match &desc.params[4].kind {
            ParamKind::Enum { labels } => assert_eq!(labels[6], "1/4"),
            other => panic!("sync div should be enum, got {other:?}"),
        }
        let target_names: Vec<&str> = desc
            .instrument_modulation_targets
            .iter()
            .map(|target| desc.params[target.base_param_idx].name.as_str())
            .collect();
        assert_eq!(target_names.len(), 16);
        for name in ["repeat rate", "intensity", "echo volume", "reverb volume"] {
            assert_eq!(
                target_names.iter().filter(|n| **n == name).count(),
                4,
                "{name} should have 4 modulation slots"
            );
        }
    }

    #[test]
    fn builtin_filterbank_exposes_sherman_params_and_mod_targets() {
        let desc = EffectDescriptor::builtin_insert("Filterbank").unwrap();
        assert_eq!(desc.name, "Filterbank");
        // 0/1 audio, 2..5 ext-mod sources, 6/7 FM/AM sidechains.
        assert_eq!(
            desc.input_channels,
            2 + crate::instruments::voice_modulator::NUM_OUTPUTS + 2
        );
        assert_eq!(desc.output_channels, 2);
        assert_eq!(desc.instrument_modulators.len(), 4);

        let names: Vec<&str> = desc.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            &names[..40],
            vec![
                "enabled",
                "input",
                "hi eq",
                "sense",
                "noise",
                "feedback",
                "crunch",
                "correction",
                "ser/par",
                "harmonics",
                "fm amount",
                "fm source",
                "am depth",
                "am source",
                "env mode",
                "attack",
                "decay",
                "sustain",
                "release",
                "env f1",
                "env f2",
                "res bleed",
                "lfo rate",
                "lfo wave",
                "lfo depth",
                "lfo trig",
                "lfo sync",
                "lfo div",
                "ar attack",
                "ar release",
                "ar depth",
                "stereo split",
                "output",
                "dry/wet",
                "f1 freq",
                "f1 res",
                "f1 mode",
                "f2 freq",
                "f2 res",
                "f2 mode",
            ]
        );

        // FM/AM sources are host-routed sidechains on ports 6/7.
        assert!(matches!(
            desc.params[11].host_control,
            Some(HostControl::FxSidechain { input_channel })
                if input_channel == crate::effects::filterbank::FILTERBANK_FM_INPUT_CHANNEL
        ));
        assert!(matches!(
            desc.params[13].host_control,
            Some(HostControl::FxSidechain { input_channel })
                if input_channel == crate::effects::filterbank::FILTERBANK_AM_INPUT_CHANNEL
        ));

        match &desc.params[9].kind {
            ParamKind::Enum { labels } => {
                assert_eq!(labels.len(), 12);
                assert_eq!(labels[0], "Free");
                assert_eq!(labels[11], "16");
            }
            other => panic!("harmonics should be enum, got {other:?}"),
        }
        match &desc.params[2].kind {
            ParamKind::Enum { labels } => {
                assert_eq!(
                    labels.iter().map(String::as_str).collect::<Vec<_>>(),
                    vec!["Cut", "Flat", "Boost"]
                );
            }
            other => panic!("hi eq should be enum, got {other:?}"),
        }
        match &desc.params[23].kind {
            ParamKind::Enum { labels } => {
                assert_eq!(
                    labels.iter().map(String::as_str).collect::<Vec<_>>(),
                    vec!["Sine", "Saw", "Ramp", "Square"]
                );
            }
            other => panic!("lfo wave should be enum, got {other:?}"),
        }

        // §5a: 19 targets × 4 slots, base params resolved by name.
        assert_eq!(desc.instrument_modulation_targets.len(), 76);
        let target_names: Vec<&str> = desc
            .instrument_modulation_targets
            .iter()
            .map(|target| desc.params[target.base_param_idx].name.as_str())
            .collect();
        for name in [
            "f1 freq", "f2 freq", "f1 res", "f2 res", "f1 mode", "f2 mode", "fm amount",
            "am depth", "ser/par", "crunch", "sense", "attack", "decay", "sustain", "release",
            "lfo rate", "lfo depth", "ar attack", "ar release",
        ] {
            assert_eq!(
                target_names.iter().filter(|n| **n == name).count(),
                4,
                "{name} should have 4 modulation slots"
            );
        }
        for target in &desc.instrument_modulation_targets {
            let depth = &desc.params[target.depth_param_idx];
            assert!(depth.name.starts_with("mod "), "depth param {:?}", depth.name);
            assert_eq!((depth.min, depth.max), (-1.0, 1.0));
        }

        // LFO tempo sync: division enum next to the other LFO controls.
        match &desc.params[27].kind {
            ParamKind::Enum { labels } => {
                assert_eq!(labels.len(), 11);
                assert_eq!(labels[6], "1/4");
            }
            other => panic!("lfo div should be enum, got {other:?}"),
        }

        // Every non-modulator param writes inside the node state array.
        for param in &desc.params {
            if param.node_param_idx == u32::MAX {
                continue; // host-routed sidechain selectors
            }
            if !crate::instruments::voice_modulator::is_source_param(param.node_param_idx) {
                assert!(
                    (param.node_param_idx as usize)
                        < crate::effects::filterbank::FILTERBANK_STATE_SIZE,
                    "param {:?} writes outside the Filterbank state array",
                    param.name
                );
            }
            assert!(
                param.default >= param.min && param.default <= param.max,
                "param {:?} default out of range",
                param.name
            );
        }
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
        assert_eq!(desc.input_channels, 2 + crate::instruments::voice_modulator::NUM_OUTPUTS);
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
            crate::effects::str8_delay::STR8_DELAY_PARAM_FILTER_FREQ as u32
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
        assert_eq!(
            &names[..7],
            &["enabled", "speed", "length", "loop", "sync", "div", "warp"]
        );
        assert_eq!(desc.input_channels, 2 + crate::instruments::voice_modulator::NUM_OUTPUTS);
        assert_eq!(desc.instrument_modulators.len(), 4);
        // 5 modulatable targets × 4 slots
        assert_eq!(desc.instrument_modulation_targets.len(), 20);
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
            crate::effects::dj_mixer::DJ_MIXER_PARAM_LENGTH_SEC as u32
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
            crate::instruments::sampler::SAMPLER_MOD_LANES_PER_PARAM * 6
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
            crate::instruments::sampler::PARAM_SCRUB_SMOOTH_TIME_MS as u32
        );
        assert_eq!((smooth.min, smooth.max, smooth.default), (0.0, 250.0, 6.0));
    }
}

// ── EffectDescriptor ──

#[derive(Clone, Debug, PartialEq)]
pub struct TensorParamDescriptor {
    pub name: String,
    pub shape: Vec<usize>,
    pub cell_offset: usize,
    pub default: Vec<f32>,
    pub min: f32,
    pub max: f32,
}

impl TensorParamDescriptor {
    pub fn cell_count(&self) -> usize {
        self.shape.iter().copied().product()
    }

    pub fn rows(&self) -> usize {
        match self.shape.as_slice() {
            [cols] if *cols > 0 => 1,
            [rows, _cols] if *rows > 0 => *rows,
            _ => 0,
        }
    }

    pub fn cols(&self) -> usize {
        match self.shape.as_slice() {
            [cols] if *cols > 0 => *cols,
            [_rows, cols] if *cols > 0 => *cols,
            _ => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TensorParamSnapshot {
    pub name: String,
    pub shape: Vec<usize>,
    pub cell_offset: usize,
    pub default: Vec<f32>,
    pub plocks: Vec<Option<Vec<f32>>>,
}

impl TensorParamSnapshot {
    pub fn cell_count(&self) -> usize {
        self.shape.iter().copied().product()
    }

    fn same_identity_as_descriptor(&self, desc: &TensorParamDescriptor) -> bool {
        self.name == desc.name
            && self.shape == desc.shape
            && self.default.len() == desc.default.len()
    }
}

fn exposed_tensor_cell_count(shape: &[usize]) -> Option<usize> {
    if !(1..=2).contains(&shape.len()) {
        return None;
    }
    let mut count = 1usize;
    for dim in shape {
        if *dim == 0 {
            return None;
        }
        count = count.checked_mul(*dim)?;
    }
    (count <= MAX_SLOT_TENSOR_PARAM_CELLS).then_some(count)
}

fn clamped_tensor_cell(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

pub fn tensor_param_descriptors_from_manifest(
    tensors: &[crate::lisp_host::TensorMeta],
    tensor_init_data: &[crate::lisp_host::TensorInit],
) -> Vec<TensorParamDescriptor> {
    let mut descriptors = Vec::new();
    for tensor in tensors {
        if descriptors.len() >= MAX_SLOT_TENSOR_PARAMS {
            break;
        }
        let name = tensor.name.trim();
        if name.is_empty() || !tensor.mutable {
            continue;
        }
        let Some(cell_count) = exposed_tensor_cell_count(&tensor.shape) else {
            continue;
        };
        let init = tensor_init_data
            .iter()
            .find(|init| init.offset == tensor.cell_offset);
        let mut default = init
            .map(|init| {
                init.data
                    .iter()
                    .copied()
                    .take(cell_count)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        default.resize(cell_count, 0.0);
        for value in &mut default {
            *value = clamped_tensor_cell(*value);
        }
        descriptors.push(TensorParamDescriptor {
            name: name.to_string(),
            shape: tensor.shape.clone(),
            cell_offset: tensor.cell_offset,
            default,
            min: 0.0,
            max: 1.0,
        });
    }
    descriptors
}

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
    pub tensor_params: Vec<TensorParamDescriptor>,
    pub input_channels: usize,
    pub output_channels: usize,
    pub instrument_modulators: Vec<InstrumentModulatorDescriptor>,
    pub instrument_modulation_targets: Vec<InstrumentModulationTarget>,
}

impl EffectDescriptor {
    pub const BUILTIN_INSERT_PREFIX: &'static str = "builtin:";

    pub fn transport_phase_param_idx(&self) -> Option<u32> {
        if self.name == "DJ Mixer" {
            Some(crate::effects::dj_mixer::DJ_MIXER_PARAM_TRANSPORT_BEAT_PHASE as u32)
        } else {
            None
        }
    }

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
            "EQ8",
            "Delay",
            "Str8 Delay",
            "Space Echo",
            "Dimension",
            "Phaser-Flanger",
            "Roar",
            "DJ Mixer",
            "Reverb",
            "Multiverb",
            "444 Compressor",
            "Glue Compressor",
            "Compressor",
            "OTT",
            "Limiter",
            "Tape",
            "Filterbank",
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
            "EQ8" => Some(Self::builtin_eq8()),
            "Delay" => Some(Self::builtin_delay()),
            "Str8 Delay" => Some(Self::builtin_str8_delay()),
            "Space Echo" => Some(Self::builtin_space_echo()),
            "Dimension" => Some(Self::builtin_dimension()),
            "Phaser-Flanger" => Some(Self::builtin_phaser_flanger()),
            "Roar" => Some(Self::builtin_roar()),
            "DJ Mixer" => Some(Self::builtin_dj_mixer()),
            "Reverb" => Some(Self::builtin_reverb_insert()),
            "Multiverb" => Some(Self::builtin_multiverb()),
            "444 Compressor" => Some(Self::builtin_444_compressor()),
            "Glue Compressor" => Some(Self::builtin_glue_compressor()),
            "Compressor" => Some(Self::builtin_compressor()),
            "OTT" => Some(Self::builtin_ott()),
            "Limiter" => Some(Self::builtin_limiter()),
            "Tape" => Some(Self::builtin_tape()),
            "Filterbank" => Some(Self::builtin_filterbank()),
            _ => None,
        }
    }

    /// Built-in 8-band parametric equalizer descriptor.
    pub fn builtin_eq8() -> Self {
        fn continuous_param(
            name: String,
            min: f32,
            max: f32,
            default: f32,
            unit: Option<&str>,
            scaling: ParamScaling,
            node_param_idx: u32,
        ) -> ParamDescriptor {
            ParamDescriptor {
                name,
                min,
                max,
                default,
                kind: ParamKind::Continuous {
                    unit: unit.map(str::to_string),
                },
                scaling,
                node_param_idx,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        let mut params = vec![Self::enabled_param(
            crate::effects::eq8::EQ8_PARAM_ENABLED as u32,
            1.0,
        )];
        for (band, default) in crate::effects::eq8::EQ8_DEFAULT_BANDS.iter().enumerate() {
            let label = band + 1;
            params.push(Self::enabled_param(
                crate::effects::eq8::eq8_band_enabled_param_idx(band) as u32,
                if default.enabled { 1.0 } else { 0.0 },
            ));
            params.last_mut().unwrap().name = format!("b{label} enabled");
            params.push(ParamDescriptor {
                name: format!("b{label} type"),
                min: 0.0,
                max: 2.0,
                default: default.filter_type,
                kind: ParamKind::Enum {
                    labels: vec![
                        "lowshelf".to_string(),
                        "bell".to_string(),
                        "highshelf".to_string(),
                    ],
                },
                scaling: ParamScaling::Linear,
                node_param_idx: crate::effects::eq8::eq8_band_type_param_idx(band) as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            });
            params.push(continuous_param(
                format!("b{label} freq"),
                20.0,
                20_000.0,
                default.freq,
                Some("Hz"),
                ParamScaling::Exponential,
                crate::effects::eq8::eq8_band_freq_param_idx(band) as u32,
            ));
            params.push(continuous_param(
                format!("b{label} gain"),
                -24.0,
                24.0,
                default.gain,
                Some("dB"),
                ParamScaling::Linear,
                crate::effects::eq8::eq8_band_gain_param_idx(band) as u32,
            ));
            params.push(continuous_param(
                format!("b{label} q"),
                0.1,
                18.0,
                default.q,
                None,
                ParamScaling::Exponential,
                crate::effects::eq8::eq8_band_q_param_idx(band) as u32,
            ));
        }

        Self {
            name: "EQ8".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params,
        }
    }

    /// Built-in filter effect descriptor.
    pub fn builtin_filter() -> Self {
        let mut desc = Self {
            name: "Filter".to_string(),
            input_channels: 2 + crate::instruments::voice_modulator::NUM_OUTPUTS,
            output_channels: 2,
            instrument_modulators: (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::instruments::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                Self::enabled_param(crate::effects::filter::FILTER_PARAM_ENABLED as u32, 1.0),
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_MODE as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_CUTOFF as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_RESONANCE as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_DRIVE as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_WET as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_LFO_AMOUNT as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_LFO_RATE as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_LFO_SYNCED as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_LFO_DIVISION as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_LFO_WAVE as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_LFO_PHASE_OFFSET as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_ENV_AMOUNT as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_ENV_ATTACK_MS as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_ENV_RELEASE_MS as u32,
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
                    node_param_idx: crate::effects::filter::FILTER_PARAM_SLOPE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        desc.params
            .extend(crate::instruments::voice_modulator::effect_param_descriptors());
        let cutoff_idx = desc
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("built-in filter cutoff param should exist");
        let depth_params = [
            crate::effects::filter::FILTER_PARAM_MOD_CUTOFF_DEPTH_1,
            crate::effects::filter::FILTER_PARAM_MOD_CUTOFF_DEPTH_2,
            crate::effects::filter::FILTER_PARAM_MOD_CUTOFF_DEPTH_3,
            crate::effects::filter::FILTER_PARAM_MOD_CUTOFF_DEPTH_4,
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
            tensor_params: Vec::new(),
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
                    node_param_idx: crate::instruments::sampler::PARAM_ATTACK_SAMPLES as u32,
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
                    node_param_idx: crate::instruments::sampler::PARAM_RELEASE_SAMPLES as u32,
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
                    node_param_idx: crate::instruments::sampler::PARAM_START_POINT as u32,
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
                    node_param_idx: crate::instruments::sampler::PARAM_END_POINT as u32,
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
                Self::enabled_param(crate::effects::delay::DELAY_PARAM_ENABLED as u32, 1.0),
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
            input_channels: 2 + crate::instruments::voice_modulator::NUM_OUTPUTS,
            output_channels: 2,
            instrument_modulators: (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::instruments::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                Self::enabled_param(
                    crate::effects::str8_delay::STR8_DELAY_PARAM_ENABLED as u32,
                    1.0,
                ),
                ParamDescriptor {
                    name: "wet".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_WET as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_FEEDBACK as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_LEFT_SYNC as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_LEFT_DIV as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_LEFT_OFFSET as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_LEFT_TIME_MS
                        as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_RIGHT_SYNC as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_RIGHT_DIV as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_RIGHT_OFFSET
                        as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_RIGHT_TIME_MS
                        as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_FILTER_FREQ as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_FILTER_Q as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_RATE as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_AMOUNT as u32,
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
                    node_param_idx: crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_PHASE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        desc.params
            .extend(crate::instruments::voice_modulator::effect_param_descriptors());

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
             depth_params: [u64; crate::instruments::voice_modulator::SLOT_COUNT],
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
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_TIME_DEPTH_1,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_TIME_DEPTH_2,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_TIME_DEPTH_3,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_TIME_DEPTH_4,
            ],
            -1000.0,
            1000.0,
            Some("ms"),
        );
        append_depth_targets(
            wet_idx,
            "wet",
            [
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_WET_DEPTH_1,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_WET_DEPTH_2,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_WET_DEPTH_3,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_WET_DEPTH_4,
            ],
            -1.0,
            1.0,
            Some("%"),
        );
        append_depth_targets(
            feedback_idx,
            "feedback",
            [
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_FEEDBACK_DEPTH_1,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_FEEDBACK_DEPTH_2,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_FEEDBACK_DEPTH_3,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_FEEDBACK_DEPTH_4,
            ],
            -0.95,
            0.95,
            None,
        );
        append_depth_targets(
            cutoff_idx,
            "cutoff",
            [
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_CUTOFF_DEPTH_1,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_CUTOFF_DEPTH_2,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_CUTOFF_DEPTH_3,
                crate::effects::str8_delay::STR8_DELAY_PARAM_MOD_CUTOFF_DEPTH_4,
            ],
            -4.0,
            4.0,
            Some("oct"),
        );

        desc
    }

    /// Roland RE-201 Space Echo style multi-head tape delay + spring reverb.
    pub fn builtin_space_echo() -> Self {
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
        let mode_labels = vec![
            "1: head 1".to_string(),
            "2: head 2".to_string(),
            "3: head 3".to_string(),
            "4: heads 1+2".to_string(),
            "5: heads 2+3".to_string(),
            "6: heads 1+2+3".to_string(),
            "7: head 1 + rev".to_string(),
            "8: head 2 + rev".to_string(),
            "9: head 3 + rev".to_string(),
            "10: heads 1+2 + rev".to_string(),
            "11: heads 2+3 + rev".to_string(),
            "12: reverb only".to_string(),
        ];
        let mut desc = Self {
            name: "Space Echo".to_string(),
            input_channels: 2 + crate::instruments::voice_modulator::NUM_OUTPUTS,
            output_channels: 2,
            instrument_modulators: (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::instruments::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                Self::enabled_param(
                    crate::effects::space_echo::SPACE_ECHO_PARAM_ENABLED as u32,
                    1.0,
                ),
                ParamDescriptor {
                    name: "mode".to_string(),
                    min: 0.0,
                    max: 11.0,
                    default: 7.0,
                    kind: ParamKind::Enum {
                        labels: mode_labels,
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_MODE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "repeat rate".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_RATE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "sync".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_SYNC as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "sync div".to_string(),
                    min: 0.0,
                    max: 10.0,
                    default: 6.0,
                    kind: ParamKind::Enum {
                        labels: sync_labels(),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_SYNC_DIV as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "sync offset".to_string(),
                    min: -0.5,
                    max: 0.5,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_SYNC_OFFSET as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "intensity".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.45,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_INTENSITY as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "bass".to_string(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_BASS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "treble".to_string(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_TREBLE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "echo volume".to_string(),
                    min: 0.0,
                    max: 1.5,
                    default: 0.8,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_ECHO_VOL as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "reverb volume".to_string(),
                    min: 0.0,
                    max: 1.5,
                    default: 0.5,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_REVERB_VOL as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "dry".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_DRY as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "input drive".to_string(),
                    min: -12.0,
                    max: 24.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_INPUT_DB as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "wow/flutter".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.35,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_WOW_FLUTTER as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "tape age".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.3,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_AGE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        desc.params
            .extend(crate::instruments::voice_modulator::effect_param_descriptors());

        let rate_idx = desc
            .params
            .iter()
            .position(|param| param.name == "repeat rate")
            .expect("built-in Space Echo repeat rate param should exist");
        let intensity_idx = desc
            .params
            .iter()
            .position(|param| param.name == "intensity")
            .expect("built-in Space Echo intensity param should exist");
        let echo_idx = desc
            .params
            .iter()
            .position(|param| param.name == "echo volume")
            .expect("built-in Space Echo echo volume param should exist");
        let reverb_idx = desc
            .params
            .iter()
            .position(|param| param.name == "reverb volume")
            .expect("built-in Space Echo reverb volume param should exist");

        let mut append_depth_targets =
            |base_param_idx: usize,
             destination_name: &str,
             depth_params: [u64; crate::instruments::voice_modulator::SLOT_COUNT],
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
            rate_idx,
            "rate",
            [
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_RATE_DEPTH_1,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_RATE_DEPTH_2,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_RATE_DEPTH_3,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_RATE_DEPTH_4,
            ],
            -1.0,
            1.0,
            None,
        );
        append_depth_targets(
            intensity_idx,
            "intensity",
            [
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_INTENSITY_DEPTH_1,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_INTENSITY_DEPTH_2,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_INTENSITY_DEPTH_3,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_INTENSITY_DEPTH_4,
            ],
            -1.0,
            1.0,
            None,
        );
        append_depth_targets(
            echo_idx,
            "echo",
            [
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_ECHO_DEPTH_1,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_ECHO_DEPTH_2,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_ECHO_DEPTH_3,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_ECHO_DEPTH_4,
            ],
            -1.5,
            1.5,
            None,
        );
        append_depth_targets(
            reverb_idx,
            "reverb",
            [
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_REVERB_DEPTH_1,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_REVERB_DEPTH_2,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_REVERB_DEPTH_3,
                crate::effects::space_echo::SPACE_ECHO_PARAM_MOD_REVERB_DEPTH_4,
            ],
            -1.5,
            1.5,
            None,
        );

        // Spring tension macro — appended last so existing projects' plock /
        // mod param indices stay stable.
        desc.params.push(ParamDescriptor {
            name: "tension".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            kind: ParamKind::Continuous {
                unit: Some("%".to_string()),
            },
            scaling: ParamScaling::Linear,
            node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_TENSION as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        });

        // Spring type + stereo width — also appended last (in this order) so
        // existing projects' plock / mod param indices stay stable.
        desc.params.push(ParamDescriptor {
            name: "spring type".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            kind: ParamKind::Enum {
                labels: vec!["RE-201".to_string(), "King Tubby".to_string()],
            },
            scaling: ParamScaling::Linear,
            node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_SPRING_TYPE as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        });
        desc.params.push(ParamDescriptor {
            name: "stereo width".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.7,
            kind: ParamKind::Continuous {
                unit: Some("%".to_string()),
            },
            scaling: ParamScaling::Linear,
            node_param_idx: crate::effects::space_echo::SPACE_ECHO_PARAM_STEREO_WIDTH as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        });

        desc
    }

    /// Sherman Filterbank 2 style dual-filter mangler (spec:
    /// docs/sherman-filterbank-spec.md). Inputs 0/1 carry the track signal,
    /// 2..5 the host ext-mod sources, 6/7 the FM/AM sidechains.
    pub fn builtin_filterbank() -> Self {
        use crate::effects::filterbank as fb;

        fn continuous(
            name: &str,
            min: f32,
            max: f32,
            default: f32,
            unit: Option<&str>,
            scaling: ParamScaling,
            node_param_idx: u64,
        ) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min,
                max,
                default,
                kind: ParamKind::Continuous {
                    unit: unit.map(str::to_string),
                },
                scaling,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        fn toggle(name: &str, default: f32, node_param_idx: u64) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min: 0.0,
                max: 1.0,
                default,
                kind: ParamKind::Boolean,
                scaling: ParamScaling::Linear,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        fn options(
            name: &str,
            labels: &[&str],
            default: f32,
            node_param_idx: u64,
        ) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min: 0.0,
                max: (labels.len() - 1) as f32,
                default,
                kind: ParamKind::Enum {
                    labels: labels.iter().map(|label| label.to_string()).collect(),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        // Host-routed sidechain source (compressor precedent). Labels are
        // patched with the track list wherever the descriptor is
        // instantiated; `input_channel` picks the node input port.
        fn sidechain(name: &str, input_channel: usize) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min: 0.0,
                max: 0.0,
                default: 0.0,
                kind: ParamKind::Enum {
                    labels: vec!["off".to_string()],
                },
                scaling: ParamScaling::Linear,
                node_param_idx: u32::MAX,
                node_param_span: 1,
                host_control: Some(HostControl::FxSidechain { input_channel }),
                ui_metadata: None,
            }
        }

        let harmonics_labels = [
            "Free", "1", "1.5", "2", "3", "4", "5", "6", "8", "9", "12", "16",
        ];

        let mut desc = Self {
            name: "Filterbank".to_string(),
            // 0/1 audio, 2..5 ext-mod sources, 6/7 FM/AM sidechains.
            input_channels: 2
                + crate::instruments::voice_modulator::NUM_OUTPUTS
                + 2,
            output_channels: 2,
            instrument_modulators: (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::instruments::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                Self::enabled_param(fb::FILTERBANK_PARAM_ENABLED as u32, 1.0),
                continuous(
                    "input",
                    -12.0,
                    30.0,
                    0.0,
                    Some("dB"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_INPUT_DB,
                ),
                options("hi eq", &["Cut", "Flat", "Boost"], 1.0, fb::FILTERBANK_PARAM_HI_EQ),
                continuous(
                    "sense",
                    0.0,
                    100.0,
                    30.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_SENSE,
                ),
                continuous(
                    "noise",
                    0.0,
                    100.0,
                    0.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_NOISE,
                ),
                continuous(
                    "feedback",
                    0.0,
                    100.0,
                    0.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_FEEDBACK,
                ),
                continuous(
                    "crunch",
                    0.0,
                    100.0,
                    25.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_CRUNCH,
                ),
                continuous(
                    "correction",
                    0.0,
                    100.0,
                    0.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_CORRECTION,
                ),
                continuous(
                    "ser/par",
                    0.0,
                    100.0,
                    100.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_SER_PAR,
                ),
                options(
                    "harmonics",
                    &harmonics_labels,
                    0.0,
                    fb::FILTERBANK_PARAM_HARMONICS,
                ),
                continuous(
                    "fm amount",
                    0.0,
                    100.0,
                    0.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_FM_AMOUNT,
                ),
                sidechain("fm source", fb::FILTERBANK_FM_INPUT_CHANNEL),
                continuous(
                    "am depth",
                    0.0,
                    100.0,
                    0.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_AM_DEPTH,
                ),
                sidechain("am source", fb::FILTERBANK_AM_INPUT_CHANNEL),
                options(
                    "env mode",
                    &["ADSR", "Follower"],
                    0.0,
                    fb::FILTERBANK_PARAM_ENV_MODE,
                ),
                continuous(
                    "attack",
                    0.5,
                    4000.0,
                    5.0,
                    Some("ms"),
                    ParamScaling::Exponential,
                    fb::FILTERBANK_PARAM_ATTACK_MS,
                ),
                continuous(
                    "decay",
                    1.0,
                    4000.0,
                    200.0,
                    Some("ms"),
                    ParamScaling::Exponential,
                    fb::FILTERBANK_PARAM_DECAY_MS,
                ),
                continuous(
                    "sustain",
                    -100.0,
                    100.0,
                    0.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_SUSTAIN,
                ),
                continuous(
                    "release",
                    1.0,
                    8000.0,
                    300.0,
                    Some("ms"),
                    ParamScaling::Exponential,
                    fb::FILTERBANK_PARAM_RELEASE_MS,
                ),
                continuous(
                    "env f1",
                    -100.0,
                    100.0,
                    50.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_ENV_F1,
                ),
                continuous(
                    "env f2",
                    -100.0,
                    100.0,
                    50.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_ENV_F2,
                ),
                continuous(
                    "res bleed",
                    0.0,
                    100.0,
                    10.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_RES_BLEED,
                ),
                continuous(
                    "lfo rate",
                    0.01,
                    2000.0,
                    0.5,
                    Some("Hz"),
                    ParamScaling::Exponential,
                    fb::FILTERBANK_PARAM_LFO_RATE,
                ),
                options(
                    "lfo wave",
                    &["Sine", "Saw", "Ramp", "Square"],
                    1.0,
                    fb::FILTERBANK_PARAM_LFO_WAVE,
                ),
                continuous(
                    "lfo depth",
                    -100.0,
                    100.0,
                    0.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_LFO_DEPTH,
                ),
                toggle("lfo trig", 0.0, fb::FILTERBANK_PARAM_LFO_TRIG),
                toggle("lfo sync", 0.0, fb::FILTERBANK_PARAM_LFO_SYNC),
                options(
                    "lfo div",
                    &[
                        "1/32", "1/16", "1/16t", "1/8", "1/8t", "1/8.", "1/4", "1/4t", "1/4.",
                        "1/2", "1",
                    ],
                    6.0,
                    fb::FILTERBANK_PARAM_LFO_DIV,
                ),
                continuous(
                    "ar attack",
                    0.5,
                    2000.0,
                    5.0,
                    Some("ms"),
                    ParamScaling::Exponential,
                    fb::FILTERBANK_PARAM_AR_ATTACK_MS,
                ),
                continuous(
                    "ar release",
                    1.0,
                    4000.0,
                    200.0,
                    Some("ms"),
                    ParamScaling::Exponential,
                    fb::FILTERBANK_PARAM_AR_RELEASE_MS,
                ),
                continuous(
                    "ar depth",
                    0.0,
                    100.0,
                    0.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_AR_DEPTH,
                ),
                toggle("stereo split", 0.0, fb::FILTERBANK_PARAM_STEREO_SPLIT),
                continuous(
                    "output",
                    -24.0,
                    24.0,
                    0.0,
                    Some("dB"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_OUTPUT_DB,
                ),
                continuous(
                    "dry/wet",
                    0.0,
                    100.0,
                    100.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_DRY_WET,
                ),
                continuous(
                    "f1 freq",
                    20.0,
                    16000.0,
                    500.0,
                    Some("Hz"),
                    ParamScaling::Exponential,
                    fb::FILTERBANK_PARAM_F1_FREQ,
                ),
                continuous(
                    "f1 res",
                    0.0,
                    110.0,
                    20.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_F1_RES,
                ),
                continuous(
                    "f1 mode",
                    0.0,
                    100.0,
                    0.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_F1_MODE,
                ),
                continuous(
                    "f2 freq",
                    20.0,
                    16000.0,
                    500.0,
                    Some("Hz"),
                    ParamScaling::Exponential,
                    fb::FILTERBANK_PARAM_F2_FREQ,
                ),
                continuous(
                    "f2 res",
                    0.0,
                    110.0,
                    20.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_F2_RES,
                ),
                continuous(
                    "f2 mode",
                    0.0,
                    100.0,
                    0.0,
                    Some("%"),
                    ParamScaling::Linear,
                    fb::FILTERBANK_PARAM_F2_MODE,
                ),
            ],
        };
        desc.params
            .extend(crate::instruments::voice_modulator::effect_param_descriptors());

        // §5a host mod targets: 10 targets × 4 slots, space-echo convention
        // (depth params + InstrumentModulationTarget entries the mod-wrapper
        // UI consumes).
        let mut append_depth_targets =
            |base_param_name: &str,
             destination_name: &str,
             depth_params: [u64; crate::instruments::voice_modulator::SLOT_COUNT]| {
                let base_param_idx = desc
                    .params
                    .iter()
                    .position(|param| param.name == base_param_name)
                    .unwrap_or_else(|| {
                        panic!("built-in Filterbank {base_param_name} param should exist")
                    });
                for (slot, node_param_idx) in depth_params.into_iter().enumerate() {
                    let depth_param_idx = desc.params.len();
                    desc.params.push(ParamDescriptor {
                        name: format!("mod {destination_name} slot {} amt", slot + 1),
                        min: -1.0,
                        max: 1.0,
                        default: 0.0,
                        kind: ParamKind::Continuous { unit: None },
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
                            depth_min: -1.0,
                            depth_max: 1.0,
                            depth_unit: None,
                        });
                }
            };

        append_depth_targets(
            "f1 freq",
            "f1 freq",
            [
                fb::FILTERBANK_PARAM_MOD_F1_FREQ_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_F1_FREQ_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_F1_FREQ_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_F1_FREQ_DEPTH_4,
            ],
        );
        append_depth_targets(
            "f2 freq",
            "f2 freq",
            [
                fb::FILTERBANK_PARAM_MOD_F2_FREQ_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_F2_FREQ_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_F2_FREQ_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_F2_FREQ_DEPTH_4,
            ],
        );
        append_depth_targets(
            "f1 res",
            "f1 res",
            [
                fb::FILTERBANK_PARAM_MOD_F1_RES_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_F1_RES_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_F1_RES_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_F1_RES_DEPTH_4,
            ],
        );
        append_depth_targets(
            "f2 res",
            "f2 res",
            [
                fb::FILTERBANK_PARAM_MOD_F2_RES_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_F2_RES_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_F2_RES_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_F2_RES_DEPTH_4,
            ],
        );
        append_depth_targets(
            "f1 mode",
            "f1 mode",
            [
                fb::FILTERBANK_PARAM_MOD_F1_MODE_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_F1_MODE_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_F1_MODE_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_F1_MODE_DEPTH_4,
            ],
        );
        append_depth_targets(
            "f2 mode",
            "f2 mode",
            [
                fb::FILTERBANK_PARAM_MOD_F2_MODE_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_F2_MODE_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_F2_MODE_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_F2_MODE_DEPTH_4,
            ],
        );
        append_depth_targets(
            "fm amount",
            "fm",
            [
                fb::FILTERBANK_PARAM_MOD_FM_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_FM_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_FM_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_FM_DEPTH_4,
            ],
        );
        append_depth_targets(
            "am depth",
            "am",
            [
                fb::FILTERBANK_PARAM_MOD_AM_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_AM_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_AM_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_AM_DEPTH_4,
            ],
        );
        append_depth_targets(
            "ser/par",
            "ser/par",
            [
                fb::FILTERBANK_PARAM_MOD_SER_PAR_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_SER_PAR_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_SER_PAR_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_SER_PAR_DEPTH_4,
            ],
        );
        append_depth_targets(
            "crunch",
            "crunch",
            [
                fb::FILTERBANK_PARAM_MOD_CRUNCH_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_CRUNCH_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_CRUNCH_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_CRUNCH_DEPTH_4,
            ],
        );
        // Second wave: performance controls (sense, envelope shapes, LFO).
        append_depth_targets(
            "sense",
            "sense",
            [
                fb::FILTERBANK_PARAM_MOD_SENSE_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_SENSE_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_SENSE_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_SENSE_DEPTH_4,
            ],
        );
        append_depth_targets(
            "attack",
            "attack",
            [
                fb::FILTERBANK_PARAM_MOD_ATTACK_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_ATTACK_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_ATTACK_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_ATTACK_DEPTH_4,
            ],
        );
        append_depth_targets(
            "decay",
            "decay",
            [
                fb::FILTERBANK_PARAM_MOD_DECAY_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_DECAY_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_DECAY_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_DECAY_DEPTH_4,
            ],
        );
        append_depth_targets(
            "sustain",
            "sustain",
            [
                fb::FILTERBANK_PARAM_MOD_SUSTAIN_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_SUSTAIN_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_SUSTAIN_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_SUSTAIN_DEPTH_4,
            ],
        );
        append_depth_targets(
            "release",
            "release",
            [
                fb::FILTERBANK_PARAM_MOD_RELEASE_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_RELEASE_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_RELEASE_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_RELEASE_DEPTH_4,
            ],
        );
        append_depth_targets(
            "lfo rate",
            "lfo rate",
            [
                fb::FILTERBANK_PARAM_MOD_LFO_RATE_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_LFO_RATE_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_LFO_RATE_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_LFO_RATE_DEPTH_4,
            ],
        );
        append_depth_targets(
            "lfo depth",
            "lfo depth",
            [
                fb::FILTERBANK_PARAM_MOD_LFO_DEPTH_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_LFO_DEPTH_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_LFO_DEPTH_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_LFO_DEPTH_DEPTH_4,
            ],
        );
        append_depth_targets(
            "ar attack",
            "ar attack",
            [
                fb::FILTERBANK_PARAM_MOD_AR_ATTACK_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_AR_ATTACK_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_AR_ATTACK_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_AR_ATTACK_DEPTH_4,
            ],
        );
        append_depth_targets(
            "ar release",
            "ar release",
            [
                fb::FILTERBANK_PARAM_MOD_AR_RELEASE_DEPTH_1,
                fb::FILTERBANK_PARAM_MOD_AR_RELEASE_DEPTH_2,
                fb::FILTERBANK_PARAM_MOD_AR_RELEASE_DEPTH_3,
                fb::FILTERBANK_PARAM_MOD_AR_RELEASE_DEPTH_4,
            ],
        );

        desc
    }

    /// Built-in Dimension chorus (Roland SDD-320 style): two antiphase
    /// BBD-voiced delay lines with an inverted stereo crossmix, compander,
    /// and band-limited wet path. No feedback anywhere.
    pub fn builtin_dimension() -> Self {
        let mut desc = Self {
            name: "Dimension".to_string(),
            input_channels: 2 + crate::instruments::voice_modulator::NUM_OUTPUTS,
            output_channels: 2,
            instrument_modulators: (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::instruments::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                Self::enabled_param(
                    crate::effects::dimension::DIMENSION_PARAM_ENABLED as u32,
                    1.0,
                ),
                ParamDescriptor {
                    name: "mode 1".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_BTN1 as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mode 2".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_BTN2 as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mode 3".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_BTN3 as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mode 4".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_BTN4 as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "dynamic color".to_string(),
                    min: 0.0,
                    max: 3.0,
                    default: 1.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "smooth".to_string(),
                            "default".to_string(),
                            "lf sat 1".to_string(),
                            "lf sat 2".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_COLOR as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "lfo shape".to_string(),
                    min: 0.0,
                    max: 4.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "default".to_string(),
                            "sine".to_string(),
                            "ramp".to_string(),
                            "square".to_string(),
                            "triangle".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_SHAPE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "rate".to_string(),
                    min: 0.25,
                    max: 4.0,
                    default: 1.0,
                    kind: ParamKind::Continuous {
                        unit: Some("x".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_RATE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "depth".to_string(),
                    min: 0.0,
                    max: 2.0,
                    default: 1.0,
                    kind: ParamKind::Continuous {
                        unit: Some("x".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_DEPTH as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "width".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_WIDTH as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "tone".to_string(),
                    min: 2000.0,
                    max: 16000.0,
                    default: 7200.0,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_TONE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mix".to_string(),
                    min: 0.0,
                    max: 1.5,
                    default: 0.7,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dimension::DIMENSION_PARAM_MIX as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        desc.params
            .extend(crate::instruments::voice_modulator::effect_param_descriptors());

        let depth_idx = desc
            .params
            .iter()
            .position(|param| param.name == "depth")
            .expect("built-in Dimension depth param should exist");
        let mix_idx = desc
            .params
            .iter()
            .position(|param| param.name == "mix")
            .expect("built-in Dimension mix param should exist");

        let mut append_depth_targets = |base_param_idx: usize,
                                        destination_name: &str,
                                        depth_params: [u64; crate::instruments::voice_modulator::SLOT_COUNT],
                                        depth_min: f32,
                                        depth_max: f32| {
            for (slot, node_param_idx) in depth_params.into_iter().enumerate() {
                let depth_param_idx = desc.params.len();
                desc.params.push(ParamDescriptor {
                    name: format!("mod {destination_name} slot {} amt", slot + 1),
                    min: depth_min,
                    max: depth_max,
                    default: 0.0,
                    kind: ParamKind::Continuous { unit: None },
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
                        depth_unit: None,
                    });
            }
        };

        append_depth_targets(
            depth_idx,
            "depth",
            [
                crate::effects::dimension::DIMENSION_PARAM_MOD_DEPTH_DEPTH_1,
                crate::effects::dimension::DIMENSION_PARAM_MOD_DEPTH_DEPTH_2,
                crate::effects::dimension::DIMENSION_PARAM_MOD_DEPTH_DEPTH_3,
                crate::effects::dimension::DIMENSION_PARAM_MOD_DEPTH_DEPTH_4,
            ],
            -2.0,
            2.0,
        );
        append_depth_targets(
            mix_idx,
            "mix",
            [
                crate::effects::dimension::DIMENSION_PARAM_MOD_MIX_DEPTH_1,
                crate::effects::dimension::DIMENSION_PARAM_MOD_MIX_DEPTH_2,
                crate::effects::dimension::DIMENSION_PARAM_MOD_MIX_DEPTH_3,
                crate::effects::dimension::DIMENSION_PARAM_MOD_MIX_DEPTH_4,
            ],
            -1.5,
            1.5,
        );

        desc
    }

    pub fn builtin_phaser_flanger() -> Self {
        let mut desc = Self {
            name: "Phaser-Flanger".to_string(),
            input_channels: 2 + crate::instruments::voice_modulator::NUM_OUTPUTS,
            output_channels: 2,
            instrument_modulators: (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::instruments::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                Self::enabled_param(
                    crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_ENABLED as u32,
                    1.0,
                ),
                ParamDescriptor {
                    name: "mode".to_string(),
                    min: 0.0,
                    max: 2.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "phaser".to_string(),
                            "flanger".to_string(),
                            "doubler".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_MODE
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "notches".to_string(),
                    min: 1.0,
                    max: 12.0,
                    default: 4.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_NOTCHES
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "center".to_string(),
                    min: 20.0,
                    max: 18000.0,
                    default: 400.0,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_CENTER
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "spread".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.35,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_SPREAD
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "blend".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_BLEND
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "flanger time".to_string(),
                    min: 0.1,
                    max: 20.0,
                    default: 2.5,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx:
                        crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_FLANGER_TIME as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "doubler time".to_string(),
                    min: 2.0,
                    max: 100.0,
                    default: 80.0,
                    kind: ParamKind::Continuous {
                        unit: Some("ms".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx:
                        crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_DOUBLER_TIME as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "sync".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_SYNC
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "rate".to_string(),
                    min: 0.01,
                    max: 20.0,
                    default: 0.15,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_RATE
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "sync div".to_string(),
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
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_SYNC_DIV
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "lfo shape".to_string(),
                    min: 0.0,
                    max: 3.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "sine".to_string(),
                            "triangle".to_string(),
                            "ramp".to_string(),
                            "square".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_SHAPE
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "amount".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.25,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_AMOUNT
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "feedback".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_FEEDBACK
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "fb invert".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_FB_INVERT
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "stereo".to_string(),
                    min: 0.0,
                    max: 180.0,
                    default: 20.0,
                    kind: ParamKind::Continuous {
                        unit: Some("°".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_STEREO
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "warmth".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_WARMTH
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "dry/wet".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous {
                        unit: Some("%".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_MIX as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "output".to_string(),
                    min: -12.0,
                    max: 12.0,
                    default: 0.0,
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_OUTPUT
                        as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "phaser circuit".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Enum {
                        labels: vec!["stack".to_string(), "classic".to_string()],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx:
                        crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_PHASER_CIRCUIT as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        desc.params
            .extend(crate::instruments::voice_modulator::effect_param_descriptors());

        let amount_idx = desc
            .params
            .iter()
            .position(|param| param.name == "amount")
            .expect("built-in Phaser-Flanger amount param should exist");
        let center_idx = desc
            .params
            .iter()
            .position(|param| param.name == "center")
            .expect("built-in Phaser-Flanger center param should exist");
        let feedback_idx = desc
            .params
            .iter()
            .position(|param| param.name == "feedback")
            .expect("built-in Phaser-Flanger feedback param should exist");
        let mix_idx = desc
            .params
            .iter()
            .position(|param| param.name == "dry/wet")
            .expect("built-in Phaser-Flanger dry/wet param should exist");

        let mut append_depth_targets =
            |base_param_idx: usize,
             destination_name: &str,
             first_depth_param: u64,
             depth_min: f32,
             depth_max: f32,
             depth_unit: Option<&str>| {
                for slot in 0..crate::instruments::voice_modulator::SLOT_COUNT {
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
                        node_param_idx: first_depth_param as u32 + slot as u32,
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
            amount_idx,
            "amount",
            crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_MOD_AMOUNT_DEPTH_1,
            -1.0,
            1.0,
            None,
        );
        // Center modulation is applied in log2(Hz): depth is in octaves.
        append_depth_targets(
            center_idx,
            "center",
            crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_MOD_CENTER_DEPTH_1,
            -4.0,
            4.0,
            Some("oct"),
        );
        append_depth_targets(
            feedback_idx,
            "feedback",
            crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_MOD_FEEDBACK_DEPTH_1,
            -1.0,
            1.0,
            None,
        );
        append_depth_targets(
            mix_idx,
            "mix",
            crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_MOD_MIX_DEPTH_1,
            -1.0,
            1.0,
            None,
        );

        desc
    }

    /// Built-in Roar multi-stage saturation descriptor (docs/roar-spec.md).
    pub fn builtin_roar() -> Self {
        fn continuous(
            name: &str,
            min: f32,
            max: f32,
            default: f32,
            unit: Option<&str>,
            scaling: ParamScaling,
            node_param_idx: u64,
        ) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min,
                max,
                default,
                kind: ParamKind::Continuous {
                    unit: unit.map(str::to_string),
                },
                scaling,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }
        fn options(
            name: &str,
            labels: &[&str],
            default: f32,
            node_param_idx: u64,
        ) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min: 0.0,
                max: (labels.len() - 1) as f32,
                default,
                kind: ParamKind::Enum {
                    labels: labels.iter().map(|label| label.to_string()).collect(),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }
        fn boolean(name: &str, default: f32, node_param_idx: u64) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min: 0.0,
                max: 1.0,
                default,
                kind: ParamKind::Boolean,
                scaling: ParamScaling::Linear,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        const SHAPERS: [&str; 12] = [
            "soft sine",
            "digital clip",
            "bit crusher",
            "diode clipper",
            "tube preamp",
            "half wave",
            "full wave",
            "polynomial",
            "fractal",
            "tri fold",
            "noise",
            "shards",
        ];
        const FILTERS: [&str; 9] = [
            "lp",
            "bp",
            "hp",
            "notch",
            "peak",
            "morph",
            "comb",
            "resample",
            "dispersion",
        ];

        let mut params = vec![
            Self::enabled_param(crate::effects::roar::ROAR_PARAM_ENABLED as u32, 1.0),
            continuous(
                "drive",
                -12.0,
                36.0,
                0.0,
                Some("dB"),
                ParamScaling::Linear,
                crate::effects::roar::ROAR_PARAM_DRIVE,
            ),
            continuous(
                "tone",
                -1.0,
                1.0,
                0.0,
                Some("%"),
                ParamScaling::Linear,
                crate::effects::roar::ROAR_PARAM_TONE,
            ),
            continuous(
                "tone freq",
                50.0,
                18_000.0,
                180.0,
                Some("Hz"),
                ParamScaling::Exponential,
                crate::effects::roar::ROAR_PARAM_TONE_FREQ,
            ),
            options(
                "tone mode",
                &["tilt", "shelf"],
                0.0,
                crate::effects::roar::ROAR_PARAM_TONE_MODE,
            ),
            options(
                "routing",
                &[
                    "single",
                    "serial",
                    "parallel",
                    "multi band",
                    "mid side",
                    "feedback",
                    "delay",
                ],
                0.0,
                crate::effects::roar::ROAR_PARAM_ROUTING,
            ),
            continuous(
                "blend",
                0.0,
                1.0,
                0.5,
                Some("%"),
                ParamScaling::Linear,
                crate::effects::roar::ROAR_PARAM_BLEND,
            ),
            continuous(
                "xover low",
                40.0,
                1_000.0,
                200.0,
                Some("Hz"),
                ParamScaling::Exponential,
                crate::effects::roar::ROAR_PARAM_XOVER_LOW,
            ),
            continuous(
                "xover high",
                500.0,
                10_000.0,
                2_000.0,
                Some("Hz"),
                ParamScaling::Exponential,
                crate::effects::roar::ROAR_PARAM_XOVER_HIGH,
            ),
            options(
                "fb mode",
                &["time", "note"],
                0.0,
                crate::effects::roar::ROAR_PARAM_FB_MODE,
            ),
            continuous(
                "fb time",
                0.5,
                1_000.0,
                18.2,
                Some("ms"),
                ParamScaling::Exponential,
                crate::effects::roar::ROAR_PARAM_FB_TIME,
            ),
            options(
                "fb div",
                &[
                    "1/32", "1/16", "1/16t", "1/8", "1/8t", "1/8.", "1/4", "1/4t", "1/4.", "1/2",
                    "1",
                ],
                3.0,
                crate::effects::roar::ROAR_PARAM_FB_DIV,
            ),
            continuous(
                "fb amount",
                0.0,
                1.0,
                0.0,
                Some("%"),
                ParamScaling::Linear,
                crate::effects::roar::ROAR_PARAM_FB_AMOUNT,
            ),
            boolean("fb invert", 0.0, crate::effects::roar::ROAR_PARAM_FB_INVERT),
            boolean("fb duck", 0.0, crate::effects::roar::ROAR_PARAM_FB_DUCK),
            continuous(
                "fb freq",
                30.0,
                18_000.0,
                1_000.0,
                Some("Hz"),
                ParamScaling::Exponential,
                crate::effects::roar::ROAR_PARAM_FB_FREQ,
            ),
            continuous(
                "fb width",
                0.5,
                9.0,
                8.0,
                Some("oct"),
                ParamScaling::Linear,
                crate::effects::roar::ROAR_PARAM_FB_WIDTH,
            ),
            continuous(
                "compress",
                0.0,
                1.0,
                0.0,
                Some("%"),
                ParamScaling::Linear,
                crate::effects::roar::ROAR_PARAM_COMPRESS,
            ),
            boolean("sc hpf", 0.0, crate::effects::roar::ROAR_PARAM_SC_HPF),
            continuous(
                "output",
                -24.0,
                24.0,
                0.0,
                Some("dB"),
                ParamScaling::Linear,
                crate::effects::roar::ROAR_PARAM_OUTPUT,
            ),
            continuous(
                "dry/wet",
                0.0,
                1.0,
                1.0,
                Some("%"),
                ParamScaling::Linear,
                crate::effects::roar::ROAR_PARAM_MIX,
            ),
        ];
        for stage in 0..crate::effects::roar::NUM_STAGES {
            let prefix = format!("s{}", stage + 1);
            params.push(options(
                &format!("{prefix} shaper"),
                &SHAPERS,
                0.0,
                crate::effects::roar::roar_stage_param(
                    stage,
                    crate::effects::roar::RoarStageField::Shaper,
                ),
            ));
            params.push(continuous(
                &format!("{prefix} amount"),
                0.0,
                1.0,
                0.0,
                Some("%"),
                ParamScaling::Linear,
                crate::effects::roar::roar_stage_param(
                    stage,
                    crate::effects::roar::RoarStageField::Amount,
                ),
            ));
            params.push(continuous(
                &format!("{prefix} bias"),
                -1.0,
                1.0,
                0.0,
                None,
                ParamScaling::Linear,
                crate::effects::roar::roar_stage_param(
                    stage,
                    crate::effects::roar::RoarStageField::Bias,
                ),
            ));
            params.push(continuous(
                &format!("{prefix} level"),
                -24.0,
                24.0,
                0.0,
                Some("dB"),
                ParamScaling::Linear,
                crate::effects::roar::roar_stage_param(
                    stage,
                    crate::effects::roar::RoarStageField::Level,
                ),
            ));
            params.push(options(
                &format!("{prefix} filter"),
                &FILTERS,
                0.0,
                crate::effects::roar::roar_stage_param(
                    stage,
                    crate::effects::roar::RoarStageField::Filter,
                ),
            ));
            params.push(continuous(
                &format!("{prefix} freq"),
                20.0,
                16_000.0,
                16_000.0,
                Some("Hz"),
                ParamScaling::Exponential,
                crate::effects::roar::roar_stage_param(
                    stage,
                    crate::effects::roar::RoarStageField::Freq,
                ),
            ));
            params.push(continuous(
                &format!("{prefix} res"),
                0.0,
                1.0,
                0.1,
                None,
                ParamScaling::Linear,
                crate::effects::roar::roar_stage_param(
                    stage,
                    crate::effects::roar::RoarStageField::Res,
                ),
            ));
            params.push(boolean(
                &format!("{prefix} pre"),
                0.0,
                crate::effects::roar::roar_stage_param(
                    stage,
                    crate::effects::roar::RoarStageField::Pre,
                ),
            ));
        }

        let mut desc = Self {
            name: "Roar".to_string(),
            input_channels: 2 + crate::instruments::voice_modulator::NUM_OUTPUTS,
            output_channels: 2,
            instrument_modulators: (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::instruments::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params,
        };
        desc.params
            .extend(crate::instruments::voice_modulator::effect_param_descriptors());

        let param_position = |desc: &Self, name: &str| {
            desc.params
                .iter()
                .position(|param| param.name == name)
                .unwrap_or_else(|| panic!("built-in Roar {name} param should exist"))
        };
        let drive_idx = param_position(&desc, "drive");
        let tone_idx = param_position(&desc, "tone");
        let fb_amount_idx = param_position(&desc, "fb amount");
        let mix_idx = param_position(&desc, "dry/wet");

        let mut append_depth_targets =
            |base_param_idx: usize,
             destination_name: &str,
             first_depth_param: u64,
             depth_min: f32,
             depth_max: f32,
             depth_unit: Option<&str>| {
                for slot in 0..crate::instruments::voice_modulator::SLOT_COUNT {
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
                        node_param_idx: first_depth_param as u32 + slot as u32,
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

        // Drive modulation is applied additively in dB.
        append_depth_targets(
            drive_idx,
            "drive",
            crate::effects::roar::ROAR_PARAM_MOD_DRIVE_DEPTH_1,
            -24.0,
            24.0,
            Some("dB"),
        );
        append_depth_targets(
            tone_idx,
            "tone",
            crate::effects::roar::ROAR_PARAM_MOD_TONE_DEPTH_1,
            -1.0,
            1.0,
            None,
        );
        append_depth_targets(
            fb_amount_idx,
            "fb amount",
            crate::effects::roar::ROAR_PARAM_MOD_FB_AMOUNT_DEPTH_1,
            -1.0,
            1.0,
            None,
        );
        append_depth_targets(
            mix_idx,
            "mix",
            crate::effects::roar::ROAR_PARAM_MOD_MIX_DEPTH_1,
            -1.0,
            1.0,
            None,
        );

        desc
    }

    pub fn builtin_dj_mixer() -> Self {
        let mut desc = Self {
            name: "DJ Mixer".to_string(),
            input_channels: 2 + crate::instruments::voice_modulator::NUM_OUTPUTS,
            output_channels: 2,
            instrument_modulators: (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::instruments::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                Self::enabled_param(crate::effects::dj_mixer::DJ_MIXER_PARAM_ENABLED as u32, 1.0),
                ParamDescriptor {
                    name: "speed".to_string(),
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dj_mixer::DJ_MIXER_PARAM_SPEED as u32,
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
                    node_param_idx: crate::effects::dj_mixer::DJ_MIXER_PARAM_LENGTH_SEC as u32,
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
                    node_param_idx: crate::effects::dj_mixer::DJ_MIXER_PARAM_LOOP as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "sync".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dj_mixer::DJ_MIXER_PARAM_SYNC as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "div".to_string(),
                    min: 0.0,
                    max: 5.0,
                    default: 4.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "1/16".to_string(),
                            "1/8".to_string(),
                            "1/4".to_string(),
                            "1/2".to_string(),
                            "1 bar".to_string(),
                            "2 bars".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dj_mixer::DJ_MIXER_PARAM_DIV as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "warp".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dj_mixer::DJ_MIXER_PARAM_WARP as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        desc.params
            .extend(crate::instruments::voice_modulator::effect_param_descriptors());

        let mut append_depth_targets =
            |target_name: &str,
             depth_params: [u64; 4],
             depth_min: f32,
             depth_max: f32,
             depth_unit: Option<&str>| {
                let base_param_idx = desc
                    .params
                    .iter()
                    .position(|param| param.name == target_name)
                    .expect("dj mixer modulation target param should exist");
                for (slot, node_param_idx) in depth_params.into_iter().enumerate() {
                    let depth_param_idx = desc.params.len();
                    desc.params.push(ParamDescriptor {
                        name: format!("mod {} slot {} amt", target_name, slot + 1),
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
            "enabled",
            [
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_ENABLED_DEPTH_1,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_ENABLED_DEPTH_2,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_ENABLED_DEPTH_3,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_ENABLED_DEPTH_4,
            ],
            -1.0,
            1.0,
            None,
        );
        append_depth_targets(
            "speed",
            [
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_SPEED_DEPTH_1,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_SPEED_DEPTH_2,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_SPEED_DEPTH_3,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_SPEED_DEPTH_4,
            ],
            -2.0,
            2.0,
            None,
        );
        append_depth_targets(
            "length",
            [
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_LENGTH_DEPTH_1,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_LENGTH_DEPTH_2,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_LENGTH_DEPTH_3,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_LENGTH_DEPTH_4,
            ],
            -3.0,
            3.0,
            Some("oct"),
        );
        append_depth_targets(
            "loop",
            [
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_LOOP_DEPTH_1,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_LOOP_DEPTH_2,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_LOOP_DEPTH_3,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_LOOP_DEPTH_4,
            ],
            -1.0,
            1.0,
            None,
        );
        append_depth_targets(
            "warp",
            [
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_WARP_DEPTH_1,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_WARP_DEPTH_2,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_WARP_DEPTH_3,
                crate::effects::dj_mixer::DJ_MIXER_PARAM_MOD_WARP_DEPTH_4,
            ],
            -1.0,
            1.0,
            None,
        );
        desc
    }

    /// Built-in reverb as a stereo insert effect.
    pub fn builtin_reverb_insert() -> Self {
        Self {
            name: "Reverb".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
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
                    node_param_idx: crate::effects::reverb::REVERB_PARAM_SIZE as u32,
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
                    node_param_idx: crate::effects::reverb::REVERB_PARAM_BRIGHT as u32,
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
                    node_param_idx: crate::effects::reverb::REVERB_PARAM_REPLACE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                Self::enabled_param(crate::effects::reverb::REVERB_PARAM_ENABLED as u32, 1.0),
            ],
        }
    }

    /// Multi-mode vintage reverb (Plate / Hall / Quad / Mod) as a stereo
    /// insert. Spec: docs/reverb-modes-spec.md. Param order is append-only —
    /// plocks and mod routings persist by descriptor index.
    pub fn builtin_multiverb() -> Self {
        fn knob(
            name: &str,
            min: f32,
            max: f32,
            default: f32,
            unit: Option<&str>,
            node_param_idx: u64,
        ) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min,
                max,
                default,
                kind: ParamKind::Continuous {
                    unit: unit.map(str::to_string),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        let mut desc = Self {
            name: "Multiverb".to_string(),
            input_channels: 2 + crate::instruments::voice_modulator::NUM_OUTPUTS,
            output_channels: 2,
            instrument_modulators: (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::instruments::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "mode".to_string(),
                    min: 0.0,
                    max: 3.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "plate".to_string(),
                            "hall".to_string(),
                            "quad".to_string(),
                            "mod".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::multiverb::MULTIVERB_PARAM_MODE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                knob(
                    "decay",
                    0.0,
                    1.0,
                    0.55,
                    None,
                    crate::effects::multiverb::MULTIVERB_PARAM_DECAY,
                ),
                knob(
                    "size",
                    0.0,
                    1.0,
                    0.5,
                    None,
                    crate::effects::multiverb::MULTIVERB_PARAM_SIZE,
                ),
                knob(
                    "predelay",
                    0.0,
                    250.0,
                    0.0,
                    Some("ms"),
                    crate::effects::multiverb::MULTIVERB_PARAM_PREDELAY_MS,
                ),
                knob(
                    "damp",
                    0.0,
                    1.0,
                    0.35,
                    None,
                    crate::effects::multiverb::MULTIVERB_PARAM_DAMP,
                ),
                knob(
                    "bass",
                    0.0,
                    1.0,
                    0.5,
                    None,
                    crate::effects::multiverb::MULTIVERB_PARAM_BASS,
                ),
                knob(
                    "diffusion",
                    0.0,
                    1.0,
                    0.7,
                    None,
                    crate::effects::multiverb::MULTIVERB_PARAM_DIFFUSION,
                ),
                knob(
                    "mod rate",
                    0.05,
                    8.0,
                    0.7,
                    Some("Hz"),
                    crate::effects::multiverb::MULTIVERB_PARAM_MOD_RATE,
                ),
                knob(
                    "mod depth",
                    0.0,
                    1.0,
                    0.15,
                    None,
                    crate::effects::multiverb::MULTIVERB_PARAM_MOD_DEPTH,
                ),
                knob(
                    "mod shape",
                    0.0,
                    1.0,
                    0.0,
                    None,
                    crate::effects::multiverb::MULTIVERB_PARAM_MOD_SHAPE,
                ),
                knob(
                    "era",
                    0.0,
                    1.0,
                    0.15,
                    None,
                    crate::effects::multiverb::MULTIVERB_PARAM_ERA,
                ),
                knob(
                    "width",
                    0.0,
                    1.0,
                    1.0,
                    None,
                    crate::effects::multiverb::MULTIVERB_PARAM_WIDTH,
                ),
                knob(
                    "mix",
                    0.0,
                    1.0,
                    0.35,
                    Some("%"),
                    crate::effects::multiverb::MULTIVERB_PARAM_MIX,
                ),
                Self::enabled_param(
                    crate::effects::multiverb::MULTIVERB_PARAM_ENABLED as u32,
                    1.0,
                ),
            ],
        };
        desc.params
            .extend(crate::instruments::voice_modulator::effect_param_descriptors());

        let decay_idx = desc
            .params
            .iter()
            .position(|param| param.name == "decay")
            .expect("built-in Multiverb decay param should exist");
        let size_idx = desc
            .params
            .iter()
            .position(|param| param.name == "size")
            .expect("built-in Multiverb size param should exist");
        let mod_depth_idx = desc
            .params
            .iter()
            .position(|param| param.name == "mod depth")
            .expect("built-in Multiverb mod depth param should exist");
        let mix_idx = desc
            .params
            .iter()
            .position(|param| param.name == "mix")
            .expect("built-in Multiverb mix param should exist");

        let mut append_depth_targets =
            |base_param_idx: usize,
             destination_name: &str,
             depth_params: [u64; crate::instruments::voice_modulator::SLOT_COUNT]| {
                for (slot, node_param_idx) in depth_params.into_iter().enumerate() {
                    let depth_param_idx = desc.params.len();
                    desc.params.push(ParamDescriptor {
                        name: format!("mod {destination_name} slot {} amt", slot + 1),
                        min: -1.0,
                        max: 1.0,
                        default: 0.0,
                        kind: ParamKind::Continuous { unit: None },
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
                            depth_min: -1.0,
                            depth_max: 1.0,
                            depth_unit: None,
                        });
                }
            };

        append_depth_targets(
            decay_idx,
            "decay",
            [
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_DECAY_DEPTH_1,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_DECAY_DEPTH_2,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_DECAY_DEPTH_3,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_DECAY_DEPTH_4,
            ],
        );
        append_depth_targets(
            size_idx,
            "size",
            [
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_SIZE_DEPTH_1,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_SIZE_DEPTH_2,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_SIZE_DEPTH_3,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_SIZE_DEPTH_4,
            ],
        );
        append_depth_targets(
            mod_depth_idx,
            "depth",
            [
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_DEPTH_DEPTH_1,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_DEPTH_DEPTH_2,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_DEPTH_DEPTH_3,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_DEPTH_DEPTH_4,
            ],
        );
        append_depth_targets(
            mix_idx,
            "mix",
            [
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_MIX_DEPTH_1,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_MIX_DEPTH_2,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_MIX_DEPTH_3,
                crate::effects::multiverb::MULTIVERB_PARAM_MOD_MIX_DEPTH_4,
            ],
        );

        desc
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
            tensor_params: Vec::new(),
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
                    node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_MODE as u32,
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
                    node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_AMOUNT as u32,
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
                    node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_ATTACK as u32,
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
                    node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_RELEASE as u32,
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
                        node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_LOW_CUT_HZ as u32,
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
                        node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_LOW_CUT_HZ as u32,
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
                    node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_DRIVE as u32,
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
                    node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_INPUT_DB as u32,
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
                    node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_OUTPUT_DB as u32,
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
                    node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_MIX as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                Self::enabled_param(crate::effects::dynamics::DYNAMICS_PARAM_ENABLED as u32, 1.0),
                ParamDescriptor {
                    name: "knee".to_string(),
                    min: 0.0,
                    max: 18.0,
                    default: if mode == 0.0 { 8.0 } else { 6.0 },
                    kind: ParamKind::Continuous {
                        unit: Some("dB".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::effects::dynamics::DYNAMICS_PARAM_KNEE_DB as u32,
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

    /// Ableton-style bread-and-butter compressor with external sidechain.
    pub fn builtin_compressor() -> Self {
        use crate::effects::compressor as comp;

        fn continuous(
            name: &str,
            min: f32,
            max: f32,
            default: f32,
            unit: Option<&str>,
            scaling: ParamScaling,
            node_param_idx: u64,
        ) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min,
                max,
                default,
                kind: ParamKind::Continuous {
                    unit: unit.map(|unit| unit.to_string()),
                },
                scaling,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        fn toggle(name: &str, default: f32, node_param_idx: u64) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min: 0.0,
                max: 1.0,
                default,
                kind: ParamKind::Boolean,
                scaling: ParamScaling::Linear,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        fn options(
            name: &str,
            labels: &[&str],
            default: f32,
            node_param_idx: u64,
        ) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min: 0.0,
                max: (labels.len() - 1) as f32,
                default,
                kind: ParamKind::Enum {
                    labels: labels.iter().map(|label| label.to_string()).collect(),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        Self {
            name: "Compressor".to_string(),
            // Inputs 0/1 carry the track signal; input 2 is the external
            // sidechain the host routes via the `sidechain` param below.
            input_channels: 3,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                continuous(
                    "threshold",
                    -70.0,
                    6.0,
                    -18.0,
                    Some("dB"),
                    ParamScaling::Linear,
                    comp::COMPRESSOR_PARAM_THRESHOLD_DB,
                ),
                continuous(
                    "ratio",
                    1.0,
                    40.0,
                    4.0,
                    None,
                    ParamScaling::Exponential,
                    comp::COMPRESSOR_PARAM_RATIO,
                ),
                continuous(
                    "attack",
                    0.01,
                    1000.0,
                    1.0,
                    Some("ms"),
                    ParamScaling::Exponential,
                    comp::COMPRESSOR_PARAM_ATTACK_MS,
                ),
                continuous(
                    "release",
                    1.0,
                    3000.0,
                    30.0,
                    Some("ms"),
                    ParamScaling::Exponential,
                    comp::COMPRESSOR_PARAM_RELEASE_MS,
                ),
                toggle("auto release", 0.0, comp::COMPRESSOR_PARAM_AUTO_RELEASE),
                options(
                    "model",
                    &["peak", "rms", "expand"],
                    comp::MODEL_RMS,
                    comp::COMPRESSOR_PARAM_MODEL,
                ),
                continuous(
                    "knee",
                    0.0,
                    18.0,
                    6.0,
                    Some("dB"),
                    ParamScaling::Linear,
                    comp::COMPRESSOR_PARAM_KNEE_DB,
                ),
                options(
                    "lookahead",
                    &["0 ms", "1 ms", "10 ms"],
                    0.0,
                    comp::COMPRESSOR_PARAM_LOOKAHEAD,
                ),
                options("env", &["lin", "log"], 1.0, comp::COMPRESSOR_PARAM_ENV_MODE),
                continuous(
                    "out",
                    -36.0,
                    36.0,
                    0.0,
                    Some("dB"),
                    ParamScaling::Linear,
                    comp::COMPRESSOR_PARAM_OUT_DB,
                ),
                toggle("makeup", 0.0, comp::COMPRESSOR_PARAM_AUTO_MAKEUP),
                continuous(
                    "dry/wet",
                    0.0,
                    1.0,
                    1.0,
                    Some("%"),
                    ParamScaling::Linear,
                    comp::COMPRESSOR_PARAM_DRY_WET,
                ),
                toggle("sc on", 0.0, comp::COMPRESSOR_PARAM_SC_ON),
                continuous(
                    "sc gain",
                    -24.0,
                    24.0,
                    0.0,
                    Some("dB"),
                    ParamScaling::Linear,
                    comp::COMPRESSOR_PARAM_SC_GAIN_DB,
                ),
                toggle("sc filter", 0.0, comp::COMPRESSOR_PARAM_SC_FILTER_ON),
                options(
                    "sc type",
                    &["lowpass", "highpass", "bandpass", "notch"],
                    0.0,
                    comp::COMPRESSOR_PARAM_SC_FILTER_TYPE,
                ),
                continuous(
                    "sc freq",
                    30.0,
                    15000.0,
                    80.0,
                    Some("Hz"),
                    ParamScaling::Exponential,
                    comp::COMPRESSOR_PARAM_SC_FREQ,
                ),
                continuous(
                    "sc res",
                    0.1,
                    8.0,
                    0.71,
                    None,
                    ParamScaling::Exponential,
                    comp::COMPRESSOR_PARAM_SC_Q,
                ),
                toggle("sc listen", 0.0, comp::COMPRESSOR_PARAM_SC_LISTEN),
                // Host-routed sidechain source. Labels are patched with the
                // track list wherever the descriptor is instantiated.
                ParamDescriptor {
                    name: "sidechain".to_string(),
                    min: 0.0,
                    max: 0.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec!["off".to_string()],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: u32::MAX,
                    node_param_span: 1,
                    host_control: Some(HostControl::FxSidechain {
                        input_channel: comp::SIDECHAIN_INPUT_CHANNEL,
                    }),
                    ui_metadata: None,
                },
                Self::enabled_param(comp::COMPRESSOR_PARAM_ENABLED as u32, 1.0),
            ],
        }
    }

    /// OTT-style 3-band upward+downward compressor.
    pub fn builtin_ott() -> Self {
        use crate::effects::ott::{self, OttBandField};

        fn continuous(
            name: &str,
            min: f32,
            max: f32,
            default: f32,
            unit: Option<&str>,
            scaling: ParamScaling,
            node_param_idx: u64,
        ) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min,
                max,
                default,
                kind: ParamKind::Continuous {
                    unit: unit.map(str::to_string),
                },
                scaling,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        fn boolean(name: &str, default: f32, node_param_idx: u64) -> ParamDescriptor {
            ParamDescriptor {
                name: name.to_string(),
                min: 0.0,
                max: 1.0,
                default,
                kind: ParamKind::Boolean,
                scaling: ParamScaling::Linear,
                node_param_idx: node_param_idx as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }
        }

        let mut params = Vec::new();
        for (band, prefix) in ["low", "mid", "high"].into_iter().enumerate() {
            let idx = |field| ott::ott_band_param(band, field);
            params.extend([
                continuous(
                    &format!("{prefix} below thr"),
                    -80.0,
                    0.0,
                    ott::DEFAULT_BELOW_THR_DB,
                    Some("dB"),
                    ParamScaling::Linear,
                    idx(OttBandField::BelowThreshold),
                ),
                continuous(
                    &format!("{prefix} below ratio"),
                    0.1,
                    100.0,
                    1.0,
                    None,
                    ParamScaling::Exponential,
                    idx(OttBandField::BelowRatio),
                ),
                continuous(
                    &format!("{prefix} above thr"),
                    -80.0,
                    0.0,
                    ott::DEFAULT_ABOVE_THR_DB,
                    Some("dB"),
                    ParamScaling::Linear,
                    idx(OttBandField::AboveThreshold),
                ),
                continuous(
                    &format!("{prefix} above ratio"),
                    0.1,
                    100.0,
                    1.0,
                    None,
                    ParamScaling::Exponential,
                    idx(OttBandField::AboveRatio),
                ),
                continuous(
                    &format!("{prefix} attack"),
                    0.1,
                    1000.0,
                    ott::BAND_DEFAULT_ATTACK_MS[band],
                    Some("ms"),
                    ParamScaling::Exponential,
                    idx(OttBandField::Attack),
                ),
                continuous(
                    &format!("{prefix} release"),
                    1.0,
                    3000.0,
                    ott::BAND_DEFAULT_RELEASE_MS[band],
                    Some("ms"),
                    ParamScaling::Exponential,
                    idx(OttBandField::Release),
                ),
                continuous(
                    &format!("{prefix} input"),
                    -24.0,
                    24.0,
                    0.0,
                    Some("dB"),
                    ParamScaling::Linear,
                    idx(OttBandField::Input),
                ),
                continuous(
                    &format!("{prefix} output"),
                    -24.0,
                    24.0,
                    0.0,
                    Some("dB"),
                    ParamScaling::Linear,
                    idx(OttBandField::Output),
                ),
                boolean(&format!("{prefix} on"), 1.0, idx(OttBandField::On)),
                boolean(&format!("{prefix} solo"), 0.0, idx(OttBandField::Solo)),
            ]);
        }
        params.extend([
            boolean("low split", 1.0, ott::OTT_PARAM_SPLIT_LOW),
            boolean("high split", 1.0, ott::OTT_PARAM_SPLIT_HIGH),
            continuous(
                "xover low",
                20.0,
                2000.0,
                120.0,
                Some("Hz"),
                ParamScaling::Exponential,
                ott::OTT_PARAM_XOVER_LOW_HZ,
            ),
            continuous(
                "xover high",
                200.0,
                18000.0,
                2500.0,
                Some("Hz"),
                ParamScaling::Exponential,
                ott::OTT_PARAM_XOVER_HIGH_HZ,
            ),
            boolean("soft knee", 1.0, ott::OTT_PARAM_SOFT_KNEE),
            boolean("rms", 0.0, ott::OTT_PARAM_RMS),
            continuous(
                "output",
                -24.0,
                24.0,
                0.0,
                Some("dB"),
                ParamScaling::Linear,
                ott::OTT_PARAM_OUTPUT_DB,
            ),
            continuous(
                "time",
                0.1,
                10.0,
                1.0,
                Some("%".into()),
                ParamScaling::Exponential,
                ott::OTT_PARAM_TIME,
            ),
            continuous(
                "amount",
                0.0,
                1.0,
                1.0,
                Some("%".into()),
                ParamScaling::Linear,
                ott::OTT_PARAM_AMOUNT,
            ),
            Self::enabled_param(ott::OTT_PARAM_ENABLED as u32, 1.0),
        ]);

        Self {
            name: "OTT".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params,
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
            tensor_params: Vec::new(),
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
                    node_param_idx: crate::effects::limiter::LIMITER_PARAM_INPUT_GAIN_DB as u32,
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
                    node_param_idx: crate::effects::limiter::LIMITER_PARAM_CEILING_DB as u32,
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
                    node_param_idx: crate::effects::limiter::LIMITER_PARAM_RELEASE_MS as u32,
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
                    node_param_idx: crate::effects::limiter::LIMITER_PARAM_LOOKAHEAD_MS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                Self::enabled_param(crate::effects::limiter::LIMITER_PARAM_ENABLED as u32, 1.0),
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
            tensor_params: Vec::new(),
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
                    node_param_idx: crate::effects::tape::TAPE_PARAM_DRIVE_DB as u32,
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
                    node_param_idx: crate::effects::tape::TAPE_PARAM_BIAS as u32,
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
                    node_param_idx: crate::effects::tape::TAPE_PARAM_SPEED as u32,
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
                    node_param_idx: crate::effects::tape::TAPE_PARAM_OUTPUT_DB as u32,
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
                    node_param_idx: crate::effects::tape::TAPE_PARAM_MIX as u32,
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
                    node_param_idx: crate::effects::tape::TAPE_PARAM_WOW as u32,
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
                    node_param_idx: crate::effects::tape::TAPE_PARAM_FLUTTER as u32,
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
                    node_param_idx: crate::effects::tape::TAPE_PARAM_HISS as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                Self::enabled_param(crate::effects::tape::TAPE_PARAM_ENABLED as u32, 1.0),
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
            Self::enabled_param(crate::instruments::sampler::SAMPLER_PARAM_ENABLED as u32, 1.0),
            ParamDescriptor {
                name: "reverse".to_string(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                kind: ParamKind::Boolean,
                scaling: ParamScaling::Linear,
                node_param_idx: crate::instruments::sampler::PARAM_REVERSE as u32,
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
                node_param_idx: crate::instruments::sampler::PARAM_LOOP_MODE as u32,
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
                node_param_idx: crate::instruments::sampler::PARAM_LOOP_XFADE_SAMPLES as u32,
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
                node_param_idx: crate::instruments::sampler::PARAM_SR_HZ as u32,
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
                node_param_idx: crate::instruments::sampler::PARAM_WARP_ENABLED as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
            ParamDescriptor {
                name: "mode".to_string(),
                min: 0.0,
                max: 3.0,
                default: 0.0,
                kind: ParamKind::Enum {
                    labels: vec![
                        "beats".to_string(),
                        "tones".to_string(),
                        "texture".to_string(),
                        "re-pitch".to_string(),
                    ],
                },
                scaling: ParamScaling::Linear,
                node_param_idx: crate::instruments::sampler::PARAM_WARP_MODE as u32,
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
                node_param_idx: crate::instruments::sampler::PARAM_WARP_SAMPLE_BPM as u32,
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
                node_param_idx: crate::instruments::sampler::PARAM_SPEED as u32,
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
                node_param_idx: crate::instruments::sampler::PARAM_SCRUB_OFFSET as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            },
        ];
        params.extend(crate::instruments::voice_modulator::ui_param_descriptors());
        let mod_source_labels: Vec<String> = std::iter::once("off".to_string())
            .chain(
                (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                    .map(|slot| crate::instruments::voice_modulator::modulator_slot_label(slot, "")),
            )
            .collect();
        let mut instrument_modulation_targets = Vec::new();
        for lane in crate::instruments::sampler::SAMPLER_MOD_TARGET_PARAMS {
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
                max: crate::instruments::voice_modulator::SLOT_COUNT as f32,
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
            node_param_idx: crate::instruments::sampler::PARAM_SCRUB_SMOOTH_TIME_MS as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        });
        // Beats-warp params. Appended at the tail (like "smooth") so plock
        // indices of older params stay stable in saved projects.
        params.push(ParamDescriptor {
            name: "preserve".to_string(),
            min: 0.0,
            max: 6.0,
            default: crate::instruments::warp_grid::PRESERVE_TRANSIENTS as f32,
            kind: ParamKind::Enum {
                labels: vec![
                    "1 bar".to_string(),
                    "1/2".to_string(),
                    "1/4".to_string(),
                    "1/8".to_string(),
                    "1/16".to_string(),
                    "1/32".to_string(),
                    "transients".to_string(),
                ],
            },
            scaling: ParamScaling::Linear,
            node_param_idx: crate::instruments::sampler::PARAM_WARP_PRESERVE as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        });
        params.push(ParamDescriptor {
            name: "fill".to_string(),
            min: 0.0,
            max: 2.0,
            default: crate::instruments::sampler::SEG_LOOP_FORWARD as f32,
            kind: ParamKind::Enum {
                labels: vec![
                    "off".to_string(),
                    "loop".to_string(),
                    "ping-pong".to_string(),
                ],
            },
            scaling: ParamScaling::Linear,
            node_param_idx: crate::instruments::sampler::PARAM_WARP_SEG_LOOP_MODE as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        });
        params.push(ParamDescriptor {
            name: "decay".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            kind: ParamKind::Continuous {
                unit: Some("%".to_string()),
            },
            scaling: ParamScaling::Linear,
            node_param_idx: crate::instruments::sampler::PARAM_WARP_SEG_ENVELOPE as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        });
        Self {
            name: "Sampler".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: (1..=crate::instruments::voice_modulator::SLOT_COUNT)
                .map(|slot| InstrumentModulatorDescriptor {
                    slot,
                    label: crate::instruments::voice_modulator::modulator_slot_label(slot, ""),
                })
                .collect(),
            instrument_modulation_targets,
            tensor_params: Vec::new(),
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
            tensor_params: Vec::new(),
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
            tensor_params: Vec::new(),
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
/// Cell storage for one slot's plocks, allocated on the first write.
/// A slot with no plocks (the overwhelmingly common case) never pays the
/// MAX_STEPS * max_params * 3-array allocation.
struct PLockCells {
    data: Vec<AtomicU32>,
    id_logical_ids: Vec<AtomicU64>,
    id_node_param_indices: Vec<AtomicU32>,
}

pub struct SlotPLockData {
    cells: OnceLock<PLockCells>,
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
        Self {
            cells: OnceLock::new(),
            max_params,
            plock_count: AtomicU32::new(0),
            step_counts: (0..MAX_STEPS).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    /// Cell arrays, if any write has materialized them. Absent arrays read as
    /// all-empty (every value NaN, every param id cleared).
    fn cells(&self) -> Option<&PLockCells> {
        self.cells.get()
    }

    fn cells_or_alloc(&self) -> &PLockCells {
        self.cells.get_or_init(|| {
            let size = MAX_STEPS * self.max_params;
            PLockCells {
                data: (0..size).map(|_| AtomicU32::new(NAN_BITS)).collect(),
                id_logical_ids: (0..size).map(|_| AtomicU64::new(0)).collect(),
                id_node_param_indices: (0..size).map(|_| AtomicU32::new(u32::MAX)).collect(),
            }
        })
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
        if let Some(cells) = self.cells() {
            for idx in 0..cells.data.len() {
                cells.data[idx].store(NAN_BITS, Ordering::Relaxed);
                cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
                cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
            }
        }
        self.plock_count.store(0, Ordering::Relaxed);
        for count in &self.step_counts {
            count.store(0, Ordering::Relaxed);
        }
    }

    pub fn get(&self, step: usize, param_idx: usize) -> Option<f32> {
        let cells = self.cells()?;
        let idx = self.index(step, param_idx);
        if idx >= cells.data.len() {
            return None;
        }
        let bits = cells.data[idx].load(Ordering::Relaxed);
        let val = f32::from_bits(bits);
        if val.is_nan() {
            None
        } else {
            Some(val)
        }
    }

    pub fn set(&self, step: usize, param_idx: usize, val: f32) {
        if self.cells().is_none() && val.is_nan() {
            // Writing "no override" into a slot with no cells is a no-op;
            // don't materialize the arrays for it.
            return;
        }
        let cells = self.cells_or_alloc();
        let idx = self.index(step, param_idx);
        if idx < cells.data.len() {
            let old_bits = cells.data[idx].swap(val.to_bits(), Ordering::Relaxed);
            self.note_cell_transition(step, old_bits, val.to_bits());
            cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
            cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
        }
    }

    pub fn set_with_id(&self, step: usize, param_idx: usize, val: f32, param_id: ParamNodeId) {
        let cells = self.cells_or_alloc();
        let idx = self.index(step, param_idx);
        if idx < cells.data.len() {
            let old_bits = cells.data[idx].swap(val.to_bits(), Ordering::Relaxed);
            self.note_cell_transition(step, old_bits, val.to_bits());
            cells.id_logical_ids[idx].store(param_id.logical_id, Ordering::Relaxed);
            cells.id_node_param_indices[idx].store(param_id.node_param_idx, Ordering::Relaxed);
        }
    }

    pub fn get_id(&self, step: usize, param_idx: usize) -> Option<ParamNodeId> {
        let cells = self.cells()?;
        let idx = self.index(step, param_idx);
        if idx >= cells.data.len() {
            return None;
        }
        let logical_id = cells.id_logical_ids[idx].load(Ordering::Relaxed);
        let node_param_idx = cells.id_node_param_indices[idx].load(Ordering::Relaxed);
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
        let Some(cells) = self.cells() else {
            return;
        };
        for p in 0..self.max_params {
            let idx = self.index(step, p);
            if idx < cells.data.len() {
                let old_bits = cells.data[idx].swap(NAN_BITS, Ordering::Relaxed);
                self.note_cell_transition(step, old_bits, NAN_BITS);
                cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
                cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
            }
        }
    }

    pub fn clear_param(&self, step: usize, param_idx: usize) {
        let Some(cells) = self.cells() else {
            return;
        };
        let idx = self.index(step, param_idx);
        if idx < cells.data.len() {
            let old_bits = cells.data[idx].swap(NAN_BITS, Ordering::Relaxed);
            self.note_cell_transition(step, old_bits, NAN_BITS);
            cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
            cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
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
        let Some(cells) = self.cells() else {
            return (Vec::new(), Vec::new());
        };
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
                let val = f32::from_bits(cells.data[idx].load(Ordering::Relaxed));
                if !val.is_nan() {
                    row[p] = Some(val);
                }
                let logical_id = cells.id_logical_ids[idx].load(Ordering::Relaxed);
                if logical_id != 0 {
                    let node_param_idx = cells.id_node_param_indices[idx].load(Ordering::Relaxed);
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
        let has_values = plocks
            .iter()
            .any(|row| row.iter().any(|value| value.is_some()));
        let cells = if has_values {
            self.cells_or_alloc()
        } else {
            // All-None rows only clear; with no cells there is nothing to
            // clear, so skip materializing the arrays.
            match self.cells() {
                Some(cells) => cells,
                None => return,
            }
        };
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
                        let old_bits = cells.data[idx].swap(val.to_bits(), Ordering::Relaxed);
                        self.note_cell_transition(step, old_bits, val.to_bits());
                        match param_id {
                            Some(param_id) => {
                                cells.id_logical_ids[idx]
                                    .store(param_id.logical_id, Ordering::Relaxed);
                                cells.id_node_param_indices[idx]
                                    .store(param_id.node_param_idx, Ordering::Relaxed);
                            }
                            None => {
                                cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
                                cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
                            }
                        }
                    }
                    None => {
                        let old_bits = cells.data[idx].swap(NAN_BITS, Ordering::Relaxed);
                        self.note_cell_transition(step, old_bits, NAN_BITS);
                        cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
                        cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
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

pub struct SlotKeyLockData {
    /// Cell arrays, allocated on the first write (see `PLockCells`); absent
    /// arrays read as all-empty.
    cells: OnceLock<PLockCells>,
    max_params: usize,
    lock_count: AtomicU32,
    note_counts: Vec<AtomicU32>,
}

impl SlotKeyLockData {
    pub fn new(max_params: usize) -> Self {
        Self {
            cells: OnceLock::new(),
            max_params,
            lock_count: AtomicU32::new(0),
            note_counts: (0..MAX_MIDI_NOTES).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    fn cells(&self) -> Option<&PLockCells> {
        self.cells.get()
    }

    fn cells_or_alloc(&self) -> &PLockCells {
        self.cells.get_or_init(|| {
            let size = MAX_MIDI_NOTES * self.max_params;
            PLockCells {
                data: (0..size).map(|_| AtomicU32::new(NAN_BITS)).collect(),
                id_logical_ids: (0..size).map(|_| AtomicU64::new(0)).collect(),
                id_node_param_indices: (0..size).map(|_| AtomicU32::new(u32::MAX)).collect(),
            }
        })
    }

    fn index(&self, note: u8, param_idx: usize) -> usize {
        note as usize * self.max_params + param_idx
    }

    pub fn has_any_lock(&self) -> bool {
        self.lock_count.load(Ordering::Relaxed) > 0
    }

    fn note_count(&self, note: u8) -> u32 {
        self.note_counts
            .get(note as usize)
            .map(|count| count.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    fn note_cell_transition(&self, note: u8, old_bits: u32, new_bits: u32) {
        let old_set = !f32::from_bits(old_bits).is_nan();
        let new_set = !f32::from_bits(new_bits).is_nan();
        if !old_set && new_set {
            self.lock_count.fetch_add(1, Ordering::Relaxed);
            if let Some(count) = self.note_counts.get(note as usize) {
                count.fetch_add(1, Ordering::Relaxed);
            }
        } else if old_set && !new_set {
            self.lock_count.fetch_sub(1, Ordering::Relaxed);
            if let Some(count) = self.note_counts.get(note as usize) {
                count.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    pub fn clear_all(&self) {
        if !self.has_any_lock() {
            return;
        }
        if let Some(cells) = self.cells() {
            for idx in 0..cells.data.len() {
                cells.data[idx].store(NAN_BITS, Ordering::Relaxed);
                cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
                cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
            }
        }
        self.lock_count.store(0, Ordering::Relaxed);
        for count in &self.note_counts {
            count.store(0, Ordering::Relaxed);
        }
    }

    pub fn get(&self, note: u8, param_idx: usize) -> Option<f32> {
        let cells = self.cells()?;
        let idx = self.index(note, param_idx);
        if idx >= cells.data.len() {
            return None;
        }
        let value = f32::from_bits(cells.data[idx].load(Ordering::Relaxed));
        (!value.is_nan()).then_some(value)
    }

    pub fn set(&self, note: u8, param_idx: usize, value: f32) {
        if self.cells().is_none() && value.is_nan() {
            // Writing "no override" into a slot with no cells is a no-op.
            return;
        }
        let cells = self.cells_or_alloc();
        let idx = self.index(note, param_idx);
        if idx < cells.data.len() {
            let old_bits = cells.data[idx].swap(value.to_bits(), Ordering::Relaxed);
            self.note_cell_transition(note, old_bits, value.to_bits());
            cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
            cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
        }
    }

    pub fn set_with_id(&self, note: u8, param_idx: usize, value: f32, param_id: ParamNodeId) {
        let cells = self.cells_or_alloc();
        let idx = self.index(note, param_idx);
        if idx < cells.data.len() {
            let old_bits = cells.data[idx].swap(value.to_bits(), Ordering::Relaxed);
            self.note_cell_transition(note, old_bits, value.to_bits());
            cells.id_logical_ids[idx].store(param_id.logical_id, Ordering::Relaxed);
            cells.id_node_param_indices[idx].store(param_id.node_param_idx, Ordering::Relaxed);
        }
    }

    pub fn get_id(&self, note: u8, param_idx: usize) -> Option<ParamNodeId> {
        let cells = self.cells()?;
        let idx = self.index(note, param_idx);
        if idx >= cells.data.len() {
            return None;
        }
        let logical_id = cells.id_logical_ids[idx].load(Ordering::Relaxed);
        let node_param_idx = cells.id_node_param_indices[idx].load(Ordering::Relaxed);
        if logical_id == 0 || node_param_idx == u32::MAX {
            None
        } else {
            Some(ParamNodeId {
                logical_id,
                node_param_idx,
            })
        }
    }

    pub fn clear_note(&self, note: u8) {
        if self.note_count(note) == 0 {
            return;
        }
        let Some(cells) = self.cells() else {
            return;
        };
        for param_idx in 0..self.max_params {
            let idx = self.index(note, param_idx);
            if idx < cells.data.len() {
                let old_bits = cells.data[idx].swap(NAN_BITS, Ordering::Relaxed);
                self.note_cell_transition(note, old_bits, NAN_BITS);
                cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
                cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
            }
        }
    }

    pub fn clear_param(&self, note: u8, param_idx: usize) {
        let Some(cells) = self.cells() else {
            return;
        };
        let idx = self.index(note, param_idx);
        if idx < cells.data.len() {
            let old_bits = cells.data[idx].swap(NAN_BITS, Ordering::Relaxed);
            self.note_cell_transition(note, old_bits, NAN_BITS);
            cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
            cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
        }
    }

    pub fn note_has_any_lock(&self, note: u8, num_params: usize) -> bool {
        if num_params.min(self.max_params) == 0 {
            return false;
        }
        self.note_count(note) > 0
    }

    pub fn capture_rows(
        &self,
        num_params: usize,
    ) -> (
        BTreeMap<u8, Vec<Option<f32>>>,
        BTreeMap<u8, Vec<Option<ParamNodeId>>>,
    ) {
        let mut locks = BTreeMap::new();
        let mut lock_ids = BTreeMap::new();
        if !self.has_any_lock() {
            return (locks, lock_ids);
        }
        let Some(cells) = self.cells() else {
            return (locks, lock_ids);
        };
        let read_np = num_params.min(self.max_params);
        for note in 0..MAX_MIDI_NOTES {
            let note = note as u8;
            if self.note_count(note) == 0 {
                continue;
            }
            let base = note as usize * self.max_params;
            let mut row = vec![None; num_params];
            let mut id_row = vec![None; num_params];
            for param_idx in 0..read_np {
                let idx = base + param_idx;
                let value = f32::from_bits(cells.data[idx].load(Ordering::Relaxed));
                if !value.is_nan() {
                    row[param_idx] = Some(value);
                }
                let logical_id = cells.id_logical_ids[idx].load(Ordering::Relaxed);
                if logical_id != 0 {
                    let node_param_idx = cells.id_node_param_indices[idx].load(Ordering::Relaxed);
                    if node_param_idx != u32::MAX {
                        id_row[param_idx] = Some(ParamNodeId {
                            logical_id,
                            node_param_idx,
                        });
                    }
                }
            }
            if row.iter().any(Option::is_some) {
                locks.insert(note, row);
                lock_ids.insert(note, id_row);
            }
        }
        (locks, lock_ids)
    }

    pub fn restore_rows(
        &self,
        locks: &BTreeMap<u8, Vec<Option<f32>>>,
        lock_ids: &BTreeMap<u8, Vec<Option<ParamNodeId>>>,
        num_params: usize,
    ) {
        self.clear_all();
        let has_values = locks
            .values()
            .any(|row| row.iter().any(|value| value.is_some()));
        if !has_values {
            return;
        }
        let cells = self.cells_or_alloc();
        let np = num_params.min(self.max_params);
        for (&note, row) in locks {
            let base = note as usize * self.max_params;
            if base >= cells.data.len() {
                continue;
            }
            let id_row = lock_ids.get(&note);
            for param_idx in 0..np.min(row.len()) {
                let Some(value) = row[param_idx] else {
                    continue;
                };
                let idx = base + param_idx;
                let old_bits = cells.data[idx].swap(value.to_bits(), Ordering::Relaxed);
                self.note_cell_transition(note, old_bits, value.to_bits());
                match id_row.and_then(|ids| ids.get(param_idx)).copied().flatten() {
                    Some(param_id) => {
                        cells.id_logical_ids[idx].store(param_id.logical_id, Ordering::Relaxed);
                        cells.id_node_param_indices[idx]
                            .store(param_id.node_param_idx, Ordering::Relaxed);
                    }
                    None => {
                        cells.id_logical_ids[idx].store(0, Ordering::Relaxed);
                        cells.id_node_param_indices[idx].store(u32::MAX, Ordering::Relaxed);
                    }
                }
            }
        }
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

#[derive(Clone, Debug)]
struct SlotTensorParamMeta {
    name: String,
    shape: Vec<usize>,
    cell_offset: usize,
    flat_offset: usize,
    len: usize,
}

pub struct SlotTensorParamData {
    metadata: Mutex<Vec<SlotTensorParamMeta>>,
    num_params: AtomicU32,
    cell_offsets: Vec<AtomicU32>,
    flat_offsets: Vec<AtomicU32>,
    lengths: Vec<AtomicU32>,
    defaults: Vec<AtomicU32>,
    /// MAX_STEPS * MAX_SLOT_TENSOR_CELLS plock cells, allocated on the first
    /// plock write; absent reads as all-NaN (no plocks).
    plocks: OnceLock<Vec<AtomicU32>>,
    plock_count: AtomicU32,
    step_counts: Vec<AtomicU32>,
}

impl SlotTensorParamData {
    pub fn new() -> Self {
        Self {
            metadata: Mutex::new(Vec::new()),
            num_params: AtomicU32::new(0),
            cell_offsets: (0..MAX_SLOT_TENSOR_PARAMS)
                .map(|_| AtomicU32::new(u32::MAX))
                .collect(),
            flat_offsets: (0..MAX_SLOT_TENSOR_PARAMS)
                .map(|_| AtomicU32::new(0))
                .collect(),
            lengths: (0..MAX_SLOT_TENSOR_PARAMS)
                .map(|_| AtomicU32::new(0))
                .collect(),
            defaults: (0..MAX_SLOT_TENSOR_CELLS)
                .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                .collect(),
            plocks: OnceLock::new(),
            plock_count: AtomicU32::new(0),
            step_counts: (0..MAX_STEPS).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    fn metadata(&self) -> std::sync::MutexGuard<'_, Vec<SlotTensorParamMeta>> {
        self.metadata
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn meta_at(&self, tensor_idx: usize) -> Option<SlotTensorParamMeta> {
        self.metadata().get(tensor_idx).cloned()
    }

    fn plock_index(step: usize, flat_offset: usize, cell_idx: usize) -> usize {
        step * MAX_SLOT_TENSOR_CELLS + flat_offset + cell_idx
    }

    fn plock_cells(&self) -> Option<&Vec<AtomicU32>> {
        self.plocks.get()
    }

    fn plock_cells_or_alloc(&self) -> &Vec<AtomicU32> {
        self.plocks.get_or_init(|| {
            (0..MAX_STEPS * MAX_SLOT_TENSOR_CELLS)
                .map(|_| AtomicU32::new(NAN_BITS))
                .collect()
        })
    }

    fn has_plock_for_meta(&self, step: usize, meta: &SlotTensorParamMeta) -> bool {
        if step >= MAX_STEPS || meta.len == 0 {
            return false;
        }
        let Some(plocks) = self.plock_cells() else {
            return false;
        };
        let idx = Self::plock_index(step, meta.flat_offset, 0);
        idx < plocks.len() && !f32::from_bits(plocks[idx].load(Ordering::Relaxed)).is_nan()
    }

    fn note_plock_transition(&self, step: usize, old_set: bool, new_set: bool) {
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

    pub fn has_any_plock(&self) -> bool {
        self.plock_count.load(Ordering::Relaxed) > 0
    }

    pub fn clear_all_plocks(&self) {
        if !self.has_any_plock() {
            return;
        }
        if let Some(plocks) = self.plock_cells() {
            for value in plocks {
                value.store(NAN_BITS, Ordering::Relaxed);
            }
        }
        self.plock_count.store(0, Ordering::Relaxed);
        for count in &self.step_counts {
            count.store(0, Ordering::Relaxed);
        }
    }

    pub fn clear(&self) {
        *self.metadata() = Vec::new();
        self.num_params.store(0, Ordering::Relaxed);
        for idx in 0..MAX_SLOT_TENSOR_PARAMS {
            self.cell_offsets[idx].store(u32::MAX, Ordering::Relaxed);
            self.flat_offsets[idx].store(0, Ordering::Relaxed);
            self.lengths[idx].store(0, Ordering::Relaxed);
        }
        for value in &self.defaults {
            value.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
        self.clear_all_plocks();
    }

    pub fn apply_descriptor(&self, descriptors: &[TensorParamDescriptor]) {
        self.clear();
        let mut metadata = Vec::new();
        let mut flat_offset = 0usize;
        for desc in descriptors.iter().take(MAX_SLOT_TENSOR_PARAMS) {
            let len = desc.default.len();
            if len == 0 || len > MAX_SLOT_TENSOR_PARAM_CELLS {
                continue;
            }
            let Some(next_offset) = flat_offset.checked_add(len) else {
                break;
            };
            if next_offset > MAX_SLOT_TENSOR_CELLS {
                break;
            }
            let idx = metadata.len();
            self.cell_offsets[idx].store(desc.cell_offset as u32, Ordering::Relaxed);
            self.flat_offsets[idx].store(flat_offset as u32, Ordering::Relaxed);
            self.lengths[idx].store(len as u32, Ordering::Relaxed);
            for (cell_idx, value) in desc.default.iter().copied().enumerate() {
                self.defaults[flat_offset + cell_idx]
                    .store(clamped_tensor_cell(value).to_bits(), Ordering::Relaxed);
            }
            metadata.push(SlotTensorParamMeta {
                name: desc.name.clone(),
                shape: desc.shape.clone(),
                cell_offset: desc.cell_offset,
                flat_offset,
                len,
            });
            flat_offset = next_offset;
        }
        self.num_params
            .store(metadata.len() as u32, Ordering::Relaxed);
        *self.metadata() = metadata;
    }

    pub fn num_params(&self) -> usize {
        self.num_params.load(Ordering::Relaxed) as usize
    }

    pub fn tensor_len(&self, tensor_idx: usize) -> usize {
        self.lengths
            .get(tensor_idx)
            .map(|len| len.load(Ordering::Relaxed) as usize)
            .unwrap_or(0)
    }

    pub fn tensor_cell_offset(&self, tensor_idx: usize) -> Option<usize> {
        let raw = self.cell_offsets.get(tensor_idx)?.load(Ordering::Relaxed);
        (raw != u32::MAX).then_some(raw as usize)
    }

    pub fn default_values(&self, tensor_idx: usize) -> Option<Vec<f32>> {
        let meta = self.meta_at(tensor_idx)?;
        let mut values = Vec::with_capacity(meta.len);
        for cell_idx in 0..meta.len {
            values.push(f32::from_bits(
                self.defaults[meta.flat_offset + cell_idx].load(Ordering::Relaxed),
            ));
        }
        Some(values)
    }

    pub fn plock_values(&self, step: usize, tensor_idx: usize) -> Option<Vec<f32>> {
        let meta = self.meta_at(tensor_idx)?;
        if !self.has_plock_for_meta(step, &meta) {
            return None;
        }
        let plocks = self.plock_cells()?;
        let mut values = Vec::with_capacity(meta.len);
        for cell_idx in 0..meta.len {
            let idx = Self::plock_index(step, meta.flat_offset, cell_idx);
            values.push(f32::from_bits(plocks[idx].load(Ordering::Relaxed)));
        }
        Some(values)
    }

    pub fn resolved_values(&self, step: Option<usize>, tensor_idx: usize) -> Option<Vec<f32>> {
        step.and_then(|step| self.plock_values(step, tensor_idx))
            .or_else(|| self.default_values(tensor_idx))
    }

    pub fn set_default(&self, tensor_idx: usize, values: &[f32]) -> bool {
        let Some(meta) = self.meta_at(tensor_idx) else {
            return false;
        };
        if values.len() != meta.len {
            return false;
        }
        for (cell_idx, value) in values.iter().copied().enumerate() {
            self.defaults[meta.flat_offset + cell_idx]
                .store(clamped_tensor_cell(value).to_bits(), Ordering::Relaxed);
        }
        true
    }

    pub fn set_default_cell(
        &self,
        tensor_idx: usize,
        cell_idx: usize,
        value: f32,
    ) -> Option<Vec<f32>> {
        let mut values = self.default_values(tensor_idx)?;
        if cell_idx >= values.len() {
            return None;
        }
        values[cell_idx] = clamped_tensor_cell(value);
        self.set_default(tensor_idx, &values).then_some(values)
    }

    pub fn set_plock(&self, step: usize, tensor_idx: usize, values: &[f32]) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        let Some(meta) = self.meta_at(tensor_idx) else {
            return false;
        };
        if values.len() != meta.len {
            return false;
        }
        let had_plock = self.has_plock_for_meta(step, &meta);
        let plocks = self.plock_cells_or_alloc();
        for (cell_idx, value) in values.iter().copied().enumerate() {
            let idx = Self::plock_index(step, meta.flat_offset, cell_idx);
            plocks[idx].store(clamped_tensor_cell(value).to_bits(), Ordering::Relaxed);
        }
        self.note_plock_transition(step, had_plock, true);
        true
    }

    pub fn set_plock_cell(
        &self,
        step: usize,
        tensor_idx: usize,
        cell_idx: usize,
        value: f32,
    ) -> Option<Vec<f32>> {
        let mut values = self
            .plock_values(step, tensor_idx)
            .or_else(|| self.default_values(tensor_idx))?;
        if cell_idx >= values.len() {
            return None;
        }
        values[cell_idx] = clamped_tensor_cell(value);
        self.set_plock(step, tensor_idx, &values).then_some(values)
    }

    pub fn clear_plock(&self, step: usize, tensor_idx: usize) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        let Some(meta) = self.meta_at(tensor_idx) else {
            return false;
        };
        let had_plock = self.has_plock_for_meta(step, &meta);
        if !had_plock {
            return true;
        }
        let Some(plocks) = self.plock_cells() else {
            return true;
        };
        for cell_idx in 0..meta.len {
            let idx = Self::plock_index(step, meta.flat_offset, cell_idx);
            plocks[idx].store(NAN_BITS, Ordering::Relaxed);
        }
        self.note_plock_transition(step, true, false);
        true
    }

    pub fn capture(&self) -> Vec<TensorParamSnapshot> {
        let metadata = self.metadata().clone();
        let mut snapshots = Vec::with_capacity(metadata.len());
        for (tensor_idx, meta) in metadata.iter().enumerate() {
            let default = self.default_values(tensor_idx).unwrap_or_default();
            let mut plocks = if self.has_any_plock() {
                vec![None; MAX_STEPS]
            } else {
                Vec::new()
            };
            if self.has_any_plock() {
                for step in 0..MAX_STEPS {
                    plocks[step] = self.plock_values(step, tensor_idx);
                }
            }
            snapshots.push(TensorParamSnapshot {
                name: meta.name.clone(),
                shape: meta.shape.clone(),
                cell_offset: meta.cell_offset,
                default,
                plocks,
            });
        }
        snapshots
    }

    pub fn restore_snapshots(&self, snapshots: &[TensorParamSnapshot]) {
        let descriptors = snapshots
            .iter()
            .filter_map(|snapshot| {
                let cell_count = exposed_tensor_cell_count(&snapshot.shape)?;
                if snapshot.default.len() != cell_count {
                    return None;
                }
                Some(TensorParamDescriptor {
                    name: snapshot.name.clone(),
                    shape: snapshot.shape.clone(),
                    cell_offset: snapshot.cell_offset,
                    default: snapshot.default.clone(),
                    min: 0.0,
                    max: 1.0,
                })
            })
            .collect::<Vec<_>>();
        self.apply_descriptor(&descriptors);
        for (tensor_idx, snapshot) in snapshots.iter().enumerate().take(self.num_params()) {
            self.set_default(tensor_idx, &snapshot.default);
            for (step, values) in snapshot.plocks.iter().enumerate().take(MAX_STEPS) {
                if let Some(values) = values {
                    self.set_plock(step, tensor_idx, values);
                }
            }
        }
    }

    pub fn migrate_matching_snapshots(
        &self,
        descriptors: &[TensorParamDescriptor],
        old_snapshots: &[TensorParamSnapshot],
    ) {
        for (tensor_idx, desc) in descriptors.iter().enumerate().take(self.num_params()) {
            let Some(old) = old_snapshots
                .iter()
                .find(|snapshot| snapshot.same_identity_as_descriptor(desc))
            else {
                continue;
            };
            self.set_default(tensor_idx, &old.default);
            for (step, values) in old.plocks.iter().enumerate().take(MAX_STEPS) {
                if let Some(values) = values {
                    self.set_plock(step, tensor_idx, values);
                }
            }
        }
    }

    pub fn or_step_plock_mask(&self, mask: &mut [u64; MAX_STEPS / 64]) {
        if !self.has_any_plock() {
            return;
        }
        for step in 0..MAX_STEPS {
            if self
                .step_counts
                .get(step)
                .map(|count| count.load(Ordering::Relaxed) > 0)
                .unwrap_or(false)
            {
                mask[step / 64] |= 1u64 << (step % 64);
            }
        }
    }

    pub fn step_has_any_plock(&self, step: usize) -> bool {
        self.step_counts
            .get(step)
            .map(|count| count.load(Ordering::Relaxed) > 0)
            .unwrap_or(false)
    }
}

// ── EffectSlotState (runtime state for one effect in a track's chain) ──

pub struct EffectSlotState {
    pub node_id: AtomicU32,           // audio graph node (0 = empty)
    pub modulator_node_id: AtomicU32, // optional host modulation bank node
    pub plocks: SlotPLockData,
    pub key_locks: SlotKeyLockData,
    pub defaults: SlotParamDefaults,
    pub tensor_params: SlotTensorParamData,
    pub num_params: AtomicU32,
    pub param_node_indices: Vec<AtomicU32>, // per-param: idx field for ParamMsg
    pub param_node_spans: Vec<AtomicU32>,   // per-param: contiguous DGen cells updated by idx
    pub transport_phase_param_idx: AtomicU32,
}

pub fn capture_key_locks_by_param_name(
    slot: &EffectSlotState,
    desc: &EffectDescriptor,
) -> BTreeMap<u8, BTreeMap<String, f32>> {
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    let num_params = num_params.min(desc.params.len());
    let (key_locks, _) = slot.key_locks.capture_rows(num_params);
    let mut out = BTreeMap::new();

    for (note, row) in key_locks {
        let mut note_locks = BTreeMap::new();
        for param_idx in 0..num_params.min(row.len()) {
            let Some(value) = row[param_idx] else {
                continue;
            };
            if slot.key_locks.get_id(note, param_idx).is_some()
                && slot.key_locks.get_id(note, param_idx) != slot.param_node_id(param_idx)
            {
                continue;
            }
            note_locks.insert(desc.params[param_idx].name.clone(), value);
        }
        if !note_locks.is_empty() {
            out.insert(note, note_locks);
        }
    }

    out
}

pub fn restore_key_locks_by_param_name(
    slot: &EffectSlotState,
    desc: &EffectDescriptor,
    key_locks: &BTreeMap<u8, BTreeMap<String, f32>>,
) {
    slot.key_locks.clear_all();

    let mut name_to_idx = BTreeMap::<String, Option<usize>>::new();
    for (idx, param) in desc.params.iter().enumerate() {
        match name_to_idx.get_mut(&param.name) {
            Some(existing) => *existing = None,
            None => {
                name_to_idx.insert(param.name.clone(), Some(idx));
            }
        }
    }

    for (&note, note_locks) in key_locks {
        for (param_name, value) in note_locks {
            if !value.is_finite() {
                continue;
            }
            let Some(Some(param_idx)) = name_to_idx.get(param_name).copied() else {
                continue;
            };
            let clamped = desc.params[param_idx].clamp(*value);
            slot.set_key_lock(note, param_idx, clamped);
        }
    }
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
        let state = Self {
            node_id: AtomicU32::new(node_id),
            modulator_node_id: AtomicU32::new(0),
            plocks: SlotPLockData::new(capacity),
            key_locks: SlotKeyLockData::new(capacity),
            defaults: SlotParamDefaults::new_from_descriptor(desc),
            tensor_params: SlotTensorParamData::new(),
            num_params: AtomicU32::new(num_params as u32),
            param_node_indices,
            param_node_spans,
            transport_phase_param_idx: AtomicU32::new(
                desc.transport_phase_param_idx()
                    .unwrap_or(NO_TRANSPORT_PHASE_PARAM),
            ),
        };
        state.tensor_params.apply_descriptor(&desc.tensor_params);
        state
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
        if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
            let logical_id = self.modulator_node_id.load(Ordering::Relaxed) as u64;
            if logical_id == 0 {
                return None;
            }
            Some(ParamNodeId {
                logical_id,
                node_param_idx: raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE,
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

    pub fn set_key_lock(&self, note: u8, param_idx: usize, val: f32) {
        if let Some(param_id) = self.param_node_id(param_idx) {
            self.key_locks.set_with_id(note, param_idx, val, param_id);
        } else {
            self.key_locks.set(note, param_idx, val);
        }
    }

    pub fn clear_key_lock(&self, note: u8, param_idx: usize) {
        self.key_locks.clear_param(note, param_idx);
    }

    pub fn clear_note_key_locks(&self, note: u8) {
        self.key_locks.clear_note(note);
    }

    /// Create an empty slot (no effect loaded).
    pub fn empty() -> Self {
        Self {
            node_id: AtomicU32::new(0),
            modulator_node_id: AtomicU32::new(0),
            plocks: SlotPLockData::new(MAX_SLOT_PARAMS),
            key_locks: SlotKeyLockData::new(MAX_SLOT_PARAMS),
            defaults: SlotParamDefaults::new_zeroed(MAX_SLOT_PARAMS),
            tensor_params: SlotTensorParamData::new(),
            num_params: AtomicU32::new(0),
            param_node_indices: (0..MAX_SLOT_PARAMS).map(|_| AtomicU32::new(0)).collect(),
            param_node_spans: (0..MAX_SLOT_PARAMS).map(|_| AtomicU32::new(1)).collect(),
            transport_phase_param_idx: AtomicU32::new(NO_TRANSPORT_PHASE_PARAM),
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
        self.transport_phase_param_idx.store(
            desc.transport_phase_param_idx()
                .unwrap_or(NO_TRANSPORT_PHASE_PARAM),
            Ordering::Relaxed,
        );
        for (i, p) in desc.params.iter().enumerate() {
            self.defaults.set(i, p.default);
            if i < self.param_node_indices.len() {
                self.param_node_indices[i].store(p.node_param_idx, Ordering::Relaxed);
            }
            if i < self.param_node_spans.len() {
                self.param_node_spans[i].store(p.node_param_span.max(1), Ordering::Relaxed);
            }
        }
        self.tensor_params.apply_descriptor(&desc.tensor_params);
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
        let saved_tensor_params = self.tensor_params.capture();
        let (saved_key_locks, saved_key_lock_ids) = self.key_locks.capture_rows(preserve);
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
        self.tensor_params
            .migrate_matching_snapshots(&desc.tensor_params, &saved_tensor_params);

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
        self.key_locks
            .restore_rows(&saved_key_locks, &saved_key_lock_ids, preserve);
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
        let old_tensor_params = self.tensor_params.capture();

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
            let mut key_locks = Vec::new();
            for note in 0..MAX_MIDI_NOTES {
                let note = note as u8;
                let Some(value) = self.key_locks.get(note, old_idx) else {
                    continue;
                };
                if new_param.accepts_migrated_value_from(old_param, value) {
                    key_locks.push((note, value));
                }
            }

            migrated.push((new_idx, default, plocks, key_locks));
        }

        self.apply_descriptor_with_modulator(new_desc, node_id, modulator_node_id);
        self.tensor_params
            .migrate_matching_snapshots(&new_desc.tensor_params, &old_tensor_params);
        for step in 0..MAX_STEPS {
            for param_idx in 0..MAX_SLOT_PARAMS {
                self.plocks.clear_param(step, param_idx);
            }
        }
        self.key_locks.clear_all();

        for (new_idx, default, plocks, key_locks) in migrated {
            if let Some(value) = default {
                self.defaults.set(new_idx, value);
            }
            for (step, value) in plocks {
                self.set_plock(step, new_idx, value);
            }
            for (note, value) in key_locks {
                self.set_key_lock(note, new_idx, value);
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
        self.tensor_params.clear();
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
        self.key_locks.clear_all();
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

/// Authoring values for one device slot, excluding live graph bindings.
///
/// This is the device-history memento. Node ids, parameter-node ids, and
/// descriptor routing metadata remain owned by the live/project slot and are
/// deliberately reconstructed when these values are applied.
#[derive(Clone, Debug)]
pub struct EffectSlotValuesSnapshot {
    pub num_params: usize,
    pub defaults: Vec<f32>,
    pub plocks: Vec<Vec<Option<f32>>>,
    pub key_locks: BTreeMap<u8, Vec<Option<f32>>>,
    pub tensor_params: Vec<TensorParamSnapshot>,
    pub ir: Option<String>,
    pub prepared_ir: Option<Arc<conv_reverb::StereoIr>>,
}

impl EffectSlotValuesSnapshot {
    pub fn bit_exact_eq(&self, other: &Self) -> bool {
        let Self {
            num_params: left_num_params,
            defaults: left_defaults,
            plocks: left_plocks,
            key_locks: left_key_locks,
            tensor_params: left_tensor_params,
            ir: left_ir,
            prepared_ir: left_prepared_ir,
        } = self;
        let Self {
            num_params: right_num_params,
            defaults: right_defaults,
            plocks: right_plocks,
            key_locks: right_key_locks,
            tensor_params: right_tensor_params,
            ir: right_ir,
            prepared_ir: right_prepared_ir,
        } = other;
        left_num_params == right_num_params
            && f32_slice_bits_eq(left_defaults, right_defaults)
            && optional_f32_rows_bits_eq(left_plocks, right_plocks)
            && left_key_locks.len() == right_key_locks.len()
            && left_key_locks.iter().all(|(note, values)| {
                right_key_locks
                    .get(note)
                    .is_some_and(|other| optional_f32_slice_bits_eq(values, other))
            })
            && tensor_snapshots_bits_eq(left_tensor_params, right_tensor_params)
            && left_ir == right_ir
            && prepared_ir_bits_eq(left_prepared_ir.as_deref(), right_prepared_ir.as_deref())
    }

    pub fn retained_bytes(&self) -> usize {
        let Self {
            num_params: _,
            defaults,
            plocks,
            key_locks,
            tensor_params,
            ir,
            prepared_ir: _,
        } = self;
        std::mem::size_of::<Self>()
            + defaults.capacity() * std::mem::size_of::<f32>()
            + optional_f32_rows_retained_bytes(plocks)
            + key_locks
                .iter()
                .map(|(_, values)| {
                    std::mem::size_of::<u8>()
                        + values.capacity() * std::mem::size_of::<Option<f32>>()
                })
                .sum::<usize>()
            + tensor_params
                .iter()
                .map(tensor_snapshot_retained_bytes)
                .sum::<usize>()
            + ir.as_ref().map_or(0, String::capacity)
    }
}

fn prepared_ir_bits_eq(
    left: Option<&conv_reverb::StereoIr>,
    right: Option<&conv_reverb::StereoIr>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            f32_slice_bits_eq(&left.left.re, &right.left.re)
                && f32_slice_bits_eq(&left.left.im, &right.left.im)
                && f32_slice_bits_eq(&left.right.re, &right.right.re)
                && f32_slice_bits_eq(&left.right.im, &right.right.im)
        }
        (None, None) => true,
        _ => false,
    }
}

fn f32_slice_bits_eq(left: &[f32], right: &[f32]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.to_bits() == right.to_bits())
}

fn optional_f32_slice_bits_eq(left: &[Option<f32>], right: &[Option<f32>]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| match (left, right) {
            (Some(left), Some(right)) => left.to_bits() == right.to_bits(),
            (None, None) => true,
            _ => false,
        })
}

fn optional_f32_rows_bits_eq(
    left: &[Vec<Option<f32>>],
    right: &[Vec<Option<f32>>],
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| optional_f32_slice_bits_eq(left, right))
}

fn tensor_snapshots_bits_eq(
    left: &[TensorParamSnapshot],
    right: &[TensorParamSnapshot],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            let TensorParamSnapshot {
                name: left_name,
                shape: left_shape,
                cell_offset: left_cell_offset,
                default: left_default,
                plocks: left_plocks,
            } = left;
            let TensorParamSnapshot {
                name: right_name,
                shape: right_shape,
                cell_offset: right_cell_offset,
                default: right_default,
                plocks: right_plocks,
            } = right;
            left_name == right_name
                && left_shape == right_shape
                && left_cell_offset == right_cell_offset
                && f32_slice_bits_eq(left_default, right_default)
                && left_plocks.len() == right_plocks.len()
                && left_plocks
                    .iter()
                    .zip(right_plocks)
                    .all(|(left, right)| match (left, right) {
                        (Some(left), Some(right)) => f32_slice_bits_eq(left, right),
                        (None, None) => true,
                        _ => false,
                    })
        })
}

fn optional_f32_rows_retained_bytes(rows: &[Vec<Option<f32>>]) -> usize {
    rows.len() * std::mem::size_of::<Vec<Option<f32>>>()
        + rows
            .iter()
            .map(|row| row.capacity() * std::mem::size_of::<Option<f32>>())
            .sum::<usize>()
}

fn tensor_snapshot_retained_bytes(tensor: &TensorParamSnapshot) -> usize {
    let TensorParamSnapshot {
        name,
        shape,
        cell_offset: _,
        default,
        plocks,
    } = tensor;
    std::mem::size_of::<TensorParamSnapshot>()
        + name.capacity()
        + shape.capacity() * std::mem::size_of::<usize>()
        + default.capacity() * std::mem::size_of::<f32>()
        + plocks.capacity() * std::mem::size_of::<Option<Vec<f32>>>()
        + plocks
            .iter()
            .flatten()
            .map(|values| values.capacity() * std::mem::size_of::<f32>())
            .sum::<usize>()
}

#[derive(Clone, Debug)]
pub struct EffectSlotSnapshot {
    pub node_id: u32,
    pub modulator_node_id: u32,
    pub num_params: u32,
    pub defaults: Vec<f32>,
    pub plocks: Vec<Vec<Option<f32>>>,
    pub plock_param_ids: Vec<Vec<Option<ParamNodeId>>>,
    pub key_locks: BTreeMap<u8, Vec<Option<f32>>>,
    pub key_lock_param_ids: BTreeMap<u8, Vec<Option<ParamNodeId>>>,
    pub tensor_params: Vec<TensorParamSnapshot>,
    pub param_node_indices: Vec<u32>,
    pub param_node_spans: Vec<u32>,
    pub transport_phase_param_idx: u32,
    /// Convolution Reverb IR reference (sample hash/stem) carried through
    /// save/restore. None for every other effect.
    pub ir: Option<String>,
}

impl EffectSlotSnapshot {
    pub fn capture_authoring_values(slot: &EffectSlotState) -> EffectSlotValuesSnapshot {
        Self::capture(slot).authoring_values()
    }

    pub fn restore_authoring_values(
        slot: &EffectSlotState,
        values: &EffectSlotValuesSnapshot,
    ) -> Result<(), String> {
        let mut current = Self::capture(slot);
        current.apply_authoring_values(values)?;
        current.restore(slot);
        Ok(())
    }

    pub fn authoring_values(&self) -> EffectSlotValuesSnapshot {
        EffectSlotValuesSnapshot {
            num_params: self.num_params as usize,
            defaults: self.defaults.clone(),
            plocks: self.plocks.clone(),
            key_locks: self.key_locks.clone(),
            tensor_params: self.tensor_params.clone(),
            ir: self.ir.clone(),
            prepared_ir: conv_reverb::prepared_ir_for(self.node_id as i32),
        }
    }

    pub fn apply_authoring_values(
        &mut self,
        values: &EffectSlotValuesSnapshot,
    ) -> Result<(), String> {
        if self.num_params as usize != values.num_params
            || self.defaults.len() < values.num_params
        {
            return Err("device scalar descriptor changed while replaying history".to_string());
        }
        if self.tensor_params.len() != values.tensor_params.len()
            || self
                .tensor_params
                .iter()
                .zip(&values.tensor_params)
                .any(|(current, saved)| {
                    current.name != saved.name
                        || current.shape != saved.shape
                        || current.cell_offset != saved.cell_offset
                })
        {
            return Err("device tensor descriptor changed while replaying history".to_string());
        }
        self.defaults.clone_from(&values.defaults);
        self.plocks.clone_from(&values.plocks);
        self.key_locks.clone_from(&values.key_locks);
        self.tensor_params.clone_from(&values.tensor_params);
        self.ir.clone_from(&values.ir);
        self.rebuild_lock_param_ids();
        Ok(())
    }

    fn rebuild_lock_param_ids(&mut self) {
        let param_ids = (0..self.num_params as usize)
            .map(|param_idx| self.param_node_id(param_idx))
            .collect::<Vec<_>>();
        self.plock_param_ids = self
            .plocks
            .iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(param_idx, value)| {
                        value.and_then(|_| param_ids.get(param_idx).copied().flatten())
                    })
                    .collect()
            })
            .collect();
        self.key_lock_param_ids = self
            .key_locks
            .iter()
            .map(|(note, row)| {
                (
                    *note,
                    row.iter()
                        .enumerate()
                        .map(|(param_idx, value)| {
                            value.and_then(|_| param_ids.get(param_idx).copied().flatten())
                        })
                        .collect(),
                )
            })
            .collect();
    }

    /// Copy scene-level parameter values while preserving all step/note locks,
    /// runtime node bindings, and descriptor metadata owned by this snapshot.
    pub fn copy_base_values_from(&mut self, source: &Self) {
        let scalar_count = (self.num_params as usize)
            .min(self.defaults.len())
            .min(source.num_params as usize)
            .min(source.defaults.len());
        self.defaults[..scalar_count].copy_from_slice(&source.defaults[..scalar_count]);

        for tensor in &mut self.tensor_params {
            let Some(source_tensor) = source.tensor_params.iter().find(|candidate| {
                candidate.name == tensor.name
                    && candidate.shape == tensor.shape
                    && candidate.default.len() == tensor.default.len()
            }) else {
                continue;
            };
            tensor.default.clone_from(&source_tensor.default);
        }
    }

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
        let (key_locks, key_lock_param_ids) = slot.key_locks.capture_rows(np);
        let tensor_params = slot.tensor_params.capture();

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
            key_locks,
            key_lock_param_ids,
            tensor_params,
            param_node_indices,
            param_node_spans,
            transport_phase_param_idx: slot.transport_phase_param_idx.load(Ordering::Relaxed),
            ir: crate::effects::conv_reverb::ir_ref_for(node_id as i32),
        }
    }

    pub fn restore(&self, slot: &EffectSlotState) {
        slot.node_id.store(self.node_id, Ordering::Relaxed);
        slot.modulator_node_id
            .store(self.modulator_node_id, Ordering::Relaxed);
        slot.num_params.store(self.num_params, Ordering::Relaxed);
        slot.transport_phase_param_idx
            .store(self.transport_phase_param_idx, Ordering::Relaxed);
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
        slot.key_locks
            .restore_rows(&self.key_locks, &self.key_lock_param_ids, np);
        slot.tensor_params.restore_snapshots(&self.tensor_params);
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
        let tensor_params = desc
            .tensor_params
            .iter()
            .map(|tensor| TensorParamSnapshot {
                name: tensor.name.clone(),
                shape: tensor.shape.clone(),
                cell_offset: tensor.cell_offset,
                default: tensor.default.clone(),
                plocks: Vec::new(),
            })
            .collect();

        Self {
            node_id,
            modulator_node_id,
            num_params: np as u32,
            defaults,
            plocks,
            plock_param_ids: (0..MAX_STEPS).map(|_| vec![None; np]).collect(),
            key_locks: BTreeMap::new(),
            key_lock_param_ids: BTreeMap::new(),
            tensor_params,
            param_node_indices,
            param_node_spans,
            transport_phase_param_idx: desc
                .transport_phase_param_idx()
                .unwrap_or(NO_TRANSPORT_PHASE_PARAM),
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
            key_locks: BTreeMap::new(),
            key_lock_param_ids: BTreeMap::new(),
            tensor_params: Vec::new(),
            param_node_indices: Vec::new(),
            param_node_spans: Vec::new(),
            transport_phase_param_idx: NO_TRANSPORT_PHASE_PARAM,
            ir: None,
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new_empty();
    }

    fn param_node_id(&self, param_idx: usize) -> Option<ParamNodeId> {
        let raw_idx = self.param_node_indices.get(param_idx).copied()?;
        ParamNodeId::from_slot_param(self.node_id, self.modulator_node_id, raw_idx)
    }

    fn ensure_plock_row_capacity(&mut self, step: usize, num_params: usize) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        while self.plocks.len() <= step {
            self.plocks.push(Vec::new());
        }
        while self.plock_param_ids.len() <= step {
            self.plock_param_ids.push(Vec::new());
        }
        if self.plocks[step].len() < num_params {
            self.plocks[step].resize(num_params, None);
        }
        if self.plock_param_ids[step].len() < num_params {
            self.plock_param_ids[step].resize(num_params, None);
        }
        true
    }

    pub fn set_plock(&mut self, step: usize, param_idx: usize, value: f32) -> bool {
        let num_params = self.num_params as usize;
        if param_idx >= num_params || !self.ensure_plock_row_capacity(step, num_params) {
            return false;
        }
        self.plocks[step][param_idx] = Some(value);
        self.plock_param_ids[step][param_idx] = self.param_node_id(param_idx);
        true
    }

    pub fn resolved_param_value(&self, step: usize, param_idx: usize, fallback: f32) -> f32 {
        let default = self.defaults.get(param_idx).copied().unwrap_or(fallback);
        let Some(value) = self
            .plocks
            .get(step)
            .and_then(|row| row.get(param_idx))
            .copied()
            .flatten()
        else {
            return default;
        };
        let expected_id = self.param_node_id(param_idx);
        let stored_id = self
            .plock_param_ids
            .get(step)
            .and_then(|row| row.get(param_idx))
            .copied()
            .flatten();
        if expected_id.is_some() && stored_id == expected_id {
            value
        } else {
            default
        }
    }

    pub fn clear_plock(&mut self, step: usize, param_idx: usize) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        if let Some(step_plocks) = self.plocks.get_mut(step) {
            if param_idx < step_plocks.len() {
                step_plocks[param_idx] = None;
            }
        }
        if let Some(step_ids) = self.plock_param_ids.get_mut(step) {
            if param_idx < step_ids.len() {
                step_ids[param_idx] = None;
            }
        }
        true
    }

    pub fn clear_step_plocks(&mut self, step: usize) {
        if step >= MAX_STEPS {
            return;
        }
        if let Some(step_plocks) = self.plocks.get_mut(step) {
            for value in step_plocks {
                *value = None;
            }
        }
        if let Some(step_ids) = self.plock_param_ids.get_mut(step) {
            for value in step_ids {
                *value = None;
            }
        }
    }

    fn ensure_key_lock_row_capacity(&mut self, note: u8, num_params: usize) {
        let row = self.key_locks.entry(note).or_default();
        if row.len() < num_params {
            row.resize(num_params, None);
        }
        let id_row = self.key_lock_param_ids.entry(note).or_default();
        if id_row.len() < num_params {
            id_row.resize(num_params, None);
        }
    }

    pub fn set_key_lock(&mut self, note: u8, param_idx: usize, value: f32) -> bool {
        let num_params = self.num_params as usize;
        if param_idx >= num_params {
            return false;
        }
        self.ensure_key_lock_row_capacity(note, num_params);
        let param_id = self.param_node_id(param_idx);
        self.key_locks
            .get_mut(&note)
            .and_then(|row| row.get_mut(param_idx))
            .map(|cell| *cell = Some(value));
        self.key_lock_param_ids
            .get_mut(&note)
            .and_then(|row| row.get_mut(param_idx))
            .map(|cell| *cell = param_id);
        true
    }

    pub fn clear_key_lock(&mut self, note: u8, param_idx: usize) -> bool {
        let remove_lock_row = if let Some(row) = self.key_locks.get_mut(&note) {
            if param_idx < row.len() {
                row[param_idx] = None;
            }
            row.iter().all(Option::is_none)
        } else {
            false
        };
        if remove_lock_row {
            self.key_locks.remove(&note);
        }

        let remove_id_row = if let Some(row) = self.key_lock_param_ids.get_mut(&note) {
            if param_idx < row.len() {
                row[param_idx] = None;
            }
            row.iter().all(Option::is_none)
        } else {
            false
        };
        if remove_id_row {
            self.key_lock_param_ids.remove(&note);
        }

        true
    }

    pub fn clear_note_key_locks(&mut self, note: u8) {
        self.key_locks.remove(&note);
        self.key_lock_param_ids.remove(&note);
    }

    pub fn tensor_default_values(&self, tensor_idx: usize) -> Option<&[f32]> {
        self.tensor_params
            .get(tensor_idx)
            .map(|tensor| tensor.default.as_slice())
    }

    pub fn tensor_plock_values(&self, step: usize, tensor_idx: usize) -> Option<&[f32]> {
        self.tensor_params
            .get(tensor_idx)
            .and_then(|tensor| tensor.plocks.get(step))
            .and_then(|values| values.as_deref())
    }

    pub fn resolved_tensor_values(&self, step: usize, tensor_idx: usize) -> Option<&[f32]> {
        self.tensor_plock_values(step, tensor_idx)
            .or_else(|| self.tensor_default_values(tensor_idx))
    }

    pub fn set_tensor_default(&mut self, tensor_idx: usize, values: Vec<f32>) -> bool {
        let Some(tensor) = self.tensor_params.get_mut(tensor_idx) else {
            return false;
        };
        if values.len() != tensor.cell_count() {
            return false;
        }
        tensor.default = values.into_iter().map(clamped_tensor_cell).collect();
        true
    }

    pub fn set_tensor_default_cell(
        &mut self,
        tensor_idx: usize,
        cell_idx: usize,
        value: f32,
    ) -> bool {
        let Some(tensor) = self.tensor_params.get_mut(tensor_idx) else {
            return false;
        };
        if cell_idx >= tensor.default.len() {
            return false;
        }
        tensor.default[cell_idx] = clamped_tensor_cell(value);
        true
    }

    fn ensure_tensor_plock_rows(&mut self, tensor_idx: usize) -> bool {
        let Some(tensor) = self.tensor_params.get_mut(tensor_idx) else {
            return false;
        };
        if tensor.plocks.is_empty() {
            tensor.plocks = vec![None; MAX_STEPS];
        } else if tensor.plocks.len() < MAX_STEPS {
            tensor.plocks.resize_with(MAX_STEPS, || None);
        }
        true
    }

    pub fn set_tensor_plock(&mut self, step: usize, tensor_idx: usize, values: Vec<f32>) -> bool {
        if step >= MAX_STEPS || !self.ensure_tensor_plock_rows(tensor_idx) {
            return false;
        }
        let Some(tensor) = self.tensor_params.get_mut(tensor_idx) else {
            return false;
        };
        if values.len() != tensor.cell_count() {
            return false;
        }
        tensor.plocks[step] = Some(values.into_iter().map(clamped_tensor_cell).collect());
        true
    }

    pub fn set_tensor_plock_cell(
        &mut self,
        step: usize,
        tensor_idx: usize,
        cell_idx: usize,
        value: f32,
    ) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        let Some(base) = self
            .tensor_plock_values(step, tensor_idx)
            .or_else(|| self.tensor_default_values(tensor_idx))
            .map(|values| values.to_vec())
        else {
            return false;
        };
        if cell_idx >= base.len() {
            return false;
        }
        let mut values = base;
        values[cell_idx] = clamped_tensor_cell(value);
        self.set_tensor_plock(step, tensor_idx, values)
    }

    pub fn clear_tensor_plock(&mut self, step: usize, tensor_idx: usize) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        let Some(tensor) = self.tensor_params.get_mut(tensor_idx) else {
            return false;
        };
        if let Some(plock) = tensor.plocks.get_mut(step) {
            *plock = None;
        }
        true
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
        let old_key_locks = self.key_locks.clone();
        let old_tensor_params = self.tensor_params.clone();

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
        self.transport_phase_param_idx = desc
            .transport_phase_param_idx()
            .unwrap_or(NO_TRANSPORT_PHASE_PARAM);
        self.plocks = (0..MAX_STEPS).map(|_| vec![None; new_np]).collect();
        self.plock_param_ids = (0..MAX_STEPS).map(|_| vec![None; new_np]).collect();
        self.key_locks = BTreeMap::new();
        self.key_lock_param_ids = BTreeMap::new();
        self.tensor_params = desc
            .tensor_params
            .iter()
            .map(|tensor| TensorParamSnapshot {
                name: tensor.name.clone(),
                shape: tensor.shape.clone(),
                cell_offset: tensor.cell_offset,
                default: tensor.default.clone(),
                plocks: Vec::new(),
            })
            .collect();
        for (tensor_idx, desc_tensor) in desc.tensor_params.iter().enumerate() {
            let Some(old_tensor) = old_tensor_params
                .iter()
                .find(|snapshot| snapshot.same_identity_as_descriptor(desc_tensor))
            else {
                continue;
            };
            if let Some(new_tensor) = self.tensor_params.get_mut(tensor_idx) {
                new_tensor.default = old_tensor.default.clone();
                new_tensor.plocks = old_tensor.plocks.clone();
            }
        }

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
        for (note, saved_row) in old_key_locks {
            let row_len = preserve.min(saved_row.len());
            for param_idx in 0..row_len {
                let Some(value) = saved_row[param_idx] else {
                    continue;
                };
                self.key_locks
                    .entry(note)
                    .or_insert_with(|| vec![None; new_np])[param_idx] = Some(value);
                self.key_lock_param_ids
                    .entry(note)
                    .or_insert_with(|| vec![None; new_np])[param_idx] = self
                    .param_node_indices
                    .get(param_idx)
                    .copied()
                    .and_then(|raw_idx| {
                        ParamNodeId::from_slot_param(node_id, modulator_node_id, raw_idx)
                    });
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
