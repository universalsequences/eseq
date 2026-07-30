//! Sound pool entities (takes spec §17.2): the Patch/Mix halves of what was
//! `TrackPatternData`'s inlined device state, held in per-track pools and
//! referenced from pool patterns, scene cells, and takes.
//!
//! Phase S1 (§18.1) is behavior-identical: every entity has exactly one
//! logical referent (a pattern and the scene cells/takes that share it), so
//! write-through to a referenced entity coincides with today's in-place
//! pattern write. `TrackPatternData` survives as the *composed* working type
//! for the live mirror, launches, undo snapshots, and serialization capture;
//! the pool stores the split form and composes/decomposes at its API edge.

use super::*;
use crate::sequencer::data::MidiFxPosition;
use crate::sequencer::data::TrackSendSnapshot;

/// Per-track Patch identity. Ids are scoped to one track's pool — they are
/// only meaningful against the `TrackPatternPool` that minted them (§17.12:
/// entities are per-track; cross-track refs don't exist in the model).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct PatchId(pub u64);

/// Per-track Mix identity (same scoping rules as [`PatchId`]).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct MixId(pub u64);

/// "The sound" (§17.2 prose): the pair of refs carried by every referent.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct SoundRefs {
    pub patch: PatchId,
    pub mix: MixId,
}

/// The device-shaped half of `TrackParamsSnapshot` (§17.8 field table).
#[derive(Clone, Debug)]
pub struct PatchParams {
    pub gate: bool,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub polyphonic: bool,
    pub max_polyphony: usize,
    pub mute_group: u8,
    pub midi_fx_chain: Vec<String>,
    pub midi_fx_position: MidiFxPosition,
}

/// A pooled Patch (§17.2): the device half of a track pattern — instrument,
/// effect and MIDI-FX slots (key locks ride inside the slots), the
/// instrument binding, the track process chain, and the device-shaped track
/// params.
#[derive(Clone, Debug)]
pub struct Patch {
    pub params: PatchParams,
    pub instrument_slot: EffectSlotSnapshot,
    pub effect_slots: Vec<EffectSlotSnapshot>,
    pub midi_fx_slots: Vec<EffectSlotSnapshot>,
    pub instrument_base_note_offset: f32,
    pub track_sound_state: TrackSoundState,
    pub sample_id: (i32, String, u32),
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    pub rack_track: Option<RackTrackSnapshot>,
    pub process_chain: crate::process::TrackProcessChain,
}

/// A pooled Mix (§17.2/§17.8): the mixer half. `solo` is deliberately
/// absent — it left the persisted model entirely (live-only session state).
#[derive(Clone, Debug)]
pub struct Mix {
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    pub send: f32,
    pub sends: Vec<TrackSendSnapshot>,
    pub output: TrackOutput,
}

/// The sequence-side residue of `TrackParamsSnapshot` (§17.8): fields that
/// change what the steps *mean* and therefore stay on the pattern.
#[derive(Clone, Debug)]
pub struct SeqParams {
    pub swing: f32,
    pub swing_resolution: SwingResolution,
    pub num_steps: usize,
    pub timebase: Timebase,
    pub accumulator_idx: usize,
    pub script_accumulator_name: Option<String>,
    pub accum_limit: f32,
    pub accum_mode: u32,
    pub fts_scale: usize,
    pub global_transpose: bool,
}

/// Per-track entity pools. Stored inside `TrackPatternPool` so undo lane
/// snapshots, track moves, and serialization carry the entities with the
/// patterns automatically.
#[derive(Clone, Debug, Default)]
pub struct TrackSoundPool {
    pub patches: HashMap<PatchId, Patch>,
    pub mixes: HashMap<MixId, Mix>,
    pub next_patch_id: u64,
    pub next_mix_id: u64,
}

impl TrackSoundPool {
    pub fn insert_patch(&mut self, patch: Patch) -> PatchId {
        let id = PatchId(self.next_patch_id);
        self.next_patch_id = self.next_patch_id.saturating_add(1);
        self.patches.insert(id, patch);
        id
    }

    pub fn insert_mix(&mut self, mix: Mix) -> MixId {
        let id = MixId(self.next_mix_id);
        self.next_mix_id = self.next_mix_id.saturating_add(1);
        self.mixes.insert(id, mix);
        id
    }

    pub fn insert(&mut self, patch: Patch, mix: Mix) -> SoundRefs {
        SoundRefs {
            patch: self.insert_patch(patch),
            mix: self.insert_mix(mix),
        }
    }

    pub fn resolves(&self, refs: SoundRefs) -> bool {
        self.patches.contains_key(&refs.patch) && self.mixes.contains_key(&refs.mix)
    }

    /// Fork (§17.3): clone the referenced entities into fresh ones. Falls
    /// back to defaults if a ref is dangling (never expected — the
    /// always-resolves invariant — but a fork must not mint nothing).
    pub fn fork(&mut self, refs: SoundRefs) -> SoundRefs {
        let patch = self
            .patches
            .get(&refs.patch)
            .cloned()
            .unwrap_or_else(Patch::new_default);
        let mix = self.mixes.get(&refs.mix).cloned().unwrap_or_default();
        self.insert(patch, mix)
    }

    /// Drop every entity not named in `keep` (save-time pruning, §17.4 —
    /// never called mid-session behind the user's back).
    pub fn retain_refs(&mut self, keep: &HashSet<SoundRefs>) {
        let patches: HashSet<PatchId> = keep.iter().map(|refs| refs.patch).collect();
        let mixes: HashSet<MixId> = keep.iter().map(|refs| refs.mix).collect();
        self.patches.retain(|id, _| patches.contains(id));
        self.mixes.retain(|id, _| mixes.contains(id));
    }
}

impl Default for Mix {
    fn default() -> Self {
        let params = TrackParamsSnapshot::default();
        Self {
            volume: params.volume,
            pan: params.pan,
            mute: params.mute,
            send: params.send,
            sends: params.sends,
            output: params.output,
        }
    }
}

impl Patch {
    pub fn new_default() -> Self {
        let params = TrackParamsSnapshot::default();
        Self {
            params: PatchParams {
                gate: params.gate,
                attack_ms: params.attack_ms,
                release_ms: params.release_ms,
                polyphonic: params.polyphonic,
                max_polyphony: params.max_polyphony,
                mute_group: params.mute_group,
                midi_fx_chain: params.midi_fx_chain,
                midi_fx_position: params.midi_fx_position,
            },
            instrument_slot: EffectSlotSnapshot::new_empty(),
            effect_slots: Vec::new(),
            midi_fx_slots: PatternSnapshot::default_midi_fx_slots(),
            instrument_base_note_offset: 0.0,
            track_sound_state: TrackSoundState::default(),
            sample_id: (-1, String::new(), 44_100),
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            rack_track: None,
            process_chain: crate::process::TrackProcessChain::default(),
        }
    }
}

/// The stored (split) form of a pool pattern: sequence data plus the refs
/// that name its sound (§17.2 "pool pattern" referent).
#[derive(Clone, Debug)]
pub struct StoredPattern {
    pub seq: TrackPatternSeq,
    pub sound: SoundRefs,
}

/// The sequence half of a pool pattern — every per-step lane, the step grid,
/// and the sequence-side params. Holds no device state at all.
#[derive(Clone, Debug)]
pub struct TrackPatternSeq {
    pub track_bits: [u64; TRACK_PATTERN_WORDS],
    pub neural_reset_bits: [u64; TRACK_PATTERN_WORDS],
    pub step_data: Vec<[f32; NUM_PARAMS]>,
    pub params: SeqParams,
    pub chord_snapshot: ChordSnapshot,
    pub timebase_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub swing_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub swing_resolution_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub project_process_lane_overrides: crate::process::ProjectLaneOverrides,
    pub plock_variant_registry: PlockVariantRegistry,
    pub key_lock_variant_registry: PlockVariantRegistry,
}

impl TrackPatternData {
    /// Decompose the composed working form into stored halves (§17.8 split).
    pub fn split(self) -> (TrackPatternSeq, Patch, Mix) {
        let TrackPatternData {
            track_bits,
            neural_reset_bits,
            step_data,
            track_params,
            effect_slots,
            midi_fx_slots,
            instrument_slot,
            instrument_base_note_offset,
            track_sound_state,
            sample_id,
            chord_snapshot,
            timebase_plock_snapshot,
            swing_plock_snapshot,
            swing_resolution_plock_snapshot,
            instrument_type,
            instrument_run_mode,
            rack_track,
            process_chain,
            project_process_lane_overrides,
            plock_variant_registry,
            key_lock_variant_registry,
        } = self;
        let seq = TrackPatternSeq {
            track_bits,
            neural_reset_bits,
            step_data,
            params: SeqParams {
                swing: track_params.swing,
                swing_resolution: track_params.swing_resolution,
                num_steps: track_params.num_steps,
                timebase: track_params.timebase,
                accumulator_idx: track_params.accumulator_idx,
                script_accumulator_name: track_params.script_accumulator_name.clone(),
                accum_limit: track_params.accum_limit,
                accum_mode: track_params.accum_mode,
                fts_scale: track_params.fts_scale,
                global_transpose: track_params.global_transpose,
            },
            chord_snapshot,
            timebase_plock_snapshot,
            swing_plock_snapshot,
            swing_resolution_plock_snapshot,
            project_process_lane_overrides,
            plock_variant_registry,
            key_lock_variant_registry,
        };
        let patch = Patch {
            params: PatchParams {
                gate: track_params.gate,
                attack_ms: track_params.attack_ms,
                release_ms: track_params.release_ms,
                polyphonic: track_params.polyphonic,
                max_polyphony: track_params.max_polyphony,
                mute_group: track_params.mute_group,
                midi_fx_chain: track_params.midi_fx_chain.clone(),
                midi_fx_position: track_params.midi_fx_position,
            },
            instrument_slot,
            effect_slots,
            midi_fx_slots,
            instrument_base_note_offset,
            track_sound_state,
            sample_id,
            instrument_type,
            instrument_run_mode,
            rack_track,
            process_chain,
        };
        let mix = Mix {
            volume: track_params.volume,
            pan: track_params.pan,
            mute: track_params.mute,
            send: track_params.send,
            sends: track_params.sends,
            output: track_params.output,
        };
        (seq, patch, mix)
    }

    /// Recompose the working form from stored halves.
    pub fn compose(seq: &TrackPatternSeq, patch: &Patch, mix: &Mix) -> TrackPatternData {
        TrackPatternData {
            track_bits: seq.track_bits,
            neural_reset_bits: seq.neural_reset_bits,
            step_data: seq.step_data.clone(),
            track_params: TrackParamsSnapshot {
                gate: patch.params.gate,
                attack_ms: patch.params.attack_ms,
                release_ms: patch.params.release_ms,
                swing: seq.params.swing,
                swing_resolution: seq.params.swing_resolution,
                num_steps: seq.params.num_steps,
                volume: mix.volume,
                pan: mix.pan,
                mute: mix.mute,
                send: mix.send,
                output: mix.output.clone(),
                sends: mix.sends.clone(),
                polyphonic: patch.params.polyphonic,
                max_polyphony: patch.params.max_polyphony,
                timebase: seq.params.timebase,
                accumulator_idx: seq.params.accumulator_idx,
                script_accumulator_name: seq.params.script_accumulator_name.clone(),
                midi_fx_chain: patch.params.midi_fx_chain.clone(),
                midi_fx_position: patch.params.midi_fx_position,
                accum_limit: seq.params.accum_limit,
                accum_mode: seq.params.accum_mode,
                fts_scale: seq.params.fts_scale,
                mute_group: patch.params.mute_group,
                global_transpose: seq.params.global_transpose,
            },
            effect_slots: patch.effect_slots.clone(),
            midi_fx_slots: patch.midi_fx_slots.clone(),
            instrument_slot: patch.instrument_slot.clone(),
            instrument_base_note_offset: patch.instrument_base_note_offset,
            track_sound_state: patch.track_sound_state.clone(),
            sample_id: patch.sample_id.clone(),
            chord_snapshot: seq.chord_snapshot.clone(),
            timebase_plock_snapshot: seq.timebase_plock_snapshot,
            swing_plock_snapshot: seq.swing_plock_snapshot,
            swing_resolution_plock_snapshot: seq.swing_resolution_plock_snapshot,
            instrument_type: patch.instrument_type,
            instrument_run_mode: patch.instrument_run_mode,
            rack_track: patch.rack_track.clone(),
            process_chain: patch.process_chain.clone(),
            project_process_lane_overrides: seq.project_process_lane_overrides.clone(),
            plock_variant_registry: seq.plock_variant_registry.clone(),
            key_lock_variant_registry: seq.key_lock_variant_registry.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::state::track_pattern_device_state_agrees;

    fn state_with_scenes(tracks: usize, scenes: usize) -> SequencerState {
        let state = SequencerState::new(
            tracks,
            (0..tracks).map(|_| default_empty_effect_chain()).collect(),
        );
        state.replace_pattern_repository(
            (0..scenes)
                .map(|_| PatternSnapshot::new_default(tracks, &[]))
                .collect(),
            0,
        );
        state
    }

    fn chunk_template(volume: f32, base_note: f32) -> TrackPatternData {
        let mut data = PatternSnapshot::new_default(1, &[])
            .track_pattern_data(0)
            .expect("default track data");
        data.track_params.num_steps = MAX_STEPS;
        data.track_params.volume = volume;
        data.instrument_base_note_offset = base_note;
        data
    }

    /// §18.1 exit criterion: every scene cell, pool pattern, and take
    /// resolves both refs, across scene create / assignment / clearing /
    /// forking / take registration.
    #[test]
    fn every_referent_resolves_sound_refs() {
        let state = state_with_scenes(2, 2);
        let take_id = state
            .register_track_take(
                0,
                None,
                vec![chunk_template(0.7, 3.0), chunk_template(0.7, 3.0)],
                MAX_STEPS as u32 + 12,
            )
            .expect("take registers");
        state.with_scenes_mut(|scenes| {
            // Scene create forks; pattern clone + assignment repoints; a
            // cleared cell keeps its refs ("no steps" never means "no
            // sound", §17.2).
            let new_scene = scenes.new_scene();
            assert!(scenes.fork_track_pattern(0).is_some());
            assert!(scenes.clone_track_pattern_into_current_scene(1).is_some());
            scenes.clear_cell(new_scene, 1);
            scenes.validate_sound_refs().expect("all refs resolve");

            // Take chunks share their take's entities structurally.
            let take = scenes.take_pools[0].get(take_id).expect("take exists").clone();
            for chunk in &take.chunks {
                assert_eq!(
                    scenes.track_pools[0].refs(*chunk),
                    Some(take.sound),
                    "chunk {} shares the take's sound",
                    chunk.0
                );
            }
            // A cell with a pattern shares the pattern's refs.
            for scene in &scenes.scenes {
                for (track, cell) in scene.cells.iter().enumerate() {
                    if let Some(id) = cell {
                        assert_eq!(
                            scenes.track_pools[track].refs(*id),
                            scene.cell_sounds.get(track).copied(),
                            "cell and pattern name the same entities"
                        );
                    }
                }
            }
        });
    }

    /// §18.1 step 5: legacy-shaped load (dense bank + take chunks, no ref
    /// model) → save-shaped export → load again; audio-relevant state is
    /// identical and the ref structure is canonical both times.
    #[test]
    fn project_scene_and_take_state_round_trips_through_split_storage() {
        // "Old project": build a live state and derive the legacy wire
        // pieces from it (dense bank + per-chunk take data, no ref model).
        let source = state_with_scenes(2, 2);
        source.with_scenes_mut(|scenes| {
            for scene_idx in 0..2 {
                for track in 0..2 {
                    let id = scenes.scenes[scene_idx].cells[track].expect("cell");
                    assert!(scenes.track_pools[track].edit(id, |data| {
                        data.track_params.volume = 0.1 + scene_idx as f32 + track as f32 * 0.5;
                        data.track_params.attack_ms = 5.0 * (scene_idx + 1) as f32;
                        data.instrument_base_note_offset = scene_idx as f32 + 7.0;
                        data.track_bits[0] = 0b1011 << track;
                    }));
                }
            }
        });
        let take_id = source
            .register_track_take(
                0,
                Some("Riff".into()),
                vec![chunk_template(0.42, 12.0), chunk_template(0.42, 12.0)],
                MAX_STEPS as u32 + 30,
            )
            .expect("take registers");

        let export = |state: &SequencerState| {
            let bank = state.export_pattern_repository();
            state.with_project_scenes(|scenes| {
                let presence: Vec<Vec<bool>> = scenes
                    .scenes
                    .iter()
                    .map(|scene| {
                        (0..scenes.track_pools.len())
                            .map(|track| scene.cells[track].is_some())
                            .collect()
                    })
                    .collect();
                let takes: Vec<(u64, Vec<(u64, String, u32, Vec<TrackPatternData>)>)> = scenes
                    .take_pools
                    .iter()
                    .enumerate()
                    .map(|(track, pool)| {
                        (
                            pool.next_take_id,
                            pool.takes
                                .iter()
                                .map(|take| {
                                    (
                                        take.id.0,
                                        take.name.clone(),
                                        take.total_len_steps,
                                        take.chunks
                                            .iter()
                                            .map(|chunk| {
                                                scenes.track_pools[track]
                                                    .get(*chunk)
                                                    .expect("chunk resolves")
                                            })
                                            .collect(),
                                    )
                                })
                                .collect(),
                        )
                    })
                    .collect();
                (bank.clone(), presence, takes)
            })
        };

        let (bank, presence, takes) = export(&source);
        // Legacy load: no ref model → §17.7 migration mints private entities
        // per pattern and collapses chunk duplicates onto one pair.
        let loaded = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        loaded.replace_pattern_repository(bank, 0);
        loaded.install_project_arrangement(&presence, takes);
        loaded.apply_project_sound_model(&[]);

        let compare = |left: &SequencerState, right: &SequencerState| {
            let left_scenes = left.capture_project_scenes();
            let right_scenes = right.capture_project_scenes();
            right_scenes.validate_sound_refs().expect("refs resolve");
            assert_eq!(left_scenes.scenes.len(), right_scenes.scenes.len());
            for (ls, rs) in left_scenes.scenes.iter().zip(&right_scenes.scenes) {
                for track in 0..2 {
                    match (ls.cells[track], rs.cells[track]) {
                        (Some(lid), Some(rid)) => {
                            let ld = left_scenes.track_pools[track].get(lid).unwrap();
                            let rd = right_scenes.track_pools[track].get(rid).unwrap();
                            assert!(
                                track_pattern_device_state_agrees(&ld, &rd),
                                "device state survives"
                            );
                            assert_eq!(ld.track_bits, rd.track_bits);
                            assert_eq!(
                                ld.track_params.volume.to_bits(),
                                rd.track_params.volume.to_bits()
                            );
                            assert_eq!(
                                ld.track_params.attack_ms.to_bits(),
                                rd.track_params.attack_ms.to_bits()
                            );
                            assert_eq!(ld.track_params.num_steps, rd.track_params.num_steps);
                        }
                        (None, None) => {}
                        (l, r) => panic!("cell presence diverged: {l:?} vs {r:?}"),
                    }
                }
            }
            // Take: same id/name/length, chunk device state identical, and
            // chunks + take share one entity pair post-load.
            let lt = left_scenes.take_pools[0].get(take_id).expect("take");
            let rt = right_scenes.take_pools[0].get(take_id).expect("take");
            assert_eq!(lt.name, rt.name);
            assert_eq!(lt.total_len_steps, rt.total_len_steps);
            assert_eq!(lt.chunks.len(), rt.chunks.len());
            for (lc, rc) in lt.chunks.iter().zip(&rt.chunks) {
                let ld = left_scenes.track_pools[0].get(*lc).unwrap();
                let rd = right_scenes.track_pools[0].get(*rc).unwrap();
                assert!(track_pattern_device_state_agrees(&ld, &rd));
                assert_eq!(ld.track_bits, rd.track_bits);
                assert_eq!(
                    right_scenes.track_pools[0].refs(*rc),
                    Some(rt.sound),
                    "chunks share the take's entities"
                );
            }
        };
        compare(&source, &loaded);

        // Save from the migrated state and load once more (a v7-shaped trip:
        // same bank pieces plus the canonical ref structure already being
        // structural). Audio-relevant state must still be identical.
        let (bank2, presence2, takes2) = export(&loaded);
        let reloaded = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        reloaded.replace_pattern_repository(bank2, 0);
        reloaded.install_project_arrangement(&presence2, takes2);
        reloaded.apply_project_sound_model(&[]);
        compare(&loaded, &reloaded);
        compare(&source, &reloaded);
    }
}
