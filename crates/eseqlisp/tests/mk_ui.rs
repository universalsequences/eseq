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
(def ui-lego-column (a b c) (v-stack a b c))
(def ui-lego-column-2 (a b) (v-stack a b))
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
        std::fs::read_to_string("../sequencer/instruments/drums/membrane-kick/ui.lisp").unwrap();
    let mut rt = Runtime::new();
    rt.eval_str(P).expect("preamble");
    rt.eval_str(&ui).expect("ui evaluates");
}

#[test]
fn membrane_snare_rim_ui_evaluates_and_measures_stroke_panel() {
    let ui = std::fs::read_to_string("../sequencer/instruments/drums/membrane-snare-rim/ui.lisp")
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
