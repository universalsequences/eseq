//! The arrangement (spec §14, stage 7d): the `song` and `region`
//! singletons, the scene lane's `scene-span`s, the tracks' arrangement
//! `clip`s and pattern `cell`s, and `track.governed` / `track.latched`.
//!
//! Identity: clips are keyed (track instance id, clip id), the arrangement's
//! stable `ClipId`, so a move or resize keeps the instance; cells (track
//! instance id, pattern id), the pool's stable `PatternId`. Both are
//! registered with their track and dropped with it (a project load replaces
//! them with the tracks). Scene changes carry no ids, so scene spans are
//! positional (`(index)`), like steps: an edit re-pushes their values.
//!
//! Feeds, each behind its own key (none reads the history revision, so a
//! knob drag re-derives none of them):
//! - Structure (the clips with their fields, the scene spans, the song's
//!   `exists`, `end` and `loop`): the committed-song revision, the track and
//!   scene instances (a reorder remaps the lanes in place, moving no song
//!   revision) and the cell set ([`SongState::structure_syncs`] counts).
//! - Source content (`num-steps`, `length`, `events`, from
//!   `collect_lane_pattern_events`, shared with `SEQ.song-lane-events`): the
//!   pattern epoch and pool content revision. Values are cached per (track
//!   instance, source); after a structure-only change only clips whose
//!   (track, source) is new are pushed, and only missing sources' lanes are
//!   previewed.
//! - Sound dots (`dot`, `dot-color`, `App::song_clip_sounds_in`): the song
//!   revision, the scenes revision (the patches and their palette colors
//!   live there), the sound binding epoch and the structure.
//! - Cells (`SequencerState::tracks_pattern_cells`, one scenes lock, shared
//!   with the legacy `track-pattern-cell-*` fields): the scenes revision,
//!   current scene, pattern epoch, song-row mirror epoch, track generation,
//!   take-lane and silenced masks and the track and bank instances.
//! - The song's mode, cursor, errors, bound clip and region are `App` state
//!   no counter tracks: compared every tick with what was last pushed
//!   ([`SongPushed`], reset when the singletons are re-registered),
//!   allocating only on a change.
//! - `track.governed` (`song_take_lane_states`): [`GovernKey`].
//! - Live (`song.position`, `manual-latch`, `scene-latched`,
//!   `track.latched`, `cell.queued`, `cell.selected`): shared sequencer
//!   state while observed; the cells' are kept in an [`ObservedList`].

use super::*;
use sequencer::app::song_region::SongRegionSelection;
use sequencer::sequencer::{arrangement_scene_spans, ArrClip, SceneSpan};

/// What the arrangement half of the sync keeps across ticks.
#[derive(Default)]
pub(super) struct SongState {
    /// What the structure was last synced under; `None` forces a sync.
    structure: Option<StructureKey>,
    /// Counts structure syncs (tests: a knob drag rebuilds no clips).
    pub(super) structure_syncs: u64,
    /// Moves with every structure sync (the clip instances may have
    /// changed): part of [`GovernKey`], the dots' key and the region's.
    generation: u64,
    /// (pattern epoch, pool content revision) the source cache holds.
    sources_key: Option<(u64, u64)>,
    /// Built source values per (track instance, source).
    sources: HashMap<(InstanceId, ClipSource), [Value; 3]>,
    /// (song revision, scenes revision, sound binding epoch, generation)
    /// the dots were last pushed under.
    dots_key: Option<(u64, u64, usize, u64)>,
    /// Every clip instance with its lane and source, in lane order.
    clips: Vec<ClipRow>,
    /// The scene span instances by position ([`registry::reconcile`]).
    spans: HashMap<u64, InstanceId>,
    /// What the cells were last synced under.
    cells_key: Option<CellKey>,
    /// Moves when the cell set changes: part of [`StructureKey`] (a clip's
    /// `cell`).
    cells_generation: u64,
    /// Every cell instance, and the observed ones per observer epoch.
    cells: Vec<InstanceId>,
    cell_observed: ObservedList,
    pushed: SongPushed,
    governed: Option<GovernKey>,
}

impl SongState {
    /// Instances that stand for their kinds in the stale check (a hot
    /// reload drops a kind's instances together): the spans, the first clip
    /// and the first cell.
    pub(super) fn representatives(&self) -> impl Iterator<Item = &InstanceId> {
        let clip = self.clips.first().map(|row| &row.id);
        self.spans.values().chain(clip).chain(self.cells.first())
    }

    /// Moves with every structure sync (the clip instances may have
    /// changed).
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// Force every keyed feed at the next tick (a schema change or a hot
    /// reload dropped instances); the registered instances are kept.
    pub(super) fn invalidate(&mut self) {
        self.structure = None;
        self.sources_key = None;
        self.sources.clear();
        self.dots_key = None;
        self.cells_key = None;
        self.cell_observed.reset();
        self.pushed = SongPushed::default();
        self.governed = None;
    }
}

/// The inputs of the structure sync.
#[derive(Clone, PartialEq)]
struct StructureKey {
    revision: u64,
    tracks: Vec<Option<InstanceId>>,
    scenes: Vec<Option<InstanceId>>,
    cells: u64,
}

/// The inputs of the cell model sync, compared each tick without
/// allocating ([`CellKey::matches`]).
#[derive(Clone, PartialEq)]
struct CellKey {
    scenes_revision: u64,
    current_scene: usize,
    pattern_epoch: u64,
    song_row_mirror_epoch: u64,
    track_generation: u64,
    take_lanes: u64,
    silenced: Vec<bool>,
    tracks: Vec<Option<InstanceId>>,
    banks: Vec<Option<InstanceId>>,
}

impl CellKey {
    /// The key now: the counters and masks, the silenced flags of `count`
    /// tracks and the track and bank instances.
    fn read(app: &app::App, tracks: &[Option<InstanceId>], banks: &[Option<InstanceId>]) -> Self {
        let state = &app.state;
        Self {
            scenes_revision: state.project_scenes_revision(),
            current_scene: state.current_scene_index(),
            pattern_epoch: state.transport.pattern_epoch.load(Ordering::Relaxed),
            song_row_mirror_epoch: app.song_row_mirror_epoch,
            track_generation: app.track_registry.generation(),
            take_lanes: state.song_take_lane_mask(),
            silenced: (0..tracks.len())
                .map(|track| state.is_scene_silenced(track))
                .collect(),
            tracks: tracks.to_vec(),
            banks: banks.to_vec(),
        }
    }

    /// Whether the key now is still `self`.
    fn matches(
        &self,
        app: &app::App,
        tracks: &[Option<InstanceId>],
        banks: &[Option<InstanceId>],
    ) -> bool {
        let state = &app.state;
        self.scenes_revision == state.project_scenes_revision()
            && self.current_scene == state.current_scene_index()
            && self.pattern_epoch == state.transport.pattern_epoch.load(Ordering::Relaxed)
            && self.song_row_mirror_epoch == app.song_row_mirror_epoch
            && self.track_generation == app.track_registry.generation()
            && self.take_lanes == state.song_take_lane_mask()
            && self.tracks == tracks
            && self.banks == banks
            && (0..tracks.len())
                .map(|track| state.is_scene_silenced(track))
                .eq(self.silenced.iter().copied())
    }
}

/// One registered clip: its lane (track position), track instance, clip id
/// and source.
#[derive(Clone, Copy)]
struct ClipRow {
    id: InstanceId,
    track: usize,
    track_id: InstanceId,
    cid: u64,
    source: ClipSource,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ClipSource {
    Pattern(u64),
    Take(u64),
}

impl ClipSource {
    fn of(clip: &ArrClip) -> Self {
        match clip.take_id {
            Some(take) => Self::Take(take),
            None => Self::Pattern(clip.pattern_id.unwrap_or(0)),
        }
    }

    fn previews(&self, entry: &LanePatternEvents) -> bool {
        match *self {
            Self::Take(take) => entry.take_id == Some(take),
            Self::Pattern(pattern) => entry.take_id.is_none() && entry.pattern_id == pattern,
        }
    }
}

/// The song fields last pushed from `App` state that no counter tracks,
/// for the singleton instances in `singletons`.
#[derive(Default)]
struct SongPushed {
    singletons: Option<(InstanceId, Option<InstanceId>)>,
    mode: Option<&'static str>,
    recording_kind: Option<&'static str>,
    cursor: Option<f64>,
    edit_error: Option<Option<String>>,
    capture: Option<(bool, Option<String>)>,
    /// The region, under the structure generation (its tracks are track
    /// positions).
    region: Option<(Option<SongRegionSelection>, u64)>,
}

/// What `song_take_lane_states` reads: song playback authority, the
/// mirrored row, the latch mask, the running song, and the structure
/// generation (the track instances, and the take claims a recorded edit
/// moves).
#[derive(Clone, Copy, PartialEq)]
struct GovernKey {
    authority: bool,
    row: Option<usize>,
    latch: u64,
    song: usize,
    generation: u64,
}

impl HostKinds {
    /// The tracks' pattern cells (`t.cells`): registered per track by
    /// pattern id, their model fields (`active`, `assigned`, `override`, the
    /// banks using them), when [`CellKey`] moved. Runs after the model sync
    /// (tracks and banks).
    pub(super) fn sync_cell_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let (tracks, banks) = (&self.track_ids, &self.bank_ids);
        if let Some(key) = &self.song.cells_key {
            if key.matches(app, tracks, banks) {
                return;
            }
        }
        let key = CellKey::read(app, tracks, banks);
        let tracks = app.state.tracks_pattern_cells(self.track_ids.len());
        let mut all = Vec::new();
        for (track, (cells, memberships)) in tracks.iter().enumerate() {
            let Some(track_id) = self.track_ids[track] else {
                continue;
            };
            let wanted: Vec<u64> = cells.iter().map(|cell| cell.pattern_id.0).collect();
            let ids = pusher.reconcile_children(track_id, CELL, &wanted);
            let first = all.len();
            for (cell, id) in cells.iter().zip(ids) {
                let Some(id) = id else { continue };
                pusher.push(id, f::CELL_TRACK, Value::Instance(track_id));
                pusher.push(id, f::CELL_PID, number(cell.pattern_id.0 as f64));
                pusher.push(id, f::CELL_ACTIVE, Value::Bool(cell.active_effective));
                let assigned = Value::Bool(cell.assigned_to_current_scene);
                pusher.push(id, f::CELL_ASSIGNED, assigned);
                pusher.push(id, f::CELL_OVERRIDE, Value::Bool(cell.overridden));
                let banks = memberships.get(&cell.pattern_id).into_iter().flatten();
                let banks = banks.filter_map(|bank| self.bank_ids.get(*bank).copied().flatten());
                pusher.push(id, f::CELL_BANKS, instance_list(banks));
                all.push(id);
            }
            let list = instance_list(all[first..].iter().copied());
            pusher.push(track_id, f::TRACK_CELLS, list);
        }
        if all != self.song.cells {
            self.song.cell_observed.reset();
            self.song.cells = all;
            self.song.cells_generation += 1;
        }
        self.song.cells_key = Some(key);
    }

    /// The observed cell fields (`queued`, `selected`), from a list kept per
    /// observer epoch ([`ObservedList`]).
    pub(super) fn sync_cell_live(&mut self, pusher: &mut Pusher<'_>) {
        let cells = &self.song.cells;
        let observed = &mut self.song.cell_observed;
        observed.refresh(pusher.rt, &CELL_LIVE.names, || cells.clone());
        observed.push_masked(pusher, &CELL_LIVE);
    }

    /// The committed song's half: the structure, the clips' sound dots and
    /// their source content, each when its key moved (see the module docs).
    pub(super) fn sync_song_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let state = &app.state;
        let revision = state.committed_song_revision();
        let structure_due = self.song.structure.as_ref().is_none_or(|key| {
            key.revision != revision
                || key.tracks != self.track_ids
                || key.scenes != self.scene_ids
                || key.cells != self.song.cells_generation
        });
        let epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
        let content = (epoch, state.pool_content_revision());
        let sound = (
            revision,
            state.project_scenes_revision(),
            app.sound_binding_epoch,
        );
        let dots_due =
            |song: &SongState| song.dots_key != Some((sound.0, sound.1, sound.2, song.generation));
        if !structure_due && self.song.sources_key == Some(content) && !dots_due(&self.song) {
            return;
        }
        let (lanes, spans) = state.with_committed_arrangement(|arrangement| match arrangement {
            Some(arrangement) => (
                arrangement.track_lanes.clone(),
                structure_due.then(|| arrangement_scene_spans(arrangement)),
            ),
            None => (Vec::new(), Some(Vec::new())),
        });
        let mut fresh = Vec::new();
        if let Some(spans) = spans {
            fresh = self.sync_song_structure(pusher, app, &lanes, &spans);
            self.song.structure = Some(StructureKey {
                revision,
                tracks: self.track_ids.clone(),
                scenes: self.scene_ids.clone(),
                cells: self.song.cells_generation,
            });
            self.song.structure_syncs += 1;
            self.song.generation += 1;
        }
        if dots_due(&self.song) {
            self.sync_clip_dots(pusher, app, &lanes);
            self.song.dots_key = Some((sound.0, sound.1, sound.2, self.song.generation));
        }
        self.sync_clip_sources(pusher, app, &lanes, content, &fresh);
    }

    /// The song's `exists`, `end`, `loop` and `spans`, and every track's
    /// clips with their structural fields. Returns the rows whose (track,
    /// source) is new: a new instance, or a moved or re-sourced clip.
    fn sync_song_structure(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        lanes: &[Vec<ArrClip>],
        spans: &[SceneSpan],
    ) -> Vec<usize> {
        if let Some(song) = pusher.singleton(SONG) {
            let (exists, end, looping) = app.state.with_committed_song(|committed| {
                (
                    committed.is_some(),
                    committed.map_or(0.0, |song| song.end_beat),
                    committed.is_some_and(|song| song.loop_enabled),
                )
            });
            pusher.push(song, f::SONG_EXISTS, Value::Bool(exists));
            pusher.push(song, f::SONG_END, number(end));
            pusher.push(song, f::SONG_LOOP, Value::Bool(looping));
            let spans = self.sync_spans(pusher, spans);
            pusher.push(song, f::SONG_SPANS, instance_list(spans));
        }
        let before: HashMap<InstanceId, (InstanceId, ClipSource)> = self
            .song
            .clips
            .iter()
            .map(|row| (row.id, (row.track_id, row.source)))
            .collect();
        let mut rows = Vec::new();
        let mut fresh = Vec::new();
        for (track, id) in self.track_ids.iter().enumerate() {
            let Some(track_id) = *id else { continue };
            let clips = lanes.get(track).map_or(&[][..], Vec::as_slice);
            let wanted: Vec<u64> = clips.iter().map(|clip| clip.id.0).collect();
            let ids = pusher.reconcile_children(track_id, CLIP, &wanted);
            let first = rows.len();
            for (clip, id) in clips.iter().zip(ids) {
                let Some(id) = id else { continue };
                pusher.push(id, f::CLIP_TRACK, Value::Instance(track_id));
                pusher.push(id, f::CLIP_CID, number(clip.id.0 as f64));
                pusher.push(id, f::CLIP_START, number(clip.start_beat));
                pusher.push(id, f::CLIP_END, number(clip.end_beat));
                let cell = clip
                    .pattern_id
                    .and_then(|pattern| pusher.rt.keyed_instance(CELL, &[track_id, pattern]));
                pusher.push(id, f::CLIP_CELL, instance_or_nil(cell));
                let take = clip.take_id.map_or(-1.0, |take| take as f64);
                pusher.push(id, f::CLIP_TAKE, number(take));
                pusher.push(id, f::CLIP_OFFSET, number(clip.offset_steps));
                let source = ClipSource::of(clip);
                if before.get(&id) != Some(&(track_id, source)) {
                    fresh.push(rows.len());
                }
                let cid = clip.id.0;
                rows.push(ClipRow {
                    id,
                    track,
                    track_id,
                    cid,
                    source,
                });
            }
            let list = instance_list(rows[first..].iter().map(|row| row.id));
            pusher.push(track_id, f::TRACK_CLIPS, list);
        }
        self.song.clips = rows;
        fresh
    }

    /// The scene span instances, positional (`reconcile` over the
    /// positions, so a shorter lane drops the tail and a hot reload's
    /// dropped instances are re-registered), their fields pushed. Returns
    /// them in order.
    fn sync_spans(&mut self, pusher: &mut Pusher<'_>, spans: &[SceneSpan]) -> Vec<InstanceId> {
        let positions: Vec<u64> = (0..spans.len() as u64).collect();
        let ids = reconcile(pusher, SCENE_SPAN, &mut self.song.spans, &positions);
        let mut list = Vec::with_capacity(spans.len());
        for (index, (span, id)) in spans.iter().zip(ids).enumerate() {
            let Some(id) = id else { continue };
            pusher.push(id, f::SCENE_SPAN_INDEX, number(index as f64));
            let scene = self.scene_ids.get(span.scene).copied().flatten();
            pusher.push(id, f::SCENE_SPAN_SCENE, instance_or_nil(scene));
            pusher.push(id, f::SCENE_SPAN_START, number(span.start_beat));
            pusher.push(id, f::SCENE_SPAN_END, number(span.end_beat));
            list.push(id);
        }
        list
    }

    /// Each clip's sound dot (`dot`, `dot-color`; the timeline's gray for a
    /// patch without a palette color), from `App::song_clip_sounds_in`
    /// (shared with `SEQ.song-clip-sounds`) over the lanes in hand.
    fn sync_clip_dots(&mut self, pusher: &mut Pusher<'_>, app: &app::App, lanes: &[Vec<ArrClip>]) {
        if self.song.clips.is_empty() {
            return;
        }
        let sounds: HashMap<u64, (bool, Option<u8>)> = app
            .song_clip_sounds_in(lanes)
            .into_iter()
            .flatten()
            .map(|(clip, dot, color)| (clip, (dot, color)))
            .collect();
        let gray = eseqlisp::widget_render::timeline::SOUND_DOT_GRAY;
        for row in &self.song.clips {
            let (dot, color) = sounds.get(&row.cid).copied().unwrap_or((false, None));
            let rgb = sound_palette_rgb(color).map_or([gray.r, gray.g, gray.b], |(_, rgb)| rgb);
            pusher.push(row.id, f::CLIP_DOT, Value::Bool(dot));
            pusher.push(row.id, f::CLIP_DOT_COLOR, rgb3(rgb));
        }
    }

    /// Each clip's source content (`num-steps`, `length`, `events`): every
    /// row when the content key moved (the cache is cleared), else the
    /// `fresh` rows only. Values come from the cache, or from the previews
    /// (`collect_lane_pattern_events`, shared with `SEQ.song-lane-events`)
    /// of the lanes holding a missing source, each source built once.
    fn sync_clip_sources(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        lanes: &[Vec<ArrClip>],
        content: (u64, u64),
        fresh: &[usize],
    ) {
        let song = &mut self.song;
        let all = song.sources_key != Some(content);
        if all {
            song.sources.clear();
            song.sources_key = Some(content);
        } else if !fresh.is_empty() {
            // Drop the sources no clip plays any more.
            let live: HashSet<(InstanceId, ClipSource)> = song
                .clips
                .iter()
                .map(|row| (row.track_id, row.source))
                .collect();
            song.sources.retain(|key, _| live.contains(key));
        }
        let rows: Vec<ClipRow> = if all {
            song.clips.clone()
        } else {
            fresh.iter().map(|&row| song.clips[row]).collect()
        };
        let missing: Vec<&ClipRow> = rows
            .iter()
            .filter(|row| !song.sources.contains_key(&(row.track_id, row.source)))
            .collect();
        if !missing.is_empty() {
            let mut needed = vec![Vec::new(); lanes.len()];
            for row in &missing {
                if needed[row.track].is_empty() {
                    needed[row.track].clone_from(&lanes[row.track]);
                }
            }
            let previews = app
                .state
                .with_project_scenes(|scenes| collect_lane_pattern_events(&needed, scenes));
            for row in missing {
                let entries = previews.get(row.track).into_iter().flatten();
                let values = match entries.into_iter().find(|entry| row.source.previews(entry)) {
                    Some(entry) => [
                        number(entry.num_steps as f64),
                        number(entry.length_beats),
                        pattern_events_value(&entry.events),
                    ],
                    None => [number(0.0), number(0.0), list_value(Vec::<Value>::new())],
                };
                song.sources.insert((row.track_id, row.source), values);
            }
        }
        for row in &rows {
            let [steps, length, events] = song.sources[&(row.track_id, row.source)].clone();
            pusher.push(row.id, f::CLIP_NUM_STEPS, steps);
            pusher.push(row.id, f::CLIP_LENGTH, length);
            pusher.push(row.id, f::CLIP_EVENTS, events);
        }
    }

    /// The song fields the `App` holds without a counter (mode, recording
    /// kind, cursor, edit error, capture state, region, bound clip),
    /// compared every tick with what was last pushed; all re-pushed when the
    /// song or region singleton is a new instance (a project load or schema
    /// reload).
    pub(super) fn sync_song_pushed(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let Some(song) = pusher.singleton(SONG) else {
            return;
        };
        let region_id = pusher.singleton(REGION);
        let pushed = &mut self.song.pushed;
        if pushed.singletons != Some((song, region_id)) {
            *pushed = SongPushed {
                singletons: Some((song, region_id)),
                ..SongPushed::default()
            };
        }
        let mode = app.song_transport_mode.binding_str();
        if pushed.mode != Some(mode) {
            pusher.push(song, f::SONG_MODE, text(mode));
            pushed.mode = Some(mode);
        }
        let kind = song_recording_kind_label(app.recording_kind);
        if pushed.recording_kind != Some(kind) {
            pusher.push(song, f::SONG_RECORDING_KIND, text(kind));
            pushed.recording_kind = Some(kind);
        }
        let cursor = app.arrangement_cursor_beat;
        if pushed.cursor != Some(cursor) {
            pusher.push(song, f::SONG_CURSOR, number(cursor));
            pushed.cursor = Some(cursor);
        }
        let error = app.song_edit_error.as_deref();
        if pushed.edit_error.as_ref().map(Option::as_deref) != Some(error) {
            pusher.push(song, f::SONG_EDIT_ERROR, text(error.unwrap_or_default()));
            pushed.edit_error = Some(app.song_edit_error.clone());
        }
        let failed = app.song_capture_failed;
        let capture_error = app.song_capture_error.as_deref();
        let capture = pushed.capture.as_ref();
        if capture.map(|(failed, error)| (*failed, error.as_deref()))
            != Some((failed, capture_error))
        {
            pusher.push(song, f::SONG_CAPTURE_FAILED, Value::Bool(failed));
            let message = text(capture_error.unwrap_or_default());
            pusher.push(song, f::SONG_CAPTURE_ERROR, message);
            pushed.capture = Some((failed, app.song_capture_error.clone()));
        }
        let region = (app.song_region_selection, self.song.generation);
        if pushed.region != Some(region) {
            if let Some(id) = region_id {
                // Cleared, the singleton reads as no tracks over 0..0.
                let (tracks, start, end, scene_lane) = match region.0 {
                    Some(region) => {
                        let (first, last) = (region.track_a, region.track_b);
                        let tracks = first.min(last)..=first.max(last);
                        let tracks = tracks.filter_map(|track| self.track_ids.get(track).copied());
                        let tracks = instance_list(tracks.flatten());
                        (
                            tracks,
                            region.start_beat,
                            region.end_beat,
                            region.scene_lane,
                        )
                    }
                    None => (instance_list(Vec::new()), 0.0, 0.0, false),
                };
                pusher.push(id, f::REGION_TRACKS, tracks);
                pusher.push(id, f::REGION_START, number(start));
                pusher.push(id, f::REGION_END, number(end));
                pusher.push(id, f::REGION_SCENE_LANE, Value::Bool(scene_lane));
            }
            let selected = region_id.filter(|_| region.0.is_some());
            pusher.push(song, f::SONG_REGION, instance_or_nil(selected));
            pushed.region = Some(region);
        }
        // The bound clip: one keyed lookup, compared with the cell.
        let bound = app.song_clip_selection.and_then(|selection| {
            let track = self.track_ids.get(selection.track).copied().flatten()?;
            pusher
                .rt
                .keyed_instance(CLIP, &[track, selection.clip_id.0])
        });
        pusher.push(song, f::SONG_BOUND_CLIP, instance_or_nil(bound));
    }

    /// `track.governed` (`song_take_lane_states`, shared with
    /// `SEQ.song-track-governed`), re-derived only when its inputs moved.
    pub(super) fn sync_governed(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let key = GovernKey {
            authority: app.song_playback_authority_active(),
            row: app.song_mirrored_row,
            latch: app.state.song_manual_latch_mask(),
            song: app
                .active_runtime_song
                .as_ref()
                .map_or(0, |song| Arc::as_ptr(song) as usize),
            generation: self.song.generation,
        };
        if self.song.governed == Some(key) {
            return;
        }
        let states = song_take_lane_states(app);
        for (track, id) in self.track_ids.iter().enumerate() {
            if let Some(id) = *id {
                let state = states.get(track).copied().unwrap_or(0);
                pusher.push(id, f::TRACK_GOVERNED, number(state));
            }
        }
        self.song.governed = Some(key);
    }
}
