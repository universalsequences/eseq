//! The glances registry (docs/rig-spec.md §7, eseq-ka5t.15): `eseq.glance`
//! maps a sequencer kind to its small live view, the neural and jaki kinds
//! register theirs, and the drum rack panel in *fx* shows the glances of
//! the kit's sequencers beside its pads.

use super::views::{distro, widget_keyed, widgets_with_prop};
use sequencer::graph::{GraphVisualizationEvent, GraphVisualizationSnapshot};
use super::*;
use sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE;

const NEURAL: &str = "alez/neural:neural";
const JAKI: &str = "alez/jaki:jaki";

/// Create an instance of `kind`, owned by rack `group` when given.
fn create(h: &mut Harness, kind: &str, group: Option<u64>) -> u64 {
    let before = crate::host_commands::instances::instance_ids(&h.app);
    let mut payload = vec![("kind", s(kind))];
    if let Some(group) = group {
        payload.push(("group-id", Value::Number(group as f64)));
    }
    h.command("instance-create", map_value(payload));
    let created = crate::host_commands::instances::instance_ids(&h.app);
    let id = *created.difference(&before).next().expect("an instance");
    settle(h);
    id
}

/// Apply what Lisp queued, sync and render until it settles.
fn settle(h: &mut Harness) {
    for _ in 0..4 {
        h.editor.runtime_mut().run_reactive_cycle();
        h.editor.refresh_runtime_side_effects();
        for (name, payload) in h.custom_commands() {
            h.command(&name, payload);
        }
        h.share_buses_and_groups();
        h.sync();
        h.show_all();
    }
}

/// The debug names of `tree`'s widgets.
fn debug_names(tree: &Value) -> Vec<String> {
    let mut named = Vec::new();
    widgets_with_prop(tree, "debug-name", &mut named);
    named
        .into_iter()
        .filter_map(|w| match &w["debug-name"] {
            Value::String(name) => Some(name.to_string()),
            _ => None,
        })
        .collect()
}

/// The keys of `tree`'s widgets.
fn keys(tree: &Value) -> Vec<String> {
    let mut keyed = Vec::new();
    widgets_with_prop(tree, "key", &mut keyed);
    keyed
        .into_iter()
        .filter_map(|w| match &w["key"] {
            Value::String(key) => Some(key.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn glance_for_returns_the_kinds_renderer_or_the_fallback() {
    let mut h = distro();
    h.eval("(import alez.neural.variable-reset) (import alez.jaki.kind) (import eseq.glance)");
    let neural = create(&mut h, NEURAL, None);
    let jaki = create(&mut h, JAKI, None);

    // The packages registered theirs by kind id; an instance or its id
    // answers; a kind nobody registered has none, a dead id no glance.
    assert_eq!(
        h.eval(&format!(
            "(list (eseq.glance/has-glance? {neural}) (eseq.glance/has-glance? (instance-ref {jaki}))
                   (if (eseq.glance/glance-renderer \"{NEURAL}\") true false)
                   (if (eseq.glance/glance-renderer \"{JAKI}\") true false)
                   (eseq.glance/glance-renderer \"nobody/else:density\")
                   (eseq.glance/has-glance? 987654) (eseq.glance/glance-for 987654 nil))"
        )),
        h.eval("(list true true true true nil false nil)")
    );

    // The neural glance: its heatmap over trigger and energy strips, keyed
    // by the caller's prefix and the instance.
    let glance = h.eval(&format!(
        "(eseq.glance/glance-for {neural} (dict :height 5 :key \"t\"))"
    ));
    assert!(debug_names(&glance).contains(&"neural-glance".to_string()), "{glance:?}");
    for suffix in ["ev", "tr", "en"] {
        let key = format!("t-gvr-{neural}-{suffix}");
        assert!(keys(&glance).contains(&key), "{key} in {:?}", keys(&glance));
    }
    // The jaki glance: its hits strip.
    let glance = h.eval(&format!("(eseq.glance/glance-for {jaki} nil)"));
    assert!(debug_names(&glance).contains(&"jaki-glance".to_string()), "{glance:?}");

    // A bare kind name matches any package's kind of that name, but the
    // exact id still wins.
    h.eval("(eseq.glance/def-glance \"neural\" (lambda (self hint) (label \"bare\")))");
    let glance = h.eval(&format!("(eseq.glance/glance-for {neural} nil)"));
    assert!(debug_names(&glance).contains(&"neural-glance".to_string()));
    h.eval(&format!("(eseq.glance/def-glance \"{NEURAL}\" nil)"));
    let glance = h.eval(&format!("(eseq.glance/glance-for {neural} nil)"));
    assert!(!debug_names(&glance).contains(&"neural-glance".to_string()), "the bare one: {glance:?}");

    // Unregistered, a kind falls back to its icon and name; the card is
    // then the fallback alone.
    h.eval(&format!("(eseq.glance/def-glance \"{JAKI}\" nil)"));
    assert_eq!(h.eval(&format!("(eseq.glance/has-glance? {jaki})")), Value::Bool(false));
    for form in ["glance-for", "glance-card"] {
        let glance = h.eval(&format!("(eseq.glance/{form} {jaki} nil)"));
        assert_eq!(debug_names(&glance), vec!["glance-fallback".to_string()], "{form}: {glance:?}");
    }
    let Value::String(label) = h.eval(&format!("(let ((i (instance-ref {jaki}))) i.label)")) else {
        panic!("a label");
    };
    let glance = h.eval(&format!("(eseq.glance/glance-for {jaki} nil)"));
    assert!(format!("{glance:?}").contains(&label.to_string()), "the fallback names {label}: {glance:?}");
}

/// A distro project whose two tracks are the pads of one kit.
fn kit_harness() -> (Harness, u64) {
    let mut h = distro();
    let (group_id, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    for (track, offset) in [(0, 0), (1, 6)] {
        h.app
            .assign_rack_pad_track_recorded(group_id, DRUM_RACK_FIRST_PAD_NOTE + offset, track)
            .expect("pad");
    }
    h.share_buses_and_groups();
    settle(&mut h);
    (h, group_id)
}

/// The *fx* buffer alone in the window, its layout.
fn fx_layout(h: &mut Harness) -> std::sync::Arc<eseqlisp::layout::LayoutNode> {
    h.eval("(set-layout (list :buf \"*fx*\" :hide-status true))");
    h.editor.set_layout_viewport(240, 60);
    let id = (h.editor.buffers.iter())
        .find(|item| item.name == "*fx*")
        .expect("*fx*")
        .id;
    h.editor.set_active_buffer(id);
    settle(h);
    h.editor.widget_layout().expect("layout")
}

#[test]
fn the_drum_rack_panel_shows_the_glances_of_the_kits_sequencers() {
    let (mut h, group_id) = kit_harness();
    h.eval("(import alez.neural.variable-reset) (import alez.jaki.kind)");
    h.eval(&format!(
        "(eseq.sequencer/show-fx-for-group (first (filter (lambda (g) (= g.gid {group_id})) (eseq.kinds/groups))))"
    ));
    settle(&mut h);
    let layout = fx_layout(&mut h);
    let pads = find_debug(&layout, "rack-fx-pads-panel").expect("the drum rack panel");
    assert_laid_out(pads, "pads panel");
    assert!(find_debug(&layout, "rack-fx-sequencers-panel").is_none(), "no sequencer, no panel");

    // A project-owned instance is not the kit's.
    create(&mut h, NEURAL, None);
    let layout = fx_layout(&mut h);
    assert!(find_debug(&layout, "rack-fx-sequencers-panel").is_none(), "not the kit's");

    let owned = create(&mut h, NEURAL, Some(group_id));
    let layout = fx_layout(&mut h);
    let pads = find_debug(&layout, "rack-fx-pads-panel").expect("the drum rack panel");
    assert_laid_out(pads, "pads panel");
    let panel = find_debug(&layout, "rack-fx-sequencers-panel").expect("the sequencers panel");
    assert_laid_out(panel, "sequencers panel");
    assert!(panel.rect.col >= pads.rect.col + pads.rect.width, "beside the pads");
    let glance = find_debug(panel, "neural-glance").expect("the neural glance");
    assert_laid_out(glance, "neural glance");
    let caption = find_debug(panel, "glance-fallback").expect("the caption");
    assert_laid_out(caption, "caption");
    let (fx, _) = h.buffer_tree("*fx*");
    let ev_key = format!("rack-fx-glance-{group_id}-gvr-{owned}-ev");
    assert!(keys(&fx).contains(&ev_key), "the kit's instance's event view");

    // Live: a fire reaches the glance's event view.
    let event_count = |h: &Harness| match widget_keyed(&h.buffer_tree("*fx*").0, &ev_key)
        .expect("the event view")
        .get("events")
    {
        Some(Value::List(rows)) => rows.len(),
        other => panic!("events: {other:?}"),
    };
    assert_eq!(event_count(&h), 0, "nothing fired yet");
    h.shared.state.set_graph_visualizations(vec![GraphVisualizationSnapshot {
        id: owned,
        name: "neural".to_string(),
        active: true,
        current_beat: 1.0,
        num_nodes: 8,
        energy: vec![0.0; 8],
        trigger_activity: vec![0.0; 8],
        event_history: vec![GraphVisualizationEvent {
            node_index: 1,
            track: Some(0),
            sample_time: 24_000,
            beat: 1.0,
            transpose: 7.0,
            velocity: 0.75,
        }],
        history_stamp: 81,
        ..Default::default()
    }]);
    settle(&mut h);
    assert_eq!(event_count(&h), 1, "the fire shows in the glance");

    // A jaki joins: two cards share the body, the jaki glance takes its
    // height hint and the kit's colour.
    let jaki = create(&mut h, JAKI, Some(group_id));
    let layout = fx_layout(&mut h);
    let panel = find_debug(&layout, "rack-fx-sequencers-panel").expect("the sequencers panel");
    assert_laid_out(panel, "sequencers panel");
    let neural_glance = find_debug(panel, "neural-glance").expect("the neural glance");
    let jaki_glance = find_debug(panel, "jaki-glance").expect("the jaki glance");
    assert_laid_out(jaki_glance, "jaki glance");
    let hint = h.eval("(eseq.effects.buffers/rack-sequencer-glance-height 2)");
    let Value::Number(hint) = hint else { panic!("{hint:?}") };
    assert!((jaki_glance.rect.height as f64 - hint).abs() < 0.11, "{:?} vs {hint}", jaki_glance.rect);
    assert!(
        jaki_glance.rect.row + jaki_glance.rect.height <= panel.rect.row + panel.rect.height + 0.01,
        "two cards fit: {:?} in {:?}",
        jaki_glance.rect,
        panel.rect
    );
    assert!(
        (neural_glance.rect.height as f64 - hint).abs() < 0.11,
        "the neural glance keeps its hint: {:?} vs {hint}",
        neural_glance.rect
    );
    assert!(jaki_glance.rect.row > neural_glance.rect.row);
    let (fx, _) = h.buffer_tree("*fx*");
    let jaki_box = widget_keyed(&fx, &format!("rack-fx-glance-{group_id}-jk-{jaki}-glance")).expect("jaki box");
    assert!(
        !matches!(jaki_box.get("background-color"), Some(Value::Keyword(_))),
        "tinted by the kit colour: {:?}",
        jaki_box.get("background-color")
    );

    // A third and fourth overflow the panel's rows: the list scrolls.
    create(&mut h, NEURAL, Some(group_id));
    create(&mut h, JAKI, Some(group_id));
    let layout = fx_layout(&mut h);
    let panel = find_debug(&layout, "rack-fx-sequencers-panel").expect("the sequencers panel");
    let scroll = find_debug(panel, "rack-fx-sequencers-scroll").expect("the scroll");
    assert_laid_out(scroll, "scroll");
    assert!(scroll.rect.row + scroll.rect.height <= panel.rect.row + panel.rect.height + 0.01);
    let pads = find_debug(&layout, "rack-fx-pads-panel").expect("the drum rack panel");
    assert_laid_out(pads, "pads panel");
}
