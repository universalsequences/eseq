//! Hands hardware MIDI messages to the Lisp mapping layer
//! (`content/ui/midi.lisp`, module `eseq.midi`). Runs on the UI thread from
//! the event loop's per-iteration MIDI drain.

use super::*;
use sequencer::midi_input::{MidiInputEvent, MidiMessage};

const DISPATCH_GLOBAL: &str = "eseq.midi/dispatch";

fn cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
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
/// the message falls through.
pub(crate) fn dispatch_midi_to_lisp(editor: &mut Editor, event: &MidiInputEvent) -> bool {
    let Some(callable) = editor.runtime_mut().global_value(DISPATCH_GLOBAL) else {
        return false;
    };
    let consumed = match editor
        .runtime_mut()
        .invoke(callable, vec![midi_message_value(event)])
    {
        Ok(Some(Value::Bool(consumed))) => consumed,
        Ok(_) => false,
        Err(error) => {
            editor.show_transient_message(format!("midi dispatch: {error:?}"));
            false
        }
    };
    editor.refresh_runtime_side_effects();
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
        runtime.register_native("seq-rack-macro-value", |_args, _ctx| Ok(Value::Number(0.5)));
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
                     (eseq.midi/midi-map (eseq.midi/cc 2) (eseq.midi/rack-macro-of 0 0)))",
            )
            .unwrap();
        assert!(dispatch_midi_to_lisp(&mut editor, &cc(1, 64)));
        assert!(!dispatch_midi_to_lisp(&mut editor, &cc(2, 64)), "track 0 is not a rack");
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
