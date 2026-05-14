use std::collections::BTreeSet;

use eseqlisp::parser::{ASTParser, Expression, Parser};
use eseqlisp::Runtime;

use crate::lisp_effect::DGenManifest;

pub fn validate_instrument_ui_source(
    ui_source: &str,
    manifest: &DGenManifest,
) -> Result<(), String> {
    let tokens = Parser::new(ui_source.to_string())
        .parse()
        .map_err(|error| format!("ui.lisp parse error: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("ui.lisp AST error: {error:?}"))?;

    let mut defsynth_ui_count = 0;
    let mut referenced_params = BTreeSet::new();
    for expr in &exprs {
        collect_ui_validation_refs(expr, &mut defsynth_ui_count, &mut referenced_params);
        validate_layout_contract(expr, UiContext::Root)?;
    }

    if defsynth_ui_count == 0 {
        return Err("ui.lisp must contain exactly one (defsynth-ui ...) form".to_string());
    }
    if defsynth_ui_count > 1 {
        return Err(format!(
            "ui.lisp must contain exactly one (defsynth-ui ...) form, found {defsynth_ui_count}"
        ));
    }

    let valid_params = manifest
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<BTreeSet<_>>();

    if referenced_params.iter().any(|name| name.trim().is_empty()) {
        return Err(
            "ui.lisp references an empty parameter name (`\"\"`). Do not pass blank placeholder params to UI helpers. If the instrument has only one envelope, use `(ui-lego-adsr-s section \"AMP ENV\" \"amp_attack\" \"amp_decay\" \"amp_sustain\" \"amp_release\")` instead of `ui-adsr-switch`. Use `ui-adsr-switch` only when both ADSR slots reference real DSP params."
                .to_string(),
        );
    }

    let unknown = referenced_params
        .iter()
        .filter(|name| name.as_str() != "base_note" && !valid_params.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();

    if !unknown.is_empty() {
        return Err(format!(
            "ui.lisp references unknown parameter(s): {}. Valid DSP params: {}",
            unknown.join(", "),
            valid_params.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    validate_ui_evaluates(ui_source)?;

    Ok(())
}

fn validate_ui_evaluates(ui_source: &str) -> Result<(), String> {
    let mut runtime = Runtime::new();
    runtime
        .eval_str(
            r#"
            (def defsynth-ui (body) body)
            (def synth-ui-current-inst (dict :synth '()))
            (def synth-ui-current-name "validation")
            (def custom-ui-current-kind "instrument")
            (def custom-ui-selected-section 0)
            (def ui-section (title body) body)
            (def ui-panel (title section body) body)
            (def ui-panel-c (title section body) body)
            (def ui-rack (mode left-panels adsr-form right-panels) (h-stack left-panels adsr-form right-panels))
            (def ui-param-control (name) (label name :font-size 10 :color :gray :bg :transparent))
            (def ui-param-knob (name title) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-param-knob-c (name title) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-adsr (title attack decay sustain release) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-adsr-switch (section-a title-a attack-a decay-a sustain-a release-a section-b title-b attack-b decay-b sustain-b release-b) (label title-a :font-size 10 :color :gray :bg :transparent))
            (def ui-adsr-c (title attack decay sustain release) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-adsr-switch-c (section-a title-a attack-a decay-a sustain-a release-a section-b title-b attack-b decay-b sustain-b release-b) (label title-a :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-adsr-s (section title attack decay sustain release) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-detail-adsr-s (section title attack decay sustain release) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-detail-adsr-switch-s (section-a title-a attack-a decay-a sustain-a release-a section-b title-b attack-b decay-b sustain-b release-b) (label title-a :font-size 10 :color :gray :bg :transparent))
            (def ui-adsr-compact-s (section title attack decay sustain release) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-adsr-compact-switch-s (section-a title-a attack-a decay-a sustain-a release-a section-b title-b attack-b decay-b sustain-b release-b) (label title-a :font-size 10 :color :gray :bg :transparent))
            (def ui-adsr-number-s (section name title decimals unit) (label title :font-size 10 :color :gray :bg :transparent))
            (def base-note () (label "base" :font-size 10 :color :gray :bg :transparent))
            (def base-note-c () (label "base" :font-size 10 :color :gray :bg :transparent))
            (def ui-accent-blue () :blue)
            (def ui-accent-cyan () :cyan)
            (def ui-accent-orange () :orange)
            (def ui-accent-green () :green)
            (def ui-accent-violet () :magenta)
            (def ui-lego-gap () 0.25)
            (def ui-lego-small-h () 1.95)
            (def ui-lego-medium-h () 4.08)
            (def ui-lego-dense-h () 3.08)
            (def ui-lego-full-h () 8.48)
            (def ui-lego-col-w () 24.0)
            (def ui-lego-strip-w () 7.2)
            (def ui-control-block-small (title accent body) body)
            (def ui-control-block-medium (title accent body) body)
            (def ui-control-block-full (title accent body) body)
            (def ui-control-block-small-s (title accent section body) body)
            (def ui-control-block-medium-s (title accent section body) body)
            (def ui-control-block-dense-s (title accent section body) body)
            (def ui-control-panel-dense-s (section body) body)
            (def ui-control-panel-small-s (section body) body)
            (def ui-control-panel-medium-s (section body) body)
            (def ui-control-block-full-s (title accent section body) body)
            (def ui-readout-block-small (title accent body) body)
            (def ui-readout-block-small-s (title accent section body) body)
            (def ui-readout-block-dense-s (title accent section body) body)
            (def ui-readout-panel-small-s (section body) body)
            (def ui-readout-panel-dense-s (section body) body)
            (def ui-readout-panel-medium-s (section body) body)
            (def ui-readout-block-medium (title accent body) body)
            (def ui-readout-block-full (title accent body) body)
            (def ui-lego-column (a b c) (v-stack a b c))
            (def ui-lego-column-2 (a b) (v-stack a b))
            (def ui-lego-column-full (a) (v-stack a))
            (def ui-lego-strip-s (title accent section body) body)
            (def ui-lego-strip-half-s (title accent section body) body)
            (def ui-lego-strip-panel-s (section body) body)
            (def ui-lego-badge (title width accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-badge-s (section title width accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-knob (name title width accent decimals) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-knob-s (section name title width accent decimals) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-num (name title width decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-num-s (section name title width decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-micro-num-s (section name title width decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-option (name title width options accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-option-s (section name title width options accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-micro-option-s (section name title width options accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-row (name title decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-base-note (width accent) (label "base" :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-micro-base-note-s (section width accent) (label "base" :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-text-row-3 (a b c) (h-stack a b c))
            (def ui-lego-text-row-4 (a b c d) (h-stack a b c d))
            (def inst-param (inst name) (dict :name name :value 0 :min 0 :max 1))
            (def inst-base-note-param (inst) (dict :name "base_note" :value 0 :min -48 :max 48))
            (def custom-ui-current-scope () (dict :kind custom-ui-current-kind :name synth-ui-current-name :inst synth-ui-current-inst))
            (def custom-ui-select-section-in-scope (scope section) section)
            (def custom-ui-set-param-in-scope (scope p value) value)
            (def custom-ui-set-param-by-name-in-scope (scope name value) value)
            "#,
        )
        .map_err(|error| format!("ui validation runtime setup failed: {error:?}"))?;
    runtime
        .eval_str(ui_source)
        .map_err(|error| format!("ui.lisp evaluation error: {error:?}"))?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiContext {
    Root,
    RowPanel,
}

fn validate_layout_contract(expr: &Expression, context: UiContext) -> Result<(), String> {
    let Expression::List(items) = expr else {
        return Ok(());
    };
    let head = match items.first() {
        Some(Expression::Symbol(head)) => head.as_str(),
        _ => "",
    };

    if head == "scroll" {
        return Err(
            "ui.lisp must not use scroll; custom synth UIs must fit the fixed-height rack panel"
                .to_string(),
        );
    }
    if matches!(
        head,
        "ui-panel"
            | "ui-panel-c"
            | "ui-section"
            | "ui-rack"
            | "ui-param-control"
            | "ui-param-knob"
            | "ui-param-knob-c"
            | "base-note"
            | "base-note-c"
    ) {
        return Err(format!(
            "ui.lisp uses legacy UI helper `{head}`; generated instrument UIs must use the current ui-control-block-*/ui-readout-block-* and ui-lego-* building blocks"
        ));
    }
    if matches!(head, "ui-adsr" | "ui-adsr-c") && context == UiContext::RowPanel {
        return Err("ui.lisp must not nest ui-adsr inside a control/readout block; place ADSR as a standalone lego column or use ui-adsr-switch".to_string());
    }
    if head.starts_with("ui-accent-") && !is_known_ui_accent(head) {
        return Err(format!(
            "ui.lisp uses unknown accent helper `{head}`. Use only these accent helpers: {}. For purple/magenta styling, use `ui-accent-violet`.",
            KNOWN_UI_ACCENTS.join(", ")
        ));
    }

    let child_context = if matches!(
        head,
        "ui-control-block-small"
            | "ui-control-block-medium"
            | "ui-control-block-full"
            | "ui-control-block-small-s"
            | "ui-control-block-medium-s"
            | "ui-control-block-full-s"
            | "ui-readout-block-small"
            | "ui-readout-block-small-s"
            | "ui-readout-block-medium"
            | "ui-readout-block-full"
    ) {
        UiContext::RowPanel
    } else {
        context
    };
    for item in items {
        validate_layout_contract(item, child_context)?;
    }
    Ok(())
}

const KNOWN_UI_ACCENTS: [&str; 5] = [
    "ui-accent-blue",
    "ui-accent-cyan",
    "ui-accent-orange",
    "ui-accent-green",
    "ui-accent-violet",
];

fn is_known_ui_accent(name: &str) -> bool {
    KNOWN_UI_ACCENTS.contains(&name)
}

fn collect_ui_validation_refs(
    expr: &Expression,
    defsynth_ui_count: &mut usize,
    referenced_params: &mut BTreeSet<String>,
) {
    let Expression::List(items) = expr else {
        return;
    };
    let Some(Expression::Symbol(head)) = items.first() else {
        for item in items {
            collect_ui_validation_refs(item, defsynth_ui_count, referenced_params);
        }
        return;
    };

    match head.as_str() {
        "defsynth-ui" => *defsynth_ui_count += 1,
        "param" | "ui-param-control" | "ui-param-knob" | "ui-param-knob-c" | "ui-lego-knob"
        | "ui-lego-num" | "ui-lego-option" | "ui-lego-row" => {
            if let Some(name) = items.get(1).and_then(ui_param_ref_name) {
                referenced_params.insert(name);
            }
        }
        "ui-lego-knob-s"
        | "ui-lego-num-s"
        | "ui-lego-option-s"
        | "ui-lego-micro-num-s"
        | "ui-lego-micro-option-s" => {
            if let Some(name) = items.get(2).and_then(ui_param_ref_name) {
                referenced_params.insert(name);
            }
        }
        "params" => {
            for item in items.iter().skip(1) {
                if matches!(item, Expression::Keyword(_)) {
                    continue;
                }
                if let Some(name) = ui_param_ref_name(item) {
                    referenced_params.insert(name);
                }
            }
        }
        "ui-adsr" | "ui-adsr-c" => {
            for item in items.iter().skip(2).take(4) {
                if let Some(name) = ui_param_ref_name(item) {
                    referenced_params.insert(name);
                }
            }
        }
        "ui-lego-adsr-s" => {
            for item in items.iter().skip(3).take(4) {
                if let Some(name) = ui_param_ref_name(item) {
                    referenced_params.insert(name);
                }
            }
        }
        "ui-adsr-number-s" => {
            if let Some(name) = items.get(2).and_then(ui_param_ref_name) {
                referenced_params.insert(name);
            }
        }
        "inst-param" => {
            if let Some(name) = items.get(2).and_then(ui_param_ref_name) {
                referenced_params.insert(name);
            }
        }
        "custom-ui-current-param" => {
            if let Some(name) = items.get(1).and_then(ui_param_ref_name) {
                referenced_params.insert(name);
            }
        }
        "custom-ui-param-in-scope" | "custom-ui-set-param-by-name-in-scope" => {
            if let Some(name) = items.get(2).and_then(ui_param_ref_name) {
                referenced_params.insert(name);
            }
        }
        "ui-adsr-switch"
        | "ui-adsr-switch-c"
        | "ui-adsr-compact-switch-s"
        | "ui-detail-adsr-switch-s" => {
            for item in items.iter().skip(3).take(4) {
                if let Some(name) = ui_param_ref_name(item) {
                    referenced_params.insert(name);
                }
            }
            for item in items.iter().skip(9).take(4) {
                if let Some(name) = ui_param_ref_name(item) {
                    referenced_params.insert(name);
                }
            }
        }
        "ui-adsr-compact-s" | "ui-detail-adsr-s" => {
            for item in items.iter().skip(3).take(4) {
                if let Some(name) = ui_param_ref_name(item) {
                    referenced_params.insert(name);
                }
            }
        }
        _ => {}
    }

    for item in items {
        collect_ui_validation_refs(item, defsynth_ui_count, referenced_params);
    }
}

fn ui_param_ref_name(expr: &Expression) -> Option<String> {
    match expr {
        Expression::String(name) | Expression::Symbol(name) => Some(name.clone()),
        Expression::List(items) => items.first().and_then(ui_param_ref_name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::validate_instrument_ui_source;
    use crate::lisp_effect::{DGenManifest, DGenParam};

    fn manifest_with_params(names: &[&str]) -> DGenManifest {
        DGenManifest {
            dylib_path: std::path::PathBuf::new(),
            total_memory_slots: 0,
            params: names
                .iter()
                .map(|name| DGenParam {
                    name: name.to_string(),
                    cell_id: 0,
                    cell_span: 4,
                    default: 0.0,
                    min: 0.0,
                    max: 1.0,
                    unit: None,
                    hidden: false,
                })
                .collect(),
            inputs: Vec::new(),
            modulators: Vec::new(),
            mod_destinations: Vec::new(),
            n_inputs: 0,
            n_outputs: 1,
            tensors: Vec::new(),
            tensor_init_data: Vec::new(),
            voice_cell_id: None,
        }
    }

    #[test]
    fn validates_defsynth_ui_and_param_refs() {
        let manifest = manifest_with_params(&[
            "amp_attack",
            "amp_decay",
            "amp_sustain",
            "amp_release",
            "cutoff",
        ]);
        validate_instrument_ui_source(
            r#"
            (defsynth-ui
              (h-stack :width :fill :gap 0.35 :align :stretch
                (ui-lego-column-full
                  (ui-control-block-medium-s "FILT" (ui-accent-green) 1
                    (h-stack :gap 0.32 :align :start
                      (ui-lego-knob-s 1 "cutoff" "cut" 4.8 (ui-accent-green) 0))))
                (ui-lego-column-full
                  (ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"))))
            "#,
            &manifest,
        )
        .unwrap();
    }

    #[test]
    fn rejects_unknown_param_refs() {
        let manifest = manifest_with_params(&["gain"]);
        let err = validate_instrument_ui_source(
            r#"
            (defsynth-ui
              (ui-lego-column-full
                (ui-control-block-medium-s "OUT" (ui-accent-orange) 0
                  (h-stack
                    (ui-lego-knob-s 0 "gaim" "gain" 4.8 (ui-accent-orange) 2)))))
            "#,
            &manifest,
        )
        .unwrap_err();
        assert!(err.contains("gaim"));
        assert!(err.contains("gain"));
    }

    #[test]
    fn rejects_empty_param_refs_with_adsr_guidance() {
        let manifest =
            manifest_with_params(&["amp_attack", "amp_decay", "amp_sustain", "amp_release"]);
        let err = validate_instrument_ui_source(
            r#"
            (defsynth-ui
              (ui-lego-column-full
                (box :width (ui-lego-col-w) :height (ui-lego-full-h)
                  (ui-adsr-switch
                    0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
                    0 " " "" "" "" ""))))
            "#,
            &manifest,
        )
        .unwrap_err();

        assert!(err.contains("empty parameter name"));
        assert!(err.contains("ui-lego-adsr-s"));
        assert!(err
            .contains("Use `ui-adsr-switch` only when both ADSR slots reference real DSP params"));
    }

    #[test]
    fn rejects_unknown_ui_accent_helpers() {
        let manifest = manifest_with_params(&["gain"]);
        let err = validate_instrument_ui_source(
            r#"
            (defsynth-ui
              (ui-lego-column-full
                (ui-control-block-medium-s "OUT" (ui-accent-magenta) 0
                  (h-stack
                    (ui-lego-knob-s 0 "gain" "gain" 4.8 (ui-accent-magenta) 2)))))
            "#,
            &manifest,
        )
        .unwrap_err();

        assert!(err.contains("unknown accent helper `ui-accent-magenta`"));
        assert!(err.contains("ui-accent-violet"));
    }

    #[test]
    fn rejects_ui_eval_errors() {
        let manifest = manifest_with_params(&["gain"]);
        let err =
            validate_instrument_ui_source(r#"(defsynth-ui (missing-widget "gain"))"#, &manifest)
                .unwrap_err();
        assert!(err.contains("evaluation error"));
    }

    #[test]
    fn rejects_scroll_and_nested_adsr() {
        let manifest =
            manifest_with_params(&["amp_attack", "amp_decay", "amp_sustain", "amp_release"]);
        let scroll_err =
            validate_instrument_ui_source(r#"(defsynth-ui (scroll (label "bad")))"#, &manifest)
                .unwrap_err();
        assert!(scroll_err.contains("must not use scroll"));

        let nested_err = validate_instrument_ui_source(
            r#"
            (defsynth-ui
              (ui-control-block-medium-s "AMP" (ui-accent-orange) 0
                (ui-adsr "amp" "amp_attack" "amp_decay" "amp_sustain" "amp_release")))
            "#,
            &manifest,
        )
        .unwrap_err();
        assert!(nested_err.contains("must not nest ui-adsr"));
    }

    #[test]
    fn rejects_legacy_ui_helpers() {
        let manifest = manifest_with_params(&["cutoff"]);
        for helper_source in [
            r#"(defsynth-ui (ui-panel "FILT" 0 (h-stack)))"#,
            r#"(defsynth-ui (ui-param-knob "cutoff" "cut"))"#,
            r#"(defsynth-ui (base-note))"#,
        ] {
            let err = validate_instrument_ui_source(helper_source, &manifest).unwrap_err();
            assert!(err.contains("legacy UI helper"), "{err}");
        }
    }

    #[test]
    fn validates_lego_dimension_helpers_used_by_generated_ui() {
        let manifest = manifest_with_params(&[
            "amp_attack",
            "amp_decay",
            "amp_sustain",
            "amp_release",
            "filt_attack",
            "filt_decay",
            "filt_sustain",
            "filt_release",
            "cutoff",
        ]);

        validate_instrument_ui_source(
            r#"
            (def envelope-column ()
              (ui-lego-column-full
                (box :width (ui-lego-col-w) :height (ui-lego-full-h)
                  (ui-adsr-switch
                    0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
                    1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release"))))

            (defsynth-ui
              (h-stack :width :fill :gap 0.35 :align :stretch
                (ui-lego-column-full
                  (ui-control-block-medium-s "FILTER" (ui-accent-green) 1
                    (h-stack :gap 0.32 :align :start
                      (ui-lego-knob-s 1 "cutoff" "cut" 4.8 (ui-accent-green) 0))))
                (envelope-column)))
            "#,
            &manifest,
        )
        .unwrap();
    }

    #[test]
    fn validates_dropdown_ui_and_inst_param_refs() {
        let manifest = manifest_with_params(&["filter_model", "cutoff", "resonance"]);

        validate_instrument_ui_source(
            r#"
            (def filter-model-label (value)
              (if (= value 1) "ladder" "svf"))

            (def filter-model-value (label)
              (if (= label "ladder") 1 0))

            (def filter-model-dropdown (section)
              (let ((p (inst-param synth-ui-current-inst "filter_model")))
                (if p
                  (let ((scope (custom-ui-current-scope)))
                    (subtree :key (str "filter-model-dropdown-" synth-ui-current-name)
                      (v-stack :width 5.0 :height 1.12 :gap 0.08 :align :start
                        (label "model" :font-size 8.2 :width 5.0 :color :dim :bg :transparent)
                        (dropdown :value (filter-model-label (get p :value))
                          :options '("svf" "ladder")
                          :width 5.6 :height 0.78 :font-size 8.0
                          :on-change (lambda (v)
                            (do
                              (custom-ui-select-section-in-scope scope section)
                              (custom-ui-set-param-in-scope scope p (filter-model-value v))))))))
                  (label "missing: filter_model" :font-size 9 :color :red :bg :transparent))))

            (defsynth-ui
              (h-stack :width :fill :gap 0.35 :align :stretch
                (ui-lego-column-full
                  (ui-control-block-medium-s "FILTER" (ui-accent-green) 1
                    (h-stack :gap 0.32 :align :start
                      (filter-model-dropdown 1)
                      (ui-lego-knob-s 1 "cutoff" "cut" 4.8 (ui-accent-green) 0)
                      (ui-lego-knob-s 1 "resonance" "res" 4.8 (ui-accent-green) 2))))))
            "#,
            &manifest,
        )
        .unwrap();
    }

    #[test]
    fn rejects_unknown_inst_param_refs_in_dropdown_ui() {
        let manifest = manifest_with_params(&["cutoff", "resonance"]);

        let err = validate_instrument_ui_source(
            r#"
            (def filter-model-dropdown ()
              (let ((p (inst-param synth-ui-current-inst "filter_modle")))
                (if p
                  (dropdown :value "svf" :options '("svf" "ladder"))
                  (label "missing" :font-size 9 :color :red :bg :transparent))))

            (defsynth-ui
              (ui-lego-column-full
                (ui-control-block-medium-s "FILTER" (ui-accent-green) 1
                  (h-stack
                    (filter-model-dropdown)
                    (ui-lego-knob-s 1 "cutoff" "cut" 4.8 (ui-accent-green) 0)))))
            "#,
            &manifest,
        )
        .unwrap_err();

        assert!(err.contains("filter_modle"));
        assert!(err.contains("cutoff"));
    }

    #[test]
    fn validates_lego_option_helper_refs() {
        let manifest = manifest_with_params(&["osc1_wave", "osc2_wave", "cutoff"]);

        validate_instrument_ui_source(
            r#"
            (defsynth-ui
              (h-stack :width :fill :gap 0.35 :align :stretch
                (ui-lego-column-full
                  (ui-control-block-medium-s "OSC" (ui-accent-cyan) 0
                    (h-stack :gap 0.32 :align :start
                      (ui-lego-option-s 0 "osc1_wave" "wave" 5.2 '("saw" "pulse" "sine" "triangle") (ui-accent-cyan))
                      (ui-lego-option-s 0 "osc2_wave" "wave" 5.2 '("saw" "pulse" "sine" "triangle") (ui-accent-violet))
                      (ui-lego-knob-s 0 "cutoff" "cut" 4.8 (ui-accent-green) 0))))))
            "#,
            &manifest,
        )
        .unwrap();
    }

    #[test]
    fn validates_dense_lego_helpers_recommended_by_prompt() {
        let manifest = manifest_with_params(&[
            "osc1_wave",
            "osc1_level",
            "osc2_wave",
            "osc2_level",
            "amp_attack",
            "amp_decay",
            "amp_sustain",
            "amp_release",
            "filt_attack",
            "filt_decay",
            "filt_sustain",
            "filt_release",
            "lfo1_wave",
            "lfo1_rate",
        ]);

        validate_instrument_ui_source(
            r#"
            (def osc-block ()
              (ui-control-panel-dense-s 0
                (v-stack :width :fill :gap 0.12 :align :start
                  (h-stack :gap 0.18 :align :start
                    (ui-lego-badge-s 0 "OSC1" 3.6 (ui-accent-cyan))
                    (ui-lego-micro-option-s 0 "osc1_wave" "wave" 4.4 '("saw" "pulse") (ui-accent-cyan))
                    (ui-lego-micro-num-s 0 "osc1_level" "lvl" 3.4 2 false (ui-accent-cyan)))
                  (h-stack :gap 0.18 :align :start
                    (ui-lego-badge-s 0 "OSC2" 3.6 (ui-accent-blue))
                    (ui-lego-micro-option-s 0 "osc2_wave" "wave" 4.4 '("saw" "pulse") (ui-accent-blue))
                    (ui-lego-micro-num-s 0 "osc2_level" "lvl" 3.4 2 false (ui-accent-blue))))))

            (def detail-block ()
              (ui-detail-adsr-switch-s
                0 "AMP" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
                1 "FILTER" "filt_attack" "filt_decay" "filt_sustain" "filt_release"))

            (def lfo-strip ()
              (ui-lego-strip-panel-s 2
                (v-stack :width :fill :align :center
                  (ui-lego-badge-s 2 "LFO1" 5.8 (ui-accent-blue))
                  (ui-lego-micro-option-s 2 "lfo1_wave" "wave" 5.8 '("sine" "saw") (ui-accent-blue))
                  (ui-lego-micro-num-s 2 "lfo1_rate" "rate" 5.8 2 "Hz" (ui-accent-blue))
                  (ui-lego-micro-base-note-s 2 5.8 (ui-accent-orange)))))

            (defsynth-ui
              (h-stack :width :fill :gap 0.35 :align :stretch
                (ui-lego-column-full (osc-block))
                (ui-lego-column-full
                  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
                    (ui-control-panel-small-s 0 (h-stack))
                    (ui-readout-panel-medium-s 0 (detail-block))))
                (lfo-strip)))
            "#,
            &manifest,
        )
        .unwrap();
    }

    #[test]
    fn rejects_unknown_lego_option_helper_refs() {
        let manifest = manifest_with_params(&["osc1_wave"]);

        let err = validate_instrument_ui_source(
            r#"
            (defsynth-ui
              (ui-lego-column-full
                (ui-control-block-medium-s "OSC" (ui-accent-cyan) 0
                  (ui-lego-option-s 0 "osc1_wvae" "wave" 5.2 '("saw" "pulse") (ui-accent-cyan)))))
            "#,
            &manifest,
        )
        .unwrap_err();

        assert!(err.contains("osc1_wvae"));
        assert!(err.contains("osc1_wave"));
    }
}
