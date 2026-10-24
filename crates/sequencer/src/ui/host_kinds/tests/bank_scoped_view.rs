//! Bank-scoped display in the factory UI (docs/rig-spec.md §9,
//! eseq-ka5t.10): `track.used-banks` / `group.used-banks`, and the
//! sequencer list, the mixer and the route menus hiding what the viewed bank
//! does not use, with "Show all tracks" overriding it.

use super::views::{distro, widget_keyed};
use super::*;
use sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE;

const REFER_BANKS: &str = "(import eseq.kinds :refer (track tracks groups banks scenes selection set-kit-parked!))
(import eseq.scene-banks :refer (view-scene-bank! used-in-viewed-bank? show-all-tracks))
(import eseq.graph-kit)";

impl Harness {
    fn bank_eval(&mut self, code: &str) -> Value {
        self.eval_with(REFER_BANKS, code)
    }

    /// Apply what Lisp queued, sync and render, until nothing is queued.
    fn bank_settle(&mut self) {
        for _ in 0..4 {
            let commands = self.custom_commands();
            if commands.is_empty() {
                break;
            }
            for (name, payload) in commands {
                self.command(&name, payload);
            }
            self.share_buses_and_groups();
            self.sync();
            self.show_all();
        }
        self.share_buses_and_groups();
        self.sync();
        self.show_all();
    }

    /// The bank labels in `t.used-banks` of the track at `index`.
    fn used_banks(&mut self, index: usize) -> Vec<String> {
        strings(&self.bank_eval(&format!("(let ((t (track {index}))) (map (lambda (b) b.label) t.used-banks))")))
    }

    fn group_used_banks(&mut self) -> Vec<String> {
        strings(&self.bank_eval(
            "(let ((g (first (filter (lambda (g) g.rack) (groups))))) (map (lambda (b) b.label) g.used-banks))",
        ))
    }

    fn has_widget(&self, buffer: &str, suffix: &str) -> bool {
        let (tree, _) = self.buffer_tree(buffer);
        widget_keyed(&tree, suffix).is_some()
    }

    fn passes(&self) -> u64 {
        self.frame.host_kinds.bank_usage_passes()
    }
}

/// A distro project with scenes 1 (bank A) and 2 (bank B), scene 2 playing:
/// the last track has a cell only in scene 2, so only bank B uses it.
/// Returns the harness and that track.
fn two_bank_harness() -> (Harness, usize) {
    let mut h = distro();
    h.command("clone-pattern", Value::Nil);
    h.command("create-scene-bank", Value::Nil);
    let bank_b = h.app.state.scene_banks()[1].id;
    h.app.state.move_scene_to_scene_bank(1, bank_b).expect("move");
    h.bank_eval("(host-command \"switch-pattern\" (dict :idx 1 :quantize \"off\"))");
    h.drain();
    assert_eq!(h.app.state.current_scene_index(), 1);
    let only_b = h.app.tracks.len() - 1;
    h.app.state.with_scenes_mut(|scenes| scenes.scenes[0].cells[only_b] = None);
    h.sync();
    h.show_all();
    h.bank_eval("(def bank-a (first (banks))) (def bank-b (nth (banks) 1))");
    (h, only_b)
}

#[test]
fn used_banks_name_the_banks_whose_scenes_give_a_track_a_cell() {
    let (mut h, only_b) = two_bank_harness();
    assert_eq!(h.used_banks(0), vec!["A", "B"]);
    assert_eq!(h.used_banks(only_b), vec!["B"], "only bank B's scene gives it a cell");

    // Nothing changed: no recompute.
    let passes = h.passes();
    h.sync();
    h.sync();
    assert_eq!(h.passes(), passes, "an idle sync does not recompute");

    // A cell edit (bank B's scene drops it too) recomputes.
    h.app.state.with_scenes_mut(|scenes| scenes.scenes[1].cells[only_b] = None);
    h.sync();
    assert!(h.passes() > passes);
    assert!(h.used_banks(only_b).is_empty(), "no scene uses it");

    // Moving the scene that gives track 0 a cell is a bank edit.
    h.app.state.with_scenes_mut(|scenes| scenes.scenes[0].cells[0] = None);
    h.sync();
    assert_eq!(h.used_banks(0), vec!["B"]);
    let bank_a = h.app.state.scene_banks()[0].id;
    h.app.state.move_scene_to_scene_bank(1, bank_a).expect("move back");
    h.sync();
    assert_eq!(h.used_banks(0), vec!["A"], "the scene moved into bank A");
}

#[test]
fn a_parked_kits_pads_are_unused_in_its_parked_bank() {
    let (mut h, _) = two_bank_harness();
    let (group_id, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    for (track, offset) in [(0, 0), (1, 6)] {
        h.app
            .assign_rack_pad_track_recorded(group_id, DRUM_RACK_FIRST_PAD_NOTE + offset, track)
            .expect("pad");
        h.app.state.pattern.patterns[track].set_step_active(0, true);
    }
    h.app.convert_rack_to_clips_recorded(group_id).expect("clips");
    // Both scenes point at the kit's clip.
    let clip = h.app.current_rack_clip(group_id).expect("the playing scene's clip");
    h.app.state.with_scenes_mut(|scenes| scenes.set_scene_rack_clip(0, group_id, Some(clip)));
    h.share_buses_and_groups();
    h.sync();
    h.show_all();
    assert_eq!(h.used_banks(0), vec!["A", "B"]);
    assert_eq!(h.group_used_banks(), vec!["A", "B"]);

    let passes = h.passes();
    h.bank_eval("(set-kit-parked! (first (filter (lambda (g) g.rack) (groups))) bank-a true)");
    h.bank_settle();
    assert!(h.passes() > passes, "parking recomputes");
    assert_eq!(h.used_banks(0), vec!["B"], "a pad of a kit bank A parks");
    assert_eq!(h.used_banks(1), vec!["B"]);
    assert_eq!(h.group_used_banks(), vec!["B"]);

    // Viewed in bank A, the pads hide from the sequencer and the mixer; the
    // kit stays as its dimmed strip.
    h.bank_eval("(view-scene-bank! bank-a)");
    h.show_all();
    assert_eq!(h.bank_eval("(used-in-viewed-bank? (track 0))"), Value::Bool(false));
    assert!(!h.has_widget("*sequencer*", "track-drop-0"), "the pad's row hides");
    assert!(!h.has_widget("*mixer*", "track-label-0"), "the pad's strip hides");
    assert!(h.has_widget("*mixer*", &format!("parked-kit-strip-{group_id}")));
    h.bank_eval("(view-scene-bank! bank-b)");
    h.show_all();
    assert!(h.has_widget("*sequencer*", "track-drop-0"));
}

#[test]
fn the_viewed_bank_hides_a_track_it_does_not_use_until_show_all() {
    let (mut h, only_b) = two_bank_harness();
    let row = format!("track-drop-{only_b}");
    let strip = format!("track-label-{only_b}");
    // The strip keeps the bank it showed first (bank A); view bank B.
    h.bank_eval("(view-scene-bank! bank-b)");
    h.show_all();
    assert!(h.has_widget("*sequencer*", &row), "bank B uses it");
    assert!(h.has_widget("*mixer*", &strip));

    h.bank_eval("(view-scene-bank! bank-a)");
    h.show_all();
    assert!(!h.has_widget("*sequencer*", &row), "bank A's sequencer hides it");
    assert!(!h.has_widget("*mixer*", &strip), "bank A's mixer hides it");
    assert!(h.has_widget("*sequencer*", "track-drop-0"), "track 0 stays");
    assert!(h.has_widget("*mixer*", "track-label-0"));
    // Nothing renumbers: the other tracks keep their keys and indices.
    assert_eq!(h.bank_eval(&format!("(let ((t (track {only_b}))) t.index)")), Value::Number(only_b as f64));
    // UP / DOWN skip it.
    let order = h.bank_eval("(map (lambda (t) t.index) (eseq.drum-rack-v2/visible-track-order))");
    assert!(!items(&order).contains(&Value::Number(only_b as f64)));

    // The route menus (a lane's track inlet, a graph's routes) omit it.
    let menu = strings(&h.bank_eval("(eseq.sequencer/lane-track-menu 0)"));
    assert!(menu.iter().all(|label| !label.starts_with(&format!("{} ", only_b + 1))), "{menu:?}");
    assert_eq!(menu.len(), only_b, "every other track");
    let routes = h.bank_eval(&format!(
        "(let ((g (dict :owner nil :nodes (list)))) (eseq.view-kit/listed? (track {only_b}) (eseq.graph-kit/route-menu-tracks g)))"
    ));
    assert_eq!(routes, Value::Bool(false));
    // A graph menu keeps the track a node routes to, and one it is told to.
    let routed = h.bank_eval(&format!(
        "(let ((n (dict :route (track {only_b}))) (g (dict :owner nil :nodes (list n))))
           (eseq.view-kit/listed? (track {only_b}) (eseq.graph-kit/route-menu-tracks g)))"
    ));
    assert_eq!(routed, Value::Bool(true), "a node's route stays listed");
    let kept = h.bank_eval(&format!(
        "(let ((g (dict :owner nil :nodes (list))))
           (eseq.view-kit/listed? (track {only_b}) (eseq.graph-kit/route-menu-tracks g :keep (list (track {only_b})))))"
    ));
    assert_eq!(kept, Value::Bool(true));
    // A menu still shows the track an inlet names now.
    let named = strings(&h.bank_eval(&format!("(eseq.sequencer/lane-track-menu {only_b})")));
    assert_eq!(named.len(), only_b + 1);

    // Show all tracks reveals it.
    h.bank_eval("(eseq.scene-banks/toggle-show-all-tracks!)");
    h.show_all();
    assert!(h.has_widget("*sequencer*", &row));
    assert!(h.has_widget("*mixer*", &strip));
    let menu = strings(&h.bank_eval("(eseq.sequencer/lane-track-menu 0)"));
    assert_eq!(menu.len(), only_b + 1);
    h.bank_eval("(eseq.scene-banks/toggle-show-all-tracks!)");
    h.show_all();
    assert!(!h.has_widget("*sequencer*", &row));
}

#[test]
fn a_selected_track_the_viewed_bank_hides_hands_the_selection_to_the_nearest() {
    let (mut h, only_b) = two_bank_harness();
    h.bank_eval("(view-scene-bank! bank-b)");
    h.bank_settle();
    h.bank_eval(&format!("(eseq.sequencer/select-track-for-edit (track {only_b}))"));
    h.bank_settle();
    assert_eq!(h.bank_eval("(let ((t selection.track)) t.index)"), Value::Number(only_b as f64));

    h.bank_eval("(view-scene-bank! bank-a)");
    h.bank_settle();
    assert_eq!(
        h.bank_eval("(let ((t selection.track)) t.index)"),
        Value::Number((only_b - 1) as f64),
        "the nearest track bank A uses"
    );
    assert_eq!(h.bank_eval("(used-in-viewed-bank? selection.track)"), Value::Bool(true));
}

fn selected(h: &mut Harness) -> f64 {
    match h.bank_eval("(let ((t selection.track)) t.index)") {
        Value::Number(n) => n,
        other => panic!("no selection: {other:?}"),
    }
}

#[test]
fn a_selection_is_rehomed_only_when_the_view_changes_and_never_off_a_bus() {
    let (mut h, only_b) = two_bank_harness();
    // Viewing bank A, a view showing every track selects the B-only track:
    // it stays selected (nothing about the view changed).
    h.bank_eval("(view-scene-bank! bank-a)");
    h.bank_settle();
    h.bank_eval(&format!("(eseq.sequencer/select-track-for-edit (track {only_b}))"));
    h.bank_settle();
    assert_eq!(selected(&mut h), only_b as f64, "not yanked back");

    // A selected bus keeps its selection through a bank switch.
    h.bank_eval("(view-scene-bank! bank-b)");
    h.bank_settle();
    h.bank_eval("(set! eseq.seq-core-state/selected-bus 0)");
    h.bank_eval("(view-scene-bank! bank-a)");
    h.bank_settle();
    assert_eq!(h.bank_eval("eseq.seq-core-state/selected-bus"), Value::Number(0.0));
    assert_eq!(selected(&mut h), only_b as f64);

    // Toggling show-all off (a view change) rehomes it, and leaves no
    // hidden track in the multi-selection or the range anchor.
    h.bank_eval("(set! eseq.seq-core-state/selected-bus -1)");
    h.bank_eval("(eseq.scene-banks/toggle-show-all-tracks!)");
    h.bank_settle();
    h.bank_eval(&format!("(eseq.sequencer/select-track-for-edit (track {only_b}))"));
    h.bank_eval(&format!("(seq-select-tracks (list 0 {only_b}) {only_b})"));
    h.bank_settle();
    let anchor = h.bank_eval("(let ((g eseq.sequencer/grid-select) (a g.anchor)) a.index)");
    assert_eq!(anchor, Value::Number(only_b as f64), "the anchor sits on the hidden track");
    h.bank_eval("(eseq.scene-banks/toggle-show-all-tracks!)");
    h.bank_settle();
    assert_eq!(selected(&mut h), 0.0, "the nearest track bank A uses");
    let in_selection = h.bank_eval("(len (filter (lambda (t) t.in-selection) (tracks)))");
    assert_eq!(in_selection, Value::Number(1.0), "the hidden track left the multi-selection");
    let anchor = h.bank_eval("(let ((g eseq.sequencer/grid-select) (a g.anchor)) a.index)");
    assert_eq!(anchor, Value::Number(0.0));
}
