//! A kit of another project as the kit format (docs/rig-spec.md §8, "Reuse"):
//! "import of a kit is that format read from a project file instead of a kit
//! file". Everything here is a pure read of the migrated `ProjectFile`; the
//! kit loader (`App::load_kit_preset_as_rack`) installs the result.
//!
//! The extraction mirrors `App::capture_rack_as_kit` (the live export) on
//! file data: pads in rack order (a modulator member travels as its
//! instrument slot, an empty one is left out), the rack bus chain, the rack's
//! plain scripts and package instances, its WHOLE clip bank in pad space
//! (not one clip per scene: the bank is shared, so a bank import can point
//! each new scene at the clip its source scene played), its groove and each
//! clip's own groove. A legacy rack (no clip bank) gets one clip per source
//! scene it sounds in, the conversion "Convert to clips" performs on a live
//! rack.
//!
//! Mod cables are NOT in the preset: the kit loader would write them into
//! every scene of the project, and an import is on in its target bank only.
//! The importer places every cable, the kit's own included, per target
//! scene (`place`).

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};

use crate::graph::ProjectGraphOverrides;
use crate::groove::KitGroove;
use crate::project::{
    ProjectKitBusChain, ProjectKitBusEffect, ProjectKitInstance, ProjectKitPad, ProjectKitPreset,
    ProjectPattern, ProjectRackClip, ProjectSoundMetadata, ProjectTrackKind,
};

use super::source::ImportSource;

/// A source kit as the kit format plus what the importer needs to map the
/// source's scenes onto the loaded rack.
pub(crate) struct ExtractedKit {
    pub preset: ProjectKitPreset,
    /// Per kit pad (preset order): the source member track.
    pub pad_tracks: Vec<usize>,
    /// Per source scene: the kit clip (index into `preset.clips`) it points
    /// at, parked or not (parking is carried separately), `None` when silent.
    pub scene_clip: Vec<Option<usize>>,
    /// Clips that play their own groove: (kit clip index, groove).
    pub clip_grooves: Vec<(usize, Option<KitGroove>, crate::groove::RackGrooveSettings)>,
    pub warnings: Vec<String>,
}

/// The source scene whose pattern is read for the state a pad's Sound
/// captures: the first clip `member` sounds in, else the saved current scene.
fn pad_sound_pattern<'a>(
    source: &'a ImportSource,
    clips: &'a [ProjectRackClip],
    member: usize,
) -> &'a ProjectPattern {
    clips
        .iter()
        .find(|clip| clip.members.get(member).copied().unwrap_or(false))
        .map(|clip| &clip.pattern)
        .unwrap_or_else(|| &source.file.patterns[source.current_scene()])
}

/// The source scenes a legacy rack sounds in, as clips (project space: the
/// scene pattern itself, members flagged by cell presence).
fn legacy_rack_clips(source: &ImportSource, group_id: u64) -> (Vec<ProjectRackClip>, Vec<Option<u64>>) {
    let Some(group) = source.group(group_id) else {
        return (Vec::new(), Vec::new());
    };
    let mut clips = Vec::new();
    let mut scene_clip = vec![None; source.scene_count()];
    for scene in 0..source.scene_count() {
        let members: Vec<bool> = group
            .members
            .iter()
            .map(|track| source.cell_present(scene, *track))
            .collect();
        if !members.iter().any(|present| *present) {
            continue;
        }
        let pattern = &source.file.patterns[scene];
        let id = clips.len() as u64 + 1;
        clips.push(ProjectRackClip {
            id,
            name: format!("Scene {}", scene + 1),
            color: None,
            members,
            graph_overrides: pattern
                .graph_overrides
                .iter()
                .filter(|graph| graph.owner_rack == Some(group_id))
                .cloned()
                .collect(),
            scene_slots: pattern.scene_slots.clone(),
            // Only the member lanes are read (in pad space, below).
            pattern: pattern.lanes_to(
                &(0..source.file.tracks.len()).map(Some).collect::<Vec<_>>(),
                source.file.tracks.len(),
            ),
            bus_chain: None,
        });
        scene_clip[scene] = Some(id);
    }
    (clips, scene_clip)
}

/// Extract kit `group_id` of `source` as a kit preset.
pub(crate) fn extract_kit(source: &ImportSource, group_id: u64) -> Result<ExtractedKit, String> {
    let file = &source.file;
    let group = source
        .group(group_id)
        .ok_or_else(|| format!("The source project has no group {group_id}"))?;
    let rack = group
        .rack
        .as_ref()
        .ok_or_else(|| format!("'{}' is not a kit", group.name))?;
    let mut warnings = Vec::new();

    // The clip bank (project space) and every source scene's pointer into it.
    let (clips, scene_pointer): (Cow<[ProjectRackClip]>, Vec<Option<u64>>) = if rack.clips.is_empty() {
        let (clips, pointers) = legacy_rack_clips(source, group_id);
        (Cow::Owned(clips), pointers)
    } else {
        let pointers = (0..source.scene_count())
            .map(|scene| source.scene_rack_clip(scene, group_id))
            .collect();
        (Cow::Borrowed(rack.clips.as_slice()), pointers)
    };

    // Pads, in rack pad order; member position -> pad position.
    let mut pads = Vec::with_capacity(rack.pads.len());
    let mut pad_tracks = Vec::with_capacity(rack.pads.len());
    let mut member_to_pad = vec![None; group.members.len()];
    for (pad_index, pad) in rack.pads.iter().enumerate() {
        let Some(track) = group.members.get(pad.member).copied() else {
            continue;
        };
        let name = source.track_name(track);
        let kind = file.tracks.get(track).map(|track| &track.kind);
        let pattern = pad_sound_pattern(source, &clips, pad.member);
        let (sound, modulator) = match kind {
            Some(ProjectTrackKind::Modulator) => {
                (None, Some(pattern.instrument_slots.get(track).cloned().unwrap_or_default()))
            }
            Some(ProjectTrackKind::Empty) | None => {
                warnings.push(format!("'{name}' is an empty track and was left out of the kit"));
                continue;
            }
            Some(_) => (
                Some(super::super::projects::sound_preset_from_project(
                    file,
                    track,
                    pattern,
                    &name,
                    Vec::new(),
                    String::new(),
                )?),
                None,
            ),
        };
        member_to_pad[pad.member] = Some(pads.len());
        pad_tracks.push(track);
        pads.push(ProjectKitPad {
            pad_note: pad.pad_note,
            choke_group: rack.choke_group(pad_index),
            role: pad.role,
            name,
            sound,
            modulator,
        });
    }
    if pads.iter().all(|pad| pad.modulator.is_some()) {
        return Err(format!("'{}' has no pad with a sound on it", group.name));
    }
    let pad_count = pads.len();
    let mut track_to_pad = vec![None; file.tracks.len()];
    for (pad, track) in pad_tracks.iter().enumerate() {
        track_to_pad[*track] = Some(pad);
    }

    // The rack's sequencers: plain scripts as recorded, package instances as
    // data (current-scene overrides and documents, routes in pad space; a
    // kit with clips carries its per-clip ones in the clips instead). The
    // kit-local id is the source id, which the loader's id map is keyed by.
    let sequencers = rack.sequencers.clone();
    let current = &file.patterns[source.current_scene()];
    let instances: Vec<ProjectKitInstance> = file
        .instances
        .list
        .iter()
        .filter(|instance| instance.owner.rack() == Some(group_id))
        .map(|instance| ProjectKitInstance {
            id: instance.id,
            kind: instance.kind.clone(),
            label: instance.label.clone(),
            overrides: current
                .graph_overrides
                .iter()
                .find(|graph| graph.sequencer_id == instance.id)
                .cloned()
                .map(|mut graph| {
                    super::super::rack_sequencers::remap_graph_member_routes(&mut graph, &member_to_pad);
                    graph
                }),
            document: current
                .scene_slots
                .iter()
                .filter_map(|(name, value)| {
                    (crate::lisp_host::instance_document_slot_owner(name) == Some(instance.id))
                        .then(|| name.rsplit_once('/'))
                        .flatten()
                        .map(|(_, field)| (field.to_string(), value.clone()))
                })
                .collect(),
        })
        .collect();
    let carried: HashSet<u64> = sequencers
        .iter()
        .map(|sequencer| sequencer.sequencer_id)
        .chain(instances.iter().map(|instance| instance.id))
        .collect();

    // The clip bank in pad space.
    let mut kit_clips = Vec::with_capacity(clips.len());
    let mut clip_index: BTreeMap<u64, usize> = BTreeMap::new();
    for clip in clips.iter() {
        let mut members = vec![false; pad_count];
        for (member, sounds) in clip.members.iter().enumerate() {
            if let Some(Some(pad)) = member_to_pad.get(member) {
                members[*pad] = *sounds;
            }
        }
        let graph_overrides: Vec<ProjectGraphOverrides> = clip
            .graph_overrides
            .iter()
            .filter(|graph| {
                graph.owner_rack.is_none_or(|owner| owner == group_id)
                    && carried.contains(&graph.sequencer_id)
            })
            .map(|graph| {
                let mut graph = graph.clone();
                super::super::rack_sequencers::remap_graph_member_routes(&mut graph, &member_to_pad);
                graph
            })
            .collect();
        let scene_slots = clip
            .scene_slots
            .iter()
            .filter(|(name, _)| {
                crate::lisp_host::instance_document_slot_owner(name)
                    .is_some_and(|id| carried.contains(&id))
            })
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        clip_index.insert(clip.id, kit_clips.len());
        kit_clips.push(ProjectRackClip {
            id: kit_clips.len() as u64 + 1,
            name: clip.name.clone(),
            color: clip.color,
            members,
            pattern: clip.pattern.lanes_to(&track_to_pad, pad_count),
            graph_overrides,
            scene_slots,
            bus_chain: None,
        });
    }
    let scene_clip = scene_pointer
        .iter()
        .map(|pointer| pointer.and_then(|clip| clip_index.get(&clip).copied()))
        .collect();
    // Clips with a groove of their own keep it (a copy enters the target
    // pool when installed).
    let clip_grooves = rack
        .clip_grooves
        .iter()
        .filter_map(|entry| {
            let index = *clip_index.get(&entry.clip)?;
            Some((index, KitGroove::from_rack(&entry.settings, &file.grooves), entry.settings.clone()))
        })
        .collect();

    // The groove the playing clip plays (the saved current scene's, else the
    // rack default).
    let groove_clip = scene_pointer.get(source.current_scene()).copied().flatten();
    let groove = KitGroove::from_rack(rack.groove_for_clip(groove_clip), &file.grooves)
        .map(crate::groove::KitGrooveField::Copy);

    Ok(ExtractedKit {
        preset: ProjectKitPreset {
            version: crate::project::project_file_version(),
            kit_version: crate::project::KIT_PRESET_VERSION,
            metadata: ProjectSoundMetadata {
                name: group.name.clone(),
                tags: Vec::new(),
                author: String::new(),
            },
            color: group.color,
            pads,
            bus_chain: group_bus_chain(source, group.bus_id),
            sequencers,
            instances,
            mod_connections: Vec::new(),
            clips: kit_clips,
            groove,
            legacy_grooves: Vec::new(),
        },
        pad_tracks,
        scene_clip,
        clip_grooves,
        warnings,
    })
}

/// A group's bus chain (the group bus as the file saved it).
pub(crate) fn group_bus_chain(source: &ImportSource, bus_id: u64) -> Option<ProjectKitBusChain> {
    let bus = source.file.buses.iter().find(|bus| bus.id == bus_id)?;
    Some(ProjectKitBusChain {
        effects: bus
            .custom_effects
            .iter()
            .enumerate()
            .map_while(|(slot, name)| {
                let name = name.as_deref()?.trim();
                if name.is_empty() {
                    return None;
                }
                Some(ProjectKitBusEffect {
                    name: name.to_string(),
                    slot: bus.effect_slots.get(slot).cloned().unwrap_or_default(),
                })
            })
            .collect(),
    })
}
