//! The factory sequencer's compact grid, its track and group headers and the
//! rack clip run, ported to the kinds (kind-bindings spec §13 stage 8,
//! eseq-0l17.11). The expanded step editor, its process lanes, the lane
//! patchbay and the pad grid: `sequencer_editor` (eseq-0l17.66).

use super::views::{assert_ported, distro, instance_bindings};
use super::*;

/// The fully ported files' sources.
const PORTED: [(&str, &str); 7] = [
    (
        "ui/sequencer.lisp",
        include_str!("../../../../../../content/ui/sequencer.lisp"),
    ),
    (
        "ui/track-collapse.lisp",
        include_str!("../../../../../../content/ui/track-collapse.lisp"),
    ),
    (
        "ui/sequencer-keys.lisp",
        include_str!("../../../../../../content/ui/sequencer-keys.lisp"),
    ),
    (
        "ui/seqv-track-params.lisp",
        include_str!("../../../../../../content/ui/seqv-track-params.lisp"),
    ),
    (
        "ui/step-grid-interactions.lisp",
        include_str!("../../../../../../content/ui/step-grid-interactions.lisp"),
    ),
    (
        "ui/seq-grid-mode.lisp",
        include_str!("../../../../../../content/ui/seq-grid-mode.lisp"),
    ),
    (
        "ui/seq-panels.lisp",
        include_str!("../../../../../../content/ui/seq-panels.lisp"),
    ),
];

#[test]
fn ported_sequencer_files_use_no_legacy_binding_forms() {
    assert_ported(&PORTED);
}

/// The step cells take their step and track, the playhead bars their track,
/// the headers bind arm, mute, solo, audibility, fader and meter: playback
/// repaints the grid and never re-renders it.
#[test]
fn the_grid_binds_its_host_state_through_kinds_and_only_repaints_on_playback() {
    let mut h = distro();
    let (tree, revision) = h.buffer_tree("*sequencer*");
    let (mut bound, mut legacy) = (Vec::new(), Vec::new());
    instance_bindings(&tree, &mut bound, &mut legacy);
    assert_eq!(legacy, Vec::<String>::new(), "legacy bindings");
    let t0 = h.track_id(0);
    for field in [
        "armed",
        "muted",
        "soloed",
        "audible",
        "volume",
        "peak",
        "governed",
        "in-selection",
        "color",
        "playhead",
        "length-step",
    ] {
        assert!(
            bound.contains(&(t0, field.to_string())),
            "track 0's {field} is bound: {bound:?}"
        );
    }
    let steps = h.steps_of(t0);
    assert!(!steps.is_empty());
    for step in &steps {
        // `held-pad`: held and the pad variant packed into one shader float.
        for field in ["active", "selected", "held-pad", "lock-kind", "variant-color"] {
            assert!(
                bound.contains(&(*step, field.to_string())),
                "step {step:?}'s {field} is bound"
            );
        }
    }

    h.set_playing(true);
    for step in 0..4 {
        h.shared.state.transport.track_playheads[0].store(step, Ordering::Relaxed);
        h.sync();
        h.show_all();
        assert_eq!(h.track_field(0, "playhead"), Value::Number(step as f64));
    }
    assert_eq!(
        h.buffer_tree("*sequencer*").1,
        revision,
        "playback only repaints"
    );
}

/// The playhead turning onto a grid's second row lights that row's number by
/// repaint (the row lamp binds `track.playhead`): no row re-renders.
#[test]
fn a_playhead_page_turn_only_repaints_the_row_numbers() {
    let mut h = distro();
    h.shared.state.pattern.track_params[0].set_num_steps(32);
    h.sync();
    h.show_all();
    h.set_playing(true);
    h.sync();
    h.show_all();
    let revision = h.buffer_tree("*sequencer*").1;
    for step in [14, 15, 16, 17, 20] {
        h.shared.state.transport.track_playheads[0].store(step, Ordering::Relaxed);
        h.sync();
        h.show_all();
    }
    assert_eq!(h.track_field(0, "playhead-page"), Value::Number(1.0));
    assert_eq!(
        h.buffer_tree("*sequencer*").1,
        revision,
        "a page turn only repaints"
    );
}

/// `track.playhead-page` is the playhead's 16-step page and
/// `track.length-step` the step a length lane set the length to, both while
/// the transport plays (-1 otherwise).
#[test]
fn the_playhead_page_and_the_length_lane_step_follow_the_transport() {
    let mut h = distro();
    h.shared.state.pattern.track_params[0].set_num_steps(32);
    let read = |h: &mut Harness| {
        (
            h.track_field(0, "playhead-page"),
            h.track_field(0, "length-step"),
        )
    };
    h.sync();
    assert_eq!(read(&mut h), (Value::Number(-1.0), Value::Number(-1.0)));
    let transport = &h.shared.state.transport;
    h.set_playing(true);
    transport.track_playheads[0].store(20, Ordering::Relaxed);
    transport.track_process_lengths[0].store(12, Ordering::Relaxed);
    h.sync();
    assert_eq!(read(&mut h), (Value::Number(1.0), Value::Number(11.0)));
    h.set_playing(false);
    h.sync();
    assert_eq!(read(&mut h), (Value::Number(-1.0), Value::Number(-1.0)));
}

/// A track's `expanded` is view state the sequencer keeps on the track:
/// toggling one re-renders that row's subtree alone.
#[test]
fn expanding_a_track_rerenders_its_row_alone() {
    let mut h = distro();
    let revision = h.buffer_tree("*sequencer*").1;
    let before = h.editor.runtime().ui_work_counters();
    h.eval("(eseq.sequencer/set-track-expanded (track 0) true)");
    h.show_all();
    let after = h.editor.runtime().ui_work_counters();
    assert_ne!(
        h.buffer_tree("*sequencer*").1,
        revision,
        "the row re-renders"
    );
    assert_eq!(
        after.full_buffer_reruns - before.full_buffer_reruns,
        1,
        "only the inert `*seq-expand-sync*` projection re-runs whole"
    );
    assert!(
        after.subtree_reruns - before.subtree_reruns <= 2,
        "the row (and its header) re-render: {before:?} -> {after:?}"
    );
    assert_eq!(
        h.eval("(len (eseq.sequencer/expanded-tracks))"),
        Value::Number(1.0)
    );
    assert_eq!(h.track_field(0, "expanded"), Value::Bool(true));
}

/// The expanded editors follow their track by its stable id: deleting the
/// expanded first track hands its position to the next track, collapsed,
/// and the undo brings the expansion and the param mode back.
#[test]
fn an_expanded_track_keeps_its_editor_through_a_delete_and_its_undo() {
    let mut h = distro();
    h.eval(
        "(let ((t (track 0)))
           (eseq.sequencer/set-track-expanded t true)
           (eseq.sequencer/set-track-param-mode t 4))",
    );
    h.show_all();
    let tid = h.track_field(0, "tid");
    h.command("delete-track", Value::Number(0.0));
    h.sync();
    h.show_all();
    assert_ne!(h.track_field(0, "tid"), tid);
    assert_eq!(h.track_field(0, "expanded"), Value::Bool(false));
    assert_eq!(
        h.eval("(eseq.sequencer/track-param-mode (track 0))"),
        Value::Number(0.0)
    );
    h.undo();
    h.sync();
    h.show_all();
    assert_eq!(h.track_field(0, "tid"), tid);
    assert_eq!(h.track_field(0, "expanded"), Value::Bool(true));
    assert_eq!(
        h.eval("(eseq.sequencer/track-param-mode (track 0))"),
        Value::Number(4.0)
    );
}

// Regression (2026-11-02, gate toggled during arrangement playback was not heard):
// an arrangement that is one scene (Scene 1) for the whole song, looping and
// playing; the user in the Seq view clicks the sampler panel's gate button
// (`sampler-gate-button`'s on-click, evaluated verbatim). The button toggles
// but the song row snapshot the scheduler plays keeps the old gate.
#[test]
fn gate_button_during_one_scene_song_playback_is_heard() {
    fn settle(h: &mut Harness) {
        h.drain();
        h.sync();
        h.show_all();
        h.tick();
        h.drain();
        h.sync();
        h.show_all();
    }
    let mut h = distro();
    // Scene 1 placed with its patterns as clips, so every lane plays its
    // Pattern 1 (an unplaced scene event leaves the lanes silent).
    h.app.arr_scene_event_insert_placing(0.0, 0).unwrap();
    h.app.set_arrangement_view_visible(false);
    settle(&mut h);
    h.app.song_transport_play(false).expect("play");
    settle(&mut h);
    let click = "(let ((t selection.track) (gate (if t t.gate false))) \
                 (eseq.seq-core-state/cool-off-follow) (seq-set-track-param :gate (if gate 0 1)))";
    let ui_gate = |h: &mut Harness| h.eval("(let ((t selection.track)) (if t t.gate nil))");
    let track = match h.eval("(let ((t selection.track)) t.index)") {
        Value::Number(n) => n as usize,
        other => panic!("selection.track.index: {other:?}"),
    };
    let live_gate = |h: &Harness| h.app.state.pattern.track_params[track].is_gate_on();
    let row_gate = |h: &Harness| {
        h.app.active_runtime_song.as_ref().map(|song| {
            song.rows.iter().map(|row| row.scheduler_snapshot.tracks[track].params.gate).collect::<Vec<_>>()
        })
    };
    let ui_before = ui_gate(&mut h);
    let live_before = live_gate(&h);
    h.eval(click);
    settle(&mut h);
    assert_ne!(ui_gate(&mut h), ui_before, "the button toggles");
    let rows = row_gate(&h).expect("the song is playing");
    assert!(
        rows.iter().all(|gate| *gate != live_before),
        "the song row snapshot (what plays) kept the old gate: {rows:?}"
    );
    h.app.song_transport_stop().unwrap();
}
