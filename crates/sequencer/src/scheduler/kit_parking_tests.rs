//! Kits on/off per bank (docs/rig-spec.md §6, eseq-ka5t.13) at the scheduler:
//! a kit the playing scene parks gets no pattern triggers on its pads and its
//! rack-owned sequencers do not run; the same kit plays again once it is on.

use super::{
    schedule_playing_lookahead, LiveMidiFxTrackState, SchedulerLookaheadState,
    SnapshotSequencerClock,
};
use crate::effects::EffectDescriptor;
use crate::lisp_host;
use crate::scheduled_event::{ScheduledEventKind, ScheduledEventQueue};
use crate::sequencer::{default_empty_effect_chain, SequencerState, MAX_TRACKS};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const KIT: u64 = 77;

fn on_scheduler_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .name("kit-parking-scheduler-harness".to_string())
        .stack_size(super::SCHEDULER_THREAD_STACK_SIZE)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("kit parking harness panicked")
}

fn park(state: &SequencerState, parked: bool) {
    state.with_scenes_mut(|scenes| {
        let count = scenes.scenes.len();
        scenes.set_kit_parked_in_scenes(KIT, parked, 0..count)
    });
}

/// Tracks a beat of pattern steps triggers on: track 0 plays alone, track 1
/// is the kit's only pad.
fn step_trigger_tracks(parked: bool) -> Vec<usize> {
    let state = SequencerState::new(2, vec![default_empty_effect_chain(), default_empty_effect_chain()]);
    state.set_rack_memberships(vec![crate::graph::RackMembership {
        group_id: KIT,
        members: vec![1],
    }]);
    state.toggle_play();
    for track in 0..2 {
        for step in [0, 4, 8, 12] {
            state.toggle_step_and_clear_plocks(track, step);
        }
    }
    park(&state, parked);
    let snapshot = state.publish_scheduler_snapshot();
    assert_eq!(snapshot.rack_parked(KIT), parked);
    assert_eq!(snapshot.tracks[1].scene_silenced, parked);
    assert!(!snapshot.tracks[0].scene_silenced, "a loose track is never parked");
    let mut clock = SnapshotSequencerClock::new(48_000);
    let mut tracks = Vec::new();
    while clock.total_beats < 4.0 {
        tracks.extend(clock.process_chunk(512, &snapshot, &state).into_iter().map(|t| t.track));
    }
    tracks.sort_unstable();
    tracks.dedup();
    tracks
}

#[test]
fn a_parked_kit_gets_no_pattern_triggers_and_plays_again_when_on() {
    assert_eq!(step_trigger_tracks(false), vec![0, 1]);
    assert_eq!(step_trigger_tracks(true), vec![0], "the parked pad stays silent");
}

#[test]
fn a_single_track_publish_keeps_the_kit_parked() {
    let state = SequencerState::new(2, vec![default_empty_effect_chain(), default_empty_effect_chain()]);
    state.set_rack_memberships(vec![crate::graph::RackMembership {
        group_id: KIT,
        members: vec![1],
    }]);
    park(&state, true);
    state.publish_scheduler_snapshot();
    let snapshot = state.publish_scheduler_track(1);
    assert!(snapshot.tracks[1].scene_silenced, "the recaptured pad is still parked");
    assert!(snapshot.rack_parked(KIT));
}

/// Hits a rack-owned `:4` generator (member 0 = track 1) schedules over one
/// bar.
fn rack_generator_hits(parked: bool) -> usize {
    on_scheduler_stack(move || {
        let state = Arc::new(SequencerState::new(2, vec![default_empty_effect_chain(), default_empty_effect_chain()]));
        state.set_rack_memberships(vec![crate::graph::RackMembership {
            group_id: KIT,
            members: vec![1],
        }]);
        let mut scratch = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(); 2],
            vec![EffectDescriptor::builtin_sampler(); 2],
            0,
            0,
        );
        scratch
            .eval(
                r#"(__register-sequencer "kit-pulse"
                     :resolution :4
                     :tick (lambda () (seq-emit :track 0 :at :now)))"#,
            )
            .expect("register the generator");
        park(&state, parked);
        state.transport.playing.store(true, Ordering::Relaxed);
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        let defs = scratch.sequencer_defs();
        scheduler.generator_runtime.sync_definitions(&defs, 0.0);
        scheduler.generator_owner_racks.insert(defs[0].id, KIT);
        let mut scratch_runtime = Some(scratch);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<64>::new();
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            96_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );
        let mut hits = 0;
        while let Some(event) = queue.pop_owned() {
            if let ScheduledEventKind::NetworkTrigger { track, .. } = event.kind {
                assert_eq!(track, 1, "member 0 is track 1");
                hits += 1;
            }
        }
        hits
    })
}

#[test]
fn a_parked_kits_rack_owned_sequencer_does_not_fire() {
    assert!(rack_generator_hits(false) >= 4, "on: one hit a beat");
    assert_eq!(rack_generator_hits(true), 0, "off: the generator does not run");
}
