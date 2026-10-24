//! Scene-side primitives for controlled import (docs/rig-spec.md §8): the
//! bare scenes an import builds a new bank from.

use super::*;

impl ProjectScenes {
    /// Append a scene with no cells to `bank`, which must be the LAST bank
    /// (an import appends its new bank at the end, so no existing scene
    /// index moves). Every existing track is bare there and keeps sounding
    /// like its current cell when a pattern is later dropped in (its cell
    /// sound forks the current one, as a new scene's does); every kit is
    /// silent (no clip pointer). Mixer-wide scene state (bus chains, cables,
    /// project-owned sequencer overrides, scene slots, the project process
    /// chain) is copied from `template`. Returns the new scene's index; the
    /// current scene does not change.
    pub fn append_bare_scene(&mut self, bank: SceneBankId, template: usize) -> Result<usize, String> {
        let last = self
            .banks
            .last()
            .ok_or_else(|| "The project has no scene bank".to_string())?;
        if last.id != bank {
            return Err(format!("Scene bank {} is not the last bank", bank.0));
        }
        if last.len >= MAX_SCENES_PER_BANK {
            return Err(format!(
                "Scene bank {} already contains {MAX_SCENES_PER_BANK} scenes",
                bank.0
            ));
        }
        let template = self.scenes.get(template).cloned();
        let track_count = self.track_pools.len();
        let mut cell_sounds = Vec::with_capacity(track_count);
        for track in 0..track_count {
            let refs = template
                .as_ref()
                .and_then(|scene| scene.cell_sounds.get(track).copied())
                .or_else(|| self.effective_sound_refs(track));
            let sounds = &mut self.track_pools[track].sounds;
            cell_sounds.push(match refs {
                Some(refs) => sounds.fork(refs),
                None => sounds.insert(Patch::new_default(), Mix::default()),
            });
        }
        let (bus_patterns, mod_connections, neural_networks, graph_overrides, scene_slots, mut project_process_chain) =
            template
                .map(|scene| {
                    (
                        scene.bus_patterns,
                        scene.mod_connections,
                        scene.neural_networks,
                        scene
                            .graph_overrides
                            .into_iter()
                            .filter(|graph| graph.owner_rack.is_none())
                            .collect(),
                        scene.scene_slots,
                        scene.project_process_chain,
                    )
                })
                .unwrap_or_default();
        crate::process::ensure_default_project_layer(&mut project_process_chain);
        let id = self.next_scene_id;
        self.next_scene_id = self
            .next_scene_id
            .checked_add(1)
            .ok_or_else(|| "Scene identity space is exhausted".to_string())?;
        let index = self.scenes.len();
        self.scenes.push(Scene {
            id: SceneId(id),
            name: format!("Scene {}", index + 1),
            cells: vec![None; track_count],
            cell_sounds,
            bus_patterns,
            mod_connections,
            neural_networks,
            graph_overrides,
            rack_clips: Vec::new(),
            slot_enable: Default::default(),
            parked_kits: Default::default(),
            parked_instances: Default::default(),
            scene_slots,
            project_process_chain,
        });
        if let Some(last) = self.banks.last_mut() {
            last.len += 1;
        }
        Ok(index)
    }
}

impl SequencerState {
    /// `normalize_rack_slot_ids` for `tracks` only: controlled import gives
    /// the lanes it brought (legacy ones carry no slot ids) the live rack's
    /// ids, and leaves every other track as it was.
    pub fn normalize_rack_slot_ids_of(&self, tracks: &[usize]) -> bool {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let mut racks = self.pattern.rack_tracks.lock().unwrap();
        let mut changed = false;
        for track in tracks {
            let live = racks.get_mut(*track).and_then(Option::as_mut);
            changed |= scenes.normalize_rack_slot_ids(*track, live);
        }
        changed
    }
}
