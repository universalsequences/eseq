//! A sample's waveform while it is dragged over an audio lane, before it is
//! dropped (Ableton): the drag starts decoding the file on a background
//! thread, which registers the waveform under [`drag_preview_key`] as it
//! goes, so the drop preview fills in from the left. No warp analysis. It
//! decodes with the same WAV decoder the drop imports with, so whatever
//! drops also previews, at the length it will land.
//!
//! - `(seq-audio-drag-preview path)`: start (or reuse) the file's decode and
//!   return `(dict :id :seconds)`, or nil when it is not a readable WAV. One
//!   preview lives at a time: a new file frees the last.
//! - `(seq-audio-drag-preview-clear)`: free it (the drop imports for real).
//! - `(seq-audio-drag-preview-generation)`: bumps as decoding progresses
//!   (the event loop publishes it as `DRAG_PREVIEW_REACTIVE`'s `generation`).
//! - `(seq-audio-file-seconds path)`: a WAV's length from its headers.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use eseqlisp::audio::sample::{SampleBuffer, decode_wav_progressively, drag_preview_key, wav_info};
use eseqlisp::vm::Value;

const DRAG_PREVIEW_REACTIVE: &str = "DRAG_PREVIEW";
const GENERATION_FIELD: &str = "generation";
/// File lengths cached at most: the cache only saves re-reading headers
/// while one drag hovers, so it is cleared rather than grown.
const SECONDS_CACHE_LIMIT: usize = 256;

/// Bumped by a decode thread each time it registers more of a waveform.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Preview ids, never reused, so a cancelled decode's late registration is
/// never drawn for another file.
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

const RUNNING: u8 = 0;
const DONE: u8 = 1;
const FAILED: u8 = 2;

/// The one live preview.
struct Preview {
    path: PathBuf,
    id: u64,
    seconds: f64,
    keep_going: Arc<AtomicBool>,
    /// `RUNNING`, `DONE` or `FAILED` (a read error part way: decode afresh).
    state: Arc<AtomicU8>,
}

impl Preview {
    fn free(&self) {
        self.keep_going.store(false, Ordering::Relaxed);
        SampleBuffer::unregister(&drag_preview_key(self.id));
    }

    fn to_value(&self) -> Value {
        let field = |value: f64| Rc::new(RefCell::new(Value::Number(value)));
        Value::Map(HashMap::from([
            ("id".to_string(), field(self.id as f64)),
            ("seconds".to_string(), field(self.seconds)),
        ]))
    }
}

pub(crate) fn register_natives(runtime: &mut eseqlisp::Runtime) {
    runtime.register_reactive(
        DRAG_PREVIEW_REACTIVE,
        vec![(GENERATION_FIELD, Value::Number(0.0))],
        false,
    );
    let preview: Rc<RefCell<Option<Preview>>> = Rc::new(RefCell::new(None));
    runtime.register_native("seq-audio-drag-preview", {
        let preview = Rc::clone(&preview);
        move |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("seq-audio-drag-preview expects a path string".to_string());
            };
            let path = sequencer::app_paths::resolve_sample_ref(Path::new(path));
            let mut preview = preview.borrow_mut();
            if let Some(live) = preview.as_ref() {
                if live.path == path && live.state.load(Ordering::Acquire) != FAILED {
                    return Ok(live.to_value());
                }
            }
            let Ok(info) = wav_info(&path) else {
                return Ok(Value::Nil);
            };
            let next = Preview {
                path: path.clone(),
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                seconds: info.seconds(),
                keep_going: Arc::new(AtomicBool::new(true)),
                state: Arc::new(AtomicU8::new(RUNNING)),
            };
            let key = drag_preview_key(next.id);
            let (keep_going, state) = (Arc::clone(&next.keep_going), Arc::clone(&next.state));
            std::thread::Builder::new()
                .name("drag-preview-decode".to_string())
                .spawn(move || {
                    let finished = decode_wav_progressively(
                        &path,
                        || keep_going.load(Ordering::Relaxed),
                        |samples, info| {
                            // A preview freed meanwhile stays freed.
                            if !keep_going.load(Ordering::Relaxed) {
                                return;
                            }
                            let stereo = sequencer::instruments::sample_core::fold_to_stereo(
                                samples,
                                usize::from(info.channels),
                            );
                            SampleBuffer::register_stereo(
                                Path::new(&key),
                                info.sample_rate,
                                info.channels,
                                stereo,
                            );
                            GENERATION.fetch_add(1, Ordering::Relaxed);
                        },
                    );
                    let outcome = if matches!(finished, Ok(true)) { DONE } else { FAILED };
                    state.store(outcome, Ordering::Release);
                })
                .map_err(|error| format!("drag preview decode thread: {error}"))?;
            // Only a decode that started replaces (and frees) the last one.
            if let Some(old) = preview.replace(next) {
                old.free();
            }
            Ok(preview.as_ref().map_or(Value::Nil, Preview::to_value))
        }
    });
    runtime.register_native("seq-audio-drag-preview-clear", move |_args, _ctx| {
        if let Some(old) = preview.borrow_mut().take() {
            old.free();
        }
        Ok(Value::Nil)
    });
    runtime.register_native("seq-audio-drag-preview-generation", |_args, ctx| {
        ctx.track_reactive_read(DRAG_PREVIEW_REACTIVE, GENERATION_FIELD);
        Ok(Value::Number(GENERATION.load(Ordering::Relaxed) as f64))
    });
    let seconds: RefCell<HashMap<PathBuf, Option<f64>>> = RefCell::new(HashMap::new());
    runtime.register_native("seq-audio-file-seconds", move |args, _ctx| {
        let Some(Value::String(path)) = args.first() else {
            return Err("seq-audio-file-seconds expects a path string".to_string());
        };
        let path = sequencer::app_paths::resolve_sample_ref(Path::new(path));
        let mut seconds = seconds.borrow_mut();
        if seconds.len() >= SECONDS_CACHE_LIMIT {
            seconds.clear();
        }
        let length = *seconds
            .entry(path)
            .or_insert_with_key(|path| wav_info(path).ok().map(|info| info.seconds()));
        Ok(length.map_or(Value::Nil, Value::Number))
    });
}

/// Publish decode progress to `DRAG_PREVIEW_REACTIVE` (event loop, every
/// tick). `last` is the generation published before.
pub(crate) fn publish(editor: &mut eseqlisp::Editor, last: &mut u64) {
    let generation = GENERATION.load(Ordering::Relaxed);
    if generation == *last {
        return;
    }
    *last = generation;
    let runtime = editor.runtime_mut();
    runtime.set_reactive(DRAG_PREVIEW_REACTIVE, GENERATION_FIELD, Value::Number(generation as f64));
    runtime.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}
