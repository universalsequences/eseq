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
        /// Step indexes whose membership in the selection changed.
        changed_steps: Vec<usize>,
    },
    ExpandedStepViewport {
        track: usize,
        track_id: usize,
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
    ProcessChain {
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
    RecordArm,
    Output,
    Collapsed,
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
    MuteGroup,
    GlobalTranspose,
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
    Plock { param: usize },
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
        if let UiInvalidation::StepSelection {
            track,
            changed_steps,
        } = invalidation
        {
            let selection_invalidation = UiInvalidation::StepSelection {
                track,
                changed_steps: changed_steps.clone(),
            };
            if pending
                .iter()
                .any(|entry| invalidation_supersedes(entry, &selection_invalidation))
            {
                return;
            }
            let previous = pending.iter().find_map(|entry| match entry {
                UiInvalidation::StepSelection {
                    track: queued_track,
                    changed_steps,
                } if *queued_track == track => Some(changed_steps.clone()),
                _ => None,
            });
            if let Some(previous) = previous {
                pending.retain(|entry| {
                    !matches!(entry, UiInvalidation::StepSelection { track: queued_track, .. } if *queued_track == track)
                });
                let mut combined = previous
                    .into_iter()
                    .chain(changed_steps)
                    .collect::<Vec<_>>();
                combined.sort_unstable();
                combined.dedup();
                pending.insert(UiInvalidation::StepSelection {
                    track,
                    changed_steps: combined,
                });
            } else {
                pending.insert(UiInvalidation::StepSelection {
                    track,
                    changed_steps,
                });
            }
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
        | (UiInvalidation::TrackTopology(_), UiInvalidation::ProcessChain { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::Step { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::StepSelection { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::ExpandedStepViewport { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::Instrument { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackFx { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::MidiFx { .. }) => true,
        (
            UiInvalidation::Pattern(PatternInvalidation::AllTracks),
            UiInvalidation::Step { .. }
            | UiInvalidation::StepSelection { .. }
            | UiInvalidation::ExpandedStepViewport { .. },
        ) => true,
        (
            UiInvalidation::Pattern(PatternInvalidation::WholeTrack { track }),
            UiInvalidation::Step {
                track: old_track, ..
            }
            | UiInvalidation::StepSelection {
                track: old_track, ..
            }
            | UiInvalidation::ExpandedStepViewport {
                track: old_track, ..
            },
        ) => track == old_track,
        (
            UiInvalidation::PianoRoll {
                track,
                change: PianoRollInvalidation::Items,
            },
            UiInvalidation::PianoRoll {
                track: old_track, ..
            },
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

    #[test]
    fn piano_roll_item_invalidation_supersedes_selection_for_same_track() {
        let queue = UiInvalidationQueue::new();
        queue.push(UiInvalidation::PianoRoll {
            track: 1,
            change: PianoRollInvalidation::Selection,
        });
        queue.push(UiInvalidation::PianoRoll {
            track: 1,
            change: PianoRollInvalidation::Items,
        });

        assert_eq!(
            queue.drain(),
            vec![UiInvalidation::PianoRoll {
                track: 1,
                change: PianoRollInvalidation::Items,
            }]
        );

        queue.push(UiInvalidation::PianoRoll {
            track: 1,
            change: PianoRollInvalidation::Items,
        });
        queue.push(UiInvalidation::PianoRoll {
            track: 1,
            change: PianoRollInvalidation::Selection,
        });

        assert_eq!(
            queue.drain(),
            vec![UiInvalidation::PianoRoll {
                track: 1,
                change: PianoRollInvalidation::Items,
            }]
        );
    }

    #[test]
    fn step_selection_invalidations_merge_changed_steps_per_track() {
        let queue = UiInvalidationQueue::new();
        queue.push(UiInvalidation::StepSelection {
            track: 2,
            changed_steps: vec![7, 3],
        });
        queue.push(UiInvalidation::StepSelection {
            track: 2,
            changed_steps: vec![5, 3],
        });
        queue.push(UiInvalidation::StepSelection {
            track: 4,
            changed_steps: vec![1],
        });

        assert_eq!(
            queue.drain(),
            vec![
                UiInvalidation::StepSelection {
                    track: 2,
                    changed_steps: vec![3, 5, 7],
                },
                UiInvalidation::StepSelection {
                    track: 4,
                    changed_steps: vec![1],
                },
            ]
        );
    }

    #[test]
    fn whole_track_invalidation_suppresses_later_step_selection_delta() {
        let queue = UiInvalidationQueue::new();
        queue.push(UiInvalidation::Pattern(PatternInvalidation::WholeTrack {
            track: 1,
        }));
        queue.push(UiInvalidation::StepSelection {
            track: 1,
            changed_steps: vec![2, 3],
        });

        assert_eq!(
            queue.drain(),
            vec![UiInvalidation::Pattern(PatternInvalidation::WholeTrack {
                track: 1,
            })]
        );
    }
}
