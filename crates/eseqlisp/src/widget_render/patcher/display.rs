use super::metrics::{CODE_NODE_HEIGHT, CODE_NODE_MIN_WIDTH, NODE_HEIGHT, NODE_MIN_WIDTH};
use super::model::{ArgValue, NodeKind, PatchNode};

pub(super) fn preview(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            break;
        }
        out.push(ch);
    }
    out
}

pub(super) fn node_display_label(node: &PatchNode) -> String {
    let base = match node.kind {
        NodeKind::Builtin
        | NodeKind::MacroInstance
        | NodeKind::Out
        | NodeKind::Constant => node.op.as_str(),
        NodeKind::In => return node.label.clone(),
        _ => node.label.as_str(),
    };
    let mut label = base.to_string();
    let display_start = if matches!(
        node.args.first(),
        Some(ArgValue::SymbolRef(_) | ArgValue::ConnectedExpr)
    ) {
        1
    } else {
        0
    };
    let last_literal = node
        .args
        .iter()
        .enumerate()
        .skip(display_start)
        .filter_map(|(idx, arg)| match arg {
            ArgValue::Literal(value) if value != "<expr>" => Some(idx),
            _ => None,
        })
        .last();
    let Some(last_literal) = last_literal else {
        return label;
    };
    for arg in node.args.iter().take(last_literal + 1).skip(display_start) {
        match arg {
            ArgValue::Literal(value) if value != "<expr>" => {
                label.push(' ');
                label.push_str(value);
            }
            ArgValue::SymbolRef(_) | ArgValue::ConnectedExpr => {
                label.push_str(" ?");
            }
            _ => {}
        }
    }
    label
}

pub(super) fn node_size(node: &PatchNode) -> (f32, f32) {
    let char_width = if node.kind == NodeKind::CodeIsland {
        0.52
    } else {
        1.16
    };
    let horizontal_padding = if node.kind == NodeKind::CodeIsland {
        2.0
    } else {
        2.65
    };
    let label = node_display_label(node);
    let text_width = label.chars().count() as f32 * char_width + horizontal_padding;
    if node.kind == NodeKind::CodeIsland {
        (
            text_width.max(CODE_NODE_MIN_WIDTH).min(34.0),
            CODE_NODE_HEIGHT,
        )
    } else {
        (text_width.max(NODE_MIN_WIDTH).min(96.0), NODE_HEIGHT)
    }
}
