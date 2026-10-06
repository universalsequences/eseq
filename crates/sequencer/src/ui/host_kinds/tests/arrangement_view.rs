//! The factory arrangement view, ported to the kinds (kind-bindings spec §13
//! stage 8, eseq-0l17.15).

use super::views::{assert_ported, distro, instance_bindings, widgets_with_prop};
use super::*;
use sequencer::sequencer::{ClipId, LaneSource, PatternId};

const PORTED: [(&str, &str); 1] = [(
    "ui/arrangement.lisp",
    include_str!("../../../../../../content/ui/arrangement.lisp"),
)];

impl Harness {
    /// Field `field` of the view's singleton `single` (`eseq.arrangement/…`).
    fn arr_field(&mut self, single: &str, field: &str) -> Value {
        self.eval(&format!("(let ((v eseq.arrangement/{single})) v.{field})"))
    }

    /// A title-bar click on clip `clip` of track position `track`.
    fn click_clip(&mut self, track: usize, clip: ClipId) {
        self.eval(&format!(
            "(eseq.arrangement/track-action {track} (dict :type :select :ids (list {}) :time 5))",
            clip.0
        ));
        self.settle();
    }

    /// The lanes track position `track` lights as the region.
    fn lit(&mut self, track: usize) -> bool {
        self.eval(&format!("(eseq.arrangement/lane-region-rect {track})")) != Value::Nil
    }

    /// Apply what the view queued, push the kinds and re-render.
    fn settle(&mut self) {
        self.drain();
        self.sync();
        self.show_all();
    }

    /// The arrangement's track lane timeline for track position `track`.
    fn track_lane(&self, track: usize) -> HashMap<String, Value> {
        let mut lanes = Vec::new();
        widgets_with_prop(&self.buffer_tree("*arrangement*").0, "lane-key", &mut lanes);
        lanes
            .into_iter()
            .find(|lane| lane.get("lane-key") == Some(&Value::Number(track as f64)))
            .unwrap_or_else(|| panic!("track lane {track}"))
    }
}

#[test]
fn ported_arrangement_view_uses_no_legacy_binding_forms() {
    assert_ported(&PORTED);
}

/// The lanes bind the song, the view state and the per-lane channels with
/// `#'`; every track lane binds the same fields and names its own lane.
#[test]
fn the_arrangement_binds_its_state_through_kinds() {
    let mut h = distro();
    let clip = h
        .app
        .arr_clip_create(0, 0.0, 4.0, LaneSource::Pattern(PatternId(1)), 0.0)
        .expect("clip");
    h.settle();
    let (tree, _) = h.buffer_tree("*arrangement*");
    let mut bound = Vec::new();
    let mut legacy = Vec::new();
    instance_bindings(&tree, &mut bound, &mut legacy);
    // (The track headers are the sequencer's, not yet ported.)
    let removed = [
        "SEQ.song-position-beats",
        "SEQ.song-region",
        "SEQ.song-bound-clip",
    ];
    assert!(
        legacy
            .iter()
            .all(|field| !field.starts_with("SEQV.arr-") && !removed.contains(&field.as_str())),
        "no legacy arrangement binding is left: {legacy:?}"
    );
    let song = h.singleton("eseq.kinds:song");
    assert!(
        bound.contains(&(song, "position".to_string())),
        "the playhead"
    );
    let view = |h: &Harness, single: &str| h.singleton(&format!("eseq.arrangement:{single}"));
    let expected = [
        (
            "arr-view",
            [
                "start",
                "duration",
                "content-length",
                "cursor-time",
                "cursor-track",
            ]
            .as_slice(),
        ),
        ("arr-select", &["lane", "clip"]),
        (
            "arr-lanes",
            &[
                "bound-lane",
                "bound-clip",
                "region-on",
                "region-lane-a",
                "region-a",
            ],
        ),
        ("arr-drag", &["ghost", "clip", "time", "lane-a", "lane-b"]),
    ];
    for (single, fields) in expected {
        let id = view(&h, single);
        for field in fields {
            assert!(
                bound.contains(&(id, field.to_string())),
                "{single}.{field} is bound: {bound:?}"
            );
        }
    }
    let lane = h.track_lane(0);
    let lane_items = items(lane.get("items").unwrap_or(&Value::Nil));
    assert_eq!(lane_items.len(), 1, "the clip is the lane's one item");
    assert!(matches!(
        lane.get("bound-lane"),
        Some(Value::ReactiveRef { .. })
    ));
    // The cid names the clip on the view's lane.
    let cid = h.eval("(let ((c (first (eseq.arrangement/track-clips 0)))) c.cid)");
    assert_eq!(cid, Value::Number(clip.0 as f64));
}

/// A title-bar click binds the clip and selects its span (`set!
/// song.bound-clip`); the host's selection reaches the lanes' bound fields,
/// lighting only the clicked lane.
#[test]
fn a_clip_click_binds_the_clip_and_lights_only_its_lane() {
    let mut h = distro();
    let clip = h
        .app
        .arr_clip_create(1, 4.0, 8.0, LaneSource::Pattern(PatternId(1)), 0.0)
        .expect("clip");
    h.settle();
    h.click_clip(1, clip);
    assert_eq!(
        h.app.song_clip_selection.map(|selection| selection.clip_id),
        Some(clip),
        "the click binds the clip"
    );
    assert_eq!(h.arr_field("arr-lanes", "bound-lane"), Value::Number(1.0));
    assert_eq!(
        h.arr_field("arr-lanes", "bound-clip"),
        Value::Number(clip.0 as f64)
    );
    assert_eq!(h.arr_field("arr-lanes", "region-on"), Value::Bool(true));
    assert!(h.lit(1), "the clip's span is the region");
    assert!(!h.lit(0), "on its own lane only");
    assert_eq!(
        h.eval("(len (eseq.arrangement/lane-selection 1))"),
        Value::Number(1.0)
    );
    assert_eq!(
        h.eval("(len (eseq.arrangement/lane-selection 0))"),
        Value::Number(0.0)
    );
    // A scene-lane click releases the binding and the region.
    h.eval("(eseq.arrangement/scene-action (dict :type :select :ids (list) :time 2))");
    h.settle();
    assert_eq!(h.app.song_clip_selection, None);
    assert_eq!(h.app.song_region_selection, None);
    assert_eq!(h.arr_field("arr-lanes", "bound-lane"), Value::Number(-1.0));
    assert_eq!(h.arr_field("arr-lanes", "region-on"), Value::Bool(false));
}

/// A click on the clip that is already bound still selects its span: the
/// binding can come from a path that leaves no region (a capture commit, a
/// cleared region), and the click must light the clip like any other.
#[test]
fn a_click_on_the_bound_clip_selects_its_span() {
    let mut h = distro();
    let clip = h
        .app
        .arr_clip_create(0, 4.0, 8.0, LaneSource::Pattern(PatternId(1)), 0.0)
        .expect("clip");
    h.settle();
    h.click_clip(0, clip);
    h.eval("(eseq.kinds/clear-region!)");
    h.settle();
    assert_eq!(
        h.app.song_clip_selection.map(|selection| selection.clip_id),
        Some(clip),
        "still bound"
    );
    assert_eq!(h.app.song_region_selection, None, "with no region");
    assert!(!h.lit(0));
    h.editor.minibuffer = None;
    h.click_clip(0, clip);
    let region = h
        .app
        .song_region_selection
        .expect("the click selects a region");
    assert_eq!(
        (
            region.track_a,
            region.track_b,
            region.start_beat,
            region.end_beat
        ),
        (0, 0, 4.0, 8.0),
        "the clip's span"
    );
    assert!(h.lit(0), "the lane lights it");
    assert!(
        h.error().starts_with("Bound: "),
        "the click reports the binding: {:?}",
        h.error()
    );
}

/// A clip's `note-dots` are its notes as the view flattens provisional
/// content (`pattern-dots`), and follow a step edit.
#[test]
fn clip_note_dots_match_the_views_flattening() {
    let mut h = distro();
    h.app
        .arr_clip_create(0, 0.0, 8.0, LaneSource::Pattern(PatternId(1)), 0.0)
        .expect("clip");
    for step in [0, 3, 6] {
        app::edit::try_apply_command(&mut h.app, app::AppCommand::ToggleStep { track: 0, step })
            .expect("step edit applies");
    }
    h.settle();
    let flattened = "(let ((c (first (eseq.arrangement/track-clips 0))))
                       (list (len c.note-dots)
                             (= c.note-dots (eseq.arrangement/pattern-dots c.events c.num-steps))))";
    assert_eq!(format!("{:?}", h.eval(flattened)), "(3 true)");
    app::edit::try_apply_command(
        &mut h.app,
        app::AppCommand::ToggleStep { track: 0, step: 9 },
    )
    .expect("step edit applies");
    h.settle();
    assert_eq!(format!("{:?}", h.eval(flattened)), "(4 true)");
}

/// A capture in flight draws from `song.pending-*`: the recorded take and
/// the captured launches, inert (no id), until the capture ends.
#[test]
fn a_capture_in_flight_draws_provisional_items() {
    let mut h = distro();
    h.app.set_arrangement_view_visible(true);
    h.app.song_transport_play(true).expect("capture starts");
    let now = Instant::now();
    h.app.state.transport.record_clock.publish(0.0, now);
    let anchor = now + Duration::from_millis(1);
    h.app.state.transport.record_clock.publish(0.0, anchor);
    // A note at beat 2 (120 BPM).
    assert!(h
        .app
        .take_record_note(0, anchor + Duration::from_secs(1), 60.0, 2.0));
    h.settle();
    let takes = items(
        &h.eval("(filter |item| (= (get item :label) \"Take\") (eseq.arrangement/track-items 0))"),
    );
    assert_eq!(takes.len(), 1, "the recording take draws on its lane");
    let item = &takes[0];
    assert_eq!(get(item, "id"), Value::Nil, "provisional content is inert");
    assert_eq!(get(item, "start"), Value::Number(2.0), "from the punch-in");
    assert_eq!(
        h.eval("(len (eseq.arrangement/track-clips 0))"),
        Value::Number(0.0),
        "nothing is committed yet"
    );
    h.app.song_capture_cancel().expect("cancel");
    h.settle();
    let takes = h.eval(
        "(len (filter |item| (= (get item :label) \"Take\") (eseq.arrangement/track-items 0)))",
    );
    assert_eq!(
        takes,
        Value::Number(0.0),
        "the provisional items go with the capture"
    );
}

/// A rejected arrangement edit shows in the banner (`song.edit-error`) until
/// the next successful one.
#[test]
fn the_edit_error_banner_follows_the_song() {
    let mut h = distro();
    let banner = |h: &Harness| {
        let mut labels = Vec::new();
        widgets_with_prop(&h.buffer_tree("*arrangement*").0, "text", &mut labels);
        labels
            .into_iter()
            .find_map(|label| match label.get("text") {
                Some(Value::String(text)) if text.starts_with("Edit rejected: ") => {
                    Some(text.clone())
                }
                _ => None,
            })
    };
    assert_eq!(banner(&h), None);
    h.command(
        "arrangement-clip-delete",
        map_value([("clip-id", Value::Number(999.0))]),
    );
    h.settle();
    let error = h.app.song_edit_error.clone().expect("a rejection");
    assert_eq!(banner(&h), Some(format!("Edit rejected: {error}")));
    h.command(
        "arrangement-clip-create",
        map_value([
            ("track", Value::Number(0.0)),
            ("start-beat", Value::Number(8.0)),
            ("end-beat", Value::Number(12.0)),
            ("pattern-id", Value::Number(1.0)),
        ]),
    );
    h.settle();
    assert_eq!(
        h.app.song_edit_error, None,
        "a successful edit clears the error"
    );
    assert_eq!(banner(&h), None);
}
