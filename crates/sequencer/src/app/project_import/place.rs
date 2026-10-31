//! Step 4 and 5 of an import (docs/rig-spec.md §8, "One bank", "Links"):
//! where the imported tracks and kits are on, as ONE recorded
//! scene-structure edit. Existing scenes outside the target only ever get
//! "no cell" for an imported track and "parked" for an imported kit (plus
//! the routed-nowhere stand-in for picked project sequencers,
//! `sequencers.rs`).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::project::{ProjectModDestination, ProjectSoundPreset};
use crate::sequencer::{BusId, ModConnection, ModDestination, PatternId, SceneBankId, TrackPatternData};

use super::links::{bus_name, cut_cable_text, remap_lane_buses, remap_lane_sidechains, IdMap, Lines};
use super::sequencers::ImportedSequencer;
use super::source::ImportSource;
use super::{App, ImportTarget, PlacedKit, Plan};

/// One scene the import is on in, and the source scene it stands for
/// (`None`: no source scene maps onto it; each track plays its sound scene,
/// each kit its default clip).
#[derive(Clone, Copy, Debug)]
pub(super) struct Target {
    pub scene: usize,
    pub source_scene: Option<usize>,
}

/// Everything `place_import` reads.
pub(super) struct ImportContext<'a> {
    pub source: &'a ImportSource,
    pub plan: &'a Plan,
    /// Plain (non-kit) tracks: source -> new.
    pub plain: Vec<(usize, usize)>,
    /// Per plain source track: the scene its Sound came from.
    pub sound_scene: HashMap<usize, usize>,
    /// The Sound each plain track was rebuilt from (lanes lift onto it).
    pub sounds: HashMap<usize, ProjectSoundPreset>,
    pub kits: Vec<PlacedKit>,
    pub ids: IdMap,
    pub sequencers: Vec<ImportedSequencer>,
    pub target: ImportTarget,
}

impl ImportContext<'_> {
    fn sound_scene(&self, track: usize) -> usize {
        self.sound_scene.get(&track).copied().unwrap_or_else(|| self.source.current_scene())
    }
}

/// What `place_import` decided.
pub(super) struct Placement {
    pub bank: SceneBankId,
    pub new_bank: bool,
    pub scenes: Vec<(usize, usize)>,
    pub cut: Lines,
}

impl App {
    /// The target scenes: a new bank per picked source bank (one scene per
    /// source scene; an empty source bank makes none), a new bank of one
    /// bare scene, or the scenes of an existing bank.
    fn import_targets(&mut self, cx: &ImportContext) -> Result<(SceneBankId, bool, Vec<Target>), String> {
        let current = self.state.current_scene_index();
        let mut targets = Vec::new();
        if !cx.plan.banks.is_empty() {
            let mut first = None;
            for source_bank in &cx.plan.banks {
                let range = cx.source.bank_range(*source_bank).unwrap_or(0..0);
                if range.is_empty() {
                    continue;
                }
                let bank = self.state.with_scenes_mut(|scenes| scenes.create_scene_bank())?;
                first.get_or_insert(bank);
                for scene in range {
                    let index =
                        self.state.with_scenes_mut(|scenes| scenes.append_bare_scene(bank, current))?;
                    targets.push(Target { scene: index, source_scene: Some(scene) });
                }
            }
            let bank = first.ok_or_else(|| "The picked bank has no scenes".to_string())?;
            return Ok((bank, true, targets));
        }
        match cx.target {
            ImportTarget::NewBank => {
                let bank = self.state.with_scenes_mut(|scenes| scenes.create_scene_bank())?;
                let index = self.state.with_scenes_mut(|scenes| scenes.append_bare_scene(bank, current))?;
                targets.push(Target { scene: index, source_scene: None });
                Ok((bank, true, targets))
            }
            ImportTarget::Bank(bank) => {
                let (bank, range) = self.bank_scene_range(bank)?;
                targets.extend(range.map(|scene| Target { scene, source_scene: None }));
                Ok((bank, false, targets))
            }
            ImportTarget::Mirror => {
                let (bank, _) = self.bank_scene_range(None)?;
                let count = self.state.with_project_scenes(|scenes| scenes.scenes.len());
                if count != cx.source.scene_count() {
                    return Err("The project's scenes changed while it was copied".to_string());
                }
                targets.extend((0..count).map(|scene| Target { scene, source_scene: Some(scene) }));
                Ok((bank, false, targets))
            }
        }
    }

    /// The source scene's lanes of `tracks` (source -> new), converted into
    /// this project's lanes, links and sidechains remapped.
    fn import_scene_lanes(
        &mut self,
        cx: &ImportContext,
        scene: usize,
        tracks: &[(usize, usize)],
        cut: &mut Lines,
    ) -> Result<Vec<(usize, TrackPatternData)>, String> {
        let mut lane_map = vec![None; cx.source.file.tracks.len()];
        for (old, new) in tracks {
            lane_map[*old] = Some(*new);
        }
        let mut pattern = cx.source.file.patterns[scene].lanes_to(&lane_map, self.tracks.len());
        for (old, new) in tracks {
            if let Some(sound) = cx.sounds.get(old) {
                super::super::break_kits::lift_plain_lane(&mut pattern, *new, sound);
            }
        }
        let mut assets = self.take_import_sample_assets();
        let converted = self.project_pattern_into_snapshot_with_policy(pattern, &mut assets, false);
        self.return_import_sample_assets(assets);
        let (snapshot, _, _) = converted?;
        Ok(tracks
            .iter()
            .filter_map(|(old, new)| {
                let mut lane = snapshot.track_pattern_data(*new)?;
                let who = cx.source.track_name(*old);
                remap_lane_buses(&mut lane, cx.source, &cx.ids, &who, true, cut);
                let descriptors = self.graph.effect_descriptors.get(*new).cloned().unwrap_or_default();
                remap_lane_sidechains(&mut lane, &descriptors, (*old, *new), &cx.ids, cx.source, &who, cut);
                Some((*old, lane))
            })
            .collect())
    }

    /// Step 4 and 5, inside one recorded scene-structure edit.
    pub(super) fn place_import(&mut self, cx: &ImportContext) -> Result<Placement, String> {
        let mut cut = Lines::default();
        let source = cx.source;
        let new_tracks: Vec<usize> = cx.ids.tracks.values().copied().collect();
        // Pin what the playing scene's new lanes play now, so the save-back
        // that closes this edit lands there and not on an imported lane.
        // Cells are written below WITHOUT `clear_cell` (which drops a pin).
        let pins: Vec<(usize, Option<PatternId>)> = self.state.with_scenes_mut(|scenes| {
            new_tracks
                .iter()
                .map(|track| {
                    let playing = scenes.effective_pattern_id(*track);
                    if let Some(slot) = scenes.track_overrides.get_mut(*track) {
                        if slot.is_none() {
                            *slot = playing;
                        }
                    }
                    (*track, playing)
                })
                .collect()
        });
        let (bank, new_bank, targets) = self.import_targets(cx)?;
        let target_set: HashSet<usize> = targets.iter().map(|target| target.scene).collect();

        // Plain lanes: every source cell of every imported plain track goes
        // into its new pool.
        let mut pool: BTreeMap<(usize, usize), PatternId> = BTreeMap::new();
        if !cx.plain.is_empty() {
            for scene in 0..source.scene_count() {
                let present: Vec<(usize, usize)> = cx
                    .plain
                    .iter()
                    .copied()
                    .filter(|(old, _)| source.cell_present(scene, *old))
                    .collect();
                if present.is_empty() {
                    continue;
                }
                let lanes = self.import_scene_lanes(cx, scene, &present, &mut cut)?;
                self.state.with_scenes_mut(|scenes| {
                    for (old, lane) in lanes {
                        let new = cx.ids.tracks[&old];
                        if let Some(track_pool) = scenes.track_pools.get_mut(new) {
                            pool.insert((scene, old), track_pool.insert(lane));
                        }
                    }
                });
            }
        }

        // Kit clip lanes still address the source's buses and tracks: their
        // sends follow the import (the kit's own bus included), anything
        // else is cut; their sidechains move to the imported tracks.
        let new_to_old: HashMap<usize, usize> = cx.ids.tracks.iter().map(|(old, new)| (*new, *old)).collect();
        let descriptors: HashMap<usize, Vec<crate::effects::EffectDescriptor>> = new_tracks
            .iter()
            .map(|track| (*track, self.graph.effect_descriptors.get(*track).cloned().unwrap_or_default()))
            .collect();
        self.state.with_scenes_mut(|scenes| {
            for kit in &cx.kits {
                let Some(bank) = scenes.rack_bank(kit.group) else {
                    continue;
                };
                let cells: Vec<(usize, PatternId)> = bank
                    .clips
                    .iter()
                    .flat_map(|clip| clip.cells.iter().enumerate())
                    .filter_map(|(position, cell)| Some((*bank.members.get(position)?, (*cell)?)))
                    .collect();
                for (track, cell) in cells {
                    let Some(old) = new_to_old.get(&track).copied() else {
                        continue;
                    };
                    let who = source.track_name(old);
                    let empty = Vec::new();
                    let descriptors = descriptors.get(&track).unwrap_or(&empty);
                    scenes.track_pools[track].edit(cell, |lane| {
                        remap_lane_buses(lane, source, &cx.ids, &who, false, &mut cut);
                        remap_lane_sidechains(lane, descriptors, (old, track), &cx.ids, source, &who, &mut cut);
                    });
                }
            }
        });

        let plain_set: HashSet<usize> = cx.plain.iter().map(|(old, _)| *old).collect();
        self.state.with_scenes_mut(|scenes| {
            // Imported plain tracks hold no cell outside the target (the
            // cell is written directly: `clear_cell` would drop the pin).
            for (_, new) in &cx.plain {
                for scene in &mut scenes.scenes {
                    if let Some(cell) = scene.cells.get_mut(*new) {
                        *cell = None;
                    }
                }
            }
            for target in &targets {
                for (old, new) in &cx.plain {
                    let from = target.source_scene.unwrap_or_else(|| cx.sound_scene(*old));
                    if let Some(id) = pool.get(&(from, *old)) {
                        scenes.set_cell(target.scene, *new, *id);
                    }
                }
            }
            // Kits: a clip in target scenes, parked everywhere else (the
            // loader left every pointer empty).
            for kit in &cx.kits {
                for target in &targets {
                    let clip = match target.source_scene {
                        Some(old) => kit.scene_clip.get(old).copied().flatten(),
                        None => kit.default_clip,
                    };
                    scenes.set_scene_rack_clip(target.scene, kit.group, clip);
                    let parked = target
                        .source_scene
                        .is_some_and(|old| source.kit_parked(old, kit.source_group));
                    let scene = &mut scenes.scenes[target.scene];
                    if parked {
                        scene.parked_kits.insert(kit.group);
                    } else {
                        scene.parked_kits.remove(&kit.group);
                    }
                }
                for (index, scene) in scenes.scenes.iter_mut().enumerate() {
                    if !target_set.contains(&index) {
                        scene.parked_kits.insert(kit.group);
                    }
                }
            }
            // Who plays (rig spec §3.2): the source scene's slot set, where
            // it has one; otherwise the lanes' own flags decide.
            for target in &targets {
                for (old, new) in &cx.ids.tracks {
                    let from = target.source_scene.unwrap_or_else(|| {
                        if plain_set.contains(old) {
                            cx.sound_scene(*old)
                        } else {
                            source.current_scene()
                        }
                    });
                    let entry = source
                        .file
                        .scene_slot_enable
                        .get(from)
                        .and_then(|entries| entries.iter().find(|(track, _)| track == old));
                    let scene = &mut scenes.scenes[target.scene];
                    match entry {
                        Some((_, slots)) => {
                            scene.slot_enable.insert(*new, slots.iter().copied().collect());
                        }
                        None => {
                            scene.slot_enable.remove(new);
                        }
                    }
                }
            }
            // Mod cables between imported things, a kit's own included, in
            // target scenes only; the rest are cut.
            for target in &targets {
                let from = target.source_scene.unwrap_or_else(|| source.current_scene());
                for (connection, destination) in source.scene_mod_connections(from) {
                    // A mirror also copies the cables INTO an imported track.
                    let into_imported = cx.ids.same_project
                        && matches!(destination, Some(ProjectModDestination::Track(track))
                            if cx.ids.tracks.contains_key(&track));
                    if !cx.ids.tracks.contains_key(&connection.source_track) && !into_imported {
                        continue;
                    }
                    let Some(source_track) = cx.ids.track(connection.source_track) else {
                        continue;
                    };
                    let mapped = match destination {
                        Some(ProjectModDestination::Track(track)) => cx.ids.track(track).map(ModDestination::Track),
                        Some(ProjectModDestination::Bus(bus)) => {
                            cx.ids.bus(bus).map(|bus| ModDestination::Bus(BusId(bus)))
                        }
                        None => None,
                    };
                    let Some(mapped) = mapped else {
                        let to = match destination {
                            Some(ProjectModDestination::Track(track)) => source.track_name(track),
                            Some(ProjectModDestination::Bus(bus)) => bus_name(source, bus),
                            None => "nothing".to_string(),
                        };
                        cut.push(cut_cable_text(source, connection.source_track, &to));
                        continue;
                    };
                    let cable = ModConnection {
                        source_output: connection.source_output,
                        source_track,
                        destination: mapped,
                        dest_input: connection.dest_input,
                    };
                    let scene = &mut scenes.scenes[target.scene];
                    if !scene.mod_connections.contains(&cable) {
                        scene.mod_connections.push(cable);
                    }
                }
            }
            // The pins hold through the edit (see above).
            for (track, playing) in &pins {
                if let Some(slot) = scenes.track_overrides.get_mut(*track) {
                    if slot.is_none() {
                        *slot = *playing;
                    }
                }
            }
        });
        self.place_project_sequencers(source, &cx.sequencers, &cx.ids, &targets, new_bank, &mut cut);
        // Copied chains name the source's instances as owners: re-key them
        // to what came along, drop the rest (docs/rig-spec.md §10).
        self.state.remap_process_slot_owners(&new_tracks, &cx.ids.instances);
        // Process lane rosters travel with their tracks (eseq-53y7).
        let mut rosters = self.state.capture_track_lane_rosters();
        let mut roster_changed = false;
        for (old, new) in &cx.ids.tracks {
            if let (Some(roster), Some(slot)) = (source.file.track_lane_rosters.get(*old), rosters.get_mut(*new)) {
                if slot != roster {
                    *slot = roster.clone();
                    roster_changed = true;
                }
            }
        }
        if roster_changed {
            self.state.restore_track_lane_rosters(&rosters);
        }
        // Legacy lanes carry no slot ids: give them the live rack's.
        self.state.normalize_rack_slot_ids_of(&new_tracks);
        let scenes = targets
            .iter()
            .filter_map(|target| target.source_scene.map(|old| (old, target.scene)))
            .collect();
        Ok(Placement { bank, new_bank, scenes, cut })
    }
}
