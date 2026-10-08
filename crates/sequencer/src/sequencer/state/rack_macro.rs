use super::*;

#[derive(Clone, Debug)]
pub struct StepSlotPlocks {
    pub params: Vec<Option<f32>>,
    pub tensor_params: Vec<Option<Vec<f32>>>,
}

impl StepSlotPlocks {
    pub(super) fn clear(&mut self) {
        self.params.fill(None);
        self.tensor_params.fill(None);
    }
}

pub const RACK_SLOT_PARAM_COUNT: usize = 6;
pub const RACK_MACRO_COUNT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RackMacroId(u8);

impl RackMacroId {
    pub const ALL: [Self; RACK_MACRO_COUNT] = [
        Self(0),
        Self(1),
        Self(2),
        Self(3),
        Self(4),
        Self(5),
        Self(6),
        Self(7),
    ];

    pub fn from_index(index: usize) -> Option<Self> {
        (index < RACK_MACRO_COUNT).then_some(Self(index as u8))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn stable_key(self) -> String {
        format!("macro_{}", self.index() + 1)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RackMacroCurve {
    #[default]
    Linear,
    Exp,
    Log,
}

impl RackMacroCurve {
    /// Every curve's label, in variant order.
    pub const LABELS: [&'static str; 3] = ["linear", "exp", "log"];

    /// The curve's label (the rack panel's, the host kinds' `curve`).
    pub fn label(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Exp => "exp",
            Self::Log => "log",
        }
    }

    /// The curve a label names (any case; `exponential` and `logarithmic`
    /// too); `label` round-trips.
    pub fn from_label(label: &str) -> Option<Self> {
        match label.to_ascii_lowercase().as_str() {
            "linear" => Some(Self::Linear),
            "exp" | "exponential" => Some(Self::Exp),
            "log" | "logarithmic" => Some(Self::Log),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RackMacroTarget {
    SlotParam {
        slot: usize,
        param: String,
    },
    SlotInstrumentParam {
        slot: usize,
        param: String,
        param_index: usize,
    },
    SlotEffectParam {
        slot: usize,
        effect_slot: usize,
        param: String,
        param_index: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RackMacroMapping {
    pub target: RackMacroTarget,
    pub range_min: f32,
    pub range_max: f32,
    pub curve: RackMacroCurve,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RackMacro {
    pub id: RackMacroId,
    pub name: String,
    pub value: f32,
    pub mappings: Vec<RackMacroMapping>,
    pub plocks: Vec<Option<f32>>,
}

impl RackMacro {
    pub(super) fn default_for(id: RackMacroId) -> Self {
        Self {
            id,
            name: format!("Macro {}", id.index() + 1),
            value: 0.0,
            mappings: Vec::new(),
            plocks: vec![None; MAX_STEPS],
        }
    }

    pub fn value_at(&self, step: usize) -> f32 {
        self.plocks
            .get(step)
            .and_then(|value| *value)
            .unwrap_or(self.value)
            .clamp(0.0, 1.0)
    }
}

impl RackMacroTarget {
    /// A stable address of the target (a merge key's part): its slot and
    /// what it drives there.
    pub fn address(&self) -> String {
        match self {
            Self::SlotParam { slot, param } => format!("slot:{slot}:{param}"),
            Self::SlotInstrumentParam {
                slot, param_index, ..
            } => format!("slot:{slot}:instrument:{param_index}"),
            Self::SlotEffectParam {
                slot,
                effect_slot,
                param_index,
                ..
            } => format!("slot:{slot}:effect:{effect_slot}:{param_index}"),
        }
    }

    fn retained_bytes(&self) -> usize {
        match self {
            Self::SlotParam { param, .. }
            | Self::SlotInstrumentParam { param, .. }
            | Self::SlotEffectParam { param, .. } => param.capacity(),
        }
    }
}

/// One field of a drum rack macro, as a recorded rack macro edit writes it
/// (eseq-0l17.44): the patch keeps the field before and after, so replay
/// writes that field alone and leaves the rest of the macro (its locks,
/// its other mappings) as it is. A mapping is named by its target (a
/// macro maps a target at most once), so an edit keeps naming its mapping
/// when an earlier one is unmapped.
#[derive(Clone, Debug, PartialEq)]
pub enum RackMacroField {
    Name(String),
    /// The macro's own value (0–1), never a p-lock.
    Value(f32),
    /// The stored range of the mapping onto `target`.
    Range {
        target: RackMacroTarget,
        min: f32,
        max: f32,
    },
    Curve {
        target: RackMacroTarget,
        curve: RackMacroCurve,
    },
}

impl RackMacroField {
    /// The history label of an edit of this field.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Name(_) => "Rename rack macro",
            Self::Value(_) => "Set rack macro value",
            Self::Range { .. } => "Set rack macro range",
            Self::Curve { .. } => "Set rack macro curve",
        }
    }

    /// The field's address within its macro (a merge key's tail).
    pub fn address(&self) -> String {
        match self {
            Self::Name(_) => "name".to_string(),
            Self::Value(_) => "value".to_string(),
            Self::Range { target, .. } => format!("mapping:{}:range", target.address()),
            Self::Curve { target, .. } => format!("mapping:{}:curve", target.address()),
        }
    }

    /// The position of the mapping onto `target` in `rack_macro`; an error
    /// when it is no longer mapped.
    fn mapping(rack_macro: &RackMacro, target: &RackMacroTarget) -> Result<usize, String> {
        (rack_macro.mappings.iter())
            .position(|mapping| mapping.target == *target)
            .ok_or_else(|| "the mapping is gone".to_string())
    }

    /// This field of `rack_macro` now; an error when its mapping is gone.
    pub fn read(&self, rack_macro: &RackMacro) -> Result<Self, String> {
        Ok(match self {
            Self::Name(_) => Self::Name(rack_macro.name.clone()),
            Self::Value(_) => Self::Value(rack_macro.value),
            Self::Range { target, .. } => {
                let mapping = &rack_macro.mappings[Self::mapping(rack_macro, target)?];
                Self::Range {
                    target: target.clone(),
                    min: mapping.range_min,
                    max: mapping.range_max,
                }
            }
            Self::Curve { target, .. } => Self::Curve {
                target: target.clone(),
                curve: rack_macro.mappings[Self::mapping(rack_macro, target)?].curve,
            },
        })
    }

    /// Write this field into `rack_macro`; a mapping that is gone, a value
    /// outside 0–1 or a range bound that is not finite is an error that
    /// changes nothing.
    pub fn write(&self, rack_macro: &mut RackMacro) -> Result<(), String> {
        match self {
            Self::Name(name) => rack_macro.name.clone_from(name),
            Self::Value(value) if (0.0..=1.0).contains(value) => rack_macro.value = *value,
            Self::Value(value) => return Err(format!("{value} is outside 0–1")),
            Self::Range { target, min, max } if min.is_finite() && max.is_finite() => {
                let index = Self::mapping(rack_macro, target)?;
                let mapping = &mut rack_macro.mappings[index];
                mapping.range_min = *min;
                mapping.range_max = *max;
            }
            Self::Range { .. } => return Err("a range bound is not finite".to_string()),
            Self::Curve { target, curve } => {
                let index = Self::mapping(rack_macro, target)?;
                rack_macro.mappings[index].curve = *curve;
            }
        }
        Ok(())
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Name(name) => name.capacity(),
                Self::Value(_) => 0,
                Self::Range { target, .. } | Self::Curve { target, .. } => target.retained_bytes(),
            }
    }
}

pub fn default_rack_macros() -> Vec<RackMacro> {
    RackMacroId::ALL
        .into_iter()
        .map(RackMacro::default_for)
        .collect()
}

pub(super) fn remove_rack_macro_slot_targets(macros: &mut [RackMacro], removed_slot: usize) {
    for rack_macro in macros {
        rack_macro.mappings.retain(|mapping| match mapping.target {
            RackMacroTarget::SlotParam { slot, .. }
            | RackMacroTarget::SlotInstrumentParam { slot, .. }
            | RackMacroTarget::SlotEffectParam { slot, .. } => slot != removed_slot,
        });
        for mapping in &mut rack_macro.mappings {
            let slot = match &mut mapping.target {
                RackMacroTarget::SlotParam { slot, .. }
                | RackMacroTarget::SlotInstrumentParam { slot, .. }
                | RackMacroTarget::SlotEffectParam { slot, .. } => slot,
            };
            if *slot > removed_slot {
                *slot -= 1;
            }
        }
    }
}
