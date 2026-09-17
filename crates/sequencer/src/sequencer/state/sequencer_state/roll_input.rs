use super::super::*;
use crate::sequencer::{RollCommand, SequenceRollSource};

impl SequencerState {
    pub fn push_roll_command(&self, command: RollCommand) {
        let mut input = self.roll_input.lock().unwrap();
        if command == RollCommand::ClearAll {
            input.held.clear();
            self.transport.sequence_rolling.store(false, Ordering::Release);
        }
        input.commands.push(command);
    }

    pub fn drain_roll_commands(&self) -> Vec<RollCommand> {
        std::mem::take(&mut self.roll_input.lock().unwrap().commands)
    }

    /// Called on the control thread. Returns whether ownership changed.
    /// Duplicate presses and unmatched releases do not retrigger the window.
    /// The caller decides whether its gesture requires keyboard roll mode.
    pub fn set_sequence_roll_held(&self, source: SequenceRollSource, on: bool) -> bool {
        let mut input = self.roll_input.lock().unwrap();
        let was_held = !input.held.is_empty();
        let pos = input.held.iter().position(|held| *held == source);
        match (on, pos) {
            (true, None) => input.held.push(source),
            (false, Some(pos)) => { input.held.remove(pos); }
            _ => return false,
        }
        let is_held = !input.held.is_empty();
        if was_held != is_held {
            self.transport.sequence_rolling.store(is_held, Ordering::Release);
            input.commands.push(RollCommand::SequenceRoll { on: is_held });
        }
        true
    }

    pub fn midi_sequence_roll_held(&self, port: usize) -> bool {
        self.roll_input.lock().unwrap().held.iter().any(|source| {
            matches!(source, SequenceRollSource::MidiNote { port: held_port, .. } if *held_port == port)
        })
    }

    pub fn has_sequence_roll_holds(&self) -> bool {
        !self.roll_input.lock().unwrap().held.is_empty()
    }

    /// A process roll can end while a manual start is queued. Serialize the
    /// mirror update with input ownership so it cannot overwrite that start.
    pub fn clear_sequence_rolling_if_unheld(&self) {
        let input = self.roll_input.lock().unwrap();
        if input.held.is_empty() {
            self.transport.sequence_rolling.store(false, Ordering::Release);
        }
    }

    /// Release a disconnected/disabled MIDI input without affecting other
    /// controllers or a held keyboard gesture.
    pub fn release_midi_sequence_rolls(&self, port: usize) {
        let mut input = self.roll_input.lock().unwrap();
        let was_held = !input.held.is_empty();
        input.held.retain(|source| {
            !matches!(source, SequenceRollSource::MidiNote { port: held_port, .. } if *held_port == port)
        });
        if was_held && input.held.is_empty() {
            self.transport.sequence_rolling.store(false, Ordering::Release);
            input.commands.push(RollCommand::SequenceRoll { on: false });
        }
    }

    pub fn set_roll_rate(&self, rate: Timebase) {
        let mut input = self.roll_input.lock().unwrap();
        self.transport.roll_rate.store(rate as u32, Ordering::Release);
        // Repeated triplet/fast rates intentionally reach the scheduler's
        // stutter-repress handling, just like the keyboard's rate keys.
        input.commands.push(RollCommand::SetRate { rate });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_and_clear_all_retire_hold_owners_before_the_next_gesture() {
        for stop in 0..3 {
            let state = SequencerState::new(0, vec![]);
            let source = SequenceRollSource::MidiNote { port: 0, channel: 0, note: 27 };
            state.start_playback();
            state.set_sequence_roll_held(source.clone(), true);
            state.drain_roll_commands();
            match stop {
                0 => state.stop_playback(),
                1 => { assert!(!state.toggle_play()); }
                _ => state.push_roll_command(RollCommand::ClearAll),
            }
            assert!(!state.has_sequence_roll_holds());
            assert!(!state.transport.sequence_rolling.load(Ordering::Acquire));
            assert_eq!(state.drain_roll_commands(), vec![RollCommand::ClearAll]);
            let keyboard = SequenceRollSource::Named("keyboard:F2".into());
            state.set_sequence_roll_held(keyboard.clone(), true);
            // An old MIDI release cannot end a newer keyboard gesture.
            assert!(!state.set_sequence_roll_held(source, false));
            assert!(state.transport.sequence_rolling.load(Ordering::Acquire));
            state.set_sequence_roll_held(keyboard, false);
            assert_eq!(state.drain_roll_commands(), vec![
                RollCommand::SequenceRoll { on: true },
                RollCommand::SequenceRoll { on: false },
            ]);
        }
    }
}
