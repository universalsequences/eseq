use super::super::*;

impl SequencerState {
    pub fn capture_current_pattern_snapshot(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> PatternSnapshot {
        let (mod_connections, neural_networks, graph_overrides) = self.current_scene_metadata();
        let mut snapshot = PatternSnapshot::capture_with_mod_connections(
            self,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            mod_connections,
            neural_networks,
            graph_overrides,
        );
        snapshot.project_process_chain = self.project_process_chain();
        let effect_descriptors = self.scratch_effect_descriptors.lock().unwrap().clone();
        let instrument_descriptors = self.scratch_instrument_descriptors.lock().unwrap().clone();
        snapshot.refresh_process_binding_param_ids(&effect_descriptors, &instrument_descriptors);
        snapshot
    }

    pub fn new(num_tracks: usize, initial_chains: Vec<Vec<EffectSlotState>>) -> Self {
        // Initialize the shared monotonic origin off the audio thread.
        let _ = RECORD_CLOCK_ORIGIN.get_or_init(Instant::now);
        let patterns: Vec<TrackPattern> = (0..MAX_TRACKS).map(|_| TrackPattern::new()).collect();
        let neural_reset_patterns: Vec<TrackPattern> =
            (0..MAX_TRACKS).map(|_| TrackPattern::new()).collect();
        let scene_silenced: Vec<AtomicBool> =
            (0..MAX_TRACKS).map(|_| AtomicBool::new(false)).collect();
        let step_data: Vec<StepData> = (0..MAX_TRACKS).map(|_| StepData::new()).collect();
        let track_params: Vec<TrackParams> = (0..MAX_TRACKS).map(|_| TrackParams::new()).collect();
        let trigger_flash: Vec<AtomicU32> = (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect();

        let mut effect_chains = initial_chains;
        for _ in effect_chains.len()..MAX_TRACKS {
            effect_chains.push(default_empty_effect_chain());
        }
        let midi_fx_slots = (0..MAX_TRACKS)
            .map(|_| {
                (0..crate::lisp_host::MAX_MIDI_FX_SLOTS)
                    .map(|_| EffectSlotState::empty())
                    .collect()
            })
            .collect();

        let slot_descriptors: Vec<Vec<EffectDescriptor>> = (0..num_tracks)
            .map(|_| EffectDescriptor::default_full_chain())
            .collect();

        let chord_data: Vec<ChordData> = (0..MAX_TRACKS).map(|_| ChordData::new()).collect();

        let state = Self {
            pattern: PatternState {
                patterns,
                neural_reset_patterns,
                scene_silenced,
                step_data,
                chord_data,
                track_params,
                effect_chains,
                midi_fx_slots,
                scenes: Mutex::new(ProjectScenes::from_pattern_snapshots(
                    &[PatternSnapshot::new_default(num_tracks, &slot_descriptors)],
                    0,
                )),
                song: Mutex::new(None),
                song_revision: AtomicU64::new(0),
                current_pattern: AtomicU32::new(0),
                num_patterns: AtomicU32::new(1),
                timebase_plocks: (0..MAX_TRACKS).map(|_| TimebasePLockData::new()).collect(),
                swing_plocks: (0..MAX_TRACKS).map(|_| SwingPLockData::new()).collect(),
                swing_resolution_plocks: (0..MAX_TRACKS)
                    .map(|_| SwingResolutionPLockData::new())
                    .collect(),
                instrument_slots: (0..MAX_TRACKS).map(|_| EffectSlotState::empty()).collect(),
                instrument_base_note_offsets: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                instrument_run_modes: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(CustomInstrumentRunMode::Instrument.runtime_flag()))
                    .collect(),
                track_sound_state: Mutex::new(
                    (0..MAX_TRACKS)
                        .map(|_| TrackSoundState::default())
                        .collect(),
                ),
                rack_tracks: Mutex::new((0..MAX_TRACKS).map(|_| None).collect()),
                process_chains: Mutex::new(
                    (0..MAX_TRACKS)
                        .map(|_| crate::process::TrackProcessChain::default())
                        .collect(),
                ),
                project_process_lane_overrides: Mutex::new(
                    (0..MAX_TRACKS).map(|_| Default::default()).collect(),
                ),
                plock_variant_registries: Mutex::new(
                    (0..MAX_TRACKS)
                        .map(|_| PlockVariantRegistry::default())
                        .collect(),
                ),
                key_lock_variant_registries: Mutex::new(
                    (0..MAX_TRACKS)
                        .map(|_| PlockVariantRegistry::default())
                        .collect(),
                ),
            },
            transport: TransportState {
                playhead: AtomicU32::new(0),
                playing: AtomicBool::new(false),
                bpm: AtomicU32::new(DEFAULT_BPM),
                master_volume: AtomicU32::new(1.0_f32.to_bits()),
                pattern_epoch: AtomicU64::new(0),
                topology_epoch: AtomicU64::new(0),
                topology_edit_kind: AtomicU32::new(TOPOLOGY_EDIT_NONE),
                topology_edit_track: AtomicU32::new(u32::MAX),
                topology_edit_request_id: AtomicU64::new(0),
                topology_edit_ready_id: AtomicU64::new(0),
                topology_edit_applied_id: AtomicU64::new(0),
                mod_reset_counter: AtomicU32::new(0),
                pending_mod_resync: AtomicBool::new(false),
                peak_l: AtomicU32::new(0.0_f32.to_bits()),
                peak_r: AtomicU32::new(0.0_f32.to_bits()),
                cpu_load_pct: AtomicU32::new(0.0_f32.to_bits()),
                trigger_flash,
                num_tracks: AtomicU32::new(num_tracks as u32),
                track_playheads: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                track_playhead_phases: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                sampler_playheads: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                active_voice_counts: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                playhead_phase: AtomicU32::new(0.0_f32.to_bits()),
                record_quantize: AtomicU32::new(
                    crate::record_quantize::RecordQuantize::DEFAULT as u32,
                ),
                record_latency_seconds: AtomicU32::new(0.0_f32.to_bits()),
                record_clock: RecordClockAnchor::new(),
                metronome_enabled: AtomicBool::new(false),
                record_quantize_thresh: AtomicU32::new(0.5_f32.to_bits()),
            },
            runtime: RuntimeBindingState {
                sampler_lids: (0..MAX_SAMPLER_POOLS).map(|_| AtomicU64::new(0)).collect(),
                modulator_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                pan_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                delay_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                send_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                rack_slot_pan_lids: (0..MAX_TRACKS)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                voice_lids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                voice_counts: (0..MAX_SAMPLER_POOLS).map(|_| AtomicU32::new(0)).collect(),
                instrument_type_flags: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                instrument_run_mode_flags: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(CustomInstrumentRunMode::Instrument.runtime_flag()))
                    .collect(),
                synth_node_ids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                sampler_gatepitch_node_ids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                sampler_modulator_node_ids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                track_engine_ids: (0..MAX_TRACKS).map(|_| AtomicU32::new(u32::MAX)).collect(),
                engine_voice_lids: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                engine_synth_node_ids: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                engine_modulator_node_ids: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                engine_voice_counts: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| AtomicU32::new(0))
                    .collect(),
                engine_route_lids: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))))
                    .collect(),
                engine_route_lids_r: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))))
                    .collect(),
                engine_ext_route_lids: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| {
                        std::array::from_fn(|_| {
                            std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                        })
                    })
                    .collect(),
                rack_engine_route_lids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                rack_engine_route_lids_r: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                rack_engine_route_engine_ids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| AtomicU32::new(u32::MAX))
                    .collect(),
                rack_engine_ext_route_lids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))))
                    .collect(),
                sampler_analysis_buffer_ids: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(u32::MAX))
                    .collect(),
                sampler_analysis_bpm: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                sampler_onset_ptr_lo: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                sampler_onset_ptr_hi: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                sampler_analysis_status: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
            },
            scheduler_snapshot: Mutex::new(Arc::new(SequencerSnapshot::empty())),
            scheduler_snapshot_version: AtomicU64::new(0),
            live_macro_overrides: Mutex::new(HashMap::new()),
            rack_macro_runtime_values: Arc::new(RackMacroRuntimeValues::new()),
            neural_visualization: Mutex::new(NeuralVisualizationSnapshot::default()),
            graph_visualizations: Mutex::new(Vec::new()),
            track_output_events: Mutex::new(Vec::new()),
            track_output_current_beat_bits: AtomicU64::new(0.0_f64.to_bits()),
            active_note_until_samples: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                .collect(),
            live_note_masks: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                .collect(),
            audio_rendered_sample: AtomicU64::new(0),
            scratch_source: Mutex::new(String::new()),
            scratch_source_version: AtomicU64::new(0),
            published_sequencers: Mutex::new(Vec::new()),
            published_sequencers_version: AtomicU64::new(0),
            published_process_authoring: Mutex::new(
                crate::process::PublishedProcessAuthoringSnapshot::default(),
            ),
            published_process_authoring_version: AtomicU64::new(0),
            scratch_effect_descriptors: Mutex::new(Vec::new()),
            scratch_instrument_descriptors: Mutex::new(Vec::new()),
            process_trace_enabled: AtomicBool::new(
                std::env::var("ESEQ_PROCESS_TRACE").is_ok_and(|value| value == "1"),
            ),
            pending_accumulator_reset_all: AtomicBool::new(false),
            pending_accumulator_reset_tracks: std::array::from_fn(|_| AtomicBool::new(false)),
            quantized_launches: crate::quantized_launch::QuantizedLaunchMailbox::default(),
            song_playback: SongPlaybackMailbox::default(),
        };
        state.publish_scheduler_snapshot();
        state
    }

    pub fn active_track_count(&self) -> usize {
        self.transport.num_tracks.load(Ordering::Acquire) as usize
    }

    pub fn quantized_launches(&self) -> &crate::quantized_launch::QuantizedLaunchMailbox {
        &self.quantized_launches
    }

    pub fn schedule_quantized_pattern_launch(
        &self,
        target: crate::quantized_launch::PatternLaunchTarget,
        quantize: crate::quantized_launch::LaunchQuantize,
        owner: crate::quantized_launch::QuantizedLaunchOwner,
    ) -> Result<
        crate::quantized_launch::QuantizedLaunchToken,
        crate::quantized_launch::QuantizedLaunchSubmitError,
    > {
        self.quantized_launches.schedule(
            target,
            quantize,
            owner,
            self.scene_count(),
            self.active_track_count(),
        )
    }

    pub fn is_scene_silenced(&self, track: usize) -> bool {
        self.pattern
            .scene_silenced
            .get(track)
            .map(|flag| flag.load(Ordering::Acquire))
            .unwrap_or(false)
    }

    pub(super) fn set_scene_silenced(&self, track: usize, silenced: bool) {
        if let Some(flag) = self.pattern.scene_silenced.get(track) {
            flag.store(silenced, Ordering::Release);
        }
    }
    pub fn scheduler_snapshot_version(&self) -> u64 {
        self.scheduler_snapshot_version.load(Ordering::Acquire)
    }
    pub fn current_pattern_index(&self) -> usize {
        self.pattern.current_pattern.load(Ordering::Relaxed) as usize
    }

    pub fn current_scene_index(&self) -> usize {
        self.current_pattern_index()
    }

    pub(crate) fn current_scene_id(&self) -> Option<SceneId> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scene_id(self.current_scene_index())
    }

    pub(crate) fn scene_index(&self, id: SceneId) -> Option<usize> {
        self.pattern.scenes.lock().unwrap().scene_index(id)
    }

    pub(crate) fn effective_track_pattern_id(&self, track: usize) -> Option<PatternId> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .effective_pattern_id(track)
    }

    pub(crate) fn live_track_params_snapshot(&self, track: usize) -> Option<TrackParamsSnapshot> {
        self.pattern
            .track_params
            .get(track)
            .map(capture_track_params_snapshot)
    }

    pub(crate) fn live_rack_track_snapshot(&self, track: usize) -> Option<RackTrackSnapshot> {
        self.pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .flatten()
    }

    pub fn scene_count(&self) -> usize {
        self.pattern.scenes.lock().unwrap().scene_count()
    }

    /// Reads one scene track in place without cloning the complete pattern.
    pub fn with_scene_track_pattern<R>(
        &self,
        scene: usize,
        track: usize,
        read: impl FnOnce(&TrackPatternData) -> R,
    ) -> Option<R> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let scene = scenes.scenes.get(scene)?;
        let pattern_id = scene.cells.get(track).copied().flatten()?;
        let pattern = scenes.track_pools.get(track)?.get(pattern_id)?;
        Some(read(pattern))
    }

    pub fn pattern_repository_len(&self) -> usize {
        self.scene_count()
    }

    pub fn export_pattern_repository(&self) -> Vec<PatternSnapshot> {
        self.pattern.scenes.lock().unwrap().snapshots()
    }

    pub fn replace_pattern_repository(&self, snapshots: Vec<PatternSnapshot>, current_idx: usize) {
        let _ = self.quantized_launches.cancel_all();
        let len = snapshots.len().max(1);
        {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let bus_patterns = scenes
                .scenes
                .iter()
                .map(|scene| scene.bus_patterns.clone())
                .collect::<Vec<_>>();
            let mut rebuilt = ProjectScenes::from_pattern_snapshots(&snapshots, current_idx);
            for (scene, bus_patterns) in rebuilt.scenes.iter_mut().zip(bus_patterns) {
                scene.bus_patterns = bus_patterns;
            }
            *scenes = rebuilt;
        }
        self.pattern
            .num_patterns
            .store(len as u32, Ordering::Relaxed);
        self.pattern.current_pattern.store(
            current_idx.min(len.saturating_sub(1)) as u32,
            Ordering::Relaxed,
        );
    }

    pub fn current_pattern_sample_ids(&self) -> Vec<(i32, String, u32)> {
        let current_pattern = self.current_pattern_index();
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scene_snapshot(current_pattern)
            .map(|snapshot| snapshot.sample_ids.clone())
            .unwrap_or_default()
    }

    pub fn effective_pattern_sample_ids(&self, track_count: usize) -> Vec<(i32, String, u32)> {
        let scenes = self.pattern.scenes.lock().unwrap();
        (0..track_count)
            .map(|track| {
                scenes
                    .effective_track_pattern(track)
                    .map(|data| data.sample_id.clone())
                    .unwrap_or((-1, String::new(), 44_100))
            })
            .collect()
    }

    #[doc(hidden)]
    pub fn capture_project_scenes(&self) -> ProjectScenes {
        self.pattern.scenes.lock().unwrap().clone()
    }

    /// Clone of the committed song, or `None` when the project has no song.
    pub fn committed_song(&self) -> Option<ProjectSong> {
        self.pattern.song.lock().unwrap().clone()
    }

    /// Replace the committed song wholesale (project load / new project).
    pub fn set_committed_song(&self, song: Option<ProjectSong>) {
        *self.pattern.song.lock().unwrap() = song;
        self.pattern.song_revision.fetch_add(1, Ordering::Release);
    }

    /// Monotonic counter bumped on every committed-song change; per-frame UI
    /// code keys song-derived rebuild work off it (docs/song-mode-spec.md 12).
    pub fn committed_song_revision(&self) -> u64 {
        self.pattern.song_revision.load(Ordering::Acquire)
    }

    /// Edit the committed song in place (topology remaps, future editing
    /// primitives). Callers are responsible for keeping the song valid.
    pub(crate) fn with_committed_song_mut<R>(
        &self,
        f: impl FnOnce(&mut Option<ProjectSong>) -> R,
    ) -> R {
        let result = f(&mut self.pattern.song.lock().unwrap());
        self.pattern.song_revision.fetch_add(1, Ordering::Release);
        result
    }

    #[cfg(test)]
    pub(crate) fn with_scenes_mut<R>(&self, f: impl FnOnce(&mut ProjectScenes) -> R) -> R {
        f(&mut self.pattern.scenes.lock().unwrap())
    }

    pub(crate) fn restore_project_scenes(
        &self,
        target: &ProjectScenes,
    ) -> Result<Vec<(i32, String, u32)>, String> {
        if target.scenes.is_empty() {
            return Err("Scene history cannot restore an empty project".to_string());
        }
        if target.track_pools.len() != self.active_track_count()
            || target.track_overrides.len() != self.active_track_count()
            || target.scenes.iter().any(|scene| scene.cells.len() != self.active_track_count())
        {
            return Err("Scene history track topology no longer matches the project".to_string());
        }
        if target.current_scene >= target.scenes.len() {
            return Err("Scene history has an invalid current-scene index".to_string());
        }
        let unique_scene_ids = target
            .scenes
            .iter()
            .map(|scene| scene.id)
            .collect::<HashSet<_>>();
        if unique_scene_ids.len() != target.scenes.len()
            || unique_scene_ids.iter().any(|id| id.0 == 0)
            || target.next_scene_id == 0
            || unique_scene_ids
                .iter()
                .any(|id| id.0 >= target.next_scene_id)
        {
            return Err("Scene history contains invalid or duplicate scene identities".to_string());
        }
        let _ = self.quantized_launches.cancel_all();
        *self.pattern.scenes.lock().unwrap() = target.clone();
        self.pattern.current_pattern.store(target.current_scene as u32, Ordering::Relaxed);
        self.pattern.num_patterns.store(target.scenes.len() as u32, Ordering::Relaxed);
        let sample_ids = self.restore_current_pattern_from_repository()
            .ok_or_else(|| "Scene history could not restore the current scene".to_string())?;
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.publish_scheduler_snapshot();
        Ok(sample_ids)
    }

    pub fn restore_current_pattern_from_repository(&self) -> Option<Vec<(i32, String, u32)>> {
        let current_pattern = self.current_pattern_index();
        let sample_ids = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let launched = scenes.launch_scene(current_pattern)?;
            for (track, data) in launched.into_iter().enumerate() {
                if let Some(data) = data {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                } else {
                    self.set_scene_silenced(track, true);
                }
            }
            scenes
                .scene_snapshot(current_pattern)
                .map(|snapshot| snapshot.sample_ids)
                .unwrap_or_default()
        };
        Some(sample_ids)
    }

    pub fn save_current_pattern_snapshot(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        let current_pattern = self.current_pattern_index();
        let snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .save_scene_snapshot(current_pattern, snapshot)
    }

    pub fn save_current_track_midi_fx_snapshot(&self, track: usize) -> bool {
        let current_pattern = self.current_pattern_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(mut snapshot) = scenes.scene_snapshot(current_pattern) else {
            return false;
        };
        if track >= self.pattern.track_params.len()
            || track >= self.pattern.midi_fx_slots.len()
            || track >= snapshot.track_params.len()
            || track >= snapshot.midi_fx_slots.len()
        {
            return false;
        }

        snapshot.track_params[track] =
            capture_track_params_snapshot(&self.pattern.track_params[track]);
        snapshot.midi_fx_slots[track] = self.pattern.midi_fx_slots[track]
            .iter()
            .map(EffectSlotSnapshot::capture)
            .collect();
        scenes.save_scene_snapshot(current_pattern, snapshot)
    }

    pub fn save_current_track_effect_snapshot(&self, track: usize) -> bool {
        let current_pattern = self.current_pattern_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(mut snapshot) = scenes.scene_snapshot(current_pattern) else {
            return false;
        };
        if track >= self.pattern.effect_chains.len() || track >= snapshot.effect_slots.len() {
            return false;
        }

        snapshot.effect_slots[track] = self.pattern.effect_chains[track]
            .iter()
            .map(EffectSlotSnapshot::capture)
            .collect();
        let effect_descriptors = self.scratch_effect_descriptors.lock().unwrap().clone();
        let instrument_descriptors = self.scratch_instrument_descriptors.lock().unwrap().clone();
        snapshot.refresh_process_binding_param_ids(&effect_descriptors, &instrument_descriptors);
        scenes.save_scene_snapshot(current_pattern, snapshot)
    }

    pub fn copy_current_effect_values_to_all_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> usize {
        let Some(source_slot) = self
            .pattern
            .effect_chains
            .get(track)
            .and_then(|slots| slots.get(slot_idx))
            .map(EffectSlotSnapshot::capture)
        else {
            return 0;
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        let mut updated = 0;
        for pattern in pool.patterns.values_mut() {
            if let Some(slot) = pattern.effect_slots.get_mut(slot_idx) {
                slot.copy_base_values_from(&source_slot);
                updated += 1;
            }
        }
        updated
    }

    pub fn copy_current_midi_fx_values_to_all_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> usize {
        let Some(source_slot) = self
            .pattern
            .midi_fx_slots
            .get(track)
            .and_then(|slots| slots.get(slot_idx))
            .map(EffectSlotSnapshot::capture)
        else {
            return 0;
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        let mut updated = 0;
        for pattern in pool.patterns.values_mut() {
            if let Some(slot) = pattern.midi_fx_slots.get_mut(slot_idx) {
                slot.copy_base_values_from(&source_slot);
                updated += 1;
            }
        }
        updated
    }

    pub fn copy_current_instrument_values_to_all_track_patterns(&self, track: usize) -> usize {
        let Some(source_slot) = self
            .pattern
            .instrument_slots
            .get(track)
            .map(EffectSlotSnapshot::capture)
        else {
            return 0;
        };
        let Some(source_base_note) = self.pattern.instrument_base_note_offsets.get(track) else {
            return 0;
        };
        let source_base_note = f32::from_bits(source_base_note.load(Ordering::Relaxed));
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        for pattern in pool.patterns.values_mut() {
            pattern.instrument_slot.copy_base_values_from(&source_slot);
            pattern.instrument_base_note_offset = source_base_note;
        }
        pool.patterns.len()
    }

}
