//! Small saved songs with audible expectations, exercised through Save and export.
use super::*;
use crate::app::App;
use crate::audio::engine;
use crate::bounce::session::WorkerEngine;
use crate::project::ProjectFile;
use crate::sequencer::{ArrClip, ProjectArrangement, StepParam, TrackOutput};
use std::sync::Arc;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES_PER_BEAT: usize = 24_000; // 120 BPM

fn saved_song(folder: &Path, configure: impl FnOnce(&mut App)) -> ProjectFile {
    let sample_path = folder.join("pulse.wav");
    let mut sample = hound::WavWriter::create(&sample_path, hound::WavSpec {
        channels: 2, sample_rate: SAMPLE_RATE, bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    }).unwrap();
    // Twenty milliseconds: separate hits cannot overlap without an effect.
    for frame in 0..960 {
        let value = 0.2 * (std::f32::consts::TAU * 440.0 * frame as f32 / SAMPLE_RATE as f32).cos();
        sample.write_sample(value).unwrap();
        sample.write_sample(value * 0.5).unwrap();
    }
    sample.finalize().unwrap();
    let owner = WorkerEngine(engine::init_headless_engine(SAMPLE_RATE, 2).unwrap());
    let engine = &owner.0;
    let mut app = App::new(Arc::clone(&engine.state), engine.lg_ptr, SAMPLE_RATE,
        engine.buses.clone(), Arc::clone(&engine.master_recorder), engine.keyboard_tx.clone());
    app.graph_controller().add_track(&sample_path).unwrap();
    app.state.pattern.patterns[0].set_step_active(0, true);
    set_clips(&app, 2.0, &[(0.0, 2.0)]);
    configure(&mut app);
    let project = app.capture_project("musical-export-fixture").unwrap();
    assert_eq!(project.bpm, 120);
    project
}

fn set_clips(app: &App, end: f64, ranges: &[(f64, f64)]) {
    let pattern = app.state.capture_project_scenes().scenes[0].cells[0].unwrap();
    let mut arrangement = ProjectArrangement::new(1, end);
    for &(start, end) in ranges {
        let id = arrangement.allocate_clip_id().unwrap();
        arrangement.track_lanes[0].push(ArrClip::new(id, start, end, Some(pattern.0)));
    }
    app.state.set_committed_arrangement(Some(arrangement)).unwrap();
}

fn render(folder: &Path, name: &str, project: &ProjectFile,
    selection: Option<(f64, f64)>, tail_seconds: f64,
) -> Vec<f32> {
    let path = folder.join(format!("{name}.json"));
    serde_json::to_writer(File::create(&path).unwrap(), project).unwrap();
    let destination = folder.join(format!("{name}.wav"));
    let options = ExportOptions {
        project: path, destination: destination.clone(), sample_rate: SAMPLE_RATE,
        tail_seconds, selection, replace: false, cancel_path: None,
    };
    let summary = export_project(&options, &BounceCancellation::default(), |_| {}).unwrap();
    let mut wav = hound::WavReader::open(destination).unwrap();
    assert_eq!(wav.spec().channels, 2);
    assert_eq!(wav.spec().sample_rate, SAMPLE_RATE);
    let samples: Vec<f32> = wav.samples::<f32>().collect::<Result<_, _>>().unwrap();
    assert_eq!(samples.len(), summary.frames as usize * 2);
    assert!(samples.iter().all(|sample| sample.is_finite()));
    samples
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |peak, sample| peak.max(sample.abs()))
}

fn assert_audio_matches(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    let difference = actual.iter().zip(expected)
        .map(|(a, b)| (a - b).abs()).fold(0.0_f32, f32::max);
    assert!(difference < 1e-6, "maximum sample difference: {difference}");
}

#[test]
fn saved_clip_restart_preserves_sub_block_timing_and_silent_gap() {
    let folder = tempfile::tempdir().unwrap();
    let second_beat = 0.75037;
    let project = saved_song(folder.path(), |app| {
        set_clips(app, 1.25, &[(0.0, 0.25), (second_beat, 1.0)]);
    });
    let audio = render(folder.path(), "clips", &project, None, 0.0);
    assert_eq!(audio.len(), 30_000 * 2);
    let second_frame = (second_beat * FRAMES_PER_BEAT as f64).ceil() as usize;
    assert_ne!(second_frame % engine::ENGINE_BLOCK_FRAMES, 0);
    let onset = |frames: &[f32]| frames.chunks_exact(2)
        .position(|frame| peak(frame) > 1e-6).expect("clip must sound");
    let first_onset = onset(&audio[..2_000 * 2]);
    let second_onset = onset(&audio[2_000 * 2..]) + 2_000;
    assert_eq!(second_onset - first_onset, second_frame,
        "clip restart must land on its authored sample, not the next DSP block");
    assert!(peak(&audio[..960 * 2]) > 0.01);
    assert!(peak(&audio[2_000 * 2..second_frame * 2]) < 1e-7, "gap must stay silent");
    assert!(peak(&audio[(second_frame + 2_000) * 2..]) < 1e-7,
        "no extra notes after the final clip");
}

#[test]
fn saved_velocity_lock_scales_one_note_and_restores_the_next() {
    let folder = tempfile::tempdir().unwrap();
    let reference = saved_song(folder.path(), |app| {
        for step in [2, 3] { app.state.pattern.patterns[0].set_step_active(step, true); }
    });
    let locked = saved_song(folder.path(), |app| {
        for step in [2, 3] { app.state.pattern.patterns[0].set_step_active(step, true); }
        app.state.set_step_param(0, 2, StepParam::Velocity, 0.25);
    });
    let normal = render(folder.path(), "normal", &reference, None, 0.0);
    let quiet = render(folder.path(), "locked", &locked, None, 0.0);
    let note = |step: usize| {
        let start = step * FRAMES_PER_BEAT / 4;
        start * 2..(start + 2_000) * 2
    };
    for step in [0, 3] {
        assert!(peak(&normal[note(step)]) > 0.01);
        assert_audio_matches(&quiet[note(step)], &normal[note(step)]);
    }
    let expected: Vec<_> = normal[note(2)].iter().map(|sample| sample * 0.25).collect();
    assert!(peak(&expected) > 0.002, "locked note must remain audible");
    assert_audio_matches(&quiet[note(2)], &expected);
}

#[test]
fn selected_export_preserves_bus_delay_history_and_tail() {
    let folder = tempfile::tempdir().unwrap();
    let project = saved_song(folder.path(), |app| {
        let bus = app.add_bus_channel("Echo");
        app.set_track_output_all_scenes_unrecorded(0, TrackOutput::Bus(bus));
        let index = app.buses.iter().position(|channel| channel.id == bus).unwrap();
        let slot = app.add_builtin_bus_effect_sync(index, "Str8 Delay").unwrap();
        let descriptor = crate::effects::EffectDescriptor::builtin_insert("Str8 Delay").unwrap();
        for (name, value) in [("wet", 1.0), ("feedback", 0.5), ("left sync", 0.0),
            ("right sync", 0.0), ("left time", 250.0), ("right time", 250.0),
            ("mod amount", 0.0)] {
            let param = descriptor.params.iter().position(|param| param.name == name).unwrap();
            app.set_bus_effect_param(index, slot, param, value).unwrap();
        }
    });
    let full = render(folder.path(), "whole", &project, None, 0.25);
    // No notes in this selected range: all its audio must come from the earlier hit.
    // Both ends fall between DSP block boundaries. The source note is long finished.
    let selected = render(folder.path(), "excerpt", &project, Some((0.4001, 1.5001)), 0.25);
    let first = (0.4001 * FRAMES_PER_BEAT as f64).ceil() as usize;
    let end = (1.5001 * FRAMES_PER_BEAT as f64).ceil() as usize;
    let tail = SAMPLE_RATE as usize / 4;
    assert_eq!(selected.len(), (end - first + tail) * 2);
    assert_audio_matches(&selected, &full[first * 2..(end + tail) * 2]);
    assert!(peak(&selected[..(end - first) * 2]) > 0.001, "prefix must prime the delay");
    assert!(peak(&selected[(end - first) * 2..]) > 0.001, "tail must preserve echoes");
}
