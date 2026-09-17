//! Hands hardware MIDI messages to the Lisp mapping layer
//! (`content/ui/midi.lisp`, module `eseq.midi`). Runs on the UI thread from
//! the event loop's per-iteration MIDI drain.

use super::*;
use sequencer::midi_input::{MidiInputEvent, MidiMessage};

const DISPATCH_GLOBAL: &str = "eseq.midi/dispatch";

/// Input lifecycle precedes the mutable Lisp mapping table, so a physical
/// release cannot be lost when the user remaps its button mid-hold.
pub(crate) fn dispatch_midi_input(editor: &mut Editor, state: &SequencerState, event: &MidiInputEvent) -> bool {
    let released_roll = crate::roll_input::release_midi_note(state, event);
    dispatch_midi_to_lisp(editor, event) || released_roll
}

pub(crate) fn reset_midi_port(editor: &mut Editor, state: &SequencerState, port: usize) {
    state.release_midi_sequence_rolls(port);
    sync_midi_port(editor, port, None);
}

pub(crate) fn register_device_state(runtime: &mut eseqlisp::Runtime) {
    // Presentation state only. Device changes always go through host commands;
    // writable fields also let authoring/capture scripts preview device states.
    runtime.register_reactive("MIDI", vec![
        ("devices", Value::List(vec![])),
        ("ports", Value::List((0..sequencer::midi_input::MAX_INPUT_PORTS)
            .map(|_| cell(Value::Nil)).collect())),
        ("error", Value::String(String::new())),
        ("persistent", Value::Bool(sequencer::midi_input::service::persistent_device_ids())),
    ], true);
}

fn cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
}

/// Connection identity follows the same ordered stream as note/controller
/// events, never the asynchronous discovery snapshot or enumeration order.
pub(crate) fn sync_midi_port(editor: &mut Editor, port: usize, identity: Option<(String, String)>) {
    let value = identity.map_or(Value::Nil, |(id, name)| Value::Map(HashMap::from([
        ("id".into(), cell(Value::String(id))),
        ("name".into(), cell(Value::String(name))),
    ])));
    editor.runtime_mut().set_reactive_list_index("MIDI", "ports", port, value);
}

pub(crate) fn sync_midi_devices(editor: &mut Editor, snapshot: sequencer::midi_input::service::Snapshot) {
    let devices = snapshot.devices.into_iter().map(|device| {
        Value::Map(HashMap::from([
            ("id".into(), cell(Value::String(device.id))),
            ("name".into(), cell(Value::String(device.name))),
            ("enabled".into(), cell(Value::Bool(device.enabled))),
            ("connected".into(), cell(Value::Bool(device.connected))),
            ("status".into(), cell(Value::String(device.status))),
        ]))
    }).map(cell).collect();
    editor.runtime_mut().set_reactive("MIDI", "devices", Value::List(devices));
    editor.runtime_mut().set_reactive("MIDI", "error", Value::String(snapshot.error));
    // Device discovery must repaint even while the transport and meters are idle.
    editor.runtime_mut().run_reactive_cycle();
    editor.mark_needs_redraw();
}

/// The message map `eseq.midi/dispatch` receives. Keys match the doc block
/// at the top of `content/ui/midi.lisp`.
pub(crate) fn midi_message_value(event: &MidiInputEvent) -> Value {
    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    map.insert("port".into(), cell(Value::Number(event.port as f64)));
    match event.message {
        MidiMessage::Note { channel, note } => {
            map.insert(
                "kind".into(),
                cell(Value::Keyword(
                    if note.on { "note-on" } else { "note-off" }.into(),
                )),
            );
            map.insert("channel".into(), cell(Value::Number(f64::from(channel))));
            map.insert("note".into(), cell(Value::Number(f64::from(note.note))));
            map.insert(
                "velocity".into(),
                cell(Value::Number(f64::from(note.velocity))),
            );
            map.insert(
                "value".into(),
                cell(Value::Number(f64::from(note.velocity))),
            );
        }
        MidiMessage::ControlChange {
            channel,
            controller,
            value,
        } => {
            map.insert("kind".into(), cell(Value::Keyword("cc".into())));
            map.insert("channel".into(), cell(Value::Number(f64::from(channel))));
            map.insert("cc".into(), cell(Value::Number(f64::from(controller))));
            map.insert("raw".into(), cell(Value::Number(f64::from(value))));
            map.insert(
                "value".into(),
                cell(Value::Number(f64::from(value) / 127.0)),
            );
        }
        MidiMessage::PitchBend { channel, value } => {
            map.insert("kind".into(), cell(Value::Keyword("pitch-bend".into())));
            map.insert("channel".into(), cell(Value::Number(f64::from(channel))));
            map.insert("value".into(), cell(Value::Number(f64::from(value))));
        }
        MidiMessage::PolyPressure { channel, note, value } => {
            map.insert("kind".into(), cell(Value::Keyword("poly-pressure".into())));
            map.insert("channel".into(), cell(Value::Number(f64::from(channel))));
            map.insert("note".into(), cell(Value::Number(f64::from(note))));
            map.insert("value".into(), cell(Value::Number(f64::from(value))));
        }
        MidiMessage::Aftertouch { channel, value } => {
            map.insert("kind".into(), cell(Value::Keyword("aftertouch".into())));
            map.insert("channel".into(), cell(Value::Number(f64::from(channel))));
            map.insert("value".into(), cell(Value::Number(f64::from(value))));
        }
    }
    Value::Map(map)
}

/// Offer one message to Lisp. Returns true when a mapping consumed it, in
/// which case the caller must not also feed it to the live keyboard. A
/// missing module (bare test editors, a broken init) means nothing is
/// consumed; a Lisp error is surfaced through the runtime's status line and
/// the message falls through. The caller runs `refresh_runtime_side_effects`
/// once per drained batch rather than per message.
pub(crate) fn dispatch_midi_to_lisp(editor: &mut Editor, event: &MidiInputEvent) -> bool {
    let Some(callable) = editor.runtime_mut().global_value(DISPATCH_GLOBAL) else {
        return false;
    };
    let Value::Map(mut message) = midi_message_value(event) else { unreachable!() };
    if let Some(Value::List(ports)) = editor.runtime().reactive_field_value("MIDI", "ports") {
        if let Some(port) = ports.get(event.port) {
            if let Value::Map(identity) = &*port.borrow() {
                for (source, target) in [("id", "device-id"), ("name", "device-name")] {
                    if let Some(value) = identity.get(source) {
                        message.insert(target.into(), value.clone());
                    }
                }
            }
        }
    }
    let consumed = match editor
        .runtime_mut()
        .invoke(callable, vec![Value::Map(message)])
    {
        Ok(Some(Value::Bool(consumed))) => consumed,
        Ok(_) => false,
        Err(error) => {
            editor.show_transient_message(format!("midi dispatch: {error:?}"));
            false
        }
    };
    consumed
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::{EditorConfig, Runtime};
    use sequencer::midi_input::MidiNoteEvent;

    /// Bare editor with `eseq.midi` loaded and the three sequencer natives it
    /// reads stubbed: track 2 is an armed rack whose macros all read 0.5.
    fn editor_with_midi_module() -> Editor {
        let mut runtime = Runtime::new();
        runtime.register_native("seq-armed-tracks", |_args, _ctx| {
            Ok(Value::List(vec![
                cell(Value::Number(0.0)),
                cell(Value::Number(2.0)),
            ]))
        });
        runtime.register_native("seq-track-is-rack?", |args, _ctx| {
            Ok(Value::Bool(matches!(args.first(), Some(Value::Number(t)) if *t == 2.0)))
        });
        runtime.register_native("seq-rack-macro-value", |args, _ctx| {
            Ok(match (args.first(), args.get(1)) {
                (Some(Value::Number(track)), Some(Value::Number(index)))
                    if *track == 2.0 && (0.0..8.0).contains(index) => Value::Number(0.5),
                _ => Value::Nil,
            })
        });
        runtime.register_native("seq-has-selection?", |_args, _ctx| Ok(Value::Bool(false)));
        let mut editor = Editor::new(runtime, EditorConfig::default());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/ui/midi.lisp");
        let source = std::fs::read_to_string(&path).expect("read content/ui/midi.lisp");
        editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("load eseq.midi: {error:?}"));
        let _ = editor.drain_host_commands();
        editor
    }

    fn cc(controller: u8, value: u8) -> MidiInputEvent {
        MidiInputEvent {
            port: 0,
            message: MidiMessage::ControlChange {
                channel: 0,
                controller,
                value,
            },
        }
    }

    fn rack_macro_commands(editor: &mut Editor) -> Vec<(usize, usize, f64)> {
        editor
            .drain_host_commands()
            .into_iter()
            .filter_map(|command| match command {
                HostCommand::Custom { name, payload } if name == "set-rack-macro-value" => {
                    let Value::Map(map) = payload else { return None };
                    let number = |key: &str| match map.get(key).map(|v| v.borrow().clone()) {
                        Some(Value::Number(n)) => n,
                        other => panic!("{key}: {other:?}"),
                    };
                    Some((number("track") as usize, number("id") as usize, number("value")))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn unmapped_message_is_not_consumed_and_records_last_message() {
        let mut editor = editor_with_midi_module();
        assert!(!dispatch_midi_to_lisp(&mut editor, &cc(14, 100)));
        let last = editor
            .runtime_mut()
            .eval_str("(get eseq.midi/last-message :cc)")
            .unwrap();
        assert_eq!(last, Some(Value::Number(14.0)));
        assert!(rack_macro_commands(&mut editor).is_empty());
    }

    #[test]
    fn cc_maps_to_macro_of_the_armed_rack() {
        let mut editor = editor_with_midi_module();
        editor
            .runtime_mut()
            .eval_str("(eseq.midi/midi-map (eseq.midi/cc 14) (eseq.midi/rack-macro 3))")
            .unwrap();
        assert!(dispatch_midi_to_lisp(&mut editor, &cc(14, 127)));
        assert!(!dispatch_midi_to_lisp(&mut editor, &cc(15, 127)), "cc 15 unmapped");
        let commands = rack_macro_commands(&mut editor);
        assert_eq!(commands.len(), 1);
        let (track, id, value) = commands[0];
        assert_eq!((track, id), (2, 3), "armed track 0 is not a rack; track 2 is");
        assert!((value - 1.0).abs() < 1e-9);
    }

    #[test]
    fn remapping_the_same_source_replaces_instead_of_stacking() {
        let mut editor = editor_with_midi_module();
        editor
            .runtime_mut()
            .eval_str(
                "(do (eseq.midi/midi-map (eseq.midi/cc 14) (eseq.midi/rack-macro 0))
                     (eseq.midi/midi-map (eseq.midi/cc 14) (eseq.midi/rack-macro 1)))",
            )
            .unwrap();
        let count = editor
            .runtime_mut()
            .eval_str("(len (eseq.midi/midi-mappings))")
            .unwrap();
        assert_eq!(count, Some(Value::Number(1.0)));
        assert!(dispatch_midi_to_lisp(&mut editor, &cc(14, 0)));
        let commands = rack_macro_commands(&mut editor);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].1, 1, "the later mapping wins");
        editor
            .runtime_mut()
            .eval_str("(eseq.midi/midi-unmap (eseq.midi/cc 14))")
            .unwrap();
        assert!(!dispatch_midi_to_lisp(&mut editor, &cc(14, 0)));
    }

    #[test]
    fn relative_mode_nudges_from_the_current_macro_value() {
        let mut editor = editor_with_midi_module();
        editor
            .runtime_mut()
            .eval_str(
                "(eseq.midi/midi-map* (eseq.midi/cc 20) (eseq.midi/rack-macro-of 2 0)
                                      (dict :mode :relative :step 0.1))",
            )
            .unwrap();
        assert!(dispatch_midi_to_lisp(&mut editor, &cc(20, 2)));
        assert!(dispatch_midi_to_lisp(&mut editor, &cc(20, 127)));
        let commands = rack_macro_commands(&mut editor);
        assert_eq!(commands.len(), 2);
        assert!((commands[0].2 - 0.7).abs() < 1e-6, "+2 detents from 0.5: {commands:?}");
        assert!((commands[1].2 - 0.4).abs() < 1e-6, "-1 detent from 0.5: {commands:?}");
    }

    #[test]
    fn explicit_track_target_ignores_arming_and_non_rack_is_a_noop() {
        let mut editor = editor_with_midi_module();
        editor
            .runtime_mut()
            .eval_str(
                "(do (eseq.midi/midi-map (eseq.midi/cc 1) (eseq.midi/rack-macro-of 2 0))
                     (eseq.midi/midi-map (eseq.midi/cc 2) (eseq.midi/rack-macro-of 0 0))
                     (eseq.midi/midi-map (eseq.midi/cc 3) (eseq.midi/rack-macro-of 2 8))
                     (eseq.midi/midi-map* (eseq.midi/cc 4) (eseq.midi/rack-macro-of 2 8)
                                           (dict :mode :relative)))",
            )
            .unwrap();
        assert!(dispatch_midi_to_lisp(&mut editor, &cc(1, 64)));
        assert!(!dispatch_midi_to_lisp(&mut editor, &cc(2, 64)), "track 0 is not a rack");
        assert!(!dispatch_midi_to_lisp(&mut editor, &cc(3, 64)), "missing absolute macro");
        assert!(!dispatch_midi_to_lisp(&mut editor, &cc(4, 1)), "missing relative macro");
        let commands = rack_macro_commands(&mut editor);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, 2);
    }

    #[test]
    fn lambda_target_and_hook_listener_see_normalised_values() {
        let mut editor = editor_with_midi_module();
        editor
            .runtime_mut()
            .eval_str(
                "(do (defstate seen-value -1)
                     (defstate hook-kind nil)
                     (eseq.midi/midi-map (eseq.midi/note 36)
                                         (lambda (value msg) (set! seen-value value)))
                     (add-hook \"midi-message-hook\" \"test\"
                               (lambda (msg) (set! hook-kind (get msg :kind)))))",
            )
            .unwrap();
        let pad = MidiInputEvent {
            port: 1,
            message: MidiMessage::Note {
                channel: 9,
                note: MidiNoteEvent {
                    note: 36,
                    velocity: 0.25,
                    on: true,
                },
            },
        };
        assert!(
            dispatch_midi_to_lisp(&mut editor, &pad),
            "a mapped note is consumed so it never reaches the live keyboard"
        );
        assert_eq!(
            editor.runtime_mut().eval_str("seen-value").unwrap(),
            Some(Value::Number(0.25))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("hook-kind").unwrap(),
            Some(Value::Keyword("note-on".into()))
        );
    }

    #[test]
    fn device_scopes_follow_connection_identity_and_override_broad_mappings() {
        let mut editor = editor_with_midi_module();
        register_device_state(editor.runtime_mut());
        sync_midi_port(&mut editor, 0, Some(("akai-1".into(), "MIDI Mix".into())));
        editor.runtime_mut().eval_str(r#"
            (eseq.midi/midi-map (eseq.midi/cc 19) (eseq.midi/rack-macro 0))
            (eseq.midi/midi-map (eseq.midi/on-device "MIDI Mix" (eseq.midi/cc 19)) (eseq.midi/rack-macro 1))
            (eseq.midi/midi-map (eseq.midi/on-device-id "akai-1" (eseq.midi/cc 19)) (eseq.midi/rack-macro 2))
        "#).unwrap();
        assert!(dispatch_midi_to_lisp(&mut editor, &cc(19, 127)));
        assert_eq!(rack_macro_commands(&mut editor), vec![(2, 2, 1.0)]);
        editor.runtime_mut().eval_str(r#"
            (eseq.midi/midi-unmap (eseq.midi/on-device-id "akai-1" (eseq.midi/cc 19)))
        "#).unwrap();
        assert!(dispatch_midi_to_lisp(&mut editor, &cc(19, 127)));
        assert_eq!(rack_macro_commands(&mut editor), vec![(2, 1, 1.0)]);
        sync_midi_port(&mut editor, 0, None);
        assert!(dispatch_midi_to_lisp(&mut editor, &cc(19, 127)));
        assert_eq!(rack_macro_commands(&mut editor), vec![(2, 0, 1.0)]);
        sync_midi_port(&mut editor, 0, Some(("ni".into(), "Keyboard".into())));
        assert!(dispatch_midi_to_lisp(&mut editor, &cc(19, 127)));
        assert_eq!(rack_macro_commands(&mut editor), vec![(2, 0, 1.0)]);
        assert_eq!(editor.runtime_mut().eval_str("(get eseq.midi/last-message :device-id)").unwrap(),
            Some(Value::String("ni".into())));
    }

    #[test]
    fn channel_scoped_source_only_matches_its_channel() {
        let mut editor = editor_with_midi_module();
        editor
            .runtime_mut()
            .eval_str("(eseq.midi/midi-map (eseq.midi/on-channel 1 (eseq.midi/cc 14)) (eseq.midi/rack-macro 0))")
            .unwrap();
        assert!(!dispatch_midi_to_lisp(&mut editor, &cc(14, 10)), "channel 0");
        let mut on_channel_one = cc(14, 10);
        if let MidiMessage::ControlChange { channel, .. } = &mut on_channel_one.message {
            *channel = 1;
        }
        assert!(dispatch_midi_to_lisp(&mut editor, &on_channel_one));
    }
}
