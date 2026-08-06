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

/// Display metadata for one pool entity (§17.11): an auto-assigned name
/// ("Patch 3" / "Mix 2" — naming is never required) and an optional color
/// index into the track's palette set. `None` means the set was exhausted at
/// mint (name-only display); colors are recycled by the cleanup gesture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundEntityMeta {
    pub name: String,
    pub color: Option<u8>,
}

/// Size of the per-track color set (§17.11): colors are unique per track per
/// entity kind while the set lasts — a timeline dot's color identifies
/// exactly one Patch on that track — and entities minted past the set fall
/// back to name-only display.
pub const SOUND_COLOR_SET: u8 = 8;

/// Per-track entity pools. Stored inside `TrackPatternPool` so undo lane
/// snapshots, track moves, and serialization carry the entities with the
/// patterns automatically.
#[derive(Clone, Debug, Default)]
pub struct TrackSoundPool {
    /// `Arc` for structural sharing with history snapshots — see
    /// `TrackPatternPool::patterns`. Mutations copy-on-write through
    /// `Arc::make_mut`.
    pub patches: HashMap<PatchId, Arc<Patch>>,
    pub mixes: HashMap<MixId, Arc<Mix>>,
    pub next_patch_id: u64,
    pub next_mix_id: u64,
    /// §17.11 display metadata, parallel to `patches`/`mixes`. Entries are
    /// auto-assigned at mint; a missing entry (an older save) is repaired by
    /// `ensure_meta` at load.
    pub patch_meta: HashMap<PatchId, SoundEntityMeta>,
    pub mix_meta: HashMap<MixId, SoundEntityMeta>,
}

impl TrackSoundPool {
    pub fn insert_patch(&mut self, patch: Patch) -> PatchId {
        let id = PatchId(self.next_patch_id);
        self.next_patch_id = self.next_patch_id.saturating_add(1);
        self.patches.insert(id, Arc::new(patch));
        let color = self.allocate_patch_color();
        self.patch_meta.insert(
            id,
            SoundEntityMeta {
                name: format!("Patch {}", id.0 + 1),
                color,
            },
        );
        id
    }

    pub fn insert_mix(&mut self, mix: Mix) -> MixId {
        let id = MixId(self.next_mix_id);
        self.next_mix_id = self.next_mix_id.saturating_add(1);
        self.mixes.insert(id, Arc::new(mix));
        let color = self.allocate_mix_color();
        self.mix_meta.insert(
            id,
            SoundEntityMeta {
                name: format!("Mix {}", id.0 + 1),
                color,
            },
        );
        id
    }

    /// Smallest color index not held by a live Patch, or `None` past the set
    /// (§17.11). Scanning live entities makes recycling automatic: cleanup
    /// removes the entity and its meta, freeing the color.
    fn allocate_patch_color(&self) -> Option<u8> {
        let used: HashSet<u8> = self
            .patch_meta
            .iter()
            .filter(|(id, _)| self.patches.contains_key(id))
            .filter_map(|(_, meta)| meta.color)
            .collect();
        (0..SOUND_COLOR_SET).find(|color| !used.contains(color))
    }

    fn allocate_mix_color(&self) -> Option<u8> {
        let used: HashSet<u8> = self
            .mix_meta
            .iter()
            .filter(|(id, _)| self.mixes.contains_key(id))
            .filter_map(|(_, meta)| meta.color)
            .collect();
        (0..SOUND_COLOR_SET).find(|color| !used.contains(color))
    }

    /// Repair pass for loads that predate display metadata (older v7 files):
    /// every live entity without a meta entry gets the auto-assignment it
    /// would have received at mint. Also drops meta for entities that no
    /// longer exist, so color allocation never counts ghosts.
    pub fn ensure_meta(&mut self) {
        self.patch_meta.retain(|id, _| self.patches.contains_key(id));
        self.mix_meta.retain(|id, _| self.mixes.contains_key(id));
        let mut missing: Vec<PatchId> = self
            .patches
            .keys()
            .filter(|id| !self.patch_meta.contains_key(id))
            .copied()
            .collect();
        missing.sort();
        for id in missing {
            let color = self.allocate_patch_color();
            self.patch_meta.insert(
                id,
                SoundEntityMeta {
                    name: format!("Patch {}", id.0 + 1),
                    color,
                },
            );
        }
        let mut missing: Vec<MixId> = self
            .mixes
            .keys()
            .filter(|id| !self.mix_meta.contains_key(id))
            .copied()
            .collect();
        missing.sort();
        for id in missing {
            let color = self.allocate_mix_color();
            self.mix_meta.insert(
                id,
                SoundEntityMeta {
                    name: format!("Mix {}", id.0 + 1),
                    color,
                },
            );
        }
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
            .map(|patch| Patch::clone(patch))
            .unwrap_or_else(Patch::new_default);
        let mix = self
            .mixes
            .get(&refs.mix)
            .map(|mix| Mix::clone(mix))
            .unwrap_or_default();
        self.insert(patch, mix)
    }

    /// Drop every entity not named in `keep` (save-time pruning, §17.4 —
    /// never called mid-session behind the user's back).
    pub fn retain_refs(&mut self, keep: &HashSet<SoundRefs>) {
        let patches: HashSet<PatchId> = keep.iter().map(|refs| refs.patch).collect();
        let mixes: HashSet<MixId> = keep.iter().map(|refs| refs.mix).collect();
        self.patches.retain(|id, _| patches.contains(id));
        self.mixes.retain(|id, _| mixes.contains(id));
        // Meta follows the entity; dropping it frees the color (§17.11).
        self.patch_meta.retain(|id, _| patches.contains(id));
        self.mix_meta.retain(|id, _| mixes.contains(id));
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

impl TrackPatternSeq {
    /// The default (empty) sequence half, derived from the same default
    /// pattern every fresh lane starts from. Used to compose content
    /// carriers for entities that have no pattern referent (bare cells).
    pub fn new_default() -> Self {
        PatternSnapshot::new_default(1, &[])
            .track_pattern_data(0)
            .expect("default snapshot has lane 0")
            .split()
            .0
    }
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
                None,
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
                None,
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

    /// §17.2 across persistence: a bare cell's sound — an entity no pattern
    /// or take chunk serializes — rides an orphan carrier and survives a
    /// save-shaped export → load.
    #[test]
    fn bare_cell_sound_survives_round_trip_via_carrier() {
        let source = state_with_scenes(1, 2);
        // Scene 1: distinctive device + mixer state, then delete the
        // pattern — the cell goes bare but keeps the sound.
        let (cell_refs, carrier_data) = source.with_scenes_mut(|scenes| {
            let id = scenes.scenes[1].cells[0].expect("scene 1 cell");
            assert!(scenes.track_pools[0].edit(id, |data| {
                data.track_params.volume = 0.77;
                data.track_params.attack_ms = 42.0;
                data.instrument_base_note_offset = 9.0;
            }));
            assert!(scenes.delete_track_pattern(0, id));
            assert!(scenes.scenes[1].cells[0].is_none());
            let refs = scenes.scenes[1].cell_sounds[0];
            let data = scenes.track_pools[0]
                .compose_bare_sound(refs)
                .expect("bare cell refs resolve");
            (refs, data)
        });

        // Save-shaped export: dense bank + presence + ref structure +
        // the carrier the save path would emit for the uncarried pair.
        let bank = source.export_pattern_repository();
        let (presence, cells, patterns) = source.with_project_scenes(|scenes| {
            let presence: Vec<Vec<bool>> = scenes
                .scenes
                .iter()
                .map(|scene| vec![scene.cells[0].is_some()])
                .collect();
            let cells: Vec<(u64, u64)> = scenes
                .scenes
                .iter()
                .map(|scene| {
                    let refs = scene.cell_sounds[0];
                    (refs.patch.0, refs.mix.0)
                })
                .collect();
            let patterns: Vec<Option<(u64, u64)>> = scenes
                .scenes
                .iter()
                .map(|scene| {
                    scene.cells[0]
                        .and_then(|id| scenes.track_pools[0].refs(id))
                        .map(|refs| (refs.patch.0, refs.mix.0))
                })
                .collect();
            (presence, cells, patterns)
        });

        let loaded = SequencerState::new(1, vec![default_empty_effect_chain()]);
        loaded.replace_pattern_repository(bank, 0);
        loaded.install_project_arrangement(&presence, Vec::new());
        loaded.apply_project_sound_model(&[(
            cells,
            patterns,
            Vec::new(),
            vec![(cell_refs.patch.0, cell_refs.mix.0, carrier_data)],
            Vec::new(),
            Vec::new(),
            None,
        )]);

        loaded.with_project_scenes(|scenes| {
            scenes.validate_sound_refs().expect("refs resolve");
            assert!(scenes.scenes[1].cells[0].is_none(), "cell stays bare");
            let refs = scenes.scenes[1].cell_sounds[0];
            let data = scenes.track_pools[0]
                .compose_bare_sound(refs)
                .expect("carrier entity resolves");
            assert_eq!(data.track_params.volume.to_bits(), 0.77f32.to_bits());
            assert_eq!(data.track_params.attack_ms.to_bits(), 42.0f32.to_bits());
            assert_eq!(data.instrument_base_note_offset.to_bits(), 9.0f32.to_bits());
            // Scene 0's cell still shares its pattern's entities.
            let id = scenes.scenes[0].cells[0].expect("scene 0 cell");
            assert_eq!(
                scenes.track_pools[0].refs(id),
                Some(scenes.scenes[0].cell_sounds[0])
            );
        });
    }

    /// Track-sound spec §2.5 (symptom-6 regression): the first cell created
    /// on a formerly-bare lane seeds its sound FROM the track sound —
    /// clone-by-default, never an alias.
    #[test]
    fn a_new_cell_on_a_formerly_bare_lane_seeds_its_sound_from_the_track_sound() {
        let state = state_with_scenes(1, 1);
        state.with_scenes_mut(|scenes| {
            let id = scenes.scenes[0].cells[0].expect("cell");
            assert!(scenes.delete_track_pattern(0, id));
        });
        // The bare lane's mirror IS the track sound (§2.3): a live edit sits
        // there awaiting the next save-back.
        state.pattern.instrument_base_note_offsets[0].store(9.0f32.to_bits(), Ordering::Relaxed);

        // An explicit content gesture materializes a pattern (§2.5).
        let snapshot = state.capture_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        );
        let mut data = snapshot.track_pattern_data(0).expect("lane data");
        data.track_bits[0] |= 1;
        let id = state
            .materialize_current_scene_pattern(0, data)
            .expect("content gesture materializes a cell");

        state.with_project_scenes(|scenes| {
            let cell_refs = scenes.track_pools[0].refs(id).expect("cell refs");
            let track_refs = scenes.track_sound_refs(0).expect("track sound");
            assert_ne!(
                cell_refs, track_refs,
                "the new cell CLONES the track sound (§2.5), never aliases it"
            );
            let cell = scenes.track_pools[0].get(id).expect("cell data");
            assert_eq!(
                cell.instrument_base_note_offset.to_bits(),
                9.0f32.to_bits(),
                "the new cell's sound content comes from the track sound"
            );
            let absorbed = scenes.track_pools[0]
                .compose_bare_sound(track_refs)
                .expect("track sound composes");
            assert_eq!(
                absorbed.instrument_base_note_offset.to_bits(),
                9.0f32.to_bits(),
                "the mirror's un-saved edit was absorbed into the track sound"
            );
            scenes.validate_sound_refs().expect("refs resolve");
        });
    }

    /// Track-sound spec §2.6: loading a project without the serialized track
    /// sound seeds it from the first RESOLVING cell (post-presence), forked,
    /// and from the newest take when no cell resolves anywhere.
    #[test]
    fn track_sound_seeding_prefers_first_resolving_cell_then_takes() {
        // Scene 0 bare, scene 1 resolves: the seed comes from scene 1.
        let state = state_with_scenes(1, 2);
        state.with_scenes_mut(|scenes| {
            let id = scenes.scenes[1].cells[0].expect("scene 1 cell");
            assert!(scenes.track_pools[0].edit(id, |data| {
                data.instrument_base_note_offset = 5.0;
            }));
        });
        state.install_project_arrangement(&[vec![false], vec![true]], Vec::new());
        state.with_project_scenes(|scenes| {
            let refs = scenes.track_sound_refs(0).expect("track sound");
            let data = scenes.track_pools[0]
                .compose_bare_sound(refs)
                .expect("track sound composes");
            assert_eq!(
                data.instrument_base_note_offset.to_bits(),
                5.0f32.to_bits(),
                "the seed forks the first resolving cell's sound"
            );
            let cell_refs = scenes.scenes[1].cells[0]
                .and_then(|id| scenes.track_pools[0].refs(id))
                .expect("cell refs");
            assert_ne!(refs, cell_refs, "forked, never aliased");
        });

        // Takes-only project: the seed comes from the newest take's sound.
        let takes_only = state_with_scenes(1, 1);
        let take = takes_only
            .register_track_take(0, None, vec![chunk_template(0.9, 12.0)], 16, None)
            .expect("take registers");
        takes_only.install_project_arrangement(&[vec![false]], Vec::new());
        takes_only.with_project_scenes(|scenes| {
            let refs = scenes.track_sound_refs(0).expect("track sound");
            let data = scenes.track_pools[0]
                .compose_bare_sound(refs)
                .expect("track sound composes");
            assert_eq!(
                data.instrument_base_note_offset.to_bits(),
                12.0f32.to_bits(),
                "a takes-only lane seeds from the take's sound"
            );
            let take_sound = scenes.take_pools[0].get(take).expect("take").sound;
            assert_ne!(refs, take_sound, "forked, never aliased");
        });
    }

    /// Track-sound spec §2.1/§2.6: the track sound's refs and content
    /// survive a save-shaped export → load through the sound model (the
    /// `track` field plus an orphan carrier).
    #[test]
    fn track_sound_survives_round_trip_via_track_field_and_carrier() {
        let names = ["Track 1".to_string()];
        let types = [InstrumentType::Sampler];
        let source = state_with_scenes(1, 1);
        source.with_scenes_mut(|scenes| {
            let id = scenes.scenes[0].cells[0].expect("cell");
            assert!(scenes.delete_track_pattern(0, id));
        });
        // A live mixer move persists into the track sound via the bare-lane
        // save-back (§2.3).
        source.pattern.track_params[0].set_volume(0.77);
        assert!(source.save_current_pattern_snapshot(1, &[-1], &[44_100], &names, &types));

        // Save-shaped export: dense bank + presence + refs + carriers, the
        // pieces `finish_project_load` reassembles.
        let bank = source.export_pattern_repository();
        let (cell_refs, cell_carrier, track_refs, track_carrier) =
            source.with_project_scenes(|scenes| {
                let cell_refs = scenes.scenes[0].cell_sounds[0];
                let track_refs = scenes.track_sound_refs(0).expect("track sound");
                (
                    cell_refs,
                    scenes.track_pools[0]
                        .compose_bare_sound(cell_refs)
                        .expect("cell carrier"),
                    track_refs,
                    scenes.track_pools[0]
                        .compose_bare_sound(track_refs)
                        .expect("track carrier"),
                )
            });

        let loaded = SequencerState::new(1, vec![default_empty_effect_chain()]);
        loaded.replace_pattern_repository(bank, 0);
        loaded.install_project_arrangement(&[vec![false]], Vec::new());
        loaded.apply_project_sound_model(&[(
            vec![(cell_refs.patch.0, cell_refs.mix.0)],
            vec![None],
            Vec::new(),
            vec![
                (cell_refs.patch.0, cell_refs.mix.0, cell_carrier),
                (track_refs.patch.0, track_refs.mix.0, track_carrier),
            ],
            Vec::new(),
            Vec::new(),
            Some((track_refs.patch.0, track_refs.mix.0)),
        )]);

        loaded.with_project_scenes(|scenes| {
            scenes.validate_sound_refs().expect("refs resolve");
            assert!(scenes.scenes[0].cells[0].is_none(), "the lane stays bare");
            let refs = scenes.track_sound_refs(0).expect("track sound resolves");
            let data = scenes.track_pools[0]
                .compose_bare_sound(refs)
                .expect("track sound composes");
            assert_eq!(
                data.track_params.volume.to_bits(),
                0.77f32.to_bits(),
                "the track sound's content survived the round trip"
            );
            // The cell's own sound is distinct and untouched by the track
            // sound's value.
            let cell = scenes.scenes[0].cell_sounds[0];
            assert_ne!(cell, refs, "cell sound and track sound stay distinct");
        });
    }

    /// §18.2 item 4 must-pass: scene create forks the Mix, so per-scene
    /// volume divergence (scene 1 at 0.5, scene 2 at 0.7) survives — while a
    /// take registered against the cell's refs SHARES its Mix.
    #[test]
    fn per_scene_mix_divergence_survives_while_takes_share() {
        let state = state_with_scenes(1, 1);
        state.with_scenes_mut(|scenes| {
            let s0 = scenes.scenes[0].cells[0].expect("scene 0 cell");
            assert!(scenes.track_pools[0].edit(s0, |data| {
                data.track_params.volume = 0.5;
            }));
            let new_scene = scenes.new_scene();
            let s1 = scenes.scenes[new_scene].cells[0].expect("new scene cell");
            assert_ne!(
                scenes.track_pools[0].refs(s0).expect("refs").mix,
                scenes.track_pools[0].refs(s1).expect("refs").mix,
                "scene create forks the Mix (§17.3)"
            );
            assert!(scenes.track_pools[0].edit(s1, |data| {
                data.track_params.volume = 0.7;
            }));
            let v0 = scenes.track_pools[0].get(s0).expect("s0").track_params.volume;
            let v1 = scenes.track_pools[0].get(s1).expect("s1").track_params.volume;
            assert_eq!(v0.to_bits(), 0.5f32.to_bits());
            assert_eq!(v1.to_bits(), 0.7f32.to_bits());
        });
        // A take that shares scene 0's refs hears scene 0's fader, and a
        // fader write through the take is heard by the cell (one Mix).
        let refs = state.with_project_scenes(|scenes| scenes.scenes[0].cell_sounds[0]);
        let take = state
            .register_track_take(
                0,
                None,
                vec![chunk_template(0.9, 0.0)],
                16,
                Some(refs),
            )
            .expect("take registers");
        state.with_scenes_mut(|scenes| {
            let take = scenes.take_pools[0].get(take).expect("take").clone();
            assert_eq!(take.sound, refs, "take shares the cell sound (§17.3)");
            let chunk = take.chunks[0];
            assert_eq!(
                scenes.track_pools[0]
                    .get(chunk)
                    .expect("chunk")
                    .track_params
                    .volume
                    .to_bits(),
                0.5f32.to_bits(),
                "the chunk's own 0.9 device half was dropped; the shared Mix wins"
            );
            assert!(scenes.track_pools[0].edit(chunk, |data| {
                data.track_params.volume = 0.25;
            }));
            let s0 = scenes.scenes[0].cells[0].expect("scene 0 cell");
            assert_eq!(
                scenes.track_pools[0].get(s0).expect("s0").track_params.volume.to_bits(),
                0.25f32.to_bits(),
                "a take-side fader write reaches the cell through the shared Mix"
            );
        });
    }

    /// §18.1 step 3 (S2): a capture with a borrowed lane is TRUTHFUL — the
    /// snapshot carries the scene-effective device state, not the borrowed
    /// bound sound, and the borrow itself survives the capture (no more
    /// release/re-bind dance around save-backs).
    #[test]
    fn capture_with_borrowed_lane_is_truthful_and_keeps_the_borrow() {
        let state = state_with_scenes(1, 1);
        let other = state.with_scenes_mut(|scenes| {
            let cell = scenes.scenes[0].cells[0].expect("cell");
            let mut data = scenes.track_pools[0].get(cell).expect("cell data");
            data.instrument_base_note_offset = 9.0;
            scenes.track_pools[0].insert(data)
        });
        let data = state
            .with_project_scenes(|scenes| scenes.track_pools[0].get(other))
            .expect("other pattern");
        assert!(state.borrow_track_device_state(0, other, &data));
        let snapshot = state.capture_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        );
        assert_eq!(
            snapshot.instrument_base_note_offsets[0].to_bits(),
            0.0f32.to_bits(),
            "the captured lane carries the scene sound, not the borrow"
        );
        assert_eq!(
            state.sound_binding_borrowed_mask() & 1,
            1,
            "the borrow survives the capture"
        );
        // The live mirror still holds the bound sound.
        assert_eq!(
            f32::from_bits(
                state.pattern.instrument_base_note_offsets[0]
                    .load(std::sync::atomic::Ordering::Relaxed)
            )
            .to_bits(),
            9.0f32.to_bits()
        );
    }

    /// §18.2 item 4 must-pass pair: launching a pool pattern brings its own
    /// Mix level (the cell repoints to the pattern's refs), and a
    /// post-launch fader tweak lands in that Mix via save-back, surviving a
    /// relaunch.
    #[test]
    fn pattern_launch_brings_its_mix_and_relaunch_preserves_tweaks() {
        let state = state_with_scenes(1, 1);
        let names = ["Track 1".to_string()];
        let types = [InstrumentType::Sampler];
        let pattern = state.with_scenes_mut(|scenes| {
            let cell = scenes.scenes[0].cells[0].expect("cell");
            let mut data = scenes.track_pools[0].get(cell).expect("cell data");
            data.track_params.volume = 0.8;
            scenes.track_pools[0].insert(data)
        });
        assert!(state.launch_track_pattern(0, pattern, 1, &[-1], &[44_100], &names, &types));
        assert_eq!(
            state.pattern.track_params[0].get_volume().to_bits(),
            0.8f32.to_bits(),
            "launch brings the pattern's own mix level"
        );
        state.with_project_scenes(|scenes| {
            assert_eq!(
                scenes.effective_sound_refs(0),
                scenes.track_pools[0].refs(pattern),
                "the launch repoints the track's effective refs to the \
                 pattern's (§17.3, via the launch override)"
            );
        });

        // Post-launch tweak: live fader move, saved back through the
        // pattern's entities.
        state.pattern.track_params[0].set_volume(0.66);
        assert!(state.save_current_pattern_snapshot(1, &[-1], &[44_100], &names, &types));
        state.with_project_scenes(|scenes| {
            assert_eq!(
                scenes.track_pools[0]
                    .get(pattern)
                    .expect("pattern")
                    .track_params
                    .volume
                    .to_bits(),
                0.66f32.to_bits(),
                "the tweak landed in the launched pattern's Mix"
            );
        });

        // Switch back to the scene's own pattern (override cleared, its own
        // forked Mix takes the fader)…
        assert!(state
            .launch_scene(0, 1, &[-1], &[44_100], &names, &types)
            .is_some());
        assert_ne!(
            state.pattern.track_params[0].get_volume().to_bits(),
            0.66f32.to_bits(),
            "the scene's own Mix is not the launched pattern's"
        );
        // …then relaunch the pattern: the post-launch tweak survives in its
        // Mix.
        assert!(state.launch_track_pattern(0, pattern, 1, &[-1], &[44_100], &names, &types));
        assert_eq!(
            state.pattern.track_params[0].get_volume().to_bits(),
            0.66f32.to_bits(),
            "relaunch restores the post-launch tweak"
        );
    }

    /// §17.11: mint auto-assigns a name and a color unique per track while
    /// the set lasts; past the set entities are name-only; cleanup recycles
    /// colors; `ensure_meta` repairs metadata-less loads.
    #[test]
    fn display_metadata_mints_unique_colors_and_recycles() {
        let mut pool = TrackSoundPool::default();
        let ids: Vec<PatchId> = (0..SOUND_COLOR_SET as usize + 1)
            .map(|_| pool.insert_patch(Patch::new_default()))
            .collect();
        let colors: Vec<Option<u8>> = ids
            .iter()
            .map(|id| pool.patch_meta.get(id).expect("meta minted").color)
            .collect();
        let assigned: HashSet<u8> = colors.iter().flatten().copied().collect();
        assert_eq!(
            assigned.len(),
            SOUND_COLOR_SET as usize,
            "colors are unique while the set lasts"
        );
        assert_eq!(
            colors.last().copied().flatten(),
            None,
            "past the set falls back to name-only"
        );
        assert_eq!(
            pool.patch_meta.get(&ids[2]).expect("meta").name,
            format!("Patch {}", ids[2].0 + 1),
            "auto-name at mint"
        );

        // Cleanup frees the dropped entity's color for the next mint.
        let freed = colors[3].expect("in-set entity has a color");
        let mix = pool.insert_mix(Mix::default());
        let keep: HashSet<SoundRefs> = ids
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != 3)
            .map(|(_, id)| SoundRefs { patch: *id, mix })
            .collect();
        pool.retain_refs(&keep);
        assert!(!pool.patch_meta.contains_key(&ids[3]), "meta follows the entity");
        let recycled = pool.insert_patch(Patch::new_default());
        assert_eq!(
            pool.patch_meta.get(&recycled).expect("meta").color,
            Some(freed),
            "the freed color is recycled"
        );

        // A metadata-less load (older v7 file) repairs to mint behavior.
        pool.patch_meta.clear();
        pool.mix_meta.clear();
        pool.ensure_meta();
        assert_eq!(pool.patch_meta.len(), pool.patches.len());
        assert_eq!(pool.mix_meta.len(), pool.mixes.len());
        let repaired: HashSet<u8> = pool
            .patch_meta
            .values()
            .filter_map(|meta| meta.color)
            .collect();
        assert_eq!(repaired.len(), SOUND_COLOR_SET as usize);
    }

    /// Deleting a track shifts every scene's `cell_sounds` together with
    /// `cells` (the parallel-vector law a track MOVE already obeys).
    #[test]
    fn remove_track_keeps_cell_sounds_aligned() {
        let state = state_with_scenes(3, 2);
        // Make track 2's sound recognizable so the shift is observable.
        let marker = state.with_scenes_mut(|scenes| {
            let id = scenes.scenes[0].cells[2].expect("track 2 cell");
            assert!(scenes.track_pools[2].edit(id, |data| {
                data.track_params.volume = 0.31;
            }));
            scenes.scenes[0].cell_sounds[2]
        });
        state.with_scenes_mut(|scenes| {
            assert!(scenes.remove_track(1));
            scenes.validate_sound_refs().expect("refs stay aligned");
            for scene in &scenes.scenes {
                assert_eq!(scene.cell_sounds.len(), scene.cells.len());
            }
            // Former track 2 now lives at index 1 and still resolves its
            // own sound against the pool that moved with it.
            assert_eq!(scenes.scenes[0].cell_sounds[1], marker);
            let data = scenes.track_pools[1]
                .compose_bare_sound(marker)
                .expect("shifted refs resolve");
            assert_eq!(data.track_params.volume.to_bits(), 0.31f32.to_bits());
        });
    }
}
