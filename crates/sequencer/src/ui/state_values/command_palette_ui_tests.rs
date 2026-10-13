//! The command palette over the real UI (docs/command-palette-spec.md):
//! the shipped `:command` definitions list, and their `:choices` stages
//! resolve against the live sequencer.

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn open_palette(editor: &mut eseqlisp::Editor) {
    editor.runtime_mut().eval_str("(command-palette)").expect("open palette");
    editor.refresh_runtime_side_effects();
}

fn type_text(editor: &mut eseqlisp::Editor, text: &str) {
    for ch in text.chars() {
        editor.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
}

fn enter(editor: &mut eseqlisp::Editor) {
    editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

fn row_labels(editor: &eseqlisp::Editor) -> Vec<String> {
    editor.palette_frame().expect("palette open").rows.into_iter().map(|row| row.label).collect()
}

#[test]
fn change_theme_picks_from_the_registry_with_the_current_theme_checked() {
    let mut editor = full_grid_editor_for_scroll_tests();
    open_palette(&mut editor);
    let rows = row_labels(&editor);
    assert!(rows.iter().any(|row| row == "Change Theme"), "{rows:?}");
    assert!(rows.iter().any(|row| row == "Add Instrument"), "{rows:?}");

    type_text(&mut editor, "change theme");
    enter(&mut editor);
    let frame = editor.palette_frame().expect("theme stage");
    assert_eq!(frame.badge.as_deref(), Some("Theme"));
    let checked: Vec<_> = frame.rows.iter().filter(|row| row.checked).map(|row| row.label.as_str()).collect();
    assert_eq!(checked, ["macOS Dark"], "the startup theme is checked");

    type_text(&mut editor, "aura");
    enter(&mut editor);
    assert!(editor.palette_frame().is_none());
    let current = editor.runtime_mut().eval_str("seq-current-theme").expect("read theme");
    assert_eq!(current, Some(Value::String("aura".into())));
}

#[test]
fn add_instrument_lists_the_instruments_tab_flattened() {
    let mut editor = full_grid_editor_for_scroll_tests();
    open_palette(&mut editor);
    type_text(&mut editor, "add instrument");
    enter(&mut editor);
    let frame = editor.palette_frame().expect("instrument stage");
    assert_eq!(frame.badge.as_deref(), Some("Instrument"));
    assert!(!frame.rows.is_empty(), "instruments listed");
    assert!(frame.rows.iter().all(|row| !row.submenu), "folders are flattened away");
}

#[test]
fn every_shipped_command_is_documented_and_runnable_from_the_palette() {
    let editor = full_grid_editor_for_scroll_tests();
    let commands = editor.runtime().commands();
    assert!(commands.len() >= 30, "menu actions are commands: {}", commands.len());
    for (name, header) in &commands {
        assert!(header.summary().is_some(), "{name} has no :doc");
        assert!(header.command_problem(name).is_none(), "{name}: {:?}", header.command_problem(name));
    }
    let titles: Vec<_> = commands.iter().filter_map(|(_, header)| header.command.as_deref()).collect();
    for expected in ["New Project", "Save", "Transpose Pattern", "Choose Agent Model…", "Change Theme"] {
        assert!(titles.contains(&expected), "{expected} missing from {titles:?}");
    }
}

#[test]
fn change_theme_previews_while_browsing_and_restores_on_escape() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.runtime_mut().eval_str(r#"(command-palette "change-theme")"#).expect("open theme stage");
    editor.refresh_runtime_side_effects();
    let theme = |editor: &mut eseqlisp::Editor| editor.runtime_mut().eval_str("seq-current-theme").expect("theme");
    editor.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(theme(&mut editor), Some(Value::String("mac-osx-light-theme".into())), "highlight previews");
    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(theme(&mut editor), Some(Value::String("mac-osx-dark".into())), "Esc restores");
}

#[test]
fn the_home_list_is_everyday_commands_flat() {
    let mut editor = full_grid_editor_for_scroll_tests();
    open_palette(&mut editor);
    let frame = editor.palette_frame().expect("open");
    assert!(frame.rows.iter().all(|row| !row.header), "no sections before typing");
    let rows = row_labels(&editor);
    assert_eq!(rows.first().map(String::as_str), Some("Apply Sound"), "ranked first: {rows:?}");
    for expected in ["Change Theme", "Add Instrument", "Add Effect", "Load Preset", "Play", "Set Tempo"] {
        assert!(rows.iter().any(|row| row == expected), "{expected} missing from {rows:?}");
    }
    for absent in ["Quit eseq", "About eseq", "Export Package…"] {
        assert!(!rows.iter().any(|row| row == absent), "{absent} pinned: {rows:?}");
    }
}

#[test]
fn set_tempo_lists_tempos_under_a_tempo_badge() {
    // Previewing and restoring are covered in eseqlisp's palette tests; this
    // editor has no host pushing transport values back.
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.runtime_mut().eval_str(r#"(command-palette "eseq.application-menus/set-tempo")"#).unwrap();
    editor.refresh_runtime_side_effects();
    assert_eq!(editor.palette_frame().unwrap().badge.as_deref(), Some("Tempo"));
    let rows = row_labels(&editor);
    assert!(rows.iter().any(|row| row == "120 BPM"), "{rows:?}");
}

#[test]
fn slash_opens_the_palette_from_sequencer_views_but_types_in_text_buffers() {
    let mut editor = full_grid_editor_for_scroll_tests();
    assert!(editor.switch_active_tile_to_buffer_named("*sequencer*"));
    type_text(&mut editor, "/");
    assert!(editor.palette_frame().is_some(), "/ opens the palette");
    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    editor.open_scratch_buffer("*source*", "");
    type_text(&mut editor, "/");
    assert!(editor.palette_frame().is_none());
    assert_eq!(editor.active_buffer().text(), "/");
}
