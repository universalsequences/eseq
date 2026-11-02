use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use super::*;
use crate::app::test_rig::Rig;

/// A mono 16-bit WAV of `frames` frames at `rate`, each sample unique so
/// two files never hash alike.
fn write_wav(dir: &Path, name: &str, frames: usize, rate: u32) -> PathBuf {
    let path = dir.join(name);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for n in 0..frames {
        writer.write_sample(((n % 1000) as i16 - 500) * 20).unwrap();
    }
    writer.finalize().unwrap();
    path
}

struct Fixture {
    rig: Rig,
    dir: tempfile::TempDir,
    track: usize,
}

fn fixture() -> Fixture {
    let mut rig = Rig::new();
    let dir = tempfile::tempdir().unwrap();
    let app = rig.app();
    app.set_audio_store_dir(dir.path().join("store"));
    app.state.transport.bpm.store(120, Ordering::Relaxed);
    let track = app.add_audio_track().unwrap();
    Fixture { rig, dir, track }
}

fn lane(app: &App, track: usize) -> Vec<AudioRegion> {
    app.state
        .with_committed_arrangement(|arr| arr.map(|arr| arr.audio_lane(track).to_vec()))
        .unwrap_or_default()
}

fn player_slot(app: &App, track: usize) -> usize {
    app.graph.track_node_ids[track].audio_player.expect("audio track has a player").slot
}

#[test]
fn an_audio_track_has_a_player_and_an_empty_clip_lane() {
    let mut f = fixture();
    let app = f.rig.app();
    assert!(app.is_audio_track(f.track));
    assert_eq!(app.graph.track_instrument_types[f.track], InstrumentType::Audio);
    player_slot(app, f.track);
    let clips = app
        .require_arrangement()
        .map(|arr| arr.track_lanes[f.track].len())
        .unwrap();
    assert_eq!(clips, 0);
}

#[test]
fn dropping_samples_lays_regions_end_to_end_in_one_undo_step() {
    let mut f = fixture();
    let one_second = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let half_second = write_wav(f.dir.path(), "b.wav", 22_050, 44_100);
    let app = f.rig.app();
    let ids = app.audio_regions_drop(f.track, 4.0, &[one_second, half_second]).unwrap();
    assert_eq!(ids.len(), 2);

    // 120 bpm: one second is two beats, half a second one beat.
    let regions = lane(app, f.track);
    let spans: Vec<_> = regions.iter().map(|r| (r.id, r.start_beat, r.end_beat, r.source_offset)).collect();
    assert_eq!(spans, vec![(ids[0], 4.0, 6.0, 0), (ids[1], 6.0, 7.0, 0)]);

    // Both files were copied into the store under their content hash.
    let stored: Vec<_> = std::fs::read_dir(f.dir.path().join("store"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(stored.len(), 2, "{stored:?}");
    assert!(stored.iter().all(|name| name.len() == 64 + 4 && name.ends_with(".wav")));

    crate::app::edit::undo(app);
    assert!(lane(app, f.track).is_empty(), "one undo removes the whole drop");
    crate::app::edit::redo(app);
    assert_eq!(lane(app, f.track).len(), 2);
}

#[test]
fn dropping_on_a_non_audio_track_is_refused() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 4_800, 48_000);
    let app = f.rig.app();
    let midi = app.graph_controller().add_empty_track().unwrap();
    let error = app.audio_regions_drop(midi, 0.0, &[wav]).unwrap_err();
    assert!(error.contains("not an audio track"), "{error}");
}

#[test]
fn sync_publishes_the_regions_and_follows_edits() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let app = f.rig.app();
    let slot = player_slot(app, f.track);
    app.sync_audio_track_playback();
    assert!(app.audio_players.published(slot).is_some_and(|p| p.regions.is_empty()));

    let id = app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    app.sync_audio_track_playback();
    let published = app.audio_players.published(slot).unwrap();
    assert_eq!(published.regions.len(), 1);
    assert_eq!((published.regions[0].start_beat, published.regions[0].end_beat), (0.0, 2.0));
    assert_eq!(published.regions[0].pcm.frames(), 48_000);

    app.audio_region_set_params(id, None, None, None, Some(true)).unwrap();
    app.sync_audio_track_playback();
    assert!(app.audio_players.published(slot).unwrap().regions.is_empty(), "muted regions are silent");
}

#[test]
fn moving_resizing_splitting_and_deleting_regions() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000 * 4, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];

    app.audio_region_move(id, 8.0, None).unwrap();
    assert_eq!(lane(app, f.track)[0].start_beat, 8.0);

    // Trim the left edge by one beat: the audio under the rest stays put.
    app.audio_region_resize(id, 9.0, 16.0).unwrap();
    let region = lane(app, f.track)[0];
    assert_eq!((region.start_beat, region.end_beat, region.source_offset), (9.0, 16.0, 24_000));

    let tail = app.audio_region_split(id, 12.0).unwrap();
    let regions = lane(app, f.track);
    assert_eq!(regions.len(), 2);
    assert_eq!((regions[1].id, regions[1].start_beat, regions[1].source_offset), (tail, 12.0, 96_000));

    let copy = app.audio_region_duplicate(tail).unwrap();
    let regions = lane(app, f.track);
    assert_eq!(regions.len(), 3);
    assert_eq!((regions[2].id, regions[2].start_beat), (copy, 16.0));

    app.audio_regions_delete(&[id, copy]).unwrap();
    let ids: Vec<_> = lane(app, f.track).iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![tail]);
}

#[test]
fn a_moved_region_truncates_what_it_lands_on() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000 * 4, 48_000);
    let app = f.rig.app();
    let ids = app.audio_regions_drop(f.track, 0.0, &[wav.clone(), wav]).unwrap();
    // Two 8-beat regions at [0, 8) and [8, 16); move the second to beat 4.
    app.audio_region_move(ids[1], 4.0, None).unwrap();
    let spans: Vec<_> = lane(app, f.track).iter().map(|r| (r.id, r.start_beat, r.end_beat)).collect();
    assert_eq!(spans, vec![(ids[0], 0.0, 4.0), (ids[1], 4.0, 12.0)]);
}

#[test]
fn audio_tracks_and_regions_survive_save_and_load() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 2.0, &[wav]).unwrap()[0];
    app.audio_region_set_params(id, Some(-6.0), Some(0.01), None, None).unwrap();
    let saved = app.capture_project("audio-roundtrip").unwrap();
    assert!(matches!(saved.tracks[f.track].kind, crate::project::ProjectTrackKind::Audio));
    let before = lane(app, f.track);

    f.rig.reload("audio-roundtrip", saved);
    let app = f.rig.app();
    assert!(app.is_audio_track(f.track));
    assert_eq!(lane(app, f.track), before);
    let slot = player_slot(app, f.track);
    let published = app.audio_players.published(slot).expect("load publishes");
    assert_eq!(published.regions.len(), 1, "the source decoded from the store");
    assert!((published.regions[0].gain - 0.501).abs() < 1e-3);
}

#[test]
fn deleting_an_audio_track_drops_its_lane_and_undo_brings_both_back() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let app = f.rig.app();
    app.graph_controller().add_empty_track().unwrap();
    app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap();
    let before = lane(app, f.track);

    app.delete_track_recorded(f.track).unwrap();
    assert!(!app.is_audio_track(f.track));
    assert!(app
        .state
        .with_committed_arrangement(|arr| arr.unwrap().audio_lanes.iter().all(Vec::is_empty)));

    crate::app::edit::undo(app);
    assert!(app.is_audio_track(f.track), "undo restores an audio track, not an empty one");
    assert_eq!(lane(app, f.track), before);
    app.sync_audio_track_playback();
    let slot = player_slot(app, f.track);
    assert_eq!(app.audio_players.published(slot).unwrap().regions.len(), 1);
}

#[test]
fn the_clip_window_slips_the_audio_under_a_fixed_start() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000 * 4, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 4.0, &[wav]).unwrap()[0];
    // Play 1.5 s from 0.5 s in: the region stays at beat 4 and lasts 3 beats.
    app.audio_region_set_window(id, 0.5, 1.5).unwrap();
    let region = lane(app, f.track)[0];
    assert_eq!((region.start_beat, region.end_beat, region.source_offset), (4.0, 7.0, 24_000));
    let error = app.audio_region_set_window(id, 5.0, 1.0).unwrap_err();
    assert!(error.contains("past the end"), "{error}");
}

#[test]
fn a_live_gain_drag_is_one_undo_step() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 4_800, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    for gain in [-1.0, -2.0, -3.0] {
        app.audio_region_set_params_keyed(id, Some(gain), None, None, None, true).unwrap();
    }
    assert_eq!(lane(app, f.track)[0].gain_db, -3.0);
    crate::app::edit::undo(app);
    assert_eq!(lane(app, f.track)[0].gain_db, 0.0, "the whole drag undoes at once");
    assert_eq!(lane(app, f.track).len(), 1, "and the drop before it stays");
}

#[test]
fn pasting_regions_keeps_their_spacing_in_one_undo_step() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let app = f.rig.app();
    let other = app.add_audio_track().unwrap();
    let ids = app.audio_regions_drop(f.track, 0.0, &[wav.clone()]).unwrap();
    let second = app.audio_regions_drop(f.track, 8.0, &[wav]).unwrap();
    let both = [ids[0], second[0]];
    let pasted = app.audio_regions_paste(&both, 16.0, Some(other)).unwrap();
    let starts: Vec<_> = lane(app, other).iter().map(|r| (r.id, r.start_beat)).collect();
    assert_eq!(starts, vec![(pasted[0], 16.0), (pasted[1], 24.0)]);
    crate::app::edit::undo(app);
    assert!(lane(app, other).is_empty());
}

#[test]
fn region_delete_cuts_a_hole_and_duplicate_ripples_the_audio_lane() {
    use crate::app::song_region::SongRegionSelection;
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000 * 4, 48_000);
    let app = f.rig.app();
    app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap();
    // One 8-beat region; delete beats 2-4 out of it.
    app.set_song_region(SongRegionSelection::new(f.track, f.track, 2.0, 4.0));
    app.song_region_delete().unwrap();
    let spans: Vec<_> = lane(app, f.track).iter().map(|r| (r.start_beat, r.end_beat, r.source_offset)).collect();
    assert_eq!(spans, vec![(0.0, 2.0, 0), (4.0, 8.0, 96_000)]);

    // Duplicate beats 4-6 in place: the slice repeats at 6 and what followed
    // slides right by two beats, still playing the same audio.
    app.set_song_region(SongRegionSelection::new(f.track, f.track, 4.0, 6.0));
    app.song_region_duplicate().unwrap();
    let spans: Vec<_> = lane(app, f.track).iter().map(|r| (r.start_beat, r.end_beat, r.source_offset)).collect();
    assert_eq!(
        spans,
        vec![(0.0, 2.0, 0), (4.0, 6.0, 96_000), (6.0, 8.0, 96_000), (8.0, 10.0, 144_000)]
    );
}

/// An arrangement holding only audio is still an arrangement: Play plays it
/// instead of latching the session scene over every lane (which dims them).
#[test]
fn play_with_only_audio_ahead_does_not_latch_the_scene() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let app = f.rig.app();
    app.graph_controller().add_empty_track().unwrap();
    app.audio_regions_drop(f.track, 4.0, &[wav]).unwrap();
    app.set_arrangement_cursor(2.0, 0);
    app.song_transport_play(false).unwrap();
    assert_eq!(app.state.song_manual_latch_mask(), 0, "no lane is latched over the arrangement");
    app.song_transport_stop().unwrap();
}

#[test]
fn a_looping_region_keeps_its_window_while_it_grows() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000 * 4, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    app.audio_region_set_window(id, 1.0, 1.0).unwrap();
    app.audio_region_set_loop(id, true).unwrap();
    let region = lane(app, f.track)[0];
    assert_eq!((region.loop_start, region.loop_end), (48_000, 96_000));
    // Growing it past the window repeats the loop instead of running on.
    app.audio_region_resize(id, 0.0, 16.0).unwrap();
    // The clip window now edits the loop and leaves the length alone.
    app.audio_region_set_window(id, 0.5, 0.5).unwrap();
    let region = lane(app, f.track)[0];
    assert_eq!((region.loop_start, region.loop_end, region.end_beat), (24_000, 48_000, 16.0));
    app.sync_audio_track_playback();
    let slot = player_slot(app, f.track);
    let published = app.audio_players.published(slot).unwrap();
    assert_eq!(published.regions[0].loop_window, Some((24_000.0, 48_000.0)));
}

#[test]
fn pitch_speeds_playback_and_shortens_a_set_window() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000 * 4, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    app.audio_region_set_pitch(id, 12.0, false).unwrap();
    // An octave up plays a one-second window in half a second: one beat.
    app.audio_region_set_window(id, 0.0, 1.0).unwrap();
    assert_eq!(lane(app, f.track)[0].end_beat, 1.0);
    app.sync_audio_track_playback();
    let slot = player_slot(app, f.track);
    assert!((app.audio_players.published(slot).unwrap().regions[0].step - 2.0).abs() < 1e-9);
}
