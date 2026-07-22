use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RackSlotParam {
    BaseNote,
    Gain,
    Pan,
    MaxPolyphony,
    Mute,
    Solo,
}

impl RackSlotParam {
    pub const ALL: [Self; RACK_SLOT_PARAM_COUNT] = [
        Self::BaseNote,
        Self::Gain,
        Self::Pan,
        Self::MaxPolyphony,
        Self::Mute,
        Self::Solo,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::BaseNote => 0,
            Self::Gain => 1,
            Self::Pan => 2,
            Self::MaxPolyphony => 3,
            Self::Mute => 4,
            Self::Solo => 5,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "base-note" => Some(Self::BaseNote),
            "gain" => Some(Self::Gain),
            "pan" => Some(Self::Pan),
            "max-polyphony" => Some(Self::MaxPolyphony),
            "mute" => Some(Self::Mute),
            "solo" => Some(Self::Solo),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::BaseNote => "base-note",
            Self::Gain => "gain",
            Self::Pan => "pan",
            Self::MaxPolyphony => "max-polyphony",
            Self::Mute => "mute",
            Self::Solo => "solo",
        }
    }

    pub fn clamp(self, value: f32) -> f32 {
        match self {
            Self::BaseNote => value.clamp(-48.0, 48.0),
            Self::Gain => value.clamp(0.0, 2.0),
            Self::Pan => value.clamp(-1.0, 1.0),
            Self::MaxPolyphony => value.round().clamp(1.0, crate::voice::MAX_VOICES as f32),
            Self::Mute | Self::Solo => {
                if value > 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RackSlotParamPlocks {
    pub rows: Vec<Vec<Option<f32>>>,
}

impl RackSlotParamPlocks {
    pub fn new() -> Self {
        Self {
            rows: (0..MAX_STEPS)
                .map(|_| vec![None; RACK_SLOT_PARAM_COUNT])
                .collect(),
        }
    }

    pub fn from_rows(mut rows: Vec<Vec<Option<f32>>>) -> Self {
        rows.truncate(MAX_STEPS);
        while rows.len() < MAX_STEPS {
            rows.push(Vec::new());
        }
        for row in &mut rows {
            row.truncate(RACK_SLOT_PARAM_COUNT);
            if row.len() < RACK_SLOT_PARAM_COUNT {
                row.resize(RACK_SLOT_PARAM_COUNT, None);
            }
            for param in RackSlotParam::ALL {
                if let Some(Some(value)) = row.get_mut(param.index()) {
                    *value = param.clamp(*value);
                }
            }
        }
        Self { rows }
    }

    pub(super) fn ensure_step(&mut self, step: usize) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        while self.rows.len() <= step {
            self.rows.push(vec![None; RACK_SLOT_PARAM_COUNT]);
        }
        if self.rows[step].len() < RACK_SLOT_PARAM_COUNT {
            self.rows[step].resize(RACK_SLOT_PARAM_COUNT, None);
        }
        true
    }

    pub fn get(&self, step: usize, param: RackSlotParam) -> Option<f32> {
        self.rows
            .get(step)
            .and_then(|row| row.get(param.index()))
            .copied()
            .flatten()
    }

    pub fn set(&mut self, step: usize, param: RackSlotParam, value: f32) -> bool {
        if !self.ensure_step(step) {
            return false;
        }
        self.rows[step][param.index()] = Some(param.clamp(value));
        true
    }

    pub fn clear(&mut self, step: usize, param: RackSlotParam) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        if let Some(row) = self.rows.get_mut(step) {
            if let Some(value) = row.get_mut(param.index()) {
                *value = None;
            }
        }
        true
    }

    pub fn clear_step(&mut self, step: usize) {
        if let Some(row) = self.rows.get_mut(step) {
            for value in row.iter_mut().take(RACK_SLOT_PARAM_COUNT) {
                *value = None;
            }
        }
    }

    pub fn step_has_plock(&self, step: usize) -> bool {
        self.rows
            .get(step)
            .is_some_and(|row| row.iter().take(RACK_SLOT_PARAM_COUNT).any(Option::is_some))
    }
}

impl Default for RackSlotParamPlocks {
    fn default() -> Self {
        Self::new()
    }
}
