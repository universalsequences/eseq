use super::*;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct PatternId(pub u64);

/// Stable logical identity for a project scene.
///
/// Scene indices are presentation order and can change when scenes are
/// inserted, deleted, or reordered. Long-lived authoring references use this
/// identity instead.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct SceneId(pub u64);

/// Stable identity for an ordered scene bank.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct SceneBankId(pub u64);

pub const MAX_SCENES_PER_BANK: usize = 24;

/// One contiguous span in the flat scene presentation order.
///
/// The offset is deliberately derived from the lengths of preceding banks;
/// storing it here would create a second, fallible source of truth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneBank {
    pub id: SceneBankId,
    pub name: Option<String>,
    pub len: usize,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct TrackPatternId {
    pub track: TrackId,
    pub pattern: PatternId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackPatternCellView {
    pub pattern_id: PatternId,
    pub assigned_to_current_scene: bool,
    pub active_effective: bool,
    pub overridden: bool,
}

/// One track's pattern pool, in the split storage form (takes spec §17.2):
/// patterns hold sequence data plus `(patch_ref, mix_ref)`, and the device
/// state lives in the co-located entity pools. The pool API composes and
/// decomposes `TrackPatternData` (the working type) at its edge, so a
/// `store`/`edit` writes the device half through the pattern's refs.
#[derive(Clone, Debug)]
pub struct TrackPatternPool {
    /// `Arc` for structural sharing with history snapshots: cloning a pool
    /// (undo capture at every scene-structure edit, including clip launches)
    /// bumps refcounts instead of deep-copying every pattern, and mutation
    /// sites copy-on-write exactly the patterns they touch via
    /// `Arc::make_mut`. At 20-clip pools the eager deep clone cost ~30ms per
    /// capture (×2 per edit, plus the same again freeing evicted patches).
    pub patterns: HashMap<PatternId, Arc<StoredPattern>>,
    pub next_id: u64,
    pub sounds: TrackSoundPool,
}

#[derive(Clone, Debug)]
pub struct SceneTrackReferenceState {
    pub mod_connections: Vec<ModConnection>,
    pub neural_networks: Vec<ProjectNeuralNetwork>,
    pub graph_overrides: Vec<ProjectGraphOverrides>,
}

#[derive(Clone, Debug)]
pub struct TrackSidechainPatternState {
    pub owner_track: usize,
    pub pattern: PatternId,
    pub slots: Vec<(usize, EffectSlotSnapshot)>,
}

#[derive(Clone, Debug)]
pub struct TrackPatternLaneState {
    pub pool: TrackPatternPool,
    pub scene_cells: Vec<Option<PatternId>>,
    /// Per-scene cell sound refs for this lane, parallel to `scene_cells`.
    pub cell_sounds: Vec<SoundRefs>,
    /// The lane's track-sound carrier pattern (track-sound spec §2.1); its
    /// entities travel inside `pool`.
    pub track_sound: Option<PatternId>,
    pub track_override: Option<PatternId>,
    pub scene_references: Vec<SceneTrackReferenceState>,
    pub sidechains: Vec<TrackSidechainPatternState>,
}

impl Default for TrackPatternPool {
    fn default() -> Self {
        Self {
            patterns: HashMap::new(),
            // Reserve 0 for atomic/sentinel uses; real track pattern ids start at 1.
            next_id: 1,
            sounds: TrackSoundPool::default(),
        }
    }
}

impl TrackPatternPool {
    /// Insert a pattern, minting a private Patch + Mix for it (today's copy
    /// semantics — §18.1 step 4: in S1 a fresh pattern always forks).
    pub fn insert(&mut self, data: TrackPatternData) -> PatternId {
        let (seq, patch, mix) = data.split();
        let sound = self.sounds.insert(patch, mix);
        self.insert_stored(StoredPattern { seq, sound })
    }

    /// Insert a pattern whose sound is an existing entity pair (take chunks
    /// share their take's Patch/Mix). The device half of `data` is dropped —
    /// the shared entities are already the sound — unless a ref is somehow
    /// unpopulated, in which case it seeds the entity.
    pub fn insert_with_refs(&mut self, data: TrackPatternData, sound: SoundRefs) -> PatternId {
        let (seq, patch, mix) = data.split();
        self.sounds.patches.entry(sound.patch).or_insert_with(|| Arc::new(patch));
        self.sounds.mixes.entry(sound.mix).or_insert_with(|| Arc::new(mix));
        // If the defensive seed above actually fired, keep the mint cursors
        // ahead of the seeded ids — a later mint colliding with one would
        // silently replace the entity under every referent sharing it.
        self.sounds.next_patch_id = self.sounds.next_patch_id.max(sound.patch.0.saturating_add(1));
        self.sounds.next_mix_id = self.sounds.next_mix_id.max(sound.mix.0.saturating_add(1));
        self.insert_stored(StoredPattern { seq, sound })
    }

    fn insert_stored(&mut self, stored: StoredPattern) -> PatternId {
        let id = PatternId(self.next_id.max(1));
        self.next_id = id.0.saturating_add(1).max(1);
        self.patterns.insert(id, Arc::new(stored));
        id
    }

    /// Insert a hidden carrier pattern for an existing entity pair — the
    /// track-sound representation (track-sound spec §2.1): a pool pattern
    /// with an empty default sequence whose only job is to name the track's
    /// own Patch/Mix so the whole device-edit machinery (capture, restore,
    /// undo, re-link) works on the track sound through an ordinary
    /// `PatternId`. Never assigned to a cell, hidden from the clip grid.
    pub fn insert_bare_sound_pattern(&mut self, sound: SoundRefs) -> Option<PatternId> {
        if !self.sounds.resolves(sound) {
            return None;
        }
        Some(self.insert_stored(StoredPattern {
            seq: TrackPatternSeq::new_default(),
            sound,
        }))
    }

    pub fn contains(&self, id: PatternId) -> bool {
        self.patterns.contains_key(&id)
    }

    /// Compose the working form of a pattern (sequence half + resolved
    /// Patch/Mix). Owned: the split storage has no contiguous
    /// `TrackPatternData` to hand out by reference.
    pub fn get(&self, id: PatternId) -> Option<TrackPatternData> {
        let stored = self.patterns.get(&id)?;
        let patch = self.sounds.patches.get(&stored.sound.patch)?;
        let mix = self.sounds.mixes.get(&stored.sound.mix)?;
        Some(TrackPatternData::compose(&stored.seq, patch, mix))
    }

    pub fn seq(&self, id: PatternId) -> Option<&TrackPatternSeq> {
        self.patterns.get(&id).map(|stored| &stored.seq)
    }

    pub fn refs(&self, id: PatternId) -> Option<SoundRefs> {
        self.patterns.get(&id).map(|stored| stored.sound)
    }

    pub fn patch(&self, id: PatternId) -> Option<&Patch> {
        self.sounds
            .patches
            .get(&self.patterns.get(&id)?.sound.patch)
            .map(Arc::as_ref)
    }

    /// Mutable access to the Patch entity a pattern references. Device edits
    /// through this write the entity — every pattern sharing it hears them.
    pub fn patch_mut(&mut self, id: PatternId) -> Option<&mut Patch> {
        let sound = self.patterns.get(&id)?.sound;
        self.sounds.patches.get_mut(&sound.patch).map(Arc::make_mut)
    }

    /// Replace a pattern wholesale: the sequence half lands on the stored
    /// pattern, the device half writes through its refs (§18.1 step 3 —
    /// this is the write path that makes every save-back an entity write).
    pub fn store(&mut self, id: PatternId, data: TrackPatternData) -> bool {
        let Some(stored) = self.patterns.get_mut(&id) else {
            return false;
        };
        let (seq, patch, mix) = data.split();
        let sound = stored.sound;
        Arc::make_mut(stored).seq = seq;
        self.sounds.patches.insert(sound.patch, Arc::new(patch));
        self.sounds.mixes.insert(sound.mix, Arc::new(mix));
        true
    }

    /// Compose, run `edit`, decompose-store. In S1 every entity has one
    /// pattern, so this coincides with an in-place mutation.
    pub fn edit(&mut self, id: PatternId, edit: impl FnOnce(&mut TrackPatternData)) -> bool {
        let Some(mut data) = self.get(id) else {
            return false;
        };
        edit(&mut data);
        self.store(id, data)
    }

    /// Compose-edit-store every pattern in the pool. Only for IDEMPOTENT
    /// edits: patterns sharing a sound (a take's chunks) write the same
    /// entities once per pattern. Non-idempotent device transforms (index
    /// remaps, structural pushes) must iterate `sounds` entities instead.
    pub fn edit_all(&mut self, mut edit: impl FnMut(PatternId, &mut TrackPatternData)) {
        let ids: Vec<PatternId> = self.patterns.keys().copied().collect();
        for id in ids {
            self.edit(id, |data| edit(id, data));
        }
    }

    /// Re-link a pattern's sound to an existing entity pair (§17.3 re-link /
    /// repoint). Reference semantics: future edits to those entities follow.
    /// Returns whether the refs actually moved; refuses dangling refs (the
    /// always-resolves invariant).
    pub fn relink_sound(&mut self, id: PatternId, refs: SoundRefs) -> bool {
        if !self.sounds.resolves(refs) {
            return false;
        }
        match self.patterns.get_mut(&id) {
            Some(stored) if stored.sound != refs => {
                Arc::make_mut(stored).sound = refs;
                true
            }
            _ => false,
        }
    }

    /// Remove a pattern, returning its composed form. Its entities stay in
    /// the pool (§17.4: never GC'd behind the user's back mid-session;
    /// unreferenced entities are pruned at save).
    pub fn remove(&mut self, id: PatternId) -> Option<TrackPatternData> {
        let composed = self.get(id)?;
        self.patterns.remove(&id);
        Some(composed)
    }

    /// Every `(patch_ref, mix_ref)` reachable from a pattern in this pool.
    pub fn referenced_sounds(&self) -> HashSet<SoundRefs> {
        self.patterns.values().map(|stored| stored.sound).collect()
    }

    /// Compose a sound's content onto an empty default sequence — the save
    /// carrier for entities referenced only by bare cells, which no pattern
    /// or take chunk serializes.
    pub fn compose_bare_sound(&self, refs: SoundRefs) -> Option<TrackPatternData> {
        let patch = self.sounds.patches.get(&refs.patch)?;
        let mix = self.sounds.mixes.get(&refs.mix)?;
        Some(TrackPatternData::compose(
            &TrackPatternSeq::new_default(),
            patch,
            mix,
        ))
    }
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub id: SceneId,
    pub name: String,
    pub cells: Vec<Option<PatternId>>,
    /// Per-track sound refs (takes spec §17.2 "scene cell" referent), kept
    /// parallel to `cells`. Always resolves — "no steps" never means "no
    /// sound": a cell with a pattern shares that pattern's refs; a cell
    /// without one keeps the last adopted refs (or a minted default).
    pub cell_sounds: Vec<SoundRefs>,
    pub bus_patterns: Vec<BusPatternSnapshot>,
    // These are scene-level because per-track launches must not swap project-wide
    // modulation, neural, or graph routing state.
    pub mod_connections: Vec<ModConnection>,
    pub neural_networks: Vec<ProjectNeuralNetwork>,
    pub graph_overrides: Vec<ProjectGraphOverrides>,
    /// Pattern-scoped portable values declared through `defscene`.
    pub scene_slots: SceneSlotStore,
    /// Project-level default process chain: composed ahead of every track's
    /// own chain at snapshot capture, so present and future tracks inherit it.
    pub project_process_chain: crate::process::TrackProcessChain,
}

#[derive(Clone, Debug)]
pub struct ProjectScenes {
    pub track_pools: Vec<TrackPatternPool>,
    /// Per-track take ownership over pool patterns (takes spec 6.1). Grown
    /// alongside `track_pools`; chunk patterns live in the pattern pool and
    /// are hidden from the clip grid because a take claims them.
    pub take_pools: Vec<TrackTakePool>,
    pub scenes: Vec<Scene>,
    pub current_scene: usize,
    pub track_overrides: Vec<Option<PatternId>>,
    /// Per-track **track sound** (track-sound spec §2.1): the hidden carrier
    /// pattern whose Patch/Mix pair is the sound a bare lane monitors, edits,
    /// and records with. Owned by the track, never resolved through
    /// `current_scene`, unaffected by transport and scene launches. Kept
    /// parallel to `track_pools`; `ensure_track_sounds` repairs gaps.
    pub track_sounds: Vec<Option<PatternId>>,
    /// Ordered contiguous spans over `scenes`. Always non-empty.
    pub banks: Vec<SceneBank>,
    pub(super) next_scene_id: u64,
    pub(super) next_bank_id: u64,
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
            let mut cell_sounds = Vec::with_capacity(track_count);
            for track in 0..track_count {
                match snapshot.track_pattern_data(track) {
                    Some(data) => {
                        let id = track_pools[track].insert(data);
                        cells[track] = Some(id);
                        cell_sounds.push(
                            track_pools[track].refs(id).expect("pattern just inserted"),
                        );
                    }
                    // No pattern lane in the snapshot: the cell still needs
                    // a resolving sound (§17.2 always-resolves invariant).
                    None => cell_sounds.push(
                        track_pools[track]
                            .sounds
                            .insert(Patch::new_default(), Mix::default()),
                    ),
                }
            }
            scenes.push(Scene {
                id: SceneId(scene_idx as u64 + 1),
                name: format!("Scene {}", scene_idx + 1),
                cells,
                cell_sounds,
                bus_patterns: Vec::new(),
                mod_connections: snapshot.mod_connections.clone(),
                neural_networks: snapshot.neural_networks.clone(),
                graph_overrides: snapshot.graph_overrides.clone(),
                scene_slots: snapshot.scene_slots.clone(),
                project_process_chain: snapshot.project_process_chain.clone(),
            });
        }

        if scenes.is_empty() {
            let cell_sounds = (0..track_count)
                .map(|track| {
                    track_pools[track]
                        .sounds
                        .insert(Patch::new_default(), Mix::default())
                })
                .collect();
            scenes.push(Scene {
                id: SceneId(1),
                name: "Scene 1".to_string(),
                cells: vec![None; track_count],
                cell_sounds,
                bus_patterns: Vec::new(),
                mod_connections: Vec::new(),
                neural_networks: Vec::new(),
                graph_overrides: Vec::new(),
                scene_slots: SceneSlotStore::default(),
                project_process_chain: crate::process::TrackProcessChain::default(),
            });
        }

        let scene_count = scenes.len();
        let mut built = Self {
            take_pools: vec![TrackTakePool::default(); track_pools.len()],
            track_pools,
            scenes,
            current_scene: current_scene.min(snapshots.len().saturating_sub(1)),
            track_overrides: vec![None; track_count],
            track_sounds: vec![None; track_count],
            banks: vec![SceneBank {
                id: SceneBankId(1),
                name: None,
                len: scene_count,
            }],
            next_scene_id: u64::try_from(snapshots.len().max(1))
                .expect("scene count exceeds stable identity space")
                .checked_add(1)
                .expect("scene identity space exhausted"),
            next_bank_id: 2,
        };
        built.ensure_track_sounds();
        built
    }

    /// Replace the default bank with serialized bank metadata when it forms a
    /// valid partition. Invalid or legacy metadata deliberately repairs to one
    /// unnamed bank containing every scene, so organization data can never
    /// make an otherwise usable project fail to load.
    pub fn install_scene_banks(&mut self, banks: Vec<SceneBank>) {
        let next_bank_id = banks
            .iter()
            .map(|bank| bank.id.0)
            .max()
            .and_then(|id| id.checked_add(1));
        if self.validate_scene_banks(&banks).is_ok() && next_bank_id.is_some() {
            self.banks = banks;
            self.next_bank_id = next_bank_id.expect("checked above");
        } else {
            self.banks = vec![SceneBank {
                id: SceneBankId(1),
                name: None,
                len: self.scenes.len(),
            }];
            self.next_bank_id = 2;
        }
    }

    pub(crate) fn next_scene_bank_id(&self) -> SceneBankId {
        SceneBankId(self.next_bank_id)
    }

    /// Preserve bank identity and boundaries across legacy snapshot-based
    /// repository rebuilds that do not change scene topology.
    pub(crate) fn copy_scene_bank_model_from(&mut self, source: &ProjectScenes) {
        assert_eq!(
            self.scenes.len(),
            source.scenes.len(),
            "scene-bank metadata can only cross a topology-preserving rebuild"
        );
        source
            .validate_scene_bank_model()
            .expect("source scene-bank model must be valid");
        self.banks = source.banks.clone();
        self.next_bank_id = source.next_bank_id;
    }

    pub fn validate_scene_bank_model(&self) -> Result<(), String> {
        self.validate_scene_banks(&self.banks)?;
        if self.next_bank_id == 0
            || self.banks.iter().any(|bank| bank.id.0 >= self.next_bank_id)
        {
            return Err("scene bank identity cursor is invalid".to_string());
        }
        Ok(())
    }

    fn validate_scene_banks(&self, banks: &[SceneBank]) -> Result<(), String> {
        if banks.is_empty() {
            return Err("a project must contain at least one scene bank".to_string());
        }
        if banks.iter().any(|bank| bank.len > MAX_SCENES_PER_BANK) {
            return Err(format!(
                "a scene bank exceeds the {MAX_SCENES_PER_BANK}-scene capacity"
            ));
        }
        let ids: HashSet<_> = banks.iter().map(|bank| bank.id).collect();
        if ids.len() != banks.len() || ids.iter().any(|id| id.0 == 0) {
            return Err("scene bank identities must be nonzero and unique".to_string());
        }
        let covered = banks.iter().try_fold(0usize, |sum, bank| {
            sum.checked_add(bank.len)
                .ok_or_else(|| "scene bank lengths overflow".to_string())
        })?;
        if covered != self.scenes.len() {
            return Err("scene bank lengths do not cover the scene list".to_string());
        }
        Ok(())
    }

    /// The track's own sound carrier pattern (track-sound spec §2.1).
    pub fn track_sound_pattern(&self, track: usize) -> Option<PatternId> {
        self.track_sounds.get(track).copied().flatten()
    }

    /// The track sound's `(patch_ref, mix_ref)` pair.
    pub fn track_sound_refs(&self, track: usize) -> Option<SoundRefs> {
        let id = self.track_sound_pattern(track)?;
        self.track_pools.get(track)?.refs(id)
    }

    /// Seed one track's sound (track-sound spec §2.6): clone (fork) the refs
    /// of the first resolving cell — scene-0-first scan — else fork the
    /// newest take's sound (the takes-only workflow this feature exists for:
    /// a default Patch would carry an empty device chain while the takes
    /// hold the sound the user actually hears), else mint a fresh default
    /// Patch/Mix. Always a fork, never an alias — edits to the track sound
    /// must not retune the seeding cell or take. Replaces any existing
    /// entry.
    pub fn seed_track_sound(&mut self, track: usize) -> Option<PatternId> {
        if track >= self.track_pools.len() {
            return None;
        }
        let source_refs = self
            .scenes
            .iter()
            .find_map(|scene| {
                let id = scene.cells.get(track).copied().flatten()?;
                self.track_pools.get(track)?.refs(id)
            })
            .or_else(|| {
                self.take_pools
                    .get(track)?
                    .takes
                    .iter()
                    .max_by_key(|take| take.id.0)
                    .map(|take| take.sound)
            });
        let pool = &mut self.track_pools[track];
        let sound = match source_refs {
            Some(refs) => pool.sounds.fork(refs),
            None => pool.sounds.insert(Patch::new_default(), Mix::default()),
        };
        let id = pool.insert_bare_sound_pattern(sound)?;
        while self.track_sounds.len() <= track {
            self.track_sounds.push(None);
        }
        self.track_sounds[track] = Some(id);
        Some(id)
    }

    /// Repair pass: every track holds a resolving track sound. Missing or
    /// dangling entries are re-seeded per §2.6; run at construction, when
    /// tracks grow, and after project-load reshaping.
    pub fn ensure_track_sounds(&mut self) {
        self.track_sounds.resize(self.track_pools.len(), None);
        self.track_sounds.truncate(self.track_pools.len());
        for track in 0..self.track_pools.len() {
            let resolves = self
                .track_sounds
                .get(track)
                .copied()
                .flatten()
                .is_some_and(|id| {
                    self.track_pools
                        .get(track)
                        .is_some_and(|pool| pool.contains(id))
                });
            if !resolves {
                self.seed_track_sound(track);
            }
        }
    }

    pub fn scene_count(&self) -> usize {
        self.scenes.len().max(1)
    }

    pub fn scene_id(&self, scene_idx: usize) -> Option<SceneId> {
        self.scenes.get(scene_idx).map(|scene| scene.id)
    }

    pub fn scene_index(&self, id: SceneId) -> Option<usize> {
        self.scenes.iter().position(|scene| scene.id == id)
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
                        .and_then(|id| self.track_pools[track].patch(id))
                        .map(|patch| patch.sample_id.clone())
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
        snapshot.scene_slots = scene.scene_slots.clone();
        snapshot.project_process_chain = scene.project_process_chain.clone();
        for track in 0..self.track_pools.len() {
            let Some(id) = scene.cells.get(track).copied().flatten() else {
                continue;
            };
            let Some(data) = self.track_pools[track].get(id) else {
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
        self.save_scene_snapshot_masked(scene_idx, snapshot, 0, 0, 0)
    }

    /// Save a live-grid snapshot into a scene, skipping lanes whose live
    /// content does not belong to that scene. A set bit in `stale_mask`
    /// marks such a lane (song-latched or row-silenced): its live grid holds
    /// performer/leftover content, and writing it through the scene cell
    /// would overwrite an unrelated pool pattern with a clone of whatever
    /// happens to be live. A stale lane is still saved when a track override
    /// pins its own pattern id — that write is a self-write into the pattern
    /// the lane is actually playing.
    ///
    /// `latched_mask` (a subset of `stale_mask`) marks manually latched
    /// lanes: on a bare lane the mirror's device state normally belongs to
    /// the track sound and is persisted there (track-sound spec §2.3), but a
    /// latched lane's mirror is the performer's clip — its device deltas
    /// must not leak into the track sound.
    ///
    /// `track_owned_mask` (the other subset of `stale_mask`) marks lanes whose
    /// sound the TRACK owns (track-sound spec §2.2.2, rev 4): arrangement
    /// context, rules 1/2 unclaimed. Their mirror IS the track sound — a
    /// resolving cell there is inert-but-visible — so the device half persists
    /// into the TRACK SOUND and the cell's own entities stay untouched. In Seq
    /// context the mask is empty and every lane saves to its cell, exactly as
    /// rev 1. The view flag lives on `SequencerState`, which is why it arrives
    /// as a mask.
    pub fn save_scene_snapshot_masked(
        &mut self,
        scene_idx: usize,
        snapshot: PatternSnapshot,
        stale_mask: u64,
        latched_mask: u64,
        track_owned_mask: u64,
    ) -> bool {
        while self.track_pools.len() < snapshot.track_bits.len() {
            self.track_pools.push(TrackPatternPool::default());
            self.take_pools.push(TrackTakePool::default());
            self.track_overrides.push(None);
            let pool = self.track_pools.last_mut().expect("just pushed");
            for scene in &mut self.scenes {
                scene.cells.push(None);
                scene
                    .cell_sounds
                    .push(pool.sounds.insert(Patch::new_default(), Mix::default()));
            }
        }
        while self.take_pools.len() < self.track_pools.len() {
            self.take_pools.push(TrackTakePool::default());
        }
        // New tracks get a track sound with their pool (§2.1).
        self.ensure_track_sounds();
        let Some(scene) = self.scenes.get_mut(scene_idx) else {
            return false;
        };
        while scene.cells.len() < snapshot.track_bits.len() {
            let track = scene.cells.len();
            scene.cells.push(None);
            scene.cell_sounds.push(
                self.track_pools[track]
                    .sounds
                    .insert(Patch::new_default(), Mix::default()),
            );
        }
        scene.mod_connections = snapshot.mod_connections.clone();
        scene.neural_networks = snapshot.neural_networks.clone();
        scene.graph_overrides = snapshot.graph_overrides.clone();
        scene.scene_slots = snapshot.scene_slots.clone();
        // Deliberately NOT copied from the snapshot: the scene itself is the
        // live authority for `project_process_chain` (edited in place via
        // edit_current_project_process_chain), and several callers save
        // snapshots that never carried it. Snapshot→scene transfer happens
        // only in from_pattern_snapshots (project load).

        for track in 0..snapshot.track_bits.len() {
            let Some(data) = snapshot.track_pattern_data(track) else {
                continue;
            };
            let resolved = self
                .track_overrides
                .get(track)
                .copied()
                .flatten()
                .or_else(|| scene.cells.get(track).copied().flatten())
                .filter(|id| self.track_pools[track].contains(*id));
            let latched = track < 64 && latched_mask >> track & 1 == 1;
            let track_owned = track < 64 && track_owned_mask >> track & 1 == 1;
            let pinned = self.track_overrides.get(track).copied().flatten().is_some();
            // A track-owned lane never reaches a cell (§2.2.2): its mirror is
            // the track sound, whatever cells happen to exist. Otherwise:
            // a stale lane holds foreign content — its cell is not what the
            // live grid is showing, so the save-back must never clone the
            // mirror over the pattern the cell really points at. A track
            // override pins the lane's own pattern id, so that write is a
            // self-write and stays allowed. A lane that resolves to nothing
            // has no pattern to clobber.
            let installed =
                !(track < 64 && stale_mask >> track & 1 == 1 && !pinned && resolved.is_some());
            if !track_owned {
                if let Some(id) = resolved.filter(|_| installed) {
                    self.track_pools[track].store(id, data);
                    continue;
                }
            }
            // Sticky bare lane (track-sound spec §2.3) and — rev 4, §2.2.2 —
            // the track-owned lane whose cell still resolves: never mint a
            // cell from leftover live content (the ghost-step resurrection,
            // spec §1.2), and never write the uninstalled cell's entities.
            // The live mirror's device/mixer state on such a lane belongs to
            // the TRACK SOUND, so persist it there instead of dropping the
            // user's edits. A latched lane's mirror is the performer's clip,
            // not the track's own sound — skip it. A cell holding a dangling
            // pattern id is ambiguous, not bare — skip it too.
            let cell_dangling =
                resolved.is_none() && scene.cells.get(track).copied().flatten().is_some();
            if latched || cell_dangling || (!track_owned && resolved.is_some()) {
                continue;
            }
            if let Some(refs) = self
                .track_sounds
                .get(track)
                .copied()
                .flatten()
                .and_then(|id| self.track_pools[track].refs(id))
            {
                let (_seq, patch, mix) = data.split();
                let sounds = &mut self.track_pools[track].sounds;
                sounds.patches.insert(refs.patch, Arc::new(patch));
                sounds.mixes.insert(refs.mix, Arc::new(mix));
            }
        }
        true
    }

    pub fn delete_scene(&mut self, scene_idx: usize) -> Option<usize> {
        if self.scenes.len() <= 1 || scene_idx >= self.scenes.len() {
            return None;
        }
        self.scenes.remove(scene_idx);
        let mut offset = 0usize;
        let owning_bank = self
            .banks
            .iter()
            .position(|bank| {
                let owns = scene_idx >= offset && scene_idx < offset + bank.len;
                offset += bank.len;
                owns
            })
            .expect("valid scene-bank partition must own every scene");
        self.banks[owning_bank].len -= 1;
        let new_idx = scene_idx.min(self.scenes.len() - 1);
        self.current_scene = new_idx;
        self.track_overrides.fill(None);
        Some(new_idx)
    }

    /// Move one scene to another position without modifying any track pattern
    /// pool. Scene cells contain stable pattern ids, so moving the scene itself
    /// preserves every track's pattern identity and data.
    pub fn reorder_scene(&mut self, source: usize, target: usize) -> Option<usize> {
        if source >= self.scenes.len() || target >= self.scenes.len() {
            return None;
        }
        if source == target {
            return Some(self.current_scene);
        }

        let scene = self.scenes.remove(source);
        self.scenes.insert(target, scene);
        self.current_scene = if self.current_scene == source {
            target
        } else if source < self.current_scene && self.current_scene <= target {
            self.current_scene - 1
        } else if target <= self.current_scene && self.current_scene < source {
            self.current_scene + 1
        } else {
            self.current_scene
        };
        Some(self.current_scene)
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

    pub fn current_scene_slots(&self) -> SceneSlotStore {
        self.scenes
            .get(self.current_scene)
            .map(|scene| scene.scene_slots.clone())
            .unwrap_or_default()
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

    /// Composed working form of the track's effective pattern. Owned: the
    /// split storage (§17.2) has no contiguous `TrackPatternData` to lend.
    pub fn effective_track_pattern(&self, track: usize) -> Option<TrackPatternData> {
        let id = self.effective_pattern_id(track)?;
        self.track_pools.get(track)?.get(id)
    }

    /// The effective cell's sound (override pattern's refs first, else the
    /// current scene cell's refs) — always resolves for a valid track.
    pub fn effective_sound_refs(&self, track: usize) -> Option<SoundRefs> {
        if let Some(id) = self.track_overrides.get(track).copied().flatten() {
            if let Some(refs) = self.track_pools.get(track)?.refs(id) {
                return Some(refs);
            }
        }
        if let Some(id) = self
            .scenes
            .get(self.current_scene)
            .and_then(|scene| scene.cells.get(track))
            .copied()
            .flatten()
        {
            if let Some(refs) = self.track_pools.get(track)?.refs(id) {
                return Some(refs);
            }
        }
        self.scenes
            .get(self.current_scene)?
            .cell_sounds
            .get(track)
            .copied()
    }

    pub fn save_effective_track_pattern(&mut self, track: usize, data: TrackPatternData) -> bool {
        let Some(id) = self.effective_pattern_id(track) else {
            return false;
        };
        self.track_pools
            .get_mut(track)
            .is_some_and(|pool| pool.store(id, data))
    }

    pub fn launch_scene(&mut self, scene: usize) -> Option<Vec<Option<TrackPatternData>>> {
        let scene_cells = self.scenes.get(scene)?.cells.clone();
        let mut track_patterns = Vec::with_capacity(scene_cells.len());
        for (track, cell) in scene_cells.iter().copied().enumerate() {
            let data = match cell {
                Some(id) => Some(self.track_pools.get(track)?.get(id)?),
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
        let data = self.track_pools.get(track)?.get(id)?;
        *self.track_overrides.get_mut(track)? = Some(id);
        Some(data)
    }

    /// Resolve every selected cell before changing any override. This keeps a
    /// masked scene launch atomic when a scene contains a stale or empty cell.
    pub fn launch_scene_tracks(
        &mut self,
        scene: usize,
        tracks: &[usize],
    ) -> Option<Vec<(usize, TrackPatternData)>> {
        let scene = self.scenes.get(scene)?;
        let resolved = tracks
            .iter()
            .copied()
            .map(|track| {
                let id = scene.cells.get(track).copied().flatten()?;
                let data = self.track_pools.get(track)?.get(id)?;
                Some((track, id, data))
            })
            .collect::<Option<Vec<_>>>()?;

        for (track, id, _) in &resolved {
            *self.track_overrides.get_mut(*track)? = Some(*id);
        }
        Some(
            resolved
                .into_iter()
                .map(|(track, _, data)| (track, data))
                .collect(),
        )
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
        // Take chunks are hidden from the clip grid (takes spec 11.2):
        // ownership by a take is the single source of truth for "hidden".
        // The track-sound carrier (track-sound spec §2.1) is not a clip
        // either.
        let takes = self.take_pools.get(track);
        let track_sound = self.track_sound_pattern(track);
        let mut ids = pool
            .patterns
            .keys()
            .copied()
            .filter(|id| !takes.is_some_and(|takes| takes.is_claimed(*id)))
            .filter(|id| Some(*id) != track_sound)
            .collect::<Vec<_>>();
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
        // Assigning a pattern to a cell repoints the cell's sound at the
        // pattern's (§17.3: after any launch/assignment, cell and pattern
        // name the same entities).
        let Some(refs) = pool.refs(id) else {
            return false;
        };
        let Some(scene) = self.scenes.get_mut(scene) else {
            return false;
        };
        if track >= scene.cells.len() {
            return false;
        }

        scene.cells[track] = Some(id);
        if let Some(cell_sound) = scene.cell_sounds.get_mut(track) {
            *cell_sound = refs;
        }
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
        let source = self.effective_track_pattern(track)?;
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
        let source = self.track_pools.get(track)?.get(source_id)?;
        let id = self.track_pools.get_mut(track)?.insert(source);
        let refs = self.track_pools.get(track)?.refs(id)?;
        let scene = self.scenes.get_mut(self.current_scene)?;
        scene.cells[track] = Some(id);
        if let Some(cell_sound) = scene.cell_sounds.get_mut(track) {
            *cell_sound = refs;
        }
        *self.track_overrides.get_mut(track)? = None;
        Some(id)
    }

    pub fn delete_track_pattern(&mut self, track: usize, id: PatternId) -> bool {
        // Chunk patterns are deleted only through their owning take
        // (takes spec 6.4); direct deletion would corrupt the take.
        if self
            .take_pools
            .get(track)
            .is_some_and(|takes| takes.is_claimed(id))
        {
            return false;
        }
        // The track-sound carrier is not user-deletable content (§2.1): it
        // is the persistent sound of the track itself.
        if self.track_sound_pattern(track) == Some(id) {
            return false;
        }
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
        let mut cell_sounds = Vec::with_capacity(self.track_pools.len());
        for track in 0..self.track_pools.len() {
            // Scene create forks eagerly per track (§17.3): a cloned pattern
            // inserted into the pool mints a fresh Patch + Mix, and the new
            // cell adopts them. A track with no effective pattern still
            // forks its current cell's sound so the new cell resolves.
            match self.effective_track_pattern(track) {
                Some(source) => {
                    let id = self.track_pools[track].insert(source);
                    cells[track] = Some(id);
                    cell_sounds.push(
                        self.track_pools[track]
                            .refs(id)
                            .expect("pattern just inserted"),
                    );
                }
                None => {
                    let source_refs = self.effective_sound_refs(track);
                    let sounds = &mut self.track_pools[track].sounds;
                    cell_sounds.push(match source_refs {
                        Some(refs) => sounds.fork(refs),
                        None => sounds.insert(Patch::new_default(), Mix::default()),
                    });
                }
            }
        }

        let scene_idx = self.scenes.len();
        let (
            bus_patterns,
            mod_connections,
            neural_networks,
            graph_overrides,
            scene_slots,
            project_process_chain,
        ) = source_scene
            .map(|scene| {
                (
                    scene.bus_patterns,
                    scene.mod_connections,
                    scene.neural_networks,
                    scene.graph_overrides,
                    scene.scene_slots,
                    scene.project_process_chain,
                )
            })
            .unwrap_or_default();
        let next_id = self.next_scene_id;
        self.next_scene_id = self
            .next_scene_id
            .checked_add(1)
            .expect("scene identity space exhausted");
        self.scenes.push(Scene {
            id: SceneId(next_id),
            name: format!("Scene {}", scene_idx + 1),
            cells,
            cell_sounds,
            bus_patterns,
            mod_connections,
            neural_networks,
            graph_overrides,
            scene_slots,
            project_process_chain,
        });
        let last_bank = self
            .banks
            .last_mut()
            .expect("a project always has at least one scene bank");
        if last_bank.len < MAX_SCENES_PER_BANK {
            last_bank.len += 1;
        } else {
            let id = SceneBankId(self.next_bank_id);
            self.next_bank_id = self
                .next_bank_id
                .checked_add(1)
                .expect("scene bank identity space exhausted");
            self.banks.push(SceneBank {
                id,
                name: None,
                len: 1,
            });
        }
        self.current_scene = scene_idx;
        self.track_overrides.fill(None);
        scene_idx
    }

    /// Reorders the per-track take pool so it stays parallel with
    /// `track_pools` when a track lane moves (undo of a track delete).
    pub fn move_track_take_pool(&mut self, from: usize, to: usize) {
        while self.take_pools.len() < self.track_pools.len() {
            self.take_pools.push(TrackTakePool::default());
        }
        if from >= self.take_pools.len() || to >= self.take_pools.len() || from == to {
            return;
        }
        let pool = self.take_pools.remove(from);
        self.take_pools.insert(to, pool);
    }

    pub fn remove_track(&mut self, track: usize) -> bool {
        if track >= self.track_pools.len() {
            return false;
        }

        self.track_pools.remove(track);
        if track < self.take_pools.len() {
            self.take_pools.remove(track);
        }
        for scene in &mut self.scenes {
            if track < scene.cells.len() {
                scene.cells.remove(track);
            }
            if track < scene.cell_sounds.len() {
                scene.cell_sounds.remove(track);
            }
        }
        if track < self.track_overrides.len() {
            self.track_overrides.remove(track);
        }
        if track < self.track_sounds.len() {
            self.track_sounds.remove(track);
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
            // Take chunks are referenced through take ownership, never scene
            // cells (takes spec 6.1); purging must not drop them. The
            // track-sound carrier is likewise a permanent referent (§2.1).
            if let Some(takes) = self.take_pools.get(track) {
                referenced.extend(takes.claimed());
            }
            if let Some(id) = self.track_sound_pattern(track) {
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

    /// Every sound entity pair still reachable on `track` — pattern refs
    /// (take chunks are pool patterns, so they're included), scene cell
    /// refs, and take sounds. Anything outside this set is an orphan:
    /// invisible to the model, awaiting pruning (§17.4).
    pub fn referenced_track_sounds(&self, track: usize) -> HashSet<SoundRefs> {
        let mut keep = self
            .track_pools
            .get(track)
            .map(|pool| pool.referenced_sounds())
            .unwrap_or_default();
        for scene in &self.scenes {
            if let Some(refs) = scene.cell_sounds.get(track) {
                keep.insert(*refs);
            }
        }
        if let Some(takes) = self.take_pools.get(track) {
            for take in &takes.takes {
                keep.insert(take.sound);
            }
        }
        keep
    }

    /// Drop every orphaned entity from every track's pool (§17.4 pruning).
    /// Safe wherever no one holds `PatchId`/`MixId` into the pools from
    /// outside `ProjectScenes` — undo lane snapshots clone pools wholesale,
    /// so they stay self-consistent regardless.
    pub fn prune_unreferenced_sounds(&mut self) -> usize {
        let mut removed = 0;
        for track in 0..self.track_pools.len() {
            let keep = self.referenced_track_sounds(track);
            let pool = &mut self.track_pools[track].sounds;
            let before = pool.patches.len() + pool.mixes.len();
            pool.retain_refs(&keep);
            removed += before - (pool.patches.len() + pool.mixes.len());
        }
        removed
    }

    /// The §17.2 always-resolves invariant: every scene cell, pool pattern,
    /// and take holds `(patch_ref, mix_ref)` that resolve in its track's
    /// entity pool — unconditionally. (§18.1 exit criterion.)
    pub fn validate_sound_refs(&self) -> Result<(), String> {
        for (track, pool) in self.track_pools.iter().enumerate() {
            for (id, stored) in &pool.patterns {
                if !pool.sounds.resolves(stored.sound) {
                    return Err(format!(
                        "Track {} pattern {} holds a sound ref that does not resolve",
                        track + 1,
                        id.0
                    ));
                }
            }
        }
        for (scene_idx, scene) in self.scenes.iter().enumerate() {
            if scene.cell_sounds.len() != scene.cells.len() {
                return Err(format!(
                    "Scene {} cell sounds ({}) are not parallel to cells ({})",
                    scene_idx + 1,
                    scene.cell_sounds.len(),
                    scene.cells.len()
                ));
            }
            for (track, refs) in scene.cell_sounds.iter().enumerate() {
                let resolves = self
                    .track_pools
                    .get(track)
                    .is_some_and(|pool| pool.sounds.resolves(*refs));
                if !resolves {
                    return Err(format!(
                        "Scene {} track {} cell sound does not resolve",
                        scene_idx + 1,
                        track + 1
                    ));
                }
            }
        }
        for (track, takes) in self.take_pools.iter().enumerate() {
            for take in &takes.takes {
                let resolves = self
                    .track_pools
                    .get(track)
                    .is_some_and(|pool| pool.sounds.resolves(take.sound));
                if !resolves {
                    return Err(format!(
                        "Track {} take {} sound does not resolve",
                        track + 1,
                        take.id.0
                    ));
                }
            }
        }
        // The track sound (track-sound spec §2.1) always resolves: its
        // carrier pattern exists in the pool (the pattern loop above then
        // validates its refs).
        for track in 0..self.track_pools.len() {
            let resolves = self
                .track_sounds
                .get(track)
                .copied()
                .flatten()
                .is_some_and(|id| self.track_pools[track].contains(id));
            if !resolves {
                return Err(format!(
                    "Track {} has no resolving track sound",
                    track + 1
                ));
            }
        }
        Ok(())
    }

    pub(super) fn edit_other_track_patterns<F>(&mut self, track: usize, mut edit: F) -> bool
    where
        F: FnMut(&mut TrackPatternData),
    {
        let current_effective = self.effective_pattern_id(track);
        let Some(pool) = self.track_pools.get_mut(track) else {
            return false;
        };
        // Callers apply structural (non-idempotent) device edits — slot
        // inserts/removes/moves — so each distinct sound entity must be
        // visited exactly once: patterns sharing a Patch (a take's chunks)
        // are edited through one representative.
        let effective_sound = current_effective.and_then(|id| pool.refs(id));
        let mut representatives: Vec<PatternId> = Vec::new();
        let mut seen: HashSet<PatchId> = HashSet::new();
        let mut ids: Vec<PatternId> = pool.patterns.keys().copied().collect();
        ids.sort_by_key(|id| id.0);
        for id in ids {
            if Some(id) == current_effective {
                continue;
            }
            let Some(sound) = pool.refs(id) else {
                continue;
            };
            if Some(sound.patch) == effective_sound.map(|refs| refs.patch) {
                continue;
            }
            if seen.insert(sound.patch) {
                representatives.push(id);
            }
        }
        for id in representatives {
            pool.edit(id, |data| edit(data));
        }
        // Bare-cell sounds (§17.2 "no steps ≠ no sound"): an entity
        // referenced only by a scene cell has no pattern representative,
        // but it is still a live sound — the same structural edit must
        // reach it, or its device chain drifts from the track's and later
        // slot edits fail validation against it.
        let mut cell_only: Vec<SoundRefs> = Vec::new();
        for scene in &self.scenes {
            if let Some(refs) = scene.cell_sounds.get(track) {
                if !seen.contains(&refs.patch)
                    && Some(refs.patch) != effective_sound.map(|refs| refs.patch)
                    && seen.insert(refs.patch)
                {
                    cell_only.push(*refs);
                }
            }
        }
        for refs in cell_only {
            let Some(mut data) = pool.compose_bare_sound(refs) else {
                continue;
            };
            edit(&mut data);
            let (_seq, patch, mix) = data.split();
            pool.sounds.patches.insert(refs.patch, Arc::new(patch));
            pool.sounds.mixes.insert(refs.mix, Arc::new(mix));
        }
        true
    }
}
