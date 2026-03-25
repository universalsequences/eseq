use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MetalGlyphRunPrimitive, MetalPrimitive, MetalQuadPrimitive,
    MetalRectPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    resolve_named_color, styled_cell,
    time_view::{TimeRuler, TimeRulerMode, TimeViewport},
};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

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
    color: Option<crate::backend::Color>,
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

#[derive(Clone)]
struct TimelineView {
    rect: Rect,
    header_height: f32,
    sidebar_width: f32,
    view_start: f64,
    view_duration: f64,
    zoom_min_duration: f64,
    zoom_max_duration: f64,
    time_ruler: Option<TimeRuler>,
    playhead_time: Option<f64>,
    lane_scroll: f64,
    snap: f64,
    tool: TimelineTool,
    lanes: Vec<TimelineLane>,
    items: Vec<TimelineItem>,
    selection: Vec<Value>,
}

#[derive(Clone)]
enum HitRegion {
    Header,
    Sidebar { lane: usize },
    Background { time: f64 },
    ItemBody { item: TimelineItem },
    ItemEdgeStart { item: TimelineItem },
    ItemEdgeEnd { item: TimelineItem },
}

impl WidgetDefinition for TimelineWidget {
    fn names(&self) -> &'static [&'static str] {
        &["timeline"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "header-height", "sidebar-width"]
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
            let item_color = if item.selected {
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

    fn begin_gesture(&self, node: &LayoutNode, local_col: f32, local_row: f32) -> Option<Value> {
        let view = TimelineView::from_props(&node.props, node.rect);
        view.begin_gesture(local_col, local_row)
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
    ) -> MouseEventOutcome {
        let view = TimelineView::from_props(&node.props, node.rect);
        match mouse_kind {
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
        view.handle_double_click(local_col, local_row)
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
        view.handle_magnify(local_col, local_row, delta)
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
        view.handle_touchpad_scroll(local_col, local_row, delta_x, delta_y)
            .map(WidgetEvent::Custom)
    }

    fn captures_scroll_gesture(&self) -> bool {
        true
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
    let aspect = if viewport.cell_w > 0.0 {
        viewport.cell_h / viewport.cell_w
    } else {
        1.0
    };
    let view = TimelineView::from_props(&node.props, rect);
    let content = view.content_rect();
    let mut primitives = Vec::new();

    if view.header_height > 0.0 {
        primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
            rect: Rect {
                row: rect.row,
                col: rect.col,
                width: rect.width,
                height: 1.0,
            },
            color: theme::STATUS_BG(),
        }));
        for (x, _) in view.metal_grid_lines() {
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: x - 0.0625,
                y: rect.row,
                width: 0.125,
                height: 1.0,
                color: theme::BRIGHT_BLACK(),
            }));
        }
        for (absolute_col, label) in view.time_ruler_labels() {
            primitives.push(MetalPrimitive::GlyphRun(MetalGlyphRunPrimitive {
                row: rect.row / aspect,
                col: absolute_col as i32 + 1,
                text: label,
                fg: theme::FG_MUTED(),
                bg: theme::STATUS_BG(),
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
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: rect.col,
                y: row_start,
                width: view.sidebar_width,
                height: lane_height,
                color: sidebar_bg,
            }));
            // Lane label text
            let label = lane.label.as_deref().unwrap_or("");
            if !label.is_empty() {
                let label_fg = lane.label_fg.unwrap_or(theme::FG());
                primitives.push(MetalPrimitive::GlyphRun(MetalGlyphRunPrimitive {
                    row: row_start / aspect,
                    col: rect.col.round() as i32,
                    text: label.chars().take(view.sidebar_width as usize).collect(),
                    fg: label_fg,
                    bg: sidebar_bg,
                }));
            }
            let divider_col = rect.col + (view.sidebar_width - 1.0).max(0.0);
            if divider_col < rect.col + rect.width {
                primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                    x: divider_col + 0.4375,
                    y: row_start,
                    width: 0.125,
                    height: lane_height,
                    color: theme::BRIGHT_BLACK(),
                }));
            }
        }
        primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x: content.col,
            y: row_start,
            width: content.width,
            height: lane_height,
            color: if lane_index % 2 == 0 {
                theme::BLACK()
            } else {
                theme::BG()
            },
        }));
    }

    for (x, is_major) in view.metal_grid_lines() {
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
                height: 1.0,
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

    for item in &view.items {
        let Some((x, y, width, height)) = view.metal_item_rect(item) else {
            continue;
        };
        let item_color = if item.selected {
            theme::PURPLE()
        } else {
            item.color.unwrap_or(theme::WHITE())
        };
        primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x,
            y,
            width,
            height,
            color: item_color,
        }));
    }

    primitives
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
        Self {
            rect,
            header_height: get_num(props, "header-height", 1.0).max(0.0) as f32,
            sidebar_width: get_num(props, "sidebar-width", 0.0).max(0.0) as f32,
            view_start: get_num(props, "view-start", 0.0),
            view_duration: get_num(props, "view-duration", 16.0).max(0.0001),
            zoom_min_duration: get_num(props, "zoom-min-duration", 8.0).max(0.0001),
            zoom_max_duration: get_num(props, "zoom-max-duration", 128.0).max(0.0001),
            time_ruler: props
                .get("time-ruler")
                .and_then(get_map)
                .and_then(|map| get_time_ruler(&map)),
            playhead_time: props.get("playhead-time").and_then(as_number),
            lane_scroll: get_num(props, "lane-scroll", 0.0).max(0.0),
            snap: get_num(props, "snap", 0.0).max(0.0),
            tool: get_tool(props),
            lanes: get_lanes(props),
            items: get_items(props),
            selection: get_selection(props),
        }
    }

    fn content_rect(&self) -> Rect {
        self.time_viewport().content_rect()
    }

    fn visible_lane_rows(&self, lane_index: usize) -> Option<(u16, u16)> {
        let content = self.content_rect();
        if content.height == 0.0 {
            return None;
        }
        let lane_scroll = self.lane_scroll.floor() as usize;
        if lane_index < lane_scroll {
            return None;
        }
        let visible_index = lane_index - lane_scroll;
        let lane_count = self.visible_lane_capacity().max(1);
        if visible_index >= lane_count {
            return None;
        }
        let lane_height = (content.height / lane_count as f32).max(1.0);
        let row_start = content.row + visible_index as f32 * lane_height;
        let row_end = if visible_index + 1 == lane_count {
            content.row + content.height
        } else {
            row_start + lane_height
        };
        Some((
            row_start.round() as u16,
            row_end.max(row_start + 1.0).round() as u16,
        ))
    }

    fn visible_lane_capacity(&self) -> usize {
        let content = self.content_rect();
        let count = content.height.max(1.0) as usize;
        count.min(self.lanes.len().max(1))
    }

    fn lane_at_row(&self, local_row: f32) -> usize {
        let content = self.content_rect();
        if content.height == 0.0 {
            return 0;
        }
        let lane_height = (content.height / self.visible_lane_capacity().max(1) as f32).max(1.0);
        let relative = ((local_row - content.row) / lane_height).floor().max(0.0) as usize;
        (self.lane_scroll.floor() as usize + relative).min(self.lanes.len().saturating_sub(1))
    }

    fn time_at_col(&self, local_col: f32) -> f64 {
        self.time_viewport().time_at_col(local_col)
    }

    fn snap_time(&self, time: f64) -> f64 {
        if self.snap <= 0.0 {
            time
        } else {
            (time / self.snap).round() * self.snap
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

    fn edge_for_time(&self, time: f64) -> f32 {
        self.time_viewport().edge_for_time(time)
    }

    fn x_for_time(&self, time: f64) -> f32 {
        self.time_viewport().x_for_time(time)
    }

    fn metal_playhead_x(&self) -> Option<f32> {
        self.time_viewport().metal_playhead_x(self.playhead_time)
    }

    fn metal_lane_rect(&self, lane_index: usize) -> Option<(f32, f32)> {
        let content = self.content_rect();
        if content.height == 0.0 {
            return None;
        }
        let lane_count = self.visible_lane_capacity().max(1) as f32;
        let lane_height = content.height / lane_count;
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

    fn item_rect(&self, item: &TimelineItem) -> Option<Rect> {
        let content = self.content_rect();
        let (row_start, row_end) = self.visible_lane_rows(item.lane)?;
        let view_end = self.view_start + self.view_duration;
        if item.end <= self.view_start || item.start >= view_end {
            return None;
        }
        let start_edge = self.edge_for_time(item.start);
        let end_edge = self.edge_for_time(item.end);
        let start_col = content.col + start_edge.min((content.width - 1.0).max(0.0));
        let width = (end_edge - start_edge).max(1.0);
        Some(Rect {
            row: row_start as f32,
            col: start_col.min(content.col + (content.width - 1.0).max(0.0)),
            width: width.min(content.width),
            height: ((row_end as f32) - (row_start as f32)).max(1.0),
        })
    }

    fn hit_test(&self, local_col: f32, local_row: f32) -> Option<HitRegion> {
        if local_col < self.rect.col || local_row < self.rect.row {
            return None;
        }
        if local_row < self.rect.row + self.header_height {
            return Some(HitRegion::Header);
        }
        let lane = self.lane_at_row(local_row);
        if local_col < self.rect.col + self.sidebar_width {
            return Some(HitRegion::Sidebar { lane });
        }
        for item in self.items.iter().rev() {
            let Some(rect) = self.item_rect(item) else {
                continue;
            };
            if local_row >= rect.row
                && local_row < rect.row + rect.height
                && local_col >= rect.col
                && local_col < rect.col + rect.width
            {
                if rect.width <= 1.0 {
                    return Some(HitRegion::ItemBody { item: item.clone() });
                }
                let edge_threshold = (rect.width * 0.2).floor().clamp(1.0, 2.0);
                if local_col <= rect.col + edge_threshold - 1.0 {
                    return Some(HitRegion::ItemEdgeStart { item: item.clone() });
                }
                if local_col >= rect.col + rect.width - edge_threshold {
                    return Some(HitRegion::ItemEdgeEnd { item: item.clone() });
                }
                return Some(HitRegion::ItemBody { item: item.clone() });
            }
        }
        Some(HitRegion::Background {
            time: self.snap_time(self.time_at_col(local_col)),
        })
    }

    fn begin_gesture(&self, local_col: f32, local_row: f32) -> Option<Value> {
        let hit = self.hit_test(local_col, local_row)?;
        let current_time = self.snap_time(self.time_at_col(local_col));
        let current_lane = self.lane_at_row(local_row);
        match self.tool {
            TimelineTool::Pointer => match hit {
                HitRegion::ItemBody { item } => {
                    let ids = if item.selected {
                        self.selected_ids_for(item.id.clone())
                    } else {
                        vec![item.id.clone()]
                    };
                    Some(map_value(vec![
                        ("kind", keyword(":move")),
                        ("ids", list_value(ids)),
                        ("anchor-id", item.id),
                        ("time-offset", Value::Number(current_time - item.start)),
                        (
                            "lane-offset",
                            Value::Number(current_lane as f64 - item.lane as f64),
                        ),
                    ]))
                }
                HitRegion::ItemEdgeStart { item } => Some(map_value(vec![
                    ("kind", keyword(":resize-start")),
                    ("id", item.id),
                ])),
                HitRegion::ItemEdgeEnd { item } => Some(map_value(vec![
                    ("kind", keyword(":resize-end")),
                    ("id", item.id),
                ])),
                HitRegion::Background { time } => Some(map_value(vec![
                    ("kind", keyword(":marquee")),
                    ("time", Value::Number(time)),
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
                ("time", Value::Number(current_time)),
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
                HitRegion::ItemBody { item }
                | HitRegion::ItemEdgeStart { item }
                | HitRegion::ItemEdgeEnd { item } => Some(action_map(vec![
                    ("type", keyword(":select")),
                    ("ids", list_value(vec![item.id])),
                    ("mode", keyword(":replace")),
                ])),
                HitRegion::Background { time, .. } => Some(action_map(vec![
                    ("type", keyword(":clear-selection")),
                    ("time", Value::Number(time)),
                ])),
                HitRegion::Header => Some(action_map(vec![
                    ("type", keyword(":set-cursor")),
                    (
                        "time",
                        Value::Number(self.snap_time(self.time_at_col(local_col))),
                    ),
                ])),
                HitRegion::Sidebar { lane } => Some(action_map(vec![
                    ("type", keyword(":set-cursor")),
                    ("lane", Value::Number(lane as f64)),
                ])),
            },
            TimelineTool::Erase => match hit {
                HitRegion::ItemBody { item }
                | HitRegion::ItemEdgeStart { item }
                | HitRegion::ItemEdgeEnd { item } => Some(action_map(vec![
                    ("type", keyword(":delete-items")),
                    ("ids", list_value(vec![item.id])),
                ])),
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
            HitRegion::Background { time } => {
                let start = self.snap_time(time);
                let default_duration = self.snap.max(1.0);
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
        let current_time = self.snap_time(self.time_at_col(local_col));
        let current_lane = self.lane_at_row(local_row);
        let gesture = get_map(gesture?)?;
        match gesture.get("kind") {
            Some(Value::Keyword(kind)) if kind == "move" => {
                let anchor_id = gesture.get("anchor-id")?.clone();
                let unclamped_start =
                    self.snap_time(current_time - as_number(gesture.get("time-offset")?)?);
                let anchor_start = self
                    .items
                    .iter()
                    .find(|item| item.id == anchor_id)
                    .map(|item| item.start)
                    .unwrap_or(0.0);
                let min_start = self
                    .min_selected_start_from_value(gesture.get("ids")?)
                    .unwrap_or(anchor_start);
                let clamped_start = unclamped_start.max(anchor_start - min_start).max(0.0);
                Some(action_map(vec![
                    ("type", keyword(":move-items-absolute")),
                    ("ids", gesture.get("ids")?.clone()),
                    ("anchor-id", anchor_id),
                    ("start", Value::Number(clamped_start)),
                    (
                        "lane",
                        Value::Number(
                            (current_lane as f64 - as_number(gesture.get("lane-offset")?)?)
                                .round()
                                .max(0.0),
                        ),
                    ),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "resize-start" => {
                let id = gesture.get("id")?.clone();
                let item = self.items.iter().find(|item| item.id == id)?;
                Some(action_map(vec![
                    ("type", keyword(":resize-item-absolute")),
                    ("id", id),
                    ("edge", keyword(":start")),
                    ("time", Value::Number(current_time.clamp(0.0, item.end))),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "resize-end" => {
                let id = gesture.get("id")?.clone();
                let item = self.items.iter().find(|item| item.id == id)?;
                Some(action_map(vec![
                    ("type", keyword(":resize-item-absolute")),
                    ("id", id),
                    ("edge", keyword(":end")),
                    ("time", Value::Number(current_time.max(item.start))),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "marquee" => {
                let start_time = as_number(gesture.get("time")?)?;
                let start_lane = as_number(gesture.get("lane")?)? as usize;
                let lane_a = start_lane.min(current_lane);
                let lane_b = start_lane.max(current_lane);
                Some(action_map(vec![
                    ("type", keyword(":marquee-select")),
                    ("time-a", Value::Number(start_time.min(current_time))),
                    ("time-b", Value::Number(start_time.max(current_time))),
                    ("lane-a", Value::Number(lane_a as f64)),
                    ("lane-b", Value::Number(lane_b as f64)),
                    ("mode", keyword(":replace")),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "draw" => {
                let start_time = as_number(gesture.get("time")?)?;
                let start_lane = as_number(gesture.get("lane")?)?;
                Some(action_map(vec![
                    ("type", keyword(":create-item")),
                    ("lane", Value::Number(start_lane)),
                    ("start", Value::Number(start_time.min(current_time))),
                    ("end", Value::Number(start_time.max(current_time))),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "pan" => {
                let start_time = as_number(gesture.get("time")?)?;
                let start_lane = as_number(gesture.get("lane")?)?;
                Some(action_map(vec![
                    ("type", keyword(":scroll-view")),
                    ("delta-time", Value::Number(start_time - current_time)),
                    (
                        "delta-lanes",
                        Value::Number(start_lane - current_lane as f64),
                    ),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "scrub" => Some(action_map(vec![
                ("type", keyword(":set-cursor")),
                ("time", Value::Number(current_time)),
            ])),
            _ => match current_hit {
                HitRegion::ItemBody { item }
                | HitRegion::ItemEdgeStart { item }
                | HitRegion::ItemEdgeEnd { item } => Some(action_map(vec![
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
        match mouse_kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta_lanes = match mouse_kind {
                    MouseEventKind::ScrollUp => -1.0,
                    MouseEventKind::ScrollDown => 1.0,
                    _ => unreachable!(),
                };
                let next_lane_scroll = (self.lane_scroll + delta_lanes).max(0.0);
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
                ]))
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                let delta_time = match mouse_kind {
                    MouseEventKind::ScrollLeft => -time_step,
                    MouseEventKind::ScrollRight => time_step,
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
                    ("delta-lanes", Value::Number(0.0)),
                ]))
            }
            _ => None,
        }
    }

    fn handle_magnify(&self, local_col: f32, local_row: f32, delta: f64) -> Option<Value> {
        self.hit_test(local_col, local_row)?;
        if delta.abs() < f64::EPSILON {
            return None;
        }
        let factor = 2.0_f64.powf(delta.clamp(-1.0, 1.0));
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

        let lane_capacity = self.visible_lane_capacity().max(1) as f32;
        let lane_height = content.height / lane_capacity;
        let delta_time =
            -(delta_x as f64 / content.width.max(1.0) as f64) * self.view_duration * 0.0625;
        let delta_lanes = if lane_height > 0.0 {
            -(delta_y / lane_height) as f64 * 0.0625
        } else {
            0.0
        };

        let next_view_start = (self.view_start + delta_time).max(0.0);
        let next_lane_scroll = (self.lane_scroll + delta_lanes).max(0.0);
        let applied_time = next_view_start - self.view_start;
        let applied_lanes = next_lane_scroll - self.lane_scroll;
        if applied_time.abs() < f64::EPSILON && applied_lanes.abs() < f64::EPSILON {
            return None;
        }
        Some(action_map(vec![
            ("type", keyword(":scroll-view")),
            ("delta-time", Value::Number(applied_time)),
            ("delta-lanes", Value::Number(applied_lanes)),
        ]))
    }

    fn handle_pointer_up(
        &self,
        local_col: f32,
        _local_row: f32,
        gesture: Option<&Value>,
    ) -> Option<Value> {
        let current_time = self.snap_time(self.time_at_col(local_col));
        let gesture = get_map(gesture?)?;
        match gesture.get("kind") {
            Some(Value::Keyword(kind)) if kind == "draw" => {
                let start_time = as_number(gesture.get("time")?)?;
                let start_lane = as_number(gesture.get("lane")?)?;
                Some(action_map(vec![
                    ("type", keyword(":finish-create-item")),
                    ("lane", Value::Number(start_lane)),
                    ("start", Value::Number(start_time.min(current_time))),
                    ("end", Value::Number(start_time.max(current_time))),
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
                color: map.get("color").map(|value| {
                    resolve_named_color(
                        &HashMap::from([("color".to_string(), value.clone())]),
                        "color",
                        theme::WIDGET_SLIDER_FILLED(),
                    )
                }),
            })
        })
        .collect()
}

fn get_selection(props: &HashMap<String, Value>) -> Vec<Value> {
    let Some(Value::List(items)) = props.get("selection") else {
        return vec![];
    };
    items.iter().map(|item| item.borrow().clone()).collect()
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
        _ => None,
    }
}

fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        _ => None,
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
    fn touchpad_scroll_emits_fractional_scroll_view_action() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(4.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("lane-scroll".to_string(), number_value(8.0)),
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
            (
                "lanes".to_string(),
                list_value_raw(vec![
                    map_value_raw(vec![("id", number_value(0.0))]),
                    map_value_raw(vec![("id", number_value(1.0))]),
                    map_value_raw(vec![("id", number_value(2.0))]),
                    map_value_raw(vec![("id", number_value(3.0))]),
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
    fn content_scroll_down_emits_lane_scroll_action() {
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
