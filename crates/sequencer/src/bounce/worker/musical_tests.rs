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
    write_pulse(&sample_path, 440.0);
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

fn write_pulse(path: &Path, frequency: f32) {
    let mut sample = hound::WavWriter::create(path, hound::WavSpec {
        channels: 2, sample_rate: SAMPLE_RATE, bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    }).unwrap();
    // Twenty milliseconds: separate hits cannot overlap without an effect.
    for frame in 0..960 {
        let value = 0.2 * (std::f32::consts::TAU * frequency * frame as f32 / SAMPLE_RATE as f32).cos();
        sample.write_sample(value).unwrap();
        sample.write_sample(value * 0.5).unwrap();
    }
    sample.finalize().unwrap();
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

#[track_caller]
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

fn saved_layer_rack(folder: &Path, configure: impl FnOnce(&mut App)) -> ProjectFile {
    let second = folder.join("second.wav");
    write_pulse(&second, 733.0);
    saved_song(folder, |app| {
        app.graph_controller().group_track_to_instrument_rack(0).unwrap();
        assert_eq!(app.graph_controller().add_sampler_slot_to_rack(0, &second).unwrap(), 1);
        configure(app);
    })
}

#[test]
fn saved_layer_rack_sums_slots_and_preserves_gain_mute_and_solo() {
    let folder = tempfile::tempdir().unwrap();
    let first = saved_layer_rack(folder.path(), |app| {
        assert!(app.set_rack_slot_mute(0, 1, true));
    });
    let second = saved_layer_rack(folder.path(), |app| {
        assert!(app.set_rack_slot_mute(0, 0, true));
    });
    let both = saved_layer_rack(folder.path(), |_| {});
    let solo = saved_layer_rack(folder.path(), |app| {
        assert!(app.set_rack_slot_solo(0, 1, true));
    });
    let quieter = saved_layer_rack(folder.path(), |app| {
        assert!(app.set_rack_slot_gain(0, 1, 0.25));
    });
    let a = render(folder.path(), "first-slot", &first, None, 0.0);
    let b = render(folder.path(), "second-slot", &second, None, 0.0);
    assert!(peak(&a) > 0.01 && peak(&b) > 0.01, "both source slots must sound");
    assert!(a.iter().zip(&b).any(|(a, b)| (a - b).abs() > 0.01),
        "different samples must retain their independent identities");
    let sum: Vec<_> = a.iter().zip(&b).map(|(a, b)| a + b).collect();
    assert_audio_matches(&render(folder.path(), "both-slots", &both, None, 0.0), &sum);
    assert_audio_matches(&render(folder.path(), "solo-second", &solo, None, 0.0), &b);
    let scaled: Vec<_> = a.iter().zip(&b).map(|(a, b)| a + b * 0.25).collect();
    assert_audio_matches(&render(folder.path(), "slot-gain", &quieter, None, 0.0), &scaled);
}

#[test]
fn saved_rack_slot_delay_preserves_dry_sibling_and_selected_tail() {
    let folder = tempfile::tempdir().unwrap();
    let dry = saved_layer_rack(folder.path(), |app| {
        assert!(app.set_rack_slot_mute(0, 1, true));
    });
    let echo = saved_layer_rack(folder.path(), |app| {
        let slot = app.add_builtin_rack_slot_effect_sync(0, 1, "Str8 Delay").unwrap();
        let descriptor = crate::effects::EffectDescriptor::builtin_insert("Str8 Delay").unwrap();
        for (name, value) in [("wet", 1.0), ("feedback", 0.5), ("left sync", 0.0),
            ("right sync", 0.0), ("left time", 250.0), ("right time", 250.0),
            ("mod amount", 0.0)] {
            let param = descriptor.params.iter().position(|param| param.name == name).unwrap();
            app.set_rack_slot_effect_param(0, 1, slot, param, value).unwrap();
        }
    });
    let mut wet_only = echo.clone();
    wet_only.patterns[0].rack_tracks[0].as_mut().unwrap().slots[0].mute = true;
    let wet_audio = render(folder.path(), "wet-slot", &wet_only, None, 0.25);
    let dry_audio = render(folder.path(), "dry-sibling", &dry, None, 0.25);
    let full = render(folder.path(), "rack-echo", &echo, None, 0.25);
    // The wet/dry control smooths from its initial value. Compare complete
    // independent branches so startup smoothing is included in the reference.
    assert!(peak(&dry_audio[..2_000 * 2]) > 0.01);
    let sum: Vec<_> = dry_audio.iter().zip(&wet_audio).map(|(a, b)| a + b).collect();
    assert_audio_matches(&full, &sum);
    assert!(peak(&dry_audio[2_000 * 2..]) < 1e-7);
    assert!(peak(&full[10_000 * 2..]) > 0.001, "slot 1's delay must reach the rack mix");
    let selected = render(folder.path(), "rack-excerpt", &echo, Some((0.4001, 1.5001)), 0.25);
    let first = (0.4001 * FRAMES_PER_BEAT as f64).ceil() as usize;
    let end = (1.5001 * FRAMES_PER_BEAT as f64).ceil() as usize;
    assert_audio_matches(&selected, &full[first * 2..(end + 12_000) * 2]);
    assert!(peak(&selected[(end - first) * 2..]) > 0.001, "rack echo must survive range end");
}

#[test]
fn saved_drum_rack_routes_both_member_lanes_through_its_bus() {
    let folder = tempfile::tempdir().unwrap();
    let second = folder.path().join("drum-second.wav");
    write_pulse(&second, 733.0);
    let build = |grouped: bool| saved_song(folder.path(), |app| {
        assert_eq!(app.graph_controller().add_track(&second).unwrap(), 1);
        app.state.pattern.patterns[1].set_step_active(4, true);
        let scenes = app.state.capture_project_scenes();
        let mut arrangement = ProjectArrangement::new(2, 2.0);
        for track in 0..2 {
            let id = arrangement.allocate_clip_id().unwrap();
            arrangement.track_lanes[track].push(ArrClip::new(id, 0.0, 2.0,
                Some(scenes.scenes[0].cells[track].unwrap().0)));
        }
        app.state.set_committed_arrangement(Some(arrangement)).unwrap();
        if grouped {
            let (group, _) = app.create_drum_rack_recorded(Some("Drums".into())).unwrap();
            app.attach_track_to_group(0, group, Some(crate::sequencer::DRUM_RACK_FIRST_PAD_NOTE)).unwrap();
            app.attach_track_to_group(1, group, Some(crate::sequencer::DRUM_RACK_FIRST_PAD_NOTE + 2)).unwrap();
        }
    });
    let loose = build(false);
    let rack = build(true);
    let reference = render(folder.path(), "loose-drums", &loose, None, 0.0);
    assert!(peak(&reference[..2_000 * 2]) > 0.01, "first drum must sound");
    assert!(peak(&reference[24_000 * 2..26_000 * 2]) > 0.01, "second drum must sound");
    assert_audio_matches(&render(folder.path(), "drum-rack", &rack, None, 0.0), &reference);
    // Mutate the saved bus value: reloading must apply it to every rack member.
    let mut muted = rack.clone();
    let bus = muted.groups.iter().find(|group| group.is_rack()).unwrap().bus_id;
    muted.buses.iter_mut().find(|channel| channel.id == bus).unwrap().mute = true;
    assert!(peak(&render(folder.path(), "muted-drums", &muted, None, 0.0)) < 1e-7,
        "neither drum may bypass the rack bus");
}

#[test]
fn saved_muted_layer_rack_is_silent_from_the_first_sample() {
    let folder = tempfile::tempdir().unwrap();
    let project = saved_layer_rack(folder.path(), |app| {
        set_clips(app, 2.0, &[(0.0, 2.0)]);
        assert!(app.set_rack_slot_mute(0, 0, true));
        assert!(app.set_rack_slot_mute(0, 1, true));
    });
    let audio = render(folder.path(), "muted-at-start", &project, None, 0.0);
    assert!(peak(&audio) < 1e-7,
        "saved muted slots must never leak into the export, peak={}", peak(&audio));
}

#[test]
fn saved_scene_routing_changes_preserve_compensated_audio_and_tail() {
    use crate::sequencer::{SceneEvent, TrackSendSnapshot};
    let folder = tempfile::tempdir().unwrap();
    let mut bus_a = crate::sequencer::BusId::MIX;
    let mut bus_b = bus_a;
    let mut project = saved_song(folder.path(), |app| {
        bus_a = app.add_bus_channel("A");
        bus_b = app.add_bus_channel("B");
        let latent_bus = app.add_bus_channel("Latent");
        app.graph_controller().add_blank_sampler_track().unwrap();
        let index = app.buses.iter().position(|bus| bus.id == latent_bus).unwrap();
        app.add_builtin_bus_effect_sync(index, crate::effects::filter_table::NAME).unwrap();
        app.set_track_output_all_scenes_unrecorded(1, TrackOutput::Bus(latent_bus));
        for step in 0..16 { app.state.pattern.patterns[0].set_step_active(step, true); }
        // The second track is silent; its bus's spectral effect requires 2048
        // samples of compensation on the audible, otherwise dry paths.
        app.set_track_output_all_scenes_unrecorded(0, TrackOutput::Bus(bus_a));
    });
    project.patterns.truncate(1);
    project.patterns.push(project.patterns[0].clone());
    project.scene_banks.clear();
    project.track_sounds.clear();
    let boundary = 0.75037; // The preceding note is still inside the PDC ring.
    let mut arrangement = ProjectArrangement::new(2, 2.25);
    arrangement.scene_lane = vec![SceneEvent { start_beat: 0.0, scene: 0 },
        SceneEvent { start_beat: boundary, scene: 1 }];
    for (start, end, pattern) in [(0.0, boundary, 1), (boundary, 2.0, 2)] {
        let id = arrangement.allocate_clip_id().unwrap();
        arrangement.track_lanes[0].push(ArrClip::new(id, start, end, Some(pattern)));
    }
    project.arrangement = Some(arrangement);
    let reference = render(folder.path(), "fixed-routing-reference", &project, None, 0.1);
    assert!(peak(&reference) > 0.01);
    project.patterns[1].bus_patterns.iter_mut().find(|bus| bus.id == bus_a.0).unwrap()
        .output = crate::project::BusOutput::Bus(bus_b.0);
    let rerouted = render(folder.path(), "bus-rerouted", &project, None, 0.1);
    assert_audio_matches(&rerouted, &reference);
    project.patterns[1].track_params[0].output = TrackOutput::Mix.into();
    assert_audio_matches(&render(folder.path(), "primary-rerouted", &project, None, 0.1), &reference);

    // A send absent in scene zero opens in scene one. Its node ids must be
    // prepared before the scheduler snapshots, and its delay history must
    // already be warm. With dry buses it adds another copy of the same note.
    project.patterns[1].track_params[0].sends = vec![TrackSendSnapshot { destination: bus_b, amount: 1.0 }.into()];
    let sent = render(folder.path(), "later-send", &project, None, 0.1);
    let late_note = 30_000 * 2..32_000 * 2;
    assert!(peak(&reference[late_note.clone()]) > 0.01);
    let doubled: Vec<_> = reference[late_note.clone()].iter().map(|sample| sample * 2.0).collect();
    assert_audio_matches(&sent[late_note], &doubled);
    let excerpt = render(folder.path(), "routing-excerpt", &project, Some((0.9, 1.9)), 0.1);
    assert_audio_matches(&excerpt, &sent[21_600 * 2..50_400 * 2]);

    project.patterns[0].track_params[0].sends = project.patterns[1].track_params[0].sends.clone();
    project.patterns[1].track_params[0].sends.clear();
    let closed = render(folder.path(), "removed-send", &project, None, 0.1);
    assert_audio_matches(&closed[30_000 * 2..32_000 * 2], &reference[30_000 * 2..32_000 * 2]);
}
