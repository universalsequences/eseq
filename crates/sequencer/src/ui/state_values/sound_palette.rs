//! Sound palette read surfaces (takes spec §17.6 / §18.3):
//! `SEQ.sound-palette` (the open overlay's entries) and
//! `SEQ.song-clip-sounds` (the timeline clip-dot identity join). Both diff by value
//! before publishing, like `scene-names` — the underlying scenes have no
//! revision counter and palette gestures can move refs without touching the
//! committed-song revision.

use super::*;
use crate::app::sound_palette::{PaletteEntry, PaletteTarget, SOUND_PALETTE_RGB};
use eseqlisp::sound_glyph_data::{
    publish_sound_glyph_frame, retain_sound_glyph_frames, SoundGlyphFrame, SoundGlyphMark,
    SoundGlyphStroke,
};
use sequencer::sound_glyph::{
    extract_skeleton, resolve_geometry, stock_skeleton, ExtractedSkeleton,
};
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct SoundPaletteFrameState {
    /// `(track, target, entries)` of the last published overlay, `None` when
    /// the last publish was Nil (closed).
    cached: Option<(usize, PaletteTarget, Vec<PaletteEntry>)>,
    /// Whether anything was ever published (so the first closed frame does
    /// not publish Nil over the registered default).
    published_open: bool,
    cached_clip_sounds: Option<Vec<Vec<(u64, bool, Option<u8>)>>>,
    glyphs: GlyphFrames,
}

/// Per-frame state for the sound-glyph feed (sound-glyph spec P2): the host
/// resolves each palette entry's plant geometry once and publishes it as an
/// eseqlisp `SoundGlyphFrame` keyed `sound-glyph:track:{t}:patch:{id}`; the
/// `sound-glyph` widget reads the frame by that key and stays dumb. P3 will
/// extend the published frame with per-branch divergence tints computed
/// here (host side), never in the widget.
#[derive(Default)]
struct GlyphFrames {
    /// Skeletons cached per instrument identity (engine name, or
    /// `builtin:<descriptor name>` for tracks without lisp source). A
    /// re-saved instrument goes stale until the app restarts — acceptable
    /// for P2; topology edits are rare mid-session.
    skeletons: HashMap<String, Arc<ExtractedSkeleton>>,
    /// key → fingerprint of the (skeleton, defaults) last published, so an
    /// unchanged sound never republishes (publish bumps the global widget
    /// state generation and would otherwise repaint every frame).
    published: HashMap<String, u64>,
    revision: u64,
}

fn glyph_key(track: usize, patch: u64) -> String {
    format!("sound-glyph:track:{track}:patch:{patch}")
}

/// Resolve + publish glyph frames for the open palette's entries. Runs every
/// sync frame while open (like the entries build itself); the fingerprint
/// gate makes the steady-state cost a hash per entry.
fn sync_glyph_frames(
    app: &app::App,
    track: usize,
    entries: &[PaletteEntry],
    glyphs: &mut GlyphFrames,
) {
    let fallback_descriptor;
    let descriptor = match app.graph.instrument_descriptors.get(track) {
        Some(desc) => desc,
        None => {
            fallback_descriptor = sequencer::effects::EffectDescriptor::builtin_sampler();
            &fallback_descriptor
        }
    };

    // Skeleton per instrument identity: authored source for custom
    // instruments, stock (descriptor param groups) otherwise.
    let custom_name = super::project_state::current_custom_instrument_name(app, track);
    let identity = match &custom_name {
        Some(name) => name.clone(),
        None => format!("builtin:{}", descriptor.name),
    };
    let extracted = match glyphs.skeletons.get(&identity) {
        Some(cached) => Arc::clone(cached),
        None => {
            let extracted = custom_name
                .as_deref()
                .and_then(|name| sequencer::lisp_host::load_instrument_source(name).ok())
                .map(|source| extract_skeleton(&source))
                .unwrap_or_else(|| stock_skeleton(descriptor));
            let extracted = Arc::new(extracted);
            glyphs
                .skeletons
                .insert(identity.clone(), Arc::clone(&extracted));
            extracted
        }
    };

    let mut active: HashSet<String> = HashSet::new();
    app.state.with_project_scenes(|scenes| {
        let Some(pool) = scenes.track_pools.get(track) else {
            return;
        };
        for entry in entries {
            let Some(patch) = pool.sounds.patches.get(&entry.patch) else {
                continue;
            };
            let key = glyph_key(track, entry.patch.0);

            let mut hasher = DefaultHasher::new();
            identity.hash(&mut hasher);
            for value in &patch.instrument_slot.defaults {
                value.to_bits().hash(&mut hasher);
            }
            let fingerprint = hasher.finish();
            active.insert(key.clone());
            if glyphs.published.get(&key) == Some(&fingerprint) {
                continue;
            }

            // Normalize the patch's authoring values to 0..1 via the
            // descriptor's min/max; descriptor params are index-aligned
            // with the slot defaults vector.
            let mut values: BTreeMap<String, f32> = BTreeMap::new();
            for (idx, param) in descriptor.params.iter().enumerate() {
                let raw = patch
                    .instrument_slot
                    .defaults
                    .get(idx)
                    .copied()
                    .unwrap_or(param.default);
                let range = param.max - param.min;
                let norm = if range.abs() > f32::EPSILON {
                    ((raw - param.min) / range).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                values.insert(param.name.clone(), norm);
            }

            let geometry = resolve_geometry(&extracted, &values);
            glyphs.revision = glyphs.revision.wrapping_add(1);
            publish_sound_glyph_frame(
                key.clone(),
                SoundGlyphFrame {
                    revision: glyphs.revision,
                    strokes: geometry
                        .strokes
                        .into_iter()
                        .map(|stroke| SoundGlyphStroke {
                            points: stroke.points,
                            width: stroke.width,
                        })
                        .collect(),
                    marks: geometry
                        .marks
                        .into_iter()
                        .map(|mark| SoundGlyphMark {
                            pos: mark.pos,
                            radius: mark.radius,
                        })
                        .collect(),
                },
            );
            glyphs.published.insert(key, fingerprint);
        }
    });
    retain_sound_glyph_frames(&active);
    glyphs.published.retain(|key, _| active.contains(key));
}

fn color_fields(map: &mut HashMap<String, Rc<RefCell<Value>>>, color: Option<u8>) {
    match color
        .map(usize::from)
        .filter(|idx| *idx < SOUND_PALETTE_RGB.len())
    {
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
            row.insert(
                "glyph-key".to_string(),
                Rc::new(RefCell::new(Value::String(glyph_key(track, entry.patch.0)))),
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
                    .map(|(clip_id, dot, color)| {
                        let mut map = HashMap::new();
                        map.insert(
                            "clip-id".to_string(),
                            Rc::new(RefCell::new(Value::Number(*clip_id as f64))),
                        );
                        map.insert("dot".to_string(), Rc::new(RefCell::new(Value::Bool(*dot))));
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
/// needed. The clip-sounds join (two lock scopes + a full per-clip build)
/// only runs while the arrangement is visible — nothing else reads it; the
/// cache clears on hide so re-showing republishes fresh. The palette half
/// stays ungated (cheap, and it also mounts in the *step* side panel).
pub(crate) fn sync_sound_palette(
    rt: &mut Runtime,
    app: &app::App,
    frame: &mut SoundPaletteFrameState,
    arrangement_visible: bool,
) -> bool {
    let mut dirty = false;
    match app.sound_palette_open {
        Some((track, target)) => {
            let entries = app.sound_palette_entries(track, target);
            sync_glyph_frames(app, track, &entries, &mut frame.glyphs);
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
                retain_sound_glyph_frames(&HashSet::new());
                frame.glyphs.published.clear();
            }
        }
    }
    if arrangement_visible {
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
    } else {
        // Hidden: skip the join entirely and forget the cache so the next
        // visible frame recomputes and republishes.
        frame.cached_clip_sounds = None;
    }
    dirty
}
