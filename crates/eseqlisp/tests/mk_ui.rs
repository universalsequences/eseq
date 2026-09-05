use eseqlisp::{
    Runtime,
    layout::{LayoutEngine, LayoutNode},
    vm::Value,
};
const P: &str = r#"
(def defsynth-ui (body) body)
(def ui-accent-blue () :blue) (def ui-accent-cyan () :cyan) (def ui-accent-orange () :orange)
(def ui-accent-green () :green) (def ui-accent-violet () :magenta)
(def ui-control-panel-dense-s (section body) body)
(def ui-readout-block-small-s (title accent section body) body)
(def ui-lego-badge-s (s t w a) (label t))
(def ui-lego-knob-s (s n t w a d) (label t))
(def ui-lego-micro-num-s (s n t w d u a) (label t))
(def ui-lego-matrix-s (s n t w h a) (label t))
(def ui-lego-xy-pad-s (s xn yn t w h a) (label t))
(def ui-lego-column (a b c) (v-stack a b c))
(def ui-lego-column-2 (a b) (v-stack a b))
(def ui-lego-panel-width-s (width height section surface body) body)
(def ui-lego-knob-sized-s (s n t w h k a d) (label t))
(def ui-lego-col-w () 24.0)
(def ui-lego-small-h () 2.0)
(def ui-lego-dense-h () 3.8)
(def ui-lego-gap () 0.06125)
"#;

fn layout_label_node<'a>(node: &'a LayoutNode, text: &str) -> Option<&'a LayoutNode> {
    if node.widget_type == "label"
        && matches!(node.props.get("text"), Some(Value::String(value)) if value == text)
    {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| layout_label_node(child, text))
}

#[test]
fn mk_ui_evaluates() {
    let ui =
        std::fs::read_to_string("../sequencer/tests/fixtures/instruments/drums/membrane-kick/ui.lisp").unwrap();
    let mut rt = Runtime::new();
    rt.eval_str(P).expect("preamble");
    rt.eval_str(&ui).expect("ui evaluates");
}

#[test]
fn membrane_snare_rim_ui_evaluates_and_measures_stroke_panel() {
    let ui = std::fs::read_to_string("../sequencer/tests/fixtures/instruments/drums/membrane-snare-rim/ui.lisp")
        .unwrap();
    let mut rt = Runtime::new();
    rt.eval_str(P).expect("preamble");
    let tree = rt
        .eval_str(&ui)
        .expect("membrane-snare-rim ui evaluates")
        .expect("ui tree");

    let layout = LayoutEngine::new(96, 18, 1.0)
        .layout(&tree)
        .expect("membrane-snare-rim ui layout");
    let stroke = layout_label_node(&layout, "STROKE").expect("STROKE label");
    assert!(
        stroke.rect.width.is_finite()
            && stroke.rect.width > 0.0
            && stroke.rect.height.is_finite()
            && stroke.rect.height > 0.0,
        "STROKE label should have a finite nonzero rect, got {:?}",
        stroke.rect
    );
}

#[test]
fn membrane_snare_mk2_ui_evaluates_and_lays_out_every_section() {
    let ui = std::fs::read_to_string("../sequencer/tests/fixtures/instruments/drums/membrane-snare-mk2/ui.lisp")
        .unwrap();
    let mut rt = Runtime::new();
    rt.eval_str(P).expect("preamble");
    let tree = rt
        .eval_str(&ui)
        .expect("membrane-snare-mk2 ui evaluates")
        .expect("ui tree");

    let layout = LayoutEngine::new(96, 18, 1.0)
        .layout(&tree)
        .expect("membrane-snare-mk2 ui layout");

    // Every section title must reach the layout with a real rect. This is the
    // check that catches a panel that silently collapsed, which is how a
    // mis-sized strip shows up: the widgets still evaluate, they just get no
    // room. SPREAD and LOWCUT are the controls this fork adds over
    // membrane-snare-rim, and MASK is the strike-position matrix that the
    // modal layout has no slot for, so all three are named explicitly.
    for label in [
        "STRIKE", "STROKE", "HEAD 1", "HEAD 2", "WIRES", "RIM", "SHAPE", "MIX", "SPREAD",
        "LOWCUT", "HIT", "TILT", "BRIGHT", "DAMP", "TENSION",
    ] {
        let node = layout_label_node(&layout, label)
            .unwrap_or_else(|| panic!("{label} label missing from layout"));
        assert!(
            node.rect.width.is_finite()
                && node.rect.width > 0.0
                && node.rect.height.is_finite()
                && node.rect.height > 0.0,
            "{label} label should have a finite nonzero rect, got {:?}",
            node.rect
        );
    }

    // The old BODY panel and its six resonator controls are gone; this fork
    // replaces them with the SHAPE strip, and nothing should resurrect them.
    // MASK is gone too: the painted strike-mask matrix became the HIT x/y pad.
    for label in ["BODY", "1 HZ", "2 HZ", "3 HZ", "MASK"] {
        assert!(
            layout_label_node(&layout, label).is_none(),
            "{label} should not appear: the body EQ became SHAPE and the mask became HIT"
        );
    }
}
