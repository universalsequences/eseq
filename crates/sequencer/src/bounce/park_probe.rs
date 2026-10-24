//! Parking cost probe (rig spec §5.1, P0, eseq-ka5t.8). Measurement only.
//!
//! Renders a saved project offline through the export path (the real audio
//! graph and scheduler, no device) in several variants and times every DSP
//! block. Differences between variants give what a silent track, an off kit
//! or its sequencer cost. Variants are edits of the input project: step bits,
//! rack lanes and rack-owned instances are edited on the JSON, builtin effect
//! bypass (`enabled` = 0) on the loaded `ProjectFile`, and track deletion
//! through `App` with a re-capture.
//!
//! ```sh
//! ESEQ_PARK_PROJECT=/abs/fixture.json ESEQ_PARK_OUT=/abs/out-dir \
//! cargo nextest run --release -p sequencer --lib \
//!   -E 'test(=bounce::session::park_probe::parking_cost_probe)' \
//!   --run-ignored only --no-capture
//! # synthetic builtin-effect rows (ESEQ_PARK_OUT only):
//! #   -E 'test(=bounce::session::park_probe::synthetic_effect_cost_probe)'
//! ```
//!
//! Optional: `ESEQ_PARK_ROUNDS` (default 3, interleaved; compare minima),
//! `ESEQ_PARK_END_BEAT` (default 32), `ESEQ_PARK_VARIANTS` (comma list,
//! default all), `ESEQ_PARK_KIT` (group id of the kit to park, default 1),
//! `ESEQ_PARK_LOOSE` (loose track indices, default every non-kit track),
//! `ESEQ_PARK_TRACKS` and `ESEQ_PARK_EFFECTS` (synthetic probe).
//! `ESEQ_PARK_DUMP_DIR` writes each render's interleaved f32 output there as
//! `<variant>.r<round>.f32`; `ESEQ_PARK_FORCE_AWAKE=3` renders as if no effect
//! ever tail-sleeps or skips a settled bypass (`effects::tail_sleep::ForceAwake`
//! bits: 1 = no bypass skip, 2 = no sleep), the A/B reference for what they
//! change. Bit 4 renders gatepitch and idle voice modulators the reference
//! way (per-frame clock ramp, no declared lane shapes), the A/B for the idle
//! voice control fast path (eseq-ka5t.16).
//!
//! The project's scratch must not import UI-only packages (gotham's
//! `proto-rig.rig` fails in the headless scheduler; strip it in the fixture).
//! Dev user instruments resolve under the checkout's `.local/instruments`.
//! The test binary can also be run directly with `RUST_MIN_STACK=16777216`
//! and `--ignored --exact <name> --nocapture` to avoid a rebuild.

use super::*;
use serde_json::{json, Value};
use std::time::Instant;

struct Variant {
    name: &'static str,
    /// Clear these kits' rack lanes (clip pointer None everywhere).
    kits_off: KitSel,
    /// Clear these tracks' steps in every scene pattern (cell present, silent).
    silent_tracks: TrackSel,
    delete_kit: bool,
    delete_loose: bool,
    /// Delete the rack-owned instances of the parked kit(s).
    delete_kit_sequencers: bool,
    /// Save builtin effects with `enabled` = 0 (what slot bypass pushes).
    bypass: BypassSel,
}

#[derive(Clone, Copy, PartialEq)]
enum BypassSel { None, KitBus, Kit, All }

#[derive(Clone, Copy, PartialEq)]
enum KitSel { None, One, All }
#[derive(Clone, Copy, PartialEq)]
enum TrackSel { None, Loose, All }

const VARIANTS: &[Variant] = &[
    Variant { name: "full", kits_off: KitSel::None, silent_tracks: TrackSel::None, delete_kit: false, delete_loose: false, delete_kit_sequencers: false, bypass: BypassSel::None },
    Variant { name: "loose_silent", kits_off: KitSel::None, silent_tracks: TrackSel::Loose, delete_kit: false, delete_loose: false, delete_kit_sequencers: false, bypass: BypassSel::None },
    Variant { name: "loose_deleted", kits_off: KitSel::None, silent_tracks: TrackSel::None, delete_kit: false, delete_loose: true, delete_kit_sequencers: false, bypass: BypassSel::None },
    Variant { name: "kit_off", kits_off: KitSel::One, silent_tracks: TrackSel::None, delete_kit: false, delete_loose: false, delete_kit_sequencers: false, bypass: BypassSel::None },
    Variant { name: "kit_off_no_seq", kits_off: KitSel::One, silent_tracks: TrackSel::None, delete_kit: false, delete_loose: false, delete_kit_sequencers: true, bypass: BypassSel::None },
    Variant { name: "all_silent", kits_off: KitSel::All, silent_tracks: TrackSel::All, delete_kit: false, delete_loose: false, delete_kit_sequencers: false, bypass: BypassSel::None },
    Variant { name: "all_silent_no_seq", kits_off: KitSel::All, silent_tracks: TrackSel::All, delete_kit: false, delete_loose: false, delete_kit_sequencers: true, bypass: BypassSel::None },
    Variant { name: "all_silent_bypass_kit_bus", kits_off: KitSel::All, silent_tracks: TrackSel::All, delete_kit: false, delete_loose: false, delete_kit_sequencers: false, bypass: BypassSel::KitBus },
    Variant { name: "all_silent_bypass_kit", kits_off: KitSel::All, silent_tracks: TrackSel::All, delete_kit: false, delete_loose: false, delete_kit_sequencers: false, bypass: BypassSel::Kit },
    Variant { name: "all_silent_bypass_all", kits_off: KitSel::All, silent_tracks: TrackSel::All, delete_kit: false, delete_loose: false, delete_kit_sequencers: false, bypass: BypassSel::All },
    Variant { name: "full_bypass_all", kits_off: KitSel::None, silent_tracks: TrackSel::None, delete_kit: false, delete_loose: false, delete_kit_sequencers: false, bypass: BypassSel::All },
];

/// Set `enabled` = 0 on the selected builtin effects in every pattern. DGen
/// and saved effects are left running (their bypass is a copy in the DGen
/// wrapper, not a descriptor param this probe can address by name).
fn bypass_effects(project: &mut crate::project::ProjectFile, select: BypassSel, kit: u64) -> usize {
    use crate::effects::EffectDescriptor;
    if select == BypassSel::None { return 0; }
    let enabled_of = |source: &crate::project::ProjectEffectSource| match source {
        crate::project::ProjectEffectSource::Builtin { name } =>
            EffectDescriptor::builtin_insert(name).and_then(|desc| desc.enabled_param_idx()),
        _ => None,
    };
    let group = project.groups.iter().find(|group| group.id == kit).cloned();
    let kit_bus = group.as_ref().map(|group| group.bus_id);
    let kit_tracks: Vec<usize> = group.map(|group| group.members.clone()).unwrap_or_default();
    let track_index = |id: u64| project.tracks.iter().position(|track| track.id.0 == id);
    let wants_track = |track: usize| match select {
        BypassSel::All => true,
        BypassSel::Kit => kit_tracks.contains(&track),
        _ => false,
    };
    let mut count = 0;
    let instances = project.device_instances.clone();
    for chain in &instances.bus_effects {
        if select != BypassSel::All && Some(chain.bus_id) != kit_bus { continue; }
        let Some(bus) = project.buses.iter_mut().find(|bus| bus.id == chain.bus_id) else { continue };
        for (slot, instance) in chain.instances.iter().enumerate() {
            if let (Some(idx), Some(effect)) = (enabled_of(&instance.source), bus.effect_slots.get_mut(slot)) {
                if idx < effect.defaults.len() { effect.defaults[idx] = 0.0; count += 1; }
            }
        }
    }
    for chain in &instances.track_effects {
        let Some(track) = track_index(chain.track_id) else { continue };
        if !wants_track(track) { continue; }
        for (slot, instance) in chain.instances.iter().enumerate() {
            let Some(idx) = enabled_of(&instance.source) else { continue };
            for pattern in &mut project.patterns {
                if let Some(effect) = pattern.effect_slots.get_mut(track).and_then(|chain| chain.get_mut(slot)) {
                    if idx < effect.defaults.len() { effect.defaults[idx] = 0.0; count += 1; }
                }
            }
        }
    }
    for chain in &instances.rack_effects {
        let Some(track) = track_index(chain.track_id) else { continue };
        if !wants_track(track) { continue; }
        for (slot, instance) in chain.instances.iter().enumerate() {
            let Some(idx) = enabled_of(&instance.source) else { continue };
            for pattern in &mut project.patterns {
                let Some(Some(rack)) = pattern.rack_tracks.get_mut(track) else { continue };
                if let Some(effect) = rack.slots.get_mut(chain.slot_index)
                    .and_then(|rack_slot| rack_slot.effect_slots.get_mut(slot))
                {
                    if idx < effect.defaults.len() { effect.defaults[idx] = 0.0; count += 1; }
                }
            }
        }
    }
    count
}

fn env_path(key: &str) -> PathBuf {
    PathBuf::from(std::env::var(key).unwrap_or_else(|_| panic!("{key} is required")))
}

fn kit_members(project: &Value, kit: u64) -> Vec<usize> {
    project["groups"].as_array().unwrap().iter()
        .find(|group| group["id"].as_u64() == Some(kit))
        .map(|group| group["members"].as_array().unwrap().iter()
            .map(|member| member.as_u64().unwrap() as usize).collect())
        .unwrap_or_default()
}

fn loose_tracks(project: &Value) -> Vec<usize> {
    if let Ok(list) = std::env::var("ESEQ_PARK_LOOSE") {
        return list.split(',').map(|track| track.trim().parse().unwrap()).collect();
    }
    let in_kit: std::collections::HashSet<usize> = project["groups"].as_array().unwrap().iter()
        .filter(|group| !group["rack"].is_null())
        .flat_map(|group| group["members"].as_array().unwrap().iter()
            .map(|member| member.as_u64().unwrap() as usize))
        .collect();
    (0..project["tracks"].as_array().unwrap().len()).filter(|track| !in_kit.contains(track)).collect()
}

/// JSON-only edits: kit clip pointers and scene-cell presence.
fn edit_json(project: &mut Value, variant: &Variant, kit: u64, loose: &[usize]) {
    let lanes = project["arrangement"]["rack_lanes"].as_object_mut().unwrap();
    for (id, lane) in lanes.iter_mut() {
        let off = match variant.kits_off {
            KitSel::None => false,
            KitSel::One => id.parse::<u64>().unwrap() == kit,
            KitSel::All => true,
        };
        if off { *lane = json!([]); }
    }
    if variant.kits_off == KitSel::All {
        // Scene rows also carry each kit's pointer.
        for scene in project["scene_rack_clips"].as_array_mut().unwrap() {
            *scene = json!([]);
        }
    } else if variant.kits_off == KitSel::One {
        for scene in project["scene_rack_clips"].as_array_mut().unwrap() {
            scene.as_array_mut().unwrap().retain(|pair| pair[0].as_u64() != Some(kit));
        }
    }
    if variant.delete_kit_sequencers {
        // Rack-owned sequencer instances of the parked kit(s), dropped from
        // the file (capture after App-side instance deletion rejects this
        // project's rack-slot effect records).
        let list = project["instances"]["list"].as_array_mut().unwrap();
        let before = list.len();
        list.retain(|instance| {
            let rack = instance["owner"]["rack"].as_u64();
            match variant.kits_off {
                KitSel::All => rack.is_none(),
                _ => rack != Some(kit),
            }
        });
        assert!(list.len() < before, "no rack-owned sequencers to delete");
    }
    let silent: Vec<usize> = match variant.silent_tracks {
        TrackSel::None => Vec::new(),
        TrackSel::Loose => loose.to_vec(),
        TrackSel::All => (0..project["tracks"].as_array().unwrap().len()).collect(),
    };
    // A silent cell: present, but with no active steps in any scene's pattern.
    for pattern in project["patterns"].as_array_mut().unwrap() {
        let bits = pattern["track_bits"].as_array_mut().unwrap();
        for &track in &silent {
            if let Some(words) = bits.get_mut(track) {
                for word in words.as_array_mut().unwrap() { *word = json!(0); }
            }
        }
    }
}

fn load_app(path: &Path) -> (App, WorkerEngine) {
    let project = crate::project::load_project_from_path(path).unwrap();
    let owner = WorkerEngine(engine::init_headless_engine(48_000, 2).unwrap());
    let engine = &owner.0;
    let mut app = App::new(Arc::clone(&engine.state), engine.lg_ptr, engine.sample_rate,
        engine.buses.clone(), Arc::clone(&engine.master_recorder), engine.keyboard_tx.clone());
    app.sample_analysis = crate::analysis::AnalysisService::synchronous();
    let name = project.name.clone();
    app.queue_bounce_project(&name, project).unwrap();
    while app.has_pending_project_load() {
        app.advance_pending_project_load().unwrap();
        assert!(unsafe { crate::audiograph::prepare_graph_for_render(engine.lg_ptr.0) });
    }
    (app, owner)
}

/// Deletions go through App so tracks, groups, lanes and instances stay
/// consistent; the result is re-captured and saved.
fn apply_deletions(input: &Path, output: &Path, variant: &Variant, kit: u64, loose: &[usize]) -> Result<(), String> {
    let (mut app, owner) = load_app(input);
    if variant.delete_loose {
        app.delete_tracks_recorded(loose.to_vec())?;
    }
    if variant.delete_kit {
        let kits: Vec<u64> = if variant.delete_loose {
            app.groups.iter().filter(|group| group.rack.is_some()).map(|group| group.id).collect()
        } else {
            vec![kit]
        };
        for id in kits { app.delete_group_with_members_recorded(id)?; }
    }
    let project = app.capture_project("park-probe")?;
    serde_json::to_writer(File::create(output).unwrap(), &project).unwrap();
    drop(app);
    drop(owner);
    Ok(())
}

fn thread_cpu_seconds() -> f64 {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
}

fn cpu_seconds() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 * 1e-6;
    tv(usage.ru_utime) + tv(usage.ru_stime)
}

/// Same per-block controls as the export render, without the WAV sink.
fn time_render(path: &Path, end_beat: f64, bypass: BypassSel, kit: u64) -> Value {
    let options = ExportOptions {
        project: path.to_path_buf(), destination: path.with_extension("unused.wav"),
        sample_rate: 48_000, tail_seconds: 0.0, selection: Some((0.0, end_beat)),
        replace: true, cancel_path: None,
    };
    let cancel = BounceCancellation::default();
    let mut project = options.load_project().unwrap();
    let bypassed = bypass_effects(&mut project, bypass, kit);
    let tracks = project.tracks.len();
    let instances = project.instances.list.len();
    let mut prepared = match PreparedExport::prepare(&options, project, &cancel) {
        Ok(prepared) => prepared,
        Err(error) => return json!({ "error": error.to_string() }),
    };
    let app = &mut prepared.app;
    let engine = &prepared.engine.0;
    let source = prepared.plan.source_range();
    let block = engine.block_size;
    let mut renderer = OfflineAudioSession::new(engine, source.end).unwrap();
    let mut rows = VecDeque::<AudibleSongRowApplied>::new();
    let mut output = vec![0.0_f32; block * 2];
    let mut times = Vec::new();
    // Scheduler advance (its own thread when live) precedes the controls hook.
    let mut sched = Vec::new();
    let (mut peak, mut energy) = (0.0_f32, 0.0_f64);
    let warmup = 48_000 / block as u64; // first second untimed
    let mut cpu_start = cpu_seconds();
    let mut thread_start = thread_cpu_seconds();
    let mut wall_start = Instant::now();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let dump_dir = std::env::var_os("ESEQ_PARK_DUMP_DIR").map(PathBuf::from);
    let mut dump = Vec::<f32>::with_capacity(if dump_dir.is_some() { source.end as usize * 2 } else { 0 });
    let force_awake = match std::env::var("ESEQ_PARK_FORCE_AWAKE") {
        Err(_) => 0,
        Ok(value) => value.trim().parse::<u32>().ok().filter(|bits| *bits <= 7).unwrap_or_else(|| {
            panic!("ESEQ_PARK_FORCE_AWAKE must be a bit mask 0-7 (1 = no bypass skip, 2 = no sleep, 4 = reference voice control), got {value:?}")
        }),
    };
    let _force_awake = crate::effects::tail_sleep::ForceAwake::set(force_awake);
    let mut start = 0_u64;
    let mut index = 0_u64;
    while start < source.end {
        if index == warmup {
            cpu_start = cpu_seconds(); thread_start = thread_cpu_seconds(); wall_start = Instant::now();
        }
        let t0 = Instant::now();
        let mut t_controls = t0;
        renderer.render_block_with_controls(start, &mut output, |frame| {
            t_controls = Instant::now();
            for notice in engine.state.drain_song_playback_notices() {
                match notice {
                    SongPlaybackNotice::RowApplied(row) => rows.push_back(row),
                    SongPlaybackNotice::StartFailed { error } => return Err(io::Error::other(error)),
                    SongPlaybackNotice::Ended { .. } => {}
                }
            }
            let control_frame = frame.min(source.end - 1);
            while rows.front().is_some_and(|row| row.effective_sample <= control_frame) {
                let row = rows.pop_front().unwrap();
                let _ = app.drain_due_mixer_controls(row.effective_sample);
                app.mirror_song_row_applied(&row).map_err(io::Error::other)?;
            }
            let _ = app.drain_due_mixer_controls(control_frame);
            Ok(())
        }).unwrap();
        if index >= warmup {
            times.push(t0.elapsed().as_secs_f64() * 1e6);
            sched.push((t_controls - t0).as_secs_f64() * 1e6);
            if dump_dir.is_some() { dump.extend_from_slice(&output); }
            for sample in &output {
                hash = (hash ^ sample.to_bits() as u64).wrapping_mul(0x100_0000_01b3);
                peak = peak.max(sample.abs());
                energy += (*sample as f64) * (*sample as f64);
            }
        }
        start += block as u64;
        index += 1;
    }
    let cpu = cpu_seconds() - cpu_start;
    let thread_cpu = thread_cpu_seconds() - thread_start;
    let wall = wall_start.elapsed().as_secs_f64();
    if let Some(dir) = &dump_dir {
        static RENDERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let round = RENDERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stem = path.file_stem().unwrap().to_string_lossy().replace(".edited", "").replace(".project", "");
        let bytes: Vec<u8> = dump.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(dir.join(format!("{stem}.r{round}.f32")), bytes).unwrap();
    }
    let audio_seconds = times.len() as f64 * block as f64 / 48_000.0;
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let budget_us = block as f64 / 48_000.0 * 1e6;
    json!({
        "tracks": tracks, "instances": instances, "bypassed_effect_records": bypassed, "block_frames": block,
        "timed_blocks": times.len(), "audio_seconds": audio_seconds,
        "block_us_mean": mean, "block_us_p50": pick(0.5), "block_us_p95": pick(0.95),
        "block_us_max": pick(1.0), "block_budget_us": budget_us,
        "dsp_load_pct_mean": mean / budget_us * 100.0,
        "scheduler_us_mean": sched.iter().sum::<f64>() / sched.len() as f64,
        "audio_us_mean": mean - sched.iter().sum::<f64>() / sched.len() as f64,
        "wall_ms_per_audio_s": wall / audio_seconds * 1000.0,
        "cpu_ms_per_audio_s": cpu / audio_seconds * 1000.0,
        "render_thread_cpu_ms_per_audio_s": thread_cpu / audio_seconds * 1000.0,
        "output_hash": format!("{hash:016x}"),
        "peak": peak, "rms": (energy / (times.len() * block * 2) as f64).sqrt(),
    })
}

#[test]
#[ignore = "measurement probe: needs a saved project and its samples (rig spec §5 P0)"]
fn parking_cost_probe() {
    crate::app_paths::init().unwrap();
    let input = env_path("ESEQ_PARK_PROJECT");
    let out = env_path("ESEQ_PARK_OUT");
    std::fs::create_dir_all(&out).unwrap();
    let end_beat: f64 = std::env::var("ESEQ_PARK_END_BEAT").map(|v| v.parse().unwrap()).unwrap_or(32.0);
    let kit: u64 = std::env::var("ESEQ_PARK_KIT").map(|v| v.parse().unwrap()).unwrap_or(1);
    let wanted: Option<Vec<String>> = std::env::var("ESEQ_PARK_VARIANTS").ok()
        .map(|list| list.split(',').map(|name| name.trim().to_string()).collect());
    let source: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    let loose = loose_tracks(&source);
    eprintln!("[park-probe] kit {kit} members {:?}; loose tracks {loose:?}", kit_members(&source, kit));
    let rounds: usize = std::env::var("ESEQ_PARK_ROUNDS").map(|v| v.parse().unwrap()).unwrap_or(3);
    let mut results = serde_json::Map::new();
    let mut prepared = Vec::new();
    for variant in VARIANTS {
        if wanted.as_ref().is_some_and(|list| !list.iter().any(|name| name == variant.name)) {
            continue;
        }
        let mut edited = source.clone();
        edit_json(&mut edited, variant, kit, &loose);
        let json_path = out.join(format!("{}.edited.json", variant.name));
        serde_json::to_writer(File::create(&json_path).unwrap(), &edited).unwrap();
        let render_path = if variant.delete_kit || variant.delete_loose {
            let path = out.join(format!("{}.project.json", variant.name));
            if let Err(error) = apply_deletions(&json_path, &path, variant, kit, &loose) {
                eprintln!("[park-probe] skip {}: {error}", variant.name);
                continue;
            }
            path
        } else {
            json_path
        };
        prepared.push((variant.name, render_path, variant.bypass));
    }
    // Rounds interleave the variants so background load spreads across them;
    // compare per-variant minima (and medians) rather than single runs.
    for round in 0..rounds {
        for (name, path, bypass) in &prepared {
            let result = time_render(path, end_beat, *bypass, kit);
            eprintln!("[park-probe] round {round} {name:<18} {result}");
            results.entry(name.to_string()).or_insert_with(|| json!([]))
                .as_array_mut().unwrap().push(result);
        }
    }
    let report = json!({
        "project": input, "end_beat": end_beat, "kit": kit, "loose_tracks": loose, "rounds": rounds,
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "variants": results,
    });
    std::fs::write(out.join("report.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}

/// One synthetic project: `tracks` sampler tracks, each with `effect` (or
/// none), its clips covering [0, end) when `playing`, and its effect's
/// `enabled` param saved as 0 when `bypassed`.
fn synthetic_project(
    path: &Path, sample: &Path, tracks: usize, effect: Option<&str>,
    playing: bool, bypassed: bool, end_beat: f64,
) {
    let fixture = WorkerEngine(engine::init_headless_engine(48_000, 2).unwrap());
    let engine = &fixture.0;
    let mut app = App::new(Arc::clone(&engine.state), engine.lg_ptr, engine.sample_rate,
        engine.buses.clone(), Arc::clone(&engine.master_recorder), engine.keyboard_tx.clone());
    let mut slots = Vec::new();
    for track in 0..tracks {
        app.graph_controller().add_track(sample).unwrap();
        if let Some(name) = effect {
            slots.push(app.add_builtin_effect_sync(track, name).unwrap());
        }
        for step in (0..16).step_by(4) {
            app.state.pattern.patterns[track].set_step_active(step, true);
        }
    }
    let cells = app.state.capture_project_scenes().scenes[0].cells.clone();
    let mut arrangement = crate::sequencer::ProjectArrangement::new(tracks, end_beat);
    if playing {
        for track in 0..tracks {
            let clip = arrangement.allocate_clip_id().unwrap();
            arrangement.track_lanes[track].push(crate::sequencer::ArrClip::new(
                clip, 0.0, end_beat, Some(cells[track].unwrap().0)));
        }
    }
    app.state.set_committed_arrangement(Some(arrangement)).unwrap();
    let mut project = app.capture_project("park-synthetic").unwrap();
    if bypassed {
        let name = effect.unwrap();
        let enabled = crate::effects::EffectDescriptor::builtin_insert(name)
            .and_then(|desc| desc.enabled_param_idx())
            .unwrap_or_else(|| panic!("{name} has no enabled param"));
        for pattern in &mut project.patterns {
            for (track, chain) in pattern.effect_slots.iter_mut().enumerate().take(tracks) {
                let slot = &mut chain[slots[track] - crate::effects::BUILTIN_SLOT_COUNT];
                slot.defaults[enabled] = 0.0;
            }
        }
    }
    serde_json::to_writer(File::create(path).unwrap(), &project).unwrap();
}

/// Builtin insert effects on `ESEQ_PARK_TRACKS` (default 8) sampler tracks:
/// silent, playing, and playing with the effect bypassed (enabled = 0).
#[test]
#[ignore = "measurement probe (rig spec §5 P0)"]
fn synthetic_effect_cost_probe() {
    crate::app_paths::init().unwrap();
    let out = env_path("ESEQ_PARK_OUT");
    std::fs::create_dir_all(&out).unwrap();
    let tracks: usize = std::env::var("ESEQ_PARK_TRACKS").map(|v| v.parse().unwrap()).unwrap_or(8);
    let end_beat = 16.0;
    let sample = out.join("burst.wav");
    let mut writer = hound::WavWriter::create(&sample, hound::WavSpec {
        channels: 2, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float,
    }).unwrap();
    let mut seed = 0x1234_5678_u32;
    for frame in 0..12_000 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
        let value = noise * 0.3 * (1.0 - frame as f32 / 12_000.0);
        writer.write_sample(value).unwrap();
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();
    let effects: Vec<String> = std::env::var("ESEQ_PARK_EFFECTS")
        .map(|list| list.split(',').map(|name| name.trim().to_string()).collect())
        .unwrap_or_else(|_| ["Reverb", "Multiverb", "Delay", "Str8 Delay", "Space Echo", "Chorus", "Compressor"]
            .iter().map(|name| name.to_string()).collect());
    let mut results = serde_json::Map::new();
    let mut prepared = Vec::new();
    let mut run = |label: String, count: usize, effect: Option<&str>, playing: bool, bypassed: bool| {
        let path = out.join(format!("{}.project.json", label.replace([' ', '/'], "_")));
        synthetic_project(&path, &sample, count, effect, playing, bypassed, end_beat);
        prepared.push((label, path));
    };
    run("empty_1_track_no_fx".into(), 1, None, false, false);
    run("samplers_silent_no_fx".into(), tracks, None, false, false);
    run("samplers_playing_no_fx".into(), tracks, None, true, false);
    for effect in &effects {
        run(format!("{effect}/silent"), tracks, Some(effect), false, false);
        run(format!("{effect}/playing"), tracks, Some(effect), true, false);
        run(format!("{effect}/bypassed_playing"), tracks, Some(effect), true, true);
    }
    let rounds: usize = std::env::var("ESEQ_PARK_ROUNDS").map(|v| v.parse().unwrap()).unwrap_or(3);
    for round in 0..rounds {
        for (label, path) in &prepared {
            let result = time_render(path, end_beat, BypassSel::None, 0);
            eprintln!("[park-probe] round {round} {label:<28} {result}");
            results.entry(label.clone()).or_insert_with(|| json!([]))
                .as_array_mut().unwrap().push(result);
        }
    }
    let report = json!({
        "tracks": tracks, "end_beat": end_beat, "rounds": rounds,
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "variants": results,
    });
    std::fs::write(out.join("synthetic-report.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}
