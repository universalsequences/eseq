use super::*;

/// Inline editor session state (instrument/effect creation/editing) plus the
/// lisp authoring-transaction checkpoints, bundled out of the event loop.
#[cfg_attr(test, derive(Default))]
pub(crate) struct EditSessionState {
    pub(crate) editor_buffer_name: Option<String>,
    pub(crate) editor_mode: Option<String>,
    pub(crate) instrument_edit_session: Option<InstrumentEditSession>,
    pub(crate) pending_instrument_preview: Option<PendingInstrumentPreview>,
    pub(crate) pending_instrument_cancel_restore: Option<PendingInstrumentCancelRestore>,
    pub(crate) pending_saved_instrument_load: Option<PendingSavedInstrumentLoad>,
    pub(crate) pending_key_lock_auditions: Vec<PendingKeyLockAudition>,
    pub(crate) effect_edit_session: Option<EffectEditSession>,
    pub(crate) pending_effect_preview: Option<PendingEffectPreview>,
    pub(crate) pending_effect_cancel_restore: Option<PendingEffectCancelRestore>,
    pub(crate) script_draft_session: Option<ScriptDraftSession>,
    pub(crate) pending_agentic_bubbles: HashMap<String, PendingAgenticBubble>,
    pub(crate) pending_learn_job: Option<PendingLearnJob>,
    pub(crate) learn_param_preview: Option<LearnParamPreview>,
    pub(crate) pending_lisp_history_transactions: HashMap<
        u64,
        (
            String,
            sequencer::app::history::UndoManager<sequencer::app::history::EditPatch>,
            usize,
        ),
    >,
}

/// Meter/CPU/modulator polling caches: values read from the audio graph at a
/// throttled rate and reused between polls.
pub(crate) struct MeterCache {
    pub(crate) cached_peak_l_level: f64,
    pub(crate) cached_peak_r_level: f64,
    pub(crate) cached_track_peak_levels: Vec<f64>,
    pub(crate) cached_rack_slot_peak_levels: Vec<Vec<f64>>,
    pub(crate) cached_bus_peak_levels: Vec<f64>,
    pub(crate) cached_modulator_phases: Vec<f64>,
    pub(crate) cached_modulator_levels: Vec<f64>,
    pub(crate) cached_cpu_load_bits: u32,
    pub(crate) last_meter_poll_at: Instant,
    pub(crate) last_cpu_ui_poll_at: Instant,
    pub(crate) last_neural_visualization_poll_at: Instant,
    pub(crate) visualization_liveness: VisualizationLiveness,
    pub(crate) last_voice_count_log_at: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParamSyncRevision {
    pub(crate) track: usize,
    pub(crate) scene: usize,
    pub(crate) pattern_epoch: u64,
    pub(crate) song_row_mirror_epoch: u64,
    pub(crate) ui_epoch: usize,
    pub(crate) fx_epoch: usize,
    pub(crate) sound_binding_epoch: usize,
    pub(crate) display_step: Option<usize>,
    pub(crate) selected_steps: Vec<usize>,
    pub(crate) selected_neural_neurons:
        Vec<sequencer::lisp_host::SelectedNeuralNeuron>,
}

/// Previous-frame values the reactive tick diffs against to decide which
/// reactives to republish.
#[cfg_attr(test, derive(Default))]
pub(crate) struct FrameDiffState {
    pub(crate) prev_editor_macro_action: (String, String),
    /// Hash of the macro-action query's inputs (edit-session path/source/
    /// validity plus the patcher's open macro view); the origin lookup only
    /// reruns when this changes.
    pub(crate) prev_editor_macro_action_fingerprint: u64,
    /// Hash of the active edit-session source; the macro sidebar values
    /// (`editor-patch-macros` / `editor-library-macros`) republish on change.
    pub(crate) prev_editor_macro_sidebar_fingerprint: u64,
    /// Macro view open in the patcher ("" = root), mirrored to
    /// `SEQ.editor-open-macro` for the sidebar's selected row.
    pub(crate) prev_editor_open_macro: String,
    pub(crate) prev_playing: bool,
    pub(crate) prev_bpm: u32,
    pub(crate) prev_playhead: u32,
    pub(crate) prev_transport_playhead: u32,
    pub(crate) prev_pattern_epoch: u64,
    /// Diffed against `App::song_row_mirror_epoch` so mirrored song-row
    /// transitions (which never bump the real pattern epoch) still trigger
    /// the full pattern-switch resync.
    pub(crate) prev_song_row_mirror_epoch: u64,
    pub(crate) prev_current_track: usize,
    pub(crate) prev_cpu_load_bits: u32,
    pub(crate) prev_peak_l_level: f64,
    pub(crate) prev_peak_r_level: f64,
    /// Whether the recording-take undo transaction is open. Mirrors
    /// (recording armed && transport playing) via
    /// `App::sync_recording_history_boundary`; force it false whenever
    /// recording is forced off outside that seam.
    pub(crate) recording_history_open: bool,
    pub(crate) prev_master_recording: bool,
    pub(crate) prev_selected_tracks: HashSet<usize>,
    pub(crate) prev_groups: Vec<sequencer::project::ProjectTrackGroup>,
    pub(crate) prev_track_peak_levels: Vec<f64>,
    pub(crate) prev_rack_slot_peak_levels: Vec<Vec<f64>>,
    pub(crate) prev_bus_peak_levels: Vec<f64>,
    pub(crate) prev_modulator_phases: Vec<f64>,
    pub(crate) prev_modulator_levels: Vec<f64>,
    pub(crate) prev_bus_playheads: Vec<usize>,
    pub(crate) prev_track_playheads: Vec<u32>,
    pub(crate) prev_track_button_states: Vec<(bool, bool)>,
    pub(crate) prev_current_track_playhead_visible: bool,
    pub(crate) prev_ui_epoch: usize,
    pub(crate) prev_fx_epoch: usize,
    pub(crate) prev_fx_value_epoch: usize,
    pub(crate) prev_sound_binding_epoch: usize,
    /// Arming/clearing a delete target republishes only the delete-target
    /// read surfaces (version reactive + mixer/rack binding fields) off this
    /// counter — it deliberately does NOT ride `ui_epoch`.
    pub(crate) prev_delete_target_version: usize,
    pub(crate) track_param_sync_revision: Option<ParamSyncRevision>,
    pub(crate) fx_param_sync_revision: Option<ParamSyncRevision>,
    /// Identity of the CLIP-derived piano-roll surfaces (clip panel, window
    /// overlay, clip kind): `(selected (track, clip id), clip source kind,
    /// committed-song revision)`. They are keyed off the clip SELECTION,
    /// which can move while the resolved write focus stays put, so the focus
    /// spec alone is not enough to decide whether they need republishing.
    pub(crate) prev_focus_clip_surface: (Option<(usize, u64)>, Option<&'static str>, u64),
    pub(crate) prev_instrument_active_notes: Vec<u8>,
    pub(crate) prev_track_active_notes: Vec<Vec<sequencer::sequencer::ActiveNoteActivity>>,
    pub(crate) prev_active_buffer_name: String,
    pub(crate) prev_selected_neural_neurons:
        BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    pub(crate) prev_agent_generation_watermark: u64,
    pub(crate) prev_sampler_analysis_key: Option<(usize, i32, u32, u32, usize)>,
    pub(crate) prev_auto_follow: bool,
    /// Browser sample preview: last published playing flag; the playhead
    /// republishes every tick while true.
    pub(crate) prev_browser_preview_playing: bool,
    pub(crate) prev_queued_transport_scene: Option<usize>,
    /// Per-track pattern id (-1 = none) with a pending quantized clip
    /// launch, for the mixer grid's queued-cell blink.
    pub(crate) prev_queued_track_clips: Vec<i64>,
    /// Song-mode reactive diff state (docs/song-mode-spec.md 12).
    pub(crate) song: SongFrameState,
    /// Sound-palette reactive diff state (takes spec §17.6/§18.3).
    pub(crate) sound_palette: SoundPaletteFrameState,
    pub(crate) watched_sampler_voice_track: Option<usize>,
    pub(crate) watched_sampler_voice_ids: Vec<i32>,
}

/// In-flight pointer-gesture state that host commands need to observe or
/// reset (e.g. committing a rack drag's scheduler snapshot once at gesture
/// end).
#[cfg_attr(test, derive(Default))]
pub(crate) struct GestureState {
    pub(crate) rack_control_snapshot_dirty: bool,
    pub(crate) piano_roll_history_gesture: Option<ActivePianoRollHistoryGesture>,
    pub(crate) preview_plock_variant: Option<(usize, String)>,
}

/// Shared handles threaded between the event loop, lisp natives, and the
/// audio engine. All fields are cheaply clonable mirrors of the handles that
/// `init_runtime` captures.
pub(crate) struct SharedHandles {
    pub(crate) state: Arc<SequencerState>,
    pub(crate) lg_raw: *mut sequencer::audiograph::LiveGraph,
    pub(crate) current_track: Arc<AtomicUsize>,
    pub(crate) selected_tracks: Arc<Mutex<HashSet<usize>>>,
    pub(crate) selected_steps: Arc<Mutex<HashSet<usize>>>,
    pub(crate) selected_neural_neurons: sequencer::lisp_host::SharedSelectedNeuralNeurons,
    pub(crate) piano_roll_selection: Arc<Mutex<HashSet<u64>>>,
    pub(crate) piano_roll_move_state: Arc<Mutex<Option<PianoRollMoveState>>>,
    pub(crate) piano_roll_focus: SharedPianoRollFocus,
    pub(crate) step_clipboard:
        Arc<Mutex<Option<(usize, Vec<(usize, sequencer::sequencer::StepSnapshot)>)>>>,
    pub(crate) ui_epoch: Arc<AtomicUsize>,
    /// Structural fx invalidation: the tick's fx branch fully republishes the
    /// effects/instrument-panel reactives (dirty marks, *fx* re-eval). Bump
    /// this for edits that change panel STRUCTURE (Boolean/Enum params drive
    /// conditional layout, add/remove effect, ...).
    pub(crate) fx_epoch: Arc<AtomicUsize>,
    /// Value-only fx invalidation: the tick's fx branch republishes via
    /// `set_reactive_value_patch` (in-place Number/Bool cell writes, NO dirty
    /// marks — field bindings carry the visible updates). Only scene/clip
    /// launch paths may bump this; anything that can change panel structure
    /// must use `fx_epoch` instead.
    pub(crate) fx_value_epoch: Arc<AtomicUsize>,
    pub(crate) ui_invalidations: Arc<UiInvalidationQueue>,
    pub(crate) expanded_step_projection: Arc<ExpandedStepProjectionRegistry>,
    pub(crate) active_delete_target: Arc<Mutex<Option<ActiveDeleteTarget>>>,
    pub(crate) active_delete_target_version: Arc<AtomicUsize>,
    pub(crate) auto_follow_override_until: Arc<Mutex<Option<Instant>>>,
    pub(crate) track_pan_ids: Arc<Mutex<Vec<i32>>>,
    pub(crate) track_collapsed: Arc<Mutex<Vec<bool>>>,
    pub(crate) bus_state: Arc<Mutex<Vec<app::BusChannelState>>>,
    pub(crate) bus_node_ids: Arc<Mutex<Vec<app::BusNodeIds>>>,
    pub(crate) track_groups: Arc<Mutex<Vec<sequencer::project::ProjectTrackGroup>>>,
    pub(crate) record_armed: Arc<Mutex<Vec<bool>>>,
    pub(crate) recording: Arc<AtomicBool>,
    pub(crate) master_recording: Arc<AtomicBool>,
    pub(crate) held_notes: Arc<Mutex<Vec<HeldKeyboardNote>>>,
    pub(crate) keyboard_octave: Arc<std::sync::atomic::AtomicI32>,
    pub(crate) sample_browser: Rc<RefCell<DebouncedSampleBrowser>>,
    pub(crate) keyboard_tx: std::sync::mpsc::Sender<KeyboardTrigger>,
    pub(crate) accumulator_names: Arc<Mutex<Vec<String>>>,
    pub(crate) piano_roll_clipboard: PianoRollClipboard,
    /// Arrangement region clipboard (region spec 5.1). Lives beside the
    /// piano-roll clipboard for the same reason: copy/paste are host commands
    /// applied where the loop context is in scope, not `App` state.
    pub(crate) arrangement_clipboard: app::song_region::ArrangementClipboardHandle,
    pub(crate) selected_drum_lane_steps: Arc<Mutex<HashSet<DrumLaneStepSelection>>>,
}

/// Borrowed bundle of the event loop's grouped state, passed to the
/// host-command dispatcher so extracted handlers keep mutating the same
/// per-loop values `fn main` owns.
pub(crate) struct LoopCtx<'a> {
    pub(crate) sessions: &'a mut EditSessionState,
    pub(crate) meters: &'a mut MeterCache,
    pub(crate) frame: &'a mut FrameDiffState,
    pub(crate) gesture: &'a mut GestureState,
    pub(crate) track_names: &'a mut Vec<String>,
    pub(crate) shared: &'a SharedHandles,
}
