//! Stage 7i: track settings, scales (`tuning`, `degree`), routing (outputs,
//! bus outputs, mod routes and port levels), the project's option lists,
//! the option constants, selection extras and transport/engine extras.

use super::*;
use sequencer::sequencer::{
    BusId, MonoTrigger, StepParam, SwingResolution, Timebase, TrackOutput, VoicePriority,
};

const REFER_7I: &str = "(import eseq.kinds :refer (track tracks buses routes transport \
                        selection engine project reset-tuning! justify-tuning! \
                        randomize-tuning! stretch-tuning! clear-degree! set-bar-transpose! \
                        mod-in-level mute-group-options accum-mode-options tuning-root-options \
                        tuning-mode-options voice-priority-options mono-trigger-options \
                        swing-resolution-options roll-rate-options))";

/// The settings of one track the setters change, for comparing before and
/// after an undo.
#[derive(Debug, PartialEq)]
struct Settings {
    gate: bool,
    poly: bool,
    voices: usize,
    priority: VoicePriority,
    trigger: MonoTrigger,
    mute_group: u8,
    swing: f32,
    resolution: SwingResolution,
    scale: usize,
    accumulator: usize,
    accum_mode: u32,
    accum_limit: f32,
    output: TrackOutput,
}

impl Harness {
    fn eval_7i(&mut self, code: &str) -> Value {
        let source = format!("{REFER_7I}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// `code` raises a Lisp error mentioning `message`.
    fn eval_7i_fails(&mut self, code: &str, message: &str) {
        let source = format!("{REFER_7I}\n{code}");
        match self.editor.runtime_mut().eval_str(&source) {
            Err(error) => {
                let error = format!("{error:?}");
                assert!(error.contains(message), "{code}: {error}");
            }
            Ok(value) => panic!(
                "{code} returned {value:?} (minibuffer {:?})",
                self.editor.minibuffer
            ),
        }
    }

    /// `code` makes a native fail: the error reaches the status line (a
    /// native never raises; it returns false).
    fn native_fails(&mut self, code: &str, message: &str) {
        self.editor.runtime_mut().take_status_message();
        self.eval_7i(code);
        let status = self.editor.runtime_mut().take_status_message();
        let status = status.unwrap_or_default();
        assert!(status.contains(message), "{code}: {status}");
    }

    fn drain_and_sync_7i(&mut self) {
        self.drain();
        self.sync();
    }

    fn settings_pushes(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().settings_pushes
    }

    fn tuning_pushes(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().tuning_pushes
    }

    fn error_7i(&self) -> String {
        self.editor.minibuffer.clone().unwrap_or_default()
    }

    /// Run `code`'s commands; they must fail with `message` and change no
    /// history.
    fn rejects(&mut self, code: &str, message: &str) {
        let undo = self.app.history.undo_len();
        self.editor.minibuffer = None;
        self.eval_7i(code);
        self.drain();
        assert!(
            self.error_7i().contains(message),
            "{code}: {}",
            self.error_7i()
        );
        assert_eq!(self.app.history.undo_len(), undo, "{code} changed nothing");
    }

    fn settings(&self, track: usize) -> Settings {
        let tp = &self.shared.state.pattern.track_params[track];
        Settings {
            gate: tp.is_gate_on(),
            poly: tp.is_polyphonic(),
            voices: tp.get_max_polyphony(),
            priority: tp.get_voice_priority(),
            trigger: tp.get_mono_trigger(),
            mute_group: tp.get_mute_group(),
            swing: tp.get_swing(),
            resolution: tp.get_swing_resolution(),
            scale: tp.get_fts_scale(),
            accumulator: tp.get_accumulator_idx(),
            accum_mode: tp.get_accum_mode(),
            accum_limit: tp.get_accum_limit(),
            output: tp.output(),
        }
    }

    /// A modulator track (appended, position 2) routed into track 0's
    /// input 1 (0-based) through the mixer's command.
    fn with_mod_route() -> Self {
        let mut h = Harness::new();
        h.app
            .graph_controller()
            .add_modulator_track()
            .expect("modulator track");
        h.connect(2, "track", 0, 1);
        h.sync();
        h
    }

    fn connect(&mut self, source: usize, kind: &str, dest: u64, input: usize) {
        let payload = map_value([
            ("source", Value::Number(source as f64)),
            ("dest-kind", s(kind)),
            ("dest", Value::Number(dest as f64)),
            ("input", Value::Number(input as f64)),
        ]);
        self.command("set-mod-route", payload);
    }

    /// A counting native for a view's renders.
    fn render_counter(&mut self, name: &str) -> Rc<std::cell::Cell<u32>> {
        let renders = Rc::new(std::cell::Cell::new(0u32));
        let counter = renders.clone();
        self.editor
            .runtime_mut()
            .register_native(name, move |_args, _ctx| {
                counter.set(counter.get() + 1);
                Ok(Value::Nil)
            });
        renders
    }
}

#[test]
fn track_settings_read_after_sync_and_match_the_model() {
    let mut h = Harness::new();
    let fx = h.add_bus("FX");
    {
        let tp = &h.shared.state.pattern.track_params[1];
        tp.toggle_gate();
        tp.set_max_polyphony(5);
        tp.set_voice_priority(VoicePriority::High);
        tp.set_mono_trigger(MonoTrigger::Legato);
        tp.set_mute_group(3);
        tp.set_swing(62.0);
        tp.set_swing_resolution(SwingResolution::Quarter);
        tp.set_fts_scale(2);
        tp.set_accum_mode(2);
        tp.set_accum_limit(12.0);
        tp.set_output(TrackOutput::Bus(fx));
    }
    h.shared.current_track.store(1, Ordering::Relaxed);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_7i("(def t1 (track 1)) (def tn t1.tuning)");
    let tp = h.settings(1);
    assert_eq!(h.eval_7i("t1.gate"), Value::Bool(tp.gate));
    assert_eq!(h.eval_7i("t1.poly"), Value::Bool(tp.poly));
    assert_eq!(h.eval_7i("t1.max-polyphony"), Value::Number(5.0));
    assert_eq!(h.eval_7i("t1.voice-priority"), s("High"));
    assert_eq!(h.eval_7i("t1.mono-trigger"), s("legato"));
    assert_eq!(h.eval_7i("t1.mute-group"), Value::Number(3.0));
    assert_eq!(h.eval_7i("t1.swing"), Value::Number(62.0));
    assert_eq!(h.eval_7i("t1.swing-resolution"), s("1/4"));
    assert_eq!(h.eval_7i("t1.fts"), s("Minor"));
    assert_eq!(h.eval_7i("t1.accum-mode"), s("rvtz"));
    assert_eq!(h.eval_7i("t1.accum-limit"), Value::Number(12.0));
    assert_eq!(h.eval_7i("t1.output.bid"), Value::Number(fx.0 as f64));
    assert_eq!(h.eval_7i("t1.mod-output"), Value::Bool(false));
    assert_eq!(h.eval_7i("(= tn.track t1)"), Value::Bool(true));
    assert_eq!(h.eval_7i("tn.on"), Value::Bool(true));
    assert_eq!(h.eval_7i("tn.root"), s("C"));
    assert_eq!(h.eval_7i("tn.mode"), s("Snap"));
    assert_eq!(h.eval_7i("(len tn.degrees)"), Value::Number(7.0));
    h.eval_7i("(def d2 (nth tn.degrees 2))");
    assert_eq!(h.eval_7i("(= d2.tuning tn)"), Value::Bool(true));
    assert_eq!(h.eval_7i("d2.index"), Value::Number(2.0));
    assert_eq!(h.eval_7i("tn.morph"), Value::Number(1.0));
    assert_eq!(h.eval_7i("(nth mute-group-options t1.mute-group)"), s("3"));
    // The output choices: every bus, the main mix first (nil is sends only).
    let outputs = h.eval_7i("(map (lambda (b) b.name) project.output-options)");
    let names: Vec<String> = h.app.buses.iter().map(|bus| bus.name.clone()).collect();
    assert_eq!(strings(&outputs)[1..], names[1..]);
    let available = Value::Bool(h.app.graph.track_exposes_mod_output(1));
    assert_eq!(available, h.eval_7i("t1.mod-output"));
    // Transport, engine and selection extras.
    h.shared.state.set_roll_rate(Timebase::EighthTriplet);
    h.sync();
    assert_eq!(h.eval_7i("transport.roll-rate"), s("8T"));
    assert_eq!(h.eval_7i("transport.sequence-rolling"), Value::Bool(false));
    assert_eq!(h.eval_7i("engine.compiling"), Value::Bool(false));
    assert_eq!(h.eval_7i("engine.overloaded"), Value::Bool(false));
    assert_eq!(h.eval_7i("selection.rack-slot"), Value::Number(-1.0));
    assert_eq!(h.eval_7i("selection.auto-follow"), Value::Bool(true));
    *h.shared.auto_follow_override_until.lock().unwrap() =
        Some(Instant::now() + Duration::from_secs(60));
    assert_eq!(h.eval_7i("selection.auto-follow"), Value::Bool(false));
    h.frame.cpu_overload.update(1, Instant::now());
    h.sync();
    assert_eq!(h.eval_7i("engine.overloaded"), Value::Bool(true));
}

#[test]
fn option_constants_match_the_host_lists() {
    let mut h = Harness::new();
    let labels = |items: &[&str]| list_value(items.iter().map(|item| s(item)));
    let rates = Timebase::ROLL_RATES.map(|rate| roll_rate_label(rate as u32));
    let modes = [
        sequencer::scale::TuningMode::Snap.label(),
        sequencer::scale::TuningMode::Map.label(),
    ];
    let cases = [
        ("mute-group-options", build_mute_group_options()),
        ("accum-mode-options", build_accum_mode_options()),
        ("tuning-root-options", build_tuning_root_options()),
        ("tuning-mode-options", labels(&modes)),
        ("voice-priority-options", labels(&VOICE_PRIORITY_LABELS)),
        ("mono-trigger-options", labels(&MONO_TRIGGER_LABELS)),
        ("swing-resolution-options", labels(&SwingResolution::LABELS)),
        ("roll-rate-options", labels(&rates)),
    ];
    for (name, host) in cases {
        assert_eq!(h.eval_7i(name), host, "{name}");
    }
    assert_eq!(TUNING_MODE_LABELS, modes);
    let priorities = [VoicePriority::Last, VoicePriority::High, VoicePriority::Low];
    assert_eq!(priorities.map(voice_priority_label), VOICE_PRIORITY_LABELS);
    let triggers = [MonoTrigger::Retrig, MonoTrigger::Legato];
    assert_eq!(triggers.map(mono_trigger_label), MONO_TRIGGER_LABELS);
}

#[test]
fn project_option_lists_follow_the_buses_and_scripts() {
    let mut h = Harness::new();
    h.sync();
    let entries = |h: &mut Harness, field: &str, count: usize| {
        let code = format!("(list {})", (0..count).map(|i| format!("(nth {field} {i})")).collect::<Vec<_>>().join(" "));
        strings(&h.eval_7i(&code))
    };
    assert_eq!(entries(&mut h, "project.fts-options", 2), ["Off", "Major"]);
    assert_eq!(
        entries(&mut h, "project.sync-options", 8),
        ["Off", "1/16", "1/8", "1/4", "1/2b", "1bar", "2bar", "4bar"]
    );
    assert_eq!(h.eval_7i("(len project.sync-options)"), Value::Number(8.0));
    assert_eq!(
        entries(&mut h, "project.accumulator-options", 3),
        ["Off", "TransposeRamp", "VelocityDecay"]
    );
    assert_eq!(
        h.eval_7i("project.output-options"),
        h.eval_7i("project.buses")
    );
    let fx = h.add_bus("FX");
    h.sync();
    let names = h.eval_7i("(map (lambda (b) b.name) project.output-options)");
    assert!(strings(&names).contains(&"FX".to_string()));
    assert_eq!(
        h.eval_7i("project.output-options"),
        h.eval_7i("project.buses")
    );
    h.app
        .rename_bus_recorded(fx, "Verb".to_string())
        .expect("rename");
    h.share_buses_and_groups();
    h.sync();
    let names = strings(&h.eval_7i("(map (lambda (b) b.name) project.output-options)"));
    assert!(names.contains(&"Verb".to_string()) && !names.contains(&"FX".to_string()));
}

#[test]
fn track_setting_setters_change_the_model_through_history() {
    let mut h = Harness::new();
    let fx = h.add_bus("FX");
    h.sync();
    // Track 1 is not the current track: setters address their own track.
    h.eval_7i(&format!(
        "(def t1 (track 1)) (def fx (first (filter (lambda (b) (= b.bid {})) (buses))))",
        fx.0
    ));
    let before = h.settings(1);
    let track0 = h.settings(0);
    let undo = h.app.history.undo_len();
    h.eval_7i(
        r#"(set! t1.gate true) (set! t1.poly true) (set! t1.max-polyphony 4)
           (set! t1.voice-priority "Low") (set! t1.mono-trigger "legato")
           (set! t1.mute-group 2) (set! t1.swing 60) (set! t1.swing-resolution "1/8")
           (set! t1.fts "Dorian") (set! t1.accum-mode "clip")
           (set! t1.accumulator "TransposeRamp") (set! t1.accum-limit 9)
           (set! t1.output fx)"#,
    );
    h.drain_and_sync_7i();
    let expected = Settings {
        gate: true,
        poly: true,
        voices: 4,
        priority: VoicePriority::Low,
        trigger: MonoTrigger::Legato,
        mute_group: 2,
        swing: 60.0,
        resolution: SwingResolution::Eighth,
        scale: 3,
        accumulator: 1,
        accum_mode: 1,
        accum_limit: 9.0,
        output: TrackOutput::Bus(fx),
    };
    let after = h.settings(1);
    assert_eq!(after, expected);
    let changed = [
        before.gate != after.gate,
        before.poly != after.poly,
        before.voices != after.voices,
        before.priority != after.priority,
        before.trigger != after.trigger,
        before.mute_group != after.mute_group,
        before.swing != after.swing,
        before.resolution != after.resolution,
        before.scale != after.scale,
        before.accum_mode != after.accum_mode,
        before.accumulator != after.accumulator,
        before.accum_limit != after.accum_limit,
        before.output != after.output,
    ];
    let edits = changed.iter().filter(|changed| **changed).count();
    assert_eq!(h.settings(0), track0, "track 0 untouched");
    assert_eq!(h.eval_7i("t1.output"), h.eval_7i("fx"));
    assert_eq!(h.eval_7i("t1.fts"), s("Dorian"));
    assert_eq!(h.eval_7i("t1.accumulator"), s("TransposeRamp"));
    assert!(h.app.history.active_gesture().is_none(), "entries ended");
    assert_eq!(
        h.app.history.undo_len() - undo,
        edits,
        "an undo entry per edit"
    );
    // Absolute: setting what the track has (any case) changes nothing.
    h.eval_7i(
        r#"(set! t1.gate true) (set! t1.swing 60) (set! t1.output fx) (set! t1.fts "dorian")
           (set! t1.voice-priority "low") (set! t1.accumulator "transposeramp")"#,
    );
    h.drain();
    assert_eq!(h.app.history.undo_len() - undo, edits);
    assert_eq!(h.shared.current_track.load(Ordering::Relaxed), 0);
    // nil is sends only; the main mix bus is main.
    h.eval_7i("(set! t1.output nil)");
    h.drain_and_sync_7i();
    assert_eq!(h.settings(1).output, TrackOutput::None);
    assert_eq!(h.eval_7i("t1.output"), Value::Nil);
    h.eval_7i("(set! t1.output (first (buses)))");
    h.drain_and_sync_7i();
    assert_eq!(h.settings(1).output, TrackOutput::Mix);
    // Undo restores every setting.
    for _ in 0..edits + 2 {
        app::edit::undo(&mut h.app);
    }
    assert_eq!(h.settings(1), before);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_7i("t1.output"), h.eval_7i("(first (buses))"));
    assert_eq!(h.eval_7i("t1.gate"), Value::Bool(before.gate));
    // Bars: in range, one undo entry.
    h.eval_7i("(set-bar-transpose! t1 0 5)");
    h.drain_and_sync_7i();
    assert_eq!(h.shared.state.bar_transpose(1, 0), 5.0);
    assert_eq!(h.eval_7i("t1.bar-transposes"), h.eval_7i("(list 5)"));
    assert!(h.app.history.active_gesture().is_none());
    app::edit::undo(&mut h.app);
    assert_eq!(h.shared.state.bar_transpose(1, 0), 0.0);
}

#[test]
fn setters_take_labels_numbers_and_bools_and_reject_the_rest() {
    let mut h = Harness::new();
    h.sync();
    h.eval_7i("(def t1 (track 1)) (def t0 (track 0))");
    let before = h.settings(1);
    for (code, message) in [
        ("(set! t1.swing (/ 0 0))", "a number from 50 to 75"),
        ("(set! t1.swing 80)", "a number from 50 to 75"),
        ("(set! t1.accum-limit -1)", "a number from 0 to 127"),
        ("(set! t1.mute-group 9)", "an integer from 0 to 8"),
        ("(set! t1.max-polyphony 0)", "an integer from 1"),
        (r#"(set! t1.voice-priority "Loud")"#, "one of [\"Last\""),
        (r#"(set! t1.swing-resolution "1/32")"#, "one of"),
        (r#"(set! t1.fts "Klingon")"#, "project.fts-options"),
        (
            r#"(set! t1.accumulator "Nope")"#,
            "project.accumulator-options",
        ),
        (r#"(set! t1.accum-mode "loop")"#, "one of"),
        ("(set-bar-transpose! t1 0 61)", "a number from -60 to 60"),
        ("(set-bar-transpose! t1 99 1)", "an integer from 0"),
    ] {
        h.rejects(code, message);
    }
    assert_eq!(h.settings(1), before);
    // A bus id that is gone.
    let tid = h.app.track_registry.id_at(1).unwrap().0;
    let payload = map_value([
        ("track-id", Value::Number(tid as f64)),
        ("setting", s("output")),
        ("bus-id", Value::Number(999.0)),
    ]);
    h.editor.minibuffer = None;
    h.command("set-track-setting", payload);
    assert!(h.error_7i().contains("no bus"), "{}", h.error_7i());
    // The current value always round-trips, an edited scale's `*` label
    // included.
    h.eval_7i(r#"(set! t1.fts "Major")"#);
    h.drain_and_sync_7i();
    h.eval_7i("(def d2 (nth t1.tuning.degrees 2)) (set! d2.offset 15)");
    h.drain_and_sync_7i();
    assert_eq!(h.eval_7i("t1.fts"), s("Major*"));
    let undo = h.app.history.undo_len();
    h.editor.minibuffer = None;
    h.eval_7i(
        "(set! t1.fts t1.fts) (set! t1.voice-priority t1.voice-priority) \
         (set! t1.accumulator t1.accumulator) (set! t1.swing t1.swing) \
         (set! t1.output t1.output) (set! t1.mute-group t1.mute-group)",
    );
    h.drain();
    assert_eq!(h.error_7i(), "", "the current values are accepted");
    assert_eq!(h.app.history.undo_len(), undo, "and change nothing");
    let offsets = h.shared.state.pattern.track_params[1].tuning().offsets;
    assert_eq!(offsets[2], 15.0);
    // The unedited name is a label too: picking the scale it is (as the
    // dropdown does) keeps its edits.
    h.eval_7i(r#"(set! t1.fts "major")"#);
    h.drain_and_sync_7i();
    assert_eq!(h.error_7i(), "");
    assert_eq!(h.eval_7i("t1.fts"), s("Major*"));
    // Roll rate: the host resolves the label (any case).
    h.eval_7i(r#"(set! transport.roll-rate "16t")"#);
    let raw = h.shared.state.transport.roll_rate.load(Ordering::Relaxed);
    assert_eq!(raw, Timebase::SixteenthTriplet as u32);
    h.native_fails(r#"(set! transport.roll-rate "17")"#, "roll rate label");
    assert_eq!(
        h.shared.state.transport.roll_rate.load(Ordering::Relaxed),
        raw
    );
    // Mod inputs are 1-4.
    h.native_fails("(mod-in-level t0 0)", "inputs are 1-4");
    h.native_fails("(mod-in-level t0 5)", "inputs are 1-4");
    // A value of the wrong type never reaches the host: set! checks the
    // field's type.
    for code in [
        "(set! t1.mute-group 2.5)",
        "(set! t1.gate 1)",
        "(set! t1.voice-priority 1)",
        "(set! t1.delete-target 1)",
        "(set! t1.tuning.root 2)",
        "(set! d2.enabled 0)",
    ] {
        h.eval_7i_fails(code, "got");
    }
    // The host checks what a direct command sends the same way.
    let tid = h.app.track_registry.id_at(1).unwrap().0;
    for (setting, value, message) in [
        ("mute-group", Value::Number(2.5), "an integer from 0 to 8"),
        ("gate", Value::Number(1.0), "true or false"),
        ("voice-priority", Value::Number(1.0), "a label"),
    ] {
        let payload = map_value([
            ("track-id", Value::Number(tid as f64)),
            ("setting", s(setting)),
            ("value", value),
        ]);
        h.editor.minibuffer = None;
        h.command("set-track-setting", payload);
        assert!(
            h.error_7i().contains(message),
            "{setting}: {}",
            h.error_7i()
        );
    }
}

#[test]
fn tuning_fields_set_through_history_and_repaint_only_their_readers() {
    let mut h = Harness::new();
    h.sync();
    h.eval_7i(r#"(def t1 (track 1)) (set! t1.fts "Major")"#);
    h.drain_and_sync_7i();
    h.eval_7i("(def tn t1.tuning) (def d2 (nth tn.degrees 2)) (def d3 (nth tn.degrees 3))");
    assert_eq!(h.eval_7i("tn.scale"), s("Major"));
    let undo = h.app.history.undo_len();
    h.eval_7i(
        r#"(set! tn.root "d") (set! d2.offset 15) (toggle! d2.enabled) (set! tn.mode "map")"#,
    );
    h.drain_and_sync_7i();
    let tuning = h.shared.state.pattern.track_params[1].tuning();
    assert_eq!(tuning.root, 2);
    assert_eq!(tuning.offsets[2], 15.0);
    assert!(!tuning.degree_enabled(2));
    assert_eq!(tuning.mode, sequencer::scale::TuningMode::Map);
    assert_eq!(h.app.history.undo_len() - undo, 4, "one entry per edit");
    assert_eq!(h.eval_7i("tn.root"), s("D"));
    assert_eq!(h.eval_7i("d2.offset"), Value::Number(15.0));
    assert_eq!(h.eval_7i("d2.enabled"), Value::Bool(false));
    assert_eq!(h.eval_7i("tn.edited"), Value::Bool(true));
    assert_eq!(h.eval_7i("t1.fts"), s("Major*"));
    // Absolute: enabling an enabled degree changes nothing.
    h.eval_7i("(set! d3.enabled true)");
    h.drain();
    assert_eq!(h.app.history.undo_len() - undo, 4);
    for (code, message) in [
        ("(set! tn.morph 2)", "a number from 0 to 1"),
        (r#"(set! tn.root "H")"#, "one of"),
        (r#"(set! tn.mode "Bend")"#, "one of"),
        ("(set! d2.offset 1300)", "a number from -1200 to 1200"),
        ("(randomize-tuning! tn 700)", "a number from 0 to 600"),
    ] {
        h.rejects(code, message);
    }
    // Undo restores, the kinds follow.
    for _ in 0..4 {
        app::edit::undo(&mut h.app);
    }
    let tuning = h.shared.state.pattern.track_params[1].tuning();
    assert_eq!((tuning.root, tuning.offsets[2]), (0, 0.0));
    h.sync();
    assert_eq!(h.eval_7i("tn.root"), s("C"));
    assert_eq!(h.eval_7i("d2.offset"), Value::Number(0.0));
    // Whole-scale actions: one entry each.
    let undo = h.app.history.undo_len();
    h.eval_7i("(stretch-tuning! tn 10) (reset-tuning! tn)");
    h.drain_and_sync_7i();
    assert_eq!(h.app.history.undo_len() - undo, 2);
    // Clearing a degree is its own entry, even within a drag on it (the
    // scale editor's right-click; legacy ClearDegree).
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    h.eval_7i("(set! d2.offset 30)");
    h.drain_and_sync_7i();
    h.eval_7i("(clear-degree! d2)");
    h.drain_and_sync_7i();
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(
        h.shared.state.pattern.track_params[1].tuning().offsets[2],
        0.0
    );
    assert_eq!(
        h.app.history.undo_len() - undo,
        2,
        "the drag, then the clear"
    );
    // A #' binding to morph repaints; a morph drag re-renders only the
    // views reading morph by value, not those reading the root.
    let root_renders = h.render_counter("count-root");
    let morph_renders = h.render_counter("count-morph");
    h.eval_7i(
        r#"(effect-buffer "*root*" (do (count-root) (label tn.root)))
           (effect-buffer "*morph*" (do (count-morph) (label (str tn.morph))))
           (def m #'tn.morph)"#,
    );
    h.show_all();
    let (roots, morphs) = (root_renders.get(), morph_renders.get());
    let pushes = h.tuning_pushes();
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for morph in [0.25, 0.5] {
        h.eval_7i(&format!("(set! tn.morph {morph})"));
        h.drain_and_sync_7i();
        h.editor.runtime_mut().run_reactive_cycle();
        assert_eq!(h.slot("m"), morph);
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), undo + 1, "the drag is one entry");
    assert_eq!(h.tuning_pushes(), pushes + 2);
    assert_eq!(
        root_renders.get(),
        roots,
        "the root's reader never re-renders"
    );
    assert_eq!(
        morph_renders.get(),
        morphs + 2,
        "the morph reader re-renders"
    );
}

#[test]
fn a_track_reorder_between_set_and_apply_still_targets_the_set_track() {
    let mut h = Harness::new();
    h.sync();
    h.eval_7i("(def t1 (track 1))");
    let id = h.app.track_registry.id_at(1).unwrap();
    h.eval_7i("(set! t1.mute-group 4) (set-bar-transpose! t1 0 -3)");
    // A track is inserted in front before the commands land.
    h.app.graph_controller().add_empty_track().expect("add");
    h.app
        .graph_controller()
        .move_appended_track_to(0)
        .expect("move");
    h.drain();
    let moved = h.app.track_registry.index_of(id).unwrap();
    assert_eq!(moved, 2);
    let params = &h.shared.state.pattern.track_params;
    assert_eq!(params[moved].get_mute_group(), 4);
    assert_eq!(params[1].get_mute_group(), 0);
    assert_eq!(h.shared.state.bar_transpose(moved, 0), -3.0);
    // A deleted track is an error, not another track's edit.
    h.sync();
    h.eval_7i("(set! t1.mute-group 6)");
    h.app
        .graph_controller()
        .delete_track(moved)
        .expect("delete");
    h.editor.minibuffer = None;
    h.drain();
    assert!(
        h.error_7i().contains("the track is gone"),
        "{}",
        h.error_7i()
    );
    let params = &h.shared.state.pattern.track_params;
    assert!((0..h.app.tracks.len()).all(|track| params[track].get_mute_group() != 6));
}

#[test]
fn script_setting_edits_keep_their_own_entries_beside_a_user_drag() {
    let mut h = Harness::new();
    h.sync();
    h.eval_7i("(def t1 (track 1))");
    // A drag view: swing set!s while the pointer is down join one entry.
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    h.eval_7i("(set! t1.swing 55)");
    h.drain();
    h.eval_7i("(set! t1.swing 58)");
    h.drain();
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.shared.state.pattern.track_params[1].get_swing(), 58.0);
    assert_eq!(h.app.history.undo_len(), undo + 1, "the drag is one entry");
    // A discrete setting never stays open, pointer or not.
    h.gesture.pointer_down = true;
    h.eval_7i("(set! t1.mute-group 1)");
    h.drain();
    assert!(h.app.history.active_gesture().is_none());
    h.gesture.pointer_down = false;
    assert_eq!(h.app.history.undo_len(), undo + 2);
    // A user's fader drag stays one entry around a script edit landing in
    // the middle of it.
    h.gesture.pointer_down = true;
    let fader = |value| app::AppCommand::SetTrackVolume { track: 0, value };
    let volume = h.shared.state.pattern.track_params[0].get_volume();
    app::try_apply_command(&mut h.app, fader(0.3)).expect("drag");
    let drag = h.app.history.active_gesture().map(|gesture| gesture.id);
    assert!(drag.is_some(), "the fader drag's gesture");
    h.eval_7i("(set! t1.mute-group 5)");
    h.drain();
    assert_eq!(h.shared.state.pattern.track_params[1].get_mute_group(), 5);
    assert_eq!(h.app.history.active_gesture().map(|g| g.id), drag);
    app::try_apply_command(&mut h.app, fader(0.2)).expect("drag");
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), undo + 4);
    app::edit::undo(&mut h.app);
    assert_eq!(h.shared.state.pattern.track_params[0].get_volume(), volume);
    app::edit::undo(&mut h.app);
    assert_eq!(h.shared.state.pattern.track_params[1].get_mute_group(), 1);
}

#[test]
fn settings_are_pushed_only_when_they_change() {
    let mut h = Harness::new();
    h.sync();
    h.eval_7i("(def t0 (track 0)) t0.swing");
    let (pushes, tunings) = (h.settings_pushes(), h.tuning_pushes());
    let syncs = h.model_syncs();
    // A model sync for something else pushes no settings and no scale.
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.model_syncs(), syncs + 1);
    assert_eq!(h.settings_pushes(), pushes);
    assert_eq!(h.tuning_pushes(), tunings);
    // A settings edit pushes that track's only; the scale stays.
    h.eval_7i("(set! t0.swing 65)");
    h.drain_and_sync_7i();
    assert_eq!(h.settings_pushes(), pushes + 1);
    assert_eq!(h.tuning_pushes(), tunings);
    assert_eq!(h.eval_7i("t0.swing"), Value::Number(65.0));
    // A scale edit pushes that track's scale only.
    h.eval_7i(r#"(set! t0.fts "Minor")"#);
    h.drain_and_sync_7i();
    assert_eq!(h.tuning_pushes(), tunings + 1);
    assert_eq!(h.settings_pushes(), pushes + 1);
}

#[test]
fn track_outputs_are_buses_by_id_even_with_duplicate_names() {
    let mut h = Harness::new();
    let first = h.add_bus("Dup");
    let second = h.add_bus("Dup");
    assert_ne!(first, second);
    h.sync();
    h.eval_7i(&format!(
        "(def t1 (track 1)) (def second (first (filter (lambda (b) (= b.bid {})) (buses))))",
        second.0
    ));
    h.eval_7i("(set! t1.output second)");
    h.drain_and_sync_7i();
    assert_eq!(h.settings(1).output, TrackOutput::Bus(second));
    assert_eq!(h.eval_7i("t1.output.bid"), Value::Number(second.0 as f64));
    // The legacy dropdown still names it.
    let label = track_output_label(&h.app, &h.shared.state.pattern.track_params[1]);
    assert_eq!(label, "Dup");
}

#[test]
fn routes_and_bus_outputs_read_set_and_keep_identity() {
    let mut h = Harness::with_mod_route();
    let a = h.add_bus("A");
    let b = h.add_bus("B");
    h.connect(2, "bus", a.0, 3);
    h.sync();
    // The route sync reads the connections alone (no graph overrides).
    assert_eq!(
        h.app.state.current_mod_connections(),
        h.app.state.current_scene_metadata().0
    );
    assert_eq!(h.eval_7i("(len (routes))"), Value::Number(2.0));
    h.eval_7i("(def r (first (routes))) (def rb (nth (routes) 1)) (def m (track 2))");
    assert_eq!(h.eval_7i("r.source"), h.eval_7i("m"));
    assert_eq!(h.eval_7i("r.dest"), h.eval_7i("(track 0)"));
    assert_eq!(h.eval_7i("r.dest-bus"), Value::Nil);
    assert_eq!(h.eval_7i("r.input"), Value::Number(2.0), "inputs are 1-4");
    assert_eq!(h.eval_7i("rb.dest"), Value::Nil);
    assert_eq!(h.eval_7i("rb.dest-bus.bid"), Value::Number(a.0 as f64));
    assert_eq!(h.eval_7i("rb.input"), Value::Number(4.0));
    assert_eq!(h.eval_7i("m.mod-output"), Value::Bool(true));
    // Selecting a route is the mixer's delete target.
    h.eval_7i("(set! r.selected true)");
    h.sync();
    assert_eq!(h.eval_7i("r.selected"), Value::Bool(true));
    assert_eq!(h.eval_7i("rb.selected"), Value::Bool(false));
    let target = h.shared.active_delete_target.lock().unwrap().clone();
    let first = h.shared.state.current_mod_connections()[0];
    assert_eq!(target, Some(mod_route_delete_target(&first)));
    h.eval_7i("(set! r.selected false)");
    assert!(h.shared.active_delete_target.lock().unwrap().is_none());
    // Removing the first route moves the other to the front: it keeps its
    // instance (re-keyed), the removed one goes.
    let (route, bus_route) = (h.instance_of(ROUTE, &[0]), h.instance_of(ROUTE, &[1]));
    let payload = map_value([
        ("source", Value::Number(2.0)),
        ("dest-kind", s("track")),
        ("dest", Value::Number(0.0)),
        ("input", Value::Number(1.0)),
    ]);
    h.command("delete-mod-route", payload);
    h.sync();
    assert!(!h.rt().instance_is_live(route));
    assert_eq!(h.instance_of(ROUTE, &[0]), bus_route);
    assert_eq!(h.eval_7i("rb.index"), Value::Number(0.0));
    assert_eq!(h.eval_7i("(routes)"), h.eval_7i("(list rb)"));
    // Re-adding it makes a new route (its id is not reused).
    h.connect(2, "track", 0, 1);
    h.sync();
    assert_eq!(h.eval_7i("(len (routes))"), Value::Number(2.0));
    assert_eq!(h.eval_7i("(= (nth (routes) 1) r)"), Value::Bool(false));
    // Adding a track keeps the routes.
    h.app.graph_controller().add_empty_track().expect("add");
    h.sync();
    assert_eq!(h.instance_of(ROUTE, &[0]), bus_route);
    assert_eq!(h.eval_7i("rb.source"), h.eval_7i("m"));
    // Bus outputs: by bus instance, set by bus ids.
    h.eval_7i(&format!(
        "(def ba (first (filter (lambda (x) (= x.bid {})) (buses))))
         (def bb (first (filter (lambda (x) (= x.bid {})) (buses))))
         (def mix (first (buses)))",
        a.0, b.0
    ));
    assert_eq!(h.eval_7i("ba.output"), h.eval_7i("mix"));
    assert_eq!(h.eval_7i("mix.output"), Value::Nil);
    assert_eq!(h.eval_7i("(len mix.output-options)"), Value::Number(0.0));
    let options = h.eval_7i("(len (filter (lambda (x) (= x bb)) ba.output-options))");
    assert_eq!(options, Value::Number(1.0));
    h.eval_7i("(set! ba.output bb)");
    h.drain_and_sync_7i();
    let bus_a = h.app.buses.iter().find(|bus| bus.id == a).unwrap();
    assert_eq!(bus_output_destination(bus_a), b);
    assert_eq!(h.eval_7i("ba.output"), h.eval_7i("bb"));
    // A cycle is no option: bb may no longer feed ba.
    let options = h.eval_7i("(len (filter (lambda (x) (= x ba)) bb.output-options))");
    assert_eq!(options, Value::Number(0.0));
    app::edit::undo(&mut h.app);
    let bus_a = h.app.buses.iter().find(|bus| bus.id == a).unwrap();
    assert_eq!(bus_output_destination(bus_a), BusId::MIX);
    // A project load replaces the routes.
    h.command("new-project", Value::Nil);
    h.sync();
    assert!(!h.rt().instance_is_live(bus_route));
    assert_eq!(h.eval_7i("(len (routes))"), Value::Number(0.0));
}

#[test]
fn selection_steps_and_delete_targets() {
    let mut h = Harness::new();
    h.sync();
    h.eval_7i("(def t0 (track 0)) (def t1 (track 1))");
    h.editor
        .runtime_mut()
        .set_global_value("cursor-step", Value::Number(3.0));
    let step = |h: &mut Harness, track: usize, step: usize| {
        h.eval_7i(&format!("(nth t{track}.steps {step})"))
    };
    assert_eq!(h.eval_7i("selection.cursor-step"), step(&mut h, 0, 3));
    assert_eq!(h.eval_7i("selection.edit-step"), step(&mut h, 0, 3));
    assert_eq!(h.eval_7i("selection.steps"), h.eval_7i("(list)"));
    h.eval_7i(
        r#"(effect-buffer "*selection*"
             (label (str selection.steps selection.cursor-step selection.edit-step)))"#,
    );
    h.show_all();
    h.shared.selected_steps.lock().unwrap().extend([7, 5]);
    h.sync();
    assert_eq!(h.eval_7i("selection.edit-step"), step(&mut h, 0, 5));
    let picked = h.eval_7i("(list (nth t0.steps 5) (nth t0.steps 7))");
    assert_eq!(h.eval_7i("selection.steps"), picked);
    // The step panel's cursor and edited step (the legacy
    // `SEQ.fx-step-*` fields, eseq-0l17.78).
    assert_eq!(h.eval_7i("selection.cursor-step.index"), Value::Number(3.0));
    assert_eq!(h.eval_7i("selection.edit-step.index"), Value::Number(5.0));
    assert_eq!(h.eval_7i("(len selection.steps)"), Value::Number(2.0));
    // Mixer delete targets: what a setter writes, the field reads.
    let target = |h: &Harness| h.shared.active_delete_target.lock().unwrap().clone();
    h.eval_7i("(set! t1.delete-target true)");
    h.sync();
    assert_eq!(h.eval_7i("t1.delete-target"), Value::Bool(true));
    assert_eq!(h.eval_7i("t0.delete-target"), Value::Bool(false));
    h.eval_7i("(set! t1.delete-target false)");
    assert_eq!(h.eval_7i("t1.delete-target"), Value::Bool(false));
    assert_eq!(target(&h), None);
    *h.shared.active_delete_target.lock().unwrap() =
        Some(ActiveDeleteTarget::MixerTracks { tracks: vec![0, 1] });
    assert_eq!(h.eval_7i("t0.delete-target"), Value::Bool(true));
    // true on a member keeps the multi-track target.
    h.eval_7i("(set! t1.delete-target true)");
    assert_eq!(
        target(&h),
        Some(ActiveDeleteTarget::MixerTracks { tracks: vec![0, 1] })
    );
    // false takes t1 out of it; t0 stays.
    h.eval_7i("(set! t1.delete-target false)");
    assert_eq!(h.eval_7i("t1.delete-target"), Value::Bool(false));
    assert_eq!(h.eval_7i("t0.delete-target"), Value::Bool(true));
    assert_eq!(
        target(&h),
        Some(ActiveDeleteTarget::MixerTrack { track: 0 })
    );
    let mut three = Some(ActiveDeleteTarget::MixerTracks {
        tracks: vec![0, 1, 2],
    });
    assert!(set_track_delete_target(&mut three, 1, false));
    assert_eq!(
        three,
        Some(ActiveDeleteTarget::MixerTracks { tracks: vec![0, 2] })
    );
    assert!(!set_track_delete_target(&mut three, 1, false));
}

#[test]
fn the_cursor_step_setter_moves_the_grid_cursor_to_the_steps_track() {
    let mut h = Harness::with_root(UiRoot::Distro);
    h.sync();
    let state = h.shared.state.clone();
    state.pattern.step_data[0].set(9, StepParam::Velocity, 0.25);
    state.pattern.step_data[1].set(9, StepParam::Velocity, 0.75);
    h.eval_7i("(def t1 (track 1))");
    let undo = h.app.history.undo_len();
    h.eval_7i("(set! selection.cursor-step (nth t1.steps 9))");
    h.drain();
    h.sync();
    assert_eq!(h.shared.current_track.load(Ordering::Relaxed), 1);
    assert_eq!(fx_step_cursor_from_runtime(h.rt()), 9);
    let cursor = h.eval_7i("selection.cursor-step");
    assert_eq!(cursor, h.eval_7i("(nth t1.steps 9)"));
    // The step panel shows the new track's step.
    assert_eq!(h.eval_7i("selection.edit-step"), cursor);
    assert_eq!(
        h.eval_7i("(let ((s selection.edit-step)) s.velocity)"),
        Value::Number(0.75)
    );
    // The grid's cursor moved, as a click on the step moves it.
    assert_eq!(
        h.eval_7i("(let ((c eseq.sequencer/grid-cursor)) c.step)"),
        Value::Number(9.0)
    );
    assert_eq!(
        h.eval_7i("(eseq.sequencer/track-cursor t1)"),
        Value::Number(9.0)
    );
    let tid = h.app.track_registry.id_at(1).unwrap().0;
    assert_eq!(h.app.history.undo_len(), undo, "no history");
    // A step past the track's length is an error.
    h.editor.minibuffer = None;
    let payload = map_value([
        ("track-id", Value::Number(tid as f64)),
        ("step", Value::Number(999.0)),
    ]);
    h.command("set-cursor-step", payload);
    assert!(
        h.error_7i().contains("cursor-step takes"),
        "{}",
        h.error_7i()
    );
}

#[test]
fn new_live_fields_are_computed_only_while_observed() {
    let mut h = Harness::with_mod_route();
    h.add_bus("FX");
    h.sync();
    h.eval_7i("(def t0 (track 0)) (def r (first (routes))) (def fx (nth (buses) 1))");
    let levels = |level: f64, h: &Harness| ModPortLevels {
        track_inputs: vec![[level; 4]; h.app.tracks.len()],
        track_outputs: vec![level; h.app.tracks.len()],
        bus_inputs: h
            .app
            .buses
            .iter()
            .map(|bus| (bus.id.0, [level; 4]))
            .collect(),
    };
    let keys = [
        f::TRACK_MOD_IN[1],
        f::TRACK_MOD_OUT_LEVEL,
        f::BUS_MOD_IN[0],
        f::TRACK_BAR_TRANSPOSES,
        f::TRACK_DELETE_TARGET,
        f::ROUTE_SELECTED,
        f::TRANSPORT_ROLL_RATE,
        f::TRANSPORT_SEQUENCE_ROLLING,
        f::ENGINE_OVERLOADED,
        f::SELECTION_AUTO_FOLLOW,
        f::SELECTION_STEPS,
        f::SELECTION_CURSOR_STEP,
        f::SELECTION_EDIT_STEP,
    ];
    for tick in 0..6 {
        h.meters.cached_mod_port_levels = levels(tick as f64 / 8.0, &h);
        h.shared.state.set_roll_rate(Timebase::ROLL_RATES[tick % 8]);
        h.shared.selected_steps.lock().unwrap().insert(tick);
        h.sync();
    }
    for key in keys {
        assert_eq!(h.computed(key), 0, "{key:?} computed while unobserved");
    }
    assert!(!h.frame.host_kinds.wants_mod_levels());
    // Observing one track's port (input 2) and a bus's (input 1): they
    // follow, and the tick keeps the levels polled.
    h.eval_7i("(def port (mod-in-level t0 2)) (def bport (mod-in-level fx 1))");
    h.meters.cached_mod_port_levels = levels(0.5, &h);
    h.sync();
    assert!(h.frame.host_kinds.wants_mod_levels());
    assert_eq!(h.slot("port"), 0.5);
    assert_eq!(h.slot("bport"), 0.5);
    assert_eq!(h.computed(f::TRACK_MOD_IN[0]), 0, "only the observed input");
    h.eval_7i("(set! port nil) (set! bport nil)");
    h.sync();
    h.sync();
    assert!(!h.frame.host_kinds.wants_mod_levels());
    // One observed route of several: one computation per tick.
    h.connect(2, "track", 1, 0);
    h.sync();
    assert_eq!(h.eval_7i("(len (routes))"), Value::Number(2.0));
    h.eval_7i("(def sel #'r.selected)");
    h.sync();
    let seeded = h.computed(f::ROUTE_SELECTED);
    for _ in 0..4 {
        h.sync();
    }
    assert_eq!(h.computed(f::ROUTE_SELECTED), seeded + 4);
    // Bar transposes and the step cursor: computed while observed, but the
    // cursor only when it may have moved.
    h.eval_7i(
        r#"(effect-buffer "*observe*"
             (label (str t0.bar-transposes selection.cursor-step)))"#,
    );
    h.show_all();
    h.sync();
    let bars = h.computed(f::TRACK_BAR_TRANSPOSES);
    let cursor = h.computed(f::SELECTION_CURSOR_STEP);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(
        h.computed(f::SELECTION_CURSOR_STEP),
        cursor,
        "the cursor is recomputed only when it may have moved"
    );
    assert_eq!(h.computed(f::TRACK_BAR_TRANSPOSES), bars + 3);
    h.editor
        .runtime_mut()
        .set_global_value("cursor-step", Value::Number(2.0));
    h.sync();
    let moved = h.computed(f::SELECTION_CURSOR_STEP);
    assert!(moved > cursor, "a moved cursor is recomputed");
    h.sync();
    h.sync();
    assert_eq!(h.computed(f::SELECTION_CURSOR_STEP), moved);
}

#[test]
fn setting_locks_show_the_displayed_steps_track_level_locks() {
    // eseq-0l17.61: the track panel's timebase, swing and swing resolution
    // show a p-lock at the current track's displayed step (the legacy
    // tp-timebase / tp-swing / tp-swing-resolution); the fields stay the
    // track's own.
    let mut h = Harness::new();
    h.sync();
    let pattern = &h.shared.state.pattern;
    pattern.swing_plocks[0].set(3, 70.0);
    pattern.timebase_plocks[0].set(3, Timebase::Eighth);
    pattern.swing_resolution_plocks[0].set(5, SwingResolution::Eighth);
    h.eval_7i("(def t0 (track 0))");
    // Nothing displayed: no lock shows.
    h.shared.current_track.store(0, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_7i("t0.setting-locks"), list_value([]));
    h.shared.selected_steps.lock().unwrap().insert(3);
    h.sync();
    assert_eq!(
        h.eval_7i("(map (lambda (row) (list (get row :name) (get row :value))) t0.setting-locks)"),
        h.eval_7i(&format!(
            "(list (list \"timebase\" \"{}\") (list \"swing\" 70))",
            Timebase::Eighth.label()
        ))
    );
    assert_ne!(
        h.eval_7i("t0.swing"),
        Value::Number(70.0),
        "the field is the track's own"
    );
    // Another track shows none (the display step is the current track's).
    h.shared.current_track.store(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_7i("t0.setting-locks"), list_value([]));
}
