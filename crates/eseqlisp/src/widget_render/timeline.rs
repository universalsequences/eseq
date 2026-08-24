use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, GpuPrimitive, GpuProportionalTextPrimitive, GpuQuadPrimitive,
    GpuRectPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetInstance,
    WidgetKeyEvent, WidgetViewport, ndc_bounds, resolve_named_color, styled_cell,
    time_view::{TimeRuler, TimeRulerMode, TimeViewport},
};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

const ALIGNMENT_HELPER_BACKWARD_SNAP_PROXIMITY: f64 = 0.25;

pub struct TimelineWidget;

pub static TIMELINE_WIDGET: TimelineWidget = TimelineWidget;

pub(super) struct TimelineCursorMarkerWidget;

pub(super) static TIMELINE_CURSOR_MARKER_WIDGET: TimelineCursorMarkerWidget =
    TimelineCursorMarkerWidget;

const TIMELINE_CURSOR_MARKER_FRAGMENT_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 instance_size_px = in.uniform_a.xy;
    float2 marker_size_px = in.uniform_a.zw;
    float padding_px = in.uniform_b.x;

    // Work in physical pixels so the edge transition remains one pixel wide
    // regardless of cell size or display scale.
    float2 p = in.uv * instance_size_px - float2(padding_px);
    float half_width = marker_size_px.x * 0.5;
    p.x -= half_width;

    float side_length = max(length(float2(marker_size_px.y, half_width)), 0.0001);
    float top_distance = p.y;
    float left_distance =
        (marker_size_px.y * (p.x + half_width) - half_width * p.y) / side_length;
    float right_distance =
        (marker_size_px.y * (half_width - p.x) - half_width * p.y) / side_length;
    float inside_distance = min(top_distance, min(left_distance, right_distance));

    float edge_width = max(fwidth(inside_distance), 0.75);
    float alpha = smoothstep(-edge_width * 0.5, edge_width * 0.5, inside_distance);
    if (alpha <= 0.001) {
        discard_fragment();
    }
    return float4(in.color_a.rgb, in.color_a.a * alpha);
}
"#, super::wgsl::TIMELINE_CURSOR_MARKER_FRAGMENT_SHADER);

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
    /// Sound-divergence indicator (takes spec §17.6): a small title-bar dot
    /// drawn when the clip's patch is not the track's scene-effective one.
    /// The color identifies the patch (§17.11 palette semantics); hosts pass
    /// `(r g b)` or `true` for the name-only gray fallback.
    sound_dot: Option<crate::backend::Color>,
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
/// repetition covers relative to the item. Values below 1 repeat; values
/// above 1 mean the item shows only part of one cycle. `phase` is the source
/// position at the item's left edge (0..1), so trims preserve the notes'
/// absolute alignment instead of restarting or stretching the preview.
#[derive(Clone)]
enum TimelineItemContent {
    Dots {
        dots: Vec<TimelineDot>,
        cycle: f64,
        phase: f64,
        /// Whether the phase wraps around the source cycle when a live
        /// start-edge ghost shifts it (patterns loop; takes clamp at their
        /// beginning — `:wrap false`).
        wrap: bool,
    },
    Peaks(Vec<PeakBucket>),
}

#[derive(Clone, Copy)]
struct TimelineDot {
    /// 0.0..1.0 within one source-content cycle.
    offset: f64,
    /// 0.0..1.0 vertical placement within the item rect (1.0 = top).
    value: f64,
    /// Note length as a fraction of one source-content cycle, in the same
    /// normalized axis as `offset`
    /// (docs/arrangement-region-editing-spec.md 3.2). `0` (the default)
    /// draws the legacy point dot; anything larger draws a bar.
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
    /// Optional host-selected lane background. When absent, the timeline
    /// keeps its legacy per-lane defaults.
    background_color: Option<crate::backend::Color>,
    header_height: f32,
    /// Reserved empty strip at the bottom of the ruler header. Arrangement
    /// uses it as a transport-start marker gutter between loop chrome and
    /// scene content; zero preserves the historical compact header.
    header_bottom_gutter: f32,
    sidebar_width: f32,
    view_start: f64,
    view_duration: f64,
    zoom_min_duration: f64,
    zoom_max_duration: f64,
    /// `:grid-density` — asks the zoom-adaptive grid for a finer initial
    /// candidate. Crowded candidates are promoted to a readable aligned
    /// interval; the resolved interval drives both drawing and `:grid` snaps.
    grid_density: f64,
    content_length: Option<f64>,
    content_length_min: f64,
    content_length_max: f64,
    /// Dragging the loop band's BODY slides the loop window (clip-edit-target
    /// spec 5.1: band-body drag = slide window; today ≡ phase shift). Off by
    /// default; the piano roll opts in.
    band_slide: bool,
    /// Emit `:double-click-item` for a title-bar double-click (clip-edit
    /// target spec 4). Opt-in per instance: an instance that does not handle
    /// it (the scene lane) must NOT have the event consumed, or the second
    /// press would silently stop starting its drag gesture.
    double_click_items: bool,
    /// Loop-window overlay over the header band (clip-edit-target spec 5):
    /// a start marker at the source step under the clip's left edge…
    window_marker: Option<f64>,
    /// …the played window when the clip span is shorter than the source…
    window_span: Option<(f64, f64)>,
    /// …or a repeat-count badge when the span covers several passes.
    window_repeat: Option<f64>,
    time_ruler: Option<TimeRuler>,
    playhead_time: Option<f64>,
    cursor_time: Option<f64>,
    /// Cursor rendering is split so a composed timeline can put the marker
    /// on its ruler lane and the vertical line on a separate content lane.
    cursor_marker_visible: bool,
    cursor_line_visible: bool,
    cursor_marker_scale: f32,
    cursor_marker_width_scale: f32,
    cursor_marker_height_scale: f32,
    cursor_color: crate::backend::Color,
    item_color: crate::backend::Color,
    item_label_font_size: f32,
    item_label_color: crate::backend::Color,
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
    Sidebar {
        lane: usize,
    },
    Background {
        time: f64,
    },
    ItemBody {
        item: TimelineItem,
    },
    ItemEdgeEnd {
        item: TimelineItem,
    },
    /// Title-bar zones, only produced when `title-bar-height > 0`
    /// (docs/arrangement-region-editing-spec.md 3.1).
    ItemTitleBar {
        item: TimelineItem,
    },
    ItemEdgeStart {
        item: TimelineItem,
    },
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

fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

fn push_rounded_rect(
    primitives: &mut Vec<GpuPrimitive>,
    rect: Rect,
    color: crate::backend::Color,
    viewport: WidgetViewport,
    radius_px: f32,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    primitives.push(GpuPrimitive::WidgetInstance {
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

fn push_cursor_marker(
    primitives: &mut Vec<GpuPrimitive>,
    center_x: f32,
    top: f32,
    width: f32,
    height: f32,
    color: crate::backend::Color,
    viewport: WidgetViewport,
) {
    const ANTIALIAS_PADDING_PX: f32 = 1.0;

    // Align the triangle's bounds to physical pixels. The marker may shift by
    // at most half a pixel, which is preferable to a different diagonal
    // rasterization pattern at otherwise equivalent timeline positions.
    let marker_width_px = (width * viewport.cell_w).round().max(1.0);
    let marker_height_px = (height * viewport.cell_h).round().max(1.0);
    let marker_left_px = (center_x * viewport.cell_w - marker_width_px * 0.5).round();
    let marker_top_px = (top * viewport.cell_h).round();
    let instance_width_px = marker_width_px + ANTIALIAS_PADDING_PX * 2.0;
    let instance_height_px = marker_height_px + ANTIALIAS_PADDING_PX * 2.0;
    let rect = Rect {
        row: (marker_top_px - ANTIALIAS_PADDING_PX) / viewport.cell_h,
        col: (marker_left_px - ANTIALIAS_PADDING_PX) / viewport.cell_w,
        width: instance_width_px / viewport.cell_w,
        height: instance_height_px / viewport.cell_h,
    };
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    primitives.push(GpuPrimitive::WidgetInstance {
        widget_type: "timeline-cursor-marker".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [
                instance_width_px,
                instance_height_px,
                marker_width_px,
                marker_height_px,
            ],
            uniform_b: [ANTIALIAS_PADDING_PX, 0.0, 0.0, 0.0],
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
            color_a: color.to_rgba(),
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: 0.0,
            pixel_aspect: instance_width_px / instance_height_px,
        },
        is_background: false,
    });
}

impl WidgetDefinition for TimelineCursorMarkerWidget {
    fn names(&self) -> &'static [&'static str] {
        &["timeline-cursor-marker"]
    }

    fn measure(
        &self,
        _node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: 1.0,
            height: 1.0,
        })
    }

    fn fragment_shader(
        &self,
        _widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        TIMELINE_CURSOR_MARKER_FRAGMENT_SHADER.source(backend)
    }
}

impl WidgetDefinition for TimelineWidget {
    fn names(&self) -> &'static [&'static str] {
        &["timeline"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "header-height", "sidebar-width"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "playhead-time",
            "cursor-time",
            "background-color",
            "item-label-font-size",
            "item-label-color",
            // Shared arrangement time axis: scroll/zoom repaint every lane
            // without rerunning any effect (UI_PERFORMANCE_TUNING.md).
            "view-start",
            "view-duration",
            "content-length",
            // Live gesture ghost for THIS lane's items (move/resize/region
            // move) and the live/committed region highlight — per-lane float
            // channels so one drag tick dirties only the affected widgets.
            "ghost-kind",
            "ghost-id",
            "ghost-time",
            "ghost-region-a",
            "ghost-region-b",
            "region-a",
            "region-b",
            "region-on",
            "selected-id",
            "bound-id",
        ]
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
                buf.set(
                    row,
                    col,
                    styled_cell(
                        ' ',
                        theme::FG(),
                        Some(view.background_color.unwrap_or(theme::BG())),
                    ),
                );
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
                let bg = view.background_color.unwrap_or_else(|| {
                    if lane % 2 == 0 {
                        theme::BLACK()
                    } else {
                        theme::BG()
                    }
                });
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
            if view.cursor_marker_visible && content.height > 0.0 {
                let marker_row = view.cursor_marker_top(1.0).round() as u16;
                buf.set(
                    marker_row,
                    cursor_col,
                    styled_cell('▼', view.cursor_color, None),
                );
            }
            if view.cursor_line_visible {
                let line_start = usize::from(view.cursor_marker_visible);
                for row_offset in line_start..content.height.round() as usize {
                    let row = content.row.round() as u16 + row_offset as u16;
                    buf.set(row, cursor_col, styled_cell('|', view.cursor_color, None));
                }
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

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        build_primitives(node, viewport)
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
pub fn debug_grid(node: &LayoutNode) -> (f64, Vec<f32>, Vec<(f32, String)>) {
    let view = TimelineView::from_props(&node.props, node.rect);
    let vp = view.time_viewport();
    (
        view.alignment_helper_grid_step(),
        vp.metal_grid_lines(view.time_ruler.as_ref())
            .into_iter()
            .map(|(x, _)| x)
            .collect(),
        vp.metal_time_ruler_labels(view.time_ruler.as_ref()),
    )
}

fn build_primitives(
    node: &LayoutNode,
    viewport: super::WidgetViewport,
) -> Vec<GpuPrimitive> {
    if node.widget_type != "timeline" {
        return Vec::new();
    }

    let rect = node.rect;
    let view = TimelineView::from_props(&node.props, rect);
    let content = view.content_rect();
    let mut primitives = Vec::new();

    if view.header_height > 0.0 {
        let header_chrome_height = (view.header_height - view.header_bottom_gutter).max(0.0);
        let loop_band = if header_chrome_height > 0.2 {
            view.loop_band_rect().map(|(x, width)| {
                let y = rect.row + (header_chrome_height * 0.55).min(header_chrome_height - 0.18);
                let bottom_inset = 0.08_f32.min(header_chrome_height * 0.12);
                let height = (header_chrome_height - (y - rect.row) - bottom_inset).max(0.12);
                (x, y, width, height)
            })
        } else {
            None
        };
        primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
            rect: Rect {
                row: rect.row,
                col: rect.col,
                width: rect.width,
                height: view.header_height,
            },
            color: theme::STATUS_BG(),
        }));
        if let Some((x, y, width, height)) = loop_band {
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
                + if header_chrome_height >= 1.6 {
                    0.26
                } else {
                    0.06
                };
            primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    row: label_row - 0.04,
                    col: label_col - 0.10,
                    width: label_width,
                    height: 0.86,
                },
                color: theme::STATUS_BG(),
            }));
            primitives.push(GpuPrimitive::ProportionalText(
                GpuProportionalTextPrimitive {
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
        // Loop-window overlay (clip-edit-target spec 5): the played window
        // and its start marker over the band, or a repeat-count badge when
        // the clip span covers several passes of the source.
        if let Some((_, y, _, height)) = loop_band {
            let content_len = view.content_length.unwrap_or(0.0);
            let view_end = view.view_start + view.view_duration;
            let mut band_segment = |seg_start: f64, seg_end: f64, alpha: f32| {
                let seg_start = seg_start.max(view.view_start).max(0.0);
                let seg_end = seg_end.min(content_len).min(view_end);
                if seg_end <= seg_start {
                    return;
                }
                let x0 = view.x_for_time(seg_start).max(content.col);
                let x1 = view.x_for_time(seg_end).min(content.col + content.width);
                if x1 <= x0 {
                    return;
                }
                primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                    x: x0,
                    y,
                    width: x1 - x0,
                    height,
                    color: crate::backend::Color {
                        a: alpha,
                        ..view.loop_color
                    },
                }));
            };
            if let Some((span_start, span_end)) = view.window_span {
                // The window may wrap past the source end (the phase offset
                // slid it): draw the tail from 0.
                band_segment(span_start, span_end.min(content_len), 0.34);
                if span_end > content_len {
                    band_segment(0.0, span_end - content_len, 0.34);
                }
            }
            if let Some(marker) = view.window_marker {
                if marker >= view.view_start && marker < view_end && marker < content_len {
                    let x = view.x_for_time(marker);
                    if x >= content.col && x <= content.col + content.width {
                        primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                            x: x - 0.09,
                            y,
                            width: 0.18,
                            height,
                            color: crate::backend::Color {
                                a: 1.0,
                                ..view.loop_color
                            },
                        }));
                    }
                }
            }
            if let Some(repeat) = view.window_repeat {
                let text = format!("×{}", repeat.round() as i64);
                let band_end = view
                    .x_for_time(content_len.min(view_end))
                    .min(content.col + content.width);
                primitives.push(GpuPrimitive::ProportionalText(
                    GpuProportionalTextPrimitive {
                        row: y + (height - 0.86).max(0.0) * 0.5,
                        col: (band_end - text.chars().count() as f32 * 0.62 - 0.3).max(content.col),
                        align_width: 0.0,
                        h_align: 0.0,
                        text,
                        font_size: 10.0,
                        scale: 1.0,
                        fg: theme::FG_MUTED(),
                        bg: theme::STATUS_BG(),
                    },
                ));
            }
        }
        if let Some((x, y, width, height)) = loop_band {
            let border_color = crate::backend::Color {
                a: 0.95,
                ..view.loop_color
            };
            let h = (1.0 / viewport.cell_h).min(height);
            let v = (1.0 / viewport.cell_w).min(width);
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x,
                y,
                width,
                height: h,
                color: border_color,
            }));
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x,
                y: y + height - h,
                width,
                height: h,
                color: border_color,
            }));
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x,
                y,
                width: v,
                height,
                color: border_color,
            }));
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
                primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
                    primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
                primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
                primitives.push(GpuPrimitive::ProportionalText(
                    GpuProportionalTextPrimitive {
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
        let grid_color = view.background_color.unwrap_or_else(|| {
            if sidebar_bg == theme::WHITE() {
                crate::backend::Color::from_hex(0x16, 0x16, 0x18)
            } else {
                crate::backend::Color::from_hex(0x0d, 0x0d, 0x0f)
            }
        });
        primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
            x: content.col,
            y: row_start,
            width: content.width,
            height: lane_height,
            color: grid_color,
        }));
    }

    let grid_lines = view.metal_grid_lines();
    for &(x, is_major) in &grid_lines {
        primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
        primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
                primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x,
                y,
                width,
                height: thickness,
                color: border_color,
            }));
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x,
                y: y + height - thickness,
                width,
                height: thickness,
                color: border_color,
            }));
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x,
                y,
                width: thickness,
                height,
                color: border_color,
            }));
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
        let (content_rect, title_bar) = match title_bar_height {
            Some(bar_height) => (
                (x, y + bar_height, width, height - bar_height),
                Some((y, bar_height)),
            ),
            None => ((x, y, width, height), None),
        };
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
        let loop_segment_xs = if title_bar_height.is_some() && radius_px > 0.5 {
            item_cycle_separator_xs(&view, item, viewport)
        } else {
            Vec::new()
        };
        // Fills are built from the item's TRUE span and then clipped to the
        // visible content, not from the view-clamped rect: an item scrolled
        // partly off-screen must be CUT at the viewport edge, not redrawn as
        // a shorter clip that rounds its corners there.
        let (fill_rect, clip) = view.item_fill_rect(item, y, height);
        // Square edge quads would poke out of a rounded clip's corners, so a
        // rounded item is outlined by drawing the border colour at the item's
        // own bounds and insetting the fill inside it — never by inflating,
        // which would push the clip into the neighbouring lane. Square items
        // keep the four edge quads below.
        //
        // Repeated play-throughs are individual rounded FILL segments over
        // this one continuous outline. Their shared x edges are not inset,
        // so adjacent segments meet through the middle while their top and
        // bottom corners expose the dark outline as a compact loop cue.
        let mut fill_rect = fill_rect;
        let thickness =
            item_border_thickness(width, height, selected_border(selected, title_bar_height));
        if radius_px > 0.0 {
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
        let fill_segments = item_loop_segment_rects(fill_rect, &loop_segment_xs);
        let loops_segmented = fill_segments.len() > 1;
        push_item_segment_fills(
            &mut primitives,
            &fill_segments,
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
                push_item_segment_fills(
                    &mut primitives,
                    &fill_segments,
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
            let bar_segments: Vec<_> = fill_segments
                .iter()
                .map(|segment| Rect {
                    height: bar_height - (segment.row - y) + radius_cells,
                    ..*segment
                })
                .collect();
            let mut bar_primitives = Vec::new();
            push_item_segment_fills(
                &mut bar_primitives,
                &bar_segments,
                item_color,
                viewport,
                radius_px,
                clip,
            );
            let bar_bottom = y + bar_height;
            primitives.extend(bar_primitives.into_iter().filter_map(|primitive| {
                let GpuPrimitive::Quad(mut quad) = primitive else {
                    return Some(primitive);
                };
                let bottom = (quad.y + quad.height).min(bar_bottom);
                if bottom <= quad.y {
                    return None;
                }
                quad.height = bottom - quad.y;
                Some(GpuPrimitive::Quad(quad))
            }));
            let hairline = (1.0 / viewport.cell_h).min(height - bar_height);
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
        // their content. The title bar is deliberately excluded so the label
        // remains a clean visual band; loop boundaries are expressed by the
        // abutting rounded segment fills instead.
        for &(grid_x, is_major) in &grid_lines {
            if grid_x <= x + 0.1 || grid_x >= x + width - 0.1 {
                continue;
            }
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x: grid_x - 0.0625,
                y: content_rect.1,
                width: 0.125,
                height: content_rect.3,
                color: crate::backend::Color {
                    r: 0.02,
                    g: 0.025,
                    b: 0.03,
                    a: if is_major { 0.22 } else { 0.10 },
                },
            }));
        }
        // Labels live in the title bar when there is one; content (notes)
        // draws in the body below it. The text run is explicitly scissored
        // to the visible item/title-bar rect: proportional text has no
        // intrinsic width constraint, and a short clip must never paint its
        // label over the next clip or an empty part of the lane.
        let label_height = title_bar_height.unwrap_or(height);
        // Sound-identity square (takes spec §17.6): a letter-sized colored
        // square at the title bar's left edge — deliberately large enough to
        // read as a click target (a future palette-overlay trigger). Sized
        // in PIXELS so it is visually square regardless of the cell aspect,
        // roughly the label's letter height. The label shifts right.
        let mut label_col = x + 0.34;
        if let Some(dot) = item.sound_dot {
            if width >= 1.2 && label_height >= 0.85 {
                let side_px = (view.item_label_font_size * 1.26).max(5.0);
                let mut side_rows = (side_px / viewport.cell_h).min(label_height * 0.66);
                let mut side_cols = side_rows * viewport.cell_h / viewport.cell_w;
                // The quad is not scissored (only the label run is), so it
                // must also fit the clip's own width or it paints into the
                // neighbor; when the width cap binds, re-derive the height
                // from it so the square stays square.
                let max_cols = (width - 0.6).max(0.0);
                if side_cols > max_cols {
                    side_cols = max_cols;
                    side_rows = side_cols * viewport.cell_w / viewport.cell_h;
                }
                // Nudged slightly below true center to sit on the label's
                // optical line (the text row leans low in the bar).
                primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                    x: x + 0.30,
                    y: y + ((label_height - side_rows).max(0.0) * 0.5) + 0.04,
                    width: side_cols,
                    height: side_rows,
                    color: dot,
                }));
                label_col = x + 0.30 + side_cols + 0.28;
            }
        }
        if let Some(label) = &item.label {
            if width >= 3.0 && label_height >= 0.85 {
                primitives.push(GpuPrimitive::PushClipRect(Rect {
                    row: y,
                    col: x,
                    width,
                    height: label_height,
                }));
                primitives.push(GpuPrimitive::ProportionalText(
                    GpuProportionalTextPrimitive {
                        row: y + ((label_height - 0.80).max(0.0) * 0.5) - 0.02,
                        col: label_col,
                        align_width: 0.0,
                        h_align: 0.0,
                        text: label.clone(),
                        font_size: view.item_label_font_size,
                        scale: 1.0,
                        fg: view.item_label_color,
                        bg: item_color,
                    },
                ));
                primitives.push(GpuPrimitive::PopClipRect);
            }
        }
        push_item_content_primitives(
            &mut primitives,
            &view,
            item,
            content_rect,
            title_bar,
            loops_segmented,
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
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x,
                y,
                width,
                height: thickness,
                color: border_color,
            }));
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x,
                y: y + height - thickness,
                width,
                height: thickness,
                color: border_color,
            }));
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x,
                y,
                width: thickness,
                height,
                color: border_color,
            }));
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
        let marker_width = (((8.0 * view.cursor_marker_scale * view.cursor_marker_width_scale)
            / viewport.cell_w)
            .max(line_width * 4.0)
            * viewport.cell_w)
            .round()
            / viewport.cell_w;
        let marker_height_scale = view.cursor_marker_scale * view.cursor_marker_height_scale;
        let marker_height = ((((5.0 * marker_height_scale) / viewport.cell_h)
            .max(0.28 * marker_height_scale)
            .min(content.height))
            * viewport.cell_h)
            .round()
            / viewport.cell_h;
        let marker_y = view.cursor_marker_top(marker_height);
        if view.cursor_marker_visible && marker_height > 0.0 {
            push_cursor_marker(
                &mut primitives,
                cursor_x,
                marker_y,
                marker_width,
                marker_height,
                view.cursor_color,
                viewport,
            );
        }
        let line_y = content.row
            + if view.cursor_marker_visible && view.header_bottom_gutter <= 0.0 {
                marker_height
            } else {
                0.0
            };
        let line_height = (content.row + content.height - line_y).max(0.0);
        if view.cursor_line_visible && line_height > 0.0 {
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x: cursor_x - line_width * 0.5,
                y: line_y,
                width: line_width,
                height: line_height,
                color: view.cursor_color,
            }));
        }
    }

    if let Some(playhead_x) = view.metal_playhead_x() {
        if view.header_height > 0.0 {
            primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
                x: playhead_x - 0.0625,
                y: rect.row,
                width: 0.125,
                height: view.header_height,
                color: theme::YELLOW(),
            }));
        }
        primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
            x: playhead_x - 0.0625,
            y: content.row,
            width: 0.125,
            height: content.height,
            color: theme::YELLOW(),
        }));
    }

    primitives
}

const ITEM_BORDER_COLOR: crate::backend::Color = crate::backend::Color {
    r: 0.02,
    g: 0.025,
    b: 0.03,
    a: 0.72,
};

const SELECTED_ITEM_BORDER_COLOR: crate::backend::Color = crate::backend::Color {
    r: 0.725,
    g: 0.933,
    b: 1.0,
    a: 1.0,
};

/// A title-barred clip shows selection through its lit body, so it keeps the
/// ordinary dark outline; bar-less hosts (piano roll) still get the bright
/// selected border.
fn selected_border(selected: bool, title_bar_height: Option<f32>) -> bool {
    selected && title_bar_height.is_none()
}

fn item_border_thickness(width: f32, height: f32, selected: bool) -> f32 {
    if selected { 0.16_f32 } else { 0.08_f32 }
        .min(width * 0.5)
        .min(height * 0.5)
}

/// Corner arc resolution cap: one slab per device pixel of radius, bounded so
/// a huge radius cannot flood the primitive list.
const ITEM_CORNER_ROWS_MAX: usize = 24;

/// Render all loop play-through fills. Segment rects abut exactly and are
/// clipped only at the item's/view's true visible bounds.
fn push_item_segment_fills(
    primitives: &mut Vec<GpuPrimitive>,
    segments: &[Rect],
    color: crate::backend::Color,
    viewport: super::WidgetViewport,
    radius_px: f32,
    clip: (f32, f32),
) {
    for segment in segments {
        push_item_fill(primitives, *segment, color, viewport, radius_px, clip);
    }
}

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
fn push_item_fill(
    primitives: &mut Vec<GpuPrimitive>,
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
        let inset_px = radius_px
            - (radius_px * radius_px - above_center * above_center)
                .max(0.0)
                .sqrt();
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
fn push_clipped_quad(
    primitives: &mut Vec<GpuPrimitive>,
    rect: Rect,
    color: crate::backend::Color,
    clip: (f32, f32),
) {
    let left = rect.col.max(clip.0);
    let right = (rect.col + rect.width).min(clip.1);
    if right <= left || rect.height <= 0.0 {
        return;
    }
    primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
const SELECTED_ITEM_BODY_COLOR: crate::backend::Color = crate::backend::Color {
    r: 0.94,
    g: 0.87,
    b: 0.68,
    a: 1.0,
};

/// Minimum on-screen item width (px) below which item content is skipped
/// entirely — narrower than this the dots/bars would only alias
/// (docs/arrangement-timeline-ui-spec.md 7.3).
const ITEM_CONTENT_MIN_WIDTH_PX: f32 = 14.0;

fn item_cycle_separator_xs(
    view: &TimelineView,
    item: &TimelineItem,
    viewport: super::WidgetViewport,
) -> Vec<f32> {
    let Some(TimelineItemContent::Dots {
        cycle, phase, wrap, ..
    }) = &item.content
    else {
        return Vec::new();
    };
    // Non-wrapping content (takes) plays its source exactly once: there are
    // no repeat play-throughs, so there is no loop boundary to cue — an item
    // stretched past the source end is silence, not another pass.
    if !wrap {
        return Vec::new();
    }
    let span = item.end - item.start;
    if span <= 0.0 {
        return Vec::new();
    }
    let cycle = cycle.max(f64::EPSILON);
    let phase = phase.rem_euclid(1.0);
    if item_cycle_width_px(view, item, viewport).is_none_or(|width| width < 5.0) {
        return Vec::new();
    }
    let view_end = view.view_start + view.view_duration;
    let visible_offset_start =
        ((view.view_start.max(item.start) - item.start) / span).clamp(0.0, 1.0);
    let visible_offset_end = ((view_end.min(item.end) - item.start) / span).clamp(0.0, 1.0);
    if visible_offset_end <= visible_offset_start {
        return Vec::new();
    }

    let first_offset = (1.0 - phase) * cycle;
    let first_visible_index = ((visible_offset_start - first_offset) / cycle)
        .ceil()
        .max(0.0);
    let mut offset = first_offset + first_visible_index * cycle;
    let mut separators = Vec::new();
    while offset < visible_offset_end && offset < 1.0 {
        let time = item.start + span * offset;
        separators.push(view.x_for_time(time));
        offset += cycle;
    }
    separators
}

/// Split an item's true fill rect at each visible source-cycle boundary.
/// Segment edges touch exactly: there is no border or spacing between repeat
/// play-throughs. Rounding each segment independently leaves only the small
/// top/bottom corner cutouts that communicate the loop.
///
/// Separators outside the viewport are intentionally absent. The first and
/// last returned rects still extend to the item's true (possibly off-screen)
/// edges, so horizontal clipping cannot invent rounded corners at the view.
fn item_loop_segment_rects(rect: Rect, separator_xs: &[f32]) -> Vec<Rect> {
    let right = rect.col + rect.width;
    let mut segments = Vec::with_capacity(separator_xs.len() + 1);
    let mut left = rect.col;
    for separator in separator_xs {
        let separator = separator.clamp(rect.col, right);
        if separator <= left {
            continue;
        }
        segments.push(Rect {
            col: left,
            width: separator - left,
            ..rect
        });
        left = separator;
    }
    if right > left {
        segments.push(Rect {
            col: left,
            width: right - left,
            ..rect
        });
    }
    if segments.is_empty() {
        segments.push(rect);
    }
    segments
}

fn item_cycle_width_px(
    view: &TimelineView,
    item: &TimelineItem,
    viewport: super::WidgetViewport,
) -> Option<f32> {
    let Some(TimelineItemContent::Dots { cycle, .. }) = &item.content else {
        return None;
    };
    let cycle_duration = (item.end - item.start).max(0.0) * cycle.max(f64::EPSILON);
    let content_width_px = view.content_rect().width * viewport.cell_w;
    Some((cycle_duration / view.view_duration.max(0.0001)) as f32 * content_width_px)
}

/// Draw an item's `content` payload as additional quads clipped to the item's
/// on-screen rect (docs/arrangement-timeline-ui-spec.md 7.3). `rect` is the
/// already view-clipped rect from `metal_item_rect`; dot x positions come
/// from the item's unclipped time span so partially visible items keep their
/// content aligned. `title_bar` is the `(y, height)` of the clip's title bar
/// when it has one. Rounded title-barred clips express visible repeats through
/// their abutting fill segments; square or bar-less hosts retain the explicit
/// separator rule.
fn push_item_content_primitives(
    primitives: &mut Vec<GpuPrimitive>,
    view: &TimelineView,
    item: &TimelineItem,
    rect: (f32, f32, f32, f32),
    title_bar: Option<(f32, f32)>,
    loop_segments_enabled: bool,
    viewport: super::WidgetViewport,
) {
    let (x, y, width, height) = rect;
    if width * viewport.cell_w < ITEM_CONTENT_MIN_WIDTH_PX {
        return;
    }
    match &item.content {
        Some(TimelineItemContent::Dots {
            dots,
            cycle,
            phase,
            wrap,
        }) => {
            let span = item.end - item.start;
            if span <= 0.0 {
                return;
            }
            let cycle = cycle.max(f64::EPSILON);
            let phase = phase.rem_euclid(1.0);
            // Derive one source cycle directly from time, not from clamped
            // screen edges, so density stays stable when a long clip extends
            // beyond either side of the viewport.
            let cycle_width_px = item_cycle_width_px(view, item, viewport).unwrap_or(0.0) as f64;

            // Cycle separators: the first boundary is the remaining part of
            // the source cycle after `phase`; later boundaries are one full
            // cycle apart. This works for both repeating clips (`cycle < 1`)
            // and a short clip that happens to cross one source boundary.
            if cycle_width_px >= 5.0 && (title_bar.is_none() || !loop_segments_enabled) {
                let separator_color = crate::backend::Color {
                    r: 0.02,
                    g: 0.025,
                    b: 0.03,
                    a: 0.42,
                };
                // Bar-less hosts keep the full-height repeat rule. A square
                // title-barred host has no rounded corners to express segment
                // boundaries, so it keeps the short legacy tick.
                let (separator_y, separator_height) = match title_bar {
                    Some((bar_y, bar_height)) => (bar_y, (bar_height * 0.55).max(0.12)),
                    None => (y, height),
                };
                for line_x in item_cycle_separator_xs(view, item, viewport) {
                    primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
            // One partial source cycle can straddle the left edge, hence the
            // extra iteration beyond the number of complete visible cycles.
            // Non-wrapping content (takes) never repeats: one pass only, so
            // the stretch past the source end stays visibly empty — extending
            // a take must read as growth, not looping.
            let repetitions = if *wrap {
                (1.0 / cycle).ceil().min(512.0) as usize + 1
            } else {
                1
            };
            for index in 0..repetitions {
                for dot in dots {
                    let offset = (dot.offset - phase + index as f64) * cycle;
                    if offset < 0.0 {
                        continue;
                    }
                    if offset >= 1.0 {
                        continue;
                    }
                    let time = item.start + offset * span;
                    if time < view.view_start || time >= view.view_start + view.view_duration {
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
                        let end_offset = (offset + dot.width * cycle).min(1.0);
                        let end_x = view.x_for_time(item.start + end_offset * span);
                        (end_x - dot_x)
                            .max(dot_width)
                            .min((x + width - dot_x).max(dot_width))
                    } else {
                        dot_width
                    };
                    primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive {
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
        let header_height = get_num(props, "header-height", 1.0).max(0.0) as f32;
        let mut view = Self {
            rect,
            background_color: props
                .get("background-color")
                .and_then(theme::parse_color_value),
            header_height,
            header_bottom_gutter: get_num(props, "header-bottom-gutter", 0.0)
                .clamp(0.0, header_height as f64) as f32,
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
            band_slide: props
                .get("band-slide")
                .and_then(as_bool)
                .unwrap_or(false),
            double_click_items: props
                .get("double-click-items")
                .and_then(as_bool)
                .unwrap_or(false),
            window_marker: props
                .get("window-marker")
                .and_then(as_number)
                .filter(|marker| *marker >= 0.0),
            window_span: props.get("window-span").and_then(|value| {
                let Value::List(items) = value else { return None };
                let start = as_number(&items.first()?.borrow())?;
                let end = as_number(&items.get(1)?.borrow())?;
                (end > start).then_some((start, end))
            }),
            window_repeat: props
                .get("window-repeat")
                .and_then(as_number)
                .filter(|count| *count > 1.0),
            time_ruler: props
                .get("time-ruler")
                .and_then(get_map)
                .and_then(|map| get_time_ruler(&map)),
            playhead_time: props.get("playhead-time").and_then(as_number),
            // Negative cursor times mean "no cursor in this lane": the
            // arrangement publishes the per-lane cursor as a bound float, and
            // floats have no nil.
            cursor_time: props
                .get("cursor-time")
                .and_then(as_number)
                .filter(|time| *time >= 0.0),
            cursor_marker_visible: props
                .get("cursor-marker-visible")
                .and_then(as_bool)
                .unwrap_or(true),
            cursor_line_visible: props
                .get("cursor-line-visible")
                .and_then(as_bool)
                .unwrap_or(true),
            cursor_marker_scale: get_num(props, "cursor-marker-scale", 1.0)
                .clamp(0.25, 4.0) as f32,
            cursor_marker_width_scale: get_num(props, "cursor-marker-width-scale", 1.0)
                .clamp(0.5, 4.0) as f32,
            cursor_marker_height_scale: get_num(props, "cursor-marker-height-scale", 1.0)
                .clamp(0.25, 4.0) as f32,
            cursor_color: resolve_named_color(props, "cursor-color", theme::CURSOR()),
            item_color: resolve_named_color(props, "item-color", theme::BLUE()),
            item_label_font_size: get_num(props, "item-label-font-size", 10.5).max(1.0) as f32,
            item_label_color: resolve_named_color(props, "item-label-color", theme::BLACK()),
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
        view.apply_bound_channels(props);
        view
    }

    /// Per-lane bound float channels (UI_PERFORMANCE_TUNING.md ownership
    /// boundary: gesture-only state must not be globally reactive). The
    /// arrangement publishes live drag ghosts, the click selection, the sound
    /// binding and the region highlight as reactive floats, so a drag tick
    /// repaints only the affected lane widgets instead of rerunning the
    /// arrangement effect. The geometry below is an exact port of the Lisp
    /// ghost projection it replaces (arrangement-track-ghost-clip): move is
    /// rigid, the end edge clamps to [start+1, content-length], the start
    /// edge clamps to [0, end-1] and re-anchors the content phase by the
    /// split rule (patterns wrap, takes clamp at zero).
    fn apply_bound_channels(&mut self, props: &HashMap<String, Value>) {
        // A clicked clip is usually BOTH channels (a click selects and binds
        // the sound), so pushing without the dedup would hold the same id
        // twice — and Backspace would emit two :delete-items for one clip,
        // deleting it and then erroring on the second, already-gone id.
        let mut select = |id: f64| {
            if id >= 0.0
                && !self
                    .selection
                    .iter()
                    .any(|existing| matches!(existing, Value::Number(n) if *n == id))
            {
                self.selection.push(Value::Number(id));
            }
        };
        select(get_num(props, "selected-id", -1.0));
        select(get_num(props, "bound-id", -1.0));
        let ghost_region_a = get_num(props, "ghost-region-a", f64::NAN);
        let ghost_region_b = get_num(props, "ghost-region-b", f64::NAN);
        let ghost_kind = get_num(props, "ghost-kind", 0.0);
        // Region highlight: the in-flight marquee/region ghost wins over the
        // committed region; either overrides a static :selection-rect prop.
        if ghost_kind >= 3.5 && ghost_kind < 4.5 && ghost_region_a.is_finite() {
            self.selection_rect = Some(TimelineSelectionRect {
                time_a: ghost_region_a,
                time_b: ghost_region_b,
                lane_a: 0,
                lane_b: 0,
            });
        } else if get_num(props, "region-on", 0.0) > 0.5 {
            self.selection_rect = Some(TimelineSelectionRect {
                time_a: get_num(props, "region-a", 0.0),
                time_b: get_num(props, "region-b", 0.0),
                lane_a: 0,
                lane_b: 0,
            });
        }
        if ghost_kind < 0.5 {
            return;
        }
        let ghost_time = get_num(props, "ghost-time", f64::NAN);
        if !ghost_time.is_finite() {
            return;
        }
        // Region move (kind 5): every item the SOURCE rectangle covers slides
        // by the drag delta; the shifted rectangle is the region highlight.
        if ghost_kind >= 4.5 {
            if !ghost_region_a.is_finite() || !ghost_region_b.is_finite() {
                return;
            }
            for item in &mut self.items {
                if item.end > ghost_region_a && item.start < ghost_region_b {
                    item.start += ghost_time;
                    item.end += ghost_time;
                }
            }
            self.selection_rect = Some(TimelineSelectionRect {
                time_a: ghost_region_a + ghost_time,
                time_b: ghost_region_b + ghost_time,
                lane_a: 0,
                lane_b: 0,
            });
            return;
        }
        if ghost_kind >= 3.5 {
            // Kind 4 is the marquee rect alone, handled above.
            return;
        }
        let ghost_id = get_num(props, "ghost-id", f64::NAN);
        let max_end = self.content_length.unwrap_or(f64::INFINITY);
        let Some(item) = self
            .items
            .iter_mut()
            .find(|item| matches!(item.id, Value::Number(id) if id == ghost_id))
        else {
            return;
        };
        let old_span = (item.end - item.start).max(1e-9);
        if ghost_kind < 1.5 {
            // Move: rigid slide, content untouched.
            let new_start = ghost_time.max(0.0);
            let delta = new_start - item.start;
            item.start = new_start;
            item.end += delta;
        } else if ghost_kind < 2.5 {
            // Start-edge resize: phase re-anchors by the split rule.
            let new_start = ghost_time.max(0.0).min(item.end - 1.0);
            let delta = new_start - item.start;
            item.start = new_start;
            let new_span = (item.end - item.start).max(1e-9);
            if let Some(TimelineItemContent::Dots {
                cycle, phase, wrap, ..
            }) = item.content.as_mut()
            {
                let length_beats = *cycle * old_span;
                if length_beats > 0.0 {
                    let shifted = *phase + delta / length_beats;
                    *phase = if *wrap {
                        shifted.rem_euclid(1.0)
                    } else {
                        shifted.max(0.0)
                    };
                }
                *cycle = (*cycle * old_span / new_span).max(1e-9);
            }
        } else {
            // End-edge resize: pure occlusion, clamped to the song end.
            let new_end = ghost_time.min(max_end).max(item.start + 1.0);
            item.end = new_end;
            let new_span = (item.end - item.start).max(1e-9);
            if let Some(TimelineItemContent::Dots { cycle, .. }) = item.content.as_mut() {
                *cycle = (*cycle * old_span / new_span).max(1e-9);
            }
        }
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

    /// The loop band's vertical extent inside the header — the SAME geometry
    /// the metal renderer draws (clip-edit-target spec 5.1: band-BODY drag =
    /// slide window; the ruler rows above the band must keep scrubbing).
    fn loop_band_rows(&self) -> Option<(f32, f32)> {
        let chrome = (self.header_height - self.header_bottom_gutter).max(0.0);
        if chrome <= 0.2 {
            return None;
        }
        let y = self.rect.row + (chrome * 0.55).min(chrome - 0.18);
        let bottom_inset = 0.08_f32.min(chrome * 0.12);
        let height = (chrome - (y - self.rect.row) - bottom_inset).max(0.12);
        Some((y, y + height))
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

    fn cursor_marker_top(&self, marker_height: f32) -> f32 {
        let content_top = self.content_rect().row;
        if self.header_bottom_gutter <= 0.0 {
            return content_top;
        }
        let gutter_top = content_top - self.header_bottom_gutter;
        gutter_top + (self.header_bottom_gutter - marker_height).max(0.0) * 0.5
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
    fn unclamped_x_for_time(&self, time: f64) -> f32 {
        let content = self.content_rect();
        if content.width == 0.0 {
            return content.col;
        }
        let position = content.col as f64
            + content.width as f64 * ((time - self.view_start) / self.view_duration.max(0.0001));
        (position as f32).clamp(
            content.col - content.width,
            content.col + content.width * 2.0,
        )
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
            // clips it makes the boundary a coin flip. Only title-barred
            // hosts want that tightening — bar-less hosts (piano roll) keep
            // the forgiving grips notes have always had.
            let handle_width = if self.has_title_bar() {
                (rect.width * 0.24).clamp(0.5, 1.25)
            } else {
                (rect.width * 0.24).clamp(1.25, 4.0)
            };
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
            // `item_rect` clamps a clip that begins left of the view to the
            // content's left edge, so `left` is NOT the clip's start edge for
            // a scrolled-off clip. Offering a start grip there would trim an
            // invisible boundary on what looks like a plain title-bar drag.
            let start_edge_visible = item.start >= self.view_start;

            if local_col >= left && local_col < right {
                if has_handles && local_col >= right - handle_width {
                    return Some(HitRegion::ItemEdgeEnd { item: item.clone() });
                }
                if has_handles
                    && start_edge_visible
                    && title_bar_bottom.is_some()
                    && local_col <= left + handle_width
                {
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
                    && start_edge_visible
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
                HitRegion::Header => {
                    // With band-slide enabled, a drag starting inside the
                    // loop band slides the loop window (clip-edit-target
                    // spec 5.1) instead of scrubbing; the plain click still
                    // parks the cursor via pointer-down as before.
                    let raw_time = self.time_at_col(local_col);
                    let in_band = self
                        .content_length
                        .is_some_and(|length| raw_time >= 0.0 && raw_time < length)
                        && self
                            .loop_band_rows()
                            .is_some_and(|(top, bottom)| local_row >= top && local_row < bottom);
                    if self.band_slide && in_band {
                        Some(map_value(vec![
                            ("kind", keyword(":slide-band")),
                            ("anchor-time", Value::Number(raw_time)),
                        ]))
                    } else {
                        Some(map_value(vec![("kind", keyword(":scrub"))]))
                    }
                }
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
                HitRegion::ItemBody { item } if self.has_title_bar() => {
                    Some(action_map(vec![
                        ("type", keyword(":clear-selection")),
                        // Preserve which clip body was pressed so hosts with
                        // an explicit clip-editor mode can promote the same
                        // press to clip selection. Hosts that treat bodies as
                        // region surfaces keep the clear-selection semantics.
                        ("ids", list_value(vec![item.id])),
                        (
                            "time",
                            Value::Number(self.cursor_snap_time(self.time_at_col(local_col))),
                        ),
                    ]))
                }
                HitRegion::ItemBody { item }
                | HitRegion::ItemEdgeEnd { item }
                | HitRegion::ItemTitleBar { item }
                | HitRegion::ItemEdgeStart { item } => Some(action_map(vec![
                    ("type", keyword(":select")),
                    ("ids", list_value(vec![item.id])),
                    ("mode", keyword(":replace")),
                    (
                        "time",
                        Value::Number(self.cursor_snap_time(self.time_at_col(local_col))),
                    ),
                ])),
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
                | HitRegion::ItemEdgeStart { item } => Some(action_map(vec![
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
            // Title bar only (clip-edit-target spec 4, locked decision 5):
            // "the top part of the clip" opens its editor. Body and edge hits
            // keep returning None so a body double-click starts nothing, and
            // instances that never handle the action (the scene lane) opt
            // out entirely so the press still starts its normal gesture.
            HitRegion::ItemTitleBar { item } if self.double_click_items => Some(action_map(vec![
                ("type", keyword(":double-click-item")),
                ("ids", list_value(vec![item.id])),
                (
                    "time",
                    Value::Number(self.cursor_snap_time(self.time_at_col(local_col))),
                ),
            ])),
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
        let is_marquee =
            matches!(gesture.get("kind"), Some(Value::Keyword(kind)) if kind == "marquee");
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
            Some(Value::Keyword(kind)) if kind == "slide-band" => {
                let anchor = as_number(gesture.get("anchor-time")?)?;
                Some(action_map(vec![
                    ("type", keyword(":slide-band")),
                    ("delta-time", Value::Number(raw_time - anchor)),
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
            // Terminal for a band slide: one action with the TOTAL delta, so
            // the host lowers the whole drag to a single undoable primitive
            // (clip-edit-target spec 5: slide = phase edit).
            Some(Value::Keyword(kind)) if kind == "slide-band" => {
                let anchor = as_number(gesture.get("anchor-time")?)?;
                let raw_time = self.time_at_col(local_col);
                Some(action_map(vec![
                    ("type", keyword(":finish-slide-band")),
                    ("delta-time", Value::Number(raw_time - anchor)),
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
        // Bindable props arrive as reactive float refs; resolving here lets
        // high-rate props (the shared arrangement view axis, live gesture
        // ghosts) update by widget repaint instead of an effect rerun.
        Some(Value::ReactiveRef { slot, .. }) => crate::reactive::read_float_slot(slot),
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
                sound_dot: map.get("sound-dot").and_then(parse_sound_dot),
            })
        })
        .collect()
}

/// Lenient `:sound-dot` parse: an `(r g b)` list becomes the dot color, a
/// bare `true` the name-only gray fallback (takes spec §17.11); anything
/// else is no dot, never a render error.
fn parse_sound_dot(value: &Value) -> Option<crate::backend::Color> {
    match value {
        Value::List(items) if items.len() >= 3 => {
            let channel = |idx: usize| as_number(&items[idx].borrow());
            Some(crate::backend::Color {
                r: channel(0)? as f32,
                g: channel(1)? as f32,
                b: channel(2)? as f32,
                a: 1.0,
            })
        }
        Value::Bool(true) => Some(crate::backend::Color {
            r: 0.62,
            g: 0.62,
            b: 0.66,
            a: 1.0,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod sound_dot_tests {
    use super::*;

    #[test]
    fn sound_dot_parses_rgb_list_gray_fallback_and_rejects_junk() {
        let rgb = Value::List(
            [0.9, 0.6, 0.3]
                .iter()
                .map(|v| std::rc::Rc::new(std::cell::RefCell::new(Value::Number(*v))))
                .collect(),
        );
        let color = parse_sound_dot(&rgb).expect("rgb list parses");
        assert!((color.r - 0.9).abs() < 1e-6);
        assert!((color.b - 0.3).abs() < 1e-6);
        assert!(
            parse_sound_dot(&Value::Bool(true)).is_some(),
            "true is the name-only gray fallback"
        );
        assert!(parse_sound_dot(&Value::Bool(false)).is_none());
        assert!(parse_sound_dot(&Value::Nil).is_none());
        assert!(parse_sound_dot(&Value::String("red".to_string())).is_none());
    }
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
        // `cycle` is the source-cycle length relative to the item. It may be
        // above one when a short item shows only a source window.
        let cycle = map
            .get("cycle")
            .and_then(as_number)
            .filter(|cycle| cycle.is_finite() && *cycle > 0.0)
            .unwrap_or(1.0);
        // `phase` is normalized rather than clamped so negative or >1 host
        // values remain meaningful source positions.
        let phase = map
            .get("phase")
            .and_then(as_number)
            .filter(|phase| phase.is_finite())
            .map(|phase| phase.rem_euclid(1.0))
            .unwrap_or(0.0);
        let wrap = !matches!(map.get("wrap"), Some(Value::Bool(false)));
        return Some(TimelineItemContent::Dots {
            dots,
            cycle,
            phase,
            wrap,
        });
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
        crate::vm::debug_assert_cell_not_frozen(value, "timeline set_map_bool");
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

    /// The bound ghost channels must reproduce the arrangement's Lisp ghost
    /// projection exactly: rigid move, clamped end resize with cycle
    /// rescaling, and start resize with the split-rule phase re-anchor
    /// (patterns wrap, takes clamp at zero).
    #[test]
    fn bound_ghost_channels_transform_items_like_the_lisp_projection() {
        let base_items = || {
            list_value_raw(vec![map_value_raw(vec![
                ("id", number_value(7.0)),
                ("lane", number_value(0.0)),
                ("start", number_value(8.0)),
                ("end", number_value(16.0)),
                (
                    "content",
                    map_value_raw(vec![
                        (
                            "dots",
                            list_value_raw(vec![map_value_raw(vec![
                                ("offset", number_value(0.0)),
                                ("value", number_value(0.5)),
                            ])]),
                        ),
                        ("cycle", number_value(0.5)),
                        ("phase", number_value(0.25)),
                    ]),
                ),
            ])])
        };
        let rect = Rect {
            row: 0.0,
            col: 0.0,
            width: 32.0,
            height: 8.0,
        };
        let view_with = |ghost: Vec<(&str, Value)>| {
            let mut props = HashMap::from([
                ("items".to_string(), base_items()),
                ("view-start".to_string(), number_value(0.0)),
                ("view-duration".to_string(), number_value(64.0)),
                ("content-length".to_string(), number_value(32.0)),
            ]);
            for (key, value) in ghost {
                props.insert(key.to_string(), value);
            }
            TimelineView::from_props(&props, rect)
        };

        // Kind 1: rigid move — span slides, content untouched.
        let view = view_with(vec![
            ("ghost-kind", number_value(1.0)),
            ("ghost-id", number_value(7.0)),
            ("ghost-time", number_value(20.0)),
        ]);
        assert_eq!(view.items[0].start, 20.0);
        assert_eq!(view.items[0].end, 28.0);
        let Some(TimelineItemContent::Dots { cycle, phase, .. }) = &view.items[0].content else {
            panic!("dots content");
        };
        assert_eq!((*cycle, *phase), (0.5, 0.25));

        // Kind 3: end resize — clamped to content-length, cycle rescales so
        // the drawn repetition length stays the same in beats.
        let view = view_with(vec![
            ("ghost-kind", number_value(3.0)),
            ("ghost-id", number_value(7.0)),
            ("ghost-time", number_value(40.0)),
        ]);
        assert_eq!(view.items[0].end, 32.0, "end clamps to the song end");
        let Some(TimelineItemContent::Dots { cycle, .. }) = &view.items[0].content else {
            panic!("dots content");
        };
        // length_beats = 0.5 * 8 = 4 -> cycle over the 24-beat span = 1/6.
        assert!((cycle - 4.0 / 24.0).abs() < 1e-9);

        // Kind 2: start resize — the split rule re-anchors the phase:
        // delta = +4 beats over length_beats 4 -> phase 0.25 + 1 wraps to
        // 0.25.
        let view = view_with(vec![
            ("ghost-kind", number_value(2.0)),
            ("ghost-id", number_value(7.0)),
            ("ghost-time", number_value(12.0)),
        ]);
        assert_eq!(view.items[0].start, 12.0);
        let Some(TimelineItemContent::Dots { cycle, phase, .. }) = &view.items[0].content else {
            panic!("dots content");
        };
        assert!((phase - 0.25).abs() < 1e-9, "a whole-cycle trim wraps back");
        assert!((cycle - 1.0).abs() < 1e-9, "cycle rescales to the new span");

        // Kind 5: region move — every item intersecting the source rect
        // slides by the delta, and the highlight rect follows.
        let view = view_with(vec![
            ("ghost-kind", number_value(5.0)),
            ("ghost-time", number_value(4.0)),
            ("ghost-region-a", number_value(0.0)),
            ("ghost-region-b", number_value(32.0)),
        ]);
        assert_eq!(view.items[0].start, 12.0);
        assert_eq!(view.items[0].end, 20.0);
        let rect = view.selection_rect.as_ref().expect("region rect");
        assert_eq!((rect.time_a, rect.time_b), (4.0, 36.0));

        // Selection channels: selected-id / bound-id merge into the
        // selection set; negative means unset.
        let view = view_with(vec![("selected-id", number_value(7.0))]);
        assert!(view.selection.iter().any(|id| id == &number_value(7.0)));
        let view = view_with(vec![("selected-id", number_value(-1.0))]);
        assert!(view.selection.is_empty());
    }

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

    fn cursor_marker_instance(primitives: &[GpuPrimitive]) -> Option<WidgetInstance> {
        primitives.iter().find_map(|primitive| match primitive {
            GpuPrimitive::WidgetInstance {
                widget_type,
                instance,
                ..
            } if widget_type == "timeline-cursor-marker" => Some(*instance),
            _ => None,
        })
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
        let Some(TimelineItemContent::Dots {
            dots, cycle, phase, ..
        }) = &items[1].content
        else {
            panic!("expected dots content");
        };
        assert_eq!(dots.len(), 2, "malformed dot entries are skipped");
        assert_eq!(dots[0].offset, 0.25);
        assert_eq!(dots[0].value, 0.5);
        assert_eq!(dots[1].offset, 1.0, "offset clamps to 0..1");
        assert_eq!(dots[1].value, 0.0, "value clamps to 0..1");
        assert_eq!(*cycle, 1.0, "absent :cycle means content spans the item");
        assert_eq!(*phase, 0.0, "absent :phase starts at the source beginning");

        assert_eq!(items[2].kind, Some(TimelineItemKind::Audio));
        let Some(TimelineItemContent::Peaks(peaks)) = &items[2].content else {
            panic!("expected peaks content");
        };
        assert_eq!(peaks.len(), 1);

        assert!(items[3].kind.is_none(), "unknown kind parses to None");
        assert!(
            items[3].content.is_none(),
            "malformed content parses to None"
        );
    }

    /// `:cycle` is the source-cycle length relative to the item, so values
    /// above one represent partial-cycle clips. `:phase` wraps into 0..1.
    #[test]
    fn dots_content_parses_optional_cycle_and_phase() {
        let content_with_cycle = |cycle: Value, phase: Option<Value>| {
            let mut entries = vec![
                (
                    "dots",
                    list_value_raw(vec![map_value_raw(vec![
                        ("offset", number_value(0.5)),
                        ("value", number_value(0.5)),
                    ])]),
                ),
                ("cycle", cycle),
            ];
            if let Some(phase) = phase {
                entries.push(("phase", phase));
            }
            map_value_raw(entries)
        };
        let parsed = |cycle: Value, phase: Option<Value>| match parse_item_content(
            &content_with_cycle(cycle, phase),
        ) {
            Some(TimelineItemContent::Dots { cycle, phase, .. }) => (cycle, phase),
            _ => panic!("expected dots content"),
        };
        assert_eq!(parsed(number_value(0.25), None), (0.25, 0.0));
        assert_eq!(
            parsed(number_value(2.0), Some(number_value(0.75))),
            (2.0, 0.75)
        );
        assert_eq!(
            parsed(number_value(4.0), Some(number_value(-0.25))),
            (4.0, 0.75)
        );
        assert_eq!(
            parsed(number_value(0.0), None),
            (1.0, 0.0),
            "zero cycle degrades"
        );
        assert_eq!(
            parsed(number_value(-0.5), None),
            (1.0, 0.0),
            "negative cycle degrades"
        );
        assert_eq!(
            parsed(Value::String("junk".to_string()), None),
            (1.0, 0.0),
            "malformed values degrade"
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
        assert!(
            view.handle_scroll(MouseEventKind::ScrollDown, 10.0, 2.0)
                .is_none()
        );
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
        assert!(
            view.handle_scroll(MouseEventKind::ScrollDown, 10.0, 2.0)
                .is_some()
        );
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

        // 128 cells over 16 units = 8 cells per unit: wide enough that the
        // readability promotion in `grid_spec` resolves the shared grid at
        // the 1.0 step this scenario is written against.
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 128.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(35.2, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(38.2, 2.0, Some(&gesture))
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
            .handle_pointer_drag(41.6, 2.0, Some(&gesture))
            .expect("grid move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(5.0))
        );

        let action = view
            .handle_pointer_drag(38.2, 2.0, Some(&gesture))
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

        // 8 cells per unit so the promoted shared grid stays at step 1.0.
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 128.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(32.8, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(28.8, 2.0, Some(&gesture))
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
            .handle_pointer_drag(26.4, 2.0, Some(&gesture))
            .expect("backwards snapped move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(3.0))
        );

        let action = view
            .handle_pointer_drag(28.8, 2.0, Some(&gesture))
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

        // 8 cells per unit so the promoted shared grid stays at step 1.0.
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 128.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(34.4, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(33.2, 2.0, Some(&gesture))
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
            .handle_pointer_drag(31.6, 2.0, Some(&gesture))
            .expect("current-grid snapped move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(4.0))
        );

        let action = view
            .handle_pointer_drag(28.4, 2.0, Some(&gesture))
            .expect("current-grid sticky move action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("start").map(|value| value.borrow().clone()),
            Some(Value::Number(4.0))
        );

        let action = view
            .handle_pointer_drag(26.0, 2.0, Some(&gesture))
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
                animation: Default::default(),
            };
            build_primitives(&node, viewport)
        };
        // Last quad of a given colour covering a point — i.e. what the eye
        // actually sees there once everything has been painted.
        let top_color_at = |primitives: &[GpuPrimitive], x: f32, y: f32| {
            primitives
                .iter()
                .filter_map(|primitive| match primitive {
                    GpuPrimitive::Quad(quad)
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
        let marquee_blue = |primitives: &[GpuPrimitive]| {
            primitives.iter().any(|primitive| {
                matches!(primitive, GpuPrimitive::Quad(quad)
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
            matches!(primitive, GpuPrimitive::Quad(quad)
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
        assert!(
            marquee_blue(&marquee),
            "the default style keeps the marquee"
        );
        assert_eq!(
            top_color_at(&marquee, x_at(6.0), body_y),
            Some(CLIP),
            "the marquee style never repaints a clip body"
        );
    }

    /// A take stretched past its source end (`:wrap false`, cycle < 1 —
    /// e.g. the grow-resize ghost) must read as GROWTH: one pass of dots,
    /// no tiled repeats, no loop-boundary segmentation.
    #[test]
    fn non_wrapping_take_content_never_tiles_dots_or_loop_segments() {
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(1.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(12.0)),
                    (
                        "content",
                        map_value_raw(vec![
                            (
                                "dots",
                                list_value_raw(vec![map_value_raw(vec![
                                    ("offset", number_value(0.25)),
                                    ("value", number_value(0.5)),
                                ])]),
                            ),
                            ("cycle", number_value(0.5)),
                            ("wrap", Value::Bool(false)),
                        ]),
                    ),
                ])]),
            ),
            ("title-bar-height".to_string(), number_value(1.0)),
            ("item-corner-radius".to_string(), number_value(0.4)),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(0.0)),
        ]);
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
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "timeline".to_string(),
            rect,
            props: props.clone(),
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
        };

        let view = TimelineView::from_props(&node.props, rect);
        assert!(
            item_cycle_separator_xs(&view, &view.items[0], viewport).is_empty(),
            "a non-wrapping item has no repeat play-throughs to cue"
        );

        let primitives = build_primitives(&node, viewport);
        let dot_quads: Vec<_> = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                GpuPrimitive::Quad(quad)
                    if (quad.color.a - 0.78).abs() < f32::EPSILON
                        && (quad.color.r - 0.02).abs() < f32::EPSILON =>
                {
                    Some(quad)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            dot_quads.len(),
            1,
            "one source pass only — a second tiled copy would read as looping"
        );
        // The single pass lands at source offset 0.25 of the first (and
        // only) cycle: item offset 0.125 → beat 5 → x 50.
        assert!(
            (dot_quads[0].x - 50.0).abs() < 1.0,
            "the dot stays at its absolute source position, got x {}",
            dot_quads[0].x
        );
    }

    #[test]
    fn title_barred_loop_clip_uses_abutting_rounded_fill_segments() {
        const CLIP: crate::backend::Color = crate::backend::Color {
            r: 0.9,
            g: 0.45,
            b: 0.15,
            a: 1.0,
        };
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(1.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(12.0)),
                    (
                        "content",
                        map_value_raw(vec![
                            ("dots", list_value_raw(Vec::new())),
                            ("cycle", number_value(0.25)),
                        ]),
                    ),
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
            ("item-corner-radius".to_string(), number_value(0.4)),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(0.0)),
        ]);
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
            animation: Default::default(),
        };

        let primitives = build_primitives(&node, viewport);
        let clip_grid_lines: Vec<_> = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                GpuPrimitive::Quad(quad)
                    if quad.width == 0.125
                        && (quad.color.r - 0.02).abs() < f32::EPSILON
                        && (quad.color.g - 0.025).abs() < f32::EPSILON
                        && (quad.color.b - 0.03).abs() < f32::EPSILON
                        && quad.color.a < 0.3 =>
                {
                    Some(quad)
                }
                _ => None,
            })
            .collect();

        assert!(
            !clip_grid_lines.is_empty(),
            "the clip should contain at least one continued grid line"
        );
        assert!(
            clip_grid_lines
                .iter()
                .all(|line| line.y == 1.0 && line.height == 3.0),
            "continued grid lines must cover only the body below the one-cell title bar"
        );
        let clip_fill_covers = |primitives: &[GpuPrimitive], x: f32, y: f32| {
            primitives.iter().any(|primitive| {
                matches!(primitive, GpuPrimitive::Quad(quad)
                    if quad.color == CLIP
                        && x >= quad.x
                        && x < quad.x + quad.width
                        && y >= quad.y
                        && y < quad.y + quad.height)
            })
        };
        // The first repeat boundary is beat 6 => x 60 in this ten-cells/beat
        // fixture. The two fills have the exact same x edge: their rounded
        // corners expose the continuous dark outline only at top and bottom,
        // while their straight middle spans meet without a gap or separator.
        assert!(
            !clip_fill_covers(&primitives, 60.0, 0.10),
            "the adjacent top corners must reveal the loop boundary"
        );
        assert!(
            clip_fill_covers(&primitives, 59.99, 2.0) && clip_fill_covers(&primitives, 60.0, 2.0),
            "abutting segment fills must meet continuously through the clip body"
        );
        assert!(
            clip_fill_covers(&primitives, 60.8, 0.20),
            "the title bar must remain filled beside the rounded corner"
        );
        assert!(
            !clip_fill_covers(&primitives, 60.0, 3.90),
            "the adjacent bottom corners must reveal the same loop boundary"
        );
        assert!(
            !primitives.iter().any(|primitive| {
                matches!(primitive, GpuPrimitive::Quad(quad)
                    if quad.width == 0.125
                        && (quad.color.a - 0.42).abs() < f32::EPSILON)
            }),
            "rounded fill segments replace the explicit repeat rule"
        );
        assert!(
            primitives.len() < 1_000,
            "four ordinary rounded segments should stay a small fixed set of quads, got {}",
            primitives.len()
        );

        // At a sufficiently far zoom-out each source cycle is below the
        // existing 5px legibility threshold. The clip then becomes one
        // ordinary rounded fill instead of producing hundreds of tiny arcs.
        let zoomed_out_rect = Rect { width: 3.0, ..rect };
        let zoomed_out_view = TimelineView::from_props(&node.props, zoomed_out_rect);
        assert!(
            item_cycle_separator_xs(&zoomed_out_view, &zoomed_out_view.items[0], viewport)
                .is_empty(),
            "sub-5px loop cycles must collapse to one continuous fill"
        );
    }

    #[test]
    fn item_label_is_scissored_to_its_visible_title_bar() {
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(1.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(4.5)),
                    ("label", Value::String("Pattern 123456789".to_string())),
                ])]),
            ),
            (
                "item-label-color".to_string(),
                list_value_raw(vec![
                    number_value(0.1),
                    number_value(0.2),
                    number_value(0.3),
                ]),
            ),
            ("item-label-font-size".to_string(), number_value(13.0)),
            ("title-bar-height".to_string(), number_value(1.0)),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(0.0)),
        ]);
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
            animation: Default::default(),
        };

        let primitives = build_primitives(&node, viewport);
        let label_run = primitives
            .windows(3)
            .find_map(|window| match window {
                [
                    GpuPrimitive::PushClipRect(clip),
                    GpuPrimitive::ProportionalText(text),
                    GpuPrimitive::PopClipRect,
                ] if text.text == "Pattern 123456789" => Some((*clip, text)),
                _ => None,
            })
            .expect("label bracketed by a clip rect");

        // At 10 cells/beat, this half-beat clip is only five cells wide,
        // much narrower than its label. The scissor is exactly the visible
        // title bar, so the backend cuts the glyph run at the clip edge.
        assert_eq!(
            label_run.0,
            Rect {
                row: 0.0,
                col: 40.0,
                width: 5.0,
                height: 1.0,
            }
        );
        assert_eq!(label_run.1.font_size, 13.0);
        assert_eq!(
            label_run.1.fg,
            crate::backend::Color {
                r: 0.1,
                g: 0.2,
                b: 0.3,
                a: 1.0,
            }
        );
        assert!(
            TIMELINE_WIDGET
                .bindable_props()
                .contains(&"item-label-font-size")
        );
        assert!(
            TIMELINE_WIDGET
                .bindable_props()
                .contains(&"item-label-color")
        );
    }

    #[test]
    fn background_color_prop_styles_the_timeline_lane() {
        let color = crate::backend::Color {
            r: 0.12,
            g: 0.16,
            b: 0.22,
            a: 1.0,
        };
        let props = HashMap::from([
            (
                "background-color".to_string(),
                list_value_raw(vec![
                    number_value(color.r as f64),
                    number_value(color.g as f64),
                    number_value(color.b as f64),
                ]),
            ),
            ("header-height".to_string(), number_value(0.0)),
        ]);
        let rect = Rect {
            row: 0.0,
            col: 0.0,
            width: 12.0,
            height: 4.0,
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
            animation: Default::default(),
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

        let primitives = build_primitives(&node, viewport);
        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, GpuPrimitive::Quad(quad)
                if quad.x == rect.col
                    && quad.y == rect.row
                    && quad.width == rect.width
                    && quad.height == rect.height
                    && quad.color == color)
        }));
        assert!(
            TIMELINE_WIDGET
                .bindable_props()
                .contains(&"background-color")
        );
    }

    #[test]
    fn dot_content_phase_keeps_partial_cycle_notes_aligned() {
        let dots = list_value_raw(vec![
            map_value_raw(vec![
                ("offset", number_value(0.0)),
                ("value", number_value(0.1)),
            ]),
            map_value_raw(vec![
                ("offset", number_value(0.25)),
                ("value", number_value(0.2)),
            ]),
            map_value_raw(vec![
                ("offset", number_value(0.5)),
                ("value", number_value(0.7)),
            ]),
            map_value_raw(vec![
                ("offset", number_value(0.75)),
                ("value", number_value(0.8)),
            ]),
        ]);
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(1.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(0.0)),
                    ("end", number_value(2.0)),
                    (
                        "content",
                        map_value_raw(vec![
                            ("dots", dots),
                            // A four-beat pattern inside a two-beat clip.
                            ("cycle", number_value(2.0)),
                            // The clip starts at source step 12 of 16.
                            ("phase", number_value(0.75)),
                        ]),
                    ),
                ])]),
            ),
            ("header-height".to_string(), number_value(0.0)),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(4.0)),
        ]);
        let rect = Rect {
            row: 0.0,
            col: 0.0,
            width: 40.0,
            height: 4.0,
        };
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            parent_subtree_root_id: None,
            subtree_root_id: None,
            stable_key: None,
            widget_type: "timeline".to_string(),
            rect,
            props,
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
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

        let primitives = build_primitives(&node, viewport);
        let dot_color = crate::backend::Color {
            r: 0.02,
            g: 0.025,
            b: 0.03,
            a: 0.78,
        };
        let rendered = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                GpuPrimitive::Quad(quad) if quad.color == dot_color => Some((quad.x, quad.y)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rendered.len(),
            2,
            "only the played source window is visible"
        );
        assert!((rendered[0].0 - 0.0).abs() < 0.001);
        assert!((rendered[0].1 - 0.77).abs() < 0.001);
        assert!((rendered[1].0 - 10.0).abs() < 0.001);
        assert!((rendered[1].1 - 3.465).abs() < 0.001);
    }

    #[test]
    fn metal_cursor_marker_starts_below_ruler_with_antialiased_marker() {
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
            animation: Default::default(),
        };

        let primitives = build_primitives(&node, viewport);
        let marker = cursor_marker_instance(&primitives).expect("cursor marker");
        assert_eq!(marker.color_a, theme::CURSOR().to_rgba());
        assert_eq!(marker.uniform_a[2], 8.0, "pixel-aligned marker width");
        assert_eq!(marker.uniform_a[3], 6.0, "pixel-aligned marker height");
        let marker_top_px = (1.0 - marker.ndc_min[1]) * viewport.vp_h * 0.5 + marker.uniform_b[0];
        let marker_tip_row = (marker_top_px + marker.uniform_a[3]) / viewport.cell_h;
        assert!((marker_top_px - 40.0).abs() < 0.001);

        let line = primitives
            .iter()
            .find_map(|primitive| match primitive {
                GpuPrimitive::Quad(quad)
                    if quad.color == theme::CURSOR() && (quad.y - marker_tip_row).abs() < 0.001 =>
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
    fn metal_cursor_marker_and_line_can_render_independently_with_scaled_sdf_marker() {
        let props = HashMap::from([
            ("cursor-time".to_string(), number_value(4.0)),
            ("cursor-marker-visible".to_string(), bool_value(true)),
            ("cursor-line-visible".to_string(), bool_value(false)),
            ("cursor-marker-scale".to_string(), number_value(1.6)),
            ("cursor-marker-width-scale".to_string(), number_value(1.5)),
            ("cursor-marker-height-scale".to_string(), number_value(0.7)),
            ("header-height".to_string(), number_value(2.0)),
            ("header-bottom-gutter".to_string(), number_value(1.0)),
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
        let mut node = LayoutNode {
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
            animation: Default::default(),
        };

        let marker_only = build_primitives(&node, viewport);
        let marker = cursor_marker_instance(&marker_only).expect("scaled cursor marker");
        assert_eq!(marker.uniform_a[2], 19.0, "80%-scaled, pixel-aligned width");
        assert_eq!(
            marker.uniform_a[3], 6.0,
            "80% overall scale plus 30% height reduction"
        );
        let marker_top_px = (1.0 - marker.ndc_min[1]) * viewport.vp_h * 0.5 + marker.uniform_b[0];
        let marker_tip_row = (marker_top_px + marker.uniform_a[3]) / viewport.cell_h;
        assert!((marker_top_px / viewport.cell_h - 1.35).abs() < 0.001);
        assert!(
            marker_tip_row < 2.0,
            "the gutter keeps the marker above scene content"
        );
        assert!(
            !marker_only.iter().any(|primitive| matches!(
                primitive,
                GpuPrimitive::Quad(quad) if quad.color == theme::CURSOR()
            )),
            "marker-only mode must not draw a cursor line"
        );

        node.props
            .insert("cursor-marker-visible".to_string(), bool_value(false));
        node.props
            .insert("cursor-line-visible".to_string(), bool_value(true));
        let line_only = build_primitives(&node, viewport);
        assert!(
            cursor_marker_instance(&line_only).is_none(),
            "line-only mode must not draw a cursor marker"
        );
        let line = line_only
            .iter()
            .find_map(|primitive| match primitive {
                GpuPrimitive::Quad(quad) if quad.color == theme::CURSOR() => Some(*quad),
                _ => None,
            })
            .expect("cursor line");
        assert_eq!(line.y, 2.0);
        assert_eq!(line.height, 10.0);
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

        // 8 cells per unit so the promoted shared grid stays at step 1.0.
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 128.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(40.0, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(43.2, 2.0, Some(&gesture))
            .expect("fractional resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        let Some(Value::Number(time)) = map.get("time").map(|value| value.borrow().clone()) else {
            panic!("expected fractional resize time");
        };
        assert!((time - 5.4).abs() < 0.0001, "time was {time}");

        let action = view
            .handle_pointer_drag(48.8, 2.0, Some(&gesture))
            .expect("snapped resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(6.0))
        );

        let action = view
            .handle_pointer_drag(43.2, 2.0, Some(&gesture))
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

        // 8 cells per unit so the promoted shared grid stays at step 1.0.
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 128.0,
                height: 8.0,
            },
        );

        let gesture = view.begin_gesture(40.0, 2.0).expect("gesture");
        let action = view
            .handle_pointer_drag(38.8, 2.0, Some(&gesture))
            .expect("fractional resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        let Some(Value::Number(time)) = map.get("time").map(|value| value.borrow().clone()) else {
            panic!("expected fractional resize time");
        };
        assert!((time - 5.1).abs() < 0.0001, "time was {time}");

        let action = view
            .handle_pointer_drag(37.6, 2.0, Some(&gesture))
            .expect("current-grid snapped resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(5.0))
        );

        let action = view
            .handle_pointer_drag(34.4, 2.0, Some(&gesture))
            .expect("current-grid sticky resize action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("time").map(|value| value.borrow().clone()),
            Some(Value::Number(5.0))
        );

        let action = view
            .handle_pointer_drag(31.2, 2.0, Some(&gesture))
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

    /// Clip-edit-target spec 5.1: with band-slide enabled, a drag starting
    /// inside the loop band slides the window — the drag frames report the
    /// running delta and the release carries the TOTAL delta for the host's
    /// single undoable phase edit. Without the prop, the header still scrubs.
    #[test]
    fn band_slide_gesture_reports_running_and_total_delta() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(2.0)),
            ("content-length".to_string(), number_value(16.0)),
            ("band-slide".to_string(), Value::Bool(true)),
            (
                "lanes".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(0.0)),
                    ("label", Value::String("L0".to_string())),
                ])]),
            ),
        ]);
        let rect = Rect {
            row: 0.0,
            col: 0.0,
            width: 32.0,
            height: 8.0,
        };
        let view = TimelineView::from_props(&props, rect);
        // With header-height 2 the band occupies rows [1.1, 1.92): press
        // INSIDE the band body at time 4 (col 8).
        let gesture = view.begin_gesture(8.0, 1.5).expect("band gesture begins");
        let Value::Map(map) = &gesture else {
            panic!("expected gesture map");
        };
        assert_eq!(
            map.get("kind").map(|value| value.borrow().clone()),
            Some(Value::Keyword("slide-band".to_string()))
        );
        assert_eq!(
            map.get("anchor-time").map(|value| value.borrow().clone()),
            Some(Value::Number(4.0))
        );

        let action = view
            .handle_pointer_drag(14.0, 1.5, Some(&gesture))
            .expect("drag reports delta");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("slide-band".to_string()))
        );
        assert_eq!(
            map.get("delta-time").map(|value| value.borrow().clone()),
            Some(Value::Number(3.0))
        );

        let action = view
            .handle_pointer_up(14.0, 1.5, Some(&gesture))
            .expect("release reports total delta");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("finish-slide-band".to_string()))
        );
        assert_eq!(
            map.get("delta-time").map(|value| value.borrow().clone()),
            Some(Value::Number(3.0))
        );

        // The RULER rows above the band keep scrubbing even with the prop
        // on (spec 5.1: band-BODY drag, not the whole header).
        let gesture = view.begin_gesture(8.0, 0.5).expect("ruler press begins");
        let Value::Map(map) = &gesture else {
            panic!("expected gesture map");
        };
        assert_eq!(
            map.get("kind").map(|value| value.borrow().clone()),
            Some(Value::Keyword("scrub".to_string()))
        );

        // Without the prop, a band-body press scrubs as before.
        let mut plain = props.clone();
        plain.remove("band-slide");
        let view = TimelineView::from_props(&plain, rect);
        let gesture = view.begin_gesture(8.0, 1.5).expect("scrub begins");
        let Value::Map(map) = &gesture else {
            panic!("expected gesture map");
        };
        assert_eq!(
            map.get("kind").map(|value| value.borrow().clone()),
            Some(Value::Keyword("scrub".to_string()))
        );
    }

    /// Clip-edit-target spec 4 (locked decision 5): a double-click on an
    /// item's TITLE BAR emits :double-click-item; the body stays inert so a
    /// body double-click starts nothing surprising.
    #[test]
    fn double_click_on_item_title_bar_emits_double_click_item() {
        let props = HashMap::from([
            ("tool".to_string(), keyword_value("pointer")),
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("snap".to_string(), number_value(1.0)),
            ("title-bar-height".to_string(), number_value(0.9)),
            ("double-click-items".to_string(), Value::Bool(true)),
            ("header-height".to_string(), number_value(0.0)),
            ("lane-height".to_string(), number_value(3.0)),
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
                    ("id", number_value(7.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(12.0)),
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
        // The item spans cols 8..24; col 16 is well clear of both edge
        // handles. Row 0.2 is inside the 0.9-row title bar.
        let action = view
            .handle_double_click(16.0, 0.2)
            .expect("title-bar double-click emits an action");
        let Value::Map(map) = action else {
            panic!("expected action map");
        };
        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("double-click-item".to_string()))
        );
        let Some(ids) = map.get("ids").map(|value| value.borrow().clone()) else {
            panic!("expected ids");
        };
        let Value::List(ids) = ids else {
            panic!("expected id list");
        };
        assert_eq!(ids.len(), 1);
        assert_eq!(*ids[0].borrow(), Value::Number(7.0));

        // The BODY (below the title bar) still returns no action.
        assert!(view.handle_double_click(16.0, 2.0).is_none());
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
        let snapped = single_lane_marquee_view(vec![("marquee-snap", keyword_value("grid"))]);
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

    /// Bar-less hosts keep the forgiving grips they always had: the narrow
    /// Ableton-style clamp is a title-bar affordance only. The 8-cell item's
    /// end handle is `8 * 0.24 = 1.92` cells wide here; with the clip clamp it
    /// would be 1.25 and col 10.5 would read as body.
    #[test]
    fn hit_test_without_a_title_bar_keeps_the_wide_end_grip() {
        let view = title_bar_view(None);
        assert_eq!(hit_name(&view, 10.5, 4.0), "edge-end");
        // The title-barred variant of the same item tightens to 1.25 cells.
        let clip_view = title_bar_view(Some(2.0));
        assert_eq!(hit_name(&clip_view, 10.5, 0.5), "title-bar");
        assert_eq!(hit_name(&clip_view, 11.5, 0.5), "edge-end");
    }

    /// A clip that begins left of the view has its rect clamped to the
    /// content's left edge, so that edge is NOT the clip's start. Offering a
    /// start grip there would trim an off-screen boundary on what looks like
    /// a plain title-bar drag, so the bar stays a move surface.
    #[test]
    fn scrolled_off_clip_start_offers_no_start_handle() {
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value_raw(vec![map_value_raw(vec![
                    ("id", number_value(1.0)),
                    ("lane", number_value(0.0)),
                    ("start", number_value(4.0)),
                    ("end", number_value(20.0)),
                ])]),
            ),
            ("view-start".to_string(), number_value(8.0)),
            ("view-duration".to_string(), number_value(16.0)),
            ("header-height".to_string(), number_value(0.0)),
            ("title-bar-height".to_string(), number_value(2.0)),
        ]);
        let view = TimelineView::from_props(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 16.0,
                height: 8.0,
            },
        );
        // Left edge of the content, on the title bar: a move, not a resize.
        assert_eq!(hit_name(&view, 0.1, 0.5), "title-bar");
        assert_eq!(hit_name(&view, 1.0, 0.5), "title-bar");
        // The end edge IS on screen (beat 20 -> col 12), so it keeps its grip.
        assert_eq!(hit_name(&view, 11.8, 0.5), "edge-end");
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
            Rect {
                row: 0.0,
                col: 0.0,
                width: 16.0,
                height: 8.0,
            },
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
        assert!(marquee_from_item_body(
            &get_map(&body).expect("gesture map")
        ));

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
    /// that is about to start (region spec 3.1/4.4). The action still names
    /// the body clip so a host's explicit clip-editor mode can select it.
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
        let Value::List(ids) = map.get("ids").expect("body clip ids").borrow().clone() else {
            panic!("body clip ids must be a list");
        };
        assert_eq!(
            ids.first().map(|id| id.borrow().clone()),
            Some(Value::Number(1.0)),
            "the host can promote a body press to clip selection in clip-editor mode"
        );

        // Without a title bar (piano roll) a body press selects, as always.
        let without_bar = title_bar_view(None);
        assert_eq!(
            action_type(
                &without_bar
                    .handle_pointer_down(8.0, 5.0)
                    .expect("body press")
            ),
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
        props.insert(
            "selection".to_string(),
            list_value_raw(vec![number_value(1.0)]),
        );
        let action = escape(&TimelineView::from_props(&props, rect)).expect("item escape");
        assert_eq!(action_type(&action), "clear-selection");
    }

    /// `:grid-density` requests a finer initial candidate. When that would
    /// crowd the view, the shared rendering/editing grid promotes it back to
    /// a readable power-of-two interval.
    #[test]
    fn grid_density_never_crowds_or_moves_bar_lines() {
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
            let dense = step_for(Some(2.0), duration);
            assert!(
                dense == stock || dense == stock / 2.0,
                "density 2 may select one finer rung only when it remains readable \
                 (stock {stock}, dense {dense}, view {duration})"
            );
            // Every resolved rung divides or multiplies the 4-beat bar, so
            // bar lines survive at any density.
            let ratio = 4.0 / dense;
            assert!(
                (ratio - ratio.round()).abs() < 1e-9 || (dense / 4.0).fract() < 1e-9,
                "step {dense} must stay bar-aligned at view {duration}"
            );
        }
        // Clamped to something sane rather than trusted blindly.
        assert_eq!(step_for(Some(0.1), 64.0), step_for(None, 64.0));
    }

    #[test]
    fn zoomed_out_cursor_and_marquee_share_the_sparse_visible_grid() {
        let props = HashMap::from([
            ("view-start".to_string(), number_value(0.0)),
            ("view-duration".to_string(), number_value(512.0)),
            ("grid-density".to_string(), number_value(2.0)),
            ("marquee-snap".to_string(), keyword_value("grid")),
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
                width: 96.0,
                height: 8.0,
            },
        );

        assert_eq!(
            view.alignment_helper_grid_step(),
            32.0,
            "the visible grid is one mark every eight bars"
        );
        assert_eq!(
            view.cursor_snap_time(70.0),
            64.0,
            "cursor placement uses the same eight-bar interval"
        );
        assert_eq!(
            view.marquee_span(70.0, 140.0),
            (64.0, 160.0),
            "selection bounds floor/ceil to that same interval"
        );
    }

    /// docs/arrangement-region-editing-spec.md 3.2: `:width` on a dot is
    /// optional and lenient — absent/malformed means the legacy point dot.
    #[test]
    fn dots_parse_optional_width_leniently() {
        let content = |width: Option<Value>| {
            let mut dot = vec![("offset", number_value(0.25)), ("value", number_value(0.5))];
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
