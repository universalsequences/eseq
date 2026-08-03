//! Host→widget data store for the palette's cohort-relative delta glyph.
//!
//! The sequencer computes all schema/cohort semantics. This crate receives a
//! compact, renderer-neutral lattice frame; the `sound-glyph` widget only
//! packs it for its Metal shader.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

/// One accent piece: a welded polyomino anchored at a lattice slot.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundGlyphPiece {
    pub slot: usize,
    pub piece: u8,
    pub hue: u8,
    pub magnitude: u8,
    pub mirror: bool,
    pub negative: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SoundGlyphFrame {
    pub revision: u64,
    pub cols: usize,
    pub rows: usize,
    /// Per slot, 0 = unassigned, else 1..15 over the substrate radius band.
    pub substrate: Vec<u8>,
    pub pieces: Vec<SoundGlyphPiece>,
    /// The anchor tile renders its substrate with no accents.
    pub anchor: bool,
    pub incompatible: bool,
}

static SOUND_GLYPH_FRAMES: OnceLock<Mutex<HashMap<String, Arc<SoundGlyphFrame>>>> = OnceLock::new();

fn sound_glyph_frames() -> &'static Mutex<HashMap<String, Arc<SoundGlyphFrame>>> {
    SOUND_GLYPH_FRAMES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn publish_sound_glyph_frame(key: impl Into<String>, frame: SoundGlyphFrame) {
    publish_sound_glyph_frames([(key.into(), frame)]);
}

/// Publish a cohort atomically and invalidate widget primitives once. A
/// reference change normally replaces every tile, so per-frame invalidation
/// would perform redundant global generation bumps.
pub fn publish_sound_glyph_frames(frames: impl IntoIterator<Item = (String, SoundGlyphFrame)>) {
    let mut store = sound_glyph_frames().lock().unwrap();
    let mut changed = false;
    for (key, frame) in frames {
        store.insert(key, Arc::new(frame));
        changed = true;
    }
    drop(store);
    if changed {
        crate::widget_render::bump_widget_state_generation();
    }
}

pub fn sound_glyph_frame(key: &str) -> Option<Arc<SoundGlyphFrame>> {
    sound_glyph_frames().lock().unwrap().get(key).cloned()
}

/// Prune ONE publisher's namespace: keys under `prefix` survive only when in
/// `active_keys`; other publishers' keys are untouched. A global retain here
/// would let the palette feed and the mixer-cell feed silently prune each
/// other every sync.
pub fn retain_sound_glyph_frames(prefix: &str, active_keys: &HashSet<String>) {
    sound_glyph_frames()
        .lock()
        .unwrap()
        .retain(|key, _| !key.starts_with(prefix) || active_keys.contains(key));
}

pub fn clear_sound_glyph_frames() {
    sound_glyph_frames().lock().unwrap().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_retain() {
        clear_sound_glyph_frames();
        publish_sound_glyph_frame(
            "glyph",
            SoundGlyphFrame {
                revision: 3,
                cols: 1,
                rows: 1,
                substrate: vec![9],
                pieces: vec![SoundGlyphPiece {
                    slot: 0, piece: 4, hue: 1, magnitude: 6,
                    mirror: false, negative: false,
                }],
                anchor: false,
                incompatible: false,
            },
        );
        assert_eq!(sound_glyph_frame("glyph").unwrap().revision, 3);
        // Retain is namespace-scoped: a different prefix leaves the key alone.
        retain_sound_glyph_frames("other:", &HashSet::new());
        assert!(sound_glyph_frame("glyph").is_some());
        retain_sound_glyph_frames("glyph", &HashSet::new());
        assert!(sound_glyph_frame("glyph").is_none());
    }
}
