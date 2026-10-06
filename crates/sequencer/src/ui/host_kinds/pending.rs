//! The provisional capture surface (spec §14, stage 7d-2): `song.pending`,
//! `pending-origin`, `pending-head` and the positional `pending-lane`,
//! `pending-scene` and `pending-launch` instances (legacy
//! `SEQ.song-pending`), while an arrangement capture take exists.
//!
//! **Identity.** Provisional content has no ids (it is inert until the
//! stop-commit), so each kind is positional (`(index)`, like `scene-span`):
//! a new note or launch re-pushes values in place, a shorter list drops the
//! tail, and the capture ending (stop, cancel or a failed commit) drops
//! every instance.
//!
//! **Feeds.** No capture take: one bool per tick (nothing at all once the
//! surface was cleared). While one exists, the content
//! (`pending_capture_content`, the legacy publisher's
//! `build_pending_content`) is rebuilt only when `pending_content_key`
//! (`App::pending_revision`, which also moves when a take begins or ends,
//! plus the pool-content and project-scenes revisions the launches' clips
//! are read from) or the arrangement structure (the track, scene and cell
//! instances it names) moved; the record head, floored to the legacy
//! quantum (`quantized_pending_head`), re-pushes `pending-head` and the
//! lanes' `end` (`PendingLaneSpan::end_beat`) when it moves. Every push is
//! compared with its cell.

use super::*;

/// The pending surface's sync state (in `SongState`).
#[derive(Default)]
pub(super) struct PendingState {
    /// (`pending_content_key`, structure generation) the content was
    /// pushed under; `None` forces a rebuild.
    content: Option<((u64, u64, u64), u64)>,
    /// The lanes last pushed, with what their ends follow the head by.
    lanes: Vec<(InstanceId, PendingLaneSpan)>,
    /// The quantized head last pushed.
    head: Option<f64>,
    /// The instances by position ([`reconcile`]).
    lane_ids: HashMap<u64, InstanceId>,
    scene_ids: HashMap<u64, InstanceId>,
    launch_ids: HashMap<u64, InstanceId>,
    /// The surface was pushed for a capture take (cleared when it ends).
    shown: bool,
    /// Content rebuilds, for tests.
    pub(super) syncs: u64,
    /// Head pushes (`pending-head` and the lanes' ends), for tests.
    pub(super) head_pushes: u64,
}

impl PendingState {
    /// The first instance of each kind (the stale check's).
    pub(super) fn representatives(&self) -> impl Iterator<Item = &InstanceId> {
        let kinds = [&self.lane_ids, &self.scene_ids, &self.launch_ids];
        kinds.into_iter().filter_map(|ids| ids.get(&0))
    }

    /// Rebuild everything at the next tick (instances are kept).
    pub(super) fn invalidate(&mut self) {
        self.content = None;
        self.head = None;
    }
}

/// Drop every instance in `known` (the capture ended).
fn drop_all(pusher: &mut Pusher<'_>, known: &mut HashMap<u64, InstanceId>) {
    for (_, id) in known.drain() {
        if pusher.rt.instance_is_live(id) {
            pusher.rt.drop_instance(id);
            pusher.changed = true;
        }
    }
}

impl HostKinds {
    /// `song.pending` and its instances (see the module docs). Runs after
    /// the song and cell syncs (launches name tracks, scenes and cells).
    pub(super) fn sync_song_pending(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let Some(song) = pusher.singleton(SONG) else {
            return;
        };
        let Some(head) = quantized_pending_head(app) else {
            if self.song.pending.shown {
                self.clear_song_pending(pusher, song);
            }
            return;
        };
        let key = (pending_content_key(app), self.song.generation());
        let rebuilt = self.song.pending.content != Some(key);
        if rebuilt {
            let content = pending_capture_content(app).unwrap_or_default();
            self.push_pending_content(pusher, song, &content);
            let pending = &mut self.song.pending;
            pending.content = Some(key);
            pending.shown = true;
            pending.syncs += 1;
        }
        let pending = &mut self.song.pending;
        if rebuilt || pending.head != Some(head) {
            pusher.push(song, f::SONG_PENDING_HEAD, number(head));
            for (id, span) in &pending.lanes {
                pusher.push(*id, f::PENDING_LANE_END, number(span.end_beat(head)));
            }
            pending.head = Some(head);
            pending.head_pushes += 1;
        }
    }

    /// Push `content`: the song's fields and every instance's (the lanes'
    /// `end` is the head's to push).
    fn push_pending_content(
        &mut self,
        pusher: &mut Pusher<'_>,
        song: InstanceId,
        content: &PendingContent,
    ) {
        let track = |track: usize| self.track_ids.get(track).copied().flatten();
        let pending = &mut self.song.pending;
        let lanes = positional(
            pusher,
            PENDING_LANE,
            &mut pending.lane_ids,
            content.lanes.len(),
        );
        pending.lanes.clear();
        for (index, (lane, id)) in content.lanes.iter().zip(&lanes).enumerate() {
            let Some(id) = id else { continue };
            pusher.push(*id, f::PENDING_LANE_INDEX, number(index as f64));
            pusher.push(
                *id,
                f::PENDING_LANE_TRACK,
                instance_or_nil(track(lane.track)),
            );
            pusher.push(*id, f::PENDING_LANE_START, number(lane.punch_in_beat));
            pusher.push(
                *id,
                f::PENDING_LANE_NUM_STEPS,
                number(lane.num_steps as f64),
            );
            pusher.push(*id, f::PENDING_LANE_LENGTH, number(lane.length_beats));
            pusher.push(
                *id,
                f::PENDING_LANE_EVENTS,
                pattern_events_value(&lane.events),
            );
            pending.lanes.push((*id, lane.span()));
        }
        let scenes = positional(
            pusher,
            PENDING_SCENE,
            &mut pending.scene_ids,
            content.scene_events.len(),
        );
        for (index, (&(start, scene), id)) in content.scene_events.iter().zip(&scenes).enumerate() {
            let Some(id) = id else { continue };
            let scene = self.scene_ids.get(scene).copied().flatten();
            pusher.push(*id, f::PENDING_SCENE_INDEX, number(index as f64));
            pusher.push(*id, f::PENDING_SCENE_SCENE, instance_or_nil(scene));
            pusher.push(*id, f::PENDING_SCENE_START, number(start));
        }
        let launches = positional(
            pusher,
            PENDING_LAUNCH,
            &mut pending.launch_ids,
            content.track_events.len(),
        );
        for (index, (event, id)) in content.track_events.iter().zip(&launches).enumerate() {
            let Some(id) = id else { continue };
            let track_id = track(event.track);
            let cell = track_id.and_then(|track_id| {
                pusher
                    .rt
                    .keyed_instance(CELL, &[track_id, event.pattern_id])
            });
            pusher.push(*id, f::PENDING_LAUNCH_INDEX, number(index as f64));
            pusher.push(*id, f::PENDING_LAUNCH_TRACK, instance_or_nil(track_id));
            pusher.push(*id, f::PENDING_LAUNCH_START, number(event.start_beat));
            pusher.push(*id, f::PENDING_LAUNCH_CELL, instance_or_nil(cell));
            pusher.push(
                *id,
                f::PENDING_LAUNCH_NUM_STEPS,
                number(event.num_steps as f64),
            );
            pusher.push(*id, f::PENDING_LAUNCH_LENGTH, number(event.length_beats));
            pusher.push(
                *id,
                f::PENDING_LAUNCH_EVENTS,
                pattern_events_value(&event.events),
            );
        }
        pusher.push(song, f::SONG_PENDING, Value::Bool(true));
        pusher.push(song, f::SONG_PENDING_ORIGIN, number(content.origin_beat));
        pusher.push(song, f::SONG_PENDING_LANES, listed_instances(&lanes));
        pusher.push(song, f::SONG_PENDING_SCENES, listed_instances(&scenes));
        pusher.push(song, f::SONG_PENDING_LAUNCHES, listed_instances(&launches));
    }

    /// The capture take is gone: drop every pending instance and clear the
    /// song's fields.
    fn clear_song_pending(&mut self, pusher: &mut Pusher<'_>, song: InstanceId) {
        let pending = &mut self.song.pending;
        drop_all(pusher, &mut pending.lane_ids);
        drop_all(pusher, &mut pending.scene_ids);
        drop_all(pusher, &mut pending.launch_ids);
        let empty = || instance_list(std::iter::empty());
        pusher.push(song, f::SONG_PENDING, Value::Bool(false));
        pusher.push(song, f::SONG_PENDING_ORIGIN, number(0.0));
        pusher.push(song, f::SONG_PENDING_HEAD, number(0.0));
        pusher.push(song, f::SONG_PENDING_LANES, empty());
        pusher.push(song, f::SONG_PENDING_SCENES, empty());
        pusher.push(song, f::SONG_PENDING_LAUNCHES, empty());
        pending.lanes.clear();
        pending.content = None;
        pending.head = None;
        pending.shown = false;
    }
}
