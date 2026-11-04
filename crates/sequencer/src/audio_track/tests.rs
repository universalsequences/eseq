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
        fade_in: 0.0,
        fade_out: 0.0,
        fade_in_power: 1.0,
        fade_out_power: 1.0,
        xfade_pre: 0.0,
        xfade_post: 0.0,
        pcm,
        locks: Vec::new(),
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
    let playback = AudioTrackPlayback { regions: vec![region(1.0, 3.0, 100.0, pcm)], ..Default::default() };
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
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 4.0, 0.0, pcm)], ..Default::default() };
    let mut player = player();
    // One beat in: 24 000 host frames = 22 050 source frames.
    let (left, _) = render(&mut player, &playback, Some(mapping(1.0)), 0, 2);
    assert!((left[0] - 22_050.0).abs() < 1e-2, "{}", left[0]);
    assert!((left[1] - 22_050.919).abs() < 1e-2, "{}", left[1]);
}

#[test]
fn edges_fade_over_the_declick_and_gaps_are_silent() {
    let pcm = Arc::new(SourcePcm { samples: vec![1.0; 2 * 96_000].into(), sample_rate: 48_000 });
    let playback = AudioTrackPlayback { regions: vec![region(1.0, 2.0, 0.0, pcm)], ..Default::default() };
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
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 4.0, 0.0, pcm)], ..Default::default() };
    let mut player = player();
    let (left, _) = render(&mut player, &playback, Some(mapping(1.0)), 0, 4);
    assert_eq!(left, vec![0.0; 4]);
}

#[test]
fn starting_ramps_in_and_stopping_ramps_out_from_where_it_was() {
    let pcm = Arc::new(SourcePcm { samples: vec![1.0; 2 * 960_000].into(), sample_rate: 48_000 });
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 32.0, 0.0, pcm)], ..Default::default() };
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
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 32.0, 0.0, pcm)], ..Default::default() };
    let mut player = player();
    render(&mut player, &playback, Some(mapping(4.0)), 0, 64);
    let (left, _) = render(&mut player, &playback, Some(mapping(9.0)), 64, 4);
    assert!(left[0] < 0.05, "the jump restarts the ramp: {}", left[0]);
}

#[test]
fn a_loop_wrap_inside_a_block_lands_on_the_exact_frame() {
    let pcm = ramp_pcm(960_000, 48_000);
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 1.0, 0.0, pcm)], ..Default::default() };
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
    let playback = AudioTrackPlayback { regions: vec![region(0.0, 4.0, 0.0, pcm)], ..Default::default() };
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
        ..Default::default()
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
        applied_playback: AtomicU64::new(0),
        applied_region: std::sync::atomic::AtomicI64::new(-1),
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
    let playback = AudioTrackPlayback { regions: vec![looped], ..Default::default() };
    let mut player = player();
    // 2 000 host frames in: 1 000 frames through the loop twice, back at its start.
    let (left, _) = render(&mut player, &playback, Some(mapping(2_000.0 / SAMPLES_PER_BEAT)), 0, 2);
    assert!((left[0] - 100.0).abs() < 1e-2, "{}", left[0]);
    assert!((left[1] - 101.0).abs() < 1e-2, "{}", left[1]);
}

fn lock_slot() -> AudioTrackSlot {
    let rendered = Box::leak(Box::new(AtomicU64::new(0)));
    let song = Box::leak(Box::new(SongPositionShared::default()));
    AudioTrackSlot {
        playback: AtomicPtr::new(std::ptr::null_mut()),
        applied_playback: AtomicU64::new(0),
        applied_region: std::sync::atomic::AtomicI64::new(-1),
        song: song as *const SongPositionShared,
        rendered: rendered as *const AtomicU64,
    }
}

/// eseq-a921: entering a locking region pushes its value, staying in it
/// re-asserts it every block, and leaving it (or stopping) pushes the track
/// value back. A region that locks nothing reverts too.
#[test]
fn region_locks_apply_inside_their_region_and_revert_outside() {
    let pcm = ramp_pcm(48_000, 48_000);
    let wet = LockParam { logical_id: 7, idx: 2, base: 0.18 };
    let mut locked = region(1.0, 2.0, 0.0, Arc::clone(&pcm));
    locked.locks = vec![(0, 0.6)];
    let playback = AudioTrackPlayback {
        regions: vec![region(0.0, 1.0, 0.0, Arc::clone(&pcm)), locked],
        lock_params: vec![wet],
        generation: 1,
    };
    let slot = lock_slot();
    let step = |beat: Option<f64>| {
        let mut pushed = Vec::new();
        apply_slot_locks(&slot, &playback, beat, &mut |param, value| pushed.push((param.logical_id, value)));
        pushed
    };
    assert_eq!(step(Some(0.5)), vec![(7, 0.18)], "first block: the track value");
    assert_eq!(step(Some(0.6)), vec![], "an unlocked region pushes nothing after that");
    assert_eq!(step(Some(1.0)), vec![(7, 0.6)], "entering the locking region");
    assert_eq!(step(Some(1.5)), vec![(7, 0.6)], "re-asserted every block inside it");
    assert_eq!(step(Some(2.5)), vec![(7, 0.18)], "past its end: back to the track value");
    assert_eq!(step(Some(1.2)), vec![(7, 0.6)], "a seek back in");
    assert_eq!(step(None), vec![(7, 0.18)], "stopping reverts");
}

/// A republished snapshot is told apart by its generation, not its address
/// (a freed snapshot's can come back): one that drops the lock reverts the
/// parameter even though the region index did not change.
#[test]
fn a_republished_snapshot_without_the_lock_reverts_it() {
    let pcm = ramp_pcm(48_000, 48_000);
    let wet = LockParam { logical_id: 7, idx: 2, base: 0.18 };
    let mut locked = region(0.0, 2.0, 0.0, Arc::clone(&pcm));
    locked.locks = vec![(0, 0.6)];
    let before = AudioTrackPlayback { regions: vec![locked], lock_params: vec![wet], generation: 1 };
    let after = AudioTrackPlayback {
        regions: vec![region(0.0, 2.0, 0.0, pcm)],
        lock_params: vec![wet],
        generation: 2,
    };
    let slot = lock_slot();
    let step = |playback: &AudioTrackPlayback| {
        let mut pushed = Vec::new();
        apply_slot_locks(&slot, playback, Some(1.0), &mut |_, value| pushed.push(value));
        pushed
    };
    assert_eq!(step(&before), vec![0.6]);
    assert_eq!(step(&after), vec![0.18], "the lock was removed: back to the track value");
    assert_eq!(step(&after), Vec::<f32>::new());
}

/// One constant-1.0 source, long enough for every region below.
fn ones() -> Arc<SourcePcm> {
    Arc::new(SourcePcm { samples: vec![1.0; 2 * 480_000].into(), sample_rate: 48_000 })
}

fn level_at(playback: &AudioTrackPlayback, beat: f64) -> f32 {
    let mut hint = 0;
    playback.frame_at(beat, 1.0 / SAMPLES_PER_BEAT, &mut hint).0
}

#[test]
fn a_curved_fade_follows_its_power() {
    let mut fading = region(0.0, 4.0, 0.0, ones());
    fading.fade_in = SAMPLES_PER_BEAT;
    fading.fade_in_power = crate::sequencer::fade_curve_power(-1.0 / 3.0);
    fading.fade_out = SAMPLES_PER_BEAT;
    fading.fade_out_power = 0.5;
    let playback = AudioTrackPlayback { regions: vec![fading], ..Default::default() };
    // Halfway into the fade-in: 0.5^2 with power 2.
    assert!((level_at(&playback, 0.5) - 0.25).abs() < 1e-4, "{}", level_at(&playback, 0.5));
    assert!((level_at(&playback, 2.0) - 1.0).abs() < 1e-6, "full between the fades");
    // Three quarters of the way through the fade-out: 0.25^0.5.
    assert!((level_at(&playback, 3.75) - 0.5).abs() < 1e-4, "{}", level_at(&playback, 3.75));
}

#[test]
fn a_crossfade_keeps_one_recording_at_full_level_across_the_cut() {
    // A cut through one constant recording, crossfaded over a beat before
    // and half a beat after the cut: the sum never dips or bumps.
    let pcm = ones();
    let head = region(0.0, 4.0, 0.0, pcm.clone());
    let mut tail = region(4.0, 8.0, 4.0 * SAMPLES_PER_BEAT, pcm);
    tail.xfade_pre = SAMPLES_PER_BEAT;
    tail.xfade_post = SAMPLES_PER_BEAT / 2.0;
    let playback = AudioTrackPlayback { regions: vec![head, tail], ..Default::default() };
    for step in 0..=24 {
        let beat = 2.75 + step as f64 * 0.0625;
        let level = level_at(&playback, beat);
        assert!((level - 1.0).abs() < 1e-4, "beat {beat}: {level}");
    }
}

#[test]
fn a_crossfade_hands_one_source_to_the_other() {
    // The head plays +1, the tail -1, so the mix says how far along it is.
    let minus = Arc::new(SourcePcm { samples: vec![-1.0; 2 * 480_000].into(), sample_rate: 48_000 });
    let head = region(0.0, 4.0, 0.0, ones());
    let mut tail = region(4.0, 8.0, SAMPLES_PER_BEAT, minus);
    tail.xfade_pre = SAMPLES_PER_BEAT;
    let playback = AudioTrackPlayback { regions: vec![head, tail], ..Default::default() };
    assert!((level_at(&playback, 2.9) - 1.0).abs() < 1e-6, "before the window only the head plays");
    assert!(level_at(&playback, 3.5).abs() < 1e-4, "equal gain at the middle: 1 - 1 = 0");
    assert!((level_at(&playback, 3.999) + 1.0).abs() < 1e-3, "the tail has taken over by the cut");
    assert!((level_at(&playback, 4.0) + 1.0).abs() < 1e-6, "no declick dip at a crossfaded start");
}
