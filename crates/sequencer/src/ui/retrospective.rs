//! Presentation of a frozen live-note capture. The buffer and import belong
//! to App; Lisp owns the crop/view gestures, with validation again on commit.

use super::*;
use sequencer::app::retrospective::{capture_bar_count, detect_capture_loop, CaptureDraft};

pub(crate) const COMMANDS: &[&str] = &[
    "retrospective-open", "retrospective-close", "retrospective-import",
    "retrospective-audition", "retrospective-stop", "retrospective-detect",
];

pub(crate) fn register_state(runtime: &mut eseqlisp::Runtime) {
    runtime.register_native("seq-capture-bar-count", |args, _ctx| {
        let [Value::Number(duration), Value::Number(bars)] = args.as_slice() else {
            return Err("seq-capture-bar-count expects duration in seconds and a whole bar count".into());
        };
        if !bars.is_finite() || *bars < 1.0 || bars.fract() != 0.0 {
            return Err("Choose a whole number of bars".into());
        }
        capture_bar_count(*duration, *bars as usize).map(|bars| Value::Number(bars as f64))
    });
    runtime.register_reactive("RETRO", vec![
        ("items", Value::List(vec![])),
        ("lanes", Value::List(vec![])),
        ("duration", Value::Number(0.0)),
        ("error", Value::String(String::new())),
        ("truncated", Value::Bool(false)),
        ("playing", Value::Bool(false)),
        ("position", Value::Number(0.0)),
    ], true); // Presentation only; capture fixtures may seed a preview.
}

pub(crate) fn publish(editor: &mut Editor, app: &app::App, draft: &CaptureDraft) -> Result<(), String> {
    // One row per played track/pitch pair: simultaneous drum-pad hits never
    // hide each other, and pitched performances still show their note names.
    let mut lanes: Vec<_> = draft.notes.iter().map(|n| (n.track, n.transpose)).collect();
    lanes.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
    lanes.dedup();
    let lane_values = lanes.iter().enumerate().map(|(index, (track, transpose))| {
        let name = app.track_registry.index_of(*track).and_then(|i| app.tracks.get(i))
            .map(String::as_str).unwrap_or("Deleted track");
        let note = (*transpose as i32 + 60).rem_euclid(12) as usize;
        let octave = (*transpose as i32 + 60).div_euclid(12) - 1;
        let pitch = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"][note];
        map_value([
            ("id", Value::Number(index as f64)),
            ("label", Value::String(format!("{name} · {pitch}{octave}"))),
        ])
    });
    let items = draft.notes.iter().enumerate().map(|(index, note)| {
        let lane = lanes.binary_search_by(|pair| {
            pair.0.cmp(&note.track).then(note.transpose.total_cmp(&pair.1))
        }).unwrap();
        map_value([
            ("id", Value::Number(index as f64)),
            ("lane", Value::Number(lane as f64)),
            ("start", Value::Number(note.start)),
            ("end", Value::Number(note.end.max(note.start + 0.015))),
        ])
    });
    let start = draft.notes.first().map(|note| note.start).unwrap_or(0.0);
    let end = draft.duration.max(0.001);
    let rt = editor.runtime_mut();
    rt.set_reactive("RETRO", "lanes", list_value(lane_values));
    rt.set_reactive("RETRO", "items", list_value(items));
    rt.set_reactive("RETRO", "duration", Value::Number(end));
    rt.set_reactive("RETRO", "error", Value::String(String::new()));
    rt.set_reactive("RETRO", "truncated", Value::Bool(draft.truncated));
    let open = rt.global_value("eseq.retrospective/open").ok_or("MIDI capture UI is unavailable")?;
    rt.invoke(open, vec![Value::Number(start), Value::Number(end)])
        .map_err(|error| format!("{error:?}"))?;
    apply_guess(editor, draft)?;
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
    Ok(())
}

/// Seed the crop from the detected groove. False when nothing repeats; the
/// crop then stays wherever it was.
fn apply_guess(editor: &mut Editor, draft: &CaptureDraft) -> Result<bool, String> {
    let Some(guess) = detect_capture_loop(&draft.notes) else { return Ok(false) };
    let rt = editor.runtime_mut();
    let apply = rt.global_value("eseq.retrospective/apply-guess").ok_or("MIDI capture UI is unavailable")?;
    rt.invoke(apply, vec![Value::Number(guess.start), Value::Number(guess.bpm as f64),
        Value::Number(guess.bars as f64)]).map_err(|error| format!("{error:?}"))?;
    Ok(true)
}

pub(crate) fn handle(name: &str, payload: Value, app: &mut app::App, editor: &mut Editor) {
    let result = (|| -> Result<(), String> {
        match name {
            "retrospective-open" => {
                app.state.note_audition.stop();
                let scene = app.state.current_scene_id().ok_or("No scene to capture into")?;
                let draft = app.retrospective.snapshot(Instant::now(), scene).clone();
                if !editor.switch_active_tile_to_buffer_named("*arrangement*") {
                    editor.switch_active_tile_to_buffer_named("*sequencer*");
                }
                publish(editor, app, &draft)?;
            }
            "retrospective-stop" => app.state.note_audition.stop(),
            "retrospective-detect" => {
                // A playing preview restarts on the detected loop.
                let draft = app.retrospective.draft.as_ref().ok_or("Open MIDI capture first")?;
                if !apply_guess(editor, draft)? {
                    return Err("No repeating groove found. Set the crop by hand".into());
                }
                editor.runtime_mut().set_reactive("RETRO", "error", Value::String(String::new()));
            }
            "retrospective-close" => {
                app.state.note_audition.stop();
                app.retrospective.draft = None;
                editor.runtime_mut().eval_str("(eseq.retrospective/close)")
                    .map_err(|error| format!("{error:?}"))?;
            }
            "retrospective-import" | "retrospective-audition" => {
                let Value::Map(map) = payload else { return Err("Missing capture crop".into()); };
                let start = map_number(&map, "start").ok_or("Missing crop start")?;
                let end = map_number(&map, "end").ok_or("Missing crop end")?;
                let bars = map_number(&map, "bars").filter(|v| v.is_finite() && *v >= 1.0 && v.fract() == 0.0)
                    .ok_or("Choose a whole number of bars")?;
                if name == "retrospective-audition" {
                    app.audition_retrospective(start, end, bars as usize)?;
                    editor.runtime_mut().set_reactive("RETRO", "error", Value::String(String::new()));
                    return Ok(());
                }
                let count = app.import_retrospective(start, end, bars as usize)?;
                app.retrospective.draft = None;
                editor.runtime_mut().eval_str("(eseq.retrospective/close)")
                    .map_err(|error| format!("{error:?}"))?;
                editor.show_transient_message(format!("Recovered {count} trigs into new track patterns"));
            }
            _ => {}
        }
        Ok(())
    })();
    if let Err(error) = result {
        editor.runtime_mut().set_reactive("RETRO", "error", Value::String(error.clone()));
        editor.show_transient_message(error);
    }
    sync(editor.runtime_mut(), app);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

pub(crate) fn sync(runtime: &mut eseqlisp::Runtime, app: &app::App) -> bool {
    let mut changed = runtime.set_reactive("RETRO", "playing",
        Value::Bool(app.state.note_audition.generation() != 0)).effects_dirty;
    if app.retrospective.draft.is_some() {
        changed |= runtime.set_reactive("RETRO", "position", Value::Number(app.state.note_audition.position())).effects_dirty;
        if let Some(error) = app.state.note_audition.take_error() {
            changed |= runtime.set_reactive("RETRO", "error", Value::String(error)).effects_dirty;
        }
    }
    changed
}
