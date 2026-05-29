use std::collections::BTreeSet;
use std::sync::Mutex;

use sequencer::sequencer::StepParam;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum UiInvalidation {
    Full(FullInvalidation),
    CurrentTrack {
        previous: usize,
        current: usize,
    },
    TrackTopology(TrackTopologyInvalidation),
    BusTopology,
    ProjectState,
    Pattern(PatternInvalidation),
    Step {
        track: usize,
        step: usize,
        change: StepInvalidation,
    },
    StepSelection {
        track: usize,
    },
    TrackMixer {
        track: usize,
        change: TrackMixerInvalidation,
    },
    TrackBusSend {
        track: usize,
        bus: usize,
    },
    TrackRoute {
        track: usize,
    },
    ModRoutes,
    BusMixer {
        bus: usize,
        change: BusMixerInvalidation,
    },
    TrackParam {
        track: usize,
        change: TrackParamInvalidation,
    },
    TrackParamPanel {
        track: usize,
    },
    Instrument {
        track: usize,
        change: InstrumentInvalidation,
    },
    TrackFx {
        track: usize,
        change: TrackFxInvalidation,
    },
    MidiFx {
        track: usize,
        change: MidiFxInvalidation,
    },
    BusFx {
        bus: usize,
        change: BusFxInvalidation,
    },
    PianoRoll {
        track: usize,
        change: PianoRollInvalidation,
    },
    Transport(TransportInvalidation),
    Recording(RecordingInvalidation),
    DeleteTarget,
    AutoFollow,
    Sidebar {
        track: usize,
        change: SidebarInvalidation,
    },
    Browser(BrowserInvalidation),
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum FullInvalidation {
    ProjectLoaded,
    PatternSwitched,
    RecoveredFromUnknownChange,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum TrackTopologyInvalidation {
    TracksAddedRemovedOrReordered,
    TrackNames,
    TrackColors,
    InstrumentType { track: usize },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum PatternInvalidation {
    AllTracks,
    WholeTrack { track: usize },
    TrackLength { track: usize },
    TrackTiming { track: usize },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum StepInvalidation {
    Active,
    Payload,
    Param(StepParamKey),
    DurationSpan,
    PlockPresence,
    Selected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum StepParamKey {
    Duration,
    Velocity,
    Speed,
    AuxA,
    AuxB,
    Transpose,
    Pan,
    Chop,
    Sync,
    Delay,
}

impl From<StepParam> for StepParamKey {
    fn from(param: StepParam) -> Self {
        match param {
            StepParam::Duration => Self::Duration,
            StepParam::Velocity => Self::Velocity,
            StepParam::Speed => Self::Speed,
            StepParam::AuxA => Self::AuxA,
            StepParam::AuxB => Self::AuxB,
            StepParam::Transpose => Self::Transpose,
            StepParam::Pan => Self::Pan,
            StepParam::Chop => Self::Chop,
            StepParam::Sync => Self::Sync,
            StepParam::Delay => Self::Delay,
        }
    }
}

impl StepParamKey {
    pub(crate) fn to_step_param(self) -> StepParam {
        match self {
            Self::Duration => StepParam::Duration,
            Self::Velocity => StepParam::Velocity,
            Self::Speed => StepParam::Speed,
            Self::AuxA => StepParam::AuxA,
            Self::AuxB => StepParam::AuxB,
            Self::Transpose => StepParam::Transpose,
            Self::Pan => StepParam::Pan,
            Self::Chop => StepParam::Chop,
            Self::Sync => StepParam::Sync,
            Self::Delay => StepParam::Delay,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum TrackMixerInvalidation {
    Volume,
    Pan,
    Mute,
    Solo,
    MutedBySolo,
    RecordArm,
    Output,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum BusMixerInvalidation {
    Volume,
    Mute,
    Solo,
    Steps,
    Timing,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum TrackParamInvalidation {
    Attack,
    Release,
    Swing,
    Send,
    NumSteps,
    Gate,
    Poly,
    MaxPolyphony,
    Timebase,
    SwingResolution,
    Fts,
    Accumulator,
    AccumLimit,
    AccumMode,
    Output,
    BusSends,
    Plocks,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum InstrumentInvalidation {
    Param { param: usize },
    BaseNote,
    SamplerSelectionTime,
    PanelTopology,
    Analysis,
    Playhead,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum TrackFxInvalidation {
    Param { slot: usize, param: usize },
    Plock { slot: usize, param: usize },
    Topology,
    PanelTree,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum MidiFxInvalidation {
    Param { slot: usize, param: usize },
    Topology,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum BusFxInvalidation {
    Param { slot: usize, param: usize },
    Topology,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum PianoRollInvalidation {
    Items,
    Selection,
    Lanes,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum TransportInvalidation {
    Playing,
    Bpm,
    TransportPlayhead,
    CurrentTrackPlayhead,
    AllTrackPlayheads,
    Cpu,
    MasterMeter,
    TrackMeters,
    BusMeters,
    Modulators,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum RecordingInvalidation {
    RecordingEnabled,
    ArmedTracks,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum SidebarInvalidation {
    TrackBrowser,
    Presets,
    Plocks,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum BrowserInvalidation {
    SampleSearch,
    SampleTree,
    ProjectTree,
    PresetTree,
    EffectTrees,
}

#[derive(Debug, Default)]
pub(crate) struct UiInvalidationQueue {
    pending: Mutex<BTreeSet<UiInvalidation>>,
}

impl UiInvalidationQueue {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(&self, invalidation: UiInvalidation) {
        let mut pending = self.pending.lock().unwrap();
        if matches!(invalidation, UiInvalidation::Full(_)) {
            pending.clear();
            pending.insert(invalidation);
            return;
        }
        if pending
            .iter()
            .any(|entry| matches!(entry, UiInvalidation::Full(_)))
        {
            return;
        }
        pending.retain(|entry| !invalidation_supersedes(&invalidation, entry));
        if pending
            .iter()
            .any(|entry| invalidation_supersedes(entry, &invalidation))
        {
            return;
        }
        pending.insert(invalidation);
    }

    pub(crate) fn drain(&self) -> Vec<UiInvalidation> {
        std::mem::take(&mut *self.pending.lock().unwrap())
            .into_iter()
            .collect()
    }

    pub(crate) fn clear(&self) {
        self.pending.lock().unwrap().clear();
    }
}

fn invalidation_supersedes(newer: &UiInvalidation, older: &UiInvalidation) -> bool {
    match (newer, older) {
        (UiInvalidation::Full(_), _) => true,
        (UiInvalidation::TrackTopology(_), UiInvalidation::TrackMixer { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackBusSend { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackRoute { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackParam { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackParamPanel { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::Step { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::StepSelection { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::Instrument { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackFx { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::MidiFx { .. }) => true,
        (
            UiInvalidation::Pattern(PatternInvalidation::AllTracks),
            UiInvalidation::Step { .. } | UiInvalidation::StepSelection { .. },
        ) => true,
        (
            UiInvalidation::Pattern(PatternInvalidation::WholeTrack { track }),
            UiInvalidation::Step {
                track: old_track, ..
            }
            | UiInvalidation::StepSelection { track: old_track },
        ) => track == old_track,
        (
            UiInvalidation::TrackFx {
                track,
                change: TrackFxInvalidation::Topology,
            },
            UiInvalidation::TrackFx {
                track: old_track, ..
            },
        ) => track == old_track,
        (
            UiInvalidation::MidiFx {
                track,
                change: MidiFxInvalidation::Topology,
            },
            UiInvalidation::MidiFx {
                track: old_track, ..
            },
        ) => track == old_track,
        (
            UiInvalidation::BusFx {
                bus,
                change: BusFxInvalidation::Topology,
            },
            UiInvalidation::BusFx { bus: old_bus, .. },
        ) => bus == old_bus,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_invalidation_supersedes_pending_narrow_invalidations() {
        let queue = UiInvalidationQueue::new();
        queue.push(UiInvalidation::TrackMixer {
            track: 3,
            change: TrackMixerInvalidation::Volume,
        });
        queue.push(UiInvalidation::Step {
            track: 3,
            step: 9,
            change: StepInvalidation::Param(StepParamKey::Velocity),
        });
        queue.push(UiInvalidation::Full(FullInvalidation::ProjectLoaded));

        assert_eq!(
            queue.drain(),
            vec![UiInvalidation::Full(FullInvalidation::ProjectLoaded)]
        );
    }

    #[test]
    fn whole_track_pattern_invalidation_supersedes_step_invalidations_for_same_track() {
        let queue = UiInvalidationQueue::new();
        queue.push(UiInvalidation::Step {
            track: 1,
            step: 4,
            change: StepInvalidation::Active,
        });
        queue.push(UiInvalidation::Step {
            track: 2,
            step: 4,
            change: StepInvalidation::Active,
        });
        queue.push(UiInvalidation::Pattern(PatternInvalidation::WholeTrack {
            track: 1,
        }));

        let drained = queue.drain();
        assert!(
            drained.contains(&UiInvalidation::Pattern(PatternInvalidation::WholeTrack {
                track: 1
            }))
        );
        assert!(drained.contains(&UiInvalidation::Step {
            track: 2,
            step: 4,
            change: StepInvalidation::Active,
        }));
        assert!(!drained.contains(&UiInvalidation::Step {
            track: 1,
            step: 4,
            change: StepInvalidation::Active,
        }));
    }
}
