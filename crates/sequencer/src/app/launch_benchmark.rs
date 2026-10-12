//! Opt-in saved-project control/scheduler latency baseline. No hardware audio
//! or UI runs here. Timed work calls the production launch and scheduler seams;
//! fixture loading, graph servicing and correctness checks are outside timers.

use super::*;
use crate::audio::engine;
use crate::quantized_launch::{LaunchQuantize, QuantizedLaunchOwner};
use crate::scheduler::{SchedulerDriver, SchedulerInput};
use crate::scheduler::scheduled_event::ScheduledEventQueue;
use crate::sequencer::SequencerSnapshot;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

struct EngineOwner(engine::HeadlessEngine);
impl Drop for EngineOwner {
    fn drop(&mut self) { unsafe { self.0.destroy(); } }
}

#[derive(Clone, Debug)]
enum Target {
    Scene(usize),
    Rack { group: u64, clip: u64 },
}

impl Target {
    fn category(&self) -> String {
        match self {
            Self::Scene(_) => "scene".to_string(),
            Self::Rack { group, .. } => format!("rack-{group}"),
        }
    }

    fn scene(&self, app: &App) -> usize {
        match self { Self::Scene(scene) => *scene, Self::Rack { .. } => app.state.current_scene_index() }
    }

    fn assign(&self, app: &mut App) {
        if let Self::Rack { group, clip } = self {
            app.set_current_rack_clip_recorded(*group, Some(*clip)).expect("assign rack clip");
        }
    }
}

fn elapsed_ms(start: Instant) -> f64 { start.elapsed().as_secs_f64() * 1000.0 }

fn distribution(mut values: Vec<f64>) -> Value {
    assert!(!values.is_empty());
    values.sort_by(f64::total_cmp);
    let percentile = |p: f64| values[(p * values.len() as f64).ceil() as usize - 1];
    json!({ "count": values.len(), "min": values[0], "p50": percentile(0.5),
        "p95": percentile(0.95), "p99": percentile(0.99), "max": values[values.len() - 1] })
}

fn shuffled<T>(values: &mut [T], seed: &mut u64) {
    // Fixed Fisher-Yates order, recorded in full in the report. No dependency
    // on HashMap iteration, thread timing or process-randomized seeds.
    for i in (1..values.len()).rev() {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        values.swap(i, (*seed % (i + 1) as u64) as usize);
    }
}

fn verify_snapshot(app: &App, scene: usize, snapshot: &SequencerSnapshot) {
    assert_eq!(snapshot.transport.current_pattern, scene);
    assert_eq!(snapshot.transport.num_tracks, app.tracks.len());
    app.state.with_project_scenes(|scenes| {
        for track in 0..app.tracks.len() {
            let cell = scenes.composed_scene_cell(scene, track);
            let actual = &snapshot.tracks[track];
            assert_eq!(actual.scene_silenced, cell.is_none(),
                "scene {scene}, track {track}: scheduler snapshot must include composed rack cells");
            if let Some(id) = cell {
                let expected = scenes.track_pools[track].seq(id).expect("resolved pattern");
                for (step, actual) in actual.steps.iter().enumerate() {
                    let active = expected.track_bits[step / 64] & (1u64 << (step % 64)) != 0;
                    assert_eq!(actual.active, active, "scene {scene}, track {track}, step {step}");
                    assert_eq!(actual.params, expected.step_data[step],
                        "scene {scene}, track {track}, step {step}: step parameters");
                }
            }
        }
    });
}

fn prepare_graph(app: &App) {
    assert!(unsafe { crate::audiograph::prepare_graph_for_render(app.graph.lg.0) },
        "service headless graph edits");
}

fn stored_lock_memory(state: &SequencerState) -> Value {
    let mut grids = std::collections::HashSet::new();
    let (mut patches, mut slots, mut empty_slots, mut bytes, mut cells) = (0, 0, 0, 0, 0);
    let mut visit = |slot: &crate::effects::EffectSlotSnapshot| {
        slots += 1;
        empty_slots += usize::from(slot.num_params == 0);
        cells += slot.plocks.iter().flatten().filter(|cell| cell.is_some()).count();
        if !slot.plocks.is_empty() && grids.insert((0, slot.plocks.as_ptr() as usize)) {
            bytes += slot.plocks.retained_bytes();
        }
        if !slot.plock_param_ids.is_empty() && grids.insert((1, slot.plock_param_ids.as_ptr() as usize)) {
            bytes += slot.plock_param_ids.retained_bytes();
        }
    };
    state.with_project_scenes(|scenes| {
        for pool in &scenes.track_pools {
            for patch in pool.sounds.patches.values() {
                patches += 1;
                visit(&patch.instrument_slot);
                for slot in patch.effect_slots.iter().chain(&patch.midi_fx_slots) { visit(slot); }
                if let Some(rack) = &patch.rack_track {
                    for slot in &rack.slots {
                        visit(&slot.instrument_slot);
                        for effect in &slot.effect_slots { visit(effect); }
                    }
                }
            }
        }
    });
    json!({ "patches": patches, "device_slots": slots, "empty_slots": empty_slots,
        "value_cells_per_owner": cells, "unique_grids": grids.len(), "unique_grid_bytes": bytes,
        "scope": "current project patch pools only; values and identities, Arc/Vec allocations; excludes allocator overhead, live atomic slots, history and other subsystems" })
}

fn run(quantized: bool) {
    assert!(!cfg!(debug_assertions), "latency measurements require --release");
    let input = PathBuf::from(std::env::var("ESEQ_LAUNCH_PROJECT").expect("ESEQ_LAUNCH_PROJECT"));
    let output = PathBuf::from(std::env::var("ESEQ_LAUNCH_OUT").expect("ESEQ_LAUNCH_OUT"));
    assert!(input.is_absolute() && output.is_absolute(), "use absolute fixture/report paths");
    let rounds: usize = std::env::var("ESEQ_LAUNCH_ROUNDS").ok()
        .map(|value| value.parse().expect("round count")).unwrap_or(2);
    assert!(rounds >= 2, "include first visits and revisits");
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    let bytes = std::fs::read(&input).unwrap();
    let frozen = output.with_extension("project.json");
    assert_ne!(input, frozen, "report snapshot must not overwrite input");
    std::fs::write(&frozen, &bytes).unwrap();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    crate::paths::enter_sequencer_dir().unwrap();
    let project = crate::project::load_project_from_path(&frozen).unwrap();
    // Rack-authored graph scripts require the UI runtime's publication seam.
    // Fail instead of silently omitting them from this control-only fixture.
    assert!(project.groups.iter().all(|group| group.rack.as_ref()
        .is_none_or(|rack| rack.sequencers.is_empty())), "use UI fixture for rack-authored scripts");
    let owner = EngineOwner(engine::init_headless_engine(48_000, 2).unwrap());
    let engine = &owner.0;
    let state = Arc::clone(&engine.state);
    let mut app = App::new(Arc::clone(&state), engine.lg_ptr, engine.sample_rate,
        engine.buses.clone(), Arc::clone(&engine.master_recorder), engine.keyboard_tx.clone());
    app.sample_analysis = crate::analysis::AnalysisService::synchronous();
    let name = project.name.clone();
    // Same loader, with missing samples treated as errors instead of fallback.
    app.queue_bounce_project(&name, project).unwrap();
    let load_start = Instant::now();
    let mut progress = 0;
    while app.has_pending_project_load() {
        app.advance_pending_project_load().unwrap();
        prepare_graph(&app);
        progress += 1;
        eprintln!("launch benchmark: load phase {progress}, {:.1}s", load_start.elapsed().as_secs_f64());
    }
    app.sample_analysis.require_complete().unwrap();
    app.publish_all_sampler_analysis_runtime();
    let mut control_runtime = crate::lisp_host::scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0);
    control_runtime.eval(&state.scratch_source()).expect("restore project scratch/kind definitions");
    for instance in &app.instances.list {
        assert!(crate::lisp_host::registered_kind(&instance.kind).is_some(),
            "project kind must be registered: {}", instance.kind);
    }
    app.publish_instance_sequencers();
    assert!(app.state.scene_count() >= 2);
    let scene_count = state.scene_count();
    let lock_memory = stored_lock_memory(&state);
    let racks: Vec<_> = app.groups.iter().filter(|group| group.rack.is_some())
        .map(|group| json!({ "id": group.id, "name": group.name, "members": group.members,
            "clips": app.rack_clip_bank(group.id).len() })).collect();
    let mut batches = vec![(0..scene_count).map(Target::Scene).collect::<Vec<_>>()];
    for group in app.groups.iter().filter(|group| group.rack.is_some()) {
        let clips = app.rack_clip_bank(group.id);
        assert!(clips.len() >= 2);
        batches.push(clips.into_iter().map(|(clip, _, _)| Target::Rack { group: group.id, clip }).collect());
    }
    let mut seed = 0x657365715f73636eu64;
    for batch in &mut batches { shuffled(batch, &mut seed); }
    state.start_playback();
    // Establish a known launch before measuring, but do not preflight or visit
    // every target. First visits therefore retain descriptor/cache cold costs.
    app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: state.current_scene_index() }).unwrap();
    prepare_graph(&app);
    let queue = Arc::new(ScheduledEventQueue::<4096>::new());
    let mut driver = SchedulerDriver::new(Arc::clone(&state), engine.sample_rate,
        engine.block_size, Arc::clone(&queue));
    let mut rendered = 0u64;
    let mut report = json!({ "schema": 1, "mode": if quantized { "quantized" } else { "immediate" },
        "project_sha256": hash, "project": input, "tracks": app.tracks.len(),
        "scenes": scene_count, "racks": racks, "rounds": rounds,
        "published_sequencers": state.published_sequencers().len(),
        "stored_lock_memory": lock_memory,
        "sample_rate": engine.sample_rate, "block_frames": engine.block_size,
        "bpm": state.latest_scheduler_snapshot().transport.bpm, "seed": "657365715f73636e", "complete": false,
        "scope": "production control + deterministic scheduler; no UI, DSP rendering, hardware audio or wall-clock deadline simulation",
        "samples": [], "summaries": {} });
    write_report(&output, &report);
    // Runtime initialization is outside launch timings. The same production
    // driver reloads scratch/process code and propagates errors as live playback.
    advance(&mut driver, &queue, &state, &mut rendered, engine.block_size, engine.sample_rate);
    for round in 0..rounds {
        for batch in &batches {
            for target in batch {
                let scene = target.scene(&app);
                let unchanged = match target {
                    Target::Scene(_) => scene == state.current_scene_index(),
                    Target::Rack { group, clip } => app.rack_clip_bank(*group).iter()
                        .any(|(id, _, active)| id == clip && *active),
                };
                if unchanged { continue; } // never time a no-op
                let from_scene = state.current_scene_index();
                let started = Instant::now();
                target.assign(&mut app);
                let assign_ms = elapsed_ms(started);
                let launch = PatternLaunchTarget::Scene { scene };
                let mut passes = Vec::new();
                let (request_ms, mirror_ms) = if quantized {
                    let start = Instant::now();
                    state.schedule_quantized_pattern_launch(launch.clone(), LaunchQuantize::Sixteenth,
                        QuantizedLaunchOwner::Transport).unwrap();
                    let request_ms = elapsed_ms(start);
                    // Audit AFTER timing the request so the audit itself does
                    // not warm its preparation caches. Reject incomplete rack
                    // content before accepting any boundary timing.
                    let snapshot = state.preflight_pattern_launch_snapshot(&launch).expect("preflight target");
                    verify_snapshot(&app, scene, &snapshot);
                    drop(snapshot);
                    let mut mirror_ms = None;
                    for _ in 0..256 {
                        passes.push(advance(&mut driver, &queue, &state, &mut rendered,
                            engine.block_size, engine.sample_rate));
                        let start = Instant::now();
                        let results = app.drain_due_pattern_launches();
                        let elapsed = elapsed_ms(start);
                        if !results.is_empty() {
                            assert_eq!(results.len(), 1);
                            let result = results.into_iter().next().unwrap().unwrap();
                            assert_eq!(result.target, launch);
                            assert!(result.warnings.is_empty(), "{:?}", result.warnings);
                            mirror_ms = Some(elapsed);
                            break;
                        }
                    }
                    (request_ms, mirror_ms.expect("launch must reach its boundary"))
                } else {
                    let start = Instant::now();
                    let result = app.apply_manual_pattern_launch(&launch).unwrap();
                    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
                    let request_ms = elapsed_ms(start);
                    // Include the first scheduler refill after the immediate
                    // epoch change as its own sample, outside control timing.
                    passes.push(advance(&mut driver, &queue, &state, &mut rendered,
                        engine.block_size, engine.sample_rate));
                    (request_ms, 0.0)
                };
                let snapshot = state.latest_scheduler_snapshot();
                verify_snapshot(&app, scene, &snapshot);
                if let Target::Rack { group, clip } = target {
                    state.with_project_scenes(|scenes| assert!(scenes.live_rack_clips.contains(&(*group, Some(*clip)))));
                }
                prepare_graph(&app);
                let control_ms = assign_ms + request_ms + mirror_ms;
                let sample = json!({ "round": round, "category": target.category(),
                    "target": format!("{target:?}"), "from_scene": from_scene, "to_scene": scene,
                    "assign_ms": assign_ms, "request_ms": request_ms, "mirror_ms": mirror_ms,
                    "control_ms": control_ms, "scheduler_passes": passes });
                eprintln!("launch benchmark: round {round} {target:?}, control {control_ms:.3}ms");
                report["samples"].as_array_mut().unwrap().push(sample);
                write_report(&output, &report);
            }
        }
    }
    let mut summaries = serde_json::Map::new();
    for batch in &batches {
        let category = batch[0].category();
        for round in 0..rounds {
            let samples: Vec<_> = report["samples"].as_array().unwrap().iter()
                .filter(|s| s["category"] == category && s["round"] == round).collect();
            let mut summary = serde_json::Map::new();
            for field in ["control_ms", "assign_ms", "request_ms", "mirror_ms"] {
                summary.insert(field.to_string(), distribution(samples.iter().map(|s| s[field].as_f64().unwrap()).collect()));
            }
            summary.insert("scheduler_pass_ms".to_string(), distribution(samples.iter()
                .flat_map(|s| s["scheduler_passes"].as_array().unwrap())
                .map(|p| p["wall_ms"].as_f64().unwrap()).collect()));
            summaries.insert(format!("{category}/round-{round}"), Value::Object(summary));
        }
    }
    report["summaries"] = Value::Object(summaries);
    report["complete"] = json!(true);
    write_report(&output, &report);
}

fn advance(driver: &mut SchedulerDriver, queue: &ScheduledEventQueue<4096>,
    state: &SequencerState, rendered: &mut u64, block: usize, sample_rate: u32) -> Value
{
    let horizon = *rendered + (block * 4) as u64;
    let start = Instant::now();
    let result = driver.advance(*rendered, horizon, SchedulerInput::Offline);
    let wall_ms = elapsed_ms(start);
    let errors = driver.take_runtime_errors();
    assert!(errors.is_empty(), "scheduler errors: {errors:?}");
    assert!(state.drain_generator_tick_errors().is_empty(), "generator errors");
    assert_eq!(result.queue_rejections, 0, "scheduler queue overflow");
    assert!(result.scheduled_until_sample >= horizon, "scheduler must fill requested horizon");
    let mut events = 0;
    while queue.pop().is_some() { events += 1; }
    let report = json!({ "rendered": *rendered, "frontier": result.scheduled_until_sample,
        "wall_ms": wall_ms, "events": events, "block_ms": block as f64 * 1000.0 / sample_rate as f64 });
    *rendered += block as u64;
    report
}

fn write_report(path: &Path, report: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(report).unwrap()).unwrap();
}

#[test]
#[ignore = "manual release benchmark; ESEQ_LAUNCH_PROJECT and ESEQ_LAUNCH_OUT must be absolute paths"]
fn saved_project_immediate_launch_latency() { run(false); }

#[test]
#[ignore = "manual release benchmark; verifies composed rack contents before timing quantized launches"]
fn saved_project_quantized_launch_latency() { run(true); }
