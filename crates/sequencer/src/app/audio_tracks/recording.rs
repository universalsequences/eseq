//! Recording onto audio tracks (docs/audio-tracks-spec.md §12–§13): which
//! input each track listens to, monitoring, and the takes an arrangement
//! capture pass records.
//!
//! A take rides the arrangement capture pass (`begin_song_capture_take` …
//! `finish_song_capture_take`): every armed audio track with an open input
//! records from the pass's first sample to its stop. The audio never
//! touches the audio thread — a writer thread follows the device's
//! [`InputRing`] — and the finished file becomes a source and a region the
//! same way a dropped sample does, as its own undo step, a moment after
//! the pass ends.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::audio_input::{
    feeds_back_by_default, output_epoch, spawn_take, AudioTrackInput, FinishedTake, InputDeviceInfo, InputHub,
    InputRing, InputState, MonitorMode, TakeHandle,
};
use crate::sequencer::{SongRenderMapping, TrackId};

/// How often the picker's device list is refreshed.
const DEVICE_RESCAN: Duration = Duration::from_secs(5);
/// How often a short take's live waveform is refreshed.
const PREVIEW_REFRESH: Duration = Duration::from_millis(80);

/// Where a take begins: an output sample and the song beat heard there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TakeStart {
    pub sample: u64,
    pub beat: f64,
    pub beats_per_sample: f64,
}

impl TakeStart {
    fn beat_at(&self, sample: i64) -> f64 {
        self.beat + (sample - self.sample as i64) as f64 * self.beats_per_sample
    }
}

/// One armed track's part in the pass.
struct ActiveTake {
    id: u64,
    track: TrackId,
    input: AudioTrackInput,
    ring: Arc<InputRing>,
    /// Input frame `f` was captured under output sample `f + offset`.
    offset: i64,
    handle: Option<TakeHandle>,
    /// Samples of the preview waveform registered so far.
    previewed: usize,
}

/// The sample-registry key take `id`'s live waveform is drawn from.
fn preview_key(id: u64) -> String {
    format!("recording-take#{id}")
}

/// The capture pass's audio takes.
pub(super) struct Recording {
    /// The rendered sample when the pass began.
    begun_at: u64,
    /// The pass began while the song was already playing (a punch in), so
    /// the takes start there rather than at the pass's first sample.
    punched_in: bool,
    /// The song's pass start when this pass began: the previous pass's,
    /// until the scheduler starts this one.
    stale_pass_start: u64,
    start: Option<TakeStart>,
    takes: Vec<ActiveTake>,
}

/// A stopped take whose file the writer is still finishing.
struct PendingRegion {
    id: u64,
    /// Where the take stopped, so the lane keeps showing it until it lands.
    end_beat: f64,
    track: TrackId,
    device: String,
    start: TakeStart,
    offset: i64,
}

/// Input state the App keeps beside its audio tracks.
#[derive(Default)]
pub struct AudioInputRuntime {
    /// Each audio track's input, by stable track id. Saved with the
    /// project.
    pub(crate) inputs: HashMap<TrackId, AudioTrackInput>,
    pub(crate) hub: InputHub,
    recording: Option<Recording>,
    /// Stopped takes by id, until their writer reports.
    pending: HashMap<u64, PendingRegion>,
    /// Finished takes waiting for the song to accept edits.
    ready: Vec<(PendingRegion, FinishedTake)>,
    finished: Option<(Sender<(u64, Result<FinishedTake, String>)>, Receiver<(u64, Result<FinishedTake, String>)>)>,
    next_take: u64,
    /// Milliseconds recorded audio is moved earlier, for the converters'
    /// latency the stream clocks do not see (spec §13).
    pub(crate) record_offset_ms: f32,
    /// When the takes' preview waveforms were last registered.
    previewed_at: Option<Instant>,
    /// When the hub was last asked to look for devices.
    scanned_at: Option<Instant>,
    /// Devices whose failure was already put on the status line.
    reported: Vec<String>,
    /// Bumped whenever what the track headers show of inputs changes.
    pub(crate) revision: u64,
}

/// A take as a lane draws it while it records.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioRecordingView {
    pub track: usize,
    pub start_beat: f64,
    pub end_beat: f64,
    /// The sample-registry key of its waveform so far.
    pub waveform_key: String,
    /// The take's length on the timeline, in seconds.
    pub seconds: f64,
}

/// What a track header shows of its input.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioInputView {
    pub input: Option<AudioTrackInput>,
    /// "opening", "open", "failed: …", or "" with no input chosen.
    pub status: String,
    /// Whether the input is heard right now.
    pub monitoring: bool,
}

/// The take start for a pass begun at rendered sample `begun_at`, once the
/// song position says: a punch in starts where it was pressed; a pass that
/// starts the transport starts on the pass's first sample, which the
/// scheduler publishes a moment after Play.
pub(crate) fn resolve_take_start(
    begun_at: u64,
    punched_in: bool,
    stale_pass_start: u64,
    block_start: u64,
    mapping: Option<SongRenderMapping>,
) -> Option<TakeStart> {
    let mapping = mapping?;
    let sample = if punched_in {
        begun_at.max(mapping.pass_start_sample)
    } else if mapping.pass_start_sample >= begun_at || mapping.pass_start_sample != stale_pass_start {
        mapping.pass_start_sample
    } else {
        // Still the previous pass's position.
        return None;
    };
    let beat = mapping.beat + (sample as f64 - block_start as f64) * mapping.beats_per_sample;
    Some(TakeStart { sample, beat, beats_per_sample: mapping.beats_per_sample })
}

impl App {
    fn set_status(&mut self, message: String) {
        self.editor.status_message = Some((message, Instant::now()));
    }

    /// The input audio track `track` records from, if one is chosen.
    pub fn audio_track_input(&self, track: usize) -> Option<&AudioTrackInput> {
        self.audio_input.inputs.get(&self.track_registry.id_at(track)?)
    }

    /// Choose what `track` records: `device`'s channel `left` (and `right`
    /// for a stereo input), or nothing. A new choice keeps the track's
    /// monitor mode, except that a built-in microphone starts unmonitored.
    pub fn set_audio_track_input(
        &mut self,
        track: usize,
        input: Option<(String, u16, Option<u16>)>,
    ) -> Result<(), String> {
        self.require_audio_track(track)?;
        let id = self.track_registry.id_at(track).ok_or_else(|| format!("Track {} has no id", track + 1))?;
        match input {
            None => {
                self.audio_input.inputs.remove(&id);
            }
            Some((device, left, right)) => {
                if device.is_empty() {
                    return Err("Choose an input device".to_string());
                }
                let monitor = match self.audio_input.inputs.get(&id) {
                    Some(previous) if previous.device == device => previous.monitor,
                    _ if feeds_back_by_default(&device) => MonitorMode::Off,
                    Some(previous) => previous.monitor,
                    None => MonitorMode::Auto,
                };
                // A device that failed before gets another try.
                if matches!(self.audio_input.hub.state(&device), Some(InputState::Failed(_))) {
                    self.audio_input.hub.close(&device);
                    self.audio_input.reported.retain(|reported| *reported != device);
                }
                self.audio_input.inputs.insert(id, AudioTrackInput { device, left, right, monitor });
            }
        }
        self.audio_input.revision += 1;
        self.sync_audio_inputs();
        Ok(())
    }

    pub fn set_audio_track_monitor(&mut self, track: usize, monitor: MonitorMode) -> Result<(), String> {
        self.require_audio_track(track)?;
        let input = self
            .track_registry
            .id_at(track)
            .and_then(|id| self.audio_input.inputs.get_mut(&id))
            .ok_or_else(|| "Choose an input first".to_string())?;
        input.monitor = monitor;
        self.audio_input.revision += 1;
        self.sync_audio_inputs();
        Ok(())
    }

    /// The input devices the picker lists: what the last scan found. The
    /// first scan runs once the project has an audio track
    /// ([`Self::sync_audio_inputs`]).
    pub fn audio_input_devices(&self) -> Vec<InputDeviceInfo> {
        self.audio_input.hub.known_devices()
    }

    /// Milliseconds recorded audio lands earlier than the stream clocks
    /// alone put it (the `arrangement-audio-record-offset-ms` knob's range).
    pub fn set_audio_record_offset_ms(&mut self, ms: f32) {
        if ms.is_finite() {
            self.audio_input.record_offset_ms = ms.clamp(-50.0, 50.0);
        }
    }

    /// The inputs of the audio tracks that exist, in track order, for the
    /// project file.
    pub(crate) fn saved_audio_inputs(&self) -> Vec<crate::audio_input::ProjectAudioInput> {
        self.audio_inputs_in_use()
            .into_iter()
            .map(|(_, track, input, _)| crate::audio_input::ProjectAudioInput { track, input })
            .collect()
    }

    /// Replace every track's input with a loaded project's.
    pub(crate) fn load_audio_inputs(&mut self, inputs: Vec<crate::audio_input::ProjectAudioInput>) {
        self.audio_input.inputs = inputs.into_iter().map(|saved| (saved.track, saved.input)).collect();
        self.audio_input.reported.clear();
        self.audio_input.revision += 1;
        // Takes still being written belong to the project that was open.
        let stale = self.audio_input.pending.drain().map(|(id, _)| id);
        for id in stale.chain(self.audio_input.ready.drain(..).map(|(pending, _)| pending.id)) {
            eseqlisp::audio::sample::SampleBuffer::unregister(&preview_key(id));
        }
    }

    fn track_armed(&self, track: usize) -> bool {
        self.graph.record_armed.get(track).copied().unwrap_or(false)
    }

    /// Each audio track with an input: `(track, id, input, armed)`.
    fn audio_inputs_in_use(&self) -> Vec<(usize, TrackId, AudioTrackInput, bool)> {
        (0..self.tracks.len())
            .filter(|&track| self.is_audio_track(track))
            .filter_map(|track| {
                let id = self.track_registry.id_at(track)?;
                let input = self.audio_input.inputs.get(&id)?.clone();
                Some((track, id, input, self.track_armed(track)))
            })
            .collect()
    }

    /// Open the devices armed or monitored tracks need, close the ones
    /// nothing does, and point each track's player at what it monitors.
    /// Runs every frame; cheap when nothing changed.
    pub fn sync_audio_inputs(&mut self) {
        let in_use = self.audio_inputs_in_use();
        if (0..self.tracks.len()).any(|track| self.is_audio_track(track)) {
            // Keep the picker's device list fresh (an interface plugged in
            // after launch), but only while the transport is stopped:
            // asking every device for its format is not free.
            let due = self.audio_input.scanned_at.is_none_or(|at| at.elapsed() >= DEVICE_RESCAN);
            if due && !self.state.is_playing() {
                self.audio_input.scanned_at = Some(Instant::now());
                self.audio_input.hub.rescan();
            }
        }
        if in_use.is_empty() && self.audio_input.hub.open_names().is_empty() {
            return;
        }
        let rate = self.graph.sample_rate as u32;
        // Devices a take is recording stay open whatever the arms say.
        let mut wanted: Vec<String> = self
            .audio_input
            .recording
            .iter()
            .flat_map(|recording| recording.takes.iter().map(|take| take.input.device.clone()))
            .collect();
        for (track, _, input, armed) in &in_use {
            let listens = *armed || input.monitor == MonitorMode::In;
            if listens && !wanted.contains(&input.device) {
                wanted.push(input.device.clone());
            }
            let ring = if listens {
                match self.audio_input.hub.ensure_open(&input.device, rate) {
                    InputState::Open(ring) => Some(ring),
                    InputState::Failed(error) => {
                        if !self.audio_input.reported.contains(&input.device) {
                            self.audio_input.reported.push(input.device.clone());
                            self.audio_input.revision += 1;
                            self.set_status(format!("Input '{}': {error}", input.device));
                        }
                        None
                    }
                    InputState::Opening => None,
                }
            } else {
                None
            };
            let Some(slot) = self.graph.track_node_ids.get(*track).and_then(|nodes| nodes.audio_player) else {
                continue;
            };
            let channels = ring.as_ref().map_or(0, |ring| ring.channels()) as u16;
            let heard = ring
                .as_ref()
                .filter(|_| input.monitor.hears(*armed) && input.left < channels)
                .map(|ring| (ring, input.left, input.right.filter(|right| *right < channels).unwrap_or(input.left)));
            self.audio_players.set_monitor(slot.slot, heard);
        }
        // A track that lost its input stops monitoring.
        for track in 0..self.tracks.len() {
            if in_use.iter().any(|(used, ..)| *used == track) {
                continue;
            }
            if let Some(slot) = self.graph.track_node_ids.get(track).and_then(|nodes| nodes.audio_player) {
                self.audio_players.set_monitor(slot.slot, None);
            }
        }
        for name in self.audio_input.hub.open_names() {
            if !wanted.contains(&name) {
                self.audio_input.hub.close(&name);
                self.audio_input.reported.retain(|reported| *reported != name);
                self.audio_input.revision += 1;
            }
        }
    }

    /// What each track's header shows of its input, by track index.
    pub fn audio_input_views(&self) -> Vec<Option<AudioInputView>> {
        (0..self.tracks.len())
            .map(|track| {
                if !self.is_audio_track(track) {
                    return None;
                }
                let input = self.audio_track_input(track).cloned();
                let armed = self.track_armed(track);
                let (status, open) = match input.as_ref().and_then(|input| self.audio_input.hub.state(&input.device)) {
                    Some(InputState::Open(_)) => ("open".to_string(), true),
                    Some(InputState::Opening) => ("opening".to_string(), false),
                    Some(InputState::Failed(error)) => (format!("failed: {error}"), false),
                    None => (String::new(), false),
                };
                let monitoring = open && input.as_ref().is_some_and(|input| input.monitor.hears(armed));
                Some(AudioInputView { input, status, monitoring })
            })
            .collect()
    }

    /// Changes when [`Self::audio_input_views`] may have: an input or
    /// monitor edit, a device opening or failing.
    pub fn audio_input_view_key(&self) -> (u64, usize, usize) {
        let names = self.audio_input.hub.open_names();
        let open = names.iter().filter(|name| self.audio_input.hub.ring(name).is_some()).count();
        (self.audio_input.revision, names.len(), open)
    }

    /// The input level of `track` since the last call (0..1+), the louder
    /// of its channels; `None` when its input is not open.
    pub fn audio_input_peak(&self, track: usize) -> Option<f32> {
        let input = self.audio_track_input(track)?;
        let ring = self.audio_input.hub.ring(&input.device)?;
        let left = ring.take_peak(input.left as usize);
        Some(input.right.map_or(left, |right| left.max(ring.take_peak(right as usize))))
    }

    /// The capture pass begins: every armed audio track whose input is open
    /// records it. Tracks armed without a usable input are named on the
    /// status line rather than silently skipped.
    pub(crate) fn begin_audio_takes(&mut self) {
        self.discard_audio_takes();
        // Devices open as tracks are armed; make sure of it for a track
        // armed in the same breath as Record.
        self.sync_audio_inputs();
        let mut takes = Vec::new();
        let mut skipped = Vec::new();
        for (track, id, input, armed) in self.audio_inputs_in_use() {
            if !armed {
                continue;
            }
            match self.audio_input.hub.ring(&input.device) {
                Some(ring) if (input.left as usize) < ring.channels() => {
                    self.audio_input.next_take += 1;
                    takes.push(ActiveTake {
                        id: self.audio_input.next_take,
                        track: id,
                        input,
                        ring,
                        offset: 0,
                        handle: None,
                        previewed: 0,
                    });
                }
                Some(_) => skipped.push(format!("track {} (its input has no channel {})", track + 1, input.left + 1)),
                None => skipped.push(format!("track {} ('{}' is not open yet)", track + 1, input.device)),
            }
        }
        if !skipped.is_empty() {
            self.set_status(format!("Not recording audio on {}", skipped.join(", ")));
        }
        if takes.is_empty() {
            return;
        }
        let (song, _) = self.state.audio_render_clock();
        self.audio_input.recording = Some(Recording {
            begun_at: self.state.audio_rendered_sample(),
            punched_in: self.state.is_playing() && song.render_mapping(self.state.audio_rendered_sample()).is_some(),
            stale_pass_start: song.pass_start_sample(),
            start: None,
            takes,
        });
    }

    /// Put audio track `track` mid-take without a device or a transport
    /// (UI capture fixtures): `beats` beats of plucked notes recorded from
    /// `start_beat`, as the lane shows a take while it records.
    pub fn stage_audio_take(&mut self, track: usize, start_beat: f64, beats: f64) -> Result<(), String> {
        self.require_audio_track(track)?;
        let rate = self.graph.sample_rate as u32;
        let samples_per_beat = f64::from(rate) * self.seconds_per_beat();
        let frames = ((beats * samples_per_beat) as usize).min(3 * rate as usize);
        let device = "Capture Input";
        let ring = self.audio_input.hub.insert_virtual(device, 1, rate);
        output_epoch().set(0);
        ring.epoch().set(0);
        let pluck = (samples_per_beat / 2.0) as usize;
        let audio: Vec<f32> = (0..frames)
            .map(|n| {
                let (note, t) = (n / pluck.max(1), (n % pluck.max(1)) as f32 / rate as f32);
                let hz = 110.0 * 2f32.powf([0.0, 7.0, 12.0, 3.0][note % 4] / 12.0);
                0.8 * (-6.0 * t).exp() * (std::f32::consts::TAU * hz * t).sin()
            })
            .collect();
        ring.write(&audio, None);
        if let Some(armed) = self.graph.record_armed.get_mut(track) {
            *armed = true;
        }
        // The UI's arm flags follow the graph's on the next tick.
        self.record_arm_sync_pending = true;
        self.set_audio_track_input(track, Some((device.to_string(), 0, None)))?;
        self.begin_audio_takes();
        self.set_audio_take_start(TakeStart { sample: 0, beat: start_beat, beats_per_sample: 1.0 / samples_per_beat });
        self.state.set_audio_rendered_sample(frames as u64);
        // Let the writer read the ring and build its waveform.
        for _ in 0..40 {
            self.audio_input.previewed_at = None;
            self.drive_audio_recording();
            let shown = self.audio_recording_views().first().and_then(|view| {
                eseqlisp::audio::sample::get_registered_sample(&view.waveform_key)
            });
            if shown.is_some_and(|sample| sample.frames + 2 >= frames / 50) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(())
    }

    /// Pin the pass's take start (tests and fixtures, where no scheduler
    /// publishes one).
    pub(crate) fn set_audio_take_start(&mut self, start: TakeStart) {
        if let Some(recording) = &mut self.audio_input.recording {
            recording.start = Some(start);
        }
    }

    /// Per frame: start the writers once the pass's first sample is known,
    /// and turn finished takes into regions.
    pub fn drive_audio_recording(&mut self) {
        if !self.audio_takes_pending() {
            return;
        }
        let rendered = self.state.audio_rendered_sample();
        // The output buffer, as the transport's device record latency.
        let block = (f32::from_bits(self.state.transport.record_latency_seconds.load(std::sync::atomic::Ordering::Relaxed))
            * self.graph.sample_rate as f32) as u32;
        let store = self.audio_tracks.store_dir();
        let record_offset_ms = self.audio_input.record_offset_ms;
        let mapping = self.state.audio_render_clock().0.render_mapping(rendered);
        let runtime = &mut self.audio_input;
        if let Some(recording) = &mut runtime.recording {
            if recording.start.is_none() {
                recording.start =
                    resolve_take_start(
                        recording.begun_at,
                        recording.punched_in,
                        recording.stale_pass_start,
                        rendered,
                        mapping,
                    );
            }
            if let Some(start) = recording.start {
                let done = runtime.finished.get_or_insert_with(channel).0.clone();
                for take in recording.takes.iter_mut().filter(|take| take.handle.is_none()) {
                    let rate = f64::from(take.ring.sample_rate());
                    let latency = (f64::from(record_offset_ms) * rate / 1000.0).round() as i64;
                    take.offset = take.ring.output_offset(output_epoch(), rendered, block) - latency;
                    let first_frame = (start.sample as i64 - take.offset).max(0) as u64;
                    take.handle = Some(spawn_take(
                        Arc::clone(&take.ring),
                        take.input.left as usize,
                        take.input.right.map(usize::from),
                        first_frame,
                        store.clone(),
                        take.id,
                        done.clone(),
                    ));
                }
            }
        }
        self.refresh_take_previews();
        self.collect_finished_takes();
    }

    /// Register each recording take's waveform as it grows: what the lane
    /// draws under the playhead. A dozen times a second at first; each
    /// refresh rebuilds the whole waveform, so a long take refreshes less
    /// often (about three times a second ten minutes in).
    fn refresh_take_previews(&mut self) {
        let runtime = &mut self.audio_input;
        let Some(recording) = &mut runtime.recording else {
            return;
        };
        let longest = recording.takes.iter().map(|take| take.previewed).max().unwrap_or(0);
        let period = PREVIEW_REFRESH + Duration::from_micros(longest as u64 / 4);
        if runtime.previewed_at.is_some_and(|at| at.elapsed() < period) {
            return;
        }
        runtime.previewed_at = Some(Instant::now());
        for take in &mut recording.takes {
            let Some(preview) = take.handle.as_ref().and_then(|handle| handle.preview_past(take.previewed)) else {
                continue;
            };
            take.previewed = preview.len();
            eseqlisp::audio::sample::SampleBuffer::register_stereo(
                std::path::Path::new(&preview_key(take.id)),
                crate::audio_input::preview_rate(take.ring.sample_rate()),
                if take.input.right.is_some() { 2 } else { 1 },
                preview,
            );
        }
    }

    /// The capture pass ends: every take stops at the sample playing now.
    /// Their regions land when the writers finish
    /// ([`Self::drive_audio_recording`]).
    pub(crate) fn finish_audio_takes(&mut self) {
        // A pass stopped within a frame of starting still gets its takes.
        self.drive_audio_recording();
        let Some(recording) = self.audio_input.recording.take() else {
            return;
        };
        let stop = self.state.audio_rendered_sample() as i64;
        let Some(start) = recording.start else {
            return;
        };
        for take in recording.takes {
            let Some(handle) = take.handle else { continue };
            handle.stop_at((stop - take.offset).max(0) as u64);
            self.audio_input.pending.insert(
                take.id,
                PendingRegion {
                    id: take.id,
                    end_beat: start.beat_at(stop),
                    track: take.track,
                    device: take.input.device,
                    start,
                    offset: take.offset,
                },
            );
        }
    }

    /// Throw the pass's takes away (Cancel, a project load).
    pub(crate) fn discard_audio_takes(&mut self) {
        if let Some(recording) = self.audio_input.recording.take() {
            for take in recording.takes {
                if let Some(handle) = take.handle {
                    handle.cancel();
                }
                eseqlisp::audio::sample::SampleBuffer::unregister(&preview_key(take.id));
            }
        }
    }

    /// Whether any audio take is being written or waiting to land.
    pub fn audio_takes_pending(&self) -> bool {
        self.audio_input.recording.is_some()
            || !self.audio_input.pending.is_empty()
            || !self.audio_input.ready.is_empty()
    }

    /// The takes the lanes draw as recordings: the ones recording now,
    /// growing with the playhead, and the stopped ones until their regions
    /// land.
    pub fn audio_recording_views(&self) -> Vec<AudioRecordingView> {
        let host_rate = self.graph.sample_rate as f64;
        let view = |id: u64, track: TrackId, start: &TakeStart, end_beat: f64| {
            let (start_beat, end_beat) = (start.beat.max(0.0), end_beat.max(0.0));
            Some(AudioRecordingView {
                track: self.track_registry.index_of(track)?,
                start_beat,
                end_beat,
                waveform_key: preview_key(id),
                seconds: (end_beat - start_beat) / start.beats_per_sample / host_rate,
            })
        };
        let mut views = Vec::new();
        if let Some((recording, start)) =
            self.audio_input.recording.as_ref().and_then(|recording| Some((recording, recording.start?)))
        {
            let now = start.beat_at(self.state.audio_rendered_sample() as i64);
            views.extend(recording.takes.iter().filter_map(|take| view(take.id, take.track, &start, now)));
        }
        let stopped = self.audio_input.pending.values().chain(self.audio_input.ready.iter().map(|(pending, _)| pending));
        views.extend(stopped.filter_map(|pending| view(pending.id, pending.track, &pending.start, pending.end_beat)));
        views
    }

    fn collect_finished_takes(&mut self) {
        let arrived: Vec<(u64, Result<FinishedTake, String>)> = match &self.audio_input.finished {
            Some((_, finished)) => finished.try_iter().collect(),
            None => Vec::new(),
        };
        for (id, result) in arrived {
            let Some(pending) = self.audio_input.pending.remove(&id) else {
                continue;
            };
            match result {
                Ok(take) => self.audio_input.ready.push((pending, take)),
                Err(error) => {
                    eseqlisp::audio::sample::SampleBuffer::unregister(&preview_key(pending.id));
                    self.set_status(format!("Audio take failed: {error}"));
                }
            }
        }
        // The song refuses edits while a capture pass runs (a punch out
        // followed at once by a punch in): the regions wait for it.
        if self.audio_input.ready.is_empty() || self.song_edits_locked() {
            return;
        }
        for (pending, take) in std::mem::take(&mut self.audio_input.ready) {
            eseqlisp::audio::sample::SampleBuffer::unregister(&preview_key(pending.id));
            let message = match self.commit_audio_take(&pending, take) {
                Ok(message) => message,
                Err(error) => format!("Audio take failed: {error}"),
            };
            self.set_status(message);
        }
    }

    /// A finished take becomes a source and a region on its track, placed
    /// under the samples that were playing while it was captured. One undo
    /// step; like any region landing on a lane it truncates what is under
    /// it (spec §12.7).
    fn commit_audio_take(&mut self, pending: &PendingRegion, take: FinishedTake) -> Result<String, String> {
        let track = self
            .track_registry
            .index_of(pending.track)
            .ok_or_else(|| "its track was deleted".to_string())?;
        if take.peak == 0.0 {
            // An input the system mutes delivers exact zeros (spec §12.6).
            let _ = std::fs::remove_file(self.audio_tracks.store_dir().join(&take.file));
            return Ok(format!(
                "'{}' delivered silence: nothing was recorded. On macOS, allow microphone access in \
                 System Settings › Privacy & Security › Microphone",
                pending.device
            ));
        }
        let rate = take.sample_rate;
        // Audio captured before the song's first beat has nowhere to go.
        let mut start_beat = pending.start.beat_at(take.first_frame as i64 + pending.offset);
        let mut source_offset = 0u64;
        let spb = self.seconds_per_beat();
        if start_beat < 0.0 {
            source_offset = ((-start_beat * spb) * f64::from(rate)).round() as u64;
            start_beat = 0.0;
        }
        if source_offset >= take.frames {
            return Err("it ended before the song began".to_string());
        }
        let meta = AudioSourceMeta {
            file: take.file,
            name: format!("{} take {}", self.tracks[track], pending.id),
            sample_rate: rate,
            channels: take.channels,
            frames: take.frames,
        };
        if !self.audio_tracks.pcm.contains_key(&meta.file) {
            self.audio_tracks.insert_pcm(&meta, take.stereo, rate);
        }
        let length = audio_frames_to_beats(take.frames - source_offset, rate, self.bpm() as f64);
        let end_beat = start_beat + length;
        let seconds = take.frames as f64 / f64::from(rate);
        self.edit_arrangement("Record audio", |arrangement, _scenes| {
            let source = arrangement.add_audio_source(meta)?;
            let id = arrangement.allocate_clip_id()?;
            arrangement.occlude_audio_span(track, start_beat, end_beat, spb)?;
            let mut region = AudioRegion::new(id, source, start_beat, end_beat);
            region.source_offset = source_offset;
            arrangement.insert_audio_region_sorted(track, region);
            arrangement.end_beat = arrangement.end_beat.max(end_beat);
            Ok(())
        })?;
        self.sync_audio_track_playback();
        let lost = if take.lost_frames > 0 {
            format!(" ({} frames were lost to a stalled disk)", take.lost_frames)
        } else {
            String::new()
        };
        Ok(format!("Recorded {seconds:.1}s of audio on track {}{lost}", track + 1))
    }
}
