use super::super::*;

impl SequencerState {
    /// Repair the specific topology drift produced by tracks that were
    /// appended without extending an already committed arrangement.
    ///
    /// Missing trailing lanes are unambiguous: project tracks are append-only
    /// at creation time, so each missing lane belongs to the track at the same
    /// index and can be stamped from that track's live scene cells. Extra
    /// lanes are not repaired here because dropping authored clips would be
    /// destructive. The repaired arrangement is compiled and installed as
    /// one object, preserving the arrangement/song lockstep invariant.
    pub(crate) fn reconcile_committed_arrangement_track_lanes(
        &self,
    ) -> Result<bool, String> {
        let Some(mut arrangement) = self.committed_arrangement() else {
            return Ok(false);
        };
        let track_count = self.with_project_scenes(SongProjectContext::song_track_count);
        if arrangement.track_lanes.len() > track_count {
            return Err(format!(
                "Arrangement has {} track lane(s) but the project has {} track(s); extra \
                 authored lanes cannot be removed automatically",
                arrangement.track_lanes.len(),
                track_count
            ));
        }
        if arrangement.track_lanes.len() == track_count {
            return Ok(false);
        }

        self.with_project_scenes(|scenes| {
            for track in arrangement.track_lanes.len()..track_count {
                append_scene_stamped_track_lane(&mut arrangement, scenes, track)?;
            }
            Ok::<(), String>(())
        })?;
        self.set_committed_arrangement(Some(arrangement))?;
        Ok(true)
    }

    fn default_track_pattern_data_for_track(
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        track: usize,
        run_mode: CustomInstrumentRunMode,
        instrument: Option<(&EffectDescriptor, u32, u32, InstrumentType)>,
    ) -> Option<TrackPatternData> {
        let mut snapshot = PatternSnapshot::new_default(track_count, slot_descriptors);
        if let Some(mode) = snapshot.instrument_run_modes.get_mut(track) {
            *mode = run_mode;
        }
        if let Some((descriptor, node_id, modulator_node_id, instrument_type)) = instrument {
            snapshot.sync_instrument_slot(
                track,
                descriptor,
                node_id,
                modulator_node_id,
                instrument_type,
            );
        }
        snapshot.track_pattern_data(track)
    }

    /// Grow the scene/pool bookkeeping to cover a newly added track.
    ///
    /// EVERY scene eagerly materializes its own pattern for the new track,
    /// mirroring `new_scene`'s fork-per-track seeding: a scene must always
    /// resolve a pattern + sound for every track, or the track's device
    /// simply vanishes whenever that scene is current (a `None` cell blanks
    /// the live mirror on launch, so instrument slots report zero params
    /// and every knob edit is rejected). Scenes the user doesn't want the
    /// track to play in stay silent naturally — the seeded patterns have no
    /// steps — and arrangement-level silence is expressed by deleting the
    /// stamped clips (explicit-empty overrides), never by bare cells.
    /// Deliberate cell clearing (`clear_scene_cell`) remains the only
    /// producer of `None` cells.
    ///
    /// If an arrangement is committed, this operation also appends its
    /// first-class track lane and stamps every scene span with that scene's
    /// materialized cell. The arrangement and its compiled song are
    /// installed together before the new track count is published.
    pub fn extend_all_pattern_snapshots_to_track(
        &self,
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        track: usize,
        run_mode: CustomInstrumentRunMode,
        instrument: Option<(&EffectDescriptor, u32, u32, InstrumentType)>,
    ) -> Result<(), String> {
        let Some(default_data) = Self::default_track_pattern_data_for_track(
            track_count,
            slot_descriptors,
            track,
            run_mode,
            instrument,
        ) else {
            return Ok(());
        };
        self.reconcile_committed_arrangement_track_lanes()?;
        let mut arrangement = self.committed_arrangement();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        while scenes.track_pools.len() < track_count {
            scenes.track_pools.push(TrackPatternPool::default());
            scenes.track_overrides.push(None);
        }
        if track >= scenes.track_pools.len() {
            return Ok(());
        }
        // Placeholder pairs minted for the new track's lane are provisional:
        // the materialize loop below repoints those cells at their pattern's
        // freshly minted entities, and the placeholders are removed so a
        // track add creates exactly one pair per scene.
        let mut placeholder_refs: Vec<(usize, SoundRefs)> = Vec::new();
        for scene_idx in 0..scenes.scenes.len() {
            while scenes.scenes[scene_idx].cells.len() < track_count {
                let lane = scenes.scenes[scene_idx].cells.len();
                let refs = scenes.track_pools[lane]
                    .sounds
                    .insert(Patch::new_default(), Mix::default());
                scenes.scenes[scene_idx].cells.push(None);
                scenes.scenes[scene_idx].cell_sounds.push(refs);
                if lane == track {
                    placeholder_refs.push((scene_idx, refs));
                }
            }
        }
        // One private pattern per scene (each insert mints its own
        // Patch + Mix), so per-scene device/mixer divergence works for the
        // new track exactly as it does for tracks that predate a scene.
        let mut materialized = Vec::new();
        for scene_idx in 0..scenes.scenes.len() {
            let has_cell = scenes.scenes[scene_idx]
                .cells
                .get(track)
                .copied()
                .flatten()
                .is_some();
            if has_cell {
                continue;
            }
            let id = scenes.track_pools[track].insert(default_data.clone());
            let refs = scenes.track_pools[track].refs(id);
            scenes.scenes[scene_idx].cells[track] = Some(id);
            if let (Some(refs), Some(cell_sound)) =
                (refs, scenes.scenes[scene_idx].cell_sounds.get_mut(track))
            {
                *cell_sound = refs;
            }
            materialized.push((scene_idx, id));
        }
        // Drop the placeholders whose cells repointed; each was minted in
        // this call and only its own cell ever saw it.
        for (scene_idx, refs) in placeholder_refs {
            let still_used = scenes.scenes[scene_idx]
                .cell_sounds
                .get(track)
                .is_some_and(|cell_sound| *cell_sound == refs);
            if !still_used {
                scenes.track_pools[track].sounds.patches.remove(&refs.patch);
                scenes.track_pools[track].sounds.mixes.remove(&refs.mix);
            }
        }
        // The new track gets its track sound (track-sound spec §2.1), seeded
        // from the scene-0 cell just materialized (§2.6).
        scenes.ensure_track_sounds();
        debug_assert!(
            scenes.validate_sound_refs().is_ok(),
            "sound refs invalid after track add: {:?}",
            scenes.validate_sound_refs().err()
        );
        drop(scenes);
        if let Some(committed) = arrangement.as_mut() {
            if committed.track_lanes.len() == track {
                self.with_project_scenes(|scenes| {
                    append_scene_stamped_track_lane(committed, scenes, track)
                })?;
                self.set_committed_arrangement(arrangement)?;
            }
        } else {
            // Legacy/direct row-model callers have no authored arrangement
            // to extend. Preserve their free-run stamps.
            for (scene_idx, id) in materialized {
                self.stamp_free_run_song_offsets_for_new_lane(track, scene_idx, id);
            }
        }
        Ok(())
    }

    /// Seed `sample_id` onto every stored pattern of `track` that has never had
    /// a real sample chosen (negative buffer id). Patterns with an explicit
    /// sample keep it. Returns the number of patterns seeded.
    pub fn seed_unset_pattern_sample_ids(
        &self,
        track: usize,
        sample_id: (i32, String, u32),
    ) -> usize {
        if sample_id.0 < 0 {
            return 0;
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        // Patterns whose Patch still has no real sample, counted before any
        // write (take chunks share a Patch; each still counts as today).
        let unseeded: Vec<PatchId> = pool
            .patterns
            .values()
            .filter(|stored| {
                pool.sounds
                    .patches
                    .get(&stored.sound.patch)
                    .is_some_and(|patch| patch.sample_id.0 < 0)
            })
            .map(|stored| stored.sound.patch)
            .collect();
        let seeded = unseeded.len();
        for patch_id in unseeded {
            if let Some(patch) = pool.sounds.patches.get_mut(&patch_id).map(Arc::make_mut) {
                patch.sample_id = sample_id.clone();
            }
        }
        seeded
    }

    pub fn set_rack_track_for_all_pattern_snapshots(
        &self,
        track: usize,
        rack_track: RackTrackSnapshot,
    ) {
        if let Some(live_rack_track) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            *live_rack_track = Some(rack_track.clone());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return;
        };
        for patch in pool.sounds.patches.values_mut().map(Arc::make_mut) {
            prepare_patch_for_rack(patch);
            patch.rack_track = Some(rack_track.clone());
        }
    }

    pub fn replace_instrument_container_with_rack(
        &self,
        track: usize,
        rack_track: RackTrackSnapshot,
    ) -> bool {
        if self.validate_instrument_source_reset_target(track).is_err() {
            return false;
        }
        self.set_rack_track_for_all_pattern_snapshots(track, rack_track);
        self.pattern.instrument_slots[track].clear();
        self.pattern.instrument_run_modes[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.runtime.instrument_run_mode_flags[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.runtime.instrument_type_flags[track]
            .store(InstrumentType::Rack.runtime_flag(), Ordering::Release);
        self.runtime.track_engine_ids[track].store(u32::MAX, Ordering::Release);
        true
    }

    /// Fold a flat track's instrument and insert chain into a one-slot rack in
    /// every stored pattern. Per-pattern instrument/effect values are retained;
    /// only their ownership moves from the track container to the rack slot.
    pub fn validate_group_flat_track_to_rack(&self, track: usize) -> Result<(), String> {
        self.validate_instrument_source_reset_target(track)?;
        let scenes = self.pattern.scenes.lock().unwrap();
        let effective_pattern_id = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no stored pattern pool", track + 1))?;
        if !pool.patterns.contains_key(&effective_pattern_id) {
            return Err(format!(
                "Track {} effective pattern is missing from its pool",
                track + 1
            ));
        }
        for (scene_idx, scene) in scenes.scenes.iter().enumerate() {
            let Some(pattern_id) = scene.cells.get(track).copied().flatten() else {
                continue;
            };
            if !pool.contains(pattern_id) {
                return Err(format!(
                    "Track {} scene {} references a missing pattern",
                    track + 1,
                    scene_idx + 1
                ));
            }
        }
        if let Some(pattern_id) = scenes.track_overrides.get(track).copied().flatten() {
            if !pool.contains(pattern_id) {
                return Err(format!(
                    "Track {} override references a missing pattern",
                    track + 1
                ));
            }
        }
        Ok(())
    }

    pub fn group_flat_track_to_rack(
        &self,
        track: usize,
        instrument_type: InstrumentType,
        instrument_run_mode: CustomInstrumentRunMode,
        engine_id: Option<usize>,
        effect_descriptors: &[EffectDescriptor],
        custom_effect_names: &[Option<String>],
    ) -> Option<RackTrackSnapshot> {
        self.validate_group_flat_track_to_rack(track).ok()?;
        let live_instrument =
            EffectSlotSnapshot::capture(self.pattern.instrument_slots.get(track)?);
        let live_effects = self
            .pattern
            .effect_chains
            .get(track)?
            .iter()
            .map(EffectSlotSnapshot::capture)
            .collect::<Vec<_>>();

        let make_slot = |data: &Patch| {
            let mut track_sound_state = data.track_sound_state.clone();
            track_sound_state.engine_id = engine_id;
            let mut slot = RackSlotSnapshot {
                instrument_type,
                instrument_run_mode,
                instrument_base_note_offset: data.instrument_base_note_offset,
                choke_group: None,
                gain: 1.0,
                pan: 0.0,
                mute: false,
                solo: false,
                max_polyphony: crate::audio::MAX_VOICES,
                param_plocks: RackSlotParamPlocks::new(),
                instrument_slot: data.instrument_slot.clone(),
                effect_slots: data.effect_slots.clone(),
                effect_descriptors: effect_descriptors.to_vec(),
                custom_effect_names: custom_effect_names.to_vec(),
                track_sound_state,
                sample_id: (instrument_type == InstrumentType::Sampler)
                    .then(|| data.sample_id.clone()),
            };
            slot.normalize_effect_chain();
            slot
        };

        let live_rack = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let effective_id = scenes.effective_pattern_id(track)?;
            let pool = scenes.track_pools.get_mut(track)?;
            let mut effective_rack = None;
            // Pattern ids stay untouched — the migration rewrites each Patch
            // ENTITY in place (once each; take chunks share one), so scene
            // cells and the launched-pattern override keep their mappings.
            let effective_patch = pool.refs(effective_id).map(|refs| refs.patch);
            for (patch_id, patch) in pool.sounds.patches.iter_mut().map(|(id, entity)| (id, Arc::make_mut(entity))) {
                let slot = make_slot(patch);
                if Some(*patch_id) == effective_patch {
                    effective_rack = Some(RackTrackSnapshot {
                        slots: vec![slot.clone()],
                        macros: default_rack_macros(),
                        runtime_macro_values: None,
                        runtime_macro_track: 0,
                    });
                }
                prepare_patch_for_rack(patch);
                patch.effect_slots = (0..crate::lisp_host::MAX_CUSTOM_FX)
                    .map(|_| EffectSlotSnapshot::new_empty())
                    .collect();
                patch.rack_track = Some(RackTrackSnapshot {
                    slots: vec![slot],
                    macros: default_rack_macros(),
                    runtime_macro_values: None,
                    runtime_macro_track: 0,
                });
            }
            effective_rack?
        };

        let mut live_rack = live_rack;
        live_rack.slots[0].instrument_slot = live_instrument;
        live_rack.slots[0].effect_slots = live_effects;
        live_rack.slots[0].normalize_effect_chain();
        self.pattern.rack_tracks.lock().unwrap()[track] = Some(live_rack.clone());
        self.pattern.instrument_slots[track].clear();
        for slot in &self.pattern.effect_chains[track] {
            slot.clear();
        }
        self.pattern.instrument_run_modes[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.runtime.instrument_run_mode_flags[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.runtime.instrument_type_flags[track]
            .store(InstrumentType::Rack.runtime_flag(), Ordering::Release);
        self.runtime.track_engine_ids[track].store(u32::MAX, Ordering::Release);
        Some(live_rack)
    }

    pub fn append_rack_slot_to_current_pattern(
        &self,
        track: usize,
        slot: RackSlotSnapshot,
    ) -> bool {
        if self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .is_none()
        {
            return false;
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(id) = scenes.effective_pattern_id(track) else {
                return false;
            };
            if scenes
                .track_pools
                .get(track)
                .and_then(|pool| pool.get(id))
                .is_none()
            {
                return false;
            }
        }

        if let Some(live_rack_track) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            match live_rack_track {
                Some(rack_track) => rack_track.slots.push(slot.clone()),
                None => {
                    *live_rack_track = Some(RackTrackSnapshot {
                        slots: vec![slot.clone()],
                        macros: default_rack_macros(),
                        runtime_macro_values: None,
                        runtime_macro_track: 0,
                    });
                }
            }
        } else {
            return false;
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let id = scenes
            .effective_pattern_id(track)
            .expect("validated current pattern id before rack slot append");
        let patch = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.patch_mut(id))
            .expect("validated current pattern data before rack slot append");
        prepare_patch_for_rack(patch);
        match patch.rack_track.as_mut() {
            Some(rack_track) => rack_track.slots.push(slot),
            None => {
                patch.rack_track = Some(RackTrackSnapshot {
                    slots: vec![slot],
                    macros: default_rack_macros(),
                    runtime_macro_values: None,
                    runtime_macro_track: 0,
                });
            }
        }
        true
    }

    pub fn append_rack_slot_for_all_pattern_snapshots(
        &self,
        track: usize,
        slot: RackSlotSnapshot,
    ) {
        if let Some(live_rack_track) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            match live_rack_track {
                Some(rack_track) => rack_track.slots.push(slot.clone()),
                None => {
                    *live_rack_track = Some(RackTrackSnapshot {
                        slots: vec![slot.clone()],
                        macros: default_rack_macros(),
                        runtime_macro_values: None,
                        runtime_macro_track: 0,
                    });
                }
            }
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return;
        };
        for patch in pool.sounds.patches.values_mut().map(Arc::make_mut) {
            prepare_patch_for_rack(patch);
            match patch.rack_track.as_mut() {
                Some(rack_track) => rack_track.slots.push(slot.clone()),
                None => {
                    patch.rack_track = Some(RackTrackSnapshot {
                        slots: vec![slot.clone()],
                        macros: default_rack_macros(),
                        runtime_macro_values: None,
                        runtime_macro_track: 0,
                    });
                }
            }
        }
    }

    pub fn remove_rack_slot_from_all_pattern_snapshots(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> bool {
        let mut removed = false;
        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            if slot_idx < live_rack_track.slots.len() {
                live_rack_track.slots.remove(slot_idx);
                remove_rack_macro_slot_targets(&mut live_rack_track.macros, slot_idx);
                removed = true;
            }
        }
        if !removed {
            return false;
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        if let Some(pool) = scenes.track_pools.get_mut(track) {
            for patch in pool.sounds.patches.values_mut().map(Arc::make_mut) {
                if let Some(rack_track) = patch.rack_track.as_mut() {
                    if slot_idx < rack_track.slots.len() {
                        rack_track.slots.remove(slot_idx);
                        remove_rack_macro_slot_targets(&mut rack_track.macros, slot_idx);
                    }
                }
            }
        }
        true
    }

    pub fn remove_rack_slot_from_current_pattern(&self, track: usize, slot_idx: usize) -> bool {
        {
            let rack_tracks = self.pattern.rack_tracks.lock().unwrap();
            let Some(Some(live_rack_track)) = rack_tracks.get(track) else {
                return false;
            };
            if live_rack_track.slots.get(slot_idx).is_none() {
                return false;
            }
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(id) = scenes.effective_pattern_id(track) else {
                return false;
            };
            let Some(patch) = scenes.track_pools.get(track).and_then(|pool| pool.patch(id))
            else {
                return false;
            };
            let Some(rack_track) = patch.rack_track.as_ref() else {
                return false;
            };
            if rack_track.slots.get(slot_idx).is_none() {
                return false;
            }
        }

        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            live_rack_track.slots.remove(slot_idx);
            remove_rack_macro_slot_targets(&mut live_rack_track.macros, slot_idx);
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let id = scenes
            .effective_pattern_id(track)
            .expect("validated current pattern id before rack slot removal");
        let patch = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.patch_mut(id))
            .expect("validated current pattern data before rack slot removal");
        let rack_track = patch
            .rack_track
            .as_mut()
            .expect("validated current rack track before rack slot removal");
        rack_track.slots.remove(slot_idx);
        remove_rack_macro_slot_targets(&mut rack_track.macros, slot_idx);
        true
    }

    pub fn replace_rack_slot_source_in_current_pattern(
        &self,
        track: usize,
        slot_idx: usize,
        replacement: RackSlotSnapshot,
    ) -> bool {
        {
            let rack_tracks = self.pattern.rack_tracks.lock().unwrap();
            let Some(Some(live_rack_track)) = rack_tracks.get(track) else {
                return false;
            };
            if live_rack_track.slots.get(slot_idx).is_none() {
                return false;
            }
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(id) = scenes.effective_pattern_id(track) else {
                return false;
            };
            let Some(patch) = scenes.track_pools.get(track).and_then(|pool| pool.patch(id))
            else {
                return false;
            };
            let Some(rack_track) = patch.rack_track.as_ref() else {
                return false;
            };
            if rack_track.slots.get(slot_idx).is_none() {
                return false;
            }
        }

        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            let slot = live_rack_track
                .slots
                .get_mut(slot_idx)
                .expect("validated live rack slot before source replacement");
            replace_rack_slot_source_preserving_controls(slot, &replacement);
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let id = scenes
            .effective_pattern_id(track)
            .expect("validated current pattern id before source replacement");
        let patch = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.patch_mut(id))
            .expect("validated current pattern data before source replacement");
        prepare_patch_for_rack(patch);
        let rack_track = patch
            .rack_track
            .as_mut()
            .expect("validated current rack track before source replacement");
        let slot = rack_track
            .slots
            .get_mut(slot_idx)
            .expect("validated current rack slot before source replacement");
        replace_rack_slot_source_preserving_controls(slot, &replacement);
        true
    }

    pub fn replace_rack_slot_source_for_all_pattern_snapshots(
        &self,
        track: usize,
        slot_idx: usize,
        replacement: RackSlotSnapshot,
    ) -> bool {
        let mut replaced = false;
        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            if let Some(slot) = live_rack_track.slots.get_mut(slot_idx) {
                replace_rack_slot_source_preserving_controls(slot, &replacement);
                replaced = true;
            }
        }
        if !replaced {
            return false;
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        if let Some(pool) = scenes.track_pools.get_mut(track) {
            for patch in pool.sounds.patches.values_mut().map(Arc::make_mut) {
                if let Some(rack_track) = patch.rack_track.as_mut() {
                    if let Some(slot) = rack_track.slots.get_mut(slot_idx) {
                        replace_rack_slot_source_preserving_controls(slot, &replacement);
                    }
                }
            }
        }
        true
    }

    pub fn sync_rack_slot_instrument_bindings_for_current_pattern(
        &self,
        track: usize,
        bindings: &[(EffectDescriptor, u32, u32)],
    ) -> bool {
        let sync_slots = |slots: &mut [RackSlotSnapshot]| {
            for (slot, (descriptor, node_id, modulator_node_id)) in
                slots.iter_mut().zip(bindings.iter())
            {
                slot.instrument_slot.sync_to_descriptor_with_modulator(
                    descriptor,
                    *node_id,
                    *modulator_node_id,
                );
            }
        };

        let mut synced_live = false;
        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            sync_slots(&mut live_rack_track.slots);
            synced_live = true;
        }

        let mut synced_current = false;
        let mut scenes = self.pattern.scenes.lock().unwrap();
        if let Some(id) = scenes.effective_pattern_id(track) {
            if let Some(patch) = scenes
                .track_pools
                .get_mut(track)
                .and_then(|pool| pool.patch_mut(id))
            {
                if let Some(rack_track) = patch.rack_track.as_mut() {
                    sync_slots(&mut rack_track.slots);
                    synced_current = true;
                }
            }
        }
        synced_live && synced_current
    }

    pub fn sync_rack_slot_instrument_bindings_for_all_patterns(
        &self,
        track: usize,
        bindings: &[(EffectDescriptor, u32, u32)],
    ) -> bool {
        let sync_slots = |slots: &mut [RackSlotSnapshot]| {
            if slots.len() != bindings.len() {
                return false;
            }
            for (slot, (descriptor, node_id, modulator_node_id)) in
                slots.iter_mut().zip(bindings.iter())
            {
                slot.instrument_slot.sync_to_descriptor_with_modulator(
                    descriptor,
                    *node_id,
                    *modulator_node_id,
                );
            }
            true
        };
        let synced_live = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get_mut(track)
            .and_then(Option::as_mut)
            .is_some_and(|rack| sync_slots(&mut rack.slots));
        if !synced_live {
            return false;
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return false;
        };
        if pool.patterns.values().any(|stored| {
            pool.sounds
                .patches
                .get(&stored.sound.patch)
                .and_then(|patch| patch.rack_track.as_ref())
                .is_none_or(|rack| rack.slots.len() != bindings.len())
        }) {
            return false;
        }
        for patch in pool.sounds.patches.values_mut().map(Arc::make_mut) {
            if let Some(rack) = patch.rack_track.as_mut() {
                if rack.slots.len() == bindings.len() {
                    sync_slots(&mut rack.slots);
                }
            }
        }
        true
    }

    pub fn update_live_rack_slot<F>(&self, track: usize, slot_idx: usize, update: F) -> bool
    where
        F: FnOnce(&mut RackSlotSnapshot),
    {
        let mut rack_tracks = self.pattern.rack_tracks.lock().unwrap();
        let Some(Some(rack_track)) = rack_tracks.get_mut(track) else {
            return false;
        };
        let Some(slot) = rack_track.slots.get_mut(slot_idx) else {
            return false;
        };
        update(slot);
        true
    }

    pub fn update_rack_macro_in_current_pattern<F>(
        &self,
        track: usize,
        id: RackMacroId,
        update: F,
    ) -> bool
    where
        F: Fn(&mut RackMacro),
    {
        let index = id.index();
        {
            let racks = self.pattern.rack_tracks.lock().unwrap();
            if racks
                .get(track)
                .and_then(Option::as_ref)
                .and_then(|rack| rack.macros.get(index))
                .is_none()
            {
                return false;
            }
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(pattern_id) = scenes.effective_pattern_id(track) else {
                return false;
            };
            if scenes
                .track_pools
                .get(track)
                .and_then(|pool| pool.patch(pattern_id))
                .and_then(|patch| patch.rack_track.as_ref())
                .and_then(|rack| rack.macros.get(index))
                .is_none()
            {
                return false;
            }
        }
        if let Some(rack_macro) = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get_mut(track)
            .and_then(Option::as_mut)
            .and_then(|rack| rack.macros.get_mut(index))
        {
            update(rack_macro);
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pattern_id = scenes
            .effective_pattern_id(track)
            .expect("validated rack pattern");
        let rack_macro = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.patch_mut(pattern_id))
            .and_then(|patch| patch.rack_track.as_mut())
            .and_then(|rack| rack.macros.get_mut(index))
            .expect("validated rack macro");
        update(rack_macro);
        true
    }

    /// Set one rack macro's parameter locks in both live and persisted views as
    /// a single transaction. Macro-knob drags commonly target many selected
    /// steps, so validating and locking the two rack snapshots once avoids
    /// repeating the full state transaction for every selected step.
    pub fn set_rack_macro_plocks_in_current_pattern(
        &self,
        track: usize,
        id: RackMacroId,
        steps: &[usize],
        value: f32,
    ) -> bool {
        let valid_steps = steps
            .iter()
            .copied()
            .filter(|step| *step < MAX_STEPS)
            .collect::<Vec<_>>();
        if valid_steps.is_empty() {
            return false;
        }
        let value = value.clamp(0.0, 1.0);
        let updated = self.update_rack_macro_in_current_pattern(track, id, |rack_macro| {
            for &step in &valid_steps {
                rack_macro.plocks[step] = Some(value);
            }
        });
        if updated {
            for step in valid_steps {
                self.rack_macro_runtime_values
                    .set_plock(track, id, step, Some(value));
            }
        }
        updated
    }

    pub fn set_live_rack_macro_default(&self, track: usize, id: RackMacroId, value: f32) {
        self.rack_macro_runtime_values.set_default(track, id, value);
    }

    pub(crate) fn rack_macro_runtime_values(&self) -> Arc<RackMacroRuntimeValues> {
        Arc::clone(&self.rack_macro_runtime_values)
    }

    pub(crate) fn sync_rack_macro_runtime_track(
        &self,
        track: usize,
        rack: Option<&RackTrackSnapshot>,
    ) {
        self.rack_macro_runtime_values.sync_track(track, rack);
    }

    pub fn update_rack_macros_for_all_pattern_snapshots<F>(&self, track: usize, update: F)
    where
        F: Fn(&mut Vec<RackMacro>),
    {
        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            update(&mut rack.macros);
        }
        if let Some(pool) = self
            .pattern
            .scenes
            .lock()
            .unwrap()
            .track_pools
            .get_mut(track)
        {
            for patch in pool.sounds.patches.values_mut().map(Arc::make_mut) {
                if let Some(rack) = patch.rack_track.as_mut() {
                    update(&mut rack.macros);
                }
            }
        }
    }

    /// Apply an edit to both the scheduler's live rack snapshot and the
    /// effective pattern that owns it. Validation happens before either copy
    /// is changed so a malformed pattern cannot leave the two views split.
    pub fn update_rack_slot_in_current_pattern<F>(
        &self,
        track: usize,
        slot_idx: usize,
        update: F,
    ) -> bool
    where
        F: Fn(&mut RackSlotSnapshot),
    {
        {
            let rack_tracks = self.pattern.rack_tracks.lock().unwrap();
            let Some(Some(rack)) = rack_tracks.get(track) else {
                return false;
            };
            if rack.slots.get(slot_idx).is_none() {
                return false;
            }
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(pattern_id) = scenes.effective_pattern_id(track) else {
                return false;
            };
            let Some(slot) = scenes
                .track_pools
                .get(track)
                .and_then(|pool| pool.patch(pattern_id))
                .and_then(|patch| patch.rack_track.as_ref())
                .and_then(|rack| rack.slots.get(slot_idx))
            else {
                return false;
            };
            let _ = slot;
        }

        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            update(&mut rack.slots[slot_idx]);
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pattern_id = scenes
            .effective_pattern_id(track)
            .expect("validated effective rack pattern before mutation");
        let slot = scenes.track_pools[track]
            .patch_mut(pattern_id)
            .and_then(|patch| patch.rack_track.as_mut())
            .and_then(|rack| rack.slots.get_mut(slot_idx))
            .expect("validated current rack slot before mutation");
        update(slot);
        true
    }

    /// Apply a structural rack-slot edit to the live scheduler snapshot and
    /// every stored pattern for the track. Rack instruments and their effect
    /// nodes are graph-owned device identity; only parameter values and locks
    /// vary per pattern. Adding, removing, or moving a device therefore must
    /// not leave another scene pointing at an empty or stale graph node.
    pub fn update_rack_slot_in_all_pattern_snapshots<F>(
        &self,
        track: usize,
        slot_idx: usize,
        update: F,
    ) -> bool
    where
        F: Fn(&mut RackSlotSnapshot),
    {
        {
            let rack_tracks = self.pattern.rack_tracks.lock().unwrap();
            let Some(Some(live_rack)) = rack_tracks.get(track) else {
                return false;
            };
            if live_rack.slots.get(slot_idx).is_none() {
                return false;
            }
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(pool) = scenes.track_pools.get(track) else {
                return false;
            };
            if pool.patterns.values().any(|stored| {
                pool.sounds
                    .patches
                    .get(&stored.sound.patch)
                    .and_then(|patch| patch.rack_track.as_ref())
                    .and_then(|rack| rack.slots.get(slot_idx))
                    .is_none()
            }) {
                return false;
            }
        }

        let mut rack_tracks = self.pattern.rack_tracks.lock().unwrap();
        let live_slot = rack_tracks[track]
            .as_mut()
            .and_then(|rack| rack.slots.get_mut(slot_idx))
            .expect("validated live rack slot before structural mutation");
        update(live_slot);
        drop(rack_tracks);

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .expect("validated rack pattern pool before structural mutation");
        for patch in pool.sounds.patches.values_mut().map(Arc::make_mut) {
            let Some(slot) = patch
                .rack_track
                .as_mut()
                .and_then(|rack| rack.slots.get_mut(slot_idx))
            else {
                continue;
            };
            update(slot);
        }
        true
    }

    pub fn capture_rack_slot_pattern_state(
        &self,
        track: usize,
        slot_index: usize,
    ) -> Result<RackSlotPatternStateSnapshot, String> {
        let live = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(slot_index))
            .cloned()
            .ok_or_else(|| format!("Track {} rack slot {} is missing", track + 1, slot_index + 1))?;
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let mut patterns = Vec::with_capacity(pool.patterns.len());
        for (pattern_id, stored) in &pool.patterns {
            let slot = pool
                .sounds
                .patches
                .get(&stored.sound.patch)
                .and_then(|patch| patch.rack_track.as_ref())
                .and_then(|rack| rack.slots.get(slot_index))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "Track {} pattern {:?} has no rack slot {}",
                        track + 1,
                        pattern_id,
                        slot_index + 1
                    )
                })?;
            patterns.push((*pattern_id, slot));
        }
        Ok(RackSlotPatternStateSnapshot {
            slot_index,
            live,
            patterns,
        })
    }

    pub fn capture_rack_macro_pattern_state(
        &self,
        track: usize,
    ) -> Result<RackMacroPatternStateSnapshot, String> {
        let live = self.pattern.rack_tracks.lock().unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .map(|rack| rack.macros.clone())
            .ok_or_else(|| format!("Track {} has no live rack", track + 1))?;
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes.track_pools.get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let patterns = pool.patterns.iter().map(|(pattern, stored)| {
            pool.sounds
                .patches
                .get(&stored.sound.patch)
                .and_then(|patch| patch.rack_track.as_ref())
                .map(|rack| (*pattern, rack.macros.clone()))
                .ok_or_else(|| format!("Track {} pattern {:?} has no rack", track + 1, pattern))
        }).collect::<Result<Vec<_>, String>>()?;
        Ok(RackMacroPatternStateSnapshot { live, patterns })
    }

    pub fn restore_rack_macro_pattern_state(
        &self,
        track: usize,
        snapshot: &RackMacroPatternStateSnapshot,
    ) -> Result<(), String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes.track_pools.get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot.patterns.iter().any(|(pattern, _)| !pool.patterns.contains_key(pattern))
        {
            return Err(format!("Track {} rack macro pattern topology changed", track + 1));
        }
        for (pattern, macros) in &snapshot.patterns {
            let rack = pool.patch_mut(*pattern)
                .and_then(|patch| patch.rack_track.as_mut())
                .ok_or_else(|| format!("Track {} pattern {:?} has no rack", track + 1, pattern))?;
            rack.macros.clone_from(macros);
        }
        drop(scenes);
        let mut racks = self.pattern.rack_tracks.lock().unwrap();
        let rack = racks.get_mut(track).and_then(Option::as_mut)
            .ok_or_else(|| format!("Track {} has no live rack", track + 1))?;
        rack.macros.clone_from(&snapshot.live);
        self.sync_rack_macro_runtime_track(track, Some(rack));
        Ok(())
    }

    pub fn restore_rack_slot_effect_pattern_state(
        &self,
        track: usize,
        snapshot: &RackSlotPatternStateSnapshot,
    ) -> Result<(), String> {
        self.validate_rack_slot_pattern_state(track, snapshot)?;
        let copy_effects = |slot: &mut RackSlotSnapshot, saved: &RackSlotSnapshot| {
            slot.effect_slots.clone_from(&saved.effect_slots);
            slot.effect_descriptors.clone_from(&saved.effect_descriptors);
            slot.custom_effect_names.clone_from(&saved.custom_effect_names);
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes.track_pools.get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        for (pattern, saved) in &snapshot.patterns {
            let slot = pool.patch_mut(*pattern)
                .and_then(|patch| patch.rack_track.as_mut())
                .and_then(|rack| rack.slots.get_mut(snapshot.slot_index))
                .ok_or_else(|| format!("Track {} pattern {:?} lost rack slot {}", track + 1, pattern, snapshot.slot_index + 1))?;
            copy_effects(slot, saved);
        }
        drop(scenes);
        let mut racks = self.pattern.rack_tracks.lock().unwrap();
        let slot = racks.get_mut(track)
            .and_then(Option::as_mut)
            .and_then(|rack| rack.slots.get_mut(snapshot.slot_index))
            .ok_or_else(|| format!("Track {} lost rack slot {}", track + 1, snapshot.slot_index + 1))?;
        copy_effects(slot, &snapshot.live);
        Ok(())
    }

    pub fn restore_rack_slot_pattern_state(
        &self,
        track: usize,
        snapshot: &RackSlotPatternStateSnapshot,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) -> Result<(), String> {
        let sync_slot = |slot: &mut RackSlotSnapshot, saved: &RackSlotSnapshot| {
            *slot = saved.clone();
            slot.instrument_slot.sync_to_descriptor_with_modulator(
                descriptor,
                node_id,
                modulator_node_id,
            );
        };
        self.validate_rack_slot_pattern_state(track, snapshot)?;
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot.patterns.iter().any(|(id, _)| {
                pool.patch(*id)
                    .and_then(|patch| patch.rack_track.as_ref())
                    .and_then(|rack| rack.slots.get(snapshot.slot_index))
                    .is_none()
            })
        {
            return Err(format!(
                "Track {} rack pattern topology changed before history replay",
                track + 1
            ));
        }
        for (pattern_id, saved) in &snapshot.patterns {
            let slot = pool
                .patch_mut(*pattern_id)
                .and_then(|patch| patch.rack_track.as_mut())
                .and_then(|rack| rack.slots.get_mut(snapshot.slot_index))
                .expect("rack pattern topology was validated");
            sync_slot(slot, saved);
        }
        drop(scenes);
        let mut racks = self.pattern.rack_tracks.lock().unwrap();
        let slot = racks[track]
            .as_mut()
            .and_then(|rack| rack.slots.get_mut(snapshot.slot_index))
            .expect("live rack topology was validated");
        sync_slot(slot, &snapshot.live);
        Ok(())
    }

    pub fn validate_rack_slot_pattern_state(
        &self,
        track: usize,
        snapshot: &RackSlotPatternStateSnapshot,
    ) -> Result<(), String> {
        let racks = self.pattern.rack_tracks.lock().unwrap();
        if racks
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(snapshot.slot_index))
            .is_none()
        {
            return Err(format!(
                "Track {} rack slot {} disappeared before history replay",
                track + 1,
                snapshot.slot_index + 1
            ));
        }
        drop(racks);
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot.patterns.iter().any(|(id, _)| {
                pool.patch(*id)
                    .and_then(|patch| patch.rack_track.as_ref())
                    .and_then(|rack| rack.slots.get(snapshot.slot_index))
                    .is_none()
            })
        {
            return Err(format!(
                "Track {} rack pattern topology changed before history replay",
                track + 1
            ));
        }
        Ok(())
    }

    pub fn validate_rack_slot_append_pattern_state(
        &self,
        track: usize,
        snapshot: &RackSlotPatternStateSnapshot,
    ) -> Result<(), String> {
        let racks = self.pattern.rack_tracks.lock().unwrap();
        let live_len = racks
            .get(track)
            .and_then(Option::as_ref)
            .map(|rack| rack.slots.len())
            .ok_or_else(|| format!("Track {} has no live rack", track + 1))?;
        if live_len != snapshot.slot_index {
            return Err(format!(
                "Track {} rack has {live_len} slots; history expected {} before append",
                track + 1,
                snapshot.slot_index
            ));
        }
        drop(racks);
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot.patterns.iter().any(|(id, _)| {
                pool.patch(*id)
                    .and_then(|patch| patch.rack_track.as_ref())
                    .is_none_or(|rack| rack.slots.len() != snapshot.slot_index)
            })
        {
            return Err(format!(
                "Track {} rack pattern topology changed before slot append replay",
                track + 1
            ));
        }
        Ok(())
    }
}
