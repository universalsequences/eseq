//! Rack clips (`docs/rack-clips-and-break-kits-spec.md` §2–4).
//!
//! A drum rack gains its own scene axis: a per-rack bank of **clips**, and one
//! `Option<RackClipId>` pointer per project scene. A clip holds the rack-scoped
//! slice of what a project scene holds today — one pattern cell per member
//! (positional over `group.members`) and the overrides of every sequencer the
//! rack owns.
//!
//! The clip's member cells are `Option<PatternId>` into the member track's own
//! `TrackPatternPool`, exactly like a scene cell. That is the whole trick: the
//! composition step in `scene_snapshot`/`launch_scene` only has to answer "which
//! pattern id does track `m` play in scene `s`" differently for rack members, so
//! every read and write funnel downstream (step edits, p-locks, roster, pads)
//! redirects into the active clip for free, and the scheduler and audio path
//! never learn that racks exist.
//!
//! A rack with an empty bank is a **legacy rack** (§4.3): the composition step
//! is skipped entirely and its members' data stays in the project scenes, so
//! pre-feature projects behave exactly as they did.

use super::*;

pub type RackClipId = u64;

/// One clip in a rack's bank.
#[derive(Clone, Debug, Default)]
pub struct RackClip {
    pub id: RackClipId,
    pub name: String,
    pub color: Option<[f32; 3]>,
    /// Positional over `group.members`; `len() == members.len()` is an
    /// invariant maintained by the join/leave funnels. `None` = that member is
    /// silent in this clip.
    pub cells: Vec<Option<PatternId>>,
    /// The rack-owned sequencer overrides, unresolved (routes are MEMBER
    /// indices, `owner_rack` set). The scheduler resolves member → track from
    /// `rack_memberships`, so the snapshot carries these exactly as a scene
    /// carries its own overrides.
    pub graph_overrides: Vec<ProjectGraphOverrides>,
}

/// One rack's clip bank. `members` mirrors `group.members` so composition needs
/// nothing but `ProjectScenes`; it is refreshed from the same group-topology
/// funnel that publishes `rack_memberships`.
#[derive(Clone, Debug, Default)]
pub struct RackClipBank {
    pub group_id: u64,
    pub members: Vec<usize>,
    pub clips: Vec<RackClip>,
    pub next_clip_id: RackClipId,
}

impl RackClipBank {
    pub fn clip(&self, id: RackClipId) -> Option<&RackClip> {
        self.clips.iter().find(|clip| clip.id == id)
    }

    pub fn clip_mut(&mut self, id: RackClipId) -> Option<&mut RackClip> {
        self.clips.iter_mut().find(|clip| clip.id == id)
    }

    fn mint_id(&mut self) -> RackClipId {
        let id = self.next_clip_id.max(1);
        self.next_clip_id = id + 1;
        id
    }
}

impl ProjectScenes {
    pub fn rack_banks(&self) -> &[RackClipBank] {
        &self.rack_banks
    }

    pub fn rack_bank(&self, group_id: u64) -> Option<&RackClipBank> {
        self.rack_banks.iter().find(|bank| bank.group_id == group_id)
    }

    pub fn rack_bank_mut(&mut self, group_id: u64) -> Option<&mut RackClipBank> {
        self.rack_banks
            .iter_mut()
            .find(|bank| bank.group_id == group_id)
    }

    /// Project load installs the banks wholesale.
    pub fn install_rack_banks(&mut self, banks: Vec<RackClipBank>) {
        self.rack_banks = banks;
        self.repair_rack_clips();
        self.adopt_live_rack_clips();
    }

    /// Declare that the live grid now holds what the current scene's pointers
    /// name. Every site that installs the live lanes (scene launch) or that
    /// mints/forks a clip out of the live lanes calls this; pointing a scene at
    /// a DIFFERENT clip deliberately does not, which is what makes the lane
    /// stale until the relaunch.
    pub fn adopt_live_rack_clips(&mut self) {
        let scene = self.current_scene;
        self.live_rack_clips = self
            .rack_banks
            .iter()
            .map(|bank| (bank.group_id, self.scene_rack_clip(scene, bank.group_id)))
            .collect();
    }

    /// Whether the live lane of a rack member holds content belonging to a clip
    /// the current scene no longer points at.
    pub fn rack_member_lane_is_stale(&self, scene_idx: usize, track: usize) -> bool {
        let Some((group_id, _)) = self.rack_member_slot(track) else {
            return false;
        };
        let live = self
            .live_rack_clips
            .iter()
            .find(|(gid, _)| *gid == group_id)
            .map(|(_, clip)| *clip);
        match live {
            Some(live) => live != self.scene_rack_clip(scene_idx, group_id),
            // No live record yet (a bank created mid-session): trust the grid.
            None => false,
        }
    }

    /// A rack's bank, created empty if the rack has none yet. `members` is the
    /// rack's member track list.
    fn bank_for(&mut self, group_id: u64, members: &[usize]) -> &mut RackClipBank {
        if self.rack_bank(group_id).is_none() {
            self.rack_banks.push(RackClipBank {
                group_id,
                members: members.to_vec(),
                clips: Vec::new(),
                next_clip_id: 1,
            });
        }
        let bank = self
            .rack_banks
            .iter_mut()
            .find(|bank| bank.group_id == group_id)
            .expect("just ensured");
        if !members.is_empty() || bank.members.is_empty() {
            bank.members = members.to_vec();
        }
        bank
    }

    /// Refresh every bank's member mirror from the live group topology. Called
    /// from the same funnel that publishes `rack_memberships`; it only rewrites
    /// track indices (reindex-on-delete), never the clip cell count — join and
    /// leave go through their own funnels so a positional cell is added or
    /// dropped exactly once, at the right position.
    pub fn sync_rack_members(&mut self, memberships: &[crate::graph::RackMembership]) {
        for bank in &mut self.rack_banks {
            let Some(membership) = memberships
                .iter()
                .find(|membership| membership.group_id == bank.group_id)
            else {
                continue;
            };
            bank.members = membership.members.clone();
        }
        // A rack that no longer exists loses its bank and every pointer to
        // it. The membership list is authoritative whenever it is published
        // (including "no racks at all"): a stale bank would otherwise keep
        // claiming its former members as clip-resolved tracks.
        let live: Vec<u64> = memberships.iter().map(|m| m.group_id).collect();
        self.rack_banks.retain(|bank| live.contains(&bank.group_id));
        for scene in &mut self.scenes {
            scene
                .rack_clips
                .retain(|(group_id, _)| live.contains(group_id));
        }
        self.live_rack_clips.retain(|(group_id, _)| live.contains(group_id));
        self.repair_rack_clips();
    }

    /// Member `position` joined rack `group_id`: every clip grows a silent cell
    /// there so `cells.len() == members.len()` holds.
    pub fn rack_clip_member_inserted(&mut self, group_id: u64, position: usize) {
        let Some(bank) = self.rack_bank_mut(group_id) else {
            return;
        };
        for clip in &mut bank.clips {
            let at = position.min(clip.cells.len());
            clip.cells.insert(at, None);
        }
    }

    /// Member `position` left rack `group_id`: every clip drops its cell.
    pub fn rack_clip_member_removed(&mut self, group_id: u64, position: usize) {
        let Some(bank) = self.rack_bank_mut(group_id) else {
            return;
        };
        for clip in &mut bank.clips {
            if position < clip.cells.len() {
                clip.cells.remove(position);
            }
        }
    }

    /// §3 invariants: clip cell length matches the member count, and every
    /// scene pointer names a clip that still exists.
    pub fn repair_rack_clips(&mut self) {
        for bank in &mut self.rack_banks {
            let members = bank.members.len();
            let mut next = bank.next_clip_id;
            for clip in &mut bank.clips {
                clip.cells.resize(members, None);
                next = next.max(clip.id + 1);
            }
            bank.next_clip_id = next.max(1);
        }
        let live: Vec<(u64, Vec<RackClipId>)> = self
            .rack_banks
            .iter()
            .map(|bank| (bank.group_id, bank.clips.iter().map(|c| c.id).collect()))
            .collect();
        for scene in &mut self.scenes {
            scene.rack_clips.retain(|(group_id, clip_id)| {
                live.iter()
                    .find(|(gid, _)| gid == group_id)
                    .is_some_and(|(_, ids)| ids.contains(clip_id))
            });
        }
    }

    /// Which rack (if any) owns `track` as member `position`. Only racks with a
    /// clip bank answer: a legacy rack's members resolve through the scene.
    pub fn rack_member_slot(&self, track: usize) -> Option<(u64, usize)> {
        self.rack_banks.iter().find_map(|bank| {
            if bank.clips.is_empty() {
                return None;
            }
            bank.members
                .iter()
                .position(|member| *member == track)
                .map(|position| (bank.group_id, position))
        })
    }

    pub fn scene_rack_clip(&self, scene_idx: usize, group_id: u64) -> Option<RackClipId> {
        self.scenes
            .get(scene_idx)?
            .rack_clips
            .iter()
            .find(|(gid, _)| *gid == group_id)
            .map(|(_, clip)| *clip)
    }

    pub fn current_rack_clip(&self, group_id: u64) -> Option<RackClipId> {
        self.scene_rack_clip(self.current_scene, group_id)
    }

    /// Point scene `scene_idx` at `clip` for rack `group_id`; `None` silences
    /// the rack in that scene (§2, silence is explicit).
    pub fn set_scene_rack_clip(
        &mut self,
        scene_idx: usize,
        group_id: u64,
        clip: Option<RackClipId>,
    ) -> bool {
        if let Some(id) = clip {
            if self.rack_bank(group_id).and_then(|bank| bank.clip(id)).is_none() {
                return false;
            }
        }
        let Some(scene) = self.scenes.get_mut(scene_idx) else {
            return false;
        };
        scene.rack_clips.retain(|(gid, _)| *gid != group_id);
        if let Some(id) = clip {
            scene.rack_clips.push((group_id, id));
        }
        true
    }

    /// The composition rule of §4.1 step 2, as one lookup: `Some(cell)` when
    /// `track` belongs to a clip-bearing rack (the inner `None` being "silent
    /// in this scene"), `None` when the scene's own cell governs.
    pub fn rack_composed_cell(
        &self,
        scene_idx: usize,
        track: usize,
    ) -> Option<Option<PatternId>> {
        let (group_id, position) = self.rack_member_slot(track)?;
        let Some(clip_id) = self.scene_rack_clip(scene_idx, group_id) else {
            return Some(None);
        };
        let cell = self
            .rack_bank(group_id)
            .and_then(|bank| bank.clip(clip_id))
            .and_then(|clip| clip.cells.get(position).copied())
            .flatten();
        Some(cell)
    }

    /// The pattern scene `scene_idx` plays on `track`, rack composition
    /// included. This is the one place the redirect happens for reads.
    pub fn composed_scene_cell(&self, scene_idx: usize, track: usize) -> Option<PatternId> {
        match self.rack_composed_cell(scene_idx, track) {
            Some(cell) => cell,
            None => self
                .scenes
                .get(scene_idx)
                .and_then(|scene| scene.cells.get(track))
                .copied()
                .flatten(),
        }
    }

    /// Scene overrides composed with every clip-bearing rack's active clip
    /// (§4.1). Rack-owned entries in the scene belong to legacy racks and stay;
    /// a rack with a bank contributes only through its pointed clip, so a
    /// `None` pointer registers nothing and its sequencers do not fire.
    pub fn composed_graph_overrides(&self, scene_idx: usize) -> Vec<ProjectGraphOverrides> {
        let Some(scene) = self.scenes.get(scene_idx) else {
            return Vec::new();
        };
        let banked = |group_id: u64| {
            self.rack_bank(group_id)
                .is_some_and(|bank| !bank.clips.is_empty())
        };
        let mut composed: Vec<ProjectGraphOverrides> = scene
            .graph_overrides
            .iter()
            .filter(|graph| !graph.owner_rack.is_some_and(banked))
            .cloned()
            .collect();
        for bank in &self.rack_banks {
            if bank.clips.is_empty() {
                continue;
            }
            let Some(clip_id) = self.scene_rack_clip(scene_idx, bank.group_id) else {
                continue;
            };
            let Some(clip) = bank.clip(clip_id) else {
                continue;
            };
            composed.extend(clip.graph_overrides.iter().cloned());
        }
        composed
    }

    /// Split an edited override list back into the scene and the clip banks.
    /// Rack-owned entries of a clip-bearing rack land in that rack's active
    /// clip, creating one under a `None` pointer (§4.2: editing silence is
    /// never a dead end); everything else stays in the scene.
    pub fn store_composed_graph_overrides(
        &mut self,
        scene_idx: usize,
        composed: Vec<ProjectGraphOverrides>,
    ) {
        let banked: Vec<u64> = self
            .rack_banks
            .iter()
            .filter(|bank| !bank.clips.is_empty())
            .map(|bank| bank.group_id)
            .collect();
        let mut scene_side = Vec::new();
        let mut per_rack: Vec<(u64, Vec<ProjectGraphOverrides>)> =
            banked.iter().map(|gid| (*gid, Vec::new())).collect();
        for graph in composed {
            match graph.owner_rack.filter(|gid| banked.contains(gid)) {
                Some(gid) => {
                    if let Some((_, entries)) =
                        per_rack.iter_mut().find(|(owner, _)| *owner == gid)
                    {
                        entries.push(graph);
                    }
                }
                None => scene_side.push(graph),
            }
        }
        if let Some(scene) = self.scenes.get_mut(scene_idx) {
            scene.graph_overrides = scene_side;
        }
        for (group_id, entries) in per_rack {
            // Only mint a clip for a rack that actually has overrides to store;
            // an empty list under a `None` pointer stays silent.
            if entries.is_empty() && self.scene_rack_clip(scene_idx, group_id).is_none() {
                continue;
            }
            let Some(clip_id) = self.ensure_scene_rack_clip(scene_idx, group_id) else {
                continue;
            };
            if let Some(clip) = self
                .rack_bank_mut(group_id)
                .and_then(|bank| bank.clip_mut(clip_id))
            {
                clip.graph_overrides = entries;
            }
        }
    }

    /// The clip scene `scene_idx` points at for `group_id`, minting one when
    /// the pointer is `None` (§4.2).
    pub fn ensure_scene_rack_clip(
        &mut self,
        scene_idx: usize,
        group_id: u64,
    ) -> Option<RackClipId> {
        if let Some(id) = self.scene_rack_clip(scene_idx, group_id) {
            if self.rack_bank(group_id).and_then(|bank| bank.clip(id)).is_some() {
                return Some(id);
            }
        }
        let name = self
            .scenes
            .get(scene_idx)
            .map(|scene| scene.name.clone())
            .unwrap_or_else(|| "Clip".to_string());
        let id = self.create_rack_clip(group_id, &name)?;
        self.set_scene_rack_clip(scene_idx, group_id, Some(id));
        if scene_idx == self.current_scene {
            self.adopt_live_rack_clips();
        }
        Some(id)
    }

    /// Append an empty clip to a rack that already has a bank.
    pub fn create_rack_clip(&mut self, group_id: u64, name: &str) -> Option<RackClipId> {
        let bank = self.rack_bank_mut(group_id)?;
        let members = bank.members.len();
        let id = bank.mint_id();
        bank.clips.push(RackClip {
            id,
            name: name.to_string(),
            color: None,
            cells: vec![None; members],
            graph_overrides: Vec::new(),
        });
        Some(id)
    }

    /// Create the bank itself (first clip of a rack that had none). `members`
    /// is the rack's live member track list.
    pub fn create_rack_clip_with_members(
        &mut self,
        group_id: u64,
        members: &[usize],
        name: &str,
    ) -> RackClipId {
        self.bank_for(group_id, members);
        self.create_rack_clip(group_id, name)
            .expect("bank was just ensured")
    }

    pub fn rename_rack_clip(&mut self, group_id: u64, clip: RackClipId, name: &str) -> bool {
        self.rack_bank_mut(group_id)
            .and_then(|bank| bank.clip_mut(clip))
            .map(|clip| clip.name = name.to_string())
            .is_some()
    }

    /// Delete a clip and clear every scene pointer to it (those scenes fall
    /// back to `None`, silent).
    pub fn delete_rack_clip(&mut self, group_id: u64, clip: RackClipId) -> bool {
        let Some(bank) = self.rack_bank_mut(group_id) else {
            return false;
        };
        let Some(index) = bank.clips.iter().position(|c| c.id == clip) else {
            return false;
        };
        bank.clips.remove(index);
        for scene in &mut self.scenes {
            scene
                .rack_clips
                .retain(|(gid, id)| !(*gid == group_id && *id == clip));
        }
        true
    }

    pub fn reorder_rack_clip(&mut self, group_id: u64, from: usize, to: usize) -> bool {
        let Some(bank) = self.rack_bank_mut(group_id) else {
            return false;
        };
        if from >= bank.clips.len() || to >= bank.clips.len() || from == to {
            return false;
        }
        let clip = bank.clips.remove(from);
        bank.clips.insert(to, clip);
        true
    }

    /// Whether `group_id` is still a legacy rack (§4.3): no clips, so the
    /// composition step is skipped and it behaves exactly as before.
    pub fn rack_is_legacy(&self, group_id: u64) -> bool {
        self.rack_bank(group_id)
            .is_none_or(|bank| bank.clips.is_empty())
    }

    /// "Convert to clips" (§4.3), run once per legacy rack: every project scene
    /// whose member slice has any content becomes a clip named after the scene,
    /// carrying that scene's member cells and the rack's owned overrides;
    /// scenes with nothing share the `None` pointer. Returns the number of
    /// clips created, or `None` when the rack already has clips.
    pub fn convert_rack_to_clips(&mut self, group_id: u64, members: &[usize]) -> Option<usize> {
        if !self.rack_is_legacy(group_id) {
            return None;
        }
        self.bank_for(group_id, members);
        let scene_count = self.scenes.len();
        let mut created = 0usize;
        for scene_idx in 0..scene_count {
            let scene = &self.scenes[scene_idx];
            let cells: Vec<Option<PatternId>> = members
                .iter()
                .map(|track| scene.cells.get(*track).copied().flatten())
                .collect();
            let overrides: Vec<ProjectGraphOverrides> = scene
                .graph_overrides
                .iter()
                .filter(|graph| graph.owner_rack == Some(group_id))
                .cloned()
                .collect();
            if cells.iter().all(Option::is_none) && overrides.is_empty() {
                // An empty scene shares the one `None` pointer rather than
                // producing an empty clip.
                continue;
            }
            let name = scene.name.clone();
            let id = self.create_rack_clip(group_id, &name)?;
            if let Some(clip) = self
                .rack_bank_mut(group_id)
                .and_then(|bank| bank.clip_mut(id))
            {
                clip.cells = cells;
                clip.graph_overrides = overrides;
            }
            self.set_scene_rack_clip(scene_idx, group_id, Some(id));
            // The rack's slices now live in the clip: drop them from the scene
            // so exactly one place owns them.
            let scene = &mut self.scenes[scene_idx];
            for track in members {
                if let Some(cell) = scene.cells.get_mut(*track) {
                    *cell = None;
                }
            }
            scene
                .graph_overrides
                .retain(|graph| graph.owner_rack != Some(group_id));
            created += 1;
        }
        // Scenes that stayed `None` still have the rack's (empty) slices in
        // them; clear the overrides there too so nothing fires.
        for scene in &mut self.scenes {
            scene
                .graph_overrides
                .retain(|graph| graph.owner_rack != Some(group_id));
        }
        // Conversion moved the same pattern ids into clips, so the live grid
        // already holds the current scene's clip.
        self.adopt_live_rack_clips();
        Some(created)
    }

    /// Editing a member under a `None` pointer creates the clip and points the
    /// scene at it; a member with no cell in the active clip mints a pattern.
    /// Returns the pattern id the edit should land in.
    pub fn ensure_rack_clip_cell(
        &mut self,
        scene_idx: usize,
        track: usize,
        data: TrackPatternData,
    ) -> Option<PatternId> {
        let (group_id, position) = self.rack_member_slot(track)?;
        let clip_id = self.ensure_scene_rack_clip(scene_idx, group_id)?;
        if let Some(existing) = self
            .rack_bank(group_id)
            .and_then(|bank| bank.clip(clip_id))
            .and_then(|clip| clip.cells.get(position).copied())
            .flatten()
        {
            if self
                .track_pools
                .get(track)
                .is_some_and(|pool| pool.contains(existing))
            {
                return Some(existing);
            }
        }
        let id = self.track_pools.get_mut(track)?.insert(data);
        if let Some(clip) = self
            .rack_bank_mut(group_id)
            .and_then(|bank| bank.clip_mut(clip_id))
        {
            if position < clip.cells.len() {
                clip.cells[position] = Some(id);
            }
        }
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{ProjectGraphNodeIntrinsicOverride, ProjectGraphRouteOverride};

    /// Four tracks, three scenes; tracks 1 and 2 are the rack's members.
    fn scenes() -> ProjectScenes {
        let snapshots = vec![
            PatternSnapshot::new_default(4, &[]),
            PatternSnapshot::new_default(4, &[]),
            PatternSnapshot::new_default(4, &[]),
        ];
        ProjectScenes::from_pattern_snapshots(&snapshots, 0)
    }

    const MEMBERS: [usize; 2] = [1, 2];
    const RACK: u64 = 7;

    /// A concrete pattern to fill a clip cell with; the rebuilt-on-load pool
    /// shape gives every track ids 1..=3.
    fn pattern_data(scenes: &ProjectScenes, track: usize) -> TrackPatternData {
        scenes.track_pools[track].get(PatternId(1)).expect("pool pattern")
    }

    fn rack_override(sequencer_id: u64, route: usize) -> ProjectGraphOverrides {
        let mut graph = ProjectGraphOverrides::default();
        graph.sequencer_id = sequencer_id;
        graph.owner_rack = Some(RACK);
        graph.node_intrinsics = vec![ProjectGraphNodeIntrinsicOverride {
            group: "n".into(),
            instance: 0,
            resolution: None,
            delay_steps: None,
            quantize: None,
            route: Some(ProjectGraphRouteOverride::Track(route)),
            seed_from: None,
            seed_on_reset: None,
            duration: None,
            swing: None,
            neural_group: None,
        }];
        graph
    }

    #[test]
    fn a_deleted_rack_loses_its_bank_and_frees_its_former_members() {
        let mut scenes = scenes();
        let clip = scenes.create_rack_clip_with_members(RACK, &MEMBERS, "Break");
        assert!(scenes.set_scene_rack_clip(0, RACK, Some(clip)));
        assert!(scenes.rack_member_slot(1).is_some());
        // The last rack is gone: the published membership list is empty.
        scenes.sync_rack_members(&[]);
        assert!(scenes.rack_bank(RACK).is_none());
        assert!(scenes.rack_member_slot(1).is_none(), "track 1 resolves through the scene again");
        assert!(scenes.scenes[0].rack_clips.is_empty());
        assert!(scenes.live_rack_clips.is_empty());
    }

    #[test]
    fn a_rack_with_no_clips_is_legacy_and_composes_nothing() {
        let scenes = scenes();
        assert!(scenes.rack_is_legacy(RACK));
        assert!(scenes.rack_member_slot(1).is_none());
        // The scene's own cell still governs every track.
        let cell = scenes.scenes[0].cells[1];
        assert_eq!(scenes.composed_scene_cell(0, 1), cell);
    }

    #[test]
    fn a_scene_pointing_at_a_clip_plays_the_clip_and_a_none_pointer_is_silent() {
        let mut scenes = scenes();
        let clip = scenes.create_rack_clip_with_members(RACK, &MEMBERS, "Break");
        let mut data = pattern_data(&scenes, 1);
        data.track_bits[0] = 0b1011;
        let member_pattern = scenes.track_pools[1].insert(data);
        scenes
            .rack_bank_mut(RACK)
            .unwrap()
            .clip_mut(clip)
            .unwrap()
            .cells[0] = Some(member_pattern);
        scenes
            .rack_bank_mut(RACK)
            .unwrap()
            .clip_mut(clip)
            .unwrap()
            .graph_overrides = vec![rack_override(11, 1)];
        assert!(scenes.set_scene_rack_clip(0, RACK, Some(clip)));

        // Scene 0 points at the clip: the member plays the clip's pattern and
        // the rack's overrides are in the snapshot, member routes UNRESOLVED.
        let snapshot = scenes.scene_snapshot(0).expect("scene 0");
        assert_eq!(
            snapshot.track_pattern_data(1).expect("member lane").track_bits[0],
            0b1011,
            "the member plays the clip's pattern"
        );
        assert_eq!(snapshot.graph_overrides.len(), 1);
        assert_eq!(snapshot.graph_overrides[0].owner_rack, Some(RACK));
        assert_eq!(
            snapshot.graph_overrides[0].node_intrinsics[0].route,
            Some(ProjectGraphRouteOverride::Track(1)),
            "member routes stay member-relative; the scheduler resolves them"
        );

        // Scene 1 has no pointer: members are silent and NOTHING of the rack's
        // sequencers is registered.
        let snapshot = scenes.scene_snapshot(1).expect("scene 1");
        assert_eq!(
            snapshot.track_pattern_data(1).expect("member lane").track_bits[0],
            0,
            "no pointer means an empty pattern, not the last one played"
        );
        assert!(snapshot.graph_overrides.is_empty(), "the rack's sequencers do not fire");
        // A plain track is untouched by any of it.
        assert!(snapshot.track_pattern_data(0).is_some());
    }

    #[test]
    fn launching_a_scene_installs_the_clips_member_slices() {
        let mut scenes = scenes();
        let clip = scenes.create_rack_clip_with_members(RACK, &MEMBERS, "Break");
        let data = pattern_data(&scenes, 2);
        let pattern = scenes.track_pools[2].insert(data);
        scenes.rack_bank_mut(RACK).unwrap().clip_mut(clip).unwrap().cells[1] = Some(pattern);
        scenes.set_scene_rack_clip(1, RACK, Some(clip));

        let launched = scenes.launch_scene(1).expect("launch");
        assert!(launched[2].is_some(), "member 1 plays the clip's pattern");
        assert!(launched[1].is_none(), "member 0 is silent in this clip");
        assert!(launched[0].is_some(), "plain tracks are unaffected");
    }

    #[test]
    fn editing_a_member_under_a_none_pointer_creates_a_clip() {
        let mut scenes = scenes();
        // Give the rack a bank (one clip) so its members are redirected at all.
        scenes.create_rack_clip_with_members(RACK, &MEMBERS, "Break");
        assert_eq!(scenes.current_rack_clip(RACK), None);

        let mut data = pattern_data(&scenes, 1);
        data.track_bits[0] = 1;
        assert!(scenes.save_effective_track_pattern(1, data));

        let clip = scenes.current_rack_clip(RACK).expect("edit minted a clip");
        let cell = scenes
            .rack_bank(RACK)
            .unwrap()
            .clip(clip)
            .unwrap()
            .cells[0]
            .expect("member cell");
        assert_eq!(scenes.effective_pattern_id(1), Some(cell));
        assert_eq!(
            scenes.track_pools[1].get(cell).unwrap().track_bits[0],
            1,
            "the edit landed in the clip, not the scene"
        );
        let scene_cell = scenes.scenes[0].cells[1].expect("the scene still has its own cell");
        assert_ne!(scene_cell, cell, "the clip's cell is not the scene's");
        assert_eq!(
            scenes.track_pools[1].get(scene_cell).unwrap().track_bits[0],
            0,
            "the scene cell was not written"
        );
    }

    #[test]
    fn a_rack_owned_override_edit_lands_in_the_active_clip() {
        let mut scenes = scenes();
        let clip = scenes.create_rack_clip_with_members(RACK, &MEMBERS, "Break");
        scenes.set_scene_rack_clip(0, RACK, Some(clip));

        scenes
            .edit_current_graph_overrides(|graphs| {
                graphs.push(rack_override(11, 0));
                graphs.push(ProjectGraphOverrides::default());
                Ok::<_, String>(())
            })
            .expect("edit");

        assert_eq!(
            scenes.rack_bank(RACK).unwrap().clip(clip).unwrap().graph_overrides.len(),
            1,
            "the rack-owned entry went into the clip"
        );
        assert_eq!(
            scenes.scenes[0].graph_overrides.len(),
            1,
            "the project-owned entry stayed in the scene"
        );
        assert_eq!(scenes.composed_graph_overrides(0).len(), 2);
    }

    #[test]
    fn converting_a_legacy_rack_moves_every_scenes_slice_into_a_named_clip() {
        let mut scenes = scenes();
        // Scene 0 and 2 have member content; scene 1 is empty for the rack.
        for scene in [0usize, 2] {
            for track in MEMBERS {
                let data = pattern_data(&scenes, track);
                let id = scenes.track_pools[track].insert(data);
                scenes.scenes[scene].cells[track] = Some(id);
            }
        }
        scenes.scenes[1].cells[1] = None;
        scenes.scenes[1].cells[2] = None;
        scenes.scenes[0].graph_overrides = vec![rack_override(11, 0)];

        let created = scenes
            .convert_rack_to_clips(RACK, &MEMBERS)
            .expect("legacy rack converts");
        assert_eq!(created, 2, "only the scenes with content become clips");
        assert_eq!(scenes.rack_bank(RACK).unwrap().clips[0].name, "Scene 1");
        assert!(scenes.current_rack_clip(RACK).is_some());
        assert_eq!(scenes.scene_rack_clip(1, RACK), None, "empty scene stays silent");
        // The clip carries the scene's member cells and its rack-owned override.
        let clip = scenes.rack_bank(RACK).unwrap().clips[0].clone();
        assert!(clip.cells.iter().all(Option::is_some));
        assert_eq!(clip.graph_overrides.len(), 1);
        // Composition is now what scene 0 plays, and it plays the same thing.
        let snapshot = scenes.scene_snapshot(0).expect("scene 0");
        assert!(snapshot.track_pattern_data(1).is_some());
        assert_eq!(snapshot.graph_overrides.len(), 1);
        // Converting twice is refused.
        assert!(scenes.convert_rack_to_clips(RACK, &MEMBERS).is_none());
    }

    #[test]
    fn member_join_and_leave_keep_the_clip_cell_count_in_step() {
        let mut scenes = scenes();
        let clip = scenes.create_rack_clip_with_members(RACK, &MEMBERS, "Break");
        assert_eq!(scenes.rack_bank(RACK).unwrap().clip(clip).unwrap().cells.len(), 2);

        scenes.rack_bank_mut(RACK).unwrap().members = vec![1, 2, 3];
        scenes.rack_clip_member_inserted(RACK, 2);
        assert_eq!(scenes.rack_bank(RACK).unwrap().clip(clip).unwrap().cells.len(), 3);

        scenes.rack_clip_member_removed(RACK, 0);
        scenes.rack_bank_mut(RACK).unwrap().members = vec![2, 3];
        assert_eq!(scenes.rack_bank(RACK).unwrap().clip(clip).unwrap().cells.len(), 2);
    }

    #[test]
    fn deleting_a_clip_clears_every_pointer_to_it() {
        let mut scenes = scenes();
        let clip = scenes.create_rack_clip_with_members(RACK, &MEMBERS, "Break");
        scenes.set_scene_rack_clip(0, RACK, Some(clip));
        scenes.set_scene_rack_clip(2, RACK, Some(clip));
        assert!(scenes.delete_rack_clip(RACK, clip));
        assert_eq!(scenes.scene_rack_clip(0, RACK), None);
        assert_eq!(scenes.scene_rack_clip(2, RACK), None);
        assert_eq!(
            scenes.scene_snapshot(0).unwrap().track_pattern_data(1).unwrap().track_bits[0],
            0,
            "the scenes that pointed at it fall back to silence, not to their own cells"
        );
    }
}
