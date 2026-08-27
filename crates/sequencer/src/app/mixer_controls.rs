/*!
Frame drain for sequenced mixer controls (jaki mute/solo routes,
docs/jaki-mixer-control-routes-spec.md §3-§4).

The scheduler lookahead stamps holds into the mailbox on `SequencerState`;
`App::drain_due_mixer_controls` runs once per event-loop frame, releasing
elapsed holds first (OFF before ON at equal times), then engaging due ones
through the same code paths as the mixer buttons — atomics plus mixer param
push — with no undo history. Group names resolve here to the group's backing
bus; unknown targets report a host error and apply nothing.
*/

use crate::mixer_control::{MixerControlOp, MixerControlTarget};

use super::App;

/// A hold's resolved destination. Group names resolve to their backing bus at
/// engage time so a mid-hold rename can never strand an engaged hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MixerControlHoldTarget {
    Track(usize),
    Bus(crate::sequencer::BusId),
}

pub type MixerControlHoldKey = (MixerControlOp, MixerControlHoldTarget);

/// One applied state flip, for the event loop to translate into mixer UI
/// invalidations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixerControlApplied {
    TrackMute { track: usize },
    TrackSolo { track: usize },
    BusMute { bus_index: usize },
    BusSolo { bus_index: usize },
}

#[derive(Debug, Default)]
pub struct MixerControlDrainOutcome {
    pub applied: Vec<MixerControlApplied>,
    pub errors: Vec<String>,
}

impl App {
    /// Drain and apply sequenced mixer controls that have become due at
    /// `rendered_sample`. While the transport is stopped, pending holds are
    /// dropped and everything engaged is released, so nothing stays stuck
    /// muted and no stale hold fires after a restart.
    pub fn drain_due_mixer_controls(&mut self, rendered_sample: u64) -> MixerControlDrainOutcome {
        let mut outcome = MixerControlDrainOutcome::default();
        if !self.state.is_playing() {
            self.state.scheduled_mixer_controls().clear();
            self.release_all_mixer_control_holds(&mut outcome);
            return outcome;
        }

        // Releases first: back-to-back windows stay engaged, and an OFF and
        // an ON due on the same frame land in the documented OFF-then-ON
        // order (spec §4).
        self.release_elapsed_mixer_control_holds(rendered_sample, &mut outcome);

        for control in self
            .state
            .scheduled_mixer_controls()
            .drain_due(rendered_sample)
        {
            let target = match self.resolve_mixer_control_target(&control.target) {
                Ok(target) => target,
                Err(error) => {
                    outcome.errors.push(error);
                    continue;
                }
            };
            let key = (control.op, target);
            match self.mixer_control_holds.get_mut(&key) {
                Some(release) => {
                    // Already engaged: extend to the later release (union of
                    // overlapping windows), no re-push.
                    *release = (*release).max(control.release_sample);
                }
                None => {
                    self.mixer_control_holds
                        .insert(key, control.release_sample);
                    self.apply_mixer_control(key, true, &mut outcome);
                }
            }
        }

        // A hold that fully elapsed within one frame (slow frame, or a very
        // short window) still engaged above in order; release it now.
        self.release_elapsed_mixer_control_holds(rendered_sample, &mut outcome);
        outcome
    }

    fn resolve_mixer_control_target(
        &self,
        target: &MixerControlTarget,
    ) -> Result<MixerControlHoldTarget, String> {
        match target {
            MixerControlTarget::Track(track) => {
                if *track >= self.state.active_track_count() {
                    return Err(format!(
                        "sequenced mixer control: track {track} out of range"
                    ));
                }
                Ok(MixerControlHoldTarget::Track(*track))
            }
            MixerControlTarget::Group(name) => {
                let group = self
                    .groups
                    .iter()
                    .find(|group| group.name == *name)
                    .ok_or_else(|| {
                        format!("sequenced mixer control: unknown group \"{name}\"")
                    })?;
                let bus_id = crate::sequencer::BusId(group.bus_id);
                if !self.buses.iter().any(|channel| channel.id == bus_id) {
                    return Err(format!(
                        "sequenced mixer control: group \"{name}\" has no backing bus"
                    ));
                }
                Ok(MixerControlHoldTarget::Bus(bus_id))
            }
        }
    }

    fn release_elapsed_mixer_control_holds(
        &mut self,
        rendered_sample: u64,
        outcome: &mut MixerControlDrainOutcome,
    ) {
        let elapsed: Vec<MixerControlHoldKey> = self
            .mixer_control_holds
            .iter()
            .filter(|(_, release)| **release <= rendered_sample)
            .map(|(key, _)| *key)
            .collect();
        for key in elapsed {
            self.mixer_control_holds.remove(&key);
            self.apply_mixer_control(key, false, outcome);
        }
    }

    /// Release everything engaged (transport stop / project switch).
    pub fn release_all_mixer_control_holds(&mut self, outcome: &mut MixerControlDrainOutcome) {
        let engaged: Vec<MixerControlHoldKey> =
            self.mixer_control_holds.keys().copied().collect();
        for key in engaged {
            self.mixer_control_holds.remove(&key);
            self.apply_mixer_control(key, false, outcome);
        }
    }

    /// Set one mute/solo flag and push it to the audio graph — the same
    /// application the mixer buttons perform, minus undo history.
    fn apply_mixer_control(
        &mut self,
        key: MixerControlHoldKey,
        engaged: bool,
        outcome: &mut MixerControlDrainOutcome,
    ) {
        let (op, target) = key;
        match target {
            MixerControlHoldTarget::Track(track) => {
                if track >= self.state.active_track_count() {
                    // The track disappeared mid-hold (topology edit); nothing
                    // left to restore.
                    return;
                }
                match op {
                    MixerControlOp::Mute => {
                        self.state.pattern.track_params[track].set_mute(engaged);
                        self.push_track_mute(track);
                        outcome.applied.push(MixerControlApplied::TrackMute { track });
                    }
                    MixerControlOp::Solo => {
                        self.state.pattern.track_params[track].set_solo(engaged);
                        self.push_solo_mutes();
                        outcome.applied.push(MixerControlApplied::TrackSolo { track });
                    }
                }
            }
            MixerControlHoldTarget::Bus(bus_id) => {
                let Some(bus_index) = self
                    .buses
                    .iter()
                    .position(|channel| channel.id == bus_id)
                else {
                    return;
                };
                match op {
                    MixerControlOp::Mute => {
                        self.buses[bus_index].mute = engaged;
                        self.push_bus_mute(bus_id);
                        outcome
                            .applied
                            .push(MixerControlApplied::BusMute { bus_index });
                    }
                    MixerControlOp::Solo => {
                        self.buses[bus_index].solo = engaged;
                        self.push_solo_mutes();
                        outcome
                            .applied
                            .push(MixerControlApplied::BusSolo { bus_index });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::app::{App, AudioBuses};
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{default_empty_effect_chain, SequencerState};

    fn test_app() -> App {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let app = App::new(
            Arc::new(state),
            LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.state.transport.playing.store(true, std::sync::atomic::Ordering::Relaxed);
        app
    }

    fn push(
        app: &App,
        engage: u64,
        release: u64,
        op: MixerControlOp,
        target: MixerControlTarget,
    ) {
        app.state
            .scheduled_mixer_controls()
            .push(engage, release, 0, op, target);
    }

    #[test]
    fn track_mute_hold_engages_and_releases() {
        let mut app = test_app();
        push(&app, 100, 200, MixerControlOp::Mute, MixerControlTarget::Track(1));

        // Not yet due: nothing applies.
        let outcome = app.drain_due_mixer_controls(50);
        assert!(outcome.applied.is_empty());
        assert!(!app.state.pattern.track_params[1].is_muted());

        let outcome = app.drain_due_mixer_controls(100);
        assert_eq!(outcome.applied, vec![MixerControlApplied::TrackMute { track: 1 }]);
        assert!(app.state.pattern.track_params[1].is_muted());

        let outcome = app.drain_due_mixer_controls(200);
        assert_eq!(outcome.applied, vec![MixerControlApplied::TrackMute { track: 1 }]);
        assert!(!app.state.pattern.track_params[1].is_muted());
    }

    #[test]
    fn track_solo_hold_engages_and_releases() {
        let mut app = test_app();
        push(&app, 0, 10, MixerControlOp::Solo, MixerControlTarget::Track(0));

        app.drain_due_mixer_controls(0);
        assert!(app.state.pattern.track_params[0].is_solo());
        app.drain_due_mixer_controls(10);
        assert!(!app.state.pattern.track_params[0].is_solo());
    }

    fn add_group(app: &mut App, name: &str, bus_id: u64) {
        app.groups.push(crate::project::ProjectTrackGroup {
            id: 7,
            name: name.to_string(),
            color: [0.0; 3],
            collapsed: false,
            members: vec![0, 1],
            bus_id,
            rack: None,
            rack_members: Vec::new(),
        });
    }

    #[test]
    fn group_mute_and_solo_holds_drive_the_backing_bus() {
        let mut app = test_app();
        let bus_id = app.buses[1].id;
        add_group(&mut app, "Drums", bus_id.0);

        push(
            &app, 0, 10,
            MixerControlOp::Mute,
            MixerControlTarget::Group("Drums".to_string()),
        );
        push(
            &app, 0, 20,
            MixerControlOp::Solo,
            MixerControlTarget::Group("Drums".to_string()),
        );

        let outcome = app.drain_due_mixer_controls(0);
        assert_eq!(
            outcome.applied,
            vec![
                MixerControlApplied::BusMute { bus_index: 1 },
                MixerControlApplied::BusSolo { bus_index: 1 },
            ]
        );
        assert!(app.buses[1].mute);
        assert!(app.buses[1].solo);

        app.drain_due_mixer_controls(10);
        assert!(!app.buses[1].mute);
        assert!(app.buses[1].solo, "solo hold is independent of the mute hold");
        app.drain_due_mixer_controls(20);
        assert!(!app.buses[1].solo);
    }

    #[test]
    fn invalid_targets_report_and_apply_nothing() {
        let mut app = test_app();
        push(&app, 0, 10, MixerControlOp::Mute, MixerControlTarget::Track(99));
        push(
            &app, 0, 10,
            MixerControlOp::Solo,
            MixerControlTarget::Group("Nope".to_string()),
        );

        let outcome = app.drain_due_mixer_controls(0);
        assert!(outcome.applied.is_empty());
        assert_eq!(outcome.errors.len(), 2);
        assert!(outcome.errors[0].contains("track 99 out of range"));
        assert!(outcome.errors[1].contains("unknown group \"Nope\""));
        assert!(app.mixer_control_holds.is_empty());
    }

    #[test]
    fn overlapping_holds_union_and_extend_the_release() {
        let mut app = test_app();
        push(&app, 0, 100, MixerControlOp::Mute, MixerControlTarget::Track(0));
        push(&app, 50, 200, MixerControlOp::Mute, MixerControlTarget::Track(0));

        let outcome = app.drain_due_mixer_controls(60);
        // One engage, no re-push for the overlapping second hold.
        assert_eq!(outcome.applied.len(), 1);
        assert!(app.state.pattern.track_params[0].is_muted());

        // Past the first hold's release but inside the extension.
        let outcome = app.drain_due_mixer_controls(150);
        assert!(outcome.applied.is_empty());
        assert!(app.state.pattern.track_params[0].is_muted());

        app.drain_due_mixer_controls(200);
        assert!(!app.state.pattern.track_params[0].is_muted());
    }

    #[test]
    fn back_to_back_windows_release_before_engaging() {
        let mut app = test_app();
        push(&app, 0, 100, MixerControlOp::Mute, MixerControlTarget::Track(0));
        app.drain_due_mixer_controls(0);
        push(&app, 100, 200, MixerControlOp::Mute, MixerControlTarget::Track(0));

        // Release of the first and engage of the second are both due: the
        // documented OFF-then-ON order leaves the track muted.
        let outcome = app.drain_due_mixer_controls(100);
        assert_eq!(outcome.applied.len(), 2);
        assert!(app.state.pattern.track_params[0].is_muted());
        app.drain_due_mixer_controls(200);
        assert!(!app.state.pattern.track_params[0].is_muted());
    }

    #[test]
    fn transport_stop_drops_pending_and_releases_engaged_holds() {
        let mut app = test_app();
        push(&app, 0, 1_000, MixerControlOp::Mute, MixerControlTarget::Track(0));
        push(&app, 500, 1_000, MixerControlOp::Solo, MixerControlTarget::Track(1));
        app.drain_due_mixer_controls(0);
        assert!(app.state.pattern.track_params[0].is_muted());

        app.state
            .transport
            .playing
            .store(false, std::sync::atomic::Ordering::Relaxed);
        app.drain_due_mixer_controls(10);
        assert!(!app.state.pattern.track_params[0].is_muted());
        assert!(app.mixer_control_holds.is_empty());
        assert_eq!(app.state.scheduled_mixer_controls().pending_len(), 0);
        // The dropped pending solo never fires after a restart.
        app.state
            .transport
            .playing
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let outcome = app.drain_due_mixer_controls(600);
        assert!(outcome.applied.is_empty());
        assert!(!app.state.pattern.track_params[1].is_solo());
    }
}
