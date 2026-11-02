//! Save a sampler's start/end range as a region (`content/ui/sample-region.lisp`).
//! Right-click the sampler waveform, Save Region…: `sample-region-open`
//! remembers which sample and range, the modal names and tags it, and
//! `sample-region-commit` cuts the range into a library sample that records
//! its parent (`sample_db::RegionOrigin`), so the browser can filter to
//! regions and say where each one came from.
//!
//! Packs too: curated lists of samples kept apart from tags (a training set
//! to point the trainer at). The browser's sample menu adds and removes
//! samples (`sample-pack-add` / `-remove`) and names a new pack through the
//! New Pack modal (`sample-pack-new`, then `sample-pack-add`); Save Region
//! can drop the region straight into one.

use crate::*;
use std::path::PathBuf;

pub(super) const COMMANDS: &[&str] = &[
    "sample-region-open", "sample-region-close", "sample-region-commit",
    "sample-pack-new", "sample-pack-add", "sample-pack-remove",
];

struct Draft {
    parent: PathBuf,
    start: f64,
    end: f64,
}

thread_local! {
    static DRAFT: std::cell::RefCell<Option<Draft>> = const { std::cell::RefCell::new(None) };
    /// The tags the last region was saved with: cutting a run of hits from
    /// one break should not mean retyping them.
    static LAST_TAGS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The pack the last region went into (or was added to from the browser).
    static LAST_PACK: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Modals are mounted in the step-panel buffers, not the *track* panel or
/// browser they are opened from.
fn activate_modal_tile(editor: &mut Editor) {
    if !editor.switch_active_tile_to_buffer_named("*arrangement*") {
        editor.switch_active_tile_to_buffer_named("*sequencer*");
    }
}

/// A region's span for display: milliseconds under a second (one-hits are
/// mostly that short, and tenths of a second would hide them), else seconds.
pub(crate) fn span_text(start: f64, end: f64) -> String {
    if end - start < 1.0 {
        format!("{:.0}–{:.0} ms", start * 1000.0, end * 1000.0)
    } else {
        format!("{:.2}–{:.2} s", start, end)
    }
}

/// The sample a sampler track (or rack slot) plays.
fn sampler_path(app: &app::App, track: usize, rack_slot: Option<usize>) -> Option<PathBuf> {
    match rack_slot {
        Some(slot) => {
            let snapshot = app.state.latest_scheduler_snapshot();
            let slot = snapshot.tracks.get(track)?.rack_track.as_ref()?.slots.get(slot)?;
            state_values::rack_slot_sample_path(app, slot).cloned()
        }
        None => app.sampler_path_for_track(track),
    }
    // Library samples are stored as `samples/<hash>.wav`, an identity rather
    // than a path relative to the working directory.
    .map(|path| sequencer::app_paths::resolve_sample_ref(&path))
}

fn parent_title(path: &std::path::Path) -> String {
    sequencer::sample_db::display_title_for_sample_path(path)
        .unwrap_or_else(|| sequencer::sample_import::default_title_for_path(path))
}

fn open(app: &app::App, editor: &mut Editor, payload: &Value) -> Result<(), String> {
    let Value::Map(map) = payload else { return Err("Missing region".into()) };
    let track = map_usize(map, "track").ok_or("Missing track")?;
    let rack_slot = map_number(map, "rack-slot").filter(|slot| *slot >= 0.0).map(|slot| slot as usize);
    let start = map_number(map, "start").filter(|v| v.is_finite()).ok_or("Missing region start")?;
    let end = map_number(map, "end").filter(|v| v.is_finite()).ok_or("Missing region end")?;
    let (start, end) = (start.min(end).max(0.0), start.max(end));
    if end - start < 0.001 {
        return Err("Set a start and end first".into());
    }
    let parent = sampler_path(app, track, rack_slot).ok_or("This sampler has no sample")?;
    let title = parent_title(&parent);
    let span = span_text(start, end);
    let name = format!("{title} {span}");
    DRAFT.with(|draft| *draft.borrow_mut() = Some(Draft { parent, start, end }));
    activate_modal_tile(editor);
    let tags = LAST_TAGS.with(|tags| tags.borrow().clone());
    let pack = LAST_PACK.with(|pack| pack.borrow().clone());
    let rt = editor.runtime_mut();
    let open = rt.global_value("eseq.sample-region/open").ok_or("Save Region UI is unavailable")?;
    rt.invoke(open, vec![
        Value::String(name), build_string_list(&tags),
        Value::String(title), Value::String(span), Value::String(pack),
    ]).map_err(|error| format!("{error:?}"))?;
    Ok(())
}

fn close(editor: &mut Editor) -> Result<(), String> {
    DRAFT.with(|draft| *draft.borrow_mut() = None);
    editor.runtime_mut().eval_str("(eseq.sample-region/close)").map_err(|error| format!("{error:?}"))?;
    Ok(())
}

fn string_list(payload: &Value, key: &str) -> Vec<String> {
    let Value::Map(map) = payload else { return Vec::new() };
    let Some(cell) = map.get(key) else { return Vec::new() };
    let Value::List(items) = &*cell.borrow() else { return Vec::new() };
    items.iter().filter_map(|item| match &*item.borrow() {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }).collect()
}

/// Store the draft's range. Returns whether it is new (identical audio keeps
/// its existing library entry).
fn store(payload: &Value) -> Result<bool, String> {
    let tags = string_list(payload, "tags");
    let (parent, start, end) = DRAFT.with(|draft| {
        let draft = draft.borrow();
        let draft = draft.as_ref().ok_or("Open Save Region first")?;
        Ok::<_, String>((draft.parent.clone(), draft.start, draft.end))
    })?;
    let name = extract_string_from_payload(payload, "name")
        .map(|name| name.trim().to_string()).filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{} {}", parent_title(&parent), span_text(start, end)));
    let paths = sequencer::app_paths::app_paths();
    let mut db = sequencer::sample_db::SampleDb::open(&paths.sample_db_path())
        .map_err(|error| format!("Could not open the sample library: {error}"))?;
    let stored = sequencer::sample_import::import_region_sample(
        &parent, start, end, &name, &tags, &paths.samples_dir(), &mut db,
    )?;
    LAST_TAGS.with(|last| *last.borrow_mut() = sequencer::sample_import::normalize_tags(&tags));
    let pack = extract_string_from_payload(payload, "pack").unwrap_or_default();
    let pack = pack.trim();
    if !pack.is_empty() {
        db.add_to_pack(pack, &stored.hash)
            .map_err(|error| format!("Saved the region, but could not add it to {pack}: {error}"))?;
    }
    LAST_PACK.with(|last| *last.borrow_mut() = pack.to_string());
    Ok(stored.inserted)
}

/// The library hash of a browser sample path (`samples/<hash>.wav`).
fn payload_hash(payload: &Value) -> Result<String, String> {
    let path = extract_string_from_payload(payload, "path").ok_or("Choose a sample")?;
    std::path::Path::new(&path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| sequencer::sample_db::is_content_hash(stem))
        .map(str::to_string)
        .ok_or_else(|| "Only library samples can go in a pack".to_string())
}

fn edit_pack(payload: &Value, add: bool) -> Result<String, String> {
    let hash = payload_hash(payload)?;
    let pack = extract_string_from_payload(payload, "pack")
        .map(|pack| pack.trim().to_string()).filter(|pack| !pack.is_empty())
        .ok_or("Name the pack")?;
    let paths = sequencer::app_paths::app_paths();
    let db = sequencer::sample_db::SampleDb::open(&paths.sample_db_path())
        .map_err(|error| format!("Could not open the sample library: {error}"))?;
    if add {
        db.add_to_pack(&pack, &hash).map_err(|error| format!("Could not add to {pack}: {error}"))?;
        LAST_PACK.with(|last| *last.borrow_mut() = pack.clone());
        Ok(format!("Added to {pack}"))
    } else {
        db.remove_from_pack(&pack, &hash).map_err(|error| format!("Could not remove from {pack}: {error}"))?;
        Ok(format!("Removed from {pack}"))
    }
}

fn open_new_pack(editor: &mut Editor, payload: &Value) -> Result<(), String> {
    let path = extract_string_from_payload(payload, "path").ok_or("Choose a sample")?;
    activate_modal_tile(editor);
    let rt = editor.runtime_mut();
    let open = rt.global_value("eseq.sample-region/open-new-pack").ok_or("New Pack UI is unavailable")?;
    rt.invoke(open, vec![Value::String(path)]).map_err(|error| format!("{error:?}"))?;
    Ok(())
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
) {
    run(name, payload, app, editor);
}

/// [`handle`] without the event loop context, which no command here uses.
#[cfg(test)]
pub(crate) fn handle_for_test(name: &str, payload: Value, app: &mut app::App, editor: &mut Editor) {
    run(name, payload, app, editor);
}

fn run(name: &str, payload: Value, app: &mut app::App, editor: &mut Editor) {
    let result = match name {
        "sample-region-open" => open(app, editor, &payload),
        "sample-region-close" => close(editor),
        "sample-region-commit" => (|| {
            let inserted = store(&payload)?;
            close(editor)?;
            let _ = refresh_sample_browser_buffer(editor);
            editor.show_transient_message(if inserted {
                "Saved the region to the sample library"
            } else {
                "That region is already in the sample library"
            });
            Ok(())
        })(),
        "sample-pack-new" => open_new_pack(editor, &payload),
        "sample-pack-add" | "sample-pack-remove" => edit_pack(&payload, name == "sample-pack-add")
            .map(|message| {
                let _ = editor.runtime_mut().eval_str("(eseq.sample-region/close-new-pack)");
                let _ = refresh_sample_browser_buffer(editor);
                editor.show_transient_message(message);
            }),
        _ => Ok(()),
    };
    if let Err(error) = result {
        let rt = editor.runtime_mut();
        if let Some(show) = rt.global_value("eseq.sample-region/show-error") {
            let _ = rt.invoke(show, vec![Value::String(error.clone())]);
        }
        editor.show_transient_message(error);
    }
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_text_uses_milliseconds_under_a_second() {
        assert_eq!(span_text(0.512, 1.02), "512–1020 ms");
        assert_eq!(span_text(28.4, 38.9), "28.40–38.90 s");
    }
}
