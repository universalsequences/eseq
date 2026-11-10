//! Input devices: one cpal input stream per open device, each filling an
//! [`InputRing`] (docs/audio-tracks-spec.md §12.1–§12.4).
//!
//! cpal streams are not `Send` on every platform and opening one can block
//! (a device rate change, the microphone permission prompt), so a worker
//! thread owns them; the control thread asks for devices by name and reads
//! what came of it.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::ring::{stream_nanos, InputRing};

/// An input device as the picker lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDeviceInfo {
    pub name: String,
    pub channels: u16,
}

/// What became of a device the hub was asked to open.
#[derive(Clone)]
pub enum InputState {
    Opening,
    Open(Arc<InputRing>),
    Failed(String),
}

enum Command {
    Open { name: String, sample_rate: u32 },
    Close { name: String },
    Scan,
}

#[derive(Default)]
struct Shared {
    states: HashMap<String, InputState>,
    devices: Vec<InputDeviceInfo>,
}

/// The control thread's handle on every input device.
pub struct InputHub {
    shared: Arc<Mutex<Shared>>,
    /// The worker, started with the first request that needs a device.
    worker: Option<Sender<Command>>,
}

impl Default for InputHub {
    fn default() -> Self {
        Self { shared: Arc::new(Mutex::new(Shared::default())), worker: None }
    }
}

impl InputHub {
    fn send(&mut self, command: Command) {
        if self.worker.is_none() {
            let (tx, rx) = channel::<Command>();
            let shared = Arc::clone(&self.shared);
            let spawned = std::thread::Builder::new().name("audio-input".to_string()).spawn(move || {
                // Streams live here until closed; dropping one stops it.
                let mut streams: HashMap<String, cpal::Stream> = HashMap::new();
                // A device's ring outlives its stream (a track's player may
                // still read it), so a device opened again fills the same
                // one rather than a new one each time it is armed.
                let mut rings: HashMap<String, Arc<InputRing>> = HashMap::new();
                while let Ok(command) = rx.recv() {
                    match command {
                        Command::Open { name, sample_rate } => {
                            let state = match open_stream(&name, sample_rate, rings.get(&name)) {
                                Ok((stream, ring)) => {
                                    streams.insert(name.clone(), stream);
                                    rings.insert(name.clone(), Arc::clone(&ring));
                                    InputState::Open(ring)
                                }
                                Err(error) => {
                                    eprintln!("audio-input: {name}: {error}");
                                    InputState::Failed(error)
                                }
                            };
                            shared.lock().unwrap().states.insert(name, state);
                        }
                        Command::Close { name } => {
                            streams.remove(&name);
                        }
                        Command::Scan => {
                            let mut devices = scan_devices();
                            // Virtual devices are not the host's to find.
                            let held = shared.lock().unwrap();
                            for (name, state) in &held.states {
                                if let (InputState::Open(ring), false) =
                                    (state, devices.iter().any(|device| device.name == *name))
                                {
                                    devices.push(InputDeviceInfo { name: name.clone(), channels: ring.channels() as u16 });
                                }
                            }
                            drop(held);
                            shared.lock().unwrap().devices = devices;
                        }
                    }
                }
            });
            if spawned.is_ok() {
                self.worker = Some(tx);
            }
        }
        if let Some(worker) = &self.worker {
            let _ = worker.send(command);
        }
    }

    /// The devices the last scan found.
    pub fn known_devices(&self) -> Vec<InputDeviceInfo> {
        self.shared.lock().unwrap().devices.clone()
    }

    /// Look for devices again; [`Self::known_devices`] changes when the
    /// scan lands.
    pub fn rescan(&mut self) {
        self.send(Command::Scan);
    }

    /// Make sure `name` is open at `sample_rate`, and say how far along it
    /// is. A failed device is not retried until [`Self::close`] forgets it.
    pub fn ensure_open(&mut self, name: &str, sample_rate: u32) -> InputState {
        if let Some(state) = self.shared.lock().unwrap().states.get(name) {
            return state.clone();
        }
        self.shared.lock().unwrap().states.insert(name.to_string(), InputState::Opening);
        self.send(Command::Open { name: name.to_string(), sample_rate });
        InputState::Opening
    }

    pub fn state(&self, name: &str) -> Option<InputState> {
        self.shared.lock().unwrap().states.get(name).cloned()
    }

    /// The ring of `name`, once it is open.
    pub fn ring(&self, name: &str) -> Option<Arc<InputRing>> {
        match self.state(name)? {
            InputState::Open(ring) => Some(ring),
            _ => None,
        }
    }

    /// Stop `name`'s stream and forget it (and any failure, so the next
    /// request tries again). Its ring stays valid for whoever holds it.
    pub fn close(&mut self, name: &str) {
        if self.shared.lock().unwrap().states.remove(name).is_some() {
            self.send(Command::Close { name: name.to_string() });
        }
    }

    /// Every device the hub was asked for and has not closed: open,
    /// opening or failed.
    pub fn open_names(&self) -> Vec<String> {
        self.shared.lock().unwrap().states.keys().cloned().collect()
    }

    /// A device that exists only as a ring the caller fills (tests and UI
    /// capture fixtures).
    pub fn insert_virtual(&mut self, name: &str, channels: usize, sample_rate: u32) -> Arc<InputRing> {
        let ring = Arc::new(InputRing::new(channels, sample_rate));
        let mut shared = self.shared.lock().unwrap();
        shared.states.insert(name.to_string(), InputState::Open(Arc::clone(&ring)));
        if !shared.devices.iter().any(|device| device.name == name) {
            shared.devices.push(InputDeviceInfo { name: name.to_string(), channels: channels as u16 });
        }
        ring
    }
}

fn scan_devices() -> Vec<InputDeviceInfo> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    let mut found: Vec<InputDeviceInfo> = devices
        .filter_map(|device| {
            let name = device.name().ok()?;
            let channels = device.default_input_config().ok()?.channels();
            (channels > 0).then_some(InputDeviceInfo { name, channels })
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|a, b| a.name == b.name);
    found
}

/// The input stream's capture time for the buffer it just delivered.
fn capture_nanos(info: &cpal::InputCallbackInfo) -> i64 {
    let timestamp = info.timestamp();
    // CoreAudio stamps the buffer itself (`callback` is the host time of
    // its first frame); cpal then subtracts a buffer it guesses at. The
    // other backends derive `capture` from the driver's reported delay.
    #[cfg(target_os = "macos")]
    let instant = timestamp.callback;
    #[cfg(not(target_os = "macos"))]
    let instant = timestamp.capture;
    stream_nanos(instant)
}

/// Open `name` at `sample_rate`, filling `kept` (the ring of its last
/// stream) when that still fits the device.
fn open_stream(
    name: &str,
    sample_rate: u32,
    kept: Option<&Arc<InputRing>>,
) -> Result<(cpal::Stream, Arc<InputRing>), String> {
    let host = cpal::default_host();
    let device = host
        .input_devices()
        .map_err(|error| format!("could not list input devices: {error}"))?
        .find(|device| device.name().is_ok_and(|found| found == name))
        .ok_or_else(|| "the input device is not connected".to_string())?;
    let default = device
        .default_input_config()
        .map_err(|error| format!("no usable input format: {error}"))?;
    // The engine does not resample input (spec §12.4): the device runs at
    // the graph rate or does not open. cpal moves a CoreAudio device to the
    // requested rate when the device supports it.
    let ranges: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_input_configs()
        .map_err(|error| format!("could not read input formats: {error}"))?
        .collect();
    let rate = cpal::SampleRate(sample_rate);
    let range = ranges
        .iter()
        .filter(|range| range.min_sample_rate() <= rate && rate <= range.max_sample_rate())
        .min_by_key(|range| {
            (
                range.channels() != default.channels(),
                range.sample_format() != cpal::SampleFormat::F32,
            )
        })
        .ok_or_else(|| {
            format!(
                "the input cannot run at the engine's {sample_rate} Hz (it runs at {} Hz); \
                 set both devices to one rate in Audio MIDI Setup",
                default.sample_rate().0
            )
        })?;
    let channels = range.channels() as usize;
    let format = range.sample_format();
    let config = cpal::StreamConfig {
        channels: range.channels(),
        sample_rate: rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let ring = match kept.filter(|kept| kept.channels() == channels && kept.sample_rate() == sample_rate) {
        Some(kept) => {
            // A new stream is a new clock relation.
            kept.epoch().clear();
            Arc::clone(kept)
        }
        None => Arc::new(InputRing::new(channels, sample_rate)),
    };
    let on_error = {
        let name = name.to_string();
        move |error| eprintln!("audio-input: {name}: stream error: {error}")
    };
    let stream = match format {
        cpal::SampleFormat::F32 => {
            let ring = Arc::clone(&ring);
            device.build_input_stream(
                &config,
                move |data: &[f32], info: &cpal::InputCallbackInfo| {
                    ring.write(data, Some(capture_nanos(info)));
                },
                on_error,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let ring = Arc::clone(&ring);
            let mut scratch = vec![0.0f32; 1 << 16];
            device.build_input_stream(
                &config,
                move |data: &[i16], info: &cpal::InputCallbackInfo| {
                    write_converted(&ring, &mut scratch, data, capture_nanos(info), |sample| {
                        f32::from(sample) / 32_768.0
                    });
                },
                on_error,
                None,
            )
        }
        cpal::SampleFormat::I32 => {
            let ring = Arc::clone(&ring);
            let mut scratch = vec![0.0f32; 1 << 16];
            device.build_input_stream(
                &config,
                move |data: &[i32], info: &cpal::InputCallbackInfo| {
                    write_converted(&ring, &mut scratch, data, capture_nanos(info), |sample| {
                        sample as f32 / 2_147_483_648.0
                    });
                },
                on_error,
                None,
            )
        }
        other => return Err(format!("unsupported input sample format {other:?}")),
    }
    .map_err(|error| format!("could not open the input: {error}"))?;
    stream.play().map_err(|error| format!("could not start the input: {error}"))?;
    Ok((stream, ring))
}

/// Convert a device buffer through `scratch` (sized once, so the callback
/// never allocates) and append it, in pieces when it is longer.
fn write_converted<T: Copy>(
    ring: &InputRing,
    scratch: &mut [f32],
    data: &[T],
    capture_nanos: i64,
    convert: impl Fn(T) -> f32,
) {
    let whole = scratch.len() - scratch.len() % ring.channels();
    let mut nanos = Some(capture_nanos);
    for piece in data.chunks(whole.max(ring.channels())) {
        for (out, sample) in scratch.iter_mut().zip(piece) {
            *out = convert(*sample);
        }
        // Only the first piece starts at the stamped time.
        ring.write(&scratch[..piece.len()], nanos.take());
    }
}
