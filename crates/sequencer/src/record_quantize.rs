/// Quantization applied when recording live keyboard notes.
///
/// `Off` deliberately means preserve the performed sub-step phase; this is
/// distinct from launch quantization, where `Off` means launch immediately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordQuantize {
    Off = 0,
    Sixteenth = 1,
    Eighth = 2,
    Quarter = 3,
    Half = 4,
    Bar = 5,
}

impl RecordQuantize {
    pub const DEFAULT: Self = Self::Sixteenth;

    pub fn from_transport_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "1/16" => Some(Self::Sixteenth),
            "1/8" => Some(Self::Eighth),
            "1/4" => Some(Self::Quarter),
            "1/2" => Some(Self::Half),
            "1 bar" | "bar" => Some(Self::Bar),
            _ => None,
        }
    }

    pub const fn transport_label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Sixteenth => "1/16",
            Self::Eighth => "1/8",
            Self::Quarter => "1/4",
            Self::Half => "1/2",
            Self::Bar => "1 bar",
        }
    }

    pub const fn grid_beats(self) -> Option<f64> {
        match self {
            Self::Off => None,
            Self::Sixteenth => Some(0.25),
            Self::Eighth => Some(0.5),
            Self::Quarter => Some(1.0),
            Self::Half => Some(2.0),
            Self::Bar => Some(4.0),
        }
    }

    pub const fn from_atomic(value: u8) -> Self {
        match value {
            0 => Self::Off,
            2 => Self::Eighth,
            3 => Self::Quarter,
            4 => Self::Half,
            5 => Self::Bar,
            _ => Self::Sixteenth,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RecordQuantize;

    #[test]
    fn transport_labels_round_trip() {
        for quantize in [
            RecordQuantize::Off,
            RecordQuantize::Sixteenth,
            RecordQuantize::Eighth,
            RecordQuantize::Quarter,
            RecordQuantize::Half,
            RecordQuantize::Bar,
        ] {
            assert_eq!(
                RecordQuantize::from_transport_label(quantize.transport_label()),
                Some(quantize)
            );
        }
    }
}
