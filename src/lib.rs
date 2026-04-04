// ── Domain modules ───────────────────────────────────────────────────────
pub mod lang;
pub mod ui;

// Re-export submodules at crate root for backward-compatible paths
// (e.g. crate::vm, crate::layout still work).
pub use lang::compiler;
pub use lang::parser;
pub use lang::vm;

pub use ui::backend;
pub use ui::frame;
pub use ui::glyph_atlas;
pub use ui::layout;
pub use ui::metal_backend;
pub use ui::theme;
pub use ui::tui;

// ── Root-level modules ───────────────────────────────────────────────────
pub mod audio;
pub mod buffer;
pub mod editor;
pub mod host;
pub mod mode;
pub mod reactive;
pub mod runtime;
pub mod text;
pub mod tile;
pub mod widget_render;
pub mod widgets;

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
pub use mode::BufferMode;
pub use runtime::{NativeContext, NativeResult, Runtime, RuntimeError, SymbolMetadata};

#[allow(dead_code)]
pub fn run_prog(prog: &str) -> Result<Option<Value>, VMError> {
    let mut runtime = Runtime::new();
    runtime.eval_str(prog)
}

pub fn run_editor(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let init_src = std::fs::read_to_string("init.lisp").unwrap_or_default();
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
        },
    );

    loop {
        editor.update_timers();
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

    let init_src = std::fs::read_to_string("init.lisp").unwrap_or_default();
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
        },
    );
    let mut backend = MetalBackend::new()?;
    backend.initialize()?;

    // Set up proportional text measurement for widget layout.
    {
        let (cell_w, cell_h) = backend.cell_dimensions();
        if let Some(measurer) = backend.create_text_measurer() {
            editor.set_text_measurer(measurer, cell_w, cell_h);
        }
    }

    let frame_interval = Duration::from_secs_f64(1.0 / 30.0);
    let mut last_render_at = Instant::now() - frame_interval;
    let mut pending_drag: Option<(Event, (f32, f32))> = None;
    #[allow(unused_mut)]
    let mut scroll_accum_y: f32 = 0.0;
    #[allow(unused_mut)]
    let mut scroll_accum_x: f32 = 0.0;

    loop {
        editor.update_timers();
        let (cols, rows) = backend.viewport_size();
        // Set aspect ratio for uniform spacing (cell_h / cell_w)
        let (cell_w, cell_h) = backend.cell_dimensions();
        if cell_w > 0.0 {
            editor.set_layout_aspect(cell_h / cell_w);
        }
        // Update tile rects once per iteration (not per-event)
        editor.update_tile_rects(cols as u16, rows as u16);
        let timeout = frame_interval.saturating_sub(last_render_at.elapsed());
        match backend.poll_event(timeout) {
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
                }
            }
            Some(Event::Resize(_, _)) => editor.mark_needs_redraw(),
            _ => {}
        }

        // Process touchpad gestures (Metal-specific, not crossterm events)
        while let Some((delta, (precise_col, precise_row))) = backend.take_pending_magnify() {
            editor.handle_tiled_touchpad_magnify(precise_col, precise_row, 0, delta);
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

        if last_render_at.elapsed() >= frame_interval {
            if let Some((Event::Mouse(mouse), (precise_col, precise_row))) = pending_drag.take() {
                editor.handle_tiled_mouse_precise(
                    mouse,
                    precise_col,
                    precise_row,
                    0, // Metal: no cell borders
                );
            }
        }

        if last_render_at.elapsed() >= frame_interval {
            let tiled_frame = frame::build_tiled_render_frame_borderless(&mut editor, cols, rows);
            backend.render_tiled(&tiled_frame)?;
            editor.clear_needs_redraw();
            last_render_at = Instant::now();
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
    #[cfg(target_os = "macos")]
    use crate::widget_render::{MetalPrimitive, widget_primitives_for_node};

    const SDF_LIGHTING_V2_DEMO: &str = include_str!("../sdf-lighting-v2-demo.lisp");

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
    fn test_tabs_do_not_add_extra_top_gap_for_shorter_selected_tab() {
        let tree = run_prog(
            r#"
            (tabs :items (list "A" "B") :value 0
              (label "short")
              (v-stack
                (label "one")
                (label "two")
                (label "three")))
            "#,
        )
        .unwrap()
        .unwrap();

        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&tree)
            .expect("layout");
        let lines = format_layout_tree_lines(&layout, 0);

        assert_eq!(lines[0], ":tabs  row=0 col=0 w=80 h=5  value=0");
        assert!(lines[1].contains("row=2 col=0"), "{}", lines[1]);
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

    #[cfg(target_os = "macos")]
    #[test]
    fn test_label_metal_primitives_are_clipped_to_label_rect() {
        let mut runtime = Runtime::new();
        let tree = runtime
            .eval_str(r#"(label "a very long label" :width 4 :bg :transparent)"#)
            .unwrap()
            .unwrap();
        let layout = LayoutEngine::new(80, 24, 1.0)
            .layout(&tree)
            .expect("layout");
        let primitives = widget_primitives_for_node(
            &layout,
            crate::widget_render::WidgetViewport {
                scroll_top: 0.0,
                scroll_left: 0.0,
                vp_w: 80.0,
                vp_h: 24.0,
                cell_w: 1.0,
                cell_h: 1.0,
                time_seconds: 0.0,
                focused_widget_id: None,
                focused_branch: false,
                tile_content_rows: 24.0,
            },
        );

        assert!(matches!(primitives.first(), Some(MetalPrimitive::PushClipRect(_))));
        assert!(matches!(primitives.get(1), Some(MetalPrimitive::ProportionalText(_))));
        assert!(matches!(primitives.get(2), Some(MetalPrimitive::PopClipRect)));
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
            pending[0].target,
            EffectTarget::BufferName(ref name) if name == "*light*"
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
    #[ignore = "profiling harness; run explicitly while optimizing compiler/runtime speed"]
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
        assert_eq!(result, Value::Number(2.0), "function should receive exactly 2 args");
    }
}
