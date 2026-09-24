//! Break kits (`docs/rack-clips-and-break-kits-spec.md` §7): the clip bank and
//! the rack-owned graph sequencers travelling inside a `.kit`.
//!
//! A kit has no project to index into, so everything positional in it is in
//! **pad space**: a kit clip's `members` flags, the meaningful lanes of its
//! `pattern`, and its graph-override routes are all indexed by position in
//! `ProjectKitPreset::pads`. Export maps member position -> pad, import maps
//! pad -> the member track it just built. That is what lets a kit carry a
//! break into a project whose tracks, member order and group ids are all
//! different.
//!
//! Instances of package kinds travel as DATA (`docs/instance-kinds-spec.md`
//! §9): `{kind, label, overrides}` per instance, never code. Loading always
//! hands each one a fresh instance id and re-keys its overrides and the kit
//! clips' overrides to it, so two kits (or one kit loaded twice) never share
//! an instance. A kind whose package is not installed stays a placeholder
//! instance that publishes once the kind registers.

use super::*;
use std::path::Path;

use crate::graph::ProjectGraphOverrides;
use crate::project::{
    ProjectInstance, ProjectInstanceOwner, ProjectKitInstance, ProjectKitModConnection,
    ProjectKitModDestination, ProjectKitPreset, ProjectPattern, ProjectRackClip,
    ProjectRackSequencer,
};
use crate::sequencer::{BusId, ModConnection, ModDestination, TrackOutput};
use crate::sequencer::{PatternSnapshot, TrackPatternData};

/// What the export of one rack's sequencers + chosen scenes produced, plus the
/// per-sequencer warnings the caller reports (a `(load "path")` whose file is
/// gone travels as a dead reference).
pub(super) struct CapturedKitContent {
    pub sequencers: Vec<ProjectRackSequencer>,
    pub instances: Vec<ProjectKitInstance>,
    pub clips: Vec<ProjectRackClip>,
    pub warnings: Vec<String>,
}

/// Where one kit-local sequencer id lands in the project a kit loads into:
/// the id and published name its overrides must be re-keyed to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct KitSequencerRekey {
    pub id: u64,
    pub name: String,
}

/// One pad a kit will carry: the rack pad's note and choke group plus the
/// member track its Sound is captured from.
pub(super) struct KitPadSource {
    pub pad_note: i32,
    pub choke_group: Option<u8>,
    pub track: usize,
    pub name: String,
    /// A modulator member (§7.5): travels as its instrument slot, not a Sound.
    pub modulator: bool,
}

/// The kit's pad space for one rack (see `App::kit_pad_roster`).
pub(super) struct KitPadRoster {
    pub pads: Vec<KitPadSource>,
    /// member position -> position in `pads`; `None` for a member with no
    /// pad or one a kit cannot carry.
    pub member_to_pad: Vec<Option<usize>>,
    pub warnings: Vec<String>,
}

impl App {
    /// The kit-side payload for `group_id`: its sequencers as recorded, and one
    /// clip per chosen scene (in the order given), named after that scene.
    ///
    /// A LEGACY rack is converted to clips first (§4.3) — in its own recorded
    /// edit, exactly as the rack menu's "Convert to clips" does — so that the
    /// export reads one representation and the project keeps playing what it
    /// played. A chosen scene the rack is silent in contributes no clip: the
    /// bank stays the set of things the rack actually plays, which is the same
    /// rule conversion itself uses.
    pub(super) fn capture_kit_content(
        &mut self,
        group_id: u64,
        scene_selection: &[usize],
    ) -> Result<CapturedKitContent, String> {
        let sequencers = self.rack_sequencers(group_id);
        let mut warnings = Vec::new();
        let instances = self.capture_kit_instances(group_id, &mut warnings)?;
        // A clip may still hold overrides of a sequencer the rack no longer
        // has (a deleted instance, one moved out, a detached script). Only
        // what the kit records travels: a stale small instance id would
        // otherwise land on an unrelated instance of the importing project.
        let carried: HashSet<u64> = sequencers
            .iter()
            .map(|sequencer| sequencer.sequencer_id)
            .chain(instances.iter().map(|instance| instance.id))
            .collect();
        for sequencer in &sequencers {
            let source = sequencer.source.trim();
            if source.is_empty() {
                warnings.push(format!(
                    "'{}' has no recorded source and will not come back",
                    sequencer.sequencer_name
                ));
            } else if let Some(path) = load_form_path(source) {
                if !Path::new(&path).is_file() {
                    warnings.push(format!(
                        "'{}' loads from {path}, which no longer exists",
                        sequencer.sequencer_name
                    ));
                }
            }
        }
        if scene_selection.is_empty() {
            return Ok(CapturedKitContent {
                sequencers,
                instances,
                clips: Vec::new(),
                warnings,
            });
        }
        if self.rack_is_legacy(group_id) {
            self.convert_rack_to_clips_recorded(group_id)?;
        }

        // member position -> kit pad position, and the kit pad count. A member
        // with no pad cannot travel: a kit rebuilds members FROM pads. Members
        // a kit cannot carry (modulator and empty tracks) are left out too, and
        // reported, so the positional pad space here is the one
        // `capture_rack_as_kit` writes.
        // (The roster's own warnings are reported by `capture_rack_as_kit`,
        // which reads the roster whether or not scenes were chosen.)
        let roster = self.kit_pad_roster(group_id)?;
        let member_to_pad = roster.member_to_pad;
        let pad_count = roster.pads.len();
        if pad_count == 0 {
            return Err("A kit needs at least one pad with a sound on it".to_string());
        }

        // Read every chosen scene's clip out of the bank in one borrow.
        #[allow(clippy::type_complexity)]
        let picked: Vec<(
            String,
            Vec<Option<TrackPatternData>>,
            Vec<ProjectGraphOverrides>,
        )> = self.state.with_scenes(|scenes| {
            let mut picked = Vec::new();
            for scene_idx in scene_selection {
                let Some(clip_id) = scenes.scene_rack_clip(*scene_idx, group_id) else {
                    continue;
                };
                let Some(clip) = scenes
                    .rack_bank(group_id)
                    .and_then(|bank| bank.clip(clip_id))
                else {
                    continue;
                };
                let Some(bank) = scenes.rack_bank(group_id) else {
                    continue;
                };
                let lanes = clip
                    .cells
                    .iter()
                    .enumerate()
                    .map(|(position, cell)| {
                        let track = bank.members.get(position).copied()?;
                        scenes.track_pools.get(track)?.get((*cell)?)
                    })
                    .collect();
                let name = scenes
                    .scenes
                    .get(*scene_idx)
                    .map(|scene| scene.name.clone())
                    .unwrap_or_else(|| clip.name.clone());
                picked.push((name, lanes, clip.graph_overrides.clone()));
            }
            picked
        });

        let mut clips = Vec::with_capacity(picked.len());
        for (index, (name, lanes, overrides)) in picked.into_iter().enumerate() {
            let mut snapshot = PatternSnapshot::new_default(pad_count, &[]);
            let mut sample_paths = vec![None; pad_count];
            let mut sample_names = vec![String::new(); pad_count];
            let mut members = vec![false; pad_count];
            for (position, lane) in lanes.into_iter().enumerate() {
                let (Some(Some(pad)), Some(lane)) = (member_to_pad.get(position), lane) else {
                    continue;
                };
                let pad = *pad;
                if pad >= pad_count {
                    continue;
                }
                members[pad] = true;
                snapshot.set_track_pattern_data(pad, lane);
                let (buffer_id, sample_name, _) = snapshot.sample_ids[pad].clone();
                if snapshot
                    .instrument_types
                    .get(pad)
                    .copied()
                    .unwrap_or(InstrumentType::Sampler)
                    == InstrumentType::Sampler
                {
                    sample_paths[pad] = self
                        .capture_sampler_source_path(buffer_id, &sample_name)?
                        .map(|path| path.to_string_lossy().to_string());
                }
                sample_names[pad] = if sample_paths[pad].is_some() {
                    sample_name
                } else {
                    String::new()
                };
            }
            let graph_overrides = overrides
                .into_iter()
                .filter(|graph| {
                    graph.owner_rack == Some(group_id) && carried.contains(&graph.sequencer_id)
                })
                .map(|mut graph| {
                    super::rack_sequencers::remap_graph_member_routes(&mut graph, &member_to_pad);
                    graph
                })
                .collect();
            clips.push(ProjectRackClip {
                id: index as u64 + 1,
                name,
                color: None,
                members,
                pattern: ProjectPattern::from_snapshot(
                    &snapshot,
                    sample_paths,
                    sample_names,
                    Vec::new(),
                ),
                graph_overrides,
                bus_chain: None,
            });
        }
        Ok(CapturedKitContent {
            sequencers,
            instances,
            clips,
            warnings,
        })
    }

    /// The pads a kit can carry, in rack pad order: every pad whose member
    /// track is an instrument (captured as a Sound) or a modulator (captured
    /// as its instrument slot, §7.5). An empty member track has nothing to
    /// carry, so it is left out and named in `warnings` instead of failing
    /// the whole export. `member_to_pad` maps member position -> position in
    /// `pads`, which is the pad space the kit's clips and cables are written
    /// in.
    pub(super) fn kit_pad_roster(&self, group_id: u64) -> Result<KitPadRoster, String> {
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let rack = group
            .rack
            .as_ref()
            .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
        let mut pads = Vec::with_capacity(rack.pads.len());
        let mut member_to_pad = vec![None; group.members.len()];
        let mut warnings = Vec::new();
        for (pad_index, pad) in rack.pads.iter().enumerate() {
            let Some(track) = group.members.get(pad.member).copied() else {
                continue;
            };
            let name = self
                .tracks
                .get(track)
                .cloned()
                .ok_or_else(|| format!("Kit pad {} has no member track", pad.pad_note))?;
            let modulator = match self.graph.track_instrument_types.get(track).copied() {
                Some(InstrumentType::Modulator) => true,
                Some(InstrumentType::Empty) | None => {
                    warnings.push(format!("'{name}' is an empty track and was left out of the kit"));
                    continue;
                }
                Some(_) => false,
            };
            member_to_pad[pad.member] = Some(pads.len());
            pads.push(KitPadSource {
                pad_note: pad.pad_note,
                choke_group: rack.choke_group(pad_index),
                track,
                name,
                modulator,
            });
        }
        Ok(KitPadRoster { pads, member_to_pad, warnings })
    }

    /// The rack's internal modulation cables in the current scene, in pad
    /// space (§7.5): a route whose source is a member pad and whose
    /// destination is a member pad or the rack's own bus. Any other route
    /// leaves the rack and is counted, not carried — a kit is self-contained.
    pub(super) fn capture_kit_mod_connections(
        &self,
        group_id: u64,
        member_to_pad: &[Option<usize>],
    ) -> Result<(Vec<ProjectKitModConnection>, usize), String> {
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let track_to_pad = |track: usize| -> Option<usize> {
            let member = group.members.iter().position(|member| *member == track)?;
            member_to_pad.get(member).copied().flatten()
        };
        let mut carried = Vec::new();
        let mut dropped = 0usize;
        for connection in self.state.current_mod_connections() {
            let Some(source_pad) = track_to_pad(connection.source_track) else {
                // Not the rack's cable at all: sourced outside it.
                continue;
            };
            let destination = match connection.destination {
                ModDestination::Track(track) => track_to_pad(track).map(ProjectKitModDestination::Pad),
                ModDestination::Bus(bus) if bus.0 == group.bus_id => {
                    Some(ProjectKitModDestination::RackBus)
                }
                ModDestination::Bus(_) => None,
            };
            let Some(destination) = destination else {
                dropped += 1;
                continue;
            };
            let cable = ProjectKitModConnection {
                source_pad,
                destination,
                dest_input: connection.dest_input,
            };
            if !carried.contains(&cable) {
                carried.push(cable);
            }
        }
        Ok((carried, dropped))
    }

    /// Install a kit's cables (§7.5) onto the rack just built: pad -> the
    /// member track it became, RackBus -> the rack's bus, into EVERY scene,
    /// since the kit carries one patch and the rack must sound the same
    /// whichever scene launches one of its clips. Cables whose pad did not
    /// load are skipped. Idempotent per scene. `pad_notes` is the kit's pad
    /// notes in kit pad order (see `kit_pad_to_position`).
    pub(super) fn install_kit_mod_connections(
        &mut self,
        group_id: u64,
        cables: &[ProjectKitModConnection],
        pad_notes: &[i32],
    ) -> Result<(), String> {
        if cables.is_empty() {
            return Ok(());
        }
        let pad_to_position = self.kit_pad_to_position(group_id, pad_notes)?;
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let pad_to_track: Vec<Option<usize>> = pad_to_position
            .iter()
            .map(|position| position.and_then(|position| group.members.get(position).copied()))
            .collect();
        let bus = BusId(group.bus_id);
        let resolved: Vec<ModConnection> = cables
            .iter()
            .filter_map(|cable| {
                let source_track = pad_to_track.get(cable.source_pad).copied().flatten()?;
                let destination = match cable.destination {
                    ProjectKitModDestination::Pad(pad) => {
                        ModDestination::Track(pad_to_track.get(pad).copied().flatten()?)
                    }
                    ProjectKitModDestination::RackBus => ModDestination::Bus(bus),
                };
                if destination == ModDestination::Track(source_track) {
                    return None;
                }
                Some(ModConnection { source_track, destination, dest_input: cable.dest_input })
            })
            .collect();
        if resolved.is_empty() {
            return Ok(());
        }
        self.state.with_scenes_mut(|scenes| {
            for scene in scenes.scenes.iter_mut() {
                for connection in &resolved {
                    if !scene.mod_connections.contains(connection) {
                        scene.mod_connections.push(*connection);
                    }
                }
            }
        });
        self.graph_controller().sync_current_pattern_mod_routes();
        Ok(())
    }

    /// The rack's instances as kit records (instance-kinds spec §9): kind,
    /// label, and the instance's overrides in the current scene with routes
    /// in PAD space. The recorded `id` is only the kit-local key the clip
    /// overrides are written under. An instance of a kind defined outside any
    /// package travels too, but is named in `warnings`: only a project that
    /// evaluates the same code can bring it back.
    fn capture_kit_instances(
        &self,
        group_id: u64,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<ProjectKitInstance>, String> {
        let instances = self.rack_instances(group_id);
        if instances.is_empty() {
            return Ok(Vec::new());
        }
        let member_to_pad = self.kit_pad_roster(group_id)?.member_to_pad;
        let current = self.state.current_graph_overrides();
        Ok(instances
            .into_iter()
            .map(|instance| {
                if crate::lisp_host::kind_package_of(&instance.kind)
                    == crate::lisp_host::SCRATCH_KIND_PACKAGE
                {
                    warnings.push(format!(
                        "'{}' is a kind defined in project code ({}), not a package; it only \
                         comes back where that code is evaluated",
                        instance.label, instance.kind
                    ));
                }
                let overrides = current
                    .iter()
                    .find(|graph| graph.sequencer_id == instance.id)
                    .cloned()
                    .map(|mut graph| {
                        super::rack_sequencers::remap_graph_member_routes(
                            &mut graph,
                            &member_to_pad,
                        );
                        graph
                    });
                ProjectKitInstance {
                    id: instance.id,
                    kind: instance.kind,
                    label: instance.label,
                    overrides,
                }
            })
            .collect())
    }

    /// Register the kit's rack-owned sequencers and instances on `group_id`,
    /// handing back the map kit-local id -> where it landed, for the clip
    /// overrides to follow (`install_kit_clips`).
    ///
    /// - A legacy plain-script sequencer (`(load …)` / script text) keeps the
    ///   rack-derived id (`graph_instance_id(name, rack)`); the App cannot
    ///   run Lisp, so the host evaluates the recorded source afterwards
    ///   (rack-clips spec §7.3). An entry whose file is missing stays
    ///   recorded, so a later re-import brings it back.
    /// - Every kit instance becomes a NEW rack-owned instance with a fresh id
    ///   (instance-kinds spec §9), in one recorded edit. When
    ///   `install_instance_overrides` (the kit has no clips of its own), each
    ///   instance's recorded overrides are installed under its new id.
    ///   An instance whose kind is not registered stays a placeholder that
    ///   publishes once its package is attached.
    pub(super) fn register_kit_sequencers(
        &mut self,
        group_id: u64,
        sequencers: &[ProjectRackSequencer],
        instances: &[ProjectKitInstance],
        install_instance_overrides: bool,
        pad_notes: &[i32],
        failures: &mut Vec<String>,
    ) -> HashMap<u64, KitSequencerRekey> {
        let mut id_map = HashMap::new();
        for sequencer in sequencers {
            let new_id =
                crate::lisp_host::graph_instance_id(&sequencer.sequencer_name, Some(group_id));
            id_map.insert(
                sequencer.sequencer_id,
                KitSequencerRekey { id: new_id, name: sequencer.sequencer_name.clone() },
            );
            if let Err(error) = self.attach_rack_sequencer_recorded(
                group_id,
                new_id,
                &sequencer.sequencer_name,
                &sequencer.source,
            ) {
                failures.push(format!("sequencer '{}': {error}", sequencer.sequencer_name));
            }
        }
        match self.add_kit_instances_recorded(
            group_id,
            instances,
            install_instance_overrides,
            pad_notes,
        ) {
            Ok(instance_map) => id_map.extend(instance_map),
            Err(error) => failures.push(format!("instances: {error}")),
        }
        id_map
    }

    /// The kit instances as new rack-owned instances with fresh ids (§9).
    /// Labels are kept unless another instance already uses one, in which
    /// case the kind's next default label is used.
    fn add_kit_instances_recorded(
        &mut self,
        group_id: u64,
        records: &[ProjectKitInstance],
        install_overrides: bool,
        pad_notes: &[i32],
    ) -> Result<HashMap<u64, KitSequencerRekey>, String> {
        if records.is_empty() {
            return Ok(HashMap::new());
        }
        let pad_to_position = self.kit_pad_to_position(group_id, pad_notes)?;
        let install_overrides =
            install_overrides && records.iter().any(|record| record.overrides.is_some());
        let records = records.to_vec();
        // The new instances' overrides are recorded (they are all this edit
        // installs); no existing instance's overrides change.
        let recorded = install_overrides.then(Vec::new);
        self.apply_recorded_instance_mutation("Load kit instances", recorded, move |app| {
            let mut map = HashMap::new();
            let mut installs = Vec::new();
            let mut reminter = app.state.with_scenes(|scenes| {
                crate::lisp_host::GraphNodeProcessReminter::new(
                    super::instances::all_override_entries(scenes),
                )
            });
            for record in records {
                let id = app.allocate_instance_id();
                let name = crate::lisp_host::instance_sequencer_name(
                    crate::lisp_host::kind_name_of(&record.kind),
                    id,
                );
                let label = Some(record.label.trim().to_string())
                    .filter(|label| {
                        !label.is_empty()
                            && !app.instances.list.iter().any(|other| &other.label == label)
                    })
                    .unwrap_or_else(|| {
                        super::instances::default_label_in(&app.instances.list, &record.kind)
                    });
                app.instances.list.push(ProjectInstance {
                    id,
                    kind: record.kind.clone(),
                    owner: ProjectInstanceOwner::Rack(group_id),
                    label,
                });
                if let (true, Some(mut graph)) = (install_overrides, record.overrides) {
                    super::rack_sequencers::remap_graph_member_routes(
                        &mut graph,
                        &pad_to_position,
                    );
                    graph.sequencer_id = id;
                    graph.sequencer_name = name.clone();
                    graph.owner_rack = Some(group_id);
                    // Fresh node process slot ids: one kit loaded twice must
                    // not share process state between its copies (§9).
                    reminter.remint(id, &mut graph);
                    installs.push(graph);
                }
                map.insert(record.id, KitSequencerRekey { id, name });
            }
            app.install_kit_instance_overrides(group_id, installs);
            Ok(map)
        })
    }

    /// A clip-less kit's instance overrides (§9), the patch the rack played:
    /// into every scene when the rack has no clip bank, else into every clip
    /// of its bank (scene-side entries of a clip-bearing rack never sound).
    /// An entry already there for the same id is left alone.
    fn install_kit_instance_overrides(
        &mut self,
        group_id: u64,
        installs: Vec<ProjectGraphOverrides>,
    ) {
        if installs.is_empty() {
            return;
        }
        let add_missing = |graphs: &mut Vec<ProjectGraphOverrides>| {
            let mut changed = false;
            for graph in &installs {
                if !graphs.iter().any(|existing| existing.sequencer_id == graph.sequencer_id) {
                    graphs.push(graph.clone());
                    changed = true;
                }
            }
            changed
        };
        let banked = self.state.with_scenes(|scenes| {
            scenes.rack_bank(group_id).is_some_and(|bank| !bank.clips.is_empty())
        });
        if banked {
            let changed = self.state.with_scenes_mut(|scenes| {
                let mut changed = false;
                if let Some(bank) = scenes.rack_bank_mut(group_id) {
                    for clip in &mut bank.clips {
                        changed |= add_missing(&mut clip.graph_overrides);
                    }
                }
                changed
            });
            if changed {
                self.state.publish_scheduler_snapshot();
            }
        } else {
            self.state.edit_all_scene_graph_overrides(add_missing);
        }
    }

    /// Delete every instance `group_id` owns, with their overrides, in one
    /// recorded edit: auditioning a kit onto a rack replaces what the rack
    /// ran (rack-clips spec §7.3), instances included.
    pub(super) fn delete_rack_instances_recorded(&mut self, group_id: u64) -> Result<(), String> {
        let ids: Vec<u64> =
            self.rack_instances(group_id).iter().map(|instance| instance.id).collect();
        if ids.is_empty() {
            return Ok(());
        }
        let recorded = Some(ids.clone());
        self.apply_recorded_instance_mutation("Replace rack instances", recorded, move |app| {
            app.instances.list.retain(|instance| !ids.contains(&instance.id));
            app.drop_instance_overrides(&ids);
            Ok(())
        })
    }

    /// kit pad position -> member position of `group_id`, matched by pad
    /// note: `pad_notes` is the kit's pad notes in kit pad order. A pad that
    /// failed to load has no rack pad, so it maps to no member; matching by
    /// note keeps every later pad on its own member, where the rack's pad
    /// order would shift them onto their neighbours.
    fn kit_pad_to_position(
        &self,
        group_id: u64,
        pad_notes: &[i32],
    ) -> Result<Vec<Option<usize>>, String> {
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let rack = group
            .rack
            .as_ref()
            .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
        Ok(pad_notes
            .iter()
            .map(|note| {
                rack.pad_index_for_note(*note)
                    .map(|index| rack.pads[index].member)
                    .filter(|member| *member < group.members.len())
            })
            .collect())
    }

    /// Install a kit's clip bank onto `group_id`, replacing whatever bank it
    /// had. No project scene is pointed at any of the new clips: the rack is
    /// SILENT until the user launches one (§7.3), which is the whole point of
    /// dropping a break into a project that already has scenes.
    pub(super) fn install_kit_clips(
        &mut self,
        group_id: u64,
        clips: Vec<ProjectRackClip>,
        id_map: &HashMap<u64, KitSequencerRekey>,
        pad_notes: &[i32],
    ) -> Result<(), String> {
        // kit pad position -> member position, by pad note. A pad that failed
        // to load has no rack pad and its lane is simply dropped.
        let pad_to_position = self.kit_pad_to_position(group_id, pad_notes)?;
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let members = group.members.clone();
        let bus = BusId(group.bus_id);
        let num_tracks = self.tracks.len();

        let mut assets: HashMap<PathBuf, ProjectSampleAsset> = HashMap::new();
        let mut built = Vec::with_capacity(clips.len());
        // One reminter for every clip, so each new sequencer's node process
        // slots get ONE fresh identity across clips, never the kit's (which
        // a second load of the same kit, or the exporting rack, also has).
        let mut reminter = self.state.with_scenes(|scenes| {
            crate::lisp_host::GraphNodeProcessReminter::new(
                super::instances::all_override_entries(scenes),
            )
        });
        for (index, clip) in clips.into_iter().enumerate() {
            let pattern =
                expand_kit_clip_pattern(clip.pattern, &pad_to_position, &members, num_tracks);
            let (snapshot, _, _) =
                self.project_pattern_into_snapshot_with_policy(pattern, &mut assets, false)?;
            let cells: Vec<Option<TrackPatternData>> = members
                .iter()
                .enumerate()
                .map(|(position, track)| {
                    let pad = pad_to_position
                        .iter()
                        .position(|slot| *slot == Some(position))?;
                    let mut lane = clip.members
                        .get(pad)
                        .copied()
                        .unwrap_or(false)
                        .then(|| snapshot.track_pattern_data(*track))
                        .flatten()?;
                    // Group membership owns the output route. The clip's
                    // saved bus ID belongs to the exporting project and may
                    // identify an unrelated bus here. Bind before inserting
                    // the lane so every later clip launch uses this rack.
                    lane.track_params.output = TrackOutput::Bus(bus);
                    Some(lane)
                })
                .collect();
            // An override keyed by an id the kit does not record belongs to
            // nothing the load created; kept, its (small, counter-issued)
            // id would match an unrelated live instance here. Drop it.
            let graph_overrides: Vec<ProjectGraphOverrides> = clip
                .graph_overrides
                .into_iter()
                .filter_map(|mut graph| {
                    let rekey = id_map.get(&graph.sequencer_id)?;
                    super::rack_sequencers::remap_graph_member_routes(&mut graph, &pad_to_position);
                    graph.owner_rack = Some(group_id);
                    graph.sequencer_id = rekey.id;
                    graph.sequencer_name.clone_from(&rekey.name);
                    reminter.remint(rekey.id, &mut graph);
                    Some(graph)
                })
                .collect();
            built.push((
                index as crate::sequencer::RackClipId + 1,
                clip.name,
                clip.color,
                graph_overrides,
                cells,
            ));
        }

        self.state.with_scenes_mut(|scenes| {
            let mut bank = crate::sequencer::RackClipBank {
                group_id,
                members: members.clone(),
                clips: Vec::with_capacity(built.len()),
                next_clip_id: built.len() as u64 + 1,
            };
            for (id, name, color, graph_overrides, cells) in built {
                let cells = cells
                    .into_iter()
                    .enumerate()
                    .map(|(position, data)| {
                        let track = *members.get(position)?;
                        let data = data?;
                        scenes
                            .track_pools
                            .get_mut(track)
                            .map(|pool| pool.insert(data))
                    })
                    .collect();
                bank.clips.push(crate::sequencer::RackClip {
                    id,
                    name,
                    color,
                    cells,
                    graph_overrides,
                });
            }
            scenes.replace_rack_bank(bank);
            // Silence in every existing scene is explicit: clear both the
            // pointers (a replaced bank's ids are meaningless) and the scene
            // cells of the new members, so nothing shadows the clips.
            for scene in &mut scenes.scenes {
                scene.rack_clips.retain(|(gid, _)| *gid != group_id);
                for track in &members {
                    if let Some(cell) = scene.cells.get_mut(*track) {
                        *cell = None;
                    }
                }
            }
            scenes.repair_rack_clips();
            scenes.adopt_live_rack_clips();
        });
        self.publish_rack_choke_runtime();
        Ok(())
    }
}

/// Kits written before instances (kit format < 4) recorded a package's
/// sequencer as an `(import m)` source (instance-kinds spec §10). One whose
/// module declares exactly one kind becomes a kit instance record keyed by
/// the recorded `sequencer_id` (which the kit's clip overrides use), so the
/// load hands it a fresh id like any other instance. A module declaring
/// several kinds is reported and stays a legacy script; one declaring none
/// is a plain script. `declared` reads package manifests only. Idempotent.
pub(super) fn migrate_kit_sequencers(
    kit: &mut ProjectKitPreset,
    declared: impl Fn(&str) -> Vec<crate::lisp_host::DeclaredKind>,
) -> Vec<String> {
    let mut notes = Vec::new();
    let mut kept = Vec::with_capacity(kit.sequencers.len());
    for record in std::mem::take(&mut kit.sequencers) {
        let Some(module) = super::instances::legacy_import_module(&record.source) else {
            kept.push(record);
            continue;
        };
        let kinds = declared(&module);
        match kinds.as_slice() {
            [] => kept.push(record),
            [kind] => {
                if !kit.instances.iter().any(|instance| instance.id == record.sequencer_id) {
                    kit.instances.push(ProjectKitInstance {
                        id: record.sequencer_id,
                        kind: kind.id.clone(),
                        label: String::new(),
                        overrides: None,
                    });
                }
            }
            many => {
                notes.push(format!(
                    "{module} declares {} kinds; '{}' was left as a script, create the one \
                     you want instead",
                    many.len(),
                    record.sequencer_name
                ));
                kept.push(record);
            }
        }
    }
    kit.sequencers = kept;
    notes
}

/// Give every clip lane the rack slot its pad plays from.
///
/// A kit pad is always a one-slot rack Sound, even when the exporting rack's
/// member was a plain instrument or sampler track. Such a member's clips carry
/// their sound in the TRACK-level `instrument_slots` / `effect_slots`, which a
/// rack track never reads, so every clip used to play the pad's single saved
/// Sound. Build each such lane's rack slot the way the Sounds browser captures
/// a plain track, keeping the pad Sound's slot-level settings (gain, pan,
/// polyphony, effect names) and taking the clip's own sound. Lanes that
/// already carry rack data, modulator pads and multi-slot racks are left
/// alone. Idempotent.
pub(super) fn lift_plain_pad_clip_lanes(kit: &mut ProjectKitPreset) {
    use crate::project::ProjectInstrumentType as Kind;
    for clip in &mut kit.clips {
        let pattern = &mut clip.pattern;
        for (pad, kit_pad) in kit.pads.iter().enumerate() {
            let Some(sound) = &kit_pad.sound else {
                continue;
            };
            let [template] = sound.rack.slots.as_slice() else {
                continue;
            };
            let is_sampler = match (pattern.instrument_types.get(pad), template.instrument_type) {
                (Some(Kind::Custom), Kind::Custom) => false,
                (Some(Kind::Sampler), Kind::Sampler) => true,
                _ => continue,
            };
            if pattern.rack_tracks.get(pad).is_some_and(Option::is_some) {
                continue;
            }
            let Some(instrument_slot) = pattern.instrument_slots.get(pad).cloned() else {
                continue;
            };
            let mut slot = template.clone();
            slot.instrument_slot = instrument_slot;
            if let Some(effects) = pattern.effect_slots.get(pad) {
                slot.effect_slots = effects.clone();
            }
            if let Some(state) = pattern.track_sound_states.get(pad) {
                slot.track_sound_state = state.clone();
            }
            if let Some(mode) = pattern.instrument_run_modes.get(pad) {
                slot.instrument_run_mode = *mode;
            }
            if let Some(offset) = pattern.instrument_base_note_offsets.get(pad) {
                slot.instrument_base_note_offset = *offset;
            }
            if is_sampler {
                if let Some(Some(path)) = pattern.sample_paths.get(pad) {
                    slot.sample_path = Some(path.clone());
                    slot.sample_name = pattern.sample_names.get(pad).cloned();
                }
            }
            if pattern.rack_tracks.len() <= pad {
                pattern.rack_tracks.resize_with(pad + 1, || None);
            }
            pattern.rack_tracks[pad] = Some(crate::project::ProjectRackTrackPattern {
                routing: sound.rack.routing,
                slots: vec![slot],
                macros: sound.rack.macros.clone(),
            });
            pattern.instrument_types[pad] = Kind::Rack;
        }
    }
}

/// `Some(path)` when a recorded sequencer source is a `(load "path")` form.
fn load_form_path(source: &str) -> Option<String> {
    let inner = source
        .trim()
        .strip_prefix("(load ")?
        .strip_suffix(')')?
        .trim();
    let path = inner.strip_prefix('"')?.strip_suffix('"')?;
    (!path.is_empty()).then(|| path.to_string())
}

/// Lift a kit clip's PAD-space pattern into a full-width project pattern whose
/// meaningful lanes sit on the member tracks the loader just built.
///
/// The base is a full-width default pattern, which is what guarantees every
/// per-track vector is exactly `num_tracks` long — `project_pattern_into_
/// snapshot_with_policy` consumes several of them verbatim, so a short one
/// would yield a short snapshot.
fn expand_kit_clip_pattern(
    compact: ProjectPattern,
    pad_to_position: &[Option<usize>],
    members: &[usize],
    num_tracks: usize,
) -> ProjectPattern {
    let mut full = ProjectPattern::from_snapshot(
        &PatternSnapshot::new_default(num_tracks, &[]),
        vec![None; num_tracks],
        vec![String::new(); num_tracks],
        Vec::new(),
    );
    let pad_to_track: Vec<Option<usize>> = pad_to_position
        .iter()
        .map(|position| position.and_then(|position| members.get(position).copied()))
        .collect();
    macro_rules! place {
        ($field:ident) => {{
            for (pad, value) in compact.$field.into_iter().enumerate() {
                let Some(Some(track)) = pad_to_track.get(pad) else {
                    continue;
                };
                if let Some(slot) = full.$field.get_mut(*track) {
                    *slot = value;
                }
            }
        }};
    }
    place!(track_bits);
    place!(neural_reset_bits);
    place!(step_data);
    place!(track_params);
    place!(effect_slots);
    place!(midi_fx_slots);
    place!(instrument_slots);
    place!(instrument_base_note_offsets);
    place!(track_sound_states);
    place!(chord_snapshots);
    place!(chord_duration_snapshots);
    place!(chord_delay_snapshots);
    place!(timebase_plock_snapshots);
    place!(bar_transpose_snapshots);
    place!(swing_plock_snapshots);
    place!(swing_resolution_plock_snapshots);
    place!(track_send_plock_snapshots);
    place!(instrument_types);
    place!(instrument_run_modes);
    place!(sample_paths);
    place!(sample_names);
    place!(rack_tracks);
    place!(process_chains);
    place!(project_process_lane_overrides);
    place!(plock_variant_registries);
    place!(key_lock_variant_registries);
    full
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use eseqlisp::vm::Value;
    use eseqlisp::Runtime;

    use super::*;
    use crate::graph::ProjectGraphRouteOverride;

    /// alez.neural's kind id: the stand-in below registers under it so the
    /// test exercises exactly the id a real kit records.
    const NEURAL: &str = "alez/neural:neural";
    const SAMPLE: &str = "../../content/impulses/lexicon-300-rich-plate.wav";

    fn headless_app() -> App {
        let engine = crate::audio::engine::init_headless_engine(48_000, 2).unwrap();
        App::new(
            engine.state,
            engine.lg_ptr,
            engine.sample_rate,
            engine.buses,
            engine.master_recorder,
            engine.keyboard_tx,
        )
    }

    /// A runtime with the graph natives bound to `app`, and a stand-in for
    /// alez.neural's `neural` kind registered under its package kind id.
    fn neural_runtime(app: &App) -> Runtime {
        crate::lisp_host::clear_kind_registry();
        let mut runtime = Runtime::new();
        crate::lisp_host::register_graph_authoring_natives(&mut runtime, Arc::clone(&app.state));
        runtime
            .eval_str(
                "(def-kind neural
                   :sequencer (:shape (line 3) :max-poly 2
                               (def-node nrn :route 0
                                 :params ((threshold :float 0 4 :default 0.5)))
                               (edges :from nrn :to nrn :topology (all-to-all)
                                 :params ((weight :float -1 1 :default 0)))))",
            )
            .expect("def-kind evaluates");
        let mut definition =
            crate::lisp_host::registered_kind("scratch:neural").expect("stand-in kind");
        definition.id = NEURAL.to_string();
        definition.package = Some("alez/neural".to_string());
        crate::lisp_host::register_kind(definition);
        runtime
    }

    /// A one-pad rack owning one neural instance whose 0->1 weight is
    /// `weight` and whose node 0 routes to member 0.
    fn rack_with_neural(app: &mut App, runtime: &mut Runtime, name: &str, weight: f64) -> (u64, u64) {
        let (group_id, _) = app.create_drum_rack_recorded(Some(name.to_string())).expect("rack");
        let track = app.graph_controller().add_track(Path::new(SAMPLE)).expect("pad track");
        app.assign_rack_pad_track_recorded(group_id, 36, track).expect("pad");
        let id = app
            .create_instance_recorded(NEURAL, ProjectInstanceOwner::Rack(group_id), None)
            .expect("neural instance");
        runtime.set_global_value("src", Value::Instance(id));
        runtime
            .eval_str(&format!("(graph-edge src :from 0 :to 1 :weight {weight})"))
            .expect("weight");
        runtime.eval_str("(graph-node src 0 :route 0)").expect("route");
        (group_id, id)
    }

    fn save_kit(app: &mut App, group_id: u64, name: &str, scenes: &[usize]) -> PathBuf {
        let (kit, _warnings) = app
            .capture_rack_as_kit(group_id, name, Vec::new(), String::new(), scenes)
            .expect("kit captures");
        let directory = std::env::temp_dir().join(format!(
            "eseq-kit-instances-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).expect("kit directory");
        let path = directory.join(format!("{name}.kit"));
        std::fs::write(&path, serde_json::to_string(&kit).expect("serialize")).expect("write");
        path
    }

    fn only_instance(app: &App, group_id: u64) -> u64 {
        let owned = app.rack_instances(group_id);
        assert_eq!(owned.len(), 1, "the loaded rack owns exactly one instance: {owned:?}");
        assert_eq!(owned[0].kind, NEURAL);
        owned[0].id
    }

    fn weight(runtime: &mut Runtime, id: u64) -> Option<Value> {
        runtime.set_global_value("probe", Value::Instance(id));
        runtime.eval_str("(graph-edge-value probe :from 0 :to 1 :weight)").expect("read weight")
    }

    fn published_owner(app: &App, id: u64) -> Option<Option<u64>> {
        app.state
            .published_sequencers()
            .into_iter()
            .find(|sequencer| sequencer.id == id)
            .map(|sequencer| sequencer.graph.expect("graph").owner_rack)
    }

    /// The motivating bug (instance-kinds spec §1, §9): two kits that each
    /// own a neural sequencer, and one kit loaded twice, must give
    /// independent instances. Before instances, both kits re-ran the same
    /// module and shared one sequencer and its overrides.
    #[test]
    fn two_kits_and_one_kit_loaded_twice_give_independent_instances() {
        let mut app = headless_app();
        let mut runtime = neural_runtime(&app);
        let (rack_a, source_a) = rack_with_neural(&mut app, &mut runtime, "Kit A", 0.25);
        let (rack_b, source_b) = rack_with_neural(&mut app, &mut runtime, "Kit B", 0.5);
        let path_a = save_kit(&mut app, rack_a, "Kit-A", &[]);
        let path_b = save_kit(&mut app, rack_b, "Kit-B", &[]);
        let kit_a = crate::project::load_kit_preset(&path_a).expect("kit A reads back");
        assert!(kit_a.sequencers.is_empty(), "instances travel as data, not sources");
        assert_eq!(kit_a.instances.len(), 1);
        assert_eq!(kit_a.instances[0].kind, NEURAL);
        let recorded = kit_a.instances[0].overrides.as_ref().expect("the rack's overrides");
        assert_eq!(recorded.edge_params.len(), 1);

        let (loaded_a, failures) = app.load_kit_as_rack(&path_a).expect("kit A loads");
        assert!(failures.is_empty(), "{failures:?}");
        let (loaded_b, failures) = app.load_kit_as_rack(&path_b).expect("kit B loads");
        assert!(failures.is_empty(), "{failures:?}");
        let (loaded_a2, failures) = app.load_kit_as_rack(&path_a).expect("kit A loads again");
        assert!(failures.is_empty(), "{failures:?}");

        let a = only_instance(&app, loaded_a);
        let b = only_instance(&app, loaded_b);
        let a2 = only_instance(&app, loaded_a2);
        let ids: std::collections::HashSet<u64> = [source_a, source_b, a, b, a2].into();
        assert_eq!(ids.len(), 5, "every load hands out a fresh id");
        for (id, rack) in [(a, loaded_a), (b, loaded_b), (a2, loaded_a2)] {
            assert_eq!(published_owner(&app, id), Some(Some(rack)), "published, owned by its rack");
        }
        assert_eq!(weight(&mut runtime, a), Some(Value::Number(0.25)), "kit A's weight came along");
        assert_eq!(weight(&mut runtime, a2), Some(Value::Number(0.25)));
        assert_eq!(weight(&mut runtime, b), Some(Value::Number(0.5)), "kit B's weight came along");
        // The route came back through pad space onto the new rack's member.
        let route = app
            .state
            .current_graph_overrides()
            .into_iter()
            .find(|graph| graph.sequencer_id == a)
            .and_then(|graph| {
                graph.node_intrinsics.iter().find(|n| n.instance == 0).and_then(|n| n.route.clone())
            });
        assert_eq!(route, Some(ProjectGraphRouteOverride::Track(0)));
        // Labels stay unique across the project.
        let labels: std::collections::HashSet<String> =
            app.instances.list.iter().map(|instance| instance.label.clone()).collect();
        assert_eq!(labels.len(), app.instances.list.len(), "{:?}", app.instances.list);

        // Editing one leaves every other instance alone.
        runtime.set_global_value("edit", Value::Instance(a));
        runtime.eval_str("(graph-edge edit :from 0 :to 1 :weight 0.9)").expect("edit a");
        assert_eq!(weight(&mut runtime, a), Some(Value::Number(0.9)));
        assert_eq!(weight(&mut runtime, a2), Some(Value::Number(0.25)), "same kit, other load");
        assert_eq!(weight(&mut runtime, b), Some(Value::Number(0.5)), "other kit");
        assert_eq!(weight(&mut runtime, source_a), Some(Value::Number(0.25)), "the exporting rack");

        // One undo takes the last load back, instance and all.
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!app.instances.contains(a2));
        assert_eq!(published_owner(&app, a2), None, "unpublished with its instance");
        assert!(app.instances.contains(a) && app.instances.contains(b));

        let _ = std::fs::remove_dir_all(path_a.parent().unwrap());
    }

    /// A break kit's clip overrides follow the instance to its fresh id and
    /// published name; auditioning the kit onto a rack replaces that rack's
    /// instances.
    #[test]
    fn break_kit_clip_overrides_rekey_to_the_fresh_instance() {
        let mut app = headless_app();
        let mut runtime = neural_runtime(&app);
        let (rack, source) = rack_with_neural(&mut app, &mut runtime, "Break", 0.25);
        app.convert_rack_to_clips_recorded(rack).expect("convert to clips");
        let path = save_kit(&mut app, rack, "Break-Kit", &[0]);
        let kit = crate::project::load_kit_preset(&path).expect("kit reads back");
        assert_eq!(kit.clips.len(), 1);
        assert_eq!(kit.clips[0].graph_overrides[0].sequencer_id, source);

        let (loaded, failures) = app.load_kit_as_rack(&path).expect("break kit loads");
        assert!(failures.is_empty(), "{failures:?}");
        let fresh = only_instance(&app, loaded);
        assert_ne!(fresh, source);
        let clip_overrides = app.state.with_scenes(|scenes| {
            scenes.rack_bank(loaded).expect("bank").clips[0].graph_overrides.clone()
        });
        assert_eq!(clip_overrides.len(), 1);
        assert_eq!(clip_overrides[0].sequencer_id, fresh);
        assert_eq!(
            clip_overrides[0].sequencer_name,
            crate::lisp_host::instance_sequencer_name("neural", fresh)
        );
        assert_eq!(clip_overrides[0].owner_rack, Some(loaded));
        let source_clip = app.state.with_scenes(|scenes| {
            scenes.rack_bank(rack).expect("source bank").clips[0].graph_overrides.clone()
        });
        assert_eq!(source_clip[0].sequencer_id, source, "the exporting rack is untouched");

        // Auditioning onto the loaded rack swaps its instance for a fresh one.
        // (Launch a clip first: a silent break-kit rack has no effective
        // member patterns, and reusing its lanes needs one.)
        let first_clip = app.state.with_scenes(|scenes| scenes.rack_bank(loaded).unwrap().clips[0].id);
        app.set_current_rack_clip_recorded(loaded, Some(first_clip)).expect("launch");
        app.load_kit_onto_rack(loaded, &path).expect("audition");
        let swapped = only_instance(&app, loaded);
        assert!(![source, fresh].contains(&swapped));
        assert!(!app.instances.contains(fresh), "the rack's previous instance is gone");
        assert_eq!(published_owner(&app, fresh), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A kit pad that fails to load leaves no rack pad, so later kit pads sit
    /// one rack pad earlier. Their instance routes must still land on the
    /// member that plays their own note, not on a neighbour.
    #[test]
    fn kit_load_with_a_failed_pad_keeps_later_pads_on_their_own_members() {
        let mut app = headless_app();
        let mut runtime = neural_runtime(&app);
        let (rack, id) = rack_with_neural(&mut app, &mut runtime, "Gappy", 0.25);
        for note in [38, 42] {
            let track = app.graph_controller().add_track(Path::new(SAMPLE)).expect("pad track");
            app.assign_rack_pad_track_recorded(rack, note, track).expect("pad");
        }
        let hat_member = {
            let group = app.groups.iter().find(|group| group.id == rack).unwrap();
            let rack = group.rack.as_ref().unwrap();
            rack.pads[rack.pad_index_for_note(42).unwrap()].member
        };
        runtime.set_global_value("src", Value::Instance(id));
        runtime
            .eval_str(&format!("(graph-node src 0 :route {hat_member})"))
            .expect("route to the hat");
        let path = save_kit(&mut app, rack, "Gappy-Kit", &[]);
        let mut kit = crate::project::load_kit_preset(&path).expect("kit reads back");
        let snare = kit.pads.iter_mut().find(|pad| pad.pad_note == 38).expect("snare pad");
        snare.sound = None;
        snare.modulator = None;
        std::fs::write(&path, serde_json::to_string(&kit).unwrap()).unwrap();

        let (loaded, failures) = app.load_kit_as_rack(&path).expect("kit loads");
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("carries neither"), "{failures:?}");
        let group = app.groups.iter().find(|group| group.id == loaded).unwrap();
        let loaded_rack = group.rack.as_ref().unwrap();
        assert_eq!(loaded_rack.pads.len(), 2);
        let loaded_hat = loaded_rack.pads[loaded_rack.pad_index_for_note(42).unwrap()].member;
        let fresh = only_instance(&app, loaded);
        let route = app
            .state
            .current_graph_overrides()
            .into_iter()
            .find(|graph| graph.sequencer_id == fresh)
            .and_then(|graph| {
                graph.node_intrinsics.iter().find(|n| n.instance == 0).and_then(|n| n.route.clone())
            });
        assert_eq!(route, Some(ProjectGraphRouteOverride::Track(loaded_hat)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// An instance whose package is not installed stays a placeholder: kept
    /// in the project, not published, and published once the kind registers.
    #[test]
    fn a_missing_kind_loads_as_a_placeholder_that_revives() {
        let mut app = headless_app();
        let _runtime = neural_runtime(&app);
        let (rack, _) = app.create_drum_rack_recorded(Some("Ghost".to_string())).expect("rack");
        let track = app.graph_controller().add_track(Path::new(SAMPLE)).expect("pad track");
        app.assign_rack_pad_track_recorded(rack, 36, track).expect("pad");
        let (mut kit, _) = app
            .capture_rack_as_kit(rack, "Ghost", Vec::new(), String::new(), &[])
            .expect("capture");
        kit.instances.push(ProjectKitInstance {
            id: 3,
            kind: "alez/missing:ghost".to_string(),
            label: "ghost 1".to_string(),
            overrides: Some(crate::graph::ProjectGraphOverrides {
                sequencer_id: 3,
                sequencer_name: "ghost#3".to_string(),
                max_poly: Some(5),
                ..Default::default()
            }),
        });
        let directory = std::env::temp_dir().join(format!("eseq-kit-ghost-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("Ghost.kit");
        std::fs::write(&path, serde_json::to_string(&kit).unwrap()).unwrap();

        let (loaded, failures) = app.load_kit_as_rack(&path).expect("loads");
        assert!(failures.is_empty(), "{failures:?}");
        let owned = app.rack_instances(loaded);
        assert_eq!(owned.len(), 1);
        let ghost = owned[0].id;
        assert_eq!(owned[0].label, "ghost 1");
        assert_eq!(published_owner(&app, ghost), None, "no kind, nothing published");
        assert!(
            app.state.current_graph_overrides().iter().any(|graph| graph.sequencer_id == ghost
                && graph.max_poly == Some(5)
                && graph.owner_rack == Some(loaded)),
            "its overrides wait under the fresh id"
        );

        let mut definition = crate::lisp_host::registered_kind(NEURAL).unwrap();
        definition.id = "alez/missing:ghost".to_string();
        definition.name = "ghost".to_string();
        crate::lisp_host::register_kind(definition);
        assert_eq!(app.publish_instance_sequencers(), 1, "the placeholder revives");
        assert_eq!(published_owner(&app, ghost), Some(Some(loaded)));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// Kits saved before instances recorded `(import m)`; a module that
    /// declares one kind becomes an instance record keyed by the old id.
    #[test]
    fn old_kit_import_sources_migrate_to_instance_records() {
        let declared = |module: &str| -> Vec<crate::lisp_host::DeclaredKind> {
            let kind = |id: &str| crate::lisp_host::DeclaredKind {
                id: id.to_string(),
                legacy_sequencer: None,
            };
            match module {
                "alez.neural.variable-reset" => vec![kind(NEURAL)],
                "demos.two" => vec![kind("demos:one"), kind("demos:two")],
                _ => Vec::new(),
            }
        };
        let mut kit: ProjectKitPreset = serde_json::from_value(serde_json::json!({
            "version": 13,
            "metadata": {"name": "Old", "tags": [], "author": ""},
            "pads": [],
            "kit_version": 3,
            "sequencers": [
                {"sequencer_id": 11, "sequencer_name": "variable-reset",
                 "source": "(import alez.neural.variable-reset)"},
                {"sequencer_id": 12, "sequencer_name": "plain",
                 "source": "(load \"@/scripts/plain.lisp\")"},
                {"sequencer_id": 13, "sequencer_name": "two", "source": "(import demos.two)"}
            ]
        }))
        .expect("old kit parses");
        let notes = migrate_kit_sequencers(&mut kit, declared);
        assert_eq!(
            kit.instances,
            vec![ProjectKitInstance {
                id: 11,
                kind: NEURAL.to_string(),
                label: String::new(),
                overrides: None,
            }]
        );
        let kept: Vec<_> = kit.sequencers.iter().map(|s| s.sequencer_name.as_str()).collect();
        assert_eq!(kept, vec!["plain", "two"]);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("declares 2 kinds"));
        let again = migrate_kit_sequencers(&mut kit, declared);
        assert_eq!(kit.instances.len(), 1, "idempotent");
        assert_eq!(again.len(), 1);
    }

    /// Node process slots of instance `id` in `graphs`: (slot id, the
    /// process-inlet wire targets of its bindings).
    fn node_slots(graphs: &[ProjectGraphOverrides], id: u64) -> Vec<(u64, Vec<u64>)> {
        graphs
            .iter()
            .filter(|graph| graph.sequencer_id == id)
            .flat_map(|graph| graph.node_intrinsics.iter())
            .filter_map(|node| node.process_chain.as_ref())
            .flat_map(|chain| chain.slots.iter())
            .map(|slot| {
                let wires = slot
                    .bindings
                    .values()
                    .flatten()
                    .filter_map(|target| match target {
                        crate::process::ParamTarget::ProcessInlet { instance_id, .. } => {
                            instance_id.map(|id| id.0)
                        }
                        _ => None,
                    })
                    .collect();
                (slot.instance_id.0, wires)
            })
            .collect()
    }

    /// Two `count` slots on node 0 of `id`, the second's reset wired from
    /// the first's output. Returns the slot ids.
    fn wired_count_slots(app: &App, runtime: &mut Runtime, id: u64) -> (u64, u64) {
        runtime.set_global_value("proc", Value::Instance(id));
        let add = |runtime: &mut Runtime| match runtime
            .eval_str("(graph-node-process-add proc 0 \"lane-count\")")
            .expect("add a count slot")
        {
            Some(Value::Number(slot)) => slot as u64,
            other => panic!("slot id, got {other:?}"),
        };
        let first = add(runtime);
        let second = add(runtime);
        app.state
            .edit_current_graph_overrides(|graphs| {
                let graph = graphs.iter_mut().find(|graph| graph.sequencer_id == id).unwrap();
                for node in &mut graph.node_intrinsics {
                    for slot in node.process_chain.iter_mut().flat_map(|chain| chain.slots.iter_mut()) {
                        if slot.instance_id.0 == first {
                            slot.bindings.insert(
                                "out".to_string(),
                                Some(crate::process::ParamTarget::ProcessInlet {
                                    process: "lane-count".to_string(),
                                    inlet: "reset".to_string(),
                                    instance_id: Some(crate::process::ProcessInstanceId(second)),
                                }),
                            );
                        }
                    }
                }
                Ok(())
            })
            .unwrap();
        (first, second)
    }

    /// The scheduler keys a node slot's runtime state (a `count`'s counter,
    /// a rand stream) by its slot id alone. A duplicate, and every load of a
    /// kit, must therefore carry fresh slot ids, with the wires between its
    /// slots following, or the "independent" copies would share counters.
    #[test]
    fn duplicates_and_kit_loads_get_their_own_node_process_state() {
        let mut app = headless_app();
        let mut runtime = neural_runtime(&app);
        let (rack, source) = rack_with_neural(&mut app, &mut runtime, "Proc", 0.25);
        let (first, second) = wired_count_slots(&app, &mut runtime, source);
        assert_eq!(
            node_slots(&app.state.current_graph_overrides(), source),
            vec![(first, vec![second]), (second, vec![])]
        );
        let runtime_id = |slot: u64| {
            let slot = crate::process::TrackProcessSlot {
                instance_id: crate::process::ProcessInstanceId(slot),
                instance_name: None,
                class_name: "lane-count".to_string(),
                enabled: true,
                project_layer: false,
                inlets: Default::default(),
                lanes: Default::default(),
                bindings: Default::default(),
                fanout: Default::default(),
                unbound_ports: Default::default(),
            };
            crate::process::track_process_slot_runtime_id(&slot, 0)
        };

        let copy = app.duplicate_instance_recorded(source).expect("duplicate");
        let copied = node_slots(&app.state.current_graph_overrides(), copy);
        assert_eq!(copied.len(), 2, "the copy keeps both slots");
        let (copy_first, copy_second) = (copied[0].0, copied[1].0);
        assert!(![first, second].contains(&copy_first) && ![first, second].contains(&copy_second));
        assert_ne!(runtime_id(copy_first), runtime_id(first), "its own counter state");
        assert_eq!(copied[0].1, vec![copy_second], "the wire follows to the copy's own slot");
        assert_eq!(
            node_slots(&app.state.current_graph_overrides(), source),
            vec![(first, vec![second]), (second, vec![])],
            "the source is untouched"
        );

        app.delete_instance_recorded(copy).expect("drop the copy again");
        let path = save_kit(&mut app, rack, "Proc-Kit", &[]);
        let (one, failures) = app.load_kit_as_rack(&path).expect("loads");
        assert!(failures.is_empty(), "{failures:?}");
        let (two, failures) = app.load_kit_as_rack(&path).expect("loads again");
        assert!(failures.is_empty(), "{failures:?}");
        let current = app.state.current_graph_overrides();
        let mut seen: std::collections::HashSet<u64> = [first, second].into();
        for loaded in [only_instance(&app, one), only_instance(&app, two)] {
            let slots = node_slots(&current, loaded);
            assert_eq!(slots.len(), 2);
            assert_eq!(slots[0].1, vec![slots[1].0], "the wire stays inside this load");
            for (slot, _) in slots {
                assert!(seen.insert(slot), "slot {slot} is shared between two instances");
            }
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A rack's instances do not outlive it: dissolving the rack gives them
    /// back to the project (routes expanded to the tracks), so a rack created
    /// later, even one that reuses the id, never adopts them. One undo brings
    /// the rack and its ownership back.
    #[test]
    fn removing_a_rack_releases_its_instances_and_a_new_rack_adopts_none() {
        let mut app = headless_app();
        let mut runtime = neural_runtime(&app);
        let (rack, id) = rack_with_neural(&mut app, &mut runtime, "Gone", 0.25);
        let member = app.groups.iter().find(|group| group.id == rack).unwrap().members[0];

        app.delete_group_recorded(rack).expect("delete the rack");
        assert_eq!(app.instances.get(id).unwrap().owner, ProjectInstanceOwner::Project);
        assert_eq!(published_owner(&app, id), Some(None), "republished project-owned");
        let route = app
            .state
            .current_graph_overrides()
            .into_iter()
            .find(|graph| graph.sequencer_id == id)
            .and_then(|graph| graph.node_intrinsics.iter().find(|n| n.instance == 0)?.route.clone());
        assert_eq!(route, Some(ProjectGraphRouteOverride::Track(member)), "member 0 -> its track");

        let (fresh, _) = app.create_drum_rack_recorded(Some("New".to_string())).expect("new rack");
        assert!(app.rack_instances(fresh).is_empty(), "the new rack adopts nothing");
        applied(crate::app::edit::undo(&mut app));
        applied(crate::app::edit::undo(&mut app));
        assert!(app.groups.iter().any(|group| group.id == rack), "one undo restores the rack");
        assert_eq!(app.instances.get(id).unwrap().owner, ProjectInstanceOwner::Rack(rack));
        assert_eq!(published_owner(&app, id), Some(Some(rack)));

        // Were an instance ever left naming a removed rack, no new rack
        // takes that id.
        app.instances.get_mut(id).unwrap().owner = ProjectInstanceOwner::Rack(rack + 50);
        let (next, _) = app.create_drum_rack_recorded(None).expect("another rack");
        assert!(next > rack + 50);
    }

    /// Deleting a rack together with its tracks deletes its instances too,
    /// in the same undo entry.
    #[test]
    fn deleting_a_rack_with_its_members_deletes_its_instances() {
        let mut app = headless_app();
        let mut runtime = neural_runtime(&app);
        let (rack, id) = rack_with_neural(&mut app, &mut runtime, "Swap", 0.25);
        app.delete_group_with_members_recorded(rack).expect("delete with members");
        assert!(!app.instances.contains(id));
        assert_eq!(published_owner(&app, id), None);
        assert!(app.state.current_graph_overrides().iter().all(|graph| graph.sequencer_id != id));
        applied(crate::app::edit::undo(&mut app));
        assert_eq!(app.instances.get(id).unwrap().owner, ProjectInstanceOwner::Rack(rack));
        assert_eq!(weight(&mut runtime, id), Some(Value::Number(0.25)), "overrides come back");
    }

    /// A plain (non-rack) member's clips keep their own sound through a kit
    /// round trip. The kit pad is a one-slot rack Sound, but the clips carry
    /// the member's sound at track level; the load used to drop it, so every
    /// clip played the pad's single saved Sound (ChickenShit Kit's Digi FM).
    #[test]
    fn plain_member_clips_keep_their_own_sound_through_a_kit() {
        let mut app = headless_app();
        let mut runtime = neural_runtime(&app);
        let (rack, _) = rack_with_neural(&mut app, &mut runtime, "Plain", 0.25);
        app.convert_rack_to_clips_recorded(rack).expect("convert to clips");
        let track = app.groups.iter().find(|group| group.id == rack).unwrap().members[0];
        // Two clips whose sampler slot differs in its first param, each
        // launched by its own scene.
        let second_scene = app.state.with_scenes_mut(|scenes| {
            let first = scenes.rack_bank(rack).unwrap().clips[0].clone();
            let cell = first.cells[0].expect("the pad lane");
            let mut data = scenes.track_pools[track].get(cell).unwrap().clone();
            assert!(!data.instrument_slot.defaults.is_empty(), "sampler slot params");
            data.instrument_slot.defaults[0] = 0.2;
            let first_cell = scenes.track_pools[track].insert(data.clone());
            data.instrument_slot.defaults[0] = 0.8;
            let second_cell = scenes.track_pools[track].insert(data);
            let bank = scenes.rack_bank_mut(rack).unwrap();
            bank.clips[0].cells[0] = Some(first_cell);
            let mut second = bank.clips[0].clone();
            second.id = bank.next_clip_id;
            bank.next_clip_id += 1;
            second.cells[0] = Some(second_cell);
            let second_id = second.id;
            bank.clips.push(second);
            let scene = scenes.new_scene();
            scenes.scenes[scene].rack_clips = vec![(rack, second_id)];
            scene
        });
        let path = save_kit(&mut app, rack, "Plain-Kit", &[0, second_scene]);
        let kit = crate::project::load_kit_preset(&path).expect("kit reads back");
        assert_eq!(kit.clips.len(), 2);
        assert!(
            kit.clips[0].pattern.rack_tracks[0].is_none(),
            "a plain member's clip carries its sound at track level"
        );

        let (loaded, failures) = app.load_kit_as_rack(&path).expect("kit loads");
        assert!(failures.is_empty(), "{failures:?}");
        let pad_track = app.groups.iter().find(|group| group.id == loaded).unwrap().members[0];
        let played: Vec<f32> = app.state.with_scenes(|scenes| {
            scenes
                .rack_bank(loaded)
                .unwrap()
                .clips
                .iter()
                .map(|clip| {
                    let cell = clip.cells[0].expect("pad lane");
                    let data = scenes.track_pools[pad_track].get(cell).unwrap();
                    let rack = data.rack_track.as_ref().expect("the pad plays a rack slot");
                    rack.slots[0].instrument_slot.defaults[0]
                })
                .collect()
        });
        assert_eq!(played, vec![0.2, 0.8], "each clip keeps its own sound");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Id-keyed instance edits reach every clip of a rack's bank, launched
    /// or not, and a clip two scenes share is moved once, for both.
    #[test]
    fn instance_edits_reach_unlaunched_clips_and_shared_clips() {
        let mut app = headless_app();
        let mut runtime = neural_runtime(&app);
        let (rack, source) = rack_with_neural(&mut app, &mut runtime, "Clips", 0.25);
        app.convert_rack_to_clips_recorded(rack).expect("convert to clips");
        // A second, unlaunched clip holding the instance at weight 0.75, and
        // a second scene sharing the first clip.
        let unlaunched = app.state.with_scenes_mut(|scenes| {
            let bank = scenes.rack_bank_mut(rack).unwrap();
            let mut clip = bank.clips[0].clone();
            clip.id = bank.clips.iter().map(|clip| clip.id).max().unwrap() + 1;
            bank.next_clip_id = clip.id + 1;
            for graph in &mut clip.graph_overrides {
                for edge in &mut graph.edge_params {
                    edge.value = 0.75;
                }
            }
            let id = clip.id;
            bank.clips.push(clip);
            let shared = scenes.scenes[0].rack_clips.clone();
            let scene = scenes.new_scene();
            scenes.scenes[scene].rack_clips = shared;
            id
        });
        let clip_weights = |app: &App, id: u64| -> Vec<(u64, f64)> {
            app.state.with_scenes(|scenes| {
                scenes
                    .rack_bank(rack)
                    .map(|bank| {
                        bank.clips
                            .iter()
                            .flat_map(|clip| {
                                clip.graph_overrides
                                    .iter()
                                    .filter(|graph| graph.sequencer_id == id)
                                    .flat_map(|graph| graph.edge_params.iter())
                                    .map(move |edge| (clip.id, edge.value))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
        };
        // (`new_scene` may mint a clip of its own; only its pointer is
        // replaced, so the bank can hold one more unlaunched clip.)
        let baseline = clip_weights(&app, source);
        assert!(baseline.contains(&(unlaunched, 0.75)), "{baseline:?}");

        let copy = app.duplicate_instance_recorded(source).expect("duplicate");
        assert_eq!(
            clip_weights(&app, copy),
            baseline,
            "the copy carries the unlaunched clips' overrides too"
        );
        app.delete_instance_recorded(copy).expect("delete the copy");
        assert!(clip_weights(&app, copy).is_empty(), "delete reaches the unlaunched clip");

        app.move_instance_owner_recorded(source, ProjectInstanceOwner::Project).expect("out");
        let scene_weights: Vec<Vec<f64>> = app.state.with_scenes(|scenes| {
            scenes
                .scenes
                .iter()
                .map(|scene| {
                    scene
                        .graph_overrides
                        .iter()
                        .filter(|graph| graph.sequencer_id == source)
                        .flat_map(|graph| graph.edge_params.iter().map(|edge| edge.value))
                        .collect()
                })
                .collect()
        });
        assert_eq!(scene_weights, vec![vec![0.25], vec![0.25]], "both scenes sharing the clip keep it");
        assert!(clip_weights(&app, source).is_empty());

        app.move_instance_owner_recorded(source, ProjectInstanceOwner::Rack(rack)).expect("back in");
        let moved_in = clip_weights(&app, source);
        assert_eq!(moved_in.len(), baseline.len(), "every clip of the bank gets the instance");
        assert!(moved_in.iter().all(|(_, weight)| *weight == 0.25), "{moved_in:?}");
        applied(crate::app::edit::undo(&mut app));
        applied(crate::app::edit::undo(&mut app));
        assert_eq!(clip_weights(&app, source), baseline, "undo restores every clip exactly");
    }

    /// A clip override keyed by an id the kit does not record (a deleted
    /// instance's leftovers) does not travel into a kit, and a kit carrying
    /// one anyway drops it on load rather than matching an unrelated
    /// instance of the importing project.
    #[test]
    fn stale_clip_overrides_neither_export_nor_import() {
        let mut app = headless_app();
        let mut runtime = neural_runtime(&app);
        let (rack, source) = rack_with_neural(&mut app, &mut runtime, "Stale", 0.25);
        app.convert_rack_to_clips_recorded(rack).expect("convert to clips");
        let stale = crate::graph::ProjectGraphOverrides {
            sequencer_id: 9_999,
            sequencer_name: "gone#9999".to_string(),
            max_poly: Some(1),
            owner_rack: Some(rack),
            ..Default::default()
        };
        app.state.with_scenes_mut(|scenes| {
            scenes.rack_bank_mut(rack).unwrap().clips[0].graph_overrides.push(stale.clone());
        });
        let path = save_kit(&mut app, rack, "Stale-Kit", &[0]);
        let mut kit = crate::project::load_kit_preset(&path).expect("kit reads back");
        let ids: Vec<u64> = kit.clips[0].graph_overrides.iter().map(|graph| graph.sequencer_id).collect();
        assert_eq!(ids, vec![source], "only what the kit records travels");

        // An older kit that carries one anyway, keyed by a live instance id.
        let bystander = app
            .create_instance_recorded(NEURAL, ProjectInstanceOwner::Project, None)
            .expect("unrelated instance");
        kit.clips[0].graph_overrides.push(crate::graph::ProjectGraphOverrides {
            sequencer_id: bystander,
            ..stale
        });
        std::fs::write(&path, serde_json::to_string(&kit).unwrap()).unwrap();
        let (loaded, failures) = app.load_kit_as_rack(&path).expect("loads");
        assert!(failures.is_empty(), "{failures:?}");
        let fresh = only_instance(&app, loaded);
        let ids: Vec<u64> = app.state.with_scenes(|scenes| {
            scenes.rack_bank(loaded).unwrap().clips[0]
                .graph_overrides
                .iter()
                .map(|graph| graph.sequencer_id)
                .collect()
        });
        assert_eq!(ids, vec![fresh], "the unrecorded override is dropped");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    fn applied<E: std::fmt::Debug>(replay: crate::app::history::HistoryReplay<E>) {
        assert!(matches!(replay, crate::app::history::HistoryReplay::Applied(_)), "{replay:?}");
    }
}
