//! The factory drum rack lookups and the *groove* buffer, ported to the
//! kinds (kind-bindings spec §13 stage 8, eseq-0l17.19): the buffer reads
//! the playing groove (`groove`, `pad-groove`, `pool-groove`,
//! `library-groove`) and edits it through the kind setters and the groove
//! actions, driven here through the production host path with the
//! scheduler's own timing function as "play" (docs/rack-groove-spec.md,
//! "UI").

use super::views::{assert_ported, distro, instance_bindings};
use super::*;
use sequencer::groove::{grooved_sample_time, GrooveFloor};
use sequencer::sequencer::{StepParam, DRUM_RACK_FIRST_PAD_NOTE};

/// The ported files' sources.
const PORTED: [(&str, &str); 2] = [
    (
        "ui/drum-rack-v2.lisp",
        include_str!("../../../../../../content/ui/drum-rack-v2.lisp"),
    ),
    (
        "ui/rack-groove-buffer.lisp",
        include_str!("../../../../../../content/ui/rack-groove-buffer.lisp"),
    ),
];

const REFER_RACK: &str = "(import eseq.kinds :refer (track tracks groups project \
                          launch-rack-clip! convert-rack-to-clips!))";

const KICK: usize = 0;
const HAT: usize = 1;
/// One 16th step in beats (the default timebase).
const STEP_BEATS: f64 = 0.25;
/// The played take: (step, Delay) per member. The kick's step-7 hit is
/// "very late", which extraction reads as an EARLY hit on step 8.
const KICK_TAKE: [(usize, f32); 3] = [(0, 0.02), (7, 0.85), (10, 0.06)];
const HAT_TAKE: [(usize, f32); 4] = [(2, 0.3), (6, 0.26), (10, 0.34), (14, 0.22)];

impl Harness {
    fn eval_rack(&mut self, code: &str) -> Value {
        self.eval_with(REFER_RACK, code)
    }

    /// Run `code`, then apply what Lisp queues, sync and render, turn after
    /// turn until it queues nothing. Returns the commands' names, in order.
    fn rack_turn(&mut self, code: &str) -> Vec<String> {
        self.settle_rack();
        self.eval_rack(code);
        self.settle_rack()
    }

    /// [`Self::rack_turn`] for a widget callback. Both settle what earlier
    /// turns left queued (the mixer's refreshes) first.
    fn rack_call(&mut self, callback: &Value, args: Vec<Value>) -> Vec<String> {
        self.settle_rack();
        self.editor
            .runtime_mut()
            .invoke(callback.clone(), args)
            .expect("widget callback");
        self.settle_rack()
    }

    fn settle_rack(&mut self) -> Vec<String> {
        let mut names = Vec::new();
        for _ in 0..4 {
            let commands = self.custom_commands();
            if commands.is_empty() {
                break;
            }
            for (name, payload) in commands {
                self.command(&name, payload);
                names.push(name);
            }
            self.share_buses_and_groups();
            self.sync();
            self.show_all();
        }
        names
    }

    /// A Lisp read of the playing groove `gr` of rack `g`.
    fn groove_read(&mut self, code: &str) -> Value {
        self.eval_rack(&format!(
            "(let ((gr (eseq.drum-rack-v2/playing-groove g))) {code})"
        ))
    }

    /// The *groove* buffer's layout.
    fn groove_layout(&mut self) -> std::sync::Arc<eseqlisp::layout::LayoutNode> {
        let buffer = (self.editor.buffers.iter())
            .find(|b| b.name == "*groove*")
            .expect("the *groove* buffer")
            .id;
        self.editor.set_active_buffer(buffer);
        self.editor.set_layout_viewport(40, 24);
        self.editor.refresh_runtime_side_effects();
        self.editor.widget_layout().expect("groove buffer layout")
    }
}

/// The prop `prop` of the widget with debug name `name`.
fn prop(layout: &eseqlisp::layout::LayoutNode, name: &str, prop: &str) -> Value {
    find_debug(layout, name)
        .unwrap_or_else(|| panic!("{name}"))
        .props
        .get(prop)
        .unwrap_or_else(|| panic!("{name} {prop}"))
        .clone()
}

#[test]
fn ported_rack_files_use_no_legacy_binding_forms() {
    assert_ported(&PORTED);
}

/// Extract, pick, edit and play a rack's groove through the *groove*
/// buffer's own controls: every edit is a kind setter or a groove action,
/// what the buffer shows is the kinds' state, and the scheduler plays it.
#[test]
fn extract_pick_and_play_a_rack_groove_through_the_groove_buffer() {
    let mut h = distro();
    // Setup (not under test): the project's two tracks on a drum rack (the
    // kick on C1, the hat on F#1), with a played, off-grid take on their
    // live patterns.
    let (group_id, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    h.app
        .assign_rack_pad_track_recorded(group_id, DRUM_RACK_FIRST_PAD_NOTE, KICK)
        .expect("kick pad");
    h.app
        .assign_rack_pad_track_recorded(group_id, DRUM_RACK_FIRST_PAD_NOTE + 6, HAT)
        .expect("hat pad");
    for (track, take) in [(KICK, &KICK_TAKE[..]), (HAT, &HAT_TAKE[..])] {
        for &(step, delay) in take {
            h.app.state.pattern.patterns[track].set_step_active(step, true);
            app::try_apply_command(
                &mut h.app,
                app::AppCommand::SetStepParam {
                    track,
                    step,
                    param: StepParam::Delay,
                    value: delay,
                },
            )
            .expect("take delay");
        }
    }
    // Where each hit was heard, in beats: step start + Delay of a step.
    let heard = |take: &[(usize, f32)]| -> Vec<f64> {
        take.iter()
            .map(|&(step, delay)| (step as f64 + delay as f64) * STEP_BEATS)
            .collect()
    };
    let (kick_heard, hat_heard) = (heard(&KICK_TAKE), heard(&HAT_TAKE));
    h.share_buses_and_groups();
    h.sync();
    h.show_all();
    h.eval_rack("(def g (first (groups)))");
    let state = h.shared.state.clone();
    let spq = 44_100.0 * 60.0 / state.transport.bpm.load(Ordering::Relaxed) as f64;
    // What the scheduler plays for a straight trig at `beat` on `track`.
    let played = |track: usize, beat: f64| -> Option<f64> {
        let table = state.track_grooves();
        let groove = table.get(track).and_then(Option::as_ref)?;
        let straight = (beat * spq).round() as u64;
        let sample = grooved_sample_time(groove, beat, straight, spq, GrooveFloor::default())
            .expect("an on-time trig always plays");
        Some(sample as f64 / spq)
    };
    let check_take = |steps: &[usize], heard: &[f64], track: usize| {
        for (step, heard) in steps.iter().zip(heard) {
            let beat = *step as f64 * STEP_BEATS;
            let at = played(track, beat).expect("the member plays through the groove");
            assert!(
                (at - heard).abs() < 2.0 / spq,
                "track {track} step {step}: played at {at} beats, heard at {heard}"
            );
        }
    };
    let grooved_kick = |h: &mut Harness| {
        h.eval_rack("(if (eseq.drum-rack-v2/groove-of-track (track 0)) true false)")
    };

    // Nothing active yet: no groove, and the swing control is live. The
    // current track (the kick) selects its rack.
    assert_eq!(h.groove_read("gr.pool-groove"), Value::Nil);
    assert!(played(KICK, 0.0).is_none());
    assert_eq!(
        h.eval_rack("(= (eseq.rack-groove-buffer/selected-rack) g)"),
        Value::Bool(true),
        "the current track's rack is the buffer's rack"
    );
    // Straight lanes: one share per pad in pad-note order, a bar of 16ths
    // with no cells, so the buffer draws the pads' hits instead.
    assert_eq!(h.groove_read("gr.slots"), Value::Number(16.0));
    assert_eq!(h.groove_read("(len gr.cells)"), Value::Number(0.0));
    assert_eq!(
        items(&h.groove_read("(map (lambda (q) q.pad.track.index) gr.pads)")),
        [Value::Number(KICK as f64), Value::Number(HAT as f64)]
    );
    assert_eq!(
        h.groove_read("(let ((q (first gr.pads))) (len q.cells))"),
        Value::Number(0.0)
    );

    // 1. Extract Groove… : open the modal, name it, commit (1 bar, 1/16,
    //    quantize source on — the modal's defaults).
    let sent = h.rack_turn(
        "(eseq.rack-groove-buffer/open-extract g)
         (let ((x eseq.rack-groove-buffer/groove-extract)) (set! x.name \"Take\"))
         (eseq.rack-groove-buffer/commit-extract)",
    );
    assert_eq!(sent, vec!["extract-rack-groove".to_string()]);
    assert_eq!(
        h.eval_rack("(let ((x eseq.rack-groove-buffer/groove-extract)) x.open)"),
        Value::Bool(false),
        "the modal closes on commit"
    );
    assert_eq!(
        h.app.grooves.len(),
        1,
        "the extracted groove is in the project pool"
    );
    assert_eq!(
        h.groove_read("gr.pool-groove.groove-id"),
        Value::Number(h.app.grooves[0].id as f64)
    );
    assert_eq!(h.groove_read("gr.enabled"), Value::Bool(true));
    assert_eq!(h.eval_rack("(len project.groove-pool)"), Value::Number(1.0));
    assert_eq!(h.groove_read("gr.grid"), s("1 bar · 1/16"));
    // The lanes: the All row and each pad's own row (the one it plays).
    assert_eq!(h.groove_read("gr.slots"), Value::Number(16.0));
    assert_eq!(h.groove_read("(len gr.cells)"), Value::Number(16.0));
    assert_eq!(
        items(&h.groove_read("(map (lambda (q) q.pad.note) gr.pads)")),
        [
            Value::Number(DRUM_RACK_FIRST_PAD_NOTE as f64),
            Value::Number((DRUM_RACK_FIRST_PAD_NOTE + 6) as f64)
        ],
        "one lane per pad in pad-note order"
    );
    // The kick's share (the first, in pad-note order).
    let kick =
        |h: &mut Harness, read: &str| h.groove_read(&format!("(let ((q (first gr.pads))) {read})"));
    let kick_cell = |h: &mut Harness, read: &str| kick(h, &format!("(nth q.{read})"));
    match kick_cell(&mut h, "cells 8") {
        Value::Number(offset) => assert!((offset + 0.15).abs() < 1e-4, "the early kick"),
        other => panic!("a cell: {other:?}"),
    }
    assert_eq!(kick_cell(&mut h, "measured 8"), Value::Bool(true));
    assert_eq!(
        kick_cell(&mut h, "measured 4"),
        Value::Bool(false),
        "an unplayed slot is filled, dimmed"
    );
    assert_eq!(kick(&mut h, "q.enabled"), Value::Bool(true));
    assert_eq!(kick(&mut h, "q.amount"), Value::Number(1.0));
    // Quantize source: the take is now on the grid (kick step 7 -> 8), no
    // Delay left.
    let patterns = &h.app.state.pattern;
    assert!(patterns.patterns[KICK].is_active(8) && !patterns.patterns[KICK].is_active(7));
    for step in [0usize, 8, 10] {
        assert_eq!(patterns.step_data[KICK].get(step, StepParam::Delay), 0.0);
    }
    // The member's swing control shows the groove hint instead.
    assert_eq!(grooved_kick(&mut h), Value::Bool(true));

    // The buffer lays out: header, amounts and a lane per pad.
    let layout = h.groove_layout();
    let panel = find_debug(&layout, "rack-groove-buffer").expect("groove buffer");
    assert_laid_out(panel, "groove buffer");
    for name in [
        "rack-groove-picker",
        "rack-groove-actions",
        "rack-groove-enabled",
        "rack-groove-timing",
        "rack-groove-velocity",
        "rack-groove-random",
        "rack-groove-all",
        "rack-groove-lanes-scroll",
        "rack-groove-pad",
        "rack-groove-pad-amount",
        "rack-groove-pad-enabled",
        "rack-groove-scale",
    ] {
        assert_laid_out(
            find_debug(panel, name).unwrap_or_else(|| panic!("{name}")),
            name,
        );
    }
    // The picker: No groove, then a "This project" header over the pool and
    // a "Factory" header over the factory MPC swings (`:headers` rows); the
    // trigger shows the groove's grid before its chevron.
    assert_eq!(
        prop(panel, "rack-groove-picker", "filterable"),
        Value::Bool(true)
    );
    assert_eq!(prop(panel, "rack-groove-picker", "value"), s("Take"));
    assert_eq!(
        prop(panel, "rack-groove-picker", "detail"),
        s("1 bar · 1/16")
    );
    let labels = strings(&prop(panel, "rack-groove-picker", "options"));
    assert_eq!(
        labels[..4],
        ["No groove", "This project", "Take", "Factory"]
    );
    assert_eq!(
        items(&prop(panel, "rack-groove-picker", "headers"))[..2],
        [Value::Number(1.0), Value::Number(3.0)]
    );
    assert_eq!(
        h.eval_rack(&format!(
            "(let ((lg (first (filter (lambda (lg) (= lg.choice \"factory:mpc-swing-66-16th\")) \
                                       project.groove-library)))) \
               (if lg lg.name \"\"))"
        )),
        s("MPC 16 Swing 66%")
    );
    assert!(
        labels.contains(&"MPC 16 Swing 66%".to_string()),
        "{labels:?}"
    );
    let details = items(&prop(panel, "rack-groove-picker", "details"));
    assert_eq!(details.len(), labels.len(), "a detail per option");
    assert_eq!(details[2], s("1 bar · 1/16"), "the take's grid");
    let footer = prop(panel, "rack-groove-picker", "footer");
    // The ≡ menu beside the picker: Extract, then the playing groove's
    // Save / Rename / Duplicate / Delete.
    assert_eq!(
        strings(&prop(panel, "rack-groove-actions", "options")),
        [
            "Extract from this rack’s clip…",
            "Save to Library",
            "Rename…",
            "Duplicate",
            "Delete from Project"
        ]
    );
    let actions_on_change = prop(panel, "rack-groove-actions", "on-change");
    let picker_on_change = prop(panel, "rack-groove-picker", "on-change");
    // The first pad row is the kick (pad-note order).
    let timing_on_change = prop(panel, "rack-groove-timing", "on-change");
    let kick_share_on_change = prop(panel, "rack-groove-pad-amount", "on-change");
    let kick_include_on_click = prop(panel, "rack-groove-pad-enabled", "on-click");
    let enabled_on_change = prop(panel, "rack-groove-enabled", "on-change");
    let scale_on_change = prop(panel, "rack-groove-scale", "on-change");
    // No role was set, so no lane carries a role badge (never a role
    // guessed from a pad's note, like "Kick" for C1).
    assert!(find_debug(panel, "rack-groove-role").is_none());

    // 2. Play: the quantized source through the groove lands where the take
    //    was heard (one repeat, so the slot median IS the hit).
    check_take(&[0, 8, 10], &kick_heard, KICK);
    check_take(&[2, 6, 10, 14], &hat_heard, HAT);
    assert!(played(KICK, 2.0).unwrap() < 2.0, "the kick is pushed EARLY");

    // 3. Pick through the picker dropdown's own `:on-change`: the footer
    //    opens the Extract Groove modal, No groove plays straight, a factory
    //    library swing is copied into the pool (copy-on-apply) and
    //    activated, playing its shared row on every pad, and the extracted
    //    groove comes back.
    let sent = h.rack_call(&picker_on_change, vec![footer]);
    assert!(sent.is_empty(), "the footer is UI only: {sent:?}");
    assert_eq!(
        h.eval_rack("(let ((x eseq.rack-groove-buffer/groove-extract)) x.open)"),
        Value::Bool(true),
        "the footer opens the Extract Groove modal"
    );
    h.eval_rack("(eseq.rack-groove-buffer/close-extract)");
    let sent = h.rack_call(&picker_on_change, vec![s("No groove")]);
    assert_eq!(sent, vec!["set-groove".to_string()]);
    assert_eq!(h.groove_read("gr.pool-groove"), Value::Nil);
    assert!(played(KICK, 2.0).is_none() && played(HAT, 0.5).is_none());
    assert_eq!(grooved_kick(&mut h), Value::Bool(false));
    let undo_before_library = h.app.history.undo_len();
    let sent = h.rack_call(&picker_on_change, vec![s("MPC 16 Swing 66%")]);
    assert_eq!(sent, vec!["set-rack-groove".to_string()]);
    assert_eq!(
        h.app.grooves.len(),
        2,
        "the library swing was copied into the pool"
    );
    assert_eq!(h.app.grooves[1].name, "MPC 16 Swing 66%");
    assert_eq!(
        h.app.history.undo_len(),
        undo_before_library + 1,
        "import + activate is one undo step"
    );
    assert_eq!(
        h.groove_read("gr.pool-groove.groove-id"),
        Value::Number(h.app.grooves[1].id as f64)
    );
    // The imported groove is now a pool groove under This project.
    let layout = h.groove_layout();
    assert_eq!(
        strings(&prop(&layout, "rack-groove-picker", "options"))[..5],
        [
            "No groove",
            "This project",
            "Take",
            "MPC 16 Swing 66%",
            "Factory"
        ]
    );
    assert_eq!(
        prop(&layout, "rack-groove-picker", "value"),
        s("MPC 16 Swing 66%")
    );
    // A two-slot swing tiles out to a bar of lanes.
    assert_eq!(h.groove_read("gr.slots"), Value::Number(16.0));
    let swung = played(HAT, 0.25).unwrap();
    assert!(
        (swung - (0.25 + 0.32 * 0.25)).abs() < 2.0 / spq,
        "66% swing: {swung}"
    );
    assert!((played(KICK, 0.0).unwrap()).abs() < 1.0 / spq);
    h.rack_call(&picker_on_change, vec![s("Take")]);
    assert_eq!(
        h.groove_read("gr.pool-groove.groove-id"),
        Value::Number(h.app.grooves[0].id as f64)
    );
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 4. Amount pickers: a drag writes through immediately and lands as ONE
    //    undo step when the gesture ends.
    let rack_settings = |app: &app::App| {
        let group = app.groups.iter().find(|g| g.id == group_id).unwrap();
        group.rack.as_ref().unwrap().groove.clone()
    };
    let undo_len = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for value in [0.8, 0.6, 0.5] {
        let sent = h.rack_call(&timing_on_change, vec![Value::Number(value)]);
        assert_eq!(sent, vec!["set-groove".to_string()]);
    }
    h.gesture.pointer_down = false;
    assert_eq!(h.groove_read("gr.timing"), Value::Number(0.5));
    let half = played(KICK, 2.0).unwrap();
    assert!(
        (half - (2.0 - 0.5 * 0.15 * STEP_BEATS)).abs() < 2.0 / spq,
        "half timing: {half}"
    );
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(
        h.app.history.undo_len(),
        undo_len + 1,
        "one drag, one undo step"
    );
    h.undo();
    assert_eq!(
        rack_settings(&h.app).timing_amount,
        1.0,
        "undo restores the pre-drag amount"
    );
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 5. A pad's share: the kick's Amt picker halves only the kick; its
    //    include dot leaves it straight (still grooved, so no track swing)
    //    while the hat keeps the pocket. Both undo.
    let undo_len = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    let sent = h.rack_call(&kick_share_on_change, vec![Value::Number(0.5)]);
    h.gesture.pointer_down = false;
    assert_eq!(sent, vec!["set-groove".to_string()]);
    assert_eq!(kick(&mut h, "q.amount"), Value::Number(0.5));
    let half = played(KICK, 2.0).unwrap();
    assert!(
        (half - (2.0 - 0.5 * 0.15 * STEP_BEATS)).abs() < 2.0 / spq,
        "half kick share: {half}"
    );
    check_take(&[2, 6, 10, 14], &hat_heard, HAT);
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), undo_len + 1);
    let sent = h.rack_call(
        &kick_include_on_click,
        vec![Value::Number(0.0), Value::Number(0.0), Value::Nil],
    );
    assert_eq!(sent, vec!["set-groove".to_string()]);
    assert_eq!(
        rack_settings(&h.app).pad(DRUM_RACK_FIRST_PAD_NOTE),
        sequencer::groove::RackGroovePad {
            pad_note: DRUM_RACK_FIRST_PAD_NOTE,
            amount: 0.5,
            enabled: false,
        },
        "excluding keeps the pad's amount"
    );
    assert_eq!(kick(&mut h, "q.enabled"), Value::Bool(false));
    assert_eq!(
        played(KICK, 2.0),
        Some(2.0),
        "an excluded pad plays straight"
    );
    check_take(&[2, 6, 10, 14], &hat_heard, HAT);
    h.undo();
    h.undo();
    assert!(
        rack_settings(&h.app).pads.is_empty(),
        "undo restores every pad"
    );
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 6. The on/off switch bypasses the groove and keeps the selection.
    let sent = h.rack_call(&enabled_on_change, vec![Value::Bool(false)]);
    assert_eq!(sent, vec!["set-groove".to_string()]);
    assert_eq!(h.groove_read("gr.enabled"), Value::Bool(false));
    assert_eq!(
        prop(&h.groove_layout(), "rack-groove-picker", "value"),
        s("Take")
    );
    assert!(played(KICK, 2.0).is_none() && played(HAT, 0.5).is_none());
    assert_eq!(grooved_kick(&mut h), Value::Bool(false));
    h.rack_call(&enabled_on_change, vec![Value::Bool(true)]);
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 7. The ≡ menu acts on the playing groove.
    let pool_len = h.app.grooves.len();
    let sent = h.rack_call(&actions_on_change, vec![s("Duplicate")]);
    assert_eq!(sent, vec!["duplicate-pool-groove".to_string()]);
    assert_eq!(h.app.grooves.len(), pool_len + 1);
    assert_eq!(h.app.grooves.last().unwrap().name, "Take copy");

    // 8. Scale 2×: the take's grid doubles (the picker reads 2 bars · 1/8)
    //    and the early kick of step 8 lands at beat 4, half a slot-width
    //    scaled: the same pocket for the pattern on 1/8 steps.
    let sent = h.rack_call(&scale_on_change, vec![s("2×")]);
    assert_eq!(sent, vec!["set-groove".to_string()]);
    assert_eq!(h.groove_read("gr.grid"), s("2 bars · 1/8"));
    assert_eq!(
        prop(&h.groove_layout(), "rack-groove-scale", "value"),
        s("2×")
    );
    let doubled = played(KICK, 4.0).unwrap();
    assert!(
        (doubled - (4.0 - 0.15 * 2.0 * STEP_BEATS)).abs() < 2.0 / spq,
        "2× kick: {doubled}"
    );
    h.rack_call(&scale_on_change, vec![s("1×")]);
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 9. A role set on the pad (the pad grid's Role ▸ menu) reaches the
    //    kick's lane as its tag; clearing it back to Standard removes it.
    let sent = h.rack_turn("(let ((p (first g.pads))) (set! p.role \"closed-hat\"))");
    assert_eq!(sent, vec!["set-pad".to_string()]);
    let layout = h.groove_layout();
    let badge = find_debug(&layout, "rack-groove-role").expect("the kick's role badge");
    assert_eq!(badge.children[0].props.get("text"), Some(&s("CH")));
    h.rack_turn("(let ((p (first g.pads))) (set! p.role \"\"))");
    assert!(find_debug(&h.groove_layout(), "rack-groove-role").is_none());

    // 10. Clips: grooves are per clip. Converted, a clip shows the rack's
    //     groove until its first edit gives it its own, in that edit's undo
    //     entry: Timing 50% on one clip leaves the other at the rack's; a
    //     75% drag there forks once and leaves the first at 50%. "Use Rack
    //     Groove" hands a clip back; "Apply to All" makes a clip's groove
    //     every clip's.
    let sent = h.rack_turn("(convert-rack-to-clips! g)");
    assert_eq!(sent, vec!["convert-rack-to-clips".to_string()]);
    let verse = h.app.current_rack_clip(group_id).expect("the playing clip");
    let chorus = (h.app.save_rack_clip_as_recorded(group_id, "Chorus")).expect("second clip");
    h.share_buses_and_groups();
    h.sync();
    h.show_all();
    let view_clip = |h: &mut Harness| h.groove_read("(if gr.clip gr.clip.cid -1)");
    let launch = |h: &mut Harness, clip: u64| {
        h.rack_turn(&format!(
            "(launch-rack-clip! (first (filter (lambda (rc) (= rc.cid {clip})) g.clips)))"
        ));
    };
    let set_timing =
        |value: f64| format!("(eseq.rack-groove-buffer/edit-groove! g \"timing\" {value})");
    let timing_of = |app: &app::App, clip: u64| {
        let group = app.groups.iter().find(|g| g.id == group_id).unwrap();
        group
            .rack
            .as_ref()
            .unwrap()
            .groove_for_clip(Some(clip))
            .timing_amount
    };
    let menu = |h: &mut Harness| strings(&h.eval_rack("(eseq.rack-groove-buffer/menu-actions g)"));
    launch(&mut h, verse);
    assert_eq!(
        view_clip(&mut h),
        Value::Number(-1.0),
        "the verse shows the rack's"
    );
    let actions = menu(&mut h);
    assert!(
        actions.contains(&"Apply to All Clips in This Rack".to_string()),
        "{actions:?}"
    );
    assert!(!actions.contains(&"Use Rack Groove for This Clip".to_string()));

    // The first edit forks the clip's own groove and lands on the copy: one
    // command, one undo entry, which undo takes back whole.
    let undo_len = h.app.history.undo_len();
    let sent = h.rack_turn(&set_timing(0.5));
    assert_eq!(sent, vec!["set-groove".to_string()]);
    assert_eq!(h.app.history.undo_len(), undo_len + 1, "one undo entry");
    assert_eq!(
        view_clip(&mut h),
        Value::Number(verse as f64),
        "the edit forked it"
    );
    assert_eq!(h.groove_read("gr.timing"), Value::Number(0.5));
    h.undo();
    h.share_buses_and_groups();
    h.sync();
    assert_eq!(
        view_clip(&mut h),
        Value::Number(-1.0),
        "undo: the verse follows the rack's again"
    );
    assert_eq!(timing_of(&h.app, verse), 1.0);
    h.rack_turn(&set_timing(0.5));
    launch(&mut h, chorus);
    assert_eq!(h.app.current_rack_clip(group_id), Some(chorus));
    assert_eq!(timing_of(&h.app, chorus), 1.0, "the chorus is untouched");
    // A drag through the buffer's own picker forks on its first step: the
    // drag is one entry, and undo leaves the chorus following.
    let timing_on_change = prop(&h.groove_layout(), "rack-groove-timing", "on-change");
    let undo_len = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for value in [0.6, 0.75] {
        let sent = h.rack_call(&timing_on_change, vec![Value::Number(value)]);
        assert_eq!(sent, vec!["set-groove".to_string()]);
    }
    h.gesture.pointer_down = false;
    assert_eq!(
        view_clip(&mut h),
        Value::Number(chorus as f64),
        "the drag forked it"
    );
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(
        h.app.history.undo_len(),
        undo_len + 1,
        "one drag, one entry"
    );
    h.undo();
    h.share_buses_and_groups();
    h.sync();
    assert_eq!(view_clip(&mut h), Value::Number(-1.0), "undo un-forks it");
    let _ = app::edit::redo(&mut h.app);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    assert_eq!(
        timing_of(&h.app, chorus),
        0.75,
        "redo: the drag's latest value"
    );
    launch(&mut h, verse);
    assert_eq!(timing_of(&h.app, verse), 0.5, "the verse kept its own");
    assert_eq!(timing_of(&h.app, chorus), 0.75);
    let rack = h
        .app
        .groups
        .iter()
        .find(|g| g.id == group_id)
        .unwrap()
        .rack
        .clone()
        .unwrap();
    assert_eq!(
        rack.groove.timing_amount, 1.0,
        "the rack's own groove is untouched"
    );

    let actions = menu(&mut h);
    assert!(
        actions.contains(&"Use Rack Groove for This Clip".to_string()),
        "{actions:?}"
    );
    let sent = h.rack_call(
        &actions_on_change,
        vec![s("Apply to All Clips in This Rack")],
    );
    assert_eq!(sent, vec!["apply-rack-groove-to-all-clips".to_string()]);
    assert_eq!(
        (timing_of(&h.app, verse), timing_of(&h.app, chorus)),
        (0.5, 0.5)
    );
    assert_eq!(
        view_clip(&mut h),
        Value::Number(-1.0),
        "every clip follows again"
    );
}

/// The buffer binds what it only draws: the amounts and each pad's share
/// (`#'gr.timing`, `#'q.amount`), the on/off switch and the selected
/// track's lane light, so a drag repaints and rebuilds nothing.
#[test]
fn the_groove_buffer_binds_its_amounts_and_shares() {
    let mut h = distro();
    let (group_id, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    h.app
        .assign_rack_pad_track_recorded(group_id, DRUM_RACK_FIRST_PAD_NOTE, KICK)
        .expect("kick pad");
    h.app.grooves.push(sequencer::groove::ProjectGroove {
        id: 3,
        name: "Swing".to_string(),
        period_beats: 4.0,
        resolution_beats: 0.25,
        pad_rows: Vec::new(),
        shared_row: sequencer::groove::GrooveRow {
            slots: vec![sequencer::groove::GrooveSlot::default(); 16],
        },
    });
    h.share_buses_and_groups();
    h.sync();
    h.show_all();
    h.eval_rack("(def g (first (groups)))");
    h.rack_turn("(let ((gr g.groove)) (set! gr.pool-groove (first project.groove-pool)))");
    let groove = match h.groove_read("gr") {
        Value::Instance(id) => id,
        other => panic!("a groove: {other:?}"),
    };
    let share = match h.groove_read("(first gr.pads)") {
        Value::Instance(id) => id,
        other => panic!("a share: {other:?}"),
    };
    let (tree, revision) = h.buffer_tree("*groove*");
    let (mut bound, mut legacy) = (Vec::new(), Vec::new());
    instance_bindings(&tree, &mut bound, &mut legacy);
    assert_eq!(legacy, Vec::<String>::new(), "legacy bindings");
    for field in ["timing", "velocity", "random", "enabled"] {
        assert!(
            bound.contains(&(groove, field.to_string())),
            "gr.{field}: {bound:?}"
        );
    }
    assert!(bound.contains(&(share, "amount".to_string())), "{bound:?}");
    assert!(
        bound.contains(&(h.track_id(0), "selected".to_string())),
        "{bound:?}"
    );
    // A drag step repaints: the buffer keeps its tree.
    h.rack_turn("(let ((gr (eseq.drum-rack-v2/playing-groove g))) (set! gr.timing 0.5))");
    assert_eq!(h.groove_read("gr.timing"), Value::Number(0.5));
    assert_eq!(
        h.buffer_tree("*groove*").1,
        revision,
        "an amount edit rebuilds nothing"
    );
}
