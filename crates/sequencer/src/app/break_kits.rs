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

use super::*;
use std::path::Path;

use crate::graph::ProjectGraphOverrides;
use crate::project::{ProjectPattern, ProjectRackClip, ProjectRackSequencer};
use crate::sequencer::{PatternSnapshot, TrackPatternData};

/// What the export of one rack's sequencers + chosen scenes produced, plus the
/// per-sequencer warnings the caller reports (a `(load "path")` whose file is
/// gone travels as a dead reference).
pub(super) struct CapturedKitContent {
    pub sequencers: Vec<ProjectRackSequencer>,
    pub clips: Vec<ProjectRackClip>,
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
                clips: Vec::new(),
                warnings,
            });
        }
        if self.rack_is_legacy(group_id) {
            self.convert_rack_to_clips_recorded(group_id)?;
        }

        // member position -> pad position, and the pad count. A member with no
        // pad cannot travel: a kit rebuilds members FROM pads.
        let (member_to_pad, pad_count) = {
            let group = self
                .groups
                .iter()
                .find(|group| group.id == group_id)
                .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
            let rack = group
                .rack
                .as_ref()
                .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
            let map: Vec<Option<usize>> = (0..group.members.len())
                .map(|member| rack.pad_index_for_member(member))
                .collect();
            (map, rack.pads.len())
        };
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
                .filter(|graph| graph.owner_rack == Some(group_id))
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
            clips,
            warnings,
        })
    }

    /// Register the kit's rack-owned sequencers on `group_id`.
    ///
    /// The recorded `sequencer_id` belongs to the EXPORTING rack — a rack-owned
    /// instance id is derived from its owner's group id (`graph_instance_id`)
    /// — so each one is re-derived here for the rack being built, and the map
    /// old id -> new id is handed back for the clip overrides to follow.
    /// Nothing is evaluated: the App cannot run Lisp, so the host evaluates the
    /// recorded sources afterwards (§7.3). An entry whose module is missing
    /// stays recorded, so a later re-import brings it back.
    pub(super) fn register_kit_sequencers(
        &mut self,
        group_id: u64,
        sequencers: &[ProjectRackSequencer],
        failures: &mut Vec<String>,
    ) -> HashMap<u64, u64> {
        let mut id_map = HashMap::new();
        for sequencer in sequencers {
            let new_id =
                crate::lisp_host::graph_instance_id(&sequencer.sequencer_name, Some(group_id));
            id_map.insert(sequencer.sequencer_id, new_id);
            if let Err(error) = self.attach_rack_sequencer_recorded(
                group_id,
                new_id,
                &sequencer.sequencer_name,
                &sequencer.source,
            ) {
                failures.push(format!("sequencer '{}': {error}", sequencer.sequencer_name));
            }
        }
        id_map
    }

    /// Install a kit's clip bank onto `group_id`, replacing whatever bank it
    /// had. No project scene is pointed at any of the new clips: the rack is
    /// SILENT until the user launches one (§7.3), which is the whole point of
    /// dropping a break into a project that already has scenes.
    pub(super) fn install_kit_clips(
        &mut self,
        group_id: u64,
        clips: Vec<ProjectRackClip>,
        id_map: &HashMap<u64, u64>,
    ) -> Result<(), String> {
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let rack = group
            .rack
            .as_ref()
            .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
        let members = group.members.clone();
        // pad position -> member position, through the pad map the loader just
        // built. A pad whose Sound failed to load has no member and its lane is
        // simply dropped.
        let pad_to_position: Vec<Option<usize>> = (0..rack.pads.len())
            .map(|pad| {
                let member = rack.pads.get(pad)?.member;
                (member < members.len()).then_some(member)
            })
            .collect();
        let num_tracks = self.tracks.len();

        let mut assets: HashMap<PathBuf, ProjectSampleAsset> = HashMap::new();
        let mut built = Vec::with_capacity(clips.len());
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
                    clip.members
                        .get(pad)
                        .copied()
                        .unwrap_or(false)
                        .then(|| snapshot.track_pattern_data(*track))
                        .flatten()
                })
                .collect();
            let graph_overrides: Vec<ProjectGraphOverrides> = clip
                .graph_overrides
                .into_iter()
                .map(|mut graph| {
                    super::rack_sequencers::remap_graph_member_routes(&mut graph, &pad_to_position);
                    graph.owner_rack = Some(group_id);
                    if let Some(new_id) = id_map.get(&graph.sequencer_id) {
                        graph.sequencer_id = *new_id;
                    }
                    graph
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
