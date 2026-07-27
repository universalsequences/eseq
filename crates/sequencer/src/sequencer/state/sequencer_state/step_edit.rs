use super::super::*;

impl SequencerState {
    pub(crate) fn toggle_step_and_clear_plocks_no_publish(&self, track: usize, step: usize) {
        let was_active = self.pattern.patterns[track].is_active(step);
        if was_active {
            let params: [f32; NUM_PARAMS] = std::array::from_fn(|param_idx| {
                self.pattern.step_data[track].get(step, StepParam::ALL[param_idx])
            });
            self.clear_step_payload_inner(track, step);
            for param in StepParam::ALL {
                self.pattern.step_data[track].set(step, param, params[param.index()]);
            }
        } else {
            self.pattern.patterns[track].set_step_active(step, true);
        }
    }

    pub fn toggle_step_and_clear_plocks(&self, track: usize, step: usize) {
        self.toggle_step_and_clear_plocks_no_publish(track, step);
        self.publish_scheduler_snapshot();
    }

    fn drum_lane_notes(&self, track: usize, step: usize) -> Vec<(f32, f32, f32)> {
        if track >= MAX_TRACKS || step >= MAX_STEPS || !self.pattern.patterns[track].is_active(step)
        {
            return Vec::new();
        }

        let step_duration = self.pattern.step_data[track]
            .get(step, StepParam::Duration)
            .max(0.0);
        let step_delay = self.pattern.step_data[track].get(step, StepParam::Delay);
        let chord_count = self.pattern.chord_data[track].count(step);
        if chord_count == 0 {
            return vec![(
                self.pattern.step_data[track].get(step, StepParam::Transpose),
                step_duration,
                step_delay,
            )];
        }

        (0..chord_count)
            .map(|voice| {
                let duration = self.pattern.chord_data[track].get_duration(step, voice);
                (
                    self.pattern.chord_data[track].get(step, voice),
                    if duration > 0.0 {
                        duration
                    } else {
                        step_duration
                    },
                    self.pattern.chord_data[track].get_delay(step, voice),
                )
            })
            .collect()
    }

    fn write_drum_lane_notes(&self, track: usize, step: usize, mut notes: Vec<(f32, f32, f32)>) {
        if notes.is_empty() {
            self.clear_step_payload_inner(track, step);
            return;
        }

        notes.sort_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.pattern.chord_data[track].clear_step(step);
        let max_duration = notes
            .iter()
            .map(|(_, duration, _)| *duration)
            .fold(0.0, f32::max);
        if notes.len() > 1 {
            for (note, duration, delay) in &notes {
                self.pattern.chord_data[track].add_note_with_timing(step, *note, *duration, *delay);
            }
        }
        self.pattern.step_data[track].set(step, StepParam::Transpose, notes[0].0);
        self.pattern.step_data[track].set(step, StepParam::Duration, max_duration);
        self.pattern.step_data[track].set(
            step,
            StepParam::Delay,
            if notes.len() == 1 { notes[0].2 } else { 0.0 },
        );
        self.pattern.patterns[track].set_step_active(step, true);
    }

    pub fn drum_lane_step_duration(&self, track: usize, step: usize, pad_note: i32) -> Option<f32> {
        self.drum_lane_notes(track, step)
            .into_iter()
            .find(|(note, _, _)| note.round() as i32 == pad_note)
            .map(|(_, duration, _)| duration)
    }

    /// Set the duration of one drum-pad voice without changing the durations
    /// of simultaneous hits stored in the same polyphonic step.
    pub fn set_drum_lane_step_duration(
        &self,
        track: usize,
        step: usize,
        pad_note: i32,
        duration: f32,
    ) -> Option<f32> {
        let duration = self.set_drum_lane_step_duration_no_publish(
            track,
            step,
            pad_note,
            duration,
        )?;
        self.publish_scheduler_snapshot();
        Some(duration)
    }

    pub fn set_drum_lane_step_duration_no_publish(
        &self,
        track: usize,
        step: usize,
        pad_note: i32,
        duration: f32,
    ) -> Option<f32> {
        if track >= MAX_TRACKS || step >= MAX_STEPS {
            return None;
        }
        let duration = duration.clamp(StepParam::Duration.min(), StepParam::Duration.max());
        let mut notes = self.drum_lane_notes(track, step);
        let (_, note_duration, _) = notes
            .iter_mut()
            .find(|(note, _, _)| note.round() as i32 == pad_note)?;
        *note_duration = duration;
        self.write_drum_lane_notes(track, step, notes);
        Some(duration)
    }

    /// Toggle one pitch lane within a polyphonic step. Drum-rack lanes are a
    /// projection of the existing step/chord representation: a single hit is
    /// stored in the step transpose field, while simultaneous hits are stored
    /// in chord data. Removing the final lane clears the complete step payload,
    /// matching the normal step-toggle behavior.
    pub fn toggle_drum_lane_step(&self, track: usize, step: usize, pad_note: i32) -> bool {
        let activated = self.toggle_drum_lane_step_no_publish(track, step, pad_note);
        self.publish_scheduler_snapshot();
        activated
    }

    pub fn toggle_drum_lane_step_no_publish(
        &self,
        track: usize,
        step: usize,
        pad_note: i32,
    ) -> bool {
        if track >= MAX_TRACKS || step >= MAX_STEPS {
            return false;
        }

        let transpose = pad_note as f32;
        let step_duration = self.pattern.step_data[track]
            .get(step, StepParam::Duration)
            .max(0.0);
        let step_delay = self.pattern.step_data[track].get(step, StepParam::Delay);
        let mut notes = self.drum_lane_notes(track, step);

        let existing = notes
            .iter()
            .position(|(note, _, _)| note.round() as i32 == pad_note);
        let activated = if let Some(index) = existing {
            notes.remove(index);
            false
        } else if notes.len() < MAX_VOICES {
            notes.push((transpose, step_duration, step_delay));
            true
        } else {
            return false;
        };

        self.write_drum_lane_notes(track, step, notes);
        activated
    }

    /// Move one or more hits in a single drum-pad lane without disturbing
    /// simultaneous hits belonging to other pads. Destination hits in this
    /// lane are replaced, matching the overwrite behavior of normal step drag.
    pub fn move_drum_lane_steps(
        &self,
        track: usize,
        pad_note: i32,
        steps: &[usize],
        delta: isize,
    ) -> bool {
        let moved = self.move_drum_lane_steps_no_publish(track, pad_note, steps, delta);
        if moved {
            self.publish_scheduler_snapshot();
        }
        moved
    }

    pub fn move_drum_lane_steps_no_publish(
        &self,
        track: usize,
        pad_note: i32,
        steps: &[usize],
        delta: isize,
    ) -> bool {
        if track >= MAX_TRACKS || delta == 0 || steps.is_empty() {
            return false;
        }
        let mut sources = steps.to_vec();
        sources.sort_unstable();
        sources.dedup();
        if sources.iter().any(|step| *step >= MAX_STEPS) {
            return false;
        }
        let destinations = sources
            .iter()
            .map(|step| *step as isize + delta)
            .collect::<Vec<_>>();
        if destinations
            .iter()
            .any(|step| *step < 0 || *step >= MAX_STEPS as isize)
        {
            return false;
        }

        let moved = sources
            .iter()
            .filter_map(|step| {
                let notes = self.drum_lane_notes(track, *step);
                notes
                    .iter()
                    .find(|(note, _, _)| note.round() as i32 == pad_note)
                    .copied()
                    .map(|note| {
                        (
                            *step,
                            note,
                            (notes.len() == 1).then(|| self.capture_step_snapshot(track, *step)),
                        )
                    })
            })
            .collect::<Vec<_>>();
        if moved.is_empty() {
            return false;
        }

        for (step, _, _) in &moved {
            let notes = self
                .drum_lane_notes(track, *step)
                .into_iter()
                .filter(|(note, _, _)| note.round() as i32 != pad_note)
                .collect();
            self.write_drum_lane_notes(track, *step, notes);
        }
        for (step, note, exclusive_snapshot) in moved {
            let destination = (step as isize + delta) as usize;
            let mut notes = self
                .drum_lane_notes(track, destination)
                .into_iter()
                .filter(|(existing, _, _)| existing.round() as i32 != pad_note)
                .collect::<Vec<_>>();
            if notes.is_empty() {
                if let Some(snapshot) = exclusive_snapshot {
                    self.restore_step_snapshot_inner(track, destination, &snapshot);
                    continue;
                }
            }
            notes.push(note);
            self.write_drum_lane_notes(track, destination, notes);
        }
        true
    }

    /// Clear selected hits from one drum-pad lane while retaining every other
    /// pad hit and the shared payload of steps that remain active.
    pub fn clear_drum_lane_steps(&self, track: usize, pad_note: i32, steps: &[usize]) -> usize {
        let cleared = self.clear_drum_lane_steps_no_publish(track, pad_note, steps);
        if cleared > 0 {
            self.publish_scheduler_snapshot();
        }
        cleared
    }

    pub fn clear_drum_lane_steps_no_publish(
        &self,
        track: usize,
        pad_note: i32,
        steps: &[usize],
    ) -> usize {
        if track >= MAX_TRACKS {
            return 0;
        }
        let mut cleared = 0;
        for step in steps.iter().copied().filter(|step| *step < MAX_STEPS) {
            let notes = self.drum_lane_notes(track, step);
            let retained = notes
                .iter()
                .copied()
                .filter(|(note, _, _)| note.round() as i32 != pad_note)
                .collect::<Vec<_>>();
            if retained.len() != notes.len() {
                self.write_drum_lane_notes(track, step, retained);
                cleared += 1;
            }
        }
        cleared
    }

    pub fn capture_step_snapshot(&self, track: usize, step: usize) -> StepSnapshot {
        let mut params = [0.0; NUM_PARAMS];
        for param in StepParam::ALL {
            params[param.index()] = self.pattern.step_data[track].get(step, param);
        }

        let chord_count = self.pattern.chord_data[track].count(step);
        let mut chord = Vec::with_capacity(chord_count);
        let mut chord_durations = Vec::with_capacity(chord_count);
        let mut chord_delays = Vec::with_capacity(chord_count);
        for note_idx in 0..chord_count {
            chord.push(self.pattern.chord_data[track].get(step, note_idx));
            chord_durations.push(self.pattern.chord_data[track].get_duration(step, note_idx));
            chord_delays.push(self.pattern.chord_data[track].get_delay(step, note_idx));
        }

        let midi_fx_plocks = self.pattern.midi_fx_slots[track]
            .iter()
            .map(|slot| capture_live_slot_step_plocks(slot, step))
            .collect();
        let effect_plocks = self.pattern.effect_chains[track]
            .iter()
            .map(|slot| capture_live_slot_step_plocks(slot, step))
            .collect();
        let instrument_plocks =
            capture_live_slot_step_plocks(&self.pattern.instrument_slots[track], step);
        let (
            rack_macro_plocks,
            rack_slot_param_plocks,
            rack_slot_instrument_plocks,
            rack_slot_effect_plocks,
        ) = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(|rack| rack.as_ref())
            .map(|rack| {
                let macro_plocks = rack
                    .macros
                    .iter()
                    .map(|rack_macro| rack_macro.plocks.get(step).copied().flatten())
                    .collect();
                let slot_params = rack
                    .slots
                    .iter()
                    .map(|slot| {
                        let params = RackSlotParam::ALL
                            .iter()
                            .map(|param| slot.param_plocks.get(step, *param))
                            .collect();
                        StepSlotPlocks {
                            params,
                            tensor_params: Vec::new(),
                        }
                    })
                    .collect();
                let instrument_params = rack
                    .slots
                    .iter()
                    .map(|slot| capture_snapshot_slot_step_plocks(&slot.instrument_slot, step))
                    .collect();
                let effect_params = rack
                    .slots
                    .iter()
                    .map(|slot| {
                        slot.effect_slots
                            .iter()
                            .map(|effect| capture_snapshot_slot_step_plocks(effect, step))
                            .collect()
                    })
                    .collect();
                (macro_plocks, slot_params, instrument_params, effect_params)
            })
            .unwrap_or_default();

        StepSnapshot {
            active: self.pattern.patterns[track].is_active(step),
            neural_reset: self.pattern.neural_reset_patterns[track].is_active(step),
            params,
            chord,
            chord_durations,
            chord_delays,
            timebase: self.pattern.timebase_plocks[track].get(step),
            swing: self.pattern.swing_plocks[track].get(step),
            swing_resolution: self.pattern.swing_resolution_plocks[track].get(step),
            midi_fx_plocks,
            effect_plocks,
            instrument_plocks,
            rack_macro_plocks,
            rack_slot_param_plocks,
            rack_slot_instrument_plocks,
            rack_slot_effect_plocks,
        }
    }

    /// Capture step cells from one stable Track Pattern target.
    ///
    /// The live lanes are authoritative only when `pattern_id` is currently
    /// effective. Inactive targets are read directly from their pattern pool.
    pub(crate) fn capture_pattern_step_cells(
        &self,
        track: usize,
        pattern_id: PatternId,
        steps: &[usize],
    ) -> Result<(Vec<StepSnapshot>, PlockVariantRegistry), String> {
        if steps.iter().any(|step| *step >= MAX_STEPS) {
            return Err("step target is out of range".to_string());
        }
        let is_effective = {
            let scenes = self.pattern.scenes.lock().unwrap();
            let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
            if !is_effective {
                let data = scenes
                    .track_pools
                    .get(track)
                    .and_then(|pool| pool.get(pattern_id))
                    .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
                let cells = steps
                    .iter()
                    .map(|step| {
                        data.capture_step_snapshot(*step)
                            .ok_or_else(|| "stored step target is out of range".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok((cells, data.plock_variant_registry.clone()));
            }
            is_effective
        };

        if is_effective {
            if track >= self.pattern.patterns.len() {
                return Err("live track target no longer exists".to_string());
            }
            let cells = steps
                .iter()
                .map(|step| self.capture_step_snapshot(track, *step))
                .collect();
            let registry = self
                .pattern
                .plock_variant_registries
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .ok_or_else(|| "live p-lock variant registry is missing".to_string())?;
            Ok((cells, registry))
        } else {
            unreachable!("inactive Track Pattern capture returned while holding repository")
        }
    }

    pub(crate) fn capture_pattern_num_steps(
        &self,
        track: usize,
        pattern_id: PatternId,
    ) -> Result<usize, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if scenes.effective_pattern_id(track) == Some(pattern_id) {
            return self
                .pattern
                .track_params
                .get(track)
                .map(TrackParams::get_num_steps)
                .ok_or_else(|| "live track target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .map(|data| data.track_params.num_steps)
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())
    }

    pub(crate) fn capture_pattern_track_params(
        &self,
        track: usize,
        pattern_id: PatternId,
    ) -> Result<TrackParamsSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if scenes.effective_pattern_id(track) == Some(pattern_id) {
            return self
                .pattern
                .track_params
                .get(track)
                .map(capture_track_params_snapshot)
                .ok_or_else(|| "live track target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .map(|data| data.track_params.clone())
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())
    }

    pub(crate) fn restore_pattern_track_params_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        snapshot: &TrackParamsSnapshot,
    ) -> Result<bool, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        data.track_params = snapshot.clone();
        if is_effective {
            let live = self
                .pattern
                .track_params
                .get(track)
                .ok_or_else(|| "live track target no longer exists".to_string())?;
            restore_track_params_snapshot(live, snapshot);
        }
        Ok(is_effective)
    }

    pub(crate) fn capture_pattern_instrument_base_note_offset(
        &self,
        track: usize,
        pattern_id: PatternId,
    ) -> Result<f32, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if self.mirror_device_pattern_id(track, &scenes) == Some(pattern_id) {
            return self
                .pattern
                .instrument_base_note_offsets
                .get(track)
                .map(|value| f32::from_bits(value.load(Ordering::Relaxed)))
                .ok_or_else(|| "live track target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .map(|data| data.instrument_base_note_offset)
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())
    }

    pub(crate) fn restore_pattern_instrument_base_note_offset_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        value: f32,
    ) -> Result<bool, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = self.mirror_device_pattern_id(track, &scenes) == Some(pattern_id);
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        data.instrument_base_note_offset = value;
        if is_effective {
            self.pattern
                .instrument_base_note_offsets
                .get(track)
                .ok_or_else(|| "live track target no longer exists".to_string())?
                .store(value.to_bits(), Ordering::Relaxed);
        }
        Ok(is_effective)
    }

    pub(crate) fn capture_pattern_instrument_device_values(
        &self,
        track: usize,
        pattern_id: PatternId,
    ) -> Result<InstrumentDeviceValuesSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if self.mirror_device_pattern_id(track, &scenes) == Some(pattern_id) {
            let slot = self
                .pattern
                .instrument_slots
                .get(track)
                .ok_or_else(|| "live instrument target no longer exists".to_string())?;
            let base_note_offset_bits = self
                .pattern
                .instrument_base_note_offsets
                .get(track)
                .ok_or_else(|| "live instrument base note is missing".to_string())?
                .load(Ordering::Relaxed);
            let sound_state = self
                .pattern
                .track_sound_state
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .ok_or_else(|| "live instrument sound state is missing".to_string())?;
            let key_lock_variant_registry = self
                .pattern
                .key_lock_variant_registries
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .ok_or_else(|| "live key-lock variant registry is missing".to_string())?;
            return Ok(InstrumentDeviceValuesSnapshot {
                slot: EffectSlotSnapshot::capture_authoring_values(slot),
                base_note_offset_bits,
                sound_state,
                key_lock_variant_registry,
            });
        }
        let data = scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        Ok(InstrumentDeviceValuesSnapshot {
            slot: data.instrument_slot.authoring_values(),
            base_note_offset_bits: data.instrument_base_note_offset.to_bits(),
            sound_state: data.track_sound_state.clone(),
            key_lock_variant_registry: data.key_lock_variant_registry.clone(),
        })
    }

    pub(crate) fn restore_pattern_instrument_device_values_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        values: &InstrumentDeviceValuesSnapshot,
    ) -> Result<bool, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = self.mirror_device_pattern_id(track, &scenes) == Some(pattern_id);
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        let mut stored_slot = data.instrument_slot.clone();
        if let Err(error) = stored_slot.apply_authoring_values(&values.slot) {
            if !is_effective {
                return Err(error);
            }
            // The pool copy can predate the current instrument descriptor
            // (older saved project, e.g. the sampler grew params). The values
            // being applied were captured from the live slot, so reseed the
            // stored layout from it instead of failing the edit forever.
            let slot = self
                .pattern
                .instrument_slots
                .get(track)
                .ok_or_else(|| "live instrument target no longer exists".to_string())?;
            stored_slot = EffectSlotSnapshot::capture(slot);
            stored_slot.apply_authoring_values(&values.slot)?;
        }
        let live_slot = if is_effective {
            let slot = self
                .pattern
                .instrument_slots
                .get(track)
                .ok_or_else(|| "live instrument target no longer exists".to_string())?;
            let mut snapshot = EffectSlotSnapshot::capture(slot);
            snapshot.apply_authoring_values(&values.slot)?;
            Some((slot, snapshot))
        } else {
            None
        };

        data.instrument_slot = stored_slot;
        data.instrument_base_note_offset = f32::from_bits(values.base_note_offset_bits);
        data.track_sound_state = values.sound_state.clone();
        data.key_lock_variant_registry = values.key_lock_variant_registry.clone();
        if let Some((slot, snapshot)) = live_slot {
            snapshot.restore(slot);
            self.pattern
                .instrument_base_note_offsets
                .get(track)
                .ok_or_else(|| "live instrument base note is missing".to_string())?
                .store(values.base_note_offset_bits, Ordering::Relaxed);
            *self
                .pattern
                .track_sound_state
                .lock()
                .unwrap()
                .get_mut(track)
                .ok_or_else(|| "live instrument sound state is missing".to_string())? =
                values.sound_state.clone();
            *self
                .pattern
                .key_lock_variant_registries
                .lock()
                .unwrap()
                .get_mut(track)
                .ok_or_else(|| "live key-lock variant registry is missing".to_string())? =
                values.key_lock_variant_registry.clone();
        }
        Ok(is_effective)
    }

    pub(crate) fn capture_pattern_effect_device_values(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
    ) -> Result<EffectSlotValuesSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if self.mirror_device_pattern_id(track, &scenes) == Some(pattern_id) {
            return self
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
                .map(EffectSlotSnapshot::capture_authoring_values)
                .ok_or_else(|| "live effect target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .and_then(|data| data.effect_slots.get(slot_idx))
            .map(EffectSlotSnapshot::authoring_values)
            .ok_or_else(|| "Track Pattern effect target no longer exists".to_string())
    }

    pub(crate) fn capture_track_effect_chain_values(
        &self,
        track: usize,
        first_slot: usize,
        slot_count: usize,
    ) -> Result<Vec<(PatternId, Vec<EffectSlotValuesSnapshot>)>, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        pool.patterns
            .iter()
            .map(|(pattern, data)| {
                let slots = if *pattern == effective {
                    self.pattern
                        .effect_chains
                        .get(track)
                        .ok_or_else(|| "live effect chain is missing".to_string())?
                        .iter()
                        .skip(first_slot)
                        .take(slot_count)
                        .map(EffectSlotSnapshot::capture_authoring_values)
                        .collect()
                } else {
                    data.effect_slots
                        .iter()
                        .skip(first_slot)
                        .take(slot_count)
                        .map(EffectSlotSnapshot::authoring_values)
                        .collect()
                };
                Ok((*pattern, slots))
            })
            .collect()
    }

    pub(crate) fn restore_track_effect_chain_values(
        &self,
        track: usize,
        first_slot: usize,
        descriptors: &[EffectDescriptor],
        node_ids: &[(u32, u32)],
        patterns: &[(PatternId, Vec<EffectSlotValuesSnapshot>)],
    ) -> Result<(), String> {
        if descriptors.len() != node_ids.len() {
            return Err("effect-chain descriptor/node layout mismatch".to_string());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != patterns.len()
            || patterns.iter().any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err(format!(
                "Track {} pattern set changed before effect history replay",
                track + 1
            ));
        }
        for (pattern, values) in patterns {
            if values.len() != descriptors.len() {
                return Err("effect-chain pattern layout mismatch".to_string());
            }
            let data = pool.patterns.get_mut(pattern).expect("pattern set was validated");
            for (offset, ((descriptor, (node_id, modulator_node_id)), values)) in descriptors
                .iter()
                .zip(node_ids)
                .zip(values)
                .enumerate()
            {
                let slot = data
                    .effect_slots
                    .get_mut(first_slot + offset)
                    .ok_or_else(|| "stored effect slot is missing".to_string())?;
                slot.sync_to_descriptor_with_modulator(
                    descriptor,
                    *node_id,
                    *modulator_node_id,
                );
                slot.apply_authoring_values(values)?;
            }
        }
        let live_values = patterns
            .iter()
            .find(|(pattern, _)| *pattern == effective)
            .map(|(_, values)| values)
            .ok_or_else(|| "effective effect pattern is missing from history".to_string())?;
        let live_chain = self
            .pattern
            .effect_chains
            .get(track)
            .ok_or_else(|| "live effect chain is missing".to_string())?;
        for (offset, ((descriptor, (node_id, modulator_node_id)), values)) in descriptors
            .iter()
            .zip(node_ids)
            .zip(live_values)
            .enumerate()
        {
            let slot = live_chain
                .get(first_slot + offset)
                .ok_or_else(|| "live effect slot is missing".to_string())?;
            let mut snapshot = EffectSlotSnapshot::capture(slot);
            snapshot.sync_to_descriptor_with_modulator(
                descriptor,
                *node_id,
                *modulator_node_id,
            );
            snapshot.apply_authoring_values(values)?;
            snapshot.restore(slot);
        }
        Ok(())
    }

    pub(crate) fn capture_track_effect_binding_state(
        &self,
        track: usize,
    ) -> Result<TrackEffectBindingStateSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let live_chain = self
            .pattern
            .process_chains
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .ok_or_else(|| "live process chain is missing".to_string())?;
        let live_lane_overrides = self
            .pattern
            .project_process_lane_overrides
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .ok_or_else(|| "live project process lane overrides are missing".to_string())?;
        let process_chains = pool
            .patterns
            .iter()
            .map(|(id, data)| {
                (*id, if *id == effective { live_chain.clone() } else { data.process_chain.clone() })
            })
            .collect();
        let project_process_lane_overrides = pool
            .patterns
            .iter()
            .map(|(id, data)| {
                (
                    *id,
                    if *id == effective {
                        live_lane_overrides.clone()
                    } else {
                        data.project_process_lane_overrides.clone()
                    },
                )
            })
            .collect();
        let mut neural_overrides = Vec::new();
        for (scene_idx, scene) in scenes.scenes.iter().enumerate() {
            for (network_idx, network) in scene.neural_networks.iter().enumerate() {
                for (neuron_idx, neuron) in network.neurons.iter().enumerate() {
                    let entries = neuron
                        .output_overrides
                        .effects
                        .iter()
                        .enumerate()
                        .filter(|(_, value)| value.target_track == track)
                        .map(|(index, value)| (index, value.clone()))
                        .collect::<Vec<_>>();
                    if !entries.is_empty() {
                        neural_overrides.push(NeuralEffectOverrideState {
                            scene: scene_idx,
                            network: network_idx,
                            neuron: neuron_idx,
                            entries,
                        });
                    }
                }
            }
        }
        Ok(TrackEffectBindingStateSnapshot {
            process_chains,
            project_process_lane_overrides,
            neural_overrides,
        })
    }

    pub(crate) fn remap_track_effect_references(
        &self,
        track: usize,
        old_to_new: &[Option<usize>],
        drop_neural_slots: &[bool],
        effect_descriptors: &[EffectDescriptor],
    ) -> Result<(), String> {
        fn remap_chain(
            chain: &mut crate::process::TrackProcessChain,
            old_to_new: &[Option<usize>],
        ) {
            for process_slot in &mut chain.slots {
                for binding in process_slot.bindings.values_mut() {
                    let Some(crate::process::ParamTarget::EffectParam { slot, .. }) = binding.as_mut() else {
                        continue;
                    };
                    match old_to_new.get(*slot).copied().flatten() {
                        Some(new_slot) => *slot = new_slot,
                        None => *binding = None,
                    }
                }
            }
        }

        let live_effect_slots = self
            .pattern
            .effect_chains
            .get(track)
            .ok_or_else(|| "live effect chain is missing".to_string())?;
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        for data in pool.patterns.values_mut() {
            remap_chain(&mut data.process_chain, old_to_new);
            crate::process::rebind_track_process_chain_effect_param_ids(
                &mut data.process_chain,
                effect_descriptors,
                &data.effect_slots,
            );
        }
        for scene in &mut scenes.scenes {
            for network in &mut scene.neural_networks {
                for neuron in &mut network.neurons {
                    neuron.output_overrides.effects.retain_mut(|value| {
                        if value.target_track != track {
                            return true;
                        }
                        if drop_neural_slots
                            .get(value.slot_index)
                            .copied()
                            .unwrap_or(true)
                        {
                            return false;
                        }
                        let Some(new_slot) = old_to_new
                            .get(value.slot_index)
                            .copied()
                            .flatten()
                        else {
                            return false;
                        };
                        value.slot_index = new_slot;
                        let Some(slot) = live_effect_slots.get(new_slot) else {
                            return false;
                        };
                        let Some(raw_idx) = slot
                            .param_node_indices
                            .get(value.param_index)
                            .map(|value| value.load(Ordering::Relaxed))
                        else {
                            return false;
                        };
                        let Some(param_id) = crate::neural::ParamNodeId::from_slot_param(
                            slot.node_id.load(Ordering::Relaxed),
                            slot.modulator_node_id.load(Ordering::Relaxed),
                            raw_idx,
                        ) else {
                            return false;
                        };
                        value.param_id = param_id;
                        true
                    });
                }
            }
        }
        drop(scenes);

        let mut live_chains = self.pattern.process_chains.lock().unwrap();
        let live_chain = live_chains
            .get_mut(track)
            .ok_or_else(|| "live process chain is missing".to_string())?;
        remap_chain(live_chain, old_to_new);
        let live_slots = live_effect_slots
            .iter()
            .map(EffectSlotSnapshot::capture)
            .collect::<Vec<_>>();
        crate::process::rebind_track_process_chain_effect_param_ids(
            live_chain,
            effect_descriptors,
            &live_slots,
        );
        Ok(())
    }

    pub(crate) fn restore_track_effect_binding_state(
        &self,
        track: usize,
        snapshot: &TrackEffectBindingStateSnapshot,
        effect_descriptors: &[EffectDescriptor],
    ) -> Result<(), String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.process_chains.len()
            || snapshot.process_chains.iter().any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err(format!(
                "Track {} pattern set changed before effect binding replay",
                track + 1
            ));
        }
        if snapshot.project_process_lane_overrides.len() != snapshot.process_chains.len()
            || snapshot
                .project_process_lane_overrides
                .iter()
                .any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err("effect history project-lane pattern set changed".to_string());
        }
        let mut live_chain = None;
        let mut live_lane_overrides = None;
        for (id, saved_chain) in &snapshot.process_chains {
            let data = pool.patterns.get_mut(id).expect("pattern set was validated");
            let mut chain = saved_chain.clone();
            crate::process::refresh_track_process_chain_binding_param_ids(
                &mut chain,
                None,
                None,
                effect_descriptors,
                &data.effect_slots,
            );
            data.process_chain = chain.clone();
            if *id == effective {
                live_chain = Some(chain);
            }
        }
        for (id, saved) in &snapshot.project_process_lane_overrides {
            pool.patterns
                .get_mut(id)
                .expect("pattern set was validated")
                .project_process_lane_overrides = saved.clone();
            if *id == effective {
                live_lane_overrides = Some(saved.clone());
            }
        }
        for scene in &mut scenes.scenes {
            for network in &mut scene.neural_networks {
                for neuron in &mut network.neurons {
                    neuron
                        .output_overrides
                        .effects
                        .retain(|value| value.target_track != track);
                }
            }
        }
        let live_slots = self
            .pattern
            .effect_chains
            .get(track)
            .ok_or_else(|| "live effect chain is missing".to_string())?;
        for saved in &snapshot.neural_overrides {
            let neuron = scenes
                .scenes
                .get_mut(saved.scene)
                .and_then(|scene| scene.neural_networks.get_mut(saved.network))
                .and_then(|network| network.neurons.get_mut(saved.neuron))
                .ok_or_else(|| {
                    format!(
                        "Track {} neural topology changed before effect history replay",
                        track + 1
                    )
                })?;
            for (index, value) in &saved.entries {
                let mut value = value.clone();
                let slot = live_slots
                    .get(value.slot_index)
                    .ok_or_else(|| "neural effect slot is out of range".to_string())?;
                let raw_idx = slot
                    .param_node_indices
                    .get(value.param_index)
                    .map(|value| value.load(Ordering::Relaxed))
                    .ok_or_else(|| "neural effect parameter is out of range".to_string())?;
                value.param_id = crate::neural::ParamNodeId::from_slot_param(
                    slot.node_id.load(Ordering::Relaxed),
                    slot.modulator_node_id.load(Ordering::Relaxed),
                    raw_idx,
                )
                .ok_or_else(|| "neural effect parameter has no live identity".to_string())?;
                neuron
                    .output_overrides
                    .effects
                    .insert((*index).min(neuron.output_overrides.effects.len()), value);
            }
        }
        drop(scenes);
        self.pattern.process_chains.lock().unwrap()[track] = live_chain
            .ok_or_else(|| "effective process chain is missing from history".to_string())?;
        self.pattern.project_process_lane_overrides.lock().unwrap()[track] = live_lane_overrides
            .ok_or_else(|| "effective project process lanes are missing from history".to_string())?;
        Ok(())
    }

    pub(crate) fn restore_pattern_effect_device_values_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
        values: &EffectSlotValuesSnapshot,
    ) -> Result<bool, String> {
        self.restore_pattern_slot_device_values_no_publish(
            track,
            pattern_id,
            slot_idx,
            values,
            false,
        )
    }

    pub(crate) fn capture_pattern_midi_fx_device_values(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
    ) -> Result<EffectSlotValuesSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if self.mirror_device_pattern_id(track, &scenes) == Some(pattern_id) {
            return self
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx))
                .map(EffectSlotSnapshot::capture_authoring_values)
                .ok_or_else(|| "live MIDI-FX target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .and_then(|data| data.midi_fx_slots.get(slot_idx))
            .map(EffectSlotSnapshot::authoring_values)
            .ok_or_else(|| "Track Pattern MIDI-FX target no longer exists".to_string())
    }

    pub(crate) fn capture_track_midi_fx_chain_values(
        &self,
        track: usize,
    ) -> Result<Vec<(PatternId, Vec<EffectSlotValuesSnapshot>)>, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        pool.patterns
            .iter()
            .map(|(pattern, data)| {
                let slots = if *pattern == effective {
                    self.pattern
                        .midi_fx_slots
                        .get(track)
                        .ok_or_else(|| "live MIDI-FX chain is missing".to_string())?
                        .iter()
                        .map(EffectSlotSnapshot::capture_authoring_values)
                        .collect()
                } else {
                    data.midi_fx_slots
                        .iter()
                        .map(EffectSlotSnapshot::authoring_values)
                        .collect()
                };
                Ok((*pattern, slots))
            })
            .collect()
    }

    pub(crate) fn restore_track_midi_fx_chain_values(
        &self,
        track: usize,
        names: &[String],
        descriptors: &[EffectDescriptor],
        patterns: &[(PatternId, Vec<EffectSlotValuesSnapshot>)],
    ) -> Result<(), String> {
        if names.len() != descriptors.len() || descriptors.len() > crate::lisp_host::MAX_MIDI_FX_SLOTS {
            return Err("MIDI-FX history layout is invalid".to_string());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != patterns.len()
            || patterns.iter().any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err(format!(
                "Track {} pattern set changed before MIDI-FX history replay",
                track + 1
            ));
        }
        for (pattern, values) in patterns {
            if values.len() != crate::lisp_host::MAX_MIDI_FX_SLOTS {
                return Err("MIDI-FX pattern layout is invalid".to_string());
            }
            let data = pool.patterns.get_mut(pattern).expect("pattern set was validated");
            data.track_params.midi_fx_chain = names.to_vec();
            for slot_idx in 0..crate::lisp_host::MAX_MIDI_FX_SLOTS {
                let descriptor = descriptors
                    .get(slot_idx)
                    .cloned()
                    .unwrap_or_else(EffectDescriptor::empty_custom_slot);
                let slot = data
                    .midi_fx_slots
                    .get_mut(slot_idx)
                    .ok_or_else(|| "stored MIDI-FX slot is missing".to_string())?;
                slot.sync_to_descriptor(&descriptor, 0);
                slot.apply_authoring_values(&values[slot_idx])?;
            }
        }
        let live_values = patterns
            .iter()
            .find(|(pattern, _)| *pattern == effective)
            .map(|(_, values)| values)
            .ok_or_else(|| "effective MIDI-FX pattern is missing from history".to_string())?;
        self.pattern.track_params[track].set_midi_fx_chain(names.to_vec());
        for slot_idx in 0..crate::lisp_host::MAX_MIDI_FX_SLOTS {
            let descriptor = descriptors
                .get(slot_idx)
                .cloned()
                .unwrap_or_else(EffectDescriptor::empty_custom_slot);
            let slot = self.pattern.midi_fx_slots[track]
                .get(slot_idx)
                .ok_or_else(|| "live MIDI-FX slot is missing".to_string())?;
            let mut snapshot = EffectSlotSnapshot::capture(slot);
            snapshot.sync_to_descriptor(&descriptor, 0);
            snapshot.apply_authoring_values(&live_values[slot_idx])?;
            snapshot.restore(slot);
        }
        Ok(())
    }

    pub(crate) fn remap_track_midi_fx_references(
        &self,
        track: usize,
        old_to_new: &[Option<usize>],
    ) -> Result<(), String> {
        fn remap_chain(
            chain: &mut crate::process::TrackProcessChain,
            old_to_new: &[Option<usize>],
        ) {
            for process_slot in &mut chain.slots {
                for binding in process_slot.bindings.values_mut() {
                    let Some(crate::process::ParamTarget::MidiFxParam { slot, .. }) = binding.as_mut() else {
                        continue;
                    };
                    match old_to_new.get(*slot).copied().flatten() {
                        Some(new_slot) => *slot = new_slot,
                        None => *binding = None,
                    }
                }
            }
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        for data in pool.patterns.values_mut() {
            remap_chain(&mut data.process_chain, old_to_new);
        }
        drop(scenes);
        let mut chains = self.pattern.process_chains.lock().unwrap();
        let chain = chains
            .get_mut(track)
            .ok_or_else(|| "live process chain is missing".to_string())?;
        remap_chain(chain, old_to_new);
        Ok(())
    }

    pub(crate) fn capture_track_process_chains(
        &self,
        track: usize,
    ) -> Result<Vec<(PatternId, crate::process::TrackProcessChain)>, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let live = self
            .pattern
            .process_chains
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .ok_or_else(|| "live process chain is missing".to_string())?;
        Ok(pool
            .patterns
            .iter()
            .map(|(id, data)| {
                (*id, if *id == effective { live.clone() } else { data.process_chain.clone() })
            })
            .collect())
    }

    pub(crate) fn restore_track_process_chains(
        &self,
        track: usize,
        saved: &[(PatternId, crate::process::TrackProcessChain)],
    ) -> Result<(), String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != saved.len()
            || saved.iter().any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err("process pattern set changed before history replay".to_string());
        }
        let mut live = None;
        for (id, chain) in saved {
            pool.patterns
                .get_mut(id)
                .expect("pattern set was validated")
                .process_chain = chain.clone();
            if *id == effective {
                live = Some(chain.clone());
            }
        }
        drop(scenes);
        self.pattern.process_chains.lock().unwrap()[track] = live
            .ok_or_else(|| "effective process chain is missing from history".to_string())?;
        Ok(())
    }

    pub(crate) fn restore_pattern_midi_fx_device_values_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
        values: &EffectSlotValuesSnapshot,
    ) -> Result<bool, String> {
        self.restore_pattern_slot_device_values_no_publish(
            track,
            pattern_id,
            slot_idx,
            values,
            true,
        )
    }

    fn restore_pattern_slot_device_values_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
        values: &EffectSlotValuesSnapshot,
        midi_fx: bool,
    ) -> Result<bool, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = self.mirror_device_pattern_id(track, &scenes) == Some(pattern_id);
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        let stored = if midi_fx {
            data.midi_fx_slots.get_mut(slot_idx)
        } else {
            data.effect_slots.get_mut(slot_idx)
        }
        .ok_or_else(|| "stored device target no longer exists".to_string())?;
        let mut stored_next = stored.clone();
        stored_next.apply_authoring_values(values)?;
        let live = if is_effective {
            let slot = if midi_fx {
                self.pattern
                    .midi_fx_slots
                    .get(track)
                    .and_then(|slots| slots.get(slot_idx))
            } else {
                self.pattern
                    .effect_chains
                    .get(track)
                    .and_then(|slots| slots.get(slot_idx))
            }
            .ok_or_else(|| "live device target no longer exists".to_string())?;
            let mut snapshot = EffectSlotSnapshot::capture(slot);
            snapshot.apply_authoring_values(values)?;
            Some((slot, snapshot))
        } else {
            None
        };
        *stored = stored_next;
        if let Some((slot, snapshot)) = live {
            snapshot.restore(slot);
        }
        Ok(is_effective)
    }

    pub(crate) fn capture_pattern_rack_slot_values(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
    ) -> Result<RackSlotValuesSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if self.mirror_device_pattern_id(track, &scenes) == Some(pattern_id) {
            return self
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .and_then(Option::as_ref)
                .and_then(|rack| rack.slots.get(slot_idx))
                .map(RackSlotSnapshot::authoring_values)
                .ok_or_else(|| "live rack slot target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .and_then(|data| data.rack_track.as_ref())
            .and_then(|rack| rack.slots.get(slot_idx))
            .map(RackSlotSnapshot::authoring_values)
            .ok_or_else(|| "Track Pattern rack slot target no longer exists".to_string())
    }

    pub(crate) fn restore_pattern_rack_slot_values_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
        values: &RackSlotValuesSnapshot,
    ) -> Result<bool, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = self.mirror_device_pattern_id(track, &scenes) == Some(pattern_id);
        let stored = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .and_then(|data| data.rack_track.as_mut())
            .and_then(|rack| rack.slots.get_mut(slot_idx))
            .ok_or_else(|| "Track Pattern rack slot target no longer exists".to_string())?;
        let mut stored_next = stored.clone();
        stored_next.apply_authoring_values(values)?;
        let live_next = if is_effective {
            let racks = self.pattern.rack_tracks.lock().unwrap();
            let live = racks
                .get(track)
                .and_then(Option::as_ref)
                .and_then(|rack| rack.slots.get(slot_idx))
                .ok_or_else(|| "live rack slot target no longer exists".to_string())?;
            let mut snapshot = live.clone();
            snapshot.apply_authoring_values(values)?;
            Some(snapshot)
        } else {
            None
        };
        *stored = stored_next;
        if let Some(snapshot) = live_next {
            let mut racks = self.pattern.rack_tracks.lock().unwrap();
            let live = racks
                .get_mut(track)
                .and_then(Option::as_mut)
                .and_then(|rack| rack.slots.get_mut(slot_idx))
                .ok_or_else(|| "live rack slot target no longer exists".to_string())?;
            *live = snapshot;
        }
        Ok(is_effective)
    }

    pub(crate) fn restore_pattern_num_steps_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        num_steps: usize,
    ) -> Result<bool, String> {
        let num_steps = num_steps.clamp(1, MAX_STEPS);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        data.track_params.num_steps = num_steps;
        if is_effective {
            self.pattern
                .track_params
                .get(track)
                .ok_or_else(|| "live track target no longer exists".to_string())?
                .set_num_steps(num_steps);
        }
        // The lane dots project pool content, so a length change has to move
        // the pool-content revision (spec 5.2) — nothing else the commit
        // touches gates that rebuild.
        self.bump_pool_content_revision();
        Ok(is_effective)
    }

    /// Restore a stable Track Pattern step batch without publishing.
    ///
    /// The pool is always updated. The live mirror is updated only if the
    /// same pattern remains effective, so scene changes cannot redirect replay.
    pub(crate) fn restore_pattern_step_cells_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        cells: &[(usize, StepSnapshot)],
        variant_registry: &PlockVariantRegistry,
    ) -> Result<bool, String> {
        if cells.iter().any(|(step, _)| *step >= MAX_STEPS) {
            return Err("step target is out of range".to_string());
        }
        let initially_effective = {
            let scenes = self.pattern.scenes.lock().unwrap();
            if scenes
                .track_pools
                .get(track)
                .and_then(|pool| pool.get(pattern_id))
                .is_none()
            {
                return Err("Track Pattern target no longer exists".to_string());
            }
            scenes.effective_pattern_id(track) == Some(pattern_id)
        };
        if initially_effective {
            self.validate_live_step_cell_target(track)?;
        }
        let is_effective = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
            if is_effective && !initially_effective {
                return Err("Track Pattern became active during step replay".to_string());
            }
            let data = scenes
                .track_pools
                .get_mut(track)
                .and_then(|pool| pool.get_mut(pattern_id))
                .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
            for (step, snapshot) in cells {
                if !data.restore_step_snapshot(*step, snapshot) {
                    return Err("stored step target is out of range".to_string());
                }
            }
            data.plock_variant_registry = variant_registry.clone();
            is_effective
        };

        if is_effective {
            if track >= self.pattern.patterns.len() {
                return Err("live track target no longer exists".to_string());
            }
            for (step, snapshot) in cells {
                self.restore_step_snapshot_inner(track, *step, snapshot);
            }
            let mut registries = self.pattern.plock_variant_registries.lock().unwrap();
            let registry = registries
                .get_mut(track)
                .ok_or_else(|| "live p-lock variant registry is missing".to_string())?;
            *registry = variant_registry.clone();
        }
        // Every step edit — live, undo and redo — funnels through here, so
        // this is the one place the arrangement's note dots can learn that
        // pool content moved (spec 5.2). The pool is written whether or not
        // the pattern is effective, so the bump is unconditional too.
        self.bump_pool_content_revision();
        Ok(is_effective)
    }

    fn validate_live_step_cell_target(&self, track: usize) -> Result<(), String> {
        let lanes = [
            (self.pattern.patterns.len(), "step active bits"),
            (
                self.pattern.neural_reset_patterns.len(),
                "neural-reset bits",
            ),
            (self.pattern.step_data.len(), "step parameter data"),
            (self.pattern.chord_data.len(), "chord data"),
            (self.pattern.timebase_plocks.len(), "timebase p-locks"),
            (self.pattern.swing_plocks.len(), "swing p-locks"),
            (
                self.pattern.swing_resolution_plocks.len(),
                "swing-resolution p-locks",
            ),
            (self.pattern.midi_fx_slots.len(), "MIDI-FX slots"),
            (self.pattern.effect_chains.len(), "audio-effect slots"),
            (self.pattern.instrument_slots.len(), "instrument slots"),
        ];
        if let Some((len, name)) = lanes.into_iter().find(|(len, _)| track >= *len) {
            return Err(format!(
                "live track {track} is missing from {name} (length {len})"
            ));
        }
        if track >= self.pattern.rack_tracks.lock().unwrap().len() {
            return Err("live rack-track lane is missing".to_string());
        }
        if track
            >= self
                .pattern
                .plock_variant_registries
                .lock()
                .unwrap()
                .len()
        {
            return Err("live p-lock variant registry is missing".to_string());
        }
        Ok(())
    }

    /// Blank a track's live note content (active steps, chords, per-step
    /// plocks) without touching its instrument/effect/mixer state. Used when
    /// a launched scene resolves no pattern for the track (takes spec 11.1):
    /// the step grid must present an empty pattern, not the previous
    /// scene's notes left behind in the live buffers.
    pub(crate) fn clear_live_track_note_content(&self, track: usize) {
        for step in 0..MAX_STEPS {
            self.clear_step_payload_inner(track, step);
        }
    }

    pub(crate) fn clear_step_payload_inner(&self, track: usize, step: usize) {
        for param in StepParam::ALL {
            self.pattern.step_data[track].set(step, param, param.default_value());
        }

        self.pattern.patterns[track].clear_step(step);
        self.pattern.neural_reset_patterns[track].clear_step(step);

        self.pattern.chord_data[track].clear_step(step);
        self.pattern.timebase_plocks[track].clear(step);
        self.pattern.swing_plocks[track].clear(step);
        self.pattern.swing_resolution_plocks[track].clear(step);

        for slot in &self.pattern.midi_fx_slots[track] {
            slot.plocks.clear_step(step);
            for tensor_idx in 0..slot.tensor_params.num_params() {
                slot.tensor_params.clear_plock(step, tensor_idx);
            }
        }

        for slot in &self.pattern.effect_chains[track] {
            slot.plocks.clear_step(step);
            for tensor_idx in 0..slot.tensor_params.num_params() {
                slot.tensor_params.clear_plock(step, tensor_idx);
            }
        }

        let instrument_slot = &self.pattern.instrument_slots[track];
        instrument_slot.plocks.clear_step(step);
        for tensor_idx in 0..instrument_slot.tensor_params.num_params() {
            instrument_slot.tensor_params.clear_plock(step, tensor_idx);
        }
        self.clear_rack_macro_plocks_for_step(track, step);
        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            for slot in &mut rack.slots {
                slot.param_plocks.clear_step(step);
                slot.instrument_slot.clear_step_plocks(step);
                for tensor_idx in 0..slot.instrument_slot.tensor_params.len() {
                    slot.instrument_slot.clear_tensor_plock(step, tensor_idx);
                }
                for effect in &mut slot.effect_slots {
                    effect.clear_step_plocks(step);
                    for tensor_idx in 0..effect.tensor_params.len() {
                        effect.clear_tensor_plock(step, tensor_idx);
                    }
                }
            }
        }
    }

    pub fn clear_step_payload(&self, track: usize, step: usize) {
        self.clear_step_payload_inner(track, step);
        self.publish_scheduler_snapshot();
    }

    pub fn clear_step_payload_no_publish(&self, track: usize, step: usize) {
        self.clear_step_payload_inner(track, step);
    }

    pub fn reconcile_plock_variant_registry_for_track(
        &self,
        track: usize,
    ) -> Vec<Option<PlockVariantAssignment>> {
        let keys = live_track_variant_keys(self, track);
        let mut registries = self.pattern.plock_variant_registries.lock().unwrap();
        let Some(registry) = registries.get_mut(track) else {
            return vec![None; MAX_STEPS];
        };
        registry.reconcile(keys)
    }

    pub fn plock_variant_registry_snapshot(&self, track: usize) -> PlockVariantRegistry {
        let _ = self.reconcile_plock_variant_registry_for_track(track);
        self.pattern
            .plock_variant_registries
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .unwrap_or_default()
    }

    pub fn reconcile_key_lock_variant_registry_for_track(
        &self,
        track: usize,
    ) -> Vec<Option<PlockVariantAssignment>> {
        let keys = live_track_key_lock_variant_keys(self, track);
        let mut registries = self.pattern.key_lock_variant_registries.lock().unwrap();
        let Some(registry) = registries.get_mut(track) else {
            return vec![None; crate::effects::MAX_MIDI_NOTES];
        };
        registry.reconcile(keys)
    }

    pub fn key_lock_variant_registry_snapshot(&self, track: usize) -> PlockVariantRegistry {
        let _ = self.reconcile_key_lock_variant_registry_for_track(track);
        self.pattern
            .key_lock_variant_registries
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .unwrap_or_default()
    }

    pub fn clear_key_lock_variant_locks_for_notes(&self, track: usize, notes: &[u8]) -> bool {
        if track >= self.pattern.instrument_slots.len() {
            return false;
        }
        let slot = &self.pattern.instrument_slots[track];
        let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
        let mut changed = false;
        for note in notes.iter().copied() {
            if slot.key_locks.note_has_any_lock(note, num_params) {
                slot.clear_note_key_locks(note);
                changed = true;
            }
        }
        if changed {
            let _ = self.reconcile_key_lock_variant_registry_for_track(track);
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub fn stamp_key_lock_variant_key_to_notes(
        &self,
        track: usize,
        key: &PlockVariantKey,
        notes: &[u8],
    ) -> bool {
        if track >= self.pattern.instrument_slots.len() {
            return false;
        }
        let mut changed = false;
        for note in notes.iter().copied() {
            if live_track_key_lock_variant_key(self, track, note)
                .as_ref()
                .is_some_and(|candidate| candidate == key)
            {
                continue;
            }
            let slot = &self.pattern.instrument_slots[track];
            slot.clear_note_key_locks(note);
            for entry in &key.entries {
                if entry.domain != crate::plock_variants::PlockVariantDomain::InstrumentKeyLock
                    || entry.slot != 0
                    || entry.cell.is_some()
                    || entry.param >= slot.num_params.load(Ordering::Relaxed) as usize
                {
                    continue;
                }
                slot.set_key_lock(note, entry.param, f32::from_bits(entry.value_bits));
            }
            changed = true;
        }
        if changed {
            let _ = self.reconcile_key_lock_variant_registry_for_track(track);
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub fn clear_variant_locks_for_steps(&self, track: usize, steps: &[usize]) -> bool {
        let changed = self.clear_variant_locks_for_steps_no_publish(track, steps);
        if changed {
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub fn clear_variant_locks_for_steps_no_publish(
        &self,
        track: usize,
        steps: &[usize],
    ) -> bool {
        if track >= self.pattern.instrument_slots.len() {
            return false;
        }
        let mut changed = false;
        for step in steps.iter().copied().filter(|step| *step < MAX_STEPS) {
            changed |= self.clear_variant_locks_for_step_inner(track, step);
        }
        if changed {
            let _ = self.reconcile_plock_variant_registry_for_track(track);
        }
        changed
    }

    pub fn stamp_variant_key_to_steps(
        &self,
        track: usize,
        key: &PlockVariantKey,
        steps: &[usize],
    ) -> bool {
        let changed = self.stamp_variant_key_to_steps_no_publish(track, key, steps);
        if changed {
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub fn stamp_variant_key_to_steps_no_publish(
        &self,
        track: usize,
        key: &PlockVariantKey,
        steps: &[usize],
    ) -> bool {
        let Some(source_step) = self.find_step_with_variant_key(track, key) else {
            return false;
        };
        self.copy_variant_locks_from_step_to_steps_no_publish(track, source_step, steps)
    }

    pub fn copy_variant_locks_from_step_to_steps(
        &self,
        track: usize,
        source_step: usize,
        steps: &[usize],
    ) -> bool {
        let changed = self.copy_variant_locks_from_step_to_steps_no_publish(
            track,
            source_step,
            steps,
        );
        if changed {
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub(crate) fn copy_variant_locks_from_step_to_steps_no_publish(
        &self,
        track: usize,
        source_step: usize,
        steps: &[usize],
    ) -> bool {
        if track >= self.pattern.instrument_slots.len() || source_step >= MAX_STEPS {
            return false;
        }
        let mut changed = false;
        for target_step in steps.iter().copied().filter(|step| *step < MAX_STEPS) {
            changed |= self.copy_variant_locks_between_steps_inner(track, source_step, target_step);
        }
        if changed {
            let _ = self.reconcile_plock_variant_registry_for_track(track);
        }
        changed
    }

    fn find_step_with_variant_key(&self, track: usize, key: &PlockVariantKey) -> Option<usize> {
        (0..MAX_STEPS).find(|step| {
            live_track_variant_key(self, track, *step)
                .as_ref()
                .is_some_and(|candidate| candidate == key)
        })
    }

    fn clear_rack_macro_plocks_for_step(&self, track: usize, step: usize) -> bool {
        let ids = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .map(|rack| {
                rack.macros
                    .iter()
                    .filter(|rack_macro| rack_macro.plocks.get(step).is_some_and(Option::is_some))
                    .map(|rack_macro| rack_macro.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut changed = false;
        for id in ids {
            changed |= self.update_rack_macro_in_current_pattern(track, id, |rack_macro| {
                rack_macro.plocks[step] = None;
            });
        }
        changed
    }

    fn copy_rack_macro_plocks_between_steps(
        &self,
        track: usize,
        source_step: usize,
        target_step: usize,
    ) -> bool {
        let values = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .map(|rack| {
                rack.macros
                    .iter()
                    .filter_map(|rack_macro| {
                        let source = rack_macro.plocks.get(source_step).copied().flatten();
                        let target = rack_macro.plocks.get(target_step).copied().flatten();
                        (source != target).then_some((rack_macro.id, source))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut changed = false;
        for (id, value) in values {
            changed |= self.update_rack_macro_in_current_pattern(track, id, |rack_macro| {
                rack_macro.plocks[target_step] = value;
            });
        }
        changed
    }

    fn clear_variant_locks_for_step_inner(&self, track: usize, step: usize) -> bool {
        let mut changed = clear_track_variant_locks(self, track, step);
        for slot in &self.pattern.midi_fx_slots[track] {
            changed |= clear_live_slot_variant_locks(slot, step);
        }
        for slot in &self.pattern.effect_chains[track] {
            changed |= clear_live_slot_variant_locks(slot, step);
        }
        changed |= clear_live_slot_variant_locks(&self.pattern.instrument_slots[track], step);
        changed |= self.clear_rack_macro_plocks_for_step(track, step);
        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            for slot in &mut rack.slots {
                changed |= clear_rack_slot_variant_locks(slot, step);
            }
        }
        changed
    }

    fn copy_variant_locks_between_steps_inner(
        &self,
        track: usize,
        source_step: usize,
        target_step: usize,
    ) -> bool {
        let mut changed = copy_track_variant_locks(self, track, source_step, target_step);
        for slot in &self.pattern.midi_fx_slots[track] {
            changed |= copy_live_slot_variant_locks(slot, source_step, target_step);
        }
        for slot in &self.pattern.effect_chains[track] {
            changed |= copy_live_slot_variant_locks(slot, source_step, target_step);
        }
        changed |= copy_live_slot_variant_locks(
            &self.pattern.instrument_slots[track],
            source_step,
            target_step,
        );
        changed |= self.copy_rack_macro_plocks_between_steps(track, source_step, target_step);
        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            for slot in &mut rack.slots {
                changed |= copy_rack_slot_variant_locks(slot, source_step, target_step);
            }
        }
        changed
    }

    pub(crate) fn set_step_param_inner(
        &self,
        track: usize,
        step: usize,
        param: StepParam,
        value: f32,
    ) {
        let previous = self.pattern.step_data[track].get(step, param);
        self.pattern.step_data[track].set(step, param, value);

        if param != StepParam::Transpose {
            return;
        }

        let applied = self.pattern.step_data[track].get(step, param);
        let delta = applied - previous;
        if delta == 0.0 {
            return;
        }

        let chord_count = self.pattern.chord_data[track].count(step);
        if chord_count == 0 {
            return;
        }

        let mut notes = Vec::with_capacity(chord_count);
        for note_idx in 0..chord_count {
            notes.push((
                self.pattern.chord_data[track].get(step, note_idx) + delta,
                self.pattern.chord_data[track].get_duration(step, note_idx),
                self.pattern.chord_data[track].get_delay(step, note_idx),
            ));
        }
        self.pattern.chord_data[track].clear_step(step);
        for (transpose, duration, delay) in notes {
            self.pattern.chord_data[track].add_note_with_timing(step, transpose, duration, delay);
        }
    }

    pub fn set_step_param(&self, track: usize, step: usize, param: StepParam, value: f32) {
        self.set_step_param_inner(track, step, param, value);
        self.publish_scheduler_snapshot();
    }

    pub fn adjust_step_param(&self, track: usize, step: usize, param: StepParam, delta: f32) {
        let current = self.pattern.step_data[track].get(step, param);
        self.set_step_param(track, step, param, current + delta);
    }

    pub(crate) fn restore_step_snapshot_inner(
        &self,
        track: usize,
        step: usize,
        snapshot: &StepSnapshot,
    ) {
        for param in StepParam::ALL {
            self.pattern.step_data[track].set(step, param, snapshot.params[param.index()]);
        }

        self.pattern.patterns[track].set_step_active(step, snapshot.active);
        self.pattern.neural_reset_patterns[track].set_step_active(step, snapshot.neural_reset);

        self.pattern.chord_data[track].clear_step(step);
        for (idx, &transpose) in snapshot.chord.iter().enumerate() {
            self.pattern.chord_data[track].add_note_with_timing(
                step,
                transpose,
                snapshot.chord_durations.get(idx).copied().unwrap_or(0.0),
                snapshot.chord_delays.get(idx).copied().unwrap_or(0.0),
            );
        }

        match snapshot.timebase {
            Some(tb) => self.pattern.timebase_plocks[track].set(step, tb),
            None => self.pattern.timebase_plocks[track].clear(step),
        }
        match snapshot.swing {
            Some(swing) => self.pattern.swing_plocks[track].set(step, swing),
            None => self.pattern.swing_plocks[track].clear(step),
        }
        match snapshot.swing_resolution {
            Some(resolution) => self.pattern.swing_resolution_plocks[track].set(step, resolution),
            None => self.pattern.swing_resolution_plocks[track].clear(step),
        }

        for (slot_idx, slot) in self.pattern.midi_fx_slots[track].iter().enumerate() {
            restore_live_slot_step_plocks(slot, step, snapshot.midi_fx_plocks.get(slot_idx));
        }
        for (slot_idx, slot) in self.pattern.effect_chains[track].iter().enumerate() {
            restore_live_slot_step_plocks(slot, step, snapshot.effect_plocks.get(slot_idx));
        }

        restore_live_slot_step_plocks(
            &self.pattern.instrument_slots[track],
            step,
            Some(&snapshot.instrument_plocks),
        );

        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            for (macro_idx, rack_macro) in rack.macros.iter_mut().enumerate() {
                rack_macro.plocks[step] = snapshot
                    .rack_macro_plocks
                    .get(macro_idx)
                    .copied()
                    .flatten();
            }
            for (slot_idx, slot) in rack.slots.iter_mut().enumerate() {
                let saved_params = snapshot.rack_slot_param_plocks.get(slot_idx);
                for param in RackSlotParam::ALL {
                    let value = saved_params
                        .and_then(|plocks| plocks.params.get(param.index()))
                        .copied()
                        .flatten();
                    match value {
                        Some(value) => {
                            slot.param_plocks.set(step, param, value);
                        }
                        None => {
                            slot.param_plocks.clear(step, param);
                        }
                    }
                }

                restore_snapshot_slot_step_plocks(
                    &mut slot.instrument_slot,
                    step,
                    snapshot.rack_slot_instrument_plocks.get(slot_idx),
                );
                for (effect_idx, effect) in slot.effect_slots.iter_mut().enumerate() {
                    restore_snapshot_slot_step_plocks(
                        effect,
                        step,
                        snapshot
                            .rack_slot_effect_plocks
                            .get(slot_idx)
                            .and_then(|effects| effects.get(effect_idx)),
                    );
                }
            }
        }
    }

    pub fn restore_step_snapshot(&self, track: usize, step: usize, snapshot: &StepSnapshot) {
        self.restore_step_snapshot_inner(track, step, snapshot);
        self.publish_scheduler_snapshot();
    }

    pub fn restore_step_snapshot_no_publish(
        &self,
        track: usize,
        step: usize,
        snapshot: &StepSnapshot,
    ) {
        self.restore_step_snapshot_inner(track, step, snapshot);
    }

    /// Cyclically rotate `steps` (sorted) left (direction < 0) or right (direction > 0).
    pub(crate) fn rotate_steps_no_publish(
        &self,
        track: usize,
        steps: &[usize],
        direction: isize,
    ) {
        if steps.len() < 2 || direction == 0 {
            return;
        }
        let snapshots: Vec<_> = steps
            .iter()
            .map(|&s| self.capture_step_snapshot(track, s))
            .collect();
        let n = steps.len();
        for (i, &step) in steps.iter().enumerate() {
            let src = if direction > 0 {
                // Rotate right: slot i gets content from slot i-1 (last wraps to first)
                if i == 0 {
                    n - 1
                } else {
                    i - 1
                }
            } else {
                // Rotate left: slot i gets content from slot i+1 (first wraps to last)
                (i + 1) % n
            };
            self.restore_step_snapshot_inner(track, step, &snapshots[src]);
        }
    }

    pub fn rotate_steps(&self, track: usize, steps: &[usize], direction: isize) {
        self.rotate_steps_no_publish(track, steps, direction);
        self.publish_scheduler_snapshot();
    }

    pub(crate) fn move_step_range_no_publish(
        &self,
        track: usize,
        lo: usize,
        hi: usize,
        new_lo: usize,
    ) {
        if lo > hi || hi >= MAX_STEPS {
            return;
        }

        let count = hi - lo + 1;
        let new_hi = new_lo + count - 1;
        if new_lo == lo || new_hi >= MAX_STEPS {
            return;
        }

        let snapshots: Vec<_> = (lo..=hi)
            .map(|step| self.capture_step_snapshot(track, step))
            .collect();

        for step in lo..=hi {
            if step < new_lo || step > new_hi {
                self.clear_step_payload_inner(track, step);
            }
        }

        for (offset, step) in (new_lo..=new_hi).enumerate() {
            self.restore_step_snapshot_inner(track, step, &snapshots[offset]);
        }
    }

    pub fn move_step_range(&self, track: usize, lo: usize, hi: usize, new_lo: usize) {
        self.move_step_range_no_publish(track, lo, hi, new_lo);
        self.publish_scheduler_snapshot();
    }

    pub(crate) fn duplicate_track_pattern_no_publish(&self, track: usize) -> usize {
        let num_steps = self.pattern.track_params[track].get_num_steps();
        let new_len = (num_steps * 2).min(MAX_STEPS);
        if new_len == num_steps {
            return num_steps;
        }

        for step in num_steps..new_len {
            let src = step - num_steps;
            let snapshot = self.capture_step_snapshot(track, src);
            self.restore_step_snapshot_inner(track, step, &snapshot);
        }

        self.pattern.track_params[track].set_num_steps(new_len);
        new_len
    }

    pub fn duplicate_track_pattern(&self, track: usize) -> usize {
        let new_len = self.duplicate_track_pattern_no_publish(track);
        self.publish_scheduler_snapshot();
        new_len
    }

    pub(crate) fn halve_track_pattern_no_publish(&self, track: usize) -> usize {
        let num_steps = self.pattern.track_params[track].get_num_steps();
        let new_len = (num_steps / 2).max(1);
        if new_len == num_steps {
            return num_steps;
        }
        self.pattern.track_params[track].set_num_steps(new_len);
        new_len
    }

    pub fn halve_track_pattern(&self, track: usize) -> usize {
        let new_len = self.halve_track_pattern_no_publish(track);
        self.publish_scheduler_snapshot();
        new_len
    }
}
