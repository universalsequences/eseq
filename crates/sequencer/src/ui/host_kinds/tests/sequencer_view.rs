//! The factory sequencer's compact grid, its track and group headers and the
//! rack clip run, ported to the kinds (kind-bindings spec §13 stage 8,
//! eseq-0l17.11). The expanded step editor, its process lanes, the lane
//! patchbay and the pad grid are not ported yet.

use super::views::{assert_ported, distro, instance_bindings, legacy_forms};
use super::*;

/// The fully ported files' sources.
const PORTED: [(&str, &str); 2] = [
    (
        "ui/track-collapse.lisp",
        include_str!("../../../../../../content/ui/track-collapse.lisp"),
    ),
    (
        "ui/sequencer-keys.lisp",
        include_str!("../../../../../../content/ui/sequencer-keys.lisp"),
    ),
];

const SEQUENCER: &str = include_str!("../../../../../../content/ui/sequencer.lisp");

/// The ported part of ui/sequencer.lisp: from the top to the expanded step
/// editor, and from the track rows to the pad grid.
fn grid_source() -> String {
    let section = |from: &str, to: &str| {
        let start = SEQUENCER.find(from).unwrap_or_else(|| panic!("{from}"));
        let end = start
            + SEQUENCER[start..]
                .find(to)
                .unwrap_or_else(|| panic!("{to}"));
        &SEQUENCER[start..end]
    };
    [
        section(
            "(module eseq.sequencer)",
            ";; ── The expanded step editor ──",
        ),
        section(
            ";; Which sound payloads track t",
            ";; ── Pad grid performance view",
        ),
    ]
    .concat()
}

#[test]
fn ported_sequencer_grid_uses_no_legacy_binding_forms() {
    assert_ported(&PORTED);
    assert_eq!(legacy_forms(&grid_source()), Vec::<&str>::new(), "the grid");
    assert!(SEQUENCER.contains("(import eseq.kinds :refer ("));
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
        for field in ["active", "selected", "held", "lock-kind", "variant-color"] {
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
