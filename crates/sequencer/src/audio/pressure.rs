//! MIDI pressure state is allocated before audio starts and addressed by the
//! original input identity, never by the transposed sounding pitch.

use crate::sequencer::{LiveNoteSource, MAX_TRACKS};

// One physical key can feed several armed tracks. A repeated note-on replaces
// that track's hold; it must not require an extra note-off to clear pressure.
#[derive(Clone, Copy, Default)]
struct TrackHolds([u64; MAX_TRACKS.div_ceil(64)]);

impl TrackHolds {
    fn any(&self) -> bool { self.0.iter().any(|word| *word != 0) }

    fn set(&mut self, track: usize, held: bool) {
        if track >= MAX_TRACKS { return; }
        let bit = 1u64 << (track % 64);
        if held { self.0[track / 64] |= bit; }
        else { self.0[track / 64] &= !bit; }
    }

    fn delete_track(&mut self, deleted: usize) {
        for track in deleted..MAX_TRACKS {
            let next = track + 1;
            let held = next < MAX_TRACKS && self.0[next / 64] & (1u64 << (next % 64)) != 0;
            self.set(track, held);
        }
    }
}

pub(super) enum PressureTarget {
    Track(usize),
    Source { port: usize, channel: u8, note: Option<u8> },
}

pub(super) fn dispatch_held_pressure(data: &mut super::AudioCallbackData, target: PressureTarget) {
    for (track_index, track) in data.active_keyboard_notes.iter().enumerate() {
        if matches!(target, PressureTarget::Track(index) if index != track_index) { continue; }
        for note in track.iter().flatten() {
            if let PressureTarget::Source { port, channel, note: key } = target {
                if !matches!(note.source,
                    Some(LiveNoteSource::Midi { port: p, channel: c, note: n })
                        if p == port && c == channel && key.is_none_or(|key| key == n)) {
                    continue;
                }
            }
            let value = data.pressure.value(note.source);
            for voice in note.voices() {
                let gatepitch = match voice.target {
                    super::ActiveKeyboardVoiceTarget::Custom { .. } => voice.logical_id,
                    super::ActiveKeyboardVoiceTarget::Sampler { .. } => {
                        if voice.gatepitch_id <= 0 { continue; }
                        voice.gatepitch_id as u64
                    }
                };
                let sequence = super::next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    super::push_graph_block_event(
                        data.lg.0, gatepitch, 0, sequence,
                        crate::audiograph::GBE_PRESSURE, &[value],
                    );
                }
            }
        }
    }
}

#[derive(Clone)]
struct Channel {
    value: f32,
    keys: [Option<f32>; 128],
    holds: [TrackHolds; 128],
}

impl Default for Channel {
    fn default() -> Self {
        Self { value: 0.0, keys: [None; 128], holds: [TrackHolds::default(); 128] }
    }
}

pub(super) struct PressureState {
    channels: Box<[Channel]>,
}

impl PressureState {
    pub(super) fn new() -> Self {
        Self {
            channels: (0..crate::midi_input::MAX_INPUT_PORTS * 16)
                .map(|_| Channel::default()).collect(),
        }
    }

    fn channel_mut(&mut self, port: usize, channel: u8) -> Option<&mut Channel> {
        if channel >= 16 { return None; }
        self.channels.get_mut(port.checked_mul(16)?.checked_add(channel as usize)?)
    }

    pub(super) fn set(&mut self, port: usize, channel: u8, note: Option<u8>, value: f32) {
        if !value.is_finite() { return; }
        let Some(state) = self.channel_mut(port, channel) else { return };
        let value = value.clamp(0.0, 1.0);
        if let Some(key) = note.filter(|key| *key < 128) {
            // A key-pressure message cannot prime a future note lifetime.
            if state.holds[key as usize].any() { state.keys[key as usize] = Some(value); }
        } else if note.is_none() {
            state.value = value;
        }
    }

    pub(super) fn press(&mut self, track: usize, source: Option<LiveNoteSource>) {
        let Some(LiveNoteSource::Midi { port, channel, note }) = source else { return };
        if note >= 128 { return; }
        let Some(state) = self.channel_mut(port, channel) else { return };
        let key = note as usize;
        if !state.holds[key].any() { state.keys[key] = None; }
        state.holds[key].set(track, true);
    }

    pub(super) fn release(&mut self, track: usize, source: Option<LiveNoteSource>) {
        let Some(LiveNoteSource::Midi { port, channel, note }) = source else { return };
        if note >= 128 { return; }
        let Some(state) = self.channel_mut(port, channel) else { return };
        let key = note as usize;
        state.holds[key].set(track, false);
        if !state.holds[key].any() { state.keys[key] = None; }
    }

    pub(super) fn release_track(&mut self, track: usize) {
        for channel in self.channels.iter_mut() {
            for (hold, pressure) in channel.holds.iter_mut().zip(&mut channel.keys) {
                hold.set(track, false);
                if !hold.any() { *pressure = None; }
            }
        }
    }

    pub(super) fn delete_track(&mut self, track: usize) {
        for channel in self.channels.iter_mut() {
            for (hold, pressure) in channel.holds.iter_mut().zip(&mut channel.keys) {
                hold.delete_track(track);
                if !hold.any() { *pressure = None; }
            }
        }
    }

    pub(super) fn value(&self, source: Option<LiveNoteSource>) -> f32 {
        let Some(LiveNoteSource::Midi { port, channel, note }) = source else { return 0.0 };
        if channel >= 16 || note >= 128 { return 0.0; }
        let Some(index) = port.checked_mul(16).and_then(|index| index.checked_add(channel as usize)) else { return 0.0 };
        let Some(state) = self.channels.get(index) else { return 0.0 };
        state.keys[note as usize].unwrap_or(state.value)
    }

    pub(super) fn reset(&mut self) {
        for channel in self.channels.iter_mut() { *channel = Channel::default(); }
    }

    pub(super) fn reset_channel(&mut self, port: usize, channel: u8) {
        let Some(state) = self.channel_mut(port, channel) else { return };
        state.value = 0.0;
        state.keys.fill(None);
        // Reset All Controllers leaves held notes sounding. Their ownership
        // remains so subsequent key pressure can address the same lifetime.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_isolated_by_port_channel_and_key_and_reset_on_new_lifetime() {
        let mut state = PressureState::new();
        let a = Some(LiveNoteSource::Midi { port: 0, channel: 2, note: 60 });
        let b = Some(LiveNoteSource::Midi { port: 0, channel: 2, note: 64 });
        let other = Some(LiveNoteSource::Midi { port: 1, channel: 2, note: 60 });
        state.set(0, 2, None, 0.4);
        state.press(0, a);
        state.press(1, a); // The same source also sounds a second armed track.
        state.press(0, b);
        state.set(0, 2, Some(60), 0.8);
        assert_eq!(state.value(a), 0.8);
        assert_eq!(state.value(b), 0.4);
        assert_eq!(state.value(other), 0.0);
        state.release(0, a);
        assert_eq!(state.value(a), 0.8);
        state.release(1, a);
        state.press(0, a);
        assert_eq!(state.value(a), 0.4);
        state.reset();
        assert_eq!(state.value(a), 0.0);
    }

    #[test]
    fn repeated_note_on_and_track_removal_do_not_leave_stale_key_pressure() {
        let mut state = PressureState::new();
        let key = Some(LiveNoteSource::Midi { port: 0, channel: 0, note: 60 });
        state.set(0, 0, None, 0.25);
        state.press(0, key);
        state.press(0, key);
        state.set(0, 0, Some(60), 0.8);
        state.release(0, key);
        assert_eq!(state.value(key), 0.25);

        state.press(1, key);
        state.press(3, key);
        state.set(0, 0, Some(60), 0.9);
        state.delete_track(1);
        assert_eq!(state.value(key), 0.9); // Former track 3 still owns it.
        state.release_track(2);
        assert_eq!(state.value(key), 0.25);
        state.set(0, 0, Some(60), 1.0); // No held owner: ignore stale pressure.
        state.press(2, key);
        assert_eq!(state.value(key), 0.25);
    }

    #[test]
    fn controller_reset_is_scoped_and_preserves_held_note_ownership() {
        let mut state = PressureState::new();
        let key = Some(LiveNoteSource::Midi { port: 0, channel: 2, note: 60 });
        let other_channel = Some(LiveNoteSource::Midi { port: 0, channel: 3, note: 60 });
        let other_port = Some(LiveNoteSource::Midi { port: 1, channel: 2, note: 60 });
        for source in [key, other_channel, other_port] { state.press(0, source); }
        state.set(0, 2, None, 0.4);
        state.set(0, 2, Some(60), 0.8);
        state.set(0, 3, None, 0.5);
        state.set(1, 2, Some(60), 0.6);
        state.reset_channel(0, 2);
        assert_eq!(state.value(key), 0.0);
        assert_eq!(state.value(other_channel), 0.5);
        assert_eq!(state.value(other_port), 0.6);
        state.set(0, 2, Some(60), 0.9);
        assert_eq!(state.value(key), 0.9);
        state.release(0, key);
        state.press(0, key);
        assert_eq!(state.value(key), 0.0);
    }
}
