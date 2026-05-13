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
            (def ui-lego-full-h () 8.48)
            (def ui-lego-col-w () 24.0)
            (def ui-control-block-small (title accent body) body)
            (def ui-control-block-medium (title accent body) body)
            (def ui-control-block-full (title accent body) body)
            (def ui-control-block-small-s (title accent section body) body)
            (def ui-control-block-medium-s (title accent section body) body)
            (def ui-control-block-full-s (title accent section body) body)
            (def ui-readout-block-small (title accent body) body)
            (def ui-readout-block-small-s (title accent section body) body)
            (def ui-readout-block-medium (title accent body) body)
            (def ui-readout-block-full (title accent body) body)
            (def ui-lego-column (a b c) (v-stack a b c))
            (def ui-lego-column-2 (a b) (v-stack a b))
            (def ui-lego-column-full (a) (v-stack a))
            (def ui-lego-knob (name title width accent decimals) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-knob-s (section name title width accent decimals) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-num (name title width decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-num-s (section name title width decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-row (name title decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-base-note (width accent) (label "base" :font-size 10 :color :gray :bg :transparent))
            (def ui-lego-text-row-3 (a b c) (h-stack a b c))
            (def ui-lego-text-row-4 (a b c d) (h-stack a b c d))
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
        | "ui-lego-num" | "ui-lego-row" => {
            if let Some(name) = items.get(1).and_then(ui_param_ref_name) {
                referenced_params.insert(name);
            }
        }
        "ui-lego-knob-s" | "ui-lego-num-s" => {
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
        "ui-adsr-switch" | "ui-adsr-switch-c" => {
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
}
