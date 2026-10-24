//! Drop-to-swap scoped to the viewed bank (docs/rig-spec.md §4,
//! eseq-ka5t.9), on a headless engine.

use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::app::edit::{redo, undo};
use crate::app::history::HistoryReplay;
use crate::app::test_rig::{launch, new_scene, rack_with_samplers, Rig, PIANO, PLATE};
use crate::app::EngineDescriptor;
use crate::lisp_host;
use crate::sequencer::{CustomInstrumentRunMode, SceneId, SlotEnable};

fn test_manifest() -> lisp_host::DGenManifest {
    lisp_host::DGenManifest {
        effect_latency_samples: None, tail_sleep: false,
        dylib_path: std::path::PathBuf::new(),
        asset_base: None,
        version: 1,
        process_abi: String::new(),
        total_memory_slots: 1,
        params: Vec::new(),
        groups: Vec::new(),
        envelopes: Vec::new(),
        inputs: ["gate", "pitch", "velocity", "trigger"]
            .into_iter()
            .enumerate()
            .map(|(channel, name)| lisp_host::DGenInput {
                channel,
                name: name.to_string(),
            })
            .collect(),
        modulators: Vec::new(),
        mod_outputs: Vec::new(),
        amp_output_channel: None,
        probes: Vec::new(),
        mod_destinations: Vec::new(),
        n_inputs: 4,
        n_outputs: 1,
        tensors: Vec::new(),
        tensor_init_data: Vec::new(),
        voice_cell_id: None,
        binding_shapes: Default::default(),
    }
}

/// A silent compiled-instrument stand-in named `name`.
fn fake_instrument(app: &mut App, name: &str) -> SlotDevice {
    let manifest = test_manifest();
    let lib_index = app.editor.instrument_libs.len();
    app.editor
        .instrument_libs
        .push(lisp_host::test_loaded_dgen_lib());
    let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
        name: name.to_string(),
        source: format!("{name}.lisp"),
        manifest: manifest.clone(),
        lib_index,
        shared_runtime: true,
    });
    SlotDevice::Instrument(PreparedRackInstrument {
        name: name.to_string(),
        engine_id,
        manifest,
        lib_index,
        run_mode: CustomInstrumentRunMode::Instrument,
    })
}

/// Four scenes: 0 and 1 in bank A, 2 and 3 in bank B.
fn two_banks(app: &mut App) -> (SceneBankId, SceneBankId) {
    for _ in 0..3 {
        new_scene(app);
    }
    let bank_b = app.state.create_scene_bank().unwrap();
    for _ in 0..2 {
        app.state.move_scene_to_scene_bank(2, bank_b).unwrap();
    }
    let bank_a = app.state.scene_banks()[0].id;
    assert_eq!(app.state.scene_bank_range(bank_b), Some(2..4));
    (bank_a, bank_b)
}

/// Every scene plays `track` on scene 0's pattern.
fn share_one_pattern(app: &mut App, track: usize) {
    app.state.with_scenes_mut(|scenes| {
        let cell = scenes.scenes[0].cells[track];
        for scene in &mut scenes.scenes {
            scene.cells[track] = cell;
        }
    });
}

fn device_name(slot: &RackSlotSnapshot) -> String {
    match slot.instrument_type {
        InstrumentType::Sampler => slot
            .sample_id
            .as_ref()
            .map(|(_, name, _)| name.clone())
            .unwrap_or_default(),
        _ => slot
            .track_sound_state
            .loaded_preset
            .clone()
            .unwrap_or_default(),
    }
}

/// The devices scene `scene` plays on rack `track`, in slot order, as the
/// published scheduler snapshot (what triggers reach) has them.
fn playing(app: &mut App, scene: usize, track: usize) -> Vec<String> {
    launch(app, scene);
    let snapshot = app.state.latest_scheduler_snapshot();
    snapshot.tracks[track]
        .rack_track
        .as_ref()
        .expect("a rack")
        .slots
        .iter()
        .filter(|slot| slot.enabled)
        .map(device_name)
        .collect()
}

fn every_scene_plays(app: &mut App, track: usize) -> Vec<Vec<String>> {
    let current = app.state.playing_scene();
    let plays = (0..app.state.scene_count())
        .map(|scene| playing(app, scene, track))
        .collect();
    launch(app, current);
    plays
}

fn slot_ids(app: &App, track: usize) -> Vec<u64> {
    app.state
        .live_rack_track_snapshot(track)
        .unwrap()
        .slots
        .iter()
        .map(|slot| slot.id)
        .collect()
}

fn slot_devices(app: &App, track: usize) -> Vec<String> {
    app.state
        .live_rack_track_snapshot(track)
        .unwrap()
        .slots
        .iter()
        .map(device_name)
        .collect()
}

fn scene_entries(app: &App, track: usize) -> Vec<(SceneId, SlotEnable)> {
    app.state
        .with_project_scenes(|scenes| scenes.track_slot_enable(track))
}

/// Every scene's slot set for `track`, by scene position.
fn scene_sets(app: &App, track: usize) -> Vec<Option<SlotEnable>> {
    app.state.with_project_scenes(|scenes| {
        (0..scenes.scenes.len())
            .map(|scene| scenes.scene_slot_enable(scene, track).cloned())
            .collect()
    })
}

fn sample_name(path: &str) -> String {
    crate::sample_db::display_title_for_sample_path(Path::new(path)).unwrap_or_else(|| {
        Path::new(path)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    })
}

/// Every pool pattern's devices for `track`, slot by slot.
fn pool_devices(app: &App, track: usize) -> Vec<Vec<(u64, String)>> {
    app.state.with_project_scenes(|scenes| {
        scenes.track_pools[track]
            .sounds
            .patches
            .values()
            .filter_map(|patch| patch.rack_track.as_ref())
            .map(|rack| {
                rack.slots
                    .iter()
                    .map(|slot| (slot.id, device_name(slot)))
                    .collect()
            })
            .collect()
    })
}

/// A global macro, engaged at 0.7, mapped onto rack slot `slot`'s gain.
fn engaged_rack_gain_macro(
    app: &mut App,
    rack: usize,
    slot: usize,
) -> crate::macro_engine::MacroId {
    let id = app
        .macro_engine
        .create_macro("slot gain", crate::macro_engine::MacroKind::Mapped)
        .unwrap();
    let mapping = crate::macro_engine::MacroMapping::new_resolved(
        rack,
        crate::process::ParamTarget::RackSlotParam {
            slot,
            param: "gain".to_string(),
        },
        None,
        0.0,
        2.0,
        crate::macro_engine::MacroCurve::Linear,
    )
    .unwrap();
    app.macro_engine.add_mapping(id, mapping).unwrap();
    app.set_macro_value(id, 0.7);
    id
}

fn rack_gain_key(rack: usize, slot: usize) -> crate::macro_engine::MacroParamKey {
    crate::macro_engine::MacroParamKey::RackSlot {
        track: rack,
        slot,
        param: "gain".to_string(),
    }
}

#[test]
fn a_bank_swap_on_a_shared_pattern_inserts_after_the_old_slot_and_keeps_other_banks() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE, PIANO]);
    let (bank_a, bank_b) = two_banks(app);
    share_one_pattern(app, rack);
    let synth = fake_instrument(app, "swap-synth");
    let (plate, piano) = (sample_name(PLATE), sample_name(PIANO));
    let ids_before = slot_ids(app, rack);
    let entries_before = scene_entries(app, rack);
    let plays_before = every_scene_plays(app, rack);
    let undo_before = app.history.undo_len();

    let outcome = app
        .swap_rack_slot_recorded(rack, 1, synth, SwapScope::Bank(Some(bank_b)))
        .unwrap();

    assert_eq!(outcome.kind, SwapKind::Inserted);
    assert_eq!(outcome.slot, 2, "directly after the old slot");
    assert_eq!(outcome.kept_banks, vec![bank_a]);
    assert_eq!(app.history.undo_len(), undo_before + 1, "one undo step");
    assert_eq!(
        slot_devices(app, rack),
        vec![plate.clone(), piano.clone(), "swap-synth".to_string()]
    );
    let ids = slot_ids(app, rack);
    assert_eq!(&ids[..2], &ids_before[..], "the old slots keep their ids");
    for pattern in pool_devices(app, rack) {
        assert_eq!(pattern.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
    }
    let plays = every_scene_plays(app, rack);
    assert_eq!(
        plays[0],
        vec![plate.clone(), piano.clone()],
        "bank A keeps the piano"
    );
    assert_eq!(plays[1], vec![plate.clone(), piano.clone()]);
    assert_eq!(
        plays[2],
        vec![plate.clone(), "swap-synth".to_string()],
        "bank B plays the synth"
    );
    assert_eq!(plays[3], vec![plate.clone(), "swap-synth".to_string()]);

    assert!(matches!(undo(app), HistoryReplay::Applied(_)));
    assert_eq!(slot_ids(app, rack), ids_before);
    assert_eq!(slot_devices(app, rack), vec![plate, piano]);
    assert_eq!(
        scene_entries(app, rack),
        entries_before,
        "undo restores every scene exactly"
    );
    assert_eq!(every_scene_plays(app, rack), plays_before);

    assert!(matches!(redo(app), HistoryReplay::Applied(_)));
    assert_eq!(slot_ids(app, rack), ids);
    assert_eq!(every_scene_plays(app, rack), plays);
}

#[test]
fn a_bank_swap_of_a_slot_no_other_bank_plays_overwrites_it_in_every_pattern() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE, PIANO]);
    let (bank_a, bank_b) = two_banks(app);
    // Bank A no longer plays the piano: only B uses it.
    assert!(app
        .set_rack_slot_enabled_in_bank_recorded(rack, 1, false, Some(bank_a))
        .unwrap());
    let synth = fake_instrument(app, "overwrite-synth");
    let ids_before = slot_ids(app, rack);

    let outcome = app
        .swap_rack_slot_recorded(rack, 1, synth, SwapScope::Bank(Some(bank_b)))
        .unwrap();

    assert_eq!(outcome.kind, SwapKind::Overwritten);
    assert_eq!(outcome.slot, 1);
    assert!(outcome.kept_banks.is_empty());
    assert_eq!(slot_ids(app, rack), ids_before, "same id, no extra row");
    for pattern in pool_devices(app, rack) {
        assert_eq!(
            pattern[1],
            (ids_before[1], "overwrite-synth".to_string()),
            "every pattern"
        );
    }
    let plate = sample_name(PLATE);
    let plays = every_scene_plays(app, rack);
    assert_eq!(plays[0], vec![plate.clone()]);
    assert_eq!(plays[2], vec![plate.clone(), "overwrite-synth".to_string()]);
    assert_eq!(plays[3], vec![plate, "overwrite-synth".to_string()]);
}

#[test]
fn a_sample_on_a_sampler_slot_is_content_of_that_pattern_only() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE]);
    let (_, bank_b) = two_banks(app);
    let cells = app.state.with_project_scenes(|scenes| {
        (
            scenes.composed_scene_cell(0, rack),
            scenes.composed_scene_cell(2, rack),
        )
    });
    assert_ne!(cells.0, cells.1, "the scenes play their own patterns");
    launch(app, 2);
    let ids_before = slot_ids(app, rack);

    let outcome = app
        .swap_rack_slot_to_sample(rack, 0, Path::new(PIANO), SwapScope::Bank(Some(bank_b)))
        .unwrap();

    assert_eq!(outcome.kind, SwapKind::Content);
    assert_eq!(slot_ids(app, rack), ids_before, "no new slot for a sample");
    let (plate, piano) = (sample_name(PLATE), sample_name(PIANO));
    let plays = every_scene_plays(app, rack);
    assert_eq!(
        plays[2],
        vec![piano],
        "the dropped-on pattern (a kit pad's clip) changed"
    );
    assert_eq!(
        plays[0],
        vec![plate.clone()],
        "every other pattern keeps its sample"
    );
    assert_eq!(plays[3], vec![plate]);
}

#[test]
fn an_insert_keeps_macros_slot_fx_identities_and_selection_on_their_devices() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE, PIANO, PLATE]);
    let (_, bank_b) = two_banks(app);
    // Rack macro 1 drives slot 3's gain; slot 3 runs a filter.
    let mapping = crate::sequencer::RackMacroMapping {
        target: crate::sequencer::RackMacroTarget::SlotParam {
            slot: 2,
            param: "gain".to_string(),
        },
        range_min: 0.0,
        range_max: 1.0,
        curve: Default::default(),
    };
    {
        let mut racks = app.state.pattern.rack_tracks.lock().unwrap();
        racks[rack].as_mut().unwrap().macros[0]
            .mappings
            .push(mapping.clone());
    }
    app.state.with_scenes_mut(|scenes| {
        for patch in scenes.track_pools[rack]
            .sounds
            .patches
            .values_mut()
            .map(Arc::make_mut)
        {
            if let Some(rack) = patch.rack_track.as_mut() {
                rack.macros[0].mappings.push(mapping.clone());
            }
        }
    });
    app.add_builtin_rack_slot_effect_sync(rack, 2, "filter")
        .unwrap();
    let track_id = app.track_registry.id_at(rack).unwrap();
    let moved_id = slot_ids(app, rack)[2];
    let moved_identity = app.device_registry.rack_slot(track_id, 2);
    let fx_nodes = |slot: &RackSlotSnapshot| {
        slot.effect_slots
            .iter()
            .map(|effect| effect.node_id)
            .collect::<Vec<_>>()
    };
    let moved_fx = fx_nodes(&app.state.live_rack_track_snapshot(rack).unwrap().slots[2]);
    assert!(
        moved_fx.iter().any(|node| *node != 0),
        "slot 3 runs an effect"
    );
    // An engaged global macro drives slot 3's gain too.
    let global = engaged_rack_gain_macro(app, rack, 2);
    assert!(app
        .macro_engine
        .overrides()
        .contains_key(&rack_gain_key(rack, 2)));
    let synth = fake_instrument(app, "insert-synth");

    let outcome = app
        .swap_rack_slot_recorded(rack, 1, synth, SwapScope::Bank(Some(bank_b)))
        .unwrap();

    assert_eq!(outcome.kind, SwapKind::Inserted);
    assert_eq!(outcome.slot, 2);
    assert_eq!(
        app.macro_engine.macro_definition(global).unwrap().mappings[0].target,
        crate::process::ParamTarget::RackSlotParam {
            slot: 3,
            param: "gain".to_string()
        },
    );
    let overrides = app.macro_engine.overrides();
    assert!(
        overrides.contains_key(&rack_gain_key(rack, 3)),
        "the engaged macro follows"
    );
    assert!(
        !overrides.contains_key(&rack_gain_key(rack, 2)),
        "not the new slot"
    );
    let live = app.state.live_rack_track_snapshot(rack).unwrap();
    assert_eq!(live.slots[3].id, moved_id, "the old slot 3 is now slot 4");
    assert_eq!(
        live.macros[0].mappings[0].target,
        crate::sequencer::RackMacroTarget::SlotParam {
            slot: 3,
            param: "gain".to_string()
        },
        "the macro still drives the same device"
    );
    assert_eq!(
        fx_nodes(&live.slots[3]),
        moved_fx,
        "its effects moved with it"
    );
    assert!(live.slots[2]
        .effect_slots
        .iter()
        .all(|effect| effect.node_id == 0));
    assert!(app.editor.effect_chain_leases.contains_host(
        crate::app::fx_chain::FxChainLocator::RackSlot {
            track: rack,
            slot: 3
        }
    ));
    assert_eq!(app.device_registry.rack_slot(track_id, 3), moved_identity);
    assert_eq!(
        app.rack_selected_slot(rack, live.slots.len()),
        2,
        "the new slot is selected"
    );
}

#[test]
fn an_empty_row_add_plays_in_the_viewed_bank_only() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE]);
    let (_, bank_b) = two_banks(app);
    let undo_before = app.history.undo_len();

    let slot = app
        .add_rack_sample_slot_in_bank_recorded(rack, Path::new(PIANO), Some(bank_b))
        .unwrap();

    assert_eq!(slot, 1);
    assert_eq!(app.history.undo_len(), undo_before + 1);
    let id = slot_ids(app, rack)[1];
    assert_eq!(app.state.scenes_enabling_rack_slot(rack, id), vec![2, 3]);
    for pattern in pool_devices(app, rack) {
        assert_eq!(pattern.len(), 2, "every pattern has the slot");
    }
    assert!(matches!(undo(app), HistoryReplay::Applied(_)));
    assert_eq!(slot_ids(app, rack).len(), 1);
    assert!(matches!(redo(app), HistoryReplay::Applied(_)));
    assert_eq!(
        app.state
            .scenes_enabling_rack_slot(rack, slot_ids(app, rack)[1]),
        vec![2, 3]
    );
}

#[test]
fn a_drop_on_a_plain_track_two_banks_play_converts_it_for_the_bank() {
    let mut rig = Rig::new();
    let app = rig.app();
    let track = app.graph_controller().add_track(Path::new(PLATE)).unwrap();
    let (_, bank_b) = two_banks(app);
    assert!(app
        .plain_track_swap_needs_rack(track, Some(bank_b))
        .unwrap());
    let buffer_before = app.graph.track_buffer_ids[track];
    let synth = fake_instrument(app, "plain-synth");
    let undo_before = app.history.undo_len();

    let outcome = app
        .swap_plain_track_recorded(track, synth, Some(bank_b))
        .unwrap();

    assert!(outcome.converted);
    assert_eq!(outcome.kind, SwapKind::Inserted);
    assert_eq!(
        app.history.undo_len(),
        undo_before + 1,
        "conversion and swap are one step"
    );
    assert_eq!(
        app.graph.track_instrument_types[track],
        InstrumentType::Rack
    );
    let plate = sample_name(PLATE);
    let plays = every_scene_plays(app, track);
    assert_eq!(plays[0], vec![plate.clone()], "bank A sounds as before");
    assert_eq!(plays[1], vec![plate]);
    assert_eq!(plays[2], vec!["plain-synth".to_string()]);
    assert_eq!(plays[3], vec!["plain-synth".to_string()]);

    assert!(matches!(undo(app), HistoryReplay::Applied(_)));
    assert_eq!(
        app.graph.track_instrument_types[track],
        InstrumentType::Sampler
    );
    assert_eq!(app.graph.track_buffer_ids[track], buffer_before);
    assert!(
        scene_entries(app, track).is_empty(),
        "a plain track owns no slot sets"
    );
}

#[test]
fn a_plain_track_only_the_bank_plays_is_not_converted() {
    let mut rig = Rig::new();
    let app = rig.app();
    let track = app.graph_controller().add_track(Path::new(PLATE)).unwrap();
    let (_, bank_b) = two_banks(app);
    app.state.with_scenes_mut(|scenes| {
        scenes.scenes[0].cells[track] = None;
        scenes.scenes[1].cells[track] = None;
    });
    assert!(!app
        .plain_track_swap_needs_rack(track, Some(bank_b))
        .unwrap());
}

#[test]
fn replace_in_all_banks_overwrites_the_slot_for_every_scene() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE, PIANO]);
    two_banks(app);
    let synth = fake_instrument(app, "everywhere-synth");
    let ids_before = slot_ids(app, rack);

    let outcome = app
        .swap_rack_slot_recorded(rack, 1, synth, SwapScope::AllBanks)
        .unwrap();

    assert_eq!(outcome.kind, SwapKind::Overwritten);
    assert_eq!(slot_ids(app, rack), ids_before);
    let plate = sample_name(PLATE);
    for plays in every_scene_plays(app, rack) {
        assert_eq!(plays, vec![plate.clone(), "everywhere-synth".to_string()]);
    }
}

#[test]
fn use_only_this_in_a_bank_switches_the_bank_alone_as_one_step() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE, PIANO]);
    let (_, bank_b) = two_banks(app);
    let before = every_scene_plays(app, rack);
    let undo_before = app.history.undo_len();

    assert!(app
        .use_only_rack_slot_in_bank_recorded(rack, 1, Some(bank_b))
        .unwrap());
    assert!(
        !app.use_only_rack_slot_in_bank_recorded(rack, 1, Some(bank_b))
            .unwrap(),
        "no-op"
    );

    assert_eq!(app.history.undo_len(), undo_before + 1);
    let plays = every_scene_plays(app, rack);
    let piano = sample_name(PIANO);
    assert_eq!(plays[0], before[0], "bank A untouched");
    assert_eq!(plays[2], vec![piano.clone()]);
    assert_eq!(plays[3], vec![piano]);
    assert!(matches!(undo(app), HistoryReplay::Applied(_)));
    assert_eq!(every_scene_plays(app, rack), before);
}

#[test]
fn purge_deletes_the_slots_no_scene_plays_as_one_step() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE, PIANO, PLATE]);
    let (bank_a, bank_b) = two_banks(app);
    for bank in [bank_a, bank_b] {
        assert!(app
            .set_rack_slot_enabled_in_bank_recorded(rack, 1, false, Some(bank))
            .unwrap());
    }
    let ids_before = slot_ids(app, rack);
    let entries_before = scene_entries(app, rack);
    assert_eq!(
        app.unused_rack_slot_names(rack),
        vec![(1, sample_name(PIANO))]
    );
    let undo_before = app.history.undo_len();

    assert_eq!(app.purge_unused_rack_slots_recorded(rack).unwrap(), vec![1]);

    assert_eq!(app.history.undo_len(), undo_before + 1);
    assert_eq!(slot_ids(app, rack), vec![ids_before[0], ids_before[2]]);
    assert!(matches!(undo(app), HistoryReplay::Applied(_)));
    assert_eq!(slot_ids(app, rack), ids_before);
    assert_eq!(scene_entries(app, rack), entries_before);
    assert!(app.purge_unused_rack_slots_recorded(rack).unwrap().len() == 1);
    assert!(app.unused_rack_slot_names(rack).is_empty());
}

/// A real saved instrument: the reload recompiles it by name.
#[test]
fn a_bank_swap_round_trips_through_a_project_file() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE, PIANO]);
    let (_, bank_b) = two_banks(app);
    share_one_pattern(app, rack);
    let name = "factory:Synths/Digi Drift";
    let outcome = app
        .swap_rack_slot_to_saved_instrument(rack, 1, name, SwapScope::Bank(Some(bank_b)))
        .unwrap();
    assert_eq!(outcome.kind, SwapKind::Inserted);
    let ids = slot_ids(app, rack);
    let sets = scene_sets(app, rack);
    let plays = every_scene_plays(app, rack);
    let saved = app.capture_project("bank-swap-roundtrip").unwrap();

    rig.reload("bank-swap-roundtrip", saved);
    let app = rig.app();
    assert_eq!(slot_ids(app, rack), ids);
    assert_eq!(scene_sets(app, rack), sets);
    assert_eq!(every_scene_plays(app, rack), plays);
}

#[test]
fn an_inserted_slot_plays_through_its_own_copies_of_the_old_slots_effects() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE, PIANO]);
    let (_, bank_b) = two_banks(app);
    let effect = app
        .add_builtin_rack_slot_effect_sync(rack, 1, "filter")
        .unwrap();
    let descriptor = app
        .rack_slot_effect_snapshot(rack, 1)
        .unwrap()
        .effect_descriptors[effect]
        .clone();
    let enabled = descriptor.enabled_param_idx().expect("filter has a bypass");
    let tone = (0..descriptor.params.len())
        .find(|idx| *idx != enabled)
        .unwrap();
    // Every pattern holds its own values: a tone, a p-lock, and in the
    // first pattern the effect bypassed.
    app.state.with_scenes_mut(|scenes| {
        for (index, patch) in scenes.track_pools[rack]
            .sounds
            .patches
            .values_mut()
            .enumerate()
        {
            let patch = Arc::make_mut(patch);
            let values = &mut patch.rack_track.as_mut().unwrap().slots[1].effect_slots[effect];
            values.defaults[tone] = 0.1 + 0.2 * index as f32;
            values.set_plock(3, tone, 0.05 + 0.1 * index as f32);
            if index == 0 {
                values.defaults[enabled] = 0.0;
            }
        }
    });
    let fx_values = |slot: &RackSlotSnapshot| {
        let values = &slot.effect_slots[effect];
        (
            values.defaults.clone(),
            (0..8)
                .map(|step| values.explicit_plock_value(step, tone))
                .collect::<Vec<_>>(),
        )
    };
    let old_node =
        app.state.live_rack_track_snapshot(rack).unwrap().slots[1].effect_slots[effect].node_id;
    let synth = fake_instrument(app, "fx-synth");

    let outcome = app
        .swap_rack_slot_recorded(rack, 1, synth, SwapScope::Bank(Some(bank_b)))
        .unwrap();

    assert_eq!(outcome.kind, SwapKind::Inserted);
    let live = app.state.live_rack_track_snapshot(rack).unwrap();
    let new_node = live.slots[2].effect_slots[effect].node_id;
    assert_ne!(new_node, 0, "the new slot runs the effect");
    assert_ne!(new_node, old_node, "its own instance, not a shared one");
    assert_eq!(
        live.slots[2].effect_descriptors[effect].name,
        descriptor.name
    );
    app.state.with_project_scenes(|scenes| {
        for patch in scenes.track_pools[rack].sounds.patches.values() {
            let rack = patch.rack_track.as_ref().unwrap();
            assert_eq!(
                fx_values(&rack.slots[2]),
                fx_values(&rack.slots[1]),
                "per pattern"
            );
            assert_eq!(rack.slots[2].effect_slots[effect].node_id, new_node);
        }
    });
    assert!(app.editor.effect_chain_leases.contains_host(
        crate::app::fx_chain::FxChainLocator::RackSlot {
            track: rack,
            slot: 2
        }
    ));
    let track_id = app.track_registry.id_at(rack).unwrap();
    let (old_slot, new_slot) = (
        app.device_registry.rack_slot(track_id, 1),
        app.device_registry.rack_slot(track_id, 2),
    );
    let old_fx = app.device_registry.rack_audio_effect_id(old_slot, effect);
    let new_fx = app.device_registry.rack_audio_effect_id(new_slot, effect);
    assert!(
        old_fx.is_some() && new_fx.is_some(),
        "both effects have identities"
    );
    assert_ne!(old_fx, new_fx, "the copy is a new device record");

    assert!(matches!(undo(app), HistoryReplay::Applied(_)));
    let live = app.state.live_rack_track_snapshot(rack).unwrap();
    assert_eq!(live.slots.len(), 2);
    assert_ne!(
        live.slots[1].effect_slots[effect].node_id, 0,
        "the old effect is back"
    );
}

#[test]
fn the_builtin_sampler_on_a_synth_track_two_banks_play_converts_it_for_the_bank() {
    let mut rig = Rig::new();
    let app = rig.app();
    let SlotDevice::Instrument(synth) = fake_instrument(app, "plain-synth") else {
        unreachable!()
    };
    let lib: *const lisp_host::LoadedDGenLib = &app.editor.instrument_libs[synth.lib_index];
    let track = unsafe {
        app.graph_controller().add_custom_track(
            &synth.name,
            synth.engine_id,
            &synth.manifest,
            &*lib,
            CustomInstrumentRunMode::Instrument,
        )
    }
    .unwrap();
    let (_, bank_b) = two_banks(app);
    let undo_before = app.history.undo_len();

    let outcome = app
        .swap_track_to_builtin_sampler_for_bank(track, Some(bank_b))
        .unwrap()
        .expect("another bank plays the track");

    assert!(outcome.converted);
    assert_eq!(app.history.undo_len(), undo_before + 1);
    let plays = every_scene_plays(app, track);
    let sampler = format!("Sampler {}", track + 1);
    assert_eq!(plays[0].len(), 1);
    assert_ne!(plays[0], vec![sampler.clone()], "bank A keeps the synth");
    assert_eq!(plays[1], plays[0]);
    assert_eq!(plays[2], vec![sampler.clone()]);
    assert_eq!(plays[3], vec![sampler]);
    assert!(matches!(undo(app), HistoryReplay::Applied(_)));
    assert_eq!(
        app.graph.track_instrument_types[track],
        InstrumentType::Custom
    );
}

#[test]
fn copied_convolution_reverb_and_filter_table_carry_their_loaded_data() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE, PIANO]);
    let (_, bank_b) = two_banks(app);
    let reverb = app
        .add_builtin_rack_slot_effect_sync(rack, 1, crate::effects::conv_reverb::NAME)
        .unwrap();
    let table = app
        .add_builtin_rack_slot_effect_sync(rack, 1, crate::effects::filter_table::NAME)
        .unwrap();
    let piano = std::fs::canonicalize(PIANO).unwrap();
    app.set_conv_reverb_ir_rack_slot(rack, 1, reverb, &piano, "yamaha-g5")
        .unwrap();
    app.set_filter_table_source_rack_slot(rack, 1, table, &piano, "yamaha-g5")
        .unwrap();
    let node = |app: &App, slot: usize, effect: usize| {
        app.state.live_rack_track_snapshot(rack).unwrap().slots[slot].effect_slots[effect].node_id
            as i32
    };
    let (old_reverb, old_table) = (node(app, 1, reverb), node(app, 1, table));
    let old_ir = crate::effects::conv_reverb::prepared_ir_for(old_reverb).expect("IR loaded");
    let old_mag =
        crate::effects::filter_table::prepared_table_for(old_table).expect("table loaded");
    let synth = fake_instrument(app, "reverb-synth");

    let outcome = app
        .swap_rack_slot_recorded(rack, 1, synth, SwapScope::Bank(Some(bank_b)))
        .unwrap();

    assert_eq!(outcome.kind, SwapKind::Inserted);
    let (new_reverb, new_table) = (node(app, 2, reverb), node(app, 2, table));
    assert!(new_reverb != 0 && new_reverb != old_reverb);
    assert!(new_table != 0 && new_table != old_table);
    let new_ir = crate::effects::conv_reverb::prepared_ir_for(new_reverb).expect("copied IR");
    assert!(
        Arc::ptr_eq(&new_ir, &old_ir),
        "the copy runs the loaded IR, not the default"
    );
    assert_eq!(
        crate::effects::conv_reverb::ir_ref_for(new_reverb).as_deref(),
        Some("yamaha-g5")
    );
    let new_mag =
        crate::effects::filter_table::prepared_table_for(new_table).expect("copied table");
    assert!(
        Arc::ptr_eq(&new_mag, &old_mag),
        "the copy runs the loaded table"
    );
}

#[test]
fn deleting_a_middle_slot_keeps_later_bindings_and_macros_on_their_device() {
    let mut rig = Rig::new();
    let app = rig.app();
    let rack = rack_with_samplers(app, &[PLATE, PIANO, PLATE]);
    let target = |slot: usize| crate::process::ParamTarget::RackSlotParam {
        slot,
        param: "gain".to_string(),
    };
    // A process drives slot 3's gain (binding and fan-out) and slot 2's;
    // a global macro drives slot 3's.
    let mut process = crate::process::TrackProcessSlot::new(
        crate::process::ProcessInstanceId(7),
        "lfo".to_string(),
    );
    process.bindings.insert("out".to_string(), Some(target(2)));
    process.bindings.insert("aux".to_string(), Some(target(1)));
    process.fanout.insert(
        "out".to_string(),
        vec![
            crate::process::ProcessPortFanout {
                target: target(2),
                lo: 0.0,
                hi: 1.0,
            },
            crate::process::ProcessPortFanout {
                target: target(1),
                lo: 0.0,
                hi: 1.0,
            },
        ],
    );
    app.state.pattern.process_chains.lock().unwrap()[rack]
        .slots
        .push(process.clone());
    app.state.with_scenes_mut(|scenes| {
        for patch in scenes.track_pools[rack]
            .sounds
            .patches
            .values_mut()
            .map(Arc::make_mut)
        {
            patch.process_chain.slots.push(process.clone());
        }
    });
    let global = engaged_rack_gain_macro(app, rack, 2);
    let kept_id = slot_ids(app, rack)[2];

    app.apply_recorded_instrument_binding_mutation(rack, "Delete rack layer", |app| {
        app.delete_rack_slot_with_identity(rack, 1)
    })
    .unwrap();

    assert_eq!(slot_ids(app, rack)[1], kept_id, "the old slot 3 is slot 2");
    let check = |chain: &crate::process::TrackProcessChain| {
        let slot = &chain.slots[0];
        assert_eq!(
            slot.bindings["out"],
            Some(target(1)),
            "the binding follows its device"
        );
        assert_eq!(
            slot.bindings["aux"], None,
            "a binding to the deleted slot falls back"
        );
        assert_eq!(
            slot.fanout["out"].len(),
            1,
            "the fan-out row to it is dropped"
        );
        assert_eq!(slot.fanout["out"][0].target, target(1));
    };
    check(&app.state.pattern.process_chains.lock().unwrap()[rack]);
    app.state.with_project_scenes(|scenes| {
        for patch in scenes.track_pools[rack].sounds.patches.values() {
            check(&patch.process_chain);
        }
    });
    assert_eq!(
        app.macro_engine.macro_definition(global).unwrap().mappings[0].target,
        target(1)
    );
    let overrides = app.macro_engine.overrides();
    assert!(
        overrides.contains_key(&rack_gain_key(rack, 1)),
        "the engaged macro follows"
    );
    assert!(!overrides.contains_key(&rack_gain_key(rack, 2)));
}
