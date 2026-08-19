    use super::{
        apply_bus_mixer_history_host_command, apply_drum_lane_history_host_command,
        apply_piano_roll_gesture_update,
        apply_piano_roll_history_host_command,
        apply_selected_steps_delete, apply_slice3_history_host_command,
        apply_toggle_step_host_command, bus_mixer_targeted_invalidation,
        slice3_track_mixer_invalidation, BusMixerInvalidation, TrackMixerInvalidation,
        build_custom_instrument_ui_source_with_overlay, claim_param_sync_revision,
        editor_surface_for_existing, effect_code_buffer_name, effect_patcher_buffer_source,
        escape_lisp_string, finish_piano_roll_gesture, instrument_code_buffer_name,
        instrument_patcher_buffer_source, EditorSurface,
        key_should_reveal_sequencer_track, patcher_layout_sidecar_path_for_dsp,
        pull_shared_bus_state, reconciled_track_index,
        restore_instrument_patcher_layout_source, should_clear_active_delete_target_for_buffer,
        show_instrument_patcher_layout_source, show_instrument_patcher_source_layout_source,
        track_meter_bindings_visible, ActiveDeleteTarget, ExpandedStepProjectionRegistry,
        FxDeleteChain, ParamSyncRevision, Runtime, StepParam, Value, AGENT_INSTRUMENT_STUB_UI,
        NEW_INSTRUMENT_STARTER_DSP,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use eseqlisp::parser::{ASTParser, Parser};
    use std::path::Path;

    fn history_test_app() -> (
        std::sync::Arc<sequencer::sequencer::SequencerState>,
        sequencer::app::App,
    ) {
        let state = std::sync::Arc::new(sequencer::sequencer::SequencerState::new(
            1,
            vec![sequencer::sequencer::default_empty_effect_chain()],
        ));
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = sequencer::app::App::new(
            state.clone(),
            sequencer::audiograph::LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            sequencer::app::AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: std::sync::Arc::new(std::sync::Mutex::new(std::sync::Arc::new(Vec::new()))),
                bus_gate_playheads: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            std::sync::Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry =
            sequencer::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        (state, app)
    }

    fn history_value_map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| {
                    (
                        key.to_string(),
                        std::rc::Rc::new(std::cell::RefCell::new(value)),
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn shared_bus_pull_copies_only_mixer_scalars_when_topology_is_unchanged() {
        let (_state, mut app) = history_test_app();
        app.buses = sequencer::app::BusChannelState::default_buses();
        app.buses[0].name = "app-owned".to_string();
        let mut shared = app.buses.clone();
        shared[0].name = "shared-owned".to_string();
        shared[0].volume = 0.25;
        shared[0].mute = true;
        shared[0].solo = true;
        let shared = std::sync::Arc::new(std::sync::Mutex::new(shared));

        assert!(pull_shared_bus_state(&mut app, &shared));
        assert_eq!(app.buses[0].name, "app-owned");
        assert_eq!(app.buses[0].volume, 0.25);
        assert!(app.buses[0].mute);
        assert!(app.buses[0].solo);
        assert!(!pull_shared_bus_state(&mut app, &shared));
    }

    #[test]
    fn shared_bus_pull_replaces_full_state_when_topology_changes() {
        let (_state, mut app) = history_test_app();
        app.buses = sequencer::app::BusChannelState::default_buses();
        let mut shared = app.buses.clone();
        shared.push(sequencer::app::BusChannelState::new(
            sequencer::sequencer::BusId(99),
            "New bus",
        ));
        let shared = std::sync::Arc::new(std::sync::Mutex::new(shared));

        assert!(pull_shared_bus_state(&mut app, &shared));
        assert_eq!(app.buses.len(), 4);
        assert_eq!(app.buses[3].name, "New bus");
    }

    #[test]
    fn param_sync_revision_claims_only_changed_composite_inputs() {
        let revision = ParamSyncRevision {
            track: 1,
            scene: 2,
            pattern_epoch: 3,
            song_row_mirror_epoch: 4,
            ui_epoch: 5,
            fx_epoch: 6,
            sound_binding_epoch: 7,
            display_step: Some(8),
            selected_steps: vec![8, 9],
            selected_neural_neurons: Vec::new(),
        };
        let mut previous = None;

        assert!(claim_param_sync_revision(&mut previous, &revision));
        assert!(!claim_param_sync_revision(&mut previous, &revision));

        let mut changed = revision;
        changed.pattern_epoch += 1;
        assert!(claim_param_sync_revision(&mut previous, &changed));
    }

    /// Clip-edit-target spec 3.4 end to end through the host-command seam: a
    /// pinned NON-effective pattern gets pool writes — the live mirror stays
    /// untouched — and undo restores the pool copy.
    #[test]
    fn metal_piano_roll_edit_against_a_pinned_clip_writes_the_pool() {
        let (state, mut app) = history_test_app();
        // Two scenes: scene 0's pattern is effective, scene 1's pattern sits
        // in the pool un-effective — the pinnable "other clip" material.
        state.replace_pattern_repository(
            vec![
                sequencer::sequencer::PatternSnapshot::new_default(1, &[]),
                sequencer::sequencer::PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state
            .restore_current_pattern_from_repository()
            .expect("scene restores");
        assert!(state.save_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[sequencer::sequencer::InstrumentType::Sampler],
        ));
        let (scene_pattern, other) = state.with_project_scenes(|scenes| {
            (
                scenes.scenes[0].cells[0].expect("scene 0 pattern"),
                scenes.scenes[1].cells[0].expect("scene 1 pattern"),
            )
        });
        assert_ne!(scene_pattern, other);
        let mut arrangement = sequencer::sequencer::ProjectArrangement::new(1, 16.0);
        arrangement.track_lanes[0].push(sequencer::sequencer::ArrClip::new(
            sequencer::sequencer::ClipId(0),
            0.0,
            16.0,
            Some(other.0),
        ));
        arrangement.next_clip_id = 1;
        state
            .set_committed_arrangement(Some(arrangement))
            .expect("arrangement installs");
        app.set_arrangement_view_visible(true);
        app.select_song_clip(0, sequencer::sequencer::ClipId(0))
            .expect("clip selects");
        assert!(matches!(
            app.track_edit_focus(0),
            sequencer::app::focus::EditFocus::Pattern { .. }
        ));

        let selection = std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashSet::new(),
        ));
        let move_state = std::sync::Arc::new(std::sync::Mutex::new(None));
        let clipboard = super::new_piano_roll_clipboard();
        let create_action = history_value_map([
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("start", Value::Number(2.0)),
            ("end", Value::Number(3.0)),
            ("lane", Value::Number(48.0)),
        ]);
        let payload = history_value_map([
            ("track", Value::Number(0.0)),
            ("action", create_action),
        ]);
        let (outcome, _, _) = apply_piano_roll_history_host_command(
            &mut app,
            &selection,
            &move_state,
            &clipboard,
            &payload,
        )
        .expect("create note against the pinned clip");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));

        let pool_active = |pattern| {
            state
                .with_pool_pattern(0, pattern, |data| {
                    data.track_bits[0] >> 2 & 1 == 1
                })
                .expect("pattern in pool")
        };
        assert!(pool_active(other), "the pinned pool pattern got the note");
        assert!(
            !state.pattern.patterns[0].is_active(2),
            "the live mirror is untouched by a pool-target write"
        );
        assert!(
            !pool_active(scene_pattern),
            "the scene pattern is not dual-written"
        );

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!pool_active(other), "undo restores the pool copy");
    }

    #[test]
    fn slice3_mixer_drag_ops_route_to_targeted_track_mixer_invalidation() {
        let volume = history_value_map([("op", Value::Keyword("volume".to_string()))]);
        assert_eq!(
            slice3_track_mixer_invalidation(&volume),
            Some(TrackMixerInvalidation::Volume)
        );
        let pan = history_value_map([("op", Value::Keyword("pan".to_string()))]);
        assert_eq!(
            slice3_track_mixer_invalidation(&pan),
            Some(TrackMixerInvalidation::Pan)
        );
        let mute_op = history_value_map([("op", Value::Keyword("toggle-mute".to_string()))]);
        assert_eq!(
            slice3_track_mixer_invalidation(&mute_op),
            Some(TrackMixerInvalidation::Mute)
        );
        let solo_op = history_value_map([("op", Value::Keyword("toggle-solo".to_string()))]);
        assert_eq!(
            slice3_track_mixer_invalidation(&solo_op),
            Some(TrackMixerInvalidation::Solo)
        );
        // Non-mixer ops keep the whole-track + ui-epoch resync path.
        let attack = history_value_map([("op", Value::Keyword("attack".to_string()))]);
        assert_eq!(slice3_track_mixer_invalidation(&attack), None);
        assert_eq!(slice3_track_mixer_invalidation(&Value::Nil), None);

        assert_eq!(
            bus_mixer_targeted_invalidation(&volume),
            Some(BusMixerInvalidation::Volume)
        );
        let mute = history_value_map([("op", Value::Keyword("toggle-mute".to_string()))]);
        assert_eq!(bus_mixer_targeted_invalidation(&mute), None);
    }

    #[test]
    fn slice3_host_action_enters_track_parameter_history() {
        let (state, mut app) = history_test_app();
        let before = state.pattern.track_params[0].get_volume();
        let payload = history_value_map([
            ("op", Value::Keyword("volume".to_string())),
            ("track", Value::Number(0.0)),
            ("value", Value::Number(0.25)),
        ]);

        let (outcome, track) = apply_slice3_history_host_command(&mut app, &payload)
            .expect("apply Slice 3 host action");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));
        assert_eq!(track, Some(0));
        sequencer::app::edit::finish_active_gesture(&mut app);
        assert_eq!(state.pattern.track_params[0].get_volume().to_bits(), 0.25f32.to_bits());
        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(state.pattern.track_params[0].get_volume().to_bits(), before.to_bits());
    }

    #[test]
    fn bus_mixer_host_action_enters_replayable_history() {
        let (_state, mut app) = history_test_app();
        let before = app.buses[1].volume;
        let payload = history_value_map([
            ("op", Value::Keyword("volume".to_string())),
            ("bus", Value::Number(1.0)),
            ("bus-id", Value::String(app.buses[1].id.0.to_string())),
            ("value", Value::Number(0.25)),
        ]);

        let (outcome, bus) = apply_bus_mixer_history_host_command(&mut app, &payload)
            .expect("apply bus mixer host action");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));
        assert_eq!(bus, 1);
        sequencer::app::edit::finish_active_gesture(&mut app);
        assert_eq!(app.buses[1].volume.to_bits(), 0.25f32.to_bits());
        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.buses[1].volume.to_bits(), before.to_bits());
    }

    #[test]
    fn metal_step_toggle_host_command_creates_replayable_history() {
        let (state, mut app) = history_test_app();
        let payload = Value::Map(
            [
                ("track", Value::Number(0.0)),
                ("step", Value::Number(6.0)),
            ]
            .into_iter()
            .map(|(key, value)| {
                (
                    key.to_string(),
                    std::rc::Rc::new(std::cell::RefCell::new(value)),
                )
            })
            .collect(),
        );

        let (outcome, track, step) =
            apply_toggle_step_host_command(&mut app, &payload).expect("toggle through host seam");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));
        assert_eq!((track, step), (0, 6));
        assert!(state.pattern.patterns[0].is_active(6));
        assert_eq!(app.history.undo_len(), 1);

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!state.pattern.patterns[0].is_active(6));
    }

    #[test]
    fn metal_selected_step_delete_is_one_replayable_history_entry() {
        let (state, mut app) = history_test_app();
        for step in [2, 5] {
            state.pattern.patterns[0].set_step_active(step, true);
            state.pattern.step_data[0].set(
                step,
                sequencer::sequencer::StepParam::Velocity,
                step as f32 / 10.0,
            );
        }
        let selected = std::sync::Arc::new(std::sync::Mutex::new(
            [2_usize, 5].into_iter().collect::<std::collections::HashSet<_>>(),
        ));

        let (outcome, steps) =
            apply_selected_steps_delete(&mut app, 0, &selected).expect("delete selected steps");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));
        assert_eq!(steps, vec![2, 5]);
        assert!(selected.lock().unwrap().is_empty());
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(!state.pattern.patterns[0].is_active(5));
        assert_eq!(app.history.undo_len(), 1);

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(state.pattern.patterns[0].is_active(2));
        assert!(state.pattern.patterns[0].is_active(5));
        assert_eq!(state.pattern.step_data[0].get(
            5,
            sequencer::sequencer::StepParam::Velocity,
        ), 0.5);
        assert!(matches!(
            sequencer::app::edit::redo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(!state.pattern.patterns[0].is_active(5));
    }

    #[test]
    fn metal_drum_lane_edits_are_individually_replayable() {
        let (state, mut app) = history_test_app();

        let toggle = history_value_map([
            ("op", Value::Keyword("toggle".to_string())),
            ("track", Value::Number(0.0)),
            ("pad-note", Value::Number(36.0)),
            ("step", Value::Number(2.0)),
        ]);
        apply_drum_lane_history_host_command(&mut app, &toggle)
            .expect("toggle drum-lane step");
        assert!(state.pattern.patterns[0].is_active(2));

        let duration = history_value_map([
            ("op", Value::Keyword("duration".to_string())),
            ("track", Value::Number(0.0)),
            ("pad-note", Value::Number(36.0)),
            ("step", Value::Number(2.0)),
            ("duration", Value::Number(3.0)),
        ]);
        apply_drum_lane_history_host_command(&mut app, &duration)
            .expect("set drum-lane duration");
        assert_eq!(state.drum_lane_step_duration(0, 2, 36), Some(3.0));

        let move_action = history_value_map([
            ("op", Value::Keyword("move".to_string())),
            ("track", Value::Number(0.0)),
            ("pad-note", Value::Number(36.0)),
            ("steps", piano_roll_id_list([2])),
            ("delta", Value::Number(3.0)),
            ("move-selection", Value::Bool(false)),
        ]);
        apply_drum_lane_history_host_command(&mut app, &move_action)
            .expect("move drum-lane step");
        assert!(!state.pattern.patterns[0].is_active(2));
        assert_eq!(state.drum_lane_step_duration(0, 5, 36), Some(3.0));

        let clear = history_value_map([
            ("op", Value::Keyword("clear".to_string())),
            ("track", Value::Number(0.0)),
            ("pad-note", Value::Number(36.0)),
            ("steps", piano_roll_id_list([5])),
        ]);
        apply_drum_lane_history_host_command(&mut app, &clear)
            .expect("clear drum-lane step");
        assert!(!state.pattern.patterns[0].is_active(5));
        assert_eq!(app.history.undo_len(), 4);

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(state.drum_lane_step_duration(0, 5, 36), Some(3.0));
        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(state.drum_lane_step_duration(0, 2, 36), Some(3.0));
        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert_ne!(state.drum_lane_step_duration(0, 2, 36), Some(3.0));
        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!state.pattern.patterns[0].is_active(2));

        for _ in 0..4 {
            assert!(matches!(
                sequencer::app::edit::redo(&mut app),
                sequencer::app::history::HistoryReplay::Applied(_)
            ));
        }
        assert!(!state.pattern.patterns[0].is_active(5));
    }

    #[test]
    fn metal_piano_roll_create_and_delete_are_individually_replayable() {
        let (state, mut app) = history_test_app();
        let selection = std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashSet::new(),
        ));
        let move_state = std::sync::Arc::new(std::sync::Mutex::new(None));
        let clipboard = super::new_piano_roll_clipboard();
        let create_action = history_value_map([
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("start", Value::Number(2.0)),
            ("end", Value::Number(3.0)),
            ("lane", Value::Number(48.0)),
        ]);
        let create_payload = history_value_map([
            ("track", Value::Number(0.0)),
            ("action", create_action),
        ]);

        let (outcome, _, _) = apply_piano_roll_history_host_command(
            &mut app,
            &selection,
            &move_state,
            &clipboard,
            &create_payload,
        )
        .expect("create piano-roll note through history seam");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));
        assert!(state.pattern.patterns[0].is_active(2));
        assert_eq!(app.history.undo_len(), 1);

        let delete_action = history_value_map([
            ("type", Value::Keyword("delete-items".to_string())),
            (
                "ids",
                Value::List(vec![std::rc::Rc::new(std::cell::RefCell::new(
                    Value::Number(super::piano_roll_item_id(2, 0) as f64),
                ))]),
            ),
        ]);
        let delete_payload = history_value_map([
            ("track", Value::Number(0.0)),
            ("action", delete_action),
        ]);
        let (outcome, _, _) = apply_piano_roll_history_host_command(
            &mut app,
            &selection,
            &move_state,
            &clipboard,
            &delete_payload,
        )
        .expect("delete piano-roll note through history seam");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));
        assert!(!state.pattern.patterns[0].is_active(2));
        assert_eq!(app.history.undo_len(), 2);

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(state.pattern.patterns[0].is_active(2));
        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(matches!(
            sequencer::app::edit::redo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(state.pattern.patterns[0].is_active(2));
        assert!(matches!(
            sequencer::app::edit::redo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!state.pattern.patterns[0].is_active(2));
    }

    fn piano_roll_gesture_payload(action: Value) -> Value {
        history_value_map([("track", Value::Number(0.0)), ("action", action)])
    }

    fn piano_roll_id_list(ids: impl IntoIterator<Item = u64>) -> Value {
        Value::List(
            ids.into_iter()
                .map(|id| {
                    std::rc::Rc::new(std::cell::RefCell::new(Value::Number(id as f64)))
                })
                .collect(),
        )
    }

    #[test]
    fn metal_piano_roll_move_drag_coalesces_preview_updates_into_one_entry() {
        let (state, mut app) = history_test_app();
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(
            2,
            sequencer::sequencer::StepParam::Duration,
            1.0,
        );
        let id = super::piano_roll_item_id(2, 0);
        let selection = std::sync::Arc::new(std::sync::Mutex::new([id].into_iter().collect()));
        let move_state = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut active = None;
        let version_before = state.scheduler_snapshot_version();

        for (destination, lane) in [(4.0, 48.0), (6.0, 41.0)] {
            let action = history_value_map([
                ("type", Value::Keyword("move-items-absolute".to_string())),
                ("ids", piano_roll_id_list([id])),
                ("anchor-id", Value::Number(id as f64)),
                ("start", Value::Number(destination)),
                ("lane", Value::Number(lane)),
            ]);
            apply_piano_roll_gesture_update(
                &mut app,
                &selection,
                &move_state,
                &mut active,
                &piano_roll_gesture_payload(action),
            )
            .expect("preview piano-roll move");
            assert_eq!(app.history.undo_len(), 0);
        }
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(!state.pattern.patterns[0].is_active(4));
        assert!(state.pattern.patterns[0].is_active(6));
        assert_eq!(state.pattern.chord_data[0].get(6, 0), 7.0);
        assert_eq!(state.scheduler_snapshot_version(), version_before + 2);

        let finish = history_value_map([
            ("type", Value::Keyword("finish-move-items".to_string())),
            ("ids", piano_roll_id_list([id])),
            ("anchor-id", Value::Number(id as f64)),
        ]);
        let (outcome, _) = finish_piano_roll_gesture(
            &mut app,
            &move_state,
            &mut active,
            &piano_roll_gesture_payload(finish),
        )
        .expect("finish piano-roll move");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));
        assert_eq!(app.history.undo_len(), 1);
        assert_eq!(state.scheduler_snapshot_version(), version_before + 2);

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(state.pattern.patterns[0].is_active(2));
        assert!(!state.pattern.patterns[0].is_active(6));
        assert_eq!(
            state.pattern.step_data[0].get(2, sequencer::sequencer::StepParam::Transpose),
            0.0,
        );
        assert_eq!(state.scheduler_snapshot_version(), version_before + 3);
        assert!(matches!(
            sequencer::app::edit::redo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(state.pattern.patterns[0].is_active(6));
        assert_eq!(state.pattern.chord_data[0].get(6, 0), 7.0);
    }

    #[test]
    fn metal_piano_roll_multi_note_move_round_trips_every_touched_cell() {
        let (state, mut app) = history_test_app();
        for (step, transpose) in [(2, 0.0), (4, 7.0)] {
            state.pattern.patterns[0].set_step_active(step, true);
            state.pattern.step_data[0].set(
                step,
                sequencer::sequencer::StepParam::Transpose,
                transpose,
            );
            state.pattern.step_data[0].set(
                step,
                sequencer::sequencer::StepParam::Duration,
                1.0,
            );
        }
        let first = super::piano_roll_item_id(2, 0);
        let second = super::piano_roll_item_id(4, 0);
        let selection = std::sync::Arc::new(std::sync::Mutex::new(
            [first, second].into_iter().collect(),
        ));
        let move_state = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut active = None;
        let move_action = history_value_map([
            ("type", Value::Keyword("move-items-absolute".to_string())),
            ("ids", piano_roll_id_list([first, second])),
            ("anchor-id", Value::Number(first as f64)),
            ("start", Value::Number(6.0)),
            ("lane", Value::Number(43.0)),
        ]);
        apply_piano_roll_gesture_update(
            &mut app,
            &selection,
            &move_state,
            &mut active,
            &piano_roll_gesture_payload(move_action),
        )
        .expect("preview multi-note move");
        let finish = history_value_map([
            ("type", Value::Keyword("finish-move-items".to_string())),
            ("ids", piano_roll_id_list([first, second])),
            ("anchor-id", Value::Number(first as f64)),
        ]);
        finish_piano_roll_gesture(
            &mut app,
            &move_state,
            &mut active,
            &piano_roll_gesture_payload(finish),
        )
        .expect("finish multi-note move");

        assert_eq!(app.history.undo_len(), 1);
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(!state.pattern.patterns[0].is_active(4));
        assert_eq!(state.pattern.chord_data[0].get(6, 0), 5.0);
        assert_eq!(state.pattern.chord_data[0].get(8, 0), 12.0);

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            state.pattern.step_data[0].get(2, sequencer::sequencer::StepParam::Transpose),
            0.0,
        );
        assert_eq!(
            state.pattern.step_data[0].get(4, sequencer::sequencer::StepParam::Transpose),
            7.0,
        );
        assert!(!state.pattern.patterns[0].is_active(6));
        assert!(!state.pattern.patterns[0].is_active(8));

        assert!(matches!(
            sequencer::app::edit::redo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(state.pattern.chord_data[0].get(6, 0), 5.0);
        assert_eq!(state.pattern.chord_data[0].get(8, 0), 12.0);
    }

    #[test]
    fn metal_piano_roll_nudge_time_and_note_is_replayable() {
        let (state, mut app) = history_test_app();
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(
            2,
            sequencer::sequencer::StepParam::Duration,
            1.0,
        );
        let id = super::piano_roll_item_id(2, 0);
        let selection = std::sync::Arc::new(std::sync::Mutex::new([id].into_iter().collect()));
        let move_state = std::sync::Arc::new(std::sync::Mutex::new(None));
        let clipboard = super::new_piano_roll_clipboard();
        let action = history_value_map([
            ("type", Value::Keyword("nudge-selection".to_string())),
            ("ids", piano_roll_id_list([id])),
            ("delta-time", Value::Number(3.0)),
            ("delta-lane", Value::Number(-5.0)),
        ]);

        let (outcome, _, _) = apply_piano_roll_history_host_command(
            &mut app,
            &selection,
            &move_state,
            &clipboard,
            &piano_roll_gesture_payload(action),
        )
        .expect("nudge piano-roll note through history seam");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));
        assert_eq!(app.history.undo_len(), 1);
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(state.pattern.patterns[0].is_active(5));
        assert_eq!(state.pattern.chord_data[0].get(5, 0), 5.0);

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(state.pattern.patterns[0].is_active(2));
        assert!(!state.pattern.patterns[0].is_active(5));
        assert_eq!(
            state.pattern.step_data[0].get(2, sequencer::sequencer::StepParam::Transpose),
            0.0,
        );

        assert!(matches!(
            sequencer::app::edit::redo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(state.pattern.patterns[0].is_active(5));
        assert_eq!(state.pattern.chord_data[0].get(5, 0), 5.0);
    }

    #[test]
    fn metal_piano_roll_paste_is_replayable_with_shared_clipboard() {
        let (state, mut app) = history_test_app();
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(
            2,
            sequencer::sequencer::StepParam::Transpose,
            4.0,
        );
        state.pattern.step_data[0].set(
            2,
            sequencer::sequencer::StepParam::Duration,
            1.5,
        );
        let id = super::piano_roll_item_id(2, 0);
        let selection = std::sync::Arc::new(std::sync::Mutex::new([id].into_iter().collect()));
        let move_state = std::sync::Arc::new(std::sync::Mutex::new(None));
        let clipboard = super::new_piano_roll_clipboard();
        let copy_action = history_value_map([
            ("type", Value::Keyword("copy-items".to_string())),
            ("ids", piano_roll_id_list([id])),
        ]);
        super::apply_piano_roll_action_with_clipboard(
            &super::PianoRollLanes::live(&state, 0),
            &selection,
            &move_state,
            &clipboard,
            &copy_action,
        )
        .expect("copy piano-roll note");

        let paste_action = history_value_map([
            ("type", Value::Keyword("paste-items".to_string())),
            ("time", Value::Number(6.0)),
        ]);
        let (outcome, _, _) = apply_piano_roll_history_host_command(
            &mut app,
            &selection,
            &move_state,
            &clipboard,
            &piano_roll_gesture_payload(paste_action),
        )
        .expect("paste piano-roll note through history seam");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));
        assert_eq!(app.history.undo_len(), 1);
        assert!(state.pattern.patterns[0].is_active(2));
        assert!(state.pattern.patterns[0].is_active(6));
        assert_eq!(state.pattern.chord_data[0].get(6, 0), 4.0);
        assert_eq!(state.pattern.chord_data[0].get_duration(6, 0), 1.5);

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(state.pattern.patterns[0].is_active(2));
        assert!(!state.pattern.patterns[0].is_active(6));

        assert!(matches!(
            sequencer::app::edit::redo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!(state.pattern.patterns[0].is_active(6));
        assert_eq!(state.pattern.chord_data[0].get(6, 0), 4.0);
    }

    #[test]
    fn metal_piano_roll_resize_drag_coalesces_preview_updates_into_one_entry() {
        let (state, mut app) = history_test_app();
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(
            2,
            sequencer::sequencer::StepParam::Duration,
            1.0,
        );
        let id = super::piano_roll_item_id(2, 0);
        let selection = std::sync::Arc::new(std::sync::Mutex::new([id].into_iter().collect()));
        let move_state = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut active = None;
        let version_before = state.scheduler_snapshot_version();

        for time in [4.0, 5.0] {
            let action = history_value_map([
                ("type", Value::Keyword("resize-item-absolute".to_string())),
                ("id", Value::Number(id as f64)),
                ("ids", piano_roll_id_list([id])),
                ("edge", Value::Keyword("end".to_string())),
                ("time", Value::Number(time)),
            ]);
            apply_piano_roll_gesture_update(
                &mut app,
                &selection,
                &move_state,
                &mut active,
                &piano_roll_gesture_payload(action),
            )
            .expect("preview piano-roll resize");
            assert_eq!(app.history.undo_len(), 0);
        }
        assert_eq!(
            state.pattern.step_data[0].get(2, sequencer::sequencer::StepParam::Duration),
            3.0,
        );
        assert_eq!(state.scheduler_snapshot_version(), version_before + 2);

        let finish = history_value_map([
            ("type", Value::Keyword("finish-resize-items".to_string())),
            ("ids", piano_roll_id_list([id])),
            ("id", Value::Number(id as f64)),
        ]);
        let (outcome, _) = finish_piano_roll_gesture(
            &mut app,
            &move_state,
            &mut active,
            &piano_roll_gesture_payload(finish),
        )
        .expect("finish piano-roll resize");
        assert!(matches!(outcome, sequencer::app::edit::EditOutcome::Applied(_)));
        assert_eq!(app.history.undo_len(), 1);
        assert_eq!(state.scheduler_snapshot_version(), version_before + 2);

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            state.pattern.step_data[0].get(2, sequencer::sequencer::StepParam::Duration),
            1.0,
        );
        assert!(matches!(
            sequencer::app::edit::redo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            state.pattern.step_data[0].get(2, sequencer::sequencer::StepParam::Duration),
            3.0,
        );
    }

    #[test]
    fn active_delete_target_buffer_switch_preserves_target_claimed_in_new_buffer() {
        let mixer_target = ActiveDeleteTarget::MixerTrack { track: 0 };
        assert!(
            !should_clear_active_delete_target_for_buffer(Some(&mixer_target), "*mixer*"),
            "clicking a mixer delete target in an inactive mixer tile should survive the tile activation"
        );
        assert!(
            should_clear_active_delete_target_for_buffer(Some(&mixer_target), "*fx*"),
            "leaving mixer for another buffer should clear a mixer delete target"
        );

        let fx_target = ActiveDeleteTarget::FxEffect {
            chain: FxDeleteChain::Audio,
            bus: None,
            slot: 2,
        };
        assert!(
            !should_clear_active_delete_target_for_buffer(Some(&fx_target), "*fx*"),
            "clicking an FX delete target in an inactive FX tile should survive the tile activation"
        );
        assert!(
            should_clear_active_delete_target_for_buffer(Some(&fx_target), "*mixer*"),
            "leaving FX for mixer should clear an FX delete target"
        );
    }

    #[test]
    fn reconciles_stale_current_track_against_track_count() {
        assert_eq!(reconciled_track_index(2, 0, 4), Some(2));
        assert_eq!(reconciled_track_index(7, 1, 4), Some(1));
        assert_eq!(reconciled_track_index(7, 9, 4), Some(3));
        assert_eq!(reconciled_track_index(0, 0, 0), None);
    }

    #[test]
    fn step_selection_sync_updates_selected_steps_without_deadlocking() {
        let state = std::sync::Arc::new(sequencer::sequencer::SequencerState::new(1, Vec::new()));
        state.pattern.track_params[0].set_num_steps(8);
        state
            .pattern
            .step_data[0]
            .set(3, sequencer::sequencer::StepParam::Velocity, 0.66);
        state
            .pattern
            .step_data[0]
            .set(3, sequencer::sequencer::StepParam::Duration, 2.5);
        state
            .pattern
            .step_data[0]
            .set(3, sequencer::sequencer::StepParam::Transpose, 7.0);
        state
            .pattern
            .step_data[0]
            .set(2, sequencer::sequencer::StepParam::Velocity, 0.72);
        state
            .pattern
            .step_data[0]
            .set(2, sequencer::sequencer::StepParam::Duration, 1.5);
        state
            .pattern
            .step_data[0]
            .set(2, sequencer::sequencer::StepParam::Transpose, -4.0);
        let selected_steps = std::sync::Arc::new(std::sync::Mutex::new(
            [2_usize, 3, 4]
                .into_iter()
                .collect::<std::collections::HashSet<_>>(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![(
                "selected-steps",
                Value::List(
                    (0..sequencer::sequencer::MAX_STEPS)
                        .map(|_| std::rc::Rc::new(std::cell::RefCell::new(Value::Bool(false))))
                        .collect(),
                ),
            )],
            true,
        );
        runtime
            .eval_str("(def cursor-step 3)")
            .expect("register cursor step");

        let expanded_step_projection = std::sync::Arc::new(ExpandedStepProjectionRegistry::new());
        super::sync_step_selection_bindings(
            &mut runtime,
            &state,
            None,
            0,
            &selected_steps,
            0,
            &expanded_step_projection,
            &(0..sequencer::sequencer::MAX_STEPS).collect::<Vec<_>>(),
            true,
        );

        assert_eq!(
            runtime
                .eval_str("(nth SEQ.selected-steps 1)")
                .expect("read unselected step"),
            Some(Value::Bool(false))
        );
        assert_eq!(
            runtime
                .eval_str("(nth SEQ.selected-steps 3)")
                .expect("read selected step"),
            Some(Value::Bool(true))
        );
        for (field, expected) in [
            ("fx-step-cursor-number", 4.0),
            ("fx-step-selection-count", 3.0),
            ("fx-step-value-velocity", 0.72),
            ("fx-step-value-duration", 1.5),
            ("fx-step-value-transpose", -4.0),
        ] {
            let value = runtime
                .eval_str(&format!("SEQ.{field}"))
                .unwrap_or_else(|error| panic!("read {field}: {error:?}"));
            let Some(Value::Number(value)) = value else {
                panic!("{field} should be numeric, got {value:?}");
            };
            assert!(
                (value - expected).abs() < 0.0001,
                "{field} expected {expected}, got {value}"
            );
        }

        selected_steps.lock().unwrap().remove(&3);
        super::sync_step_selection_bindings(
            &mut runtime,
            &state,
            None,
            0,
            &selected_steps,
            0,
            &expanded_step_projection,
            &[3],
            true,
        );
        assert_eq!(
            runtime
                .eval_str("(nth SEQ.selected-steps 3)")
                .expect("read cleared step"),
            Some(Value::Bool(false))
        );
        assert_eq!(
            runtime
                .eval_str("(nth SEQ.selected-steps 4)")
                .expect("read unchanged selected step"),
            Some(Value::Bool(true)),
            "delta sync must preserve selection indexes outside changed_steps"
        );
    }

    #[test]
    fn single_step_param_sync_updates_selected_panel_scalar_binding() {
        let state = std::sync::Arc::new(sequencer::sequencer::SequencerState::new(1, Vec::new()));
        state.pattern.track_params[0].set_num_steps(8);
        state
            .pattern
            .step_data[0]
            .set(4, sequencer::sequencer::StepParam::Velocity, 0.72);
        let selected_steps = std::sync::Arc::new(std::sync::Mutex::new(
            [4, 5, 6, 7].into_iter().collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![(
                "velocities",
                Value::List(
                    (0..sequencer::sequencer::MAX_STEPS)
                        .map(|_| std::rc::Rc::new(std::cell::RefCell::new(Value::Number(0.0))))
                        .collect(),
                ),
            )],
            true,
        );
        runtime
            .eval_str("(def cursor-step 0)")
            .expect("register cursor step");

        super::sync_single_step_param_binding(
            &mut runtime,
            &state,
            0,
            4,
            sequencer::sequencer::StepParam::Velocity,
            0,
            &selected_steps,
            &std::sync::Arc::new(ExpandedStepProjectionRegistry::new()),
        );

        let value = runtime
            .eval_str("SEQ.fx-step-value-velocity")
            .expect("read cursor velocity binding");
        let Some(Value::Number(value)) = value else {
            panic!("selected velocity should be numeric, got {value:?}");
        };
        assert!((value - 0.72).abs() < 0.0001);
    }

    #[test]
    fn instrument_plock_presence_sync_updates_step_markers() {
        let state = std::sync::Arc::new(sequencer::sequencer::SequencerState::new(1, vec![vec![]]));
        state.pattern.track_params[0].set_num_steps(8);
        let desc = sequencer::effects::EffectDescriptor::builtin_sampler();
        state.pattern.instrument_slots[0].apply_descriptor(&desc, 17);
        state.pattern.instrument_slots[0].set_plock(2, 8, 22_050.0);
        let effect_descriptors = vec![Vec::new()];
        let mut runtime = Runtime::new();

        let selected_steps =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::from([
                2, 3,
            ])));
        super::sync_instrument_plock_presence_fields(
            &mut runtime,
            &state,
            &effect_descriptors,
            0,
            &selected_steps,
        );

        assert_eq!(
            runtime
                .eval_str("(nth SEQ.step-has-plocks 2)")
                .expect("read p-locked step"),
            Some(Value::Bool(true))
        );
        assert_eq!(
            runtime
                .eval_str("(nth SEQ.step-has-plocks 3)")
                .expect("read unp-locked step"),
            Some(Value::Bool(false))
        );
        assert_eq!(
            runtime
                .eval_str(&format!(
                    r#"(reactive-get "SEQ" "{}")"#,
                    super::track_step_plocked_field(0, 2)
                ))
                .expect("read selected p-locked step field"),
            Some(Value::Bool(true))
        );
        assert_eq!(
            runtime
                .eval_str(&format!(
                    r#"(reactive-get "SEQ" "{}")"#,
                    super::track_step_plocked_field(0, 3)
                ))
                .expect("read selected unp-locked step field"),
            Some(Value::Bool(false))
        );
    }

    #[test]
    fn selecting_a_lock_free_step_still_publishes_the_track_variant_chips() {
        let (state, app) = history_test_app();
        state.pattern.track_params[0].set_num_steps(8);
        let desc = sequencer::effects::EffectDescriptor::builtin_sampler();
        state.pattern.instrument_slots[0].apply_descriptor(&desc, 17);
        state.pattern.instrument_slots[0].set_plock(2, 8, 22_050.0);
        state.reconcile_plock_variant_registry_for_track(0);

        // Step 5 has no locks of its own, but the track has a variant on step 2:
        // the chip strip is how the user stamps that variant onto step 5.
        let selected_steps =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::from([5])));
        let mut runtime = Runtime::new();
        super::sync_track_plocks_for_neural_selection(
            &mut runtime,
            &app,
            &state,
            0,
            &selected_steps,
            &std::collections::BTreeSet::new(),
        );

        assert_eq!(
            runtime
                .eval_str("(len SEQ.track-plocks)")
                .expect("read p-lock rows"),
            Some(Value::Number(0.0)),
            "a lock-free step has no p-lock rows to show"
        );
        assert_eq!(
            runtime
                .eval_str(r#"(get (nth SEQ.track-plock-variants 0) :kind)"#)
                .expect("read default chip"),
            Some(Value::String("def".to_string())),
            "the default chip must stay available on a lock-free step"
        );
        assert_eq!(
            runtime
                .eval_str(r#"(get (nth SEQ.track-plock-variants 1) :kind)"#)
                .expect("read variant chip"),
            Some(Value::String("variant".to_string())),
            "the track's existing variants must stay choosable on a lock-free step"
        );
    }

    #[test]
    fn duration_span_sync_updates_covered_steps_after_source_duration_change() {
        let state = std::sync::Arc::new(sequencer::sequencer::SequencerState::new(1, Vec::new()));
        state.pattern.track_params[0].set_num_steps(8);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Duration, 3.0);
        let mut runtime = Runtime::new();

        super::sync_track_duration_span_binding_fields(&mut runtime, &state, 0, 2);

        assert_eq!(
            runtime
                .eval_str(r#"(reactive-get "SEQ" "seq-track-step-duration-0-4")"#)
                .expect("read covered step"),
            Some(Value::Bool(true)),
            "duration source at step 2 with length 3 should mark step 4 covered"
        );
        assert_eq!(
            runtime
                .eval_str(r#"(reactive-get "SEQ" "seq-track-step-duration-0-5")"#)
                .expect("read uncovered step"),
            Some(Value::Bool(false)),
            "duration source at step 2 with length 3 should not cover step 5"
        );
    }

    #[test]
    fn sequencer_reveal_is_limited_to_navigation_keys() {
        assert!(key_should_reveal_sequencer_track(&KeyEvent::new(
            KeyCode::Up,
            KeyModifiers::NONE
        )));
        assert!(key_should_reveal_sequencer_track(&KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::NONE
        )));
        assert!(
            !key_should_reveal_sequencer_track(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            "plain Tab changes the app view and should not scroll a sequencer row"
        );
        assert!(
            !key_should_reveal_sequencer_track(&KeyEvent::new(
                KeyCode::Char('v'),
                KeyModifiers::NONE
            )),
            "parameter shortcuts should not reveal and scroll the current row"
        );
        assert!(
            !key_should_reveal_sequencer_track(&KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL)),
            "non-track-navigation tab shortcuts should not reveal the sequencer row"
        );
    }

    #[test]
    fn sequencer_visibility_keeps_track_meter_bindings_live_without_mixer() {
        assert!(track_meter_bindings_visible(true, false));
        assert!(track_meter_bindings_visible(false, true));
        assert!(track_meter_bindings_visible(true, true));
        assert!(!track_meter_bindings_visible(false, false));
    }

    #[test]
    fn new_instrument_starter_declares_standard_inputs_and_adsr() {
        let source = NEW_INSTRUMENT_STARTER_DSP;
        for (idx, name) in [
            (1, "gate"),
            (2, "pitch"),
            (3, "velocity"),
            (4, "trigger"),
            (5, "clock"),
            (6, "mod1"),
            (7, "mod2"),
            (8, "mod3"),
            (9, "mod4"),
        ] {
            assert!(
                source.contains(&format!("(def {name} (in {idx} @name {name}")),
                "starter source should declare input {idx} as {name}"
            );
        }
        for (idx, name) in [(1, "mod1"), (2, "mod2"), (3, "mod3"), (4, "mod4")] {
            assert!(
                source.contains(&format!(
                    "(def {name} (in {} @name {name} @modulator {idx}))",
                    idx + 5
                )),
                "starter source should mark {name} as modulator {idx}"
            );
        }
        for (name, role) in [
            ("attack", "attack"),
            ("decay", "decay"),
            ("sustain", "sustain"),
            ("release", "release"),
        ] {
            assert!(
                source.contains(&format!(
                    "(param {name} @group amp @env amp-env @role {role}"
                )),
                "starter source should tag {name} as amp-env {role}"
            );
        }
        assert!(source.contains("(def env (adsr gate trigger attack decay sustain release))"));
        assert!(source.contains("(out (* phase env velocity (mod gain)) 1 @name audio)"));
    }

    #[test]
    fn new_instrument_starter_compiles() {
        sequencer::lisp_host::compile_instrument(NEW_INSTRUMENT_STARTER_DSP, 44_100)
            .expect("starter instrument should compile");
    }

    #[test]
    fn new_effect_starter_compiles() {
        sequencer::lisp_host::compile_lisp(sequencer::lisp_host::EFFECT_TEMPLATE, 44_100)
            .expect("starter effect should compile");
    }

    #[test]
    fn instrument_patcher_buffer_uses_user_source_path_and_preview_command() {
        let source = instrument_patcher_buffer_source(
            "*instrument-patcher:digitone*",
            Path::new("instruments/digitone/dsp.lisp"),
        );

        assert!(source.contains("(effect-buffer \"*instrument-patcher:digitone*\""));
        assert!(source.contains(":intent :instrument"));
        assert!(source.contains(":path \"instruments/digitone/dsp.lisp\""));
        assert!(source.contains("(host-command \"preview-instrument-patch\" event)"));
        assert!(!source.contains("eseq.patch-learn"));
        assert!(!source.contains("patch-learn-open"));
        assert!(!source.contains("defmacro"));
    }

    #[test]
    fn patch_learn_effective_source_contains_the_instrument_preamble_before_the_patch() {
        let source = r#"
            (def env (adsr gate trigger attack decay sustain release))
            (def curved
              (adsrexp gate trigger attack decay sustain release attack_curve fall_curve))
        "#;
        let effective = sequencer::lisp_host::effective_instrument_source(source, 44_100)
            .expect("prepare Patch Learn source");
        for (name, call_source) in [
            ("adsr", "(adsr gate trigger attack decay sustain release)"),
            (
                "adsrexp",
                "(adsrexp gate trigger attack decay sustain release attack_curve fall_curve)",
            ),
        ] {
            let definition = effective
                .find(&format!("(defmacro {name}"))
                .unwrap_or_else(|| panic!("instrument preamble should define {name}"));
            let call = effective
                .rfind(call_source)
                .unwrap_or_else(|| panic!("editor patch should retain its {name} call"));
            assert!(definition < call, "{name} must be defined before the patch is evaluated");
        }
    }

    #[test]
    fn effect_patcher_buffer_uses_effect_intent_and_preview_command() {
        let source = effect_patcher_buffer_source(
            "*effect-patcher:lexilush*",
            Path::new("effects/lexilush/dsp.lisp"),
        );

        assert!(source.contains("(effect-buffer \"*effect-patcher:lexilush*\""));
        assert!(source.contains(":intent :effect"));
        assert!(source.contains(":path \"effects/lexilush/dsp.lisp\""));
        assert!(source.contains("(host-command \"preview-effect-patch\" event)"));
        assert!(!source.contains("defmacro"));
    }

    #[test]
    fn editor_surface_routes_by_authored_sidecar_and_projectability() {
        let dir = std::env::temp_dir().join(format!(
            "eseq-editor-surface-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dsp_path = dir.join("dsp.lisp");
        let clean_source = "(def pitch (in 1 @name pitch))\n(def phase (phasor pitch))\n(out phase 1 @name audio)";
        std::fs::write(&dsp_path, clean_source).unwrap();
        let intent = eseqlisp::widget_render::patcher::PatcherIntent::Instrument;

        assert_eq!(
            editor_surface_for_existing(&dsp_path, clean_source, intent),
            EditorSurface::Code,
            "no sidecar → code editor"
        );

        let layout_path = dir.join("dsp.layout.json");
        std::fs::write(
            &layout_path,
            r#"{ "version": 2, "authored": true, "root": { "nodes": {}, "cables": {} } }"#,
        )
        .unwrap();
        assert_eq!(
            editor_surface_for_existing(&dsp_path, clean_source, intent),
            EditorSurface::Patch,
            "authored sidecar + projectable source → patch editor"
        );

        let island_source = format!("{clean_source}\n(let ((x 1)) x)\n");
        assert_eq!(
            editor_surface_for_existing(&dsp_path, &island_source, intent),
            EditorSurface::Code,
            "code islands demote to the code editor even with an authored sidecar"
        );

        std::fs::write(
            &layout_path,
            r#"{ "version": 1, "root": { "nodes": {}, "cables": {} } }"#,
        )
        .unwrap();
        assert_eq!(
            editor_surface_for_existing(&dsp_path, clean_source, intent),
            EditorSurface::Code,
            "auto-materialized v1 sidecars never count as authored"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn promotion_and_eject_flip_editor_surface_routing() {
        let dir = std::env::temp_dir().join(format!(
            "eseq-promote-eject-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dsp_path = dir.join("dsp.lisp");
        let clean_source = "(def pitch (in 1 @name pitch))\n(def phase (phasor pitch))\n(out phase 1 @name audio)";
        std::fs::write(&dsp_path, clean_source).unwrap();
        let intent = eseqlisp::widget_render::patcher::PatcherIntent::Instrument;

        assert_eq!(
            editor_surface_for_existing(&dsp_path, clean_source, intent),
            EditorSurface::Code,
            "unpromoted item opens as code"
        );

        // §3.3 promotion: clean source stamps an authored sidecar → patch.
        eseqlisp::widget_render::patcher::promote_source_to_patch(
            &dsp_path,
            clean_source,
            intent,
        )
        .expect("clean source should promote");
        assert_eq!(
            editor_surface_for_existing(&dsp_path, clean_source, intent),
            EditorSurface::Patch,
            "promotion routes back into the patch editor"
        );

        // §3.4 eject: flips authored off but keeps layout for re-promotion.
        let layout_path = dir.join("dsp.layout.json");
        let before: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&layout_path).unwrap()).unwrap();
        eseqlisp::widget_render::patcher::eject_patch_authored_sidecar(&dsp_path)
            .expect("eject should flip the authored flag");
        assert_eq!(
            editor_surface_for_existing(&dsp_path, clean_source, intent),
            EditorSurface::Code,
            "ejected item opens as code"
        );
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&layout_path).unwrap()).unwrap();
        assert_eq!(after["authored"], serde_json::json!(false));
        assert_eq!(
            after["root"]["nodes"], before["root"]["nodes"],
            "eject keeps layout data for re-promotion"
        );

        // §3.3 refusal: code islands block promotion with a diagnostic.
        let island_source = format!("{clean_source}\n(let ((x 1)) x)\n");
        let error = eseqlisp::widget_render::patcher::promote_source_to_patch(
            &dsp_path,
            &island_source,
            intent,
        )
        .expect_err("code islands must refuse promotion");
        assert!(
            error.contains("Cannot open as patch"),
            "promotion refusal should carry the diagnostic list: {error}"
        );
        assert_eq!(
            editor_surface_for_existing(&dsp_path, &island_source, intent),
            EditorSurface::Code
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn code_buffer_names_are_distinct_from_patcher_buffers() {
        assert_eq!(
            instrument_code_buffer_name("digitone"),
            "*instrument-code:digitone*"
        );
        assert_eq!(effect_code_buffer_name("lexilush"), "*effect-code:lexilush*");
    }

    #[test]
    fn instrument_patcher_buffer_escapes_lisp_path_strings() {
        assert_eq!(escape_lisp_string("a\\b\"c"), "a\\\\b\\\"c");
    }

    #[test]
    fn instrument_patcher_layout_preserves_lower_panel_buffer() {
        let source = show_instrument_patcher_layout_source("*instrument-patcher:digitone*");

        assert_eq!(
            source,
            "(eseq.seq-layout/apply-instrument-patcher-layout \"*instrument-patcher:digitone*\")"
        );
    }

    #[test]
    fn instrument_patcher_source_layout_includes_patcher_and_source_buffers() {
        let source = show_instrument_patcher_source_layout_source(
            "*instrument-patcher:digitone*",
            "*patcher-emitted:instruments/digitone/dsp.lisp*",
        );

        assert_eq!(
            source,
            "(eseq.seq-layout/apply-instrument-patcher-source-layout \"*instrument-patcher:digitone*\" \"*patcher-emitted:instruments/digitone/dsp.lisp*\")"
        );
    }

    #[test]
    fn patcher_layout_sidecar_uses_stem_for_legacy_single_file_effects() {
        assert_eq!(
            patcher_layout_sidecar_path_for_dsp(Path::new("effects/legacy-delay.lisp")),
            Path::new("effects/legacy-delay.layout.json")
        );
        assert_eq!(
            patcher_layout_sidecar_path_for_dsp(Path::new("effects/lexilush/dsp.lisp")),
            Path::new("effects/lexilush/dsp.layout.json")
        );
    }

    #[test]
    fn instrument_patcher_layout_restore_uses_remembered_step_panel() {
        let source = restore_instrument_patcher_layout_source();

        assert_eq!(source, "(eseq.seq-panels/seq-restore-instrument-patcher-layout)");
    }

    #[test]
    fn agent_instrument_stub_ui_parses() {
        let tokens = Parser::new(AGENT_INSTRUMENT_STUB_UI.to_string())
            .parse()
            .expect("stub UI should tokenize");
        ASTParser::new(tokens)
            .parse()
            .expect("stub UI should parse");
    }

    #[test]
    fn agent_instrument_stub_ui_registers_as_custom_synth_ui() {
        const LEGACY_AGENT_INSTRUMENT_STUB_UI: &str = r#"(defwidget agent-instrument-stub-bg-legacy
  :width 70 :height 8.2
  :shader
  (sdf/fill
    (sdf/rounded-rect width height 0.45)
    (material :color (rgba (+ 0.1 (* 0.1 (sin itime))) 0.2 0.4 1.0))))

(defsynth-ui
  (box :width 70 :height 8.2 :padding 0 :debug-name "agent-instrument-stub-skeleton"
    (agent-instrument-stub-bg-legacy)))
"#;
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "agent-draft-1/".to_string(),
            "instruments/agent-draft-1/ui.lisp".to_string(),
            LEGACY_AGENT_INSTRUMENT_STUB_UI.to_string(),
        )));
        let mut runtime = Runtime::new();
        runtime
            .eval_str(
                r#"(def synth-ui-current-inst false)
                (def synth-ui-current-name "")
                (def custom-ui-current-kind "instrument")
                (def custom-ui-selected-section 0)
                (def eseq.effects.custom-ui-sections/custom-ui-selected-section-for-current-scope () 0)
                (def agent-instrument-stub-bg ()
                    (box :width 70 :height 8.2
                      (label "stub" :font-size 10 :color :gray :bg :transparent)))"#,
            )
            .expect("install stub widget test double");
        runtime
            .eval_str(&custom_ui_source)
            .expect("stub custom UI should evaluate");
        let rendered = runtime
            .eval_str(
                r#"(custom-instrument-synth-ui
                     (dict :name "agent-draft-1/"
                           :synth (list (dict :name "base_note"
                                              :control "eseq.effects.custom-ui-runtime/base-note"
                                              :value 0
                                              :min -48
                                              :max 48))))"#,
            )
            .expect("stub custom UI should render");
        assert!(
            !matches!(rendered, Some(Value::Bool(false)) | None),
            "stub instrument should dispatch to its custom skeleton UI"
        );
    }

    struct SequencerDirGuard {
        original: std::path::PathBuf,
    }

    impl SequencerDirGuard {
        fn enter() -> Self {
            let original = std::env::current_dir().expect("read current dir");
            sequencer::paths::enter_sequencer_dir().expect("enter sequencer crate dir");
            Self { original }
        }
    }

    impl Drop for SequencerDirGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
        }
    }

    struct TestEngineGuard {
        lg_raw: *mut sequencer::audiograph::LiveGraph,
    }

    impl Drop for TestEngineGuard {
        fn drop(&mut self) {
            unsafe {
                sequencer::audiograph::clear_os_workgroup();
                sequencer::audiograph::engine_stop_workers();
                sequencer::audiograph::destroy_live_graph(self.lg_raw);
            }
        }
    }

    struct HeadlessAudioPump {
        running: std::sync::Arc<std::sync::atomic::AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl HeadlessAudioPump {
        fn start(lg_ptr: sequencer::audiograph::LiveGraphPtr, channels: usize) -> Self {
            let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let worker_running = std::sync::Arc::clone(&running);
            let handle = std::thread::Builder::new()
                .name("project-92-headless-audio-pump".to_string())
                .spawn(move || {
                    let frames = 512;
                    let mut output = vec![0.0f32; frames * channels.max(1)];
                    while worker_running.load(std::sync::atomic::Ordering::Relaxed) {
                        unsafe {
                            lg_ptr.process_next_block(output.as_mut_ptr(), frames as i32);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                })
                .expect("spawn headless audio pump");
            Self {
                running,
                handle: Some(handle),
            }
        }
    }

    impl Drop for HeadlessAudioPump {
        fn drop(&mut self) {
            self.running
                .store(false, std::sync::atomic::Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn find_layout_node_by_stable_key<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        key: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if node.stable_key.as_deref() == Some(key) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_layout_node_by_stable_key(child, key))
    }

    fn find_layout_node_by_stable_key_suffix<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        suffix: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if node
            .stable_key
            .as_deref()
            .is_some_and(|key| key.ends_with(suffix))
        {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_layout_node_by_stable_key_suffix(child, suffix))
    }

    fn find_layout_node_by_debug_name<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        name: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if matches!(node.props.get("debug-name"), Some(Value::String(value)) if value == name) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_layout_node_by_debug_name(child, name))
    }

    fn layout_prop_number(node: &eseqlisp::layout::LayoutNode, name: &str) -> Option<f64> {
        match node.props.get(name) {
            Some(Value::Number(value)) => Some(*value),
            Some(Value::ReactiveRef { slot, .. }) => {
                Some(eseqlisp::reactive::read_float_slot(slot) as f64)
            }
            _ => None,
        }
    }

    /// Depth-first walk collecting every node the predicate accepts. Used by
    /// the drag probes to discover the real control nodes in a tile layout
    /// instead of hard-coding widget ids.
    fn collect_layout_nodes<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        keep: &mut dyn FnMut(&eseqlisp::layout::LayoutNode) -> bool,
        out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
    ) {
        if keep(node) {
            out.push(node);
        }
        for child in &node.children {
            collect_layout_nodes(child, keep, out);
        }
    }

    fn find_layout_node_by_widget_type<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        widget_type: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if node.widget_type == widget_type {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_layout_node_by_widget_type(child, widget_type))
    }

    fn visible_layout_revisions(editor: &eseqlisp::Editor) -> Vec<(String, u64)> {
        let mut revisions = editor
            .tile_root
            .leaf_ids()
            .into_iter()
            .filter_map(|tile_id| {
                let leaf = editor.tile_root.find_leaf(tile_id)?;
                let buffer = editor.buffers.get(leaf.buffer_idx)?;
                Some((buffer.name.clone(), leaf.layout_revision))
            })
            .collect::<Vec<_>>();
        revisions.sort_by(|a, b| a.0.cmp(&b.0));
        revisions
    }

    fn changed_layout_buffers(before: &[(String, u64)], after: &[(String, u64)]) -> Vec<String> {
        let before = before
            .iter()
            .cloned()
            .collect::<std::collections::HashMap<_, _>>();
        after
            .iter()
            .filter_map(|(name, revision)| {
                (before.get(name).copied() != Some(*revision)).then(|| name.clone())
            })
            .collect()
    }

    #[test]
    #[ignore = "perf probe: initializes the real metal_seq app graph and loads crates/sequencer/projects/92.json"]
    fn project_92_mixer_track_badge_switch_reports_layout_work() {
        std::thread::Builder::new()
            .name("project-92-track-switch-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(project_92_mixer_track_badge_switch_reports_layout_work_impl)
            .expect("spawn project 92 track switch probe")
            .join()
            .expect("project 92 track switch probe should pass");
    }

    fn project_92_mixer_track_badge_switch_reports_layout_work_impl() {
        use super::*;
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use std::collections::HashSet;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::Instant;

        fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
            MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }
        }

        let _dir = SequencerDirGuard::enter();
        assert!(
            Path::new("projects/92.json").exists(),
            "project 92 must be available at crates/sequencer/projects/92.json"
        );

        let eng = engine::init_headless_engine(44_100, 2).expect("initialize headless app graph");
        let lg_raw = eng.lg_ptr.0;
        let state = eng.state.clone();
        let lg_ptr = eng.lg_ptr;
        let sample_rate = eng.sample_rate;
        let _engine_guard = TestEngineGuard { lg_raw };
        let _audio_pump = HeadlessAudioPump::start(lg_ptr, eng.channels as usize);
        let master_recorder = eng.master_recorder.clone();
        let mut app = app::App::new(
            state.clone(),
            lg_ptr,
            sample_rate,
            eng.buses,
            eng.master_recorder,
            eng.keyboard_tx,
        );

        let mut track_names = Vec::<String>::new();
        let track_pan_ids = Arc::new(Mutex::new(Vec::<i32>::new()));
        let track_collapsed = Arc::new(Mutex::new(app.track_collapsed.clone()));
        let bus_state = Arc::new(Mutex::new(app.buses.clone()));
        let bus_node_ids = Arc::new(Mutex::new(app.graph.bus_node_ids.clone()));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_tracks = Arc::new(Mutex::new(HashSet::<usize>::new()));
        let track_groups = Arc::new(Mutex::new(app.groups.clone()));
        let selected_steps = Arc::new(Mutex::new(HashSet::<usize>::new()));
        let selected_neural_neurons: sequencer::lisp_host::SharedSelectedNeuralNeurons =
            Arc::new(Mutex::new(BTreeSet::new()));
        let piano_roll_selection = Arc::new(Mutex::new(HashSet::<u64>::new()));
        let piano_roll_move_state = Arc::new(Mutex::new(None));
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        let fx_epoch = Arc::new(AtomicUsize::new(0));
        let ui_invalidations = Arc::new(UiInvalidationQueue::new());
        let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
        let recording = Arc::new(AtomicBool::new(false));
        let master_recording = Arc::new(AtomicBool::new(false));
        let record_armed = Arc::new(Mutex::new(Vec::<bool>::new()));
        let armed_rack: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let active_delete_target = Arc::new(Mutex::new(None));
        let active_delete_target_version = Arc::new(AtomicUsize::new(0));
        let auto_follow_override_until = Arc::new(Mutex::new(None));

        let RuntimeInit {
            runtime,
            accumulator_names,
            midi_fx_names: _,
            sample_browser: _,
            piano_roll_clipboard: _,
            selected_drum_lane_steps: _,
        } = init_runtime(
            &app,
            state.clone(),
            &track_names,
            track_pan_ids.clone(),
            track_collapsed.clone(),
            bus_state.clone(),
            bus_node_ids.clone(),
            current_track.clone(),
            selected_tracks.clone(),
            track_groups.clone(),
            selected_steps.clone(),
            piano_roll_selection.clone(),
            piano_roll_move_state,
            super::new_shared_piano_roll_focus(),
            recording.clone(),
            master_recording.clone(),
            master_recorder.clone(),
            record_armed.clone(),
            armed_rack.clone(),
            ui_epoch.clone(),
            fx_epoch.clone(),
            ui_invalidations.clone(),
            expanded_step_projection.clone(),
            selected_neural_neurons.clone(),
            active_delete_target.clone(),
            active_delete_target_version.clone(),
            auto_follow_override_until.clone(),
            lg_raw,
        );

        let mut editor = Editor::new(
            runtime,
            eseqlisp::EditorConfig {
                vim_mode: true,
                ..eseqlisp::EditorConfig::default()
            },
        );
        reload_custom_instrument_ui(&mut editor);
        let _ = editor.open_or_create_file_buffer(UI_ENTRYPOINT_PATH);
        let grid_source = editor.active_buffer().text();
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            Some(std::path::PathBuf::from(UI_ENTRYPOINT_PATH)),
            &grid_source,
            overlays,
        );
        assert!(
            report.success,
            "failed to load grid UI: {}",
            report.failure_message()
        );
        editor.process_lisp_reload_report(report);
        editor.refresh_runtime_side_effects();
        reload_custom_instrument_ui(&mut editor);
        editor.set_layout_viewport(180, 70);
        editor.update_tile_rects(180, 70);
        let _ = editor.drain_host_commands();

        app.queue_project_load_named("92")
            .expect("queue project 92 load");
        for _ in 0..512 {
            if !app.has_pending_project_load() {
                break;
            }
            app.advance_pending_project_load()
                .expect("advance project 92 load");
        }
        assert!(
            !app.has_pending_project_load(),
            "project 92 load did not finish"
        );
        assert!(
            app.tracks.len() >= 2,
            "project 92 should have multiple tracks"
        );

        current_track.store(0, Ordering::Relaxed);
        *track_pan_ids.lock().unwrap() = app
            .graph
            .track_node_ids
            .iter()
            .map(|ids| ids.pan_id)
            .collect();
        *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
        *record_armed.lock().unwrap() = vec![false; app.tracks.len()];
        sync_shared_track_collapsed(&track_collapsed, &app);
        push_project_scratch_to_named_buffer(&mut editor, &app);
        if let Err(error) = evaluate_project_scratch_on_ui_runtime(&mut editor, &app) {
            editor.handle_host_event(HostEvent::Status(format!("Scratch UI eval error: {error}")));
        }

        let cached_track_peak_levels = vec![0.0; track_names.len()];
        let cached_bus_peak_levels = read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
        let (cached_modulator_phases, cached_modulator_levels) =
            read_modulator_display_values(app.graph.lg, &app);

        {
            let rt = editor.runtime_mut();
            sync_project_state(rt, &app);
            sync_track_topology_state(
                rt,
                &app,
                &state,
                &mut track_names,
                0,
                &selected_steps,
                &piano_roll_selection,
                &accumulator_names,
                &record_armed,
                &cached_track_peak_levels,
            );
            rt.set_reactive(
                "SEQ",
                "selected-steps",
                build_selection_value(&selected_steps),
            );
            rt.set_reactive(
                "SEQ",
                "bus-effects",
                build_bus_effects_value_for_selection(&app, Some(&selected_steps)),
            );
            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
            sync_modulator_phase_fields(rt, &cached_modulator_phases);
            sync_modulator_level_fields(rt, &cached_modulator_levels);
            sync_mixer_delete_target_binding_fields(
                rt,
                app.tracks.len(),
                &state,
                active_delete_target.lock().unwrap().as_ref(),
            );
            rt.set_reactive(
                "SEQ",
                "delete-target-version",
                Value::Number(active_delete_target_version.load(Ordering::Relaxed) as f64),
            );
            rt.run_reactive_cycle();
        }
        editor.refresh_runtime_side_effects();
        refresh_visible_track_topology_layouts(&mut editor);
        editor.update_tile_rects(180, 70);
        let _ = editor.drain_host_commands();

        let mixer_buffer_idx = editor
            .buffers
            .iter()
            .position(|buffer| buffer.name == "*mixer*")
            .expect("mixer buffer");
        let mixer_tile = editor
            .tile_root
            .find_leaf_by_buffer_idx(mixer_buffer_idx)
            .expect("visible mixer tile");
        let mixer_tile_id = mixer_tile.id;
        editor.switch_active_tile(mixer_tile_id);
        let mixer_layout = editor.widget_layout().expect("mixer active layout");
        let target_track = 1usize;
        let target_badge = find_layout_node_by_stable_key_suffix(
            &mixer_layout,
            &format!("/track-label-{target_track}"),
        )
        .expect("target mixer track badge");
        let click_col = target_badge.rect.col + target_badge.rect.width * 0.5;
        let click_row = target_badge.rect.row + target_badge.rect.height * 0.5;
        let content_width = mixer_layout.rect.width.ceil().max(1.0) as u16;
        let content_height = mixer_layout.rect.height.ceil().max(1.0) as u16;
        let before_revisions = visible_layout_revisions(&editor);

        let measured = Instant::now();
        let phase = Instant::now();
        editor.handle_mouse_precise(
            mouse_event(
                MouseEventKind::Down(MouseButton::Left),
                click_col.floor() as u16,
                click_row.floor() as u16,
            ),
            0,
            0,
            content_width,
            content_height,
            click_col,
            click_row,
        );
        let commands = editor.drain_host_commands();
        let click_dispatch = phase.elapsed();
        assert!(
            commands.iter().any(|command| matches!(
                command,
                HostCommand::Custom { name, .. } if name == "reveal-sequencer-track"
            )),
            "mixer track badge click should queue reveal-sequencer-track, got {commands:?}"
        );
        assert_eq!(
            current_track.load(Ordering::Relaxed),
            target_track,
            "mixer track badge click should select the target track"
        );

        let phase = Instant::now();
        let ct =
            current_track_for_app(&mut app, &current_track).expect("current track after click");
        editor.reset_widget_scroll_for_buffer_named("*metal*");
        editor.reset_widget_scroll_for_buffer_named("*fx*");
        editor
            .runtime_mut()
            .eval_str("(set! eseq.seq-core-state/selected-bus -1)")
            .expect("clear selected bus");
        reset_sampler_waveform_view(&mut editor);
        let pre_sync = phase.elapsed();

        let phase = Instant::now();
        {
            let rt = editor.runtime_mut();
            sync_track_topology_state(
                rt,
                &app,
                &state,
                &mut track_names,
                ct,
                &selected_steps,
                &piano_roll_selection,
                &accumulator_names,
                &record_armed,
                &cached_track_peak_levels,
            );
        }
        let topology_sync = phase.elapsed();

        let phase = Instant::now();
        {
            let rt = editor.runtime_mut();
            rt.set_reactive(
                "SEQ",
                "selected-steps",
                build_selection_value(&selected_steps),
            );
            rt.set_reactive(
                "SEQ",
                "bus-effects",
                build_bus_effects_value_for_selection(&app, Some(&selected_steps)),
            );
            sync_mixer_delete_target_binding_fields(
                rt,
                app.tracks.len(),
                &state,
                active_delete_target.lock().unwrap().as_ref(),
            );
            rt.set_reactive(
                "SEQ",
                "delete-target-version",
                Value::Number(active_delete_target_version.load(Ordering::Relaxed) as f64),
            );
            rt.run_reactive_cycle();
        }
        let reactive_cycle = phase.elapsed();

        let phase = Instant::now();
        editor.refresh_runtime_side_effects();
        let runtime_side_effects = phase.elapsed();

        let phase = Instant::now();
        reveal_sequencer_current_track(&mut editor, &app, ct);
        let sequencer_reveal = phase.elapsed();

        let phase = Instant::now();
        editor.mark_needs_redraw();
        let redraw_mark = phase.elapsed();
        let elapsed = measured.elapsed();

        let after_revisions = visible_layout_revisions(&editor);
        let changed_buffers = changed_layout_buffers(&before_revisions, &after_revisions);
        let trace = editor
            .runtime()
            .last_ui_invalidation_trace()
            .expect("track switch should produce an invalidation trace");
        let mut relayout_timings = Vec::<(String, String, f64)>::new();
        if trace.relayout_duration > std::time::Duration::ZERO {
            relayout_timings.push((
                editor.active_buffer().name.clone(),
                format!(
                    "active-{}",
                    trace.relayout_mode.as_deref().unwrap_or("unknown")
                ),
                trace.relayout_duration.as_secs_f64() * 1000.0,
            ));
        }
        relayout_timings.extend(editor.last_layout_refresh_timings().iter().map(|timing| {
            (
                timing.buffer_name.clone(),
                format!(
                    "inactive-{}-tile-{}",
                    timing.mode,
                    timing
                        .tile_id
                        .map(|tile_id| tile_id.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ),
                timing.elapsed.as_secs_f64() * 1000.0,
            )
        }));
        relayout_timings.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        let worst_relayout = relayout_timings.first().cloned();

        eprintln!(
            "[project-92-track-switch] track=1 elapsed_ms={:.3} click_dispatch_ms={:.3} pre_sync_ms={:.3} topology_sync_ms={:.3} reactive_cycle_ms={:.3} runtime_side_effects_ms={:.3} sequencer_reveal_ms={:.3} redraw_mark_ms={:.3} changed_layout_buffers={:?} sequencer_relayout={} relayout_timings={:?} worst_relayout={:?} dirty_fields={} affected_buffers={:?} widget_tree_flushes={} full_reruns={} subtree_reruns={} relayout_mode={:?} relayout_ms={:.3} relayout_failure={:?}",
            elapsed.as_secs_f64() * 1000.0,
            click_dispatch.as_secs_f64() * 1000.0,
            pre_sync.as_secs_f64() * 1000.0,
            topology_sync.as_secs_f64() * 1000.0,
            reactive_cycle.as_secs_f64() * 1000.0,
            runtime_side_effects.as_secs_f64() * 1000.0,
            sequencer_reveal.as_secs_f64() * 1000.0,
            redraw_mark.as_secs_f64() * 1000.0,
            changed_buffers,
            changed_buffers.iter().any(|name| name == "*sequencer*"),
            relayout_timings,
            worst_relayout,
            trace.dirty_fields.len(),
            trace.affected_buffers,
            trace.widget_tree_flushes,
            trace.full_buffer_reruns,
            trace.subtree_reruns,
            trace.relayout_mode,
            trace.relayout_duration.as_secs_f64() * 1000.0,
            trace.relayout_failure_reason,
        );

        assert!(
            changed_buffers.iter().any(|name| name == "*fx*"),
            "fx layout should change after selecting a different track"
        );
        assert!(
            !changed_buffers.iter().any(|name| name == "*sequencer*"),
            "sequencer should reveal the selected track from its cached layout without relayout"
        );
        assert!(
            !trace.affected_buffers.iter().any(|name| name == "*mixer*"),
            "mixer track badge selection should use widget bindings instead of rerunning the mixer widget tree"
        );
        assert_eq!(
            trace.subtree_reruns, 0,
            "track switch should not rerun mixer/sequencer subtree work for badge styling"
        );
        assert!(
            trace.widget_tree_flushes > 0,
            "track switch should report widget tree work"
        );
    }

    #[test]
    #[ignore = "perf probe: initializes the real metal_seq app graph and loads crates/sequencer/projects/92.json"]
    fn project_92_scene_switch_reports_layout_work() {
        std::thread::Builder::new()
            .name("project-92-scene-switch-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| project_92_ui_performance_probe_impl(Project92UiProbe::SceneSwitch))
            .expect("spawn project 92 scene switch probe")
            .join()
            .expect("project 92 scene switch probe should pass");
    }

    #[test]
    #[ignore = "release-mode perf probe: focuses a non-sequencer tile, sends Escape through the real binding and invalidation path, builds the tiled frame, and refreshes the retained Metal primitive scene"]
    fn project_92_escape_clears_48_of_64_selected_steps_end_to_end_perf() {
        std::thread::Builder::new()
            .name("project-92-escape-selection-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| project_92_ui_performance_probe_impl(Project92UiProbe::EscapeDeselect))
            .expect("spawn project 92 Escape selection probe")
            .join()
            .expect("project 92 Escape selection probe should pass");
    }

    #[test]
    #[ignore = "release-mode perf probe: compares a three-target instrument-rack macro with a direct rack number-picker through the real mouse, host-command, reactive, tiled-frame, and retained-render paths"]
    fn project_92_rack_macro_drag_end_to_end_perf() {
        std::thread::Builder::new()
            .name("project-92-rack-macro-drag-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| project_92_ui_performance_probe_impl(Project92UiProbe::RackMacroDrag))
            .expect("spawn project 92 rack macro drag probe")
            .join()
            .expect("project 92 rack macro drag probe should pass");
    }

    #[test]
    #[ignore = "release-mode perf probe: exercises sequencer Cmd+A and real step pointer gestures through host mutation, targeted invalidation, tiled-frame, and retained-render paths"]
    fn project_92_step_interactions_end_to_end_perf() {
        std::thread::Builder::new()
            .name("project-92-step-interactions-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| project_92_ui_performance_probe_impl(Project92UiProbe::StepInteractions))
            .expect("spawn project 92 step interaction probe")
            .join()
            .expect("project 92 step interaction probe should pass");
    }

    #[test]
    #[ignore = "release-mode perf probe: Cmd+A select-all and Escape unselect-all under the real production multi-pane layout (eseq.seq-layout/apply-fx-layout), with retained Metal updates for every visible tile"]
    fn project_92_full_layout_step_interactions_end_to_end_perf() {
        std::thread::Builder::new()
            .name("project-92-full-layout-step-interactions-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                project_92_ui_performance_probe_impl(Project92UiProbe::StepInteractionsFullLayout)
            })
            .expect("spawn project 92 full-layout step interaction probe")
            .join()
            .expect("project 92 full-layout step interaction probe should pass");
    }

    #[test]
    #[ignore = "release-mode perf probe: step interactions with a realistic committed arrangement (scene lane + clip lanes) present, exercising the per-tick song-state sync the real event loop runs"]
    fn project_92_arranged_step_interactions_end_to_end_perf() {
        std::thread::Builder::new()
            .name("project-92-arranged-step-interactions-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| project_92_ui_performance_probe_impl(Project92UiProbe::ArrangedStepInteractions))
            .expect("spawn project 92 arranged step interaction probe")
            .join()
            .expect("project 92 arranged step interaction probe should pass");
    }

    #[test]
    #[ignore = "release-mode perf probe: Seq-view selection gestures (Cmd+A, shift-drag range, cmd-drag multi-select, toggle drag) on the real saved pianohold project (takes, use_arrangement, ~137 clips)"]
    fn pianohold_step_selection_end_to_end_perf() {
        std::thread::Builder::new()
            .name("pianohold-step-selection-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| project_92_ui_performance_probe_impl(Project92UiProbe::PianoholdSelection))
            .expect("spawn pianohold step selection probe")
            .join()
            .expect("pianohold step selection probe should pass");
    }

    #[test]
    #[ignore = "release-mode perf probe: instrument knob drag with and without a selected step (p-lock write) under the real production multi-pane layout, with retained Metal updates for every visible tile"]
    fn project_92_full_layout_instrument_plock_knob_drag_end_to_end_perf() {
        std::thread::Builder::new()
            .name("project-92-full-layout-plock-knob-drag-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                project_92_ui_performance_probe_impl(Project92UiProbe::InstrumentPlockKnobDrag)
            })
            .expect("spawn project 92 full-layout p-lock knob drag probe")
            .join()
            .expect("project 92 full-layout p-lock knob drag probe should pass");
    }

    #[test]
    #[ignore = "release-mode perf probe: response-curve-editor drag versus a plain knob drag on the same builtin Filter params (no step selected) under the real production multi-pane layout"]
    fn project_92_full_layout_response_curve_editor_drag_end_to_end_perf() {
        std::thread::Builder::new()
            .name("project-92-full-layout-response-curve-drag-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                project_92_ui_performance_probe_impl(Project92UiProbe::ResponseCurveEditorDrag)
            })
            .expect("spawn project 92 full-layout response-curve drag probe")
            .join()
            .expect("project 92 full-layout response-curve drag probe should pass");
    }

    #[test]
    #[ignore = "release-mode perf probe: *step*-buffer Transpose/Velocity/Duration number-picker drags (real mouse -> lisp -> set-step-param-history handler) under the production multi-pane layout"]
    fn project_92_full_layout_step_buffer_param_drag_end_to_end_perf() {
        std::thread::Builder::new()
            .name("project-92-full-layout-step-buffer-param-drag-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| project_92_ui_performance_probe_impl(Project92UiProbe::StepBufferParamDrag))
            .expect("spawn project 92 full-layout step-buffer param drag probe")
            .join()
            .expect("project 92 full-layout step-buffer param drag probe should pass");
    }

    #[test]
    #[ignore = "release-mode perf probe: scene launch (transport scene pill) and track-clip launch (mixer pattern cell) through the real dispatch_custom_host_command seam, on project 92 with every track's pattern pool grown to 20 clips, under the production multi-pane layout"]
    fn project_92_full_layout_scene_and_clip_launch_end_to_end_perf() {
        std::thread::Builder::new()
            .name("project-92-full-layout-launch-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| project_92_ui_performance_probe_impl(Project92UiProbe::SceneAndClipLaunch))
            .expect("spawn project 92 full-layout launch probe")
            .join()
            .expect("project 92 full-layout launch probe should pass");
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Project92UiProbe {
        SceneSwitch,
        EscapeDeselect,
        RackMacroDrag,
        StepInteractions,
        /// Cmd+A select-all / Escape unselect-all under the real production
        /// multi-pane startup layout (`seq-apply-fx-layout`, exactly as
        /// `editor_setup::create_editor` installs it): transport bar, samples
        /// sidebar, sequencer, step/track panels, mixer strip, and the *fx*
        /// lower panel are all visible, so the fx/mixer selection publication
        /// paths and every tile's retained Metal update are inside the timed
        /// region.
        StepInteractionsFullLayout,
        /// Selection-gesture probe on the saved `pianohold` project: a real
        /// takes-bearing arrangement (take_pools, use_arrangement, ~137
        /// clips), covering the drag-selection paths the project-92 probes
        /// do not exercise.
        PianoholdSelection,
        /// Same interactions as `StepInteractions`, but with a committed
        /// arrangement at realistic scale (pianohold.json-like: 18 scene
        /// events, ~18 clips per track lane) and the real reactive-tick song
        /// syncs inside the timed region — the configuration where the
        /// arrangement feature can tax Seq-view step editing.
        ArrangedStepInteractions,
        /// Instrument knob drag under the production multi-pane layout, with
        /// and without a selected step. With a selection the knob's
        /// `on-change` lowers to `set-instrument-plock` instead of
        /// `set-instrument-param`, which bumps BOTH `ui_epoch` and
        /// `fx_epoch` on every drag update — the probe replays the real
        /// reactive tick's epoch-driven resyncs so that cost is measured.
        InstrumentPlockKnobDrag,
        /// `response-curve-editor` drag versus a plain knob drag on the same
        /// builtin Filter cutoff/resonance params, no step selected. The
        /// curve emits `set-effect-param-batch` (two params per update)
        /// through `builtin-fx-handle-filter-curve-action`; the knob emits
        /// `set-effect-param` for one of them, so the two medians are
        /// directly comparable.
        ResponseCurveEditorDrag,
        /// The *step* buffer's Transpose / Velocity / Duration number-pickers
        /// (`ui/effects/step-buffer.lisp` -> `fx-step-parameters-panel`),
        /// dragged with the step cursor parked on a step and nothing selected
        /// — the "step 4 · 0 selected" panel the user edits. Each drag update
        /// runs `fx-step-set-param` -> `seq-set-step-param` ->
        /// `set-step-param-history`, dispatched through the REAL
        /// `dispatch_custom_host_command` seam (not a mirror of it) so the
        /// handler's own epoch/invalidation policy is what the probe measures.
        StepBufferParamDrag,
        /// Scene launch and track-clip launch under the production
        /// multi-pane layout, at "large project" clip scale: after loading
        /// project 92 the fixture grows every track's pattern pool to 20
        /// clips (via the real `fork_current_track_pattern` primitive), the
        /// configuration where launches get laggy in real projects.
        ///
        /// Both gestures are the real ones — a mouse Down on a
        /// `transport-scene-pill-*` (-> `switch-pattern`) and on a
        /// `eseq.mixer/track-pattern-cell-*` (-> `set-scene-cell`, the live
        /// clip-launch path; `launch-track-pattern` is dead) — and both host
        /// commands run through the REAL `dispatch_custom_host_command`
        /// seam. The visible update replays the reactive tick INCLUDING its
        /// pattern-epoch resync branch: `switch-pattern` does its project
        /// resync inline (and stamps `ctx.frame.prev_pattern_epoch`), while
        /// `set-scene-cell` relies on that per-tick branch, so the two
        /// scenarios split the same cost across different phases.
        SceneAndClipLaunch,
    }

    fn project_92_ui_performance_probe_impl(probe: Project92UiProbe) {
        use super::*;
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use std::collections::HashSet;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        fn duration_ms(duration: Duration) -> f64 {
            duration.as_secs_f64() * 1000.0
        }

        fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
            MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }
        }

        let _dir = SequencerDirGuard::enter();
        let full_layout = matches!(
            probe,
            Project92UiProbe::StepInteractionsFullLayout
                | Project92UiProbe::InstrumentPlockKnobDrag
                | Project92UiProbe::ResponseCurveEditorDrag
                | Project92UiProbe::StepBufferParamDrag
                | Project92UiProbe::SceneAndClipLaunch
        );
        // The production layout packs seven tiles; 180x70 leaves the smaller
        // step-panel tile too short to keep all 64 step cells on screen, so
        // the full-layout variant runs at a larger cell viewport with the
        // same ~1250x850 production aspect.
        let (vp_cols, vp_rows): (u16, u16) = if full_layout { (220, 110) } else { (180, 70) };
        let project_name = if probe == Project92UiProbe::PianoholdSelection {
            "pianohold"
        } else {
            "92"
        };
        assert!(
            Path::new(&format!("projects/{project_name}.json")).exists(),
            "project {project_name} must be available at crates/sequencer/projects/{project_name}.json"
        );

        let eng = engine::init_headless_engine(44_100, 2).expect("initialize headless app graph");
        let lg_raw = eng.lg_ptr.0;
        let state = eng.state.clone();
        let lg_ptr = eng.lg_ptr;
        let sample_rate = eng.sample_rate;
        let _engine_guard = TestEngineGuard { lg_raw };
        let _audio_pump = HeadlessAudioPump::start(lg_ptr, eng.channels as usize);
        let master_recorder = eng.master_recorder.clone();
        // Kept so the step-buffer probe can assemble the real `SharedHandles`
        // and drive `dispatch_custom_host_command` instead of a hand-written
        // mirror of the handler's policy.
        let keyboard_tx = eng.keyboard_tx.clone();
        let mut app = app::App::new(
            state.clone(),
            lg_ptr,
            sample_rate,
            eng.buses,
            eng.master_recorder,
            eng.keyboard_tx,
        );

        let mut track_names = Vec::<String>::new();
        let track_pan_ids = Arc::new(Mutex::new(Vec::<i32>::new()));
        let track_collapsed = Arc::new(Mutex::new(app.track_collapsed.clone()));
        let bus_state = Arc::new(Mutex::new(app.buses.clone()));
        let bus_node_ids = Arc::new(Mutex::new(app.graph.bus_node_ids.clone()));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_tracks = Arc::new(Mutex::new(HashSet::<usize>::new()));
        let track_groups = Arc::new(Mutex::new(app.groups.clone()));
        let selected_steps = Arc::new(Mutex::new(HashSet::<usize>::new()));
        let selected_neural_neurons: sequencer::lisp_host::SharedSelectedNeuralNeurons =
            Arc::new(Mutex::new(BTreeSet::new()));
        let piano_roll_selection = Arc::new(Mutex::new(HashSet::<u64>::new()));
        let piano_roll_move_state = Arc::new(Mutex::new(None));
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        let fx_epoch = Arc::new(AtomicUsize::new(0));
        let fx_value_epoch = Arc::new(AtomicUsize::new(0));
        let ui_invalidations = Arc::new(UiInvalidationQueue::new());
        let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
        let recording = Arc::new(AtomicBool::new(false));
        let master_recording = Arc::new(AtomicBool::new(false));
        let record_armed = Arc::new(Mutex::new(Vec::<bool>::new()));
        let armed_rack: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let active_delete_target = Arc::new(Mutex::new(None));
        let active_delete_target_version = Arc::new(AtomicUsize::new(0));
        let auto_follow_override_until = Arc::new(Mutex::new(None));
        // Cloned (not moved) into `init_runtime` so the step-buffer probe can
        // hand the very same handles to a real `SharedHandles`.
        let piano_roll_focus = super::new_shared_piano_roll_focus();

        let RuntimeInit {
            runtime,
            accumulator_names,
            midi_fx_names: _,
            sample_browser,
            piano_roll_clipboard,
            selected_drum_lane_steps,
        } = init_runtime(
            &app,
            state.clone(),
            &track_names,
            track_pan_ids.clone(),
            track_collapsed.clone(),
            bus_state.clone(),
            bus_node_ids.clone(),
            current_track.clone(),
            selected_tracks.clone(),
            track_groups.clone(),
            selected_steps.clone(),
            piano_roll_selection.clone(),
            piano_roll_move_state.clone(),
            piano_roll_focus.clone(),
            recording.clone(),
            master_recording.clone(),
            master_recorder.clone(),
            record_armed.clone(),
            armed_rack.clone(),
            ui_epoch.clone(),
            fx_epoch.clone(),
            ui_invalidations.clone(),
            expanded_step_projection.clone(),
            selected_neural_neurons.clone(),
            active_delete_target.clone(),
            active_delete_target_version.clone(),
            auto_follow_override_until.clone(),
            lg_raw,
        );

        let mut editor = Editor::new(
            runtime,
            eseqlisp::EditorConfig {
                vim_mode: true,
                ..eseqlisp::EditorConfig::default()
            },
        );
        reload_custom_instrument_ui(&mut editor);
        let _ = editor.open_or_create_file_buffer(UI_ENTRYPOINT_PATH);
        let grid_source = editor.active_buffer().text();
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            Some(std::path::PathBuf::from(UI_ENTRYPOINT_PATH)),
            &grid_source,
            overlays,
        );
        assert!(
            report.success,
            "failed to load grid UI: {}",
            report.failure_message()
        );
        editor.process_lisp_reload_report(report);
        editor.refresh_runtime_side_effects();
        if full_layout {
            // Install the production multi-pane startup layout through the
            // exact helper the real app uses (editor_setup.rs), so the probe
            // layout cannot drift from what create_editor produces.
            super::editor_setup::apply_startup_grid_layout(&mut editor)
                .expect("apply production startup grid layout");
        }
        reload_custom_instrument_ui(&mut editor);
        editor.set_layout_viewport(vp_cols, vp_rows);
        editor.update_tile_rects(vp_cols, vp_rows);
        let _ = editor.drain_host_commands();

        app.queue_project_load_named(project_name)
            .expect("queue probe project load");
        for _ in 0..512 {
            if !app.has_pending_project_load() {
                break;
            }
            app.advance_pending_project_load()
                .expect("advance probe project load");
        }
        assert!(
            !app.has_pending_project_load(),
            "project {project_name} load did not finish"
        );
        assert!(
            app.state.scene_count() >= 2,
            "project {project_name} should have multiple scenes"
        );

        if probe == Project92UiProbe::ArrangedStepInteractions {
            // Commit an arrangement at realistic scale through the same edit
            // primitive the arrangement UI lowers to (`arr_replace` backs
            // def-song / capture commit): 18 scene-lane events and ~18 clips
            // per track referencing this project's real pool patterns.
            use sequencer::sequencer::{ArrClip, ProjectArrangement, SceneEvent};
            let pool_ids: Vec<Vec<u64>> = app.state.with_project_scenes(|scenes| {
                scenes
                    .track_pools
                    .iter()
                    .map(|pool| {
                        let mut ids: Vec<u64> = pool.patterns.keys().map(|id| id.0).collect();
                        ids.sort_unstable();
                        ids
                    })
                    .collect()
            });
            let track_count = app.tracks.len();
            let scene_count = app.state.scene_count().max(1);
            let mut arrangement = ProjectArrangement::new(track_count, 720.0);
            for event in 1..18 {
                arrangement.scene_lane.push(SceneEvent {
                    start_beat: event as f64 * 40.0,
                    scene: event % scene_count,
                });
            }
            for track in 0..track_count {
                let ids = &pool_ids[track];
                if ids.is_empty() {
                    continue;
                }
                for clip in 0..18 {
                    let id = arrangement
                        .allocate_clip_id()
                        .expect("allocate arranged fixture clip id");
                    let start_beat = clip as f64 * 40.0 + (track % 3) as f64 * 8.0;
                    arrangement.track_lanes[track].push(ArrClip::new(
                        id,
                        start_beat,
                        start_beat + 16.0,
                        Some(ids[clip % ids.len()]),
                    ));
                }
            }
            app.arr_replace(arrangement)
                .expect("commit arranged step-interaction fixture");
            assert!(
                app.state.committed_arrangement().is_some(),
                "arranged fixture must produce a committed arrangement"
            );
        }

        if probe == Project92UiProbe::SceneAndClipLaunch {
            // Grow every track's pattern pool to jungle-ology scale (~20
            // clips per track) through the real fork primitive — the mixer
            // clip grid, its per-cell glyph feeds, and the pool-wide
            // save-back work all scale with this count, and it is the
            // configuration where launches turn laggy in real projects.
            const POOL_CLIPS_PER_TRACK: usize = 20;
            let track_count = app.tracks.len();
            for track in 0..track_count {
                loop {
                    let pool_len = app
                        .state
                        .with_project_scenes(|scenes| scenes.track_pools[track].patterns.len());
                    if pool_len >= POOL_CLIPS_PER_TRACK {
                        break;
                    }
                    app.state
                        .fork_current_track_pattern(
                            track,
                            track_count,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        )
                        .unwrap_or_else(|| panic!("fork fixture pool clip for track {track}"));
                }
            }
            let pool_sizes: Vec<usize> = app.state.with_project_scenes(|scenes| {
                scenes
                    .track_pools
                    .iter()
                    .map(|pool| pool.patterns.len())
                    .collect()
            });
            assert!(
                pool_sizes.iter().all(|len| *len >= POOL_CLIPS_PER_TRACK),
                "launch fixture must grow every pool to {POOL_CLIPS_PER_TRACK} clips, got {pool_sizes:?}"
            );
        }

        if probe == Project92UiProbe::RackMacroDrag {
            app.load_rack_preset_onto_track(0, "rifton")
                .expect("load realistic instrument rack fixture");
            let slot = state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .expect("rack fixture track")
                .slots[0]
                .clone();
            let descriptor = app
                .rack_slot_instrument_descriptor(&slot)
                .expect("rack fixture instrument descriptor");
            let mappings = descriptor
                .params
                .iter()
                .enumerate()
                .take(3)
                .map(
                    |(param_index, param)| sequencer::sequencer::RackMacroMapping {
                        target: sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                            slot: 0,
                            param: param.name.clone(),
                            param_index,
                        },
                        range_min: param.min,
                        range_max: param.max,
                        curve: sequencer::sequencer::RackMacroCurve::Linear,
                    },
                )
                .collect::<Vec<_>>();
            assert_eq!(mappings.len(), 3, "rack fixture instrument parameters");
            state.update_rack_macros_for_all_pattern_snapshots(0, |macros| {
                macros[0].mappings = mappings.clone();
            });
        }

        current_track.store(0, Ordering::Relaxed);
        *track_pan_ids.lock().unwrap() = app
            .graph
            .track_node_ids
            .iter()
            .map(|ids| ids.pan_id)
            .collect();
        *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
        *record_armed.lock().unwrap() = vec![false; app.tracks.len()];
        sync_shared_track_collapsed(&track_collapsed, &app);
        push_project_scratch_to_named_buffer(&mut editor, &app);
        if let Err(error) = evaluate_project_scratch_on_ui_runtime(&mut editor, &app) {
            editor.handle_host_event(HostEvent::Status(format!("Scratch UI eval error: {error}")));
        }

        let cached_track_peak_levels = vec![0.0; app.tracks.len()];
        let cached_bus_peak_levels = read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
        let (cached_modulator_phases, cached_modulator_levels) =
            read_modulator_display_values(app.graph.lg, &app);

        {
            let rt = editor.runtime_mut();
            sync_project_state(rt, &app);
            sync_track_topology_state(
                rt,
                &app,
                &state,
                &mut track_names,
                0,
                &selected_steps,
                &piano_roll_selection,
                &accumulator_names,
                &record_armed,
                &cached_track_peak_levels,
            );
            rt.set_reactive(
                "SEQ",
                "selected-steps",
                build_selection_value(&selected_steps),
            );
            rt.set_reactive(
                "SEQ",
                "bus-effects",
                build_bus_effects_value_for_selection(&app, Some(&selected_steps)),
            );
            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
            sync_modulator_phase_fields(rt, &cached_modulator_phases);
            sync_modulator_level_fields(rt, &cached_modulator_levels);
            sync_mixer_delete_target_binding_fields(
                rt,
                app.tracks.len(),
                &state,
                active_delete_target.lock().unwrap().as_ref(),
            );
            rt.set_reactive(
                "SEQ",
                "delete-target-version",
                Value::Number(active_delete_target_version.load(Ordering::Relaxed) as f64),
            );
            rt.run_reactive_cycle();
        }
        editor.refresh_runtime_side_effects();
        refresh_visible_track_topology_layouts(&mut editor);
        editor.update_tile_rects(vp_cols, vp_rows);
        let _ = editor.drain_host_commands();

        if probe == Project92UiProbe::PianoholdSelection {
            const TRACK: usize = 0;
            const WARMUPS: usize = 5;
            const SAMPLES: usize = 20;
            let num_steps = state.pattern.track_params[TRACK].get_num_steps();
            assert!(
                num_steps >= 34,
                "pianohold track 0 must expose a full step grid (got {num_steps})"
            );

            let sequencer_buffer_id = editor
                .buffers
                .iter()
                .find(|buffer| buffer.name == "*sequencer*")
                .expect("sequencer buffer should exist")
                .id;
            editor.set_active_buffer(sequencer_buffer_id);
            sync_all_track_sequencer_state(
                editor.runtime_mut(),
                &state,
                &app,
                TRACK,
                &selected_steps,
            );
            // Real per-tick syncs, exactly like the arranged probe: pianohold
            // carries takes + use_arrangement, so this exercises the
            // take-pool-aware lane-event collection every frame.
            let mut song_frame = super::state_values::SongFrameState::default();
            let transport_visible = editor_has_visible_buffer(&editor, "*transport*");
            assert!(
                !editor_has_visible_buffer(&editor, "*arrangement*"),
                "pianohold probe must measure the Seq view"
            );
            app.sync_track_sound_bindings();
            super::state_values::sync_song_state(
                editor.runtime_mut(),
                &app,
                &mut song_frame,
                transport_visible,
            );
            assert!(
                song_frame
                    .cached_lanes
                    .as_ref()
                    .is_some_and(|lanes| lanes.iter().map(|lane| lane.len()).sum::<usize>() >= 100),
                "pianohold must publish its real clip lanes"
            );
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.update_tile_rects(180, 70);

            let initial_frame =
                eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);
            let initial_seq_frame = initial_frame
                .tiles
                .iter()
                .find(|tile| tile.frame.buffer_name == "*sequencer*")
                .expect("visible initial sequencer frame");
            let initial_layout = initial_seq_frame
                .frame
                .widget_layout
                .as_ref()
                .expect("initial sequencer layout");
            let viewport = eseqlisp::widget_render::WidgetViewport {
                cell_w: 8.0,
                cell_h: 16.0,
                vp_w: 1440.0,
                vp_h: 1120.0,
                time_seconds: 0.0,
                focused_widget_id: initial_seq_frame.frame.focused_widget_id,
                focused_branch: true,
                overlay_viewport_bottom: 70.0,
                scroll_top: initial_seq_frame.frame.widget_scroll_top
                    + initial_seq_frame.frame.text_scroll_top as f32,
                scroll_left: initial_seq_frame.frame.widget_layout_scroll_left,
                inherited_hover: false,
            };
            let (mut retained_runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                initial_layout,
                viewport,
                viewport.scroll_top,
                70,
            );
            let retained_run_indices =
                eseqlisp::widget_render::build_metal_primitive_run_index(&retained_runs);
            let step_clipboard = Arc::new(Mutex::new(None));
            let step_center = |editor: &mut Editor, step: usize| {
                let layout = editor.widget_layout().expect("sequencer layout");
                let cell = find_layout_node_by_stable_key_suffix(
                    &layout,
                    &format!("/step-cell-{TRACK}-{step}"),
                )
                .unwrap_or_else(|| panic!("visible sequencer step cell {step}"));
                (
                    cell.rect.col + cell.rect.width * 0.5,
                    cell.rect.row + cell.rect.height * 0.5,
                    layout.rect.width.ceil().max(1.0) as u16,
                    layout.rect.height.ceil().max(1.0) as u16,
                )
            };
            let apply_pending_step_commands = |editor: &mut Editor, app: &mut app::App| {
                for command in editor.drain_host_commands() {
                    let HostCommand::Custom { name, payload } = command else {
                        continue;
                    };
                    match name.as_str() {
                        "toggle-step" => {
                            let (outcome, track, step) =
                                apply_toggle_step_host_command(app, &payload)
                                    .expect("apply pianohold step toggle");
                            assert!(matches!(outcome, app::edit::EditOutcome::Applied(_)));
                            selected_steps.lock().unwrap().clear();
                            ui_invalidations.push(UiInvalidation::StepBatch {
                                track,
                                steps: vec![step],
                            });
                        }
                        other => panic!("unexpected pianohold host command {other}"),
                    }
                }
            };
            let neural = BTreeSet::new();
            let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
            let mixer_visible = editor_has_visible_buffer(&editor, "*mixer*");
            let mut finish_visible_update = |editor: &mut Editor, app: &mut app::App| {
                let invalidations = ui_invalidations.drain();
                if !invalidations.is_empty() {
                    apply_ui_invalidations(
                        invalidations,
                        UiInvalidationApplyCtx {
                            app,
                            editor,
                            state: &state,
                            track_collapsed: &track_collapsed,
                            bus_state: &bus_state,
                            current_track_idx: TRACK,
                            selected_steps: &selected_steps,
                            selected_neural_neurons: &neural,
                            piano_roll_selection: &piano_roll_selection,
                            accumulator_names: &accumulator_names,
                            cached_track_peak_levels: &cached_track_peak_levels,
                            cached_bus_peak_levels: &cached_bus_peak_levels,
                            record_armed: &record_armed,
                            active_delete_target: &active_delete_target,
                            active_delete_target_version: &active_delete_target_version,
                            expanded_step_projection: &expanded_step_projection,
                            fx_visible,
                            sequencer_visible: true,
                            mixer_visible,
                        },
                    );
                }
                app.sync_track_sound_bindings();
                super::state_values::sync_song_state(
                    editor.runtime_mut(),
                    app,
                    &mut song_frame,
                    transport_visible,
                );
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                let frame =
                    eseqlisp::frame::build_tiled_render_frame_borderless(editor, 180, 70);
                let seq_frame = frame
                    .tiles
                    .iter()
                    .find(|tile| tile.frame.buffer_name == "*sequencer*")
                    .expect("visible sequencer frame after pianohold action");
                let layout = seq_frame
                    .frame
                    .widget_layout
                    .as_ref()
                    .expect("sequencer layout after pianohold action");
                let (_, stats) =
                    eseqlisp::widget_render::refresh_metal_primitive_runs_retained_in_place(
                        layout,
                        viewport,
                        viewport.scroll_top,
                        70,
                        &mut retained_runs,
                        &retained_run_indices,
                        &seq_frame.frame.dirty_widget_ids,
                    );
                assert_eq!(stats.missing_previous_runs, 0);
                assert_eq!(stats.invalid_previous_runs, 0);
            };
            let percentile = |samples: &mut Vec<f64>, fraction: f64| {
                samples.sort_by(|a, b| a.total_cmp(b));
                let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
                samples[index]
            };
            let clear_selection = |editor: &mut Editor,
                                   app: &mut app::App,
                                   finish: &mut dyn FnMut(&mut Editor, &mut app::App)| {
                selected_steps.lock().unwrap().clear();
                ui_invalidations.push(UiInvalidation::StepSelection {
                    track: TRACK,
                    changed_steps: (0..num_steps).collect(),
                });
                finish(editor, app);
            };

            let mut cmd_a_samples = Vec::with_capacity(SAMPLES);
            let mut shift_range_samples = Vec::with_capacity(SAMPLES);
            let mut shift_range_dispatch = Vec::with_capacity(SAMPLES);
            let mut cmd_multi_samples = Vec::with_capacity(SAMPLES);
            let mut toggle_samples = Vec::with_capacity(SAMPLES);

            for iteration in 0..(WARMUPS + SAMPLES) {
                // (a) Cmd+A through the real shortcut path.
                clear_selection(&mut editor, &mut app, &mut finish_visible_update);
                let started = Instant::now();
                assert!(handle_metal_command_shortcut_with_ui_epoch(
                    &mut editor,
                    &crossterm::event::KeyEvent::new(
                        crossterm::event::KeyCode::Char('a'),
                        KeyModifiers::SUPER,
                    ),
                    &state,
                    &current_track,
                    &selected_steps,
                    &step_clipboard,
                    &ui_epoch,
                ));
                finish_visible_update(&mut editor, &mut app);
                if iteration >= WARMUPS {
                    cmd_a_samples.push(duration_ms(started.elapsed()));
                }
                assert_eq!(selected_steps.lock().unwrap().len(), num_steps);

                // (b) shift-click-drag range selection: arm with a real
                // shift-click on step 8, then time one drag tick to step 16.
                clear_selection(&mut editor, &mut app, &mut finish_visible_update);
                let (col, row, width, height) = step_center(&mut editor, 8);
                editor.handle_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column: col.floor() as u16,
                        row: row.floor() as u16,
                        modifiers: KeyModifiers::SHIFT,
                    },
                    0,
                    0,
                    width,
                    height,
                    col,
                    row,
                );
                apply_pending_step_commands(&mut editor, &mut app);
                finish_visible_update(&mut editor, &mut app);
                assert_eq!(*selected_steps.lock().unwrap(), HashSet::from([8]));
                let (target_col, target_row, _, _) = step_center(&mut editor, 16);
                let started = Instant::now();
                editor.handle_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::Drag(MouseButton::Left),
                        column: target_col.floor() as u16,
                        row: target_row.floor() as u16,
                        modifiers: KeyModifiers::SHIFT,
                    },
                    0,
                    0,
                    width,
                    height,
                    target_col,
                    target_row,
                );
                apply_pending_step_commands(&mut editor, &mut app);
                let dispatch_done = Instant::now();
                finish_visible_update(&mut editor, &mut app);
                if iteration >= WARMUPS {
                    shift_range_samples.push(duration_ms(started.elapsed()));
                    shift_range_dispatch.push(duration_ms(dispatch_done - started));
                }
                assert_eq!(
                    *selected_steps.lock().unwrap(),
                    (8..=16).collect::<HashSet<_>>(),
                    "shift drag must select the full 8..=16 range",
                );
                editor.handle_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::Up(MouseButton::Left),
                        column: target_col.floor() as u16,
                        row: target_row.floor() as u16,
                        modifiers: KeyModifiers::SHIFT,
                    },
                    0,
                    0,
                    width,
                    height,
                    target_col,
                    target_row,
                );

                // (c) cmd-click-drag multi-select: arm with a real cmd-click
                // on step 20, then time one drag tick onto step 21.
                clear_selection(&mut editor, &mut app, &mut finish_visible_update);
                let (col, row, width, height) = step_center(&mut editor, 20);
                editor.handle_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column: col.floor() as u16,
                        row: row.floor() as u16,
                        modifiers: KeyModifiers::SUPER,
                    },
                    0,
                    0,
                    width,
                    height,
                    col,
                    row,
                );
                apply_pending_step_commands(&mut editor, &mut app);
                finish_visible_update(&mut editor, &mut app);
                assert_eq!(*selected_steps.lock().unwrap(), HashSet::from([20]));
                let (target_col, target_row, _, _) = step_center(&mut editor, 21);
                let started = Instant::now();
                editor.handle_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::Drag(MouseButton::Left),
                        column: target_col.floor() as u16,
                        row: target_row.floor() as u16,
                        modifiers: KeyModifiers::SUPER,
                    },
                    0,
                    0,
                    width,
                    height,
                    target_col,
                    target_row,
                );
                apply_pending_step_commands(&mut editor, &mut app);
                finish_visible_update(&mut editor, &mut app);
                if iteration >= WARMUPS {
                    cmd_multi_samples.push(duration_ms(started.elapsed()));
                }
                assert_eq!(
                    *selected_steps.lock().unwrap(),
                    HashSet::from([20, 21]),
                    "cmd drag must add the dragged-over step to the selection",
                );
                editor.handle_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::Up(MouseButton::Left),
                        column: target_col.floor() as u16,
                        row: target_row.floor() as u16,
                        modifiers: KeyModifiers::SUPER,
                    },
                    0,
                    0,
                    width,
                    height,
                    target_col,
                    target_row,
                );

                // (d) toggle drag onto an empty step, as in the 92 probe.
                selected_steps.lock().unwrap().clear();
                state.pattern.patterns[TRACK].set_step_active(32, false);
                state.pattern.patterns[TRACK].set_step_active(33, false);
                ui_invalidations.push(UiInvalidation::StepBatch {
                    track: TRACK,
                    steps: vec![32, 33],
                });
                finish_visible_update(&mut editor, &mut app);
                let (start_col, start_row, width, height) = step_center(&mut editor, 32);
                let (target_col, target_row, _, _) = step_center(&mut editor, 33);
                editor.handle_mouse_precise(
                    mouse_event(
                        MouseEventKind::Down(MouseButton::Left),
                        start_col.floor() as u16,
                        start_row.floor() as u16,
                    ),
                    0,
                    0,
                    width,
                    height,
                    start_col,
                    start_row,
                );
                apply_pending_step_commands(&mut editor, &mut app);
                finish_visible_update(&mut editor, &mut app);
                let started = Instant::now();
                editor.handle_mouse_precise(
                    mouse_event(
                        MouseEventKind::Drag(MouseButton::Left),
                        target_col.floor() as u16,
                        target_row.floor() as u16,
                    ),
                    0,
                    0,
                    width,
                    height,
                    target_col,
                    target_row,
                );
                apply_pending_step_commands(&mut editor, &mut app);
                finish_visible_update(&mut editor, &mut app);
                if iteration >= WARMUPS {
                    toggle_samples.push(duration_ms(started.elapsed()));
                }
                assert!(state.pattern.patterns[TRACK].is_active(33));
                editor
                    .runtime_mut()
                    .eval_str("(eseq.sequencer/grid-step-pointer-up 0 33 (dict :sx -1))")
                    .expect("finish pianohold toggle drag");
                state.pattern.patterns[TRACK].set_step_active(32, false);
                state.pattern.patterns[TRACK].set_step_active(33, false);
                ui_invalidations.push(UiInvalidation::StepBatch {
                    track: TRACK,
                    steps: vec![32, 33],
                });
                finish_visible_update(&mut editor, &mut app);
            }

            eprintln!(
                "[pianohold-step-shift-range-dispatch] median_ms={:.3}",
                percentile(&mut shift_range_dispatch, 0.50),
            );
            // Quiet-machine medians on 2026-07-28: cmd-a 0.32, shift-range
            // 3.5, cmd-multi 0.25, toggle 0.68 ms. The shift-range tick jumps
            // 8 cells in one event and the box-pointer drag-segment walk
            // dispatches once per crossed cell (~0.35 ms/cell — the same
            // per-cell cost as a single-cell tick), so its ceiling reflects
            // that linearity, not a defect.
            for (name, samples, ceiling_ms) in [
                ("cmd-a", &mut cmd_a_samples, 1.5_f64),
                ("shift-range-tick", &mut shift_range_samples, 8.0),
                ("cmd-multi-tick", &mut cmd_multi_samples, 1.5),
                ("toggle-drag", &mut toggle_samples, 3.0),
            ] {
                let median = percentile(samples, 0.50);
                eprintln!(
                    "[pianohold-step-{name}] tracks={} steps={num_steps} samples={SAMPLES} median_ms={:.3} p95_ms={:.3}",
                    app.tracks.len(),
                    median,
                    percentile(samples, 0.95),
                );
                assert!(
                    median <= ceiling_ms,
                    "{name} median {median:.3} ms exceeded the {ceiling_ms:.1} ms ceiling on the pianohold fixture",
                );
            }
            return;
        }

        // Drag probes under the production multi-pane layout: an instrument
        // knob (with and without a selected step, i.e. p-lock writes) and the
        // response-curve-editor versus a plain knob on the same params.
        //
        // Unlike the selection probes, a device drag bumps `ui_epoch` /
        // `fx_epoch`, so the visible-update helper below also replays the
        // reactive tick's epoch-driven resyncs (reactive_tick.rs) — skipping
        // them would hide most of the per-drag cost.
        if matches!(
            probe,
            Project92UiProbe::InstrumentPlockKnobDrag | Project92UiProbe::ResponseCurveEditorDrag
        ) {
            const TRACK: usize = 0;
            const STEP_COUNT: usize = 64;
            const WARMUPS: usize = 5;
            const SAMPLES: usize = 20;
            let curve_probe = probe == Project92UiProbe::ResponseCurveEditorDrag;
            let probe_prefix = if curve_probe {
                "project-92-fullayout-curve"
            } else {
                "project-92-fullayout-plock-knob"
            };

            let sequencer_buffer_idx = editor
                .buffers
                .iter()
                .position(|buffer| buffer.name == "*sequencer*")
                .expect("sequencer buffer index");
            let sequencer_tile = editor
                .tile_root
                .leaf_ids()
                .into_iter()
                .find(|tile_id| {
                    editor
                        .tile_root
                        .find_leaf(*tile_id)
                        .is_some_and(|leaf| leaf.buffer_idx == sequencer_buffer_idx)
                })
                .expect("the production layout must show *sequencer*");
            editor.switch_active_tile(sequencer_tile);
            assert_eq!(
                editor.active_buffer().name,
                "*sequencer*",
                "the drag probes start focused on the sequencer tile"
            );

            state.pattern.track_params[TRACK].set_num_steps(STEP_COUNT);
            for step in 0..STEP_COUNT {
                state.pattern.patterns[TRACK].set_step_active(step, step < 24);
            }

            // Probe B needs a builtin whose panel hosts a
            // response-curve-editor; the Filter panel (filter-panel.lisp) is
            // that panel, and its `cut` knob writes the same param the curve
            // drag does, so the two medians compare like for like. Project 92
            // already ships a Filter on track 0 — reuse it rather than
            // stacking a second one onto the real chain.
            let filter_slot = if curve_probe {
                let existing = app
                    .graph
                    .effect_descriptors
                    .get(TRACK)
                    .and_then(|slots| {
                        slots
                            .iter()
                            .position(|desc| desc.name == "Filter")
                    });
                match existing {
                    Some(slot) => Some(slot),
                    None => {
                        let slot = app
                            .add_builtin_effect_sync(TRACK, "Filter")
                            .expect("install builtin Filter fixture on track 0");
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        Some(slot)
                    }
                }
            } else {
                None
            };

            sync_all_track_sequencer_state(
                editor.runtime_mut(),
                &state,
                &app,
                TRACK,
                &selected_steps,
            );

            let transport_visible = editor_has_visible_buffer(&editor, "*transport*");
            let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
            let mixer_visible = editor_has_visible_buffer(&editor, "*mixer*");
            assert!(fx_visible, "production layout must show the *fx* panel");
            assert!(mixer_visible, "production layout must show the *mixer* strip");
            assert!(
                transport_visible,
                "production layout must show the *transport* bar"
            );
            assert!(
                !editor_has_visible_buffer(&editor, "*arrangement*"),
                "the drag probes must measure the Seq view"
            );

            {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    build_effects_value(
                        &state,
                        TRACK,
                        &app.graph.effect_descriptors,
                        &selected_steps,
                    ),
                );
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, TRACK, &selected_steps),
                );
            }
            sync_fx_param_binding_fields_with_neural_selection(
                editor.runtime_mut(),
                &app,
                &state,
                TRACK,
                &selected_steps,
                None,
            );

            let mut song_frame = super::state_values::SongFrameState::default();
            app.sync_track_sound_bindings();
            super::state_values::sync_song_state(
                editor.runtime_mut(),
                &app,
                &mut song_frame,
                transport_visible,
            );
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.update_tile_rects(vp_cols, vp_rows);

            let initial_frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                &mut editor,
                vp_cols as usize,
                vp_rows as usize,
            );
            let visible_buffers: Vec<String> = initial_frame
                .tiles
                .iter()
                .map(|tile| tile.frame.buffer_name.clone())
                .collect();
            for required in ["*sequencer*", "*fx*", "*mixer*", "*transport*"] {
                assert!(
                    visible_buffers.iter().any(|name| name == required),
                    "the production layout must show {required}, got {visible_buffers:?}"
                );
            }
            eprintln!("[{probe_prefix}-visible-buffers] {visible_buffers:?}");

            struct TileRetained {
                buffer_name: String,
                viewport: eseqlisp::widget_render::WidgetViewport,
                runs: Vec<eseqlisp::widget_render::MetalPrimitiveRun>,
                indices: eseqlisp::widget_render::MetalPrimitiveRunIndex,
            }
            let mut tile_retained: Vec<TileRetained> = Vec::new();
            for tile in &initial_frame.tiles {
                let layout = tile.frame.widget_layout.as_ref().unwrap_or_else(|| {
                    panic!(
                        "visible tile {} must have a widget layout",
                        tile.frame.buffer_name
                    )
                });
                let viewport = eseqlisp::widget_render::WidgetViewport {
                    cell_w: 8.0,
                    cell_h: 16.0,
                    vp_w: vp_cols as f32 * 8.0,
                    vp_h: vp_rows as f32 * 16.0,
                    time_seconds: 0.0,
                    focused_widget_id: tile.frame.focused_widget_id,
                    focused_branch: tile.is_active,
                    overlay_viewport_bottom: vp_rows as f32,
                    scroll_top: tile.frame.widget_scroll_top + tile.frame.text_scroll_top as f32,
                    scroll_left: tile.frame.widget_layout_scroll_left,
                    inherited_hover: false,
                };
                let (runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                    layout,
                    viewport,
                    viewport.scroll_top,
                    vp_rows,
                );
                let indices = eseqlisp::widget_render::build_metal_primitive_run_index(&runs);
                tile_retained.push(TileRetained {
                    buffer_name: tile.frame.buffer_name.clone(),
                    viewport,
                    runs,
                    indices,
                });
            }

            // Screen-space origin of the *fx* tile's content, so drags go
            // through `handle_tiled_mouse_precise` exactly like the real
            // event loop (event_loop.rs) instead of a tile-local shortcut.
            let fx_tile = initial_frame
                .tiles
                .iter()
                .find(|tile| tile.frame.buffer_name == "*fx*")
                .expect("visible fx tile");
            let fx_origin_col = fx_tile.body_rect.col.floor();
            let fx_origin_row = fx_tile.body_rect.row.floor();
            let fx_scroll_top =
                fx_tile.frame.widget_scroll_top + fx_tile.frame.text_scroll_top as f32;
            let fx_scroll_left = fx_tile.frame.widget_layout_scroll_left;
            let fx_body_rect = fx_tile.body_rect;

            // Discover the real drag targets in the fx tile layout.
            let fx_layout = fx_tile
                .frame
                .widget_layout
                .as_ref()
                .expect("fx tile layout")
                .clone();
            if std::env::var_os("ESEQ_PROBE_DUMP").is_some() {
                let mut nodes = Vec::new();
                collect_layout_nodes(
                    &fx_layout,
                    &mut |node| {
                        matches!(
                            node.widget_type.as_str(),
                            "knob-number" | "response-curve-editor"
                        )
                    },
                    &mut nodes,
                );
                for node in &nodes {
                    eprintln!(
                        "[{probe_prefix}-dump] type={} key={:?} rect=({:.2},{:.2},{:.2},{:.2})",
                        node.widget_type,
                        node.stable_key,
                        node.rect.col,
                        node.rect.row,
                        node.rect.width,
                        node.rect.height,
                    );
                }
                let mut keyed = Vec::new();
                collect_layout_nodes(
                    &fx_layout,
                    &mut |node| {
                        node.stable_key
                            .as_deref()
                            .is_some_and(|key| key.starts_with("sampler-param-") || key.starts_with("builtin-fx-"))
                    },
                    &mut keyed,
                );
                for node in &keyed {
                    eprintln!(
                        "[{probe_prefix}-dump-key] key={:?} type={} rect=({:.2},{:.2})",
                        node.stable_key, node.widget_type, node.rect.col, node.rect.row
                    );
                }
            }

            // The knob target: for the p-lock probe an instrument knob in the
            // sampler panel; for the curve probe the Filter's `cut` knob,
            // which writes the same param the curve drag does.
            let knob_key = {
                let mut nodes = Vec::new();
                collect_layout_nodes(
                    &fx_layout,
                    &mut |node| {
                        node.widget_type == "knob-number"
                            && node.stable_key.as_deref().is_some_and(|key| {
                                if curve_probe {
                                    key.starts_with("builtin-fx-cut-knob-")
                                } else {
                                    key.starts_with("sampler-param-")
                                }
                            })
                    },
                    &mut nodes,
                );
                nodes
                    .iter()
                    .find(|node| {
                        let center_col = fx_origin_col + node.rect.col + node.rect.width * 0.5
                            - fx_scroll_left;
                        let center_row = fx_origin_row + node.rect.row + node.rect.height * 0.5
                            - fx_scroll_top;
                        center_col >= fx_body_rect.col
                            && center_col < fx_body_rect.col + fx_body_rect.width
                            && center_row >= fx_body_rect.row
                            && center_row < fx_body_rect.row + fx_body_rect.height
                    })
                    .and_then(|node| node.stable_key.clone())
                    .unwrap_or_else(|| {
                        panic!(
                            "no on-screen knob target found in the fx tile (candidates={:?})",
                            nodes
                                .iter()
                                .map(|node| node.stable_key.clone())
                                .collect::<Vec<_>>()
                        )
                    })
            };
            eprintln!("[{probe_prefix}-knob-target] stable_key={knob_key}");
            let _curve_key = if curve_probe {
                let mut nodes = Vec::new();
                collect_layout_nodes(
                    &fx_layout,
                    &mut |node| node.widget_type == "response-curve-editor",
                    &mut nodes,
                );
                let node = nodes
                    .first()
                    .expect("the Filter panel must render a response-curve-editor");
                let key = node
                    .stable_key
                    .clone()
                    .unwrap_or_else(|| "response-curve-editor".to_string());
                eprintln!(
                    "[{probe_prefix}-curve-target] stable_key={key:?} rect=({:.2},{:.2},{:.2},{:.2})",
                    node.rect.col, node.rect.row, node.rect.width, node.rect.height
                );
                Some(key)
            } else {
                None
            };

            // --- visible update: the real reactive tick, minus the render --
            let neural = selected_neural_neurons.lock().unwrap().clone();
            let mut prev_ui_epoch = ui_epoch.load(Ordering::Relaxed);
            let mut prev_fx_epoch = fx_epoch.load(Ordering::Relaxed);
            let mut track_param_sync_revision: Option<super::loop_ctx::ParamSyncRevision> = None;
            let mut fx_param_sync_revision: Option<super::loop_ctx::ParamSyncRevision> = None;

            struct DragUpdate {
                tick_sync_ms: f64,
                invalidation_ms: f64,
                epoch_sync_ms: f64,
                reactive_ms: f64,
                frame_ms: f64,
                retained_ms: f64,
                // epoch-sync sub-phases (reactive_tick's ui/fx epoch branches)
                epoch_seq_state_ms: f64,
                epoch_track_params_ms: f64,
                epoch_fx_bindings_ms: f64,
                epoch_piano_ms: f64,
                epoch_fx_values_ms: f64,
                // reactive sub-phases
                reactive_cycle_ms: f64,
                side_effects_ms: f64,
                seq_layout_refresh_ms: f64,
                // inactive-tile layout refreshes the reactive cycle's side
                // effects triggered during this update
                layout_refresh_ms: f64,
                layout_refresh_count: usize,
                ui_epoch_fired: bool,
                fx_epoch_fired: bool,
                tiles: Vec<(String, usize, f64, bool)>,
            }

            let mut finish_visible_update = |editor: &mut Editor,
                                             app: &mut app::App,
                                             tiles: &mut Vec<TileRetained>|
             -> DragUpdate {
                let started = Instant::now();
                // reactive_tick.rs order: sound bindings + song state, then
                // typed invalidations, then the epoch-driven resyncs.
                app.sync_track_sound_bindings();
                super::state_values::sync_song_state(
                    editor.runtime_mut(),
                    app,
                    &mut song_frame,
                    transport_visible,
                );
                let tick_sync_done = Instant::now();
                let invalidations = ui_invalidations.drain();
                if !invalidations.is_empty() {
                    apply_ui_invalidations(
                        invalidations,
                        UiInvalidationApplyCtx {
                            app,
                            editor,
                            state: &state,
                            track_collapsed: &track_collapsed,
                            bus_state: &bus_state,
                            current_track_idx: TRACK,
                            selected_steps: &selected_steps,
                            selected_neural_neurons: &neural,
                            piano_roll_selection: &piano_roll_selection,
                            accumulator_names: &accumulator_names,
                            cached_track_peak_levels: &cached_track_peak_levels,
                            cached_bus_peak_levels: &cached_bus_peak_levels,
                            record_armed: &record_armed,
                            active_delete_target: &active_delete_target,
                            active_delete_target_version: &active_delete_target_version,
                            expanded_step_projection: &expanded_step_projection,
                            fx_visible,
                            sequencer_visible: true,
                            mixer_visible,
                        },
                    );
                }
                let invalidations_done = Instant::now();
                let mut epoch_seq_state_ms = 0.0;
                let mut epoch_track_params_ms = 0.0;
                let mut epoch_fx_bindings_ms = 0.0;
                let mut epoch_piano_ms = 0.0;
                let mut epoch_fx_values_ms = 0.0;

                // --- reactive_tick.rs ui_epoch / fx_epoch branches ---------
                let ui_ep = ui_epoch.load(Ordering::Relaxed);
                let fx_ep = fx_epoch.load(Ordering::Relaxed);
                let ui_epoch_fired = ui_ep != prev_ui_epoch;
                let fx_epoch_fired = fx_visible && fx_ep != prev_fx_epoch;
                if ui_epoch_fired {
                    let mut sorted_steps: Vec<usize> =
                        selected_steps.lock().unwrap().iter().copied().collect();
                    sorted_steps.sort_unstable();
                    let revision = super::loop_ctx::ParamSyncRevision {
                        track: TRACK,
                        scene: state.current_scene_index(),
                        pattern_epoch: state.transport.pattern_epoch.load(Ordering::Relaxed),
                        song_row_mirror_epoch: app.song_row_mirror_epoch,
                        ui_epoch: ui_ep,
                        fx_epoch: fx_ep,
                        sound_binding_epoch: app.sound_binding_epoch,
                        display_step: displayed_plock_step(
                            &state,
                            TRACK,
                            sorted_steps.first().copied(),
                        ),
                        selected_steps: sorted_steps,
                        selected_neural_neurons: neural.iter().copied().collect(),
                    };
                    sync_shared_track_collapsed(&track_collapsed, app);
                    {
                        let rt = editor.runtime_mut();
                        sync_macro_state(rt, app);
                        sync_track_name_state(rt, &mut track_names, app);
                        rt.set_reactive("SEQ", "steps", build_steps_value(&state, TRACK));
                        sync_step_param_lists(rt, &state, TRACK);
                        let phase = Instant::now();
                        sync_all_track_sequencer_state(rt, &state, app, TRACK, &selected_steps);
                        let _ = sync_all_expanded_step_viewports(
                            rt,
                            &state,
                            app,
                            &selected_steps,
                            TRACK,
                            &expanded_step_projection,
                        );
                        epoch_seq_state_ms = duration_ms(phase.elapsed());
                        sync_track_mixer_state(rt, app, &state);
                        sync_bus_mixer_state(rt, app);
                        sync_track_peak_fields(rt, &cached_track_peak_levels);
                        sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                        *accumulator_names.lock().unwrap() = build_accumulator_names(app);
                        let phase = Instant::now();
                        if super::reactive_tick::claim_param_sync_revision(
                            &mut track_param_sync_revision,
                            &revision,
                        ) {
                            sync_track_params_with_neural_selection(
                                rt,
                                app,
                                &state,
                                TRACK,
                                &selected_steps,
                                Some(&neural),
                            );
                        }
                        epoch_track_params_ms = duration_ms(phase.elapsed());
                        let _ = sync_track_plock_variant_preview(
                            rt,
                            app,
                            &state,
                            TRACK,
                            &selected_steps,
                            None,
                        );
                        let phase = Instant::now();
                        if super::reactive_tick::claim_param_sync_revision(
                            &mut fx_param_sync_revision,
                            &revision,
                        ) {
                            let _ = sync_fx_param_binding_fields_with_neural_selection(
                                rt,
                                app,
                                &state,
                                TRACK,
                                &selected_steps,
                                Some(&neural),
                            );
                        }
                        epoch_fx_bindings_ms = duration_ms(phase.elapsed());
                        rt.set_reactive(
                            "SEQ",
                            "selected-steps",
                            build_selection_value(&selected_steps),
                        );
                        let phase = Instant::now();
                        sync_piano_roll_state(rt, app, &state, TRACK, &piano_roll_selection);
                        epoch_piano_ms = duration_ms(phase.elapsed());
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(&state, TRACK, &app.graph.effect_descriptors),
                        );
                        sync_mixer_delete_target_binding_fields(
                            rt,
                            app.tracks.len(),
                            &state,
                            active_delete_target.lock().unwrap().as_ref(),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "record-armed",
                            build_record_armed_value(&record_armed.lock().unwrap()),
                        );
                    }
                    prev_ui_epoch = ui_ep;
                }
                if fx_epoch_fired {
                    let phase = Instant::now();
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "effects",
                        build_effects_value(
                            &state,
                            TRACK,
                            &app.graph.effect_descriptors,
                            &selected_steps,
                        ),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "midi-effects",
                        build_midi_effects_value(&state, TRACK, &selected_steps),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "instrument-panel",
                        build_instrument_panel_value(app, TRACK, &selected_steps),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "step-has-plocks",
                        build_step_has_plocks(&state, TRACK, &app.graph.effect_descriptors),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "bus-effects",
                        build_bus_effects_value_for_selection(app, Some(&selected_steps)),
                    );
                    prev_fx_epoch = fx_ep;
                    epoch_fx_values_ms = duration_ms(phase.elapsed());
                }
                let epoch_sync_done = Instant::now();

                editor.runtime_mut().run_reactive_cycle();
                let cycle_done = Instant::now();
                editor.refresh_runtime_side_effects();
                let side_effects_done = Instant::now();
                // `refresh_runtime_side_effects` clears this vector on entry,
                // so what it holds now is exactly this update's inactive-tile
                // layout refresh work.
                let (layout_refresh_ms, layout_refresh_count) = {
                    let timings = editor.last_layout_refresh_timings();
                    (
                        timings
                            .iter()
                            .map(|timing| timing.elapsed.as_secs_f64() * 1000.0)
                            .sum::<f64>(),
                        timings.len(),
                    )
                };
                if ui_epoch_fired {
                    // reactive_tick.rs refreshes the sequencer tile's layout
                    // after the cycle whenever the ui epoch fired.
                    editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                }
                let reactive_done = Instant::now();
                let frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                    editor,
                    vp_cols as usize,
                    vp_rows as usize,
                );
                let frame_done = Instant::now();
                assert_eq!(
                    frame.tiles.len(),
                    tiles.len(),
                    "the production layout must keep every tile visible"
                );
                let mut tile_stats = Vec::with_capacity(frame.tiles.len());
                for tile in &frame.tiles {
                    let entry = tiles
                        .iter_mut()
                        .find(|entry| entry.buffer_name == tile.frame.buffer_name)
                        .unwrap_or_else(|| {
                            panic!("retained runs for visible tile {}", tile.frame.buffer_name)
                        });
                    let layout = tile.frame.widget_layout.as_ref().unwrap_or_else(|| {
                        panic!(
                            "visible tile {} must keep a widget layout",
                            tile.frame.buffer_name
                        )
                    });
                    let tile_started = Instant::now();
                    let (_, stats) =
                        eseqlisp::widget_render::refresh_metal_primitive_runs_retained_in_place(
                            layout,
                            entry.viewport,
                            entry.viewport.scroll_top,
                            vp_rows,
                            &mut entry.runs,
                            &entry.indices,
                            &tile.frame.dirty_widget_ids,
                        );
                    let structural_rebuild =
                        stats.missing_previous_runs > 0 || stats.invalid_previous_runs > 0;
                    if structural_rebuild {
                        // Production fallback (metal_backend.rs): a tile whose
                        // widget structure changed rebuilds its whole run
                        // scene, and that cost stays inside the timed region.
                        let (runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                            layout,
                            entry.viewport,
                            entry.viewport.scroll_top,
                            vp_rows,
                        );
                        entry.indices =
                            eseqlisp::widget_render::build_metal_primitive_run_index(&runs);
                        entry.runs = runs;
                    }
                    tile_stats.push((
                        tile.frame.buffer_name.clone(),
                        tile.frame.dirty_widget_ids.len(),
                        duration_ms(tile_started.elapsed()),
                        structural_rebuild,
                    ));
                }
                let retained_done = Instant::now();
                DragUpdate {
                    tick_sync_ms: duration_ms(tick_sync_done - started),
                    invalidation_ms: duration_ms(invalidations_done - tick_sync_done),
                    epoch_sync_ms: duration_ms(epoch_sync_done - invalidations_done),
                    reactive_ms: duration_ms(reactive_done - epoch_sync_done),
                    frame_ms: duration_ms(frame_done - reactive_done),
                    retained_ms: duration_ms(retained_done - frame_done),
                    epoch_seq_state_ms,
                    epoch_track_params_ms,
                    epoch_fx_bindings_ms,
                    epoch_piano_ms,
                    epoch_fx_values_ms,
                    reactive_cycle_ms: duration_ms(cycle_done - epoch_sync_done),
                    side_effects_ms: duration_ms(side_effects_done - cycle_done),
                    seq_layout_refresh_ms: duration_ms(reactive_done - side_effects_done),
                    layout_refresh_ms,
                    layout_refresh_count,
                    ui_epoch_fired,
                    fx_epoch_fired,
                    tiles: tile_stats,
                }
            };

            let percentile = |samples: &mut Vec<f64>, fraction: f64| {
                samples.sort_by(|a, b| a.total_cmp(b));
                let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
                samples[index]
            };

            struct DragSamples {
                total: Vec<f64>,
                dispatch: Vec<f64>,
                host: Vec<f64>,
                tick_sync: Vec<f64>,
                invalidation: Vec<f64>,
                epoch_sync: Vec<f64>,
                reactive: Vec<f64>,
                frame: Vec<f64>,
                retained: Vec<f64>,
                epoch_seq_state: Vec<f64>,
                epoch_track_params: Vec<f64>,
                epoch_fx_bindings: Vec<f64>,
                epoch_piano: Vec<f64>,
                epoch_fx_values: Vec<f64>,
                reactive_cycle: Vec<f64>,
                side_effects: Vec<f64>,
                seq_layout_refresh: Vec<f64>,
                layout_refresh: Vec<f64>,
                layout_refresh_count: Vec<f64>,
                ui_epoch_updates: usize,
                fx_epoch_updates: usize,
                tile_retained: std::collections::BTreeMap<String, Vec<f64>>,
                tile_rebuilds: std::collections::BTreeMap<String, usize>,
                tile_dirty: std::collections::BTreeMap<String, usize>,
            }
            impl DragSamples {
                fn new() -> Self {
                    Self {
                        total: Vec::new(),
                        dispatch: Vec::new(),
                        host: Vec::new(),
                        tick_sync: Vec::new(),
                        invalidation: Vec::new(),
                        epoch_sync: Vec::new(),
                        reactive: Vec::new(),
                        frame: Vec::new(),
                        retained: Vec::new(),
                        epoch_seq_state: Vec::new(),
                        epoch_track_params: Vec::new(),
                        epoch_fx_bindings: Vec::new(),
                        epoch_piano: Vec::new(),
                        epoch_fx_values: Vec::new(),
                        reactive_cycle: Vec::new(),
                        side_effects: Vec::new(),
                        seq_layout_refresh: Vec::new(),
                        layout_refresh: Vec::new(),
                        layout_refresh_count: Vec::new(),
                        ui_epoch_updates: 0,
                        fx_epoch_updates: 0,
                        tile_retained: std::collections::BTreeMap::new(),
                        tile_rebuilds: std::collections::BTreeMap::new(),
                        tile_dirty: std::collections::BTreeMap::new(),
                    }
                }
                fn record(
                    &mut self,
                    total_ms: f64,
                    dispatch_ms: f64,
                    host_ms: f64,
                    update: &DragUpdate,
                ) {
                    self.total.push(total_ms);
                    self.dispatch.push(dispatch_ms);
                    self.host.push(host_ms);
                    self.tick_sync.push(update.tick_sync_ms);
                    self.invalidation.push(update.invalidation_ms);
                    self.epoch_sync.push(update.epoch_sync_ms);
                    self.reactive.push(update.reactive_ms);
                    self.frame.push(update.frame_ms);
                    self.retained.push(update.retained_ms);
                    self.epoch_seq_state.push(update.epoch_seq_state_ms);
                    self.epoch_track_params.push(update.epoch_track_params_ms);
                    self.epoch_fx_bindings.push(update.epoch_fx_bindings_ms);
                    self.epoch_piano.push(update.epoch_piano_ms);
                    self.epoch_fx_values.push(update.epoch_fx_values_ms);
                    self.reactive_cycle.push(update.reactive_cycle_ms);
                    self.side_effects.push(update.side_effects_ms);
                    self.seq_layout_refresh.push(update.seq_layout_refresh_ms);
                    self.layout_refresh.push(update.layout_refresh_ms);
                    self.layout_refresh_count
                        .push(update.layout_refresh_count as f64);
                    if update.ui_epoch_fired {
                        self.ui_epoch_updates += 1;
                    }
                    if update.fx_epoch_fired {
                        self.fx_epoch_updates += 1;
                    }
                    for (name, dirty, retained_ms, rebuilt) in &update.tiles {
                        self.tile_retained
                            .entry(name.clone())
                            .or_default()
                            .push(*retained_ms);
                        *self.tile_dirty.entry(name.clone()).or_default() += *dirty;
                        if *rebuilt {
                            *self.tile_rebuilds.entry(name.clone()).or_default() += 1;
                        }
                    }
                }
            }

            let step_center = |editor: &mut Editor, step: usize| {
                let layout = editor.widget_layout().expect("sequencer layout");
                let cell = find_layout_node_by_stable_key_suffix(
                    &layout,
                    &format!("/step-cell-{TRACK}-{step}"),
                )
                .unwrap_or_else(|| panic!("visible sequencer step cell {step}"));
                (
                    cell.rect.col + cell.rect.width * 0.5,
                    cell.rect.row + cell.rect.height * 0.5,
                    layout.rect.width.ceil().max(1.0) as u16,
                    layout.rect.height.ceil().max(1.0) as u16,
                )
            };

            let step_clipboard = Arc::new(Mutex::new(None));
            struct ScenarioResult {
                label: String,
                median_ms: f64,
                dispatch_ms: f64,
                ui_epoch_updates: usize,
                fx_epoch_updates: usize,
            }
            let mut scenario_results: Vec<ScenarioResult> = Vec::new();

            // (label, selected steps, drag the curve instead of the knob)
            let scenarios: Vec<(&str, usize, bool)> = if curve_probe {
                vec![("curve-drag", 0, true), ("knob-baseline", 0, false)]
            } else {
                vec![
                    ("knob-no-selection", 0, false),
                    ("knob-1-step-plock", 1, false),
                    ("knob-64-step-plock", STEP_COUNT, false),
                ]
            };

            for (label, selection_size, drag_curve) in scenarios {
                // --- establish the selection through the real gestures -----
                editor.switch_active_tile(sequencer_tile);
                selected_steps.lock().unwrap().clear();
                ui_invalidations.push(UiInvalidation::StepSelection {
                    track: TRACK,
                    changed_steps: (0..STEP_COUNT).collect(),
                });
                finish_visible_update(&mut editor, &mut app, &mut tile_retained);
                if selection_size == STEP_COUNT {
                    assert!(handle_metal_command_shortcut_with_ui_epoch(
                        &mut editor,
                        &crossterm::event::KeyEvent::new(
                            crossterm::event::KeyCode::Char('a'),
                            KeyModifiers::SUPER,
                        ),
                        &state,
                        &current_track,
                        &selected_steps,
                        &step_clipboard,
                        &ui_epoch,
                    ));
                } else if selection_size == 1 {
                    let (col, row, width, height) = step_center(&mut editor, 8);
                    editor.handle_mouse_precise(
                        MouseEvent {
                            kind: MouseEventKind::Down(MouseButton::Left),
                            column: col.floor() as u16,
                            row: row.floor() as u16,
                            modifiers: KeyModifiers::SHIFT,
                        },
                        0,
                        0,
                        width,
                        height,
                        col,
                        row,
                    );
                    let _ = editor.drain_host_commands();
                }
                finish_visible_update(&mut editor, &mut app, &mut tile_retained);
                assert_eq!(
                    selected_steps.lock().unwrap().len(),
                    selection_size,
                    "{label}: real selection gesture must select {selection_size} steps"
                );

                // --- locate the drag target in the freshly built fx layout --
                let target_rect = {
                    let frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                        &mut editor,
                        vp_cols as usize,
                        vp_rows as usize,
                    );
                    let fx = frame
                        .tiles
                        .iter()
                        .find(|tile| tile.frame.buffer_name == "*fx*")
                        .expect("visible fx tile");
                    let layout = fx.frame.widget_layout.as_ref().expect("fx layout");
                    let node = if drag_curve {
                        let mut nodes = Vec::new();
                        collect_layout_nodes(
                            layout,
                            &mut |node| node.widget_type == "response-curve-editor",
                            &mut nodes,
                        );
                        nodes
                            .first()
                            .copied()
                            .expect("the Filter panel must keep its response-curve-editor")
                    } else {
                        find_layout_node_by_stable_key(layout, &knob_key)
                            .and_then(|node| find_layout_node_by_widget_type(node, "knob-number"))
                            .unwrap_or_else(|| panic!("{label}: knob target {knob_key} vanished"))
                    };
                    node.rect
                };
                let (down_col, down_row) = if drag_curve {
                    (
                        fx_origin_col + target_rect.col + target_rect.width * 0.5 - fx_scroll_left,
                        fx_origin_row + target_rect.row + target_rect.height * 0.5 - fx_scroll_top,
                    )
                } else {
                    (
                        fx_origin_col + target_rect.col + target_rect.width * 0.5 - fx_scroll_left,
                        fx_origin_row + target_rect.row + target_rect.height * 0.5 - fx_scroll_top,
                    )
                };
                assert!(
                    down_col >= fx_body_rect.col
                        && down_col < fx_body_rect.col + fx_body_rect.width
                        && down_row >= fx_body_rect.row
                        && down_row < fx_body_rect.row + fx_body_rect.height,
                    "{label}: drag target must be on screen inside the fx tile ({down_col},{down_row}) body={:?}",
                    (
                        fx_body_rect.col,
                        fx_body_rect.row,
                        fx_body_rect.width,
                        fx_body_rect.height
                    )
                );

                // --- open the drag gesture (outside the timed region) ------
                editor.handle_tiled_mouse_precise(
                    mouse_event(
                        MouseEventKind::Down(MouseButton::Left),
                        down_col.floor() as u16,
                        down_row.floor() as u16,
                    ),
                    down_col,
                    down_row,
                    0,
                );
                let _ = editor.drain_host_commands();
                assert_eq!(
                    editor.active_buffer().name,
                    "*fx*",
                    "{label}: pressing the control must focus the fx tile"
                );
                finish_visible_update(&mut editor, &mut app, &mut tile_retained);

                let mut samples = DragSamples::new();
                let mut host_apply_samples: Vec<f64> = Vec::new();
                let mut host_sync_samples: Vec<f64> = Vec::new();
                let mut applied_commands = 0usize;
                for iteration in 0..(WARMUPS + SAMPLES) {
                    // Alternate around the press point so every update is a
                    // real value change (the curve gates on `meaningful_change`).
                    let offset = if iteration % 2 == 0 { -1.5 } else { 1.5 };
                    let (drag_col, drag_row) = if drag_curve {
                        (down_col + offset * 2.0, down_row + offset)
                    } else {
                        (down_col, down_row + offset)
                    };

                    let started = Instant::now();
                    editor.handle_tiled_mouse_precise(
                        mouse_event(
                            MouseEventKind::Drag(MouseButton::Left),
                            drag_col.floor() as u16,
                            drag_row.floor() as u16,
                        ),
                        drag_col,
                        drag_row,
                        0,
                    );
                    let commands = editor.drain_host_commands();
                    let dispatch_done = Instant::now();
                    let mut host_apply_ms = 0.0f64;
                    let mut host_sync_ms = 0.0f64;
                    assert!(
                        !commands.is_empty(),
                        "{label}: drag update {iteration} must emit a host command"
                    );
                    for command in commands {
                        let HostCommand::Custom { name, payload } = command else {
                            continue;
                        };
                        let Value::Map(ref map) = payload else {
                            panic!("{label}: {name} payload must be a map: {payload:?}");
                        };
                        applied_commands += 1;
                        // Split the host phase so the probe reports how much of
                        // it is the model write (`apply_command`) versus the
                        // targeted display syncs (which run a reactive cycle).
                        match name.as_str() {
                            // instrument_params.rs handlers
                            "set-instrument-param" | "set-instrument-plock" => {
                                let plock = name == "set-instrument-plock";
                                assert_eq!(
                                    plock,
                                    selection_size > 0,
                                    "{label}: selection state must pick the p-lock command"
                                );
                                let param_idx =
                                    map_usize(map, "param-idx").expect("param-idx");
                                let user_val =
                                    map_number(map, "value").expect("value") as f32;
                                let desc = app
                                    .graph
                                    .instrument_descriptors
                                    .get(TRACK)
                                    .and_then(|desc| desc.params.get(param_idx))
                                    .cloned()
                                    .expect("instrument param descriptor");
                                let stored = desc.clamp(desc.user_input_to_stored(user_val));
                                let (neural_selection, wrote_neural_plock, _) =
                                    record_selected_neural_instrument_plock(
                                        &mut editor,
                                        &state,
                                        &selected_neural_neurons,
                                        TRACK,
                                        param_idx,
                                        stored,
                                    );
                                assert!(!wrote_neural_plock);
                                // Mirrors instrument_params.rs: the row list is
                                // only republished when the row set can change.
                                let plock_row_existed = displayed_plock_step(
                                    &state,
                                    TRACK,
                                    selected_plock_step(&selected_steps),
                                )
                                .and_then(|step| {
                                    state
                                        .pattern
                                        .instrument_slots
                                        .get(TRACK)
                                        .and_then(|slot| slot.plocks.get(step, param_idx))
                                })
                                .is_some()
                                    && matches!(
                                        desc.kind,
                                        sequencer::effects::ParamKind::Continuous { .. }
                                    );
                                let display_step = if plock {
                                    let steps: Vec<usize> = selected_steps
                                        .lock()
                                        .unwrap()
                                        .iter()
                                        .copied()
                                        .collect();
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetInstrumentPlockMulti {
                                            track: TRACK,
                                            steps,
                                            param_idx,
                                            value: stored,
                                        },
                                    );
                                    displayed_plock_step(
                                        &state,
                                        TRACK,
                                        selected_plock_step(&selected_steps),
                                    )
                                } else {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetInstrumentParam {
                                            track: TRACK,
                                            param_idx,
                                            value: stored,
                                        },
                                    );
                                    None
                                };
                                host_apply_ms += duration_ms(dispatch_done.elapsed());
                                let sync_started = Instant::now();
                                sync_instrument_param_authoring_display(
                                    &mut editor,
                                    InstrumentParamDisplaySync {
                                        app: &app,
                                        state: &state,
                                        selected_steps: &selected_steps,
                                        selection: &neural_selection,
                                        expanded_step_projection: &expanded_step_projection,
                                        track: TRACK,
                                        param_idx,
                                        display_step,
                                        sync_plock_list: plock && !plock_row_existed,
                                        sync_plock_presence: plock,
                                        sync_sampler_times: true,
                                    },
                                );
                                host_sync_ms += duration_ms(sync_started.elapsed());
                                if param_change_needs_fx_rebuild(&desc) {
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            // effects.rs handlers
                            "set-effect-param-batch" => {
                                let slot_idx = map_usize(map, "slot-idx").expect("slot-idx");
                                assert_eq!(
                                    Some(slot_idx),
                                    filter_slot,
                                    "{label}: the curve must drive the Filter slot"
                                );
                                let updates =
                                    map_param_updates(map).expect("batch updates");
                                let batch = updates
                                    .into_iter()
                                    .filter_map(|(param_idx, value)| {
                                        let desc = app
                                            .graph
                                            .effect_descriptors
                                            .get(TRACK)?
                                            .get(slot_idx)?
                                            .params
                                            .get(param_idx)?;
                                        Some(app::AppCommand::SetEffectParam {
                                            track: TRACK,
                                            slot_idx,
                                            param_idx,
                                            value: value.clamp(desc.min, desc.max),
                                        })
                                    })
                                    .collect::<Vec<_>>();
                                assert_eq!(
                                    batch.len(),
                                    2,
                                    "{label}: a filter curve drag writes cutoff and resonance"
                                );
                                let param_indices = batch
                                    .iter()
                                    .filter_map(|command| match command {
                                        app::AppCommand::SetEffectParam {
                                            param_idx, ..
                                        } => Some(*param_idx),
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>();
                                app::edit::apply_coalesced_device_value_batch(
                                    &mut app,
                                    &batch,
                                    "effect-curve",
                                    "Set effect curve",
                                )
                                .expect("apply curve batch");
                                let neural_selection =
                                    selected_neural_neurons.lock().unwrap().clone();
                                sync_effect_param_batch_display(
                                    &mut editor,
                                    &app,
                                    &neural_selection,
                                    TRACK,
                                    slot_idx,
                                    &param_indices,
                                    None,
                                );
                                if map_bool(map, "commit") {
                                    app::edit::finish_active_gesture(&mut app);
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            "set-effect-param" => {
                                let slot_idx = map_usize(map, "slot-idx").expect("slot-idx");
                                let param_idx =
                                    map_usize(map, "param-idx").expect("param-idx");
                                let value = map_number(map, "value").expect("value") as f32;
                                let desc = app
                                    .graph
                                    .effect_descriptors
                                    .get(TRACK)
                                    .and_then(|slots| slots.get(slot_idx))
                                    .and_then(|desc| desc.params.get(param_idx))
                                    .cloned();
                                let clamped = desc
                                    .as_ref()
                                    .map(|p| value.clamp(p.min, p.max))
                                    .unwrap_or(value);
                                let (neural_selection, wrote_neural_plock, _) =
                                    record_selected_neural_effect_plock(
                                        &mut editor,
                                        &state,
                                        &selected_neural_neurons,
                                        TRACK,
                                        slot_idx,
                                        param_idx,
                                        clamped,
                                    );
                                assert!(!wrote_neural_plock);
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetEffectParam {
                                        track: TRACK,
                                        slot_idx,
                                        param_idx,
                                        value: clamped,
                                    },
                                );
                                sync_effect_param_authoring_display(
                                    &mut editor,
                                    EffectParamDisplaySync {
                                        state: &state,
                                        effect_descriptors: &app.graph.effect_descriptors,
                                        app: &app,
                                        selected_steps: &selected_steps,
                                        selection: &neural_selection,
                                        track: TRACK,
                                        slot_idx,
                                        param_idx,
                                        display_step: None,
                                        sync_plock_list: false,
                                    },
                                );
                                if desc.as_ref().is_some_and(param_change_needs_fx_rebuild) {
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            other => panic!("{label}: unexpected host command {other}"),
                        }
                    }
                    let host_done = Instant::now();
                    let update =
                        finish_visible_update(&mut editor, &mut app, &mut tile_retained);
                    let total_ms = duration_ms(started.elapsed());
                    let fx_dirty = update
                        .tiles
                        .iter()
                        .find(|(name, _, _, _)| name == "*fx*")
                        .map(|(_, dirty, _, _)| *dirty)
                        .expect("fx tile stats");
                    assert!(
                        fx_dirty > 0,
                        "{label}: a device drag must dirty widgets in the visible *fx* tile"
                    );
                    if iteration >= WARMUPS {
                        samples.record(
                            total_ms,
                            duration_ms(dispatch_done - started),
                            duration_ms(host_done - dispatch_done),
                            &update,
                        );
                        host_apply_samples.push(host_apply_ms);
                        host_sync_samples.push(host_sync_ms);
                    }
                }

                // Close the gesture outside the timed region.
                editor.handle_tiled_mouse_precise(
                    mouse_event(
                        MouseEventKind::Up(MouseButton::Left),
                        down_col.floor() as u16,
                        down_row.floor() as u16,
                    ),
                    down_col,
                    down_row,
                    0,
                );
                let _ = editor.drain_host_commands();
                app::edit::finish_active_gesture(&mut app);
                finish_visible_update(&mut editor, &mut app, &mut tile_retained);

                assert!(
                    applied_commands >= WARMUPS + SAMPLES,
                    "{label}: every drag update must reach a host command"
                );
                let median = percentile(&mut samples.total, 0.50);
                let dispatch_median = percentile(&mut samples.dispatch, 0.50);
                let host_median = percentile(&mut samples.host, 0.50);
                eprintln!(
                    "[{probe_prefix}-{label}] selected={} tiles={} samples={} median_ms={:.3} p95_ms={:.3} input_ms={:.3} host_ms={:.3} visible_update_ms={:.3} ui_epoch_updates={}/{} fx_epoch_updates={}/{}",
                    selection_size,
                    visible_buffers.len(),
                    SAMPLES,
                    median,
                    percentile(&mut samples.total, 0.95),
                    dispatch_median,
                    host_median,
                    median - dispatch_median - host_median,
                    samples.ui_epoch_updates,
                    SAMPLES,
                    samples.fx_epoch_updates,
                    SAMPLES,
                );
                eprintln!(
                    "[{probe_prefix}-{label}-host-detail] apply_ms={:.3} display_sync_ms={:.3}",
                    percentile(&mut host_apply_samples, 0.50),
                    percentile(&mut host_sync_samples, 0.50),
                );
                eprintln!(
                    "[{probe_prefix}-{label}-phases] tick_sync_ms={:.3} invalidation_ms={:.3} epoch_sync_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
                    percentile(&mut samples.tick_sync, 0.50),
                    percentile(&mut samples.invalidation, 0.50),
                    percentile(&mut samples.epoch_sync, 0.50),
                    percentile(&mut samples.reactive, 0.50),
                    percentile(&mut samples.frame, 0.50),
                    percentile(&mut samples.retained, 0.50),
                );
                eprintln!(
                    "[{probe_prefix}-{label}-epoch-detail] seq_state_ms={:.3} track_params_ms={:.3} fx_bindings_ms={:.3} piano_ms={:.3} fx_values_ms={:.3}",
                    percentile(&mut samples.epoch_seq_state, 0.50),
                    percentile(&mut samples.epoch_track_params, 0.50),
                    percentile(&mut samples.epoch_fx_bindings, 0.50),
                    percentile(&mut samples.epoch_piano, 0.50),
                    percentile(&mut samples.epoch_fx_values, 0.50),
                );
                eprintln!(
                    "[{probe_prefix}-{label}-reactive-detail] cycle_ms={:.3} side_effects_ms={:.3} seq_layout_refresh_ms={:.3} inactive_layout_refresh_ms={:.3} inactive_layout_refreshes={:.0}",
                    percentile(&mut samples.reactive_cycle, 0.50),
                    percentile(&mut samples.side_effects, 0.50),
                    percentile(&mut samples.seq_layout_refresh, 0.50),
                    percentile(&mut samples.layout_refresh, 0.50),
                    percentile(&mut samples.layout_refresh_count, 0.50),
                );
                let tile_rebuilds = samples.tile_rebuilds.clone();
                let tile_dirty = samples.tile_dirty.clone();
                let tile_breakdown = samples
                    .tile_retained
                    .iter_mut()
                    .map(|(tile, tile_samples)| {
                        format!(
                            "{tile}={:.3}(dirty={} rebuilds={})",
                            percentile(tile_samples, 0.50),
                            tile_dirty.get(tile).copied().unwrap_or(0),
                            tile_rebuilds.get(tile).copied().unwrap_or(0),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                eprintln!("[{probe_prefix}-{label}-retained-tiles] {tile_breakdown}");
                scenario_results.push(ScenarioResult {
                    label: label.to_string(),
                    median_ms: median,
                    dispatch_ms: dispatch_median,
                    ui_epoch_updates: samples.ui_epoch_updates,
                    fx_epoch_updates: samples.fx_epoch_updates,
                });
            }

            let scenario = |name: &str| -> &ScenarioResult {
                scenario_results
                    .iter()
                    .find(|result| result.label == name)
                    .unwrap_or_else(|| panic!("{name} scenario"))
            };

            // Ceilings. The absolute numbers are loose (the medians below are
            // 0.6-3.1ms on this machine; CI hardware varies), so the real
            // gates are the ratio and epoch assertions, which are
            // machine-independent.
            if curve_probe {
                let curve = scenario("curve-drag");
                let knob = scenario("knob-baseline");
                let ratio = curve.median_ms / knob.median_ms.max(f64::MIN_POSITIVE);
                eprintln!(
                    "[{probe_prefix}-comparison] curve_median_ms={:.3} knob_median_ms={:.3} curve_vs_knob={:.2}x",
                    curve.median_ms, knob.median_ms, ratio,
                );
                // Was 13.2x before the drag echo was deleted from
                // filter-core.lisp / str8-delay.lisp; now ~1.0x.
                assert!(
                    ratio < 4.0,
                    "curve drag must cost about what a knob drag costs, got {ratio:.2}x \
                     (curve {:.3}ms vs knob {:.3}ms) — a curve drag is re-running its panel again",
                    curve.median_ms,
                    knob.median_ms,
                );
                // 87% of the old cost was the on-action callback dirtying the
                // panel inside `VM::invoke`'s synchronous reactive pass.
                assert!(
                    curve.dispatch_ms < 1.5,
                    "curve drag dispatch (widget event -> lisp callback) must stay cheap, got {:.3}ms",
                    curve.dispatch_ms,
                );
                assert!(
                    curve.median_ms < 3.0,
                    "curve drag median {:.3}ms (was 8.2ms before the fix)",
                    curve.median_ms,
                );
                assert!(
                    knob.median_ms < 3.0,
                    "knob baseline median {:.3}ms",
                    knob.median_ms,
                );
            } else {
                let live = scenario("knob-no-selection");
                let one = scenario("knob-1-step-plock");
                let all = scenario("knob-64-step-plock");
                let one_ratio = one.median_ms / live.median_ms.max(f64::MIN_POSITIVE);
                let all_ratio = all.median_ms / live.median_ms.max(f64::MIN_POSITIVE);
                eprintln!(
                    "[{probe_prefix}-comparison] live_median_ms={:.3} plock1_median_ms={:.3} plock64_median_ms={:.3} plock1_vs_live={:.2}x plock64_vs_live={:.2}x",
                    live.median_ms, one.median_ms, all.median_ms, one_ratio, all_ratio,
                );
                // The whole point of the fix: a continuous p-lock drag no
                // longer bumps ui_epoch/fx_epoch per event, so it no longer
                // rebuilds SEQ.instrument-panel and reruns the entire *fx*
                // widget source (that was 43ms, 47x the unselected knob).
                for result in [live, one, all] {
                    assert_eq!(
                        (result.ui_epoch_updates, result.fx_epoch_updates),
                        (0, 0),
                        "{}: a continuous instrument param drag must not bump ui/fx epochs",
                        result.label,
                    );
                }
                assert!(
                    one_ratio < 8.0 && all_ratio < 8.0,
                    "writing a p-lock must stay close to writing a base value, got {one_ratio:.2}x / {all_ratio:.2}x \
                     (live {:.3}ms, 1-step {:.3}ms, 64-step {:.3}ms)",
                    live.median_ms,
                    one.median_ms,
                    all.median_ms,
                );
                assert!(
                    live.median_ms < 4.0,
                    "unselected knob drag median {:.3}ms",
                    live.median_ms,
                );
                assert!(
                    one.median_ms < 12.0 && all.median_ms < 12.0,
                    "p-lock knob drag medians {:.3}ms / {:.3}ms (were 43.7ms / 44.8ms before the fix)",
                    one.median_ms,
                    all.median_ms,
                );
            }
            return;
        }

        // ---------------------------------------------------------------
        // *step*-buffer parameter edits (step-buffer.lisp ->
        // fx-step-parameters-panel): drag Transpose / Velocity / Duration
        // with the step cursor parked on a step and NOTHING selected — the
        // "step N · 0 selected" panel the user reports as slow.
        //
        // Every drag update runs the real chain:
        //   number-picker drag -> fx-step-set-param -> seq-set-step-param
        //   -> HostCommand "set-step-param-history"
        //   -> dispatch_custom_host_command -> step_history::handle
        // The handler is driven through the REAL dispatch seam (a
        // `SharedHandles`/`LoopCtx` built from this probe's own handles), not
        // a hand-written mirror, so whatever epoch/invalidation policy the
        // handler has is exactly what the probe measures — and a fix that
        // changes that policy needs no probe edit to be reflected here.
        // ---------------------------------------------------------------
        if probe == Project92UiProbe::StepBufferParamDrag {
            const TRACK: usize = 0;
            const STEP_COUNT: usize = 64;
            /// Cursor lands here. Kept inactive by the fixture below so the
            /// plain click that parks the cursor toggles the step on instead
            /// of selecting it (step-pointer-up -> seq-toggle-step), leaving
            /// the panel in its "0 selected" state.
            const CURSOR_STEP: usize = 11;
            const WARMUPS: usize = 5;
            const SAMPLES: usize = 20;
            let probe_prefix = "project-92-fullayout-step-buffer";

            let sequencer_buffer_idx = editor
                .buffers
                .iter()
                .position(|buffer| buffer.name == "*sequencer*")
                .expect("sequencer buffer index");
            let sequencer_tile = editor
                .tile_root
                .leaf_ids()
                .into_iter()
                .find(|tile_id| {
                    editor
                        .tile_root
                        .find_leaf(*tile_id)
                        .is_some_and(|leaf| leaf.buffer_idx == sequencer_buffer_idx)
                })
                .expect("the production layout must show *sequencer*");
            editor.switch_active_tile(sequencer_tile);

            state.pattern.track_params[TRACK].set_num_steps(STEP_COUNT);
            for step in 0..STEP_COUNT {
                state.pattern.patterns[TRACK]
                    .set_step_active(step, step < 24 && step != CURSOR_STEP);
            }

            let transport_visible = editor_has_visible_buffer(&editor, "*transport*");
            let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
            let mixer_visible = editor_has_visible_buffer(&editor, "*mixer*");
            assert!(
                editor_has_visible_buffer(&editor, "*step*"),
                "production layout must show the *step* panel"
            );
            assert!(fx_visible, "production layout must show the *fx* panel");
            assert!(mixer_visible, "production layout must show the *mixer* strip");
            assert!(
                transport_visible,
                "production layout must show the *transport* bar"
            );
            assert!(
                !editor_has_visible_buffer(&editor, "*arrangement*"),
                "the step-buffer probe must measure the Seq view"
            );

            sync_all_track_sequencer_state(
                editor.runtime_mut(),
                &state,
                &app,
                TRACK,
                &selected_steps,
            );
            {
                let rt = editor.runtime_mut();
                sync_step_param_lists(rt, &state, TRACK);
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, TRACK));
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    build_effects_value(
                        &state,
                        TRACK,
                        &app.graph.effect_descriptors,
                        &selected_steps,
                    ),
                );
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, TRACK, &selected_steps),
                );
            }
            sync_fx_param_binding_fields_with_neural_selection(
                editor.runtime_mut(),
                &app,
                &state,
                TRACK,
                &selected_steps,
                None,
            );

            let mut song_frame = super::state_values::SongFrameState::default();
            app.sync_track_sound_bindings();
            super::state_values::sync_song_state(
                editor.runtime_mut(),
                &app,
                &mut song_frame,
                transport_visible,
            );
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.update_tile_rects(vp_cols, vp_rows);

            let initial_frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                &mut editor,
                vp_cols as usize,
                vp_rows as usize,
            );
            let visible_buffers: Vec<String> = initial_frame
                .tiles
                .iter()
                .map(|tile| tile.frame.buffer_name.clone())
                .collect();
            for required in ["*sequencer*", "*step*", "*fx*", "*mixer*", "*transport*"] {
                assert!(
                    visible_buffers.iter().any(|name| name == required),
                    "the production layout must show {required}, got {visible_buffers:?}"
                );
            }
            eprintln!("[{probe_prefix}-visible-buffers] {visible_buffers:?}");

            struct TileRetained {
                buffer_name: String,
                viewport: eseqlisp::widget_render::WidgetViewport,
                runs: Vec<eseqlisp::widget_render::MetalPrimitiveRun>,
                indices: eseqlisp::widget_render::MetalPrimitiveRunIndex,
            }
            let mut tile_retained: Vec<TileRetained> = Vec::new();
            for tile in &initial_frame.tiles {
                let layout = tile.frame.widget_layout.as_ref().unwrap_or_else(|| {
                    panic!(
                        "visible tile {} must have a widget layout",
                        tile.frame.buffer_name
                    )
                });
                let viewport = eseqlisp::widget_render::WidgetViewport {
                    cell_w: 8.0,
                    cell_h: 16.0,
                    vp_w: vp_cols as f32 * 8.0,
                    vp_h: vp_rows as f32 * 16.0,
                    time_seconds: 0.0,
                    focused_widget_id: tile.frame.focused_widget_id,
                    focused_branch: tile.is_active,
                    overlay_viewport_bottom: vp_rows as f32,
                    scroll_top: tile.frame.widget_scroll_top + tile.frame.text_scroll_top as f32,
                    scroll_left: tile.frame.widget_layout_scroll_left,
                    inherited_hover: false,
                };
                let (runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                    layout,
                    viewport,
                    viewport.scroll_top,
                    vp_rows,
                );
                let indices = eseqlisp::widget_render::build_metal_primitive_run_index(&runs);
                tile_retained.push(TileRetained {
                    buffer_name: tile.frame.buffer_name.clone(),
                    viewport,
                    runs,
                    indices,
                });
            }

            // The real host-command seam. Every handle here is the same one
            // the lisp natives and the probe's own syncs already share, so
            // the handler mutates exactly the state the probe reads back.
            let cached_track_peak_levels = vec![0.0; app.tracks.len()];
            let shared = SharedHandles {
                state: state.clone(),
                lg_raw,
                current_track: current_track.clone(),
                selected_tracks: selected_tracks.clone(),
                selected_steps: selected_steps.clone(),
                selected_neural_neurons: selected_neural_neurons.clone(),
                piano_roll_selection: piano_roll_selection.clone(),
                piano_roll_move_state: piano_roll_move_state.clone(),
                piano_roll_focus: piano_roll_focus.clone(),
                step_clipboard: Arc::new(Mutex::new(None)),
                ui_epoch: ui_epoch.clone(),
                fx_epoch: fx_epoch.clone(),
                fx_value_epoch: fx_value_epoch.clone(),
                ui_invalidations: ui_invalidations.clone(),
                expanded_step_projection: expanded_step_projection.clone(),
                active_delete_target: active_delete_target.clone(),
                active_delete_target_version: active_delete_target_version.clone(),
                auto_follow_override_until: auto_follow_override_until.clone(),
                track_pan_ids: track_pan_ids.clone(),
                track_collapsed: track_collapsed.clone(),
                bus_state: bus_state.clone(),
                bus_node_ids: bus_node_ids.clone(),
                track_groups: track_groups.clone(),
                record_armed: record_armed.clone(),
                armed_rack: Arc::new(Mutex::new(None)),
                recording: recording.clone(),
                master_recording: master_recording.clone(),
                held_notes: Arc::new(Mutex::new(Vec::new())),
                roll_record: Arc::new(Mutex::new(RollRecordBuffer::default())),
                step_print: Arc::new(Mutex::new(StepPrintState::default())),
                keyboard_octave: Arc::new(std::sync::atomic::AtomicI32::new(0)),
                sample_browser: sample_browser.clone(),
                keyboard_tx: keyboard_tx.clone(),
                accumulator_names: accumulator_names.clone(),
                piano_roll_clipboard: piano_roll_clipboard.clone(),
                arrangement_clipboard: app::song_region::new_arrangement_clipboard(),
                selected_drum_lane_steps: selected_drum_lane_steps.clone(),
            };
            let mut sessions = EditSessionState::default();
            let mut frame_diff = FrameDiffState::default();
            let mut gesture_state = GestureState::default();
            let mut meters = MeterCache {
                cached_peak_l_level: 0.0,
                cached_peak_r_level: 0.0,
                cached_track_peak_levels: cached_track_peak_levels.clone(),
                cached_rack_slot_peak_levels: Vec::new(),
                cached_bus_peak_levels: cached_bus_peak_levels.clone(),
                cached_modulator_phases: Vec::new(),
                cached_modulator_levels: Vec::new(),
                cached_cpu_load_bits: 0.0f32.to_bits(),
                last_meter_poll_at: Instant::now(),
                last_cpu_ui_poll_at: Instant::now(),
                last_neural_visualization_poll_at: Instant::now(),
                visualization_liveness: VisualizationLiveness::default(),
                last_voice_count_log_at: Instant::now(),
            };
            // The visible-update closure below borrows `track_names`
            // mutably for its whole lifetime, so the dispatch context gets
            // its own copy. `set-step-param-history` never touches it.
            let mut ctx_track_names = track_names.clone();

            let mut apply_host_commands = |editor: &mut Editor,
                                           app: &mut app::App,
                                           commands: Vec<HostCommand>|
             -> usize {
                let mut applied = 0usize;
                for command in commands {
                    let HostCommand::Custom { name, payload } = command else {
                        continue;
                    };
                    let mut ctx = LoopCtx {
                        sessions: &mut sessions,
                        meters: &mut meters,
                        frame: &mut frame_diff,
                        gesture: &mut gesture_state,
                        track_names: &mut ctx_track_names,
                        shared: &shared,
                    };
                    dispatch_custom_host_command(&name, payload, app, editor, &mut ctx);
                    applied += 1;
                }
                applied
            };

            // --- visible update: the real reactive tick, minus the render --
            let neural = selected_neural_neurons.lock().unwrap().clone();
            let mut prev_ui_epoch = ui_epoch.load(Ordering::Relaxed);
            let mut prev_fx_epoch = fx_epoch.load(Ordering::Relaxed);
            // `seq-pause-auto-follow` (which `fx-step-set-param` calls on every
            // edit) only bumps ui_epoch on the following -> paused transition;
            // the pause itself reaches the UI through this per-tick delta write
            // in reactive_tick.rs. Mirror it so the probe measures the path
            // that now carries it.
            let mut prev_auto_follow =
                super::state_values::auto_follow_enabled(&auto_follow_override_until);
            let mut track_param_sync_revision: Option<super::loop_ctx::ParamSyncRevision> = None;
            let mut fx_param_sync_revision: Option<super::loop_ctx::ParamSyncRevision> = None;

            struct EditUpdate {
                tick_sync_ms: f64,
                invalidation_ms: f64,
                epoch_sync_ms: f64,
                reactive_ms: f64,
                frame_ms: f64,
                retained_ms: f64,
                epoch_seq_state_ms: f64,
                epoch_track_params_ms: f64,
                epoch_fx_bindings_ms: f64,
                epoch_piano_ms: f64,
                epoch_fx_values_ms: f64,
                reactive_cycle_ms: f64,
                side_effects_ms: f64,
                seq_layout_refresh_ms: f64,
                layout_refresh_ms: f64,
                layout_refresh_count: usize,
                ui_epoch_fired: bool,
                fx_epoch_fired: bool,
                tiles: Vec<(String, usize, f64, bool)>,
            }

            let mut finish_visible_update = |editor: &mut Editor,
                                             app: &mut app::App,
                                             tiles: &mut Vec<TileRetained>|
             -> EditUpdate {
                let started = Instant::now();
                app.sync_track_sound_bindings();
                super::state_values::sync_song_state(
                    editor.runtime_mut(),
                    app,
                    &mut song_frame,
                    transport_visible,
                );
                let tick_sync_done = Instant::now();
                let invalidations = ui_invalidations.drain();
                if !invalidations.is_empty() {
                    apply_ui_invalidations(
                        invalidations,
                        UiInvalidationApplyCtx {
                            app,
                            editor,
                            state: &state,
                            track_collapsed: &track_collapsed,
                            bus_state: &bus_state,
                            current_track_idx: TRACK,
                            selected_steps: &selected_steps,
                            selected_neural_neurons: &neural,
                            piano_roll_selection: &piano_roll_selection,
                            accumulator_names: &accumulator_names,
                            cached_track_peak_levels: &cached_track_peak_levels,
                            cached_bus_peak_levels: &cached_bus_peak_levels,
                            record_armed: &record_armed,
                            active_delete_target: &active_delete_target,
                            active_delete_target_version: &active_delete_target_version,
                            expanded_step_projection: &expanded_step_projection,
                            fx_visible,
                            sequencer_visible: true,
                            mixer_visible,
                        },
                    );
                }
                let invalidations_done = Instant::now();
                let mut epoch_seq_state_ms = 0.0;
                let mut epoch_track_params_ms = 0.0;
                let mut epoch_fx_bindings_ms = 0.0;
                let mut epoch_piano_ms = 0.0;
                let mut epoch_fx_values_ms = 0.0;

                // reactive_tick.rs: `SEQ.auto-follow` delta write. This is the
                // surface `seq-pause-auto-follow` actually feeds now that it
                // only bumps ui_epoch on the following -> paused transition.
                let auto_follow =
                    super::state_values::auto_follow_enabled(&auto_follow_override_until);
                if auto_follow != prev_auto_follow {
                    editor
                        .runtime_mut()
                        .set_reactive("SEQ", "auto-follow", Value::Bool(auto_follow));
                    prev_auto_follow = auto_follow;
                }

                // --- reactive_tick.rs ui_epoch / fx_epoch branches ---------
                let ui_ep = ui_epoch.load(Ordering::Relaxed);
                let fx_ep = fx_epoch.load(Ordering::Relaxed);
                let ui_epoch_fired = ui_ep != prev_ui_epoch;
                let fx_epoch_fired = fx_visible && fx_ep != prev_fx_epoch;
                if ui_epoch_fired {
                    let mut sorted_steps: Vec<usize> =
                        selected_steps.lock().unwrap().iter().copied().collect();
                    sorted_steps.sort_unstable();
                    let revision = super::loop_ctx::ParamSyncRevision {
                        track: TRACK,
                        scene: state.current_scene_index(),
                        pattern_epoch: state.transport.pattern_epoch.load(Ordering::Relaxed),
                        song_row_mirror_epoch: app.song_row_mirror_epoch,
                        ui_epoch: ui_ep,
                        fx_epoch: fx_ep,
                        sound_binding_epoch: app.sound_binding_epoch,
                        display_step: displayed_plock_step(
                            &state,
                            TRACK,
                            sorted_steps.first().copied(),
                        ),
                        selected_steps: sorted_steps,
                        selected_neural_neurons: neural.iter().copied().collect(),
                    };
                    sync_shared_track_collapsed(&track_collapsed, app);
                    {
                        let rt = editor.runtime_mut();
                        sync_macro_state(rt, app);
                        sync_track_name_state(rt, &mut track_names, app);
                        rt.set_reactive("SEQ", "steps", build_steps_value(&state, TRACK));
                        sync_step_param_lists(rt, &state, TRACK);
                        let phase = Instant::now();
                        sync_all_track_sequencer_state(rt, &state, app, TRACK, &selected_steps);
                        let _ = sync_all_expanded_step_viewports(
                            rt,
                            &state,
                            app,
                            &selected_steps,
                            TRACK,
                            &expanded_step_projection,
                        );
                        epoch_seq_state_ms = duration_ms(phase.elapsed());
                        sync_track_mixer_state(rt, app, &state);
                        sync_bus_mixer_state(rt, app);
                        sync_track_peak_fields(rt, &cached_track_peak_levels);
                        sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                        *accumulator_names.lock().unwrap() = build_accumulator_names(app);
                        let phase = Instant::now();
                        if super::reactive_tick::claim_param_sync_revision(
                            &mut track_param_sync_revision,
                            &revision,
                        ) {
                            sync_track_params_with_neural_selection(
                                rt,
                                app,
                                &state,
                                TRACK,
                                &selected_steps,
                                Some(&neural),
                            );
                        }
                        epoch_track_params_ms = duration_ms(phase.elapsed());
                        let _ = sync_track_plock_variant_preview(
                            rt,
                            app,
                            &state,
                            TRACK,
                            &selected_steps,
                            None,
                        );
                        let phase = Instant::now();
                        if super::reactive_tick::claim_param_sync_revision(
                            &mut fx_param_sync_revision,
                            &revision,
                        ) {
                            let _ = sync_fx_param_binding_fields_with_neural_selection(
                                rt,
                                app,
                                &state,
                                TRACK,
                                &selected_steps,
                                Some(&neural),
                            );
                        }
                        epoch_fx_bindings_ms = duration_ms(phase.elapsed());
                        rt.set_reactive(
                            "SEQ",
                            "selected-steps",
                            build_selection_value(&selected_steps),
                        );
                        let phase = Instant::now();
                        sync_piano_roll_state(rt, app, &state, TRACK, &piano_roll_selection);
                        epoch_piano_ms = duration_ms(phase.elapsed());
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(&state, TRACK, &app.graph.effect_descriptors),
                        );
                        sync_mixer_delete_target_binding_fields(
                            rt,
                            app.tracks.len(),
                            &state,
                            active_delete_target.lock().unwrap().as_ref(),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "record-armed",
                            build_record_armed_value(&record_armed.lock().unwrap()),
                        );
                    }
                    prev_ui_epoch = ui_ep;
                }
                if fx_epoch_fired {
                    let phase = Instant::now();
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "effects",
                        build_effects_value(
                            &state,
                            TRACK,
                            &app.graph.effect_descriptors,
                            &selected_steps,
                        ),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "midi-effects",
                        build_midi_effects_value(&state, TRACK, &selected_steps),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "instrument-panel",
                        build_instrument_panel_value(app, TRACK, &selected_steps),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "step-has-plocks",
                        build_step_has_plocks(&state, TRACK, &app.graph.effect_descriptors),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "bus-effects",
                        build_bus_effects_value_for_selection(app, Some(&selected_steps)),
                    );
                    prev_fx_epoch = fx_ep;
                    epoch_fx_values_ms = duration_ms(phase.elapsed());
                }
                let epoch_sync_done = Instant::now();

                editor.runtime_mut().run_reactive_cycle();
                let cycle_done = Instant::now();
                editor.refresh_runtime_side_effects();
                let side_effects_done = Instant::now();
                let (layout_refresh_ms, layout_refresh_count) = {
                    let timings = editor.last_layout_refresh_timings();
                    (
                        timings
                            .iter()
                            .map(|timing| timing.elapsed.as_secs_f64() * 1000.0)
                            .sum::<f64>(),
                        timings.len(),
                    )
                };
                if ui_epoch_fired {
                    editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                }
                let reactive_done = Instant::now();
                let frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                    editor,
                    vp_cols as usize,
                    vp_rows as usize,
                );
                let frame_done = Instant::now();
                assert_eq!(
                    frame.tiles.len(),
                    tiles.len(),
                    "the production layout must keep every tile visible"
                );
                let mut tile_stats = Vec::with_capacity(frame.tiles.len());
                for tile in &frame.tiles {
                    let entry = tiles
                        .iter_mut()
                        .find(|entry| entry.buffer_name == tile.frame.buffer_name)
                        .unwrap_or_else(|| {
                            panic!("retained runs for visible tile {}", tile.frame.buffer_name)
                        });
                    let layout = tile.frame.widget_layout.as_ref().unwrap_or_else(|| {
                        panic!(
                            "visible tile {} must keep a widget layout",
                            tile.frame.buffer_name
                        )
                    });
                    let tile_started = Instant::now();
                    let (_, stats) =
                        eseqlisp::widget_render::refresh_metal_primitive_runs_retained_in_place(
                            layout,
                            entry.viewport,
                            entry.viewport.scroll_top,
                            vp_rows,
                            &mut entry.runs,
                            &entry.indices,
                            &tile.frame.dirty_widget_ids,
                        );
                    let structural_rebuild =
                        stats.missing_previous_runs > 0 || stats.invalid_previous_runs > 0;
                    if structural_rebuild {
                        let (runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                            layout,
                            entry.viewport,
                            entry.viewport.scroll_top,
                            vp_rows,
                        );
                        entry.indices =
                            eseqlisp::widget_render::build_metal_primitive_run_index(&runs);
                        entry.runs = runs;
                    }
                    tile_stats.push((
                        tile.frame.buffer_name.clone(),
                        tile.frame.dirty_widget_ids.len(),
                        duration_ms(tile_started.elapsed()),
                        structural_rebuild,
                    ));
                }
                let retained_done = Instant::now();
                EditUpdate {
                    tick_sync_ms: duration_ms(tick_sync_done - started),
                    invalidation_ms: duration_ms(invalidations_done - tick_sync_done),
                    epoch_sync_ms: duration_ms(epoch_sync_done - invalidations_done),
                    reactive_ms: duration_ms(reactive_done - epoch_sync_done),
                    frame_ms: duration_ms(frame_done - reactive_done),
                    retained_ms: duration_ms(retained_done - frame_done),
                    epoch_seq_state_ms,
                    epoch_track_params_ms,
                    epoch_fx_bindings_ms,
                    epoch_piano_ms,
                    epoch_fx_values_ms,
                    reactive_cycle_ms: duration_ms(cycle_done - epoch_sync_done),
                    side_effects_ms: duration_ms(side_effects_done - cycle_done),
                    seq_layout_refresh_ms: duration_ms(reactive_done - side_effects_done),
                    layout_refresh_ms,
                    layout_refresh_count,
                    ui_epoch_fired,
                    fx_epoch_fired,
                    tiles: tile_stats,
                }
            };

            let percentile = |samples: &mut Vec<f64>, fraction: f64| {
                samples.sort_by(|a, b| a.total_cmp(b));
                let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
                samples[index]
            };

            struct EditSamples {
                total: Vec<f64>,
                dispatch: Vec<f64>,
                host: Vec<f64>,
                tick_sync: Vec<f64>,
                invalidation: Vec<f64>,
                epoch_sync: Vec<f64>,
                reactive: Vec<f64>,
                frame: Vec<f64>,
                retained: Vec<f64>,
                epoch_seq_state: Vec<f64>,
                epoch_track_params: Vec<f64>,
                epoch_fx_bindings: Vec<f64>,
                epoch_piano: Vec<f64>,
                epoch_fx_values: Vec<f64>,
                reactive_cycle: Vec<f64>,
                side_effects: Vec<f64>,
                seq_layout_refresh: Vec<f64>,
                layout_refresh: Vec<f64>,
                layout_refresh_count: Vec<f64>,
                ui_epoch_updates: usize,
                fx_epoch_updates: usize,
                tile_retained: std::collections::BTreeMap<String, Vec<f64>>,
                tile_rebuilds: std::collections::BTreeMap<String, usize>,
                tile_dirty: std::collections::BTreeMap<String, usize>,
            }
            impl EditSamples {
                fn new() -> Self {
                    Self {
                        total: Vec::new(),
                        dispatch: Vec::new(),
                        host: Vec::new(),
                        tick_sync: Vec::new(),
                        invalidation: Vec::new(),
                        epoch_sync: Vec::new(),
                        reactive: Vec::new(),
                        frame: Vec::new(),
                        retained: Vec::new(),
                        epoch_seq_state: Vec::new(),
                        epoch_track_params: Vec::new(),
                        epoch_fx_bindings: Vec::new(),
                        epoch_piano: Vec::new(),
                        epoch_fx_values: Vec::new(),
                        reactive_cycle: Vec::new(),
                        side_effects: Vec::new(),
                        seq_layout_refresh: Vec::new(),
                        layout_refresh: Vec::new(),
                        layout_refresh_count: Vec::new(),
                        ui_epoch_updates: 0,
                        fx_epoch_updates: 0,
                        tile_retained: std::collections::BTreeMap::new(),
                        tile_rebuilds: std::collections::BTreeMap::new(),
                        tile_dirty: std::collections::BTreeMap::new(),
                    }
                }
                fn record(
                    &mut self,
                    total_ms: f64,
                    dispatch_ms: f64,
                    host_ms: f64,
                    update: &EditUpdate,
                ) {
                    self.total.push(total_ms);
                    self.dispatch.push(dispatch_ms);
                    self.host.push(host_ms);
                    self.tick_sync.push(update.tick_sync_ms);
                    self.invalidation.push(update.invalidation_ms);
                    self.epoch_sync.push(update.epoch_sync_ms);
                    self.reactive.push(update.reactive_ms);
                    self.frame.push(update.frame_ms);
                    self.retained.push(update.retained_ms);
                    self.epoch_seq_state.push(update.epoch_seq_state_ms);
                    self.epoch_track_params.push(update.epoch_track_params_ms);
                    self.epoch_fx_bindings.push(update.epoch_fx_bindings_ms);
                    self.epoch_piano.push(update.epoch_piano_ms);
                    self.epoch_fx_values.push(update.epoch_fx_values_ms);
                    self.reactive_cycle.push(update.reactive_cycle_ms);
                    self.side_effects.push(update.side_effects_ms);
                    self.seq_layout_refresh.push(update.seq_layout_refresh_ms);
                    self.layout_refresh.push(update.layout_refresh_ms);
                    self.layout_refresh_count
                        .push(update.layout_refresh_count as f64);
                    if update.ui_epoch_fired {
                        self.ui_epoch_updates += 1;
                    }
                    if update.fx_epoch_fired {
                        self.fx_epoch_updates += 1;
                    }
                    for (name, dirty, retained_ms, rebuilt) in &update.tiles {
                        self.tile_retained
                            .entry(name.clone())
                            .or_default()
                            .push(*retained_ms);
                        *self.tile_dirty.entry(name.clone()).or_default() += *dirty;
                        if *rebuilt {
                            *self.tile_rebuilds.entry(name.clone()).or_default() += 1;
                        }
                    }
                }
            }

            // --- park the step cursor through the real click gesture -------
            {
                let (col, row, width, height) = {
                    let layout = editor.widget_layout().expect("sequencer layout");
                    let cell = find_layout_node_by_stable_key_suffix(
                        &layout,
                        &format!("/step-cell-{TRACK}-{CURSOR_STEP}"),
                    )
                    .unwrap_or_else(|| panic!("visible sequencer step cell {CURSOR_STEP}"));
                    (
                        cell.rect.col + cell.rect.width * 0.5,
                        cell.rect.row + cell.rect.height * 0.5,
                        layout.rect.width.ceil().max(1.0) as u16,
                        layout.rect.height.ceil().max(1.0) as u16,
                    )
                };
                for kind in [
                    MouseEventKind::Down(MouseButton::Left),
                    MouseEventKind::Up(MouseButton::Left),
                ] {
                    editor.handle_mouse_precise(
                        mouse_event(kind, col.floor() as u16, row.floor() as u16),
                        0,
                        0,
                        width,
                        height,
                        col,
                        row,
                    );
                    let commands = editor.drain_host_commands();
                    apply_host_commands(&mut editor, &mut app, commands);
                }
                finish_visible_update(&mut editor, &mut app, &mut tile_retained);
            }
            assert!(
                selected_steps.lock().unwrap().is_empty(),
                "the cursor click must leave the panel in its '0 selected' state"
            );
            assert_eq!(
                editor
                    .runtime()
                    .reactive_field_value("SEQ", "fx-step-cursor-number")
                    .map(|value| match value {
                        Value::Number(number) => *number,
                        other => panic!("fx-step-cursor-number should be a number: {other:?}"),
                    }),
                Some((CURSOR_STEP + 1) as f64),
                "the click must park the step cursor on step {}",
                CURSOR_STEP + 1
            );

            struct ScenarioResult {
                label: String,
                median_ms: f64,
                dispatch_ms: f64,
                host_ms: f64,
                ui_epoch_updates: usize,
                fx_epoch_updates: usize,
            }
            let mut scenario_results: Vec<ScenarioResult> = Vec::new();

            // (picker key suffix, the model field the edit must move)
            let scenarios: Vec<(&str, StepParam)> = vec![
                ("transpose", StepParam::Transpose),
                ("velocity", StepParam::Velocity),
                ("duration", StepParam::Duration),
            ];

            for (label, param) in scenarios {
                let picker_key = format!("/step-param-{label}");
                // Park the edited param mid-range first (fixture setup, not a
                // measured edit): the picker maps drag distance onto
                // [value..max] / [min..value], so a value already sitting at a
                // rail would make half the drag updates write the value they
                // already hold — a handler NoOp with nothing to measure.
                let seed_value = match param {
                    StepParam::Transpose => 0.0,
                    StepParam::Velocity => 0.5,
                    _ => 8.0,
                };
                state.pattern.step_data[TRACK].set(CURSOR_STEP, param, seed_value);
                ui_invalidations.push(UiInvalidation::Step {
                    track: TRACK,
                    step: CURSOR_STEP,
                    change: StepInvalidation::Param(param.into()),
                });
                finish_visible_update(&mut editor, &mut app, &mut tile_retained);

                // Locate the picker in a freshly built *step* tile layout, and
                // convert to screen space so the drag goes through
                // `handle_tiled_mouse_precise` exactly like event_loop.rs.
                let (down_col, down_row, step_body_rect) = {
                    let frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                        &mut editor,
                        vp_cols as usize,
                        vp_rows as usize,
                    );
                    let step_tile = frame
                        .tiles
                        .iter()
                        .find(|tile| tile.frame.buffer_name == "*step*")
                        .expect("visible step tile");
                    let origin_col = step_tile.body_rect.col.floor();
                    let origin_row = step_tile.body_rect.row.floor();
                    let scroll_top = step_tile.frame.widget_scroll_top
                        + step_tile.frame.text_scroll_top as f32;
                    let scroll_left = step_tile.frame.widget_layout_scroll_left;
                    let layout = step_tile
                        .frame
                        .widget_layout
                        .as_ref()
                        .expect("step tile layout");
                    let node = find_layout_node_by_stable_key_suffix(layout, &picker_key)
                        .and_then(|node| find_layout_node_by_widget_type(node, "number-picker"))
                        .unwrap_or_else(|| {
                            panic!(
                                "the *step* panel must render a number-picker keyed {picker_key} \
                                 (a drum-rack track swaps Transpose for a sound dropdown)"
                            )
                        });
                    (
                        origin_col + node.rect.col + node.rect.width * 0.5 - scroll_left,
                        origin_row + node.rect.row + node.rect.height * 0.5 - scroll_top,
                        step_tile.body_rect,
                    )
                };
                assert!(
                    down_col >= step_body_rect.col
                        && down_col < step_body_rect.col + step_body_rect.width
                        && down_row >= step_body_rect.row
                        && down_row < step_body_rect.row + step_body_rect.height,
                    "{label}: picker must be on screen inside the *step* tile ({down_col},{down_row}) body={:?}",
                    (
                        step_body_rect.col,
                        step_body_rect.row,
                        step_body_rect.width,
                        step_body_rect.height
                    )
                );

                // Open the gesture outside the timed region.
                editor.handle_tiled_mouse_precise(
                    mouse_event(
                        MouseEventKind::Down(MouseButton::Left),
                        down_col.floor() as u16,
                        down_row.floor() as u16,
                    ),
                    down_col,
                    down_row,
                    0,
                );
                let commands = editor.drain_host_commands();
                apply_host_commands(&mut editor, &mut app, commands);
                assert_eq!(
                    editor.active_buffer().name,
                    "*step*",
                    "{label}: pressing the picker must focus the *step* tile"
                );
                finish_visible_update(&mut editor, &mut app, &mut tile_retained);

                let value_before = state.pattern.step_data[TRACK].get(CURSOR_STEP, param);
                let mut observed_change = false;
                let mut real_edits = 0usize;
                let mut samples = EditSamples::new();
                let mut applied_commands = 0usize;
                for iteration in 0..(WARMUPS + SAMPLES) {
                    // Alternate around the press row so every update is a real
                    // value change (an unchanged write is a handler NoOp).
                    let offset = if iteration % 2 == 0 { -3.0 } else { 3.0 };
                    let drag_row = down_row + offset;

                    let started = Instant::now();
                    editor.handle_tiled_mouse_precise(
                        mouse_event(
                            MouseEventKind::Drag(MouseButton::Left),
                            down_col.floor() as u16,
                            drag_row.floor() as u16,
                        ),
                        down_col,
                        drag_row,
                        0,
                    );
                    let commands = editor.drain_host_commands();
                    let dispatch_done = Instant::now();
                    assert!(
                        !commands.is_empty(),
                        "{label}: drag update {iteration} must emit a host command"
                    );
                    for command in &commands {
                        if let HostCommand::Custom { name, .. } = command {
                            assert_eq!(
                                name, "set-step-param-history",
                                "{label}: the *step* panel picker must lower to \
                                 set-step-param-history, got {name}"
                            );
                        }
                    }
                    let value_pre = state.pattern.step_data[TRACK].get(CURSOR_STEP, param);
                    applied_commands += apply_host_commands(&mut editor, &mut app, commands);
                    let host_done = Instant::now();
                    let value_post = state.pattern.step_data[TRACK].get(CURSOR_STEP, param);
                    if value_post != value_before {
                        observed_change = true;
                    }
                    let update =
                        finish_visible_update(&mut editor, &mut app, &mut tile_retained);
                    let total_ms = duration_ms(started.elapsed());
                    let tile_dirty = |name: &str| -> usize {
                        update
                            .tiles
                            .iter()
                            .find(|(tile, _, _, _)| tile == name)
                            .map(|(_, dirty, _, _)| *dirty)
                            .unwrap_or_else(|| panic!("{name} tile stats"))
                    };
                    // Only a write that actually moves the model can be
                    // required to repaint: an identical write is a handler
                    // NoOp by design. The mid-range seed plus the alternating
                    // drag rows keep every update a real change (asserted as
                    // `real_edits` after the loop).
                    if value_post != value_pre {
                        assert!(
                            tile_dirty("*step*") > 0,
                            "{label}: a step-param edit ({value_pre} -> {value_post} on \
                             iteration {iteration}) must dirty widgets in the visible \
                             *step* tile"
                        );
                        if param == StepParam::Duration {
                            assert!(
                                tile_dirty("*sequencer*") > 0,
                                "{label}: a duration edit ({value_pre} -> {value_post}) must \
                                 repaint the compact grid's duration bar in the visible \
                                 *sequencer* tile"
                            );
                        }
                        real_edits += 1;
                    }
                    if iteration >= WARMUPS {
                        samples.record(
                            total_ms,
                            duration_ms(dispatch_done - started),
                            duration_ms(host_done - dispatch_done),
                            &update,
                        );
                    }
                }

                // Close the gesture outside the timed region.
                editor.handle_tiled_mouse_precise(
                    mouse_event(
                        MouseEventKind::Up(MouseButton::Left),
                        down_col.floor() as u16,
                        down_row.floor() as u16,
                    ),
                    down_col,
                    down_row,
                    0,
                );
                let commands = editor.drain_host_commands();
                apply_host_commands(&mut editor, &mut app, commands);
                app::edit::finish_active_gesture(&mut app);
                finish_visible_update(&mut editor, &mut app, &mut tile_retained);

                assert!(
                    applied_commands >= WARMUPS + SAMPLES,
                    "{label}: every drag update must reach the real host-command handler"
                );
                assert!(
                    observed_change,
                    "{label}: the drag must actually move the step's {param:?} value \
                     (started at {value_before})"
                );
                // Every measured update should be a genuine model write; a
                // probe that mostly measured NoOps would report a flattering
                // median for a path nobody exercises.
                assert!(
                    real_edits >= SAMPLES,
                    "{label}: only {real_edits}/{} drag updates actually changed the model — \
                     the fixture is measuring handler NoOps",
                    WARMUPS + SAMPLES,
                );
                assert!(
                    selected_steps.lock().unwrap().is_empty(),
                    "{label}: the probe must stay in the '0 selected' state"
                );

                let median = percentile(&mut samples.total, 0.50);
                let dispatch_median = percentile(&mut samples.dispatch, 0.50);
                let host_median = percentile(&mut samples.host, 0.50);
                eprintln!(
                    "[{probe_prefix}-{label}] cursor_step={} tiles={} samples={} real_edits={real_edits}/{} median_ms={:.3} p95_ms={:.3} input_ms={:.3} host_ms={:.3} visible_update_ms={:.3} ui_epoch_updates={}/{} fx_epoch_updates={}/{}",
                    CURSOR_STEP + 1,
                    visible_buffers.len(),
                    SAMPLES,
                    WARMUPS + SAMPLES,
                    median,
                    percentile(&mut samples.total, 0.95),
                    dispatch_median,
                    host_median,
                    median - dispatch_median - host_median,
                    samples.ui_epoch_updates,
                    SAMPLES,
                    samples.fx_epoch_updates,
                    SAMPLES,
                );
                eprintln!(
                    "[{probe_prefix}-{label}-phases] tick_sync_ms={:.3} invalidation_ms={:.3} epoch_sync_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
                    percentile(&mut samples.tick_sync, 0.50),
                    percentile(&mut samples.invalidation, 0.50),
                    percentile(&mut samples.epoch_sync, 0.50),
                    percentile(&mut samples.reactive, 0.50),
                    percentile(&mut samples.frame, 0.50),
                    percentile(&mut samples.retained, 0.50),
                );
                eprintln!(
                    "[{probe_prefix}-{label}-epoch-detail] seq_state_ms={:.3} track_params_ms={:.3} fx_bindings_ms={:.3} piano_ms={:.3} fx_values_ms={:.3}",
                    percentile(&mut samples.epoch_seq_state, 0.50),
                    percentile(&mut samples.epoch_track_params, 0.50),
                    percentile(&mut samples.epoch_fx_bindings, 0.50),
                    percentile(&mut samples.epoch_piano, 0.50),
                    percentile(&mut samples.epoch_fx_values, 0.50),
                );
                eprintln!(
                    "[{probe_prefix}-{label}-reactive-detail] cycle_ms={:.3} side_effects_ms={:.3} seq_layout_refresh_ms={:.3} inactive_layout_refresh_ms={:.3} inactive_layout_refreshes={:.0}",
                    percentile(&mut samples.reactive_cycle, 0.50),
                    percentile(&mut samples.side_effects, 0.50),
                    percentile(&mut samples.seq_layout_refresh, 0.50),
                    percentile(&mut samples.layout_refresh, 0.50),
                    percentile(&mut samples.layout_refresh_count, 0.50),
                );
                let tile_rebuilds = samples.tile_rebuilds.clone();
                let tile_dirty_totals = samples.tile_dirty.clone();
                let tile_breakdown = samples
                    .tile_retained
                    .iter_mut()
                    .map(|(tile, tile_samples)| {
                        format!(
                            "{tile}={:.3}(dirty={} rebuilds={})",
                            percentile(tile_samples, 0.50),
                            tile_dirty_totals.get(tile).copied().unwrap_or(0),
                            tile_rebuilds.get(tile).copied().unwrap_or(0),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                eprintln!("[{probe_prefix}-{label}-retained-tiles] {tile_breakdown}");
                scenario_results.push(ScenarioResult {
                    label: label.to_string(),
                    median_ms: median,
                    dispatch_ms: dispatch_median,
                    host_ms: host_median,
                    ui_epoch_updates: samples.ui_epoch_updates,
                    fx_epoch_updates: samples.fx_epoch_updates,
                });
            }

            eprintln!(
                "[{probe_prefix}-comparison] {}",
                scenario_results
                    .iter()
                    .map(|result| format!(
                        "{}={:.3}ms(input={:.3} host={:.3} ui_epochs={} fx_epochs={})",
                        result.label,
                        result.median_ms,
                        result.dispatch_ms,
                        result.host_ms,
                        result.ui_epoch_updates,
                        result.fx_epoch_updates,
                    ))
                    .collect::<Vec<_>>()
                    .join(" ")
            );

            // Ceilings with headroom over the measured post-fix medians
            // (transpose 2.4ms, duration 2.2ms, velocity 7.7ms). A step-param
            // drag update must stay off the ui_epoch resync path: bumping the
            // epoch per event cost ~7ms of `sync_all_track_sequencer_state`
            // plus a `*sequencer*` layout refresh and took transpose/duration
            // to ~11ms and velocity to ~16ms.
            //
            // Velocity's remaining budget is NOT this handler's doing: with
            // the default param mode (0 = velocity), the legacy `*metal*` step
            // grid's buffer root reads the whole `SEQ.velocities` list, so any
            // write to it forces a full rerun of that buffer (~4.5ms) even
            // though no tile shows it. Tracked separately; tighten this once
            // that buffer stops depending on the whole list.
            for result in &scenario_results {
                let ceiling_ms = if result.label == "velocity" { 12.0 } else { 5.0 };
                assert!(
                    result.median_ms < ceiling_ms,
                    "{}: *step*-buffer param drag median {:.3}ms exceeded the \
                     {ceiling_ms:.1}ms ceiling",
                    result.label,
                    result.median_ms,
                );
                assert_eq!(
                    result.ui_epoch_updates, 0,
                    "{}: a *step*-buffer param drag must not bump ui_epoch — every \
                     bump costs a whole-project resync ({} of {SAMPLES} updates did)",
                    result.label, result.ui_epoch_updates,
                );
                assert_eq!(
                    result.fx_epoch_updates, 0,
                    "{}: a *step*-buffer param drag must not bump fx_epoch",
                    result.label,
                );
            }
            return;
        }

        // ---------------------------------------------------------------
        // Scene launch + track-clip launch at large-project clip scale.
        //
        // Clip launch is the real gesture: a mouse Down on a mixer
        // `eseq.mixer/track-pattern-cell-*` (its on-click fires on Down),
        // which lowers to `set-scene-cell` — the live clip-launch path.
        // Scene launch is a mouse Down on a `transport-scene-pill-*`,
        // which lowers to `switch-pattern`. Both commands run through the
        // REAL `dispatch_custom_host_command` seam, and the visible update
        // replays the reactive tick INCLUDING its pattern-epoch resync
        // branch: `switch-pattern` resyncs inline in the handler (stamping
        // `ctx.frame.prev_pattern_epoch`), while `set-scene-cell` leaves
        // that work to the per-tick branch — so the same project-wide
        // resync cost lands in `host` for scenes and in `pattern_sync` for
        // clips, and the probe reports both.
        // ---------------------------------------------------------------
        if probe == Project92UiProbe::SceneAndClipLaunch {
            const TRACK: usize = 0;
            const WARMUPS: usize = 4;
            const SAMPLES: usize = 12;
            let probe_prefix = "project-92-fullayout-launch";

            let sequencer_buffer_idx = editor
                .buffers
                .iter()
                .position(|buffer| buffer.name == "*sequencer*")
                .expect("sequencer buffer index");
            let sequencer_tile = editor
                .tile_root
                .leaf_ids()
                .into_iter()
                .find(|tile_id| {
                    editor
                        .tile_root
                        .find_leaf(*tile_id)
                        .is_some_and(|leaf| leaf.buffer_idx == sequencer_buffer_idx)
                })
                .expect("the production layout must show *sequencer*");
            editor.switch_active_tile(sequencer_tile);

            let transport_visible = editor_has_visible_buffer(&editor, "*transport*");
            let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
            let mixer_visible = editor_has_visible_buffer(&editor, "*mixer*");
            assert!(fx_visible, "production layout must show the *fx* panel");
            assert!(mixer_visible, "production layout must show the *mixer* strip");
            assert!(
                transport_visible,
                "production layout must show the *transport* bar"
            );
            assert!(
                !editor_has_visible_buffer(&editor, "*arrangement*"),
                "the launch probe must measure the Seq view"
            );

            sync_all_track_sequencer_state(
                editor.runtime_mut(),
                &state,
                &app,
                TRACK,
                &selected_steps,
            );
            {
                let rt = editor.runtime_mut();
                sync_step_param_lists(rt, &state, TRACK);
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, TRACK));
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    build_effects_value(
                        &state,
                        TRACK,
                        &app.graph.effect_descriptors,
                        &selected_steps,
                    ),
                );
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, TRACK, &selected_steps),
                );
            }
            sync_fx_param_binding_fields_with_neural_selection(
                editor.runtime_mut(),
                &app,
                &state,
                TRACK,
                &selected_steps,
                None,
            );

            // The real host-command seam (same construction as the *step*-
            // buffer probe): every handle is shared with the probe's own
            // syncs, so the handlers mutate exactly the state read back.
            let cached_track_peak_levels = vec![0.0; app.tracks.len()];
            let shared = SharedHandles {
                state: state.clone(),
                lg_raw,
                current_track: current_track.clone(),
                selected_tracks: selected_tracks.clone(),
                selected_steps: selected_steps.clone(),
                selected_neural_neurons: selected_neural_neurons.clone(),
                piano_roll_selection: piano_roll_selection.clone(),
                piano_roll_move_state: piano_roll_move_state.clone(),
                piano_roll_focus: piano_roll_focus.clone(),
                step_clipboard: Arc::new(Mutex::new(None)),
                ui_epoch: ui_epoch.clone(),
                fx_epoch: fx_epoch.clone(),
                fx_value_epoch: fx_value_epoch.clone(),
                ui_invalidations: ui_invalidations.clone(),
                expanded_step_projection: expanded_step_projection.clone(),
                active_delete_target: active_delete_target.clone(),
                active_delete_target_version: active_delete_target_version.clone(),
                auto_follow_override_until: auto_follow_override_until.clone(),
                track_pan_ids: track_pan_ids.clone(),
                track_collapsed: track_collapsed.clone(),
                bus_state: bus_state.clone(),
                bus_node_ids: bus_node_ids.clone(),
                track_groups: track_groups.clone(),
                record_armed: record_armed.clone(),
                armed_rack: Arc::new(Mutex::new(None)),
                recording: recording.clone(),
                master_recording: master_recording.clone(),
                held_notes: Arc::new(Mutex::new(Vec::new())),
                roll_record: Arc::new(Mutex::new(RollRecordBuffer::default())),
                step_print: Arc::new(Mutex::new(StepPrintState::default())),
                keyboard_octave: Arc::new(std::sync::atomic::AtomicI32::new(0)),
                sample_browser: sample_browser.clone(),
                keyboard_tx: keyboard_tx.clone(),
                accumulator_names: accumulator_names.clone(),
                piano_roll_clipboard: piano_roll_clipboard.clone(),
                arrangement_clipboard: app::song_region::new_arrangement_clipboard(),
                selected_drum_lane_steps: selected_drum_lane_steps.clone(),
            };
            let mut sessions = EditSessionState::default();
            let mut gesture_state = GestureState::default();
            let mut meters = MeterCache {
                cached_peak_l_level: 0.0,
                cached_peak_r_level: 0.0,
                cached_track_peak_levels: cached_track_peak_levels.clone(),
                cached_rack_slot_peak_levels: Vec::new(),
                cached_bus_peak_levels: cached_bus_peak_levels.clone(),
                cached_modulator_phases: Vec::new(),
                cached_modulator_levels: Vec::new(),
                cached_cpu_load_bits: 0.0f32.to_bits(),
                last_meter_poll_at: Instant::now(),
                last_cpu_ui_poll_at: Instant::now(),
                last_neural_visualization_poll_at: Instant::now(),
                visualization_liveness: VisualizationLiveness::default(),
                last_voice_count_log_at: Instant::now(),
            };
            let mut ctx_track_names = track_names.clone();
            // The dispatch context and the tick mirror below share this
            // FrameDiffState, exactly like the real loop: `switch-pattern`
            // stamps `prev_pattern_epoch` after its inline resync, and the
            // mirror's pattern-epoch branch must observe that stamp or it
            // would double-run the resync the handler already did.
            let mut frame_diff = FrameDiffState::default();
            frame_diff.prev_pattern_epoch =
                state.transport.pattern_epoch.load(Ordering::Relaxed);
            frame_diff.prev_song_row_mirror_epoch = app.song_row_mirror_epoch;
            frame_diff.prev_ui_epoch = ui_epoch.load(Ordering::Relaxed);
            frame_diff.prev_fx_epoch = fx_epoch.load(Ordering::Relaxed);
            frame_diff.prev_fx_value_epoch = fx_value_epoch.load(Ordering::Relaxed);
            frame_diff.prev_sound_binding_epoch = app.sound_binding_epoch;
            frame_diff.prev_delete_target_version =
                active_delete_target_version.load(Ordering::Relaxed);
            frame_diff.prev_track_button_states = track_button_state_snapshot(&state);
            frame_diff.prev_track_playheads = track_playheads_snapshot(&state, &app);

            let mut apply_host_commands = |editor: &mut Editor,
                                           app: &mut app::App,
                                           frame: &mut FrameDiffState,
                                           commands: Vec<HostCommand>|
             -> Vec<String> {
                let mut applied = Vec::new();
                for command in commands {
                    let HostCommand::Custom { name, payload } = command else {
                        continue;
                    };
                    let mut ctx = LoopCtx {
                        sessions: &mut sessions,
                        meters: &mut meters,
                        frame,
                        gesture: &mut gesture_state,
                        track_names: &mut ctx_track_names,
                        shared: &shared,
                    };
                    dispatch_custom_host_command(&name, payload, app, editor, &mut ctx);
                    applied.push(name);
                }
                applied
            };

            app.sync_track_sound_bindings();
            super::state_values::sync_song_state(
                editor.runtime_mut(),
                &app,
                &mut frame_diff.song,
                transport_visible,
            );
            let _ = super::state_values::sync_sound_palette(
                editor.runtime_mut(),
                &app,
                &mut frame_diff.sound_palette,
                false,
            );
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.update_tile_rects(vp_cols, vp_rows);

            let initial_frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                &mut editor,
                vp_cols as usize,
                vp_rows as usize,
            );
            let visible_buffers: Vec<String> = initial_frame
                .tiles
                .iter()
                .map(|tile| tile.frame.buffer_name.clone())
                .collect();
            for required in ["*sequencer*", "*fx*", "*mixer*", "*transport*"] {
                assert!(
                    visible_buffers.iter().any(|name| name == required),
                    "the production layout must show {required}, got {visible_buffers:?}"
                );
            }
            eprintln!("[{probe_prefix}-visible-buffers] {visible_buffers:?}");

            struct TileRetained {
                buffer_name: String,
                viewport: eseqlisp::widget_render::WidgetViewport,
                runs: Vec<eseqlisp::widget_render::MetalPrimitiveRun>,
                indices: eseqlisp::widget_render::MetalPrimitiveRunIndex,
            }
            let mut tile_retained: Vec<TileRetained> = Vec::new();
            for tile in &initial_frame.tiles {
                let layout = tile.frame.widget_layout.as_ref().unwrap_or_else(|| {
                    panic!(
                        "visible tile {} must have a widget layout",
                        tile.frame.buffer_name
                    )
                });
                let viewport = eseqlisp::widget_render::WidgetViewport {
                    cell_w: 8.0,
                    cell_h: 16.0,
                    vp_w: vp_cols as f32 * 8.0,
                    vp_h: vp_rows as f32 * 16.0,
                    time_seconds: 0.0,
                    focused_widget_id: tile.frame.focused_widget_id,
                    focused_branch: tile.is_active,
                    overlay_viewport_bottom: vp_rows as f32,
                    scroll_top: tile.frame.widget_scroll_top + tile.frame.text_scroll_top as f32,
                    scroll_left: tile.frame.widget_layout_scroll_left,
                    inherited_hover: false,
                };
                let (runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                    layout,
                    viewport,
                    viewport.scroll_top,
                    vp_rows,
                );
                let indices = eseqlisp::widget_render::build_metal_primitive_run_index(&runs);
                tile_retained.push(TileRetained {
                    buffer_name: tile.frame.buffer_name.clone(),
                    viewport,
                    runs,
                    indices,
                });
            }

            // --- visible update: the real reactive tick, minus the render --
            let neural = selected_neural_neurons.lock().unwrap().clone();

            struct LaunchUpdate {
                tick_sync_ms: f64,
                invalidation_ms: f64,
                pattern_sync_ms: f64,
                epoch_sync_ms: f64,
                reactive_ms: f64,
                frame_ms: f64,
                retained_ms: f64,
                reactive_cycle_ms: f64,
                side_effects_ms: f64,
                layout_refresh_after_ms: f64,
                inactive_layout_refresh_ms: f64,
                inactive_layout_refresh_count: usize,
                pattern_epoch_fired: bool,
                ui_epoch_fired: bool,
                fx_epoch_fired: bool,
                tiles: Vec<(String, usize, f64, bool)>,
            }

            let mut finish_visible_update = |editor: &mut Editor,
                                             app: &mut app::App,
                                             frame: &mut FrameDiffState,
                                             tiles: &mut Vec<TileRetained>|
             -> LaunchUpdate {
                let started = Instant::now();
                // reactive_tick.rs order: sound bindings (+ binding-epoch ->
                // fx_epoch bump), song state, sound palette (which carries
                // the mixer pattern-cell glyph sweep a launch dirties), then
                // typed invalidations, then the epoch-driven resyncs.
                app.sync_track_sound_bindings();
                if app.sound_binding_epoch != frame.prev_sound_binding_epoch {
                    frame.prev_sound_binding_epoch = app.sound_binding_epoch;
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                }
                super::state_values::sync_song_state(
                    editor.runtime_mut(),
                    app,
                    &mut frame.song,
                    transport_visible,
                );
                let _ = super::state_values::sync_sound_palette(
                    editor.runtime_mut(),
                    app,
                    &mut frame.sound_palette,
                    false,
                );
                let tick_sync_done = Instant::now();
                let ct = current_track.load(Ordering::Relaxed);
                let invalidations = ui_invalidations.drain();
                if !invalidations.is_empty() {
                    apply_ui_invalidations(
                        invalidations,
                        UiInvalidationApplyCtx {
                            app,
                            editor,
                            state: &state,
                            track_collapsed: &track_collapsed,
                            bus_state: &bus_state,
                            current_track_idx: ct,
                            selected_steps: &selected_steps,
                            selected_neural_neurons: &neural,
                            piano_roll_selection: &piano_roll_selection,
                            accumulator_names: &accumulator_names,
                            cached_track_peak_levels: &cached_track_peak_levels,
                            cached_bus_peak_levels: &cached_bus_peak_levels,
                            record_armed: &record_armed,
                            active_delete_target: &active_delete_target,
                            active_delete_target_version: &active_delete_target_version,
                            expanded_step_projection: &expanded_step_projection,
                            fx_visible,
                            sequencer_visible: true,
                            mixer_visible,
                        },
                    );
                }
                let invalidations_done = Instant::now();

                let build_revision = |state: &Arc<SequencerState>,
                                      app: &app::App|
                 -> super::loop_ctx::ParamSyncRevision {
                    let mut sorted_steps: Vec<usize> =
                        selected_steps.lock().unwrap().iter().copied().collect();
                    sorted_steps.sort_unstable();
                    super::loop_ctx::ParamSyncRevision {
                        track: ct,
                        scene: state.current_scene_index(),
                        pattern_epoch: state.transport.pattern_epoch.load(Ordering::Relaxed),
                        song_row_mirror_epoch: app.song_row_mirror_epoch,
                        ui_epoch: ui_epoch.load(Ordering::Relaxed),
                        fx_epoch: fx_epoch.load(Ordering::Relaxed),
                        sound_binding_epoch: app.sound_binding_epoch,
                        display_step: displayed_plock_step(
                            state,
                            ct,
                            sorted_steps.first().copied(),
                        ),
                        selected_steps: sorted_steps,
                        selected_neural_neurons: neural.iter().copied().collect(),
                    }
                };

                // --- reactive_tick.rs pattern-epoch branch -----------------
                // This is the branch a clip launch (`set-scene-cell`) leans
                // on for its project resync; `switch-pattern` pre-stamps
                // `prev_pattern_epoch` so scene launches skip it.
                let epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
                let mirror_epoch = app.song_row_mirror_epoch;
                let pattern_epoch_fired = (epoch != frame.prev_pattern_epoch
                    || mirror_epoch != frame.prev_song_row_mirror_epoch)
                    && !app.tracks.is_empty();
                if pattern_epoch_fired {
                    let revision = build_revision(&state, app);
                    let rt = editor.runtime_mut();
                    sync_shared_track_collapsed(&track_collapsed, app);
                    sync_track_name_state(rt, &mut track_names, app);
                    sync_pattern_state(rt, &state);
                    sync_selected_neural_neuron_bindings(rt, &state, &neural);
                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                    sync_all_track_sequencer_state(rt, &state, app, ct, &selected_steps);
                    let _ = sync_all_expanded_step_viewports(
                        rt,
                        &state,
                        app,
                        &selected_steps,
                        ct,
                        &expanded_step_projection,
                    );
                    sync_piano_roll_state(rt, app, &state, ct, &piano_roll_selection);
                    sync_step_param_lists(rt, &state, ct);
                    sync_track_mixer_state(rt, app, &state);
                    sync_bus_mixer_state(rt, app);
                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
                    if super::reactive_tick::claim_param_sync_revision(
                        &mut frame.track_param_sync_revision,
                        &revision,
                    ) {
                        sync_track_params_with_neural_selection(
                            rt,
                            app,
                            &state,
                            ct,
                            &selected_steps,
                            Some(&neural),
                        );
                    }
                    let _ = sync_track_plock_variant_preview(
                        rt,
                        app,
                        &state,
                        ct,
                        &selected_steps,
                        None,
                    );
                    if super::reactive_tick::claim_param_sync_revision(
                        &mut frame.fx_param_sync_revision,
                        &revision,
                    ) {
                        let _ = sync_fx_param_binding_fields_with_neural_selection(
                            rt,
                            app,
                            &state,
                            ct,
                            &selected_steps,
                            Some(&neural),
                        );
                    }
                    rt.set_reactive(
                        "SEQ",
                        "step-has-plocks",
                        build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                    );
                    sync_sidebar_browser(rt, app, ct);
                    frame.prev_pattern_epoch = epoch;
                    frame.prev_song_row_mirror_epoch = mirror_epoch;
                    frame.prev_track_button_states = track_button_state_snapshot(&state);
                }
                let pattern_sync_done = Instant::now();

                // --- reactive_tick.rs delete-target-version branch ---------
                let delete_version = active_delete_target_version.load(Ordering::Relaxed);
                if delete_version != frame.prev_delete_target_version {
                    frame.prev_delete_target_version = delete_version;
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "delete-target-version",
                        Value::Number(delete_version as f64),
                    );
                    sync_mixer_delete_target_binding_fields(
                        rt,
                        app.tracks.len(),
                        &state,
                        active_delete_target.lock().unwrap().as_ref(),
                    );
                }

                // --- reactive_tick.rs ui_epoch / fx_epoch branches ---------
                let ui_ep = ui_epoch.load(Ordering::Relaxed);
                let fx_ep = fx_epoch.load(Ordering::Relaxed);
                let fx_value_ep = fx_value_epoch.load(Ordering::Relaxed);
                let ui_epoch_fired = ui_ep != frame.prev_ui_epoch;
                let fx_epoch_fired = fx_visible
                    && (fx_ep != frame.prev_fx_epoch
                        || fx_value_ep != frame.prev_fx_value_epoch);
                // Mirrors the tick's structural-vs-value split: launches bump
                // fx_value_epoch (in-place patch), edits bump fx_epoch (full).
                let fx_structural = fx_ep != frame.prev_fx_epoch;
                if ui_epoch_fired {
                    let revision = build_revision(&state, app);
                    sync_shared_track_collapsed(&track_collapsed, app);
                    let rt = editor.runtime_mut();
                    sync_macro_state(rt, app);
                    sync_track_name_state(rt, &mut track_names, app);
                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                    sync_step_param_lists(rt, &state, ct);
                    sync_all_track_sequencer_state(rt, &state, app, ct, &selected_steps);
                    let _ = sync_all_expanded_step_viewports(
                        rt,
                        &state,
                        app,
                        &selected_steps,
                        ct,
                        &expanded_step_projection,
                    );
                    sync_track_mixer_state(rt, app, &state);
                    sync_bus_mixer_state(rt, app);
                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
                    if super::reactive_tick::claim_param_sync_revision(
                        &mut frame.track_param_sync_revision,
                        &revision,
                    ) {
                        sync_track_params_with_neural_selection(
                            rt,
                            app,
                            &state,
                            ct,
                            &selected_steps,
                            Some(&neural),
                        );
                    }
                    let _ = sync_track_plock_variant_preview(
                        rt,
                        app,
                        &state,
                        ct,
                        &selected_steps,
                        None,
                    );
                    if super::reactive_tick::claim_param_sync_revision(
                        &mut frame.fx_param_sync_revision,
                        &revision,
                    ) {
                        let _ = sync_fx_param_binding_fields_with_neural_selection(
                            rt,
                            app,
                            &state,
                            ct,
                            &selected_steps,
                            Some(&neural),
                        );
                    }
                    rt.set_reactive(
                        "SEQ",
                        "selected-steps",
                        build_selection_value(&selected_steps),
                    );
                    sync_piano_roll_state(rt, app, &state, ct, &piano_roll_selection);
                    rt.set_reactive(
                        "SEQ",
                        "step-has-plocks",
                        build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                    );
                    sync_mixer_delete_target_binding_fields(
                        rt,
                        app.tracks.len(),
                        &state,
                        active_delete_target.lock().unwrap().as_ref(),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "record-armed",
                        build_record_armed_value(&record_armed.lock().unwrap()),
                    );
                    frame.prev_track_button_states = track_button_state_snapshot(&state);
                    frame.prev_ui_epoch = ui_ep;
                }
                if fx_epoch_fired {
                    let rt = editor.runtime_mut();
                    let publish = |rt: &mut Runtime, field: &str, value: Value| {
                        if fx_structural {
                            rt.set_reactive("SEQ", field, value);
                        } else {
                            rt.set_reactive_value_patch("SEQ", field, value);
                        }
                    };
                    publish(
                        rt,
                        "effects",
                        build_effects_value(
                            &state,
                            ct,
                            &app.graph.effect_descriptors,
                            &selected_steps,
                        ),
                    );
                    publish(
                        rt,
                        "midi-effects",
                        build_midi_effects_value(&state, ct, &selected_steps),
                    );
                    publish(
                        rt,
                        "instrument-panel",
                        build_instrument_panel_value(app, ct, &selected_steps),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "step-has-plocks",
                        build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                    );
                    publish(
                        rt,
                        "bus-effects",
                        build_bus_effects_value_for_selection(app, Some(&selected_steps)),
                    );
                    frame.prev_fx_epoch = fx_ep;
                    frame.prev_fx_value_epoch = fx_value_ep;
                }
                let epoch_sync_done = Instant::now();

                editor.runtime_mut().run_reactive_cycle();
                let cycle_done = Instant::now();
                editor.refresh_runtime_side_effects();
                let side_effects_done = Instant::now();
                let (inactive_layout_refresh_ms, inactive_layout_refresh_count) = {
                    let timings = editor.last_layout_refresh_timings();
                    (
                        timings
                            .iter()
                            .map(|timing| timing.elapsed.as_secs_f64() * 1000.0)
                            .sum::<f64>(),
                        timings.len(),
                    )
                };
                // reactive_tick.rs post-cycle refreshes: the mixer after a
                // pattern-epoch resync, the sequencer after a ui-epoch one.
                if pattern_epoch_fired && mixer_visible {
                    editor.refresh_visible_layouts_for_buffer_named("*mixer*");
                }
                if ui_epoch_fired {
                    editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                }
                let reactive_done = Instant::now();
                let frame_built = eseqlisp::frame::build_tiled_render_frame_borderless(
                    editor,
                    vp_cols as usize,
                    vp_rows as usize,
                );
                let frame_done = Instant::now();
                assert_eq!(
                    frame_built.tiles.len(),
                    tiles.len(),
                    "the production layout must keep every tile visible"
                );
                let mut tile_stats = Vec::with_capacity(frame_built.tiles.len());
                for tile in &frame_built.tiles {
                    let entry = tiles
                        .iter_mut()
                        .find(|entry| entry.buffer_name == tile.frame.buffer_name)
                        .unwrap_or_else(|| {
                            panic!("retained runs for visible tile {}", tile.frame.buffer_name)
                        });
                    let layout = tile.frame.widget_layout.as_ref().unwrap_or_else(|| {
                        panic!(
                            "visible tile {} must keep a widget layout",
                            tile.frame.buffer_name
                        )
                    });
                    let tile_started = Instant::now();
                    let (_, stats) =
                        eseqlisp::widget_render::refresh_metal_primitive_runs_retained_in_place(
                            layout,
                            entry.viewport,
                            entry.viewport.scroll_top,
                            vp_rows,
                            &mut entry.runs,
                            &entry.indices,
                            &tile.frame.dirty_widget_ids,
                        );
                    let structural_rebuild =
                        stats.missing_previous_runs > 0 || stats.invalid_previous_runs > 0;
                    if structural_rebuild {
                        let (runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                            layout,
                            entry.viewport,
                            entry.viewport.scroll_top,
                            vp_rows,
                        );
                        entry.indices =
                            eseqlisp::widget_render::build_metal_primitive_run_index(&runs);
                        entry.runs = runs;
                    }
                    tile_stats.push((
                        tile.frame.buffer_name.clone(),
                        tile.frame.dirty_widget_ids.len(),
                        duration_ms(tile_started.elapsed()),
                        structural_rebuild,
                    ));
                }
                let retained_done = Instant::now();
                LaunchUpdate {
                    tick_sync_ms: duration_ms(tick_sync_done - started),
                    invalidation_ms: duration_ms(invalidations_done - tick_sync_done),
                    pattern_sync_ms: duration_ms(pattern_sync_done - invalidations_done),
                    epoch_sync_ms: duration_ms(epoch_sync_done - pattern_sync_done),
                    reactive_ms: duration_ms(reactive_done - epoch_sync_done),
                    frame_ms: duration_ms(frame_done - reactive_done),
                    retained_ms: duration_ms(retained_done - frame_done),
                    reactive_cycle_ms: duration_ms(cycle_done - epoch_sync_done),
                    side_effects_ms: duration_ms(side_effects_done - cycle_done),
                    layout_refresh_after_ms: duration_ms(reactive_done - side_effects_done),
                    inactive_layout_refresh_ms,
                    inactive_layout_refresh_count,
                    pattern_epoch_fired,
                    ui_epoch_fired,
                    fx_epoch_fired,
                    tiles: tile_stats,
                }
            };

            let percentile = |samples: &mut Vec<f64>, fraction: f64| {
                samples.sort_by(|a, b| a.total_cmp(b));
                let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
                samples[index]
            };

            struct LaunchSamples {
                total: Vec<f64>,
                dispatch: Vec<f64>,
                host: Vec<f64>,
                tick_sync: Vec<f64>,
                invalidation: Vec<f64>,
                pattern_sync: Vec<f64>,
                epoch_sync: Vec<f64>,
                reactive: Vec<f64>,
                frame: Vec<f64>,
                retained: Vec<f64>,
                reactive_cycle: Vec<f64>,
                side_effects: Vec<f64>,
                layout_refresh_after: Vec<f64>,
                inactive_layout_refresh: Vec<f64>,
                pattern_epoch_updates: usize,
                ui_epoch_updates: usize,
                fx_epoch_updates: usize,
                tile_retained: std::collections::BTreeMap<String, Vec<f64>>,
                tile_rebuilds: std::collections::BTreeMap<String, usize>,
                tile_dirty: std::collections::BTreeMap<String, usize>,
            }
            impl LaunchSamples {
                fn new() -> Self {
                    Self {
                        total: Vec::new(),
                        dispatch: Vec::new(),
                        host: Vec::new(),
                        tick_sync: Vec::new(),
                        invalidation: Vec::new(),
                        pattern_sync: Vec::new(),
                        epoch_sync: Vec::new(),
                        reactive: Vec::new(),
                        frame: Vec::new(),
                        retained: Vec::new(),
                        reactive_cycle: Vec::new(),
                        side_effects: Vec::new(),
                        layout_refresh_after: Vec::new(),
                        inactive_layout_refresh: Vec::new(),
                        pattern_epoch_updates: 0,
                        ui_epoch_updates: 0,
                        fx_epoch_updates: 0,
                        tile_retained: std::collections::BTreeMap::new(),
                        tile_rebuilds: std::collections::BTreeMap::new(),
                        tile_dirty: std::collections::BTreeMap::new(),
                    }
                }
                fn record(
                    &mut self,
                    total_ms: f64,
                    dispatch_ms: f64,
                    host_ms: f64,
                    update: &LaunchUpdate,
                ) {
                    self.total.push(total_ms);
                    self.dispatch.push(dispatch_ms);
                    self.host.push(host_ms);
                    self.tick_sync.push(update.tick_sync_ms);
                    self.invalidation.push(update.invalidation_ms);
                    self.pattern_sync.push(update.pattern_sync_ms);
                    self.epoch_sync.push(update.epoch_sync_ms);
                    self.reactive.push(update.reactive_ms);
                    self.frame.push(update.frame_ms);
                    self.retained.push(update.retained_ms);
                    self.reactive_cycle.push(update.reactive_cycle_ms);
                    self.side_effects.push(update.side_effects_ms);
                    self.layout_refresh_after.push(update.layout_refresh_after_ms);
                    self.inactive_layout_refresh
                        .push(update.inactive_layout_refresh_ms);
                    if update.pattern_epoch_fired {
                        self.pattern_epoch_updates += 1;
                    }
                    if update.ui_epoch_fired {
                        self.ui_epoch_updates += 1;
                    }
                    if update.fx_epoch_fired {
                        self.fx_epoch_updates += 1;
                    }
                    for (name, dirty, retained_ms, rebuilt) in &update.tiles {
                        self.tile_retained
                            .entry(name.clone())
                            .or_default()
                            .push(*retained_ms);
                        *self.tile_dirty.entry(name.clone()).or_default() += *dirty;
                        if *rebuilt {
                            *self.tile_rebuilds.entry(name.clone()).or_default() += 1;
                        }
                    }
                }
            }

            struct ScenarioReport {
                label: String,
                median_ms: f64,
                dispatch_ms: f64,
                host_ms: f64,
                pattern_epoch_updates: usize,
                ui_epoch_updates: usize,
                fx_epoch_updates: usize,
            }
            let mut scenario_reports: Vec<ScenarioReport> = Vec::new();
            let probe_track_count = app.tracks.len();
            let probe_scene_count = state.scene_count();
            let report_scenario = |label: &str,
                                   samples: &mut LaunchSamples,
                                   reports: &mut Vec<ScenarioReport>| {
                let median = percentile(&mut samples.total, 0.50);
                let dispatch_median = percentile(&mut samples.dispatch, 0.50);
                let host_median = percentile(&mut samples.host, 0.50);
                eprintln!(
                    "[{probe_prefix}-{label}] tracks={} scenes={} samples={SAMPLES} median_ms={:.3} p95_ms={:.3} input_ms={:.3} host_ms={:.3} visible_update_ms={:.3} pattern_epoch_updates={}/{SAMPLES} ui_epoch_updates={}/{SAMPLES} fx_epoch_updates={}/{SAMPLES}",
                    probe_track_count,
                    probe_scene_count,
                    median,
                    percentile(&mut samples.total, 0.95),
                    dispatch_median,
                    host_median,
                    median - dispatch_median - host_median,
                    samples.pattern_epoch_updates,
                    samples.ui_epoch_updates,
                    samples.fx_epoch_updates,
                );
                eprintln!(
                    "[{probe_prefix}-{label}-phases] tick_sync_ms={:.3} invalidation_ms={:.3} pattern_sync_ms={:.3} epoch_sync_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
                    percentile(&mut samples.tick_sync, 0.50),
                    percentile(&mut samples.invalidation, 0.50),
                    percentile(&mut samples.pattern_sync, 0.50),
                    percentile(&mut samples.epoch_sync, 0.50),
                    percentile(&mut samples.reactive, 0.50),
                    percentile(&mut samples.frame, 0.50),
                    percentile(&mut samples.retained, 0.50),
                );
                eprintln!(
                    "[{probe_prefix}-{label}-reactive-detail] cycle_ms={:.3} side_effects_ms={:.3} post_cycle_layout_refresh_ms={:.3} inactive_layout_refresh_ms={:.3}",
                    percentile(&mut samples.reactive_cycle, 0.50),
                    percentile(&mut samples.side_effects, 0.50),
                    percentile(&mut samples.layout_refresh_after, 0.50),
                    percentile(&mut samples.inactive_layout_refresh, 0.50),
                );
                let tile_rebuilds = samples.tile_rebuilds.clone();
                let tile_dirty_totals = samples.tile_dirty.clone();
                let tile_breakdown = samples
                    .tile_retained
                    .iter_mut()
                    .map(|(tile, tile_samples)| {
                        format!(
                            "{tile}={:.3}(dirty={} rebuilds={})",
                            percentile(tile_samples, 0.50),
                            tile_dirty_totals.get(tile).copied().unwrap_or(0),
                            tile_rebuilds.get(tile).copied().unwrap_or(0),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                eprintln!("[{probe_prefix}-{label}-retained-tiles] {tile_breakdown}");
                reports.push(ScenarioReport {
                    label: label.to_string(),
                    median_ms: median,
                    dispatch_ms: dispatch_median,
                    host_ms: host_median,
                    pattern_epoch_updates: samples.pattern_epoch_updates,
                    ui_epoch_updates: samples.ui_epoch_updates,
                    fx_epoch_updates: samples.fx_epoch_updates,
                });
            };

            // Screen-space centers of two of TRACK's mixer pattern cells, so
            // alternating clicks always launch a different clip (a re-launch
            // of the active clip would be a NoOp on the model).
            let locate_cell_targets = |editor: &mut Editor| -> Vec<(u64, f32, f32)> {
                let frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                    editor,
                    vp_cols as usize,
                    vp_rows as usize,
                );
                let mixer_tile = frame
                    .tiles
                    .iter()
                    .find(|tile| tile.frame.buffer_name == "*mixer*")
                    .expect("visible mixer tile");
                let origin_col = mixer_tile.body_rect.col.floor();
                let origin_row = mixer_tile.body_rect.row.floor();
                let scroll_top = mixer_tile.frame.widget_scroll_top
                    + mixer_tile.frame.text_scroll_top as f32;
                let scroll_left = mixer_tile.frame.widget_layout_scroll_left;
                let body = mixer_tile.body_rect;
                let layout = mixer_tile
                    .frame
                    .widget_layout
                    .as_ref()
                    .expect("mixer tile layout");
                let cells = state.track_pattern_cells(TRACK);
                assert!(
                    cells.len() >= 2,
                    "track {TRACK} must expose at least two pool clips, got {}",
                    cells.len()
                );
                let mut targets = Vec::new();
                for cell in &cells {
                    let key = format!(
                        "/track-pattern-cell-{TRACK}-{}",
                        cell.pattern_id.0
                    );
                    let Some(node) = find_layout_node_by_stable_key_suffix(layout, &key) else {
                        continue;
                    };
                    let center_col =
                        origin_col + node.rect.col + node.rect.width * 0.5 - scroll_left;
                    let center_row =
                        origin_row + node.rect.row + node.rect.height * 0.5 - scroll_top;
                    if center_col >= body.col
                        && center_col < body.col + body.width
                        && center_row >= body.row
                        && center_row < body.row + body.height
                    {
                        targets.push((cell.pattern_id.0, center_col, center_row));
                    }
                    if targets.len() == 2 {
                        break;
                    }
                }
                assert!(
                    targets.len() == 2,
                    "the mixer strip must show at least two on-screen pattern cells for \
                     track {TRACK} (pool has {} clips)",
                    cells.len()
                );
                targets
            };

            // --- scenario: clip launch (mixer pattern cell -> set-scene-cell)
            {
                // Pre-warm click, untimed: focuses the mixer tile and pays
                // one-time first-interaction costs.
                let targets = locate_cell_targets(&mut editor);
                let (_, warm_col, warm_row) = targets[0];
                for kind in [
                    MouseEventKind::Down(MouseButton::Left),
                    MouseEventKind::Up(MouseButton::Left),
                ] {
                    editor.handle_tiled_mouse_precise(
                        mouse_event(kind, warm_col.floor() as u16, warm_row.floor() as u16),
                        warm_col,
                        warm_row,
                        0,
                    );
                    let commands = editor.drain_host_commands();
                    apply_host_commands(&mut editor, &mut app, &mut frame_diff, commands);
                }
                finish_visible_update(&mut editor, &mut app, &mut frame_diff, &mut tile_retained);
                assert_eq!(
                    editor.active_buffer().name,
                    "*mixer*",
                    "clicking a pattern cell must focus the mixer tile"
                );

                let mut samples = LaunchSamples::new();
                let mut set_scene_cell_dispatches = 0usize;
                for iteration in 0..(WARMUPS + SAMPLES) {
                    // Re-locate each iteration: a launch can restructure the
                    // mixer strip layout (active markers, glyph frames).
                    let targets = locate_cell_targets(&mut editor);
                    let (pattern_id, col, row) = targets[1 - (iteration % 2)];

                    let started = Instant::now();
                    editor.handle_tiled_mouse_precise(
                        mouse_event(
                            MouseEventKind::Down(MouseButton::Left),
                            col.floor() as u16,
                            row.floor() as u16,
                        ),
                        col,
                        row,
                        0,
                    );
                    let commands = editor.drain_host_commands();
                    let dispatch_done = Instant::now();
                    assert!(
                        !commands.is_empty(),
                        "clip-launch click {iteration} must emit host commands"
                    );
                    let applied =
                        apply_host_commands(&mut editor, &mut app, &mut frame_diff, commands);
                    let host_done = Instant::now();
                    set_scene_cell_dispatches += applied
                        .iter()
                        .filter(|name| name.as_str() == "set-scene-cell")
                        .count();
                    assert!(
                        applied.iter().any(|name| name == "set-scene-cell"),
                        "clip-launch click {iteration} must dispatch set-scene-cell, got {applied:?}"
                    );
                    let update = finish_visible_update(
                        &mut editor,
                        &mut app,
                        &mut frame_diff,
                        &mut tile_retained,
                    );
                    if iteration == 0 && pattern_switch_profile_enabled() {
                        if let Some(trace) = editor.runtime().last_ui_invalidation_trace() {
                            let mut timings = trace.reactive_exec_timings.clone();
                            timings.sort_by(|a, b| b.1.cmp(&a.1));
                            timings.truncate(12);
                            eprintln!(
                                "[{probe_prefix}-clip-launch-dirty-fields] {:?}",
                                trace.dirty_fields
                            );
                            eprintln!(
                                "[{probe_prefix}-clip-launch-trace] affected={:?} full_reruns={} subtree_reruns={} apply_ms={:.2} flush_ms={:.2} top_effects={:?}",
                                trace.affected_buffers,
                                trace.full_buffer_reruns,
                                trace.subtree_reruns,
                                trace.reactive_apply_duration.as_secs_f64() * 1000.0,
                                trace.reactive_flush_duration.as_secs_f64() * 1000.0,
                                timings
                                    .iter()
                                    .map(|(name, duration)| (
                                        name.clone(),
                                        (duration.as_secs_f64() * 1000.0 * 100.0).round()
                                            / 100.0
                                    ))
                                    .collect::<Vec<_>>(),
                            );
                        }
                    }
                    let total_ms = duration_ms(started.elapsed());
                    assert_eq!(
                        state.scene_track_pattern_id(state.current_scene_index(), TRACK),
                        Some(PatternId(pattern_id)),
                        "clip-launch click {iteration} must assign pattern {pattern_id} \
                         into the current scene's cell"
                    );
                    assert!(
                        update.pattern_epoch_fired || update.ui_epoch_fired,
                        "clip-launch click {iteration} must trigger a project resync \
                         (neither pattern nor ui epoch fired)"
                    );
                    let mixer_dirty = update
                        .tiles
                        .iter()
                        .find(|(tile, _, _, _)| tile == "*mixer*")
                        .map(|(_, dirty, _, _)| *dirty)
                        .expect("mixer tile stats");
                    assert!(
                        mixer_dirty > 0,
                        "clip-launch click {iteration} must dirty widgets in the mixer strip"
                    );
                    if iteration >= WARMUPS {
                        samples.record(
                            total_ms,
                            duration_ms(dispatch_done - started),
                            duration_ms(host_done - dispatch_done),
                            &update,
                        );
                    }
                    // Close the pointer outside the timed region.
                    editor.handle_tiled_mouse_precise(
                        mouse_event(
                            MouseEventKind::Up(MouseButton::Left),
                            col.floor() as u16,
                            row.floor() as u16,
                        ),
                        col,
                        row,
                        0,
                    );
                    let commands = editor.drain_host_commands();
                    apply_host_commands(&mut editor, &mut app, &mut frame_diff, commands);
                }
                assert_eq!(
                    set_scene_cell_dispatches,
                    WARMUPS + SAMPLES,
                    "every clip-launch click must reach the real set-scene-cell handler"
                );
                report_scenario("clip-launch", &mut samples, &mut scenario_reports);
            }

            // --- scenario: scene launch (transport pill -> switch-pattern) --
            {
                let locate_pill = |editor: &mut Editor, scene: usize| -> (f32, f32) {
                    let frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                        editor,
                        vp_cols as usize,
                        vp_rows as usize,
                    );
                    let transport_tile = frame
                        .tiles
                        .iter()
                        .find(|tile| tile.frame.buffer_name == "*transport*")
                        .expect("visible transport tile");
                    let origin_col = transport_tile.body_rect.col.floor();
                    let origin_row = transport_tile.body_rect.row.floor();
                    let scroll_top = transport_tile.frame.widget_scroll_top
                        + transport_tile.frame.text_scroll_top as f32;
                    let scroll_left = transport_tile.frame.widget_layout_scroll_left;
                    let body = transport_tile.body_rect;
                    let layout = transport_tile
                        .frame
                        .widget_layout
                        .as_ref()
                        .expect("transport tile layout");
                    // ui/transport.lisp is `eseq.transport`, so the pill's
                    // `:key` renders qualified (`eseq.transport/…`).
                    let node = find_layout_node_by_stable_key_suffix(
                        layout,
                        &format!("/transport-scene-pill-{scene}"),
                    )
                    .unwrap_or_else(|| panic!("visible transport scene pill {scene}"));
                    let center_col =
                        origin_col + node.rect.col + node.rect.width * 0.5 - scroll_left;
                    let center_row =
                        origin_row + node.rect.row + node.rect.height * 0.5 - scroll_top;
                    assert!(
                        center_col >= body.col
                            && center_col < body.col + body.width
                            && center_row >= body.row
                            && center_row < body.row + body.height,
                        "scene pill {scene} must be on screen inside the transport tile: \
                         center=({center_col:.2},{center_row:.2}) body=({:.2},{:.2},{:.2},{:.2}) \
                         node_rect=({:.2},{:.2},{:.2},{:.2}) scroll=({scroll_left:.2},{scroll_top:.2})",
                        body.col,
                        body.row,
                        body.width,
                        body.height,
                        node.rect.col,
                        node.rect.row,
                        node.rect.width,
                        node.rect.height,
                    );
                    (center_col, center_row)
                };

                assert!(
                    state.scene_count() >= 3,
                    "the scene-launch scenario alternates between scenes 1 and 2"
                );
                // The transport tile keeps a startup-sized cached layout until
                // something refreshes it; re-lay it out at its real tile size
                // so the pill rects match what the tile shows.
                editor.refresh_visible_layouts_for_buffer_named("*transport*");
                let mut samples = LaunchSamples::new();
                let mut switch_dispatches = 0usize;
                for iteration in 0..(WARMUPS + SAMPLES) {
                    // Alternate scenes so every launch changes the scene.
                    let scene = 1 + (iteration % 2);
                    let (col, row) = locate_pill(&mut editor, scene);

                    let started = Instant::now();
                    editor.handle_tiled_mouse_precise(
                        mouse_event(
                            MouseEventKind::Down(MouseButton::Left),
                            col.floor() as u16,
                            row.floor() as u16,
                        ),
                        col,
                        row,
                        0,
                    );
                    let commands = editor.drain_host_commands();
                    let dispatch_done = Instant::now();
                    assert!(
                        !commands.is_empty(),
                        "scene-launch click {iteration} must emit host commands"
                    );
                    let applied =
                        apply_host_commands(&mut editor, &mut app, &mut frame_diff, commands);
                    let host_done = Instant::now();
                    switch_dispatches += applied
                        .iter()
                        .filter(|name| name.as_str() == "switch-pattern")
                        .count();
                    assert!(
                        applied.iter().any(|name| name == "switch-pattern"),
                        "scene-launch click {iteration} must dispatch switch-pattern, got {applied:?}"
                    );
                    if iteration == 0 && pattern_switch_profile_enabled() {
                        if let Some(trace) = editor.runtime().last_ui_invalidation_trace() {
                            let mut timings = trace.reactive_exec_timings.clone();
                            timings.sort_by(|a, b| b.1.cmp(&a.1));
                            timings.truncate(12);
                            eprintln!(
                                "[{probe_prefix}-scene-launch-dirty-fields] {:?}",
                                trace.dirty_fields
                            );
                            eprintln!(
                                "[{probe_prefix}-scene-launch-trace] dirty_fields={} affected={:?} full_reruns={} subtree_reruns={} apply_ms={:.2} flush_ms={:.2} top_effects={:?}",
                                trace.dirty_fields.len(),
                                trace.affected_buffers,
                                trace.full_buffer_reruns,
                                trace.subtree_reruns,
                                trace.reactive_apply_duration.as_secs_f64() * 1000.0,
                                trace.reactive_flush_duration.as_secs_f64() * 1000.0,
                                timings
                                    .iter()
                                    .map(|(name, duration)| (
                                        name.clone(),
                                        (duration.as_secs_f64() * 1000.0 * 100.0).round()
                                            / 100.0
                                    ))
                                    .collect::<Vec<_>>(),
                            );
                        }
                        let refreshes = editor
                            .last_layout_refresh_timings()
                            .iter()
                            .map(|timing| {
                                (
                                    timing.buffer_name.clone(),
                                    timing.mode.clone(),
                                    (timing.elapsed.as_secs_f64() * 1000.0 * 100.0).round()
                                        / 100.0,
                                )
                            })
                            .collect::<Vec<_>>();
                        eprintln!(
                            "[{probe_prefix}-scene-launch-layout-refreshes] {refreshes:?}"
                        );
                    }
                    let update = finish_visible_update(
                        &mut editor,
                        &mut app,
                        &mut frame_diff,
                        &mut tile_retained,
                    );
                    let total_ms = duration_ms(started.elapsed());
                    assert_eq!(
                        state.current_scene_index(),
                        scene,
                        "scene-launch click {iteration} must land on scene {scene}"
                    );
                    if iteration >= WARMUPS {
                        samples.record(
                            total_ms,
                            duration_ms(dispatch_done - started),
                            duration_ms(host_done - dispatch_done),
                            &update,
                        );
                    }
                    // Close the pointer outside the timed region (fires
                    // scene-push-end, a no-op for a plain click).
                    editor.handle_tiled_mouse_precise(
                        mouse_event(
                            MouseEventKind::Up(MouseButton::Left),
                            col.floor() as u16,
                            row.floor() as u16,
                        ),
                        col,
                        row,
                        0,
                    );
                    let commands = editor.drain_host_commands();
                    apply_host_commands(&mut editor, &mut app, &mut frame_diff, commands);
                }
                assert_eq!(
                    switch_dispatches,
                    WARMUPS + SAMPLES,
                    "every scene-launch click must reach the real switch-pattern handler"
                );
                report_scenario("scene-launch", &mut samples, &mut scenario_reports);
            }

            eprintln!(
                "[{probe_prefix}-comparison] {}",
                scenario_reports
                    .iter()
                    .map(|report| format!(
                        "{}={:.3}ms(input={:.3} host={:.3} pattern_epochs={} ui_epochs={} fx_epochs={})",
                        report.label,
                        report.median_ms,
                        report.dispatch_ms,
                        report.host_ms,
                        report.pattern_epoch_updates,
                        report.ui_epoch_updates,
                        report.fx_epoch_updates,
                    ))
                    .collect::<Vec<_>>()
                    .join(" ")
            );

            // Ceilings with headroom over the quiet-machine medians after the
            // 2026-08-05 pass (clip 57.9ms, scene 87.2ms; baseline was 119 /
            // 89). What remains is honest single-cycle work: the *fx* panel
            // re-evaluation for genuinely changed device state (~22ms), the
            // structural sequencer/mixer relayouts on scene change, and the
            // hidden *metal* buffer's whole-list rerun (~5ms) — each tracked
            // as follow-ups in the launch-perf memory.
            for report in &scenario_reports {
                let ceiling_ms = if report.label == "clip-launch" { 75.0 } else { 105.0 };
                assert!(
                    report.median_ms < ceiling_ms,
                    "{}: launch median {:.3}ms exceeded the {ceiling_ms:.0}ms ceiling",
                    report.label,
                    report.median_ms,
                );
            }
            let clip = scenario_reports
                .iter()
                .find(|report| report.label == "clip-launch")
                .expect("clip-launch report");
            // Regression guards for the epoch policy this pass established:
            // arming the launched cell as the delete target must NOT bump
            // ui_epoch (each bump re-syncs the whole project), and the fx
            // values must ride the tick's fx-epoch branch instead of an
            // inline handler resync (which cost a second reactive cycle).
            assert_eq!(
                clip.ui_epoch_updates, 0,
                "a clip launch must not bump ui_epoch — the pattern-epoch tick \
                 branch already carries the project resync"
            );
            assert_eq!(
                clip.pattern_epoch_updates, SAMPLES,
                "every clip launch must resync through the pattern-epoch tick branch"
            );
            return;
        }

        if probe == Project92UiProbe::StepInteractionsFullLayout {
            const TRACK: usize = 0;
            const STEP_COUNT: usize = 64;
            const WARMUPS: usize = 5;
            const SAMPLES: usize = 20;

            // Focus the tile that shows *sequencer* without disturbing the
            // production layout (set_active_buffer would swap the active
            // tile's buffer instead of switching tiles).
            let sequencer_buffer_idx = editor
                .buffers
                .iter()
                .position(|buffer| buffer.name == "*sequencer*")
                .expect("sequencer buffer index");
            let sequencer_tile = editor
                .tile_root
                .leaf_ids()
                .into_iter()
                .find(|tile_id| {
                    editor
                        .tile_root
                        .find_leaf(*tile_id)
                        .is_some_and(|leaf| leaf.buffer_idx == sequencer_buffer_idx)
                })
                .expect("the production layout must show *sequencer*");
            editor.switch_active_tile(sequencer_tile);
            assert_eq!(
                editor.active_buffer().name,
                "*sequencer*",
                "the full-layout probe must focus the sequencer tile"
            );

            state.pattern.track_params[TRACK].set_num_steps(STEP_COUNT);
            for step in 0..STEP_COUNT {
                state.pattern.patterns[TRACK].set_step_active(step, step < 24);
            }
            // Give step 0 a real instrument p-lock whose value differs from
            // the track's base value. Cmd+A displays the lowest selected step
            // (step 0) in the fx panel, so with the production layout the
            // p-lock publication path must visibly change the fx tile — and
            // Escape must revert it. Without any p-lock the fx panel would
            // legitimately have nothing to repaint and the probe could not
            // tell that path was skipped.
            {
                let instrument_desc = app
                    .graph
                    .instrument_descriptors
                    .get(TRACK)
                    .expect("track 0 instrument descriptor");
                let (plock_param_idx, plock_pdesc) = instrument_desc
                    .params
                    .iter()
                    .enumerate()
                    .find(|(_, pdesc)| {
                        matches!(
                            pdesc.kind,
                            sequencer::effects::ParamKind::Continuous { .. }
                        ) && pdesc.max > pdesc.min
                    })
                    .expect("track 0 must expose a continuous instrument param");
                let base = app
                    .effective_instrument_param_value(TRACK, plock_param_idx)
                    .unwrap_or(plock_pdesc.default);
                let plock_value = if (base - plock_pdesc.min).abs() >= (plock_pdesc.max - base).abs()
                {
                    plock_pdesc.min
                } else {
                    plock_pdesc.max
                };
                assert!(
                    (plock_value - base).abs() > f32::EPSILON,
                    "fixture p-lock value must differ from the base instrument value"
                );
                state.pattern.instrument_slots[TRACK].set_plock(0, plock_param_idx, plock_value);
            }
            sync_all_track_sequencer_state(
                editor.runtime_mut(),
                &state,
                &app,
                TRACK,
                &selected_steps,
            );

            // The production layout must make the selection side-effect
            // publication paths live: these flags gate the fx/mixer work in
            // apply_ui_invalidations and reactive_sync.rs, and the
            // single-tile probes leave them all false.
            let transport_visible = editor_has_visible_buffer(&editor, "*transport*");
            let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
            let mixer_visible = editor_has_visible_buffer(&editor, "*mixer*");
            assert!(fx_visible, "production layout must show the *fx* panel");
            assert!(mixer_visible, "production layout must show the *mixer* strip");
            assert!(
                transport_visible,
                "production layout must show the *transport* bar"
            );
            assert!(
                !editor_has_visible_buffer(&editor, "*arrangement*"),
                "the full-layout step probe must measure the Seq view"
            );

            let mut song_frame = super::state_values::SongFrameState::default();
            app.sync_track_sound_bindings();
            super::state_values::sync_song_state(
                editor.runtime_mut(),
                &app,
                &mut song_frame,
                transport_visible,
            );
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.update_tile_rects(vp_cols, vp_rows);

            // Tripwire: every step cell the Cmd+A/Escape repaint must cover
            // has to exist in the sequencer tile's layout at this viewport.
            {
                let layout = editor.widget_layout().expect("sequencer layout");
                for step in [0, 8, 15, 16, 31, 32, 47, 48, 63] {
                    assert!(
                        find_layout_node_by_stable_key_suffix(
                            &layout,
                            &format!("/step-cell-{TRACK}-{step}"),
                        )
                        .is_some(),
                        "step cell {step} must be present in the sequencer tile layout at {vp_cols}x{vp_rows}",
                    );
                }
            }

            let initial_frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                &mut editor,
                vp_cols as usize,
                vp_rows as usize,
            );
            let visible_buffers: Vec<String> = initial_frame
                .tiles
                .iter()
                .map(|tile| tile.frame.buffer_name.clone())
                .collect();
            assert!(
                visible_buffers.len() >= 3,
                "the production layout must keep at least 3 tiles visible, got {visible_buffers:?}"
            );
            for required in ["*sequencer*", "*fx*", "*mixer*", "*transport*"] {
                assert!(
                    visible_buffers.iter().any(|name| name == required),
                    "the production layout must show {required}, got {visible_buffers:?}"
                );
            }
            eprintln!("[project-92-fullayout-visible-buffers] {visible_buffers:?}");

            // Retained Metal runs for EVERY visible tile, mirroring what the
            // Metal backend keeps per widget scene. Refusing to track a tile
            // here would let an "optimization" cheat by dropping its redraw.
            struct TileRetained {
                buffer_name: String,
                viewport: eseqlisp::widget_render::WidgetViewport,
                runs: Vec<eseqlisp::widget_render::MetalPrimitiveRun>,
                indices: eseqlisp::widget_render::MetalPrimitiveRunIndex,
            }
            let mut tile_retained: Vec<TileRetained> = Vec::new();
            for tile in &initial_frame.tiles {
                let layout = tile.frame.widget_layout.as_ref().unwrap_or_else(|| {
                    panic!("visible tile {} must have a widget layout", tile.frame.buffer_name)
                });
                let viewport = eseqlisp::widget_render::WidgetViewport {
                    cell_w: 8.0,
                    cell_h: 16.0,
                    vp_w: vp_cols as f32 * 8.0,
                    vp_h: vp_rows as f32 * 16.0,
                    time_seconds: 0.0,
                    focused_widget_id: tile.frame.focused_widget_id,
                    focused_branch: tile.is_active,
                    overlay_viewport_bottom: vp_rows as f32,
                    scroll_top: tile.frame.widget_scroll_top
                        + tile.frame.text_scroll_top as f32,
                    scroll_left: tile.frame.widget_layout_scroll_left,
                    inherited_hover: false,
                };
                let (runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                    layout,
                    viewport,
                    viewport.scroll_top,
                    vp_rows,
                );
                let indices = eseqlisp::widget_render::build_metal_primitive_run_index(&runs);
                tile_retained.push(TileRetained {
                    buffer_name: tile.frame.buffer_name.clone(),
                    viewport,
                    runs,
                    indices,
                });
            }
            // The shell packs its state uniforms in compiled shader order, so
            // derive the `selected` slot from the registered widget def
            // instead of hard-coding an index.
            let shell_selected_idx = eseqlisp::widget_render::sdf_widget::sdf_widget_def(
                "seqv-step-shell",
            )
            .expect("seqv-step-shell widget def")
            .state_uniforms
            .iter()
            .position(|name| name == "selected")
            .expect("seqv-step-shell must expose a selected state uniform");
            let count_selected_shells = |tiles: &Vec<TileRetained>| -> usize {
                let read_uniform =
                    |instance: &eseqlisp::widget_render::WidgetInstance, idx: usize| -> f32 {
                        match idx {
                            0..=3 => instance.uniform_a[idx],
                            4..=7 => instance.uniform_b[idx - 4],
                            8..=11 => instance.uniform_c[idx - 8],
                            _ => instance.uniform_d[idx - 12],
                        }
                    };
                tiles
                    .iter()
                    .find(|tile| tile.buffer_name == "*sequencer*")
                    .expect("retained runs for the sequencer tile")
                    .runs
                    .iter()
                    .flat_map(|run| &run.primitives)
                    .filter(|primitive| {
                        matches!(
                            eseqlisp::widget_render::innermost_primitive(primitive),
                            eseqlisp::widget_render::MetalPrimitive::WidgetInstance {
                                widget_type,
                                instance,
                                ..
                            } if widget_type == "seqv-step-shell"
                                && read_uniform(instance, shell_selected_idx) > 0.5
                        )
                    })
                    .count()
            };

            struct FullLayoutUpdate {
                invalidation_ms: f64,
                tick_sync_ms: f64,
                reactive_ms: f64,
                frame_ms: f64,
                retained_ms: f64,
                // (buffer name, dirty widget count, retained refresh ms,
                //  structural full-rebuild fallback taken)
                tiles: Vec<(String, usize, f64, bool)>,
            }
            let step_clipboard = Arc::new(Mutex::new(None));
            let neural = selected_neural_neurons.lock().unwrap().clone();
            let mut finish_visible_update = |editor: &mut Editor,
                                             app: &mut app::App,
                                             tiles: &mut Vec<TileRetained>|
             -> FullLayoutUpdate {
                let started = Instant::now();
                let invalidations = ui_invalidations.drain();
                if !invalidations.is_empty() {
                    apply_ui_invalidations(
                        invalidations,
                        UiInvalidationApplyCtx {
                            app,
                            editor,
                            state: &state,
                            track_collapsed: &track_collapsed,
                            bus_state: &bus_state,
                            current_track_idx: TRACK,
                            selected_steps: &selected_steps,
                            selected_neural_neurons: &neural,
                            piano_roll_selection: &piano_roll_selection,
                            accumulator_names: &accumulator_names,
                            cached_track_peak_levels: &cached_track_peak_levels,
                            cached_bus_peak_levels: &cached_bus_peak_levels,
                            record_armed: &record_armed,
                            active_delete_target: &active_delete_target,
                            active_delete_target_version: &active_delete_target_version,
                            expanded_step_projection: &expanded_step_projection,
                            fx_visible,
                            sequencer_visible: true,
                            mixer_visible,
                        },
                    );
                }
                let invalidations_done = Instant::now();
                // The real event loop's reactive tick runs these song/sound
                // syncs on every frame before the reactive cycle
                // (reactive_tick.rs).
                app.sync_track_sound_bindings();
                super::state_values::sync_song_state(
                    editor.runtime_mut(),
                    app,
                    &mut song_frame,
                    transport_visible,
                );
                let tick_sync_done = Instant::now();
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                let reactive_done = Instant::now();
                let frame = eseqlisp::frame::build_tiled_render_frame_borderless(
                    editor,
                    vp_cols as usize,
                    vp_rows as usize,
                );
                let frame_done = Instant::now();
                assert_eq!(
                    frame.tiles.len(),
                    tiles.len(),
                    "the production layout must keep every tile visible"
                );
                let mut tile_stats = Vec::with_capacity(frame.tiles.len());
                for tile in &frame.tiles {
                    let entry = tiles
                        .iter_mut()
                        .find(|entry| entry.buffer_name == tile.frame.buffer_name)
                        .unwrap_or_else(|| {
                            panic!("retained runs for visible tile {}", tile.frame.buffer_name)
                        });
                    let layout = tile.frame.widget_layout.as_ref().unwrap_or_else(|| {
                        panic!(
                            "visible tile {} must keep a widget layout",
                            tile.frame.buffer_name
                        )
                    });
                    let tile_started = Instant::now();
                    let (_, stats) =
                        eseqlisp::widget_render::refresh_metal_primitive_runs_retained_in_place(
                            layout,
                            entry.viewport,
                            entry.viewport.scroll_top,
                            vp_rows,
                            &mut entry.runs,
                            &entry.indices,
                            &tile.frame.dirty_widget_ids,
                        );
                    let structural_rebuild = stats.missing_previous_runs > 0
                        || stats.invalid_previous_runs > 0;
                    if tile.frame.buffer_name == "*sequencer*" {
                        // The interaction's primary surface keeps the strict
                        // contract the single-tile probes enforce.
                        assert_eq!(
                            stats.missing_previous_runs, 0,
                            "sequencer tile retained refresh must not miss runs"
                        );
                        assert_eq!(
                            stats.invalid_previous_runs, 0,
                            "sequencer tile retained refresh must not invalidate runs"
                        );
                    } else if structural_rebuild {
                        // Production fallback: when a tile's widget structure
                        // changed, the Metal backend rebuilds that tile's run
                        // scene in full (metal_backend.rs,
                        // refresh_widget_run_scene_for_dirty_layout). That
                        // real cost stays inside the timed region.
                        let (runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                            layout,
                            entry.viewport,
                            entry.viewport.scroll_top,
                            vp_rows,
                        );
                        entry.indices =
                            eseqlisp::widget_render::build_metal_primitive_run_index(&runs);
                        entry.runs = runs;
                    }
                    tile_stats.push((
                        tile.frame.buffer_name.clone(),
                        tile.frame.dirty_widget_ids.len(),
                        duration_ms(tile_started.elapsed()),
                        structural_rebuild,
                    ));
                }
                let retained_done = Instant::now();
                FullLayoutUpdate {
                    invalidation_ms: duration_ms(invalidations_done - started),
                    tick_sync_ms: duration_ms(tick_sync_done - invalidations_done),
                    reactive_ms: duration_ms(reactive_done - tick_sync_done),
                    frame_ms: duration_ms(frame_done - reactive_done),
                    retained_ms: duration_ms(retained_done - frame_done),
                    tiles: tile_stats,
                }
            };

            let percentile = |samples: &mut Vec<f64>, fraction: f64| {
                samples.sort_by(|a, b| a.total_cmp(b));
                let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
                samples[index]
            };
            let fx_display_step = |editor: &mut Editor| -> f64 {
                match editor
                    .runtime_mut()
                    .eval_str("SEQ.fx-step-display-step")
                    .expect("read SEQ.fx-step-display-step")
                {
                    Some(Value::Number(step)) => step,
                    other => panic!("SEQ.fx-step-display-step must be a number, got {other:?}"),
                }
            };

            struct ActionSamples {
                total: Vec<f64>,
                dispatch: Vec<f64>,
                invalidation: Vec<f64>,
                tick_sync: Vec<f64>,
                reactive: Vec<f64>,
                frame: Vec<f64>,
                retained: Vec<f64>,
                tile_retained: std::collections::BTreeMap<String, Vec<f64>>,
                tile_rebuilds: std::collections::BTreeMap<String, usize>,
            }
            impl ActionSamples {
                fn new() -> Self {
                    Self {
                        total: Vec::new(),
                        dispatch: Vec::new(),
                        invalidation: Vec::new(),
                        tick_sync: Vec::new(),
                        reactive: Vec::new(),
                        frame: Vec::new(),
                        retained: Vec::new(),
                        tile_retained: std::collections::BTreeMap::new(),
                        tile_rebuilds: std::collections::BTreeMap::new(),
                    }
                }
                fn record(
                    &mut self,
                    total_ms: f64,
                    dispatch_ms: f64,
                    update: &FullLayoutUpdate,
                ) {
                    self.total.push(total_ms);
                    self.dispatch.push(dispatch_ms);
                    self.invalidation.push(update.invalidation_ms);
                    self.tick_sync.push(update.tick_sync_ms);
                    self.reactive.push(update.reactive_ms);
                    self.frame.push(update.frame_ms);
                    self.retained.push(update.retained_ms);
                    for (name, _, retained_ms, rebuilt) in &update.tiles {
                        self.tile_retained
                            .entry(name.clone())
                            .or_default()
                            .push(*retained_ms);
                        if *rebuilt {
                            *self.tile_rebuilds.entry(name.clone()).or_default() += 1;
                        }
                    }
                }
            }
            let mut select_samples = ActionSamples::new();
            let mut unselect_samples = ActionSamples::new();

            for iteration in 0..(WARMUPS + SAMPLES) {
                // Reset to a known empty selection outside the timed region.
                selected_steps.lock().unwrap().clear();
                ui_invalidations.push(UiInvalidation::StepSelection {
                    track: TRACK,
                    changed_steps: (0..STEP_COUNT).collect(),
                });
                finish_visible_update(&mut editor, &mut app, &mut tile_retained);
                assert_eq!(
                    count_selected_shells(&tile_retained),
                    0,
                    "reset must leave no selected step shells in the retained scene"
                );

                // (a) Cmd+A select-all through the real shortcut path.
                let started = Instant::now();
                assert!(handle_metal_command_shortcut_with_ui_epoch(
                    &mut editor,
                    &crossterm::event::KeyEvent::new(
                        crossterm::event::KeyCode::Char('a'),
                        KeyModifiers::SUPER,
                    ),
                    &state,
                    &current_track,
                    &selected_steps,
                    &step_clipboard,
                    &ui_epoch,
                ));
                let dispatch_done = Instant::now();
                let update = finish_visible_update(&mut editor, &mut app, &mut tile_retained);
                let total_ms = duration_ms(started.elapsed());
                assert_eq!(
                    selected_steps.lock().unwrap().len(),
                    STEP_COUNT,
                    "Cmd+A must select all {STEP_COUNT} steps"
                );
                assert_eq!(
                    count_selected_shells(&tile_retained),
                    STEP_COUNT,
                    "all {STEP_COUNT} selected step shells must be in the retained sequencer scene"
                );
                assert_eq!(
                    fx_display_step(&mut editor),
                    0.0,
                    "select-all must publish the selection's p-lock display step to the fx panel"
                );
                let fx_dirty = update
                    .tiles
                    .iter()
                    .find(|(name, _, _, _)| name == "*fx*")
                    .map(|(_, dirty, _, _)| *dirty)
                    .expect("fx tile stats");
                assert!(
                    fx_dirty > 0,
                    "select-all must dirty widgets in the visible *fx* tile"
                );
                if iteration >= WARMUPS {
                    select_samples.record(
                        total_ms,
                        duration_ms(dispatch_done - started),
                        &update,
                    );
                }

                // (b) Escape unselect-all through the real production key
                // binding (Cmd+A is a pure select-all; the production path
                // for clearing a full selection is ESC -> seq-clear-ui-selection).
                let started = Instant::now();
                editor.handle_key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Esc,
                    KeyModifiers::NONE,
                ));
                let dispatch_done = Instant::now();
                let update = finish_visible_update(&mut editor, &mut app, &mut tile_retained);
                let total_ms = duration_ms(started.elapsed());
                assert!(
                    selected_steps.lock().unwrap().is_empty(),
                    "Escape must clear the full selection"
                );
                assert_eq!(
                    count_selected_shells(&tile_retained),
                    0,
                    "no selected step shells may remain in the retained scene after Escape"
                );
                assert_eq!(
                    fx_display_step(&mut editor),
                    -1.0,
                    "unselect must revert the fx panel's p-lock display step"
                );
                if iteration >= WARMUPS {
                    unselect_samples.record(
                        total_ms,
                        duration_ms(dispatch_done - started),
                        &update,
                    );
                }
            }

            for (name, samples) in [
                ("cmd-a-select", &mut select_samples),
                ("escape-unselect", &mut unselect_samples),
            ] {
                let median = percentile(&mut samples.total, 0.50);
                let dispatch_median = percentile(&mut samples.dispatch, 0.50);
                eprintln!(
                    "[project-92-fullayout-{name}] tracks={} steps={} tiles={} samples={} median_ms={:.3} p95_ms={:.3} dispatch_host_ms={:.3} visible_update_ms={:.3}",
                    app.tracks.len(),
                    STEP_COUNT,
                    visible_buffers.len(),
                    SAMPLES,
                    median,
                    percentile(&mut samples.total, 0.95),
                    dispatch_median,
                    median - dispatch_median,
                );
                eprintln!(
                    "[project-92-fullayout-{name}-visible-phases] invalidation_ms={:.3} tick_sync_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
                    percentile(&mut samples.invalidation, 0.50),
                    percentile(&mut samples.tick_sync, 0.50),
                    percentile(&mut samples.reactive, 0.50),
                    percentile(&mut samples.frame, 0.50),
                    percentile(&mut samples.retained, 0.50),
                );
                let tile_rebuilds = samples.tile_rebuilds.clone();
                let tile_breakdown = samples
                    .tile_retained
                    .iter_mut()
                    .map(|(tile, tile_samples)| {
                        format!(
                            "{tile}={:.3}(rebuilds={})",
                            percentile(tile_samples, 0.50),
                            tile_rebuilds.get(tile).copied().unwrap_or(0),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                eprintln!(
                    "[project-92-fullayout-{name}-retained-tiles] {tile_breakdown}"
                );
            }
            return;
        }

        if matches!(
            probe,
            Project92UiProbe::StepInteractions | Project92UiProbe::ArrangedStepInteractions
        ) {
            const TRACK: usize = 0;
            const STEP_COUNT: usize = 64;
            const SELECTED_MOVE_STEPS: usize = 16;
            const WARMUPS: usize = 5;
            const SAMPLES: usize = 20;
            let arranged = probe == Project92UiProbe::ArrangedStepInteractions;
            let probe_prefix = if arranged {
                "project-92-arranged-step"
            } else {
                "project-92-step"
            };

            let sequencer_buffer_id = editor
                .buffers
                .iter()
                .find(|buffer| buffer.name == "*sequencer*")
                .expect("sequencer buffer should exist")
                .id;
            editor.set_active_buffer(sequencer_buffer_id);
            state.pattern.track_params[TRACK].set_num_steps(STEP_COUNT);
            for step in 0..STEP_COUNT {
                state.pattern.patterns[TRACK].set_step_active(step, step < 24);
            }
            sync_all_track_sequencer_state(
                editor.runtime_mut(),
                &state,
                &app,
                TRACK,
                &selected_steps,
            );
            // The real event loop runs the song-state and sound-binding syncs
            // on every reactive tick (reactive_tick.rs); seed them here so the
            // arrangement read surfaces are published and warm, then include
            // the same syncs inside the timed region below.
            let mut song_frame = super::state_values::SongFrameState::default();
            let transport_visible = editor_has_visible_buffer(&editor, "*transport*");
            let arrangement_buffer_visible =
                editor_has_visible_buffer(&editor, "*arrangement*");
            assert!(
                !arrangement_buffer_visible,
                "step probes must measure the Seq view, not the Arr view"
            );
            let song_position_visible = transport_visible || arrangement_buffer_visible;
            app.sync_track_sound_bindings();
            super::state_values::sync_song_state(
                editor.runtime_mut(),
                &app,
                &mut song_frame,
                song_position_visible,
            );
            if arranged {
                assert!(
                    song_frame
                        .cached_lanes
                        .as_ref()
                        .is_some_and(|lanes| lanes.iter().any(|lane| !lane.is_empty())),
                    "arranged probe must publish non-empty song lanes"
                );
            }
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.update_tile_rects(180, 70);

            let initial_frame =
                eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);
            let initial_seq_frame = initial_frame
                .tiles
                .iter()
                .find(|tile| tile.frame.buffer_name == "*sequencer*")
                .expect("visible initial sequencer frame");
            let initial_layout = initial_seq_frame
                .frame
                .widget_layout
                .as_ref()
                .expect("initial sequencer layout");
            let viewport = eseqlisp::widget_render::WidgetViewport {
                cell_w: 8.0,
                cell_h: 16.0,
                vp_w: 1440.0,
                vp_h: 1120.0,
                time_seconds: 0.0,
                focused_widget_id: initial_seq_frame.frame.focused_widget_id,
                focused_branch: true,
                overlay_viewport_bottom: 70.0,
                scroll_top: initial_seq_frame.frame.widget_scroll_top
                    + initial_seq_frame.frame.text_scroll_top as f32,
                scroll_left: initial_seq_frame.frame.widget_layout_scroll_left,
                inherited_hover: false,
            };
            let (mut retained_runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                initial_layout,
                viewport,
                viewport.scroll_top,
                70,
            );
            let retained_run_indices =
                eseqlisp::widget_render::build_metal_primitive_run_index(&retained_runs);
            let step_clipboard = Arc::new(Mutex::new(None));
            let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
            let mixer_visible = editor_has_visible_buffer(&editor, "*mixer*");
            let step_center = |editor: &mut Editor, step: usize| {
                let layout = editor.widget_layout().expect("sequencer layout");
                let cell = find_layout_node_by_stable_key_suffix(
                    &layout,
                    &format!("/step-cell-{TRACK}-{step}"),
                )
                .unwrap_or_else(|| panic!("visible sequencer step cell {step}"));
                (
                    cell.rect.col + cell.rect.width * 0.5,
                    cell.rect.row + cell.rect.height * 0.5,
                    layout.rect.width.ceil().max(1.0) as u16,
                    layout.rect.height.ceil().max(1.0) as u16,
                )
            };

            let apply_pending_step_commands = |editor: &mut Editor, app: &mut app::App| {
                for command in editor.drain_host_commands() {
                    let HostCommand::Custom { name, payload } = command else {
                        continue;
                    };
                    match name.as_str() {
                        "toggle-step" => {
                            let (outcome, track, step) =
                                apply_toggle_step_host_command(app, &payload)
                                    .expect("apply benchmark step toggle");
                            assert!(matches!(outcome, app::edit::EditOutcome::Applied(_)));
                            selected_steps.lock().unwrap().clear();
                            ui_invalidations.push(UiInvalidation::StepBatch {
                                track,
                                steps: vec![step],
                            });
                        }
                        "move-step-history" => {
                            let (outcome, track, steps, affected_steps, delta, move_selection) =
                                apply_move_step_history_host_command(app, &payload)
                                    .expect("apply benchmark step move");
                            assert!(matches!(outcome, app::edit::EditOutcome::Applied(_)));
                            let moved_steps = steps
                                .iter()
                                .map(|step| (*step as isize + delta) as usize)
                                .collect::<Vec<_>>();
                            let mut changed_selection = Vec::new();
                            if move_selection {
                                let mut selected = selected_steps.lock().unwrap();
                                let previous = selected.clone();
                                selected.clear();
                                selected.extend(moved_steps.iter().copied());
                                changed_selection = previous
                                    .symmetric_difference(&selected)
                                    .copied()
                                    .collect();
                                changed_selection.sort_unstable();
                            }
                            ui_invalidations.push(UiInvalidation::StepBatch {
                                track,
                                steps: affected_steps,
                            });
                            if move_selection {
                                ui_invalidations.push(UiInvalidation::StepSelection {
                                    track,
                                    changed_steps: changed_selection,
                                });
                            }
                        }
                        "delete-selected-steps" => {
                            // Mirror the production handler
                            // (host_commands/step_history.rs "delete-selected-steps").
                            let track = match &payload {
                                Value::Map(map) => {
                                    super::history_commands::map_usize(map, "track")
                                }
                                _ => None,
                            }
                            .expect("benchmark delete payload track");
                            let (outcome, steps) =
                                apply_selected_steps_delete(app, track, &selected_steps)
                                    .expect("apply benchmark selected-step delete");
                            assert!(matches!(outcome, app::edit::EditOutcome::Applied(_)));
                            ui_invalidations.push(UiInvalidation::StepBatch {
                                track,
                                steps: steps.clone(),
                            });
                            ui_invalidations.push(UiInvalidation::StepSelection {
                                track,
                                changed_steps: steps,
                            });
                        }
                        other => panic!("unexpected benchmark host command {other}"),
                    }
                }
            };

            let neural = selected_neural_neurons.lock().unwrap().clone();
            let mut finish_visible_update = |editor: &mut Editor, app: &mut app::App| {
                let started = Instant::now();
                let invalidations = ui_invalidations.drain();
                if !invalidations.is_empty() {
                    apply_ui_invalidations(
                        invalidations,
                        UiInvalidationApplyCtx {
                            app,
                            editor,
                            state: &state,
                            track_collapsed: &track_collapsed,
                            bus_state: &bus_state,
                            current_track_idx: TRACK,
                            selected_steps: &selected_steps,
                            selected_neural_neurons: &neural,
                            piano_roll_selection: &piano_roll_selection,
                            accumulator_names: &accumulator_names,
                            cached_track_peak_levels: &cached_track_peak_levels,
                            cached_bus_peak_levels: &cached_bus_peak_levels,
                            record_armed: &record_armed,
                            active_delete_target: &active_delete_target,
                            active_delete_target_version: &active_delete_target_version,
                            expanded_step_projection: &expanded_step_projection,
                            fx_visible,
                            sequencer_visible: true,
                            mixer_visible,
                        },
                    );
                }
                let invalidations_done = Instant::now();
                // The real event loop's reactive tick runs these song/sound
                // syncs on every frame before the reactive cycle
                // (reactive_tick.rs); they are part of the user-visible
                // latency of every step interaction and belong in the sample.
                app.sync_track_sound_bindings();
                super::state_values::sync_song_state(
                    editor.runtime_mut(),
                    app,
                    &mut song_frame,
                    song_position_visible,
                );
                let tick_sync_done = Instant::now();
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                let reactive_done = Instant::now();
                let frame =
                    eseqlisp::frame::build_tiled_render_frame_borderless(editor, 180, 70);
                let seq_frame = frame
                    .tiles
                    .iter()
                    .find(|tile| tile.frame.buffer_name == "*sequencer*")
                    .expect("visible sequencer frame after benchmark action");
                let layout = seq_frame
                    .frame
                    .widget_layout
                    .as_ref()
                    .expect("sequencer layout after benchmark action");
                let frame_done = Instant::now();
                let (_, stats) =
                    eseqlisp::widget_render::refresh_metal_primitive_runs_retained_in_place(
                        layout,
                        viewport,
                        viewport.scroll_top,
                        70,
                        &mut retained_runs,
                        &retained_run_indices,
                        &seq_frame.frame.dirty_widget_ids,
                    );
                assert_eq!(stats.missing_previous_runs, 0);
                assert_eq!(stats.invalid_previous_runs, 0);
                let retained_done = Instant::now();
                (
                    duration_ms(invalidations_done - started),
                    duration_ms(tick_sync_done - invalidations_done),
                    duration_ms(reactive_done - tick_sync_done),
                    duration_ms(frame_done - reactive_done),
                    duration_ms(retained_done - frame_done),
                )
            };

            let percentile = |samples: &mut Vec<f64>, fraction: f64| {
                samples.sort_by(|a, b| a.total_cmp(b));
                let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
                samples[index]
            };
            let mut select_one_samples = Vec::with_capacity(SAMPLES);
            let mut select_all_samples = Vec::with_capacity(SAMPLES);
            let mut move_samples = Vec::with_capacity(SAMPLES);
            let mut toggle_drag_samples = Vec::with_capacity(SAMPLES);
            let mut select_one_dispatch = Vec::with_capacity(SAMPLES);
            let mut select_all_dispatch = Vec::with_capacity(SAMPLES);
            let mut move_dispatch = Vec::with_capacity(SAMPLES);
            let mut toggle_drag_dispatch = Vec::with_capacity(SAMPLES);
            let mut move_invalidation = Vec::with_capacity(SAMPLES);
            let mut move_tick_sync = Vec::with_capacity(SAMPLES);
            let mut move_reactive = Vec::with_capacity(SAMPLES);
            let mut move_frame = Vec::with_capacity(SAMPLES);
            let mut move_retained = Vec::with_capacity(SAMPLES);
            let mut delete_samples = Vec::with_capacity(SAMPLES);
            let mut delete_dispatch = Vec::with_capacity(SAMPLES);
            let mut delete_shortcut = Vec::with_capacity(WARMUPS + SAMPLES);
            let mut delete_apply = Vec::with_capacity(WARMUPS + SAMPLES);
            let mut delete_invalidation = Vec::with_capacity(SAMPLES);
            let mut delete_tick_sync = Vec::with_capacity(SAMPLES);
            let mut delete_reactive = Vec::with_capacity(SAMPLES);
            let mut delete_frame = Vec::with_capacity(SAMPLES);
            let mut delete_retained = Vec::with_capacity(SAMPLES);

            for iteration in 0..(WARMUPS + SAMPLES) {
                selected_steps.lock().unwrap().clear();
                ui_invalidations.push(UiInvalidation::StepSelection {
                    track: TRACK,
                    changed_steps: (0..STEP_COUNT).collect(),
                });
                finish_visible_update(&mut editor, &mut app);
                let (col, row, width, height) = step_center(&mut editor, 8);
                let started = Instant::now();
                editor.handle_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column: col.floor() as u16,
                        row: row.floor() as u16,
                        modifiers: KeyModifiers::SHIFT,
                    },
                    0,
                    0,
                    width,
                    height,
                    col,
                    row,
                );
                apply_pending_step_commands(&mut editor, &mut app);
                let dispatch_done = Instant::now();
                finish_visible_update(&mut editor, &mut app);
                assert_eq!(*selected_steps.lock().unwrap(), HashSet::from([8]));
                if iteration >= WARMUPS {
                    select_one_samples.push(duration_ms(started.elapsed()));
                    select_one_dispatch.push(duration_ms(dispatch_done - started));
                }

                selected_steps.lock().unwrap().clear();
                ui_invalidations.push(UiInvalidation::StepSelection {
                    track: TRACK,
                    changed_steps: (0..STEP_COUNT).collect(),
                });
                finish_visible_update(&mut editor, &mut app);
                let started = Instant::now();
                assert!(handle_metal_command_shortcut_with_ui_epoch(
                    &mut editor,
                    &crossterm::event::KeyEvent::new(
                        crossterm::event::KeyCode::Char('a'),
                        KeyModifiers::SUPER,
                    ),
                    &state,
                    &current_track,
                    &selected_steps,
                    &step_clipboard,
                    &ui_epoch,
                ));
                let dispatch_done = Instant::now();
                finish_visible_update(&mut editor, &mut app);
                assert_eq!(selected_steps.lock().unwrap().len(), STEP_COUNT);
                if iteration >= WARMUPS {
                    select_all_samples.push(duration_ms(started.elapsed()));
                    select_all_dispatch.push(duration_ms(dispatch_done - started));
                }

                for step in 0..STEP_COUNT {
                    state.pattern.patterns[TRACK].set_step_active(
                        step,
                        step >= 8 && step < 8 + SELECTED_MOVE_STEPS,
                    );
                }
                {
                    let mut selected = selected_steps.lock().unwrap();
                    selected.clear();
                    selected.extend(8..8 + SELECTED_MOVE_STEPS);
                }
                ui_invalidations.push(UiInvalidation::Pattern(
                    PatternInvalidation::WholeTrack { track: TRACK },
                ));
                ui_invalidations.push(UiInvalidation::StepSelection {
                    track: TRACK,
                    changed_steps: (0..STEP_COUNT).collect(),
                });
                finish_visible_update(&mut editor, &mut app);
                assert_eq!(
                    editor.runtime_mut().eval_str("(eseq.step-grid-interactions/step-selected? 8)").unwrap(),
                    Some(Value::Bool(true)),
                    "move fixture must expose step 8 as selected to the gesture Lisp",
                );
                assert_eq!(
                    editor
                        .runtime_mut()
                        .eval_str("(seq-track-step-active? 0 8)")
                        .unwrap(),
                    Some(Value::Bool(true)),
                    "move fixture must expose step 8 as active to the gesture Lisp",
                );
                assert_eq!(
                    editor.runtime_mut().eval_str("SEQ.current-track").unwrap(),
                    Some(Value::Number(0.0)),
                    "move fixture must target the visible current track",
                );
                let (_, _, width, height) = step_center(&mut editor, 8);
                let (target_col, target_row, _, _) = step_center(&mut editor, 9);
                // Arm the exact production pointer-down handler with an
                // explicit left-edge local coordinate. The timed operation is
                // the subsequent real mouse drag tick; pointer-down is a
                // precondition and is intentionally outside the sample.
                editor
                    .runtime_mut()
                    .eval_str("(eseq.sequencer/grid-step-pointer-down 0 8 (dict :sx -1))")
                    .expect("arm selected-step move gesture");
                assert_eq!(
                    editor.runtime_mut().eval_str("step-move-last").unwrap(),
                    Some(Value::Number(8.0)),
                    "pointer down on the selected active step must arm move dragging",
                );
                let started = Instant::now();
                editor.handle_mouse_precise(
                    mouse_event(
                        MouseEventKind::Drag(MouseButton::Left),
                        target_col.floor() as u16,
                        target_row.floor() as u16,
                    ),
                    0,
                    0,
                    width,
                    height,
                    target_col,
                    target_row,
                );
                apply_pending_step_commands(&mut editor, &mut app);
                let dispatch_done = Instant::now();
                let move_visible_phases = finish_visible_update(&mut editor, &mut app);
                assert_eq!(
                    *selected_steps.lock().unwrap(),
                    (9..9 + SELECTED_MOVE_STEPS).collect::<HashSet<_>>()
                );
                if iteration >= WARMUPS {
                    move_samples.push(duration_ms(started.elapsed()));
                    move_dispatch.push(duration_ms(dispatch_done - started));
                    move_invalidation.push(move_visible_phases.0);
                    move_tick_sync.push(move_visible_phases.1);
                    move_reactive.push(move_visible_phases.2);
                    move_frame.push(move_visible_phases.3);
                    move_retained.push(move_visible_phases.4);
                }
                editor
                    .runtime_mut()
                    .eval_str("(eseq.sequencer/grid-step-pointer-up 0 9 (dict :sx -1))")
                    .expect("finish benchmark selected-step drag");

                selected_steps.lock().unwrap().clear();
                state.pattern.patterns[TRACK].set_step_active(32, false);
                state.pattern.patterns[TRACK].set_step_active(33, false);
                ui_invalidations.push(UiInvalidation::Pattern(
                    PatternInvalidation::WholeTrack { track: TRACK },
                ));
                finish_visible_update(&mut editor, &mut app);
                let (start_col, start_row, width, height) = step_center(&mut editor, 32);
                let (target_col, target_row, _, _) = step_center(&mut editor, 33);
                editor.handle_mouse_precise(
                    mouse_event(
                        MouseEventKind::Down(MouseButton::Left),
                        start_col.floor() as u16,
                        start_row.floor() as u16,
                    ),
                    0,
                    0,
                    width,
                    height,
                    start_col,
                    start_row,
                );
                apply_pending_step_commands(&mut editor, &mut app);
                finish_visible_update(&mut editor, &mut app);
                let started = Instant::now();
                editor.handle_mouse_precise(
                    mouse_event(
                        MouseEventKind::Drag(MouseButton::Left),
                        target_col.floor() as u16,
                        target_row.floor() as u16,
                    ),
                    0,
                    0,
                    width,
                    height,
                    target_col,
                    target_row,
                );
                apply_pending_step_commands(&mut editor, &mut app);
                let dispatch_done = Instant::now();
                finish_visible_update(&mut editor, &mut app);
                assert!(state.pattern.patterns[TRACK].is_active(33));
                if iteration >= WARMUPS {
                    toggle_drag_samples.push(duration_ms(started.elapsed()));
                    toggle_drag_dispatch.push(duration_ms(dispatch_done - started));
                }
                editor
                    .runtime_mut()
                    .eval_str("(eseq.sequencer/grid-step-pointer-up 0 33 (dict :sx -1))")
                    .expect("finish benchmark toggle drag");

                // delete-16: sixteen active steps selected, then the real
                // Backspace shortcut; the visible result is those steps
                // cleared and the selection emptied.
                for step in 0..STEP_COUNT {
                    state.pattern.patterns[TRACK]
                        .set_step_active(step, step >= 40 && step < 40 + SELECTED_MOVE_STEPS);
                }
                {
                    let mut selected = selected_steps.lock().unwrap();
                    selected.clear();
                    selected.extend(40..40 + SELECTED_MOVE_STEPS);
                }
                ui_invalidations.push(UiInvalidation::Pattern(
                    PatternInvalidation::WholeTrack { track: TRACK },
                ));
                ui_invalidations.push(UiInvalidation::StepSelection {
                    track: TRACK,
                    changed_steps: (0..STEP_COUNT).collect(),
                });
                finish_visible_update(&mut editor, &mut app);
                let started = Instant::now();
                assert!(
                    handle_metal_command_shortcut_with_ui_epoch(
                        &mut editor,
                        &crossterm::event::KeyEvent::new(
                            crossterm::event::KeyCode::Backspace,
                            KeyModifiers::NONE,
                        ),
                        &state,
                        &current_track,
                        &selected_steps,
                        &step_clipboard,
                        &ui_epoch,
                    ),
                    "Backspace must dispatch the selected-step delete shortcut",
                );
                let shortcut_done = Instant::now();
                apply_pending_step_commands(&mut editor, &mut app);
                let dispatch_done = Instant::now();
                delete_shortcut.push(duration_ms(shortcut_done - started));
                delete_apply.push(duration_ms(dispatch_done - shortcut_done));
                let delete_visible_phases = finish_visible_update(&mut editor, &mut app);
                assert!(
                    selected_steps.lock().unwrap().is_empty(),
                    "selected-step delete must clear the selection",
                );
                assert!(
                    !state.pattern.patterns[TRACK].is_active(40),
                    "selected-step delete must clear the deleted steps",
                );
                assert_eq!(
                    editor
                        .runtime_mut()
                        .eval_str("(seq-track-step-active? 0 40)")
                        .unwrap(),
                    Some(Value::Bool(false)),
                    "deleted step must read as inactive through the UI bindings",
                );
                if iteration >= WARMUPS {
                    delete_samples.push(duration_ms(started.elapsed()));
                    delete_dispatch.push(duration_ms(dispatch_done - started));
                    delete_invalidation.push(delete_visible_phases.0);
                    delete_tick_sync.push(delete_visible_phases.1);
                    delete_reactive.push(delete_visible_phases.2);
                    delete_frame.push(delete_visible_phases.3);
                    delete_retained.push(delete_visible_phases.4);
                }
                // Restore the initial fixture actives so the next iteration's
                // select-one clicks an active step, as before this block ran.
                for step in 0..STEP_COUNT {
                    state.pattern.patterns[TRACK].set_step_active(step, step < 24);
                }
                ui_invalidations.push(UiInvalidation::Pattern(
                    PatternInvalidation::WholeTrack { track: TRACK },
                ));
                finish_visible_update(&mut editor, &mut app);
            }

            // Medians recorded by this same release-mode, end-to-end probe on
            // project 92 before the targeted invalidation work. The first four
            // are the 2026-07-22 pre-tuning medians; delete-16 is the
            // 2026-07-28 pre-tuning median (per-eval program clone in
            // `Vm::eval_str` + whole-track invalidation, fixed the same day).
            // The arranged variant enforces the same ceilings: realistic
            // arrangement state must not push Seq-view step editing over them.
            let references: [Option<f64>; 5] = if arranged {
                [
                    Some(8.251),
                    Some(7.773),
                    Some(106.412),
                    Some(21.020),
                    Some(19.243),
                ]
            } else {
                [
                    Some(8.251),
                    Some(7.773),
                    Some(106.412),
                    Some(21.020),
                    Some(18.969),
                ]
            };
            for ((name, samples, dispatch), reference) in [
                ("select-one", &mut select_one_samples, &mut select_one_dispatch),
                ("cmd-a", &mut select_all_samples, &mut select_all_dispatch),
                ("move-16", &mut move_samples, &mut move_dispatch),
                ("toggle-drag", &mut toggle_drag_samples, &mut toggle_drag_dispatch),
                ("delete-16", &mut delete_samples, &mut delete_dispatch),
            ]
            .into_iter()
            .zip(references)
            {
                let median = percentile(samples, 0.50);
                let dispatch_median = percentile(dispatch, 0.50);
                let speedup = match reference {
                    Some(reference_ms) => format!(" speedup={:.1}x", reference_ms / median),
                    None => String::new(),
                };
                eprintln!(
                    "[{probe_prefix}-{name}] tracks={} steps={} samples={} median_ms={:.3} p95_ms={:.3}{speedup} dispatch_host_ms={:.3} visible_update_ms={:.3}",
                    app.tracks.len(),
                    STEP_COUNT,
                    SAMPLES,
                    median,
                    percentile(samples, 0.95),
                    dispatch_median,
                    median - dispatch_median,
                );
                if let Some(reference_ms) = reference {
                    // delete-16 lands ~12x on a quiet machine but its
                    // post-tuning median (~1.2-2.2 ms) sits close to the 10x
                    // line under concurrent load, so it enforces 8x; the
                    // long-standing actions keep the 10x contract.
                    let required = if name == "delete-16" { 8.0 } else { 10.0 };
                    assert!(
                        median <= reference_ms / required,
                        "{name} median {median:.3} ms did not reach {required}x versus the {reference_ms:.3} ms baseline",
                    );
                }
            }
            eprintln!(
                "[{probe_prefix}-delete-16-dispatch-phases] shortcut_ms={:.3} apply_ms={:.3}",
                percentile(&mut delete_shortcut, 0.50),
                percentile(&mut delete_apply, 0.50),
            );
            eprintln!(
                "[{probe_prefix}-delete-16-visible-phases] invalidation_ms={:.3} tick_sync_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
                percentile(&mut delete_invalidation, 0.50),
                percentile(&mut delete_tick_sync, 0.50),
                percentile(&mut delete_reactive, 0.50),
                percentile(&mut delete_frame, 0.50),
                percentile(&mut delete_retained, 0.50),
            );
            eprintln!(
                "[{probe_prefix}-move-16-visible-phases] invalidation_ms={:.3} tick_sync_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
                percentile(&mut move_invalidation, 0.50),
                percentile(&mut move_tick_sync, 0.50),
                percentile(&mut move_reactive, 0.50),
                percentile(&mut move_frame, 0.50),
                percentile(&mut move_retained, 0.50),
            );
            return;
        }

        if probe == Project92UiProbe::RackMacroDrag {
            const TRACK: usize = 0;
            const SELECTED_COUNT: usize = 32;
            const WARMUPS: usize = 5;
            const SAMPLES: usize = 20;
            // Recorded before the optimization on this exact project/rack fixture,
            // after replacing the old cheap slot-gain/pan/mute mappings with three
            // real instrument-parameter mappings.
            const REFERENCE_LIVE_MEDIAN_MS: f64 = 5.023;
            const REFERENCE_PLOCK_MEDIAN_MS: f64 = 2.234;

            struct RackMacroPerfSamples {
                total: Vec<f64>,
                host: Vec<f64>,
                reactive: Vec<f64>,
                frame: Vec<f64>,
                retained: Vec<f64>,
            }

            let fx_buffer_id = editor
                .buffers
                .iter()
                .find(|buffer| buffer.name == "*fx*")
                .expect("fx buffer should exist")
                .id;
            editor.set_active_buffer(fx_buffer_id);
            editor
                .runtime_mut()
                .eval_str("(if (not eseq.effects.state/rack-panel-macros-open) (eseq.effects.instrument-panel/rack-panel-toggle-macros) false)")
                .expect("open rack macro bank");
            editor.refresh_runtime_side_effects();
            editor.update_tile_rects(180, 70);

            let percentile = |samples: &mut Vec<f64>, fraction: f64| {
                samples.sort_by(|a, b| a.total_cmp(b));
                let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
                samples[index]
            };
            let mut run_scenario = |selected_count: usize| {
                selected_steps.lock().unwrap().clear();
                selected_steps.lock().unwrap().extend(0..selected_count);
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "selected-steps",
                    build_selection_value(&selected_steps),
                );
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();

                let initial_frame =
                    eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);
                let initial_fx_frame = initial_frame
                    .tiles
                    .iter()
                    .find(|tile| tile.frame.buffer_name == "*fx*")
                    .expect("visible initial fx frame");
                let initial_fx_layout = initial_fx_frame
                    .frame
                    .widget_layout
                    .as_ref()
                    .expect("initial fx layout");
                let initial_viewport = eseqlisp::widget_render::WidgetViewport {
                    cell_w: 8.0,
                    cell_h: 16.0,
                    vp_w: 1440.0,
                    vp_h: 1120.0,
                    time_seconds: 0.0,
                    focused_widget_id: initial_fx_frame.frame.focused_widget_id,
                    focused_branch: true,
                    overlay_viewport_bottom: 70.0,
                    scroll_top: initial_fx_frame.frame.widget_scroll_top
                        + initial_fx_frame.frame.text_scroll_top as f32,
                    scroll_left: initial_fx_frame.frame.widget_layout_scroll_left,
                    inherited_hover: false,
                };
                let (mut retained_runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                    initial_fx_layout,
                    initial_viewport,
                    initial_viewport.scroll_top,
                    70,
                );
                let retained_run_indices =
                    eseqlisp::widget_render::build_metal_primitive_run_index(&retained_runs);

                let mut samples = RackMacroPerfSamples {
                    total: Vec::with_capacity(SAMPLES),
                    host: Vec::with_capacity(SAMPLES),
                    reactive: Vec::with_capacity(SAMPLES),
                    frame: Vec::with_capacity(SAMPLES),
                    retained: Vec::with_capacity(SAMPLES),
                };
                for iteration in 0..(WARMUPS + SAMPLES) {
                    let layout = editor.widget_layout().expect("rack macro layout");
                    let knob = find_layout_node_by_debug_name(&layout, "rack-macro-knob-0")
                        .expect("first rack macro knob");
                    let col = knob.rect.col + knob.rect.width * 0.5;
                    let row = knob.rect.row + knob.rect.height * 0.5;
                    let target_row = if iteration % 2 == 0 {
                        row - 1.0
                    } else {
                        row + 1.0
                    };
                    let width = layout.rect.width.ceil().max(1.0) as u16;
                    let height = layout.rect.height.ceil().max(1.0) as u16;

                    editor.handle_mouse_precise(
                        mouse_event(
                            MouseEventKind::Down(MouseButton::Left),
                            col.floor() as u16,
                            row.floor() as u16,
                        ),
                        0,
                        0,
                        width,
                        height,
                        col,
                        row,
                    );
                    let _ = editor.drain_host_commands();

                    let started = Instant::now();
                    editor.handle_mouse_precise(
                        mouse_event(
                            MouseEventKind::Drag(MouseButton::Left),
                            col.floor() as u16,
                            target_row.floor() as u16,
                        ),
                        0,
                        0,
                        width,
                        height,
                        col,
                        target_row,
                    );
                    let commands = editor.drain_host_commands();
                    assert_eq!(commands.len(), 1, "macro drag commands={commands:?}");
                    let HostCommand::Custom { name, payload } = &commands[0] else {
                        panic!("macro drag must emit a custom command: {commands:?}");
                    };
                    let Value::Map(map) = payload else {
                        panic!("macro drag payload must be a map: {payload:?}");
                    };
                    assert!(apply_rack_macro_host_command(
                        name,
                        map,
                        &mut editor,
                        &mut app,
                        &state,
                        &selected_steps,
                        &ui_epoch,
                        &fx_epoch,
                    ));
                    let host_done = Instant::now();
                    editor.runtime_mut().run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    let reactive_done = Instant::now();
                    let frame =
                        eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);
                    let fx_frame = frame
                        .tiles
                        .iter()
                        .find(|tile| tile.frame.buffer_name == "*fx*")
                        .expect("visible fx frame after macro drag");
                    let fx_layout = fx_frame
                        .frame
                        .widget_layout
                        .as_ref()
                        .expect("fx layout after macro drag");
                    let frame_done = Instant::now();
                    let viewport = eseqlisp::widget_render::WidgetViewport {
                        cell_w: 8.0,
                        cell_h: 16.0,
                        vp_w: 1440.0,
                        vp_h: 1120.0,
                        time_seconds: 0.0,
                        focused_widget_id: fx_frame.frame.focused_widget_id,
                        focused_branch: true,
                        overlay_viewport_bottom: 70.0,
                        scroll_top: fx_frame.frame.widget_scroll_top
                            + fx_frame.frame.text_scroll_top as f32,
                        scroll_left: fx_frame.frame.widget_layout_scroll_left,
                        inherited_hover: false,
                    };
                    let (_, retained_stats) =
                        eseqlisp::widget_render::refresh_metal_primitive_runs_retained_in_place(
                            fx_layout,
                            viewport,
                            viewport.scroll_top,
                            70,
                            &mut retained_runs,
                            &retained_run_indices,
                            &fx_frame.frame.dirty_widget_ids,
                        );
                    assert_eq!(retained_stats.missing_previous_runs, 0);
                    assert_eq!(retained_stats.invalid_previous_runs, 0);
                    assert!(
                        !retained_runs.is_empty(),
                        "macro drag must retain Metal runs"
                    );
                    let knob_wrapper =
                        find_layout_node_by_debug_name(fx_layout, "rack-macro-knob-0")
                            .expect("rendered first rack macro knob");
                    let knob = find_layout_node_by_widget_type(knob_wrapper, "knob-number")
                        .expect("rendered rack macro knob-number");
                    assert!(layout_prop_number(knob, "value").is_some());
                    if iteration >= WARMUPS {
                        let retained_done = Instant::now();
                        samples.total.push(duration_ms(started.elapsed()));
                        samples.host.push(duration_ms(host_done - started));
                        samples
                            .reactive
                            .push(duration_ms(reactive_done - host_done));
                        samples.frame.push(duration_ms(frame_done - reactive_done));
                        samples
                            .retained
                            .push(duration_ms(retained_done - frame_done));
                    }
                }
                samples
            };

            // Populate frame/layout caches through the production path. The old
            // comparison happened to do this by running its legacy scenario
            // first, which hid the dependency from the measured samples.
            let _unmeasured_cache_warmup = run_scenario(0);
            let mut live_samples = run_scenario(0);
            let _unmeasured_plock_cache_warmup = run_scenario(SELECTED_COUNT);
            let mut plock_samples = run_scenario(SELECTED_COUNT);
            let live_median = percentile(&mut live_samples.total, 0.50);
            let plock_median = percentile(&mut plock_samples.total, 0.50);
            let live_speedup = REFERENCE_LIVE_MEDIAN_MS / live_median;
            let plock_speedup = REFERENCE_PLOCK_MEDIAN_MS / plock_median;
            eprintln!(
                "[project-92-rack-macro-drag] mappings=3 selected=0 samples={} reference_median_ms={:.3} median_ms={:.3} p95_ms={:.3} speedup={:.1}x",
                SAMPLES,
                REFERENCE_LIVE_MEDIAN_MS,
                live_median,
                percentile(&mut live_samples.total, 0.95),
                live_speedup,
            );
            eprintln!(
                "[project-92-rack-macro-drag] mappings=3 selected={} samples={} reference_median_ms={:.3} median_ms={:.3} p95_ms={:.3} speedup={:.1}x",
                SELECTED_COUNT,
                SAMPLES,
                REFERENCE_PLOCK_MEDIAN_MS,
                plock_median,
                percentile(&mut plock_samples.total, 0.95),
                plock_speedup,
            );
            eprintln!(
                "[project-92-rack-macro-drag-phases] selected=0 host_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
                percentile(&mut live_samples.host, 0.50),
                percentile(&mut live_samples.reactive, 0.50),
                percentile(&mut live_samples.frame, 0.50),
                percentile(&mut live_samples.retained, 0.50),
            );
            eprintln!(
                "[project-92-rack-macro-drag-phases] selected={} host_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
                SELECTED_COUNT,
                percentile(&mut plock_samples.host, 0.50),
                percentile(&mut plock_samples.reactive, 0.50),
                percentile(&mut plock_samples.frame, 0.50),
                percentile(&mut plock_samples.retained, 0.50),
            );
            drop(run_scenario);

            selected_steps.lock().unwrap().clear();
            editor.runtime_mut().set_reactive(
                "SEQ",
                "selected-steps",
                build_selection_value(&selected_steps),
            );
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();

            let initial_frame =
                eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);
            let initial_fx_frame = initial_frame
                .tiles
                .iter()
                .find(|tile| tile.frame.buffer_name == "*fx*")
                .expect("visible initial fx frame for direct rack control");
            let initial_fx_layout = initial_fx_frame
                .frame
                .widget_layout
                .as_ref()
                .expect("initial fx layout for direct rack control");
            let direct_slot = state.pattern.rack_tracks.lock().unwrap()[TRACK]
                .as_ref()
                .expect("direct rack fixture track")
                .slots[0]
                .clone();
            let direct_descriptor = app
                .rack_slot_instrument_descriptor(&direct_slot)
                .expect("direct rack fixture instrument descriptor");
            let (direct_param_idx, direct_param, direct_param_key_suffix) = direct_descriptor
                .params
                .iter()
                .enumerate()
                .skip(3)
                .find_map(|(param_idx, param)| {
                    let is_haptic_free_continuous =
                        matches!(param.kind, sequencer::effects::ParamKind::Continuous { .. })
                            && (param.max - param.min).abs() <= 1.0
                            && !sequencer::instruments::voice_modulator::is_bar_resync_param(
                                param.node_param_idx,
                            );
                    let suffix = format!("-{}", param.name);
                    (is_haptic_free_continuous
                        && find_layout_node_by_stable_key_suffix(initial_fx_layout, &suffix)
                            .and_then(|node| find_layout_node_by_widget_type(node, "knob-number"))
                            .is_some())
                    .then(|| (param_idx, param.clone(), suffix))
                })
                .expect("visible unmapped 0-1 rack instrument knob");
            let initial_viewport = eseqlisp::widget_render::WidgetViewport {
                cell_w: 8.0,
                cell_h: 16.0,
                vp_w: 1440.0,
                vp_h: 1120.0,
                time_seconds: 0.0,
                focused_widget_id: initial_fx_frame.frame.focused_widget_id,
                focused_branch: true,
                overlay_viewport_bottom: 70.0,
                scroll_top: initial_fx_frame.frame.widget_scroll_top
                    + initial_fx_frame.frame.text_scroll_top as f32,
                scroll_left: initial_fx_frame.frame.widget_layout_scroll_left,
                inherited_hover: false,
            };
            let (mut direct_retained_runs, _) =
                eseqlisp::widget_render::collect_metal_primitive_runs(
                    initial_fx_layout,
                    initial_viewport,
                    initial_viewport.scroll_top,
                    70,
                );
            let direct_retained_indices =
                eseqlisp::widget_render::build_metal_primitive_run_index(&direct_retained_runs);
            let mut direct_samples = RackMacroPerfSamples {
                total: Vec::with_capacity(SAMPLES),
                host: Vec::with_capacity(SAMPLES),
                reactive: Vec::with_capacity(SAMPLES),
                frame: Vec::with_capacity(SAMPLES),
                retained: Vec::with_capacity(SAMPLES),
            };
            let mut direct_input_samples = Vec::with_capacity(SAMPLES);
            let mut direct_state_samples = Vec::with_capacity(SAMPLES);
            let mut direct_sync_samples = Vec::with_capacity(SAMPLES);
            for iteration in 0..(WARMUPS + SAMPLES) {
                let layout = editor.widget_layout().expect("direct rack control layout");
                let control_wrapper =
                    find_layout_node_by_stable_key_suffix(&layout, &direct_param_key_suffix)
                        .expect("direct rack instrument parameter wrapper");
                let control = find_layout_node_by_widget_type(control_wrapper, "knob-number")
                    .expect("direct rack instrument knob");
                let col = control.rect.col + control.rect.width * 0.5;
                let row = control.rect.row + control.rect.height * 0.5;
                let target_row = if iteration % 2 == 0 {
                    row - 1.0
                } else {
                    row + 1.0
                };
                let width = layout.rect.width.ceil().max(1.0) as u16;
                let height = layout.rect.height.ceil().max(1.0) as u16;
                editor.handle_mouse_precise(
                    mouse_event(
                        MouseEventKind::Down(MouseButton::Left),
                        col.floor() as u16,
                        row.floor() as u16,
                    ),
                    0,
                    0,
                    width,
                    height,
                    col,
                    row,
                );
                let _ = editor.drain_host_commands();

                let started = Instant::now();
                editor.handle_mouse_precise(
                    mouse_event(
                        MouseEventKind::Drag(MouseButton::Left),
                        col.floor() as u16,
                        target_row.floor() as u16,
                    ),
                    0,
                    0,
                    width,
                    height,
                    col,
                    target_row,
                );
                let commands = editor.drain_host_commands();
                let input_done = Instant::now();
                assert_eq!(
                    commands.len(),
                    1,
                    "direct rack control commands={commands:?}"
                );
                let HostCommand::Custom { name, payload } = &commands[0] else {
                    panic!("direct rack control must emit a custom command: {commands:?}");
                };
                assert_eq!(name, "set-rack-slot-instrument-param");
                let Value::Map(map) = payload else {
                    panic!("direct rack control payload must be a map: {payload:?}");
                };
                let track = map_usize(map, "track").expect("direct rack track");
                let slot_idx = map_usize(map, "slot").expect("direct rack slot");
                let param_idx = map_usize(map, "param-idx").expect("direct rack param");
                assert_eq!(param_idx, direct_param_idx);
                let user_value = map_number(map, "value").expect("direct rack value") as f32;
                let stored = direct_param.clamp(direct_param.user_input_to_stored(user_value));
                app.set_rack_slot_instrument_param(track, slot_idx, param_idx, stored);
                let state_done = Instant::now();
                refresh_rack_direct_param_reactive(
                    &mut editor,
                    &app,
                    &state,
                    track,
                    RackDirectDisplayTarget::InstrumentParam {
                        slot_idx,
                        param_idx,
                    },
                    &selected_steps,
                    false,
                    &ui_epoch,
                );
                let host_done = Instant::now();
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                let reactive_done = Instant::now();
                let frame =
                    eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);
                let fx_frame = frame
                    .tiles
                    .iter()
                    .find(|tile| tile.frame.buffer_name == "*fx*")
                    .expect("visible fx frame after direct rack edit");
                let fx_layout = fx_frame
                    .frame
                    .widget_layout
                    .as_ref()
                    .expect("fx layout after direct rack edit");
                let frame_done = Instant::now();
                let viewport = eseqlisp::widget_render::WidgetViewport {
                    cell_w: 8.0,
                    cell_h: 16.0,
                    vp_w: 1440.0,
                    vp_h: 1120.0,
                    time_seconds: 0.0,
                    focused_widget_id: fx_frame.frame.focused_widget_id,
                    focused_branch: true,
                    overlay_viewport_bottom: 70.0,
                    scroll_top: fx_frame.frame.widget_scroll_top
                        + fx_frame.frame.text_scroll_top as f32,
                    scroll_left: fx_frame.frame.widget_layout_scroll_left,
                    inherited_hover: false,
                };
                let (_, retained_stats) =
                    eseqlisp::widget_render::refresh_metal_primitive_runs_retained_in_place(
                        fx_layout,
                        viewport,
                        viewport.scroll_top,
                        70,
                        &mut direct_retained_runs,
                        &direct_retained_indices,
                        &fx_frame.frame.dirty_widget_ids,
                    );
                assert_eq!(retained_stats.missing_previous_runs, 0);
                assert_eq!(retained_stats.invalid_previous_runs, 0);
                let rendered_wrapper =
                    find_layout_node_by_stable_key_suffix(fx_layout, &direct_param_key_suffix)
                        .expect("rendered direct rack instrument parameter wrapper");
                let rendered_control =
                    find_layout_node_by_widget_type(rendered_wrapper, "knob-number")
                        .expect("rendered direct rack instrument knob");
                let rendered_value = layout_prop_number(rendered_control, "value")
                    .expect("rendered direct rack instrument value");
                assert!((rendered_value - user_value as f64).abs() < 0.0001);
                if iteration >= WARMUPS {
                    let retained_done = Instant::now();
                    direct_samples.total.push(duration_ms(started.elapsed()));
                    direct_samples.host.push(duration_ms(host_done - started));
                    direct_samples
                        .reactive
                        .push(duration_ms(reactive_done - host_done));
                    direct_samples
                        .frame
                        .push(duration_ms(frame_done - reactive_done));
                    direct_samples
                        .retained
                        .push(duration_ms(retained_done - frame_done));
                    direct_input_samples.push(duration_ms(input_done - started));
                    direct_state_samples.push(duration_ms(state_done - input_done));
                    direct_sync_samples.push(duration_ms(host_done - state_done));
                }
            }
            let direct_median = percentile(&mut direct_samples.total, 0.50);
            eprintln!(
                "[project-92-direct-rack-instrument-drag] param={:?} selected=0 samples={} median_ms={:.3} p95_ms={:.3} versus_macro={:.1}x slower",
                direct_param.name,
                SAMPLES,
                direct_median,
                percentile(&mut direct_samples.total, 0.95),
                direct_median / live_median,
            );
            eprintln!(
                "[project-92-direct-rack-instrument-drag-phases] host_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
                percentile(&mut direct_samples.host, 0.50),
                percentile(&mut direct_samples.reactive, 0.50),
                percentile(&mut direct_samples.frame, 0.50),
                percentile(&mut direct_samples.retained, 0.50),
            );
            eprintln!(
                "[project-92-direct-rack-instrument-drag-host-detail] input_ms={:.3} state_ms={:.3} sync_ms={:.3}",
                percentile(&mut direct_input_samples, 0.50),
                percentile(&mut direct_state_samples, 0.50),
                percentile(&mut direct_sync_samples, 0.50),
            );
            assert!(
                live_speedup >= 20.0,
                "live macro speedup={live_speedup:.1}x"
            );
            assert!(
                plock_speedup >= 20.0,
                "selected-step macro speedup={plock_speedup:.1}x"
            );
            return;
        }

        if probe == Project92UiProbe::EscapeDeselect {
            const TRACK: usize = 0;
            const STEP_COUNT: usize = 64;
            const SELECTED_COUNT: usize = 48;
            const WARMUPS: usize = 5;
            const SAMPLES: usize = 20;

            let sequencer_buffer_id = editor
                .buffers
                .iter()
                .find(|buffer| buffer.name == "*sequencer*")
                .expect("sequencer buffer should exist")
                .id;
            let sequencer_buffer_idx = editor
                .buffers
                .iter()
                .position(|buffer| buffer.id == sequencer_buffer_id)
                .expect("sequencer buffer index");
            let step_buffer_idx = editor
                .buffers
                .iter()
                .position(|buffer| buffer.name == "*step*")
                .expect("step buffer index");
            editor.set_active_buffer(sequencer_buffer_id);
            let non_sequencer_tile = editor
                .split_active_tile(eseqlisp::tile::SplitDir::Vertical, step_buffer_idx)
                .expect("split visible sequencer and step tiles");
            editor.switch_active_tile(non_sequencer_tile);
            let focused_buffer_name = editor.active_buffer().name.clone();
            assert_eq!(focused_buffer_name, "*step*");
            assert!(
                editor.tile_root.leaf_ids().into_iter().any(|tile_id| editor
                    .tile_root
                    .find_leaf(tile_id)
                    .is_some_and(|leaf| leaf.buffer_idx == sequencer_buffer_idx)),
                "the Escape benchmark must keep *sequencer* visible"
            );
            state.pattern.track_params[TRACK].set_num_steps(STEP_COUNT);
            for step in 0..STEP_COUNT {
                state.pattern.patterns[TRACK].set_step_active(step, step % 2 == 0);
            }
            sync_all_track_sequencer_state(
                editor.runtime_mut(),
                &state,
                &app,
                TRACK,
                &selected_steps,
            );
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);

            let mut samples = Vec::with_capacity(SAMPLES);
            for iteration in 0..(WARMUPS + SAMPLES) {
                selected_steps.lock().unwrap().extend(0..SELECTED_COUNT);
                let neural = selected_neural_neurons.lock().unwrap().clone();
                apply_ui_invalidations(
                    vec![UiInvalidation::StepSelection {
                        track: TRACK,
                        changed_steps: (0..SELECTED_COUNT).collect(),
                    }],
                    UiInvalidationApplyCtx {
                        app: &mut app,
                        editor: &mut editor,
                        state: &state,
                        track_collapsed: &track_collapsed,
                        bus_state: &bus_state,
                        current_track_idx: TRACK,
                        selected_steps: &selected_steps,
                        selected_neural_neurons: &neural,
                        piano_roll_selection: &piano_roll_selection,
                        accumulator_names: &accumulator_names,
                        cached_track_peak_levels: &cached_track_peak_levels,
                        cached_bus_peak_levels: &cached_bus_peak_levels,
                        record_armed: &record_armed,
                        active_delete_target: &active_delete_target,
                        active_delete_target_version: &active_delete_target_version,
                        expanded_step_projection: &expanded_step_projection,
                        fx_visible: true,
                        sequencer_visible: true,
                        mixer_visible: true,
                    },
                );
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                let frame =
                    eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);
                let sequencer_frame = frame
                    .tiles
                    .iter()
                    .find(|tile| tile.frame.buffer_name == "*sequencer*")
                    .expect("visible sequencer frame after selection setup");
                assert!(
                    !sequencer_frame.frame.dirty_widget_ids.is_empty(),
                    "selection setup from {focused_buffer_name} must dirty the visible *sequencer* step widgets"
                );
                let selected_layout = sequencer_frame
                    .frame
                    .widget_layout
                    .as_ref()
                    .expect("visible sequencer layout after selection setup");
                let viewport = eseqlisp::widget_render::WidgetViewport {
                    cell_w: 8.0,
                    cell_h: 16.0,
                    vp_w: 1440.0,
                    vp_h: 1120.0,
                    time_seconds: 0.0,
                    focused_widget_id: sequencer_frame.frame.focused_widget_id,
                    focused_branch: false,
                    overlay_viewport_bottom: 70.0,
                    scroll_top: sequencer_frame.frame.widget_scroll_top
                        + sequencer_frame.frame.text_scroll_top as f32,
                    scroll_left: sequencer_frame.frame.widget_layout_scroll_left,
                    inherited_hover: false,
                };
                let (mut retained_runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
                    selected_layout,
                    viewport,
                    viewport.scroll_top,
                    70,
                );
                let retained_run_indices =
                    eseqlisp::widget_render::build_metal_primitive_run_index(&retained_runs);
                let selected_step_frame = frame
                    .tiles
                    .iter()
                    .find(|tile| tile.frame.buffer_name == "*step*")
                    .expect("visible step frame after selection setup");
                let step_viewport = eseqlisp::widget_render::WidgetViewport {
                    focused_widget_id: selected_step_frame.frame.focused_widget_id,
                    focused_branch: true,
                    scroll_top: selected_step_frame.frame.widget_scroll_top
                        + selected_step_frame.frame.text_scroll_top as f32,
                    scroll_left: selected_step_frame.frame.widget_layout_scroll_left,
                    ..viewport
                };

                assert!(
                    editor.focus_widget_by_stable_key(
                        "eseq.effects.track-panels/step-param-velocity",
                        Some("number-picker")
                    ),
                    "the Escape benchmark must reproduce a focused *step* number picker"
                );

                let started = Instant::now();
                editor.handle_key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Esc,
                    crossterm::event::KeyModifiers::NONE,
                ));
                let invalidations = ui_invalidations.drain();
                assert!(
                    selected_steps.lock().unwrap().is_empty(),
                    "the real Escape binding must clear all selected steps"
                );
                assert_eq!(
                    invalidations,
                    vec![UiInvalidation::StepSelection {
                        track: TRACK,
                        changed_steps: (0..SELECTED_COUNT).collect(),
                    }],
                    "Escape must preserve the exact changed-step delta"
                );
                apply_ui_invalidations(
                    invalidations,
                    UiInvalidationApplyCtx {
                        app: &mut app,
                        editor: &mut editor,
                        state: &state,
                        track_collapsed: &track_collapsed,
                        bus_state: &bus_state,
                        current_track_idx: TRACK,
                        selected_steps: &selected_steps,
                        selected_neural_neurons: &neural,
                        piano_roll_selection: &piano_roll_selection,
                        accumulator_names: &accumulator_names,
                        cached_track_peak_levels: &cached_track_peak_levels,
                        cached_bus_peak_levels: &cached_bus_peak_levels,
                        record_armed: &record_armed,
                        active_delete_target: &active_delete_target,
                        active_delete_target_version: &active_delete_target_version,
                        expanded_step_projection: &expanded_step_projection,
                        fx_visible: true,
                        sequencer_visible: true,
                        mixer_visible: true,
                    },
                );
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                let frame =
                    eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);
                let sequencer_frame = frame
                    .tiles
                    .iter()
                    .find(|tile| tile.frame.buffer_name == "*sequencer*")
                    .expect("visible sequencer frame after Escape");
                assert!(
                    !sequencer_frame.frame.dirty_widget_ids.is_empty(),
                    "Escape from {focused_buffer_name} must dirty the visible *sequencer* step widgets in the same frame"
                );
                let deselected_layout = sequencer_frame
                    .frame
                    .widget_layout
                    .as_ref()
                    .expect("visible sequencer layout after Escape");
                let (_, retained_stats) =
                    eseqlisp::widget_render::refresh_metal_primitive_runs_retained_in_place(
                        deselected_layout,
                        viewport,
                        viewport.scroll_top,
                        70,
                        &mut retained_runs,
                        &retained_run_indices,
                        &sequencer_frame.frame.dirty_widget_ids,
                    );
                assert_eq!(
                    retained_stats.missing_previous_runs, 0,
                    "Escape render must update the retained scene without missing runs"
                );
                assert_eq!(
                    retained_stats.invalid_previous_runs, 0,
                    "Escape render must update the retained scene without invalid runs"
                );
                let deselected_step_frame = frame
                    .tiles
                    .iter()
                    .find(|tile| tile.frame.buffer_name == "*step*")
                    .expect("visible step frame after Escape");
                let deselected_step_layout = deselected_step_frame
                    .frame
                    .widget_layout
                    .as_ref()
                    .expect("visible step layout after Escape");
                let (deselected_step_runs, _) =
                    eseqlisp::widget_render::collect_metal_primitive_runs(
                        deselected_step_layout,
                        step_viewport,
                        step_viewport.scroll_top,
                        70,
                    );
                assert!(
                    !deselected_step_runs.is_empty(),
                    "Escape render must rebuild the active *step* Metal scene"
                );
                let selected_shells_after_escape = retained_runs
                    .iter()
                    .flat_map(|run| &run.primitives)
                    .filter(|primitive| {
                        matches!(
                            eseqlisp::widget_render::innermost_primitive(primitive),
                            eseqlisp::widget_render::MetalPrimitive::WidgetInstance {
                                widget_type,
                                instance,
                                ..
                            } if widget_type == "seqv-step-shell" && instance.uniform_b[0] > 0.5
                        )
                    })
                    .count();
                assert_eq!(
                    selected_shells_after_escape, 0,
                    "the retained Metal scene must contain no selected step shells after Escape"
                );
                if iteration >= WARMUPS {
                    samples.push(duration_ms(started.elapsed()));
                }
            }
            samples.sort_by(|a, b| a.total_cmp(b));
            let percentile = |fraction: f64| {
                let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
                samples[index]
            };
            eprintln!(
                "[project-92-escape-deselect] focused_buffer={} tracks={} steps={} active={} selected={} samples={} median_ms={:.3} p95_ms={:.3} min_ms={:.3} max_ms={:.3}",
                focused_buffer_name,
                app.tracks.len(),
                STEP_COUNT,
                STEP_COUNT / 2,
                SELECTED_COUNT,
                SAMPLES,
                percentile(0.50),
                percentile(0.95),
                samples[0],
                samples[samples.len() - 1],
            );
            return;
        }

        let before_revisions = visible_layout_revisions(&editor);
        let start_pattern = app.state.current_scene_index();
        let target_pattern = (start_pattern + 1) % app.state.scene_count();
        let ct = current_track.load(Ordering::Relaxed);
        let fx_visible = editor_has_visible_buffer(&editor, "*fx*");

        let measured = Instant::now();
        let switch_bus_elapsed;
        let state_switch_elapsed;
        let state_switch_profile;
        let apply_samples_elapsed;
        let restored_defaults_elapsed;
        let sync_names_pattern_elapsed;
        let sync_current_steps_elapsed;
        let sync_sequencer_elapsed;
        let sync_sequencer_profile;
        let sync_step_params_elapsed;
        let sync_mixer_elapsed;
        let sync_fx_lists_elapsed;
        let mut sync_effects_elapsed = Duration::ZERO;
        let mut sync_midi_effects_elapsed = Duration::ZERO;
        let mut sync_instrument_panel_elapsed = Duration::ZERO;
        let mut sync_accumulators_elapsed = Duration::ZERO;
        let sync_track_params_elapsed;
        let sync_fx_bindings_elapsed;
        let sync_plocks_sidebar_elapsed;
        let reactive_elapsed;
        let side_effects_elapsed;
        let mut mixer_refresh_elapsed = Duration::ZERO;

        let started = Instant::now();
        app.switch_bus_pattern(target_pattern);
        switch_bus_elapsed = started.elapsed();

        let started = Instant::now();
        let switched = app.state.switch_pattern_profiled(
            target_pattern,
            app.tracks.len(),
            &app.graph.track_buffer_ids,
            &app.graph.track_sample_rates,
            &app.tracks,
            &app.graph.track_instrument_types,
        );
        state_switch_elapsed = started.elapsed();
        let switched = switched.expect("project 92 scene switch should change sample ids");
        state_switch_profile = switched.profile;
        let sample_ids = switched.sample_ids;

        let started = Instant::now();
        app.graph_controller().apply_sample_ids(&sample_ids);
        let apply_ids_only = started.elapsed();
        app.graph_controller().sync_current_pattern_mod_routes();
        apply_samples_elapsed = started.elapsed();
        if std::env::var("ESEQ_SCENE_TRACE").is_ok_and(|v| v == "1") {
            eprintln!(
                "[apply-samples-trace] apply_ids_ms={:.3} mod_routes_ms={:.3}",
                duration_ms(apply_ids_only),
                duration_ms(apply_samples_elapsed - apply_ids_only)
            );
        }

        let started = Instant::now();
        app.push_all_restored_defaults();
        restored_defaults_elapsed = started.elapsed();

        {
            let rt = editor.runtime_mut();
            let started = Instant::now();
            sync_shared_track_collapsed(&track_collapsed, &app);
            sync_track_name_state(rt, &mut track_names, &app);
            sync_pattern_state(rt, &state);
            sync_names_pattern_elapsed = started.elapsed();

            let started = Instant::now();
            rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
            sync_current_steps_elapsed = started.elapsed();

            let started = Instant::now();
            sync_sequencer_profile =
                sync_all_track_sequencer_state_profiled(rt, &state, &app, ct, &selected_steps);
            sync_sequencer_elapsed = started.elapsed();

            let started = Instant::now();
            sync_step_param_lists(rt, &state, ct);
            sync_step_params_elapsed = started.elapsed();

            let started = Instant::now();
            sync_track_mixer_state(rt, &app, &state);
            sync_bus_mixer_state(rt, &app);
            sync_track_peak_fields(rt, &cached_track_peak_levels);
            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
            sync_mixer_elapsed = started.elapsed();

            let started = Instant::now();
            if fx_visible {
                let sub_started = Instant::now();
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    build_effects_value(&state, ct, &app.graph.effect_descriptors, &selected_steps),
                );
                sync_effects_elapsed = sub_started.elapsed();

                let sub_started = Instant::now();
                rt.set_reactive(
                    "SEQ",
                    "midi-effects",
                    build_midi_effects_value(&state, ct, &selected_steps),
                );
                sync_midi_effects_elapsed = sub_started.elapsed();

                let sub_started = Instant::now();
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, ct, &selected_steps),
                );
                sync_instrument_panel_elapsed = sub_started.elapsed();

                let sub_started = Instant::now();
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                sync_accumulators_elapsed = sub_started.elapsed();
            } else {
                fx_epoch.fetch_add(1, Ordering::Relaxed);
            }
            sync_fx_lists_elapsed = started.elapsed();

            let started = Instant::now();
            let selected_neural_snapshot = selected_neural_neurons.lock().unwrap().clone();
            sync_track_params_with_neural_selection(
                rt,
                &app,
                &state,
                ct,
                &selected_steps,
                Some(&selected_neural_snapshot),
            );
            sync_track_params_elapsed = started.elapsed();

            let started = Instant::now();
            sync_fx_param_binding_fields_with_neural_selection(
                rt,
                &app,
                &state,
                ct,
                &selected_steps,
                Some(&selected_neural_snapshot),
            );
            sync_fx_bindings_elapsed = started.elapsed();

            let started = Instant::now();
            rt.set_reactive(
                "SEQ",
                "step-has-plocks",
                build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
            );
            sync_sidebar_browser(rt, &app, ct);
            sync_plocks_sidebar_elapsed = started.elapsed();

            let started = Instant::now();
            rt.run_reactive_cycle();
            reactive_elapsed = started.elapsed();
        }

        let started = Instant::now();
        editor.refresh_runtime_side_effects();
        side_effects_elapsed = started.elapsed();

        if editor_has_visible_buffer(&editor, "*mixer*") {
            let started = Instant::now();
            editor.refresh_visible_layouts_for_buffer_named("*mixer*");
            mixer_refresh_elapsed = started.elapsed();
        }
        let elapsed = measured.elapsed();

        let after_revisions = visible_layout_revisions(&editor);
        let changed_buffers = changed_layout_buffers(&before_revisions, &after_revisions);
        let trace = editor
            .runtime()
            .last_ui_invalidation_trace()
            .expect("scene switch should produce an invalidation trace");
        let mut reactive_hot = trace.reactive_exec_timings.clone();
        reactive_hot.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        if std::env::var("ESEQ_SCENE_TRACE").is_ok_and(|v| v == "1") {
            eprintln!(
                "[project-92-scene-switch-fields] dirty_fields={:?}",
                trace.dirty_fields
            );
            for (label, elapsed) in &reactive_hot {
                eprintln!(
                    "[project-92-scene-switch-exec] {:.3}ms {}",
                    duration_ms(*elapsed),
                    label
                );
            }
        }
        let reactive_hot = reactive_hot
            .into_iter()
            .take(6)
            .map(|(label, elapsed)| (label, duration_ms(elapsed)))
            .collect::<Vec<_>>();
        let mut relayout_timings = Vec::<(String, String, f64)>::new();
        if trace.relayout_duration > Duration::ZERO {
            relayout_timings.push((
                editor.active_buffer().name.clone(),
                format!(
                    "active-{}",
                    trace.relayout_mode.as_deref().unwrap_or("unknown")
                ),
                duration_ms(trace.relayout_duration),
            ));
        }
        relayout_timings.extend(editor.last_layout_refresh_timings().iter().map(|timing| {
            (
                timing.buffer_name.clone(),
                format!(
                    "inactive-{}-tile-{}",
                    timing.mode,
                    timing
                        .tile_id
                        .map(|tile_id| tile_id.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ),
                duration_ms(timing.elapsed),
            )
        }));
        relayout_timings.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        let worst_relayout = relayout_timings.first().cloned();

        eprintln!(
            "[project-92-scene-switch] from={} to={} elapsed_ms={:.3} switch_bus_ms={:.3} state_switch_ms={:.3} apply_samples_ms={:.3} defaults_ms={:.3} names_pattern_ms={:.3} current_steps_ms={:.3} sequencer_bindings_ms={:.3} step_params_ms={:.3} mixer_ms={:.3} fx_lists_ms={:.3} effects_ms={:.3} midi_effects_ms={:.3} instrument_panel_ms={:.3} accumulators_ms={:.3} track_params_ms={:.3} fx_bindings_ms={:.3} plocks_sidebar_ms={:.3} reactive_ms={:.3} side_effects_ms={:.3} mixer_refresh_ms={:.3} changed_layout_buffers={:?} relayout_timings={:?} worst_relayout={:?} dirty_fields={} affected_buffers={:?} widget_tree_flushes={} full_reruns={} subtree_reruns={} relayout_mode={:?} relayout_ms={:.3} relayout_failure={:?}",
            start_pattern,
            target_pattern,
            duration_ms(elapsed),
            duration_ms(switch_bus_elapsed),
            duration_ms(state_switch_elapsed),
            duration_ms(apply_samples_elapsed),
            duration_ms(restored_defaults_elapsed),
            duration_ms(sync_names_pattern_elapsed),
            duration_ms(sync_current_steps_elapsed),
            duration_ms(sync_sequencer_elapsed),
            duration_ms(sync_step_params_elapsed),
            duration_ms(sync_mixer_elapsed),
            duration_ms(sync_fx_lists_elapsed),
            duration_ms(sync_effects_elapsed),
            duration_ms(sync_midi_effects_elapsed),
            duration_ms(sync_instrument_panel_elapsed),
            duration_ms(sync_accumulators_elapsed),
            duration_ms(sync_track_params_elapsed),
            duration_ms(sync_fx_bindings_elapsed),
            duration_ms(sync_plocks_sidebar_elapsed),
            duration_ms(reactive_elapsed),
            duration_ms(side_effects_elapsed),
            duration_ms(mixer_refresh_elapsed),
            changed_buffers,
            relayout_timings,
            worst_relayout,
            trace.dirty_fields.len(),
            trace.affected_buffers,
            trace.widget_tree_flushes,
            trace.full_buffer_reruns,
            trace.subtree_reruns,
            trace.relayout_mode,
            duration_ms(trace.relayout_duration),
            trace.relayout_failure_reason,
        );

        eprintln!(
            "[project-92-scene-switch-detail] state_total_ms={:.3} state_capture_ms={:.3} state_lock_wait_ms={:.3} state_save_current_ms={:.3} state_launch_data_ms={:.3} state_restore_tracks_ms={:.3} state_collect_samples_ms={:.3} state_update_atoms_ms={:.3} state_mod_resync_ms={:.3} state_publish_snapshot_ms={:.3} seq_total_ms={:.3} seq_track_steps_ms={:.3} seq_track_num_steps_ms={:.3} seq_track_timebases_ms={:.3} seq_track_duration_spans_ms={:.3} seq_track_step_has_plocks_ms={:.3} seq_track_playheads_ms={:.3} seq_track_velocities_ms={:.3} seq_track_durations_ms={:.3} seq_track_auxas_ms={:.3} seq_track_transposes_ms={:.3} seq_track_pans_ms={:.3} seq_track_syncs_ms={:.3} seq_track_delays_ms={:.3} seq_step_bindings_ms={:.3} seq_playhead_fields_ms={:.3} step_active_ms={:.3} step_duration_ms={:.3} step_plocked_ms={:.3} step_selected_ms={:.3} step_slider_ms={:.3} step_haptic_ms={:.3} step_active_sets={:?} step_duration_sets={:?} step_plocked_sets={:?} step_selected_sets={:?} step_slider_sets={:?} step_haptic_sets={:?} reactive_apply_ms={:.3} reactive_flush_ms={:.3} reactive_cycle_trace_ms={:.3} reactive_hot={:?}",
            duration_ms(state_switch_profile.total),
            duration_ms(state_switch_profile.capture_current_snapshot),
            duration_ms(state_switch_profile.scene_lock_wait),
            duration_ms(state_switch_profile.save_current_snapshot),
            duration_ms(state_switch_profile.launch_scene_data),
            duration_ms(state_switch_profile.restore_tracks),
            duration_ms(state_switch_profile.collect_sample_ids),
            duration_ms(state_switch_profile.update_pattern_atoms),
            duration_ms(state_switch_profile.schedule_mod_resync),
            duration_ms(state_switch_profile.publish_scheduler_snapshot),
            duration_ms(sync_sequencer_profile.elapsed),
            duration_ms(sync_sequencer_profile.track_steps),
            duration_ms(sync_sequencer_profile.track_num_steps),
            duration_ms(sync_sequencer_profile.track_timebases),
            duration_ms(sync_sequencer_profile.track_duration_spans),
            duration_ms(sync_sequencer_profile.track_step_has_plocks),
            duration_ms(sync_sequencer_profile.track_playheads),
            duration_ms(sync_sequencer_profile.track_velocities),
            duration_ms(sync_sequencer_profile.track_durations),
            duration_ms(sync_sequencer_profile.track_auxas),
            duration_ms(sync_sequencer_profile.track_transposes),
            duration_ms(sync_sequencer_profile.track_pans),
            duration_ms(sync_sequencer_profile.track_syncs),
            duration_ms(sync_sequencer_profile.track_delays),
            duration_ms(sync_sequencer_profile.step_bindings.elapsed),
            duration_ms(sync_sequencer_profile.playhead_fields),
            duration_ms(sync_sequencer_profile.step_bindings.active_elapsed),
            duration_ms(sync_sequencer_profile.step_bindings.duration_elapsed),
            duration_ms(sync_sequencer_profile.step_bindings.plocked_elapsed),
            duration_ms(sync_sequencer_profile.step_bindings.selected_elapsed),
            duration_ms(sync_sequencer_profile.step_bindings.slider_elapsed),
            duration_ms(sync_sequencer_profile.step_bindings.haptic_elapsed),
            sync_sequencer_profile.step_bindings.active_sets,
            sync_sequencer_profile.step_bindings.duration_sets,
            sync_sequencer_profile.step_bindings.plocked_sets,
            sync_sequencer_profile.step_bindings.selected_sets,
            sync_sequencer_profile.step_bindings.slider_sets,
            sync_sequencer_profile.step_bindings.haptic_sets,
            duration_ms(trace.reactive_apply_duration),
            duration_ms(trace.reactive_flush_duration),
            duration_ms(trace.reactive_cycle_duration),
            reactive_hot,
        );

        assert_eq!(
            app.state.current_scene_index(),
            target_pattern,
            "scene switch should update the current project scene"
        );
        assert!(
            trace.widget_tree_flushes > 0,
            "scene switch should report widget tree work"
        );
    }
    /// Arrangement-view end-to-end perf probe (UI_PERFORMANCE_TUNING.md).
    ///
    /// Loads saved project `pianohold` (7 tracks, 12 patterns, ~137 stored
    /// clips across 7 arrangement lanes plus an 18-event scene lane), toggles
    /// into the arrangement view through the real Tab binding, and drives the
    /// real pointer/gesture paths of the timeline lanes: clip-resize commit
    /// (the renderer number: song republish -> reactive -> frame -> retained),
    /// clip select, one live resize-drag tick, one live move-drag tick, one
    /// marquee tick and one horizontal pan tick. Each timed region covers all
    /// synchronous work required for the user-visible result: host command
    /// application, song read-surface publish, reactive cycle, tiled-frame
    /// build and retained Metal primitive refresh.
    #[test]
    #[ignore = "release-mode perf probe: initializes the real metal_seq app graph and loads crates/sequencer/projects/pianohold.json"]
    fn arrangement_view_interactions_end_to_end_perf() {
        std::thread::Builder::new()
            .name("arrangement-interactions-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(arrangement_view_interactions_end_to_end_perf_impl)
            .expect("spawn arrangement interaction probe")
            .join()
            .expect("arrangement interaction probe should pass");
    }
    fn arrangement_view_interactions_end_to_end_perf_impl() {
        use super::*;
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use std::collections::HashSet;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};
        fn duration_ms(duration: Duration) -> f64 {
            duration.as_secs_f64() * 1000.0
        }
        fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
            MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }
        }
        const PROJECT: &str = "pianohold";
        const VIEW_W: usize = 180;
        const VIEW_H: usize = 160;
        const VIEW_DURATION: f64 = 64.0;
        const WARMUPS: usize = 5;
        const SAMPLES: usize = 20;
        let _dir = SequencerDirGuard::enter();
        assert!(
            Path::new("projects/pianohold.json").exists(),
            "project pianohold must be available at crates/sequencer/projects/pianohold.json"
        );
        let eng = engine::init_headless_engine(44_100, 2).expect("initialize headless app graph");
        let lg_raw = eng.lg_ptr.0;
        let state = eng.state.clone();
        let lg_ptr = eng.lg_ptr;
        let sample_rate = eng.sample_rate;
        let _engine_guard = TestEngineGuard { lg_raw };
        let _audio_pump = HeadlessAudioPump::start(lg_ptr, eng.channels as usize);
        let master_recorder = eng.master_recorder.clone();
        let mut app = app::App::new(
            state.clone(),
            lg_ptr,
            sample_rate,
            eng.buses,
            eng.master_recorder,
            eng.keyboard_tx,
        );
        let mut track_names = Vec::<String>::new();
        let track_pan_ids = Arc::new(Mutex::new(Vec::<i32>::new()));
        let track_collapsed = Arc::new(Mutex::new(app.track_collapsed.clone()));
        let bus_state = Arc::new(Mutex::new(app.buses.clone()));
        let bus_node_ids = Arc::new(Mutex::new(app.graph.bus_node_ids.clone()));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_tracks = Arc::new(Mutex::new(HashSet::<usize>::new()));
        let track_groups = Arc::new(Mutex::new(app.groups.clone()));
        let selected_steps = Arc::new(Mutex::new(HashSet::<usize>::new()));
        let selected_neural_neurons: sequencer::lisp_host::SharedSelectedNeuralNeurons =
            Arc::new(Mutex::new(BTreeSet::new()));
        let piano_roll_selection = Arc::new(Mutex::new(HashSet::<u64>::new()));
        let piano_roll_move_state = Arc::new(Mutex::new(None));
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        let fx_epoch = Arc::new(AtomicUsize::new(0));
        let ui_invalidations = Arc::new(UiInvalidationQueue::new());
        let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
        let recording = Arc::new(AtomicBool::new(false));
        let master_recording = Arc::new(AtomicBool::new(false));
        let record_armed = Arc::new(Mutex::new(Vec::<bool>::new()));
        let armed_rack: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let active_delete_target = Arc::new(Mutex::new(None));
        let active_delete_target_version = Arc::new(AtomicUsize::new(0));
        let auto_follow_override_until = Arc::new(Mutex::new(None));
        let RuntimeInit {
            runtime,
            accumulator_names,
            midi_fx_names: _,
            sample_browser: _,
            piano_roll_clipboard: _,
            selected_drum_lane_steps: _,
        } = init_runtime(
            &app,
            state.clone(),
            &track_names,
            track_pan_ids.clone(),
            track_collapsed.clone(),
            bus_state.clone(),
            bus_node_ids.clone(),
            current_track.clone(),
            selected_tracks.clone(),
            track_groups.clone(),
            selected_steps.clone(),
            piano_roll_selection.clone(),
            piano_roll_move_state,
            super::new_shared_piano_roll_focus(),
            recording.clone(),
            master_recording.clone(),
            master_recorder.clone(),
            record_armed.clone(),
            armed_rack.clone(),
            ui_epoch.clone(),
            fx_epoch.clone(),
            ui_invalidations.clone(),
            expanded_step_projection.clone(),
            selected_neural_neurons.clone(),
            active_delete_target.clone(),
            active_delete_target_version.clone(),
            auto_follow_override_until.clone(),
            lg_raw,
        );
        let mut editor = Editor::new(
            runtime,
            eseqlisp::EditorConfig {
                vim_mode: true,
                ..eseqlisp::EditorConfig::default()
            },
        );
        reload_custom_instrument_ui(&mut editor);
        let _ = editor.open_or_create_file_buffer(UI_ENTRYPOINT_PATH);
        let grid_source = editor.active_buffer().text();
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            Some(std::path::PathBuf::from(UI_ENTRYPOINT_PATH)),
            &grid_source,
            overlays,
        );
        assert!(
            report.success,
            "failed to load grid UI: {}",
            report.failure_message()
        );
        editor.process_lisp_reload_report(report);
        editor.refresh_runtime_side_effects();
        reload_custom_instrument_ui(&mut editor);
        editor.set_layout_viewport(VIEW_W as u16, VIEW_H as u16);
        editor.update_tile_rects(VIEW_W as u16, VIEW_H as u16);
        let _ = editor.drain_host_commands();
        app.queue_project_load_named(PROJECT)
            .expect("queue pianohold load");
        for _ in 0..2048 {
            if !app.has_pending_project_load() {
                break;
            }
            app.advance_pending_project_load()
                .expect("advance pianohold load");
        }
        assert!(
            !app.has_pending_project_load(),
            "pianohold load did not finish"
        );
        assert!(
            app.state.committed_arrangement().is_some(),
            "pianohold must carry a committed arrangement"
        );
        current_track.store(0, Ordering::Relaxed);
        *track_pan_ids.lock().unwrap() = app
            .graph
            .track_node_ids
            .iter()
            .map(|ids| ids.pan_id)
            .collect();
        *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
        *record_armed.lock().unwrap() = vec![false; app.tracks.len()];
        sync_shared_track_collapsed(&track_collapsed, &app);
        push_project_scratch_to_named_buffer(&mut editor, &app);
        if let Err(error) = evaluate_project_scratch_on_ui_runtime(&mut editor, &app) {
            editor.handle_host_event(HostEvent::Status(format!("Scratch UI eval error: {error}")));
        }
        let cached_track_peak_levels = vec![0.0; app.tracks.len()];
        let cached_bus_peak_levels = read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
        let (cached_modulator_phases, cached_modulator_levels) =
            read_modulator_display_values(app.graph.lg, &app);
        let mut song_frame = SongFrameState::default();
        {
            let rt = editor.runtime_mut();
            sync_project_state(rt, &app);
            sync_track_topology_state(
                rt,
                &app,
                &state,
                &mut track_names,
                0,
                &selected_steps,
                &piano_roll_selection,
                &accumulator_names,
                &record_armed,
                &cached_track_peak_levels,
            );
            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
            sync_modulator_phase_fields(rt, &cached_modulator_phases);
            sync_modulator_level_fields(rt, &cached_modulator_levels);
            sync_song_state(rt, &app, &mut song_frame, true);
            rt.run_reactive_cycle();
        }
        editor.refresh_runtime_side_effects();
        refresh_visible_track_topology_layouts(&mut editor);
        editor.update_tile_rects(VIEW_W as u16, VIEW_H as u16);
        let _ = editor.drain_host_commands();
        let eval = |editor: &mut Editor, expr: &str| {
            editor
                .runtime_mut()
                .eval_str(expr)
                .unwrap_or_else(|error| panic!("{expr}: {error:?}"))
        };
        let read_num = |editor: &mut Editor, expr: &str| -> f64 {
            match eval(editor, expr) {
                Some(Value::Number(n)) => n,
                other => panic!("{expr}: expected a number, got {other:?}"),
            }
        };
        // Focus the sequencer tile, then toggle to the arrangement view
        // through the production Tab binding.
        let sequencer_buffer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_buffer_id);
        editor.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            KeyModifiers::NONE,
        ));
        editor.refresh_runtime_side_effects();
        assert_eq!(
            eval(&mut editor, "(eseq.seq-step-tabs/seq-arrangement-view?)"),
            Some(Value::Bool(true)),
            "Tab must toggle into the arrangement view"
        );
        // Pin the shared time axis so gesture geometry is deterministic.
        eval(
            &mut editor,
            "(do (set! eseq.arrangement/view-duration 64) (eseq.arrangement/set-view-start 0 64))",
        );
        editor.refresh_runtime_side_effects();
        editor.update_tile_rects(VIEW_W as u16, VIEW_H as u16);
        let _ = editor.drain_host_commands();
        let initial_frame =
            eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, VIEW_W, VIEW_H);
        let initial_arr_frame = initial_frame
            .tiles
            .iter()
            .find(|tile| tile.frame.buffer_name == "*arrangement*")
            .expect("visible initial arrangement frame");
        let initial_layout = initial_arr_frame
            .frame
            .widget_layout
            .as_ref()
            .expect("initial arrangement layout");
        let viewport = eseqlisp::widget_render::WidgetViewport {
            cell_w: 8.0,
            cell_h: 16.0,
            vp_w: 1440.0,
            vp_h: 1120.0,
            time_seconds: 0.0,
            focused_widget_id: initial_arr_frame.frame.focused_widget_id,
            focused_branch: true,
            overlay_viewport_bottom: VIEW_H as f32,
            scroll_top: initial_arr_frame.frame.widget_scroll_top
                + initial_arr_frame.frame.text_scroll_top as f32,
            scroll_left: initial_arr_frame.frame.widget_layout_scroll_left,
            inherited_hover: false,
        };
        let (mut retained_runs, _) = eseqlisp::widget_render::collect_metal_primitive_runs(
            initial_layout,
            viewport,
            viewport.scroll_top,
            VIEW_H as u16,
        );
        let retained_run_indices =
            eseqlisp::widget_render::build_metal_primitive_run_index(&retained_runs);
        // The arrangement tile's screen origin: widget layouts are
        // tile-content-local, while handle_mouse_precise takes screen cells.
        let tile_origin = (
            initial_arr_frame.rect.col,
            initial_arr_frame.rect.row,
        );
        let send_mouse = |editor: &mut Editor, kind: MouseEventKind, col: f32, row: f32| {
            let screen_col = tile_origin.0 + col;
            let screen_row = tile_origin.1 + row;
            editor.handle_mouse_precise(
                mouse_event(kind, screen_col.floor() as u16, screen_row.floor() as u16),
                tile_origin.0 as u16,
                tile_origin.1 as u16,
                VIEW_W as u16,
                VIEW_H as u16,
                screen_col,
                screen_row,
            );
        };
        // Apply drained host commands the way the production loop seams do:
        // song editing primitives through apply_song_edit_command, the
        // transport-side selection/region commands through their app methods.
        let apply_pending_song_commands = |editor: &mut Editor, app: &mut app::App| {
            let commands = editor.drain_host_commands();
            for command in commands {
                let HostCommand::Custom { name, payload } = command else {
                    continue;
                };
                let field = |key: &str| -> Option<f64> {
                    match &payload {
                        Value::Map(map) => map.get(key).and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n),
                            _ => None,
                        }),
                        _ => None,
                    }
                };
                match name.as_str() {
                    "song-select-clip" => {
                        let track = field("track").expect("select track") as usize;
                        let clip_id = sequencer::sequencer::ClipId(
                            field("clip-id").expect("select clip id") as u64,
                        );
                        let span = match (field("start"), field("end")) {
                            (Some(start), Some(end)) => Some((start, end)),
                            _ => None,
                        };
                        app.select_song_clip_span(track, clip_id, span)
                            .expect("select song clip");
                    }
                    "song-deselect-clip" => {
                        app.set_song_clip_selection(None);
                    }
                    "song-set-region" => {
                        let scene_lane = matches!(
                            &payload,
                            Value::Map(map) if matches!(
                                map.get("scene-lane").map(|cell| cell.borrow().clone()),
                                Some(Value::Bool(true))
                            )
                        );
                        app.set_song_region(app::song_region::SongRegionSelection::new_in_lane(
                            field("track-a").expect("region track-a") as usize,
                            field("track-b").expect("region track-b") as usize,
                            field("start").expect("region start"),
                            field("end").expect("region end"),
                            scene_lane,
                        ));
                    }
                    "song-clear-region" => {
                        app.clear_song_region();
                    }
                    "song-set-arr-cursor" => {
                        let beat = field("time").expect("cursor time");
                        let track = field("track").unwrap_or(-1.0);
                        app.set_arrangement_cursor(beat, track as isize);
                    }
                    _ => {
                        let applied = apply_song_edit_command(&name, &payload, &mut *app)
                            .unwrap_or_else(|| panic!("unexpected benchmark host command {name}"));
                        applied.unwrap_or_else(|error| {
                            panic!("apply benchmark song command {name}: {error}")
                        });
                    }
                }
            }
        };
        // The visible-update phases the production frame performs after an
        // arrangement mutation: song read-surface publish, reactive cycle,
        // tiled-frame build, retained Metal refresh.
        let mut finish_visible_update = |editor: &mut Editor,
                                         app: &mut app::App,
                                         song_frame: &mut SongFrameState| {
            let started = Instant::now();
            sync_song_state(editor.runtime_mut(), app, song_frame, true);
            let sync_done = Instant::now();
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let reactive_done = Instant::now();
            let frame =
                eseqlisp::frame::build_tiled_render_frame_borderless(editor, VIEW_W, VIEW_H);
            let arr_frame = frame
                .tiles
                .iter()
                .find(|tile| tile.frame.buffer_name == "*arrangement*")
                .expect("visible arrangement frame after benchmark action");
            let layout = arr_frame
                .frame
                .widget_layout
                .as_ref()
                .expect("arrangement layout after benchmark action");
            let frame_done = Instant::now();
            let (_, stats) =
                eseqlisp::widget_render::refresh_metal_primitive_runs_retained_in_place(
                    layout,
                    viewport,
                    viewport.scroll_top,
                    VIEW_H as u16,
                    &mut retained_runs,
                    &retained_run_indices,
                    &arr_frame.frame.dirty_widget_ids,
                );
            assert_eq!(stats.missing_previous_runs, 0);
            assert_eq!(stats.invalid_previous_runs, 0);
            let retained_done = Instant::now();
            (
                duration_ms(sync_done - started),
                duration_ms(reactive_done - sync_done),
                duration_ms(frame_done - reactive_done),
                duration_ms(retained_done - frame_done),
                stats.rebuilt_runs,
            )
        };
        // Fixture clip: a PATTERN clip (take windows change the drawn :end)
        // inside the pinned 64-beat view, at least 8 beats long, with at
        // least 2 beats of empty lane after it so the end-edge grab cannot
        // land on the next clip's start handle. Searched across all tracks.
        let track_count = read_num(&mut editor, "(len SEQ.song-lanes)") as usize;
        let mut fixture: Option<(usize, f64, f64, f64, usize)> = None;
        'outer: for track in 0..track_count {
            let clip_count = read_num(
                &mut editor,
                &format!("(len (eseq.arrangement/track-clips {track}))"),
            ) as usize;
            for index in 0..clip_count {
                let clip = |editor: &mut Editor, key: &str| {
                    eval(
                        editor,
                        &format!("(get (nth (eseq.arrangement/track-clips {track}) {index}) :{key})"),
                    )
                };
                let Some(Value::Number(start)) = clip(&mut editor, "start-beat") else {
                    continue;
                };
                let Some(Value::Number(end)) = clip(&mut editor, "end-beat") else {
                    continue;
                };
                if !(start >= 0.0 && end - start >= 8.0 && end - start <= 48.0) {
                    continue;
                }
                if !matches!(clip(&mut editor, "take-id"), Some(Value::Nil)) {
                    continue;
                }
                let next_start = if index + 1 < clip_count {
                    read_num(
                        &mut editor,
                        &format!(
                            "(get (nth (eseq.arrangement/track-clips {track}) {}) :start-beat)",
                            index + 1
                        ),
                    )
                } else {
                    f64::INFINITY
                };
                if next_start - end < 2.0 {
                    continue;
                }
                let Some(Value::Number(id)) = clip(&mut editor, "clip-id") else {
                    continue;
                };
                fixture = Some((track, id, start, end, index));
                break 'outer;
            }
        }
        let (fixture_track, clip_id, clip_start, clip_end, clip_index) = fixture.expect(
            "pianohold must expose a >=8-beat pattern clip with trailing space inside the view",
        );
        let clip_count = read_num(
            &mut editor,
            &format!("(len (eseq.arrangement/track-clips {fixture_track}))"),
        ) as usize;
        // Center the shared time axis on the fixture clip so its geometry is
        // on screen (the qualifying clip is usually late in the song).
        let view_start = ((clip_start + clip_end - VIEW_DURATION) * 0.5).max(0.0).floor();
        eval(
            &mut editor,
            &format!("(eseq.arrangement/set-view-start {view_start} 64)"),
        );
        // The setter clamps against the scroll extent; use what it kept.
        let view_start = read_num(&mut editor, "eseq.arrangement/view-start");
        editor.refresh_runtime_side_effects();
        let _ = editor.widget_layout();
        let lane_rect = |editor: &mut Editor| -> (f32, f32, f32, f32) {
            let layout = editor.widget_layout().expect("arrangement layout");
            let key = format!("/track-lane-{fixture_track}");
            let container = find_layout_node_by_stable_key_suffix(&layout, &key)
                .expect("fixture track lane container");
            let lane = find_layout_node_by_widget_type(container, "timeline")
                .expect("track 0 timeline instance");
            (
                lane.rect.col,
                lane.rect.row,
                lane.rect.width,
                lane.rect.height,
            )
        };
        let time_to_col = move |lane: (f32, f32, f32, f32), beat: f64| -> f32 {
            lane.0 + lane.2 * (((beat - view_start) / VIEW_DURATION) as f32)
        };
        let percentile = |samples: &mut Vec<f64>, fraction: f64| {
            samples.sort_by(|a, b| a.total_cmp(b));
            let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
            samples[index]
        };
        let mut commit_samples = Vec::with_capacity(SAMPLES);
        let mut commit_phases: (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut select_samples = Vec::with_capacity(SAMPLES);
        let mut resize_samples = Vec::with_capacity(SAMPLES);
        let mut resize_phases: (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut move_samples = Vec::with_capacity(SAMPLES);
        let mut marquee_samples = Vec::with_capacity(SAMPLES);
        let mut scroll_samples = Vec::with_capacity(SAMPLES);
        for iteration in 0..(WARMUPS + SAMPLES) {
            // arr-commit-resize: the renderer number. A real song edit
            // (clip-resize primitive) republished through the full pipeline.
            let shrunk_end = clip_end - 1.0;
            let apply_clip_resize = |app: &mut app::App, end: f64| {
                let payload = Value::Map(
                    [
                        ("clip-id", Value::Number(clip_id)),
                        ("start-beat", Value::Number(clip_start)),
                        ("end-beat", Value::Number(end)),
                    ]
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
                    .collect::<HashMap<_, _>>(),
                );
                apply_song_edit_command("arrangement-clip-resize", &payload, &mut *app)
                    .expect("clip-resize is a song edit command")
                    .expect("apply benchmark clip resize");
            };
            let started = Instant::now();
            apply_clip_resize(&mut app, shrunk_end);
            let phases = finish_visible_update(&mut editor, &mut app, &mut song_frame);
            let elapsed = duration_ms(started.elapsed());
            assert_eq!(
                read_num(
                    &mut editor,
                    &format!("(get (nth (eseq.arrangement/track-clips {fixture_track}) {clip_index}) :end-beat)"),
                ),
                shrunk_end,
                "clip-resize commit must republish the shrunk clip"
            );
            if iteration >= WARMUPS {
                commit_samples.push(elapsed);
                commit_phases.0.push(phases.0);
                commit_phases.1.push(phases.1);
                commit_phases.2.push(phases.2);
                commit_phases.3.push(phases.3);
            }
            // Restore outside the sample.
            apply_clip_resize(&mut app, clip_end);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            // arr-select-clip: real mouse down+up on the clip title bar.
            let lane = lane_rect(&mut editor);
            let title_col = time_to_col(lane, (clip_start + clip_end) * 0.5);
            let title_row = lane.1 + 0.3;
            let started = Instant::now();
            for kind in [
                MouseEventKind::Down(MouseButton::Left),
                MouseEventKind::Up(MouseButton::Left),
            ] {
                send_mouse(&mut editor, kind, title_col, title_row);
            }
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            let elapsed = duration_ms(started.elapsed());
            assert_eq!(
                read_num(&mut editor, &format!("(len (eseq.arrangement/lane-selection {fixture_track}))")),
                1.0,
                "clicking the clip title bar must select the clip"
            );
            if iteration >= WARMUPS {
                select_samples.push(elapsed);
            }
            // Deselect outside the sample.
            eval(
                &mut editor,
                &format!("(eseq.arrangement/track-action {fixture_track} (dict :type :clear-selection :time 0))"),
            );
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            // arr-resize-tick: arm the real end-edge drag, then time ONE
            // live drag tick (ghost update through reactive/frame/retained).
            let lane = lane_rect(&mut editor);
            let edge_col = time_to_col(lane, clip_end) - 0.2;
            let target_col = time_to_col(lane, clip_end - 8.0);
            let row = lane.1 + 0.4;
            send_mouse(&mut editor, MouseEventKind::Down(MouseButton::Left), edge_col, row);
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            let started = Instant::now();
            send_mouse(&mut editor, MouseEventKind::Drag(MouseButton::Left), target_col, row);
            let phases = finish_visible_update(&mut editor, &mut app, &mut song_frame);
            let elapsed = duration_ms(started.elapsed());
            assert!(
                phases.4 > 0,
                "the resize tick must repaint retained runs in the same frame                  (ghost channels must dirty the lane widget)"
            );
            assert_eq!(
                eval(&mut editor, "(eseq.arrangement/track-drag-kind)"),
                Some(Value::Keyword("track-resize".to_string())),
                "the live resize tick must be previewing through the drag state"
            );
            assert!(
                read_num(
                    &mut editor,
                    &format!(
                        "(reactive-get \"SEQV\" (eseq.arrangement/channel \"ghost-kind\" {fixture_track}))"
                    ),
                ) >= 2.0,
                "the resize tick must publish the lane ghost channel"
            );
            assert!(
                read_num(
                    &mut editor,
                    &format!(
                        "(reactive-get \"SEQV\" (eseq.arrangement/channel \"ghost-time\" {fixture_track}))"
                    ),
                ) < clip_end,
                "the resize ghost must shorten the drawn clip"
            );
            if iteration >= WARMUPS {
                resize_samples.push(elapsed);
                resize_phases.0.push(phases.0);
                resize_phases.1.push(phases.1);
                resize_phases.2.push(phases.2);
                resize_phases.3.push(phases.3);
            }
            // Return to the original edge and release outside the sample.
            send_mouse(&mut editor, MouseEventKind::Drag(MouseButton::Left), edge_col, row);
            send_mouse(&mut editor, MouseEventKind::Up(MouseButton::Left), edge_col, row);
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            assert_eq!(eval(&mut editor, "eseq.arrangement/track-drag"), Some(Value::Nil));
            // Whatever the release committed, restore the fixture geometry.
            apply_clip_resize(&mut app, clip_end);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            eval(
                &mut editor,
                &format!("(eseq.arrangement/track-action {fixture_track} (dict :type :clear-selection :time 0))"),
            );
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            // arr-move-tick: arm the title-bar drag, time one move tick.
            let lane = lane_rect(&mut editor);
            let title_col = time_to_col(lane, (clip_start + clip_end) * 0.5);
            let move_target_col = time_to_col(lane, (clip_start + clip_end) * 0.5 + 8.0);
            let row = lane.1 + 0.4;
            send_mouse(&mut editor, MouseEventKind::Down(MouseButton::Left), title_col, row);
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            let started = Instant::now();
            send_mouse(&mut editor, MouseEventKind::Drag(MouseButton::Left), move_target_col, row);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            let elapsed = duration_ms(started.elapsed());
            assert!(
                matches!(
                    eval(&mut editor, "(eseq.arrangement/track-drag-kind)"),
                    Some(Value::Keyword(ref kind)) if kind == "track-move" || kind == "region-move"
                ),
                "the live move tick must be previewing through the drag state"
            );
            if iteration >= WARMUPS {
                move_samples.push(elapsed);
            }
            // Return and release outside the sample.
            send_mouse(&mut editor, MouseEventKind::Drag(MouseButton::Left), title_col, row);
            send_mouse(&mut editor, MouseEventKind::Up(MouseButton::Left), title_col, row);
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            if read_num(
                &mut editor,
                &format!("(get (nth (eseq.arrangement/track-clips {fixture_track}) {clip_index}) :start-beat)"),
            ) != clip_start
            {
                let restore_payload = Value::Map(
                    [
                        ("clip-id", Value::Number(clip_id)),
                        ("start-beat", Value::Number(clip_start)),
                    ]
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
                    .collect::<HashMap<_, _>>(),
                );
                apply_song_edit_command("arrangement-clip-move", &restore_payload, &mut app)
                    .expect("clip-move is a song edit command")
                    .expect("restore benchmark clip position");
            }
            eval(
                &mut editor,
                &format!("(eseq.arrangement/track-action {fixture_track} (dict :type :clear-selection :time 0))"),
            );
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            // arr-marquee-tick: sweep a region from the clip body.
            let lane = lane_rect(&mut editor);
            let body_col = time_to_col(lane, clip_start + 1.0);
            let body_row = lane.1 + 1.5;
            let sweep_col = time_to_col(lane, clip_start + 9.0);
            send_mouse(&mut editor, MouseEventKind::Down(MouseButton::Left), body_col, body_row);
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            let started = Instant::now();
            send_mouse(&mut editor, MouseEventKind::Drag(MouseButton::Left), sweep_col, body_row);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            let elapsed = duration_ms(started.elapsed());
            assert!(
                eval(&mut editor, "eseq.arrangement/region-ghost") != Some(Value::Nil),
                "the live marquee tick must be previewing the region ghost"
            );
            if iteration >= WARMUPS {
                marquee_samples.push(elapsed);
            }
            send_mouse(&mut editor, MouseEventKind::Up(MouseButton::Left), sweep_col, body_row);
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            app.clear_song_region();
            eval(
                &mut editor,
                &format!("(eseq.arrangement/track-action {fixture_track} (dict :type :clear-selection :time 0))"),
            );
            apply_pending_song_commands(&mut editor, &mut app);
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            // arr-scroll-tick: one horizontal pan of the shared time axis
            // (this reruns every lane: the whole-view rebuild path).
            let lane = lane_rect(&mut editor);
            let center_col = lane.0 + lane.2 * 0.5;
            let center_row = lane.1 + lane.3 * 0.5;
            let started = Instant::now();
            // Pan LEFT: the fixture view sits at the clamped right edge of
            // the scroll extent, so a rightward pan would no-op.
            assert!(editor.handle_touchpad_scroll(
                tile_origin.0 as u16,
                tile_origin.1 as u16,
                tile_origin.0 + center_col,
                tile_origin.1 + center_row,
                40.0,
                0.5
            ));
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
            let elapsed = duration_ms(started.elapsed());
            assert!(
                read_num(&mut editor, "eseq.arrangement/view-start") != view_start,
                "the pan tick must move the shared time axis"
            );
            if iteration >= WARMUPS {
                scroll_samples.push(elapsed);
            }
            // Return the axis outside the sample.
            eval(
                &mut editor,
                &format!("(eseq.arrangement/set-view-start {view_start} 64)"),
            );
            finish_visible_update(&mut editor, &mut app, &mut song_frame);
        }
        // Pre-tuning medians recorded by this same probe on pianohold before
        // the arrangement tuning work. The 10x guardrail is enforced against
        // the pre-tuning baselines once ARR_ENFORCE_TENFOLD is on.
        for (name, reference_ms, samples) in [
            (
                "commit-resize",
                ARR_BASELINE_COMMIT_RESIZE_MS,
                &mut commit_samples,
            ),
            (
                "select-clip",
                ARR_BASELINE_SELECT_CLIP_MS,
                &mut select_samples,
            ),
            (
                "resize-tick",
                ARR_BASELINE_RESIZE_TICK_MS,
                &mut resize_samples,
            ),
            ("move-tick", ARR_BASELINE_MOVE_TICK_MS, &mut move_samples),
            (
                "marquee-tick",
                ARR_BASELINE_MARQUEE_TICK_MS,
                &mut marquee_samples,
            ),
            (
                "scroll-tick",
                ARR_BASELINE_SCROLL_TICK_MS,
                &mut scroll_samples,
            ),
        ] {
            let median = percentile(samples, 0.50);
            eprintln!(
                "[arrangement-{name}] tracks={} clips={} samples={} median_ms={:.3} p95_ms={:.3} speedup={:.1}x",
                app.tracks.len(),
                clip_count,
                SAMPLES,
                median,
                percentile(samples, 0.95),
                reference_ms / median,
            );
            if ARR_ENFORCE_TENFOLD {
                assert!(
                    median <= reference_ms / 10.0,
                    "{name} median {median:.3} ms did not reach 10x versus the {reference_ms:.3} ms baseline",
                );
            }
        }
        eprintln!(
            "[arrangement-commit-resize-visible-phases] sync_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
            percentile(&mut commit_phases.0, 0.50),
            percentile(&mut commit_phases.1, 0.50),
            percentile(&mut commit_phases.2, 0.50),
            percentile(&mut commit_phases.3, 0.50),
        );
        eprintln!(
            "[arrangement-resize-tick-visible-phases] sync_ms={:.3} reactive_ms={:.3} frame_ms={:.3} retained_ms={:.3}",
            percentile(&mut resize_phases.0, 0.50),
            percentile(&mut resize_phases.1, 0.50),
            percentile(&mut resize_phases.2, 0.50),
            percentile(&mut resize_phases.3, 0.50),
        );
    }
    /// Pre-tuning medians for the arrangement probe (see
    /// arrangement_view_interactions_end_to_end_perf). Placeholders until the
    /// baseline run records them; the guardrail switches on with
    /// ARR_ENFORCE_TENFOLD once tuning lands.
    const ARR_BASELINE_COMMIT_RESIZE_MS: f64 = 146.821;
    const ARR_BASELINE_SELECT_CLIP_MS: f64 = 121.338;
    const ARR_BASELINE_RESIZE_TICK_MS: f64 = 66.364;
    const ARR_BASELINE_MOVE_TICK_MS: f64 = 62.763;
    const ARR_BASELINE_MARQUEE_TICK_MS: f64 = 73.331;
    const ARR_BASELINE_SCROLL_TICK_MS: f64 = 60.966;
    const ARR_ENFORCE_TENFOLD: bool = true;
