use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MetalPrimitive, MetalProportionalTextPrimitive, MetalQuadPrimitive,
    MetalRectPrimitive, MetalTrianglePrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent,
    WidgetInstance, WidgetKeyEvent, WidgetViewport, ndc_bounds, resolve_named_color, styled_cell,
    time_view::{TimeRuler, TimeRulerMode, TimeViewport},
};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

const ALIGNMENT_HELPER_BACKWARD_SNAP_PROXIMITY: f64 = 0.25;

pub struct TimelineWidget;

pub static TIMELINE_WIDGET: TimelineWidget = TimelineWidget;

#[derive(Clone)]
struct TimelineLane {
    _id: Value,
    label: Option<String>,
    sidebar_bg: Option<crate::backend::Color>,
    label_fg: Option<crate::backend::Color>,
}

#[derive(Clone)]
struct TimelineItem {
    id: Value,
    lane: usize,
    start: f64,
    end: f64,
    selected: bool,
    label: Option<String>,
    color: Option<crate::backend::Color>,
    /// Rendering hint only (docs/arrangement-timeline-ui-spec.md 7): parsed
    /// so hosts can tag clips today; kinds gain distinct visuals as clip
    /// types diverge, so nothing consumes it yet.
    #[allow(dead_code)]
    kind: Option<TimelineItemKind>,
    content: Option<TimelineItemContent>,
}

/// What kind of clip an item represents (docs/arrangement-timeline-ui-spec.md
/// 7). Deliberately decoupled from `TimelineItemContent`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TimelineItemKind {
    Midi,
    Audio,
    Scene,
}

/// Content drawn inside an item's rect. The widget stays dumb: dots and peak
/// buckets are pre-normalized by the host; no musical or audio-decoding
/// knowledge lives here. `cycle` is the fraction of the item span one
/// repetition covers (0..1]; content repeats at that period with a separator
/// line per boundary (a looping clip shows its cycles, DAW-style). 1.0 (the
/// default) means the content spans the whole item — no tiling.
#[derive(Clone)]
enum TimelineItemContent {
    Dots { dots: Vec<TimelineDot>, cycle: f64 },
    Peaks(Vec<PeakBucket>),
}

#[derive(Clone, Copy)]
struct TimelineDot {
    /// 0.0..1.0 within the item's [start, end).
    offset: f64,
    /// 0.0..1.0 vertical placement within the item rect (1.0 = top).
    value: f64,
    /// Note length as a fraction of the item's span, in the same normalized
    /// axis as `offset` (docs/arrangement-region-editing-spec.md 3.2). `0`
    /// (the default) draws the legacy point dot; anything larger draws a bar.
    width: f64,
}

/// One min/max amplitude bucket of a precomputed waveform peak cache
/// (docs/arrangement-timeline-ui-spec.md 7.2). Parsed now; rendering lands
/// with the audio-track asset pipeline (spec 3 non-goal), so the fields are
/// not read yet.
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct PeakBucket {
    min: f32, // -1.0..1.0
    max: f32,
}

#[derive(Clone)]
struct TimelineSelectionRect {
    time_a: f64,
    time_b: f64,
    lane_a: usize,
    lane_b: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TimelineTool {
    Pointer,
    Draw,
    Erase,
    Marquee,
    Pan,
    Scrub,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarStyle {
    Default,
    Piano,
}

#[derive(Clone)]
struct TimelineView {
    rect: Rect,
    header_height: f32,
    sidebar_width: f32,
    view_start: f64,
    view_duration: f64,
    zoom_min_duration: f64,
    zoom_max_duration: f64,
    /// `:grid-density` — divides the zoom-adaptive grid step, so `2` gives
    /// twice as many grid cells at every zoom. Both the drawn grid and every
    /// `:grid` snap (marquee, resize) follow it, so a host that wants finer
    /// selection without zooming in raises this. Default `1` leaves the stock
    /// ladder, which is what the piano roll and every other host get.
    grid_density: f64,
    content_length: Option<f64>,
    content_length_min: f64,
    content_length_max: f64,
    time_ruler: Option<TimeRuler>,
    playhead_time: Option<f64>,
    cursor_time: Option<f64>,
    item_color: crate::backend::Color,
    loop_color: crate::backend::Color,
    sidebar_style: SidebarStyle,
    lane_scroll: f64,
    lane_height: Option<f32>,
    /// Corner radius of item fills, in CELLS so it scales with the UI zoom
    /// like every other timeline dimension. `0` — the default — keeps the
    /// square quads every host drew before; larger values round the clip
    /// (GarageBand-style).
    item_corner_radius: f32,
    /// Height (cells) of the Ableton-style clip title bar
    /// (docs/arrangement-region-editing-spec.md 3.1). `0` — the default and
    /// what the piano roll passes — reproduces the pre-title-bar behavior
    /// exactly: no title-bar zone, no start-edge handle, body drags move.
    title_bar_height: f32,
    scroll_viewport_height: Option<f32>,
    snap: f64,
    resize_snap: f64,
    resize_snap_to_grid: bool,
    /// `:marquee-snap :grid` — marquee spans are emitted quantized to the
    /// zoom-adaptive grid ladder, min down / max up
    /// (docs/arrangement-region-editing-spec.md 4.3), so a drag always selects
    /// whole grid cells. Default off: the piano roll keeps raw pointer times.
    marquee_snap_to_grid: bool,
    snap_floor: bool,
    resize_snap_floor: bool,
    min_duration: Option<f64>,
    create_duration: Option<f64>,
    move_alignment_helper: bool,
    resize_alignment_helper: bool,
    smooth_scroll: bool,
    /// `:scroll-passthrough :vertical` — this instance owns no vertical
    /// scrolling: vertical-dominant wheel/touchpad deltas are declined so an
    /// enclosing scroll container can handle them, while horizontal deltas
    /// still pan the shared time axis.
    vertical_scroll_passthrough: bool,
    tool: TimelineTool,
    lanes: Vec<TimelineLane>,
    items: Vec<TimelineItem>,
    selection: Vec<Value>,
    selection_rect: Option<TimelineSelectionRect>,
    selection_rect_style: SelectionRectStyle,
}

/// How `:selection-rect` paints (`:selection-rect-style`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectionRectStyle {
    /// Default: a translucent blue wash with a bright outline — a marquee
    /// lassoing items, which is what the piano roll and the scene lane draw.
    Marquee,
    /// `:region` — Ableton's arrangement region
    /// (docs/arrangement-region-editing-spec.md 4.4): the selection LIGHTS the
    /// lane instead of shading over it. Empty lane background inside the span
    /// takes the selected-body colour, and every clip's BODY inside the span
    /// takes the same colour while its title bar keeps the clip colour, so the
    /// highlight reads as one continuous band across clips and gaps alike.
    Region,
}

#[derive(Clone)]
enum HitRegion {
    Header,
    ContentLengthEnd,
    Sidebar { lane: usize },
    Background { time: f64 },
    ItemBody { item: TimelineItem },
    ItemEdgeEnd { item: TimelineItem },
    /// Title-bar zones, only produced when `title-bar-height > 0`
    /// (docs/arrangement-region-editing-spec.md 3.1).
    ItemTitleBar { item: TimelineItem },
    ItemEdgeStart { item: TimelineItem },
}

/// Which edge the pointer is over. Both are grabbable on a title-barred
/// clip, so the highlight has to say WHICH one — with narrow handles and
/// abutting clips, "some edge here" is exactly the ambiguity that made
/// resizing feel like a coin flip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HoverEdge {
    Start,
    End,
}

thread_local! {
    static TIMELINE_HOVER_EDGE: RefCell<Option<(u64, Value, HoverEdge)>> =
        const { RefCell::new(None) };
}

fn set_timeline_hover_edge(widget_id: u64, hover: Option<(Value, HoverEdge)>) {
    TIMELINE_HOVER_EDGE.with(|state| {
        let mut state = state.borrow_mut();
        let next = hover.map(|(id, edge)| (widget_id, id, edge));
        if *state != next {
            *state = next;
            super::bump_widget_state_generation();
        }
    });
}

fn timeline_hover_edge_for(widget_id: u64, item_id: &Value) -> Option<HoverEdge> {
    TIMELINE_HOVER_EDGE.with(|state| {
        state
            .borrow()
            .as_ref()
            .and_then(|(hover_widget_id, hover_item_id, edge)| {
                (*hover_widget_id == widget_id && hover_item_id == item_id).then_some(*edge)
            })
    })
}

#[cfg(target_os = "macos")]
fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

#[cfg(target_os = "macos")]
fn push_rounded_rect(
    primitives: &mut Vec<MetalPrimitive>,
    rect: Rect,
    color: crate::backend::Color,
    viewport: WidgetViewport,
    radius_px: f32,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    primitives.push(MetalPrimitive::WidgetInstance {
        widget_type: "box".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
            color_a: color.to_rgba(),
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: normalized_corner_radius(rect, viewport, radius_px),
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background: false,
    });
}

impl WidgetDefinition for TimelineWidget {
    fn names(&self) -> &'static [&'static str] {
        &["timeline"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "header-height", "sidebar-width"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["playhead-time", "cursor-time"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(constraints.max_width)
            .clamp(1.0, constraints.max_width.max(1.0));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(8.0)
            .max(1.0);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let view = TimelineView::from_props(props, rect);
        let content = view.content_rect();
        if content.width == 0.0 || content.height == 0.0 {
            return;
        }

        for row_offset in 0..(rect.height.round() as u16) {
            let row = rect.row.round() as u16 + row_offset;
            for col_offset in 0..(rect.width.round() as u16) {
                let col = rect.col.round() as u16 + col_offset;
                buf.set(row, col, styled_cell(' ', theme::FG(), Some(theme::BG())));
            }
        }

        if view.header_height > 0.0 {
            let header_row = rect.row.round() as u16;
            for col_offset in 0..(rect.width.round() as u16) {
                let col = rect.col.round() as u16 + col_offset;
                buf.set(
                    header_row,
                    col,
                    styled_cell(' ', theme::FG(), Some(theme::STATUS_BG())),
                );
            }
            for (absolute_col, is_major) in view.grid_columns() {
                if header_row < (rect.row + rect.height).round() as u16 {
                    buf.set(
                        header_row,
                        absolute_col,
                        styled_cell(
                            '|',
                            if is_major {
                                theme::PURPLE()
                            } else {
                                theme::BRIGHT_BLACK()
                            },
                            Some(theme::STATUS_BG()),
                        ),
                    );
                }
            }
            for (absolute_col, label) in view.time_ruler_labels() {
                for (offset, ch) in label.chars().enumerate() {
                    let col = absolute_col + 1 + offset as u16;
                    if col >= (rect.col + rect.width).round() as u16 {
                        break;
                    }
                    buf.set(
                        header_row,
                        col,
                        styled_cell(ch, theme::FG(), Some(theme::STATUS_BG())),
                    );
                }
            }
        }

        for row_offset in 0..(content.height.round() as u16) {
            let row = content.row.round() as u16 + row_offset;
            for col_offset in 0..(content.width.round() as u16) {
                let col = content.col.round() as u16 + col_offset;
                let lane = view.lane_at_row(row as f32);
                let bg = if lane % 2 == 0 {
                    theme::BLACK()
                } else {
                    theme::BG()
                };
                buf.set(row, col, styled_cell(' ', theme::FG(), Some(bg)));
            }
        }

        for (col, is_major) in view.grid_columns() {
            for row_offset in 0..(content.height.round() as u16) {
                let row = content.row.round() as u16 + row_offset;
                buf.set(
                    row,
                    col,
                    styled_cell(
                        '|',
                        if is_major {
                            theme::BRIGHT_BLACK()
                        } else {
                            crate::backend::Color::from_hex(0x33, 0x33, 0x38)
                        },
                        None,
                    ),
                );
            }
        }

        if let Some(playhead_col) = view.playhead_col() {
            if view.header_height > 0.0 && rect.row < rect.row + rect.height {
                buf.set(
                    rect.row.round() as u16,
                    playhead_col,
                    styled_cell('|', theme::YELLOW(), Some(theme::STATUS_BG())),
                );
            }
            for row_offset in 0..(content.height.round() as u16) {
                let row = content.row.round() as u16 + row_offset;
                buf.set(row, playhead_col, styled_cell('|', theme::YELLOW(), None));
            }
        }

        if let Some(cursor_col) = view.cursor_col() {
            if content.height > 0.0 {
                let marker_row = content.row.round() as u16;
                buf.set(
                    marker_row,
                    cursor_col,
                    styled_cell('▼', theme::CURSOR(), None),
                );
            }
            for row_offset in 1..(content.height.round() as u16) {
                let row = content.row.round() as u16 + row_offset;
                buf.set(row, cursor_col, styled_cell('|', theme::CURSOR(), None));
            }
        }

        for (lane_index, lane) in view.lanes.iter().enumerate() {
            let Some((row_start, row_end)) = view.visible_lane_rows(lane_index) else {
                continue;
            };
            if view.sidebar_width > 0.0 {
                let sidebar_bg = lane.sidebar_bg.unwrap_or(theme::BLACK());
                let label_fg = lane.label_fg.unwrap_or(theme::FG());
                for row in row_start..row_end {
                    for col_offset in 0..(view.sidebar_width.round() as u16) {
                        let col = rect.col.round() as u16 + col_offset;
                        buf.set(row, col, styled_cell(' ', label_fg, Some(sidebar_bg)));
                    }
                }
                let label = lane.label.as_deref().unwrap_or("");
                for (idx, ch) in label.chars().take(view.sidebar_width as usize).enumerate() {
                    let col = rect.col.round() as u16 + idx as u16;
                    if row_start < (rect.row + rect.height).round() as u16 {
                        buf.set(row_start, col, styled_cell(ch, label_fg, Some(sidebar_bg)));
                    }
                }
                for row in row_start..row_end {
                    let divider_col = rect.col.round() as u16
                        + (view.sidebar_width - 1.0).max(0.0).round() as u16;
                    if divider_col < (rect.col + rect.width).round() as u16 {
                        buf.set(
                            row,
                            divider_col,
                            styled_cell('│', theme::BRIGHT_BLACK(), Some(sidebar_bg)),
                        );
                    }
                }
            }
        }

        for item in &view.items {
            let Some(item_rect) = view.item_rect(item) else {
                continue;
            };
            let item_color = if view.item_selected(item) {
                theme::PURPLE()
            } else {
                item.color.unwrap_or(theme::WHITE())
            };
            for row_offset in 0..(item_rect.height.round() as u16) {
                let row = item_rect.row.round() as u16 + row_offset;
                for col_offset in 0..(item_rect.width.round() as u16) {
                    let col = item_rect.col.round() as u16 + col_offset;
                    buf.set(row, col, styled_cell(' ', item_color, Some(item_color)));
                }
            }
        }
    }

    fn begin_gesture(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
        _modifiers: KeyModifiers,
    ) -> Option<Value> {
        let view = TimelineView::from_props(&node.props, node.rect);
        view.begin_gesture(local_col, scroll_adjusted_row(local_row))
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        let local_row = scroll_adjusted_row(local_row);
        let view = TimelineView::from_props(&node.props, node.rect);
        match mouse_kind {
            MouseEventKind::Moved => {
                let hovered_edge = match view.hit_test(local_col, local_row) {
                    Some(HitRegion::ItemEdgeEnd { item }) => Some((item.id, HoverEdge::End)),
                    Some(HitRegion::ItemEdgeStart { item }) => Some((item.id, HoverEdge::Start)),
                    _ => None,
                };
                set_timeline_hover_edge(node.widget_id, hovered_edge);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Down(MouseButton::Left) => view
                .handle_pointer_down(local_col, local_row)
                .map(WidgetEvent::Custom)
                .map(MouseEventOutcome::Dispatch)
                .unwrap_or(MouseEventOutcome::Consume),
            MouseEventKind::Drag(MouseButton::Left) => view
                .handle_pointer_drag(local_col, local_row, gesture)
                .map(WidgetEvent::Custom)
                .map(MouseEventOutcome::Dispatch)
                .unwrap_or(MouseEventOutcome::Consume),
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => view
                .handle_scroll(mouse_kind, local_col, local_row)
                .map(WidgetEvent::Custom)
                .map(MouseEventOutcome::Dispatch)
                .unwrap_or(MouseEventOutcome::Consume),
            MouseEventKind::Up(MouseButton::Left) => view
                .handle_pointer_up(local_col, local_row, gesture)
                .map(WidgetEvent::Custom)
                .map(MouseEventOutcome::Dispatch)
                .unwrap_or(MouseEventOutcome::Consume),
            _ => MouseEventOutcome::Consume,
        }
    }

    fn cursor(&self, node: &LayoutNode, local_col: f32, local_row: f32) -> super::WidgetCursor {
        let view = TimelineView::from_props(&node.props, node.rect);
        match view.hit_test(local_col, scroll_adjusted_row(local_row)) {
            Some(HitRegion::ItemEdgeEnd { .. })
            | Some(HitRegion::ItemEdgeStart { .. })
            | Some(HitRegion::ContentLengthEnd) => super::WidgetCursor::EwResize,
            Some(HitRegion::ItemTitleBar { .. }) => super::WidgetCursor::Grab,
            _ => super::WidgetCursor::Default,
        }
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let view = TimelineView::from_props(&node.props, node.rect);
        view.handle_key(key).map(WidgetEvent::Custom)
    }

    fn double_click_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
    ) -> Option<WidgetEvent> {
        let view = TimelineView::from_props(&node.props, node.rect);
        view.handle_double_click(local_col, scroll_adjusted_row(local_row))
            .map(WidgetEvent::Custom)
    }

    fn magnify_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
        delta: f64,
    ) -> Option<WidgetEvent> {
        let view = TimelineView::from_props(&node.props, node.rect);
        view.handle_magnify(local_col, scroll_adjusted_row(local_row), delta)
            .map(WidgetEvent::Custom)
    }

    fn scroll_gesture_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> Option<WidgetEvent> {
        let view = TimelineView::from_props(&node.props, node.rect);
        view.handle_touchpad_scroll(local_col, scroll_adjusted_row(local_row), delta_x, delta_y)
            .map(WidgetEvent::Custom)
    }

    fn captures_scroll_gesture(&self, node: &LayoutNode) -> bool {
        // Pass-through instances let declined (vertical) gestures bubble to
        // the enclosing scroll container instead of swallowing them.
        !vertical_scroll_passthrough(&node.props)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        build_metal_primitives(node, viewport)
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let callback = node.props.get("on-action")?.clone();
        let action = match event {
            WidgetEvent::Custom(action) => Some(action),
            _ => None,
        };
        Some(EventOutput {
            callback,
            args: vec![action?],
        })
    }
}

/// Test accessor for a laid-out timeline's grid: `(grid step, grid line xs,
/// ruler labels)`. Hosts compose several timeline instances over one shared
/// time axis (the arrangement's ruler lane plus one lane per track), and every
/// one of them must land on the SAME grid — it is what both the drawn lines
/// and `:grid` snapping quantize to. This exists so a test can assert that
/// across instances, which no public render output makes checkable.
#[cfg(target_os = "macos")]
pub fn debug_grid(node: &LayoutNode) -> (f64, Vec<f32>, Vec<(f32, String)>) {
    let view = TimelineView::from_props(&node.props, node.rect);
    let vp = view.time_viewport();
    (
        view.alignment_helper_grid_step(),
        vp.metal_grid_lines(view.time_ruler.as_ref()).into_iter().map(|(x, _)| x).collect(),
        vp.metal_time_ruler_labels(view.time_ruler.as_ref()),
    )
}

#[cfg(target_os = "macos")]
fn build_metal_primitives(
    node: &LayoutNode,
    viewport: super::WidgetViewport,
) -> Vec<MetalPrimitive> {
    if node.widget_type != "timeline" {
        return Vec::new();
    }

    let rect = node.rect;
    let view = TimelineView::from_props(&node.props, rect);
    let content = view.content_rect();
    let mut primitives = Vec::new();

    if view.header_height > 0.0 {
        let loop_band = view.loop_band_rect().map(|(x, width)| {
            let y = rect.row + (view.header_height * 0.55).min(view.header_height - 0.18);
            let bottom_inset = 0.08_f32.min(view.header_height * 0.12);
            let height = (view.header_height - (y - rect.row) - bottom_inset).max(0.12);
            (x, y, width, height)
        });
        primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
            rect: Rect {
                row: rect.row,
                col: rect.col,
                width: rect.width,
                height: view.header_height,
            },
            color: theme::STATUS_BG(),
        }));
        if let Some((x, y, width, height)) = loop_band {
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y,
                width,
                height,
                color: crate::backend::Color {
                    a: 0.16,
                    ..view.loop_color
                },
            }));
        }
        for (x, _) in view.metal_grid_lines() {
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: x - 0.0625,
                y: rect.row,
                width: 0.125,
                height: view.header_height,
                color: theme::BRIGHT_BLACK(),
            }));
        }
        for (x, label) in view.metal_time_ruler_labels() {
            let label_col = x + 0.36;
            let label_width = label.chars().count() as f32 * 0.58 + 0.28;
            let label_row = rect.row
                + if view.header_height >= 1.6 {
                    0.26
                } else {
                    0.06
                };
            primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: Rect {
                    row: label_row - 0.04,
                    col: label_col - 0.10,
                    width: label_width,
                    height: 0.86,
                },
                color: theme::STATUS_BG(),
            }));
            primitives.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: label_row,
                    col: label_col,
                    align_width: 0.0,
                    h_align: 0.0,
                    text: label,
                    font_size: 10.5,
                    scale: 1.0,
                    fg: theme::FG_MUTED(),
                    bg: theme::STATUS_BG(),
                },
            ));
        }
        if let Some((x, y, width, height)) = loop_band {
            let border_color = crate::backend::Color {
                a: 0.95,
                ..view.loop_color
            };
            let h = (1.0 / viewport.cell_h).min(height);
            let v = (1.0 / viewport.cell_w).min(width);
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y,
                width,
                height: h,
                color: border_color,
            }));
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y: y + height - h,
                width,
                height: h,
                color: border_color,
            }));
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y,
                width: v,
                height,
                color: border_color,
            }));
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: x + width - v,
                y,
                width: v,
                height,
                color: border_color,
            }));
        }
    }

    for lane_index in 0..view.lanes.len() {
        let Some((row_start, lane_height)) = view.metal_lane_rect(lane_index) else {
            continue;
        };
        if view.sidebar_width > 0.0 {
            let lane = &view.lanes[lane_index];
            let sidebar_bg = lane.sidebar_bg.unwrap_or(theme::BLACK());
            if view.sidebar_style == SidebarStyle::Piano {
                let white_key = theme::WHITE();
                let border_color = crate::backend::Color::from_hex(0x1a, 0x1a, 0x1d);
                let is_black_key = sidebar_bg == theme::BLACK();
                primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                    x: rect.col,
                    y: row_start,
                    width: view.sidebar_width,
                    height: lane_height,
                    color: white_key,
                }));
                let separator_height = (1.0 / viewport.cell_h).min(lane_height);
                push_rounded_rect(
                    &mut primitives,
                    Rect {
                        row: row_start + lane_height * 0.5 - separator_height * 0.5,
                        col: rect.col,
                        width: view.sidebar_width,
                        height: separator_height,
                    },
                    border_color,
                    viewport,
                    0.0,
                );
                if is_black_key {
                    let black_width = (view.sidebar_width * 0.66).max(0.0);
                    let black_height = (lane_height * 0.78).max(0.24);
                    let black_color = crate::backend::Color::from_hex(0x05, 0x05, 0x06);
                    let black_rect = Rect {
                        row: row_start + ((lane_height - black_height) * 0.5),
                        col: rect.col,
                        width: black_width,
                        height: black_height,
                    };
                    let cap_width = (black_height * 0.7).min(black_width);
                    let body_width = (black_width - cap_width * 0.5).max(0.0);
                    primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                        x: black_rect.col,
                        y: black_rect.row,
                        width: body_width,
                        height: black_rect.height,
                        color: black_color,
                    }));
                    push_rounded_rect(
                        &mut primitives,
                        Rect {
                            row: black_rect.row,
                            col: black_rect.col + black_width - cap_width,
                            width: cap_width,
                            height: black_rect.height,
                        },
                        black_color,
                        viewport,
                        16.0,
                    );
                }
            } else {
                primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                    x: rect.col,
                    y: row_start,
                    width: view.sidebar_width,
                    height: lane_height,
                    color: sidebar_bg,
                }));
            }
            // Lane label text
            let label = lane.label.as_deref().unwrap_or("");
            if !label.is_empty() {
                let label_fg = lane.label_fg.unwrap_or(theme::FG());
                primitives.push(MetalPrimitive::ProportionalText(
                    MetalProportionalTextPrimitive {
                        row: row_start + ((lane_height - 1.0).max(0.0) * 0.5)
                            - 0.02
                            - if view.sidebar_style == SidebarStyle::Piano {
                                lane_height * 0.5
                            } else {
                                0.0
                            },
                        col: if view.sidebar_style == SidebarStyle::Piano {
                            rect.col + 0.24
                        } else {
                            rect.col + 0.12
                        },
                        align_width: 0.0,
                        h_align: 0.0,
                        text: label.to_string(),
                        font_size: 10.5,
                        scale: 1.0,
                        fg: label_fg,
                        bg: if view.sidebar_style == SidebarStyle::Piano {
                            theme::WHITE()
                        } else {
                            sidebar_bg
                        },
                    },
                ));
            }
        }
        let lane = &view.lanes[lane_index];
        let sidebar_bg = lane.sidebar_bg.unwrap_or(theme::BLACK());
        let grid_color = if sidebar_bg == theme::WHITE() {
            crate::backend::Color::from_hex(0x16, 0x16, 0x18)
        } else {
            crate::backend::Color::from_hex(0x0d, 0x0d, 0x0f)
        };
        primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x: content.col,
            y: row_start,
            width: content.width,
            height: lane_height,
            color: grid_color,
        }));
    }

    let grid_lines = view.metal_grid_lines();
    for &(x, is_major) in &grid_lines {
        primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x: x - 0.0625,
            y: content.row,
            width: 0.125,
            height: content.height,
            color: if is_major {
                theme::BRIGHT_BLACK()
            } else {
                crate::backend::Color::from_hex(0x33, 0x33, 0x38)
            },
        }));
    }

    if let Some((x, y, width, height)) = view.unavailable_rect() {
        primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x,
            y,
            width,
            height,
            color: crate::backend::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.36,
            },
        }));
    }

    // The region span this instance draws, if any: reused below to light each
    // clip's body over the same x-range, so the highlight is continuous across
    // clips and the gaps between them.
    let region_span = match (view.selection_rect_style, view.metal_selection_rect()) {
        (SelectionRectStyle::Region, Some((x, _, width, _))) => Some((x, x + width)),
        _ => None,
    };
    if let Some((x, y, width, height)) = view.metal_selection_rect() {
        if view.selection_rect_style == SelectionRectStyle::Region {
            // Lane background inside the region: the SAME colour a selected
            // clip's body takes, so an empty lane and a clip body inside the
            // selection read as one band (Ableton's arrangement region).
            // Clips draw over this and re-light their own bodies below.
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y,
                width,
                height,
                color: SELECTED_ITEM_BODY_COLOR,
            }));
            // The fill is opaque — it REPLACES the lane colour rather than
            // shading it, which is what makes a bare lane and a clip body read
            // as one band — so the lane's grid would vanish under it. Redraw
            // it on top in the same dark wash clips use for their in-body grid
            // continuation, so the bar lines stay legible and identical either
            // side of a clip edge.
            for &(grid_x, is_major) in &grid_lines {
                if grid_x <= x || grid_x >= x + width {
                    continue;
                }
                primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                    x: grid_x - 0.0625,
                    y,
                    width: 0.125,
                    height,
                    color: crate::backend::Color {
                        r: 0.02,
                        g: 0.025,
                        b: 0.03,
                        a: if is_major { 0.22 } else { 0.10 },
                    },
                }));
            }
        } else {
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y,
                width,
                height,
                color: crate::backend::Color {
                    r: 0.38,
                    g: 0.68,
                    b: 0.92,
                    a: 0.15,
                },
            }));
            let border_color = crate::backend::Color {
                r: 0.38,
                g: 0.68,
                b: 0.92,
                a: 0.78,
            };
            let thickness = 0.07;
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y,
                width,
                height: thickness,
                color: border_color,
            }));
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y: y + height - thickness,
                width,
                height: thickness,
                color: border_color,
            }));
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y,
                width: thickness,
                height,
                color: border_color,
            }));
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: x + width - thickness,
                y,
                width: thickness,
                height,
                color: border_color,
            }));
        }
    }

    let mut item_rects = Vec::new();
    for item in &view.items {
        let Some((x, y, width, height)) = view.metal_item_rect(item) else {
            continue;
        };
        let item_color = item.color.unwrap_or(view.item_color);
        // Ableton clip anatomy (docs/arrangement-region-editing-spec.md 3.1):
        // title bar and body share the clip's one color and are read apart by
        // a 1px hairline, not by a shade change. Without a title bar the item
        // is one flat fill, as before.
        let title_bar_height = view.item_title_bar_bottom(&Rect {
            row: y,
            col: x,
            width,
            height,
        });
        let title_bar_height = title_bar_height.map(|bottom| bottom - y);
        // Selection is a fill change, not a border: the body lights up while
        // the title bar keeps the clip color (Ableton's selected clip).
        let selected = view.item_selected(item);
        let body_color = if selected && title_bar_height.is_some() {
            SELECTED_ITEM_BODY_COLOR
        } else {
            item_color
        };
        // The radius prop is in cells, so it scales with the UI zoom exactly
        // like every other timeline dimension.
        let radius_cells = view.item_corner_radius;
        let radius_px = radius_cells * viewport.cell_h;
        // Fills are built from the item's TRUE span and then clipped to the
        // visible content, not from the view-clamped rect: an item scrolled
        // partly off-screen must be CUT at the viewport edge, not redrawn as
        // a shorter clip that rounds its corners there.
        let (fill_rect, clip) = view.item_fill_rect(item, y, height);
        // Square edge quads would poke out of a rounded clip's corners, so a
        // rounded item is outlined by drawing the border colour at the item's
        // own bounds and insetting the fill inside it — never by inflating,
        // which would push the clip into the neighbouring lane. Square items
        // keep the four edge quads (below).
        let mut fill_rect = fill_rect;
        if radius_px > 0.0 {
            let thickness = item_border_thickness(
                width,
                height,
                selected_border(selected, title_bar_height),
            );
            if thickness > 0.0 {
                push_item_fill(
                    &mut primitives,
                    fill_rect,
                    if selected_border(selected, title_bar_height) {
                        SELECTED_ITEM_BORDER_COLOR
                    } else {
                        ITEM_BORDER_COLOR
                    },
                    viewport,
                    radius_px,
                    clip,
                );
                fill_rect = Rect {
                    row: fill_rect.row + thickness,
                    col: fill_rect.col + thickness,
                    width: (fill_rect.width - thickness * 2.0).max(0.0),
                    height: (fill_rect.height - thickness * 2.0).max(0.0),
                };
            }
        }
        push_item_fill(
            &mut primitives,
            fill_rect,
            body_color,
            viewport,
            radius_px,
            clip,
        );
        // Region highlight over a clip: relight the fill across the part of
        // the clip the region covers, using the same rounded geometry (so the
        // clip's corners still round) merely clipped to the region's x-range.
        // The title bar redraws on top right below, which is what keeps the
        // bar in the clip's own colour while the BODY lights up.
        if let Some((region_x0, region_x1)) = region_span {
            let region_clip = (clip.0.max(region_x0), clip.1.min(region_x1));
            if region_clip.1 > region_clip.0 {
                push_item_fill(
                    &mut primitives,
                    fill_rect,
                    SELECTED_ITEM_BODY_COLOR,
                    viewport,
                    radius_px,
                    region_clip,
                );
            }
        }
        if let Some(bar_height) = title_bar_height {
            // The bar rounds only its top corners: the arc rows are laid out
            // over a rect that runs one radius past the bar, then the draw is
            // cut at the bar's bottom, so no bottom arc is ever emitted.
            let bar_rect = Rect {
                height: bar_height - (fill_rect.row - y) + radius_cells,
                ..fill_rect
            };
            let mut bar_primitives = Vec::new();
            push_item_fill(
                &mut bar_primitives,
                bar_rect,
                item_color,
                viewport,
                radius_px,
                clip,
            );
            let bar_bottom = y + bar_height;
            primitives.extend(bar_primitives.into_iter().filter_map(|primitive| {
                let MetalPrimitive::Quad(mut quad) = primitive else {
                    return Some(primitive);
                };
                let bottom = (quad.y + quad.height).min(bar_bottom);
                if bottom <= quad.y {
                    return None;
                }
                quad.height = bottom - quad.y;
                Some(MetalPrimitive::Quad(quad))
            }));
            let hairline = (1.0 / viewport.cell_h).min(height - bar_height);
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y: y + bar_height,
                width,
                height: hairline,
                color: crate::backend::Color {
                    r: 0.02,
                    g: 0.025,
                    b: 0.03,
                    a: 0.72,
                },
            }));
        }
        // Faint grid continuation inside the clip body (DAW convention): the
        // background grid stays legible through items without competing with
        // their content.
        for &(grid_x, is_major) in &grid_lines {
            if grid_x <= x + 0.1 || grid_x >= x + width - 0.1 {
                continue;
            }
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: grid_x - 0.0625,
                y,
                width: 0.125,
                height,
                color: crate::backend::Color {
                    r: 0.02,
                    g: 0.025,
                    b: 0.03,
                    a: if is_major { 0.22 } else { 0.10 },
                },
            }));
        }
        // Labels live in the title bar when there is one; content (notes)
        // draws in the body below it.
        let label_height = title_bar_height.unwrap_or(height);
        if let Some(label) = &item.label {
            if width >= 3.0 && label_height >= 0.85 {
                primitives.push(MetalPrimitive::ProportionalText(
                    MetalProportionalTextPrimitive {
                        row: y + ((label_height - 0.80).max(0.0) * 0.5) - 0.02,
                        col: x + 0.34,
                        align_width: 0.0,
                        h_align: 0.0,
                        text: label.clone(),
                        font_size: 10.5,
                        scale: 1.0,
                        fg: theme::BLACK(),
                        bg: item_color,
                    },
                ));
            }
        }
        let (content_rect, title_bar) = match title_bar_height {
            Some(bar_height) => (
                (x, y + bar_height, width, height - bar_height),
                Some((y, bar_height)),
            ),
            None => ((x, y, width, height), None),
        };
        push_item_content_primitives(
            &mut primitives,
            &view,
            item,
            content_rect,
            title_bar,
            viewport,
        );
        item_rects.push((
            x,
            y,
            width,
            height,
            selected_border(selected, title_bar_height),
            timeline_hover_edge_for(node.widget_id, &item.id),
        ));
    }

    let item_radius = view.item_corner_radius;
    for (x, y, width, height, selected, resize_hovered) in item_rects {
        let thickness = item_border_thickness(width, height, selected);
        if thickness <= 0.0 {
            continue;
        }
        let border_color = if selected {
            SELECTED_ITEM_BORDER_COLOR
        } else {
            ITEM_BORDER_COLOR
        };
        // Rounded items were already outlined under their fill, in the loop
        // above; only the square ones get edge quads here.
        if item_radius <= 0.0 {
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y,
                width,
                height: thickness,
                color: border_color,
            }));
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y: y + height - thickness,
                width,
                height: thickness,
                color: border_color,
            }));
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x,
                y,
                width: thickness,
                height,
                color: border_color,
            }));
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: x + width - thickness,
                y,
                width: thickness,
                height,
                color: border_color,
            }));
        }
        if let Some(hovered_edge) = resize_hovered {
            let hover_width = 0.22_f32.min(width.max(0.0));
            let hover_col = match hovered_edge {
                HoverEdge::Start => x,
                HoverEdge::End => x + width - hover_width,
            };
            push_item_fill(
                &mut primitives,
                Rect {
                    row: y,
                    col: hover_col,
                    width: hover_width,
                    height,
                },
                crate::backend::Color {
                    r: 0.74,
                    g: 0.94,
                    b: 1.0,
                    a: 0.95,
                },
                viewport,
                item_radius * viewport.cell_h,
                (content.col, content.col + content.width),
            );
        }
    }

    // Cursor and playhead draw ABOVE items so position markers stay visible
    // through clip bodies (DAW convention).
    if let Some(cursor_x) = view.metal_cursor_x() {
        let line_width = (1.0 / viewport.cell_w).max(0.08);
        let marker_width = (8.0 / viewport.cell_w).max(line_width * 4.0);
        let marker_height = (5.0 / viewport.cell_h).max(0.28).min(content.height);
        if marker_height > 0.0 {
            primitives.push(MetalPrimitive::Triangle(MetalTrianglePrimitive {
                points: [
                    [cursor_x - marker_width * 0.5, content.row],
                    [cursor_x + marker_width * 0.5, content.row],
                    [cursor_x, content.row + marker_height],
                ],
                color: theme::CURSOR(),
            }));
        }
        let line_y = content.row + marker_height;
        let line_height = (content.row + content.height - line_y).max(0.0);
        if line_height > 0.0 {
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: cursor_x - line_width * 0.5,
                y: line_y,
                width: line_width,
                height: line_height,
                color: theme::CURSOR(),
            }));
        }
    }

    if let Some(playhead_x) = view.metal_playhead_x() {
        if view.header_height > 0.0 {
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: playhead_x - 0.0625,
                y: rect.row,
                width: 0.125,
                height: view.header_height,
                color: theme::YELLOW(),
            }));
        }
        primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x: playhead_x - 0.0625,
            y: content.row,
            width: 0.125,
            height: content.height,
            color: theme::YELLOW(),
        }));
    }

    primitives
}

#[cfg(target_os = "macos")]
const ITEM_BORDER_COLOR: crate::backend::Color = crate::backend::Color {
    r: 0.02,
    g: 0.025,
    b: 0.03,
    a: 0.72,
};

#[cfg(target_os = "macos")]
const SELECTED_ITEM_BORDER_COLOR: crate::backend::Color = crate::backend::Color {
    r: 0.725,
    g: 0.933,
    b: 1.0,
    a: 1.0,
};

/// A title-barred clip shows selection through its lit body, so it keeps the
/// ordinary dark outline; bar-less hosts (piano roll) still get the bright
/// selected border.
#[cfg(target_os = "macos")]
fn selected_border(selected: bool, title_bar_height: Option<f32>) -> bool {
    selected && title_bar_height.is_none()
}

#[cfg(target_os = "macos")]
fn item_border_thickness(width: f32, height: f32, selected: bool) -> f32 {
    if selected { 0.16_f32 } else { 0.08_f32 }
        .min(width * 0.5)
        .min(height * 0.5)
}

/// Corner arc resolution cap: one slab per device pixel of radius, bounded so
/// a huge radius cannot flood the primitive list.
#[cfg(target_os = "macos")]
const ITEM_CORNER_ROWS_MAX: usize = 24;

/// Fill one item rect, rounded when the host asked for a corner radius and a
/// plain quad otherwise (`:item-corner-radius`, default 0 — every pre-existing
/// host keeps square clips).
///
/// The rounding is built from plain quads rather than the rounded-rect
/// shader: widget instances are batched into their own pass that composites
/// either under every quad (burying the clip under its own lane background)
/// or over every quad (burying the clip's notes, grid and hairline). One
/// slab per device-pixel row walks the corner arc, and each row's boundary
/// pixel is drawn at fractional alpha equal to its coverage — the same
/// coverage an `fwidth` mask computes, resolved on the CPU, blending against
/// whatever is already beneath the clip.
#[cfg(target_os = "macos")]
fn push_item_fill(
    primitives: &mut Vec<MetalPrimitive>,
    rect: Rect,
    color: crate::backend::Color,
    viewport: super::WidgetViewport,
    radius_px: f32,
    clip: (f32, f32),
) {
    let cell_w = viewport.cell_w.max(0.0001);
    let cell_h = viewport.cell_h.max(0.0001);
    let radius_px = radius_px
        .min(rect.width * cell_w * 0.5)
        .min(rect.height * cell_h * 0.5);
    if radius_px <= 0.5 {
        push_clipped_quad(primitives, rect, color, clip);
        return;
    }
    let radius_y = radius_px / cell_h;

    push_clipped_quad(
        primitives,
        Rect {
            row: rect.row + radius_y,
            col: rect.col,
            width: rect.width,
            height: rect.height - radius_y * 2.0,
            
        },
        color,
        clip,
    );

    let rows = (radius_px.ceil() as usize).min(ITEM_CORNER_ROWS_MAX);
    let row_height = radius_y / rows as f32;
    for row in 0..rows {
        // Horizontal inset of the arc at this row's center: the corner circle
        // is centred `radius` in from both edges.
        let offset_px = (row as f32 + 0.5) * (radius_px / rows as f32);
        let above_center = radius_px - offset_px;
        let inset_px =
            radius_px - (radius_px * radius_px - above_center * above_center).max(0.0).sqrt();
        // Boundary pixel: partial coverage becomes partial alpha.
        let solid_inset_px = inset_px.ceil();
        let coverage = (solid_inset_px - inset_px).clamp(0.0, 1.0);
        let solid_inset = solid_inset_px / cell_w;
        let edge_pixel = 1.0 / cell_w;
        let solid_width = rect.width - solid_inset * 2.0;
        let offset = radius_y * (row as f32 / rows as f32);
        for top in [
            rect.row + offset,
            rect.row + rect.height - offset - row_height,
        ] {
            if solid_width > 0.0 {
                push_clipped_quad(
                    primitives,
                    Rect {
                        row: top,
                        col: rect.col + solid_inset,
                        width: solid_width,
                        height: row_height,
                        
                    },
                    color,
                    clip,
                );
            }
            if coverage <= 0.004 {
                continue;
            }
            let feathered = crate::backend::Color {
                a: color.a * coverage,
                ..color
            };
            for col in [
                rect.col + solid_inset - edge_pixel,
                rect.col + rect.width - solid_inset,
            ] {
                push_clipped_quad(
                    primitives,
                    Rect {
                        row: top,
                        col,
                        width: edge_pixel,
                        height: row_height,
                        
                    },
                    feathered,
                    clip,
                );
            }
        }
    }
}

/// Push one quad clipped to a horizontal `[min, max]` window, dropping it when
/// nothing of it is visible. Horizontal only: lanes already clip vertically.
#[cfg(target_os = "macos")]
fn push_clipped_quad(
    primitives: &mut Vec<MetalPrimitive>,
    rect: Rect,
    color: crate::backend::Color,
    clip: (f32, f32),
) {
    let left = rect.col.max(clip.0);
    let right = (rect.col + rect.width).min(clip.1);
    if right <= left || rect.height <= 0.0 {
        return;
    }
    primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: left,
        y: rect.row,
        width: right - left,
        height: rect.height,
        color,
    }));
}

/// Body fill of a selected clip that has a title bar
/// (docs/arrangement-region-editing-spec.md 3.1): a fixed warm light tint,
/// the same for every clip color, so selection reads at a glance the way
/// Ableton's does. The bar keeps the clip's own color.
#[cfg(target_os = "macos")]
const SELECTED_ITEM_BODY_COLOR: crate::backend::Color = crate::backend::Color {
    r: 0.94,
    g: 0.87,
    b: 0.68,
    a: 1.0,
};

/// Minimum on-screen item width (px) below which item content is skipped
/// entirely — narrower than this the dots/bars would only alias
/// (docs/arrangement-timeline-ui-spec.md 7.3).
#[cfg(target_os = "macos")]
const ITEM_CONTENT_MIN_WIDTH_PX: f32 = 14.0;

/// Draw an item's `content` payload as additional quads clipped to the item's
/// on-screen rect (docs/arrangement-timeline-ui-spec.md 7.3). `rect` is the
/// already view-clipped rect from `metal_item_rect`; dot x positions come
/// from the item's unclipped time span so partially visible items keep their
/// content aligned. `title_bar` is the `(y, height)` of the clip's title bar
/// when it has one: repeat boundaries then read as short ticks hanging off
/// the top of the bar (Ableton's loop marker) instead of full-height rules
/// through the notes.
#[cfg(target_os = "macos")]
fn push_item_content_primitives(
    primitives: &mut Vec<MetalPrimitive>,
    view: &TimelineView,
    item: &TimelineItem,
    rect: (f32, f32, f32, f32),
    title_bar: Option<(f32, f32)>,
    viewport: super::WidgetViewport,
) {
    let (x, y, width, height) = rect;
    if width * viewport.cell_w < ITEM_CONTENT_MIN_WIDTH_PX {
        return;
    }
    match &item.content {
        Some(TimelineItemContent::Dots { dots, cycle }) => {
            let span = item.end - item.start;
            if span <= 0.0 {
                return;
            }
            let cycle = cycle.clamp(f64::EPSILON, 1.0);
            let cycles = (1.0 / cycle).ceil().min(512.0) as usize;
            let cycle_width_px = (width * viewport.cell_w) as f64 * cycle;
            let view_end = view.view_start + view.view_duration;

            // Cycle separators: one line per repetition boundary, so a
            // looping clip reads as its repeats (skipped when the cycles are
            // too narrow to resolve).
            if cycles > 1 && cycle_width_px >= 5.0 {
                let separator_color = crate::backend::Color {
                    r: 0.02,
                    g: 0.025,
                    b: 0.03,
                    a: 0.42,
                };
                // A tick off the top of the title bar when there is one, a
                // full-height rule otherwise (piano roll / bar-less hosts).
                let (separator_y, separator_height) = match title_bar {
                    Some((bar_y, bar_height)) => (bar_y, (bar_height * 0.55).max(0.12)),
                    None => (y, height),
                };
                for index in 1..cycles {
                    let time = item.start + span * cycle * index as f64;
                    if time < view.view_start || time >= view_end || time >= item.end {
                        continue;
                    }
                    let line_x = view.x_for_time(time);
                    primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                        x: line_x - 0.0625,
                        y: separator_y,
                        width: 0.125,
                        height: separator_height,
                        color: separator_color,
                    }));
                }
            }

            if cycle_width_px < 8.0 {
                return;
            }
            let dot_width = (3.0 / viewport.cell_w).min(width);
            let dot_height = (3.0 / viewport.cell_h).min(height);
            let inner_height = (height - dot_height).max(0.0);
            let dot_color = crate::backend::Color {
                r: 0.02,
                g: 0.025,
                b: 0.03,
                a: 0.78,
            };
            for index in 0..cycles {
                for dot in dots {
                    let offset = (index as f64 + dot.offset) * cycle;
                    if offset >= 1.0 {
                        break;
                    }
                    let time = item.start + offset * span;
                    if time < view.view_start || time >= view_end {
                        continue;
                    }
                    let dot_x = view
                        .x_for_time(time)
                        .clamp(x, (x + width - dot_width).max(x));
                    let dot_y = y + (1.0 - dot.value) as f32 * inner_height;
                    // Real note length (spec 3.2): a bar spanning the note's
                    // duration, never narrower than the legacy 3px dot and
                    // never painted past the item's end.
                    let quad_width = if dot.width > 0.0 {
                        let end_offset = ((index as f64 + dot.offset + dot.width) * cycle).min(1.0);
                        let end_x = view.x_for_time(item.start + end_offset * span);
                        (end_x - dot_x)
                            .max(dot_width)
                            .min((x + width - dot_x).max(dot_width))
                    } else {
                        dot_width
                    };
                    primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                        x: dot_x,
                        y: dot_y,
                        width: quad_width,
                        height: dot_height,
                        color: dot_color,
                    }));
                }
            }
        }
        // Peaks rendering lands with the audio-track asset pipeline
        // (docs/arrangement-timeline-ui-spec.md 3, Slice D).
        Some(TimelineItemContent::Peaks(_)) | None => {}
    }
}

impl TimelineView {
    fn time_viewport(&self) -> TimeViewport {
        TimeViewport {
            rect: self.rect,
            header_height: self.header_height,
            sidebar_width: self.sidebar_width,
            view_start: self.view_start,
            view_duration: self.view_duration,
            zoom_min_duration: self.zoom_min_duration,
            zoom_max_duration: self.zoom_max_duration,
            grid_density: self.grid_density,
        }
    }

    fn from_props(props: &HashMap<String, Value>, rect: Rect) -> Self {
        let view_duration = get_num(props, "view-duration", 16.0).max(0.0001);
        let view_start = get_num(props, "view-start", 0.0).max(0.0);
        let mut view = Self {
            rect,
            header_height: get_num(props, "header-height", 1.0).max(0.0) as f32,
            sidebar_width: get_num(props, "sidebar-width", 0.0).max(0.0) as f32,
            view_start,
            view_duration,
            zoom_min_duration: get_num(props, "zoom-min-duration", 8.0).max(0.0001),
            zoom_max_duration: get_num(props, "zoom-max-duration", 128.0).max(0.0001),
            grid_density: get_num(props, "grid-density", 1.0).clamp(1.0, 8.0),
            content_length: props
                .get("content-length")
                .and_then(as_number)
                .map(|length| length.max(0.0)),
            content_length_min: get_num(props, "content-length-min", 1.0).max(0.0),
            content_length_max: get_num(props, "content-length-max", 256.0).max(1.0),
            time_ruler: props
                .get("time-ruler")
                .and_then(get_map)
                .and_then(|map| get_time_ruler(&map)),
            playhead_time: props.get("playhead-time").and_then(as_number),
            cursor_time: props.get("cursor-time").and_then(as_number),
            item_color: resolve_named_color(props, "item-color", theme::BLUE()),
            loop_color: resolve_named_color(props, "loop-color", theme::BLUE()),
            sidebar_style: get_sidebar_style(props),
            lane_scroll: get_num(props, "lane-scroll", 0.0).max(0.0),
            lane_height: props
                .get("lane-height")
                .and_then(as_number)
                .map(|height| height.max(0.2) as f32),
            item_corner_radius: get_num(props, "item-corner-radius", 0.0).max(0.0) as f32,
            title_bar_height: get_num(props, "title-bar-height", 0.0).max(0.0) as f32,
            scroll_viewport_height: props
                .get("scroll-viewport-height")
                .and_then(as_number)
                .map(|height| height.max(0.0) as f32),
            snap: get_num(props, "snap", 0.0).max(0.0),
            resize_snap: get_num(props, "resize-snap", get_num(props, "snap", 0.0)).max(0.0),
            resize_snap_to_grid: matches!(
                props.get("resize-snap"),
                Some(Value::Keyword(mode)) | Some(Value::String(mode)) if mode == "grid"
            ),
            marquee_snap_to_grid: matches!(
                props.get("marquee-snap"),
                Some(Value::Keyword(mode)) | Some(Value::String(mode)) if mode == "grid"
            ),
            snap_floor: matches!(
                props.get("snap-mode"),
                Some(Value::Keyword(mode)) | Some(Value::String(mode)) if mode == "floor"
            ),
            resize_snap_floor: props.get("resize-snap-mode").map_or_else(
                || {
                    matches!(
                        props.get("snap-mode"),
                        Some(Value::Keyword(mode)) | Some(Value::String(mode)) if mode == "floor"
                    )
                },
                |mode| matches!(mode, Value::Keyword(mode) | Value::String(mode) if mode == "floor"),
            ),
            min_duration: props
                .get("min-duration")
                .and_then(as_number)
                .map(|duration| duration.max(0.0001)),
            create_duration: props
                .get("create-duration")
                .and_then(as_number)
                .map(|duration| duration.max(0.0001)),
            move_alignment_helper: matches!(
                props.get("move-snap-mode"),
                Some(Value::Keyword(mode)) | Some(Value::String(mode)) if mode == "alignment-helper"
            ),
            resize_alignment_helper: matches!(
                props.get("resize-snap-mode"),
                Some(Value::Keyword(mode)) | Some(Value::String(mode)) if mode == "alignment-helper"
            ),
            smooth_scroll: matches!(
                props.get("scroll-mode"),
                Some(Value::Keyword(mode)) | Some(Value::String(mode)) if mode == "smooth"
            ),
            vertical_scroll_passthrough: vertical_scroll_passthrough(props),
            tool: get_tool(props),
            lanes: get_lanes(props),
            items: get_items(props),
            selection: get_selection(props),
            selection_rect: get_selection_rect(props),
            selection_rect_style: match props.get("selection-rect-style") {
                Some(Value::Keyword(style)) | Some(Value::String(style)) if style == "region" => {
                    SelectionRectStyle::Region
                }
                _ => SelectionRectStyle::Marquee,
            },
        };
        view.lane_scroll = view.clamp_lane_scroll(view.lane_scroll);
        view
    }

    fn content_rect(&self) -> Rect {
        self.time_viewport().content_rect()
    }

    fn unavailable_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let content_length = self.content_length?;
        if content_length >= self.view_start + self.view_duration {
            return None;
        }
        let content = self.content_rect();
        if content.width <= 0.0 || self.rect.width <= 0.0 || self.rect.height <= 0.0 {
            return None;
        }
        let start_time = content_length.max(self.view_start);
        let x = self.x_for_time(start_time).max(content.col);
        let width = (self.rect.col + self.rect.width - x).max(0.0);
        if width <= 0.0 {
            return None;
        }
        Some((x, self.rect.row, width, self.rect.height))
    }

    fn loop_band_rect(&self) -> Option<(f32, f32)> {
        let content_length = self.content_length?;
        if content_length <= 0.0 {
            return None;
        }
        let view_end = self.view_start + self.view_duration;
        if view_end <= 0.0 || self.view_start >= content_length {
            return None;
        }
        let content = self.content_rect();
        let start = self
            .x_for_time(0.0_f64.max(self.view_start))
            .max(content.col);
        let end = self
            .x_for_time(content_length.min(view_end))
            .min(content.col + content.width);
        if end <= start {
            return None;
        }
        Some((start, end - start))
    }

    fn minimum_duration(&self) -> f64 {
        self.min_duration
            .unwrap_or_else(|| self.effective_resize_snap())
            .max(0.0001)
    }

    fn default_create_duration(&self) -> f64 {
        self.create_duration
            .unwrap_or_else(|| self.minimum_duration())
            .max(self.minimum_duration())
    }

    fn minimum_end_for(&self, item: &TimelineItem) -> f64 {
        item.start + self.minimum_duration()
    }

    fn bounded_span(&self, start_time: f64, current_time: f64) -> (f64, f64) {
        let min_duration = self.minimum_duration();
        if current_time >= start_time {
            (start_time, current_time.max(start_time + min_duration))
        } else {
            (current_time, start_time.max(current_time + min_duration))
        }
    }

    fn lane_height(&self) -> f32 {
        if let Some(lane_height) = self.lane_height {
            return lane_height.max(0.2);
        }
        let content = self.content_rect();
        let first_visible_lane = self.lane_scroll.floor().max(0.0) as usize;
        let lane_count = self.lanes.len().saturating_sub(first_visible_lane).max(1);
        let visible_lanes = (content.height.ceil().max(1.0) as usize)
            .min(lane_count)
            .max(1);
        (content.height / visible_lanes as f32).max(0.2)
    }

    fn max_lane_scroll(&self) -> f64 {
        let content = self.content_rect();
        let scroll_content_height = self
            .scroll_viewport_height
            .map(|height| (height - self.header_height).max(0.0))
            .unwrap_or(content.height);
        if scroll_content_height <= 0.0 || self.lanes.is_empty() {
            return 0.0;
        }
        let lane_height = self.lane_height().max(0.2) as f64;
        let visible_lanes = (scroll_content_height as f64 / lane_height).max(1.0);
        (self.lanes.len() as f64 - visible_lanes).max(0.0)
    }

    fn clamp_lane_scroll(&self, lane_scroll: f64) -> f64 {
        lane_scroll.max(0.0).min(self.max_lane_scroll())
    }

    fn anchor_lane_at_row(&self, local_row: f32) -> f64 {
        let content = self.content_rect();
        if content.height == 0.0 {
            return self.lane_scroll;
        }
        let relative = ((local_row - content.row) / self.lane_height()).max(0.0) as f64;
        self.lane_scroll + relative
    }

    fn visible_lane_rows(&self, lane_index: usize) -> Option<(u16, u16)> {
        let content = self.content_rect();
        if content.height == 0.0 {
            return None;
        }
        let lane_height = self.lane_height();
        let row_start = content.row + (lane_index as f32 - self.lane_scroll as f32) * lane_height;
        let row_end = row_start + lane_height;
        let clip_top = content.row;
        let clip_bottom = content.row + content.height;
        let row_start = row_start.max(clip_top);
        let row_end = row_end.min(clip_bottom);
        if row_end <= row_start {
            return None;
        }
        Some((
            row_start.round() as u16,
            row_end.max(row_start + 1.0).round() as u16,
        ))
    }

    fn lane_at_row(&self, local_row: f32) -> usize {
        let content = self.content_rect();
        if content.height == 0.0 {
            return 0;
        }
        let relative = ((local_row - content.row) / self.lane_height()).max(0.0) as f64;
        ((self.lane_scroll + relative).floor().max(0.0) as usize)
            .min(self.lanes.len().saturating_sub(1))
    }

    fn time_at_col(&self, local_col: f32) -> f64 {
        self.time_viewport().time_at_col(local_col)
    }

    fn snap_value(&self, time: f64, snap: f64, floor: bool) -> f64 {
        if snap <= 0.0 {
            time
        } else if floor {
            (time / snap).floor() * snap
        } else {
            (time / snap).round() * snap
        }
    }

    fn snap_time(&self, time: f64) -> f64 {
        self.snap_value(time, self.snap, self.snap_floor)
    }

    fn snap_resize_time(&self, time: f64) -> f64 {
        self.snap_value(time, self.effective_resize_snap(), self.resize_snap_floor)
    }

    fn snap_edit_time(&self, time: f64) -> f64 {
        self.snap_value(time, self.effective_resize_snap(), true)
    }

    fn alignment_helper_grid_step(&self) -> f64 {
        self.time_viewport()
            .grid_step(self.time_ruler.as_ref())
            .max(0.0001)
    }

    fn alignment_helper_snapped_time(
        &self,
        raw_time: f64,
        anchor_cell_start: f64,
        previous_cell_snap_start: f64,
        anchor_has_delay: bool,
        grid: f64,
    ) -> f64 {
        if anchor_has_delay && raw_time < anchor_cell_start && raw_time > previous_cell_snap_start {
            anchor_cell_start
        } else {
            (raw_time / grid).floor() * grid
        }
    }

    fn alignment_helper_time_for_drag(
        &self,
        raw_time: f64,
        anchor_time: f64,
        gesture_value: &Value,
    ) -> f64 {
        let grid = self.alignment_helper_grid_step();
        let anchor_cell_start = (anchor_time / grid).floor() * grid;
        let anchor_cell_end = anchor_cell_start + grid;
        let previous_cell_snap_start =
            anchor_cell_start - grid * (1.0 - ALIGNMENT_HELPER_BACKWARD_SNAP_PROXIMITY);
        let anchor_has_delay = anchor_time > anchor_cell_start + f64::EPSILON;
        let has_snapped = map_bool(gesture_value, "alignment-helper-snapped");

        if has_snapped {
            return self.alignment_helper_snapped_time(
                raw_time,
                anchor_cell_start,
                previous_cell_snap_start,
                anchor_has_delay,
                grid,
            );
        }

        if raw_time >= anchor_cell_end
            || (raw_time < anchor_cell_start && anchor_has_delay)
            || raw_time <= previous_cell_snap_start
        {
            set_map_bool(gesture_value, "alignment-helper-snapped", true);
            self.alignment_helper_snapped_time(
                raw_time,
                anchor_cell_start,
                previous_cell_snap_start,
                anchor_has_delay,
                grid,
            )
        } else {
            raw_time
        }
    }

    fn move_start_for_drag(
        &self,
        raw_time: f64,
        snapped_time: f64,
        gesture_value: &Value,
        gesture: &HashMap<String, Value>,
        anchor_start: f64,
    ) -> Option<f64> {
        if !self.move_alignment_helper {
            let time_offset = as_number(gesture.get("time-offset")?)?;
            return Some(self.snap_time(snapped_time - time_offset));
        }

        let raw_time_offset = as_number(
            gesture
                .get("raw-time-offset")
                .or_else(|| gesture.get("time-offset"))?,
        )?;
        let raw_start = raw_time - raw_time_offset;
        Some(self.alignment_helper_time_for_drag(raw_start, anchor_start, gesture_value))
    }

    fn resize_end_for_drag(
        &self,
        raw_time: f64,
        snapped_resize_time: f64,
        gesture_value: &Value,
        gesture: &HashMap<String, Value>,
        anchor_end: f64,
    ) -> Option<f64> {
        if !self.resize_alignment_helper {
            return Some(snapped_resize_time);
        }

        let raw_time_offset = as_number(gesture.get("raw-time-offset")?)?;
        let raw_end = raw_time - raw_time_offset;
        Some(self.alignment_helper_time_for_drag(raw_end, anchor_end, gesture_value))
    }

    /// Start-edge mirror of `resize_end_for_drag`
    /// (docs/arrangement-region-editing-spec.md 3.1): same snap ladder, same
    /// alignment-helper behavior, anchored on the item's start.
    fn resize_start_for_drag(
        &self,
        raw_time: f64,
        snapped_resize_time: f64,
        gesture_value: &Value,
        gesture: &HashMap<String, Value>,
        anchor_start: f64,
    ) -> Option<f64> {
        if !self.resize_alignment_helper {
            return Some(snapped_resize_time);
        }

        let raw_time_offset = as_number(gesture.get("raw-time-offset")?)?;
        let raw_start = raw_time - raw_time_offset;
        Some(self.alignment_helper_time_for_drag(raw_start, anchor_start, gesture_value))
    }

    /// Ordered marquee span, grid-quantized under `:marquee-snap :grid`
    /// (docs/arrangement-region-editing-spec.md 4.3): the low edge floors and
    /// the high edge ceils onto the zoom-adaptive ladder, so the selection is
    /// always whole grid cells and "grab exactly 4 bars" is a sloppy drag.
    /// A span that quantizes to nothing (a vertical-only drag landing on a
    /// grid line) widens to the one cell the pointer is in rather than
    /// selecting zero time.
    fn marquee_span(&self, time_a: f64, time_b: f64) -> (f64, f64) {
        let low = time_a.min(time_b);
        let high = time_a.max(time_b);
        if !self.marquee_snap_to_grid {
            return (low, high);
        }
        let grid = self.alignment_helper_grid_step();
        let start = (low / grid).floor() * grid;
        let end = (high / grid).ceil() * grid;
        if end <= start + f64::EPSILON {
            (start, start + grid)
        } else {
            (start, end)
        }
    }

    fn effective_resize_snap(&self) -> f64 {
        if self.resize_snap_to_grid {
            self.time_viewport().grid_step(self.time_ruler.as_ref())
        } else {
            self.resize_snap
        }
    }

    fn playhead_col(&self) -> Option<u16> {
        self.time_viewport().playhead_col(self.playhead_time)
    }

    fn grid_columns(&self) -> Vec<(u16, bool)> {
        self.time_viewport().grid_columns(self.time_ruler.as_ref())
    }

    fn time_ruler_labels(&self) -> Vec<(u16, String)> {
        self.time_viewport()
            .time_ruler_labels(self.time_ruler.as_ref())
    }

    fn metal_grid_lines(&self) -> Vec<(f32, bool)> {
        self.time_viewport()
            .metal_grid_lines(self.time_ruler.as_ref())
    }

    fn metal_time_ruler_labels(&self) -> Vec<(f32, String)> {
        self.time_viewport()
            .metal_time_ruler_labels(self.time_ruler.as_ref())
    }

    fn edge_for_time(&self, time: f64) -> f32 {
        self.time_viewport().edge_for_time(time)
    }

    fn x_for_time(&self, time: f64) -> f32 {
        self.time_viewport().x_for_time(time)
    }

    fn metal_playhead_x(&self) -> Option<f32> {
        self.time_viewport().metal_playhead_x(self.playhead_time)
    }

    fn cursor_col(&self) -> Option<u16> {
        self.time_viewport().playhead_col(self.cursor_time)
    }

    fn metal_cursor_x(&self) -> Option<f32> {
        self.time_viewport().metal_playhead_x(self.cursor_time)
    }

    fn cursor_snap_time(&self, time: f64) -> f64 {
        let grid = self.alignment_helper_grid_step();
        (time / grid).round() * grid
    }

    fn metal_lane_rect(&self, lane_index: usize) -> Option<(f32, f32)> {
        let content = self.content_rect();
        if content.height == 0.0 {
            return None;
        }
        let lane_height = self.lane_height();
        let top = content.row + (lane_index as f32 - self.lane_scroll as f32) * lane_height;
        let bottom = top + lane_height;
        let clip_top = content.row;
        let clip_bottom = content.row + content.height;
        let visible_top = top.max(clip_top);
        let visible_bottom = bottom.min(clip_bottom);
        if visible_bottom <= visible_top {
            return None;
        }
        Some((visible_top, (visible_bottom - visible_top).max(0.25)))
    }

    fn metal_item_rect(&self, item: &TimelineItem) -> Option<(f32, f32, f32, f32)> {
        let (y, height) = self.metal_lane_rect(item.lane)?;
        let view_end = self.view_start + self.view_duration;
        if item.end <= self.view_start || item.start >= view_end {
            return None;
        }
        let start = item.start.max(self.view_start);
        let end = item.end.min(view_end);
        let x = self.x_for_time(start);
        let x_end = self.x_for_time(end);
        Some((x, y, (x_end - x).max(0.25), height))
    }

    /// An item's fill geometry: the rect its TRUE `[start, end)` span would
    /// occupy (which may run past either side of the view) plus the
    /// horizontal window that geometry must be clipped to. Rounded corners
    /// then only appear at the item's real edges — an item scrolled partly
    /// off-screen is cut square at the viewport edge instead of looking like
    /// a shorter clip that begins there.
    #[cfg(target_os = "macos")]
    fn item_fill_rect(&self, item: &TimelineItem, row: f32, height: f32) -> (Rect, (f32, f32)) {
        let content = self.content_rect();
        let left = self.unclamped_x_for_time(item.start);
        let right = self.unclamped_x_for_time(item.end);
        (
            Rect {
                row,
                col: left,
                width: (right - left).max(0.25),
                height,
            },
            (content.col, content.col + content.width),
        )
    }

    /// `x_for_time` clamps to the visible window; this doesn't, so off-screen
    /// item edges keep their true position (bounded to one viewport width
    /// either side, which is off-screen enough and keeps the math finite).
    #[cfg(target_os = "macos")]
    fn unclamped_x_for_time(&self, time: f64) -> f32 {
        let content = self.content_rect();
        if content.width == 0.0 {
            return content.col;
        }
        let position = content.col as f64
            + content.width as f64 * ((time - self.view_start) / self.view_duration.max(0.0001));
        (position as f32).clamp(content.col - content.width, content.col + content.width * 2.0)
    }

    fn item_selected(&self, item: &TimelineItem) -> bool {
        item.selected || self.selection.iter().any(|id| id == &item.id)
    }

    fn metal_selection_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let selection = self.selection_rect.as_ref()?;
        let content = self.content_rect();
        let time_a = selection.time_a.min(selection.time_b);
        let time_b = selection.time_a.max(selection.time_b);
        let view_end = self.view_start + self.view_duration;
        if time_b <= self.view_start || time_a >= view_end {
            return None;
        }

        let x0 = self
            .x_for_time(time_a.max(self.view_start))
            .max(content.col);
        let x1 = self
            .x_for_time(time_b.min(view_end))
            .min(content.col + content.width);
        if x1 <= x0 {
            return None;
        }

        let lane_a = selection.lane_a.min(selection.lane_b);
        let lane_b = selection.lane_a.max(selection.lane_b);
        let top = content.row + (lane_a as f32 - self.lane_scroll as f32) * self.lane_height();
        let bottom =
            content.row + ((lane_b + 1) as f32 - self.lane_scroll as f32) * self.lane_height();
        let y0 = top.max(content.row);
        let y1 = bottom.min(content.row + content.height);
        if y1 <= y0 {
            return None;
        }
        Some((x0, y0, x1 - x0, y1 - y0))
    }

    fn item_rect(&self, item: &TimelineItem) -> Option<Rect> {
        let content = self.content_rect();
        let (row_start, row_height) = self.metal_lane_rect(item.lane)?;
        let view_end = self.view_start + self.view_duration;
        if item.end <= self.view_start || item.start >= view_end {
            return None;
        }
        let start_edge = self.edge_for_time(item.start);
        let end_edge = self.edge_for_time(item.end);
        let start_col = content.col + start_edge.min((content.width - 1.0).max(0.0));
        let width = (end_edge - start_edge).max(1.0);
        Some(Rect {
            row: row_start,
            col: start_col.min(content.col + (content.width - 1.0).max(0.0)),
            width: width.min(content.width),
            height: row_height.max(0.25),
        })
    }

    /// Whether items draw a title bar at all
    /// (docs/arrangement-region-editing-spec.md 3.1).
    fn has_title_bar(&self) -> bool {
        self.title_bar_height > 0.0
    }

    /// Bottom row of an item's title bar, or `None` when the bar is off. The
    /// bar never eats the whole item: a body row always remains so the clip
    /// keeps a selection surface even in a short lane.
    fn item_title_bar_bottom(&self, rect: &Rect) -> Option<f32> {
        if !self.has_title_bar() {
            return None;
        }
        let height = self.title_bar_height.min(rect.height * 0.5);
        (height > 0.0).then(|| rect.row + height)
    }

    fn hit_test(&self, local_col: f32, local_row: f32) -> Option<HitRegion> {
        if local_col < self.rect.col || local_row < self.rect.row {
            return None;
        }
        if local_col < self.rect.col + self.sidebar_width {
            let lane = self.lane_at_row(local_row);
            return Some(HitRegion::Sidebar { lane });
        }
        if local_row < self.rect.row + self.header_height {
            if self.content_length.is_some() {
                let content = self.content_rect();
                let content_end = self.x_for_time(self.content_length?);
                let edge_slop = 0.75;
                if content_end >= content.col
                    && content_end <= content.col + content.width
                    && local_col >= content_end - edge_slop
                    && local_col <= content_end + edge_slop
                {
                    return Some(HitRegion::ContentLengthEnd);
                }
            }
            return Some(HitRegion::Header);
        }
        // A handle hit INSIDE an item always beats a slop hit from a
        // neighbour that merely ends/starts nearby: back-to-back clips share
        // a boundary, and without this the two slop zones overlap and the
        // winner is decided by draw order — grabbing a clip's end edge would
        // flip to resizing the NEXT clip's start a pixel later. Slop hits are
        // therefore collected as a fallback and only used when no item
        // contains the pointer at all.
        let mut slop_hit: Option<HitRegion> = None;
        for item in self.items.iter().rev() {
            let Some(rect) = self.item_rect(item) else {
                continue;
            };
            if local_row < rect.row || local_row >= rect.row + rect.height {
                continue;
            }

            let left = rect.col;
            let right = rect.col + rect.width;
            // Narrow, roughly fixed-width grips (Ableton): a fat handle over
            // a long clip swallows most of its title bar, and on abutting
            // clips it makes the boundary a coin flip.
            let handle_width = (rect.width * 0.24).clamp(0.5, 1.25);
            let outside_slop = 0.75;

            // With a title bar the drag handles live on the bar only, so the
            // body stays a pure selection surface
            // (docs/arrangement-region-editing-spec.md 3.1). Without one
            // (piano roll) the end handle spans the item's full height, as
            // it always has.
            let title_bar_bottom = self.item_title_bar_bottom(&rect);
            let in_title_bar = match title_bar_bottom {
                Some(bottom) => local_row < bottom,
                None => true,
            };
            let has_handles = in_title_bar && rect.width > 1.0;

            if local_col >= left && local_col < right {
                if has_handles && local_col >= right - handle_width {
                    return Some(HitRegion::ItemEdgeEnd { item: item.clone() });
                }
                if has_handles && title_bar_bottom.is_some() && local_col <= left + handle_width {
                    return Some(HitRegion::ItemEdgeStart { item: item.clone() });
                }
                if in_title_bar && title_bar_bottom.is_some() {
                    return Some(HitRegion::ItemTitleBar { item: item.clone() });
                }
                return Some(HitRegion::ItemBody { item: item.clone() });
            }

            if has_handles && slop_hit.is_none() {
                if local_col >= right && local_col <= right + outside_slop {
                    slop_hit = Some(HitRegion::ItemEdgeEnd { item: item.clone() });
                } else if title_bar_bottom.is_some()
                    && local_col <= left
                    && local_col >= left - outside_slop
                {
                    slop_hit = Some(HitRegion::ItemEdgeStart { item: item.clone() });
                }
            }
        }
        slop_hit.or(Some(HitRegion::Background {
            time: self.time_at_col(local_col),
        }))
    }

    fn begin_gesture(&self, local_col: f32, local_row: f32) -> Option<Value> {
        let hit = self.hit_test(local_col, local_row)?;
        let current_time = self.snap_time(self.time_at_col(local_col));
        let current_marquee_time = self.time_at_col(local_col);
        let current_lane = self.lane_at_row(local_row);
        match self.tool {
            TimelineTool::Pointer => match hit {
                // With a title bar the body is a region-selection surface,
                // not a move surface (docs/arrangement-region-editing-spec.md
                // 3.1); moves start from the bar instead.
                HitRegion::ItemBody { .. } if self.has_title_bar() => Some(map_value(vec![
                    ("kind", keyword(":marquee")),
                    ("time", Value::Number(current_marquee_time)),
                    ("lane", Value::Number(current_lane as f64)),
                    ("row", Value::Number(local_row as f64)),
                    // A body marquee starts on a clip the pointer-down
                    // already selected; a zero-movement release must not
                    // then clear that selection the way a background click
                    // does.
                    ("origin", keyword(":item-body")),
                ])),
                HitRegion::ItemBody { item } | HitRegion::ItemTitleBar { item } => {
                    let ids = if self.item_selected(&item) {
                        self.selected_ids_for(item.id.clone())
                    } else {
                        vec![item.id.clone()]
                    };
                    let raw_time = self.time_at_col(local_col);
                    Some(map_value(vec![
                        ("kind", keyword(":move")),
                        ("ids", list_value(ids)),
                        ("anchor-id", item.id),
                        ("anchor-start", Value::Number(item.start)),
                        ("time-offset", Value::Number(current_time - item.start)),
                        ("raw-time-offset", Value::Number(raw_time - item.start)),
                        ("alignment-helper-snapped", Value::Bool(false)),
                        (
                            "lane-offset",
                            Value::Number(current_lane as f64 - item.lane as f64),
                        ),
                    ]))
                }
                HitRegion::ItemEdgeEnd { item } => {
                    let ids = if self.item_selected(&item) {
                        self.selected_ids_for(item.id.clone())
                    } else {
                        vec![item.id.clone()]
                    };
                    let raw_time = self.time_at_col(local_col);
                    Some(map_value(vec![
                        ("kind", keyword(":resize-end")),
                        ("id", item.id),
                        ("ids", list_value(ids)),
                        ("anchor-end", Value::Number(item.end)),
                        ("raw-time-offset", Value::Number(raw_time - item.end)),
                        ("alignment-helper-snapped", Value::Bool(false)),
                    ]))
                }
                HitRegion::ItemEdgeStart { item } => {
                    let ids = if self.item_selected(&item) {
                        self.selected_ids_for(item.id.clone())
                    } else {
                        vec![item.id.clone()]
                    };
                    let raw_time = self.time_at_col(local_col);
                    Some(map_value(vec![
                        ("kind", keyword(":resize-start")),
                        ("id", item.id),
                        ("ids", list_value(ids)),
                        ("anchor-start", Value::Number(item.start)),
                        ("anchor-end", Value::Number(item.end)),
                        ("raw-time-offset", Value::Number(raw_time - item.start)),
                        ("alignment-helper-snapped", Value::Bool(false)),
                    ]))
                }
                HitRegion::ContentLengthEnd => {
                    Some(map_value(vec![("kind", keyword(":resize-content-length"))]))
                }
                HitRegion::Background { .. } => Some(map_value(vec![
                    ("kind", keyword(":marquee")),
                    ("time", Value::Number(current_marquee_time)),
                    ("lane", Value::Number(current_lane as f64)),
                    ("row", Value::Number(local_row as f64)),
                ])),
                HitRegion::Header => Some(map_value(vec![("kind", keyword(":scrub"))])),
                HitRegion::Sidebar { lane } => Some(map_value(vec![
                    ("kind", keyword(":sidebar")),
                    ("lane", Value::Number(lane as f64)),
                ])),
            },
            TimelineTool::Draw => Some(map_value(vec![
                ("kind", keyword(":draw")),
                ("time", Value::Number(current_time)),
                ("lane", Value::Number(current_lane as f64)),
            ])),
            TimelineTool::Marquee => Some(map_value(vec![
                ("kind", keyword(":marquee")),
                ("time", Value::Number(current_marquee_time)),
                ("lane", Value::Number(current_lane as f64)),
                ("row", Value::Number(local_row as f64)),
            ])),
            TimelineTool::Pan => Some(map_value(vec![
                ("kind", keyword(":pan")),
                ("time", Value::Number(current_time)),
                ("lane", Value::Number(current_lane as f64)),
            ])),
            TimelineTool::Scrub => Some(map_value(vec![("kind", keyword(":scrub"))])),
            TimelineTool::Erase => None,
        }
    }

    fn handle_pointer_down(&self, local_col: f32, local_row: f32) -> Option<Value> {
        let hit = self.hit_test(local_col, local_row)?;
        match self.tool {
            TimelineTool::Pointer => match hit {
                // With a title bar the body is a REGION surface, not the clip:
                // pressing it must not select the clip
                // (docs/arrangement-region-editing-spec.md 3.1/4.4 — only the
                // bar selects). It behaves like pressing empty lane space:
                // drop the current selection and park the edit cursor, which
                // is also where the region drag about to start begins.
                HitRegion::ItemBody { .. } if self.has_title_bar() => {
                    Some(action_map(vec![
                        ("type", keyword(":clear-selection")),
                        (
                            "time",
                            Value::Number(self.cursor_snap_time(self.time_at_col(local_col))),
                        ),
                    ]))
                }
                HitRegion::ItemBody { item }
                | HitRegion::ItemEdgeEnd { item }
                | HitRegion::ItemTitleBar { item }
                | HitRegion::ItemEdgeStart { item } => {
                    Some(action_map(vec![
                        ("type", keyword(":select")),
                        ("ids", list_value(vec![item.id])),
                        ("mode", keyword(":replace")),
                        (
                            "time",
                            Value::Number(self.cursor_snap_time(self.time_at_col(local_col))),
                        ),
                    ]))
                }
                HitRegion::ContentLengthEnd => None,
                HitRegion::Background { time, .. } => Some(action_map(vec![
                    ("type", keyword(":clear-selection")),
                    ("time", Value::Number(self.cursor_snap_time(time))),
                ])),
                HitRegion::Header => Some(action_map(vec![
                    ("type", keyword(":set-cursor")),
                    (
                        "time",
                        Value::Number(self.cursor_snap_time(self.time_at_col(local_col))),
                    ),
                ])),
                HitRegion::Sidebar { lane } => Some(action_map(vec![
                    ("type", keyword(":set-cursor")),
                    ("lane", Value::Number(lane as f64)),
                ])),
            },
            TimelineTool::Erase => match hit {
                HitRegion::ItemBody { item }
                | HitRegion::ItemEdgeEnd { item }
                | HitRegion::ItemTitleBar { item }
                | HitRegion::ItemEdgeStart { item } => {
                    Some(action_map(vec![
                        ("type", keyword(":delete-items")),
                        ("ids", list_value(vec![item.id])),
                    ]))
                }
                _ => None,
            },
            TimelineTool::Scrub => Some(action_map(vec![
                ("type", keyword(":set-cursor")),
                (
                    "time",
                    Value::Number(self.snap_time(self.time_at_col(local_col))),
                ),
            ])),
            _ => None,
        }
    }

    fn handle_double_click(&self, local_col: f32, local_row: f32) -> Option<Value> {
        if self.tool != TimelineTool::Pointer {
            return None;
        }
        match self.hit_test(local_col, local_row)? {
            HitRegion::Background { .. } => {
                let start = self
                    .cursor_time
                    .unwrap_or_else(|| self.cursor_snap_time(self.time_at_col(local_col)));
                let default_duration = self.default_create_duration();
                Some(action_map(vec![
                    ("type", keyword(":finish-create-item")),
                    ("lane", Value::Number(self.lane_at_row(local_row) as f64)),
                    ("start", Value::Number(start)),
                    ("end", Value::Number(start + default_duration)),
                ]))
            }
            _ => None,
        }
    }

    fn handle_pointer_drag(
        &self,
        local_col: f32,
        local_row: f32,
        gesture: Option<&Value>,
    ) -> Option<Value> {
        let current_hit = self.hit_test(local_col, local_row);
        let raw_time = self.time_at_col(local_col);
        let current_time = self.snap_time(raw_time);
        let current_resize_time = self.snap_resize_time(raw_time);
        let current_marquee_time = raw_time;
        let current_lane = self.lane_at_row(local_row);
        let gesture_value = gesture?;
        let gesture = get_map(gesture_value)?;
        // A marquee in progress must survive the pointer leaving this
        // instance: with one widget instance per arrangement track, the
        // vertical travel that names OTHER tracks is exactly the travel that
        // leaves this lane's rect (region spec 4.2), and drag capture means
        // no other instance will report it. Every other gesture still
        // requires a hit, as before.
        let is_marquee = matches!(gesture.get("kind"), Some(Value::Keyword(kind)) if kind == "marquee");
        if current_hit.is_none() && !is_marquee {
            return None;
        }
        match gesture.get("kind") {
            Some(Value::Keyword(kind)) if kind == "move" => {
                let anchor_id = gesture.get("anchor-id")?.clone();
                let anchor_start = self
                    .items
                    .iter()
                    .find(|item| item.id == anchor_id)
                    .map(|item| item.start)
                    .unwrap_or(0.0);
                let anchor_start = gesture
                    .get("anchor-start")
                    .and_then(as_number)
                    .unwrap_or(anchor_start);
                let unclamped_start = self.move_start_for_drag(
                    raw_time,
                    current_time,
                    gesture_value,
                    &gesture,
                    anchor_start,
                )?;
                let min_start = self
                    .min_selected_start_from_value(gesture.get("ids")?)
                    .unwrap_or(anchor_start);
                let clamped_start = unclamped_start.max(anchor_start - min_start).max(0.0);
                let next_lane = (current_lane as f64 - as_number(gesture.get("lane-offset")?)?)
                    .round()
                    .max(0.0);
                let anchor_lane = self
                    .items
                    .iter()
                    .find(|item| item.id == anchor_id)
                    .map(|item| item.lane as f64)
                    .unwrap_or(next_lane);
                if (clamped_start - anchor_start).abs() < f64::EPSILON
                    && (next_lane - anchor_lane).abs() < f64::EPSILON
                {
                    return None;
                }
                Some(action_map(vec![
                    ("type", keyword(":move-items-absolute")),
                    ("ids", gesture.get("ids")?.clone()),
                    ("anchor-id", anchor_id),
                    ("start", Value::Number(clamped_start)),
                    ("lane", Value::Number(next_lane)),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "resize-end" => {
                let id = gesture.get("id")?.clone();
                let item = self.items.iter().find(|item| item.id == id)?;
                let anchor_end = gesture
                    .get("anchor-end")
                    .and_then(as_number)
                    .unwrap_or(item.end);
                let next_time = self
                    .resize_end_for_drag(
                        raw_time,
                        current_resize_time,
                        gesture_value,
                        &gesture,
                        anchor_end,
                    )?
                    .max(self.minimum_end_for(item));
                Some(action_map(vec![
                    ("type", keyword(":resize-item-absolute")),
                    ("id", id.clone()),
                    ("ids", gesture.get("ids")?.clone()),
                    ("edge", keyword(":end")),
                    ("time", Value::Number(next_time)),
                    ("duration", Value::Number(next_time - item.start)),
                    ("duration-delta", Value::Number(next_time - item.end)),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "resize-start" => {
                let id = gesture.get("id")?.clone();
                let item = self.items.iter().find(|item| item.id == id)?;
                let anchor_start = gesture
                    .get("anchor-start")
                    .and_then(as_number)
                    .unwrap_or(item.start);
                let next_time = self
                    .resize_start_for_drag(
                        raw_time,
                        current_resize_time,
                        gesture_value,
                        &gesture,
                        anchor_start,
                    )?
                    .max(0.0)
                    .min(item.end - self.minimum_duration());
                Some(action_map(vec![
                    ("type", keyword(":resize-item-absolute")),
                    ("id", id.clone()),
                    ("ids", gesture.get("ids")?.clone()),
                    ("edge", keyword(":start")),
                    ("time", Value::Number(next_time)),
                    ("duration", Value::Number(item.end - next_time)),
                    ("duration-delta", Value::Number(item.start - next_time)),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "resize-content-length" => {
                let length = current_resize_time
                    .round()
                    .clamp(self.content_length_min, self.content_length_max);
                Some(action_map(vec![
                    ("type", keyword(":resize-content-length")),
                    ("length", Value::Number(length)),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "marquee" => {
                let start_time = as_number(gesture.get("time")?)?;
                let start_lane = as_number(gesture.get("lane")?)? as usize;
                let row_delta = marquee_row_delta(&gesture, local_row);
                if (start_time - current_marquee_time).abs() < f64::EPSILON
                    && start_lane == current_lane
                    && row_delta == 0.0
                {
                    if marquee_from_item_body(&gesture) {
                        return None;
                    }
                    return Some(action_map(vec![("type", keyword(":clear-selection"))]));
                }
                let lane_a = start_lane.min(current_lane);
                let lane_b = start_lane.max(current_lane);
                let (time_a, time_b) = self.marquee_span(start_time, current_marquee_time);
                Some(action_map(vec![
                    ("type", keyword(":marquee-select")),
                    ("time-a", Value::Number(time_a)),
                    ("time-b", Value::Number(time_b)),
                    ("lane-a", Value::Number(lane_a as f64)),
                    ("lane-b", Value::Number(lane_b as f64)),
                    (
                        "row-delta",
                        Value::Number(marquee_row_delta(&gesture, local_row)),
                    ),
                    ("mode", keyword(":replace")),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "draw" => {
                let start_time = as_number(gesture.get("time")?)?;
                let start_lane = as_number(gesture.get("lane")?)?;
                let (start, end) = self.bounded_span(start_time, current_resize_time);
                Some(action_map(vec![
                    ("type", keyword(":create-item")),
                    ("lane", Value::Number(start_lane)),
                    ("start", Value::Number(start)),
                    ("end", Value::Number(end)),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "pan" => {
                let start_time = as_number(gesture.get("time")?)?;
                let start_lane = as_number(gesture.get("lane")?)?;
                let next_view_start = (self.view_start + start_time - current_time).max(0.0);
                let next_lane_scroll =
                    self.clamp_lane_scroll(self.lane_scroll + start_lane - current_lane as f64);
                Some(action_map(vec![
                    ("type", keyword(":scroll-view")),
                    (
                        "delta-time",
                        Value::Number(next_view_start - self.view_start),
                    ),
                    ("view-start", Value::Number(next_view_start)),
                    (
                        "delta-lanes",
                        Value::Number(next_lane_scroll - self.lane_scroll),
                    ),
                    ("lane-scroll", Value::Number(next_lane_scroll)),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "scrub" => Some(action_map(vec![
                ("type", keyword(":set-cursor")),
                ("time", Value::Number(current_time)),
            ])),
            _ => match current_hit? {
                HitRegion::ItemBody { item }
                | HitRegion::ItemEdgeEnd { item }
                | HitRegion::ItemTitleBar { item }
                | HitRegion::ItemEdgeStart { item } => Some(action_map(vec![
                    ("type", keyword(":delete-items")),
                    ("ids", list_value(vec![item.id])),
                ])),
                _ => None,
            },
        }
    }

    fn handle_scroll(
        &self,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
    ) -> Option<Value> {
        let hit = self.hit_test(local_col, local_row)?;
        let time_step = self.scroll_time_step();
        if matches!(hit, HitRegion::Header) {
            let anchor_time = self.time_at_col(local_col);
            let factor = match mouse_kind {
                MouseEventKind::ScrollUp | MouseEventKind::ScrollRight => 1.05,
                MouseEventKind::ScrollDown | MouseEventKind::ScrollLeft => 1.0 / 1.05,
                _ => return None,
            };
            return self.zoom_action(anchor_time, factor);
        }
        if matches!(hit, HitRegion::Sidebar { .. }) {
            let factor = match mouse_kind {
                MouseEventKind::ScrollUp => 1.08,
                MouseEventKind::ScrollDown => 1.0 / 1.08,
                _ => return None,
            };
            return Some(action_map(vec![
                ("type", keyword(":zoom-lanes")),
                (
                    "anchor-lane",
                    Value::Number(self.anchor_lane_at_row(local_row)),
                ),
                ("factor", Value::Number(factor)),
            ]));
        }
        match mouse_kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if self.vertical_scroll_passthrough {
                    return None;
                }
                let lane_step = if self.smooth_scroll { 0.35 } else { 1.0 };
                let delta_lanes = match mouse_kind {
                    MouseEventKind::ScrollUp => -lane_step,
                    MouseEventKind::ScrollDown => lane_step,
                    _ => unreachable!(),
                };
                let next_lane_scroll = self.clamp_lane_scroll(self.lane_scroll + delta_lanes);
                if (next_lane_scroll - self.lane_scroll).abs() < f64::EPSILON {
                    return None;
                }
                Some(action_map(vec![
                    ("type", keyword(":scroll-view")),
                    ("delta-time", Value::Number(0.0)),
                    (
                        "delta-lanes",
                        Value::Number(next_lane_scroll - self.lane_scroll),
                    ),
                    ("lane-scroll", Value::Number(next_lane_scroll)),
                ]))
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                let time_scroll_step = if self.smooth_scroll {
                    time_step * 0.35
                } else {
                    time_step
                };
                let delta_time = match mouse_kind {
                    MouseEventKind::ScrollLeft => -time_scroll_step,
                    MouseEventKind::ScrollRight => time_scroll_step,
                    _ => unreachable!(),
                };
                let next_view_start = (self.view_start + delta_time).max(0.0);
                if (next_view_start - self.view_start).abs() < f64::EPSILON {
                    return None;
                }
                Some(action_map(vec![
                    ("type", keyword(":scroll-view")),
                    (
                        "delta-time",
                        Value::Number(next_view_start - self.view_start),
                    ),
                    ("view-start", Value::Number(next_view_start)),
                    ("delta-lanes", Value::Number(0.0)),
                ]))
            }
            _ => None,
        }
    }

    fn handle_magnify(&self, local_col: f32, local_row: f32, delta: f64) -> Option<Value> {
        let hit = self.hit_test(local_col, local_row)?;
        if delta.abs() < f64::EPSILON {
            return None;
        }
        let factor = 2.0_f64.powf(delta.clamp(-1.0, 1.0));
        if matches!(hit, HitRegion::Sidebar { .. }) {
            return Some(action_map(vec![
                ("type", keyword(":zoom-lanes")),
                (
                    "anchor-lane",
                    Value::Number(self.anchor_lane_at_row(local_row)),
                ),
                ("factor", Value::Number(factor)),
            ]));
        }
        self.zoom_action(self.time_at_col(local_col), factor)
    }

    fn handle_touchpad_scroll(
        &self,
        local_col: f32,
        local_row: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> Option<Value> {
        let hit = self.hit_test(local_col, local_row)?;
        let content = self.content_rect();
        if content.width == 0.0 || content.height == 0.0 {
            return None;
        }

        if matches!(hit, HitRegion::Header) {
            let zoom_delta = if delta_y.abs() >= delta_x.abs() {
                delta_y as f64
            } else {
                delta_x as f64
            };
            if zoom_delta.abs() < f64::EPSILON {
                return None;
            }
            let factor = 2.0_f64.powf((zoom_delta as f64 / 240.0).clamp(-1.0, 1.0));
            return self.zoom_action(self.time_at_col(local_col), factor);
        }

        if matches!(hit, HitRegion::Sidebar { .. }) {
            if delta_y.abs() < f32::EPSILON {
                return None;
            }
            let factor = 2.0_f64.powf((delta_y as f64 / 240.0).clamp(-1.0, 1.0));
            return Some(action_map(vec![
                ("type", keyword(":zoom-lanes")),
                (
                    "anchor-lane",
                    Value::Number(self.anchor_lane_at_row(local_row)),
                ),
                ("factor", Value::Number(factor)),
            ]));
        }

        let horizontal_dominant = delta_x.abs() >= delta_y.abs();
        if !horizontal_dominant && self.vertical_scroll_passthrough {
            return None;
        }
        let lane_height = self.lane_height();
        let delta_time = if horizontal_dominant {
            -(delta_x as f64 / content.width.max(1.0) as f64) * self.view_duration * 0.0625
        } else {
            0.0
        };
        let delta_lanes = if !horizontal_dominant && lane_height > 0.0 {
            -(delta_y / lane_height) as f64 * 0.0625
        } else {
            0.0
        };

        let next_view_start = (self.view_start + delta_time).max(0.0);
        let next_lane_scroll = self.clamp_lane_scroll(self.lane_scroll + delta_lanes);
        let applied_time = next_view_start - self.view_start;
        let applied_lanes = next_lane_scroll - self.lane_scroll;
        if applied_time.abs() < f64::EPSILON && applied_lanes.abs() < f64::EPSILON {
            return None;
        }
        Some(action_map(vec![
            ("type", keyword(":scroll-view")),
            ("delta-time", Value::Number(applied_time)),
            ("view-start", Value::Number(next_view_start)),
            ("delta-lanes", Value::Number(applied_lanes)),
            ("lane-scroll", Value::Number(next_lane_scroll)),
        ]))
    }

    fn handle_pointer_up(
        &self,
        local_col: f32,
        local_row: f32,
        gesture: Option<&Value>,
    ) -> Option<Value> {
        let raw_time = self.time_at_col(local_col);
        let current_resize_time = self.snap_resize_time(raw_time);
        let current_marquee_time = raw_time;
        let current_lane = self.lane_at_row(local_row);
        let gesture = get_map(gesture?)?;
        match gesture.get("kind") {
            Some(Value::Keyword(kind)) if kind == "draw" => {
                let start_time = as_number(gesture.get("time")?)?;
                let start_lane = as_number(gesture.get("lane")?)?;
                let (start, end) = if (start_time - current_resize_time).abs() < f64::EPSILON {
                    (start_time, start_time + self.default_create_duration())
                } else {
                    self.bounded_span(start_time, current_resize_time)
                };
                Some(action_map(vec![
                    ("type", keyword(":finish-create-item")),
                    ("lane", Value::Number(start_lane)),
                    ("start", Value::Number(start)),
                    ("end", Value::Number(end)),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "marquee" => {
                let start_time = as_number(gesture.get("time")?)?;
                let start_lane = as_number(gesture.get("lane")?)? as usize;
                // Vertical travel counts as movement: a single-lane instance
                // (one arrangement track lane) reports every cross-track drag
                // as lane 0 -> lane 0, so without this a straight-down region
                // sweep would read as a plain click (region spec 4.2/4.4).
                let row_delta = marquee_row_delta(&gesture, local_row);
                if (start_time - current_marquee_time).abs() < f64::EPSILON
                    && start_lane == current_lane
                    && row_delta == 0.0
                {
                    if marquee_from_item_body(&gesture) {
                        return None;
                    }
                    return Some(action_map(vec![("type", keyword(":clear-selection"))]));
                }
                let lane_a = start_lane.min(current_lane);
                let lane_b = start_lane.max(current_lane);
                let (time_a, time_b) = self.marquee_span(start_time, current_marquee_time);
                Some(action_map(vec![
                    ("type", keyword(":finish-marquee-select")),
                    ("time-a", Value::Number(time_a)),
                    ("time-b", Value::Number(time_b)),
                    ("lane-a", Value::Number(lane_a as f64)),
                    ("lane-b", Value::Number(lane_b as f64)),
                    (
                        "row-delta",
                        Value::Number(marquee_row_delta(&gesture, local_row)),
                    ),
                    ("mode", keyword(":replace")),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "move" => Some(action_map(vec![
                ("type", keyword(":finish-move-items")),
                ("ids", gesture.get("ids")?.clone()),
                ("anchor-id", gesture.get("anchor-id")?.clone()),
            ])),
            Some(Value::Keyword(kind)) if kind == "resize-end" || kind == "resize-start" => {
                Some(action_map(vec![
                    ("type", keyword(":finish-resize-items")),
                    ("ids", gesture.get("ids")?.clone()),
                    ("id", gesture.get("id")?.clone()),
                ]))
            }
            // Terminal pair for the content-length drag, mirroring the other
            // paired gestures: hosts that must not commit per drag-frame
            // (arrangement song-end -> one undoable primitive) listen for
            // this; hosts that apply :resize-content-length live are
            // unaffected by the extra release action.
            Some(Value::Keyword(kind)) if kind == "resize-content-length" => {
                let length = current_resize_time
                    .round()
                    .clamp(self.content_length_min, self.content_length_max);
                Some(action_map(vec![
                    ("type", keyword(":finish-resize-content-length")),
                    ("length", Value::Number(length)),
                ]))
            }
            _ => None,
        }
    }

    fn handle_key(&self, key: WidgetKeyEvent) -> Option<Value> {
        let selected_ids = self.selected_ids_for(Value::Nil);
        let selected_ids = selected_ids
            .into_iter()
            .filter(|id| !matches!(id, Value::Nil))
            .collect::<Vec<_>>();
        let fine_time = self.snap.max(1.0);
        let coarse_time = (fine_time * 4.0).max(4.0);
        let time_step = if key.modifiers.contains(KeyModifiers::SHIFT) {
            coarse_time
        } else {
            fine_time
        };
        let lane_step = if key.modifiers.contains(KeyModifiers::SHIFT) {
            4.0
        } else {
            1.0
        };
        match key.code {
            KeyCode::Char('c') | KeyCode::Char('C')
                if !selected_ids.is_empty()
                    && key
                        .modifiers
                        .intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL) =>
            {
                Some(action_map(vec![
                    ("type", keyword(":copy-items")),
                    ("ids", list_value(selected_ids)),
                ]))
            }
            KeyCode::Char('v') | KeyCode::Char('V')
                if key
                    .modifiers
                    .intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL) =>
            {
                Some(action_map(vec![
                    ("type", keyword(":paste-items")),
                    (
                        "time",
                        Value::Number(self.cursor_time.unwrap_or(self.view_start)),
                    ),
                ]))
            }
            KeyCode::Char('a') | KeyCode::Char('A')
                if key
                    .modifiers
                    .intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL) =>
            {
                let ids = self
                    .items
                    .iter()
                    .map(|item| item.id.clone())
                    .collect::<Vec<_>>();
                Some(action_map(vec![
                    ("type", keyword(":select")),
                    ("ids", list_value(ids)),
                    ("mode", keyword(":replace")),
                ]))
            }
            KeyCode::Left if !selected_ids.is_empty() => Some(action_map(vec![
                ("type", keyword(":nudge-selection")),
                ("ids", list_value(selected_ids)),
                (
                    "delta-time",
                    Value::Number(
                        self.clamp_nudge_delta(&self.selected_ids_for(Value::Nil), -time_step),
                    ),
                ),
                ("delta-lane", Value::Number(0.0)),
            ])),
            KeyCode::Right if !selected_ids.is_empty() => Some(action_map(vec![
                ("type", keyword(":nudge-selection")),
                ("ids", list_value(selected_ids)),
                ("delta-time", Value::Number(time_step)),
                ("delta-lane", Value::Number(0.0)),
            ])),
            KeyCode::Up if !selected_ids.is_empty() => Some(action_map(vec![
                ("type", keyword(":nudge-selection")),
                ("ids", list_value(selected_ids)),
                ("delta-time", Value::Number(0.0)),
                ("delta-lane", Value::Number(-lane_step)),
            ])),
            KeyCode::Down if !selected_ids.is_empty() => Some(action_map(vec![
                ("type", keyword(":nudge-selection")),
                ("ids", list_value(selected_ids)),
                ("delta-time", Value::Number(0.0)),
                ("delta-lane", Value::Number(lane_step)),
            ])),
            KeyCode::Backspace | KeyCode::Delete if !selected_ids.is_empty() => {
                Some(action_map(vec![
                    ("type", keyword(":delete-items")),
                    ("ids", list_value(selected_ids)),
                ]))
            }
            // Escape clears whatever is selected — items OR a region
            // (docs/arrangement-region-editing-spec.md 4.4). Gating on items
            // alone left a region highlight with no way to dismiss it, since
            // an arrangement region names no items in this lane. Still gated
            // on there being SOMETHING to clear, so Escape keeps falling
            // through to its global binding when the lane holds no selection.
            KeyCode::Esc if !selected_ids.is_empty() || self.selection_rect.is_some() => {
                Some(action_map(vec![("type", keyword(":clear-selection"))]))
            }
            KeyCode::Char('p') => Some(action_map(vec![
                ("type", keyword(":set-tool")),
                ("tool", keyword(":pointer")),
            ])),
            KeyCode::Char('d') => Some(action_map(vec![
                ("type", keyword(":set-tool")),
                ("tool", keyword(":draw")),
            ])),
            KeyCode::Char('e') => Some(action_map(vec![
                ("type", keyword(":set-tool")),
                ("tool", keyword(":erase")),
            ])),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.zoom_action(self.view_start + self.view_duration * 0.5, 1.1)
            }
            KeyCode::Char('-') => {
                self.zoom_action(self.view_start + self.view_duration * 0.5, 1.0 / 1.1)
            }
            _ => None,
        }
    }

    fn zoom_action(&self, anchor_time: f64, factor: f64) -> Option<Value> {
        self.time_viewport().zoom_action(anchor_time, factor)?;
        Some(action_map(vec![
            ("type", keyword(":zoom-view")),
            ("anchor-time", Value::Number(anchor_time)),
            ("factor", Value::Number(factor)),
        ]))
    }

    fn selected_ids_for(&self, fallback: Value) -> Vec<Value> {
        if !self.selection.is_empty() {
            return self.selection.clone();
        }
        let selected: Vec<Value> = self
            .items
            .iter()
            .filter(|item| item.selected)
            .map(|item| item.id.clone())
            .collect();
        if selected.is_empty() {
            vec![fallback]
        } else {
            selected
        }
    }

    fn min_selected_start_from_value(&self, ids: &Value) -> Option<f64> {
        let Value::List(ids) = ids else {
            return None;
        };
        self.min_selected_start(&ids.iter().map(|id| id.borrow().clone()).collect::<Vec<_>>())
    }

    fn min_selected_start(&self, ids: &[Value]) -> Option<f64> {
        self.items
            .iter()
            .filter(|item| ids.iter().any(|id| *id == item.id))
            .map(|item| item.start)
            .reduce(f64::min)
    }

    fn clamp_nudge_delta(&self, ids: &[Value], delta_time: f64) -> f64 {
        if delta_time >= 0.0 {
            return delta_time;
        }
        let Some(min_start) = self.min_selected_start(ids) else {
            return delta_time;
        };
        delta_time.max(-min_start)
    }

    fn scroll_time_step(&self) -> f64 {
        self.snap.max(1.0)
    }
}

/// Whether a `:marquee` gesture began on a clip body rather than empty lane
/// background (docs/arrangement-region-editing-spec.md 3.1).
fn marquee_from_item_body(gesture: &HashMap<String, Value>) -> bool {
    matches!(gesture.get("origin"), Some(Value::Keyword(origin)) if origin == "item-body")
}

/// Vertical travel of a `:marquee` gesture in cells: the pointer's current
/// row minus the row the gesture began on, signed and **unclamped** — the
/// pointer routinely leaves the instance rect, and drag capture keeps the
/// events here (docs/arrangement-region-editing-spec.md 4.2). A host with one
/// widget instance per track reconstructs the track span from this; a
/// multi-lane host reads `lane-a`/`lane-b` instead and ignores it.
///
/// Both rows are already scroll-adjusted (`scroll_adjusted_row`), so the
/// difference is free of any scroll the container did mid-drag.
fn marquee_row_delta(gesture: &HashMap<String, Value>, local_row: f32) -> f64 {
    // A gesture map without a start row cannot say how far the pointer
    // travelled; report no travel rather than the raw row.
    let Some(start_row) = gesture.get("row").and_then(as_number) else {
        return 0.0;
    };
    local_row as f64 - start_row
}

fn vertical_scroll_passthrough(props: &HashMap<String, Value>) -> bool {
    matches!(
        props.get("scroll-passthrough"),
        Some(Value::Keyword(mode)) | Some(Value::String(mode)) if mode == "vertical"
    )
}

/// Pointer rows arrive in scroll-viewport coordinates when this instance
/// lives inside a `scroll` container; node rects are in content coordinates,
/// so shift by the per-event scroll offset (0 outside a container).
fn scroll_adjusted_row(local_row: f32) -> f32 {
    local_row + super::scroll::current_event_scroll_offset()
}

fn get_tool(props: &HashMap<String, Value>) -> TimelineTool {
    match props.get("tool") {
        Some(Value::Keyword(tool)) | Some(Value::String(tool)) => match tool.as_str() {
            "draw" => TimelineTool::Draw,
            "erase" => TimelineTool::Erase,
            "marquee" => TimelineTool::Marquee,
            "pan" => TimelineTool::Pan,
            "scrub" => TimelineTool::Scrub,
            _ => TimelineTool::Pointer,
        },
        _ => TimelineTool::Pointer,
    }
}

fn get_sidebar_style(props: &HashMap<String, Value>) -> SidebarStyle {
    match props.get("sidebar-style") {
        Some(Value::Keyword(style)) | Some(Value::String(style)) if style == "piano" => {
            SidebarStyle::Piano
        }
        _ => SidebarStyle::Default,
    }
}

fn get_num(props: &HashMap<String, Value>, key: &str, default: f64) -> f64 {
    match props.get(key) {
        Some(Value::Number(n)) => *n,
        _ => default,
    }
}

fn get_lanes(props: &HashMap<String, Value>) -> Vec<TimelineLane> {
    let Some(Value::List(items)) = props.get("lanes") else {
        return vec![TimelineLane {
            _id: Value::Number(0.0),
            label: None,
            sidebar_bg: None,
            label_fg: None,
        }];
    };
    items
        .iter()
        .filter_map(|item| {
            get_map(&item.borrow()).map(|map| TimelineLane {
                _id: map.get("id").cloned().unwrap_or(Value::Nil),
                label: map.get("label").and_then(as_string),
                sidebar_bg: map.get("sidebar-bg").and_then(as_color),
                label_fg: map.get("label-fg").and_then(as_color),
            })
        })
        .collect()
}

fn get_items(props: &HashMap<String, Value>) -> Vec<TimelineItem> {
    let Some(Value::List(items)) = props.get("items") else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|item| {
            let map = get_map(&item.borrow())?;
            Some(TimelineItem {
                id: map.get("id").cloned().unwrap_or(Value::Nil),
                lane: map.get("lane").and_then(as_usize).unwrap_or(0),
                start: map.get("start").and_then(as_number).unwrap_or(0.0),
                end: map.get("end").and_then(as_number).unwrap_or(0.0),
                selected: map.get("selected").and_then(as_bool).unwrap_or(false),
                label: map.get("label").and_then(as_string),
                color: map.get("color").map(|value| {
                    resolve_named_color(
                        &HashMap::from([("color".to_string(), value.clone())]),
                        "color",
                        theme::WIDGET_SLIDER_FILLED(),
                    )
                }),
                kind: map.get("kind").and_then(parse_item_kind),
                content: map.get("content").and_then(parse_item_content),
            })
        })
        .collect()
}

/// Lenient `:kind` parse (docs/arrangement-timeline-ui-spec.md 7): unknown or
/// malformed values are `None`, never a render error.
fn parse_item_kind(value: &Value) -> Option<TimelineItemKind> {
    match as_string(value)?.as_str() {
        "midi" => Some(TimelineItemKind::Midi),
        "audio" => Some(TimelineItemKind::Audio),
        "scene" => Some(TimelineItemKind::Scene),
        _ => None,
    }
}

/// Lenient `:content` parse (docs/arrangement-timeline-ui-spec.md 7): a map
/// with a `:dots` or `:peaks` list; malformed entries are skipped in the same
/// `filter_map` spirit as `get_items`, and anything else is `None`.
fn parse_item_content(value: &Value) -> Option<TimelineItemContent> {
    let map = get_map(value)?;
    if let Some(Value::List(entries)) = map.get("dots") {
        let dots = entries
            .iter()
            .filter_map(|entry| {
                let entry = get_map(&entry.borrow())?;
                Some(TimelineDot {
                    offset: entry.get("offset").and_then(as_number)?.clamp(0.0, 1.0),
                    value: entry.get("value").and_then(as_number)?.clamp(0.0, 1.0),
                    // Optional (spec 3.2): a missing or malformed :width
                    // degrades to the point dot, never a parse failure.
                    width: entry
                        .get("width")
                        .and_then(as_number)
                        .map(|width| width.clamp(0.0, 1.0))
                        .unwrap_or(0.0),
                })
            })
            .collect();
        // A malformed or out-of-range :cycle degrades to no tiling.
        let cycle = map
            .get("cycle")
            .and_then(as_number)
            .filter(|cycle| *cycle > 0.0 && *cycle <= 1.0)
            .unwrap_or(1.0);
        return Some(TimelineItemContent::Dots { dots, cycle });
    }
    if let Some(Value::List(entries)) = map.get("peaks") {
        let peaks = entries
            .iter()
            .filter_map(|entry| {
                let entry = get_map(&entry.borrow())?;
                Some(PeakBucket {
                    min: entry.get("min").and_then(as_number)?.clamp(-1.0, 1.0) as f32,
                    max: entry.get("max").and_then(as_number)?.clamp(-1.0, 1.0) as f32,
                })
            })
            .collect();
        return Some(TimelineItemContent::Peaks(peaks));
    }
    None
}

fn get_selection(props: &HashMap<String, Value>) -> Vec<Value> {
    let Some(Value::List(items)) = props.get("selection") else {
        return vec![];
    };
    items.iter().map(|item| item.borrow().clone()).collect()
}

fn get_selection_rect(props: &HashMap<String, Value>) -> Option<TimelineSelectionRect> {
    let map = props.get("selection-rect").and_then(get_map)?;
    Some(TimelineSelectionRect {
        time_a: map.get("time-a").and_then(as_number)?,
        time_b: map.get("time-b").and_then(as_number)?,
        lane_a: map.get("lane-a").and_then(as_usize)?,
        lane_b: map.get("lane-b").and_then(as_usize)?,
    })
}

fn get_time_ruler(map: &HashMap<String, Value>) -> Option<TimeRuler> {
    let mode = map.get("mode").and_then(as_string)?;
    match mode.as_str() {
        "bars-beats" => Some(TimeRuler {
            mode: TimeRulerMode::BarsBeats {
                beats_per_bar: map
                    .get("beats-per-bar")
                    .and_then(as_number)
                    .unwrap_or(4.0)
                    .max(1.0) as i64,
            },
        }),
        _ => None,
    }
}

fn get_map(value: &Value) -> Option<HashMap<String, Value>> {
    match value {
        Value::Map(map) => Some(
            map.iter()
                .map(|(key, value)| (key.clone(), value.borrow().clone()))
                .collect(),
        ),
        _ => None,
    }
}

fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => Some(*n),
        Value::ReactiveRef { slot, .. } => Some(crate::reactive::read_float_slot(slot)),
        _ => None,
    }
}

fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn map_bool(value: &Value, key: &str) -> bool {
    let Value::Map(map) = value else {
        return false;
    };
    map.get(key)
        .and_then(|value| as_bool(&value.borrow()))
        .unwrap_or(false)
}

fn set_map_bool(value: &Value, key: &str, next: bool) {
    let Value::Map(map) = value else {
        return;
    };
    if let Some(value) = map.get(key) {
        *value.borrow_mut() = Value::Bool(next);
    }
}

fn as_usize(value: &Value) -> Option<usize> {
    as_number(value).map(|n| n.max(0.0) as usize)
}

fn as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) | Value::Keyword(s) => Some(s.clone()),
        _ => None,
    }
}

fn as_color(value: &Value) -> Option<crate::backend::Color> {
    let name = as_string(value)?;
    Some(resolve_named_color(
        &HashMap::from([("color".to_string(), Value::Keyword(name))]),
        "color",
        theme::FG(),
    ))
}

fn action_map(entries: Vec<(&str, Value)>) -> Value {
    map_value(entries)
}

fn keyword(name: &str) -> Value {
    Value::Keyword(name.trim_start_matches(':').to_string())
}

fn map_value(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect(),
    )
}

fn list_value(items: Vec<Value>) -> Value {
    Value::List(
        items
            .into_iter()
            .map(|value| Rc::new(RefCell::new(value)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyword_value(name: &str) -> Value {
        Value::Keyword(name.to_string())
    }

    fn number_value(value: f64) -> Value {
        Value::Number(value)
    }

    fn bool_value(value: bool) -> Value {
        Value::Bool(value)
    }

    fn list_value_raw(items: Vec<Value>) -> Value {
        Value::List(
            items
                .into_iter()
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        )
    }

    fn map_value_raw(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
                .collect(),
        )
    }

    fn lanes_value(count: usize) -> Value {
        list_value_raw(
            (0..count)
                .map(|index| {
                    map_value_raw(vec![
                        ("id", number_value(index as f64)),
                        ("label", Value::String(format!("L{index}"))),
                    ])
                })
                .collect(),
        )
    }

    /// docs/arrangement-timeline-ui-spec.md 7: `:kind`/`:content` are
    /// optional and lenient — absent keys parse exactly as before (piano-roll
    /// regression), malformed values become `None`/get skipped, never a
    /// render error.
    #[test]
    fn items_parse_optional_kind_and_content_leniently() {
        let items = list_value_raw(vec![
            // Piano-roll-shaped item: no kind/content keys at all.
            map_value_raw(vec![
                ("id", number_value(1.0)),
                ("lane", number_value(0.0)),
                ("start", number_value(0.0)),
                ("end", number_value(4.0)),
            ]),
            // MIDI clip with dots; one malformed dot entry is skipped and
            // out-of-range values clamp.
            map_value_raw(vec![
                ("id", number_value(2.0)),
                ("start", number_value(4.0)),
                ("end", number_value(8.0)),
                ("kind", keyword_value("midi")),
                (
                    "content",
                    map_value_raw(vec![(
                        "dots",
                        list_value_raw(vec![
                            map_value_raw(vec![
                                ("offset", number_value(0.25)),
                                ("value", number_value(0.5)),
                            ]),
                            map_value_raw(vec![
                                ("offset", number_value(2.0)),
                                ("value", number_value(-1.0)),
                            ]),
                            map_value_raw(vec![("offset", number_value(0.5))]),
                            Value::String("junk".to_string()),
                        ]),
                    )]),
                ),
            ]),
            // Audio clip with peak buckets.
            map_value_raw(vec![
                ("id", number_value(3.0)),
                ("start", number_value(8.0)),
                ("end", number_value(12.0)),
                ("kind", keyword_value("audio")),
                (
                    "content",
                    map_value_raw(vec![(
                        "peaks",
                        list_value_raw(vec![map_value_raw(vec![
                            ("min", number_value(-0.5)),
                            ("max", number_value(0.5)),
                        ])]),
                    )]),
                ),
            ]),
            // Unknown kind and malformed content are None, not errors.
            map_value_raw(vec![
                ("id", number_value(4.0)),
                ("start", number_value(12.0)),
                ("end", number_value(16.0)),
                ("kind", keyword_value("automation")),
                ("content", number_value(3.0)),
            ]),
        ]);
        let props = HashMap::from([("items".to_string(), items)]);
        let items = get_items(&props);
        assert_eq!(items.len(), 4);

        assert!(items[0].kind.is_none());
        assert!(items[0].content.is_none());

        assert_eq!(items[1].kind, Some(TimelineItemKind::Midi));
        let Some(TimelineItemContent::Dots { dots, cycle }) = &items[1].content else {
            panic!("expected dots content");
        };
        assert_eq!(dots.len(), 2, "malformed dot entries are skipped");
        assert_eq!(dots[0].offset, 0.25);
        assert_eq!(dots[0].value, 0.5);
        assert_eq!(dots[1].offset, 1.0, "offset clamps to 0..1");
        assert_eq!(dots[1].value, 0.0, "value clamps to 0..1");
        assert_eq!(*cycle, 1.0, "absent :cycle means content spans the item");

        assert_eq!(items[2].kind, Some(TimelineItemKind::Audio));
        let Some(TimelineItemContent::Peaks(peaks)) = &items[2].content else {
            panic!("expected peaks content");
        };
        assert_eq!(peaks.len(), 1);

        assert!(items[3].kind.is_none(), "unknown kind parses to None");
        assert!(items[3].content.is_none(), "malformed content parses to None");
    }

    /// `:cycle` marks how much of the item one content repetition covers
    /// (looping-clip visualization); out-of-range values degrade to 1.0.
    #[test]
    fn dots_content_parses_optional_cycle_fraction() {
        let content_with_cycle = |cycle: Value| {
            map_value_raw(vec![
                (
                    "dots",
                    list_value_raw(vec![map_value_raw(vec![
                        ("offset", number_value(0.5)),
                        ("value", number_value(0.5)),
                    ])]),
                ),
                ("cycle", cycle),
            ])
        };
        let parsed_cycle = |value: Value| match parse_item_content(&content_with_cycle(value)) {
            Some(TimelineItemContent::Dots { cycle, .. }) => cycle,
            _ => panic!("expected dots content"),
        };
        assert_eq!(parsed_cycle(number_value(0.25)), 0.25);
        assert_eq!(parsed_cycle(number_value(1.0)), 1.0);
        assert_eq!(parsed_cycle(number_value(0.0)), 1.0, "zero degrades");
        assert_eq!(parsed_cycle(number_value(4.0)), 1.0, ">1 degrades");
        assert_eq!(parsed_cycle(number_value(-0.5)), 1.0, "negative degrades");
        assert_eq!(
            parsed_cycle(Value::String("junk".to_string())),
            1.0,
            "malformed degrades"
        );
    }

    /// The ruler must not invent a grid line/label at the view's left edge:
    /// the first mark is the first one at or after view-start, not an
    /// off-screen mark clamped to the edge.
    #[test]
    fn ruler_starts_at_first_mark_inside_the_view() {
        let props = HashMap::from([
            ("view-start".to_string(), number_value(2.5)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(2.0)),
            (
                "time-ruler".to_string(),
                map_value_raw(vec![
                    ("mode", keyword_value("bars-beats")),
                    ("beats-per-bar", number_value(4.0)),
                ]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let ruler = view.time_ruler.clone();
        let viewport = view.time_viewport();
        // At this zoom the grid step is 4 beats; the first mark inside
        // [2.5, 18.5) is beat 4 — nothing may sit clamped at the left edge.
        let lines = viewport.metal_grid_lines(ruler.as_ref());
        assert!(!lines.is_empty());
        let content_left = viewport.content_rect().col;
        let expected_x = view.x_for_time(4.0);
        assert!(
            (lines[0].0 - expected_x).abs() < 0.01,
            "first grid line must be at beat 4 (x {expected_x}), got {}",
            lines[0].0
        );
        assert!(lines[0].0 > content_left + 0.5, "no edge-clamped line");

        let labels = viewport.metal_time_ruler_labels(ruler.as_ref());
        assert!(!labels.is_empty());
        assert_eq!(labels[0].1, "2", "beat 4 labels as bar 2");
        assert!(
            (labels[0].0 - expected_x).abs() < 0.5,
            "first label sits on its grid line, not pinned to the view edge"
        );
    }

    /// `:scroll-passthrough :vertical` declines vertical wheel/touchpad
    /// deltas (so an enclosing scroll container can take them) while
    /// horizontal deltas still pan the shared time axis.
    #[test]
    fn vertical_scroll_passthrough_declines_vertical_but_pans_horizontal() {
        let props = HashMap::from([
            ("scroll-passthrough".to_string(), keyword_value("vertical")),
            ("view-start".to_string(), number_value(8.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("lanes".to_string(), lanes_value(8)),
            ("lane-height".to_string(), number_value(1.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 4.0,
            },
        );
        assert!(vertical_scroll_passthrough(&props));
        // Vertical-dominant touchpad delta: declined even though the lanes
        // could scroll (8 lanes in a 4-row viewport).
        assert!(view.handle_touchpad_scroll(10.0, 2.0, 0.0, -30.0).is_none());
        // Vertical wheel: declined.
        assert!(view
            .handle_scroll(MouseEventKind::ScrollDown, 10.0, 2.0)
            .is_none());
        // Horizontal-dominant touchpad delta still pans the time axis.
        let action = view
            .handle_touchpad_scroll(10.0, 2.0, -30.0, 2.0)
            .expect("horizontal pan still handled");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("scroll-view".to_string()))
        );

        // Without the prop, the same vertical deltas scroll the lanes.
        let mut default_props = props.clone();
        default_props.remove("scroll-passthrough");
        let view = TimelineView::from_props(
            &default_props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 4.0,
            },
        );
        assert!(view.handle_touchpad_scroll(10.0, 2.0, 0.0, -30.0).is_some());
        assert!(view
            .handle_scroll(MouseEventKind::ScrollDown, 10.0, 2.0)
            .is_some());
    }

    /// The content-length drag gets a terminal action on release (mirroring
    /// the other paired gestures) so hosts can commit one undoable edit
    /// instead of applying per drag-frame.
    #[test]
    fn content_length_drag_release_emits_finish_action_with_final_length() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("lanes".to_string(), lanes_value(1)),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("content-length".to_string(), number_value(8.0)),
            ("content-length-min".to_string(), number_value(1.0)),
            ("content-length-max".to_string(), number_value(64.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let gesture = map_value_raw(vec![("kind", keyword_value("resize-content-length"))]);
        let action = view
            .handle_pointer_up(24.0, 2.0, Some(&gesture))
            .expect("release action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("finish-resize-content-length".to_string()))
        );
        assert_eq!(
            map.get("length").map(|value| value.borrow().clone()),
            Some(Value::Number(12.0)),
            "24 of 32 cols over a 16-duration view is time 12"
        );
    }

    #[test]
    fn latched_move_gesture_continues_after_pointer_leaves_item_bounds() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(8.0)),
                    ("selected", bool_value(true)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(10.0, 2.0).expect("gesture");

        let action = view
            .handle_pointer_drag(27.0, 2.0, Some(&gesture))
            .expect("drag action");

        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("move-items-absolute".to_string()))
        );
    }

    #[test]
    fn no_op_item_drag_emits_no_move_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(8.0)),
                    ("selected", bool_value(false)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(10.0, 2.0).expect("gesture");
        assert!(
            view.handle_pointer_drag(10.2, 2.1, Some(&gesture))
                .is_none()
        );
    }

    #[test]
    fn alignment_helper_move_edits_fractional_start_until_next_grid_line() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.25)),
                    ("end", number_value(5.25)),
                    ("selected", bool_value(false)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            (
                "move-snap-mode".to_string(),
                keyword_value("alignment-helper"),
            ),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(8.2, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(8.95, 2.0, Some(&gesture))
            .expect("fractional move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        let Some(Value::Number(start)) = map.get("start").map(|value| value.borrow().clone())
        else {
            panic!("expected fractional start");
        };
        assert!((start - 4.625).abs() < 0.0001, "start was {start}");

        let action = view
            .handle_pointer_drag(9.7, 2.0, Some(&gesture))
            .expect("grid move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(5.0))
        );

        let action = view
            .handle_pointer_drag(8.95, 2.0, Some(&gesture))
            .expect("latched grid move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(4.0))
        );
    }

    #[test]
    fn alignment_helper_move_allows_backwards_fractional_start_before_previous_grid_line() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(5.0)),
                    ("selected", bool_value(false)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            (
                "move-snap-mode".to_string(),
                keyword_value("alignment-helper"),
            ),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(8.2, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(7.2, 2.0, Some(&gesture))
            .expect("backwards fractional move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        let Some(Value::Number(start)) = map.get("start").map(|value| value.borrow().clone())
        else {
            panic!("expected backwards fractional start");
        };
        assert!((start - 3.5).abs() < 0.0001, "start was {start}");

        let action = view
            .handle_pointer_drag(6.6, 2.0, Some(&gesture))
            .expect("backwards snapped move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(3.0))
        );

        let action = view
            .handle_pointer_drag(7.2, 2.0, Some(&gesture))
            .expect("latched backwards snapped move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(3.0))
        );
    }

    #[test]
    fn alignment_helper_move_sticks_to_current_grid_when_delayed_note_moves_backwards() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.25)),
                    ("end", number_value(5.25)),
                    ("selected", bool_value(false)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            (
                "move-snap-mode".to_string(),
                keyword_value("alignment-helper"),
            ),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(8.6, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(8.3, 2.0, Some(&gesture))
            .expect("fractional move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        let Some(Value::Number(start)) = map.get("start").map(|value| value.borrow().clone())
        else {
            panic!("expected fractional start");
        };
        assert!((start - 4.1).abs() < 0.0001, "start was {start}");

        let action = view
            .handle_pointer_drag(7.9, 2.0, Some(&gesture))
            .expect("current-grid snapped move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(4.0))
        );

        let action = view
            .handle_pointer_drag(7.1, 2.0, Some(&gesture))
            .expect("current-grid sticky move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(4.0))
        );

        let action = view
            .handle_pointer_drag(6.5, 2.0, Some(&gesture))
            .expect("previous-grid snapped move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(3.0))
        );
    }

    #[test]
    fn alignment_helper_move_uses_zoomed_ruler_grid_step() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.25)),
                    ("end", number_value(5.25)),
                    ("selected", bool_value(false)),
                ])]),
            ),
            (
                "time-ruler".to_string(),
                map_value_raw(vec![
                    ("mode", keyword_value("bars-beats")),
                    ("beats-per-bar", number_value(4.0)),
                ]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(8.0)),
            ("snap".to_string(), number_value(1.0)),
            (
                "move-snap-mode".to_string(),
                keyword_value("alignment-helper"),
            ),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 192.0,
                height: 8.0,
            },
        );
        assert_eq!(view.alignment_helper_grid_step(), 0.5);

        let gesture = view.begin_gesture(102.0, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(108.0, 2.0, Some(&gesture))
            .expect("zoomed grid move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(4.5))
        );
    }

    #[test]
    fn pointer_click_sets_cursor_time_to_zoomed_grid() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "time-ruler".to_string(),
                map_value_raw(vec![
                    ("mode", keyword_value("bars-beats")),
                    ("beats-per-bar", number_value(4.0)),
                ]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(8.0)),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 192.0,
                height: 8.0,
            },
        );

        let action = view.handle_pointer_down(107.0, 2.0).expect("clear action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("clear-selection".to_string()))
        );
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(4.5))
        );
    }

    #[test]
    fn item_click_sets_cursor_time_to_zoomed_grid() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "time-ruler".to_string(),
                map_value_raw(vec![
                    ("mode", keyword_value("bars-beats")),
                    ("beats-per-bar", number_value(4.0)),
                ]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(5.0)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(8.0)),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 192.0,
                height: 8.0,
            },
        );

        let action = view.handle_pointer_down(107.0, 2.0).expect("select action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("select".to_string()))
        );
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(4.5))
        );
    }

    /// `:selection-rect-style :region` (region spec 4.4, Ableton arrangement):
    /// the selection LIGHTS the lane rather than washing a translucent
    /// marquee over the top of it. Empty lane background inside the span and
    /// every covered clip's BODY take the selected-body colour, while the
    /// clip's title bar keeps the clip colour — so the highlight reads as one
    /// continuous band across clips and the gaps between them, and never
    /// hides the clips behind a veil the way the marquee style would.
    #[cfg(target_os = "macos")]
    #[test]
    fn region_selection_style_lights_the_lane_and_clip_bodies() {
        const CLIP: crate::backend::Color = crate::backend::Color {
            r: 0.9,
            g: 0.2,
            b: 0.4,
            a: 1.0,
        };
        let props = |style: Option<&str>| {
            let mut props = HashMap::from([
                (
                    "items".to_string(),
                    list_value_raw(vec![map_value_raw(vec![
                        ("id", number_value(1.0)),
                        ("lane", number_value(0.0)),
                        ("start", number_value(4.0)),
                        ("end", number_value(12.0)),
                    ])]),
                ),
                (
                    "item-color".to_string(),
                    list_value_raw(vec![
                        number_value(CLIP.r as f64),
                        number_value(CLIP.g as f64),
                        number_value(CLIP.b as f64),
                    ]),
                ),
                ("title-bar-height".to_string(), number_value(1.0)),
                ("view-start".to_string(), number_value(0.0)),
                ("view-duration".to_string(), number_value(16.0)),
                ("header-height".to_string(), number_value(0.0)),
                (
                    "selection-rect".to_string(),
                    map_value_raw(vec![
                        ("time-a", number_value(0.0)),
                        ("time-b", number_value(8.0)),
                        ("lane-a", number_value(0.0)),
                        ("lane-b", number_value(0.0)),
                    ]),
                ),
            ]);
            if let Some(style) = style {
                props.insert("selection-rect-style".to_string(), keyword_value(style));
            }
            props
        };
        let rect = Rect {
            row: 0.0,
            col: 0.0,
            width: 160.0,
            height: 4.0,
        };
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1920.0,
            vp_h: 1080.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 12.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };
        let build = |style: Option<&str>| {
            let node = LayoutNode {
                widget_id: 1,
                stable_widget_id: None,
                subtree_root_id: None,
                parent_subtree_root_id: None,
                stable_key: None,
                widget_type: "timeline".to_string(),
                rect,
                props: props(style),
                children: Vec::new(),
                focusable: false,
            };
            build_metal_primitives(&node, viewport)
        };
        // Last quad of a given colour covering a point — i.e. what the eye
        // actually sees there once everything has been painted.
        let top_color_at = |primitives: &[MetalPrimitive], x: f32, y: f32| {
            primitives
                .iter()
                .filter_map(|primitive| match primitive {
                    MetalPrimitive::Quad(quad)
                        if quad.color.a >= 0.999
                            && x >= quad.x
                            && x < quad.x + quad.width
                            && y >= quad.y
                            && y < quad.y + quad.height =>
                    {
                        Some(quad.color)
                    }
                    _ => None,
                })
                .next_back()
        };
        let marquee_blue = |primitives: &[MetalPrimitive]| {
            primitives.iter().any(|primitive| {
                matches!(primitive, MetalPrimitive::Quad(quad)
                    if (quad.color.r - 0.38).abs() < 0.001
                        && (quad.color.g - 0.68).abs() < 0.001
                        && (quad.color.b - 0.92).abs() < 0.001)
            })
        };

        // Beats: the region covers 0..8, the clip covers 4..12, so beat 2 is
        // bare lane inside the region, beat 6 is clip inside it, and beat 10
        // is clip outside it.
        let x_at = |beat: f32| rect.col + rect.width * (beat / 16.0);
        let bar_y = 0.5;
        let body_y = 3.0;

        let region = build(Some("region"));
        assert!(
            !marquee_blue(&region),
            "the region style must not wash the blue marquee over the lane"
        );
        assert_eq!(
            top_color_at(&region, x_at(2.0), body_y),
            Some(SELECTED_ITEM_BODY_COLOR),
            "bare lane inside the region lights up"
        );
        assert_eq!(
            top_color_at(&region, x_at(2.0), bar_y),
            Some(SELECTED_ITEM_BODY_COLOR),
            "with no clip there, the lit band is full lane height"
        );
        assert_eq!(
            top_color_at(&region, x_at(6.0), body_y),
            Some(SELECTED_ITEM_BODY_COLOR),
            "a covered clip's BODY lights up in the same colour"
        );
        assert_eq!(
            top_color_at(&region, x_at(6.0), bar_y),
            Some(CLIP),
            "but its title bar keeps the clip colour"
        );
        assert_eq!(
            top_color_at(&region, x_at(10.0), body_y),
            Some(CLIP),
            "the part of the clip outside the region is untouched"
        );

        // The opaque fill would swallow the lane's grid, so the grid is
        // redrawn over it: bar lines stay legible inside the selection, in
        // the same dark wash a clip body uses for its own grid continuation.
        let grid_over_region = region.iter().any(|primitive| {
            matches!(primitive, MetalPrimitive::Quad(quad)
                if quad.width < 0.2
                    && quad.color.a < 0.5
                    && quad.color.r < 0.05
                    && quad.x > x_at(0.0)
                    && quad.x < x_at(8.0))
        });
        assert!(
            grid_over_region,
            "grid lines must be redrawn on top of the region fill"
        );

        // Default style (piano roll, scene lane) is unchanged: a translucent
        // blue wash with an outline, and no body relighting.
        let marquee = build(None);
        assert!(marquee_blue(&marquee), "the default style keeps the marquee");
        assert_eq!(
            top_color_at(&marquee, x_at(6.0), body_y),
            Some(CLIP),
            "the marquee style never repaints a clip body"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn metal_cursor_marker_starts_below_ruler_with_triangle_marker() {
        let props = HashMap::from([
            ("cursor-time".to_string(), number_value(4.0)),
            ("header-height".to_string(), number_value(2.0)),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(8.0)),
        ]);
        let rect = Rect {
            row: 0.0,
            col: 0.0,
            width: 192.0,
            height: 12.0,
        };
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1920.0,
            vp_h: 1080.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 12.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "timeline".to_string(),
            rect,
            props,
            children: Vec::new(),
            focusable: false,
        };

        let primitives = build_metal_primitives(&node, viewport);
        let triangle = primitives
            .iter()
            .find_map(|primitive| match primitive {
                MetalPrimitive::Triangle(triangle) if triangle.color == theme::CURSOR() => {
                    Some(*triangle)
                }
                _ => None,
            })
            .expect("cursor triangle");
        assert_eq!(triangle.points[0][1], 2.0);
        assert_eq!(triangle.points[1][1], 2.0);

        let line = primitives
            .iter()
            .find_map(|primitive| match primitive {
                MetalPrimitive::Quad(quad)
                    if ((quad.x + quad.width * 0.5) - triangle.points[2][0]).abs() < 0.001
                        && (quad.y - triangle.points[2][1]).abs() < 0.001 =>
                {
                    Some(*quad)
                }
                _ => None,
            })
            .expect("cursor line");
        assert!(line.y > 2.0, "line should start below marker tip");
        assert!(
            line.height < rect.height - 2.0,
            "line should not extend into the ruler"
        );
    }

    #[test]
    fn timeline_copy_and_paste_keys_emit_item_clipboard_actions() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(5.0)),
                    ("selected", bool_value(true)),
                ])]),
            ),
            (
                "selection".to_string(),
                list_value_raw(vec![number_value(10.0)]),
            ),
            ("cursor-time".to_string(), number_value(6.5)),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let action = view
            .handle_key(WidgetKeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::SUPER,
            })
            .expect("copy action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("copy-items".to_string()))
        );

        let action = view
            .handle_key(WidgetKeyEvent {
                code: KeyCode::Char('v'),
                modifiers: KeyModifiers::SUPER,
            })
            .expect("paste action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("paste-items".to_string()))
        );
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(6.5))
        );
    }

    #[test]
    fn right_edge_resize_hit_allows_slightly_outside_note() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(8.0)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(16.4, 2.0).expect("gesture");
        let Value::Map(map) = gesture else {
            panic!("expected gesture map");
        };
        assert_eq!(
            map.get("kind").map(|value| value.borrow().clone()),
            Some(Value::Keyword("resize-end".to_string()))
        );
    }

    #[test]
    fn left_edge_click_starts_move_not_resize() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(8.0)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(8.2, 2.0).expect("gesture");
        let Value::Map(map) = gesture else {
            panic!("expected gesture map");
        };
        assert_eq!(
            map.get("kind").map(|value| value.borrow().clone()),
            Some(Value::Keyword("move".to_string()))
        );
    }

    #[test]
    fn resize_drag_uses_resize_snap_independent_from_move_snap() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(8.0)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            ("resize-snap".to_string(), number_value(0.5)),
            ("snap-mode".to_string(), keyword_value("floor")),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(16.0, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(17.5, 2.0, Some(&gesture))
            .expect("resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("resize-item-absolute".to_string()))
        );
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(8.5))
        );
    }

    #[test]
    fn resize_selected_item_sends_selection_ids_and_duration_delta() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![
                    map_value_raw(vec![
                        ("id", number_value(10.0)),
                        ("lane", number_value(0.0)),
                        ("start", number_value(4.0)),
                        ("end", number_value(8.0)),
                        ("selected", bool_value(true)),
                    ]),
                    map_value_raw(vec![
                        ("id", number_value(11.0)),
                        ("lane", number_value(0.0)),
                        ("start", number_value(9.0)),
                        ("end", number_value(12.0)),
                        ("selected", bool_value(true)),
                    ]),
                ]),
            ),
            (
                "selection".to_string(),
                list_value_raw(vec![number_value(10.0), number_value(11.0)]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(16.0, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(18.0, 2.0, Some(&gesture))
            .expect("resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("resize-item-absolute".to_string()))
        );
        assert_eq!(
            map.get("duration-delta")
                .map(|value| value.borrow().clone()),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            map.get("duration").map(|value| value.borrow().clone()),
            Some(Value::Number(5.0))
        );
        let ids = map.get("ids").expect("ids");
        let Value::List(ids) = &*ids.borrow() else {
            panic!("expected ids list");
        };
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn resize_snap_rounds_to_nearest_boundary_when_configured() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(8.0)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            ("resize-snap".to_string(), number_value(0.5)),
            ("snap-mode".to_string(), keyword_value("floor")),
            ("resize-snap-mode".to_string(), keyword_value("round")),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(16.0, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(17.4, 2.0, Some(&gesture))
            .expect("resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(8.5))
        );

        let action = view
            .handle_pointer_drag(17.6, 2.0, Some(&gesture))
            .expect("resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(9.0))
        );
    }

    #[test]
    fn resize_alignment_helper_edits_fractional_end_until_next_grid_line() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(5.0)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            ("resize-snap".to_string(), number_value(1.0)),
            (
                "resize-snap-mode".to_string(),
                keyword_value("alignment-helper"),
            ),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(10.0, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(10.8, 2.0, Some(&gesture))
            .expect("fractional resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        let Some(Value::Number(time)) = map.get("time").map(|value| value.borrow().clone()) else {
            panic!("expected fractional resize time");
        };
        assert!((time - 5.4).abs() < 0.0001, "time was {time}");

        let action = view
            .handle_pointer_drag(12.2, 2.0, Some(&gesture))
            .expect("snapped resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(6.0))
        );

        let action = view
            .handle_pointer_drag(10.8, 2.0, Some(&gesture))
            .expect("latched snapped resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(5.0))
        );
    }

    #[test]
    fn resize_alignment_helper_sticks_to_current_grid_when_delayed_end_moves_backwards() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(3.0)),
                    ("end", number_value(5.25)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            ("resize-snap".to_string(), number_value(1.0)),
            (
                "resize-snap-mode".to_string(),
                keyword_value("alignment-helper"),
            ),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(10.0, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(9.7, 2.0, Some(&gesture))
            .expect("fractional resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        let Some(Value::Number(time)) = map.get("time").map(|value| value.borrow().clone()) else {
            panic!("expected fractional resize time");
        };
        assert!((time - 5.1).abs() < 0.0001, "time was {time}");

        let action = view
            .handle_pointer_drag(9.4, 2.0, Some(&gesture))
            .expect("current-grid snapped resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(5.0))
        );

        let action = view
            .handle_pointer_drag(8.6, 2.0, Some(&gesture))
            .expect("current-grid sticky resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(5.0))
        );

        let action = view
            .handle_pointer_drag(7.8, 2.0, Some(&gesture))
            .expect("previous-grid snapped resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(4.0))
        );
    }

    #[test]
    fn pointer_up_emits_move_and_resize_completion_actions() {
        let view = TimelineView::from_props(
            &HashMap::new(),
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let move_gesture = map_value_raw(vec![
            ("kind", keyword_value("move")),
            ("ids", list_value_raw(vec![number_value(7.0)])),
            ("anchor-id", number_value(7.0)),
        ]);
        let resize_gesture = map_value_raw(vec![
            ("kind", keyword_value("resize-end")),
            ("ids", list_value_raw(vec![number_value(7.0)])),
            ("id", number_value(7.0)),
        ]);

        for (gesture, expected) in [
            (move_gesture, "finish-move-items"),
            (resize_gesture, "finish-resize-items"),
        ] {
            let Value::Map(action) = view
                .handle_pointer_up(8.0, 2.0, Some(&gesture))
                .expect("gesture completion action")
            else {
                panic!("expected action map");
            };
            assert_eq!(
                action.get("type").map(|value| value.borrow().clone()),
                Some(Value::Keyword(expected.to_string())),
            );
        }
    }

    #[test]
    fn draw_create_clamps_to_minimum_snap_duration() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("draw")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            ("resize-snap".to_string(), number_value(0.25)),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let gesture = map_value_raw(vec![
            ("kind", keyword_value("draw")),
            ("time", number_value(4.0)),
            ("lane", number_value(0.0)),
        ]);
        let action = view
            .handle_pointer_up(8.0, 2.0, Some(&gesture))
            .expect("finish action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(4.0))
        );
        assert_eq!(
            map.get("end").map(|value| value.borrow().clone()),
            Some(Value::Number(4.25))
        );
    }

    #[test]
    fn double_click_create_uses_configured_create_duration() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            ("resize-snap".to_string(), number_value(0.25)),
            ("create-duration".to_string(), number_value(2.5)),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view.handle_double_click(8.0, 2.0).expect("create action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(4.0))
        );
        assert_eq!(
            map.get("end").map(|value| value.borrow().clone()),
            Some(Value::Number(6.5))
        );
    }

    #[test]
    fn draw_click_create_uses_configured_create_duration() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("draw")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            ("resize-snap".to_string(), number_value(0.25)),
            ("create-duration".to_string(), number_value(2.5)),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let gesture = map_value_raw(vec![
            ("kind", keyword_value("draw")),
            ("time", number_value(4.0)),
            ("lane", number_value(0.0)),
        ]);
        let action = view
            .handle_pointer_up(8.0, 2.0, Some(&gesture))
            .expect("finish action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("end").map(|value| value.borrow().clone()),
            Some(Value::Number(6.5))
        );
    }

    #[test]
    fn grid_resize_snap_follows_zoomed_ruler_resolution() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(8.0)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(4.0)),
            ("resize-snap".to_string(), keyword_value("grid")),
            ("resize-snap-mode".to_string(), keyword_value("round")),
            (
                "time-ruler".to_string(),
                map_value_raw(vec![
                    ("mode", keyword_value("bars-beats")),
                    ("beats-per-bar", number_value(4.0)),
                ]),
            ),
        ]);

        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 128.0,
                height: 8.0,
            },
        );

        assert_eq!(view.effective_resize_snap(), 0.5);
        assert_eq!(view.snap_resize_time(3.4), 3.5);
    }

    #[test]
    fn double_click_create_uses_nearest_zoomed_cursor_grid_snap() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(4.0)),
            ("snap".to_string(), number_value(1.0)),
            ("resize-snap".to_string(), keyword_value("grid")),
            ("resize-snap-mode".to_string(), keyword_value("round")),
            (
                "time-ruler".to_string(),
                map_value_raw(vec![
                    ("mode", keyword_value("bars-beats")),
                    ("beats-per-bar", number_value(4.0)),
                ]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 128.0,
                height: 8.0,
            },
        );

        let action = view
            .handle_double_click(76.8, 2.0)
            .expect("double click should create");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(2.5))
        );
        assert_eq!(
            map.get("end").map(|value| value.borrow().clone()),
            Some(Value::Number(3.0))
        );
    }

    #[test]
    fn marquee_selection_uses_raw_pointer_times() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(4.0)),
            ("snap".to_string(), number_value(1.0)),
            ("resize-snap".to_string(), keyword_value("grid")),
            ("resize-snap-mode".to_string(), keyword_value("round")),
            (
                "time-ruler".to_string(),
                map_value_raw(vec![
                    ("mode", keyword_value("bars-beats")),
                    ("beats-per-bar", number_value(4.0)),
                ]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 128.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(76.8, 2.0).expect("marquee gesture");
        let action = view
            .handle_pointer_drag(108.8, 2.0, Some(&gesture))
            .expect("marquee action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        let time_a = map
            .get("time-a")
            .and_then(|value| as_number(&value.borrow()))
            .expect("time-a");
        let time_b = map
            .get("time-b")
            .and_then(|value| as_number(&value.borrow()))
            .expect("time-b");
        assert!((time_a - 2.4).abs() < 0.0001);
        assert!((time_b - 3.4).abs() < 0.0001);
    }

    /// A single-lane instance (one arrangement track lane) reports vertical
    /// travel as `row-delta` in cells, signed and unclamped: drag capture
    /// keeps every event in the originating lane, so the host reconstructs the
    /// track span from this (region spec 4.2). `lane-a`/`lane-b` stay pinned
    /// at 0 because the instance has exactly one lane.
    fn single_lane_marquee_view(extra: Vec<(&str, Value)>) -> TimelineView {
        let mut props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(4.0)),
            ("header-height".to_string(), number_value(0.0)),
            ("sidebar-width".to_string(), number_value(0.0)),
            (
                "time-ruler".to_string(),
                map_value_raw(vec![
                    ("mode", keyword_value("bars-beats")),
                    ("beats-per-bar", number_value(4.0)),
                ]),
            ),
        ]);
        for (key, value) in extra {
            props.insert(key.to_string(), value);
        }
        TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 128.0,
                height: 3.0,
            },
        )
    }

    fn action_number(action: &Value, key: &str) -> f64 {
        let Value::Map(map) = action else {
            panic!("expected action map, got {action:?}");
        };
        map.get(key)
            .and_then(|value| as_number(&value.borrow()))
            .unwrap_or_else(|| panic!("action has no numeric :{key}"))
    }

    #[test]
    fn marquee_reports_signed_unclamped_row_delta() {
        let view = single_lane_marquee_view(vec![]);
        let gesture = view.begin_gesture(32.0, 1.5).expect("marquee gesture");

        // Downward drag, far past the instance's 3-row rect.
        let down = view
            .handle_pointer_drag(64.0, 9.5, Some(&gesture))
            .expect("marquee action");
        assert!((action_number(&down, "row-delta") - 8.0).abs() < 1e-9);
        // One lane: the lane span cannot express the travel, only row-delta.
        assert_eq!(action_number(&down, "lane-a"), 0.0);
        assert_eq!(action_number(&down, "lane-b"), 0.0);

        // Upward drag, above the top of the rect: negative and unclamped.
        let up = view
            .handle_pointer_drag(64.0, -4.5, Some(&gesture))
            .expect("marquee action");
        assert!((action_number(&up, "row-delta") + 6.0).abs() < 1e-9);

        // The release payload carries the same travel.
        let finish = view
            .handle_pointer_up(64.0, 9.5, Some(&gesture))
            .expect("finish marquee action");
        assert!((action_number(&finish, "row-delta") - 8.0).abs() < 1e-9);
    }

    /// `:marquee-snap :grid` (region spec 4.3): the low edge floors and the
    /// high edge ceils onto the zoom-adaptive ladder, so a sloppy drag still
    /// selects whole grid cells. Hosts that pass nothing — the piano roll —
    /// keep the raw pointer times.
    #[test]
    fn marquee_snap_grid_floors_min_and_ceils_max() {
        let snapped =
            single_lane_marquee_view(vec![("marquee-snap", keyword_value("grid"))]);
        let grid = snapped.alignment_helper_grid_step();
        assert_eq!(grid, 0.5, "fixture must sit on a known grid rung");

        // 76.8 cells -> beat 2.4, 108.8 -> beat 3.4: neither is on a grid line.
        let gesture = snapped.begin_gesture(76.8, 1.0).expect("marquee gesture");
        let action = snapped
            .handle_pointer_drag(108.8, 1.0, Some(&gesture))
            .expect("marquee action");
        assert!((action_number(&action, "time-a") - 2.0).abs() < 1e-9);
        assert!((action_number(&action, "time-b") - 3.5).abs() < 1e-9);

        // Dragging right-to-left snaps the same span: order is normalized
        // before quantization.
        let back = snapped.begin_gesture(108.8, 1.0).expect("marquee gesture");
        let action = snapped
            .handle_pointer_drag(76.8, 1.0, Some(&back))
            .expect("marquee action");
        assert!((action_number(&action, "time-a") - 2.0).abs() < 1e-9);
        assert!((action_number(&action, "time-b") - 3.5).abs() < 1e-9);

        // A vertical-only drag (no horizontal travel) would quantize to an
        // empty span; it widens to the one cell the pointer is in.
        let vertical = snapped.begin_gesture(64.0, 1.0).expect("marquee gesture");
        let action = snapped
            .handle_pointer_drag(64.0, 8.0, Some(&vertical))
            .expect("marquee action");
        assert!((action_number(&action, "time-a") - 2.0).abs() < 1e-9);
        assert!((action_number(&action, "time-b") - 2.5).abs() < 1e-9);

        // Without the prop, raw pointer times survive unchanged.
        let raw = single_lane_marquee_view(vec![]);
        let gesture = raw.begin_gesture(76.8, 1.0).expect("marquee gesture");
        let action = raw
            .handle_pointer_drag(108.8, 1.0, Some(&gesture))
            .expect("marquee action");
        assert!((action_number(&action, "time-a") - 2.4).abs() < 1e-4);
        assert!((action_number(&action, "time-b") - 3.4).abs() < 1e-4);
    }

    #[test]
    fn content_length_marks_unavailable_region() {
        let props = HashMap::from([
            ("view-start".to_string(), number_value(12.0)),
            ("view-duration".to_string(), number_value(8.0)),
            ("content-length".to_string(), number_value(16.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 100.0,
                height: 8.0,
            },
        );

        assert_eq!(view.unavailable_rect(), Some((50.0, 0.0, 50.0, 8.0)));
    }

    #[test]
    fn selection_prop_marks_matching_item_selected() {
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(8.0)),
                    ("selected", bool_value(false)),
                ])]),
            ),
            (
                "selection".to_string(),
                list_value_raw(vec![number_value(10.0)]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );

        assert!(view.item_selected(&view.items[0]));
    }

    #[test]
    fn header_scroll_up_emits_zoom_view_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_scroll(MouseEventKind::ScrollUp, 10.0, 0.0)
            .expect("zoom action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("zoom-view".to_string()))
        );
        assert!(matches!(
            map.get("factor").map(|value| value.borrow().clone()),
            Some(Value::Number(factor)) if factor > 1.0
        ));
    }

    #[test]
    fn sidebar_scroll_up_emits_lane_zoom_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("sidebar-width".to_string(), number_value(4.0)),
            ("lane-scroll".to_string(), number_value(10.0)),
            ("lane-height".to_string(), number_value(2.0)),
            ("lanes".to_string(), lanes_value(24)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_scroll(MouseEventKind::ScrollUp, 1.0, 2.0)
            .expect("lane zoom action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("zoom-lanes".to_string()))
        );
        assert_eq!(
            map.get("anchor-lane").map(|value| value.borrow().clone()),
            Some(Value::Number(10.5))
        );
        assert!(matches!(
            map.get("factor").map(|value| value.borrow().clone()),
            Some(Value::Number(factor)) if factor > 1.0
        ));
    }

    #[test]
    fn sidebar_header_intersection_scroll_emits_lane_zoom_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("header-height".to_string(), number_value(1.0)),
            ("sidebar-width".to_string(), number_value(4.0)),
            ("lane-scroll".to_string(), number_value(10.0)),
            ("lane-height".to_string(), number_value(2.0)),
            ("lanes".to_string(), lanes_value(24)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_scroll(MouseEventKind::ScrollUp, 1.0, 0.5)
            .expect("lane zoom action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("zoom-lanes".to_string()))
        );
    }

    #[test]
    fn sidebar_touchpad_scroll_emits_lane_zoom_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("sidebar-width".to_string(), number_value(4.0)),
            ("lane-scroll".to_string(), number_value(10.0)),
            ("lane-height".to_string(), number_value(2.0)),
            ("lanes".to_string(), lanes_value(24)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_touchpad_scroll(1.0, 2.0, 0.0, 24.0)
            .expect("lane zoom action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("zoom-lanes".to_string()))
        );
        assert_eq!(
            map.get("anchor-lane").map(|value| value.borrow().clone()),
            Some(Value::Number(10.5))
        );
    }

    #[test]
    fn sidebar_magnify_emits_lane_zoom_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("sidebar-width".to_string(), number_value(4.0)),
            ("lane-scroll".to_string(), number_value(10.0)),
            ("lane-height".to_string(), number_value(2.0)),
            ("lanes".to_string(), lanes_value(24)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_magnify(1.0, 2.0, 0.25)
            .expect("lane zoom action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("zoom-lanes".to_string()))
        );
        assert_eq!(
            map.get("anchor-lane").map(|value| value.borrow().clone()),
            Some(Value::Number(10.5))
        );
    }

    #[test]
    fn bars_beats_time_ruler_formats_header_labels() {
        let props = HashMap::from([
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            (
                "time-ruler".to_string(),
                map_value_raw(vec![
                    ("mode", keyword_value("bars-beats")),
                    ("beats-per-bar", number_value(4.0)),
                ]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 129.0,
                height: 8.0,
            },
        );
        let labels: Vec<String> = view
            .time_ruler_labels()
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        assert!(labels.iter().any(|label| label == "1"));
        assert!(labels.iter().any(|label| label == "1.2"));
        assert!(labels.iter().any(|label| label == "1.3"));
        assert!(labels.iter().any(|label| label == "1.4"));
        assert!(labels.iter().any(|label| label == "2"));
    }

    #[test]
    fn header_scroll_zoom_uses_unsnapped_anchor_time() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 18.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_scroll(MouseEventKind::ScrollUp, 9.5, 0.0)
            .expect("zoom action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert!(matches!(
            map.get("anchor-time").map(|value| value.borrow().clone()),
            Some(Value::Number(anchor)) if (anchor.fract()).abs() > f64::EPSILON
        ));
    }

    #[test]
    fn magnify_emits_zoom_view_action_with_pointer_anchor() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_magnify(10.5, 2.0, 0.2)
            .expect("magnify zoom action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("zoom-view".to_string()))
        );
        assert!(matches!(
            map.get("factor").map(|value| value.borrow().clone()),
            Some(Value::Number(factor)) if factor > 1.0
        ));
        assert!(matches!(
            map.get("anchor-time").map(|value| value.borrow().clone()),
            Some(Value::Number(anchor)) if anchor > 0.0
        ));
    }

    #[test]
    fn touchpad_scroll_locks_to_dominant_axis() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(4.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("lane-scroll".to_string(), number_value(8.0)),
            ("lane-height".to_string(), number_value(1.0)),
            ("lanes".to_string(), lanes_value(24)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_touchpad_scroll(10.0, 2.0, 12.0, 8.0)
            .expect("touchpad scroll action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("scroll-view".to_string()))
        );
        assert!(matches!(
            map.get("delta-time").map(|value| value.borrow().clone()),
            Some(Value::Number(delta)) if delta < 0.0 && delta > -16.0
        ));
        assert_eq!(
            map.get("delta-lanes").map(|value| value.borrow().clone()),
            Some(Value::Number(0.0))
        );

        let action = view
            .handle_touchpad_scroll(10.0, 2.0, 8.0, 12.0)
            .expect("touchpad scroll action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("delta-time").map(|value| value.borrow().clone()),
            Some(Value::Number(0.0))
        );
        assert!(matches!(
            map.get("delta-lanes").map(|value| value.borrow().clone()),
            Some(Value::Number(delta)) if delta < 0.0 && delta > -8.0
        ));
    }

    #[test]
    fn metal_lane_rect_supports_fractional_clipping() {
        let props = HashMap::from([
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("lane-scroll".to_string(), number_value(0.25)),
            ("lane-height".to_string(), number_value(2.0)),
            ("lanes".to_string(), lanes_value(8)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let (y, height) = view.metal_lane_rect(0).expect("partially visible top lane");
        assert!(y >= 1.0, "top lane should be clipped to the content top");
        assert!(height < 1.75, "top lane height should be partially clipped");
    }

    #[test]
    fn header_scroll_up_at_zoom_min_emits_no_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(8.0)),
            ("zoom-min-duration".to_string(), number_value(8.0)),
            ("zoom-max-duration".to_string(), number_value(128.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        assert!(
            view.handle_scroll(MouseEventKind::ScrollUp, 10.0, 0.0)
                .is_none()
        );
    }

    #[test]
    fn header_scroll_down_at_zoom_max_emits_no_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(128.0)),
            ("zoom-min-duration".to_string(), number_value(8.0)),
            ("zoom-max-duration".to_string(), number_value(128.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        assert!(
            view.handle_scroll(MouseEventKind::ScrollDown, 10.0, 0.0)
                .is_none()
        );
    }

    #[test]
    fn lane_scroll_prop_clamps_to_last_full_view() {
        let props = HashMap::from([
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(1.0)),
            ("lane-scroll".to_string(), number_value(42.0)),
            ("lane-height".to_string(), number_value(1.0)),
            ("lanes".to_string(), lanes_value(16)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        assert_eq!(view.lane_scroll, 9.0);
    }

    #[test]
    fn content_scroll_down_at_lane_bottom_emits_no_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(1.0)),
            ("lane-scroll".to_string(), number_value(9.0)),
            ("lane-height".to_string(), number_value(1.0)),
            ("lanes".to_string(), lanes_value(16)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        assert!(
            view.handle_scroll(MouseEventKind::ScrollDown, 10.0, 2.0)
                .is_none()
        );
    }

    #[test]
    fn lane_scroll_clamps_to_scroll_viewport_height_when_content_is_taller() {
        let props = HashMap::from([
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(2.0)),
            ("lane-scroll".to_string(), number_value(96.0)),
            ("lane-height".to_string(), number_value(0.5)),
            ("scroll-viewport-height".to_string(), number_value(11.5)),
            ("lanes".to_string(), lanes_value(97)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 35.0,
            },
        );
        assert_eq!(view.lane_scroll, 78.0);
    }

    #[test]
    fn content_scroll_down_emits_lane_scroll_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("lane-height".to_string(), number_value(1.0)),
            ("lanes".to_string(), lanes_value(16)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_scroll(MouseEventKind::ScrollDown, 10.0, 2.0)
            .expect("scroll action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("scroll-view".to_string()))
        );
        assert_eq!(
            map.get("delta-time").map(|value| value.borrow().clone()),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            map.get("delta-lanes").map(|value| value.borrow().clone()),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            map.get("lane-scroll").map(|value| value.borrow().clone()),
            Some(Value::Number(1.0))
        );
    }

    #[test]
    fn content_scroll_right_emits_time_scroll_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_scroll(MouseEventKind::ScrollRight, 10.0, 2.0)
            .expect("time scroll action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("scroll-view".to_string()))
        );
        assert_eq!(
            map.get("delta-lanes").map(|value| value.borrow().clone()),
            Some(Value::Number(0.0))
        );
        assert!(matches!(
            map.get("delta-time").map(|value| value.borrow().clone()),
            Some(Value::Number(delta)) if delta > 0.0
        ));
    }

    #[test]
    fn content_scroll_left_at_zero_emits_no_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        assert!(
            view.handle_scroll(MouseEventKind::ScrollLeft, 10.0, 2.0)
                .is_none(),
            "scrolling past time zero should not emit a no-op action"
        );
    }

    #[test]
    fn draw_pointer_up_emits_finish_create_item_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("draw")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let gesture = view.begin_gesture(6.0, 2.0).expect("gesture");
        let action = view
            .handle_pointer_up(12.0, 2.0, Some(&gesture))
            .expect("finish action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("finish-create-item".to_string()))
        );
    }

    #[test]
    fn background_click_finishes_as_clear_selection() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let gesture = view.begin_gesture(10.0, 2.0).expect("gesture");
        let action = view
            .handle_pointer_up(10.0, 2.0, Some(&gesture))
            .expect("clear action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("clear-selection".to_string()))
        );
    }

    #[test]
    fn right_arrow_emits_nudge_selection_action() {
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(8.0)),
                    ("selected", bool_value(true)),
                ])]),
            ),
            ("snap".to_string(), number_value(1.0)),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_key(WidgetKeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::NONE,
            })
            .expect("nudge action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("nudge-selection".to_string()))
        );
    }

    #[test]
    fn move_action_clamps_group_start_at_zero() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
            (
                "items".to_string(),
                list_value_raw(vec![
                    map_value_raw(vec![
                        ("id", number_value(10.0)),
                        ("lane", number_value(0.0)),
                        ("start", number_value(1.0)),
                        ("end", number_value(4.0)),
                        ("selected", bool_value(true)),
                    ]),
                    map_value_raw(vec![
                        ("id", number_value(11.0)),
                        ("lane", number_value(0.0)),
                        ("start", number_value(5.0)),
                        ("end", number_value(8.0)),
                        ("selected", bool_value(true)),
                    ]),
                ]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let gesture = map_value_raw(vec![
            ("kind", keyword_value("move")),
            (
                "ids",
                list_value_raw(vec![number_value(10.0), number_value(11.0)]),
            ),
            ("anchor-id", number_value(10.0)),
            ("time-offset", number_value(3.0)),
            ("lane-offset", number_value(0.0)),
        ]);
        let action = view
            .handle_pointer_drag(0.0, 2.0, Some(&gesture))
            .expect("move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(0.0))
        );
    }

    #[test]
    fn left_nudge_clamps_at_zero() {
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(0.0)),
                    ("end", number_value(3.0)),
                    ("selected", bool_value(true)),
                ])]),
            ),
            ("snap".to_string(), number_value(1.0)),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let action = view
            .handle_key(WidgetKeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::NONE,
            })
            .expect("nudge action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("delta-time").map(|value| value.borrow().clone()),
            Some(Value::Number(0.0))
        );
    }

    #[test]
    fn playhead_time_maps_to_visible_column() {
        let props = HashMap::from([
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("playhead-time".to_string(), number_value(8.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 17.0,
                height: 8.0,
            },
        );
        assert_eq!(view.playhead_col(), Some(8));
    }

    #[test]
    fn snapped_item_start_matches_grid_column() {
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(10.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(8.0)),
                    ("end", number_value(12.0)),
                    ("selected", bool_value(false)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 17.0,
                height: 8.0,
            },
        );
        let grid_col = view
            .grid_columns()
            .into_iter()
            .find_map(|(col, _)| (col == 8).then_some(col))
            .expect("grid line at beat 8");
        let item = view.items.first().expect("item");
        let rect = view.item_rect(item).expect("item rect");
        assert_eq!(rect.col.round() as u16, grid_col);
    }
    // ── Clip anatomy (docs/arrangement-region-editing-spec.md 3.1) ─────────

    /// One 8-beat item over a 16-beat view in a 20-row-tall lane, optionally
    /// with a title bar.
    fn title_bar_view(title_bar_height: Option<f64>) -> TimelineView {
        let mut props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(1.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(12.0)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(0.0)),
        ]);
        if let Some(height) = title_bar_height {
            props.insert("title-bar-height".to_string(), number_value(height));
        }
        TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 16.0,
                height: 8.0,
            },
        )
    }

    fn hit_name(view: &TimelineView, col: f32, row: f32) -> &'static str {
        match view.hit_test(col, row) {
            Some(HitRegion::ItemTitleBar { .. }) => "title-bar",
            Some(HitRegion::ItemEdgeStart { .. }) => "edge-start",
            Some(HitRegion::ItemEdgeEnd { .. }) => "edge-end",
            Some(HitRegion::ItemBody { .. }) => "body",
            Some(HitRegion::Background { .. }) => "background",
            Some(HitRegion::Header) => "header",
            Some(HitRegion::Sidebar { .. }) => "sidebar",
            Some(HitRegion::ContentLengthEnd) => "content-length-end",
            None => "none",
        }
    }

    /// With no title bar (the piano-roll default) the item is exactly what it
    /// has always been: an end handle spanning the full height and body
    /// everywhere else, with no start handle anywhere.
    #[test]
    fn hit_test_without_a_title_bar_is_unchanged() {
        let view = title_bar_view(None);
        // Item spans cols 4..12 (1 cell per beat).
        for row in [0.1_f32, 4.0, 7.9] {
            assert_eq!(hit_name(&view, 4.2, row), "body", "row {row}");
            assert_eq!(hit_name(&view, 8.0, row), "body", "row {row}");
            assert_eq!(hit_name(&view, 11.8, row), "edge-end", "row {row}");
            // The start edge is a title-bar-only affordance.
            assert_eq!(hit_name(&view, 3.5, row), "background", "row {row}");
        }
    }

    /// With a title bar the handles and the move zone live on the bar only;
    /// everything below it is body (the region-selection surface).
    #[test]
    fn hit_test_splits_the_item_at_the_title_bar() {
        let view = title_bar_view(Some(2.0));
        // Bar rows [0, 2): start handle, middle, end handle.
        for row in [0.1_f32, 1.9] {
            assert_eq!(hit_name(&view, 3.5, row), "edge-start", "row {row}");
            assert_eq!(hit_name(&view, 4.2, row), "edge-start", "row {row}");
            assert_eq!(hit_name(&view, 8.0, row), "title-bar", "row {row}");
            assert_eq!(hit_name(&view, 11.8, row), "edge-end", "row {row}");
        }
        // Body rows [2, 8): no handles at all, including at both edges.
        for row in [2.1_f32, 5.0, 7.9] {
            assert_eq!(hit_name(&view, 4.2, row), "body", "row {row}");
            assert_eq!(hit_name(&view, 8.0, row), "body", "row {row}");
            assert_eq!(hit_name(&view, 11.8, row), "body", "row {row}");
            assert_eq!(hit_name(&view, 3.5, row), "background", "row {row}");
        }
    }

    /// Back-to-back clips share one boundary, and each wants a handle there.
    /// The point decides: left of the boundary is the LEFT clip's end handle,
    /// right of it the RIGHT clip's start handle — never the other way round,
    /// whatever the draw order. (The bug this pins: a fat handle plus
    /// symmetric outside-slop made the zones overlap, so approaching a clip's
    /// end edge flipped to resizing the next clip's start.)
    #[test]
    fn abutting_clips_split_their_shared_boundary_by_side() {
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![
                    map_value_raw(vec![
                        ("id", number_value(1.0)),
                        ("lane", number_value(0.0)),
                        ("start", number_value(0.0)),
                        ("end", number_value(8.0)),
                    ]),
                    map_value_raw(vec![
                        ("id", number_value(2.0)),
                        ("lane", number_value(0.0)),
                        ("start", number_value(8.0)),
                        ("end", number_value(16.0)),
                    ]),
                ]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(0.0)),
            ("title-bar-height".to_string(), number_value(2.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect { row: 0.0, col: 0.0, width: 16.0, height: 8.0 },
        );
        let item_at = |col: f32| match view.hit_test(col, 1.0) {
            Some(HitRegion::ItemEdgeEnd { item }) | Some(HitRegion::ItemEdgeStart { item }) => {
                match item.id {
                    Value::Number(id) => id,
                    other => panic!("expected a numeric item id, got {other:?}"),
                }
            }
            _ => panic!("expected an edge handle at {col}"),
        };
        // Boundary at col 8. Just inside the left clip: its end handle.
        assert_eq!(hit_name(&view, 7.9, 1.0), "edge-end");
        assert_eq!(item_at(7.9), 1.0);
        // Just inside the right clip: its start handle.
        assert_eq!(hit_name(&view, 8.0, 1.0), "edge-start");
        assert_eq!(item_at(8.0), 2.0);

        // And the handles stay narrow: a long clip is mostly title bar, not
        // grip. Half a beat in from either edge is already the move zone.
        assert_eq!(hit_name(&view, 1.5, 1.0), "title-bar");
        assert_eq!(hit_name(&view, 6.5, 1.0), "title-bar");
    }

    /// The bar never swallows the whole clip: a title bar taller than the
    /// lane still leaves a body row to select in.
    #[test]
    fn title_bar_never_consumes_the_whole_item() {
        let view = title_bar_view(Some(64.0));
        assert_eq!(hit_name(&view, 8.0, 0.5), "title-bar");
        assert_eq!(hit_name(&view, 8.0, 7.5), "body");
    }

    /// Cursors: move over the bar, resize over both handles, default in the
    /// body — and nothing but default anywhere without a title bar.
    #[test]
    fn title_bar_zones_report_their_cursors() {
        let with_bar = title_bar_view(Some(2.0));
        assert_eq!(
            cursor_for_hit(with_bar.hit_test(8.0, 1.0)),
            super::super::WidgetCursor::Grab
        );
        assert_eq!(
            cursor_for_hit(with_bar.hit_test(4.2, 1.0)),
            super::super::WidgetCursor::EwResize
        );
        assert_eq!(
            cursor_for_hit(with_bar.hit_test(11.8, 1.0)),
            super::super::WidgetCursor::EwResize
        );
        assert_eq!(
            cursor_for_hit(with_bar.hit_test(8.0, 5.0)),
            super::super::WidgetCursor::Default
        );
    }

    /// Mirror of `TimelineWidget::cursor`'s mapping, driven straight off a
    /// hit region so the test needs no `LayoutNode`.
    fn cursor_for_hit(hit: Option<HitRegion>) -> super::super::WidgetCursor {
        match hit {
            Some(HitRegion::ItemEdgeEnd { .. })
            | Some(HitRegion::ItemEdgeStart { .. })
            | Some(HitRegion::ContentLengthEnd) => super::super::WidgetCursor::EwResize,
            Some(HitRegion::ItemTitleBar { .. }) => super::super::WidgetCursor::Grab,
            _ => super::super::WidgetCursor::Default,
        }
    }

    /// Gestures follow the zones: the bar moves, the body marquees (tagged
    /// so a zero-movement release does not clear the selection), and the
    /// start handle opens a `:resize-start`.
    #[test]
    fn title_bar_zones_begin_their_gestures() {
        let gesture_kind = |value: &Value| {
            let map = get_map(value).expect("gesture map");
            match map.get("kind") {
                Some(Value::Keyword(kind)) => kind.clone(),
                other => panic!("expected a gesture kind, got {other:?}"),
            }
        };
        let with_bar = title_bar_view(Some(2.0));
        assert_eq!(
            gesture_kind(&with_bar.begin_gesture(8.0, 1.0).expect("bar gesture")),
            "move"
        );
        assert_eq!(
            gesture_kind(&with_bar.begin_gesture(4.2, 1.0).expect("start gesture")),
            "resize-start"
        );
        assert_eq!(
            gesture_kind(&with_bar.begin_gesture(11.8, 1.0).expect("end gesture")),
            "resize-end"
        );
        let body = with_bar.begin_gesture(8.0, 5.0).expect("body gesture");
        assert_eq!(gesture_kind(&body), "marquee");
        assert!(marquee_from_item_body(&get_map(&body).expect("gesture map")));

        // Without a title bar the body still starts a move, as it always has.
        let without_bar = title_bar_view(None);
        assert_eq!(
            gesture_kind(&without_bar.begin_gesture(8.0, 5.0).expect("body gesture")),
            "move"
        );
    }

    /// Pointer-down follows the same split: only the TITLE BAR selects the
    /// clip. Pressing the body starts a region instead, so it must behave
    /// like pressing empty lane space — clear the selection and park the edit
    /// cursor — rather than selecting the whole clip out from under the drag
    /// that is about to start (region spec 3.1/4.4).
    #[test]
    fn title_bar_selects_the_clip_but_the_body_clears_and_places_the_cursor() {
        let action_type = |value: &Value| {
            let Value::Map(map) = value else {
                panic!("expected action map, got {value:?}");
            };
            match map.get("type").map(|cell| cell.borrow().clone()) {
                Some(Value::Keyword(kind)) => kind,
                other => panic!("expected an action type, got {other:?}"),
            }
        };
        let with_bar = title_bar_view(Some(2.0));
        assert_eq!(
            action_type(&with_bar.handle_pointer_down(8.0, 1.0).expect("bar press")),
            "select",
            "the title bar is the clip-selection zone"
        );
        let body = with_bar.handle_pointer_down(8.0, 5.0).expect("body press");
        assert_eq!(
            action_type(&body),
            "clear-selection",
            "the body is a region surface: pressing it never selects the clip"
        );
        let Value::Map(map) = &body else {
            panic!("expected action map");
        };
        assert!(
            map.contains_key("time"),
            "the body press still carries a time so the host can park the cursor"
        );

        // Without a title bar (piano roll) a body press selects, as always.
        let without_bar = title_bar_view(None);
        assert_eq!(
            action_type(&without_bar.handle_pointer_down(8.0, 5.0).expect("body press")),
            "select"
        );
    }

    /// Escape dismisses a REGION, not just selected items (region spec 4.4).
    /// An arrangement region names no items in the lane it covers, so gating
    /// Escape on the item selection left the highlight stuck on screen. It
    /// still declines Escape when the lane holds nothing, so the key keeps
    /// falling through to its global binding.
    #[test]
    fn escape_clears_a_region_selection_not_just_selected_items() {
        let escape = |view: &TimelineView| {
            view.handle_key(WidgetKeyEvent {
                code: KeyCode::Esc,
                modifiers: KeyModifiers::empty(),
            })
        };
        let action_type = |value: &Value| {
            let Value::Map(map) = value else {
                panic!("expected action map, got {value:?}");
            };
            match map.get("type").map(|cell| cell.borrow().clone()) {
                Some(Value::Keyword(kind)) => kind,
                other => panic!("expected an action type, got {other:?}"),
            }
        };

        let mut props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(1.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(12.0)),
                ])]),
            ),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(0.0)),
        ]);
        let rect = Rect {
            row: 0.0,
            col: 0.0,
            width: 16.0,
            height: 4.0,
        };
        assert!(
            escape(&TimelineView::from_props(&props, rect)).is_none(),
            "nothing selected: Escape falls through to its global binding"
        );

        // A region with no selected item still clears — this is the case that
        // was stuck.
        props.insert(
            "selection-rect".to_string(),
            map_value_raw(vec![
                ("time-a", number_value(0.0)),
                ("time-b", number_value(8.0)),
                ("lane-a", number_value(0.0)),
                ("lane-b", number_value(0.0)),
            ]),
        );
        let action = escape(&TimelineView::from_props(&props, rect)).expect("region escape");
        assert_eq!(action_type(&action), "clear-selection");

        // And a selected item with no region clears, as it always has.
        props.remove("selection-rect");
        props.insert("selection".to_string(), list_value_raw(vec![number_value(1.0)]));
        let action = escape(&TimelineView::from_props(&props, rect)).expect("item escape");
        assert_eq!(action_type(&action), "clear-selection");
    }

    /// `:grid-density` divides the zoom-adaptive grid step, so a host can get
    /// a finer grid — and finer `:grid` snapping — without zooming in. The
    /// ladder rungs are powers of two, so the denser grid stays aligned to the
    /// coarser one: bar lines never move, lines appear between them. Hosts
    /// that pass nothing (the piano roll) keep the stock ladder exactly.
    #[test]
    fn grid_density_subdivides_the_ladder_without_moving_bar_lines() {
        let step_for = |density: Option<f64>, view_duration: f64| {
            let mut props = HashMap::from([
                ("view-start".to_string(), number_value(0.0)),
                ("view-duration".to_string(), number_value(view_duration)),
                ("header-height".to_string(), number_value(0.0)),
                (
                    "time-ruler".to_string(),
                    map_value_raw(vec![
                        ("mode", keyword_value("bars-beats")),
                        ("beats-per-bar", number_value(4.0)),
                    ]),
                ),
            ]);
            if let Some(density) = density {
                props.insert("grid-density".to_string(), number_value(density));
            }
            let view = TimelineView::from_props(
                &props,
                Rect {
                    row: 0.0,
                    col: 0.0,
                    width: 64.0,
                    height: 4.0,
                },
            );
            view.alignment_helper_grid_step()
        };

        for duration in [16.0, 32.0, 64.0, 128.0, 512.0] {
            let stock = step_for(None, duration);
            assert_eq!(
                step_for(Some(1.0), duration),
                stock,
                "density 1 is the stock ladder"
            );
            assert_eq!(
                step_for(Some(2.0), duration),
                stock / 2.0,
                "density 2 is exactly one rung finer at view {duration}"
            );
            // Every rung divides or multiplies the 4-beat bar, so bar lines
            // survive at any density: the denser step still lands on beat 4.
            let dense = step_for(Some(2.0), duration);
            let ratio = 4.0 / dense;
            assert!(
                (ratio - ratio.round()).abs() < 1e-9 || (dense / 4.0).fract() < 1e-9,
                "step {dense} must stay bar-aligned at view {duration}"
            );
        }
        // Clamped to something sane rather than trusted blindly.
        assert_eq!(step_for(Some(0.1), 64.0), step_for(None, 64.0));
    }

    /// docs/arrangement-region-editing-spec.md 3.2: `:width` on a dot is
    /// optional and lenient — absent/malformed means the legacy point dot.
    #[test]
    fn dots_parse_optional_width_leniently() {
        let content = |width: Option<Value>| {
            let mut dot = vec![
                ("offset", number_value(0.25)),
                ("value", number_value(0.5)),
            ];
            if let Some(width) = width {
                dot.push(("width", width));
            }
            map_value_raw(vec![("dots", list_value_raw(vec![map_value_raw(dot)]))])
        };
        let parsed_width = |width: Option<Value>| match parse_item_content(&content(width)) {
            Some(TimelineItemContent::Dots { dots, .. }) => dots[0].width,
            _ => panic!("expected dots content"),
        };
        assert_eq!(parsed_width(None), 0.0, "absent :width is a point dot");
        assert_eq!(parsed_width(Some(number_value(0.25))), 0.25);
        assert_eq!(parsed_width(Some(number_value(-1.0))), 0.0, "clamps low");
        assert_eq!(parsed_width(Some(number_value(4.0))), 1.0, "clamps high");
        assert_eq!(
            parsed_width(Some(Value::String("junk".to_string()))),
            0.0,
            "malformed degrades to a point dot"
        );
    }
}
