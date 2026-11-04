//! Audio lanes in the arrangement view (docs/audio-tracks-spec.md §8): a
//! dropped sample draws as a waveform region, gestures on the lane edit
//! regions, and a double-click opens the clip editor in the lower panel.

use super::views::{distro, widgets_with_prop};
use super::*;

/// A 16-bit mono WAV of `frames` frames at 48 kHz.
fn write_wav(dir: &std::path::Path, name: &str, frames: usize) -> std::path::PathBuf {
    let path = dir.join(name);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for n in 0..frames {
        writer.write_sample(((n % 480) as i16 - 240) * 50).unwrap();
    }
    writer.finalize().unwrap();
    path
}

struct Fixture {
    h: Harness,
    dir: tempfile::TempDir,
    track: usize,
}

fn fixture() -> Fixture {
    let mut h = distro();
    let dir = tempfile::tempdir().unwrap();
    h.app.set_audio_store_dir(dir.path().join("store"));
    h.app.state.transport.bpm.store(120, Ordering::Relaxed);
    let track = h.app.add_audio_track().unwrap();
    audio_settle(&mut h);
    Fixture { h, dir, track }
}

impl Harness {
    fn lane_items(&self, track: usize) -> Vec<HashMap<String, Value>> {
        let mut lanes = Vec::new();
        widgets_with_prop(&self.buffer_tree("*arrangement*").0, "lane-key", &mut lanes);
        let lane = lanes
            .into_iter()
            .find(|lane| lane.get("lane-key") == Some(&Value::Number(track as f64)))
            .unwrap_or_else(|| panic!("track lane {track}"));
        match lane.get("items") {
            Some(Value::List(items)) => items
                .iter()
                .map(|item| match &*item.borrow() {
                    Value::Map(map) => {
                        map.iter().map(|(k, v)| (k.clone(), v.borrow().clone())).collect()
                    }
                    other => panic!("item is not a map: {other:?}"),
                })
                .collect(),
            other => panic!("lane items: {other:?}"),
        }
    }

    fn has_widget_key(&self, buffer: &str, key: &str) -> bool {
        let mut keyed = Vec::new();
        widgets_with_prop(&self.buffer_tree(buffer).0, "key", &mut keyed);
        keyed.iter().any(|widget| widget.get("key") == Some(&Value::String(key.to_string())))
    }
}

/// Apply what the view queued, push the kinds and re-render, with the
/// per-frame audio-track sync the event loop runs.
fn audio_settle(h: &mut Harness) {
    h.app.sync_audio_track_playback();
    h.drain();
    h.sync();
    h.show_all();
}

fn number(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(n)) => *n,
        other => panic!("not a number: {other:?}"),
    }
}

#[test]
fn a_sample_dropped_on_an_audio_lane_draws_as_a_waveform_region() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 48_000);
    // Drop at the lane's left edge (sx -1): the view starts at beat 0.
    f.h.eval(&format!(
        r#"(eseq.arrangement/audio-drop {} (dict :drag-type "sample" :payload (dict :path "{}") :sx -1))"#,
        f.track,
        wav.display()
    ));
    audio_settle(&mut f.h);

    let items = f.h.lane_items(f.track);
    assert_eq!(items.len(), 1, "{items:?}");
    let item = &items[0];
    assert_eq!(item.get("kind"), Some(&Value::Keyword("audio".to_string())));
    // One second at 120 bpm is two beats.
    assert_eq!((number(item.get("start")), number(item.get("end"))), (0.0, 2.0));
    let Some(Value::Map(content)) = item.get("content") else {
        panic!("a loaded region carries waveform content: {item:?}");
    };
    let Value::String(key) = content["sample"].borrow().clone() else {
        panic!("content names its sample");
    };
    assert!(
        eseqlisp::audio::sample::get_registered_sample(&key).is_some(),
        "the waveform is registered under {key}"
    );
}

#[test]
fn double_clicking_a_region_opens_the_clip_editor() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 24_000);
    let id = f.h.app.audio_regions_drop(f.track, 4.0, &[wav]).unwrap()[0];
    audio_settle(&mut f.h);
    f.h.eval(&format!(
        "(eseq.arrangement/audio-lane-action {} (dict :type :double-click-item :ids (list {}) :time 4))",
        f.track, id.0
    ));
    audio_settle(&mut f.h);
    audio_settle(&mut f.h);
    assert!(
        f.h.has_widget_key("*piano-roll*", "audio-clip-editor"),
        "the lower panel shows the audio clip editor"
    );
    assert!(f.h.has_widget_key("*piano-roll*", "audio-clip-waveform"));
    let bound = f.h.eval("(let ((c eseq.audio-clip/audio-clip)) c.region)");
    assert_eq!(bound, Value::Number(id.0 as f64));
}

#[test]
fn the_lane_deletes_copies_and_pastes_regions() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 24_000);
    let id = f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    audio_settle(&mut f.h);
    let act = |h: &mut Harness, event: String| {
        h.eval(&format!("(eseq.arrangement/audio-lane-action {} {event})", f.track));
        audio_settle(h);
    };
    act(&mut f.h, format!("(dict :type :select :ids (list {}) :time 0)", id.0));
    act(&mut f.h, "(dict :type :copy-items)".to_string());
    act(&mut f.h, "(dict :type :paste-items :time 8)".to_string());
    let starts: Vec<f64> =
        f.h.lane_items(f.track).iter().map(|item| number(item.get("start"))).collect();
    assert_eq!(starts, vec![0.0, 8.0]);

    act(&mut f.h, format!("(dict :type :delete-items :ids (list {}))", id.0));
    let starts: Vec<f64> =
        f.h.lane_items(f.track).iter().map(|item| number(item.get("start"))).collect();
    assert_eq!(starts, vec![8.0]);
}

#[test]
fn moving_and_trimming_on_the_lane_commit_once_on_release() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
    let id = f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    audio_settle(&mut f.h);
    let act = |h: &mut Harness, event: String| {
        h.eval(&format!("(eseq.arrangement/audio-lane-action {} {event})", f.track));
        audio_settle(h);
    };
    act(&mut f.h, format!("(dict :type :move-items-absolute :anchor-id {} :start 4)", id.0));
    act(&mut f.h, format!("(dict :type :move-items-absolute :anchor-id {} :start 6)", id.0));
    act(&mut f.h, "(dict :type :finish-move-items)".to_string());
    act(&mut f.h, format!("(dict :type :resize-item-absolute :id {} :edge :start :time 7)", id.0));
    act(&mut f.h, "(dict :type :finish-resize-items)".to_string());
    let item = f.h.lane_items(f.track).remove(0);
    assert_eq!((number(item.get("start")), number(item.get("end"))), (7.0, 10.0));
    let undo_len = f.h.app.history.undo_len();
    assert!(undo_len >= 3, "drop, move and trim are one entry each: {undo_len}");
}

/// A press in a region's body is a region surface (the user's report): it
/// parks the transport-start cursor where it lands and selects nothing, so
/// the drag that follows sweeps a time selection.
#[test]
fn a_body_press_places_the_cursor_and_selects_nothing() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
    let id = f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    audio_settle(&mut f.h);
    f.h.eval(&format!(
        "(eseq.arrangement/audio-lane-action {} (dict :type :clear-selection :ids (list {}) :time 1.5))",
        f.track, id.0
    ));
    audio_settle(&mut f.h);
    assert_eq!(f.h.eval("(let ((v eseq.arrangement/arr-view)) v.cursor-time)"), Value::Number(1.5));
    assert_eq!(f.h.eval("(let ((v eseq.arrangement/arr-select)) v.clip)"), Value::Number(-1.0));
    assert_eq!(f.h.app.arrangement_cursor_beat, 1.5, "the transport starts from the press");
}

/// Sweep part of a region, copy, paste: the pasted region plays exactly the
/// swept slice of the audio.
#[test]
fn a_swept_slice_of_a_region_copies_and_pastes_its_own_audio() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
    f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap();
    audio_settle(&mut f.h);
    let act = |h: &mut Harness, event: String| {
        h.eval(&format!("(eseq.arrangement/audio-lane-action {} {event})", f.track));
        audio_settle(h);
    };
    // The region spans beats 0-4; sweep its second half.
    act(&mut f.h, "(dict :type :finish-marquee-select :time-a 2 :time-b 3 :row-delta 0)".to_string());
    act(&mut f.h, "(dict :type :copy-items)".to_string());
    act(&mut f.h, "(dict :type :paste-items :time 8)".to_string());
    let items = f.h.lane_items(f.track);
    assert_eq!(items.len(), 2, "{items:?}");
    let pasted = &items[1];
    assert_eq!((number(pasted.get("start")), number(pasted.get("end"))), (8.0, 9.0));
    let Some(Value::Map(content)) = pasted.get("content") else {
        panic!("pasted region draws its audio");
    };
    let offset = match &*content["offset"].borrow() {
        Value::Number(n) => *n,
        other => panic!("offset: {other:?}"),
    };
    assert!((offset - 1.0).abs() < 1e-6, "the slice starts one second in: {offset}");
}

#[test]
fn cmd_e_splits_the_region_under_the_cursor() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
    f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap();
    audio_settle(&mut f.h);
    f.h.eval(&format!(
        "(eseq.arrangement/audio-lane-action {} (dict :type :split-items :ids (list) :time 1))",
        f.track
    ));
    audio_settle(&mut f.h);
    let spans: Vec<(f64, f64)> = f
        .h
        .lane_items(f.track)
        .iter()
        .map(|item| (number(item.get("start")), number(item.get("end"))))
        .collect();
    assert_eq!(spans, vec![(0.0, 1.0), (1.0, 4.0)]);
}

/// Ableton: with a time selection inside a region, Cmd+E cuts at both of its
/// ends, so the selected stretch becomes its own region.
#[test]
fn cmd_e_cuts_a_selection_out_at_both_ends() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
    f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap();
    audio_settle(&mut f.h);
    let act = |h: &mut Harness, event: &str| {
        h.eval(&format!("(eseq.arrangement/audio-lane-action {} {event})", f.track));
        audio_settle(h);
    };
    act(&mut f.h, "(dict :type :finish-marquee-select :time-a 1 :time-b 3 :row-delta 0)");
    act(&mut f.h, "(dict :type :split-items :ids (list) :time 1)");
    let spans: Vec<(f64, f64)> = f
        .h
        .lane_items(f.track)
        .iter()
        .map(|item| (number(item.get("start")), number(item.get("end"))))
        .collect();
    assert_eq!(spans, vec![(0.0, 1.0), (1.0, 3.0), (3.0, 4.0)]);
}

/// A sample dragged over an audio lane previews the region it would make
/// where it would land, from the file's header length.
#[test]
fn dragging_a_sample_over_a_lane_previews_where_it_lands() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 3);
    f.h.eval(&format!(
        r#"(eseq.arrangement/audio-drag-over {} (dict :drag-type "sample" :payload (dict :path "{}") :sx -1))"#,
        f.track,
        wav.display()
    ));
    // Three seconds at 120 bpm: six beats from the lane's left edge.
    assert_eq!(
        f.h.eval("(let ((d eseq.arrangement/arr-drop)) (list d.lane d.start d.end))"),
        f.h.eval(&format!("(list {} 0 6)", f.track))
    );
    // The file started decoding when the drag arrived: its preview fills in
    // in the background until the whole three seconds are registered.
    let preview = number(Some(&f.h.eval("(let ((d eseq.arrangement/arr-drop)) d.sample)")));
    assert!(preview >= 0.0, "one file draws its waveform");
    let key = eseqlisp::audio::sample::drag_preview_key(preview as u64);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let decoded = eseqlisp::audio::sample::get_registered_sample(&key)
            .map_or(0.0, |sample| sample.duration_seconds);
        if (decoded - 3.0).abs() < 1e-6 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "decoded {decoded}s of 3");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // The same file dragged again reuses its waveform.
    let again = f.h.eval(&format!(r#"(get (seq-audio-drag-preview "{}") :id)"#, wav.display()));
    assert_eq!(number(Some(&again)), preview);
    // The drop imports it for real and frees the preview.
    f.h.eval("(seq-audio-drag-preview-clear)");
    assert!(eseqlisp::audio::sample::get_registered_sample(&key).is_none(), "freed");
}

/// Ableton: a drag across the clip editor's waveform selects that stretch of
/// audio there and the same stretch of the timeline, so it can be copied;
/// the start/end markers stay where they were.
#[test]
fn a_waveform_drag_selects_the_same_stretch_of_the_timeline() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
    let id = f.h.app.audio_regions_drop(f.track, 4.0, &[wav]).unwrap()[0];
    audio_settle(&mut f.h);
    f.h.eval(&format!(
        "(eseq.arrangement/audio-lane-action {} (dict :type :double-click-item :ids (list {}) :time 4))",
        f.track, id.0
    ));
    audio_settle(&mut f.h);
    // Seconds 0.5-1.0 of the source: beats 5-6 at 120 bpm.
    f.h.eval(&format!(
        "(eseq.audio-clip/waveform-action (eseq.audio-clip/bound-region (track {}))
           (dict :type :end-range :start 0.5 :end 1.0))",
        f.track
    ));
    audio_settle(&mut f.h);
    let region = f.h.app.song_region_selection.expect("a timeline region");
    assert_eq!((region.start_beat, region.end_beat), (5.0, 6.0));
    assert!(region.contains_track(f.track));
    let lane = f.h.app.state.with_committed_arrangement(|a| a.unwrap().audio_lane(f.track).to_vec());
    assert_eq!((lane[0].start_beat, lane[0].end_beat, lane[0].source_offset), (4.0, 8.0, 0), "the markers did not move");
}

/// Cmd+C in the clip editor (the *piano-roll* buffer) copies the arrangement
/// region a waveform drag selected; the clip editor binds the song position
/// as its playhead.
#[test]
fn the_clip_editor_copies_the_selected_range_and_follows_the_song() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
    let id = f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    audio_settle(&mut f.h);
    f.h.eval(&format!(
        "(eseq.arrangement/audio-lane-action {} (dict :type :double-click-item :ids (list {}) :time 0))",
        f.track, id.0
    ));
    audio_settle(&mut f.h);
    f.h.eval(&format!(
        "(eseq.audio-clip/waveform-action (eseq.audio-clip/bound-region (track {}))
           (dict :type :end-range :start 0.5 :end 1.0))",
        f.track
    ));
    audio_settle(&mut f.h);
    // Edit > Copy reaches the focused waveform first; it refuses the key and
    // hands it to its :on-focus-key, which copies the region.
    assert_eq!(f.h.eval(r#"(eseq.audio-clip/focus-key "s-c" false)"#), Value::Bool(true));
    let mut waveforms = Vec::new();
    widgets_with_prop(&f.h.buffer_tree("*piano-roll*").0, "on-focus-key", &mut waveforms);
    assert!(!waveforms.is_empty(), "the waveform takes refused keys");
    audio_settle(&mut f.h);
    f.h.eval(&format!("(eseq.arrangement/set-cursor 8 {})", f.track));
    assert_eq!(f.h.eval(r#"(eseq.audio-clip/focus-key "s-v" false)"#), Value::Bool(true));
    audio_settle(&mut f.h);
    let starts: Vec<f64> =
        f.h.lane_items(f.track).iter().map(|item| number(item.get("start"))).collect();
    assert_eq!(starts, vec![0.0, 8.0], "the copied stretch pasted at the cursor");

    let mut waveforms = Vec::new();
    widgets_with_prop(&f.h.buffer_tree("*piano-roll*").0, "playhead-map", &mut waveforms);
    assert_eq!(waveforms.len(), 1, "the clip editor maps the song position");
}

/// A range dragged across audio the region does not play (outside its start
/// and end markers) stays selected in the clip editor, selects nothing on the
/// timeline, and Cmd+C / Cmd+V still copy and paste that stretch of source.
#[test]
fn the_clip_editor_copies_a_range_outside_the_playable_window() {
    let mut f = fixture();
    let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
    let id = f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap()[0];
    f.h.app.audio_region_set_window(id, 0.0, 0.25, false).unwrap();
    audio_settle(&mut f.h);
    f.h.eval(&format!(
        "(eseq.arrangement/audio-lane-action {} (dict :type :double-click-item :ids (list {}) :time 0))",
        f.track, id.0
    ));
    audio_settle(&mut f.h);
    f.h.eval(&format!(
        "(eseq.audio-clip/waveform-action (eseq.audio-clip/bound-region (track {}))
           (dict :type :end-range :start 1.0 :end 1.5))",
        f.track
    ));
    audio_settle(&mut f.h);
    let mut waveforms = Vec::new();
    widgets_with_prop(&f.h.buffer_tree("*piano-roll*").0, "range-start", &mut waveforms);
    assert_eq!(waveforms.len(), 1);
    assert_eq!(waveforms[0].get("range-start"), Some(&Value::Number(1.0)), "the range stays in the clip view");
    assert!(f.h.app.song_region_selection.is_none(), "nothing on the timeline to select");
    assert_eq!(f.h.eval(r#"(eseq.audio-clip/focus-key "s-c" false)"#), Value::Bool(true));
    audio_settle(&mut f.h);
    f.h.eval(&format!("(eseq.arrangement/set-cursor 8 {})", f.track));
    assert_eq!(f.h.eval(r#"(eseq.audio-clip/focus-key "s-v" false)"#), Value::Bool(true));
    audio_settle(&mut f.h);
    let lane = f.h.app.state.with_committed_arrangement(|a| a.unwrap().audio_lane(f.track).to_vec());
    let spans: Vec<_> = lane.iter().map(|r| (r.start_beat, r.source_offset)).collect();
    assert_eq!(spans, vec![(0.0, 0), (8.0, 48_000)], "the out-of-window stretch pasted at the cursor");
}

/// One mixer state per audio track: a fader move while stopped survives
/// Play and Stop, with or without a scene launched first (the distro has
/// scenes and session tracks, as a real project does).
#[test]
fn an_audio_track_fader_move_while_stopped_survives_play() {
    for launch_scene in [false, true] {
        let mut f = fixture();
        let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
        f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap();
        f.h.app.set_arrangement_view_visible(true);
        audio_settle(&mut f.h);
        if launch_scene {
            f.h.app
                .apply_pattern_launch(&sequencer::quantized_launch::PatternLaunchTarget::Scene { scene: 0 })
                .unwrap();
            audio_settle(&mut f.h);
        }
        let track = f.track;
        let volume = |h: &mut Harness| h.app.state.pattern.track_params[track].get_volume();
        f.h.eval(&format!("(let ((t (track {track}))) (set! t.volume 0.3))"));
        audio_settle(&mut f.h);
        assert_eq!(volume(&mut f.h), 0.3);
        f.h.app.song_transport_play(false).expect("play");
        audio_settle(&mut f.h);
        assert_eq!(volume(&mut f.h), 0.3, "Play keeps the fader (scene launched: {launch_scene})");
        f.h.app.song_transport_stop().expect("stop");
        audio_settle(&mut f.h);
        assert_eq!(volume(&mut f.h), 0.3, "Stop keeps the fader (scene launched: {launch_scene})");
    }
}

/// A fader move DURING song playback survives Stop and a fresh Play, in a
/// project whose arrangement has a scene row (the user's repro, 2026-11-02:
/// "volume x, play, change to y, restart, back to x").
#[test]
fn an_audio_track_fader_move_while_playing_survives_a_restart() {
    for (scene_row, arrangement_view) in [(false, true), (true, true), (true, false)] {
        let mut f = fixture();
        let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
        f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap();
        let track = f.track;
        if scene_row {
            // Scene 0 holds a cell on the audio track that remembers its own
            // volume, as any project saved with session content does.
            f.h.app.state.pattern.track_params[track].set_volume(0.9);
            let app = &f.h.app;
            assert!(app.state.save_current_pattern_snapshot(
                app.tracks.len(),
                &app.graph.track_buffer_ids,
                &app.graph.track_sample_rates,
                &app.tracks,
                &app.graph.track_instrument_types,
            ));
            f.h.app.arr_scene_event_insert(0.0, 0).unwrap();
        }
        f.h.app.set_arrangement_view_visible(arrangement_view);
        audio_settle(&mut f.h);
        let volume = |h: &mut Harness| h.app.state.pattern.track_params[track].get_volume();
        for (value, label) in [(0.3, "first pass"), (0.6, "second pass")] {
            f.h.app.song_transport_play(false).expect("play");
            audio_settle(&mut f.h);
            f.h.eval(&format!("(let ((t (track {track}))) (set! t.volume {value}))"));
            audio_settle(&mut f.h);
            f.h.app.song_transport_stop().expect("stop");
            audio_settle(&mut f.h);
            assert_eq!(volume(&mut f.h), value, "Stop keeps it ({label}, scene row {scene_row}, arrangement view {arrangement_view})");
            f.h.app.song_transport_play(false).expect("play again");
            audio_settle(&mut f.h);
            assert_eq!(volume(&mut f.h), value, "Play keeps it ({label}, scene row {scene_row}, arrangement view {arrangement_view})");
            f.h.app.song_transport_stop().expect("stop");
            audio_settle(&mut f.h);
        }
    }
}

/// The user's repro (2026-11-02): an audio track whose scene cell remembers
/// a different mix than the track sound. Stopped showed the track sound,
/// Play showed the cell, and an edit landed in whichever was installed —
/// "edits flow to a dead store". One state per audio track: whatever is
/// set, stopped or playing, in either view, is what the next pass plays.
#[test]
fn an_audio_track_has_one_mix_whatever_plays_it() {
    for arrangement_view in [true, false] {
        let mut f = fixture();
        let wav = write_wav(f.dir.path(), "hit.wav", 48_000 * 2);
        f.h.app.audio_regions_drop(f.track, 0.0, &[wav]).unwrap();
        let track = f.track;
        // Fork the cell's sound away from the track sound with its own
        // volume, as an older project's cell can be.
        f.h.app.state.pattern.track_params[track].set_volume(0.9);
        {
            let app = &f.h.app;
            assert!(app.state.save_current_pattern_snapshot(
                app.tracks.len(),
                &app.graph.track_buffer_ids,
                &app.graph.track_sample_rates,
                &app.tracks,
                &app.graph.track_instrument_types,
            ));
        }
        f.h.app.arr_scene_event_insert_placing(0.0, 0).unwrap();
        f.h.app.set_arrangement_view_visible(arrangement_view);
        audio_settle(&mut f.h);
        let volume = |h: &mut Harness| h.app.state.pattern.track_params[track].get_volume();
        let set = |h: &mut Harness, v: f64| {
            h.eval(&format!("(let ((t (track {track}))) (set! t.volume {v}))"));
            audio_settle(h);
        };
        // Stopped edit, then Play.
        set(&mut f.h, 0.3);
        f.h.app.song_transport_play(false).expect("play");
        audio_settle(&mut f.h);
        assert_eq!(volume(&mut f.h), 0.3, "Play keeps a stopped edit (arrangement view {arrangement_view})");
        // Playing edit, then Stop and Play.
        set(&mut f.h, 0.6);
        f.h.app.song_transport_stop().expect("stop");
        audio_settle(&mut f.h);
        assert_eq!(volume(&mut f.h), 0.6, "Stop keeps a playing edit (arrangement view {arrangement_view})");
        f.h.app.song_transport_play(false).expect("play");
        audio_settle(&mut f.h);
        assert_eq!(volume(&mut f.h), 0.6, "the next pass keeps it (arrangement view {arrangement_view})");
        f.h.app.song_transport_stop().expect("stop");
    }
}

/// eseq-a921: with a region selected on the timeline, an effect knob turn
/// locks that region (the track value stays), the knob reads the lock as
/// locked and the dot lights; selecting nothing edits the track value and
/// shows it; "Clear p-locks" removes the region locks.
#[test]
fn an_effect_edit_with_a_region_selected_locks_the_region() {
    let mut f = fixture();
    let slot = f.h.app.add_builtin_effect_sync(f.track, "Reverb").unwrap();
    let wav = write_wav(f.dir.path(), "hit.wav", 24_000);
    let ids = f.h.app.audio_regions_drop(f.track, 0.0, &[wav.clone(), wav]).unwrap();
    audio_settle(&mut f.h);
    let param = 0;
    let name = f.h.app.graph.effect_descriptors[f.track][slot].params[param].name.clone();
    let base = f.h.app.state.pattern.effect_chains[f.track][slot].defaults.get(param);
    let (min, max) = {
        let desc = &f.h.app.graph.effect_descriptors[f.track][slot].params[param];
        (desc.min, desc.max)
    };
    let locked = if base > (min + max) / 2.0 { min } else { max };
    f.h.eval_all(&format!(
        r#"(def fxd (let ((t (track {}))) (first (filter (lambda (d) (= d.slot {slot})) (filter (lambda (d) (not (= d.role "instrument"))) t.devices)))))
           (def prm (device-param fxd "{name}"))"#,
        f.track
    ));
    let read = |h: &mut Harness| h.eval_all("(list prm.value prm.locked prm.has-locks)");
    let select = |h: &mut Harness, id: u64| {
        h.eval_all(&format!("(eseq.arrangement/audio-lane-action {} (dict :type :select :ids (list {id}) :time 0))", f.track));
        audio_settle(h);
    };
    let set = |h: &mut Harness, value: f32| {
        let payload = h.eval_all(&format!("(dict :slot-idx {slot} :param-idx {param} :value {value})"));
        h.command("set-effect-param", payload);
        audio_settle(h);
    };

    select(&mut f.h, ids[0].0);
    set(&mut f.h, locked);
    assert_eq!(f.h.app.audio_region_effect_locks(f.track, slot, param), vec![(ids[0], locked)]);
    assert_eq!(
        f.h.app.state.pattern.effect_chains[f.track][slot].defaults.get(param),
        base,
        "the track value is untouched"
    );
    let shown = read(&mut f.h);
    assert_eq!(f.h.eval_all("(list prm.locked prm.has-locks)"), f.h.eval_all("(list true true)"), "{shown:?}");
    let unlocked_value = f.h.eval_all("prm.base");
    assert_ne!(f.h.eval_all("prm.value"), unlocked_value, "the knob shows the lock, not the track value");

    // The other region shows the track value, unlocked; the dot stays lit.
    select(&mut f.h, ids[1].0);
    assert_eq!(f.h.eval_all("(list prm.locked prm.has-locks)"), f.h.eval_all("(list false true)"));

    // Nothing selected: an edit moves the track value.
    f.h.eval_all(&format!("(eseq.arrangement/audio-lane-action {} (dict :type :clear-selection))", f.track));
    audio_settle(&mut f.h);
    assert_eq!(f.h.app.audio_lock_regions(f.track), None);
    let moved = (base + locked) / 2.0;
    set(&mut f.h, moved);
    assert_eq!(f.h.app.state.pattern.effect_chains[f.track][slot].defaults.get(param), moved);
    assert_eq!(f.h.app.audio_region_effect_locks(f.track, slot, param), vec![(ids[0], locked)]);

    // Clear p-locks from the knob's menu.
    let payload = f.h.eval_all(&format!(
        r#"(dict :track {} :target "effect" :slot-idx {slot} :rack-slot 0 :param-idx {param} :scope "all")"#,
        f.track
    ));
    f.h.command("clear-param-plocks", payload);
    audio_settle(&mut f.h);
    assert!(f.h.app.audio_region_effect_locks(f.track, slot, param).is_empty());
    assert_eq!(f.h.eval_all("prm.has-locks"), Value::Bool(false));
}
