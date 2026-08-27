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
    /// The committed arrangement (docs/arrangement-lane-model-spec.md 6), or
    /// `None` when the project has no arrangement. This is the *authored*
    /// model; `song` above is its compiled playback form, kept in lockstep by
    /// `set_committed_arrangement` (spec 7). Lives beside `song` for the same
    /// reason: `ProjectScenes` is rebuilt wholesale from snapshots.
    pub(super) arrangement: Mutex<Option<ProjectArrangement>>,
    /// Bumped on every committed-song replacement/edit so per-frame UI code
    /// can rebuild song-derived reactive values (`song-rows`) only when the
    /// song actually changed (docs/song-mode-spec.md 12).
    pub(super) song_revision: AtomicU64,
    /// Bumped on every pool step/geometry write (the
    /// `restore_pattern_*_no_publish` funnel every step edit, undo and redo
    /// passes through). Per-frame UI code that projects POOL CONTENT — the
    /// arrangement lane dots — keys its rebuild off this
    /// (docs/realtime-arrangement-feedback-spec.md 5.2). Deliberately not
    /// `pattern_epoch`: that epoch drives scene-launch-scale resyncs and a
    /// per-note bump would stampede unrelated caches.
    pub(super) pool_content_revision: AtomicU64,
    pub(super) current_pattern: AtomicU32,
    pub(super) num_patterns: AtomicU32,
    pub timebase_plocks: Vec<TimebasePLockData>,
    pub swing_plocks: Vec<SwingPLockData>,
    pub swing_resolution_plocks: Vec<SwingResolutionPLockData>,
    pub track_send_plocks: Vec<TrackSendPLockData>,
    /// Transient graph bindings for bus-send p-lock dispatch. Authoring locks
    /// remain keyed by stable BusId and survive graph reconstruction.
    pub track_send_runtime_targets: Mutex<Vec<Vec<TrackSendRuntimeTarget>>>,
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
    /// Audio *device* output latency, in seconds (f32 bits): the cost of the
    /// output buffer path, published once when the stream is built.
    pub record_latency_seconds: AtomicU32,
    /// Plugin-delay-compensation latency of the current graph, in seconds
    /// (f32 bits), republished by the latency planner whenever the plan
    /// changes. Held separately from `record_latency_seconds` so the device
    /// term and the graph term compose rather than clobber each other; note
    /// timestamping uses the sum, via
    /// [`SequencerState::total_record_latency_seconds`].
    pub pdc_latency_seconds: AtomicU32,
    /// Monotonic audio-clock anchor published by the audio callback.
    pub record_clock: RecordClockAnchor,
    pub metronome_enabled: AtomicBool,
    pub record_quantize_thresh: AtomicU32,
    /// Roll mode toggle (docs/rolling-core-spec.md 3): while on, held live
    /// keys retrigger on the roll grid instead of firing immediately.
    pub roll_mode: AtomicBool,
    /// Current roll rate as a `Timebase` discriminant. Read fresh by the
    /// scheduler every chunk (feel invariant F2), never cached.
    pub roll_rate: AtomicU32,
    /// Momentary: the sequence-roll key is held (sequencer rolling is phase 2;
    /// the state is published now so UI and commands stay in lockstep).
    pub sequence_rolling: AtomicBool,
    /// Scheduler-published sequence-roll window per track (f64 bits). A NaN
    /// start marks a non-participating track; lengths are zero in that case.
    pub roll_window_starts: Vec<AtomicU64>,
    pub roll_window_lengths: Vec<AtomicU64>,
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

    /// Invalidate the anchor so `sample` returns `None` until the audio
    /// callback publishes a fresh one. Called on the playing→stopped
    /// transition: the anchor otherwise freezes at the last played beat, and
    /// a key press in the window between the next Play and the first playing
    /// audio block would resolve against that stale clock (the note lands at
    /// the previous run's playhead instead of the reset one).
    pub fn invalidate(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
        self.timestamp_nanos.store(0, Ordering::Relaxed);
        self.beats_bits
            .store(0.0_f64.to_bits(), Ordering::Relaxed);
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
    /// Drum rack v2 choke assignments, one entry per track, published by the
    /// control thread whenever rack membership or a pad's choke group changes
    /// (`App::publish_rack_choke_runtime`) and read lock-free on the audio
    /// thread. `0` means the track is not a rack pad with a choke group;
    /// otherwise see [`rack_choke_key`].
    pub rack_choke_keys: Vec<AtomicU64>,
}

/// Packs a drum rack v2 choke assignment into the value stored in
/// [`RuntimeBindingState::rack_choke_keys`]: the owning rack's group id in the
/// high bits, the choke group in the low 8. Two tracks choke each other iff
/// their keys are equal and non-zero, so the audio thread never has to walk
/// group membership. Choke group `0` means "unassigned" and packs to `0`.
pub fn rack_choke_key(group_id: u64, choke_group: u8) -> u64 {
    if choke_group == 0 {
        return 0;
    }
    (group_id << 8) | choke_group as u64
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

/// Live step-param print override (bead eseq-jc9), read lock-free by the
/// scheduler. While the *step* panel's print latch is armed, the scheduler
/// substitutes these values when it resolves step events on the armed track:
/// the pattern write only lands BEHIND the playhead (each step is stamped as
/// it passes, after it was already scheduled), so without this override the
/// printed value would only be heard one loop later. `mask` bits:
/// 1 = velocity, 2 = duration, 4 = transpose; 0 = disarmed. Values are f32
/// bits. Written by the control thread on latch/disarm, read per scheduled
/// step by the scheduler.
#[derive(Default)]
pub struct StepPrintOverride {
    track: std::sync::atomic::AtomicUsize,
    mask: AtomicU32,
    velocity_bits: AtomicU32,
    duration_bits: AtomicU32,
    transpose_bits: AtomicU32,
}

impl StepPrintOverride {
    pub const VELOCITY: u32 = 1;
    pub const DURATION: u32 = 1 << 1;
    pub const TRANSPOSE: u32 = 1 << 2;

    /// Publish the full latch state: values first, mask last (Release), so a
    /// reader that observes a set bit never sees a stale value for it.
    pub fn set(
        &self,
        track: usize,
        velocity: Option<f32>,
        duration: Option<f32>,
        transpose: Option<f32>,
    ) {
        self.track.store(track, Ordering::Relaxed);
        let mut mask = 0;
        if let Some(value) = velocity {
            self.velocity_bits.store(value.to_bits(), Ordering::Relaxed);
            mask |= Self::VELOCITY;
        }
        if let Some(value) = duration {
            self.duration_bits.store(value.to_bits(), Ordering::Relaxed);
            mask |= Self::DURATION;
        }
        if let Some(value) = transpose {
            self.transpose_bits.store(value.to_bits(), Ordering::Relaxed);
            mask |= Self::TRANSPOSE;
        }
        self.mask.store(mask, Ordering::Release);
    }

    pub fn clear(&self) {
        self.mask.store(0, Ordering::Relaxed);
    }

    /// The latched (velocity, duration, transpose) overrides for `track`, all
    /// `None` when disarmed or armed for a different track.
    pub fn values_for_track(&self, track: usize) -> (Option<f32>, Option<f32>, Option<f32>) {
        let mask = self.mask.load(Ordering::Acquire);
        if mask == 0 || self.track.load(Ordering::Relaxed) != track {
            return (None, None, None);
        }
        let value = |bit: u32, bits: &AtomicU32| {
            (mask & bit != 0).then(|| f32::from_bits(bits.load(Ordering::Relaxed)))
        };
        (
            value(Self::VELOCITY, &self.velocity_bits),
            value(Self::DURATION, &self.duration_bits),
            value(Self::TRANSPOSE, &self.transpose_bits),
        )
    }
}

pub struct SequencerState {
    pub pattern: PatternState,
    pub transport: TransportState,
    /// Live step-param print override (bead eseq-jc9): scheduler-side
    /// substitution so the printed value is audible the moment it is touched.
    pub step_print_override: StepPrintOverride,
    pub runtime: RuntimeBindingState,
    pub(super) scheduler_snapshot: Mutex<Arc<SequencerSnapshot>>,
    pub(super) scheduler_snapshot_version: AtomicU64,
    /// Realtime-safe delivery of the published snapshot to the audio callback,
    /// and of the outgoing snapshot back to a non-realtime thread that frees it
    /// (bead eseq-sj01). The audio thread never touches `scheduler_snapshot`.
    pub(super) snapshot_handoff: SchedulerSnapshotHandoff,
    /// Depth of the active `publish coalescing` scopes. While non-zero,
    /// `publish_scheduler_snapshot` records the intent in
    /// `pending_coalesced_publish` instead of capturing, and the scope's exit
    /// performs one capture for the whole transition (bead eseq-sj01).
    pub(super) publish_coalesce_depth: AtomicU64,
    pub(super) pending_coalesced_publish: AtomicBool,
    /// Command-thread macro values waiting to be folded into the next
    /// immutable scheduler snapshot. The scheduler never reads this lock.
    pub(super) live_macro_overrides: Mutex<HashMap<crate::macro_engine::MacroParamKey, f32>>,
    pub(super) rack_macro_runtime_values: Arc<RackMacroRuntimeValues>,
    pub(super) neural_visualization: Mutex<NeuralVisualizationSnapshot>,
    pub(super) graph_visualizations: Mutex<Vec<GraphVisualizationSnapshot>>,
    pub(super) graph_control_commands: Mutex<Vec<crate::graph::GraphControlCommand>>,
    /// Control-thread roll commands, drained at the top of every scheduler
    /// worker iteration (docs/rolling-core-spec.md 3).
    pub(super) roll_commands: Mutex<Vec<crate::sequencer::RollCommand>>,
    /// Scheduler → control-thread rolled-hit feedback, drained in the UI
    /// reactive tick and written back on note release
    /// (docs/rolling-core-spec.md 6).
    pub(super) roll_recorded_hits: Mutex<Vec<crate::sequencer::RollHitRecorded>>,
    /// Audio-callback → control-thread live note-on stamps (bead eseq-2awi):
    /// the render-timeline beat each live keyboard/pad trigger actually
    /// sounded at, drained in the UI reactive tick and at note release to
    /// reposition the recorded step (record-as-heard for unquantized live
    /// recording). Realtime-safe SPSC ring — the callback never locks.
    pub(super) live_trigger_stamps: crate::sequencer::LiveTriggerStampRing,
    pub(super) track_output_events: Mutex<Vec<TrackOutputEvent>>,
    pub(super) track_output_current_beat_bits: AtomicU64,
    pub(super) active_note_until_samples: Vec<[AtomicU64; 128]>,
    pub(super) active_note_velocity_bits: Vec<[AtomicU32; 128]>,
    pub(super) live_note_velocity_bits: Vec<[AtomicU32; 128]>,
    pub(super) active_note_trigger_ids: Vec<[AtomicU64; 128]>,
    pub(super) active_note_trigger_sequence: AtomicU64,
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
    /// Control-thread channel writes awaiting the scheduler
    /// (docs/jaki-live-channel-widgets-spec.md 7). These deliberately do not
    /// ride `published_process_authoring`: `ProcessRuntime::sync_channels`
    /// prefers an existing runtime value over the authored initial, so a value
    /// smuggled in as an initial would be dropped. The lookahead worker drains
    /// this queue at the top of a chunk instead.
    pub(super) pending_process_channel_writes: Mutex<Vec<(String, crate::process::ProcessLiteral)>>,
    /// Latest scheduler-owned value for each value channel. Inline channel
    /// widgets poll this mirror; values stay scheduler-owned and are copied
    /// here only in the thread-safe process-literal representation.
    pub(super) process_channel_values: Mutex<HashMap<String, crate::process::ProcessLiteral>>,
    /// Incremented only when `process_channel_values` changes, so the
    /// event-driven UI can request a frame that polls inline bindings.
    pub(super) process_channel_values_version: AtomicU64,
    pub(super) scratch_effect_descriptors: Mutex<Vec<Vec<EffectDescriptor>>>,
    pub(super) scratch_instrument_descriptors: Mutex<Vec<EffectDescriptor>>,
    pub(super) process_trace_enabled: AtomicBool,
    pub(super) pending_accumulator_reset_all: AtomicBool,
    pub(super) pending_accumulator_reset_tracks: [AtomicBool; MAX_TRACKS],
    pub(super) quantized_launches: crate::quantized_launch::QuantizedLaunchMailbox,
    /// Sequenced mixer-control holds (jaki mute/solo routes): the scheduler
    /// lookahead pushes sample-stamped holds; the app thread drains due ones
    /// each frame (docs/jaki-mixer-control-routes-spec.md).
    pub(super) scheduled_mixer_controls: crate::mixer_control::MixerControlMailbox,
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
    /// Scene-scoped manual-override latch (takes spec 10): set by a manual
    /// SCENE launch during song playback/capture. While set, the song's
    /// SCENE-LEVEL authority is suspended too — the control-side row mirror
    /// must not move the session's current scene, the `current_pattern`
    /// atomic, or the bus pattern (scene-keyed reactive bindings and bus/
    /// group fx recall would audibly re-apply the row's scene over the
    /// performer's launch). Cleared with the track latch (Back to Song,
    /// transport stop, punch-out); per-track Back to Song leaves it.
    /// Transient transport state; never serialized.
    pub(super) song_scene_latch: AtomicBool,
    /// Which lanes the CURRENTLY MIRRORED song row resolves to a take chunk
    /// (takes spec 11.2 UX). Written by the control-side row mirror; read by
    /// `track_pattern_cells` so the mixer clip grid never marks a scene clip
    /// "playing" while the lane is actually playing a take. Transient
    /// transport state; never serialized. Bit `t` = track `t`.
    pub(super) song_take_lane_mask: AtomicU64,
    /// True while the user is standing in the ARRANGEMENT view (track-sound
    /// spec §2.2.2). Ownership of a lane's sound is view-keyed, and the
    /// state-side consumers (save-back masks, the stop resync,
    /// `mirror_device_pattern_id`) cannot see the `App` — so the App mirrors
    /// its `arrangement_view_visible` here on every view switch. Control-side
    /// intent, not transport state; never serialized.
    pub(super) arrangement_context: AtomicBool,
    /// Tracks whose live device state is on loan to a sound binding (takes
    /// spec 16.2): the mirror shows a take's or a track clip's frozen
    /// devices instead of the effective scene pattern's. Any session
    /// save-back (`capture_current_pattern_snapshot`) would otherwise write
    /// the borrowed sound over the scene pattern, so capture releases these
    /// lanes first. Transient; never serialized. Bit `t` = track `t`.
    pub(super) sound_binding_borrowed: AtomicU64,
    /// Per borrowed track, the pool pattern whose device state the live
    /// mirror is showing. Device edits key off "is this pattern the live
    /// mirror?" (`mirror_device_pattern_id`), which is the bound pattern
    /// while a lane is borrowed and the effective scene pattern otherwise.
    pub(super) sound_binding_patterns: Mutex<HashMap<usize, PatternId>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackOutputEvent {
    pub track: usize,
    pub sample_time: u64,
    pub beat: f64,
    pub transpose: f32,
    pub velocity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveNoteActivity {
    pub note: u8,
    pub velocity: f32,
    /// Monotonic identity of the most recent note-on for this track and pitch.
    /// A changed ID is an onset even when the note never left the active set.
    pub trigger_id: u64,
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
