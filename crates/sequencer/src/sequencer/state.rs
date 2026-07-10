use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::effects::{
    EffectDescriptor, EffectSlotSnapshot, EffectSlotState, HostControl, MAX_SLOT_PARAMS,
};
use crate::graph::{GraphVisualizationSnapshot, ProjectGraphOverrides};
use crate::neural::{
    remap_neural_network_routes_after_track_delete, NeuralVisualizationSnapshot,
    ProjectNeuralNetwork,
};
use crate::plock_variants::{
    live_track_key_lock_variant_key, live_track_key_lock_variant_keys, live_track_variant_key,
    live_track_variant_keys, PlockVariantAssignment, PlockVariantKey, PlockVariantRegistry,
};
use crate::voice::MAX_VOICES;

use super::data::{
    ChordData, ChordSnapshot, CustomInstrumentRunMode, InstrumentType, ModConnection, RackRouting,
    StepData, StepParam, SwingPLockData, SwingResolution, SwingResolutionPLockData, Timebase,
    TimebasePLockData, TrackParams, TrackParamsSnapshot, TrackPattern, TrackSoundState,
    DEFAULT_BPM, EXT_MOD_INPUT_COUNT, MAX_INSTRUMENT_ENGINES, MAX_RACK_SLOTS, MAX_SAMPLER_POOLS,
    MAX_STEPS, MAX_TRACKS, NUM_PARAMS, TRACK_PATTERN_WORDS,
};
use super::snapshot::SequencerSnapshot;
use super::{BusId, TrackOutput};

#[derive(Clone)]
pub struct StepSlotPlocks {
    pub params: Vec<Option<f32>>,
}

pub const RACK_SLOT_PARAM_COUNT: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RackSlotParam {
    BaseNote,
    Gain,
    Pan,
    MaxPolyphony,
    Mute,
    Solo,
}

impl RackSlotParam {
    pub const ALL: [Self; RACK_SLOT_PARAM_COUNT] = [
        Self::BaseNote,
        Self::Gain,
        Self::Pan,
        Self::MaxPolyphony,
        Self::Mute,
        Self::Solo,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::BaseNote => 0,
            Self::Gain => 1,
            Self::Pan => 2,
            Self::MaxPolyphony => 3,
            Self::Mute => 4,
            Self::Solo => 5,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "base-note" => Some(Self::BaseNote),
            "gain" => Some(Self::Gain),
            "pan" => Some(Self::Pan),
            "max-polyphony" => Some(Self::MaxPolyphony),
            "mute" => Some(Self::Mute),
            "solo" => Some(Self::Solo),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::BaseNote => "base-note",
            Self::Gain => "gain",
            Self::Pan => "pan",
            Self::MaxPolyphony => "max-polyphony",
            Self::Mute => "mute",
            Self::Solo => "solo",
        }
    }

    pub fn clamp(self, value: f32) -> f32 {
        match self {
            Self::BaseNote => value.clamp(-48.0, 48.0),
            Self::Gain => value.clamp(0.0, 2.0),
            Self::Pan => value.clamp(-1.0, 1.0),
            Self::MaxPolyphony => value.round().clamp(1.0, crate::voice::MAX_VOICES as f32),
            Self::Mute | Self::Solo => {
                if value > 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RackSlotParamPlocks {
    pub rows: Vec<Vec<Option<f32>>>,
}

impl RackSlotParamPlocks {
    pub fn new() -> Self {
        Self {
            rows: (0..MAX_STEPS)
                .map(|_| vec![None; RACK_SLOT_PARAM_COUNT])
                .collect(),
        }
    }

    pub fn from_rows(mut rows: Vec<Vec<Option<f32>>>) -> Self {
        rows.truncate(MAX_STEPS);
        while rows.len() < MAX_STEPS {
            rows.push(Vec::new());
        }
        for row in &mut rows {
            row.truncate(RACK_SLOT_PARAM_COUNT);
            if row.len() < RACK_SLOT_PARAM_COUNT {
                row.resize(RACK_SLOT_PARAM_COUNT, None);
            }
            for param in RackSlotParam::ALL {
                if let Some(Some(value)) = row.get_mut(param.index()) {
                    *value = param.clamp(*value);
                }
            }
        }
        Self { rows }
    }

    fn ensure_step(&mut self, step: usize) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        while self.rows.len() <= step {
            self.rows.push(vec![None; RACK_SLOT_PARAM_COUNT]);
        }
        if self.rows[step].len() < RACK_SLOT_PARAM_COUNT {
            self.rows[step].resize(RACK_SLOT_PARAM_COUNT, None);
        }
        true
    }

    pub fn get(&self, step: usize, param: RackSlotParam) -> Option<f32> {
        self.rows
            .get(step)
            .and_then(|row| row.get(param.index()))
            .copied()
            .flatten()
    }

    pub fn set(&mut self, step: usize, param: RackSlotParam, value: f32) -> bool {
        if !self.ensure_step(step) {
            return false;
        }
        self.rows[step][param.index()] = Some(param.clamp(value));
        true
    }

    pub fn clear(&mut self, step: usize, param: RackSlotParam) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        if let Some(row) = self.rows.get_mut(step) {
            if let Some(value) = row.get_mut(param.index()) {
                *value = None;
            }
        }
        true
    }

    pub fn clear_step(&mut self, step: usize) {
        if let Some(row) = self.rows.get_mut(step) {
            for value in row.iter_mut().take(RACK_SLOT_PARAM_COUNT) {
                *value = None;
            }
        }
    }

    pub fn step_has_plock(&self, step: usize) -> bool {
        self.rows
            .get(step)
            .is_some_and(|row| row.iter().take(RACK_SLOT_PARAM_COUNT).any(Option::is_some))
    }
}

impl Default for RackSlotParamPlocks {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct BusPatternSnapshot {
    pub id: BusId,
    pub gate_sequence: BusGateSequence,
    pub effect_plocks: Vec<Vec<Vec<Option<f32>>>>,
    /// Per-scene base (non-plocked) effect parameter values, indexed
    /// `[slot][param]`. Recalled on scene switch so a bus effect knob can
    /// hold different values per scene. Empty for legacy snapshots.
    pub effect_defaults: Vec<Vec<f32>>,
}

#[derive(Clone)]
pub struct BusGateSequence {
    pub steps: [bool; MAX_STEPS],
    pub velocities: [f32; MAX_STEPS],
    pub durations: [f32; MAX_STEPS],
    pub syncs: [f32; MAX_STEPS],
    pub num_steps: usize,
    pub timebase: Timebase,
    pub swing: f32,
    pub swing_resolution: SwingResolution,
    pub timebase_plocks: [Option<Timebase>; MAX_STEPS],
    pub swing_plocks: [Option<f32>; MAX_STEPS],
    pub swing_resolution_plocks: [Option<SwingResolution>; MAX_STEPS],
}

impl Default for BusGateSequence {
    fn default() -> Self {
        Self {
            steps: [true; MAX_STEPS],
            velocities: [1.0; MAX_STEPS],
            durations: [1.0; MAX_STEPS],
            syncs: [0.0; MAX_STEPS],
            num_steps: 16,
            timebase: Timebase::Sixteenth,
            swing: 50.0,
            swing_resolution: SwingResolution::Sixteenth,
            timebase_plocks: [None; MAX_STEPS],
            swing_plocks: [None; MAX_STEPS],
            swing_resolution_plocks: [None; MAX_STEPS],
        }
    }
}

impl BusGateSequence {
    pub fn toggle_step(&mut self, step: usize) -> Option<bool> {
        let value = self.steps.get_mut(step)?;
        *value = !*value;
        Some(*value)
    }

    pub fn set_step_velocity(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.velocities.get_mut(step)?;
        *slot = value.clamp(0.0, 1.0);
        Some(*slot)
    }

    pub fn set_step_duration(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.durations.get_mut(step)?;
        *slot = value.clamp(0.1, 2.0);
        Some(*slot)
    }

    pub fn set_step_sync(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.syncs.get_mut(step)?;
        *slot = value
            .round()
            .clamp(0.0, (crate::sequencer::SYNC_COUNT - 1) as f32);
        Some(*slot)
    }

    pub fn set_num_steps(&mut self, value: usize) {
        self.num_steps = value.clamp(1, MAX_STEPS);
    }

    pub fn has_step_plock(&self, step: usize) -> bool {
        step < MAX_STEPS
            && (self.timebase_plocks[step].is_some()
                || self.swing_plocks[step].is_some()
                || self.swing_resolution_plocks[step].is_some())
    }
}

#[derive(Clone)]
pub struct StepSnapshot {
    pub active: bool,
    pub neural_reset: bool,
    pub params: [f32; NUM_PARAMS],
    pub chord: Vec<f32>,
    pub chord_durations: Vec<f32>,
    pub chord_delays: Vec<f32>,
    pub timebase: Option<Timebase>,
    pub swing: Option<f32>,
    pub swing_resolution: Option<SwingResolution>,
    pub effect_plocks: Vec<StepSlotPlocks>,
    pub instrument_plocks: StepSlotPlocks,
    pub rack_slot_param_plocks: Vec<StepSlotPlocks>,
    pub rack_slot_instrument_plocks: Vec<StepSlotPlocks>,
}

impl StepSnapshot {
    pub fn without_audio_plocks(&self) -> Self {
        let mut snapshot = self.clone();
        for plocks in &mut snapshot.effect_plocks {
            for value in &mut plocks.params {
                *value = None;
            }
        }
        for value in &mut snapshot.instrument_plocks.params {
            *value = None;
        }
        for plocks in &mut snapshot.rack_slot_param_plocks {
            for value in &mut plocks.params {
                *value = None;
            }
        }
        for plocks in &mut snapshot.rack_slot_instrument_plocks {
            for value in &mut plocks.params {
                *value = None;
            }
        }
        snapshot
    }
}

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
    pub project_process_chain: crate::process::TrackProcessChain,
    pub plock_variant_registries: Vec<PlockVariantRegistry>,
    pub key_lock_variant_registries: Vec<PlockVariantRegistry>,
}

#[derive(Clone, Debug)]
pub struct RackTrackSnapshot {
    pub routing: RackRouting,
    pub slots: Vec<RackSlotSnapshot>,
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
    pub track_sound_state: TrackSoundState,
    pub sample_id: Option<(i32, String, u32)>,
}

impl RackSlotSnapshot {
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

fn prepare_track_pattern_data_for_rack(data: &mut TrackPatternData) {
    data.instrument_type = InstrumentType::Rack;
    data.instrument_run_mode = CustomInstrumentRunMode::Instrument;
    data.instrument_slot = EffectSlotSnapshot::new_empty();
    data.instrument_base_note_offset = 0.0;
    data.track_sound_state.engine_id = None;
}

fn replace_rack_slot_source_preserving_controls(
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
}

#[derive(Clone)]
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
    pub plock_variant_registry: PlockVariantRegistry,
    pub key_lock_variant_registry: PlockVariantRegistry,
}

impl TrackPatternData {
    fn refresh_process_effect_binding_param_ids_for_slot(
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

    fn restore_to(&self, state: &SequencerState, track: usize) -> bool {
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
        tp.gate.store(snap.gate, Ordering::Relaxed);
        tp.set_attack_ms(snap.attack_ms);
        tp.set_release_ms(snap.release_ms);
        tp.set_swing(snap.swing);
        tp.set_swing_resolution(snap.swing_resolution);
        tp.set_num_steps(snap.num_steps);
        tp.set_volume(snap.volume);
        tp.set_pan(snap.pan);
        tp.set_mute(snap.mute);
        tp.set_solo(snap.solo);
        tp.set_send(snap.send);
        tp.set_output(snap.output.clone());
        tp.set_sends(snap.sends.clone());
        tp.polyphonic.store(snap.polyphonic, Ordering::Relaxed);
        tp.set_max_polyphony(snap.max_polyphony);
        tp.set_timebase(snap.timebase);
        tp.set_accumulator_idx(snap.accumulator_idx);
        tp.set_script_accumulator_name(snap.script_accumulator_name.clone());
        tp.set_midi_fx_chain(snap.midi_fx_chain.clone());
        tp.set_midi_fx_position(snap.midi_fx_position);
        tp.set_accum_limit(snap.accum_limit);
        tp.set_accum_mode(snap.accum_mode);
        tp.set_fts_scale(snap.fts_scale);
        tp.set_mute_group(snap.mute_group);

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

    fn remove_effect_slot(&mut self, slot_idx: usize) {
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

    fn insert_empty_effect_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.effect_slots.len() {
            return;
        }
        for idx in (slot_idx + 1..self.effect_slots.len()).rev() {
            self.effect_slots[idx] = self.effect_slots[idx - 1].clone();
        }
        self.effect_slots[slot_idx].clear();
    }

    fn move_effect_slot_to(&mut self, source_slot: usize, target_slot: usize) {
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

    fn sync_effect_slot_with_modulator(
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

    fn remap_sidechain_references_after_track_delete(
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
                    if let Some(selection) = slot.plocks[step].get(param_idx).and_then(|v| *v) {
                        slot.plocks[step][param_idx] =
                            Some(remap_sidechain_selection_after_track_delete(
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

    fn remove_midi_fx_slot(&mut self, slot_idx: usize) {
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

    fn insert_midi_fx_slot(&mut self, slot_idx: usize, name: String, desc: &EffectDescriptor) {
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

    fn move_midi_fx_slot_to(&mut self, source_slot: usize, target_slot: usize) {
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

    fn clear(
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

    fn default_step_params() -> [f32; NUM_PARAMS] {
        let mut params = [0.0f32; NUM_PARAMS];
        for param in StepParam::ALL {
            params[param.index()] = param.default_value();
        }
        params
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct PatternId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackPatternCellView {
    pub pattern_id: PatternId,
    pub assigned_to_current_scene: bool,
    pub active_effective: bool,
    pub overridden: bool,
}

#[derive(Clone)]
pub struct TrackPatternPool {
    pub patterns: HashMap<PatternId, TrackPatternData>,
    pub next_id: u64,
}

impl Default for TrackPatternPool {
    fn default() -> Self {
        Self {
            patterns: HashMap::new(),
            // Reserve 0 for atomic/sentinel uses; real track pattern ids start at 1.
            next_id: 1,
        }
    }
}

impl TrackPatternPool {
    pub fn insert(&mut self, data: TrackPatternData) -> PatternId {
        let id = PatternId(self.next_id.max(1));
        self.next_id = id.0.saturating_add(1).max(1);
        self.patterns.insert(id, data);
        id
    }

    pub fn contains(&self, id: PatternId) -> bool {
        self.patterns.contains_key(&id)
    }

    pub fn get(&self, id: PatternId) -> Option<&TrackPatternData> {
        self.patterns.get(&id)
    }

    pub fn get_mut(&mut self, id: PatternId) -> Option<&mut TrackPatternData> {
        self.patterns.get_mut(&id)
    }

    pub fn remove(&mut self, id: PatternId) -> Option<TrackPatternData> {
        self.patterns.remove(&id)
    }
}

#[derive(Clone)]
pub struct Scene {
    pub name: String,
    pub cells: Vec<Option<PatternId>>,
    pub bus_patterns: Vec<BusPatternSnapshot>,
    // These are scene-level because per-track launches must not swap project-wide
    // modulation, neural, or graph routing state.
    pub mod_connections: Vec<ModConnection>,
    pub neural_networks: Vec<ProjectNeuralNetwork>,
    pub graph_overrides: Vec<ProjectGraphOverrides>,
    /// Project-level default process chain: composed ahead of every track's
    /// own chain at snapshot capture, so present and future tracks inherit it.
    pub project_process_chain: crate::process::TrackProcessChain,
}

#[derive(Clone)]
pub struct ProjectScenes {
    pub track_pools: Vec<TrackPatternPool>,
    pub scenes: Vec<Scene>,
    pub current_scene: usize,
    pub track_overrides: Vec<Option<PatternId>>,
}

impl ProjectScenes {
    pub fn from_pattern_snapshots(
        snapshots: &[PatternSnapshot],
        current_scene: usize,
    ) -> ProjectScenes {
        let track_count = snapshots
            .iter()
            .map(|snapshot| snapshot.track_bits.len())
            .max()
            .unwrap_or(0);
        let mut track_pools = vec![TrackPatternPool::default(); track_count];
        let mut scenes = Vec::with_capacity(snapshots.len().max(1));

        for (scene_idx, snapshot) in snapshots.iter().enumerate() {
            let mut cells = vec![None; track_count];
            for track in 0..track_count {
                if let Some(data) = snapshot.track_pattern_data(track) {
                    cells[track] = Some(track_pools[track].insert(data));
                }
            }
            scenes.push(Scene {
                name: format!("Scene {}", scene_idx + 1),
                cells,
                bus_patterns: Vec::new(),
                mod_connections: snapshot.mod_connections.clone(),
                neural_networks: snapshot.neural_networks.clone(),
                graph_overrides: snapshot.graph_overrides.clone(),
                project_process_chain: snapshot.project_process_chain.clone(),
            });
        }

        if scenes.is_empty() {
            scenes.push(Scene {
                name: "Scene 1".to_string(),
                cells: vec![None; track_count],
                bus_patterns: Vec::new(),
                mod_connections: Vec::new(),
                neural_networks: Vec::new(),
                graph_overrides: Vec::new(),
                project_process_chain: crate::process::TrackProcessChain::default(),
            });
        }

        Self {
            track_pools,
            scenes,
            current_scene: current_scene.min(snapshots.len().saturating_sub(1)),
            track_overrides: vec![None; track_count],
        }
    }

    pub fn scene_count(&self) -> usize {
        self.scenes.len().max(1)
    }

    /// Sample ids for a scene without cloning the full track pattern data.
    pub fn scene_sample_ids(&self, scene_idx: usize) -> Option<Vec<(i32, String, u32)>> {
        let scene = self.scenes.get(scene_idx)?;
        Some(
            (0..self.track_pools.len())
                .map(|track| {
                    scene
                        .cells
                        .get(track)
                        .copied()
                        .flatten()
                        .and_then(|id| self.track_pools[track].get(id))
                        .map(|data| data.sample_id.clone())
                        .unwrap_or((-1, String::new(), 44_100))
                })
                .collect(),
        )
    }

    /// Metadata-only view of a scene (no track pattern data is cloned).
    pub fn scene_metadata(
        &self,
        scene_idx: usize,
    ) -> Option<(
        Vec<ModConnection>,
        Vec<ProjectNeuralNetwork>,
        Vec<ProjectGraphOverrides>,
    )> {
        let scene = self.scenes.get(scene_idx)?;
        Some((
            scene.mod_connections.clone(),
            scene.neural_networks.clone(),
            scene.graph_overrides.clone(),
        ))
    }

    pub fn scene_snapshot(&self, scene_idx: usize) -> Option<PatternSnapshot> {
        let scene = self.scenes.get(scene_idx)?;
        let mut snapshot = PatternSnapshot::new_default(self.track_pools.len(), &[]);
        snapshot.mod_connections = scene.mod_connections.clone();
        snapshot.neural_networks = scene.neural_networks.clone();
        snapshot.graph_overrides = scene.graph_overrides.clone();
        snapshot.project_process_chain = scene.project_process_chain.clone();
        for track in 0..self.track_pools.len() {
            let Some(id) = scene.cells.get(track).copied().flatten() else {
                continue;
            };
            let Some(data) = self.track_pools[track].get(id).cloned() else {
                continue;
            };
            snapshot.set_track_pattern_data(track, data);
        }
        Some(snapshot)
    }

    pub fn snapshots(&self) -> Vec<PatternSnapshot> {
        (0..self.scenes.len())
            .filter_map(|scene_idx| self.scene_snapshot(scene_idx))
            .collect()
    }

    pub fn save_scene_snapshot(&mut self, scene_idx: usize, snapshot: PatternSnapshot) -> bool {
        while self.track_pools.len() < snapshot.track_bits.len() {
            self.track_pools.push(TrackPatternPool::default());
            self.track_overrides.push(None);
            for scene in &mut self.scenes {
                scene.cells.push(None);
            }
        }
        let Some(scene) = self.scenes.get_mut(scene_idx) else {
            return false;
        };
        while scene.cells.len() < snapshot.track_bits.len() {
            scene.cells.push(None);
        }
        scene.mod_connections = snapshot.mod_connections.clone();
        scene.neural_networks = snapshot.neural_networks.clone();
        scene.graph_overrides = snapshot.graph_overrides.clone();
        // Deliberately NOT copied from the snapshot: the scene itself is the
        // live authority for `project_process_chain` (edited in place via
        // edit_current_project_process_chain), and several callers save
        // snapshots that never carried it. Snapshot→scene transfer happens
        // only in from_pattern_snapshots (project load).

        for track in 0..snapshot.track_bits.len() {
            let Some(data) = snapshot.track_pattern_data(track) else {
                continue;
            };
            let Some(id) = self
                .track_overrides
                .get(track)
                .copied()
                .flatten()
                .or_else(|| scene.cells.get(track).copied().flatten())
                .filter(|id| self.track_pools[track].contains(*id))
            else {
                continue;
            };
            if let Some(slot) = self.track_pools[track].get_mut(id) {
                *slot = data;
            }
        }
        true
    }

    pub fn delete_scene(&mut self, scene_idx: usize) -> Option<usize> {
        if self.scenes.len() <= 1 || scene_idx >= self.scenes.len() {
            return None;
        }
        self.scenes.remove(scene_idx);
        let new_idx = scene_idx.min(self.scenes.len() - 1);
        self.current_scene = new_idx;
        self.track_overrides.fill(None);
        Some(new_idx)
    }

    pub fn current_scene_metadata(
        &self,
    ) -> (
        Vec<ModConnection>,
        Vec<ProjectNeuralNetwork>,
        Vec<ProjectGraphOverrides>,
    ) {
        self.scenes
            .get(self.current_scene)
            .map(|scene| {
                (
                    scene.mod_connections.clone(),
                    scene.neural_networks.clone(),
                    scene.graph_overrides.clone(),
                )
            })
            .unwrap_or_default()
    }

    pub fn edit_current_mod_connections<F, R>(&mut self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ModConnection>) -> Result<R, String>,
    {
        let scene = self
            .scenes
            .get_mut(self.current_scene)
            .ok_or_else(|| "current scene out of range".to_string())?;
        edit(&mut scene.mod_connections)
    }

    pub fn current_neural_networks(&self) -> Vec<ProjectNeuralNetwork> {
        self.scenes
            .get(self.current_scene)
            .map(|scene| scene.neural_networks.clone())
            .unwrap_or_default()
    }

    pub fn edit_current_neural_networks<F, R>(&mut self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ProjectNeuralNetwork>) -> Result<R, String>,
    {
        let scene = self
            .scenes
            .get_mut(self.current_scene)
            .ok_or_else(|| "current scene out of range".to_string())?;
        edit(&mut scene.neural_networks)
    }

    pub fn current_graph_overrides(&self) -> Vec<ProjectGraphOverrides> {
        self.scenes
            .get(self.current_scene)
            .map(|scene| scene.graph_overrides.clone())
            .unwrap_or_default()
    }

    pub fn current_project_process_chain(&self) -> crate::process::TrackProcessChain {
        self.scenes
            .get(self.current_scene)
            .map(|scene| scene.project_process_chain.clone())
            .unwrap_or_default()
    }

    pub fn edit_current_project_process_chain<F, R>(&mut self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut crate::process::TrackProcessChain) -> Result<R, String>,
    {
        let scene = self
            .scenes
            .get_mut(self.current_scene)
            .ok_or_else(|| "current scene out of range".to_string())?;
        edit(&mut scene.project_process_chain)
    }

    pub fn edit_current_graph_overrides<F, R>(&mut self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ProjectGraphOverrides>) -> Result<R, String>,
    {
        let scene = self
            .scenes
            .get_mut(self.current_scene)
            .ok_or_else(|| "current scene out of range".to_string())?;
        edit(&mut scene.graph_overrides)
    }

    pub fn effective_pattern_id(&self, track: usize) -> Option<PatternId> {
        self.track_overrides
            .get(track)
            .copied()
            .flatten()
            .or_else(|| {
                self.scenes
                    .get(self.current_scene)
                    .and_then(|scene| scene.cells.get(track))
                    .copied()
                    .flatten()
            })
    }

    pub fn effective_track_pattern(&self, track: usize) -> Option<&TrackPatternData> {
        let id = self.effective_pattern_id(track)?;
        self.track_pools.get(track)?.get(id)
    }

    pub fn save_effective_track_pattern(&mut self, track: usize, data: TrackPatternData) -> bool {
        let Some(id) = self.effective_pattern_id(track) else {
            return false;
        };
        let Some(slot) = self
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
        else {
            return false;
        };
        *slot = data;
        true
    }

    pub fn launch_scene(&mut self, scene: usize) -> Option<Vec<Option<TrackPatternData>>> {
        let scene_cells = self.scenes.get(scene)?.cells.clone();
        let mut track_patterns = Vec::with_capacity(scene_cells.len());
        for (track, cell) in scene_cells.iter().copied().enumerate() {
            let data = match cell {
                Some(id) => Some(self.track_pools.get(track)?.get(id)?.clone()),
                None => None,
            };
            track_patterns.push(data);
        }

        self.current_scene = scene;
        self.track_overrides.fill(None);
        Some(track_patterns)
    }

    pub fn launch_track_pattern(
        &mut self,
        track: usize,
        id: PatternId,
    ) -> Option<TrackPatternData> {
        let data = self.track_pools.get(track)?.get(id)?.clone();
        *self.track_overrides.get_mut(track)? = Some(id);
        Some(data)
    }

    pub fn track_pattern_cells(&self, track: usize) -> Vec<TrackPatternCellView> {
        let Some(pool) = self.track_pools.get(track) else {
            return Vec::new();
        };
        let assigned = self
            .scenes
            .get(self.current_scene)
            .and_then(|scene| scene.cells.get(track))
            .copied()
            .flatten();
        let override_id = self.track_overrides.get(track).copied().flatten();
        let active = override_id.or(assigned);
        let overridden = override_id.is_some();
        let mut ids = pool.patterns.keys().copied().collect::<Vec<_>>();
        ids.sort_by_key(|id| id.0);
        ids.into_iter()
            .map(|pattern_id| TrackPatternCellView {
                pattern_id,
                assigned_to_current_scene: Some(pattern_id) == assigned,
                active_effective: Some(pattern_id) == active,
                overridden,
            })
            .collect()
    }

    pub fn set_cell(&mut self, scene: usize, track: usize, id: PatternId) -> bool {
        let Some(pool) = self.track_pools.get(track) else {
            return false;
        };
        if !pool.contains(id) {
            return false;
        }
        let Some(scene) = self.scenes.get_mut(scene) else {
            return false;
        };
        if track >= scene.cells.len() {
            return false;
        }

        scene.cells[track] = Some(id);
        true
    }

    pub fn clear_cell(&mut self, scene: usize, track: usize) -> Option<PatternId> {
        let scene = self.scenes.get_mut(scene)?;
        let cell = scene.cells.get_mut(track)?;
        let cleared = cell.take();
        if let Some(id) = cleared {
            if self.track_overrides.get(track).copied().flatten() == Some(id) {
                self.track_overrides[track] = None;
            }
        }
        cleared
    }

    pub fn fork_track_pattern(&mut self, track: usize) -> Option<PatternId> {
        let source = self.effective_track_pattern(track)?.clone();
        let id = self.track_pools.get_mut(track)?.insert(source);
        *self.track_overrides.get_mut(track)? = Some(id);
        Some(id)
    }

    pub fn clone_track_pattern_into_current_scene(&mut self, track: usize) -> Option<PatternId> {
        let source_id = self
            .track_overrides
            .get(track)
            .copied()
            .flatten()
            .or_else(|| {
                self.scenes
                    .get(self.current_scene)
                    .and_then(|scene| scene.cells.get(track))
                    .copied()
                    .flatten()
            })?;
        self.clone_track_pattern_id_into_current_scene(track, source_id)
    }

    pub fn clone_track_pattern_id_into_current_scene(
        &mut self,
        track: usize,
        source_id: PatternId,
    ) -> Option<PatternId> {
        if track >= self.scenes.get(self.current_scene)?.cells.len() {
            return None;
        }
        let source = self.track_pools.get(track)?.get(source_id)?.clone();
        let id = self.track_pools.get_mut(track)?.insert(source);
        let scene = self.scenes.get_mut(self.current_scene)?;
        scene.cells[track] = Some(id);
        *self.track_overrides.get_mut(track)? = None;
        Some(id)
    }

    pub fn delete_track_pattern(&mut self, track: usize, id: PatternId) -> bool {
        let Some(pool) = self.track_pools.get_mut(track) else {
            return false;
        };
        if pool.remove(id).is_none() {
            return false;
        }
        for scene in &mut self.scenes {
            if scene.cells.get(track).copied().flatten() == Some(id) {
                scene.cells[track] = None;
            }
        }
        if self.track_overrides.get(track).copied().flatten() == Some(id) {
            self.track_overrides[track] = None;
        }
        true
    }

    pub fn new_scene(&mut self) -> usize {
        let source_scene = self.scenes.get(self.current_scene).cloned();
        let mut cells = vec![None; self.track_pools.len()];
        for track in 0..self.track_pools.len() {
            if let Some(source) = self.effective_track_pattern(track).cloned() {
                cells[track] = Some(self.track_pools[track].insert(source));
            }
        }

        let scene_idx = self.scenes.len();
        let (
            bus_patterns,
            mod_connections,
            neural_networks,
            graph_overrides,
            project_process_chain,
        ) = source_scene
            .map(|scene| {
                (
                    scene.bus_patterns,
                    scene.mod_connections,
                    scene.neural_networks,
                    scene.graph_overrides,
                    scene.project_process_chain,
                )
            })
            .unwrap_or_default();
        self.scenes.push(Scene {
            name: format!("Scene {}", scene_idx + 1),
            cells,
            bus_patterns,
            mod_connections,
            neural_networks,
            graph_overrides,
            project_process_chain,
        });
        self.current_scene = scene_idx;
        self.track_overrides.fill(None);
        scene_idx
    }

    pub fn remove_track(&mut self, track: usize) -> bool {
        if track >= self.track_pools.len() {
            return false;
        }

        self.track_pools.remove(track);
        for scene in &mut self.scenes {
            if track < scene.cells.len() {
                scene.cells.remove(track);
            }
        }
        if track < self.track_overrides.len() {
            self.track_overrides.remove(track);
        }
        true
    }

    pub fn purge_unused_track_patterns(&mut self) -> usize {
        let mut removed = 0;
        for track in 0..self.track_pools.len() {
            let mut referenced = HashSet::new();
            for scene in &self.scenes {
                if let Some(id) = scene.cells.get(track).copied().flatten() {
                    referenced.insert(id);
                }
            }
            if let Some(id) = self.track_overrides.get(track).copied().flatten() {
                referenced.insert(id);
            }

            let before = self.track_pools[track].patterns.len();
            self.track_pools[track]
                .patterns
                .retain(|id, _| referenced.contains(id));
            removed += before - self.track_pools[track].patterns.len();
        }
        removed
    }

    fn edit_other_track_patterns<F>(&mut self, track: usize, mut edit: F) -> bool
    where
        F: FnMut(&mut TrackPatternData),
    {
        let current_effective = self.effective_pattern_id(track);
        let Some(pool) = self.track_pools.get_mut(track) else {
            return false;
        };
        for (id, data) in &mut pool.patterns {
            if Some(*id) != current_effective {
                edit(data);
            }
        }
        true
    }
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

    fn truncate_tracks(&mut self, track_count: usize) {
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

    pub fn restore_track(&self, state: &SequencerState, track: usize) -> bool {
        let Some(data) = self.track_pattern_data(track) else {
            return false;
        };
        data.restore_to(state, track)
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
        self.plock_variant_registries[track] = PlockVariantRegistry::default();
        self.key_lock_variant_registries[track] = PlockVariantRegistry::default();
    }

    fn default_step_data() -> Vec<[f32; NUM_PARAMS]> {
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

    fn default_effect_slots(
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

    fn default_instrument_slot() -> EffectSlotSnapshot {
        EffectSlotSnapshot::new_empty()
    }

    fn default_midi_fx_slots() -> Vec<EffectSlotSnapshot> {
        (0..crate::lisp_host::MAX_MIDI_FX_SLOTS)
            .map(|_| EffectSlotSnapshot::new_empty())
            .collect()
    }

    fn push_default_track(&mut self, t: usize, slot_descriptors: &[Vec<EffectDescriptor>]) {
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
    fn track_lane_count_is_consistent(&self) -> bool {
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
        instrument_type: InstrumentType,
    ) {
        while self.instrument_slots.len() <= track {
            self.push_default_track(track, &[]);
        }
        self.instrument_slots[track].sync_to_descriptor(desc, node_id);
        if track < self.instrument_types.len() {
            self.instrument_types[track] = instrument_type;
        }
    }
}

fn remove_track_lane_if_present<T>(lanes: &mut Vec<T>, track_idx: usize) {
    if track_idx < lanes.len() {
        lanes.remove(track_idx);
    }
}

fn remap_optional_track_after_delete(track: usize, deleted_track: usize) -> Option<usize> {
    if track == deleted_track {
        None
    } else if track > deleted_track {
        Some(track - 1)
    } else {
        Some(track)
    }
}

fn remap_graph_overrides_after_track_delete(
    overrides: &mut [ProjectGraphOverrides],
    deleted_track: usize,
) {
    for graph in overrides {
        for intrinsic in &mut graph.node_intrinsics {
            if let Some(route) = intrinsic.route.take() {
                intrinsic.route = match route {
                    crate::graph::ProjectGraphRouteOverride::None => {
                        Some(crate::graph::ProjectGraphRouteOverride::None)
                    }
                    crate::graph::ProjectGraphRouteOverride::Track(track) => {
                        remap_optional_track_after_delete(track, deleted_track)
                            .map(crate::graph::ProjectGraphRouteOverride::Track)
                    }
                };
            }
            if let Some(seed_from) = intrinsic.seed_from.take() {
                intrinsic.seed_from = Some(match seed_from {
                    crate::graph::ProjectGraphSeedFrom::Route => {
                        crate::graph::ProjectGraphSeedFrom::Route
                    }
                    crate::graph::ProjectGraphSeedFrom::Tracks(tracks) => {
                        crate::graph::ProjectGraphSeedFrom::Tracks(
                            tracks
                                .into_iter()
                                .filter_map(|track| {
                                    remap_optional_track_after_delete(track, deleted_track)
                                })
                                .collect(),
                        )
                    }
                });
            }
        }
    }
}

fn remap_mod_connection_after_track_delete(
    connection: ModConnection,
    deleted_track: usize,
) -> Option<ModConnection> {
    if connection.source_track == deleted_track {
        return None;
    }
    let destination = match connection.destination {
        crate::sequencer::ModDestination::Track(track) if track == deleted_track => return None,
        crate::sequencer::ModDestination::Track(track) => {
            crate::sequencer::ModDestination::Track(if track > deleted_track {
                track - 1
            } else {
                track
            })
        }
        crate::sequencer::ModDestination::Bus(bus) => crate::sequencer::ModDestination::Bus(bus),
    };
    Some(ModConnection {
        source_track: if connection.source_track > deleted_track {
            connection.source_track - 1
        } else {
            connection.source_track
        },
        destination,
        dest_input: connection.dest_input,
    })
}

fn mod_destination_valid_for_track_count(
    destination: crate::sequencer::ModDestination,
    track_count: usize,
) -> bool {
    match destination {
        crate::sequencer::ModDestination::Track(track) => track < track_count,
        crate::sequencer::ModDestination::Bus(_) => true,
    }
}

fn sidechain_source_track(
    owner_track: usize,
    selection_idx: usize,
    total_tracks: usize,
) -> Option<usize> {
    if selection_idx == 0 {
        return None;
    }
    let mut current_idx = 0usize;
    for source_track in 0..total_tracks {
        if source_track == owner_track {
            continue;
        }
        current_idx += 1;
        if current_idx == selection_idx {
            return Some(source_track);
        }
    }
    None
}

fn sidechain_selection_index(
    owner_track: usize,
    source_track: usize,
    total_tracks: usize,
) -> usize {
    if source_track >= total_tracks || source_track == owner_track {
        return 0;
    }
    let mut selection_idx = 0usize;
    for candidate in 0..total_tracks {
        if candidate == owner_track {
            continue;
        }
        selection_idx += 1;
        if candidate == source_track {
            return selection_idx;
        }
    }
    0
}

fn remap_sidechain_selection_after_track_delete(
    owner_track_old: usize,
    selection_idx: usize,
    deleted_track: usize,
    old_track_count: usize,
) -> usize {
    let Some(source_old) = sidechain_source_track(owner_track_old, selection_idx, old_track_count)
    else {
        return 0;
    };
    if source_old == deleted_track {
        return 0;
    }
    let owner_new = if owner_track_old > deleted_track {
        owner_track_old - 1
    } else {
        owner_track_old
    };
    let source_new = if source_old > deleted_track {
        source_old - 1
    } else {
        source_old
    };
    sidechain_selection_index(owner_new, source_new, old_track_count - 1)
}

fn remap_snapshot_sidechain_references_after_track_delete(
    snapshot: &mut PatternSnapshot,
    effect_descriptors: &[Vec<EffectDescriptor>],
    deleted_track: usize,
    old_track_count: usize,
) {
    for owner_track in 0..old_track_count {
        if owner_track == deleted_track || owner_track >= snapshot.effect_slots.len() {
            continue;
        }
        let Some(track_descs) = effect_descriptors.get(owner_track) else {
            continue;
        };
        for (slot_idx, slot) in snapshot.effect_slots[owner_track].iter_mut().enumerate() {
            let Some(desc) = track_descs.get(slot_idx) else {
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
                let remapped = remap_sidechain_selection_after_track_delete(
                    owner_track,
                    slot.defaults
                        .get(param_idx)
                        .copied()
                        .unwrap_or(0.0)
                        .round()
                        .max(0.0) as usize,
                    deleted_track,
                    old_track_count,
                ) as f32;
                if param_idx < slot.defaults.len() {
                    slot.defaults[param_idx] = remapped;
                }
                for step in 0..MAX_STEPS {
                    if let Some(selection) = slot.plocks[step].get(param_idx).and_then(|v| *v) {
                        slot.plocks[step][param_idx] =
                            Some(remap_sidechain_selection_after_track_delete(
                                owner_track,
                                selection.round().max(0.0) as usize,
                                deleted_track,
                                old_track_count,
                            ) as f32);
                    }
                }
            }
        }
    }
}

pub fn default_empty_effect_chain() -> Vec<EffectSlotState> {
    use crate::lisp_host::MAX_CUSTOM_FX;
    (0..MAX_CUSTOM_FX)
        .map(|_| EffectSlotState::empty())
        .collect()
}

pub struct PatternState {
    pub patterns: Vec<TrackPattern>,
    pub neural_reset_patterns: Vec<TrackPattern>,
    scene_silenced: Vec<AtomicBool>,
    pub step_data: Vec<StepData>,
    pub chord_data: Vec<ChordData>,
    pub track_params: Vec<TrackParams>,
    pub effect_chains: Vec<Vec<EffectSlotState>>,
    pub midi_fx_slots: Vec<Vec<EffectSlotState>>,
    scenes: Mutex<ProjectScenes>,
    current_pattern: AtomicU32,
    num_patterns: AtomicU32,
    pub timebase_plocks: Vec<TimebasePLockData>,
    pub swing_plocks: Vec<SwingPLockData>,
    pub swing_resolution_plocks: Vec<SwingResolutionPLockData>,
    pub instrument_slots: Vec<EffectSlotState>,
    pub instrument_base_note_offsets: Vec<AtomicU32>,
    pub instrument_run_modes: Vec<AtomicU32>,
    pub track_sound_state: Mutex<Vec<TrackSoundState>>,
    pub rack_tracks: Mutex<Vec<Option<RackTrackSnapshot>>>,
    pub process_chains: Mutex<Vec<crate::process::TrackProcessChain>>,
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
    /// Per-track sampler playhead as normalized 0.0–1.0 (f32 bits).
    pub sampler_playheads: Vec<AtomicU32>,
    pub active_voice_counts: Vec<AtomicU32>,
    pub playhead_phase: AtomicU32,
    pub record_quantize_thresh: AtomicU32,
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
    pub sampler_analysis_buffer_ids: Vec<AtomicU32>,
    pub sampler_analysis_bpm: Vec<AtomicU32>,
    pub sampler_onset_ptr_lo: Vec<AtomicU32>,
    pub sampler_onset_ptr_hi: Vec<AtomicU32>,
    pub sampler_analysis_status: Vec<AtomicU32>,
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

pub struct SequencerState {
    pub pattern: PatternState,
    pub transport: TransportState,
    pub runtime: RuntimeBindingState,
    scheduler_snapshot: Mutex<Arc<SequencerSnapshot>>,
    scheduler_snapshot_version: AtomicU64,
    neural_visualization: Mutex<NeuralVisualizationSnapshot>,
    graph_visualizations: Mutex<Vec<GraphVisualizationSnapshot>>,
    track_output_events: Mutex<Vec<TrackOutputEvent>>,
    track_output_current_beat_bits: AtomicU64,
    active_note_until_samples: Vec<[AtomicU64; 128]>,
    live_note_masks: Vec<[AtomicU64; 2]>,
    audio_rendered_sample: AtomicU64,
    scratch_source: Mutex<String>,
    scratch_source_version: AtomicU64,
    published_sequencers: Mutex<Vec<PublishedSequencer>>,
    published_sequencers_version: AtomicU64,
    published_process_authoring: Mutex<crate::process::PublishedProcessAuthoringSnapshot>,
    published_process_authoring_version: AtomicU64,
    scratch_effect_descriptors: Mutex<Vec<Vec<EffectDescriptor>>>,
    scratch_instrument_descriptors: Mutex<Vec<EffectDescriptor>>,
    process_trace_enabled: AtomicBool,
    pending_accumulator_reset_all: AtomicBool,
    pending_accumulator_reset_tracks: [AtomicBool; MAX_TRACKS],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackOutputEvent {
    pub track: usize,
    pub sample_time: u64,
    pub beat: f64,
    pub transpose: f32,
    pub velocity: f32,
}

const TRACK_OUTPUT_EVENT_HISTORY_CAP: usize = 1024;

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

const TOPOLOGY_EDIT_NONE: u32 = 0;
const TOPOLOGY_EDIT_DELETE_TRACK: u32 = 1;

fn capture_track_params_snapshot(track_params: &TrackParams) -> TrackParamsSnapshot {
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
        solo: track_params.is_solo(),
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
                sampler_playheads: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                active_voice_counts: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                playhead_phase: AtomicU32::new(0.0_f32.to_bits()),
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
        };
        state.publish_scheduler_snapshot();
        state
    }

    pub fn active_track_count(&self) -> usize {
        self.transport.num_tracks.load(Ordering::Acquire) as usize
    }

    pub fn is_scene_silenced(&self, track: usize) -> bool {
        self.pattern
            .scene_silenced
            .get(track)
            .map(|flag| flag.load(Ordering::Acquire))
            .unwrap_or(false)
    }

    fn set_scene_silenced(&self, track: usize, silenced: bool) {
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

    pub fn scene_count(&self) -> usize {
        self.pattern.scenes.lock().unwrap().scene_count()
    }

    pub fn pattern_repository_len(&self) -> usize {
        self.scene_count()
    }

    pub fn export_pattern_repository(&self) -> Vec<PatternSnapshot> {
        self.pattern.scenes.lock().unwrap().snapshots()
    }

    pub fn replace_pattern_repository(&self, snapshots: Vec<PatternSnapshot>, current_idx: usize) {
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

    pub fn sync_effect_slot_with_modulator_in_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        self.save_current_track_effect_snapshot(track);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.sync_effect_slot_with_modulator(slot_idx, descriptor, node_id, modulator_node_id);
            data.refresh_process_effect_binding_param_ids_for_slot(slot_idx, descriptor);
        });
    }

    pub fn normalize_current_pattern_instrument_run_mode(
        &self,
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        track: usize,
        run_mode: CustomInstrumentRunMode,
    ) {
        self.extend_all_pattern_snapshots_to_track(
            track_count,
            slot_descriptors,
            track,
            run_mode,
            None,
        );
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(id) = scenes.effective_pattern_id(track) else {
            return;
        };
        if let Some(data) = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
        {
            data.instrument_run_mode = run_mode;
        }
    }

    fn default_track_pattern_data_for_track(
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        track: usize,
        run_mode: CustomInstrumentRunMode,
        instrument: Option<(&EffectDescriptor, u32, InstrumentType)>,
    ) -> Option<TrackPatternData> {
        let mut snapshot = PatternSnapshot::new_default(track_count, slot_descriptors);
        if let Some(mode) = snapshot.instrument_run_modes.get_mut(track) {
            *mode = run_mode;
        }
        if let Some((descriptor, node_id, instrument_type)) = instrument {
            snapshot.sync_instrument_slot(track, descriptor, node_id, instrument_type);
        }
        snapshot.track_pattern_data(track)
    }

    pub fn extend_all_pattern_snapshots_to_track(
        &self,
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        track: usize,
        run_mode: CustomInstrumentRunMode,
        instrument: Option<(&EffectDescriptor, u32, InstrumentType)>,
    ) {
        let Some(default_data) = Self::default_track_pattern_data_for_track(
            track_count,
            slot_descriptors,
            track,
            run_mode,
            instrument,
        ) else {
            return;
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        while scenes.track_pools.len() < track_count {
            scenes.track_pools.push(TrackPatternPool::default());
            scenes.track_overrides.push(None);
            for scene in &mut scenes.scenes {
                scene.cells.push(None);
            }
        }
        if track >= scenes.track_pools.len() {
            return;
        }
        for scene_idx in 0..scenes.scenes.len() {
            while scenes.scenes[scene_idx].cells.len() < track_count {
                scenes.scenes[scene_idx].cells.push(None);
            }
            if scenes.scenes[scene_idx].cells[track].is_none() {
                let id = scenes.track_pools[track].insert(default_data.clone());
                scenes.scenes[scene_idx].cells[track] = Some(id);
            }
        }
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
        for data in pool.patterns.values_mut() {
            prepare_track_pattern_data_for_rack(data);
            data.rack_track = Some(rack_track.clone());
        }
    }

    pub fn append_rack_slot_to_current_pattern(
        &self,
        track: usize,
        routing: RackRouting,
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
                        routing,
                        slots: vec![slot.clone()],
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
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
            .expect("validated current pattern data before rack slot append");
        prepare_track_pattern_data_for_rack(data);
        match data.rack_track.as_mut() {
            Some(rack_track) => rack_track.slots.push(slot),
            None => {
                data.rack_track = Some(RackTrackSnapshot {
                    routing,
                    slots: vec![slot],
                });
            }
        }
        true
    }

    pub fn append_rack_slot_for_all_pattern_snapshots(
        &self,
        track: usize,
        routing: RackRouting,
        slot: RackSlotSnapshot,
    ) {
        if let Some(live_rack_track) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            match live_rack_track {
                Some(rack_track) => rack_track.slots.push(slot.clone()),
                None => {
                    *live_rack_track = Some(RackTrackSnapshot {
                        routing,
                        slots: vec![slot.clone()],
                    });
                }
            }
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return;
        };
        for data in pool.patterns.values_mut() {
            prepare_track_pattern_data_for_rack(data);
            match data.rack_track.as_mut() {
                Some(rack_track) => rack_track.slots.push(slot.clone()),
                None => {
                    data.rack_track = Some(RackTrackSnapshot {
                        routing,
                        slots: vec![slot.clone()],
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
                removed = true;
            }
        }
        if !removed {
            return false;
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        if let Some(pool) = scenes.track_pools.get_mut(track) {
            for data in pool.patterns.values_mut() {
                if let Some(rack_track) = data.rack_track.as_mut() {
                    if slot_idx < rack_track.slots.len() {
                        rack_track.slots.remove(slot_idx);
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
            let Some(data) = scenes.track_pools.get(track).and_then(|pool| pool.get(id)) else {
                return false;
            };
            let Some(rack_track) = data.rack_track.as_ref() else {
                return false;
            };
            if rack_track.slots.get(slot_idx).is_none() {
                return false;
            }
        }

        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            live_rack_track.slots.remove(slot_idx);
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let id = scenes
            .effective_pattern_id(track)
            .expect("validated current pattern id before rack slot removal");
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
            .expect("validated current pattern data before rack slot removal");
        let rack_track = data
            .rack_track
            .as_mut()
            .expect("validated current rack track before rack slot removal");
        rack_track.slots.remove(slot_idx);
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
            let Some(data) = scenes.track_pools.get(track).and_then(|pool| pool.get(id)) else {
                return false;
            };
            let Some(rack_track) = data.rack_track.as_ref() else {
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
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
            .expect("validated current pattern data before source replacement");
        prepare_track_pattern_data_for_rack(data);
        let rack_track = data
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
            for data in pool.patterns.values_mut() {
                if let Some(rack_track) = data.rack_track.as_mut() {
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
            if let Some(data) = scenes
                .track_pools
                .get_mut(track)
                .and_then(|pool| pool.get_mut(id))
            {
                if let Some(rack_track) = data.rack_track.as_mut() {
                    sync_slots(&mut rack_track.slots);
                    synced_current = true;
                }
            }
        }
        synced_live && synced_current
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

    pub fn insert_effect_slot_in_other_track_patterns(&self, track: usize, slot_idx: usize) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| data.insert_empty_effect_slot(slot_idx));
    }

    pub fn move_effect_slot_in_other_track_patterns(
        &self,
        track: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.move_effect_slot_to(source_slot, target_slot);
        });
    }

    pub fn remove_effect_slot_from_track_patterns(&self, track: usize, slot_idx: usize) {
        self.save_current_track_effect_snapshot(track);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| data.remove_effect_slot(slot_idx));
    }

    pub fn insert_midi_fx_slot_in_other_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
        name: String,
        descriptor: &EffectDescriptor,
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.insert_midi_fx_slot(slot_idx, name.clone(), descriptor);
        });
    }

    pub fn move_midi_fx_slot_in_other_track_patterns(
        &self,
        track: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.move_midi_fx_slot_to(source_slot, target_slot);
        });
    }

    pub fn remove_midi_fx_slot_from_track_patterns(&self, track: usize, slot_idx: usize) {
        self.save_current_track_midi_fx_snapshot(track);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| data.remove_midi_fx_slot(slot_idx));
    }

    pub fn remove_bus_references_from_all_track_patterns(&self, bus_id: BusId) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        for pool in &mut scenes.track_pools {
            for data in pool.patterns.values_mut() {
                if data.track_params.output == TrackOutput::Bus(bus_id) {
                    data.track_params.output = TrackOutput::Mix;
                }
                data.track_params
                    .sends
                    .retain(|send| send.destination != bus_id);
            }
        }
    }

    /// Force one track's output across every stored scene. Track output is
    /// otherwise per-scene, but a track group is a global concept — its members
    /// must reach the backing bus in every scene, or switching scenes would tear
    /// the group's routing apart (and a saved project would silently lose it).
    pub fn set_track_output_in_all_track_patterns(&self, track: usize, output: TrackOutput) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        if let Some(pool) = scenes.track_pools.get_mut(track) {
            for data in pool.patterns.values_mut() {
                data.track_params.output = output.clone();
            }
        }
    }

    fn ensure_scene_bus_patterns_len_locked(
        scenes: &mut ProjectScenes,
        len: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        for scene in scenes.scenes.iter_mut().take(len) {
            if scene.bus_patterns.is_empty() {
                scene.bus_patterns = default_snapshot.to_vec();
            }
        }
    }

    pub fn save_current_bus_pattern_snapshot(&self, snapshot: Vec<BusPatternSnapshot>) {
        let current_scene = self.current_scene_index();
        let target_len = self.scene_count().max(current_scene + 1);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, &snapshot);
        scenes.scenes[current_scene].bus_patterns = snapshot;
    }

    pub fn bus_pattern_snapshot_or_default(
        &self,
        scene_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<BusPatternSnapshot> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, scene_idx + 1, default_snapshot);
        scenes
            .scenes
            .get(scene_idx)
            .map(|scene| scene.bus_patterns.clone())
            .unwrap_or_else(|| default_snapshot.to_vec())
    }

    pub fn ensure_bus_pattern_repository_len(
        &self,
        len: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, len, default_snapshot);
    }

    pub fn clone_bus_pattern_snapshot(
        &self,
        source_idx: usize,
        new_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<BusPatternSnapshot> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(
            &mut scenes,
            source_idx.max(new_idx) + 1,
            default_snapshot,
        );
        let source = scenes
            .scenes
            .get(source_idx)
            .map(|scene| scene.bus_patterns.clone())
            .unwrap_or_else(|| default_snapshot.to_vec());
        if let Some(scene) = scenes.scenes.get_mut(new_idx) {
            scene.bus_patterns = source.clone();
        }
        source
    }

    pub fn delete_bus_pattern_snapshot(
        &self,
        _deleted_idx: usize,
        new_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<BusPatternSnapshot> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, new_idx + 1, default_snapshot);
        scenes
            .scenes
            .get(new_idx)
            .map(|scene| scene.bus_patterns.clone())
            .unwrap_or_else(|| default_snapshot.to_vec())
    }

    pub fn export_bus_pattern_repository(
        &self,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<Vec<BusPatternSnapshot>> {
        let target_len = self.scene_count();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        scenes
            .scenes
            .iter()
            .map(|scene| scene.bus_patterns.clone())
            .collect()
    }

    pub fn replace_bus_pattern_repository(
        &self,
        snapshots: Vec<Vec<BusPatternSnapshot>>,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(snapshots.len());
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        for (scene, snapshot) in scenes.scenes.iter_mut().zip(snapshots) {
            scene.bus_patterns = snapshot;
        }
    }

    pub fn insert_bus_effect_slot_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let current_scene = self.current_scene_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(current_scene + 1);
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        for (scene_idx, scene) in scenes.scenes.iter_mut().enumerate() {
            if scene_idx == current_scene {
                continue;
            }
            let Some(bus) = scene.bus_patterns.get_mut(bus_idx) else {
                continue;
            };
            if slot_idx > bus.effect_plocks.len() {
                continue;
            }
            bus.effect_plocks.insert(slot_idx, Vec::new());
            bus.effect_plocks.truncate(crate::lisp_host::MAX_CUSTOM_FX);
        }
    }

    pub fn move_bus_effect_slot_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        source_slot: usize,
        target_slot: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let current_scene = self.current_scene_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(current_scene + 1);
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        for (scene_idx, scene) in scenes.scenes.iter_mut().enumerate() {
            if scene_idx == current_scene {
                continue;
            }
            let Some(bus) = scene.bus_patterns.get_mut(bus_idx) else {
                continue;
            };
            if source_slot >= bus.effect_plocks.len() {
                continue;
            }
            let plocks = bus.effect_plocks.remove(source_slot);
            let mut target = target_slot.min(bus.effect_plocks.len());
            if source_slot < target {
                target = target.saturating_sub(1);
            }
            bus.effect_plocks.insert(target, plocks);
            bus.effect_plocks.truncate(crate::lisp_host::MAX_CUSTOM_FX);
        }
    }

    pub fn edit_pattern_repository<F, R>(&self, edit: F) -> R
    where
        F: FnOnce(&mut Vec<PatternSnapshot>, usize) -> R,
    {
        let current_pattern = self.current_pattern_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let bus_patterns = scenes
            .scenes
            .iter()
            .map(|scene| scene.bus_patterns.clone())
            .collect::<Vec<_>>();
        let mut snapshots = scenes.snapshots();
        let result = edit(&mut snapshots, current_pattern);
        let mut rebuilt = ProjectScenes::from_pattern_snapshots(&snapshots, current_pattern);
        for (scene, bus_patterns) in rebuilt.scenes.iter_mut().zip(bus_patterns) {
            scene.bus_patterns = bus_patterns;
        }
        *scenes = rebuilt;
        result
    }

    pub fn edit_non_current_pattern_snapshots<F>(&self, mut edit: F)
    where
        F: FnMut(&mut PatternSnapshot),
    {
        self.edit_pattern_repository(|bank, current_pattern| {
            for (pattern_idx, snapshot) in bank.iter_mut().enumerate() {
                if pattern_idx != current_pattern {
                    edit(snapshot);
                }
            }
        });
    }

    pub fn edit_all_pattern_snapshots<F>(&self, mut edit: F)
    where
        F: FnMut(&mut PatternSnapshot),
    {
        self.edit_pattern_repository(|bank, _| {
            for snapshot in bank.iter_mut() {
                edit(snapshot);
            }
        });
    }

    pub fn current_scene_metadata(
        &self,
    ) -> (
        Vec<ModConnection>,
        Vec<ProjectNeuralNetwork>,
        Vec<ProjectGraphOverrides>,
    ) {
        let current_pattern = self.current_pattern_index();
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scene_metadata(current_pattern)
            .unwrap_or_default()
    }

    pub fn current_mod_connections(&self) -> Vec<ModConnection> {
        self.current_scene_metadata().0
    }

    pub fn edit_current_mod_connections<F, R>(&self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ModConnection>) -> Result<R, String>,
    {
        let current_pattern = self.current_pattern_index();
        let result = {
            let mut bank = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            if bank.current_scene != current_pattern {
                bank.current_scene = current_pattern.min(bank.scene_count().saturating_sub(1));
            }
            bank.edit_current_mod_connections(edit)?
        };
        self.publish_scheduler_snapshot();
        Ok(result)
    }
    pub fn latest_scheduler_snapshot(&self) -> Arc<SequencerSnapshot> {
        self.scheduler_snapshot.lock().unwrap().clone()
    }

    pub fn set_neural_visualization(&self, snapshot: NeuralVisualizationSnapshot) {
        *self.neural_visualization.lock().unwrap() = snapshot;
    }

    pub fn neural_visualization(&self) -> NeuralVisualizationSnapshot {
        self.neural_visualization.lock().unwrap().clone()
    }

    pub fn set_graph_visualizations(&self, snapshots: Vec<GraphVisualizationSnapshot>) {
        *self.graph_visualizations.lock().unwrap() = snapshots;
    }

    pub fn graph_visualizations(&self) -> Vec<GraphVisualizationSnapshot> {
        self.graph_visualizations.lock().unwrap().clone()
    }

    pub fn append_track_output_events(&self, events: impl IntoIterator<Item = TrackOutputEvent>) {
        let mut history = self.track_output_events.lock().unwrap();
        history.extend(events);
        let overflow = history.len().saturating_sub(TRACK_OUTPUT_EVENT_HISTORY_CAP);
        if overflow > 0 {
            history.drain(0..overflow);
        }
    }

    pub fn clear_track_output_events(&self) {
        self.track_output_events.lock().unwrap().clear();
    }

    pub fn track_output_events(&self) -> Vec<TrackOutputEvent> {
        self.track_output_events.lock().unwrap().clone()
    }

    pub fn set_track_output_current_beat(&self, beat: f64) {
        self.track_output_current_beat_bits
            .store(beat.max(0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn track_output_current_beat(&self) -> f64 {
        f64::from_bits(self.track_output_current_beat_bits.load(Ordering::Relaxed))
    }

    /// Publish the audio clock used to expire scheduled-note activity without
    /// taking a lock on the realtime thread.
    pub fn set_audio_rendered_sample(&self, sample: u64) {
        self.audio_rendered_sample.store(sample, Ordering::Release);
    }

    /// Keep a scheduled MIDI note active through its gate end. `fetch_max`
    /// preserves overlapping/retriggered instances of the same pitch.
    pub fn mark_scheduled_note_active_until(&self, track: usize, note: u8, sample: u64) {
        if let Some(notes) = self.active_note_until_samples.get(track) {
            notes[note as usize].fetch_max(sample, Ordering::Relaxed);
        }
    }

    /// Live notes have explicit note-off events, so replace their compact mask
    /// independently from scheduled expirations. The two sources can overlap.
    pub fn replace_live_notes(&self, track: usize, notes: impl IntoIterator<Item = u8>) {
        let Some(words) = self.live_note_masks.get(track) else {
            return;
        };
        let mut next = [0_u64; 2];
        for note in notes {
            next[note as usize / 64] |= 1_u64 << (note as usize % 64);
        }
        for (word, value) in words.iter().zip(next) {
            word.store(value, Ordering::Release);
        }
    }

    pub fn active_notes(&self, track: usize) -> Vec<u8> {
        let (Some(until), Some(live)) = (
            self.active_note_until_samples.get(track),
            self.live_note_masks.get(track),
        ) else {
            return Vec::new();
        };
        let rendered = self.audio_rendered_sample.load(Ordering::Acquire);
        (0_u8..=127)
            .filter(|note| {
                let idx = *note as usize;
                let bit = 1_u64 << (idx % 64);
                live[idx / 64].load(Ordering::Relaxed) & bit != 0
                    || until[idx].load(Ordering::Relaxed) > rendered
            })
            .collect()
    }

    pub fn publish_scheduler_snapshot(&self) -> Arc<SequencerSnapshot> {
        let snapshot = Arc::new(SequencerSnapshot::capture(self));
        self.publish_scheduler_snapshot_arc(snapshot)
    }

    fn publish_scheduler_snapshot_arc(
        &self,
        snapshot: Arc<SequencerSnapshot>,
    ) -> Arc<SequencerSnapshot> {
        {
            let mut published = self.scheduler_snapshot.lock().unwrap();
            *published = Arc::clone(&snapshot);
        }
        self.scheduler_snapshot_version
            .fetch_add(1, Ordering::AcqRel);
        snapshot
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_scheduler_snapshot_from_track_pattern_data(
        &self,
        tracks: &[TrackPatternData],
        mod_connections: Vec<ModConnection>,
        neural_networks: Vec<ProjectNeuralNetwork>,
        graph_overrides: Vec<ProjectGraphOverrides>,
        project_process_chain: crate::process::TrackProcessChain,
    ) -> Arc<SequencerSnapshot> {
        self.publish_scheduler_snapshot_arc(Arc::new(
            SequencerSnapshot::capture_from_track_pattern_data(
                self,
                tracks,
                mod_connections,
                neural_networks,
                graph_overrides,
                project_process_chain,
            ),
        ))
    }

    pub fn current_neural_networks(&self) -> Vec<ProjectNeuralNetwork> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .current_neural_networks()
    }

    pub fn edit_current_neural_networks<F, R>(&self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ProjectNeuralNetwork>) -> Result<R, String>,
    {
        let result = {
            let mut bank = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            bank.current_scene = self
                .current_scene_index()
                .min(bank.scene_count().saturating_sub(1));
            bank.edit_current_neural_networks(edit)?
        };
        self.publish_scheduler_snapshot();
        Ok(result)
    }

    pub fn current_graph_overrides(&self) -> Vec<ProjectGraphOverrides> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .current_graph_overrides()
    }

    pub fn edit_current_graph_overrides<F, R>(&self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ProjectGraphOverrides>) -> Result<R, String>,
    {
        let result = {
            let mut bank = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            bank.current_scene = self
                .current_scene_index()
                .min(bank.scene_count().saturating_sub(1));
            bank.edit_current_graph_overrides(edit)?
        };
        self.publish_scheduler_snapshot();
        Ok(result)
    }

    pub fn set_neural_reset_step(
        &self,
        track: usize,
        step: usize,
        enabled: bool,
    ) -> Result<bool, String> {
        if track >= self.active_track_count() {
            return Err("track out of range".to_string());
        }
        if step >= MAX_STEPS {
            return Err("step out of range".to_string());
        }
        self.pattern.neural_reset_patterns[track].set_step_active(step, enabled);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Ok(enabled)
    }

    pub fn request_track_delete_boundary(&self, track_idx: usize) -> u64 {
        let request_id = self
            .transport
            .topology_edit_request_id
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        self.transport
            .topology_edit_track
            .store(track_idx as u32, Ordering::Release);
        self.transport
            .topology_edit_kind
            .store(TOPOLOGY_EDIT_DELETE_TRACK, Ordering::Release);
        request_id
    }

    pub fn topology_edit_ready(&self, request_id: u64) -> bool {
        self.transport
            .topology_edit_ready_id
            .load(Ordering::Acquire)
            >= request_id
    }

    pub fn topology_edit_in_flight(&self) -> bool {
        self.transport.topology_edit_kind.load(Ordering::Acquire) != TOPOLOGY_EDIT_NONE
    }

    pub fn complete_topology_edit(&self, request_id: u64) {
        self.transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.transport
            .topology_edit_applied_id
            .store(request_id, Ordering::Release);
        self.transport
            .topology_edit_kind
            .store(TOPOLOGY_EDIT_NONE, Ordering::Release);
        self.transport
            .topology_edit_track
            .store(u32::MAX, Ordering::Release);
    }

    fn reset_track_params_to_default(&self, track: usize) {
        let defaults = TrackParamsSnapshot::default();
        let params = &self.pattern.track_params[track];
        params.gate.store(defaults.gate, Ordering::Relaxed);
        params.set_attack_ms(defaults.attack_ms);
        params.set_release_ms(defaults.release_ms);
        params.set_swing(defaults.swing);
        params.set_swing_resolution(defaults.swing_resolution);
        params.set_num_steps(defaults.num_steps);
        params.set_volume(defaults.volume);
        params.set_pan(defaults.pan);
        params.set_mute(defaults.mute);
        params.set_solo(defaults.solo);
        params.set_send(defaults.send);
        params.set_output(defaults.output);
        params.set_sends(defaults.sends);
        params
            .polyphonic
            .store(defaults.polyphonic, Ordering::Relaxed);
        params.set_max_polyphony(defaults.max_polyphony);
        params.set_timebase(defaults.timebase);
        params.set_accumulator_idx(defaults.accumulator_idx);
        params.set_script_accumulator_name(defaults.script_accumulator_name);
        params.set_midi_fx_chain(defaults.midi_fx_chain);
        params.set_midi_fx_position(defaults.midi_fx_position);
        params.set_accum_limit(defaults.accum_limit);
        params.set_accum_mode(defaults.accum_mode);
        params.set_fts_scale(defaults.fts_scale);
        params.set_mute_group(defaults.mute_group);
        params.set_global_transpose(defaults.global_transpose);
    }

    pub fn clear_live_track_state(&self, track_count: usize) {
        for track in 0..track_count.min(MAX_TRACKS) {
            self.clear_live_track_lane(track);
            self.clear_runtime_track_binding_in_place(track);
        }
    }

    fn clear_live_track_lane(&self, track: usize) {
        self.pattern.patterns[track].store_bits([0u64; TRACK_PATTERN_WORDS]);
        self.pattern.neural_reset_patterns[track].store_bits([0u64; TRACK_PATTERN_WORDS]);
        for step in 0..MAX_STEPS {
            for param in StepParam::ALL {
                self.pattern.step_data[track].set(step, param, param.default_value());
            }
            self.pattern.chord_data[track].clear_step(step);
            self.pattern.timebase_plocks[track].clear(step);
            self.pattern.swing_plocks[track].clear(step);
            self.pattern.swing_resolution_plocks[track].clear(step);
        }
        self.reset_track_params_to_default(track);
        for slot in &self.pattern.effect_chains[track] {
            slot.clear();
        }
        for slot in &self.pattern.midi_fx_slots[track] {
            slot.clear();
        }
        self.pattern.instrument_slots[track].clear();
        self.pattern.instrument_base_note_offsets[track].store(0.0f32.to_bits(), Ordering::Relaxed);
        self.pattern.instrument_run_modes[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Relaxed,
        );
        if let Some(sound) = self
            .pattern
            .track_sound_state
            .lock()
            .unwrap()
            .get_mut(track)
        {
            *sound = TrackSoundState::default();
        }
        if let Some(rack_track) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            *rack_track = None;
        }
        if let Some(chain) = self.pattern.process_chains.lock().unwrap().get_mut(track) {
            *chain = crate::process::TrackProcessChain::default();
        }
        if let Some(registry) = self
            .pattern
            .plock_variant_registries
            .lock()
            .unwrap()
            .get_mut(track)
        {
            *registry = PlockVariantRegistry::default();
        }
        if let Some(registry) = self
            .pattern
            .key_lock_variant_registries
            .lock()
            .unwrap()
            .get_mut(track)
        {
            *registry = PlockVariantRegistry::default();
        }
    }

    fn clear_runtime_track_binding_in_place(&self, track: usize) {
        self.set_scene_silenced(track, false);
        self.transport.track_playheads[track].store(0, Ordering::Relaxed);
        self.transport.trigger_flash[track].store(0, Ordering::Relaxed);
        self.runtime.sampler_lids[track].store(0, Ordering::Relaxed);
        self.runtime.modulator_lids[track].store(0, Ordering::Relaxed);
        self.runtime.voice_counts[track].store(0, Ordering::Relaxed);
        self.runtime.instrument_type_flags[track].store(0, Ordering::Relaxed);
        self.runtime.instrument_run_mode_flags[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Relaxed,
        );
        self.runtime.track_engine_ids[track].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_buffer_ids[track].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_bpm[track].store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_lo[track].store(0, Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_hi[track].store(0, Ordering::Relaxed);
        self.runtime.sampler_analysis_status[track].store(0, Ordering::Relaxed);
        for slot in 0..MAX_RACK_SLOTS {
            self.runtime.rack_slot_pan_lids[track][slot].store(0, Ordering::Relaxed);
        }
        for voice in 0..MAX_VOICES {
            self.runtime.voice_lids[track][voice].store(0, Ordering::Relaxed);
            self.runtime.synth_node_ids[track][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_gatepitch_node_ids[track][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_modulator_node_ids[track][voice].store(0, Ordering::Relaxed);
        }
        self.pending_accumulator_reset_tracks[track].store(false, Ordering::Relaxed);
        for engine_id in 0..self.runtime.engine_route_lids.len() {
            for voice in 0..MAX_VOICES {
                self.runtime.engine_route_lids[engine_id][voice][track].store(0, Ordering::Relaxed);
                self.runtime.engine_route_lids_r[engine_id][voice][track]
                    .store(0, Ordering::Relaxed);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.runtime.engine_ext_route_lids[engine_id][voice][track][input]
                        .store(0, Ordering::Relaxed);
                }
            }
        }
    }

    fn shift_runtime_track_bindings_left(&self, track_idx: usize, old_count: usize) {
        for idx in track_idx..old_count.saturating_sub(1) {
            let next = idx + 1;
            self.transport.track_playheads[idx].store(
                self.transport.track_playheads[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.transport.trigger_flash[idx].store(
                self.transport.trigger_flash[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.pattern.scene_silenced[idx].store(
                self.pattern.scene_silenced[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_lids[idx].store(
                self.runtime.sampler_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.modulator_lids[idx].store(
                self.runtime.modulator_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.pan_lids[idx].store(
                self.runtime.pan_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.delay_lids[idx].store(
                self.runtime.delay_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.send_lids[idx].store(
                self.runtime.send_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            for slot in 0..MAX_RACK_SLOTS {
                self.runtime.rack_slot_pan_lids[idx][slot].store(
                    self.runtime.rack_slot_pan_lids[next][slot].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
            }
            self.runtime.voice_counts[idx].store(
                self.runtime.voice_counts[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.instrument_type_flags[idx].store(
                self.runtime.instrument_type_flags[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.instrument_run_mode_flags[idx].store(
                self.runtime.instrument_run_mode_flags[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.track_engine_ids[idx].store(
                self.runtime.track_engine_ids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_analysis_buffer_ids[idx].store(
                self.runtime.sampler_analysis_buffer_ids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_analysis_bpm[idx].store(
                self.runtime.sampler_analysis_bpm[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_onset_ptr_lo[idx].store(
                self.runtime.sampler_onset_ptr_lo[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_onset_ptr_hi[idx].store(
                self.runtime.sampler_onset_ptr_hi[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_analysis_status[idx].store(
                self.runtime.sampler_analysis_status[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            for voice in 0..MAX_VOICES {
                self.runtime.voice_lids[idx][voice].store(
                    self.runtime.voice_lids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                self.runtime.synth_node_ids[idx][voice].store(
                    self.runtime.synth_node_ids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                self.runtime.sampler_gatepitch_node_ids[idx][voice].store(
                    self.runtime.sampler_gatepitch_node_ids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                self.runtime.sampler_modulator_node_ids[idx][voice].store(
                    self.runtime.sampler_modulator_node_ids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
            }
            self.pending_accumulator_reset_tracks[idx].store(
                self.pending_accumulator_reset_tracks[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            for engine_id in 0..self.runtime.engine_route_lids.len() {
                for voice in 0..MAX_VOICES {
                    self.runtime.engine_route_lids[engine_id][voice][idx].store(
                        self.runtime.engine_route_lids[engine_id][voice][next]
                            .load(Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                    self.runtime.engine_route_lids_r[engine_id][voice][idx].store(
                        self.runtime.engine_route_lids_r[engine_id][voice][next]
                            .load(Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        self.runtime.engine_ext_route_lids[engine_id][voice][idx][input].store(
                            self.runtime.engine_ext_route_lids[engine_id][voice][next][input]
                                .load(Ordering::Relaxed),
                            Ordering::Relaxed,
                        );
                    }
                }
            }
        }

        if old_count == 0 {
            return;
        }
        let last = old_count - 1;
        self.set_scene_silenced(last, false);
        self.transport.track_playheads[last].store(0, Ordering::Relaxed);
        self.transport.trigger_flash[last].store(0, Ordering::Relaxed);
        self.runtime.sampler_lids[last].store(0, Ordering::Relaxed);
        self.runtime.modulator_lids[last].store(0, Ordering::Relaxed);
        self.runtime.pan_lids[last].store(0, Ordering::Relaxed);
        self.runtime.delay_lids[last].store(0, Ordering::Relaxed);
        self.runtime.send_lids[last].store(0, Ordering::Relaxed);
        for slot in 0..MAX_RACK_SLOTS {
            self.runtime.rack_slot_pan_lids[last][slot].store(0, Ordering::Relaxed);
        }
        self.runtime.voice_counts[last].store(0, Ordering::Relaxed);
        self.runtime.instrument_type_flags[last].store(0, Ordering::Relaxed);
        self.runtime.instrument_run_mode_flags[last].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Relaxed,
        );
        self.runtime.track_engine_ids[last].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_buffer_ids[last].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_bpm[last].store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_lo[last].store(0, Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_hi[last].store(0, Ordering::Relaxed);
        self.runtime.sampler_analysis_status[last].store(0, Ordering::Relaxed);
        for voice in 0..MAX_VOICES {
            self.runtime.voice_lids[last][voice].store(0, Ordering::Relaxed);
            self.runtime.synth_node_ids[last][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_gatepitch_node_ids[last][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_modulator_node_ids[last][voice].store(0, Ordering::Relaxed);
        }
        self.pending_accumulator_reset_tracks[last].store(false, Ordering::Relaxed);
        for engine_id in 0..self.runtime.engine_route_lids.len() {
            for voice in 0..MAX_VOICES {
                self.runtime.engine_route_lids[engine_id][voice][last].store(0, Ordering::Relaxed);
                self.runtime.engine_route_lids_r[engine_id][voice][last]
                    .store(0, Ordering::Relaxed);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.runtime.engine_ext_route_lids[engine_id][voice][last][input]
                        .store(0, Ordering::Relaxed);
                }
            }
        }
    }

    /// Delete one track from live sequencer state and compact higher track indices.
    ///
    /// This state-side helper is the non-graph half of track deletion semantics:
    /// the deleted lane disappears from the current pattern and all snapshots in
    /// memory, higher lanes shift down immediately, and the old trailing lane is
    /// cleared so stale state cannot leak back after future restores.
    pub fn remove_track(
        &self,
        track_idx: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
        effect_descriptors: &[Vec<EffectDescriptor>],
    ) -> bool {
        let old_count = self.active_track_count();
        if old_count <= 1 || track_idx >= old_count {
            return false;
        }

        let current_pattern = self.current_scene_index();
        let mut current_snapshot = self.capture_current_pattern_snapshot(
            old_count,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes.save_scene_snapshot(current_pattern, current_snapshot.clone());
            for owner_track in 0..old_count {
                if owner_track == track_idx {
                    continue;
                }
                let Some(track_descs) = effect_descriptors.get(owner_track) else {
                    continue;
                };
                let Some(pool) = scenes.track_pools.get_mut(owner_track) else {
                    continue;
                };
                for data in pool.patterns.values_mut() {
                    data.remap_sidechain_references_after_track_delete(
                        owner_track,
                        track_descs,
                        track_idx,
                        old_count,
                    );
                }
            }
            scenes.remove_track(track_idx);
        }

        remap_snapshot_sidechain_references_after_track_delete(
            &mut current_snapshot,
            effect_descriptors,
            track_idx,
            old_count,
        );
        current_snapshot.remove_track(track_idx);
        current_snapshot.restore(self);
        self.shift_runtime_track_bindings_left(track_idx, old_count);
        self.clear_live_track_lane(old_count - 1);
        self.transport
            .num_tracks
            .store((old_count - 1) as u32, Ordering::Release);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.request_all_accumulator_resets();
        self.publish_scheduler_snapshot();
        true
    }

    pub fn clear_track_in_place(
        &self,
        track_idx: usize,
        effect_descriptors: &[Vec<EffectDescriptor>],
    ) -> bool {
        let track_count = self.active_track_count();
        if track_idx >= track_count {
            return false;
        }

        {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let Some(pool) = scenes.track_pools.get_mut(track_idx) else {
                return false;
            };
            for data in pool.patterns.values_mut() {
                data.clear(track_idx, effect_descriptors, InstrumentType::Sampler);
            }
        }

        self.clear_live_track_lane(track_idx);
        self.clear_runtime_track_binding_in_place(track_idx);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.request_all_accumulator_resets();
        self.publish_scheduler_snapshot();
        true
    }

    pub fn scratch_source(&self) -> String {
        self.scratch_source.lock().unwrap().clone()
    }
    pub fn scratch_source_version(&self) -> u64 {
        self.scratch_source_version.load(Ordering::Acquire)
    }
    pub fn set_scratch_source(&self, source: impl Into<String>) {
        *self.scratch_source.lock().unwrap() = source.into();
        self.scratch_source_version.fetch_add(1, Ordering::AcqRel);
    }
    /// Publish (upsert by id) a UI-authored generator definition for the scheduler.
    pub fn publish_sequencer(&self, sequencer: PublishedSequencer) {
        {
            let mut list = self.published_sequencers.lock().unwrap();
            if let Some(existing) = list.iter_mut().find(|s| s.id == sequencer.id) {
                *existing = sequencer;
            } else {
                list.push(sequencer);
            }
        }
        self.published_sequencers_version
            .fetch_add(1, Ordering::AcqRel);
    }
    pub fn unpublish_sequencer_by_name(&self, name: &str) -> bool {
        let removed = {
            let mut list = self.published_sequencers.lock().unwrap();
            let before = list.len();
            list.retain(|sequencer| sequencer.name != name);
            list.len() != before
        };
        if removed {
            self.published_sequencers_version
                .fetch_add(1, Ordering::AcqRel);
        }
        removed
    }
    pub fn published_sequencers(&self) -> Vec<PublishedSequencer> {
        self.published_sequencers.lock().unwrap().clone()
    }
    pub fn published_sequencers_version(&self) -> u64 {
        self.published_sequencers_version.load(Ordering::Acquire)
    }
    /// Publish the complete UI-authored process/channel authoring snapshot.
    pub fn publish_process_authoring(
        &self,
        snapshot: crate::process::PublishedProcessAuthoringSnapshot,
    ) {
        *self.published_process_authoring.lock().unwrap() = snapshot;
        self.published_process_authoring_version
            .fetch_add(1, Ordering::AcqRel);
    }
    pub fn published_process_authoring(&self) -> crate::process::PublishedProcessAuthoringSnapshot {
        self.published_process_authoring.lock().unwrap().clone()
    }
    pub fn published_process_authoring_version(&self) -> u64 {
        self.published_process_authoring_version
            .load(Ordering::Acquire)
    }
    pub fn track_process_chain(&self, track: usize) -> Option<crate::process::TrackProcessChain> {
        if track >= self.active_track_count() {
            return None;
        }
        self.pattern
            .process_chains
            .lock()
            .unwrap()
            .get(track)
            .cloned()
    }
    pub fn set_track_process_chain(
        &self,
        track: usize,
        chain: crate::process::TrackProcessChain,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let mut chains = self.pattern.process_chains.lock().unwrap();
        let Some(slot) = chains.get_mut(track) else {
            return false;
        };
        *slot = chain;
        drop(chains);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }
    fn publish_process_chain_edit(&self) {
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
    }
    /// A track's effective chain as the scheduler sees it: project-layer
    /// slots ahead of the track's own slots. UI surfaces read this composed
    /// view so project slots appear (badged) in every track's process column.
    pub fn composed_track_process_chain(
        &self,
        track: usize,
    ) -> Option<crate::process::TrackProcessChain> {
        let track_chain = self.track_process_chain(track)?;
        let project_chain = self.project_process_chain();
        Some(crate::process::compose_effective_process_chain(
            &project_chain,
            &track_chain,
        ))
    }
    /// The project-level default process chain for the current scene. Every
    /// track — present and future — runs these slots ahead of its own chain.
    pub fn project_process_chain(&self) -> crate::process::TrackProcessChain {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .current_project_process_chain()
    }
    /// Whole-layer replace of the project process chain (`(processes :project ...)`).
    pub fn set_project_process_chain(&self, chain: crate::process::TrackProcessChain) -> bool {
        let updated = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes
                .edit_current_project_process_chain(|current| {
                    *current = chain;
                    Ok(())
                })
                .is_ok()
        };
        if updated {
            self.publish_process_chain_edit();
        }
        updated
    }
    fn edit_project_process_chain_slot<R>(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        edit: impl FnOnce(&mut crate::process::TrackProcessSlot) -> R,
    ) -> Option<R> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes
            .edit_current_project_process_chain(|chain| {
                Ok(chain
                    .slots
                    .iter_mut()
                    .find(|slot| slot.instance_id == instance_id)
                    .map(edit))
            })
            .ok()
            .flatten()
    }
    /// Enable or bypass one pattern-scoped process-chain slot.
    ///
    /// Returns `false` only when the track or instance does not exist. A
    /// repeated write of the current value is a successful no-op.
    pub fn set_track_process_slot_enabled(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        enabled: bool,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            match chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
            {
                Some(slot) => {
                    let changed = slot.enabled != enabled;
                    slot.enabled = enabled;
                    Some(changed)
                }
                None => None,
            }
        };
        // Project-layer slots are editable from any track's panel; the toggle
        // lands on the one shared object.
        let Some(changed) = changed.or_else(|| {
            self.edit_project_process_chain_slot(instance_id, |slot| {
                let changed = slot.enabled != enabled;
                slot.enabled = enabled;
                changed
            })
        }) else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    /// Move a slot before another instance, or to the end when `before` is
    /// `None`. Instance ids make this stable across reactive UI refreshes and
    /// avoid index-shift ambiguity while dragging downward.
    pub fn move_track_process_slot_before(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        before: Option<crate::process::ProcessInstanceId>,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        fn move_slot_within_chain(
            chain: &mut crate::process::TrackProcessChain,
            instance_id: crate::process::ProcessInstanceId,
            before: Option<crate::process::ProcessInstanceId>,
        ) -> Option<bool> {
            let source_index = chain
                .slots
                .iter()
                .position(|slot| slot.instance_id == instance_id)?;
            if before == Some(instance_id) {
                return Some(false);
            }
            if before
                .is_some_and(|target| !chain.slots.iter().any(|slot| slot.instance_id == target))
            {
                return None;
            }
            let previous_order = chain
                .slots
                .iter()
                .map(|slot| slot.instance_id)
                .collect::<Vec<_>>();
            let slot = chain.slots.remove(source_index);
            let target_index = before
                .and_then(|target| {
                    chain
                        .slots
                        .iter()
                        .position(|slot| slot.instance_id == target)
                })
                .unwrap_or(chain.slots.len());
            chain.slots.insert(target_index, slot);
            Some(
                previous_order
                    != chain
                        .slots
                        .iter()
                        .map(|slot| slot.instance_id)
                        .collect::<Vec<_>>(),
            )
        }
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            move_slot_within_chain(chain, instance_id, before)
        };
        // Reordering a project slot moves it within the project layer only.
        let changed = changed.or_else(|| {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes
                .edit_current_project_process_chain(|chain| {
                    Ok(move_slot_within_chain(chain, instance_id, before))
                })
                .ok()
                .flatten()
        });
        let Some(changed) = changed else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    /// Detach one slot from one track in the current pattern.
    pub fn remove_track_process_slot(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let removed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            let previous_len = chain.slots.len();
            chain.slots.retain(|slot| slot.instance_id != instance_id);
            chain.slots.len() != previous_len
        };
        // A project slot has no per-track detach: removing it from any track's
        // panel removes the shared slot from the project layer.
        let removed = removed || {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes
                .edit_current_project_process_chain(|chain| {
                    let previous_len = chain.slots.len();
                    chain.slots.retain(|slot| slot.instance_id != instance_id);
                    Ok(chain.slots.len() != previous_len)
                })
                .unwrap_or(false)
        };
        if removed {
            self.publish_process_chain_edit();
        }
        removed
    }
    pub fn set_process_lane_value(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: impl Into<String>,
        step: usize,
        value: f32,
    ) -> bool {
        if track >= self.active_track_count() || step >= MAX_STEPS {
            return false;
        }
        let inlet_name = inlet_name.into();
        let write_lane_step = |slot: &mut crate::process::TrackProcessSlot| {
            let lane = slot.lanes.entry(inlet_name.clone()).or_default();
            if lane.values.len() <= step {
                lane.values.resize(step + 1, 0.0);
            }
            lane.values[step] = value;
        };
        let updated = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            match chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
            {
                Some(slot) => {
                    write_lane_step(slot);
                    true
                }
                None => false,
            }
        };
        // Project-layer lanes are singular: an edit from any track's lane UI
        // writes the one shared lane.
        if !updated
            && self
                .edit_project_process_chain_slot(instance_id, write_lane_step)
                .is_none()
        {
            return false;
        }
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }
    /// Replace a scalar inlet on every current-pattern chain slot owned by
    /// `instance_id`. This is the durable counterpart to authoring-handle knob
    /// edits like `(climb :limit 6)`: it updates pattern-scoped attachment
    /// state without touching step data or p-lock storage.
    pub fn set_process_inlet_value(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
        value: crate::process::ProcessLiteral,
    ) -> usize {
        let mut updated = 0;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    slot.inlets.insert(inlet_name.to_string(), value.clone());
                    updated += 1;
                }
            }
        }
        if self
            .edit_project_process_chain_slot(instance_id, |slot| {
                slot.inlets.insert(inlet_name.to_string(), value.clone());
            })
            .is_some()
        {
            updated += 1;
        }
        if updated > 0 {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        updated
    }
    /// Replace a scalar inlet on one track attachment. UI slot editors use
    /// this track-local form; authored process handles intentionally retain
    /// the all-attachments behavior of `set_process_inlet_value` above.
    pub fn set_track_process_inlet_value(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
        value: crate::process::ProcessLiteral,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
                .map(|slot| {
                    let changed = slot.inlets.get(inlet_name) != Some(&value);
                    slot.inlets.insert(inlet_name.to_string(), value.clone());
                    changed
                })
        };
        let Some(changed) = changed.or_else(|| {
            self.edit_project_process_chain_slot(instance_id, |slot| {
                let changed = slot.inlets.get(inlet_name) != Some(&value);
                slot.inlets.insert(inlet_name.to_string(), value.clone());
                changed
            })
        }) else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    pub fn set_process_port_binding(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
        target: crate::process::ParamTarget,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }

        let apply = |slot: &mut crate::process::TrackProcessSlot| {
            let current = slot.bindings.get(port_name);
            if matches!(current, Some(Some(existing)) if existing == &target) {
                false
            } else {
                slot.bindings
                    .insert(port_name.to_string(), Some(target.clone()));
                true
            }
        };
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
                .map(apply)
        };
        let Some(changed) =
            changed.or_else(|| self.edit_project_process_chain_slot(instance_id, apply))
        else {
            return false;
        };
        if changed {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        true
    }
    /// Bind a process target port on every current-pattern chain slot owned by
    /// `instance_id`. This mirrors `set_process_lane_values`: authored Lisp and
    /// UI interactions both update the pattern-owned slots currently attached to
    /// tracks without touching step/plock storage.
    pub fn set_process_port_binding_for_instance(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
        target: crate::process::ParamTarget,
    ) -> usize {
        let mut updated = 0;
        let mut changed = false;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    updated += 1;
                    let current = slot.bindings.get(port_name);
                    if !matches!(current, Some(Some(existing)) if existing == &target) {
                        slot.bindings
                            .insert(port_name.to_string(), Some(target.clone()));
                        changed = true;
                    }
                }
            }
        }
        if let Some(project_changed) = self.edit_project_process_chain_slot(instance_id, |slot| {
            let current = slot.bindings.get(port_name);
            if matches!(current, Some(Some(existing)) if existing == &target) {
                false
            } else {
                slot.bindings
                    .insert(port_name.to_string(), Some(target.clone()));
                true
            }
        }) {
            updated += 1;
            changed |= project_changed;
        }
        if changed {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        updated
    }
    pub fn clear_process_port_binding(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }

        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
                .map(|slot| slot.bindings.remove(port_name).is_some())
        };
        let Some(changed) = changed.or_else(|| {
            self.edit_project_process_chain_slot(instance_id, |slot| {
                slot.bindings.remove(port_name).is_some()
            })
        }) else {
            return false;
        };
        if changed {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        true
    }
    /// Replace a lane wholesale on every current-pattern chain slot owned by
    /// `instance_id` (a handle can be attached to several tracks). Returns the
    /// number of slots updated; publishes only when at least one matched.
    pub fn set_process_lane_values(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
        values: Vec<f32>,
    ) -> usize {
        let mut updated = 0;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    slot.lanes.insert(
                        inlet_name.to_string(),
                        crate::process::ProcessLane {
                            values: values.clone(),
                        },
                    );
                    updated += 1;
                }
            }
        }
        if self
            .edit_project_process_chain_slot(instance_id, |slot| {
                slot.lanes.insert(
                    inlet_name.to_string(),
                    crate::process::ProcessLane {
                        values: values.clone(),
                    },
                );
            })
            .is_some()
        {
            updated += 1;
        }
        if updated > 0 {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        updated
    }
    pub fn process_instance_attachment_count(
        &self,
        instance_id: crate::process::ProcessInstanceId,
    ) -> usize {
        let track_attachments: usize = self
            .pattern
            .process_chains
            .lock()
            .unwrap()
            .iter()
            .map(|chain| {
                chain
                    .slots
                    .iter()
                    .filter(|slot| slot.instance_id == instance_id)
                    .count()
            })
            .sum();
        let project_attachments = self
            .project_process_chain()
            .slots
            .iter()
            .filter(|slot| slot.instance_id == instance_id)
            .count();
        track_attachments + project_attachments
    }
    pub fn scratch_runtime_descriptors(
        &self,
    ) -> (Vec<Vec<EffectDescriptor>>, Vec<EffectDescriptor>) {
        (
            self.scratch_effect_descriptors.lock().unwrap().clone(),
            self.scratch_instrument_descriptors.lock().unwrap().clone(),
        )
    }
    pub fn set_scratch_runtime_descriptors(
        &self,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
    ) {
        {
            *self.scratch_effect_descriptors.lock().unwrap() = effect_descriptors;
        }
        {
            *self.scratch_instrument_descriptors.lock().unwrap() = instrument_descriptors;
        }
        self.publish_scheduler_snapshot();
    }
    pub fn process_trace_enabled(&self) -> bool {
        self.process_trace_enabled.load(Ordering::Relaxed)
    }
    pub fn set_process_trace_enabled(&self, enabled: bool) {
        let previous = self.process_trace_enabled.swap(enabled, Ordering::Relaxed);
        if previous != enabled {
            self.publish_scheduler_snapshot();
        }
    }
    pub fn request_accumulator_reset(&self, track: usize) {
        if track < MAX_TRACKS {
            self.pending_accumulator_reset_tracks[track].store(true, Ordering::Release);
        }
    }
    pub fn request_all_accumulator_resets(&self) {
        self.pending_accumulator_reset_all
            .store(true, Ordering::Release);
    }
    pub fn take_accumulator_reset_requests(&self) -> (bool, [bool; MAX_TRACKS]) {
        let all = self
            .pending_accumulator_reset_all
            .swap(false, Ordering::AcqRel);
        let mut tracks = [false; MAX_TRACKS];
        for (idx, flag) in tracks.iter_mut().enumerate() {
            *flag = self.pending_accumulator_reset_tracks[idx].swap(false, Ordering::AcqRel);
        }
        (all, tracks)
    }
    pub fn current_step(&self) -> usize {
        self.transport.playhead.load(Ordering::Relaxed) as usize
    }
    pub fn track_step(&self, track: usize) -> usize {
        self.transport.track_playheads[track].load(Ordering::Relaxed) as usize
    }
    pub fn is_playing(&self) -> bool {
        self.transport.playing.load(Ordering::Relaxed)
    }

    pub fn start_playback(&self) {
        self.reset_playheads();
        self.transport.playing.store(true, Ordering::Relaxed);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
    }

    pub fn stop_playback(&self) {
        self.transport.playing.store(false, Ordering::Relaxed);
        self.reset_playheads();
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
    }

    pub fn toggle_play(&self) -> bool {
        if self.is_playing() {
            self.stop_playback();
            false
        } else {
            self.start_playback();
            true
        }
    }

    pub fn reset_playheads(&self) {
        self.transport.playhead.store(0, Ordering::Relaxed);
        self.transport
            .playhead_phase
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
        for playhead in &self.transport.track_playheads {
            playhead.store(0, Ordering::Relaxed);
        }
        for playhead in &self.transport.sampler_playheads {
            playhead.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
    }
    /// Publish a snapshot of all pattern/transport atomics so that future
    /// snapshot-based audio-thread readers can pick up the latest state.
    ///
    /// Currently this is a **no-op** because the audio thread reads atomics
    /// directly from `SequencerState`.  The method exists as a hook for the
    /// planned `Arc<SequencerSnapshot>` architecture — once that lands, this
    pub fn schedule_mod_resync(&self) {
        if self.is_playing() {
            self.transport
                .pending_mod_resync
                .store(true, Ordering::Relaxed);
        } else {
            self.transport
                .mod_reset_counter
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn scene_track_pattern_id(&self, scene: usize, track: usize) -> Option<PatternId> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scenes
            .get(scene)?
            .cells
            .get(track)
            .copied()
            .flatten()
    }

    pub fn track_pattern_cells(&self, track: usize) -> Vec<TrackPatternCellView> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .track_pattern_cells(track)
    }

    pub fn launch_scene(
        &self,
        scene_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String, u32)>> {
        self.launch_scene_profiled(
            scene_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        )
        .map(|result| result.sample_ids)
    }

    pub fn launch_scene_profiled(
        &self,
        scene_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternSwitchResult> {
        let total_started = Instant::now();
        let mut profile = PatternSwitchProfile::default();

        let started = Instant::now();
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        profile.capture_current_snapshot = started.elapsed();

        let (sample_ids, snapshot_source) = {
            let started = Instant::now();
            let mut scenes = self.pattern.scenes.lock().unwrap();
            profile.scene_lock_wait = started.elapsed();

            let current_scene = self.current_scene_index();
            if scene_idx >= scenes.scene_count() {
                return None;
            }

            let started = Instant::now();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            profile.save_current_snapshot = started.elapsed();

            let started = Instant::now();
            let launched = scenes.launch_scene(scene_idx)?;
            profile.launch_scene_data = started.elapsed();

            let started = Instant::now();
            for (track, data) in launched.iter().enumerate() {
                if let Some(data) = data {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                } else {
                    self.set_scene_silenced(track, true);
                }
            }
            profile.restore_tracks = started.elapsed();

            let started = Instant::now();
            let sample_ids = scenes.scene_sample_ids(scene_idx).unwrap_or_default();
            profile.collect_sample_ids = started.elapsed();

            let started = Instant::now();
            self.pattern
                .current_pattern
                .store(scene_idx as u32, Ordering::Relaxed);
            self.pattern
                .num_patterns
                .store(scenes.scene_count() as u32, Ordering::Relaxed);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            profile.update_pattern_atoms = started.elapsed();

            let metadata = scenes.current_scene_metadata();
            let project_process_chain = scenes.current_project_process_chain();
            let snapshot_source = launched
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .map(|tracks| {
                    (
                        tracks,
                        metadata.0,
                        metadata.1,
                        metadata.2,
                        project_process_chain,
                    )
                });

            (sample_ids, snapshot_source)
        };

        let started = Instant::now();
        self.schedule_mod_resync();
        profile.schedule_mod_resync = started.elapsed();

        let started = Instant::now();
        if let Some((
            tracks,
            mod_connections,
            neural_networks,
            graph_overrides,
            project_process_chain,
        )) = snapshot_source
        {
            self.publish_scheduler_snapshot_from_track_pattern_data(
                &tracks,
                mod_connections,
                neural_networks,
                graph_overrides,
                project_process_chain,
            );
        } else {
            self.publish_scheduler_snapshot();
        }
        profile.publish_scheduler_snapshot = started.elapsed();
        profile.total = total_started.elapsed();

        Some(PatternSwitchResult {
            sample_ids,
            profile,
        })
    }

    pub fn launch_track_pattern(
        &self,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        if track >= num_tracks {
            return false;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let launched = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
                return false;
            }
            scenes.launch_track_pattern(track, pattern_id)
        };
        let Some(data) = launched else {
            return false;
        };
        data.restore_to(self, track);
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }

    pub fn fork_current_track_pattern(
        &self,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let id = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            scenes.fork_track_pattern(track)?
        };
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Some(id)
    }

    pub fn clone_current_scene_track_pattern(
        &self,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let (id, data) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            let id = scenes.clone_track_pattern_into_current_scene(track)?;
            let data = scenes.effective_track_pattern(track)?.clone();
            (id, data)
        };
        data.restore_to(self, track);
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Some(id)
    }

    pub fn clone_track_pattern_id_into_current_scene(
        &self,
        track: usize,
        source_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let (id, data) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            let id = scenes.clone_track_pattern_id_into_current_scene(track, source_id)?;
            let data = scenes.effective_track_pattern(track)?.clone();
            (id, data)
        };
        data.restore_to(self, track);
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Some(id)
    }

    pub fn delete_track_pattern(
        &self,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        if track >= num_tracks {
            return false;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let (was_effective, replacement) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            let was_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
            if !scenes.delete_track_pattern(track, pattern_id) {
                return false;
            }
            let replacement = if was_effective {
                scenes.effective_track_pattern(track).cloned()
            } else {
                None
            };
            (was_effective, replacement)
        };

        if was_effective {
            if let Some(data) = replacement {
                data.restore_to(self, track);
                self.set_scene_silenced(track, false);
            } else {
                self.set_scene_silenced(track, true);
            }
        }
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }

    pub fn set_scene_cell(
        &self,
        scene: usize,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        if track >= num_tracks {
            return false;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let restore_current_track = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
                return false;
            }
            if !scenes.set_cell(scene, track, pattern_id) {
                return false;
            }
            if scene == current_scene {
                if let Some(override_slot) = scenes.track_overrides.get_mut(track) {
                    *override_slot = None;
                }
                scenes
                    .track_pools
                    .get(track)
                    .and_then(|pool| pool.get(pattern_id))
                    .cloned()
            } else {
                None
            }
        };

        if let Some(data) = restore_current_track {
            data.restore_to(self, track);
            self.set_scene_silenced(track, false);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        true
    }

    pub fn clear_scene_cell(
        &self,
        scene: usize,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let (cleared, should_silence) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
                return None;
            }
            let cleared = scenes.clear_cell(scene, track)?;
            let should_silence =
                scene == current_scene && scenes.effective_pattern_id(track).is_none();
            (cleared, should_silence)
        };

        if should_silence {
            self.set_scene_silenced(track, true);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        Some(cleared)
    }

    pub fn switch_pattern(
        &self,
        new_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String, u32)>> {
        self.switch_pattern_profiled(
            new_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        )
        .map(|result| result.sample_ids)
    }

    pub fn switch_pattern_profiled(
        &self,
        new_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternSwitchResult> {
        let cur = self.current_scene_index();
        if new_idx == cur {
            return None;
        }
        self.launch_scene_profiled(
            new_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        )
    }

    pub fn clone_pattern(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> usize {
        let new_idx = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let cur = self.current_scene_index();
            let current_metadata = scenes.current_scene_metadata();
            let current_snapshot = PatternSnapshot::capture_with_mod_connections(
                self,
                num_tracks,
                buffer_ids,
                sample_rates,
                names,
                instrument_types,
                current_metadata.0,
                current_metadata.1,
                current_metadata.2,
            );
            scenes.save_scene_snapshot(cur, current_snapshot);
            let new_idx = scenes.new_scene();
            self.pattern
                .current_pattern
                .store(new_idx as u32, Ordering::Relaxed);
            self.pattern
                .num_patterns
                .store(scenes.scene_count() as u32, Ordering::Relaxed);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            new_idx
        };
        self.publish_scheduler_snapshot();
        new_idx
    }

    pub fn delete_pattern(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String, u32)>> {
        let sample_ids = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            if scenes.scene_count() <= 1 {
                return None;
            }
            let cur = self.current_scene_index();
            let current_metadata = scenes.current_scene_metadata();
            let current_snapshot = PatternSnapshot::capture_with_mod_connections(
                self,
                num_tracks,
                buffer_ids,
                sample_rates,
                names,
                instrument_types,
                current_metadata.0,
                current_metadata.1,
                current_metadata.2,
            );
            scenes.save_scene_snapshot(cur, current_snapshot);
            let new_idx = scenes.delete_scene(cur)?;
            let launched = scenes.launch_scene(new_idx)?;
            for (track, data) in launched.into_iter().enumerate() {
                if let Some(data) = data {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                } else {
                    self.set_scene_silenced(track, true);
                }
            }
            let sample_ids = scenes
                .scene_snapshot(new_idx)
                .map(|snapshot| snapshot.sample_ids)
                .unwrap_or_default();
            self.pattern
                .current_pattern
                .store(new_idx as u32, Ordering::Relaxed);
            self.pattern
                .num_patterns
                .store(scenes.scene_count() as u32, Ordering::Relaxed);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            sample_ids
        };
        self.schedule_mod_resync();
        self.publish_scheduler_snapshot();
        Some(sample_ids)
    }

    pub fn propagate_track_to_all_patterns(
        &self,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let cur = self.current_scene_index();
        if cur >= scenes.scene_count() || track >= num_tracks {
            return false;
        }
        let current_metadata = scenes.current_scene_metadata();
        let current_snapshot = PatternSnapshot::capture_with_mod_connections(
            self,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            current_metadata.0,
            current_metadata.1,
            current_metadata.2,
        );
        scenes.save_scene_snapshot(cur, current_snapshot);
        let Some(source) = scenes.scene_snapshot(cur) else {
            return false;
        };
        let mut snapshots = scenes.snapshots();
        for (pattern_idx, snapshot) in snapshots.iter_mut().enumerate() {
            if pattern_idx != cur {
                snapshot.clone_track_lane_from(&source, track);
            }
        }
        let bus_patterns = scenes
            .scenes
            .iter()
            .map(|scene| scene.bus_patterns.clone())
            .collect::<Vec<_>>();
        let mut rebuilt = ProjectScenes::from_pattern_snapshots(&snapshots, cur);
        for (scene, bus_patterns) in rebuilt.scenes.iter_mut().zip(bus_patterns) {
            scene.bus_patterns = bus_patterns;
        }
        *scenes = rebuilt;
        true
    }

    pub fn toggle_step_and_clear_plocks(&self, track: usize, step: usize) {
        let was_active = self.pattern.patterns[track].is_active(step);
        self.pattern.patterns[track].toggle_step(step);
        if was_active {
            for slot in &self.pattern.effect_chains[track] {
                slot.plocks.clear_step(step);
            }
            self.pattern.timebase_plocks[track].clear(step);
            self.pattern.swing_plocks[track].clear(step);
            self.pattern.swing_resolution_plocks[track].clear(step);
            self.pattern.neural_reset_patterns[track].clear_step(step);
            for param_idx in 0..MAX_SLOT_PARAMS {
                self.pattern.instrument_slots[track]
                    .plocks
                    .clear_param(step, param_idx);
            }
            if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
                for slot in &mut rack.slots {
                    slot.param_plocks.clear_step(step);
                    slot.instrument_slot.clear_step_plocks(step);
                }
            }
            self.pattern.chord_data[track].clear_step(step);
        }
        self.publish_scheduler_snapshot();
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

        let mut effect_plocks = Vec::with_capacity(self.pattern.effect_chains[track].len());
        for slot in &self.pattern.effect_chains[track] {
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            let mut params = Vec::with_capacity(num_params);
            for param_idx in 0..num_params {
                params.push(slot.plocks.get(step, param_idx));
            }
            effect_plocks.push(StepSlotPlocks { params });
        }

        let instrument_slot = &self.pattern.instrument_slots[track];
        let instrument_param_count = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
        let mut instrument_plocks = Vec::with_capacity(instrument_param_count);
        for param_idx in 0..instrument_param_count {
            instrument_plocks.push(instrument_slot.plocks.get(step, param_idx));
        }
        let (rack_slot_param_plocks, rack_slot_instrument_plocks) = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(|rack| rack.as_ref())
            .map(|rack| {
                let slot_params = rack
                    .slots
                    .iter()
                    .map(|slot| {
                        let params = RackSlotParam::ALL
                            .iter()
                            .map(|param| slot.param_plocks.get(step, *param))
                            .collect();
                        StepSlotPlocks { params }
                    })
                    .collect();
                let instrument_params = rack
                    .slots
                    .iter()
                    .map(|slot| {
                        let num_params = slot.instrument_slot.num_params as usize;
                        let params = (0..num_params)
                            .map(|param_idx| {
                                slot.instrument_slot
                                    .plocks
                                    .get(step)
                                    .and_then(|step_plocks| step_plocks.get(param_idx))
                                    .copied()
                                    .flatten()
                            })
                            .collect();
                        StepSlotPlocks { params }
                    })
                    .collect();
                (slot_params, instrument_params)
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
            effect_plocks,
            instrument_plocks: StepSlotPlocks {
                params: instrument_plocks,
            },
            rack_slot_param_plocks,
            rack_slot_instrument_plocks,
        }
    }

    fn clear_step_payload_inner(&self, track: usize, step: usize) {
        for param in StepParam::ALL {
            self.pattern.step_data[track].set(step, param, param.default_value());
        }

        self.pattern.patterns[track].clear_step(step);
        self.pattern.neural_reset_patterns[track].clear_step(step);

        self.pattern.chord_data[track].clear_step(step);
        self.pattern.timebase_plocks[track].clear(step);
        self.pattern.swing_plocks[track].clear(step);
        self.pattern.swing_resolution_plocks[track].clear(step);

        for slot in &self.pattern.effect_chains[track] {
            slot.plocks.clear_step(step);
        }

        for param_idx in 0..MAX_SLOT_PARAMS {
            self.pattern.instrument_slots[track]
                .plocks
                .clear_param(step, param_idx);
        }
        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            for slot in &mut rack.slots {
                slot.param_plocks.clear_step(step);
                slot.instrument_slot.clear_step_plocks(step);
            }
        }
    }

    pub fn clear_step_payload(&self, track: usize, step: usize) {
        self.clear_step_payload_inner(track, step);
        self.publish_scheduler_snapshot();
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
        if track >= self.pattern.instrument_slots.len() {
            return false;
        }
        let mut changed = false;
        for step in steps.iter().copied().filter(|step| *step < MAX_STEPS) {
            changed |= self.clear_variant_locks_for_step_inner(track, step);
        }
        if changed {
            let _ = self.reconcile_plock_variant_registry_for_track(track);
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub fn stamp_variant_key_to_steps(
        &self,
        track: usize,
        key: &PlockVariantKey,
        steps: &[usize],
    ) -> bool {
        let Some(source_step) = self.find_step_with_variant_key(track, key) else {
            return false;
        };
        self.copy_variant_locks_from_step_to_steps(track, source_step, steps)
    }

    pub fn copy_variant_locks_from_step_to_steps(
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
            self.publish_scheduler_snapshot();
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

    fn clear_variant_locks_for_step_inner(&self, track: usize, step: usize) -> bool {
        let mut changed = false;
        for slot in &self.pattern.effect_chains[track] {
            changed |= clear_live_slot_variant_locks(slot, step);
        }
        changed |= clear_live_slot_variant_locks(&self.pattern.instrument_slots[track], step);
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
        let mut changed = false;
        for slot in &self.pattern.effect_chains[track] {
            changed |= copy_live_slot_variant_locks(slot, source_step, target_step);
        }
        changed |= copy_live_slot_variant_locks(
            &self.pattern.instrument_slots[track],
            source_step,
            target_step,
        );
        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            for slot in &mut rack.slots {
                changed |= copy_rack_slot_variant_locks(slot, source_step, target_step);
            }
        }
        changed
    }

    fn set_step_param_inner(&self, track: usize, step: usize, param: StepParam, value: f32) {
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

    fn restore_step_snapshot_inner(&self, track: usize, step: usize, snapshot: &StepSnapshot) {
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

        for (slot_idx, slot) in self.pattern.effect_chains[track].iter().enumerate() {
            let saved = snapshot.effect_plocks.get(slot_idx);
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            for param_idx in 0..num_params {
                let val = saved
                    .and_then(|plocks| plocks.params.get(param_idx))
                    .copied()
                    .flatten();
                match val {
                    Some(val) => slot.set_plock(step, param_idx, val),
                    None => slot.plocks.clear_param(step, param_idx),
                }
            }
        }

        let instrument_slot = &self.pattern.instrument_slots[track];
        let instrument_param_count = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
        for param_idx in 0..instrument_param_count {
            match snapshot
                .instrument_plocks
                .params
                .get(param_idx)
                .copied()
                .flatten()
            {
                Some(val) => instrument_slot.set_plock(step, param_idx, val),
                None => instrument_slot.plocks.clear_param(step, param_idx),
            }
        }

        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
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

                let saved = snapshot.rack_slot_instrument_plocks.get(slot_idx);
                let num_params = slot.instrument_slot.num_params as usize;
                for param_idx in 0..num_params {
                    let value = saved
                        .and_then(|plocks| plocks.params.get(param_idx))
                        .copied()
                        .flatten();
                    match value {
                        Some(value) => {
                            slot.instrument_slot.set_plock(step, param_idx, value);
                        }
                        None => {
                            slot.instrument_slot.clear_plock(step, param_idx);
                        }
                    }
                }
            }
        }
    }

    pub fn restore_step_snapshot(&self, track: usize, step: usize, snapshot: &StepSnapshot) {
        self.restore_step_snapshot_inner(track, step, snapshot);
        self.publish_scheduler_snapshot();
    }

    /// Cyclically rotate `steps` (sorted) left (direction < 0) or right (direction > 0).
    pub fn rotate_steps(&self, track: usize, steps: &[usize], direction: isize) {
        if steps.len() < 2 {
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
        self.publish_scheduler_snapshot();
    }

    pub fn move_step_range(&self, track: usize, lo: usize, hi: usize, new_lo: usize) {
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
        self.publish_scheduler_snapshot();
    }

    pub fn duplicate_track_pattern(&self, track: usize) -> usize {
        let num_steps = self.pattern.track_params[track].get_num_steps();
        let new_len = (num_steps * 2).min(MAX_STEPS);
        if new_len == num_steps {
            return num_steps;
        }

        for step in num_steps..new_len {
            let src = step - num_steps;
            let active = self.pattern.patterns[track].is_active(src);
            self.pattern.patterns[track].set_step_active(step, active);
            let neural_reset = self.pattern.neural_reset_patterns[track].is_active(src);
            self.pattern.neural_reset_patterns[track].set_step_active(step, neural_reset);
        }

        for step in num_steps..new_len {
            let src = step - num_steps;
            for param in StepParam::ALL {
                let val = self.pattern.step_data[track].get(src, param);
                self.pattern.step_data[track].set(step, param, val);
            }
        }

        for slot in &self.pattern.effect_chains[track] {
            let np = slot.num_params.load(Ordering::Relaxed) as usize;
            for step in num_steps..new_len {
                let src = step - num_steps;
                for p in 0..np {
                    match slot.plocks.get(src, p) {
                        Some(val) => slot.set_plock(step, p, val),
                        None => slot.plocks.clear_param(step, p),
                    }
                }
            }
        }

        for step in num_steps..new_len {
            let src = step - num_steps;
            self.pattern.chord_data[track].copy_step(src, step);
        }

        for step in num_steps..new_len {
            let src = step - num_steps;
            match self.pattern.timebase_plocks[track].get(src) {
                Some(tb) => self.pattern.timebase_plocks[track].set(step, tb),
                None => self.pattern.timebase_plocks[track].clear(step),
            }
            match self.pattern.swing_plocks[track].get(src) {
                Some(swing) => self.pattern.swing_plocks[track].set(step, swing),
                None => self.pattern.swing_plocks[track].clear(step),
            }
            match self.pattern.swing_resolution_plocks[track].get(src) {
                Some(resolution) => {
                    self.pattern.swing_resolution_plocks[track].set(step, resolution)
                }
                None => self.pattern.swing_resolution_plocks[track].clear(step),
            }
        }

        self.pattern.track_params[track].set_num_steps(new_len);
        self.publish_scheduler_snapshot();
        new_len
    }

    pub fn halve_track_pattern(&self, track: usize) -> usize {
        let num_steps = self.pattern.track_params[track].get_num_steps();
        let new_len = (num_steps / 2).max(1);
        if new_len == num_steps {
            return num_steps;
        }
        self.pattern.track_params[track].set_num_steps(new_len);
        self.publish_scheduler_snapshot();
        new_len
    }
}

fn option_f32_bits_equal(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.to_bits() == b.to_bits(),
        (None, None) => true,
        _ => false,
    }
}

fn f32_slices_bits_equal(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(left, right)| left.to_bits() == right.to_bits())
}

fn clear_live_slot_variant_locks(slot: &EffectSlotState, step: usize) -> bool {
    let mut changed = false;
    let num_params = (slot.num_params.load(Ordering::Relaxed) as usize).min(MAX_SLOT_PARAMS);
    for param_idx in 0..num_params {
        if slot.plocks.get(step, param_idx).is_some() {
            slot.plocks.clear_param(step, param_idx);
            changed = true;
        }
    }
    for tensor_idx in 0..slot.tensor_params.num_params() {
        if slot.tensor_params.plock_values(step, tensor_idx).is_some() {
            slot.tensor_params.clear_plock(step, tensor_idx);
            changed = true;
        }
    }
    changed
}

fn copy_live_slot_variant_locks(
    slot: &EffectSlotState,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    let num_params = (slot.num_params.load(Ordering::Relaxed) as usize).min(MAX_SLOT_PARAMS);
    for param_idx in 0..num_params {
        let source = slot.plocks.get(source_step, param_idx);
        let target = slot.plocks.get(target_step, param_idx);
        if option_f32_bits_equal(source, target) {
            continue;
        }
        match source {
            Some(value) => slot.set_plock(target_step, param_idx, value),
            None => slot.plocks.clear_param(target_step, param_idx),
        }
        changed = true;
    }

    for tensor_idx in 0..slot.tensor_params.num_params() {
        let source = slot.tensor_params.plock_values(source_step, tensor_idx);
        let target = slot.tensor_params.plock_values(target_step, tensor_idx);
        let equal = match (&source, &target) {
            (Some(source), Some(target)) => f32_slices_bits_equal(source, target),
            (None, None) => true,
            _ => false,
        };
        if equal {
            continue;
        }
        match source {
            Some(values) => {
                slot.tensor_params
                    .set_plock(target_step, tensor_idx, &values);
            }
            None => {
                slot.tensor_params.clear_plock(target_step, tensor_idx);
            }
        }
        changed = true;
    }
    changed
}

fn clear_rack_slot_variant_locks(slot: &mut RackSlotSnapshot, step: usize) -> bool {
    let mut changed = false;
    for param in RackSlotParam::ALL {
        if slot.param_plocks.get(step, param).is_some() {
            slot.param_plocks.clear(step, param);
            changed = true;
        }
    }
    changed |= clear_snapshot_slot_variant_locks(&mut slot.instrument_slot, step);
    changed
}

fn copy_rack_slot_variant_locks(
    slot: &mut RackSlotSnapshot,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    for param in RackSlotParam::ALL {
        let source = slot.param_plocks.get(source_step, param);
        let target = slot.param_plocks.get(target_step, param);
        if option_f32_bits_equal(source, target) {
            continue;
        }
        match source {
            Some(value) => {
                slot.param_plocks.set(target_step, param, value);
            }
            None => {
                slot.param_plocks.clear(target_step, param);
            }
        }
        changed = true;
    }
    changed |=
        copy_snapshot_slot_variant_locks(&mut slot.instrument_slot, source_step, target_step);
    changed
}

fn clear_snapshot_slot_variant_locks(slot: &mut EffectSlotSnapshot, step: usize) -> bool {
    let mut changed = false;
    let num_params = slot.num_params as usize;
    let params_to_clear = slot
        .plocks
        .get(step)
        .map(|row| {
            (0..num_params.min(row.len()))
                .filter(|param_idx| row[*param_idx].is_some())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for param_idx in params_to_clear {
        slot.clear_plock(step, param_idx);
        changed = true;
    }
    for tensor_idx in 0..slot.tensor_params.len() {
        if slot.tensor_plock_values(step, tensor_idx).is_some() {
            slot.clear_tensor_plock(step, tensor_idx);
            changed = true;
        }
    }
    changed
}

fn copy_snapshot_slot_variant_locks(
    slot: &mut EffectSlotSnapshot,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    let num_params = slot.num_params as usize;
    for param_idx in 0..num_params {
        let source = slot
            .plocks
            .get(source_step)
            .and_then(|row| row.get(param_idx))
            .copied()
            .flatten();
        let target = slot
            .plocks
            .get(target_step)
            .and_then(|row| row.get(param_idx))
            .copied()
            .flatten();
        if option_f32_bits_equal(source, target) {
            continue;
        }
        match source {
            Some(value) => {
                slot.set_plock(target_step, param_idx, value);
            }
            None => {
                slot.clear_plock(target_step, param_idx);
            }
        }
        changed = true;
    }

    for tensor_idx in 0..slot.tensor_params.len() {
        let source = slot
            .tensor_plock_values(source_step, tensor_idx)
            .map(|values| values.to_vec());
        let target = slot
            .tensor_plock_values(target_step, tensor_idx)
            .map(|values| values.to_vec());
        let equal = match (&source, &target) {
            (Some(source), Some(target)) => f32_slices_bits_equal(source, target),
            (None, None) => true,
            _ => false,
        };
        if equal {
            continue;
        }
        match source {
            Some(values) => {
                slot.set_tensor_plock(target_step, tensor_idx, values);
            }
            None => {
                slot.clear_tensor_plock(target_step, tensor_idx);
            }
        }
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{
        EffectDescriptor, EffectSlotSnapshot, HostControl, ParamDescriptor, ParamKind,
        ParamScaling, BUILTIN_SLOT_COUNT,
    };
    use crate::neural::ParamNodeId;
    use crate::sequencer::ModDestination;

    #[test]
    fn active_notes_merge_scheduled_expirations_with_live_note_state() {
        let state = SequencerState::new(1, vec![vec![]]);
        state.set_audio_rendered_sample(100);
        state.mark_scheduled_note_active_until(0, 60, 200);
        state.replace_live_notes(0, [64]);
        assert_eq!(state.active_notes(0), vec![60, 64]);

        state.set_audio_rendered_sample(200);
        assert_eq!(state.active_notes(0), vec![64]);

        state.mark_scheduled_note_active_until(0, 64, 300);
        state.replace_live_notes(0, []);
        assert_eq!(state.active_notes(0), vec![64]);

        state.set_audio_rendered_sample(300);
        assert!(state.active_notes(0).is_empty());
    }

    fn sample_track_params(id: usize) -> TrackParamsSnapshot {
        TrackParamsSnapshot {
            gate: id % 2 == 0,
            attack_ms: 10.0 + id as f32,
            release_ms: 20.0 + id as f32,
            swing: 0.1 * id as f32,
            swing_resolution: SwingResolution::Quarter,
            num_steps: 8 + id,
            volume: 0.2 * id as f32,
            pan: -1.0 + id as f32,
            mute: id % 2 == 0,
            solo: id == 1,
            send: 0.3 * id as f32,
            output: crate::sequencer::TrackOutput::Mix,
            sends: vec![crate::sequencer::TrackSendSnapshot {
                destination: crate::sequencer::BusId::DEFAULT_A,
                amount: 0.1 * id as f32,
            }],
            polyphonic: id % 2 == 1,
            max_polyphony: 1 + (id % 12),
            timebase: Timebase::Quarter,
            accumulator_idx: id,
            script_accumulator_name: Some(format!("acc-{id}")),
            midi_fx_chain: vec![format!("fx-{id}")],
            midi_fx_position: crate::sequencer::MidiFxPosition::PostAccumulator,
            accum_limit: (1 + id) as f32,
            accum_mode: id as u32,
            fts_scale: id + 1,
            mute_group: (id % 9) as u8,
            global_transpose: id % 2 == 0,
        }
    }

    fn sample_effect_slot_snapshot(id: usize) -> EffectSlotSnapshot {
        EffectSlotSnapshot {
            node_id: 100 + id as u32,
            modulator_node_id: 0,
            num_params: 2,
            defaults: vec![id as f32, id as f32 + 0.5],
            plocks: (0..MAX_STEPS)
                .map(|step| {
                    if step == id {
                        vec![Some(id as f32), None]
                    } else {
                        vec![None, None]
                    }
                })
                .collect(),
            plock_param_ids: vec![vec![None, None]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: vec![id as u32, id as u32 + 10],
            param_node_spans: vec![1, 1],
            transport_phase_param_idx: crate::effects::NO_TRANSPORT_PHASE_PARAM,
            tensor_params: Vec::new(),
            ir: None,
        }
    }

    #[test]
    fn track_mute_group_defaults_and_clamps() {
        let params = TrackParams::new();
        assert_eq!(params.get_mute_group(), 0);

        params.set_mute_group(4);
        assert_eq!(params.get_mute_group(), 4);

        params.set_mute_group(42);
        assert_eq!(params.get_mute_group(), 8);
    }

    #[test]
    fn key_lock_variant_stamp_copies_key_lock_signature_to_selected_notes() {
        let desc = EffectDescriptor::builtin_filter();
        let cutoff_idx = desc
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("filter descriptor should include cutoff");
        let mode_idx = desc
            .params
            .iter()
            .position(|param| param.name == "mode")
            .expect("filter descriptor should include mode");
        let state = SequencerState::new(1, vec![]);
        state.pattern.instrument_slots[0].apply_descriptor(&desc, 100);
        state.pattern.instrument_slots[0].set_key_lock(60, cutoff_idx, 900.0);
        state.pattern.instrument_slots[0].set_key_lock(60, mode_idx, 2.0);

        let assignments = state.reconcile_key_lock_variant_registry_for_track(0);
        let assignment = assignments[60]
            .clone()
            .expect("source note should be assigned to a key-lock variant");
        assert_eq!(assignment.label, "A");
        assert_eq!(assignment.param_count, 2);

        assert!(state.stamp_key_lock_variant_key_to_notes(0, &assignment.key, &[62, 64]));
        let slot = &state.pattern.instrument_slots[0];
        assert_eq!(slot.key_locks.get(62, cutoff_idx), Some(900.0));
        assert_eq!(slot.key_locks.get(62, mode_idx), Some(2.0));
        assert_eq!(slot.key_locks.get(64, cutoff_idx), Some(900.0));
        assert_eq!(slot.key_locks.get(64, mode_idx), Some(2.0));

        let assignments = state.reconcile_key_lock_variant_registry_for_track(0);
        assert_eq!(
            assignments[60].as_ref().map(|item| item.label.as_str()),
            Some("A")
        );
        assert_eq!(
            assignments[62].as_ref().map(|item| item.label.as_str()),
            Some("A")
        );
        assert_eq!(
            assignments[64].as_ref().map(|item| item.label.as_str()),
            Some("A")
        );

        assert!(state.clear_key_lock_variant_locks_for_notes(0, &[62]));
        assert_eq!(slot.key_locks.get(62, cutoff_idx), None);
        assert_eq!(slot.key_locks.get(62, mode_idx), None);
        assert_eq!(slot.key_locks.get(64, cutoff_idx), Some(900.0));
    }

    fn sample_pattern_snapshot(num_tracks: usize) -> PatternSnapshot {
        PatternSnapshot {
            track_bits: (0..num_tracks)
                .map(|track| {
                    let mut bits = [0u64; TRACK_PATTERN_WORDS];
                    bits[0] = (track as u64) + 1;
                    bits
                })
                .collect(),
            neural_reset_bits: vec![[0u64; TRACK_PATTERN_WORDS]; num_tracks],
            step_data: (0..num_tracks)
                .map(|track| {
                    let mut steps = vec![[0.0; NUM_PARAMS]; MAX_STEPS];
                    steps[0][0] = track as f32 + 0.25;
                    steps[1][1] = track as f32 + 0.5;
                    steps
                })
                .collect(),
            track_params: (0..num_tracks).map(sample_track_params).collect(),
            effect_slots: (0..num_tracks)
                .map(|track| vec![sample_effect_slot_snapshot(track)])
                .collect(),
            midi_fx_slots: (0..num_tracks)
                .map(|_| vec![EffectSlotSnapshot::new_empty(); crate::lisp_host::MAX_MIDI_FX_SLOTS])
                .collect(),
            instrument_slots: (0..num_tracks)
                .map(|track| sample_effect_slot_snapshot(track + 10))
                .collect(),
            instrument_base_note_offsets: (0..num_tracks)
                .map(|track| track as f32 - 12.0)
                .collect(),
            instrument_run_modes: (0..num_tracks)
                .map(|track| {
                    if track % 2 == 0 {
                        CustomInstrumentRunMode::Instrument
                    } else {
                        CustomInstrumentRunMode::FreePatch
                    }
                })
                .collect(),
            track_sound_states: (0..num_tracks)
                .map(|track| TrackSoundState {
                    loaded_preset: Some(format!("preset-{track}")),
                    dirty: track % 2 == 0,
                    engine_id: Some(track),
                })
                .collect(),
            sample_ids: (0..num_tracks)
                .map(|track| (track as i32, format!("track-{track}"), 44_100))
                .collect(),
            chord_snapshots: (0..num_tracks)
                .map(|track| {
                    let mut chord = ChordSnapshot::new_default();
                    chord.steps[0] = vec![track as f32, track as f32 + 7.0];
                    chord
                })
                .collect(),
            timebase_plock_snapshots: (0..num_tracks)
                .map(|track| {
                    let mut arr = [None; MAX_STEPS];
                    arr[0] = Some(track as u32);
                    arr
                })
                .collect(),
            swing_plock_snapshots: (0..num_tracks)
                .map(|track| {
                    let mut arr = [None; MAX_STEPS];
                    arr[1] = Some((track as u32) + 10);
                    arr
                })
                .collect(),
            swing_resolution_plock_snapshots: (0..num_tracks)
                .map(|track| {
                    let mut arr = [None; MAX_STEPS];
                    arr[2] = Some((track as u32) + 20);
                    arr
                })
                .collect(),
            instrument_types: (0..num_tracks)
                .map(|track| {
                    if track % 2 == 0 {
                        InstrumentType::Sampler
                    } else {
                        InstrumentType::Custom
                    }
                })
                .collect(),
            mod_connections: Vec::new(),
            neural_networks: Vec::new(),
            graph_overrides: Vec::new(),
            rack_tracks: vec![None; num_tracks],
            process_chains: vec![crate::process::TrackProcessChain::default(); num_tracks],
            project_process_chain: crate::process::TrackProcessChain::default(),
            plock_variant_registries: vec![PlockVariantRegistry::default(); num_tracks],
            key_lock_variant_registries: vec![PlockVariantRegistry::default(); num_tracks],
        }
    }

    fn sample_rack_track_snapshot() -> RackTrackSnapshot {
        RackTrackSnapshot {
            routing: RackRouting::Broadcast,
            slots: vec![RackSlotSnapshot {
                instrument_type: InstrumentType::Custom,
                instrument_run_mode: CustomInstrumentRunMode::Instrument,
                instrument_base_note_offset: 7.0,
                pad_note: None,
                choke_group: None,
                gain: 0.8,
                pan: -0.2,
                mute: false,
                solo: true,
                max_polyphony: 3,
                param_plocks: RackSlotParamPlocks::new(),
                instrument_slot: sample_effect_slot_snapshot(77),
                track_sound_state: TrackSoundState {
                    loaded_preset: Some("rack-lead".to_string()),
                    dirty: true,
                    engine_id: Some(12),
                },
                sample_id: None,
            }],
        }
    }

    fn sample_sampler_rack_slot(
        buffer_id: i32,
        sample_name: &str,
        sample_rate: u32,
        instrument_slot_id: usize,
        pad_note: Option<i32>,
    ) -> RackSlotSnapshot {
        RackSlotSnapshot {
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: -12.0,
            pad_note,
            choke_group: None,
            gain: 0.5,
            pan: 0.25,
            mute: false,
            solo: false,
            max_polyphony: 2,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: sample_effect_slot_snapshot(instrument_slot_id),
            track_sound_state: TrackSoundState::default(),
            sample_id: Some((buffer_id, sample_name.to_string(), sample_rate)),
        }
    }

    fn sample_sidechain_descriptor() -> EffectDescriptor {
        EffectDescriptor {
            name: "duck".to_string(),
            params: vec![ParamDescriptor {
                name: "sidechain".to_string(),
                min: 0.0,
                max: 3.0,
                default: 0.0,
                kind: ParamKind::Enum {
                    labels: vec![
                        "off".to_string(),
                        "track-a".to_string(),
                        "track-b".to_string(),
                        "track-c".to_string(),
                    ],
                },
                scaling: ParamScaling::Linear,
                node_param_idx: u32::MAX,
                node_param_span: 1,
                host_control: Some(HostControl::FxSidechain { input_channel: 0 }),
                ui_metadata: None,
            }],
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
        }
    }

    #[test]
    fn pattern_repository_ownership_stays_inside_state_module() {
        fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("read source dir") {
                let entry = entry.expect("read source entry");
                let path = entry.path();
                if path.is_dir() {
                    collect_rs_files(&path, out);
                } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }

        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source_dir = manifest_dir.join("src");
        let state_file = source_dir.join("sequencer").join("state.rs");
        let mut files = Vec::new();
        collect_rs_files(&source_dir, &mut files);

        let forbidden = [
            ".pattern.pattern_bank",
            ".pattern.current_pattern",
            ".pattern.num_patterns",
            ".bus_pattern_bank",
            ".edit_pattern_repository",
            ".edit_non_current_pattern_snapshots",
            ".edit_all_pattern_snapshots",
        ];
        let mut violations = Vec::new();
        for file in files {
            if file == state_file {
                continue;
            }
            let source = std::fs::read_to_string(&file).expect("read Rust source");
            let normalized: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
            for pattern in forbidden {
                if normalized.contains(pattern) {
                    violations.push(format!(
                        "{} contains direct {} access",
                        file.strip_prefix(&manifest_dir).unwrap_or(&file).display(),
                        pattern
                    ));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "pattern repository access must go through SequencerState facade:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn repository_effect_slot_insert_applies_to_other_patterns_for_one_track_only() {
        let state = SequencerState::new(2, (0..2).map(|_| default_empty_effect_chain()).collect());
        let descriptor_lane =
            vec![EffectDescriptor::empty_custom_slot(); crate::lisp_host::MAX_CUSTOM_FX];
        let slot_descriptors = vec![descriptor_lane.clone(), descriptor_lane];
        let mut current = PatternSnapshot::new_default(2, &slot_descriptors);
        let mut other = PatternSnapshot::new_default(2, &slot_descriptors);
        current.effect_slots[0][BUILTIN_SLOT_COUNT].node_id = 7;
        other.effect_slots[0][BUILTIN_SLOT_COUNT].node_id = 42;
        other.effect_slots[0][BUILTIN_SLOT_COUNT + 1].node_id = 43;
        other.effect_slots[1][BUILTIN_SLOT_COUNT].node_id = 99;
        state.replace_pattern_repository(vec![current, other], 0);

        state.insert_effect_slot_in_other_track_patterns(0, BUILTIN_SLOT_COUNT);

        let after = state.export_pattern_repository();
        assert_eq!(after[0].effect_slots[0][BUILTIN_SLOT_COUNT].node_id, 7);
        assert_eq!(after[1].effect_slots[1][BUILTIN_SLOT_COUNT].node_id, 99);
        assert_eq!(after[1].effect_slots[0][BUILTIN_SLOT_COUNT].node_id, 0);
        assert_eq!(after[1].effect_slots[0][BUILTIN_SLOT_COUNT].num_params, 0);
        assert_eq!(after[1].effect_slots[0][BUILTIN_SLOT_COUNT + 1].node_id, 42);
    }

    #[test]
    fn topology_edit_preserves_shared_track_pattern_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        let shared = state.scene_track_pattern_id(0, 0).unwrap();
        {
            let mut scenes = state.pattern.scenes.lock().unwrap();
            assert!(scenes.set_cell(1, 0, shared));
        }

        state.insert_effect_slot_in_other_track_patterns(0, BUILTIN_SLOT_COUNT);

        let scenes = state.pattern.scenes.lock().unwrap();
        assert_eq!(scenes.scenes[0].cells[0], Some(shared));
        assert_eq!(scenes.scenes[1].cells[0], Some(shared));
    }

    #[test]
    fn repository_midi_fx_insert_applies_to_other_patterns_for_one_track_only() {
        let state = SequencerState::new(2, (0..2).map(|_| default_empty_effect_chain()).collect());
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(2, &[]),
                PatternSnapshot::new_default(2, &[]),
            ],
            0,
        );
        let before = state.export_pattern_repository();
        let mut descriptor = EffectDescriptor::empty_custom_slot();
        descriptor.name = "arp".to_string();

        state.insert_midi_fx_slot_in_other_track_patterns(0, 0, "arp".to_string(), &descriptor);

        let after = state.export_pattern_repository();
        assert_eq!(
            after[0].track_params[0].midi_fx_chain,
            before[0].track_params[0].midi_fx_chain
        );
        assert_eq!(
            after[1].track_params[1].midi_fx_chain,
            before[1].track_params[1].midi_fx_chain
        );
        assert_eq!(
            after[1].track_params[0].midi_fx_chain,
            vec!["arp".to_string()]
        );
    }

    fn sample_bus_pattern_snapshot(marker: f32) -> Vec<BusPatternSnapshot> {
        let mut gate_sequence = BusGateSequence::default();
        gate_sequence.velocities[0] = marker;
        vec![BusPatternSnapshot {
            id: BusId::DEFAULT_A,
            gate_sequence,
            effect_plocks: vec![
                vec![vec![Some(marker)]],
                vec![vec![Some(marker + 1.0)]],
                vec![vec![Some(marker + 2.0)]],
            ],
            effect_defaults: vec![vec![marker]],
        }]
    }

    #[test]
    fn bus_pattern_repository_clone_and_delete_are_state_owned() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        let first = sample_bus_pattern_snapshot(0.25);
        let second = sample_bus_pattern_snapshot(0.75);
        state.replace_bus_pattern_repository(vec![first.clone(), second], &first);

        let new_scene = state.clone_pattern(
            1,
            &[-1],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Sampler],
        );
        let cloned = state.clone_bus_pattern_snapshot(0, new_scene, &first);
        assert_eq!(cloned[0].gate_sequence.velocities[0], 0.25);
        assert_eq!(
            state.bus_pattern_snapshot_or_default(new_scene, &first)[0]
                .gate_sequence
                .velocities[0],
            0.25
        );

        state
            .switch_pattern(
                0,
                1,
                &[-1],
                &[44_100],
                &[String::from("track")],
                &[InstrumentType::Sampler],
            )
            .unwrap();
        state
            .delete_pattern(
                1,
                &[-1],
                &[44_100],
                &[String::from("track")],
                &[InstrumentType::Sampler],
            )
            .unwrap();
        let restored = state.delete_bus_pattern_snapshot(0, 0, &first);
        assert_eq!(restored[0].gate_sequence.velocities[0], 0.75);
    }

    #[test]
    fn bus_effect_slot_topology_updates_other_scene_bus_patterns_only() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        let current = sample_bus_pattern_snapshot(0.25);
        let other = sample_bus_pattern_snapshot(0.75);
        state.replace_bus_pattern_repository(vec![current.clone(), other], &current);

        state.insert_bus_effect_slot_in_other_scene_patterns(0, 1, &current);

        let current_after = state.bus_pattern_snapshot_or_default(0, &current);
        let other_after = state.bus_pattern_snapshot_or_default(1, &current);
        assert_eq!(
            current_after[0].effect_plocks[1][0][0],
            Some(1.25),
            "current scene bus plocks should not be touched"
        );
        assert!(
            other_after[0].effect_plocks[1].is_empty(),
            "other scene should receive an empty inserted bus effect slot"
        );
        assert_eq!(other_after[0].effect_plocks[2][0][0], Some(1.75));
    }

    #[test]
    fn track_pattern_data_extracts_one_complete_lane() {
        let snapshot = sample_pattern_snapshot(3);

        let data = snapshot.track_pattern_data(2).unwrap();

        assert_eq!(data.track_bits[0], 3);
        assert_eq!(data.neural_reset_bits, [0u64; TRACK_PATTERN_WORDS]);
        assert_eq!(data.step_data[0][0], 2.25);
        assert_eq!(data.track_params.num_steps, 10);
        assert_eq!(data.effect_slots[0].node_id, 102);
        assert_eq!(data.instrument_slot.node_id, 112);
        assert_eq!(data.instrument_base_note_offset, -10.0);
        assert_eq!(
            data.track_sound_state.loaded_preset.as_deref(),
            Some("preset-2")
        );
        assert_eq!(data.sample_id, (2, "track-2".to_string(), 44_100));
        assert_eq!(data.chord_snapshot.steps[0], vec![2.0, 9.0]);
        assert_eq!(data.timebase_plock_snapshot[0], Some(2));
        assert_eq!(data.swing_plock_snapshot[1], Some(12));
        assert_eq!(data.swing_resolution_plock_snapshot[2], Some(22));
        assert_eq!(data.instrument_type, InstrumentType::Sampler);
        assert_eq!(
            data.instrument_run_mode,
            CustomInstrumentRunMode::Instrument
        );
    }

    #[test]
    fn track_pattern_data_round_trips_rack_track_lane() {
        let mut source = sample_pattern_snapshot(2);
        source.rack_tracks[1] = Some(sample_rack_track_snapshot());
        let data = source.track_pattern_data(1).unwrap();
        let rack = data.rack_track.as_ref().unwrap();
        assert_eq!(rack.routing, RackRouting::Broadcast);
        assert_eq!(rack.slots.len(), 1);
        assert_eq!(rack.slots[0].instrument_base_note_offset, 7.0);
        assert_eq!(rack.slots[0].gain, 0.8);
        assert!(rack.slots[0].solo);

        let mut target = PatternSnapshot::new_default(1, &[]);
        target.set_track_pattern_data(0, data);

        let restored = target.rack_tracks[0].as_ref().unwrap();
        assert_eq!(restored.routing, RackRouting::Broadcast);
        assert_eq!(restored.slots[0].instrument_type, InstrumentType::Custom);
        assert_eq!(
            restored.slots[0].track_sound_state.loaded_preset.as_deref(),
            Some("rack-lead")
        );
        assert_eq!(restored.slots[0].instrument_slot.node_id, 177);
    }

    #[test]
    fn append_rack_slot_preserves_existing_rack_slots_in_pattern_pool() {
        let state = SequencerState::new(1, Vec::new());
        let initial = sample_rack_track_snapshot();
        state.set_rack_track_for_all_pattern_snapshots(0, initial.clone());
        let appended = sample_sampler_rack_slot(123, "layer", 48_000, 88, None);

        state.append_rack_slot_for_all_pattern_snapshots(0, RackRouting::Broadcast, appended);

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots.len(), 2);
        assert_eq!(
            live.slots[0].track_sound_state.loaded_preset,
            Some("rack-lead".to_string())
        );
        assert_eq!(live.slots[1].sample_id.as_ref().unwrap().1, "layer");

        let repository = state.export_pattern_repository();
        let restored = repository[0].rack_tracks[0].as_ref().unwrap();
        assert_eq!(restored.slots.len(), 2);
        assert_eq!(
            restored.slots[0].track_sound_state.loaded_preset.as_deref(),
            Some("rack-lead")
        );
        assert_eq!(restored.slots[1].instrument_base_note_offset, -12.0);
        assert_eq!(restored.slots[1].max_polyphony, 2);
    }

    #[test]
    fn append_rack_slot_to_current_pattern_does_not_mutate_other_patterns() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();
        state.set_rack_track_for_all_pattern_snapshots(0, sample_rack_track_snapshot());

        let appended = sample_sampler_rack_slot(123, "current-layer", 48_000, 88, None);

        assert!(state.append_rack_slot_to_current_pattern(0, RackRouting::Broadcast, appended));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots.len(), 2);
        assert_eq!(live.slots[1].sample_id.as_ref().unwrap().1, "current-layer");

        let repository = state.export_pattern_repository();
        let current = repository[0].rack_tracks[0].as_ref().unwrap();
        let other = repository[1].rack_tracks[0].as_ref().unwrap();
        assert_eq!(current.slots.len(), 2);
        assert_eq!(
            current.slots[1].sample_id.as_ref().unwrap().1,
            "current-layer"
        );
        assert_eq!(other.slots.len(), 1);
        assert_eq!(
            other.slots[0].track_sound_state.loaded_preset.as_deref(),
            Some("rack-lead")
        );
        assert!(other.slots[0].sample_id.is_none());
    }

    #[test]
    fn replace_rack_slot_source_preserves_pad_controls_and_slot_plocks() {
        let state = SequencerState::new(1, Vec::new());
        let mut initial = sample_rack_track_snapshot();
        initial.routing = RackRouting::ByPitch;
        initial.slots[0].pad_note = Some(2);
        initial.slots[0].choke_group = Some(3);
        initial.slots[0].instrument_base_note_offset = 5.0;
        initial.slots[0].gain = 0.7;
        initial.slots[0].pan = -0.4;
        initial.slots[0].mute = true;
        initial.slots[0].solo = false;
        initial.slots[0].max_polyphony = 6;
        assert!(initial.slots[0]
            .param_plocks
            .set(4, RackSlotParam::Gain, 0.25));
        state.set_rack_track_for_all_pattern_snapshots(0, initial);

        let replacement = RackSlotSnapshot {
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: -12.0,
            pad_note: Some(9),
            choke_group: Some(8),
            gain: 0.2,
            pan: 0.5,
            mute: false,
            solo: true,
            max_polyphony: 1,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: sample_effect_slot_snapshot(88),
            track_sound_state: TrackSoundState::default(),
            sample_id: Some((123, "replacement".to_string(), 48_000)),
        };

        assert!(state.replace_rack_slot_source_for_all_pattern_snapshots(0, 0, replacement));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        let live_slot = &live.slots[0];
        assert_eq!(live_slot.instrument_type, InstrumentType::Sampler);
        assert_eq!(live_slot.sample_id.as_ref().unwrap().1, "replacement");
        assert_eq!(live_slot.instrument_slot.node_id, 188);
        assert_eq!(live_slot.pad_note, Some(2));
        assert_eq!(live_slot.choke_group, Some(3));
        assert_eq!(live_slot.instrument_base_note_offset, 5.0);
        assert_eq!(live_slot.gain, 0.7);
        assert_eq!(live_slot.pan, -0.4);
        assert!(live_slot.mute);
        assert!(!live_slot.solo);
        assert_eq!(live_slot.max_polyphony, 6);
        assert_eq!(
            live_slot.param_plocks.get(4, RackSlotParam::Gain),
            Some(0.25)
        );

        let repository = state.export_pattern_repository();
        let restored = &repository[0].rack_tracks[0].as_ref().unwrap().slots[0];
        assert_eq!(restored.sample_id.as_ref().unwrap().1, "replacement");
        assert_eq!(restored.pad_note, Some(2));
        assert_eq!(restored.choke_group, Some(3));
        assert_eq!(restored.instrument_base_note_offset, 5.0);
        assert_eq!(
            restored.param_plocks.get(4, RackSlotParam::Gain),
            Some(0.25)
        );
    }

    #[test]
    fn replace_rack_slot_source_in_current_pattern_keeps_other_patterns_unchanged() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();

        let mut initial = sample_rack_track_snapshot();
        initial.routing = RackRouting::ByPitch;
        initial.slots[0].pad_note = Some(0);
        initial.slots[0].choke_group = Some(4);
        initial.slots[0].instrument_base_note_offset = 5.0;
        initial.slots[0].gain = 0.7;
        initial.slots[0].pan = -0.4;
        initial.slots[0].mute = true;
        initial.slots[0].solo = false;
        initial.slots[0].max_polyphony = 6;
        assert!(initial.slots[0]
            .param_plocks
            .set(4, RackSlotParam::Gain, 0.25));
        state.set_rack_track_for_all_pattern_snapshots(0, initial);

        let mut replacement = sample_sampler_rack_slot(321, "replacement", 48_000, 88, Some(9));
        replacement.choke_group = Some(8);
        replacement.instrument_base_note_offset = -24.0;
        replacement.gain = 0.2;
        replacement.pan = 0.5;
        replacement.mute = false;
        replacement.solo = true;
        replacement.max_polyphony = 1;

        assert!(state.replace_rack_slot_source_in_current_pattern(0, 0, replacement));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "replacement");
        assert_eq!(live.slots[0].pad_note, Some(0));
        assert_eq!(live.slots[0].choke_group, Some(4));
        assert_eq!(live.slots[0].instrument_base_note_offset, 5.0);
        assert_eq!(live.slots[0].gain, 0.7);
        assert_eq!(live.slots[0].pan, -0.4);
        assert!(live.slots[0].mute);
        assert!(!live.slots[0].solo);
        assert_eq!(live.slots[0].max_polyphony, 6);
        assert_eq!(
            live.slots[0].param_plocks.get(4, RackSlotParam::Gain),
            Some(0.25)
        );

        let repository = state.export_pattern_repository();
        let current = &repository[0].rack_tracks[0].as_ref().unwrap().slots[0];
        let other = &repository[1].rack_tracks[0].as_ref().unwrap().slots[0];
        assert_eq!(current.sample_id.as_ref().unwrap().1, "replacement");
        assert_eq!(current.pad_note, Some(0));
        assert_eq!(current.choke_group, Some(4));
        assert_eq!(current.param_plocks.get(4, RackSlotParam::Gain), Some(0.25));
        assert!(other.sample_id.is_none());
        assert_eq!(
            other.track_sound_state.loaded_preset.as_deref(),
            Some("rack-lead")
        );
        assert_eq!(other.pad_note, Some(0));
        assert_eq!(other.choke_group, Some(4));
        assert_eq!(other.param_plocks.get(4, RackSlotParam::Gain), Some(0.25));
    }

    #[test]
    fn sync_rack_slot_bindings_for_current_pattern_does_not_rebind_other_patterns() {
        let state = make_state_with_tracks(1);
        let mut current = PatternSnapshot::new_default(1, &[]);
        let mut other = PatternSnapshot::new_default(1, &[]);
        let mut current_rack = sample_rack_track_snapshot();
        let mut other_rack = sample_rack_track_snapshot();
        current_rack.slots[0].instrument_slot = sample_effect_slot_snapshot(11);
        other_rack.slots[0].instrument_slot = sample_effect_slot_snapshot(22);
        current.instrument_types[0] = InstrumentType::Rack;
        other.instrument_types[0] = InstrumentType::Rack;
        current.rack_tracks[0] = Some(current_rack);
        other.rack_tracks[0] = Some(other_rack);
        state.replace_pattern_repository(vec![current, other], 0);
        state.restore_current_pattern_from_repository().unwrap();

        let descriptor = EffectDescriptor::builtin_sampler();
        assert!(state
            .sync_rack_slot_instrument_bindings_for_current_pattern(0, &[(descriptor, 999, 1000)]));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots[0].instrument_slot.node_id, 999);
        assert_eq!(live.slots[0].instrument_slot.modulator_node_id, 1000);

        let repository = state.export_pattern_repository();
        let current_slot = &repository[0].rack_tracks[0].as_ref().unwrap().slots[0];
        let other_slot = &repository[1].rack_tracks[0].as_ref().unwrap().slots[0];
        assert_eq!(current_slot.instrument_slot.node_id, 999);
        assert_eq!(current_slot.instrument_slot.modulator_node_id, 1000);
        assert_eq!(other_slot.instrument_slot.node_id, 122);
        assert_eq!(other_slot.instrument_slot.modulator_node_id, 0);
    }

    #[test]
    fn launch_scene_restores_pattern_locked_rack_sources() {
        let state = make_state_with_tracks(1);
        let mut first = PatternSnapshot::new_default(1, &[]);
        let mut second = PatternSnapshot::new_default(1, &[]);
        first.instrument_types[0] = InstrumentType::Rack;
        second.instrument_types[0] = InstrumentType::Rack;
        first.rack_tracks[0] = Some(RackTrackSnapshot {
            routing: RackRouting::Broadcast,
            slots: vec![sample_sampler_rack_slot(
                101,
                "pattern-one",
                44_100,
                11,
                None,
            )],
        });
        second.rack_tracks[0] = Some(RackTrackSnapshot {
            routing: RackRouting::Broadcast,
            slots: vec![sample_sampler_rack_slot(
                202,
                "pattern-two",
                44_100,
                22,
                None,
            )],
        });
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "pattern-one");

        state
            .launch_scene(
                1,
                1,
                &[-1],
                &[44_100],
                &[String::from("rack")],
                &[InstrumentType::Rack],
            )
            .unwrap();
        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "pattern-two");

        state
            .launch_scene(
                0,
                1,
                &[-1],
                &[44_100],
                &[String::from("rack")],
                &[InstrumentType::Rack],
            )
            .unwrap();
        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "pattern-one");
    }

    #[test]
    fn remove_rack_slot_from_current_pattern_does_not_mutate_other_patterns() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();
        state.set_rack_track_for_all_pattern_snapshots(0, sample_rack_track_snapshot());
        state.append_rack_slot_for_all_pattern_snapshots(
            0,
            RackRouting::Broadcast,
            sample_sampler_rack_slot(123, "layer", 48_000, 88, None),
        );

        assert!(state.remove_rack_slot_from_current_pattern(0, 0));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots.len(), 1);
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "layer");

        let repository = state.export_pattern_repository();
        let current = repository[0].rack_tracks[0].as_ref().unwrap();
        let other = repository[1].rack_tracks[0].as_ref().unwrap();
        assert_eq!(current.slots.len(), 1);
        assert_eq!(current.slots[0].sample_id.as_ref().unwrap().1, "layer");
        assert_eq!(other.slots.len(), 2);
        assert_eq!(
            other.slots[0].track_sound_state.loaded_preset.as_deref(),
            Some("rack-lead")
        );
        assert_eq!(other.slots[1].sample_id.as_ref().unwrap().1, "layer");
    }

    #[test]
    fn remove_rack_slot_updates_live_and_pattern_pool_slots() {
        let state = SequencerState::new(1, Vec::new());
        let initial = sample_rack_track_snapshot();
        state.set_rack_track_for_all_pattern_snapshots(0, initial);
        let appended = sample_sampler_rack_slot(123, "layer", 48_000, 88, None);
        state.append_rack_slot_for_all_pattern_snapshots(0, RackRouting::Broadcast, appended);

        assert!(state.remove_rack_slot_from_all_pattern_snapshots(0, 0));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots.len(), 1);
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "layer");

        let repository = state.export_pattern_repository();
        let restored = repository[0].rack_tracks[0].as_ref().unwrap();
        assert_eq!(restored.slots.len(), 1);
        assert_eq!(restored.slots[0].sample_id.as_ref().unwrap().1, "layer");
        assert_eq!(restored.slots[0].instrument_base_note_offset, -12.0);
    }

    #[test]
    fn normalize_track_count_extends_missing_rack_lane_for_legacy_snapshots() {
        let mut snapshot = sample_pattern_snapshot(2);
        snapshot.rack_tracks.clear();

        snapshot.normalize_track_count(2, &[]);

        assert_eq!(snapshot.rack_tracks.len(), 2);
        assert!(snapshot.rack_tracks.iter().all(Option::is_none));
        assert!(snapshot.track_lane_count_is_consistent());
    }

    #[test]
    fn set_track_pattern_data_round_trips_one_lane() {
        let source = sample_pattern_snapshot(3);
        let data = source.track_pattern_data(2).unwrap();
        let mut target = PatternSnapshot::new_default(1, &[]);

        target.set_track_pattern_data(0, data);

        assert_eq!(target.track_bits[0][0], 3);
        assert_eq!(target.step_data[0][0][0], 2.25);
        assert_eq!(target.track_params[0].num_steps, 10);
        assert_eq!(target.effect_slots[0][0].node_id, 102);
        assert_eq!(target.instrument_slots[0].node_id, 112);
        assert_eq!(target.sample_ids[0], (2, "track-2".to_string(), 44_100));
        assert_eq!(target.chord_snapshots[0].steps[0], vec![2.0, 9.0]);
        assert_eq!(target.timebase_plock_snapshots[0][0], Some(2));
        assert_eq!(target.instrument_types[0], InstrumentType::Sampler);
        assert_eq!(
            target.instrument_run_modes[0],
            CustomInstrumentRunMode::Instrument
        );
    }

    #[test]
    fn project_scenes_identity_mapping_splits_patterns_into_track_pools() {
        let first = sample_pattern_snapshot(2);
        let mut second = sample_pattern_snapshot(2);
        second.track_bits[0][0] = 99;
        second.track_bits[1][0] = 199;
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 2,
        };
        second.mod_connections.push(route);

        let scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 1);

        assert_eq!(scenes.current_scene, 1);
        assert_eq!(scenes.track_pools.len(), 2);
        assert_eq!(scenes.track_pools[0].patterns.len(), 2);
        assert_eq!(scenes.track_pools[1].patterns.len(), 2);
        assert_eq!(scenes.scenes.len(), 2);
        assert_eq!(scenes.track_overrides, vec![None, None]);

        let first_track_zero = scenes.scenes[0].cells[0].unwrap();
        let second_track_zero = scenes.scenes[1].cells[0].unwrap();
        assert_ne!(first_track_zero, second_track_zero);
        assert_eq!(
            scenes.track_pools[0]
                .get(first_track_zero)
                .unwrap()
                .track_bits[0],
            1
        );
        assert_eq!(
            scenes.track_pools[0]
                .get(second_track_zero)
                .unwrap()
                .track_bits[0],
            99
        );

        let second_track_one = scenes.scenes[1].cells[1].unwrap();
        assert_eq!(
            scenes.track_pools[1]
                .get(second_track_one)
                .unwrap()
                .track_bits[0],
            199
        );
        assert_eq!(scenes.scenes[1].mod_connections, vec![route]);
        assert!(scenes.scenes[0].mod_connections.is_empty());
    }

    #[test]
    fn project_scenes_effective_pattern_prefers_track_override() {
        let first = sample_pattern_snapshot(2);
        let mut second = sample_pattern_snapshot(2);
        second.track_bits[1][0] = 42;
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);

        let scene_pattern = scenes.scenes[0].cells[1].unwrap();
        let override_pattern = scenes.scenes[1].cells[1].unwrap();
        assert_eq!(scenes.effective_pattern_id(1), Some(scene_pattern));

        scenes.track_overrides[1] = Some(override_pattern);

        assert_eq!(scenes.effective_pattern_id(1), Some(override_pattern));
    }

    #[test]
    fn project_scenes_new_scene_forks_current_effective_pattern_per_track() {
        let first = sample_pattern_snapshot(2);
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 3,
        };
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first], 0);
        scenes.scenes[0].mod_connections.push(route);
        let track_zero_original = scenes.scenes[0].cells[0].unwrap();
        let track_one_original = scenes.scenes[0].cells[1].unwrap();

        let track_one_override = scenes.fork_track_pattern(1).unwrap();
        scenes.track_pools[1]
            .get_mut(track_one_override)
            .unwrap()
            .track_bits[0] = 77;

        let new_scene = scenes.new_scene();

        assert_eq!(new_scene, 1);
        assert_eq!(scenes.current_scene, 1);
        assert_eq!(scenes.track_overrides, vec![None, None]);
        assert_eq!(scenes.track_pools[0].patterns.len(), 2);
        assert_eq!(scenes.track_pools[1].patterns.len(), 3);
        assert_eq!(scenes.scenes[1].mod_connections, vec![route]);

        let track_zero_new = scenes.scenes[1].cells[0].unwrap();
        let track_one_new = scenes.scenes[1].cells[1].unwrap();
        assert_ne!(track_zero_original, track_zero_new);
        assert_ne!(track_one_original, track_one_new);
        assert_ne!(track_one_override, track_one_new);
        assert_eq!(
            scenes.track_pools[0]
                .get(track_zero_new)
                .unwrap()
                .track_bits[0],
            scenes.track_pools[0]
                .get(track_zero_original)
                .unwrap()
                .track_bits[0]
        );
        assert_eq!(
            scenes.track_pools[1].get(track_one_new).unwrap().track_bits[0],
            77
        );
    }

    #[test]
    fn project_scenes_set_cell_shares_pool_entry_and_fork_diverges() {
        let first = sample_pattern_snapshot(1);
        let mut second = sample_pattern_snapshot(1);
        second.track_bits[0][0] = 42;
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 1);

        let shared = scenes.scenes[0].cells[0].unwrap();
        assert!(scenes.set_cell(1, 0, shared));
        scenes.track_pools[0].get_mut(shared).unwrap().track_bits[0] = 123;

        assert_eq!(
            scenes.effective_track_pattern(0).unwrap().track_bits[0],
            123
        );

        let forked = scenes.fork_track_pattern(0).unwrap();
        scenes.track_pools[0].get_mut(forked).unwrap().track_bits[0] = 999;

        assert_eq!(
            scenes.track_pools[0].get(shared).unwrap().track_bits[0],
            123
        );
        assert_eq!(
            scenes.track_pools[0].get(forked).unwrap().track_bits[0],
            999
        );
        assert_eq!(scenes.effective_pattern_id(0), Some(forked));
    }

    #[test]
    fn project_scenes_clear_cell_keeps_orphan_re_shareable() {
        let first = sample_pattern_snapshot(1);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first], 0);
        let id = scenes.scenes[0].cells[0].unwrap();
        scenes.track_overrides[0] = Some(id);

        assert_eq!(scenes.clear_cell(0, 0), Some(id));

        assert_eq!(scenes.scenes[0].cells[0], None);
        assert_eq!(scenes.track_overrides[0], None);
        assert!(scenes.track_pools[0].contains(id));
        assert!(scenes.set_cell(0, 0, id));
        assert_eq!(scenes.scenes[0].cells[0], Some(id));
    }

    #[test]
    fn project_scenes_launch_scene_clears_overrides_and_preserves_empty_cells() {
        let first = sample_pattern_snapshot(2);
        let second = sample_pattern_snapshot(2);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let override_id = scenes.scenes[1].cells[0].unwrap();
        scenes.track_overrides[0] = Some(override_id);
        scenes.clear_cell(1, 1);

        let launched = scenes.launch_scene(1).unwrap();

        assert_eq!(scenes.current_scene, 1);
        assert_eq!(scenes.track_overrides, vec![None, None]);
        assert_eq!(launched.len(), 2);
        assert_eq!(launched[0].as_ref().unwrap().track_bits[0], 1);
        assert!(launched[1].is_none());
    }

    #[test]
    fn project_scenes_launch_track_pattern_sets_override_and_returns_restore_data() {
        let first = sample_pattern_snapshot(1);
        let mut second = sample_pattern_snapshot(1);
        second.track_bits[0][0] = 88;
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let id = scenes.scenes[1].cells[0].unwrap();

        let data = scenes.launch_track_pattern(0, id).unwrap();

        assert_eq!(data.track_bits[0], 88);
        assert_eq!(scenes.track_overrides[0], Some(id));
        assert_eq!(scenes.effective_pattern_id(0), Some(id));
    }

    #[test]
    fn project_scenes_save_effective_track_pattern_makes_edits_durable_across_launches() {
        let first = sample_pattern_snapshot(1);
        let second = sample_pattern_snapshot(1);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let original_id = scenes.scenes[0].cells[0].unwrap();
        let other_id = scenes.scenes[1].cells[0].unwrap();
        let mut edited = scenes.effective_track_pattern(0).unwrap().clone();
        edited.track_bits[0] = 321;

        assert!(scenes.save_effective_track_pattern(0, edited));
        scenes.launch_track_pattern(0, other_id).unwrap();
        assert_eq!(
            scenes.effective_track_pattern(0).unwrap().track_bits[0],
            scenes.track_pools[0].get(other_id).unwrap().track_bits[0]
        );
        scenes.launch_scene(0).unwrap();

        assert_eq!(scenes.effective_pattern_id(0), Some(original_id));
        assert_eq!(
            scenes.effective_track_pattern(0).unwrap().track_bits[0],
            321
        );
    }

    #[test]
    fn project_scenes_remove_track_drops_pool_scene_column_and_override() {
        let first = sample_pattern_snapshot(3);
        let mut second = sample_pattern_snapshot(3);
        second.track_bits[2][0] = 44;
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let track_two_id = scenes.scenes[1].cells[2].unwrap();
        scenes.track_overrides[2] = Some(track_two_id);

        assert!(scenes.remove_track(1));

        assert_eq!(scenes.track_pools.len(), 2);
        assert_eq!(scenes.track_overrides.len(), 2);
        assert_eq!(scenes.scenes[0].cells.len(), 2);
        assert_eq!(scenes.scenes[1].cells.len(), 2);
        assert_eq!(scenes.track_overrides[1], Some(track_two_id));
        assert_eq!(scenes.scenes[1].cells[1], Some(track_two_id));
        assert_eq!(
            scenes.track_pools[1]
                .get(scenes.scenes[1].cells[1].unwrap())
                .unwrap()
                .track_bits[0],
            44
        );
    }

    #[test]
    fn project_scenes_purge_unused_track_patterns_removes_only_unreferenced_orphans() {
        let first = sample_pattern_snapshot(1);
        let second = sample_pattern_snapshot(1);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let scene_zero_id = scenes.scenes[0].cells[0].unwrap();
        let scene_one_id = scenes.scenes[1].cells[0].unwrap();
        let override_only_id = scenes.fork_track_pattern(0).unwrap();
        let orphan_id = scenes.clear_cell(1, 0).unwrap();

        assert_eq!(orphan_id, scene_one_id);
        assert_eq!(scenes.purge_unused_track_patterns(), 1);

        assert!(scenes.track_pools[0].contains(scene_zero_id));
        assert!(scenes.track_pools[0].contains(override_only_id));
        assert!(!scenes.track_pools[0].contains(orphan_id));
        assert_eq!(scenes.track_overrides[0], Some(override_only_id));
    }

    #[test]
    fn pattern_snapshot_remove_track_compacts_all_track_lanes() {
        let mut snapshot = sample_pattern_snapshot(3);

        snapshot.remove_track(1);

        assert_eq!(snapshot.track_bits.len(), 2);
        assert_eq!(snapshot.step_data.len(), 2);
        assert_eq!(snapshot.track_params.len(), 2);
        assert_eq!(snapshot.effect_slots.len(), 2);
        assert_eq!(snapshot.instrument_slots.len(), 2);
        assert_eq!(snapshot.instrument_base_note_offsets.len(), 2);
        assert_eq!(snapshot.track_sound_states.len(), 2);
        assert_eq!(snapshot.sample_ids.len(), 2);
        assert_eq!(snapshot.chord_snapshots.len(), 2);
        assert_eq!(snapshot.timebase_plock_snapshots.len(), 2);
        assert_eq!(snapshot.swing_plock_snapshots.len(), 2);
        assert_eq!(snapshot.swing_resolution_plock_snapshots.len(), 2);
        assert_eq!(snapshot.instrument_types.len(), 2);
        assert_eq!(snapshot.instrument_run_modes.len(), 2);

        assert_eq!(snapshot.track_bits[0][0], 1);
        assert_eq!(snapshot.track_bits[1][0], 3);
        assert_eq!(snapshot.step_data[0][0][0], 0.25);
        assert_eq!(snapshot.step_data[1][0][0], 2.25);
        assert_eq!(snapshot.track_params[0].num_steps, 8);
        assert_eq!(snapshot.track_params[1].num_steps, 10);
        assert_eq!(snapshot.effect_slots[0][0].node_id, 100);
        assert_eq!(snapshot.effect_slots[1][0].node_id, 102);
        assert_eq!(snapshot.instrument_slots[0].node_id, 110);
        assert_eq!(snapshot.instrument_slots[1].node_id, 112);
        assert_eq!(snapshot.instrument_base_note_offsets, vec![-12.0, -10.0]);
        assert_eq!(
            snapshot.track_sound_states[0].loaded_preset.as_deref(),
            Some("preset-0")
        );
        assert_eq!(
            snapshot.track_sound_states[1].loaded_preset.as_deref(),
            Some("preset-2")
        );
        assert_eq!(snapshot.sample_ids[0], (0, "track-0".to_string(), 44_100));
        assert_eq!(snapshot.sample_ids[1], (2, "track-2".to_string(), 44_100));
        assert_eq!(snapshot.chord_snapshots[0].steps[0], vec![0.0, 7.0]);
        assert_eq!(snapshot.chord_snapshots[1].steps[0], vec![2.0, 9.0]);
        assert_eq!(snapshot.timebase_plock_snapshots[0][0], Some(0));
        assert_eq!(snapshot.timebase_plock_snapshots[1][0], Some(2));
        assert_eq!(snapshot.swing_plock_snapshots[0][1], Some(10));
        assert_eq!(snapshot.swing_plock_snapshots[1][1], Some(12));
        assert_eq!(snapshot.swing_resolution_plock_snapshots[0][2], Some(20));
        assert_eq!(snapshot.swing_resolution_plock_snapshots[1][2], Some(22));
        assert_eq!(snapshot.instrument_types[0], InstrumentType::Sampler);
        assert_eq!(snapshot.instrument_types[1], InstrumentType::Sampler);
        assert_eq!(
            snapshot.instrument_run_modes,
            vec![
                CustomInstrumentRunMode::Instrument,
                CustomInstrumentRunMode::Instrument
            ]
        );
    }

    #[test]
    fn pattern_snapshot_remove_track_shifts_first_track() {
        let mut snapshot = sample_pattern_snapshot(3);

        snapshot.remove_track(0);

        assert_eq!(snapshot.track_bits.len(), 2);
        assert_eq!(snapshot.track_bits[0][0], 2);
        assert_eq!(snapshot.track_bits[1][0], 3);
        assert_eq!(snapshot.sample_ids[0], (1, "track-1".to_string(), 44_100));
        assert_eq!(snapshot.sample_ids[1], (2, "track-2".to_string(), 44_100));
    }

    #[test]
    fn pattern_snapshot_remove_track_remaps_mod_connections() {
        let mut snapshot = sample_pattern_snapshot(4);
        snapshot.mod_connections = vec![
            ModConnection {
                source_track: 1,
                destination: ModDestination::Track(3),
                dest_input: 2,
            },
            ModConnection {
                source_track: 0,
                destination: ModDestination::Track(2),
                dest_input: 1,
            },
            ModConnection {
                source_track: 2,
                destination: ModDestination::Track(1),
                dest_input: 3,
            },
        ];

        snapshot.remove_track(1);

        assert_eq!(
            snapshot.mod_connections,
            vec![ModConnection {
                source_track: 0,
                destination: ModDestination::Track(1),
                dest_input: 1,
            }]
        );
    }

    #[test]
    fn pattern_snapshot_remove_track_preserves_bus_mod_destinations() {
        let mut snapshot = sample_pattern_snapshot(4);
        snapshot.mod_connections = vec![
            ModConnection {
                source_track: 2,
                destination: ModDestination::Bus(BusId(42)),
                dest_input: 1,
            },
            ModConnection {
                source_track: 1,
                destination: ModDestination::Bus(BusId(42)),
                dest_input: 2,
            },
        ];

        snapshot.remove_track(1);

        assert_eq!(
            snapshot.mod_connections,
            vec![ModConnection {
                source_track: 1,
                destination: ModDestination::Bus(BusId(42)),
                dest_input: 1,
            }]
        );
    }

    #[test]
    fn pattern_snapshot_remove_track_remaps_neural_routes() {
        let mut snapshot = sample_pattern_snapshot(4);
        let mut network = crate::neural::ProjectNeuralNetwork::default();
        network.neurons[0].route = Some(0);
        network.neurons[1].route = Some(1);
        network.neurons[2].route = Some(3);
        snapshot.neural_networks = vec![network];

        snapshot.remove_track(1);

        let neurons = &snapshot.neural_networks[0].neurons;
        assert_eq!(neurons[0].route, Some(0));
        assert_eq!(neurons[1].route, None);
        assert_eq!(neurons[2].route, Some(2));
    }

    #[test]
    fn pattern_snapshot_normalize_fills_missing_loaded_project_lanes() {
        let mut snapshot = sample_pattern_snapshot(1);
        snapshot.step_data[0].truncate(3);
        snapshot.normalize_track_count(3, &[]);

        assert!(snapshot.track_lane_count_is_consistent());
        assert_eq!(snapshot.track_bits.len(), 3);
        assert_eq!(snapshot.track_bits[0][0], 1);
        assert_eq!(snapshot.track_bits[1], [0; TRACK_PATTERN_WORDS]);
        assert_eq!(snapshot.track_bits[2], [0; TRACK_PATTERN_WORDS]);
        assert_eq!(snapshot.step_data[0].len(), MAX_STEPS);
        assert_eq!(
            snapshot.step_data[0][3][StepParam::Velocity.index()],
            StepParam::Velocity.default_value()
        );
        assert_eq!(
            snapshot.track_params[1].num_steps,
            TrackParamsSnapshot::default().num_steps
        );
        assert_eq!(snapshot.sample_ids[2], (-1, String::new(), 44_100));
        assert_eq!(snapshot.instrument_types[1], InstrumentType::Sampler);
        assert_eq!(
            snapshot.instrument_run_modes[1],
            CustomInstrumentRunMode::Instrument
        );
    }

    #[test]
    fn pattern_snapshot_normalize_truncates_extra_loaded_project_lanes() {
        let mut snapshot = sample_pattern_snapshot(4);
        snapshot.normalize_track_count(2, &[]);

        assert!(snapshot.track_lane_count_is_consistent());
        assert_eq!(snapshot.track_bits.len(), 2);
        assert_eq!(snapshot.track_bits[0][0], 1);
        assert_eq!(snapshot.track_bits[1][0], 2);
        assert_eq!(
            snapshot.sample_ids,
            vec![
                (0, "track-0".to_string(), 44_100),
                (1, "track-1".to_string(), 44_100)
            ]
        );
    }

    #[test]
    fn pattern_snapshot_remove_effect_slot_compacts_slot_plocks() {
        let mut snapshot = sample_pattern_snapshot(1);
        snapshot.effect_slots[0] = vec![
            sample_effect_slot_snapshot(0),
            sample_effect_slot_snapshot(1),
            sample_effect_slot_snapshot(2),
        ];

        snapshot.remove_effect_slot(0, 1);

        assert_eq!(snapshot.effect_slots[0].len(), 3);
        assert_eq!(snapshot.effect_slots[0][0].node_id, 100);
        assert_eq!(snapshot.effect_slots[0][1].node_id, 102);
        assert_eq!(snapshot.effect_slots[0][1].defaults, vec![2.0, 2.5]);
        assert_eq!(snapshot.effect_slots[0][1].plocks[2][0], Some(2.0));
        assert_eq!(snapshot.effect_slots[0][2].node_id, 0);
        assert_eq!(snapshot.effect_slots[0][2].num_params, 0);
        assert!(snapshot.effect_slots[0][2].defaults.is_empty());
        assert!(snapshot.effect_slots[0][2].plocks[2].is_empty());
    }

    #[test]
    fn pattern_snapshot_insert_empty_effect_slot_shifts_existing_slots() {
        let mut snapshot = sample_pattern_snapshot(1);
        snapshot.effect_slots[0] = vec![
            sample_effect_slot_snapshot(0),
            sample_effect_slot_snapshot(1),
            sample_effect_slot_snapshot(2),
            EffectSlotSnapshot::new_empty(),
        ];

        snapshot.insert_empty_effect_slot(0, 1);

        assert_eq!(snapshot.effect_slots[0][0].node_id, 100);
        assert_eq!(snapshot.effect_slots[0][1].node_id, 0);
        assert_eq!(snapshot.effect_slots[0][2].node_id, 101);
        assert_eq!(snapshot.effect_slots[0][3].node_id, 102);
    }

    #[test]
    fn pattern_snapshot_move_effect_slot_reorders_without_losing_payload() {
        let mut snapshot = sample_pattern_snapshot(1);
        snapshot.effect_slots[0] = vec![
            sample_effect_slot_snapshot(0),
            sample_effect_slot_snapshot(1),
            sample_effect_slot_snapshot(2),
            EffectSlotSnapshot::new_empty(),
        ];

        snapshot.move_effect_slot_to(0, 2, 1);

        assert_eq!(snapshot.effect_slots[0][0].node_id, 100);
        assert_eq!(snapshot.effect_slots[0][1].node_id, 102);
        assert_eq!(snapshot.effect_slots[0][1].defaults, vec![2.0, 2.5]);
        assert_eq!(snapshot.effect_slots[0][2].node_id, 101);
        assert_eq!(snapshot.effect_slots[0][3].node_id, 0);
    }

    #[test]
    fn pattern_snapshot_remove_track_ignores_out_of_range_index() {
        let mut snapshot = sample_pattern_snapshot(2);
        let original = snapshot.clone();

        snapshot.remove_track(5);

        assert_eq!(snapshot.track_bits, original.track_bits);
        assert_eq!(snapshot.step_data, original.step_data);
        assert_eq!(snapshot.track_params.len(), original.track_params.len());
        assert_eq!(snapshot.sample_ids, original.sample_ids);
    }

    #[test]
    fn pattern_snapshot_remove_track_tolerates_sparse_legacy_lanes() {
        let mut snapshot = sample_pattern_snapshot(3);
        snapshot.swing_plock_snapshots.clear();
        snapshot.swing_resolution_plock_snapshots.truncate(1);
        snapshot.instrument_types.truncate(2);
        snapshot.instrument_run_modes.truncate(2);

        snapshot.remove_track(1);

        assert_eq!(snapshot.track_bits.len(), 2);
        assert_eq!(snapshot.step_data.len(), 2);
        assert_eq!(snapshot.track_params.len(), 2);
        assert_eq!(snapshot.swing_plock_snapshots.len(), 0);
        assert_eq!(snapshot.swing_resolution_plock_snapshots.len(), 1);
        assert_eq!(snapshot.instrument_types.len(), 1);
        assert_eq!(snapshot.instrument_run_modes.len(), 1);
    }

    #[test]
    fn remap_sidechain_selection_resets_deleted_source_to_off() {
        let remapped = remap_sidechain_selection_after_track_delete(0, 2, 2, 4);
        assert_eq!(remapped, 0);
    }

    #[test]
    fn remap_sidechain_selection_shifts_source_above_deleted_track() {
        let remapped = remap_sidechain_selection_after_track_delete(0, 3, 1, 4);
        assert_eq!(remapped, 2);
    }

    #[test]
    fn remap_snapshot_sidechain_references_updates_defaults_and_plocks() {
        let mut snapshot = PatternSnapshot {
            track_bits: vec![[0; TRACK_PATTERN_WORDS]; 4],
            neural_reset_bits: vec![[0; TRACK_PATTERN_WORDS]; 4],
            step_data: vec![vec![[0.0; NUM_PARAMS]; MAX_STEPS]; 4],
            track_params: vec![TrackParamsSnapshot::default(); 4],
            effect_slots: vec![
                vec![EffectSlotSnapshot::new_empty()],
                vec![EffectSlotSnapshot {
                    node_id: 1,
                    modulator_node_id: 0,
                    num_params: 1,
                    defaults: vec![3.0],
                    plocks: {
                        let mut plocks = (0..MAX_STEPS).map(|_| vec![None]).collect::<Vec<_>>();
                        plocks[0][0] = Some(2.0);
                        plocks
                    },
                    plock_param_ids: vec![vec![None]; MAX_STEPS],
                    key_locks: std::collections::BTreeMap::new(),
                    key_lock_param_ids: std::collections::BTreeMap::new(),
                    param_node_indices: vec![0],
                    param_node_spans: vec![1],
                    transport_phase_param_idx: crate::effects::NO_TRANSPORT_PHASE_PARAM,
                    tensor_params: Vec::new(),
                    ir: None,
                }],
                vec![EffectSlotSnapshot::new_empty()],
                vec![EffectSlotSnapshot::new_empty()],
            ],
            midi_fx_slots: vec![
                vec![
                    EffectSlotSnapshot::new_empty();
                    crate::lisp_host::MAX_MIDI_FX_SLOTS
                ];
                4
            ],
            instrument_slots: vec![EffectSlotSnapshot::new_empty(); 4],
            instrument_base_note_offsets: vec![0.0; 4],
            track_sound_states: vec![TrackSoundState::default(); 4],
            sample_ids: vec![(-1, String::new(), 44_100); 4],
            chord_snapshots: (0..4).map(|_| ChordSnapshot::new_default()).collect(),
            timebase_plock_snapshots: vec![[None; MAX_STEPS]; 4],
            swing_plock_snapshots: vec![[None; MAX_STEPS]; 4],
            swing_resolution_plock_snapshots: vec![[None; MAX_STEPS]; 4],
            instrument_types: vec![InstrumentType::Sampler; 4],
            instrument_run_modes: vec![CustomInstrumentRunMode::Instrument; 4],
            mod_connections: Vec::new(),
            neural_networks: Vec::new(),
            graph_overrides: Vec::new(),
            rack_tracks: vec![None; 4],
            process_chains: vec![crate::process::TrackProcessChain::default(); 4],
            project_process_chain: crate::process::TrackProcessChain::default(),
            plock_variant_registries: vec![PlockVariantRegistry::default(); 4],
            key_lock_variant_registries: vec![PlockVariantRegistry::default(); 4],
        };
        let descriptors = vec![
            vec![EffectDescriptor::builtin_filter()],
            vec![sample_sidechain_descriptor()],
            vec![EffectDescriptor::builtin_filter()],
            vec![EffectDescriptor::builtin_filter()],
        ];

        remap_snapshot_sidechain_references_after_track_delete(&mut snapshot, &descriptors, 2, 4);

        assert_eq!(snapshot.effect_slots[1][0].defaults[0], 2.0);
        assert_eq!(snapshot.effect_slots[1][0].plocks[0][0], Some(0.0));
    }

    #[test]
    fn move_step_range_preserves_chords_and_step_plocks() {
        let state = SequencerState::new(
            1,
            vec![vec![EffectSlotState::new(
                &EffectDescriptor::builtin_filter(),
                1,
            )]],
        );
        state.pattern.track_params[0].set_num_steps(8);
        state.pattern.instrument_slots[0].apply_descriptor(&EffectDescriptor::builtin_delay(), 2);

        state.pattern.patterns[0].toggle_step(1);
        state.pattern.step_data[0].set(1, StepParam::Velocity, 0.6);
        state.pattern.chord_data[0].add_note(1, 0.0);
        state.pattern.chord_data[0].add_note(1, 4.0);
        state.pattern.chord_data[0].add_note(1, 7.0);
        state.pattern.timebase_plocks[0].set(1, Timebase::Eighth);
        state.pattern.effect_chains[0][0].set_plock(1, 2, 440.0);
        state.pattern.instrument_slots[0].set_plock(1, 0, 0.75);

        state.pattern.patterns[0].toggle_step(2);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.3);
        state.pattern.chord_data[0].add_note(2, 12.0);
        state.pattern.timebase_plocks[0].set(2, Timebase::QuarterTriplet);
        state.pattern.effect_chains[0][0].set_plock(2, 2, 880.0);
        state.pattern.instrument_slots[0].set_plock(2, 0, 0.25);

        state.move_step_range(0, 1, 2, 2);

        assert!(!state.pattern.patterns[0].is_active(1));
        assert_eq!(state.pattern.chord_data[0].count(1), 0);
        assert_eq!(
            state.pattern.step_data[0].get(1, StepParam::Velocity),
            StepParam::Velocity.default_value()
        );
        assert_eq!(state.pattern.timebase_plocks[0].get(1), None);
        assert_eq!(state.pattern.effect_chains[0][0].plocks.get(1, 2), None);
        assert_eq!(state.pattern.instrument_slots[0].plocks.get(1, 0), None);

        assert!(state.pattern.patterns[0].is_active(2));
        assert_eq!(state.pattern.step_data[0].get(2, StepParam::Velocity), 0.6);
        assert_eq!(state.pattern.chord_data[0].count(2), 3);
        assert_eq!(state.pattern.chord_data[0].get(2, 0), 0.0);
        assert_eq!(state.pattern.chord_data[0].get(2, 1), 4.0);
        assert_eq!(state.pattern.chord_data[0].get(2, 2), 7.0);
        assert_eq!(
            state.pattern.timebase_plocks[0].get(2),
            Some(Timebase::Eighth)
        );
        assert_eq!(
            state.pattern.effect_chains[0][0].plocks.get(2, 2),
            Some(440.0)
        );
        assert_eq!(
            state.pattern.instrument_slots[0].plocks.get(2, 0),
            Some(0.75)
        );

        assert!(state.pattern.patterns[0].is_active(3));
        assert_eq!(state.pattern.step_data[0].get(3, StepParam::Velocity), 0.3);
        assert_eq!(state.pattern.chord_data[0].count(3), 1);
        assert_eq!(state.pattern.chord_data[0].get(3, 0), 12.0);
        assert_eq!(
            state.pattern.timebase_plocks[0].get(3),
            Some(Timebase::QuarterTriplet)
        );
        assert_eq!(
            state.pattern.effect_chains[0][0].plocks.get(3, 2),
            Some(880.0)
        );
        assert_eq!(
            state.pattern.instrument_slots[0].plocks.get(3, 0),
            Some(0.25)
        );
    }

    fn make_state_with_instrument() -> SequencerState {
        let state = SequencerState::new(
            1,
            vec![vec![EffectSlotState::new(
                &EffectDescriptor::builtin_filter(),
                1,
            )]],
        );
        state.pattern.track_params[0].set_num_steps(8);
        state.pattern.instrument_slots[0].apply_descriptor(&EffectDescriptor::builtin_delay(), 2);
        state
    }

    fn make_state_with_rack_slot() -> SequencerState {
        let state = make_state_with_instrument();
        state.set_rack_track_for_all_pattern_snapshots(0, sample_rack_track_snapshot());
        state
    }

    fn make_state_with_tracks(num_tracks: usize) -> SequencerState {
        SequencerState::new(
            num_tracks,
            (0..num_tracks)
                .map(|_| default_empty_effect_chain())
                .collect(),
        )
    }

    fn sample_process_chain() -> crate::process::TrackProcessChain {
        crate::process::TrackProcessChain {
            slots: vec![crate::process::TrackProcessSlot {
                instance_id: crate::process::ProcessInstanceId(7),
                instance_name: Some("sparse-h".to_string()),
                class_name: "sparse".to_string(),
                enabled: true,
                project_layer: false,
                inlets: std::collections::BTreeMap::new(),
                lanes: std::collections::BTreeMap::from([(
                    "amount".to_string(),
                    crate::process::ProcessLane {
                        values: vec![0.0, 1.0],
                    },
                )]),
                bindings: std::collections::BTreeMap::new(),
            }],
        }
    }

    fn effect_process_chain(
        port: &str,
        effect: &str,
        param: &str,
        param_id: ParamNodeId,
    ) -> crate::process::TrackProcessChain {
        crate::process::TrackProcessChain {
            slots: vec![crate::process::TrackProcessSlot {
                instance_id: crate::process::ProcessInstanceId(8),
                instance_name: Some("phase3b-writer-h".to_string()),
                class_name: "phase3b-mappable-writer".to_string(),
                enabled: true,
                project_layer: false,
                inlets: std::collections::BTreeMap::new(),
                lanes: std::collections::BTreeMap::new(),
                bindings: std::collections::BTreeMap::from([(
                    port.to_string(),
                    Some(crate::process::ParamTarget::EffectParam {
                        slot: 0,
                        effect: effect.to_string(),
                        param: param.to_string(),
                        param_id: Some(param_id),
                    }),
                )]),
            }],
        }
    }

    fn effect_binding_param_id(
        chain: &crate::process::TrackProcessChain,
        port: &str,
    ) -> Option<ParamNodeId> {
        let binding = chain.slots.first()?.bindings.get(port)?.as_ref()?;
        match binding {
            crate::process::ParamTarget::EffectParam { param_id, .. } => *param_id,
            _ => None,
        }
    }

    #[test]
    fn process_binding_param_ids_refresh_to_restored_effect_slot() {
        let desc = EffectDescriptor::builtin_insert("Str8 Delay").expect("Str8 Delay descriptor");
        let wet_idx = desc
            .params
            .iter()
            .position(|param| param.name == "wet")
            .expect("Str8 Delay should expose wet");
        let fresh_slot = EffectSlotSnapshot::new_default(&desc, 130);
        let expected = ParamNodeId::from_slot_param(
            fresh_slot.node_id,
            fresh_slot.modulator_node_id,
            fresh_slot.param_node_indices[wet_idx],
        )
        .expect("wet should have a live node identity");
        let stale = ParamNodeId {
            logical_id: 79,
            node_param_idx: expected.node_param_idx,
        };

        let mut snapshot = sample_pattern_snapshot(1);
        snapshot.effect_slots[0] = vec![fresh_slot];
        snapshot.process_chains[0] = effect_process_chain("color", &desc.name, "wet", stale);

        snapshot.refresh_process_binding_param_ids(&[vec![desc]], &[]);

        assert_eq!(
            effect_binding_param_id(&snapshot.process_chains[0], "color"),
            Some(expected)
        );
    }

    #[test]
    fn process_binding_param_ids_refresh_when_scene_restores_stored_chain() {
        let desc = EffectDescriptor::builtin_insert("Str8 Delay").expect("Str8 Delay descriptor");
        let wet_idx = desc
            .params
            .iter()
            .position(|param| param.name == "wet")
            .expect("Str8 Delay should expose wet");
        let first_slot = EffectSlotSnapshot::new_default(&desc, 130);
        let second_slot = EffectSlotSnapshot::new_default(&desc, 131);
        let first_expected = ParamNodeId::from_slot_param(
            first_slot.node_id,
            first_slot.modulator_node_id,
            first_slot.param_node_indices[wet_idx],
        )
        .expect("first wet should have a live node identity");
        let second_expected = ParamNodeId::from_slot_param(
            second_slot.node_id,
            second_slot.modulator_node_id,
            second_slot.param_node_indices[wet_idx],
        )
        .expect("second wet should have a live node identity");
        let stale = ParamNodeId {
            logical_id: 79,
            node_param_idx: first_expected.node_param_idx,
        };

        let state = make_state_with_tracks(1);
        state.set_scratch_runtime_descriptors(
            vec![vec![desc.clone()]],
            vec![EffectDescriptor::builtin_sampler()],
        );

        let mut first = sample_pattern_snapshot(1);
        first.effect_slots[0] = vec![first_slot];
        first.process_chains[0] = effect_process_chain("color", &desc.name, "wet", stale);
        let mut second = sample_pattern_snapshot(1);
        second.effect_slots[0] = vec![second_slot];
        second.process_chains[0] = effect_process_chain("color", &desc.name, "wet", stale);

        let buffer_ids = [-1];
        let sample_rates = [44_100];
        let names = [String::new()];
        let instrument_types = [InstrumentType::Sampler];
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        assert_eq!(
            state
                .track_process_chain(0)
                .and_then(|chain| effect_binding_param_id(&chain, "color")),
            Some(first_expected)
        );

        state
            .switch_pattern(1, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch to scene 2");
        assert_eq!(
            state
                .track_process_chain(0)
                .and_then(|chain| effect_binding_param_id(&chain, "color")),
            Some(second_expected)
        );
        {
            let scheduler_snapshot = state.latest_scheduler_snapshot();
            assert_eq!(
                effect_binding_param_id(&scheduler_snapshot.tracks[0].process_chain, "color"),
                Some(second_expected)
            );
            assert_eq!(
                scheduler_snapshot.tracks[0].effect_descriptors[0].name,
                desc.name
            );
        }

        state
            .switch_pattern(0, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch back to scene 1");
        assert_eq!(
            state
                .track_process_chain(0)
                .and_then(|chain| effect_binding_param_id(&chain, "color")),
            Some(first_expected)
        );
        {
            let scheduler_snapshot = state.latest_scheduler_snapshot();
            assert_eq!(
                effect_binding_param_id(&scheduler_snapshot.tracks[0].process_chain, "color"),
                Some(first_expected)
            );
            assert_eq!(
                scheduler_snapshot.tracks[0].effect_descriptors[0].name,
                desc.name
            );
        }
    }

    #[test]
    fn process_chain_and_lane_values_survive_snapshots_and_project_save() {
        let state = make_state_with_tracks(1);
        let mut expected = sample_process_chain();

        assert!(state.set_track_process_chain(0, expected.clone()));
        assert!(state.set_process_lane_value(
            0,
            crate::process::ProcessInstanceId(7),
            "amount",
            4,
            2.0,
        ));
        expected.slots[0]
            .lanes
            .get_mut("amount")
            .unwrap()
            .values
            .resize(5, 0.0);
        expected.slots[0].lanes.get_mut("amount").unwrap().values[4] = 2.0;

        assert_eq!(state.track_process_chain(0), Some(expected.clone()));
        assert_eq!(
            SequencerSnapshot::capture(&state).tracks[0].process_chain,
            expected
        );

        let snapshot = PatternSnapshot::capture(
            &state,
            1,
            &[-1],
            &[44_100],
            &[String::new()],
            &[InstrumentType::Sampler],
        );
        assert_eq!(snapshot.process_chains[0], expected);
        let project_pattern = crate::project::ProjectPattern::from_snapshot(
            &snapshot,
            vec![None],
            vec![String::new()],
            Vec::new(),
        );
        assert_eq!(project_pattern.process_chains[0], expected);

        assert!(state.set_track_process_chain(0, crate::process::TrackProcessChain::default()));
        assert_eq!(
            state.track_process_chain(0),
            Some(crate::process::TrackProcessChain::default())
        );
        assert!(snapshot.restore_track(&state, 0));
        assert_eq!(state.track_process_chain(0), Some(expected));
    }

    #[test]
    fn process_chain_slot_edits_are_track_scoped_and_snapshot_visible() {
        let state = make_state_with_tracks(2);
        let mut first = sample_process_chain().slots.remove(0);
        let mut second = first.clone();
        second.instance_id = crate::process::ProcessInstanceId(8);
        second.class_name = "second".to_string();
        let mut third = first.clone();
        third.instance_id = crate::process::ProcessInstanceId(9);
        third.class_name = "third".to_string();
        first.inlets.insert(
            "depth".to_string(),
            crate::process::ProcessLiteral::Number(1.0),
        );
        second.inlets = first.inlets.clone();
        third.inlets = first.inlets.clone();
        let chain = crate::process::TrackProcessChain {
            slots: vec![first, second, third],
        };
        assert!(state.set_track_process_chain(0, chain));

        assert!(state.set_track_process_slot_enabled(
            0,
            crate::process::ProcessInstanceId(8),
            false,
        ));
        assert!(state.move_track_process_slot_before(
            0,
            crate::process::ProcessInstanceId(9),
            Some(crate::process::ProcessInstanceId(7)),
        ));
        assert!(state.set_track_process_inlet_value(
            0,
            crate::process::ProcessInstanceId(8),
            "depth",
            crate::process::ProcessLiteral::Number(4.0),
        ));

        let edited = state.track_process_chain(0).expect("track 1 process chain");
        assert_eq!(
            edited
                .slots
                .iter()
                .map(|slot| slot.instance_id.0)
                .collect::<Vec<_>>(),
            vec![9, 7, 8]
        );
        assert!(!edited.slots[2].enabled);
        assert_eq!(
            edited.slots[2].inlets.get("depth"),
            Some(&crate::process::ProcessLiteral::Number(4.0))
        );
        assert!(
            state
                .track_process_chain(1)
                .expect("track 2 process chain")
                .slots
                .is_empty(),
            "slot edits must not leak to another track"
        );
        assert_eq!(
            state.latest_scheduler_snapshot().tracks[0].process_chain,
            edited,
            "every successful slot edit must publish to the scheduler snapshot"
        );

        assert!(state.move_track_process_slot_before(
            0,
            crate::process::ProcessInstanceId(7),
            None,
        ));
        assert_eq!(
            state
                .track_process_chain(0)
                .unwrap()
                .slots
                .iter()
                .map(|slot| slot.instance_id.0)
                .collect::<Vec<_>>(),
            vec![9, 8, 7]
        );
        assert!(state.remove_track_process_slot(0, crate::process::ProcessInstanceId(9)));
        assert_eq!(
            state
                .track_process_chain(0)
                .unwrap()
                .slots
                .iter()
                .map(|slot| slot.instance_id.0)
                .collect::<Vec<_>>(),
            vec![8, 7]
        );
        assert!(!state.remove_track_process_slot(0, crate::process::ProcessInstanceId(99)));
        assert!(!state.move_track_process_slot_before(
            0,
            crate::process::ProcessInstanceId(99),
            None,
        ));
    }

    #[test]
    fn process_chain_and_lane_values_survive_scene_switching() {
        let state = make_state_with_tracks(1);
        let mut first = PatternSnapshot::new_default(1, &[]);
        first.process_chains[0] = sample_process_chain();
        let mut second = PatternSnapshot::new_default(1, &[]);
        second.process_chains[0] = sample_process_chain();
        second.process_chains[0].slots[0].instance_id = crate::process::ProcessInstanceId(99);
        second.process_chains[0].slots[0].class_name = "second".to_string();
        second.process_chains[0].slots[0]
            .lanes
            .get_mut("amount")
            .unwrap()
            .values = vec![4.0, 5.0, 6.0];

        let buffer_ids = [-1];
        let sample_rates = [44_100];
        let names = [String::new()];
        let instrument_types = [InstrumentType::Sampler];
        state.replace_pattern_repository(vec![first.clone(), second.clone()], 0);
        state.restore_current_pattern_from_repository().unwrap();
        assert_eq!(
            state.track_process_chain(0),
            Some(first.process_chains[0].clone())
        );

        state
            .switch_pattern(1, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch to scene 2");
        assert_eq!(
            state.track_process_chain(0),
            Some(second.process_chains[0].clone())
        );

        state
            .switch_pattern(0, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch back to scene 1");
        assert_eq!(
            state.track_process_chain(0),
            Some(first.process_chains[0].clone())
        );
    }

    #[test]
    fn project_process_chain_is_scene_scoped_and_survives_scene_switching() {
        let state = make_state_with_tracks(2);
        let mut project_chain = sample_process_chain();
        project_chain.slots[0].project_layer = true;

        let mut first = PatternSnapshot::new_default(2, &[]);
        first.project_process_chain = project_chain.clone();
        let second = PatternSnapshot::new_default(2, &[]);

        let buffer_ids = [-1, -1];
        let sample_rates = [44_100, 44_100];
        let names = [String::new(), String::new()];
        let instrument_types = [InstrumentType::Sampler, InstrumentType::Sampler];
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();

        assert_eq!(state.project_process_chain(), project_chain);
        // Every track's effective chain starts with the shared project slot.
        for track in 0..2 {
            let composed = state
                .composed_track_process_chain(track)
                .expect("composed chain");
            assert_eq!(composed.slots.len(), 1);
            assert!(composed.slots[0].project_layer);
        }
        // The scheduler snapshot sees the composed chain on every track.
        let snapshot = state.publish_scheduler_snapshot();
        for track in 0..2 {
            assert_eq!(snapshot.tracks[track].process_chain.slots.len(), 1);
            assert!(snapshot.tracks[track].process_chain.slots[0].project_layer);
        }

        // Settings are pattern-scoped: scene 2 has its own (empty) layer.
        state
            .switch_pattern(1, 2, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch to scene 2");
        assert!(state.project_process_chain().slots.is_empty());
        let snapshot = state.latest_scheduler_snapshot();
        assert!(snapshot.tracks[0].process_chain.slots.is_empty());

        state
            .switch_pattern(0, 2, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch back to scene 1");
        assert_eq!(state.project_process_chain(), project_chain);

        // Whole-layer replace + export roundtrip.
        assert!(state.set_project_process_chain(crate::process::TrackProcessChain::default()));
        assert!(state.project_process_chain().slots.is_empty());
        assert!(state.set_project_process_chain(project_chain.clone()));
        let bank = state.export_pattern_repository();
        assert_eq!(bank[0].project_process_chain, project_chain);
        assert!(bank[1].project_process_chain.slots.is_empty());
    }

    #[test]
    fn project_process_chain_slot_edits_reach_the_shared_slot_from_any_track() {
        let state = make_state_with_tracks(2);
        let mut project_chain = sample_process_chain();
        project_chain.slots[0].project_layer = true;
        let instance_id = project_chain.slots[0].instance_id;
        assert!(state.set_project_process_chain(project_chain));

        // Track-scoped mutators fall back to the shared project slot when the
        // instance is not in that track's own chain (edit-from-any-track).
        assert!(state.set_track_process_slot_enabled(1, instance_id, false));
        assert!(!state.project_process_chain().slots[0].enabled);
        assert!(state.set_track_process_slot_enabled(0, instance_id, true));
        assert!(state.project_process_chain().slots[0].enabled);

        assert!(state.set_process_lane_value(1, instance_id, "amount", 2, 7.0));
        assert_eq!(
            state.project_process_chain().slots[0]
                .lanes
                .get("amount")
                .map(|lane| lane.values.clone()),
            Some(vec![0.0, 1.0, 7.0])
        );

        // Instance-wide mutators (lane! / handle knob writes) reach it too.
        assert_eq!(
            state.set_process_lane_values(instance_id, "amount", vec![0.0, 3.0]),
            1
        );
        assert_eq!(state.process_instance_attachment_count(instance_id), 1);

        // Removing from any track's panel removes the shared slot.
        assert!(state.remove_track_process_slot(0, instance_id));
        assert!(state.project_process_chain().slots.is_empty());
        assert!(!state.remove_track_process_slot(0, instance_id));
    }

    #[test]
    fn clone_pattern_preserves_mod_connections_on_source_and_clone() {
        let state = make_state_with_tracks(2);
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 2,
        };
        state
            .edit_current_mod_connections(|routes| {
                routes.push(route);
                Ok(())
            })
            .unwrap();

        let cloned_idx = state.clone_pattern(
            2,
            &[-1, -1],
            &[44_100, 44_100],
            &[String::from("mod"), String::from("synth")],
            &[InstrumentType::Modulator, InstrumentType::Custom],
        );

        let bank = state.export_pattern_repository();
        assert_eq!(cloned_idx, 1);
        assert_eq!(bank[0].mod_connections, vec![route]);
        assert_eq!(bank[1].mod_connections, vec![route]);
    }

    #[test]
    fn switch_pattern_publishes_snapshot_after_releasing_pattern_bank() {
        let state = make_state_with_tracks(2);
        state.pattern.patterns[0].toggle_step(5);
        state.pattern.step_data[0].set(5, StepParam::Velocity, 0.75);
        state.pattern.chord_data[0].add_note(5, 7.0);
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 3,
        };
        state
            .edit_current_mod_connections(|routes| {
                routes.push(route);
                Ok(())
            })
            .unwrap();
        state.clone_pattern(
            2,
            &[-1, -1],
            &[44_100, 44_100],
            &[String::from("mod"), String::from("synth")],
            &[InstrumentType::Modulator, InstrumentType::Custom],
        );
        state
            .edit_current_mod_connections(|routes| {
                routes.clear();
                Ok(())
            })
            .unwrap();

        let sample_ids = state.switch_pattern(
            0,
            2,
            &[-1, -1],
            &[44_100, 44_100],
            &[String::from("mod"), String::from("synth")],
            &[InstrumentType::Modulator, InstrumentType::Custom],
        );

        assert!(sample_ids.is_some());
        let snapshot = state.latest_scheduler_snapshot();
        assert_eq!(snapshot.transport.current_pattern, 0);
        assert_eq!(snapshot.mod_connections, vec![route]);
        assert!(snapshot.tracks[0].steps[5].active);
        assert_eq!(
            snapshot.tracks[0].steps[5].params[StepParam::Velocity.index()],
            0.75
        );
        assert_eq!(snapshot.tracks[0].steps[5].chord, vec![7.0]);
    }

    #[test]
    fn delete_pattern_preserves_remaining_pattern_mod_connections() {
        let state = make_state_with_tracks(2);
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 1,
        };
        state
            .edit_current_mod_connections(|routes| {
                routes.push(route);
                Ok(())
            })
            .unwrap();
        state.clone_pattern(
            2,
            &[-1, -1],
            &[44_100, 44_100],
            &[String::from("mod"), String::from("synth")],
            &[InstrumentType::Modulator, InstrumentType::Custom],
        );

        let sample_ids = state.delete_pattern(
            2,
            &[-1, -1],
            &[44_100, 44_100],
            &[String::from("mod"), String::from("synth")],
            &[InstrumentType::Modulator, InstrumentType::Custom],
        );

        assert!(sample_ids.is_some());
        let bank = state.export_pattern_repository();
        assert_eq!(bank.len(), 1);
        assert_eq!(bank[0].mod_connections, vec![route]);
    }

    fn launch_test_args() -> (Vec<i32>, Vec<u32>, Vec<String>, Vec<InstrumentType>) {
        (
            vec![-1, -1],
            vec![44_100, 44_100],
            vec![String::from("one"), String::from("two")],
            vec![InstrumentType::Sampler, InstrumentType::Sampler],
        )
    }

    fn snapshot_with_active_step(track_count: usize, track: usize, step: usize) -> PatternSnapshot {
        let mut snapshot = PatternSnapshot::new_default(track_count, &[]);
        snapshot.track_bits[track][step / 64] |= 1u64 << (step % 64);
        snapshot
    }

    #[test]
    fn launch_track_pattern_changes_only_requested_track_and_scene_launch_clears_override() {
        let state = make_state_with_tracks(2);
        let first = PatternSnapshot::new_default(2, &[]);
        let second = snapshot_with_active_step(2, 0, 3);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let pattern_id = state.scene_track_pattern_id(1, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        assert!(state.launch_track_pattern(
            0,
            pattern_id,
            2,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));

        assert!(state.pattern.patterns[0].is_active(3));
        assert!(!state.pattern.patterns[1].is_active(3));
        assert_eq!(state.current_scene_index(), 0);

        state
            .launch_scene(0, 2, &buffer_ids, &sample_rates, &names, &instrument_types)
            .unwrap();
        assert!(
            !state.pattern.patterns[0].is_active(3),
            "scene launch should clear the per-track override"
        );
    }

    #[test]
    fn track_pattern_cells_report_assigned_active_and_override_state() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![sample_pattern_snapshot(1), sample_pattern_snapshot(1)],
            1,
        );
        let scene_zero_id = state.scene_track_pattern_id(0, 0).unwrap();
        let scene_one_id = state.scene_track_pattern_id(1, 0).unwrap();

        let cells = state.track_pattern_cells(0);
        assert_eq!(cells.len(), 2);
        let scene_one_cell = cells
            .iter()
            .find(|cell| cell.pattern_id == scene_one_id)
            .unwrap();
        assert!(scene_one_cell.assigned_to_current_scene);
        assert!(scene_one_cell.active_effective);
        assert!(!scene_one_cell.overridden);

        assert!(state.launch_track_pattern(
            0,
            scene_zero_id,
            1,
            &[-1],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Sampler],
        ));

        let cells = state.track_pattern_cells(0);
        let override_cell = cells
            .iter()
            .find(|cell| cell.pattern_id == scene_zero_id)
            .unwrap();
        assert!(!override_cell.assigned_to_current_scene);
        assert!(override_cell.active_effective);
        assert!(override_cell.overridden);
        let assigned_cell = cells
            .iter()
            .find(|cell| cell.pattern_id == scene_one_id)
            .unwrap();
        assert!(assigned_cell.assigned_to_current_scene);
        assert!(!assigned_cell.active_effective);
        assert!(assigned_cell.overridden);
    }

    #[test]
    fn set_current_scene_cell_restores_shared_pattern_without_override() {
        let state = make_state_with_tracks(1);
        let first = PatternSnapshot::new_default(1, &[]);
        let second = snapshot_with_active_step(1, 0, 4);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let shared = state.scene_track_pattern_id(1, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        assert!(state.set_scene_cell(
            0,
            0,
            shared,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));

        assert!(state.pattern.patterns[0].is_active(4));
        let cells = state.track_pattern_cells(0);
        let shared_cell = cells.iter().find(|cell| cell.pattern_id == shared).unwrap();
        assert!(shared_cell.assigned_to_current_scene);
        assert!(shared_cell.active_effective);
        assert!(!shared_cell.overridden);
        assert!(!state.is_scene_silenced(0));
    }

    #[test]
    fn set_current_scene_cell_clears_override_and_persists_after_scene_return() {
        let state = make_state_with_tracks(1);
        let first = PatternSnapshot::new_default(1, &[]);
        let second = snapshot_with_active_step(1, 0, 4);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let shared = state.scene_track_pattern_id(1, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        assert!(state.launch_track_pattern(
            0,
            shared,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));
        assert!(state
            .track_pattern_cells(0)
            .into_iter()
            .any(|cell| cell.pattern_id == shared && cell.active_effective && cell.overridden));

        assert!(state.set_scene_cell(
            0,
            0,
            shared,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));
        assert!(state.track_pattern_cells(0).into_iter().any(|cell| {
            cell.pattern_id == shared
                && cell.assigned_to_current_scene
                && cell.active_effective
                && !cell.overridden
        }));

        state
            .launch_scene(1, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .unwrap();
        state
            .launch_scene(0, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .unwrap();

        assert_eq!(state.scene_track_pattern_id(0, 0), Some(shared));
        assert!(state.pattern.patterns[0].is_active(4));
        assert!(state.track_pattern_cells(0).into_iter().any(|cell| {
            cell.pattern_id == shared
                && cell.assigned_to_current_scene
                && cell.active_effective
                && !cell.overridden
        }));
    }

    #[test]
    fn clone_current_scene_track_pattern_commits_new_pattern_id() {
        let state = make_state_with_tracks(1);
        let first = snapshot_with_active_step(1, 0, 2);
        state.replace_pattern_repository(vec![first], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let original = state.scene_track_pattern_id(0, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        let cloned = state
            .clone_current_scene_track_pattern(
                0,
                1,
                &buffer_ids,
                &sample_rates,
                &names,
                &instrument_types,
            )
            .unwrap();

        assert_ne!(cloned, original);
        assert_eq!(state.scene_track_pattern_id(0, 0), Some(cloned));
        assert!(state.pattern.patterns[0].is_active(2));
        let cells = state.track_pattern_cells(0);
        assert!(cells.iter().any(|cell| cell.pattern_id == original
            && !cell.assigned_to_current_scene
            && !cell.active_effective));
        assert!(cells.iter().any(|cell| cell.pattern_id == cloned
            && cell.assigned_to_current_scene
            && cell.active_effective
            && !cell.overridden));
    }

    #[test]
    fn clone_selected_track_pattern_id_commits_that_source_into_current_scene() {
        let state = make_state_with_tracks(1);
        let first = snapshot_with_active_step(1, 0, 2);
        let second = snapshot_with_active_step(1, 0, 7);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let source = state.scene_track_pattern_id(1, 0).unwrap();
        let original = state.scene_track_pattern_id(0, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        let cloned = state
            .clone_track_pattern_id_into_current_scene(
                0,
                source,
                1,
                &buffer_ids,
                &sample_rates,
                &names,
                &instrument_types,
            )
            .unwrap();

        assert_ne!(cloned, source);
        assert_ne!(cloned, original);
        assert_eq!(state.scene_track_pattern_id(0, 0), Some(cloned));
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(state.pattern.patterns[0].is_active(7));
        assert!(state.track_pattern_cells(0).iter().any(|cell| {
            cell.pattern_id == cloned
                && cell.assigned_to_current_scene
                && cell.active_effective
                && !cell.overridden
        }));
    }

    #[test]
    fn delete_track_pattern_clears_scene_cells_and_silences_if_effective() {
        let state = make_state_with_tracks(1);
        let first = snapshot_with_active_step(1, 0, 2);
        let second = snapshot_with_active_step(1, 0, 5);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let first_id = state.scene_track_pattern_id(0, 0).unwrap();
        let second_id = state.scene_track_pattern_id(1, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        assert!(state.delete_track_pattern(
            0,
            second_id,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));
        assert_eq!(state.scene_track_pattern_id(1, 0), None);
        assert!(!state.is_scene_silenced(0));
        assert!(state
            .track_pattern_cells(0)
            .iter()
            .all(|cell| cell.pattern_id != second_id));

        assert!(state.delete_track_pattern(
            0,
            first_id,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));
        assert_eq!(state.scene_track_pattern_id(0, 0), None);
        assert!(state.is_scene_silenced(0));
        assert!(state.track_pattern_cells(0).is_empty());
    }

    #[test]
    fn clear_current_scene_cell_silences_without_deleting_pattern() {
        let state = make_state_with_tracks(1);
        let first = snapshot_with_active_step(1, 0, 2);
        state.replace_pattern_repository(vec![first], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let assigned = state.scene_track_pattern_id(0, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        let cleared = state.clear_scene_cell(
            0,
            0,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        );

        assert_eq!(cleared, Some(assigned));
        assert!(state.is_scene_silenced(0));
        assert!(state.pattern.patterns[0].is_active(2));
        assert!(state.track_pattern_cells(0).iter().any(|cell| {
            cell.pattern_id == assigned
                && !cell.assigned_to_current_scene
                && !cell.active_effective
                && !cell.overridden
        }));
    }

    #[test]
    fn launch_scene_with_empty_cell_silences_track_without_mutating_live_lane() {
        let state = make_state_with_tracks(1);
        state.clone_pattern(
            1,
            &[-1],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Sampler],
        );
        assert_eq!(state.current_scene_index(), 1);
        {
            let mut scenes = state.pattern.scenes.lock().unwrap();
            assert!(scenes.clear_cell(0, 0).is_some());
        }
        state.pattern.patterns[0].set_step_active(3, true);
        state.pattern.track_params[0].set_num_steps(7);

        let sample_ids = state.launch_scene(
            0,
            1,
            &[-1],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Sampler],
        );

        assert!(sample_ids.is_some());
        assert!(state.is_scene_silenced(0));
        assert!(state.latest_scheduler_snapshot().tracks[0].scene_silenced);
        assert!(state.pattern.patterns[0].is_active(3));
        assert_eq!(state.pattern.track_params[0].get_num_steps(), 7);
        assert_eq!(state.current_scene_index(), 0);
    }

    #[test]
    fn saving_scene_snapshot_preserves_empty_scene_cells() {
        let first = sample_pattern_snapshot(1);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first], 0);
        let orphan = scenes.clear_cell(0, 0).unwrap();
        let mut live = sample_pattern_snapshot(1);
        live.track_bits[0][0] = 99;

        assert!(scenes.save_scene_snapshot(0, live));

        assert_eq!(scenes.scenes[0].cells[0], None);
        assert!(scenes.track_pools[0].contains(orphan));
        assert_eq!(
            scenes.track_pools[0].get(orphan).unwrap().track_bits[0],
            1,
            "capturing while an empty cell is active must not overwrite orphan data"
        );
    }

    #[test]
    fn launch_scene_captures_live_edits_before_switching() {
        let state = make_state_with_tracks(2);
        let first = PatternSnapshot::new_default(2, &[]);
        let second = snapshot_with_active_step(2, 1, 7);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        state.pattern.patterns[0].set_step_active(5, true);
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        state
            .launch_scene(1, 2, &buffer_ids, &sample_rates, &names, &instrument_types)
            .unwrap();
        assert!(state.pattern.patterns[1].is_active(7));

        state
            .launch_scene(0, 2, &buffer_ids, &sample_rates, &names, &instrument_types)
            .unwrap();
        assert!(state.pattern.patterns[0].is_active(5));
    }

    fn populate_step(state: &SequencerState, track: usize, step: usize) {
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.neural_reset_patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Velocity, 0.75);
        state.pattern.step_data[track].set(step, StepParam::Transpose, 7.0);
        state.pattern.chord_data[track].add_note(step, 0.0);
        state.pattern.chord_data[track].add_note(step, 4.0);
        state.pattern.timebase_plocks[track].set(step, Timebase::Eighth);
        state.pattern.effect_chains[track][0].set_plock(step, 0, 440.0);
        state.pattern.instrument_slots[track].set_plock(step, 0, 0.5);
    }

    fn assert_step_matches_populated(state: &SequencerState, track: usize, step: usize) {
        assert!(
            state.pattern.patterns[track].is_active(step),
            "step {step} should be active"
        );
        assert!(
            state.pattern.neural_reset_patterns[track].is_active(step),
            "step {step} should carry neural reset"
        );
        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Velocity),
            0.75
        );
        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Transpose),
            7.0
        );
        assert_eq!(state.pattern.chord_data[track].count(step), 2);
        assert_eq!(state.pattern.chord_data[track].get(step, 0), 0.0);
        assert_eq!(state.pattern.chord_data[track].get(step, 1), 4.0);
        assert_eq!(
            state.pattern.timebase_plocks[track].get(step),
            Some(Timebase::Eighth)
        );
        assert_eq!(
            state.pattern.effect_chains[track][0].plocks.get(step, 0),
            Some(440.0)
        );
        assert_eq!(
            state.pattern.instrument_slots[track].plocks.get(step, 0),
            Some(0.5)
        );
    }

    fn assert_step_is_default(state: &SequencerState, track: usize, step: usize) {
        assert!(
            !state.pattern.patterns[track].is_active(step),
            "step {step} should be inactive"
        );
        assert!(
            !state.pattern.neural_reset_patterns[track].is_active(step),
            "step {step} should not carry neural reset"
        );
        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Velocity),
            StepParam::Velocity.default_value()
        );
        assert_eq!(state.pattern.chord_data[track].count(step), 0);
        assert_eq!(state.pattern.timebase_plocks[track].get(step), None);
        assert_eq!(
            state.pattern.effect_chains[track][0].plocks.get(step, 0),
            None
        );
        assert_eq!(
            state.pattern.instrument_slots[track].plocks.get(step, 0),
            None
        );
    }

    fn rack_slot_plock_value(
        state: &SequencerState,
        track: usize,
        slot_idx: usize,
        step: usize,
        param_idx: usize,
    ) -> Option<f32> {
        state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(|rack| rack.as_ref())
            .and_then(|rack| rack.slots.get(slot_idx))
            .and_then(|slot| slot.instrument_slot.plocks.get(step))
            .and_then(|step_plocks| step_plocks.get(param_idx))
            .copied()
            .flatten()
    }

    fn rack_slot_param_plock_value(
        state: &SequencerState,
        track: usize,
        slot_idx: usize,
        step: usize,
        param: RackSlotParam,
    ) -> Option<f32> {
        state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(|rack| rack.as_ref())
            .and_then(|rack| rack.slots.get(slot_idx))
            .and_then(|slot| slot.param_plocks.get(step, param))
    }

    #[test]
    fn step_snapshot_capture_clear_restore_preserves_rack_slot_instrument_plocks() {
        let state = make_state_with_rack_slot();
        {
            let mut racks = state.pattern.rack_tracks.lock().unwrap();
            let slot = &mut racks[0].as_mut().unwrap().slots[0];
            assert!(slot.param_plocks.set(2, RackSlotParam::Gain, 0.42));
            assert!(slot.param_plocks.set(4, RackSlotParam::Gain, 0.84));
            assert!(slot.instrument_slot.set_plock(2, 0, 0.42));
            assert!(slot.instrument_slot.set_plock(4, 0, 0.84));
        }

        let snap = state.capture_step_snapshot(0, 2);
        assert_eq!(
            snap.rack_slot_param_plocks[0].params[RackSlotParam::Gain.index()],
            Some(0.42)
        );
        assert_eq!(snap.rack_slot_instrument_plocks[0].params[0], Some(0.42));

        state.clear_step_payload(0, 2);
        assert_eq!(
            rack_slot_param_plock_value(&state, 0, 0, 2, RackSlotParam::Gain),
            None
        );
        assert_eq!(
            rack_slot_param_plock_value(&state, 0, 0, 4, RackSlotParam::Gain),
            Some(0.84)
        );
        assert_eq!(rack_slot_plock_value(&state, 0, 0, 2, 0), None);
        assert_eq!(rack_slot_plock_value(&state, 0, 0, 4, 0), Some(0.84));

        state.restore_step_snapshot(0, 5, &snap);
        assert_eq!(
            rack_slot_param_plock_value(&state, 0, 0, 5, RackSlotParam::Gain),
            Some(0.42)
        );
        assert_eq!(rack_slot_plock_value(&state, 0, 0, 5, 0), Some(0.42));
    }

    #[test]
    fn sampler_runtime_arrays_cover_rack_slot_pool_domain() {
        let state = make_state_with_tracks(1);
        let last_rack_pool = crate::sequencer::rack_slot_pool_index(
            MAX_TRACKS - 1,
            crate::sequencer::MAX_RACK_SLOTS - 1,
        )
        .expect("last rack slot pool should exist");
        assert_eq!(last_rack_pool + 1, MAX_SAMPLER_POOLS);
        assert_eq!(state.runtime.sampler_lids.len(), MAX_SAMPLER_POOLS);
        assert_eq!(state.runtime.voice_counts.len(), MAX_SAMPLER_POOLS);
        assert_eq!(state.runtime.voice_lids.len(), MAX_SAMPLER_POOLS);
        assert_eq!(state.runtime.synth_node_ids.len(), MAX_SAMPLER_POOLS);
        assert_eq!(
            state.runtime.sampler_gatepitch_node_ids.len(),
            MAX_SAMPLER_POOLS
        );
        assert_eq!(
            state.runtime.sampler_modulator_node_ids.len(),
            MAX_SAMPLER_POOLS
        );

        state.runtime.sampler_lids[last_rack_pool].store(123, Ordering::Relaxed);
        state.runtime.voice_counts[last_rack_pool].store(1, Ordering::Relaxed);
        state.runtime.voice_lids[last_rack_pool][0].store(456, Ordering::Relaxed);
        state.runtime.synth_node_ids[last_rack_pool][0].store(789, Ordering::Relaxed);
        state.runtime.sampler_gatepitch_node_ids[last_rack_pool][0].store(101, Ordering::Relaxed);
        state.runtime.sampler_modulator_node_ids[last_rack_pool][0].store(112, Ordering::Relaxed);

        assert_eq!(
            state.runtime.sampler_lids[last_rack_pool].load(Ordering::Relaxed),
            123
        );
    }

    #[test]
    fn remove_track_compacts_live_state_and_runtime_bindings() {
        let state = make_state_with_tracks(3);
        let names = vec!["kick".to_string(), "snare".to_string(), "hat".to_string()];
        let buffer_ids = vec![10, 20, 30];
        let instrument_types = vec![
            InstrumentType::Sampler,
            InstrumentType::Custom,
            InstrumentType::Sampler,
        ];
        let effect_descriptors = vec![EffectDescriptor::default_full_chain(); 3];

        for track in 0..3 {
            state.pattern.track_params[track].set_num_steps(8 + track);
            state.pattern.track_params[track].set_volume(0.2 * (track + 1) as f32);
            state.pattern.track_params[track].set_accumulator_idx(track);
            state.pattern.track_params[track]
                .set_script_accumulator_name(Some(format!("acc-{track}")));
            state.pattern.track_params[track].set_accum_limit(10.0 + track as f32);
            state.pattern.track_params[track].set_fts_scale(track + 1);
            state.pattern.track_params[track].set_mute_group(track as u8);
            state.pattern.patterns[track].set_step_active(track, true);
            state.pattern.step_data[track].set(
                track,
                StepParam::Velocity,
                0.1 * (track + 1) as f32,
            );
            state.pattern.chord_data[track].add_note(track, track as f32 + 0.5);
            state.pattern.timebase_plocks[track].set(track, Timebase::Eighth);
            state.pattern.swing_plocks[track].set(track, 55.0 + track as f32);
            state.pattern.swing_resolution_plocks[track].set(track, SwingResolution::Quarter);
            state.pattern.effect_chains[track][0]
                .node_id
                .store((100 + track) as u32, Ordering::Relaxed);
            state.pattern.effect_chains[track][0]
                .num_params
                .store(1, Ordering::Relaxed);
            state.pattern.effect_chains[track][0]
                .defaults
                .set(0, track as f32 + 1.0);
            state.pattern.effect_chains[track][0].set_plock(track, 0, 300.0 + track as f32);
            state.pattern.instrument_slots[track]
                .node_id
                .store((200 + track) as u32, Ordering::Relaxed);
            state.pattern.instrument_slots[track]
                .num_params
                .store(1, Ordering::Relaxed);
            state.pattern.instrument_slots[track].set_plock(track, 0, 0.25 + track as f32);
            state.pattern.instrument_base_note_offsets[track]
                .store((track as f32 + 12.0).to_bits(), Ordering::Relaxed);
            let run_mode = if track == 2 {
                CustomInstrumentRunMode::FreePatch
            } else {
                CustomInstrumentRunMode::Instrument
            };
            state.pattern.instrument_run_modes[track]
                .store(run_mode.runtime_flag(), Ordering::Relaxed);
            state.pattern.track_sound_state.lock().unwrap()[track] = TrackSoundState {
                engine_id: Some(track),
                loaded_preset: Some(format!("preset-{track}")),
                dirty: track % 2 == 0,
            };

            state.transport.track_playheads[track].store((track * 4) as u32, Ordering::Relaxed);
            state.transport.trigger_flash[track].store((track * 10) as u32, Ordering::Relaxed);
            state.runtime.sampler_lids[track].store((track as u64) + 10, Ordering::Relaxed);
            state.runtime.pan_lids[track].store((track as u64) + 20, Ordering::Relaxed);
            state.runtime.delay_lids[track].store((track as u64) + 30, Ordering::Relaxed);
            state.runtime.send_lids[track].store((track as u64) + 40, Ordering::Relaxed);
            state.runtime.voice_counts[track].store((track + 1) as u32, Ordering::Relaxed);
            state.runtime.instrument_type_flags[track].store((track % 2) as u32, Ordering::Relaxed);
            state.runtime.instrument_run_mode_flags[track]
                .store(run_mode.runtime_flag(), Ordering::Relaxed);
            state.runtime.track_engine_ids[track].store((track as u32) + 50, Ordering::Relaxed);
            state.runtime.voice_lids[track][0].store((track as u64) + 60, Ordering::Relaxed);
            state.runtime.synth_node_ids[track][0].store((track as u32) + 70, Ordering::Relaxed);
            state.pending_accumulator_reset_tracks[track].store(track == 2, Ordering::Relaxed);
        }

        assert!(state.remove_track(
            1,
            &buffer_ids,
            &[44_100, 44_100, 44_100],
            &names,
            &instrument_types,
            &effect_descriptors
        ));

        assert_eq!(state.active_track_count(), 2);
        assert_eq!(state.pattern.track_params[1].get_num_steps(), 10);
        assert_eq!(state.pattern.track_params[1].get_volume(), 0.6);
        assert_eq!(state.pattern.track_params[1].get_accumulator_idx(), 2);
        assert_eq!(
            state.pattern.track_params[1]
                .script_accumulator_name()
                .as_deref(),
            Some("acc-2")
        );
        assert_eq!(state.pattern.track_params[1].get_accum_limit(), 12.0);
        assert_eq!(state.pattern.track_params[1].get_fts_scale(), 3);
        assert_eq!(state.pattern.track_params[1].get_mute_group(), 2);
        assert!(state.pattern.patterns[1].is_active(2));
        assert_eq!(state.pattern.step_data[1].get(2, StepParam::Velocity), 0.3);
        assert_eq!(state.pattern.chord_data[1].get(2, 0), 2.5);
        assert_eq!(
            state.pattern.timebase_plocks[1].get(2),
            Some(Timebase::Eighth)
        );
        assert_eq!(state.pattern.swing_plocks[1].get(2), Some(57.0));
        assert_eq!(
            state.pattern.swing_resolution_plocks[1].get(2),
            Some(SwingResolution::Quarter)
        );
        assert_eq!(
            state.pattern.effect_chains[1][0]
                .node_id
                .load(Ordering::Relaxed),
            102
        );
        assert_eq!(state.pattern.effect_chains[1][0].defaults.get(0), 3.0);
        assert_eq!(
            state.pattern.effect_chains[1][0].plocks.get(2, 0),
            Some(302.0)
        );
        assert_eq!(
            state.pattern.instrument_slots[1]
                .node_id
                .load(Ordering::Relaxed),
            202
        );
        assert_eq!(
            state.pattern.instrument_slots[1].plocks.get(2, 0),
            Some(2.25)
        );
        assert_eq!(
            f32::from_bits(state.pattern.instrument_base_note_offsets[1].load(Ordering::Relaxed)),
            14.0
        );
        assert_eq!(
            state.pattern.track_sound_state.lock().unwrap()[1]
                .loaded_preset
                .as_deref(),
            Some("preset-2")
        );
        assert_eq!(
            state.transport.track_playheads[1].load(Ordering::Relaxed),
            8
        );
        assert_eq!(state.transport.trigger_flash[1].load(Ordering::Relaxed), 20);
        assert_eq!(state.runtime.sampler_lids[1].load(Ordering::Relaxed), 12);
        assert_eq!(state.runtime.pan_lids[1].load(Ordering::Relaxed), 22);
        assert_eq!(state.runtime.delay_lids[1].load(Ordering::Relaxed), 32);
        assert_eq!(state.runtime.send_lids[1].load(Ordering::Relaxed), 42);
        assert_eq!(state.runtime.voice_counts[1].load(Ordering::Relaxed), 3);
        assert_eq!(
            state.runtime.instrument_type_flags[1].load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            CustomInstrumentRunMode::from_runtime_flag(
                state.pattern.instrument_run_modes[1].load(Ordering::Relaxed)
            ),
            CustomInstrumentRunMode::FreePatch
        );
        assert_eq!(
            CustomInstrumentRunMode::from_runtime_flag(
                state.runtime.instrument_run_mode_flags[1].load(Ordering::Relaxed)
            ),
            CustomInstrumentRunMode::FreePatch
        );
        assert_eq!(
            state.runtime.track_engine_ids[1].load(Ordering::Relaxed),
            52
        );
        assert_eq!(state.runtime.voice_lids[1][0].load(Ordering::Relaxed), 62);
        assert_eq!(
            state.runtime.synth_node_ids[1][0].load(Ordering::Relaxed),
            72
        );
        assert!(state.pending_accumulator_reset_tracks[1].load(Ordering::Relaxed));
    }

    #[test]
    fn remove_track_clears_old_trailing_lane() {
        let state = make_state_with_tracks(3);
        let names = vec!["kick".to_string(), "snare".to_string(), "hat".to_string()];
        let buffer_ids = vec![10, 20, 30];
        let instrument_types = vec![InstrumentType::Sampler; 3];
        let effect_descriptors = vec![EffectDescriptor::default_full_chain(); 3];

        state.pattern.patterns[2].set_step_active(0, true);
        state.pattern.step_data[2].set(0, StepParam::Velocity, 0.9);
        state.pattern.chord_data[2].add_note(0, 7.0);
        state.pattern.timebase_plocks[2].set(0, Timebase::Quarter);
        state.pattern.swing_plocks[2].set(0, 60.0);
        state.pattern.swing_resolution_plocks[2].set(0, SwingResolution::Eighth);
        state.pattern.effect_chains[2][0]
            .node_id
            .store(999, Ordering::Relaxed);
        state.pattern.effect_chains[2][0]
            .num_params
            .store(1, Ordering::Relaxed);
        state.pattern.effect_chains[2][0].set_plock(0, 0, 123.0);
        state.pattern.instrument_slots[2]
            .node_id
            .store(888, Ordering::Relaxed);
        state.pattern.instrument_slots[2]
            .num_params
            .store(1, Ordering::Relaxed);
        state.pattern.instrument_slots[2].set_plock(0, 0, 0.75);
        state.pattern.instrument_run_modes[2].store(
            CustomInstrumentRunMode::FreePatch.runtime_flag(),
            Ordering::Relaxed,
        );
        state.transport.track_playheads[2].store(12, Ordering::Relaxed);
        state.runtime.sampler_lids[2].store(77, Ordering::Relaxed);
        state.runtime.instrument_run_mode_flags[2].store(
            CustomInstrumentRunMode::FreePatch.runtime_flag(),
            Ordering::Relaxed,
        );
        state.runtime.track_engine_ids[2].store(66, Ordering::Relaxed);

        assert!(state.remove_track(
            1,
            &buffer_ids,
            &[44_100, 44_100, 44_100],
            &names,
            &instrument_types,
            &effect_descriptors
        ));

        assert!(!state.pattern.patterns[2].is_active(0));
        assert_eq!(
            state.pattern.step_data[2].get(0, StepParam::Velocity),
            StepParam::Velocity.default_value()
        );
        assert_eq!(state.pattern.chord_data[2].count(0), 0);
        assert_eq!(state.pattern.timebase_plocks[2].get(0), None);
        assert_eq!(state.pattern.swing_plocks[2].get(0), None);
        assert_eq!(state.pattern.swing_resolution_plocks[2].get(0), None);
        assert_eq!(
            state.pattern.effect_chains[2][0]
                .node_id
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(state.pattern.effect_chains[2][0].plocks.get(0, 0), None);
        assert_eq!(
            state.pattern.instrument_slots[2]
                .node_id
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(state.pattern.instrument_slots[2].plocks.get(0, 0), None);
        assert_eq!(
            state.transport.track_playheads[2].load(Ordering::Relaxed),
            0
        );
        assert_eq!(state.runtime.sampler_lids[2].load(Ordering::Relaxed), 0);
        assert_eq!(
            CustomInstrumentRunMode::from_runtime_flag(
                state.pattern.instrument_run_modes[2].load(Ordering::Relaxed)
            ),
            CustomInstrumentRunMode::Instrument
        );
        assert_eq!(
            CustomInstrumentRunMode::from_runtime_flag(
                state.runtime.instrument_run_mode_flags[2].load(Ordering::Relaxed)
            ),
            CustomInstrumentRunMode::Instrument
        );
        assert_eq!(
            state.runtime.track_engine_ids[2].load(Ordering::Relaxed),
            u32::MAX
        );
    }

    #[test]
    fn pattern_restore_preserves_live_runtime_engine_binding() {
        let state = make_state_with_tracks(1);
        state.runtime.track_engine_ids[0].store(77, Ordering::Relaxed);
        state.pattern.track_sound_state.lock().unwrap()[0] = TrackSoundState {
            engine_id: Some(12),
            loaded_preset: Some("pad".to_string()),
            dirty: false,
        };

        let snapshot = PatternSnapshot::capture(
            &state,
            1,
            &[0],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Custom],
        );

        state.runtime.track_engine_ids[0].store(91, Ordering::Relaxed);
        snapshot.restore(&state);

        assert_eq!(
            state.runtime.track_engine_ids[0].load(Ordering::Relaxed),
            91
        );
        assert_eq!(
            state.pattern.track_sound_state.lock().unwrap()[0].engine_id,
            Some(77)
        );
    }

    #[test]
    fn pattern_restore_track_only_changes_requested_track() {
        let state = make_state_with_tracks(2);
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.patterns[1].set_step_active(1, true);
        state.pattern.step_data[0].set(0, StepParam::Duration, 2.0);
        state.pattern.step_data[1].set(0, StepParam::Duration, 3.0);
        state.pattern.track_params[0].set_num_steps(5);
        state.pattern.track_params[1].set_num_steps(6);

        let mut snapshot = PatternSnapshot::new_default(2, &[]);
        snapshot.track_bits[1][0] = 1 << 7;
        snapshot.step_data[1][0][StepParam::Duration.index()] = 9.0;
        snapshot.track_params[1].num_steps = 12;
        snapshot.instrument_base_note_offsets[1] = 7.0;
        snapshot.timebase_plock_snapshots[1][0] = Some(Timebase::Eighth as u32);

        assert!(snapshot.restore_track(&state, 1));

        assert!(state.pattern.patterns[0].is_active(0));
        assert_eq!(state.pattern.step_data[0].get(0, StepParam::Duration), 2.0);
        assert_eq!(state.pattern.track_params[0].get_num_steps(), 5);

        assert!(!state.pattern.patterns[1].is_active(1));
        assert!(state.pattern.patterns[1].is_active(7));
        assert_eq!(state.pattern.step_data[1].get(0, StepParam::Duration), 9.0);
        assert_eq!(state.pattern.track_params[1].get_num_steps(), 12);
        assert_eq!(
            f32::from_bits(state.pattern.instrument_base_note_offsets[1].load(Ordering::Relaxed)),
            7.0
        );
        assert_eq!(
            state.pattern.timebase_plocks[1].get(0),
            Some(Timebase::Eighth)
        );
    }

    #[test]
    fn set_step_param_transpose_shifts_chord_notes() {
        let state = make_state_with_instrument();
        let track = 0;
        let step = 2;

        state.pattern.step_data[track].set(step, StepParam::Transpose, 7.0);
        state.pattern.chord_data[track].add_note(step, 0.0);
        state.pattern.chord_data[track].add_note(step, 4.0);

        state.set_step_param(track, step, StepParam::Transpose, 10.0);

        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Transpose),
            10.0
        );
        assert_eq!(state.pattern.chord_data[track].count(step), 2);
        assert_eq!(state.pattern.chord_data[track].get(step, 0), 3.0);
        assert_eq!(state.pattern.chord_data[track].get(step, 1), 7.0);
    }

    #[test]
    fn adjust_step_param_transpose_shifts_chord_notes() {
        let state = make_state_with_instrument();
        let track = 0;
        let step = 2;

        state.pattern.step_data[track].set(step, StepParam::Transpose, 7.0);
        state.pattern.chord_data[track].add_note(step, 0.0);
        state.pattern.chord_data[track].add_note(step, 4.0);

        state.adjust_step_param(track, step, StepParam::Transpose, -2.0);

        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Transpose),
            5.0
        );
        assert_eq!(state.pattern.chord_data[track].count(step), 2);
        assert_eq!(state.pattern.chord_data[track].get(step, 0), -2.0);
        assert_eq!(state.pattern.chord_data[track].get(step, 1), 2.0);
    }

    // ── copy / paste (capture_step_snapshot + restore_step_snapshot) ──

    #[test]
    fn copy_paste_preserves_all_fields() {
        let state = make_state_with_instrument();
        populate_step(&state, 0, 2);

        let snap = state.capture_step_snapshot(0, 2);
        state.restore_step_snapshot(0, 5, &snap);

        assert_step_matches_populated(&state, 0, 5);
        // Source step is unchanged
        assert_step_matches_populated(&state, 0, 2);
    }

    #[test]
    fn copy_paste_multi_step_with_offsets() {
        // Simulates Ctrl+C on steps 1,2 then Ctrl+V at step 4.
        let state = make_state_with_instrument();
        populate_step(&state, 0, 1);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.3);

        let anchor = 1usize;
        let clipboard: Vec<(usize, StepSnapshot)> = [1usize, 2]
            .iter()
            .map(|&s| (s - anchor, state.capture_step_snapshot(0, s)))
            .collect();

        let dest_start = 4usize;
        for (offset, snap) in &clipboard {
            state.restore_step_snapshot(0, dest_start + offset, snap);
        }

        // Step 4 (offset 0) should match original step 1
        assert_step_matches_populated(&state, 0, 4);
        // Step 5 (offset 1) should match original step 2
        assert!(state.pattern.patterns[0].is_active(5));
        assert_eq!(state.pattern.step_data[0].get(5, StepParam::Velocity), 0.3);
    }

    #[test]
    fn paste_inactive_snapshot_over_active_step_preserves_existing() {
        // An "empty" snapshot must not overwrite an active step.
        let state = make_state_with_instrument();
        populate_step(&state, 0, 3);

        let empty_snap = state.capture_step_snapshot(0, 7); // step 7 is default/inactive
        assert!(!empty_snap.active);

        // Simulate the paste guard from Ctrl+V: skip if snapshot inactive and dest active
        let dest = 3usize;
        if !empty_snap.active && state.pattern.patterns[0].is_active(dest) {
            // correctly skipped
        } else {
            state.restore_step_snapshot(0, dest, &empty_snap);
            panic!("should not overwrite active step with empty snapshot");
        }

        assert_step_matches_populated(&state, 0, 3);
    }

    #[test]
    fn paste_active_snapshot_over_empty_step_writes_data() {
        let state = make_state_with_instrument();
        populate_step(&state, 0, 1);

        let snap = state.capture_step_snapshot(0, 1);
        assert!(snap.active);

        // Dest step 5 is empty — paste guard should allow the write
        let dest = 5usize;
        assert!(!state.pattern.patterns[0].is_active(dest));
        // Guard passes (snap.active == true), so we restore
        state.restore_step_snapshot(0, dest, &snap);

        assert_step_matches_populated(&state, 0, 5);
    }

    #[test]
    fn paste_out_of_bounds_offsets_are_skipped() {
        let state = make_state_with_instrument();
        populate_step(&state, 0, 0);
        let ns = state.pattern.track_params[0].get_num_steps(); // 8

        let snap = state.capture_step_snapshot(0, 0);
        // dest_start=6, offsets 0..4 → destinations 6,7,8,9; 8 and 9 exceed ns
        let dest_start = 6usize;
        for offset in 0..4 {
            let dest = dest_start + offset;
            if dest >= ns {
                continue; // bounds guard — no write, no panic
            }
            state.restore_step_snapshot(0, dest, &snap);
        }

        assert!(state.pattern.patterns[0].is_active(6));
        assert!(state.pattern.patterns[0].is_active(7));
    }

    // ── rotate_steps ──

    #[test]
    fn rotate_steps_left_wraps_first_to_last() {
        // A B C _ at steps 0,1,2,3  →  B C _ A
        let state = make_state_with_instrument();
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 1.0);
        state.pattern.patterns[0].set_step_active(1, true);
        state.pattern.step_data[0].set(1, StepParam::Transpose, 2.0);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Transpose, 3.0);
        // step 3 stays empty

        state.rotate_steps(0, &[0, 1, 2, 3], -1);

        assert!(state.pattern.patterns[0].is_active(0));
        assert_eq!(state.pattern.step_data[0].get(0, StepParam::Transpose), 2.0);
        assert!(state.pattern.patterns[0].is_active(1));
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Transpose), 3.0);
        assert!(!state.pattern.patterns[0].is_active(2)); // formerly empty step 3
        assert!(state.pattern.patterns[0].is_active(3));
        assert_eq!(state.pattern.step_data[0].get(3, StepParam::Transpose), 1.0);
    }

    #[test]
    fn rotate_steps_right_wraps_last_to_first() {
        // A B C _ at steps 0,1,2,3  →  _ A B C
        let state = make_state_with_instrument();
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 1.0);
        state.pattern.patterns[0].set_step_active(1, true);
        state.pattern.step_data[0].set(1, StepParam::Transpose, 2.0);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Transpose, 3.0);
        // step 3 stays empty

        state.rotate_steps(0, &[0, 1, 2, 3], 1);

        assert!(!state.pattern.patterns[0].is_active(0)); // formerly empty step 3
        assert!(state.pattern.patterns[0].is_active(1));
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Transpose), 1.0);
        assert!(state.pattern.patterns[0].is_active(2));
        assert_eq!(state.pattern.step_data[0].get(2, StepParam::Transpose), 2.0);
        assert!(state.pattern.patterns[0].is_active(3));
        assert_eq!(state.pattern.step_data[0].get(3, StepParam::Transpose), 3.0);
    }

    #[test]
    fn rotate_steps_preserves_plocks_and_chords() {
        // step 0 has full data; step 1 is empty. Rotate left: step 1 gets step 0's data.
        let state = make_state_with_instrument();
        populate_step(&state, 0, 0);

        state.rotate_steps(0, &[0, 1], -1);

        assert_step_is_default(&state, 0, 0);
        assert_step_matches_populated(&state, 0, 1);
    }

    #[test]
    fn rotate_steps_two_left_equals_rotate_by_two() {
        // A B C → (left) → B C A → (left) → C A B
        let state = make_state_with_instrument();
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 10.0);
        state.pattern.patterns[0].set_step_active(1, true);
        state.pattern.step_data[0].set(1, StepParam::Transpose, 20.0);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Transpose, 30.0);

        state.rotate_steps(0, &[0, 1, 2], -1);
        state.rotate_steps(0, &[0, 1, 2], -1);

        assert_eq!(
            state.pattern.step_data[0].get(0, StepParam::Transpose),
            30.0
        );
        assert_eq!(
            state.pattern.step_data[0].get(1, StepParam::Transpose),
            10.0
        );
        assert_eq!(
            state.pattern.step_data[0].get(2, StepParam::Transpose),
            20.0
        );
    }

    // ── clear_step_payload ──

    #[test]
    fn clear_step_payload_removes_all_data_including_plocks() {
        let state = make_state_with_instrument();
        populate_step(&state, 0, 3);

        state.clear_step_payload(0, 3);

        assert_step_is_default(&state, 0, 3);
    }

    #[test]
    fn clear_step_payload_on_inactive_step_is_safe() {
        let state = make_state_with_instrument();
        // step 4 was never populated — clearing it should not panic
        state.clear_step_payload(0, 4);
        assert_step_is_default(&state, 0, 4);
    }

    #[test]
    fn published_scheduler_snapshot_reflects_initial_state() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );

        let snapshot = state.latest_scheduler_snapshot();

        assert_eq!(state.scheduler_snapshot_version(), 1);
        assert_eq!(snapshot.transport.num_tracks, 2);
        assert_eq!(snapshot.transport.bpm, DEFAULT_BPM);
        assert_eq!(snapshot.tracks.len(), 2);
        assert_eq!(snapshot.tracks[0].params.num_steps, 16);
    }

    #[test]
    fn transport_starts_stopped_and_toggle_resets_playheads() {
        let state = SequencerState::new(2, vec![default_empty_effect_chain()]);

        assert!(!state.is_playing());
        assert!(!state.latest_scheduler_snapshot().transport.playing);

        state.transport.playhead.store(9, Ordering::Relaxed);
        state.transport.track_playheads[0].store(3, Ordering::Relaxed);
        state.transport.track_playheads[1].store(7, Ordering::Relaxed);
        state.transport.sampler_playheads[0].store(0.5_f32.to_bits(), Ordering::Relaxed);

        assert!(state.toggle_play());
        assert!(state.is_playing());
        assert!(state.latest_scheduler_snapshot().transport.playing);
        assert_eq!(state.transport.playhead.load(Ordering::Relaxed), 0);
        assert_eq!(
            state.transport.track_playheads[0].load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            state.transport.track_playheads[1].load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            f32::from_bits(state.transport.sampler_playheads[0].load(Ordering::Relaxed)),
            0.0
        );

        state.transport.playhead.store(4, Ordering::Relaxed);
        state.transport.track_playheads[0].store(2, Ordering::Relaxed);

        assert!(!state.toggle_play());
        assert!(!state.is_playing());
        assert!(!state.latest_scheduler_snapshot().transport.playing);
        assert_eq!(state.transport.playhead.load(Ordering::Relaxed), 0);
        assert_eq!(
            state.transport.track_playheads[0].load(Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn published_scheduler_snapshot_updates_on_step_mutation() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let before = state.scheduler_snapshot_version();

        state.toggle_step_and_clear_plocks(0, 3);
        state.set_step_param(0, 3, StepParam::Transpose, 9.0);

        let snapshot = state.latest_scheduler_snapshot();
        assert!(state.scheduler_snapshot_version() > before);
        assert!(snapshot.tracks[0].steps[3].active);
        assert_eq!(
            snapshot.tracks[0].steps[3].params[StepParam::Transpose.index()],
            9.0
        );
    }

    #[test]
    fn publish_scheduler_snapshot_captures_transport_changes() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);

        state.transport.bpm.store(172, Ordering::Relaxed);
        state.publish_scheduler_snapshot();

        let snapshot = state.latest_scheduler_snapshot();
        assert_eq!(snapshot.transport.bpm, 172);
    }

    #[test]
    fn publish_scheduler_snapshot_captures_current_pattern_mod_connections() {
        let state = make_state_with_tracks(2);
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 2,
        };
        state
            .edit_current_mod_connections(|routes| {
                routes.push(route);
                Ok(())
            })
            .unwrap();

        state.publish_scheduler_snapshot();

        let snapshot = state.latest_scheduler_snapshot();
        assert_eq!(snapshot.mod_connections, vec![route]);
    }

    #[test]
    fn accumulator_reset_requests_are_consumed_once() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );

        state.request_accumulator_reset(1);
        state.request_all_accumulator_resets();

        let (all, tracks) = state.take_accumulator_reset_requests();
        assert!(all);
        assert!(tracks[1]);

        let (all_again, tracks_again) = state.take_accumulator_reset_requests();
        assert!(!all_again);
        assert!(!tracks_again[1]);
    }

    #[test]
    fn default_empty_effect_chain_has_no_builtin_nodes() {
        let chain = default_empty_effect_chain();
        assert_eq!(chain.len(), crate::lisp_host::MAX_CUSTOM_FX);
        for slot in chain {
            assert_eq!(slot.node_id.load(Ordering::Relaxed), 0);
            assert_eq!(slot.num_params.load(Ordering::Relaxed), 0);
        }
    }
}
