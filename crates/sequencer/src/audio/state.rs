/*!
The audio callback's state: `AudioCallbackData` and friends.

`AudioCallbackData` is the hub struct threaded (by `&mut`) through nearly
every function in this module: graph pointer, sequencer state and snapshot,
voice pools, event queues, recorder, metronome, and assorted caches. Also
home to live keyboard-note bookkeeping (`ActiveKeyboardNote` storage and
release helpers), metronome synthesis state, and small per-bus/transport
cache structs.
*/

#[allow(unused_imports)]
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ActiveKeyboardVoiceTarget {
    Sampler { pool_id: usize },
    Custom { engine_id: usize, free_patch: bool },
}

impl Default for ActiveKeyboardVoiceTarget {
    fn default() -> Self {
        Self::Sampler { pool_id: 0 }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ActiveKeyboardVoice {
    pub(super) logical_id: u64,
    pub(super) gatepitch_id: i32,
    pub(super) target: ActiveKeyboardVoiceTarget,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ActiveKeyboardNote {
    pub(super) source_transpose: f32,
    pub(super) midi_note: Option<u8>,
    pub(super) velocity: f32,
    pub(super) voice_count: u8,
    pub(super) voices: [ActiveKeyboardVoice; MAX_RACK_SLOTS],
}

impl ActiveKeyboardNote {
    pub(super) fn new(
        source_transpose: f32,
        midi_note: Option<u8>,
        velocity: f32,
        voices: &[ActiveKeyboardVoice],
    ) -> Option<Self> {
        if voices.is_empty() {
            return None;
        }
        let mut note = Self {
            source_transpose,
            midi_note,
            velocity: velocity.clamp(0.0, 1.0),
            voice_count: 0,
            voices: [ActiveKeyboardVoice::default(); MAX_RACK_SLOTS],
        };
        for voice in voices.iter().take(MAX_RACK_SLOTS) {
            note.voices[note.voice_count as usize] = *voice;
            note.voice_count += 1;
        }
        Some(note)
    }

    pub(super) fn voices(&self) -> &[ActiveKeyboardVoice] {
        &self.voices[..self.voice_count as usize]
    }

    pub(super) fn remove_voice_by_lid(&mut self, logical_id: u64) -> bool {
        if logical_id == 0 {
            return false;
        }
        let Some(pos) = self
            .voices()
            .iter()
            .position(|voice| voice.logical_id == logical_id)
        else {
            return false;
        };
        let voice_count = self.voice_count as usize;
        for idx in pos..voice_count.saturating_sub(1) {
            self.voices[idx] = self.voices[idx + 1];
        }
        self.voice_count -= 1;
        true
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct HostTransportClockRuntime {
    was_playing: bool,
    play_start_sample: u64,
}

impl HostTransportClockRuntime {
    pub(super) fn sample(
        &mut self,
        playing: bool,
        block_start_sample: u64,
        sample_rate: f64,
        bpm: u32,
    ) -> HostTransportClock {
        // Only an actual transport edge may move the phase anchor. Graph and
        // track topology have a separate lifecycle and must never restart it.
        if playing != self.was_playing {
            self.play_start_sample = block_start_sample;
        }
        self.was_playing = playing;

        let samples_per_bar = sample_rate * 240.0 / bpm.max(1) as f64;
        let elapsed_samples = block_start_sample.saturating_sub(self.play_start_sample);
        HostTransportClock {
            bar_phase: (elapsed_samples as f64 / samples_per_bar).fract() as f32,
            bar_phase_increment: (1.0 / samples_per_bar) as f32,
        }
    }
}

pub(super) struct AudioCallbackData {
    pub(super) lg: LiveGraphPtr,
    pub(super) state: Arc<SequencerState>,
    pub(super) num_channels: usize,
    pub(super) sample_rate: f64,
    pub(super) last_bpm: u32,
    pub(super) last_mod_reset_counter: u32,
    pub(super) voice_pools: Vec<VoicePool>,
    pub(super) custom_engine_pools: Vec<CustomEnginePool>,
    pub(super) scheduler_snapshot: Arc<SequencerSnapshot>,
    pub(super) scheduler_snapshot_version: u64,
    pub(super) active_keyboard_notes: [[Option<ActiveKeyboardNote>; MAX_VOICES]; MAX_TRACKS],
    pub(super) keyboard_rx: std::sync::mpsc::Receiver<KeyboardTrigger>,
    pub(super) master_recorder: Arc<MasterRecorder>,
    pub(super) accumulator_states: [crate::accumulator::AccumulatorRuntimeState; MAX_TRACKS],
    pub(super) last_playing: bool,
    pub(super) last_pattern: u32,
    pub(super) last_num_tracks: usize,
    pub(super) last_topology_epoch: u64,
    pub(super) pending_topology_delete_track: Option<usize>,
    pub(super) host_transport_clock: HostTransportClockRuntime,
    pub(super) free_patch_transport_routes: [FreePatchTransportRouteState; MAX_TRACKS],
    /// Sample at which each track last fired a drum rack v2 choke trigger,
    /// `u64::MAX` for never. Two pads of one choke group hit on the same frame
    /// (closed + open hat on one step) would otherwise choke each other's
    /// brand-new voice and both fall silent; a track that triggered at the
    /// same sample is skipped by the choke pass instead.
    pub(super) rack_choke_last_trigger: [u64; MAX_TRACKS],
    /// Reused note-off buffer for the choke pass, so cutting voices allocates
    /// nothing on the audio thread after the first block that needs it.
    pub(super) rack_choke_note_offs: Vec<RackSlotNoteOff>,
    /// Per-track flag set on pattern switch/play-start; each track clears its own flag at step 0.
    pub(super) pending_accum_reset: [bool; MAX_TRACKS],
    pub(super) scheduled_events: Arc<ScheduledEventQueue<SCHEDULED_EVENT_QUEUE_CAPACITY>>,
    pub(super) countdown_events: Vec<CountdownEvent>,
    pub(super) block_events: Vec<BlockEvent>,
    pub(super) block_events_need_sort: bool,
    pub(super) current_callback_nframes: usize,
    pub(super) rendered_samples: Arc<AtomicU64>,
    /// Bus effect slots, published by the UI thread so the callback can reach
    /// each bus effect's modulator node for the transport clock/phase
    /// broadcasts. Read under `try_lock`; a failed lock just skips a block.
    pub(super) bus_effect_runtime: Arc<Mutex<Arc<Vec<BusEffectRuntimeState>>>>,
    pub(super) dropped_scheduled_events: u64,
    pub(super) late_scheduled_events: u64,
    pub(super) event_seq: u64,
    pub(super) trace_audio: bool,
    pub(super) trace_callback_counter: u64,
    pub(super) trace_render_probe_blocks: u32,
    pub(super) trace_silent_active_callbacks: u32,
    pub(super) transport_beats: f64,
    pub(super) transport_was_playing: bool,
    pub(super) metronome: MetronomeState,
    pub(super) preview: preview::PreviewVoice,
}

/// Stateful click oscillator. It lives in the callback data so a short click
/// can span output blocks without allocating or touching the graph.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct MetronomeState {
    pub(super) phase: f64,
    pub(super) envelope: f32,
    pub(super) decay_per_sample: f32,
    pub(super) frequency_hz: f64,
}

impl MetronomeState {
    const GAIN: f32 = 0.25;
    const ENVELOPE_FLOOR: f32 = 1.0e-4;

    pub(super) fn trigger(&mut self, sample_rate: f64, accented: bool) {
        self.phase = 0.0;
        self.envelope = 1.0;
        self.frequency_hz = if accented { 2_000.0 } else { 1_500.0 };
        // A 5ms exponential decay reaches the practical silence threshold.
        self.decay_per_sample = ((Self::ENVELOPE_FLOOR as f64).ln() / (sample_rate * 0.005))
            .exp()
            .clamp(0.0, 1.0) as f32;
    }

    pub(super) fn sample(&mut self, sample_rate: f64) -> f32 {
        if self.envelope < Self::ENVELOPE_FLOOR {
            return 0.0;
        }
        let value = (std::f64::consts::TAU * self.phase).sin() as f32 * self.envelope * Self::GAIN;
        self.phase = (self.phase + self.frequency_hz / sample_rate).fract();
        self.envelope *= self.decay_per_sample;
        value
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct FreePatchTransportRouteState {
    pub(super) valid: bool,
    pub(super) engine_id: usize,
    pub(super) route_hash: u64,
    pub(super) open: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FreePatchTransportRouteTarget {
    pub(super) engine_id: usize,
    pub(super) route_hash: u64,
    pub(super) open: bool,
}

pub(super) fn clear_active_keyboard_note_by_lid(
    active_notes: &mut [[Option<ActiveKeyboardNote>; MAX_VOICES]; MAX_TRACKS],
    logical_id: u64,
) {
    if logical_id == 0 {
        return;
    }
    for track_notes in active_notes.iter_mut() {
        for slot in track_notes.iter_mut() {
            if let Some(note) = slot.as_mut() {
                note.remove_voice_by_lid(logical_id);
                if note.voice_count == 0 {
                    *slot = None;
                }
            }
        }
    }
}

pub(super) fn store_active_keyboard_note(
    active_notes: &mut [[Option<ActiveKeyboardNote>; MAX_VOICES]; MAX_TRACKS],
    track_idx: usize,
    source_transpose: f32,
    midi_note: Option<u8>,
    velocity: f32,
    voices: &[ActiveKeyboardVoice],
) {
    let Some(note) = ActiveKeyboardNote::new(source_transpose, midi_note, velocity, voices) else {
        return;
    };
    for voice in voices {
        clear_active_keyboard_note_by_lid(active_notes, voice.logical_id);
    }
    let track_notes = &mut active_notes[track_idx];
    if let Some(slot) = track_notes.iter_mut().find(|slot| {
        slot.is_some_and(|note| (note.source_transpose - source_transpose).abs() < 0.01)
    }) {
        *slot = Some(note);
        return;
    }
    if let Some(slot) = track_notes.iter_mut().find(|slot| slot.is_none()) {
        *slot = Some(note);
        return;
    }
    track_notes[0] = Some(note);
}

pub(super) fn take_active_keyboard_note(
    active_notes: &mut [[Option<ActiveKeyboardNote>; MAX_VOICES]; MAX_TRACKS],
    track_idx: usize,
    source_transpose: f32,
) -> Option<ActiveKeyboardNote> {
    let track_notes = &mut active_notes[track_idx];
    for slot in track_notes.iter_mut() {
        if slot.is_some_and(|note| (note.source_transpose - source_transpose).abs() < 0.01) {
            return slot.take();
        }
    }
    None
}

/// Whether a live key-up should cut the voices it started.
///
/// Only gated tracks release on key-up. An ungated track is a one-shot: the
/// sequenced path schedules no gate-off for it (`gate_mode` in audio/fire.rs),
/// so cutting a live trigger would make jamming sound gated while the recording
/// of that same jam plays back ungated.
pub(super) fn live_key_release_cuts_voice(
    state: &crate::sequencer::SequencerState,
    track_idx: usize,
) -> bool {
    state.pattern.track_params[track_idx].is_gate_on()
}

pub(super) fn release_active_keyboard_voice(
    data: &mut AudioCallbackData,
    voice: ActiveKeyboardVoice,
    frame_offset: u32,
    block_end_sample: u64,
) {
    if voice.logical_id == 0 {
        return;
    }
    match voice.target {
        ActiveKeyboardVoiceTarget::Sampler { pool_id } => {
            if let Some(pool) = data.voice_pools.get_mut(pool_id) {
                pool.release_voice_by_logical_id(voice.logical_id);
            }
            let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
            let sampler_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                if voice.gatepitch_id > 0 {
                    send_custom_note_off(
                        data.lg.0,
                        voice.gatepitch_id as u64,
                        frame_offset,
                        gatepitch_seq,
                    );
                }
                send_sampler_note_off(data.lg.0, voice.logical_id, frame_offset, sampler_seq);
            }
        }
        ActiveKeyboardVoiceTarget::Custom {
            engine_id,
            free_patch,
        } => {
            if let Some(pool) = data.custom_engine_pools.get_mut(engine_id) {
                if free_patch {
                    pool.release_free_patch_voice_by_logical_id(voice.logical_id);
                } else {
                    pool.release_voice_by_logical_id(voice.logical_id, block_end_sample);
                }
            }
            let seq = next_block_event_sequence(data);
            unsafe {
                send_custom_note_off(data.lg.0, voice.logical_id, frame_offset, seq);
            }
        }
    }
}

pub(super) fn release_active_keyboard_note(
    data: &mut AudioCallbackData,
    note: ActiveKeyboardNote,
    frame_offset: u32,
    block_end_sample: u64,
) {
    for voice in note.voices() {
        release_active_keyboard_voice(data, *voice, frame_offset, block_end_sample);
    }
}
