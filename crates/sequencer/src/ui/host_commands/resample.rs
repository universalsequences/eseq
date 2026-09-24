//! SP-404 style resampling (`content/ui/resample.lisp`). The master recorder
//! always keeps the last 30 seconds of output; `resample-open` prints that
//! ring into a frozen draft, the modal crops and auditions it through the
//! browser preview voice, and `resample-commit` stores the crop as a library
//! sample and loads it on a new sampler track.

use crate::*;
use std::sync::Arc;

use sequencer::audio::preview;

pub(super) const COMMANDS: &[&str] = &[
    "resample-open", "resample-close", "resample-audition", "resample-commit",
];

/// Every resample starts with this tag; the modal lets the user remove it.
const DEFAULT_TAG: &str = "Resampled";
/// Anything quieter than this counts as silence when seeding the crop.
const SILENCE: f32 = 1.0e-4;

struct Draft {
    /// Interleaved stereo, oldest first.
    samples: Arc<Vec<f32>>,
    sample_rate: u32,
}

thread_local! {
    static DRAFT: std::cell::RefCell<Option<Draft>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn register_state(runtime: &mut eseqlisp::Runtime) {
    runtime.register_reactive("RESAMPLE", vec![
        ("buffer", Value::Bool(false)),
        ("duration", Value::Number(0.0)),
        ("error", Value::String(String::new())),
    ], true); // Presentation only; capture fixtures may seed a preview.
}

/// The audible span of an interleaved stereo print, in seconds, so the crop
/// opens on what was played rather than on leading and trailing silence.
pub(crate) fn audible_span(samples: &[f32], sample_rate: u32) -> Option<(f64, f64)> {
    let loud = |frame: &[f32]| frame.iter().any(|s| s.abs() > SILENCE);
    let first = samples.chunks_exact(2).position(loud)?;
    let last = samples.chunks_exact(2).rposition(loud)?;
    let rate = f64::from(sample_rate.max(1));
    Some((first as f64 / rate, (last + 1) as f64 / rate))
}

/// Local wall-clock time, the default name of a resample.
fn capture_name() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    // SAFETY: localtime_r only writes the `tm` it is handed.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return "Resample".into();
    }
    format!(
        "Resample {:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec,
    )
}

/// Frames `[start, end)` of the draft for a crop in seconds.
fn crop_frames(draft: &Draft, start: f64, end: f64) -> Result<(usize, usize), String> {
    let frames = draft.samples.len() / 2;
    let rate = f64::from(draft.sample_rate);
    if !start.is_finite() || !end.is_finite() {
        return Err("Choose a crop inside the capture".into());
    }
    let from = ((start.max(0.0) * rate).round() as usize).min(frames);
    let to = ((end.max(0.0) * rate).round() as usize).min(frames);
    if to <= from {
        return Err("Choose a non-empty crop inside the capture".into());
    }
    Ok((from, to))
}

fn crop_payload(payload: &Value) -> Result<(f64, f64), String> {
    let Value::Map(map) = payload else { return Err("Missing resample crop".into()) };
    let start = map_number(map, "start").ok_or("Missing crop start")?;
    let end = map_number(map, "end").ok_or("Missing crop end")?;
    Ok((start, end))
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

pub(crate) fn open(app: &app::App, editor: &mut Editor) -> Result<(), String> {
    preview::stop();
    let print = app.master_recorder.print_history()
        .ok_or("Resampling is unavailable without an audio device")?;
    if print.samples.len() < 4 {
        return Err("Nothing has played yet".into());
    }
    let sample_rate = print.sample_rate;
    let duration = print.samples.len() as f64 / 2.0 / f64::from(sample_rate.max(1));
    let (start, end) = audible_span(&print.samples, sample_rate).unwrap_or((0.0, duration));
    // A unique key per print so the waveform subtree never shows a stale one.
    static PRINTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let key = format!("resample://{}", PRINTS.fetch_add(1, Ordering::Relaxed));
    let buffer = eseqlisp::audio::sample::SampleBuffer::from_pcm(
        key.into(), sample_rate, 2, print.samples.clone(),
    ).register();
    DRAFT.with(|draft| *draft.borrow_mut() = Some(Draft { samples: Arc::new(print.samples), sample_rate }));

    if !editor.switch_active_tile_to_buffer_named("*arrangement*") {
        editor.switch_active_tile_to_buffer_named("*sequencer*");
    }
    let rt = editor.runtime_mut();
    rt.set_reactive("RESAMPLE", "buffer", buffer.to_value());
    rt.set_reactive("RESAMPLE", "duration", Value::Number(duration));
    rt.set_reactive("RESAMPLE", "error", Value::String(String::new()));
    let open = rt.global_value("eseq.resample/open").ok_or("Resample UI is unavailable")?;
    rt.invoke(open, vec![
        Value::Number(start), Value::Number(end),
        Value::String(capture_name()), build_string_list(&[DEFAULT_TAG.to_string()]),
    ]).map_err(|error| format!("{error:?}"))?;
    Ok(())
}

pub(crate) fn close(editor: &mut Editor) -> Result<(), String> {
    preview::stop();
    DRAFT.with(|draft| *draft.borrow_mut() = None);
    let rt = editor.runtime_mut();
    rt.set_reactive("RESAMPLE", "buffer", Value::Bool(false));
    rt.eval_str("(eseq.resample/close)").map_err(|error| format!("{error:?}"))?;
    Ok(())
}

fn audition(payload: &Value) -> Result<(), String> {
    let (start, end) = crop_payload(payload)?;
    DRAFT.with(|draft| {
        let draft = draft.borrow();
        let draft = draft.as_ref().ok_or("Open resample first")?;
        let (from, to) = crop_frames(draft, start, end)?;
        preview::play_region(draft.samples.clone(), draft.sample_rate, from, to, true);
        Ok(())
    })
}

/// Store the crop in the library. Returns the stored file and whether it is
/// new (identical audio keeps its existing library entry).
fn store(payload: &Value) -> Result<(std::path::PathBuf, bool), String> {
    let (start, end) = crop_payload(payload)?;
    let name = extract_string_from_payload(payload, "name")
        .map(|name| name.trim().to_string()).filter(|name| !name.is_empty())
        .unwrap_or_else(capture_name);
    let tags = string_list(payload, "tags");
    let (samples, sample_rate) = DRAFT.with(|draft| {
        let draft = draft.borrow();
        let draft = draft.as_ref().ok_or("Open resample first")?;
        let (from, to) = crop_frames(draft, start, end)?;
        Ok::<_, String>((draft.samples[from * 2..to * 2].to_vec(), draft.sample_rate))
    })?;
    let paths = sequencer::app_paths::app_paths();
    let mut db = sequencer::sample_db::SampleDb::open(&paths.sample_db_path())
        .map_err(|error| format!("Could not open the sample library: {error}"))?;
    let stored = sequencer::sample_import::import_pcm_sample(
        samples, sample_rate, 2, &name, &tags, &paths.samples_dir(), &mut db,
    )?;
    Ok((stored.path, stored.inserted))
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = match name {
        "resample-open" => open(app, editor),
        "resample-close" => close(editor),
        "resample-audition" => audition(&payload),
        "resample-commit" => (|| {
            let (path, inserted) = store(&payload)?;
            close(editor)?;
            let _ = refresh_sample_browser_buffer(editor);
            let tracks_before = app.tracks.len();
            let path_value = Value::String(path.display().to_string());
            super::tracks::handle("add-track-sample", map_value([("path", path_value)]), app, editor, ctx);
            if app.tracks.len() > tracks_before {
                editor.show_transient_message(if inserted {
                    "Resampled onto a new sampler track and saved to the library"
                } else {
                    "Resampled onto a new sampler track (already in the library)"
                });
            }
            // A failed track add already reported why; the sample stays saved.
            Ok(())
        })(),
        _ => Ok(()),
    };
    if let Err(error) = result {
        editor.runtime_mut().set_reactive("RESAMPLE", "error", Value::String(error.clone()));
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
    fn audible_span_skips_leading_and_trailing_silence() {
        let mut samples = vec![0.0; 2 * 10];
        samples[2 * 3 + 1] = 0.5; // frame 3, right channel
        samples[2 * 6] = -0.2; // frame 6
        assert_eq!(audible_span(&samples, 10), Some((0.3, 0.7)));
        assert_eq!(audible_span(&[0.0; 8], 10), None);
    }

    #[test]
    fn crop_frames_clamps_to_the_print_and_rejects_empty_crops() {
        let draft = Draft { samples: Arc::new(vec![0.0; 2 * 100]), sample_rate: 10 };
        assert_eq!(crop_frames(&draft, 1.0, 2.5), Ok((10, 25)));
        assert_eq!(crop_frames(&draft, -1.0, 99.0), Ok((0, 100)));
        assert!(crop_frames(&draft, 3.0, 3.0).is_err());
        assert!(crop_frames(&draft, f64::NAN, 1.0).is_err());
    }
}
