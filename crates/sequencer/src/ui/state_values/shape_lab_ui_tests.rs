//! The `*shape-lab*` buffer (content/ui/modal/shape-lab.lisp): a solid loaded
//! from a script renders its profile canvas and 3-D preview, a canvas drag
//! moves the picked point, a finished edit asks for a solve, and Update draft
//! hands the host the shape to write back.

use super::*;

fn eval(editor: &mut Editor, source: &str) -> Value {
    editor.runtime_mut().eval_str(source).unwrap_or_else(|error| panic!("{source}: {error:?}")).unwrap_or(Value::Nil)
}

fn find_by_key_suffix<'a>(
    node: &'a eseqlisp::layout::LayoutNode,
    key: &str,
) -> Option<&'a eseqlisp::layout::LayoutNode> {
    if node.stable_key.as_deref().is_some_and(|k| k.ends_with(key)) {
        return Some(node);
    }
    node.children.iter().find_map(|child| find_by_key_suffix(child, key))
}

fn host_commands(editor: &mut Editor, name: &str) -> Vec<Value> {
    editor
        .drain_host_commands()
        .into_iter()
        .filter_map(|command| match command {
            HostCommand::Custom { name: n, payload } if n == name => Some(payload),
            _ => None,
        })
        .collect()
}

fn field(value: &Value, key: &str) -> Value {
    match value {
        Value::Map(map) => map.get(key).map(|item| item.borrow().clone()).unwrap_or(Value::Nil),
        other => panic!("{key} of {other:?}"),
    }
}

/// What shape_lab.rs `load` sends for a script with one bronze bell.
const LOADED: &str = r#"(eseq.modal.shape-lab/loaded "bell.lisp" "/tmp"
  (list (dict :id "bell" :editable true
              :spec (dict :solid (dict :revolve (dict :profile (list (list 0 0.25 0.012) (list 0.1 0.2 0.01) (list 0.15 0 0.016))
                                                      :segments (dict :along 12 :around 24 :through 1)))
                          :modes 8 :made-of eseq.modal.materials/bronze))))"#;

fn lab_layout(editor: &mut Editor) -> std::sync::Arc<eseqlisp::layout::LayoutNode> {
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("shape lab status after refresh: {status}");
    }
    let id = editor.buffers.iter().find(|b| b.name == "*shape-lab*").expect("shape lab buffer").id;
    editor.set_active_buffer(id);
    editor.set_layout_viewport(140, 40);
    let layout = editor.widget_layout().expect("shape lab layout");
    assert_finite_layout_tree(&layout);
    layout
}

#[test]
fn a_loaded_solid_shows_its_profile_and_preview_and_asks_for_a_solve() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.drain_host_commands();
    eval(&mut editor, LOADED);
    // The script had no solve to show: the lab asks for one.
    let solves = host_commands(&mut editor, "shape-lab-solve");
    assert_eq!(solves.len(), 1, "{solves:?}");
    assert_eq!(field(&solves[0], "dir"), Value::String("/tmp".into()));
    let preview = eval(&mut editor, "eseq.modal.shape-lab/lab-preview");
    assert!(matches!(field(&preview, "mesh-id"), Value::Number(_)), "{preview:?}");
    let layout = lab_layout(&mut editor);
    for key in ["lab-profile", "lab-preview", "lab-apply", "lab-material", "lab-modes", "lab-template", "lab-layers", "lab-detail"] {
        let node = find_by_key_suffix(&layout, key).unwrap_or_else(|| panic!("{key}"));
        assert_finite_nonzero_rect(node, key);
    }
}

/// A press on a point picks it; the drag moves it (snapped to 0.1 mm)
/// without solving; the release solves; Update draft sends the new shape.
#[test]
fn dragging_a_profile_point_moves_it_then_solves_and_applies() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, LOADED);
    editor.drain_host_commands();
    // the pointer (-1..1, y down) over point 2, (0.1, 0.2)
    let at = |editor: &mut Editor, r: f64, z: f64| -> (f64, f64) {
        let view = eval(editor, "eseq.modal.shape-lab/lab-view");
        let n = |key: &str| match field(&view, key) {
            Value::Number(n) => n,
            other => panic!("{key}: {other:?}"),
        };
        ((r - n("cr")) / n("hr"), (n("cz") - z) / n("hz"))
    };
    let (sx, sy) = at(&mut editor, 0.1, 0.2);
    eval(&mut editor, &format!("(eseq.modal.shape-lab/canvas-down {sx} {sy} 0)"));
    assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-point"), Value::Number(1.0));
    let (sx, sy) = at(&mut editor, 0.11237, 0.18);
    eval(&mut editor, &format!("(eseq.modal.shape-lab/canvas-drag {sx} {sy} 0)"));
    assert!(host_commands(&mut editor, "shape-lab-solve").is_empty(), "a drag does not solve");
    let point = eval(&mut editor, "(nth (eseq.modal.shape-lab/profile) 1)");
    assert_eq!(format!("{point:?}"), format!("{:?}", eval(&mut editor, "(list 0.1124 0.18 0.01)")));
    eval(&mut editor, &format!("(eseq.modal.shape-lab/canvas-up {sx} {sy} 0)"));
    assert_eq!(host_commands(&mut editor, "shape-lab-solve").len(), 1);

    eval(&mut editor, "(eseq.modal.shape-lab/set-material! \"brass\")");
    editor.drain_host_commands();
    // Update draft waits for the solve, so the script reads it from the cache
    eval(&mut editor, "(eseq.modal.shape-lab/update-draft)");
    assert!(host_commands(&mut editor, "shape-lab-apply").is_empty());
    eval(&mut editor, "(eseq.modal.shape-lab/solved eseq.modal.shape-lab/lab-ticket (dict :file \"/tmp/x.modes.json\" :freqs (list 440) :mass 2))");
    eval(&mut editor, "(eseq.modal.shape-lab/update-draft)");
    let applies = host_commands(&mut editor, "shape-lab-apply");
    assert_eq!(applies.len(), 1);
    assert_eq!(field(&applies[0], "buffer"), Value::String("bell.lisp".into()));
    assert_eq!(field(&applies[0], "id"), Value::String("bell".into()));
    assert_eq!(field(&applies[0], "made-of"), Value::String("brass".into()));
    assert_eq!(field(&applies[0], "modes"), Value::Number(8.0));
    let profile = field(&field(&field(&applies[0], "shape"), "revolve"), "profile");
    let Value::List(points) = profile else { panic!("profile") };
    assert_eq!(points.len(), 3);
}

/// Only the latest solve's answer is shown: the 3-D view plays its modes.
#[test]
fn a_solve_answer_plays_its_modes_and_a_stale_one_is_ignored() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, LOADED);
    let ticket = eval(&mut editor, "eseq.modal.shape-lab/lab-ticket");
    let Value::Number(ticket) = ticket else { panic!("ticket") };
    let answer = |t: f64| {
        format!("(eseq.modal.shape-lab/solved {t} (dict :file \"/tmp/x.modes.json\" :freqs (list 440 880 1100) :mass 2))")
    };
    eval(&mut editor, &answer(ticket - 1.0));
    assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-solved"), Value::Nil);
    eval(&mut editor, &answer(ticket));
    let layout = lab_layout(&mut editor);
    assert_finite_nonzero_rect(find_by_key_suffix(&layout, "lab-cycle").expect("3-D view"), "lab-cycle");
    assert!(find_by_key_suffix(&layout, "lab-mode-0").is_none(), "no modes list");
}

/// The host's load: a real modal script in a buffer is evaluated, its solid
/// (a coarse bar, solved on evaluation) reaches the lab with the spec it was
/// solved from and its solve, and the lab opens.
#[test]
fn loading_a_modal_script_hands_the_lab_its_solid() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let script = "(import eseq.modal :as m)
(import eseq.modal.materials :as mat)
(m/patch :name \"probe-lab\"
  :objects (list (m/mallet 'mal)
                 (m/solid 'bar :made-of mat/steel :modes 4 :resolution (dict :cells 10)
                          :shape (dict :box (dict :size (list 0.3 0.03 0.01)))))
  :connections (list (m/felt 'hit 'mal 'bar (list 0.15 0 0.005)))
  :listens (list (m/listen 'bar (list -0.14 0 0.005))))";
    editor.open_scratch_buffer("probe-lab.lisp", script);
    let payload = eval(&mut editor, "(dict :buffer \"probe-lab.lisp\")");
    crate::host_commands::shape_lab::load(&mut editor, &payload).unwrap();
    assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-buffer"), Value::String("probe-lab.lisp".into()));
    assert_eq!(eval(&mut editor, "(len eseq.modal.shape-lab/lab-solids)"), Value::Number(1.0));
    assert_eq!(eval(&mut editor, "(get (first eseq.modal.shape-lab/lab-solids) :editable)"), Value::Bool(true));
    assert_eq!(eval(&mut editor, "(get (eseq.modal.shape-lab/part) :size)"), eval(&mut editor, "(list 0.3 0.03 0.01)"));
    assert_eq!(eval(&mut editor, "(get eseq.modal.shape-lab/lab-spec :modes)"), Value::Number(4.0));
    // the script's own solve: no new one asked for
    let solved = eval(&mut editor, "eseq.modal.shape-lab/lab-solved");
    assert!(matches!(field(&solved, "file"), Value::String(_)), "{solved:?}");
    assert!(editor.buffer_named_is_showing("*shape-lab*"));
    let layout = lab_layout(&mut editor);
    for key in ["lab-box-x", "lab-cycle"] {
        let node = find_by_key_suffix(&layout, key).unwrap_or_else(|| panic!("{key}"));
        assert_finite_nonzero_rect(node, key);
    }
}

/// Solves run one at a time on a worker thread: a request made meanwhile
/// waits, replacing an older waiting one, and the answer reaches the lab.
#[test]
fn background_solves_queue_the_latest_and_answer_the_lab() {
    use crate::host_commands::shape_lab::{poll_shape_solves, solve, ShapeSolves};
    let mut editor = full_grid_editor_for_scroll_tests();
    let spec = "(dict :solid (dict :box (dict :size (list 0.2 0.02 0.01))) :modes 3 :resolution (dict :cells 8)
                      :made-of eseq.modal.materials/aluminium)";
    let mut solves = ShapeSolves::default();
    for ticket in [1, 2, 3] {
        let payload = eval(&mut editor, &format!("(dict :ticket {ticket} :dir \"/tmp\" :spec {spec})"));
        solve(&mut solves, &payload).unwrap();
    }
    assert_eq!(solves.tickets(), (Some(1.0), Some(3.0)));
    assert!(solves.busy());
    eval(&mut editor, "(set! eseq.modal.shape-lab/lab-ticket 3)");
    let start = std::time::Instant::now();
    while solves.tickets() != (None, None) {
        assert!(start.elapsed() < std::time::Duration::from_secs(120), "solves did not finish");
        poll_shape_solves(&mut editor, &mut solves);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let solved = eval(&mut editor, "eseq.modal.shape-lab/lab-solved");
    assert!(matches!(field(&solved, "file"), Value::String(_)), "{solved:?}");
    assert_eq!(eval(&mut editor, "(len (get eseq.modal.shape-lab/lab-solved :freqs))"), Value::Number(3.0));
}

/// What shape_lab.rs `load` sends for a script with one steel bar.
const LOADED_BAR: &str = r#"(eseq.modal.shape-lab/loaded "bar.lisp" "/tmp"
  (list (dict :id "bar" :editable true
              :spec (dict :solid (dict :box (dict :size (list 0.3 0.04 0.012)))
                          :modes 8 :made-of eseq.modal.materials/steel))))"#;

/// The pointer (-1..1, y down) over (u, v) in the parts canvas.
fn pointer_at(editor: &mut Editor, u: f64, v: f64) -> (f64, f64) {
    let view = eval(editor, "eseq.modal.shape-lab/lab-view");
    let n = |key: &str| match field(&view, key) {
        Value::Number(n) => n,
        other => panic!("{key}: {other:?}"),
    };
    ((u - n("cr")) / n("hr"), (n("cz") - v) / n("hz"))
}

fn lisp_str(editor: &mut Editor, source: &str) -> String {
    format!("{:?}", eval(editor, source))
}

/// The parts round trip: a lone part stays a primitive, cuts make a
/// difference of the added parts' union, and a disc footprint is hit inside
/// and missed outside.
#[test]
fn shape_parts_read_and_write_back_their_nesting() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let box_ = "(dict :box (dict :size (list 0.3 0.04 0.012)))";
    assert_eq!(lisp_str(&mut editor, &format!("(eseq.modal.shape-parts/shape-of (eseq.modal.shape-parts/parts-of {box_}))")), lisp_str(&mut editor, box_));
    let drilled = "(dict :difference (list (dict :union (list (dict :box (dict :size (list 0.3 0.04 0.012)))
                                                             (dict :sphere (dict :radius 0.02))))
                                           (dict :cylinder (dict :radius 0.004 :height 0.02 :center (list 0.08 0 0)))))";
    assert_eq!(eval(&mut editor, &format!("(len (eseq.modal.shape-parts/parts-of {drilled}))")), Value::Number(3.0));
    assert_eq!(lisp_str(&mut editor, &format!("(eseq.modal.shape-parts/shape-of (eseq.modal.shape-parts/parts-of {drilled}))")), lisp_str(&mut editor, drilled));
    // a cut listed before an added part: back in shape-of's order
    let ps = "(list (dict :shape (dict :sphere (dict :radius 0.01)) :cut true) (dict :shape (dict :sphere (dict :radius 0.05)) :cut false))";
    assert_eq!(eval(&mut editor, &format!("(eseq.modal.shape-parts/reorder {ps} 0)")), Value::Number(1.0));
    let hole = "(dict :cylinder (dict :radius 0.004 :height 0.02 :center (list 0.08 0 0)))";
    assert_eq!(eval(&mut editor, &format!("(eseq.modal.shape-parts/hit? (eseq.modal.shape-parts/footprint {hole} 0 1) 0.081 0.001)")), Value::Bool(true));
    assert_eq!(eval(&mut editor, &format!("(eseq.modal.shape-parts/hit? (eseq.modal.shape-parts/footprint {hole} 0 1) 0.085 0.0)")), Value::Bool(false));
    // seen from the front the hole is a rectangle through the bar's thickness
    assert_eq!(eval(&mut editor, &format!("(eseq.modal.shape-parts/hit? (eseq.modal.shape-parts/footprint {hole} 0 2) 0.0835 0.009)")), Value::Bool(true));
    assert_eq!(eval(&mut editor, &format!("(eseq.modal.shape-parts/hit? (eseq.modal.shape-parts/footprint {hole} 0 2) 0.0835 0.011)")), Value::Bool(false));
}

/// A box opens the parts editor. Drilling drops a hole where the pointer
/// is and drags it (solving on release); a row, an arch, the detail and a
/// delete all edit the parts; Update draft sends the shape and resolution.
#[test]
fn drilling_a_bar_cuts_holes_and_applies_them_with_their_detail() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, LOADED_BAR);
    let layout = lab_layout(&mut editor);
    for key in ["lab-parts", "lab-tool-Hole", "lab-tool-Arch", "lab-box-x", "lab-detail", "lab-view-plane", "lab-layers", "lab-part-repeat"] {
        let node = find_by_key_suffix(&layout, key).unwrap_or_else(|| panic!("{key}"));
        assert_finite_nonzero_rect(node, key);
    }
    assert!(find_by_key_suffix(&layout, "lab-profile").is_none());
    editor.drain_host_commands();

    eval(&mut editor, "(set! eseq.modal.shape-lab/lab-tool :hole)");
    let (sx, sy) = pointer_at(&mut editor, 0.08, 0.0);
    eval(&mut editor, &format!("(eseq.modal.shape-lab/parts-down {sx} {sy} 0)"));
    assert_eq!(eval(&mut editor, "(eseq.modal.shape-parts/kind-of (eseq.modal.shape-lab/shape))"), Value::Keyword("difference".into()));
    assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-part"), Value::Number(1.0));
    assert!(host_commands(&mut editor, "shape-lab-solve").is_empty(), "drilling waits for the release");
    let (sx, sy) = pointer_at(&mut editor, 0.1, 0.005);
    eval(&mut editor, &format!("(eseq.modal.shape-lab/parts-drag {sx} {sy} 0)"));
    eval(&mut editor, &format!("(eseq.modal.shape-lab/parts-up {sx} {sy} 0)"));
    assert_eq!(host_commands(&mut editor, "shape-lab-solve").len(), 1);
    assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-tool"), Value::Keyword("select".into()), "one hole, then Move");
    let center = lisp_str(&mut editor, "(eseq.modal.shape-parts/center-of (get (eseq.modal.shape-lab/selected-part) :shape))");
    assert_eq!(center, lisp_str(&mut editor, "(list 0.1 0.005 0)"));
    // the drilled bar meshes (its :axis keyword reads as the solver's axis)
    let preview = eval(&mut editor, "eseq.modal.shape-lab/lab-preview");
    assert!(matches!(field(&preview, "mesh-id"), Value::Number(_)), "{preview:?}");

    // a click that moves nothing selects without solving
    eval(&mut editor, "(set! eseq.modal.shape-lab/lab-tool :select)");
    let (sx, sy) = pointer_at(&mut editor, -0.1, 0.0);
    eval(&mut editor, &format!("(eseq.modal.shape-lab/parts-down {sx} {sy} 0)"));
    eval(&mut editor, &format!("(eseq.modal.shape-lab/parts-up {sx} {sy} 0)"));
    assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-part"), Value::Number(0.0));
    assert!(host_commands(&mut editor, "shape-lab-solve").is_empty());

    // the hole as a row of three, then an arch under the bar
    eval(&mut editor, "(set! eseq.modal.shape-lab/lab-part 1)");
    eval(&mut editor, "(eseq.modal.shape-lab/make-row 3)");
    assert_eq!(eval(&mut editor, "(len (eseq.modal.shape-lab/layer-parts))"), Value::Number(4.0));
    eval(&mut editor, "(eseq.modal.shape-lab/add-part! (eseq.modal.shape-lab/placed-part :arch 0.02 0) true)");
    let arch = eval(&mut editor, "(get (eseq.modal.shape-lab/selected-part) :shape)");
    assert_eq!(field(&field(&arch, "cylinder"), "axis"), Value::Keyword("y".into()));
    eval(&mut editor, "(eseq.modal.shape-lab/delete-part)");
    assert_eq!(eval(&mut editor, "(len (eseq.modal.shape-lab/layer-parts))"), Value::Number(4.0));
    // the body's only added part cannot be cut or deleted
    eval(&mut editor, "(set! eseq.modal.shape-lab/lab-part 0)");
    eval(&mut editor, "(eseq.modal.shape-lab/toggle-cut)");
    eval(&mut editor, "(eseq.modal.shape-lab/delete-part)");
    assert_eq!(eval(&mut editor, "(get (first (eseq.modal.shape-lab/layer-parts)) :cut)"), Value::Bool(false));

    eval(&mut editor, "(eseq.modal.shape-lab/step-detail 1)");
    assert_eq!(eval(&mut editor, "(eseq.modal.shape-lab/detail)"), Value::Number(48.0));
    // every view draws
    for plane in [":front", ":side", ":top"] {
        eval(&mut editor, &format!("(eseq.modal.shape-lab/set-plane! {plane})"));
        let layout = lab_layout(&mut editor);
        assert_finite_nonzero_rect(find_by_key_suffix(&layout, "lab-parts").expect("parts canvas"), plane);
    }
    editor.drain_host_commands();
    eval(&mut editor, "(eseq.modal.shape-lab/solved eseq.modal.shape-lab/lab-ticket (dict :file \"/tmp/x.modes.json\" :freqs (list 440) :mass 2))");
    eval(&mut editor, "(eseq.modal.shape-lab/update-draft)");
    let applies = host_commands(&mut editor, "shape-lab-apply");
    assert_eq!(applies.len(), 1);
    assert_eq!(field(&field(&applies[0], "resolution"), "cells"), Value::Number(48.0));
    let Value::List(items) = field(&field(&applies[0], "shape"), "difference") else { panic!("difference") };
    assert_eq!(items.len(), 4);
    // the host's progress shows on the button; a failure shows its error
    // and leaves the edit to try again
    eval(&mut editor, "(eseq.modal.shape-lab/draft-status :compiling nil)");
    assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-edited"), Value::Bool(false));
    eval(&mut editor, "(eseq.modal.shape-lab/draft-status :failed \"Draft did not compile: boom\")");
    assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-edited"), Value::Bool(true));
    let layout = lab_layout(&mut editor);
    assert_finite_nonzero_rect(find_by_key_suffix(&layout, "lab-draft-error").expect("error line"), "lab-draft-error");
}

/// A revolve's Holes & parts tab cuts into the revolve itself: the bell
/// stays the body, the hole goes through it.
#[test]
fn a_bell_can_be_drilled_from_its_parts_tab() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, LOADED);
    eval(&mut editor, "(eseq.modal.shape-lab/set-editor! :parts)");
    eval(&mut editor, "(eseq.modal.shape-lab/set-plane! :front)");
    let layout = lab_layout(&mut editor);
    for key in ["lab-parts", "lab-edit-profile", "lab-tool-Hole"] {
        assert_finite_nonzero_rect(find_by_key_suffix(&layout, key).unwrap_or_else(|| panic!("{key}")), key);
    }
    // a hole through the bell's wall, where the front view was clicked
    eval(&mut editor, "(eseq.modal.shape-lab/add-part! (eseq.modal.shape-lab/placed-part :hole 0.125 0.1) true)");
    let shape = eval(&mut editor, "(eseq.modal.shape-lab/shape)");
    let Value::List(items) = field(&shape, "difference") else { panic!("{shape:?}") };
    assert!(matches!(field(&items[0].borrow(), "revolve"), Value::Map(_)));
    // the preview meshes the drilled bell: its fitted mesh (12 along, 24
    // around), carved; finer detail scales those counts until the hole
    // takes elements out
    let preview = eval(&mut editor, "eseq.modal.shape-lab/lab-preview");
    assert!(matches!(field(&preview, "mesh-id"), Value::Number(_)), "{preview:?}");
    assert!(matches!(field(&preview, "elements"), Value::Number(n) if n <= 288.0), "{preview:?}");
    for _ in 0..4 {
        eval(&mut editor, "(eseq.modal.shape-lab/step-detail 1)");
    }
    let segments = eval(&mut editor, "(get (eseq.modal.shape-parts/body (eseq.modal.shape-lab/segmented-revolve)) :segments)");
    let count = |key: &str| match field(&segments, key) {
        Value::Number(n) => n,
        other => panic!("{key}: {other:?}"),
    };
    assert_eq!((count("along"), count("around")), (30.0, 60.0));
    let preview = eval(&mut editor, "eseq.modal.shape-lab/lab-preview");
    let Value::Number(elements) = field(&preview, "elements") else { panic!("{preview:?}") };
    assert!(elements < 30.0 * 60.0 && elements > 0.9 * 30.0 * 60.0, "{elements}");
    eval(&mut editor, "(eseq.modal.shape-lab/set-editor! :profile)");
    let layout = lab_layout(&mut editor);
    assert!(find_by_key_suffix(&layout, "lab-parts").is_some(), "a cut revolve stays in the parts editor");
}

/// A clamp the shape misses (a ride's stand point inside a bar's empty
/// air) is caught by the preview: no doomed solve, a plain message and a
/// fix. Clamped to the shape it solves; dragged, it moves; freed, the
/// script's :block goes; a template starts free.
#[test]
fn a_clamp_that_misses_the_shape_is_explained_and_fixable() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.drain_host_commands();
    eval(
        &mut editor,
        r#"(eseq.modal.shape-lab/loaded "bar.lisp" "/tmp"
  (list (dict :id "bar" :editable true
              :spec (dict :solid (dict :box (dict :size (list 0.3 0.04 0.012)))
                          :block (dict :sphere (dict :center (list 0 0 0.046) :radius 0.008))
                          :modes 8 :made-of eseq.modal.materials/steel))))"#,
    );
    assert_eq!(eval(&mut editor, "(eseq.modal.shape-lab/clamp-misses?)"), Value::Bool(true));
    assert!(host_commands(&mut editor, "shape-lab-solve").is_empty(), "a shape its clamp misses is not solved");
    eval(&mut editor, "(eseq.modal.shape-lab/select-layer! eseq.modal.shape-lab/clamp-layer)");
    let layout = lab_layout(&mut editor);
    for key in ["lab-layers", "lab-clamp-snap", "lab-clamp-free", "lab-clamp-radius"] {
        assert_finite_nonzero_rect(find_by_key_suffix(&layout, key).unwrap_or_else(|| panic!("{key}")), key);
    }
    eval(&mut editor, "(eseq.modal.shape-lab/update-draft)");
    assert!(host_commands(&mut editor, "shape-lab-apply").is_empty(), "nothing solved, nothing applied");

    eval(&mut editor, "(eseq.modal.shape-lab/clamp-to-shape)");
    assert_eq!(eval(&mut editor, "(eseq.modal.shape-lab/clamp-misses?)"), Value::Bool(false));
    assert_eq!(host_commands(&mut editor, "shape-lab-solve").len(), 1);
    let center = eval(&mut editor, "(eseq.modal.shape-parts/region-center (eseq.modal.shape-lab/held-by))");
    assert_eq!(format!("{center:?}"), lisp_str(&mut editor, "(list 0 0 0.006)"), "onto the bar's top face");

    // drag it along the bar in the front view
    eval(&mut editor, "(eseq.modal.shape-lab/set-plane! :front)");
    let (sx, sy) = pointer_at(&mut editor, 0.0, 0.006);
    eval(&mut editor, &format!("(eseq.modal.shape-lab/parts-down {sx} {sy} 0)"));
    assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-drag-clamp"), Value::Bool(true));
    let (sx, sy) = pointer_at(&mut editor, 0.05, 0.006);
    eval(&mut editor, &format!("(eseq.modal.shape-lab/parts-drag {sx} {sy} 0)"));
    eval(&mut editor, &format!("(eseq.modal.shape-lab/parts-up {sx} {sy} 0)"));
    let center = eval(&mut editor, "(eseq.modal.shape-parts/region-center (eseq.modal.shape-lab/held-by))");
    assert_eq!(format!("{center:?}"), lisp_str(&mut editor, "(list 0.05 0 0.006)"));
    assert_eq!(host_commands(&mut editor, "shape-lab-solve").len(), 1);
    assert_eq!(eval(&mut editor, "(len (eseq.modal.shape-lab/layer-parts))"), Value::Number(1.0), "the drag moved no part");

    eval(&mut editor, "(eseq.modal.shape-lab/free-it)");
    eval(&mut editor, "(eseq.modal.shape-lab/solved eseq.modal.shape-lab/lab-ticket (dict :file \"/tmp/x.modes.json\" :freqs (list 440) :mass 2))");
    editor.drain_host_commands();
    eval(&mut editor, "(eseq.modal.shape-lab/update-draft)");
    let applies = host_commands(&mut editor, "shape-lab-apply");
    assert_eq!(field(&applies[0], "block"), Value::Keyword("free".into()));

    eval(&mut editor, "(eseq.modal.shape-lab/clamp-it)");
    assert!(matches!(eval(&mut editor, "(eseq.modal.shape-lab/held-by)"), Value::Map(_)));
    assert_eq!(eval(&mut editor, "(eseq.modal.shape-lab/clamp-misses?)"), Value::Bool(false));
    eval(&mut editor, "(eseq.modal.shape-lab/use-template (nth eseq.modal.shape-lab/templates 1))");
    assert_eq!(eval(&mut editor, "(eseq.modal.shape-lab/held-by)"), Value::Nil, "a template starts free");
}

/// The user's flow end to end, short of the dgen compile: a real script
/// loaded, a hole drilled, Update draft's write-back, then the evaluation
/// C-c C-c does: it must still be a program that generates an instrument.
#[test]
fn an_applied_shape_still_evaluates_to_an_instrument() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let script = "(import eseq.modal :as m)
(import eseq.modal.materials :as mat)
(m/patch :name \"probe-lab-apply\"
  :objects (list (m/mallet 'mal)
                 (m/solid 'bar :made-of mat/steel :modes 4 :resolution (dict :cells 12)
                          :block (dict :sphere (dict :center (list 0 0 0.046) :radius 0.008))
                          :shape (dict :box (dict :size (list 0.3 0.03 0.012)))))
  :connections (list (m/felt 'hit 'mal 'bar (list 0.15 0 0.006)))
  :listens (list (m/listen 'bar (list -0.14 0 0.006))))";
    editor.open_scratch_buffer("probe-lab-apply.lisp", script);
    let payload = eval(&mut editor, "(dict :buffer \"probe-lab-apply.lisp\")");
    crate::host_commands::shape_lab::load(&mut editor, &payload).unwrap();
    eval(&mut editor, "(eseq.modal.shape-lab/free-it)");
    eval(&mut editor, "(eseq.modal.shape-lab/set-plane! :top)");
    eval(&mut editor, "(eseq.modal.shape-lab/add-part! (eseq.modal.shape-lab/placed-part :hole 0.06 0) true)");
    eval(&mut editor, "(eseq.modal.shape-lab/solved eseq.modal.shape-lab/lab-ticket (dict :file \"/tmp/x.modes.json\" :freqs (list 440) :mass 2))");
    editor.drain_host_commands();
    eval(&mut editor, "(eseq.modal.shape-lab/update-draft)");
    let applies = host_commands(&mut editor, "shape-lab-apply");
    assert_eq!(applies.len(), 1);
    let name = crate::host_commands::shape_lab::write_back(&mut editor, &applies[0]).unwrap();
    let text = editor.buffers.iter().find(|b| b.name == name).unwrap().text();
    assert!(text.contains(":difference") && !text.contains(":block"), "{text}");
    let value = sequencer::lisp_host::eval_script(editor.runtime_mut(), std::path::PathBuf::from("/tmp/probe-lab-apply.lisp"), &text)
        .unwrap_or_else(|errors| panic!("{errors:?}\n{text}"))
        .unwrap_or(Value::Nil);
    assert!(sequencer::lisp_host::is_program(&value), "{value:?}");
    sequencer::lisp_host::generate_from_program(editor.runtime_mut(), std::path::PathBuf::from("/tmp/probe-lab-apply.lisp"), value)
        .unwrap_or_else(|errors| panic!("{errors:?}\n{text}"));
    // the script now builds exactly the spec the lab solved, so its
    // evaluation reads that solve from the cache instead of solving again
    let solved_spec = eval(&mut editor, "eseq.modal.shape-lab/lab-spec");
    crate::host_commands::shape_lab::load(&mut editor, &payload).unwrap();
    let script_spec = eval(&mut editor, "(get (first eseq.modal.shape-lab/lab-solids) :spec)");
    let hash = |spec: &Value| {
        let (spec, _) = sequencer::lisp_host::solid_spec(spec, std::path::Path::new("/tmp")).unwrap();
        sequencer::modal_fe::spec_hash(&spec)
    };
    assert_eq!(hash(&solved_spec), hash(&script_spec), "{solved_spec:?}\n{script_spec:?}");
}

/// Turning a bell's hole into an added part makes the body a union, which
/// is voxelized: the lab says the bell's wall is thinner than the cubes,
/// and Mesh shows the cube size, not the bell's own element size.
#[test]
fn adding_a_part_to_a_thin_revolve_warns_that_its_wall_will_vanish() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, LOADED);
    eval(&mut editor, "(eseq.modal.shape-lab/set-editor! :parts)");
    eval(&mut editor, "(eseq.modal.shape-lab/add-part! (eseq.modal.shape-lab/placed-part :hole 0.05 0.05) true)");
    assert_eq!(eval(&mut editor, "(eseq.modal.shape-lab/voxel-body?)"), Value::Bool(false), "a cut keeps the fitted mesh");
    eval(&mut editor, "(eseq.modal.shape-lab/toggle-cut)");
    assert_eq!(eval(&mut editor, "(eseq.modal.shape-lab/voxel-body?)"), Value::Bool(true));
    let warning = lisp_str(&mut editor, "(eseq.modal.shape-lab/detail-warning)");
    assert!(warning.contains("wall is thinner"), "{warning}");
    // 32 voxel cells across the body's longest side, not the bell's 24 around
    let Value::Number(cell) = eval(&mut editor, "(eseq.modal.shape-lab/cell-size)") else { panic!() };
    let Value::Number(longest) = eval(&mut editor, "(eseq.modal.shape-lab/body-extent)") else { panic!() };
    assert!((cell - longest / 32.0).abs() < 1e-9, "{cell} vs {longest}");
}

/// An added cylinder's Radius and Depth fields edit it through their real
/// callbacks (the value they show is millimetres).
#[test]
fn an_added_cylinders_size_fields_edit_it() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, LOADED_BAR);
    eval(&mut editor, "(eseq.modal.shape-lab/add-part! (eseq.modal.shape-lab/placed-part :hole 0.05 0) true)");
    eval(&mut editor, "(eseq.modal.shape-lab/toggle-cut)");
    let index = eval(&mut editor, "eseq.modal.shape-lab/lab-part");
    assert_eq!(eval(&mut editor, "(get (eseq.modal.shape-lab/selected-part) :cut)"), Value::Bool(false));
    for (key, field_name, mm) in [("lab-field-Radius", "radius", 7.5), ("lab-field-Depth", "height", 30.0)] {
        let layout = lab_layout(&mut editor);
        let picker = find_by_key_suffix(&layout, key).unwrap_or_else(|| panic!("{key}"));
        assert_finite_nonzero_rect(picker, key);
        let on_change = picker.props.get("on-change").expect("callback").clone();
        editor.runtime_mut().invoke(on_change, vec![Value::Number(mm)]).unwrap();
        assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-part"), index, "still selected");
        let shape = eval(&mut editor, "(get (eseq.modal.shape-lab/selected-part) :shape)");
        assert_eq!(field(&field(&shape, "cylinder"), field_name), Value::Number(mm / 1000.0), "{key}");
    }
    let layout = lab_layout(&mut editor);
    let on_change = find_by_key_suffix(&layout, "lab-center-1").expect("Y field").props.get("on-change").unwrap().clone();
    editor.runtime_mut().invoke(on_change, vec![Value::Number(12.0)]).unwrap();
    let center = eval(&mut editor, "(eseq.modal.shape-parts/center-of (get (eseq.modal.shape-lab/selected-part) :shape))");
    assert_eq!(format!("{center:?}"), lisp_str(&mut editor, "(list 0.05 0.012 0)"));
}

/// A solve shows a progress bar with Cancel; cancelling leaves the shape
/// unsolved with a Solve button, ignores the cancelled solve's progress,
/// and Solve asks again.
#[test]
fn a_running_solve_shows_progress_and_can_be_cancelled() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, LOADED_BAR);
    let ticket = eval(&mut editor, "eseq.modal.shape-lab/lab-ticket");
    let Value::Number(ticket) = ticket else { panic!("ticket") };
    eval(&mut editor, &format!("(eseq.modal.shape-lab/solve-progress {ticket} \"factoring\" 0.4)"));
    let layout = lab_layout(&mut editor);
    for key in ["lab-solve-bar", "lab-solve-phase", "lab-solve-cancel"] {
        assert_finite_nonzero_rect(find_by_key_suffix(&layout, key).unwrap_or_else(|| panic!("{key}")), key);
    }
    editor.drain_host_commands();
    eval(&mut editor, "(eseq.modal.shape-lab/cancel-solve)");
    assert_eq!(host_commands(&mut editor, "shape-lab-cancel").len(), 1);
    // the cancelled solve's late progress is ignored
    eval(&mut editor, &format!("(eseq.modal.shape-lab/solve-progress {ticket} \"finding modes\" 0.7)"));
    assert_eq!(eval(&mut editor, "eseq.modal.shape-lab/lab-solve-progress"), Value::Nil);
    let layout = lab_layout(&mut editor);
    assert_finite_nonzero_rect(find_by_key_suffix(&layout, "lab-solve-again").expect("Solve"), "lab-solve-again");
    assert!(find_by_key_suffix(&layout, "lab-solve-bar").is_none());
    eval(&mut editor, "(eseq.modal.shape-lab/request-solve)");
    assert_eq!(host_commands(&mut editor, "shape-lab-solve").len(), 1);
}
