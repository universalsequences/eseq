//! Stage 7d-2: the provisional capture surface (`song.pending`,
//! `pending-lane`, `pending-scene`, `pending-launch`).

use super::*;
use sequencer::sequencer::PatternId;

const REFER_7D2: &str = "(import eseq.kinds :refer (track scenes song))
     (def lane-at (i) (nth song.pending-lanes i))
     (def launch-at (i) (nth song.pending-launches i))";

impl Harness {
    fn eval_7d2(&mut self, code: &str) -> Value {
        let source = format!("{REFER_7D2}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// Start an arrangement capture (no committed song: the capture covers
    /// the whole song, so its starting state is content too) and anchor the
    /// record clock at beat 0; returns the anchor instant.
    fn start_capture(&mut self) -> Instant {
        self.app.set_arrangement_view_visible(true);
        self.app.song_transport_play(true).expect("capture starts");
        assert!(self.app.pending_capture_active());
        let now = Instant::now();
        self.app.state.transport.record_clock.publish(0.0, now);
        let anchor = now + Duration::from_millis(1);
        self.app.state.transport.record_clock.publish(0.0, anchor);
        anchor
    }

    /// Record a note on `track` at `beats` past `anchor` (120 BPM).
    fn record_note(&mut self, anchor: Instant, track: usize, beats: f64, transpose: f32) {
        let press = anchor + Duration::from_secs_f64(beats * 0.5);
        assert!(self.app.take_record_note(track, press, transpose, 2.0));
    }

    fn pending_syncs(&self) -> u64 {
        self.frame.host_kinds.song.pending.syncs
    }

    fn pending_instances(&self) -> usize {
        [PENDING_LANE, PENDING_SCENE, PENDING_LAUNCH]
            .iter()
            .map(|kind| {
                (0..32)
                    .filter(|i| self.rt().keyed_instance(kind, &[*i]).is_some())
                    .count()
            })
            .sum()
    }

    /// The legacy `SEQ.song-pending`, published as the reactive tick does.
    fn legacy_pending(&mut self) -> Value {
        self.legacy_pending_in(&mut SongFrameState::default())
    }

    /// [`Self::legacy_pending`] through a frame kept across ticks (its
    /// rebuild gate in play).
    fn legacy_pending_in(&mut self, frame: &mut SongFrameState) -> Value {
        sync_song_pending(self.editor.runtime_mut(), &self.app, frame);
        (self
            .rt()
            .reactive_field_value("SEQ", "song-pending")
            .cloned())
        .unwrap_or(Value::Nil)
    }
}

#[test]
fn pending_fields_match_the_legacy_surface() {
    let mut h = Harness::new();
    h.sync();
    assert_eq!(h.eval_7d2("song.pending"), Value::Bool(false));
    let anchor = h.start_capture();
    h.record_note(anchor, 0, 4.0, 3.0);
    h.app.observe_manual_clip_launch(1, PatternId(0));
    h.sync();
    let legacy = h.legacy_pending();
    assert_eq!(h.eval_7d2("song.pending"), Value::Bool(true));
    assert_eq!(
        h.eval_7d2("song.pending-origin"),
        get(&legacy, "origin-beat")
    );
    assert_eq!(h.eval_7d2("song.pending-head"), get(&legacy, "head-beat"));
    let lanes = items(&get(&legacy, "lanes"));
    assert_eq!(lanes.len(), 1);
    assert_eq!(
        h.eval_7d2("(len song.pending-lanes)"),
        Value::Number(lanes.len() as f64)
    );
    for (i, lane) in lanes.iter().enumerate() {
        let field = |h: &mut Harness, name: &str| {
            h.eval_7d2(&format!("(let ((l (lane-at {i}))) l.{name})"))
        };
        let track = num(get(lane, "track"));
        assert_eq!(
            field(&mut h, "track"),
            h.eval_7d2(&format!("(track {track})"))
        );
        assert_eq!(field(&mut h, "index"), Value::Number(i as f64));
        assert_eq!(field(&mut h, "start"), get(lane, "start-beat"));
        assert_eq!(field(&mut h, "end"), get(lane, "end-beat"));
        assert_eq!(field(&mut h, "num-steps"), get(lane, "num-steps"));
        assert_eq!(field(&mut h, "length"), get(lane, "length-beats"));
        assert_eq!(field(&mut h, "events"), get(lane, "events"));
    }
    // The whole-song capture's starting scene, then every launch.
    let scenes = items(&get(&legacy, "scene-events"));
    assert!(!scenes.is_empty(), "the capture's starting scene");
    assert_eq!(
        h.eval_7d2("(len song.pending-scenes)"),
        Value::Number(scenes.len() as f64)
    );
    for (i, scene) in scenes.iter().enumerate() {
        let code =
            format!("(let ((s (nth song.pending-scenes {i}))) (list s.index s.start s.scene))");
        let expected = format!(
            "(list {i} {} (nth (scenes) {}))",
            num(get(scene, "start-beat")),
            num(get(scene, "scene"))
        );
        assert_eq!(h.eval_7d2(&code), h.eval_7d2(&expected));
    }
    let launches = items(&get(&legacy, "track-events"));
    assert!(!launches.is_empty(), "the clip launch at least");
    assert_eq!(
        h.eval_7d2("(len song.pending-launches)"),
        Value::Number(launches.len() as f64)
    );
    for (i, launch) in launches.iter().enumerate() {
        let field = |h: &mut Harness, name: &str| {
            h.eval_7d2(&format!("(let ((l (launch-at {i}))) l.{name})"))
        };
        let track = num(get(launch, "track"));
        assert_eq!(
            field(&mut h, "track"),
            h.eval_7d2(&format!("(track {track})"))
        );
        assert_eq!(field(&mut h, "start"), get(launch, "start-beat"));
        assert_eq!(field(&mut h, "num-steps"), get(launch, "num-steps"));
        assert_eq!(field(&mut h, "length"), get(launch, "length-beats"));
        assert_eq!(field(&mut h, "events"), get(launch, "events"));
        let pid = num(get(launch, "pattern-id"));
        let cell = format!("(let ((c (launch-at {i}))) (list c.cell.pid c.cell.track))");
        let expected = format!("(list {pid} (track {track}))");
        assert_eq!(h.eval_7d2(&cell), h.eval_7d2(&expected));
    }
}

#[test]
fn the_pending_surface_costs_nothing_without_a_capture_and_rebuilds_on_its_revision() {
    let mut h = Harness::new();
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.pending_syncs(), 0, "no capture take: no work");
    assert_eq!(h.pending_instances(), 0);
    let anchor = h.start_capture();
    h.sync();
    let syncs = h.pending_syncs();
    assert_eq!(syncs, 1);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(
        h.pending_syncs(),
        syncs,
        "an idle capture tick rebuilds nothing"
    );
    // A recorded note moves the pending revision: one rebuild.
    h.record_note(anchor, 0, 2.0, 0.0);
    h.sync();
    h.sync();
    assert_eq!(h.pending_syncs(), syncs + 1);
    assert_eq!(h.eval_7d2("(len song.pending-lanes)"), Value::Number(1.0));
}

#[test]
fn pending_instances_are_positional_and_go_when_the_capture_ends() {
    let mut h = Harness::new();
    let anchor = h.start_capture();
    h.record_note(anchor, 0, 2.0, 0.0);
    h.sync();
    let lane = h.eval_7d2("(lane-at 0)");
    let launch = h.eval_7d2("(launch-at 0)");
    // A second lane re-pushes values in place: the first handle stays.
    h.record_note(anchor, 1, 1.0, 5.0);
    h.sync();
    assert_eq!(h.eval_7d2("(len song.pending-lanes)"), Value::Number(2.0));
    assert_eq!(h.eval_7d2("(lane-at 0)"), lane);
    assert_eq!(h.eval_7d2("(launch-at 0)"), launch);
    // Cancel: every pending instance goes, and the song reads idle.
    h.app.song_capture_cancel().expect("cancel");
    h.sync();
    assert_eq!(h.eval_7d2("song.pending"), Value::Bool(false));
    assert_eq!(h.eval_7d2("(len song.pending-lanes)"), Value::Number(0.0));
    assert_eq!(h.eval_7d2("(len song.pending-scenes)"), Value::Number(0.0));
    assert_eq!(
        h.eval_7d2("(len song.pending-launches)"),
        Value::Number(0.0)
    );
    assert_eq!(h.pending_instances(), 0);
    for id in [lane, launch] {
        let Value::Instance(id) = id else {
            panic!("{id:?}")
        };
        assert!(!h.rt().instance_is_live(id));
    }
    let syncs = h.pending_syncs();
    h.sync();
    assert_eq!(h.pending_syncs(), syncs, "cleared once, then nothing");
    // A stop commits and clears it the same way.
    let anchor = h.start_capture();
    h.record_note(anchor, 0, 1.0, 0.0);
    h.sync();
    assert_eq!(h.eval_7d2("song.pending"), Value::Bool(true));
    h.app.song_transport_stop().expect("stop");
    h.sync();
    assert_eq!(h.eval_7d2("song.pending"), Value::Bool(false));
    assert_eq!(h.pending_instances(), 0);
}

impl Harness {
    /// Field `name` of `(launch-at i)`.
    fn launch_field(&mut self, i: usize, name: &str) -> Value {
        self.eval_7d2(&format!("(let ((l (launch-at {i}))) l.{name})"))
    }

    /// Move the record head to `beats` (the record clock anchored now; at
    /// 1 BPM the wall time a sync takes moves it by a hair).
    fn set_record_head(&mut self, beats: f64) {
        let transport = &self.app.state.transport;
        transport.bpm.store(1, std::sync::atomic::Ordering::Relaxed);
        transport.record_clock.publish(beats, Instant::now());
    }
}

#[test]
fn a_launched_patterns_pool_edit_mid_capture_rebuilds_its_launch() {
    let mut h = Harness::new();
    h.start_capture();
    h.sync();
    let mut legacy = SongFrameState::default();
    h.legacy_pending_in(&mut legacy);
    // The whole-song capture's starting clip on track 0.
    assert_eq!(h.launch_field(0, "track"), h.eval_7d2("(track 0)"));
    let events = h.launch_field(0, "events");
    // The reads settle.
    h.sync();
    let syncs = h.pending_syncs();
    sequencer::app::edit::try_apply_command(
        &mut h.app,
        sequencer::app::AppCommand::ToggleStep { track: 0, step: 3 },
    )
    .expect("step edit applies");
    h.sync();
    assert_eq!(h.pending_syncs(), syncs + 1, "a pool edit rebuilds");
    assert_ne!(h.launch_field(0, "events"), events, "the new step shows");
    let launch = &items(&get(&h.legacy_pending_in(&mut legacy), "track-events"))[0];
    assert_eq!(
        h.launch_field(0, "events"),
        get(launch, "events"),
        "the legacy surface rebuilds too"
    );
    // A length change: num-steps and length follow.
    let steps = num(h.launch_field(0, "num-steps")) as usize;
    let length = h.launch_field(0, "length");
    let n = if steps == 8 { 12 } else { 8 };
    sequencer::app::edit::try_apply_command(
        &mut h.app,
        sequencer::app::AppCommand::SetTrackNumSteps { track: 0, n },
    )
    .expect("length edit applies");
    h.sync();
    assert_eq!(h.launch_field(0, "num-steps"), Value::Number(n as f64));
    assert_ne!(h.launch_field(0, "length"), length);
    let launch = &items(&get(&h.legacy_pending_in(&mut legacy), "track-events"))[0];
    assert_eq!(h.launch_field(0, "length"), get(launch, "length-beats"));
}

#[test]
fn a_scene_reassignment_mid_capture_rebuilds_the_launches_it_expands() {
    let mut h = Harness::new();
    h.command("clone-pattern", Value::Nil);
    h.sync();
    h.start_capture();
    h.sync();
    let mut legacy = SongFrameState::default();
    h.legacy_pending_in(&mut legacy);
    assert_eq!(h.launch_field(0, "track"), h.eval_7d2("(track 0)"));
    let before = num(h.eval_7d2("(let ((l (launch-at 0))) l.cell.pid)")) as u64;
    let other = h
        .shared
        .state
        .track_pattern_cells(0)
        .iter()
        .map(|cell| cell.pattern_id)
        .find(|pattern| pattern.0 != before)
        .expect("a second pattern in track 0's pool");
    let scene = h.app.state.capture_project_scenes().current_scene;
    let app = &h.app;
    assert!(app.state.set_scene_cell(
        scene,
        0,
        other,
        app.tracks.len(),
        &app.graph.track_buffer_ids,
        &app.graph.track_sample_rates,
        &app.tracks,
        &app.graph.track_instrument_types,
    ));
    h.sync();
    assert_eq!(
        h.eval_7d2("(let ((l (launch-at 0))) l.cell.pid)"),
        Value::Number(other.0 as f64),
        "the starting clip names the newly assigned pattern"
    );
    let launch = &items(&get(&h.legacy_pending_in(&mut legacy), "track-events"))[0];
    assert_eq!(get(launch, "pattern-id"), Value::Number(other.0 as f64));
    assert_eq!(h.launch_field(0, "events"), get(launch, "events"));
}

#[test]
fn a_capture_started_in_the_tick_another_ended_shows_its_own_content() {
    let mut h = Harness::new();
    let anchor = h.start_capture();
    h.record_note(anchor, 0, 2.0, 0.0);
    h.sync();
    let mut legacy = SongFrameState::default();
    h.legacy_pending_in(&mut legacy);
    assert_eq!(h.eval_7d2("(len song.pending-lanes)"), Value::Number(1.0));
    // The reads settle.
    h.sync();
    // Cancel and start again before the next tick.
    h.app.song_capture_cancel().expect("cancel");
    h.start_capture();
    h.sync();
    assert_eq!(h.eval_7d2("song.pending"), Value::Bool(true));
    assert_eq!(
        h.eval_7d2("(len song.pending-lanes)"),
        Value::Number(0.0),
        "the cancelled take's lane is gone"
    );
    let legacy = h.legacy_pending_in(&mut legacy);
    assert!(items(&get(&legacy, "lanes")).is_empty());
}

#[test]
fn the_record_head_moves_only_the_head_fields_a_quantum_at_a_time() {
    let mut h = Harness::new();
    let anchor = h.start_capture();
    h.record_note(anchor, 0, 0.0, 0.0);
    h.set_record_head(2.1);
    h.sync();
    assert_eq!(h.eval_7d2("song.pending-head"), Value::Number(2.0));
    let end = h.eval_7d2("(let ((l (lane-at 0))) l.end)");
    // A view's first reads settle (the reads themselves resync the song
    // structure once); the counters start from there.
    h.sync();
    let syncs = h.pending_syncs();
    let pushes = h.frame.host_kinds.song.pending.head_pushes;
    // Inside the quantum: nothing.
    h.set_record_head(2.15);
    h.sync();
    assert_eq!(h.frame.host_kinds.song.pending.head_pushes, pushes);
    // Across a 0.25-beat boundary: the head and the lane's end, no rebuild.
    h.set_record_head(2.3);
    h.sync();
    assert_eq!(h.frame.host_kinds.song.pending.head_pushes, pushes + 1);
    assert_eq!(h.pending_syncs(), syncs, "the head rebuilds no content");
    assert_eq!(h.eval_7d2("song.pending-head"), Value::Number(2.25));
    let moved = h.eval_7d2("(let ((l (lane-at 0))) l.end)");
    assert_ne!(moved, end);
    let lane = &items(&get(&h.legacy_pending(), "lanes"))[0];
    assert_eq!(moved, get(lane, "end-beat"));
}
