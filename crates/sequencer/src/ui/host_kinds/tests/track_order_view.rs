//! Track reordering (eseq-ssd6): the grid, the mixer and the arrangement draw
//! rows in the display order, and dropping a track's name (or a group) on a
//! row in any of them moves it there through the shipped drop handlers.

use super::views::{distro, widget_keyed, widgets_with_prop};
use super::*;

impl Harness {
    /// Track indices of the rows `expr` (a list of tracks) names, in order.
    fn order_of(&mut self, expr: &str) -> Vec<f64> {
        let value = self.eval(&format!("(map (lambda (t) t.index) {expr})"));
        let Value::List(items) = value else {
            panic!("{expr} is not a list: {value:?}");
        };
        items
            .iter()
            .map(|item| match &*item.borrow() {
                Value::Number(n) => *n,
                other => panic!("not an index: {other:?}"),
            })
            .collect()
    }

    /// The three views' row orders, as track indices: the grid's and the
    /// mixer's navigation order, and the arrangement's rendered rows.
    fn view_orders(&mut self) -> [Vec<f64>; 3] {
        // Each track lane names its track index as `:lane-key` (a rack
        // lane's is negative); rows render top to bottom.
        let (tree, _) = self.buffer_tree("*arrangement*");
        let mut lanes = Vec::new();
        widgets_with_prop(&tree, "lane-key", &mut lanes);
        let mut arrangement = Vec::new();
        for lane in lanes {
            if let Some(Value::Number(n)) = lane.get("lane-key") {
                if *n >= 0.0 && !arrangement.contains(n) {
                    arrangement.push(*n);
                }
            }
        }
        [
            self.order_of("(eseq.drum-rack-v2/visible-track-order)"),
            self.order_of("(eseq.drum-rack-v2/mixer-visible-track-order)"),
            arrangement,
        ]
    }

    /// A drop event as the editor builds one: `drag-type`, its `payload` and
    /// the target's `:drop-meta`, all Lisp source.
    fn drop_event(&mut self, drag_type: &str, payload: &str, target: &str) -> Value {
        self.eval(&format!(
            "(dict :drag-type \"{drag_type}\" :payload {payload} :target {target})"
        ))
    }

    /// Fire the `:on-drop` of the drop target keyed `key` in `buffer` (a
    /// widget whose key ends with `key` and that has an `:on-drop`).
    fn drop_on(&mut self, buffer: &str, key: &str, event: Value) {
        let (tree, _) = self.buffer_tree(buffer);
        let mut droppable = Vec::new();
        widgets_with_prop(&tree, "on-drop", &mut droppable);
        let keys: Vec<String> = droppable
            .iter()
            .map(|w| format!("{:?}", w.get("key")))
            .collect();
        let widget = droppable
            .into_iter()
            .find(|w| matches!(w.get("key"), Some(Value::String(k)) if k.ends_with(key)))
            .unwrap_or_else(|| panic!("no drop target {key} in {buffer}: {keys:?}"));
        self.fire_and_show(&widget, "on-drop", event);
        self.share_buses_and_groups();
        self.sync();
        self.show_all();
    }
}

fn all_views(order: &[f64]) -> [Vec<f64>; 3] {
    [order.to_vec(), order.to_vec(), order.to_vec()]
}

#[test]
fn track_names_and_labels_are_drag_sources_and_rows_accept_the_drop() {
    let mut h = distro();
    h.pkg_tracks(3);
    h.show_all();
    let (grid, _) = h.buffer_tree("*sequencer*");
    let name = widget_keyed(&grid, "select-1").expect("grid name box");
    assert_eq!(name.get("drag-type"), Some(&s("track-badge")));
    let row = widget_keyed(&grid, "track-drop-2").expect("grid row");
    let Some(Value::List(types)) = row.get("drop-types") else {
        panic!("row drop types: {row:?}");
    };
    assert!(
        types.iter().any(|t| *t.borrow() == s("track-badge")),
        "a grid row accepts a dragged track: {types:?}"
    );
    let (mixer, _) = h.buffer_tree("*mixer*");
    let label = widget_keyed(&mixer, "track-label-1").expect("mixer label");
    assert_eq!(label.get("drag-type"), Some(&s("track-badge")));
}

#[test]
fn dropping_a_track_in_any_view_reorders_all_three_and_undoes() {
    let mut h = distro();
    h.pkg_tracks(4);
    h.show_all();
    assert_eq!(h.view_orders(), all_views(&[0.0, 1.0, 2.0, 3.0]));
    let names = h.app.tracks.clone();

    // Grid: track 0's name dropped on track 2's row (moving down: after it).
    let event = h.drop_event("track-badge", "(dict :track 0)", "(dict :kind \"track\" :track 2)");
    h.drop_on("*sequencer*", "track-drop-2", event);
    assert_eq!(h.view_orders(), all_views(&[1.0, 2.0, 0.0, 3.0]));
    assert_eq!(h.app.tracks, names, "indices and lanes stay put");
    assert_eq!(h.order_of("(eseq.kinds/ordered-tracks)"), vec![1.0, 2.0, 0.0, 3.0]);

    // Mixer: track 3's label dropped on track 1's strip (moving up: before).
    let event = h.drop_event("track-badge", "(dict :track 3)", "(dict :kind \"track\" :track 1)");
    h.drop_on("*mixer*", "mixer-strip-drop-1", event);
    assert_eq!(h.view_orders(), all_views(&[3.0, 1.0, 2.0, 0.0]));

    // Arrangement: track 1 dropped on track 0's header (moving down).
    let event = h.drop_event("track-badge", "(dict :track 1)", "(dict :kind \"track\" :track 0)");
    h.drop_on("*arrangement*", "track-header-0", event);
    assert_eq!(h.view_orders(), all_views(&[3.0, 2.0, 0.0, 1.0]));

    h.undo();
    h.sync();
    h.show_all();
    assert_eq!(h.view_orders(), all_views(&[3.0, 1.0, 2.0, 0.0]));
}

#[test]
fn tracks_move_into_within_and_out_of_groups_and_groups_move_as_blocks() {
    let mut h = distro();
    h.pkg_tracks(5);
    h.app.group_tracks_recorded(vec![1, 2]).expect("group");
    h.share_buses_and_groups();
    h.sync();
    h.show_all();
    let group = h.app.groups[0].id;
    let gidx = 0;
    assert_eq!(h.view_orders(), all_views(&[0.0, 1.0, 2.0, 3.0, 4.0]));

    // Onto a member: joins the group in that place.
    let event = h.drop_event("track-badge", "(dict :track 4)", "(dict :kind \"track\" :track 1)");
    h.drop_on("*sequencer*", "track-drop-1", event);
    assert!(h.app.groups[0].members.contains(&4), "joined the group");
    assert_eq!(h.view_orders(), all_views(&[0.0, 4.0, 1.0, 2.0, 3.0]));

    // Within the group.
    let event = h.drop_event("track-badge", "(dict :track 4)", "(dict :kind \"track\" :track 2)");
    h.drop_on("*sequencer*", "track-drop-2", event);
    assert_eq!(h.view_orders(), all_views(&[0.0, 1.0, 2.0, 4.0, 3.0]));

    // Onto a loose track: leaves the group.
    let event = h.drop_event("track-badge", "(dict :track 2)", "(dict :kind \"track\" :track 3)");
    h.drop_on("*sequencer*", "track-drop-3", event);
    assert!(!h.app.groups[0].members.contains(&2), "left the group");
    assert_eq!(h.view_orders(), all_views(&[0.0, 1.0, 4.0, 3.0, 2.0]));

    // Onto the group's header in the grid: joins it last.
    let event = h.drop_event(
        "track-badge",
        "(dict :track 0)",
        &format!("(dict :kind \"group\" :gidx {gidx})"),
    );
    h.drop_on("*sequencer*", &format!("group-header-drop-{group}"), event);
    assert!(h.app.groups[0].members.contains(&0), "joined via the header");
    assert_eq!(h.view_orders(), all_views(&[1.0, 4.0, 0.0, 3.0, 2.0]));

    // The group block dragged onto the last loose track moves past it.
    let event = h.drop_event(
        "track-group",
        &format!("(dict :gid {group})"),
        "(dict :kind \"track\" :track 2)",
    );
    h.drop_on("*sequencer*", "track-drop-2", event);
    assert_eq!(h.view_orders(), all_views(&[3.0, 2.0, 1.0, 4.0, 0.0]));
}

/// Whether some string prop of the widget keyed `key` in `buffer` (a
/// button's caption) is `text`.
fn shows(h: &Harness, buffer: &str, key: &str, text: &str) -> bool {
    let (tree, _) = h.buffer_tree(buffer);
    let mut keyed = Vec::new();
    widgets_with_prop(&tree, "key", &mut keyed);
    keyed
        .iter()
        .filter(|w| matches!(w.get("key"), Some(Value::String(k)) if k.ends_with(key)))
        .flat_map(|w| w.values())
        .any(|v| matches!(v, Value::String(s) if s == text))
}

#[test]
fn track_numbers_follow_the_display_order_in_labels_and_route_menus() {
    let mut h = distro();
    h.pkg_tracks(3);
    h.show_all();
    // Track 2 (index) dropped on track 0: it is first, so it reads "1".
    let event = h.drop_event("track-badge", "(dict :track 2)", "(dict :kind \"track\" :track 0)");
    h.drop_on("*sequencer*", "track-drop-0", event);
    assert_eq!(h.view_orders(), all_views(&[2.0, 0.0, 1.0]));
    for (index, number) in [(2, 1.0), (0, 2.0), (1, 3.0)] {
        assert_eq!(
            h.eval(&format!("(eseq.kinds/track-number (track {index}))")),
            Value::Number(number),
            "track {index}'s number",
        );
    }
    assert!(shows(&h, "*sequencer*", "mute-2", "1"), "the grid's mute button numbers by position");
    assert!(shows(&h, "*sequencer*", "mute-0", "2"));
    assert_eq!(h.app.track_number(2), 1, "host-built labels number alike");
    // A project graph's route menu: display order, display numbers, and a
    // pick still maps back to the track it names.
    h.eval("(import eseq.graph-kit)");
    let menu = h.eval(
        "(let ((g (dict :owner nil :nodes (list))))
           (map (lambda (t) (eseq.graph-kit/route-label g t))
             (eseq.scene-banks/bank-tracks (eseq.scene-banks/scene-viewed-bank) (list))))",
    );
    assert_eq!(menu, h.eval("(list \"Track 1\" \"Track 2\" \"Track 3\")"));
    assert_eq!(
        h.eval("(eseq.graph-kit/route-label (dict :owner nil) (track 2))"),
        s("Track 1"),
    );
}

