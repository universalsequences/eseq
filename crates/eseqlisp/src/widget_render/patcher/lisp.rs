use std::collections::{HashMap, HashSet};

use crate::defmacro_library::{DefmacroLibrary, DefmacroPackage, parse_use_defmacro};
use crate::parser::{ASTParser, Expression, Parser, format_expression};

use super::model::{
    ConnectionKind, MacroOrigin, MacroPatch, MacroSignature, NodeKind, OperatorPortShape, Patch,
    PatcherIntent,
};
use super::project::{Projector, dgenlisp_constant_names, dgenlisp_operator_port_shapes};
use super::sidecar;

pub fn parse_patch_source(source: &str, intent: PatcherIntent) -> Result<Patch, String> {
    let exprs = parse_source_exprs(source)?;
    let macros = local_macro_signatures(&exprs);
    Ok(Projector::new(macros, intent).project(&exprs))
}

pub fn parse_patch_source_with_library(
    source: &str,
    intent: PatcherIntent,
    library: &DefmacroLibrary,
) -> Result<Patch, String> {
    let exprs = parse_source_exprs(source)?;
    let mut macros = local_macro_signatures(&exprs);
    for package in library.packages().values() {
        macros
            .entry(package.name.clone())
            .or_insert_with(|| MacroSignature {
                params: package.params.clone(),
                outputs: package.outputs.clone(),
            });
    }
    let mut patch = Projector::new(macros.clone(), intent).project(&exprs);
    for package in library.packages().values() {
        if patch
            .macros
            .iter()
            .any(|macro_patch| macro_patch.name == package.name)
        {
            continue;
        }
        patch
            .macros
            .push(project_library_macro_patch(package, &macros, intent)?);
    }
    Ok(patch)
}

fn parse_source_exprs(source: &str) -> Result<Vec<Expression>, String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("failed to tokenize dsp.lisp: {error:?}"))?;
    ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("failed to parse dsp.lisp: {error:?}"))
}

fn local_macro_signatures(exprs: &[Expression]) -> HashMap<String, MacroSignature> {
    exprs
        .iter()
        .filter_map(|expr| {
            let Expression::List(items) = expr else {
                return None;
            };
            if symbol_at(items, 0) != Some("defmacro") {
                return None;
            }
            let name = symbol_at(items, 1)?.to_string();
            let params = match items.get(2) {
                Some(Expression::List(params)) => params
                    .iter()
                    .filter_map(|expr| match expr {
                        Expression::Symbol(name) => Some(name.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            Some((
                name,
                MacroSignature {
                    params,
                    outputs: infer_macro_outputs(&items[3..]),
                },
            ))
        })
        .collect()
}

fn project_library_macro_patch(
    package: &DefmacroPackage,
    macro_signatures: &HashMap<String, MacroSignature>,
    intent: PatcherIntent,
) -> Result<MacroPatch, String> {
    let exprs = parse_source_exprs(&package.source)?;
    let mut projected = Projector::new(macro_signatures.clone(), intent).project(&exprs);
    sidecar::apply_layout_file(&package.layout_path, &mut projected)?;
    let mut macro_patch = projected
        .macros
        .into_iter()
        .find(|macro_patch| macro_patch.name == package.name)
        .ok_or_else(|| {
            format!(
                "library macro package '{}' did not project its public defmacro",
                package.name
            )
        })?;
    macro_patch.origin = MacroOrigin::Library {
        source_path: package.source_path.to_string_lossy().to_string(),
        layout_path: package.layout_path.to_string_lossy().to_string(),
    };
    Ok(macro_patch)
}

fn infer_macro_outputs(body: &[Expression]) -> Vec<String> {
    let Some(return_expr) = body.last() else {
        return Vec::new();
    };
    let count = tuple_return_items(return_expr)
        .map(|items| items.len())
        .unwrap_or(1);
    (0..count)
        .map(|idx| {
            if idx == 0 {
                "out".to_string()
            } else {
                format!("out{}", idx + 1)
            }
        })
        .collect()
}

fn tuple_return_items(expr: &Expression) -> Option<Vec<&Expression>> {
    let Expression::List(items) = expr else {
        return None;
    };
    (symbol_at(items, 0) == Some("tuple")).then(|| items.iter().skip(1).collect())
}

pub(super) fn symbol_at(items: &[Expression], idx: usize) -> Option<&str> {
    match items.get(idx) {
        Some(Expression::Symbol(symbol)) => Some(symbol),
        _ => None,
    }
}

pub(super) fn attribute_value(items: &[Expression], attr: &str) -> Option<String> {
    attribute_value_items(items, attr).map(|values| join_formatted(values, format_expression))
}

/// The full value run for `attr`, which is more than one item for bracketed arrays.
pub(super) fn attribute_value_items<'a>(
    items: &'a [Expression],
    attr: &str,
) -> Option<&'a [Expression]> {
    let key_idx = items
        .iter()
        .position(|item| matches!(item, Expression::Symbol(key) if key == attr))?;
    let len = attribute_value_len(items, key_idx + 1);
    (len > 0).then(|| &items[key_idx + 1..key_idx + 1 + len])
}

/// Joins tokens with spaces, except before a closing bracket: `]` lexes as its own token
/// (a number stops at the first non-digit), and a naive join renders `[3 3 ]`.
fn join_formatted(values: &[Expression], format: fn(&Expression) -> String) -> String {
    let mut out = String::new();
    for value in values {
        let text = format(value);
        if !out.is_empty() && !text.starts_with(']') {
            out.push(' ');
        }
        out.push_str(&text);
    }
    out
}

pub(super) fn attribute_symbol_value<'a>(items: &'a [Expression], attr: &str) -> Option<&'a str> {
    items
        .windows(2)
        .find_map(|pair| match (&pair[0], &pair[1]) {
            (Expression::Symbol(key), Expression::Symbol(value)) if key == attr => {
                Some(value.as_str())
            }
            _ => None,
        })
}

pub(super) fn node_kind_for_op(op: &str, macros: &HashSet<String>) -> NodeKind {
    match op {
        "in" => NodeKind::In,
        "out" => NodeKind::Out,
        "param" => NodeKind::Param,
        "history" | "make-history" | "read-history" | "write-history" => NodeKind::History,
        _ if dgenlisp_constant_names().contains(op) || is_numeric_literal(op) => NodeKind::Constant,
        _ if macros.contains(op) => NodeKind::MacroInstance,
        _ => NodeKind::Builtin,
    }
}

pub(super) fn is_use_defmacro_form(expr: &Expression) -> bool {
    parse_use_defmacro(expr).ok().flatten().is_some()
}

pub(super) fn is_unsupported_call_head(op: &str) -> bool {
    matches!(
        op,
        "let" | "if" | "do" | "lambda" | "match" | "each" | "quote" | "quasiquote"
    )
}

pub(super) fn call_input_args<'a>(op: &str, items: &'a [Expression]) -> Vec<&'a Expression> {
    match op {
        "in" => positional_args(items, 1).into_iter().take(1).collect(),
        "param" | "make-history" => Vec::new(),
        "out" => positional_args(items, 1).into_iter().take(2).collect(),
        "write-history" => positional_args(items, 1)
            .into_iter()
            .nth(1)
            .into_iter()
            .collect(),
        _ => positional_args(items, 1),
    }
}

pub(super) fn positional_args(items: &[Expression], start: usize) -> Vec<&Expression> {
    let mut args = Vec::new();
    let mut idx = start;
    while idx < items.len() {
        if is_attribute_key(&items[idx]) {
            idx += attribute_span_len(items, idx);
            continue;
        }
        args.push(&items[idx]);
        idx += 1;
    }
    args
}

pub(super) fn is_attribute_key(expr: &Expression) -> bool {
    matches!(expr, Expression::Symbol(symbol) if symbol.starts_with('@'))
}

/// Number of items the value of the attribute at `key_idx` occupies, plus the key itself.
///
/// `[` and `]` are not lexer delimiters, so a bracketed vector literal never parses as a
/// single expression: `@shape [3 3]` lexes as `@shape`, `[3`, `3`, `3]`. Every caller that
/// walks an item list has to skip the whole run, otherwise the array tail leaks out as
/// positional arguments (phantom inlets, mangled writeback).
pub(super) fn attribute_span_len(items: &[Expression], key_idx: usize) -> usize {
    1 + attribute_value_len(items, key_idx + 1)
}

/// Number of items the attribute value starting at `value_idx` occupies.
pub(super) fn attribute_value_len(items: &[Expression], value_idx: usize) -> usize {
    let Some(first) = items.get(value_idx) else {
        return 0;
    };
    let mut depth = bracket_depth_delta(first);
    if depth <= 0 {
        return 1;
    }
    for (offset, item) in items.iter().enumerate().skip(value_idx + 1) {
        depth += bracket_depth_delta(item);
        if depth <= 0 {
            return offset - value_idx + 1;
        }
    }
    // Unterminated array (mid-edit text): swallow the rest rather than spraying the tail
    // across the positional slots.
    items.len() - value_idx
}

fn bracket_depth_delta(expr: &Expression) -> i32 {
    let Expression::Symbol(symbol) = expr else {
        return 0;
    };
    symbol.chars().fold(0, |depth, ch| match ch {
        '[' => depth + 1,
        ']' => depth - 1,
        _ => depth,
    })
}

/// Marks every item index consumed by an `@key value...` run, so callers that walk a list
/// positionally can tell attribute payload from real arguments.
pub(super) fn attribute_item_mask(items: &[Expression]) -> Vec<bool> {
    let mut mask = vec![false; items.len()];
    let mut idx = 1;
    while idx < items.len() {
        if is_attribute_key(&items[idx]) {
            let span = attribute_span_len(items, idx);
            for flag in mask.iter_mut().skip(idx).take(span) {
                *flag = true;
            }
            idx += span;
            continue;
        }
        idx += 1;
    }
    mask
}

/// Rewrites `[1,4,5,6]` to `[1 4 5 6]`. A comma is the unquote token, so a comma-separated
/// array lexes into `Unquote` nodes instead of the bracketed run the compiler expects.
pub(super) fn normalize_editor_node_text(text: &str) -> String {
    if !text.contains(',') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut depth = 0i32;
    let mut in_string = false;
    for ch in text.chars() {
        match ch {
            '"' => in_string = !in_string,
            '[' if !in_string => depth += 1,
            ']' if !in_string => depth -= 1,
            ',' if !in_string && depth > 0 => {
                out.push(' ');
                continue;
            }
            _ => {}
        }
        out.push(ch);
    }
    out
}

pub(super) fn connection_kind_for_op(op: &str) -> ConnectionKind {
    if op == "write-history" {
        ConnectionKind::Feedback
    } else {
        ConnectionKind::Forward
    }
}

pub(super) fn node_label(op: &str, items: &[Expression], def_name: Option<&str>) -> String {
    match op {
        "in" => in_label(items),
        "out" => out_label(items),
        "param" => param_label(items),
        "history" | "make-history" => "history".to_string(),
        _ => {
            let mut label = def_name
                .map(|name| format!("{op} {name}"))
                .unwrap_or_else(|| op.to_string());
            // `in`/`out`/`param` build their attributes into the label above; every other
            // op has to carry them here too, otherwise the generator (which emits from the
            // model alone) drops `@shape`/`@data`/`@file` on the next save.
            label.push_str(&attributes_suffix(items));
            label
        }
    }
}

/// The ` @key value...` runs carried by a node label, rendered back to source text.
///
/// A builtin's display label and its emitted call are both rebuilt from op + input slots,
/// so this is what keeps `@shape`/`@data`/`@file` alive across the round trip.
pub(super) fn label_attributes_suffix(label: &str) -> String {
    let source = format!("({})", normalize_editor_node_text(label.trim()));
    let Ok(tokens) = Parser::new(source).parse() else {
        return String::new();
    };
    let Ok(exprs) = ASTParser::new(tokens).parse() else {
        return String::new();
    };
    match exprs.first() {
        Some(Expression::List(items)) => attributes_suffix(items),
        _ => String::new(),
    }
}

/// The ` @key value...` runs of `items`, rendered back to source text.
pub(super) fn attributes_suffix(items: &[Expression]) -> String {
    let mut suffix = String::new();
    let mut idx = 1;
    while idx < items.len() {
        if is_attribute_key(&items[idx]) {
            let span = attribute_span_len(items, idx);
            suffix.push(' ');
            suffix.push_str(&join_formatted(
                &items[idx..items.len().min(idx + span)],
                format_patch_literal,
            ));
            idx += span;
            continue;
        }
        idx += 1;
    }
    suffix
}

fn in_label(items: &[Expression]) -> String {
    let mut label = String::from("in");
    if let Some(channel) = positional_args(items, 1).first() {
        label.push(' ');
        label.push_str(&format_patch_literal(channel));
    }
    if let Some(name) = attribute_symbol_value(items, "@name") {
        label.push_str(" @name ");
        label.push_str(name);
    }
    label
}

fn out_label(items: &[Expression]) -> String {
    let Some(modulator) = attribute_patch_literal_value(items, "@modulator") else {
        return attribute_value(items, "@name").unwrap_or_else(|| "out".to_string());
    };
    let mut label = String::from("out");
    if let Some(channel) = positional_args(items, 1).get(1) {
        label.push(' ');
        label.push_str(&format_patch_literal(channel));
    }
    label.push_str(" @modulator ");
    label.push_str(&modulator);
    label
}

fn attribute_patch_literal_value(items: &[Expression], attr: &str) -> Option<String> {
    attribute_value_items(items, attr).map(|values| join_formatted(values, format_patch_literal))
}

fn param_label(items: &[Expression]) -> String {
    let mut label = String::from("param");
    for item in items.iter().skip(1) {
        label.push(' ');
        label.push_str(&format_patch_literal(item));
    }
    label
}

pub(super) fn format_patch_literal(expr: &Expression) -> String {
    match expr {
        Expression::Number(n) if *n == n.trunc() && n.abs() < 1e15 => {
            format!("{:.0}", n)
        }
        _ => format_expression(expr),
    }
}

pub(super) fn default_outputs(op: &str) -> Vec<String> {
    match op {
        "out" | "write-history" | "make-history" => Vec::new(),
        _ => vec!["out".to_string()],
    }
}

pub(super) fn editor_node_port_shape(
    op: &str,
    kind: NodeKind,
    macro_signatures: &HashMap<String, MacroSignature>,
) -> OperatorPortShape {
    match kind {
        NodeKind::In | NodeKind::Param => OperatorPortShape {
            input_count: 0,
            output_count: 1,
        },
        NodeKind::Constant => OperatorPortShape {
            input_count: 0,
            output_count: 1,
        },
        NodeKind::Out => OperatorPortShape {
            input_count: 1,
            output_count: 0,
        },
        NodeKind::History => OperatorPortShape {
            input_count: 1,
            output_count: 1,
        },
        NodeKind::MacroInstance => OperatorPortShape {
            input_count: macro_signatures
                .get(op)
                .map(|signature| signature.params.len())
                .unwrap_or(1),
            output_count: macro_signatures
                .get(op)
                .map(|signature| signature.outputs.len())
                .unwrap_or(1),
        },
        _ => dgenlisp_operator_port_shapes()
            .get(op)
            .copied()
            .unwrap_or(OperatorPortShape {
                input_count: 1,
                output_count: 1,
            }),
    }
}

pub(super) fn parse_editor_node_text(text: &str) -> Result<(String, Vec<String>), String> {
    let source = format!("({})", normalize_editor_node_text(text));
    let tokens = Parser::new(source)
        .parse()
        .map_err(|error| format!("failed to tokenize node text: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("failed to parse node text: {error:?}"))?;
    let Some(Expression::List(items)) = exprs.first() else {
        return Err("node text must start with an operator".to_string());
    };
    let Some(op) = symbol_at(items, 0) else {
        if let Some(value) = items.first().and_then(numeric_literal_text) {
            return Ok((value, Vec::new()));
        }
        return Err("node text must start with a symbolic operator".to_string());
    };
    Ok((
        op.to_string(),
        positional_args(items, 1)
            .into_iter()
            .map(format_patch_literal)
            .collect(),
    ))
}

pub(super) fn is_numeric_literal(text: &str) -> bool {
    text.parse::<f64>().is_ok()
}

fn numeric_literal_text(expr: &Expression) -> Option<String> {
    match expr {
        Expression::Number(_) => Some(format_patch_literal(expr)),
        _ => None,
    }
}
