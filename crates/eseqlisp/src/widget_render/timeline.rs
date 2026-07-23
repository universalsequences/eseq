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
    scroll_viewport_height: Option<f32>,
    snap: f64,
    resize_snap: f64,
    resize_snap_to_grid: bool,
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
}

#[derive(Clone)]
enum HitRegion {
    Header,
    ContentLengthEnd,
    Sidebar { lane: usize },
    Background { time: f64 },
    ItemBody { item: TimelineItem },
    ItemEdgeEnd { item: TimelineItem },
}

thread_local! {
    static TIMELINE_HOVER_EDGE: RefCell<Option<(u64, Value)>> = const { RefCell::new(None) };
}

fn set_timeline_hover_edge(widget_id: u64, item_id: Option<Value>) {
    TIMELINE_HOVER_EDGE.with(|state| {
        let mut state = state.borrow_mut();
        let next = item_id.map(|id| (widget_id, id));
        if *state != next {
            *state = next;
            super::bump_widget_state_generation();
        }
    });
}

fn timeline_hover_edge_matches(widget_id: u64, item_id: &Value) -> bool {
    TIMELINE_HOVER_EDGE.with(|state| {
        state
            .borrow()
            .as_ref()
            .is_some_and(|(hover_widget_id, hover_item_id)| {
                *hover_widget_id == widget_id && hover_item_id == item_id
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
                let hovered_item = match view.hit_test(local_col, local_row) {
                    Some(HitRegion::ItemEdgeEnd { item }) => Some(item.id),
                    _ => None,
                };
                set_timeline_hover_edge(node.widget_id, hovered_item);
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
            Some(HitRegion::ItemEdgeEnd { .. }) | Some(HitRegion::ContentLengthEnd) => {
                super::WidgetCursor::EwResize
            }
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

    if let Some((x, y, width, height)) = view.metal_selection_rect() {
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

    let mut item_rects = Vec::new();
    for item in &view.items {
        let Some((x, y, width, height)) = view.metal_item_rect(item) else {
            continue;
        };
        let item_color = item.color.unwrap_or(view.item_color);
        primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x,
            y,
            width,
            height,
            color: item_color,
        }));
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
        if let Some(label) = &item.label {
            if width >= 3.0 && height >= 0.85 {
                primitives.push(MetalPrimitive::ProportionalText(
                    MetalProportionalTextPrimitive {
                        row: y + ((height - 0.80).max(0.0) * 0.5) - 0.02,
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
        push_item_content_primitives(&mut primitives, &view, item, (x, y, width, height), viewport);
        item_rects.push((
            x,
            y,
            width,
            height,
            view.item_selected(item),
            timeline_hover_edge_matches(node.widget_id, &item.id),
        ));
    }

    let item_border_color = crate::backend::Color {
        r: 0.02,
        g: 0.025,
        b: 0.03,
        a: 0.72,
    };
    let selected_border_color = crate::backend::Color::from_hex(0xb9, 0xee, 0xff);
    for (x, y, width, height, selected, resize_hovered) in item_rects {
        let thickness = if selected { 0.16_f32 } else { 0.08_f32 }
            .min(width * 0.5)
            .min(height * 0.5);
        if thickness <= 0.0 {
            continue;
        }
        let border_color = if selected {
            selected_border_color
        } else {
            item_border_color
        };
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
        if resize_hovered {
            let hover_width = 0.22_f32.min(width.max(0.0));
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: x + width - hover_width,
                y,
                width: hover_width,
                height,
                color: crate::backend::Color {
                    r: 0.74,
                    g: 0.94,
                    b: 1.0,
                    a: 0.95,
                },
            }));
        }
    }

    primitives
}

/// Minimum on-screen item width (px) below which item content is skipped
/// entirely — narrower than this the dots/bars would only alias
/// (docs/arrangement-timeline-ui-spec.md 7.3).
#[cfg(target_os = "macos")]
const ITEM_CONTENT_MIN_WIDTH_PX: f32 = 14.0;

/// Draw an item's `content` payload as additional quads clipped to the item's
/// on-screen rect (docs/arrangement-timeline-ui-spec.md 7.3). `rect` is the
/// already view-clipped rect from `metal_item_rect`; dot x positions come
/// from the item's unclipped time span so partially visible items keep their
/// content aligned.
#[cfg(target_os = "macos")]
fn push_item_content_primitives(
    primitives: &mut Vec<MetalPrimitive>,
    view: &TimelineView,
    item: &TimelineItem,
    rect: (f32, f32, f32, f32),
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
                for index in 1..cycles {
                    let time = item.start + span * cycle * index as f64;
                    if time < view.view_start || time >= view_end || time >= item.end {
                        continue;
                    }
                    let line_x = view.x_for_time(time);
                    primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                        x: line_x - 0.0625,
                        y,
                        width: 0.125,
                        height,
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
                    primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                        x: dot_x,
                        y: dot_y,
                        width: dot_width,
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
        for item in self.items.iter().rev() {
            let Some(rect) = self.item_rect(item) else {
                continue;
            };
            if local_row < rect.row || local_row >= rect.row + rect.height {
                continue;
            }

            let left = rect.col;
            let right = rect.col + rect.width;
            let handle_width = (rect.width * 0.24).clamp(1.25, 4.0);
            let outside_slop = 0.75;

            if rect.width > 1.0
                && local_col >= right - handle_width
                && local_col <= right + outside_slop
            {
                return Some(HitRegion::ItemEdgeEnd { item: item.clone() });
            }
            if local_col >= left && local_col < right {
                return Some(HitRegion::ItemBody { item: item.clone() });
            }
        }
        Some(HitRegion::Background {
            time: self.time_at_col(local_col),
        })
    }

    fn begin_gesture(&self, local_col: f32, local_row: f32) -> Option<Value> {
        let hit = self.hit_test(local_col, local_row)?;
        let current_time = self.snap_time(self.time_at_col(local_col));
        let current_marquee_time = self.time_at_col(local_col);
        let current_lane = self.lane_at_row(local_row);
        match self.tool {
            TimelineTool::Pointer => match hit {
                HitRegion::ItemBody { item } => {
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
                HitRegion::ContentLengthEnd => {
                    Some(map_value(vec![("kind", keyword(":resize-content-length"))]))
                }
                HitRegion::Background { .. } => Some(map_value(vec![
                    ("kind", keyword(":marquee")),
                    ("time", Value::Number(current_marquee_time)),
                    ("lane", Value::Number(current_lane as f64)),
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
                HitRegion::ItemBody { item } | HitRegion::ItemEdgeEnd { item } => {
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
                HitRegion::ItemBody { item } | HitRegion::ItemEdgeEnd { item } => {
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
        let current_hit = self.hit_test(local_col, local_row)?;
        let raw_time = self.time_at_col(local_col);
        let current_time = self.snap_time(raw_time);
        let current_resize_time = self.snap_resize_time(raw_time);
        let current_marquee_time = raw_time;
        let current_lane = self.lane_at_row(local_row);
        let gesture_value = gesture?;
        let gesture = get_map(gesture_value)?;
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
                if (start_time - current_marquee_time).abs() < f64::EPSILON
                    && start_lane == current_lane
                {
                    return Some(action_map(vec![("type", keyword(":clear-selection"))]));
                }
                let lane_a = start_lane.min(current_lane);
                let lane_b = start_lane.max(current_lane);
                Some(action_map(vec![
                    ("type", keyword(":marquee-select")),
                    (
                        "time-a",
                        Value::Number(start_time.min(current_marquee_time)),
                    ),
                    (
                        "time-b",
                        Value::Number(start_time.max(current_marquee_time)),
                    ),
                    ("lane-a", Value::Number(lane_a as f64)),
                    ("lane-b", Value::Number(lane_b as f64)),
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
            _ => match current_hit {
                HitRegion::ItemBody { item } | HitRegion::ItemEdgeEnd { item } => {
                    Some(action_map(vec![
                        ("type", keyword(":delete-items")),
                        ("ids", list_value(vec![item.id])),
                    ]))
                }
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
                if (start_time - current_marquee_time).abs() < f64::EPSILON
                    && start_lane == current_lane
                {
                    return Some(action_map(vec![("type", keyword(":clear-selection"))]));
                }
                let lane_a = start_lane.min(current_lane);
                let lane_b = start_lane.max(current_lane);
                Some(action_map(vec![
                    ("type", keyword(":finish-marquee-select")),
                    (
                        "time-a",
                        Value::Number(start_time.min(current_marquee_time)),
                    ),
                    (
                        "time-b",
                        Value::Number(start_time.max(current_marquee_time)),
                    ),
                    ("lane-a", Value::Number(lane_a as f64)),
                    ("lane-b", Value::Number(lane_b as f64)),
                    ("mode", keyword(":replace")),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "move" => Some(action_map(vec![
                ("type", keyword(":finish-move-items")),
                ("ids", gesture.get("ids")?.clone()),
                ("anchor-id", gesture.get("anchor-id")?.clone()),
            ])),
            Some(Value::Keyword(kind)) if kind == "resize-end" => Some(action_map(vec![
                ("type", keyword(":finish-resize-items")),
                ("ids", gesture.get("ids")?.clone()),
                ("id", gesture.get("id")?.clone()),
            ])),
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
            KeyCode::Esc if !selected_ids.is_empty() => {
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
}
