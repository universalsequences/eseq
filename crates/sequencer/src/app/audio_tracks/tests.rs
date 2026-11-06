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

    app.audio_region_update(id, AudioRegionPatch { muted: Some(true), ..Default::default() }, false).unwrap();
    app.sync_audio_track_playback();
    assert!(app.audio_players.published(slot).unwrap().regions.is_empty(), "muted regions are silent");
}

#[test]
fn moving_resizing_splitting_and_deleting_regions() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000 * 4, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];

    app.audio_region_place(id, 8.0, None, Place::Move).unwrap();
    assert_eq!(lane(app, f.track)[0].start_beat, 8.0);

    // Trim the left edge by one beat: the audio under the rest stays put.
    app.audio_region_resize(id, 9.0, 16.0).unwrap();
    let region = lane(app, f.track)[0].clone();
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
    app.audio_region_place(ids[1], 4.0, None, Place::Move).unwrap();
    let spans: Vec<_> = lane(app, f.track).iter().map(|r| (r.id, r.start_beat, r.end_beat)).collect();
    assert_eq!(spans, vec![(ids[0], 0.0, 4.0), (ids[1], 4.0, 12.0)]);
}

#[test]
fn audio_tracks_and_regions_survive_save_and_load() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 2.0, &[wav]).unwrap()[0];
    app.audio_region_update(
        id,
        AudioRegionPatch { gain_db: Some(-6.0), fade_in_seconds: Some(0.01), ..Default::default() },
        false,
    )
    .unwrap();
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
    app.audio_region_set_window(id, 0.5, 1.5, false).unwrap();
    let region = lane(app, f.track)[0].clone();
    assert_eq!((region.start_beat, region.end_beat, region.source_offset), (4.0, 7.0, 24_000));
    let error = app.audio_region_set_window(id, 5.0, 1.0, false).unwrap_err();
    assert!(error.contains("past the end"), "{error}");
}

#[test]
fn a_live_gain_drag_is_one_undo_step() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 4_800, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    for gain in [-1.0, -2.0, -3.0] {
        app.audio_region_update(id, AudioRegionPatch { gain_db: Some(gain), ..Default::default() }, true).unwrap();
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
    app.audio_region_set_window(id, 1.0, 1.0, false).unwrap();
    app.audio_region_set_loop(id, true).unwrap();
    let region = lane(app, f.track)[0].clone();
    assert_eq!((region.loop_start, region.loop_end), (48_000, 96_000));
    // Growing it past the window repeats the loop instead of running on.
    app.audio_region_resize(id, 0.0, 16.0).unwrap();
    // The clip window now edits the loop and leaves the length alone.
    app.audio_region_set_window(id, 0.5, 0.5, false).unwrap();
    let region = lane(app, f.track)[0].clone();
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
    app.audio_region_update(id, AudioRegionPatch { transpose: Some(12.0), ..Default::default() }, false).unwrap();
    // An octave up plays a one-second window in half a second: one beat.
    app.audio_region_set_window(id, 0.0, 1.0, false).unwrap();
    assert_eq!(lane(app, f.track)[0].end_beat, 1.0);
    app.sync_audio_track_playback();
    let slot = player_slot(app, f.track);
    assert!((app.audio_players.published(slot).unwrap().regions[0].step - 2.0).abs() < 1e-9);
}

/// The clip editor's range copies any stretch of the source, including audio
/// the region does not play: here the region plays 0.5-1.0 s and the copy
/// takes 2.0-3.0 s, pasting as a 2-beat region (120 bpm) from 2.0 s in.
#[test]
fn a_source_range_outside_the_window_copies_and_pastes() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000 * 4, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    app.audio_region_set_window(id, 0.5, 0.5, false).unwrap();
    let clipboard = app.audio_source_range_copy(id, 2.0, 3.0).unwrap();
    assert_eq!(clipboard.len_beats, 2.0);
    app.song_region_paste(&clipboard, 4.0).unwrap();
    let spans: Vec<_> =
        lane(app, f.track).iter().map(|r| (r.start_beat, r.end_beat, r.source_offset)).collect();
    assert_eq!(spans, vec![(0.0, 1.0, 24_000), (4.0, 6.0, 96_000)]);
    assert!(app.audio_source_range_copy(id, 3.0, 3.0).is_err());
}

/// A scene launch overrides the session tracks but never an audio track: its
/// only content is the timeline, so its lane stays unlatched (undimmed). A
/// project of only audio tracks latches nothing, so Back to Arrangement
/// stays off.
#[test]
fn a_scene_launch_never_latches_an_audio_track() {
    use crate::quantized_launch::PatternLaunchTarget;
    let mut f = fixture();
    let app = f.rig.app();
    app.sync_audio_track_playback();
    app.apply_pattern_launch(&PatternLaunchTarget::Scene { scene: 0 }).unwrap();
    assert_eq!(app.state.song_manual_latch_mask(), 0, "only an audio track: nothing latched");
    assert!(!app.state.song_scene_latch(), "and no scene latch");

    let session = app.graph_controller().add_empty_track().unwrap();
    app.sync_audio_track_playback();
    app.apply_pattern_launch(&PatternLaunchTarget::Scene { scene: 0 }).unwrap();
    let mask = app.state.song_manual_latch_mask();
    assert_eq!(mask >> f.track & 1, 0, "the audio track stays on the timeline");
    assert_eq!(mask >> session & 1, 1, "the session track latches");
    assert!(app.state.song_scene_latch());
}

/// An audio track has ONE mixer state, not one per scene: a volume set while
/// stopped survives Play and Stop, even when the arrangement plays a scene
/// whose cell on the track remembers another volume.
#[test]
fn an_audio_track_volume_set_while_stopped_survives_play_and_stop() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let app = f.rig.app();
    app.set_arrangement_view_visible(true);
    app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap();
    app.sync_audio_track_playback();
    // Scene 0 gets a cell on the audio track remembering volume 0.9.
    app.state.pattern.track_params[f.track].set_volume(0.9);
    assert!(app.state.save_current_pattern_snapshot(
        app.tracks.len(),
        &app.graph.track_buffer_ids,
        &app.graph.track_sample_rates,
        &app.tracks,
        &app.graph.track_instrument_types,
    ));
    app.arr_scene_event_insert(0.0, 0).unwrap();

    for view in [true, false] {
        app.set_arrangement_view_visible(view);
        crate::app::try_apply_command(app, crate::app::AppCommand::SetTrackVolume { track: f.track, value: 0.3 })
            .unwrap();
        app.song_transport_play(false).unwrap();
        app.sync_track_sound_bindings();
        assert_eq!(
            app.state.pattern.track_params[f.track].get_volume(),
            0.3,
            "Play keeps it (arrangement view {view})"
        );
        app.song_transport_stop().unwrap();
        app.sync_track_sound_bindings();
        assert_eq!(
            app.state.pattern.track_params[f.track].get_volume(),
            0.3,
            "Stop keeps it (arrangement view {view})"
        );
    }
}

/// After a scene launch (which latches session tracks), a volume set while
/// stopped on an audio track still survives Play: the audio track never
/// latched, so nothing re-launches a scene cell's mixer over it.
#[test]
fn an_audio_track_volume_survives_play_after_a_scene_launch() {
    use crate::quantized_launch::PatternLaunchTarget;
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let app = f.rig.app();
    app.set_arrangement_view_visible(true);
    app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap();
    app.sync_audio_track_playback();
    app.state.pattern.track_params[f.track].set_volume(0.9);
    assert!(app.state.save_current_pattern_snapshot(
        app.tracks.len(),
        &app.graph.track_buffer_ids,
        &app.graph.track_sample_rates,
        &app.tracks,
        &app.graph.track_instrument_types,
    ));
    app.arr_scene_event_insert(0.0, 0).unwrap();
    app.apply_pattern_launch(&PatternLaunchTarget::Scene { scene: 0 }).unwrap();
    app.sync_track_sound_bindings();
    crate::app::try_apply_command(app, crate::app::AppCommand::SetTrackVolume { track: f.track, value: 0.3 })
        .unwrap();
    app.song_transport_play(false).unwrap();
    app.sync_track_sound_bindings();
    assert_eq!(app.state.pattern.track_params[f.track].get_volume(), 0.3, "Play keeps it");
    app.song_transport_stop().unwrap();
    app.sync_track_sound_bindings();
    assert_eq!(app.state.pattern.track_params[f.track].get_volume(), 0.3, "Stop keeps it");
}

/// eseq-a921: an effect parameter locked on one region is published with
/// the region (its graph target and the track value it reverts to), the
/// other region locks nothing, undo removes the lock, and a copy carries it.
#[test]
fn a_region_lock_is_published_with_its_region_and_undoes() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let app = f.rig.app();
    let slot = app.add_builtin_effect_sync(f.track, "Reverb").unwrap();
    let ids = app.audio_regions_drop(f.track, 0.0, &[wav.clone(), wav]).unwrap();
    let base = app.state.pattern.effect_chains[f.track][slot].defaults.get(0);
    let locked = if base > 0.5 { 0.1 } else { 0.9 };
    app.audio_region_set_effect_locks(f.track, &ids[..1], slot, &[(0, Some(locked))], false).unwrap();
    assert_eq!(app.audio_region_effect_locks(f.track, slot, 0), vec![(ids[0], locked)]);
    app.sync_audio_track_playback();
    let player = player_slot(app, f.track);
    let playback = app.audio_players.published(player).expect("published");
    assert_eq!(playback.lock_params.len(), 1);
    assert_eq!(playback.lock_params[0].base, base, "reverts to the track value");
    assert_eq!(playback.regions[0].locks, vec![(0, locked)]);
    assert!(playback.regions[1].locks.is_empty());

    // A track-value edit republishes the value the lock reverts to.
    app.state.pattern.effect_chains[f.track][slot].defaults.set(0, 0.42);
    app.sync_audio_track_playback();
    let playback = app.audio_players.published(player).unwrap();
    assert_eq!(playback.lock_params[0].base, 0.42);

    // Undo clears it; the parameter stays in the snapshot so it reverts.
    crate::app::edit::undo(app);
    assert!(app.audio_region_effect_locks(f.track, slot, 0).is_empty());
    app.sync_audio_track_playback();
    let playback = app.audio_players.published(player).unwrap();
    assert!(playback.regions.iter().all(|region| region.locks.is_empty()));
    assert_eq!(playback.lock_params.len(), 1, "an earlier lock still reverts");

    // Copy/paste carries the lock.
    crate::app::edit::redo(app);
    let copied = app.audio_region_place(ids[0], 8.0, None, Place::Copy).unwrap();
    assert!(app.audio_region_effect_locks(f.track, slot, 0).contains(&(copied, locked)));
}

/// One 4-second file split at beat 4 (120 bpm): two abutting regions with
/// audio on both sides of the cut, as chopping a recording leaves them.
fn chopped(f: &mut Fixture) -> (ClipId, ClipId) {
    let wav = write_wav(f.dir.path(), "long.wav", 4 * 48_000, 48_000);
    let app = f.rig.app();
    let head = app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    let tail = app.audio_region_split(head, 4.0).unwrap();
    (head, tail)
}

#[test]
fn fade_handles_set_length_and_shape_without_overlapping() {
    let mut f = fixture();
    let (head, _) = chopped(&mut f);
    let app = f.rig.app();
    // One beat (half a second) of fade-in, then a curve whose midpoint sits
    // at three quarters.
    app.audio_region_fade(head, FadeHandle::In, 1.0, true).unwrap();
    app.audio_region_fade(head, FadeHandle::InCurve, 0.75, true).unwrap();
    let region = lane(app, f.track)[0].clone();
    assert_eq!(region.fade_in_seconds, 0.5);
    let power = fade_curve_power(region.fade_in_curve);
    assert!((crate::sequencer::fade_shape(0.5, power) - 0.75).abs() < 1e-3);
    // A fade-out can only take what the fade-in leaves (the head is 2 s).
    app.audio_region_fade(head, FadeHandle::Out, -10.0, true).unwrap();
    assert_eq!(lane(app, f.track)[0].fade_out_seconds, 1.5);
    crate::app::edit::undo(app);
    let undone = lane(app, f.track)[0].clone();
    assert_eq!((undone.fade_in_seconds, undone.fade_out_seconds), (0.0, 0.0), "the whole drag is one step");
}

#[test]
fn a_fade_in_dragged_over_the_region_before_crossfades_into_it() {
    let mut f = fixture();
    let (head, tail) = chopped(&mut f);
    let app = f.rig.app();
    // Dragged a beat left of the tail's start: a half-second crossfade made
    // of the tail's own audio from before the cut.
    app.audio_region_fade(tail, FadeHandle::In, 3.0, false).unwrap();
    let regions = lane(app, f.track);
    assert_eq!((regions[1].crossfade_pre_seconds, regions[1].fade_in_seconds), (0.5, 0.0));
    let views = app.audio_region_views();
    let views = &views[f.track];
    assert_eq!(views[1].crossfade, Some((1.0, 0.0)), "in beats");
    assert_eq!(views[0].crossfade_next_pre, Some(1.0), "the head shows where it starts");

    // The player hears it: the tail pre-rolls a second of host frames.
    app.sync_audio_track_playback();
    let published = app.audio_players.published(player_slot(app, f.track)).unwrap();
    assert_eq!((published.regions[1].xfade_pre, published.regions[1].xfade_post), (24_000.0, 0.0));

    // The crossfade's start handle (on the head) widens it, but never past
    // the audio before the tail's start.
    app.audio_region_fade(head, FadeHandle::CrossfadeStart, -50.0, false).unwrap();
    assert_eq!(lane(app, f.track)[1].crossfade_pre_seconds, 2.0, "all of the head");

    // Dragging the head's fade-out right past its end crossfades over the
    // head's own audio past the cut instead.
    app.audio_region_fade(head, FadeHandle::Out, 4.5, false).unwrap();
    let regions = lane(app, f.track);
    assert_eq!((regions[1].crossfade_pre_seconds, regions[1].crossfade_post_seconds), (0.0, 0.25));
}

#[test]
fn whole_files_side_by_side_have_nothing_to_crossfade_with() {
    let mut f = fixture();
    let a = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let b = write_wav(f.dir.path(), "b.wav", 48_001, 48_000);
    let app = f.rig.app();
    let ids = app.audio_regions_drop(f.track, 0.0, &[a, b]).unwrap();
    // The second file starts at its first frame: no audio before it.
    app.audio_region_fade(ids[1], FadeHandle::In, 1.0, false).unwrap();
    let regions = lane(app, f.track);
    assert_eq!((regions[1].crossfade_pre_seconds, regions[1].fade_in_seconds), (0.0, 0.0));
    assert_eq!(app.audio_region_views()[f.track][1].crossfade, None);
}

#[test]
fn a_split_leaves_the_fade_in_on_the_head_and_the_fade_out_on_the_tail() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "long.wav", 4 * 48_000, 48_000);
    let app = f.rig.app();
    let id = app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    app.audio_region_fade(id, FadeHandle::In, 1.0, false).unwrap();
    app.audio_region_fade(id, FadeHandle::Out, 7.0, false).unwrap();
    app.audio_region_split(id, 4.0).unwrap();
    let regions = lane(app, f.track);
    assert_eq!((regions[0].fade_in_seconds, regions[0].fade_out_seconds), (0.5, 0.0));
    assert_eq!((regions[1].fade_in_seconds, regions[1].fade_out_seconds), (0.0, 0.5));
}

#[test]
fn a_fade_in_dragged_left_crossfades_even_when_its_file_starts_at_the_cut() {
    // The left region is a trimmed file (audio past its end), the right one
    // a whole file starting at its first frame: nothing before its start.
    let mut f = fixture();
    let long = write_wav(f.dir.path(), "long.wav", 4 * 48_000, 48_000);
    let whole = write_wav(f.dir.path(), "whole.wav", 2 * 48_000, 48_000);
    let app = f.rig.app();
    let left = app.audio_regions_drop(f.track, 0.0, &[long]).unwrap()[0];
    app.audio_region_resize(left, 0.0, 4.0).unwrap();
    let right = app.audio_regions_drop(f.track, 4.0, &[whole]).unwrap()[0];
    // A beat left of the right region's start: the half-second crossfade
    // runs after the cut, over the left region's audio past its end.
    app.audio_region_fade(right, FadeHandle::In, 3.0, false).unwrap();
    let regions = lane(app, f.track);
    assert_eq!((regions[1].crossfade_pre_seconds, regions[1].crossfade_post_seconds), (0.0, 0.5));
    assert_eq!(app.audio_region_views()[f.track][1].crossfade, Some((0.0, 1.0)));
}

#[test]
fn a_multi_selection_moves_together_in_one_undo_step() {
    let mut f = fixture();
    let a = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let b = write_wav(f.dir.path(), "b.wav", 48_001, 48_000);
    let c = write_wav(f.dir.path(), "c.wav", 48_002, 48_000);
    let app = f.rig.app();
    // Three 2-beat regions end to end at 0, 2 and 4.
    let ids = app.audio_regions_drop(f.track, 0.0, &[a, b, c]).unwrap();
    // The first two move a beat right: they keep their spacing, and only
    // the unselected third one is truncated (its head, under the second).
    app.audio_regions_move(&ids[..2], 1.0).unwrap();
    // (The files differ by a frame so they store apart: round it away.)
    let round = |beat: f64| (beat * 1000.0).round() / 1000.0;
    let spans: Vec<_> =
        lane(app, f.track).iter().map(|r| (r.id, round(r.start_beat), round(r.end_beat))).collect();
    assert_eq!(spans, vec![(ids[0], 1.0, 3.0), (ids[1], 3.0, 5.0), (ids[2], 5.0, 6.0)]);
    crate::app::edit::undo(app);
    let spans: Vec<_> = lane(app, f.track).iter().map(|r| (round(r.start_beat), round(r.end_beat))).collect();
    assert_eq!(spans, vec![(0.0, 2.0), (2.0, 4.0), (4.0, 6.0)], "one undo puts all three back");
}

#[test]
fn a_fade_dragged_toward_a_neighbour_with_nothing_to_blend_keeps_its_fade() {
    let mut f = fixture();
    let a = write_wav(f.dir.path(), "a.wav", 48_000, 48_000);
    let b = write_wav(f.dir.path(), "b.wav", 48_001, 48_000);
    let app = f.rig.app();
    let ids = app.audio_regions_drop(f.track, 0.0, &[a, b]).unwrap();
    app.audio_region_fade(ids[0], FadeHandle::Out, 1.5, false).unwrap();
    // Neither whole file has audio past their shared edge: dragging the
    // second one's fade-in left over the first is no crossfade, and leaves
    // the first one's fade-out alone.
    app.audio_region_fade(ids[1], FadeHandle::In, 1.8, false).unwrap();
    let regions = lane(app, f.track);
    assert_eq!(regions[0].fade_out_seconds, 0.25);
    assert_eq!((regions[1].crossfade_pre_seconds, regions[1].crossfade_post_seconds), (0.0, 0.0));
}
