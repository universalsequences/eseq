//! The piano roll's buffer mode in the full factory UI: musical typing and
//! the inherited sequencer keymap (grid focus, Backspace and the gutter audition are in
//! host_kinds/tests/piano_roll_view.rs, which has a live piano roll).
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn piano_roll_editor() -> eseqlisp::Editor {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor
        .runtime_mut()
        .eval_str("(eseq.seq-panels/seq-toggle-main-or-piano-roll)")
        .expect("open piano roll");
    editor.refresh_runtime_side_effects();
    assert!(editor.switch_active_tile_to_buffer_named("*piano-roll*"));
    let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 240, 80);
    editor
}

/// Regression: the buffer named a mode that auto-qualified to one that does
/// not exist, so it had no keymap — musical typing never reached an armed
/// track and Backspace did nothing until the note grid itself had focus.
#[test]
fn piano_roll_buffer_mode_keeps_musical_typing_and_the_sequencer_keymap() {
    let editor = piano_roll_editor();
    let held = Arc::new(Mutex::new(Vec::new()));
    let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    assert!(editor.active_mode_accepts_live_keys(), "piano roll mode chain must opt in");
    assert!(crate::input::should_route_to_live_keyboard(&editor, &key, &held, false));
    // The inherited sequencer keymap is back (arrows, track up/down …).
    assert!(editor
        .buffer_mode_keybinding("*piano-roll*", KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
        .is_some());
}
