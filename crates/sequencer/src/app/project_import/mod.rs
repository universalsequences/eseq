//! Controlled import from another project (docs/rig-spec.md §8, eseq-ka5t.14).
//!
//! **Parse.** The source file is read and run through the loader's own
//! file-level migrations (`prepare_project_file_for_load`) into an
//! [`ImportSource`]; nothing of it reaches the running App until an import
//! is asked for, and browsing it (`ImportSource::peek`) is a pure read.
//!
//! **Import pipeline** (`App::import_from_project`), one undo step:
//!
//! 1. *Plan*: the picks (kits, plain groups, plain tracks, whole banks,
//!    project-owned sequencers) are checked against the names the peek
//!    showed, then resolve to source kits, groups and tracks. A bank brings
//!    every kit and track it sounds.
//! 2. *Kits*: each source kit is extracted as the kit format
//!    (`extract::extract_kit`) and installed by the kit loader
//!    (`App::load_kit_preset_as_rack`): new tracks, new bus, new rack-owned
//!    instances owned by the new kit, its whole clip bank and clip grooves.
//! 3. *Tracks and groups*: each plain track is rebuilt from its Sound (as a
//!    kit pad is) and appended; a plain group becomes a new group over the
//!    imported members, with its bus chain. Picked project-owned sequencers
//!    become new project-owned instances (`sequencers`).
//! 4. *Scenes* (`place`, one recorded scene-structure edit): the target
//!    bank is the viewed bank, or a new bank the import appends. A plain
//!    track's lanes go into its new pool; target scenes get a cell, every
//!    other scene none. Kits point at a clip in target scenes and are parked
//!    everywhere else. A bank import appends one bare scene per source scene.
//! 5. *Links* (`links`): every source track, bus and instance id maps to the
//!    one the import created. Outputs, sends, process bus sends, sidechains,
//!    mod cables, slot sets and sequencer routes follow the map; anything
//!    pointing at something not imported is cut and listed in the summary,
//!    never pointed at whatever has the same index here.
//!
//! **Assets** are shared by file through `assets::ImportSampleCache`.

mod assets;
mod extract;
mod links;
mod place;
mod sequencers;
mod source;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};

pub(crate) use assets::ImportSampleCache;
pub use links::Lines;
pub use source::{
    ImportSource, PeekBank, PeekGroup, PeekKit, PeekScene, PeekSequencer, PeekTrack, ProjectPeek,
};

use super::App;
use crate::effects::HostControl;
use crate::project::ProjectTrackKind;
use crate::sequencer::SceneBankId;
use links::IdMap;
use place::ImportContext;

/// Where an import lands (rig spec §8, "One bank").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportTarget {
    /// An existing bank (`None`: the playing scene's).
    Bank(Option<SceneBankId>),
    /// A new bank the import appends, with one bare scene.
    #[default]
    NewBank,
}

/// What a pick names in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PickKind {
    Kit,
    Group,
    Track,
    Bank,
    Sequencer,
}

impl PickKind {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "kit" => Self::Kit,
            "group" => Self::Group,
            "track" => Self::Track,
            "bank" => Self::Bank,
            "sequencer" => Self::Sequencer,
            _ => return None,
        })
    }
}

/// What to import from a source project. Ids and indices are the SOURCE's.
#[derive(Clone, Debug, Default)]
pub struct ImportRequest {
    /// Kits (drum rack group ids).
    pub kits: Vec<u64>,
    /// Plain groups (group ids): their member tracks and child kits.
    pub groups: Vec<u64>,
    /// Tracks by index. A kit pad brings its whole kit.
    pub tracks: Vec<usize>,
    /// Whole banks (bank ids): each becomes a new bank.
    pub banks: Vec<u64>,
    /// Project-owned sequencers (instance ids).
    pub sequencers: Vec<u64>,
    /// Where the kits and tracks are on (a bank import makes its own bank).
    pub target: ImportTarget,
    /// The name each pick had when it was picked (the peek's). A pick whose
    /// source now names something else refuses the import: the file changed
    /// since the peek.
    pub expected_names: Vec<(PickKind, u64, String)>,
}

impl ImportRequest {
    pub fn is_empty(&self) -> bool {
        self.kits.is_empty()
            && self.groups.is_empty()
            && self.tracks.is_empty()
            && self.banks.is_empty()
            && self.sequencers.is_empty()
    }
}

/// What an import did, for the summary the user sees.
#[derive(Clone, Debug, Default)]
pub struct ImportSummary {
    pub project: String,
    /// The bank the import is on in.
    pub bank: Option<SceneBankId>,
    pub bank_label: String,
    pub new_bank: bool,
    /// Source group id -> new group id (kits and plain groups).
    pub groups: Vec<(u64, u64)>,
    /// Source track -> new track (kit pads included).
    pub tracks: Vec<(usize, usize)>,
    /// Source scene -> new scene (bank imports).
    pub scenes: Vec<(usize, usize)>,
    /// Source instance id -> new instance id.
    pub instances: Vec<(u64, u64)>,
    /// Links to things that were not imported, left unconnected.
    pub unconnected: Lines,
    /// Everything else worth knowing (migration notes, limitations).
    pub notes: Lines,
}

impl ImportSummary {
    /// One status line.
    pub fn message(&self) -> String {
        let groups = self.groups.len();
        let tracks = self.tracks.len();
        let mut text = format!(
            "Imported {tracks} track{} ({groups} group{}) from '{}' into bank {}{}",
            if tracks == 1 { "" } else { "s" },
            if groups == 1 { "" } else { "s" },
            self.project,
            self.bank_label,
            if self.new_bank { " (new)" } else { "" },
        );
        if !self.unconnected.is_empty() {
            text.push_str(&format!("; {} link(s) left unconnected", self.unconnected.len()));
        }
        text
    }

    /// Every line of the summary: the status line, then each unconnected
    /// link and note.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![self.message()];
        lines.extend(self.unconnected.iter().map(|line| format!("Unconnected: {line}")));
        lines.extend(self.notes.iter().cloned());
        lines
    }
}

/// A plain group the plan brings, with the members that come along.
#[derive(Debug)]
struct PlannedGroup {
    id: u64,
    tracks: Vec<usize>,
    kits: Vec<u64>,
}

/// The resolved picks, in source ids.
#[derive(Debug, Default)]
pub(super) struct Plan {
    kits: Vec<u64>,
    groups: Vec<PlannedGroup>,
    tracks: Vec<usize>,
    banks: Vec<u64>,
    sequencers: Vec<u64>,
    notes: Vec<String>,
}

impl Plan {
    fn is_empty(&self) -> bool {
        self.kits.is_empty()
            && self.groups.is_empty()
            && self.tracks.is_empty()
            && self.banks.is_empty()
            && self.sequencers.is_empty()
    }
}

/// The name a pick shows in the peek (see `ImportRequest::expected_names`).
fn pick_name(source: &ImportSource, kind: PickKind, id: u64) -> Option<String> {
    match kind {
        PickKind::Kit | PickKind::Group => source.group(id).map(|group| group.name.clone()),
        PickKind::Track => {
            (usize::try_from(id).ok()? < source.file.tracks.len()).then(|| source.track_name(id as usize))
        }
        PickKind::Bank => source
            .banks()
            .iter()
            .position(|(bank, _, _)| *bank == id)
            .map(crate::sequencer::scene_bank_auto_label),
        PickKind::Sequencer => source.file.instances.get(id).map(|instance| instance.label.clone()),
    }
}

fn plan(source: &ImportSource, request: &ImportRequest) -> Result<Plan, String> {
    for (kind, id, expected) in &request.expected_names {
        if pick_name(source, *kind, *id).as_deref() != Some(expected.as_str()) {
            return Err(format!(
                "'{}' changed since it was peeked ('{expected}' is not where it was); peek it again",
                source.name
            ));
        }
    }
    let mut kits: BTreeSet<u64> = BTreeSet::new();
    let mut tracks: BTreeSet<usize> = BTreeSet::new();
    let mut groups: BTreeSet<u64> = BTreeSet::new();
    let mut notes = Vec::new();
    let usable = |track: usize| {
        source
            .file
            .tracks
            .get(track)
            .is_some_and(|track| !matches!(track.kind, ProjectTrackKind::Empty))
    };
    for gid in &request.kits {
        let group = source.group(*gid).ok_or_else(|| format!("The source has no kit {gid}"))?;
        if !group.is_rack() {
            return Err(format!("'{}' is a group, not a kit", group.name));
        }
        kits.insert(*gid);
    }
    for gid in &request.groups {
        let group = source.group(*gid).ok_or_else(|| format!("The source has no group {gid}"))?;
        if group.is_rack() {
            kits.insert(*gid);
            continue;
        }
        groups.insert(*gid);
        tracks.extend(group.members.iter().copied().filter(|track| usable(*track)));
        kits.extend(group.rack_members.iter().copied());
    }
    for track in &request.tracks {
        if *track >= source.file.tracks.len() {
            return Err(format!("The source has no track {}", track + 1));
        }
        if let Some(kit) = source.kit_of_track(*track) {
            notes.push(format!(
                "'{}' is a pad of kit '{}', which comes along whole",
                source.track_name(*track),
                source.group(kit).map(|group| group.name.as_str()).unwrap_or("?")
            ));
            kits.insert(kit);
        } else if usable(*track) {
            tracks.insert(*track);
        } else {
            notes.push(format!("'{}' is an empty track and was skipped", source.track_name(*track)));
        }
    }
    for bank in &request.banks {
        let range = source
            .bank_range(*bank)
            .ok_or_else(|| format!("The source has no bank {bank}"))?;
        for group in &source.file.groups {
            if group.is_rack() && range.clone().any(|scene| source.kit_plays_in_scene(scene, group.id)) {
                kits.insert(group.id);
            }
        }
        for track in 0..source.file.tracks.len() {
            if source.kit_of_track(track).is_none()
                && usable(track)
                && range.clone().any(|scene| source.cell_present(scene, track))
            {
                tracks.insert(track);
            }
        }
    }
    // A plain group comes along with the tracks it groups.
    for group in &source.file.groups {
        if !group.is_rack()
            && (group.members.iter().any(|track| tracks.contains(track))
                || group.rack_members.iter().any(|kit| kits.contains(kit)))
        {
            groups.insert(group.id);
        }
    }
    let mut sequencers = Vec::new();
    for id in &request.sequencers {
        let instance = source
            .file
            .instances
            .get(*id)
            .ok_or_else(|| format!("The source has no sequencer {id}"))?;
        if instance.owner.rack().is_some() {
            notes.push(format!("'{}' belongs to a kit and travels with it", instance.label));
        } else {
            sequencers.push(*id);
        }
    }
    let groups = groups
        .into_iter()
        .filter_map(|id| {
            let group = source.group(id)?;
            Some(PlannedGroup {
                id,
                tracks: group.members.iter().copied().filter(|track| tracks.contains(track)).collect(),
                kits: group.rack_members.iter().copied().filter(|kit| kits.contains(kit)).collect(),
            })
        })
        .collect();
    Ok(Plan {
        kits: kits.into_iter().collect(),
        groups,
        tracks: tracks.into_iter().collect(),
        banks: request.banks.clone(),
        sequencers,
        notes,
    })
}

/// A kit the import installed, with where its source scenes point.
pub(super) struct PlacedKit {
    source_group: u64,
    group: u64,
    /// Per source scene: the new clip id it plays.
    scene_clip: Vec<Option<u64>>,
    /// The clip a target scene plays when no source scene maps onto it.
    default_clip: Option<u64>,
}

/// Notes about what an import of `source` does not carry.
fn source_notes(source: &ImportSource, plan: &Plan) -> Vec<String> {
    let mut notes = Vec::new();
    if !source.file.macros.is_empty() {
        notes.push(format!(
            "{} project macro{} not imported",
            source.file.macros.len(),
            if source.file.macros.len() == 1 { " was" } else { "s were" }
        ));
    }
    if source.file.version < crate::project::SCENE_SLOT_ENABLE_VERSION {
        notes.push(format!(
            "'{}' is a version {} project: its scenes carry no slot sets, so the imported \
             lanes' stored slot flags decide what plays (the loader's row split is not run)",
            source.name, source.file.version
        ));
    }
    // Scenes that shared one sound arrive as separate copies.
    let shared = plan.tracks.iter().any(|track| {
        source.file.track_sounds.get(*track).is_some_and(|sounds| {
            let refs: Vec<(u64, u64)> = sounds.patterns.iter().flatten().map(|r| (r.patch, r.mix)).collect();
            let unique: BTreeSet<(u64, u64)> = refs.iter().copied().collect();
            unique.len() < refs.len()
        })
    });
    if shared {
        notes.push("Scenes that shared one sound arrive with a copy each".to_string());
    }
    notes
}

/// The refusal of an import that would change nothing.
const NOTHING_IMPORTED: &str = "The import changed nothing";

impl App {
    /// Import the picks of `request` from `source` (rig spec §8) as ONE undo
    /// step. On error nothing stays behind.
    pub fn import_from_project(
        &mut self,
        source: &ImportSource,
        request: &ImportRequest,
    ) -> Result<ImportSummary, String> {
        self.import_from_project_then(source, request, |_, _| Vec::new())
    }

    /// [`App::import_from_project`], running `finish` (the host's half: rack
    /// scripts, package attaches) before the undo entry closes, so whatever
    /// it records joins the one step. Its notes join the summary.
    pub fn import_from_project_then(
        &mut self,
        source: &ImportSource,
        request: &ImportRequest,
        finish: impl FnOnce(&mut App, &ImportSummary) -> Vec<String>,
    ) -> Result<ImportSummary, String> {
        if request.is_empty() {
            return Err("Pick a kit, group, track or bank to import".to_string());
        }
        super::edit::finish_active_gesture(self);
        let checkpoint = self.history.clone();
        let history_len = self.history.undo_len();
        self.begin_import_sample_sharing();
        let result = self.import_from_project_inner(source, request);
        self.end_import_sample_sharing();
        // Nothing recorded means nothing to undo: refuse BEFORE the host's
        // `finish` (rack scripts, package attaches) leaves effects that no
        // undo entry would carry.
        let result = result.and_then(|summary| {
            if self.history.undo_len() > history_len {
                Ok(summary)
            } else {
                Err(NOTHING_IMPORTED.to_string())
            }
        });
        match result {
            Ok(mut summary) => {
                let notes = finish(self, &summary);
                summary.notes.extend(notes);
                let label = format!("Import from {}", source.name);
                match self.history.undo_len().saturating_sub(history_len) {
                    // Refused above as NOTHING_IMPORTED; relabelling here
                    // would rename an unrelated earlier undo entry.
                    0 => debug_assert!(false, "an import that recorded nothing reached finish"),
                    1 => self.history.relabel_last(label),
                    _ => {
                        super::edit::squash_history_since(self, history_len, label);
                    }
                }
                self.sync_expr_process_classes_after_import();
                Ok(summary)
            }
            Err(error) => match super::edit::rollback_history_to(self, checkpoint) {
                Ok(()) => Err(error),
                Err(rollback) => Err(format!("Import failed ({error}); undoing it also failed ({rollback:?})")),
            },
        }
    }

    fn import_from_project_inner(
        &mut self,
        source: &ImportSource,
        request: &ImportRequest,
    ) -> Result<ImportSummary, String> {
        let plan = plan(source, request)?;
        if plan.is_empty() {
            // Every pick was skipped (an empty track, a kit's own
            // sequencer): stop before the placement records an empty step.
            return Err(std::iter::once(NOTHING_IMPORTED.to_string())
                .chain(plan.notes.iter().cloned())
                .collect::<Vec<_>>()
                .join("; "));
        }
        let mut summary = ImportSummary { project: source.name.clone(), ..Default::default() };
        summary.notes.extend(source.notes.iter().cloned());
        summary.notes.extend(plan.notes.iter().cloned());
        summary.notes.extend(source_notes(source, &plan));

        let pad_total: usize = plan
            .kits
            .iter()
            .filter_map(|gid| source.group(*gid))
            .map(|group| group.members.len())
            .sum();
        if self.tracks.len() + pad_total + plan.tracks.len() > crate::sequencer::MAX_TRACKS {
            return Err(format!(
                "Importing {} tracks would pass the {}-track limit",
                pad_total + plan.tracks.len(),
                crate::sequencer::MAX_TRACKS
            ));
        }

        let mut ids = IdMap { source_tracks: source.file.tracks.len(), ..Default::default() };
        let mut kits = Vec::new();
        for gid in &plan.kits {
            kits.push(self.import_kit(source, *gid, &mut ids, &mut summary)?);
        }
        // A plain track is rebuilt from the Sound of the scene it plays in
        // (the saved current scene, else its first), its lanes lift onto it.
        let sound_scene: HashMap<usize, usize> = plan
            .tracks
            .iter()
            .map(|track| {
                let current = source.current_scene();
                let scene = if source.cell_present(current, *track) {
                    current
                } else {
                    (0..source.scene_count())
                        .find(|scene| source.cell_present(*scene, *track))
                        .unwrap_or(current)
                };
                (*track, scene)
            })
            .collect();
        let mut sounds = HashMap::new();
        let mut plain = Vec::with_capacity(plan.tracks.len());
        for track in &plan.tracks {
            let (new, sound) = self.import_plain_track(source, *track, sound_scene[track])?;
            ids.tracks.insert(*track, new);
            summary.tracks.push((*track, new));
            plain.push((*track, new));
            if let Some(sound) = sound {
                sounds.insert(*track, sound);
            }
        }
        for group in &plan.groups {
            self.import_group(source, group, &mut ids, &mut summary)?;
        }
        ids.target_tracks = self.tracks.len();
        let mut cut = Lines::default();
        for bus in ids.buses.values().copied().collect::<Vec<_>>() {
            self.remap_imported_bus_sidechains(bus, source, &ids, &mut cut)?;
        }
        let mut sequencers = Vec::with_capacity(plan.sequencers.len());
        for id in &plan.sequencers {
            let imported = self.import_project_instance(source, *id)?;
            ids.instances.insert(imported.old, imported.new);
            summary.instances.push((imported.old, imported.new));
            sequencers.push(imported);
        }

        let current = self.state.current_scene_index();
        let context = ImportContext {
            source,
            plan: &plan,
            plain,
            sound_scene,
            sounds,
            kits,
            ids,
            sequencers,
            target: request.target,
        };
        let placement =
            self.apply_recorded_scene_structure_mutation("Import scenes", |app| app.place_import(&context))?;
        summary.bank = Some(placement.bank);
        summary.bank_label = self.bank_label(placement.bank);
        summary.new_bank = placement.new_bank;
        summary.scenes = placement.scenes;
        summary.unconnected.extend(cut.iter().cloned());
        summary.unconnected.extend(placement.cut.iter().cloned());
        // The playing scene picks the new lanes up: the pins `place_import`
        // set made the save-back land on what the live grid was playing.
        debug_assert_eq!(self.state.current_scene_index(), current, "an import never moves the playing scene");
        self.apply_pattern_launch(&crate::quantized_launch::PatternLaunchTarget::Scene { scene: current })
            .map_err(|error| format!("Could not relaunch the playing scene: {error:?}"))?;
        self.state.publish_scheduler_snapshot();
        Ok(summary)
    }

    /// Step 2: one source kit through the kit loader.
    fn import_kit(
        &mut self,
        source: &ImportSource,
        source_group: u64,
        ids: &mut IdMap,
        summary: &mut ImportSummary,
    ) -> Result<PlacedKit, String> {
        let extracted = extract::extract_kit(source, source_group)?;
        let name = extracted.preset.metadata.name.clone();
        summary.notes.extend(extracted.warnings.iter().map(|w| format!("{name}: {w}")));
        let pad_notes: Vec<i32> = extracted.preset.pads.iter().map(|pad| pad.pad_note).collect();
        let clip_count = extracted.preset.clips.len();
        let (group_id, failures, instance_map) = self.load_kit_preset_as_rack(extracted.preset, &name)?;
        summary.notes.extend(failures.into_iter().map(|failure| format!("{name}: {failure}")));
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Kit '{name}' did not load"))?;
        let rack = group.rack.as_ref().ok_or_else(|| format!("Kit '{name}' is not a rack"))?;
        for (pad, note) in pad_notes.iter().enumerate() {
            let Some(index) = rack.pad_index_for_note(*note) else {
                continue;
            };
            let Some(track) = group.members.get(rack.pads[index].member).copied() else {
                continue;
            };
            if let Some(source_track) = extracted.pad_tracks.get(pad) {
                ids.tracks.insert(*source_track, track);
                summary.tracks.push((*source_track, track));
            }
        }
        if let Some(source_bus) = source.group(source_group).map(|group| group.bus_id) {
            ids.buses.insert(source_bus, group.bus_id);
        }
        // Package instances by the loader's own map (plain rack scripts are
        // keyed by their re-derived ids, not instance ids).
        let owned: BTreeSet<u64> = self.rack_instances(group_id).iter().map(|instance| instance.id).collect();
        for (old, new) in instance_map {
            if owned.contains(&new) {
                ids.instances.insert(old, new);
                summary.instances.push((old, new));
            }
        }
        summary.instances.sort_unstable();
        summary.groups.push((source_group, group_id));
        // The ids the loader gave the kit's clips: its bank holds one clip
        // per kit clip, in kit order. A kit whose clips failed to install
        // has no bank, so nothing points at a clip it lacks.
        let installed: Vec<u64> = self.state.with_scenes(|scenes| {
            scenes
                .rack_bank(group_id)
                .filter(|bank| bank.clips.len() == clip_count)
                .map(|bank| bank.clips.iter().map(|clip| clip.id).collect())
                .unwrap_or_default()
        });
        let clip_id = |index: usize| installed.get(index).copied();
        // Clips that play their own groove keep it (a copy enters the pool).
        if !extracted.clip_grooves.is_empty() {
            let grooves = extracted.clip_grooves;
            self.apply_recorded_bus_group_structure_mutation("Import clip grooves", move |app| {
                let mut entries = Vec::with_capacity(grooves.len());
                for (index, groove, settings) in &grooves {
                    let Some(clip) = clip_id(*index) else {
                        continue;
                    };
                    let mut settings = match groove {
                        Some(groove) => {
                            super::rack_grooves::kit_groove_settings(&mut app.grooves, Some(groove))
                        }
                        None => settings.clone(),
                    };
                    if groove.is_none() {
                        settings.active = None;
                    }
                    entries.push(crate::project::RackClipGroove { clip, settings });
                }
                app.rack_config_mut(group_id)?.clip_grooves = entries;
                Ok(())
            })?;
        }
        let scene_clip: Vec<Option<u64>> =
            extracted.scene_clip.iter().map(|clip| clip.and_then(clip_id)).collect();
        let default_clip = scene_clip
            .get(source.current_scene())
            .copied()
            .flatten()
            .or_else(|| scene_clip.iter().flatten().next().copied())
            .or_else(|| clip_id(0));
        Ok(PlacedKit { source_group, group: group_id, scene_clip, default_clip })
    }

    /// Step 3: one plain track, rebuilt from its Sound at `scene` (a kit
    /// pad's route). Returns the new track and the Sound its lanes lift
    /// onto (`None` for a modulator).
    fn import_plain_track(
        &mut self,
        source: &ImportSource,
        track: usize,
        scene: usize,
    ) -> Result<(usize, Option<crate::project::ProjectSoundPreset>), String> {
        let name = source.track_name(track);
        let pattern = &source.file.patterns[scene];
        let kind = &source.file.tracks[track].kind;
        let (new, sound) = match kind {
            ProjectTrackKind::Modulator => {
                let new = self.graph_controller().add_modulator_track()?;
                self.tracks[new] = name.clone();
                self.apply_kit_modulator_settings(
                    new,
                    &pattern.instrument_slots.get(track).cloned().unwrap_or_default(),
                );
                (new, None)
            }
            ProjectTrackKind::Empty => return Err(format!("'{name}' is an empty track")),
            _ => {
                let sound = super::projects::sound_preset_from_project(
                    &source.file,
                    track,
                    pattern,
                    &name,
                    Vec::new(),
                    String::new(),
                )?;
                (self.add_track_from_sound_preset(sound.clone(), &name)?, Some(sound))
            }
        };
        if let Some(authored) = self.track_name_user_authored.get_mut(new) {
            *authored = true;
        }
        if let (Some(color), Some(slot)) = (source.file.tracks[track].color(), self.track_colors.get_mut(new)) {
            *slot = color;
        }
        self.commit_created_track(new, "Import track")?;
        Ok((new, sound))
    }

    /// Step 3: a plain group over the imported members, with its bus chain
    /// (restored as a recorded bus effect edit, so undo and redo keep it).
    fn import_group(
        &mut self,
        source: &ImportSource,
        planned: &PlannedGroup,
        ids: &mut IdMap,
        summary: &mut ImportSummary,
    ) -> Result<(), String> {
        let Some(group) = source.group(planned.id) else {
            return Ok(());
        };
        let new_members: Vec<usize> = planned.tracks.iter().filter_map(|t| ids.tracks.get(t).copied()).collect();
        let new_groups: HashMap<u64, u64> = summary.groups.iter().copied().collect();
        let new_kits: Vec<u64> = planned.kits.iter().filter_map(|kit| new_groups.get(kit).copied()).collect();
        if new_members.len() + new_kits.len() < 2 {
            summary.notes.push(format!(
                "Group '{}' was not recreated: only one of its members came along",
                group.name
            ));
            return Ok(());
        }
        let bus = self.group_tracks_and_racks_recorded(new_members, new_kits)?;
        let (group_id, name, color) = self
            .groups
            .iter()
            .find(|candidate| candidate.bus_id == bus.0)
            .map(|candidate| (candidate.id, candidate.name.clone(), candidate.color))
            .ok_or_else(|| format!("Group '{}' did not form", group.name))?;
        if name != group.name {
            self.rename_group_recorded(group_id, group.name.clone())?;
        }
        if color != group.color {
            self.set_group_color_recorded(
                group_id,
                crate::track_color::TrackColor::new(group.color[0], group.color[1], group.color[2]),
            )?;
        }
        if let Some(chain) = extract::group_bus_chain(source, group.bus_id) {
            if !chain.effects.is_empty() {
                if let Err(error) = self.restore_kit_bus_chain(bus.0, &chain) {
                    summary.notes.push(format!("Group '{}' bus effects: {error}", group.name));
                }
            }
        }
        ids.buses.insert(group.bus_id, bus.0);
        summary.groups.push((planned.id, group_id));
        Ok(())
    }

    /// An imported bus's effect sidechains (restored with the source's
    /// selections) re-pointed at the imported tracks, as one recorded bus
    /// effect edit; a source track that did not come along selects nothing.
    fn remap_imported_bus_sidechains(
        &mut self,
        bus_id: u64,
        source: &ImportSource,
        ids: &IdMap,
        cut: &mut Lines,
    ) -> Result<(), String> {
        let Some(bus_idx) = self.buses.iter().position(|bus| bus.id.0 == bus_id) else {
            return Ok(());
        };
        let bus = &self.buses[bus_idx];
        let who = bus.name.clone();
        let mut changes = Vec::new();
        for (slot, descriptor) in bus.effect_descriptors.iter().enumerate() {
            let Some(values) = bus.effect_slots.get(slot) else {
                continue;
            };
            for (param, desc) in descriptor.params.iter().enumerate() {
                if !matches!(desc.host_control, Some(HostControl::FxSidechain { .. })) {
                    continue;
                }
                let old = values.defaults.get(param).copied().unwrap_or(0.0).round().max(0.0) as usize;
                let new = links::remap_bus_sidechain(old, ids, source, &who, cut);
                if new != old {
                    changes.push((slot, param, new));
                }
            }
        }
        if changes.is_empty() {
            return Ok(());
        }
        self.apply_recorded_bus_effect_chain_mutation(bus_idx, "Import bus sidechains", move |app| {
            for (slot, param, selection) in changes {
                app.apply_bus_effect_sidechain_selection(bus_idx, slot, param, selection);
                if let Some(value) = app.buses[bus_idx].effect_slots[slot].defaults.get_mut(param) {
                    *value = selection as f32;
                }
            }
            app.save_current_bus_pattern();
            Ok(())
        })
    }

    /// `A`, `B`, ... for `bank` by its position.
    fn bank_label(&self, bank: SceneBankId) -> String {
        self.state.with_project_scenes(|scenes| {
            scenes
                .scene_banks()
                .iter()
                .position(|candidate| candidate.id == bank)
                .map(crate::sequencer::scene_bank_auto_label)
                .unwrap_or_default()
        })
    }
}
