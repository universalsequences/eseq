use super::super::WidgetDefinition;
use super::super::WidgetKeyEvent;
use super::super::text_input::{TextInputState, cache_char_widths};
use super::display::*;
use super::emit::{emit_patch_debug_lisp, emit_patch_debug_lisp_for_view};
use super::geometry::*;
use super::interaction::*;
use super::metrics::*;
use super::model::{CableEndpoint, InputPortRef, OutputPortRef};
use super::project::dgenlisp_operator_names;
use super::render::*;
use super::state::*;
use super::writeback::{WriteBackError, emit_patch_writeback};
use super::*;
use crate::editor::{Editor, EditorConfig};
use crate::layout::{LayoutNode, MeasureCtx, Rect, TextMeasurer};
use crate::runtime::Runtime;
use crate::theme;
use crate::vm::Value;
use crossterm::event::{KeyCode, KeyModifiers};
use std::collections::HashMap;
use std::fs;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn parse(source: &str) -> Patch {
    parse_patch_source(source, PatcherIntent::Instrument).unwrap()
}

#[test]
fn named_constant_def_projects_with_binding_id() {
    let patch = parse("(def value1 0.3)\n(def phase (phasor value1))");

    let constant = patch
        .nodes
        .iter()
        .find(|node| node.op == "0.3")
        .expect("constant node");
    assert_eq!(constant.id, "value1");
    assert!(patch.nodes.iter().all(|node| node.id != "0.3"));
    assert!(
        patch
            .connections
            .iter()
            .any(|connection| connection.from_node == "value1" && connection.to_node == "phase")
    );
}

fn source_expr(scope: SourceScopeId, form_index: usize, path: &[usize]) -> SourceExprId {
    SourceExprId {
        form_id: SourceFormId {
            scope,
            index: form_index,
        },
        path: ExprPath(
            path.iter()
                .copied()
                .map(ExprPathSegment::ListItem)
                .collect(),
        ),
    }
}

#[cfg(target_os = "macos")]
struct FixedWidthTextMeasurer;

#[cfg(target_os = "macos")]
impl TextMeasurer for FixedWidthTextMeasurer {
    fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
        text.chars()
            .map(|ch| if ch.is_whitespace() { 5.0 } else { 10.0 })
            .sum()
    }

    fn line_height_px(&self, _font_size: f32) -> f32 {
        20.0
    }
}

struct VariableWidthTextMeasurer;

impl TextMeasurer for VariableWidthTextMeasurer {
    fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
        text.chars()
            .map(|ch| match ch {
                'i' | 'l' | ' ' => 4.0,
                'W' | 'm' => 18.0,
                _ => 10.0,
            })
            .sum()
    }

    fn line_height_px(&self, _font_size: f32) -> f32 {
        20.0
    }
}

fn node_expr(node: &PatchNode) -> SourceExprId {
    node.source
        .as_ref()
        .and_then(|source| source.expr.clone())
        .expect("node source expr")
}

fn temp_patcher_source_path(name: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("{name}-{nonce}.lisp"))
}

fn temp_patcher_dsp_path(name: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{name}-{nonce}"));
    fs::create_dir_all(&dir).unwrap();
    dir.join("dsp.lisp")
}

fn patcher_props_for_path(path: &std::path::Path) -> HashMap<String, Value> {
    HashMap::from([
        (
            "path".to_string(),
            Value::String(path.to_string_lossy().into()),
        ),
        (
            "intent".to_string(),
            Value::Keyword("instrument".to_string()),
        ),
    ])
}

#[test]
fn missing_layout_sidecar_is_materialized_on_first_load() {
    let path = temp_patcher_dsp_path("patcher-sidecar-materialize");
    fs::write(
        &path,
        "(def pitch (in 1 @name pitch))\n(def phase (phasor pitch))\n(out phase 1 @name audio)",
    )
    .unwrap();

    let (_path, patch) = load_patch_from_props(&patcher_props_for_path(&path)).unwrap();
    let sidecar_path = sidecar::sidecar_path_for_source(&path);
    let sidecar = fs::read_to_string(&sidecar_path).expect("sidecar should be written");
    let json: serde_json::Value = serde_json::from_str(&sidecar).unwrap();

    assert_eq!(json["version"], 1);
    assert!(
        json["root"]["nodes"]
            .as_object()
            .expect("root nodes")
            .contains_key("phase")
    );
    assert!(patch.nodes.iter().any(|node| node.id == "phase"));
}

#[test]
fn existing_layout_sidecar_preserves_positions_and_places_only_new_nodes() {
    let path = temp_patcher_dsp_path("patcher-sidecar-preserve");
    fs::write(
        &path,
        "(def pitch (in 1 @name pitch))\n(def phase (phasor pitch))\n(out phase 1 @name audio)",
    )
    .unwrap();
    load_patch_from_props(&patcher_props_for_path(&path)).unwrap();
    let sidecar_path = sidecar::sidecar_path_for_source(&path);
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    json["root"]["nodes"]["phase"] = serde_json::json!({ "x": 123.0, "y": 45.0 });
    fs::write(&sidecar_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    fs::write(
        &path,
        "(def pitch (in 1 @name pitch))\n(def phase (phasor pitch))\n(def shaped (* phase 0.5))\n(out shaped 1 @name audio)",
    )
    .unwrap();
    let (_path, patch) = load_patch_from_props(&patcher_props_for_path(&path)).unwrap();

    let phase = patch.nodes.iter().find(|node| node.id == "phase").unwrap();
    let shaped = patch.nodes.iter().find(|node| node.id == "shaped").unwrap();
    assert_eq!(phase.position, (123.0, 45.0));
    assert_ne!(
        shaped.position,
        (0.0, 0.0),
        "new node should keep an auto/constrained placement"
    );
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    assert!(
        saved["root"]["nodes"]
            .as_object()
            .unwrap()
            .contains_key("shaped")
    );
}

#[test]
fn persisted_widget_layout_reloads_exact_saved_positions_after_restart() {
    let path = temp_patcher_dsp_path("patcher-sidecar-widget-reload");
    fs::write(
        &path,
        "(defmacro shape (sig) (def folded (* sig 0.5)) folded)\n\
         (def pitch (in 1 @name pitch))\n\
         (def phase (phasor pitch))\n\
         (def shaped (shape phase))\n\
         (out shaped 1 @name audio)",
    )
    .unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let (_path, root_patch) = load_patch_from_props(&node.props).unwrap();
    let phase = root_patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "phase")
        .unwrap();
    let shaped = root_patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "shaped")
        .unwrap();
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "shape")
        .unwrap();
    let folded = macro_patch
        .patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "folded")
        .unwrap();

    let mut state = PatcherInteractionState::default();
    set_node_edit_position(
        &mut state,
        "root",
        phase,
        (41.25, 7.5),
        node_display_label(phase),
    );
    set_node_edit_position(
        &mut state,
        "root",
        shaped,
        (103.75, 29.25),
        node_display_label(shaped),
    );
    set_node_edit_position(
        &mut state,
        "macro:shape",
        folded,
        (14.5, 88.0),
        node_display_label(folded),
    );
    set_patcher_interaction_state(key, state.clone());

    persist_patcher_layout(&node, &state).expect("layout should save");
    set_patcher_interaction_state(key, PatcherInteractionState::default());

    let (_path, reloaded) = load_patch_from_props(&node.props).unwrap();
    assert_eq!(
        reloaded
            .nodes
            .iter()
            .find(|patch_node| patch_node.id == "phase")
            .unwrap()
            .position,
        (41.25, 7.5)
    );
    assert_eq!(
        reloaded
            .nodes
            .iter()
            .find(|patch_node| patch_node.id == "shaped")
            .unwrap()
            .position,
        (103.75, 29.25)
    );
    let reloaded_macro = reloaded
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "shape")
        .unwrap();
    assert_eq!(
        reloaded_macro
            .patch
            .nodes
            .iter()
            .find(|patch_node| patch_node.id == "folded")
            .unwrap()
            .position,
        (14.5, 88.0)
    );
}

#[test]
fn macro_layout_sidecar_is_scoped_by_macro_name() {
    let path = temp_patcher_dsp_path("patcher-sidecar-macro");
    fs::write(
        &path,
        "(defmacro op (sig) (def shaped (* sig 0.5)) shaped)\n(def pitch (in 1 @name pitch))\n(def phase (phasor pitch))\n(def out1 (op phase))\n(out out1 1 @name audio)",
    )
    .unwrap();
    load_patch_from_props(&patcher_props_for_path(&path)).unwrap();
    let sidecar_path = sidecar::sidecar_path_for_source(&path);
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    json["macros"]["op"]["nodes"]["shaped"] = serde_json::json!({ "x": 77.0, "y": 33.0 });
    fs::write(&sidecar_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    let (_path, patch) = load_patch_from_props(&patcher_props_for_path(&path)).unwrap();
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "op")
        .unwrap();
    let shaped = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "shaped")
        .unwrap();
    assert_eq!(shaped.position, (77.0, 33.0));
}

#[test]
fn stale_and_malformed_sidecar_entries_do_not_change_semantics() {
    let path = temp_patcher_dsp_path("patcher-sidecar-stale");
    fs::write(
        &path,
        "(def pitch (in 1 @name pitch))\n(def phase (phasor pitch))\n(out phase 1 @name audio)",
    )
    .unwrap();
    let sidecar_path = sidecar::sidecar_path_for_source(&path);
    fs::write(
        &sidecar_path,
        r#"{
  "version": 1,
  "root": {
    "nodes": {
      "phase": { "x": 88.0, "y": 22.0 },
      "missing": { "x": 1.0, "y": 2.0 }
    },
    "cables": {
      "missing:0->phase:0": { "segmented": true, "y": 5.0 }
    }
  }
}"#,
    )
    .unwrap();
    let (_path, patch) = load_patch_from_props(&patcher_props_for_path(&path)).unwrap();
    assert_eq!(
        patch
            .nodes
            .iter()
            .find(|node| node.id == "phase")
            .unwrap()
            .position,
        (88.0, 22.0)
    );
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    assert!(
        !saved["root"]["nodes"]
            .as_object()
            .unwrap()
            .contains_key("missing")
    );
    fs::write(&sidecar_path, "{ not json").unwrap();
    let (_path, reparsed) = load_patch_from_props(&patcher_props_for_path(&path)).unwrap();
    assert!(reparsed.nodes.iter().any(|node| node.id == "phase"));
    assert!(
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&sidecar_path).unwrap())
            .is_ok(),
        "malformed sidecar should be replaced with a valid materialized layout"
    );
}

fn compile_patch_source_with_dgenlisp(source: &str) -> Result<(), String> {
    let source_path = temp_patcher_source_path("patcher-dgen-compile");
    let out_dir = std::env::temp_dir().join("patcher-dgen-compile-out");
    fs::create_dir_all(&out_dir).map_err(|error| error.to_string())?;
    fs::write(&source_path, source).map_err(|error| error.to_string())?;

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("eseqlisp crate should live below the repository root");
    let tool_path = repo_root.join("crates/sequencer/tools/DGenLisp");
    let output = std::process::Command::new(tool_path)
        .args(["compile", source_path.to_str().unwrap()])
        .args(["-o", out_dir.to_str().unwrap()])
        .args(["--name", "patcher_dgen_compile_test"])
        .args(["--sample-rate", "44100"])
        .args(["--voices", "12"])
        .output()
        .map_err(|error| error.to_string())?;

    let _ = fs::remove_file(source_path);
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        ))
    }
}

fn allocate_created_text_node(
    state: &mut PatcherInteractionState,
    view_key: &str,
    text: &str,
) -> String {
    let node_id = allocate_created_node(state, view_key, (0.0, 0.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key(view_key, &node_id))
        .unwrap()
        .text = text.to_string();
    node_id
}

fn connect_output_to_input(
    state: &mut PatcherInteractionState,
    view_key: &str,
    from_node: &str,
    to_node: &str,
    to_input: usize,
) {
    allocate_created_connection(
        state,
        view_key,
        OutputPortRef {
            node_id: from_node.to_string(),
            output_index: 0,
        },
        InputPortRef {
            node_id: to_node.to_string(),
            input_index: to_input,
        },
    );
}

fn patcher_test_node(path: &std::path::Path) -> LayoutNode {
    LayoutNode {
        widget_id: 1,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 100.0,
        },
        props: HashMap::from([
            (
                "path".to_string(),
                Value::String(path.display().to_string()),
            ),
            (
                "intent".to_string(),
                Value::Keyword("instrument".to_string()),
            ),
            (
                "on-change".to_string(),
                Value::Symbol("callback".to_string()),
            ),
        ]),
        children: Vec::new(),
        focusable: true,
    }
}

#[test]
fn node_size_uses_cached_proportional_character_widths() {
    let label = "Wii".to_string();
    let measurer = VariableWidthTextMeasurer;
    let measure_ctx = MeasureCtx {
        text_measurer: Some(&measurer),
        cell_w: 10.0,
        cell_h: 20.0,
        inherited_font_size: NODE_FONT_SIZE,
    };
    cache_char_widths(label.clone(), NODE_FONT_SIZE, &measure_ctx);

    let node = PatchNode {
        id: "wide-narrow".to_string(),
        op: label,
        kind: NodeKind::Builtin,
        label: String::new(),
        args: Vec::new(),
        outputs: Vec::new(),
        position: (0.0, 0.0),
        diagnostic: None,
        source: None,
    };

    let (width, height) = node_size(&node);

    assert_eq!(height, NODE_HEIGHT);
    assert_eq!(
        width, 5.8,
        "measured `Wii` should be 2.6 cells plus node padding, not 3 fixed-width characters"
    );
}

#[test]
fn agentic_bubble_cmd_k_creates_ephemeral_prompt_without_source_write() {
    let path = temp_patcher_source_path("agentic-bubble-spawn");
    fs::write(&path, "(out 0)").expect("write source");
    let node = patcher_test_node(&path);
    let before = fs::read_to_string(&path).expect("read source");

    let event = PATCHER_WIDGET.key_event(
        &node,
        WidgetKeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::SUPER,
        },
    );

    assert!(event.is_some(), "cmd+k should be consumed");
    assert_eq!(fs::read_to_string(&path).expect("read source"), before);
    let state = get_patcher_interaction_state(patcher_state_key(&node));
    assert_eq!(state.agentic_bubbles.len(), 1);
    assert!(editing_agentic_bubble_id(&state).is_some());
}

#[test]
fn agentic_bubble_cmd_k_uses_last_pointer_model_position() {
    let path = temp_patcher_source_path("agentic-bubble-pointer-position");
    fs::write(&path, "(out 0)").expect("write source");
    let node = patcher_test_node(&path);
    handle_patcher_pointer_moved(&node, 23.0, 31.0, 1.0, 1.0);

    PATCHER_WIDGET.key_event(
        &node,
        WidgetKeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::SUPER,
        },
    );

    let state = get_patcher_interaction_state(patcher_state_key(&node));
    let bubble = state
        .agentic_bubbles
        .values()
        .next()
        .expect("agentic bubble");
    let pan = get_patcher_pan_state(patcher_state_key(&node));
    let expected = screen_to_model(node.rect, &pan, (23.0, 31.0));
    assert!((bubble.position.0 - expected.0).abs() < 0.001);
    assert!((bubble.position.1 - expected.1).abs() < 0.001);
}

#[test]
fn agentic_bubble_enter_emits_submit_payload_and_pending_state() {
    let path = temp_patcher_source_path("agentic-bubble-submit");
    fs::write(&path, "(out 0)").expect("write source");
    let node = patcher_test_node(&path);
    PATCHER_WIDGET.key_event(
        &node,
        WidgetKeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::SUPER,
        },
    );
    let key = patcher_state_key(&node);
    let mut state = get_patcher_interaction_state(key);
    let bubble_id = editing_agentic_bubble_id(&state).expect("editing bubble");
    state
        .agentic_bubbles
        .get_mut(&bubble_id)
        .expect("bubble")
        .prompt = "warm folded sine".to_string();
    set_patcher_interaction_state(key, state);

    let event = PATCHER_WIDGET
        .key_event(
            &node,
            WidgetKeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
            },
        )
        .expect("submit event");
    let output = PATCHER_WIDGET
        .handle_event(&node, event)
        .expect("on-change output");
    assert_eq!(output.args.len(), 1);
    let Value::Map(map) = &output.args[0] else {
        panic!("submit payload should be a map");
    };
    assert!(matches!(
        &*map.get("status").expect("status").borrow(),
        Value::Keyword(status) if status == "agentic-submit"
    ));
    assert!(matches!(
        &*map.get("prompt").expect("prompt").borrow(),
        Value::String(prompt) if prompt == "warm folded sine"
    ));
    let state = get_patcher_interaction_state(key);
    let bubble = state.agentic_bubbles.get(&bubble_id).expect("bubble");
    assert!(matches!(bubble.state, AgenticBubbleState::Pending { .. }));
}

#[test]
fn pending_agentic_bubble_requests_animation_frames() {
    let path = temp_patcher_source_path("agentic-bubble-animation");
    fs::write(&path, "(out 0)").expect("write source");
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    assert!(!PATCHER_WIDGET.wants_animation_frames(&node));

    let mut state = PatcherInteractionState::default();
    allocate_agentic_bubble(&mut state, (2.0, 3.0));
    let bubble = state.agentic_bubbles.values_mut().next().expect("bubble");
    bubble.state = AgenticBubbleState::Pending {
        started_at: Instant::now(),
    };
    set_patcher_interaction_state(key, state);

    assert!(PATCHER_WIDGET.wants_animation_frames(&node));

    let mut state = get_patcher_interaction_state(key);
    let bubble = state.agentic_bubbles.values_mut().next().expect("bubble");
    bubble.state = AgenticBubbleState::Error {
        summary: "failed".to_string(),
        raw_output: String::new(),
        failed_at: Instant::now(),
    };
    set_patcher_interaction_state(key, state);

    assert!(!PATCHER_WIDGET.wants_animation_frames(&node));
}

#[test]
fn visible_patcher_pending_bubble_marks_editor_animating() {
    let path = temp_patcher_source_path("visible-agentic-bubble-animation");
    fs::write(&path, "(out 0)").expect("write source");
    let escaped_path = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(80, 30);
    editor
        .runtime_mut()
        .eval_str(&format!(
            r#"(effect-buffer "*patcher-test*"
  (patcher :intent :instrument :width :fill :height :fill :path "{}"))"#,
            escaped_path
        ))
        .unwrap();
    editor.refresh_runtime_side_effects();
    let patcher_buffer_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*patcher-test*")
        .expect("patcher buffer")
        .id;
    editor.set_active_buffer(patcher_buffer_id);
    editor.update_tile_rects(80, 30);
    editor.sync_layout_to_active_leaf();

    let layout = editor.widget_layout().expect("patcher layout");
    assert_eq!(layout.widget_type, "patcher");
    assert!(!editor.visible_widgets_animating());

    let key = patcher_state_key(&layout);
    let mut state = PatcherInteractionState::default();
    allocate_agentic_bubble(&mut state, (2.0, 3.0));
    state
        .agentic_bubbles
        .values_mut()
        .next()
        .expect("bubble")
        .state = AgenticBubbleState::Pending {
        started_at: Instant::now(),
    };
    set_patcher_interaction_state(key, state);

    assert!(editor.visible_widgets_animating());
}

#[test]
fn agentic_bubble_resolve_writes_macro_and_keeps_instance_edit_ephemeral() {
    let path = temp_patcher_source_path("agentic-bubble-resolve");
    fs::write(&path, "(out 0)").expect("write source");
    let node = patcher_test_node(&path);
    PATCHER_WIDGET.key_event(
        &node,
        WidgetKeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::SUPER,
        },
    );
    let key = patcher_state_key(&node);
    let mut state = get_patcher_interaction_state(key);
    let bubble_id = editing_agentic_bubble_id(&state).expect("editing bubble");
    let bubble = state.agentic_bubbles.get_mut(&bubble_id).expect("bubble");
    bubble.prompt = "gain".to_string();
    bubble.generation = 1;
    bubble.state = AgenticBubbleState::Pending {
        started_at: std::time::Instant::now(),
    };
    set_patcher_interaction_state(key, state);

    resolve_agentic_bubble(
        &path,
        PatcherIntent::Instrument,
        &bubble_id,
        1,
        "agentic-gain",
        "(defmacro agentic-gain (x amount) (* x amount))",
    )
    .expect("resolve bubble");

    let source = fs::read_to_string(&path).expect("read source");
    assert!(source.contains("(defmacro agentic-gain"));
    let state = get_patcher_interaction_state(key);
    assert!(!state.agentic_bubbles.contains_key(&bubble_id));
    assert!(
        state
            .edit_state
            .nodes
            .values()
            .any(|edit| edit.text == "agentic-gain")
    );
}

#[test]
fn agentic_bubble_resolve_ignores_unrelated_invalid_created_nodes() {
    let path = temp_patcher_source_path("agentic-bubble-resolve-isolated");
    fs::write(&path, "(out 0)").expect("write source");
    let node = patcher_test_node(&path);
    PATCHER_WIDGET.key_event(
        &node,
        WidgetKeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::SUPER,
        },
    );
    let key = patcher_state_key(&node);
    let mut state = get_patcher_interaction_state(key);
    let bubble_id = editing_agentic_bubble_id(&state).expect("editing bubble");
    let bubble = state.agentic_bubbles.get_mut(&bubble_id).expect("bubble");
    bubble.prompt = "modal kick".to_string();
    bubble.generation = 1;
    bubble.state = AgenticBubbleState::Pending {
        started_at: std::time::Instant::now(),
    };
    let unrelated_id = allocate_created_node(&mut state, "root", (3.0, 3.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &unrelated_id))
        .expect("unrelated edit")
        .text = "agentic".to_string();
    set_patcher_interaction_state(key, state);

    resolve_agentic_bubble(
        &path,
        PatcherIntent::Instrument,
        &bubble_id,
        1,
        "agenticcreate-a-mini-modal-synthesis-generator-forkick-sdrums",
        "(defmacro agenticcreate-a-mini-modal-synthesis-generator-forkick-sdrums (excitation freq q tightness) (def m1 (svf excitation freq q 1)) m1)",
    )
    .expect("resolve bubble should not validate unrelated created node");

    let source = fs::read_to_string(&path).expect("read source");
    assert!(
        source.contains("(defmacro agenticcreate-a-mini-modal-synthesis-generator-forkick-sdrums")
    );
    let state = get_patcher_interaction_state(key);
    assert!(!state.agentic_bubbles.contains_key(&bubble_id));
    assert!(
        state
            .edit_state
            .nodes
            .values()
            .any(|edit| edit.id == unrelated_id && edit.text == "agentic"),
        "unrelated edit should remain live but should not block bubble materialization"
    );
}

#[test]
fn node_rects_leave_room_for_multiple_input_ports() {
    let patch = parse(
        r#"
            (def a (in 1 @name a))
            (def b (in 2 @name b))
            (def c (in 3 @name c))
            (def d (in 4 @name d))
            (def sig (svf a b c d))
            "#,
    );
    let rects = patch_node_rects(
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        &PatcherPanState::default(),
    );
    let sig = rects.get("sig").expect("svf rect");
    let zoom = DEFAULT_ZOOM;
    let expected_width =
        (PORT_EDGE_PADDING_CELLS * 2.0 + PORT_MIN_CENTER_SPACING_CELLS * 3.0) * zoom;

    assert_eq!(sig.width, expected_width);
}

#[test]
fn multi_input_port_centers_keep_minimum_spacing() {
    let patch = parse(
        r#"
            (def a (in 1 @name a))
            (def b (in 2 @name b))
            (def c (in 3 @name c))
            (def d (in 4 @name d))
            (def sig (svf a b c d))
            "#,
    );
    let rects = patch_node_rects(
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        &PatcherPanState::default(),
    );
    let sig = *rects.get("sig").expect("svf rect");
    let input_indices = patch_input_indices(&patch);
    let slot_count = patch_input_slot_counts(&patch, &input_indices)
        .get("sig")
        .copied()
        .expect("svf input slots");
    let centers: Vec<f32> = (0..slot_count)
        .map(|idx| port_center(sig, idx, slot_count, true).0)
        .collect();
    let min_spacing = PORT_MIN_CENTER_SPACING_CELLS * DEFAULT_ZOOM;

    for pair in centers.windows(2) {
        assert!(
            pair[1] - pair[0] >= min_spacing - 0.0001,
            "adjacent input ports should be separated by at least {min_spacing}, got {centers:?}"
        );
    }
}

#[test]
fn patcher_change_payload_emits_valid_writeback_source() {
    let path = temp_patcher_source_path("patcher-change-valid");
    fs::write(&path, "(def sig (phasor 440))\n(out sig 1 @name audio)\n").unwrap();
    let node = patcher_test_node(&path);

    let payload = patcher_writeback_payload(&node);
    let Value::Map(map) = payload else {
        panic!("expected payload map");
    };

    assert_eq!(
        map.get("status").map(|value| value.borrow().clone()),
        Some(Value::Keyword("valid".to_string()))
    );
    assert!(
        matches!(map.get("source").map(|value| value.borrow().clone()), Some(Value::String(source)) if source.contains("out sig"))
    );
    let _ = fs::remove_file(path);
}

#[test]
fn patcher_change_payload_does_not_install_emitted_layout_before_source_is_saved() {
    let path = temp_patcher_dsp_path("patcher-unsaved-layout-preview");
    fs::write(&path, "(def sig (phasor 440))\n(out sig 1 @name audio)\n").unwrap();
    let node = patcher_test_node(&path);
    let (_path, root_patch) = load_patch_from_props(&node.props).unwrap();
    let output = root_patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.kind == NodeKind::Out)
        .unwrap();
    let incoming = root_patch
        .connections
        .iter()
        .find(|connection| connection.to_node == output.id && connection.to_input == 0)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "root", (222.0, 33.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created))
        .unwrap()
        .text = "* 0.5".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key("root", &source_connection_id(incoming)));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: incoming.from_node.clone(),
            output_index: incoming.from_output,
        },
        InputPortRef {
            node_id: created.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: output.id.clone(),
            input_index: 0,
        },
    );
    set_patcher_interaction_state(patcher_state_key(&node), state);
    let sidecar_path = sidecar::sidecar_path_for_source(&path);
    let original_sidecar: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();

    let payload = patcher_writeback_payload(&node);
    let Value::Map(map) = payload else {
        panic!("expected payload map");
    };
    let layout = match map.get("layout").map(|value| value.borrow().clone()) {
        Some(Value::String(layout)) => layout,
        other => panic!("expected emitted layout string, got {other:?}"),
    };
    assert!(
        !layout.contains(&created),
        "emitted layout should be reconciled to emitted source ids"
    );

    let installed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    assert_eq!(
        installed, original_sidecar,
        "semantic preview payload must not persist sidecar before Save/Finalize"
    );
    fs::write(
        &path,
        match map.get("source").map(|value| value.borrow().clone()) {
            Some(Value::String(source)) => source,
            other => panic!("expected emitted source string, got {other:?}"),
        },
    )
    .unwrap();
    fs::write(&sidecar_path, layout).unwrap();
    set_patcher_interaction_state(patcher_state_key(&node), PatcherInteractionState::default());
    let reloaded = load_patch_from_props(&node.props).unwrap().1;
    assert!(
        reloaded
            .nodes
            .iter()
            .any(|patch_node| patch_node.id == "mul1"),
        "saved source and sidecar should reload with emitted ids"
    );
}

#[test]
fn semantic_save_payload_layout_reloads_saved_widget_positions_after_restart() {
    let path = temp_patcher_dsp_path("patcher-sidecar-semantic-save-reload");
    fs::write(
        &path,
        "(def pitch (in 1 @name pitch))\n(def phase (phasor pitch))\n(out phase 1 @name audio)",
    )
    .unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let (_path, root_patch) = load_patch_from_props(&node.props).unwrap();
    let phase = root_patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "phase")
        .unwrap();
    let out = root_patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.kind == NodeKind::Out)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    set_node_edit_position(
        &mut state,
        "root",
        phase,
        (52.0, 18.25),
        node_display_label(phase),
    );
    set_node_edit_position(
        &mut state,
        "root",
        out,
        (133.5, 61.0),
        node_display_label(out),
    );
    set_patcher_interaction_state(key, state);

    let payload = patcher_writeback_payload(&node);
    let Value::Map(map) = payload else {
        panic!("expected payload map");
    };
    let source = match map.get("source").map(|value| value.borrow().clone()) {
        Some(Value::String(source)) => source,
        other => panic!("expected emitted source string, got {other:?}; payload={map:?}"),
    };
    let layout = match map.get("layout").map(|value| value.borrow().clone()) {
        Some(Value::String(layout)) => layout,
        other => panic!("expected emitted layout string, got {other:?}"),
    };
    fs::write(&path, source).unwrap();
    fs::write(sidecar::sidecar_path_for_source(&path), layout).unwrap();

    set_patcher_interaction_state(key, PatcherInteractionState::default());
    let (_path, reloaded) = load_patch_from_props(&node.props).unwrap();
    assert_eq!(
        reloaded
            .nodes
            .iter()
            .find(|patch_node| patch_node.id == "phase")
            .unwrap()
            .position,
        (52.0, 18.25)
    );
    assert_eq!(
        reloaded
            .nodes
            .iter()
            .find(|patch_node| patch_node.kind == NodeKind::Out)
            .unwrap()
            .position,
        (133.5, 61.0)
    );
}

#[test]
fn finalized_create_instrument_flow_reopens_with_saved_created_node_layout() {
    let draft_path = temp_patcher_dsp_path("patcher-create-flow-draft");
    fs::write(
        &draft_path,
        "(def gate (in 1 @name gate))\n\
         (def pitch (in 2 @name pitch))\n\
         (def velocity (in 3 @name velocity))\n\
         (def trigger (in 4 @name trigger))\n\
         (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)\n\
         (def env (adsr gate trigger 5 120 0.8 180))\n\
         (def phase (phasor pitch))\n\
         (def osc (scale phase 0 1 -1 1))\n\
         (out (* osc env velocity (mod gain)) 1 @name audio)\n",
    )
    .unwrap();
    let draft_node = patcher_test_node(&draft_path);
    let key = patcher_state_key(&draft_node);
    let (_path, root_patch) = load_patch_from_props(&draft_node.props).unwrap();
    let phase_to_osc = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == "phase" && connection.to_node == "osc")
        .expect("starter patch should connect phase into osc");

    let mut state = PatcherInteractionState::default();
    let multiply = allocate_created_text_node(&mut state, "root", "* 3");
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(phase_to_osc),
        ));
    connect_output_to_input(&mut state, "root", "phase", &multiply, 0);
    connect_output_to_input(&mut state, "root", &multiply, "osc", 0);
    let placed_position = (87.25, 24.5);
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &multiply))
        .unwrap()
        .position = placed_position;
    set_patcher_interaction_state(key, state);

    let payload = patcher_writeback_payload(&draft_node);
    let Value::Map(map) = payload else {
        panic!("expected payload map");
    };
    let source = match map.get("source").map(|value| value.borrow().clone()) {
        Some(Value::String(source)) => source,
        other => panic!("expected emitted source string, got {other:?}; payload={map:?}"),
    };
    let layout = match map.get("layout").map(|value| value.borrow().clone()) {
        Some(Value::String(layout)) => layout,
        other => panic!("expected emitted layout string, got {other:?}"),
    };
    assert!(
        source.contains("(def mul1 (* phase 3.0))"),
        "created multiply should be materialized before final save:\n{source}"
    );
    let layout_json: serde_json::Value = serde_json::from_str(&layout).unwrap();
    let mul_layout = &layout_json["root"]["nodes"]["mul1"];
    assert!(
        (mul_layout["x"].as_f64().unwrap() - placed_position.0 as f64).abs() < 0.0001
            && (mul_layout["y"].as_f64().unwrap() - placed_position.1 as f64).abs() < 0.0001,
        "emitted finalized layout should transfer created-node position to generated id: {layout}"
    );

    let final_path = temp_patcher_dsp_path("patcher-create-flow-finalized");
    fs::write(&final_path, source).unwrap();
    fs::write(sidecar::sidecar_path_for_source(&final_path), layout).unwrap();
    set_patcher_interaction_state(key, PatcherInteractionState::default());
    let final_node = patcher_test_node(&final_path);
    let (_path, reloaded) = load_patch_from_props(&final_node.props).unwrap();
    let mul = reloaded
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "mul1")
        .expect("finalized patch should reload generated multiply node");
    assert_eq!(mul.position, placed_position);
}

#[test]
fn existing_instrument_edit_reopens_with_created_chain_layout_after_deleting_source_node() {
    let path = temp_patcher_dsp_path("patcher-existing-created-chain");
    fs::write(
        &path,
        "(def gate (in 1 @name gate))\n\
         (def pitch (in 2 @name pitch))\n\
         (def velocity (in 3 @name velocity))\n\
         (def trigger (in 4 @name trigger))\n\
         (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)\n\
         (def env (adsr gate trigger 5 120 0.8 180))\n\
         (def phase (phasor pitch))\n\
         (def osc (scale phase 0 1 -1 1))\n\
         (out (* osc env velocity (mod gain)) 1 @name audio)\n",
    )
    .unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let (_path, root_patch) = load_patch_from_props(&node.props).unwrap();
    let osc = root_patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "osc")
        .expect("fixture should project source osc node");
    let final_multiply = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == "osc")
        .map(|connection| connection.to_node.clone())
        .expect("osc should feed final output multiply");

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &osc.id));
    let multiply = allocate_created_text_node(&mut state, "root", "* 1");
    let cosine = allocate_created_text_node(&mut state, "root", "cos");
    let mul_position = (62.0, 34.0);
    let cos_position = (43.0, 22.0);
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &multiply))
        .unwrap()
        .position = mul_position;
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &cosine))
        .unwrap()
        .position = cos_position;
    connect_output_to_input(&mut state, "root", "phase", &multiply, 0);
    connect_output_to_input(&mut state, "root", &multiply, &cosine, 0);
    connect_output_to_input(&mut state, "root", &cosine, &final_multiply, 0);
    set_patcher_interaction_state(key, state);

    let payload = patcher_writeback_payload(&node);
    let Value::Map(map) = payload else {
        panic!("expected payload map");
    };
    let source = match map.get("source").map(|value| value.borrow().clone()) {
        Some(Value::String(source)) => source,
        other => panic!("expected emitted source string, got {other:?}; payload={map:?}"),
    };
    let layout = match map.get("layout").map(|value| value.borrow().clone()) {
        Some(Value::String(layout)) => layout,
        other => panic!("expected emitted layout string, got {other:?}"),
    };
    assert!(
        source.contains("(def mul1 (* phase 1.0))")
            && source.contains("(def cos1 (cos mul1)")
            && source.contains("(* cos1 env velocity"),
        "created replacement chain should be materialized before save:\n{source}"
    );
    let layout_json: serde_json::Value = serde_json::from_str(&layout).unwrap();
    assert!(
        (layout_json["root"]["nodes"]["mul1"]["x"].as_f64().unwrap() - mul_position.0 as f64).abs()
            < 0.0001
            && (layout_json["root"]["nodes"]["mul1"]["y"].as_f64().unwrap()
                - mul_position.1 as f64)
                .abs()
                < 0.0001,
        "emitted layout should keep created multiply position: {layout}"
    );
    assert!(
        (layout_json["root"]["nodes"]["cos1"]["x"].as_f64().unwrap() - cos_position.0 as f64).abs()
            < 0.0001
            && (layout_json["root"]["nodes"]["cos1"]["y"].as_f64().unwrap()
                - cos_position.1 as f64)
                .abs()
                < 0.0001,
        "emitted layout should keep created cos position: {layout}"
    );

    fs::write(&path, source).unwrap();
    fs::write(sidecar::sidecar_path_for_source(&path), layout).unwrap();
    set_patcher_interaction_state(key, PatcherInteractionState::default());
    let (_path, reloaded) = load_patch_from_props(&node.props).unwrap();
    assert_eq!(
        reloaded
            .nodes
            .iter()
            .find(|patch_node| patch_node.id == "mul1")
            .unwrap()
            .position,
        mul_position
    );
    assert_eq!(
        reloaded
            .nodes
            .iter()
            .find(|patch_node| patch_node.id == "cos1")
            .unwrap()
            .position,
        cos_position
    );
}

#[test]
fn semantic_save_payload_maps_created_literal_layout_to_generated_constant_binding() {
    let path = temp_patcher_dsp_path("patcher-created-literal-layout");
    fs::write(
        &path,
        "(def pitch (in 1 @name pitch))\n\
         (def phase (phasor pitch))\n\
         (def tri (triangle phase 0.1))\n\
         (out tri 1 @name audio)\n",
    )
    .unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let literal_position = (19.25, 20.5);
    let phasor_position = (22.0, 22.75);

    let mut state = PatcherInteractionState::default();
    let literal = allocate_created_text_node(&mut state, "root", "0.3");
    let phasor = allocate_created_text_node(&mut state, "root", "phasor");
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &literal))
        .unwrap()
        .position = literal_position;
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &phasor))
        .unwrap()
        .position = phasor_position;
    connect_output_to_input(&mut state, "root", &literal, &phasor, 0);
    connect_output_to_input(&mut state, "root", &phasor, "tri", 1);
    set_patcher_interaction_state(key, state);

    let payload = patcher_writeback_payload(&node);
    let Value::Map(map) = payload else {
        panic!("expected payload map");
    };
    let source = match map.get("source").map(|value| value.borrow().clone()) {
        Some(Value::String(source)) => source,
        other => panic!("expected emitted source string, got {other:?}; payload={map:?}"),
    };
    let layout = match map.get("layout").map(|value| value.borrow().clone()) {
        Some(Value::String(layout)) => layout,
        other => panic!("expected emitted layout string, got {other:?}"),
    };

    assert!(
        source.contains("(def value1 0.3)") && source.contains("(def phasor1 (phasor value1))"),
        "created literal should materialize as a named constant feeding phasor:\n{source}"
    );
    let layout_json: serde_json::Value = serde_json::from_str(&layout).unwrap();
    assert!(
        layout_json["root"]["nodes"]["0.3"].is_null(),
        "layout should not use literal text as the saved node id: {layout}"
    );
    assert!(
        (layout_json["root"]["nodes"]["value1"]["x"]
            .as_f64()
            .unwrap()
            - literal_position.0 as f64)
            .abs()
            < 0.0001
            && (layout_json["root"]["nodes"]["value1"]["y"]
                .as_f64()
                .unwrap()
                - literal_position.1 as f64)
                .abs()
                < 0.0001,
        "layout should keep the visible constant position under value1: {layout}"
    );
}

#[test]
fn dgenlisp_mod_special_form_projects_as_valid_expression_operator() {
    assert!(dgenlisp_operator_names().contains("mod"));

    let patch = parse("(def level (mod gain))");
    let node = patch.nodes.iter().find(|node| node.op == "mod").unwrap();

    assert_eq!(node.args.len(), 1);
    assert_eq!(node.diagnostic, None);
}

#[test]
fn instrument_preamble_helpers_project_as_documented_operators() {
    let patch = parse(
        r#"
        (def env (adsr gate trigger 1 2 0.5 4))
        (def lfo (mod_unipolar osc))
        (def pitch (apply_pitch_mod_semi base mod1 12))
        (def cutoff (apply_cutoff_mod_safe base mod1 2000))
        (def width (apply_pw_mod_safe base mod1 0.2))
        (def blep (polyblep phase freq))
        (def typo (polypleb phase freq))
        (def wave (wavetable-read-512 table slot phase))
        (def morph (wavetable-morph-512 table a b phase mix))
        (def filtered (svf sig cutoff q 0))
        "#,
    );

    for op in [
        "adsr",
        "mod_unipolar",
        "apply_pitch_mod_semi",
        "apply_cutoff_mod_safe",
        "apply_pw_mod_safe",
        "polyblep",
        "polypleb",
        "wavetable-read-512",
        "wavetable-morph-512",
        "svf",
    ] {
        let node = patch
            .nodes
            .iter()
            .find(|node| node.op == op)
            .unwrap_or_else(|| panic!("missing projected node for {op}"));
        assert_eq!(node.diagnostic, None, "{op}");
    }
}

#[test]
fn projects_instrument_plumbing_and_nested_calls() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (triangle (phasor pitch)))
            (out sig 1 @name audio)
            "#,
    );
    assert!(
        patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::In && node.id == "pitch")
    );
    assert!(patch.nodes.iter().any(|node| node.op == "phasor"));
    assert!(patch.nodes.iter().any(|node| node.op == "triangle"));
    assert!(
        patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::Out && node.id == "audio")
    );
    assert!(patch.connections.len() >= 3, "{:#?}", patch.connections);
}

#[test]
fn source_metadata_tracks_nested_expression_paths() {
    let patch = parse("(def result (phasor (* 25 (param freq @min 1 @max 100)) (rampToTrig xyz)))");

    let phasor = patch.nodes.iter().find(|node| node.op == "phasor").unwrap();
    let multiply = patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let constant = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Constant && node.op == "25")
        .unwrap();
    let param = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param)
        .unwrap();
    let ramp = patch
        .nodes
        .iter()
        .find(|node| node.op == "rampToTrig")
        .unwrap();

    assert_eq!(node_expr(phasor), source_expr(SourceScopeId::Root, 0, &[2]));
    assert_eq!(
        node_expr(multiply),
        source_expr(SourceScopeId::Root, 0, &[2, 1])
    );
    assert!(matches!(
        &multiply.source.as_ref().unwrap().owner,
        SourceOwner::NestedExpr { expr }
            if expr == &source_expr(SourceScopeId::Root, 0, &[2, 1])
    ));
    assert_eq!(
        node_expr(constant),
        source_expr(SourceScopeId::Root, 0, &[2, 1, 1])
    );
    assert!(matches!(
        &constant.source.as_ref().unwrap().owner,
        SourceOwner::ArgumentSlot { call, arg }
            if call == &source_expr(SourceScopeId::Root, 0, &[2, 1])
                && arg.item_index == 1
    ));
    assert_eq!(
        node_expr(param),
        source_expr(SourceScopeId::Root, 0, &[2, 1, 2])
    );
    assert_eq!(
        node_expr(ramp),
        source_expr(SourceScopeId::Root, 0, &[2, 2])
    );

    let phasor_shape = phasor
        .source
        .as_ref()
        .and_then(|source| source.call_shape.as_ref())
        .unwrap();
    assert_eq!(phasor_shape.positional_args.len(), 2);
    assert_eq!(phasor_shape.positional_args[0].semantic_index, 0);
    assert_eq!(phasor_shape.positional_args[0].item_index, 1);
    assert_eq!(
        phasor_shape.positional_args[0].expr,
        source_expr(SourceScopeId::Root, 0, &[2, 1])
    );
    assert_eq!(phasor_shape.positional_args[1].semantic_index, 1);
    assert_eq!(phasor_shape.positional_args[1].item_index, 2);
    assert_eq!(
        phasor_shape.positional_args[1].expr,
        source_expr(SourceScopeId::Root, 0, &[2, 2])
    );
}

#[test]
fn source_metadata_separates_positional_args_from_attributes() {
    let patch = parse("(param freq @min 1 @max 100)");
    let param = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param)
        .unwrap();
    let shape = param
        .source
        .as_ref()
        .and_then(|source| source.call_shape.as_ref())
        .unwrap();

    assert_eq!(shape.positional_args.len(), 1);
    assert_eq!(shape.positional_args[0].semantic_index, 0);
    assert_eq!(shape.positional_args[0].item_index, 1);
    assert_eq!(
        shape.positional_args[0].expr,
        source_expr(SourceScopeId::Root, 0, &[1])
    );

    assert_eq!(
        shape
            .attributes
            .iter()
            .map(|attr| (
                attr.key.as_str(),
                attr.key_item_index,
                attr.value_item_index
            ))
            .collect::<Vec<_>>(),
        vec![("@min", 2, 3), ("@max", 4, 5)]
    );
    assert_eq!(
        shape.attributes[0].value,
        source_expr(SourceScopeId::Root, 0, &[3])
    );
    assert_eq!(
        shape.attributes[1].value,
        source_expr(SourceScopeId::Root, 0, &[5])
    );
}

#[test]
fn operator_metadata_comes_from_generated_dgenlisp_json() {
    let names = dgenlisp_operator_names();
    assert!(names.len() >= 100, "expected generated operator metadata");
    assert!(names.contains("phasor"));
    assert!(names.contains("spectrum-delay"));
    assert!(names.contains("tosignal"));
}

#[test]
fn projects_params_and_attributes_as_param_node() {
    let patch = parse("(param cutoff @default 800 @min 20 @max 12000)");
    let node = patch.nodes.iter().find(|node| node.id == "cutoff").unwrap();
    assert_eq!(node.kind, NodeKind::Param);
    assert_eq!(
        node_display_label(node),
        "param cutoff @default 800 @min 20 @max 12000"
    );
}

#[test]
fn param_references_project_as_connections_not_literal_args() {
    let patch = parse(
        r#"
            (param size @min 0 @max 3000 @default 300)
            (def input (in 1))
            (def delayed (delay input size))
            "#,
    );
    let param = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param)
        .unwrap();
    let delay = patch.nodes.iter().find(|node| node.op == "delay").unwrap();

    assert_eq!(
        node_display_label(param),
        "param size @min 0 @max 3000 @default 300"
    );
    assert_eq!(node_display_label(delay), "delay");
    assert!(
        patch.connections.iter().any(|connection| {
            connection.from_node == param.id
                && connection.to_node == delay.id
                && connection.to_input == 1
        }),
        "{:#?}",
        patch.connections
    );
}

#[test]
fn source_metadata_resolves_param_references_by_binding_identity() {
    let patch = parse(
        r#"
            (def unresolved (phasor freq))
            (param freq @min 1 @max 100)
            (def a (phasor freq))
            (def b (+ freq a))
            "#,
    );
    let param_binding = BindingId {
        scope: SourceScopeId::Root,
        name: "freq".to_string(),
        kind: BindingKind::Param,
    };
    let resolved = patch
        .connections
        .iter()
        .filter_map(|connection| connection.source.as_ref())
        .filter_map(|source| match &source.previous_arg {
            SourceArgValue::SymbolReference {
                symbol,
                resolved_binding,
                ..
            } if symbol == "freq" => resolved_binding.clone(),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(resolved, vec![param_binding.clone(), param_binding]);
    assert!(
        patch
            .nodes
            .iter()
            .find(|node| node.id == "unresolved")
            .unwrap()
            .args
            .iter()
            .any(|arg| matches!(arg, ArgValue::Literal(value) if value == "freq"))
    );
}

#[test]
fn projects_destructuring_def_outputs() {
    let patch = parse("(def (re im) (fft input))");
    let node = patch.nodes.iter().find(|node| node.op == "fft").unwrap();
    assert_eq!(node.outputs, vec!["re".to_string(), "im".to_string()]);
}

#[test]
fn collapses_history_read_and_write_into_make_history_node() {
    let patch = parse(
        r#"
            (make-history h)
            (def sig (noise))
            (def previous (read-history h))
            (def mixed (+ sig previous))
            (write-history h sig)
            "#,
    );
    let history = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::History)
        .expect("history node");
    assert_eq!(node_display_label(history), "history");
    assert_eq!(
        patch
            .nodes
            .iter()
            .filter(|node| matches!(
                node.op.as_str(),
                "make-history" | "read-history" | "write-history"
            ))
            .count(),
        1,
        "{:#?}",
        patch.nodes
    );
    assert!(
        patch
            .connections
            .iter()
            .any(|connection| connection.from_node == history.id
                && connection.kind == ConnectionKind::Forward)
    );
    assert!(
        patch
            .connections
            .iter()
            .any(|connection| connection.to_node == history.id
                && connection.to_input == 0
                && connection.kind == ConnectionKind::Feedback)
    );
}

#[test]
fn source_metadata_tracks_history_compound_ownership_and_connections() {
    let patch = parse(
        r#"
            (make-history h)
            (def sig (noise))
            (def delta (- (read-history h) sig))
            (write-history h sig)
            "#,
    );
    let history = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::History)
        .expect("history node");
    let source = history.source.as_ref().expect("history source");
    let SourceOwner::Compound { parts } = &source.owner else {
        panic!("history should have compound owner: {source:#?}");
    };
    assert!(
        parts.iter().any(
            |owner| matches!(owner, SourceOwner::TopLevelForm { form_id } if form_id.index == 0)
        )
    );
    assert!(
        parts
            .iter()
            .any(|owner| matches!(owner, SourceOwner::NestedExpr { expr } if expr == &source_expr(SourceScopeId::Root, 2, &[2, 1])))
    );
    assert!(
        parts.iter().any(
            |owner| matches!(owner, SourceOwner::TopLevelForm { form_id } if form_id.index == 3)
        )
    );

    let feedback = patch
        .connections
        .iter()
        .find(|connection| connection.kind == ConnectionKind::Feedback)
        .unwrap();
    let feedback_source = feedback.source.as_ref().expect("feedback source");
    assert_eq!(
        feedback_source.to_call,
        source_expr(SourceScopeId::Root, 3, &[])
    );
    assert_eq!(feedback_source.to_arg.semantic_index, 1);
    assert_eq!(feedback_source.to_arg.item_index, 2);

    assert!(
        patch
            .connections
            .iter()
            .any(|connection| connection.from_node == history.id
                && connection.kind == ConnectionKind::Forward
                && connection.source.as_ref().is_some_and(|source| {
                    source.to_call == source_expr(SourceScopeId::Root, 2, &[2])
                        && source.to_arg.semantic_index == 0
                        && source.to_arg.item_index == 1
                }))
    );
}

#[test]
fn unsupported_forms_become_code_islands() {
    let patch = parse("(if gate (out 1 1 @name audio) (out 0 1 @name audio))");
    assert!(
        patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::CodeIsland)
    );
    assert!(!patch.diagnostics.is_empty());
}

#[test]
fn source_metadata_marks_code_island_owner() {
    let patch = parse("(if gate (out 1 1 @name audio) (out 0 1 @name audio))");
    let code = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::CodeIsland)
        .unwrap();
    let source = code.source.as_ref().expect("code island source");
    assert_eq!(source.expr, Some(source_expr(SourceScopeId::Root, 0, &[])));
    assert!(matches!(
        &source.owner,
        SourceOwner::CodeIsland { form_id }
            if *form_id == SourceFormId {
                scope: SourceScopeId::Root,
                index: 0,
            }
    ));
}

#[test]
fn source_metadata_scopes_macro_subpatches_separately() {
    let patch = parse(
        r#"
            (def root (phasor 1))
            (defmacro ap (sig g)
              (def scaled (* sig g))
              (phasor scaled))
            "#,
    );
    let root_phasor = patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .expect("root phasor");
    assert_eq!(
        node_expr(root_phasor),
        source_expr(SourceScopeId::Root, 0, &[2])
    );

    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let scaled = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.op == "*")
        .expect("macro multiply");
    let macro_scope = SourceScopeId::Macro {
        name: "ap".to_string(),
    };
    assert_eq!(node_expr(scaled), source_expr(macro_scope.clone(), 0, &[2]));

    let macro_phasor = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .expect("macro phasor");
    assert_eq!(
        node_expr(macro_phasor),
        source_expr(macro_scope.clone(), 1, &[])
    );

    let sig_binding = BindingId {
        scope: macro_scope.clone(),
        name: "sig".to_string(),
        kind: BindingKind::MacroParam,
    };
    assert!(
        macro_patch
            .patch
            .connections
            .iter()
            .filter_map(|connection| connection.source.as_ref())
            .any(|source| matches!(
                &source.previous_arg,
                SourceArgValue::SymbolReference {
                    symbol,
                    resolved_binding: Some(binding),
                    ..
                } if symbol == "sig" && binding == &sig_binding
            ))
    );
    assert_ne!(node_expr(root_phasor).form_id.scope, macro_scope);
}

#[test]
fn debug_emit_preserves_nested_structure_for_source_backed_patch() {
    let patch = parse("(def result (phasor (* 25 (param freq @min 1 @max 100)) (rampToTrig xyz)))");

    assert_eq!(
        emit_patch_debug_lisp(&patch),
        "(def result (phasor (* 25 (param freq @min 1 @max 100)) (rampToTrig xyz)))"
    );
}

#[test]
fn debug_emit_reflects_committed_node_text_edits_without_saving() {
    let source = parse(
        r#"
            (param freq)
            (def result (phasor freq))
            "#,
    );
    let phasor = source
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", phasor, "phasor".to_string());
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &phasor.id))
        .unwrap()
        .text = "sine".to_string();

    let patch = patch_with_interaction_state(source, &state, "root");

    assert_eq!(
        emit_patch_debug_lisp(&patch),
        "(param freq)\n(def result (sine freq))"
    );
}

#[test]
fn debug_emit_wraps_macro_subpatches_as_defmacro() {
    let patch = parse(
        r#"
            (defmacro ap (sig g)
              (def node (+ (* sig 1) (* h g)))
              (- node g))
            "#,
    );
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();

    assert_eq!(
        emit_patch_debug_lisp_for_view("macro:ap", &macro_patch.patch),
        "(defmacro ap (sig g)\n  (def node (+ (* sig 1) (* h g)))\n  (- node g))"
    );
}

#[test]
fn debug_emit_uses_macro_parameter_names_for_edited_connections() {
    let patch = parse(
        r#"
            (defmacro ap (sig g)
              (def node (+ sig (* h g)))
              node)
            "#,
    );
    let mut macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap()
        .patch
        .clone();
    let sig_node_id = macro_patch
        .nodes
        .iter()
        .find(|node| {
            matches!(
                node.source.as_ref().map(|source| &source.owner),
                Some(SourceOwner::MacroParameter { binding, .. }) if binding.name == "sig"
            )
        })
        .unwrap()
        .id
        .clone();
    let plus_node_id = macro_patch
        .nodes
        .iter()
        .find(|node| node.op == "+")
        .unwrap()
        .id
        .clone();
    macro_patch.connections.retain(|connection| {
        !(connection.from_node == sig_node_id && connection.to_node == plus_node_id)
    });
    macro_patch.nodes.push(PatchNode {
        id: "created-mul".to_string(),
        op: "*".to_string(),
        kind: NodeKind::Builtin,
        label: "* 1".to_string(),
        args: vec![ArgValue::ConnectedExpr, ArgValue::Literal("1".to_string())],
        outputs: vec!["out".to_string()],
        position: (0.0, 0.0),
        diagnostic: None,
        source: None,
    });
    macro_patch.connections.push(PatchConnection {
        from_node: sig_node_id,
        from_output: 0,
        to_node: "created-mul".to_string(),
        to_input: 0,
        kind: ConnectionKind::Forward,
        segment: None,
        source: None,
    });
    macro_patch.connections.push(PatchConnection {
        from_node: "created-mul".to_string(),
        from_output: 0,
        to_node: plus_node_id,
        to_input: 0,
        kind: ConnectionKind::Forward,
        segment: None,
        source: None,
    });

    assert_eq!(
        emit_patch_debug_lisp_for_view("macro:ap", &macro_patch),
        "(defmacro ap (sig g)\n  (def node (+ (* sig 1) (* h g)))\n  node)"
    );
}

#[test]
fn writeback_emits_unchanged_root_patch_as_complete_normalized_lisp() {
    let source = r#"
        (param freq)
        (def result (phasor freq))
        (out result 1)
    "#;

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(param freq)\n(def result (phasor freq))\n(out result 1)"
    );
}

#[test]
fn writeback_emits_unchanged_macro_as_complete_normalized_defmacro() {
    let source = r#"
        (defmacro ap (sig g)
          (def node (+ sig g))
          node)
    "#;

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(defmacro ap (sig g) (def node (+ sig g)) node)"
    );
}

#[test]
fn writeback_node_text_edit_rewrites_owning_root_expression() {
    let source = r#"
        (param freq)
        (def result (phasor freq))
    "#;
    let patch = parse(source);
    let phasor = patch.nodes.iter().find(|node| node.op == "phasor").unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", phasor, "phasor".to_string());
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &phasor.id))
        .unwrap()
        .text = "sin".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param freq)\n(def result (sin freq))"
    );
}

#[test]
fn writeback_allows_source_connection_segment_layout_edit() {
    let source = r#"
        (param freq)
        (def result (phasor freq))
    "#;
    let patch = parse(source);
    let connection = patch.connections.first().unwrap();
    let mut state = PatcherInteractionState::default();
    set_connection_segment_edit(
        &mut state,
        "root",
        connection,
        Some(CableSegmentInfo {
            is_segmented: true,
            segment_row: 12.5,
        }),
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param freq)\n(def result (phasor freq))"
    );
}

#[test]
fn writeback_source_connection_edit_moves_destination_input() {
    let source = r#"
        (param freq)
        (param mod)
        (def result (phasor freq mod))
    "#;
    let patch = parse(source);
    let connection = patch
        .connections
        .iter()
        .find(|connection| connection.to_input == 0)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    set_connection_segment_edit(&mut state, "root", connection, connection.segment);
    state
        .edit_state
        .connections
        .get_mut(&connection_edit_key(
            "root",
            &source_connection_id(connection),
        ))
        .unwrap()
        .to
        .input_index = 1;

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param freq)\n(param mod)\n(def result (phasor __patcher_missing_input__ freq))"
    );
}

#[test]
fn writeback_source_connection_edit_replaces_source_reference() {
    let source = r#"
        (def a (in 1))
        (def b (in 2))
        (def result (+ a 1))
    "#;
    let patch = parse(source);
    let b = patch.nodes.iter().find(|node| node.id == "b").unwrap();
    let connection = patch.connections.first().unwrap();
    let mut state = PatcherInteractionState::default();
    set_connection_segment_edit(&mut state, "root", connection, connection.segment);
    state
        .edit_state
        .connections
        .get_mut(&connection_edit_key(
            "root",
            &source_connection_id(connection),
        ))
        .unwrap()
        .from = OutputPortRef {
        node_id: b.id.clone(),
        output_index: 0,
    };

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def a (in 1))\n(def b (in 2))\n(def result (+ b 1.0))"
    );
}

#[test]
fn writeback_root_param_rename_updates_resolved_references_only() {
    let source = r#"
        (def unresolved (phasor freq))
        (param freq @min 1 @max 100)
        (def a (phasor freq))
        (def b (+ freq a))
    "#;
    let patch = parse(source);
    let param = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", param, node_display_label(param));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param.id))
        .unwrap()
        .text = "param cutoff @min 1 @max 100".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def unresolved (phasor freq))\n(param cutoff @min 1.0 @max 100.0)\n(def a (phasor cutoff))\n(def b (+ cutoff a))"
    );
}

#[test]
fn writeback_root_param_rename_collision_returns_blocker() {
    let source = r#"
        (param freq)
        (param cutoff)
        (def result (phasor freq))
    "#;
    let patch = parse(source);
    let param = patch.nodes.iter().find(|node| node.id == "freq").unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", param, node_display_label(param));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param.id))
        .unwrap()
        .text = "param cutoff".to_string();

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::BindingRenameCollision { name, .. }) if name == "cutoff"
    ));
}

#[test]
fn writeback_root_param_rename_with_code_island_returns_blocker() {
    let source = r#"
        (if gate freq 0)
        (param freq)
        (def result (phasor freq))
    "#;
    let patch = parse(source);
    let param = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", param, node_display_label(param));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param.id))
        .unwrap()
        .text = "param cutoff".to_string();

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::BindingRenameBlockedByCodeIsland { name, .. }) if name == "freq"
    ));
}

#[test]
fn writeback_nested_node_text_edit_preserves_nested_structure() {
    let source = "(def result (phasor (* 25 freq) (mix xyz a b)))";
    let patch = parse(source);
    let nested = patch.nodes.iter().find(|node| node.op == "mix").unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", nested, "mix".to_string());
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &nested.id))
        .unwrap()
        .text = "+".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def result (phasor (* 25.0 freq) (+ xyz a b)))"
    );
}

#[test]
fn writeback_macro_node_text_edit_rewrites_inside_defmacro() {
    let source = r#"
        (defmacro ap (sig g)
          (def node (+ sig g))
          node)
    "#;
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let plus = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.op == "+")
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "macro:ap", plus, "+".to_string());
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:ap", &plus.id))
        .unwrap()
        .text = "mix".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro ap (sig g) (def node (mix sig g)) node)"
    );
}

#[test]
fn writeback_macro_parameter_rename_updates_header_and_resolved_references() {
    let source = r#"
        (def sig (in 1))
        (defmacro ap (sig g)
          (def node (+ sig g))
          (phasor node))
    "#;
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let sig = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.outputs == vec!["sig".to_string()])
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "macro:ap", sig, node_display_label(sig));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:ap", &sig.id))
        .unwrap()
        .text = "in 1 @name input".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))\n(defmacro ap (input g) (def node (+ input g)) (phasor node))"
    );
}

#[test]
fn writeback_macro_parameter_in_form_without_name_preserves_existing_symbol() {
    let source = "(defmacro ap (sig g) (+ sig g))";
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let sig = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.outputs == vec!["sig".to_string()])
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "macro:ap", sig, node_display_label(sig));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:ap", &sig.id))
        .unwrap()
        .text = "in 1".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro ap (sig g) (+ sig g))"
    );
}

#[test]
fn writeback_macro_parameter_rename_collision_returns_blocker() {
    let source = "(defmacro ap (sig g) (+ sig g))";
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let sig = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.outputs == vec!["sig".to_string()])
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "macro:ap", sig, node_display_label(sig));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:ap", &sig.id))
        .unwrap()
        .text = "in 1 @name g".to_string();

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::BindingRenameCollision { name, .. }) if name == "g"
    ));
}

#[test]
fn writeback_macro_parameter_rename_with_code_island_returns_blocker() {
    let source = "(defmacro ap (sig) (if gate sig 0) sig)";
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let sig = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.outputs == vec!["sig".to_string()])
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "macro:ap", sig, node_display_label(sig));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:ap", &sig.id))
        .unwrap()
        .text = "in 1 @name input".to_string();

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::BindingRenameBlockedByCodeIsland { name, .. }) if name == "sig"
    ));
}

#[test]
fn writeback_synthetic_macro_return_out_is_not_emitted() {
    let source = "(defmacro passthrough (sig) sig)";

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(defmacro passthrough (sig) sig)"
    );
}

#[test]
fn writeback_untouched_code_island_emits_normalized_source() {
    let source = r#"
        (let ((x 1)) x)
        (param freq)
    "#;

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(let ((x 1.0)) x)\n(param freq)"
    );
}

#[test]
fn writeback_edited_code_island_returns_blocker() {
    let source = "(let ((x 1)) x)";
    let patch = parse(source);
    let code = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::CodeIsland)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", code, node_display_label(code));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &code.id))
        .unwrap()
        .text = "code changed".to_string();

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::EditedCodeIsland { .. })
    ));
}

#[test]
fn writeback_unknown_operator_edit_returns_blocker() {
    let source = "(def result (phasor freq))";
    let patch = parse(source);
    let phasor = patch.nodes.iter().find(|node| node.op == "phasor").unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", phasor, "phasor".to_string());
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &phasor.id))
        .unwrap()
        .text = "definitely-not-an-op".to_string();

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::UnknownOperator { operator, .. })
            if operator == "definitely-not-an-op"
    ));
}

#[test]
fn writeback_created_node_returns_phase_boundary_blocker() {
    let source = "(def result (phasor freq))";
    let mut state = PatcherInteractionState::default();
    allocate_created_node(&mut state, "root", (1.0, 1.0));

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::UnsupportedCreatedNode { .. })
    ));
}

#[test]
fn writeback_existing_feedforward_history_round_trips() {
    let source = r#"
        (make-history h)
        (def sig (in 1))
        (def delta (- sig (read-history h)))
        (write-history h sig)
    "#;

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(make-history h)\n(def sig (in 1))\n(def delta (- sig (read-history h)))\n(write-history h sig)"
    );
}

#[test]
fn writeback_existing_feedback_history_round_trips() {
    let source = r#"
        (make-history h)
        (write-history h (mix sig (read-history h) alpha))
    "#;

    assert_eq!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        )
        .unwrap(),
        "(make-history h)\n(write-history h (mix sig (read-history h) alpha))"
    );
}

#[test]
fn writeback_created_history_uses_generated_name_for_make_read_and_write() {
    let source = r#"
        (def sig (in 1))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let history_id = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &history_id))
        .unwrap()
        .text = "history".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: history_id.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: history_id,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(make-history history1)\n(def sig (in 1))\n(out (read-history history1) 1)\n(write-history history1 sig)"
    );
}

#[test]
fn writeback_created_history_feedback_into_created_mix_emits_onepole() {
    let source = r#"
        (def sig (in 1))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    let mix = allocate_created_text_node(&mut state, "root", "mix ? 0.99");
    let history = allocate_created_text_node(&mut state, "root", "history");
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));

    connect_output_to_input(&mut state, "root", &sig.id, &mix, 0);
    connect_output_to_input(&mut state, "root", &sig.id, &history, 0);
    connect_output_to_input(&mut state, "root", &history, &mix, 1);
    connect_output_to_input(&mut state, "root", &mix, &out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert_eq!(
        emitted,
        "(make-history history1)\n(def sig (in 1))\n(def mix1 (mix sig (read-history history1) 0.99))\n(out mix1 1)\n(write-history history1 sig)"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
    assert!(!emitted.contains("created-"));
}

#[test]
fn writeback_created_history_recursive_onepole_writes_generated_mix() {
    let source = r#"
        (def sig (in 1))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    let mix = allocate_created_text_node(&mut state, "root", "mix ? 0.99");
    let history = allocate_created_text_node(&mut state, "root", "history");
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));

    connect_output_to_input(&mut state, "root", &sig.id, &mix, 0);
    connect_output_to_input(&mut state, "root", &history, &mix, 1);
    connect_output_to_input(&mut state, "root", &mix, &out.id, 0);
    connect_output_to_input(&mut state, "root", &mix, &history, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert_eq!(
        emitted,
        "(make-history history1)\n(def sig (in 1))\n(def mix1 (mix sig (read-history history1) 0.99))\n(out mix1 1)\n(write-history history1 mix1)"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
    assert!(!emitted.contains("created-"));
}

#[test]
fn writeback_created_history_longer_feedback_chain_preserves_generated_dependency_order() {
    let source = r#"
        (def sig (in 1))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    let mix = allocate_created_text_node(&mut state, "root", "mix ? 0.99");
    let multiply = allocate_created_text_node(&mut state, "root", "* 0.5");
    let history = allocate_created_text_node(&mut state, "root", "history");
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));

    connect_output_to_input(&mut state, "root", &sig.id, &mix, 0);
    connect_output_to_input(&mut state, "root", &history, &mix, 1);
    connect_output_to_input(&mut state, "root", &mix, &multiply, 0);
    connect_output_to_input(&mut state, "root", &multiply, &out.id, 0);
    connect_output_to_input(&mut state, "root", &multiply, &history, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert_eq!(
        emitted,
        "(make-history history1)\n(def sig (in 1))\n(def mix1 (mix sig (read-history history1) 0.99))\n(def mul1 (* mix1 0.5))\n(out mul1 1)\n(write-history history1 mul1)"
    );
    assert_eq!(emitted.matches("(make-history").count(), 1);
    assert_eq!(emitted.matches("(write-history").count(), 1);
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
    assert!(!emitted.contains("created-"));
}

#[test]
fn writeback_source_history_read_into_created_mix_preserves_history_name() {
    let source = r#"
        (make-history h)
        (def sig (in 1))
        (out sig 1)
        (write-history h sig)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let history = patch.nodes.iter().find(|node| node.id == "h").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    let mix = allocate_created_text_node(&mut state, "root", "mix ? 0.99");
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));

    connect_output_to_input(&mut state, "root", &sig.id, &mix, 0);
    connect_output_to_input(&mut state, "root", &history.id, &mix, 1);
    connect_output_to_input(&mut state, "root", &mix, &out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert_eq!(
        emitted,
        "(make-history h)\n(def sig (in 1))\n(def mix1 (mix sig (read-history h) 0.99))\n(out mix1 1)\n(write-history h sig)"
    );
    assert_eq!(emitted.matches("(make-history h)").count(), 1);
    assert!(!emitted.contains("history1"));
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn writeback_multiple_writes_to_created_history_return_blocker() {
    let source = r#"
        (def sig (in 1))
        (def other (in 2))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let other = patch.nodes.iter().find(|node| node.id == "other").unwrap();

    let mut state = PatcherInteractionState::default();
    let history = allocate_created_text_node(&mut state, "root", "history");
    connect_output_to_input(&mut state, "root", &sig.id, &history, 0);
    connect_output_to_input(&mut state, "root", &other.id, &history, 0);

    assert!(matches!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state),
        Err(WriteBackError::MultipleHistoryWrites { history_id, .. }) if history_id == history
    ));
}

#[test]
fn writeback_created_macro_history_emits_inside_defmacro() {
    let source = "(defmacro ap (sig) sig)";
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let sig = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.outputs == vec!["sig".to_string()])
        .unwrap();
    let out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = macro_patch
        .patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let history_id = allocate_created_node(&mut state, "macro:ap", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:ap", &history_id))
        .unwrap()
        .text = "history".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:ap",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "macro:ap",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: history_id.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:ap",
        OutputPortRef {
            node_id: history_id,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro ap (sig) (make-history history1) (write-history history1 sig) (read-history history1))"
    );
}

#[test]
fn writeback_multiple_history_writes_return_blocker() {
    let source = r#"
        (make-history h)
        (def sig (in 1))
        (write-history h sig)
        (write-history h 0)
    "#;

    assert!(matches!(
        emit_patch_writeback(
            source,
            PatcherIntent::Instrument,
            &PatcherInteractionState::default()
        ),
        Err(WriteBackError::MultipleHistoryWrites { history_id, .. }) if history_id == "h"
    ));
}

#[test]
fn writeback_generated_binding_uses_existing_high_water_suffix() {
    let source = r#"
        (def sig (in 1))
        (def phasor1 (phasor sig))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created))
        .unwrap()
        .text = "phasor".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: created.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))\n(def phasor2 (phasor sig))\n(def phasor1 (phasor sig))\n(out phasor2 1)"
    );
}

#[test]
fn writeback_generated_binding_can_wrap_nested_output_expression() {
    let source = r#"
        (def sig (in 1))
        (out (phasor sig) 1)
    "#;
    let patch = parse(source);
    let phasor = patch.nodes.iter().find(|node| node.op == "phasor").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let phasor_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phasor.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created))
        .unwrap()
        .text = "* 1".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(phasor_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: phasor.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: created.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))\n(def mul1 (* (phasor sig) 1.0))\n(out mul1 1)"
    );
}

#[test]
fn writeback_can_replace_deleted_def_node_with_created_node_before_later_output() {
    let source = r#"
        (def pitch (in 2 @name pitch))
        (def phase (phasor pitch))
        (def osc (scale phase 0 1 -1 1))
        (out osc 1 @name audio)
    "#;
    let patch = parse(source);
    let phase = patch.nodes.iter().find(|node| node.id == "phase").unwrap();
    let osc = patch.nodes.iter().find(|node| node.id == "osc").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let phase_to_osc = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phase.id && connection.to_node == osc.id)
        .unwrap();
    let osc_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == osc.id && connection.to_node == out.id)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &osc.id));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(phase_to_osc),
        ));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(osc_to_out),
        ));

    let multiply = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &multiply))
        .unwrap()
        .text = "* .1".to_string();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: phase.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: multiply.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: multiply,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def pitch (in 2 @name pitch))\n(def phase (phasor pitch))\n(def mul1 (* phase 0.1))\n(out mul1 1 @name audio)"
    );
}

#[test]
fn writeback_generated_binding_can_depend_on_created_phasor_and_literal() {
    let source = r#"
        (def sig (phasor 440))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let multiply = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &multiply))
        .unwrap()
        .text = "*".to_string();
    let phasor = allocate_created_node(&mut state, "root", (2.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &phasor))
        .unwrap()
        .text = "phasor".to_string();
    let five = allocate_created_node(&mut state, "root", (2.0, 0.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &five))
        .unwrap()
        .text = "5".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: multiply.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: five,
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: phasor,
            output_index: 0,
        },
        InputPortRef {
            node_id: multiply.clone(),
            input_index: 1,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: multiply,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def value1 5.0)\n(def phasor1 (phasor value1))\n(def sig (phasor 440.0))\n(def mul1 (* sig phasor1))\n(out mul1 1)"
    );
}

#[test]
fn writeback_created_modulatable_param_uses_param_name_for_mod_accessor() {
    let source = r#"
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def velocity (in 3 @name velocity))
        (def trigger (in 4 @name trigger))
        (def mod1 (in 5 @name mod1 @modulator 1))
        (def mod2 (in 6 @name mod2 @modulator 2))
        (def mod3 (in 7 @name mod3 @modulator 3))
        (def mod4 (in 8 @name mod4 @modulator 4))
        (def mod5 (in 9 @name mod5 @modulator 5))
        (def mod6 (in 10 @name mod6 @modulator 6))
        (def ext1 (in 11 @name ext1 @modulator 7))
        (def ext2 (in 12 @name ext2 @modulator 8))
        (def ext3 (in 13 @name ext3 @modulator 9))
        (def ext4 (in 14 @name ext4 @modulator 10))
        (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
        (def phase (phasor pitch))
        (out (* phase velocity (mod gain)) 1 @name audio)
    "#;
    let patch = parse(source);
    let pitch = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
    let phase = patch.nodes.iter().find(|node| node.id == "phase").unwrap();
    let pitch_to_phase = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == pitch.id && connection.to_node == phase.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let param = allocate_created_node(&mut state, "root", (1.0, 1.0));
    let param_text = "param newparam @default 0.5 @min 0 @max 1 @mod true @mod-mode additive";
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param))
        .unwrap()
        .text = param_text.to_string();
    let mod_node = allocate_created_node(&mut state, "root", (2.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &mod_node))
        .unwrap()
        .text = "mod".to_string();
    let add = allocate_created_node(&mut state, "root", (3.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &add))
        .unwrap()
        .text = "+".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(pitch_to_phase),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: param,
            output_index: 0,
        },
        InputPortRef {
            node_id: mod_node.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: mod_node,
            output_index: 0,
        },
        InputPortRef {
            node_id: add.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: pitch.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: add.clone(),
            input_index: 1,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: add,
            output_index: 0,
        },
        InputPortRef {
            node_id: phase.id.clone(),
            input_index: 0,
        },
    );

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    assert!(
        emitted.contains(
            "(param newparam @default 0.5 @min 0.0 @max 1.0 @mod true @mod-mode additive)"
        )
    );
    assert!(emitted.contains("(def modulated1 (mod newparam))"));
    assert!(emitted.contains("(def add1 (+ modulated1 pitch))"));
    assert!(emitted.contains("(def phase (phasor add1))"));
    assert!(!emitted.contains("(def param"));
    assert!(!emitted.contains("(mod param"));
    assert!(!emitted.contains("(def mod7"));
    let ext4_index = emitted
        .find("(def ext4 (in 14 @name ext4 @modulator 10))")
        .expect("instrument modulator inputs should be present");
    let modulated_index = emitted
        .find("(def modulated1 (mod newparam))")
        .expect("created mod accessor should be materialized");
    let phase_index = emitted
        .find("(def phase (phasor add1))")
        .expect("source consumer should be rewritten");
    assert!(ext4_index < modulated_index);
    assert!(modulated_index < phase_index);
}

#[test]
fn writeback_created_unconnected_modulatable_param_follows_modulator_inputs() {
    let source = r#"
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def velocity (in 3 @name velocity))
        (def trigger (in 4 @name trigger))
        (def mod1 (in 5 @name mod1 @modulator 1))
        (def mod2 (in 6 @name mod2 @modulator 2))
        (def mod3 (in 7 @name mod3 @modulator 3))
        (def mod4 (in 8 @name mod4 @modulator 4))
        (def mod5 (in 9 @name mod5 @modulator 5))
        (def mod6 (in 10 @name mod6 @modulator 6))
        (def ext1 (in 11 @name ext1 @modulator 7))
        (def ext2 (in 12 @name ext2 @modulator 8))
        (def ext3 (in 13 @name ext3 @modulator 9))
        (def ext4 (in 14 @name ext4 @modulator 10))
        (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
        (def phase (phasor pitch))
        (out (* phase velocity (mod gain)) 1 @name audio)
    "#;
    let mut state = PatcherInteractionState::default();
    let param = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param))
        .unwrap()
        .text = "param xyz @default 0 @min 0 @max 1 @mod true @mod-mode additive".to_string();

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    let ext4_index = emitted
        .find("(def ext4 (in 14 @name ext4 @modulator 10))")
        .expect("instrument modulator inputs should be present");
    let xyz_index = emitted
        .find("(param xyz @default 0.0 @min 0.0 @max 1.0 @mod true @mod-mode additive)")
        .expect("created modulatable param should be emitted");
    let gain_index = emitted
        .find("(param gain @default 0.5 @min 0.0 @max 1.0 @mod true @mod-mode additive)")
        .expect("existing gain param should remain present");
    assert!(ext4_index < xyz_index);
    assert!(xyz_index < gain_index);
}

#[test]
fn writeback_created_node_depending_on_mod_accessor_follows_modulator_inputs() {
    let source = r#"
        (defmacro op (input depth) (* input depth))
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def op1 (op pitch 1))
        (def velocity (in 3 @name velocity))
        (def trigger (in 4 @name trigger))
        (def mod1 (in 5 @name mod1 @modulator 1))
        (def mod2 (in 6 @name mod2 @modulator 2))
        (def mod3 (in 7 @name mod3 @modulator 3))
        (def mod4 (in 8 @name mod4 @modulator 4))
        (def mod5 (in 9 @name mod5 @modulator 5))
        (def mod6 (in 10 @name mod6 @modulator 6))
        (def ext1 (in 11 @name ext1 @modulator 7))
        (def ext2 (in 12 @name ext2 @modulator 8))
        (def ext3 (in 13 @name ext3 @modulator 9))
        (def ext4 (in 14 @name ext4 @modulator 10))
        (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
        (out (* op1 velocity (mod gain)) 1 @name audio)
    "#;
    let mut state = PatcherInteractionState::default();
    let param = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param))
        .unwrap()
        .text = "param mything @default 0.5 @min 0 @max 1 @mod true @mod-mode additive".to_string();
    let mod_node = allocate_created_node(&mut state, "root", (2.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &mod_node))
        .unwrap()
        .text = "mod".to_string();
    let op = allocate_created_node(&mut state, "root", (3.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &op))
        .unwrap()
        .text = "op".to_string();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: param,
            output_index: 0,
        },
        InputPortRef {
            node_id: mod_node.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: "pitch".to_string(),
            output_index: 0,
        },
        InputPortRef {
            node_id: op.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: mod_node,
            output_index: 0,
        },
        InputPortRef {
            node_id: op,
            input_index: 1,
        },
    );

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    let ext4_index = emitted
        .find("(def ext4 (in 14 @name ext4 @modulator 10))")
        .expect("instrument modulator inputs should be present");
    let param_index = emitted
        .find("(param mything @default 0.5 @min 0.0 @max 1.0 @mod true @mod-mode additive)")
        .expect("created modulatable param should be emitted");
    let modulated_index = emitted
        .find("(def modulated1 (mod mything))")
        .expect("created mod accessor should be emitted");
    let op_index = emitted
        .find("(def op2 (op pitch modulated1))")
        .expect("created dependent macro call should be emitted");
    assert!(ext4_index < param_index);
    assert!(param_index < modulated_index);
    assert!(modulated_index < op_index);
}

#[test]
fn writeback_generated_binding_avoids_scope_name_collisions() {
    let source = r#"
        (param phasor1)
        (make-history phasor2)
        (defmacro phasor3 (sig) sig)
        (def sig (in 1))
        (out sig 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created))
        .unwrap()
        .text = "phasor".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: created.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param phasor1)\n(make-history phasor2)\n(defmacro phasor3 (sig) sig)\n(def sig (in 1))\n(def phasor4 (phasor sig))\n(out phasor4 1)"
    );
}

#[test]
fn writeback_macro_generated_binding_uses_macro_local_scope() {
    let source = r#"
        (def phasor1 (phasor 1))
        (defmacro ap (sig) sig)
    "#;
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let sig = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.outputs == vec!["sig".to_string()])
        .unwrap();
    let out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = macro_patch
        .patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "macro:ap", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:ap", &created))
        .unwrap()
        .text = "phasor".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:ap",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "macro:ap",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: created.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:ap",
        OutputPortRef {
            node_id: created,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def phasor1 (phasor 1.0))\n(defmacro ap (sig) (def phasor1 (phasor sig)) phasor1)"
    );
}

#[test]
fn writeback_shared_created_node_emits_one_generated_def_and_multiple_refs() {
    let source = r#"
        (def sig (in 1))
        (out sig 1)
        (def clipped (clip sig 0 1))
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let clip = patch.nodes.iter().find(|node| node.op == "clip").unwrap();
    let sig_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let sig_to_clip = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == clip.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created))
        .unwrap()
        .text = "phasor".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_clip),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: created.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created,
            output_index: 0,
        },
        InputPortRef {
            node_id: clip.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))\n(def phasor1 (phasor sig))\n(out phasor1 1)\n(def clipped (clip phasor1 0.0 1.0))"
    );
}

#[test]
fn writeback_cable_create_updates_destination_semantic_arg() {
    let source = r#"
        (def a (in 1))
        (def b (in 2))
        (def result (+ a 1))
    "#;
    let patch = parse(source);
    let b = patch.nodes.iter().find(|node| node.id == "b").unwrap();
    let plus = patch.nodes.iter().find(|node| node.op == "+").unwrap();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: b.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: plus.id.clone(),
            input_index: 1,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def a (in 1))\n(def b (in 2))\n(def result (+ a b))"
    );
}

#[test]
fn writeback_cable_create_uses_semantic_arg_index_with_attributes() {
    let source = r#"
        (def a (in 1))
        (def b (in 2))
        (def result (foo a @mode fast 1))
    "#;
    let patch = parse(source);
    let b = patch.nodes.iter().find(|node| node.id == "b").unwrap();
    let foo = patch.nodes.iter().find(|node| node.op == "foo").unwrap();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: b.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: foo.id.clone(),
            input_index: 1,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def a (in 1))\n(def b (in 2))\n(def result (foo a @mode fast b))"
    );
}

#[test]
fn writeback_source_cable_rewire_replaces_destination_arg_once() {
    let source = r#"
        (def a (in 1))
        (def b (in 2))
        (def result (+ a 1))
    "#;
    let patch = parse(source);
    let a = patch.nodes.iter().find(|node| node.id == "a").unwrap();
    let b = patch.nodes.iter().find(|node| node.id == "b").unwrap();
    let plus = patch.nodes.iter().find(|node| node.op == "+").unwrap();
    let a_to_plus = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == a.id && connection.to_node == plus.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(a_to_plus),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: b.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: plus.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def a (in 1))\n(def b (in 2))\n(def result (+ b 1.0))"
    );
}

#[test]
fn writeback_cable_delete_in_root_emits_missing_input_sentinel() {
    let source = r#"
        (def sig (in 1))
        (out sig 1)
    "#;
    let patch = parse(source);
    let connection = patch.connections.first().unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(connection),
        ));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))\n(out __patcher_missing_input__ 1)"
    );
}

#[test]
fn writeback_cable_delete_in_macro_emits_missing_input_sentinel() {
    let source = "(defmacro ap (sig) (out sig 1))";
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "ap")
        .unwrap();
    let connection = macro_patch.patch.connections.first().unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:ap",
            &source_connection_id(connection),
        ));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro ap (sig) (out __patcher_missing_input__ 1))"
    );
}

#[test]
fn writeback_deleting_source_backed_top_level_node_removes_form() {
    let source = r#"
        (def sig (in 1))
        (def result (phasor sig))
        (out result 1)
    "#;
    let patch = parse(source);
    let result = patch.nodes.iter().find(|node| node.id == "result").unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &result.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))\n(out result 1)"
    );
}

#[test]
fn writeback_deleting_multiple_top_level_nodes_removes_expected_forms() {
    let source = r#"
        (def sig (in 1))
        (def carrier (phasor sig))
        (def shaped (* carrier 0.5))
        (out shaped 1)
    "#;
    let patch = parse(source);
    let sig = patch.nodes.iter().find(|node| node.id == "sig").unwrap();
    let shaped = patch.nodes.iter().find(|node| node.id == "shaped").unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &sig.id));
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &shaped.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def carrier (phasor sig))\n(out shaped 1)"
    );
}

#[test]
fn writeback_deleted_top_level_node_ignores_incident_deleted_connections() {
    let source = r#"
        (def sig (in 1))
        (def result (phasor sig))
        (out result 1)
    "#;
    let patch = parse(source);
    let result = patch.nodes.iter().find(|node| node.id == "result").unwrap();
    let result_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == result.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &result.id));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(result_to_out),
        ));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))\n(out result 1)"
    );
}

#[test]
fn writeback_deleting_nested_source_node_replaces_it_with_missing_input() {
    let source = r#"
        (def sig (in 1))
        (def result (phasor (* sig 2)))
    "#;
    let patch = parse(source);
    let nested = patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &nested.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))\n(def result (phasor __patcher_missing_input__))"
    );
}

#[test]
fn writeback_deleting_nested_source_node_does_not_promote_its_input() {
    let source = "(def result (phasor (* (noise) 2)))";
    let patch = parse(source);
    let nested = patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &nested.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def result (phasor __patcher_missing_input__))"
    );
}

#[test]
fn literal_args_are_inlined_and_do_not_create_visible_ports() {
    let patch = parse(
        r#"
            (def signal (in 1 @name signal))
            (out (* signal 3) 1 @name audio)
            "#,
    );
    let multiply = patch
        .nodes
        .iter()
        .find(|node| node.op == "*")
        .expect("anonymous multiply node");

    assert_eq!(node_display_label(multiply), "* 3");

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&multiply.id).map(Vec::as_slice),
        Some(&[0][..])
    );
}

#[test]
fn inline_args_keep_placeholders_for_connected_args_before_later_literals() {
    let patch = parse(
        r#"
            (defmacro ap (sig g d) sig)
            (def signal (in 1))
            (def gain (in 2))
            (def tapped (ap signal gain 0.6))
            "#,
    );
    let ap = patch
        .nodes
        .iter()
        .find(|node| node.op == "ap")
        .expect("macro instance node");

    assert_eq!(node_display_label(ap), "ap ? 0.6");

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&ap.id).map(Vec::as_slice),
        Some(&[0, 1][..])
    );

    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    assert_eq!(input_slot_counts.get(&ap.id).copied(), Some(3));
}

#[test]
fn inline_literals_reserve_semantic_input_slots_without_rendering_ports() {
    let patch = parse(
        r#"
            (def signal (in 1))
            (def gain (in 2))
            (def scaled (* signal gain 0.00012))
            "#,
    );
    let multiply = patch
        .nodes
        .iter()
        .find(|node| node.op == "*")
        .expect("multiply node");

    assert_eq!(node_display_label(multiply), "* ? 0.00012");

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&multiply.id).map(Vec::as_slice),
        Some(&[0, 1][..])
    );

    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    assert_eq!(input_slot_counts.get(&multiply.id).copied(), Some(3));

    let rects = patch_node_rects(
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        &PatcherPanState::default(),
    );
    let multiply_rect = *rects.get(&multiply.id).expect("multiply rect");
    let slot_count = input_slot_counts[&multiply.id];
    let first = port_center(multiply_rect, 0, slot_count, true);
    let second = port_center(multiply_rect, 1, slot_count, true);
    let hidden_third = port_center(multiply_rect, 2, slot_count, true);

    assert!(
        second.0 < hidden_third.0,
        "the second semantic inlet should stay in the middle slot, leaving the inline third slot hidden"
    );

    let node_rects = patch_node_rects(
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        &PatcherPanState::default(),
    );
    let output_counts = patch_output_counts(&patch);
    let gain_connection = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == multiply.id && connection.to_input == 1)
        .expect("gain connection");
    let (_, gain_endpoint) = connection_endpoints(
        gain_connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .expect("gain endpoint");

    assert_eq!(gain_endpoint, second);
    assert!(first.0 < second.0);
}

#[test]
fn leading_numeric_constants_become_nodes_to_preserve_input_order() {
    let patch = parse(
        r#"
            (defmacro ap (sig g d) sig)
            (def signal (in 1))
            (def delay (in 2))
            (def tapped (ap signal 0.6 delay))
            "#,
    );
    let ap = patch
        .nodes
        .iter()
        .find(|node| node.op == "ap")
        .expect("macro instance node");

    let constant = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Constant && node.op == "0.6")
        .expect("numeric constant node");

    assert_eq!(node_display_label(constant), "0.6");
    assert_eq!(node_display_label(ap), "ap");

    assert!(patch.connections.iter().any(|connection| {
        connection.from_node == constant.id
            && connection.to_node == ap.id
            && connection.to_input == 1
    }));

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&ap.id).map(Vec::as_slice),
        Some(&[0, 1, 2][..])
    );

    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    assert_eq!(input_slot_counts.get(&ap.id).copied(), Some(3));
}

#[test]
fn trailing_constants_inline_reserve_hidden_semantic_input_slots() {
    let patch = parse(
        r#"
            (def pitch (in 1))
            (def phase (phasor pitch))
            (def radians (* phase twopi))
            "#,
    );
    let multiply = patch
        .nodes
        .iter()
        .find(|node| node.op == "*")
        .expect("multiply node");

    assert_eq!(node_display_label(multiply), "* twopi");

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&multiply.id).map(Vec::as_slice),
        Some(&[0][..])
    );

    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    assert_eq!(input_slot_counts.get(&multiply.id).copied(), Some(2));
}

#[test]
fn leading_constants_become_nodes_to_preserve_input_order() {
    let patch = parse(
        r#"
            (def pitch (in 1))
            (def phase (phasor pitch))
            (def radians (* twopi phase))
            "#,
    );
    let constant = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Constant && node.op == "twopi")
        .expect("twopi constant node");
    let multiply = patch
        .nodes
        .iter()
        .find(|node| node.op == "*")
        .expect("multiply node");

    assert_eq!(node_display_label(constant), "twopi");
    assert_eq!(node_display_label(multiply), "*");

    assert!(patch.connections.iter().any(|connection| {
        connection.from_node == constant.id
            && connection.to_node == multiply.id
            && connection.to_input == 0
    }));
    assert!(
        patch
            .connections
            .iter()
            .any(|connection| connection.to_node == multiply.id && connection.to_input == 1)
    );

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&multiply.id).map(Vec::as_slice),
        Some(&[0, 1][..])
    );
}

#[test]
fn standalone_constant_defs_project_as_constant_nodes() {
    let patch = parse(
        r#"
            (def radians twopi)
            (out radians 1)
            "#,
    );
    let constant = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Constant && node.op == "twopi")
        .expect("twopi constant node");

    assert_eq!(node_display_label(constant), "twopi");
    assert!(patch.connections.iter().any(|connection| {
        connection.from_node == constant.id
            && patch
                .nodes
                .iter()
                .any(|node| node.id == connection.to_node && node.kind == NodeKind::Out)
    }));
}

#[test]
fn display_labels_omit_def_names_and_show_in_out_channels() {
    let patch = parse(
        r#"
            (def signal (in 1 @name pitch))
            (def scaled (* signal 3))
            (out scaled 1 @name audio)
            "#,
    );
    let input = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let multiply = patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let output = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();

    assert_eq!(node_display_label(input), "in 1 @name pitch");
    assert_eq!(node_display_label(multiply), "* 3");
    assert_eq!(node_display_label(output), "out 1");
}

#[test]
fn macro_parameter_nodes_display_argument_names() {
    let patch = parse("(defmacro filter-bank (input cutoff gain) (* input gain))");
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "filter-bank")
        .expect("macro patch");
    let labels: Vec<String> = macro_patch
        .patch
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::In)
        .map(node_display_label)
        .collect();
    assert_eq!(
        labels,
        vec![
            "in 1 @name input".to_string(),
            "in 2 @name cutoff".to_string(),
            "in 3 @name gain".to_string()
        ]
    );
}

#[test]
fn port_tooltips_use_macro_parameter_and_output_names() {
    let patch = parse(
        "(defmacro fm-operator (carrier modulator index) (+ carrier (* modulator index)))\n\
         (def pitch (in 1 @name pitch))\n\
         (def fm1 (fm-operator pitch 0.5 1.0))",
    );
    assert_eq!(
        input_port_tooltip(
            &patch,
            &InputPortRef {
                node_id: "fm1".to_string(),
                input_index: 2,
            },
        ),
        Some("in 3: index".to_string())
    );
    assert_eq!(
        output_port_tooltip(
            &patch,
            &OutputPortRef {
                node_id: "pitch".to_string(),
                output_index: 0,
            },
        ),
        Some("out 1: pitch".to_string())
    );
}

#[test]
fn instrument_signature_modulator_inputs_are_hidden_boilerplate() {
    let patch = parse(
        r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (def mod1 (in 5 @name mod1 @modulator 1))
            (def mod2 (in 6 @name mod2 @modulator 2))
            (def mod3 (in 7 @name mod3 @modulator 3))
            (def mod4 (in 8 @name mod4 @modulator 4))
            (def mod5 (in 9 @name mod5 @modulator 5))
            (def mod6 (in 10 @name mod6 @modulator 6))
            (def ext1 (in 11 @name ext1 @modulator 7))
            (def ext2 (in 12 @name ext2 @modulator 8))
            (def ext3 (in 13 @name ext3 @modulator 9))
            (def ext4 (in 14 @name ext4 @modulator 10))
            (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
            (out (* gate (mod gain)) 1 @name audio)
            "#,
    );

    for name in ["gate", "pitch", "velocity", "trigger"] {
        assert!(
            patch.nodes.iter().any(|node| node.id == name),
            "missing visible instrument input {name}"
        );
    }
    for name in [
        "mod1", "mod2", "mod3", "mod4", "mod5", "mod6", "ext1", "ext2", "ext3", "ext4",
    ] {
        assert!(
            !patch.nodes.iter().any(|node| node.id == name),
            "boilerplate modulator input {name} should be hidden"
        );
    }
    assert!(patch.nodes.iter().any(|node| node.op == "mod"));
}

#[test]
fn writeback_preserves_integer_modulator_attribute_tokens() {
    let source = r#"
        (def gate (in 1 @name gate))
        (def mod1 (in 5 @name mod1 @modulator 1))
        (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
        (out (* gate (mod gain)) 1 @name audio)
    "#;

    let emitted = emit_patch_writeback(
        source,
        PatcherIntent::Instrument,
        &PatcherInteractionState::default(),
    )
    .unwrap();

    assert!(emitted.contains("@modulator 1)"), "{emitted}");
    assert!(!emitted.contains("@modulator 1.0"), "{emitted}");
    assert!(
        emitted.contains("(def gate (in 1 @name gate))"),
        "{emitted}"
    );
    assert!(
        emitted.contains("(out (* gate (mod gain)) 1 @name audio)"),
        "{emitted}"
    );
    assert!(!emitted.contains("(in 1.0"), "{emitted}");
    assert!(!emitted.contains(" 1.0 @name audio"), "{emitted}");
}

#[test]
fn effect_patcher_does_not_hide_matching_modulator_input_forms() {
    let root_patch = parse_patch_source(
        "(def mod1 (in 5 @name mod1 @modulator 1))\n(out mod1 1)",
        PatcherIntent::Effect,
    )
    .unwrap();

    assert!(root_patch.nodes.iter().any(|node| node.id == "mod1"));
}

#[test]
fn interaction_positions_override_auto_layout_positions() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let pitch = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
    assert_ne!(pitch.position, (22.0, 7.0));

    let mut state = PatcherInteractionState::default();
    set_node_edit_position(
        &mut state,
        "root",
        pitch,
        (22.0, 7.0),
        node_display_label(pitch),
    );
    let patch = patch_with_interaction_state(patch, &state, "root");
    let pitch = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
    assert_eq!(pitch.position, (22.0, 7.0));
}

#[test]
fn patcher_hit_testing_uses_node_rects_after_pan() {
    let patch = parse("(def pitch (in 1 @name pitch))");
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rect = *patch_node_rects(&patch, rect, &pan).get("pitch").unwrap();
    let hit = hit_patcher_node(
        &patch,
        rect,
        &pan,
        node_rect.col + node_rect.width * 0.5,
        node_rect.row + node_rect.height * 0.5,
    );
    assert_eq!(hit.as_deref(), Some("pitch"));
}

#[test]
fn patcher_output_port_hit_testing_uses_rendered_port_positions() {
    let patch = parse("(def pitch (in 1 @name pitch))");
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rect = *patch_node_rects(&patch, rect, &pan).get("pitch").unwrap();
    let output_counts = patch_output_counts(&patch);
    let center = port_center(node_rect, 0, output_counts["pitch"], false);

    let hit = hit_patcher_output_port(
        &patch,
        rect,
        &pan,
        &output_counts,
        center.0,
        center.1,
        10.0,
        20.0,
    );
    assert_eq!(
        hit,
        Some(OutputPortRef {
            node_id: "pitch".to_string(),
            output_index: 0,
        })
    );
}

#[test]
fn patcher_output_port_hit_testing_matches_rendered_circle() {
    let patch = parse("(def pitch (in 1 @name pitch))");
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rect = *patch_node_rects(&patch, rect, &pan).get("pitch").unwrap();
    let output_counts = patch_output_counts(&patch);
    let center = port_center(node_rect, 0, output_counts["pitch"], false);
    let radius_rows = (PORT_OUTER_DIAMETER_PX * 0.5) / 20.0;

    let inside_rendered_circle = hit_patcher_output_port(
        &patch,
        rect,
        &pan,
        &output_counts,
        center.0,
        center.1 - radius_rows + 0.01,
        10.0,
        20.0,
    );
    assert_eq!(
        inside_rendered_circle,
        Some(OutputPortRef {
            node_id: "pitch".to_string(),
            output_index: 0,
        })
    );

    let inside_node_body_but_outside_port_circle = hit_patcher_output_port(
        &patch,
        rect,
        &pan,
        &output_counts,
        center.0,
        center.1 - radius_rows - 0.01,
        10.0,
        20.0,
    );
    assert_eq!(inside_node_body_but_outside_port_circle, None);
}

#[test]
fn nearest_patcher_input_port_respects_max_distance() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let phasor_rect = *node_rects
        .iter()
        .find_map(|(node_id, rect)| {
            patch
                .nodes
                .iter()
                .find(|node| node.id == *node_id && node.op == "phasor")
                .map(|_| rect)
        })
        .unwrap();
    let phasor_id = patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap()
        .id
        .clone();
    let center = port_center(phasor_rect, 0, input_slot_counts[&phasor_id], true);
    let source = OutputPortRef {
        node_id: "pitch".to_string(),
        output_index: 0,
    };

    let near = nearest_patcher_input_port(
        &patch,
        rect,
        &pan,
        &input_indices,
        &input_slot_counts,
        &source,
        center.0 + 0.2,
        center.1 + 0.2,
    );
    assert_eq!(
        near,
        Some(InputPortRef {
            node_id: phasor_id.clone(),
            input_index: 0,
        })
    );

    let far = nearest_patcher_input_port(
        &patch,
        rect,
        &pan,
        &input_indices,
        &input_slot_counts,
        &source,
        center.0 + 20.0,
        center.1 + 20.0,
    );
    assert_eq!(far, None);
}

#[test]
fn created_connections_are_applied_to_working_patch() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor))
            "#,
    );
    let phasor_id = patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap()
        .id
        .clone();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: "pitch".to_string(),
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor_id.clone(),
            input_index: 0,
        },
    );

    let patch = patch_with_interaction_state(patch, &state, "root");
    assert!(patch.connections.iter().any(|connection| {
        connection.from_node == "pitch"
            && connection.from_output == 0
            && connection.to_node == phasor_id
            && connection.to_input == 0
            && connection.kind == ConnectionKind::Forward
    }));
}

#[test]
fn patcher_cable_hit_testing_uses_rendered_curve() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let connection = patch.connections.first().expect("source connection");
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let (start, end) = connection_endpoints(
        connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .expect("rendered connection endpoints");
    let curve = super::super::cable::cable_curve(start, end);
    let midpoint = super::super::cable::cubic_bezier_point(curve, 0.5);

    assert_eq!(
        hit_patcher_cable(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            midpoint.0,
            midpoint.1,
        )
        .as_deref(),
        Some(source_connection_id(connection).as_str())
    );
}

#[test]
fn segmented_cable_hit_testing_uses_orthogonal_path() {
    let mut patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let to_node = patch.connections.first().unwrap().to_node.clone();
    patch
        .nodes
        .iter_mut()
        .find(|node| node.id == to_node)
        .unwrap()
        .position
        .0 += 12.0;
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let (start, end) = connection_endpoints(
        patch.connections.first().unwrap(),
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    let origin = patcher_origin(rect, &pan);
    let zoom = patcher_zoom(&pan);
    let rendered_segment_row = (start.1 + end.1) * 0.5;
    let segment_row = (rendered_segment_row - origin.1) / zoom;
    patch.connections[0].segment = Some(CableSegmentInfo {
        is_segmented: true,
        segment_row,
    });
    let horizontal_midpoint = ((start.0 + end.0) * 0.5, rendered_segment_row);

    assert_eq!(
        hit_patcher_cable(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            horizontal_midpoint.0,
            horizontal_midpoint.1,
        )
        .as_deref(),
        Some(source_connection_id(patch.connections.first().unwrap()).as_str())
    );
}

#[test]
fn segmented_horizontal_segment_hit_is_used_for_dragging() {
    let mut patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let to_node = patch.connections.first().unwrap().to_node.clone();
    patch
        .nodes
        .iter_mut()
        .find(|node| node.id == to_node)
        .unwrap()
        .position
        .0 += 12.0;
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let (start, end) = connection_endpoints(
        patch.connections.first().unwrap(),
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    let origin = patcher_origin(rect, &pan);
    let zoom = patcher_zoom(&pan);
    let rendered_segment_row = (start.1 + end.1) * 0.5;
    let segment_row = (rendered_segment_row - origin.1) / zoom;
    patch.connections[0].segment = Some(CableSegmentInfo {
        is_segmented: true,
        segment_row,
    });

    assert_eq!(
        hit_patcher_segmented_cable_horizontal_segment(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            (start.0 + end.0) * 0.5,
            rendered_segment_row,
        )
        .as_deref(),
        Some(source_connection_id(patch.connections.first().unwrap()).as_str())
    );
}

#[test]
fn segment_row_drag_clamps_normal_and_wraparound_cases() {
    let normal = super::super::cable::segment_row_for_drag(
        (10.0, 10.0),
        (20.0, 20.0),
        30.0,
        SEGMENTED_CABLE_DRAG_PADDING_CELLS,
        SEGMENTED_CABLE_DRAG_EXTRA_RANGE_CELLS,
    );
    assert!(normal < 20.0, "{normal}");

    let wrap = super::super::cable::segment_row_for_drag(
        (10.0, 20.0),
        (20.0, 10.0),
        40.0,
        SEGMENTED_CABLE_DRAG_PADDING_CELLS,
        SEGMENTED_CABLE_DRAG_EXTRA_RANGE_CELLS,
    );
    assert!(wrap > 20.0, "{wrap}");
}

#[test]
fn super_y_toggles_selected_cable_segmentation() {
    let source = r#"
        (def pitch (in 1 @name pitch))
        (def sig (phasor pitch))
    "#;
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-segment-toggle-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let patch = parse(source);
    let selected_cable = source_connection_id(patch.connections.first().unwrap());
    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 778_899,
        stable_widget_id: Some(778_899),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(
        key,
        PatcherInteractionState {
            selected_cable: Some(selected_cable.clone()),
            ..Default::default()
        },
    );

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char('y'),
                    modifiers: KeyModifiers::SUPER,
                },
            )
            .is_none()
    );

    let state = get_patcher_interaction_state(key);
    let patch = patch_with_interaction_state(patch, &state, "root");
    let segment = patch
        .connections
        .iter()
        .find(|connection| source_connection_id(connection) == selected_cable)
        .and_then(|connection| connection.segment)
        .unwrap();
    assert!(segment.is_segmented);
}

#[test]
#[cfg(target_os = "macos")]
fn segmented_cable_render_row_tracks_pan_origin_once() {
    let source = r#"
        (def pitch (in 1 @name pitch))
        (def sig (phasor pitch))
    "#;
    let mut patch = parse(source);
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let pan_for_layout = PatcherPanState::default();
    let connection = patch.connections.first().unwrap().clone();
    patch
        .nodes
        .iter_mut()
        .find(|node| node.id == connection.to_node)
        .unwrap()
        .position
        .0 += 12.0;
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let node_rects = patch_node_rects(&patch, rect, &pan_for_layout);
    let (start, end) = connection_endpoints(
        &connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    assert!(super::super::cable::should_render_segmented_cable(
        start, end
    ));
    let origin = patcher_origin(rect, &pan_for_layout);
    let stored_segment_row = ((start.1 + end.1) * 0.5) - origin.1;
    patch.connections[0].segment = Some(CableSegmentInfo {
        is_segmented: true,
        segment_row: stored_segment_row,
    });
    let state = PatcherInteractionState::default();

    let mut pan = PatcherPanState {
        viewport_width: rect.width,
        viewport_height: rect.height,
        content_width: 200.0,
        content_height: 200.0,
        ..Default::default()
    };
    let first_origin = patcher_origin(rect, &pan);
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        rect,
        WidgetViewport {
            vp_w: 800.0,
            vp_h: 600.0,
            cell_w: 10.0,
            cell_h: 20.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 30.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &pan,
        &state,
    );
    let first_segment_row = prims
        .iter()
        .find_map(|prim| match prim {
            MetalPrimitive::PatchCable(cable) if cable.is_segmented => Some(cable.segment_row),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        first_segment_row,
        first_origin.1 + stored_segment_row * patcher_zoom(&pan)
    );

    pan.offset_y = 12.0;
    let second_origin = patcher_origin(rect, &pan);
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        rect,
        WidgetViewport {
            vp_w: 800.0,
            vp_h: 600.0,
            cell_w: 10.0,
            cell_h: 20.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 30.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &pan,
        &state,
    );
    let second_segment_row = prims
        .iter()
        .find_map(|prim| match prim {
            MetalPrimitive::PatchCable(cable) if cable.is_segmented => Some(cable.segment_row),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        second_segment_row,
        second_origin.1 + stored_segment_row * patcher_zoom(&pan)
    );
    assert_eq!(
        second_segment_row - first_segment_row,
        second_origin.1 - first_origin.1
    );
}

#[test]
#[cfg(target_os = "macos")]
fn segmented_cable_rendering_collapses_aligned_ports_to_vertical_curve() {
    let source = r#"
        (def pitch (in 1 @name pitch))
        (def sig (phasor pitch))
    "#;
    let mut patch = parse(source);
    let pitch_x = patch
        .nodes
        .iter()
        .find(|node| node.id == "pitch")
        .unwrap()
        .position
        .0;
    patch
        .nodes
        .iter_mut()
        .find(|node| node.id == "sig")
        .unwrap()
        .position
        .0 = pitch_x;
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let (start, end) = connection_endpoints(
        patch.connections.first().unwrap(),
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    assert!(!super::super::cable::should_render_segmented_cable(
        start, end
    ));
    let origin = patcher_origin(rect, &pan);
    patch.connections[0].segment = Some(CableSegmentInfo {
        is_segmented: true,
        segment_row: ((start.1 + end.1) * 0.5) - origin.1,
    });

    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        rect,
        WidgetViewport {
            vp_w: 800.0,
            vp_h: 600.0,
            cell_w: 10.0,
            cell_h: 20.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 30.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &pan,
        &PatcherInteractionState::default(),
    );

    let cable = prims
        .iter()
        .find_map(|prim| match prim {
            MetalPrimitive::PatchCable(cable) => Some(cable),
            _ => None,
        })
        .unwrap();
    assert!(!cable.is_segmented);
    assert_eq!(cable.start[0], cable.end[0]);
    assert_eq!(cable.control1[0], cable.start[0]);
    assert_eq!(cable.control2[0], cable.end[0]);
}

#[test]
fn selected_source_cable_delete_marks_connection_deleted() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let connection_id = source_connection_id(patch.connections.first().unwrap());
    let mut state = PatcherInteractionState {
        selected_cable: Some(connection_id.clone()),
        ..Default::default()
    };

    delete_connection_edit_or_mark_deleted(&mut state, "root", &connection_id);

    assert_eq!(state.selected_cable, None);
    assert!(
        state
            .edit_state
            .deleted_connections
            .contains(&connection_edit_key("root", &connection_id))
    );
    let patch = patch_with_interaction_state(patch, &state, "root");
    assert!(patch.connections.is_empty(), "{:#?}", patch.connections);
}

#[test]
fn selected_created_cable_delete_removes_connection_edit() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor))
            "#,
    );
    let phasor_id = patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap()
        .id
        .clone();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: "pitch".to_string(),
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor_id,
            input_index: 0,
        },
    );
    let patch = patch_with_interaction_state(patch, &state, "root");
    let connection_id = source_connection_id(patch.connections.first().unwrap());
    state.selected_cable = Some(connection_id.clone());

    delete_connection_edit_or_mark_deleted(&mut state, "root", &connection_id);

    assert_eq!(state.selected_cable, None);
    assert!(state.edit_state.connections.is_empty());
    assert!(state.edit_state.deleted_connections.is_empty());
}

#[test]
fn selected_source_node_delete_hides_node_and_incident_connections() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let mut state = PatcherInteractionState::default();
    state.selected_nodes.insert("pitch".to_string());

    assert!(delete_selected_nodes(&mut state, "root"));

    assert!(state.selected_nodes.is_empty());
    assert!(
        state
            .edit_state
            .deleted_nodes
            .contains(&node_edit_key("root", "pitch"))
    );
    let patch = patch_with_interaction_state(patch, &state, "root");
    assert!(!patch.nodes.iter().any(|node| node.id == "pitch"));
    assert!(
        patch
            .connections
            .iter()
            .all(|connection| connection.from_node != "pitch" && connection.to_node != "pitch"),
        "{:#?}",
        patch.connections
    );
}

#[test]
fn selected_created_node_delete_removes_node_and_created_connections() {
    let patch = parse(
        r#"
            (def sig (phasor))
            "#,
    );
    let phasor_id = patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap()
        .id
        .clone();
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (0.0, 0.0));
    if let Some(edit) = state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
    {
        edit.text = "param freq".to_string();
    }
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created_id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor_id,
            input_index: 0,
        },
    );
    state.selected_nodes.insert(created_id.clone());

    assert!(delete_selected_nodes(&mut state, "root"));

    assert!(
        !state
            .edit_state
            .nodes
            .contains_key(&node_edit_key("root", &created_id))
    );
    assert!(state.edit_state.deleted_nodes.is_empty());
    assert!(state.edit_state.connections.is_empty());
    let patch = patch_with_interaction_state(patch, &state, "root");
    assert!(!patch.nodes.iter().any(|node| node.id == created_id));
    assert!(patch.connections.is_empty());
}

#[test]
fn selected_cable_handles_are_hit_near_rendered_edit_points() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let connection = patch.connections.first().expect("source connection");
    let connection_id = source_connection_id(connection);
    let pan = PatcherPanState::default();
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let (start, end) = connection_endpoints(
        connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .expect("rendered connection endpoints");
    let (from_handle, to_handle) =
        connection_cable_edit_points(connection, start, end, patcher_zoom(&pan));

    assert_eq!(
        hit_patcher_cable_handle(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            Some(&connection_id),
            from_handle.0,
            from_handle.1,
        ),
        Some((connection_id.clone(), CableEndpoint::From))
    );
    assert_eq!(
        hit_patcher_cable_handle(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            Some(&connection_id),
            to_handle.0,
            to_handle.1,
        ),
        Some((connection_id, CableEndpoint::To))
    );
}

#[test]
fn dragging_selected_cable_endpoint_reconnects_and_keeps_cable_selected() {
    let source = r#"
            (def pitch (in 1 @name pitch))
            (def gate (in 2 @name gate))
            (def sig (phasor pitch))
            "#;
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-cable-endpoint-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let original_connection = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == "pitch")
        .cloned()
        .unwrap();
    let original_connection_id = source_connection_id(&original_connection);
    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 445_566,
        stable_widget_id: Some(445_566),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(
        key,
        PatcherInteractionState {
            selected_cable: Some(original_connection_id.clone()),
            ..Default::default()
        },
    );
    let pan = PatcherPanState::default();
    let node_rects = patch_node_rects(&root_patch, node.rect, &pan);
    let input_indices = patch_input_indices(&root_patch);
    let input_slot_counts = patch_input_slot_counts(&root_patch, &input_indices);
    let output_counts = patch_output_counts(&root_patch);
    let (start, end) = connection_endpoints(
        &original_connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    let (from_handle, _) =
        connection_cable_edit_points(&original_connection, start, end, patcher_zoom(&pan));
    let gate_rect = node_rects.get("gate").unwrap();
    let gate_output = port_center(*gate_rect, 0, output_counts["gate"], false);

    handle_patcher_pointer_down(
        &node,
        from_handle.0,
        from_handle.1,
        KeyModifiers::empty(),
        10.0,
        20.0,
    );
    handle_patcher_pointer_drag(&node, gate_output.0, gate_output.1);
    handle_patcher_pointer_up(&node, gate_output.0, gate_output.1);

    let state = get_patcher_interaction_state(key);
    let edited_patch = patch_with_interaction_state(root_patch, &state, "root");
    let new_connection_id = connection_id_from_ports(
        &OutputPortRef {
            node_id: "gate".to_string(),
            output_index: 0,
        },
        &InputPortRef {
            node_id: original_connection.to_node.clone(),
            input_index: original_connection.to_input,
        },
    );
    assert_eq!(
        state.selected_cable.as_deref(),
        Some(new_connection_id.as_str())
    );
    assert!(!edited_patch.connections.iter().any(|connection| {
        connection.from_node == "pitch" && connection.to_node == original_connection.to_node
    }));
    assert!(edited_patch.connections.iter().any(|connection| {
        connection.from_node == "gate" && connection.to_node == original_connection.to_node
    }));

    let _ = std::fs::remove_file(path);
}

#[test]
fn pan_state_allows_overscroll_and_clamps_to_finite_canvas_bounds() {
    let mut state = PatcherPanState {
        offset_x: 100.0,
        offset_y: 100.0,
        zoom: DEFAULT_ZOOM,
        content_width: 50.0,
        content_height: 30.0,
        viewport_width: 20.0,
        viewport_height: 10.0,
    };
    clamp_patcher_pan_state(&mut state);
    assert_eq!(state.offset_x, 78.0);
    assert_eq!(state.offset_y, 68.0);

    state.offset_x = -200.0;
    state.offset_y = -200.0;
    clamp_patcher_pan_state(&mut state);
    assert_eq!(state.offset_x, -48.0);
    assert_eq!(state.offset_y, -48.0);
}

#[test]
fn default_patcher_zoom_is_thirty_percent_zoomed_out() {
    assert!((PatcherPanState::default().zoom - 0.7).abs() < f32::EPSILON);
}

#[test]
fn patcher_magnify_clamps_zoom() {
    let node = LayoutNode {
        widget_id: 987_656,
        stable_widget_id: Some(987_656),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 20.0,
            height: 10.0,
        },
        props: HashMap::new(),
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);

    for _ in 0..12 {
        PATCHER_WIDGET.magnify_event(&node, 10.0, 5.0, 1.0);
    }
    assert_eq!(get_patcher_pan_state(key).zoom, MAX_ZOOM);

    for _ in 0..12 {
        PATCHER_WIDGET.magnify_event(&node, 10.0, 5.0, -1.0);
    }
    assert_eq!(get_patcher_pan_state(key).zoom, MIN_ZOOM);
}

#[test]
fn patcher_magnify_preserves_pointer_anchor_model_position() {
    let node = LayoutNode {
        widget_id: 987_657,
        stable_widget_id: Some(987_657),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props: HashMap::new(),
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    let before = screen_to_model(node.rect, &get_patcher_pan_state(key), (31.0, 14.0));

    PATCHER_WIDGET.magnify_event(&node, 31.0, 14.0, 0.5);

    let after = screen_to_model(node.rect, &get_patcher_pan_state(key), (31.0, 14.0));
    assert!((before.0 - after.0).abs() < 0.001, "{before:?} {after:?}");
    assert!((before.1 - after.1).abs() < 0.001, "{before:?} {after:?}");
}

#[test]
fn patcher_hit_testing_uses_zoomed_node_rects() {
    let patch = parse("(def pitch (in 1 @name pitch))");
    let pan = PatcherPanState {
        zoom: 1.6,
        ..Default::default()
    };
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let node_rect = *patch_node_rects(&patch, rect, &pan).get("pitch").unwrap();
    let hit = hit_patcher_node(
        &patch,
        rect,
        &pan,
        node_rect.col + node_rect.width * 0.5,
        node_rect.row + node_rect.height * 0.5,
    );
    assert_eq!(hit.as_deref(), Some("pitch"));
}

#[test]
fn patcher_node_drag_converts_screen_delta_to_model_delta_after_zoom() {
    let path = temp_patcher_source_path("patcher-zoom-drag");
    fs::write(&path, "(def pitch (in 1 @name pitch))\n").unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    set_patcher_pan_state(
        key,
        PatcherPanState {
            zoom: 2.0,
            ..Default::default()
        },
    );
    let root_patch = load_patch_from_props(&node.props).unwrap().1;
    let start_position = root_patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "pitch")
        .unwrap()
        .position;
    let pan = get_patcher_pan_state(key);
    let rect = *patch_node_rects(&root_patch, node.rect, &pan)
        .get("pitch")
        .unwrap();
    let start = (rect.col + rect.width * 0.5, rect.row + rect.height * 0.5);

    handle_patcher_pointer_down(&node, start.0, start.1, KeyModifiers::empty(), 10.0, 20.0);
    handle_patcher_pointer_drag(&node, start.0 + 4.0, start.1 + 2.0);

    let state = get_patcher_interaction_state(key);
    let edited = patch_with_interaction_state(root_patch, &state, "root");
    let moved = edited
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "pitch")
        .unwrap()
        .position;
    assert!((moved.0 - (start_position.0 + 2.0)).abs() < 0.001);
    assert!((moved.1 - (start_position.1 + 1.0)).abs() < 0.001);

    let _ = fs::remove_file(path);
}

#[test]
fn patcher_node_drag_release_reports_layout_change_for_host_layout_payload() {
    let path = temp_patcher_dsp_path("patcher-drag-release-layout-change");
    fs::write(&path, "(def pitch (in 1 @name pitch))\n").unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let root_patch = load_patch_from_props(&node.props).unwrap().1;
    let sidecar_path = sidecar::sidecar_path_for_source(&path);
    let original_sidecar: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    let pan = get_patcher_pan_state(key);
    let rect = *patch_node_rects(&root_patch, node.rect, &pan)
        .get("pitch")
        .unwrap();
    let start = (rect.col + rect.width * 0.5, rect.row + rect.height * 0.5);

    handle_patcher_pointer_down(&node, start.0, start.1, KeyModifiers::empty(), 10.0, 20.0);
    handle_patcher_pointer_drag(&node, start.0 + 3.0, start.1 + 2.0);
    assert_eq!(
        handle_patcher_pointer_up(&node, start.0 + 3.0, start.1 + 2.0),
        PatcherChangeKind::Layout,
        "layout-only drag release should notify the host without implying a semantic change"
    );
    let edited_after_drag = patch_with_interaction_state(
        root_patch.clone(),
        &get_patcher_interaction_state(key),
        "root",
    );
    let expected_position = edited_after_drag
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "pitch")
        .unwrap()
        .position;

    let payload = patcher_layout_payload(&node);
    let Value::Map(map) = payload else {
        panic!("expected layout payload map");
    };
    assert!(matches!(
        map.get("status").map(|value| value.borrow().clone()),
        Some(Value::Keyword(status)) if status == "layout"
    ));
    assert!(
        map.get("layout").is_some(),
        "layout-only payload should include the current sidecar"
    );
    assert!(
        map.get("source").is_none(),
        "layout-only payload must not include source, because source triggers recompilation"
    );
    let layout = match map.get("layout").map(|value| value.borrow().clone()) {
        Some(Value::String(layout)) => layout,
        other => panic!("expected layout string, got {other:?}"),
    };
    let payload_sidecar: serde_json::Value = serde_json::from_str(&layout).unwrap();
    let payload_pitch_position = &payload_sidecar["root"]["nodes"]["pitch"];
    assert!(
        (payload_pitch_position["x"].as_f64().unwrap() - expected_position.0 as f64).abs() < 0.0001
            && (payload_pitch_position["y"].as_f64().unwrap() - expected_position.1 as f64).abs()
                < 0.0001,
        "layout payload should include the moved position: payload={payload_pitch_position:?}, expected={expected_position:?}"
    );

    let disk_sidecar: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    assert_eq!(
        disk_sidecar, original_sidecar,
        "layout-only drag release must not persist to disk before Save/Finalize"
    );

    fs::write(&sidecar_path, layout).unwrap();
    set_patcher_interaction_state(key, PatcherInteractionState::default());
    let reloaded = load_patch_from_props(&node.props).unwrap().1;
    let pitch = reloaded
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "pitch")
        .unwrap();
    assert_eq!(pitch.position, expected_position);
}

#[test]
fn patcher_double_click_create_uses_model_position_after_zoom() {
    let path = temp_patcher_source_path("patcher-zoom-create");
    fs::write(&path, "\n").unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let pan = PatcherPanState {
        zoom: 2.0,
        ..Default::default()
    };
    set_patcher_pan_state(key, pan.clone());
    let local = (70.0, 50.0);
    let expected = screen_to_model(node.rect, &pan, local);

    assert!(handle_patcher_double_click(&node, local.0, local.1));

    let state = get_patcher_interaction_state(key);
    let created = state
        .edit_state
        .nodes
        .values()
        .find(|edit| matches!(edit.origin, PatcherNodeOrigin::Created { .. }))
        .expect("created node edit");
    assert!((created.position.0 - expected.0).abs() < 0.001);
    assert!((created.position.1 - expected.1).abs() < 0.001);

    let _ = fs::remove_file(path);
}

#[test]
fn touchpad_horizontal_pan_matches_canvas_drag_direction() {
    let node = LayoutNode {
        widget_id: 987_655,
        stable_widget_id: Some(987_655),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 20.0,
            height: 10.0,
        },
        props: HashMap::new(),
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);

    PATCHER_WIDGET.scroll_gesture_event(&node, 10.0, 5.0, 100.0, 0.0);

    let state = get_patcher_pan_state(key);
    assert!(
        state.offset_x < 0.0,
        "positive horizontal gesture delta should move canvas right, got {}",
        state.offset_x
    );
}

#[test]
fn defmacro_becomes_read_only_subpatch() {
    let patch = parse(
        r#"
            (defmacro ap (x)
              (def y (allpass x 100 0.5))
              y)
            (def z (ap input))
            "#,
    );
    assert_eq!(patch.macros.len(), 1);
    assert_eq!(patch.macros[0].name, "ap");
    let macro_patch = &patch.macros[0].patch;
    assert!(
        macro_patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::In && node_display_label(node) == "in 1"),
        "{:#?}",
        macro_patch.nodes
    );
    assert!(
        macro_patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::Out && node_display_label(node) == "out 1"),
        "{:#?}",
        macro_patch.nodes
    );
    assert!(
        patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::MacroInstance)
    );
    assert!(
        !patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::MacroDefinition),
        "{:#?}",
        patch.nodes
    );
}

#[test]
fn macro_body_can_reference_another_macro_as_drillable_node() {
    let patch = parse(
        r#"
            (defmacro tone (x)
              (phasor x))
            (defmacro shaped (x)
              (tone x))
            (def z (shaped input))
            "#,
    );
    let shaped = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "shaped")
        .expect("shaped macro patch");
    let nested = shaped
        .patch
        .nodes
        .iter()
        .find(|node| node.op == "tone")
        .expect("nested tone macro node");

    assert_eq!(nested.kind, NodeKind::MacroInstance);
    assert_eq!(nested.diagnostic, None);
}

#[test]
fn double_clicking_macro_instance_edits_text_and_breadcrumb_returns_to_root() {
    let source = r#"
            (defmacro ap (x)
              (def y (allpass x 100 0.5)))
            (def z (ap input))
        "#;
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-macro-nav-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let macro_node = root_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance)
        .unwrap();

    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 112_233,
        stable_widget_id: Some(112_233),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(key, PatcherInteractionState::default());

    let rects = patch_node_rects(&root_patch, node.rect, &PatcherPanState::default());
    let macro_rect = rects.get(&macro_node.id).unwrap();
    assert!(handle_patcher_double_click(
        &node,
        macro_rect.col + macro_rect.width * 0.5,
        macro_rect.row + macro_rect.height * 0.5
    ));
    let state = get_patcher_interaction_state(key);
    assert_eq!(state.active_macro, None);
    assert_eq!(
        state.text_edit.as_ref().map(|edit| edit.node_id.as_str()),
        Some(macro_node.id.as_str())
    );

    let mut state = get_patcher_interaction_state(key);
    state.text_edit = None;
    state.selected_nodes.insert(macro_node.id.clone());
    set_patcher_interaction_state(key, state);
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert_eq!(
        get_patcher_interaction_state(key).active_macro.as_deref(),
        Some("ap")
    );

    let mut state = get_patcher_interaction_state(key);
    state.active_macro = Some("ap".to_string());
    set_patcher_interaction_state(key, state);
    handle_patcher_pointer_moved(&node, 1.2, 0.8, 1.0, 1.0);
    assert!(get_patcher_interaction_state(key).hover_back_button);

    handle_patcher_pointer_down(&node, 1.2, 0.8, KeyModifiers::empty(), 10.0, 20.0);
    assert_eq!(get_patcher_interaction_state(key).active_macro, None);

    let _ = std::fs::remove_file(path);
}

#[test]
fn enter_on_macro_instance_inside_macro_opens_nested_macro() {
    let source = r#"
            (defmacro tone (x)
              (phasor x))
            (defmacro shaped (x)
              (tone x))
            (def z (shaped input))
        "#;
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-nested-macro-nav-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let shaped = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "shaped")
        .expect("shaped macro patch");
    let nested = shaped
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance && node.op == "tone")
        .expect("nested tone macro instance");

    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 112_244,
        stable_widget_id: Some(112_244),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState {
        active_macro: Some("shaped".to_string()),
        ..PatcherInteractionState::default()
    };
    state.selected_nodes.insert(nested.id.clone());
    set_patcher_interaction_state(key, state);

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert_eq!(
        get_patcher_interaction_state(key).active_macro.as_deref(),
        Some("tone")
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn moving_macro_instance_inside_macro_keeps_nested_macro_classification() {
    let root_patch = parse(
        r#"
            (defmacro tone (x)
              (phasor x))
            (defmacro shaped (x)
              (tone x))
            (def z (shaped input))
        "#,
    );
    let mut state = PatcherInteractionState {
        active_macro: Some("shaped".to_string()),
        ..PatcherInteractionState::default()
    };
    let active_patch = active_patcher_patch(&root_patch, &state);
    let nested = active_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance && node.op == "tone")
        .expect("nested tone macro instance");

    set_node_edit_position(
        &mut state,
        "macro:shaped",
        nested,
        (nested.position.0 + 1.0, nested.position.1 + 1.0),
        node_display_label(nested),
    );

    let active_patch = active_patcher_patch(&root_patch, &state);
    let edited_patch = patch_with_interaction_state(active_patch, &state, "macro:shaped");
    let edited = edited_patch
        .nodes
        .iter()
        .find(|node| node.id == nested.id)
        .expect("edited nested tone node");

    assert_eq!(edited.kind, NodeKind::MacroInstance);
    assert_eq!(edited.diagnostic, None);
}

#[test]
fn double_clicking_background_creates_editable_draft_node() {
    let source = "(def pitch (in 1))";
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-draft-node-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 223_344,
        stable_widget_id: Some(223_344),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(key, PatcherInteractionState::default());

    assert!(handle_patcher_double_click(&node, 40.0, 20.0));
    assert!(get_patcher_interaction_state(key).text_edit.is_some());

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char('p'),
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char('h'),
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char(' '),
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert_eq!(
        get_patcher_interaction_state(key)
            .text_edit
            .as_ref()
            .map(|edit| edit.text.as_str()),
        Some("ph ")
    );
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );

    let state = get_patcher_interaction_state(key);
    assert!(state.text_edit.is_none());
    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    assert_eq!(patch.nodes.len(), 1);
    assert_eq!(node_display_label(&patch.nodes[0]), "ph");
    assert!(matches!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", &patch.nodes[0].id))
            .map(|edit| &edit.origin),
        Some(PatcherNodeOrigin::Created { .. })
    ));

    let _ = std::fs::remove_file(path);
}

#[test]
fn committed_editor_nodes_project_ports_from_operator_metadata() {
    let macro_arities = HashMap::new();
    let phasor = node_from_editor_text("draft", "phasor", (0.0, 0.0), &macro_arities, false);
    assert_eq!(node_display_label(&phasor), "phasor");
    assert_eq!(phasor.outputs.len(), 1);

    let patch = Patch {
        nodes: vec![phasor],
        connections: Vec::new(),
        macros: Vec::new(),
        diagnostics: Vec::new(),
    };
    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get("draft").map(Vec::as_slice),
        Some(&[0, 1][..])
    );

    let multiply = node_from_editor_text("mul", "* 3", (0.0, 0.0), &macro_arities, false);
    assert_eq!(node_display_label(&multiply), "* 3");
    let patch = Patch {
        nodes: vec![multiply],
        connections: Vec::new(),
        macros: Vec::new(),
        diagnostics: Vec::new(),
    };
    let input_indices = patch_input_indices(&patch);
    assert_eq!(input_indices.get("mul").map(Vec::as_slice), Some(&[0][..]));
    let slot_counts = patch_input_slot_counts(&patch, &input_indices);
    assert_eq!(slot_counts.get("mul").copied(), Some(2));

    let history = node_from_editor_text("hist", "history", (0.0, 0.0), &macro_arities, false);
    assert_eq!(history.kind, NodeKind::History);
    assert_eq!(node_display_label(&history), "history");
    assert_eq!(history.diagnostic, None);
    assert_eq!(history.outputs.len(), 1);

    let constant = node_from_editor_text("const", "twopi", (0.0, 0.0), &macro_arities, false);
    assert_eq!(constant.kind, NodeKind::Constant);
    assert_eq!(node_display_label(&constant), "twopi");
    assert_eq!(constant.diagnostic, None);
    assert_eq!(constant.args.len(), 0);
    assert_eq!(constant.outputs.len(), 1);

    let number = node_from_editor_text("num", "3", (0.0, 0.0), &macro_arities, false);
    assert_eq!(number.kind, NodeKind::Constant);
    assert_eq!(node_display_label(&number), "3");
    assert_eq!(number.diagnostic, None);
    assert_eq!(number.args.len(), 0);
    assert_eq!(number.outputs.len(), 1);

    let patch = Patch {
        nodes: vec![history],
        connections: Vec::new(),
        macros: Vec::new(),
        diagnostics: Vec::new(),
    };
    let input_indices = patch_input_indices(&patch);
    assert_eq!(input_indices.get("hist").map(Vec::as_slice), Some(&[0][..]));
}

#[test]
fn actively_edited_created_nodes_suppress_unknown_operator_diagnostics() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (0.0, 0.0));
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id.clone(),
        text: "p".to_string(),
        original_text: String::new(),
        state: TextInputState {
            cursor_pos: 1,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    assert_eq!(patch.nodes.len(), 1);
    assert_eq!(node_display_label(&patch.nodes[0]), "p");
    assert_eq!(patch.nodes[0].diagnostic, None);
    assert!(patch_input_indices(&patch).get(&created_id).is_none());
    assert_eq!(patch_output_counts(&patch).get(&created_id), None);
}

#[test]
fn created_node_positions_are_owned_by_the_node_edit() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (1.0, 2.0));
    let edit = state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
        .unwrap();
    edit.text = "phasor".to_string();
    edit.position = (9.0, 8.0);

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    assert_eq!(patch.nodes[0].position, (9.0, 8.0));
}

#[test]
fn double_clicking_node_edits_display_text_in_memory() {
    let source = "(def pitch (in 1))";
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-edit-node-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let pitch = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "pitch")
        .unwrap();
    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 334_455,
        stable_widget_id: Some(334_455),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(key, PatcherInteractionState::default());

    let rects = patch_node_rects(&root_patch, node.rect, &PatcherPanState::default());
    let pitch_rect = rects.get(&pitch.id).unwrap();
    assert!(handle_patcher_double_click(
        &node,
        pitch_rect.col + NODE_TEXT_COL_OFFSET,
        pitch_rect.row + pitch_rect.height * 0.5,
    ));
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char('x'),
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );

    let state = get_patcher_interaction_state(key);
    assert_eq!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", "pitch"))
            .map(|edit| edit.text.as_str()),
        Some("xin 1")
    );
    assert!(matches!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", "pitch"))
            .map(|edit| &edit.origin),
        Some(PatcherNodeOrigin::Source { source_node_id }) if source_node_id == "pitch"
    ));

    let _ = std::fs::remove_file(path);
}

#[test]
fn backspace_without_text_edit_deletes_selected_nodes() {
    let source = r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#;
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-node-delete-{}.lisp",
        std::process::id()
    ));
    std::fs::write(&path, source).unwrap();
    let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let mut props = HashMap::new();
    props.insert(
        "path".to_string(),
        Value::String(path.to_string_lossy().to_string()),
    );
    let node = LayoutNode {
        widget_id: 556_677,
        stable_widget_id: Some(556_677),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState::default();
    state.selected_nodes.insert("pitch".to_string());
    set_patcher_interaction_state(key, state);

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Backspace,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    let state = get_patcher_interaction_state(key);
    assert!(state.text_edit.is_none());
    assert!(state.selected_nodes.is_empty());
    assert!(
        state
            .edit_state
            .deleted_nodes
            .contains(&node_edit_key("root", "pitch"))
    );
    let patch = patch_with_interaction_state(root_patch, &state, "root");
    assert!(
        !patch
            .nodes
            .iter()
            .any(|patch_node| patch_node.id == "pitch")
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn patcher_reports_text_capture_only_while_node_text_edit_is_active() {
    let node = LayoutNode {
        widget_id: 556_688,
        stable_widget_id: Some(556_688),
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "patcher".to_string(),
        rect: Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        },
        props: HashMap::new(),
        children: Vec::new(),
        focusable: true,
    };
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(key, PatcherInteractionState::default());
    assert!(!patcher_has_text_edit(&node));

    let mut state = PatcherInteractionState::default();
    state.text_edit = Some(PatcherTextEdit {
        node_id: "draft".to_string(),
        text: String::new(),
        original_text: String::new(),
        state: TextInputState::default(),
        autocomplete_selected: 0,
    });
    set_patcher_interaction_state(key, state);
    assert!(patcher_has_text_edit(&node));
}

#[test]
fn patcher_text_edit_tab_autocompletes_operator_without_committing() {
    let path = temp_patcher_source_path("patcher-autocomplete-tab");
    fs::write(&path, "(def sig (bi))").unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState::default();
    state.edit_state.nodes.insert(
        node_edit_key("root", "sig"),
        PatcherNodeEdit {
            view_key: "root".to_string(),
            id: "sig".to_string(),
            origin: PatcherNodeOrigin::Source {
                source_node_id: "sig".to_string(),
            },
            text: "bi".to_string(),
            position: (0.0, 0.0),
        },
    );
    state.text_edit = Some(PatcherTextEdit {
        node_id: "sig".to_string(),
        text: "bi".to_string(),
        original_text: "bi".to_string(),
        state: TextInputState {
            cursor_pos: 2,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });
    set_patcher_interaction_state(key, state);

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Tab,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some(),
        "Tab must be consumed while patcher node text edit is active"
    );

    let state = get_patcher_interaction_state(key);
    let edit = state.text_edit.as_ref().expect("Tab should not commit");
    assert_eq!(edit.text, "biquad ");
    assert_eq!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", "sig"))
            .map(|node| node.text.as_str()),
        Some("bi"),
        "autocomplete should only edit the active text buffer; Enter still commits"
    );

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char('3'),
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    let state = get_patcher_interaction_state(key);
    assert!(state.text_edit.is_none());
    assert_eq!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", "sig"))
            .map(|node| node.text.as_str()),
        Some("biquad 3")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn patcher_text_edit_tab_autocompletes_local_defmacro() {
    let path = temp_patcher_source_path("patcher-autocomplete-local-macro");
    fs::write(
        &path,
        "(defmacro shape (sig amount) (* sig amount))\n(def sig (sha))",
    )
    .unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState::default();
    state.edit_state.nodes.insert(
        node_edit_key("root", "sig"),
        PatcherNodeEdit {
            view_key: "root".to_string(),
            id: "sig".to_string(),
            origin: PatcherNodeOrigin::Source {
                source_node_id: "sig".to_string(),
            },
            text: "sha".to_string(),
            position: (0.0, 0.0),
        },
    );
    state.text_edit = Some(PatcherTextEdit {
        node_id: "sig".to_string(),
        text: "sha".to_string(),
        original_text: "sha".to_string(),
        state: TextInputState {
            cursor_pos: 3,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });
    set_patcher_interaction_state(key, state);

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Tab,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    let state = get_patcher_interaction_state(key);
    assert_eq!(
        state.text_edit.as_ref().map(|edit| edit.text.as_str()),
        Some("shape ")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn patcher_text_edit_arrow_keys_cycle_autocomplete_selection() {
    let path = temp_patcher_source_path("patcher-autocomplete-arrows");
    fs::write(&path, "(def sig (m))").unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState::default();
    state.text_edit = Some(PatcherTextEdit {
        node_id: "sig".to_string(),
        text: "m".to_string(),
        original_text: "m".to_string(),
        state: TextInputState {
            cursor_pos: 1,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });
    set_patcher_interaction_state(key, state);

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Down,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    let state = get_patcher_interaction_state(key);
    assert_eq!(
        state
            .text_edit
            .as_ref()
            .map(|edit| edit.autocomplete_selected),
        Some(1)
    );

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Up,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    let state = get_patcher_interaction_state(key);
    assert_eq!(
        state
            .text_edit
            .as_ref()
            .map(|edit| edit.autocomplete_selected),
        Some(0)
    );

    let _ = fs::remove_file(path);
}

#[test]
fn patcher_text_edit_consumes_tab_even_without_autocomplete_match() {
    let path = temp_patcher_source_path("patcher-autocomplete-tab-consume");
    fs::write(&path, "(def sig (zzzz))").unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState::default();
    state.text_edit = Some(PatcherTextEdit {
        node_id: "sig".to_string(),
        text: "zzzz".to_string(),
        original_text: "zzzz".to_string(),
        state: TextInputState {
            cursor_pos: 4,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });
    set_patcher_interaction_state(key, state);

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Tab,
                    modifiers: KeyModifiers::empty(),
                },
            )
            .is_some()
    );
    let state = get_patcher_interaction_state(key);
    assert_eq!(
        state.text_edit.as_ref().map(|edit| edit.text.as_str()),
        Some("zzzz")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn created_node_reedit_updates_same_node_edit_text() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (3.0, 4.0));
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id.clone(),
        text: "phasor".to_string(),
        original_text: String::new(),
        state: TextInputState {
            cursor_pos: 6,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });
    commit_patcher_text_edit(&mut state, "root");
    assert_eq!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", &created_id))
            .map(|edit| edit.text.as_str()),
        Some("phasor")
    );

    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id.clone(),
        text: "triangle".to_string(),
        original_text: "phasor".to_string(),
        state: TextInputState {
            cursor_pos: 8,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });
    commit_patcher_text_edit(&mut state, "root");
    assert_eq!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", &created_id))
            .map(|edit| edit.text.as_str()),
        Some("triangle")
    );

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    assert_eq!(patch.nodes.len(), 1);
    assert_eq!(node_display_label(&patch.nodes[0]), "triangle");
}

#[test]
fn defmacro_created_node_promotes_to_macro_instance_text() {
    let root_patch = parse("(def sig (in 1))\n(out sig 1)");
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (3.0, 4.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
        .unwrap()
        .text = "defmacro *saturate*".to_string();

    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &created_id,
    ));
    assert_eq!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", &created_id))
            .map(|edit| edit.text.as_str()),
        Some("saturate")
    );

    let patch = patch_with_interaction_state(root_patch, &state, "root");
    let instance = patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == created_id)
        .unwrap();
    assert_eq!(instance.kind, NodeKind::MacroInstance);
    assert_eq!(instance.op, "saturate");
    assert!(
        patch
            .macros
            .iter()
            .any(|macro_patch| macro_patch.name == "saturate")
    );
}

#[test]
fn writeback_created_macro_emits_default_valid_defmacro_without_placeholder_call() {
    let source = "(def sig (in 1))\n(out sig 1)";
    let root_patch = parse(source);
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (3.0, 4.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
        .unwrap()
        .text = "defmacro *saturate*".to_string();
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &created_id,
    ));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro saturate (input) (* input 1.0))\n(def sig (in 1))\n(out sig 1)"
    );
}

#[test]
fn writeback_connected_created_macro_instance_emits_call_after_definition() {
    let source = "(def sig (in 1))\n(out sig 1)";
    let root_patch = parse(source);
    let sig = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "sig")
        .unwrap();
    let out = root_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let sig_to_out = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sig.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (3.0, 4.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
        .unwrap()
        .text = "defmacro *saturate*".to_string();
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &created_id,
    ));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: created_id.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: created_id,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro saturate (input) (* input 1.0))\n(def sig (in 1))\n(def saturate1 (saturate sig))\n(out saturate1 1)"
    );
}

#[test]
fn writeback_active_edit_inside_created_macro_updates_default_body() {
    let source = "(def sig (in 1))\n(out sig 1)";
    let root_patch = parse(source);
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (3.0, 4.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
        .unwrap()
        .text = "defmacro *a*".to_string();
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &created_id,
    ));
    state.active_macro = Some("a".to_string());

    let macro_patch = active_patcher_patch(&root_patch, &state);
    let return_node = macro_patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .expect("default macro should expose a return node");
    state.text_edit = Some(PatcherTextEdit {
        node_id: return_node.id.clone(),
        text: "* 2".to_string(),
        original_text: node_display_label(return_node),
        state: TextInputState {
            cursor_pos: 3,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro a (input) (* input 2.0))\n(def sig (in 1))\n(out sig 1)"
    );
}

#[test]
fn writeback_deleting_created_macro_default_return_keeps_valid_body() {
    let source = "(def sig (in 1))\n(out sig 1)";
    let root_patch = parse(source);
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_text_node(&mut state, "root", "defmacro *op*");
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &created_id,
    ));
    state.active_macro = Some("op".to_string());

    let macro_patch = active_patcher_patch(&root_patch, &state);
    let return_node = macro_patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .expect("default macro should expose a return node");
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:op", &return_node.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro op (input) input)\n(def sig (in 1))\n(out sig 1)"
    );
}

#[test]
fn writeback_replacing_created_macro_default_body_with_chain_compiles() {
    let source = "(def sig (in 1))\n(out sig 1)";
    let root_patch = parse(source);
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (3.0, 4.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
        .unwrap()
        .text = "defmacro *op*".to_string();
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &created_id,
    ));
    state.active_macro = Some("op".to_string());

    let macro_patch = active_patcher_patch(&root_patch, &state);
    let input = macro_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let return_node = macro_patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .expect("default macro should expose a return node");
    let out = macro_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:op", &return_node.id));

    let phasor = allocate_created_text_node(&mut state, "macro:op", "phasor");
    let triangle = allocate_created_text_node(&mut state, "macro:op", "triangle");
    connect_output_to_input(&mut state, "macro:op", &input.id, &phasor, 0);
    connect_output_to_input(&mut state, "macro:op", &phasor, &triangle, 0);
    connect_output_to_input(&mut state, "macro:op", &triangle, &out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert_eq!(
        emitted,
        "(defmacro op (input) (def phasor1 (phasor input)) (def triangle1 (triangle phasor1)) triangle1)\n(def sig (in 1))\n(out sig 1)"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn writeback_created_macro_default_return_rewired_to_chain_then_root_input_compiles() {
    let source = "(def pitch (in 1 @name pitch))\n(out (* (phasor pitch) 1) 1 @name audio)";
    let root_patch = parse(source);
    let mut state = PatcherInteractionState::default();
    let macro_instance = allocate_created_text_node(&mut state, "root", "defmacro *ap*");
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &macro_instance,
    ));
    state.active_macro = Some("ap".to_string());

    let macro_patch = active_patcher_patch(&root_patch, &state);
    let input = macro_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let return_node = macro_patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .expect("default macro should expose a return node");
    let out = macro_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let return_to_out = macro_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == return_node.id && connection.to_node == out.id)
        .unwrap();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:ap",
            &source_connection_id(return_to_out),
        ));

    let phasor = allocate_created_text_node(&mut state, "macro:ap", "phasor");
    let triangle = allocate_created_text_node(&mut state, "macro:ap", "triangle");
    connect_output_to_input(&mut state, "macro:ap", &input.id, &phasor, 0);
    connect_output_to_input(&mut state, "macro:ap", &phasor, &triangle, 0);
    connect_output_to_input(&mut state, "macro:ap", &triangle, &out.id, 0);

    let source_with_macro_chain =
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    state.active_macro = None;
    connect_output_to_input(&mut state, "root", "pitch", &macro_instance, 0);

    let emitted =
        emit_patch_writeback(&source_with_macro_chain, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(def ap1 (ap pitch))"),
        "connecting root pitch to the macro should emit a macro call:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn writeback_created_macro_input_extends_header_and_root_usage_arity() {
    let source = "(def pitch (in 1 @name pitch))\n(out pitch 1 @name audio)";
    let root_patch = parse(source);
    let pitch_to_out = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == "pitch")
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let macro_instance = allocate_created_text_node(&mut state, "root", "defmacro *op*");
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &macro_instance,
    ));

    state.active_macro = Some("op".to_string());
    let input2 = allocate_created_text_node(&mut state, "macro:op", "in 2");
    let macro_patch = patch_with_interaction_state(root_patch.clone(), &state, "macro:op");
    let input2_node = macro_patch
        .nodes
        .iter()
        .find(|node| node.id == input2)
        .expect("created macro input should be visible");
    assert_eq!(input2_node.kind, NodeKind::In);

    state.active_macro = None;
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &macro_instance))
        .unwrap()
        .text = "op 3".to_string();
    let root_with_created_input = patch_with_interaction_state(root_patch.clone(), &state, "root");
    let edited_instance = root_with_created_input
        .nodes
        .iter()
        .find(|node| node.id == macro_instance)
        .expect("macro instance should still exist");
    assert_eq!(edited_instance.kind, NodeKind::MacroInstance);
    assert_eq!(edited_instance.args.len(), 2);
    assert_eq!(edited_instance.diagnostic, None);

    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(pitch_to_out),
        ));
    connect_output_to_input(&mut state, "root", "pitch", &macro_instance, 0);
    connect_output_to_input(
        &mut state,
        "root",
        &macro_instance,
        &pitch_to_out.to_node,
        0,
    );

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert_eq!(
        emitted,
        "(defmacro op (input input2) (* input 1.0))\n(def pitch (in 1 @name pitch))\n(def op1 (op pitch 3.0))\n(out op1 1 @name audio)"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn writeback_created_macro_input_allows_existing_usage_to_gain_argument() {
    let source = r#"
        (defmacro op (input) input)
        (def pitch (in 1 @name pitch))
        (def op1 (op pitch))
        (out op1 1 @name audio)
    "#;
    let root_patch = parse(source);
    let op1 = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "op1")
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let input2 = allocate_created_text_node(&mut state, "macro:op", "in 2");
    ensure_source_node_edit(&mut state, "root", op1, node_display_label(op1));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", "op1"))
        .unwrap()
        .text = "op 3".to_string();

    let root_with_created_input = patch_with_interaction_state(root_patch.clone(), &state, "root");
    let edited_op1 = root_with_created_input
        .nodes
        .iter()
        .find(|node| node.id == "op1")
        .unwrap();
    assert_eq!(edited_op1.args.len(), 2);
    assert_eq!(edited_op1.diagnostic, None);

    let macro_patch = patch_with_interaction_state(root_patch, &state, "macro:op");
    assert!(macro_patch.nodes.iter().any(|node| node.id == input2));

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(defmacro op (input input2) input)"),
        "created macro inlet should extend defmacro params:\n{emitted}"
    );
    assert!(
        emitted.contains("(def op1 (op pitch 3.0))"),
        "root macro usage should retain the new second argument:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn writeback_two_created_macro_instances_can_replace_existing_source_input() {
    let source = r#"
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def trigger (in 3 @name trigger))
        (def attack (param attack @default 5))
        (def release (param release @default 200))
        (def env (adsr gate trigger attack 100 1 release))
        (out env 1 @name audio)
    "#;
    let root_patch = parse(source);
    let mut state = PatcherInteractionState::default();
    let first_macro = allocate_created_text_node(&mut state, "root", "defmacro *op*");
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &first_macro,
    ));
    state.active_macro = Some("op".to_string());

    let macro_patch = active_patcher_patch(&root_patch, &state);
    let input = macro_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let return_node = macro_patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .expect("default macro should expose a return node");
    let out = macro_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let return_to_out = macro_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == return_node.id && connection.to_node == out.id)
        .unwrap();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:op",
            &source_connection_id(return_to_out),
        ));
    let phasor = allocate_created_text_node(&mut state, "macro:op", "phasor");
    let triangle = allocate_created_text_node(&mut state, "macro:op", "triangle");
    connect_output_to_input(&mut state, "macro:op", &input.id, &phasor, 0);
    connect_output_to_input(&mut state, "macro:op", &phasor, &triangle, 0);
    connect_output_to_input(&mut state, "macro:op", &triangle, &out.id, 0);

    let source_with_macro_chain =
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    let root_patch = parse(&source_with_macro_chain);
    let attack_to_env = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == "attack" && connection.to_node == "env")
        .unwrap();

    state.active_macro = None;
    let second_macro = allocate_created_text_node(&mut state, "root", "op");
    let sum = allocate_created_text_node(&mut state, "root", "+");
    connect_output_to_input(&mut state, "root", "pitch", &first_macro, 0);
    connect_output_to_input(&mut state, "root", "pitch", &second_macro, 0);
    connect_output_to_input(&mut state, "root", &first_macro, &sum, 0);
    connect_output_to_input(&mut state, "root", &second_macro, &sum, 1);
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(attack_to_env),
        ));
    connect_output_to_input(&mut state, "root", &sum, "env", 2);

    let emitted =
        emit_patch_writeback(&source_with_macro_chain, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(def add1 (+ op1 op2))") || emitted.contains("(def add1 (+ op2 op1))"),
        "expected summed macro instances before env attack input:\n{emitted}"
    );
    assert!(
        emitted.contains("(def env (adsr gate trigger add1 100.0 1.0 release))"),
        "expected env attack input to come from the sum:\n{emitted}"
    );
}

#[test]
fn writeback_stale_deleted_source_connection_recovers_when_created_sum_replaces_it() {
    let source = r#"
        (defmacro op (input) (def phasor1 (phasor input)) (def triangle1 (triangle phasor1)) triangle1)
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def trigger (in 3 @name trigger))
        (def attack (param attack @default 5))
        (def release (param release @default 200))
        (def op1 (op pitch))
        (def op2 (op pitch))
        (def add1 (+ op1 op2))
        (def env (adsr gate trigger add1 100 1 release))
        (out env 1 @name audio)
    "#;
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key("root", "attack:0->env:2"));
    let sum = allocate_created_text_node(&mut state, "root", "+");
    connect_output_to_input(&mut state, "root", "op1", &sum, 0);
    connect_output_to_input(&mut state, "root", "op2", &sum, 1);
    connect_output_to_input(&mut state, "root", &sum, "env", 2);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(def env (adsr gate trigger add1 100.0 1.0 release))"),
        "stale attack deletion should not prevent recovery once env input is replaced:\n{emitted}"
    );
}

#[test]
fn writeback_macro_created_node_after_generated_binding_preserves_dependency() {
    let source = "(defmacro op (input) (def phasor1 (phasor input)) phasor1)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "op")
        .unwrap();
    let phasor = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let phasor_to_out = macro_patch
        .patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phasor.id && connection.to_node == out.id)
        .unwrap();

    let mut state = PatcherInteractionState {
        active_macro: Some("op".to_string()),
        ..Default::default()
    };
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:op",
            &source_connection_id(phasor_to_out),
        ));
    let triangle = allocate_created_node(&mut state, "macro:op", (3.0, 4.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:op", &triangle))
        .unwrap()
        .text = "triangle".to_string();
    allocate_created_connection(
        &mut state,
        "macro:op",
        OutputPortRef {
            node_id: phasor.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: triangle.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:op",
        OutputPortRef {
            node_id: triangle,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro op (input) (def phasor1 (phasor input)) (def triangle1 (triangle phasor1)) triangle1)"
    );
}

#[test]
fn writeback_literal_into_created_phasor_in_macro_preserves_literal_node() {
    let source = "(defmacro xop (input input2) (def phasor1 (phasor input)) (def triangle1 (triangle phasor1 input2)) (def mul1 (* triangle1 twopi)) mul1)\n(def pitch (in 2 @name pitch))\n(def xop1 (xop pitch 1.0))\n(out xop1 1 @name audio)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "xop")
        .unwrap();
    let triangle = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "triangle1")
        .unwrap();

    let mut state = PatcherInteractionState {
        active_macro: Some("xop".to_string()),
        ..Default::default()
    };
    let rate = allocate_created_text_node(&mut state, "macro:xop", "0.3");
    let phasor = allocate_created_text_node(&mut state, "macro:xop", "phasor");
    connect_output_to_input(&mut state, "macro:xop", &rate, &phasor, 0);
    connect_output_to_input(&mut state, "macro:xop", &phasor, &triangle.id, 1);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(def value1 0.3)"),
        "created literal should be preserved as a generated binding:\n{emitted}"
    );
    assert!(
        emitted.contains("(def phasor2 (phasor value1))"),
        "created phasor should reference the literal node binding instead of inlining it:\n{emitted}"
    );
    assert!(
        !emitted.contains("(def phasor2 (phasor 0.3))"),
        "connected literal must not be inlined into the created phasor:\n{emitted}"
    );
    assert!(
        emitted.contains("(def triangle1 (triangle phasor1 phasor2))"),
        "created phasor should still connect into triangle's second inlet:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));

    let roundtrip = parse(&emitted);
    let xop = roundtrip
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "xop")
        .unwrap();
    let value = xop
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Constant && node.op == "0.3")
        .expect("roundtrip should project the generated literal as a constant node");
    let phasor = xop
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor2")
        .expect("roundtrip should keep the generated phasor binding");
    assert_eq!(node_display_label(phasor), "phasor");
    assert!(xop.patch.connections.iter().any(|connection| {
        connection.from_node == value.id
            && connection.to_node == phasor.id
            && connection.to_input == 0
    }));
    assert!(xop.patch.connections.iter().any(|connection| {
        connection.from_node == phasor.id
            && connection.to_node == "triangle1"
            && connection.to_input == 1
    }));
}

#[test]
fn writeback_macro_followup_edit_ignores_stale_materialized_created_node() {
    let source = "(defmacro op (input) (def phasor1 (phasor input)) phasor1)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "op")
        .unwrap();
    let input = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let phasor = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let phasor_to_out = macro_patch
        .patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phasor.id && connection.to_node == out.id)
        .unwrap();

    let mut state = PatcherInteractionState {
        active_macro: Some("op".to_string()),
        ..Default::default()
    };

    let stale_phasor = allocate_created_node(&mut state, "macro:op", (2.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:op", &stale_phasor))
        .unwrap()
        .text = "phasor".to_string();
    allocate_created_connection(
        &mut state,
        "macro:op",
        OutputPortRef {
            node_id: input.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: stale_phasor.clone(),
            input_index: 0,
        },
    );

    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:op",
            &source_connection_id(phasor_to_out),
        ));
    let triangle = allocate_created_node(&mut state, "macro:op", (3.0, 4.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:op", &triangle))
        .unwrap()
        .text = "triangle".to_string();
    allocate_created_connection(
        &mut state,
        "macro:op",
        OutputPortRef {
            node_id: phasor.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: triangle.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:op",
        OutputPortRef {
            node_id: triangle,
            output_index: 0,
        },
        InputPortRef {
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro op (input) (def phasor1 (phasor input)) (def triangle1 (triangle phasor1)) triangle1)"
    );
}

#[test]
fn writeback_created_macro_chain_then_root_rewire_compiles() {
    let source = "(def pitch (in 1 @name pitch))\n(out (* (phasor pitch) 1) 1 @name audio)";
    let root_patch = parse(source);
    let phase = root_patch
        .nodes
        .iter()
        .find(|node| node.op == "phasor")
        .unwrap();
    let osc = root_patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let phase_to_osc = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phase.id && connection.to_node == osc.id)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    let macro_instance = allocate_created_node(&mut state, "root", (3.0, 4.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &macro_instance))
        .unwrap()
        .text = "defmacro *op*".to_string();
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &macro_instance,
    ));

    let source_with_macro_chain = "\
(defmacro op (input) (def phasor1 (phasor input)) (def triangle1 (triangle phasor1)) triangle1)
(def pitch (in 1 @name pitch))
(out (* (phasor pitch) 1) 1 @name audio)";
    let root_patch_with_macro_chain = parse(source_with_macro_chain);
    let macro_patch = root_patch_with_macro_chain
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "op")
        .unwrap();
    let input = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let macro_out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();

    let macro_phasor = allocate_created_node(&mut state, "macro:op", (2.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:op", &macro_phasor))
        .unwrap()
        .text = "phasor".to_string();
    let macro_triangle = allocate_created_node(&mut state, "macro:op", (3.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:op", &macro_triangle))
        .unwrap()
        .text = "triangle".to_string();
    allocate_created_connection(
        &mut state,
        "macro:op",
        OutputPortRef {
            node_id: input.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: macro_phasor.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:op",
        OutputPortRef {
            node_id: macro_phasor,
            output_index: 0,
        },
        InputPortRef {
            node_id: macro_triangle.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:op",
        OutputPortRef {
            node_id: macro_triangle,
            output_index: 0,
        },
        InputPortRef {
            node_id: macro_out.id.clone(),
            input_index: 0,
        },
    );

    state.active_macro = None;
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(phase_to_osc),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: "pitch".to_string(),
            output_index: 0,
        },
        InputPortRef {
            node_id: macro_instance.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: macro_instance,
            output_index: 0,
        },
        InputPortRef {
            node_id: osc.id.clone(),
            input_index: 0,
        },
    );

    let emitted =
        emit_patch_writeback(&source_with_macro_chain, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(defmacro op (input)"),
        "emitted source should contain the created macro:\n{emitted}"
    );
    assert!(
        emitted.contains("(def phasor1 (phasor input))")
            && emitted.contains("(def triangle1 (triangle phasor1))"),
        "emitted source should keep the macro-local chain definitions:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn source_and_created_nodes_share_one_edit_model() {
    let source = parse("(def pitch (in 1))");
    let pitch = source.nodes.iter().find(|node| node.id == "pitch").unwrap();
    let mut state = PatcherInteractionState::default();
    set_node_edit_position(
        &mut state,
        "root",
        pitch,
        (12.0, 3.0),
        node_display_label(pitch),
    );
    let created_id = allocate_created_node(&mut state, "root", (4.0, 5.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created_id))
        .unwrap()
        .text = "* 3".to_string();

    let patch = patch_with_interaction_state(source, &state, "root");
    let source_node = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
    let created_node = patch
        .nodes
        .iter()
        .find(|node| node.id == created_id)
        .unwrap();
    assert_eq!(source_node.position, (12.0, 3.0));
    assert_eq!(node_display_label(created_node), "* 3");
    assert_eq!(state.edit_state.nodes.len(), 2);
    assert!(matches!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", "pitch"))
            .map(|edit| &edit.origin),
        Some(PatcherNodeOrigin::Source { .. })
    ));
    assert!(matches!(
        state
            .edit_state
            .nodes
            .get(&node_edit_key("root", &created_id))
            .map(|edit| &edit.origin),
        Some(PatcherNodeOrigin::Created { .. })
    ));
}

#[test]
fn layout_assigns_finite_nonzero_node_positions() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (triangle (phasor pitch)))
            (out sig 1 @name audio)
            "#,
    );
    for node in &patch.nodes {
        assert!(node.position.0.is_finite());
        assert!(node.position.1.is_finite());
        assert!(node.position.0 >= 0.0);
        assert!(node.position.1 >= 0.0);
    }
}

#[test]
fn layout_preserves_source_order_instead_of_sorting_ids_alphabetically() {
    let patch = parse(
        r#"
            (def z (in 1 @name z_source))
            (def a (in 2 @name a_source))
            (def left (* z 2))
            (def right (* a 3))
            (out (+ left right) 1 @name audio)
            "#,
    );
    let z = patch.nodes.iter().find(|node| node.id == "z").unwrap();
    let a = patch.nodes.iter().find(|node| node.id == "a").unwrap();
    assert!(
        z.position.0 < a.position.0,
        "layout should use source/dataflow order, not alphabetical id order: z={:?} a={:?}",
        z.position,
        a.position
    );
}

#[test]
fn layout_stacks_many_params_vertically() {
    let patch = parse(
        r#"
            (param mix @min 0 @max 1 @default 0.5)
            (param tone @min 0 @max 1 @default 0.5)
            (param drive @min 0 @max 1 @default 0.5)
            (param gain @min 0 @max 1 @default 0.5)
            (def signal (in 1 @name signal))
            (def shaped (* signal gain))
            (out (+ (* shaped mix) tone drive) 1 @name audio)
            "#,
    );
    let params = ["mix", "tone", "drive", "gain"]
        .into_iter()
        .map(|id| patch.nodes.iter().find(|node| node.id == id).unwrap())
        .collect::<Vec<_>>();
    let x = params[0].position.0;
    for param in &params {
        assert!(
            (param.position.0 - x).abs() < 0.01,
            "params should share a control-stack x position: {id} at {:?}, expected x {x}",
            param.position,
            id = param.id
        );
    }
    for pair in params.windows(2) {
        assert!(
            pair[0].position.1 < pair[1].position.1,
            "params should be stacked vertically in stable order: {}={:?} {}={:?}",
            pair[0].id,
            pair[0].position,
            pair[1].id,
            pair[1].position
        );
    }
}

#[test]
fn layout_segments_generated_connections() {
    let patch = parse(
        r#"
            (def signal (in 1 @name signal))
            (def a (* signal 0.5))
            (def b (+ a 0.25))
            (out b 1 @name audio)
            "#,
    );
    for connection in &patch.connections {
        assert!(
            connection
                .segment
                .is_some_and(|segment| segment.is_segmented),
            "generated connection should have deterministic segment routing: {} -> {}",
            connection.from_node,
            connection.to_node
        );
    }
}

#[test]
fn layout_reuses_segment_lane_for_same_source_fanout() {
    let patch = parse(
        r#"
            (param amount @min 0 @max 1 @default 0.5)
            (def input (in 1 @name input))
            (def a (* input amount))
            (def b (+ amount input))
            (out (+ a b) 1 @name audio)
            "#,
    );
    let lanes = patch
        .connections
        .iter()
        .filter(|connection| connection.from_node == "amount")
        .map(|connection| connection.segment.unwrap().segment_row)
        .collect::<Vec<_>>();
    assert!(lanes.len() >= 2, "fixture should create param fanout");
    for lane in lanes.iter().skip(1) {
        assert!(
            (*lane - lanes[0]).abs() < 0.01,
            "same source fanout should share one segment lane: {:?}",
            lanes
        );
    }
}

#[test]
fn layout_separates_segment_lanes_for_different_sources() {
    let patch = parse(
        r#"
            (param a @min 0 @max 1 @default 0.5)
            (param b @min 0 @max 1 @default 0.5)
            (def input (in 1 @name input))
            (def left (* input a))
            (def right (* input b))
            (out (+ left right) 1 @name audio)
            "#,
    );
    let lane_for = |source: &str| {
        patch
            .connections
            .iter()
            .find(|connection| connection.from_node == source)
            .and_then(|connection| connection.segment)
            .unwrap()
            .segment_row
    };
    let a_lane = lane_for("a");
    let b_lane = lane_for("b");
    assert!(
        (a_lane - b_lane).abs() >= 0.5,
        "different source nodes should not share overlapping segment lanes: a={a_lane} b={b_lane}"
    );
}

#[test]
fn layout_routes_stacked_params_right_before_crossing_next_param() {
    let patch = parse(
        r#"
            (param first @min 0 @max 1 @default 0.5)
            (param second @min 0 @max 1 @default 0.5)
            (param third @min 0 @max 1 @default 0.5)
            (def signal (in 1 @name signal))
            (def a (* signal first))
            (def b (* signal second))
            (def c (* signal third))
            (out (+ a b c) 1 @name audio)
            "#,
    );
    let first = patch.nodes.iter().find(|node| node.id == "first").unwrap();
    let second = patch.nodes.iter().find(|node| node.id == "second").unwrap();
    let first_lane = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == "first")
        .and_then(|connection| connection.segment)
        .unwrap()
        .segment_row;
    assert!(
        first_lane < second.position.1,
        "top stacked param should turn right before crossing the next param: first={:?} second={:?} lane={first_lane}",
        first.position,
        second.position
    );
}

#[test]
fn layout_aligns_primary_signal_chain_centers() {
    let patch = parse(
        r#"
            (def signal (in 1 @name signal))
            (def a (* signal 0.5))
            (def b (+ a 0.25))
            (def c (delay b 120))
            (out c 1 @name audio)
            "#,
    );
    let center = |id: &str| {
        let node = patch.nodes.iter().find(|node| node.id == id).unwrap();
        let input_indices = patch_input_indices(&patch);
        let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
        let output_counts = patch_output_counts(&patch);
        let (width, _) = node_size_for_ports(
            node,
            input_slot_counts.get(&node.id).copied().unwrap_or(0),
            output_counts.get(&node.id).copied().unwrap_or(0),
        );
        node.position.0 + width * 0.5
    };

    let a = center("a");
    let b = center("b");
    let c = center("c");
    assert!(
        (a - b).abs() < 0.01 && (b - c).abs() < 0.01,
        "primary signal chain should align vertically by center: a={a} b={b} c={c}"
    );
}

#[test]
fn layout_places_single_use_constants_near_consumers() {
    let patch = parse(
        r#"
            (def signal (in 1 @name signal))
            (def shaped (* 0.5 signal))
            (out shaped 1 @name audio)
            "#,
    );
    let constant = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Constant)
        .unwrap();
    let shaped = patch.nodes.iter().find(|node| node.id == "shaped").unwrap();
    assert!(
        (constant.position.0 - shaped.position.0).abs() < 8.0,
        "single-use constant should be local to its consumer: constant={:?} consumer={:?}",
        constant.position,
        shaped.position
    );
}

#[test]
fn layout_keeps_collapsed_history_near_feedback_loop_body() {
    let patch = parse(
        r#"
            (make-history h)
            (def signal (in 1 @name signal))
            (def body (+ (read-history h) signal))
            (write-history h body)
            (out body 1 @name audio)
            "#,
    );
    let history = patch.nodes.iter().find(|node| node.id == "h").unwrap();
    let body = patch.nodes.iter().find(|node| node.id == "body").unwrap();
    assert!(
        history.position.1 > VIEW_PADDING_Y,
        "history should not be treated as a rank-0 global source when it has a feedback writer: {:?}",
        history.position
    );
    assert!(
        (history.position.1 - body.position.1).abs() <= LAYER_SPACING + 0.01,
        "history should stay close to its feedback loop body: history={:?} body={:?}",
        history.position,
        body.position
    );
}

#[test]
fn layout_keeps_lexilush_history_nodes_near_feedback_writers() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sequencer/effects/lexilush/dsp.lisp");
    let source = std::fs::read_to_string(path).unwrap();
    let patch = parse_patch_source(&source, PatcherIntent::Effect).unwrap();
    let id_to_node = patch
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    for connection in patch
        .connections
        .iter()
        .filter(|connection| connection.kind == ConnectionKind::Feedback)
    {
        let writer = id_to_node
            .get(connection.from_node.as_str())
            .expect("feedback writer node");
        let history = id_to_node
            .get(connection.to_node.as_str())
            .expect("feedback history node");
        assert_eq!(history.kind, NodeKind::History);
        assert!(
            (writer.position.1 - history.position.1).abs() <= LAYER_SPACING + 0.01,
            "history node `{}` should remain near feedback writer `{}`: history={:?} writer={:?}",
            history.id,
            writer.id,
            history.position,
            writer.position
        );
    }
}

#[test]
fn fixture_videogame_arp_projects_without_parse_failure() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sequencer/instruments/arcade/videogame-arp/dsp.lisp");
    let source = std::fs::read_to_string(path).unwrap();
    let patch = parse_patch_source(&source, PatcherIntent::Instrument).unwrap();
    assert!(!patch.nodes.is_empty());
}

#[test]
fn fixture_lexilush_projects_without_parse_failure() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sequencer/effects/lexilush/dsp.lisp");
    let source = std::fs::read_to_string(path).unwrap();
    let patch = parse_patch_source(&source, PatcherIntent::Effect).unwrap();
    assert!(!patch.nodes.is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_emits_nodes_and_cables() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (triangle (phasor pitch)))
            (out sig 1 @name audio)
            "#,
    );
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &PatcherInteractionState::default(),
    );
    let text_count = prims
        .iter()
        .filter(|prim| matches!(prim, MetalPrimitive::ProportionalText(_)))
        .count();
    let rect_count = prims
        .iter()
        .filter(|prim| matches!(prim, MetalPrimitive::Rect(_)))
        .count();
    let rounded_count = prims
        .iter()
        .filter(|prim| matches!(prim, MetalPrimitive::WidgetInstance { .. }))
        .count();
    let cable_count = prims
        .iter()
        .filter(|prim| matches!(prim, MetalPrimitive::PatchCable(_)))
        .count();
    let min_cable_radius = prims
        .iter()
        .filter_map(|prim| match prim {
            MetalPrimitive::PatchCable(cable) => Some(cable.radius_px),
            _ => None,
        })
        .fold(f32::INFINITY, f32::min);
    assert!(text_count >= patch.nodes.len(), "{text_count}");
    assert!(rounded_count >= patch.nodes.len() * 2, "{rounded_count}");
    assert!(cable_count >= patch.connections.len(), "{cable_count}");
    assert!(min_cable_radius >= 4.4 * DEFAULT_ZOOM, "{min_cable_radius}");
    assert!(
        rect_count == 0,
        "patcher node chrome should use rounded widget instances, got {rect_count} rects"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_marks_selected_cable_and_handles() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let selected_cable = source_connection_id(patch.connections.first().unwrap());
    let state = PatcherInteractionState {
        selected_cable: Some(selected_cable),
        ..Default::default()
    };
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &state,
    );

    let selected_cable_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                prim,
                MetalPrimitive::PatchCable(cable) if cable.color == theme::PATCHER_ERROR()
            )
        })
        .count();
    let handle_shell_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                prim,
                MetalPrimitive::Circle(circle) if circle.color == theme::PATCHER_ERROR()
            )
        })
        .count();

    assert_eq!(selected_cable_count, 1);
    assert_eq!(handle_shell_count, 2);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_endpoint_drag_replaces_original_selected_cable() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let connection = patch.connections.first().unwrap();
    let selected_cable = source_connection_id(connection);
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 100.0,
        height: 40.0,
    };
    let pan = PatcherPanState::default();
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let (start, end) = connection_endpoints(
        connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    let state = PatcherInteractionState {
        selected_cable: Some(selected_cable.clone()),
        drag: Some(PatcherDragState::CableEndpoint {
            cable_id: selected_cable,
            endpoint: CableEndpoint::To,
            original_from: OutputPortRef {
                node_id: connection.from_node.clone(),
                output_index: connection.from_output,
            },
            original_to: InputPortRef {
                node_id: connection.to_node.clone(),
                input_index: connection.to_input,
            },
            start_col: start.0,
            start_row: start.1,
            end_col: end.0,
            end_row: end.1,
            current_col: end.0 + 5.0,
            current_row: end.1 + 2.0,
            target_from: None,
            target_to: None,
        }),
        ..Default::default()
    };
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        rect,
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &pan,
        &state,
    );

    let cable_count = prims
        .iter()
        .filter(|prim| matches!(prim, MetalPrimitive::PatchCable(_)))
        .count();
    let selected_cable_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                prim,
                MetalPrimitive::PatchCable(cable) if cable.color == theme::PATCHER_ERROR()
            )
        })
        .count();
    let handle_shell_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                prim,
                MetalPrimitive::Circle(circle) if circle.color == theme::PATCHER_ERROR()
            )
        })
        .count();

    assert_eq!(
        cable_count, 1,
        "endpoint dragging should render the moving cable instead of original plus preview"
    );
    assert_eq!(selected_cable_count, 1);
    assert_eq!(handle_shell_count, 2);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_emits_edit_cursor_as_foreground_overlay() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (2.0, 2.0));
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id,
        text: "phasor".to_string(),
        original_text: String::new(),
        state: TextInputState {
            cursor_pos: 6,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &state,
    );

    let cursor_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                prim,
                MetalPrimitive::ForegroundRect(rect)
                    if rect.color == theme::PATCHER_EDIT_CURSOR()
            )
        })
        .count();
    assert_eq!(
        cursor_count, 1,
        "active patcher text edit should render exactly one foreground cursor"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_emits_autocomplete_panel_for_active_operator_prefix() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (2.0, 2.0));
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id,
        text: "bi".to_string(),
        original_text: String::new(),
        state: TextInputState {
            cursor_pos: 2,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &state,
    );

    assert!(
        prims.iter().any(|prim| {
            matches!(
                prim,
                MetalPrimitive::ProportionalText(text) if text.text == "biquad"
            )
        }),
        "active operator prefix should render its autocomplete suggestion"
    );
    assert!(
        prims.iter().any(|prim| {
            matches!(
                prim,
                MetalPrimitive::ProportionalText(text) if text.text == "IIR biquad filter."
            )
        }),
        "autocomplete documentation panel should render structured documentation from the operator manifest"
    );
    assert!(
        prims.iter().any(|prim| {
            matches!(
                prim,
                MetalPrimitive::ProportionalText(text)
                    if text.text.contains("inlets:")
                        && text.text.contains("signal signal|float")
            )
        }),
        "autocomplete documentation panel should render structured inlet signatures"
    );
    assert!(
        prims.iter().any(|prim| {
            matches!(
                prim,
                MetalPrimitive::ProportionalText(text)
                    if text.text.contains("outlets:")
                        && text.text.contains("out")
            )
        }),
        "autocomplete documentation panel should render structured outlet signatures"
    );
    let suggestion_col = prims
        .iter()
        .find_map(|prim| match prim {
            MetalPrimitive::ProportionalText(text) if text.text == "biquad" => Some(text.col),
            _ => None,
        })
        .expect("suggestion text");
    let doc_col = prims
        .iter()
        .find_map(|prim| match prim {
            MetalPrimitive::ProportionalText(text) if text.text == "IIR biquad filter." => {
                Some(text.col)
            }
            _ => None,
        })
        .expect("documentation text");
    assert!(
        doc_col > suggestion_col + 10.0,
        "selected operator documentation should render in a separate panel to the right"
    );
    assert!(
        prims
            .iter()
            .filter(|prim| {
                matches!(
                    prim,
                    MetalPrimitive::WidgetInstance { widget_type, instance, .. }
                        if widget_type == "patcher-node"
                            && instance.color_a == theme::PATCHER_AUTOCOMPLETE_BORDER().to_rgba()
                            && instance.color_b == theme::PATCHER_AUTOCOMPLETE_BG().to_rgba()
                )
            })
            .count()
            >= 2,
        "autocomplete list and selected-documentation panels should both render full bordered chrome"
    );
    assert!(
        prims.iter().any(|prim| {
            matches!(
                prim,
                MetalPrimitive::ForegroundRect(rect)
                    if rect.color == theme::PATCHER_AUTOCOMPLETE_SELECTED_BG()
            )
        }),
        "autocomplete panel should render the selected row highlight"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_wraps_selected_autocomplete_documentation() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (2.0, 2.0));
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id,
        text: "phas".to_string(),
        original_text: String::new(),
        state: TextInputState {
            cursor_pos: 4,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &state,
    );

    let suggestion_col = prims
        .iter()
        .find_map(|prim| match prim {
            MetalPrimitive::ProportionalText(text) if text.text == "phase-vocoder" => {
                Some(text.col)
            }
            _ => None,
        })
        .expect("suggestion text");
    let doc_lines: Vec<&str> = prims
        .iter()
        .filter_map(|prim| match prim {
            MetalPrimitive::ProportionalText(text) if text.col > suggestion_col + 10.0 => {
                Some(text.text.as_str())
            }
            _ => None,
        })
        .collect();

    assert!(
        doc_lines.iter().any(|line| line.starts_with("outlets:")),
        "wrapped documentation should include structured outlet signatures"
    );
    assert!(
        doc_lines.iter().all(|line| line.chars().count() <= 56),
        "documentation lines should be pre-wrapped before rendering"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_uses_single_text_run_for_active_node_edit_with_spaces() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (2.0, 2.0));
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id,
        text: "in 3 4".to_string(),
        original_text: String::new(),
        state: TextInputState {
            cursor_pos: 6,
            selection_anchor: None,
            selecting: false,
        },
        autocomplete_selected: 0,
    });

    let patch = patch_with_interaction_state(Patch::default(), &state, "root");
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &state,
    );

    let label_runs: Vec<&str> = prims
        .iter()
        .filter_map(|prim| match prim {
            MetalPrimitive::ProportionalText(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        label_runs.contains(&"in 3 4"),
        "active edit should render the complete edit buffer as one text run: {label_runs:?}"
    );
    assert!(
        !label_runs.contains(&"in") && !label_runs.contains(&"3 4"),
        "active edit text must not be split around whitespace because the cursor is measured against the unsplit buffer: {label_runs:?}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_places_committed_node_tail_after_measured_space_width() {
    let label = "in 7 8".to_string();
    let measurer = FixedWidthTextMeasurer;
    let measure_ctx = MeasureCtx {
        text_measurer: Some(&measurer),
        cell_w: 10.0,
        cell_h: 20.0,
        inherited_font_size: NODE_FONT_SIZE,
    };
    cache_char_widths(label, NODE_FONT_SIZE, &measure_ctx);

    let patch = Patch {
        nodes: vec![PatchNode {
            id: "committed-space-node".to_string(),
            op: "in".to_string(),
            kind: NodeKind::Builtin,
            label: "in".to_string(),
            args: vec![
                ArgValue::Literal("7".to_string()),
                ArgValue::Literal("8".to_string()),
            ],
            outputs: vec!["out".to_string()],
            position: (2.0, 2.0),
            diagnostic: None,
            source: None,
        }],
        connections: Vec::new(),
        macros: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 800.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 40.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        &PatcherPanState::default(),
        &PatcherInteractionState::default(),
    );

    let head = prims.iter().find_map(|prim| match prim {
        MetalPrimitive::ProportionalText(text) if text.text == "in" => Some(text.col),
        _ => None,
    });
    let tail = prims.iter().find_map(|prim| match prim {
        MetalPrimitive::ProportionalText(text) if text.text == "7 8" => Some(text.col),
        _ => None,
    });
    let head = head.expect("committed in node should render head text");
    let tail = tail.expect("committed in node should render tail text");

    assert_eq!(
        tail - head,
        2.5 * DEFAULT_ZOOM,
        "tail should start after the measured width of `in `, not after a fixed visual gap"
    );
}
