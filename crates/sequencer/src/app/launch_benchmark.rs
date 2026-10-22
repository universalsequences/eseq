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
    let (owner, mut app, _control_runtime) = load_saved_project(project);
    let engine = &owner.0;
    let state = Arc::clone(&engine.state);
    for instance in &app.instances.list {
        assert!(crate::lisp_host::registered_kind(&instance.kind).is_some(),
            "project kind must be registered: {}", instance.kind);
    }
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

/// Load `project` into a headless engine through the production loader, with
/// missing samples treated as errors instead of fallback, then restore its
/// scratch/kind definitions and publish its sequencers. Keep the returned
/// control runtime alive while the App plays.
fn load_saved_project(
    project: crate::project::ProjectFile,
) -> (EngineOwner, App, crate::lisp_host::ScratchControlRuntime) {
    let owner = EngineOwner(engine::init_headless_engine(48_000, 2).unwrap());
    let engine = &owner.0;
    let state = Arc::clone(&engine.state);
    let mut app = App::new(Arc::clone(&state), engine.lg_ptr, engine.sample_rate,
        engine.buses.clone(), Arc::clone(&engine.master_recorder), engine.keyboard_tx.clone());
    app.sample_analysis = crate::analysis::AnalysisService::synchronous();
    let name = project.name.clone();
    app.queue_bounce_project(&name, project).unwrap();
    let load_start = Instant::now();
    let mut progress = 0;
    while app.has_pending_project_load() {
        app.advance_pending_project_load().unwrap();
        prepare_graph(&app);
        progress += 1;
        eprintln!("saved project: load phase {progress}, {:.1}s", load_start.elapsed().as_secs_f64());
    }
    app.sample_analysis.require_complete().unwrap();
    app.publish_all_sampler_analysis_runtime();
    let mut control_runtime = crate::lisp_host::scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0);
    control_runtime.eval(&state.scratch_source()).expect("restore project scratch/kind definitions");
    app.publish_instance_sequencers();
    (owner, app, control_runtime)
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

/// Scene-switch timing repro on a saved project: immediate launches between
/// two scenes at offsets swept through one step, checking that every step
/// both scenes share sounds exactly once. ESEQ_SWITCH_PROJECT (absolute
/// path), ESEQ_SWITCH_SCENES="a,b" (0-based), ESEQ_SWITCH_STEP (0-based step
/// the switch lands in).
#[test]
#[ignore = "manual repro; ESEQ_SWITCH_PROJECT must be an absolute path"]
fn saved_project_immediate_scene_switch_keeps_shared_hits() {
    use crate::scheduler::scheduled_event::ScheduledEventKind;
    let input = PathBuf::from(std::env::var("ESEQ_SWITCH_PROJECT").expect("ESEQ_SWITCH_PROJECT"));
    let scenes: Vec<usize> = std::env::var("ESEQ_SWITCH_SCENES").unwrap_or("1,2".into())
        .split(',').map(|s| s.trim().parse().unwrap()).collect();
    let switch_step: f64 = std::env::var("ESEQ_SWITCH_STEP").ok()
        .map(|v| v.parse().unwrap()).unwrap_or(6.0);
    // ESEQ_SWITCH_LEGACY=1 launches through the old take-back resync.
    let legacy = std::env::var_os("ESEQ_SWITCH_LEGACY").is_some();
    // ESEQ_SWITCH_SIXTEENTH=1 launches quantized to the next 16th instead.
    let sixteenth = std::env::var_os("ESEQ_SWITCH_SIXTEENTH").is_some();
    // ESEQ_SWITCH_GROOVED=1 tolerates grooved hits off their straight sample.
    let grooved = std::env::var_os("ESEQ_SWITCH_GROOVED").is_some();
    crate::paths::enter_sequencer_dir().unwrap();
    let project = crate::project::load_project_from_path(&input).unwrap();
    let (owner, mut app, _control_runtime) = load_saved_project(project);
    let engine = &owner.0;
    let state = Arc::clone(&engine.state);

    let tracks = app.tracks.len();
    // active[scene][track][step]
    let active: Vec<Vec<Vec<bool>>> = scenes.iter().map(|&scene| (0..tracks).map(|track| {
        state.with_project_scenes(|s| s.composed_scene_cell(scene, track)
            .and_then(|id| s.track_pools[track].seq(id).map(|seq| (0..16).map(|step|
                seq.track_bits[step / 64] & (1u64 << (step % 64)) != 0).collect()))
            .unwrap_or_else(|| vec![false; 16]))
    }).collect()).collect();
    for (i, scene) in scenes.iter().enumerate() {
        for track in 0..tracks {
            let row: String = active[i][track].iter().map(|a| if *a { 'x' } else { '.' }).collect();
            eprintln!("scene {} track {track:2} {:<28} {row}", scene + 1, app.tracks[track]);
        }
    }

    app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: scenes[0] }).unwrap();
    prepare_graph(&app);
    // Play as the app does: the arrangement transport, which auto-latches the
    // selected scene on a silent start, so every switch is a latched launch.
    // ESEQ_SWITCH_SESSION=1 plays plain session playback instead.
    if std::env::var_os("ESEQ_SWITCH_SESSION").is_some() {
        state.start_playback();
    } else {
        app.song_transport_play(false).expect("play the arrangement transport");
    }
    prepare_graph(&app);
    let queue = Arc::new(ScheduledEventQueue::<4096>::new());
    let block = engine.block_size as u64;
    let mut driver = SchedulerDriver::new(Arc::clone(&state), engine.sample_rate,
        engine.block_size, Arc::clone(&queue));
    let spq = 48_000.0 * 60.0 / state.latest_scheduler_snapshot().transport.bpm as f64;
    let step_samples = spq / 4.0;
    let bar = spq * 4.0;
    eprintln!("block {block}, step {step_samples:.1} samples, bar {bar:.1}");
    // The audio callback, modelled in production order: refresh the snapshot
    // (its epoch is what counts), drop stale countdowns, drain the queue
    // dropping stale events, dispatch the block (late events at offset 0),
    // render, and only then publish `rendered_samples = block_end`.
    struct Audio { epoch: u64, countdown: Vec<(u64, usize, usize, u64)>, played: Vec<(u64, usize, usize)> }
    impl Audio {
        fn dispatch(&mut self, state: &SequencerState, queue: &ScheduledEventQueue<4096>,
            block_start: u64, block: u64)
        {
            self.epoch = state.latest_scheduler_snapshot().transport.pattern_epoch;
            let epoch = self.epoch;
            self.countdown.retain(|(_, _, _, e)| *e == epoch);
            while let Some(event) = queue.pop() {
                if let ScheduledEventKind::ResolvedTrigger { track, step, .. } = &event.kind {
                    let track = *track;
                    if event.pattern_epoch == epoch {
                        self.countdown.push((event.sample_time, track, *step, event.pattern_epoch));
                    }
                }
            }
            let end = block_start + block;
            let (due, rest): (Vec<_>, Vec<_>) = self.countdown.drain(..).partition(|(s, _, _, _)| *s < end);
            self.countdown = rest;
            self.played.extend(due.into_iter().map(|(s, t, step, _)| (s.max(block_start), t, step)));
        }
    }
    #[derive(Clone, Copy, Debug)]
    enum Phase { BeforeCallback, DuringRender, SchedulerAfterCallback }
    let phases = [Phase::BeforeCallback, Phase::DuringRender, Phase::SchedulerAfterCallback];
    let mut audio = Audio { epoch: 0, countdown: Vec::new(), played: Vec::new() };
    let mut switches: Vec<(u64, usize, Phase)> = Vec::new();
    let offsets = 4;
    let bars = offsets * phases.len() * 2 + 2;
    let mut rendered = 0u64;
    let mut next_switch = 1;
    let mut target = 0;
    let pass = |driver: &mut SchedulerDriver, rendered: u64| {
        let result = driver.advance(rendered, rendered + block * 4, SchedulerInput::Offline);
        assert!(driver.take_runtime_errors().is_empty());
        assert_eq!(result.queue_rejections, 0);
    };
    while (rendered as f64) < bar * bars as f64 {
        let n = next_switch - 1;
        // Land the switch in the block holding `switch_step`'s boundary, or
        // a few blocks before it.
        let due = bar * next_switch as f64 + step_samples * switch_step
            - (n % offsets) as f64 * block as f64;
        let switching = next_switch < bars - 1 && (rendered + block) as f64 > due;
        let launch = |app: &mut App, target: &mut usize| {
            *target = 1 - *target;
            let launch = PatternLaunchTarget::Scene { scene: scenes[*target] };
            if sixteenth {
                app.state.schedule_quantized_pattern_launch(launch, LaunchQuantize::Sixteenth,
                    QuantizedLaunchOwner::Transport).unwrap();
            } else if legacy {
                app.apply_manual_pattern_launch(&launch).unwrap();
            } else if !app.try_queue_live_pattern_launch(&launch).unwrap() {
                app.apply_manual_pattern_launch(&launch).unwrap();
            }
            prepare_graph(app);
        };
        if switching {
            let phase = phases[(n / offsets) % phases.len()];
            switches.push((rendered, 1 - target, phase));
            next_switch += 1;
            match phase {
                Phase::BeforeCallback => {
                    launch(&mut app, &mut target);
                    pass(&mut driver, rendered);
                    audio.dispatch(&state, &queue, rendered, block);
                }
                Phase::DuringRender => {
                    pass(&mut driver, rendered);
                    audio.dispatch(&state, &queue, rendered, block);
                    // The UI publishes and the scheduler wakes while this
                    // block renders: `rendered_samples` is still its start.
                    launch(&mut app, &mut target);
                    pass(&mut driver, rendered);
                }
                Phase::SchedulerAfterCallback => {
                    pass(&mut driver, rendered);
                    launch(&mut app, &mut target);
                    audio.dispatch(&state, &queue, rendered, block);
                }
            }
        } else {
            pass(&mut driver, rendered);
            audio.dispatch(&state, &queue, rendered, block);
        }
        // The UI loop mirrors scheduler-applied launches.
        for result in app.drain_due_pattern_launches() {
            result.expect("live launch mirrors");
        }
        rendered += block;
    }
    // Grooves move a hit off its straight boundary by the same amount every
    // bar; the first bar, before any switch, measures each step's offset.
    let groove_offset = |track: usize, step: usize| -> Option<i64> {
        let boundary = (step as f64 * step_samples).round() as i64;
        audio.played.iter()
            .find(|(s, t, st)| *t == track && *st == step
                && (*s as i64 - boundary).abs() < step_samples as i64)
            .map(|(s, _, _)| *s as i64 - boundary)
    };
    let mut problems = Vec::new();
    // A rack track's steps come from its rack clips, not the scene cell, so
    // only the doubled-hit check covers it.
    let rack_tracks: Vec<bool> = {
        let snapshot = state.latest_scheduler_snapshot();
        (0..tracks).map(|t| snapshot.tracks.get(t).is_some_and(|track| track.rack_track.is_some())).collect()
    };
    let mut sorted = audio.played.clone();
    sorted.sort();
    for (i, (s, t, st)) in sorted.iter().enumerate() {
        if let Some((s2, _, _)) = sorted[i + 1..].iter()
            .take_while(|(s2, _, _)| s2 - s < (step_samples / 2.0) as u64)
            .find(|(_, t2, st2)| t2 == t && st2 == st)
        {
            problems.push(format!("track {t} {} step {} doubled at {s} and {s2}", app.tracks[*t], st + 1));
        }
    }
    for (index, (at, to, phase)) in switches.iter().enumerate() {
        let bar_start = (*at as f64 / bar).floor() * bar;
        for step in 0..16usize {
            let boundary = (bar_start + step as f64 * step_samples).round() as u64;
            for track in 0..tracks {
                if rack_tracks[track] || !(active[0][track][step] && active[1][track][step]) { continue; }
                // Grooves/swing can move a hit off its straight boundary.
                let found: Vec<_> = audio.played.iter()
                    .filter(|(s, t, st)| *t == track && *st == step && s.abs_diff(boundary) < step_samples as u64)
                    .map(|(s, _, _)| *s).collect();
                // Exactly once, on its own sample: a late replay lands on a
                // block start instead.
                let expected = boundary as i64 + groove_offset(track, step).unwrap_or(0);
                let exact = found.len() == 1
                    && (grooved || (found[0] as i64 - expected).abs() <= 1);
                if !exact {
                    problems.push(format!(
                        "switch {index} {phase:?} at {at} (step {:.2}) -> scene {}: track {track} {} step {} sounded {}x at {found:?} (boundary {boundary})",
                        (*at as f64 - bar_start) / step_samples + 1.0, scenes[*to] + 1, app.tracks[track], step + 1, found.len()));
                }
            }
        }
    }
    state.stop_playback();
    for problem in &problems { eprintln!("problem: {problem}"); }
    assert!(problems.is_empty(), "{} problems", problems.len());
}
