//! Presentation of a frozen live-note capture. The buffer and import belong
//! to App; Lisp owns the crop/view gestures, with validation again on commit.

use super::*;
use crate::presented::{RetroItem, RetroView};
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
    // The capture's presentation is the `retro` kind (the host kinds push
    // it from `presented`); capture fixtures seed a preview
    // (`present-fixture`).
}

/// A variant pad's name as its cell shows it: "v1 tight", or "v1 D4" while
/// unnamed; "v1" when the pad is gone.
fn pad_variant_label(app: &app::App, track: Option<usize>, variant: u32) -> String {
    let pad = track.and_then(|track| {
        app.groups.iter().find_map(|group| {
            let member = group.members.iter().position(|m| *m == track)?;
            let rack = group.rack.as_ref()?;
            rack.pads.get(rack.pad_index_for_variant(member, variant)?)
        })
    });
    match pad {
        Some(pad) => match &pad.name {
            Some(name) => format!("v{variant} {name}"),
            None => format!(
                "v{variant} {}",
                crate::state_values::drum_rack_pad_label(pad.pad_note)
            ),
        },
        None => format!("v{variant}"),
    }
}

pub(crate) fn publish(editor: &mut Editor, app: &app::App, draft: &CaptureDraft) -> Result<(), String> {
    // One row per played track/pitch pair: simultaneous drum-pad hits never
    // hide each other, and pitched performances still show their note names.
    // A pad variant is its own row (docs/rack-pad-variants-spec.md), named
    // as the pad shows it.
    let variant_of = |note_variant: Option<u32>| note_variant.unwrap_or(0);
    let mut lanes: Vec<_> = draft
        .notes
        .iter()
        .map(|n| (n.track, n.transpose, variant_of(n.variant)))
        .collect();
    lanes.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)).then(a.2.cmp(&b.2)));
    lanes.dedup();
    let labels: Vec<String> = lanes
        .iter()
        .map(|(track, transpose, variant)| {
            let index = app.track_registry.index_of(*track);
            let name = index
                .and_then(|i| app.tracks.get(i))
                .map(String::as_str)
                .unwrap_or("Deleted track");
            if *variant > 0 {
                return format!("{name} · {}", pad_variant_label(app, index, *variant));
            }
            let note = (*transpose as i32 + 60).rem_euclid(12) as usize;
            let octave = (*transpose as i32 + 60).div_euclid(12) - 1;
            let pitch = [
                "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
            ][note];
            format!("{name} · {pitch}{octave}")
        })
        .collect();
    let items: Vec<RetroItem> = draft
        .notes
        .iter()
        .map(|note| RetroItem {
            lane: lanes
                .binary_search_by(|pair| {
                    pair.0
                        .cmp(&note.track)
                        .then(note.transpose.total_cmp(&pair.1))
                        .then(pair.2.cmp(&variant_of(note.variant)))
                })
                .unwrap(),
            start: note.start,
            end: note.end.max(note.start + 0.015),
        })
        .collect();
    let start = draft.notes.first().map(|note| note.start).unwrap_or(0.0);
    let end = draft.duration.max(0.001);
    let rt = editor.runtime_mut();
    present_retro(rt, |r| {
        *r = RetroView {
            lanes: labels,
            items,
            duration: end,
            truncated: draft.truncated,
            error: String::new(),
        }
    });
    // The crop spans the capture from its first note; the roll's zoom takes
    // the duration from here (the tick has not pushed `retro.duration` yet).
    let open = rt.global_value("eseq.retrospective/open").ok_or("MIDI capture UI is unavailable")?;
    rt.invoke(open, vec![Value::Number(start), Value::Number(end), Value::Number(end)])
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
                present_retro(editor.runtime_mut(), |r| r.error.clear());
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
                    present_retro(editor.runtime_mut(), |r| r.error.clear());
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
        present_retro(editor.runtime_mut(), |r| r.error = error.clone());
        editor.show_transient_message(error);
    }
    sync(editor.runtime_mut(), app);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

/// An audition error, into the capture's `error`. The audition's `playing`,
/// `position` and `playhead` are the `retro` kind's live fields (the host
/// kinds read the mailbox while something observes them).
pub(crate) fn sync(runtime: &mut eseqlisp::Runtime, app: &app::App) -> bool {
    if app.retrospective.draft.is_none() {
        return false;
    }
    match app.state.note_audition.take_error() {
        Some(error) => present_retro(runtime, |r| r.error = error),
        None => false,
    }
}
