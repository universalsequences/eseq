use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::effects::{EffectDescriptor, EffectSlotSnapshot, EffectSlotState, MAX_SLOT_PARAMS};
use crate::voice::MAX_VOICES;

use super::data::{
    ChordData, ChordSnapshot, InstrumentType, StepData, StepParam, Timebase, TimebasePLockData,
    TrackParams, TrackParamsSnapshot, TrackPattern, TrackSoundState, DEFAULT_BPM, MAX_STEPS,
    MAX_TRACKS, NUM_PARAMS, TRACK_PATTERN_WORDS,
};
use super::snapshot::SequencerSnapshot;

#[derive(Clone)]
pub struct StepSlotPlocks {
    pub params: Vec<Option<f32>>,
}

#[derive(Clone)]
pub struct StepSnapshot {
    pub active: bool,
    pub params: [f32; NUM_PARAMS],
    pub chord: Vec<f32>,
    pub timebase: Option<Timebase>,
    pub effect_plocks: Vec<StepSlotPlocks>,
    pub instrument_plocks: StepSlotPlocks,
}

#[derive(Clone)]
pub struct PatternSnapshot {
    pub track_bits: Vec<[u64; TRACK_PATTERN_WORDS]>,
    pub step_data: Vec<Vec<[f32; NUM_PARAMS]>>,
    pub track_params: Vec<TrackParamsSnapshot>,
    pub effect_slots: Vec<Vec<EffectSlotSnapshot>>,
    pub instrument_slots: Vec<EffectSlotSnapshot>,
    pub instrument_base_note_offsets: Vec<f32>,
    pub track_sound_states: Vec<TrackSoundState>,
    pub sample_ids: Vec<(i32, String)>,
    pub chord_snapshots: Vec<ChordSnapshot>,
    pub timebase_plock_snapshots: Vec<[Option<u32>; MAX_STEPS]>,
    pub instrument_types: Vec<InstrumentType>,
}

impl PatternSnapshot {
    pub fn capture(
        state: &SequencerState,
        num_tracks: usize,
        track_buffer_ids: &[i32],
        track_names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Self {
        let mut track_bits = Vec::with_capacity(num_tracks);
        let mut step_data = Vec::with_capacity(num_tracks);
        let mut track_params = Vec::with_capacity(num_tracks);
        let mut effect_slots = Vec::with_capacity(num_tracks);
        let mut instrument_slots = Vec::with_capacity(num_tracks);
        let mut instrument_base_note_offsets = Vec::with_capacity(num_tracks);
        let track_sound_state = state.pattern.track_sound_state.lock().unwrap();
        let mut sound_states = Vec::with_capacity(num_tracks);
        let mut sample_ids = Vec::with_capacity(num_tracks);
        let mut chord_snapshots = Vec::with_capacity(num_tracks);
        let mut timebase_plock_snapshots = Vec::with_capacity(num_tracks);
        let mut inst_types = Vec::with_capacity(num_tracks);

        for t in 0..num_tracks {
            track_bits.push(state.pattern.patterns[t].load_bits());

            let mut steps = Vec::with_capacity(MAX_STEPS);
            for s in 0..MAX_STEPS {
                let mut params = [0.0f32; NUM_PARAMS];
                for p in StepParam::ALL {
                    params[p.index()] = state.pattern.step_data[t].get(s, p);
                }
                steps.push(params);
            }
            step_data.push(steps);

            let tp = &state.pattern.track_params[t];
            track_params.push(TrackParamsSnapshot {
                gate: tp.is_gate_on(),
                attack_ms: tp.get_attack_ms(),
                release_ms: tp.get_release_ms(),
                swing: tp.get_swing(),
                swing_resolution: tp.get_swing_resolution(),
                num_steps: tp.get_num_steps(),
                volume: tp.get_volume(),
                pan: tp.get_pan(),
                send: tp.get_send(),
                polyphonic: tp.is_polyphonic(),
                timebase: tp.get_timebase(),
                accumulator_idx: tp.get_accumulator_idx(),
                script_accumulator_name: tp.script_accumulator_name(),
                accum_limit: tp.get_accum_limit(),
                accum_mode: tp.get_accum_mode(),
                fts_scale: tp.get_fts_scale(),
            });

            let chain: Vec<EffectSlotSnapshot> = state.pattern.effect_chains[t]
                .iter()
                .map(EffectSlotSnapshot::capture)
                .collect();
            effect_slots.push(chain);
            instrument_slots.push(EffectSlotSnapshot::capture(
                &state.pattern.instrument_slots[t],
            ));
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
            sample_ids.push((buf_id, name));
            chord_snapshots.push(ChordSnapshot::capture(&state.pattern.chord_data[t]));
            timebase_plock_snapshots.push(state.pattern.timebase_plocks[t].snapshot());
            inst_types.push(if t < instrument_types.len() {
                instrument_types[t]
            } else {
                InstrumentType::Sampler
            });
        }

        Self {
            track_bits,
            step_data,
            track_params,
            effect_slots,
            instrument_slots,
            instrument_base_note_offsets,
            track_sound_states: sound_states,
            sample_ids,
            chord_snapshots,
            timebase_plock_snapshots,
            instrument_types: inst_types,
        }
    }

    pub fn restore(&self, state: &SequencerState) {
        let num_tracks = self.track_bits.len();
        let mut track_sound_state = state.pattern.track_sound_state.lock().unwrap();
        for t in 0..num_tracks {
            state.pattern.patterns[t].store_bits(self.track_bits[t]);

            for s in 0..MAX_STEPS {
                for p in StepParam::ALL {
                    state.pattern.step_data[t].set(s, p, self.step_data[t][s][p.index()]);
                }
            }

            let tp = &state.pattern.track_params[t];
            let snap = &self.track_params[t];
            tp.gate.store(snap.gate, Ordering::Relaxed);
            tp.set_attack_ms(snap.attack_ms);
            tp.set_release_ms(snap.release_ms);
            tp.set_swing(snap.swing);
            tp.set_swing_resolution(snap.swing_resolution);
            tp.set_num_steps(snap.num_steps);
            tp.set_volume(snap.volume);
            tp.set_pan(snap.pan);
            tp.set_send(snap.send);
            tp.polyphonic.store(snap.polyphonic, Ordering::Relaxed);
            tp.set_timebase(snap.timebase);
            tp.set_accumulator_idx(snap.accumulator_idx);
            tp.set_script_accumulator_name(snap.script_accumulator_name.clone());
            tp.set_accum_limit(snap.accum_limit);
            tp.set_accum_mode(snap.accum_mode);
            tp.set_fts_scale(snap.fts_scale);

            for (slot_idx, slot_snap) in self.effect_slots[t].iter().enumerate() {
                if slot_idx < state.pattern.effect_chains[t].len() {
                    slot_snap.restore(&state.pattern.effect_chains[t][slot_idx]);
                }
            }

            if t < self.instrument_slots.len() {
                self.instrument_slots[t].restore(&state.pattern.instrument_slots[t]);
            }
            if t < self.instrument_base_note_offsets.len() {
                state.pattern.instrument_base_note_offsets[t].store(
                    self.instrument_base_note_offsets[t].to_bits(),
                    Ordering::Relaxed,
                );
            }
            if t < self.track_sound_states.len() && t < track_sound_state.len() {
                track_sound_state[t] = self.track_sound_states[t].clone();
                let engine_id = self.track_sound_states[t]
                    .engine_id
                    .map(|id| id as u32)
                    .unwrap_or(u32::MAX);
                state.runtime.track_engine_ids[t].store(engine_id, Ordering::Relaxed);
            }

            if t < self.chord_snapshots.len() {
                self.chord_snapshots[t].restore(&state.pattern.chord_data[t]);
            }
            if t < self.timebase_plock_snapshots.len() {
                state.pattern.timebase_plocks[t].restore(&self.timebase_plock_snapshots[t]);
            }
        }
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

    fn push_default_track(&mut self, t: usize, slot_descriptors: &[Vec<EffectDescriptor>]) {
        self.track_bits.push([0u64; TRACK_PATTERN_WORDS]);
        self.step_data.push(Self::default_step_data());
        self.track_params.push(TrackParamsSnapshot::default());
        self.effect_slots
            .push(Self::default_effect_slots(t, slot_descriptors));
        self.instrument_slots.push(Self::default_instrument_slot());
        self.instrument_base_note_offsets.push(0.0);
        self.track_sound_states.push(TrackSoundState::default());
        self.sample_ids.push((-1, String::new()));
        self.chord_snapshots.push(ChordSnapshot::new_default());
        self.timebase_plock_snapshots.push([None; MAX_STEPS]);
        self.instrument_types.push(InstrumentType::Sampler);
    }

    pub fn new_default(num_tracks: usize, slot_descriptors: &[Vec<EffectDescriptor>]) -> Self {
        let mut snap = Self {
            track_bits: Vec::with_capacity(num_tracks),
            step_data: Vec::with_capacity(num_tracks),
            track_params: Vec::with_capacity(num_tracks),
            effect_slots: Vec::with_capacity(num_tracks),
            instrument_slots: Vec::with_capacity(num_tracks),
            instrument_base_note_offsets: Vec::with_capacity(num_tracks),
            track_sound_states: Vec::with_capacity(num_tracks),
            sample_ids: Vec::with_capacity(num_tracks),
            chord_snapshots: Vec::with_capacity(num_tracks),
            timebase_plock_snapshots: Vec::with_capacity(num_tracks),
            instrument_types: Vec::with_capacity(num_tracks),
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
        while self.track_bits.len() < new_count {
            let t = self.track_bits.len();
            self.push_default_track(t, slot_descriptors);
        }
    }

    pub fn sync_effect_slot(
        &mut self,
        track: usize,
        slot_idx: usize,
        desc: &EffectDescriptor,
        node_id: u32,
    ) {
        while self.effect_slots.len() <= track {
            self.push_default_track(track, &[]);
        }
        while self.effect_slots[track].len() <= slot_idx {
            self.effect_slots[track].push(EffectSlotSnapshot::new_empty());
        }
        self.effect_slots[track][slot_idx].sync_to_descriptor(desc, node_id);
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

pub fn default_empty_effect_chain() -> Vec<EffectSlotState> {
    use crate::lisp_effect::MAX_CUSTOM_FX;
    let filter_desc = EffectDescriptor::builtin_filter();
    let delay_desc = EffectDescriptor::builtin_delay();
    let filter_slot = EffectSlotState::new(&filter_desc, 0);
    let delay_slot = EffectSlotState::new(&delay_desc, 0);
    let mut chain = vec![filter_slot, delay_slot];
    for _ in 0..MAX_CUSTOM_FX {
        chain.push(EffectSlotState::empty());
    }
    chain
}

pub struct PatternState {
    pub patterns: Vec<TrackPattern>,
    pub step_data: Vec<StepData>,
    pub chord_data: Vec<ChordData>,
    pub track_params: Vec<TrackParams>,
    pub effect_chains: Vec<Vec<EffectSlotState>>,
    pub pattern_bank: Mutex<Vec<PatternSnapshot>>,
    pub current_pattern: AtomicU32,
    pub num_patterns: AtomicU32,
    pub timebase_plocks: Vec<TimebasePLockData>,
    pub instrument_slots: Vec<EffectSlotState>,
    pub instrument_base_note_offsets: Vec<AtomicU32>,
    pub track_sound_state: Mutex<Vec<TrackSoundState>>,
}

pub struct TransportState {
    pub playhead: AtomicU32,
    pub playing: AtomicBool,
    pub bpm: AtomicU32,
    pub master_volume: AtomicU32,
    pub pattern_epoch: AtomicU64,
    pub mod_reset_counter: AtomicU32,
    pub pending_mod_resync: AtomicBool,
    pub peak_l: AtomicU32,
    pub peak_r: AtomicU32,
    pub cpu_load_pct: AtomicU32,
    pub trigger_flash: Vec<AtomicU32>,
    pub num_tracks: AtomicU32,
    pub track_playheads: Vec<AtomicU32>,
    pub playhead_phase: AtomicU32,
    pub record_quantize_thresh: AtomicU32,
}

pub struct RuntimeBindingState {
    pub sampler_lids: Vec<AtomicU64>,
    pub pan_lids: Vec<AtomicU64>,
    pub delay_lids: Vec<AtomicU64>,
    pub send_lids: Vec<AtomicU64>,
    pub voice_lids: Vec<[AtomicU64; MAX_VOICES]>,
    pub voice_counts: Vec<AtomicU32>,
    pub instrument_type_flags: Vec<AtomicU32>,
    pub synth_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub track_engine_ids: Vec<AtomicU32>,
    pub engine_voice_lids: Vec<[AtomicU64; MAX_VOICES]>,
    pub engine_synth_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub engine_modulator_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub engine_voice_counts: Vec<AtomicU32>,
    pub engine_route_lids: Vec<[[AtomicU64; MAX_TRACKS]; MAX_VOICES]>,
    pub engine_route_lids_r: Vec<[[AtomicU64; MAX_TRACKS]; MAX_VOICES]>,
}

pub struct SequencerState {
    pub pattern: PatternState,
    pub transport: TransportState,
    pub runtime: RuntimeBindingState,
    scheduler_snapshot: Mutex<Arc<SequencerSnapshot>>,
    scheduler_snapshot_version: AtomicU64,
    scratch_source: Mutex<String>,
    scratch_source_version: AtomicU64,
    scratch_effect_descriptors: Mutex<Vec<Vec<EffectDescriptor>>>,
    scratch_instrument_descriptors: Mutex<Vec<EffectDescriptor>>,
    pending_accumulator_reset_all: AtomicBool,
    pending_accumulator_reset_tracks: [AtomicBool; MAX_TRACKS],
}

impl SequencerState {
    pub fn new(num_tracks: usize, initial_chains: Vec<Vec<EffectSlotState>>) -> Self {
        let patterns: Vec<TrackPattern> = (0..MAX_TRACKS).map(|_| TrackPattern::new()).collect();
        let step_data: Vec<StepData> = (0..MAX_TRACKS).map(|_| StepData::new()).collect();
        let track_params: Vec<TrackParams> = (0..MAX_TRACKS).map(|_| TrackParams::new()).collect();
        let trigger_flash: Vec<AtomicU32> = (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect();

        let mut effect_chains = initial_chains;
        for _ in effect_chains.len()..MAX_TRACKS {
            effect_chains.push(default_empty_effect_chain());
        }

        let slot_descriptors: Vec<Vec<EffectDescriptor>> = (0..num_tracks)
            .map(|_| EffectDescriptor::default_full_chain())
            .collect();

        let chord_data: Vec<ChordData> = (0..MAX_TRACKS).map(|_| ChordData::new()).collect();

        let state = Self {
            pattern: PatternState {
                patterns,
                step_data,
                chord_data,
                track_params,
                effect_chains,
                pattern_bank: Mutex::new(vec![PatternSnapshot::new_default(
                    num_tracks,
                    &slot_descriptors,
                )]),
                current_pattern: AtomicU32::new(0),
                num_patterns: AtomicU32::new(1),
                timebase_plocks: (0..MAX_TRACKS).map(|_| TimebasePLockData::new()).collect(),
                instrument_slots: (0..MAX_TRACKS).map(|_| EffectSlotState::empty()).collect(),
                instrument_base_note_offsets: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                track_sound_state: Mutex::new(
                    (0..MAX_TRACKS)
                        .map(|_| TrackSoundState::default())
                        .collect(),
                ),
            },
            transport: TransportState {
                playhead: AtomicU32::new(0),
                playing: AtomicBool::new(false),
                bpm: AtomicU32::new(DEFAULT_BPM),
                master_volume: AtomicU32::new(1.0_f32.to_bits()),
                pattern_epoch: AtomicU64::new(0),
                mod_reset_counter: AtomicU32::new(0),
                pending_mod_resync: AtomicBool::new(false),
                peak_l: AtomicU32::new(0.0_f32.to_bits()),
                peak_r: AtomicU32::new(0.0_f32.to_bits()),
                cpu_load_pct: AtomicU32::new(0.0_f32.to_bits()),
                trigger_flash,
                num_tracks: AtomicU32::new(num_tracks as u32),
                track_playheads: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                playhead_phase: AtomicU32::new(0.0_f32.to_bits()),
                record_quantize_thresh: AtomicU32::new(0.5_f32.to_bits()),
            },
            runtime: RuntimeBindingState {
                sampler_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                pan_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                delay_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                send_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                voice_lids: (0..MAX_TRACKS)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                voice_counts: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                instrument_type_flags: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                synth_node_ids: (0..MAX_TRACKS)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                track_engine_ids: (0..MAX_TRACKS).map(|_| AtomicU32::new(u32::MAX)).collect(),
                engine_voice_lids: (0..MAX_TRACKS)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                engine_synth_node_ids: (0..MAX_TRACKS)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                engine_modulator_node_ids: (0..MAX_TRACKS)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                engine_voice_counts: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                engine_route_lids: (0..MAX_TRACKS)
                    .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))))
                    .collect(),
                engine_route_lids_r: (0..MAX_TRACKS)
                    .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))))
                    .collect(),
            },
            scheduler_snapshot: Mutex::new(Arc::new(SequencerSnapshot::empty())),
            scheduler_snapshot_version: AtomicU64::new(0),
            scratch_source: Mutex::new(String::new()),
            scratch_source_version: AtomicU64::new(0),
            scratch_effect_descriptors: Mutex::new(Vec::new()),
            scratch_instrument_descriptors: Mutex::new(Vec::new()),
            pending_accumulator_reset_all: AtomicBool::new(false),
            pending_accumulator_reset_tracks: std::array::from_fn(|_| AtomicBool::new(false)),
        };
        state.publish_scheduler_snapshot();
        state
    }

    pub fn active_track_count(&self) -> usize {
        self.transport.num_tracks.load(Ordering::Acquire) as usize
    }
    pub fn scheduler_snapshot_version(&self) -> u64 {
        self.scheduler_snapshot_version.load(Ordering::Acquire)
    }
    pub fn latest_scheduler_snapshot(&self) -> Arc<SequencerSnapshot> {
        self.scheduler_snapshot.lock().unwrap().clone()
    }
    pub fn publish_scheduler_snapshot(&self) -> Arc<SequencerSnapshot> {
        let snapshot = Arc::new(SequencerSnapshot::capture(self));
        {
            let mut published = self.scheduler_snapshot.lock().unwrap();
            *published = Arc::clone(&snapshot);
        }
        self.scheduler_snapshot_version.fetch_add(1, Ordering::AcqRel);
        snapshot
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
    pub fn scratch_runtime_descriptors(&self) -> (Vec<Vec<EffectDescriptor>>, Vec<EffectDescriptor>) {
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
        *self.scratch_effect_descriptors.lock().unwrap() = effect_descriptors;
        *self.scratch_instrument_descriptors.lock().unwrap() = instrument_descriptors;
    }
    pub fn request_accumulator_reset(&self, track: usize) {
        if track < MAX_TRACKS {
            self.pending_accumulator_reset_tracks[track].store(true, Ordering::Release);
        }
    }
    pub fn request_all_accumulator_resets(&self) {
        self.pending_accumulator_reset_all.store(true, Ordering::Release);
    }
    pub fn take_accumulator_reset_requests(&self) -> (bool, [bool; MAX_TRACKS]) {
        let all = self.pending_accumulator_reset_all.swap(false, Ordering::AcqRel);
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
    pub fn toggle_play(&self) {
        self.transport.playing.fetch_xor(true, Ordering::Relaxed);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
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

    pub fn switch_pattern(
        &self,
        new_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String)>> {
        let mut bank = self.pattern.pattern_bank.lock().unwrap();
        let cur = self.pattern.current_pattern.load(Ordering::Relaxed) as usize;
        if new_idx == cur || new_idx >= bank.len() {
            return None;
        }
        bank[cur] = PatternSnapshot::capture(self, num_tracks, buffer_ids, names, instrument_types);
        bank[new_idx].restore(self);
        self.pattern
            .current_pattern
            .store(new_idx as u32, Ordering::Relaxed);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.publish_scheduler_snapshot();
        Some(bank[new_idx].sample_ids.clone())
    }

    pub fn clone_pattern(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> usize {
        let mut bank = self.pattern.pattern_bank.lock().unwrap();
        let cur = self.pattern.current_pattern.load(Ordering::Relaxed) as usize;
        bank[cur] = PatternSnapshot::capture(self, num_tracks, buffer_ids, names, instrument_types);
        let cloned = bank[cur].clone();
        bank.push(cloned);
        let new_idx = bank.len() - 1;
        self.pattern
            .current_pattern
            .store(new_idx as u32, Ordering::Relaxed);
        self.pattern
            .num_patterns
            .store(bank.len() as u32, Ordering::Relaxed);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        new_idx
    }

    pub fn delete_pattern(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String)>> {
        let mut bank = self.pattern.pattern_bank.lock().unwrap();
        if bank.len() <= 1 {
            return None;
        }
        let cur = self.pattern.current_pattern.load(Ordering::Relaxed) as usize;
        bank[cur] = PatternSnapshot::capture(self, num_tracks, buffer_ids, names, instrument_types);
        bank.remove(cur);
        let new_idx = cur.min(bank.len() - 1);
        bank[new_idx].restore(self);
        self.pattern
            .current_pattern
            .store(new_idx as u32, Ordering::Relaxed);
        self.pattern
            .num_patterns
            .store(bank.len() as u32, Ordering::Relaxed);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.publish_scheduler_snapshot();
        Some(bank[new_idx].sample_ids.clone())
    }

    pub fn toggle_step_and_clear_plocks(&self, track: usize, step: usize) {
        let was_active = self.pattern.patterns[track].is_active(step);
        self.pattern.patterns[track].toggle_step(step);
        if was_active {
            for slot in &self.pattern.effect_chains[track] {
                slot.plocks.clear_step(step);
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
        for note_idx in 0..chord_count {
            chord.push(self.pattern.chord_data[track].get(step, note_idx));
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

        StepSnapshot {
            active: self.pattern.patterns[track].is_active(step),
            params,
            chord,
            timebase: self.pattern.timebase_plocks[track].get(step),
            effect_plocks,
            instrument_plocks: StepSlotPlocks {
                params: instrument_plocks,
            },
        }
    }

    fn clear_step_payload_inner(&self, track: usize, step: usize) {
        for param in StepParam::ALL {
            self.pattern.step_data[track].set(step, param, param.default_value());
        }

        self.pattern.patterns[track].clear_step(step);

        self.pattern.chord_data[track].clear_step(step);
        self.pattern.timebase_plocks[track].clear(step);

        for slot in &self.pattern.effect_chains[track] {
            slot.plocks.clear_step(step);
        }

        for param_idx in 0..MAX_SLOT_PARAMS {
            self.pattern.instrument_slots[track]
                .plocks
                .clear_param(step, param_idx);
        }
    }

    pub fn clear_step_payload(&self, track: usize, step: usize) {
        self.clear_step_payload_inner(track, step);
        self.publish_scheduler_snapshot();
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
            notes.push(self.pattern.chord_data[track].get(step, note_idx) + delta);
        }
        self.pattern.chord_data[track].clear_step(step);
        for transpose in notes {
            self.pattern.chord_data[track].add_note(step, transpose);
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

        self.pattern.chord_data[track].clear_step(step);
        for &transpose in &snapshot.chord {
            self.pattern.chord_data[track].add_note(step, transpose);
        }

        match snapshot.timebase {
            Some(tb) => self.pattern.timebase_plocks[track].set(step, tb),
            None => self.pattern.timebase_plocks[track].clear(step),
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
                    Some(val) => slot.plocks.set(step, param_idx, val),
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
                Some(val) => instrument_slot.plocks.set(step, param_idx, val),
                None => instrument_slot.plocks.clear_param(step, param_idx),
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
                        Some(val) => slot.plocks.set(step, p, val),
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

#[cfg(test)]
mod tests {
    use super::*;

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
        state.pattern.effect_chains[0][0].plocks.set(1, 2, 440.0);
        state.pattern.instrument_slots[0].plocks.set(1, 0, 0.75);

        state.pattern.patterns[0].toggle_step(2);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.3);
        state.pattern.chord_data[0].add_note(2, 12.0);
        state.pattern.timebase_plocks[0].set(2, Timebase::QuarterTriplet);
        state.pattern.effect_chains[0][0].plocks.set(2, 2, 880.0);
        state.pattern.instrument_slots[0].plocks.set(2, 0, 0.25);

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

    fn populate_step(state: &SequencerState, track: usize, step: usize) {
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Velocity, 0.75);
        state.pattern.step_data[track].set(step, StepParam::Transpose, 7.0);
        state.pattern.chord_data[track].add_note(step, 0.0);
        state.pattern.chord_data[track].add_note(step, 4.0);
        state.pattern.timebase_plocks[track].set(step, Timebase::Eighth);
        state.pattern.effect_chains[track][0]
            .plocks
            .set(step, 0, 440.0);
        state.pattern.instrument_slots[track]
            .plocks
            .set(step, 0, 0.5);
    }

    fn assert_step_matches_populated(state: &SequencerState, track: usize, step: usize) {
        assert!(
            state.pattern.patterns[track].is_active(step),
            "step {step} should be active"
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
    fn accumulator_reset_requests_are_consumed_once() {
        let state = SequencerState::new(2, vec![default_empty_effect_chain(), default_empty_effect_chain()]);

        state.request_accumulator_reset(1);
        state.request_all_accumulator_resets();

        let (all, tracks) = state.take_accumulator_reset_requests();
        assert!(all);
        assert!(tracks[1]);

        let (all_again, tracks_again) = state.take_accumulator_reset_requests();
        assert!(!all_again);
        assert!(!tracks_again[1]);
    }
}
