//! Project-owned sequencers on/off per bank in the factory UI
//! (docs/rig-spec.md §10, eseq-ka5t.6): a new instance is on only in the
//! viewed bank, its view tab carries the bank-scoped power toggle (dot, hint,
//! mixed dot), the Packages tab's instance menu turns it off/on in the bank,
//! recalls another bank's settings, and purges unused ones.

use super::views::{distro, widget_keyed, widgets_with_prop};
use super::*;

const JAKI: &str = "alez/jaki:jaki";

/// Apply what Lisp queued, sync and render until it settles.
fn settle(h: &mut Harness) -> Vec<String> {
    let mut names = Vec::new();
    for _ in 0..4 {
        h.editor.runtime_mut().run_reactive_cycle();
        h.editor.refresh_runtime_side_effects();
        for (name, payload) in h.custom_commands() {
            h.command(&name, payload.clone());
            names.push(name);
        }
        h.share_buses_and_groups();
        h.sync();
        h.show_all();
    }
    names
}

/// Four scenes: bank A = {0, 1}, bank B = {2, 3}; scene 0 plays.
fn two_banks(h: &mut Harness) {
    let names = h.app.tracks.clone();
    for _ in 0..3 {
        let bank = h.app.state.scene_bank_containing(h.app.state.current_scene_index()).unwrap();
        h.app
            .state
            .clone_pattern_in_scene_bank(
                bank,
                sequencer::sequencer::SceneSwapInheritance::Source(0),
                names.len(),
                &h.app.graph.track_buffer_ids.clone(),
                &h.app.graph.track_sample_rates.clone(),
                &names,
                &h.app.graph.track_instrument_types.clone(),
            )
            .expect("a scene");
    }
    let bank_b = h.app.state.create_scene_bank().unwrap();
    for _ in 0..2 {
        h.app.state.move_scene_to_scene_bank(2, bank_b).unwrap();
    }
    h.app.state.with_scenes_mut(|scenes| scenes.current_scene = 0);
    settle(h);
}

/// The view buffer of instance `id`.
fn view_buffer(h: &Harness, id: u64) -> String {
    h.editor
        .runtime()
        .bound_view_buffers()
        .into_iter()
        .find_map(|(target, view)| match view {
            eseqlisp::vm::BoundView::Instance(bound) if bound == id => Some(target),
            _ => None,
        })
        .expect("the instance's view buffer")
}

fn debug_named(tree: &Value, name: &str) -> Vec<HashMap<String, Value>> {
    let mut named = Vec::new();
    widgets_with_prop(tree, "debug-name", &mut named);
    named
        .into_iter()
        .filter(|w| matches!(&w["debug-name"], Value::String(n) if n == name))
        .collect()
}

fn parked(h: &Harness, id: u64) -> Vec<bool> {
    h.app.state.with_project_scenes(|scenes| {
        (0..scenes.scenes.len()).map(|scene| scenes.instance_parked_in_scene(scene, id)).collect()
    })
}

fn menu_labels(h: &mut Harness, id: u64) -> Vec<String> {
    let value = h.eval(&format!(
        "(map (lambda (a) (get a :label)) (eseq.browser/instance-bank-actions (dict :instance-id {id})))"
    ));
    let Value::List(items) = value else { panic!("a list: {value:?}") };
    items
        .iter()
        .map(|item| match &*item.borrow() {
            Value::String(label) => label.to_string(),
            other => panic!("a label: {other:?}"),
        })
        .collect()
}

#[test]
fn a_project_sequencers_view_has_a_bank_power_toggle_and_the_packages_menu_switches_it() {
    let mut h = distro();
    h.eval("(import alez.jaki.kind)");
    two_banks(&mut h);
    let before = crate::host_commands::instances::instance_ids(&h.app);
    h.command("instance-create", map_value(vec![("kind", s(JAKI))]));
    let id = *crate::host_commands::instances::instance_ids(&h.app)
        .difference(&before)
        .next()
        .expect("an instance");
    settle(&mut h);
    assert_eq!(parked(&h, id), vec![false, false, true, true], "made in the viewed bank A");

    // The view tab: power dot on, hint, no mixed dot.
    let buffer = view_buffer(&h, id);
    let (tree, _) = h.buffer_tree(&buffer);
    assert_eq!(debug_named(&tree, "instance-power").len(), 1, "the power strip: {tree:?}");
    let dot = widget_keyed(&tree, "instance-power-dot").expect("the power dot");
    assert_eq!(dot.get("active"), Some(&Value::Number(1.0)));
    let hint = &debug_named(&tree, "instance-power-hint")[0];
    assert!(format!("{hint:?}").contains("On in bank A"), "{hint:?}");
    assert!(debug_named(&tree, "bank-power-mixed").is_empty());
    assert_eq!(menu_labels(&mut h, id), vec!["Turn off in bank A".to_string()]);

    // The toggle turns bank A off (one command, one undo step): now unused.
    h.eval(&format!("(eseq.instance-frame/toggle-instance-in-viewed-bank! {id})"));
    assert_eq!(settle(&mut h), vec!["set-instance-parked".to_string()]);
    assert_eq!(parked(&h, id), vec![true; 4]);
    let (tree, _) = h.buffer_tree(&buffer);
    let dot = widget_keyed(&tree, "instance-power-dot").expect("the power dot");
    assert_eq!(dot.get("active"), Some(&Value::Number(0.0)), "the toggle reads off");
    let hint = &debug_named(&tree, "instance-power-hint")[0];
    assert!(format!("{hint:?}").contains("Off in bank A"), "{hint:?}");
    assert_eq!(
        menu_labels(&mut h, id),
        vec!["Turn on in bank A".to_string(), "Purge unused sequencers...".to_string()]
    );
    assert_eq!(h.eval("(len (eseq.kinds/unused-instances))"), Value::Number(1.0));

    // A mixed bank: on with the small dot.
    h.app.state.with_scenes_mut(|scenes| scenes.set_parked_in_scenes(sequencer::sequencer::ParkedSet::Instances, id, false, 1..2));
    settle(&mut h);
    let (tree, _) = h.buffer_tree(&buffer);
    assert_eq!(debug_named(&tree, "bank-power-mixed").len(), 1, "the mixed dot");
    assert_eq!(
        h.eval(&format!("(eseq.bank-power/instance-bank-state {id} (first (eseq.kinds/banks)))")),
        Value::Keyword("mixed".into())
    );

    // Bank B: "Turn on in bank B like bank A" copies bank A's settings.
    h.eval("(eseq.scene-banks/view-scene-bank! (nth (eseq.kinds/banks) 1))");
    settle(&mut h);
    let labels = menu_labels(&mut h, id);
    assert!(labels.contains(&"Turn on in bank B like bank A".to_string()), "{labels:?}");
    h.eval(&format!(
        "(eseq.kinds/activate-instance-like-bank! {id} (eseq.scene-banks/scene-viewed-bank) (first (eseq.kinds/banks)))"
    ));
    assert_eq!(settle(&mut h), vec!["instance-activate".to_string()]);
    assert_eq!(parked(&h, id), vec![true, false, false, false]);

    // Purge asks first (naming the instance); confirmed, it deletes.
    h.app.state.with_scenes_mut(|scenes| scenes.set_parked_in_scenes(sequencer::sequencer::ParkedSet::Instances, id, true, 0..4));
    let label = h.app.instances.get(id).unwrap().label.clone();
    assert!(crate::host_commands::instances::purge_confirm_message(&[label.clone()]).contains(&label));
    h.command("instance-purge-unused", map_value(vec![("confirmed", Value::Bool(true))]));
    assert!(h.app.instances.get(id).is_none(), "purged");
    assert_eq!(parked(&h, id), vec![false; 4], "its parking went with it");
}
