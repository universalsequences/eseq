#![allow(
    dead_code,
    clippy::approx_constant,
    clippy::arc_with_non_send_sync,
    clippy::borrow_deref_ref,
    clippy::collapsible_str_replace,
    clippy::collapsible_if,
    clippy::derivable_impls,
    clippy::doc_overindented_list_items,
    clippy::empty_line_after_doc_comments,
    clippy::excessive_precision,
    clippy::get_first,
    clippy::implicit_saturating_sub,
    clippy::if_same_then_else,
    clippy::items_after_test_module,
    clippy::large_enum_variant,
    clippy::manual_clamp,
    clippy::manual_contains,
    clippy::manual_div_ceil,
    clippy::manual_ignore_case_cmp,
    clippy::manual_is_multiple_of,
    clippy::manual_memcpy,
    clippy::manual_ok_err,
    clippy::manual_range_contains,
    clippy::manual_repeat_n,
    clippy::map_flatten,
    clippy::map_entry,
    clippy::missing_safety_doc,
    clippy::missing_const_for_thread_local,
    clippy::needless_borrows_for_generic_args,
    clippy::needless_lifetimes,
    clippy::needless_return,
    clippy::needless_range_loop,
    clippy::new_without_default,
    clippy::not_unsafe_ptr_arg_deref,
    clippy::option_map_unit_fn,
    clippy::question_mark,
    clippy::redundant_closure,
    clippy::should_implement_trait,
    clippy::single_element_loop,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_cast,
    clippy::useless_vec
)]

// ── Domain modules ───────────────────────────────────────────────────────
pub mod lang;
pub mod ui;

// Re-export submodules at crate root for backward-compatible paths
// (e.g. crate::vm, crate::layout still work).
pub use lang::compiler;
pub use lang::modules;
pub use lang::parser;
pub use lang::vm;

#[cfg(feature = "capture-harness")]
pub(crate) mod capture_scenes;
#[cfg(all(target_os = "macos", feature = "capture-harness"))]
pub mod metal_shader_capture;
#[cfg(all(feature = "wgpu", feature = "capture-harness"))]
pub mod shader_capture;

pub use ui::backend;
pub use ui::frame;
pub use ui::glyph_atlas;
pub use ui::layout;
pub use ui::metal_backend;
pub use ui::theme;
pub use ui::tui;
#[cfg(feature = "wgpu")]
pub use ui::wgpu_app;
#[cfg(feature = "wgpu")]
pub use ui::wgpu_backend;

// ── Root-level modules ───────────────────────────────────────────────────
pub mod audio;
pub mod buffer;
pub(crate) mod clipboard;
pub mod defmacro_library;
mod dgenlisp;
pub mod editor;
pub mod host;
pub mod hot_reload;
pub mod live_audio;
pub mod mode;
pub mod module_alias_migration;
pub mod module_export_migration;
pub mod package;
pub mod reactive;
pub mod runtime;
pub mod sound_glyph_data;
pub mod text;
pub mod text_capture;
pub mod tile;
pub mod widget_render;
pub mod widgets;

/// Factory Lisp shipped at the repository or bundle content root. Standalone
/// eseqlisp tools use this source-tree mapping; embedding applications should
/// also pass the returned path as `EditorConfig::init_source_path` so relative
/// loads (such as `./themes.lisp`) resolve beside it.
pub fn factory_core_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("ESEQ_DEV_MANIFEST_DIR")).join("../../content/core")
}

pub fn factory_init_path() -> std::path::PathBuf {
    factory_core_dir().join("init.lisp")
}

pub fn factory_init_source() -> String {
    std::fs::read_to_string(factory_init_path()).unwrap_or_default()
}

use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
};
use ratatui::DefaultTerminal;

use vm::{VMError, Value};

pub use editor::{Editor, EditorConfig, EditorError, EditorExit};
pub use host::{BufferId, CompileKind, HostCommand, HostEvent};
pub use hot_reload::{ModuleLoadRoot, ReloadReport, SourceOverlay, SourceSnapshot};
pub use mode::BufferMode;
pub use runtime::{NativeContext, NativeResult, Runtime, RuntimeError, SymbolMetadata};

#[allow(dead_code)]
pub fn run_prog(prog: &str) -> Result<Option<Value>, VMError> {
    let mut runtime = Runtime::new();
    runtime.eval_str(prog)
}

pub fn run_editor(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let init_src = factory_init_source();
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
            ..EditorConfig::default()
        },
    );

    loop {
        editor.update_timers();
        if editor.visible_widgets_animating() {
            editor.mark_needs_redraw();
        }
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) => editor.handle_key(key),
                Event::Mouse(mouse) => {
                    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                    editor.update_tile_rects(cols, rows);
                    editor.handle_tiled_mouse_precise(
                        mouse,
                        mouse.column as f32,
                        mouse.row as f32,
                        1, // TUI: cell-based borders
                    );
                }
                Event::Resize(_, _) => editor.mark_needs_redraw(),
                _ => {}
            }
        }

        if editor.needs_redraw() {
            terminal.draw(|f| {
                let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                let tiled_frame =
                    frame::build_tiled_render_frame(&mut editor, cols as usize, rows as usize);
                tui::render_tiled(f, &tiled_frame);
            })?;
            editor.clear_needs_redraw();
        }

        if editor.should_quit() {
            break;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn run_metal() -> Result<(), backend::BackendError> {
    use backend::Backend;
    use metal_backend::MetalBackend;

    let init_src = factory_init_source();
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
            ..EditorConfig::default()
        },
    );
    let mut backend = MetalBackend::new()?;
    backend.initialize()?;

    // Set up proportional text measurement for widget layout.
    {
        let (cell_w, cell_h) = backend.cell_dimensions();
        if let Some((text_cell_w, text_cell_h)) = backend.sync_text_zoom(editor.text_zoom()) {
            editor.set_text_cell_dimensions(cell_w, cell_h, text_cell_w, text_cell_h);
        }
        if let Some(measurer) = backend.create_text_measurer() {
            editor.set_text_measurer(measurer, cell_w, cell_h);
        }
    }

    let idle_frame_interval = Duration::from_secs_f64(1.0 / 30.0);
    let animation_frame_interval = Duration::from_secs_f64(1.0 / 60.0);
    let mut last_render_at = Instant::now() - idle_frame_interval;
    let mut pending_drag: Option<(Event, (f32, f32))> = None;
    // Edge-tracked so the un-hide fires even when the gesture is dropped for a
    // reason other than mouse-up (editor reload, tile close, focus loss).
    let mut hidden_drag_started = false;
    #[allow(unused_mut)]
    let mut scroll_accum_y: f32 = 0.0;
    #[allow(unused_mut)]
    let mut scroll_accum_x: f32 = 0.0;

    loop {
        editor.update_timers();
        let (cols, rows) = backend.viewport_size();
        // Set aspect ratio for uniform spacing (cell_h / cell_w)
        let (cell_w, cell_h) = backend.cell_dimensions();
        editor.set_layout_cell_dimensions(cell_w, cell_h);
        if let Some((text_cell_w, text_cell_h)) = backend.sync_text_zoom(editor.text_zoom()) {
            editor.set_text_cell_dimensions(cell_w, cell_h, text_cell_w, text_cell_h);
        }
        if cell_w > 0.0 {
            editor.set_layout_aspect(cell_h / cell_w);
        }
        // Update tile rects once per iteration (not per-event)
        editor.update_tile_rects(cols as u16, rows as u16);
        let now_seconds = backend.time_seconds();
        let sdf_animation_active =
            crate::widget_render::sdf_widget::sdf_visual_animations_active(now_seconds);
        let widget_animation_active = editor.visible_widgets_animating();
        if sdf_animation_active || widget_animation_active {
            editor.mark_needs_redraw();
        }
        let frame_interval = if sdf_animation_active || widget_animation_active {
            animation_frame_interval
        } else {
            idle_frame_interval
        };

        let timeout = frame_interval.saturating_sub(last_render_at.elapsed());
        // Render on resize ticks delivered while the poll is stuck inside
        // macOS's live-resize modal loop; otherwise the compositor stretches
        // the previous frame to the new window size.
        let mut live_resize_redraw = |backend: &mut MetalBackend| {
            let (cols, rows) = backend.viewport_size();
            editor.update_tile_rects(cols as u16, rows as u16);
            let tiled_frame = frame::build_tiled_render_frame_borderless(&mut editor, cols, rows);
            match backend.render_tiled(&tiled_frame) {
                Ok(metal_backend::TiledRenderStatus::Presented) => {
                    editor.clear_needs_redraw();
                    last_render_at = Instant::now();
                }
                Ok(metal_backend::TiledRenderStatus::NotPresented) => {
                    frame::requeue_unpresented_tiled_frame(&mut editor, &tiled_frame);
                }
                Err(_) => {}
            }
        };
        match backend.poll_event_with_redraw(timeout, &mut live_resize_redraw) {
            Some(Event::Key(key)) => editor.handle_key(key),
            Some(Event::Mouse(mouse)) => {
                let (precise_col, precise_row) = backend
                    .take_last_precise_mouse()
                    .unwrap_or((mouse.column as f32, mouse.row as f32));
                if matches!(
                    mouse.kind,
                    crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left)
                ) {
                    pending_drag = Some((Event::Mouse(mouse), (precise_col, precise_row)));
                } else {
                    // Clear stale drag on mouse up so it doesn't fire after release
                    if matches!(mouse.kind, crossterm::event::MouseEventKind::Up(_)) {
                        pending_drag = None;
                    }
                    editor.handle_tiled_mouse_precise(
                        mouse,
                        precise_col,
                        precise_row,
                        0, // Metal: no cell borders
                    );
                    backend.set_widget_cursor(editor.widget_cursor());
                }
            }
            Some(Event::Resize(_, _)) => editor.mark_needs_redraw(),
            _ => {}
        }

        // Hidden-cursor infinite drag: hide/lock the pointer while a gesture
        // on an opted-in widget is live, restore it the moment it is not.
        let hidden_drag_wanted = editor.hidden_drag_gesture_active();
        if hidden_drag_wanted != hidden_drag_started {
            if hidden_drag_wanted {
                backend.begin_hidden_drag();
            } else {
                backend.end_hidden_drag();
            }
            hidden_drag_started = hidden_drag_wanted;
        }

        // Process touchpad gestures (Metal-specific, not crossterm events)
        let mut handled_magnify = false;
        while let Some((delta, (precise_col, precise_row))) = backend.take_pending_magnify() {
            editor.handle_tiled_touchpad_magnify(precise_col, precise_row, 0, delta);
            handled_magnify = true;
        }
        if handled_magnify {
            while backend.take_pending_scroll().is_some() {}
        }
        while let Some(((delta_x, delta_y), (precise_col, precise_row))) =
            backend.take_pending_scroll()
        {
            // First try widget-level scroll (timeline etc.) in the hovered tile.
            let widget_handled =
                editor.handle_tiled_touchpad_scroll(precise_col, precise_row, 0, delta_x, delta_y);
            if widget_handled {
                continue;
            }

            // In UI mode, apply pixel deltas directly for smooth sub-cell scrolling.
            // Use the same cells-per-pixel ratio as the scroll widget (0.05).
            if editor.is_ui_scroll_mode() {
                let scroll_speed = 0.05; // cells per pixel-delta
                let delta_cells_y = delta_y * scroll_speed;
                let delta_cells_x = delta_x * scroll_speed;
                editor.apply_smooth_widget_scroll(delta_cells_x, delta_cells_y);
                continue;
            }

            let line_px = backend.viewport_size().1.max(1) as f32 / (rows.max(1) as f32);

            // Accumulate pixel deltas for text buffer scrolling (cell-based).
            // Only emit ScrollUp/Down after accumulating ~1 line of pixels.
            scroll_accum_y += delta_y;
            let threshold = line_px.max(20.0); // at least 20px per scroll step
            while scroll_accum_y > threshold {
                scroll_accum_y -= threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollUp,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
            while scroll_accum_y < -threshold {
                scroll_accum_y += threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollDown,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
            // Horizontal scroll accumulation
            scroll_accum_x += delta_x;
            while scroll_accum_x > threshold {
                scroll_accum_x -= threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollLeft,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
            while scroll_accum_x < -threshold {
                scroll_accum_x += threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollRight,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
        }

        if let Some((Event::Mouse(mouse), (precise_col, precise_row))) = pending_drag.take() {
            editor.handle_tiled_mouse_precise(
                mouse,
                precise_col,
                precise_row,
                0, // Metal: no cell borders
            );
        }

        if last_render_at.elapsed() >= frame_interval {
            let tiled_frame = frame::build_tiled_render_frame_borderless(&mut editor, cols, rows);
            match backend.render_tiled(&tiled_frame)? {
                metal_backend::TiledRenderStatus::Presented => {
                    editor.clear_needs_redraw();
                    last_render_at = Instant::now();
                }
                metal_backend::TiledRenderStatus::NotPresented => {
                    frame::requeue_unpresented_tiled_frame(&mut editor, &tiled_frame);
                    last_render_at = Instant::now();
                }
            }
        }

        if editor.should_quit() {
            break;
        }
    }

    backend.teardown()
}

pub fn run_standalone() -> io::Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let result = run_editor(&mut terminal);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::HashMap,
        rc::Rc,
        time::{Duration, Instant},
    };

    use super::{Runtime, Value, run_prog};
    use crate::layout::{LayoutEngine, format_layout_tree_lines};
    use crate::parser::Parser;
    use crate::vm::{EffectTarget, VMError, format_lisp_source, format_lisp_value};
    const SDF_LIGHTING_V2_DEMO: &str = include_str!("../examples/sdf-lighting-v2-demo.lisp");

    fn demo_source_without_window_ops() -> &'static str {
        SDF_LIGHTING_V2_DEMO
            .trim_end()
            .strip_suffix("\n(delete-other-windows)\n(split-window-right \"*light*\")")
            .expect("demo should end with window management epilogue")
    }

    fn duration_stats(mut samples: Vec<Duration>) -> (Duration, Duration, Duration) {
        assert!(!samples.is_empty(), "expected at least one duration sample");
        samples.sort();
        let min = samples[0];
        let median = samples[samples.len() / 2];
        let total_secs = samples.iter().map(Duration::as_secs_f64).sum::<f64>();
        let avg = Duration::from_secs_f64(total_secs / samples.len() as f64);
        (min, median, avg)
    }

    fn assert_widget_diagnostic(value: &Value, expected_message: &str) {
        let Value::Map(map) = value else {
            panic!("expected widget diagnostic map, got {value:?}");
        };
        assert!(matches!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword(widget_type)) if widget_type == "label"
        ));
        assert!(matches!(
            map.get("debug-name").map(|value| value.borrow().clone()),
            Some(Value::String(debug_name)) if debug_name == "widget-diagnostic"
        ));
        assert!(matches!(
            map.get("__widget-diagnostic")
                .map(|value| value.borrow().clone()),
            Some(Value::String(message)) if message == expected_message
        ));
        assert!(matches!(
            map.get("text").map(|value| value.borrow().clone()),
            Some(Value::String(text)) if text.contains(expected_message)
        ));
    }

    #[test]
    fn test_basic_sum() {
        assert_eq!(run_prog("(+ 1 2)"), Ok(Some(Value::Number(3.0))));
    }

    #[test]
    fn test_decimal_literal() {
        assert_eq!(run_prog("0.25"), Ok(Some(Value::Number(0.25))));
    }

    #[test]
    fn test_var_set() {
        assert_eq!(
            run_prog("(def x 5) (* x 10)"),
            Ok(Some(Value::Number(50.0)))
        );
    }

    #[test]
    fn test_function_def() {
        assert_eq!(
            run_prog("(def sq (x) (* x x)) (sq 10)"),
            Ok(Some(Value::Number(100.0)))
        );
    }

    #[test]
    fn test_multi_expression_function_body() {
        assert_eq!(
            run_prog("(def f () 1 2 3) (f)"),
            Ok(Some(Value::Number(3.0)))
        );
    }

    #[test]
    fn test_function_closure() {
        assert_eq!(
            run_prog(
                "(def sq (x) (* x x)) (def make-fn (fn a) (lambda (y) (fn (+ a y)))) ((make-fn sq 2) 10)"
            ),
            Ok(Some(Value::Number(144.0)))
        );
    }

    #[test]
    fn test_stored_zero_arg_closure_can_be_called_across_evals() {
        let mut runtime = Runtime::new();
        let closure = runtime.eval_str("(lambda () 42)").unwrap().unwrap();
        runtime.set_global_value("saved", closure);

        assert_eq!(runtime.eval_str("(saved)"), Ok(Some(Value::Number(42.0))));
    }

    #[test]
    fn test_stored_arg_closure_can_be_called_across_evals() {
        let mut runtime = Runtime::new();
        let closure = runtime.eval_str("(lambda (x) (+ x 1))").unwrap().unwrap();
        runtime.set_global_value("saved", closure);

        assert_eq!(runtime.eval_str("(saved 5)"), Ok(Some(Value::Number(6.0))));
    }

    #[test]
    fn test_stored_closure_with_body_can_be_called_across_evals() {
        let mut runtime = Runtime::new();
        let closure = runtime.eval_str("(lambda () (+ 2 3))").unwrap().unwrap();
        runtime.set_global_value("saved", closure);

        assert_eq!(runtime.eval_str("(saved)"), Ok(Some(Value::Number(5.0))));
    }

    #[test]
    fn test_stored_closure_can_call_native_across_evals() {
        let mut runtime = Runtime::new();
        runtime.register_native("bump", |args, _ctx| match args.first() {
            Some(Value::Number(n)) => Ok(Value::Number(n + 1.0)),
            _ => Err("expected number".to_string()),
        });
        let closure = runtime.eval_str("(lambda (x) (bump x))").unwrap().unwrap();
        runtime.set_global_value("saved", closure);

        assert_eq!(runtime.eval_str("(saved 5)"), Ok(Some(Value::Number(6.0))));
    }

    #[test]
    fn test_closure_round_trips_through_native_storage() {
        let mut runtime = Runtime::new();
        let stored: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
        let stored_for_native = Rc::clone(&stored);
        runtime.register_native("capture", move |args, _ctx| {
            *stored_for_native.borrow_mut() = args.first().cloned();
            Ok(Value::Bool(true))
        });

        assert_eq!(
            runtime.eval_str("(capture (lambda (x) (+ x 1)))"),
            Ok(Some(Value::Bool(true)))
        );

        let closure = stored.borrow().clone().expect("closure should be captured");
        runtime.set_global_value("saved", closure);

        assert_eq!(runtime.eval_str("(saved 5)"), Ok(Some(Value::Number(6.0))));
    }

    #[test]
    fn test_unknown_variable_includes_missing_symbol_name() {
        let mut runtime = Runtime::new();
        assert_eq!(
            runtime.eval_str("missing-name"),
            Err(VMError::UnknownVariable("missing-name".to_string()))
        );
    }

    #[test]
    fn test_lambda_shorthand_as_argument() {
        assert_eq!(
            run_prog("(def use-fn (val fn) (fn val)) (use-fn 5 |x| (+ x 1))"),
            Ok(Some(Value::Number(6.0)))
        );
    }

    #[test]
    fn test_lambda_shorthand_with_multiple_args() {
        assert_eq!(
            run_prog("(def use-fn (a b fn) (fn a b)) (use-fn 5 7 |x y| (+ x y))"),
            Ok(Some(Value::Number(12.0)))
        );
    }

    #[test]
    fn test_if_statement_false() {
        assert_eq!(
            run_prog("(if (= 5 4) 10 15)"),
            Ok(Some(Value::Number(15.0)))
        );
    }

    #[test]
    fn test_if_statement_true() {
        assert_eq!(
            run_prog("(if (= 5 5) 10 15)"),
            Ok(Some(Value::Number(10.0)))
        );
    }

    #[test]
    fn test_nested_if_over_dict_keyword_type_hits_move_items_branch() {
        let program = r#"
            (defstate lastmove "")
            (def handle-timeline-action (e)
              (if (= (get e :type) :select)
                (set! lastmove "select")
                (if (= (get e :type) :clear-selection)
                  (set! lastmove "clear")
                  (if (= (get e :type) :marquee-select)
                    (set! lastmove "marquee")
                    (if (= (get e :type) :move-items)
                      (set! lastmove "hello")
                      (if (= (get e :type) :resize-item)
                        (set! lastmove "resize")
                        (if (= (get e :type) :create-item)
                          (set! lastmove "create")
                          nil))))))
              lastmove)
            (handle-timeline-action (dict :type :move-items :ids (list 10) :delta-time 1))
        "#;

        assert_eq!(
            run_prog(program),
            Ok(Some(Value::String("hello".to_string())))
        );
    }

    #[test]
    fn test_match_statement_dispatches_on_keyword_from_dict() {
        let program = r#"
            (def handle-timeline-action (e)
              (match e.type
                :select "select"
                :move-items-absolute "move"
                :resize-item-absolute "resize"
                _ "unknown"))
            (handle-timeline-action (dict :type :move-items-absolute :ids (list 10) :start 4 :lane 1))
        "#;

        assert_eq!(
            run_prog(program),
            Ok(Some(Value::String("move".to_string())))
        );
    }

    #[test]
    fn test_match_statement_without_default_returns_nil() {
        assert_eq!(
            run_prog("(match :resize-item-absolute :move-items-absolute 1 :select 2)"),
            Ok(Some(Value::Nil))
        );
    }

    #[test]
    fn test_and_short_circuits_and_returns_falsey_value() {
        assert_eq!(run_prog("(and true 1 nil 2)"), Ok(Some(Value::Nil)));
    }

    #[test]
    fn test_and_short_circuits_side_effects() {
        let program = r#"
            (defstate x 0)
            (and false (set! x 1))
            x
        "#;
        assert_eq!(run_prog(program), Ok(Some(Value::Number(0.0))));
    }

    #[test]
    fn test_or_short_circuits_and_returns_truthy_value() {
        assert_eq!(run_prog("(or nil false 7 9)"), Ok(Some(Value::Number(7.0))));
    }

    #[test]
    fn test_or_short_circuits_side_effects() {
        let program = r#"
            (defstate x 0)
            (or true (set! x 1))
            x
        "#;
        assert_eq!(run_prog(program), Ok(Some(Value::Number(0.0))));
    }

    #[test]
    fn test_let_map_destructuring_by_symbol_name() {
        assert_eq!(
            run_prog("(let (((id lane) (dict :id 10 :lane 2))) (+ id lane))"),
            Ok(Some(Value::Number(12.0)))
        );
    }

    #[test]
    fn test_lambda_map_destructuring_by_symbol_name() {
        assert_eq!(
            run_prog("((lambda ((id lane)) (+ id lane)) (dict :id 10 :lane 2))"),
            Ok(Some(Value::Number(12.0)))
        );
    }

    #[test]
    fn test_lambda_shorthand_map_destructuring_by_symbol_name() {
        assert_eq!(
            run_prog("(|(id lane)| (+ id lane) (dict :id 10 :lane 2))"),
            Ok(Some(Value::Number(12.0)))
        );
    }

    #[test]
    fn test_thread_first_macro() {
        assert_eq!(
            run_prog("(-> 5 (+ 3) (* 2))"),
            Ok(Some(Value::Number(16.0)))
        );
    }

    #[test]
    fn test_thread_last_macro() {
        assert_eq!(
            run_prog("(->> 5 (list 1 2) (reverse) first)"),
            Ok(Some(Value::Number(5.0)))
        );
    }

    #[test]
    fn test_recursion() {
        assert_eq!(
            run_prog("(def gauss (n) (if (= n 0) 0 (+ n (gauss (- n 1))))) (gauss 5)"),
            Ok(Some(Value::Number(15.0)))
        );
    }

    #[test]
    fn test_let_expression() {
        assert_eq!(
            run_prog("(let ((a 2) (b 5)) (+ a b))"),
            Ok(Some(Value::Number(7.0)))
        );
    }

    #[test]
    fn test_let_sequential_binding() {
        // Each binding can reference previous ones
        assert_eq!(
            run_prog("(let ((a 10) (b (+ a 5))) b)"),
            Ok(Some(Value::Number(15.0)))
        );
    }

    #[test]
    fn test_let_sequential_three_bindings() {
        assert_eq!(
            run_prog("(let ((a 2) (b (* a 3)) (c (+ a b))) c)"),
            Ok(Some(Value::Number(8.0)))
        );
    }

    #[test]
    fn test_dired_refresh_pattern() {
        // Reproduce the exact dired-refresh let nesting with separate eval calls
        let mut runtime = Runtime::new();
        runtime
            .eval_str(
                "(def my-filter (fn xs)
                   (if (= (len xs) 0) '()
                     (if (fn (first xs))
                       (cons (first xs) (my-filter fn (rest xs)))
                       (my-filter fn (rest xs)))))",
            )
            .unwrap();
        runtime
            .eval_str(
                "(def my-map (fn xs)
                   (if (= (len xs) 0) '()
                     (cons (fn (first xs)) (my-map fn (rest xs)))))",
            )
            .unwrap();
        // Simulate dired-refresh: set! global between two lets, use outer vars in inner let
        runtime.eval_str("(def my-global '())").unwrap();
        runtime
            .eval_str("(def my-format (x) (str \"item-\" x))")
            .unwrap();
        let result = runtime.eval_str(
            "(def test-it ()
               (let ((entries (list 1 2 3 4 5))
                     (dirs (my-filter |x| (> x 3) entries))
                     (files (my-filter |x| (not (> x 3)) entries)))
                 (set! my-global (append dirs files))
                 (let ((dir-lines (my-map my-format dirs))
                       (file-lines (my-map my-format files)))
                   (append dir-lines file-lines))))
             (test-it)",
        );
        assert!(result.is_ok(), "dired pattern failed: {result:?}");
    }

    #[test]
    fn test_let_deep_upvalue_chain() {
        // Outer let var used deep inside inner let after set! (dired pattern)
        assert_eq!(
            run_prog(
                "(def my-double (x) (* x 2))
                 (let ((a 5)
                       (b (+ a 1))
                       (c (+ a 2)))
                   (set! a 99)
                   (let ((d (my-double b))
                         (e (my-double c)))
                     (+ d e)))"
            ),
            Ok(Some(Value::Number(26.0)))
        );
    }

    #[test]
    fn test_let_outer_vars_in_inner_let_bindings() {
        // Outer let vars used in inner let bindings (like dired-refresh)
        assert_eq!(
            run_prog(
                "(let ((a 10) (b 20))
                   (let ((c (* a 2)) (d (+ b 1)))
                     (+ c d)))"
            ),
            Ok(Some(Value::Number(41.0)))
        );
    }

    #[test]
    fn test_let_sequential_with_user_globals() {
        // Simulates the dired pattern: globals used inside nested let bindings
        assert_eq!(
            run_prog(
                "(def my-filter (fn xs) (if (= (len xs) 0) '() (if (fn (first xs)) (cons (first xs) (my-filter fn (rest xs))) (my-filter fn (rest xs)))))
                 (let ((entries (list 1 2 3 4))
                       (big (my-filter |x| (> x 2) entries)))
                   (len big))"
            ),
            Ok(Some(Value::Number(2.0)))
        );
    }

    #[test]
    fn test_let_sequential_nested_let_uses_outer_binding() {
        assert_eq!(
            run_prog("(let ((a 1) (b 2)) (let ((c (+ a b))) c))"),
            Ok(Some(Value::Number(3.0)))
        );
    }

    #[test]
    fn test_do_expression() {
        assert_eq!(run_prog("(do 1 2 3)"), Ok(Some(Value::Number(3.0))));
    }

    #[test]
    fn test_dict_get() {
        assert_eq!(
            run_prog("(def p (dict :name \"Alec\" :age 25)) (get p :age)"),
            Ok(Some(Value::Number(25.0)))
        );
    }

    #[test]
    fn test_merge() {
        assert_eq!(
            run_prog("(def p (dict :age 25)) (def p2 (merge p :age 30)) (get p2 :age)"),
            Ok(Some(Value::Number(30.0)))
        );
    }

    #[test]
    fn test_nil_literal_and_truthiness() {
        assert_eq!(run_prog("nil"), Ok(Some(Value::Nil)));
        assert_eq!(run_prog("(if nil 10 20)"), Ok(Some(Value::Number(20.0))));
        assert_eq!(run_prog("(not nil)"), Ok(Some(Value::Bool(true))));
    }

    #[test]
    fn test_dot_syntax() {
        assert_eq!(
            run_prog("(def p (dict :age 25)) p.age"),
            Ok(Some(Value::Number(25.0)))
        );
    }

    #[test]
    fn test_quote_symbol() {
        assert_eq!(
            run_prog("'hello"),
            Ok(Some(Value::Symbol("hello".to_string())))
        );
    }

    #[test]
    fn test_quote_list() {
        assert_eq!(
            run_prog("'(1 2 3)"),
            Ok(Some(Value::List(vec![
                std::rc::Rc::new(std::cell::RefCell::new(Value::Number(1.0))),
                std::rc::Rc::new(std::cell::RefCell::new(Value::Number(2.0))),
                std::rc::Rc::new(std::cell::RefCell::new(Value::Number(3.0))),
            ])))
        );
    }

    #[test]
    fn test_quote_list_preserves_symbols() {
        assert_eq!(
            run_prog("'(seq-toggle-step 1)"),
            Ok(Some(Value::List(vec![
                std::rc::Rc::new(std::cell::RefCell::new(Value::Symbol(
                    "seq-toggle-step".to_string(),
                ))),
                std::rc::Rc::new(std::cell::RefCell::new(Value::Number(1.0))),
            ])))
        );
    }

    #[test]
    fn test_lisp_printer_for_list() {
        let value = run_prog("'(1 2 3)").unwrap().unwrap();
        assert_eq!(format_lisp_value(&value), "(1 2 3)");
    }

    #[test]
    fn test_lisp_printer_for_map() {
        let value = run_prog("(dict :step 1 :active false)").unwrap().unwrap();
        assert_eq!(format_lisp_value(&value), "{:active false :step 1}");
    }

    #[test]
    fn test_list_native() {
        assert_eq!(
            run_prog("(list 1 2 3)"),
            Ok(Some(Value::List(vec![
                std::rc::Rc::new(std::cell::RefCell::new(Value::Number(1.0))),
                std::rc::Rc::new(std::cell::RefCell::new(Value::Number(2.0))),
                std::rc::Rc::new(std::cell::RefCell::new(Value::Number(3.0))),
            ])))
        );
    }

    #[test]
    fn test_nth_native() {
        assert_eq!(
            run_prog("(nth (list 10 20 30) 1)"),
            Ok(Some(Value::Number(20.0)))
        );
    }

    #[test]
    fn test_range_native() {
        let value = run_prog("(range 1 4)").unwrap().unwrap();
        assert_eq!(format_lisp_value(&value), "(1 2 3)");
    }

    #[test]
    fn test_rand_int_native_in_range() {
        let value = run_prog("(rand-int 10 20)").unwrap().unwrap();
        let Value::Number(n) = value else {
            panic!("expected number");
        };
        assert!((10.0..20.0).contains(&n), "rand-int returned {n}");
    }

    #[test]
    fn test_division_inside_zero_arg_function_call() {
        let value = run_prog("(def myrand () (/ (rand-int 100) 100)) (myrand)")
            .unwrap()
            .unwrap();
        let Value::Number(n) = value else {
            panic!("expected number");
        };
        assert!((0.0..1.0).contains(&n) || (n - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_reverse_native() {
        let value = run_prog("(reverse (list 1 2 3))").unwrap().unwrap();
        assert_eq!(format_lisp_value(&value), "(3 2 1)");
    }

    #[test]
    fn test_str_uses_lisp_printer_for_quoted_list() {
        let value = run_prog("(str '(seq-toggle-step 1))").unwrap().unwrap();
        assert_eq!(format_lisp_value(&value), "\"('seq-toggle-step 1)\"");
    }

    #[test]
    fn test_lisp_source_for_quoted_list() {
        let value = run_prog("'(seq-toggle-step 1)").unwrap().unwrap();
        assert_eq!(format_lisp_source(&value), "(seq-toggle-step 1)");
    }

    #[test]
    fn test_source_native_for_quoted_list() {
        let value = run_prog("(source '(seq-toggle-step 1))").unwrap().unwrap();
        assert_eq!(format_lisp_value(&value), "\"(seq-toggle-step 1)\"");
    }

    #[test]
    fn test_map_filter_reduce_zip_builtins() {
        let program = r#"
            (def empty? (xs) (= (len xs) 0))
            (list
              (map |x| (+ x 1) (list 1 2 3))
              (filter |x| (> x 1) (list 1 2 3))
              (reduce |acc x| (+ acc x) 0 (list 1 2 3))
              (zip (list 1 2 3) (list 4 5 6) (list 7 8)))
        "#;
        let value = run_prog(program).unwrap().unwrap();
        assert_eq!(
            format_lisp_value(&value),
            "((2 3 4) (2 3) 6 ((1 4 7) (2 5 8)))"
        );
    }

    #[test]
    fn test_each_zip_tuple_destructuring_reads_positionally() {
        let value = run_prog(
            r#"
            (let ((toggles (list true false true))
                  (levels (list 10 20 30)))
              (each (zip toggles levels) |(enabled level)|
                (list enabled level)))
        "#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            format_lisp_value(&value),
            "((true 10) (false 20) (true 30))"
        );
    }

    #[test]
    fn test_widget_native_builds_map_shape() {
        let value = run_prog("(slider :min 0 :max 100 :value 50)")
            .unwrap()
            .unwrap();
        let Value::Map(map) = value else {
            panic!("expected widget map");
        };

        assert_eq!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword("slider".to_string()))
        );
        assert_eq!(
            map.get("min").map(|value| value.borrow().clone()),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            map.get("max").map(|value| value.borrow().clone()),
            Some(Value::Number(100.0))
        );
        assert_eq!(
            map.get("value").map(|value| value.borrow().clone()),
            Some(Value::Number(50.0))
        );
    }

    #[test]
    fn test_effect_layout_matches_phase_one_spec() {
        let mut runtime = Runtime::new();
        let value = runtime
            .eval_str(
                r#"
                (effect
                  (v-stack
                    (label "Hello World")
                    (h-stack
                      (label "X:")
                      (slider :min 0 :max 100 :value 50))))
                "#,
            )
            .unwrap()
            .unwrap();
        assert_eq!(value, Value::Nil);

        let tree = run_prog(
            r#"
            (v-stack
              (label "Hello World")
              (h-stack
                (label "X:")
                (slider :min 0 :max 100 :value 50)))
            "#,
        )
        .unwrap()
        .unwrap();

        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&tree)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);

        assert_eq!(
            lines,
            vec![
                ":v-stack  row=0 col=0 w=19 h=2".to_string(),
                "  :label  row=0 col=0 w=11 h=1  text=\"Hello World\"".to_string(),
                "  :h-stack  row=1 col=0 w=19 h=1".to_string(),
                "    :label  row=1 col=0 w=2 h=1  text=\"X:\"".to_string(),
                "    :slider  row=1 col=3 w=16 h=1  value=50  min=0  max=100".to_string(),
            ]
        );
    }

    #[test]
    fn test_nested_vstack_in_hstack_uses_content_width() {
        let tree = run_prog(
            r#"
            (h-stack
              (hslider :min 0 :max 100 :value 30)
              (v-stack
                (hslider :min 0 :max 100 :value 50)
                (hslider :min 0 :max 100 :value 20))
              (vslider :min 0 :max 100 :value 20))
            "#,
        )
        .unwrap()
        .unwrap();

        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&tree)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);

        // The v-stack contains two 16-wide hsliders, so its width should be 16
        // (content width), NOT 80 (terminal width). The vslider should start
        // right after: col = 16 (hslider) + 1 (gap) + 16 (v-stack) + 1 (gap) = 34.
        // h-stack sizes to content: 16 + 1 + 16 + 1 + 2 = 36
        assert_eq!(lines[0], ":h-stack  row=0 col=0 w=36 h=8");
        // First hslider at col 0
        assert_eq!(
            lines[1],
            "  :hslider  row=0 col=0 w=16 h=1  value=30  min=0  max=100"
        );
        // v-stack should be 16 wide (its content), starting at col 17
        assert_eq!(lines[2], "  :v-stack  row=0 col=17 w=16 h=2");
        // vslider should be right after v-stack: col = 17 + 16 + 1 = 34
        assert_eq!(
            lines[5],
            "  :vslider  row=0 col=34 w=2 h=8  value=20  min=0  max=100"
        );
    }

    #[test]
    fn test_grid_defaults_to_child_sized_cells() {
        let tree = run_prog(
            r#"
            (grid :cols 4
              (knob :size 1 :value 0)
              (knob :size 1 :value 0)
              (knob :size 1 :value 0)
              (knob :size 1 :value 0))
            "#,
        )
        .unwrap()
        .unwrap();

        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&tree)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);

        assert_eq!(lines[0], ":grid  row=0 col=0 w=4 h=1");
        assert_eq!(lines[1], "  :knob  row=0 col=0 w=1 h=1  value=0");
        assert_eq!(lines[2], "  :knob  row=0 col=1 w=1 h=1  value=0");
        assert_eq!(lines[3], "  :knob  row=0 col=2 w=1 h=1  value=0");
        assert_eq!(lines[4], "  :knob  row=0 col=3 w=1 h=1  value=0");
    }

    #[test]
    fn test_grid_preserves_child_size_inside_explicit_slots() {
        let tree = run_prog(
            r#"
            (grid :cols 2 :col-width 4 :row-height 4
              (knob :size 1 :value 0)
              (knob :size 2 :value 0))
            "#,
        )
        .unwrap()
        .unwrap();

        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&tree)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);

        assert_eq!(lines[0], ":grid  row=0 col=0 w=8 h=4");
        assert_eq!(lines[1], "  :knob  row=0 col=0 w=1 h=1  value=0");
        assert_eq!(lines[2], "  :knob  row=0 col=4 w=2 h=2  value=0");
    }

    #[test]
    fn test_knob_size_respects_layout_aspect() {
        let tree = run_prog(r#"(knob :size 2 :value 0)"#).unwrap().unwrap();

        let layout = LayoutEngine::new(80, 24, 2.0)
            .layout(&tree)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);

        assert_eq!(lines[0], ":knob  row=0 col=0 w=2 h=1  value=0");
    }

    #[test]
    fn test_reactive_namespace_reads_transparently() {
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "APP",
            vec![
                ("counter", Value::Number(5.0)),
                ("label", Value::String("hello".to_string())),
            ],
            false,
        );

        let tree = runtime
            .eval_str(
                r#"
                (v-stack
                  (label APP.label)
                  (label (fmt "count: {}" APP.counter)))
                "#,
            )
            .unwrap()
            .unwrap();

        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&tree)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);
        assert_eq!(
            lines,
            vec![
                ":v-stack  row=0 col=0 w=8 h=2".to_string(),
                "  :label  row=0 col=0 w=5 h=1  text=\"hello\"".to_string(),
                "  :label  row=1 col=0 w=8 h=1  text=\"count: 5\"".to_string(),
            ]
        );
    }

    #[test]
    fn test_reactive_cycle_updates_registered_state() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("counter", Value::Number(5.0))], false);

        let _ = runtime
            .eval_str(r#"(effect (label (fmt "count: {}" APP.counter)))"#)
            .unwrap();

        runtime.set_reactive("APP", "counter", Value::Number(42.0));
        runtime.run_reactive_cycle();

        let updated = runtime
            .eval_str(r#"(label (fmt "count: {}" APP.counter))"#)
            .unwrap()
            .unwrap();
        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&updated)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);
        assert_eq!(
            lines,
            vec![":label  row=0 col=0 w=9 h=1  text=\"count: 42\"".to_string()]
        );
    }

    #[test]
    fn bind_does_not_subscribe_effects_and_marks_bound_widget_dirty() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime.register_reactive("APP", vec![("peak", Value::Number(0.1))], true);

        runtime
            .eval_str(
                r#"
                (effect
                  (mixer-meter
                    :level-l (bind "APP" "peak")
                    :level-r 0.0
                    :width 2.22 :height 4.24))
                "#,
            )
            .expect("install bound meter effect");

        let layout = runtime.current_layout.as_ref().expect("bound meter layout");
        assert_eq!(layout.widget_type, "mixer-meter");
        let widget_id = layout.widget_id;
        assert!(matches!(
            layout.props.get("level-l"),
            Some(Value::ReactiveRef { namespace, field, .. })
                if namespace == "APP" && field == "peak"
        ));
        let _ = runtime.take_dirty_widget_ids();
        let _ = runtime.drain_rendered_layouts();

        runtime.set_reactive("APP", "peak", Value::Number(0.8));
        runtime.run_reactive_cycle();

        assert_eq!(
            runtime
                .current_layout
                .as_ref()
                .map(|layout| layout.widget_id),
            Some(widget_id),
            "binding-only writes must not rebuild or relayout the widget tree"
        );
        assert_eq!(runtime.take_dirty_widget_ids(), vec![widget_id]);
        assert!(
            runtime.drain_rendered_layouts().is_empty(),
            "binding-only writes must not rerun effects"
        );
    }

    #[test]
    fn number_label_native_accepts_value_binding() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime.register_reactive("APP", vec![("cpu", Value::Number(12.0))], true);

        runtime
            .eval_str(
                r#"
                (effect
                  (number-label
                    :value (bind "APP" "cpu")
                    :decimals 0
                    :suffix "%"
                    :width 4
                    :height 1))
                "#,
            )
            .expect("install bound number-label effect");

        let layout = runtime
            .current_layout
            .as_ref()
            .expect("number-label layout");
        assert_eq!(layout.widget_type, "number-label");
        let widget_id = layout.widget_id;
        let _ = runtime.take_dirty_widget_ids();
        let _ = runtime.drain_rendered_layouts();

        runtime.set_reactive("APP", "cpu", Value::Number(35.0));
        runtime.run_reactive_cycle();

        assert_eq!(runtime.take_dirty_widget_ids(), vec![widget_id]);
        assert!(
            runtime.drain_rendered_layouts().is_empty(),
            "number-label binding-only writes must not rerun effects"
        );
    }

    #[test]
    fn bind_nth_marks_only_widgets_for_changed_indices_dirty() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime.register_reactive(
            "APP",
            vec![(
                "levels",
                Value::List(vec![
                    Rc::new(RefCell::new(Value::Number(0.1))),
                    Rc::new(RefCell::new(Value::Number(0.2))),
                ]),
            )],
            true,
        );

        runtime
            .eval_str(
                r#"
                (effect
                  (h-stack
                    (mixer-meter
                      :level-l (bind-nth "APP" "levels" 0)
                      :level-r 0.0
                      :width 2.22 :height 4.24)
                    (mixer-meter
                      :level-l (bind-nth "APP" "levels" 1)
                      :level-r 0.0
                      :width 2.22 :height 4.24)))
                "#,
            )
            .expect("install indexed bound meter effect");

        let layout = runtime.current_layout.as_ref().expect("bound meter layout");
        let first_meter_id = layout.children[0].widget_id;
        let second_meter_id = layout.children[1].widget_id;
        let _ = runtime.take_dirty_widget_ids();
        let _ = runtime.drain_rendered_layouts();

        runtime.set_reactive(
            "APP",
            "levels",
            Value::List(vec![
                Rc::new(RefCell::new(Value::Number(0.1))),
                Rc::new(RefCell::new(Value::Number(0.9))),
            ]),
        );
        runtime.run_reactive_cycle();

        assert_eq!(runtime.take_dirty_widget_ids(), vec![second_meter_id]);
        assert!(
            runtime.drain_rendered_layouts().is_empty(),
            "indexed binding-only writes must not rerun effects"
        );

        runtime.set_reactive(
            "APP",
            "levels",
            Value::List(vec![
                Rc::new(RefCell::new(Value::Number(0.8))),
                Rc::new(RefCell::new(Value::Number(0.9))),
            ]),
        );

        assert_eq!(runtime.take_dirty_widget_ids(), vec![first_meter_id]);
    }

    #[test]
    fn reactive_get_still_subscribes_effects() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime.register_reactive("APP", vec![("peak", Value::Number(0.1))], true);

        runtime
            .eval_str(r#"(effect (label (fmt "peak: {}" (reactive-get "APP" "peak"))))"#)
            .expect("install reactive-get effect");
        let _ = runtime.drain_rendered_layouts();

        runtime.set_reactive("APP", "peak", Value::Number(0.8));
        runtime.run_reactive_cycle();

        let rendered = runtime.drain_rendered_layouts();
        assert!(
            rendered
                .iter()
                .flatten()
                .any(|line| line.contains("peak: 0.8")),
            "reactive-get writes should rerun dependent effects: {rendered:?}"
        );
    }

    #[test]
    fn reactive_set_reruns_only_matching_field_subtree() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![], true);

        runtime
            .eval_str(
                r#"
                (effect-buffer "*rows*"
                  (v-stack
                    (subtree :key "row-a"
                      (label (if (reactive-get "APP" "a") "a on" "a off")))
                    (subtree :key "row-b"
                      (label (if (reactive-get "APP" "b") "b on" "b off")))))
                "#,
            )
            .expect("install row subtrees");
        let _ = runtime.take_pending_buffer_widget_trees();

        runtime
            .eval_str(r#"(reactive-set "APP" "a" true)"#)
            .expect("set reactive field from Lisp");

        let pending = runtime.take_pending_buffer_widget_trees();
        assert_eq!(
            pending.len(),
            1,
            "reactive-set should rerun only subtrees that read the changed field"
        );
        let rendered_tree = match &pending[0] {
            crate::vm::PendingUiUpdate::FullTree(update) => format!("{:?}", update.tree),
            crate::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => format!("{tree:?}"),
        };
        assert!(
            rendered_tree.contains("a on"),
            "updated subtree should be the row-a subtree: {rendered_tree}"
        );
        assert!(
            !rendered_tree.contains("b off"),
            "row-b must not be rerendered when APP.a changes: {rendered_tree}"
        );
    }

    #[test]
    fn set_reactive_updates_unsubscribed_fields_before_dependent_rerun() {
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "APP",
            vec![
                ("names", Value::List(vec![])),
                ("values", Value::List(vec![])),
            ],
            true,
        );

        runtime
            .eval_str(
                r#"
                (effect-buffer "*list*"
                  (v-stack
                    (each (range 0 (len APP.names)) |i|
                      (label (fmt "{}" (+ (nth APP.values i) 1))))))
                "#,
            )
            .expect("install sparse-list effect");
        let _ = runtime.take_pending_buffer_widget_trees();

        runtime.set_reactive(
            "APP",
            "names",
            Value::List(vec![Rc::new(RefCell::new(Value::String(
                "first".to_string(),
            )))]),
        );
        runtime.set_reactive(
            "APP",
            "values",
            Value::List(vec![Rc::new(RefCell::new(Value::Number(41.0)))]),
        );
        runtime.run_reactive_cycle();

        let pending = runtime.take_pending_buffer_widget_trees();
        assert_eq!(pending.len(), 1, "names should rerun the list effect");
        let crate::vm::PendingUiUpdate::FullTree(update) = &pending[0] else {
            panic!("expected full-tree update");
        };
        assert!(
            format!("{:?}", update.tree).contains("42"),
            "rerun must see the latest unsubscribed APP.values field: {:?}",
            update.tree
        );
    }

    #[test]
    fn transport_clock_widget_constructor_is_registered() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("playhead", Value::Number(0.0))], true);

        let value = runtime
            .eval_str(
                r#"(transport-clock
                    :playhead (bind "APP" "playhead")
                    :width 10
                    :height 1.2)"#,
            )
            .expect("transport-clock should evaluate")
            .expect("transport-clock should return a widget");

        let Value::Map(map) = value else {
            panic!("transport-clock should return a widget map, got {value:?}");
        };
        assert!(matches!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword(widget_type)) if widget_type == "transport-clock"
        ));
    }

    #[test]
    fn reactive_ref_rejected_for_non_bindable_widget_prop() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("peak", Value::Number(0.1))], true);

        let value = runtime
            .eval_str(r#"(mixer-meter :width (bind "APP" "peak") :level-l 0 :level-r 0)"#)
            .expect("evaluate invalid binding")
            .expect("invalid binding returns an error value");

        assert_widget_diagnostic(
            &value,
            "mixer-meter: :width does not accept reactive bindings",
        );
    }

    #[test]
    fn nested_widget_diagnostics_are_preserved_as_children() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("peak", Value::Number(0.1))], true);

        let value = runtime
            .eval_str(
                r#"(box
                    (mixer-meter :width (bind "APP" "peak") :level-l 0 :level-r 0))"#,
            )
            .expect("evaluate wrapper with invalid child binding")
            .expect("wrapper should return a widget");

        let Value::Map(map) = value else {
            panic!("box should return a widget map, got {value:?}");
        };
        let children = map
            .get("children")
            .expect("diagnostic child should not be dropped")
            .borrow()
            .clone();
        let Value::List(children) = children else {
            panic!("children should be a list, got {children:?}");
        };
        assert_eq!(children.len(), 1);
        assert_widget_diagnostic(
            &children[0].borrow(),
            "mixer-meter: :width does not accept reactive bindings",
        );
    }

    #[test]
    fn nested_widget_diagnostics_layout_with_visible_rects() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("peak", Value::Number(0.1))], true);

        let value = runtime
            .eval_str(
                r#"(box :width 30
                    (mixer-meter :width (bind "APP" "peak") :level-l 0 :level-r 0))"#,
            )
            .expect("evaluate wrapper with invalid child binding")
            .expect("wrapper should return a widget");

        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&value)
            .expect("diagnostic widget should lay out");
        let diagnostic = layout
            .children
            .first()
            .expect("diagnostic child should be in layout");
        assert_eq!(diagnostic.widget_type, "label");
        assert!(
            diagnostic.rect.width.is_finite()
                && diagnostic.rect.width > 0.0
                && diagnostic.rect.height.is_finite()
                && diagnostic.rect.height > 0.0,
            "diagnostic should have a visible finite rect, got {:?}",
            diagnostic.rect
        );
        assert!(matches!(
            diagnostic.props.get("__widget-diagnostic"),
            Some(Value::String(message))
                if message == "mixer-meter: :width does not accept reactive bindings"
        ));
    }

    #[test]
    fn box_background_sdf_accepts_background_widget_bindable_props() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("active", Value::Bool(true))], true);

        let value = runtime
            .eval_str(
                r#"
                (defwidget bindable-bg
                  :width 1 :height 1
                  :state (active)
                  :bindable (active)
                  :shader (if (= active 1) (rgba 1 1 1 1) (rgba 0 0 0 0)))
                (box :background "bindable-bg" :active (bind "APP" "active"))
                "#,
            )
            .expect("evaluate box background binding")
            .expect("box should return a widget");

        let Value::Map(map) = value else {
            panic!("box should return a widget map, got {value:?}");
        };
        assert!(matches!(
            map.get("active").map(|value| value.borrow().clone()),
            Some(Value::ReactiveRef { .. })
        ));
    }

    #[test]
    fn reactive_set_updates_bindable_float_slots() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime.register_reactive("APP", vec![("active", Value::Bool(false))], true);
        runtime
            .eval_str(
                r#"
                (def active-ref (bind "APP" "active"))
                (effect (label "cursor" :active active-ref :active-color :yellow))
                "#,
            )
            .expect("create reactive binding and bound widget");
        let widget_id = runtime
            .current_layout
            .as_ref()
            .expect("bound label layout")
            .widget_id;
        let _ = runtime.take_dirty_widget_ids();

        assert_eq!(
            runtime.eval_str("(reactive-value active-ref)").unwrap(),
            Some(Value::Number(0.0))
        );
        runtime
            .eval_str(r#"(reactive-set "APP" "active" true)"#)
            .expect("set active through lisp reactive-set");
        assert_eq!(
            runtime.eval_str("(reactive-value active-ref)").unwrap(),
            Some(Value::Number(1.0)),
            "Lisp reactive-set should update the slot read by bind/reactive-value"
        );
        assert_eq!(
            runtime.take_dirty_widget_ids(),
            vec![widget_id],
            "Lisp reactive-set should dirty widgets bound through bind"
        );
    }

    #[test]
    fn reactive_float_bindings_are_isolated_per_runtime() {
        let mut first = Runtime::new();
        first.register_reactive("APP", vec![("level", Value::Number(0.0))], true);
        first
            .eval_str(r#"(def level-ref (bind "APP" "level"))"#)
            .expect("bind first runtime level");

        let mut second = Runtime::new();
        second.register_reactive("APP", vec![("level", Value::Number(0.0))], true);
        second
            .eval_str(r#"(def level-ref (bind "APP" "level"))"#)
            .expect("bind second runtime level");

        first
            .eval_str(r#"(reactive-set "APP" "level" 0.25)"#)
            .expect("set first runtime level");
        second
            .eval_str(r#"(reactive-set "APP" "level" 0.75)"#)
            .expect("set second runtime level");

        assert_eq!(
            first.eval_str("(reactive-value level-ref)").unwrap(),
            Some(Value::Number(0.25))
        );
        assert_eq!(
            second.eval_str("(reactive-value level-ref)").unwrap(),
            Some(Value::Number(0.75))
        );
    }

    #[test]
    fn box_accepts_selected_and_muted_reactive_bindings() {
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "APP",
            vec![
                ("selected", Value::Bool(true)),
                ("muted", Value::Bool(false)),
            ],
            true,
        );

        let value = runtime
            .eval_str(
                r#"(box
                    :selected (bind "APP" "selected")
                    :muted (bind "APP" "muted")
                    :background-color :black
                    :selected-background-color :blue
                    :muted-background-color :gray
                    :border-color :dim
                    :selected-border-color :white
                    :muted-border-color :dark-gray)"#,
            )
            .expect("evaluate box state bindings")
            .expect("box should return a widget");

        let Value::Map(map) = value else {
            panic!("box should return a widget map, got {value:?}");
        };
        assert!(matches!(
            map.get("selected").map(|value| value.borrow().clone()),
            Some(Value::ReactiveRef { .. })
        ));
        assert!(matches!(
            map.get("muted").map(|value| value.borrow().clone()),
            Some(Value::ReactiveRef { .. })
        ));
    }

    #[test]
    fn removed_bound_subtree_stops_receiving_binding_dirty_marks() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime.register_reactive(
            "APP",
            vec![("show", Value::Bool(true)), ("peak", Value::Number(0.1))],
            true,
        );

        runtime
            .eval_str(
                r#"
                (effect
                  (if APP.show
                    (mixer-meter
                      :level-l (bind "APP" "peak")
                      :level-r 0.0
                      :width 2.22 :height 4.24)
                    (label "off")))
                "#,
            )
            .expect("install conditional bound meter");
        let meter_id = runtime
            .current_layout
            .as_ref()
            .expect("meter layout")
            .widget_id;
        let _ = runtime.take_dirty_widget_ids();

        runtime.set_reactive("APP", "show", Value::Bool(false));
        runtime.run_reactive_cycle();
        assert_eq!(
            runtime
                .current_layout
                .as_ref()
                .map(|layout| layout.widget_type.as_str()),
            Some("label")
        );
        let _ = runtime.take_dirty_widget_ids();

        runtime.set_reactive("APP", "peak", Value::Number(0.9));
        runtime.run_reactive_cycle();

        assert!(
            !runtime.take_dirty_widget_ids().contains(&meter_id),
            "stale widget binding registrations must be removed when a subtree is replaced"
        );
    }

    #[test]
    fn test_derived_reactive_flow_updates_only_dependent_effects() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("x", Value::Number(3.0))], true);

        let _ = runtime
            .eval_str(
                r#"
                (def doubled (derived (* APP.x 2)))
                (effect (label (fmt "doubled: {}" doubled)))
                "#,
            )
            .unwrap();

        let initial = runtime.drain_rendered_layouts();
        assert_eq!(
            initial,
            vec![vec![
                ":label  row=0 col=0 w=10 h=1  text=\"doubled: 6\"".to_string()
            ]]
        );

        runtime.set_reactive("APP", "x", Value::Number(7.0));
        runtime.run_reactive_cycle();

        let updated = runtime.drain_rendered_layouts();
        assert_eq!(
            updated,
            vec![vec![
                ":label  row=0 col=0 w=11 h=1  text=\"doubled: 14\"".to_string()
            ]]
        );

        runtime.set_reactive("APP", "y", Value::Number(99.0));
        runtime.run_reactive_cycle();

        assert!(runtime.drain_rendered_layouts().is_empty());

        let value = runtime
            .eval_str(r#"(label (fmt "doubled: {}" doubled))"#)
            .unwrap()
            .unwrap();
        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&value)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);
        assert_eq!(
            lines,
            vec![":label  row=0 col=0 w=11 h=1  text=\"doubled: 14\"".to_string()]
        );
    }

    #[test]
    fn test_lisp_state_binding_reads_like_plain_value() {
        let mut runtime = Runtime::new();
        let tree = runtime
            .eval_str(
                r#"
                (def my_state_value (state 33))
                (hslider :min 0 :max 100 :value my_state_value)
                "#,
            )
            .unwrap()
            .unwrap();

        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&tree)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);
        assert_eq!(
            lines,
            vec![":hslider  row=0 col=0 w=16 h=1  value=33  min=0  max=100".to_string()]
        );
    }

    #[test]
    fn test_label_coerces_each_bound_numbers_to_text() {
        let mut runtime = Runtime::new();
        let tree = runtime
            .eval_str(
                r#"
                (defstate steps '(4 8 15 16))
                (h-stack
                  (each steps |step|
                    (label step)))
                "#,
            )
            .unwrap()
            .unwrap();

        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&tree)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);
        assert_eq!(
            lines,
            vec![
                ":h-stack  row=0 col=0 w=9 h=1".to_string(),
                "  :label  row=0 col=0 w=1 h=1  text=\"4\"".to_string(),
                "  :label  row=0 col=2 w=1 h=1  text=\"8\"".to_string(),
                "  :label  row=0 col=4 w=2 h=1  text=\"15\"".to_string(),
                "  :label  row=0 col=7 w=2 h=1  text=\"16\"".to_string(),
            ]
        );
    }

    #[test]
    fn waveform_layout_reserves_requested_height_in_vstack() {
        let tree = run_prog(
            r#"
            (v-stack
              (label "waveform demo")
              (label "last action: nil")
              (label "loaded sample")
              (waveform
                :height 6
                :header-height 0.5
                :view-start 0
                :view-duration 1
                :buffer
                  (dict
                    :sample-rate 48000
                    :channels 1
                    :frames 8
                    :duration 1
                    :peaks
                      (list
                        (dict
                          :samples-per-bucket 1
                          :buckets
                            (list
                              (dict :min -0.5 :max 0.5)
                              (dict :min -0.25 :max 0.25))))))
              (hslider :min 0 :max 100 :value 50))
            "#,
        )
        .unwrap()
        .unwrap();

        let layout = LayoutEngine::new(120, 40, 1.0)
            .layout(&tree)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);

        assert_eq!(lines[0], ":v-stack  row=0 col=0 w=120 h=10");
        assert_eq!(
            lines[1],
            "  :label  row=0 col=0 w=13 h=1  text=\"waveform demo\""
        );
        assert_eq!(
            lines[2],
            "  :label  row=1 col=0 w=16 h=1  text=\"last action: nil\""
        );
        assert_eq!(
            lines[3],
            "  :label  row=2 col=0 w=13 h=1  text=\"loaded sample\""
        );
        assert_eq!(lines[4], "  :waveform  row=3 col=0 w=120 h=6");
        assert_eq!(
            lines[5],
            "  :hslider  row=9 col=0 w=16 h=1  value=50  min=0  max=100"
        );
    }

    #[test]
    fn waveform_layout_child_rows_match_measured_height() {
        let tree = run_prog(
            r#"
            (v-stack
              (label "a")
              (label "b")
              (label "c")
              (waveform :height 6 :view-start 0 :view-duration 1)
              (hslider :min 0 :max 100 :value 50))
            "#,
        )
        .unwrap()
        .unwrap();

        let layout = LayoutEngine::new(120, 40, 1.0)
            .layout(&tree)
            .expect("layout");

        assert_eq!(layout.children.len(), 5);
        assert_eq!(layout.children[0].widget_type, "label");
        assert_eq!(layout.children[0].rect.row, 0.0);
        assert_eq!(layout.children[1].rect.row, 1.0);
        assert_eq!(layout.children[2].rect.row, 2.0);
        assert_eq!(layout.children[3].widget_type, "waveform");
        assert_eq!(layout.children[3].rect.row, 3.0);
        assert_eq!(layout.children[3].rect.height, 6.0);
        assert_eq!(layout.children[4].widget_type, "hslider");
        assert_eq!(layout.children[4].rect.row, 9.0);
    }

    #[test]
    fn test_fmt_supports_numeric_precision() {
        let mut runtime = Runtime::new();
        assert_eq!(
            runtime.eval_str(r#"(fmt "{:.2}" 3.14159)"#),
            Ok(Some(Value::String("3.14".to_string())))
        );
        assert_eq!(
            runtime.eval_str(r#"(fmt "value: {:.1}" 12.75)"#),
            Ok(Some(Value::String("value: 12.8".to_string())))
        );
        assert_eq!(
            runtime.eval_str(r#"(fmt "hello {}" "world")"#),
            Ok(Some(Value::String("hello world".to_string())))
        );
    }

    #[test]
    fn test_substring_uses_character_indices() {
        let mut runtime = Runtime::new();
        assert_eq!(
            runtime.eval_str(r#"(substring "Tim Maia – Est Dificil" 0 12)"#),
            Ok(Some(Value::String("Tim Maia – E".to_string())))
        );
    }

    #[test]
    fn test_set_bang_updates_lisp_state_and_reruns_effects() {
        let mut runtime = Runtime::new();
        let _ = runtime
            .eval_str(
                r#"
                (def my_state_value (state 5))
                (effect (label (fmt "value: {}" my_state_value)))
                "#,
            )
            .unwrap();

        assert_eq!(
            runtime.drain_rendered_layouts(),
            vec![vec![
                ":label  row=0 col=0 w=8 h=1  text=\"value: 5\"".to_string()
            ]]
        );

        let _ = runtime.eval_str(r#"(set! my_state_value 17)"#).unwrap();

        assert_eq!(
            runtime.drain_rendered_layouts(),
            vec![vec![
                ":label  row=0 col=0 w=9 h=1  text=\"value: 17\"".to_string()
            ]]
        );
        assert_eq!(
            runtime.eval_str("my_state_value"),
            Ok(Some(Value::Number(17.0)))
        );
    }

    #[test]
    fn test_set_bang_updates_writable_registered_namespace() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("counter", Value::Number(5.0))], true);

        let _ = runtime
            .eval_str(r#"(effect (label (fmt "count: {}" APP.counter)))"#)
            .unwrap();
        let _ = runtime.drain_rendered_layouts();

        let _ = runtime.eval_str(r#"(set! APP.counter 12)"#).unwrap();

        assert_eq!(
            runtime.drain_rendered_layouts(),
            vec![vec![
                ":label  row=0 col=0 w=9 h=1  text=\"count: 12\"".to_string()
            ]]
        );
        assert_eq!(
            runtime.eval_str("APP.counter"),
            Ok(Some(Value::Number(12.0)))
        );
    }

    #[test]
    fn test_set_bang_rejects_readonly_registered_namespace() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("counter", Value::Number(5.0))], false);

        assert_eq!(
            runtime.eval_str(r#"(set! APP.counter 12)"#),
            Err(VMError::ReadonlyReactive("APP".to_string()))
        );
    }

    #[test]
    fn test_callback_driven_state_update_preserves_full_effect_layout() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 8);
        runtime
            .eval_str(
                r#"
                (def x (state 4))
                (def cb |v| (set! x v))
                (effect
                  (h-stack
                    (label "hello")
                    (hslider :min 0 :max 100 :value x :on-change cb)))
                "#,
            )
            .unwrap();

        let callback = runtime.eval_str("cb").unwrap().unwrap();
        runtime.invoke(callback, vec![Value::Number(25.0)]).unwrap();

        let layout = runtime.current_layout.as_ref().expect("layout");
        assert_eq!(layout.widget_type, "h-stack");
        assert_eq!(layout.children.len(), 2);
        assert_eq!(layout.children[0].widget_type, "label");
        assert_eq!(layout.children[1].widget_type, "hslider");
        assert_eq!(
            layout.children[0].props.get("text"),
            Some(&Value::String("hello".to_string()))
        );
        assert_eq!(
            layout.children[1].props.get("value"),
            Some(&Value::Number(25.0))
        );
    }

    #[test]
    fn invoke_global_calls_existing_function_without_compiling_a_call_expression() {
        let mut runtime = Runtime::new();
        runtime
            .eval_str("(def add-offset |value| (+ value 7))")
            .unwrap();

        assert_eq!(
            runtime.invoke_global("add-offset", vec![Value::Number(5.0)]),
            Ok(Some(Value::Number(12.0)))
        );
        assert_eq!(
            runtime.invoke_global("missing-handler", Vec::new()),
            Err(VMError::UnknownVariable("missing-handler".to_string()))
        );
    }

    #[test]
    fn test_re_evaluating_effect_replaces_previous_preview_effect() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("x", Value::Number(1.0))], true);

        runtime
            .eval_str(r#"(effect (hslider :min 0 :max 100 :value APP.x))"#)
            .unwrap();
        runtime
            .eval_str(
                r#"(effect (h-stack (label "hello") (hslider :min 0 :max 100 :value APP.x)))"#,
            )
            .unwrap();

        runtime.set_reactive("APP", "x", Value::Number(12.0));
        runtime.run_reactive_cycle();

        let layout = runtime.current_layout.as_ref().expect("layout");
        assert_eq!(layout.widget_type, "h-stack");
        assert_eq!(layout.children.len(), 2);
        assert_eq!(layout.children[0].widget_type, "label");
        assert_eq!(layout.children[1].widget_type, "hslider");
    }

    #[test]
    fn sdf_widget_captures_state_as_reactive_uniform_props() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 8);
        runtime
            .eval_str(
                r#"
                (defstate val 0.5)
                (defwidget xyz
                  :width 5 :height 5
                  :shader (sdf/layer
                            (sdf/fill (sdf/circle val) :accent)))
                (effect (xyz))
                "#,
            )
            .unwrap();

        let layout = runtime.current_layout.as_ref().expect("layout");
        assert_eq!(layout.widget_type, "xyz");
        assert_eq!(
            layout.props.get("shader-state-val"),
            Some(&Value::Number(0.5))
        );

        runtime.eval_str("(set! val 0.75)").unwrap();

        let layout = runtime.current_layout.as_ref().expect("layout");
        assert_eq!(
            layout.props.get("shader-state-val"),
            Some(&Value::Number(0.75))
        );
    }

    #[test]
    fn sdf_stdlib_is_the_module_pilot() {
        // Acceptance test for module-system spec §9: sdf-stdlib carries a
        // (module sdf) header and bare defmacro names, and consumers keep
        // calling (sdf/circle …) verbatim as qualified references.
        let mut runtime = Runtime::new();
        assert!(
            runtime.declared_modules().contains_key("sdf"),
            "sdf-stdlib should declare (module sdf)"
        );
        for key in ["sdf/circle", "sdf/rounded-rect", "sdf/lit", "sdf/hslider-fill"] {
            assert!(
                runtime.macros().contains_key(key),
                "expected macro key {key}"
            );
        }
        let shader = runtime
            .eval_str("(sdf->metal '(sdf/layer (sdf/fill (sdf/circle 0.5) (rgba 1 0 0 1))))")
            .expect("sdf->metal eval");
        let Some(Value::String(shader)) = shader else {
            panic!("sdf->metal should return a shader string");
        };
        assert!(
            !shader.starts_with("error:"),
            "shader compile failed: {shader}"
        );
    }

    #[test]
    fn layout_recomputes_when_viewport_changes() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 8);
        runtime
            .eval_str(
                r#"
                (effect
                  (timeline
                    :height 8
                    :sidebar-width 6
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list)
                    :view-start 0
                    :view-duration 16
                    :snap 1))
                "#,
            )
            .unwrap();

        let initial = runtime.current_layout.as_ref().expect("layout").rect;
        assert_eq!(initial.width, 40.0);
        assert_eq!(initial.height, 8.0);

        runtime.set_layout_viewport(72, 18);

        let resized = runtime.current_layout.as_ref().expect("layout").rect;
        assert_eq!(resized.width, 72.0);
        assert_eq!(resized.height, 8.0);
    }

    #[test]
    fn test_each_renders_widgets_for_step_list() {
        let mut runtime = Runtime::new();
        let steps = Value::List(vec![
            Rc::new(RefCell::new(Value::Map(HashMap::from([(
                "velocity".to_string(),
                Rc::new(RefCell::new(Value::Number(20.0))),
            )])))),
            Rc::new(RefCell::new(Value::Map(HashMap::from([(
                "velocity".to_string(),
                Rc::new(RefCell::new(Value::Number(50.0))),
            )])))),
        ]);
        runtime.register_reactive("pattern", vec![("steps", steps)], true);

        runtime
            .eval_str(
                r#"
                (effect
                  (h-stack
                    (each pattern.steps |step|
                      (hslider :min 0 :max 100 :bind step.velocity))))
                "#,
            )
            .unwrap();

        let layout = runtime.current_layout.as_ref().expect("layout");
        assert_eq!(layout.widget_type, "h-stack");
        assert_eq!(layout.children.len(), 2);
        assert_eq!(
            layout.children[0].props.get("value"),
            Some(&Value::Number(20.0))
        );
        assert_eq!(
            layout.children[1].props.get("value"),
            Some(&Value::Number(50.0))
        );
    }

    #[test]
    fn sdf_lighting_v2_demo_evaluates_and_emits_light_buffer() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(120, 40);

        runtime
            .eval_str(demo_source_without_window_ops())
            .expect("eval demo");

        let pending = runtime.take_pending_buffer_widget_trees();
        assert_eq!(pending.len(), 1, "demo should emit one named buffer effect");
        assert!(matches!(
            pending[0],
            crate::vm::PendingUiUpdate::FullTree(ref update)
                if matches!(update.target, EffectTarget::BufferName(ref name) if name == "*light*")
        ));
        assert!(
            runtime
                .completion_symbols()
                .iter()
                .any(|symbol| symbol == "v2-holo"),
            "demo should register v2-holo widget"
        );
        assert!(
            runtime
                .completion_symbols()
                .iter()
                .any(|symbol| symbol == "v2-gem"),
            "demo should register v2-gem widget"
        );
        assert!(
            runtime
                .completion_symbols()
                .iter()
                .any(|symbol| symbol == "v2-holo-bar"),
            "demo should register v2-holo-bar widget"
        );
    }

    #[test]
    fn subtree_form_preserves_explicit_subtree_root_metadata() {
        fn map_prop_u64(value: &Value, key: &str) -> Option<u64> {
            let Value::Map(map) = value else {
                return None;
            };
            match map.get(key).map(|value| value.borrow().clone()) {
                Some(Value::Number(n)) if n >= 0.0 && n.fract() == 0.0 => Some(n as u64),
                _ => None,
            }
        }

        fn map_prop_string(value: &Value, key: &str) -> Option<String> {
            let Value::Map(map) = value else {
                return None;
            };
            match map.get(key).map(|value| value.borrow().clone()) {
                Some(Value::String(s)) => Some(s),
                _ => None,
            }
        }

        fn first_child(value: &Value) -> Option<Value> {
            let Value::Map(map) = value else {
                return None;
            };
            match map.get("children").map(|value| value.borrow().clone()) {
                Some(Value::List(children)) => children.first().map(|child| child.borrow().clone()),
                _ => None,
            }
        }

        let mut runtime = Runtime::new();
        runtime
            .eval_str(
                r#"
                (effect-buffer "*subtree*"
                  (subtree :key "step-1"
                    (box
                      (label "hello"))))
                "#,
            )
            .expect("eval subtree");

        let pending = runtime.take_pending_buffer_widget_trees();
        let crate::vm::PendingUiUpdate::ReplaceSubtree {
            subtree_root_id,
            tree,
            ..
        } = &pending[0]
        else {
            panic!("expected replace subtree update");
        };
        let root_id = map_prop_u64(tree, "__subtree-root-id").expect("root subtree id");
        assert_eq!(*subtree_root_id, root_id);
        assert_eq!(
            map_prop_string(tree, "__stable-key").as_deref(),
            Some("step-1")
        );
        let child = first_child(tree).expect("child");
        assert_eq!(map_prop_u64(&child, "__subtree-root-id"), Some(root_id));
        assert_eq!(
            map_prop_u64(&child, "__parent-subtree-root-id"),
            Some(root_id)
        );
    }

    fn keyed_subtree_prop_cell(
        value: &Value,
        stable_key: &str,
        prop: &str,
    ) -> Option<Rc<RefCell<Value>>> {
        let Value::Map(map) = value else {
            return None;
        };
        if matches!(
            map.get("__stable-key").map(|cell| cell.borrow().clone()),
            Some(Value::String(key)) if key == stable_key
        ) {
            return map.get(prop).cloned();
        }
        let children = map.get("children")?;
        let Value::List(children) = &*children.borrow() else {
            return None;
        };
        children
            .iter()
            .find_map(|child| keyed_subtree_prop_cell(&child.borrow(), stable_key, prop))
    }

    /// Recursively finds the first `label` text in a rendered widget tree.
    fn first_label_text(value: &Value) -> Option<String> {
        let Value::Map(map) = value else {
            return None;
        };
        if let Some(Value::String(text)) = map.get("text").map(|cell| cell.borrow().clone()) {
            return Some(text);
        }
        if let Some(children) = map.get("children") {
            if let Value::List(children) = &*children.borrow() {
                for child in children {
                    if let Some(text) = first_label_text(&child.borrow()) {
                        return Some(text);
                    }
                }
            }
        }
        None
    }

    /// eseq-4kd: a keyed subtree absent from one buffer rerun (an fx-owner
    /// toggle) keeps its node and render cache; toggling back reuses the
    /// cached render without re-invoking the body — unless a reactive
    /// dependency changed while it was offscreen.
    #[test]
    fn keyed_subtree_reused_across_absence_when_dependencies_unchanged() {
        let mut runtime = Runtime::new();
        let renders: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let renders_for_native = Rc::clone(&renders);
        runtime.register_native("count-render", move |_args, _ctx| {
            *renders_for_native.borrow_mut() += 1;
            Ok(Value::Bool(true))
        });
        runtime.register_reactive(
            "APP",
            vec![
                ("owner", Value::Number(0.0)),
                ("panel-value", Value::Number(1.0)),
            ],
            false,
        );
        runtime
            .eval_str(
                r#"
                (effect-buffer "*owner*"
                  (if (= APP.owner 0)
                    (v-stack
                      (subtree :key "panel"
                        (if (count-render)
                          (label (fmt "value: {}" APP.panel-value))
                          (label "unreachable"))))
                    (label "other owner")))
                "#,
            )
            .expect("eval owner-switch effect");
        let _ = runtime.take_pending_buffer_widget_trees();
        assert_eq!(*renders.borrow(), 1, "initial render runs the body");

        // Owner switches away: the panel leaves the tree but stays cached.
        runtime.set_reactive("APP", "owner", Value::Number(1.0));
        runtime.run_reactive_cycle();
        let _ = runtime.take_pending_buffer_widget_trees();
        assert_eq!(*renders.borrow(), 1, "absence must not render the panel");

        // Owner switches back with the panel's dependency unchanged: the
        // cached render is reused, the body is not re-invoked.
        runtime.set_reactive("APP", "owner", Value::Number(0.0));
        runtime.run_reactive_cycle();
        let pending = runtime.take_pending_buffer_widget_trees();
        assert_eq!(*renders.borrow(), 1, "clean re-registration reuses cache");
        let crate::vm::PendingUiUpdate::FullTree(update) = &pending[0] else {
            panic!("expected full tree update");
        };
        assert_eq!(
            first_label_text(&update.tree).as_deref(),
            Some("value: 1"),
            "reused panel keeps its rendered content"
        );

        // Owner switches away again and the panel's dependency changes while
        // it is offscreen: the detached node collects the dirty mark without
        // emitting an update for the absent panel.
        runtime.set_reactive("APP", "owner", Value::Number(1.0));
        runtime.run_reactive_cycle();
        let _ = runtime.take_pending_buffer_widget_trees();
        runtime.set_reactive("APP", "panel-value", Value::Number(2.0));
        runtime.run_reactive_cycle();
        assert_eq!(
            runtime.take_pending_buffer_widget_trees().len(),
            0,
            "a detached panel must not emit updates"
        );
        assert_eq!(*renders.borrow(), 1);

        // Toggling back now must re-render: the cached tree is stale.
        runtime.set_reactive("APP", "owner", Value::Number(0.0));
        runtime.run_reactive_cycle();
        let pending = runtime.take_pending_buffer_widget_trees();
        assert_eq!(*renders.borrow(), 2, "dirty dependency forces a rerender");
        let crate::vm::PendingUiUpdate::FullTree(update) = &pending[0] else {
            panic!("expected full tree update");
        };
        assert_eq!(first_label_text(&update.tree).as_deref(), Some("value: 2"));
    }

    #[test]
    fn cached_subtree_annotation_preserves_cells_only_in_the_same_stable_id_context() {
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "APP",
            vec![
                ("rerun", Value::Number(0.0)),
                ("leading-sibling", Value::Bool(false)),
            ],
            false,
        );
        runtime
            .eval_str(
                r#"
                (effect-buffer "*annotation-identity*"
                  (let ((_ APP.rerun))
                    (v-stack
                      (if APP.leading-sibling (label "leading") nil)
                      (subtree :key "panel"
                        (box (label "cached"))))))
                "#,
            )
            .expect("eval annotation identity effect");
        let initial = runtime.take_pending_buffer_widget_trees();
        let crate::vm::PendingUiUpdate::FullTree(initial) = &initial[0] else {
            panic!("expected initial full tree");
        };
        let initial_children = keyed_subtree_prop_cell(&initial.tree, "panel", "children")
            .expect("initial cached subtree children");

        runtime.set_reactive("APP", "rerun", Value::Number(1.0));
        runtime.run_reactive_cycle();
        let same_context = runtime.take_pending_buffer_widget_trees();
        let crate::vm::PendingUiUpdate::FullTree(same_context) = &same_context[0] else {
            panic!("expected same-context full tree");
        };
        let same_context_children =
            keyed_subtree_prop_cell(&same_context.tree, "panel", "children")
                .expect("same-context cached subtree children");
        assert!(
            Rc::ptr_eq(&initial_children, &same_context_children),
            "unchanged cached subtree annotation must preserve cell identity"
        );

        runtime.set_reactive("APP", "leading-sibling", Value::Bool(true));
        runtime.run_reactive_cycle();
        let changed_context = runtime.take_pending_buffer_widget_trees();
        let crate::vm::PendingUiUpdate::FullTree(changed_context) = &changed_context[0] else {
            panic!("expected changed-context full tree");
        };
        let changed_context_children =
            keyed_subtree_prop_cell(&changed_context.tree, "panel", "children")
                .expect("changed-context cached subtree children");
        assert!(
            !Rc::ptr_eq(&same_context_children, &changed_context_children),
            "a changed tree path must not reuse stable-id annotation"
        );
    }

    /// eseq-4kd: reuse is refused when the subtree body's captured inputs
    /// changed, even though no reactive dependency of the subtree itself is
    /// dirty (the parent read the value; the body only captured it).
    #[test]
    fn keyed_subtree_rerenders_when_captured_inputs_change() {
        let mut runtime = Runtime::new();
        let renders: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let renders_for_native = Rc::clone(&renders);
        runtime.register_native("count-render", move |_args, _ctx| {
            *renders_for_native.borrow_mut() += 1;
            Ok(Value::Bool(true))
        });
        runtime.register_reactive(
            "APP",
            vec![
                ("owner", Value::Number(0.0)),
                ("parent-value", Value::Number(1.0)),
            ],
            false,
        );
        runtime
            .eval_str(
                r#"
                (effect-buffer "*capture*"
                  (if (= APP.owner 0)
                    (let ((captured APP.parent-value))
                      (v-stack
                        (subtree :key "panel"
                          (if (count-render)
                            (label (fmt "captured: {}" captured))
                            (label "unreachable")))))
                    (label "other owner")))
                "#,
            )
            .expect("eval capture effect");
        let _ = runtime.take_pending_buffer_widget_trees();
        assert_eq!(*renders.borrow(), 1);

        // Change the captured input while the panel is offscreen. The panel
        // never read it reactively, so its node stays clean — the input
        // snapshot comparison is what must catch this.
        runtime.set_reactive("APP", "owner", Value::Number(1.0));
        runtime.run_reactive_cycle();
        let _ = runtime.take_pending_buffer_widget_trees();
        runtime.set_reactive("APP", "parent-value", Value::Number(7.0));
        runtime.run_reactive_cycle();
        let _ = runtime.take_pending_buffer_widget_trees();

        runtime.set_reactive("APP", "owner", Value::Number(0.0));
        runtime.run_reactive_cycle();
        let pending = runtime.take_pending_buffer_widget_trees();
        assert_eq!(*renders.borrow(), 2, "changed capture forces a rerender");
        let crate::vm::PendingUiUpdate::FullTree(update) = &pending[0] else {
            panic!("expected full tree update");
        };
        assert_eq!(
            first_label_text(&update.tree).as_deref(),
            Some("captured: 7")
        );
    }

    #[test]
    fn nested_subtree_reactive_update_emits_replace_subtree() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("counter", Value::Number(1.0))], false);
        runtime
            .eval_str(
                r#"
                (effect-buffer "*subtree*"
                  (v-stack
                    (subtree :key "counter-label"
                      (label (fmt "count: {}" APP.counter)))
                    (label "static")))
                "#,
            )
            .expect("eval subtree effect");
        let _ = runtime.take_pending_buffer_widget_trees();

        runtime.set_reactive("APP", "counter", Value::Number(2.0));
        runtime.run_reactive_cycle();

        let pending = runtime.take_pending_buffer_widget_trees();
        assert_eq!(pending.len(), 1, "expected only subtree replacement");
        let crate::vm::PendingUiUpdate::ReplaceSubtree {
            subtree_root_id: _,
            tree,
            reactive_dependencies,
            ..
        } = &pending[0]
        else {
            panic!("expected replace subtree update");
        };
        assert_eq!(
            reactive_dependencies,
            &vec![crate::vm::ReactiveFieldKey {
                namespace: "APP".to_string(),
                field: "counter".to_string(),
            }]
        );

        let Value::Map(map) = tree else {
            panic!("expected subtree tree map");
        };
        let Some(text) = map.get("text").map(|value| value.borrow().clone()) else {
            panic!("expected text");
        };
        assert_eq!(text, Value::String("count: 2".to_string()));
    }

    #[test]
    fn active_subtree_reactive_update_reuses_targeted_layout() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime.register_reactive("APP", vec![("active", Value::Bool(true))], false);
        runtime
            .eval_str(
                r#"
                (effect
                  (v-stack
                    (subtree :key "row"
                      (box :width 4 :height 1 :active APP.active))
                    (label "static" :width 6)))
                "#,
            )
            .expect("install active subtree effect");
        let initial_revision = runtime.layout_revision();
        let initial_box_id = runtime
            .current_layout
            .as_ref()
            .and_then(|layout| layout.children.first())
            .map(|node| node.widget_id)
            .expect("subtree box id");
        let _ = runtime.take_dirty_widget_ids();
        let _ = runtime.drain_rendered_layouts();

        runtime.set_reactive("APP", "active", Value::Bool(false));
        runtime.run_reactive_cycle();

        let trace = runtime
            .last_ui_invalidation_trace()
            .expect("invalidation trace");
        assert_eq!(trace.relayout_mode.as_deref(), Some("subtree-reuse"));
        assert_eq!(trace.relayout_failure_reason, None);
        assert_eq!(trace.subtree_failure_reason, None);
        assert_eq!(
            runtime.layout_revision(),
            initial_revision,
            "non-size subtree prop changes should not bump the layout revision"
        );
        assert_eq!(runtime.take_dirty_widget_ids(), vec![initial_box_id]);
        assert!(
            runtime
                .current_layout
                .as_ref()
                .and_then(|layout| layout.children.first())
                .is_some_and(|node| node.props.get("active") == Some(&Value::Bool(false))),
            "targeted subtree relayout should update layout props"
        );
    }

    #[test]
    fn active_subtree_size_change_uses_targeted_relayout() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime.register_reactive("APP", vec![("height", Value::Number(1.0))], false);
        runtime
            .eval_str(
                r#"
                (effect
                  (v-stack
                    (subtree :key "row"
                      (box :width 4 :height APP.height))
                    (label "static" :width 6)))
                "#,
            )
            .expect("install active subtree effect");
        let initial_revision = runtime.layout_revision();
        let initial_row_height = runtime
            .current_layout
            .as_ref()
            .and_then(|layout| layout.children.first())
            .map(|node| node.rect.height)
            .expect("initial subtree row height");
        let initial_static_row = runtime
            .current_layout
            .as_ref()
            .and_then(|layout| layout.children.get(1))
            .map(|node| node.rect.row)
            .expect("initial static sibling row");
        let _ = runtime.take_dirty_widget_ids();
        let _ = runtime.drain_rendered_layouts();

        runtime.set_reactive("APP", "height", Value::Number(3.0));
        runtime.run_reactive_cycle();

        let trace = runtime
            .last_ui_invalidation_trace()
            .expect("invalidation trace");
        assert_eq!(trace.relayout_mode.as_deref(), Some("subtree-relayout"));
        assert_eq!(trace.relayout_failure_reason, None);
        assert_eq!(trace.subtree_failure_reason, None);
        assert!(
            runtime.layout_revision() > initial_revision,
            "size-changing subtree updates should bump the layout revision"
        );
        let layout = runtime
            .current_layout
            .as_ref()
            .expect("updated current layout");
        assert!(
            layout.children[0].rect.height > initial_row_height,
            "partial relayout should resize the changed subtree, before={} after={}",
            initial_row_height,
            layout.children[0].rect.height
        );
        assert!(
            layout.children[1].rect.row > initial_static_row,
            "partial relayout should translate following siblings after the resized subtree"
        );
    }

    #[test]
    fn active_subtree_size_change_can_grow_root_beyond_viewport() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime.register_reactive("APP", vec![("height", Value::Number(1.0))], false);
        runtime
            .eval_str(
                r#"
                (effect
                  (v-stack :width :fill
                    (subtree :key "row"
                      (box :key "target" :width :fill :height APP.height))
                    (box :key "tail" :width :fill :height 1)
                    (box :key "filler" :width :fill :height 1 :flex 1)))
                "#,
            )
            .expect("install active subtree effect");
        assert_eq!(
            runtime
                .current_layout
                .as_ref()
                .map(|layout| layout.rect.height),
            Some(10.0),
            "the initial flex layout should fill the viewport"
        );

        runtime.set_reactive("APP", "height", Value::Number(12.0));
        runtime.run_reactive_cycle();

        let trace = runtime
            .last_ui_invalidation_trace()
            .expect("invalidation trace");
        assert_eq!(trace.relayout_mode.as_deref(), Some("subtree-relayout"));
        assert_eq!(trace.relayout_failure_reason, None);
        let layout = runtime
            .current_layout
            .as_ref()
            .expect("updated current layout");
        assert_eq!(layout.rect.height, 14.0);
        assert_eq!(layout.children[0].rect.height, 12.0);
        assert!(
            layout.children[1].rect.row
                >= layout.children[0].rect.row + layout.children[0].rect.height,
            "the tail must start after the over-viewport subtree; layout={layout:#?}"
        );
    }

    #[test]
    fn whole_tree_reuse_geometry_change_bumps_layout_revision() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);

        runtime
            .eval_str(
                r#"
                (effect
                  (v-stack
                    (label "short" :width 5)
                    (label "tail" :width 4)))
                "#,
            )
            .expect("install initial layout effect");
        let initial_revision = runtime.layout_revision();
        let _ = runtime.take_dirty_widget_ids();

        runtime
            .eval_str(
                r#"
                (effect
                  (v-stack
                    (label "short" :width 8)
                    (label "tail" :width 4)))
                "#,
            )
            .expect("install width-changing layout effect");

        assert!(
            runtime.layout_revision() > initial_revision,
            "whole-tree layout reuse must bump the cache key when reused geometry changes"
        );
    }

    #[test]
    fn cached_widget_layout_restore_rejects_mismatched_fill_viewport() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(20, 5);
        runtime
            .eval_str(
                r#"
                (effect
                  (h-stack :width :fill :height 1
                    (box :key "left" :width 4 :height 1)
                    (box :key "spacer" :flex 1 :width 0 :height 1)
                    (box :key "right" :width 2 :height 1)))
                "#,
            )
            .expect("install fill-width layout");

        let cached_layout = runtime
            .current_layout
            .as_ref()
            .expect("initial layout")
            .clone();
        let cached_right_col = cached_layout
            .children
            .iter()
            .find(|node| node.stable_key.as_deref() == Some("right"))
            .expect("cached right child")
            .rect
            .col;
        let tree = runtime.current_widget_tree().expect("current widget tree");
        let snapshot = runtime.current_committed_ui_snapshot();
        let layout_revision = runtime.layout_revision();

        runtime.restore_widget_tree_with_cached_layout(
            tree,
            snapshot,
            Some(cached_layout),
            Some((40.0, 5.0)),
            0,
            layout_revision,
        );

        let restored_layout = runtime
            .current_layout
            .as_ref()
            .expect("restored layout should be available");
        let right_col = restored_layout
            .children
            .iter()
            .find(|node| node.stable_key.as_deref() == Some("right"))
            .expect("restored right child")
            .rect
            .col;

        assert_eq!(restored_layout.rect.width, 40.0);
        assert!(
            right_col > cached_right_col + 10.0,
            "right-aligned child should relayout for the wider viewport, old={cached_right_col} new={right_col}"
        );
    }

    #[test]
    fn subtree_inside_each_initial_render_keeps_children() {
        fn first_child_texts(value: &Value) -> Vec<String> {
            let Value::Map(map) = value else {
                return vec![];
            };
            let Some(Value::List(children)) =
                map.get("children").map(|value| value.borrow().clone())
            else {
                return vec![];
            };
            children
                .iter()
                .filter_map(|child| {
                    let Value::Map(child_map) = child.borrow().clone() else {
                        return None;
                    };
                    match child_map.get("text").map(|value| value.borrow().clone()) {
                        Some(Value::String(text)) => Some(text),
                        _ => None,
                    }
                })
                .collect()
        }

        let mut runtime = Runtime::new();
        runtime
            .eval_str(
                r#"
                (effect-buffer "*loop-subtree*"
                  (v-stack
                    (each '(0 1 2) |i|
                      (subtree :key (fmt "item-{}" i)
                        (label (fmt "item-{}" i))))))
                "#,
            )
            .expect("eval loop subtree effect");

        let pending = runtime.take_pending_buffer_widget_trees();
        assert_eq!(pending.len(), 1, "expected a single initial buffer tree");
        let crate::vm::PendingUiUpdate::FullTree(tree) = &pending[0] else {
            panic!("expected initial full tree update");
        };
        assert_eq!(
            first_child_texts(&tree.tree),
            vec!["item-0", "item-1", "item-2"]
        );
    }

    #[test]
    fn subtree_inside_each_reactive_update_emits_replace_subtrees() {
        let mut runtime = Runtime::new();
        runtime.register_reactive("APP", vec![("counter", Value::Number(1.0))], false);
        runtime
            .eval_str(
                r#"
                (effect-buffer "*loop-subtree*"
                  (v-stack
                    (each '(0 1 2) |i|
                      (subtree :key (fmt "item-{}" i)
                        (label (fmt "{}:{}" i APP.counter))))))
                "#,
            )
            .expect("eval loop subtree effect");
        let _ = runtime.take_pending_buffer_widget_trees();

        runtime.set_reactive("APP", "counter", Value::Number(2.0));
        runtime.run_reactive_cycle();

        let pending = runtime.take_pending_buffer_widget_trees();
        assert_eq!(
            pending.len(),
            3,
            "expected one subtree replacement per loop child"
        );
        for update in pending {
            let crate::vm::PendingUiUpdate::ReplaceSubtree {
                tree,
                reactive_dependencies,
                ..
            } = update
            else {
                panic!("expected replace subtree update");
            };
            assert_eq!(
                reactive_dependencies,
                vec![crate::vm::ReactiveFieldKey {
                    namespace: "APP".to_string(),
                    field: "counter".to_string(),
                }]
            );
            let Value::Map(map) = tree else {
                panic!("expected subtree tree map");
            };
            let Some(Value::String(text)) = map.get("text").map(|value| value.borrow().clone())
            else {
                panic!("expected subtree text");
            };
            assert!(text.ends_with(":2"), "expected updated text, got {text}");
        }
    }

    #[test]
    fn subtree_inside_grid_step_cell_keeps_full_tree_widget_shape() {
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "APP",
            vec![
                ("selected", Value::List(vec![])),
                ("active-0", Value::Bool(true)),
                ("active-1", Value::Bool(false)),
            ],
            false,
        );
        runtime
            .eval_str(
                r#"
                (effect-buffer "*grid-subtree*"
                  (grid :cols 2 :col-width 4
                    (each '(0 1) |step|
                      (let ((selected false))
                        (box :padding 0.25
                          (v-stack :align :center :gap 0.5
                            (vslider :height 4 :width 2 :min 0 :max 1 :value 0.5)
                            (box :width 3 :height 1.5
                              (label "x"))
                            (subtree :key (fmt "step-playhead-label-{}" step)
                              (box :width 3 :height 1 :align :center
                                (label (fmt "{}" step)
                                  :color (if selected
                                           :yellow
                                             (if (reactive-get "APP" (fmt "active-{}" step))
                                               :white
                                             :gray)))))))))))
                "#,
            )
            .expect("eval grid subtree effect");

        let pending = runtime.take_pending_buffer_widget_trees();
        assert_eq!(
            pending.len(),
            1,
            "expected a single initial full-tree update"
        );
        let crate::vm::PendingUiUpdate::FullTree(tree) = &pending[0] else {
            panic!("expected full tree update");
        };
        let Value::Map(map) = &tree.tree else {
            panic!("expected widget tree map, got {:?}", tree.tree);
        };
        let Some(Value::Keyword(widget_type)) = map.get("type").map(|value| value.borrow().clone())
        else {
            panic!("expected root widget type");
        };
        assert_eq!(widget_type, "grid");
    }

    #[test]
    #[ignore = "eseq-4tl: profiling harness; run explicitly while optimizing compiler/runtime speed"]
    fn profile_sdf_lighting_v2_demo_eval_speed() {
        let rounds = std::env::var("ESEQLISP_PROFILE_ITERS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(10);

        let mut parse_samples = Vec::with_capacity(rounds);
        let mut ast_samples = Vec::with_capacity(rounds);
        let mut compile_samples = Vec::with_capacity(rounds);
        let mut execute_samples = Vec::with_capacity(rounds);
        let mut clear_layout_samples = Vec::with_capacity(rounds);
        let mut sync_theme_samples = Vec::with_capacity(rounds);
        let mut cache_samples = Vec::with_capacity(rounds);
        let mut flush_widget_samples = Vec::with_capacity(rounds);
        let mut total_samples = Vec::with_capacity(rounds);
        let mut parser_profiles = Vec::with_capacity(rounds);

        for _ in 0..rounds {
            let mut parser = Parser::new(demo_source_without_window_ops().to_string());
            parser.parse().expect("tokenize demo");
            parser_profiles.push(parser.profile());

            let mut runtime = Runtime::new();
            runtime.set_layout_viewport(120, 40);
            let total_started = Instant::now();
            let (_, profile) = runtime
                .profile_eval_str(demo_source_without_window_ops())
                .expect("profile demo");
            total_samples.push(total_started.elapsed());
            parse_samples.push(profile.vm_parse);
            ast_samples.push(profile.vm_ast);
            compile_samples.push(profile.vm_compile);
            execute_samples.push(profile.vm_execute);
            clear_layout_samples.push(profile.clear_layout_effects);
            sync_theme_samples.push(profile.sync_theme);
            cache_samples.push(profile.invalidate_symbol_cache);
            flush_widget_samples.push(profile.flush_widget_trees);
        }

        let (parse_min, parse_median, parse_avg) = duration_stats(parse_samples);
        let (ast_min, ast_median, ast_avg) = duration_stats(ast_samples);
        let (compile_min, compile_median, compile_avg) = duration_stats(compile_samples);
        let (execute_min, execute_median, execute_avg) = duration_stats(execute_samples);
        let (clear_min, clear_median, clear_avg) = duration_stats(clear_layout_samples);
        let (sync_min, sync_median, sync_avg) = duration_stats(sync_theme_samples);
        let (cache_min, cache_median, cache_avg) = duration_stats(cache_samples);
        let (flush_min, flush_median, flush_avg) = duration_stats(flush_widget_samples);
        let (total_min, total_median, total_avg) = duration_stats(total_samples);
        let avg_parser_profile = {
            let runs = parser_profiles.len().max(1);
            let sum = parser_profiles.iter().fold(
                crate::parser::ParserProfile::default(),
                |mut acc, profile| {
                    acc.input_bytes += profile.input_bytes;
                    acc.peek_calls += profile.peek_calls;
                    acc.next_calls += profile.next_calls;
                    acc.peek_nth_calls += profile.peek_nth_calls;
                    acc.estimated_char_visits += profile.estimated_char_visits;
                    acc.parse_text_calls += profile.parse_text_calls;
                    acc.skip_whitespace_loops += profile.skip_whitespace_loops;
                    acc.parse_symbol_calls += profile.parse_symbol_calls;
                    acc.parse_number_calls += profile.parse_number_calls;
                    acc.parse_string_calls += profile.parse_string_calls;
                    acc.comments_skipped += profile.comments_skipped;
                    acc.tokens_emitted += profile.tokens_emitted;
                    acc
                },
            );
            crate::parser::ParserProfile {
                input_bytes: sum.input_bytes / runs,
                peek_calls: sum.peek_calls / runs,
                next_calls: sum.next_calls / runs,
                peek_nth_calls: sum.peek_nth_calls / runs,
                estimated_char_visits: sum.estimated_char_visits / runs,
                parse_text_calls: sum.parse_text_calls / runs,
                skip_whitespace_loops: sum.skip_whitespace_loops / runs,
                parse_symbol_calls: sum.parse_symbol_calls / runs,
                parse_number_calls: sum.parse_number_calls / runs,
                parse_string_calls: sum.parse_string_calls / runs,
                comments_skipped: sum.comments_skipped / runs,
                tokens_emitted: sum.tokens_emitted / runs,
            }
        };

        eprintln!(
            "sdf-lighting-v2-demo profile over {rounds} runs\n  source bytes        avg={}\n  parser tokens       avg={}\n  parser peek calls   avg={}\n  parser next calls   avg={}\n  parser peek_nth     avg={}\n  parser est charwalk avg={} ({:.1}x input)\n  parser symbols      avg={}\n  parser numbers      avg={}\n  parser strings      avg={}\n  parser comments     avg={}\n  total               min={:.2}ms median={:.2}ms avg={:.2}ms\n  clear-layout        min={:.2}ms median={:.2}ms avg={:.2}ms\n  vm parse            min={:.2}ms median={:.2}ms avg={:.2}ms\n  vm ast              min={:.2}ms median={:.2}ms avg={:.2}ms\n  vm compile          min={:.2}ms median={:.2}ms avg={:.2}ms\n  vm execute          min={:.2}ms median={:.2}ms avg={:.2}ms\n  sync-theme          min={:.2}ms median={:.2}ms avg={:.2}ms\n  invalidate-cache    min={:.2}ms median={:.2}ms avg={:.2}ms\n  flush-widget-trees  min={:.2}ms median={:.2}ms avg={:.2}ms",
            avg_parser_profile.input_bytes,
            avg_parser_profile.tokens_emitted,
            avg_parser_profile.peek_calls,
            avg_parser_profile.next_calls,
            avg_parser_profile.peek_nth_calls,
            avg_parser_profile.estimated_char_visits,
            avg_parser_profile.estimated_char_visits as f64
                / avg_parser_profile.input_bytes.max(1) as f64,
            avg_parser_profile.parse_symbol_calls,
            avg_parser_profile.parse_number_calls,
            avg_parser_profile.parse_string_calls,
            avg_parser_profile.comments_skipped,
            total_min.as_secs_f64() * 1_000.0,
            total_median.as_secs_f64() * 1_000.0,
            total_avg.as_secs_f64() * 1_000.0,
            clear_min.as_secs_f64() * 1_000.0,
            clear_median.as_secs_f64() * 1_000.0,
            clear_avg.as_secs_f64() * 1_000.0,
            parse_min.as_secs_f64() * 1_000.0,
            parse_median.as_secs_f64() * 1_000.0,
            parse_avg.as_secs_f64() * 1_000.0,
            ast_min.as_secs_f64() * 1_000.0,
            ast_median.as_secs_f64() * 1_000.0,
            ast_avg.as_secs_f64() * 1_000.0,
            compile_min.as_secs_f64() * 1_000.0,
            compile_median.as_secs_f64() * 1_000.0,
            compile_avg.as_secs_f64() * 1_000.0,
            execute_min.as_secs_f64() * 1_000.0,
            execute_median.as_secs_f64() * 1_000.0,
            execute_avg.as_secs_f64() * 1_000.0,
            sync_min.as_secs_f64() * 1_000.0,
            sync_median.as_secs_f64() * 1_000.0,
            sync_avg.as_secs_f64() * 1_000.0,
            cache_min.as_secs_f64() * 1_000.0,
            cache_median.as_secs_f64() * 1_000.0,
            cache_avg.as_secs_f64() * 1_000.0,
            flush_min.as_secs_f64() * 1_000.0,
            flush_median.as_secs_f64() * 1_000.0,
            flush_avg.as_secs_f64() * 1_000.0,
        );

        if let Some(max_avg_ms) = std::env::var("ESEQLISP_MAX_AVG_EVAL_MS")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
        {
            assert!(
                total_avg.as_secs_f64() * 1_000.0 <= max_avg_ms,
                "average eval time {:.2}ms exceeded threshold {:.2}ms",
                total_avg.as_secs_f64() * 1_000.0,
                max_avg_ms
            );
        }
    }

    #[test]
    fn test_widget_as_arg_to_function_call() {
        // A widget builder (v-stack) with keyword args passed as an argument
        // to a regular function should produce exactly one value on the stack.
        // The parent function should receive the string and the widget map
        // as two separate arguments.
        let result = run_prog(
            r#"
            (def capture (name tree) (list name (get tree :type)))
            (capture "hello" (v-stack :padding 1 (label "child")))
            "#,
        )
        .unwrap()
        .unwrap();

        let Value::List(items) = result else {
            panic!("expected list, got: {result:?}");
        };
        assert_eq!(items.len(), 2, "capture should receive exactly 2 args");
        assert_eq!(
            items[0].borrow().clone(),
            Value::String("hello".to_string()),
            "first arg should be the string"
        );
        assert_eq!(
            items[1].borrow().clone(),
            Value::Keyword("v-stack".to_string()),
            "second arg should be the v-stack widget's :type"
        );
    }

    #[test]
    fn test_widget_as_arg_to_runtime_native() {
        // Test with a runtime-level native (like render-widget-to-buffer)
        // that receives a string and a widget tree as two args.
        use std::sync::{Arc, Mutex};
        let captured_args: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let cap = captured_args.clone();

        let mut runtime = Runtime::new();
        runtime.register_native("test-capture", move |args, _ctx| {
            let mut out = cap.lock().unwrap();
            for arg in &args {
                out.push(format!("{:?}", std::mem::discriminant(arg)));
            }
            Ok(Value::Number(args.len() as f64))
        });

        let result = runtime
            .eval_str(r#"(test-capture "*buf*" (v-stack :padding 1 (label "x")))"#)
            .unwrap()
            .unwrap();

        let args = captured_args.lock().unwrap();
        assert_eq!(
            result,
            Value::Number(2.0),
            "native should receive exactly 2 args, got {} args: {:?}",
            args.len(),
            *args
        );
    }

    /// Regression: confirm that `(h-stack ... (map fn (chunks list n)) other)`
    /// correctly flattens the mapped list of v-stacks as h-stack children.
    /// This is the pattern used by `ui-rack` in ui/effects.lisp; if the
    /// list isn't spliced, instruments only render one column instead of
    /// multiple.
    #[test]
    fn test_map_chunks_flattens_into_hstack_children() {
        let mut runtime = Runtime::new();
        let tree = runtime
            .eval_str(
                r#"
                (def make-panel (title)
                  (box :width 5 :height 2 (label title)))
                (def make-col (panels)
                  (v-stack :width 5 :gap 0.1 panels))
                (h-stack :width :fill :gap 0.5
                  (map make-col
                       (chunks (list (make-panel "a") (make-panel "b")
                                     (make-panel "c") (make-panel "d"))
                               2))
                  (label "ADSR")
                  (map make-col
                       (chunks (list (make-panel "e") (make-panel "f"))
                               2)))
                "#,
            )
            .unwrap()
            .unwrap();

        // Find the h-stack's children list and count widget-typed entries.
        let Value::Map(map) = &tree else {
            panic!("expected widget map, got {:?}", tree);
        };
        let children_value = map
            .get("children")
            .expect("h-stack has children")
            .borrow()
            .clone();
        let Value::List(children) = children_value else {
            panic!("expected children list");
        };
        let widget_count = children
            .iter()
            .filter(|c| matches!(&*c.borrow(), Value::Map(m) if m.contains_key("type")))
            .count();
        // Expected: 2 v-stacks (left), 1 label (ADSR), 1 v-stack (right) = 4
        assert_eq!(
            widget_count, 4,
            "h-stack should have 4 widget children after list flattening, got {widget_count}"
        );
    }

    /// Verify the ui-rack pattern from ui/effects.lisp produces the expected
    /// number of column children (4 panels into 2-per-column = 2 v-stacks per side).
    #[test]
    fn test_ui_rack_breathe_produces_correct_columns() {
        let mut runtime = Runtime::new();
        let tree = runtime
            .eval_str(
                r#"
                (def panel (title) (box :width 5 :height 3 (label title)))
                (def col-breathe (col) (v-stack :width 22.0 :gap 0.10 col))
                (def ui-rack (mode left adsr right)
                  (h-stack :width :fill :gap 0.4 :align :stretch
                    (map col-breathe (chunks left 2))
                    adsr
                    (map col-breathe (chunks right 2))))
                (ui-rack :breathe
                  (list (panel "GLOBAL") (panel "VCO 1")
                        (panel "VCO 2 / MIX") (panel "DIRT"))
                  (box :width 23 :height 8 (label "ADSR"))
                  (list (panel "MS FILTER") (panel "HP / SCREAM")
                        (panel "MOD") (panel "NOISE / RING")))
                "#,
            )
            .unwrap()
            .unwrap();

        let Value::Map(map) = &tree else {
            panic!("expected widget map, got {:?}", tree);
        };
        let Value::Keyword(typ) = &*map.get("type").unwrap().borrow() else {
            panic!("expected type keyword");
        };
        assert_eq!(typ, "h-stack");

        let children_value = map.get("children").unwrap().borrow().clone();
        let Value::List(children) = children_value else {
            panic!("expected children list");
        };
        // Expected: 2 v-stacks (left cols) + 1 box (adsr) + 2 v-stacks (right cols) = 5
        let kinds: Vec<String> = children
            .iter()
            .filter_map(|c| {
                let v = c.borrow();
                if let Value::Map(m) = &*v {
                    if let Some(t) = m.get("type") {
                        if let Value::Keyword(k) = &*t.borrow() {
                            return Some(k.clone());
                        }
                    }
                }
                None
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["v-stack", "v-stack", "box", "v-stack", "v-stack"],
            "ui-rack should produce 2 left cols + adsr + 2 right cols"
        );

        // Verify the v-stack has correct :width set
        let Value::Map(first_col) = &*children[0].borrow() else {
            panic!()
        };
        let width = first_col.get("width").unwrap().borrow().clone();
        assert!(
            matches!(width, Value::Number(n) if (n - 22.0).abs() < 0.01),
            "first v-stack should have :width 22.0, got {:?}",
            width
        );
    }

    /// Verify that a macro that wraps a `(do ...)` body around an impl call
    /// — the pattern used by metal-seq-fx's `ui-panel` to publish the panel
    /// section to descendants — produces a usable widget.
    #[test]
    fn test_ui_panel_macro_expansion_produces_widget() {
        let mut runtime = Runtime::new();
        let widget = runtime
            .eval_str(
                r#"
                (defstate current-section -1)
                (def panel-impl (title section body)
                  (box :width :fill :height 3.4 :on-click (lambda (info) section) body))
                (defmacro panel (title section body)
                  `(do
                     (set! current-section ,section)
                     (let ((__p (panel-impl ,title ,section ,body)))
                       (set! current-section -1)
                       __p)))
                (panel "X" 1 (label "knob"))
                "#,
            )
            .unwrap()
            .unwrap();

        let Value::Map(map) = &widget else {
            panic!("expected widget map, got {:?}", widget);
        };
        let typ = map.get("type").unwrap().borrow().clone();
        assert!(
            matches!(typ, Value::Keyword(ref k) if k == "box"),
            "macro should produce a box widget at the top, got {:?}",
            typ
        );
    }

    #[test]
    fn test_widget_keyword_args_dont_leak_to_parent() {
        // Regression: when (v-stack :padding 1 ...) appeared as an arg to
        // a function, :padding leaked as a separate arg to the parent.
        let result = run_prog(
            r#"
            (def arg-count (a b) 2)
            (arg-count "first" (v-stack :padding 1 (label "x")))
            "#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result,
            Value::Number(2.0),
            "function should receive exactly 2 args"
        );
    }

    #[test]
    fn test_set_statement_does_not_leak_operand_stack_values() {
        let result = run_prog(
            r#"
            (def current "")
            (def make-child ()
              (do
                (set! current "leaked-if-stack-is-wrong")
                (box :debug-name "child")))
            (def capture (name child)
              (list name (get child :debug-name)))
            (capture "expected-name" (make-child))
            "#,
        )
        .unwrap()
        .unwrap();

        let Value::List(items) = result else {
            panic!("expected list, got: {result:?}");
        };
        assert_eq!(
            items[0].borrow().clone(),
            Value::String("expected-name".to_string()),
            "set! must not leave stale values that shift caller arguments"
        );
        assert_eq!(
            items[1].borrow().clone(),
            Value::String("child".to_string())
        );
    }

    #[test]
    fn test_def_inside_do_is_expression_valued() {
        let result = run_prog("(do (def local-in-do 41) (+ local-in-do 1))")
            .unwrap()
            .unwrap();
        assert_eq!(result, Value::Number(42.0));
    }

    #[test]
    fn set_reactive_list_index_updates_only_target_slot_value() {
        let mut runtime = Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime.register_reactive(
            "APP",
            vec![(
                "values",
                Value::List(vec![
                    Rc::new(RefCell::new(Value::Number(1.0))),
                    Rc::new(RefCell::new(Value::Number(2.0))),
                    Rc::new(RefCell::new(Value::Number(3.0))),
                ]),
            )],
            true,
        );
        runtime
            .eval_str(
                r#"
                (effect
                  (h-stack
                    (mixer-meter
                      :level-l (bind-nth "APP" "values" 0)
                      :level-r 0.0
                      :width 2.22 :height 4.24)
                    (mixer-meter
                      :level-l (bind-nth "APP" "values" 1)
                      :level-r 0.0
                      :width 2.22 :height 4.24)))
                "#,
            )
            .expect("install indexed binding effect");
        let layout = runtime.current_layout.as_ref().expect("bound meter layout");
        let second_meter_id = layout.children[1].widget_id;
        let _ = runtime.take_dirty_widget_ids();
        let _ = runtime.drain_rendered_layouts();

        runtime.set_reactive_list_index("APP", "values", 1, Value::Number(0.75));
        assert_eq!(runtime.take_dirty_widget_ids(), vec![second_meter_id]);
        runtime.run_reactive_cycle();

        let result = runtime.eval_str("APP.values").unwrap().unwrap();
        let Value::List(items) = result else {
            panic!("expected APP.values to remain a list, got {result:?}");
        };
        assert_eq!(items[0].borrow().clone(), Value::Number(1.0));
        assert_eq!(items[1].borrow().clone(), Value::Number(0.75));
        assert_eq!(items[2].borrow().clone(), Value::Number(3.0));
        assert!(
            runtime.drain_rendered_layouts().is_empty(),
            "indexed binding-only partial writes must not rerun effects"
        );
    }

    #[test]
    fn set_reactive_list_index_invalidates_direct_bus_solo_read_effects() {
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![(
                "bus-solos",
                Value::List(vec![
                    Rc::new(RefCell::new(Value::Bool(false))),
                    Rc::new(RefCell::new(Value::Bool(false))),
                ]),
            )],
            false,
        );
        runtime
            .eval_str(
                r#"
                (effect-buffer "*mixer*"
                  (v-stack
                    (subtree :key "bus-0"
                      (label (fmt "bus0:{}" (nth SEQ.bus-solos 0))))
                    (subtree :key "bus-1"
                      (label (fmt "bus1:{}" (nth SEQ.bus-solos 1))))))
                "#,
            )
            .expect("install direct bus solo read effect");
        let _ = runtime.take_pending_buffer_widget_trees();

        runtime.set_reactive_list_index("SEQ", "bus-solos", 1, Value::Bool(true));
        runtime.run_reactive_cycle();

        assert!(
            !runtime.take_pending_buffer_widget_trees().is_empty(),
            "partial bus solo writes must dirty effects that read SEQ.bus-solos directly"
        );
    }

    fn fx_panel_value(name: &str, value: f64) -> Value {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(name.to_string()))),
        );
        map.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(value))),
        );
        Value::List(vec![Rc::new(RefCell::new(Value::Map(map)))])
    }

    fn install_fx_reader(runtime: &mut Runtime) {
        runtime.register_reactive(
            "SEQ",
            vec![("effects", fx_panel_value("Delay", 0.5))],
            false,
        );
        runtime
            .eval_str(
                r#"
                (effect-buffer "*fx*"
                  (v-stack
                    (each SEQ.effects |fx idx|
                      (label (fmt "{}:{}" (get fx :name) (get fx :value))))))
                "#,
            )
            .expect("install fx reader effect");
        let _ = runtime.take_pending_buffer_widget_trees();
    }

    #[test]
    fn set_reactive_value_patch_skips_rerun_for_value_only_diffs() {
        let mut runtime = Runtime::new();
        install_fx_reader(&mut runtime);

        let result =
            runtime.set_reactive_value_patch("SEQ", "effects", fx_panel_value("Delay", 0.9));
        assert!(!result.changed && !result.effects_dirty && !result.widgets_dirty);
        runtime.run_reactive_cycle();
        assert!(
            runtime.take_pending_buffer_widget_trees().is_empty(),
            "value-only patches must not rerun the fx reader effect"
        );

        // The patched cell is shared with the VM global: the next honest
        // rerun (structural change) must render the patched number.
        let value = runtime
            .eval_str("(get (nth SEQ.effects 0) :value)")
            .unwrap()
            .unwrap();
        assert_eq!(value, Value::Number(0.9));
        // A rebuilt identical tree now compares equal: no rerun either.
        let result =
            runtime.set_reactive_value_patch("SEQ", "effects", fx_panel_value("Delay", 0.9));
        assert!(!result.changed);
    }

    #[test]
    fn set_reactive_value_patch_falls_back_on_string_change() {
        let mut runtime = Runtime::new();
        install_fx_reader(&mut runtime);

        let result =
            runtime.set_reactive_value_patch("SEQ", "effects", fx_panel_value("Reverb", 0.5));
        assert!(result.changed, "renames are structural");
        runtime.run_reactive_cycle();
        assert!(
            !runtime.take_pending_buffer_widget_trees().is_empty(),
            "structural fallbacks must rerun the fx reader effect"
        );
    }

    #[test]
    fn set_reactive_value_patch_falls_back_on_shape_change() {
        let mut runtime = Runtime::new();
        install_fx_reader(&mut runtime);

        let Value::List(mut items) = fx_panel_value("Delay", 0.5) else {
            unreachable!();
        };
        items.extend(match fx_panel_value("Chorus", 0.1) {
            Value::List(extra) => extra,
            _ => unreachable!(),
        });
        let result = runtime.set_reactive_value_patch("SEQ", "effects", Value::List(items));
        assert!(result.changed, "list growth is structural");
        runtime.run_reactive_cycle();
        assert!(
            !runtime.take_pending_buffer_widget_trees().is_empty(),
            "shape fallbacks must rerun the fx reader effect"
        );
    }
}

#[cfg(test)]
mod modal_layout_tests {
    use crate::layout::{LayoutEngine, LayoutNode, Rect};
    use crate::run_prog;

    fn find_widget<'a>(node: &'a LayoutNode, widget_type: &str) -> Option<&'a LayoutNode> {
        if node.widget_type == widget_type {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_widget(child, widget_type))
    }

    fn modal_tree(is_open: bool) -> crate::vm::Value {
        run_prog(&format!(
            r#"
            (v-stack
              (label "before")
              (modal :is-open {is_open}
                (v-stack
                  (label "modal body")
                  (scroll :height 6
                    (each (list "a" "b" "c") |s| (label s)))))
              (label "after"))
            "#
        ))
        .unwrap()
        .unwrap()
    }

    #[test]
    fn open_modal_leaves_parent_geometry_unchanged() {
        let engine = LayoutEngine::new(80, 24, 1.0);
        let closed = engine.layout(&modal_tree(false)).expect("closed layout");
        let open = engine.layout(&modal_tree(true)).expect("open layout");

        assert_eq!(closed.rect, open.rect, "root rect must not shift on open");
        // Sibling labels keep identical geometry in both states.
        for (closed_child, open_child) in closed.children.iter().zip(open.children.iter()) {
            assert_eq!(
                closed_child.rect, open_child.rect,
                "sibling {} shifted when the modal opened",
                open_child.widget_type
            );
        }
        let closed_modal = find_widget(&closed, "modal").expect("modal node");
        let open_modal = find_widget(&open, "modal").expect("modal node");
        assert_eq!(closed_modal.rect.width, 0.0);
        assert_eq!(closed_modal.rect.height, 0.0);
        assert_eq!(open_modal.rect.width, 0.0);
        assert_eq!(open_modal.rect.height, 0.0);
        assert!(
            closed_modal.children.is_empty(),
            "closed modal lays out no children"
        );
        assert_eq!(
            open_modal.children.len(),
            1,
            "open modal lays out its child"
        );
    }

    #[test]
    fn open_modal_centers_against_the_fallback_tile_viewport() {
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&modal_tree(true)).expect("layout");
        let modal = find_widget(&layout, "modal").expect("modal node");

        // Fallback frame = the 80x24 tile. Default panel = 70% of the frame,
        // centered: rect (12, 3.6) 56x16.8. Content is inset by the panel
        // padding (1.5 cols, 0.7 rows).
        let child = &modal.children[0];
        assert!(
            (child.rect.col - 13.5).abs() < 0.01,
            "col {}",
            child.rect.col
        );
        assert!(
            (child.rect.row - 4.3).abs() < 0.01,
            "row {}",
            child.rect.row
        );
        assert!(
            (child.rect.width - 53.0).abs() < 0.01,
            "width {}",
            child.rect.width
        );
        assert!(
            (child.rect.height - 15.4).abs() < 0.01,
            "height {}",
            child.rect.height
        );

        // Children (including the `each` rows inside scroll) stay within the
        // panel rect.
        let panel = Rect {
            row: 3.6,
            col: 12.0,
            width: 56.0,
            height: 16.8,
        };
        fn assert_within(node: &LayoutNode, panel: Rect) {
            // Scroll content may extend below the clip; the scroll viewport
            // itself must fit.
            assert!(
                node.rect.col >= panel.col - 0.01,
                "{} col {}",
                node.widget_type,
                node.rect.col
            );
            assert!(
                node.rect.row >= panel.row - 0.01,
                "{} row {}",
                node.widget_type,
                node.rect.row
            );
            assert!(
                node.rect.col + node.rect.width <= panel.col + panel.width + 0.01,
                "{} right {}",
                node.widget_type,
                node.rect.col + node.rect.width
            );
            if node.widget_type == "scroll" {
                return;
            }
            for child in &node.children {
                assert_within(child, panel);
            }
        }
        assert_within(child, panel);
    }

    #[test]
    fn open_modal_centers_against_a_backend_frame_viewport() {
        let mut engine = LayoutEngine::new(30, 8, 1.0);
        engine.frame_viewport = Some(Rect {
            row: -6.0,
            col: -4.0,
            width: 60.0,
            height: 40.0,
        });
        let layout = engine.layout(&modal_tree(true)).expect("layout");
        let modal = find_widget(&layout, "modal").expect("modal node");
        let child = &modal.children[0];

        // Panel = 70% of the 60x40 frame centered at its (offset) origin:
        // rect (5, 0) 42x28; the child content is inset by the panel padding.
        assert!(
            (child.rect.col - 6.5).abs() < 0.01,
            "col {}",
            child.rect.col
        );
        assert!(
            (child.rect.row - 0.7).abs() < 0.01,
            "row {}",
            child.rect.row
        );
        assert!(
            (child.rect.width - 39.0).abs() < 0.01,
            "width {}",
            child.rect.width
        );
        assert!(
            (child.rect.height - 26.6).abs() < 0.01,
            "height {}",
            child.rect.height
        );
        // The subtree escapes the 30x8 tile: content extends past row 8.
        assert!(child.rect.row + child.rect.height > 8.0);
    }

    #[test]
    fn modal_pixel_dimensions_are_stable_in_layout_cells() {
        let tree = run_prog(
            r#"
            (modal :is-open true :width-px 960 :height-px 800
              (box :width :fill :height :fill))
            "#,
        )
        .unwrap()
        .unwrap();
        let mut engine = LayoutEngine::new(200, 100, 1.0);
        engine.cell_w = 10.0;
        engine.cell_h = 20.0;

        let layout = engine.layout(&tree).expect("layout");
        let modal = find_widget(&layout, "modal").expect("modal node");
        let child = &modal.children[0];

        // 960x800 px becomes 96x40 cells, centered in the 200x100 frame,
        // then inset by the modal's 1.5x0.7-cell content padding.
        assert!(
            (child.rect.col - 53.5).abs() < 0.01,
            "col {}",
            child.rect.col
        );
        assert!(
            (child.rect.row - 30.7).abs() < 0.01,
            "row {}",
            child.rect.row
        );
        assert!(
            (child.rect.width - 93.0).abs() < 0.01,
            "width {}",
            child.rect.width
        );
        assert!(
            (child.rect.height - 38.6).abs() < 0.01,
            "height {}",
            child.rect.height
        );
    }

    #[test]
    fn open_modal_emits_overlay_primitives_beyond_a_short_tile() {
        use crate::widget_render::{
            OverlayKind, WidgetViewport, clear_overlay, collect_gpu_primitives, topmost_overlay,
        };

        clear_overlay();
        let mut engine = LayoutEngine::new(30, 8, 1.0);
        engine.frame_viewport = Some(Rect {
            row: -6.0,
            col: -4.0,
            width: 60.0,
            height: 40.0,
        });
        let layout = engine.layout(&modal_tree(true)).expect("layout");
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 600.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 34.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };

        let (tile_prims, overlay_prims) = collect_gpu_primitives(&layout, viewport, 0.0, 8);
        assert!(
            !overlay_prims.is_empty(),
            "open modal must emit overlay primitives"
        );
        let entry = topmost_overlay().expect("open modal registers an overlay entry");
        assert_eq!(entry.kind, OverlayKind::Modal);
        assert!(entry.rect.width.is_finite() && entry.rect.width > 0.0);
        assert!(entry.rect.height.is_finite() && entry.rect.height > 0.0);
        // Panel escapes the 8-row tile.
        assert!(entry.rect.row + entry.rect.height > 8.0);
        // Nothing from the modal subtree leaks into the tile scene: the modal
        // body label must not appear among tile primitives.
        let tile_text: Vec<String> = tile_prims
            .iter()
            .filter_map(|prim| match prim {
                crate::widget_render::GpuPrimitive::ProportionalText(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            !tile_text.iter().any(|text| text == "modal body"),
            "modal subtree leaked into the tile scene: {tile_text:?}"
        );
        clear_overlay();
    }
}
