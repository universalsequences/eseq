//! Stage 7h: drum racks (`pad`, `rack-clip`, `groove`, `pad-groove`,
//! `pool-groove`, `library-groove`, `group.armed` and the group's rack
//! fields, `track.pad`).

use super::*;
use sequencer::groove::{GrooveRow, GrooveSlot, GrooveSlotSource, ProjectGroove};
use sequencer::project::PadRole;
use sequencer::sequencer::{TrackId, DRUM_RACK_FIRST_PAD_NOTE};

const REFER_7H: &str = "(import eseq.kinds :refer (track tracks scenes groups project transport \
                        pad-role-options groove-scale-options trigger-pad! launch-rack-clip! \
                        silence-rack! save-rack-clip-as! delete-rack-clip! convert-rack-to-clips! \
                        apply-groove-to-all-clips! duplicate-groove! delete-groove!))";

/// The kick and hat pads' notes (C1, D1).
const KICK: i32 = DRUM_RACK_FIRST_PAD_NOTE;
const HAT: i32 = DRUM_RACK_FIRST_PAD_NOTE + 2;

/// A one-bar 16th groove with every odd 16th late by `late` slots.
fn swing(id: u64, name: &str, late: f32) -> ProjectGroove {
    let slot = |index: usize| GrooveSlot {
        offset: if index % 2 == 1 { late } else { 0.0 },
        source: if index % 4 == 1 {
            GrooveSlotSource::Measured
        } else {
            GrooveSlotSource::Zero
        },
        ..GrooveSlot::default()
    };
    ProjectGroove {
        id,
        name: name.to_string(),
        period_beats: 4.0,
        resolution_beats: 0.25,
        pad_rows: Vec::new(),
        shared_row: GrooveRow {
            slots: (0..16).map(slot).collect(),
        },
    }
}

impl Harness {
    fn eval_7h(&mut self, code: &str) -> Value {
        let source = format!("{REFER_7H}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// Run `code`'s commands, mirror the groups as the event loop does,
    /// then sync.
    fn run_7h(&mut self, code: &str) {
        self.eval_7h(code);
        self.drain();
        self.share_buses_and_groups();
        self.sync();
    }

    /// Run `code`'s commands; they must fail with `message` and change no
    /// history.
    fn rejects_7h(&mut self, code: &str, message: &str) {
        let undo = self.app.history.undo_len();
        self.editor.minibuffer = None;
        self.eval_7h(code);
        self.drain();
        let error = self.editor.minibuffer.clone().unwrap_or_default();
        assert!(error.contains(message), "{code}: {error}");
        assert_eq!(self.app.history.undo_len(), undo, "{code} changed nothing");
    }

    /// A drum rack holding tracks 0 (the kick, on C1) and 1 (the hat, on
    /// D1), with a pool groove; `g` is the rack, `p0` / `p1` its pads, `gr`
    /// its own groove. Returns the rack's group id.
    fn drum_rack(&mut self) -> u64 {
        self.drum_rack_on(0, 1)
    }

    /// [`Self::drum_rack`] over tracks `kick` and `hat`.
    fn drum_rack_on(&mut self, kick: usize, hat: usize) -> u64 {
        let (group, _) = self.app.create_drum_rack_recorded(None).expect("rack");
        self.app
            .assign_rack_pad_track_recorded(group, KICK, kick)
            .expect("kick pad");
        self.app
            .assign_rack_pad_track_recorded(group, HAT, hat)
            .expect("hat pad");
        self.app.grooves.push(swing(7, "Swing", 0.25));
        self.share_buses_and_groups();
        self.sync();
        self.eval_7h(
            "(def g (first (groups))) (def p0 (first g.pads)) (def p1 (nth g.pads 1))
             (def gr g.groove)",
        );
        group
    }

    fn rack(&self, group: u64) -> &sequencer::project::ProjectRackConfig {
        let group = self.app.groups.iter().find(|g| g.id == group).unwrap();
        group.rack.as_ref().expect("a rack")
    }

    fn rack_syncs(&self) -> u64 {
        self.frame.host_kinds.racks.syncs
    }

    fn lane_builds(&self) -> u64 {
        self.frame.host_kinds.racks.lane_builds
    }

    fn library_listings(&self) -> u64 {
        self.frame.host_kinds.racks.library_listings
    }

    fn seq_7h(&self, field: &str) -> Value {
        self.rt()
            .reactive_field_value("SEQ", field)
            .unwrap_or_else(|| panic!("SEQ.{field}"))
            .clone()
    }

    /// Publish the legacy group, groove and rack clip fields as the tick
    /// does.
    fn publish_legacy_racks(&mut self) {
        let state = self.shared.state.clone();
        let rt = self.editor.runtime_mut();
        sync_groups_bindings(rt, &self.app.groups, &self.app.grooves);
        sync_rack_clip_state(rt, &state);
    }
}

fn get(value: &Value, key: &str) -> Value {
    match value {
        Value::Map(map) => map
            .get(key)
            .map_or(Value::Nil, |cell| cell.borrow().clone()),
        other => panic!("not a map: {other:?}"),
    }
}

fn list(value: Value) -> Vec<Value> {
    match value {
        Value::List(items) => items.iter().map(|item| item.borrow().clone()).collect(),
        other => panic!("not a list: {other:?}"),
    }
}

#[test]
fn rack_fields_read_after_sync_and_match_the_legacy_fields() {
    let mut h = Harness::new();
    let gid = h.drum_rack();
    assert_eq!(h.eval_7h("g.rack"), Value::Bool(true));
    assert_eq!(h.eval_7h("(len g.pads)"), Value::Number(2.0));
    assert_eq!(h.eval_7h("p0.group"), h.eval_7h("g"));
    assert_eq!(h.eval_7h("p0.track"), h.eval_7h("(track 0)"));
    assert_eq!(h.eval_7h("(let ((t (track 1))) t.pad)"), h.eval_7h("p1"));
    assert_eq!(h.eval_7h("p0.note"), Value::Number(KICK as f64));
    assert_eq!(h.eval_7h("p0.label"), s("C1"));
    assert_eq!(h.eval_7h("p0.choke"), Value::Number(0.0));
    assert_eq!(h.eval_7h("p0.role"), s(""));
    let kick = PadRole::standard(KICK).expect("the standard layout names C1");
    assert_eq!(h.eval_7h("p0.role-tag"), s(kick.tag()));
    // The standard role by key, comparable with `role` and the options.
    assert_eq!(h.eval_7h("p0.standard-role"), s(kick.key()));
    assert_eq!(h.eval_7h("p0.standard-role-label"), s(kick.label()));
    let options = list(h.eval_7h("pad-role-options"));
    assert!(options.contains(&h.eval_7h("p0.standard-role")));
    // The member's steps are the pad's lane (legacy SEQ.track-steps).
    assert_eq!(
        h.eval_7h("p0.track.steps"),
        h.eval_7h("(let ((t (track 0))) t.steps)")
    );
    // Legacy parity: SEQ.groups' :pads.
    h.publish_legacy_racks();
    let groups = list(h.seq_7h("groups"));
    let pads = list(get(&groups[0], "pads"));
    for (index, pad) in pads.iter().enumerate() {
        h.eval_7h(&format!("(def px (nth g.pads {index}))"));
        let fields = [
            ("pad-note", "note"),
            ("label", "label"),
            ("role", "role"),
            ("role-tag", "role-tag"),
            ("role-label", "role-label"),
            ("standard-role-label", "standard-role-label"),
        ];
        for (legacy, field) in fields {
            assert_eq!(
                get(pad, legacy),
                h.eval_7h(&format!("px.{field}")),
                "{field}"
            );
        }
        assert_eq!(get(pad, "track"), h.eval_7h("px.track.index"));
        let choke = num(get(pad, "choke")).max(0.0);
        assert_eq!(Value::Number(choke), h.eval_7h("px.choke"));
    }
    // The rack's own groove, straight, and the pool.
    assert_eq!(h.eval_7h("gr.group"), h.eval_7h("g"));
    assert_eq!(h.eval_7h("gr.clip"), Value::Nil);
    assert_eq!(h.eval_7h("gr.pool-groove"), Value::Nil);
    assert_eq!(h.eval_7h("gr.slots"), Value::Number(16.0));
    assert_eq!(h.eval_7h("(len gr.pads)"), Value::Number(2.0));
    assert_eq!(
        h.eval_7h("(let ((sh (first gr.pads))) sh.pad)"),
        h.eval_7h("p0")
    );
    assert_eq!(h.eval_7h("(len project.groove-pool)"), Value::Number(1.0));
    h.eval_7h("(def pg (first project.groove-pool))");
    assert_eq!(h.eval_7h("pg.groove-id"), Value::Number(7.0));
    assert_eq!(h.eval_7h("pg.name"), s("Swing"));
    assert_eq!(h.eval_7h("pg.racks"), list_value(Vec::new()));
    let pool = list(h.seq_7h("groove-pool"));
    assert_eq!(get(&pool[0], "grid"), h.eval_7h("pg.grid"));
    // The library listing is the pickers'.
    let library = list(h.seq_7h("groove-library"));
    assert_eq!(
        h.eval_7h("(len project.groove-library)"),
        Value::Number(library.len() as f64)
    );
    for (index, entry) in library.iter().enumerate() {
        h.eval_7h(&format!("(def lx (nth project.groove-library {index}))"));
        assert_eq!(get(entry, "key"), h.eval_7h("lx.choice"));
        assert_eq!(get(entry, "name"), h.eval_7h("lx.name"));
        assert_eq!(get(entry, "tier"), h.eval_7h("lx.tier"));
    }
    // Play the pool groove: the lanes and amounts match SEQ.rack-grooves.
    h.run_7h("(set! gr.pool-groove pg) (set! gr.timing 0.75)");
    assert_eq!(h.eval_7h("gr.pool-groove"), h.eval_7h("pg"));
    assert_eq!(h.eval_7h("pg.racks"), h.eval_7h("(list g)"));
    h.publish_legacy_racks();
    let entry = list(h.seq_7h("rack-grooves")).remove(0);
    assert_eq!(get(&entry, "active-groove-id"), Value::Number(7.0));
    assert_eq!(get(&entry, "enabled"), h.eval_7h("gr.enabled"));
    assert_eq!(get(&entry, "scale"), h.eval_7h("gr.scale"));
    assert_eq!(get(&entry, "active-grid"), h.eval_7h("gr.grid"));
    let lanes = get(&entry, "lanes");
    assert_eq!(get(&lanes, "slots"), h.eval_7h("gr.slots"));
    assert_eq!(get(&lanes, "all-cells"), h.eval_7h("gr.cells"));
    assert_eq!(get(&lanes, "all-measured"), h.eval_7h("gr.measured"));
    for (index, pad) in list(get(&lanes, "pads")).iter().enumerate() {
        h.eval_7h(&format!("(def sx (nth gr.pads {index}))"));
        assert_eq!(get(pad, "pad-note"), h.eval_7h("sx.pad.note"));
        assert_eq!(get(pad, "cells"), h.eval_7h("sx.cells"));
        assert_eq!(get(pad, "measured"), h.eval_7h("sx.measured"));
        assert_eq!(get(pad, "enabled"), h.eval_7h("sx.enabled"));
        let field = rack_groove_pad_amount_field(gid, num(get(pad, "pad-note")) as i32);
        assert_eq!(h.seq_7h(&field), h.eval_7h("sx.amount"));
    }
    for amount in ["timing", "velocity", "random"] {
        let field = rack_groove_amount_field(amount, gid);
        assert_eq!(h.seq_7h(&field), h.eval_7h(&format!("gr.{amount}")));
    }
    // A legacy rack: no clips, no bank.
    assert_eq!(h.eval_7h("g.legacy"), Value::Bool(true));
    assert_eq!(h.eval_7h("g.clips"), list_value(Vec::new()));
    assert_eq!(h.eval_7h("g.rack-clip"), Value::Nil);
    assert_eq!(h.seq_7h("rack-clips"), list_value(Vec::new()));
    // Convert: a clip per scene, the current scene playing its own.
    h.run_7h("(convert-rack-to-clips! g)");
    assert_eq!(h.eval_7h("g.legacy"), Value::Bool(false));
    h.eval_7h("(def rc (first g.clips))");
    assert_eq!(h.eval_7h("g.rack-clip"), h.eval_7h("rc"));
    assert_eq!(h.eval_7h("rc.group"), h.eval_7h("g"));
    assert_eq!(h.eval_7h("rc.active"), Value::Bool(true));
    assert_eq!(h.eval_7h("rc.index"), Value::Number(0.0));
    assert_eq!(h.eval_7h("rc.groove"), Value::Nil);
    assert_eq!(h.eval_7h("rc.own-groove"), Value::Bool(false));
    h.publish_legacy_racks();
    let banks = list(h.seq_7h("rack-clips"));
    assert_eq!(get(&banks[0], "active"), h.eval_7h("rc.cid"));
    let clips = list(get(&banks[0], "clips"));
    assert_eq!(
        h.eval_7h("(len g.clips)"),
        Value::Number(clips.len() as f64)
    );
    for (index, clip) in clips.iter().enumerate() {
        h.eval_7h(&format!("(def cx (nth g.clips {index}))"));
        assert_eq!(get(clip, "id"), h.eval_7h("cx.cid"));
        assert_eq!(get(clip, "name"), h.eval_7h("cx.name"));
        let cid = num(get(clip, "id")) as u64;
        let active = h.seq_7h(&format!("rack-clip-active-{gid}-{cid}"));
        let active = Value::Bool(active == Value::Number(1.0));
        assert_eq!(active, h.eval_7h("cx.active"));
    }
    let scene_clips = list(get(&banks[0], "scene-clips"));
    for (index, clip) in clips.iter().enumerate() {
        let cid = get(clip, "id");
        let legacy: Vec<Value> = (scene_clips.iter().enumerate())
            .filter(|(_, pointer)| **pointer == cid)
            .map(|(scene, _)| Value::Number(scene as f64))
            .collect();
        h.eval_7h(&format!("(def cx (nth g.clips {index}))"));
        let scenes = h.eval_7h("(map (lambda (sc) sc.index) cx.scenes)");
        assert_eq!(list(scenes), legacy, "clip {index}'s scenes");
    }
    let index = h.seq_7h(&format!("rack-clip-index-{gid}"));
    assert_eq!(index, Value::Number(1.0), "the legacy index is 1-based");
    // Silence: no clip plays.
    h.run_7h("(silence-rack! g)");
    assert_eq!(h.eval_7h("g.rack-clip"), Value::Nil);
    assert_eq!(h.eval_7h("rc.active"), Value::Bool(false));
    h.run_7h("(launch-rack-clip! rc)");
    assert_eq!(h.eval_7h("g.rack-clip"), h.eval_7h("rc"));
}

#[test]
fn rack_setters_go_through_history_and_undo() {
    let mut h = Harness::new();
    let gid = h.drum_rack();
    let undo = h.app.history.undo_len();
    // Pads: note (an occupied note swaps), choke, role.
    h.run_7h("(set! p0.note -30)");
    assert_eq!(h.eval_7h("p0.note"), Value::Number(-30.0));
    assert_eq!(h.rack(gid).pads[0].pad_note, -30);
    h.run_7h(&format!("(set! p0.note {HAT})"));
    assert_eq!(h.eval_7h("p0.note"), Value::Number(HAT as f64));
    assert_eq!(h.eval_7h("p1.note"), Value::Number(-30.0), "swapped");
    h.run_7h("(set! p0.choke 3) (set! p0.choke 3)");
    assert_eq!(h.eval_7h("p0.choke"), Value::Number(3.0));
    h.run_7h("(set! p1.role \"SNARE\")");
    assert_eq!(h.eval_7h("p1.role"), s("snare"));
    assert_eq!(h.eval_7h("p1.role-tag"), s("SD"));
    assert_eq!(h.app.history.undo_len() - undo, 4, "one entry per change");
    h.rejects_7h("(set! p0.note 52)", "an integer from -36 to 51");
    h.rejects_7h("(set! p0.choke 17)", "an integer from 0 to 16");
    h.rejects_7h("(set! p0.role \"cowbell\")", "one of pad-role-options");
    for _ in 0..2 {
        app::edit::undo(&mut h.app);
    }
    h.share_buses_and_groups();
    h.sync();
    assert_eq!(h.eval_7h("p1.role"), s(""));
    assert_eq!(h.eval_7h("p0.choke"), Value::Number(0.0));
    // The rack's arm: absolute, exclusive with the members' record arms.
    *h.shared.record_armed.lock().unwrap() = vec![true, false];
    h.run_7h("(set! g.armed true) (set! g.armed true)");
    assert_eq!(*h.shared.armed_rack.lock().unwrap(), Some(gid));
    assert_eq!(h.eval_7h("g.armed"), Value::Bool(true));
    assert_eq!(h.shared.record_armed.lock().unwrap()[0], false);
    h.run_7h("(set! g.armed false)");
    assert_eq!(*h.shared.armed_rack.lock().unwrap(), None);
    assert_eq!(h.eval_7h("g.armed"), Value::Bool(false));
    // Grooves: each edit one entry, values by the value rule.
    h.eval_7h("(def pg (first project.groove-pool)) (def sh (nth gr.pads 1))");
    let undo = h.app.history.undo_len();
    h.run_7h("(set! gr.pool-groove pg) (set! gr.enabled false) (set! gr.scale 2)");
    h.run_7h(
        "(set! gr.timing 0.5) (set! gr.random 0.25) (set! sh.amount 0.5) (set! sh.enabled false)",
    );
    let settings = h.rack(gid).groove.clone();
    assert_eq!(settings.active, Some(7));
    assert!(!settings.enabled);
    assert_eq!(settings.scale, 2.0);
    assert_eq!(
        (settings.timing_amount, settings.random_amount),
        (0.5, 0.25)
    );
    let share = settings.pad(num(h.eval_7h("sh.pad.note")) as i32);
    assert_eq!((share.amount, share.enabled), (0.5, false));
    assert_eq!(h.app.history.undo_len() - undo, 7);
    assert_eq!(h.eval_7h("gr.timing"), Value::Number(0.5));
    assert_eq!(h.eval_7h("sh.amount"), Value::Number(0.5));
    assert_eq!(h.eval_7h("sh.enabled"), Value::Bool(false));
    h.rejects_7h("(set! gr.timing 2)", "a number from 0 to 1.5");
    h.rejects_7h("(set! gr.scale 1.5)", "one of groove-scale-options");
    h.rejects_7h("(set! sh.amount -0.1)", "a number from 0 to 1");
    app::edit::undo(&mut h.app);
    h.share_buses_and_groups();
    h.sync();
    assert_eq!(h.eval_7h("sh.enabled"), Value::Bool(true));
    // A drag view's amount set!s while the pointer is down: one entry.
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for timing in [0.6, 0.7, 0.8] {
        h.run_7h(&format!(
            "(set! gr.timing {timing}) (set! sh.amount {timing})"
        ));
        assert!((num(h.eval_7h("gr.timing")) - timing).abs() < 1e-6);
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), undo + 1, "the drag is one entry");
    app::edit::undo(&mut h.app);
    assert_eq!(h.rack(gid).groove.timing_amount, 0.5);
    assert_eq!(h.app.history.undo_len(), undo);
    // A clip's own groove, and the clip's name.
    h.share_buses_and_groups();
    h.run_7h("(convert-rack-to-clips! g)");
    h.eval_7h("(def rc (first g.clips))");
    h.run_7h("(set! rc.name \"Verse\") (set! rc.own-groove true)");
    assert_eq!(h.eval_7h("rc.name"), s("Verse"));
    assert_eq!(h.eval_7h("rc.own-groove"), Value::Bool(true));
    h.eval_7h("(def cg rc.groove)");
    assert_eq!(h.eval_7h("cg.clip"), h.eval_7h("rc"));
    h.run_7h("(set! cg.timing 1.25)");
    let cid = num(h.eval_7h("rc.cid")) as u64;
    assert_eq!(
        h.rack(gid).clip_groove(cid).map(|own| own.timing_amount),
        Some(1.25)
    );
    assert_eq!(
        h.rack(gid).groove.timing_amount,
        0.5,
        "the rack's own stays"
    );
    h.rejects_7h("(set! rc.name \"  \")", "a non-empty name");
    h.run_7h("(set! rc.own-groove false)");
    assert_eq!(h.eval_7h("rc.groove"), Value::Nil);
    app::edit::undo(&mut h.app);
    h.share_buses_and_groups();
    h.sync();
    assert_eq!(h.eval_7h("rc.own-groove"), Value::Bool(true));
    // The pool groove's name.
    h.run_7h("(set! pg.name \"Shuffle\")");
    assert_eq!(h.eval_7h("pg.name"), s("Shuffle"));
    h.rejects_7h("(set! pg.name \"\")", "a non-empty name");
    app::edit::undo(&mut h.app);
    h.sync();
    assert_eq!(h.eval_7h("pg.name"), s("Swing"));
}

#[test]
fn a_member_reorder_between_set_and_apply_still_targets_the_set_pad() {
    let mut h = Harness::new();
    h.app.graph_controller().add_empty_track().expect("add");
    let gid = h.drum_rack_on(1, 2);
    // The kick moves to another note, and the track in front of the rack
    // goes, before the command lands.
    h.eval_7h("(set! p0.choke 2)");
    h.app
        .set_rack_pad_note_recorded(gid, KICK, -20)
        .expect("move");
    h.app.delete_track_recorded(0).expect("delete");
    h.drain();
    let rack = h.rack(gid);
    let kick = rack
        .pads
        .iter()
        .position(|pad| pad.pad_note == -20)
        .unwrap();
    assert_eq!(rack.choke_group(kick), Some(2));
    assert_eq!(rack.choke_group(1 - kick), None);
    // A pad whose member left the rack is an error, not another pad's edit.
    h.share_buses_and_groups();
    h.sync();
    h.eval_7h("(set! p1.choke 4)");
    h.app.remove_track_from_group_recorded(1).expect("remove");
    h.editor.minibuffer = None;
    h.drain();
    let error = h.editor.minibuffer.clone().unwrap_or_default();
    assert!(error.contains("the pad is gone"), "{error}");
    assert_eq!(h.rack(gid).choke_group(0), Some(2));
}

#[test]
fn rack_identity_survives_pad_moves_member_changes_reorders_and_project_load() {
    let mut h = Harness::new();
    h.app.graph_controller().add_empty_track().expect("add");
    let gid = h.drum_rack_on(1, 2);
    let group = h.instance_of(GROUP, &[0]);
    let tid = |h: &Harness, track: usize| h.app.track_registry.id_at(track).unwrap().0;
    let (kick_tid, hat_tid) = (tid(&h, 1), tid(&h, 2));
    let kick = h.instance_of(PAD, &[group, kick_tid]);
    let hat = h.instance_of(PAD, &[group, hat_tid]);
    let groove = h.instance_of(GROOVE, &[group, 0]);
    let share = h.instance_of(PAD_GROOVE, &[groove, hat_tid]);
    let kick_share = h.instance_of(PAD_GROOVE, &[groove, kick_tid]);
    // A note move and a swap keep the instances; only `note` moves.
    h.app
        .set_rack_pad_note_recorded(gid, KICK, HAT)
        .expect("swap");
    h.share_buses_and_groups();
    h.sync();
    assert_eq!(h.instance_of(PAD, &[group, kick_tid]), kick);
    assert_eq!(
        h.rt().instance_field(kick, "note"),
        Ok(Value::Number(HAT as f64))
    );
    assert_eq!(
        h.rt().instance_field(hat, "note"),
        Ok(Value::Number(KICK as f64))
    );
    // The share list follows the note order; the instances stay.
    assert_eq!(
        h.rt().instance_field(groove, "pads"),
        Ok(list_value([
            Value::Instance(share),
            Value::Instance(kick_share)
        ]))
    );
    // A track deleted in front re-indexes the members: same pads; its
    // undo too.
    for step in ["delete", "undo"] {
        match step {
            "delete" => {
                h.app.delete_track_recorded(0).expect("delete");
            }
            _ => {
                app::edit::undo(&mut h.app);
            }
        }
        h.share_buses_and_groups();
        h.sync();
        let position = h.app.track_registry.index_of(TrackId(kick_tid)).unwrap();
        let track = h.track_id(position as u64);
        assert_eq!(
            h.rt().instance_field(kick, "track"),
            Ok(Value::Instance(track))
        );
        assert_eq!(
            h.rt().instance_field(track, "pad"),
            Ok(Value::Instance(kick))
        );
        assert!(h.rt().instance_is_live(hat), "{step}");
        assert!(h.rt().instance_is_live(share), "{step}");
    }
    // A new member gets a new pad; the others stay.
    let third = h.app.graph_controller().add_empty_track().expect("add");
    h.app
        .assign_rack_pad_track_recorded(gid, KICK + 4, third)
        .expect("third pad");
    h.share_buses_and_groups();
    h.sync();
    assert_eq!(h.eval_7h("(len g.pads)"), Value::Number(3.0));
    assert!(h.rt().instance_is_live(kick) && h.rt().instance_is_live(hat));
    // Removing a member drops its pad (and share) only.
    let hat_track = h.app.track_registry.index_of(TrackId(hat_tid)).unwrap();
    h.app
        .remove_track_from_group_recorded(hat_track)
        .expect("remove");
    h.share_buses_and_groups();
    h.sync();
    assert!(!h.rt().instance_is_live(hat));
    assert!(!h.rt().instance_is_live(share));
    assert!(h.rt().instance_is_live(kick));
    assert_eq!(h.eval_7h("(len g.pads)"), Value::Number(2.0));
    // Rack clips: keyed by clip id across another clip's save.
    h.run_7h("(convert-rack-to-clips! g)");
    let first = h.eval_7h("(first g.clips)");
    h.run_7h("(save-rack-clip-as! g \"B\")");
    assert_eq!(h.eval_7h("(first g.clips)"), first);
    assert_eq!(h.eval_7h("(len g.clips)"), Value::Number(2.0));
    let pool = h.instance_of(POOL_GROOVE, &[0]);
    // A project load replaces the rack and everything under it, and the pool.
    h.command("new-project", Value::Nil);
    h.share_buses_and_groups();
    h.sync();
    for id in [group, kick, groove, pool] {
        assert!(!h.rt().instance_is_live(id), "{id:?} replaced");
    }
    let Value::Instance(first) = first else {
        panic!("a clip");
    };
    assert!(!h.rt().instance_is_live(first));
    assert_eq!(h.eval_7h("project.groove-pool"), list_value(Vec::new()));
}

#[test]
fn rack_live_fields_are_computed_only_while_observed() {
    let mut h = Harness::new();
    let gid = h.drum_rack();
    for lit in [true, false, true] {
        h.frame.rack_pad_triggers = vec![lit, !lit];
        *h.shared.armed_rack.lock().unwrap() = lit.then_some(gid);
        h.sync();
    }
    assert_eq!(h.computed(f::PAD_TRIGGERED), 0, "unobserved");
    assert_eq!(h.computed(f::GROUP_ARMED), 0, "unobserved");
    // A by-value read asks the reader once.
    assert_eq!(h.eval_7h("p0.triggered"), Value::Bool(true));
    assert_eq!(h.computed(f::PAD_TRIGGERED), 1);
    // A binding: the pad follows its member's light, on the tick.
    h.eval_7h("(def lit #'p1.triggered) (def armed #'g.armed)");
    h.sync();
    let after = h.computed(f::PAD_TRIGGERED);
    h.frame.rack_pad_triggers = vec![false, true];
    h.sync();
    assert_eq!(h.slot("lit"), 1.0);
    assert_eq!(h.slot("armed"), 1.0);
    *h.shared.armed_rack.lock().unwrap() = None;
    h.sync();
    assert_eq!(h.slot("armed"), 0.0);
    assert!(
        h.computed(f::PAD_TRIGGERED) - after <= 4,
        "the observed pad only, not every pad"
    );
    // Rack syncs run on rack edits only; an amount edit rebuilds no lanes.
    let (syncs, lanes) = (h.rack_syncs(), h.lane_builds());
    let models = h.model_syncs();
    let volume = app::AppCommand::SetTrackVolume {
        track: 0,
        value: 0.3,
    };
    app::try_apply_command(&mut h.app, volume).expect("volume");
    app::edit::finish_active_gesture(&mut h.app);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert!(h.model_syncs() > models, "the model sync ran");
    assert_eq!(h.rack_syncs(), syncs, "an edit elsewhere runs no rack sync");
    h.run_7h("(set! gr.timing 0.3)");
    assert_eq!(h.rack_syncs(), syncs + 1);
    assert_eq!(h.lane_builds(), lanes, "an amount rebuilds no lanes");
    h.run_7h("(set! p0.note -20)");
    assert!(h.lane_builds() > lanes, "a pad move rebuilds them");
    // An amount drag lists the library no more; a library edit (the UI
    // epoch) does.
    let listings = h.library_listings();
    h.gesture.pointer_down = true;
    for timing in [0.4, 0.5, 0.6] {
        h.run_7h(&format!("(set! gr.timing {timing})"));
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    assert_eq!(h.library_listings(), listings, "a drag lists nothing");
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.library_listings(), listings + 1);
}

#[test]
fn the_racks_own_groove_is_clip_0_in_every_command() {
    let mut h = Harness::new();
    let gid = h.drum_rack();
    h.run_7h("(convert-rack-to-clips! g)");
    h.eval_7h("(def rc (first g.clips))");
    h.run_7h("(set! rc.own-groove true) (set! gr.timing 1.25)");
    let cid = num(h.eval_7h("rc.cid")) as u64;
    assert!(h.rack(gid).clip_groove(cid).is_some());
    // The rack's own groove (clip 0) becomes every clip's.
    h.editor.minibuffer = None;
    h.run_7h("(apply-groove-to-all-clips! gr)");
    assert_eq!(h.editor.minibuffer, None);
    assert_eq!(h.eval_7h("rc.own-groove"), Value::Bool(false));
    assert_eq!(h.rack(gid).groove.timing_amount, 1.25);
    // The legacy commands take 0 for the rack's own too (and -1, as the
    // buffer sends it).
    for (clip, timing) in [(0, 0.5), (-1, 0.25)] {
        h.run_7h(&format!(
            "(host-command \"set-rack-groove-amount\"
               (dict :group-id g.gid :clip-id {clip} :amount \"timing\" :value {timing}))"
        ));
        app::edit::finish_active_gesture(&mut h.app);
        assert_eq!(h.rack(gid).groove.timing_amount, timing, "clip-id {clip}");
    }
}

#[test]
fn triggering_a_gone_pad_is_an_error() {
    let mut h = Harness::new();
    h.drum_rack();
    h.editor.minibuffer = None;
    h.run_7h("(trigger-pad! p0)");
    assert_eq!(h.editor.minibuffer, None, "a live pad plays");
    // The hat's member leaves the rack before the hit lands.
    h.app.remove_track_from_group_recorded(1).expect("remove");
    h.rejects_7h("(trigger-pad! p1)", "the pad is gone");
}

#[test]
fn rack_option_constants_match_the_host() {
    let mut h = Harness::new();
    let roles = list(h.eval_7h("pad-role-options"));
    let keys: Vec<Value> = PadRole::ALL.iter().map(|role| s(role.key())).collect();
    assert_eq!(roles, keys);
    let scales = list(h.eval_7h("groove-scale-options"));
    let host: Vec<Value> = sequencer::groove::GROOVE_SCALES
        .iter()
        .map(|scale| Value::Number(*scale as f64))
        .collect();
    assert_eq!(scales, host);
    // The declared ranges are the setters' (the value rule's) bounds.
    let range = |h: &Harness, kind: &str, field: &str| {
        let schema = h.rt().instance_kind_schema(kind).expect(kind);
        let declared = schema.host.iter().find(|host| host.field.name == field);
        declared.and_then(|host| host.range).expect(field)
    };
    use sequencer::groove::{
        GROOVE_PAD_AMOUNT_MAX, GROOVE_RANDOM_AMOUNT_MAX, GROOVE_TIMING_AMOUNT_MAX,
        GROOVE_VELOCITY_AMOUNT_MAX,
    };
    use sequencer::sequencer::DRUM_RACK_LAST_PAD_NOTE;
    let ranges = [
        (GROOVE, "timing", GROOVE_TIMING_AMOUNT_MAX as f64),
        (GROOVE, "velocity", GROOVE_VELOCITY_AMOUNT_MAX as f64),
        (GROOVE, "random", GROOVE_RANDOM_AMOUNT_MAX as f64),
        (PAD_GROOVE, "amount", GROOVE_PAD_AMOUNT_MAX as f64),
        (
            PAD,
            "choke",
            crate::host_commands::rack_kinds::CHOKE_GROUPS as f64,
        ),
    ];
    for (kind, field, max) in ranges {
        assert_eq!(range(&h, kind, field), (0.0, max), "{kind}.{field}");
    }
    let notes = (
        DRUM_RACK_FIRST_PAD_NOTE as f64,
        DRUM_RACK_LAST_PAD_NOTE as f64,
    );
    assert_eq!(range(&h, PAD, "note"), notes);
    for kind in [
        PAD,
        RACK_CLIP,
        GROOVE,
        PAD_GROOVE,
        POOL_GROOVE,
        LIBRARY_GROOVE,
    ] {
        assert!(
            host_kind_names().contains(&eseqlisp::vm::kind_name_of(kind)),
            "{kind} is reserved"
        );
        assert!(
            h.rt().instance_kind_schema(kind).is_some(),
            "{kind} declared"
        );
    }
}
