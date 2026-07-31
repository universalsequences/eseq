//! Host→widget data store for the `sound-glyph` widget (sound-glyph spec
//! P2). Same shape as the live-audio meter stores: the host resolves a
//! sound's plant geometry (sequencer's `sound_glyph` lib) into a
//! [`SoundGlyphFrame`] and publishes it under a string key; the widget reads
//! the frame by its `:source` prop at primitive-build time and stays dumb —
//! it never parses lisp or computes geometry.
//!
//! Frames only change when a sound's params change (palette sync), not per
//! audio frame. P3 (diff coloring) extends [`SoundGlyphStroke`] /
//! [`SoundGlyphMark`] with a host-computed per-branch tint; the widget will
//! keep drawing whatever the frame says.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

/// One stroked polyline in glyph unit space (x, y in 0..1, y = 0 at the
/// top), root first. `width` is the root stroke width in unit space; the
/// renderer tapers toward the tip.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundGlyphStroke {
    pub points: Vec<[f32; 2]>,
    pub width: f32,
}

/// A node mark (filled dot) in glyph unit space; `radius` in unit space.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundGlyphMark {
    pub pos: [f32; 2],
    pub radius: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundGlyphFrame {
    /// Bumped by the host on republish so primitive caches invalidate.
    pub revision: u64,
    pub strokes: Vec<SoundGlyphStroke>,
    pub marks: Vec<SoundGlyphMark>,
}

static SOUND_GLYPH_FRAMES: OnceLock<Mutex<HashMap<String, Arc<SoundGlyphFrame>>>> = OnceLock::new();

fn sound_glyph_frames() -> &'static Mutex<HashMap<String, Arc<SoundGlyphFrame>>> {
    SOUND_GLYPH_FRAMES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn publish_sound_glyph_frame(key: impl Into<String>, frame: SoundGlyphFrame) {
    {
        let mut frames = sound_glyph_frames().lock().unwrap();
        frames.insert(key.into(), Arc::new(frame));
    }
    // Widgets fold the frame into their primitives at build time, so a new
    // frame must invalidate the compiled primitive cache to repaint.
    crate::widget_render::bump_widget_state_generation();
}

pub fn sound_glyph_frame(key: &str) -> Option<Arc<SoundGlyphFrame>> {
    sound_glyph_frames().lock().unwrap().get(key).cloned()
}

pub fn retain_sound_glyph_frames(active_keys: &HashSet<String>) {
    sound_glyph_frames()
        .lock()
        .unwrap()
        .retain(|key, _| active_keys.contains(key));
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
                strokes: vec![SoundGlyphStroke {
                    points: vec![[0.5, 0.9], [0.5, 0.1]],
                    width: 0.01,
                }],
                marks: vec![SoundGlyphMark {
                    pos: [0.5, 0.5],
                    radius: 0.01,
                }],
            },
        );
        assert_eq!(sound_glyph_frame("glyph").unwrap().revision, 3);
        retain_sound_glyph_frames(&HashSet::new());
        assert!(sound_glyph_frame("glyph").is_none());
    }
}
