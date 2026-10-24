//! Controlled import tests (docs/rig-spec.md §8).

use std::path::Path;
use std::sync::Arc;

use eseqlisp::vm::Value;
use eseqlisp::Runtime;

use super::*;
use crate::app::test_rig::{launch, Rig, PIANO, PLATE};
use crate::project::{ProjectFile, ProjectInstanceOwner};
use crate::sequencer::{BusId, ModConnection, ModDestination, SceneSwapInheritance, TrackOutput, TrackSendSnapshot};

/// alez.neural's kind id; the stand-in below registers under it.
const NEURAL: &str = "alez/neural:neural";

/// A runtime with the graph natives bound to `app` and a stand-in neural
/// kind registered under its package id (as the break-kit tests do).
fn neural_runtime(app: &App) -> Runtime {
    crate::lisp_host::clear_kind_registry();
    let mut runtime = Runtime::new();
    crate::lisp_host::register_graph_authoring_natives(&mut runtime, Arc::clone(&app.state));
    runtime
        .eval_str(
            "(def-kind neural
               :sequencer (:shape (line 3) :max-poly 2
                           (def-node nrn :route 0
                             :params ((threshold :float 0 4 :default 0.5)))
                           (edges :from nrn :to nrn :topology (all-to-all)
                             :params ((weight :float -1 1 :default 0)))))",
        )
        .expect("def-kind evaluates");
    let mut definition = crate::lisp_host::registered_kind("scratch:neural").expect("stand-in kind");
    definition.id = NEURAL.to_string();
    definition.package = Some("alez/neural".to_string());
    crate::lisp_host::register_kind(definition);
    runtime
}

fn new_scene_in(app: &mut App, bank: SceneBankId) -> usize {
    let names = app.tracks.clone();
    let inheritance = SceneSwapInheritance::Source(app.state.playing_scene());
    app.state
        .clone_pattern_in_scene_bank(
            bank,
            inheritance,
            names.len(),
            &app.graph.track_buffer_ids.clone(),
            &app.graph.track_sample_rates.clone(),
            &names,
            &app.graph.track_instrument_types.clone(),
        )
        .expect("a new scene")
}

/// Steps on `track`'s current cell.
fn set_steps(app: &mut App, track: usize, steps: &[usize]) {
    let mut scenes = app.capture_synchronized_scene_structure_state().expect("scenes");
    let mut data = scenes.effective_track_pattern(track).expect("a pattern");
    data.clear_step_content();
    for step in steps {
        data.track_bits[0] |= 1 << step;
    }
    assert!(scenes.save_effective_track_pattern(track, data));
    app.restore_scene_structure_state(&scenes).expect("install the steps");
}

/// Two banks of two scenes each: A = {0, 1}, B = {2, 3}. Returns them.
fn two_banks(app: &mut App) -> (SceneBankId, SceneBankId) {
    let bank_a = app.state.scene_banks()[0].id;
    for _ in 0..3 {
        new_scene_in(app, bank_a);
    }
    let bank_b = app.state.create_scene_bank().unwrap();
    for _ in 0..2 {
        app.state.move_scene_to_scene_bank(2, bank_b).unwrap();
    }
    launch(app, 0);
    (bank_a, bank_b)
}

/// A project as a saved file reads back (a JSON round trip, as
/// `Rig::reload` does).
fn round_trip(project: ProjectFile) -> ProjectFile {
    let encoded = serde_json::to_value(&project).unwrap();
    serde_json::from_value(encoded).unwrap()
}

/// The SOURCE project: a loose plate track (track 0), a two-pad clip kit
/// "Src Kit" (plate + piano pads with steps), optionally owning a neural
/// instance, and a plain group over two piano tracks. Two banks of two
/// scenes. Returns the source and the kit's group id.
fn source_project(with_neural: bool) -> (ImportSource, u64) {
    source_project_with(with_neural, |_, _, _| {})
}

/// [`source_project`], with `edit(app, kit, kick)` run on the source before
/// it is captured.
fn source_project_with(
    with_neural: bool,
    edit: impl FnOnce(&mut App, u64, usize),
) -> (ImportSource, u64) {
    let mut rig = Rig::new();
    let app = rig.app();
    let mut runtime = with_neural.then(|| neural_runtime(app));
    app.graph_controller().add_track(Path::new(PLATE)).expect("loose track");
    let (kit, _) = app.create_drum_rack_recorded(Some("Src Kit".to_string())).expect("kit");
    let kick = app.graph_controller().add_track(Path::new(PLATE)).expect("pad 1");
    app.assign_rack_pad_track_recorded(kit, 36, kick).expect("pad 1");
    let snare = app.graph_controller().add_track(Path::new(PIANO)).expect("pad 2");
    app.assign_rack_pad_track_recorded(kit, 38, snare).expect("pad 2");
    set_steps(app, kick, &[0, 4, 8, 12]);
    set_steps(app, snare, &[4, 12]);
    app.convert_rack_to_clips_recorded(kit).expect("clips");
    if let Some(runtime) = runtime.as_mut() {
        let id = app
            .create_instance_recorded(NEURAL, ProjectInstanceOwner::Rack(kit), None)
            .expect("neural instance");
        runtime.set_global_value("src", Value::Instance(id));
        runtime.eval_str("(graph-edge src :from 0 :to 1 :weight 0.25)").expect("weight");
        runtime.eval_str("(graph-node src 0 :route 1)").expect("route to pad 2");
    }
    let keys_a = app.graph_controller().add_track(Path::new(PIANO)).expect("keys a");
    let keys_b = app.graph_controller().add_track(Path::new(PIANO)).expect("keys b");
    set_steps(app, keys_a, &[0, 2]);
    set_steps(app, keys_b, &[1, 3]);
    app.group_tracks_recorded(vec![keys_a, keys_b]).expect("keys group");
    if let Some(runtime) = runtime.as_mut() {
        // A project-owned conductor playing the first keys track.
        let id = app
            .create_instance_recorded(NEURAL, ProjectInstanceOwner::Project, Some("conductor".into()))
            .expect("project neural");
        runtime.set_global_value("cond", Value::Instance(id));
        runtime
            .eval_str(&format!("(graph-node cond 0 :route {keys_a})"))
            .expect("route to keys a");
        runtime
            .eval_str(&format!("(graph-node cond 1 :route {keys_b})"))
            .expect("route to keys b");
        // A node process slot, whose id an import must re-mint.
        runtime
            .eval_str("(graph-node-process-add cond 0 \"lane-count\")")
            .expect("a node process slot");
    }
    two_banks(app);
    edit(app, kit, kick);
    let project = app.capture_project("source").expect("capture");
    let source = ImportSource::from_file("source", round_trip(project)).expect("source");
    (source, kit)
}

/// The TARGET: one loose track playing steps, two banks A/B.
fn target(app: &mut App) -> (SceneBankId, SceneBankId) {
    let track = app.graph_controller().add_track(Path::new(PLATE)).expect("track");
    set_steps(app, track, &[0, 8]);
    two_banks(app)
}

/// What every scene of the target resolves to for its first `tracks`
/// tracks: cells, the steps they play, clip pointers and parked kits of
/// `kits`, slot sets, and the scheduler's silenced flags when it plays it.
fn resolved(app: &mut App, tracks: usize, kits: &[u64]) -> Vec<String> {
    let scenes = app.state.scene_count();
    let playing = app.state.current_scene_index();
    let out = (0..scenes)
        .map(|scene| {
            let static_part = app.state.with_project_scenes(|s| {
                let sc = &s.scenes[scene];
                let cells: Vec<_> = sc.cells.iter().take(tracks).map(|cell| cell.map(|id| id.0)).collect();
                let bits: Vec<_> = (0..tracks)
                    .map(|track| {
                        s.composed_scene_cell(scene, track)
                            .and_then(|id| s.track_pools[track].get(id))
                            .map(|data| data.track_bits[0])
                    })
                    .collect();
                let clips: Vec<_> = sc.rack_clips.iter().filter(|(g, _)| kits.contains(g)).collect();
                let parked: Vec<_> = sc.parked_kits.iter().filter(|g| kits.contains(g)).collect();
                let slots: Vec<_> = sc.slot_enable.iter().filter(|(t, _)| **t < tracks).collect();
                format!("{cells:?} {bits:?} {clips:?} {parked:?} {slots:?}")
            });
            launch(app, scene);
            let snapshot = app.state.latest_scheduler_snapshot();
            let silenced: Vec<bool> =
                snapshot.tracks.iter().take(tracks).map(|t| t.scene_silenced).collect();
            format!("{static_part} | {silenced:?}")
        })
        .collect();
    launch(app, playing);
    out
}

fn new_group(summary: &ImportSummary, source_group: u64) -> u64 {
    summary
        .groups
        .iter()
        .find(|(old, _)| *old == source_group)
        .map(|(_, new)| *new)
        .expect("the group was imported")
}

/// Whether kit `group` sounds when scene `scene` plays: not parked, every
/// pad able to trigger.
fn kit_sounds_in(app: &mut App, group: u64, scene: usize) -> bool {
    launch(app, scene);
    let snapshot = app.state.latest_scheduler_snapshot();
    let members = app.groups.iter().find(|g| g.id == group).unwrap().members.clone();
    let pointed = app.state.with_project_scenes(|s| s.scene_rack_clip(scene, group).is_some());
    pointed
        && !snapshot.rack_parked(group)
        && members.iter().all(|track| !snapshot.tracks[*track].scene_silenced)
}

#[test]
fn the_peek_lists_banks_kits_groups_tracks_and_changes_nothing() {
    let (source, kit) = source_project(false);
    let mut rig = Rig::new();
    let app = rig.app();
    target(app);
    let before = resolved(app, app.tracks.len(), &[]);
    let peek = source.peek();
    assert_eq!(peek.banks.len(), 2);
    assert_eq!(peek.banks[0].label, "A");
    assert_eq!(peek.banks.iter().map(|bank| bank.scenes.len()).sum::<usize>(), 4);
    let peeked_kit = peek.kits.iter().find(|k| k.group_id == kit).expect("the kit");
    assert_eq!(peeked_kit.name, "Src Kit");
    assert_eq!(peeked_kit.pads, 2);
    assert_eq!(peeked_kit.devices.len(), 2);
    assert_eq!(peeked_kit.banks, vec!["A".to_string(), "B".to_string()]);
    assert_eq!(peek.groups.len(), 1, "the plain group");
    assert_eq!(peek.groups[0].tracks.len(), 2);
    assert_eq!(peek.tracks.len(), 3, "loose + two grouped tracks, no pads");
    assert_eq!(resolved(app, app.tracks.len(), &[]), before, "browsing changed nothing");
}

/// Rig spec §8 "One bank": a kit imported into bank B is on in B only.
/// Bank A resolves exactly as before, the kit is parked in every A scene,
/// and one undo removes all of it.
#[test]
fn a_kit_imported_into_bank_b_plays_only_there_and_undoes_in_one_step() {
    let (source, kit) = source_project(false);
    let mut rig = Rig::new();
    let app = rig.app();
    let (_bank_a, bank_b) = target(app);
    let tracks_before = app.tracks.len();
    let groups_before = app.groups.len();
    let scenes_before = app.state.scene_count();
    let before = resolved(app, tracks_before, &[]);
    let undo_before = app.history.undo_len();

    let summary = app
        .import_from_project(
            &source,
            &ImportRequest {
                kits: vec![kit],
                target: ImportTarget::Bank(Some(bank_b)),
                ..Default::default()
            },
        )
        .expect("import");
    assert_eq!(app.history.undo_len(), undo_before + 1, "one undo step");
    assert_eq!(summary.bank, Some(bank_b));
    assert!(!summary.new_bank);
    let group = new_group(&summary, kit);
    assert_eq!(app.tracks.len(), tracks_before + 2, "two pads appended");
    let members = app.groups.iter().find(|g| g.id == group).unwrap().members.clone();
    assert!(members.iter().all(|track| *track >= tracks_before), "pads are new tracks");
    assert_eq!(app.state.scene_count(), scenes_before);

    // Everything that was there resolves as before; the kit is parked in A.
    assert_eq!(resolved(app, tracks_before, &[]), before, "the old tracks resolve as before");
    for scene in 0..2 {
        assert!(app.state.with_project_scenes(|s| s.kit_parked_in_scene(scene, group)), "scene {scene}");
        assert_eq!(
            app.state.with_project_scenes(|s| s.scene_rack_clip(scene, group)),
            None,
            "no clip pointer outside the target"
        );
        assert!(!kit_sounds_in(app, group, scene), "bank A scene {scene} is silent");
    }
    for scene in 2..4 {
        assert!(kit_sounds_in(app, group, scene), "bank B scene {scene} plays the kit");
    }
    // ...with the source's steps on its pads.
    let mut bits: Vec<u64> = app.state.with_project_scenes(|s| {
        members
            .iter()
            .filter_map(|track| s.composed_scene_cell(2, *track).and_then(|id| s.track_pools[*track].get(id)))
            .map(|data| data.track_bits[0])
            .collect()
    });
    bits.sort_unstable();
    assert_eq!(bits, vec![(1 << 4) | (1 << 12), 0x1111], "kick and snare steps");

    let crate::app::history::HistoryReplay::Applied(step) = crate::app::edit::undo(app) else {
        panic!("undo");
    };
    assert_eq!(step.label, "Import from source");
    assert_eq!(app.tracks.len(), tracks_before);
    assert_eq!(app.groups.len(), groups_before);
    assert_eq!(resolved(app, tracks_before, &[]), before, "undo restores every scene");
}

/// "Importing a bank brings its scenes as a new bank, with the scenes'
/// cells remapped to the new track indices"; the existing banks stay.
#[test]
fn a_bank_imports_as_a_new_bank_with_remapped_cells() {
    let (source, kit) = source_project(false);
    let source_bank_b = source.banks()[1].0;
    let mut rig = Rig::new();
    let app = rig.app();
    target(app);
    let tracks_before = app.tracks.len();
    let scenes_before = app.state.scene_count();
    let banks_before = app.state.scene_banks().len();
    let before = resolved(app, tracks_before, &[]);

    let summary = app
        .import_from_project(&source, &ImportRequest { banks: vec![source_bank_b], ..Default::default() })
        .expect("import");
    assert!(summary.new_bank);
    assert_eq!(app.state.scene_banks().len(), banks_before + 1);
    assert_eq!(app.state.scene_count(), scenes_before + 2, "the bank's two scenes");
    assert_eq!(summary.scenes, vec![(2, scenes_before), (3, scenes_before + 1)]);
    assert_eq!(
        resolved(app, tracks_before, &[])[..scenes_before].to_vec(),
        before,
        "the existing banks resolve as before"
    );
    // Cells remapped: each imported plain track plays its source steps in
    // the new scenes and holds no cell in the old ones.
    let plain: Vec<(usize, usize)> = summary
        .tracks
        .iter()
        .copied()
        .filter(|(old, _)| source.kit_of_track(*old).is_none())
        .collect();
    assert_eq!(plain.len(), 3, "the loose track and the grouped pair");
    for (old, new) in plain {
        let source_bits = source.file.patterns[2].track_bits[old][0];
        app.state.with_project_scenes(|s| {
            let cell = s.scenes[scenes_before].cells[new].expect("a cell in the new scene");
            assert_eq!(s.track_pools[new].get(cell).unwrap().track_bits[0], source_bits);
            for scene in 0..scenes_before {
                assert_eq!(s.scenes[scene].cells[new], None, "no cell in old scene {scene}");
            }
        });
    }
    let group = new_group(&summary, kit);
    assert!(kit_sounds_in(app, group, scenes_before));
    assert!(!kit_sounds_in(app, group, 0));
}

/// A route to something not imported is listed and left off, never pointed
/// at whatever the target has under the same id.
#[test]
fn an_unpicked_route_is_listed_as_unconnected_not_mispointed() {
    let (source, _) = source_project(false);
    let keys = source.file.groups.iter().find(|g| !g.is_rack()).unwrap().clone();
    let mut rig = Rig::new();
    let app = rig.app();
    target(app);
    // Give the target a bus under the source group's bus id, so a naive
    // copy would land on it.
    while !app.buses.iter().any(|bus| bus.id.0 == keys.bus_id) {
        app.add_bus_channel("Decoy");
    }
    let summary = app
        .import_from_project(
            &source,
            &ImportRequest {
                tracks: vec![keys.members[0]],
                target: ImportTarget::NewBank,
                ..Default::default()
            },
        )
        .expect("import");
    assert_eq!(summary.tracks.len(), 1);
    let new = summary.tracks[0].1;
    assert!(
        summary.unconnected.iter().any(|line| line.contains("output")),
        "the group output is listed: {:?}",
        summary.unconnected
    );
    assert!(summary.notes.iter().any(|note| note.contains("not recreated")), "{:?}", summary.notes);
    let scene = app
        .state
        .with_project_scenes(|s| s.scene_bank_range(summary.bank.unwrap()))
        .expect("the new bank")
        .start;
    app.state.with_project_scenes(|s| {
        let cell = s.scenes[scene].cells[new].expect("a cell");
        let output = s.track_pools[new].get(cell).unwrap().track_params.output;
        assert_eq!(output, TrackOutput::Mix, "not the decoy bus {}", keys.bus_id);
    });
}

/// The sample buffers every clip lane of kit `group` plays.
fn kit_buffers(app: &App, group: u64) -> Vec<i32> {
    let mut buffers: Vec<i32> = app.state.with_project_scenes(|s| {
        let bank = s.rack_bank(group).expect("a clip bank");
        bank.clips
            .iter()
            .flat_map(|clip| clip.cells.iter().enumerate())
            .filter_map(|(position, cell)| {
                let track = *bank.members.get(position)?;
                s.track_pools[track].get((*cell)?)
            })
            .filter_map(|data| data.rack_track)
            .flat_map(|rack| rack.slots.into_iter().filter_map(|slot| slot.sample_id.map(|s| s.0)))
            .collect()
    });
    buffers.sort_unstable();
    buffers.dedup();
    buffers
}

/// "Assets are shared": the same kit imported twice decodes each sample once.
#[test]
fn the_same_kit_imported_twice_loads_its_samples_once() {
    let (source, kit) = source_project(false);
    let mut rig = Rig::new();
    let app = rig.app();
    target(app);
    let request = ImportRequest { kits: vec![kit], target: ImportTarget::NewBank, ..Default::default() };
    let first = app.import_from_project(&source, &request).expect("first import");
    let decodes = app.import_samples.decodes();
    assert!(decodes >= 1, "the first import decodes its samples");
    let second = app.import_from_project(&source, &request).expect("second import");
    assert_eq!(app.import_samples.decodes(), decodes, "the second import decodes nothing");
    let one = kit_buffers(app, new_group(&first, kit));
    let two = kit_buffers(app, new_group(&second, kit));
    assert!(!one.is_empty());
    assert_eq!(one, two, "both kits play the same buffers");
}

/// A project written before scene-owned slot state (v17) imports: its
/// slots get ids, its lanes' stored flags decide what plays, and the kit
/// and the track play in the target bank.
#[test]
fn a_legacy_pre_v18_source_imports() {
    let (source, kit) = source_project(false);
    let mut file = source.file.clone();
    file.version = 17;
    file.scene_slot_enable.clear();
    file.scene_parked_kits.clear();
    let strip = |pattern: &mut crate::project::ProjectPattern| {
        for rack in pattern.rack_tracks.iter_mut().flatten() {
            for slot in &mut rack.slots {
                slot.id = 0;
            }
        }
    };
    file.patterns.iter_mut().for_each(strip);
    for group in &mut file.groups {
        if let Some(rack) = group.rack.as_mut() {
            rack.clips.iter_mut().for_each(|clip| strip(&mut clip.pattern));
        }
    }
    let legacy = ImportSource::from_file("legacy", round_trip(file)).expect("legacy source");
    let loose = 0;
    let mut rig = Rig::new();
    let app = rig.app();
    let (_, bank_b) = target(app);
    let summary = app
        .import_from_project(
            &legacy,
            &ImportRequest {
                kits: vec![kit],
                tracks: vec![loose],
                target: ImportTarget::Bank(Some(bank_b)),
                ..Default::default()
            },
        )
        .expect("import");
    let group = new_group(&summary, kit);
    assert!(kit_sounds_in(app, group, 2));
    let new_loose = summary.tracks.iter().find(|(old, _)| *old == loose).unwrap().1;
    launch(app, 2);
    let snapshot = app.state.latest_scheduler_snapshot();
    assert!(!snapshot.tracks[new_loose].scene_silenced);
    let rack = snapshot.tracks[new_loose].rack_track.clone().expect("a rack track");
    assert!(rack.slots.iter().all(|slot| slot.id != 0), "slots got ids");
    assert!(rack.slots.iter().any(|slot| slot.enabled), "and play");
}

/// "Sequencers travel with their owner": the kit's rack-owned neural
/// instance comes with it, as a fresh instance owned by the new kit.
#[test]
fn a_kits_neural_instance_comes_along_owned_by_the_new_kit() {
    let (source, kit) = source_project(true);
    let source_instance = source
        .file
        .instances
        .list
        .iter()
        .find(|instance| instance.owner == ProjectInstanceOwner::Rack(kit))
        .expect("the source kit owns a neural")
        .id;
    let mut rig = Rig::new();
    let app = rig.app();
    let _runtime = neural_runtime(app);
    let (_, bank_b) = target(app);
    let summary = app
        .import_from_project(
            &source,
            &ImportRequest {
                kits: vec![kit],
                target: ImportTarget::Bank(Some(bank_b)),
                ..Default::default()
            },
        )
        .expect("import");
    let group = new_group(&summary, kit);
    let owned = app.rack_instances(group);
    assert_eq!(owned.len(), 1, "{owned:?}");
    assert_eq!(owned[0].kind, NEURAL);
    assert_eq!(owned[0].owner, ProjectInstanceOwner::Rack(group));
    assert_ne!(owned[0].id, 0);
    assert_eq!(summary.instances, vec![(source_instance, owned[0].id)]);
    // Its per-clip overrides follow it to the new id.
    let (clips, keyed) = app.state.with_project_scenes(|s| {
        let bank = s.rack_bank(group).expect("a clip bank");
        let graphs: Vec<_> = bank.clips.iter().flat_map(|clip| clip.graph_overrides.iter()).collect();
        (
            graphs.len(),
            graphs.iter().all(|graph| graph.sequencer_id == owned[0].id && graph.owner_rack == Some(group)),
        )
    });
    assert!(clips > 0, "the clips carry the neural's overrides");
    assert!(keyed, "every clip override is keyed to the new instance");
}

/// A picked project-owned sequencer comes along as a new project-owned
/// instance: in the target bank its routes land on the imported tracks
/// (a route to a track left behind is cut and listed); every other scene
/// parks it (docs/rig-spec.md §10) with its nodes routed nowhere.
#[test]
fn a_picked_project_sequencer_routes_only_in_the_target_bank() {
    let (source, _) = source_project(true);
    let conductor = source
        .file
        .instances
        .list
        .iter()
        .find(|instance| instance.owner == ProjectInstanceOwner::Project)
        .expect("the conductor")
        .id;
    let keys = source.file.groups.iter().find(|g| !g.is_rack()).unwrap().clone();
    let mut rig = Rig::new();
    let app = rig.app();
    let _runtime = neural_runtime(app);
    let (_, bank_b) = target(app);
    let summary = app
        .import_from_project(
            &source,
            &ImportRequest {
                tracks: vec![keys.members[0]],
                sequencers: vec![conductor],
                target: ImportTarget::Bank(Some(bank_b)),
                ..Default::default()
            },
        )
        .expect("import");
    let new = summary.instances.iter().find(|(old, _)| *old == conductor).expect("imported").1;
    let instance = app.instances.get(new).expect("the new instance").clone();
    assert_eq!(instance.owner, ProjectInstanceOwner::Project);
    assert_ne!(new, conductor, "a fresh id");
    let new_keys = summary.tracks[0].1;
    let routes = |app: &App, scene: usize| -> Vec<Option<crate::graph::ProjectGraphRouteOverride>> {
        app.state.with_project_scenes(|s| {
            s.scenes[scene]
                .graph_overrides
                .iter()
                .find(|graph| graph.sequencer_id == new)
                .map(|graph| graph.node_intrinsics.iter().map(|node| node.route.clone()).collect())
                .unwrap_or_default()
        })
    };
    use crate::graph::ProjectGraphRouteOverride as Route;
    for scene in 2..4 {
        let routes = routes(app, scene);
        assert!(routes.contains(&Some(Route::Track(new_keys))), "scene {scene}: {routes:?}");
        assert!(!routes.iter().any(|route| matches!(route, Some(Route::Track(t)) if *t != new_keys)));
    }
    for scene in 0..2 {
        let routes = routes(app, scene);
        assert!(!routes.is_empty(), "bank A scene {scene} has the off override");
        assert!(routes.iter().all(|route| route == &Some(Route::None)), "scene {scene}: {routes:?}");
    }
    assert!(
        summary.unconnected.iter().any(|line| line.contains("conductor")),
        "the route to the track left behind is listed: {:?}",
        summary.unconnected
    );
    // Parked outside the target bank, on in it (§10): no limitation note.
    app.state.with_project_scenes(|s| {
        for scene in 0..4 {
            assert_eq!(s.instance_parked_in_scene(scene, new), scene < 2, "scene {scene}");
        }
    });
    assert!(!summary.notes.iter().any(|note| note.contains("§10")), "{:?}", summary.notes);
    // Node process slots get fresh ids, never the source's (review item 4).
    let slot_ids = |graphs: &[crate::graph::ProjectGraphOverrides], id: u64| -> Vec<u64> {
        graphs
            .iter()
            .filter(|graph| graph.sequencer_id == id)
            .flat_map(|graph| graph.node_intrinsics.iter())
            .filter_map(|node| node.process_chain.as_ref())
            .flat_map(|chain| chain.slots.iter().map(|slot| slot.instance_id.0))
            .collect()
    };
    let source_slots = slot_ids(&source.file.patterns[0].graph_overrides, conductor);
    assert!(!source_slots.is_empty(), "the source conductor has a node process slot");
    let imported = app.state.with_project_scenes(|s| slot_ids(&s.scenes[2].graph_overrides, new));
    assert_eq!(imported.len(), source_slots.len());
    assert!(imported.iter().all(|id| !source_slots.contains(id)), "{imported:?} vs {source_slots:?}");
}

/// The steps of `track`'s cell in each of `scenes`.
fn cell_bits(app: &App, track: usize, scenes: std::ops::Range<usize>) -> Vec<Option<u64>> {
    app.state.with_project_scenes(|s| {
        scenes
            .map(|scene| {
                s.composed_scene_cell(scene, track)
                    .and_then(|id| s.track_pools[track].get(id))
                    .map(|data| data.track_bits[0])
            })
            .collect()
    })
}

/// Review item 1: importing into the bank that is PLAYING must not let the
/// closing save-back write the new tracks' empty live grid over the
/// imported lanes.
#[test]
fn importing_into_the_playing_bank_keeps_every_imported_step() {
    let (source, kit) = source_project(false);
    let keys_a = source.file.groups.iter().find(|g| !g.is_rack()).unwrap().members[0];
    let kick = source.group(kit).unwrap().members[0];
    let mut rig = Rig::new();
    let app = rig.app();
    let (bank_a, _) = target(app);
    assert_eq!(app.state.current_scene_index(), 0, "bank A plays");
    let summary = app
        .import_from_project(
            &source,
            &ImportRequest {
                kits: vec![kit],
                tracks: vec![keys_a],
                target: ImportTarget::Bank(Some(bank_a)),
                ..Default::default()
            },
        )
        .expect("import");
    let new = |old: usize| summary.tracks.iter().find(|(o, _)| *o == old).unwrap().1;
    // Play around the bank: every launch saves the live grid back first.
    for scene in [1, 0, 1, 0] {
        launch(app, scene);
    }
    let keys_bits = source.file.patterns[0].track_bits[keys_a][0];
    assert_ne!(keys_bits, 0);
    assert_eq!(cell_bits(app, new(keys_a), 0..2), vec![Some(keys_bits); 2], "the keys steps survive");
    assert_eq!(cell_bits(app, new(kick), 0..2), vec![Some(0x1111); 2], "the kick steps survive");
}

/// Review item 2: a kit clip lane's sends follow the import; a send to a
/// bus that did not come along is cut and listed, not left on the target's
/// bus with the same id.
#[test]
fn a_kit_lanes_send_to_a_bus_left_behind_is_cut_not_mispointed() {
    let (source, kit) = source_project_with(false, |app, kit, _| {
        app.state.with_scenes_mut(|scenes| {
            let bank = scenes.rack_bank(kit).unwrap().clone();
            for clip in &bank.clips {
                for (position, cell) in clip.cells.iter().enumerate() {
                    let (Some(cell), Some(track)) = (cell, bank.members.get(position)) else {
                        continue;
                    };
                    scenes.track_pools[*track].edit(*cell, |lane| {
                        lane.track_params.sends.push(TrackSendSnapshot {
                            destination: BusId(crate::sequencer::DEFAULT_BUS_A_ID),
                            amount: 0.5,
                        });
                    });
                }
            }
        });
    });
    let mut rig = Rig::new();
    let app = rig.app();
    let (_, bank_b) = target(app);
    assert!(app.buses.iter().any(|bus| bus.id.0 == crate::sequencer::DEFAULT_BUS_A_ID), "a decoy");
    let summary = app
        .import_from_project(
            &source,
            &ImportRequest { kits: vec![kit], target: ImportTarget::Bank(Some(bank_b)), ..Default::default() },
        )
        .expect("import");
    assert!(
        summary.unconnected.iter().any(|line| line.contains("send to 'Bus A'")),
        "{:?}",
        summary.unconnected
    );
    let group = new_group(&summary, kit);
    let sends: Vec<BusId> = app.state.with_project_scenes(|s| {
        let bank = s.rack_bank(group).unwrap();
        bank.clips
            .iter()
            .flat_map(|clip| clip.cells.iter().enumerate())
            .filter_map(|(position, cell)| s.track_pools[bank.members[position]].get((*cell)?))
            .flat_map(|lane| lane.track_params.sends.into_iter().map(|send| send.destination))
            .collect()
    });
    assert!(sends.is_empty(), "no send left on the target's Bus A: {sends:?}");
}

/// Review item 3: a sidechain selector is positional over the tracks other
/// than its owner. It is decoded in the source's track space and encoded in
/// this project's; a source track that did not come along selects nothing.
#[test]
fn sidechain_selections_follow_the_imported_tracks() {
    let (source, _) = source_project(false);
    let mut descriptor = crate::effects::EffectDescriptor::empty_custom_slot();
    descriptor.params.push(crate::effects::ParamDescriptor {
        name: "sidechain".to_string(),
        min: 0.0,
        max: 8.0,
        default: 0.0,
        kind: crate::effects::ParamKind::Enum { labels: Vec::new() },
        scaling: crate::effects::ParamScaling::Linear,
        node_param_idx: u32::MAX,
        node_param_span: 1,
        host_control: Some(crate::effects::HostControl::FxSidechain { input_channel: 2 }),
        ui_metadata: None,
        percent_ratio: false,
    });
    let slot = |selection: f32| {
        let mut slot = crate::effects::EffectSlotSnapshot::new_empty();
        slot.num_params = 1;
        slot.defaults = vec![selection];
        slot
    };
    // Source: owner track 4, five tracks; selection 1 = track 0, 4 = track 3.
    let mut ids = links::IdMap { source_tracks: 5, target_tracks: 9, ..Default::default() };
    ids.tracks.insert(4, 8);
    ids.tracks.insert(0, 6);
    let mut cut = Lines::default();
    let mut slots = vec![slot(1.0), slot(4.0)];
    links::remap_slot_sidechains(&mut slots, &[descriptor.clone(), descriptor], (4, 8), &ids, &source, "t", &mut cut);
    // Target: owner 8 of nine tracks; track 6 is selection 7.
    assert_eq!(slots[0].defaults[0], 7.0, "track 0 -> its import, track 6");
    assert_eq!(slots[1].defaults[0], 0.0, "track 3 was not imported");
    assert!(cut.iter().any(|line| line.contains("sidechain")), "{cut:?}");
    assert_eq!(links::remap_bus_sidechain(1, &ids, &source, "bus", &mut cut), 7, "bus selector: track + 1");
}

/// Review item 5 / 7b: a kit's own mod cable arrives once, with its source
/// output, in the target bank only.
#[test]
fn a_kits_own_mod_cable_arrives_once_in_the_target_bank_only() {
    let (source, kit) = source_project_with(false, |app, kit, kick| {
        let modulator = app.graph_controller().add_modulator_track().expect("modulator");
        app.assign_rack_pad_track_recorded(kit, 40, modulator).expect("modulator pad");
        app.state.with_scenes_mut(|scenes| {
            for scene in &mut scenes.scenes {
                scene.mod_connections.push(ModConnection {
                    source_output: 1,
                    source_track: modulator,
                    destination: ModDestination::Track(kick),
                    dest_input: 2,
                });
            }
        });
    });
    let mut rig = Rig::new();
    let app = rig.app();
    let (_, bank_b) = target(app);
    let summary = app
        .import_from_project(
            &source,
            &ImportRequest { kits: vec![kit], target: ImportTarget::Bank(Some(bank_b)), ..Default::default() },
        )
        .expect("import");
    assert!(
        !summary.unconnected.iter().any(|line| line.contains("mod cable")),
        "{:?}",
        summary.unconnected
    );
    let group = new_group(&summary, kit);
    let members = app.groups.iter().find(|g| g.id == group).unwrap().members.clone();
    let kit_cables = |scene: usize| -> Vec<ModConnection> {
        app.state.with_project_scenes(|s| {
            s.scenes[scene]
                .mod_connections
                .iter()
                .copied()
                .filter(|cable| members.contains(&cable.source_track))
                .collect()
        })
    };
    for scene in 2..4 {
        let cables = kit_cables(scene);
        assert_eq!(cables.len(), 1, "scene {scene}: {cables:?}");
        assert_eq!((cables[0].source_output, cables[0].dest_input), (1, 2));
    }
    for scene in 0..2 {
        assert!(kit_cables(scene).is_empty(), "bank A scene {scene} gets no kit cable");
    }
}

/// Review item 10: a group's bus effects survive undo and redo of the
/// import, which is one step labelled after the source.
#[test]
fn a_groups_bus_effects_survive_undo_and_redo() {
    let (source, _) = source_project_with(false, |app, _, _| {
        let bus = app.groups.iter().find(|g| !g.is_rack()).unwrap().bus_id;
        let bus_idx = app.buses.iter().position(|b| b.id.0 == bus).unwrap();
        app.add_builtin_bus_effect_sync(bus_idx, "Reverb").expect("a bus effect");
    });
    let keys = source.file.groups.iter().find(|g| !g.is_rack()).unwrap().clone();
    let mut rig = Rig::new();
    let app = rig.app();
    target(app);
    let summary = app
        .import_from_project(&source, &ImportRequest { groups: vec![keys.id], ..Default::default() })
        .expect("import");
    let group = new_group(&summary, keys.id);
    let effects = |app: &App| -> Vec<String> {
        app.groups
            .iter()
            .find(|g| g.id == group)
            .and_then(|g| app.buses.iter().find(|b| b.id.0 == g.bus_id))
            .map(|bus| bus.custom_effect_names.iter().flatten().cloned().collect())
            .unwrap_or_default()
    };
    assert_eq!(effects(app).len(), 1, "the group bus has its effect");
    assert_eq!(app.groups.iter().find(|g| g.id == group).unwrap().name, keys.name);
    let crate::app::history::HistoryReplay::Applied(step) = crate::app::edit::undo(app) else {
        panic!("undo");
    };
    assert_eq!(step.label, "Import from source");
    assert!(!app.groups.iter().any(|g| g.id == group));
    let crate::app::history::HistoryReplay::Applied(_) = crate::app::edit::redo(app) else {
        panic!("redo");
    };
    assert_eq!(effects(app).len(), 1, "redo brings the bus effect back");
}

/// Review item 11: an empty source bank makes no bank, and picks that no
/// longer name what the peek showed refuse the import.
#[test]
fn an_empty_bank_or_a_changed_file_refuses_the_import() {
    let (source, kit) = source_project(false);
    let mut file = source.file.clone();
    file.scene_banks.push(crate::project::ProjectSceneBank { id: 99, name: None, len: 0 });
    let source = ImportSource::from_file("source", file).unwrap();
    let mut rig = Rig::new();
    let app = rig.app();
    target(app);
    let banks = app.state.scene_banks().len();
    let undo = app.history.undo_len();
    let error = app
        .import_from_project(&source, &ImportRequest { banks: vec![99], ..Default::default() })
        .expect_err("an empty bank");
    assert!(error.contains("no scenes"), "{error}");
    assert_eq!(app.state.scene_banks().len(), banks, "no bank left behind");
    let error = app
        .import_from_project(
            &source,
            &ImportRequest {
                kits: vec![kit],
                expected_names: vec![(PickKind::Kit, kit, "Another Kit".to_string())],
                ..Default::default()
            },
        )
        .expect_err("a renamed kit");
    assert!(error.contains("changed since"), "{error}");
    assert_eq!(app.history.undo_len(), undo, "nothing recorded");
}

/// Manual check against the user's real project (a stripped copy, read
/// only): its piano kit imports into a fresh project. Ignored: it needs the
/// local fixture and its samples.
#[test]
#[ignore]
fn the_gotham_piano_kit_imports_into_a_fresh_project() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.local/benchmarks/park-probe-2026-10-23/gotham/fixture.json");
    let source = ImportSource::open(&path).expect("gotham fixture");
    let piano = source
        .peek()
        .kits
        .iter()
        .find(|kit| kit.name.contains("Piano"))
        .expect("the piano kit")
        .group_id;
    let mut rig = Rig::new();
    let app = rig.app();
    let _runtime = neural_runtime(app);
    app.graph_controller().add_track(Path::new(PLATE)).expect("a first track");
    let summary = app
        .import_from_project(&source, &ImportRequest { kits: vec![piano], ..Default::default() })
        .expect("import the piano kit");
    eprintln!("{}", summary.lines().join("\n"));
    let group = new_group(&summary, piano);
    let scene = app
        .state
        .with_project_scenes(|s| s.scene_bank_range(summary.bank.unwrap()))
        .unwrap()
        .start;
    assert!(kit_sounds_in(app, group, scene));
    assert_eq!(app.rack_instances(group).len(), 1, "its neural came along");
}

/// Docs/rig-spec.md §10 in an import: a new bank gets the default-active
/// rule, so this project's own project-owned conductors are parked in its
/// scenes (and nowhere else); the imported conductor plays only there; and a
/// copied chain's `:owner` names the imported conductor's NEW id.
#[test]
fn a_new_bank_import_parks_the_masters_conductors_and_re_keys_process_owners() {
    // The source: the conductor owns a process slot on the first keys track.
    let (source, _) = source_project_with(true, |app, _, _| {
        let conductor = app
            .instances
            .list
            .iter()
            .find(|instance| instance.owner == ProjectInstanceOwner::Project)
            .expect("the conductor")
            .id;
        let keys = app.groups.iter().find(|g| !g.is_rack()).unwrap().members[0];
        let mut slot = crate::process::TrackProcessSlot::new(
            crate::process::ProcessInstanceId(4242),
            "lane-count".to_string(),
        );
        slot.owner_instance = Some(conductor);
        let mut chain = app.state.track_process_chain(keys).unwrap();
        chain.slots.push(slot);
        assert!(app.state.set_track_process_chain(keys, chain));
    });
    let conductor = source
        .file
        .instances
        .list
        .iter()
        .find(|instance| instance.owner == ProjectInstanceOwner::Project)
        .expect("the conductor")
        .id;
    let keys = source.file.groups.iter().find(|g| !g.is_rack()).unwrap().clone();
    let mut rig = Rig::new();
    let app = rig.app();
    let _runtime = neural_runtime(app);
    target(app);
    let master = app
        .create_instance_recorded(NEURAL, ProjectInstanceOwner::Project, Some("master conductor".into()))
        .expect("a master conductor");
    // On everywhere in the target, as a legacy conductor is.
    app.state.with_scenes_mut(|s| s.forget_parked(crate::sequencer::ParkedSet::Instances, &[master]));
    let before = app.state.scene_count();
    let summary = app
        .import_from_project(
            &source,
            &ImportRequest {
                tracks: vec![keys.members[0]],
                sequencers: vec![conductor],
                target: ImportTarget::NewBank,
                ..Default::default()
            },
        )
        .expect("import");
    let new = summary.instances.iter().find(|(old, _)| *old == conductor).expect("imported").1;
    let after = app.state.scene_count();
    assert!(after > before, "a new bank");
    app.state.with_project_scenes(|s| {
        for scene in 0..after {
            let in_new_bank = scene >= before;
            assert_eq!(s.instance_parked_in_scene(scene, master), in_new_bank, "master, scene {scene}");
            assert_eq!(s.instance_parked_in_scene(scene, new), !in_new_bank, "imported, scene {scene}");
        }
    });
    let new_keys = summary.tracks[0].1;
    let owners: Vec<Option<u64>> = app.state.with_project_scenes(|s| {
        s.track_pools[new_keys]
            .sounds
            .patches
            .values()
            .flat_map(|patch| patch.process_chain.slots.iter())
            .filter(|slot| slot.class_name == "lane-count")
            .map(|slot| slot.owner_instance)
            .collect()
    });
    assert!(!owners.is_empty(), "the owned slot came along");
    assert!(owners.iter().all(|owner| *owner == Some(new)), "re-keyed to the new id: {owners:?} (new {new})");
}
