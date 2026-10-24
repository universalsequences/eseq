//! Project-owned sequencers on/off per bank (docs/rig-spec.md §10,
//! eseq-ka5t.1/.5) at the scheduler: an instance the playing scene parks
//! does not tick, and the process slots it owns (`:owner`) do not apply; the
//! same instance runs again in a scene that does not park it.

use super::{
    schedule_playing_lookahead, LiveMidiFxTrackState, SchedulerLookaheadState,
};
use crate::effects::EffectDescriptor;
use crate::lisp_host;
use crate::scheduled_event::{ScheduledEventKind, ScheduledEventQueue};
use crate::sequencer::{default_empty_effect_chain, SequencerState, MAX_TRACKS};
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn on_scheduler_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .name("instance-parking-scheduler-harness".to_string())
        .stack_size(super::SCHEDULER_THREAD_STACK_SIZE)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("instance parking harness panicked")
}

/// Two scenes; `parked_in` lists the scenes that park the generator. Plays
/// scene `playing` for a bar and returns the hits the project-owned `:4`
/// generator scheduled.
fn project_generator_hits(parked_in: &'static [usize], playing: usize) -> usize {
    on_scheduler_stack(move || {
        let state = Arc::new(SequencerState::new(2, vec![default_empty_effect_chain(), default_empty_effect_chain()]));
        state.replace_pattern_repository(
            vec![
                crate::sequencer::PatternSnapshot::new_default(2, &[]),
                crate::sequencer::PatternSnapshot::new_default(2, &[]),
            ],
            playing,
        );
        let mut scratch = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(); 2],
            vec![EffectDescriptor::builtin_sampler(); 2],
            0,
            0,
        );
        scratch
            .eval(
                r#"(__register-sequencer "conductor"
                     :resolution :4
                     :tick (lambda () (seq-emit :track 0 :at :now)))"#,
            )
            .expect("register the generator");
        let defs = scratch.sequencer_defs();
        let id = defs[0].id;
        state.with_scenes_mut(|scenes| {
            assert_eq!(scenes.scenes.len(), 2);
            scenes.current_scene = playing;
            for scene in parked_in {
                scenes.set_parked_in_scenes(crate::sequencer::ParkedSet::Instances, id, true, *scene..*scene + 1);
            }
        });
        state.transport.playing.store(true, Ordering::Relaxed);
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        scheduler.generator_runtime.sync_definitions(&defs, 0.0);
        let mut scratch_runtime = Some(scratch);
        let snapshot = state.publish_scheduler_snapshot();
        assert_eq!(snapshot.instance_parked(id), parked_in.contains(&playing));
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
            if let ScheduledEventKind::NetworkTrigger { .. } = event.kind {
                hits += 1;
            }
        }
        hits
    })
}

#[test]
fn a_parked_project_sequencer_does_not_fire_in_bank_a_and_fires_in_bank_b() {
    assert!(project_generator_hits(&[], 0) >= 4, "on everywhere: one hit a beat");
    assert_eq!(project_generator_hits(&[0], 0), 0, "parked in the playing scene: silent");
    assert!(project_generator_hits(&[0], 1) >= 4, "another scene that plays it: runs");
}

/// A process declared with `:owner` is disabled in the snapshot of a scene
/// that parks its owner (and in a single-track recapture), on again when the
/// owner is on; a slot with no owner never is.
#[test]
fn an_owned_process_is_inert_while_its_owner_is_parked() {
    const OWNER: u64 = 12;
    let state = Arc::new(SequencerState::new(2, vec![default_empty_effect_chain(), default_empty_effect_chain()]));
    let mut scratch = lisp_host::ScratchControlRuntime::new(
        Arc::clone(&state),
        vec![Vec::new(); 2],
        vec![EffectDescriptor::builtin_sampler(); 2],
        0,
        0,
    );
    scratch
        .eval(
            r#"
            (def-accumulator owned-climb
              :target (step-param :transpose)
              :amount (amount :lane true :default 0)
              :range (-128 128)
              :mode :clip)
            (processes :track 0 :owner 12 (owned-climb :amount (lane 0 1)))
            (processes :track 1 (owned-climb :amount (lane 0 1)))
            "#,
        )
        .expect("attach the chains");
    let chain = state.track_process_chain(0).expect("track 0 chain");
    assert_eq!(chain.slots[0].owner_instance, Some(OWNER), ":owner tags the slot");
    assert_eq!(state.track_process_chain(1).unwrap().slots[0].owner_instance, None);
    let enabled = |snapshot: &crate::sequencer::SequencerSnapshot, track: usize| -> bool {
        snapshot.tracks[track]
            .process_chain
            .slots
            .iter()
            .find(|slot| slot.class_name == "owned-climb")
            .expect("the slot")
            .enabled
    };
    let snapshot = state.publish_scheduler_snapshot();
    assert!(enabled(&snapshot, 0) && enabled(&snapshot, 1));
    state.with_scenes_mut(|scenes| scenes.set_parked_in_scenes(crate::sequencer::ParkedSet::Instances, OWNER, true, 0..1));
    let snapshot = state.publish_scheduler_snapshot();
    assert!(!enabled(&snapshot, 0), "the parked owner's process is inert");
    assert!(enabled(&snapshot, 1), "an unowned process is never gated");
    let recaptured = state.publish_scheduler_track(0);
    assert!(!enabled(&recaptured, 0), "a single-track recapture stays gated");
    assert!(
        state.track_process_chain(0).unwrap().slots[0].enabled,
        "the live chain (what saves) is left alone"
    );
    // The owner tag survives the project file.
    let encoded = serde_json::to_string(&state.track_process_chain(0).unwrap()).unwrap();
    assert!(encoded.contains("\"owner_instance\":12"), "{encoded}");
    // A rack-owned instance follows its kit: a stale entry never gates it.
    state.with_scenes_mut(|scenes| {
        scenes.instance_racks.insert(OWNER, 3);
    });
    let snapshot = state.publish_scheduler_snapshot();
    assert!(enabled(&snapshot, 0));
    state.with_scenes_mut(|scenes| {
        scenes.instance_racks.remove(&OWNER);
        scenes.set_parked_in_scenes(crate::sequencer::ParkedSet::Instances, OWNER, false, 0..1)
    });
    assert!(enabled(&state.publish_scheduler_snapshot(), 0), "on again");
}
