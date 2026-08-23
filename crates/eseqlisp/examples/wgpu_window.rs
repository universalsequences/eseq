//! Render a real eseqlisp widget tree through the retained primitive-run wgpu path.
//!
//! Run with: `cargo run -p eseqlisp --example wgpu_window --features wgpu`
//! Set `ESEQ_WGPU_FRAMES=<n>` to present `n` frames and exit (0 = run until the
//! window is closed), so the spike doubles as a non-interactive smoke check.
//!
//! Note on what shows up: a `box` with a `background` prop compiles to an SDF
//! `WidgetInstance`, not a solid `Rect`, so the panel chrome stays unpainted
//! until the SDF pipelines are ported (eseq-linux.7). What this renderer draws
//! is the solid `Rect`/`Quad` geometry the leaf widgets emit — and the boxes'
//! `PushClipRect`/`PopClipRect` pairs still clip it.

use std::collections::HashMap;
use std::sync::Arc;

use eseqlisp::backend::Color;
use eseqlisp::layout::{LayoutNode, Rect};
use eseqlisp::vm::Value;
use eseqlisp::wgpu_backend::WgpuBackend;
use eseqlisp::widget_render::{
    GpuPrimitive, GpuPrimitiveRun, WidgetViewport, collect_gpu_primitive_runs,
    collect_gpu_primitive_runs_retained, innermost_primitive,
};
use winit::dpi::PhysicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::WindowBuilder;

const CELL_W: f32 = 12.0;
const CELL_H: f32 = 24.0;

fn node(id: u64, widget_type: &str, rect: Rect, props: Vec<(&str, &str)>) -> LayoutNode {
    LayoutNode {
        widget_id: id,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: widget_type.into(),
        rect,
        props: props
            .into_iter()
            .map(|(key, value)| (key.to_string(), Value::String(value.to_string())))
            .collect::<HashMap<_, _>>(),
        children: vec![],
        focusable: false,
        animation: Default::default(),
    }
}

fn widget_tree() -> LayoutNode {
    // The panel box emits PushClipRect/PopClipRect around its children.
    let mut panel = node(
        1,
        "box",
        Rect {
            row: 1.0,
            col: 2.0,
            width: 42.0,
            height: 20.0,
        },
        vec![("background", "panel")],
    );
    panel.children = vec![
        node(
            2,
            "scope",
            Rect {
                row: 3.0,
                col: 5.0,
                width: 18.0,
                height: 6.0,
            },
            vec![
                ("background-color", "#12303a"),
                ("waveform-color", "#36c5a3"),
            ],
        ),
        // This deliberately overhangs the panel on the right. The panel's
        // production clip rect must cut it off at column 44.
        node(
            3,
            "scope",
            Rect {
                row: 12.0,
                col: 28.0,
                width: 24.0,
                height: 6.0,
            },
            vec![
                ("background-color", "#3a1d1a"),
                ("waveform-color", "#e46f61"),
            ],
        ),
    ];
    panel
}

fn viewport(size: PhysicalSize<u32>) -> WidgetViewport {
    WidgetViewport {
        cell_w: CELL_W,
        cell_h: CELL_H,
        vp_w: size.width as f32,
        vp_h: size.height as f32,
        time_seconds: 0.0,
        focused_widget_id: None,
        focused_branch: false,
        overlay_viewport_bottom: size.height as f32 / CELL_H,
        scroll_top: 0.0,
        scroll_left: 0.0,
        inherited_hover: false,
    }
}

fn collect(tree: &LayoutNode, size: PhysicalSize<u32>) -> Vec<GpuPrimitiveRun> {
    collect_gpu_primitive_runs(
        tree,
        viewport(size),
        0.0,
        (size.height / CELL_H as u32) as u16,
    )
    .0
}

/// Count what this renderer can actually paint, so an empty window is
/// immediately distinguishable from a broken one.
fn drawable_primitive_count(runs: &[GpuPrimitiveRun]) -> usize {
    runs.iter()
        .flat_map(|run| run.primitives.iter())
        .filter(|primitive| {
            matches!(
                innermost_primitive(primitive),
                GpuPrimitive::Rect(_) | GpuPrimitive::ForegroundRect(_) | GpuPrimitive::Quad(_)
            )
        })
        .count()
}

fn main() {
    let frame_limit = std::env::var("ESEQ_WGPU_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("eseqlisp — wgpu primitive runs")
            .with_inner_size(PhysicalSize::new(900, 600))
            .build(&event_loop)
            .expect("failed to create window"),
    );
    let mut backend =
        pollster::block_on(WgpuBackend::new(window.clone())).expect("failed to initialize wgpu");
    let tree = widget_tree();
    let mut runs = collect(&tree, window.inner_size());
    let mut presented_frames = 0;

    let drawable = drawable_primitive_count(&runs);
    println!(
        "{} primitive runs, {drawable} solid rect/quad primitives to draw",
        runs.len()
    );
    assert!(
        drawable > 0,
        "the spike tree produced no rect/quad geometry — the window would only show the clear color"
    );

    event_loop
        .run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Wait);
            match event {
                Event::WindowEvent { window_id, event } if window_id == window.id() => {
                    match event {
                        WindowEvent::CloseRequested => elwt.exit(),
                        WindowEvent::Resized(size) => {
                            backend.resize(size);
                            runs = collect(&tree, size);
                            window.request_redraw();
                        }
                        WindowEvent::RedrawRequested => {
                            let status = backend
                                .render_primitive_runs(
                                    &runs,
                                    CELL_W,
                                    CELL_H,
                                    Color::rgb(0.035, 0.04, 0.055),
                                )
                                .expect("wgpu frame failed");
                            if status == eseqlisp::wgpu_backend::WgpuRenderStatus::Presented {
                                presented_frames += 1;
                                if frame_limit != 0 && presented_frames >= frame_limit {
                                    println!("presented {presented_frames} frames — exiting");
                                    elwt.exit();
                                }
                            }
                            let (retained, _, _) = collect_gpu_primitive_runs_retained(
                                &tree,
                                viewport(window.inner_size()),
                                0.0,
                                (window.inner_size().height / CELL_H as u32) as u16,
                                &runs,
                                &[],
                            );
                            runs = retained;
                        }
                        _ => {}
                    }
                }
                Event::AboutToWait => window.request_redraw(),
                _ => {}
            }
        })
        .expect("event loop failed");
}
