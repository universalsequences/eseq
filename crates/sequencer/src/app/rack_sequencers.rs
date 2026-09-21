//! Rack-owned graph sequencers (`docs/rack-clips-and-break-kits-spec.md` §5).
//!
//! A graph `def-sequencer` can belong to a drum rack instead of the project.
//! Its routes and seed tracks are then MEMBER indices into the rack, its
//! instance id is namespaced by the rack (`graph_instance_id`), and the rack
//! config records how to bring it back (`ProjectRackSequencer::source`). The
//! scheduler resolves member -> track from the membership mirror this module
//! publishes; nothing here touches the runtime.

use super::*;
use crate::graph::{ProjectGraphOverrides, ProjectGraphRouteOverride, ProjectGraphSeedFrom, RackMembership};
use crate::project::ProjectRackSequencer;

/// `Some(module)` when a recorded rack sequencer source is a one-line
/// `(import module)` form, which is how package scripts are recorded.
pub fn rack_sequencer_module(source: &str) -> Option<String> {
    let inner = source.trim().strip_prefix("(import ")?.strip_suffix(')')?.trim();
    let module = inner.split_whitespace().next()?;
    (!module.is_empty() && !module.starts_with(':')).then(|| module.to_string())
}

impl App {
    /// Tell the Lisp side which modules' graph sequencers belong to which
    /// rack, so a `def-sequencer` inside an imported module publishes as
    /// rack-owned however the module gets imported (spec §5.1).
    pub fn publish_rack_owner_modules(&self) {
        let owners = self
            .groups
            .iter()
            .filter_map(|group| group.rack.as_ref().map(|rack| (group.id, rack)))
            .flat_map(|(group_id, rack)| {
                rack.sequencers
                    .iter()
                    .filter_map(move |s| rack_sequencer_module(&s.source).map(|m| (m, group_id)))
            })
            .collect();
        crate::lisp_host::set_rack_owner_modules(owners);
    }

    /// Every drum rack's member tracks in member order, for the scheduler's
    /// member-route resolution.
    pub fn rack_memberships(&self) -> Vec<RackMembership> {
        self.groups
            .iter()
            .filter(|group| group.rack.is_some())
            .map(|group| RackMembership { group_id: group.id, members: group.members.clone() })
            .collect()
    }

    /// The sequencers a rack owns, in attach order.
    pub fn rack_sequencers(&self, group_id: u64) -> Vec<ProjectRackSequencer> {
        self.groups
            .iter()
            .find(|group| group.id == group_id)
            .and_then(|group| group.rack.as_ref())
            .map(|rack| rack.sequencers.clone())
            .unwrap_or_default()
    }

    /// Record a sequencer instance the host just evaluated under `group_id`.
    /// Upserts by id, so re-attaching (hot reload) keeps one record.
    pub fn attach_rack_sequencer_recorded(
        &mut self,
        group_id: u64,
        sequencer_id: u64,
        sequencer_name: &str,
        source: &str,
    ) -> Result<(), String> {
        let entry = ProjectRackSequencer {
            sequencer_id,
            sequencer_name: sequencer_name.to_string(),
            source: source.to_string(),
        };
        self.apply_recorded_bus_group_structure_mutation("Attach sequencer to rack", move |app| {
            let rack = app.rack_config_mut(group_id)?;
            match rack.sequencers.iter_mut().find(|s| s.sequencer_id == entry.sequencer_id) {
                Some(existing) => *existing = entry,
                None => rack.sequencers.push(entry),
            }
            app.publish_rack_owner_modules();
            Ok(())
        })
    }

    /// Give a rack-owned sequencer back to the project: member routes expand
    /// to the tracks they currently resolve to, the overrides re-key to the
    /// project-owned instance id, and the rack instance is unpublished. The
    /// script itself is not re-evaluated here; the host re-imports it so it
    /// republishes project-owned and picks the expanded overrides up by name.
    /// Returns the sequencer's name and its recorded source form.
    pub fn detach_rack_sequencer_recorded(
        &mut self,
        group_id: u64,
        sequencer_id: u64,
    ) -> Result<(String, String), String> {
        let members = self.rack_member_tracks(group_id)?;
        self.apply_recorded_bus_group_structure_mutation("Detach sequencer from rack", move |app| {
            let rack = app.rack_config_mut(group_id)?;
            let position = rack
                .sequencers
                .iter()
                .position(|s| s.sequencer_id == sequencer_id)
                .ok_or_else(|| format!("Rack {group_id} does not own sequencer {sequencer_id}"))?;
            let entry = rack.sequencers.remove(position);
            let project_id = crate::lisp_host::graph_instance_id(&entry.sequencer_name, None);
            app.state.edit_all_scene_graph_overrides(|graphs| {
                let mut changed = false;
                for graph in graphs.iter_mut() {
                    if graph.sequencer_id != sequencer_id || graph.owner_rack != Some(group_id) {
                        continue;
                    }
                    expand_member_routes_to_tracks(graph, &members);
                    graph.owner_rack = None;
                    graph.sequencer_id = project_id;
                    changed = true;
                }
                changed
            });
            app.state.unpublish_sequencer_by_id(sequencer_id);
            app.publish_rack_owner_modules();
            Ok((entry.sequencer_name, entry.source))
        })
    }

    /// Move a project-owned graph sequencer into a rack. Every explicit route
    /// and seed track it uses, in every scene, is rewritten: a track that is a
    /// member of the rack becomes that member's index, and a track outside the
    /// rack becomes "off" (seed lists simply drop it). Nothing is refused.
    /// Returns the rack-owned instance id.
    pub fn move_sequencer_into_rack_recorded(
        &mut self,
        group_id: u64,
        sequencer_id: u64,
        source: &str,
    ) -> Result<u64, String> {
        let members = self.rack_member_tracks(group_id)?;
        let published = self
            .state
            .published_sequencers()
            .into_iter()
            .find(|published| published.id == sequencer_id)
            .ok_or_else(|| format!("Sequencer {sequencer_id} is not published"))?;
        let manifest = published
            .graph
            .clone()
            .ok_or_else(|| format!("'{}' is not a graph sequencer", published.name))?;
        if manifest.owner_rack.is_some() {
            return Err(format!("'{}' already belongs to a rack", manifest.name));
        }
        // The manifest's default `:route n` is read as member n once the rack
        // owns it (a demo's `:route 0` lands on the first pad); only explicit
        // per-node overrides are remapped here.
        let rack_id = crate::lisp_host::graph_instance_id(&manifest.name, Some(group_id));
        let entry = ProjectRackSequencer {
            sequencer_id: rack_id,
            sequencer_name: manifest.name.clone(),
            source: source.to_string(),
        };
        let mut rack_manifest = manifest.clone();
        rack_manifest.id = rack_id;
        rack_manifest.owner_rack = Some(group_id);
        let mut rack_published = published.clone();
        rack_published.id = rack_id;
        rack_published.graph = Some(rack_manifest);
        self.apply_recorded_bus_group_structure_mutation("Move sequencer into rack", move |app| {
            let rack = app.rack_config_mut(group_id)?;
            match rack.sequencers.iter_mut().find(|s| s.sequencer_id == entry.sequencer_id) {
                Some(existing) => *existing = entry,
                None => rack.sequencers.push(entry),
            }
            app.state.edit_all_scene_graph_overrides(|graphs| {
                let mut changed = false;
                for graph in graphs.iter_mut() {
                    if !manifest.matches_overrides(graph) {
                        continue;
                    }
                    contract_track_routes_to_members(graph, &members);
                    graph.owner_rack = Some(group_id);
                    graph.sequencer_id = rack_id;
                    changed = true;
                }
                changed
            });
            app.state.unpublish_sequencer_by_id(sequencer_id);
            app.state.publish_sequencer(rack_published);
            app.publish_rack_owner_modules();
            Ok(rack_id)
        })
    }

    /// Member `position` just left rack `group_id`: its cell drops out of every
    /// rack clip, nodes routed to it go silent and later members shift down, in
    /// every scene. Called from the group-member funnels, inside whatever
    /// recorded edit removed the member.
    pub(crate) fn remap_rack_sequencer_routes_after_member_removed(
        &self,
        group_id: u64,
        position: usize,
    ) {
        // Rack clips are positional over members too (rack-clips spec §3): the
        // leaving member's cell drops out of every clip, keeping
        // `clip.cells.len() == members.len()`.
        self.state
            .with_scenes_mut(|scenes| scenes.rack_clip_member_removed(group_id, position));
        let owns_sequencers = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .and_then(|group| group.rack.as_ref())
            .is_some_and(|rack| !rack.sequencers.is_empty());
        if !owns_sequencers {
            return;
        }
        self.state.edit_all_scene_graph_overrides(|graphs| {
            let mut changed = false;
            for graph in graphs.iter_mut() {
                changed |= graph.remap_after_rack_member_removed(group_id, position);
            }
            changed
        });
    }

    fn rack_member_tracks(&self, group_id: u64) -> Result<Vec<usize>, String> {
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        if group.rack.is_none() {
            return Err(format!("Track group {group_id} is not a drum rack"));
        }
        Ok(group.members.clone())
    }

    fn rack_config_mut(&mut self, group_id: u64) -> Result<&mut crate::project::ProjectRackConfig, String> {
        self.groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?
            .rack
            .as_mut()
            .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))
    }
}

/// Rewrite a rack-owned override's member-relative routes and seed tracks
/// through `map`, indexed by the OLD position: `map[old] = Some(new)` moves
/// it, `None` drops it to "off". Break kits use this twice — member position
/// -> pad on export, pad -> member position on import (§7).
pub(crate) fn remap_graph_member_routes(graph: &mut ProjectGraphOverrides, map: &[Option<usize>]) {
    for intrinsic in &mut graph.node_intrinsics {
        if let Some(ProjectGraphRouteOverride::Track(from)) = intrinsic.route {
            intrinsic.route = Some(
                map.get(from)
                    .copied()
                    .flatten()
                    .map(ProjectGraphRouteOverride::Track)
                    .unwrap_or(ProjectGraphRouteOverride::None),
            );
        }
        if let Some(ProjectGraphSeedFrom::Tracks(seed)) = &intrinsic.seed_from {
            intrinsic.seed_from = Some(ProjectGraphSeedFrom::Tracks(
                seed.iter()
                    .filter_map(|from| map.get(*from).copied().flatten())
                    .collect(),
            ));
        }
    }
}

fn expand_member_routes_to_tracks(graph: &mut ProjectGraphOverrides, members: &[usize]) {
    for intrinsic in &mut graph.node_intrinsics {
        if let Some(ProjectGraphRouteOverride::Track(member)) = intrinsic.route {
            intrinsic.route = Some(
                members
                    .get(member)
                    .map(|track| ProjectGraphRouteOverride::Track(*track))
                    .unwrap_or(ProjectGraphRouteOverride::None),
            );
        }
        if let Some(ProjectGraphSeedFrom::Tracks(seed)) = &intrinsic.seed_from {
            intrinsic.seed_from = Some(ProjectGraphSeedFrom::Tracks(
                seed.iter().filter_map(|member| members.get(*member).copied()).collect(),
            ));
        }
    }
}

fn contract_track_routes_to_members(graph: &mut ProjectGraphOverrides, members: &[usize]) {
    let member_of = |track: usize| members.iter().position(|member| *member == track);
    for intrinsic in &mut graph.node_intrinsics {
        if let Some(ProjectGraphRouteOverride::Track(track)) = intrinsic.route {
            intrinsic.route = Some(
                member_of(track)
                    .map(ProjectGraphRouteOverride::Track)
                    .unwrap_or(ProjectGraphRouteOverride::None),
            );
        }
        if let Some(ProjectGraphSeedFrom::Tracks(seed)) = &intrinsic.seed_from {
            intrinsic.seed_from = Some(ProjectGraphSeedFrom::Tracks(
                seed.iter().filter_map(|track| member_of(*track)).collect(),
            ));
        }
    }
}
