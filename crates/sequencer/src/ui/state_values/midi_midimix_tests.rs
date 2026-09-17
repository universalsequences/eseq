use super::*;
use crate::midi_dispatch::{dispatch_midi_input, dispatch_midi_to_lisp, register_device_state, reset_midi_port, sync_midi_port};
use sequencer::midi_input::{MidiInputEvent, MidiMessage, MidiNoteEvent};
use sequencer::sequencer::{RollCommand, SequenceRollSource, Timebase};

fn set_field(editor: &mut Editor, field: &str, source: &str) {
    let value = editor.runtime_mut().eval_str(source).unwrap().unwrap();
    editor.runtime_mut().set_reactive("SEQ", field, value);
}

fn fixture() -> Editor {
    fixture_with_state(Arc::new(SequencerState::new(0, vec![])))
}

fn fixture_with_state(state: Arc<SequencerState>) -> Editor {
    // Load the shipped entry point: this also proves automatic installation,
    // module imports and real mixer ordering, without duplicating that policy.
    let mut editor = full_grid_editor_for_scroll_tests();
    register_device_state(editor.runtime_mut());
    crate::roll_input::register_natives(editor.runtime_mut(), state);
    sync_midi_port(&mut editor, 0, Some(("akai-id".into(), "MIDI Mix".into())));
    sync_midi_port(&mut editor, 1, Some(("ni-id".into(), "Komplete Kontrol".into())));
    editor.runtime_mut().register_native("seq-rack-macro-value", |_, _| Ok(Value::Nil));
    for name in ["seq-set-track-volume", "seq-set-bus-volume", "seq-toggle-track-mute",
        "seq-toggle-bus-mute", "seq-toggle-track-solo", "seq-toggle-bus-solo",
        "seq-toggle-record-arm", "seq-toggle-rack-arm"] {
        editor.runtime_mut().register_native(name, move |args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: name.into(),
                payload: Value::List(args.iter().cloned().map(|v| Rc::new(RefCell::new(v))).collect()),
            });
            Ok(Value::Bool(true))
        });
    }
    set_field(&mut editor, "num-tracks", "12");
    set_field(&mut editor, "groups", r#"(list
        (dict :id 90 :bus-id 40 :parent -1 :members '(1 2 5) :anchor 1 :rack false :rack-members '(91))
        (dict :id 91 :bus-id 41 :parent 90 :members '(4) :anchor 4 :rack true)
        (dict :id 92 :bus-id 42 :parent -1 :members '() :anchor -1 :rack true))"#);
    // Storage order intentionally differs from identity and puts master last.
    set_field(&mut editor, "bus-ids", "'(40 2 41 1 42 7 0)");
    set_field(&mut editor, "bus-names", r#"'("Group" "Bus B" "Nested rack" "Bus A" "Rack" "Bus C" "Mix")"#);
    set_field(&mut editor, "track-bus-sends", r#"(map |i| (list
        (dict :bus-id 40 :bus-idx 0) (dict :bus-id 2 :bus-idx 1)
        (dict :bus-id 41 :bus-idx 2) (dict :bus-id 1 :bus-idx 3)
        (dict :bus-id 42 :bus-idx 4) (dict :bus-id 7 :bus-idx 5)) (range 0 12))"#);
    editor.drain_host_commands();
    editor
}

fn cc(editor: &mut Editor, port: usize, controller: u8, value: u8) -> bool {
    dispatch_midi_to_lisp(editor, &MidiInputEvent {
        port, message: MidiMessage::ControlChange { channel: 0, controller, value },
    })
}

fn note(editor: &mut Editor, number: u8, on: bool) {
    assert!(dispatch_midi_to_lisp(editor, &MidiInputEvent {
        port: 0,
        message: MidiMessage::Note { channel: 0, note: MidiNoteEvent {
            note: number, velocity: if on { 1.0 } else { 0.0 }, on,
        } },
    }), "control note {number} must never play an instrument, including releases");
}

fn commands(editor: &mut Editor) -> Vec<(String, Value)> {
    editor.drain_host_commands().into_iter().map(|command| match command {
        HostCommand::Custom { name, payload } => (name, payload),
        other => panic!("unexpected MIDI command: {other:?}"),
    }).collect()
}

fn native(editor: &mut Editor, name: &str, args: &[f64]) {
    let expected = Value::List(args.iter().map(|n| Rc::new(RefCell::new(Value::Number(*n)))).collect());
    assert_eq!(commands(editor), vec![(name.into(), expected)]);
}

fn roll_note(editor: &mut Editor, state: &SequencerState, port: usize, number: u8, on: bool) -> bool {
    dispatch_midi_input(editor, state, &MidiInputEvent {
        port,
        message: MidiMessage::Note { channel: 0, note: MidiNoteEvent {
            note: number, velocity: if on { 1.0 } else { 0.0 }, on,
        } },
    })
}

#[test]
fn midimix_arm_buttons_preselect_rates_and_solo_holds_roll() {
    let state = Arc::new(SequencerState::new(0, vec![]));
    let mut editor = fixture_with_state(state.clone());
    assert!(!state.transport.roll_mode.load(Ordering::Acquire));
    // Prepare every rate before touching SOLO. Track/group targets must never
    // arm, and selecting a rate alone must not start a roll.
    for i in 0..8 {
        assert!(roll_note(&mut editor, &state, 0, 3 + i * 3, true));
        assert!(roll_note(&mut editor, &state, 0, 3 + i * 3, false));
        let rate = Timebase::roll_rate_from_key(char::from(b'1' + i)).unwrap();
        assert_eq!(state.transport.roll_rate.load(Ordering::Acquire), rate as u32);
        assert!(!state.transport.sequence_rolling.load(Ordering::Acquire));
        assert_eq!(state.drain_roll_commands(), vec![RollCommand::SetRate { rate }]);
        assert!(commands(&mut editor).is_empty(), "rate selection must never arm a track");
    }
    assert!(roll_note(&mut editor, &state, 0, 27, true));
    assert!(roll_note(&mut editor, &state, 0, 27, true));
    assert!(state.transport.sequence_rolling.load(Ordering::Acquire));
    assert_eq!(state.transport.roll_rate.load(Ordering::Acquire), Timebase::ThirtySecondTriplet as u32,
        "the roll starts with the preselected rate");
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SequenceRoll { on: true }]);
    // Rates are hardware positions, independent of mixer topology.
    set_field(&mut editor, "num-tracks", "0");
    set_field(&mut editor, "groups", "'()");
    for i in 0..8 {
        assert!(roll_note(&mut editor, &state, 0, 3 + i * 3, true));
        assert!(roll_note(&mut editor, &state, 0, 3 + i * 3, false));
        let rate = Timebase::roll_rate_from_key(char::from(b'1' + i)).unwrap();
        assert_eq!(state.transport.roll_rate.load(Ordering::Acquire), rate as u32);
        assert_eq!(state.drain_roll_commands(), vec![RollCommand::SetRate { rate }]);
    }
    // Same-rate presses still reach the scheduler's stutter reanchor policy.
    assert!(roll_note(&mut editor, &state, 0, 24, true));
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SetRate {
        rate: Timebase::ThirtySecondTriplet,
    }]);
    assert!(commands(&mut editor).is_empty(), "rate buttons must never change arm state");
    assert!(roll_note(&mut editor, &state, 0, 27, false));
    assert!(!state.transport.sequence_rolling.load(Ordering::Acquire));
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SequenceRoll { on: false }]);
    set_field(&mut editor, "num-tracks", "1");
    assert!(roll_note(&mut editor, &state, 0, 3, false));
    assert!(commands(&mut editor).is_empty(), "releasing an old rate button cannot arm");
    assert!(roll_note(&mut editor, &state, 0, 3, true));
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SetRate { rate: Timebase::Quarter }]);
    assert!(commands(&mut editor).is_empty());
    assert!(!state.transport.sequence_rolling.load(Ordering::Acquire));
    assert!(roll_note(&mut editor, &state, 0, 27, true));
    assert_eq!(state.transport.roll_rate.load(Ordering::Acquire), Timebase::Quarter as u32);
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SequenceRoll { on: true }]);
    assert!(roll_note(&mut editor, &state, 0, 27, false));
    assert!(!state.transport.roll_mode.load(Ordering::Acquire), "dedicated MIDI roll leaves keyboard mode alone");
}

#[test]
fn midimix_roll_ownership_survives_overlapping_inputs_and_releases_after_unmapping() {
    let state = Arc::new(SequencerState::new(0, vec![]));
    let mut editor = fixture_with_state(state.clone());
    sync_midi_port(&mut editor, 2, Some(("akai-2".into(), "MIDI Mix".into())));
    assert!(roll_note(&mut editor, &state, 0, 27, true));
    state.drain_roll_commands();
    // Rate buttons on another MIDImix affect the shared rate without taking
    // ownership of the first controller's held roll.
    assert!(roll_note(&mut editor, &state, 2, 3, true));
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SetRate { rate: Timebase::Quarter }]);
    assert!(commands(&mut editor).is_empty());
    assert!(!state.midi_sequence_roll_held(2));
    assert!(roll_note(&mut editor, &state, 2, 27, true));
    let keyboard = SequenceRollSource::Named("keyboard:F2".into());
    state.set_sequence_roll_held(keyboard.clone(), true);
    assert!(state.drain_roll_commands().is_empty(), "overlapping holds do not recapture");
    reset_midi_port(&mut editor, &state, 0);
    assert!(!state.midi_sequence_roll_held(0));
    assert!(state.midi_sequence_roll_held(2));
    assert!(state.transport.sequence_rolling.load(Ordering::Acquire));
    state.set_sequence_roll_held(keyboard, false);
    assert!(state.drain_roll_commands().is_empty(), "keyboard release preserves MIDI hold");
    editor.runtime_mut().eval_str(r#"
        (eseq.midi/midi-unmap
          (eseq.midi/on-device "MIDI Mix" (eseq.midi/on-channel 0 (eseq.midi/note 27))))"#).unwrap();
    assert!(roll_note(&mut editor, &state, 2, 27, false), "unmapped roll release stays consumed");
    assert!(!state.transport.sequence_rolling.load(Ordering::Acquire));
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SequenceRoll { on: false }]);
    // Reconnect/reinstall preserves unconditional rate selection and can roll again.
    sync_midi_port(&mut editor, 0, Some(("akai-id".into(), "MIDI Mix".into())));
    editor.runtime_mut().eval_str(r#"(eseq.midi-midimix/install "MIDI Mix")"#).unwrap();
    assert!(roll_note(&mut editor, &state, 0, 3, true));
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SetRate { rate: Timebase::Quarter }]);
    assert!(commands(&mut editor).is_empty());
    assert!(roll_note(&mut editor, &state, 0, 27, true));
    state.drain_roll_commands();
    reset_midi_port(&mut editor, &state, 0);
    assert!(!state.transport.sequence_rolling.load(Ordering::Acquire));
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SequenceRoll { on: false }]);
}

#[test]
fn midimix_faders_follow_top_level_mixer_order_and_live_topology() {
    let mut editor = fixture();
    for (cc_num, name, target) in [
        (19, "seq-set-track-volume", 0.0), (23, "seq-set-bus-volume", 0.0),
        (27, "seq-set-track-volume", 3.0), (31, "seq-set-track-volume", 6.0),
        (49, "seq-set-track-volume", 7.0), (53, "seq-set-track-volume", 8.0),
        (57, "seq-set-track-volume", 9.0), (61, "seq-set-track-volume", 10.0),
        (62, "seq-set-bus-volume", 6.0),
    ] {
        assert!(cc(&mut editor, 0, cc_num, 64));
        native(&mut editor, name, &[target, 64.0 / 127.0]);
    }
    // Collapsing a group changes no hardware target.
    set_field(&mut editor, "groups", r#"(list
        (dict :id 90 :bus-id 40 :parent -1 :members '(1 2 4 5) :anchor 1 :collapsed true))"#);
    assert!(cc(&mut editor, 0, 27, 0));
    native(&mut editor, "seq-set-track-volume", &[3.0, 0.0]);
    // Ungroup/delete/project replacement takes effect on the next event.
    set_field(&mut editor, "groups", "'()");
    set_field(&mut editor, "num-tracks", "2");
    set_field(&mut editor, "bus-ids", "'()");
    set_field(&mut editor, "bus-names", "'()");
    assert!(cc(&mut editor, 0, 23, 127));
    native(&mut editor, "seq-set-track-volume", &[1.0, 1.0]);
    assert!(cc(&mut editor, 0, 27, 127));
    assert!(commands(&mut editor).is_empty());
    set_field(&mut editor, "groups", r#"(list
        (dict :id 92 :bus-id 42 :parent -1 :members '() :anchor -1 :rack true))"#);
    set_field(&mut editor, "bus-ids", "'(42 0)");
    set_field(&mut editor, "bus-names", r#"'("Rack" "Mix")"#);
    assert!(cc(&mut editor, 0, 27, 127));
    native(&mut editor, "seq-set-bus-volume", &[0.0, 1.0]);
}

#[test]
fn midimix_remaining_strips_follow_visible_bus_order_and_reassign_after_bus_changes() {
    let mut editor = fixture();
    // Four top-level items: track 0, group (including its nested rack), track 3,
    // and an empty rack. The remaining four faders reach B, A, C and Mix.
    set_field(&mut editor, "num-tracks", "6");
    set_field(&mut editor, "bus-ids", "'(0 40 2 41 1 42 7)");
    set_field(&mut editor, "bus-names", r#"'("Mix" "Group" "Bus B" "Nested rack" "Bus A" "Rack" "Bus C")"#);
    for (number, bus) in [(23, 1.0), (31, 5.0), (49, 2.0), (53, 4.0), (57, 6.0), (61, 0.0), (62, 0.0)] {
        assert!(cc(&mut editor, 0, number, 127));
        native(&mut editor, "seq-set-bus-volume", &[bus, 1.0]);
    }
    note(&mut editor, 13, true);
    native(&mut editor, "seq-toggle-bus-mute", &[2.0]);
    note(&mut editor, 14, true);
    native(&mut editor, "seq-toggle-bus-solo", &[2.0]);
    for number in [13, 14, 15] {
        note(&mut editor, number, false);
    }
    note(&mut editor, 15, true);
    for number in [46, 47, 48] {
        assert!(cc(&mut editor, 0, number, 127));
    }
    assert!(commands(&mut editor).is_empty(), "bus knobs/releases are inert and rate buttons never arm");

    // Remove C and reorder the stored buses. Targets follow the live display,
    // and its mixer controls become inert instead of retaining C/Mix.
    set_field(&mut editor, "bus-ids", "'(0 42 1 40 2 41)");
    set_field(&mut editor, "bus-names", r#"'("Mix" "Rack" "Bus A" "Group" "Bus B" "Nested rack")"#);
    for (number, bus) in [(49, 2.0), (53, 4.0), (57, 0.0)] {
        assert!(cc(&mut editor, 0, number, 0));
        native(&mut editor, "seq-set-bus-volume", &[bus, 0.0]);
    }
    for number in [58, 59, 60, 61] {
        assert!(cc(&mut editor, 0, number, 127));
    }
    for number in [22, 23, 24] {
        note(&mut editor, number, true);
        note(&mut editor, number, false);
    }
    assert!(commands(&mut editor).is_empty(), "missing strips never edit mixer or arm state");

    // A project with no tracks/groups still maps its visible Mix strip, even
    // though its bus index is zero (falsy in Lisp).
    set_field(&mut editor, "num-tracks", "0");
    set_field(&mut editor, "groups", "'()");
    set_field(&mut editor, "bus-ids", "'(0)");
    set_field(&mut editor, "bus-names", r#"'("Mix")"#);
    assert!(cc(&mut editor, 0, 19, 64));
    native(&mut editor, "seq-set-bus-volume", &[0.0, 64.0 / 127.0]);
    assert!(cc(&mut editor, 0, 23, 127));
    assert!(commands(&mut editor).is_empty());
}

#[test]
fn midimix_sends_skip_group_buses_and_keyboard_ccs_keep_their_mapping() {
    let mut editor = fixture();
    editor.runtime_mut().eval_str(r#"
        (eseq.midi/midi-map (eseq.midi/cc 16)
          (lambda (value msg) (host-command "keyboard-macro" value)))"#).unwrap();
    for (number, bus) in [(16, 1), (17, 3), (24, 1), (59, 3)] {
        assert!(cc(&mut editor, 0, number, 127));
        let got = commands(&mut editor);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "set-track-bus-send");
        assert_eq!(extract_usize_from_payload(&got[0].1, "bus"), Some(bus));
        assert_eq!(extract_usize_from_payload(&got[0].1, "track"),
            Some(if number == 59 { 10 } else if number == 24 { 3 } else { 0 }));
    }
    for number in [18, 60] {
        assert!(cc(&mut editor, 0, number, 127));
        assert!(commands(&mut editor).is_empty(), "bottom knobs never edit the third send");
    }
    for number in [20, 21, 22] {
        assert!(cc(&mut editor, 0, number, 127));
        assert!(commands(&mut editor).is_empty(), "group knobs are inert");
    }
    set_field(&mut editor, "track-bus-sends", "(list (list (dict :bus-id 1 :bus-idx 3)))");
    assert!(cc(&mut editor, 0, 17, 127));
    assert!(commands(&mut editor).is_empty(), "absent sends are inert");
    assert!(cc(&mut editor, 1, 16, 127));
    assert_eq!(commands(&mut editor), vec![("keyboard-macro".into(), Value::Number(1.0))]);
    assert!(!cc(&mut editor, 1, 23, 127));
    // Identity retirement and slot reuse must not inherit the Akai mapping.
    sync_midi_port(&mut editor, 0, None);
    assert!(!cc(&mut editor, 0, 23, 127));
    sync_midi_port(&mut editor, 0, Some(("other".into(), "Keyboard".into())));
    assert!(!cc(&mut editor, 0, 23, 127));
    sync_midi_port(&mut editor, 5, Some(("akai-id".into(), "MIDI Mix".into())));
    assert!(cc(&mut editor, 5, 19, 127));
    native(&mut editor, "seq-set-track-volume", &[0.0, 1.0]);
}

#[test]
fn midimix_bottom_knobs_follow_strip_rack_macros_and_live_topology() {
    let mut editor = fixture();
    let racks = Arc::new(std::sync::Mutex::new(std::collections::HashSet::from([0, 1, 3, 10])));
    let available = racks.clone();
    editor.runtime_mut().register_native("seq-rack-macro-value", move |args, _| {
        Ok(match (args.first(), args.get(1)) {
            (Some(Value::Number(track)), Some(Value::Number(index)))
                if *index == 0.0 && available.lock().unwrap().contains(&(*track as usize)) => Value::Number(0.0),
            _ => Value::Nil,
        })
    });
    let available = racks.clone();
    editor.runtime_mut().register_native("seq-track-is-rack?", move |args, _| {
        Ok(Value::Bool(matches!(args.first(), Some(Value::Number(track))
            if available.lock().unwrap().contains(&(*track as usize)))))
    });
    editor.runtime_mut().register_native("seq-armed-tracks", |_, _| {
        Ok(Value::List(vec![Rc::new(RefCell::new(Value::Number(10.0)))]))
    });
    set_field(&mut editor, "current-track", "10");
    // Zero-valued macro/track indices and CC values are valid. UI focus and
    // the armed keyboard target must not redirect the strip's macro.
    for (controller, track, raw) in [(18, 0, 0), (26, 3, 64), (60, 10, 127)] {
        assert!(cc(&mut editor, 0, controller, raw));
        let expected = editor.runtime_mut().eval_str(&format!(
            "(dict :track {track} :id 0 :value {})", f64::from(raw) / 127.0)).unwrap().unwrap();
        assert_eq!(commands(&mut editor), vec![("set-rack-macro-value".into(), expected)]);
    }
    for controller in [22, 30, 48, 52, 56] {
        assert!(cc(&mut editor, 0, controller, 127));
        assert!(commands(&mut editor).is_empty(), "groups and non-rack tracks are inert");
    }
    editor.runtime_mut().register_native("seq-has-selection?", |_, _| Ok(Value::Bool(true)));
    assert!(cc(&mut editor, 0, 26, 127));
    let expected = editor.runtime_mut().eval_str("(dict :track 3 :id 0 :value 1)").unwrap().unwrap();
    assert_eq!(commands(&mut editor), vec![("set-rack-macro-plock".into(), expected)]);
    racks.lock().unwrap().remove(&3);
    assert!(cc(&mut editor, 0, 26, 127));
    assert!(commands(&mut editor).is_empty(), "removing the rack clears its macro target");
    // Ungrouping exposes track 1 in strip 2; before this its macro was hidden.
    set_field(&mut editor, "groups", "'()");
    assert!(cc(&mut editor, 0, 22, 0));
    let expected = editor.runtime_mut().eval_str("(dict :track 1 :id 0 :value 0)").unwrap().unwrap();
    assert_eq!(commands(&mut editor), vec![("set-rack-macro-plock".into(), expected)]);
    set_field(&mut editor, "num-tracks", "1");
    assert!(cc(&mut editor, 0, 22, 127));
    assert!(commands(&mut editor).is_empty(), "a bus replacing the track has no rack macro");
}

#[test]
fn midimix_buttons_toggle_only_on_press_and_scene_buttons_keep_quantization() {
    let state = Arc::new(SequencerState::new(0, vec![]));
    let mut editor = fixture_with_state(state.clone());
    for (number, name, target) in [
        (1, "seq-toggle-track-mute", 0.0),
        (4, "seq-toggle-bus-mute", 0.0), (7, "seq-toggle-track-mute", 3.0),
        (22, "seq-toggle-track-mute", 10.0), (2, "seq-toggle-track-solo", 0.0),
        (5, "seq-toggle-bus-solo", 0.0),
    ] {
        note(&mut editor, number, true);
        native(&mut editor, name, &[target]);
        note(&mut editor, number, false);
        assert!(commands(&mut editor).is_empty());
    }
    note(&mut editor, 6, true);
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SetRate { rate: Timebase::QuarterTriplet }]);
    assert!(commands(&mut editor).is_empty(), "plain groups do not arm member tracks");
    set_field(&mut editor, "groups", r#"(list
        (dict :id 90 :bus-id 40 :parent -1 :members '(1 2 4 5) :anchor 1 :rack true))"#);
    note(&mut editor, 6, true);
    assert_eq!(state.drain_roll_commands(), vec![RollCommand::SetRate { rate: Timebase::QuarterTriplet }]);
    assert!(commands(&mut editor).is_empty(), "rack strips select rate without arming the rack");
    set_field(&mut editor, "scene-launch-quantize", "\"1 bar\"");
    for (number, delta) in [(25, -1.0), (26, 1.0)] {
        note(&mut editor, number, true);
        note(&mut editor, number, false);
        let got = commands(&mut editor);
        let expected = editor.runtime_mut().eval_str(&format!(
            "(dict :delta {delta} :quantize \"1 bar\")")).unwrap().unwrap();
        assert_eq!(got, vec![("switch-pattern-relative".into(), expected)]);
    }
    note(&mut editor, 27, true);
    note(&mut editor, 27, false);
    assert!(commands(&mut editor).is_empty());
    let before = editor.runtime_mut().eval_str("(len (eseq.midi/midi-mappings))").unwrap();
    editor.runtime_mut().eval_str("(eseq.midi-midimix/install \"MIDI Mix\")").unwrap();
    assert_eq!(editor.runtime_mut().eval_str("(len (eseq.midi/midi-mappings))").unwrap(), before);
}
