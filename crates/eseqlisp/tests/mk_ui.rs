use eseqlisp::Runtime;
const P: &str = r#"
(def defsynth-ui (body) body)
(def ui-accent-blue () :blue) (def ui-accent-cyan () :cyan) (def ui-accent-orange () :orange)
(def ui-accent-green () :green) (def ui-accent-violet () :magenta)
(def ui-control-panel-dense-s (section body) body)
(def ui-readout-block-small-s (title accent section body) body)
(def ui-lego-badge-s (s t w a) (label t))
(def ui-lego-knob-s (s n t w a d) (label t))
(def ui-lego-micro-num-s (s n t w d u a) (label t))
(def ui-lego-column (a b c) (v-stack a b c))
(def ui-lego-column-2 (a b) (v-stack a b))
"#;
#[test]
fn mk_ui_evaluates() {
    let ui =
        std::fs::read_to_string("../sequencer/instruments/drums/membrane-kick/ui.lisp").unwrap();
    let mut rt = Runtime::new();
    rt.eval_str(P).expect("preamble");
    rt.eval_str(&ui).expect("ui evaluates");
}
