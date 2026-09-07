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

#[derive(Clone, Copy)]
pub(super) enum PressureTarget {
    Track(usize),
    Source { port: usize, channel: u8, note: Option<u8> },
    Reset { port: usize, channel: u8 },
}

impl PressureTarget {
    fn matches(self, track: usize, source: Option<LiveNoteSource>) -> bool {
        match self {
            Self::Track(index) => index == track,
            Self::Source { port, channel, note } => matches!(source,
                Some(LiveNoteSource::Midi { port: p, channel: c, note: n })
                    if p == port && c == channel && note.is_none_or(|key| key == n)),
            Self::Reset { port, channel } => matches!(source,
                Some(LiveNoteSource::Midi { port: p, channel: c, .. }) if p == port && c == channel),
        }
    }
}

/// Kept with the allocated DSP voice, including its release tail. Key pressure
/// belongs to one physical hold; channel controls continue through release.
#[derive(Clone, Copy, Default)]
pub(super) struct VoiceExpression {
    pub source: Option<LiveNoteSource>,
    generation: u64,
    key_pressure: Option<f32>,
}

impl VoiceExpression {
    pub(super) fn new(source: Option<LiveNoteSource>) -> Self {
        Self { source, ..Self::default() }
    }

    pub(super) fn with_origin(origin: Option<crate::sequencer::LiveNoteOrigin>) -> Self {
        origin.map_or(Self::default(), |origin| Self {
            source: Some(origin.source), generation: origin.generation, key_pressure: None,
        })
    }

    pub(super) fn values(&mut self, state: &PressureState, reset: bool) -> [f32; 3] {
        let Some((channel, key)) = state.source_channel(self.source) else { return [0.0; 3] };
        let generation = channel.generations[key];
        if self.generation == 0 { self.generation = generation; }
        if reset {
            self.key_pressure = None;
        } else if self.generation == generation && channel.holds[key].any() {
            self.key_pressure = channel.keys[key];
        }
        [self.key_pressure.unwrap_or(channel.value), channel.pitch_bend, channel.mod_wheel]
    }
}

pub(super) fn dispatch_voice_expression(
    data: &mut super::AudioCallbackData, engine: usize, voice: usize, frame: u32,
) {
    let slot = &mut data.custom_engine_pools[engine].voices[voice];
    if slot.expression.source.is_none() { return; }
    let value = slot.expression.values(&data.pressure, false);
    let sequence = super::next_event_sequence_from(&mut data.event_seq);
    unsafe {
        super::push_graph_block_event(data.lg.0, slot.logical_id, frame, sequence,
            crate::audiograph::GBE_EXPRESSION, &value);
    }
}

pub(super) fn dispatch_held_pressure(data: &mut super::AudioCallbackData, target: PressureTarget) {
    // Capture newly allocated live sources once. Allocation clears the record,
    // so a stolen/reused voice cannot retain its previous controller owner.
    if let PressureTarget::Track(track) = target {
        for note in data.active_keyboard_notes[track].iter().flatten() {
            for voice in note.voices() {
                if let super::ActiveKeyboardVoiceTarget::Custom { engine_id, .. } = voice.target {
                    if let Some(slot) = data.custom_engine_pools[engine_id].voices.iter_mut()
                        .find(|slot| slot.logical_id == voice.logical_id) {
                        if slot.expression.generation == 0 {
                            slot.expression.source = note.source;
                        }
                    }
                }
            }
        }
    }
    for pool in &mut data.custom_engine_pools {
        for voice in &mut pool.voices[..pool.num_voices] {
            let Some(track) = voice.assigned_track else { continue; };
            if !(voice.active || voice.release_started_sample.is_some())
                || !target.matches(track, voice.expression.source) { continue; }
            let value = voice.expression.values(&data.pressure, matches!(target, PressureTarget::Reset { .. }));
            let sequence = super::next_event_sequence_from(&mut data.event_seq);
            unsafe {
                super::push_graph_block_event(data.lg.0, voice.logical_id, 0, sequence,
                    crate::audiograph::GBE_EXPRESSION, &value);
            }
        }
    }
    for (track_index, track) in data.active_keyboard_notes.iter().enumerate() {
        for note in track.iter().flatten() {
            if !target.matches(track_index, note.source) { continue; }
            let value = data.pressure.expression(note.source);
            for voice in note.voices() {
                if !matches!(voice.target, super::ActiveKeyboardVoiceTarget::Sampler { .. })
                    || voice.gatepitch_id <= 0 { continue; }
                let sequence = super::next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    super::push_graph_block_event(data.lg.0, voice.gatepitch_id as u64, 0, sequence,
                        crate::audiograph::GBE_EXPRESSION, &value);
                }
            }
        }
    }
}

#[derive(Clone)]
struct Channel {
    value: f32,
    pitch_bend: f32,
    mod_wheel: f32,
    keys: [Option<f32>; 128],
    holds: [TrackHolds; 128],
    generations: [u64; 128],
}

impl Default for Channel {
    fn default() -> Self {
        Self { value: 0.0, pitch_bend: 0.0, mod_wheel: 0.0, keys: [None; 128], holds: [TrackHolds::default(); 128], generations: [0; 128] }
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

    #[cfg(test)]
    pub(super) fn press(&mut self, track: usize, source: Option<LiveNoteSource>) {
        self.press_generation(track, source, 0);
    }

    pub(super) fn press_generation(&mut self, track: usize, source: Option<LiveNoteSource>, generation: u64) {
        let Some(LiveNoteSource::Midi { port, channel, note }) = source else { return };
        if note >= 128 { return; }
        let Some(state) = self.channel_mut(port, channel) else { return };
        let key = note as usize;
        if (generation != 0 && generation != state.generations[key]) || !state.holds[key].any() {
            state.keys[key] = None;
            state.holds[key] = TrackHolds::default();
            state.generations[key] = if generation == 0 {
                state.generations[key].wrapping_add(1).max(1)
            } else { generation };
        }
        state.holds[key].set(track, true);
    }

    #[cfg(test)]
    pub(super) fn release(&mut self, track: usize, source: Option<LiveNoteSource>) {
        self.release_generation(track, source, 0);
    }

    /// Returns false for an obsolete key-up, which must not stop a newer voice.
    pub(super) fn release_generation(&mut self, track: usize, source: Option<LiveNoteSource>, generation: u64) -> bool {
        let Some(LiveNoteSource::Midi { port, channel, note }) = source else { return true };
        if note >= 128 { return false; }
        let Some(state) = self.channel_mut(port, channel) else { return false };
        let key = note as usize;
        if generation != 0 && generation != state.generations[key] { return false; }
        state.holds[key].set(track, false);
        if !state.holds[key].any() { state.keys[key] = None; }
        true
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

    fn source_channel(&self, source: Option<LiveNoteSource>) -> Option<(&Channel, usize)> {
        let Some(LiveNoteSource::Midi { port, channel, note }) = source else { return None };
        if channel >= 16 || note >= 128 { return None; }
        let index = port.checked_mul(16)?.checked_add(channel as usize)?;
        Some((self.channels.get(index)?, note as usize))
    }

    pub(super) fn value(&self, source: Option<LiveNoteSource>) -> f32 {
        self.source_channel(source).map_or(0.0, |(state, key)| state.keys[key].unwrap_or(state.value))
    }

    pub(super) fn set_pitch_bend(&mut self, port: usize, channel: u8, value: f32) {
        if !value.is_finite() { return; }
        if let Some(state) = self.channel_mut(port, channel) {
            state.pitch_bend = value.clamp(-1.0, 1.0);
        }
    }

    pub(super) fn set_mod_wheel(&mut self, port: usize, channel: u8, value: f32) {
        if !value.is_finite() { return; }
        if let Some(state) = self.channel_mut(port, channel) {
            state.mod_wheel = value.clamp(0.0, 1.0);
        }
    }

    pub(super) fn expression(&self, source: Option<LiveNoteSource>) -> [f32; 3] {
        let Some(LiveNoteSource::Midi { port, channel, note }) = source else { return [0.0; 3] };
        if channel >= 16 || note >= 128 { return [0.0; 3]; }
        let Some(index) = port.checked_mul(16).and_then(|index| index.checked_add(channel as usize)) else { return [0.0; 3] };
        let Some(state) = self.channels.get(index) else { return [0.0; 3] };
        [self.value(source), state.pitch_bend, state.mod_wheel]
    }

    pub(super) fn reset(&mut self) {
        for channel in self.channels.iter_mut() { *channel = Channel::default(); }
    }

    pub(super) fn reset_channel(&mut self, port: usize, channel: u8) {
        let Some(state) = self.channel_mut(port, channel) else { return };
        state.value = 0.0;
        state.pitch_bend = 0.0;
        state.mod_wheel = 0.0;
        state.keys.fill(None);
        // Reset All Controllers leaves held notes sounding. Their ownership
        // remains so subsequent key pressure can address the same lifetime.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsolete_key_up_cannot_release_a_new_generation() {
        let mut state = PressureState::new();
        let source = Some(LiveNoteSource::Midi { port: 0, channel: 0, note: 60 });
        state.press_generation(0, source, 10);
        state.press_generation(1, source, 10);
        state.set(0, 0, Some(60), 0.8);
        state.press_generation(0, source, 11);
        state.press_generation(1, source, 11);
        state.set(0, 0, Some(60), 0.2);
        assert!(!state.release_generation(0, source, 10));
        assert_eq!(state.value(source), 0.2);
        assert!(state.release_generation(0, source, 11));
        assert_eq!(state.value(source), 0.2);
        assert!(state.release_generation(1, source, 11));
        assert_eq!(state.value(source), 0.0);
    }

    #[test]
    fn release_tail_keeps_its_key_pressure_but_follows_channel_controls() {
        let mut state = PressureState::new();
        let source = Some(LiveNoteSource::Midi { port: 0, channel: 2, note: 60 });
        state.press(0, source);
        let mut tail = VoiceExpression::new(source);
        state.set(0, 2, Some(60), 0.8);
        assert_eq!(tail.values(&state, false), [0.8, 0.0, 0.0]);
        state.release(0, source);
        state.set_pitch_bend(0, 2, 0.5);
        state.set_mod_wheel(0, 2, 0.3);
        assert_eq!(tail.values(&state, false), [0.8, 0.5, 0.3]);
        state.press(0, source);
        let mut next = VoiceExpression::new(source);
        state.set(0, 2, Some(60), 0.2);
        assert_eq!(next.values(&state, false), [0.2, 0.5, 0.3]);
        assert_eq!(tail.values(&state, false), [0.8, 0.5, 0.3]);
        state.reset_channel(0, 2);
        assert_eq!(tail.values(&state, true), [0.0; 3]);
        assert_eq!(next.values(&state, true), [0.0; 3]);
    }

    #[test]
    fn channel_expression_is_inherited_isolated_and_reset_without_releasing_keys() {
        let mut state = PressureState::new();
        let a = Some(LiveNoteSource::Midi { port: 0, channel: 2, note: 60 });
        let b = Some(LiveNoteSource::Midi { port: 0, channel: 2, note: 64 });
        let other = Some(LiveNoteSource::Midi { port: 1, channel: 2, note: 60 });
        state.set_pitch_bend(0, 2, -0.5);
        state.set_mod_wheel(0, 2, 0.75);
        state.press(0, a);
        state.press(0, b);
        state.set(0, 2, Some(60), 0.8);
        assert_eq!(state.expression(a), [0.8, -0.5, 0.75]);
        assert_eq!(state.expression(b), [0.0, -0.5, 0.75]);
        assert_eq!(state.expression(other), [0.0; 3]);
        state.set_pitch_bend(0, 2, f32::NAN);
        state.set_mod_wheel(0, 2, f32::INFINITY);
        assert_eq!(state.expression(b), [0.0, -0.5, 0.75]);
        state.reset_channel(0, 2);
        state.set(0, 2, Some(60), 0.4);
        assert_eq!(state.expression(a), [0.4, 0.0, 0.0]);
    }

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
