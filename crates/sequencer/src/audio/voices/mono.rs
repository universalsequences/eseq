//! Held-note order is independent of voice ownership: many held keys share
//! one mono voice. Retain the most recent 128 holds, matching a MIDI keyboard.
//! At capacity the oldest hold loses fallback priority; releasing it is inert.

use crate::sequencer::KeyboardTrigger;

#[derive(Default)]
pub(in crate::audio) struct MonoHeldNotes {
    notes: arrayvec::ArrayVec<MonoHeldNote, 128>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::audio) struct MonoHeldNote {
    pub trigger: KeyboardTrigger,
    pub resolved_transpose: f32,
}

pub(in crate::audio) enum MonoRelease {
    Unheld,
    Buried,
    Last,
    Resume(MonoHeldNote),
}

impl MonoHeldNotes {
    fn position(&self, note: &KeyboardTrigger) -> Option<usize> {
        self.notes.iter().position(|held| {
            held.trigger.source == note.source
                && (note.source.is_some() || held.trigger.transpose == note.transpose)
        })
    }

    pub(in crate::audio) fn press(&mut self, note: KeyboardTrigger, resolved_transpose: f32) {
        if let Some(index) = self.position(&note) {
            self.notes.remove(index);
        }
        if self.notes.is_full() {
            self.notes.remove(0);
        }
        self.notes.push(MonoHeldNote { trigger: note, resolved_transpose });
    }

    pub(in crate::audio) fn release(&mut self, note: &KeyboardTrigger) -> MonoRelease {
        let Some(index) = self.position(note) else {
            return MonoRelease::Unheld;
        };
        let was_current = index + 1 == self.notes.len();
        self.notes.remove(index);
        if !was_current {
            MonoRelease::Buried
        } else if let Some(previous) = self.notes.last() {
            MonoRelease::Resume(*previous)
        } else {
            MonoRelease::Last
        }
    }

    pub(in crate::audio) fn clear(&mut self) {
        self.notes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::LiveNoteSource;

    fn note(port: usize, key: u8) -> KeyboardTrigger {
        KeyboardTrigger {
            source: Some(LiveNoteSource::Midi { port, channel: 0, note: key }),
            track: 0,
            transpose: key as f32 - 60.0,
            velocity: 0.75,
            note_off: false,
        }
    }

    #[test]
    fn buried_release_is_inert_and_current_release_restores_previous_source() {
        let mut held = MonoHeldNotes::default();
        let a = note(0, 60);
        let b = note(1, 60);
        let c = note(0, 64);
        held.press(a, a.transpose + 12.0);
        held.press(b, b.transpose + 12.0);
        held.press(c, c.transpose + 12.0);
        assert!(matches!(held.release(&b), MonoRelease::Buried));
        assert!(matches!(held.release(&c), MonoRelease::Resume(n) if n.trigger == a && n.resolved_transpose == a.transpose + 12.0));
        assert!(matches!(held.release(&a), MonoRelease::Last));
        assert!(matches!(held.release(&a), MonoRelease::Unheld));
    }

    #[test]
    fn repress_reorders_and_reset_discards_fallback() {
        let mut held = MonoHeldNotes::default();
        let a = note(0, 60);
        let b = note(0, 64);
        held.press(a, a.transpose + 12.0);
        held.press(b, b.transpose + 12.0);
        held.press(a, a.transpose + 12.0);
        assert!(matches!(held.release(&a), MonoRelease::Resume(n) if n.trigger == b && n.resolved_transpose == b.transpose + 12.0));
        held.clear();
        assert!(matches!(held.release(&b), MonoRelease::Unheld));
    }
}
