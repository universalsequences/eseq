//! The factory sequencer's expanded step editor, its process lanes, lane
//! strip and patchbay, and the drum rack pad grid, ported to the kinds
//! (kind-bindings spec §13 stage 8, eseq-0l17.66).

use super::views::{bound, distro, widget_keyed};
use super::*;

const EDITOR_REFER: &str =
    "(import eseq.kinds :refer (track tracks groups transport selection add-process!))";

impl Harness {
    fn eval_editor(&mut self, code: &str) -> Value {
        self.eval_with(EDITOR_REFER, code)
    }

    /// Run `code`'s commands and sync, as the event loop would.
    fn run_editor(&mut self, code: &str) {
        self.eval_editor(code);
        self.drain();
        self.share_buses_and_groups();
        self.sync();
        self.show_all();
    }

    fn editor_instance(&mut self, code: &str) -> InstanceId {
        match self.eval_editor(code) {
            Value::Instance(id) => id,
            other => panic!("{code}: not an instance: {other:?}"),
        }
    }

    /// The props of the `*sequencer*` widget keyed `key` (its suffix).
    fn sequencer_widget(&self, key: &str) -> HashMap<String, Value> {
        let (tree, _) = self.buffer_tree("*sequencer*");
        widget_keyed(&tree, key).unwrap_or_else(|| {
            let mut keyed = Vec::new();
            super::views::widgets_with_prop(&tree, "key", &mut keyed);
            let keys: Vec<_> = keyed.iter().map(|w| w["key"].clone()).take(40).collect();
            panic!("no widget keyed {key} among {keys:?}")
        })
    }

    fn tid(&mut self, track: usize) -> u64 {
        num(self.track_field(track, "tid")) as u64
    }
}

/// The distro root, with `t0` the first track.
fn editor_harness() -> Harness {
    let mut h = distro();
    h.publish_library();
    h.sync();
    h.eval_editor(
        r#"(def t0 (track 0)) (def t1 (track 1))
           (def lane-offset eseq.seqv-track-params/seqv-process-lane-mode-offset)
           (def by-name (lambda (t name) (eseq.view-kit/named t.processes name)))
           (def lane-mode (lambda (t name)
             (let ((p (by-name t name)) (l (first p.lanes))) (+ lane-offset l.position))))
           (def shown-lane (lambda (t) (nth t.lanes (- t.param-mode lane-offset))))"#,
    );
    h
}

fn bound_to(props: &HashMap<String, Value>, prop: &str, id: InstanceId, field: &str) -> bool {
    bound(props, prop) == Some((id, field.to_string()))
}

/// Each slot of an expanded velocity editor shows its step: the slider binds
/// the step's velocity and gate, the toggle its selection and takes the
/// step; editing a step's velocity repaints and re-renders nothing.
#[test]
fn the_expanded_editor_binds_its_slots_steps_and_only_repaints_on_step_edits() {
    let mut h = editor_harness();
    h.run_editor("(eseq.sequencer/set-track-expanded (track 0) true)");
    let tid = h.tid(0);
    for slot in 0..16 {
        let step = h.editor_instance(&format!("(nth t0.steps {slot})"));
        let slider = h.sequencer_widget(&format!("expanded-step-slider-{tid}-{slot}"));
        assert!(bound_to(&slider, "value", step, "velocity"), "{slider:?}");
        assert!(bound_to(&slider, "haptic-value", step, "velocity"));
        assert!(bound_to(&slider, "active", step, "active"));
        let toggle = h.sequencer_widget(&format!("expanded-step-toggle-{tid}-{slot}"));
        assert!(bound_to(&toggle, "selected", step, "selected"));
        assert_eq!(toggle["step"], Value::Instance(step));
        let label = h.sequencer_widget(&format!("expanded-step-label-{tid}-{slot}"));
        assert!(bound_to(&label, "active", step, "selected"));
        let dot = h.sequencer_widget(&format!("expanded-step-playhead-{tid}-{slot}"));
        assert!(bound_to(&dot, "active", step, "playing"));
    }
    let revision = h.buffer_tree("*sequencer*").1;
    h.run_editor("(let ((s (nth t0.steps 3))) (set! s.velocity 0.4))");
    assert!((num(h.eval_editor("(let ((s (nth t0.steps 3))) s.velocity)")) - 0.4).abs() < 1e-6);
    assert_eq!(
        h.buffer_tree("*sequencer*").1,
        revision,
        "a step edit only repaints"
    );
}

/// A curved param (duration) shows its slider position by value, in a
/// subtree per slot: the slot re-runs alone when its step changes.
#[test]
fn a_curved_param_slot_reads_its_step_in_a_subtree_of_its_own() {
    let mut h = editor_harness();
    h.run_editor(
        "(let ((t (track 0)))
           (eseq.sequencer/set-track-expanded t true)
           (eseq.sequencer/set-track-param-mode t 1))",
    );
    let tid = h.tid(0);
    let slider = h.sequencer_widget(&format!("expanded-step-slider-{tid}-2"));
    assert_eq!(
        slider["haptic-value"],
        Value::Number(1.0),
        "a step lasts 1 by default"
    );
    let before = h.editor.runtime().ui_work_counters();
    h.run_editor("(let ((s (nth t0.steps 2))) (set! s.duration 4))");
    let after = h.editor.runtime().ui_work_counters();
    let slider = h.sequencer_widget(&format!("expanded-step-slider-{tid}-2"));
    assert_eq!(slider["haptic-value"], Value::Number(4.0));
    assert!(
        num(slider["value"].clone()) > 0.5,
        "4 steps sit past the curve's pivot"
    );
    assert_eq!(
        after.full_buffer_reruns, before.full_buffer_reruns,
        "the sequencer does not re-run: {before:?} -> {after:?}"
    );
    assert!(
        after.subtree_reruns - before.subtree_reruns <= 1,
        "at most the slot's slider re-runs: {before:?} -> {after:?}"
    );
}

/// The editor shows the page its cursor is on (`t.page`): moving the cursor
/// to another page re-runs the slot grid, the cursor picker and the pages,
/// never the row; the slots then show that page's steps and the cursor frame
/// binds the track's cursor.
#[test]
fn a_cursor_on_another_page_turns_the_editor_without_rerendering_the_row() {
    let mut h = editor_harness();
    h.shared.state.pattern.track_params[0].set_num_steps(32);
    h.sync();
    h.run_editor("(eseq.sequencer/set-track-expanded (track 0) true)");
    let tid = h.tid(0);
    let t0 = h.track_id(0);
    let column = h.sequencer_widget(&format!("expanded-step-column-{tid}-0"));
    assert!(bound_to(&column, "cursor", t0, "cursor"));
    let before = h.editor.runtime().ui_work_counters();
    h.run_editor("(eseq.sequencer/set-track-cursor (track 0) 18)");
    let after = h.editor.runtime().ui_work_counters();
    assert_eq!(h.track_field(0, "cursor"), Value::Number(18.0));
    assert_eq!(h.track_field(0, "page"), Value::Number(1.0));
    assert_eq!(
        after.full_buffer_reruns - before.full_buffer_reruns,
        1,
        "only the inert `*seq-expand-sync*` projection re-runs whole: {before:?} -> {after:?}"
    );
    assert!(
        after.subtree_reruns - before.subtree_reruns <= 3,
        "at most the slots, the cursor picker and the pages re-run: {before:?} -> {after:?}"
    );
    let step = h.editor_instance("(nth t0.steps 16)");
    let slider = h.sequencer_widget(&format!("expanded-step-slider-{tid}-0"));
    assert!(bound_to(&slider, "value", step, "velocity"));
    let label = h.sequencer_widget(&format!("expanded-step-label-{tid}-0"));
    assert_eq!(label["value"], Value::Number(17.0));
    let page = h.sequencer_widget(&format!("expanded-page-{tid}-1"));
    assert_eq!(page["active"], Value::Bool(true));
    let cursor = h.editor_instance("(nth t0.steps 18)");
    let picker = h.sequencer_widget(&format!("expanded-param-number-picker-{tid}"));
    assert!(bound_to(&picker, "value", cursor, "velocity"));
}

/// While the transport plays and the editor follows, it shows the
/// playhead's page; stopped, its cursor's.
#[test]
fn a_following_editor_shows_the_playheads_page() {
    let mut h = editor_harness();
    h.shared.state.pattern.track_params[0].set_num_steps(32);
    h.sync();
    h.run_editor("(eseq.sequencer/set-track-expanded (track 0) true)");
    let tid = h.tid(0);
    h.set_playing(true);
    h.shared.state.transport.track_playheads[0].store(20, Ordering::Relaxed);
    h.sync();
    h.show_all();
    if h.eval_editor("selection.auto-follow") == Value::Bool(true) {
        let label = h.sequencer_widget(&format!("expanded-step-label-{tid}-0"));
        assert_eq!(label["value"], Value::Number(17.0), "the playhead's page");
    }
    h.set_playing(false);
    h.sync();
    h.show_all();
    let label = h.sequencer_widget(&format!("expanded-step-label-{tid}-0"));
    assert_eq!(label["value"], Value::Number(1.0), "the cursor's page");
}

/// The pages' bar transposes read `t.bar-transposes` and set it through
/// `set-bar-transpose!`: one undo entry, and the picker shows the value.
#[test]
fn a_bar_transpose_picker_shows_and_sets_its_bar() {
    let mut h = editor_harness();
    h.shared.state.pattern.track_params[0].set_num_steps(32);
    h.sync();
    h.run_editor("(eseq.sequencer/set-track-expanded (track 0) true)");
    let tid = h.tid(0);
    let picker = h.sequencer_widget(&format!("expanded-bar-transpose-{tid}-1"));
    assert_eq!(picker["value"], Value::Number(0.0));
    assert_eq!(picker["active"], Value::Bool(false), "a bar at 0 is dim");
    let undo = h.app.history.undo_len();
    h.fire_and_show(&picker, "on-change", Value::Number(5.0));
    assert_eq!(h.app.history.undo_len(), undo + 1);
    assert_eq!(h.shared.state.bar_transpose(0, 1), 5.0);
    let picker = h.sequencer_widget(&format!("expanded-bar-transpose-{tid}-1"));
    assert_eq!(picker["value"], Value::Number(5.0));
    assert_eq!(picker["active"], Value::Bool(true));
}

/// A slot's slider edits its step (one undo entry), or every selected step
/// as a p-lock when the step is selected.
#[test]
fn a_slot_slider_edits_its_step_through_the_kinds() {
    let mut h = editor_harness();
    h.run_editor("(eseq.sequencer/set-track-expanded (track 0) true)");
    let tid = h.tid(0);
    let slider = h.sequencer_widget(&format!("expanded-step-slider-{tid}-5"));
    h.fire_and_show(&slider, "on-change", Value::Number(0.25));
    assert_eq!(
        h.shared.state.pattern.step_data[0].get(5, StepParam::Velocity),
        0.25
    );
    assert_eq!(
        h.track_field(0, "cursor"),
        Value::Number(5.0),
        "the slot takes the cursor"
    );
}

/// A lane mode's sliders show the lane's values and edit them with
/// `set-lane-steps!`; the strip shows the lane's process and the patchbay
/// its chain.
#[test]
fn a_lane_mode_edits_its_lane_and_shows_its_strip_and_patchbay() {
    let mut h = editor_harness();
    h.run_editor(
        "(let ((t (track 0)))
           (eseq.sequencer/set-track-expanded t true)
           (eseq.sequencer/set-track-param-mode t lane-offset))",
    );
    let tid = h.tid(0);
    let lane = h.editor_instance("(first t0.lanes)");
    let process = h.editor_instance("(let ((l (first t0.lanes))) l.process)");
    let proc_id = num(h.eval_editor("(let ((l (first t0.lanes))) l.process.proc-id)")) as u64;
    let strip = h.sequencer_widget(&format!("lane-strip-{tid}-{proc_id}"));
    assert!(strip.contains_key("background-color"));
    let processes = h.eval_editor("t0.processes");
    for p in &h.instances(processes) {
        let id = num(h.rt().instance_field(*p, "proc-id").unwrap()) as u64;
        h.sequencer_widget(&format!("lane-patch-col-{id}"));
    }
    let slider = h.sequencer_widget(&format!("expanded-step-slider-{tid}-2"));
    let max = num(h.rt().instance_field(lane, "max").unwrap());
    h.fire_and_show(&slider, "on-change", Value::Number(max));
    let values = items(&h.rt().instance_field(lane, "values").unwrap());
    assert_eq!(values[2], Value::Number(max), "the lane's step 2");
    let slider = h.sequencer_widget(&format!("expanded-step-slider-{tid}-2"));
    assert_eq!(slider["haptic-value"], Value::Number(max));
    assert_eq!(
        slider["fill"],
        Value::Keyword("process-lane-accent".to_string())
    );
    let enable = h.sequencer_widget(&format!("lane-enable-{proc_id}"));
    h.fire_and_show(&enable, "on-click", Value::Nil);
    assert_eq!(
        h.rt().instance_field(process, "enabled").unwrap(),
        Value::Bool(false)
    );
}

/// A cable dropped from one process's port onto another's in port binds
/// the port, a further one adds a fan-out entry (whose bounds
/// `set-fanout!` sets); selecting a cable and Delete clears it again.
#[test]
fn the_patchbay_wires_and_unwires_processes_through_the_kinds() {
    let mut h = editor_harness();
    h.run_editor(
        "(let ((t (track 0)))
           (eseq.sequencer/set-track-expanded t true)
           (eseq.sequencer/set-track-param-mode t lane-offset))",
    );
    h.eval_editor(
        r#"(def writer (by-name t0 "rand"))
           (def conn (filter (lambda (pt) pt.connectable) writer.ports))
           (def writer-port (first (filter (lambda (pt) (= pt.name "wire")) conn)))
           (def reader (by-name t0 "cmp A"))
           (def cable-id (+ (* writer.index 16) (eseq.view-kit/index-of conn writer-port)))"#,
    );
    let reader = h.editor_instance("reader");
    let undo = h.app.history.undo_len();
    h.run_editor("(eseq.sequencer/lane-patch-connect 0 cable-id reader.index 0)");
    assert_eq!(h.app.history.undo_len(), undo + 1);
    assert_eq!(
        h.eval_editor("writer-port.target-process"),
        Value::Instance(reader),
        "{}",
        h.error()
    );
    let inlet = h.eval_editor("(first reader.in-ports)");
    let Value::String(inlet) = inlet else {
        panic!("{inlet:?}")
    };
    let reader_id = num(h.rt().instance_field(reader, "proc-id").unwrap()) as u64;
    let port = h.sequencer_widget(&format!("lane-patch-in-port-{reader_id}-{inlet}"));
    let cable = num(h.eval_editor("cable-id"));
    assert_eq!(
        port["connected-sources"],
        list_value([Value::Number(cable)])
    );
    // A second cable out of the port is a fan-out entry on it.
    h.eval_editor(r#"(def reader2 (by-name t0 "roll"))"#);
    let reader2 = h.editor_instance("reader2");
    h.run_editor("(eseq.sequencer/lane-patch-connect 0 cable-id reader2.index 0)");
    assert_eq!(
        h.eval_editor("(let ((fo (first writer-port.fanout))) fo.target-process)"),
        Value::Instance(reader2),
        "{}",
        h.error()
    );
    assert_eq!(
        h.eval_editor("writer-port.target-process"),
        Value::Instance(reader)
    );
    // The strip's range pickers set the entry's bounds (`set-fanout!`).
    h.run_editor(r#"(eseq.kinds/set-fanout! (first writer-port.fanout) "lo" -3)"#);
    assert_eq!(
        h.eval_editor("(let ((fo (first writer-port.fanout))) fo.lo)"),
        Value::Number(-3.0)
    );
    h.eval_editor("(eseq.sequencer/lane-patch-select-cable 0 cable-id reader.index 0)");
    assert_eq!(
        h.eval_editor("(eseq.sequencer/lane-patch-cable-selected?)"),
        Value::Bool(true)
    );
    h.run_editor("(eseq.sequencer/lane-patch-delete-selected)");
    assert_eq!(h.eval_editor("writer-port.target-process"), Value::Nil);
    h.eval_editor("(eseq.sequencer/lane-patch-select-cable 0 cable-id reader2.index 0)");
    h.run_editor("(eseq.sequencer/lane-patch-delete-selected)");
    assert_eq!(
        h.eval_editor("(len writer-port.fanout)"),
        Value::Number(0.0)
    );
    assert_eq!(
        h.eval_editor("(eseq.sequencer/lane-patch-cable-selected?)"),
        Value::Bool(false)
    );
}

/// The + box adds a process of the picked class to the track's own lanes
/// and, once the host lists it, selects its lane.
#[test]
fn the_add_box_adds_a_process_and_selects_its_lane() {
    let mut h = editor_harness();
    h.run_editor(
        "(let ((t (track 0)))
           (eseq.sequencer/set-track-expanded t true)
           (eseq.sequencer/set-track-param-mode t lane-offset))",
    );
    let before = h.eval_editor("t0.processes");
    let before = h.instances(before).len();
    h.run_editor(
        "(eseq.sequencer/lane-add-pick (track 0)
           (first (filter (lambda (c) (= c.name \"lane-length\")) (eseq.sequencer/lane-add-options))))",
    );
    h.sync();
    h.show_all();
    let processes = h.eval_editor("t0.processes");
    assert_eq!(h.instances(processes).len(), before + 1, "{}", h.error());
    assert_eq!(
        h.eval_editor("(let ((l (shown-lane (track 0)))) l.process.class-name)"),
        Value::String("lane-length".to_string()),
        "the new process's lane is selected"
    );
}

/// The pad grid draws a rack's pads on their notes: an occupied cell lights
/// on its pad's trigger (`#'p.triggered`), a drop moves a pad to a note
/// (`p.note`), and the role menu sets `p.role`.
#[test]
fn the_pad_grid_draws_and_edits_a_racks_pads() {
    let mut h = editor_harness();
    let (group, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    let kick = sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE;
    h.app
        .assign_rack_pad_track_recorded(group, kick, 0)
        .expect("kick pad");
    h.share_buses_and_groups();
    h.sync();
    h.eval_editor("(def g (first (groups))) (def p (first g.pads))");
    let pad = h.editor_instance("p");
    let gid = num(h.eval_editor("g.gid")) as u64;
    let grid = h.eval_editor("(eseq.sequencer/pad-grid g)");
    // C1 is the bottom-left cell (12) of the rack's default page.
    let cell = widget_keyed(&grid, &format!("rack-pad-cell-{gid}-12")).expect("C1 cell");
    assert!(bound_to(&cell, "selected", pad, "triggered"));
    let empty = widget_keyed(&grid, &format!("rack-pad-cell-{gid}-13")).expect("C#1 cell");
    assert_eq!(empty["selected"], Value::Bool(false));
    h.run_editor(
        "(eseq.sequencer/drop-pad-on-note
           (dict :payload (dict :group-id g.gid :pad-note p.note)) g (+ p.note 1))",
    );
    assert_eq!(
        h.rt().instance_field(pad, "note").unwrap(),
        Value::Number((kick + 1) as f64)
    );
    h.eval_editor("(eseq.sequencer/open-pad-menu (dict :at (dict :col 1 :row 1)) p)");
    h.run_editor("(eseq.sequencer/choose-pad-role \"clap\")");
    assert_eq!(
        h.rt().instance_field(pad, "role").unwrap(),
        Value::String("clap".into())
    );
    assert_eq!(
        h.eval_editor("(eseq.sequencer/focused-pad g)"),
        Value::Instance(pad)
    );
    // The rack panel's address (ui/effects/buffers.lisp): by group position.
    assert_eq!(
        h.eval_editor("(get (eseq.sequencer/selected-pad 0) :track)"),
        Value::Number(0.0)
    );
}

/// The lane selector lists every lane of the track's chain under one
/// dropdown (default lanes by their short names), and picking one shows it.
#[test]
fn the_lane_selector_lists_every_lane_and_picks_one() {
    let mut h = editor_harness();
    h.run_editor("(eseq.sequencer/set-track-expanded t0 true)");
    let tid = h.tid(0);
    let selector = h.sequencer_widget(&format!("expanded-process-lane-selector-{tid}"));
    let options = items(&selector["options"]);
    let lanes = h.eval_editor("t0.lanes");
    assert_eq!(
        options.len(),
        h.instances(lanes).len() + 1,
        "none, then a row per lane"
    );
    assert_eq!(options[0], s("none"));
    assert!(options.contains(&s("prob")), "{options:?}");
    assert_eq!(selector["value"], s("none"));
    h.run_editor("(eseq.sequencer/select-process-lane-option t0 \"prob\")");
    assert_eq!(
        h.eval_editor("t0.param-mode"),
        h.eval_editor("(lane-mode t0 \"prob\")")
    );
    let selector = h.sequencer_widget(&format!("expanded-process-lane-selector-{tid}"));
    assert_eq!(selector["value"], s("prob"));
    h.run_editor("(eseq.sequencer/select-process-lane-option t0 \"none\")");
    assert_eq!(
        h.eval_editor("t0.param-mode"),
        Value::Number(3.0),
        "back to transpose"
    );
}

/// A lane slot's slider writes every selected step when its step is one of
/// them; the row's picker writes the cursor step; a lane wider than 1 moves
/// in whole steps unless its UI step says otherwise; a symmetric range
/// fills from 0.
#[test]
fn lane_edits_follow_the_selection_the_cursor_and_the_lane_step() {
    let mut h = editor_harness();
    h.run_editor(
        "(eseq.sequencer/set-track-expanded t0 true)
         (eseq.sequencer/set-track-param-mode t0 (lane-mode t0 \"tacc\"))",
    );
    let tid = h.tid(0);
    let lane = h.editor_instance("(let ((p (by-name t0 \"tacc\"))) (first p.lanes))");
    let value = |h: &mut Harness, step: usize| {
        items(&h.rt().instance_field(lane, "values").unwrap())[step].clone()
    };
    let (min, max) = (
        num(h.rt().instance_field(lane, "min").unwrap()),
        num(h.rt().instance_field(lane, "max").unwrap()),
    );
    let slider = h.sequencer_widget(&format!("expanded-step-slider-{tid}-1"));
    let origin = if min < 0.0 && -min == max { 0.0 } else { min };
    assert_eq!(slider["origin"], Value::Number(origin));
    // Selected steps 1..3: a slot on step 2 writes all three.
    h.eval_editor("(seq-select-step-range 1 3)");
    h.sync();
    let slider = h.sequencer_widget(&format!("expanded-step-slider-{tid}-2"));
    h.fire_and_show(&slider, "on-change", Value::Number(2.4));
    for step in 1..=3 {
        assert_eq!(
            value(&mut h, step),
            Value::Number(2.0),
            "step {step}: whole steps"
        );
    }
    // A half step: the UI step quantizes what the sliders write.
    h.eval_editor("(eseq.sequencer/set-lane-slider-step (let ((p (by-name t0 \"tacc\"))) (first p.lanes)) 0.5)");
    h.eval_editor("(seq-clear-selection)");
    h.eval_editor("(eseq.sequencer/set-track-cursor t0 5)");
    let picker = h.sequencer_widget(&format!("expanded-param-number-picker-{tid}"));
    h.show_all();
    let picker_now = h.sequencer_widget(&format!("expanded-param-number-picker-{tid}"));
    assert_eq!(picker_now["step"], Value::Number(0.5));
    h.fire_and_show(&picker, "on-change", Value::Number(1.3));
    assert_eq!(
        value(&mut h, 5),
        Value::Number(1.5),
        "the cursor step, on the half step"
    );
}

/// The strip's map button arms its OUT port (the fx panel's process map);
/// a param tab clicked while armed binds the port to that step param.
#[test]
fn the_strip_maps_its_out_port_onto_a_step_param_tab() {
    let mut h = editor_harness();
    h.run_editor(
        "(eseq.sequencer/set-track-expanded t0 true)
         (eseq.sequencer/set-track-param-mode t0 (lane-mode t0 \"rand\"))",
    );
    let tid = h.tid(0);
    let proc_id = num(h.eval_editor("(let ((p (by-name t0 \"rand\"))) p.proc-id)")) as u64;
    let map = h.sequencer_widget(&format!("lane-map-{proc_id}"));
    h.fire_and_show(&map, "on-click", Value::Nil);
    let tab = h.sequencer_widget(&format!("expanded-param-tab-{tid}-0"));
    assert_eq!(
        tab["background-color"],
        Value::Keyword("process-map-arm-bg".into())
    );
    h.fire_and_show(&tab, "on-click", Value::Nil);
    assert_eq!(
        h.eval_editor(
            "(let ((p (by-name t0 \"rand\")))
               (let ((pt (first (filter (lambda (pt) pt.mappable) p.ports)))) pt.target-step-param))"
        ),
        s("velocity")
    );
}

/// A card's enable dot bypasses a project lane on this track only; with the
/// strip's scope on all tracks, on every track.
#[test]
fn a_cards_enable_dot_bypasses_this_track_or_every_track_by_scope() {
    let mut h = editor_harness();
    h.run_editor(
        "(eseq.sequencer/set-track-expanded t0 true)
         (eseq.sequencer/set-track-param-mode t0 (lane-mode t0 \"prob\"))",
    );
    let enabled = |h: &mut Harness, t: &str| {
        h.eval_editor(&format!("(let ((p (by-name {t} \"rand\"))) p.enabled)"))
    };
    let proc_id = num(h.eval_editor("(let ((p (by-name t0 \"rand\"))) p.proc-id)")) as u64;
    let click = |h: &mut Harness| {
        let dot = h.sequencer_widget(&format!("lane-patch-enable-{proc_id}"));
        h.fire_and_show(&dot, "on-click", Value::Nil);
    };
    click(&mut h);
    assert_eq!(enabled(&mut h, "t0"), Value::Bool(false));
    assert_eq!(
        enabled(&mut h, "t1"),
        Value::Bool(true),
        "the other track keeps it"
    );
    click(&mut h);
    h.eval_editor("(eseq.sequencer/lane-toggle-edit-scope)");
    click(&mut h);
    assert_eq!(enabled(&mut h, "t0"), Value::Bool(false));
    assert_eq!(enabled(&mut h, "t1"), Value::Bool(false), "all tracks");
}

/// Cards reorder by drop (the dragged card takes the target's place) and go
/// by their menu; the editor stays on its lane through both.
#[test]
fn cards_move_by_drop_and_go_by_their_menu() {
    let mut h = editor_harness();
    for class in ["lane-length", "lane-roll"] {
        h.run_editor(&format!(
            "(add-process! t0 (first (filter (lambda (c) (= c.name \"{class}\")) (eseq.sequencer/lane-add-options))))"
        ));
    }
    h.eval_editor(
        "(def own (filter (lambda (p) (not p.project)) t0.processes))
         (def first-own (first own)) (def second-own (nth own 1))",
    );
    h.run_editor(
        "(eseq.sequencer/set-track-expanded t0 true)
         (eseq.sequencer/set-track-param-mode t0 (let ((l (first second-own.lanes))) (+ lane-offset l.position)))",
    );
    let second = h.editor_instance("second-own");
    h.run_editor("(eseq.sequencer/lane-patch-move-card 0 second-own.proc-id first-own.proc-id)");
    assert_eq!(
        h.editor_instance("(first (filter (lambda (p) (not p.project)) t0.processes))"),
        second
    );
    assert_eq!(
        h.eval_editor("(let ((l (shown-lane t0))) (= l.process second-own))"),
        Value::Bool(true),
        "the editor stays on the moved lane"
    );
    let first = h.editor_instance("first-own");
    h.run_editor("(eseq.sequencer/lane-patch-remove-card 0 first-own.proc-id)");
    assert!(!h.rt().instance_is_live(first));
    assert_eq!(
        h.eval_editor("(let ((l (shown-lane t0))) (= l.process second-own))"),
        Value::Bool(true),
        "and through a delete"
    );
}

/// A rack's grid opens on the page holding its kit and is note-positional:
/// the bottom-left cell is the page's C, paging moves a cell an octave, and
/// a drop on an empty cell adds a member on exactly that note.
#[test]
fn the_pad_grid_is_note_positional_and_empty_cells_take_new_members() {
    let mut h = editor_harness();
    let (group, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    let kick = sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE;
    h.app
        .assign_rack_pad_track_recorded(group, kick, 0)
        .expect("kick pad");
    h.share_buses_and_groups();
    h.sync();
    h.eval_editor("(def g (first (groups)))");
    assert_eq!(
        h.eval_editor("(eseq.sequencer/pad-cell-note g 12)"),
        Value::Number(kick as f64)
    );
    assert_eq!(
        h.eval_editor("(eseq.sequencer/pad-cell-note g 0)"),
        Value::Number((kick + 12) as f64)
    );
    h.eval_editor("(eseq.sequencer/set-pad-page g (+ (eseq.sequencer/pad-page g) 1))");
    assert_eq!(
        h.eval_editor("(eseq.sequencer/pad-cell-note g 12)"),
        Value::Number((kick + 12) as f64),
        "a page up is an octave up"
    );
    h.eval_editor("(eseq.sequencer/set-pad-page g (- (eseq.sequencer/pad-page g) 1))");
    h.editor.drain_host_commands();
    h.eval_editor(
        "(eseq.sequencer/drop-on-pad-cell
           (dict :drag-type \"sample\" :payload (dict :path \"samples/hat.wav\")
                 :target (dict :kind \"rack-pad\"))
           g 13)",
    );
    let payload = h.last_custom("add-track-sample");
    assert_eq!(get(&payload, "pad-note"), Value::Number((kick + 1) as f64));
    assert_eq!(get(&payload, "group-id"), Value::Number(group as f64));
    // An occupied cell is its member track's own drop target.
    let meta =
        h.eval_editor("(eseq.sequencer/pad-cell-drop-meta g 12 (eseq.sequencer/pad-at g 12))");
    assert_eq!(get(&meta, "track"), Value::Number(0.0));
    assert_eq!(get(&meta, "from-pad"), Value::Bool(true));
}

/// The octave map draws every note, occupied ones filled and lit by their
/// pad's trigger, and highlights the grid's page; the *fx* rack panel
/// reaches the map and the grid by the rack's group position.
#[test]
fn the_pad_map_mirrors_the_grid_and_the_rack_panel_reaches_both() {
    let mut h = editor_harness();
    let (group, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    let kick = sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE;
    h.app
        .assign_rack_pad_track_recorded(group, kick, 0)
        .expect("kick pad");
    h.share_buses_and_groups();
    h.sync();
    h.eval_editor("(def g (first (groups))) (def p (first g.pads))");
    let pad = h.editor_instance("p");
    let map = h.eval_editor("(eseq.sequencer/pad-map g)");
    let cell = widget_keyed(&map, &format!("rack-pad-map-cell-{group}-{kick}")).expect("kick note");
    assert!(bound_to(&cell, "selected", pad, "triggered"));
    let empty =
        widget_keyed(&map, &format!("rack-pad-map-cell-{group}-{}", kick + 1)).expect("C#1");
    assert_eq!(empty["selected"], Value::Bool(false));
    assert_ne!(cell["background-color"], empty["background-color"]);
    let row = widget_keyed(&map, &format!("rack-pad-map-row-{group}-{kick}")).expect("C1 row");
    assert_eq!(
        row["border-color"],
        Value::Keyword("mixer-strip-selected-border".into())
    );
    // The *fx* rack panel (ui/effects/buffers.lisp) addresses them by the
    // group's position.
    let grid = h.eval_editor("(eseq.sequencer/rack-pad-grid 0)");
    assert!(widget_keyed(&grid, &format!("rack-pad-grid-{group}")).is_some());
    let map = h.eval_editor("(eseq.sequencer/rack-pad-map 0)");
    assert!(widget_keyed(&map, &format!("rack-pad-map-{group}")).is_some());
}

/// A node bay reads its node's processes (`n.processes`): an expr card's
/// error dot falls back to the process's run error (`p.error`, under the
/// slot's own id), and the harmony meter reads its scope cells.
#[test]
fn a_node_bays_errors_and_scopes_read_the_node_processes() {
    let mut h = editor_harness();
    h.eval("(import alez.neural.variable-reset)");
    let before = crate::host_commands::instances::instance_ids(&h.app);
    h.eval("(host-command \"instance-create\" (dict :kind \"alez/neural:neural\"))");
    h.drain();
    let created = crate::host_commands::instances::instance_ids(&h.app);
    let id = *created.difference(&before).next().expect("an instance");
    h.sync();
    h.eval_editor(&format!(
        "(def nn (instance-ref {id})) (def ns (eseq.sequencer/lane-patch-register-node nn 1))
         (def rid (graph-node-process-add nn 1 \"lane-rand\"))"
    ));
    h.drain();
    h.sync();
    let rid = num(h.eval_editor("rid")) as u64;
    let error = "(let ((n (eseq.sequencer/node-of ns)) (p (first n.processes)))
                   (eseq.sequencer/lane-patch-expr-error ns (dict :instance-id rid :process p)))";
    assert_eq!(h.eval_editor(error), Value::Nil);
    h.shared
        .state
        .publish_process_run_errors(std::collections::BTreeMap::from([(
            rid,
            "boom".to_string(),
        )]));
    h.sync();
    assert_eq!(h.eval_editor(error), s("boom"));
    assert_eq!(
        h.eval_editor("(let ((n (eseq.sequencer/node-of ns)) (p (first n.processes))) p.proc-id)"),
        Value::Number(rid as f64)
    );
}

/// Cmd+A over a two-plus track selection (`selection.tracks`) selects every
/// step of each; a single selected track stays the plain select-all.
#[test]
fn select_all_spans_a_multi_track_selection() {
    let mut h = editor_harness();
    let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    for name in [
        "seq-select-all-steps-on-tracks",
        "seq-select-all-steps",
        "seq-set-track",
    ] {
        let calls = std::rc::Rc::clone(&calls);
        h.editor
            .runtime_mut()
            .register_native(name, move |args, _ctx| {
                let args: Vec<_> = args.iter().map(eseqlisp::vm::format_lisp_value).collect();
                calls
                    .borrow_mut()
                    .push(format!("{name} {}", args.join(" ")));
                Ok(Value::Nil)
            });
    }
    h.shared.current_track.store(1, Ordering::Relaxed);
    h.shared.selected_tracks.lock().unwrap().extend([0, 1]);
    h.sync();
    h.eval_editor("(eseq.sequencer/select-all-current-track-steps)");
    assert_eq!(*calls.borrow(), ["seq-select-all-steps-on-tracks (0 1)"]);

    calls.borrow_mut().clear();
    h.shared.selected_tracks.lock().unwrap().remove(&0);
    h.sync();
    h.eval_editor("(eseq.sequencer/select-all-current-track-steps)");
    assert_eq!(*calls.borrow(), ["seq-select-all-steps "]);
}

/// The grid cursor is global: the edit target and a track's cursor frame
/// wrap it to the current track's length, and selecting another track
/// shows that track the same global cursor (no stale per-track cursor).
#[test]
fn the_global_cursor_wraps_to_each_tracks_length() {
    let mut h = editor_harness();
    h.shared.state.pattern.track_params[1].set_num_steps(6);
    h.shared.current_track.store(1, Ordering::Relaxed);
    h.sync();
    h.run_editor("(eseq.step-grid-interactions/set-track-cursor-step 8)");
    assert_eq!(
        h.eval_editor("(eseq.seq-core-state/current-step)"),
        Value::Number(2.0)
    );
    assert_eq!(
        h.eval_editor("(eseq.sequencer/track-cursor t1)"),
        Value::Number(2.0)
    );
    h.run_editor("(eseq.sequencer/select-track-for-edit t0)");
    assert_eq!(
        h.eval_editor("eseq.vanilla/cursor-step"),
        Value::Number(8.0)
    );
    assert_eq!(
        h.eval_editor("(eseq.sequencer/track-cursor t0)"),
        Value::Number(8.0)
    );
}

/// Two expanded rows keep their own param tab and page.
#[test]
fn expanded_rows_keep_their_own_tab_and_page() {
    let mut h = editor_harness();
    for track in 0..2 {
        h.shared.state.pattern.track_params[track].set_num_steps(32);
    }
    h.sync();
    h.run_editor(
        "(eseq.sequencer/set-track-expanded t0 true)
         (eseq.sequencer/set-track-expanded t1 true)",
    );
    let (tid0, tid1) = (h.tid(0), h.tid(1));
    for (tid, mode) in [(tid0, 3), (tid1, 4)] {
        let tab = h.sequencer_widget(&format!("expanded-param-tab-{tid}-{mode}"));
        h.fire_and_show(&tab, "on-click", Value::Nil);
    }
    let page = h.sequencer_widget(&format!("expanded-page-{tid0}-1"));
    h.fire_and_show(&page, "on-click", Value::Nil);
    assert_eq!(h.track_field(0, "param-mode"), Value::Number(3.0));
    assert_eq!(h.track_field(1, "param-mode"), Value::Number(4.0));
    assert_eq!(h.track_field(0, "page"), Value::Number(1.0));
    assert_eq!(h.track_field(1, "page"), Value::Number(0.0));
}

/// A control on another track's expanded row makes that track current
/// before it edits: a slot slider, and the double button.
#[test]
fn an_expanded_rows_controls_make_its_track_current_first() {
    let mut h = editor_harness();
    h.run_editor("(eseq.sequencer/set-track-expanded t1 true)");
    assert_eq!(h.shared.current_track.load(Ordering::Relaxed), 0);
    let tid = h.tid(1);
    let slider = h.sequencer_widget(&format!("expanded-step-slider-{tid}-0"));
    h.fire_and_show(&slider, "on-change", Value::Number(0.25));
    assert_eq!(h.shared.current_track.load(Ordering::Relaxed), 1);
    assert_eq!(
        h.shared.state.pattern.step_data[1].get(0, StepParam::Velocity),
        0.25
    );

    h.run_editor("(eseq.sequencer/select-track-for-edit t0)");
    assert_eq!(h.shared.current_track.load(Ordering::Relaxed), 0);
    let steps = h.shared.state.pattern.track_params[1].get_num_steps();
    let double = h.sequencer_widget(&format!("expanded-double-{tid}"));
    h.fire_and_show(&double, "on-click", Value::Nil);
    assert_eq!(h.shared.current_track.load(Ordering::Relaxed), 1);
    assert_eq!(
        h.shared.state.pattern.track_params[1].get_num_steps(),
        steps * 2
    );
}

/// In a lane mode with no lane, the row picker shows 0 (not a step field);
/// when the editor's lane leaves the chain and no lane is left, the editor
/// goes back to transpose, as picking "none" does.
#[test]
fn a_lane_mode_without_a_lane_shows_0_and_falls_back_to_transpose() {
    let mut h = editor_harness();
    h.run_editor(
        "(eseq.sequencer/set-track-expanded t0 true)
         (eseq.sequencer/set-track-param-mode t0 (+ lane-offset 999))",
    );
    let tid = h.tid(0);
    let picker = h.sequencer_widget(&format!("expanded-param-number-picker-{tid}"));
    assert_eq!(picker["value"], Value::Number(0.0), "{picker:?}");

    h.run_editor("(eseq.sequencer/set-track-param-mode t0 (lane-mode t0 \"rand\"))");
    h.run_editor("(eseq.sequencer/lane-patch-reselect-lane t0 (list))");
    assert_eq!(h.track_field(0, "param-mode"), Value::Number(3.0));
}

/// The + box stops waiting once the host lists the added process, whether
/// or not it has a lane to select.
#[test]
fn the_add_box_stops_waiting_once_its_process_is_listed() {
    let mut h = editor_harness();
    h.run_editor("(eseq.sequencer/set-track-expanded t0 true)");
    h.run_editor(
        "(eseq.sequencer/lane-add-pick t0
           (first (filter (lambda (c) (= c.name \"lane-roll\")) (eseq.sequencer/lane-add-options))))",
    );
    h.sync();
    h.show_all();
    assert_eq!(
        h.eval_editor("(let ((pending eseq.sequencer/lane-add)) pending.track)"),
        Value::Nil
    );
    assert_eq!(
        h.eval_editor("(let ((l (shown-lane t0))) l.process.class-name)"),
        s("lane-roll")
    );
}

/// Arming an out port (a click) only repaints: the ports bind the pending
/// port, so no expanded row and no subtree re-runs, and the port is the
/// patch machinery's pending source (`:pending-port` against its id), not
/// the same port of another track's bay.
#[test]
fn arming_an_out_port_only_repaints() {
    let mut h = editor_harness();
    h.run_editor(
        "(eseq.sequencer/set-track-expanded t0 true)
         (eseq.sequencer/set-track-expanded t1 true)
         (eseq.sequencer/set-track-param-mode t0 (lane-mode t0 \"rand\"))
         (eseq.sequencer/set-track-param-mode t1 (lane-mode t1 \"rand\"))",
    );
    // An out port by its id (`:track`).
    let out_port = |h: &Harness, id: usize| {
        let (tree, _) = h.buffer_tree("*sequencer*");
        let mut ports = Vec::new();
        super::views::widgets_with_prop(&tree, "patch-port", &mut ports);
        ports
            .into_iter()
            .find(|w| {
                w.get("direction") == Some(&Value::Keyword("out".into()))
                    && w.get("track") == Some(&Value::Number(id as f64))
            })
            .unwrap_or_else(|| panic!("no out port {id}"))
    };
    // Rand's first out port: (bay 0, its slot, ordinal 0); bay 1's twin.
    let id = num(h.eval_editor("(let ((p (by-name t0 \"rand\"))) (* p.index 16))")) as usize;
    let other_id = id + 4096 * 16;
    let port = out_port(&h, id);
    assert!(!eseqlisp::widget_render::patch_port_pending(&port, id));
    h.sync();
    h.show_all();
    let before = h.editor.runtime().ui_work_counters();
    let revision = h.buffer_tree("*sequencer*").1;
    h.fire_and_show(&port, "on-mouse-down", Value::Nil);
    let after = h.editor.runtime().ui_work_counters();
    assert_eq!(
        h.eval_editor("(eseq.sequencer/lane-patch-pending-port)"),
        Value::Number(id as f64)
    );
    assert_eq!(
        after.subtree_reruns, before.subtree_reruns,
        "{before:?} -> {after:?}"
    );
    assert_eq!(
        after.full_buffer_reruns, before.full_buffer_reruns,
        "{before:?} -> {after:?}"
    );
    assert_eq!(
        h.buffer_tree("*sequencer*").1,
        revision,
        "arming only repaints"
    );
    let port = out_port(&h, id);
    assert!(eseqlisp::widget_render::patch_port_pending(&port, id));
    let other = out_port(&h, other_id);
    assert!(!eseqlisp::widget_render::patch_port_pending(
        &other, other_id
    ));
    assert_eq!(port["slot-key"], other["slot-key"], "one place in two bays");
    assert_ne!(port["bay-key"], other["bay-key"]);
}
