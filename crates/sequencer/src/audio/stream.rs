/*!
Output-stream construction: the bridge from engine setup to the running
callback.

`build_output_stream` allocates the sampler/custom-engine voice pools,
assembles `AudioCallbackData`, spawns the keyboard MIDI-FX router thread and
the scheduler thread, and builds and plays the CPAL stream whose data
callback is `audio_callback`. `query_device_config` probes the default output
device for (sample rate, channels).
*/

#[allow(unused_imports)]
use super::*;

/// Build a cpal output stream that drives the audiograph.
pub fn build_output_stream(
    lg: *mut LiveGraph,
    state: Arc<SequencerState>,
    sample_rate: u32,
    num_channels: usize,
    block_size: usize,
    master_recorder: Arc<MasterRecorder>,
    keyboard_rx: std::sync::mpsc::Receiver<KeyboardTrigger>,
    bus_effect_runtime: Arc<Mutex<Arc<Vec<BusEffectRuntimeState>>>>,
) -> Result<Stream, String> {
    // The DEVICE half of the record latency only. CPAL does not expose
    // portable output latency, so use the configured output block as the
    // sensible default; users can tune this transport value when their
    // device/OS path has additional latency. The graph's own compensation is
    // published separately by the latency planner and summed at the read site
    // (`SequencerState::total_record_latency_seconds`) — do not fold it in
    // here, or the next plan change would clobber this term.
    state.transport.record_latency_seconds.store(
        (block_size as f32 / sample_rate.max(1) as f32).to_bits(),
        Ordering::Release,
    );
    // Initialize voice pools from state
    let mut voice_pools: Vec<VoicePool> =
        (0..MAX_SAMPLER_POOLS).map(|_| VoicePool::new()).collect();
    let mut custom_engine_pools: Vec<CustomEnginePool> = (0..MAX_INSTRUMENT_ENGINES)
        .map(|_| CustomEnginePool::new())
        .collect();

    // Pre-populate voice pools for any existing tracks
    let num_tracks = state.active_track_count();
    for t in 0..num_tracks {
        sync_sampler_voice_pool(&state, t, &mut voice_pools[t]);

        if let Some(engine_id) = track_engine_id(&state, t) {
            sync_custom_engine_pool(&state, engine_id, &mut custom_engine_pools[engine_id]);
        }
    }

    let scheduled_events = Arc::new(ScheduledEventQueue::new());
    let rendered_samples = Arc::new(AtomicU64::new(0));
    let (audio_keyboard_tx, audio_keyboard_rx) = std::sync::mpsc::channel();
    let (live_keyboard_tx, live_keyboard_rx) = std::sync::mpsc::channel();
    {
        let state_for_keyboard_router = Arc::clone(&state);
        let _ = std::thread::Builder::new()
            .name("keyboard-midi-fx-router".to_string())
            .spawn(move || {
                while let Ok(trigger) = keyboard_rx.recv() {
                    if trigger.note_off {
                        let _ = live_keyboard_tx.send(trigger);
                        let _ = audio_keyboard_tx.send(trigger);
                        continue;
                    }
                    let use_midi_fx = trigger.track
                        < state_for_keyboard_router.active_track_count()
                        && !state_for_keyboard_router.pattern.track_params[trigger.track]
                            .midi_fx_chain()
                            .is_empty();
                    if use_midi_fx {
                        let _ = live_keyboard_tx.send(trigger);
                    } else {
                        let _ = audio_keyboard_tx.send(trigger);
                    }
                }
            });
    }
    let initial_scheduler_snapshot_version = state.scheduler_snapshot_version();
    let initial_scheduler_snapshot = state.latest_scheduler_snapshot();
    let initial_num_tracks = initial_scheduler_snapshot.transport.num_tracks;
    let initial_topology_epoch = initial_scheduler_snapshot.transport.topology_epoch;
    let trace_audio = env_flag("TINYSEQ_AUDIO_TRACE", false);
    crate::instruments::voice_modulator::set_process_stats_enabled(trace_audio);
    if trace_audio {
        eprintln!("audio-trace: enabled");
    }

    // Keep the large callback state behind one pointer before handing the
    // closure through CPAL's generic stream builders. Passing it by value makes
    // debug builds reserve a copy-sized stack slot at every generic layer.
    let cb_data = Box::new(AudioCallbackData {
        lg: LiveGraphPtr(lg),
        state,
        num_channels,
        sample_rate: sample_rate as f64,
        last_bpm: 0,
        last_mod_reset_counter: 0,
        voice_pools,
        custom_engine_pools,
        scheduler_snapshot: initial_scheduler_snapshot,
        scheduler_snapshot_version: initial_scheduler_snapshot_version,
        active_keyboard_notes: (0..MAX_TRACKS).map(|_| [None; MAX_VOICES]).collect(),
        keyboard_rx: audio_keyboard_rx,
        master_recorder,
        accumulator_states: [crate::accumulator::AccumulatorRuntimeState::default(); MAX_TRACKS],
        last_playing: false,
        last_pattern: u32::MAX,
        last_num_tracks: initial_num_tracks,
        last_topology_epoch: initial_topology_epoch,
        pending_topology_delete_track: None,
        host_transport_clock: HostTransportClockRuntime::default(),
        free_patch_transport_routes: [FreePatchTransportRouteState::default(); MAX_TRACKS],
        rack_choke_last_trigger: [u64::MAX; MAX_TRACKS],
        rack_choke_note_offs: Vec::with_capacity(MAX_VOICES * 2),
        pending_accum_reset: [false; MAX_TRACKS],
        scheduled_events: Arc::clone(&scheduled_events),
        countdown_events: Vec::with_capacity(SCHEDULED_COUNTDOWN_CAPACITY),
        block_events: Vec::with_capacity(SCHEDULED_BLOCK_SCRATCH_CAPACITY),
        block_events_need_sort: false,
        current_callback_nframes: block_size,
        output_block_size: OutputBlockSizeVerifier::new(block_size),
        callback_thread_initialized: false,
        rendered_samples: Arc::clone(&rendered_samples),
        bus_effect_runtime,
        dropped_scheduled_events: 0,
        late_scheduled_events: 0,
        event_seq: 0,
        trace_audio,
        trace_callback_counter: 0,
        trace_render_probe_blocks: 0,
        trace_silent_active_callbacks: 0,
        transport_beats: 0.0,
        transport_was_playing: false,
        metronome: MetronomeState::default(),
        preview: preview::PreviewVoice::default(),
    });
    crate::scheduler::spawn_scheduler_thread(
        Arc::clone(&cb_data.state),
        sample_rate,
        block_size,
        rendered_samples,
        scheduled_events,
        live_keyboard_rx,
    );

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;

    let config = cpal::StreamConfig {
        channels: num_channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Fixed(block_size as u32),
    };

    start_cpal_output_stream(&device, &config, cb_data)
}

fn start_cpal_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut cb_data: Box<AudioCallbackData>,
) -> Result<Stream, String> {
    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                audio_callback(&mut cb_data, data);
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("Failed to build output stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("Failed to play stream: {e}"))?;

    Ok(stream)
}

/// Query the default output device, preserving the system sample rate when possible.
pub fn query_device_config() -> Result<(u32, u16), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;
    let default_config = device
        .default_output_config()
        .map_err(|e| format!("Failed to get default config: {e}"))?;
    let ranges: Vec<OutputFormatRange> = device
        .supported_output_configs()
        .map_err(|e| format!("Failed to query supported output configs: {e}"))?
        .map(|range| OutputFormatRange {
            channels: range.channels(),
            min_sample_rate: range.min_sample_rate().0,
            max_sample_rate: range.max_sample_rate().0,
            supports_f32: range.sample_format() == cpal::SampleFormat::F32,
        })
        .collect();
    let selected = select_output_config(
        default_config.sample_rate().0,
        default_config.channels(),
        ranges,
    )
    .ok_or_else(|| {
        let device_name = device
            .name()
            .unwrap_or_else(|_| "default output device".to_string());
        format!(
            "{device_name} does not support f32 output at either {} Hz or its default {} Hz rate",
            FALLBACK_SAMPLE_RATE,
            default_config.sample_rate().0
        )
    })?;

    Ok((selected.sample_rate, selected.channels))
}
