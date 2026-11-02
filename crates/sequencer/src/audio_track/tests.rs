use super::*;

const HOST_RATE: f32 = 48_000.0;
/// 120 bpm at 48 kHz: 24 000 host frames per beat.
const SAMPLES_PER_BEAT: f64 = 24_000.0;

fn mapping(beat: f64) -> SongRenderMapping {
    SongRenderMapping {
        beat,
        beats_per_sample: 1.0 / SAMPLES_PER_BEAT,
        pass_start_sample: 0,
        end_beat: 64.0,
        loop_enabled: false,
        open_ended: false,
    }
}

fn player() -> RegionPlayerState {
    RegionPlayerState {
        slot: std::ptr::null(),
        host_rate: HOST_RATE,
        ramp: 1.0,
        last_beat: 0.0,
        last_beats_per_sample: 0.0,
        region_hint: 0,
    }
}

/// A source whose left channel at frame `n` is `n` and right is `-n`, so
/// every read says which frame it came from.
fn ramp_pcm(frames: usize, sample_rate: u32) -> Arc<SourcePcm> {
    let samples = (0..frames).flat_map(|n| [n as f32, -(n as f32)]).collect();
    Arc::new(SourcePcm { samples, sample_rate })
}

fn region(start: f64, end: f64, offset: f64, pcm: Arc<SourcePcm>) -> PlaybackRegion {
    PlaybackRegion {
        start_beat: start,
        end_beat: end,
        source_offset: offset,
        step: sample_core::playback_ratio(pcm.sample_rate, HOST_RATE),
        loop_window: None,
        gain: 1.0,
        fade_in: DECLICK_FRAMES,
        fade_out: DECLICK_FRAMES,
        pcm,
    }
}

fn render(
    player: &mut RegionPlayerState,
    playback: &AudioTrackPlayback,
    mapping: Option<SongRenderMapping>,
    block_start: u64,
    frames: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (vec![9.0; frames], vec![9.0; frames]);
    render_block(player, Some(playback), mapping, block_start, &mut left, &mut right);
    (left, right)
}

#[test]
fn plays_the_source_frame_for_each_song_position_past_the_declick() {
    let pcm = ramp_pcm(96_000, 48_000);
    let playback = AudioTrackPlayback { regions: vec![region(1.0, 3.0, 100.0, pcm)] };
    let mut player = player();
    // Block starts half a beat into the region: 12 000 host frames in.
    let (left, right) = render(&mut player, &playback, Some(mapping(1.5)), 0, 8);
    for (n, (l, r)) in left.iter().zip(&right).enumerate() {
        let frame = 100.0 + 12_000.0 + n as f32;
        assert!((l - frame).abs() < 1e-3 && (r + frame).abs() < 1e-3, "frame {n}: {l} {r}");
    }
}

#[test]
fn a_44k_source_steps_at_its_native_rate() {
    let pcm = ramp_pcm(96_000, 44_100);
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 4.0, 0.0, pcm)] };
    let mut player = player();
    // One beat in: 24 000 host frames = 22 050 source frames.
    let (left, _) = render(&mut player, &playback, Some(mapping(1.0)), 0, 2);
    assert!((left[0] - 22_050.0).abs() < 1e-2, "{}", left[0]);
    assert!((left[1] - 22_050.919).abs() < 1e-2, "{}", left[1]);
}

#[test]
fn edges_fade_over_the_declick_and_gaps_are_silent() {
    let pcm = Arc::new(SourcePcm { samples: vec![1.0; 2 * 96_000].into(), sample_rate: 48_000 });
    let playback = AudioTrackPlayback { regions: vec![region(1.0, 2.0, 0.0, pcm)] };
    let mut player = player();
    // Start 4 frames before the region.
    let start = 1.0 - 4.0 / SAMPLES_PER_BEAT;
    let (left, _) = render(&mut player, &playback, Some(mapping(start)), 0, 72);
    assert_eq!(&left[..4], &[0.0; 4], "nothing plays before the region");
    assert_eq!(left[4], 0.0, "the first frame of the region starts the fade at zero");
    assert!((left[4 + 32] - 0.5).abs() < 1e-6, "halfway through the declick");
    assert!((left[4 + 64] - 1.0).abs() < 1e-6);

    // The last frames before the end fade back down.
    let mut player = self::player();
    let end = 2.0 - 2.0 / SAMPLES_PER_BEAT;
    let (left, _) = render(&mut player, &playback, Some(mapping(end)), 0, 4);
    assert!((left[0] - 2.0 / 64.0).abs() < 1e-6, "{}", left[0]);
    assert!((left[1] - 1.0 / 64.0).abs() < 1e-6, "{}", left[1]);
    assert_eq!(&left[2..], &[0.0, 0.0], "past the end is silence");
}

#[test]
fn regions_past_their_audio_play_silence() {
    let pcm = ramp_pcm(10, 48_000);
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 4.0, 0.0, pcm)] };
    let mut player = player();
    let (left, _) = render(&mut player, &playback, Some(mapping(1.0)), 0, 4);
    assert_eq!(left, vec![0.0; 4]);
}

#[test]
fn starting_ramps_in_and_stopping_ramps_out_from_where_it_was() {
    let pcm = Arc::new(SourcePcm { samples: vec![1.0; 2 * 960_000].into(), sample_rate: 48_000 });
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 32.0, 0.0, pcm)] };
    let mut player = player();
    player.ramp = 0.0;
    let (left, _) = render(&mut player, &playback, Some(mapping(4.0)), 0, 128);
    assert!((left[0] - 1.0 / 64.0).abs() < 1e-6, "the transport starts silent");
    assert_eq!(left[127], 1.0);

    let (left, _) = render(&mut player, &playback, None, 128, 128);
    assert!(left[0] > 0.9 && left[0] < 1.0, "stopping fades instead of cutting: {}", left[0]);
    assert_eq!(left[127], 0.0);
    assert_eq!(player.ramp, 0.0);
}

#[test]
fn a_seek_ramps_from_silence_instead_of_splicing() {
    let pcm = Arc::new(SourcePcm { samples: vec![1.0; 2 * 960_000].into(), sample_rate: 48_000 });
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 32.0, 0.0, pcm)] };
    let mut player = player();
    render(&mut player, &playback, Some(mapping(4.0)), 0, 64);
    let (left, _) = render(&mut player, &playback, Some(mapping(9.0)), 64, 4);
    assert!(left[0] < 0.05, "the jump restarts the ramp: {}", left[0]);
}

#[test]
fn a_loop_wrap_inside_a_block_lands_on_the_exact_frame() {
    let pcm = ramp_pcm(960_000, 48_000);
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 1.0, 0.0, pcm)] };
    let mut player = player();
    let looping = SongRenderMapping {
        end_beat: 8.0,
        loop_enabled: true,
        ..mapping(8.0 - 2.0 / SAMPLES_PER_BEAT)
    };
    let (left, _) = render(&mut player, &playback, Some(looping), 0, 4);
    assert_eq!(&left[..2], &[0.0, 0.0], "beats 7.99… hold no region");
    // Frame 2 is song beat 0 again: source frame 0, and the ramp restarts.
    assert_eq!(left[2], 0.0);
    assert!(left[3] > 0.0 && left[3] < 1.0, "{}", left[3]);
}

#[test]
fn frames_before_the_pass_start_are_silent() {
    let pcm = Arc::new(SourcePcm { samples: vec![1.0; 2 * 96_000].into(), sample_rate: 48_000 });
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 4.0, 0.0, pcm)] };
    let mut player = player();
    let pre_roll = SongRenderMapping { pass_start_sample: 1_002, ..mapping(1.0) };
    let (left, _) = render(&mut player, &playback, Some(pre_roll), 1_000, 4);
    assert_eq!(&left[..2], &[0.0, 0.0]);
    assert!(left[2] > 0.0);
}

#[test]
fn hint_search_finds_regions_in_any_order() {
    let pcm = ramp_pcm(10, 48_000);
    let playback = AudioTrackPlayback {
        regions: (0..8)
            .map(|n| region(n as f64 * 2.0, n as f64 * 2.0 + 1.0, 0.0, Arc::clone(&pcm)))
            .collect(),
    };
    assert_eq!(playback.region_at(6.5, 0), Some(3));
    assert_eq!(playback.region_at(6.5, 3), Some(3));
    assert_eq!(playback.region_at(8.0, 3), Some(4));
    assert_eq!(playback.region_at(7.5, 3), None);
    assert_eq!(playback.region_at(100.0, 7), None);
}

#[test]
fn retired_snapshots_wait_for_the_audio_thread_to_finish_a_block() {
    let rendered = Box::leak(Box::new(AtomicU64::new(512)));
    let song = Box::leak(Box::new(SongPositionShared::default()));
    let mut players = AudioTrackPlayers::default();
    players.rendered = Some(rendered as *const AtomicU64);
    players.slots.push(Box::new(AudioTrackSlot {
        playback: AtomicPtr::new(std::ptr::null_mut()),
        song: song as *const SongPositionShared,
        rendered: rendered as *const AtomicU64,
    }));
    players.publish(0, AudioTrackPlayback::default());
    assert_eq!(players.retired_len(), 0, "nothing to retire on the first publish");
    players.publish(0, AudioTrackPlayback::default());
    assert_eq!(players.retired_len(), 1, "a block may still be reading the old snapshot");
    rendered.store(1_024, Ordering::SeqCst);
    players.reclaim();
    assert_eq!(players.retired_len(), 0);
}

#[test]
fn a_looping_region_repeats_its_window() {
    let pcm = ramp_pcm(96_000, 48_000);
    let mut looped = region(0.0, 8.0, 100.0, pcm);
    looped.loop_window = Some((100.0, 1_100.0));
    let playback = AudioTrackPlayback { regions: vec![looped] };
    let mut player = player();
    // 2 000 host frames in: 1 000 frames through the loop twice, back at its start.
    let (left, _) = render(&mut player, &playback, Some(mapping(2_000.0 / SAMPLES_PER_BEAT)), 0, 2);
    assert!((left[0] - 100.0).abs() < 1e-2, "{}", left[0]);
    assert!((left[1] - 101.0).abs() < 1e-2, "{}", left[1]);
}
