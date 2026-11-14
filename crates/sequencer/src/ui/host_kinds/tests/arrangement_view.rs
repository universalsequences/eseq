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
    pub(super) fn settle(&mut self) {
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
        get(item, "grow"),
        Value::Bool(true),
        "the timeline stretches it to the playhead between head pushes"
    );
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

/// docs/arrangement-rack-lanes-spec.md §5: the rack row edits its
/// rack-lane clips: a drag moves one (anchor and all), an edge drag trims
/// it (anchor kept), and the menu's split, duplicate, change-clip, insert
/// and delete each land as one arrangement edit.
/// A clip-bearing rack with one pad and a rack lane holding one clip of its
/// scene's rack clip over beats 0..8: (group, that rack clip, the lane clip).
fn rack_lane_fixture(h: &mut Harness) -> (u64, u64, ClipId) {
    let (group, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    h.app.assign_rack_pad_track_recorded(group, 36, 0).expect("kick pad");
    h.app.convert_rack_to_clips_recorded(group).expect("clips");
    let verse = h.app.current_rack_clip(group).expect("the scene's clip");
    let mut arrangement = h.app.state.committed_arrangement().expect("arrangement");
    let id = arrangement.allocate_clip_id().expect("id");
    arrangement.end_beat = arrangement.end_beat.max(64.0);
    arrangement.rack_lanes.insert(
        group,
        vec![sequencer::sequencer::RackArrClip {
            id,
            start_beat: 0.0,
            end_beat: 8.0,
            rack_clip: verse,
            anchor_beat: 0.0,
        }],
    );
    h.app.state.set_committed_arrangement(Some(arrangement)).expect("commit");
    h.share_buses_and_groups();
    h.settle();
    (group, verse, id)
}

/// Rack `group`'s lane as (start, end, rack clip, anchor) per clip.
fn rack_lane_of(h: &Harness, group: u64) -> Vec<(f64, f64, u64, f64)> {
    h.app
        .state
        .committed_arrangement()
        .expect("arrangement")
        .rack_lanes
        .get(&group)
        .map(|lane| {
            lane.iter()
                .map(|clip| (clip.start_beat, clip.end_beat, clip.rack_clip, clip.anchor_beat))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_rack_row_edits_its_rack_lane_clips() {
    let mut h = distro();
    let (group, verse, id) = rack_lane_fixture(&mut h);
    let lane = |h: &Harness| rack_lane_of(h, group);
    let act = |h: &mut Harness, event: String| {
        h.eval(&format!(
            "(eseq.arrangement/rack-lane-action (first (eseq.kinds/groups)) {event})"
        ));
        h.settle();
    };
    let edit = |h: &mut Harness, payload: String| {
        h.eval(&format!("(eseq.arrangement/rack-edit {payload})"));
        h.settle();
    };

    // Title-bar drag: the clip and its anchor move together.
    act(&mut h, format!("(dict :type :move-items-absolute :anchor-id {} :start 4)", id.0));
    act(&mut h, "(dict :type :finish-move-items)".into());
    assert_eq!(lane(&h), vec![(4.0, 12.0, verse, 4.0)]);
    // Start-edge drag: the span trims, the anchor stays.
    act(
        &mut h,
        format!("(dict :type :resize-item-absolute :id {} :edge :start :time 6)", id.0),
    );
    act(&mut h, "(dict :type :finish-resize-items)".into());
    assert_eq!(lane(&h), vec![(6.0, 12.0, verse, 4.0)]);
    // Split, then duplicate the right half after itself.
    edit(&mut h, format!("(dict :type :rack-clip-split :clip-id {} :beat 8)", id.0));
    assert_eq!(lane(&h), vec![(6.0, 8.0, verse, 4.0), (8.0, 12.0, verse, 4.0)]);
    let right = h.app.state.committed_arrangement().unwrap().rack_lanes[&group][1].id;
    edit(&mut h, format!("(dict :type :rack-clip-duplicate :clip-id {})", right.0));
    assert_eq!(lane(&h)[2], (12.0, 16.0, verse, 8.0));
    // Change clip: a second rack clip, swapped in place.
    let chorus = h.app.save_rack_clip_as_recorded(group, "Chorus").expect("chorus");
    edit(
        &mut h,
        format!("(dict :type :rack-clip-set :clip-id {} :rack-clip {chorus})", right.0),
    );
    assert_eq!(lane(&h)[1].2, chorus);
    // Insert on empty lane: one loop of the clip, anchored at its start.
    edit(
        &mut h,
        format!("(dict :type :rack-clip-place :group {group} :rack-clip {verse} :start 32)"),
    );
    assert_eq!(lane(&h).last().map(|clip| (clip.0, clip.3)), Some((32.0, 32.0)));
    // Backspace deletes the selected clip.
    act(&mut h, format!("(dict :type :delete-items :ids (list {}))", id.0));
    assert_eq!(lane(&h).len(), 3, "the first half is gone");
    assert_eq!(lane(&h)[0].0, 8.0);

    // The context menu: over a clip it offers the clip verbs, over empty
    // lane the rack's clips to insert.
    let menu_keys = |h: &mut Harness, beat: f64| {
        h.eval(&format!(
            "(let ((g (first (eseq.kinds/groups))) (m eseq.arrangement/arr-rack-menu)) \
               (eseq.arrangement/open-rack-menu g (dict :sx -1)) \
               (set! m.clip (eseq.arrangement/rack-clip-at g {beat})))"
        ));
        h.settle();
        let mut nodes = Vec::new();
        widgets_with_prop(&h.buffer_tree("*arrangement*").0, "key", &mut nodes);
        nodes
            .into_iter()
            .filter_map(|node| match node.get("key") {
                Some(Value::String(key)) if key.starts_with("arr-rack-") => Some(key.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let over_clip = menu_keys(&mut h, 9.0);
    for key in ["arr-rack-change", "arr-rack-split", "arr-rack-duplicate", "arr-rack-delete"] {
        assert!(over_clip.iter().any(|k| k == key), "{key} in {over_clip:?}");
    }
    let empty = menu_keys(&mut h, 20.0);
    assert!(empty.iter().any(|k| k == "arr-rack-insert"), "{empty:?}");
}

/// docs/arrangement-rack-lanes-spec.md §6: while a capture runs, a
/// clip-bearing rack's row grows with the rack clips the captured scene
/// launches played, and its members' rows with their patterns, inert until
/// the capture ends.
#[test]
fn a_capture_in_flight_grows_the_rack_row() {
    let mut h = distro();
    let (group, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    h.app.assign_rack_pad_track_recorded(group, 36, 0).expect("kick pad");
    h.app.convert_rack_to_clips_recorded(group).expect("clips");
    let verse = h.app.current_rack_clip(group).expect("the scene's clip");
    h.share_buses_and_groups();
    h.app.set_arrangement_view_visible(true);
    h.app.song_transport_play(true).expect("capture starts");
    // Put the record head 8 beats in, past the starting scene's launch. The
    // beats carry the offset: the clock counts from a process-wide origin, so
    // an anchor back-dated by 4 s saturates to zero in a process younger than
    // that, and the head only moved while booting the harness took longer.
    h.app.state.transport.record_clock.publish(8.0, Instant::now());
    h.settle();
    let head = quantized_pending_head(&h.app).expect("a record head");
    assert!(head > 0.0, "the record head moved: {head}");
    let pending = items(&h.eval("(let ((g (first (eseq.kinds/groups)))) g.pending-arr-clips)"));
    assert_eq!(pending.len(), 1, "the starting scene's rack clip");
    assert_eq!(get(&pending[0], "rack-clip"), Value::Number(verse as f64));
    assert_eq!(get(&pending[0], "start"), Value::Number(0.0));

    let rack_items =
        items(&h.eval("(eseq.arrangement/rack-lane-items (first (eseq.kinds/groups)))"));
    assert_eq!(rack_items.len(), 1, "the provisional rack-lane clip draws");
    assert_eq!(get(&rack_items[0], "id"), Value::Nil, "inert");
    // The growing span is pushed with no span of its own: the timeline
    // stretches it to the playhead, so no lane reruns on a head tick.
    assert_eq!(get(&rack_items[0], "end"), Value::Number(0.0));
    assert_eq!(get(&rack_items[0], "grow"), Value::Bool(true));
    let member_items = items(&h.eval("(eseq.arrangement/track-items 0)"));
    let member = member_items
        .iter()
        .find(|item| get(item, "grow") == Value::Bool(true))
        .expect("the member's row grows too");
    assert_eq!(get(member, "end"), Value::Number(0.0));

    h.app.song_capture_cancel().expect("cancel");
    h.settle();
    let pending = items(&h.eval("(let ((g (first (eseq.kinds/groups)))) g.pending-arr-clips)"));
    assert!(pending.is_empty(), "the provisional clips go with the capture");
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

/// docs/arrangement-rack-lanes-spec.md §6: a clip-bearing rack gets a rack
/// row above its first member, drawing its rack lane's clips as ring
/// summaries (one ring per member) labelled with the rack clip; collapsing
/// the rack hides the member rows and keeps the rack row.
#[test]
fn a_clip_bearing_rack_draws_a_rack_row_of_ring_summaries() {
    let mut h = distro();
    let (group, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    h.app.assign_rack_pad_track_recorded(group, 36, 0).expect("kick pad");
    h.app.assign_rack_pad_track_recorded(group, 38, 1).expect("snare pad");
    h.app.convert_rack_to_clips_recorded(group).expect("clips");
    let verse = h.app.current_rack_clip(group).expect("the scene's clip");
    let mut arrangement = h.app.state.committed_arrangement().expect("arrangement");
    let id = arrangement.allocate_clip_id().expect("id");
    arrangement.rack_lanes.insert(
        group,
        vec![sequencer::sequencer::RackArrClip {
            id,
            start_beat: 0.0,
            end_beat: 8.0,
            rack_clip: verse,
            anchor_beat: 0.0,
        }],
    );
    h.app.state.set_committed_arrangement(Some(arrangement)).expect("commit");
    h.share_buses_and_groups();
    h.settle();

    let rack_lane = |h: &Harness| {
        let mut lanes = Vec::new();
        widgets_with_prop(&h.buffer_tree("*arrangement*").0, "items", &mut lanes);
        lanes
            .into_iter()
            .find(|lane| lane.get("key") == Some(&s(&format!("rack-lane-{group}"))))
    };
    let lane = rack_lane(&h).expect("the rack row's lane");
    let lane_items = items(lane.get("items").unwrap_or(&Value::Nil));
    assert_eq!(lane_items.len(), 1, "one rack-lane clip");
    let Value::Map(item) = &lane_items[0] else { panic!("item map") };
    let field = |name: &str| item.get(name).map(|value| value.borrow().clone());
    assert_eq!(field("end"), Some(Value::Number(8.0)));
    // The timeline shows the clip's display label ("Clip N" while unnamed).
    let label = h.app.state.with_scenes(|scenes| {
        scenes.rack_bank(group).and_then(|bank| bank.clip_label(verse))
    });
    assert_eq!(field("label"), Some(s(&label.expect("clip label"))));
    let Some(Value::Map(content)) = field("content") else { panic!("ring content") };
    let rings = items(&content.get("rings").expect("rings").borrow());
    assert_eq!(rings.len(), 2, "one ring per member");

    // Each member's row draws its pattern in the rack clip over the span,
    // under a string id no stored clip has: the rack lane owns the span.
    let verse_cells = h.app.state.with_scenes(|scenes| {
        scenes.rack_bank(group).and_then(|bank| bank.clip(verse)).map(|clip| clip.cells.clone())
    });
    let verse_cells = verse_cells.expect("verse cells");
    for member in 0..2 {
        let member_items = items(&h.eval(&format!("(eseq.arrangement/track-items {member})")));
        assert_eq!(member_items.len(), 1, "member {member} draws the rack clip's pattern");
        let item = &member_items[0];
        assert_eq!(get(item, "start"), Value::Number(0.0));
        assert_eq!(get(item, "end"), Value::Number(8.0));
        let pid = verse_cells[member].expect("a member pattern").0;
        assert_eq!(get(item, "label"), s(&format!("Pattern {pid}")));
        assert_eq!(get(item, "id"), s(&format!("rack-{}-{member}", id.0)));
    }

    // A click on one binds the member's pattern in that rack clip as the
    // track's edit focus, and lights the item.
    h.eval(&format!(
        "(eseq.arrangement/track-action 1 (dict :type :select :ids (list \"rack-{}-1\") :time 0))",
        id.0
    ));
    h.settle();
    let snare = verse_cells[1].expect("snare pattern");
    assert_eq!(
        h.app.song_clip_selection,
        Some(sequencer::app::sound_binding::SongClipSelection {
            track: 1,
            clip_id: id,
            source: sequencer::app::sound_binding::BoundSource::Pattern(snare),
        })
    );
    let member_items = items(&h.eval("(eseq.arrangement/track-items 1)"));
    assert_eq!(get(&member_items[0], "selected"), Value::Bool(true));

    // Make Own Clip copies it onto the member's lane as a pattern clip, an
    // override that wins over the rack lane there.
    h.eval(
        "(eseq.arrangement/make-own-clip 1 (first (eseq.arrangement/rack-member-items 1)))",
    );
    h.settle();
    let own = h.app.state.committed_arrangement().expect("arrangement").track_lanes[1].clone();
    assert_eq!(own.len(), 1, "one own clip");
    assert_eq!(own[0].pattern_id, Some(snare.0));
    assert_eq!((own[0].start_beat, own[0].end_beat), (0.0, 8.0));

    // The members' rows stay below the rack row; collapsing the rack hides
    // them and keeps the rack row.
    let lane_keys = |h: &Harness| {
        let mut lanes = Vec::new();
        widgets_with_prop(&h.buffer_tree("*arrangement*").0, "lane-key", &mut lanes);
        lanes
            .into_iter()
            // Track lanes only: a rack lane's key is negative.
            .filter_map(|lane| match lane.get("lane-key") {
                Some(Value::Number(n)) if *n >= 0.0 => Some(*n as usize),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(lane_keys(&h).contains(&0) && lane_keys(&h).contains(&1));
    h.eval("(let ((g (first (eseq.kinds/groups)))) (toggle! g.collapsed))");
    // Group collapse lands in the shared groups the app's tick pulls from.
    h.app.groups = h.shared.track_groups.lock().unwrap().clone();
    h.settle();
    let shown = lane_keys(&h);
    assert!(!shown.contains(&0) && !shown.contains(&1), "members hidden: {shown:?}");
    assert!(rack_lane(&h).is_some(), "the rack row stays");
}

#[test]
fn region_ops_carry_the_rack_lanes_of_the_rack_rows_they_cover() {
    let mut h = distro();
    let (group, verse, id) = rack_lane_fixture(&mut h);
    let lane = |h: &Harness| rack_lane_of(h, group);
    let act = |h: &mut Harness, event: &str| {
        h.eval(&format!(
            "(eseq.arrangement/rack-lane-action (first (eseq.kinds/groups)) {event})"
        ));
        h.settle();
    };
    let lit = |h: &mut Harness| {
        h.eval("(eseq.arrangement/rack-lane-region (first (eseq.kinds/groups)))")
    };

    // Clicking a rack-lane clip selects its span on the rack row alone.
    act(&mut h, &format!("(dict :type :select :ids (list {}) :time 1)", id.0));
    let region = h.app.song_region_selection.expect("a one-clip region");
    assert!(region.rack_a && region.rack_b, "{region:?}");
    assert_eq!((region.start_beat, region.end_beat), (0.0, 8.0));
    assert_eq!(h.app.region_racks(&region), vec![group]);
    assert_ne!(lit(&mut h), Value::Nil, "the rack lane lights the region");
    assert_eq!(
        h.eval("(let ((s eseq.kinds/song) (r s.region)) (len r.tracks))"),
        Value::Number(0.0),
        "no track row is in it"
    );

    // Copy and paste: the pasted clip keeps its phase relative to its span.
    let clipboard = h.app.song_region_copy().expect("copy");
    assert_eq!(clipboard.racks.len(), 1);
    h.app.song_region_paste(&clipboard, 16.0).expect("paste");
    assert_eq!(lane(&h), vec![(0.0, 8.0, verse, 0.0), (16.0, 24.0, verse, 16.0)]);

    // A region move drags the anchor along; the region follows.
    h.app.song_region_move(4.0).expect("move");
    assert_eq!(lane(&h)[0], (4.0, 12.0, verse, 4.0));
    let region = h.app.song_region_selection.expect("region");
    assert_eq!((region.start_beat, region.end_beat), (4.0, 12.0));

    // Delete silences the rack row's span.
    h.app.song_region_delete().expect("delete");
    assert_eq!(lane(&h), vec![(16.0, 24.0, verse, 16.0)]);

    // A sweep over the rack row only lights that lane and no track lane.
    act(&mut h, "(dict :type :marquee-select :time-a 16 :time-b 24 :row-delta 0)");
    assert_ne!(lit(&mut h), Value::Nil, "the rack lane draws the sweep");
    assert_eq!(
        h.eval("(eseq.arrangement/lane-region-rect 0)"),
        Value::Nil,
        "its first member's lane does not"
    );

    // That sweep, then duplicate: the copy lands right after with its phase
    // continued, and what followed ripples right.
    act(
        &mut h,
        "(dict :type :finish-marquee-select :time-a 16 :time-b 24 :row-delta 0)",
    );
    h.app.song_region_duplicate().expect("duplicate");
    assert_eq!(
        lane(&h),
        vec![(16.0, 24.0, verse, 16.0), (24.0, 32.0, verse, 24.0)]
    );

    // A sweep from the rack row down one row covers the rack and its first
    // member; one back up from the member covers the same two rows.
    let member = h.app.state.with_project_scenes(|scenes| {
        scenes.rack_bank(group).map(|bank| bank.members[0]).expect("member")
    });
    act(
        &mut h,
        "(dict :type :finish-marquee-select :time-a 0 :time-b 4 \
           :row-delta eseq.arrangement/track-row-pitch)",
    );
    let down = h.app.song_region_selection.expect("region");
    assert_eq!(
        (down.track_a, down.rack_a, down.track_b, down.rack_b),
        (member, true, member, false)
    );
    h.eval(&format!(
        "(eseq.arrangement/track-action {member} (dict :type :finish-marquee-select \
           :time-a 0 :time-b 4 :row-delta (- 0 eseq.arrangement/track-row-pitch)))"
    ));
    h.settle();
    assert_eq!(h.app.song_region_selection, Some(down));

    // Backspace on a member row's item deletes the rack-lane clip it shows.
    let cid = h.app.state.committed_arrangement().unwrap().rack_lanes[&group][0].id;
    h.eval(&format!(
        "(eseq.arrangement/track-action {member} \
           (dict :type :select :ids (list \"rack-{}-{member}\") :time 17))",
        cid.0
    ));
    h.settle();
    h.eval(&format!(
        "(eseq.arrangement/track-action {member} (dict :type :delete-items :ids (list)))"
    ));
    h.settle();
    assert_eq!(lane(&h), vec![(24.0, 32.0, verse, 24.0)]);
}

#[test]
fn select_all_then_delete_empties_the_track_and_rack_lanes() {
    let mut h = distro();
    let (group, _, _) = rack_lane_fixture(&mut h);
    // A track clip after the rack lane's, so the span reaches past it.
    h.app
        .arr_clip_create(0, 8.0, 16.0, LaneSource::Pattern(PatternId(1)), 0.0)
        .expect("track clip");
    h.settle();

    // Cmd+A, then Backspace (which lowers to song-region-delete).
    assert_eq!(h.eval("(eseq.arrangement/select-all-clips)"), Value::Bool(true));
    h.settle();
    let region = h.app.song_region_selection.expect("a region");
    assert_eq!((region.start_beat, region.end_beat), (0.0, 16.0));
    assert_eq!(h.app.region_racks(&region), vec![group]);
    h.app.song_region_delete().expect("delete");
    let arrangement = h.app.state.committed_arrangement().expect("arrangement");
    assert!(arrangement.rack_lanes.values().all(Vec::is_empty), "rack lanes cleared");
    assert!(arrangement.track_lanes.iter().all(Vec::is_empty), "track lanes cleared");
}

/// The view pages to the playhead only when it leaves the window, putting
/// it a third of the way in (`follow-anchor`), and otherwise holds still;
/// a hand scroll pauses
/// that for `arrangement-follow-cooloff` seconds or until the next Play, and
/// the off switch stops it.
#[test]
fn the_view_follows_the_playhead_until_a_hand_scroll() {
    let mut h = distro();
    h.app.set_arrangement_view_visible(true);
    h.app.song_transport_play(true).expect("capture starts");
    let head = |h: &mut Harness, beat: f64| {
        h.app.state.transport.record_clock.publish(beat, Instant::now());
        h.settle();
    };
    let start = |h: &mut Harness| match h.arr_field("arr-view", "start") {
        Value::Number(n) => n,
        other => panic!("start {other:?}"),
    };

    // The head at `beat` sits `follow-anchor` of the 64-beat window in.
    let anchored = |beat: f64| beat - 0.33 * 64.0;
    head(&mut h, 8.0);
    assert_eq!(start(&mut h), 0.0, "an on-screen head never moves the view");
    head(&mut h, 60.0);
    assert_eq!(start(&mut h), 0.0, "not even near the right edge");
    head(&mut h, 200.0);
    let at = start(&mut h);
    assert!((at - anchored(200.0)).abs() < 1.0, "off screen: the view pages to it: {at}");
    head(&mut h, 230.0);
    assert_eq!(start(&mut h), at, "on screen again, so it holds still");
    // A loop wrapping the head back before the window pages back to it.
    head(&mut h, 150.0);
    let back = start(&mut h);
    assert!((back - anchored(150.0)).abs() < 1.0, "paged back to the wrap: {back}");

    // A hand scroll holds the view where the user put it.
    h.eval("(setopt eseq.arrangement/arrangement-follow-cooloff 0.2)");
    h.settle();
    h.eval("(eseq.arrangement/view-action (dict :type :scroll-view :view-start 0))");
    head(&mut h, 300.0);
    assert_eq!(start(&mut h), 0.0, "paused by the hand scroll");

    // A spent cooloff resumes following on the next head push alone: the
    // paused run must still have subscribed to the position.
    std::thread::sleep(Duration::from_millis(300));
    head(&mut h, 400.0);
    let at = start(&mut h);
    assert!((at - anchored(400.0)).abs() < 1.0, "following again: {at}");

    // Play ends a pause still inside its cooloff: scroll back, stop, play
    // again, and the view follows at once.
    h.eval("(setopt eseq.arrangement/arrangement-follow-cooloff 60)");
    h.eval("(eseq.arrangement/view-action (dict :type :scroll-view :view-start 0))");
    head(&mut h, 450.0);
    assert_eq!(start(&mut h), 0.0, "paused by the hand scroll");
    h.app.song_capture_cancel().expect("stop");
    h.settle();
    h.app.song_transport_play(true).expect("play again");
    head(&mut h, 500.0);
    let at = start(&mut h);
    assert!((at - anchored(500.0)).abs() < 1.0, "Play cleared the pause: {at}");

    // Off: the view stays put.
    h.eval("(setopt eseq.arrangement/arrangement-follow-playhead false)");
    h.eval("(eseq.arrangement/view-action (dict :type :scroll-view :view-start 0))");
    head(&mut h, 600.0);
    assert_eq!(start(&mut h), 0.0, "follow is off");
    h.app.song_capture_cancel().expect("cancel");
}

/// eseq-vum0: a recorded scene push draws on the push lane under the scene
/// lane — labeled with its target scene, its curve as item content — and
/// the lane's delete gesture removes it as one arrangement edit.
#[test]
fn recorded_scene_pushes_draw_and_delete_on_the_push_lane() {
    use sequencer::sequencer::{ScenePushPoint, ScenePushRegion};
    let mut h = distro();
    h.app.set_arrangement_view_visible(true);
    h.command("clone-pattern", Value::Nil);
    h.settle();
    assert_eq!(h.eval("(len (eseq.arrangement/push-items))"), Value::Number(0.0));
    let mut arrangement = h.app.state.committed_arrangement().expect("arrangement");
    let id = arrangement.allocate_clip_id().expect("id");
    arrangement.end_beat = arrangement.end_beat.max(16.0);
    arrangement.push_lane.push(ScenePushRegion {
        id,
        start_beat: 4.0,
        end_beat: 8.0,
        target_scene: 1,
        points: vec![
            ScenePushPoint { beat: 0.0, value: 0.0 },
            ScenePushPoint { beat: 2.0, value: 1.0 },
        ],
    });
    h.app.state.set_committed_arrangement(Some(arrangement)).expect("commit");
    h.settle();

    let pushes = items(&h.eval("(eseq.arrangement/push-items)"));
    assert_eq!(pushes.len(), 1);
    let item = &pushes[0];
    assert_eq!(get(item, "id"), Value::Number(id.0 as f64));
    assert_eq!(get(item, "start"), Value::Number(4.0));
    assert_eq!(get(item, "end"), Value::Number(8.0));
    let Value::String(label) = get(item, "label") else { panic!("a label") };
    assert!(label.starts_with("→ "), "names the target scene: {label}");
    assert_eq!(
        h.eval("(let ((p (first (eseq.arrangement/push-items)))) (len (get (get p :content) :curve)))"),
        Value::Number(2.0),
        "the curve rides as item content"
    );

    h.eval(&format!("(eseq.arrangement/push-lane-action (dict :type :delete-items :ids (list {})))", id.0));
    h.settle();
    assert!(h.app.state.committed_arrangement().expect("arrangement").push_lane.is_empty());
    assert_eq!(h.eval("(len (eseq.arrangement/push-items))"), Value::Number(0.0));

    // Cmd+A selects the pushes with everything else (they light up), and
    // the region delete Backspace sends takes them too.
    sequencer::app::edit::undo(&mut h.app);
    h.settle();
    assert_eq!(h.eval("(len (eseq.arrangement/push-items))"), Value::Number(1.0));
    assert_eq!(h.eval("(eseq.arrangement/select-all-clips)"), Value::Bool(true));
    h.settle();
    assert_eq!(
        h.eval("(let ((r (eseq.arrangement/push-selection-rect))) (and (<= (get r :time-a) 4) (>= (get r :time-b) 8)))"),
        Value::Bool(true),
        "Cmd+A lights the push lane over the push"
    );
    h.command("song-region-delete", Value::Nil);
    h.settle();
    assert!(
        h.app.state.committed_arrangement().expect("arrangement").push_lane.is_empty(),
        "Cmd+A, Backspace deletes the push"
    );
}

/// eseq-vum0: a sweep on the push lane selects a time range of it alone;
/// Backspace clears just that range (cutting the push it straddles). An
/// edge drag resizes a push: in trims the curve, out holds its value.
#[test]
fn push_lane_sweeps_clear_a_range_and_edges_resize() {
    use sequencer::sequencer::{ScenePushPoint, ScenePushRegion};
    let mut h = distro();
    h.app.set_arrangement_view_visible(true);
    h.command("clone-pattern", Value::Nil);
    h.settle();
    let mut arrangement = h.app.state.committed_arrangement().expect("arrangement");
    let id = arrangement.allocate_clip_id().expect("id");
    arrangement.end_beat = arrangement.end_beat.max(32.0);
    arrangement.push_lane.push(ScenePushRegion {
        id,
        start_beat: 4.0,
        end_beat: 12.0,
        target_scene: 1,
        points: vec![
            ScenePushPoint { beat: 0.0, value: 0.0 },
            ScenePushPoint { beat: 8.0, value: 1.0 },
        ],
    });
    h.app.state.set_committed_arrangement(Some(arrangement)).expect("commit");
    h.settle();
    let spans = |h: &Harness| -> Vec<(f64, f64)> {
        let arrangement = h.app.state.committed_arrangement().expect("arrangement");
        arrangement.push_lane.iter().map(|push| (push.start_beat, push.end_beat)).collect()
    };

    h.eval("(eseq.arrangement/push-lane-action (dict :type :marquee-select :time-a 10 :time-b 6))");
    h.eval("(eseq.arrangement/push-lane-action (dict :type :finish-marquee-select :time-a 10 :time-b 6))");
    h.settle();
    assert_eq!(
        h.eval("(get (first (eseq.arrangement/push-items)) :selected)"),
        Value::Bool(false),
        "a range lights its span, not the whole push"
    );
    assert_eq!(
        h.eval("(let ((r (eseq.arrangement/push-selection-rect))) (list (get r :time-a) (get r :time-b)))"),
        h.eval("(list 6 10)"),
        "the swept range, ordered"
    );
    h.eval("(eseq.arrangement/push-lane-action (dict :type :delete-range :time-a 6 :time-b 10))");
    h.settle();
    assert_eq!(spans(&h), vec![(4.0, 6.0), (10.0, 12.0)], "only the range goes");

    let first = h.app.state.committed_arrangement().unwrap().push_lane[0].id;
    h.eval(&format!(
        "(eseq.arrangement/push-lane-action (dict :type :resize-item-absolute :id {} :edge :end :time 9))",
        first.0
    ));
    h.settle();
    assert_eq!(
        h.eval("(get (first (eseq.arrangement/push-items)) :end)"),
        Value::Number(9.0),
        "the edge ghosts while dragged"
    );
    h.eval("(eseq.arrangement/push-lane-action (dict :type :finish-resize-items))");
    h.settle();
    assert_eq!(spans(&h), vec![(4.0, 9.0), (10.0, 12.0)]);
    let pushed = h.app.state.committed_arrangement().unwrap().push_lane[0].clone();
    assert!((pushed.value_at(8.5) - 0.25).abs() < 1.0e-4, "pulled out, it holds where it was cut");

    h.eval(&format!(
        "(eseq.arrangement/push-lane-action (dict :type :resize-item-absolute :id {} :edge :start :time 5))",
        first.0
    ));
    h.eval("(eseq.arrangement/push-lane-action (dict :type :finish-resize-items))");
    h.settle();
    assert_eq!(spans(&h), vec![(5.0, 9.0), (10.0, 12.0)]);
    let pushed = h.app.state.committed_arrangement().unwrap().push_lane[0].clone();
    assert!((pushed.value_at(5.0) - 0.125).abs() < 1.0e-4, "the curve stays in time");
}

/// eseq-vum0: while an edge of a push drags, its curve keeps its beats (an
/// edge cut in trims it, one pulled out holds) instead of stretching across
/// the ghost — the ghost draws exactly the curve the release commits.
#[test]
fn push_resize_ghost_draws_the_curve_the_release_commits() {
    use sequencer::sequencer::{ScenePushPoint, ScenePushRegion};
    let mut h = distro();
    h.app.set_arrangement_view_visible(true);
    h.command("clone-pattern", Value::Nil);
    h.settle();
    let mut arrangement = h.app.state.committed_arrangement().expect("arrangement");
    let id = arrangement.allocate_clip_id().expect("id");
    arrangement.end_beat = arrangement.end_beat.max(32.0);
    arrangement.push_lane.push(ScenePushRegion {
        id,
        start_beat: 4.0,
        end_beat: 12.0,
        target_scene: 1,
        points: vec![
            ScenePushPoint { beat: 0.0, value: 0.0 },
            ScenePushPoint { beat: 2.0, value: 1.0 },
            ScenePushPoint { beat: 8.0, value: 0.0 },
        ],
    });
    h.app.state.set_committed_arrangement(Some(arrangement)).expect("commit");
    h.settle();
    // The drawn curve as (offset, value) pairs; offsets are region fractions.
    let curve = |h: &mut Harness| -> Vec<(f64, f64)> {
        let flat = h.eval(
            "(reduce |acc p| (append acc (list p.offset p.value)) '() \
               (get (get (first (eseq.arrangement/push-items)) :content) :curve))",
        );
        let Value::List(items) = flat else { panic!("curve list, got {flat:?}") };
        let numbers: Vec<f64> = items
            .iter()
            .map(|item| match *item.borrow() {
                Value::Number(n) => n,
                ref other => panic!("number, got {other:?}"),
            })
            .collect();
        numbers.chunks(2).map(|pair| (pair[0], pair[1])).collect()
    };
    // Drawn as beats, so a ghost and a committed curve that look the same
    // compare equal however their points are split.
    let sample = |points: &[(f64, f64)]| -> Vec<f64> {
        (0..=32)
            .map(|i| {
                let at = i as f64 / 32.0;
                let before = points.iter().rev().find(|p| p.0 <= at);
                let after = points.iter().find(|p| p.0 > at);
                match (before, after) {
                    (Some(p), Some(q)) => p.1 + (q.1 - p.1) * (at - p.0) / (q.0 - p.0),
                    (Some(p), None) => p.1,
                    (None, Some(q)) => q.1,
                    (None, None) => 0.0,
                }
            })
            .map(|value| (value * 1.0e4).round() / 1.0e4)
            .collect()
    };
    let span = |h: &mut Harness| -> (f64, f64) {
        let item = h.eval("(let ((i (first (eseq.arrangement/push-items)))) (list (get i :start) (get i :end)))");
        let Value::List(items) = item else { panic!("span") };
        let n = |i: usize| match *items[i].borrow() {
            Value::Number(n) => n,
            _ => panic!("span number"),
        };
        (n(0), n(1))
    };

    // End in, end out past the original, start in, start out.
    for (edge, time) in [("end", 8.0), ("end", 14.0), ("start", 7.0), ("start", 2.0)] {
        h.eval(&format!(
            "(eseq.arrangement/push-lane-action (dict :type :resize-item-absolute :id {} :edge :{edge} :time {time}))",
            id.0
        ));
        h.settle();
        let ghost_span = span(&mut h);
        let ghost = curve(&mut h);
        h.eval("(eseq.arrangement/push-lane-action (dict :type :finish-resize-items))");
        h.settle();
        assert_eq!(span(&mut h), ghost_span, "{edge} → {time}: the ghost spans what commits");
        let committed = curve(&mut h);
        assert_eq!(
            sample(&ghost),
            sample(&committed),
            "{edge} → {time}: ghost {ghost:?} draws the committed {committed:?}"
        );
    }
}

/// The ruler's loop brace (Ableton's loop): a drag repaints it live and
/// commits once, on release; the toggle turns looping on with the brace
/// where it is, and the ruler draws exactly what the song holds.
#[test]
fn the_loop_brace_drags_live_commits_once_and_toggles() {
    let mut h = distro();
    h.settle();
    let committed = |h: &Harness| {
        h.app.state.with_committed_arrangement(|arrangement| {
            let arrangement = arrangement.expect("an arrangement");
            (arrangement.loop_window(), arrangement.loop_enabled)
        })
    };
    let (before, looping) = committed(&h);
    assert!(!looping, "looping starts off");

    h.eval("(eseq.arrangement/scene-action (dict :type :loop-brace :start 4 :end 12))");
    h.settle();
    assert_eq!(h.arr_field("arr-view", "loop-start"), Value::Number(4.0));
    assert_eq!(h.arr_field("arr-view", "loop-end"), Value::Number(12.0));
    assert_eq!(committed(&h).0, before, "a live drag frame commits nothing");

    h.eval("(eseq.arrangement/scene-action (dict :type :finish-loop-brace :start 4 :end 12))");
    h.settle();
    assert_eq!(committed(&h).0, (4.0, 12.0));
    assert_eq!(h.arr_field("arr-view", "loop-dragging"), Value::Bool(false));

    h.eval("(let ((s eseq.kinds/song)) (toggle! s.loop))");
    h.settle();
    assert_eq!(committed(&h), ((4.0, 12.0), true));
    assert_eq!(h.arr_field("arr-view", "loop-on"), Value::Number(1.0));

    // The ruler is the only lane with the brace, bound to the view state.
    let mut braced = Vec::new();
    widgets_with_prop(&h.buffer_tree("*arrangement*").0, "loop-brace", &mut braced);
    assert_eq!(braced.len(), 1, "one ruler draws the brace");
}

/// A sound designed in session view survives the arrangement round trip:
/// scene 1 launched (every lane latched and pinned to its pattern), its clip
/// clicked and released in the arrangement, a scene cloned with `+`, a Place
/// rejected for overlap, back to session. Releasing the clip used to load
/// the track's stock sound into the latched lane, and the clone's save-back
/// wrote it over scene 1's pattern.
#[test]
fn a_session_sound_survives_a_clip_click_and_scene_clone_in_the_arrangement() {
    let mut h = distro();
    h.app.set_arrangement_view_visible(false);
    let p1 = h
        .app
        .state
        .with_project_scenes(|scenes| scenes.effective_pattern_id(0))
        .expect("scene 1 has a pattern on track 0");
    let target = 0.33f32;
    sequencer::app::try_apply_command(
        &mut h.app,
        sequencer::app::AppCommand::SetTrackVolume { track: 0, value: target },
    )
    .expect("edit");
    let stored = |h: &Harness, pattern| {
        h.app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0].get(pattern).expect("pattern").track_params.volume
        })
    };
    let live = |h: &Harness| h.app.state.pattern.track_params[0].get_volume();
    let step = |h: &mut Harness| {
        h.settle();
        h.app.sync_track_sound_bindings();
    };
    // The scene pill: a manual launch latches every lane.
    h.command("switch-pattern", map_value([("idx", Value::Number(0.0))]));
    step(&mut h);
    assert_ne!(h.app.state.song_manual_latch_mask() & 1, 0, "the launch latches");

    h.app.set_arrangement_view_visible(true);
    h.command(
        "arrangement-scene-insert",
        map_value([
            ("beat", Value::Number(0.0)),
            ("scene", Value::Number(0.0)),
            ("place-patterns", Value::Bool(true)),
        ]),
    );
    step(&mut h);
    let clip = h.app.state.committed_arrangement().expect("arrangement").track_lanes[0][0].id;
    h.click_clip(0, clip);
    h.app.sync_track_sound_bindings();
    h.eval("(eseq.arrangement/track-action 0 (dict :type :select :ids (list) :time 90))");
    step(&mut h);
    assert_eq!(h.app.song_clip_selection, None);
    assert_eq!(live(&h), target, "releasing the clip keeps the latched pattern's sound");

    h.click_clip(0, clip);
    h.app.sync_track_sound_bindings();
    h.eval_with("(import eseq.transport :refer (seq-clone-pattern))", "(seq-clone-pattern)");
    step(&mut h);
    assert_eq!(h.app.state.current_scene_index(), 1, "the clone is current");
    assert_eq!(stored(&h, p1), target, "the clone's save-back spares scene 1");
    let p2 = h
        .app
        .state
        .with_project_scenes(|scenes| scenes.effective_pattern_id(0))
        .expect("the clone has a pattern");
    assert_ne!(p2, p1);
    assert_eq!(
        h.app.state.with_project_scenes(|scenes| scenes.track_overrides[0]),
        Some(p2),
        "the latched lane is pinned to the clone"
    );

    h.eval("(eseq.arrangement/begin-placement)");
    h.settle();
    h.eval("(eseq.arrangement/track-action 0 (dict :type :place-item :time 4))");
    step(&mut h);
    assert!(h.app.song_edit_error.is_some(), "the overlapping Place is rejected");
    h.eval("(eseq.arrangement/track-action 0 (dict :type :select :ids (list) :time 90))");
    step(&mut h);
    assert_eq!(live(&h), target, "the lane still sounds as designed");

    h.app.set_arrangement_view_visible(false);
    h.command("switch-pattern", map_value([("idx", Value::Number(0.0))]));
    step(&mut h);
    assert_eq!((stored(&h, p1), stored(&h, p2)), (target, target));
    assert_eq!(live(&h), target, "scene 1 plays the sound it was given");
}
