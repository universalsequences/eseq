use super::*;

#[derive(Clone, Debug)]
pub struct TrackPatternData {
    pub track_bits: [u64; TRACK_PATTERN_WORDS],
    pub neural_reset_bits: [u64; TRACK_PATTERN_WORDS],
    pub step_data: Vec<[f32; NUM_PARAMS]>,
    pub track_params: TrackParamsSnapshot,
    pub effect_slots: Vec<EffectSlotSnapshot>,
    pub midi_fx_slots: Vec<EffectSlotSnapshot>,
    pub instrument_slot: EffectSlotSnapshot,
    pub instrument_base_note_offset: f32,
    pub track_sound_state: TrackSoundState,
    pub sample_id: (i32, String, u32),
    pub chord_snapshot: ChordSnapshot,
    pub timebase_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub swing_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub swing_resolution_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    pub rack_track: Option<RackTrackSnapshot>,
    pub process_chain: crate::process::TrackProcessChain,
    pub project_process_lane_overrides: crate::process::ProjectLaneOverrides,
    pub plock_variant_registry: PlockVariantRegistry,
    pub key_lock_variant_registry: PlockVariantRegistry,
}

impl TrackPatternData {
    /// Strip every per-step lane (notes, params, chords, timing plocks),
    /// keeping the track-level device/param state. Used to mint empty take
    /// chunks from a template pattern (takes spec 6.1).
    pub fn clear_step_content(&mut self) {
        self.track_bits = [0u64; TRACK_PATTERN_WORDS];
        self.neural_reset_bits = [0u64; TRACK_PATTERN_WORDS];
        for step in self.step_data.iter_mut() {
            for param in StepParam::ALL {
                step[param.index()] = param.default_value();
            }
        }
        for lane in [
            &mut self.chord_snapshot.steps,
            &mut self.chord_snapshot.durations,
            &mut self.chord_snapshot.delays,
        ] {
            for step in lane.iter_mut() {
                step.clear();
            }
        }
        self.timebase_plock_snapshot = [None; MAX_STEPS];
        self.swing_plock_snapshot = [None; MAX_STEPS];
        self.swing_resolution_plock_snapshot = [None; MAX_STEPS];
    }

    /// Copy one step's complete per-step content (activation, params,
    /// chord notes/durations/delays, timing plocks) from `src`'s
    /// `src_step` into this pattern's `dst_step`.
    pub fn copy_step_content_from(
        &mut self,
        dst_step: usize,
        src: &TrackPatternData,
        src_step: usize,
    ) {
        if dst_step >= MAX_STEPS || src_step >= MAX_STEPS {
            return;
        }
        let src_active = src.track_bits[src_step / 64] >> (src_step % 64) & 1 == 1;
        if src_active {
            self.track_bits[dst_step / 64] |= 1 << (dst_step % 64);
        } else {
            self.track_bits[dst_step / 64] &= !(1 << (dst_step % 64));
        }
        let src_reset = src.neural_reset_bits[src_step / 64] >> (src_step % 64) & 1 == 1;
        if src_reset {
            self.neural_reset_bits[dst_step / 64] |= 1 << (dst_step % 64);
        } else {
            self.neural_reset_bits[dst_step / 64] &= !(1 << (dst_step % 64));
        }
        if let (Some(dst), Some(src_row)) =
            (self.step_data.get_mut(dst_step), src.step_data.get(src_step))
        {
            *dst = *src_row;
        }
        for (dst_lane, src_lane) in [
            (&mut self.chord_snapshot.steps, &src.chord_snapshot.steps),
            (&mut self.chord_snapshot.durations, &src.chord_snapshot.durations),
            (&mut self.chord_snapshot.delays, &src.chord_snapshot.delays),
        ] {
            if let Some(dst) = dst_lane.get_mut(dst_step) {
                *dst = src_lane.get(src_step).cloned().unwrap_or_default();
            }
        }
        self.timebase_plock_snapshot[dst_step] = src.timebase_plock_snapshot[src_step];
        self.swing_plock_snapshot[dst_step] = src.swing_plock_snapshot[src_step];
        self.swing_resolution_plock_snapshot[dst_step] =
            src.swing_resolution_plock_snapshot[src_step];
    }
}

/// Instrument-owned authoring state for one track pattern.  Structural
/// instrument replacement deliberately resets these fields; keeping them in a
/// separate snapshot lets undo restore the binding without overwriting notes,
/// timing, mixer values, or effect state edited by other operations.
#[derive(Clone, Debug)]
pub struct TrackInstrumentPatternState {
    pub instrument_slot: EffectSlotSnapshot,
    pub instrument_base_note_offset: f32,
    pub track_sound_state: TrackSoundState,
    pub sample_id: (i32, String, u32),
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    pub rack_track: Option<RackTrackSnapshot>,
    pub process_chain: crate::process::TrackProcessChain,
    pub project_process_lane_overrides: crate::process::ProjectLaneOverrides,
    pub plock_variant_registry: PlockVariantRegistry,
    pub key_lock_variant_registry: PlockVariantRegistry,
}

#[derive(Clone, Debug)]
pub struct NeuralInstrumentOverrideState {
    pub scene: usize,
    pub network: usize,
    pub neuron: usize,
    pub entries: Vec<(usize, crate::neural::ProjectParamOverride)>,
}

#[derive(Clone, Debug)]
pub struct NeuralEffectOverrideState {
    pub scene: usize,
    pub network: usize,
    pub neuron: usize,
    pub entries: Vec<(usize, crate::neural::ProjectEffectParamOverride)>,
}

#[derive(Clone, Debug)]
pub struct TrackEffectBindingStateSnapshot {
    pub process_chains: Vec<(PatternId, crate::process::TrackProcessChain)>,
    pub project_process_lane_overrides:
        Vec<(PatternId, crate::process::ProjectLaneOverrides)>,
    pub neural_overrides: Vec<NeuralEffectOverrideState>,
}

#[derive(Clone, Debug)]
pub struct TrackInstrumentPatternStateSnapshot {
    pub live: TrackInstrumentPatternState,
    pub patterns: Vec<(PatternId, TrackInstrumentPatternState)>,
    pub neural_overrides: Vec<NeuralInstrumentOverrideState>,
}

#[derive(Clone, Debug)]
pub struct RackSlotPatternStateSnapshot {
    pub slot_index: usize,
    pub live: RackSlotSnapshot,
    pub patterns: Vec<(PatternId, RackSlotSnapshot)>,
}

#[derive(Clone, Debug)]
pub struct RackMacroPatternStateSnapshot {
    pub live: Vec<RackMacro>,
    pub patterns: Vec<(PatternId, Vec<RackMacro>)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstrumentSlotResetSummary {
    pub patterns_reset: usize,
    pub patterns_with_cleared_locks: usize,
    pub process_bindings_dropped: usize,
    pub neural_overrides_dropped: usize,
}

pub(super) enum InstrumentSourceReset {
    Custom {
        engine_id: usize,
        run_mode: CustomInstrumentRunMode,
    },
    Sampler {
        sample_id: (i32, String, u32),
    },
}

impl InstrumentSourceReset {
    pub(super) fn instrument_type(&self) -> InstrumentType {
        match self {
            Self::Custom { .. } => InstrumentType::Custom,
            Self::Sampler { .. } => InstrumentType::Sampler,
        }
    }

    pub(super) fn run_mode(&self) -> CustomInstrumentRunMode {
        match self {
            Self::Custom { run_mode, .. } => *run_mode,
            Self::Sampler { .. } => CustomInstrumentRunMode::Instrument,
        }
    }

    pub(super) fn engine_id(&self) -> Option<usize> {
        match self {
            Self::Custom { engine_id, .. } => Some(*engine_id),
            Self::Sampler { .. } => None,
        }
    }

    pub(super) fn sample_id(&self) -> (i32, String, u32) {
        match self {
            Self::Custom { .. } => (-1, String::new(), 44_100),
            Self::Sampler { sample_id } => sample_id.clone(),
        }
    }
}

pub(super) const INSTRUMENT_PLOCK_VARIANT_DOMAINS: &[PlockVariantDomain] = &[
    PlockVariantDomain::Instrument,
    PlockVariantDomain::InstrumentTensor,
    PlockVariantDomain::InstrumentKeyLock,
];

pub(super) fn instrument_slot_has_locks(slot: &EffectSlotSnapshot) -> bool {
    slot.plocks
        .iter()
        .any(|row| row.iter().any(Option::is_some))
        || slot
            .key_locks
            .values()
            .any(|row| row.iter().any(Option::is_some))
        || slot
            .tensor_params
            .iter()
            .any(|tensor| tensor.plocks.iter().any(Option::is_some))
}

impl TrackPatternData {
    pub(super) fn instrument_state(&self) -> TrackInstrumentPatternState {
        TrackInstrumentPatternState {
            instrument_slot: self.instrument_slot.clone(),
            instrument_base_note_offset: self.instrument_base_note_offset,
            track_sound_state: self.track_sound_state.clone(),
            sample_id: self.sample_id.clone(),
            instrument_type: self.instrument_type,
            instrument_run_mode: self.instrument_run_mode,
            rack_track: self.rack_track.clone(),
            process_chain: self.process_chain.clone(),
            project_process_lane_overrides: self.project_process_lane_overrides.clone(),
            plock_variant_registry: self.plock_variant_registry.clone(),
            key_lock_variant_registry: self.key_lock_variant_registry.clone(),
        }
    }

    pub(super) fn restore_instrument_state(
        &mut self,
        state: &TrackInstrumentPatternState,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        let mut instrument_slot = state.instrument_slot.clone();
        instrument_slot.sync_to_descriptor_with_modulator(
            descriptor,
            node_id,
            modulator_node_id,
        );
        self.instrument_slot = instrument_slot;
        self.instrument_base_note_offset = state.instrument_base_note_offset;
        self.track_sound_state = state.track_sound_state.clone();
        self.sample_id = state.sample_id.clone();
        self.instrument_type = state.instrument_type;
        self.instrument_run_mode = state.instrument_run_mode;
        self.rack_track = state.rack_track.clone();
        self.process_chain = state.process_chain.clone();
        crate::process::rebind_track_process_chain_instrument_param_ids(
            &mut self.process_chain,
            descriptor,
            &self.instrument_slot,
        );
        self.project_process_lane_overrides = state.project_process_lane_overrides.clone();
        self.plock_variant_registry = state.plock_variant_registry.clone();
        self.key_lock_variant_registry = state.key_lock_variant_registry.clone();
    }

    pub(super) fn capture_step_snapshot(&self, step: usize) -> Option<StepSnapshot> {
        if step >= MAX_STEPS {
            return None;
        }
        let word = step / 64;
        let bit = step % 64;
        let params = *self.step_data.get(step)?;
        let rack = self.rack_track.as_ref();

        Some(StepSnapshot {
            active: (self.track_bits[word] >> bit) & 1 == 1,
            neural_reset: (self.neural_reset_bits[word] >> bit) & 1 == 1,
            params,
            chord: self.chord_snapshot.steps.get(step)?.clone(),
            chord_durations: self.chord_snapshot.durations.get(step)?.clone(),
            chord_delays: self.chord_snapshot.delays.get(step)?.clone(),
            timebase: self.timebase_plock_snapshot[step].map(Timebase::from_index),
            swing: self.swing_plock_snapshot[step].map(f32::from_bits),
            swing_resolution: self.swing_resolution_plock_snapshot[step]
                .map(SwingResolution::from_index),
            midi_fx_plocks: self
                .midi_fx_slots
                .iter()
                .map(|slot| capture_snapshot_slot_step_plocks(slot, step))
                .collect(),
            effect_plocks: self
                .effect_slots
                .iter()
                .map(|slot| capture_snapshot_slot_step_plocks(slot, step))
                .collect(),
            instrument_plocks: capture_snapshot_slot_step_plocks(
                &self.instrument_slot,
                step,
            ),
            rack_macro_plocks: rack
                .map(|rack| {
                    rack.macros
                        .iter()
                        .map(|rack_macro| rack_macro.plocks[step])
                        .collect()
                })
                .unwrap_or_default(),
            rack_slot_param_plocks: rack
                .map(|rack| {
                    rack.slots
                        .iter()
                        .map(|slot| StepSlotPlocks {
                            params: RackSlotParam::ALL
                                .iter()
                                .map(|param| slot.param_plocks.get(step, *param))
                                .collect(),
                            tensor_params: Vec::new(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            rack_slot_instrument_plocks: rack
                .map(|rack| {
                    rack.slots
                        .iter()
                        .map(|slot| {
                            capture_snapshot_slot_step_plocks(&slot.instrument_slot, step)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            rack_slot_effect_plocks: rack
                .map(|rack| {
                    rack.slots
                        .iter()
                        .map(|slot| {
                            slot.effect_slots
                                .iter()
                                .map(|effect| {
                                    capture_snapshot_slot_step_plocks(effect, step)
                                })
                                .collect()
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    pub(super) fn restore_step_snapshot(&mut self, step: usize, snapshot: &StepSnapshot) -> bool {
        if step >= MAX_STEPS
            || step >= self.step_data.len()
            || step >= self.chord_snapshot.steps.len()
            || step >= self.chord_snapshot.durations.len()
            || step >= self.chord_snapshot.delays.len()
        {
            return false;
        }
        let word = step / 64;
        let mask = 1u64 << (step % 64);
        if snapshot.active {
            self.track_bits[word] |= mask;
        } else {
            self.track_bits[word] &= !mask;
        }
        if snapshot.neural_reset {
            self.neural_reset_bits[word] |= mask;
        } else {
            self.neural_reset_bits[word] &= !mask;
        }
        self.step_data[step] = snapshot.params;

        self.chord_snapshot.steps[step] = snapshot.chord.clone();
        self.chord_snapshot.durations[step] = snapshot.chord_durations.clone();
        self.chord_snapshot.delays[step] = snapshot.chord_delays.clone();
        self.timebase_plock_snapshot[step] = snapshot.timebase.map(|value| value as u32);
        self.swing_plock_snapshot[step] = snapshot.swing.map(f32::to_bits);
        self.swing_resolution_plock_snapshot[step] =
            snapshot.swing_resolution.map(|value| value as u32);

        for (slot_idx, slot) in self.midi_fx_slots.iter_mut().enumerate() {
            restore_snapshot_slot_step_plocks(slot, step, snapshot.midi_fx_plocks.get(slot_idx));
        }
        for (slot_idx, slot) in self.effect_slots.iter_mut().enumerate() {
            restore_snapshot_slot_step_plocks(slot, step, snapshot.effect_plocks.get(slot_idx));
        }
        restore_snapshot_slot_step_plocks(
            &mut self.instrument_slot,
            step,
            Some(&snapshot.instrument_plocks),
        );

        if let Some(rack) = self.rack_track.as_mut() {
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
                    match saved_params
                        .and_then(|plocks| plocks.params.get(param.index()))
                        .copied()
                        .flatten()
                    {
                        Some(value) => slot.param_plocks.set(step, param, value),
                        None => slot.param_plocks.clear(step, param),
                    };
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
        true
    }

    pub(super) fn reset_instrument_source(
        &mut self,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
        source: &InstrumentSourceReset,
    ) -> (bool, usize) {
        let cleared_locks = instrument_slot_has_locks(&self.instrument_slot);
        self.instrument_slot =
            EffectSlotSnapshot::new_default_with_modulator(descriptor, node_id, modulator_node_id);
        self.track_sound_state = TrackSoundState {
            engine_id: source.engine_id(),
            loaded_preset: None,
            dirty: false,
        };
        self.sample_id = source.sample_id();
        self.instrument_type = source.instrument_type();
        self.instrument_run_mode = source.run_mode();
        self.rack_track = None;
        self.plock_variant_registry
            .remove_domains(INSTRUMENT_PLOCK_VARIANT_DOMAINS);
        self.key_lock_variant_registry
            .remove_domains(INSTRUMENT_PLOCK_VARIANT_DOMAINS);
        let dropped_bindings = crate::process::rebind_track_process_chain_instrument_param_ids(
            &mut self.process_chain,
            descriptor,
            &self.instrument_slot,
        );
        (cleared_locks, dropped_bindings)
    }

    pub(super) fn refresh_process_effect_binding_param_ids_for_slot(
        &mut self,
        slot_idx: usize,
        descriptor: &EffectDescriptor,
    ) {
        let Some(effect_slot) = self.effect_slots.get(slot_idx) else {
            return;
        };
        crate::process::refresh_track_process_chain_effect_binding_param_ids_for_slot(
            &mut self.process_chain,
            slot_idx,
            descriptor,
            effect_slot,
        );
    }

    pub(crate) fn refreshed_process_chain(
        &self,
        instrument_descriptor: Option<&EffectDescriptor>,
        effect_descriptors: &[EffectDescriptor],
    ) -> crate::process::TrackProcessChain {
        let mut process_chain = self.process_chain.clone();
        crate::process::refresh_track_process_chain_binding_param_ids(
            &mut process_chain,
            instrument_descriptor,
            Some(&self.instrument_slot),
            effect_descriptors,
            &self.effect_slots,
        );
        process_chain
    }

    /// Adopt `source`'s sound, keeping every per-step lane and the step grid.
    /// The explicit-propagation gestures (takes spec 16.5) and the take-chunk
    /// fan-out are the only things that move a device snapshot between
    /// patterns; the step-grid fields stay this pattern's own for the same
    /// reason `restore_device_state_to` keeps them.
    pub fn copy_device_state_from(&mut self, source: &TrackPatternData) {
        let num_steps = self.track_params.num_steps;
        let timebase = self.track_params.timebase;
        let swing = self.track_params.swing;
        let swing_resolution = self.track_params.swing_resolution;
        self.track_params = source.track_params.clone();
        self.track_params.num_steps = num_steps;
        self.track_params.timebase = timebase;
        self.track_params.swing = swing;
        self.track_params.swing_resolution = swing_resolution;

        self.effect_slots = source.effect_slots.clone();
        self.midi_fx_slots = source.midi_fx_slots.clone();
        self.instrument_slot = source.instrument_slot.clone();
        self.instrument_base_note_offset = source.instrument_base_note_offset;
        self.instrument_type = source.instrument_type;
        self.instrument_run_mode = source.instrument_run_mode;
        self.track_sound_state = source.track_sound_state.clone();
        self.rack_track = source.rack_track.clone();
        self.process_chain = source.process_chain.clone();
        self.sample_id = source.sample_id.clone();
    }

    /// Restore only the device/sound half of this snapshot into the live
    /// mirror — instruments, effects, MIDI FX, rack and the mixer-side track
    /// params — leaving every per-step lane and the step grid itself alone.
    ///
    /// This is how a sound binding (takes spec 16.2) puts a take's or a
    /// track clip's frozen sound behind the device panel without dragging
    /// its step content into the session surface. The step-grid fields
    /// (`num_steps`, timebase, swing) are deliberately kept from the live
    /// mirror: a take chunk is always `MAX_STEPS` wide and adopting that
    /// would resize the session's step view.
    pub(super) fn restore_device_state_to(&self, state: &SequencerState, track: usize) -> bool {
        if track >= state.pattern.track_params.len()
            || track >= state.pattern.effect_chains.len()
            || track >= state.pattern.midi_fx_slots.len()
            || track >= state.pattern.instrument_slots.len()
            || track >= state.pattern.instrument_base_note_offsets.len()
            || track >= state.pattern.instrument_run_modes.len()
            || track >= state.runtime.instrument_run_mode_flags.len()
        {
            return false;
        }

        let tp = &state.pattern.track_params[track];
        let mut params = self.track_params.clone();
        params.num_steps = tp.get_num_steps();
        params.timebase = tp.get_timebase();
        params.swing = tp.get_swing();
        params.swing_resolution = tp.get_swing_resolution();
        restore_track_params_snapshot(tp, &params);

        for (slot_idx, slot_snap) in self.effect_slots.iter().enumerate() {
            if slot_idx < state.pattern.effect_chains[track].len() {
                slot_snap.restore(&state.pattern.effect_chains[track][slot_idx]);
            }
        }
        for (slot_idx, slot_snap) in self.midi_fx_slots.iter().enumerate() {
            if slot_idx < state.pattern.midi_fx_slots[track].len() {
                slot_snap.restore(&state.pattern.midi_fx_slots[track][slot_idx]);
            }
        }

        self.instrument_slot
            .restore(&state.pattern.instrument_slots[track]);
        state.pattern.instrument_base_note_offsets[track].store(
            self.instrument_base_note_offset.to_bits(),
            Ordering::Relaxed,
        );
        state.pattern.instrument_run_modes[track]
            .store(self.instrument_run_mode.runtime_flag(), Ordering::Relaxed);
        state.runtime.instrument_run_mode_flags[track]
            .store(self.instrument_run_mode.runtime_flag(), Ordering::Relaxed);

        {
            let mut track_sound_state = state.pattern.track_sound_state.lock().unwrap();
            if track < track_sound_state.len() {
                track_sound_state[track] = self.track_sound_state.clone();
            }
        }
        {
            let mut rack_tracks = state.pattern.rack_tracks.lock().unwrap();
            if track < rack_tracks.len() {
                rack_tracks[track] = self.rack_track.clone();
            }
        }
        let refreshed_process_chain = {
            let effect_descriptors = state.scratch_effect_descriptors.lock().unwrap();
            let instrument_descriptors = state.scratch_instrument_descriptors.lock().unwrap();
            self.refreshed_process_chain(
                instrument_descriptors.get(track),
                effect_descriptors
                    .get(track)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )
        };
        {
            let mut process_chains = state.pattern.process_chains.lock().unwrap();
            if track < process_chains.len() {
                process_chains[track] = refreshed_process_chain;
            }
        }
        true
    }

    pub(super) fn restore_to(&self, state: &SequencerState, track: usize) -> bool {
        if track >= state.pattern.patterns.len()
            || track >= state.pattern.neural_reset_patterns.len()
            || track >= state.pattern.step_data.len()
            || track >= state.pattern.track_params.len()
            || track >= state.pattern.effect_chains.len()
            || track >= state.pattern.midi_fx_slots.len()
            || track >= state.pattern.instrument_slots.len()
            || track >= state.pattern.instrument_base_note_offsets.len()
            || track >= state.pattern.instrument_run_modes.len()
            || track >= state.runtime.instrument_run_mode_flags.len()
            || track >= state.pattern.chord_data.len()
            || track >= state.pattern.timebase_plocks.len()
            || track >= state.pattern.swing_plocks.len()
            || track >= state.pattern.swing_resolution_plocks.len()
            || track >= state.pattern.process_chains.lock().unwrap().len()
            || track >= state.pattern.plock_variant_registries.lock().unwrap().len()
            || track
                >= state
                    .pattern
                    .key_lock_variant_registries
                    .lock()
                    .unwrap()
                    .len()
        {
            return false;
        }

        state.pattern.patterns[track].store_bits(self.track_bits);
        state.pattern.neural_reset_patterns[track].store_bits(self.neural_reset_bits);

        state.pattern.step_data[track].store_rows_clamped(&self.step_data);

        let tp = &state.pattern.track_params[track];
        let snap = &self.track_params;
        restore_track_params_snapshot(tp, snap);

        for (slot_idx, slot_snap) in self.effect_slots.iter().enumerate() {
            if slot_idx < state.pattern.effect_chains[track].len() {
                slot_snap.restore(&state.pattern.effect_chains[track][slot_idx]);
            }
        }
        for (slot_idx, slot_snap) in self.midi_fx_slots.iter().enumerate() {
            if slot_idx < state.pattern.midi_fx_slots[track].len() {
                slot_snap.restore(&state.pattern.midi_fx_slots[track][slot_idx]);
            }
        }

        self.instrument_slot
            .restore(&state.pattern.instrument_slots[track]);
        state.pattern.instrument_base_note_offsets[track].store(
            self.instrument_base_note_offset.to_bits(),
            Ordering::Relaxed,
        );
        state.pattern.instrument_run_modes[track]
            .store(self.instrument_run_mode.runtime_flag(), Ordering::Relaxed);
        state.runtime.instrument_run_mode_flags[track]
            .store(self.instrument_run_mode.runtime_flag(), Ordering::Relaxed);

        {
            let mut track_sound_state = state.pattern.track_sound_state.lock().unwrap();
            if track < track_sound_state.len() {
                track_sound_state[track] = self.track_sound_state.clone();
            }
        }
        {
            let mut rack_tracks = state.pattern.rack_tracks.lock().unwrap();
            if track < rack_tracks.len() {
                rack_tracks[track] = self.rack_track.clone();
            }
        }
        let refreshed_process_chain = {
            let effect_descriptors = state.scratch_effect_descriptors.lock().unwrap();
            let instrument_descriptors = state.scratch_instrument_descriptors.lock().unwrap();
            self.refreshed_process_chain(
                instrument_descriptors.get(track),
                effect_descriptors
                    .get(track)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )
        };
        {
            let mut process_chains = state.pattern.process_chains.lock().unwrap();
            if track < process_chains.len() {
                process_chains[track] = refreshed_process_chain;
            }
        }
        {
            let mut overrides = state.pattern.project_process_lane_overrides.lock().unwrap();
            if track < overrides.len() {
                overrides[track] = self.project_process_lane_overrides.clone();
            }
        }
        {
            let mut registries = state.pattern.plock_variant_registries.lock().unwrap();
            if track < registries.len() {
                registries[track] = self.plock_variant_registry.clone();
            }
        }
        let active_variant_keys = live_track_variant_keys(state, track);
        if let Some(registry) = state
            .pattern
            .plock_variant_registries
            .lock()
            .unwrap()
            .get_mut(track)
        {
            registry.prune_to_keys(&active_variant_keys);
        }
        {
            let mut registries = state.pattern.key_lock_variant_registries.lock().unwrap();
            if track < registries.len() {
                registries[track] = self.key_lock_variant_registry.clone();
            }
        }
        let active_key_lock_variant_keys = live_track_key_lock_variant_keys(state, track);
        if let Some(registry) = state
            .pattern
            .key_lock_variant_registries
            .lock()
            .unwrap()
            .get_mut(track)
        {
            registry.prune_to_keys(&active_key_lock_variant_keys);
        }

        self.chord_snapshot
            .restore(&state.pattern.chord_data[track]);
        state.pattern.timebase_plocks[track].restore(&self.timebase_plock_snapshot);
        state.pattern.swing_plocks[track].restore(&self.swing_plock_snapshot);
        state.pattern.swing_resolution_plocks[track].restore(&self.swing_resolution_plock_snapshot);

        true
    }

    pub(super) fn remove_effect_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.effect_slots.len() {
            return;
        }
        for idx in slot_idx..self.effect_slots.len().saturating_sub(1) {
            self.effect_slots[idx] = self.effect_slots[idx + 1].clone();
        }
        if let Some(last) = self.effect_slots.last_mut() {
            last.clear();
        }
    }

    pub(super) fn insert_empty_effect_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.effect_slots.len() {
            return;
        }
        for idx in (slot_idx + 1..self.effect_slots.len()).rev() {
            self.effect_slots[idx] = self.effect_slots[idx - 1].clone();
        }
        self.effect_slots[slot_idx].clear();
    }

    pub(super) fn move_effect_slot_to(&mut self, source_slot: usize, target_slot: usize) {
        if source_slot >= self.effect_slots.len()
            || target_slot >= self.effect_slots.len()
            || source_slot == target_slot
        {
            return;
        }
        let entry = self.effect_slots.remove(source_slot);
        self.effect_slots.insert(target_slot, entry);
        while self.effect_slots.len() <= target_slot.max(source_slot) {
            self.effect_slots.push(EffectSlotSnapshot::new_empty());
        }
    }

    pub(super) fn sync_effect_slot_with_modulator(
        &mut self,
        slot_idx: usize,
        desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        while self.effect_slots.len() <= slot_idx {
            self.effect_slots.push(EffectSlotSnapshot::new_empty());
        }
        self.effect_slots[slot_idx].sync_to_descriptor_with_modulator(
            desc,
            node_id,
            modulator_node_id,
        );
    }

    pub(super) fn remap_sidechain_references_after_track_delete(
        &mut self,
        owner_track_old: usize,
        effect_descriptors: &[EffectDescriptor],
        deleted_track: usize,
        old_track_count: usize,
    ) {
        for (slot_idx, slot) in self.effect_slots.iter_mut().enumerate() {
            let Some(desc) = effect_descriptors.get(slot_idx) else {
                continue;
            };
            let num_params = slot.num_params as usize;
            for param_idx in 0..num_params.min(desc.params.len()) {
                if !matches!(
                    desc.params[param_idx].host_control,
                    Some(HostControl::FxSidechain { .. })
                ) {
                    continue;
                }
                if param_idx < slot.defaults.len() {
                    slot.defaults[param_idx] = remap_sidechain_selection_after_track_delete(
                        owner_track_old,
                        slot.defaults[param_idx].round().max(0.0) as usize,
                        deleted_track,
                        old_track_count,
                    ) as f32;
                }
                for step in 0..MAX_STEPS {
                    let selection = slot.plocks.get(step)
                        .and_then(|params| params.get(param_idx))
                        .and_then(|value| *value);
                    if let (Some(selection), Some(value)) = (
                        selection,
                        slot.plocks.get_mut(step).and_then(|params| params.get_mut(param_idx)),
                    ) {
                        *value = Some(remap_sidechain_selection_after_track_delete(
                            owner_track_old,
                            selection.round().max(0.0) as usize,
                            deleted_track,
                            old_track_count,
                        ) as f32);
                    }
                }
            }
        }
    }

    pub(super) fn remove_midi_fx_slot(&mut self, slot_idx: usize) {
        if slot_idx < self.track_params.midi_fx_chain.len() {
            self.track_params.midi_fx_chain.remove(slot_idx);
        }
        if slot_idx >= self.midi_fx_slots.len() {
            return;
        }
        for idx in slot_idx..self.midi_fx_slots.len().saturating_sub(1) {
            self.midi_fx_slots[idx] = self.midi_fx_slots[idx + 1].clone();
        }
        if let Some(last) = self.midi_fx_slots.last_mut() {
            last.clear();
        }
    }

    pub(super) fn insert_midi_fx_slot(&mut self, slot_idx: usize, name: String, desc: &EffectDescriptor) {
        let insert_idx = slot_idx.min(self.track_params.midi_fx_chain.len());
        self.track_params.midi_fx_chain.insert(insert_idx, name);
        if insert_idx >= self.midi_fx_slots.len() {
            return;
        }
        for idx in (insert_idx + 1..self.midi_fx_slots.len()).rev() {
            self.midi_fx_slots[idx] = self.midi_fx_slots[idx - 1].clone();
        }
        self.midi_fx_slots[insert_idx].sync_to_descriptor(desc, 0);
    }

    pub(super) fn move_midi_fx_slot_to(&mut self, source_slot: usize, target_slot: usize) {
        if source_slot >= self.track_params.midi_fx_chain.len() {
            return;
        }
        let target_slot = target_slot.min(self.track_params.midi_fx_chain.len().saturating_sub(1));
        if source_slot == target_slot {
            return;
        }
        let name = self.track_params.midi_fx_chain.remove(source_slot);
        self.track_params.midi_fx_chain.insert(target_slot, name);
        if source_slot >= self.midi_fx_slots.len() || target_slot >= self.midi_fx_slots.len() {
            return;
        }
        let entry = self.midi_fx_slots.remove(source_slot);
        self.midi_fx_slots.insert(target_slot, entry);
        while self.midi_fx_slots.len() <= target_slot.max(source_slot) {
            self.midi_fx_slots.push(EffectSlotSnapshot::new_empty());
        }
    }

    pub(super) fn clear(
        &mut self,
        track: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        instrument_type: InstrumentType,
    ) {
        self.track_bits = [0u64; TRACK_PATTERN_WORDS];
        self.neural_reset_bits = [0u64; TRACK_PATTERN_WORDS];
        self.step_data = PatternSnapshot::default_step_data();
        self.track_params = TrackParamsSnapshot::default();
        self.effect_slots = PatternSnapshot::default_effect_slots(track, slot_descriptors);
        self.midi_fx_slots = PatternSnapshot::default_midi_fx_slots();
        self.instrument_slot = PatternSnapshot::default_instrument_slot();
        self.instrument_base_note_offset = 0.0;
        self.track_sound_state = TrackSoundState::default();
        self.sample_id = (-1, String::new(), 44_100);
        self.chord_snapshot = ChordSnapshot::new_default();
        self.timebase_plock_snapshot = [None; MAX_STEPS];
        self.swing_plock_snapshot = [None; MAX_STEPS];
        self.swing_resolution_plock_snapshot = [None; MAX_STEPS];
        self.instrument_type = instrument_type;
        self.instrument_run_mode = CustomInstrumentRunMode::Instrument;
        self.rack_track = None;
        self.plock_variant_registry = PlockVariantRegistry::default();
    }

    pub(super) fn default_step_params() -> [f32; NUM_PARAMS] {
        let mut params = [0.0f32; NUM_PARAMS];
        for param in StepParam::ALL {
            params[param.index()] = param.default_value();
        }
        params
    }
}
