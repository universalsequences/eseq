//! Generic Lisp controls for held MIDI sequence rolls. Hardware assignments
//! and modifier/rate-button policy belong to the Lisp mapping, not this layer.

use super::*;
use sequencer::midi_input::{MidiInputEvent, MidiMessage, MAX_INPUT_PORTS};
use sequencer::sequencer::{SequenceRollSource, Timebase};

fn index(value: &Value, limit: usize) -> Option<usize> {
    match value {
        Value::Number(n) if n.is_finite() && *n >= 0.0 && *n < limit as f64 && n.fract() == 0.0 =>
            Some(*n as usize),
        _ => None,
    }
}

fn note_source(message: &Value) -> Option<(SequenceRollSource, bool)> {
    let Value::Map(map) = message else { return None };
    let number = |key: &str, limit| index(&map.get(key)?.borrow(), limit);
    let on = match &*map.get("kind")?.borrow() {
        Value::Keyword(kind) if kind == "note-on" => {
            matches!(&*map.get("value")?.borrow(), Value::Number(n) if n.is_finite() && *n > 0.0)
        }
        Value::Keyword(kind) if kind == "note-off" => false,
        _ => return None,
    };
    Some((SequenceRollSource::MidiNote {
        port: number("port", MAX_INPUT_PORTS)?,
        channel: number("channel", 16)? as u8,
        note: number("note", 128)? as u8,
    }, on))
}

pub(crate) fn register_natives(runtime: &mut Runtime, state: Arc<SequencerState>) {
    let roll_state = state.clone();
    runtime.register_native_with_docs(
        "seq-midi-sequence-roll",
        "(seq-midi-sequence-roll message)",
        "Hold/release a sequence roll from a MIDI note message; independent of keyboard Roll mode.",
        move |args, _ctx| {
            let Some((source, on)) = args.first().and_then(note_source) else {
                return Err("seq-midi-sequence-roll expects a MIDI note message".into());
            };
            roll_state.set_sequence_roll_held(source, on);
            Ok(Value::Bool(true))
        },
    );
    let roll_state = state.clone();
    runtime.register_native_with_docs(
        "seq-midi-sequence-roll-held?",
        "(seq-midi-sequence-roll-held? port)",
        "Whether this MIDI input port currently holds a sequence roll.",
        move |args, _ctx| {
            let Some(port) = args.first().and_then(|arg| index(arg, MAX_INPUT_PORTS)) else {
                return Err("seq-midi-sequence-roll-held? expects a MIDI port index".into());
            };
            Ok(Value::Bool(roll_state.midi_sequence_roll_held(port)))
        },
    );
    runtime.register_native_with_docs(
        "seq-set-roll-rate",
        "(seq-set-roll-rate index)",
        "Set roll rate 0–7: 1/4, 1/4t, 1/8, 1/8t, 1/16, 1/16t, 1/32, 1/32t.",
        move |args, _ctx| {
            let Some(rate) = args.first().and_then(|arg| index(arg, Timebase::ROLL_RATES.len())) else {
                return Err("seq-set-roll-rate expects a rate index from 0 to 7".into());
            };
            state.set_roll_rate(Timebase::ROLL_RATES[rate]);
            Ok(Value::Bool(true))
        },
    );
}

/// Physical release is unconditional, even if its mapping was removed or
/// reloaded during the hold. Return whether this note owned a roll.
pub(crate) fn release_midi_note(state: &SequencerState, event: &MidiInputEvent) -> bool {
    if let MidiMessage::Note { channel, note } = event.message {
        if !note.on {
            return state.set_sequence_roll_held(SequenceRollSource::MidiNote {
                port: event.port, channel, note: note.note,
            }, false);
        }
    }
    false
}
