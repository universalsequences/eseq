//! Arrangement region selection — a time x track rectangle over the song
//! (docs/arrangement-region-editing-spec.md 4).
//!
//! The region is Rust-owned for the same reasons the bound clip is
//! (`sound_binding::SongClipSelection`): it must survive a view switch and a
//! buffer reload, and the copy/paste/delete/move primitives read it directly
//! rather than being handed a rectangle by the UI script. The Lisp side owns
//! only the transient in-drag ghost.

use super::App;

/// A committed region selection. Track indices are MODEL indices and both
/// ends are inclusive; the beat span is half-open `[start_beat, end_beat)`,
/// matching how the song's lane spans are addressed everywhere else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SongRegionSelection {
    pub track_a: usize,
    pub track_b: usize,
    pub start_beat: f64,
    pub end_beat: f64,
}

impl SongRegionSelection {
    /// Normalizing constructor: callers pass the two ends of a drag in
    /// whatever order the pointer produced them.
    pub fn new(track_a: usize, track_b: usize, start_beat: f64, end_beat: f64) -> Self {
        Self {
            track_a: track_a.min(track_b),
            track_b: track_a.max(track_b),
            start_beat: start_beat.min(end_beat).max(0.0),
            end_beat: start_beat.max(end_beat).max(0.0),
        }
    }

    /// A region that selects no time selects nothing at all — the callers
    /// treat this as "clear" rather than storing an empty rectangle.
    pub fn is_empty(&self) -> bool {
        self.end_beat <= self.start_beat
    }

    pub fn contains_track(&self, track: usize) -> bool {
        track >= self.track_a && track <= self.track_b
    }
}

impl App {
    /// Set the region (spec 4.1). Returns true when it actually changed.
    ///
    /// The region names no single clip, so it takes the selection channel
    /// away from the clip selection and RELEASES the sound binding — the same
    /// rule scene-lane selections follow (takes spec 16.6 cause 2). An empty
    /// or degenerate rectangle clears instead of committing nothing-selected.
    pub fn set_song_region(&mut self, region: SongRegionSelection) -> bool {
        if region.is_empty() {
            return self.clear_song_region();
        }
        // Order matters only for the binding resync: dropping the clip
        // selection re-resolves every track's bound source once.
        self.set_song_clip_selection(None);
        if self.song_region_selection == Some(region) {
            return false;
        }
        self.song_region_selection = Some(region);
        true
    }

    /// Set the region as the footprint of the clip that was just selected,
    /// WITHOUT releasing the clip selection.
    ///
    /// Clicking a clip's title bar selects the clip *and* selects its span as
    /// a one-clip region (Ableton: a selected clip is a selected region, which
    /// is what makes copy/delete on it mean anything). Only a free marquee —
    /// which names no single clip — takes the binding away, via
    /// `set_song_region`.
    pub fn set_song_region_for_clip(&mut self, region: SongRegionSelection) -> bool {
        if region.is_empty() {
            return self.clear_song_region();
        }
        if self.song_region_selection == Some(region) {
            return false;
        }
        self.song_region_selection = Some(region);
        true
    }

    /// Clear the region (spec 4.1). Returns true when it actually changed.
    /// Does NOT touch the sound binding: clearing a region leaves the tracks
    /// on whatever the playback/scene rules resolve, which is where the
    /// region put them.
    pub fn clear_song_region(&mut self) -> bool {
        if self.song_region_selection.is_none() {
            return false;
        }
        self.song_region_selection = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::sound_binding::tests::app_with_take;
    use crate::app::sound_binding::BoundSource;
    use crate::sequencer::SongRowId;

    #[test]
    fn new_normalizes_reversed_ends() {
        let region = SongRegionSelection::new(5, 2, 16.0, 4.0);
        assert_eq!(region.track_a, 2);
        assert_eq!(region.track_b, 5);
        assert_eq!(region.start_beat, 4.0);
        assert_eq!(region.end_beat, 16.0);
        assert!(region.contains_track(3));
        assert!(!region.contains_track(6));
        assert!(!region.is_empty());
    }

    #[test]
    fn zero_width_region_is_empty() {
        assert!(SongRegionSelection::new(0, 0, 8.0, 8.0).is_empty());
    }

    #[test]
    fn negative_beats_clamp_to_zero() {
        let region = SongRegionSelection::new(0, 1, -4.0, 8.0);
        assert_eq!(region.start_beat, 0.0);
        assert_eq!(region.end_beat, 8.0);
    }

    /// 4.1 mutual exclusivity, region -> clip: a region names no single clip,
    /// so committing one drops the clip selection and hands the sound binding
    /// back to the playback/scene rules.
    #[test]
    fn setting_a_region_releases_the_bound_clip() {
        let (mut app, take, scene_pattern, _chunks) = app_with_take();
        app.select_song_clip(0, SongRowId(0))
            .expect("clip selects");
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take))
        );

        assert!(app.set_song_region(SongRegionSelection::new(0, 0, 0.0, 8.0)));
        assert_eq!(app.song_clip_selection, None, "the clip selection is dropped");
        assert!(
            app.track_sound_binding(0).is_scene(),
            "the binding falls back off rule 1"
        );
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Pattern(scene_pattern))
        );
    }

    /// Clip -> region: selecting a clip REPLACES the region with that clip's
    /// own span and keeps the binding — a selected clip is a one-clip region
    /// (Ableton), not the opposite of one. Both light the same body colour.
    #[test]
    fn selecting_a_clip_narrows_the_region_to_its_span() {
        let (mut app, take, _scene_pattern, _chunks) = app_with_take();
        app.set_song_region(SongRegionSelection::new(0, 0, 0.0, 64.0));
        assert!(app.song_region_selection.is_some());

        app.select_song_clip_span(0, SongRowId(0), Some((4.0, 12.0)))
            .expect("clip selects");
        assert_eq!(
            app.song_region_selection,
            Some(SongRegionSelection::new(0, 0, 4.0, 12.0)),
            "the region becomes the clip's footprint"
        );
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take)),
            "and the clip still owns the sound binding"
        );
    }

    /// Selecting with no span (nothing under the pointer) clears the region
    /// rather than leaving it highlighting a clip that is not selected.
    #[test]
    fn selecting_a_clip_without_a_span_clears_the_region() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        app.set_song_region(SongRegionSelection::new(0, 0, 0.0, 64.0));
        app.select_song_clip_span(0, SongRowId(0), None)
            .expect("clip selects");
        assert_eq!(app.song_region_selection, None);
    }

    /// Deselecting a clip must NOT clear the region: `set_song_region`
    /// deselects on its way in, and that internal call would otherwise wipe
    /// the region it is about to store.
    #[test]
    fn deselecting_a_clip_leaves_the_region_alone() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        app.set_song_region(SongRegionSelection::new(1, 3, 4.0, 12.0));
        app.set_song_clip_selection(None);
        assert_eq!(
            app.song_region_selection,
            Some(SongRegionSelection::new(1, 3, 4.0, 12.0))
        );

        assert!(app.clear_song_region());
        assert_eq!(app.song_region_selection, None);
        assert!(!app.clear_song_region(), "clearing twice reports no change");
    }

    /// A zero-width rectangle is not a selection: it clears instead.
    #[test]
    fn empty_region_clears_rather_than_committing() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 12.0));
        app.set_song_region(SongRegionSelection::new(0, 0, 8.0, 8.0));
        assert_eq!(app.song_region_selection, None);
    }
}
