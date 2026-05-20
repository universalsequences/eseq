use super::super::WidgetDefinition;
use super::super::WidgetKeyEvent;
use super::super::text_input::{TextInputState, cache_char_widths};
use super::display::*;
use super::emit::{emit_patch_debug_lisp, emit_patch_debug_lisp_for_view};
use super::geometry::*;
use super::interaction::*;
use super::metrics::*;
use super::model::{CableEndpoint, InputPortRef, OutputPortRef};
use super::project::dgenlisp_operator_names;
use super::render::*;
use super::state::*;
use super::writeback::{WriteBackError, emit_patch_writeback};
use super::*;
use crate::layout::{LayoutNode, MeasureCtx, Rect, TextMeasurer};
use crate::theme;
use crate::vm::Value;
use crossterm::event::{KeyCode, KeyModifiers};
use std::collections::HashMap;

fn parse(source: &str) -> Patch {
    parse_patch_source(source, PatcherIntent::Instrument).unwrap()
}

fn source_expr(scope: SourceScopeId, form_index: usize, path: &[usize]) -> SourceExprId {
    SourceExprId {
        form_id: SourceFormId {
            scope,
            index: form_index,
        },
        path: ExprPath(
            path.iter()
                .copied()
                .map(ExprPathSegment::ListItem)
                .collect(),
        ),
    }
}

#[cfg(target_os = "macos")]
struct FixedWidthTextMeasurer;

#[cfg(target_os = "macos")]
impl TextMeasurer for FixedWidthTextMeasurer {
    fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
        text.chars()
            .map(|ch| if ch.is_whitespace() { 5.0 } else { 10.0 })
            .sum()
    }

    fn line_height_px(&self, _font_size: f32) -> f32 {
        20.0
    }
}

fn node_expr(node: &PatchNode) -> SourceExprId {
    node.source
        .as_ref()
        .and_then(|source| source.expr.clone())
        .expect("node source expr")
}

#[test]
fn projects_instrument_plumbing_and_nested_calls() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (triangle (phasor pitch)))
            (out sig 1 @name audio)
            "#,
    );
    assert!(
        patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::In && node.id == "pitch")
    );
    assert!(patch.nodes.iter().any(|node| node.op == "phasor"));
    assert!(patch.nodes.iter().any(|node| node.op == "triangle"));
    assert!(
        patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::Out && node.id == "audio")
    );
    assert!(patch.connections.len() >= 3, "{:#?}", patch.connections);
}

#[test]
fn source_metadata_tracks_nested_expression_paths() {
    let patch = parse("(def result (phasor (* 25 (param freq @min 1 @max 100)) (rampToTrig xyz)))");

    let phasor = patch.nodes.iter().find(|node| node.op == "phasor").unwrap();
    let multiply = patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let constant = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Constant && node.op == "25")
        .unwrap();
    let param = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param)
        .unwrap();
    let ramp = patch
        .nodes
        .iter()
        .find(|node| node.op == "rampToTrig")
        .unwrap();

    assert_eq!(node_expr(phasor), source_expr(SourceScopeId::Root, 0, &[2]));
    assert_eq!(
        node_expr(multiply),
        source_expr(SourceScopeId::Root, 0, &[2, 1])
    );
    assert!(matches!(
        &multiply.source.as_ref().unwrap().owner,
        SourceOwner::NestedExpr { expr }
            if expr == &source_expr(SourceScopeId::Root, 0, &[2, 1])
    ));
    assert_eq!(
        node_expr(constant),
        source_expr(SourceScopeId::Root, 0, &[2, 1, 1])
    );
    assert!(matches!(
        &constant.source.as_ref().unwrap().owner,
        SourceOwner::ArgumentSlot { call, arg }
            if call == &source_expr(SourceScopeId::Root, 0, &[2, 1])
                && arg.item_index == 1
    ));
    assert_eq!(
        node_expr(param),
        source_expr(SourceScopeId::Root, 0, &[2, 1, 2])
    );
    assert_eq!(
        node_expr(ramp),
        source_expr(SourceScopeId::Root, 0, &[2, 2])
    );

    let phasor_shape = phasor
        .source
        .as_ref()
        .and_then(|source| source.call_shape.as_ref())
        .unwrap();
    assert_eq!(phasor_shape.positional_args.len(), 2);
    assert_eq!(phasor_shape.positional_args[0].semantic_index, 0);
    assert_eq!(phasor_shape.positional_args[0].item_index, 1);
    assert_eq!(
        phasor_shape.positional_args[0].expr,
        source_expr(SourceScopeId::Root, 0, &[2, 1])
    );
    assert_eq!(phasor_shape.positional_args[1].semantic_index, 1);
    assert_eq!(phasor_shape.positional_args[1].item_index, 2);
    assert_eq!(
        phasor_shape.positional_args[1].expr,
        source_expr(SourceScopeId::Root, 0, &[2, 2])
    );
}

#[test]
fn source_metadata_separates_positional_args_from_attributes() {
    let patch = parse("(param freq @min 1 @max 100)");
    let param = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param)
        .unwrap();
    let shape = param
        .source
        .as_ref()
        .and_then(|source| source.call_shape.as_ref())
        .unwrap();

    assert_eq!(shape.positional_args.len(), 1);
    assert_eq!(shape.positional_args[0].semantic_index, 0);
    assert_eq!(shape.positional_args[0].item_index, 1);
    assert_eq!(
        shape.positional_args[0].expr,
        source_expr(SourceScopeId::Root, 0, &[1])
    );

    assert_eq!(
        shape
            .attributes
            .iter()
            .map(|attr| (
                attr.key.as_str(),
                attr.key_item_index,
                attr.value_item_index
            ))
            .collect::<Vec<_>>(),
        vec![("@min", 2, 3), ("@max", 4, 5)]
    );
    assert_eq!(
        shape.attributes[0].value,
        source_expr(SourceScopeId::Root, 0, &[3])
    );
    assert_eq!(
        shape.attributes[1].value,
        source_expr(SourceScopeId::Root, 0, &[5])
    );
}

#[test]
fn operator_metadata_comes_from_generated_dgenlisp_json() {
    let names = dgenlisp_operator_names();
    assert!(names.len() >= 100, "expected generated operator metadata");
    assert!(names.contains("phasor"));
    assert!(names.contains("spectrum-delay"));
    assert!(names.contains("tosignal"));
}

#[test]
fn projects_params_and_attributes_as_param_node() {
    let patch = parse("(param cutoff @default 800 @min 20 @max 12000)");
    let node = patch.nodes.iter().find(|node| node.id == "cutoff").unwrap();
    assert_eq!(node.kind, NodeKind::Param);
    assert_eq!(
        node_display_label(node),
        "param cutoff @default 800 @min 20 @max 12000"
    );
}

#[test]
fn param_references_project_as_connections_not_literal_args() {
    let patch = parse(
        r#"
            (param size @min 0 @max 3000 @default 300)
            (def input (in 1))
            (def delayed (delay input size))
            "#,
    );
    let param = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param)
        .unwrap();
    let delay = patch.nodes.iter().find(|node| node.op == "delay").unwrap();

    assert_eq!(
        node_display_label(param),
        "param size @min 0 @max 3000 @default 300"
    );
    assert_eq!(node_display_label(delay), "delay");
    assert!(
        patch.connections.iter().any(|connection| {
            connection.from_node == param.id
                && connection.to_node == delay.id
                && connection.to_input == 1
        }),
        "{:#?}",
        patch.connections
    );
}

#[test]
fn source_metadata_resolves_param_references_by_binding_identity() {
    let patch = parse(
        r#"
            (def unresolved (phasor freq))
            (param freq @min 1 @max 100)
            (def a (phasor freq))
            (def b (+ freq a))
            "#,
    );
    let param_binding = BindingId {
        scope: SourceScopeId::Root,
        name: "freq".to_string(),
        kind: BindingKind::Param,
    };
    let resolved = patch
        .connections
        .iter()
        .filter_map(|connection| connection.source.as_ref())
        .filter_map(|source| match &source.previous_arg {
            SourceArgValue::SymbolReference {
                symbol,
                resolved_binding,
                ..
            } if symbol == "freq" => resolved_binding.clone(),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(resolved, vec![param_binding.clone(), param_binding]);
    assert!(
        patch
            .nodes
            .iter()
            .find(|node| node.id == "unresolved")
            .unwrap()
            .args
            .iter()
            .any(|arg| matches!(arg, ArgValue::Literal(value) if value == "freq"))
    );
}

#[test]
fn projects_destructuring_def_outputs() {
    let patch = parse("(def (re im) (fft input))");
    let node = patch.nodes.iter().find(|node| node.op == "fft").unwrap();
    assert_eq!(node.outputs, vec!["re".to_string(), "im".to_string()]);
}

#[test]
fn collapses_history_read_and_write_into_make_history_node() {
    let patch = parse(
        r#"
            (make-history h)
            (def sig (noise))
            (def previous (read-history h))
            (def mixed (+ sig previous))
            (write-history h sig)
            "#,
    );
    let history = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::History)
        .expect("history node");
    assert_eq!(node_display_label(history), "history");
    assert_eq!(
        patch
            .nodes
            .iter()
            .filter(|node| matches!(
                node.op.as_str(),
                "make-history" | "read-history" | "write-history"
            ))
            .count(),
        1,
        "{:#?}",
        patch.nodes
    );
    assert!(
        patch
            .connections
            .iter()
            .any(|connection| connection.from_node == history.id
                && connection.kind == ConnectionKind::Forward)
    );
    assert!(
        patch
            .connections
            .iter()
            .any(|connection| connection.to_node == history.id
                && connection.to_input == 0
                && connection.kind == ConnectionKind::Feedback)
    );
}

#[test]
fn source_metadata_tracks_history_compound_ownership_and_connections() {
    let patch = parse(
        r#"
            (make-history h)
            (def sig (noise))
            (def delta (- (read-history h) sig))
            (write-history h sig)
            "#,
    );
    let history = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::History)
        .expect("history node");
    let source = history.source.as_ref().expect("history source");
    let SourceOwner::Compound { parts } = &source.owner else {
        panic!("history should have compound owner: {source:#?}");
    };
    assert!(
        parts.iter().any(
            |owner| matches!(owner, SourceOwner::TopLevelForm { form_id } if form_id.index == 0)
        )
    );
    assert!(
        parts
            .iter()
            .any(|owner| matches!(owner, SourceOwner::NestedExpr { expr } if expr == &source_expr(SourceScopeId::Root, 2, &[2, 1])))
    );
    assert!(
        parts.iter().any(
            |owner| matches!(owner, SourceOwner::TopLevelForm { form_id } if form_id.index == 3)
        )
    );

    let feedback = patch
        .connections
        .iter()
        .find(|connection| connection.kind == ConnectionKind::Feedback)
        .unwrap();
    let feedback_source = feedback.source.as_ref().expect("feedback source");
    assert_eq!(
        feedback_source.to_call,
        source_expr(SourceScopeId::Root, 3, &[])
    );
    assert_eq!(feedback_source.to_arg.semantic_index, 1);
    assert_eq!(feedback_source.to_arg.item_index, 2);

    assert!(
        patch
            .connections
            .iter()
            .any(|connection| connection.from_node == history.id
                && connection.kind == ConnectionKind::Forward
                && connection.source.as_ref().is_some_and(|source| {
                    source.to_call == source_expr(SourceScopeId::Root, 2, &[2])
                        && source.to_arg.semantic_index == 0
                        && source.to_arg.item_index == 1
                }))
    );
}

#[test]
fn unsupported_forms_become_code_islands() {
    let patch = parse("(if gate (out 1 1 @name audio) (out 0 1 @name audio))");
    assert!(
        patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::CodeIsland)
    );
    assert!(!patch.diagnostics.is_empty());
}

#[test]
fn source_metadata_marks_code_island_owner() {
    let patch = parse("(if gate (out 1 1 @name audio) (out 0 1 @name audio))");
    let code = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::CodeIsland)
        .unwrap();
    let source = code.source.as_ref().expect("code island source");
    assert_eq!(source.expr, Some(source_expr(SourceScopeId::Root, 0, &[])));
    assert!(matches!(
        &source.owner,
        SourceOwner::CodeIsland { form_id }
            if *form_id == SourceFormId {
                scope: SourceScopeId::Root,
                index: 0,
            }
    ));
}

#[test]
fn source_metadata_scopes_macro_subpatches_separately() {
    let patch = parse(
        r#"
            (def root (phasor 1))
            (defmacro ap (sig g)
              (def scaled (* sig g))
              (phasor scaled))
            "#,
    );
    let root_phasor = patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .expect("root phasor");
    assert_eq!(
        node_expr(root_phasor),
        source_expr(SourceScopeId::Root, 0, &[2])
    );

    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let scaled = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.op == "*")
        .expect("macro multiply");
    let macro_scope = SourceScopeId::Macro {
        name: "ap".to_string(),
    };
    assert_eq!(node_expr(scaled), source_expr(macro_scope.clone(), 0, &[2]));

    let macro_phasor = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .expect("macro phasor");
    assert_eq!(
        node_expr(macro_phasor),
        source_expr(macro_scope.clone(), 1, &[])
    );

    let sig_binding = BindingId {
        scope: macro_scope.clone(),
        name: "sig".to_string(),
        kind: BindingKind::MacroParam,
    };
    assert!(
        macro_patch
            .patch
            .connections
            .iter()
            .filter_map(|connection| connection.source.as_ref())
            .any(|source| matches!(
                &source.previous_arg,
                SourceArgValue::SymbolReference {
                    symbol,
                    resolved_binding: Some(binding),
                    ..
                } if symbol == "sig" && binding == &sig_binding
            ))
    );
    assert_ne!(node_expr(root_phasor).form_id.scope, macro_scope);
}

#[test]
fn debug_emit_preserves_nested_structure_for_source_backed_patch() {
    let patch = parse("(def result (phasor (* 25 (param freq @min 1 @max 100)) (rampToTrig xyz)))");

    assert_eq!(
        emit_patch_debug_lisp(&patch),
        "(def result (phasor (* 25 (param freq @min 1 @max 100)) (rampToTrig xyz)))"
    );
}

#[test]
fn debug_emit_reflects_committed_node_text_edits_without_saving() {
    let source = parse(
        r#"
            (param freq)
            (def result (phasor freq))
            "#,
    );
    let phasor = source
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", phasor, "phasor".to_string());
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &phasor.id))
        .unwrap()
        .text = "sine".to_string();

    let patch = patch_with_interaction_state(source, &state, "root");

    assert_eq!(
        emit_patch_debug_lisp(&patch),
        "(param freq)\n(def result (sine freq))"
    );
}

#[test]
fn debug_emit_wraps_macro_subpatches_as_defmacro() {
    let patch = parse(
        r#"
            (defmacro ap (sig g)
              (def node (+ (* sig 1) (* h g)))
              (- node g))
            "#,
    );
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();

    assert_eq!(
        emit_patch_debug_lisp_for_view("macro:ap", &macro_patch.patch),
        "(defmacro ap (sig g)\n  (def node (+ (* sig 1) (* h g)))\n  (- node g))"
    );
}

#[test]
fn debug_emit_uses_macro_parameter_names_for_edited_connections() {
    let patch = parse(
        r#"
            (defmacro ap (sig g)
              (def node (+ sig (* h g)))
              node)
            "#,
    );
    let mut macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap()
        .patch
        .clone();
    let sig_node_id = macro_patch
        .nodes
        .iter()
        .find(|node| {
            matches!(
                node.source.as_ref().map(|source| &source.owner),
                Some(SourceOwner::MacroParameter { binding, .. }) if binding.name == "sig"
            )
        })
        .unwrap()
        .id
        .clone();
    let plus_node_id = macro_patch
        .nodes
        .iter()
        .find(|node| node.op == "+")
        .unwrap()
        .id
        .clone();
    macro_patch.connections.retain(|connection| {
        !(connection.from_node == sig_node_id && connection.to_node == plus_node_id)
    });
    macro_patch.nodes.push(PatchNode {
        id: "created-mul".to_string(),
        op: "*".to_string(),
        kind: NodeKind::Builtin,
        label: "* 1".to_string(),
        args: vec![ArgValue::ConnectedExpr, ArgValue::Literal("1".to_string())],
        outputs: vec!["out".to_string()],
        position: (0.0, 0.0),
        diagnostic: None,
        source: None,
    });
    macro_patch.connections.push(PatchConnection {
        from_node: sig_node_id,
        from_output: 0,
        to_node: "created-mul".to_string(),
        to_input: 0,
        kind: ConnectionKind::Forward,
        segment: None,
        source: None,
    });
    macro_patch.connections.push(PatchConnection {
        from_node: "created-mul".to_string(),
        from_output: 0,
        to_node: plus_node_id,
        to_input: 0,
        kind: ConnectionKind::Forward,
        segment: None,
        source: None,
    });

    assert_eq!(
        emit_patch_debug_lisp_for_view("macro:ap", &macro_patch),
        "(defmacro ap (sig g)\n  (def node (+ (* sig 1) (* h g)))\n  node)"
    );
}

#[test]
fn writeback_emits_unchanged_root_patch_as_complete_normalized_lisp() {
    let source = r#"
        (param freq)
        (def result (phasor freq))
        (out result 1)
    "#;

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(param freq)\n(def result (phasor freq))\n(out result 1.0)"
    );
}

#[test]
fn writeback_emits_unchanged_macro_as_complete_normalized_defmacro() {
    let source = r#"
        (defmacro ap (sig g)
          (def node (+ sig g))
          node)
    "#;

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(defmacro ap (sig g) (def node (+ sig g)) node)"
    );
}

#[test]
fn writeback_node_text_edit_rewrites_owning_root_expression() {
    let source = r#"
        (param freq)
        (def result (phasor freq))
    "#;
    let patch = parse(source);
    let phasor = patch.nodes.iter().find(|node| node.op == "phasor").unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", phasor, "phasor".to_string());
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &phasor.id))
        .unwrap()
        .text = "sin".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param freq)\n(def result (sin freq))"
    );
}

#[test]
fn writeback_root_param_rename_updates_resolved_references_only() {
    let source = r#"
        (def unresolved (phasor freq))
        (param freq @min 1 @max 100)
        (def a (phasor freq))
        (def b (+ freq a))
    "#;
    let patch = parse(source);
    let param = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", param, node_display_label(param));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param.id))
        .unwrap()
        .text = "param cutoff @min 1 @max 100".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def unresolved (phasor freq))\n(param cutoff @min 1.0 @max 100.0)\n(def a (phasor cutoff))\n(def b (+ cutoff a))"
    );
}

#[test]
fn writeback_root_param_rename_collision_returns_blocker() {
    let source = r#"
        (param freq)
        (param cutoff)
        (def result (phasor freq))
    "#;
    let patch = parse(source);
    let param = patch.nodes.iter().find(|node| node.id == "freq").unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", param, node_display_label(param));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param.id))
        .unwrap()
        .text = "param cutoff".to_string();

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::BindingRenameCollision { name, .. }) if name == "cutoff"
    ));
}

#[test]
fn writeback_root_param_rename_with_code_island_returns_blocker() {
    let source = r#"
        (if gate freq 0)
        (param freq)
        (def result (phasor freq))
    "#;
    let patch = parse(source);
    let param = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", param, node_display_label(param));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param.id))
        .unwrap()
        .text = "param cutoff".to_string();

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::BindingRenameBlockedByCodeIsland { name, .. }) if name == "freq"
    ));
}

#[test]
fn writeback_nested_node_text_edit_preserves_nested_structure() {
    let source = "(def result (phasor (* 25 freq) (mix xyz a b)))";
    let patch = parse(source);
    let nested = patch.nodes.iter().find(|node| node.op == "mix").unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", nested, "mix".to_string());
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &nested.id))
        .unwrap()
        .text = "+".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def result (phasor (* 25.0 freq) (+ xyz a b)))"
    );
}

#[test]
fn writeback_macro_node_text_edit_rewrites_inside_defmacro() {
    let source = r#"
        (defmacro ap (sig g)
          (def node (+ sig g))
          node)
    "#;
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let plus = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.op == "+")
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "macro:ap", plus, "+".to_string());
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:ap", &plus.id))
        .unwrap()
        .text = "mix".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro ap (sig g) (def node (mix sig g)) node)"
    );
}

#[test]
fn writeback_synthetic_macro_return_out_is_not_emitted() {
    let source = "(defmacro passthrough (sig) sig)";

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(defmacro passthrough (sig) sig)"
    );
}

#[test]
fn writeback_untouched_code_island_emits_normalized_source() {
    let source = r#"
        (let ((x 1)) x)
        (param freq)
    "#;

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(let ((x 1.0)) x)\n(param freq)"
    );
}

#[test]
fn writeback_edited_code_island_returns_blocker() {
    let source = "(let ((x 1)) x)";
    let patch = parse(source);
    let code = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::CodeIsland)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", code, node_display_label(code));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &code.id))
        .unwrap()
        .text = "code changed".to_string();

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::EditedCodeIsland { .. })
    ));
}

#[test]
fn writeback_unknown_operator_edit_returns_blocker() {
    let source = "(def result (phasor freq))";
    let patch = parse(source);
    let phasor = patch.nodes.iter().find(|node| node.op == "phasor").unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", phasor, "phasor".to_string());
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &phasor.id))
        .unwrap()
        .text = "definitely-not-an-op".to_string();

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::UnknownOperator { operator, .. })
            if operator == "definitely-not-an-op"
    ));
}

#[test]
fn writeback_created_node_returns_phase_boundary_blocker() {
    let source = "(def result (phasor freq))";
    let mut state = PatcherInteractionState::default();
    allocate_created_node(&mut state, "root", (1.0, 1.0));

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::UnsupportedCreatedNode { .. })
    ));
}

#[test]
fn writeback_existing_feedforward_history_round_trips() {
    let source = r#"
        (make-history h)
        (def sig (in 1))
        (def delta (- sig (read-history h)))
        (write-history h sig)
    "#;

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(make-history h)\n(def sig (in 1.0))\n(def delta (- sig (read-history h)))\n(write-history h sig)"
    );
}

#[test]
fn writeback_existing_feedback_history_round_trips() {
    let source = r#"
        (make-history h)
        (write-history h (mix sig (read-history h) alpha))
    "#;

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(make-history h)\n(write-history h (mix sig (read-history h) alpha))"
    );
}

#[test]
fn writeback_created_history_uses_generated_name_for_make_read_and_write() {
    let source = r#"
        (def sig (in 1))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let history_id = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &history_id))
        .unwrap()
        .text = "history".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: history_id.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: history_id,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(make-history history1)\n(def sig (in 1.0))\n(out (read-history history1) 1.0)\n(write-history history1 sig)"
    );
}

#[test]
fn writeback_created_macro_history_emits_inside_defmacro() {
    let source = "(defmacro ap (sig) sig)";
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let sig = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.outputs == vec!["sig".to_string()])
        .unwrap();
    let out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = macro_patch
        .patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let history_id = allocate_created_node(&mut state, "macro:ap", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:ap", &history_id))
        .unwrap()
        .text = "history".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:ap",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "macro:ap",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: history_id.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:ap",
        OutputPortRef {
            node_id: history_id,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro ap (sig) (make-history history1) (write-history history1 sig) (read-history history1))"
    );
}

#[test]
fn writeback_multiple_history_writes_return_blocker() {
    let source = r#"
        (make-history h)
        (def sig (in 1))
        (write-history h sig)
        (write-history h 0)
    "#;

    assert!(matches!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        ),
        Err(WriteBackError::MultipleHistoryWrites { history_id, .. }) if history_id == "h"
    ));
}

#[test]
fn writeback_generated_binding_uses_existing_high_water_suffix() {
    let source = r#"
        (def sig (in 1))
        (def phasor1 (phasor sig))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created))
        .unwrap()
        .text = "phasor".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: created.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1.0))\n(def phasor2 (phasor sig))\n(def phasor1 (phasor sig))\n(out phasor2 1.0)"
    );
}

#[test]
fn writeback_generated_binding_avoids_scope_name_collisions() {
    let source = r#"
        (param phasor1)
        (make-history phasor2)
        (defmacro phasor3 (sig) sig)
        (def sig (in 1))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created))
        .unwrap()
        .text = "phasor".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: created.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param phasor1)\n(make-history phasor2)\n(defmacro phasor3 (sig) sig)\n(def sig (in 1.0))\n(def phasor4 (phasor sig))\n(out phasor4 1.0)"
    );
}

#[test]
fn writeback_macro_generated_binding_uses_macro_local_scope() {
    let source = r#"
        (def phasor1 (phasor 1))
        (defmacro ap (sig) sig)
    "#;
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let sig = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.outputs == vec!["sig".to_string()])
        .unwrap();
    let out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = macro_patch
        .patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "macro:ap", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:ap", &created))
        .unwrap()
        .text = "phasor".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:ap",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "macro:ap",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: created.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:ap",
        OutputPortRef {
            node_id: created,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def phasor1 (phasor 1.0))\n(defmacro ap (sig) (def phasor1 (phasor sig)) phasor1)"
    );
}

#[test]
fn writeback_shared_created_node_emits_one_generated_def_and_multiple_refs() {
    let source = r#"
        (def sig (in 1))
        (out sig 1)
        (def clipped (clip sig 0 1))
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let clip = patch.nodes.iter().find(|node| node.op == "clip").unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let sig_to_clip = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == clip.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created))
        .unwrap()
        .text = "phasor".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_clip),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: created.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created,
            output_index: 0,
        },
        InputPortRef {
            node_id: clip.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1.0))\n(def phasor1 (phasor sig))\n(out phasor1 1.0)\n(def clipped (clip phasor1 0.0 1.0))"
    );
}

#[test]
fn writeback_cable_create_updates_destination_semantic_arg() {
    let source = r#"
        (def a (in 1))
        (def b (in 2))
        (def result (+ a 1))
    "#;
    let patch = parse(source);
    let b = patch.nodes.iter().find(|node| node.id == "b").unwrap();
    let plus = patch.nodes.iter().find(|node| node.op == "+").unwrap();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: b.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: plus.id.clone(),
            input_index: 1,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def a (in 1.0))\n(def b (in 2.0))\n(def result (+ a b))"
    );
}

#[test]
fn writeback_cable_create_uses_semantic_arg_index_with_attributes() {
    let source = r#"
        (def a (in 1))
        (def b (in 2))
        (def result (foo a @mode fast 1))
    "#;
    let patch = parse(source);
    let b = patch.nodes.iter().find(|node| node.id == "b").unwrap();
    let foo = patch.nodes.iter().find(|node| node.op == "foo").unwrap();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: b.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: foo.id.clone(),
            input_index: 1,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def a (in 1.0))\n(def b (in 2.0))\n(def result (foo a @mode fast b))"
    );
}

#[test]
fn writeback_source_cable_rewire_replaces_destination_arg_once() {
    let source = r#"
        (def a (in 1))
        (def b (in 2))
        (def result (+ a 1))
    "#;
    let patch = parse(source);
    let a = patch.nodes.iter().find(|node| node.id == "a").unwrap();
    let b = patch.nodes.iter().find(|node| node.id == "b").unwrap();
    let plus = patch.nodes.iter().find(|node| node.op == "+").unwrap();
    let a_to_plus = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == a.id && connection.to_node == plus.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(a_to_plus),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: b.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: plus.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def a (in 1.0))\n(def b (in 2.0))\n(def result (+ b 1.0))"
    );
}

#[test]
fn writeback_cable_delete_in_root_emits_missing_input_sentinel() {
    let source = r#"
        (def sig (in 1))
        (out sig 1)
    "#;
    let patch = parse(source);
    let connection = patch.connections.first().unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(connection),
        ));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1.0))\n(out __patcher_missing_input__ 1.0)"
    );
}

#[test]
fn writeback_cable_delete_in_macro_emits_missing_input_sentinel() {
    let source = "(defmacro ap (sig) (out sig 1))";
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let connection = macro_patch.patch.connections.first().unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:ap",
            &source_connection_id(connection),
        ));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro ap (sig) (out __patcher_missing_input__ 1.0))"
    );
}

#[test]
fn writeback_deleting_source_backed_top_level_node_removes_form() {
    let source = r#"
        (def sig (in 1))
        (def result (phasor sig))
        (out result 1)
    "#;
    let patch = parse(source);
    let result = patch.nodes.iter().find(|node| node.id == "result").unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &result.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1.0))\n(out result 1.0)"
    );
}

#[test]
fn writeback_deleting_multiple_top_level_nodes_removes_expected_forms() {
    let source = r#"
        (def sig (in 1))
        (def carrier (phasor sig))
        (def shaped (* carrier 0.5))
        (out shaped 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let shaped = patch.nodes.iter().find(|node| node.id == "shaped").unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &sig.id));
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &shaped.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def carrier (phasor sig))\n(out shaped 1.0)"
    );
}

#[test]
fn writeback_deleted_top_level_node_ignores_incident_deleted_connections() {
    let source = r#"
        (def sig (in 1))
        (def result (phasor sig))
        (out result 1)
    "#;
    let patch = parse(source);
    let result = patch.nodes.iter().find(|node| node.id == "result").unwrap();
    let result_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == result.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &result.id));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(result_to_out),
        ));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1.0))\n(out result 1.0)"
    );
}

#[test]
fn writeback_deleting_nested_source_node_replaces_it_with_missing_input() {
    let source = r#"
        (def sig (in 1))
        (def result (phasor (* sig 2)))
    "#;
    let patch = parse(source);
    let nested = patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &nested.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1.0))\n(def result (phasor __patcher_missing_input__))"
    );
}

#[test]
fn writeback_deleting_nested_source_node_does_not_promote_its_input() {
    let source = "(def result (phasor (* (noise) 2)))";
    let patch = parse(source);
    let nested = patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &nested.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def result (phasor __patcher_missing_input__))"
    );
}

#[test]
fn literal_args_are_inlined_and_do_not_create_visible_ports() {
    let patch = parse(
        r#"
            (def signal (in 1 @name signal))
            (out (* signal 3) 1 @name audio)
            "#,
    );
    let multiply = patch
        .nodes
        .iter()
        .find(|node| node.op == "*")
        .expect("anonymous multiply node");

    assert_eq!(node_display_label(multiply), "* 3");

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&multiply.id).map(Vec::as_slice),
        Some(&[0][..])
    );
}

#[test]
fn inline_args_keep_placeholders_for_connected_args_before_later_literals() {
    let patch = parse(
        r#"
            (defmacro ap (sig g d) sig)
            (def signal (in 1))
            (def gain (in 2))
            (def tapped (ap signal gain 0.6))
            "#,
    );
    let ap = patch
        .nodes
        .iter()
        .find(|node| node.op == "ap")
        .expect("macro instance node");

    assert_eq!(node_display_label(ap), "ap ? 0.6");

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&ap.id).map(Vec::as_slice),
        Some(&[0, 1][..])
    );

    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    assert_eq!(input_slot_counts.get(&ap.id).copied(), Some(2));
}

#[test]
fn leading_numeric_constants_become_nodes_to_preserve_input_order() {
    let patch = parse(
        r#"
            (defmacro ap (sig g d) sig)
            (def signal (in 1))
            (def delay (in 2))
            (def tapped (ap signal 0.6 delay))
            "#,
    );
    let ap = patch
        .nodes
        .iter()
        .find(|node| node.op == "ap")
        .expect("macro instance node");

    let constant = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Constant && node.op == "0.6")
        .expect("numeric constant node");

    assert_eq!(node_display_label(constant), "0.6");
    assert_eq!(node_display_label(ap), "ap");

    assert!(patch.connections.iter().any(|connection| {
        connection.from_node == constant.id
            && connection.to_node == ap.id
            && connection.to_input == 1
    }));

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&ap.id).map(Vec::as_slice),
        Some(&[0, 1, 2][..])
    );

    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    assert_eq!(input_slot_counts.get(&ap.id).copied(), Some(3));
}

#[test]
fn trailing_constants_inline_without_reserving_visible_input_slots() {
    let patch = parse(
        r#"
            (def pitch (in 1))
            (def phase (phasor pitch))
            (def radians (* phase twopi))
            "#,
    );
    let multiply = patch
        .nodes
        .iter()
        .find(|node| node.op == "*")
        .expect("multiply node");

    assert_eq!(node_display_label(multiply), "* twopi");

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&multiply.id).map(Vec::as_slice),
        Some(&[0][..])
    );

    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    assert_eq!(input_slot_counts.get(&multiply.id).copied(), Some(1));
}

#[test]
fn leading_constants_become_nodes_to_preserve_input_order() {
    let patch = parse(
        r#"
            (def pitch (in 1))
            (def phase (phasor pitch))
            (def radians (* twopi phase))
            "#,
    );
    let constant = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Constant && node.op == "twopi")
        .expect("twopi constant node");
    let multiply = patch
        .nodes
        .iter()
        .find(|node| node.op == "*")
        .expect("multiply node");

    assert_eq!(node_display_label(constant), "twopi");
    assert_eq!(node_display_label(multiply), "*");

    assert!(patch.connections.iter().any(|connection| {
        connection.from_node == constant.id
            && connection.to_node == multiply.id
            && connection.to_input == 0
    }));
    assert!(
        patch
            .connections
            .iter()
            .any(|connection| connection.to_node == multiply.id && connection.to_input == 1)
    );

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&multiply.id).map(Vec::as_slice),
        Some(&[0, 1][..])
    );
}

#[test]
fn standalone_constant_defs_project_as_constant_nodes() {
    let patch = parse(
        r#"
            (def radians twopi)
            (out radians 1)
            "#,
    );
    let constant = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Constant && node.op == "twopi")
        .expect("twopi constant node");

    assert_eq!(node_display_label(constant), "twopi");
    assert!(patch.connections.iter().any(|connection| {
        connection.from_node == constant.id
            && patch
                .nodes
                .iter()
                .any(|node| node.id == connection.to_node && node.kind == NodeKind::Out)
    }));
}

#[test]
fn display_labels_omit_def_names_and_show_in_out_channels() {
    let patch = parse(
        r#"
            (def signal (in 1 @name pitch))
            (def scaled (* signal 3))
            (out scaled 1 @name audio)
            "#,
    );
    let input = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let multiply = patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let output = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();

    assert_eq!(node_display_label(input), "in 1");
    assert_eq!(node_display_label(multiply), "* 3");
    assert_eq!(node_display_label(output), "out 1");
}

#[test]
fn interaction_positions_override_auto_layout_positions() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let pitch = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
    assert_ne!(pitch.position, (22.0, 7.0));

    let mut state = PatcherInteractionState::default();
    set_node_edit_position(
        &mut state,
        "root",
        pitch,
        (22.0, 7.0),
        node_display_label(pitch),
    );
    let patch = patch_with_interaction_state(patch, &state, "root");
    let pitch = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
    assert_eq!(pitch.position, (22.0, 7.0));
}

#[test]
fn patcher_hit_testing_uses_node_rects_after_pan() {
    let patch = parse("(def pitch (in 1 @name pitch))");
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let hit = hit_patcher_node(&patch, rect, &pan, 7.0, 6.8);
    assert_eq!(hit.as_deref(), Some("pitch"));
}

#[test]
fn patcher_output_port_hit_testing_uses_rendered_port_positions() {
    let patch = parse("(def pitch (in 1 @name pitch))");
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rect = *patch_node_rects(&patch, rect, &pan).get("pitch").unwrap();
    let output_counts = patch_output_counts(&patch);
    let center = port_center(node_rect, 0, output_counts["pitch"], false);

    let hit = hit_patcher_output_port(
        &patch,
        rect,
        &pan,
        &output_counts,
        center.0,
        center.1,
        10.0,
        20.0,
    );
    assert_eq!(
        hit,
        Some(OutputPortRef {
            node_id: "pitch".to_string(),
            output_index: 0,
        })
    );
}

#[test]
fn patcher_output_port_hit_testing_matches_rendered_circle() {
    let patch = parse("(def pitch (in 1 @name pitch))");
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rect = *patch_node_rects(&patch, rect, &pan).get("pitch").unwrap();
    let output_counts = patch_output_counts(&patch);
    let center = port_center(node_rect, 0, output_counts["pitch"], false);
    let radius_rows = (PORT_OUTER_DIAMETER_PX * 0.5) / 20.0;

    let inside_rendered_circle = hit_patcher_output_port(
        &patch,
        rect,
        &pan,
        &output_counts,
        center.0,
        center.1 - radius_rows + 0.01,
        10.0,
        20.0,
    );
    assert_eq!(
        inside_rendered_circle,
        Some(OutputPortRef {
            node_id: "pitch".to_string(),
            output_index: 0,
        })
    );

    let inside_node_body_but_outside_port_circle = hit_patcher_output_port(
        &patch,
        rect,
        &pan,
        &output_counts,
        center.0,
        center.1 - radius_rows - 0.01,
        10.0,
        20.0,
    );
    assert_eq!(inside_node_body_but_outside_port_circle, None);
}

#[test]
fn nearest_patcher_input_port_respects_max_distance() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let phasor_rect = *node_rects
        .iter()
        .find_map(|(node_id, rect)| {
            patch
                .nodes
                .iter()
                .find(|node| node.id == *node_id && node.op == "phasor")
                .map(|_| rect)
        })
        .unwrap();
    let phasor_id = patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap()
        .id
        .clone();
    let center = port_center(phasor_rect, 0, input_slot_counts[&phasor_id], true);
    let source = OutputPortRef {
        node_id: "pitch".to_string(),
        output_index: 0,
    };

    let near = nearest_patcher_input_port(
        &patch,
        rect,
        &pan,
        &input_indices,
        &input_slot_counts,
        &source,
        center.0 + 0.2,
        center.1 + 0.2,
    );
    assert_eq!(
        near,
        Some(InputPortRef {
            node_id: phasor_id.clone(),
            input_index: 0,
        })
    );

    let far = nearest_patcher_input_port(
        &patch,
        rect,
        &pan,
        &input_indices,
        &input_slot_counts,
        &source,
        center.0 + 20.0,
        center.1 + 20.0,
    );
    assert_eq!(far, None);
}

#[test]
fn created_connections_are_applied_to_working_patch() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor))
            "#,
    );
    let phasor_id = patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap()
        .id
        .clone();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: "pitch".to_string(),
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor_id.clone(),
            input_index: 0,
        },
    );

    let patch = patch_with_interaction_state(patch, &state, "root");
    assert!(patch.connections.iter().any(|connection| {
        connection.from_node == "pitch"
            && connection.from_output == 0
            && connection.to_node == phasor_id
            && connection.to_input == 0
            && connection.kind == ConnectionKind::Forward
    }));
}

#[test]
fn patcher_cable_hit_testing_uses_rendered_curve() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let connection = patch.connections.first().expect("source connection");
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let (start, end) = connection_endpoints(
        connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .expect("rendered connection endpoints");
    let curve = super::super::cable::cable_curve(start, end);
    let midpoint = super::super::cable::cubic_bezier_point(curve, 0.5);

    assert_eq!(
        hit_patcher_cable(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            midpoint.0,
            midpoint.1,
        )
        .as_deref(),
        Some(source_connection_id(connection).as_str())
    );
}

#[test]
fn segmented_cable_hit_testing_uses_orthogonal_path() {
    let mut patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let to_node = patch.connections.first().unwrap().to_node.clone();
    patch
        .nodes
        .iter_mut()
        .find(|node| node.id == to_node)
        .unwrap()
        .position
        .0 += 12.0;
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let (start, end) = connection_endpoints(
        patch.connections.first().unwrap(),
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    let origin = patcher_origin(rect, &pan);
    let rendered_segment_row = (start.1 + end.1) * 0.5;
    let segment_row = rendered_segment_row - origin.1;
    patch.connections[0].segment = Some(CableSegmentInfo {
        is_segmented: true,
        segment_row,
    });
    let horizontal_midpoint = ((start.0 + end.0) * 0.5, rendered_segment_row);

    assert_eq!(
        hit_patcher_cable(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            horizontal_midpoint.0,
            horizontal_midpoint.1,
        )
        .as_deref(),
        Some(source_connection_id(patch.connections.first().unwrap()).as_str())
    );
}

#[test]
fn segmented_horizontal_segment_hit_is_used_for_dragging() {
    let mut patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let to_node = patch.connections.first().unwrap().to_node.clone();
    patch
        .nodes
        .iter_mut()
        .find(|node| node.id == to_node)
        .unwrap()
        .position
        .0 += 12.0;
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let (start, end) = connection_endpoints(
        patch.connections.first().unwrap(),
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    let origin = patcher_origin(rect, &pan);
    let rendered_segment_row = (start.1 + end.1) * 0.5;
    let segment_row = rendered_segment_row - origin.1;
    patch.connections[0].segment = Some(CableSegmentInfo {
        is_segmented: true,
        segment_row,
    });

    assert_eq!(
        hit_patcher_segmented_cable_horizontal_segment(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            (start.0 + end.0) * 0.5,
            rendered_segment_row,
        )
        .as_deref(),
        Some(source_connection_id(patch.connections.first().unwrap()).as_str())
    );
}

#[test]
fn segment_row_drag_clamps_normal_and_wraparound_cases() {
    let normal = super::super::cable::segment_row_for_drag(
        (10.0, 10.0),
        (20.0, 20.0),
        30.0,
        SEGMENTED_CABLE_DRAG_PADDING_CELLS,
        SEGMENTED_CABLE_DRAG_EXTRA_RANGE_CELLS,
    );
    assert!(normal < 20.0, "{normal}");

    let wrap = super::super::cable::segment_row_for_drag(
        (10.0, 20.0),
        (20.0, 10.0),
        40.0,
        SEGMENTED_CABLE_DRAG_PADDING_CELLS,
        SEGMENTED_CABLE_DRAG_EXTRA_RANGE_CELLS,
    );
    assert!(wrap > 20.0, "{wrap}");
}

#[test]
fn super_y_toggles_selected_cable_segmentation() {
    let source = r#"
        (def pitch (in 1 @name pitch))
        (def sig (phasor pitch))
    "#;
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-segment-toggle-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let patch = parse(source);
    let selected_cable = source_connection_id(patch.connections.first().unwrap());
    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 778_899,
        stable_widget_id: Some(778_899),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(
        key,
        PatcherInteractionState {
            selected_cable: Some(selected_cable.clone()),
            ..Default::default()
        },
    );

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char('y'),
                    modifiers: KeyModifiers::SUPER,
                },
            )
            .is_some()
    );

    let state = get_patcher_interaction_state(key);
    let patch = patch_with_interaction_state(patch, &state, "root");
    let segment = patch
        .connections
        .iter()
        .find(|connection| source_connection_id(connection) == selected_cable)
        .and_then(|connection| connection.segment)
        .unwrap();
    assert!(segment.is_segmented);
    assert_ne!(segment.segment_row, 0.0);
}

#[test]
#[cfg(target_os = "macos")]
fn segmented_cable_render_row_tracks_pan_origin_once() {
    let source = r#"
        (def pitch (in 1 @name pitch))
        (def sig (phasor pitch))
    "#;
    let mut patch = parse(source);
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let pan_for_layout = PatcherPanState::default();
    let connection = patch.connections.first().unwrap().clone();
    patch
        .nodes
        .iter_mut()
        .find(|node| node.id == connection.to_node)
        .unwrap()
        .position
        .0 += 12.0;
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let node_rects = patch_node_rects(&patch, rect, &pan_for_layout);
    let (start, end) = connection_endpoints(
        &connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    assert!(super::super::cable::should_render_segmented_cable(
        start, end
    ));
    let origin = patcher_origin(rect, &pan_for_layout);
    let stored_segment_row = ((start.1 + end.1) * 0.5) - origin.1;
    patch.connections[0].segment = Some(CableSegmentInfo {
        is_segmented: true,
        segment_row: stored_segment_row,
    });
    let state = PatcherInteractionState::default();

    let mut pan = PatcherPanState {
        viewport_width: rect.width,
        viewport_height: rect.height,
        content_width: 200.0,
        content_height: 200.0,
        ..Default::default()
    };
    let first_origin = patcher_origin(rect, &pan);
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        rect,
        WidgetViewport {
            vp_w: 800.0,
            vp_h: 600.0,
            cell_w: 10.0,
            cell_h: 20.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 30.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &pan,
        &state,
    );
    let first_segment_row = prims
        .iter()
        .find_map(|prim| match prim {
            MetalPrimitive::PatchCable(cable) if cable.is_segmented => Some(cable.segment_row),
            _ => None,
        })
        .unwrap();
    assert_eq!(first_segment_row, first_origin.1 + stored_segment_row);

    pan.offset_y = 12.0;
    let second_origin = patcher_origin(rect, &pan);
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        rect,
        WidgetViewport {
            vp_w: 800.0,
            vp_h: 600.0,
            cell_w: 10.0,
            cell_h: 20.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 30.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &pan,
        &state,
    );
    let second_segment_row = prims
        .iter()
        .find_map(|prim| match prim {
            MetalPrimitive::PatchCable(cable) if cable.is_segmented => Some(cable.segment_row),
            _ => None,
        })
        .unwrap();
    assert_eq!(second_segment_row, second_origin.1 + stored_segment_row);
    assert_eq!(
        second_segment_row - first_segment_row,
        second_origin.1 - first_origin.1
    );
}

#[test]
#[cfg(target_os = "macos")]
fn segmented_cable_rendering_collapses_aligned_ports_to_vertical_curve() {
    let source = r#"
        (def pitch (in 1 @name pitch))
        (def sig (phasor pitch))
    "#;
    let mut patch = parse(source);
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let (start, end) = connection_endpoints(
        patch.connections.first().unwrap(),
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    assert!(!super::super::cable::should_render_segmented_cable(
        start, end
    ));
    let origin = patcher_origin(rect, &pan);
    patch.connections[0].segment = Some(CableSegmentInfo {
        is_segmented: true,
        segment_row: ((start.1 + end.1) * 0.5) - origin.1,
    });

    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        rect,
        WidgetViewport {
            vp_w: 800.0,
            vp_h: 600.0,
            cell_w: 10.0,
            cell_h: 20.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 30.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &pan,
        &PatcherInteractionState::default(),
    );

    let cable = prims
        .iter()
        .find_map(|prim| match prim {
            MetalPrimitive::PatchCable(cable) => Some(cable),
            _ => None,
        })
        .unwrap();
    assert!(!cable.is_segmented);
    assert_eq!(cable.start[0], cable.end[0]);
    assert_eq!(cable.control1[0], cable.start[0]);
    assert_eq!(cable.control2[0], cable.end[0]);
}

#[test]
fn selected_source_cable_delete_marks_connection_deleted() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let connection_id = source_connection_id(patch.connections.first().unwrap());
    let mut state = PatcherInteractionState {
        selected_cable: Some(connection_id.clone()),
        ..Default::default()
    };

    delete_connection_edit_or_mark_deleted(&mut state, "root", &connection_id);

    assert_eq!(state.selected_cable, None);
    assert!(
        state
            .edit_state
            .deleted_connections
            .contains(&connection_edit_key("root", &connection_id))
    );
    let patch = patch_with_interaction_state(patch, &state, "root");
    assert!(patch.connections.is_empty(), "{:#?}", patch.connections);
}

#[test]
fn selected_created_cable_delete_removes_connection_edit() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor))
            "#,
    );
    let phasor_id = patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap()
        .id
        .clone();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: "pitch".to_string(),
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor_id,
            input_index: 0,
        },
    );
    let patch = patch_with_interaction_state(patch, &state, "root");
    let connection_id = source_connection_id(patch.connections.first().unwrap());
    state.selected_cable = Some(connection_id.clone());

    delete_connection_edit_or_mark_deleted(&mut state, "root", &connection_id);

    assert_eq!(state.selected_cable, None);
    assert!(state.edit_state.connections.is_empty());
    assert!(state.edit_state.deleted_connections.is_empty());
}

#[test]
fn selected_source_node_delete_hides_node_and_incident_connections() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let mut state = PatcherInteractionState::default();
    state.selected_nodes.insert("pitch".to_string());

    assert!(delete_selected_nodes(&mut state, "root"));

    assert!(state.selected_nodes.is_empty());
    assert!(
        state
            .edit_state
            .deleted_nodes
            .contains(&node_edit_key("root", "pitch"))
    );
    let patch = patch_with_interaction_state(patch, &state, "root");
    assert!(!patch.nodes.iter().any(|node| node.id == "pitch"));
    assert!(
        patch
            .connections
            .iter()
            .all(|connection| connection.from_node != "pitch" && connection.to_node != "pitch"),
        "{:#?}",
        patch.connections
    );
}

#[test]
fn selected_created_node_delete_removes_node_and_created_connections() {
    let patch = parse(
        r#"
            (def sig (phasor))
            "#,
    );
    let phasor_id = patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap()
        .id
        .clone();
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (0.0, 0.0));
    if let Some(edit) = state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
    {
        edit.text = "param freq".to_string();
    }
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created_id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor_id,
            input_index: 0,
        },
    );
    state.selected_nodes.insert(created_id.clone());

    assert!(delete_selected_nodes(&mut state, "root"));

    assert!(
        !state
            .edit_state
            .nodes
            .contains_key(&node_edit_key("root", &created_id))
    );
    assert!(state.edit_state.deleted_nodes.is_empty());
    assert!(state.edit_state.connections.is_empty());
    let patch = patch_with_interaction_state(patch, &state, "root");
    assert!(!patch.nodes.iter().any(|node| node.id == created_id));
    assert!(patch.connections.is_empty());
}

#[test]
fn selected_cable_handles_are_hit_near_rendered_edit_points() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let connection = patch.connections.first().expect("source connection");
    let connection_id = source_connection_id(connection);
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let (start, end) = connection_endpoints(
        connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .expect("rendered connection endpoints");
    let (from_handle, to_handle) = cable_edit_points(start, end);

    assert_eq!(
        hit_patcher_cable_handle(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            Some(&connection_id),
            from_handle.0,
            from_handle.1,
        ),
        Some((connection_id.clone(), CableEndpoint::From))
    );
    assert_eq!(
        hit_patcher_cable_handle(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            Some(&connection_id),
            to_handle.0,
            to_handle.1,
        ),
        Some((connection_id, CableEndpoint::To))
    );
}

#[test]
fn dragging_selected_cable_endpoint_reconnects_and_keeps_cable_selected() {
    let source = r#"
            (def pitch (in 1 @name pitch))
            (def gate (in 2 @name gate))
            (def sig (phasor pitch))
            "#;
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-cable-endpoint-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let original_connection = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == "pitch")
        .cloned()
        .unwrap();
    let original_connection_id = source_connection_id(&original_connection);
    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 445_566,
        stable_widget_id: Some(445_566),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(
        key,
        PatcherInteractionState {
            selected_cable: Some(original_connection_id.clone()),
            ..Default::default()
        },
    );
    let pan = PatcherPanState::default();
    let node_rects = patch_node_rects(&root_patch, node.rect, &pan);
    let input_indices = patch_input_indices(&root_patch);
    let input_slot_counts = patch_input_slot_counts(&root_patch, &input_indices);
    let output_counts = patch_output_counts(&root_patch);
    let (start, end) = connection_endpoints(
        &original_connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    let (from_handle, _) = cable_edit_points(start, end);
    let gate_rect = node_rects.get("gate").unwrap();
    let gate_output = port_center(*gate_rect, 0, output_counts["gate"], false);

    handle_patcher_pointer_down(
        &node,
        from_handle.0,
        from_handle.1,
        KeyModifiers::empty(),
        10.0,
        20.0,
    );
    handle_patcher_pointer_drag(&node, gate_output.0, gate_output.1);
    handle_patcher_pointer_up(&node, gate_output.0, gate_output.1);

    let state = get_patcher_interaction_state(key);
    let edited_patch = patch_with_interaction_state(root_patch, &state, "root");
    let new_connection_id = connection_id_from_ports(
        &OutputPortRef {
            node_id: "gate".to_string(),
            output_index: 0,
        },
        &InputPortRef {
            node_id: original_connection.to_node.clone(),
            input_index: original_connection.to_input,
        },
    );
    assert_eq!(
        state.selected_cable.as_deref(),
        Some(new_connection_id.as_str())
    );
    assert!(!edited_patch.connections.iter().any(|connection| {
        connection.from_node == "pitch" && connection.to_node == original_connection.to_node
    }));
    assert!(edited_patch.connections.iter().any(|connection| {
        connection.from_node == "gate" && connection.to_node == original_connection.to_node
    }));

    let _ = std::fs::remove_file(path);
}

#[test]
fn pan_state_allows_overscroll_and_clamps_to_finite_canvas_bounds() {
    let mut state = PatcherPanState {
        offset_x: 100.0,
        offset_y: 100.0,
        content_width: 50.0,
        content_height: 30.0,
        viewport_width: 20.0,
        viewport_height: 10.0,
    };
    clamp_patcher_pan_state(&mut state);
    assert_eq!(state.offset_x, 78.0);
    assert_eq!(state.offset_y, 68.0);

    state.offset_x = -200.0;
    state.offset_y = -200.0;
    clamp_patcher_pan_state(&mut state);
    assert_eq!(state.offset_x, -48.0);
    assert_eq!(state.offset_y, -48.0);
}

#[test]
fn touchpad_horizontal_pan_matches_canvas_drag_direction() {
    let node = LayoutNode {
        widget_id: 987_655,
        stable_widget_id: Some(987_655),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 20.0,
            height: 10.0,
        },
        props: HashMap::new(),
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);

    PATCHER_WIDGET.scroll_gesture_event(&node, 10.0, 5.0, 100.0, 0.0);

    let state = get_patcher_pan_state(key);
    assert!(
        state.offset_x < 0.0,
        "positive horizontal gesture delta should move canvas right, got {}",
        state.offset_x
    );
}

#[test]
fn defmacro_becomes_read_only_subpatch() {
    let patch = parse(
        r#"
            (defmacro ap (x)
              (def y (allpass x 100 0.5))
              y)
            (def z (ap input))
            "#,
    );
    assert_eq!(patch.macros.len(), 1);
    assert_eq!(patch.macros[0].name, "ap");
    let macro_patch = &patch.macros[0].patch;
    assert!(
        macro_patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::In && node_display_label(node) == "in 1"),
        "{:#?}",
        macro_patch.nodes
    );
    assert!(
        macro_patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::Out && node_display_label(node) == "out 1"),
        "{:#?}",
        macro_patch.nodes
    );
    assert!(
        patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::MacroInstance)
    );
    assert!(
        !patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::MacroDefinition),
        "{:#?}",
        patch.nodes
    );
}

#[test]
fn double_clicking_macro_instance_edits_text_and_breadcrumb_returns_to_root() {
    let source = r#"
            (defmacro ap (x)
              (def y (allpass x 100 0.5)))
            (def z (ap input))
        "#;
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-macro-nav-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let macro_node = root_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance)
        .unwrap();

    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 112_233,
        stable_widget_id: Some(112_233),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(key, PatcherInteractionState::default());

    let rects = patch_node_rects(&root_patch, node.rect, &PatcherPanState::default());
    let macro_rect = rects.get(&macro_node.id).unwrap();
    assert!(handle_patcher_double_click(
        &node,
        macro_rect.col + macro_rect.width * 0.5,
        macro_rect.row + macro_rect.height * 0.5
    ));
    let state = get_patcher_interaction_state(key);
    assert_eq!(state.active_macro, None);
    assert_eq!(
        state.text_edit.as_ref().map(|edit| edit.node_id.as_str()),
        Some(macro_node.id.as_str())
    );

    let mut state = get_patcher_interaction_state(key);
    state.text_edit = None;
    state.selected_nodes.insert(macro_node.id.clone());
    set_patcher_interaction_state(key, state);
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert_eq!(
        get_patcher_interaction_state(key).active_macro.as_deref(),
        Some("ap")
    );

    let mut state = get_patcher_interaction_state(key);
    state.active_macro = Some("ap".to_string());
    set_patcher_interaction_state(key, state);
    handle_patcher_pointer_moved(&node, 1.2, 0.8);
    assert!(get_patcher_interaction_state(key).hover_back_button);

    handle_patcher_pointer_down(&node, 1.2, 0.8, KeyModifiers::empty(), 10.0, 20.0);
    assert_eq!(get_patcher_interaction_state(key).active_macro, None);

    let _ = std::fs::remove_file(path);
}

#[test]
fn double_clicking_background_creates_editable_draft_node() {
    let source = "(def pitch (in 1))";
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-draft-node-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 223_344,
        stable_widget_id: Some(223_344),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(key, PatcherInteractionState::default());

    assert!(handle_patcher_double_click(&node, 40.0, 20.0));
    assert!(get_patcher_interaction_state(key).text_edit.is_some());

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char('p'),
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char('h'),
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );

    let state = get_patcher_interaction_state(key);
    assert!(state.text_edit.is_none());
    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    assert_eq!(patch.nodes.len(), 1);
    assert_eq!(node_display_label(&patch.nodes[0]), "ph");
    assert!(matches!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", &patch.nodes[0].id))
            .map(|edit| &edit.origin),
        Some(PatcherNodeOrigin::Created { .. })
    ));

    let _ = std::fs::remove_file(path);
}

#[test]
fn committed_editor_nodes_project_ports_from_operator_metadata() {
    let macro_arities = HashMap::new();
    let phasor = node_from_editor_text("draft", "phasor", (0.0, 0.0), &macro_arities, false);
    assert_eq!(node_display_label(&phasor), "phasor");
    assert_eq!(phasor.outputs.len(), 1);

    let patch = Patch {
        nodes: vec![phasor],
        connections: Vec::new(),
        macros: Vec::new(),
        diagnostics: Vec::new(),
    };
    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get("draft").map(Vec::as_slice),
        Some(&[0][..])
    );

    let multiply = node_from_editor_text("mul", "* 3", (0.0, 0.0), &macro_arities, false);
    assert_eq!(node_display_label(&multiply), "* 3");
    let patch = Patch {
        nodes: vec![multiply],
        connections: Vec::new(),
        macros: Vec::new(),
        diagnostics: Vec::new(),
    };
    let input_indices = patch_input_indices(&patch);
    assert_eq!(input_indices.get("mul").map(Vec::as_slice), Some(&[0][..]));
    let slot_counts = patch_input_slot_counts(&patch, &input_indices);
    assert_eq!(slot_counts.get("mul").copied(), Some(1));

    let history = node_from_editor_text("hist", "history", (0.0, 0.0), &macro_arities, false);
    assert_eq!(history.kind, NodeKind::History);
    assert_eq!(node_display_label(&history), "history");
    assert_eq!(history.diagnostic, None);
    assert_eq!(history.outputs.len(), 1);

    let constant = node_from_editor_text("const", "twopi", (0.0, 0.0), &macro_arities, false);
    assert_eq!(constant.kind, NodeKind::Constant);
    assert_eq!(node_display_label(&constant), "twopi");
    assert_eq!(constant.diagnostic, None);
    assert_eq!(constant.args.len(), 0);
    assert_eq!(constant.outputs.len(), 1);

    let number = node_from_editor_text("num", "3", (0.0, 0.0), &macro_arities, false);
    assert_eq!(number.kind, NodeKind::Constant);
    assert_eq!(node_display_label(&number), "3");
    assert_eq!(number.diagnostic, None);
    assert_eq!(number.args.len(), 0);
    assert_eq!(number.outputs.len(), 1);

    let patch = Patch {
        nodes: vec![history],
        connections: Vec::new(),
        macros: Vec::new(),
        diagnostics: Vec::new(),
    };
    let input_indices = patch_input_indices(&patch);
    assert_eq!(input_indices.get("hist").map(Vec::as_slice), Some(&[0][..]));
}

#[test]
fn actively_edited_created_nodes_suppress_unknown_operator_diagnostics() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (0.0, 0.0));
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id.clone(),
        text: "p".to_string(),
        original_text: String::new(),
        state: TextInputState {
            cursor_pos: 1,
            selection_anchor: None,
            selecting: false,
        },
    });

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    assert_eq!(patch.nodes.len(), 1);
    assert_eq!(node_display_label(&patch.nodes[0]), "p");
    assert_eq!(patch.nodes[0].diagnostic, None);
    assert!(patch_input_indices(&patch).get(&created_id).is_none());
    assert_eq!(patch_output_counts(&patch).get(&created_id), None);
}

#[test]
fn created_node_positions_are_owned_by_the_node_edit() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (1.0, 2.0));
    let edit = state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
        .unwrap();
    edit.text = "phasor".to_string();
    edit.position = (9.0, 8.0);

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    assert_eq!(patch.nodes[0].position, (9.0, 8.0));
}

#[test]
fn double_clicking_node_edits_display_text_in_memory() {
    let source = "(def pitch (in 1))";
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-edit-node-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let pitch = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "pitch")
        .unwrap();
    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 334_455,
        stable_widget_id: Some(334_455),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(key, PatcherInteractionState::default());

    let rects = patch_node_rects(&root_patch, node.rect, &PatcherPanState::default());
    let pitch_rect = rects.get(&pitch.id).unwrap();
    assert!(handle_patcher_double_click(
        &node,
        pitch_rect.col + NODE_TEXT_COL_OFFSET,
        pitch_rect.row + pitch_rect.height * 0.5,
    ));
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char('x'),
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );

    let state = get_patcher_interaction_state(key);
    assert_eq!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", "pitch"))
            .map(|edit| edit.text.as_str()),
        Some("xin 1")
    );
    assert!(matches!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", "pitch"))
            .map(|edit| &edit.origin),
        Some(PatcherNodeOrigin::Source { source_node_id }) if source_node_id == "pitch"
    ));

    let _ = std::fs::remove_file(path);
}

#[test]
fn backspace_without_text_edit_deletes_selected_nodes() {
    let source = r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#;
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-node-delete-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 556_677,
        stable_widget_id: Some(556_677),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState::default();
    state.selected_nodes.insert("pitch".to_string());
    set_patcher_interaction_state(key, state);

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Backspace,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    let state = get_patcher_interaction_state(key);
    assert!(state.text_edit.is_none());
    assert!(state.selected_nodes.is_empty());
    assert!(
        state
            .edit_state
            .deleted_nodes
            .contains(&node_edit_key("root", "pitch"))
    );
    let patch = patch_with_interaction_state(root_patch, &state, "root");
    assert!(
        !patch
            .nodes
            .iter()
            .any(|patch_node| patch_node.id == "pitch")
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn created_node_reedit_updates_same_node_edit_text() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (3.0, 4.0));
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id.clone(),
        text: "phasor".to_string(),
        original_text: String::new(),
        state: TextInputState {
            cursor_pos: 6,
            selection_anchor: None,
            selecting: false,
        },
    });
    commit_patcher_text_edit(&mut state, "root");
    assert_eq!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", &created_id))
            .map(|edit| edit.text.as_str()),
        Some("phasor")
    );

    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id.clone(),
        text: "triangle".to_string(),
        original_text: "phasor".to_string(),
        state: TextInputState {
            cursor_pos: 8,
            selection_anchor: None,
            selecting: false,
        },
    });
    commit_patcher_text_edit(&mut state, "root");
    assert_eq!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", &created_id))
            .map(|edit| edit.text.as_str()),
        Some("triangle")
    );

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    assert_eq!(patch.nodes.len(), 1);
    assert_eq!(node_display_label(&patch.nodes[0]), "triangle");
}

#[test]
fn source_and_created_nodes_share_one_edit_model() {
    let source = parse("(def pitch (in 1))");
    let pitch = source.nodes.iter().find(|node| node.id == "pitch").unwrap();
    let mut state = PatcherInteractionState::default();
    set_node_edit_position(
        &mut state,
        "root",
        pitch,
        (12.0, 3.0),
        node_display_label(pitch),
    );
    let created_id = allocate_created_node(&mut state, "root", (4.0, 5.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
        .unwrap()
        .text = "* 3".to_string();

    let patch = patch_with_interaction_state(source, &state, "root");
    let source_node = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
    let created_node = patch
        .nodes
        .iter()
        .find(|node| node.id == created_id)
        .unwrap();
    assert_eq!(source_node.position, (12.0, 3.0));
    assert_eq!(node_display_label(created_node), "* 3");
    assert_eq!(state.edit_state.nodes.len(), 2);
    assert!(matches!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", "pitch"))
            .map(|edit| &edit.origin),
        Some(PatcherNodeOrigin::Source { .. })
    ));
    assert!(matches!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", &created_id))
            .map(|edit| &edit.origin),
        Some(PatcherNodeOrigin::Created { .. })
    ));
}

#[test]
fn layout_assigns_finite_nonzero_node_positions() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (triangle (phasor pitch)))
            (out sig 1 @name audio)
            "#,
    );
    for node in &patch.nodes {
        assert!(node.position.0.is_finite());
        assert!(node.position.1.is_finite());
        assert!(node.position.0 >= 0.0);
        assert!(node.position.1 >= 0.0);
    }
}

#[test]
fn fixture_videogame_arp_projects_without_parse_failure() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sequencer/instruments/arcade/videogame-arp/dsp.lisp");
    let source = std::fs::read_to_string(path).unwrap();
    let patch = parse_patch_source(&source, PatcherIntent::Instrument).unwrap();
    assert!(!patch.nodes.is_empty());
}

#[test]
fn fixture_lexilush_projects_without_parse_failure() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sequencer/effects/lexilush/dsp.lisp");
    let source = std::fs::read_to_string(path).unwrap();
    let patch = parse_patch_source(&source, PatcherIntent::Effect).unwrap();
    assert!(!patch.nodes.is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_emits_nodes_and_cables() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (triangle (phasor pitch)))
            (out sig 1 @name audio)
            "#,
    );
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &PatcherInteractionState::default(),
    );
    let text_count = prims
        .iter()
        .filter(|prim| matches!(prim, MetalPrimitive::ProportionalText(_)))
        .count();
    let rect_count = prims
        .iter()
        .filter(|prim| matches!(prim, MetalPrimitive::Rect(_)))
        .count();
    let rounded_count = prims
        .iter()
        .filter(|prim| matches!(prim, MetalPrimitive::WidgetInstance { .. }))
        .count();
    let cable_count = prims
        .iter()
        .filter(|prim| matches!(prim, MetalPrimitive::PatchCable(_)))
        .count();
    let min_cable_radius = prims
        .iter()
        .filter_map(|prim| match prim {
            MetalPrimitive::PatchCable(cable) => Some(cable.radius_px),
            _ => None,
        })
        .fold(f32::INFINITY, f32::min);
    assert!(text_count >= patch.nodes.len(), "{text_count}");
    assert!(rounded_count >= patch.nodes.len() * 2, "{rounded_count}");
    assert!(cable_count >= patch.connections.len(), "{cable_count}");
    assert!(min_cable_radius >= 4.4, "{min_cable_radius}");
    assert!(
        rect_count == 0,
        "patcher node chrome should use rounded widget instances, got {rect_count} rects"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_marks_selected_cable_and_handles() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let selected_cable = source_connection_id(patch.connections.first().unwrap());
    let state = PatcherInteractionState {
        selected_cable: Some(selected_cable),
        ..Default::default()
    };
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &state,
    );

    let selected_cable_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                prim,
                MetalPrimitive::PatchCable(cable) if cable.color == theme::PATCHER_ERROR()
            )
        })
        .count();
    let handle_shell_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                prim,
                MetalPrimitive::Circle(circle) if circle.color == theme::PATCHER_ERROR()
            )
        })
        .count();

    assert_eq!(selected_cable_count, 1);
    assert_eq!(handle_shell_count, 2);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_endpoint_drag_replaces_original_selected_cable() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let connection = patch.connections.first().unwrap();
    let selected_cable = source_connection_id(connection);
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 100.0,
        height: 40.0,
    };
    let pan = PatcherPanState::default();
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let (start, end) = connection_endpoints(
        connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    let state = PatcherInteractionState {
        selected_cable: Some(selected_cable.clone()),
        drag: Some(PatcherDragState::CableEndpoint {
            cable_id: selected_cable,
            endpoint: CableEndpoint::To,
            original_from: OutputPortRef {
                node_id: connection.from_node.clone(),
                output_index: connection.from_output,
            },
            original_to: InputPortRef {
                node_id: connection.to_node.clone(),
                input_index: connection.to_input,
            },
            start_col: start.0,
            start_row: start.1,
            end_col: end.0,
            end_row: end.1,
            current_col: end.0 + 5.0,
            current_row: end.1 + 2.0,
            target_from: None,
            target_to: None,
        }),
        ..Default::default()
    };
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        rect,
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &pan,
        &state,
    );

    let cable_count = prims
        .iter()
        .filter(|prim| matches!(prim, MetalPrimitive::PatchCable(_)))
        .count();
    let selected_cable_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                prim,
                MetalPrimitive::PatchCable(cable) if cable.color == theme::PATCHER_ERROR()
            )
        })
        .count();
    let handle_shell_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                prim,
                MetalPrimitive::Circle(circle) if circle.color == theme::PATCHER_ERROR()
            )
        })
        .count();

    assert_eq!(
        cable_count, 1,
        "endpoint dragging should render the moving cable instead of original plus preview"
    );
    assert_eq!(selected_cable_count, 1);
    assert_eq!(handle_shell_count, 2);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_emits_edit_cursor_as_foreground_overlay() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (2.0, 2.0));
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id,
        text: "phasor".to_string(),
        original_text: String::new(),
        state: TextInputState {
            cursor_pos: 6,
            selection_anchor: None,
            selecting: false,
        },
    });

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &state,
    );

    let cursor_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                prim,
                MetalPrimitive::ForegroundRect(rect)
                    if rect.color == theme::PATCHER_EDIT_CURSOR()
            )
        })
        .count();
    assert_eq!(
        cursor_count, 1,
        "active patcher text edit should render exactly one foreground cursor"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_uses_single_text_run_for_active_node_edit_with_spaces() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (2.0, 2.0));
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id,
        text: "in 3 4".to_string(),
        original_text: String::new(),
        state: TextInputState {
            cursor_pos: 6,
            selection_anchor: None,
            selecting: false,
        },
    });

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &state,
    );

    let label_runs: Vec<&str> = prims
        .iter()
        .filter_map(|prim| match prim {
            MetalPrimitive::ProportionalText(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        label_runs.contains(&"in 3 4"),
        "active edit should render the complete edit buffer as one text run: {label_runs:?}"
    );
    assert!(
        !label_runs.contains(&"in") && !label_runs.contains(&"3 4"),
        "active edit text must not be split around whitespace because the cursor is measured against the unsplit buffer: {label_runs:?}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_places_committed_node_tail_after_measured_space_width() {
    let label = "in 7 8".to_string();
    let measurer = FixedWidthTextMeasurer;
    let measure_ctx = MeasureCtx {
        text_measurer: Some(&measurer),
        cell_w: 10.0,
        cell_h: 20.0,
        inherited_font_size: NODE_FONT_SIZE,
    };
    cache_char_widths(label, NODE_FONT_SIZE, &measure_ctx);

    let patch = Patch {
        nodes: vec![PatchNode {
            id: "committed-space-node".to_string(),
            op: "in".to_string(),
            kind: NodeKind::Builtin,
            label: "in".to_string(),
            args: vec![
                ArgValue::Literal("7".to_string()),
                ArgValue::Literal("8".to_string()),
            ],
            outputs: vec!["out".to_string()],
            position: (2.0, 2.0),
            diagnostic: None,
            source: None,
        }],
        connections: Vec::new(),
        macros: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &PatcherInteractionState::default(),
    );

    let head = prims.iter().find_map(|prim| match prim {
        MetalPrimitive::ProportionalText(text) if text.text == "in" => Some(text.col),
        _ => None,
    });
    let tail = prims.iter().find_map(|prim| match prim {
        MetalPrimitive::ProportionalText(text) if text.text == "7 8" => Some(text.col),
        _ => None,
    });
    let head = head.expect("committed in node should render head text");
    let tail = tail.expect("committed in node should render tail text");

    assert_eq!(
        tail - head,
        2.5,
        "tail should start after the measured width of `in `, not after a fixed visual gap"
    );
}
