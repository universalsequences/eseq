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

#[derive(Clone, Debug)]
pub struct TrackPatternPool {
    pub patterns: HashMap<PatternId, TrackPatternData>,
    pub next_id: u64,
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

#[derive(Clone, Debug)]
pub struct Scene {
    pub id: SceneId,
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

#[derive(Clone, Debug)]
pub struct ProjectScenes {
    pub track_pools: Vec<TrackPatternPool>,
    pub scenes: Vec<Scene>,
    pub current_scene: usize,
    pub track_overrides: Vec<Option<PatternId>>,
    pub(super) next_scene_id: u64,
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
                id: SceneId(scene_idx as u64 + 1),
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
                id: SceneId(1),
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
            next_scene_id: u64::try_from(snapshots.len().max(1))
                .expect("scene count exceeds stable identity space")
                .checked_add(1)
                .expect("scene identity space exhausted"),
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
                // Bare track (takes spec 11.1): no pattern exists anywhere
                // for this lane — the pool is EMPTY, which distinguishes a
                // bare track from a deliberately cleared cell (cleared
                // cells must stay cleared). Materialize one lazily on the
                // first real content (any active step) so live edits
                // persist; an untouched bare track keeps its empty pool
                // and None cell.
                if self.track_pools[track].patterns.is_empty()
                    && data.track_bits.iter().any(|bits| *bits != 0)
                {
                    let id = self.track_pools[track].insert(data);
                    scene.cells[track] = Some(id);
                }
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
                let data = self.track_pools.get(track)?.get(id)?.clone();
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
        let next_id = self.next_scene_id;
        self.next_scene_id = self
            .next_scene_id
            .checked_add(1)
            .expect("scene identity space exhausted");
        self.scenes.push(Scene {
            id: SceneId(next_id),
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

    pub(super) fn edit_other_track_patterns<F>(&mut self, track: usize, mut edit: F) -> bool
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
