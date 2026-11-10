use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Duration;

use super::ring::RING_SECONDS;
use super::*;

/// `frames` stereo frames counting up from `from`: left is the frame
/// number, right its negative.
fn ramp(from: usize, frames: usize) -> Vec<f32> {
    (from..from + frames).flat_map(|n| [n as f32, -(n as f32)]).collect()
}

#[test]
fn readers_follow_the_ring_by_frame_number() {
    let ring = InputRing::new(2, 1_000);
    ring.write(&ramp(0, 300), None);
    ring.write(&ramp(300, 200), None);
    assert_eq!(ring.written(), 500);
    assert_eq!(ring.block_frames(), 200);
    assert_eq!((ring.sample(0, 0), ring.sample(499, 0), ring.sample(499, 1)), (0.0, 499.0, -499.0));
    // A channel the device does not have reads its last one.
    assert_eq!(ring.sample(7, 5), -7.0);
    assert_eq!(ring.oldest(), 0);
    // Past its length the ring forgets the oldest audio, and says so.
    let total = RING_SECONDS * 1_000 + 20_000;
    ring.write(&ramp(500, total), None);
    assert!(ring.oldest() > 0);
    let oldest = ring.oldest();
    assert_eq!(ring.sample(oldest, 0), oldest as f32);
    assert_eq!(ring.sample(ring.written() - 1, 0), (500 + total - 1) as f32);
}

#[test]
fn the_meter_reads_each_channels_peak_once() {
    let ring = InputRing::new(2, 1_000);
    ring.write(&[0.25, -0.5, -0.75, 0.1], None);
    assert_eq!((ring.take_peak(0), ring.take_peak(1)), (0.75, 0.5));
    assert_eq!(ring.take_peak(0), 0.0, "read once");
    assert_eq!(ring.take_peak(9), 0.0);
}

#[test]
fn stream_clocks_place_an_input_frame_under_the_sample_it_was_played_to() {
    let ring = InputRing::new(1, 48_000);
    let output = StreamEpoch::default();
    // Without clocks: the newest frame is one output and one input buffer
    // behind what is rendered.
    ring.write(&[0.0; 256], None);
    assert_eq!(ring.output_offset(&output, 10_000, 512), 10_000 - 256 - 512 - 256);
    // The input's frame 0 was captured 10 ms after the output's sample 0
    // played: frame f was captured under sample f + 480.
    output.observe(5_000_000, 0, 48_000);
    ring.epoch().set(15_000_000);
    assert_eq!(ring.output_offset(&output, 10_000, 512), 480);
    // A callback's stamp is for its first frame, whatever its number.
    let later = StreamEpoch::default();
    later.observe(5_000_000 + 1_000_000_000, 48_000, 48_000);
    assert_eq!(later.get(), Some(5_000_000));
}

#[test]
fn an_epoch_smooths_jitter_and_follows_a_jump() {
    let epoch = StreamEpoch::default();
    assert_eq!(epoch.get(), None);
    epoch.observe(1_000_000, 0, 48_000);
    epoch.observe(1_080_000, 0, 48_000);
    assert_eq!(epoch.get(), Some(1_010_000), "an eighth of the way to a jittered stamp");
    epoch.observe(90_000_000, 0, 48_000);
    assert_eq!(epoch.get(), Some(90_000_000), "a dropped buffer moves the clock at once");
}

#[test]
fn a_take_is_the_ring_from_its_first_frame_to_its_stop() {
    let dir = tempfile::tempdir().unwrap();
    let ring = Arc::new(InputRing::new(2, 48_000));
    ring.write(&ramp(0, 1_000), None);
    let (done, finished) = channel();
    // Started late: the take still begins at frame 100.
    let take = spawn_take(Arc::clone(&ring), 1, None, 100, dir.path().to_path_buf(), 7u8, done);
    ring.write(&ramp(1_000, 1_000), None);
    take.stop_at(1_500);
    let (tag, result) = finished.recv_timeout(Duration::from_secs(5)).unwrap();
    let take = result.unwrap();
    assert_eq!(tag, 7);
    assert_eq!((take.first_frame, take.frames, take.channels, take.sample_rate), (100, 1_400, 1, 48_000));
    assert_eq!(take.lost_frames, 0);
    assert_eq!(take.peak, 1_499.0);
    // A mono take plays on both sides.
    assert_eq!(&take.stereo[..4], &[-100.0, -100.0, -101.0, -101.0]);
    // The file is in the store under its hash, and holds the same audio.
    let path = dir.path().join(&take.file);
    let mut reader = hound::WavReader::open(&path).unwrap();
    assert_eq!(reader.spec().channels, 1);
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples.len(), 1_400);
    assert_eq!((samples[0], samples[1_399]), (-100.0, -1_499.0));
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(take.file, format!("{:x}.wav", <sha2::Sha256 as sha2::Digest>::digest(&bytes)));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no partial file is left");
}

#[test]
fn a_stereo_take_waits_for_frames_the_device_has_not_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let ring = Arc::new(InputRing::new(2, 48_000));
    let (done, finished) = channel();
    let take = spawn_take(Arc::clone(&ring), 0, Some(1), 0, dir.path().to_path_buf(), (), done);
    take.stop_at(600);
    std::thread::sleep(Duration::from_millis(30));
    ring.write(&ramp(0, 1_000), None);
    let take = finished.recv_timeout(Duration::from_secs(5)).unwrap().1.unwrap();
    assert_eq!((take.frames, take.channels), (600, 2));
    assert_eq!(&take.stereo[take.stereo.len() - 2..], &[599.0, -599.0]);
}

#[test]
fn a_cancelled_take_leaves_nothing_and_an_empty_one_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let ring = Arc::new(InputRing::new(1, 48_000));
    ring.write(&[0.5; 64], None);
    let (done, finished) = channel();
    let cancelled = spawn_take(Arc::clone(&ring), 0, None, 0, dir.path().to_path_buf(), 1, done.clone());
    cancelled.cancel();
    let empty = spawn_take(Arc::clone(&ring), 0, None, 64, dir.path().to_path_buf(), 2, done);
    empty.stop_at(64);
    let (tag, result) = finished.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(tag, 2, "the cancelled take reports nothing");
    assert!(result.is_err());
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn monitor_modes_and_input_labels() {
    assert!(!MonitorMode::Off.hears(true));
    assert!(MonitorMode::Auto.hears(true) && !MonitorMode::Auto.hears(false));
    assert!(MonitorMode::In.hears(false));
    for mode in [MonitorMode::Off, MonitorMode::Auto, MonitorMode::In] {
        assert_eq!(MonitorMode::parse(mode.name()), Some(mode));
    }
    let mono = AudioTrackInput { device: "Scarlett".into(), left: 2, right: None, monitor: MonitorMode::Auto };
    assert_eq!(mono.channel_label(), "3");
    let stereo = AudioTrackInput { right: Some(1), left: 0, ..mono.clone() };
    assert_eq!(stereo.channel_label(), "1/2");
    assert!(feeds_back_by_default("MacBook Pro Microphone"));
    assert!(!feeds_back_by_default("Scarlett 2i2 USB"));
    // The project form is flat, and an older file's input has no monitor.
    let saved = ProjectAudioInput { track: crate::sequencer::TrackId(4), input: stereo };
    let json = serde_json::to_value(&saved).unwrap();
    assert_eq!(json, serde_json::json!({"track": 4, "device": "Scarlett", "left": 0, "right": 1, "monitor": "auto"}));
    let legacy: ProjectAudioInput =
        serde_json::from_value(serde_json::json!({"track": 4, "device": "Scarlett", "left": 0})).unwrap();
    assert_eq!((legacy.input.right, legacy.input.monitor), (None, MonitorMode::Auto));
}

#[test]
fn a_virtual_device_is_open_at_once_and_closing_forgets_it() {
    let mut hub = InputHub::default();
    let ring = hub.insert_virtual("Loop", 2, 48_000);
    assert!(Arc::ptr_eq(&hub.ring("Loop").unwrap(), &ring));
    assert!(matches!(hub.ensure_open("Loop", 48_000), InputState::Open(_)));
    assert_eq!(hub.known_devices(), vec![InputDeviceInfo { name: "Loop".into(), channels: 2 }]);
    hub.close("Loop");
    assert!(hub.ring("Loop").is_none());
    assert!(hub.open_names().is_empty());
}

/// Opens every real input device on this machine. Run by hand
/// (`--run-ignored only`): it needs hardware, and macOS may ask for
/// microphone access on the terminal's behalf.
#[test]
#[ignore = "needs audio hardware"]
fn real_input_devices_open_and_deliver_stamped_frames() {
    let mut hub = InputHub::default();
    hub.rescan();
    std::thread::sleep(Duration::from_millis(1_500));
    let devices = hub.known_devices();
    eprintln!("input devices: {devices:?}");
    assert!(!devices.is_empty(), "no input devices found");
    for device in devices {
        let default_rate = {
            use cpal::traits::{DeviceTrait, HostTrait};
            cpal::default_host()
                .input_devices()
                .unwrap()
                .find(|found| found.name().is_ok_and(|name| name == device.name))
                .and_then(|found| found.default_input_config().ok())
                .map_or(48_000, |config| config.sample_rate().0)
        };
        hub.ensure_open(&device.name, default_rate);
        std::thread::sleep(Duration::from_millis(1_500));
        match hub.state(&device.name) {
            Some(InputState::Open(ring)) => {
                let before = (ring.written(), ring.epoch().get());
                std::thread::sleep(Duration::from_millis(1_000));
                let after = (ring.written(), ring.epoch().get());
                let peak = (0..ring.channels()).map(|channel| ring.take_peak(channel)).fold(0.0, f32::max);
                eprintln!(
                    "{}: {} ch @ {default_rate} Hz, block {}, {} frames in 1 s, epoch drift {} ns, peak {peak}",
                    device.name,
                    ring.channels(),
                    ring.block_frames(),
                    after.0 - before.0,
                    after.1.unwrap_or(0) - before.1.unwrap_or(0),
                );
                assert!(after.0 > before.0, "{} delivered nothing", device.name);
                assert!(after.1.is_some(), "{} stamps no capture time", device.name);
            }
            Some(InputState::Failed(error)) => eprintln!("{}: failed: {error}", device.name),
            _ => eprintln!("{}: still opening", device.name),
        }
        hub.close(&device.name);
    }
}
