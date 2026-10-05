//! Drum racks (spec §14, stage 7h): the racks' `pad`s, `rack-clip`s and
//! `groove`s (with each pad's `pad-groove` share), the project's groove pool
//! (`pool-groove`) and library (`library-groove`), `track.pad`, and the
//! group's rack fields (`pads`, `clips`, `rack-clip`, `legacy`, `groove`;
//! `armed` live).
//!
//! Identity: pads are keyed (group instance id, member `TrackId`), so moving
//! a pad to another note (a swap included) or reordering members keeps the
//! instance; rack clips (group instance id, clip id), the bank's stable,
//! never-reused id; grooves (group instance id, clip id), 0 for the rack's
//! own (clip ids start at 1); a pad's share (groove instance id, member
//! `TrackId`). All are registered with their rack and dropped with it, so a
//! project load replaces them with the groups. Pool grooves are positional
//! (`(index)`), the instance kept by `GrooveId` across reorders
//! (`registry::reconcile`, replaced on a project load, like buses); library
//! grooves (index) by an id allocated per picker key while the file is
//! listed.
//!
//! Feeds, each behind its own key (none reads the history revision, so an
//! edit elsewhere re-derives none of them):
//! - The rack clips ([`ClipsKey`]): the scenes revision (every bank or
//!   pointer edit moves it), the current scene, the racks and the group and
//!   scene instances.
//! - The pads, grooves and pool ([`RackKey`]): the groups' generation (a
//!   pad-map or groove edit; moved by the model sync), the pool, the track
//!   and group instances and the rack clip instances. A groove's lanes (`slots`, `cells`, `measured`, the
//!   pad shares' lanes: `groove_lanes`, shared with `SEQ.rack-grooves`) are
//!   rebuilt only when the groove it plays or the pads moved
//!   ([`LaneKey`]), so an amount drag pushes the amounts alone.
//! - The library: re-listed (`listed_groove_library`, shared with
//!   `SEQ.groove-library`) when the UI epoch, the library generation (a
//!   library save, rename or delete moves both) or whether the project has
//!   a rack or a pool groove moved, so an amount drag lists nothing; pushed
//!   when it changed.
//! - Live: `pad.triggered` (the tick's pad lights,
//!   `read_rack_pad_trigger_flags`, shared with `SEQ.rack-pad-trigger-*`),
//!   kept in an [`ObservedList`]; `group.armed` is the group sync's
//!   (`mixer.rs`).

use super::*;
use sequencer::groove::library::library_generation;
use sequencer::groove::library::GrooveLibraryEntry;
use sequencer::groove::{pool_groove, ProjectGroove, RackGrooveSettings};
use sequencer::project::{PadRole, ProjectRackConfig, ProjectRackPad, ProjectTrackGroup};

/// What the drum rack half of the sync keeps across ticks.
#[derive(Default)]
pub(super) struct RackState {
    /// What the pads, grooves and pool were last synced under; `None`
    /// forces a sync.
    key: Option<RackKey>,
    /// What the rack clips were last synced under.
    clips_key: Option<ClipsKey>,
    /// Every rack clip instance, and a generation moved when they change
    /// (part of [`RackKey`]: a groove's `clip`, a clip's `groove`).
    clips: Vec<InstanceId>,
    clips_generation: u64,
    /// Rack and rack clip syncs (tests: an edit elsewhere runs neither).
    pub(super) syncs: u64,
    pub(super) clip_syncs: u64,
    /// Groove lane rebuilds (tests: an amount drag rebuilds none).
    pub(super) lane_builds: u64,
    /// Pool groove id → instance (dropped on a project load).
    pub(super) pool: HashMap<u64, InstanceId>,
    pool_ids: Vec<Option<InstanceId>>,
    /// Library id → instance, the ids allocated per picker key, and the
    /// listing last pushed.
    library: HashMap<u64, InstanceId>,
    library_keys: HashMap<String, u64>,
    next_library_id: u64,
    library_ids: Vec<Option<InstanceId>>,
    library_entries: Option<Vec<GrooveLibraryEntry>>,
    /// (UI epoch, library generation, has a rack or a pool groove) the
    /// library was last listed under.
    library_due: Option<(usize, u64, bool)>,
    /// Library listings (tests: an amount drag lists none).
    pub(super) library_listings: u64,
    /// Every pad instance; the observed pads per observer epoch.
    pads: Vec<InstanceId>,
    pad_observed: ObservedList,
    /// Each groove instance's lane inputs as last pushed.
    lanes: HashMap<InstanceId, LaneKey>,
}

impl RackState {
    /// Instances that stand for their kinds in the stale check (a hot
    /// reload drops a kind's instances together).
    pub(super) fn representatives(&self) -> impl Iterator<Item = &InstanceId> {
        let pool = self.pool_ids.iter().flatten().next();
        let library = self.library_ids.iter().flatten().next();
        (pool.into_iter())
            .chain(library)
            .chain(self.pads.first())
            .chain(self.clips.first())
    }

    /// Force every keyed feed at the next tick (a schema change or a hot
    /// reload dropped instances); the registered instances are kept.
    pub(super) fn invalidate(&mut self) {
        self.key = None;
        self.clips_key = None;
        self.library_entries = None;
        self.library_due = None;
        self.lanes.clear();
        self.pad_observed.reset();
    }
}

/// The inputs of the pad, groove and pool sync. The groups by the model
/// sync's generation (`HostKinds::groups_generation`); the pool, a few
/// grooves at most and edited from many places, by value.
struct RackKey {
    groups: u64,
    pool: Vec<ProjectGroove>,
    tracks: Vec<Option<InstanceId>>,
    group_ids: Vec<Option<InstanceId>>,
    clips: u64,
}

impl RackKey {
    fn matches(
        &self,
        app: &app::App,
        groups: u64,
        tracks: &[Option<InstanceId>],
        group_ids: &[Option<InstanceId>],
        clips: u64,
    ) -> bool {
        self.clips == clips
            && self.groups == groups
            && self.pool == app.grooves
            && self.tracks == tracks
            && self.group_ids == group_ids
    }
}

/// The inputs of the rack clip sync, compared without allocating.
struct ClipsKey {
    scenes_revision: u64,
    current_scene: usize,
    /// (group id, is a rack) per group position.
    racks: Vec<(u64, bool)>,
    group_ids: Vec<Option<InstanceId>>,
    scene_ids: Vec<Option<InstanceId>>,
}

impl ClipsKey {
    fn read(
        app: &app::App,
        group_ids: &[Option<InstanceId>],
        scene_ids: &[Option<InstanceId>],
    ) -> Self {
        Self {
            scenes_revision: app.state.project_scenes_revision(),
            current_scene: app.state.current_scene_index(),
            racks: app.groups.iter().map(|g| (g.id, g.is_rack())).collect(),
            group_ids: group_ids.to_vec(),
            scene_ids: scene_ids.to_vec(),
        }
    }

    fn matches(
        &self,
        app: &app::App,
        group_ids: &[Option<InstanceId>],
        scene_ids: &[Option<InstanceId>],
    ) -> bool {
        self.scenes_revision == app.state.project_scenes_revision()
            && self.current_scene == app.state.current_scene_index()
            && self.group_ids == group_ids
            && self.scene_ids == scene_ids
            && (app.groups.iter().map(|g| (g.id, g.is_rack()))).eq(self.racks.iter().copied())
    }
}

/// What a groove's lanes derive from: the pool groove it plays and the
/// rack's pads with their member track ids.
struct LaneKey {
    groove: Option<ProjectGroove>,
    pads: Vec<(u64, ProjectRackPad)>,
}

impl LaneKey {
    fn read(groove: Option<&ProjectGroove>, rows: &[PadRow<'_>]) -> Self {
        Self {
            groove: groove.cloned(),
            pads: rows.iter().map(|row| (row.tid, *row.pad)).collect(),
        }
    }

    /// Compared in place, without building a key.
    fn matches(&self, groove: Option<&ProjectGroove>, rows: &[PadRow<'_>]) -> bool {
        self.groove.as_ref() == groove
            && (self.pads.iter())
                .map(|(tid, pad)| (*tid, pad))
                .eq(rows.iter().map(|row| (row.tid, row.pad)))
    }
}

/// One rack's clip bank, read under the scenes lock.
enum RackBank {
    /// A plain group: no clips.
    NotRack,
    /// A rack without a bank: its members play the project scenes.
    Legacy,
    Clips {
        clips: Vec<RackClipRow>,
        current: Option<u64>,
    },
}

struct RackClipRow {
    cid: u64,
    name: String,
    /// The scene positions pointing at the clip.
    scenes: Vec<usize>,
}

/// One pad with its member track's position and stable id.
struct PadRow<'a> {
    pad: &'a ProjectRackPad,
    /// Its position in the pad map (the choke groups' index).
    index: usize,
    track: usize,
    tid: u64,
}

/// The rack's pads whose member resolves to a track the registry names, in
/// pad order.
fn pad_rows<'a>(
    group: &ProjectTrackGroup,
    rack: &'a ProjectRackConfig,
    tids: &[sequencer::sequencer::TrackId],
) -> Vec<PadRow<'a>> {
    let rows = rack.pads.iter().enumerate().filter_map(|(index, pad)| {
        let track = *group.members.get(pad.member)?;
        let tid = tids.get(track)?.0;
        Some(PadRow {
            pad,
            index,
            track,
            tid,
        })
    });
    rows.collect()
}

fn push_pad(
    pusher: &mut Pusher<'_>,
    id: InstanceId,
    group: InstanceId,
    track: Option<InstanceId>,
    row: &PadRow<'_>,
    choke: Option<u8>,
) {
    let pad = row.pad;
    let role = pad.effective_role();
    pusher.push(id, f::PAD_GROUP, Value::Instance(group));
    pusher.push(id, f::PAD_TRACK, instance_or_nil(track));
    pusher.push(id, f::PAD_NOTE, number(pad.pad_note));
    let label = Value::String(drum_rack_pad_label(pad.pad_note));
    pusher.push(id, f::PAD_LABEL, label);
    pusher.push(id, f::PAD_CHOKE, number(choke.unwrap_or(0)));
    pusher.push(id, f::PAD_ROLE, text(pad.role.map_or("", PadRole::key)));
    pusher.push(id, f::PAD_ROLE_TAG, text(role.map_or("", PadRole::tag)));
    pusher.push(id, f::PAD_ROLE_LABEL, text(role.map_or("", PadRole::label)));
    let standard = PadRole::standard(pad.pad_note);
    pusher.push(
        id,
        f::PAD_STANDARD_ROLE,
        text(standard.map_or("", PadRole::key)),
    );
    let standard = standard.map_or("", PadRole::label);
    pusher.push(id, f::PAD_STANDARD_ROLE_LABEL, text(standard));
}

impl HostKinds {
    /// The racks' clip banks (`rack-clip`s, `group.clips`, `rack-clip`,
    /// `legacy`), when [`ClipsKey`] moved. Runs after the model sync
    /// (groups and scenes).
    pub(super) fn sync_rack_clips(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let (group_ids, scene_ids) = (&self.group_ids, &self.scene_ids);
        let racks = &mut self.racks;
        if racks
            .clips_key
            .as_ref()
            .is_some_and(|key| key.matches(app, group_ids, scene_ids))
        {
            return;
        }
        let key = ClipsKey::read(app, group_ids, scene_ids);
        let banks: Vec<RackBank> = app.state.with_scenes(|scenes| {
            let scene_count = scenes.scenes.len();
            let bank = |group: &ProjectTrackGroup| {
                if !group.is_rack() {
                    return RackBank::NotRack;
                }
                let Some(bank) = scenes.rack_bank(group.id) else {
                    return RackBank::Legacy;
                };
                let clips = bank.clips.iter().map(|clip| RackClipRow {
                    cid: clip.id,
                    name: clip.name.clone(),
                    scenes: (0..scene_count)
                        .filter(|scene| scenes.scene_rack_clip(*scene, group.id) == Some(clip.id))
                        .collect(),
                });
                RackBank::Clips {
                    clips: clips.collect(),
                    current: scenes.current_rack_clip(group.id),
                }
            };
            app.groups.iter().map(bank).collect()
        });
        let mut all = Vec::new();
        for (group, bank) in app.groups.iter().zip(&banks) {
            let Some(group_id) = self.groups.get(&group.id).copied() else {
                continue;
            };
            let (rows, current) = match bank {
                RackBank::Clips { clips, current } => (clips.as_slice(), *current),
                RackBank::NotRack | RackBank::Legacy => (&[][..], None),
            };
            let wanted: Vec<u64> = rows.iter().map(|row| row.cid).collect();
            let ids = reconcile_children(pusher, group_id, RACK_CLIP, &wanted);
            let first = all.len();
            let mut playing = None;
            for (index, (row, id)) in rows.iter().zip(ids).enumerate() {
                let Some(id) = id else { continue };
                pusher.push(id, f::RACK_CLIP_GROUP, Value::Instance(group_id));
                pusher.push(id, f::RACK_CLIP_CID, number(row.cid as f64));
                pusher.push(id, f::RACK_CLIP_INDEX, number(index as f64));
                pusher.push(id, f::RACK_CLIP_NAME, text(&row.name));
                let active = current == Some(row.cid);
                pusher.push(id, f::RACK_CLIP_ACTIVE, Value::Bool(active));
                let scenes = row.scenes.iter();
                let scenes =
                    scenes.filter_map(|scene| self.scene_ids.get(*scene).copied().flatten());
                pusher.push(id, f::RACK_CLIP_SCENES, instance_list(scenes));
                if active {
                    playing = Some(id);
                }
                all.push(id);
            }
            let clips = instance_list(all[first..].iter().copied());
            pusher.push(group_id, f::GROUP_CLIPS, clips);
            pusher.push(group_id, f::GROUP_RACK_CLIP, instance_or_nil(playing));
            let legacy = matches!(bank, RackBank::Legacy);
            pusher.push(group_id, f::GROUP_LEGACY, Value::Bool(legacy));
        }
        let racks = &mut self.racks;
        if all != racks.clips {
            racks.clips = all;
            racks.clips_generation += 1;
        }
        racks.clip_syncs += 1;
        racks.clips_key = Some(key);
    }

    /// The pool, the racks' pads and grooves, the clips' grooves and
    /// `track.pad`, when [`RackKey`] moved. Runs after the rack clips.
    pub(super) fn sync_rack_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let (groups, generation) = (self.groups_generation, self.racks.clips_generation);
        let (tracks, group_ids) = (&self.track_ids, &self.group_ids);
        if (self.racks.key.as_ref())
            .is_some_and(|key| key.matches(app, groups, tracks, group_ids, generation))
        {
            return;
        }
        let tids = app.track_registry.ids();
        if tids.len() != app.tracks.len() {
            return; // the registry lags the track list: retry next tick
        }
        self.sync_groove_pool(pusher, app);
        let rt = &*pusher.rt;
        self.racks.lanes.retain(|id, _| rt.instance_is_live(*id));
        let mut pads = Vec::new();
        let mut pad_tracks = HashMap::new();
        let mut holders = vec![None; self.track_ids.len()];
        for group in &app.groups {
            let Some(group_id) = self.groups.get(&group.id).copied() else {
                continue;
            };
            let Some(rack) = group.rack.as_ref() else {
                // A plain group (or a rack turned back into one).
                reconcile_children(pusher, group_id, PAD, &[]);
                reconcile_children(pusher, group_id, GROOVE, &[]);
                pusher.push(group_id, f::GROUP_PADS, instance_list([]));
                pusher.push(group_id, f::GROUP_GROOVE, Value::Nil);
                continue;
            };
            let rows = pad_rows(group, rack, tids);
            let wanted: Vec<u64> = rows.iter().map(|row| row.tid).collect();
            let ids = reconcile_children(pusher, group_id, PAD, &wanted);
            let first = pads.len();
            for (row, id) in rows.iter().zip(ids) {
                let Some(id) = id else { continue };
                let track = self.track_ids.get(row.track).copied().flatten();
                push_pad(
                    pusher,
                    id,
                    group_id,
                    track,
                    row,
                    rack.choke_group(row.index),
                );
                pad_tracks.insert(id, row.track);
                if let Some(holder) = holders.get_mut(row.track) {
                    *holder = Some(id);
                }
                pads.push(id);
            }
            let list = instance_list(pads[first..].iter().copied());
            pusher.push(group_id, f::GROUP_PADS, list);
            self.sync_rack_grooves(pusher, app, group_id, rack, &rows);
        }
        for (track, id) in self.track_ids.iter().enumerate() {
            if let Some(id) = *id {
                let pad = holders.get(track).copied().flatten();
                pusher.push(id, f::TRACK_PAD, instance_or_nil(pad));
            }
        }
        pusher.shared.borrow_mut().pad_tracks = pad_tracks;
        let racks = &mut self.racks;
        if pads != racks.pads {
            racks.pads = pads;
            racks.pad_observed.reset();
        }
        racks.syncs += 1;
        racks.key = Some(RackKey {
            groups,
            pool: app.grooves.clone(),
            tracks: self.track_ids.clone(),
            group_ids: self.group_ids.clone(),
            clips: generation,
        });
    }

    /// One rack's grooves: its own (clip 0) and each clip's own, their
    /// fields, the pad shares, `group.groove` and the clips' `groove` /
    /// `own-groove`. `rows` are its pads.
    fn sync_rack_grooves(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        group_id: InstanceId,
        rack: &ProjectRackConfig,
        rows: &[PadRow<'_>],
    ) {
        let owned = rack
            .clip_grooves
            .iter()
            .map(|own| (own.clip, &own.settings));
        let settings: Vec<(u64, &RackGrooveSettings)> =
            std::iter::once((0, &rack.groove)).chain(owned).collect();
        let wanted: Vec<u64> = settings.iter().map(|(clip, _)| *clip).collect();
        let ids = reconcile_children(pusher, group_id, GROOVE, &wanted);
        let mut own = None;
        for ((clip, settings), id) in settings.iter().zip(&ids) {
            let Some(id) = *id else { continue };
            if *clip == 0 {
                own = Some(id);
            }
            let clip_id = (*clip != 0)
                .then(|| pusher.rt.keyed_instance(RACK_CLIP, &[group_id, *clip]))
                .flatten();
            self.push_groove(pusher, app, id, group_id, clip_id, settings, rack, rows);
        }
        pusher.push(group_id, f::GROUP_GROOVE, instance_or_nil(own));
        let clips: Vec<(InstanceId, u64)> = pusher
            .rt
            .keyed_children_of_kind(group_id, RACK_CLIP)
            .filter_map(|(id, key)| Some((id, *key.get(1)?)))
            .collect();
        for (clip, cid) in clips {
            let groove = wanted
                .iter()
                .zip(&ids)
                .find(|(wanted, _)| **wanted == cid)
                .and_then(|(_, id)| *id);
            pusher.push(clip, f::RACK_CLIP_GROOVE, instance_or_nil(groove));
            let owns = rack.clip_groove(cid).is_some();
            pusher.push(clip, f::RACK_CLIP_OWN_GROOVE, Value::Bool(owns));
        }
    }

    /// One groove's fields; its lanes (and the pad shares' instances and
    /// lanes) only when [`LaneKey`] moved.
    #[allow(clippy::too_many_arguments)]
    fn push_groove(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        id: InstanceId,
        group_id: InstanceId,
        clip: Option<InstanceId>,
        settings: &RackGrooveSettings,
        rack: &ProjectRackConfig,
        rows: &[PadRow<'_>],
    ) {
        let resolved = settings
            .active
            .and_then(|groove| pool_groove(&app.grooves, groove));
        let playing = resolved.and_then(|groove| self.racks.pool.get(&groove.id).copied());
        pusher.push(id, f::GROOVE_GROUP, Value::Instance(group_id));
        pusher.push(id, f::GROOVE_CLIP, instance_or_nil(clip));
        pusher.push(id, f::GROOVE_POOL_GROOVE, instance_or_nil(playing));
        pusher.push(id, f::GROOVE_ENABLED, Value::Bool(settings.enabled));
        pusher.push(id, f::GROOVE_TIMING, number(settings.timing_amount));
        pusher.push(id, f::GROOVE_VELOCITY, number(settings.velocity_amount));
        pusher.push(id, f::GROOVE_RANDOM, number(settings.random_amount));
        pusher.push(id, f::GROOVE_SCALE, number(settings.scale));
        let grid = resolved.map(|groove| scaled_grid_label(groove, settings.scale));
        pusher.push(id, f::GROOVE_GRID, Value::String(grid.unwrap_or_default()));
        let tid_of = |pad: &ProjectRackPad| {
            let row = rows.iter().find(|row| row.pad.pad_note == pad.pad_note)?;
            Some(row.tid)
        };
        let lanes_current =
            (self.racks.lanes.get(&id)).is_some_and(|key| key.matches(resolved, rows));
        if !lanes_current {
            let lanes = groove_lanes(rack, resolved);
            pusher.push(id, f::GROOVE_SLOTS, number(lanes.slots as f64));
            let (cells, measured) = lanes.all.values();
            pusher.push(id, f::GROOVE_CELLS, cells);
            pusher.push(id, f::GROOVE_MEASURED, measured);
            let shares: Vec<(u64, &GrooveLane)> = lanes
                .pads
                .iter()
                .filter_map(|(pad, lane)| Some((tid_of(pad)?, lane)))
                .collect();
            let wanted: Vec<u64> = shares.iter().map(|(tid, _)| *tid).collect();
            let ids = reconcile_children(pusher, id, PAD_GROOVE, &wanted);
            let group_pad = |rt: &Runtime, tid: u64| rt.keyed_instance(PAD, &[group_id, tid]);
            for ((tid, lane), share) in shares.iter().zip(&ids) {
                let Some(share) = *share else { continue };
                pusher.push(share, f::PAD_GROOVE_GROOVE, Value::Instance(id));
                let pad = group_pad(pusher.rt, *tid);
                pusher.push(share, f::PAD_GROOVE_PAD, instance_or_nil(pad));
                let (cells, measured) = lane.values();
                pusher.push(share, f::PAD_GROOVE_CELLS, cells);
                pusher.push(share, f::PAD_GROOVE_MEASURED, measured);
            }
            pusher.push(id, f::GROOVE_PADS, instance_list(ids.into_iter().flatten()));
            self.racks.lanes.insert(id, LaneKey::read(resolved, rows));
            self.racks.lane_builds += 1;
        }
        // The pad shares: what an Amt drag or an include dot moves.
        for row in rows {
            let Some(share) = pusher.rt.keyed_instance(PAD_GROOVE, &[id, row.tid]) else {
                continue;
            };
            let pad = settings.pad(row.pad.pad_note);
            pusher.push(share, f::PAD_GROOVE_AMOUNT, number(pad.amount));
            pusher.push(share, f::PAD_GROOVE_ENABLED, Value::Bool(pad.enabled));
        }
    }

    /// The project's groove pool (`pool-groove`, `project.groove-pool`).
    /// Non-distinct ids leave it as it was.
    fn sync_groove_pool(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let model: Vec<u64> = app.grooves.iter().map(|groove| groove.id).collect();
        if !distinct(&model) {
            return;
        }
        let pool = reconcile(pusher, POOL_GROOVE, &mut self.racks.pool, &model);
        for (index, (groove, id)) in app.grooves.iter().zip(&pool).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::POOL_GROOVE_INDEX, number(index as f64));
            pusher.push(id, f::POOL_GROOVE_ID, number(groove.id as f64));
            pusher.push(id, f::POOL_GROOVE_NAME, text(&groove.name));
            let grid = Value::String(groove_grid_label(groove));
            pusher.push(id, f::POOL_GROOVE_GRID, grid);
            let racks = app.racks_using_groove(groove.id).into_iter();
            let racks = racks.filter_map(|gid| self.groups.get(&gid).copied());
            pusher.push(id, f::POOL_GROOVE_RACKS, instance_list(racks));
        }
        if let Some(project) = pusher.singleton(PROJECT) {
            let list = instance_list(pool.iter().flatten().copied());
            pusher.push(project, f::PROJECT_GROOVE_POOL, list);
        }
        self.racks.pool_ids = pool;
    }

    /// The groove library (`library-groove`, `project.groove-library`):
    /// listed when the UI epoch, the library generation or whether there is
    /// a rack or a pool groove to show it beside moved, pushed when the
    /// listing changed.
    pub(super) fn sync_groove_library(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let racks = &mut self.racks;
        let shown = app.groups.iter().any(|group| group.rack.is_some()) || !app.grooves.is_empty();
        let epoch = pusher.sources.ui_epoch.load(Ordering::Relaxed);
        let due = (epoch, library_generation(), shown);
        if racks.library_due == Some(due) {
            return;
        }
        let Some(project) = pusher.singleton(PROJECT) else {
            return;
        };
        racks.library_due = Some(due);
        racks.library_listings += 1;
        let entries = listed_groove_library(&app.groups, &app.grooves);
        if racks.library_entries.as_ref() == Some(&entries) {
            return;
        }
        let keys: Vec<String> = entries
            .iter()
            .map(|entry| entry.choice().picker_key())
            .collect();
        racks.library_keys.retain(|key, _| keys.contains(key));
        let mut model = Vec::with_capacity(keys.len());
        let mut listed = Vec::with_capacity(keys.len());
        for (entry, key) in entries.iter().zip(keys) {
            let id = *racks.library_keys.entry(key.clone()).or_insert_with(|| {
                racks.next_library_id += 1;
                racks.next_library_id
            });
            if !model.contains(&id) {
                model.push(id);
                listed.push((entry, key));
            }
        }
        let library = reconcile(pusher, LIBRARY_GROOVE, &mut racks.library, &model);
        for (index, ((entry, key), id)) in listed.into_iter().zip(&library).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::LIBRARY_GROOVE_INDEX, number(index as f64));
            pusher.push(id, f::LIBRARY_GROOVE_CHOICE, Value::String(key));
            pusher.push(id, f::LIBRARY_GROOVE_NAME, text(&entry.name));
            pusher.push(id, f::LIBRARY_GROOVE_TIER, text(entry.tier.key()));
        }
        let list = instance_list(library.iter().flatten().copied());
        pusher.push(project, f::PROJECT_GROOVE_LIBRARY, list);
        racks.library_ids = library;
        racks.library_entries = Some(entries);
    }

    /// The observed pad fields (`triggered`), from a list kept per observer
    /// epoch ([`ObservedList`]).
    pub(super) fn sync_rack_live(&mut self, pusher: &mut Pusher<'_>) {
        let racks = &mut self.racks;
        let pads = &racks.pads;
        racks
            .pad_observed
            .refresh(pusher.rt, &PAD_LIVE.names, || pads.clone());
        racks.pad_observed.push_masked(pusher, &PAD_LIVE);
    }
}
