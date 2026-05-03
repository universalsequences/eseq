use std::sync::atomic::{AtomicU32, Ordering};

use crate::sequencer::MAX_STEPS;

/// Maximum number of parameters per effect slot.
/// Custom instruments can easily exceed 16 params, and sequenced p-lock dispatch
/// iterates over every declared param. Keep this comfortably above current
/// instrument sizes so defaults/plocks/node indices stay aligned.
pub const MAX_SLOT_PARAMS: usize = 128;

/// Number of built-in effect slots (Filter, Delay). Slots at this index or higher are custom/lisp.
pub const BUILTIN_SLOT_COUNT: usize = 2;

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
    pub node_param_idx: u32, // index into audio node's state array
    pub host_control: Option<HostControl>,
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{EffectDescriptor, EffectSlotState, ParamDescriptor, ParamKind, ParamScaling};

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
            host_control: None,
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
            host_control: None,
        };

        let value = desc.denormalize(0.5);
        assert!((value - 632.4555).abs() < 0.1, "value was {value}");
    }

    #[test]
    fn sync_descriptor_preserves_existing_defaults_and_plocks() {
        let original = EffectDescriptor {
            name: "orig".to_string(),
            input_channels: 2,
            output_channels: 2,
            params: vec![
                ParamDescriptor {
                    name: "a".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.1,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 3,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "b".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.2,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 4,
                    host_control: None,
                },
            ],
        };
        let rebound = EffectDescriptor {
            name: "rebound".to_string(),
            input_channels: 2,
            output_channels: 2,
            params: vec![
                ParamDescriptor {
                    name: "a".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.9,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 10,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "b".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.8,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 11,
                    host_control: None,
                },
            ],
        };

        let slot = EffectSlotState::new(&original, 100);
        slot.defaults.set(0, 0.42);
        slot.defaults.set(1, 0.73);
        slot.plocks.set(3, 0, 0.33);
        slot.plocks.set(4, 1, 0.66);

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
    fn lisp_manifest_params_address_dgen_wrapper_state() {
        let desc = EffectDescriptor::from_lisp_manifest(
            "custom",
            &[crate::lisp_effect::DGenParam {
                name: "cutoff".to_string(),
                cell_id: 12,
                default: 1000.0,
                min: 20.0,
                max: 20_000.0,
                unit: Some("Hz".to_string()),
                hidden: false,
            }],
            0,
            1,
        );

        assert_eq!(
            desc.params[0].node_param_idx,
            (crate::lisp_effect::HEADER_SLOTS + 12) as u32
        );
    }
}

// ── EffectDescriptor ──

#[derive(Clone, Debug)]
pub struct EffectDescriptor {
    pub name: String,
    pub params: Vec<ParamDescriptor>,
    pub input_channels: usize,
    pub output_channels: usize,
}

impl EffectDescriptor {
    /// Built-in filter effect descriptor.
    pub fn builtin_filter() -> Self {
        Self {
            name: "Filter".to_string(),
            input_channels: 2,
            output_channels: 2,
            params: vec![
                ParamDescriptor {
                    name: "enabled".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: 0,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "mode".to_string(),
                    min: 0.0,
                    max: 2.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec![
                            "lowpass".to_string(),
                            "highpass".to_string(),
                            "bandpass".to_string(),
                        ],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 1,
                    host_control: None,
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
                    node_param_idx: 2,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "resonance".to_string(),
                    min: 0.5,
                    max: 10.0,
                    default: 1.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 3,
                    host_control: None,
                },
            ],
        }
    }

    /// Built-in delay effect descriptor.
    pub fn builtin_delay() -> Self {
        Self {
            name: "Delay".to_string(),
            input_channels: 2,
            output_channels: 2,
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
                    node_param_idx: 0,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "synced".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: 1,
                    host_control: None,
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
                    node_param_idx: 2,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "feedback".to_string(),
                    min: 0.0,
                    max: 0.95,
                    default: 0.3,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 3,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "dampening".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 4,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "width".to_string(),
                    min: 0.0,
                    max: 2.0,
                    default: 1.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 5,
                    host_control: None,
                },
            ],
        }
    }

    /// Built-in sampler instrument descriptor.
    pub fn builtin_sampler() -> Self {
        Self {
            name: "Sampler".to_string(),
            input_channels: 0,
            output_channels: 2,
            params: vec![
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
                },
            ],
        }
    }

    /// Default effect chain descriptors: [Filter, Delay].
    pub fn default_chain() -> Vec<Self> {
        vec![Self::builtin_filter(), Self::builtin_delay()]
    }

    /// Full default chain: [Filter, Delay] + MAX_CUSTOM_FX empty slots.
    pub fn default_full_chain() -> Vec<Self> {
        let mut chain = Self::default_chain();
        for _ in 0..crate::lisp_effect::MAX_CUSTOM_FX {
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
        }
    }

    /// Construct from a lisp effect manifest.
    pub fn from_lisp_manifest(
        name: &str,
        params: &[crate::lisp_effect::DGenParam],
        input_channels: usize,
        output_channels: usize,
    ) -> Self {
        let descriptors = params
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
                node_param_idx: (crate::lisp_effect::HEADER_SLOTS + p.cell_id) as u32,
                host_control: None,
            })
            .collect();
        Self {
            name: name.to_string(),
            params: descriptors,
            input_channels,
            output_channels,
        }
    }
}

// ── SlotPLockData (replaces EffectPLockData and LispPLockData) ──

/// Per-slot per-step parameter overrides.
/// NaN = no override (use slot default).
/// No internal clamping — callers pass clamped values.
pub struct SlotPLockData {
    data: Vec<AtomicU32>,
    max_params: usize,
}

impl SlotPLockData {
    pub fn new(max_params: usize) -> Self {
        let size = MAX_STEPS * max_params;
        let data: Vec<AtomicU32> = (0..size).map(|_| AtomicU32::new(NAN_BITS)).collect();
        Self { data, max_params }
    }

    fn index(&self, step: usize, param_idx: usize) -> usize {
        step * self.max_params + param_idx
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
            self.data[idx].store(val.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn clear_step(&self, step: usize) {
        for p in 0..self.max_params {
            let idx = self.index(step, p);
            if idx < self.data.len() {
                self.data[idx].store(NAN_BITS, Ordering::Relaxed);
            }
        }
    }

    pub fn clear_param(&self, step: usize, param_idx: usize) {
        let idx = self.index(step, param_idx);
        if idx < self.data.len() {
            self.data[idx].store(NAN_BITS, Ordering::Relaxed);
        }
    }

    pub fn step_has_any_plock(&self, step: usize, num_params: usize) -> bool {
        for p in 0..num_params.min(self.max_params) {
            let idx = self.index(step, p);
            if idx < self.data.len() {
                let bits = self.data[idx].load(Ordering::Relaxed);
                if !f32::from_bits(bits).is_nan() {
                    return true;
                }
            }
        }
        false
    }
}

// ── SlotParamDefaults (replaces TrackEffectDefaults and LispParamDefaults) ──

pub struct SlotParamDefaults {
    data: Vec<AtomicU32>,
}

impl SlotParamDefaults {
    pub fn new_from_descriptor(desc: &EffectDescriptor) -> Self {
        let data: Vec<AtomicU32> = desc
            .params
            .iter()
            .map(|p| AtomicU32::new(p.default.to_bits()))
            .collect();
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
    pub node_id: AtomicU32, // audio graph node (0 = empty)
    pub plocks: SlotPLockData,
    pub defaults: SlotParamDefaults,
    pub num_params: AtomicU32,
    pub param_node_indices: Vec<AtomicU32>, // per-param: idx field for ParamMsg
}

impl EffectSlotState {
    pub fn new(desc: &EffectDescriptor, node_id: u32) -> Self {
        let num_params = desc.params.len();
        let param_node_indices: Vec<AtomicU32> = desc
            .params
            .iter()
            .map(|p| AtomicU32::new(p.node_param_idx))
            .collect();
        Self {
            node_id: AtomicU32::new(node_id),
            plocks: SlotPLockData::new(MAX_SLOT_PARAMS),
            defaults: SlotParamDefaults::new_from_descriptor(desc),
            num_params: AtomicU32::new(num_params as u32),
            param_node_indices,
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

    /// Create an empty slot (no effect loaded).
    pub fn empty() -> Self {
        Self {
            node_id: AtomicU32::new(0),
            plocks: SlotPLockData::new(MAX_SLOT_PARAMS),
            defaults: SlotParamDefaults::new_zeroed(MAX_SLOT_PARAMS),
            num_params: AtomicU32::new(0),
            param_node_indices: (0..MAX_SLOT_PARAMS).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    /// Overwrite this pre-allocated slot in-place from a descriptor and node ID.
    pub fn apply_descriptor(&self, desc: &EffectDescriptor, node_id: u32) {
        self.node_id.store(node_id, Ordering::Relaxed);
        self.num_params
            .store(desc.params.len() as u32, Ordering::Relaxed);
        for (i, p) in desc.params.iter().enumerate() {
            self.defaults.set(i, p.default);
            if i < self.param_node_indices.len() {
                self.param_node_indices[i].store(p.node_param_idx, Ordering::Relaxed);
            }
        }
    }

    /// Rebind this live slot to the current graph descriptor/node while
    /// preserving the stored defaults and p-locks as far as possible.
    pub fn sync_descriptor(&self, desc: &EffectDescriptor, node_id: u32) {
        let old_num_params = self.num_params.load(Ordering::Relaxed) as usize;
        let preserve = old_num_params.min(desc.params.len());

        let mut saved_defaults = Vec::with_capacity(preserve);
        for param_idx in 0..preserve {
            saved_defaults.push(self.defaults.get(param_idx));
        }

        let mut saved_plocks = Vec::with_capacity(MAX_STEPS);
        for step in 0..MAX_STEPS {
            let mut step_plocks = Vec::with_capacity(preserve);
            for param_idx in 0..preserve {
                step_plocks.push(self.plocks.get(step, param_idx));
            }
            saved_plocks.push(step_plocks);
        }

        self.apply_descriptor(desc, node_id);

        for param_idx in 0..preserve {
            self.defaults.set(param_idx, saved_defaults[param_idx]);
        }
        for step in 0..MAX_STEPS {
            for param_idx in 0..preserve {
                match saved_plocks[step][param_idx] {
                    Some(value) => self.plocks.set(step, param_idx, value),
                    None => self.plocks.clear_param(step, param_idx),
                }
            }
        }
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
        }
        for step in 0..MAX_STEPS {
            for param_idx in 0..MAX_SLOT_PARAMS {
                self.plocks.clear_param(step, param_idx);
            }
        }
    }
}

// ── EffectSlotSnapshot (for pattern save/restore) ──

#[derive(Clone, Debug)]
pub struct EffectSlotSnapshot {
    pub node_id: u32,
    pub num_params: u32,
    pub defaults: Vec<f32>,
    pub plocks: Vec<Vec<Option<f32>>>,
    pub param_node_indices: Vec<u32>,
}

impl EffectSlotSnapshot {
    pub fn capture(slot: &EffectSlotState) -> Self {
        let node_id = slot.node_id.load(Ordering::Relaxed);
        let num_params = slot.num_params.load(Ordering::Relaxed);
        let np = num_params as usize;

        let mut defaults = Vec::with_capacity(np);
        for i in 0..np {
            defaults.push(slot.defaults.get(i));
        }

        let mut plocks = Vec::with_capacity(MAX_STEPS);
        for s in 0..MAX_STEPS {
            let mut step_plocks = Vec::with_capacity(np);
            for i in 0..np {
                step_plocks.push(slot.plocks.get(s, i));
            }
            plocks.push(step_plocks);
        }

        let mut param_node_indices = Vec::with_capacity(np);
        for i in 0..np {
            if i < slot.param_node_indices.len() {
                param_node_indices.push(slot.param_node_indices[i].load(Ordering::Relaxed));
            } else {
                param_node_indices.push(0);
            }
        }

        Self {
            node_id,
            num_params,
            defaults,
            plocks,
            param_node_indices,
        }
    }

    pub fn restore(&self, slot: &EffectSlotState) {
        slot.node_id.store(self.node_id, Ordering::Relaxed);
        slot.num_params.store(self.num_params, Ordering::Relaxed);
        let np = self.num_params as usize;

        for i in 0..np {
            if i < self.defaults.len() {
                slot.defaults.set(i, self.defaults[i]);
            }
        }

        for s in 0..MAX_STEPS {
            if s < self.plocks.len() {
                for i in 0..np {
                    if i < self.plocks[s].len() {
                        match self.plocks[s][i] {
                            Some(val) => slot.plocks.set(s, i, val),
                            None => slot.plocks.clear_param(s, i),
                        }
                    }
                }
            }
        }
    }

    pub fn new_default(desc: &EffectDescriptor, node_id: u32) -> Self {
        let np = desc.params.len();
        let defaults: Vec<f32> = desc.params.iter().map(|p| p.default).collect();
        let plocks: Vec<Vec<Option<f32>>> = (0..MAX_STEPS).map(|_| vec![None; np]).collect();
        let param_node_indices: Vec<u32> = desc.params.iter().map(|p| p.node_param_idx).collect();

        Self {
            node_id,
            num_params: np as u32,
            defaults,
            plocks,
            param_node_indices,
        }
    }

    pub fn new_empty() -> Self {
        Self {
            node_id: 0,
            num_params: 0,
            defaults: Vec::new(),
            plocks: (0..MAX_STEPS).map(|_| Vec::new()).collect(),
            param_node_indices: Vec::new(),
        }
    }

    pub fn sync_to_descriptor(&mut self, desc: &EffectDescriptor, node_id: u32) {
        let new_np = desc.params.len();
        let old_defaults = self.defaults.clone();
        let old_plocks = self.plocks.clone();

        self.node_id = node_id;
        self.num_params = new_np as u32;
        self.defaults = desc.params.iter().map(|p| p.default).collect();
        self.param_node_indices = desc.params.iter().map(|p| p.node_param_idx).collect();
        self.plocks = (0..MAX_STEPS).map(|_| vec![None; new_np]).collect();

        let preserve = old_defaults.len().min(new_np);
        for i in 0..preserve {
            self.defaults[i] = old_defaults[i];
        }
        for step in 0..MAX_STEPS {
            if let Some(saved_step) = old_plocks.get(step) {
                for param_idx in 0..preserve.min(saved_step.len()) {
                    self.plocks[step][param_idx] = saved_step[param_idx];
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
