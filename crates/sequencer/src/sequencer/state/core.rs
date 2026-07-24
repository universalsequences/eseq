use super::*;

pub fn default_empty_effect_chain() -> Vec<EffectSlotState> {
    use crate::lisp_host::MAX_CUSTOM_FX;
    (0..MAX_CUSTOM_FX)
        .map(|_| EffectSlotState::empty())
        .collect()
}

pub struct PatternState {
    pub patterns: Vec<TrackPattern>,
    pub neural_reset_patterns: Vec<TrackPattern>,
    pub(super) scene_silenced: Vec<AtomicBool>,
    pub step_data: Vec<StepData>,
    pub chord_data: Vec<ChordData>,
    pub track_params: Vec<TrackParams>,
    pub effect_chains: Vec<Vec<EffectSlotState>>,
    pub midi_fx_slots: Vec<Vec<EffectSlotState>>,
    pub(super) scenes: Mutex<ProjectScenes>,
    /// The committed song (docs/song-mode-spec.md section 5), or `None` when
    /// the project has no song. Stored beside — not inside — `ProjectScenes`
    /// because several paths rebuild `ProjectScenes` wholesale from snapshots
    /// (`from_pattern_snapshots`) and would silently drop an embedded song.
    pub(super) song: Mutex<Option<ProjectSong>>,
    /// Bumped on every committed-song replacement/edit so per-frame UI code
    /// can rebuild song-derived reactive values (`song-rows`) only when the
    /// song actually changed (docs/song-mode-spec.md 12).
    pub(super) song_revision: AtomicU64,
    pub(super) current_pattern: AtomicU32,
    pub(super) num_patterns: AtomicU32,
    pub timebase_plocks: Vec<TimebasePLockData>,
    pub swing_plocks: Vec<SwingPLockData>,
    pub swing_resolution_plocks: Vec<SwingResolutionPLockData>,
    pub instrument_slots: Vec<EffectSlotState>,
    pub instrument_base_note_offsets: Vec<AtomicU32>,
    pub instrument_run_modes: Vec<AtomicU32>,
    pub track_sound_state: Mutex<Vec<TrackSoundState>>,
    pub rack_tracks: Mutex<Vec<Option<RackTrackSnapshot>>>,
    pub process_chains: Mutex<Vec<crate::process::TrackProcessChain>>,
    pub project_process_lane_overrides: Mutex<Vec<crate::process::ProjectLaneOverrides>>,
    pub plock_variant_registries: Mutex<Vec<PlockVariantRegistry>>,
    pub key_lock_variant_registries: Mutex<Vec<PlockVariantRegistry>>,
}

pub struct TransportState {
    pub playhead: AtomicU32,
    pub playing: AtomicBool,
    pub bpm: AtomicU32,
    pub master_volume: AtomicU32,
    pub pattern_epoch: AtomicU64,
    pub topology_epoch: AtomicU64,
    pub topology_edit_kind: AtomicU32,
    pub topology_edit_track: AtomicU32,
    pub topology_edit_request_id: AtomicU64,
    pub topology_edit_ready_id: AtomicU64,
    pub topology_edit_applied_id: AtomicU64,
    pub mod_reset_counter: AtomicU32,
    pub pending_mod_resync: AtomicBool,
    pub peak_l: AtomicU32,
    pub peak_r: AtomicU32,
    pub cpu_load_pct: AtomicU32,
    pub trigger_flash: Vec<AtomicU32>,
    pub num_tracks: AtomicU32,
    pub track_playheads: Vec<AtomicU32>,
    /// Per-track phase within the active step, normalized to 0.0..=1.0.
    pub track_playhead_phases: Vec<AtomicU32>,
    /// Per-track sampler playhead as normalized 0.0–1.0 (f32 bits).
    pub sampler_playheads: Vec<AtomicU32>,
    pub active_voice_counts: Vec<AtomicU32>,
    pub playhead_phase: AtomicU32,
    /// The live-keyboard record quantization mode (`RecordQuantize as u8`).
    pub record_quantize: AtomicU32,
    /// Audio output latency compensation used when timestamping keyboard note-ons.
    pub record_latency_seconds: AtomicU32,
    /// Monotonic audio-clock anchor published by the audio callback.
    pub record_clock: RecordClockAnchor,
    pub metronome_enabled: AtomicBool,
    pub record_quantize_thresh: AtomicU32,
}

/// Lock-free snapshot of the render clock for wall-clock interpolation on the
/// UI thread. The sequence counter makes the two payload values atomic as a
/// pair without placing a mutex on the realtime callback.
pub struct RecordClockAnchor {
    pub(super) sequence: AtomicU64,
    pub(super) beats_bits: AtomicU64,
    pub(super) timestamp_nanos: AtomicU64,
}

impl RecordClockAnchor {
    pub fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            beats_bits: AtomicU64::new(0.0_f64.to_bits()),
            timestamp_nanos: AtomicU64::new(0),
        }
    }

    /// Publish an anchor from the audio callback. The odd/even sequence is a
    /// standard seqlock protocol; readers retry rather than observing a mixed
    /// beat/timestamp pair.
    pub fn publish(&self, beats: f64, timestamp: Instant) {
        self.sequence.fetch_add(1, Ordering::Release);
        self.beats_bits
            .store(beats.max(0.0).to_bits(), Ordering::Relaxed);
        self.timestamp_nanos
            .store(record_clock_nanos(timestamp), Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    /// Sample the anchor at `timestamp`, returning the anchor beat and the
    /// SIGNED elapsed seconds from the anchor to `timestamp`. The elapsed
    /// term is negative when `timestamp` predates the newest anchor — the
    /// normal case for note-on instants resolved at key RELEASE (the anchor
    /// republishes every audio block, so a held note's press is always in
    /// the anchor's past). Clamping that to zero would stamp the note at its
    /// release instant instead of its press.
    pub fn sample(&self, timestamp: Instant) -> Option<(f64, f64)> {
        for _ in 0..8 {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let beats = f64::from_bits(self.beats_bits.load(Ordering::Relaxed));
            let anchor_nanos = self.timestamp_nanos.load(Ordering::Relaxed);
            let after = self.sequence.load(Ordering::Acquire);
            if before == after {
                if !beats.is_finite() || anchor_nanos == 0 {
                    return None;
                }
                let now_nanos = record_clock_nanos(timestamp);
                let elapsed_secs =
                    (now_nanos as i128 - anchor_nanos as i128) as f64 / 1.0e9;
                return Some((beats, elapsed_secs));
            }
        }
        None
    }
}

impl Default for RecordClockAnchor {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) static RECORD_CLOCK_ORIGIN: OnceLock<Instant> = OnceLock::new();

pub(super) fn record_clock_nanos(timestamp: Instant) -> u64 {
    timestamp
        .saturating_duration_since(*RECORD_CLOCK_ORIGIN.get_or_init(Instant::now))
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordPosition {
    pub step: usize,
    pub phase: f32,
}

pub struct RuntimeBindingState {
    pub sampler_lids: Vec<AtomicU64>,
    pub modulator_lids: Vec<AtomicU64>,
    pub pan_lids: Vec<AtomicU64>,
    pub delay_lids: Vec<AtomicU64>,
    pub send_lids: Vec<AtomicU64>,
    pub rack_slot_pan_lids: Vec<[AtomicU64; MAX_RACK_SLOTS]>,
    pub voice_lids: Vec<[AtomicU64; MAX_VOICES]>,
    pub voice_counts: Vec<AtomicU32>,
    pub instrument_type_flags: Vec<AtomicU32>,
    pub instrument_run_mode_flags: Vec<AtomicU32>,
    pub synth_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub sampler_gatepitch_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub sampler_modulator_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub track_engine_ids: Vec<AtomicU32>,
    pub engine_voice_lids: Vec<[AtomicU64; MAX_VOICES]>,
    pub engine_synth_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub engine_modulator_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub engine_voice_counts: Vec<AtomicU32>,
    pub engine_route_lids: Vec<[[AtomicU64; MAX_TRACKS]; MAX_VOICES]>,
    pub engine_route_lids_r: Vec<[[AtomicU64; MAX_TRACKS]; MAX_VOICES]>,
    pub engine_ext_route_lids: Vec<[[[AtomicU64; EXT_MOD_INPUT_COUNT]; MAX_TRACKS]; MAX_VOICES]>,
    /// Per-rack-slot routes for shared custom engines. Rack slots use the same
    /// stable pool identity as sampler rack slots, so multiple slots on one
    /// track can consume one engine without manufacturing duplicate engines.
    pub rack_engine_route_lids: Vec<[AtomicU64; MAX_VOICES]>,
    pub rack_engine_route_lids_r: Vec<[AtomicU64; MAX_VOICES]>,
    pub rack_engine_route_engine_ids: Vec<AtomicU32>,
    pub rack_engine_ext_route_lids: Vec<[[AtomicU64; EXT_MOD_INPUT_COUNT]; MAX_VOICES]>,
    pub sampler_analysis_buffer_ids: Vec<AtomicU32>,
    pub sampler_analysis_bpm: Vec<AtomicU32>,
    pub sampler_onset_ptr_lo: Vec<AtomicU32>,
    pub sampler_onset_ptr_hi: Vec<AtomicU32>,
    pub sampler_analysis_status: Vec<AtomicU32>,
}

/// A sequencer definition published from the UI/editor runtime to the scheduler VM.
///
/// Two shapes share this channel:
/// - **tick mode** (`graph == None`): `tick_source` is the auto-quoted `:tick` body
///   serialized to re-evaluable lisp (see `lisp_host::sequencer_tick_source`) and
///   `resolution` is a `Timebase` index; the scheduler registers it into its generator
///   runtime.
/// - **graph mode** (`graph == Some(_)`): the whole-body manifest is carried in-process
///   as a [`crate::graph::GraphManifest`]; the scheduler materializes it into a
///   `GraphRuntime`. `tick_source`/`resolution` are unused.
///
/// The scheduler polls [`SequencerState::published_sequencers_version`] and reconciles.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishedSequencer {
    pub id: u64,
    pub name: String,
    pub resolution: u8,
    pub tick_source: String,
    /// Present iff this is a graph-mode sequencer.
    pub graph: Option<crate::graph::GraphManifest>,
}

pub struct SequencerState {
    pub pattern: PatternState,
    pub transport: TransportState,
    pub runtime: RuntimeBindingState,
    pub(super) scheduler_snapshot: Mutex<Arc<SequencerSnapshot>>,
    pub(super) scheduler_snapshot_version: AtomicU64,
    /// Command-thread macro values waiting to be folded into the next
    /// immutable scheduler snapshot. The scheduler never reads this lock.
    pub(super) live_macro_overrides: Mutex<HashMap<crate::macro_engine::MacroParamKey, f32>>,
    pub(super) rack_macro_runtime_values: Arc<RackMacroRuntimeValues>,
    pub(super) neural_visualization: Mutex<NeuralVisualizationSnapshot>,
    pub(super) graph_visualizations: Mutex<Vec<GraphVisualizationSnapshot>>,
    pub(super) track_output_events: Mutex<Vec<TrackOutputEvent>>,
    pub(super) track_output_current_beat_bits: AtomicU64,
    pub(super) active_note_until_samples: Vec<[AtomicU64; 128]>,
    pub(super) live_note_masks: Vec<[AtomicU64; 2]>,
    pub(super) audio_rendered_sample: AtomicU64,
    /// The scheduler's rendered-beat clock (`rendered_total_beats` in
    /// scheduler/worker.rs), published every scheduler loop. This is the same
    /// clock quantized-launch deadlines are computed against
    /// (`quantized_launch::launch_deadline`), so song capture reads it as the
    /// audible beat for immediate/unquantized launches
    /// (docs/song-mode-spec.md 8.2). Stored as `f64::to_bits`.
    pub(super) scheduler_rendered_beats_bits: AtomicU64,
    pub(super) scratch_source: Mutex<String>,
    pub(super) scratch_source_version: AtomicU64,
    pub(super) published_sequencers: Mutex<Vec<PublishedSequencer>>,
    pub(super) published_sequencers_version: AtomicU64,
    pub(super) published_process_authoring: Mutex<crate::process::PublishedProcessAuthoringSnapshot>,
    pub(super) published_process_authoring_version: AtomicU64,
    pub(super) scratch_effect_descriptors: Mutex<Vec<Vec<EffectDescriptor>>>,
    pub(super) scratch_instrument_descriptors: Mutex<Vec<EffectDescriptor>>,
    pub(super) process_trace_enabled: AtomicBool,
    pub(super) pending_accumulator_reset_all: AtomicBool,
    pub(super) pending_accumulator_reset_tracks: [AtomicBool; MAX_TRACKS],
    pub(super) quantized_launches: crate::quantized_launch::QuantizedLaunchMailbox,
    /// Song playback command/notice channels plus render-rate position
    /// atomics (docs/song-mode-spec.md 10.2).
    pub(super) song_playback: SongPlaybackMailbox,
    /// Manual-override latch bitmask (takes spec 10): while a bit is set,
    /// the song's launch authority is suspended for that track — the
    /// scheduler schedules the track from the LIVE session snapshot
    /// (free-running) instead of the active song row, and the control-side
    /// row mirror leaves the lane alone. Transient transport state; never
    /// serialized. Bit `t` = track `t` (`MAX_TRACKS <= 64`).
    pub(super) song_manual_latch: AtomicU64,
    /// Which lanes the CURRENTLY MIRRORED song row resolves to a take chunk
    /// (takes spec 11.2 UX). Written by the control-side row mirror; read by
    /// `track_pattern_cells` so the mixer clip grid never marks a scene clip
    /// "playing" while the lane is actually playing a take. Transient
    /// transport state; never serialized. Bit `t` = track `t`.
    pub(super) song_take_lane_mask: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackOutputEvent {
    pub track: usize,
    pub sample_time: u64,
    pub beat: f64,
    pub transpose: f32,
    pub velocity: f32,
}

pub(super) const TRACK_OUTPUT_EVENT_HISTORY_CAP: usize = 1024;

#[derive(Clone, Debug, Default)]
pub struct PatternSwitchProfile {
    pub total: Duration,
    pub capture_current_snapshot: Duration,
    pub scene_lock_wait: Duration,
    pub save_current_snapshot: Duration,
    pub launch_scene_data: Duration,
    pub restore_tracks: Duration,
    pub collect_sample_ids: Duration,
    pub update_pattern_atoms: Duration,
    pub schedule_mod_resync: Duration,
    pub publish_scheduler_snapshot: Duration,
}

#[derive(Clone, Debug)]
pub struct PatternSwitchResult {
    pub sample_ids: Vec<(i32, String, u32)>,
    pub profile: PatternSwitchProfile,
}

pub(super) const TOPOLOGY_EDIT_NONE: u32 = 0;
pub(super) const TOPOLOGY_EDIT_DELETE_TRACK: u32 = 1;

pub(super) fn capture_track_params_snapshot(track_params: &TrackParams) -> TrackParamsSnapshot {
    TrackParamsSnapshot {
        gate: track_params.is_gate_on(),
        attack_ms: track_params.get_attack_ms(),
        release_ms: track_params.get_release_ms(),
        swing: track_params.get_swing(),
        swing_resolution: track_params.get_swing_resolution(),
        num_steps: track_params.get_num_steps(),
        volume: track_params.get_volume(),
        pan: track_params.get_pan(),
        mute: track_params.is_muted(),
        solo: track_params.is_solo(),
        send: track_params.get_send(),
        output: track_params.output(),
        sends: track_params.sends(),
        polyphonic: track_params.is_polyphonic(),
        max_polyphony: track_params.get_max_polyphony(),
        timebase: track_params.get_timebase(),
        accumulator_idx: track_params.get_accumulator_idx(),
        script_accumulator_name: track_params.script_accumulator_name(),
        midi_fx_chain: track_params.midi_fx_chain(),
        midi_fx_position: track_params.get_midi_fx_position(),
        accum_limit: track_params.get_accum_limit(),
        accum_mode: track_params.get_accum_mode(),
        fts_scale: track_params.get_fts_scale(),
        mute_group: track_params.get_mute_group(),
        global_transpose: track_params.uses_global_transpose(),
    }
}

pub(super) fn restore_track_params_snapshot(track_params: &TrackParams, snapshot: &TrackParamsSnapshot) {
    track_params.gate.store(snapshot.gate, Ordering::Relaxed);
    track_params.set_attack_ms(snapshot.attack_ms);
    track_params.set_release_ms(snapshot.release_ms);
    track_params.set_swing(snapshot.swing);
    track_params.set_swing_resolution(snapshot.swing_resolution);
    track_params.set_num_steps(snapshot.num_steps);
    track_params.set_volume(snapshot.volume);
    track_params.set_pan(snapshot.pan);
    track_params.set_mute(snapshot.mute);
    track_params.set_solo(snapshot.solo);
    track_params.set_send(snapshot.send);
    track_params.set_output(snapshot.output.clone());
    track_params.set_sends(snapshot.sends.clone());
    track_params.polyphonic.store(snapshot.polyphonic, Ordering::Relaxed);
    track_params.set_max_polyphony(snapshot.max_polyphony);
    track_params.set_timebase(snapshot.timebase);
    track_params.set_accumulator_idx(snapshot.accumulator_idx);
    track_params.set_script_accumulator_name(snapshot.script_accumulator_name.clone());
    track_params.set_midi_fx_chain(snapshot.midi_fx_chain.clone());
    track_params.set_midi_fx_position(snapshot.midi_fx_position);
    track_params.set_accum_limit(snapshot.accum_limit);
    track_params.set_accum_mode(snapshot.accum_mode);
    track_params.set_fts_scale(snapshot.fts_scale);
    track_params.set_mute_group(snapshot.mute_group);
    track_params.set_global_transpose(snapshot.global_transpose);
}
