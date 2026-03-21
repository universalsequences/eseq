use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
#[cfg(target_os = "macos")]
use std::sync::Arc;

use crossterm::event::{KeyCode, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    styled_cell,
    time_view::{TimeRuler, TimeRulerMode, TimeViewport},
};
#[cfg(target_os = "macos")]
use super::{
    MetalGlyphRunPrimitive, MetalPrimitive, MetalQuadPrimitive, MetalRectPrimitive,
    MetalWaveformPrimitive,
};
use crate::audio::sample::{
    MinMaxPair as WaveformBucket, SampleBuffer, WaveformMipLevel as WaveformLevel,
    get_registered_sample,
};
use crate::layout::{Constraints, LayoutNode, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct WaveformWidget;

pub static WAVEFORM_WIDGET: WaveformWidget = WaveformWidget;

#[derive(Clone)]
struct WaveformBuffer {
    sample: Arc<SampleBuffer>,
}

#[derive(Clone)]
struct WaveformView {
    rect: Rect,
    header_height: f32,
    sidebar_width: f32,
    view_start: f64,
    view_duration: f64,
    zoom_min_duration: f64,
    zoom_max_duration: f64,
    time_ruler: Option<TimeRuler>,
    playhead_time: Option<f64>,
    cursor_time: Option<f64>,
    selection_start: Option<f64>,
    selection_end: Option<f64>,
    buffer: Option<WaveformBuffer>,
}

#[derive(Clone)]
enum HitRegion {
    Header,
    Content { time: f64 },
}

impl WidgetDefinition for WaveformWidget {
    fn names(&self) -> &'static [&'static str] {
        &["waveform"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "header-height", "sidebar-width"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
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
        let view = WaveformView::from_props(props, rect);
        let content = view.content_rect();
        if content.width == 0.0 || content.height == 0.0 {
            return;
        }

        for row_offset in 0..(rect.height.round() as u16) {
            let row = rect.row.round() as u16 + row_offset;
            for col_offset in 0..(rect.width.round() as u16) {
                let col = rect.col.round() as u16 + col_offset;
                buf.set(row, col, styled_cell(' ', theme::FG, Some(theme::BG)));
            }
        }

        if view.header_height > 0.0 {
            let header_row = rect.row.round() as u16;
            for col_offset in 0..(rect.width.round() as u16) {
                let col = rect.col.round() as u16 + col_offset;
                buf.set(
                    header_row,
                    col,
                    styled_cell(' ', theme::FG, Some(theme::STATUS_BG)),
                );
            }
            for (absolute_col, is_major) in view.grid_columns() {
                buf.set(
                    header_row,
                    absolute_col,
                    styled_cell(
                        '|',
                        if is_major {
                            theme::PURPLE
                        } else {
                            theme::BRIGHT_BLACK
                        },
                        Some(theme::STATUS_BG),
                    ),
                );
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
                        styled_cell(ch, theme::FG, Some(theme::STATUS_BG)),
                    );
                }
            }
        }

        for row_offset in 0..(content.height.round() as u16) {
            let row = content.row.round() as u16 + row_offset;
            for col_offset in 0..(content.width.round() as u16) {
                let col = content.col.round() as u16 + col_offset;
                buf.set(row, col, styled_cell(' ', theme::FG, Some(theme::BLACK)));
            }
        }

        for (col, is_major) in view.grid_columns() {
            for row_offset in 0..(content.height.round() as u16) {
                let row = content.row.round() as u16 + row_offset;
                buf.set(
                    row,
                    col,
                    styled_cell(
                        ':',
                        if is_major {
                            theme::FG_MUTED
                        } else {
                            crate::backend::Color::from_hex(0x4a, 0x4a, 0x50)
                        },
                        None,
                    ),
                );
            }
        }

        let content_left = content.col.round() as u16;
        let content_right = (content.col + content.width - 1.0).max(content.col).round() as u16;
        let content_top = content.row.round() as u16;
        let content_bottom = (content.row + content.height - 1.0)
            .max(content.row)
            .round() as u16;
        for col in content_left..=content_right {
            buf.set(content_top, col, styled_cell('-', theme::FG_MUTED, None));
            buf.set(content_bottom, col, styled_cell('-', theme::FG_MUTED, None));
        }
        for row in content_top..=content_bottom {
            buf.set(row, content_left, styled_cell('|', theme::FG_MUTED, None));
            buf.set(row, content_right, styled_cell('|', theme::FG_MUTED, None));
        }

        let zero_row =
            content.row.round() as u16 + ((content.height - 1.0).max(0.0) / 2.0).round() as u16;
        for col_offset in 0..(content.width.round() as u16) {
            let col = content.col.round() as u16 + col_offset;
            buf.set(
                zero_row,
                col,
                styled_cell('-', theme::WHITE, Some(theme::BLACK)),
            );
        }

        if let Some((selection_start, selection_end)) = view.selection_range() {
            let start_col = view.col_for_time(selection_start);
            let end_col = view.col_for_time(selection_end);
            for row_offset in 0..(content.height.round() as u16) {
                let row = content.row.round() as u16 + row_offset;
                for col in start_col.min(end_col)..=start_col.max(end_col) {
                    buf.set(
                        row,
                        col,
                        styled_cell(' ', theme::FG, Some(theme::STATUS_BG)),
                    );
                }
            }
        }

        if let Some(buffer) = &view.buffer {
            let level = view.best_level(buffer);
            let col_count = content.width.max(1.0).round() as usize;
            for offset in 0..col_count {
                let Some((top, bottom)) =
                    view.sample_column_range(buffer, level, offset, col_count)
                else {
                    continue;
                };
                let col = content.col.round() as u16 + offset as u16;
                for row in top..=bottom {
                    buf.set(row, col, styled_cell('|', theme::CYAN, None));
                }
            }
        }

        if let Some(cursor_col) = view.playhead_col(view.cursor_time) {
            for row_offset in 0..(content.height.round() as u16) {
                let row = content.row.round() as u16 + row_offset;
                buf.set(row, cursor_col, styled_cell('|', theme::YELLOW, None));
            }
        }
        if let Some(playhead_col) = view.playhead_col(view.playhead_time) {
            for row_offset in 0..(content.height.round() as u16) {
                let row = content.row.round() as u16 + row_offset;
                buf.set(row, playhead_col, styled_cell('|', theme::GREEN, None));
            }
        }
    }

    fn begin_gesture(&self, node: &LayoutNode, local_col: f32, local_row: f32) -> Option<Value> {
        let view = WaveformView::from_props(&node.props, node.rect);
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
    ) -> MouseEventOutcome {
        let view = WaveformView::from_props(&node.props, node.rect);
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
            MouseEventKind::Up(MouseButton::Left) => view
                .handle_pointer_up(local_col, local_row, gesture)
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
            _ => MouseEventOutcome::Consume,
        }
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let view = WaveformView::from_props(&node.props, node.rect);
        view.handle_key(key).map(WidgetEvent::Custom)
    }

    fn magnify_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
        delta: f64,
    ) -> Option<WidgetEvent> {
        let view = WaveformView::from_props(&node.props, node.rect);
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
        let view = WaveformView::from_props(&node.props, node.rect);
        view.handle_touchpad_scroll(local_col, local_row, delta_x, delta_y)
            .map(WidgetEvent::Custom)
    }

    fn captures_scroll_gesture(&self) -> bool {
        true
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

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        build_metal_primitives(node, viewport)
    }
}

#[cfg(target_os = "macos")]
fn build_metal_primitives(
    node: &LayoutNode,
    viewport: super::WidgetViewport,
) -> Vec<MetalPrimitive> {
    if node.widget_type != "waveform" {
        return Vec::new();
    }

    let rect = node.rect;
    let aspect = if viewport.cell_w > 0.0 {
        viewport.cell_h / viewport.cell_w
    } else {
        1.0
    };
    let view = WaveformView::from_props(&node.props, rect);
    let content = view.content_rect();
    if content.width == 0.0 || content.height == 0.0 {
        return Vec::new();
    }

    let mut primitives = Vec::new();
    primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
        rect,
        color: theme::BG,
    }));

    if view.header_height > 0.0 {
        primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
            rect: Rect {
                row: rect.row,
                col: rect.col,
                width: rect.width,
                height: view.header_height.min(rect.height),
            },
            color: theme::STATUS_BG,
        }));
        for (x, is_major) in view
            .time_viewport()
            .metal_grid_lines(view.time_ruler.as_ref())
        {
            primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
                x: x - 0.0625,
                y: rect.row,
                width: 0.125,
                height: view.header_height.min(rect.height),
                color: if is_major {
                    theme::PURPLE
                } else {
                    theme::BRIGHT_BLACK
                },
            }));
        }
        for (absolute_col, label) in view.time_ruler_labels() {
            primitives.push(MetalPrimitive::GlyphRun(MetalGlyphRunPrimitive {
                row: rect.row - 3.5,
                col: absolute_col as i32 + 1,
                text: label,
                fg: theme::FG,
                bg: theme::STATUS_BG,
            }));
        }
    }

    for (x, is_major) in view
        .time_viewport()
        .metal_grid_lines(view.time_ruler.as_ref())
    {
        primitives.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x: x - 0.0625,
            y: content.row,
            width: 0.125,
            height: content.height,
            color: if is_major {
                theme::FG_MUTED
            } else {
                crate::backend::Color::from_hex(0x4a, 0x4a, 0x50)
            },
        }));
    }

    if let Some(buffer) = &view.buffer {
        let selection = view.normalized_selection();
        let playhead = view.normalized_playhead();
        let level = view.best_level(buffer);
        primitives.push(MetalPrimitive::Waveform(MetalWaveformPrimitive {
            rect: Rect {
                col: content.col,
                row: content.row - 3.0,
                width: content.width,
                height: content.height / 2.5,
            },
            sample_key: buffer.sample.cache_key(),
            sample_start: (view.view_start / buffer.sample.duration_seconds).clamp(0.0, 1.0) as f32,
            sample_end: ((view.view_start + view.view_duration) / buffer.sample.duration_seconds)
                .clamp(0.0, 1.0) as f32,
            samples_per_bucket: level.samples_per_bucket as u32,
            bucket_count: level.buckets.len() as u32,
            selection_start: selection.map(|(start, _)| start).unwrap_or(0.0),
            selection_end: selection.map(|(_, end)| end).unwrap_or(0.0),
            playhead_position: playhead.unwrap_or(0.0),
            show_playhead: playhead.is_some(),
            waveform_color: theme::WHITE,
            selection_color: theme::YELLOW,
        }));
    }

    primitives
}

impl WaveformView {
    fn from_props(props: &HashMap<String, Value>, rect: Rect) -> Self {
        Self {
            rect,
            header_height: get_num(props, "header-height", 0.5).max(0.0) as f32,
            sidebar_width: get_num(props, "sidebar-width", 0.0).max(0.0) as f32,
            view_start: get_num(props, "view-start", 0.0).max(0.0),
            view_duration: get_num(props, "view-duration", 1.0).max(0.0001),
            zoom_min_duration: get_num(props, "zoom-min-duration", 0.001).max(0.0001),
            zoom_max_duration: get_num(props, "zoom-max-duration", 128.0).max(0.0001),
            time_ruler: props
                .get("time-ruler")
                .and_then(get_map)
                .and_then(|map| get_time_ruler(&map))
                .or(Some(TimeRuler {
                    mode: TimeRulerMode::Seconds,
                })),
            playhead_time: props.get("playhead-time").and_then(as_number),
            cursor_time: props.get("cursor-time").and_then(as_number),
            selection_start: props.get("selection-start").and_then(as_number),
            selection_end: props.get("selection-end").and_then(as_number),
            buffer: props.get("buffer").and_then(parse_waveform_buffer),
        }
    }

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

    fn content_rect(&self) -> Rect {
        self.time_viewport().content_rect()
    }

    fn grid_columns(&self) -> Vec<(u16, bool)> {
        self.time_viewport().grid_columns(self.time_ruler.as_ref())
    }

    fn time_ruler_labels(&self) -> Vec<(u16, String)> {
        self.time_viewport()
            .time_ruler_labels(self.time_ruler.as_ref())
    }

    fn col_for_time(&self, time: f64) -> u16 {
        self.time_viewport().col_for_time(time)
    }

    fn playhead_col(&self, time: Option<f64>) -> Option<u16> {
        self.time_viewport().playhead_col(time)
    }

    fn time_at_col(&self, local_col: f32) -> f64 {
        self.time_viewport().time_at_col(local_col)
    }

    fn best_level<'a>(&self, buffer: &'a WaveformBuffer) -> &'a WaveformLevel {
        let content_width = self.content_rect().width.max(1.0) as f64;
        let visible_frames = self.view_duration * buffer.sample.sample_rate as f64;
        let target_samples_per_pixel = (visible_frames / content_width).max(1.0);
        let preferred_max_bucket = (target_samples_per_pixel * 0.5).max(1.0);

        buffer
            .sample
            .levels()
            .iter()
            .take_while(|level| level.samples_per_bucket as f64 <= preferred_max_bucket)
            .last()
            .or_else(|| {
                buffer
                    .sample
                    .levels()
                    .iter()
                    .find(|level| level.samples_per_bucket as f64 >= target_samples_per_pixel)
            })
            .unwrap_or(&buffer.sample.levels()[0])
    }

    fn waveform_rows(&self, min: f32, max: f32) -> (u16, u16) {
        let content = self.content_rect();
        let top = content.row.round() as u16;
        let bottom = (content.row + content.height - 1.0)
            .max(content.row)
            .round() as u16;
        let half_height = (content.height - 1.0).max(1.0) / 2.0;
        let center = content.row + half_height;
        let max_row = (center - max.clamp(-1.0, 1.0) * half_height).round() as u16;
        let min_row = (center - min.clamp(-1.0, 1.0) * half_height).round() as u16;
        let mut row_a = max_row.clamp(top, bottom);
        let mut row_b = min_row.clamp(top, bottom);
        if row_a > row_b {
            std::mem::swap(&mut row_a, &mut row_b);
        }
        if row_a == row_b && row_b < bottom {
            row_b += 1;
        }
        (row_a, row_b)
    }

    fn selection_range(&self) -> Option<(f64, f64)> {
        Some((self.selection_start?, self.selection_end?))
    }

    fn hit_test(&self, local_col: f32, local_row: f32) -> Option<HitRegion> {
        if local_col < self.rect.col || local_row < self.rect.row {
            return None;
        }
        if local_row >= self.rect.row + self.rect.height
            || local_col >= self.rect.col + self.rect.width
        {
            return None;
        }
        if local_row < self.rect.row + self.header_height {
            return Some(HitRegion::Header);
        }
        Some(HitRegion::Content {
            time: self.time_at_col(local_col),
        })
    }

    fn begin_gesture(&self, local_col: f32, local_row: f32) -> Option<Value> {
        match self.hit_test(local_col, local_row)? {
            HitRegion::Header => Some(map_value(vec![("kind", keyword(":scrub"))])),
            HitRegion::Content { time } => Some(map_value(vec![
                ("kind", keyword(":select-range")),
                ("time", Value::Number(time)),
            ])),
        }
    }

    fn handle_pointer_down(&self, local_col: f32, local_row: f32) -> Option<Value> {
        match self.hit_test(local_col, local_row)? {
            HitRegion::Header | HitRegion::Content { .. } => Some(action_map(vec![
                ("type", keyword(":set-cursor")),
                ("time", Value::Number(self.time_at_col(local_col))),
            ])),
        }
    }

    fn handle_pointer_drag(
        &self,
        local_col: f32,
        local_row: f32,
        gesture: Option<&Value>,
    ) -> Option<Value> {
        self.hit_test(local_col, local_row)?;
        let gesture = get_map(gesture?)?;
        match gesture.get("kind") {
            Some(Value::Keyword(kind)) if kind == "select-range" => {
                let start = as_number(gesture.get("time")?)?;
                let end = self.time_at_col(local_col);
                Some(action_map(vec![
                    ("type", keyword(":set-selection")),
                    ("start", Value::Number(start.min(end))),
                    ("end", Value::Number(start.max(end))),
                ]))
            }
            Some(Value::Keyword(kind)) if kind == "scrub" => Some(action_map(vec![
                ("type", keyword(":set-cursor")),
                ("time", Value::Number(self.time_at_col(local_col))),
            ])),
            _ => None,
        }
    }

    fn handle_pointer_up(
        &self,
        _local_col: f32,
        _local_row: f32,
        _gesture: Option<&Value>,
    ) -> Option<Value> {
        None
    }

    fn handle_scroll(
        &self,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
    ) -> Option<Value> {
        let hit = self.hit_test(local_col, local_row)?;
        if matches!(hit, HitRegion::Header) {
            let factor = match mouse_kind {
                MouseEventKind::ScrollUp | MouseEventKind::ScrollRight => 1.05,
                MouseEventKind::ScrollDown | MouseEventKind::ScrollLeft => 1.0 / 1.05,
                _ => return None,
            };
            return self.zoom_action(self.time_at_col(local_col), factor);
        }

        let delta_time = match mouse_kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft => -self.scroll_time_step(),
            MouseEventKind::ScrollDown | MouseEventKind::ScrollRight => self.scroll_time_step(),
            _ => return None,
        };
        let next_view_start = self.clamp_view_start(self.view_start + delta_time);
        let applied = next_view_start - self.view_start;
        if applied.abs() < f64::EPSILON {
            return None;
        }
        Some(action_map(vec![
            ("type", keyword(":scroll-view")),
            ("delta-time", Value::Number(applied)),
        ]))
    }

    fn handle_magnify(&self, local_col: f32, local_row: f32, delta: f64) -> Option<Value> {
        self.hit_test(local_col, local_row)?;
        if delta.abs() < f64::EPSILON {
            return None;
        }
        self.zoom_action(
            self.time_at_col(local_col),
            2.0_f64.powf(delta.clamp(-1.0, 1.0)),
        )
    }

    fn handle_touchpad_scroll(
        &self,
        local_col: f32,
        local_row: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> Option<Value> {
        let hit = self.hit_test(local_col, local_row)?;
        if matches!(hit, HitRegion::Header) {
            let zoom_delta = if delta_y.abs() >= delta_x.abs() {
                delta_y as f64
            } else {
                delta_x as f64
            };
            if zoom_delta.abs() < f64::EPSILON {
                return None;
            }
            return self.zoom_action(
                self.time_at_col(local_col),
                2.0_f64.powf((zoom_delta / 240.0).clamp(-1.0, 1.0)),
            );
        }

        let content = self.content_rect();
        if content.width == 0.0 {
            return None;
        }
        let pan_delta = if delta_x.abs() >= delta_y.abs() {
            delta_x as f64
        } else {
            delta_y as f64
        };
        let delta_time = -(pan_delta / content.width.max(1.0) as f64) * self.view_duration * 0.0625;
        let next_view_start = self.clamp_view_start(self.view_start + delta_time);
        let applied = next_view_start - self.view_start;
        if applied.abs() < f64::EPSILON {
            return None;
        }
        Some(action_map(vec![
            ("type", keyword(":scroll-view")),
            ("delta-time", Value::Number(applied)),
        ]))
    }

    fn handle_key(&self, key: WidgetKeyEvent) -> Option<Value> {
        match key.code {
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.zoom_action(self.view_start + self.view_duration * 0.5, 1.1)
            }
            KeyCode::Char('-') => {
                self.zoom_action(self.view_start + self.view_duration * 0.5, 1.0 / 1.1)
            }
            KeyCode::Left => Some(action_map(vec![
                ("type", keyword(":scroll-view")),
                ("delta-time", Value::Number(-self.scroll_time_step())),
            ])),
            KeyCode::Right => Some(action_map(vec![
                ("type", keyword(":scroll-view")),
                ("delta-time", Value::Number(self.scroll_time_step())),
            ])),
            KeyCode::Esc if self.selection_range().is_some() => {
                Some(action_map(vec![("type", keyword(":clear-selection"))]))
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

    fn scroll_time_step(&self) -> f64 {
        (self.view_duration * 0.1).clamp(0.001, 10.0)
    }

    fn clamp_view_start(&self, next_start: f64) -> f64 {
        let max_start = self
            .buffer
            .as_ref()
            .map(|buffer| (buffer.sample.duration_seconds - self.view_duration).max(0.0))
            .unwrap_or(f64::INFINITY);
        next_start.max(0.0).min(max_start)
    }

    fn sample_column_range(
        &self,
        buffer: &WaveformBuffer,
        level: &WaveformLevel,
        offset: usize,
        col_count: usize,
    ) -> Option<(u16, u16)> {
        if level.buckets.is_empty() {
            return None;
        }
        let width = col_count.max(1) as f64;
        let col_center = (offset as f64 + 0.5) / width;
        let sample_time = self.view_start + self.view_duration * col_center;
        if sample_time < 0.0 || sample_time > buffer.sample.duration_seconds {
            return None;
        }
        let bucket_seconds = level.samples_per_bucket as f64 / buffer.sample.sample_rate as f64;
        if bucket_seconds <= 0.0 {
            return None;
        }
        let exact_index =
            (sample_time / bucket_seconds).clamp(0.0, (level.buckets.len() - 1) as f64);
        let index_a = exact_index.floor() as usize;
        let index_b = (index_a + 1).min(level.buckets.len() - 1);
        let frac = (exact_index - index_a as f64) as f32;
        let bucket_a = level.buckets[index_a];
        let bucket_b = level.buckets[index_b];
        let mut min = lerp(bucket_a.min, bucket_b.min, frac);
        let mut max = lerp(bucket_a.max, bucket_b.max, frac);
        if min > max {
            std::mem::swap(&mut min, &mut max);
        }
        if (max - min).abs() < 0.05 {
            let center = (min + max) * 0.5;
            min = (center - 0.025).max(-1.0);
            max = (center + 0.025).min(1.0);
        }
        Some(self.waveform_rows(min, max))
    }

    #[cfg(target_os = "macos")]
    fn normalized_selection(&self) -> Option<(f32, f32)> {
        let (start, end) = self.selection_range()?;
        let view_end = self.view_start + self.view_duration;
        if end < self.view_start || start > view_end {
            return None;
        }
        let norm =
            |time: f64| ((time - self.view_start) / self.view_duration).clamp(0.0, 1.0) as f32;
        Some((norm(start.min(end)), norm(start.max(end))))
    }

    #[cfg(target_os = "macos")]
    fn normalized_playhead(&self) -> Option<f32> {
        let time = self.playhead_time.or(self.cursor_time)?;
        if time < self.view_start || time > self.view_start + self.view_duration {
            return None;
        }
        Some(((time - self.view_start) / self.view_duration).clamp(0.0, 1.0) as f32)
    }
}

fn get_num(props: &HashMap<String, Value>, key: &str, default: f64) -> f64 {
    match props.get(key) {
        Some(Value::Number(n)) => *n,
        _ => default,
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

fn parse_waveform_buffer(value: &Value) -> Option<WaveformBuffer> {
    let map = get_map(value)?;
    for key_name in ["registry-key", "path", "id"] {
        if let Some(Value::String(key)) = map.get(key_name)
            && let Some(sample) = get_registered_sample(key)
        {
            return Some(WaveformBuffer { sample });
        }
    }
    let sample_rate = map.get("sample-rate").and_then(as_number)? as u32;
    let duration_seconds = map.get("duration").and_then(as_number)?;
    let levels = match map.get("peaks") {
        Some(Value::List(levels)) => levels
            .iter()
            .filter_map(|level| {
                let level = get_map(&level.borrow())?;
                let samples_per_bucket =
                    level.get("samples-per-bucket").and_then(as_number)? as usize;
                let buckets = match level.get("buckets") {
                    Some(Value::List(buckets)) => buckets
                        .iter()
                        .filter_map(|bucket| {
                            let bucket = get_map(&bucket.borrow())?;
                            Some(WaveformBucket {
                                min: bucket.get("min").and_then(as_number)? as f32,
                                max: bucket.get("max").and_then(as_number)? as f32,
                            })
                        })
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                Some(WaveformLevel {
                    samples_per_bucket,
                    buckets,
                })
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    if levels.is_empty() {
        return None;
    }
    Some(WaveformBuffer {
        sample: Arc::new(SampleBuffer {
            id: map
                .get("id")
                .and_then(|value| match value {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "waveform".to_string()),
            path: map
                .get("path")
                .and_then(|value| match value {
                    Value::String(s) => Some(std::path::PathBuf::from(s)),
                    _ => None,
                })
                .unwrap_or_else(|| std::path::PathBuf::from("waveform")),
            sample_rate,
            channels: map.get("channels").and_then(as_number).unwrap_or(1.0) as u16,
            frames: map.get("frames").and_then(as_number).unwrap_or(0.0) as usize,
            duration_seconds,
            peaks: levels,
        }),
    })
}

fn get_time_ruler(map: &HashMap<String, Value>) -> Option<TimeRuler> {
    let mode = match map.get("mode") {
        Some(Value::Keyword(mode)) | Some(Value::String(mode)) => mode.as_str(),
        _ => return None,
    };
    match mode {
        "seconds" => Some(TimeRuler {
            mode: TimeRulerMode::Seconds,
        }),
        _ => None,
    }
}

fn keyword(name: &str) -> Value {
    Value::Keyword(name.trim_start_matches(':').to_string())
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn action_map(entries: Vec<(&str, Value)>) -> Value {
    map_value(entries)
}

fn map_value(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget_render::CellBuffer;

    fn number_value(value: f64) -> Value {
        Value::Number(value)
    }

    fn keyword_value(name: &str) -> Value {
        Value::Keyword(name.to_string())
    }

    fn map_value_raw(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
                .collect(),
        )
    }

    fn list_value_raw(items: Vec<Value>) -> Value {
        Value::List(
            items
                .into_iter()
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        )
    }

    fn sample_buffer_value() -> Value {
        map_value_raw(vec![
            ("sample-rate", number_value(44_100.0)),
            ("duration", number_value(1.0)),
            (
                "peaks",
                list_value_raw(vec![map_value_raw(vec![
                    ("samples-per-bucket", number_value(64.0)),
                    (
                        "buckets",
                        list_value_raw(vec![
                            map_value_raw(vec![
                                ("min", number_value(-0.5)),
                                ("max", number_value(0.5)),
                            ]),
                            map_value_raw(vec![
                                ("min", number_value(-0.25)),
                                ("max", number_value(0.75)),
                            ]),
                        ]),
                    ),
                ])]),
            ),
        ])
    }

    #[test]
    fn header_scroll_emits_zoom_action() {
        let props = HashMap::from([
            ("buffer".to_string(), sample_buffer_value()),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(1.0)),
        ]);
        let view = WaveformView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let Value::Map(map) = view
            .handle_scroll(MouseEventKind::ScrollUp, 10.0, 0.0)
            .expect("zoom action")
        else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("zoom-view".to_string()))
        );
    }

    #[test]
    fn drag_emits_selection_range() {
        let props = HashMap::from([
            ("buffer".to_string(), sample_buffer_value()),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(1.0)),
            (
                "time-ruler".to_string(),
                map_value_raw(vec![("mode", keyword_value("seconds"))]),
            ),
        ]);
        let view = WaveformView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 32.0,
                height: 8.0,
            },
        );
        let gesture = view.begin_gesture(4.0, 2.0).expect("gesture");
        let Value::Map(map) = view
            .handle_pointer_drag(20.0, 2.0, Some(&gesture))
            .expect("selection action")
        else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("set-selection".to_string()))
        );
    }

    #[test]
    fn tui_render_draws_visible_waveform_cells() {
        let props = HashMap::from([
            ("buffer".to_string(), sample_buffer_value()),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(1.0)),
        ]);
        let mut buf = CellBuffer::new(40, 12);
        WAVEFORM_WIDGET.tui_render(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 40.0,
                height: 12.0,
            },
            &mut buf,
        );
        let has_waveform = buf
            .cells
            .iter()
            .flatten()
            .flatten()
            .any(|cell| cell.ch != ' ');
        assert!(
            has_waveform,
            "expected waveform stroke cells to be rendered"
        );
    }
}
