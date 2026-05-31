use std::collections::{HashMap, HashSet};

use crate::parser::{ASTParser, Expression, Parser, format_expression};

use super::model::{
    ConnectionKind, MacroSignature, NodeKind, OperatorPortShape, Patch, PatcherIntent,
};
use super::project::{Projector, dgenlisp_constant_names, dgenlisp_operator_port_shapes};

pub fn parse_patch_source(source: &str, intent: PatcherIntent) -> Result<Patch, String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("failed to tokenize dsp.lisp: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("failed to parse dsp.lisp: {error:?}"))?;
    let macros = exprs
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
        .collect();
    Ok(Projector::new(macros, intent).project(&exprs))
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
    items
        .windows(2)
        .find_map(|pair| match (&pair[0], &pair[1]) {
            (Expression::Symbol(key), value) if key == attr => Some(format_expression(value)),
            _ => None,
        })
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
        if matches!(&items[idx], Expression::Symbol(symbol) if symbol.starts_with('@')) {
            idx += 2;
            continue;
        }
        args.push(&items[idx]);
        idx += 1;
    }
    args
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
        _ => def_name
            .map(|name| format!("{op} {name}"))
            .unwrap_or_else(|| op.to_string()),
    }
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
    items
        .windows(2)
        .find_map(|pair| match (&pair[0], &pair[1]) {
            (Expression::Symbol(key), value) if key == attr => Some(format_patch_literal(value)),
            _ => None,
        })
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
    let source = format!("({text})");
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
