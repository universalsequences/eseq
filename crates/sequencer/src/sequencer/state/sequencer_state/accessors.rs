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
        // A borrowed lane's mirror holds a take's/clip's devices, not the
        // scene pattern's; capturing it would save that sound over the scene
        // (takes spec 16.2). Release first — the App rebinds on its next
        // tick, after whatever launch or row transition triggered this.
        self.release_bound_device_state();
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
            scheduler_rendered_beats_bits: AtomicU64::new(0.0_f64.to_bits()),
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
            song_manual_latch: AtomicU64::new(0),
            song_take_lane_mask: AtomicU64::new(0),
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

    /// Insert `chunks` into `track`'s pattern pool and register a take over
    /// them (takes spec 6.1). Production facade for scenes mutation (the raw
    /// `with_scenes_mut` seam is test-only).
    pub(crate) fn register_track_take(
        &self,
        track: usize,
        name: Option<String>,
        chunks: Vec<TrackPatternData>,
        total_len_steps: u32,
    ) -> Result<TakeId, String> {
        if chunks.is_empty() {
            return Err("a take requires at least one chunk pattern".to_string());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        if track >= scenes.track_pools.len() {
            return Err(format!("track {} does not exist", track + 1));
        }
        let chunk_ids: Vec<PatternId> = {
            let pool = &mut scenes.track_pools[track];
            chunks.into_iter().map(|data| pool.insert(data)).collect()
        };
        while scenes.take_pools.len() < scenes.track_pools.len() {
            scenes.take_pools.push(TrackTakePool::default());
        }
        Ok(scenes.take_pools[track].insert(name, chunk_ids, total_len_steps))
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
        let existing = take.chunks.len();
        let mut minted = Vec::new();
        if needed_chunks > existing {
            let mut blank = scenes.track_pools[track]
                .get(template_id)
                .cloned()
                .ok_or_else(|| {
                    format!("take {}'s template chunk is missing from the pool", take_id.0)
                })?;
            blank.clear_step_content();
            blank.track_params.num_steps = MAX_STEPS;
            for _ in existing..needed_chunks {
                minted.push(scenes.track_pools[track].insert(blank.clone()));
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
                let chunks: Vec<PatternId> =
                    chunk_data.into_iter().map(|data| pool.insert(data)).collect();
                let Some(take_pool) = scenes.take_pools.get_mut(track) else {
                    continue;
                };
                take_pool.takes.push(TrackTake {
                    id: TakeId(id),
                    name,
                    chunks,
                    total_len_steps,
                });
                take_pool.next_take_id = take_pool.next_take_id.max(id.saturating_add(1));
            }
            if let Some(take_pool) = scenes.take_pools.get_mut(track) {
                take_pool.next_take_id = take_pool.next_take_id.max(next_take_id);
            }
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
            .map(f)
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
            .and_then(|pool| pool.get_mut(pattern))
            .map(f)
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

    /// Copy `source`'s device snapshot onto `targets` within `track`'s
    /// pattern pool, leaving every target's step content alone. The one
    /// production seam for moving a sound between patterns: the explicit
    /// propagation gestures (takes spec 16.5). Returns how many targets
    /// actually changed hands.
    pub(crate) fn copy_track_pattern_device_state(
        &self,
        track: usize,
        source: PatternId,
        targets: &[PatternId],
    ) -> Result<usize, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} does not exist", track + 1))?;
        let sound = pool
            .get(source)
            .ok_or_else(|| format!("Pattern {} is not in the track's pool", source.0))?
            .clone();
        let mut copied = 0;
        for target in targets {
            if *target == source {
                continue;
            }
            let Some(data) = pool.get_mut(*target) else {
                continue;
            };
            data.copy_device_state_from(&sound);
            copied += 1;
        }
        Ok(copied)
    }

    #[cfg(test)]
    pub(crate) fn with_scenes_mut<R>(&self, f: impl FnOnce(&mut ProjectScenes) -> R) -> R {
        f(&mut self.pattern.scenes.lock().unwrap())
    }

    /// Bare-scene lazy materialization (takes spec 11.1): insert `data` into
    /// `track`'s pool and assign it to the CURRENT scene's empty cell.
    /// Returns `None` when the track is out of range or the cell already
    /// holds a pattern (callers re-resolve instead of overwriting).
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
        let id = scenes.track_pools.get_mut(track)?.insert(data);
        *scenes.scenes.get_mut(current)?.cells.get_mut(track)? = Some(id);
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
        let mapping = self.with_project_scenes(|scenes| {
            let data = scenes.track_pools.get(track)?.get(pattern)?;
            let num_steps = data.track_params.num_steps.max(1);
            let step_beats = data.track_params.timebase.step_beats(num_steps);
            (step_beats > 0.0).then(|| (1.0 / step_beats, num_steps as f64))
        });
        let Some((steps_per_beat, num_steps)) = mapping else {
            return;
        };
        self.with_committed_song_mut(|song| {
            let Some(song) = song.as_mut() else {
                return;
            };
            for row in &mut song.rows {
                if row.scene != scene_idx
                    || row.overrides.iter().any(|over| over.track == track)
                {
                    continue;
                }
                let offset = (row.start_beat * steps_per_beat).rem_euclid(num_steps);
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
