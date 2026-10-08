use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// The same targeted step change landed on several cells of one track.
    /// Keeping it as one queue entry lets high-rate writers share derived
    /// p-lock/render work and take the selection lock once per frame.
    StepInvalidationBatch {
        track: usize,
        steps: Vec<usize>,
        change: StepInvalidation,
    },
    /// Structural state for several cells changed in one mutation. Applying
    /// this as a batch shares selection locks and derived plock/color lanes.
    StepBatch {
        track: usize,
        steps: Vec<usize>,
    },
    StepSelection {
        track: usize,
        /// Step indexes whose membership in the selection changed.
        changed_steps: Vec<usize>,
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
    ProcessLaneValues {
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
    Retrig,
    RetrigRate,
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
            StepParam::Retrig => Self::Retrig,
            StepParam::RetrigRate => Self::RetrigRate,
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
            Self::Retrig => StepParam::Retrig,
            Self::RetrigRate => StepParam::RetrigRate,
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
    MonoTrigger,
    VoicePriority,
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
    /// Host-owned metadata shown by an existing effect panel changed without
    /// changing the bus chain itself (for example a loaded table/IR name).
    PanelTree,
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

#[derive(Debug)]
pub(crate) struct UiInvalidationQueue {
    pending: Mutex<BTreeSet<UiInvalidation>>,
    /// P-lock revisions for readers that do not drain the queue (the host
    /// kinds' step p-lock render and `has-locks`): moved by an invalidation
    /// that may move a track's p-locks ([`UiInvalidation::plock_scope`]).
    plocks: TrackGenerations,
    /// Process chain revisions, likewise (the host kinds' process lanes):
    /// moved by an invalidation that may move a track's process chain or
    /// lane values ([`UiInvalidation::process_scope`]).
    processes: TrackGenerations,
}

/// Per-track revisions: `all` moves on an invalidation that may move every
/// track's, `tracks[t]` on one that may move track `t`'s.
#[derive(Debug)]
struct TrackGenerations {
    all: AtomicU64,
    tracks: [AtomicU64; sequencer::sequencer::MAX_TRACKS],
}

impl Default for TrackGenerations {
    fn default() -> Self {
        Self {
            all: AtomicU64::new(0),
            tracks: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl TrackGenerations {
    /// Track `track`'s revision.
    fn of(&self, track: usize) -> u64 {
        let all = self.all.load(Ordering::Relaxed);
        let own = self
            .tracks
            .get(track)
            .map_or(0, |generation| generation.load(Ordering::Relaxed));
        all.wrapping_add(own)
    }

    /// Move the revisions `scope` names (a track past the array moves all).
    fn bump(&self, scope: Option<PlockScope>) {
        let generation = match scope {
            Some(PlockScope::AllTracks) => &self.all,
            Some(PlockScope::Track(track)) => self.tracks.get(track).unwrap_or(&self.all),
            None => return,
        };
        generation.fetch_add(1, Ordering::Relaxed);
    }
}

/// Which tracks' p-locks an invalidation may have moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlockScope {
    AllTracks,
    Track(usize),
}

impl UiInvalidation {
    /// The tracks whose p-locks (any family: device params, sends, step
    /// params, process lanes) or pattern shape this invalidation may have
    /// moved; `None` for one that cannot move a p-lock (mixer, selection,
    /// meters, a device's base value, …).
    pub(crate) fn plock_scope(&self) -> Option<PlockScope> {
        use PlockScope::{AllTracks, Track};
        Some(match self {
            Self::Full(_) | Self::ProjectState | Self::Pattern(PatternInvalidation::AllTracks) => {
                AllTracks
            }
            Self::TrackTopology(TrackTopologyInvalidation::InstrumentType { track }) => {
                Track(*track)
            }
            Self::TrackTopology(TrackTopologyInvalidation::TracksAddedRemovedOrReordered) => {
                AllTracks
            }
            Self::Pattern(
                PatternInvalidation::WholeTrack { track }
                | PatternInvalidation::TrackLength { track },
            ) => Track(*track),
            Self::Step { track, change, .. }
            | Self::StepInvalidationBatch { track, change, .. }
                if *change != StepInvalidation::Selected =>
            {
                Track(*track)
            }
            Self::StepBatch { track, .. }
            | Self::TrackBusSend { track, .. }
            | Self::ProcessLaneValues { track }
            | Self::ProcessChain { track }
            | Self::MidiFx { track, .. }
            | Self::TrackFx {
                track,
                change:
                    TrackFxInvalidation::Plock { .. }
                    | TrackFxInvalidation::Topology
                    | TrackFxInvalidation::PanelTree,
            }
            | Self::Instrument {
                track,
                change: InstrumentInvalidation::Plock { .. } | InstrumentInvalidation::PanelTopology,
            }
            | Self::TrackParam {
                track,
                change:
                    TrackParamInvalidation::Plocks
                    | TrackParamInvalidation::BusSends
                    | TrackParamInvalidation::NumSteps,
            }
            | Self::Sidebar {
                track,
                change: SidebarInvalidation::Plocks,
            } => Track(*track),
            _ => return None,
        })
    }

    /// The tracks whose process chain (slots, lanes, inlets, bindings) this
    /// invalidation may have moved; `None` for one that cannot.
    pub(crate) fn process_scope(&self) -> Option<PlockScope> {
        use PlockScope::{AllTracks, Track};
        Some(match self {
            Self::Full(_)
            | Self::ProjectState
            | Self::Pattern(PatternInvalidation::AllTracks)
            | Self::TrackTopology(TrackTopologyInvalidation::TracksAddedRemovedOrReordered) => {
                AllTracks
            }
            Self::Pattern(PatternInvalidation::WholeTrack { track })
            | Self::ProcessLaneValues { track }
            | Self::ProcessChain { track } => Track(*track),
            _ => return None,
        })
    }
}

impl Default for UiInvalidationQueue {
    fn default() -> Self {
        Self {
            pending: Mutex::default(),
            plocks: TrackGenerations::default(),
            processes: TrackGenerations::default(),
        }
    }
}

impl UiInvalidationQueue {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Track `track`'s p-lock revision: moves whenever an invalidation that
    /// may move its p-locks is pushed ([`UiInvalidation::plock_scope`]).
    pub(crate) fn plock_generation(&self, track: usize) -> u64 {
        self.plocks.of(track)
    }

    /// Track `track`'s process chain revision: moves whenever an
    /// invalidation that may move its process chain is pushed
    /// ([`UiInvalidation::process_scope`]).
    pub(crate) fn process_generation(&self, track: usize) -> u64 {
        self.processes.of(track)
    }

    pub(crate) fn push(&self, invalidation: UiInvalidation) {
        self.plocks.bump(invalidation.plock_scope());
        self.processes.bump(invalidation.process_scope());
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
        if let UiInvalidation::StepInvalidationBatch {
            track,
            steps,
            change,
        } = invalidation
        {
            if steps.is_empty() {
                return;
            }
            let batch = UiInvalidation::StepInvalidationBatch {
                track,
                steps: steps.clone(),
                change: change.clone(),
            };
            if pending
                .iter()
                .any(|entry| invalidation_supersedes(entry, &batch))
            {
                return;
            }
            let previous = pending.iter().find_map(|entry| match entry {
                UiInvalidation::StepInvalidationBatch {
                    track: queued_track,
                    steps,
                    change: queued_change,
                } if *queued_track == track && *queued_change == change => Some(steps.clone()),
                _ => None,
            });
            if previous.is_some() {
                pending.retain(|entry| {
                    !matches!(entry, UiInvalidation::StepInvalidationBatch {
                        track: queued_track,
                        change: queued_change,
                        ..
                    } if *queued_track == track && *queued_change == change)
                });
            }
            let mut combined = previous
                .into_iter()
                .flatten()
                .chain(steps)
                .collect::<Vec<_>>();
            combined.sort_unstable();
            combined.dedup();
            pending.insert(UiInvalidation::StepInvalidationBatch {
                track,
                steps: combined,
                change,
            });
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
        (UiInvalidation::ProcessChain { track }, UiInvalidation::ProcessLaneValues { track: other }) =>
            track == other,
        (UiInvalidation::TrackTopology(_), UiInvalidation::TrackMixer { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackBusSend { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackRoute { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackParam { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackParamPanel { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::ProcessChain { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::ProcessLaneValues { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::Step { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::StepInvalidationBatch { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::StepSelection { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::Instrument { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::TrackFx { .. })
        | (UiInvalidation::TrackTopology(_), UiInvalidation::MidiFx { .. }) => true,
        (
            UiInvalidation::Pattern(PatternInvalidation::AllTracks),
            UiInvalidation::Step { .. }
            | UiInvalidation::StepInvalidationBatch { .. }
            | UiInvalidation::StepSelection { .. },
        ) => true,
        (
            UiInvalidation::Pattern(PatternInvalidation::WholeTrack { track }),
            UiInvalidation::Step {
                track: old_track, ..
            }
            | UiInvalidation::StepInvalidationBatch {
                track: old_track, ..
            }
            | UiInvalidation::StepSelection {
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
    fn step_invalidation_batches_merge_steps_per_track_and_change() {
        let queue = UiInvalidationQueue::new();
        queue.push(UiInvalidation::StepInvalidationBatch {
            track: 2,
            steps: vec![7, 3],
            change: StepInvalidation::PlockPresence,
        });
        queue.push(UiInvalidation::StepInvalidationBatch {
            track: 2,
            steps: vec![5, 3],
            change: StepInvalidation::PlockPresence,
        });

        assert_eq!(
            queue.drain(),
            vec![UiInvalidation::StepInvalidationBatch {
                track: 2,
                steps: vec![3, 5, 7],
                change: StepInvalidation::PlockPresence,
            }]
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
