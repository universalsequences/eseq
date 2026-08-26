pub(super) const DEFAULT_WIDTH: f32 = 96.0;
pub(super) const DEFAULT_HEIGHT: f32 = 38.0;
pub(super) const NODE_MIN_WIDTH: f32 = 5.8;
pub(super) const NODE_HEIGHT: f32 = 1.58;
pub(super) const CODE_NODE_MIN_WIDTH: f32 = 14.0;
pub(super) const CODE_NODE_HEIGHT: f32 = 3.7;
pub(super) const PORT_MIN_CENTER_SPACING_CELLS: f32 = 2.8;
pub(super) const CREATED_NODE_BELOW_GAP_CELLS: f32 = 1.6;
pub(super) const LAYER_SPACING: f32 = 4.95;
pub(super) const NODE_COLUMN_GAP: f32 = 3.0;
pub(super) const VIEW_PADDING_X: f32 = 4.0;
pub(super) const VIEW_PADDING_Y: f32 = 4.0;
pub(super) const PATCH_ORIGIN_COL_OFFSET: f32 = 2.0;
pub(super) const PATCH_ORIGIN_ROW_OFFSET: f32 = 2.4;
pub(super) const DEFAULT_ZOOM: f32 = 0.7;
pub(super) const MIN_ZOOM: f32 = 0.35 / 3.0;
pub(super) const MAX_ZOOM: f32 = 2.5;
pub(super) const NODE_BORDER_WIDTH_PX: f32 = 3.75;
pub(super) const NODE_CORNER_RADIUS_PX: f32 = 28.0;
pub(super) const NODE_FONT_SIZE: f32 = 16.0;
/// Completion morph for cmd+k bubbles: the bubble card's chrome eases into the
/// node's rounded chrome over the shape window, then the "just materialized"
/// border tint settles back to the resting border over the colour window.
pub(super) const AGENTIC_MORPH_SHAPE_SECS: f32 = 0.28;
/// Grow-in for a freshly opened cmd+k bubble: it scales up from
/// `AGENTIC_APPEAR_START_SCALE`, fades in, and relaxes from an over-rounded
/// start down to the card's resting radius — the mirror of the way it opens
/// back up into the node's radius on completion.
pub(super) const AGENTIC_APPEAR_SECS: f32 = 0.18;
pub(super) const AGENTIC_APPEAR_START_SCALE: f32 = 0.72;
pub(super) const AGENTIC_APPEAR_START_RADIUS_PX: f32 = 30.0;

/// Resting corner radius of the bubble card, in design pixels at zoom 1.
///
/// Every agentic radius is quoted in the macOS 2x design-pixel space, converted
/// by `ui_design_px`, and scaled by `zoom` at the point of use.
/// `normalized_corner_radius` divides by the rect's framebuffer-pixel height,
/// so that product is the only form that holds a constant proportional radius
/// as the card grows line by line *and* as the canvas zooms — quoting a radius
/// in cells would make a one-line bubble and a seven-line bubble round
/// differently. It also has to stay under `normalized_corner_radius`' 0.5 clamp at the
/// smallest box it is used on, or that box silently renders as a pill.
pub(super) const AGENTIC_CARD_RADIUS_PX: f32 = 16.0;
/// Radius of the inset text box, concentric with the card: the card's radius
/// less the padding between the two, so both curves stay parallel. Small enough
/// that a one-line box still reads as a rounded rect rather than a pill.
pub(super) const AGENTIC_INNER_RADIUS_PX: f32 = 8.0;
/// Hairlines. `NODE_BORDER_WIDTH_PX` is a node's heavy stroke; the bubble is
/// carried by its surface and its inset box, not by its outline.
pub(super) const AGENTIC_CARD_BORDER_WIDTH_PX: f32 = 1.5;
pub(super) const AGENTIC_INNER_BORDER_WIDTH_PX: f32 = 1.15;

/// Bubble layout, in unscaled layout cells. The card is padding, then a header
/// row that sits *outside* the text box, then one or two inset boxes.
pub(super) const AGENTIC_CARD_PAD_X: f32 = 0.62;
pub(super) const AGENTIC_CARD_PAD_TOP: f32 = 0.55;
pub(super) const AGENTIC_CARD_PAD_BOTTOM: f32 = 0.62;
/// Tall enough to hold the spinner, which is the tallest thing on the row. The
/// status text is centred within it rather than sitting on its top edge.
pub(super) const AGENTIC_HEADER_ROW_H: f32 = 1.62;
/// Rough line box of the header text at `AGENTIC_HEADER_FONT_SIZE`, in cells,
/// used only to centre it in `AGENTIC_HEADER_ROW_H`.
pub(super) const AGENTIC_HEADER_TEXT_ROWS: f32 = 0.98;
pub(super) const AGENTIC_HEADER_TO_BOX_GAP: f32 = 0.42;
pub(super) const AGENTIC_INNER_PAD_X: f32 = 0.58;
pub(super) const AGENTIC_INNER_PAD_Y: f32 = 0.42;
pub(super) const AGENTIC_LINE_H: f32 = 1.3;
/// A composer box reserves a row under its text for the send chevron, so the
/// button never has to be dodged by the wrap.
pub(super) const AGENTIC_SEND_ROW_H: f32 = 1.75;
/// The disc, not the glyph — the glyph keeps its own font size, so growing this
/// is what puts padding around it.
pub(super) const AGENTIC_SEND_DIAMETER_PX: f32 = 30.0;
/// Blank cells between the spinner and the status word. The slot the spinner
/// takes is *derived* from its pixel diameter rather than fixed in cells — a
/// constant cell width drifts off the dots as the cell aspect changes, which is
/// how the status text ended up sitting on top of them.
pub(super) const AGENTIC_SPINNER_GAP: f32 = 0.5;
pub(super) const AGENTIC_SPINNER_DIAMETER_PX: f32 = 36.0;
/// Top of the first inset box, measured from the top of the card.
pub(super) const AGENTIC_INNER_TOP: f32 =
    AGENTIC_CARD_PAD_TOP + AGENTIC_HEADER_ROW_H + AGENTIC_HEADER_TO_BOX_GAP;
/// Prompt text and cursor are held back until the box has essentially formed.
/// Fading them instead would rekey the proportional-text vertex cache on every
/// animation frame, since the run's colour is part of that cache key.
pub(super) const AGENTIC_APPEAR_TEXT_AT: f32 = 0.55;
/// Escape plays the grow-in backwards, a little quicker than it opened.
pub(super) const AGENTIC_CLOSE_SECS: f32 = 0.14;
/// Frames to keep asking for after an agentic animation has played out.
///
/// A widget's cached primitive run is only refreshed while it is animating
/// (`widget_render::active_animation_widgets`), so the last frame an animation
/// *draws* is the one that stays on screen — the settled frame is never
/// rendered. That is invisible for an animation ending with something on
/// screen, but a shrink-out ends with nothing: the final shrinking frame was
/// being left behind as a ghost until an unrelated event (a mouse move) marked
/// the patcher dirty. Running a beat past the end buys the frame that actually
/// draws the bubble gone.
pub(super) const AGENTIC_ANIMATION_SETTLE_SECS: f32 = 0.12;
/// An answer arrives in a much wider, taller box than the pending spinner it
/// replaces, so the box eases between the two layouts instead of jumping. The
/// answer text waits for the box to be mostly grown or it would overflow it.
pub(super) const AGENTIC_ANSWER_RESIZE_SECS: f32 = 0.28;
pub(super) const AGENTIC_ANSWER_TEXT_AT: f32 = 0.6;
/// Rows between the answer box and the follow-up composer box below it.
pub(super) const AGENTIC_BOX_GAP: f32 = 0.5;
pub(super) const AGENTIC_MORPH_COLOR_SECS: f32 = 0.6;
pub(super) const CODE_NODE_FONT_SIZE: f32 = 11.0;
pub(super) const NODE_TEXT_COL_OFFSET: f32 = 0.92;
pub(super) const NODE_RESIZE_HANDLE_SIZE_CELLS: f32 = 0.42;
pub(super) const NODE_RESIZE_HANDLE_HIT_SIZE_CELLS: f32 = 1.05;
pub(super) const PORT_OUTER_DIAMETER_PX: f32 = 27.0;
pub(super) const PORT_INNER_DIAMETER_PX: f32 = 18.75;
pub(super) const PORT_EDGE_PADDING_CELLS: f32 = 2.15;
pub(super) const CABLE_TARGET_RADIUS_CELLS: f32 = 2.25;
pub(super) const CABLE_HIT_RADIUS_CELLS: f32 = 0.4;
pub(super) const CABLE_HANDLE_DISTANCE_CELLS: f32 = 1.15;
/// Upper bound on the handle inset as a fraction of the cable's span, so both
/// handles stay on their own half of a short cable.
pub(super) const CABLE_HANDLE_MAX_SPAN_FRACTION: f32 = 0.35;
pub(super) const CABLE_HANDLE_HIT_RADIUS_CELLS: f32 = 0.9;
pub(super) const CABLE_HANDLE_RADIUS_PX: f32 = 13.0;
pub(super) const CABLE_FEEDBACK_RADIUS_PX: f32 = 3.6;
pub(super) const CABLE_FORWARD_RADIUS_PX: f32 = 4.4;
pub(super) const SEGMENTED_CABLE_CORNER_RADIUS_CELLS: f32 = 0.72;
pub(super) const SEGMENTED_CABLE_DRAG_PADDING_CELLS: f32 = 0.35;
pub(super) const SEGMENTED_CABLE_DRAG_EXTRA_RANGE_CELLS: f32 = 5.4;
pub(super) const TOUCHPAD_PAN_SPEED_CELLS_PER_PIXEL: f32 = 0.05;
pub(super) const WHEEL_PAN_STEP_CELLS: f32 = 3.0;
pub(super) const PAN_OVERSCROLL_VIEWPORT_FACTOR: f32 = 1.0;
pub(super) const PAN_OVERSCROLL_MIN_CELLS: f32 = 48.0;
