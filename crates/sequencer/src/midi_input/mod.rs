//! Hardware MIDI keyboard input (bead eseq-egs6).
//!
//! Available MIDI inputs are discovered by `service` while running. Channel voice
//! messages are parsed on the driver thread and queued for the UI thread,
//! which drains them once per loop iteration. Every message is first offered
//! to the Lisp mapping layer (`eseq.midi/dispatch`, `content/ui/midi.lisp`);
//! notes it does not consume then take the same live-note path as musical
//! typing (`ui/input.rs`), so arming, MIDI-FX routing, record-as-heard
//! stamping, take capture and pattern recording all behave exactly as they
//! do for computer-keyboard notes. Clock, sysex and other system messages
//! are dropped at the driver.
//!
//! The channel is one-way and the UI thread never blocks on it. A waker,
//! when the windowing backend provides one, interrupts the host loop's idle
//! poll so a note queued between frames is heard within a frame.

use std::sync::Arc;

pub mod service;

/// Bound controller storage before audio starts. MIDI exposes sixteen
/// channels and 128 keys on each of these independently addressed ports.
pub const MAX_INPUT_PORTS: usize = 64;

/// One parsed note message from a hardware port.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiNoteEvent {
    pub note: u8,
    /// 0.0..=1.0; note-offs carry 0.0.
    pub velocity: f32,
    pub on: bool,
}

/// One parsed channel-voice message from a hardware port.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MidiMessage {
    Note {
        /// 0-based channel (0..=15).
        channel: u8,
        note: MidiNoteEvent,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        /// Raw 0..=127.
        value: u8,
    },
    PitchBend {
        channel: u8,
        /// -1.0..=1.0, 0.0 at centre.
        value: f32,
    },
    PolyPressure {
        channel: u8,
        note: u8,
        /// Key pressure, independent of note-on velocity, 0.0..=1.0.
        value: f32,
    },
    Aftertouch {
        channel: u8,
        /// Channel pressure, 0.0..=1.0.
        value: f32,
    },
}

/// Parse a raw MIDI message. A note-on with velocity 0 is a note-off per the
/// MIDI spec (running-status keyboards send these). Returns `None` for
/// anything that is not a supported channel-voice message.
pub fn parse_message(bytes: &[u8]) -> Option<MidiMessage> {
    let (&status, rest) = bytes.split_first()?;
    let channel = status & 0x0F;
    let data1 = *rest.first()?;
    let data2 = if status & 0xF0 == 0xD0 {
        0
    } else {
        *rest.get(1)?
    };
    if data1 > 127 || data2 > 127 {
        return None;
    }
    match status & 0xF0 {
        0x90 if data2 > 0 => Some(MidiMessage::Note {
            channel,
            note: MidiNoteEvent {
                note: data1,
                velocity: f32::from(data2) / 127.0,
                on: true,
            },
        }),
        0x90 | 0x80 => Some(MidiMessage::Note {
            channel,
            note: MidiNoteEvent {
                note: data1,
                velocity: 0.0,
                on: false,
            },
        }),
        0xB0 => Some(MidiMessage::ControlChange {
            channel,
            controller: data1,
            value: data2,
        }),
        0xA0 => Some(MidiMessage::PolyPressure {
            channel,
            note: data1,
            value: f32::from(data2) / 127.0,
        }),
        0xD0 => Some(MidiMessage::Aftertouch {
            channel,
            value: f32::from(data1) / 127.0,
        }),
        0xE0 => {
            let raw = i32::from(data1) | (i32::from(data2) << 7);
            Some(MidiMessage::PitchBend {
                channel,
                value: ((raw - 8192) as f32 / 8192.0).clamp(-1.0, 1.0),
            })
        }
        _ => None,
    }
}

/// Note-only view of a message, for callers that ignore everything else.
pub fn parse_note_message(bytes: &[u8]) -> Option<MidiNoteEvent> {
    match parse_message(bytes)? {
        MidiMessage::Note { note, .. } => Some(note),
        _ => None,
    }
}

/// Hook the driver thread calls after queueing an event.
pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

/// A parsed message tagged with its live connection slot. Slots stay stable
/// while connected; lifecycle cleanup precedes reuse by another device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiInputEvent {
    pub port: usize,
    pub message: MidiMessage,
}

#[cfg(test)]
mod tests {
    #[test]
    fn pressure_preserves_channel_key_and_rejects_truncated_messages() {
        use super::{parse_message, MidiMessage};
        assert_eq!(parse_message(&[0xA9, 72, 127]),
            Some(MidiMessage::PolyPressure { channel: 9, note: 72, value: 1.0 }));
        assert_eq!(parse_message(&[0xD9, 0]),
            Some(MidiMessage::Aftertouch { channel: 9, value: 0.0 }));
        for truncated in [&[0xA9, 72][..], &[0x90, 60], &[0x80, 60], &[0xB0, 1], &[0xE0, 0]] {
            assert_eq!(parse_message(truncated), None);
        }
    }

    use super::*;

    #[test]
    fn note_on_parses_with_scaled_velocity() {
        assert_eq!(
            parse_note_message(&[0x90, 60, 127]),
            Some(MidiNoteEvent {
                note: 60,
                velocity: 1.0,
                on: true
            })
        );
        let half = parse_note_message(&[0x95, 61, 64]).unwrap();
        assert!(half.on);
        assert_eq!(half.note, 61);
        assert!((half.velocity - 64.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn note_off_and_zero_velocity_note_on_both_release() {
        assert_eq!(
            parse_note_message(&[0x80, 60, 40]),
            Some(MidiNoteEvent {
                note: 60,
                velocity: 0.0,
                on: false
            })
        );
        assert_eq!(
            parse_note_message(&[0x90, 60, 0]),
            Some(MidiNoteEvent {
                note: 60,
                velocity: 0.0,
                on: false
            })
        );
    }

    #[test]
    fn non_note_messages_are_not_notes() {
        assert_eq!(parse_note_message(&[0xB0, 1, 100]), None);
        assert_eq!(parse_note_message(&[0xE0, 0, 64]), None);
        assert_eq!(parse_note_message(&[0xF8]), None);
        assert_eq!(parse_note_message(&[0x90]), None);
        assert_eq!(parse_note_message(&[]), None);
    }

    #[test]
    fn control_change_pitch_bend_and_aftertouch_parse_with_channel() {
        assert_eq!(
            parse_message(&[0xB2, 14, 100]),
            Some(MidiMessage::ControlChange {
                channel: 2,
                controller: 14,
                value: 100
            })
        );
        assert_eq!(
            parse_message(&[0xE0, 0, 64]),
            Some(MidiMessage::PitchBend {
                channel: 0,
                value: 0.0
            })
        );
        let Some(MidiMessage::PitchBend { value, .. }) = parse_message(&[0xE0, 127, 127])
        else {
            panic!("pitch bend");
        };
        assert!(value > 0.999, "{value}");
        assert_eq!(
            parse_message(&[0xD5, 127]),
            Some(MidiMessage::Aftertouch {
                channel: 5,
                value: 1.0
            })
        );
        assert_eq!(parse_message(&[0xF8]), None, "clock is dropped");
        assert_eq!(
            parse_message(&[0xA0, 60, 10]),
            Some(MidiMessage::PolyPressure { channel: 0, note: 60, value: 10.0 / 127.0 }),
            "poly pressure keeps the addressed key"
        );
    }
}
