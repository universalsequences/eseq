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
        // A borrowed lane's mirror holds a bound take's/clip's devices, not
        // the scene pattern's (takes spec 16.2). §16.11 released the borrow
        // here (an audible reset + rebind on every capture); under §17 the
        // bound sound already lives in its pool entities — panel edits write
        // them directly — so the capture is instead made TRUTHFUL: the
        // borrowed lane's device half is replaced with the scene-effective
        // entity state, and the borrow (and the monitor) survives the
        // capture untouched. No consumer of this snapshot can leak a bound
        // sound into the scene's entities.
        let borrowed = self.sound_binding_borrowed_mask();
        if borrowed != 0 {
            let scenes = self.pattern.scenes.lock().unwrap();
            for track in 0..num_tracks.min(64) {
                if borrowed >> track & 1 == 0 {
                    continue;
                }
                let effective = scenes.effective_track_pattern(track).or_else(|| {
                    let refs = scenes.effective_sound_refs(track)?;
                    scenes
                        .track_pools
                        .get(track)
                        .and_then(|pool| pool.compose_bare_sound(refs))
                });
                if let Some(data) = effective {
                    snapshot.overwrite_track_device_state(track, &data);
                }
            }
        }
        snapshot.scene_slots = self.current_scene_slots();
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
                arrangement: Mutex::new(None),
                song_revision: AtomicU64::new(0),
                pool_content_revision: AtomicU64::new(0),
                current_pattern: AtomicU32::new(0),
                num_patterns: AtomicU32::new(1),
                timebase_plocks: (0..MAX_TRACKS).map(|_| TimebasePLockData::new()).collect(),
                swing_plocks: (0..MAX_TRACKS).map(|_| SwingPLockData::new()).collect(),
                swing_resolution_plocks: (0..MAX_TRACKS)
                    .map(|_| SwingResolutionPLockData::new())
                    .collect(),
                track_send_plocks: (0..MAX_TRACKS)
                    .map(|_| TrackSendPLockData::new())
                    .collect(),
                track_send_runtime_targets: Mutex::new(vec![Vec::new(); MAX_TRACKS]),
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
                display_modulator_node_ids: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0))
                    .collect(),
                rack_slot_display_modulator_node_ids: (0..MAX_TRACKS)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                playhead_phase: AtomicU32::new(0.0_f32.to_bits()),
                record_quantize: AtomicU32::new(
                    crate::record_quantize::RecordQuantize::DEFAULT as u32,
                ),
                record_latency_seconds: AtomicU32::new(0.0_f32.to_bits()),
                pdc_latency_seconds: AtomicU32::new(0.0_f32.to_bits()),
                record_clock: RecordClockAnchor::new(),
                metronome_enabled: AtomicBool::new(false),
                record_quantize_thresh: AtomicU32::new(0.5_f32.to_bits()),
                roll_mode: AtomicBool::new(false),
                roll_rate: AtomicU32::new(Timebase::Sixteenth as u32),
                sequence_rolling: AtomicBool::new(false),
                roll_window_starts: (0..MAX_TRACKS)
                    .map(|_| AtomicU64::new(f64::NAN.to_bits()))
                    .collect(),
                roll_window_lengths: (0..MAX_TRACKS)
                    .map(|_| AtomicU64::new(0.0_f64.to_bits()))
                    .collect(),
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
                sampler_analysis_buffer_ids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| AtomicU32::new(u32::MAX))
                    .collect(),
                sampler_analysis_bpm: (0..MAX_SAMPLER_POOLS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                sampler_onset_ptr_lo: (0..MAX_SAMPLER_POOLS).map(|_| AtomicU32::new(0)).collect(),
                sampler_onset_ptr_hi: (0..MAX_SAMPLER_POOLS).map(|_| AtomicU32::new(0)).collect(),
                sampler_analysis_status: (0..MAX_SAMPLER_POOLS).map(|_| AtomicU32::new(0)).collect(),
                rack_choke_keys: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
            },
            scheduler_snapshot: Mutex::new(Arc::new(SequencerSnapshot::empty())),
            scheduler_snapshot_version: AtomicU64::new(0),
            snapshot_handoff: SchedulerSnapshotHandoff::new(),
            publish_coalesce_depth: AtomicU64::new(0),
            pending_coalesced_publish: AtomicBool::new(false),
            live_macro_overrides: Mutex::new(HashMap::new()),
            rack_macro_runtime_values: Arc::new(RackMacroRuntimeValues::new()),
            neural_visualization: Mutex::new(NeuralVisualizationSnapshot::default()),
            graph_visualizations: Mutex::new(Vec::new()),
            graph_control_commands: Mutex::new(Vec::new()),
            roll_commands: Mutex::new(Vec::new()),
            roll_recorded_hits: Mutex::new(Vec::new()),
            live_trigger_stamps: crate::sequencer::LiveTriggerStampRing::default(),
            step_print_override: StepPrintOverride::default(),
            device_print_override: DeviceParamPrintOverride::default(),
            rack_macro_print_override: RackMacroPrintOverride::default(),
            track_output_events: Mutex::new(Vec::new()),
            track_output_current_beat_bits: AtomicU64::new(0.0_f64.to_bits()),
            active_note_until_samples: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                .collect(),
            active_note_velocity_bits: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())))
                .collect(),
            live_note_velocity_bits: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())))
                .collect(),
            active_note_trigger_ids: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                .collect(),
            active_note_trigger_sequence: AtomicU64::new(0),
            audio_rendered_sample: AtomicU64::new(0),
            scheduler_rendered_beats_bits: AtomicU64::new(0.0_f64.to_bits()),
            scratch_source: Mutex::new(String::new()),
            scratch_source_version: AtomicU64::new(0),
            published_sequencers: Mutex::new(Vec::new()),
            published_sequencers_version: AtomicU64::new(0),
            published_scene_slot_declarations: Mutex::new(std::collections::BTreeMap::new()),
            generator_tick_errors: Mutex::new(Vec::new()),
            published_process_authoring: Mutex::new(
                crate::process::PublishedProcessAuthoringSnapshot::default(),
            ),
            published_process_authoring_version: AtomicU64::new(0),
            pending_process_channel_writes: Mutex::new(Vec::new()),
            process_channel_values: Mutex::new(HashMap::new()),
            process_channel_values_version: AtomicU64::new(0),
            scratch_effect_descriptors: Mutex::new(Vec::new()),
            scratch_instrument_descriptors: Mutex::new(Vec::new()),
            process_trace_enabled: AtomicBool::new(
                std::env::var("ESEQ_PROCESS_TRACE").is_ok_and(|value| value == "1"),
            ),
            pending_accumulator_reset_all: AtomicBool::new(false),
            pending_accumulator_reset_tracks: std::array::from_fn(|_| AtomicBool::new(false)),
            quantized_launches: crate::quantized_launch::QuantizedLaunchMailbox::default(),
            scheduled_mixer_controls: crate::mixer_control::MixerControlMailbox::default(),
            song_playback: SongPlaybackMailbox::default(),
            song_manual_latch: AtomicU64::new(0),
            song_scene_latch: AtomicBool::new(false),
            song_take_lane_mask: AtomicU64::new(0),
            // Matches `App::arrangement_view_visible`'s initial value; the
            // App re-asserts it on every view switch and on project load.
            arrangement_context: AtomicBool::new(false),
            sound_binding_borrowed: AtomicU64::new(0),
            sound_binding_patterns: Mutex::new(HashMap::new()),
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

    pub fn scheduled_mixer_controls(&self) -> &crate::mixer_control::MixerControlMailbox {
        &self.scheduled_mixer_controls
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
        // Session-mode quantized launches are applied by the scheduler
        // itself (chunk split at the boundary, song-row semantics), which
        // needs the target prebuilt as a snapshot. A failed preflight falls
        // back to the legacy control-side apply so the launch error still
        // surfaces at the boundary.
        let snapshot = if quantize != crate::quantized_launch::LaunchQuantize::Off
            && self.is_playing()
            && !self.song_playback.shared().is_active()
        {
            self.preflight_pattern_launch_snapshot(&target)
        } else {
            None
        };
        self.quantized_launches.schedule(
            target,
            quantize,
            owner,
            self.scene_count(),
            self.active_track_count(),
            snapshot,
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

    pub fn current_scene_id(&self) -> Option<SceneId> {
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

    pub fn set_track_send_runtime_targets(
        &self,
        track: usize,
        targets: Vec<TrackSendRuntimeTarget>,
    ) {
        if let Some(slot) = self.pattern.track_send_runtime_targets.lock().unwrap().get_mut(track) {
            *slot = targets;
        }
    }

    pub fn live_rack_track_snapshot(&self, track: usize) -> Option<RackTrackSnapshot> {
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
        Some(read(&pattern))
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

    /// Install additive scene-bank metadata after rebuilding a project from
    /// its serialized pattern snapshots. Malformed metadata is repaired by
    /// `ProjectScenes` rather than rejecting the project load.
    pub(crate) fn install_project_scene_banks(&self, banks: Vec<SceneBank>) {
        self.pattern.scenes.lock().unwrap().install_scene_banks(banks);
    }

    /// Insert `chunks` into `track`'s pattern pool and register a take over
    /// them (takes spec 6.1). Production facade for scenes mutation (the raw
    /// `with_scenes_mut` seam is test-only).
    ///
    /// `sound: Some(refs)` shares an existing Patch/Mix pair (§17.3 "take
    /// record → share": punch-in passes the bound cell's refs); the chunks'
    /// device halves are dropped. `None` mints a private pair from the first
    /// chunk's device state (legacy imports and callers with no live binding).
    pub(crate) fn register_track_take(
        &self,
        track: usize,
        name: Option<String>,
        chunks: Vec<TrackPatternData>,
        total_len_steps: u32,
        sound: Option<SoundRefs>,
    ) -> Result<TakeId, String> {
        if chunks.is_empty() {
            return Err("a take requires at least one chunk pattern".to_string());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        if track >= scenes.track_pools.len() {
            return Err(format!("track {} does not exist", track + 1));
        }
        let (chunk_ids, take_sound) = {
            let pool = &mut scenes.track_pools[track];
            let shared = sound.filter(|refs| pool.sounds.resolves(*refs));
            debug_assert!(
                sound.is_none() || shared.is_some(),
                "register_track_take on track {} was passed sound refs that do \
                 not resolve — a caller read stale refs; falling back to a \
                 private mint forks the sound",
                track + 1
            );
            let mut chunks = chunks.into_iter();
            let first = chunks.next().expect("checked non-empty above");
            let mut ids = Vec::with_capacity(chunks.len() + 1);
            let sound = match shared {
                Some(refs) => {
                    ids.push(pool.insert_with_refs(first, refs));
                    refs
                }
                None => {
                    let first_id = pool.insert(first);
                    ids.push(first_id);
                    pool.refs(first_id).expect("chunk just inserted")
                }
            };
            for data in chunks {
                ids.push(pool.insert_with_refs(data, sound));
            }
            (ids, sound)
        };
        while scenes.take_pools.len() < scenes.track_pools.len() {
            scenes.take_pools.push(TrackTakePool::default());
        }
        Ok(scenes.take_pools[track].insert(name, chunk_ids, total_len_steps, take_sound))
    }

    /// Resize a take's playable length (takes spec 6.1 invariants preserved).
    /// Growing past the current chunk coverage mints fresh chunks cloned from
    /// an existing one with the step content cleared — cloning keeps the
    /// per-chunk device snapshot in agreement (spec 16.4). Shrinking keeps
    /// chunks that still hold notes (re-growing restores them) but drops
    /// trailing chunks with no active steps, so a grow-then-shrink round trip
    /// leaves the scenes exactly as they were.
    pub(crate) fn resize_track_take(
        &self,
        track: usize,
        take_id: TakeId,
        new_len_steps: u32,
    ) -> Result<(), String> {
        if new_len_steps == 0 {
            return Err("a take must keep a positive length".to_string());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let take = scenes
            .take_pools
            .get(track)
            .and_then(|takes| takes.get(take_id))
            .ok_or_else(|| {
                format!("take {} does not exist on track {}", take_id.0, track + 1)
            })?;
        let needed_chunks = (new_len_steps as usize).div_ceil(MAX_STEPS).max(1);
        let template_id = *take.chunks.first().expect("takes always have a chunk");
        let take_sound = take.sound;
        let existing = take.chunks.len();
        let mut minted = Vec::new();
        if needed_chunks > existing {
            let mut blank = scenes.track_pools[track]
                .get(template_id)
                .ok_or_else(|| {
                    format!("take {}'s template chunk is missing from the pool", take_id.0)
                })?;
            blank.clear_step_content();
            blank.track_params.num_steps = MAX_STEPS;
            for _ in existing..needed_chunks {
                // New chunks share the take's Patch/Mix (§17.2): device
                // agreement is structural rather than kept by cloning.
                minted.push(
                    scenes.track_pools[track].insert_with_refs(blank.clone(), take_sound),
                );
            }
        }
        let doomed: Vec<PatternId> = {
            let take = scenes.take_pools[track].get_mut(take_id).expect("located above");
            take.chunks.extend(minted);
            take.total_len_steps = new_len_steps;
            let mut doomed = Vec::new();
            while take.chunks.len() > needed_chunks {
                doomed.push(*take.chunks.last().expect("longer than needed_chunks >= 1"));
                take.chunks.pop();
            }
            doomed
        };
        // Only content-free chunks may leave, and only from the tail down:
        // chunk order is positional (index i covers steps [i·256, (i+1)·256)),
        // so a surviving chunk pins every chunk below it in place. Chunks that
        // still hold notes stay claimed so re-growing the take restores them.
        let mut kept = Vec::new();
        for chunk in doomed {
            // `doomed` is in pop order: highest chunk index first.
            let empty = scenes.track_pools[track]
                .get(chunk)
                .is_some_and(|data| data.track_bits.iter().all(|word| *word == 0));
            if empty && kept.is_empty() {
                scenes.track_pools[track].remove(chunk);
            } else {
                kept.push(chunk);
            }
        }
        if !kept.is_empty() {
            kept.reverse();
            scenes.take_pools[track]
                .get_mut(take_id)
                .expect("located above")
                .chunks
                .extend(kept);
        }
        Ok(())
    }

    /// Remove a take and delete its chunk patterns from the pattern pool
    /// (takes spec 6.4). The caller owns removing song overrides that
    /// reference the take and committing the combined undo entry.
    pub(crate) fn remove_track_take(&self, track: usize, take_id: TakeId) -> Result<(), String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let take = scenes
            .take_pools
            .get_mut(track)
            .and_then(|takes| takes.remove(take_id))
            .ok_or_else(|| {
                format!("take {} does not exist on track {}", take_id.0, track + 1)
            })?;
        if let Some(pool) = scenes.track_pools.get_mut(track) {
            for chunk in take.chunks {
                pool.remove(chunk);
            }
        }
        Ok(())
    }

    /// Clones of a track's takes for UI listings (takes spec 11.3).
    pub fn track_takes(&self, track: usize) -> Vec<TrackTake> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .take_pools
            .get(track)
            .map(|takes| takes.takes.clone())
            .unwrap_or_default()
    }

    /// Clone of one take.
    pub fn track_take(&self, track: usize, take_id: TakeId) -> Option<TrackTake> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .take_pools
            .get(track)
            .and_then(|takes| takes.get(take_id))
            .cloned()
    }

    /// Load-side arrangement install (takes spec 6.1/11.1), run right after
    /// `replace_pattern_repository`: re-apply per-scene cell absence so bare
    /// lanes survive reload (the pattern bank is dense; the loader
    /// materializes every cell), and rebuild each track's take pool. Chunk
    /// patterns are inserted into the freshly rebuilt pattern pools with new
    /// pool ids; take ids are restored verbatim (song overrides reference
    /// them by stable id).
    pub(crate) fn install_project_arrangement(
        &self,
        scene_cell_presence: &[Vec<bool>],
        take_pools: Vec<(u64, Vec<(u64, String, u32, Vec<TrackPatternData>)>)>,
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        for (scene_idx, mask) in scene_cell_presence.iter().enumerate() {
            for (track, present) in mask.iter().enumerate() {
                if *present {
                    continue;
                }
                let cleared = scenes
                    .scenes
                    .get_mut(scene_idx)
                    .and_then(|scene| scene.cells.get_mut(track))
                    .and_then(|cell| cell.take());
                if let Some(id) = cleared {
                    if let Some(pool) = scenes.track_pools.get_mut(track) {
                        pool.remove(id);
                    }
                }
            }
        }
        for (track, (next_take_id, takes)) in take_pools.into_iter().enumerate() {
            for (id, name, total_len_steps, chunk_data) in takes {
                let Some(pool) = scenes.track_pools.get_mut(track) else {
                    continue;
                };
                // §17.7 migration: collapse the per-chunk device duplicates
                // into one Patch + Mix shared by every chunk and the take.
                // §16.4 guarantees the serialized chunks are identical; a
                // divergence found here is a latent §16-era bug — report it.
                let mut chunk_data = chunk_data.into_iter();
                let Some(first) = chunk_data.next() else {
                    continue;
                };
                let mut chunks = Vec::with_capacity(chunk_data.len() + 1);
                let first_id = pool.insert(first);
                let sound = pool.refs(first_id).expect("chunk just inserted");
                chunks.push(first_id);
                for (idx, data) in chunk_data.enumerate() {
                    let agrees = pool.get(first_id).is_some_and(|first| {
                        crate::sequencer::state::track_pattern_device_state_agrees(
                            &first, &data,
                        )
                    });
                    // Report only — this fires on FILE data (a legacy save
                    // that violated §16.4), so it must never abort a load,
                    // debug builds included. Collapsing to chunk 0's sound
                    // is the documented §17.7 resolution.
                    if !agrees {
                        eprintln!(
                            "[takes-migration] track {} take {}: chunk {} device state \
                             diverges from chunk 0 (takes spec 16.4 violation in the \
                             saved project); collapsing to chunk 0's sound",
                            track + 1,
                            id,
                            idx + 1
                        );
                    }
                    chunks.push(pool.insert_with_refs(data, sound));
                }
                let Some(take_pool) = scenes.take_pools.get_mut(track) else {
                    continue;
                };
                take_pool.takes.push(TrackTake {
                    id: TakeId(id),
                    name,
                    chunks,
                    total_len_steps,
                    sound,
                });
                take_pool.next_take_id = take_pool.next_take_id.max(id.saturating_add(1));
            }
            if let Some(take_pool) = scenes.take_pools.get_mut(track) {
                take_pool.next_take_id = take_pool.next_take_id.max(next_take_id);
            }
        }
        // Re-seed every track sound now that bare cells are re-applied
        // (track-sound spec §2.6): the constructor seeded against the dense
        // bank before absence was known, so a track whose first cells are
        // bare would otherwise carry a default-lane sound instead of its
        // first RESOLVING cell's. Files that serialize the track sound
        // re-link it afterwards in `apply_project_sound_model`; the reseeded
        // private entities become orphans and are pruned.
        for track in 0..scenes.track_pools.len() {
            if let Some(id) = scenes.track_sounds.get(track).copied().flatten() {
                if let Some(pool) = scenes.track_pools.get_mut(track) {
                    pool.remove(id);
                }
            }
            if track < scenes.track_sounds.len() {
                scenes.track_sounds[track] = None;
            }
        }
        scenes.ensure_track_sounds();
    }

    /// The track-sound carrier pattern id (track-sound spec §2.1).
    pub fn track_sound_pattern_id(&self, track: usize) -> Option<PatternId> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .track_sound_pattern(track)
    }

    /// Load the track sound's device state into the live mirror. Only
    /// meaningful on a bare lane (track-sound spec §2.2: the mirror there IS
    /// the track sound) — a repoint of the track sound has no cell restore
    /// to make it audible, so the caller re-loads it explicitly.
    pub fn restore_track_sound_to_mirror(&self, track: usize) -> bool {
        let data = {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(id) = scenes.track_sound_pattern(track) else {
                return false;
            };
            scenes.track_pools.get(track).and_then(|pool| pool.get(id))
        };
        match data {
            Some(data) => data.restore_device_state_to(self, track),
            None => false,
        }
    }

    /// Load the lane's OWNER cell (track override first, then the scene
    /// cell) into the live mirror — the Seq-context counterpart of
    /// `restore_track_sound_to_mirror`, and the same repaint the borrow
    /// release performs for a lane that was on loan.
    ///
    /// A repoint of a cell-owned lane has to run this: the mirror still
    /// holds the OUTGOING patch's device state, and the mirror is what the
    /// next save-back stores into whatever the cell resolves to now — i.e.
    /// straight over the incoming pool Patch (eseq-md9).
    pub fn restore_effective_cell_sound_to_mirror(&self, track: usize) -> bool {
        let data = {
            let scenes = self.pattern.scenes.lock().unwrap();
            scenes.effective_track_pattern(track)
        };
        match data {
            Some(data) => data.restore_device_state_to(self, track),
            None => false,
        }
    }

    /// Entering-arrangement-view half of the view-switch seam (track-sound
    /// spec §2.9 step 2): install the TRACK SOUND into the mirror on every
    /// lane rules 1/2 do not claim. Only the device half moves — note content
    /// on a playing lane is never touched.
    ///
    /// Call AFTER the context flag flips, so `track_owned_lane_mask` names
    /// the lanes the arrangement view now owns.
    pub fn install_track_sounds_into_mirror(&self) {
        self.restore_track_sounds_to_mirror_masked(u64::MAX);
    }

    /// Claim-end half of the §2.8 invariant: reinstall the TRACK SOUND's
    /// device half for the lanes in `mask` that the track owns NOW. Every
    /// path that ends a rule-1/2 claim in arrangement context must put the
    /// owner back into the mirror — the borrow release already does
    /// (`release_borrowed_lanes` falls back to the carrier); this is the
    /// LATCH-release counterpart. Callers pass the latch mask captured
    /// before the clear; the intersection with `track_owned_lane_mask`
    /// drops lanes something still claims (re-borrowed selections) and makes
    /// the whole call a no-op in Seq context, where the cells own the lanes.
    pub fn restore_track_sounds_to_mirror_masked(&self, mask: u64) {
        let mask = mask & self.track_owned_lane_mask();
        if mask == 0 {
            return;
        }
        let restore: Vec<(usize, TrackPatternData)> = {
            let scenes = self.pattern.scenes.lock().unwrap();
            (0..self.pattern.track_params.len().min(64))
                .filter(|track| mask >> track & 1 == 1)
                .filter_map(|track| {
                    let id = scenes.track_sound_pattern(track)?;
                    Some((track, scenes.track_pools.get(track)?.get(id)?))
                })
                .collect()
        };
        for (track, data) in restore {
            data.restore_device_state_to(self, track);
        }
        self.schedule_mod_resync();
        self.publish_scheduler_snapshot();
    }

    /// Apply a project file's serialized sound ref STRUCTURE (takes spec
    /// 18.1 step 5) after pools/cells/takes are installed. Per track, file
    /// entity ids that name the same entity across referents are re-linked
    /// to one live entity: the first referent seen keeps its (freshly
    /// minted) entities as canonical; later referents naming the same file
    /// id repoint to them. Content is identical across such referents by
    /// construction (the save composed it from one entity), so re-linking
    /// never changes what anything sounds like. Referents are processed in
    /// content-priority order — patterns, then takes, then orphan carriers,
    /// then cells — so an id's canonical entity always comes from a referent
    /// whose content the file actually carried (cells carry none). Tuple per
    /// track: `(cells-per-scene, patterns-per-scene, takes, carriers,
    /// patch-meta, mix-meta)` of file-local `(patch, mix)` ids; `u64::MAX`
    /// marks an absent placeholder; carriers additionally hold the composed
    /// content for entities no pattern or chunk serialized (bare cells,
    /// §17.2); meta entries are `(file-id, name, color)` with color `< 0`
    /// meaning name-only (§17.11). Every pool finishes with `ensure_meta`,
    /// so files predating display metadata load with mint-style
    /// auto-assignments.
    #[allow(clippy::type_complexity)]
    pub(crate) fn apply_project_sound_model(
        &self,
        track_sounds: &[(
            Vec<(u64, u64)>,
            Vec<Option<(u64, u64)>>,
            Vec<(u64, u64)>,
            Vec<(u64, u64, TrackPatternData)>,
            Vec<(u64, String, i32)>,
            Vec<(u64, String, i32)>,
            Option<(u64, u64)>,
        )],
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        for (track, (cells, patterns, takes, carriers, patch_meta, mix_meta, track_refs)) in
            track_sounds.iter().enumerate()
        {
            if track >= scenes.track_pools.len() {
                break;
            }
            let mut patch_map: HashMap<u64, PatchId> = HashMap::new();
            let mut mix_map: HashMap<u64, MixId> = HashMap::new();
            // Patterns first: they carry the content, so their minted
            // entities become the canonical ones.
            for (scene_idx, file_refs) in patterns.iter().enumerate() {
                let Some((file_patch, file_mix)) = *file_refs else {
                    continue;
                };
                let Some(pattern_id) = scenes
                    .scenes
                    .get(scene_idx)
                    .and_then(|scene| scene.cells.get(track))
                    .copied()
                    .flatten()
                else {
                    continue;
                };
                let Some(pool) = scenes.track_pools.get_mut(track) else {
                    continue;
                };
                let Some(live) = pool.refs(pattern_id) else {
                    continue;
                };
                let canonical_patch = *patch_map.entry(file_patch).or_insert(live.patch);
                let canonical_mix = *mix_map.entry(file_mix).or_insert(live.mix);
                if canonical_patch != live.patch || canonical_mix != live.mix {
                    if let Some(stored) = pool.patterns.get_mut(&pattern_id).map(Arc::make_mut) {
                        stored.sound = SoundRefs {
                            patch: canonical_patch,
                            mix: canonical_mix,
                        };
                    }
                }
            }
            for (take_idx, (file_patch, file_mix)) in takes.iter().enumerate() {
                let Some(live) = scenes
                    .take_pools
                    .get(track)
                    .and_then(|takes| takes.takes.get(take_idx))
                    .map(|take| take.sound)
                else {
                    continue;
                };
                let canonical_patch = *patch_map.entry(*file_patch).or_insert(live.patch);
                let canonical_mix = *mix_map.entry(*file_mix).or_insert(live.mix);
                let canonical = SoundRefs {
                    patch: canonical_patch,
                    mix: canonical_mix,
                };
                if canonical == live {
                    continue;
                }
                let chunk_ids: Vec<PatternId> = scenes
                    .take_pools
                    .get_mut(track)
                    .and_then(|takes| takes.takes.get_mut(take_idx))
                    .map(|take| {
                        take.sound = canonical;
                        take.chunks.clone()
                    })
                    .unwrap_or_default();
                if let Some(pool) = scenes.track_pools.get_mut(track) {
                    for chunk in chunk_ids {
                        if let Some(stored) = pool.patterns.get_mut(&chunk).map(Arc::make_mut) {
                            stored.sound = canonical;
                        }
                    }
                }
            }
            // Orphan carriers: content for entities no pattern or chunk
            // serialized (bare cells). Seed only the ids still unclaimed —
            // a carrier's other half may duplicate content a pattern
            // already canonicalized, and that copy must lose.
            for (file_patch, file_mix, data) in carriers {
                if *file_patch == u64::MAX || *file_mix == u64::MAX {
                    continue;
                }
                let need_patch = !patch_map.contains_key(file_patch);
                let need_mix = !mix_map.contains_key(file_mix);
                if !need_patch && !need_mix {
                    continue;
                }
                let Some(pool) = scenes.track_pools.get_mut(track) else {
                    continue;
                };
                let (_seq, patch, mix) = data.clone().split();
                if need_patch {
                    patch_map.insert(*file_patch, pool.sounds.insert_patch(patch));
                }
                if need_mix {
                    mix_map.insert(*file_mix, pool.sounds.insert_mix(mix));
                }
            }
            // Cells last: they carry no content of their own, so they only
            // ever adopt entities seeded above — falling back to their live
            // (migration-minted) refs when the file id is unknown.
            for (scene_idx, (file_patch, file_mix)) in cells.iter().enumerate() {
                if *file_patch == u64::MAX || *file_mix == u64::MAX {
                    continue;
                }
                let Some(live) = scenes
                    .scenes
                    .get(scene_idx)
                    .and_then(|scene| scene.cell_sounds.get(track))
                    .copied()
                else {
                    continue;
                };
                let canonical_patch = *patch_map.entry(*file_patch).or_insert(live.patch);
                let canonical_mix = *mix_map.entry(*file_mix).or_insert(live.mix);
                if let Some(cell_sound) = scenes
                    .scenes
                    .get_mut(scene_idx)
                    .and_then(|scene| scene.cell_sounds.get_mut(track))
                {
                    *cell_sound = SoundRefs {
                        patch: canonical_patch,
                        mix: canonical_mix,
                    };
                }
            }
            // The track sound (track-sound spec §2.6): re-link the carrier
            // pattern to the file's entities — shared ids canonicalize onto
            // a referent that carried content (a cell pattern, a take, or an
            // orphan carrier); unknown ids keep the load-time reseed.
            if let Some((file_patch, file_mix)) = track_refs {
                if *file_patch != u64::MAX && *file_mix != u64::MAX {
                    if let Some(live) = scenes
                        .track_sounds
                        .get(track)
                        .copied()
                        .flatten()
                        .and_then(|id| scenes.track_pools.get(track)?.refs(id))
                    {
                        let canonical_patch =
                            *patch_map.entry(*file_patch).or_insert(live.patch);
                        let canonical_mix = *mix_map.entry(*file_mix).or_insert(live.mix);
                        let canonical = SoundRefs {
                            patch: canonical_patch,
                            mix: canonical_mix,
                        };
                        if canonical != live {
                            if let (Some(id), Some(pool)) = (
                                scenes.track_sounds.get(track).copied().flatten(),
                                scenes.track_pools.get_mut(track),
                            ) {
                                if let Some(stored) =
                                    pool.patterns.get_mut(&id).map(Arc::make_mut)
                                {
                                    stored.sound = canonical;
                                }
                            }
                        }
                    }
                }
            }
            // §17.11 display metadata, translated through the file-id →
            // canonical-entity maps built above so names land on the entity
            // every referent actually adopted.
            if let Some(pool) = scenes.track_pools.get_mut(track) {
                for (file_id, name, color) in patch_meta {
                    let Some(live) = patch_map.get(file_id) else {
                        continue;
                    };
                    if pool.sounds.patches.contains_key(live) {
                        pool.sounds.patch_meta.insert(
                            *live,
                            SoundEntityMeta {
                                name: name.clone(),
                                color: u8::try_from(*color).ok(),
                            },
                        );
                    }
                }
                for (file_id, name, color) in mix_meta {
                    let Some(live) = mix_map.get(file_id) else {
                        continue;
                    };
                    if pool.sounds.mixes.contains_key(live) {
                        pool.sounds.mix_meta.insert(
                            *live,
                            SoundEntityMeta {
                                name: name.clone(),
                                color: u8::try_from(*color).ok(),
                            },
                        );
                    }
                }
            }
        }
        // Auto-assign for anything the file didn't cover (older v7 saves,
        // legacy migrations) and drop meta for entities that no longer exist.
        for pool in &mut scenes.track_pools {
            pool.sounds.ensure_meta();
        }
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
                    .effective_pattern_id(track)
                    .and_then(|id| scenes.track_pools.get(track)?.patch(id))
                    .map(|patch| patch.sample_id.clone())
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

    /// Replace the committed song directly, clearing any stored arrangement.
    ///
    /// The stored authoring model is lanes and rows are compiled output
    /// (docs/arrangement-lane-model-spec.md 7), so this is NOT an authoring
    /// path: nothing that edits the project reaches it any more. What is left
    /// is project reset (`start_new_project`, always `None`), the undo replay
    /// of the retired `EditPatch::Song`, and playback tests that install a row
    /// model directly. Installing a song therefore *clears* the arrangement
    /// rather than deriving one: an arrangement left standing beside a song it
    /// did not compile to would break the
    /// `committed_arrangement() == Some(a) implies committed_song() ==
    /// Some(compile(a))` invariant and, on the next save, write lanes that do
    /// not match what plays. Authoring goes through
    /// `set_committed_arrangement`.
    pub fn set_committed_song(&self, song: Option<ProjectSong>) {
        *self.pattern.arrangement.lock().unwrap() = None;
        self.install_committed_song(song);
    }

    /// Install a compiled/committed song without touching the arrangement.
    fn install_committed_song(&self, song: Option<ProjectSong>) {
        *self.pattern.song.lock().unwrap() = song;
        self.pattern.song_revision.fetch_add(1, Ordering::Release);
    }

    /// Clone of the committed arrangement, or `None` when the project has
    /// none (docs/arrangement-lane-model-spec.md 6).
    pub fn committed_arrangement(&self) -> Option<ProjectArrangement> {
        self.pattern.arrangement.lock().unwrap().clone()
    }

    /// Borrow the committed arrangement in place. Per-frame readers (the
    /// piano-roll focus playhead) must use this instead of
    /// `committed_arrangement`, which deep-clones every lane on every call.
    pub fn with_committed_arrangement<R>(
        &self,
        f: impl FnOnce(Option<&ProjectArrangement>) -> R,
    ) -> R {
        f(self.pattern.arrangement.lock().unwrap().as_ref())
    }

    /// Clear the authored arrangement and its compiled song together.
    ///
    /// Project topology teardown must use this before removing tracks. An
    /// arrangement's lanes are indexed by track, so leaving the old lanes
    /// installed while a replacement project's tracks are added makes the
    /// first track registration look like destructive topology drift.
    pub fn clear_committed_arrangement(&self) {
        *self.pattern.arrangement.lock().unwrap() = None;
        self.install_committed_song(None);
    }

    /// Install `arrangement` and its compiled song together (spec 7).
    ///
    /// The arrangement is compiled against the **live** project scenes, which
    /// is the only context that can see scene cells and timebases — compiling
    /// against `SerializedSongContext` silently loses every scene-backdrop
    /// phase override. A compile (or validation) failure installs *nothing*
    /// and returns the error, so the committed song can never disagree with
    /// the committed arrangement: the invariant is
    /// `committed_arrangement() == Some(a)` implies
    /// `committed_song() == Some(compile_arrangement(a, live scenes))`.
    /// `None` clears both.
    pub fn set_committed_arrangement(
        &self,
        arrangement: Option<ProjectArrangement>,
    ) -> Result<(), String> {
        let Some(arrangement) = arrangement else {
            self.clear_committed_arrangement();
            return Ok(());
        };
        // Borrowed, not cloned: `capture_project_scenes` would copy every
        // pattern pool on every arrangement edit.
        let compiled =
            self.with_project_scenes(|scenes| compile_arrangement(&arrangement, scenes))?;
        *self.pattern.arrangement.lock().unwrap() = Some(arrangement);
        self.install_committed_song(Some(compiled));
        Ok(())
    }

    /// Edit the committed arrangement in place (topology remaps). The song is
    /// left alone; callers that change lane content must recompile through
    /// `set_committed_arrangement`. Used by the remaps that already edit the
    /// compiled song in place through `with_committed_song_mut`, so the two
    /// stay in lockstep without a recompile.
    pub(crate) fn with_committed_arrangement_mut<R>(
        &self,
        f: impl FnOnce(&mut Option<ProjectArrangement>) -> R,
    ) -> R {
        f(&mut self.pattern.arrangement.lock().unwrap())
    }

    /// Monotonic counter bumped on every committed-song change; per-frame UI
    /// code keys song-derived rebuild work off it (docs/song-mode-spec.md 12).
    pub fn committed_song_revision(&self) -> u64 {
        self.pattern.song_revision.load(Ordering::Acquire)
    }

    /// Monotonic counter bumped on every pool step/geometry write. UI code
    /// that projects pool CONTENT (the arrangement lane dots) keys its
    /// rebuild off it, so a note edit refreshes the timeline without a
    /// committed-song change and without touching `pattern_epoch`
    /// (docs/realtime-arrangement-feedback-spec.md 5.2).
    pub fn pool_content_revision(&self) -> u64 {
        self.pattern.pool_content_revision.load(Ordering::Acquire)
    }

    pub(crate) fn bump_pool_content_revision(&self) {
        self.pattern
            .pool_content_revision
            .fetch_add(1, Ordering::Release);
    }

    /// Run `f` against the live project scenes without cloning them. Read-only
    /// seam for per-frame UI sync that derives scene-dependent projections
    /// (docs/song-mode-spec.md 5.5); the scenes lock is held only for `f`.
    pub fn with_project_scenes<R>(&self, f: impl FnOnce(&ProjectScenes) -> R) -> R {
        f(&self.pattern.scenes.lock().unwrap())
    }

    /// Read one pool pattern's stored data. `None` when the pattern is gone.
    pub fn with_pool_pattern<R>(
        &self,
        track: usize,
        pattern: PatternId,
        f: impl FnOnce(&TrackPatternData) -> R,
    ) -> Option<R> {
        let scenes = self.pattern.scenes.lock().unwrap();
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern))
            .map(|pattern| f(&pattern))
    }

    /// Targeted pool write seam (clip-edit-target spec 3.4): mutate one pool
    /// pattern's stored data in place. Callers must only address patterns
    /// that are NOT currently effective for `track` — the effective pattern's
    /// truth is the live mirror, and writing its pool copy underneath it
    /// would desync the two (the `capture_pattern_step_cells` rule). The
    /// focus resolution guarantees this; gestures bail when it moves.
    pub fn with_pool_pattern_mut<R>(
        &self,
        track: usize,
        pattern: PatternId,
        f: impl FnOnce(&mut TrackPatternData) -> R,
    ) -> Option<R> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        debug_assert!(
            scenes.effective_pattern_id(track) != Some(pattern),
            "pool write addressed track {track}'s EFFECTIVE pattern {}; the live \
             mirror owns it (clip-edit-target spec 3.4)",
            pattern.0
        );
        scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| {
                let mut data = pool.get(pattern)?;
                let result = f(&mut data);
                pool.store(pattern, data);
                Some(result)
            })
            .map(|result| result)
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

    /// Re-link patterns, takes, and bare cells on `track` to existing
    /// entities (§17.3 re-link — the S2 successor of the §16.5 device copy:
    /// reference semantics, not a value copy). `patch` / `mix` may each be
    /// `None` to keep a target's current half — palette Apply moves the
    /// patch only, Apply-with-mix moves both (§17.6). `cells` are scene
    /// indices whose BARE cell refs move directly; a cell that holds a
    /// pattern must be addressed by that pattern instead, and scene cells
    /// whose cell is a re-linked pattern follow it, keeping cell and
    /// pattern naming the same entities. All-or-nothing: every target is
    /// validated before anything moves. Returns how many of the named
    /// referents moved; pattern-cell follow-ups ride along uncounted.
    pub(crate) fn relink_track_sound_refs_masked(
        &self,
        track: usize,
        patterns: &[PatternId],
        takes: &[TakeId],
        cells: &[usize],
        patch: Option<PatchId>,
        mix: Option<MixId>,
    ) -> Result<usize, String> {
        if patch.is_none() && mix.is_none() {
            return Err("Nothing to re-link: neither a patch nor a mix was named".to_string());
        }
        let merged = |current: SoundRefs| SoundRefs {
            patch: patch.unwrap_or(current.patch),
            mix: mix.unwrap_or(current.mix),
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let scenes = &mut *scenes;
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} does not exist", track + 1))?;
        if let Some(id) = patch {
            if !pool.sounds.patches.contains_key(&id) {
                return Err(format!(
                    "Patch {} does not resolve on track {}",
                    id.0,
                    track + 1
                ));
            }
        }
        if let Some(id) = mix {
            if !pool.sounds.mixes.contains_key(&id) {
                return Err(format!(
                    "Mix {} does not resolve on track {}",
                    id.0,
                    track + 1
                ));
            }
        }
        for scene_idx in cells {
            let scene = scenes
                .scenes
                .get(*scene_idx)
                .ok_or_else(|| format!("Scene {} does not exist", scene_idx + 1))?;
            if scene.cells.get(track).copied().flatten().is_some() {
                return Err(format!(
                    "Scene {} track {} holds a pattern; re-link the pattern, not the cell",
                    scene_idx + 1,
                    track + 1
                ));
            }
            if scene.cell_sounds.get(track).is_none() {
                return Err(format!(
                    "Scene {} has no cell sound for track {}",
                    scene_idx + 1,
                    track + 1
                ));
            }
        }
        // Erroring below this point would leave earlier re-links applied with
        // no history entry recording them (the caller only commits a patch on
        // Ok), so every take must resolve before anything moves.
        for take_id in takes {
            if scenes
                .take_pools
                .get(track)
                .and_then(|takes| takes.get(*take_id))
                .is_none()
            {
                return Err(format!(
                    "Take {} does not exist on track {}",
                    take_id.0,
                    track + 1
                ));
            }
        }
        let mut changed = 0;
        let mut moved_patterns: Vec<(PatternId, SoundRefs)> = Vec::new();
        for pattern in patterns {
            let Some(current) = pool.refs(*pattern) else {
                continue;
            };
            let target = merged(current);
            if pool.relink_sound(*pattern, target) {
                changed += 1;
                moved_patterns.push((*pattern, target));
            }
        }
        for take_id in takes {
            let mut moved = false;
            let (target, chunks) = scenes
                .take_pools
                .get_mut(track)
                .and_then(|takes| takes.get_mut(*take_id))
                .map(|take| {
                    let target = merged(take.sound);
                    if take.sound != target {
                        take.sound = target;
                        moved = true;
                    }
                    (target, take.chunks.clone())
                })
                .expect("validated above under the same lock");
            for chunk in chunks {
                if pool.relink_sound(chunk, target) {
                    moved = true;
                }
            }
            if moved {
                changed += 1;
            }
        }
        // Bare-cell targets: the cell's refs move directly (§17.2 "no steps
        // ≠ no sound" — a bare cell is still a referent).
        for scene_idx in cells {
            let Some(slot) = scenes
                .scenes
                .get_mut(*scene_idx)
                .and_then(|scene| scene.cell_sounds.get_mut(track))
            else {
                continue;
            };
            let target = merged(*slot);
            if *slot != target {
                *slot = target;
                changed += 1;
            }
        }
        // Cells follow their pattern (§17.3: after any launch/assignment,
        // cell and pattern name the same entities — a re-link included).
        // Uncounted: a cell can only move when its pattern just did, so
        // `changed` already reflects it.
        for scene in &mut scenes.scenes {
            if let Some(Some(cell)) = scene.cells.get(track) {
                if let Some((_, target)) = moved_patterns.iter().find(|(id, _)| id == cell) {
                    if let Some(slot) = scene.cell_sounds.get_mut(track) {
                        *slot = *target;
                    }
                }
            }
        }
        Ok(changed)
    }

    #[cfg(test)]
    pub(crate) fn with_scenes_mut<R>(&self, f: impl FnOnce(&mut ProjectScenes) -> R) -> R {
        f(&mut self.pattern.scenes.lock().unwrap())
    }

    /// §17.4 pruning: drop orphaned Patch/Mix entities from every track's
    /// pool. Returns how many entities were removed.
    pub fn prune_unreferenced_sounds(&self) -> usize {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .prune_unreferenced_sounds()
    }

    /// One-track interactive pruning (§17.4 "clean up unused" / §18.3):
    /// the same reachability rule as the save-time prune, scoped to `track`.
    pub(crate) fn prune_unreferenced_sounds_for_track(&self, track: usize) -> usize {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let keep = scenes.referenced_track_sounds(track);
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        let before = pool.sounds.patches.len() + pool.sounds.mixes.len();
        pool.sounds.retain_refs(&keep);
        before - (pool.sounds.patches.len() + pool.sounds.mixes.len())
    }

    /// Fork (§17.3 "own parameters"): mint clones of `refs` on `track`,
    /// returning the fresh pair. The caller repoints a referent at it (via
    /// the masked re-link) under the same undo entry.
    pub(crate) fn fork_track_sound(&self, track: usize, refs: SoundRefs) -> Option<SoundRefs> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes.track_pools.get_mut(track)?;
        pool.sounds.resolves(refs).then(|| pool.sounds.fork(refs))
    }

    /// Rename one entity's display name (§17.11 — overlay-only gesture).
    /// Exactly one of `patch`/`mix` is expected.
    pub(crate) fn rename_track_sound_entity(
        &self,
        track: usize,
        patch: Option<PatchId>,
        mix: Option<MixId>,
        name: &str,
    ) -> Result<(), String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} does not exist", track + 1))?;
        let meta = match (patch, mix) {
            (Some(id), None) => pool
                .sounds
                .patch_meta
                .get_mut(&id)
                .filter(|_| pool.sounds.patches.contains_key(&id)),
            (None, Some(id)) => pool
                .sounds
                .mix_meta
                .get_mut(&id)
                .filter(|_| pool.sounds.mixes.contains_key(&id)),
            _ => return Err("Name exactly one entity (a patch or a mix)".to_string()),
        };
        match meta {
            Some(meta) => {
                meta.name = name.trim().to_string();
                Ok(())
            }
            None => Err(format!("The entity does not exist on track {}", track + 1)),
        }
    }

    /// Bare-scene lazy materialization (takes spec 11.1): insert `data` into
    /// `track`'s pool and assign it to the CURRENT scene's empty cell.
    /// Returns `None` when the track is out of range or the cell already
    /// holds a pattern (callers re-resolve instead of overwriting).
    ///
    /// Seeding (track-sound spec §2.5): the new pattern's sound CLONES the
    /// track sound — the "set the sound, then record" workflow. `data` is
    /// captured from the live mirror, which on a bare lane is the track
    /// sound; its device half is first absorbed into the track sound (so
    /// un-saved mixer moves are not lost), then forked for the new cell.
    pub(crate) fn materialize_current_scene_pattern(
        &self,
        track: usize,
        data: TrackPatternData,
    ) -> Option<PatternId> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let current = scenes.current_scene;
        if scenes
            .scenes
            .get(current)?
            .cells
            .get(track)
            .copied()
            .flatten()
            .is_some()
        {
            return None;
        }
        let track_sound = scenes.track_sound_refs(track);
        let id = match track_sound {
            Some(refs) => {
                let pool = scenes.track_pools.get_mut(track)?;
                // Absorb the mirror's device half into the track sound
                // (mixer moves persist only at save-backs), then fork it
                // for the new cell. `insert_with_refs` keeps only the
                // sequence half of `data` — the fork is the sound.
                let (_seq, patch, mix) = data.clone().split();
                pool.sounds.patches.insert(refs.patch, Arc::new(patch));
                pool.sounds.mixes.insert(refs.mix, Arc::new(mix));
                let sound = pool.sounds.fork(refs);
                pool.insert_with_refs(data, sound)
            }
            None => scenes.track_pools.get_mut(track)?.insert(data),
        };
        let refs = scenes.track_pools.get(track)?.refs(id)?;
        {
            let scene = scenes.scenes.get_mut(current)?;
            *scene.cells.get_mut(track)? = Some(id);
            if let Some(cell_sound) = scene.cell_sounds.get_mut(track) {
                *cell_sound = refs;
            }
        }
        drop(scenes);
        // The scene now resolves a pattern for this track; the launch-time
        // silencing for the empty cell no longer applies.
        self.set_scene_silenced(track, false);
        self.stamp_free_run_song_offsets_for_new_lane(track, current, id);
        Some(id)
    }

    /// A lane that just came into existence in `scene_idx` (a bare track
    /// gaining its first pattern there) newly resolves in every committed
    /// song row referencing that scene. Rows captured from a live
    /// performance sit at unquantized beats and their other lanes carry
    /// free-run phase stamps (takes spec 7.2/9.4); anchoring the new lane
    /// at step 0 of each off-grid row start would play it out of time.
    /// Stamp the same free-run phase as materialized overrides so the new
    /// lane joins the grid like every captured lane. Rows aligned to the
    /// pattern grid get offset 0 and are left untouched — anchored and
    /// free-run agree there, so painted arrangements are unaffected.
    /// Existing overrides for the track (including explicit-empty) are
    /// always respected.
    pub(crate) fn stamp_free_run_song_offsets_for_new_lane(
        &self,
        track: usize,
        scene_idx: usize,
        pattern: PatternId,
    ) {
        let geometry = self.with_project_scenes(|scenes| {
            let data = scenes.track_pools.get(track)?.get(pattern)?;
            Some(data.step_geometry())
        });
        let Some(geometry) = geometry else {
            return;
        };
        let num_steps = geometry.num_steps() as f64;
        self.with_committed_song_mut(|song| {
            let Some(song) = song.as_mut() else {
                return;
            };
            for row in &mut song.rows {
                if row.scene != Some(scene_idx)
                    || row.overrides.iter().any(|over| over.track == track)
                {
                    continue;
                }
                let offset = geometry.steps_at_beats(row.start_beat);
                if offset < 1e-9 || offset > num_steps - 1e-9 {
                    continue;
                }
                row.overrides.push(ProjectSongTrackOverride {
                    track,
                    pattern_id: Some(pattern.0),
                    take_id: None,
                    offset_steps: offset,
                });
                row.overrides.sort_by_key(|over| over.track);
            }
        });
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
        target
            .validate_scene_bank_model()
            .map_err(|error| format!("Scene history contains invalid scene banks: {error}"))?;

        // A scene-order edit stores the scene memento, while arrangement/song
        // references live outside it. Stable SceneId lets replay derive the
        // exact old-index -> restored-index mapping without enlarging every
        // scene-structure history entry. This covers a pure permutation plus
        // the single-op insert/delete restores history produces (undo of a
        // mid-list insert removes the inserted scene; redo re-adds it): every
        // surviving reference follows its scene's id, and a reference to a
        // scene the restore removes collapses onto the slot it vacated, the
        // same policy as forward delete remapping.
        let current_scene_ids = self
            .pattern
            .scenes
            .lock()
            .unwrap()
            .scenes
            .iter()
            .map(|scene| scene.id)
            .collect::<Vec<_>>();
        let target_scene_indices = target
            .scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| (scene.id, index))
            .collect::<HashMap<_, _>>();
        let shared_ids = current_scene_ids
            .iter()
            .filter(|id| target_scene_indices.contains_key(id))
            .count();
        let restores_superset = shared_ids == current_scene_ids.len();
        let restores_subset = shared_ids == target_scene_indices.len();
        if shared_ids > 0 && (restores_superset || restores_subset) {
            let remap = current_scene_ids
                .iter()
                .enumerate()
                .map(|(index, id)| {
                    target_scene_indices.get(id).copied().unwrap_or_else(|| {
                        let vacated = current_scene_ids[..index]
                            .iter()
                            .filter(|id| target_scene_indices.contains_key(id))
                            .count();
                        vacated.min(target_scene_indices.len() - 1)
                    })
                })
                .collect::<Vec<_>>();
            self.with_committed_song_mut(|song| {
                if let Some(song) = song {
                    for row in &mut song.rows {
                        if let Some(scene) = row.scene.as_mut() {
                            if let Some(target) = remap.get(*scene) {
                                *scene = *target;
                            }
                        }
                    }
                }
            });
            self.with_committed_arrangement_mut(|arrangement| {
                if let Some(arrangement) = arrangement {
                    for event in &mut arrangement.scene_lane {
                        if let Some(target) = remap.get(event.scene) {
                            event.scene = *target;
                        }
                    }
                }
            });
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
                    // §2.8 load seam (symptom 7): a bare lane's mirror must
                    // come up holding the TRACK SOUND — with fresh-track
                    // defaults left there, the next stop save-back would
                    // overwrite the user's sound (and every take sharing its
                    // refs) with stock.
                    if let Some(data) = scenes
                        .track_sound_pattern(track)
                        .and_then(|id| scenes.track_pools.get(track)?.get(id))
                    {
                        data.restore_device_state_to(self, track);
                    }
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
        // One derivation seam for the three masks (§2.8): `masked_save_masks`
        // folds arrangement-context borrows into the stale + latched halves,
        // so a selected clip's live grid never saves over the inert cell.
        let save_masks = self.masked_save_masks();
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .save_scene_snapshot_masked(current_pattern, snapshot, save_masks.0, save_masks.1, save_masks.2)
    }

    /// Write one lane out of a scene-derived snapshot into the pattern that
    /// lane currently resolves to (track override first, then the scene
    /// cell). The per-track save-back sites below build their snapshot from
    /// `scene_snapshot`, so every OTHER lane in it carries that scene's CELL
    /// content; pushing the whole grid through `save_scene_snapshot_masked`
    /// would write that cell content over any lane that has a track override
    /// (a launched clip), destroying the pattern being played. A stale lane
    /// with no override is skipped for the same reason the masked full-grid
    /// save skips it.
    fn save_scene_track_lane(
        scenes: &mut ProjectScenes,
        scene_idx: usize,
        track: usize,
        snapshot: &PatternSnapshot,
        stale_mask: u64,
    ) -> bool {
        let Some(data) = snapshot.track_pattern_data(track) else {
            return false;
        };
        let override_id = scenes.track_overrides.get(track).copied().flatten();
        if track < 64 && stale_mask >> track & 1 == 1 && override_id.is_none() {
            return false;
        }
        let Some(id) = override_id.or_else(|| {
            scenes
                .scenes
                .get(scene_idx)
                .and_then(|scene| scene.cells.get(track).copied().flatten())
        }) else {
            return false;
        };
        scenes
            .track_pools
            .get_mut(track)
            .is_some_and(|pool| pool.store(id, data))
    }

    pub fn save_current_track_midi_fx_snapshot(&self, track: usize) -> bool {
        let current_pattern = self.current_pattern_index();
        let stale_mask = self.stale_live_lane_mask();
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
        Self::save_scene_track_lane(&mut scenes, current_pattern, track, &snapshot, stale_mask)
    }

    pub fn save_current_track_effect_snapshot(&self, track: usize) -> bool {
        let current_pattern = self.current_pattern_index();
        let stale_mask = self.stale_live_lane_mask();
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
        Self::save_scene_track_lane(&mut scenes, current_pattern, track, &snapshot, stale_mask)
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
        // Device-wide edit: write each distinct Patch entity once (take
        // chunks share one), while still counting per pattern as before.
        let TrackPatternPool { patterns, sounds, .. } = pool;
        let mut updated = 0;
        let mut seen: HashSet<PatchId> = HashSet::new();
        for stored in patterns.values() {
            let Some(patch) = sounds.patches.get_mut(&stored.sound.patch).map(Arc::make_mut) else {
                continue;
            };
            let Some(slot) = patch.effect_slots.get_mut(slot_idx) else {
                continue;
            };
            if seen.insert(stored.sound.patch) {
                slot.copy_base_values_from(&source_slot);
            }
            updated += 1;
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
        let TrackPatternPool { patterns, sounds, .. } = pool;
        let mut updated = 0;
        let mut seen: HashSet<PatchId> = HashSet::new();
        for stored in patterns.values() {
            let Some(patch) = sounds.patches.get_mut(&stored.sound.patch).map(Arc::make_mut) else {
                continue;
            };
            let Some(slot) = patch.midi_fx_slots.get_mut(slot_idx) else {
                continue;
            };
            if seen.insert(stored.sound.patch) {
                slot.copy_base_values_from(&source_slot);
            }
            updated += 1;
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
        let TrackPatternPool { patterns, sounds, .. } = pool;
        let mut seen: HashSet<PatchId> = HashSet::new();
        for stored in patterns.values() {
            if !seen.insert(stored.sound.patch) {
                continue;
            }
            if let Some(patch) = sounds.patches.get_mut(&stored.sound.patch).map(Arc::make_mut) {
                patch.instrument_slot.copy_base_values_from(&source_slot);
                patch.instrument_base_note_offset = source_base_note;
            }
        }
        patterns.len()
    }

}
