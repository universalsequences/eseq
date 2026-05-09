use std::sync::atomic::{AtomicU32, Ordering};

use crate::sequencer::MAX_STEPS;

/// Maximum number of parameters per effect slot.
/// Custom instruments can easily exceed 16 params, and sequenced p-lock dispatch
/// iterates over every declared param. Keep this comfortably above current
/// instrument sizes so defaults/plocks/node indices stay aligned.
pub const MAX_SLOT_PARAMS: usize = 128;

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

    #[test]
    fn builtin_dynamics_exposes_macro_params() {
        let desc = EffectDescriptor::builtin_444_compressor();
        let names: Vec<&str> = desc.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "mode", "amount", "attack", "release", "low cut", "drive", "output", "mix",
                "enabled"
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
                "Reverb",
                "444 Compressor",
                "Glue Compressor",
                "Compressor",
                "Limiter"
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
    }

    #[test]
    fn default_full_chain_contains_only_empty_insert_slots() {
        let chain = EffectDescriptor::default_full_chain();
        assert_eq!(chain.len(), crate::lisp_effect::MAX_CUSTOM_FX);
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
            host_control: None,
        }
    }

    pub fn builtin_insert_names() -> &'static [&'static str] {
        &[
            "Filter",
            "Delay",
            "Reverb",
            "444 Compressor",
            "Glue Compressor",
            "Compressor",
            "Limiter",
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
            "Reverb" => Some(Self::builtin_reverb_insert()),
            "444 Compressor" => Some(Self::builtin_444_compressor()),
            "Glue Compressor" => Some(Self::builtin_glue_compressor()),
            "Compressor" => Some(Self::builtin_compressor()),
            "Limiter" => Some(Self::builtin_limiter()),
            _ => None,
        }
    }

    /// Built-in filter effect descriptor.
    pub fn builtin_filter() -> Self {
        Self {
            name: "Filter".to_string(),
            input_channels: 2,
            output_channels: 2,
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
                    node_param_idx: crate::filter::FILTER_PARAM_CUTOFF as u32,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "resonance".to_string(),
                    min: 0.5,
                    max: 10.0,
                    default: 1.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_RESONANCE as u32,
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
                },
                ParamDescriptor {
                    name: "lfo sync".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::filter::FILTER_PARAM_LFO_SYNCED as u32,
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    node_param_idx: crate::sampler::PARAM_ATTACK_SAMPLES as u32,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "synced".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::sampler::PARAM_RELEASE_SAMPLES as u32,
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
                    node_param_idx: crate::sampler::PARAM_START_POINT as u32,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "feedback".to_string(),
                    min: 0.0,
                    max: 0.95,
                    default: 0.3,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::sampler::PARAM_END_POINT as u32,
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
                Self::enabled_param(crate::delay::DELAY_PARAM_ENABLED as u32, 1.0),
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
                    host_control: None,
                },
                ParamDescriptor {
                    name: "size".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.2,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::reverb::REVERB_PARAM_SIZE as u32,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "brightness".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.8,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::reverb::REVERB_PARAM_BRIGHT as u32,
                    host_control: None,
                },
                ParamDescriptor {
                    name: "replace".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.3,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::reverb::REVERB_PARAM_REPLACE as u32,
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
                },
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
                },
                Self::enabled_param(crate::dynamics::DYNAMICS_PARAM_ENABLED as u32, 1.0),
            ],
        }
    }

    /// SP-404-inspired compressor: sustain, level, and post-compression color.
    pub fn builtin_444_compressor() -> Self {
        Self::builtin_dynamics_variant("444 Compressor", 1.0, 0.62, 1.0, 2.0, 55.0, 0.32, -1.0, 1.0)
    }

    /// SSL-style bus glue: linked stereo detection, low-cut sidechain, and auto release.
    pub fn builtin_glue_compressor() -> Self {
        Self::builtin_dynamics_variant("Glue Compressor", 0.0, 0.42, 2.0, 2.0, 90.0, 0.12, 0.0, 1.0)
    }

    /// General-purpose compressor with conservative hybrid behavior.
    pub fn builtin_compressor() -> Self {
        Self {
            name: "Compressor".to_string(),
            input_channels: 2,
            output_channels: 2,
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
                    host_control: None,
                },
                ParamDescriptor {
                    name: "ratio".to_string(),
                    min: 1.0,
                    max: 20.0,
                    default: 4.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Exponential,
                    node_param_idx: crate::compressor::COMPRESSOR_PARAM_RATIO as u32,
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
                },
                Self::enabled_param(crate::limiter::LIMITER_PARAM_ENABLED as u32, 1.0),
            ],
        }
    }

    /// Back-compat alias for projects created while the generic prototype existed.
    pub fn builtin_dynamics() -> Self {
        Self::builtin_444_compressor()
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
                Self::enabled_param(crate::sampler::SAMPLER_PARAM_ENABLED as u32, 1.0),
                ParamDescriptor {
                    name: "reverse".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::sampler::PARAM_REVERSE as u32,
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
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
                    host_control: None,
                },
            ],
        }
    }

    /// Default fixed effect chain descriptors.
    pub fn default_chain() -> Vec<Self> {
        Vec::new()
    }

    /// Full default chain: MAX_CUSTOM_FX empty insert slots.
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
                node_param_idx: (crate::lisp_effect::HEADER_SLOTS + p.cell_id) as u32,
                host_control: None,
            })
            .collect();
        descriptors.push(Self::enabled_param(
            crate::lisp_effect::DGEN_ENABLED_PARAM_IDX as u32,
            1.0,
        ));
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

    /// Copy all runtime slot payload from another slot.
    ///
    /// Used when compacting custom FX slots after deleting one slot. This moves
    /// defaults and p-locks along with the node binding so slot-indexed
    /// automation continues to refer to the same audible effect.
    pub fn copy_from(&self, other: &EffectSlotState) {
        EffectSlotSnapshot::capture(other).restore(self);
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

    pub fn clear(&mut self) {
        *self = Self::new_empty();
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
