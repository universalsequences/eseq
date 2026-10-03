//! Patcher side of DGen `(probe x @id "…" @view number|scope)` taps
//! (docs/patcher-probes-spec.md §6.2).
//!
//! A probe is an ordinary builtin node: it round-trips through the graph
//! payload and the generator like any other. What this module adds:
//!
//! - **Creation.** `number~` / `scope~` are patcher-only spellings of
//!   `probe @view number|scope`; they are expanded on commit and never reach
//!   source. A committed probe always carries an `@id`: the one it already had,
//!   else a freshly minted `pN` unique within the source. Paste mints new ids.
//! - **Display.** The node header shows the alias (`number~`) rather than the
//!   attribute list, and the live value of the probe's latest frame, read from
//!   [`crate::live_audio::patch_probe_frame`] while painting so a new frame
//!   repaints the patcher without a layout pass.

use std::collections::HashSet;
use std::sync::Arc;

use crate::live_audio::{self, ProbeFrame, ProbeView};
use crate::parser::Expression;

use super::generate::label_items;
use super::lisp::{
    attribute_value_items, attributes_suffix_except, format_patch_literal, join_formatted,
    positional_args,
};
use super::model::{Patch, PatchNode};
use super::state::PatcherInteractionState;

pub(super) const PROBE_OP: &str = "probe";
/// Patcher-only spellings of a probe with a given `@view`.
const PROBE_ALIASES: [(&str, ProbeView); 2] =
    [("number~", ProbeView::Number), ("scope~", ProbeView::Scope)];
/// Prefix of the ids the patcher mints. Deliberately not the compiler's own
/// default (`probe-<ordinal>`), so a minted id can never collide with one the
/// compiler assigns to a hand-written id-less probe.
const MINTED_ID_PREFIX: &str = "p";
/// Widest value [`format_probe_value`] produces; the node reserves room for it
/// so the box does not jitter as digits change.
pub(super) const PROBE_VALUE_WIDTH_SAMPLE: &str = "-8.888e-88";
/// Shown when no frame is available (no live instance, or not compiled yet).
pub(super) const PROBE_NO_VALUE: &str = "—";

/// The `@view` an alias head stands for.
pub(super) fn probe_alias_view(head: &str) -> Option<ProbeView> {
    PROBE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == head)
        .map(|(_, view)| *view)
}

/// The alias names, for autocomplete.
pub(super) fn probe_alias_names() -> impl Iterator<Item = &'static str> {
    PROBE_ALIASES.iter().map(|(alias, _)| *alias)
}

pub(super) fn is_probe_node(node: &PatchNode) -> bool {
    node.op == PROBE_OP
}

/// Cheap pre-check, no parse: whether `text`'s head token is `probe` or an
/// alias. Necessary, not sufficient — [`parse_probe`] still decides — so every
/// probe text path can skip the parser for ordinary node text.
pub(super) fn is_probe_text(text: &str) -> bool {
    let head = head_token(text);
    head == PROBE_OP || probe_alias_view(head).is_some()
}

/// The text up to the first whitespace or `(`.
fn head_token(text: &str) -> &str {
    let text = text.trim_start();
    let end = text
        .find(|ch: char| ch.is_whitespace() || ch == '(')
        .unwrap_or(text.len());
    &text[..end]
}

/// A string-or-symbol attribute value (`@id "cut"`, `@view scope`).
fn attribute_text(items: &[Expression], key: &str) -> Option<String> {
    match attribute_value_items(items, key)? {
        [Expression::String(value) | Expression::Symbol(value)] if !value.is_empty() => {
            Some(value.clone())
        }
        _ => None,
    }
}

/// A probe node's `@id` and `@view`, read with one parse of its label.
/// Painting computes these once per node ([`probe_attrs_by_node`]) and hands
/// them down, rather than re-parsing the label for every question.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ProbeAttrs {
    /// `None` for an id-less (hand-written) probe.
    pub(super) id: Option<String>,
    /// `None` for a `@view` this build does not know (passed through as an
    /// opaque hint, spec §4); absent defaults like the compiler.
    pub(super) view: Option<ProbeView>,
}

impl ProbeAttrs {
    /// `None` when `node` is not a probe (no parse).
    pub(super) fn of(node: &PatchNode) -> Option<Self> {
        if !is_probe_node(node) {
            return None;
        }
        let items = label_items(&node.label).unwrap_or_default();
        Some(Self {
            id: attribute_text(&items, "@id"),
            view: match attribute_text(&items, "@view") {
                Some(view) => ProbeView::parse(&view),
                None => Some(ProbeView::default()),
            },
        })
    }

    /// Whether the node header shows a live value: `number`, and `scope`,
    /// whose header row carries the latest value above its waveform.
    pub(super) fn shows_value(&self) -> bool {
        matches!(self.view, Some(ProbeView::Number | ProbeView::Scope))
    }

    /// Drawn in its `scope` view: a taller, resizable node that plots the
    /// capture ring under its header.
    pub(super) fn is_scope(&self) -> bool {
        self.view == Some(ProbeView::Scope)
    }

    /// The header a probe node draws in place of its attribute list: the
    /// alias for its view (`number~`, `scope~`, else `probe`), followed by its
    /// id when the author chose one (`number~ cut`). Minted ids are noise and
    /// stay hidden.
    pub(super) fn header_base(&self) -> String {
        let base = PROBE_ALIASES
            .iter()
            .find(|(_, alias_view)| Some(*alias_view) == self.view)
            .map_or(PROBE_OP, |(alias, _)| *alias);
        match &self.id {
            Some(id) if !is_minted_id(id) => format!("{base} {id}"),
            _ => base.to_string(),
        }
    }
}

/// [`ProbeAttrs`] of every probe node in `patch`, by node id.
pub(super) fn probe_attrs_by_node(patch: &Patch) -> std::collections::HashMap<&str, ProbeAttrs> {
    patch
        .nodes
        .iter()
        .filter_map(|node| Some((node.id.as_str(), ProbeAttrs::of(node)?)))
        .collect()
}

/// The probe's `@id`. `None` for an id-less (hand-written) probe.
pub(super) fn probe_node_id(node: &PatchNode) -> Option<String> {
    ProbeAttrs::of(node)?.id
}

/// The probe's `@view` as written (an unknown view passes through), else the
/// compiler's default. For the context menu's `:probe-view`.
pub(super) fn probe_node_view_text(node: &PatchNode) -> String {
    label_items(&node.label)
        .and_then(|items| attribute_text(&items, "@view"))
        .unwrap_or_else(|| ProbeView::default().as_str().to_string())
}

/// A probe drawn in its `scope` view. One label parse; painting uses
/// [`ProbeAttrs`] instead.
pub(super) fn is_scope_probe(node: &PatchNode) -> bool {
    ProbeAttrs::of(node).is_some_and(|attrs| attrs.is_scope())
}

fn is_minted_id(id: &str) -> bool {
    id.strip_prefix(MINTED_ID_PREFIX)
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()))
}

/// Probe node text, parsed once: what every text rewrite below starts from.
pub(super) struct ParsedProbe {
    items: Vec<Expression>,
    /// The view an alias head (`number~`) stands for; `None` for `probe`.
    alias_view: Option<ProbeView>,
    /// `@id`, when written.
    pub(super) id: Option<String>,
    /// `@view` as written, when present.
    view: Option<String>,
}

impl ParsedProbe {
    /// The view text the canonical form writes: the alias's, else the one
    /// written (an unknown view passes through), else the default.
    fn view_text(&self) -> &str {
        match (self.alias_view, &self.view) {
            (Some(view), _) => view.as_str(),
            (None, Some(view)) => view,
            (None, None) => ProbeView::default().as_str(),
        }
    }

    /// `probe <positional…> [@id "<id>"] @view <view> <other attributes…>`.
    fn build(&self, id: Option<&str>, view: &str) -> String {
        let positional = positional_args(&self.items, 1)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut text = PROBE_OP.to_string();
        if !positional.is_empty() {
            text.push(' ');
            text.push_str(&join_formatted(&positional, format_patch_literal));
        }
        if let Some(id) = id {
            text.push_str(" @id ");
            text.push_str(&format_patch_literal(&Expression::String(id.to_string())));
        }
        text.push_str(" @view ");
        text.push_str(view);
        text.push_str(&attributes_suffix_except(&self.items, &["@id", "@view"]));
        text
    }
}

/// Parse probe node text (`probe …`, `number~ …`, `scope~ …`); `None` for
/// anything else, without parsing when the head token already rules it out.
pub(super) fn parse_probe(text: &str) -> Option<ParsedProbe> {
    if !is_probe_text(text) {
        return None;
    }
    let items = label_items(text)?;
    let Some(Expression::Symbol(head)) = items.first() else {
        return None;
    };
    let alias_view = probe_alias_view(head);
    if head != PROBE_OP && alias_view.is_none() {
        return None;
    }
    Some(ParsedProbe {
        id: attribute_text(&items, "@id"),
        view: attribute_text(&items, "@view"),
        alias_view,
        items,
    })
}

/// The `@id` written in probe node text (`probe @id "cut"`), if any.
pub(super) fn probe_id_from_text(text: &str) -> Option<String> {
    parse_probe(text)?.id
}

/// Model-level desugar of an alias head: `number~ @name x` →
/// `probe @view number @name x`. No id is minted here (that needs the rest of
/// the source); it is how a stray alias that never went through a commit (an
/// agent edit, a test fixture) still projects as a working probe.
pub(super) fn expand_probe_alias(text: &str) -> Option<String> {
    // Runs for every edited node on every paint: an alias head is the rare
    // case, so rule everything else out before parsing.
    probe_alias_view(head_token(text))?;
    let parsed = parse_probe(text)?;
    let view = parsed.alias_view?;
    Some(parsed.build(parsed.id.as_deref(), view.as_str()))
}

/// The smallest `pN` (N ≥ 1) not in `taken`.
pub(super) fn mint_probe_id(taken: &HashSet<String>) -> String {
    (1usize..)
        .map(|n| format!("{MINTED_ID_PREFIX}{n}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("an unbounded range always yields a free id")
}

/// Canonical committed text for probe node text, or `None` when `text` is not
/// a probe. The id is, in order: the one written in `text`, `previous_id` (the
/// node's id before this edit, so retyping `number~` over a probe keeps its
/// identity), else a fresh one minted against `taken`.
pub(super) fn canonical_probe_text(
    text: &str,
    previous_id: Option<&str>,
    taken: &HashSet<String>,
) -> Option<String> {
    let parsed = parse_probe(text)?;
    let id = parsed
        .id
        .clone()
        .or_else(|| previous_id.filter(|id| !id.is_empty()).map(str::to_string))
        .unwrap_or_else(|| mint_probe_id(taken));
    Some(parsed.build(Some(&id), parsed.view_text()))
}

/// Probe text with its id replaced by a fresh one, which is added to `taken`
/// (paste: the copy is a new probe, not a second view of the original).
pub(super) fn reminted_probe_text(text: &str, taken: &mut HashSet<String>) -> Option<String> {
    let parsed = parse_probe(text)?;
    let id = mint_probe_id(taken);
    let text = parsed.build(Some(&id), parsed.view_text());
    taken.insert(id);
    Some(text)
}

/// Every probe id the model of `patch` uses, macro bodies included.
pub(super) fn probe_ids_in_patch(patch: &Patch) -> HashSet<String> {
    let mut ids = HashSet::new();
    collect_patch_probe_ids(patch, &mut ids);
    ids
}

fn collect_patch_probe_ids(patch: &Patch, ids: &mut HashSet<String>) {
    ids.extend(patch.nodes.iter().filter_map(probe_node_id));
    for macro_patch in &patch.macros {
        collect_patch_probe_ids(&macro_patch.patch, ids);
    }
}

/// Probe ids written in the interaction overlay's node texts (every view),
/// skipping `except_key`'s own edit.
pub(super) fn probe_ids_in_edits(
    state: &PatcherInteractionState,
    except_key: Option<&str>,
) -> HashSet<String> {
    state
        .edit_state
        .nodes
        .iter()
        .filter(|(key, _)| Some(key.as_str()) != except_key)
        .filter_map(|(_, edit)| probe_id_from_text(&edit.text))
        .collect()
}

/// Number-view text: about four significant digits, non-finite values
/// spelled plainly. Never wider than [`PROBE_VALUE_WIDTH_SAMPLE`].
pub(super) fn format_probe_value(value: f32) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    if value == 0.0 {
        return "0".to_string();
    }
    let value = f64::from(value);
    let mut exponent = value.abs().log10().floor() as i32;
    // Rounding can carry into the next decade (9.99996 → 10.000): one more
    // pass with the carried exponent keeps it at four significant digits.
    for _ in 0..2 {
        if !(-3..5).contains(&exponent) {
            break;
        }
        let decimals = (3 - exponent).max(0) as usize;
        let text = format!("{value:.decimals$}");
        let rounded = text.parse::<f64>().unwrap_or(value).abs();
        if rounded >= 10f64.powi(exponent + 1) {
            exponent += 1;
            continue;
        }
        return text;
    }
    format!("{value:.3e}")
}

/// What a probe node's value field shows.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ProbeValueDisplay {
    pub(super) text: String,
    /// No frame, or a held (stale) frame: drawn dimmed.
    pub(super) dimmed: bool,
    /// `(min, max)` of the latest block, for the hover tooltip.
    pub(super) range: Option<(f32, f32)>,
}

impl ProbeValueDisplay {
    /// The hover tooltip's `min … max` text; built only when hovered.
    pub(super) fn range_text(&self) -> Option<String> {
        let (min, max) = self.range?;
        Some(format!(
            "min {}  max {}{}",
            format_probe_value(min),
            format_probe_value(max),
            if self.dimmed { "  (held)" } else { "" }
        ))
    }
}

pub(super) fn probe_value_display(frame: Option<&ProbeFrame>) -> ProbeValueDisplay {
    match frame {
        None => ProbeValueDisplay {
            text: PROBE_NO_VALUE.to_string(),
            dimmed: true,
            range: None,
        },
        Some(frame) => ProbeValueDisplay {
            text: format_probe_value(frame.last),
            dimmed: frame.stale,
            range: Some((frame.min, frame.max)),
        },
    }
}

/// The live frame behind a probe with `id`, read from the patcher's live
/// `instance` (resolved once per paint with
/// [`live_audio::patch_probe_instance`]). Reading it while painting registers
/// a paint dependency on this frame, so a new one repaints the patcher.
pub(super) fn probe_frame_in_instance(instance: &str, id: &str) -> Option<Arc<ProbeFrame>> {
    // v1: first occurrence (spec §6.2). A macro body expands once per
    // instance, so its probes repeat; root-level ids are unique.
    live_audio::probe_frame_for(instance, id, 0)
}

/// The live frame behind a probe node of the patcher at `path`.
#[cfg(test)]
pub(super) fn probe_frame_for_node(path: &str, node: &PatchNode) -> Option<Arc<ProbeFrame>> {
    let id = probe_node_id(node)?;
    probe_frame_in_instance(&live_audio::patch_probe_instance(path)?, &id)
}

// ---------------------------------------------------------------------------
// Scope view
// ---------------------------------------------------------------------------

/// Default scope node size, in cells: wide enough for a readable trace,
/// about four node rows tall (header row + plot).
pub(super) const SCOPE_DEFAULT_WIDTH: f32 = 28.0;
pub(super) const SCOPE_DEFAULT_HEIGHT: f32 = super::metrics::NODE_HEIGHT * 4.0;
/// Smallest a resize can make the scope: header plus a sliver of plot.
pub(super) const SCOPE_MIN_HEIGHT: f32 = super::metrics::NODE_HEIGHT * 2.5;
/// Headroom added above and below the trace, as a fraction of the span.
const SCOPE_RANGE_HEADROOM: f32 = 0.08;

/// Finite min/max over the ring, or `None` when it holds no finite value.
fn ring_extent(pairs: &[(f32, f32)]) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &(min, max) in pairs {
        for value in [min, max] {
            if value.is_finite() {
                lo = lo.min(value);
                hi = hi.max(value);
            }
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// The y range a scope would show for this ring with no history: the trace's
/// extent with a little headroom, stretched to include 0 when the signal sits
/// near it (a 0.2…0.9 envelope reads better on a 0-based axis than floating),
/// and widened around a flat line so it doesn't collapse to a zero span.
pub(super) fn scope_target_range(pairs: &[(f32, f32)]) -> Option<(f32, f32)> {
    let (mut lo, mut hi) = ring_extent(pairs)?;
    // Include zero when the trace's near edge is within half its far edge's
    // distance of it: 0.2..0.9 or 200..5000 gain a baseline, while 439..441
    // (a steady value with a little wobble) keeps a tight axis so the wobble
    // stays visible. A signal that crosses zero includes it already.
    if lo > 0.0 && lo <= hi * 0.5 {
        lo = 0.0;
    } else if hi < 0.0 && hi >= lo * 0.5 {
        hi = 0.0;
    }
    let span = hi - lo;
    if span <= f32::EPSILON * hi.abs().max(lo.abs()).max(1.0) {
        let pad = (hi.abs() * 0.1).max(1e-3);
        return Some((lo - pad, hi + pad));
    }
    let pad = span * SCOPE_RANGE_HEADROOM;
    Some((lo - pad, hi + pad))
}

/// The y range a scope paints `frame` with: the publisher's eased
/// [`ProbeFrame::display_range`], else this frame's own target range.
pub(super) fn scope_display_range(frame: &ProbeFrame) -> Option<(f32, f32)> {
    frame
        .display_range
        .filter(|range| range.0.is_finite() && range.1.is_finite() && range.0 < range.1)
        .or_else(|| scope_target_range(frame.scope.as_deref()?))
}

/// One plotted column of the scope: x, and the screen rows of the column's
/// max (top) and min (bottom). Rows grow downward.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScopeColumn {
    pub(super) x: f32,
    pub(super) top: f32,
    pub(super) bottom: f32,
}

/// Bin the ring into at most `max_columns` columns spanning `plot` (in the
/// caller's units: cells), each the envelope (min of mins, max of maxes) of
/// its pairs, mapped through `range` and clamped to the plot. Pairs with no
/// finite value are skipped, leaving no column, so a NaN burst shows as a gap.
pub(super) fn scope_columns(
    pairs: &[(f32, f32)],
    plot: crate::layout::Rect,
    range: (f32, f32),
    max_columns: usize,
) -> Vec<ScopeColumn> {
    let count = pairs.len().min(max_columns.max(2));
    if pairs.is_empty() || count == 0 || plot.width <= 0.0 || plot.height <= 0.0 {
        return Vec::new();
    }
    let span = (range.1 - range.0).max(f32::MIN_POSITIVE);
    let row_of = |value: f32| {
        let t = ((value - range.0) / span).clamp(0.0, 1.0);
        plot.row + (1.0 - t) * plot.height
    };
    let last = (count - 1).max(1) as f32;
    (0..count)
        .filter_map(|column| {
            let start = column * pairs.len() / count;
            let end = ((column + 1) * pairs.len() / count).max(start + 1);
            let (lo, hi) = ring_extent(&pairs[start..end.min(pairs.len())])?;
            Some(ScopeColumn {
                x: plot.col + plot.width * column as f32 / last,
                top: row_of(hi),
                bottom: row_of(lo),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Editing: insert on a cable, switch view
// ---------------------------------------------------------------------------

/// Probe text with `@view` set to `view`, keeping the id written in `text`
/// (or minting one against `taken` for a hand-written id-less probe).
/// `None` when `text` is not a probe.
pub(super) fn probe_text_with_view(
    text: &str,
    view: ProbeView,
    taken: &HashSet<String>,
) -> Option<String> {
    let parsed = parse_probe(text)?;
    let id = parsed.id.clone().unwrap_or_else(|| mint_probe_id(taken));
    Some(parsed.build(Some(&id), view.as_str()))
}

/// Every probe id in use for the patcher at `node`: the source's, and the
/// interaction overlay's (except `except_key`'s own edit).
pub(super) fn taken_probe_ids(
    node: &crate::layout::LayoutNode,
    state: &PatcherInteractionState,
    except_key: Option<&str>,
) -> HashSet<String> {
    let root_patch = super::load_patch_from_props(&node.props)
        .ok()
        .map(|(_, root_patch)| root_patch);
    taken_probe_ids_in(root_patch.as_ref(), state, except_key)
}

/// [`taken_probe_ids`] for an already loaded root patch.
pub(super) fn taken_probe_ids_in(
    root_patch: Option<&Patch>,
    state: &PatcherInteractionState,
    except_key: Option<&str>,
) -> HashSet<String> {
    let mut taken = probe_ids_in_edits(state, except_key);
    if let Some(root_patch) = root_patch {
        taken.extend(probe_ids_in_patch(root_patch));
    }
    taken
}

/// Splice a passthrough probe (`@view number` or `scope`) into the selected
/// cable: `source → probe → destination`, the probe centred on the cable's
/// midpoint and selected. One edit-state change, so one undo step. `false`
/// when no cable is selected or it is a feedback cable (a probe there would
/// turn the loop's delayed edge into a forward one).
pub(super) fn insert_probe_on_selected_cable(
    node: &crate::layout::LayoutNode,
    state: &mut PatcherInteractionState,
    view_key: &str,
    view: ProbeView,
) -> bool {
    use super::geometry::{
        connection_endpoints_at, patch_input_indices, patch_input_slot_counts, patch_node_rects,
        patch_output_counts, patcher_zoom, screen_to_model,
    };
    use super::model::{ConnectionKind, InputPortRef, OutputPortRef};
    use super::state::{
        active_patcher_patch, allocate_created_connection, allocate_created_node_avoiding,
        delete_connection_edit_or_mark_deleted, get_patcher_pan_state, node_edit_key,
        note_touched_node, patch_with_interaction_state, patcher_state_key, source_connection_id,
    };

    let Some(cable_id) = state.selected_cable.clone() else {
        return false;
    };
    let Ok((_, root_patch)) = super::load_patch_from_props(&node.props) else {
        return false;
    };
    let patch =
        patch_with_interaction_state(active_patcher_patch(&root_patch, state), state, view_key);
    let Some(connection) = patch
        .connections
        .iter()
        .find(|connection| source_connection_id(connection) == cable_id)
        .cloned()
    else {
        return false;
    };
    if connection.kind == ConnectionKind::Feedback {
        return false;
    }

    let text = {
        let taken = taken_probe_ids(node, state, None);
        format!(
            "{PROBE_OP} @id \"{}\" @view {}",
            mint_probe_id(&taken),
            view.as_str()
        )
    };
    // Centre the new node on the cable's midpoint (in model cells).
    let pan_state = get_patcher_pan_state(patcher_state_key(node));
    let input_indices = patch_input_indices(&patch);
    let midpoint = connection_endpoints_at(
        &connection,
        &patch_node_rects(&patch, node.rect, &pan_state),
        &input_indices,
        &patch_input_slot_counts(&patch, &input_indices),
        &patch_output_counts(&patch),
        patcher_zoom(&pan_state),
    )
    .map(|(start, end)| {
        screen_to_model(
            node.rect,
            &pan_state,
            ((start.0 + end.0) * 0.5, (start.1 + end.1) * 0.5),
        )
    })
    .unwrap_or_else(|| {
        patch
            .nodes
            .iter()
            .find(|candidate| candidate.id == connection.from_node)
            .map(|from| {
                (
                    from.position.0,
                    from.position.1 + super::metrics::LAYER_SPACING,
                )
            })
            .unwrap_or((0.0, 0.0))
    });
    let (width, height) = super::display::node_size_for_text(&text, 1, 1);
    let position = (midpoint.0 - width * 0.5, midpoint.1 - height * 0.5);

    let taken_node_ids = patch
        .nodes
        .iter()
        .map(|patch_node| patch_node.id.clone())
        .collect::<HashSet<_>>();
    let probe_node_id = allocate_created_node_avoiding(state, view_key, position, &taken_node_ids);
    if let Some(edit) = state
        .edit_state
        .nodes
        .get_mut(&node_edit_key(view_key, &probe_node_id))
    {
        edit.text = text;
    }
    delete_connection_edit_or_mark_deleted(state, view_key, &cable_id);
    allocate_created_connection(
        state,
        view_key,
        OutputPortRef {
            node_id: connection.from_node.clone(),
            output_index: connection.from_output,
        },
        InputPortRef {
            node_id: probe_node_id.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        state,
        view_key,
        OutputPortRef {
            node_id: probe_node_id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: connection.to_node.clone(),
            input_index: connection.to_input,
        },
    );
    state.selected_cable = None;
    state.selected_nodes.clear();
    state.selected_nodes.insert(probe_node_id.clone());
    note_touched_node(state, &probe_node_id);
    true
}

/// The single selected node, when it is a probe, with its attributes.
pub(super) fn selected_probe(
    node: &crate::layout::LayoutNode,
    state: &PatcherInteractionState,
    view_key: &str,
) -> Option<(Patch, PatchNode, ProbeAttrs)> {
    if state.selected_nodes.len() != 1 {
        return None;
    }
    let node_id = state.selected_nodes.iter().next()?;
    let (_, root_patch) = super::load_patch_from_props(&node.props).ok()?;
    let patch = super::state::patch_with_interaction_state(
        super::state::active_patcher_patch(&root_patch, state),
        state,
        view_key,
    );
    let patch_node = patch
        .nodes
        .iter()
        .find(|candidate| &candidate.id == node_id)?
        .clone();
    let attrs = ProbeAttrs::of(&patch_node)?;
    Some((patch, patch_node, attrs))
}

/// Switch the single selected probe to `view`, keeping its `@id`. The node's
/// width override is dropped so it takes the new view's default width; a
/// scope's height is kept, so switching back restores it. `false` when the
/// selection is not one probe or it already shows `view`.
pub(super) fn set_selected_probe_view(
    node: &crate::layout::LayoutNode,
    state: &mut PatcherInteractionState,
    view_key: &str,
    view: ProbeView,
) -> bool {
    use super::state::{ensure_source_node_edit, node_edit_key};
    let Some((patch, patch_node, attrs)) = selected_probe(node, state, view_key) else {
        return false;
    };
    if attrs.view == Some(view) {
        return false;
    }
    let key = node_edit_key(view_key, &patch_node.id);
    let current_text = match state.edit_state.nodes.get(&key) {
        Some(edit) => edit.text.clone(),
        None => super::display::editable_node_text(
            &patch_node,
            &super::interaction::inbound_slots_for_node(&patch, &patch_node.id),
        ),
    };
    let taken = taken_probe_ids(node, state, Some(&key));
    let Some(text) = probe_text_with_view(&current_text, view, &taken) else {
        return false;
    };
    ensure_source_node_edit(state, view_key, &patch_node, current_text);
    let Some(edit) = state.edit_state.nodes.get_mut(&key) else {
        return false;
    };
    edit.text = text;
    edit.width = None;
    true
}
