//! Sequenced legato held-note stack (docs/legato-mono-spec.md, sequenced
//! counterpart of `MonoHeldNotes`).
//!
//! When a sequenced mono note takes over a still-gated voice as legato, the
//! note it displaced is usually still inside its own duration: a recorded
//! phrase holds the first key while the second is tapped. The live path resumes
//! the older key when the newer one is released; this stack gives playback the
//! same fallback. Each gated sequenced note is pushed with the block-relative
//! sample its gate ends on, decremented once per callback like the countdown
//! queue. When the current note's gate-off fires, the newest displaced note
//! whose gate is still running is resumed with a legato note-on and owns the
//! gate for its remaining duration.

use super::super::events::GateOffTarget;

/// Total holds across all voices. A mono voice rarely stacks more than a few
/// overlapping steps; the cap keeps this real-time-safe with no allocation.
const MAX_HOLDS: usize = 256;

#[derive(Clone, Copy, Debug)]
pub(in crate::audio) struct SequencedLegatoHold {
    pub logical_id: u64,
    pub track_idx: usize,
    pub target: GateOffTarget,
    pub pitch_hz: f32,
    pub velocity: f32,
    /// Samples until this note's gate ends, relative to the start of the
    /// current callback block. `advance` runs at the start of every block,
    /// before that block's events dispatch, so it stays block-relative.
    pub remaining_samples: f64,
}

#[derive(Default)]
pub(in crate::audio) struct SequencedLegatoHolds {
    holds: arrayvec::ArrayVec<SequencedLegatoHold, MAX_HOLDS>,
}

impl SequencedLegatoHolds {
    /// Record a gated sequenced note-on. A retrigger (non-legato) note-on
    /// discards whatever the voice was holding; a legato note-on stacks on top
    /// of it. `gate_end_offset` is the gate-off's offset from the current
    /// block start (`frame_offset + gate_samples`).
    pub fn note_on(&mut self, hold: SequencedLegatoHold, legato: bool, gate_end_offset: f64) {
        if !legato {
            self.clear_lid(hold.logical_id);
        }
        if self.holds.is_full() {
            // Drop the oldest hold on this voice first, else the oldest overall.
            let victim = self
                .holds
                .iter()
                .position(|held| held.logical_id == hold.logical_id)
                .unwrap_or(0);
            self.holds.remove(victim);
        }
        self.holds.push(SequencedLegatoHold {
            remaining_samples: gate_end_offset,
            ..hold
        });
    }

    pub fn clear_lid(&mut self, logical_id: u64) {
        self.holds.retain(|held| held.logical_id != logical_id);
    }

    pub fn clear(&mut self) {
        self.holds.clear();
    }

    /// Advance one callback block; run where countdown events are advanced.
    pub fn advance(&mut self, nframes: usize) {
        let block_len = nframes as f64;
        for held in &mut self.holds {
            held.remaining_samples -= block_len;
        }
        // A hold whose gate would already have ended can never be resumed.
        self.holds.retain(|held| held.remaining_samples >= 0.0);
    }

    /// The voice's current note gated off at `frame_offset` of this block.
    /// Pops it and returns the newest displaced note still inside its gate,
    /// already re-pushed as the voice's current note, or `None` when the
    /// voice should really close.
    pub fn resume_on_gate_off(
        &mut self,
        logical_id: u64,
        frame_offset: u32,
    ) -> Option<SequencedLegatoHold> {
        if let Some(top) = self.holds.iter().rposition(|held| held.logical_id == logical_id) {
            self.holds.remove(top);
        }
        let now = frame_offset as f64;
        self.holds
            .retain(|held| held.logical_id != logical_id || held.remaining_samples > now);
        self.holds
            .iter()
            .rposition(|held| held.logical_id == logical_id)
            .map(|idx| self.holds[idx])
    }

    pub fn is_empty(&self) -> bool {
        self.holds.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NFRAMES: usize = 64;

    fn hold(lid: u64, pitch_hz: f32) -> SequencedLegatoHold {
        SequencedLegatoHold {
            logical_id: lid,
            track_idx: 0,
            target: GateOffTarget::Custom { engine_id: 0, free_patch: false },
            pitch_hz,
            velocity: 0.8,
            remaining_samples: 0.0,
        }
    }

    #[test]
    fn released_legato_note_resumes_the_displaced_note_for_its_remaining_gate() {
        let mut holds = SequencedLegatoHolds::default();
        // Note A: on at frame 0, gate 1000 samples.
        holds.note_on(hold(7, 110.0), false, 1000.0);
        holds.advance(NFRAMES);
        // Note B takes over as legato at frame 10 of the next block, gate 300.
        holds.note_on(hold(7, 220.0), true, 310.0);
        for _ in 0..4 {
            holds.advance(NFRAMES);
        }
        // B's gate-off fires this block at frame 310 - 4*64 = 54.
        let resumed = holds
            .resume_on_gate_off(7, 54)
            .expect("A is still inside its gate");
        assert_eq!(resumed.pitch_hz, 110.0);
        // A's gate ends at 1000 - 5*64 = 680 block-relative samples.
        assert_eq!(resumed.remaining_samples, 680.0);
        assert!(resumed.remaining_samples > 54.0);

        // A now owns the gate; when it ends, nothing is left to resume.
        assert!(holds.resume_on_gate_off(7, 0).is_none());
        assert!(holds.is_empty());
    }

    #[test]
    fn displaced_note_whose_gate_already_ended_is_not_resumed() {
        let mut holds = SequencedLegatoHolds::default();
        holds.note_on(hold(7, 110.0), false, 100.0);
        holds.advance(NFRAMES);
        holds.note_on(hold(7, 220.0), true, 500.0);
        for _ in 0..3 {
            holds.advance(NFRAMES);
        }
        // A ended at 100 samples, long before B's gate-off.
        assert!(holds.resume_on_gate_off(7, 20).is_none());
        assert!(holds.is_empty());
    }

    #[test]
    fn retrigger_note_on_discards_the_voice_stack_and_other_voices_are_untouched() {
        let mut holds = SequencedLegatoHolds::default();
        holds.note_on(hold(7, 110.0), false, 1000.0);
        holds.note_on(hold(9, 330.0), false, 1000.0);
        holds.advance(NFRAMES);
        holds.note_on(hold(7, 220.0), true, 900.0);
        holds.advance(NFRAMES);
        // A full retrigger on voice 7 forgets both A and B.
        holds.note_on(hold(7, 440.0), false, 900.0);
        holds.advance(NFRAMES);
        assert!(holds.resume_on_gate_off(7, 0).is_none());
        // Voice 9 still has its note recorded.
        holds.clear_lid(7);
        assert!(!holds.is_empty());
        assert!(holds.resume_on_gate_off(9, 0).is_none());
        assert!(holds.is_empty());
    }

    #[test]
    fn last_note_priority_resumes_the_newest_displaced_note() {
        let mut holds = SequencedLegatoHolds::default();
        holds.note_on(hold(7, 110.0), false, 2000.0);
        holds.note_on(hold(7, 165.0), true, 2000.0);
        holds.note_on(hold(7, 220.0), true, 200.0);
        holds.advance(NFRAMES);
        let resumed = holds.resume_on_gate_off(7, 0).expect("two notes still held");
        assert_eq!(resumed.pitch_hz, 165.0);
        let resumed = holds.resume_on_gate_off(7, 0).expect("the first note is still held");
        assert_eq!(resumed.pitch_hz, 110.0);
        assert!(holds.resume_on_gate_off(7, 0).is_none());
    }
}
