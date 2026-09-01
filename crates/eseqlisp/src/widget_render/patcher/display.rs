use super::lisp::label_attributes_suffix;
use super::metrics::{
    CODE_NODE_FONT_SIZE, CODE_NODE_HEIGHT, CODE_NODE_MIN_WIDTH, NODE_FONT_SIZE, NODE_HEIGHT,
    NODE_MIN_WIDTH, PORT_EDGE_PADDING_CELLS, PORT_MIN_CENTER_SPACING_CELLS,
};
use super::model::{ArgValue, NodeKind, PatchNode};
#[cfg(target_os = "macos")]
use super::text_metrics::measured_text_width;
use std::collections::HashSet;
use std::ops::Range;

const MISSING_INPUT_SENTINEL: &str = "__patcher_missing_input__";

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

/// A node label together with the char range each displayed argument occupies in it.
///
/// Token order in the label is not argument order — slots are skipped and some
/// arguments render nothing at all — so the spans are recorded by the one loop
/// that builds the text rather than recovered by re-tokenizing it afterwards.
pub(super) struct NodeDisplayLabel {
    pub(super) text: String,
    pub(super) arg_spans: Vec<(usize, Range<usize>)>,
}

pub(super) fn node_display_label(node: &PatchNode) -> String {
    build_node_display_label(node).text
}

/// Char ranges, in `node_display_label(node)`, of each argument token the label
/// draws, paired with that argument's index. Inlined literals draw no port, so
/// this is the only handle hover has on which inlet they are.
pub(super) fn node_display_label_arg_spans(node: &PatchNode) -> Vec<(usize, Range<usize>)> {
    build_node_display_label(node).arg_spans
}

fn push_arg_token(
    label: &mut String,
    arg_spans: &mut Vec<(usize, Range<usize>)>,
    arg_index: usize,
    token: &str,
) {
    label.push(' ');
    let start = label.chars().count();
    label.push_str(token);
    arg_spans.push((arg_index, start..label.chars().count()));
}

pub(super) fn build_node_display_label(node: &PatchNode) -> NodeDisplayLabel {
    let bare = |text: String| NodeDisplayLabel {
        text,
        arg_spans: Vec::new(),
    };
    // `base` is either the bare op — in which case the label's attributes have to be
    // reattached below — or a full label that already carries them (`param`, `in`, `out`).
    let (base, base_carries_attributes) = match node.kind {
        NodeKind::Out if node.label.contains("@modulator") => return bare(node.label.clone()),
        NodeKind::Builtin | NodeKind::MacroInstance | NodeKind::Out | NodeKind::Constant => {
            (node.op.as_str(), false)
        }
        NodeKind::In => return bare(node.label.clone()),
        _ => (node.label.as_str(), true),
    };
    build_label_over_slots(
        node,
        base,
        base_carries_attributes,
        node_display_input_slots(node),
    )
}

fn build_label_over_slots(
    node: &PatchNode,
    base: &str,
    base_carries_attributes: bool,
    slots: Vec<usize>,
) -> NodeDisplayLabel {
    let mut label = base.to_string();
    let mut arg_spans = Vec::new();
    for idx in slots {
        if let Some(inline) = node.inline_inputs.get(idx).and_then(|input| input.as_ref()) {
            push_arg_token(&mut label, &mut arg_spans, idx, &inline.label());
            continue;
        }
        match &node.args[idx] {
            ArgValue::Literal(value) if value == MISSING_INPUT_SENTINEL => {
                push_arg_token(&mut label, &mut arg_spans, idx, "?");
            }
            ArgValue::Literal(value) if value != "<expr>" => {
                push_arg_token(&mut label, &mut arg_spans, idx, value);
            }
            ArgValue::SymbolRef(_) | ArgValue::ConnectedExpr => {
                push_arg_token(&mut label, &mut arg_spans, idx, "?");
            }
            _ => {}
        }
    }
    // A node being typed into is projected as a bare builtin whose op is the
    // whole typed text, attributes and all, so re-attaching the suffix doubled
    // the label — and with it the node's width — for every keystroke.
    if !base_carries_attributes && base != node.label {
        // Attributes trail the inputs, matching the source form the label was built from.
        label.push_str(&label_attributes_suffix(&node.label));
    }
    NodeDisplayLabel {
        text: label,
        arg_spans,
    }
}

/// The first argument the label draws. A node whose slot 0 is a cable slot
/// starts at 1 — that slot's port is drawn, not written.
fn node_display_start(node: &PatchNode) -> usize {
    if matches!(
        node.args.first(),
        Some(ArgValue::SymbolRef(_) | ArgValue::ConnectedExpr)
    ) || matches!(
        node.args.first(),
        Some(ArgValue::Literal(value)) if value == MISSING_INPUT_SENTINEL
    ) {
        1
    } else {
        0
    }
}

pub(super) fn node_display_input_slots(node: &PatchNode) -> Vec<usize> {
    let display_start = node_display_start(node);
    let last_displayed = node
        .args
        .iter()
        .enumerate()
        .skip(display_start)
        .filter_map(|(idx, arg)| match arg {
            _ if node
                .inline_inputs
                .get(idx)
                .and_then(|input| input.as_ref())
                .is_some() =>
            {
                Some(idx)
            }
            ArgValue::Literal(value) if value != "<expr>" && value != MISSING_INPUT_SENTINEL => {
                Some(idx)
            }
            _ => None,
        })
        .last();
    let Some(last_displayed) = last_displayed else {
        return Vec::new();
    };
    node.args
        .iter()
        .enumerate()
        .take(last_displayed + 1)
        .skip(display_start)
        .filter_map(|(idx, arg)| {
            if node
                .inline_inputs
                .get(idx)
                .and_then(|input| input.as_ref())
                .is_some()
            {
                return Some(idx);
            }
            match arg {
                ArgValue::Literal(value) if value == MISSING_INPUT_SENTINEL => Some(idx),
                ArgValue::Literal(value) if value != "<expr>" => Some(idx),
                ArgValue::SymbolRef(_) | ArgValue::ConnectedExpr => Some(idx),
                _ => None,
            }
        })
        .collect()
}

/// The text a node hands to the editor — double-click, and the patcher
/// clipboard — chosen so the node rebuilt from it keeps the original's
/// argument indices.
///
/// `node_display_label` alone is not enough: it stops at the last literal or
/// inline slot, so a node whose trailing slots are all cabled (a two-cable
/// `(- a b)`) renders as a bare `-`. Rebuilding that text sizes the node from
/// the operator's documented arity — one input for `-` — and the second cable
/// is dropped with no diagnostic. Every slot up to the highest one carrying
/// anything, cables included, therefore gets an explicit token, `?` standing in
/// for a cable.
///
/// `inbound_slots` is the set of argument indices this node has cables landing
/// on.
pub(super) fn editable_node_text(node: &PatchNode, inbound_slots: &HashSet<usize>) -> String {
    let Some(slots) = editable_input_slots(node, inbound_slots) else {
        return node_display_label(node);
    };
    build_label_over_slots(node, node.op.as_str(), false, slots).text
}

/// The padded slot list `editable_node_text` writes out, or `None` when this
/// node's text cannot be padded without shifting its arguments.
///
/// Editor text reserves slot 0 for the implicit cable (`node_from_editor_text`
/// places the first written token at index 1), so a node whose slot 0 holds a
/// literal would have every argument shifted by padding — those keep the
/// display label unchanged, as does any node with an undrawn `<expr>` slot in
/// the middle of the run, which would leave a hole in the token sequence.
fn editable_input_slots(node: &PatchNode, inbound_slots: &HashSet<usize>) -> Option<Vec<usize>> {
    if !matches!(node.kind, NodeKind::Builtin | NodeKind::MacroInstance) {
        return None;
    }
    if node_display_start(node) != 1 {
        return None;
    }
    let has_inline = |idx: usize| {
        node.inline_inputs
            .get(idx)
            .and_then(|input| input.as_ref())
            .is_some()
    };
    let carries = |idx: usize| {
        inbound_slots.contains(&idx)
            || has_inline(idx)
            || matches!(
                node.args.get(idx),
                Some(ArgValue::Literal(value)) if value != "<expr>" && value != MISSING_INPUT_SENTINEL
            )
    };
    let highest_used = (1..node.args.len())
        .filter(|idx| carries(*idx))
        .next_back()?;
    // A slot the label draws nothing for would leave a hole that shifts every
    // later token onto the wrong argument.
    if (1..=highest_used).any(|idx| {
        !has_inline(idx)
            && matches!(node.args.get(idx), Some(ArgValue::Literal(value)) if value == "<expr>")
    }) {
        return None;
    }
    Some((1..=highest_used).collect())
}

pub(super) fn node_size(node: &PatchNode) -> (f32, f32) {
    node_size_for_ports(node, 0, node.outputs.len())
}

pub(super) fn node_font_size(node: &PatchNode) -> f32 {
    if node.kind == NodeKind::CodeIsland {
        CODE_NODE_FONT_SIZE
    } else {
        NODE_FONT_SIZE
    }
}

pub(super) fn node_autogenerated_size_for_ports(
    node: &PatchNode,
    input_slot_count: usize,
    output_count: usize,
) -> (f32, f32) {
    let width = node_autogenerated_width_for_label(node, &node_display_label(node))
        .max(port_width(input_slot_count.max(output_count)));
    let height = if node.kind == NodeKind::CodeIsland {
        CODE_NODE_HEIGHT
    } else {
        NODE_HEIGHT
    };
    (width, height)
}

pub(super) fn node_autogenerated_width_for_label(node: &PatchNode, label: &str) -> f32 {
    let horizontal_padding = if node.kind == NodeKind::CodeIsland {
        2.0
    } else {
        2.65
    };
    let font_size = node_font_size(node);
    #[cfg(target_os = "macos")]
    let label_width = measured_text_width(label, font_size)
        .unwrap_or_else(|| estimated_label_width_cells(label, font_size));
    // The terminal renderer places patcher label characters directly in its
    // fixed cell grid, so this is exact in that backend rather than a font
    // width estimate.
    #[cfg(not(target_os = "macos"))]
    let label_width = label.chars().count() as f32;
    let text_width = label_width + horizontal_padding;
    if node.kind == NodeKind::CodeIsland {
        text_width.max(CODE_NODE_MIN_WIDTH).min(34.0)
    } else {
        text_width.max(NODE_MIN_WIDTH).min(96.0)
    }
}

/// Fallback average glyph advance, in layout cells per point of font size, used
/// until the renderer has measured something to calibrate against.
///
/// Everything here is expressed in cells, never pixels: a layout cell is the
/// monospace advance of the text atlas, which is itself built at the display
/// scale factor, so cells-per-character is scale independent. (Deriving this
/// from a pixel advance instead is how the first version of this estimate
/// underestimated every label by the retina scale factor.) Measurements range
/// from 0.055-0.065 for the system font over the mono cell
/// across node/code font sizes and scale factors. This sits at the top of that
/// range so the estimate never packs nodes tighter than they render.
const FALLBACK_ADVANCE_CELLS_PER_POINT: f32 = 0.065;

/// Approximate width of `label` in layout cells for the passes that run before
/// that label's glyph advances have been measured.
///
/// The auto layout runs while a patch is projected, which happens *before* the
/// measure pass caches that patch's label advances (and the projected patch is
/// then cached, so the cold-cache widths are the ones the layout keeps). Falling
/// back to a zero-width label collapsed every node to `NODE_MIN_WIDTH`, so the
/// layout packed nodes far tighter than they render and they visibly overlapped.
pub(super) fn estimated_label_width_cells(label: &str, font_size: f32) -> f32 {
    let advance = super::text_metrics::measured_average_advance_cells_per_point()
        .unwrap_or(FALLBACK_ADVANCE_CELLS_PER_POINT);
    label.chars().count() as f32 * advance * font_size
}

pub(super) fn node_size_for_ports(
    node: &PatchNode,
    input_slot_count: usize,
    output_count: usize,
) -> (f32, f32) {
    let (autogenerated_width, height) =
        node_autogenerated_size_for_ports(node, input_slot_count, output_count);
    let width = node
        .width
        .filter(|width| width.is_finite())
        .map(|width| width.max(autogenerated_width))
        .unwrap_or(autogenerated_width);
    (width, height)
}

fn port_width(port_count: usize) -> f32 {
    if port_count == 0 {
        0.0
    } else {
        PORT_EDGE_PADDING_CELLS * 2.0
            + PORT_MIN_CENTER_SPACING_CELLS * port_count.saturating_sub(1) as f32
    }
}
