/*!
Shared eval-context state and registries that both lisp_host language sides
depend on.

Defines the `Shared*` type aliases (`SharedSequencerEvalContext`,
`SharedAccumulatorEvalContext`, `SharedProcessAuthoring`, …) — `Arc<Mutex<…>>`
handles threaded through every native so registration code, the UI, and the
scheduler can exchange state — together with the structs behind them:
`RegisteredAccumulator` / `RegisteredSequencer` (lisp-defined accumulators,
MIDI FX, and `def-sequencer` generators), `ProcessAuthoringRegistry`, the
per-invocation eval contexts, and `EmittedAccumulatorEvent`, the payload
every lisp emit path produces.

Also home to [`ScratchControlRuntime`]: an eseqlisp `Runtime` bundled with
all of the above, which the UI scratch buffer and the scheduler both drive
(`eval`, `invoke_accumulator`, `invoke_midi_fx`, `invoke_process_run`,
`invoke_sequencer_tick`, …). This file is the seam the rest of `lisp_host`
plugs into, which is why it was extracted first in the split.
*/

use super::*;

#[derive(Clone, Debug, Default)]
pub(crate) struct SequencerEvalContext {
    pub(super) track: usize,
    pub(super) cursor_step: usize,
}

pub(super) type SharedSequencerEvalContext = Arc<Mutex<SequencerEvalContext>>;

#[derive(Clone, Debug, Default)]
pub(crate) struct SequencerNativeMetadata {
    pub(super) effect_descriptors: Vec<Vec<EffectDescriptor>>,
    pub(super) instrument_descriptors: Vec<EffectDescriptor>,
}

pub(super) type SharedSequencerNativeMetadata = Arc<Mutex<SequencerNativeMetadata>>;

#[derive(Clone)]
pub(crate) struct RegisteredAccumulator {
    pub(super) name: String,
    pub(super) callback: RegisteredAccumulatorCallback,
    pub(super) params: Vec<crate::effects::ParamDescriptor>,
}

#[derive(Clone)]
pub(super) enum RegisteredAccumulatorCallback {
    Source(String),
    Closure(EValue),
}

pub(super) type SharedRegisteredAccumulators = Arc<Mutex<Vec<RegisteredAccumulator>>>;
pub(super) type SharedRegisteredMidiFx = Arc<Mutex<Vec<RegisteredAccumulator>>>;
pub(super) type SharedPendingMidiFxParams = Arc<Mutex<Vec<crate::effects::ParamDescriptor>>>;
pub(super) type SharedMidiFxState = Arc<Mutex<HashMap<String, EValue>>>;

#[derive(Clone)]
pub(crate) struct AccumulatorEvalContext {
    pub(super) step_index: usize,
    pub(super) resolved: ResolvedStep,
    pub(super) chord: Vec<f32>,
    pub(super) chord_durations: Vec<f32>,
    pub(super) chord_step_transpose: f32,
    pub(super) note_spans: Option<Vec<AccumulatorNoteSpan>>,
    pub(super) midi_fx_scope: Option<(usize, String)>,
    pub(super) midi_fx_slot: EffectSlotSnapshot,
    pub(super) midi_fx_param_names: Vec<String>,
    pub(super) arp_phase_beats: f32,
    pub(super) step_beats: f32,
    pub(super) num_steps: usize,
    pub(super) suppressed: bool,
    pub(super) effect_slots: Vec<EffectSlotSnapshot>,
    pub(super) instrument_slot: EffectSlotSnapshot,
    pub(super) effect_params: Vec<ScheduledEffectParam>,
    pub(super) instrument_params: Vec<ScheduledInstrumentParam>,
    pub(super) emitted: Vec<EmittedAccumulatorEvent>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmittedAccumulatorEvent {
    pub offset_beats: f32,
    pub track: Option<usize>,
    pub resolved: ResolvedStep,
    pub chord: Vec<f32>,
    pub chord_durations: Vec<f32>,
    pub chord_step_transpose: f32,
    pub effect_params: Vec<ScheduledEffectParam>,
    pub instrument_params: Vec<ScheduledInstrumentParam>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccumulatorNoteSpan {
    pub transpose: f32,
    pub start_beats: f32,
    pub end_beats: f32,
}

#[derive(Clone)]
pub struct AccumulatorEvalOutput {
    pub resolved: ResolvedStep,
    pub suppressed: bool,
    pub effect_params: Vec<ScheduledEffectParam>,
    pub instrument_params: Vec<ScheduledInstrumentParam>,
    pub emitted: Vec<EmittedAccumulatorEvent>,
}

pub(super) type SharedAccumulatorEvalContext = Arc<Mutex<Option<AccumulatorEvalContext>>>;

pub(super) type SharedRegisteredSequencers = Arc<Mutex<Vec<RegisteredSequencer>>>;
pub(super) type SharedGeneratorTickContext = Arc<Mutex<Option<GeneratorTickContext>>>;
pub(super) type SharedProcessAuthoring = Arc<Mutex<ProcessAuthoringRegistry>>;
pub(super) type SharedProcessEvalContext = Arc<Mutex<Option<ProcessEvalContext>>>;
/// Process-channel values and their payload generation, published atomically
/// for generator ticks once per lookahead chunk.
#[derive(Clone, Default)]
pub(super) struct GeneratorChannelSnapshot {
    pub(super) payload_epoch: u32,
    pub(super) values: Arc<HashMap<String, EValue>>,
}

pub(super) type SharedGeneratorChannels = Arc<Mutex<GeneratorChannelSnapshot>>;
pub(super) type SharedSceneSlotSnapshot = Arc<Mutex<Arc<crate::sequencer::SceneSlotStore>>>;
pub(super) type ProcessPublishHook = Arc<dyn Fn(crate::process::PublishedProcessAuthoringSnapshot) + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProcessEvalScope {
    Run,
    RatchetShape,
}

#[derive(Clone)]
pub struct PublishedProcessAuthoringNatives {
    pub(super) process_authoring: SharedProcessAuthoring,
    pub(super) process_chain_state: Arc<crate::sequencer::SequencerState>,
    pub(super) publish: Option<ProcessPublishHook>,
}

impl PublishedProcessAuthoringNatives {
    pub fn define_process_accumulator(
        &self,
        args: Vec<eseqlisp::vm::Value>,
        vm: &mut eseqlisp::vm::VM,
    ) -> Result<eseqlisp::vm::Value, String> {
        register_process_accumulator_def(
            args,
            vm,
            &self.process_authoring,
            Some(Arc::clone(&self.process_chain_state)),
            self.publish.clone(),
        )
    }

    /// Freeze the definitions registered so far as the package layer (see
    /// [`ProcessAuthoringRegistry::mark_package_defs`]).
    pub fn mark_package_defs(&self) {
        if let Ok(mut registry) = self.process_authoring.lock() {
            registry.mark_package_defs();
        }
    }

    /// Drop the outgoing project's authored processes, channels, patches and
    /// conductor attachments, then republish so every consumer of the
    /// published snapshot sees the cleared registry (bead eseq-jo7.21).
    pub fn reset_project_authored(&self) {
        if let Ok(mut registry) = self.process_authoring.lock() {
            registry.reset_project_authored();
        }
        publish_process_authoring(&self.process_authoring, &self.publish);
    }
}
pub(super) const UI_PROCESS_HANDLE_BASE: u64 = 1_u64 << 48;
/// A lisp `def-sequencer` definition as held by the scheduler-side VM: its id
/// (stable hash of the name, for hot-reload matching), display name, `:resolution`
/// timebase, and the `:tick` closure to invoke per boundary crossing.
#[derive(Clone)]
pub(crate) struct RegisteredSequencer {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) resolution: Timebase,
    pub(super) tick: RegisteredAccumulatorCallback,
}

pub(super) struct CompiledSequencerTick {
    source: String,
    callback: Result<EValue, String>,
}

/// Per-invocation context for a generator `:tick`, mirroring [`AccumulatorEvalContext`]
/// but for self-clocked generators: musical position only (no source step), an RNG
/// cell for `gen-rand`, and the buffer that `seq-emit` pushes into.
pub(crate) struct GeneratorTickContext {
    pub(super) tick_index: u64,
    pub(super) beat: f64,
    pub(super) resolution_beats: f64,
    pub(super) random_state: u64,
    pub(super) state: HashMap<String, f64>,
    pub(super) emitted: Vec<EmittedAccumulatorEvent>,
    /// Mixer-control holds pushed by `seq-emit-control`
    /// (docs/jaki-mixer-control-routes-spec.md).
    pub(super) controls: Vec<crate::mixer_control::EmittedMixerControl>,
}

#[derive(Default)]
pub(super) struct ProcessAuthoringRegistry {
    pub(super) next_handle_id: u64,
    /// The value `next_handle_id` started at, so a project switch can rewind
    /// handle minting to the same base the registry was constructed with.
    pub(super) handle_base: u64,
    /// Ids of the `def-process` definitions that belong to the package layer
    /// (`content/processes/builtin.lisp`, evaluated once at runtime
    /// construction). These are library classes, not live state, so they
    /// survive a project switch; everything else in this registry is
    /// project-authored and is dropped (bead eseq-jo7.21).
    pub(super) package_def_ids: HashSet<u64>,
    pub(super) defs: Vec<crate::process::ProcessDef>,
    pub(super) instances: Vec<crate::process::AuthoredProcessInstance>,
    pub(super) channels: Vec<crate::process::AuthoredChannel>,
    pub(super) patches: Vec<crate::process::AuthoredPatch>,
    pub(super) conductors: Vec<crate::process::AuthoredConductorAttachment>,
    pub(super) outlet_handles: HashMap<u64, crate::process::ProcessOutletRef>,
    pub(super) channel_handles: HashMap<u64, String>,
    /// Channel writes made through a channel handle, awaiting a drain onto the
    /// scheduler (docs/jaki-live-channel-widgets-spec.md 7). Kept off
    /// [`ProcessAuthoringRegistry::snapshot`] on purpose: `sync_channels`
    /// prefers an existing runtime value over the authored initial, so a write
    /// carried as an initial would be silently swallowed.
    pub(super) pending_channel_writes: Vec<(String, crate::process::ProcessLiteral)>,
    /// Last value written to each channel through a handle. Survives the drain
    /// so an inline widget bound to the channel reports the author's own last
    /// value rather than snapping back to the `defchan` initial.
    pub(super) channel_write_echo: HashMap<String, crate::process::ProcessLiteral>,
}

impl ProcessAuthoringRegistry {
    pub(super) fn with_handle_base(handle_base: u64) -> Self {
        Self {
            next_handle_id: handle_base,
            handle_base,
            ..Self::default()
        }
    }

    /// Freeze the currently registered definitions as the package layer. Call
    /// this once, right after the process library has been evaluated into the
    /// runtime and before any project source can reach it.
    pub(super) fn mark_package_defs(&mut self) {
        self.package_def_ids = self.defs.iter().map(|def| def.id).collect();
    }

    /// Drop every project-authored entry, keeping only the package
    /// definitions marked by [`ProcessAuthoringRegistry::mark_package_defs`].
    /// Instances, channels, patches and conductor attachments are live state
    /// keyed to the outgoing project's tracks and handles, so none of them
    /// survive — otherwise the previous project's jaki sequencers keep
    /// scheduling under the new one.
    pub(super) fn reset_project_authored(&mut self) {
        self.defs.retain(|def| self.package_def_ids.contains(&def.id));
        self.instances.clear();
        self.channels.clear();
        self.patches.clear();
        self.conductors.clear();
        self.outlet_handles.clear();
        self.channel_handles.clear();
        self.pending_channel_writes.clear();
        self.channel_write_echo.clear();
        self.next_handle_id = self.handle_base;
    }

    pub(super) fn next_id(&mut self) -> u64 {
        self.next_handle_id = self.next_handle_id.saturating_add(1).max(1);
        self.next_handle_id
    }

    pub(super) fn upsert_def(&mut self, def: crate::process::ProcessDef) {
        if let Some(existing) = self.defs.iter_mut().find(|entry| entry.id == def.id) {
            *existing = def;
        } else {
            self.defs.push(def);
        }
    }

    pub(super) fn upsert_instance(&mut self, instance: crate::process::AuthoredProcessInstance) {
        if let Some(existing) = self
            .instances
            .iter_mut()
            .find(|entry| entry.handle_id == instance.handle_id)
        {
            *existing = instance;
        } else {
            self.instances.push(instance);
        }
    }

    pub(super) fn name_instance(&mut self, handle_id: crate::process::AuthoredHandleId, name: &str) {
        self.instances
            .retain(|entry| entry.handle_id == handle_id || entry.name.as_deref() != Some(name));
        if let Some(instance) = self
            .instances
            .iter_mut()
            .find(|entry| entry.handle_id == handle_id)
        {
            instance.name = Some(name.to_string());
            instance.anonymous = false;
        }
    }

    pub(super) fn queue_channel_write(
        &mut self,
        name: String,
        value: crate::process::ProcessLiteral,
    ) {
        self.channel_write_echo.insert(name.clone(), value.clone());
        self.pending_channel_writes.push((name, value));
    }

    pub(super) fn take_pending_channel_writes(
        &mut self,
    ) -> Vec<(String, crate::process::ProcessLiteral)> {
        std::mem::take(&mut self.pending_channel_writes)
    }

    pub(super) fn name_channel(&mut self, handle_id: crate::process::AuthoredHandleId, name: &str) {
        self.channels
            .retain(|entry| entry.handle_id == handle_id || entry.name.as_deref() != Some(name));
        if let Some(channel) = self
            .channels
            .iter_mut()
            .find(|entry| entry.handle_id == handle_id)
        {
            channel.name = Some(name.to_string());
        }
    }

    pub(super) fn snapshot(&self) -> crate::process::ProcessAuthoringSnapshot {
        let live_handles = self
            .instances
            .iter()
            .map(|instance| instance.handle_id)
            .collect::<HashSet<_>>();
        crate::process::ProcessAuthoringSnapshot {
            defs: self.defs.clone(),
            instances: self.instances.clone(),
            channels: self.channels.clone(),
            patches: self.patches.clone(),
            conductors: self
                .conductors
                .iter()
                .filter(|attachment| live_handles.contains(&attachment.process_handle_id))
                .cloned()
                .collect(),
        }
    }
}

pub(crate) struct ProcessEvalContext {
    pub(super) runtime_id: u64,
    pub(super) beat: f64,
    pub(super) inlets: HashMap<String, EValue>,
    pub(super) state: HashMap<String, EValue>,
    pub(super) event: Option<EValue>,
    pub(super) step_context: Option<crate::process::ProcessStepEventContext>,
    pub(super) ports: Vec<crate::process::ProcessPortDef>,
    pub(super) reads: crate::process::ProcessReadSnapshot,
    pub(super) conductor_observe_tracks: Vec<usize>,
    pub(super) conductor_play_tracks: Vec<usize>,
    pub(super) outputs: Vec<crate::process::ProcessOutput>,
    pub(super) emissions: Vec<EmittedAccumulatorEvent>,
    pub(super) commands: Vec<crate::process::ProcessRunCommand>,
    pub(super) target_writes: Vec<crate::process::ProcessTargetWrite>,
    pub(super) transpose: Option<f32>,
    pub(super) random_state: u64,
    pub(super) scope: ProcessEvalScope,
}

pub struct ScratchControlRuntime {
    pub(super) runtime: Runtime,
    pub(super) context: SharedSequencerEvalContext,
    pub(super) metadata: SharedSequencerNativeMetadata,
    pub(super) accumulators: SharedRegisteredAccumulators,
    pub(super) midi_fx: SharedRegisteredMidiFx,
    pub(super) pending_midi_fx_params: SharedPendingMidiFxParams,
    pub(super) midi_fx_state: SharedMidiFxState,
    pub(super) accumulator_eval: SharedAccumulatorEvalContext,
    pub(super) sequencers: SharedRegisteredSequencers,
    pub(super) generator_tick: SharedGeneratorTickContext,
    pub(super) generator_channels: SharedGeneratorChannels,
    pub(super) scene_slots: SharedSceneSlotSnapshot,
    pub(super) process_authoring: SharedProcessAuthoring,
    pub(super) process_eval: SharedProcessEvalContext,
    pub(super) graph_node: SharedGraphNodeContext,
    pub(super) graph_updates: HashMap<u64, CompiledGraphUpdate>,
    sequencer_tick_callbacks: HashMap<u64, CompiledSequencerTick>,
    pub(super) process_run_callbacks: HashMap<String, EValue>,
    #[cfg(test)]
    pub(super) sequencer_tick_compile_count: usize,
    #[cfg(test)]
    pub(super) process_run_cache_enabled: bool,
    pub(super) runtime_globals: Vec<String>,
}

impl ScratchControlRuntime {
    pub fn new(
        state: Arc<crate::sequencer::SequencerState>,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
        track: usize,
        cursor_step: usize,
    ) -> Self {
        Self::new_with_process_chain_writes(
            state,
            effect_descriptors,
            instrument_descriptors,
            track,
            cursor_step,
            true,
        )
    }

    pub fn new_scheduler(
        state: Arc<crate::sequencer::SequencerState>,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
        track: usize,
        cursor_step: usize,
    ) -> Self {
        Self::new_with_process_chain_writes(
            state,
            effect_descriptors,
            instrument_descriptors,
            track,
            cursor_step,
            false,
        )
    }

    pub(super) fn new_with_process_chain_writes(
        state: Arc<crate::sequencer::SequencerState>,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
        track: usize,
        cursor_step: usize,
        write_process_chain_state: bool,
    ) -> Self {
        let context = Arc::new(Mutex::new(SequencerEvalContext { track, cursor_step }));
        let metadata = Arc::new(Mutex::new(SequencerNativeMetadata {
            effect_descriptors,
            instrument_descriptors,
        }));
        let accumulators = Arc::new(Mutex::new(Vec::new()));
        let midi_fx = Arc::new(Mutex::new(Vec::new()));
        let pending_midi_fx_params = Arc::new(Mutex::new(Vec::new()));
        let midi_fx_state = Arc::new(Mutex::new(HashMap::new()));
        let accumulator_eval = Arc::new(Mutex::new(None));
        let sequencers = Arc::new(Mutex::new(Vec::new()));
        let generator_tick = Arc::new(Mutex::new(None));
        let generator_channels: SharedGeneratorChannels =
            Arc::new(Mutex::new(GeneratorChannelSnapshot::default()));
        let scene_slots: SharedSceneSlotSnapshot = Arc::new(Mutex::new(Arc::new(
            state.latest_scheduler_snapshot().scene_slots.clone(),
        )));
        let process_authoring = Arc::new(Mutex::new(ProcessAuthoringRegistry::default()));
        let process_eval = Arc::new(Mutex::new(None));
        let graph_node: SharedGraphNodeContext = Arc::new(Mutex::new(None));
        let process_chain_state = write_process_chain_state.then(|| Arc::clone(&state));
        let mut runtime = Runtime::new();
        let app_paths = crate::app_paths::app_paths();
        runtime.set_load_root(app_paths.factory_root());
        runtime.set_scoped_module_load_path(app_paths.module_load_roots().0);
        runtime.set_theme_sync_enabled(false);
        runtime.register_native_with_docs(
            "eseq.seq-script-picker/seq-register-script-source-tab",
            "(eseq.seq-script-picker/seq-register-script-source-tab label)",
            "No-op outside the Metal Seq UI; lets source-tab scripts load in scratch/scheduler runtimes.",
            |_args, _ctx| Ok(EValue::Nil),
        );
        register_sequencer_natives_with_accumulators(
            &mut runtime,
            Arc::clone(&state),
            Arc::clone(&context),
            Arc::clone(&metadata),
            Arc::clone(&accumulators),
            Arc::clone(&midi_fx),
            Arc::clone(&pending_midi_fx_params),
            Arc::clone(&midi_fx_state),
            Arc::clone(&accumulator_eval),
            Arc::clone(&sequencers),
            Arc::clone(&generator_tick),
            Arc::clone(&generator_channels),
        );
        // Scratch callbacks execute against the immutable snapshot selected
        // for their scheduler chunk, never by locking the mutable UI scene
        // bank. Re-registering replaces the live resolver installed by the
        // general native set while retaining the same lowering targets.
        register_scene_slot_natives_with_snapshot(
            &mut runtime,
            Arc::clone(&state),
            Some(Arc::clone(&scene_slots)),
            false,
        );
        register_process_natives(
            &mut runtime,
            Arc::clone(&process_authoring),
            Arc::clone(&process_eval),
            None,
            process_chain_state.clone(),
            true,
        );
        register_process_chain_natives(
            &mut runtime,
            Arc::clone(&state),
            Arc::clone(&process_authoring),
            None,
            write_process_chain_state,
        );
        register_def_accumulator_dispatch_native(
            &mut runtime,
            Arc::clone(&accumulators),
            Arc::clone(&process_authoring),
            process_chain_state,
            None,
        );
        graph_update::register_graph_node_natives(&mut runtime, Arc::clone(&graph_node));
        register_process_graph_emit_native(&mut runtime, Arc::clone(&process_eval));
        let mut this = Self {
            runtime,
            context,
            metadata,
            accumulators,
            midi_fx,
            pending_midi_fx_params,
            midi_fx_state,
            accumulator_eval,
            sequencers,
            generator_tick,
            generator_channels,
            scene_slots,
            process_authoring,
            process_eval,
            graph_node,
            graph_updates: HashMap::new(),
            sequencer_tick_callbacks: HashMap::new(),
            process_run_callbacks: HashMap::new(),
            #[cfg(test)]
            sequencer_tick_compile_count: 0,
            #[cfg(test)]
            process_run_cache_enabled: true,
            runtime_globals: Vec::new(),
        };
        this.install_accumulator_macro();
        this.install_midi_fx_macro();
        this.refresh_runtime_globals();
        this
    }

    /// Publish the current process-channel values for `chan-get` reads inside
    /// generator `:tick` bodies. The scheduler refreshes this from the process
    /// runtime once per lookahead chunk, before ticking generators.
    pub fn set_generator_channel_values(
        &self,
        payload_epoch: u32,
        values: HashMap<String, EValue>,
    ) {
        if let Ok(mut guard) = self.generator_channels.lock() {
            *guard = GeneratorChannelSnapshot {
                payload_epoch,
                values: Arc::new(values),
            };
        }
    }

    /// Select the immutable pattern snapshot observed by shipped callbacks at
    /// the next scheduler boundary.
    pub fn set_scene_slot_snapshot(&self, slots: Arc<crate::sequencer::SceneSlotStore>) {
        // Recover through poisoning rather than skipping the update: silently
        // keeping a stale snapshot forever would make every shipped tick read
        // the previous chunk's slots with no diagnostic.
        let mut guard = self
            .scene_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = slots;
    }

    pub fn set_position(&mut self, track: usize, cursor_step: usize) {
        if let Ok(mut ctx) = self.context.lock() {
            ctx.track = track;
            ctx.cursor_step = cursor_step;
        }
        self.refresh_runtime_globals();
    }

    pub fn sync_descriptors(
        &mut self,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
    ) {
        if let Ok(mut metadata) = self.metadata.lock() {
            metadata.effect_descriptors = effect_descriptors;
            metadata.instrument_descriptors = instrument_descriptors;
        }
        self.refresh_runtime_globals();
    }

    pub fn eval(&mut self, code: &str) -> Result<Option<EValue>, String> {
        // External evaluation can redefine macros used by a shipped process
        // body without changing that body's source text. Recompile callbacks
        // after every authoring evaluation so cached macro expansion can never
        // outlive the environment that produced it.
        self.process_run_callbacks.clear();
        self.runtime.eval_str(code).map_err(|e| format!("{e:?}"))
    }

    pub fn eval_source_at_path(
        &mut self,
        path: impl Into<PathBuf>,
        code: &str,
    ) -> Result<Option<EValue>, String> {
        self.process_run_callbacks.clear();
        self.runtime
            .eval_source_at_path(path.into(), code)
            .map_err(|e| format!("{e:?}"))
    }

    pub fn take_status_message(&mut self) -> Option<String> {
        self.runtime.take_status_message()
    }

    pub fn set_theme_sync_enabled(&mut self, enabled: bool) {
        self.runtime.set_theme_sync_enabled(enabled);
    }

    pub fn set_global_value(&mut self, name: &str, value: EValue) {
        self.runtime.set_global_value(name, value);
    }

    pub(super) fn refresh_runtime_globals(&mut self) {
        self.runtime_globals = install_runtime_globals(
            &mut self.runtime,
            &self.context,
            &self.metadata,
            &self.runtime_globals,
        );
    }

    pub(super) fn install_accumulator_macro(&mut self) {}

    pub(super) fn install_midi_fx_macro(&mut self) {
        let _ = self.runtime.eval_str(
            r#"
            (defmacro def-midi-fx (name body)
              `(__register-midi-fx ,name
                 (lambda (fx-step fx-value) ,body)))
            "#,
        );
    }

    pub fn accumulator_names(&self) -> Vec<String> {
        self.accumulators
            .lock()
            .map(|registry| registry.iter().map(|entry| entry.name.clone()).collect())
            .unwrap_or_default()
    }

    pub fn midi_fx_names(&self) -> Vec<String> {
        self.midi_fx
            .lock()
            .map(|registry| registry.iter().map(|entry| entry.name.clone()).collect())
            .unwrap_or_default()
    }

    pub fn midi_fx_descriptors(&self) -> Vec<EffectDescriptor> {
        self.midi_fx
            .lock()
            .map(|registry| {
                registry
                    .iter()
                    .map(|entry| {
                        let mut desc = EffectDescriptor::empty_custom_slot();
                        desc.name = entry.name.clone();
                        desc.params = entry.params.clone();
                        desc
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn invoke_accumulator(
        &mut self,
        registry_index: usize,
        step: usize,
        value: f32,
        resolved: ResolvedStep,
        chord: Vec<f32>,
        chord_durations: Vec<f32>,
        chord_step_transpose: f32,
        note_spans: Option<Vec<AccumulatorNoteSpan>>,
        step_beats: f32,
        num_steps: usize,
        effect_slots: Vec<EffectSlotSnapshot>,
        instrument_slot: EffectSlotSnapshot,
        effect_params: Vec<ScheduledEffectParam>,
        instrument_params: Vec<ScheduledInstrumentParam>,
    ) -> Result<AccumulatorEvalOutput, String> {
        let callback = self
            .accumulators
            .lock()
            .map_err(|_| "failed to lock accumulator registry".to_string())?
            .get(registry_index)
            .map(|entry| entry.callback.clone())
            .ok_or_else(|| "registered accumulator out of range".to_string())?;
        {
            let mut eval_ctx = self
                .accumulator_eval
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            *eval_ctx = Some(AccumulatorEvalContext {
                step_index: step,
                resolved,
                chord,
                chord_durations,
                chord_step_transpose,
                note_spans,
                midi_fx_scope: None,
                midi_fx_slot: EffectSlotSnapshot::new_empty(),
                midi_fx_param_names: Vec::new(),
                arp_phase_beats: 0.0,
                step_beats,
                num_steps,
                suppressed: false,
                effect_slots,
                instrument_slot,
                effect_params,
                instrument_params,
                emitted: Vec::new(),
            });
        }
        self.runtime
            .set_global_value("acc-step", EValue::Number(step as f64));
        self.runtime
            .set_global_value("acc-value", EValue::Number(value as f64));
        match callback {
            RegisteredAccumulatorCallback::Source(source) => {
                self.runtime
                    .eval_str(&source)
                    .map_err(|e| format!("{e:?}"))?;
            }
            RegisteredAccumulatorCallback::Closure(callback) => {
                self.runtime
                    .invoke(
                        callback,
                        vec![EValue::Number(step as f64), EValue::Number(value as f64)],
                    )
                    .map_err(|e| format!("{e:?}"))?;
            }
        }
        let output = self
            .accumulator_eval
            .lock()
            .map_err(|_| "failed to lock accumulator eval context".to_string())?
            .take()
            .ok_or_else(|| "accumulator did not produce an evaluation context".to_string())?;
        Ok(AccumulatorEvalOutput {
            resolved: output.resolved,
            suppressed: output.suppressed,
            effect_params: output.effect_params,
            instrument_params: output.instrument_params,
            emitted: output.emitted,
        })
    }

    pub fn invoke_midi_fx(
        &mut self,
        registry_index: usize,
        track: usize,
        step: usize,
        value: f32,
        resolved: ResolvedStep,
        chord: Vec<f32>,
        chord_durations: Vec<f32>,
        chord_step_transpose: f32,
        note_spans: Option<Vec<AccumulatorNoteSpan>>,
        midi_fx_slot: EffectSlotSnapshot,
        step_beats: f32,
        num_steps: usize,
        effect_slots: Vec<EffectSlotSnapshot>,
        instrument_slot: EffectSlotSnapshot,
        effect_params: Vec<ScheduledEffectParam>,
        instrument_params: Vec<ScheduledInstrumentParam>,
    ) -> Result<AccumulatorEvalOutput, String> {
        self.invoke_midi_fx_with_arp_phase_beats(
            registry_index,
            track,
            step,
            value,
            resolved,
            chord,
            chord_durations,
            chord_step_transpose,
            note_spans,
            midi_fx_slot,
            0.0,
            step_beats,
            num_steps,
            effect_slots,
            instrument_slot,
            effect_params,
            instrument_params,
        )
    }

    pub fn invoke_midi_fx_with_arp_phase_beats(
        &mut self,
        registry_index: usize,
        track: usize,
        step: usize,
        value: f32,
        resolved: ResolvedStep,
        chord: Vec<f32>,
        chord_durations: Vec<f32>,
        chord_step_transpose: f32,
        note_spans: Option<Vec<AccumulatorNoteSpan>>,
        midi_fx_slot: EffectSlotSnapshot,
        arp_phase_beats: f32,
        step_beats: f32,
        num_steps: usize,
        effect_slots: Vec<EffectSlotSnapshot>,
        instrument_slot: EffectSlotSnapshot,
        effect_params: Vec<ScheduledEffectParam>,
        instrument_params: Vec<ScheduledInstrumentParam>,
    ) -> Result<AccumulatorEvalOutput, String> {
        let entry = self
            .midi_fx
            .lock()
            .map_err(|_| "failed to lock MIDI FX registry".to_string())?
            .get(registry_index)
            .cloned()
            .ok_or_else(|| "registered MIDI FX out of range".to_string())?;
        let midi_fx_slot = if midi_fx_slot.num_params == 0 && !entry.params.is_empty() {
            EffectSlotSnapshot::new_default(
                &EffectDescriptor {
                    name: entry.name.clone(),
                    params: entry.params.clone(),
                    input_channels: 0,
                    output_channels: 0,
                    instrument_modulators: Vec::new(),
                    instrument_modulation_targets: Vec::new(),
                    tensor_params: Vec::new(),
                },
                0,
            )
        } else {
            midi_fx_slot
        };
        {
            let mut eval_ctx = self
                .accumulator_eval
                .lock()
                .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
            *eval_ctx = Some(AccumulatorEvalContext {
                step_index: step,
                resolved,
                chord,
                chord_durations,
                chord_step_transpose,
                note_spans,
                midi_fx_scope: Some((track, entry.name.clone())),
                midi_fx_slot,
                midi_fx_param_names: entry
                    .params
                    .iter()
                    .map(|param| param.name.clone())
                    .collect(),
                arp_phase_beats,
                step_beats,
                num_steps,
                suppressed: false,
                effect_slots,
                instrument_slot,
                effect_params,
                instrument_params,
                emitted: Vec::new(),
            });
        }
        self.runtime
            .set_global_value("fx-step", EValue::Number(step as f64));
        self.runtime
            .set_global_value("fx-value", EValue::Number(value as f64));
        match entry.callback {
            RegisteredAccumulatorCallback::Source(source) => {
                self.runtime
                    .eval_str(&source)
                    .map_err(|e| format!("{e:?}"))?;
            }
            RegisteredAccumulatorCallback::Closure(callback) => {
                self.runtime
                    .invoke(
                        callback,
                        vec![EValue::Number(step as f64), EValue::Number(value as f64)],
                    )
                    .map_err(|e| format!("{e:?}"))?;
            }
        }
        let output = self
            .accumulator_eval
            .lock()
            .map_err(|_| "failed to lock MIDI FX eval context".to_string())?
            .take()
            .ok_or_else(|| "MIDI FX did not produce an evaluation context".to_string())?;
        Ok(AccumulatorEvalOutput {
            resolved: output.resolved,
            suppressed: output.suppressed,
            effect_params: output.effect_params,
            instrument_params: output.instrument_params,
            emitted: output.emitted,
        })
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Runtime,
        SharedSequencerEvalContext,
        SharedSequencerNativeMetadata,
        SharedRegisteredAccumulators,
        SharedRegisteredMidiFx,
        SharedPendingMidiFxParams,
        SharedMidiFxState,
        SharedAccumulatorEvalContext,
        SharedRegisteredSequencers,
        SharedGeneratorTickContext,
        SharedGeneratorChannels,
        SharedSceneSlotSnapshot,
        SharedProcessAuthoring,
        SharedProcessEvalContext,
        SharedGraphNodeContext,
    ) {
        (
            self.runtime,
            self.context,
            self.metadata,
            self.accumulators,
            self.midi_fx,
            self.pending_midi_fx_params,
            self.midi_fx_state,
            self.accumulator_eval,
            self.sequencers,
            self.generator_tick,
            self.generator_channels,
            self.scene_slots,
            self.process_authoring,
            self.process_eval,
            self.graph_node,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        runtime: Runtime,
        context: SharedSequencerEvalContext,
        metadata: SharedSequencerNativeMetadata,
        accumulators: SharedRegisteredAccumulators,
        midi_fx: SharedRegisteredMidiFx,
        pending_midi_fx_params: SharedPendingMidiFxParams,
        midi_fx_state: SharedMidiFxState,
        accumulator_eval: SharedAccumulatorEvalContext,
        sequencers: SharedRegisteredSequencers,
        generator_tick: SharedGeneratorTickContext,
        generator_channels: SharedGeneratorChannels,
        scene_slots: SharedSceneSlotSnapshot,
        process_authoring: SharedProcessAuthoring,
        process_eval: SharedProcessEvalContext,
        graph_node: SharedGraphNodeContext,
    ) -> Self {
        let mut this = Self {
            runtime,
            context,
            metadata,
            accumulators,
            midi_fx,
            pending_midi_fx_params,
            midi_fx_state,
            accumulator_eval,
            sequencers,
            generator_tick,
            generator_channels,
            scene_slots,
            process_authoring,
            process_eval,
            graph_node,
            graph_updates: HashMap::new(),
            sequencer_tick_callbacks: HashMap::new(),
            process_run_callbacks: HashMap::new(),
            #[cfg(test)]
            sequencer_tick_compile_count: 0,
            #[cfg(test)]
            process_run_cache_enabled: true,
            runtime_globals: Vec::new(),
        };
        this.install_accumulator_macro();
        this.install_midi_fx_macro();
        this.refresh_runtime_globals();
        this
    }

    /// Register a generator whose `:tick` is shipped source (from a UI-runtime
    /// `def-sequencer` published via `SequencerState`). Upserts by id so re-evaluating
    /// the authoring file hot-reloads the body without duplicating the generator.
    pub fn register_published_sequencer(
        &mut self,
        id: u64,
        name: String,
        resolution: Timebase,
        tick_source: String,
        requires: &[String],
    ) -> Result<(), String> {
        // Import the tick's declared modules (`:requires`) before compiling.
        // This runtime carries the full package module load path, so shipped
        // ticks that call package functions resolve without the authoring
        // file's source ever crossing the VM boundary. `__import-module`
        // reports failure through its return value (and load-once dedups
        // repeat registrations), so a bad module fails registration loudly
        // instead of surfacing as per-tick UnknownVariable errors.
        for module in requires {
            if !eseqlisp::modules::is_valid_module_name(module) {
                return Err(format!(
                    "sequencer {name} ({id}): invalid :requires module name '{module}'"
                ));
            }
            let result = self.runtime.eval_str(&format!("(import {module})"));
            // `eval_str` does not consume load errors itself; drain them so a
            // failure is reported here and can never poison a later
            // path-based eval on this runtime.
            let load_errors = self.runtime.take_source_load_errors();
            match result {
                Ok(_) if load_errors.is_empty() => {}
                Ok(_) => {
                    return Err(format!(
                        "sequencer {name} ({id}): failed to import required module: {}",
                        load_errors.join("; ")
                    ));
                }
                Err(error) => {
                    return Err(format!(
                        "sequencer {name} ({id}): failed to import required module {module}: {error:?}"
                    ));
                }
            }
        }
        if let Err(error) = self.sequencer_tick_callback(id, &tick_source) {
            self.sequencers
                .lock()
                .map_err(|_| "failed to lock sequencer registry".to_string())?
                .retain(|entry| entry.id != id);
            return Err(error);
        }

        let entry = RegisteredSequencer {
            id,
            name,
            resolution,
            tick: RegisteredAccumulatorCallback::Source(tick_source),
        };
        let mut registry = self
            .sequencers
            .lock()
            .map_err(|_| "failed to lock sequencer registry".to_string())?;
        if let Some(existing) = registry.iter_mut().find(|entry| entry.id == id) {
            *existing = entry;
        } else {
            registry.push(entry);
        }
        Ok(())
    }

    fn sequencer_tick_callback(&mut self, id: u64, source: &str) -> Result<EValue, String> {
        if let Some(compiled) = self.sequencer_tick_callbacks.get(&id) {
            if compiled.source == source {
                return compiled.callback.clone();
            }
        }

        // Remove the old VM-owned value before attempting a replacement. A failed
        // hot reload must never leave the previous body callable under this id.
        self.sequencer_tick_callbacks.remove(&id);
        #[cfg(test)]
        {
            self.sequencer_tick_compile_count += 1;
        }
        let wrapped = format!("(lambda () {source})");
        let callback = self
            .runtime
            .eval_str(&wrapped)
            .map_err(|error| {
                format!("failed to compile sequencer tick {id}: {error:?}; source={source}")
            })
            .and_then(|value| {
                value.ok_or_else(|| {
                    format!("sequencer tick {id} compilation produced no callback")
                })
            })
            .and_then(|value| match value {
                EValue::Closure(_, _) | EValue::NativeFunction(_) => Ok(value),
                other => Err(format!(
                    "sequencer tick {id} must compile to a callable, got {}",
                    eseqlisp::vm::format_lisp_value(&other)
                )),
            });
        self.sequencer_tick_callbacks.insert(
            id,
            CompiledSequencerTick {
                source: source.to_string(),
                callback: callback.clone(),
            },
        );
        callback
    }

    /// Definitions of all generators registered in this VM, for the scheduler to
    /// reconcile into its [`crate::generator::GeneratorRuntime`].
    pub fn sequencer_defs(&self) -> Vec<crate::generator::GeneratorDef> {
        self.sequencers
            .lock()
            .map(|registry| {
                registry
                    .iter()
                    .map(|entry| crate::generator::GeneratorDef {
                        id: entry.id,
                        name: entry.name.clone(),
                        resolution_beats: entry
                            .resolution
                            .step_beats(crate::generator::GENERATOR_RESOLUTION_REF_STEPS),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Invoke a registered generator's `:tick` closure for one boundary crossing,
    /// returning the events it emitted plus the advanced RNG state. Mirrors
    /// [`Self::invoke_accumulator`] but for self-clocked generators.
    pub fn invoke_sequencer_tick(
        &mut self,
        registry_index: usize,
        input: crate::generator::GeneratorTickInput,
    ) -> Result<crate::generator::GeneratorTickResult, String> {
        let (id, callback) = self
            .sequencers
            .lock()
            .map_err(|_| "failed to lock sequencer registry".to_string())?
            .get(registry_index)
            .map(|entry| (entry.id, entry.tick.clone()))
            .ok_or_else(|| "registered sequencer out of range".to_string())?;
        let callback = match callback {
            RegisteredAccumulatorCallback::Source(source) => {
                self.sequencer_tick_callback(id, &source)?
            }
            RegisteredAccumulatorCallback::Closure(callback) => callback,
        };
        {
            let mut ctx = self
                .generator_tick
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            *ctx = Some(GeneratorTickContext {
                tick_index: input.tick_index,
                beat: input.beat,
                resolution_beats: input.resolution_beats,
                random_state: input.random_state,
                state: input.state,
                emitted: Vec::new(),
                controls: Vec::new(),
            });
        }
        let invocation = self
            .runtime
            .invoke(callback, vec![])
            .map_err(|error| format!("{error:?}"));
        let ctx = self
            .generator_tick
            .lock()
            .map_err(|_| "failed to lock generator tick context".to_string())?
            .take()
            .ok_or_else(|| "generator tick did not produce a context".to_string())?;
        invocation?;
        Ok(crate::generator::GeneratorTickResult {
            emitted: ctx.emitted,
            controls: ctx.controls,
            random_state: ctx.random_state,
            state: ctx.state,
        })
    }

    pub fn process_authoring_snapshot(&self) -> crate::process::ProcessAuthoringSnapshot {
        self.process_authoring
            .lock()
            .map(|registry| registry.snapshot())
            .unwrap_or_default()
    }

    pub fn invoke_process_run(
        &mut self,
        invocation: crate::process::ProcessRunInvocation,
    ) -> Result<crate::process::ProcessRunResult, String> {
        let conductor_observe_tracks = invocation.reads.conductor_observe_tracks.clone();
        let conductor_play_tracks = invocation.reads.conductor_play_tracks.clone();
        {
            let mut ctx = self
                .process_eval
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            *ctx = Some(ProcessEvalContext {
                runtime_id: invocation.runtime_id,
                beat: invocation.beat,
                inlets: invocation.inlets,
                state: invocation.state,
                event: invocation.event,
                step_context: invocation.step_context,
                ports: invocation.ports,
                reads: invocation.reads,
                conductor_observe_tracks,
                conductor_play_tracks,
                outputs: Vec::new(),
                emissions: Vec::new(),
                commands: Vec::new(),
                target_writes: Vec::new(),
                transpose: None,
                random_state: invocation.seed,
                scope: ProcessEvalScope::Run,
            });
        }
        let _ = self.runtime.take_status_message();
        let execution_result = (|| -> Result<(), String> {
            if self.process_run_cache_enabled() {
                let callback =
                    if let Some(callback) = self.process_run_callbacks.get(&invocation.source) {
                        callback.clone()
                    } else {
                        let callback_source = format!("(lambda () {})", invocation.source);
                        let callback = self
                            .runtime
                            .eval_str(&callback_source)
                            .map_err(|error| format!("{error:?}"))?
                            .ok_or_else(|| {
                                "process body did not compile to a callback".to_string()
                            })?;
                        self.process_run_callbacks
                            .insert(invocation.source.clone(), callback.clone());
                        callback
                    };
                self.runtime
                    .invoke(callback, Vec::new())
                    .map_err(|error| format!("{error:?}"))?;
            } else {
                self.runtime
                    .eval_str(&invocation.source)
                    .map_err(|error| format!("{error:?}"))?;
            }
            Ok(())
        })();
        if let Err(error) = execution_result {
            if let Ok(mut ctx) = self.process_eval.lock() {
                ctx.take();
            }
            return Err(error);
        }
        let process_status = self.runtime.take_status_message();
        let ctx = self
            .process_eval
            .lock()
            .map_err(|_| "failed to lock process eval context".to_string())?
            .take()
            .ok_or_else(|| "process run did not produce a context".to_string())?;
        if let Some(status) = process_status {
            if status.starts_with("Error:") {
                return Err(status);
            }
        }
        Ok(crate::process::ProcessRunResult {
            runtime_id: invocation.runtime_id,
            beat: invocation.beat,
            sample_time: invocation.sample_time,
            state: ctx.state,
            outputs: ctx.outputs,
            emissions: ctx.emissions,
            commands: ctx.commands,
            target_writes: ctx.target_writes,
            transpose: ctx.transpose,
        })
    }

    #[cfg(test)]
    pub(crate) fn set_process_run_cache_enabled(&mut self, enabled: bool) {
        self.process_run_cache_enabled = enabled;
    }

    #[inline]
    pub(super) fn process_run_cache_enabled(&self) -> bool {
        #[cfg(test)]
        {
            self.process_run_cache_enabled
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    pub fn invoke_process_ratchet_shape(
        &mut self,
        shape_context: &mut crate::process::ProcessRatchetShapeContext,
        shape: &EValue,
        index: u32,
        event: crate::process::ProcessRatchetEvent,
    ) -> Result<crate::process::ProcessRatchetEvent, String> {
        let event_value = process_ratchet_event_value(event);
        {
            let mut ctx = self
                .process_eval
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            *ctx = Some(ProcessEvalContext {
                runtime_id: shape_context.runtime_id,
                beat: shape_context.beat,
                inlets: shape_context.inlets.clone(),
                state: shape_context.state.clone(),
                event: shape_context.event.clone(),
                step_context: Some(shape_context.step_context.clone()),
                ports: shape_context.ports.clone(),
                reads: crate::process::ProcessReadSnapshot::default(),
                conductor_observe_tracks: Vec::new(),
                conductor_play_tracks: Vec::new(),
                outputs: Vec::new(),
                emissions: Vec::new(),
                commands: Vec::new(),
                target_writes: Vec::new(),
                transpose: None,
                random_state: shape_context.random_state,
                scope: ProcessEvalScope::RatchetShape,
            });
        }
        let _ = self.runtime.take_status_message();
        let invoke_result = self.runtime.invoke(
            shape.clone(),
            vec![EValue::Number(index as f64), event_value.clone()],
        );
        let shape_status = self.runtime.take_status_message();
        let ctx = self
            .process_eval
            .lock()
            .map_err(|_| "failed to lock process eval context".to_string())?
            .take()
            .ok_or_else(|| "ratchet shape did not produce an evaluation context".to_string())?;
        shape_context.random_state = ctx.random_state;
        if let Some(status) = shape_status {
            if status.starts_with("Error:") {
                return Err(status);
            }
        }
        let returned = invoke_result.map_err(|error| format!("{error:?}"))?;
        let shaped_value = match returned {
            Some(value @ EValue::Map(_)) => value,
            _ => event_value,
        };
        process_ratchet_event_from_value(&shaped_value)
    }
}
