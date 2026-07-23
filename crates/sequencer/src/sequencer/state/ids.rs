use super::*;

/// Stable logical identity for a sequencer track.
///
/// Dense track indices remain the runtime addressing scheme, but authoring
/// references that must survive reordering use this id instead.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrackId(pub u64);

impl TrackId {
    pub const MIN: Self = Self(1);

    pub fn new(value: u64) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectInstanceId(pub u64);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MidiFxInstanceId(pub u64);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RackSlotId(pub u64);
