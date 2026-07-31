//! Sound palette read surfaces (takes spec §17.6 / §18.3):
//! `SEQ.sound-palette` (the open overlay's entries) and
//! `SEQ.song-clip-sounds` (the timeline clip-dot join). Both diff by value
//! before publishing, like `scene-names` — the underlying scenes have no
//! revision counter and palette gestures can move refs without touching the
//! committed-song revision.

use super::*;
use crate::app::sound_palette::{PaletteEntry, PaletteTarget, SOUND_PALETTE_RGB};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
pub(crate) struct SoundPaletteFrameState {
    /// `(track, target, entries)` of the last published overlay, `None` when
    /// the last publish was Nil (closed).
    cached: Option<(usize, PaletteTarget, Vec<PaletteEntry>)>,
    /// Whether anything was ever published (so the first closed frame does
    /// not publish Nil over the registered default).
    published_open: bool,
    cached_clip_sounds: Option<Vec<Vec<(u64, bool, Option<u8>)>>>,
}

fn color_fields(map: &mut HashMap<String, Rc<RefCell<Value>>>, color: Option<u8>) {
    match color.map(usize::from).filter(|idx| *idx < SOUND_PALETTE_RGB.len()) {
        Some(idx) => {
            let [r, g, b] = SOUND_PALETTE_RGB[idx];
            map.insert(
                "color".to_string(),
                Rc::new(RefCell::new(Value::Number(idx as f64))),
            );
            map.insert(
                "color-r".to_string(),
                Rc::new(RefCell::new(Value::Number(r as f64))),
            );
            map.insert(
                "color-g".to_string(),
                Rc::new(RefCell::new(Value::Number(g as f64))),
            );
            map.insert(
                "color-b".to_string(),
                Rc::new(RefCell::new(Value::Number(b as f64))),
            );
        }
        None => {
            map.insert("color".to_string(), Rc::new(RefCell::new(Value::Nil)));
        }
    }
}

fn build_palette_value(track: usize, target: PaletteTarget, entries: &[PaletteEntry]) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "track".to_string(),
        Rc::new(RefCell::new(Value::Number(track as f64))),
    );
    let (kind, id) = match target {
        PaletteTarget::Take(id) => ("take", Some(id.0)),
        PaletteTarget::Pattern(id) => ("pattern", Some(id.0)),
        PaletteTarget::Cell => ("cell", None),
    };
    map.insert(
        "target-kind".to_string(),
        Rc::new(RefCell::new(Value::String(kind.to_string()))),
    );
    map.insert(
        "target-id".to_string(),
        Rc::new(RefCell::new(match id {
            Some(id) => Value::Number(id as f64),
            None => Value::Nil,
        })),
    );
    let rows = entries
        .iter()
        .map(|entry| {
            let mut row = HashMap::new();
            row.insert(
                "patch-id".to_string(),
                Rc::new(RefCell::new(Value::Number(entry.patch.0 as f64))),
            );
            row.insert(
                "mix-id".to_string(),
                Rc::new(RefCell::new(match entry.mix {
                    Some(id) => Value::Number(id.0 as f64),
                    None => Value::Nil,
                })),
            );
            row.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(entry.name.clone()))),
            );
            row.insert(
                "referents".to_string(),
                Rc::new(RefCell::new(Value::String(entry.referents.clone()))),
            );
            row.insert(
                "base".to_string(),
                Rc::new(RefCell::new(Value::Bool(entry.is_base))),
            );
            row.insert(
                "current".to_string(),
                Rc::new(RefCell::new(Value::Bool(entry.is_current))),
            );
            color_fields(&mut row, entry.color);
            Rc::new(RefCell::new(Value::Map(row)))
        })
        .collect();
    map.insert(
        "entries".to_string(),
        Rc::new(RefCell::new(Value::List(rows))),
    );
    Value::Map(map)
}

fn build_clip_sounds_value(tracks: &[Vec<(u64, bool, Option<u8>)>]) -> Value {
    Value::List(
        tracks
            .iter()
            .map(|clips| {
                let clips = clips
                    .iter()
                    .map(|(clip_id, diverges, color)| {
                        let mut map = HashMap::new();
                        map.insert(
                            "clip-id".to_string(),
                            Rc::new(RefCell::new(Value::Number(*clip_id as f64))),
                        );
                        map.insert(
                            "diverges".to_string(),
                            Rc::new(RefCell::new(Value::Bool(*diverges))),
                        );
                        color_fields(&mut map, *color);
                        Rc::new(RefCell::new(Value::Map(map)))
                    })
                    .collect();
                Rc::new(RefCell::new(Value::List(clips)))
            })
            .collect(),
    )
}

/// Publish the palette read surfaces. Returns true when a reactive cycle is
/// needed.
pub(crate) fn sync_sound_palette(
    rt: &mut Runtime,
    app: &app::App,
    frame: &mut SoundPaletteFrameState,
) -> bool {
    let mut dirty = false;
    match app.sound_palette_open {
        Some((track, target)) => {
            let entries = app.sound_palette_entries(track, target);
            let snapshot = (track, target, entries);
            if frame.cached.as_ref() != Some(&snapshot) {
                dirty |= rt
                    .set_reactive(
                        "SEQ",
                        "sound-palette",
                        build_palette_value(snapshot.0, snapshot.1, &snapshot.2),
                    )
                    .effects_dirty;
                frame.cached = Some(snapshot);
                frame.published_open = true;
            }
        }
        None => {
            if frame.published_open || frame.cached.is_some() {
                dirty |= rt
                    .set_reactive("SEQ", "sound-palette", Value::Nil)
                    .effects_dirty;
                frame.cached = None;
                frame.published_open = false;
            }
        }
    }
    let clip_sounds = app.song_clip_sounds();
    if frame.cached_clip_sounds.as_ref() != Some(&clip_sounds) {
        dirty |= rt
            .set_reactive(
                "SEQ",
                "song-clip-sounds",
                build_clip_sounds_value(&clip_sounds),
            )
            .effects_dirty;
        frame.cached_clip_sounds = Some(clip_sounds);
    }
    dirty
}
