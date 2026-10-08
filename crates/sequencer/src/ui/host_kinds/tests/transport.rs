//! The factory transport, scene banks and MIDI capture views, ported to the
//! kinds (kind-bindings spec §13 stage 8, eseq-0l17.12).

use super::views::{assert_ported, distro, instance_bindings, legacy_forms, widgets_with_prop};
use super::*;

/// The ported views' sources.
const PORTED: [(&str, &str); 3] = [
    (
        "ui/transport.lisp",
        include_str!("../../../../../../content/ui/transport.lisp"),
    ),
    (
        "ui/scene-banks.lisp",
        include_str!("../../../../../../content/ui/scene-banks.lisp"),
    ),
    (
        "ui/retrospective.lisp",
        include_str!("../../../../../../content/ui/retrospective.lisp"),
    ),
];

#[test]
fn ported_transport_views_use_no_legacy_binding_forms() {
    assert_ported(&PORTED);
    // The scanner sees through comments, never strings.
    assert_eq!(
        legacy_forms(";; SEQ.playing (bind-seq \"x\")\n(label \"SEQ.x\")"),
        Vec::<&str>::new()
    );
    assert_eq!(legacy_forms("(if SEQ.playing 1 0)"), ["SEQ."]);
    assert_eq!(
        legacy_forms("(defwidget w :state (a) :bindable (a))"),
        [":bindable"]
    );
    // The generic `(bind "NS" field)`, not a surface's `(bind name)`.
    assert_eq!(legacy_forms("(bind \"SEQV\" (field gid))"), ["(bind "]);
    assert_eq!(
        legacy_forms("(def bind (name) 0) (knob :value (bind \"x\") :y (bind name))"),
        Vec::<&str>::new()
    );
}

#[test]
fn the_transport_binds_its_host_state_through_kinds_and_only_repaints_on_playback() {
    let mut h = distro();
    let (tree, revision) = h.buffer_tree("*transport*");
    let (mut bound, mut legacy) = (Vec::new(), Vec::new());
    instance_bindings(&tree, &mut bound, &mut legacy);
    assert_eq!(legacy, Vec::<String>::new(), "legacy bindings");
    let (transport, master, engine, song) = (
        h.singleton(TRANSPORT),
        h.singleton(MASTER),
        h.singleton(ENGINE),
        h.singleton(SONG),
    );
    for (id, field) in [
        (transport, "playing"),
        (transport, "recording"),
        (transport, "position"),
        (transport, "metronome"),
        (transport, "roll-mode"),
        (master, "recording"),
        (master, "peak-l"),
        (master, "peak-r"),
        (engine, "cpu-load"),
        (engine, "latency-ms"),
        (engine, "overloaded"),
        (song, "cursor"),
        (song, "manual-latch"),
    ] {
        assert!(
            bound.contains(&(id, field.to_string())),
            "{field} is bound: {bound:?}"
        );
    }
    // Each scene pill binds its scene's `active`.
    let scenes = h.eval("(scenes)");
    let scenes = h.instances(scenes);
    assert!(!scenes.is_empty());
    for scene in &scenes {
        assert!(bound.contains(&(*scene, "active".to_string())));
    }
    let mut pills = Vec::new();
    widgets_with_prop(&tree, "drag-type", &mut pills);
    assert_eq!(
        pills.len(),
        scenes.len(),
        "a pill per scene of the viewed bank"
    );

    // Playback, the CPU readout and the master meter repaint; the transport
    // never re-renders for them.
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    h.meters.cached_cpu_load_bits = 37.0f32.to_bits();
    assert!(h.sync());
    h.show_all();
    h.editor.refresh_runtime_side_effects();
    assert_eq!(h.eval("transport.playing"), Value::Bool(true));
    assert_eq!(
        h.buffer_tree("*transport*").1,
        revision,
        "playback only repaints"
    );
}

#[test]
fn a_loaded_project_shows_the_bank_of_its_playing_scene() {
    let mut h = distro();
    let mut project = h.app.capture_export_project().unwrap();
    // Two banks with the saved current scene in the second.
    project.patterns = vec![project.patterns[0].clone(); 28];
    project.scene_banks = vec![
        sequencer::project::ProjectSceneBank {
            id: 1,
            name: None,
            len: 10,
        },
        sequencer::project::ProjectSceneBank {
            id: 2,
            name: None,
            len: 18,
        },
    ];
    project.current_pattern = 11;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bank-load.json");
    std::fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
    let viewed = |h: &mut Harness| h.eval("(eseq.scene-banks/scene-viewed-bank-index)");
    for _ in 0..2 {
        h.eval("(eseq.transport/select-scene-bank \"A\")");
        assert_eq!(viewed(&mut h), Value::Number(0.0));
        h.app
            .queue_project_load_from_path("bank-load", &path)
            .unwrap();
        while h.app.has_pending_project_load() {
            h.app.advance_pending_project_load().unwrap();
        }
        assert_eq!(h.app.state.current_scene_index(), 11);
        // Reloading (the same file too) replaces the bank instances, so the
        // strip shows the loaded scene's bank again.
        h.sync();
        h.show_all();
        assert_eq!(viewed(&mut h), Value::Number(1.0));
        let (tree, _) = h.buffer_tree("*transport*");
        let mut pills = Vec::new();
        widgets_with_prop(&tree, "drag-type", &mut pills);
        assert_eq!(pills.len(), 18, "bank B's scenes");
        assert!(pills
            .iter()
            .any(|pill| pill.get("scene") == Some(&Value::Number(11.0))));
    }
}

#[test]
fn a_scene_launch_only_repaints_the_transport() {
    let mut h = distro();
    h.command("clone-pattern", Value::Nil);
    h.eval("(launch! (first (scenes)))");
    h.drain();
    h.sync();
    h.show_all();
    assert_eq!(h.app.state.current_scene_index(), 0);
    let (_, revision) = h.buffer_tree("*transport*");
    // The pills bind `s.active`: a launch moves the lit pill by repainting.
    h.eval("(launch! (nth (scenes) 1))");
    h.drain();
    assert_eq!(h.app.state.current_scene_index(), 1);
    h.sync();
    h.show_all();
    assert_eq!(
        h.eval("(let ((s (nth (scenes) 1))) s.active)"),
        Value::Bool(true)
    );
    assert_eq!(
        h.buffer_tree("*transport*").1,
        revision,
        "a launch only repaints"
    );
}

#[test]
fn a_removed_viewed_bank_falls_back_to_the_previous_one() {
    // scene-banks spec §4: a structural edit (here an undo) that removes the
    // viewed bank shows the bank at its index, clamped: the previous one.
    let mut h = distro();
    h.command("create-scene-bank", Value::Nil);
    h.command("create-scene-bank", Value::Nil);
    h.sync();
    h.show_all();
    let viewed = |h: &mut Harness| h.eval("(eseq.scene-banks/scene-viewed-bank-index)");
    h.eval("(eseq.transport/select-scene-bank \"C\")");
    assert_eq!(viewed(&mut h), Value::Number(2.0));
    assert!(matches!(
        sequencer::app::edit::undo(&mut h.app),
        sequencer::app::history::HistoryReplay::Applied(_)
    ));
    h.sync();
    h.show_all();
    assert_eq!(h.eval("(len (banks))"), Value::Number(2.0));
    assert_eq!(
        viewed(&mut h),
        Value::Number(1.0),
        "bank B, not the playing A"
    );
}

/// The scene launch quantization round-trips through the host kinds' UI
/// state of record (eseq-0l17.78; it was `SEQ.scene-launch-quantize`):
/// `set-scene-launch-quantize` sets `HostKinds`, the next sync pushes
/// `transport.launch-quantize`, an unknown label changes nothing, and a
/// new project keeps it (it is not saved in the project).
#[test]
fn scene_launch_quantize_round_trips_through_the_host_kinds_field() {
    use sequencer::quantized_launch::LaunchQuantize;
    let mut h = Harness::new();
    h.sync();
    assert_eq!(h.frame.host_kinds.scene_launch_quantize(), LaunchQuantize::Off);
    assert_eq!(h.eval("transport.launch-quantize"), Value::String("off".into()));
    for (label, quantize, shown) in [
        ("1/4", LaunchQuantize::Quarter, "1/4"),
        ("bar", LaunchQuantize::Bar, "1 bar"),
        ("1/16", LaunchQuantize::Sixteenth, "1/16"),
    ] {
        h.command("set-scene-launch-quantize", Value::String(label.into()));
        assert_eq!(h.frame.host_kinds.scene_launch_quantize(), quantize, "{label}");
        h.sync();
        assert_eq!(
            h.eval("transport.launch-quantize"),
            Value::String(shown.into()),
            "{label}"
        );
    }
    h.command("set-scene-launch-quantize", Value::String("1/3".into()));
    assert_eq!(
        h.frame.host_kinds.scene_launch_quantize(),
        LaunchQuantize::Sixteenth,
        "an unknown label changes nothing"
    );
    h.command("new-project", Value::Nil);
    h.sync();
    assert_eq!(
        h.eval("transport.launch-quantize"),
        Value::String("1/16".into()),
        "a new project keeps the UI setting"
    );
}
