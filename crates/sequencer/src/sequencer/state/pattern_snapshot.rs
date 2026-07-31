use super::*;

#[derive(Clone)]
pub struct PatternSnapshot {
    pub track_bits: Vec<[u64; TRACK_PATTERN_WORDS]>,
    pub neural_reset_bits: Vec<[u64; TRACK_PATTERN_WORDS]>,
    pub step_data: Vec<Vec<[f32; NUM_PARAMS]>>,
    pub track_params: Vec<TrackParamsSnapshot>,
    pub effect_slots: Vec<Vec<EffectSlotSnapshot>>,
    pub midi_fx_slots: Vec<Vec<EffectSlotSnapshot>>,
    pub instrument_slots: Vec<EffectSlotSnapshot>,
    pub instrument_base_note_offsets: Vec<f32>,
    pub track_sound_states: Vec<TrackSoundState>,
    pub sample_ids: Vec<(i32, String, u32)>,
    pub chord_snapshots: Vec<ChordSnapshot>,
    pub timebase_plock_snapshots: Vec<[Option<u32>; MAX_STEPS]>,
    pub swing_plock_snapshots: Vec<[Option<u32>; MAX_STEPS]>,
    pub swing_resolution_plock_snapshots: Vec<[Option<u32>; MAX_STEPS]>,
    pub instrument_types: Vec<InstrumentType>,
    pub instrument_run_modes: Vec<CustomInstrumentRunMode>,
    pub mod_connections: Vec<ModConnection>,
    pub neural_networks: Vec<ProjectNeuralNetwork>,
    pub graph_overrides: Vec<ProjectGraphOverrides>,
    pub rack_tracks: Vec<Option<RackTrackSnapshot>>,
    pub process_chains: Vec<crate::process::TrackProcessChain>,
    pub project_process_lane_overrides: Vec<crate::process::ProjectLaneOverrides>,
    pub project_process_chain: crate::process::TrackProcessChain,
    pub plock_variant_registries: Vec<PlockVariantRegistry>,
    pub key_lock_variant_registries: Vec<PlockVariantRegistry>,
}

#[derive(Clone, Debug)]
pub struct RackTrackSnapshot {
    pub routing: RackRouting,
    pub slots: Vec<RackSlotSnapshot>,
    pub macros: Vec<RackMacro>,
    pub(crate) runtime_macro_values: Option<Arc<RackMacroRuntimeValues>>,
    pub(crate) runtime_macro_track: usize,
}

impl RackTrackSnapshot {
    pub fn new(routing: RackRouting, slots: Vec<RackSlotSnapshot>, macros: Vec<RackMacro>) -> Self {
        Self {
            routing,
            slots,
            macros,
            runtime_macro_values: None,
            runtime_macro_track: 0,
        }
    }

    pub fn normalize_macros(&mut self) {
        let mut normalized = default_rack_macros();
        for rack_macro in std::mem::take(&mut self.macros) {
            if let Some(target) = normalized.get_mut(rack_macro.id.index()) {
                *target = rack_macro;
                target.value = target.value.clamp(0.0, 1.0);
                target.plocks.resize(MAX_STEPS, None);
                target.plocks.truncate(MAX_STEPS);
            }
        }
        self.macros = normalized;
    }

    pub(crate) fn attach_runtime_macro_values(
        &mut self,
        values: Arc<RackMacroRuntimeValues>,
        track: usize,
    ) {
        self.runtime_macro_values = Some(values);
        self.runtime_macro_track = track;
    }

    pub(crate) fn runtime_macro_value_at(&self, id: RackMacroId, step: usize) -> Option<f32> {
        self.runtime_macro_values
            .as_ref()
            .and_then(|values| values.value_at(self.runtime_macro_track, id, step))
    }
}

/// Pointer-rate rack macro values shared by immutable scheduler snapshots.
///
/// Rack topology and mappings remain snapshot-owned, but default values and
/// per-step locks are fixed-size scalar control data. Keeping those scalars in
/// atomics lets the command thread publish a knob drag immediately without
/// deep-cloning every rack slot, effect descriptor, and p-lock grid.
#[derive(Debug)]
pub(crate) struct RackMacroRuntimeValues {
    pub(super) defaults: Vec<[AtomicU32; RACK_MACRO_COUNT]>,
    pub(super) plocks: Vec<Box<[AtomicU64]>>,
}

impl RackMacroRuntimeValues {
    pub(super) fn new() -> Self {
        Self {
            defaults: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())))
                .collect(),
            plocks: (0..MAX_TRACKS)
                .map(|_| {
                    (0..RACK_MACRO_COUNT * MAX_STEPS)
                        .map(|_| AtomicU64::new(0))
                        .collect::<Vec<_>>()
                        .into_boxed_slice()
                })
                .collect(),
        }
    }

    pub(super) fn encode_plock(value: Option<f32>) -> u64 {
        value.map_or(0, |value| (1_u64 << 32) | u64::from(value.to_bits()))
    }

    pub(super) fn decode_plock(value: u64) -> Option<f32> {
        (value >> 32 != 0).then(|| f32::from_bits(value as u32))
    }

    pub(super) fn set_default(&self, track: usize, id: RackMacroId, value: f32) {
        if let Some(defaults) = self.defaults.get(track) {
            defaults[id.index()].store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub(super) fn set_plock(&self, track: usize, id: RackMacroId, step: usize, value: Option<f32>) {
        let Some(plocks) = self.plocks.get(track) else {
            return;
        };
        let Some(cell) = plocks.get(id.index() * MAX_STEPS + step) else {
            return;
        };
        cell.store(Self::encode_plock(value), Ordering::Relaxed);
    }

    pub(super) fn value_at(&self, track: usize, id: RackMacroId, step: usize) -> Option<f32> {
        let defaults = self.defaults.get(track)?;
        let plocks = self.plocks.get(track)?;
        let plock = plocks
            .get(id.index() * MAX_STEPS + step)
            .and_then(|cell| Self::decode_plock(cell.load(Ordering::Relaxed)));
        Some(plock.unwrap_or_else(|| f32::from_bits(defaults[id.index()].load(Ordering::Relaxed))))
    }

    pub(super) fn sync_track(&self, track: usize, rack: Option<&RackTrackSnapshot>) {
        for index in 0..RACK_MACRO_COUNT {
            let Some(id) = RackMacroId::from_index(index) else {
                continue;
            };
            let rack_macro = rack.and_then(|rack| rack.macros.get(index));
            self.set_default(track, id, rack_macro.map_or(0.0, |item| item.value));
            for step in 0..MAX_STEPS {
                self.set_plock(
                    track,
                    id,
                    step,
                    rack_macro.and_then(|item| item.plocks.get(step).copied().flatten()),
                );
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RackSlotSnapshot {
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    pub instrument_base_note_offset: f32,
    pub pad_note: Option<i32>,
    pub choke_group: Option<u8>,
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub max_polyphony: usize,
    pub param_plocks: RackSlotParamPlocks,
    pub instrument_slot: EffectSlotSnapshot,
    pub effect_slots: Vec<EffectSlotSnapshot>,
    pub effect_descriptors: Vec<EffectDescriptor>,
    pub custom_effect_names: Vec<Option<String>>,
    pub track_sound_state: TrackSoundState,
    pub sample_id: Option<(i32, String, u32)>,
}

#[derive(Clone, Debug)]
pub struct InstrumentDeviceValuesSnapshot {
    pub slot: EffectSlotValuesSnapshot,
    pub base_note_offset_bits: u32,
    pub sound_state: TrackSoundState,
    pub key_lock_variant_registry: PlockVariantRegistry,
}

impl InstrumentDeviceValuesSnapshot {
    pub fn bit_exact_eq(&self, other: &Self) -> bool {
        self.slot.bit_exact_eq(&other.slot)
            && self.base_note_offset_bits == other.base_note_offset_bits
            && self.sound_state.engine_id == other.sound_state.engine_id
            && self.sound_state.loaded_preset == other.sound_state.loaded_preset
            && self.sound_state.dirty == other.sound_state.dirty
            && self.key_lock_variant_registry == other.key_lock_variant_registry
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.slot.retained_bytes()
            + self
                .sound_state
                .loaded_preset
                .as_ref()
                .map_or(0, String::capacity)
    }
}

#[derive(Clone, Debug)]
pub struct RackSlotValuesSnapshot {
    pub base_note_offset_bits: u32,
    pub choke_group: Option<u8>,
    pub gain_bits: u32,
    pub pan_bits: u32,
    pub mute: bool,
    pub solo: bool,
    pub max_polyphony: usize,
    pub param_plocks: RackSlotParamPlocks,
    pub instrument_slot: EffectSlotValuesSnapshot,
    pub effect_slots: Vec<EffectSlotValuesSnapshot>,
    pub sound_state: TrackSoundState,
}

impl RackSlotValuesSnapshot {
    pub fn bit_exact_eq(&self, other: &Self) -> bool {
        self.base_note_offset_bits == other.base_note_offset_bits
            && self.choke_group == other.choke_group
            && self.gain_bits == other.gain_bits
            && self.pan_bits == other.pan_bits
            && self.mute == other.mute
            && self.solo == other.solo
            && self.max_polyphony == other.max_polyphony
            && optional_f32_rows_bit_exact_eq(
                &self.param_plocks.rows,
                &other.param_plocks.rows,
            )
            && self.instrument_slot.bit_exact_eq(&other.instrument_slot)
            && self.effect_slots.len() == other.effect_slots.len()
            && self
                .effect_slots
                .iter()
                .zip(&other.effect_slots)
                .all(|(left, right)| left.bit_exact_eq(right))
            && self.sound_state.engine_id == other.sound_state.engine_id
            && self.sound_state.loaded_preset == other.sound_state.loaded_preset
            && self.sound_state.dirty == other.sound_state.dirty
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.param_plocks.rows.capacity() * std::mem::size_of::<Vec<Option<f32>>>()
            + self
                .param_plocks
                .rows
                .iter()
                .map(|row| row.capacity() * std::mem::size_of::<Option<f32>>())
                .sum::<usize>()
            + self.instrument_slot.retained_bytes()
            + self
                .effect_slots
                .iter()
                .map(EffectSlotValuesSnapshot::retained_bytes)
                .sum::<usize>()
            + self
                .sound_state
                .loaded_preset
                .as_ref()
                .map_or(0, String::capacity)
    }
}

pub(super) fn optional_f32_rows_bit_exact_eq(
    left: &[Vec<Option<f32>>],
    right: &[Vec<Option<f32>>],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.len() == right.len()
                && left.iter().zip(right).all(|(left, right)| match (left, right) {
                    (Some(left), Some(right)) => left.to_bits() == right.to_bits(),
                    (None, None) => true,
                    _ => false,
                })
        })
}

impl RackSlotSnapshot {
    pub fn authoring_values(&self) -> RackSlotValuesSnapshot {
        RackSlotValuesSnapshot {
            base_note_offset_bits: self.instrument_base_note_offset.to_bits(),
            choke_group: self.choke_group,
            gain_bits: self.gain.to_bits(),
            pan_bits: self.pan.to_bits(),
            mute: self.mute,
            solo: self.solo,
            max_polyphony: self.max_polyphony,
            param_plocks: self.param_plocks.clone(),
            instrument_slot: self.instrument_slot.authoring_values(),
            effect_slots: self
                .effect_slots
                .iter()
                .map(EffectSlotSnapshot::authoring_values)
                .collect(),
            sound_state: self.track_sound_state.clone(),
        }
    }

    pub fn apply_authoring_values(
        &mut self,
        values: &RackSlotValuesSnapshot,
    ) -> Result<(), String> {
        let mut instrument_slot = self.instrument_slot.clone();
        instrument_slot.apply_authoring_values(&values.instrument_slot)?;
        if self.effect_slots.len() != values.effect_slots.len() {
            return Err("rack effect chain changed while replaying history".to_string());
        }
        let mut effect_slots = self.effect_slots.clone();
        for (slot, values) in effect_slots.iter_mut().zip(&values.effect_slots) {
            slot.apply_authoring_values(values)?;
        }
        self.instrument_base_note_offset = f32::from_bits(values.base_note_offset_bits);
        self.choke_group = values.choke_group;
        self.gain = f32::from_bits(values.gain_bits);
        self.pan = f32::from_bits(values.pan_bits);
        self.mute = values.mute;
        self.solo = values.solo;
        self.max_polyphony = values.max_polyphony;
        self.param_plocks = values.param_plocks.clone();
        self.instrument_slot = instrument_slot;
        self.effect_slots = effect_slots;
        self.track_sound_state = values.sound_state.clone();
        Ok(())
    }

    pub fn empty_effect_slots() -> Vec<EffectSlotSnapshot> {
        (0..crate::lisp_host::MAX_CUSTOM_FX)
            .map(|_| EffectSlotSnapshot::new_empty())
            .collect()
    }

    pub fn empty_effect_names() -> Vec<Option<String>> {
        vec![None; crate::lisp_host::MAX_CUSTOM_FX]
    }

    pub fn normalize_effect_chain(&mut self) {
        self.effect_slots.resize_with(
            crate::lisp_host::MAX_CUSTOM_FX,
            EffectSlotSnapshot::new_empty,
        );
        self.effect_slots.truncate(crate::lisp_host::MAX_CUSTOM_FX);
        self.effect_descriptors.resize_with(
            crate::lisp_host::MAX_CUSTOM_FX,
            EffectDescriptor::empty_custom_slot,
        );
        self.effect_descriptors
            .truncate(crate::lisp_host::MAX_CUSTOM_FX);
        self.custom_effect_names
            .resize(crate::lisp_host::MAX_CUSTOM_FX, None);
        self.custom_effect_names
            .truncate(crate::lisp_host::MAX_CUSTOM_FX);
    }

    pub fn param_default(&self, param: RackSlotParam) -> f32 {
        match param {
            RackSlotParam::BaseNote => self.instrument_base_note_offset,
            RackSlotParam::Gain => self.gain,
            RackSlotParam::Pan => self.pan,
            RackSlotParam::MaxPolyphony => self.max_polyphony as f32,
            RackSlotParam::Mute => {
                if self.mute {
                    1.0
                } else {
                    0.0
                }
            }
            RackSlotParam::Solo => {
                if self.solo {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    pub fn param_value_at_step(&self, param: RackSlotParam, step: usize) -> f32 {
        self.param_plocks
            .get(step, param)
            .unwrap_or_else(|| self.param_default(param))
    }

    pub fn set_param_plock(&mut self, step: usize, param: RackSlotParam, value: f32) -> bool {
        self.param_plocks.set(step, param, value)
    }
}

pub(super) fn prepare_track_pattern_data_for_rack(data: &mut TrackPatternData) {
    data.instrument_type = InstrumentType::Rack;
    data.instrument_run_mode = CustomInstrumentRunMode::Instrument;
    data.instrument_slot = EffectSlotSnapshot::new_empty();
    data.instrument_base_note_offset = 0.0;
    data.track_sound_state.engine_id = None;
}

/// Patch-entity twin of `prepare_track_pattern_data_for_rack` (§17.2: the
/// instrument binding lives on the Patch).
pub(super) fn prepare_patch_for_rack(patch: &mut Patch) {
    patch.instrument_type = InstrumentType::Rack;
    patch.instrument_run_mode = CustomInstrumentRunMode::Instrument;
    patch.instrument_slot = EffectSlotSnapshot::new_empty();
    patch.instrument_base_note_offset = 0.0;
    patch.track_sound_state.engine_id = None;
}

pub(super) fn replace_rack_slot_source_preserving_controls(
    slot: &mut RackSlotSnapshot,
    replacement: &RackSlotSnapshot,
) {
    let pad_note = slot.pad_note;
    let choke_group = slot.choke_group;
    let instrument_base_note_offset = slot.instrument_base_note_offset;
    let gain = slot.gain;
    let pan = slot.pan;
    let mute = slot.mute;
    let solo = slot.solo;
    let max_polyphony = slot.max_polyphony;
    let param_plocks = slot.param_plocks.clone();
    let effect_slots = slot.effect_slots.clone();
    let effect_descriptors = slot.effect_descriptors.clone();
    let custom_effect_names = slot.custom_effect_names.clone();

    *slot = replacement.clone();
    slot.pad_note = pad_note;
    slot.choke_group = choke_group;
    slot.instrument_base_note_offset = instrument_base_note_offset;
    slot.gain = gain;
    slot.pan = pan;
    slot.mute = mute;
    slot.solo = solo;
    slot.max_polyphony = max_polyphony;
    slot.param_plocks = param_plocks;
    slot.effect_slots = effect_slots;
    slot.effect_descriptors = effect_descriptors;
    slot.custom_effect_names = custom_effect_names;
}

impl PatternSnapshot {
    pub fn refresh_process_binding_param_ids(
        &mut self,
        effect_descriptors: &[Vec<EffectDescriptor>],
        instrument_descriptors: &[EffectDescriptor],
    ) {
        for track in 0..self.process_chains.len() {
            crate::process::refresh_track_process_chain_binding_param_ids(
                &mut self.process_chains[track],
                instrument_descriptors.get(track),
                self.instrument_slots.get(track),
                effect_descriptors
                    .get(track)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                self.effect_slots
                    .get(track)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
        }
    }

    /// Delete one track lane from this snapshot and compact higher track indices.
    ///
    /// This is the snapshot-side half of track deletion semantics:
    /// deleting a track removes it from every track-indexed lane immediately,
    /// and every track after it shifts down by one. The live delete path will
    /// pair this with graph teardown, live-state compaction, and UI refresh.
    pub fn remove_track(&mut self, track_idx: usize) {
        if track_idx >= self.track_bits.len() {
            return;
        }

        self.track_bits.remove(track_idx);
        remove_track_lane_if_present(&mut self.neural_reset_bits, track_idx);
        remove_track_lane_if_present(&mut self.step_data, track_idx);
        remove_track_lane_if_present(&mut self.track_params, track_idx);
        remove_track_lane_if_present(&mut self.effect_slots, track_idx);
        remove_track_lane_if_present(&mut self.midi_fx_slots, track_idx);
        remove_track_lane_if_present(&mut self.instrument_slots, track_idx);
        remove_track_lane_if_present(&mut self.instrument_base_note_offsets, track_idx);
        remove_track_lane_if_present(&mut self.track_sound_states, track_idx);
        remove_track_lane_if_present(&mut self.sample_ids, track_idx);
        remove_track_lane_if_present(&mut self.chord_snapshots, track_idx);
        remove_track_lane_if_present(&mut self.timebase_plock_snapshots, track_idx);
        remove_track_lane_if_present(&mut self.swing_plock_snapshots, track_idx);
        remove_track_lane_if_present(&mut self.swing_resolution_plock_snapshots, track_idx);
        remove_track_lane_if_present(&mut self.instrument_types, track_idx);
        remove_track_lane_if_present(&mut self.instrument_run_modes, track_idx);
        remove_track_lane_if_present(&mut self.rack_tracks, track_idx);
        remove_track_lane_if_present(&mut self.process_chains, track_idx);
        remove_track_lane_if_present(&mut self.project_process_lane_overrides, track_idx);
        remove_track_lane_if_present(&mut self.plock_variant_registries, track_idx);
        remove_track_lane_if_present(&mut self.key_lock_variant_registries, track_idx);
        self.mod_connections = self
            .mod_connections
            .iter()
            .filter_map(|connection| {
                remap_mod_connection_after_track_delete(*connection, track_idx)
            })
            .collect();
        remap_neural_network_routes_after_track_delete(&mut self.neural_networks, track_idx);
        remap_graph_overrides_after_track_delete(&mut self.graph_overrides, track_idx);
    }

    pub fn remove_effect_slot(&mut self, track: usize, slot_idx: usize) {
        let Some(slots) = self.effect_slots.get_mut(track) else {
            return;
        };
        if slot_idx >= slots.len() {
            return;
        }

        for idx in slot_idx..slots.len().saturating_sub(1) {
            slots[idx] = slots[idx + 1].clone();
        }
        if let Some(last) = slots.last_mut() {
            last.clear();
        }
    }

    pub fn insert_empty_effect_slot(&mut self, track: usize, slot_idx: usize) {
        let Some(slots) = self.effect_slots.get_mut(track) else {
            return;
        };
        if slot_idx >= slots.len() {
            return;
        }
        for idx in (slot_idx + 1..slots.len()).rev() {
            slots[idx] = slots[idx - 1].clone();
        }
        slots[slot_idx].clear();
    }

    pub fn move_effect_slot_to(&mut self, track: usize, source_slot: usize, target_slot: usize) {
        let Some(slots) = self.effect_slots.get_mut(track) else {
            return;
        };
        if source_slot >= slots.len() || target_slot >= slots.len() || source_slot == target_slot {
            return;
        }
        let entry = slots.remove(source_slot);
        slots.insert(target_slot, entry);
        while slots.len() <= target_slot.max(source_slot) {
            slots.push(EffectSlotSnapshot::new_empty());
        }
    }

    pub fn remove_midi_fx_slot(&mut self, track: usize, slot_idx: usize) {
        let Some(params) = self.track_params.get_mut(track) else {
            return;
        };
        if slot_idx < params.midi_fx_chain.len() {
            params.midi_fx_chain.remove(slot_idx);
        }

        let Some(slots) = self.midi_fx_slots.get_mut(track) else {
            return;
        };
        if slot_idx >= slots.len() {
            return;
        }

        for idx in slot_idx..slots.len().saturating_sub(1) {
            slots[idx] = slots[idx + 1].clone();
        }
        if let Some(last) = slots.last_mut() {
            last.clear();
        }
    }

    pub fn insert_midi_fx_slot(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: String,
        desc: &EffectDescriptor,
    ) {
        let Some(params) = self.track_params.get_mut(track) else {
            return;
        };
        let insert_idx = slot_idx.min(params.midi_fx_chain.len());
        params.midi_fx_chain.insert(insert_idx, name);

        let Some(slots) = self.midi_fx_slots.get_mut(track) else {
            return;
        };
        if insert_idx >= slots.len() {
            return;
        }
        for idx in (insert_idx + 1..slots.len()).rev() {
            slots[idx] = slots[idx - 1].clone();
        }
        slots[insert_idx].sync_to_descriptor(desc, 0);
    }

    pub fn move_midi_fx_slot_to(&mut self, track: usize, source_slot: usize, target_slot: usize) {
        let Some(params) = self.track_params.get_mut(track) else {
            return;
        };
        if source_slot >= params.midi_fx_chain.len() {
            return;
        }
        let target_slot = target_slot.min(params.midi_fx_chain.len().saturating_sub(1));
        if source_slot == target_slot {
            return;
        }
        let name = params.midi_fx_chain.remove(source_slot);
        params.midi_fx_chain.insert(target_slot, name);

        let Some(slots) = self.midi_fx_slots.get_mut(track) else {
            return;
        };
        if source_slot >= slots.len() || target_slot >= slots.len() {
            return;
        }
        let entry = slots.remove(source_slot);
        slots.insert(target_slot, entry);
        while slots.len() <= target_slot.max(source_slot) {
            slots.push(EffectSlotSnapshot::new_empty());
        }
    }

    /// Normalize every track-indexed lane to the exact live track count.
    ///
    /// Project files can be older than the current in-memory snapshot shape, or
    /// can legitimately omit lanes for newly-added track-scoped data. Restoring
    /// a partial snapshot is unsafe because any missing lane would leave the
    /// previous live project's lane intact. This method makes the snapshot a
    /// complete replacement before it is committed to the pattern bank.
    pub fn normalize_track_count(
        &mut self,
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
    ) {
        self.truncate_tracks(track_count);
        while self.track_bits.len() < track_count {
            let track = self.track_bits.len();
            self.track_bits.push([0u64; TRACK_PATTERN_WORDS]);
            self.neural_reset_bits.push([0u64; TRACK_PATTERN_WORDS]);
            self.step_data.push(Self::default_step_data());
            self.track_params.push(TrackParamsSnapshot::default());
            self.effect_slots
                .push(Self::default_effect_slots(track, slot_descriptors));
            self.midi_fx_slots.push(Self::default_midi_fx_slots());
            self.instrument_slots.push(Self::default_instrument_slot());
            self.instrument_base_note_offsets.push(0.0);
            self.track_sound_states.push(TrackSoundState::default());
            self.sample_ids.push((-1, String::new(), 44_100));
            self.chord_snapshots.push(ChordSnapshot::new_default());
            self.timebase_plock_snapshots.push([None; MAX_STEPS]);
            self.swing_plock_snapshots.push([None; MAX_STEPS]);
            self.swing_resolution_plock_snapshots
                .push([None; MAX_STEPS]);
            self.instrument_types.push(InstrumentType::Sampler);
            self.instrument_run_modes
                .push(CustomInstrumentRunMode::Instrument);
            self.rack_tracks.push(None);
            self.process_chains
                .push(crate::process::TrackProcessChain::default());
            self.plock_variant_registries
                .push(PlockVariantRegistry::default());
            self.key_lock_variant_registries
                .push(PlockVariantRegistry::default());
        }
        while self.neural_reset_bits.len() < track_count {
            self.neural_reset_bits.push([0u64; TRACK_PATTERN_WORDS]);
        }
        while self.rack_tracks.len() < track_count {
            self.rack_tracks.push(None);
        }
        while self.process_chains.len() < track_count {
            self.process_chains
                .push(crate::process::TrackProcessChain::default());
        }
        while self.project_process_lane_overrides.len() < track_count {
            self.project_process_lane_overrides.push(Default::default());
        }
        while self.plock_variant_registries.len() < track_count {
            self.plock_variant_registries
                .push(PlockVariantRegistry::default());
        }
        while self.key_lock_variant_registries.len() < track_count {
            self.key_lock_variant_registries
                .push(PlockVariantRegistry::default());
        }
        self.mod_connections.retain(|connection| {
            connection.source_track < track_count
                && mod_destination_valid_for_track_count(connection.destination, track_count)
                && connection.dest_input < EXT_MOD_INPUT_COUNT
        });

        for steps in &mut self.step_data {
            steps.truncate(MAX_STEPS);
            while steps.len() < MAX_STEPS {
                let mut params = [0.0f32; NUM_PARAMS];
                for param in StepParam::ALL {
                    params[param.index()] = param.default_value();
                }
                steps.push(params);
            }
        }
    }

    pub(super) fn truncate_tracks(&mut self, track_count: usize) {
        self.track_bits.truncate(track_count);
        self.neural_reset_bits.truncate(track_count);
        self.step_data.truncate(track_count);
        self.track_params.truncate(track_count);
        self.effect_slots.truncate(track_count);
        self.midi_fx_slots.truncate(track_count);
        self.instrument_slots.truncate(track_count);
        self.instrument_base_note_offsets.truncate(track_count);
        self.track_sound_states.truncate(track_count);
        self.sample_ids.truncate(track_count);
        self.chord_snapshots.truncate(track_count);
        self.timebase_plock_snapshots.truncate(track_count);
        self.swing_plock_snapshots.truncate(track_count);
        self.swing_resolution_plock_snapshots.truncate(track_count);
        self.instrument_types.truncate(track_count);
        self.instrument_run_modes.truncate(track_count);
        self.rack_tracks.truncate(track_count);
        self.process_chains.truncate(track_count);
        self.project_process_lane_overrides.truncate(track_count);
        self.plock_variant_registries.truncate(track_count);
        self.key_lock_variant_registries.truncate(track_count);
        self.mod_connections.retain(|connection| {
            connection.source_track < track_count
                && mod_destination_valid_for_track_count(connection.destination, track_count)
                && connection.dest_input < EXT_MOD_INPUT_COUNT
        });
    }

    pub fn capture(
        state: &SequencerState,
        num_tracks: usize,
        track_buffer_ids: &[i32],
        track_sample_rates: &[u32],
        track_names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Self {
        let mut track_bits = Vec::with_capacity(num_tracks);
        let mut neural_reset_bits = Vec::with_capacity(num_tracks);
        let mut step_data = Vec::with_capacity(num_tracks);
        let mut track_params = Vec::with_capacity(num_tracks);
        let mut effect_slots = Vec::with_capacity(num_tracks);
        let mut midi_fx_slots = Vec::with_capacity(num_tracks);
        let mut instrument_slots = Vec::with_capacity(num_tracks);
        let mut instrument_base_note_offsets = Vec::with_capacity(num_tracks);
        let track_sound_state = state.pattern.track_sound_state.lock().unwrap();
        let mut sound_states = Vec::with_capacity(num_tracks);
        let mut sample_ids = Vec::with_capacity(num_tracks);
        let mut chord_snapshots = Vec::with_capacity(num_tracks);
        let mut timebase_plock_snapshots = Vec::with_capacity(num_tracks);
        let mut swing_plock_snapshots = Vec::with_capacity(num_tracks);
        let mut swing_resolution_plock_snapshots = Vec::with_capacity(num_tracks);
        let mut inst_types = Vec::with_capacity(num_tracks);
        let mut instrument_run_modes = Vec::with_capacity(num_tracks);
        let mut rack_tracks = Vec::with_capacity(num_tracks);
        let mut process_chains = Vec::with_capacity(num_tracks);
        let mut plock_variant_registries = Vec::with_capacity(num_tracks);
        let mut key_lock_variant_registries = Vec::with_capacity(num_tracks);

        let scene_trace = {
            static SCENE_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *SCENE_TRACE.get_or_init(|| std::env::var("ESEQ_SCENE_TRACE").is_ok_and(|v| v == "1"))
        };
        let mut steps_elapsed = std::time::Duration::ZERO;
        let mut effects_elapsed = std::time::Duration::ZERO;
        let mut midi_elapsed = std::time::Duration::ZERO;
        let mut instrument_elapsed = std::time::Duration::ZERO;
        let mut rest_elapsed = std::time::Duration::ZERO;
        for t in 0..num_tracks {
            track_bits.push(state.pattern.patterns[t].load_bits());
            neural_reset_bits.push(state.pattern.neural_reset_patterns[t].load_bits());

            let started = Instant::now();
            step_data.push(state.pattern.step_data[t].load_rows());
            steps_elapsed += started.elapsed();

            let tp = &state.pattern.track_params[t];
            track_params.push(capture_track_params_snapshot(tp));

            let started = Instant::now();
            let chain: Vec<EffectSlotSnapshot> = state.pattern.effect_chains[t]
                .iter()
                .map(EffectSlotSnapshot::capture)
                .collect();
            effect_slots.push(chain);
            effects_elapsed += started.elapsed();
            let started = Instant::now();
            midi_fx_slots.push(
                state.pattern.midi_fx_slots[t]
                    .iter()
                    .map(EffectSlotSnapshot::capture)
                    .collect(),
            );
            midi_elapsed += started.elapsed();
            let started = Instant::now();
            instrument_slots.push(EffectSlotSnapshot::capture(
                &state.pattern.instrument_slots[t],
            ));
            instrument_elapsed += started.elapsed();
            let rest_started = Instant::now();
            instrument_base_note_offsets.push(f32::from_bits(
                state.pattern.instrument_base_note_offsets[t].load(Ordering::Relaxed),
            ));
            let mut sound = track_sound_state.get(t).cloned().unwrap_or_default();
            let engine_id = state.runtime.track_engine_ids[t].load(Ordering::Relaxed);
            sound.engine_id = if engine_id == u32::MAX {
                None
            } else {
                Some(engine_id as usize)
            };
            sound_states.push(sound);

            let buf_id = if t < track_buffer_ids.len() {
                track_buffer_ids[t]
            } else {
                -1
            };
            let name = if t < track_names.len() {
                track_names[t].clone()
            } else {
                String::new()
            };
            let sample_rate = if t < track_sample_rates.len() {
                track_sample_rates[t]
            } else {
                44_100
            };
            sample_ids.push((buf_id, name, sample_rate));
            chord_snapshots.push(ChordSnapshot::capture(&state.pattern.chord_data[t]));
            timebase_plock_snapshots.push(state.pattern.timebase_plocks[t].snapshot());
            swing_plock_snapshots.push(state.pattern.swing_plocks[t].snapshot());
            swing_resolution_plock_snapshots
                .push(state.pattern.swing_resolution_plocks[t].snapshot());
            inst_types.push(if t < instrument_types.len() {
                instrument_types[t]
            } else {
                InstrumentType::Sampler
            });
            instrument_run_modes.push(CustomInstrumentRunMode::from_runtime_flag(
                state.pattern.instrument_run_modes[t].load(Ordering::Relaxed),
            ));
            rack_tracks.push(
                state
                    .pattern
                    .rack_tracks
                    .lock()
                    .unwrap()
                    .get(t)
                    .cloned()
                    .unwrap_or(None),
            );
            process_chains.push(
                state
                    .pattern
                    .process_chains
                    .lock()
                    .unwrap()
                    .get(t)
                    .cloned()
                    .unwrap_or_default(),
            );
            let mut registry = state
                .pattern
                .plock_variant_registries
                .lock()
                .unwrap()
                .get(t)
                .cloned()
                .unwrap_or_default();
            registry.prune_to_keys(&live_track_variant_keys(state, t));
            plock_variant_registries.push(registry);
            let mut key_registry = state
                .pattern
                .key_lock_variant_registries
                .lock()
                .unwrap()
                .get(t)
                .cloned()
                .unwrap_or_default();
            key_registry.prune_to_keys(&live_track_key_lock_variant_keys(state, t));
            key_lock_variant_registries.push(key_registry);
            rest_elapsed += rest_started.elapsed();
        }
        if scene_trace {
            eprintln!(
                "[capture-trace] steps={:.3}ms effects={:.3}ms midi={:.3}ms instrument={:.3}ms rest={:.3}ms",
                steps_elapsed.as_secs_f64() * 1000.0,
                effects_elapsed.as_secs_f64() * 1000.0,
                midi_elapsed.as_secs_f64() * 1000.0,
                instrument_elapsed.as_secs_f64() * 1000.0,
                rest_elapsed.as_secs_f64() * 1000.0,
            );
        }

        Self {
            track_bits,
            neural_reset_bits,
            step_data,
            track_params,
            effect_slots,
            midi_fx_slots,
            instrument_slots,
            instrument_base_note_offsets,
            track_sound_states: sound_states,
            sample_ids,
            chord_snapshots,
            timebase_plock_snapshots,
            swing_plock_snapshots,
            swing_resolution_plock_snapshots,
            instrument_types: inst_types,
            instrument_run_modes,
            mod_connections: Vec::new(),
            neural_networks: Vec::new(),
            graph_overrides: Vec::new(),
            rack_tracks,
            process_chains,
            project_process_lane_overrides: state
                .pattern
                .project_process_lane_overrides
                .lock()
                .unwrap()
                .iter()
                .take(num_tracks)
                .cloned()
                .collect(),
            project_process_chain: crate::process::TrackProcessChain::default(),
            plock_variant_registries,
            key_lock_variant_registries,
        }
    }

    pub fn capture_with_mod_connections(
        state: &SequencerState,
        num_tracks: usize,
        track_buffer_ids: &[i32],
        track_sample_rates: &[u32],
        track_names: &[String],
        instrument_types: &[InstrumentType],
        mod_connections: Vec<ModConnection>,
        neural_networks: Vec<ProjectNeuralNetwork>,
        graph_overrides: Vec<ProjectGraphOverrides>,
    ) -> Self {
        let mut snapshot = Self::capture(
            state,
            num_tracks,
            track_buffer_ids,
            track_sample_rates,
            track_names,
            instrument_types,
        );
        snapshot.mod_connections = mod_connections;
        snapshot.neural_networks = neural_networks;
        snapshot.graph_overrides = graph_overrides;
        snapshot
    }

    pub fn restore(&self, state: &SequencerState) {
        for track in 0..self.track_bits.len() {
            self.restore_track(state, track);
        }
    }

    /// Whole-grid restores relocate lanes (track delete/reorder compaction),
    /// so the snapshot's graph identity is adopted verbatim — the live slot
    /// at this index belonged to a different track before the shift.
    pub fn restore_track(&self, state: &SequencerState, track: usize) -> bool {
        let Some(data) = self.track_pattern_data(track) else {
            return false;
        };
        data.restore_to_adopting_snapshot_identity(state, track)
    }

    /// Replace one lane's device/sound half with `data`'s — instruments,
    /// effects, MIDI FX, rack, process chain, and the device+mixer track
    /// params — leaving the lane's step content and step-grid fields
    /// (`num_steps`, timebase, swing) untouched. Used to make a capture
    /// TRUTHFUL for a borrowed lane (takes spec 18.1 step 3): the mirror
    /// holds the bound source's devices, and the scene save-back must
    /// persist the scene-effective sound instead.
    pub fn overwrite_track_device_state(&mut self, track: usize, data: &TrackPatternData) {
        let Some(params) = self.track_params.get_mut(track) else {
            return;
        };
        let mut device_params = data.track_params.clone();
        device_params.num_steps = params.num_steps;
        device_params.timebase = params.timebase;
        device_params.swing = params.swing;
        device_params.swing_resolution = params.swing_resolution;
        *params = device_params;
        if let Some(slots) = self.effect_slots.get_mut(track) {
            *slots = data.effect_slots.clone();
        }
        if let Some(slots) = self.midi_fx_slots.get_mut(track) {
            *slots = data.midi_fx_slots.clone();
        }
        if let Some(slot) = self.instrument_slots.get_mut(track) {
            *slot = data.instrument_slot.clone();
        }
        if let Some(offset) = self.instrument_base_note_offsets.get_mut(track) {
            *offset = data.instrument_base_note_offset;
        }
        if let Some(state) = self.track_sound_states.get_mut(track) {
            *state = data.track_sound_state.clone();
        }
        if let Some(sample) = self.sample_ids.get_mut(track) {
            *sample = data.sample_id.clone();
        }
        if let Some(instrument_type) = self.instrument_types.get_mut(track) {
            *instrument_type = data.instrument_type;
        }
        if let Some(run_mode) = self.instrument_run_modes.get_mut(track) {
            *run_mode = data.instrument_run_mode;
        }
        if let Some(rack) = self.rack_tracks.get_mut(track) {
            *rack = data.rack_track.clone();
        }
        if let Some(chain) = self.process_chains.get_mut(track) {
            *chain = data.process_chain.clone();
        }
    }

    pub fn track_pattern_data(&self, track: usize) -> Option<TrackPatternData> {
        Some(TrackPatternData {
            track_bits: *self.track_bits.get(track)?,
            neural_reset_bits: self
                .neural_reset_bits
                .get(track)
                .copied()
                .unwrap_or([0u64; TRACK_PATTERN_WORDS]),
            step_data: self.step_data.get(track)?.clone(),
            track_params: self.track_params.get(track)?.clone(),
            effect_slots: self.effect_slots.get(track)?.clone(),
            midi_fx_slots: self.midi_fx_slots.get(track).cloned().unwrap_or_else(|| {
                vec![EffectSlotSnapshot::new_empty(); crate::lisp_host::MAX_MIDI_FX_SLOTS]
            }),
            instrument_slot: self
                .instrument_slots
                .get(track)
                .cloned()
                .unwrap_or_else(EffectSlotSnapshot::new_empty),
            instrument_base_note_offset: self
                .instrument_base_note_offsets
                .get(track)
                .copied()
                .unwrap_or(0.0),
            track_sound_state: self
                .track_sound_states
                .get(track)
                .cloned()
                .unwrap_or_default(),
            sample_id: self
                .sample_ids
                .get(track)
                .cloned()
                .unwrap_or((-1, String::new(), 44_100)),
            chord_snapshot: self
                .chord_snapshots
                .get(track)
                .cloned()
                .unwrap_or_else(ChordSnapshot::new_default),
            timebase_plock_snapshot: self
                .timebase_plock_snapshots
                .get(track)
                .copied()
                .unwrap_or([None; MAX_STEPS]),
            swing_plock_snapshot: self
                .swing_plock_snapshots
                .get(track)
                .copied()
                .unwrap_or([None; MAX_STEPS]),
            swing_resolution_plock_snapshot: self
                .swing_resolution_plock_snapshots
                .get(track)
                .copied()
                .unwrap_or([None; MAX_STEPS]),
            instrument_type: self
                .instrument_types
                .get(track)
                .copied()
                .unwrap_or(InstrumentType::Sampler),
            instrument_run_mode: self
                .instrument_run_modes
                .get(track)
                .copied()
                .unwrap_or(CustomInstrumentRunMode::Instrument),
            rack_track: self.rack_tracks.get(track).cloned().unwrap_or(None),
            process_chain: self.process_chains.get(track).cloned().unwrap_or_default(),
            project_process_lane_overrides: self
                .project_process_lane_overrides
                .get(track)
                .cloned()
                .unwrap_or_default(),
            plock_variant_registry: self
                .plock_variant_registries
                .get(track)
                .cloned()
                .unwrap_or_default(),
            key_lock_variant_registry: self
                .key_lock_variant_registries
                .get(track)
                .cloned()
                .unwrap_or_default(),
        })
    }

    pub fn set_track_pattern_data(&mut self, track: usize, data: TrackPatternData) {
        while self.track_bits.len() <= track {
            let next_track = self.track_bits.len();
            self.push_default_track(next_track, &[]);
        }

        self.track_bits[track] = data.track_bits;
        self.neural_reset_bits[track] = data.neural_reset_bits;
        self.step_data[track] = data.step_data;
        self.track_params[track] = data.track_params;
        self.effect_slots[track] = data.effect_slots;
        self.midi_fx_slots[track] = data.midi_fx_slots;
        self.instrument_slots[track] = data.instrument_slot;
        self.instrument_base_note_offsets[track] = data.instrument_base_note_offset;
        self.track_sound_states[track] = data.track_sound_state;
        self.sample_ids[track] = data.sample_id;
        self.chord_snapshots[track] = data.chord_snapshot;
        self.timebase_plock_snapshots[track] = data.timebase_plock_snapshot;
        self.swing_plock_snapshots[track] = data.swing_plock_snapshot;
        self.swing_resolution_plock_snapshots[track] = data.swing_resolution_plock_snapshot;
        self.instrument_types[track] = data.instrument_type;
        self.instrument_run_modes[track] = data.instrument_run_mode;
        self.rack_tracks[track] = data.rack_track;
        self.process_chains[track] = data.process_chain;
        self.project_process_lane_overrides[track] = data.project_process_lane_overrides;
        self.plock_variant_registries[track] = data.plock_variant_registry;
        self.key_lock_variant_registries[track] = data.key_lock_variant_registry;
    }

    pub fn clone_track_lane_from(&mut self, source: &PatternSnapshot, track: usize) {
        if let Some(data) = source.track_pattern_data(track) {
            self.set_track_pattern_data(track, data);
        }
    }

    pub fn clear_track(
        &mut self,
        track: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        instrument_type: InstrumentType,
    ) {
        if track >= self.track_bits.len() {
            return;
        }
        self.track_bits[track] = [0u64; TRACK_PATTERN_WORDS];
        self.neural_reset_bits[track] = [0u64; TRACK_PATTERN_WORDS];
        self.step_data[track] = Self::default_step_data();
        self.track_params[track] = TrackParamsSnapshot::default();
        self.effect_slots[track] = Self::default_effect_slots(track, slot_descriptors);
        self.midi_fx_slots[track] = Self::default_midi_fx_slots();
        self.instrument_slots[track] = Self::default_instrument_slot();
        self.instrument_base_note_offsets[track] = 0.0;
        self.track_sound_states[track] = TrackSoundState::default();
        self.sample_ids[track] = (-1, String::new(), 44_100);
        self.chord_snapshots[track] = ChordSnapshot::new_default();
        self.timebase_plock_snapshots[track] = [None; MAX_STEPS];
        self.swing_plock_snapshots[track] = [None; MAX_STEPS];
        self.swing_resolution_plock_snapshots[track] = [None; MAX_STEPS];
        self.instrument_types[track] = instrument_type;
        self.instrument_run_modes[track] = CustomInstrumentRunMode::Instrument;
        self.rack_tracks[track] = None;
        self.process_chains[track] = crate::process::TrackProcessChain::default();
        self.project_process_lane_overrides[track] = Default::default();
        self.plock_variant_registries[track] = PlockVariantRegistry::default();
        self.key_lock_variant_registries[track] = PlockVariantRegistry::default();
    }

    pub(super) fn default_step_data() -> Vec<[f32; NUM_PARAMS]> {
        (0..MAX_STEPS)
            .map(|_| {
                let mut params = [0.0f32; NUM_PARAMS];
                for p in StepParam::ALL {
                    params[p.index()] = p.default_value();
                }
                params
            })
            .collect()
    }

    pub(super) fn default_effect_slots(
        t: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
    ) -> Vec<EffectSlotSnapshot> {
        if t < slot_descriptors.len() {
            slot_descriptors[t]
                .iter()
                .map(|desc| EffectSlotSnapshot::new_default(desc, 0))
                .collect()
        } else {
            Vec::new()
        }
    }

    pub(super) fn default_instrument_slot() -> EffectSlotSnapshot {
        EffectSlotSnapshot::new_empty()
    }

    pub(super) fn default_midi_fx_slots() -> Vec<EffectSlotSnapshot> {
        (0..crate::lisp_host::MAX_MIDI_FX_SLOTS)
            .map(|_| EffectSlotSnapshot::new_empty())
            .collect()
    }

    pub(super) fn push_default_track(&mut self, t: usize, slot_descriptors: &[Vec<EffectDescriptor>]) {
        self.track_bits.push([0u64; TRACK_PATTERN_WORDS]);
        self.neural_reset_bits.push([0u64; TRACK_PATTERN_WORDS]);
        self.step_data.push(Self::default_step_data());
        self.track_params.push(TrackParamsSnapshot::default());
        self.effect_slots
            .push(Self::default_effect_slots(t, slot_descriptors));
        self.midi_fx_slots.push(Self::default_midi_fx_slots());
        self.instrument_slots.push(Self::default_instrument_slot());
        self.instrument_base_note_offsets.push(0.0);
        self.track_sound_states.push(TrackSoundState::default());
        self.sample_ids.push((-1, String::new(), 44_100));
        self.chord_snapshots.push(ChordSnapshot::new_default());
        self.timebase_plock_snapshots.push([None; MAX_STEPS]);
        self.swing_plock_snapshots.push([None; MAX_STEPS]);
        self.swing_resolution_plock_snapshots
            .push([None; MAX_STEPS]);
        self.instrument_types.push(InstrumentType::Sampler);
        self.instrument_run_modes
            .push(CustomInstrumentRunMode::Instrument);
        self.rack_tracks.push(None);
        self.process_chains
            .push(crate::process::TrackProcessChain::default());
        self.project_process_lane_overrides.push(Default::default());
        self.plock_variant_registries
            .push(PlockVariantRegistry::default());
        self.key_lock_variant_registries
            .push(PlockVariantRegistry::default());
    }

    pub fn new_default(num_tracks: usize, slot_descriptors: &[Vec<EffectDescriptor>]) -> Self {
        let mut snap = Self {
            track_bits: Vec::with_capacity(num_tracks),
            neural_reset_bits: Vec::with_capacity(num_tracks),
            step_data: Vec::with_capacity(num_tracks),
            track_params: Vec::with_capacity(num_tracks),
            effect_slots: Vec::with_capacity(num_tracks),
            midi_fx_slots: Vec::with_capacity(num_tracks),
            instrument_slots: Vec::with_capacity(num_tracks),
            instrument_base_note_offsets: Vec::with_capacity(num_tracks),
            track_sound_states: Vec::with_capacity(num_tracks),
            sample_ids: Vec::with_capacity(num_tracks),
            chord_snapshots: Vec::with_capacity(num_tracks),
            timebase_plock_snapshots: Vec::with_capacity(num_tracks),
            swing_plock_snapshots: Vec::with_capacity(num_tracks),
            swing_resolution_plock_snapshots: Vec::with_capacity(num_tracks),
            instrument_types: Vec::with_capacity(num_tracks),
            instrument_run_modes: Vec::with_capacity(num_tracks),
            mod_connections: Vec::new(),
            neural_networks: Vec::new(),
            graph_overrides: Vec::new(),
            rack_tracks: Vec::with_capacity(num_tracks),
            process_chains: Vec::with_capacity(num_tracks),
            project_process_lane_overrides: Vec::with_capacity(num_tracks),
            project_process_chain: crate::process::TrackProcessChain::default(),
            plock_variant_registries: Vec::with_capacity(num_tracks),
            key_lock_variant_registries: Vec::with_capacity(num_tracks),
        };
        for t in 0..num_tracks {
            snap.push_default_track(t, slot_descriptors);
        }
        snap
    }

    pub fn extend_to_tracks(
        &mut self,
        new_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
    ) {
        if new_count <= self.track_bits.len() {
            return;
        }
        let old_count = self.track_bits.len();
        self.normalize_track_count(new_count, slot_descriptors);
        debug_assert_eq!(self.track_bits.len(), new_count);
        debug_assert!(old_count <= new_count);
    }

    #[cfg(test)]
    pub(super) fn track_lane_count_is_consistent(&self) -> bool {
        let n = self.track_bits.len();
        self.step_data.len() == n
            && self.neural_reset_bits.len() == n
            && self.track_params.len() == n
            && self.effect_slots.len() == n
            && self.midi_fx_slots.len() == n
            && self.instrument_slots.len() == n
            && self.instrument_base_note_offsets.len() == n
            && self.track_sound_states.len() == n
            && self.sample_ids.len() == n
            && self.chord_snapshots.len() == n
            && self.timebase_plock_snapshots.len() == n
            && self.swing_plock_snapshots.len() == n
            && self.instrument_types.len() == n
            && self.swing_resolution_plock_snapshots.len() == n
            && self.instrument_run_modes.len() == n
            && self.rack_tracks.len() == n
            && self.process_chains.len() == n
            && self.project_process_lane_overrides.len() == n
            && self.plock_variant_registries.len() == n
            && self.key_lock_variant_registries.len() == n
            && self.step_data.iter().all(|steps| steps.len() == MAX_STEPS)
    }

    pub fn sync_effect_slot(
        &mut self,
        track: usize,
        slot_idx: usize,
        desc: &EffectDescriptor,
        node_id: u32,
    ) {
        self.sync_effect_slot_with_modulator(track, slot_idx, desc, node_id, 0);
    }

    pub fn sync_effect_slot_with_modulator(
        &mut self,
        track: usize,
        slot_idx: usize,
        desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        while self.effect_slots.len() <= track {
            self.push_default_track(track, &[]);
        }
        while self.effect_slots[track].len() <= slot_idx {
            self.effect_slots[track].push(EffectSlotSnapshot::new_empty());
        }
        self.effect_slots[track][slot_idx].sync_to_descriptor_with_modulator(
            desc,
            node_id,
            modulator_node_id,
        );
    }

    pub fn sync_midi_fx_slot(&mut self, track: usize, slot_idx: usize, desc: &EffectDescriptor) {
        while self.midi_fx_slots.len() <= track {
            self.push_default_track(track, &[]);
        }
        while self.midi_fx_slots[track].len() <= slot_idx {
            self.midi_fx_slots[track].push(EffectSlotSnapshot::new_empty());
        }
        self.midi_fx_slots[track][slot_idx].sync_to_descriptor(desc, 0);
    }

    pub fn sync_instrument_slot(
        &mut self,
        track: usize,
        desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
        instrument_type: InstrumentType,
    ) {
        while self.instrument_slots.len() <= track {
            self.push_default_track(track, &[]);
        }
        self.instrument_slots[track].sync_to_descriptor_with_modulator(
            desc,
            node_id,
            modulator_node_id,
        );
        if track < self.instrument_types.len() {
            self.instrument_types[track] = instrument_type;
        }
    }
}
