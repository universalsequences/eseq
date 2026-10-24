//! Kits on/off per bank in the factory UI (docs/rig-spec.md §6,
//! eseq-ka5t.13): the group kind's parked fields, the drum rack panel's
//! power toggle in *fx*, the mixer's group menu and the dimmed strip a
//! parked kit keeps in the bank's mixer.

use super::views::{distro, widget_keyed, widgets_with_prop};
use super::*;
use sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE;

const REFER_KITS: &str = "(import eseq.kinds :refer (groups banks set-kit-parked!)) (import eseq.bank-power :refer (kit-bank-state))";

impl Harness {
    fn kit_eval(&mut self, code: &str) -> Value {
        self.eval_with(REFER_KITS, code)
    }

    /// Run `code`, apply what it queued, sync and render. Returns the
    /// commands' names.
    fn kit_turn(&mut self, code: &str) -> Vec<String> {
        self.kit_settle();
        self.kit_eval(code);
        self.kit_settle()
    }

    fn kit_settle(&mut self) -> Vec<String> {
        let mut names = Vec::new();
        for _ in 0..4 {
            let commands = self.custom_commands();
            if commands.is_empty() {
                break;
            }
            for (name, payload) in commands {
                self.command(&name, payload);
                names.push(name);
            }
            self.share_buses_and_groups();
            self.sync();
            self.show_all();
        }
        self.share_buses_and_groups();
        self.sync();
        self.show_all();
        names
    }

    /// The widgets of buffer `buffer` with debug name `debug`.
    fn kit_widgets(&self, buffer: &str, debug: &str) -> Vec<HashMap<String, Value>> {
        let (tree, _) = self.buffer_tree(buffer);
        let mut named = Vec::new();
        widgets_with_prop(&tree, "debug-name", &mut named);
        named
            .into_iter()
            .filter(|w| matches!(&w["debug-name"], Value::String(name) if name == debug))
            .collect()
    }
}

fn kw(name: &str) -> Value {
    Value::Keyword(name.into())
}

/// A distro project whose two tracks are the pads of one clip-bearing kit.
fn kit_harness() -> (Harness, u64) {
    let mut h = distro();
    let (group_id, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    for (track, offset) in [(0, 0), (1, 6)] {
        h.app
            .assign_rack_pad_track_recorded(group_id, DRUM_RACK_FIRST_PAD_NOTE + offset, track)
            .expect("pad");
        h.app.state.pattern.patterns[track].set_step_active(0, true);
    }
    h.app.convert_rack_to_clips_recorded(group_id).expect("clips");
    h.share_buses_and_groups();
    h.sync();
    h.show_all();
    h.kit_eval("(def g (first (filter (lambda (g) g.rack) (groups)))) (def b (first (banks)))");
    (h, group_id)
}

#[test]
fn a_parked_kit_reads_off_in_its_bank_and_keeps_a_dimmed_strip_in_the_mixer() {
    let (mut h, group_id) = kit_harness();
    assert_eq!(h.kit_eval("(kit-bank-state g b)"), kw("on"));
    assert_eq!(h.kit_eval("g.parked"), Value::Bool(false));
    assert!(h.kit_widgets("*mixer*", "parked-kit-strip").is_empty(), "an on kit is a live strip");
    let menu = h.kit_eval("(get (eseq.mixer/kit-power-action g) :label)");
    assert_eq!(menu, Value::String("Turn kit off in bank A".into()));

    let sent = h.kit_turn("(set-kit-parked! g b true)");
    assert_eq!(sent, vec!["set-kit-parked".to_string()]);
    assert!(h.app.state.with_project_scenes(|scenes| scenes.kit_parked_in_every_scene(group_id)));
    assert_eq!(h.kit_eval("(kit-bank-state g b)"), kw("off"));
    assert_eq!(h.kit_eval("g.parked"), Value::Bool(true));
    assert_eq!(h.kit_eval("(len g.parked-banks)"), Value::Number(1.0));
    assert_eq!(h.kit_eval("g.unused"), Value::Bool(true), "the only bank parks it");
    assert_eq!(h.kit_eval("(eseq.drum-rack-v2/kit-off-in-viewed-bank? g)"), Value::Bool(true));

    // The mixer still lists the kit, as a dimmed strip whose menu turns it
    // back on.
    let strips = h.kit_widgets("*mixer*", "parked-kit-strip");
    assert_eq!(strips.len(), 1, "the parked kit's strip is in the bank's mixer");
    let menu = h.kit_eval("(get (eseq.mixer/kit-power-action g) :label)");
    assert_eq!(menu, Value::String("Turn kit on in bank A".into()));

    // Clicking it selects the kit: *fx* shows its drum rack, toggle off.
    h.fire_and_show(&strips[0], "on-click", Value::Nil);
    let power = h.kit_widgets("*fx*", "bank-power");
    assert_eq!(power.len(), 1, "the drum rack panel is showing");
    let (fx, _) = h.buffer_tree("*fx*");
    let dot = widget_keyed(&fx, &format!("rack-kit-power-{group_id}")).expect("the power dot");
    assert_eq!(dot.get("active"), Some(&Value::Number(0.0)), "the toggle reads off");
    let hint = h.kit_eval("(get (eseq.drum-rack-v2/kit-viewed-header g) :hint)");
    assert_eq!(hint, Value::String("Kit off in bank A".into()));

    // The toggle on an off bank turns it on.
    let sent = h.kit_turn("(eseq.drum-rack-v2/toggle-kit-in-viewed-bank! g)");
    assert_eq!(sent, vec!["set-kit-parked".to_string()]);
    assert_eq!(h.kit_eval("(kit-bank-state g b)"), kw("on"));
    assert!(h.kit_widgets("*mixer*", "parked-kit-strip").is_empty());
}

#[test]
fn the_toggle_on_a_mixed_bank_turns_every_scene_off() {
    let (mut h, group_id) = kit_harness();
    let names = h.app.tracks.clone();
    h.app.state.clone_pattern(
        names.len(),
        &h.app.graph.track_buffer_ids.clone(),
        &h.app.graph.track_sample_rates.clone(),
        &names,
        &h.app.graph.track_instrument_types.clone(),
    );
    h.app.state.with_scenes_mut(|scenes| scenes.set_kit_parked_in_scenes(group_id, true, 0..1));
    h.sync();
    h.show_all();
    assert_eq!(h.kit_eval("(kit-bank-state g b)"), kw("mixed"));
    assert_eq!(
        h.kit_eval("(get (eseq.drum-rack-v2/kit-viewed-header g) :hint)"),
        Value::String("Kit on in bank A (some scenes off)".into())
    );
    let sent = h.kit_turn("(eseq.drum-rack-v2/toggle-kit-in-viewed-bank! g)");
    assert_eq!(sent, vec!["set-kit-parked".to_string()]);
    assert_eq!(h.kit_eval("(kit-bank-state g b)"), kw("off"));
}

#[test]
fn a_clip_launch_on_a_parked_kit_is_refused_but_silencing_is_allowed() {
    let (mut h, group_id) = kit_harness();
    let verse = h.app.current_rack_clip(group_id).expect("the scene's clip");
    let chorus = h.app.save_rack_clip_as_recorded(group_id, "Chorus").expect("a second clip");
    assert_eq!(h.app.current_rack_clip(group_id), Some(chorus));
    h.kit_turn("(set-kit-parked! g b true)");
    let undo_len = h.app.history.undo_len();
    // Through Lisp (the clip grid's launch) and through the App funnel
    // itself (agent and script callers).
    h.kit_turn(&format!(
        "(eseq.kinds/launch-rack-clip! (first (filter (lambda (rc) (= rc.cid {verse})) g.clips)))"
    ));
    assert_eq!(h.app.current_rack_clip(group_id), Some(chorus), "the pointer stays");
    assert_eq!(h.app.history.undo_len(), undo_len, "nothing recorded");
    assert_eq!(
        h.app.set_current_rack_clip_recorded(group_id, Some(verse)),
        Err("Kit is off in bank A".to_string())
    );
    // Silencing is a pointer edit, allowed while parked.
    h.app.set_current_rack_clip_recorded(group_id, None).expect("silence");
    assert_eq!(h.app.current_rack_clip(group_id), None);
}
