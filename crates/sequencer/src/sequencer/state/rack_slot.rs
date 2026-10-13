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

    /// A per-hit / macro spelling: case- and `_`/`-`-insensitive, an
    /// optional leading `:`, plus the `transpose` and `polyphony` aliases.
    pub fn from_label(label: &str) -> Option<Self> {
        let normalized = label
            .trim_start_matches(':')
            .replace('_', "-")
            .to_ascii_lowercase();
        match normalized.as_str() {
            "transpose" => Some(Self::BaseNote),
            "polyphony" => Some(Self::MaxPolyphony),
            other => Self::from_name(other),
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
            Self::MaxPolyphony => value.round().clamp(1.0, crate::audio::MAX_VOICES as f32),
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
    pub rows: crate::effects::StepLockRows<f32>,
}

impl RackSlotParamPlocks {
    pub fn new() -> Self {
        Self {
            rows: crate::effects::StepLockRows::default(),
        }
    }

    pub fn from_rows(mut rows: Vec<Vec<Option<f32>>>) -> Self {
        rows.truncate(MAX_STEPS);
        for row in &mut rows {
            row.truncate(RACK_SLOT_PARAM_COUNT);
            for param in RackSlotParam::ALL {
                if let Some(Some(value)) = row.get_mut(param.index()) {
                    *value = param.clamp(*value);
                }
            }
        }
        Self { rows: rows.into() }
    }

    pub fn get(&self, step: usize, param: RackSlotParam) -> Option<f32> {
        self.rows
            .get(step)
            .and_then(|row| row.get(param.index()))
            .copied()
            .flatten()
    }

    pub fn set(&mut self, step: usize, param: RackSlotParam, value: f32) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        self.rows.set(step, param.index(), Some(param.clamp(value)));
        true
    }

    pub fn clear(&mut self, step: usize, param: RackSlotParam) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        self.rows.set(step, param.index(), None);
        true
    }

    pub fn clear_step(&mut self, step: usize) {
        self.rows.clear_step(step);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rack_control_locks_normalize_share_and_edit_independently() {
        let mut rows = vec![vec![None; RACK_SLOT_PARAM_COUNT]; MAX_STEPS];
        rows[255][RackSlotParam::Gain.index()] = Some(9.0);
        let mut locks = RackSlotParamPlocks::from_rows(rows);
        assert_eq!(locks.get(255, RackSlotParam::Gain), Some(2.0));
        let saved = locks.clone();
        assert_eq!(locks.rows.as_ptr(), saved.rows.as_ptr());
        locks.clear_step(255);
        assert_eq!(locks.rows.retained_bytes(), 0);
        assert_eq!(saved.get(255, RackSlotParam::Gain), Some(2.0));
        assert!(locks.set(4, RackSlotParam::Pan, -0.0));
        assert_eq!(locks.get(4, RackSlotParam::Pan).unwrap().to_bits(), (-0.0f32).to_bits());
        locks.clear(4, RackSlotParam::Pan);
        assert_eq!(locks.rows.retained_bytes(), 0);
        assert!(!locks.set(MAX_STEPS, RackSlotParam::Gain, 1.0));
    }
}
