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
    keyboard_rx: std::sync::mpsc::Receiver<crate::sequencer::LiveInputEvent>,
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
    let scheduled_events = Arc::new(ScheduledEventQueue::new());
    let rendered_samples = Arc::new(AtomicU64::new(0));
    let (audio_keyboard_tx, audio_keyboard_rx) = std::sync::mpsc::channel();
    let (live_keyboard_tx, live_keyboard_rx) = std::sync::mpsc::channel();
    {
        let state_for_keyboard_router = Arc::clone(&state);
        let _ = std::thread::Builder::new()
            .name("keyboard-midi-fx-router".to_string())
            .spawn(move || {
                while let Ok(event) = keyboard_rx.recv() {
                    let crate::sequencer::LiveInputEvent::Note(trigger) = event else {
                        let _ = audio_keyboard_tx.send(event);
                        continue;
                    };
                    if trigger.note_off {
                        let _ = live_keyboard_tx.send(trigger);
                        let _ = audio_keyboard_tx.send(event);
                        continue;
                    }
                    let use_midi_fx = trigger.track
                        < state_for_keyboard_router.active_track_count()
                        && !state_for_keyboard_router.pattern.track_params[trigger.track]
                            .midi_fx_chain()
                            .is_empty();
                    if use_midi_fx {
                        let _ = audio_keyboard_tx.send(crate::sequencer::LiveInputEvent::SourceNote(trigger));
                        let _ = live_keyboard_tx.send(trigger);
                    } else {
                        let _ = audio_keyboard_tx.send(event);
                    }
                }
            });
    }
    let cb_data = new_audio_callback_data(
        lg, state, sample_rate, num_channels, block_size, master_recorder,
        audio_keyboard_rx, bus_effect_runtime,
        Arc::clone(&scheduled_events), Arc::clone(&rendered_samples),
    );
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

    start_cpal_output_stream(&device, &config, block_size, cb_data)
}

fn start_cpal_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    block_size: usize,
    mut cb_data: Box<AudioCallbackData>,
) -> Result<Stream, String> {
    #[cfg(feature = "audio-rtsan")]
    rtsan_standalone::ensure_initialized();
    let channels = cb_data.num_channels;
    // CPAL honors `BufferSize::Fixed` only as a hint on ALSA; PipeWire answers a
    // 512-frame request with whatever `avail_update` reports (235 frames on the
    // Linux workstation). Render exact graph blocks and serve the device out of
    // them so every node — spectral DGenLisp effects above all — sees the block
    // size it was compiled for (eseq-linux.73).
    let mut blocks = FixedOutputBlocks::new(block_size, channels);
    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                #[cfg(feature = "audio-heap-audit")]
                let _heap = crate::heap_audit::AudioScope::enter();
                // Cover the complete application callback, including first-call
                // setup, fixed-block adaptation, diagnostics and local drops.
                #[cfg(feature = "audio-rtsan")]
                let _realtime = super::rt_audit::scope();
                if let Some(observation) =
                    cb_data.output_block_size.observe(data.len() / channels.max(1))
                {
                    match observation {
                        OutputBlockSizeObservation::Matched { frames } => {
                            eprintln!(
                                "audio: output callback block size verified at {frames} frames"
                            );
                        }
                        OutputBlockSizeObservation::Mismatched { requested, actual } => {
                            eprintln!(
                                "audio: output callback uses {actual} frames after requesting {requested}; rendering fixed {requested}-frame graph blocks"
                            );
                        }
                    }
                }
                blocks.serve(data, |block| audio_callback(&mut cb_data, block));
                #[cfg(feature = "audio-experiments")]
                if super::experiment::silence_device() {
                    data.fill(0.0);
                }
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
    #[cfg(target_os = "linux")]
    let preferred_sample_rate = pipewire::default_output_graph_rate();
    #[cfg(not(target_os = "linux"))]
    let preferred_sample_rate = None;

    let selected = select_output_config_with_preferred_rate(
        preferred_sample_rate,
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

    if let Some(graph_rate) = preferred_sample_rate {
        if selected.sample_rate == graph_rate {
            eprintln!("audio: matched output to the PipeWire graph rate at {graph_rate} Hz");
        } else {
            eprintln!(
                "audio: PipeWire graph rate {graph_rate} Hz is unsupported by the default output; using {} Hz",
                selected.sample_rate
            );
        }
    }

    Ok((selected.sample_rate, selected.channels))
}
