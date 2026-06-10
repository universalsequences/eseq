use super::super::WidgetDefinition;
use super::super::WidgetKeyEvent;
use super::super::text_input::{TextInputState, cache_char_widths, text_width_from_char_cache};
use super::alignment::*;
use super::display::*;
use super::emit::{emit_patch_debug_lisp, emit_patch_debug_lisp_for_view};
use super::geometry::*;
use super::interaction::*;
use super::metrics::*;
use super::model::{
    CableEndpoint, InputPortRef, InputPresentation, OutputPortRef,
    connection_touches_hidden_inline_node, hidden_inline_node_ids,
};
use super::project::dgenlisp_operator_names;
use super::render::*;
use super::state::*;
use super::writeback::{
    WriteBackError, emit_patch_writeback, emit_patch_writeback_result,
    emit_patch_writeback_result_with_library,
};
use super::*;
use crate::defmacro_library::DefmacroLibrary;
use crate::editor::{Editor, EditorConfig};
use crate::layout::{LayoutNode, MeasureCtx, Rect, TextMeasurer};
use crate::runtime::Runtime;
use crate::theme;
use crate::vm::Value;
use crossterm::event::{KeyCode, KeyModifiers};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn parse(source: &str) -> Patch {
    parse_patch_source(source, PatcherIntent::Instrument).unwrap()
}

fn temp_defmacro_library(name: &str, packages: &[(&str, &str)]) -> DefmacroLibrary {
    let root = std::env::temp_dir().join(format!(
        "eseq-patcher-defmacro-library-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    for (package, source) in packages {
        let dir = root.join(package);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("macro.lisp"), source).unwrap();
    }
    DefmacroLibrary::load(root).unwrap()
}

#[test]
fn parse_patch_source_with_library_projects_imported_macro_instance() {
    let library = temp_defmacro_library("project", &[("shape", "(defmacro shape (x) (* x 2))")]);
    let patch = parse_patch_source_with_library(
        "(use-defmacro shape)\n(def input (in 1))\n(def out1 (shape input))\n(out out1 1)",
        PatcherIntent::Instrument,
        &library,
    )
    .unwrap();

    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "shape")
        .expect("library macro patch");
    assert!(matches!(macro_patch.origin, MacroOrigin::Library { .. }));
    assert!(
        !macro_patch.patch.nodes.is_empty(),
        "library macro should project to an enterable macro view"
    );
    assert!(
        patch
            .nodes
            .iter()
            .any(|node| node.op == "shape" && node.kind == NodeKind::MacroInstance)
    );
    assert!(patch.diagnostics.is_empty());
}

#[test]
fn writeback_with_library_adds_import_for_used_library_macro() {
    let library = temp_defmacro_library("writeback", &[("shape", "(defmacro shape (x) (* x 2))")]);
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_node(&mut state, "root", (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &created))
        .unwrap()
        .text = "shape".to_string();
    let source = "(def input (in 1))\n(out input 1)";
    let emitted = emit_patch_writeback_result_with_library(
        source,
        PatcherIntent::Instrument,
        &state,
        &library,
    )
    .unwrap()
    .source;

    assert!(emitted.contains("(use-defmacro shape)"));
}

#[test]
fn staged_library_macro_edits_overlay_compile_library_without_writing_package() {
    let library = temp_defmacro_library(
        "staged-overlay",
        &[("shape", "(defmacro shape (x) (* x 2))")],
    );
    let source = "(use-defmacro shape)\n(def input (in 1))\n(def out1 (shape input))\n(out out1 1)";
    let root_patch =
        parse_patch_source_with_library(source, PatcherIntent::Instrument, &library).unwrap();
    let package = library.package("shape").unwrap();
    let package_source_before = fs::read_to_string(&package.source_path).unwrap();
    let mut state = PatcherInteractionState {
        active_macro: Some("shape".to_string()),
        ..PatcherInteractionState::default()
    };
    state.edit_state.nodes.insert(
        node_edit_key("macro:shape", "return"),
        PatcherNodeEdit {
            view_key: "macro:shape".to_string(),
            id: "return".to_string(),
            origin: PatcherNodeOrigin::Source {
                source_node_id: "return".to_string(),
            },
            text: "* 3".to_string(),
            position: (8.0, 8.0),
            width: None,
        },
    );

    let staged =
        library_with_staged_macro_edits(&root_patch, PatcherIntent::Instrument, &state, &library)
            .unwrap();
    let materialized = staged.materialize_source(source).unwrap().source;

    assert!(materialized.contains("(defmacro shape (x) (* x 3.0))"));
    assert_eq!(
        fs::read_to_string(&package.source_path).unwrap(),
        package_source_before,
        "preview staging must not write library package source"
    );
}

#[test]
fn patcher_writeback_payload_stages_library_macro_edits_without_autosave() {
    let library = temp_defmacro_library(
        "payload-stage",
        &[("shape", "(defmacro shape (x) (* x 2))")],
    );
    let package = library.package("shape").unwrap();
    let path = temp_patcher_dsp_path("patcher-payload-stage-library-macro");
    fs::write(
        &path,
        "(use-defmacro shape)\n(def input (in 1))\n(def out1 (shape input))\n(out out1 1)",
    )
    .unwrap();
    let mut node = patcher_test_node(&path);
    node.props.insert(
        "defmacro-library-root".to_string(),
        Value::String(library.root().display().to_string()),
    );
    let package_source_before = fs::read_to_string(&package.source_path).unwrap();
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState {
        active_macro: Some("shape".to_string()),
        ..PatcherInteractionState::default()
    };
    state.edit_state.nodes.insert(
        node_edit_key("macro:shape", "return"),
        PatcherNodeEdit {
            view_key: "macro:shape".to_string(),
            id: "return".to_string(),
            origin: PatcherNodeOrigin::Source {
                source_node_id: "return".to_string(),
            },
            text: "* 3".to_string(),
            position: (8.0, 8.0),
            width: None,
        },
    );
    set_patcher_interaction_state(key, state);

    let Value::Map(payload) = patcher_writeback_payload(&node) else {
        panic!("expected payload map");
    };
    let status = payload
        .get("status")
        .and_then(|value| match &*value.borrow() {
            Value::Keyword(value) | Value::String(value) => Some(value.clone()),
            _ => None,
        })
        .unwrap();
    let source = payload
        .get("source")
        .and_then(|value| match &*value.borrow() {
            Value::String(value) => Some(value.clone()),
            _ => None,
        })
        .unwrap();
    let compile_source = payload
        .get("compile-source")
        .and_then(|value| match &*value.borrow() {
            Value::String(value) => Some(value.clone()),
            _ => None,
        })
        .unwrap();

    assert_eq!(status, "valid");
    assert!(source.contains("(use-defmacro shape)"));
    assert!(!source.contains("(defmacro shape"));
    assert!(compile_source.contains("(defmacro shape (x) (* x 3.0))"));
    assert_eq!(
        fs::read_to_string(&package.source_path).unwrap(),
        package_source_before,
        "preview payload generation must not write library package source"
    );
}

#[test]
fn missing_macro_return_projects_as_recoverable_out_node() {
    let source = "(defmacro simp (input) __patcher_missing_input__)";
    let patch = parse_patch_source(source, PatcherIntent::Instrument).unwrap();
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "simp")
        .unwrap();

    assert!(
        !macro_patch
            .patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::CodeIsland),
        "bare missing return sentinel should not become a code island"
    );
    let out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    assert_eq!(node_display_label(out), "out 1");
    let input_indices = patch_input_indices(&macro_patch.patch);
    assert_eq!(
        input_indices.get(&out.id).map(Vec::as_slice),
        Some(&[0][..])
    );
}

#[test]
fn writeback_can_reconnect_missing_macro_return_out_node() {
    let source = "(defmacro simp (input) __patcher_missing_input__)";
    let mut state = PatcherInteractionState {
        active_macro: Some("simp".to_string()),
        ..PatcherInteractionState::default()
    };
    state.edit_state.connections.insert(
        connection_edit_key("macro:simp", "created-cable-0"),
        PatcherConnectionEdit {
            view_key: "macro:simp".to_string(),
            id: "created-cable-0".to_string(),
            origin: PatcherConnectionOrigin::Created {
                created_id: "created-cable-0".to_string(),
            },
            from: OutputPortRef {
                node_id: "input".to_string(),
                output_index: 0,
            },
            to: InputPortRef {
                node_id: "out".to_string(),
                input_index: 0,
            },
            kind: ConnectionKind::Forward,
            segment: None,
        },
    );

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    assert_eq!(emitted, "(defmacro simp (input) input)");
}

#[test]
fn library_macro_view_edit_persists_to_library_source_not_root_source() {
    let library = temp_defmacro_library("view-edit", &[("shape", "(defmacro shape (x) (* x 2))")]);
    let source = "(use-defmacro shape)\n(def input (in 1))\n(def out1 (shape input))\n(out out1 1)";
    let root_patch =
        parse_patch_source_with_library(source, PatcherIntent::Instrument, &library).unwrap();
    let package = library.package("shape").unwrap();
    let mut state = PatcherInteractionState {
        active_macro: Some("shape".to_string()),
        ..PatcherInteractionState::default()
    };
    let macro_patch = active_patcher_patch(&root_patch, &state);
    let return_node = macro_patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .expect("library macro should expose a return node");
    set_node_edit_position(
        &mut state,
        "macro:shape",
        return_node,
        (12.0, 34.0),
        node_display_label(return_node),
    );
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("macro:shape", &return_node.id))
        .unwrap()
        .text = "* 3".to_string();

    let persisted =
        persist_library_macro_edits(&root_patch, PatcherIntent::Instrument, &state, &library)
            .unwrap();
    assert_eq!(persisted, vec!["shape".to_string()]);

    let library_source = fs::read_to_string(&package.source_path).unwrap();
    assert_eq!(library_source, "(defmacro shape (x) (* x 3.0))");
    let layout: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&package.layout_path).unwrap()).unwrap();
    assert_eq!(layout["macros"]["shape"]["nodes"]["return"]["x"], 12.0);
    assert_eq!(layout["macros"]["shape"]["nodes"]["return"]["y"], 34.0);
    let reloaded_library = DefmacroLibrary::load(library.root()).unwrap();
    let reloaded_patch =
        parse_patch_source_with_library(source, PatcherIntent::Instrument, &reloaded_library)
            .unwrap();
    let reloaded_return = reloaded_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "shape")
        .unwrap()
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .unwrap();
    assert_eq!(reloaded_return.position, (12.0, 34.0));
    let root_state = interaction_state_without_library_macro_views(&state, &root_patch);
    let root_source = emit_patch_writeback_result_with_library(
        source,
        PatcherIntent::Instrument,
        &root_state,
        &library,
    )
    .unwrap()
    .source;
    assert_eq!(root_source, source);
}

#[test]
fn library_macro_text_edit_preserves_existing_package_layout_for_untouched_nodes() {
    let library = temp_defmacro_library(
        "view-edit-preserve-layout",
        &[("shape", "(defmacro shape (x) (* x 2))")],
    );
    let package = library.package("shape").unwrap();
    fs::write(
        &package.layout_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "version": 1,
            "root": {},
            "macros": {
                "shape": {
                    "nodes": {
                        "x": { "x": 41.0, "y": 7.0 },
                        "return": { "x": 52.0, "y": 13.0 },
                        "out": { "x": 63.0, "y": 19.0 }
                    }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let library = DefmacroLibrary::load(library.root()).unwrap();
    let source = "(use-defmacro shape)\n(def input (in 1))\n(def out1 (shape input))\n(out out1 1)";
    let root_patch =
        parse_patch_source_with_library(source, PatcherIntent::Instrument, &library).unwrap();
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "shape")
        .unwrap();
    let return_node = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .unwrap();
    assert_eq!(return_node.position, (52.0, 13.0));

    let mut state = PatcherInteractionState {
        active_macro: Some("shape".to_string()),
        ..PatcherInteractionState::default()
    };
    state.edit_state.nodes.insert(
        node_edit_key("macro:shape", "return"),
        PatcherNodeEdit {
            view_key: "macro:shape".to_string(),
            id: "return".to_string(),
            origin: PatcherNodeOrigin::Source {
                source_node_id: "return".to_string(),
            },
            text: "* 5".to_string(),
            position: return_node.position,
            width: return_node.width,
        },
    );

    let persisted =
        persist_library_macro_edits(&root_patch, PatcherIntent::Instrument, &state, &library)
            .unwrap();
    assert_eq!(persisted, vec!["shape".to_string()]);

    let layout: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&package.layout_path).unwrap()).unwrap();
    assert_eq!(layout["macros"]["shape"]["nodes"]["x"]["x"], 41.0);
    assert_eq!(layout["macros"]["shape"]["nodes"]["x"]["y"], 7.0);
    assert_eq!(layout["macros"]["shape"]["nodes"]["out"]["x"], 63.0);
    assert_eq!(layout["macros"]["shape"]["nodes"]["out"]["y"], 19.0);
    assert_eq!(layout["macros"]["shape"]["nodes"]["return"]["x"], 52.0);
    assert_eq!(layout["macros"]["shape"]["nodes"]["return"]["y"], 13.0);
}

#[test]
fn library_macro_autosave_consumes_live_macro_edit_overlay_after_persist() {
    let library = temp_defmacro_library(
        "view-edit-consumes-overlay",
        &[("shape", "(defmacro shape (x) (* x 2))")],
    );
    let source = "(use-defmacro shape)\n(def input (in 1))\n(def out1 (shape input))\n(out out1 1)";
    let root_patch =
        parse_patch_source_with_library(source, PatcherIntent::Instrument, &library).unwrap();
    let mut state = PatcherInteractionState {
        active_macro: Some("shape".to_string()),
        ..PatcherInteractionState::default()
    };
    state.selected_nodes.insert("created-0".to_string());
    state.z_order.insert(
        "macro:shape".to_string(),
        vec![
            "x".to_string(),
            "return".to_string(),
            "created-0".to_string(),
        ],
    );
    state.edit_state.nodes.insert(
        node_edit_key("macro:shape", "created-0"),
        PatcherNodeEdit {
            view_key: "macro:shape".to_string(),
            id: "created-0".to_string(),
            origin: PatcherNodeOrigin::Created {
                created_id: "created-0".to_string(),
            },
            text: "in 2 @name shape".to_string(),
            position: (50.0, 12.0),
            width: None,
        },
    );

    let persisted =
        persist_library_macro_edits(&root_patch, PatcherIntent::Instrument, &state, &library)
            .unwrap();
    clear_persisted_macro_view_edits(&mut state, &persisted);

    assert_eq!(state.active_macro.as_deref(), Some("shape"));
    assert!(
        state.edit_state.nodes.is_empty(),
        "persisted created node overlay should be consumed after library autosave"
    );
    assert!(
        state.selected_nodes.is_empty(),
        "selection should not keep pointing at consumed overlay nodes"
    );
    assert!(
        !state.z_order.contains_key("macro:shape"),
        "macro z-order should be rebuilt from persisted source after autosave"
    );

    let package = library.package("shape").unwrap();
    let library_source = fs::read_to_string(&package.source_path).unwrap();
    assert!(
        library_source.contains("(defmacro shape (x shape)"),
        "created macro input should be persisted as a macro parameter"
    );
    let reloaded_library = DefmacroLibrary::load(library.root()).unwrap();
    let reloaded_patch =
        parse_patch_source_with_library(source, PatcherIntent::Instrument, &reloaded_library)
            .unwrap();
    let reloaded_shape = reloaded_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "shape")
        .unwrap();
    let shape_param_count = reloaded_shape
        .patch
        .nodes
        .iter()
        .filter(|node| node.id == "shape")
        .count();
    assert_eq!(
        shape_param_count, 1,
        "reloaded macro should contain the persisted parameter once, without the stale overlay duplicate"
    );
}

#[test]
fn library_macro_autosave_keeps_package_macro_when_adding_library_dependency() {
    let library = temp_defmacro_library(
        "view-edit-library-dependency",
        &[
            ("simp8", "(defmacro simp8 (x) (* x 8))"),
            ("simp10", "(defmacro simp10 (input) (* input 1))"),
        ],
    );
    let source =
        "(use-defmacro simp10)\n(def sig (in 1))\n(def shaped (simp10 sig))\n(out shaped 1)";
    let root_patch =
        parse_patch_source_with_library(source, PatcherIntent::Instrument, &library).unwrap();
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "simp10")
        .expect("library macro should project");
    let input = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let return_node = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .expect("macro should expose return node");
    let out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let return_to_out = macro_patch
        .patch
        .connections
        .iter()
        .find(|connection| connection.from_node == return_node.id && connection.to_node == out.id)
        .unwrap();

    let mut state = PatcherInteractionState {
        active_macro: Some("simp10".to_string()),
        ..PatcherInteractionState::default()
    };
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:simp10", &return_node.id));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:simp10",
            &source_connection_id(return_to_out),
        ));
    let dependency = allocate_created_text_node(&mut state, "macro:simp10", "simp8");
    connect_output_to_input(&mut state, "macro:simp10", &input.id, &dependency, 0);
    connect_output_to_input(&mut state, "macro:simp10", &dependency, &out.id, 0);

    let persisted =
        persist_library_macro_edits(&root_patch, PatcherIntent::Instrument, &state, &library)
            .unwrap();
    assert_eq!(persisted, vec!["simp10".to_string()]);

    let package = library.package("simp10").unwrap();
    let saved = fs::read_to_string(&package.source_path).unwrap();
    assert!(
        saved.contains("(use-defmacro simp8)"),
        "library dependency import should be saved:\n{saved}"
    );
    assert!(
        saved.contains("(defmacro simp10 (input)"),
        "package must keep its public defmacro:\n{saved}"
    );
    DefmacroPackage::from_source(&package.package_dir, "simp10", &saved).unwrap();
    DefmacroLibrary::load(library.root()).unwrap();
}

#[test]
fn save_local_macro_to_library_replaces_local_def_with_import() {
    let library = temp_defmacro_library("save-action", &[]);
    let path = temp_patcher_dsp_path("patcher-save-macro-to-library");
    fs::write(
        &path,
        "(defmacro shape (x) (* x 2))\n(def input (in 1))\n(def out1 (shape input))\n(out out1 1)",
    )
    .unwrap();
    let mut node = patcher_test_node(&path);
    node.props.insert(
        "defmacro-library-root".to_string(),
        Value::String(library.root().display().to_string()),
    );
    let mut state = PatcherInteractionState {
        active_macro: Some("shape".to_string()),
        ..PatcherInteractionState::default()
    };
    let root_patch = load_patch_from_props(&node.props).unwrap().1;
    let macro_patch = active_patcher_patch(&root_patch, &state);
    let return_node = macro_patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .unwrap();
    set_node_edit_position(
        &mut state,
        "macro:shape",
        return_node,
        (19.0, 23.0),
        node_display_label(return_node),
    );
    let source = fs::read_to_string(&path).unwrap();
    let result = emit_patch_writeback_result_with_library(
        &source,
        PatcherIntent::Instrument,
        &state,
        &library,
    )
    .unwrap();
    let layout = writeback_layout_for_source(
        &result.source,
        PatcherIntent::Instrument,
        &node.props,
        &root_patch,
        &state,
        &result.generated_node_ids,
    )
    .unwrap();
    let (source, _layout) = apply_macro_library_action(
        result.source,
        layout,
        &PatcherMacroLibraryAction {
            kind: PatcherMacroLibraryActionKind::SaveToLibrary,
            macro_name: "shape".to_string(),
        },
        &root_patch,
        PatcherIntent::Instrument,
        &node.props,
        &library,
        &state,
        &result.generated_node_ids,
    )
    .unwrap();

    assert!(source.contains("(use-defmacro shape)"));
    assert!(!source.contains("(defmacro shape"));
    let package_dir = library.root().join("shape");
    assert_eq!(
        fs::read_to_string(package_dir.join("macro.lisp")).unwrap(),
        "(defmacro shape (x) (* x 2.0))\n"
    );
    let package_layout: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(package_dir.join("macro.layout.json")).unwrap())
            .unwrap();
    assert_eq!(
        package_layout["macros"]["shape"]["nodes"]["return"]["x"],
        19.0
    );
    assert_eq!(
        package_layout["macros"]["shape"]["nodes"]["return"]["y"],
        23.0
    );
    assert!(package_dir.join("manifest.json").exists());
}

#[test]
fn fork_library_macro_to_local_replaces_import_with_defmacro_and_copies_layout() {
    let library =
        temp_defmacro_library("fork-action", &[("shape", "(defmacro shape (x) (* x 4))")]);
    let package = library.package("shape").unwrap();
    fs::write(
        &package.layout_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "version": 1,
            "root": {},
            "macros": {
                "shape": {
                    "nodes": {
                        "return": { "x": 31.0, "y": 37.0 }
                    }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let path = temp_patcher_dsp_path("patcher-fork-macro-to-local");
    fs::write(
        &path,
        "(use-defmacro shape)\n(def input (in 1))\n(def out1 (shape input))\n(out out1 1)",
    )
    .unwrap();
    let mut node = patcher_test_node(&path);
    node.props.insert(
        "defmacro-library-root".to_string(),
        Value::String(library.root().display().to_string()),
    );
    let state = PatcherInteractionState {
        active_macro: Some("shape".to_string()),
        ..PatcherInteractionState::default()
    };
    let root_patch = load_patch_from_props(&node.props).unwrap().1;
    let source = fs::read_to_string(&path).unwrap();
    let result = emit_patch_writeback_result_with_library(
        &source,
        PatcherIntent::Instrument,
        &state,
        &library,
    )
    .unwrap();
    let layout = writeback_layout_for_source(
        &result.source,
        PatcherIntent::Instrument,
        &node.props,
        &root_patch,
        &state,
        &result.generated_node_ids,
    )
    .unwrap();
    let (source, layout) = apply_macro_library_action(
        result.source,
        layout,
        &PatcherMacroLibraryAction {
            kind: PatcherMacroLibraryActionKind::Fork,
            macro_name: "shape".to_string(),
        },
        &root_patch,
        PatcherIntent::Instrument,
        &node.props,
        &library,
        &state,
        &result.generated_node_ids,
    )
    .unwrap();

    assert!(source.contains("(defmacro shape"));
    assert!(source.contains("(* x 4.0)"));
    assert!(!source.contains("(use-defmacro shape)"));
    let layout: serde_json::Value = serde_json::from_str(&layout).unwrap();
    assert_eq!(layout["macros"]["shape"]["nodes"]["return"]["x"], 31.0);
    assert_eq!(layout["macros"]["shape"]["nodes"]["return"]["y"], 37.0);
}

#[test]
fn root_layout_sidecar_excludes_library_macro_scopes() {
    let library = temp_defmacro_library(
        "root-layout-excludes-library",
        &[("shape", "(defmacro shape (x) (* x 2))")],
    );
    let source = "(use-defmacro shape)\n(def input (in 1))\n(def out1 (shape input))\n(out out1 1)";
    let patch =
        parse_patch_source_with_library(source, PatcherIntent::Instrument, &library).unwrap();
    assert!(
        patch
            .macros
            .iter()
            .any(|macro_patch| matches!(macro_patch.origin, MacroOrigin::Library { .. }))
    );

    let layout = sidecar::current_layout_json(&patch, &PatcherInteractionState::default()).unwrap();
    let layout: serde_json::Value = serde_json::from_str(&layout).unwrap();

    assert!(layout["macros"].get("shape").is_none());
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

#[cfg(target_os = "macos")]
fn inner_prim(prim: &MetalPrimitive) -> &MetalPrimitive {
    crate::widget_render::innermost_primitive(prim)
}

#[cfg(target_os = "macos")]
fn effective_z(prim: &MetalPrimitive) -> i32 {
    crate::widget_render::effective_z_index(prim)
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
fn existing_layout_sidecar_preserves_positions_without_materializing_new_nodes() {
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
        "new source node can retain its projected fallback position"
    );
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    assert!(
        !saved["root"]["nodes"]
            .as_object()
            .unwrap()
            .contains_key("shaped"),
        "loading source with an existing sidecar must not materialize new node layout"
    );
}

#[test]
fn layout_sidecar_preserves_optional_node_widths() {
    let path = temp_patcher_dsp_path("patcher-sidecar-width");
    fs::write(
        &path,
        "(def pitch (in 1 @name pitch))\n(def phase (phasor pitch))\n(out phase 1 @name audio)",
    )
    .unwrap();
    load_patch_from_props(&patcher_props_for_path(&path)).unwrap();
    let sidecar_path = sidecar::sidecar_path_for_source(&path);
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    json["root"]["nodes"]["phase"] = serde_json::json!({ "x": 123.0, "y": 45.0, "width": 24.0 });
    json["root"]["nodes"]["pitch"] = serde_json::json!({ "x": 10.0, "y": 11.0 });
    fs::write(&sidecar_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    let (_path, patch) = load_patch_from_props(&patcher_props_for_path(&path)).unwrap();
    let phase = patch.nodes.iter().find(|node| node.id == "phase").unwrap();
    let pitch = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
    assert_eq!(phase.position, (123.0, 45.0));
    assert_eq!(phase.width, Some(24.0));
    assert_eq!(pitch.position, (10.0, 11.0));
    assert_eq!(
        pitch.width, None,
        "old sidecar node layouts without width should still load"
    );

    let layout = sidecar::current_layout_json(&patch, &PatcherInteractionState::default()).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&layout).unwrap();
    assert_eq!(saved["root"]["nodes"]["phase"]["width"], 24.0);
    assert!(
        saved["root"]["nodes"]["pitch"].get("width").is_none(),
        "autogenerated-width nodes should not persist redundant width metadata"
    );
}

#[test]
fn z_order_initializes_from_patch_nodes() {
    let patch = parse("(def a (in 1))\n(def b (phasor a))\n(out b)");
    let mut state = PatcherInteractionState::default();

    sync_patcher_z_order(&mut state, "root", &patch);

    assert_eq!(
        state.z_order.get("root").cloned().unwrap_or_default(),
        patch
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn z_order_removes_deleted_nodes_and_appends_created_nodes() {
    let patch = parse("(def a (in 1))\n(def b (phasor a))\n(out b)");
    let mut state = PatcherInteractionState::default();
    state.z_order.insert(
        "root".to_string(),
        vec!["deleted".to_string(), "b".to_string()],
    );

    sync_patcher_z_order(&mut state, "root", &patch);

    assert_eq!(
        state.z_order.get("root").cloned().unwrap_or_default(),
        vec!["b".to_string(), "a".to_string(), "out#0".to_string()]
    );
}

#[test]
fn bring_nodes_to_front_preserves_relative_stack_order() {
    let mut state = PatcherInteractionState::default();
    state.z_order.insert(
        "root".to_string(),
        vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ],
    );

    bring_nodes_to_front(&mut state, "root", &["c".to_string(), "a".to_string()]);

    assert_eq!(
        state.z_order.get("root").cloned().unwrap_or_default(),
        vec![
            "b".to_string(),
            "d".to_string(),
            "a".to_string(),
            "c".to_string()
        ]
    );
}

#[test]
fn created_node_appends_to_top_of_active_view_z_order() {
    let mut state = PatcherInteractionState::default();
    state
        .z_order
        .insert("macro:shape".to_string(), vec!["existing".to_string()]);

    let created = allocate_created_node(&mut state, "macro:shape", (1.0, 1.0));

    assert_eq!(
        state
            .z_order
            .get("macro:shape")
            .cloned()
            .unwrap_or_default(),
        vec!["existing".to_string(), created]
    );
}

#[test]
fn hit_testing_uses_z_order_not_patch_vector_order() {
    let mut patch = parse("(def a (in 1))\n(def b (phasor a))");
    for node in &mut patch.nodes {
        node.position = (2.0, 2.0);
    }
    let mut state = PatcherInteractionState::default();
    sync_patcher_z_order(&mut state, "root", &patch);
    bring_nodes_to_front(&mut state, "root", &["a".to_string()]);
    let ordered = ordered_patch_nodes(&patch, &state, "root");

    let node_rect = patch_node_rects(
        &patch,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        &PatcherPanState::default(),
    )
    .get("a")
    .copied()
    .expect("a rect");
    let hit = hit_patcher_node(
        &patch,
        &ordered,
        Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        },
        &PatcherPanState::default(),
        node_rect.col + node_rect.width * 0.5,
        node_rect.row + node_rect.height * 0.5,
    );

    assert_eq!(hit.as_deref(), Some("a"));
}

#[test]
fn pointer_down_bring_node_to_front() {
    let path = temp_patcher_source_path("patcher-z-pointer-down");
    fs::write(&path, "(def a (in 1))\n(def b (phasor a))").expect("write source");
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let (_, patch) = load_patch_from_props(&node.props).expect("load patch");
    let mut state = PatcherInteractionState::default();
    sync_patcher_z_order(&mut state, "root", &patch);
    set_patcher_interaction_state(key, state);
    let a_rect = patch_node_rects(&patch, node.rect, &PatcherPanState::default())
        .get("a")
        .copied()
        .expect("a rect");

    handle_patcher_pointer_down(
        &node,
        a_rect.col + a_rect.width * 0.5,
        a_rect.row + a_rect.height * 0.5,
        KeyModifiers::NONE,
        10.0,
        20.0,
    );

    let state = get_patcher_interaction_state(key);
    assert_eq!(
        state
            .z_order
            .get("root")
            .and_then(|stack| stack.last())
            .map(String::as_str),
        Some("a")
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
        saved["root"]["nodes"]
            .as_object()
            .unwrap()
            .contains_key("missing"),
        "valid existing sidecars are user-authored layout and should not be rewritten on load"
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

fn set_patch_node_position(patch: &mut Patch, node_id: &str, position: (f32, f32)) {
    patch
        .nodes
        .iter_mut()
        .find(|node| node.id == node_id)
        .unwrap_or_else(|| panic!("missing node {node_id}"))
        .position = position;
}

fn patch_node_position(patch: &Patch, node_id: &str) -> (f32, f32) {
    patch
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .unwrap_or_else(|| panic!("missing node {node_id}"))
        .position
}

#[derive(Clone, Debug)]
struct ExpectedPersistenceNode {
    view_key: String,
    node_id: String,
    position: (f32, f32),
}

#[derive(Clone, Debug)]
struct ExpectedPersistenceConnection {
    view_key: String,
    from_node: String,
    from_output: usize,
    to_node: String,
    to_input: usize,
}

#[derive(Clone, Debug)]
struct ExpectedPersistenceSegment {
    view_key: String,
    from_node: String,
    from_output: usize,
    to_node: String,
    to_input: usize,
    segment_row: f32,
}

#[derive(Clone, Debug, Default)]
struct PersistenceExpectations {
    nodes: Vec<ExpectedPersistenceNode>,
    connections: Vec<ExpectedPersistenceConnection>,
    segments: Vec<ExpectedPersistenceSegment>,
    source_contains: Vec<String>,
    source_not_contains: Vec<String>,
}

fn expected_node(view_key: &str, node_id: &str, position: (f32, f32)) -> ExpectedPersistenceNode {
    ExpectedPersistenceNode {
        view_key: view_key.to_string(),
        node_id: node_id.to_string(),
        position,
    }
}

fn expected_connection(
    view_key: &str,
    from_node: &str,
    from_output: usize,
    to_node: &str,
    to_input: usize,
) -> ExpectedPersistenceConnection {
    ExpectedPersistenceConnection {
        view_key: view_key.to_string(),
        from_node: from_node.to_string(),
        from_output,
        to_node: to_node.to_string(),
        to_input,
    }
}

fn expected_segment(
    view_key: &str,
    from_node: &str,
    from_output: usize,
    to_node: &str,
    to_input: usize,
    segment_row: f32,
) -> ExpectedPersistenceSegment {
    ExpectedPersistenceSegment {
        view_key: view_key.to_string(),
        from_node: from_node.to_string(),
        from_output,
        to_node: to_node.to_string(),
        to_input,
        segment_row,
    }
}

fn generated_persistence_source(seed: u64) -> String {
    let inline_amount = match seed % 4 {
        0 => "0.125",
        1 => "0.25",
        2 => "0.5",
        _ => "0.75",
    };
    let nested_tail = match seed % 3 {
        0 => "(def folded (fold shaped 0.33))",
        1 => "(def folded (fold (* shaped 0.8) 0.33))",
        _ => "(def folded (fold (mix shaped phase 0.2) 0.33))",
    };
    format!(
        "(defmacro fold (sig amt) (def shaped (mix sig amt {inline_amount})) shaped)\n\
         (def gate (in 1 @name gate))\n\
         (def pitch (in 2 @name pitch))\n\
         (def velocity (in 3 @name velocity))\n\
         (def trigger (in 4 @name trigger))\n\
         (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)\n\
         (def rate 0.5)\n\
         (def env (adsr gate trigger 5 120 0.8 180))\n\
         (def phase (phasor pitch))\n\
         (def tri (triangle phase 0.1))\n\
         (def shaped (* tri (mod gain)))\n\
         {nested_tail}\n\
         (out folded 1 @name audio)\n"
    )
}

fn persistence_position(seed: u64, ordinal: usize, scope_bias: f32) -> (f32, f32) {
    let x = 7.25 + scope_bias + ((seed as usize * 13 + ordinal * 17) % 89) as f32 + 0.125;
    let y = 5.5 + scope_bias * 0.5 + ((seed as usize * 19 + ordinal * 11) % 61) as f32 + 0.375;
    (x, y)
}

fn move_all_persistence_nodes(
    state: &mut PatcherInteractionState,
    view_key: &str,
    patch: &Patch,
    seed: u64,
    scope_bias: f32,
    expectations: &mut PersistenceExpectations,
) {
    let hidden_node_ids = hidden_inline_node_ids(patch);
    for (idx, node) in patch.nodes.iter().enumerate() {
        if hidden_node_ids.contains(&node.id) {
            continue;
        }
        let position = persistence_position(seed, idx, scope_bias);
        set_node_edit_position(state, view_key, node, position, node_display_label(node));
        expectations
            .nodes
            .push(expected_node(view_key, &node.id, position));
    }
}

fn visible_persistence_connections(patch: &Patch) -> Vec<&PatchConnection> {
    let hidden_node_ids = hidden_inline_node_ids(patch);
    patch
        .connections
        .iter()
        .filter(|connection| !connection_touches_hidden_inline_node(connection, &hidden_node_ids))
        .collect()
}

fn set_created_node_position(
    state: &mut PatcherInteractionState,
    view_key: &str,
    node_id: &str,
    position: (f32, f32),
) {
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key(view_key, node_id))
        .unwrap_or_else(|| panic!("missing created node edit {view_key}::{node_id}"))
        .position = position;
}

fn source_connection_for_input<'a>(
    patch: &'a Patch,
    to_node: &str,
    to_input: usize,
) -> &'a PatchConnection {
    patch
        .connections
        .iter()
        .find(|connection| connection.to_node == to_node && connection.to_input == to_input)
        .unwrap_or_else(|| panic!("missing source connection into {to_node}:{to_input}"))
}

fn source_connection_for_input_opt<'a>(
    patch: &'a Patch,
    to_node: &str,
    to_input: usize,
) -> Option<&'a PatchConnection> {
    patch
        .connections
        .iter()
        .find(|connection| connection.to_node == to_node && connection.to_input == to_input)
}

fn persistence_patch_for_view<'a>(patch: &'a Patch, view_key: &str) -> &'a Patch {
    if view_key == "root" {
        return patch;
    }
    let macro_name = view_key
        .strip_prefix("macro:")
        .unwrap_or_else(|| panic!("unsupported view key {view_key}"));
    &patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == macro_name)
        .unwrap_or_else(|| panic!("missing macro patch {macro_name}"))
        .patch
}

fn persistence_payload_source_and_layout(node: &LayoutNode, case_name: &str) -> (String, String) {
    let payload = patcher_writeback_payload(node);
    let Value::Map(map) = payload else {
        panic!("{case_name}: expected writeback payload map");
    };
    assert_eq!(
        map.get("status").map(|value| value.borrow().clone()),
        Some(Value::Keyword("valid".to_string())),
        "{case_name}: expected valid writeback payload, got {map:?}"
    );
    let source = match map.get("source").map(|value| value.borrow().clone()) {
        Some(Value::String(source)) => source,
        other => panic!("{case_name}: expected emitted source string, got {other:?}"),
    };
    let layout = match map.get("layout").map(|value| value.borrow().clone()) {
        Some(Value::String(layout)) => layout,
        other => panic!("{case_name}: expected emitted layout string, got {other:?}"),
    };
    (source, layout)
}

fn assert_close_position(
    case_name: &str,
    view_key: &str,
    node_id: &str,
    actual: (f32, f32),
    expected: (f32, f32),
) {
    assert!(
        (actual.0 - expected.0).abs() < 0.0001 && (actual.1 - expected.1).abs() < 0.0001,
        "{case_name}: {view_key}::{node_id} position changed: expected {expected:?}, got {actual:?}"
    );
}

fn assert_persistence_expectations(
    case_name: &str,
    emitted_source: &str,
    emitted_layout: &str,
    reloaded: &Patch,
    expectations: &PersistenceExpectations,
) {
    for needle in &expectations.source_contains {
        assert!(
            emitted_source.contains(needle),
            "{case_name}: emitted source did not contain `{needle}`:\n{emitted_source}"
        );
    }
    for needle in &expectations.source_not_contains {
        assert!(
            !emitted_source.contains(needle),
            "{case_name}: emitted source unexpectedly contained `{needle}`:\n{emitted_source}"
        );
    }

    let layout_json: serde_json::Value =
        serde_json::from_str(emitted_layout).unwrap_or_else(|error| {
            panic!("{case_name}: emitted layout should be valid JSON: {error}")
        });
    for expected in &expectations.nodes {
        let layout_node = if expected.view_key == "root" {
            &layout_json["root"]["nodes"][&expected.node_id]
        } else {
            let macro_name = expected.view_key.strip_prefix("macro:").unwrap();
            &layout_json["macros"][macro_name]["nodes"][&expected.node_id]
        };
        let layout_x = layout_node["x"]
            .as_f64()
            .unwrap_or_else(|| panic!("{case_name}: missing layout x for {:?}", expected));
        let layout_y = layout_node["y"]
            .as_f64()
            .unwrap_or_else(|| panic!("{case_name}: missing layout y for {:?}", expected));
        assert_close_position(
            case_name,
            &expected.view_key,
            &expected.node_id,
            (layout_x as f32, layout_y as f32),
            expected.position,
        );

        let patch = persistence_patch_for_view(reloaded, &expected.view_key);
        let actual = patch_node_position(patch, &expected.node_id);
        assert_close_position(
            case_name,
            &expected.view_key,
            &expected.node_id,
            actual,
            expected.position,
        );
    }

    for expected in &expectations.connections {
        let patch = persistence_patch_for_view(reloaded, &expected.view_key);
        assert!(
            patch.connections.iter().any(|connection| {
                connection.from_node == expected.from_node
                    && connection.from_output == expected.from_output
                    && connection.to_node == expected.to_node
                    && connection.to_input == expected.to_input
            }),
            "{case_name}: missing expected connection {:?}; connections={:?}",
            expected,
            patch
                .connections
                .iter()
                .map(source_connection_id)
                .collect::<Vec<_>>()
        );
    }

    for expected in &expectations.segments {
        let patch = persistence_patch_for_view(reloaded, &expected.view_key);
        let connection = patch
            .connections
            .iter()
            .find(|connection| {
                connection.from_node == expected.from_node
                    && connection.from_output == expected.from_output
                    && connection.to_node == expected.to_node
                    && connection.to_input == expected.to_input
            })
            .unwrap_or_else(|| panic!("{case_name}: missing segmented connection {expected:?}"));
        let segment = connection
            .segment
            .unwrap_or_else(|| panic!("{case_name}: missing segment for {expected:?}"));
        assert!(
            segment.is_segmented,
            "{case_name}: segment should be enabled"
        );
        assert!(
            (segment.segment_row - expected.segment_row).abs() < 0.0001,
            "{case_name}: segment row changed for {:?}: expected {}, got {}",
            expected,
            expected.segment_row,
            segment.segment_row
        );
    }
}

fn build_persistence_case(
    seed: u64,
    path: &std::path::Path,
) -> (LayoutNode, PatcherInteractionState, PersistenceExpectations) {
    fs::write(path, generated_persistence_source(seed)).unwrap();
    let node = patcher_test_node(path);
    let (_path, root_patch) = load_patch_from_props(&node.props).unwrap();
    let mode = seed % 8;
    let mut state = PatcherInteractionState::default();
    let mut expectations = PersistenceExpectations::default();

    match mode {
        0 => {
            move_all_persistence_nodes(
                &mut state,
                "root",
                &root_patch,
                seed,
                0.0,
                &mut expectations,
            );
            for macro_patch in &root_patch.macros {
                move_all_persistence_nodes(
                    &mut state,
                    &format!("macro:{}", macro_patch.name),
                    &macro_patch.patch,
                    seed + 17,
                    45.0,
                    &mut expectations,
                );
            }
        }
        1 => {
            let literal_position = persistence_position(seed, 0, 10.0);
            let phasor_position = persistence_position(seed, 1, 10.0);
            let literal = allocate_created_text_node(&mut state, "root", "0.5");
            let phasor = allocate_created_text_node(&mut state, "root", "phasor trigger");
            set_created_node_position(&mut state, "root", &literal, literal_position);
            set_created_node_position(&mut state, "root", &phasor, phasor_position);
            if let Some(old) = source_connection_for_input_opt(&root_patch, "tri", 1) {
                state
                    .edit_state
                    .deleted_connections
                    .insert(connection_edit_key("root", &source_connection_id(old)));
            }
            connect_output_to_input(&mut state, "root", &literal, &phasor, 0);
            connect_output_to_input(&mut state, "root", &phasor, "tri", 1);
            expectations
                .nodes
                .push(expected_node("root", "value1", literal_position));
            expectations
                .nodes
                .push(expected_node("root", "phasor1", phasor_position));
            expectations
                .connections
                .push(expected_connection("root", "value1", 0, "phasor1", 0));
            expectations
                .connections
                .push(expected_connection("root", "trigger", 0, "phasor1", 1));
            expectations
                .connections
                .push(expected_connection("root", "phasor1", 0, "tri", 1));
            expectations
                .source_contains
                .push("(def value1 0.5)".to_string());
            expectations
                .source_contains
                .push("(def phasor1 (phasor value1 trigger))".to_string());
            expectations
                .source_contains
                .push("(def tri (triangle phase phasor1))".to_string());
            expectations
                .source_not_contains
                .push("(def phasor1 (phasor 0.5".to_string());
        }
        2 => {
            let mul_position = persistence_position(seed, 0, 20.0);
            let cos_position = persistence_position(seed, 1, 20.0);
            let multiply = allocate_created_text_node(&mut state, "root", "* 2");
            let cosine = allocate_created_text_node(&mut state, "root", "cos");
            set_created_node_position(&mut state, "root", &multiply, mul_position);
            set_created_node_position(&mut state, "root", &cosine, cos_position);
            let old = source_connection_for_input(&root_patch, "tri", 0);
            state
                .edit_state
                .deleted_connections
                .insert(connection_edit_key("root", &source_connection_id(old)));
            connect_output_to_input(&mut state, "root", "phase", &multiply, 0);
            connect_output_to_input(&mut state, "root", &multiply, &cosine, 0);
            connect_output_to_input(&mut state, "root", &cosine, "tri", 0);
            expectations
                .nodes
                .push(expected_node("root", "mul1", mul_position));
            expectations
                .nodes
                .push(expected_node("root", "cos1", cos_position));
            expectations
                .connections
                .push(expected_connection("root", "phase", 0, "mul1", 0));
            expectations
                .connections
                .push(expected_connection("root", "mul1", 0, "cos1", 0));
            expectations
                .connections
                .push(expected_connection("root", "cos1", 0, "tri", 0));
            expectations
                .source_contains
                .push("(def mul1 (* phase 2.0))".to_string());
            expectations
                .source_contains
                .push("(def cos1 (cos mul1))".to_string());
            expectations
                .source_contains
                .push("(def tri (triangle cos1 0.1))".to_string());
        }
        3 => {
            for (idx, connection) in visible_persistence_connections(&root_patch)
                .into_iter()
                .take(4)
                .enumerate()
            {
                let row = 18.25 + seed as f32 * 0.5 + idx as f32 * 3.75;
                set_connection_segment_edit(
                    &mut state,
                    "root",
                    connection,
                    Some(CableSegmentInfo {
                        is_segmented: true,
                        segment_row: row,
                    }),
                );
                expectations.segments.push(expected_segment(
                    "root",
                    &connection.from_node,
                    connection.from_output,
                    &connection.to_node,
                    connection.to_input,
                    row,
                ));
            }
            move_all_persistence_nodes(
                &mut state,
                "root",
                &root_patch,
                seed,
                5.0,
                &mut expectations,
            );
        }
        4 => {
            let macro_patch = root_patch
                .macros
                .iter()
                .find(|macro_patch| macro_patch.name == "fold")
                .unwrap();
            let literal_position = persistence_position(seed, 0, 55.0);
            let phasor_position = persistence_position(seed, 1, 55.0);
            let literal = allocate_created_text_node(&mut state, "macro:fold", "0.125");
            let phasor = allocate_created_text_node(&mut state, "macro:fold", "phasor amt");
            set_created_node_position(&mut state, "macro:fold", &literal, literal_position);
            set_created_node_position(&mut state, "macro:fold", &phasor, phasor_position);
            if let Some(old) = source_connection_for_input_opt(&macro_patch.patch, "shaped", 2) {
                state
                    .edit_state
                    .deleted_connections
                    .insert(connection_edit_key(
                        "macro:fold",
                        &source_connection_id(old),
                    ));
            }
            connect_output_to_input(&mut state, "macro:fold", &literal, &phasor, 0);
            connect_output_to_input(&mut state, "macro:fold", &phasor, "shaped", 2);
            expectations
                .nodes
                .push(expected_node("macro:fold", "value1", literal_position));
            expectations
                .nodes
                .push(expected_node("macro:fold", "phasor1", phasor_position));
            expectations.connections.push(expected_connection(
                "macro:fold",
                "value1",
                0,
                "phasor1",
                0,
            ));
            expectations.connections.push(expected_connection(
                "macro:fold",
                "amt",
                0,
                "phasor1",
                1,
            ));
            expectations.connections.push(expected_connection(
                "macro:fold",
                "phasor1",
                0,
                "shaped",
                2,
            ));
            expectations
                .source_contains
                .push("(def value1 0.125)".to_string());
            expectations
                .source_contains
                .push("(def phasor1 (phasor value1 amt))".to_string());
            expectations
                .source_contains
                .push("(def shaped (mix sig amt phasor1))".to_string());
            expectations
                .source_not_contains
                .push("(def phasor1 (phasor 0.125".to_string());
        }
        5 => {
            let value_position = persistence_position(seed, 0, 25.0);
            let shaper_position = persistence_position(seed, 1, 25.0);
            let value = allocate_created_text_node(&mut state, "root", "0.875");
            let shaper = allocate_created_text_node(&mut state, "root", "mix phase 0.25");
            set_created_node_position(&mut state, "root", &value, value_position);
            set_created_node_position(&mut state, "root", &shaper, shaper_position);
            let old = source_connection_for_input(&root_patch, "shaped", 0);
            state
                .edit_state
                .deleted_connections
                .insert(connection_edit_key("root", &source_connection_id(old)));
            connect_output_to_input(&mut state, "root", &value, &shaper, 0);
            connect_output_to_input(&mut state, "root", &shaper, "shaped", 0);
            expectations
                .nodes
                .push(expected_node("root", "value1", value_position));
            expectations
                .nodes
                .push(expected_node("root", "mix1", shaper_position));
            expectations
                .connections
                .push(expected_connection("root", "value1", 0, "mix1", 0));
            expectations
                .connections
                .push(expected_connection("root", "phase", 0, "mix1", 1));
            expectations
                .connections
                .push(expected_connection("root", "mix1", 0, "shaped", 0));
            expectations
                .source_contains
                .push("(def mix1 (mix value1 phase 0.25))".to_string());
        }
        6 => {
            let rate = root_patch
                .nodes
                .iter()
                .find(|node| node.id == "rate")
                .expect("fixture should project source constant rate");
            let position = persistence_position(seed, 0, 35.0);
            set_node_edit_position(&mut state, "root", rate, position, node_display_label(rate));
            state
                .edit_state
                .nodes
                .get_mut(&node_edit_key("root", &rate.id))
                .unwrap()
                .text = "phasor".to_string();
            expectations
                .nodes
                .push(expected_node("root", "rate", position));
            expectations
                .source_contains
                .push("(def rate (phasor))".to_string());
            expectations
                .source_not_contains
                .push("(def rate phasor)".to_string());
        }
        _ => {
            move_all_persistence_nodes(
                &mut state,
                "root",
                &root_patch,
                seed,
                12.0,
                &mut expectations,
            );
            let macro_patch = root_patch
                .macros
                .iter()
                .find(|macro_patch| macro_patch.name == "fold")
                .unwrap();
            move_all_persistence_nodes(
                &mut state,
                "macro:fold",
                &macro_patch.patch,
                seed + 23,
                65.0,
                &mut expectations,
            );
            let connection = root_patch
                .connections
                .iter()
                .find(|connection| connection.to_node == "folded")
                .unwrap_or_else(|| root_patch.connections.first().unwrap());
            let row = 27.75 + seed as f32;
            set_connection_segment_edit(
                &mut state,
                "root",
                connection,
                Some(CableSegmentInfo {
                    is_segmented: true,
                    segment_row: row,
                }),
            );
            expectations.segments.push(expected_segment(
                "root",
                &connection.from_node,
                connection.from_output,
                &connection.to_node,
                connection.to_input,
                row,
            ));
        }
    }

    (node, state, expectations)
}

fn run_persistence_case(seed: u64) {
    let case_name = format!("persistence-seed-{seed}");
    let path = temp_patcher_dsp_path(&case_name);
    let (node, state, expectations) = build_persistence_case(seed, &path);
    let key = patcher_state_key(&node);
    set_patcher_interaction_state(key, state);

    let (source, layout) = persistence_payload_source_and_layout(&node, &case_name);
    fs::write(&path, &source).unwrap();
    fs::write(sidecar::sidecar_path_for_source(&path), &layout).unwrap();
    set_patcher_interaction_state(key, PatcherInteractionState::default());

    let (_path, reloaded) = load_patch_from_props(&node.props).unwrap();
    assert_persistence_expectations(&case_name, &source, &layout, &reloaded, &expectations);

    let second_layout =
        sidecar::current_layout_json(&reloaded, &PatcherInteractionState::default()).unwrap();
    let first_json: serde_json::Value = serde_json::from_str(&layout).unwrap();
    let second_json: serde_json::Value = serde_json::from_str(&second_layout).unwrap();
    assert_eq!(
        second_json, first_json,
        "{case_name}: reload/materialize changed persisted layout sidecar"
    );
}

fn run_patcher_persistence_fuzz(seed_count: u64) {
    for seed in 0..seed_count {
        run_persistence_case(seed);
    }
}

#[test]
fn patcher_persistence_fuzz_default() {
    run_patcher_persistence_fuzz(16);
}

#[test]
#[ignore = "larger deterministic patcher persistence corpus"]
fn patcher_persistence_fuzz_stress() {
    run_patcher_persistence_fuzz(96);
}

#[test]
fn reset_patcher_state_for_path_clears_registered_stable_widget_state() {
    let path = temp_patcher_dsp_path("patcher-reset-stable-key");
    fs::write(&path, "(out 0)").unwrap();
    let mut node = patcher_test_node(&path);
    node.stable_widget_id = Some(998_877);
    let key = patcher_state_key(&node);

    let mut state = PatcherInteractionState::default();
    state.selected_nodes.insert("stale-node".to_string());
    set_patcher_interaction_state(key, state);

    reset_patcher_state_for_path(&path, PatcherIntent::Instrument);

    assert!(
        get_patcher_interaction_state(key).selected_nodes.is_empty(),
        "mode reset should clear stable-widget keyed patcher interaction state"
    );
}

#[test]
fn reload_patcher_macro_view_for_path_keeps_macro_view_and_clears_edit_overlays() {
    let path = temp_patcher_dsp_path("patcher-reload-macro-view");
    fs::write(&path, "(defmacro simp (x) (phasor x))\n(out 0)").unwrap();
    let mut node = patcher_test_node(&path);
    node.stable_widget_id = Some(112_358);
    let key = patcher_state_key(&node);

    let mut state = PatcherInteractionState {
        active_macro: Some("simp".to_string()),
        ..PatcherInteractionState::default()
    };
    state.selected_nodes.insert("return".to_string());
    state.edit_state.nodes.insert(
        node_edit_key("macro:simp", "created-0"),
        PatcherNodeEdit {
            view_key: "macro:simp".to_string(),
            id: "created-0".to_string(),
            origin: PatcherNodeOrigin::Created {
                created_id: "created-0".to_string(),
            },
            text: "triangle".to_string(),
            position: (12.0, 14.0),
            width: None,
        },
    );
    set_patcher_interaction_state(key, state);

    reload_patcher_macro_view_for_path(&path, "simp");

    let state = get_patcher_interaction_state(key);
    assert_eq!(state.active_macro.as_deref(), Some("simp"));
    assert!(
        state.selected_nodes.is_empty(),
        "reload should clear stale selection"
    );
    assert!(
        state.edit_state.nodes.is_empty(),
        "reload should clear stale edited/created macro graph overlays"
    );
}

#[test]
fn active_macro_layout_merge_preserves_previous_positions_for_untouched_nodes() {
    let source = "(defmacro simp (x) (scale (triangle (phasor x)) 0 1 -1 1))\n(out 0)";
    let mut root_patch = parse_patch_source(source, PatcherIntent::Instrument).unwrap();
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "simp")
        .expect("simp macro");
    let first_node = macro_patch
        .patch
        .nodes
        .first()
        .expect("first macro node")
        .id
        .clone();
    let second_node = macro_patch
        .patch
        .nodes
        .get(1)
        .expect("second macro node")
        .clone();
    let previous_layout = serde_json::json!({
        "version": 1,
        "root": {},
        "macros": {
            "simp": {
                "nodes": {
                    first_node.clone(): { "x": 42.0, "y": 10.0 },
                    second_node.id.clone(): { "x": 77.0, "y": 20.0 }
                }
            }
        }
    })
    .to_string();
    sidecar::apply_layout_json(&previous_layout, "test previous layout", &mut root_patch).unwrap();

    let mut state = PatcherInteractionState {
        active_macro: Some("simp".to_string()),
        ..PatcherInteractionState::default()
    };
    state.edit_state.nodes.insert(
        node_edit_key("macro:simp", &second_node.id),
        PatcherNodeEdit {
            view_key: "macro:simp".to_string(),
            id: second_node.id.clone(),
            origin: PatcherNodeOrigin::Source {
                source_node_id: second_node.id.clone(),
            },
            text: node_display_label(&second_node),
            position: (99.0, 33.0),
            width: None,
        },
    );
    let merged_layout = sidecar::current_layout_json(&root_patch, &state).unwrap();
    let package_layout = layout_json_for_single_macro_scope(&merged_layout, "simp").unwrap();
    let package_layout: serde_json::Value = serde_json::from_str(&package_layout).unwrap();

    assert_eq!(
        package_layout["macros"]["simp"]["nodes"][&first_node]["x"], 42.0,
        "untouched macro node should keep its previous layout position"
    );
    assert_eq!(
        package_layout["macros"]["simp"]["nodes"][&second_node.id]["x"], 99.0,
        "edited macro node should use the live interaction position"
    );
}

#[test]
fn active_macro_state_for_path_prefers_macro_edit_state_over_empty_registration() {
    let path = temp_patcher_dsp_path("patcher-active-macro-recent-key");
    fs::write(&path, "(defmacro simp (x) (phasor x))\n(out 0)").unwrap();

    let mut old_node = patcher_test_node(&path);
    old_node.stable_widget_id = Some(1001);
    let old_key = patcher_state_key(&old_node);
    set_patcher_interaction_state(
        old_key,
        PatcherInteractionState {
            active_macro: Some("simp".to_string()),
            ..PatcherInteractionState::default()
        },
    );

    let mut current_node = patcher_test_node(&path);
    current_node.stable_widget_id = Some(1002);
    let current_key = patcher_state_key(&current_node);
    let mut current_state = PatcherInteractionState {
        active_macro: Some("simp".to_string()),
        ..PatcherInteractionState::default()
    };
    current_state.edit_state.nodes.insert(
        node_edit_key("macro:simp", "input"),
        PatcherNodeEdit {
            view_key: "macro:simp".to_string(),
            id: "input".to_string(),
            origin: PatcherNodeOrigin::Source {
                source_node_id: "input".to_string(),
            },
            text: "in 1 @name input".to_string(),
            position: (55.0, 66.0),
            width: None,
        },
    );
    set_patcher_interaction_state(current_key, current_state);

    let active_state =
        active_interaction_state_for_path(&path, "simp").expect("active macro state");
    assert_eq!(
        active_state
            .edit_state
            .nodes
            .get(&node_edit_key("macro:simp", "input"))
            .map(|edit| edit.position),
        Some((55.0, 66.0)),
        "save/fork should use the current visible patcher state, not an older registered state"
    );

    let _ = patcher_state_key(&old_node);
    let active_state =
        active_interaction_state_for_path(&path, "simp").expect("active macro state");
    assert_eq!(
        active_state
            .edit_state
            .nodes
            .get(&node_edit_key("macro:simp", "input"))
            .map(|edit| edit.position),
        Some((55.0, 66.0)),
        "an empty active macro state must not clobber the visible edited macro layout"
    );
}

#[test]
fn patcher_state_key_separates_paths_even_when_stable_widget_id_is_reused() {
    let draft_path = temp_patcher_dsp_path("patcher-stable-key-draft");
    let final_path = temp_patcher_dsp_path("patcher-stable-key-final");
    fs::write(&draft_path, "(def input (in 1 @name input))\n(out input)").unwrap();
    fs::write(&final_path, "(def input (in 1 @name input))\n(out input)").unwrap();

    let mut draft_node = patcher_test_node(&draft_path);
    draft_node.stable_widget_id = Some(112_358);
    let mut final_node = patcher_test_node(&final_path);
    final_node.stable_widget_id = draft_node.stable_widget_id;

    let draft_key = patcher_state_key(&draft_node);
    let final_key = patcher_state_key(&final_node);
    assert_ne!(
        draft_key, final_key,
        "a reused patcher widget slot must not carry draft edit state into a different source path"
    );

    let mut draft_state = PatcherInteractionState::default();
    draft_state.selected_nodes.insert("input".to_string());
    set_patcher_interaction_state(draft_key, draft_state);

    assert!(
        get_patcher_interaction_state(final_key)
            .selected_nodes
            .is_empty(),
        "opening a finalized patch in the same widget slot should start from its own path state"
    );
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
        width: None,
        param: None,
        inline_inputs: Vec::new(),
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
fn agentic_bubble_cmd_k_on_selected_macro_creates_edit_target() {
    let path = temp_patcher_source_path("agentic-bubble-edit-target");
    fs::write(
        &path,
        "(defmacro smooth (sig amt) (mix sig amt 0.5))\n(def input (in 1))\n(def shaped (smooth input 0.25))\n(out shaped)",
    )
    .expect("write source");
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut state = get_patcher_interaction_state(key);
    state.selected_nodes.insert("shaped".to_string());
    set_patcher_interaction_state(key, state);

    PATCHER_WIDGET.key_event(
        &node,
        WidgetKeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::SUPER,
        },
    );

    let state = get_patcher_interaction_state(key);
    let bubble = state.agentic_bubbles.values().next().expect("edit bubble");
    match &bubble.target {
        AgenticBubbleTarget::EditMacro {
            instance_node_id,
            macro_name,
            params,
            source,
        } => {
            assert_eq!(instance_node_id, "shaped");
            assert_eq!(macro_name, "smooth");
            assert_eq!(params, &vec!["sig".to_string(), "amt".to_string()]);
            assert!(source.contains("(defmacro smooth"));
        }
        AgenticBubbleTarget::CreateMacro => panic!("expected edit target"),
    }
}

#[test]
fn agentic_bubble_edit_submit_payload_includes_macro_context() {
    let path = temp_patcher_source_path("agentic-bubble-edit-submit");
    fs::write(
        &path,
        "(defmacro smooth (sig amt) (mix sig amt 0.5))\n(def input (in 1))\n(def shaped (smooth input 0.25))\n(out shaped)",
    )
    .expect("write source");
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut state = get_patcher_interaction_state(key);
    state.selected_nodes.insert("shaped".to_string());
    set_patcher_interaction_state(key, state);
    PATCHER_WIDGET.key_event(
        &node,
        WidgetKeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::SUPER,
        },
    );
    let mut state = get_patcher_interaction_state(key);
    let bubble_id = editing_agentic_bubble_id(&state).expect("editing bubble");
    state
        .agentic_bubbles
        .get_mut(&bubble_id)
        .expect("bubble")
        .prompt = "explain it".to_string();
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
    let Value::Map(map) = &output.args[0] else {
        panic!("submit payload should be a map");
    };
    assert!(matches!(
        &*map.get("target").expect("target").borrow(),
        Value::Keyword(target) if target == "edit-macro"
    ));
    assert!(matches!(
        &*map
            .get("existing-macro-name")
            .expect("existing macro name")
            .borrow(),
        Value::String(name) if name == "smooth"
    ));
    assert!(matches!(
        &*map
            .get("existing-macro-params")
            .expect("existing macro params")
            .borrow(),
        Value::String(params) if params == "sig amt"
    ));
}

#[test]
fn agentic_bubble_answer_resolution_keeps_source_unchanged() {
    let path = temp_patcher_source_path("agentic-bubble-answer");
    fs::write(
        &path,
        "(defmacro smooth (sig amt) (mix sig amt 0.5))\n(def input (in 1))\n(def shaped (smooth input 0.25))\n(out shaped)",
    )
    .expect("write source");
    let before = fs::read_to_string(&path).expect("read source");
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState::default();
    let bubble_id = allocate_agentic_bubble_with_target(
        &mut state,
        (1.0, 1.0),
        AgenticBubbleTarget::EditMacro {
            instance_node_id: "shaped".to_string(),
            macro_name: "smooth".to_string(),
            params: vec!["sig".to_string(), "amt".to_string()],
            source: "(defmacro smooth (sig amt) (mix sig amt 0.5))".to_string(),
        },
    );
    let bubble = state.agentic_bubbles.get_mut(&bubble_id).expect("bubble");
    bubble.prompt = "what does it do?".to_string();
    bubble.generation = 1;
    bubble.state = AgenticBubbleState::Pending {
        started_at: Instant::now(),
    };
    set_patcher_interaction_state(key, state);

    resolve_agentic_bubble_answer(&path, &bubble_id, 1, "It crossfades toward amt.");

    assert_eq!(fs::read_to_string(&path).expect("read source"), before);
    let state = get_patcher_interaction_state(key);
    let bubble = state
        .agentic_bubbles
        .get(&bubble_id)
        .expect("answer bubble");
    assert!(
        matches!(&bubble.state, AgenticBubbleState::Answer { text, .. } if text.contains("crossfades"))
    );
}

#[test]
fn agentic_bubble_macro_edit_resolution_replaces_macro_and_keeps_instance() {
    let path = temp_patcher_source_path("agentic-bubble-edit-resolve");
    fs::write(
        &path,
        "(defmacro smooth (sig amt) (mix sig amt 0.5))\n(def input (in 1))\n(def shaped (smooth input 0.25))\n(out shaped)",
    )
    .expect("write source");
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState::default();
    let bubble_id = allocate_agentic_bubble_with_target(
        &mut state,
        (1.0, 1.0),
        AgenticBubbleTarget::EditMacro {
            instance_node_id: "shaped".to_string(),
            macro_name: "smooth".to_string(),
            params: vec!["sig".to_string(), "amt".to_string()],
            source: "(defmacro smooth (sig amt) (mix sig amt 0.5))".to_string(),
        },
    );
    let bubble = state.agentic_bubbles.get_mut(&bubble_id).expect("bubble");
    bubble.generation = 1;
    bubble.state = AgenticBubbleState::Pending {
        started_at: Instant::now(),
    };
    set_patcher_interaction_state(key, state);

    resolve_agentic_bubble_macro_edit(
        &path,
        PatcherIntent::Instrument,
        &bubble_id,
        1,
        "smooth",
        "(defmacro smooth (sig amt) (def wet (mix sig amt 0.75)) wet)",
    )
    .expect("resolve edit");

    let source = fs::read_to_string(&path).expect("read source");
    assert!(source.contains("(def wet (mix sig amt 0.75))"));
    assert!(source.contains("(def shaped (smooth input 0.25))"));
    let state = get_patcher_interaction_state(key);
    assert!(!state.agentic_bubbles.contains_key(&bubble_id));
    assert!(state.agentic_morph_nodes.contains_key("shaped"));
}

#[test]
fn agentic_bubble_macro_edit_resolution_updates_all_registered_widgets() {
    let path = temp_patcher_source_path("agentic-bubble-edit-multi-widget");
    fs::write(
        &path,
        "(defmacro smooth (sig amt) (mix sig amt 0.5))\n(def input (in 1))\n(def shaped (smooth input 0.25))\n(out shaped)",
    )
    .expect("write source");
    let mut node_a = patcher_test_node(&path);
    node_a.stable_widget_id = Some(101);
    let mut node_b = patcher_test_node(&path);
    node_b.stable_widget_id = Some(202);
    let key_a = patcher_state_key(&node_a);
    let key_b = patcher_state_key(&node_b);

    for key in [key_a, key_b] {
        let mut state = PatcherInteractionState::default();
        let bubble_id = allocate_agentic_bubble_with_target(
            &mut state,
            (1.0, 1.0),
            AgenticBubbleTarget::EditMacro {
                instance_node_id: "shaped".to_string(),
                macro_name: "smooth".to_string(),
                params: vec!["sig".to_string(), "amt".to_string()],
                source: "(defmacro smooth (sig amt) (mix sig amt 0.5))".to_string(),
            },
        );
        assert_eq!(bubble_id, "bubble-0");
        let bubble = state.agentic_bubbles.get_mut(&bubble_id).expect("bubble");
        bubble.generation = 1;
        bubble.state = AgenticBubbleState::Pending {
            started_at: Instant::now(),
        };
        set_patcher_interaction_state(key, state);
    }

    resolve_agentic_bubble_macro_edit(
        &path,
        PatcherIntent::Instrument,
        "bubble-0",
        1,
        "smooth",
        "(defmacro smooth (sig amt) (def wet (mix sig amt 0.9)) wet)",
    )
    .expect("resolve edit");

    let source = fs::read_to_string(&path).expect("read source");
    assert_eq!(source.matches("(defmacro smooth").count(), 1);
    assert!(source.contains("(def wet (mix sig amt 0.9))"));
    for key in [key_a, key_b] {
        let state = get_patcher_interaction_state(key);
        assert!(!state.agentic_bubbles.contains_key("bubble-0"));
        assert!(state.agentic_morph_nodes.contains_key("shaped"));
    }
}

#[test]
fn agentic_bubble_macro_edit_rematerializes_edited_macro_layout_scope() {
    let path = temp_patcher_dsp_path("agentic-bubble-edit-layout");
    fs::write(
        &path,
        "(defmacro smooth (sig amt) (def shaped (mix sig amt 0.5)) shaped)\n(def input (in 1))\n(def out1 (smooth input 0.25))\n(out out1)",
    )
    .expect("write source");
    load_patch_from_props(&patcher_props_for_path(&path)).expect("materialize sidecar");
    let sidecar_path = sidecar::sidecar_path_for_source(&path);
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).expect("read sidecar"))
            .expect("parse sidecar");
    json["macros"]["smooth"]["nodes"]["shaped"] = serde_json::json!({ "x": 333.0, "y": 222.0 });
    json["macros"]["smooth"]["cables"] = serde_json::json!({});
    fs::write(&sidecar_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut state = PatcherInteractionState::default();
    let bubble_id = allocate_agentic_bubble_with_target(
        &mut state,
        (1.0, 1.0),
        AgenticBubbleTarget::EditMacro {
            instance_node_id: "out1".to_string(),
            macro_name: "smooth".to_string(),
            params: vec!["sig".to_string(), "amt".to_string()],
            source: "(defmacro smooth (sig amt) (def shaped (mix sig amt 0.5)) shaped)".to_string(),
        },
    );
    let bubble = state.agentic_bubbles.get_mut(&bubble_id).expect("bubble");
    bubble.generation = 1;
    bubble.state = AgenticBubbleState::Pending {
        started_at: Instant::now(),
    };
    set_patcher_interaction_state(key, state);

    resolve_agentic_bubble_macro_edit(
        &path,
        PatcherIntent::Instrument,
        &bubble_id,
        1,
        "smooth",
        "(defmacro smooth (sig amt) (def shaped (mix sig amt 0.75)) shaped)",
    )
    .expect("resolve edit");

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).expect("read saved sidecar"))
            .expect("parse saved sidecar");
    let shaped = &saved["macros"]["smooth"]["nodes"]["shaped"];
    assert_ne!(
        shaped,
        &serde_json::json!({ "x": 333.0, "y": 222.0 }),
        "edited macro scope should use fresh auto layout rather than stale sidecar positions"
    );
    let cables = saved["macros"]["smooth"]["cables"]
        .as_object()
        .expect("macro cables");
    assert!(
        !cables.is_empty(),
        "edited macro scope should persist generated segmented cable lanes"
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
fn agentic_created_macro_save_payload_reopens_with_visible_instance_layout() {
    let path = temp_patcher_dsp_path("patcher-agentic-created-macro-layout");
    fs::write(
        &path,
        "(def input (in 1 @name input))\n(out input 1 @name audio)\n",
    )
    .unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    load_patch_from_props(&node.props).unwrap();

    let mut state = PatcherInteractionState::default();
    let bubble_id = allocate_agentic_bubble(&mut state, (12.0, 8.0));
    let bubble = state.agentic_bubbles.get_mut(&bubble_id).unwrap();
    bubble.generation = 1;
    bubble.state = AgenticBubbleState::Pending {
        started_at: Instant::now(),
    };
    set_patcher_interaction_state(key, state);

    resolve_agentic_bubble(
        &path,
        PatcherIntent::Instrument,
        &bubble_id,
        1,
        "softfold",
        "(defmacro softfold (sig) (tanh sig))",
    )
    .unwrap();

    let mut state = get_patcher_interaction_state(key);
    let visible = debug_patch_for_state(&node, &state, "root").unwrap();
    let macro_instance = visible
        .nodes
        .iter()
        .find(|patch_node| patch_node.op == "softfold")
        .expect("visible agentic macro instance");
    let macro_instance_id = macro_instance.id.clone();
    let input_to_audio = visible
        .connections
        .iter()
        .find(|connection| connection.from_node == "input" && connection.to_node == "audio")
        .expect("input should initially feed the output");
    let placed_position = (91.25, 37.5);
    set_node_edit_position(
        &mut state,
        "root",
        macro_instance,
        placed_position,
        node_display_label(macro_instance),
    );
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(input_to_audio),
        ));
    connect_output_to_input(&mut state, "root", "input", &macro_instance_id, 0);
    connect_output_to_input(&mut state, "root", &macro_instance_id, "audio", 0);
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
        other => panic!("expected emitted layout string, got {other:?}; payload={map:?}"),
    };

    let final_path = temp_patcher_dsp_path("patcher-agentic-created-macro-final");
    fs::write(&final_path, source).unwrap();
    fs::write(sidecar::sidecar_path_for_source(&final_path), layout).unwrap();
    set_patcher_interaction_state(key, PatcherInteractionState::default());
    let final_node = patcher_test_node(&final_path);
    let (_path, reloaded) = load_patch_from_props(&final_node.props).unwrap();
    let reloaded_instance = reloaded
        .nodes
        .iter()
        .find(|patch_node| patch_node.op == "softfold")
        .expect("reloaded macro instance");
    assert_eq!(reloaded_instance.position, placed_position);
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
fn operator_metadata_exposes_param_ui_metadata_attributes() {
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../sequencer/tools/dgenlisp-operators.json"
    ))
    .expect("bundled dgenlisp-operators.json must be valid JSON");
    let operators = manifest["operators"]
        .as_array()
        .expect("operator manifest should contain operators");
    let param = operators
        .iter()
        .find(|operator| operator["name"].as_str() == Some("param"))
        .expect("param operator metadata");
    let attrs = param["attributes"]
        .as_array()
        .expect("param attributes")
        .iter()
        .map(|attr| attr.as_str().unwrap())
        .collect::<HashSet<_>>();
    assert!(attrs.contains("@group"));
    assert!(attrs.contains("@env"));
    assert!(attrs.contains("@role"));

    let role = param["attribute_docs"]
        .as_array()
        .expect("param attribute docs")
        .iter()
        .find(|attr| attr["name"].as_str() == Some("@role"))
        .expect("@role attribute doc");
    let role_values = role["values"]
        .as_array()
        .expect("@role enum values")
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(role_values, vec!["attack", "decay", "sustain", "release"]);
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
fn projects_param_ui_metadata_attributes_as_param_node() {
    let patch = parse(
        "(param amp_attack @group amp @env amp_env @role attack @default 5 @min 0 @max 1000)",
    );
    let node = patch
        .nodes
        .iter()
        .find(|node| node.id == "amp_attack")
        .unwrap();
    assert_eq!(node.kind, NodeKind::Param);
    assert_eq!(
        node_display_label(node),
        "param amp_attack @group amp @env amp_env @role attack @default 5 @min 0 @max 1000"
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
    assert_eq!(node_display_label(delay), "delay size");
    assert!(
        patch.connections.iter().any(|connection| {
            connection.from_node == param.id
                && connection.to_node == delay.id
                && connection.to_input == 1
                && connection.presentation == InputPresentation::InlineRawParam
        }),
        "{:#?}",
        patch.connections
    );
}

#[test]
fn def_wrapped_param_references_can_inline_from_source_node_metadata() {
    let patch = parse(
        r#"
            (def size (param size @min 0 @max 3000 @default 300))
            (def input (in 1))
            (def delayed (delay input size))
            "#,
    );
    let delay = patch.nodes.iter().find(|node| node.op == "delay").unwrap();
    let size_connection = source_connection_for_input(&patch, &delay.id, 1);

    assert_eq!(node_display_label(delay), "delay size");
    assert_eq!(
        size_connection.presentation,
        InputPresentation::InlineRawParam
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
fn defmacro_tuple_return_projects_multiple_subpatch_outlets() {
    let patch = parse(
        r#"
            (defmacro multi (a)
              (tuple (* a 2) (* a 3)))
            (def sig (in 1))
            (def out1 (multi sig))
            "#,
    );
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "multi")
        .expect("multi macro");
    assert_eq!(
        macro_patch.outputs,
        vec!["out".to_string(), "out2".to_string()]
    );
    assert_eq!(
        macro_patch
            .patch
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Out)
            .count(),
        2
    );
}

#[test]
fn destructured_macro_instance_uses_destructuring_names_as_outputs() {
    let patch = parse(
        r#"
            (defmacro multi (a)
              (tuple (* a 2) (* a 3)))
            (def sig (in 1))
            (def (x y) (multi sig))
            "#,
    );
    let node = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance && node.op == "multi")
        .expect("multi instance");
    assert_eq!(node.outputs, vec!["x".to_string(), "y".to_string()]);
}

#[test]
fn scalar_macro_instance_keeps_single_output() {
    let patch = parse(
        r#"
            (defmacro single (a) (* a 2))
            (def sig (in 1))
            (def x (single sig))
            "#,
    );
    let node = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance && node.op == "single")
        .expect("single instance");
    assert_eq!(node.outputs, vec!["out".to_string()]);
}

#[test]
fn multi_output_macro_nodes_have_visible_nonzero_port_rects() {
    let patch = parse(
        r#"
            (defmacro multi (a)
              (tuple (* a 2) (* a 3)))
            (def sig (in 1))
            (def (x y) (multi sig))
            "#,
    );
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 80.0,
        height: 30.0,
    };
    let pan = PatcherPanState::default();
    let root_rects = patch_node_rects(&patch, rect, &pan);
    let root_outputs = patch_output_counts(&patch);
    let instance = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance)
        .unwrap();
    let instance_rect = root_rects.get(&instance.id).unwrap();
    assert!(instance_rect.width > 0.0 && instance_rect.height > 0.0);
    assert_eq!(root_outputs.get(&instance.id), Some(&2));
    for output_index in 0..2 {
        let center = port_center(*instance_rect, output_index, 2, false);
        assert!(center.0.is_finite() && center.1.is_finite());
    }

    let macro_patch = &patch.macros[0].patch;
    let macro_rects = patch_node_rects(macro_patch, rect, &pan);
    let out_nodes = macro_patch
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Out)
        .collect::<Vec<_>>();
    assert_eq!(out_nodes.len(), 2);
    for node in out_nodes {
        let node_rect = macro_rects.get(&node.id).unwrap();
        assert!(node_rect.width > 0.0 && node_rect.height > 0.0);
    }
}

#[test]
fn created_macro_instance_uses_macro_signature_output_count() {
    let source = r#"
            (defmacro multi (a)
              (tuple (* a 2) (* a 3)))
            (def sig (in 1))
            (out sig 1)
        "#;
    let root_patch = parse(source);
    let mut state = PatcherInteractionState::default();
    let created = allocate_created_text_node(&mut state, "root", "multi");

    let patch = patch_with_interaction_state(root_patch, &state, "root");
    let node = patch.nodes.iter().find(|node| node.id == created).unwrap();
    assert_eq!(node.outputs, vec!["out".to_string(), "out2".to_string()]);
}

#[test]
fn root_macro_instance_reflects_unsaved_created_macro_out_nodes() {
    let source =
        "(defmacro xyz (input) input)\n(def sig (in 1))\n(def result (xyz sig))\n(out result 1)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "xyz")
        .unwrap();
    let input = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let original_out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:xyz", &original_out.id));
    let times1 = allocate_created_text_node(&mut state, "macro:xyz", "* 1");
    let times2 = allocate_created_text_node(&mut state, "macro:xyz", "* 2");
    let out1 = allocate_created_text_node(&mut state, "macro:xyz", "out 1");
    let out2 = allocate_created_text_node(&mut state, "macro:xyz", "out 2");
    for target in [&times1, &times2] {
        allocate_created_connection(
            &mut state,
            "macro:xyz",
            OutputPortRef {
                node_id: input.id.clone(),
                output_index: 0,
            },
            InputPortRef {
                node_id: target.clone(),
                input_index: 0,
            },
        );
    }
    allocate_created_connection(
        &mut state,
        "macro:xyz",
        OutputPortRef {
            node_id: times1,
            output_index: 0,
        },
        InputPortRef {
            node_id: out1,
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:xyz",
        OutputPortRef {
            node_id: times2,
            output_index: 0,
        },
        InputPortRef {
            node_id: out2,
            input_index: 0,
        },
    );

    let root = patch_with_interaction_state(root_patch, &state, "root");
    let xyz = root
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance && node.op == "xyz")
        .unwrap();
    assert_eq!(
        xyz.outputs,
        vec!["out".to_string(), "out2".to_string()],
        "root macro instance should reflect unsaved visual macro outlets"
    );
}

#[test]
fn debug_emit_macro_view_serializes_multiple_outlets_as_tuple() {
    let patch = parse(
        r#"
            (defmacro multi (a)
              (tuple (* a 2) (* a 3)))
            (def sig (in 1))
            (def (x y) (multi sig))
            "#,
    );
    let emitted = emit_patch_debug_lisp_for_view("macro:multi", &patch.macros[0].patch);
    assert_eq!(emitted, "(defmacro multi (a)\n  (tuple (* a 2) (* a 3)))");
}

#[test]
fn writeback_source_destructured_macro_outlet_uses_matching_symbol() {
    let source = r#"
            (defmacro multi (a) (tuple (* a 2) (* a 3)))
            (def sig (in 1))
            (def (x y) (multi sig))
            (def sum (+ x y))
            (out sum 1)
        "#;
    let root_patch = parse(source);
    let multi = root_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance && node.op == "multi")
        .unwrap();
    let sum = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "sum")
        .unwrap();
    let x_to_sum = root_patch
        .connections
        .iter()
        .find(|connection| {
            connection.from_node == multi.id
                && connection.to_node == sum.id
                && connection.to_input == 0
        })
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key("root", &source_connection_id(x_to_sum)));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: multi.id.clone(),
            output_index: 1,
        },
        InputPortRef {
            node_id: sum.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro multi (a) (tuple (* a 2.0) (* a 3.0)))\n(def sig (in 1))\n(def (x y) (multi sig))\n(def sum (+ y y))\n(out sum 1)"
    );
}

#[test]
fn writeback_created_multi_output_macro_instance_emits_destructuring_def() {
    let source = r#"
            (defmacro multi (a) (tuple (* a 2) (* a 3)))
            (def sig (in 1))
            (def sum (+ sig sig))
            (out sum 1)
        "#;
    let root_patch = parse(source);
    let sig = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "sig")
        .unwrap();
    let sum = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "sum")
        .unwrap();
    let sig_to_sum = root_patch
        .connections
        .iter()
        .filter(|connection| connection.from_node == sig.id && connection.to_node == sum.id)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(sig_to_sum.len(), 2);

    let mut state = PatcherInteractionState::default();
    let multi = allocate_created_text_node(&mut state, "root", "multi");
    for connection in &sig_to_sum {
        state
            .edit_state
            .deleted_connections
            .insert(connection_edit_key(
                "root",
                &source_connection_id(connection),
            ));
    }
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sig.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: multi.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: multi.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: sum.id.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: multi,
            output_index: 1,
        },
        InputPortRef {
            node_id: sum.id.clone(),
            input_index: 1,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro multi (a) (tuple (* a 2.0) (* a 3.0)))\n(def sig (in 1))\n(def (multi1 multi2) (multi sig))\n(def sum (+ multi1 multi2))\n(out sum 1)"
    );
}

#[test]
fn writeback_scalar_macro_binding_expands_to_destructuring_when_second_outlet_is_used() {
    let source = r#"
            (defmacro multi (a) (tuple (* a 2) (* a 3)))
            (def sig (in 1))
            (def foo (multi sig))
            (def sum (+ foo foo))
            (out sum 1)
        "#;
    let root_patch = parse(source);
    let foo = root_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance && node.id == "foo")
        .unwrap();
    let sum = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "sum")
        .unwrap();
    let foo_to_sum_1 = root_patch
        .connections
        .iter()
        .find(|connection| {
            connection.from_node == foo.id
                && connection.to_node == sum.id
                && connection.to_input == 1
        })
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(foo_to_sum_1),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: foo.id.clone(),
            output_index: 1,
        },
        InputPortRef {
            node_id: sum.id.clone(),
            input_index: 1,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro multi (a) (tuple (* a 2.0) (* a 3.0)))\n(def sig (in 1))\n(def (foo foo2) (multi sig))\n(def sum (+ foo foo2))\n(out sum 1)"
    );
}

#[test]
fn writeback_created_macro_out_nodes_emit_tuple_return_from_created_upstream_nodes() {
    let source = "(defmacro multi (input) input)\n(def sig (in 1))\n(def m (multi sig))\n(out m 1)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "multi")
        .unwrap();
    let input = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let original_out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:multi", &original_out.id));
    let mul1 = allocate_created_text_node(&mut state, "macro:multi", "* 1");
    let mul3 = allocate_created_text_node(&mut state, "macro:multi", "* 3");
    let out1 = allocate_created_text_node(&mut state, "macro:multi", "out 1");
    let out2 = allocate_created_text_node(&mut state, "macro:multi", "out 2");
    allocate_created_connection(
        &mut state,
        "macro:multi",
        OutputPortRef {
            node_id: input.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: mul1.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:multi",
        OutputPortRef {
            node_id: input.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: mul3.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:multi",
        OutputPortRef {
            node_id: mul1,
            output_index: 0,
        },
        InputPortRef {
            node_id: out1,
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:multi",
        OutputPortRef {
            node_id: mul3,
            output_index: 0,
        },
        InputPortRef {
            node_id: out2,
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro multi (input) (tuple (* input 1.0) (* input 3.0)))\n(def sig (in 1))\n(def m (multi sig))\n(out m 1)"
    );
}

#[test]
fn writeback_created_macro_out_preserves_source_out_when_expanding_scalar_macro() {
    let source = "(defmacro sqa (input) (* input 1.0))\n(def pitch (in 1))\n(def sqa1 (sqa pitch))\n(def phase (phasor sqa1))\n(out phase 1)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "sqa")
        .unwrap();
    let input = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let sqa = root_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MacroInstance && node.op == "sqa")
        .unwrap();
    let phase = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "phase")
        .unwrap();
    let sqa_to_phase = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == sqa.id && connection.to_node == phase.id)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    let times2 = allocate_created_text_node(&mut state, "macro:sqa", "* 2");
    let out2 = allocate_created_text_node(&mut state, "macro:sqa", "out 2");
    allocate_created_connection(
        &mut state,
        "macro:sqa",
        OutputPortRef {
            node_id: input.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: times2.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:sqa",
        OutputPortRef {
            node_id: times2,
            output_index: 0,
        },
        InputPortRef {
            node_id: out2,
            input_index: 0,
        },
    );
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sqa_to_phase),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: sqa.id.clone(),
            output_index: 1,
        },
        InputPortRef {
            node_id: phase.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro sqa (input) (tuple (* input 1.0) (* input 2.0)))\n(def pitch (in 1))\n(def (sqa1 sqa12) (sqa pitch))\n(def phase (phasor sqa12))\n(out phase 1)"
    );
}

#[test]
fn writeback_created_macro_sparse_out_channels_emit_placeholder_tuple_slots() {
    let source =
        "(defmacro sparse (input) input)\n(def sig (in 1))\n(def m (sparse sig))\n(out m 1)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "sparse")
        .unwrap();
    let input = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let original_out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:sparse", &original_out.id));
    let times3 = allocate_created_text_node(&mut state, "macro:sparse", "* 3");
    let out3 = allocate_created_text_node(&mut state, "macro:sparse", "out 3");
    allocate_created_connection(
        &mut state,
        "macro:sparse",
        OutputPortRef {
            node_id: input.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: times3.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:sparse",
        OutputPortRef {
            node_id: times3,
            output_index: 0,
        },
        InputPortRef {
            node_id: out3,
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro sparse (input) (tuple __patcher_missing_input__ __patcher_missing_input__ (* input 3.0)))\n(def sig (in 1))\n(def m (sparse sig))\n(out m 1)"
    );
}

#[test]
fn writeback_created_macro_out_nodes_emit_tuple_ordered_by_channel_not_creation_order() {
    let source = "(defmacro multi (input) input)\n(def sig (in 1))\n(def m (multi sig))\n(out m 1)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "multi")
        .unwrap();
    let input = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let original_out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:multi", &original_out.id));
    let times4 = allocate_created_text_node(&mut state, "macro:multi", "* 4");
    let times2 = allocate_created_text_node(&mut state, "macro:multi", "* 2");
    let out2 = allocate_created_text_node(&mut state, "macro:multi", "out 2");
    let out1 = allocate_created_text_node(&mut state, "macro:multi", "out 1");
    for target in [&times4, &times2] {
        allocate_created_connection(
            &mut state,
            "macro:multi",
            OutputPortRef {
                node_id: input.id.clone(),
                output_index: 0,
            },
            InputPortRef {
                node_id: target.clone(),
                input_index: 0,
            },
        );
    }
    allocate_created_connection(
        &mut state,
        "macro:multi",
        OutputPortRef {
            node_id: times4,
            output_index: 0,
        },
        InputPortRef {
            node_id: out2,
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:multi",
        OutputPortRef {
            node_id: times2,
            output_index: 0,
        },
        InputPortRef {
            node_id: out1,
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro multi (input) (tuple (* input 2.0) (* input 4.0)))\n(def sig (in 1))\n(def m (multi sig))\n(out m 1)"
    );
}

#[test]
fn writeback_created_macro_out_source_shared_with_created_consumer_materializes_once() {
    let source = "(defmacro multi (input) input)\n(def sig (in 1))\n(def m (multi sig))\n(out m 1)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "multi")
        .unwrap();
    let input = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let original_out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:multi", &original_out.id));
    let shared = allocate_created_text_node(&mut state, "macro:multi", "* 2");
    let plus = allocate_created_text_node(&mut state, "macro:multi", "+ 1");
    let out1 = allocate_created_text_node(&mut state, "macro:multi", "out 1");
    allocate_created_connection(
        &mut state,
        "macro:multi",
        OutputPortRef {
            node_id: input.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: shared.clone(),
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:multi",
        OutputPortRef {
            node_id: shared.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: out1,
            input_index: 0,
        },
    );
    allocate_created_connection(
        &mut state,
        "macro:multi",
        OutputPortRef {
            node_id: shared,
            output_index: 0,
        },
        InputPortRef {
            node_id: plus,
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro multi (input)\n  (def mul1 (* input 2.0))\n  (def add1 (+ mul1 1.0))\n  mul1)\n(def sig (in 1))\n(def m (multi sig))\n(out m 1)"
    );
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
fn writeback_deletes_source_history_compound_owner_parts() {
    let source = r#"
        (make-history h)
        (def sig (noise))
        (def delta (- (read-history h) sig))
        (write-history h sig)
        (out delta 1)
    "#;
    let patch = parse(source);
    let history = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::History)
        .expect("history node");
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &history.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (noise))\n(def delta (- __patcher_missing_input__ sig))\n(out delta 1)"
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
        width: None,
        param: None,
        inline_inputs: Vec::new(),
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
        presentation: InputPresentation::Cable,
        presentation_override: None,
        source: None,
    });
    macro_patch.connections.push(PatchConnection {
        from_node: "created-mul".to_string(),
        from_output: 0,
        to_node: plus_node_id,
        to_input: 0,
        kind: ConnectionKind::Forward,
        segment: None,
        presentation: InputPresentation::Cable,
        presentation_override: None,
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
        "(defmacro ap (sig g)\n  (def node (+ sig g))\n  node)"
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
fn writeback_source_connection_edit_moves_destination_input_across_missing_gap() {
    let source = r#"
        (param freq)
        (def result (phasor freq))
    "#;
    let patch = parse(source);
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
        .to
        .input_index = 2;

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param freq)\n(def result (phasor __patcher_missing_input__ __patcher_missing_input__ freq))"
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
fn writeback_root_param_rename_preserves_ui_metadata_attributes() {
    let source = r#"
        (param amp_attack @group amp @env amp_env @role attack @default 5 @min 0 @max 1000)
        (def env_phase (phasor amp_attack))
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
        .text =
        "param amp_a @group amp @env amp_env @role attack @default 5 @min 0 @max 1000".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param amp_a @group amp @env amp_env @role attack @default 5.0 @min 0.0 @max 1000.0)\n(def env_phase (phasor amp_a))"
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
fn writeback_constant_text_edit_to_param_rewrites_binding_references() {
    let source = r#"
        (def value1 5.0)
        (def phasor1 (phasor value1))
    "#;
    let patch = parse(source);
    let value = patch.nodes.iter().find(|node| node.id == "value1").unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", value, node_display_label(value));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &value.id))
        .unwrap()
        .text = "param xyz".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param xyz)\n(def phasor1 (phasor xyz))"
    );
}

#[test]
fn writeback_constant_text_edit_to_known_operator_emits_call_not_symbol_literal() {
    let source = r#"
        (def rate 0.5)
        (def phasor1 (phasor rate))
    "#;
    let patch = parse(source);
    let rate = patch.nodes.iter().find(|node| node.id == "rate").unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", rate, node_display_label(rate));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &rate.id))
        .unwrap()
        .text = "phasor".to_string();

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert_eq!(emitted, "(def rate (phasor))\n(def phasor1 (phasor rate))");
    let roundtrip = parse(&emitted);
    let rate = roundtrip
        .nodes
        .iter()
        .find(|node| node.id == "rate")
        .expect("edited constant should reload as a source-owned phasor node");
    assert_eq!(rate.op, "phasor");
    assert_eq!(node_display_label(rate), "phasor");
}

#[test]
fn writeback_created_literal_does_not_alias_existing_equal_constant_binding() {
    let source = r#"
        (def rate 0.5)
        (def trigger (in 4 @name trigger))
        (def phase (phasor trigger))
        (def tri (triangle phase 0.1))
    "#;
    let root_patch = parse(source);
    let mut state = PatcherInteractionState::default();
    let literal = allocate_created_text_node(&mut state, "root", "0.5");
    let phasor = allocate_created_text_node(&mut state, "root", "phasor trigger");
    connect_output_to_input(&mut state, "root", &literal, &phasor, 0);
    connect_output_to_input(&mut state, "root", &phasor, "tri", 1);
    if let Some(old) = source_connection_for_input_opt(&root_patch, "tri", 1) {
        state
            .edit_state
            .deleted_connections
            .insert(connection_edit_key("root", &source_connection_id(old)));
    }

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(def value1 0.5)")
            && emitted.contains("(def phasor1 (phasor value1 trigger))")
            && emitted.contains("(def tri (triangle phase phasor1))"),
        "created visible literal should get its own binding instead of reusing `rate`:\n{emitted}"
    );
    assert!(
        !emitted.contains("(def phasor1 (phasor rate trigger))"),
        "created literal must not alias the existing equal-valued rate binding:\n{emitted}"
    );
}

#[test]
fn writeback_param_text_edit_to_constant_preserves_binding_references() {
    let source = r#"
        (param xyz)
        (def phasor1 (phasor xyz))
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
        .text = "5.0".to_string();

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def xyz 5.0)\n(def phasor1 (phasor xyz))"
    );
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
        "(defmacro ap (sig g)\n  (def node (mix sig g))\n  node)"
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
        "(defmacro ap (input g)\n  (def node (+ input g))\n  (phasor node))\n(def sig (in 1))"
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
fn writeback_deleting_code_island_removes_unsupported_form() {
    let source = "(let ((x 1)) x)\n(param freq)";
    let patch = parse(source);
    let code = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::CodeIsland)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &code.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param freq)"
    );
}

#[test]
fn writeback_deleting_macro_code_island_removes_unsupported_macro_form() {
    let source = "(defmacro simp (input)\n  (make-history history1)\n  (write-history missing value)\n  input)";
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "simp")
        .unwrap();
    let code = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::CodeIsland)
        .unwrap();
    let mut state = PatcherInteractionState {
        active_macro: Some("simp".to_string()),
        ..PatcherInteractionState::default()
    };
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:simp", &code.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro simp (input)\n  (make-history history1)\n  input)"
    );
}

#[test]
fn library_macro_autosave_deleting_code_island_recovers_package_source() {
    let library = temp_defmacro_library(
        "view-edit-delete-code-island",
        &[(
            "simp11",
            "(defmacro simp11 (input)\n  (make-history history1)\n  (write-history missing value)\n  input)",
        )],
    );
    let source =
        "(use-defmacro simp11)\n(def sig (in 1))\n(def shaped (simp11 sig))\n(out shaped 1)";
    let root_patch =
        parse_patch_source_with_library(source, PatcherIntent::Instrument, &library).unwrap();
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "simp11")
        .expect("library macro should project");
    let code = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::CodeIsland)
        .expect("unsupported write-history should project as a code island");

    let mut state = PatcherInteractionState {
        active_macro: Some("simp11".to_string()),
        ..PatcherInteractionState::default()
    };
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:simp11", &code.id));

    let persisted =
        persist_library_macro_edits(&root_patch, PatcherIntent::Instrument, &state, &library)
            .unwrap();
    assert_eq!(persisted, vec!["simp11".to_string()]);

    let package = library.package("simp11").unwrap();
    let saved = fs::read_to_string(&package.source_path).unwrap();
    assert!(
        !saved.contains("write-history"),
        "deleted code island form should be removed from package source:\n{saved}"
    );
    assert!(
        saved.contains("(defmacro simp11 (input)"),
        "package should still contain its public macro:\n{saved}"
    );
    DefmacroPackage::from_source(&package.package_dir, "simp11", &saved).unwrap();
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
fn writeback_history_read_into_missing_source_arg_inserts_arg() {
    let source = r#"
        (make-history h)
        (def sig (in 1))
        (def result (mix sig))
        (write-history h sig)
    "#;
    let patch = parse(source);
    let history = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::History)
        .unwrap();
    let mix = patch.nodes.iter().find(|node| node.id == "result").unwrap();
    let mut state = PatcherInteractionState::default();
    connect_output_to_input(&mut state, "root", &history.id, &mix.id, 1);

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(make-history h)\n(def sig (in 1))\n(def result (mix sig (read-history h)))\n(write-history h sig)"
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
fn emitted_layout_maps_created_history_node_and_cables_to_saved_history_name() {
    let source = r#"
        (def sig (in 1))
        (out sig 1)
    "#;
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
    let mix = allocate_created_text_node(&mut state, "root", "mix ? 0.99");
    let history = allocate_created_text_node(&mut state, "root", "history");
    let history_position = (101.25, 77.5);
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &history))
        .unwrap()
        .position = history_position;
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

    let result = emit_patch_writeback_result(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(result.source.contains("(make-history history1)"));
    let mut emitted_patch = parse_patch_source(&result.source, PatcherIntent::Instrument).unwrap();
    let layout_json: serde_json::Value = serde_json::from_str(
        &sidecar::emitted_layout_json_with_node_map(
            &mut emitted_patch,
            &root_patch,
            &state,
            &result.generated_node_ids,
        )
        .unwrap(),
    )
    .unwrap();

    let history_layout = &layout_json["root"]["nodes"]["history1"];
    assert!(
        (history_layout["x"].as_f64().unwrap() - history_position.0 as f64).abs() < 0.0001
            && (history_layout["y"].as_f64().unwrap() - history_position.1 as f64).abs() < 0.0001,
        "emitted sidecar should preserve the created history node position under the emitted history name: layout={layout_json:#?}"
    );
    assert!(
        layout_json["root"]["cables"]
            .as_object()
            .unwrap()
            .is_empty(),
        "visible unsegmented history cables should not be saved as auto-segmented emitted cables: layout={layout_json:#?}"
    );
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
fn writeback_macro_history_source_cable_layout_edit_is_not_counted_as_second_write() {
    let source = r#"
        (defmacro karplus_strong (excitation)
          (make-history filter_hist)
          (def filtered (read-history filter_hist))
          (write-history filter_hist excitation)
          filtered)
        (def input (in 1))
        (def out1 (karplus_strong input))
        (out out1 1)
    "#;
    let patch = parse(source);
    let macro_patch = patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "karplus_strong")
        .unwrap();
    let write = source_connection_for_input(&macro_patch.patch, "filter_hist", 0);

    let mut state = PatcherInteractionState::default();
    set_connection_segment_edit(
        &mut state,
        "macro:karplus_strong",
        write,
        Some(CableSegmentInfo {
            is_segmented: true,
            segment_row: 12.0,
        }),
    );

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    assert!(
        emitted.contains("(write-history filter_hist excitation)"),
        "{emitted}"
    );
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
        "(defmacro ap (sig)\n  (make-history history1)\n  (write-history history1 sig)\n  (read-history history1))"
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
fn writeback_generated_binding_wraps_nested_expression_after_nested_input_rewire() {
    let source = "\
(defmacro gain2 (x) (* x 2))
(def phase (phasor 440))
(def env (in 1))
(out (* phase env) 1)";
    let patch = parse(source);
    let phase = patch.nodes.iter().find(|node| node.id == "phase").unwrap();
    let multiply = patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let phase_to_multiply = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phase.id && connection.to_node == multiply.id)
        .unwrap();
    let multiply_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == multiply.id && connection.to_node == out.id)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(phase_to_multiply),
        ));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(multiply_to_out),
        ));

    let gain = allocate_created_text_node(&mut state, "root", "gain2");
    connect_output_to_input(&mut state, "root", &multiply.id, &gain, 0);
    connect_output_to_input(&mut state, "root", &gain, &out.id, 0);
    let scale = allocate_created_text_node(&mut state, "root", "* twopi");
    let cos = allocate_created_text_node(&mut state, "root", "cos");
    connect_output_to_input(&mut state, "root", &phase.id, &scale, 0);
    connect_output_to_input(&mut state, "root", &scale, &cos, 0);
    connect_output_to_input(&mut state, "root", &cos, &multiply.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(def gain21 (gain2 (* cos1 env)))"),
        "generated macro binding should wrap the rewired nested expression:\n{emitted}"
    );
    assert!(
        emitted.find("(def cos1 ").unwrap() < emitted.find("(def gain21 ").unwrap(),
        "generated dependency should be emitted before its consumer:\n{emitted}"
    );
    assert!(
        emitted.contains("(out gain21 1)"),
        "out should consume the generated macro binding:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
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
fn writeback_created_param_replaces_deleted_constant_source_input() {
    let source = r#"
        (def value1 5.0)
        (def phasor1 (phasor value1))
        (def mul1 (* phasor1 320.0))
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def add1 (+ pitch mul1))
        (def velocity (in 3 @name velocity))
        (def trigger (in 4 @name trigger))
        (def mod1 (in 5 @name mod1 @modulator 1))
        (def mod2 (in 6 @name mod2 @modulator 2))
        (def mod3 (in 7 @name mod3 @modulator 3))
        (def mod4 (in 8 @name mod4 @modulator 4))
        (param attack @default 5.0 @min 0.0 @max 1000.0 @unit ms)
        (param decay @default 120.0 @min 1.0 @max 2000.0 @unit ms)
        (param sustain @default 0.8 @min 0.0 @max 1.0)
        (param release @default 180.0 @min 1.0 @max 5000.0 @unit ms)
        (param gain @default 0.5 @min 0.0 @max 1.0 @mod true @mod-mode additive)
        (def env (adsr gate trigger attack decay sustain release))
        (def phase (phasor add1))
        (def osc (scale phase 0.0 1.0 -1.0 1.0))
        (out (* osc env velocity (mod gain)) 1 @name audio)
    "#;
    let patch = parse(source);
    let value = patch.nodes.iter().find(|node| node.id == "value1").unwrap();
    let phasor = patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let value_to_phasor = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == value.id && connection.to_node == phasor.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &value.id));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(value_to_phasor),
        ));
    let param = allocate_created_text_node(&mut state, "root", "param xyz");
    connect_output_to_input(&mut state, "root", &param, &phasor.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        !emitted.contains("value1"),
        "deleted constant binding must not remain referenced:\n{emitted}"
    );
    assert!(
        emitted.contains("(param xyz)"),
        "created param should be emitted:\n{emitted}"
    );
    assert!(
        emitted.contains("(def phasor1 (phasor xyz))"),
        "phasor should consume the created param symbol:\n{emitted}"
    );
}

#[test]
fn writeback_reconnecting_converted_param_uses_param_name_not_stale_def_name() {
    let source = r#"
        (def value1 (param xyz))
        (def phasor1 (phasor __patcher_missing_input__))
    "#;
    let patch = parse(source);
    let value = patch.nodes.iter().find(|node| node.id == "value1").unwrap();
    let phasor = patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", value, node_display_label(value));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &value.id))
        .unwrap()
        .text = "param xyz".to_string();
    connect_output_to_input(&mut state, "root", &value.id, &phasor.id, 0);

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param xyz)\n(def phasor1 (phasor xyz))"
    );
}

#[test]
fn writeback_reconnecting_unsaved_constant_to_param_edit_uses_param_name() {
    let source = r#"
        (def value1 5.1)
        (def phasor1 (phasor value1))
        (def mul1 (* phasor1 320.0))
    "#;
    let mut state = PatcherInteractionState::default();
    state.edit_state.nodes.insert(
        node_edit_key("root", "value1"),
        PatcherNodeEdit {
            view_key: "root".to_string(),
            id: "value1".to_string(),
            origin: PatcherNodeOrigin::Source {
                source_node_id: "value1".to_string(),
            },
            text: "param xyz".to_string(),
            position: (76.14, 4.0),
            width: None,
        },
    );
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key("root", "value1:0->phasor1:0"));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: "value1".to_string(),
            output_index: 0,
        },
        InputPortRef {
            node_id: "phasor1".to_string(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param xyz)\n(def phasor1 (phasor xyz))\n(def mul1 (* phasor1 320.0))"
    );
}

#[test]
fn writeback_created_param_can_feed_created_node_that_replaces_source_input() {
    let source = r#"
        (def value1 5.0)
        (def phasor1 (phasor value1))
        (def mul1 (* phasor1 320.0))
    "#;
    let patch = parse(source);
    let phasor = patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let mul = patch.nodes.iter().find(|node| node.id == "mul1").unwrap();
    let phasor_to_mul = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phasor.id && connection.to_node == mul.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(phasor_to_mul),
        ));
    let created_multiply = allocate_created_text_node(&mut state, "root", "*");
    connect_output_to_input(&mut state, "root", &phasor.id, &created_multiply, 1);
    connect_output_to_input(&mut state, "root", &created_multiply, &mul.id, 0);
    let created_param = allocate_created_text_node(&mut state, "root", "param xyz");
    connect_output_to_input(&mut state, "root", &created_param, &created_multiply, 0);

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(param xyz)\n(def value1 5.0)\n(def phasor1 (phasor value1))\n(def mul2 (* xyz phasor1))\n(def mul1 (* mul2 320.0))"
    );
}

#[derive(Clone, Debug)]
struct WritebackFuzzRng(u64);

impl WritebackFuzzRng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }

    fn bool(&mut self) -> bool {
        self.next() & 1 == 1
    }

    fn usize(&mut self, max: usize) -> usize {
        (self.next() as usize) % max
    }
}

fn assert_writeback_fuzz_compiles(source: &str, state: &PatcherInteractionState, seed: u64) {
    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, state)
        .unwrap_or_else(|error| panic!("writeback failed for seed {seed}: {error:?}"));
    assert!(
        !emitted.contains("__patcher_missing_input__"),
        "fully connected fuzz edit emitted missing input for seed {seed}:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted).unwrap_or_else(|error| {
        panic!("compiled writeback failed for seed {seed}:\n{emitted}\n{error}")
    });
}

#[test]
fn writeback_fuzz_created_values_replacing_source_inputs_compile() {
    let source = r#"
        (def value1 5.0)
        (def phasor1 (phasor value1))
        (def mul1 (* phasor1 320.0))
        (out mul1 1)
    "#;
    let patch = parse(source);
    let phasor = patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let mul = patch.nodes.iter().find(|node| node.id == "mul1").unwrap();
    let phasor_to_mul = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phasor.id && connection.to_node == mul.id)
        .unwrap();

    for seed in 0..32 {
        let mut rng = WritebackFuzzRng::new(seed);
        let mut state = PatcherInteractionState::default();
        state
            .edit_state
            .deleted_connections
            .insert(connection_edit_key(
                "root",
                &source_connection_id(phasor_to_mul),
            ));

        let chain_len = 1 + rng.usize(3);
        let mut previous: Option<String> = None;
        for index in 0..chain_len {
            let op = allocate_created_text_node(&mut state, "root", "*");
            if index == 0 {
                let phasor_input = rng.usize(2);
                connect_output_to_input(&mut state, "root", &phasor.id, &op, phasor_input);
                let other_input = 1 - phasor_input;
                if rng.bool() {
                    let param = allocate_created_text_node(
                        &mut state,
                        "root",
                        &format!("param fuzz{seed}_{index}"),
                    );
                    connect_output_to_input(&mut state, "root", &param, &op, other_input);
                } else {
                    let literal =
                        allocate_created_text_node(&mut state, "root", &format!("{}", index + 2));
                    connect_output_to_input(&mut state, "root", &literal, &op, other_input);
                }
            } else if let Some(previous_node) = previous.as_ref() {
                let previous_input = rng.usize(2);
                connect_output_to_input(&mut state, "root", previous_node, &op, previous_input);
                let param = allocate_created_text_node(
                    &mut state,
                    "root",
                    &format!("param chain{seed}_{index}"),
                );
                connect_output_to_input(&mut state, "root", &param, &op, 1 - previous_input);
            }
            previous = Some(op);
        }
        connect_output_to_input(&mut state, "root", previous.as_ref().unwrap(), &mul.id, 0);

        assert_writeback_fuzz_compiles(source, &state, seed);
    }
}

#[test]
fn writeback_fuzz_unsaved_param_conversions_reconnect_without_stale_symbols() {
    let source = r#"
        (def value1 5.0)
        (def phasor1 (phasor value1))
        (def mul1 (* phasor1 320.0))
        (out mul1 1)
    "#;
    let patch = parse(source);
    let phasor = patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let mul = patch.nodes.iter().find(|node| node.id == "mul1").unwrap();
    let value_to_phasor = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == "value1" && connection.to_node == phasor.id)
        .unwrap();
    let phasor_to_mul = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phasor.id && connection.to_node == mul.id)
        .unwrap();

    for seed in 0..32 {
        let mut rng = WritebackFuzzRng::new(seed + 1000);
        let mut state = PatcherInteractionState::default();
        state.edit_state.nodes.insert(
            node_edit_key("root", "value1"),
            PatcherNodeEdit {
                view_key: "root".to_string(),
                id: "value1".to_string(),
                origin: PatcherNodeOrigin::Source {
                    source_node_id: "value1".to_string(),
                },
                text: format!("param unsaved{seed}"),
                position: (0.0, 0.0),
                width: None,
            },
        );
        state
            .edit_state
            .deleted_connections
            .insert(connection_edit_key(
                "root",
                &source_connection_id(value_to_phasor),
            ));
        state
            .edit_state
            .deleted_connections
            .insert(connection_edit_key(
                "root",
                &source_connection_id(phasor_to_mul),
            ));
        connect_output_to_input(&mut state, "root", "value1", &phasor.id, 0);

        if rng.bool() {
            let created_multiply = allocate_created_text_node(&mut state, "root", "*");
            let phasor_input = rng.usize(2);
            connect_output_to_input(
                &mut state,
                "root",
                &phasor.id,
                &created_multiply,
                phasor_input,
            );
            connect_output_to_input(
                &mut state,
                "root",
                "value1",
                &created_multiply,
                1 - phasor_input,
            );
            connect_output_to_input(&mut state, "root", &created_multiply, &mul.id, 0);
        } else {
            connect_output_to_input(&mut state, "root", &phasor.id, &mul.id, 0);
        }

        let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state)
            .unwrap_or_else(|error| panic!("writeback failed for seed {seed}: {error:?}"));
        assert!(
            !emitted.contains("value1"),
            "stale value1 reference survived seed {seed}:\n{emitted}"
        );
        compile_patch_source_with_dgenlisp(&emitted).unwrap_or_else(|error| {
            panic!("compiled writeback failed for seed {seed}:\n{emitted}\n{error}")
        });
    }
}

#[test]
fn writeback_fuzz_mixed_file_and_created_macros_compile() {
    let source = r#"
        (defmacro fileop (input amount) (* input amount))
        (def value1 5.0)
        (def phasor1 (phasor value1))
        (def mul1 (* phasor1 320.0))
        (out mul1 1)
    "#;
    let root_patch = parse(source);
    let phasor = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let mul = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "mul1")
        .unwrap();
    let phasor_to_mul = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phasor.id && connection.to_node == mul.id)
        .unwrap();

    for seed in 0..24 {
        let mut rng = WritebackFuzzRng::new(seed + 2000);
        let mut state = PatcherInteractionState::default();
        state
            .edit_state
            .deleted_connections
            .insert(connection_edit_key(
                "root",
                &source_connection_id(phasor_to_mul),
            ));

        let created_macro = allocate_created_text_node(&mut state, "root", "defmacro *userop*");
        assert!(promote_created_macro_definition(
            &root_patch,
            &mut state,
            "root",
            &created_macro,
        ));
        let file_macro =
            allocate_created_text_node(&mut state, "root", &format!("fileop {}", 2 + rng.usize(5)));
        let created_multiply = allocate_created_text_node(&mut state, "root", "*");
        let created_param =
            allocate_created_text_node(&mut state, "root", &format!("param macrofuzz{seed}"));

        let mut chain = if rng.bool() {
            vec![
                file_macro.clone(),
                created_multiply.clone(),
                created_macro.clone(),
            ]
        } else {
            vec![
                created_macro.clone(),
                created_multiply.clone(),
                file_macro.clone(),
            ]
        };
        if rng.bool() {
            chain.reverse();
        }

        let first_input = if chain[0] == created_multiply {
            rng.usize(2)
        } else {
            0
        };
        connect_output_to_input(&mut state, "root", &phasor.id, &chain[0], first_input);
        if chain[0] == created_multiply {
            connect_output_to_input(
                &mut state,
                "root",
                &created_param,
                &chain[0],
                1 - first_input,
            );
        }

        for index in 1..chain.len() {
            let previous = chain[index - 1].clone();
            let current = chain[index].clone();
            let current_input = if current == created_multiply {
                rng.usize(2)
            } else {
                0
            };
            connect_output_to_input(&mut state, "root", &previous, &current, current_input);
            if current == created_multiply {
                connect_output_to_input(
                    &mut state,
                    "root",
                    &created_param,
                    &current,
                    1 - current_input,
                );
            }
        }
        connect_output_to_input(&mut state, "root", chain.last().unwrap(), &mul.id, 0);

        assert_writeback_fuzz_compiles(source, &state, seed + 2000);
    }
}

#[test]
fn writeback_created_macro_call_stays_after_created_macro_definition_when_file_macros_are_late() {
    let source = r#"
        (def value2 0.3)
        (def value1 3.0)
        (param idx @min 0 @max 3)
        (def phasor1 (phasor value1))
        (defmacro ramp-to-lfo-shapes (ramp)
          (tuple ramp ramp ramp ramp))
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def velocity (in 3 @name velocity))
        (def trigger (in 4 @name trigger))
        (def mod1 (in 5 @name mod1 @modulator 1))
        (def mod2 (in 6 @name mod2 @modulator 2))
        (def mod3 (in 7 @name mod3 @modulator 3))
        (def mod4 (in 8 @name mod4 @modulator 4))
        (def phase (phasor pitch))
        (def (shape1 shape2 shape3 shape4) (ramp-to-lfo-shapes phase))
        (def osc (scale shape1 0.0 1.0 -1.0 1.0))
        (out (* osc velocity) 1 @name audio)
    "#;
    let root_patch = parse(source);
    let phasor = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let osc = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "osc")
        .unwrap();
    let osc_inlet = 0;
    let shape_to_osc = root_patch
        .connections
        .iter()
        .find(|connection| connection.to_node == osc.id && connection.to_input == osc_inlet)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(shape_to_osc),
        ));

    let created_macro =
        allocate_created_text_node(&mut state, "root", "defmacro *ramp-to-trapezoid*");
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &created_macro,
    ));
    let macro_source = r#"(defmacro ramp-to-trapezoid (ramp rise fall)
          (def r-val (max rise 0.0001))
          (def f-val (max fall 0.0001))
          (min (/ ramp r-val) (/ (- 1.0 ramp) f-val)))"#;
    state
        .edit_state
        .created_macros
        .get_mut("ramp-to-trapezoid")
        .unwrap()
        .source = Some(macro_source.to_string());
    connect_output_to_input(&mut state, "root", &phasor.id, &created_macro, 0);
    connect_output_to_input(&mut state, "root", "value2", &created_macro, 1);
    connect_output_to_input(&mut state, "root", "value2", &created_macro, 2);
    connect_output_to_input(&mut state, "root", &created_macro, &osc.id, osc_inlet);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    let macro_index = emitted
        .find("(defmacro ramp-to-trapezoid")
        .expect("created macro definition should be emitted");
    let call_index = emitted
        .find("(def ramp-to-trapezoid1 (ramp-to-trapezoid phasor1 value2 value2))")
        .expect("created macro call should be emitted");
    assert!(
        macro_index < call_index,
        "created macro definition must precede generated call:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted).unwrap();
}

#[test]
fn writeback_hoists_late_macro_definitions_before_root_calls() {
    let source = r#"
        (def value2 0.3)
        (def value1 3.0)
        (def phasor1 (phasor value1))
        (def ramp-to-trapezoid1 (ramp-to-trapezoid phasor1 value2 value2))
        (defmacro ramp-to-trapezoid (ramp rise fall)
          (def r-val (max rise 0.0001))
          (def f-val (max fall 0.0001))
          (min (/ ramp r-val) (/ (- 1.0 ramp) f-val)))
        (out ramp-to-trapezoid1 1)
    "#;

    let emitted = emit_patch_writeback(
        source,
        PatcherIntent::Instrument,
        &PatcherInteractionState::default(),
    )
    .unwrap();

    let macro_index = emitted
        .find("(defmacro ramp-to-trapezoid")
        .expect("macro definition should be emitted");
    let call_index = emitted
        .find("(def ramp-to-trapezoid1 (ramp-to-trapezoid phasor1 value2 value2))")
        .expect("macro call should remain emitted");
    assert!(
        macro_index < call_index,
        "macro definition must precede root call:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted).unwrap();
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
    let mod4_index = emitted
        .find("(def mod4 (in 8 @name mod4 @modulator 4))")
        .expect("instrument modulator inputs should be present");
    let modulated_index = emitted
        .find("(def modulated1 (mod newparam))")
        .expect("created mod accessor should be materialized");
    let phase_index = emitted
        .find("(def phase (phasor add1))")
        .expect("source consumer should be rewritten");
    assert!(mod4_index < modulated_index);
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

    let mod4_index = emitted
        .find("(def mod4 (in 8 @name mod4 @modulator 4))")
        .expect("instrument modulator inputs should be present");
    let xyz_index = emitted
        .find("(param xyz @default 0.0 @min 0.0 @max 1.0 @mod true @mod-mode additive)")
        .expect("created modulatable param should be emitted");
    let gain_index = emitted
        .find("(param gain @default 0.5 @min 0.0 @max 1.0 @mod true @mod-mode additive)")
        .expect("existing gain param should remain present");
    assert!(mod4_index < xyz_index);
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

    let mod4_index = emitted
        .find("(def mod4 (in 8 @name mod4 @modulator 4))")
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
    assert!(mod4_index < param_index);
    assert!(param_index < modulated_index);
    assert!(modulated_index < op_index);
}

fn badmallet_modulatable_param_source() -> &'static str {
    r#"
        (param marimbamix)
        (def value8 1.0)
        (def sub1 (- value8 marimbamix))
        (def mul1 (* marimbamix 0.5))
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def velocity (in 3 @name velocity))
        (def trigger (in 4 @name trigger))
        (def mod1 (in 5 @name mod1 @modulator 1))
        (def mod2 (in 6 @name mod2 @modulator 2))
        (def mod3 (in 7 @name mod3 @modulator 3))
        (def mod4 (in 8 @name mod4 @modulator 4))
        (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
        (def add1 (+ sub1 mul1))
        (out (* add1 velocity (mod gain)) 1 @name audio)
    "#
}

#[test]
fn writeback_existing_param_made_modulatable_follows_modulator_inputs() {
    let source = badmallet_modulatable_param_source();
    let patch = parse(source);
    let param = patch
        .nodes
        .iter()
        .find(|node| node.id == "marimbamix")
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", param, node_display_label(param));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param.id))
        .unwrap()
        .text = "param marimbamix @min 0 @max 1 @mod true @mod-mode additive".to_string();

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    let mod4_index = emitted
        .find("(def mod4 (in 8 @name mod4 @modulator 4))")
        .expect("instrument modulator inputs should be present");
    let param_index = emitted
        .find("(param marimbamix @min 0.0 @max 1.0 @mod true @mod-mode additive)")
        .expect("edited modulatable param should be present");
    assert!(mod4_index < param_index);
    compile_patch_source_with_dgenlisp(&emitted).unwrap();
}

#[test]
fn writeback_created_mod_from_existing_modulatable_param_follows_modulator_inputs() {
    let source = badmallet_modulatable_param_source();
    let patch = parse(source);
    let param = patch
        .nodes
        .iter()
        .find(|node| node.id == "marimbamix")
        .unwrap();
    let sub1 = patch.nodes.iter().find(|node| node.id == "sub1").unwrap();
    let param_to_sub1 = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == param.id && connection.to_node == sub1.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", param, node_display_label(param));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param.id))
        .unwrap()
        .text = "param marimbamix @min 0 @max 1 @mod true @mod-mode additive".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(param_to_sub1),
        ));
    let mod_node = allocate_created_text_node(&mut state, "root", "mod");
    connect_output_to_input(&mut state, "root", &param.id, &mod_node, 0);
    connect_output_to_input(&mut state, "root", &mod_node, &sub1.id, 1);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    let mod4_index = emitted
        .find("(def mod4 (in 8 @name mod4 @modulator 4))")
        .expect("instrument modulator inputs should be present");
    let param_index = emitted
        .find("(param marimbamix @min 0.0 @max 1.0 @mod true @mod-mode additive)")
        .expect("edited modulatable param should be present");
    let modulated_index = emitted
        .find("(def modulated1 (mod marimbamix))")
        .expect("created mod accessor should be present");
    let sub1_index = emitted
        .find("(def sub1 (- value8 modulated1))")
        .expect("source consumer should use the created mod accessor");
    assert!(mod4_index < param_index, "{emitted}");
    assert!(param_index < modulated_index, "{emitted}");
    assert!(modulated_index < sub1_index, "{emitted}");
    compile_patch_source_with_dgenlisp(&emitted).unwrap();
}

#[test]
fn effect_writeback_created_mod_from_newly_modulatable_early_param_precedes_consumers() {
    let source = r#"
        (param am @min 0.1 @max 320 @default 0.1)
        (def phasor1 (phasor am))
        (def mul3 (* phasor1 twopi))
        (def cos1 (cos mul3))
        (def input_l (in 1 @name Left))
        (def mul1 (* input_l cos1))
        (def input_r (in 2 @name Right))
        (def mul2 (* input_r cos1))
        (def mix-amt (param mix @min 0 @max 1 @default 0.5))
        (out (mix input_l mul1 mix-amt) 1 @name Left)
        (out (mix input_r mul2 mix-amt) 2 @name Right)
    "#;
    let patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let param = patch.nodes.iter().find(|node| node.id == "am").unwrap();
    let phasor = patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let param_to_phasor = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == param.id && connection.to_node == phasor.id)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", param, node_display_label(param));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &param.id))
        .unwrap()
        .text = "param am @min 0.1 @max 320 @default 0.1 @mod true @mod-mode additive".to_string();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(param_to_phasor),
        ));
    let mod_node = allocate_created_text_node(&mut state, "root", "mod");
    connect_output_to_input(&mut state, "root", &param.id, &mod_node, 0);
    connect_output_to_input(&mut state, "root", &mod_node, &phasor.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();

    let param_index = emitted
        .find("(param am @min 0.1 @max 320.0 @default 0.1 @mod true @mod-mode additive)")
        .expect("edited modulatable param should be present");
    let mod4_index = emitted
        .find("(def mod4 (in 6 @name mod4 @modulator 4))")
        .expect("effect modulator inputs should be present");
    let modulated_index = emitted
        .find("(def modulated1 (mod am))")
        .expect("created mod accessor should be present");
    let phasor_index = emitted
        .find("(def phasor1 (phasor modulated1))")
        .expect("source consumer should use created mod accessor");
    assert!(param_index < modulated_index, "{emitted}");
    assert!(mod4_index < modulated_index, "{emitted}");
    assert!(modulated_index < phasor_index, "{emitted}");
    compile_patch_source_with_dgenlisp(&emitted).unwrap();
}

#[test]
fn writeback_created_modulatable_param_replacing_early_source_input_precedes_mod_accessor() {
    let source = r#"
        (def value1 2.0)
        (defmacro cymbal_model (trig amount) (* trig amount))
        (def phasor1 (phasor value1))
        (def ramp2trig1 (ramp2trig phasor1))
        (def hit (cymbal_model ramp2trig1 0.5))
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def velocity (in 3 @name velocity))
        (def trigger (in 4 @name trigger))
        (def mod1 (in 5 @name mod1 @modulator 1))
        (def mod2 (in 6 @name mod2 @modulator 2))
        (def mod3 (in 7 @name mod3 @modulator 3))
        (def mod4 (in 8 @name mod4 @modulator 4))
        (out hit 1)
    "#;
    let patch = parse(source);
    let phasor = patch
        .nodes
        .iter()
        .find(|node| node.id == "phasor1")
        .unwrap();
    let ramp2trig = patch
        .nodes
        .iter()
        .find(|node| node.id == "ramp2trig1")
        .unwrap();
    let phasor_to_ramp2trig = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == phasor.id && connection.to_node == ramp2trig.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(phasor_to_ramp2trig),
        ));
    let param = allocate_created_text_node(
        &mut state,
        "root",
        "param ramp @min 0 @max 1 @mod true @mod-mode additive",
    );
    let mod_node = allocate_created_text_node(&mut state, "root", "mod");
    connect_output_to_input(&mut state, "root", &param, &mod_node, 0);
    connect_output_to_input(&mut state, "root", &mod_node, &ramp2trig.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    let mod4_index = emitted
        .find("(def mod4 (in 8 @name mod4 @modulator 4))")
        .expect("instrument modulator inputs should be present");
    let param_index = emitted
        .find("(param ramp @min 0.0 @max 1.0 @mod true @mod-mode additive)")
        .expect("created modulatable param should be emitted");
    let modulated_index = emitted
        .find("(def modulated1 (mod ramp))")
        .expect("created mod accessor should be emitted");
    let ramp2trig_index = emitted
        .find("(def ramp2trig1 (ramp2trig modulated1))")
        .expect("source consumer should use generated mod accessor");
    assert!(mod4_index < param_index, "{emitted}");
    assert!(param_index < modulated_index, "{emitted}");
    assert!(modulated_index < ramp2trig_index, "{emitted}");
    compile_patch_source_with_dgenlisp(&emitted).unwrap();
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
        "(defmacro phasor3 (sig) sig)\n(param phasor1)\n(make-history phasor2)\n(def sig (in 1))\n(def phasor4 (phasor sig))\n(out phasor4 1)"
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
        "(defmacro ap (sig)\n  (def phasor1 (phasor sig))\n  phasor1)\n(def phasor1 (phasor 1.0))"
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
fn writeback_cable_create_moves_destination_after_new_later_dependency() {
    let source = r#"
        (def phase (phasor 2))
        (def velocity (in 4 @name velocity))
        (out phase 1 @name audio)
    "#;
    let patch = parse(source);
    let velocity = patch
        .nodes
        .iter()
        .find(|node| node.id == "velocity")
        .unwrap();
    let phasor = patch.nodes.iter().find(|node| node.id == "phase").unwrap();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: velocity.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor.id.clone(),
            input_index: 1,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def velocity (in 4 @name velocity))\n(def phase (phasor 2.0 velocity))\n(out phase 1 @name audio)"
    );
}

#[test]
fn writeback_cable_create_moves_destination_with_existing_dependents() {
    let source = r#"
        (def phase (phasor 2))
        (def trig (ramp2trig phase))
        (def hit (cymbal_model trig 0.5))
        (def velocity (in 3 @name velocity))
        (def trigger (in 4 @name trigger))
        (out hit 1 @name audio)
    "#;
    let patch = parse(source);
    let trigger = patch
        .nodes
        .iter()
        .find(|node| node.id == "trigger")
        .unwrap();
    let phasor = patch.nodes.iter().find(|node| node.id == "phase").unwrap();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: trigger.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor.id.clone(),
            input_index: 1,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def velocity (in 3 @name velocity))\n(def trigger (in 4 @name trigger))\n(def phase (phasor 2.0 trigger))\n(def trig (ramp2trig phase))\n(def hit (cymbal_model trig 0.5))\n(out hit 1 @name audio)"
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
fn writeback_cable_create_inserts_missing_destination_arg_before_attributes() {
    let source = r#"
        (def a (in 1))
        (def b (in 2))
        (def result (foo a @mode fast))
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
        "(def a (in 1))\n(def b (in 2))\n(def result (foo a b @mode fast))"
    );
}

#[test]
fn writeback_cable_create_fills_missing_destination_arg_gaps() {
    let source = r#"
        (def a (in 1))
        (def b (in 2))
        (def result (foo a))
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
            input_index: 2,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def a (in 1))\n(def b (in 2))\n(def result (foo a __patcher_missing_input__ b))"
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
fn writeback_source_cable_rewire_moves_destination_after_new_later_dependency() {
    let source = r#"
        (def pitch (in 1 @name pitch))
        (def phase (phasor pitch))
        (def velocity (in 4 @name velocity))
        (out phase 1 @name audio)
    "#;
    let patch = parse(source);
    let pitch = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
    let velocity = patch
        .nodes
        .iter()
        .find(|node| node.id == "velocity")
        .unwrap();
    let phasor = patch.nodes.iter().find(|node| node.id == "phase").unwrap();
    let pitch_to_phasor = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == pitch.id && connection.to_node == phasor.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(pitch_to_phasor),
        ));
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: velocity.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def pitch (in 1 @name pitch))\n(def velocity (in 4 @name velocity))\n(def phase (phasor velocity))\n(out phase 1 @name audio)"
    );
}

#[test]
fn writeback_macro_cable_create_moves_destination_after_new_later_dependency() {
    let source = r#"
        (defmacro op (input)
          (def phase (phasor 2))
          (def shaped (* input 0.5))
          phase)
        (def sig (in 1 @name sig))
        (def out1 (op sig))
        (out out1 1 @name audio)
    "#;
    let patch = parse(source);
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
    let phasor = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "phase")
        .unwrap();
    let mut state = PatcherInteractionState::default();
    allocate_created_connection(
        &mut state,
        "macro:op",
        OutputPortRef {
            node_id: shaped.id.clone(),
            output_index: 0,
        },
        InputPortRef {
            node_id: phasor.id.clone(),
            input_index: 1,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro op (input)\n  (def shaped (* input 0.5))\n  (def phase (phasor 2.0 shaped))\n  phase)\n(def sig (in 1 @name sig))\n(def out1 (op sig))\n(out out1 1 @name audio)"
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
fn writeback_delete_then_readd_inline_out_connection_preserves_source() {
    let source = "(out (phasor 1) 1)";
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
            node_id: out.id.clone(),
            input_index: 0,
        },
    );

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(out (phasor 1.0) 1)"
    );
}

#[test]
fn writeback_delete_then_recreate_out_connection_emits_out_form() {
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
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &out.id));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(sig_to_out),
        ));
    let created_out = allocate_created_text_node(&mut state, "root", "out 1");
    connect_output_to_input(&mut state, "root", &sig.id, &created_out, 0);

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))\n(out sig 1)"
    );
}

#[test]
fn writeback_delete_then_recreate_out_connection_preserves_nested_source() {
    let source = r#"
        (def sig (in 1))
        (out (* sig 0.5) 1)
    "#;
    let patch = parse(source);
    let multiply = patch.nodes.iter().find(|node| node.op == "*").unwrap();
    let out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let multiply_to_out = patch
        .connections
        .iter()
        .find(|connection| connection.from_node == multiply.id && connection.to_node == out.id)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &out.id));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(multiply_to_out),
        ));
    let created_out = allocate_created_text_node(&mut state, "root", "out 1");
    connect_output_to_input(&mut state, "root", &multiply.id, &created_out, 0);

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))\n(out (* sig 0.5) 1)"
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
fn writeback_deleting_last_macro_instance_removes_macro_definition() {
    let source = r#"
        (defmacro op (input) (* input 2))
        (def sig (in 1))
        (def shaped (op sig))
    "#;
    let patch = parse(source);
    let shaped = patch.nodes.iter().find(|node| node.id == "shaped").unwrap();
    assert_eq!(shaped.kind, NodeKind::MacroInstance);
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &shaped.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))"
    );
}

#[test]
fn writeback_deleting_node_removes_existing_unreferenced_macro_definition() {
    let source = r#"
        (defmacro orphan (input) (* input 2))
        (def sig (in 1))
        (def unused (phasor sig))
    "#;
    let patch = parse(source);
    let unused = patch.nodes.iter().find(|node| node.id == "unused").unwrap();
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &unused.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(def sig (in 1))"
    );
}

#[test]
fn writeback_deleting_one_macro_instance_keeps_macro_definition_when_still_used() {
    let source = r#"
        (defmacro op (input) (* input 2))
        (def sig (in 1))
        (def shaped (op sig))
        (def shaped2 (op sig))
    "#;
    let patch = parse(source);
    let shaped = patch.nodes.iter().find(|node| node.id == "shaped").unwrap();
    assert_eq!(shaped.kind, NodeKind::MacroInstance);
    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &shaped.id));

    assert_eq!(
        emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap(),
        "(defmacro op (input) (* input 2.0))\n(def sig (in 1))\n(def shaped2 (op sig))"
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
fn direct_non_mod_params_inline_on_non_first_inlets() {
    let patch = parse(
        r#"
            (def gate (in 1 @name gate))
            (def trigger (in 2 @name trigger))
            (param attack @default 5)
            (param decay @default 120)
            (param sustain @default 0.8)
            (param release @default 180)
            (def env (adsr gate trigger attack decay sustain release))
            "#,
    );
    let env = patch
        .nodes
        .iter()
        .find(|node| node.id == "env")
        .expect("env node");

    assert_eq!(
        node_display_label(env),
        "adsr ? attack decay sustain release"
    );
    assert_eq!(node_display_input_slots(env), vec![1, 2, 3, 4, 5]);
    for (input_index, name) in [(2, "attack"), (3, "decay"), (4, "sustain"), (5, "release")] {
        let connection = source_connection_for_input(&patch, "env", input_index);
        assert_eq!(connection.presentation, InputPresentation::InlineRawParam);
        assert_eq!(connection.presentation_override, None);
        assert_eq!(
            env.inline_inputs
                .get(input_index)
                .and_then(|input| input.as_ref())
                .map(|input| input.label()),
            Some(name.to_string())
        );
    }
    assert_eq!(
        source_connection_for_input(&patch, "env", 0).presentation,
        InputPresentation::Cable
    );
    assert_eq!(
        source_connection_for_input(&patch, "env", 1).presentation,
        InputPresentation::Cable
    );

    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get("env").map(Vec::as_slice),
        Some(&[0, 1][..])
    );
}

#[test]
fn metadata_params_inline_on_non_first_inlets() {
    let patch = parse(
        r#"
            (def gate (in 1 @name gate))
            (def trigger (in 2 @name trigger))
            (param attack @group amp @env amp-env @role attack @default 5)
            (param decay @group amp @env amp-env @role decay @default 120)
            (param sustain @group amp @env amp-env @role sustain @default 0.8)
            (param release @group amp @env amp-env @role release @default 180)
            (def env (adsr gate trigger attack decay sustain release))
            "#,
    );
    let env = patch
        .nodes
        .iter()
        .find(|node| node.id == "env")
        .expect("env node");

    assert_eq!(
        node_display_label(env),
        "adsr ? attack decay sustain release"
    );
    assert_eq!(node_display_input_slots(env), vec![1, 2, 3, 4, 5]);
    for (input_index, name) in [(2, "attack"), (3, "decay"), (4, "sustain"), (5, "release")] {
        let connection = source_connection_for_input(&patch, "env", input_index);
        assert_eq!(connection.presentation, InputPresentation::InlineRawParam);
        assert_eq!(connection.presentation_override, None);
        assert_eq!(
            env.inline_inputs
                .get(input_index)
                .and_then(|input| input.as_ref())
                .map(|input| input.label()),
            Some(name.to_string())
        );
    }
    assert_eq!(
        source_connection_for_input(&patch, "env", 0).presentation,
        InputPresentation::Cable
    );
    assert_eq!(
        source_connection_for_input(&patch, "env", 1).presentation,
        InputPresentation::Cable
    );
    assert_eq!(
        patch_input_indices(&patch).get("env").map(Vec::as_slice),
        Some(&[0, 1][..])
    );
}

#[test]
fn moving_metadata_param_preserves_inline_adsr_param_presentation() {
    let root_patch = parse(
        r#"
            (def gate (in 1 @name gate))
            (def trigger (in 2 @name trigger))
            (param attack @group amp @env amp-env @role attack @default 5 @min 0 @max 1000 @unit ms)
            (param decay @group amp @env amp-env @role decay @default 120 @min 1 @max 2000 @unit ms)
            (param sustain @group amp @env amp-env @role sustain @default 0.8 @min 0 @max 1)
            (param release @group amp @env amp-env @role release @default 180 @min 1 @max 5000 @unit ms)
            (def env (adsr gate trigger attack decay sustain release))
            "#,
    );
    let attack = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "attack")
        .expect("attack param");
    let mut state = PatcherInteractionState::default();
    set_node_edit_position(
        &mut state,
        "root",
        attack,
        (attack.position.0 + 8.0, attack.position.1 + 2.0),
        node_display_label(attack),
    );

    let patch = patch_with_interaction_state(root_patch, &state, "root");
    let attack = patch
        .nodes
        .iter()
        .find(|node| node.id == "attack")
        .expect("moved attack param");
    assert_eq!(
        attack.param.as_ref().map(|param| param.name.as_str()),
        Some("attack")
    );

    let env = patch
        .nodes
        .iter()
        .find(|node| node.id == "env")
        .expect("env node");
    assert_eq!(
        node_display_label(env),
        "adsr ? attack decay sustain release"
    );
    for (input_index, name) in [(2, "attack"), (3, "decay"), (4, "sustain"), (5, "release")] {
        let connection = source_connection_for_input(&patch, "env", input_index);
        assert_eq!(connection.presentation, InputPresentation::InlineRawParam);
        assert_eq!(
            env.inline_inputs
                .get(input_index)
                .and_then(|input| input.as_ref())
                .map(|input| input.label()),
            Some(name.to_string())
        );
    }
    assert_eq!(
        patch_input_indices(&patch).get("env").map(Vec::as_slice),
        Some(&[0, 1][..])
    );
}

#[test]
fn inline_params_keep_placeholder_for_cabled_second_inlet() {
    let patch = parse(
        r#"
            (def gate (in 1 @name gate))
            (def trigger (in 2 @name trigger))
            (param amp_attack @default 5)
            (param amp_decay @default 120)
            (param amp_sustain @default 0.8)
            (param amp_release @default 180)
            (def env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))
            "#,
    );
    let env = patch
        .nodes
        .iter()
        .find(|node| node.id == "env")
        .expect("env node");

    assert_eq!(
        node_display_label(env),
        "adsr ? amp_attack amp_decay amp_sustain amp_release"
    );
    assert_eq!(node_display_input_slots(env), vec![1, 2, 3, 4, 5]);
    assert_eq!(
        patch_input_indices(&patch).get("env").map(Vec::as_slice),
        Some(&[0, 1][..])
    );
}

#[test]
fn writeback_preserves_cabled_second_inlet_when_editing_later_inline_param() {
    let source = r#"
        (def gate (in 1 @name gate))
        (def trigger (in 2 @name trigger))
        (param attack @default 5)
        (param decay @default 120)
        (param sustain @default 0.8)
        (param release @default 180)
        (def env (adsr gate trigger attack decay sustain release))
    "#;
    let patch = parse(source);
    let env = patch
        .nodes
        .iter()
        .find(|node| node.id == "env")
        .expect("env node");
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", env, node_display_label(env));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", "env"))
        .unwrap()
        .text = "adsr ? attack decay sustain 240".to_string();

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    assert!(
        emitted.contains("(def env (adsr gate trigger attack decay sustain 240.0))"),
        "{emitted}"
    );
    assert!(!emitted.contains("(adsr gate ?"), "{emitted}");
}

#[test]
fn writeback_operator_only_edit_drops_stale_extra_source_args() {
    let source = r#"
        (def signal (in 1 @name signal))
        (def delay_time (in 2 @name delay-time))
        (def delayed (delay signal delay_time 4000.0))
    "#;
    let patch = parse(source);
    let delay = patch
        .nodes
        .iter()
        .find(|node| node.id == "delayed")
        .expect("delay node");
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", delay, node_display_label(delay));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", "delayed"))
        .unwrap()
        .text = "delay".to_string();

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    assert!(
        emitted.contains("(def delayed (delay signal delay_time))"),
        "{emitted}"
    );
    assert!(!emitted.contains("4000"), "{emitted}");
}

#[test]
fn modulatable_params_inline_only_when_read_through_nested_mod() {
    let direct = parse(
        r#"
            (def signal (in 1))
            (param gain @default 0.5 @mod true @mod-mode additive)
            (def scaled (* signal gain))
            "#,
    );
    let scaled = direct
        .nodes
        .iter()
        .find(|node| node.id == "scaled")
        .expect("scaled node");
    let gain_connection = source_connection_for_input(&direct, "scaled", 1);
    assert_eq!(gain_connection.presentation, InputPresentation::Cable);
    assert_eq!(
        scaled.inline_inputs.get(1).and_then(|input| input.as_ref()),
        None
    );
    assert_eq!(
        patch_input_indices(&direct)
            .get("scaled")
            .map(Vec::as_slice),
        Some(&[0, 1][..])
    );

    let modulated = parse(
        r#"
            (def signal (in 1))
            (param gain @default 0.5 @mod true @mod-mode additive)
            (def scaled (* signal (mod gain)))
            "#,
    );
    let mod_connection = source_connection_for_input(&modulated, "scaled", 1);
    assert_eq!(
        mod_connection.presentation,
        InputPresentation::InlineModParam
    );
    assert!(modulated.nodes.iter().any(|node| node.op == "mod"));
    let scaled = modulated
        .nodes
        .iter()
        .find(|node| node.id == "scaled")
        .expect("scaled node");
    assert_eq!(node_display_label(scaled), "* gain~");
    assert_eq!(
        patch_input_indices(&modulated)
            .get("scaled")
            .map(Vec::as_slice),
        Some(&[0][..])
    );
    assert!(
        ordered_patch_nodes(&modulated, &PatcherInteractionState::default(), "root")
            .iter()
            .all(|node| node.op != "mod")
    );
}

#[test]
fn moving_metadata_mod_param_preserves_inline_mod_presentation() {
    let root_patch = parse(
        r#"
            (def signal (in 1))
            (param cutoff @group filter @env filter_env @role cutoff @default 1000 @min 50 @max 10000 @mod true @mod-mode additive)
            (param res @group filter @env filter_env @role resonance @default 1 @min 0.5 @max 10 @mod true @mod-mode additive)
            (def filtered (svf signal (mod cutoff) (mod res) 0))
            "#,
    );
    let cutoff = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "cutoff")
        .expect("cutoff param");
    let mut state = PatcherInteractionState::default();
    set_node_edit_position(
        &mut state,
        "root",
        cutoff,
        (cutoff.position.0 + 4.0, cutoff.position.1 + 2.0),
        node_display_label(cutoff),
    );

    let patch = patch_with_interaction_state(root_patch, &state, "root");
    let cutoff = patch
        .nodes
        .iter()
        .find(|node| node.id == "cutoff")
        .expect("moved cutoff param");
    assert_eq!(
        cutoff
            .param
            .as_ref()
            .map(|param| (param.name.as_str(), param.modulatable)),
        Some(("cutoff", true))
    );

    let filtered = patch
        .nodes
        .iter()
        .find(|node| node.id == "filtered")
        .expect("filtered node");
    assert_eq!(node_display_label(filtered), "svf cutoff~ res~ 0");
    for (input_index, name) in [(1, "cutoff~"), (2, "res~")] {
        let connection = source_connection_for_input(&patch, "filtered", input_index);
        assert_eq!(connection.presentation, InputPresentation::InlineModParam);
        assert_eq!(
            filtered
                .inline_inputs
                .get(input_index)
                .and_then(|input| input.as_ref())
                .map(|input| input.label()),
            Some(name.to_string())
        );
    }
    assert!(
        ordered_patch_nodes(&patch, &PatcherInteractionState::default(), "root")
            .iter()
            .all(|node| node.op != "mod"),
        "touching a modulatable param must not expose the hidden mod accessor nodes"
    );
    assert_eq!(
        patch_input_indices(&patch)
            .get("filtered")
            .map(Vec::as_slice),
        Some(&[0][..])
    );
}

#[test]
fn generated_layout_sidecar_omits_inlined_mod_nodes_and_cables() {
    let patch = parse(
        r#"
            (def signal (in 1))
            (param gain @default 0.5 @mod true @mod-mode additive)
            (def scaled (* signal (mod gain)))
            "#,
    );
    let scaled = patch
        .nodes
        .iter()
        .find(|node| node.id == "scaled")
        .expect("scaled node");
    let mod_node = patch
        .nodes
        .iter()
        .find(|node| node.op == "mod")
        .expect("nested mod node");
    assert_eq!(node_display_label(scaled), "* gain~");
    assert!(hidden_inline_node_ids(&patch).contains(&mod_node.id));

    let layout_json: serde_json::Value = serde_json::from_str(
        &sidecar::current_layout_json(&patch, &PatcherInteractionState::default()).unwrap(),
    )
    .unwrap();
    assert!(
        layout_json["root"]["nodes"]
            .as_object()
            .expect("layout nodes")
            .get(&mod_node.id)
            .is_none(),
        "hidden inline mod node should not be persisted as a visible layout node"
    );
    assert!(
        layout_json["root"]["cables"]
            .as_object()
            .into_iter()
            .flat_map(|cables| cables.keys())
            .all(|key| !key.contains(&mod_node.id)),
        "hidden inline mod cables should not be persisted as visible cables"
    );
    assert!(
        layout_json["root"].get("inputPresentation").is_none(),
        "default inline-mod presentation should not need a sidecar override"
    );
}

#[test]
fn cable_presentation_override_exposes_default_inline_param() {
    let patch = parse(
        r#"
            (def gate (in 1 @name gate))
            (def trigger (in 2 @name trigger))
            (param attack @default 5)
            (param release @default 180)
            (def env (adsr gate trigger attack 120 0.8 release))
            "#,
    );
    assert_eq!(
        source_connection_for_input(&patch, "env", 2).presentation,
        InputPresentation::InlineRawParam
    );

    let mut state = PatcherInteractionState::default();
    set_input_presentation_override(&mut state, "root", "env", 2, InputPresentation::Cable);
    let patched = patch_with_interaction_state(patch.clone(), &state, "root");
    let env = patched
        .nodes
        .iter()
        .find(|node| node.id == "env")
        .expect("env node");
    let attack_connection = source_connection_for_input(&patched, "env", 2);

    assert_eq!(attack_connection.presentation, InputPresentation::Cable);
    assert_eq!(
        attack_connection.presentation_override,
        Some(InputPresentation::Cable)
    );
    assert_eq!(
        env.inline_inputs.get(2).and_then(|input| input.as_ref()),
        None
    );

    let layout_json: serde_json::Value =
        serde_json::from_str(&sidecar::current_layout_json(&patch, &state).unwrap()).unwrap();
    assert_eq!(layout_json["root"]["inputPresentation"]["env:2"], "cable");
}

#[test]
fn editor_mod_suffix_expands_to_mod_expression_and_inline_mod_sidecar() {
    let source = r#"
        (def signal (in 1))
        (param cutoff @default 1000 @mod true @mod-mode additive)
        (def filtered (svf signal cutoff 1 0))
        (out filtered 1)
    "#;
    let root_patch = parse(source);
    let filtered = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "filtered")
        .expect("filtered node");
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", filtered, node_display_label(filtered));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", "filtered"))
        .unwrap()
        .text = "svf cutoff~ 1 0".to_string();

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(def filtered (svf signal (mod cutoff) 1.0 0.0))"),
        "expected cutoff~ to write back as a canonical mod expression:\n{emitted}"
    );

    let mut emitted_patch = parse_patch_source(&emitted, PatcherIntent::Instrument).unwrap();
    let layout_json: serde_json::Value = serde_json::from_str(
        &sidecar::emitted_layout_json_with_node_map(
            &mut emitted_patch,
            &root_patch,
            &state,
            &HashMap::new(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        layout_json["root"]["inputPresentation"]["filtered:1"],
        "inline-mod-param"
    );
    let filtered = emitted_patch
        .nodes
        .iter()
        .find(|node| node.id == "filtered")
        .expect("filtered node");
    assert_eq!(node_display_label(filtered), "svf cutoff~ 1 0");
}

#[test]
fn editor_mod_suffix_requires_modulatable_param() {
    let source = r#"
        (def signal (in 1))
        (param cutoff @default 1000)
        (def filtered (svf signal cutoff 1 0))
    "#;
    let root_patch = parse(source);
    let filtered = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "filtered")
        .expect("filtered node");
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", filtered, node_display_label(filtered));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", "filtered"))
        .unwrap()
        .text = "svf cutoff~ 1 0".to_string();

    let error = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap_err();
    match error {
        WriteBackError::InvalidEdit { reason, .. } => {
            assert!(reason.contains("requires `cutoff` to be declared as a modulatable param"));
        }
        other => panic!("expected invalid edit for non-modulatable cutoff~, got {other:?}"),
    }
}

#[test]
fn writeback_created_modulatable_param_can_feed_created_inline_mod_shorthand() {
    let source = r#"
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def velocity (in 3 @name velocity))
        (def trigger (in 4 @name trigger))
        (def mod1 (in 5 @name mod1 @modulator 1))
        (def mod2 (in 6 @name mod2 @modulator 2))
        (def mod3 (in 7 @name mod3 @modulator 3))
        (def mod4 (in 8 @name mod4 @modulator 4))

        (param attack @default 5 @min 0 @max 1000 @unit ms)
        (param decay @default 120 @min 1 @max 2000 @unit ms)
        (param sustain @default 0.8 @min 0 @max 1)
        (param release @default 180 @min 1 @max 5000 @unit ms)
        (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)

        (def env (adsr gate trigger attack decay sustain release))
        (def phase (phasor pitch))
        (out (* phase env velocity (mod gain)) 1 @name audio)
    "#;
    let patch = parse(source);
    let phase_to_multiply = patch
        .connections
        .iter()
        .find(|connection| {
            connection.from_node == "phase"
                && patch
                    .nodes
                    .iter()
                    .find(|node| node.id == connection.to_node)
                    .is_some_and(|node| node.op == "*")
        })
        .expect("starter patch should connect phase into the final multiply");

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(phase_to_multiply),
        ));
    let _triparam = allocate_created_text_node(
        &mut state,
        "root",
        "param triparam @default 0.5 @min 0 @max 1 @mod true @mod-mode additive",
    );
    let triangle = allocate_created_text_node(&mut state, "root", "triangle triparam~");
    connect_output_to_input(&mut state, "root", "phase", &triangle, 0);
    connect_output_to_input(
        &mut state,
        "root",
        &triangle,
        &phase_to_multiply.to_node,
        phase_to_multiply.to_input,
    );

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();

    assert!(
        emitted.contains(
            "(param triparam @default 0.5 @min 0.0 @max 1.0 @mod true @mod-mode additive)"
        ),
        "{emitted}"
    );
    assert!(
        emitted.contains("(def triangle1 (triangle phase (mod triparam)))"),
        "{emitted}"
    );
    assert!(
        emitted.contains("(out (* triangle1 env velocity (mod gain)) 1 @name audio)"),
        "{emitted}"
    );
    assert!(!emitted.contains("triparam~"), "{emitted}");
    parse_patch_source(&emitted, PatcherIntent::Instrument).unwrap();
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
fn display_label_hides_missing_input_sentinel_for_trailing_unconnected_inlet() {
    let patch = parse(
        r#"
            (defmacro gain2 (x) (* x 2))
            (def shaped (gain2 __patcher_missing_input__))
            "#,
    );
    let shaped = patch
        .nodes
        .iter()
        .find(|node| node.id == "shaped")
        .expect("macro instance");

    assert_eq!(node_display_label(shaped), "gain2");
    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&shaped.id).map(Vec::as_slice),
        Some(&[0][..])
    );
}

#[test]
fn display_label_shows_placeholder_for_missing_input_before_later_literal() {
    let patch = parse(
        r#"
            (defmacro ap (sig g d) sig)
            (def signal (in 1))
            (def tapped (ap signal __patcher_missing_input__ 0.6))
            "#,
    );
    let tapped = patch
        .nodes
        .iter()
        .find(|node| node.id == "tapped")
        .expect("macro instance");

    assert_eq!(node_display_label(tapped), "ap ? 0.6");
}

#[test]
fn source_backed_missing_input_sentinel_preserves_all_operator_inlets() {
    let patch = parse(
        "(def mixed (mix __patcher_missing_input__ __patcher_missing_input__ __patcher_missing_input__))",
    );
    let mixed = patch
        .nodes
        .iter()
        .find(|node| node.id == "mixed")
        .expect("mix node");

    assert_eq!(node_display_label(mixed), "mix");
    let input_indices = patch_input_indices(&patch);
    assert_eq!(
        input_indices.get(&mixed.id).map(Vec::as_slice),
        Some(&[0, 1, 2][..])
    );
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    assert_eq!(input_slot_counts.get(&mixed.id).copied(), Some(3));
}

#[test]
fn display_label_shows_out_modulator_metadata() {
    let patch = parse(
        r#"
            (def signal (in 1 @name pitch))
            (out signal 2 @name slow @modulator 1)
            "#,
    );
    let output = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();

    assert_eq!(node_display_label(output), "out 2 @modulator 1");
}

#[test]
fn writeback_out_text_edit_preserves_and_updates_modulator_metadata() {
    let source = r#"
        (def signal (in 1 @name pitch))
        (out signal 2 @name slow @modulator 1)
    "#;
    let patch = parse(source);
    let output = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", output, node_display_label(output));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &output.id))
        .unwrap()
        .text = "out 3 @modulator 2".to_string();

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(out signal 3 @modulator 2 @name slow)"),
        "edited out node should preserve @name and keep the edited @modulator metadata:\n{emitted}"
    );
}

#[test]
fn writeback_out_text_edit_keeps_original_attributes_when_not_retyped() {
    let source = r#"
        (def signal (in 1 @name pitch))
        (out signal 2 @name slow @modulator 1)
    "#;
    let patch = parse(source);
    let output = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let mut state = PatcherInteractionState::default();
    ensure_source_node_edit(&mut state, "root", output, node_display_label(output));
    state
        .edit_state
        .nodes
        .get_mut(&node_edit_key("root", &output.id))
        .unwrap()
        .text = "out 3".to_string();

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(out signal 3 @name slow @modulator 1)"),
        "channel-only edits should keep original out attributes:\n{emitted}"
    );
}

#[test]
fn writeback_created_modulator_out_can_consume_created_chain() {
    let source = r#"
        (def clock (in 5 @name clock))
        (out 0 1 @name audio)
    "#;
    let mut state = PatcherInteractionState::default();
    let multiply = allocate_created_text_node(&mut state, "root", "* 16");
    let wrapped = allocate_created_text_node(&mut state, "root", "wrap 0 1");
    let triangle = allocate_created_text_node(&mut state, "root", "triangle .0003");
    let shaped = allocate_created_text_node(&mut state, "root", "pow 3");
    let out = allocate_created_text_node(&mut state, "root", "out 2 @name m @modulator 1");

    connect_output_to_input(&mut state, "root", "clock", &multiply, 0);
    connect_output_to_input(&mut state, "root", &multiply, &wrapped, 0);
    connect_output_to_input(&mut state, "root", &wrapped, &triangle, 0);
    connect_output_to_input(&mut state, "root", &triangle, &shaped, 0);
    connect_output_to_input(&mut state, "root", &shaped, &out, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("@modulator 1"),
        "created modulation output should retain @modulator metadata:\n{emitted}"
    );
    assert!(
        emitted.contains("(out "),
        "created modulation output should be emitted:\n{emitted}"
    );
    assert!(
        emitted.contains(" 2 "),
        "created modulation output should keep channel 2:\n{emitted}"
    );
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
    for name in ["mod1", "mod2", "mod3", "mod4"] {
        assert!(
            !patch.nodes.iter().any(|node| node.id == name),
            "boilerplate modulator input {name} should be hidden"
        );
    }
    assert!(patch.nodes.iter().any(|node| node.op == "mod"));
}

#[test]
fn instrument_signature_modulator_inputs_after_clock_are_hidden_boilerplate() {
    let patch = parse(
        r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (def clock (in 5 @name clock))
            (def mod1 (in 6 @name mod1 @modulator 1))
            (def mod2 (in 7 @name mod2 @modulator 2))
            (def mod3 (in 8 @name mod3 @modulator 3))
            (def mod4 (in 9 @name mod4 @modulator 4))
            (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
            (out (* gate (mod gain)) 1 @name audio)
            "#,
    );

    for name in ["gate", "pitch", "velocity", "trigger", "clock"] {
        assert!(
            patch.nodes.iter().any(|node| node.id == name),
            "missing visible instrument input {name}"
        );
    }
    for name in ["mod1", "mod2", "mod3", "mod4"] {
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
fn writeback_recognizes_clocked_host_modulator_inputs_before_created_mod_output() {
    let source = r#"
        (def gate (in 1 @name gate))
        (def pitch (in 2 @name pitch))
        (def velocity (in 3 @name velocity))
        (def trigger (in 4 @name trigger))
        (def clock (in 5 @name clock))
        (def mod1 (in 6 @name mod1 @modulator 1))
        (def mod2 (in 7 @name mod2 @modulator 2))
        (def mod3 (in 8 @name mod3 @modulator 3))
        (def mod4 (in 9 @name mod4 @modulator 4))
        (param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
        (out (* gate (mod gain)) 1 @name audio)
    "#;
    let mut state = PatcherInteractionState::default();
    let multiply = allocate_created_text_node(&mut state, "root", "* 16");
    let wrapped = allocate_created_text_node(&mut state, "root", "wrap 0 1");
    let triangle = allocate_created_text_node(&mut state, "root", "triangle .003");
    let shaped = allocate_created_text_node(&mut state, "root", "pow 3");
    let out = allocate_created_text_node(&mut state, "root", "out 2 @name mod1 @modulator 1");

    connect_output_to_input(&mut state, "root", "clock", &multiply, 0);
    connect_output_to_input(&mut state, "root", &multiply, &wrapped, 0);
    connect_output_to_input(&mut state, "root", &wrapped, &triangle, 0);
    connect_output_to_input(&mut state, "root", &triangle, &shaped, 0);
    connect_output_to_input(&mut state, "root", &shaped, &out, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(def mod1 (in 6 @name mod1 @modulator 1))"),
        "existing clocked host modulator inputs should be recognized, not reinserted:\n{emitted}"
    );
    assert!(
        emitted.contains("@name mod1 @modulator 1"),
        "created modulation output should keep requested name and slot:\n{emitted}"
    );
}

#[test]
fn effect_signature_modulator_inputs_are_hidden_boilerplate() {
    let root_patch = parse_patch_source(
        r#"
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mod1 (in 3 @name mod1 @modulator 1))
        (def mod2 (in 4 @name mod2 @modulator 2))
        (def mod3 (in 5 @name mod3 @modulator 3))
        (def mod4 (in 6 @name mod4 @modulator 4))
        (param mix @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
        (out (* input_l (mod mix)) 1)
        (out (* input_r (mod mix)) 2)
        "#,
        PatcherIntent::Effect,
    )
    .unwrap();

    for name in ["input_l", "input_r"] {
        assert!(
            root_patch.nodes.iter().any(|node| node.id == name),
            "missing visible effect audio input {name}"
        );
    }
    for name in ["mod1", "mod2", "mod3", "mod4"] {
        assert!(
            !root_patch.nodes.iter().any(|node| node.id == name),
            "effect host modulator input {name} should be hidden"
        );
    }
}

#[test]
fn effect_writeback_created_modulatable_param_inserts_host_modulator_inputs() {
    let source = r#"
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mix-amt (param mix @min 0 @max 1 @default 0.5))
        (out (* input_l mix-amt) 1 @name Left)
        (out (* input_r mix-amt) 2 @name Right)
    "#;
    let mut state = PatcherInteractionState::default();
    allocate_created_text_node(
        &mut state,
        "root",
        "param tone @default 0.5 @min 0 @max 1 @mod true @mod-mode additive",
    );

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();

    let input_r_index = emitted
        .find("(def input_r (in 2 @name Right))")
        .expect("right input should remain present");
    let mod1_index = emitted
        .find("(def mod1 (in 3 @name mod1 @modulator 1))")
        .expect("effect mod1 input should be inserted at channel 3");
    let mod4_index = emitted
        .find("(def mod4 (in 6 @name mod4 @modulator 4))")
        .expect("effect mod4 input should be inserted at channel 6");
    let param_index = emitted
        .find("(param tone @default 0.5 @min 0.0 @max 1.0 @mod true @mod-mode additive)")
        .expect("created modulatable param should be emitted");
    assert!(input_r_index < mod1_index, "{emitted}");
    assert!(mod1_index < mod4_index, "{emitted}");
    assert!(mod4_index < param_index, "{emitted}");
    compile_patch_source_with_dgenlisp(&emitted).unwrap();
}

#[test]
fn effect_writeback_created_modulatable_param_can_feed_existing_output() {
    let source = r#"
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mix-amt (param mix @min 0 @max 1 @default 0.5))
        (def processed_l input_l)
        (def processed_r input_r)
        (out (mix input_l processed_l mix-amt) 1 @name Left)
        (out (mix input_r processed_r mix-amt) 2 @name Right)
    "#;
    let patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let input_l = patch
        .nodes
        .iter()
        .find(|node| node.id == "input_l")
        .unwrap();
    let mix_amt = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Param && node.label.contains("param mix "))
        .unwrap();
    let left_out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out && node.id == "Left")
        .unwrap();
    let original_left_signal = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == left_out.id && connection.to_input == 0)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(original_left_signal),
        ));

    let mix = allocate_created_text_node(&mut state, "root", "mix");
    let unity = allocate_created_text_node(&mut state, "root", "* 1");
    let other = allocate_created_text_node(
        &mut state,
        "root",
        "param other @mod true @mod-mode additive @min 0 @max 1",
    );
    let other_mod = allocate_created_text_node(&mut state, "root", "mod");
    let mul = allocate_created_text_node(&mut state, "root", "*");

    connect_output_to_input(&mut state, "root", &input_l.id, &mix, 0);
    connect_output_to_input(&mut state, "root", &input_l.id, &mix, 1);
    connect_output_to_input(&mut state, "root", &mix_amt.id, &mix, 2);
    connect_output_to_input(&mut state, "root", &mix, &unity, 0);
    connect_output_to_input(&mut state, "root", &other, &other_mod, 0);
    connect_output_to_input(&mut state, "root", &unity, &mul, 0);
    connect_output_to_input(&mut state, "root", &other_mod, &mul, 1);
    connect_output_to_input(&mut state, "root", &mul, &left_out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();

    assert!(
        emitted.contains("(param other @mod true @mod-mode additive @min 0.0 @max 1.0)"),
        "created modulatable param should be emitted:\n{emitted}"
    );
    assert!(
        emitted.contains("(mod other)"),
        "created chain should read the modulated param:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted).unwrap();
}

#[test]
fn effect_writeback_created_modulated_delay_emits_param_before_mod_use() {
    let source = r#"
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mix-amt (param mix @min 0 @max 1 @default 0.5))
        (def processed_l input_l)
        (def processed_r input_r)
        (out (mix input_l processed_l mix-amt) 1 @name Left)
        (out (mix input_r processed_r mix-amt) 2 @name Right)
    "#;
    let patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let input_l = patch
        .nodes
        .iter()
        .find(|node| node.id == "input_l")
        .unwrap();
    let left_out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out && node.id == "Left")
        .unwrap();
    let original_left_signal = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == left_out.id && connection.to_input == 0)
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(original_left_signal),
        ));

    let delaytime = allocate_created_text_node(
        &mut state,
        "root",
        "param delaytime @min 50 @max 5000 @default 50 @mod true @mod-mode additive",
    );
    let delaytime_mod = allocate_created_text_node(&mut state, "root", "mod");
    let delay = allocate_created_text_node(&mut state, "root", "delay");
    connect_output_to_input(&mut state, "root", &input_l.id, &delay, 0);
    connect_output_to_input(&mut state, "root", &delaytime, &delaytime_mod, 0);
    connect_output_to_input(&mut state, "root", &delaytime_mod, &delay, 1);
    connect_output_to_input(&mut state, "root", &delay, &left_out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();
    let param_index = emitted.find("(param delaytime").unwrap();
    let mod_index = emitted.find("(def modulated").unwrap();
    assert!(
        param_index < mod_index,
        "modulated delaytime must be emitted after its parameter definition:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn effect_writeback_reconnecting_modulated_delay_preserves_param_before_mod_use() {
    let source = r#"
        (defmacro gain2 (x) (* x 0.45))
        (defmacro simp2 (input) (def phasor1 (phasor input)) (def mul1 (* phasor1 twopi)) (def cos1 (cos mul1)) cos1)
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mod1 (in 3 @name mod1 @modulator 1))
        (def mod2 (in 4 @name mod2 @modulator 2))
        (def mod3 (in 5 @name mod3 @modulator 3))
        (def mod4 (in 6 @name mod4 @modulator 4))
        (param delaytime @min 50.0 @max 5000.0 @default 50.0 @mod true @mod-mode additive)
        (def modulated1 (mod delaytime))
        (def delay1 (delay __patcher_missing_input__ modulated1))
        (def mix-amt (param mix @min 0.0 @max 1.0 @default 0.5))
        (def processed_l input_l)
        (def processed_r input_r)
        (out (mix input_l processed_l mix-amt) 1 @name Left)
        (out (mix input_r processed_r mix-amt) 2 @name Right)
    "#;
    let patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let mut state = PatcherInteractionState::default();
    connect_output_to_input(&mut state, "root", "input_l", "delay1", 0);
    let processed_l_to_left_mix = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == "mix#0" && connection.to_input == 1)
        .unwrap();
    set_connection_segment_edit(
        &mut state,
        "root",
        processed_l_to_left_mix,
        processed_l_to_left_mix.segment,
    );
    state
        .edit_state
        .connections
        .get_mut(&connection_edit_key(
            "root",
            &source_connection_id(processed_l_to_left_mix),
        ))
        .unwrap()
        .from = OutputPortRef {
        node_id: "delay1".to_string(),
        output_index: 0,
    };

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();
    let input_l_index = emitted.find("(def input_l").unwrap();
    let param_index = emitted.find("(param delaytime").unwrap();
    let mod_index = emitted.find("(def modulated1").unwrap();
    assert!(
        input_l_index < param_index && param_index < mod_index,
        "source-backed input, parameter, and mod use must keep dependency order:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn effect_writeback_created_modulatable_param_wrapping_inline_source_output_compiles() {
    let source = r#"
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mix-amt (param mix @min 0 @max 1 @default 0.5))
        (def processed_l input_l)
        (def processed_r input_r)
        (out (mix input_l processed_l mix-amt) 1 @name Left)
        (out (mix input_r processed_r mix-amt) 2 @name Right)
    "#;
    let patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let left_out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out && node.id == "Left")
        .unwrap();
    let original_left_signal = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == left_out.id && connection.to_input == 0)
        .unwrap();
    let inline_left_mix = original_left_signal.from_node.clone();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(original_left_signal),
        ));

    let amount = allocate_created_text_node(
        &mut state,
        "root",
        "param am @min 0 @max 1 @mod true @mod-mode additive",
    );
    let amount_mod = allocate_created_text_node(&mut state, "root", "mod");
    let mul = allocate_created_text_node(&mut state, "root", "*");

    connect_output_to_input(&mut state, "root", &inline_left_mix, &mul, 0);
    connect_output_to_input(&mut state, "root", &amount, &amount_mod, 0);
    connect_output_to_input(&mut state, "root", &amount_mod, &mul, 1);
    connect_output_to_input(&mut state, "root", &mul, &left_out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();

    assert!(
        emitted.contains("(mix input_l processed_l mix-amt)"),
        "test must preserve the inline source expression that depends on processed_l:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn effect_writeback_created_macro_instance_after_generated_chain_compiles() {
    let source = r#"
        (defmacro tanh-saturator (input drive bias makeup mix_amt)
          (def biased (+ input bias))
          (def driven (* biased drive))
          (def wet (tanh driven))
          (def scaled (* wet makeup))
          (mix input scaled mix_amt))
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mix-amt (param mix @min 0 @max 1 @default 0.5))
        (def processed_l input_l)
        (def processed_r input_r)
        (out (mix input_l processed_l mix-amt) 1 @name Left)
        (out (mix input_r processed_r mix-amt) 2 @name Right)
    "#;
    let patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let left_out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out && node.id == "Left")
        .unwrap();
    let original_left_signal = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == left_out.id && connection.to_input == 0)
        .unwrap();
    let inline_left_mix = original_left_signal.from_node.clone();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(original_left_signal),
        ));

    let amount = allocate_created_text_node(
        &mut state,
        "root",
        "param xyz @min 0 @max 1 @mod true @mod-mode additive",
    );
    let amount_mod = allocate_created_text_node(&mut state, "root", "mod");
    let mul = allocate_created_text_node(&mut state, "root", "*");
    let saturator = allocate_created_text_node(&mut state, "root", "tanh-saturator 2 0.5 2 0.3");

    connect_output_to_input(&mut state, "root", &inline_left_mix, &mul, 0);
    connect_output_to_input(&mut state, "root", &amount, &amount_mod, 0);
    connect_output_to_input(&mut state, "root", &amount_mod, &mul, 1);
    connect_output_to_input(&mut state, "root", &mul, &saturator, 0);
    connect_output_to_input(&mut state, "root", &saturator, &left_out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();

    assert!(
        emitted.contains("(def mul1 (* (mix input_l processed_l mix-amt) modulated1))"),
        "test must generate the upstream multiply from the inline output expression:\n{emitted}"
    );
    assert!(
        emitted.contains("(def tanh-saturator1 (tanh-saturator mul1 2.0 0.5 2.0 0.3))"),
        "test must generate a macro call that consumes the generated multiply:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn effect_writeback_created_macro_instance_consumes_stereo_generated_inline_outputs_compiles() {
    let source = r#"
        (defmacro stereo-fold (left right blend)
          (mix left right blend))
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mix-amt (param mix @min 0 @max 1 @default 0.5))
        (def processed_l input_l)
        (def processed_r input_r)
        (out (mix input_l processed_l mix-amt) 1 @name Left)
        (out (mix input_r processed_r mix-amt) 2 @name Right)
    "#;
    let patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let left_out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out && node.id == "Left")
        .unwrap();
    let right_out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out && node.id == "Right")
        .unwrap();
    let original_left_signal = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == left_out.id && connection.to_input == 0)
        .unwrap();
    let original_right_signal = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == right_out.id && connection.to_input == 0)
        .unwrap();
    let inline_left_mix = original_left_signal.from_node.clone();
    let inline_right_mix = original_right_signal.from_node.clone();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(original_left_signal),
        ));

    let amount = allocate_created_text_node(
        &mut state,
        "root",
        "param depth @min 0 @max 1 @mod true @mod-mode additive",
    );
    let amount_mod = allocate_created_text_node(&mut state, "root", "mod");
    let left_mul = allocate_created_text_node(&mut state, "root", "*");
    let right_mul = allocate_created_text_node(&mut state, "root", "*");
    let blend = allocate_created_text_node(&mut state, "root", "0.25");
    let fold = allocate_created_text_node(&mut state, "root", "stereo-fold");

    connect_output_to_input(&mut state, "root", &inline_left_mix, &left_mul, 0);
    connect_output_to_input(&mut state, "root", &amount_mod, &left_mul, 1);
    connect_output_to_input(&mut state, "root", &inline_right_mix, &right_mul, 0);
    connect_output_to_input(&mut state, "root", &amount_mod, &right_mul, 1);
    connect_output_to_input(&mut state, "root", &amount, &amount_mod, 0);
    connect_output_to_input(&mut state, "root", &left_mul, &fold, 0);
    connect_output_to_input(&mut state, "root", &right_mul, &fold, 1);
    connect_output_to_input(&mut state, "root", &blend, &fold, 2);
    connect_output_to_input(&mut state, "root", &fold, &left_out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();

    assert!(
        emitted.contains("(mix input_l processed_l mix-amt)")
            && emitted.contains("(mix input_r processed_r mix-amt)"),
        "test must preserve both inline source expressions:\n{emitted}"
    );
    assert!(
        emitted.contains("(stereo-fold"),
        "test must generate a macro call consuming both generated branches:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn effect_writeback_created_multi_output_macro_replaces_both_inline_outputs_compiles() {
    let source = r#"
        (defmacro split-saturator (input amount)
          (def wet (tanh (* input amount)))
          (tuple wet (* wet 0.5)))
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mix-amt (param mix @min 0 @max 1 @default 0.5))
        (def processed_l input_l)
        (def processed_r input_r)
        (out (mix input_l processed_l mix-amt) 1 @name Left)
        (out (mix input_r processed_r mix-amt) 2 @name Right)
    "#;
    let patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let left_out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out && node.id == "Left")
        .unwrap();
    let right_out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out && node.id == "Right")
        .unwrap();
    let original_left_signal = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == left_out.id && connection.to_input == 0)
        .unwrap();
    let original_right_signal = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == right_out.id && connection.to_input == 0)
        .unwrap();
    let inline_left_mix = original_left_signal.from_node.clone();

    let mut state = PatcherInteractionState::default();
    for connection in [original_left_signal, original_right_signal] {
        state
            .edit_state
            .deleted_connections
            .insert(connection_edit_key(
                "root",
                &source_connection_id(connection),
            ));
    }

    let amount = allocate_created_text_node(
        &mut state,
        "root",
        "param drive @min 0 @max 4 @default 1 @mod true @mod-mode additive",
    );
    let amount_mod = allocate_created_text_node(&mut state, "root", "mod");
    let mul = allocate_created_text_node(&mut state, "root", "*");
    let split = allocate_created_text_node(&mut state, "root", "split-saturator");

    connect_output_to_input(&mut state, "root", &inline_left_mix, &mul, 0);
    connect_output_to_input(&mut state, "root", &amount, &amount_mod, 0);
    connect_output_to_input(&mut state, "root", &amount_mod, &mul, 1);
    connect_output_to_input(&mut state, "root", &mul, &split, 0);
    connect_output_to_input(&mut state, "root", &amount_mod, &split, 1);
    connect_output_to_input(&mut state, "root", &split, &left_out.id, 0);
    allocate_created_connection(
        &mut state,
        "root",
        OutputPortRef {
            node_id: split,
            output_index: 1,
        },
        InputPortRef {
            node_id: right_out.id.clone(),
            input_index: 0,
        },
    );

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();

    assert!(
        emitted.contains("(def (split-saturator"),
        "multi-output created macro instance should emit a destructuring def:\n{emitted}"
    );
    assert!(
        emitted.contains("(out split-saturator"),
        "both effect outs should reference generated macro outputs:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn effect_writeback_deep_generated_chain_before_created_macro_instance_compiles() {
    let source = r#"
        (defmacro saturate-stage (input amount)
          (tanh (* input amount)))
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mix-amt (param mix @min 0 @max 1 @default 0.5))
        (def processed_l input_l)
        (def processed_r input_r)
        (out (mix input_l processed_l mix-amt) 1 @name Left)
        (out (mix input_r processed_r mix-amt) 2 @name Right)
    "#;
    let patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let left_out = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out && node.id == "Left")
        .unwrap();
    let original_left_signal = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == left_out.id && connection.to_input == 0)
        .unwrap();
    let inline_left_mix = original_left_signal.from_node.clone();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(original_left_signal),
        ));

    let amount = allocate_created_text_node(
        &mut state,
        "root",
        "param crush @min 0 @max 1 @mod true @mod-mode additive",
    );
    let amount_mod = allocate_created_text_node(&mut state, "root", "mod");
    let pre_gain = allocate_created_text_node(&mut state, "root", "*");
    let bias = allocate_created_text_node(&mut state, "root", "+ 0.25");
    let clip = allocate_created_text_node(&mut state, "root", "saturate-stage 3");

    connect_output_to_input(&mut state, "root", &inline_left_mix, &pre_gain, 0);
    connect_output_to_input(&mut state, "root", &amount, &amount_mod, 0);
    connect_output_to_input(&mut state, "root", &amount_mod, &pre_gain, 1);
    connect_output_to_input(&mut state, "root", &pre_gain, &bias, 0);
    connect_output_to_input(&mut state, "root", &bias, &clip, 0);
    connect_output_to_input(&mut state, "root", &clip, &left_out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();

    assert!(
        emitted.contains("(saturate-stage"),
        "created macro stage should survive the generated chain:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn effect_writeback_new_macro_definition_consumes_generated_inline_chain_compiles() {
    let source = r#"
        (def input_l (in 1 @name Left))
        (def input_r (in 2 @name Right))
        (def mix-amt (param mix @min 0 @max 1 @default 0.5))
        (def processed_l input_l)
        (def processed_r input_r)
        (out (mix input_l processed_l mix-amt) 1 @name Left)
        (out (mix input_r processed_r mix-amt) 2 @name Right)
    "#;
    let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
    let left_out = root_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out && node.id == "Left")
        .unwrap();
    let original_left_signal = root_patch
        .connections
        .iter()
        .find(|connection| connection.to_node == left_out.id && connection.to_input == 0)
        .unwrap();
    let inline_left_mix = original_left_signal.from_node.clone();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(original_left_signal),
        ));

    let macro_instance = allocate_created_text_node(&mut state, "root", "defmacro *clipper*");
    assert!(promote_created_macro_definition(
        &root_patch,
        &mut state,
        "root",
        &macro_instance,
    ));
    let macro_source = r#"(defmacro clipper (input amount)
  (def driven (* input amount))
  (tanh driven))"#;
    state
        .edit_state
        .created_macros
        .get_mut("clipper")
        .unwrap()
        .source = Some(macro_source.to_string());

    let amount = allocate_created_text_node(
        &mut state,
        "root",
        "param drive @min 0 @max 8 @default 1 @mod true @mod-mode additive",
    );
    let amount_mod = allocate_created_text_node(&mut state, "root", "mod");
    let pre_gain = allocate_created_text_node(&mut state, "root", "*");

    connect_output_to_input(&mut state, "root", &inline_left_mix, &pre_gain, 0);
    connect_output_to_input(&mut state, "root", &amount, &amount_mod, 0);
    connect_output_to_input(&mut state, "root", &amount_mod, &pre_gain, 1);
    connect_output_to_input(&mut state, "root", &pre_gain, &macro_instance, 0);
    connect_output_to_input(&mut state, "root", &amount_mod, &macro_instance, 1);
    connect_output_to_input(&mut state, "root", &macro_instance, &left_out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Effect, &state).unwrap();

    let macro_index = emitted
        .find("(defmacro clipper")
        .expect("created macro definition should be emitted");
    let call_index = emitted
        .find("(def clipper1 (clipper")
        .expect("created macro call should be emitted");
    assert!(
        macro_index < call_index,
        "created macro definition must precede its generated call:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
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
    let state = PatcherInteractionState::default();
    let ordered_nodes = ordered_patch_nodes(&patch, &state, "root");
    let hit = hit_patcher_node(
        &patch,
        &ordered_nodes,
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
    let state = PatcherInteractionState::default();
    let ordered_nodes = ordered_patch_nodes(&patch, &state, "root");

    let hit = hit_patcher_output_port(
        &patch,
        &ordered_nodes,
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
    let state = PatcherInteractionState::default();
    let ordered_nodes = ordered_patch_nodes(&patch, &state, "root");

    let inside_rendered_circle = hit_patcher_output_port(
        &patch,
        &ordered_nodes,
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
        &ordered_nodes,
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
fn segmented_horizontal_segment_hit_uses_tight_cable_radius() {
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
    let midpoint_col = (start.0 + end.0) * 0.5;

    assert_eq!(
        hit_patcher_segmented_cable_horizontal_segment(
            &patch,
            rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            midpoint_col,
            rendered_segment_row + CABLE_HIT_RADIUS_CELLS * zoom + 0.01,
        ),
        None
    );
}

#[test]
fn node_drag_wins_over_segmented_cable_hit_inside_node_body() {
    let path = temp_patcher_source_path("patcher-node-wins-over-cable");
    fs::write(
        &path,
        "(def pitch (in 1 @name pitch))\n(out pitch 1 @name audio)",
    )
    .expect("write source");
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let (_, root_patch) = load_patch_from_props(&node.props).expect("load patch");
    let pan = PatcherPanState::default();
    set_patcher_pan_state(key, pan.clone());
    let mut state = PatcherInteractionState::default();
    let target_node_id = root_patch
        .connections
        .first()
        .expect("source connection")
        .to_node
        .clone();
    let target_node = root_patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == target_node_id)
        .unwrap();
    set_node_edit_position(
        &mut state,
        "root",
        target_node,
        (target_node.position.0 + 12.0, target_node.position.1),
        node_display_label(target_node),
    );
    let edited_patch = patch_with_interaction_state(root_patch.clone(), &state, "root");
    let node_rects = patch_node_rects(&edited_patch, node.rect, &pan);
    let input_indices = patch_input_indices(&edited_patch);
    let input_slot_counts = patch_input_slot_counts(&edited_patch, &input_indices);
    let output_counts = patch_output_counts(&edited_patch);
    let connection = edited_patch.connections.first().expect("source connection");
    let (_start, end) = connection_endpoints(
        connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    let target_rect = node_rects.get(&connection.to_node).copied().unwrap();
    let zoom = patcher_zoom(&pan);
    let origin = patcher_origin(node.rect, &pan);
    let segment_screen_row = target_rect.row + target_rect.height * 0.5;
    let segment_row = (segment_screen_row - origin.1) / zoom;
    set_connection_segment_edit(
        &mut state,
        "root",
        connection,
        Some(CableSegmentInfo {
            is_segmented: true,
            segment_row,
        }),
    );
    set_patcher_interaction_state(key, state);
    let hit_col = end.0 - SEGMENTED_CABLE_CORNER_RADIUS_CELLS * zoom - 0.01;

    assert_eq!(
        hit_patcher_node(
            &edited_patch,
            &ordered_patch_nodes(&edited_patch, &get_patcher_interaction_state(key), "root"),
            node.rect,
            &pan,
            hit_col,
            segment_screen_row,
        )
        .as_deref(),
        Some(connection.to_node.as_str())
    );
    assert_eq!(
        hit_patcher_segmented_cable_horizontal_segment(
            &patch_with_interaction_state(
                root_patch.clone(),
                &get_patcher_interaction_state(key),
                "root",
            ),
            node.rect,
            &pan,
            &input_indices,
            &input_slot_counts,
            &output_counts,
            hit_col,
            segment_screen_row,
        )
        .as_deref(),
        Some(source_connection_id(connection).as_str())
    );

    handle_patcher_pointer_down(
        &node,
        hit_col,
        segment_screen_row,
        KeyModifiers::empty(),
        10.0,
        20.0,
    );

    let state = get_patcher_interaction_state(key);
    assert!(
        matches!(state.drag, Some(PatcherDragState::Nodes { .. })),
        "{:?}",
        state.drag
    );

    let _ = fs::remove_file(path);
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

    let initial_state = get_patcher_interaction_state(key);
    let initial_patch = patch_with_interaction_state(patch.clone(), &initial_state, "root");
    let initially_segmented = initial_patch
        .connections
        .iter()
        .find(|connection| source_connection_id(connection) == selected_cable)
        .and_then(|connection| connection.segment.as_ref())
        .is_some_and(|segment| segment.is_segmented);

    assert!(
        PATCHER_WIDGET
            .key_event(
                &node,
                WidgetKeyEvent {
                    code: KeyCode::Char('y'),
                    modifiers: KeyModifiers::SUPER,
                },
            )
            .is_some()
    );

    let state = get_patcher_interaction_state(key);
    let patch = patch_with_interaction_state(patch, &state, "root");
    let segment = patch
        .connections
        .iter()
        .find(|connection| source_connection_id(connection) == selected_cable)
        .and_then(|connection| connection.segment)
        .unwrap();
    assert_ne!(segment.is_segmented, initially_segmented);
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
        .find_map(|prim| match inner_prim(prim) {
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
        .find_map(|prim| match inner_prim(prim) {
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
        .find_map(|prim| match inner_prim(prim) {
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
    handle_patcher_pointer_drag(
        &node,
        gate_output.0,
        gate_output.1,
        KeyModifiers::empty(),
        10.0,
        20.0,
    );
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
    let state = PatcherInteractionState::default();
    let ordered_nodes = ordered_patch_nodes(&patch, &state, "root");
    let hit = hit_patcher_node(
        &patch,
        &ordered_nodes,
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
    handle_patcher_pointer_drag(
        &node,
        start.0 + 4.0,
        start.1 + 2.0,
        KeyModifiers::empty(),
        10.0,
        20.0,
    );

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
fn patcher_alignment_snaps_x_edge_within_threshold() {
    let mut patch = parse("(def a (in 1 @name a))\n(def b (phasor a))");
    set_patch_node_position(&mut patch, "a", (10.0, 10.0));
    set_patch_node_position(&mut patch, "b", (30.0, 20.0));
    let mut snap = AlignmentSnapState::default();
    let selected = HashSet::from(["b".to_string()]);

    let aligned = align_dragged_primary_position(
        &patch,
        "b",
        &selected,
        (10.4, 20.0),
        &mut snap,
        1.0,
        10.0,
        20.0,
    );

    assert!((aligned.0 - 10.0).abs() < 0.001);
    assert!(snap.snapped_x);
    assert!(
        snap.guides
            .iter()
            .any(|guide| guide.kind == AlignmentGuideKind::Vertical)
    );
}

#[test]
fn patcher_alignment_prefers_nearer_x_candidate_over_patch_order() {
    let mut patch =
        parse("(def far (in 1 @name far))\n(def near (in 1 @name near))\n(def moved (phasor 1))");
    set_patch_node_position(&mut patch, "far", (10.0, 0.0));
    set_patch_node_position(&mut patch, "near", (10.45, 12.0));
    set_patch_node_position(&mut patch, "moved", (30.0, 14.0));
    let mut snap = AlignmentSnapState::default();
    let selected = HashSet::from(["moved".to_string()]);

    let aligned = align_dragged_primary_position(
        &patch,
        "moved",
        &selected,
        (10.3, 14.0),
        &mut snap,
        1.0,
        10.0,
        20.0,
    );

    assert!((aligned.0 - 10.45).abs() < 0.001, "{aligned:?}");
}

#[test]
fn patcher_alignment_snaps_y_edge_within_threshold() {
    let mut patch = parse("(def a (in 1 @name a))\n(def b (phasor a))");
    set_patch_node_position(&mut patch, "a", (10.0, 10.0));
    set_patch_node_position(&mut patch, "b", (25.0, 30.0));
    let mut snap = AlignmentSnapState::default();
    let selected = HashSet::from(["b".to_string()]);

    let aligned = align_dragged_primary_position(
        &patch,
        "b",
        &selected,
        (25.0, 10.2),
        &mut snap,
        1.0,
        10.0,
        20.0,
    );

    assert!((aligned.1 - 10.0).abs() < 0.001);
    assert!(snap.snapped_y);
    assert!(
        snap.guides
            .iter()
            .any(|guide| guide.kind == AlignmentGuideKind::Horizontal)
    );
}

#[test]
fn patcher_alignment_does_not_snap_outside_enter_threshold() {
    let mut patch = parse("(def a (in 1 @name a))\n(def b (phasor a))");
    set_patch_node_position(&mut patch, "a", (10.0, 10.0));
    set_patch_node_position(&mut patch, "b", (30.0, 20.0));
    let mut snap = AlignmentSnapState::default();
    let selected = HashSet::from(["b".to_string()]);

    let raw = (12.5, 20.0);
    let aligned =
        align_dragged_primary_position(&patch, "b", &selected, raw, &mut snap, 1.0, 10.0, 20.0);

    assert_eq!(aligned, raw);
    assert!(!snap.snapped_x);
    assert!(snap.guides.is_empty());
}

#[test]
fn patcher_alignment_hysteresis_keeps_existing_x_snap_until_escape_threshold() {
    let mut patch = parse("(def a (in 1 @name a))\n(def b (phasor a))");
    set_patch_node_position(&mut patch, "a", (10.0, 10.0));
    set_patch_node_position(&mut patch, "b", (30.0, 20.0));
    let mut snap = AlignmentSnapState {
        snapped_x: true,
        ..Default::default()
    };
    let selected = HashSet::from(["b".to_string()]);

    let aligned = align_dragged_primary_position(
        &patch,
        "b",
        &selected,
        (10.7, 20.0),
        &mut snap,
        1.0,
        10.0,
        20.0,
    );

    assert!((aligned.0 - 10.0).abs() < 0.001);
    assert!(snap.snapped_x);
}

#[test]
fn patcher_alignment_snaps_dragged_output_to_other_input_x() {
    let mut patch = parse("(def a (in 1 @name a))\n(def b (phasor a))");
    set_patch_node_position(&mut patch, "a", (0.0, 10.0));
    set_patch_node_position(&mut patch, "b", (12.77, 20.0));
    {
        let source = patch.nodes.iter_mut().find(|node| node.id == "a").unwrap();
        source.width = Some(12.0);
        source.outputs = vec!["left".to_string(), "right".to_string()];
    }
    patch
        .nodes
        .iter_mut()
        .find(|node| node.id == "b")
        .unwrap()
        .width = Some(20.0);
    let mut snap = AlignmentSnapState::default();
    let selected = HashSet::from(["a".to_string()]);

    let aligned = align_dragged_primary_position(
        &patch,
        "a",
        &selected,
        (0.0, 10.0),
        &mut snap,
        1.0,
        10.0,
        20.0,
    );

    assert!((aligned.0 - 0.5).abs() < 0.001, "{aligned:?}");
    assert!(snap.snapped_x);
}

#[test]
fn patcher_alignment_group_drag_applies_primary_adjustment_to_all_selected_nodes() {
    let path = temp_patcher_source_path("patcher-alignment-group-drag");
    fs::write(
        &path,
        "(def a (in 1 @name a))\n(def b (phasor a))\n(def c (sin b))",
    )
    .unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let root_patch = load_patch_from_props(&node.props).unwrap().1;
    let mut state = PatcherInteractionState::default();
    for (node_id, position) in [
        ("a", (10.0, 10.0)),
        ("b", (30.0, 35.0)),
        ("c", (40.0, 45.0)),
    ] {
        let patch_node = root_patch
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .unwrap();
        set_node_edit_position(
            &mut state,
            "root",
            patch_node,
            position,
            node_display_label(patch_node),
        );
    }
    state.selected_nodes = HashSet::from(["b".to_string(), "c".to_string()]);
    set_patcher_interaction_state(key, state);
    let edited = patch_with_interaction_state(
        root_patch.clone(),
        &get_patcher_interaction_state(key),
        "root",
    );
    let pan = get_patcher_pan_state(key);
    let b_rect = *patch_node_rects(&edited, node.rect, &pan).get("b").unwrap();
    let start = (
        b_rect.col + b_rect.width * 0.5,
        b_rect.row + b_rect.height * 0.5,
    );
    let delta_x = (10.4 - 30.0) * patcher_zoom(&pan);

    handle_patcher_pointer_down(&node, start.0, start.1, KeyModifiers::empty(), 10.0, 20.0);
    handle_patcher_pointer_drag(
        &node,
        start.0 + delta_x,
        start.1,
        KeyModifiers::empty(),
        10.0,
        20.0,
    );

    let moved =
        patch_with_interaction_state(root_patch, &get_patcher_interaction_state(key), "root");
    assert!((patch_node_position(&moved, "b").0 - 10.0).abs() < 0.001);
    assert!((patch_node_position(&moved, "c").0 - 20.0).abs() < 0.001);

    let _ = fs::remove_file(path);
}

#[test]
fn patcher_alignment_super_drag_bypasses_snapping() {
    let path = temp_patcher_source_path("patcher-alignment-super-bypass");
    fs::write(&path, "(def a (in 1 @name a))\n(def b (phasor a))").unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let root_patch = load_patch_from_props(&node.props).unwrap().1;
    let mut state = PatcherInteractionState::default();
    for (node_id, position) in [("a", (10.0, 10.0)), ("b", (30.0, 35.0))] {
        let patch_node = root_patch
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .unwrap();
        set_node_edit_position(
            &mut state,
            "root",
            patch_node,
            position,
            node_display_label(patch_node),
        );
    }
    set_patcher_interaction_state(key, state);
    let edited = patch_with_interaction_state(
        root_patch.clone(),
        &get_patcher_interaction_state(key),
        "root",
    );
    let pan = get_patcher_pan_state(key);
    let b_rect = *patch_node_rects(&edited, node.rect, &pan).get("b").unwrap();
    let start = (
        b_rect.col + b_rect.width * 0.5,
        b_rect.row + b_rect.height * 0.5,
    );
    let delta_x = (10.4 - 30.0) * patcher_zoom(&pan);

    handle_patcher_pointer_down(&node, start.0, start.1, KeyModifiers::empty(), 10.0, 20.0);
    handle_patcher_pointer_drag(
        &node,
        start.0 + delta_x,
        start.1,
        KeyModifiers::SUPER,
        10.0,
        20.0,
    );

    let moved =
        patch_with_interaction_state(root_patch, &get_patcher_interaction_state(key), "root");
    assert!((patch_node_position(&moved, "b").0 - 10.4).abs() < 0.001);
    let drag = get_patcher_interaction_state(key).drag.unwrap();
    if let PatcherDragState::Nodes { alignment, .. } = drag {
        assert!(alignment.guides.is_empty());
    } else {
        panic!("expected node drag");
    }

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
    handle_patcher_pointer_drag(
        &node,
        start.0 + 3.0,
        start.1 + 2.0,
        KeyModifiers::empty(),
        10.0,
        20.0,
    );
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
fn patcher_right_corner_resize_changes_width_without_moving_node() {
    let path = temp_patcher_source_path("patcher-right-resize");
    fs::write(&path, "(def pitch (in 1 @name pitch))\n").unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let root_patch = load_patch_from_props(&node.props).unwrap().1;
    let pitch = root_patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "pitch")
        .unwrap();
    let start_position = pitch.position;
    let pan = get_patcher_pan_state(key);
    let rect = *patch_node_rects(&root_patch, node.rect, &pan)
        .get("pitch")
        .unwrap();
    let start_width = rect.width / patcher_zoom(&pan);
    let mut state = get_patcher_interaction_state(key);
    state.selected_nodes.insert("pitch".to_string());
    set_patcher_interaction_state(key, state);

    let corner = (rect.col + rect.width, rect.row);
    handle_patcher_pointer_down(&node, corner.0, corner.1, KeyModifiers::empty(), 10.0, 20.0);
    handle_patcher_pointer_drag(
        &node,
        corner.0 + 3.5,
        corner.1 + 10.0,
        KeyModifiers::empty(),
        10.0,
        20.0,
    );
    assert_eq!(
        handle_patcher_pointer_up(&node, corner.0 + 3.5, corner.1 + 10.0),
        PatcherChangeKind::Layout
    );

    let state = get_patcher_interaction_state(key);
    let edited = patch_with_interaction_state(root_patch, &state, "root");
    let resized = edited
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "pitch")
        .unwrap();
    assert_eq!(resized.position, start_position);
    assert!((resized.width.unwrap() - (start_width + 3.5 / patcher_zoom(&pan))).abs() < 0.001);
}

#[test]
fn patcher_left_corner_resize_keeps_right_edge_anchored() {
    let path = temp_patcher_source_path("patcher-left-resize");
    fs::write(&path, "(def pitch (in 1 @name pitch))\n").unwrap();
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let root_patch = load_patch_from_props(&node.props).unwrap().1;
    let pan = get_patcher_pan_state(key);
    let rect = *patch_node_rects(&root_patch, node.rect, &pan)
        .get("pitch")
        .unwrap();
    let start_width = rect.width / patcher_zoom(&pan);
    let pitch = root_patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "pitch")
        .unwrap();
    let start_right = pitch.position.0 + start_width;
    let mut state = get_patcher_interaction_state(key);
    state.selected_nodes.insert("pitch".to_string());
    set_patcher_interaction_state(key, state);

    let corner = (rect.col, rect.row);
    handle_patcher_pointer_down(&node, corner.0, corner.1, KeyModifiers::empty(), 10.0, 20.0);
    handle_patcher_pointer_drag(
        &node,
        corner.0 - 2.8,
        corner.1 + 12.0,
        KeyModifiers::empty(),
        10.0,
        20.0,
    );

    let state = get_patcher_interaction_state(key);
    let edited = patch_with_interaction_state(root_patch, &state, "root");
    let resized = edited
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == "pitch")
        .unwrap();
    let right = resized.position.0 + resized.width.unwrap();
    assert!((right - start_right).abs() < 0.001);
    assert!((resized.width.unwrap() - (start_width + 2.8 / patcher_zoom(&pan))).abs() < 0.001);
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
            .any(|node| node.kind == NodeKind::In && node_display_label(node).starts_with("in 1")),
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
            width: None,
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
            width: None,
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
        "(defmacro op (input)\n  (def phasor1 (phasor input))\n  (def triangle1 (triangle phasor1))\n  triangle1)\n(def sig (in 1))\n(out sig 1)"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn writeback_replacing_existing_macro_return_by_deleting_return_node_compiles() {
    let source = "(defmacro simp (input) (* input 1))\n(def sig (in 1))\n(def simp1 (simp sig))\n(out simp1 1)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "simp")
        .expect("macro should project");
    let input = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .unwrap();
    let return_node = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .expect("macro should expose the visual return expression");
    let out = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .unwrap();
    let return_to_out = macro_patch
        .patch
        .connections
        .iter()
        .find(|connection| connection.from_node == return_node.id && connection.to_node == out.id)
        .unwrap();

    let mut state = PatcherInteractionState {
        active_macro: Some("simp".to_string()),
        ..PatcherInteractionState::default()
    };
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:simp", &return_node.id));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "macro:simp",
            &source_connection_id(return_to_out),
        ));
    let phasor = allocate_created_text_node(&mut state, "macro:simp", "phasor");
    let multiply = allocate_created_text_node(&mut state, "macro:simp", "* twopi");
    let cos = allocate_created_text_node(&mut state, "macro:simp", "cos");
    connect_output_to_input(&mut state, "macro:simp", &input.id, &phasor, 0);
    connect_output_to_input(&mut state, "macro:simp", &phasor, &multiply, 0);
    connect_output_to_input(&mut state, "macro:simp", &multiply, &cos, 0);
    connect_output_to_input(&mut state, "macro:simp", &cos, &out.id, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert_eq!(
        emitted,
        "(defmacro simp (input)\n  (def phasor1 (phasor input))\n  (def mul1 (* phasor1 twopi))\n  (def cos1 (cos mul1))\n  cos1)\n(def sig (in 1))\n(def simp1 (simp sig))\n(out simp1 1)"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn writeback_replacing_created_macro_scaffold_with_created_in_and_out_compiles() {
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
    let input = macro_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::In)
        .expect("default macro should expose an input node");
    let return_node = macro_patch
        .nodes
        .iter()
        .find(|node| node.id == "return")
        .expect("default macro should expose a return node");
    let out = macro_patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .expect("default macro should expose an output node");

    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:op", &input.id));
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:op", &return_node.id));
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("macro:op", &out.id));

    let pitch = allocate_created_text_node(&mut state, "macro:op", "in 1 @name pitch");
    let harmonicity = allocate_created_text_node(&mut state, "macro:op", "in 2 @name harmonicity");
    let multiply = allocate_created_text_node(&mut state, "macro:op", "*");
    let out1 = allocate_created_text_node(&mut state, "macro:op", "out 1");
    connect_output_to_input(&mut state, "macro:op", &pitch, &multiply, 0);
    connect_output_to_input(&mut state, "macro:op", &harmonicity, &multiply, 1);
    connect_output_to_input(&mut state, "macro:op", &multiply, &out1, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert_eq!(
        emitted,
        "(defmacro op (pitch harmonicity) (* pitch harmonicity))\n(def sig (in 1))\n(out sig 1)"
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
fn writeback_rewires_displaced_nested_call_through_source_consumers() {
    let source = r#"
        (defmacro simp11 (input) input)
        (def pitch (in 1 @name pitch))
        (def env (in 2 @name env))
        (def velocity (in 3 @name velocity))
        (param gain @default 0.5 @min 0.0 @max 1.0 @mod true @mod-mode additive)
        (def phase (phasor pitch))
        (def simp111 (simp11 (* phase env velocity (mod gain))))
        (def simp112 (simp11 simp111))
        (out simp112 1 @name audio)
    "#;
    let root_patch = parse(source);
    let phase = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "phase")
        .unwrap();
    let multiply = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "*#0")
        .unwrap();
    let multiply_to_simp111 = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == multiply.id && connection.to_node == "simp111")
        .unwrap();
    let simp111_to_simp112 = root_patch
        .connections
        .iter()
        .find(|connection| connection.from_node == "simp111" && connection.to_node == "simp112")
        .unwrap();

    let mut state = PatcherInteractionState::default();
    state
        .edit_state
        .deleted_nodes
        .insert(node_edit_key("root", &phase.id));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(multiply_to_simp111),
        ));
    state
        .edit_state
        .deleted_connections
        .insert(connection_edit_key(
            "root",
            &source_connection_id(simp111_to_simp112),
        ));
    connect_output_to_input(&mut state, "root", "pitch", "simp111", 0);
    connect_output_to_input(&mut state, "root", "simp111", &multiply.id, 0);
    connect_output_to_input(&mut state, "root", &multiply.id, "simp112", 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        !emitted.contains("(def phase"),
        "deleted phasor binding should be removed:\n{emitted}"
    );
    assert!(
        emitted.contains("(def simp111 (simp11 pitch))"),
        "pitch should feed the first simp11 call directly:\n{emitted}"
    );
    assert!(
        emitted.contains("(def simp112 (simp11 (* simp111 env velocity (mod gain))))"),
        "the displaced multiply should move to the second simp11 input with simp111 as its first input:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
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
        "(defmacro op (input)\n  (def phasor1 (phasor input))\n  (def triangle1 (triangle phasor1))\n  triangle1)"
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
fn writeback_generated_binding_into_missing_macro_non_first_input_inserts_arg() {
    let source = "(defmacro xop (input) (def phasor1 (phasor input)) (def mul1 (* phasor1)) mul1)\n(def pitch (in 2 @name pitch))\n(def xop1 (xop pitch))\n(out xop1 1 @name audio)";
    let root_patch = parse(source);
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == "xop")
        .unwrap();
    let mul = macro_patch
        .patch
        .nodes
        .iter()
        .find(|node| node.id == "mul1")
        .unwrap();

    let mut state = PatcherInteractionState {
        active_macro: Some("xop".to_string()),
        ..Default::default()
    };
    let rate = allocate_created_text_node(&mut state, "macro:xop", "44100");
    let phasor = allocate_created_text_node(&mut state, "macro:xop", "phasor");
    connect_output_to_input(&mut state, "macro:xop", &rate, &phasor, 0);
    connect_output_to_input(&mut state, "macro:xop", &phasor, &mul.id, 1);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    assert!(
        emitted.contains("(def value1 44100.0)"),
        "created number should materialize as a generated binding:\n{emitted}"
    );
    assert!(
        emitted.contains("(def phasor2 (phasor value1))"),
        "created phasor should reference generated number binding:\n{emitted}"
    );
    assert!(
        emitted.contains("(def mul1 (* phasor1 phasor2))"),
        "generated phasor should be inserted into missing second input:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn writeback_created_binding_depending_on_late_source_input_is_inserted_after_input() {
    let source = r#"
        (def early (phasor 1))
        (def trigger (in 4 @name trigger))
        (out early 1 @name audio)
    "#;
    let root_patch = parse(source);
    let trigger = root_patch
        .nodes
        .iter()
        .find(|node| node.id == "trigger")
        .unwrap();
    let mut state = PatcherInteractionState::default();
    let value = allocate_created_text_node(&mut state, "root", "2");
    let phasor = allocate_created_text_node(&mut state, "root", "phasor");
    connect_output_to_input(&mut state, "root", &value, &phasor, 0);
    connect_output_to_input(&mut state, "root", &trigger.id, &phasor, 1);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    let trigger_idx = emitted.find("(def trigger").unwrap();
    let phasor_idx = emitted.find("(def phasor1").unwrap();
    assert!(
        phasor_idx > trigger_idx,
        "generated phasor must be emitted after source input it references:\n{emitted}"
    );
    assert!(
        emitted.contains("(def phasor1 (phasor value1 trigger))"),
        "generated phasor should still reference trigger by symbol:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
}

#[test]
fn writeback_created_binding_with_inline_late_source_symbol_is_inserted_after_input() {
    let source = r#"
        (def early (phasor 1))
        (def trigger (in 4 @name trigger))
        (out early 1 @name audio)
    "#;
    let mut state = PatcherInteractionState::default();
    let value = allocate_created_text_node(&mut state, "root", "2");
    let phasor = allocate_created_text_node(&mut state, "root", "phasor trigger");
    connect_output_to_input(&mut state, "root", &value, &phasor, 0);

    let emitted = emit_patch_writeback(source, PatcherIntent::Instrument, &state).unwrap();
    let trigger_idx = emitted.find("(def trigger").unwrap();
    let phasor_idx = emitted.find("(def phasor1").unwrap();
    assert!(
        phasor_idx > trigger_idx,
        "generated phasor with inline trigger must be emitted after trigger input:\n{emitted}"
    );
    assert!(
        emitted.contains("(def phasor1 (phasor value1 trigger))"),
        "generated phasor should preserve inline trigger symbol:\n{emitted}"
    );
    compile_patch_source_with_dgenlisp(&emitted)
        .unwrap_or_else(|error| panic!("emitted source should compile:\n{error}\n{emitted}"));
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
        "(defmacro op (input)\n  (def phasor1 (phasor input))\n  (def triangle1 (triangle phasor1))\n  triangle1)"
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
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let port_x = |id: &str, inlet: bool| {
        let node = patch.nodes.iter().find(|node| node.id == id).unwrap();
        let input_count = input_slot_counts.get(&node.id).copied().unwrap_or(0);
        let output_count = output_counts.get(&node.id).copied().unwrap_or(0);
        let (width, _) = node_size_for_ports(node, input_count, output_count);
        let count = if inlet { input_count } else { output_count };
        node.position.0 + port_x_offset(0, count, width)
    };

    // The chain cable should be perfectly vertical: each node's first inlet
    // sits directly under the previous node's outlet.
    for (from, to) in [("a", "b"), ("b", "c")] {
        let outlet = port_x(from, false);
        let inlet = port_x(to, true);
        assert!(
            (outlet - inlet).abs() < 0.01,
            "primary signal chain cable {from}->{to} should be vertical: outlet={outlet} inlet={inlet}"
        );
    }
}

#[test]
fn node_size_uses_autogenerated_width_without_override() {
    let node = parse("(def sig (phasor 440))")
        .nodes
        .into_iter()
        .find(|node| node.id == "sig")
        .unwrap();
    let autogenerated = node_autogenerated_size_for_ports(&node, 1, 1);
    let sized = node_size_for_ports(&node, 1, 1);
    assert_eq!(sized, autogenerated);
}

#[test]
fn node_size_clamps_width_override_to_autogenerated_minimum() {
    let mut node = parse("(def sig (phasor 440))")
        .nodes
        .into_iter()
        .find(|node| node.id == "sig")
        .unwrap();
    let (autogenerated_width, height) = node_autogenerated_size_for_ports(&node, 1, 1);
    node.width = Some(autogenerated_width * 0.5);
    assert_eq!(
        node_size_for_ports(&node, 1, 1),
        (autogenerated_width, height)
    );
    node.width = Some(autogenerated_width + 8.0);
    assert_eq!(
        node_size_for_ports(&node, 1, 1),
        (autogenerated_width + 8.0, height)
    );
}

#[test]
fn node_width_override_affects_rects_ports_content_and_cables() {
    let mut patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let sig = patch
        .nodes
        .iter_mut()
        .find(|node| node.id == "sig")
        .unwrap();
    sig.width = Some(32.0);
    let sig_position = sig.position;
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 100.0,
        height: 40.0,
    };
    let pan = PatcherPanState {
        zoom: 1.0,
        ..Default::default()
    };
    let node_rects = patch_node_rects(&patch, rect, &pan);
    let sig_rect = node_rects.get("sig").unwrap();
    assert!((sig_rect.width - 32.0).abs() < 0.001);
    assert!(patch_content_size(&patch).0 >= sig_position.0 + 32.0);

    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    let connection = patch.connections.first().unwrap();
    let (_start, end) = connection_endpoints(
        connection,
        &node_rects,
        &input_indices,
        &input_slot_counts,
        &output_counts,
    )
    .unwrap();
    let expected_input = port_center(*sig_rect, 0, 1, true);
    assert_eq!(end, expected_input);
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
        .filter(|prim| matches!(inner_prim(prim), MetalPrimitive::ProportionalText(_)))
        .count();
    let rect_count = prims
        .iter()
        .filter(|prim| matches!(inner_prim(prim), MetalPrimitive::Rect(_)))
        .count();
    let rounded_count = prims
        .iter()
        .filter(|prim| matches!(inner_prim(prim), MetalPrimitive::WidgetInstance { .. }))
        .count();
    let cable_count = prims
        .iter()
        .filter(|prim| matches!(inner_prim(prim), MetalPrimitive::PatchCable(_)))
        .count();
    let min_cable_radius = prims
        .iter()
        .filter_map(|prim| match inner_prim(prim) {
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
fn metal_render_vertically_centers_normal_node_text_band() {
    let patch = parse("(def sig (phasor 440))");
    let panel = Rect {
        row: 0.0,
        col: 0.0,
        width: 100.0,
        height: 40.0,
    };
    let viewport = WidgetViewport {
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
    };
    let pan = PatcherPanState::default();
    let node_rects = patch_node_rects(&patch, panel, &pan);
    let sig_rect = node_rects.get("sig").expect("sig node rect");
    let mut prims = Vec::new();
    draw_patch(
        &mut prims,
        &patch,
        panel,
        viewport,
        &pan,
        &PatcherInteractionState::default(),
    );

    let text_row = prims
        .iter()
        .find_map(|prim| match inner_prim(prim) {
            MetalPrimitive::ProportionalText(text) if text.text == "phasor" => Some(text.row),
            _ => None,
        })
        .expect("phasor node text");
    let expected_row = sig_rect.row + (sig_rect.height - DEFAULT_ZOOM) * 0.5;

    assert!(
        (text_row - expected_row).abs() < 0.000_1,
        "node text row should center the renderer's one-cell text band in the node: row={text_row} expected={expected_row}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_groups_node_sublayers_by_z_order() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
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

    let node_chrome_z = prims
        .iter()
        .filter_map(|prim| match inner_prim(prim) {
            MetalPrimitive::WidgetInstance { widget_type, .. } if widget_type == "patcher-node" => {
                Some(effective_z(prim))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let port_z = prims
        .iter()
        .filter_map(|prim| match inner_prim(prim) {
            MetalPrimitive::WidgetInstance { widget_type, .. } if widget_type == "patcher-port" => {
                Some(effective_z(prim))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let text_z = prims
        .iter()
        .filter_map(|prim| match inner_prim(prim) {
            MetalPrimitive::ProportionalText(text)
                if text.text == "in" || text.text == "phasor" =>
            {
                Some(effective_z(prim))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let cable_z = prims
        .iter()
        .filter_map(|prim| match inner_prim(prim) {
            MetalPrimitive::PatchCable(_) => Some(effective_z(prim)),
            _ => None,
        })
        .next()
        .expect("patch cable");

    assert!(node_chrome_z.contains(&0));
    assert!(node_chrome_z.contains(&PATCHER_Z_SLOTS_PER_NODE));
    assert!(port_z.contains(&(PatcherZSlot::Ports as i32)));
    assert!(port_z.contains(&(PATCHER_Z_SLOTS_PER_NODE + PatcherZSlot::Ports as i32)));
    assert!(text_z.contains(&(PatcherZSlot::Text as i32)));
    assert!(text_z.contains(&(PATCHER_Z_SLOTS_PER_NODE + PatcherZSlot::Text as i32)));
    assert!(cable_z > PATCHER_Z_SLOTS_PER_NODE + PatcherZSlot::Text as i32);
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
                inner_prim(prim),
                MetalPrimitive::PatchCable(cable) if cable.color == theme::PATCHER_ERROR()
            )
        })
        .count();
    let handle_shell_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                inner_prim(prim),
                MetalPrimitive::Circle(circle) if circle.color == theme::PATCHER_ERROR()
            )
        })
        .count();

    assert_eq!(selected_cable_count, 1);
    assert_eq!(handle_shell_count, 2);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_emits_alignment_guide_rects_for_node_drag() {
    let patch = parse("(def a (in 1 @name a))\n(def b (phasor a))");
    let state = PatcherInteractionState {
        drag: Some(PatcherDragState::Nodes {
            primary_node_id: "b".to_string(),
            start_col: 0.0,
            start_row: 0.0,
            start_positions: HashMap::new(),
            alignment: AlignmentSnapState {
                snapped_x: true,
                guides: vec![AlignmentGuide {
                    kind: AlignmentGuideKind::Vertical,
                    position: 12.0,
                    extent_start: 8.0,
                    extent_end: 24.0,
                }],
                ..Default::default()
            },
        }),
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

    let guide_rects = prims
        .iter()
        .filter_map(|prim| match inner_prim(prim) {
            MetalPrimitive::ForegroundRect(rect)
                if rect.color == theme::PATCHER_ALIGNMENT_GUIDE() =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(guide_rects.len(), 1);
    assert!(guide_rects[0].width > 0.0);
    assert!(guide_rects[0].height > 0.0);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_emits_resize_handles_for_selected_node_only() {
    let patch = parse(
        r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
    );
    let mut state = PatcherInteractionState::default();
    state.selected_nodes.insert("sig".to_string());
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

    let handles = prims
        .iter()
        .filter_map(|prim| match inner_prim(prim) {
            MetalPrimitive::ForegroundRect(rect)
                if rect.color == theme::PATCHER_NODE_SELECTED_BORDER()
                    && rect.rect.height <= NODE_RESIZE_HANDLE_SIZE_CELLS * DEFAULT_ZOOM + 0.001 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(handles.len(), 4);
    for handle in handles {
        let px_w = handle.width * 10.0;
        let px_h = handle.height * 20.0;
        assert!(
            (px_w - px_h).abs() < 0.001,
            "resize handles should be square in pixels, got {px_w}x{px_h}"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_suppresses_resize_handles_for_active_text_edit() {
    let mut state = PatcherInteractionState::default();
    let created_id = allocate_created_node(&mut state, "root", (2.0, 2.0));
    state.selected_nodes.insert(created_id.clone());
    state.text_edit = Some(PatcherTextEdit {
        node_id: created_id,
        text: "phasor".to_string(),
        original_text: String::new(),
        state: TextInputState::default(),
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

    let handle_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                inner_prim(prim),
                MetalPrimitive::ForegroundRect(rect)
                    if rect.color == theme::PATCHER_NODE_SELECTED_BORDER()
                        && rect.rect.height <= NODE_RESIZE_HANDLE_SIZE_CELLS * DEFAULT_ZOOM + 0.001
            )
        })
        .count();
    assert_eq!(handle_count, 0);
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
        .filter(|prim| matches!(inner_prim(prim), MetalPrimitive::PatchCable(_)))
        .count();
    let selected_cable_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                inner_prim(prim),
                MetalPrimitive::PatchCable(cable) if cable.color == theme::PATCHER_ERROR()
            )
        })
        .count();
    let handle_shell_count = prims
        .iter()
        .filter(|prim| {
            matches!(
                inner_prim(prim),
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
                inner_prim(prim),
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
fn metal_render_macro_back_button_uses_shader_chevron_not_text_glyph() {
    let path = temp_patcher_source_path("macro-back-chevron-render");
    fs::write(
        &path,
        r#"
            (defmacro ap (x)
              (phasor x))
            (def z (ap input))
        "#,
    )
    .expect("write source");
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let state = PatcherInteractionState {
        active_macro: Some("ap".to_string()),
        ..Default::default()
    };
    set_patcher_interaction_state(key, state);

    let prims = build_metal_primitives_for_patcher(
        &node,
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
    );

    assert!(
        prims.iter().any(|prim| {
            matches!(
                inner_prim(prim),
                MetalPrimitive::WidgetInstance { widget_type, .. }
                    if widget_type == "patcher-back-chevron"
            )
        }),
        "macro back button should render a shader-backed chevron"
    );
    assert!(
        !prims.iter().any(|prim| {
            matches!(
                inner_prim(prim),
                MetalPrimitive::ProportionalText(text) if text.text == "<"
            )
        }),
        "macro back button should not render the old ASCII chevron text glyph"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_emits_agentic_bubble_body_as_foreground_overlay() {
    let path = temp_patcher_source_path("agentic-bubble-foreground-render");
    fs::write(&path, "(out 0)").expect("write source");
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut pan = PatcherPanState::default();
    pan.zoom = 1.0;
    pan.content_width = 100.0;
    pan.content_height = 100.0;
    set_patcher_pan_state(key, pan);
    let mut state = PatcherInteractionState::default();
    allocate_agentic_bubble(&mut state, (2.0, 3.0));
    set_patcher_interaction_state(key, state);

    let prims = build_metal_primitives_for_patcher(
        &node,
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
    );

    let foreground_bubble_rects = prims
        .iter()
        .filter(|prim| {
            matches!(
                inner_prim(prim),
                MetalPrimitive::ForegroundRect(rect)
                    if rect.rect.width > 10.0 && rect.rect.height > 4.0
            )
        })
        .count();
    assert_eq!(
        foreground_bubble_rects, 1,
        "agentic bubble body must render in the foreground layer above cables and ports"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_render_uses_wide_wrapped_answer_agentic_bubble() {
    let path = temp_patcher_source_path("agentic-bubble-answer-render");
    fs::write(&path, "(out 0)").expect("write source");
    let node = patcher_test_node(&path);
    let key = patcher_state_key(&node);
    let mut pan = PatcherPanState::default();
    pan.zoom = 1.0;
    pan.content_width = 100.0;
    pan.content_height = 100.0;
    set_patcher_pan_state(key, pan);
    let mut state = PatcherInteractionState::default();
    let bubble_id = allocate_agentic_bubble(&mut state, (2.0, 3.0));
    let bubble = state.agentic_bubbles.get_mut(&bubble_id).expect("bubble");
    bubble.state = AgenticBubbleState::Answer {
        text: "This macro implements a virtual-analog TR-707-style kick drum synthesizer:\n\n1. **Envelopes**: It uses three separate decay histories triggered by 'trig'.\n2. **Pitch Modulation**: The fast envelope creates a rapid downward pitch sweep applied to both resonators."
            .to_string(),
        answered_at: Instant::now(),
    };
    set_patcher_interaction_state(key, state);

    let prims = build_metal_primitives_for_patcher(
        &node,
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
    );

    let body_width = prims
        .iter()
        .filter_map(|prim| match inner_prim(prim) {
            MetalPrimitive::ForegroundRect(rect)
                if rect.rect.width > 10.0 && rect.rect.height > 4.0 =>
            {
                Some(rect.rect.width)
            }
            _ => None,
        })
        .next()
        .expect("answer bubble body");
    assert!(
        body_width > 30.0,
        "answer bubble should use the wider answer layout"
    );

    let answer_lines = prims
        .iter()
        .filter_map(|prim| match inner_prim(prim) {
            MetalPrimitive::ProportionalText(text)
                if text.text.contains("macro")
                    || text.text.contains("TR-707")
                    || text.text.contains("Envelopes")
                    || text.text.contains("Pitch")
                    || text.text.contains("resonators") =>
            {
                Some(text.text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        answer_lines.len() > 1,
        "answer text should wrap into multiple rendered lines"
    );
    let max_answer_line_width = answer_lines
        .iter()
        .map(|line| text_width_from_char_cache(line, 13.0, 13.0 * 0.62 / 10.0))
        .fold(0.0, f32::max);
    assert!(
        max_answer_line_width <= body_width - 1.3 + 0.1,
        "answer text lines should fit within the answer bubble body"
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
                inner_prim(prim),
                MetalPrimitive::ProportionalText(text) if text.text == "biquad"
            )
        }),
        "active operator prefix should render its autocomplete suggestion"
    );
    assert!(
        prims.iter().any(|prim| {
            matches!(
                inner_prim(prim),
                MetalPrimitive::ProportionalText(text) if text.text == "IIR biquad filter."
            )
        }),
        "autocomplete documentation panel should render structured documentation from the operator manifest"
    );
    assert!(
        prims.iter().any(|prim| {
            matches!(
                inner_prim(prim),
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
                inner_prim(prim),
                MetalPrimitive::ProportionalText(text)
                    if text.text.contains("outlets:")
                        && text.text.contains("out")
            )
        }),
        "autocomplete documentation panel should render structured outlet signatures"
    );
    let suggestion_col = prims
        .iter()
        .find_map(|prim| match inner_prim(prim) {
            MetalPrimitive::ProportionalText(text) if text.text == "biquad" => Some(text.col),
            _ => None,
        })
        .expect("suggestion text");
    let doc_col = prims
        .iter()
        .find_map(|prim| match inner_prim(prim) {
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
                    inner_prim(prim),
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
                inner_prim(prim),
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
        .find_map(|prim| match inner_prim(prim) {
            MetalPrimitive::ProportionalText(text) if text.text == "phase-vocoder" => {
                Some(text.col)
            }
            _ => None,
        })
        .expect("suggestion text");
    let doc_lines: Vec<&str> = prims
        .iter()
        .filter_map(|prim| match inner_prim(prim) {
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
        .filter_map(|prim| match inner_prim(prim) {
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
            width: None,
            param: None,
            inline_inputs: Vec::new(),
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

    let head = prims.iter().find_map(|prim| match inner_prim(prim) {
        MetalPrimitive::ProportionalText(text) if text.text == "in" => Some(text.col),
        _ => None,
    });
    let tail = prims.iter().find_map(|prim| match inner_prim(prim) {
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
