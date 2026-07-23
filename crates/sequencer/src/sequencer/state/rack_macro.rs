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
