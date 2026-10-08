use super::*;

#[test]
fn edit_instrument_menu_tracks_selection_and_opens_existing_editor() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor
        .runtime_mut()
        .eval_str(
            r#"
        (set-window-buffer "*transport*")
        (eseq.transport/open-application-menu "Edit" (dict :at (dict :col 2 :row 1)))
    "#,
        )
        .unwrap();
    for (kind, instrument, enabled) in [
        ("sampler", "", false),
        ("custom", "core/drift", true),
        ("custom", "Synths/Heat", true),
        ("rack", "Rack", false),
        ("empty", "", false),
    ] {
        seed_browser_tracks(&mut editor, &[kind], 0);
        set_kind_field(
            &mut editor,
            "browser",
            "instrument",
            Value::String(instrument.to_string()),
        );
        editor.refresh_runtime_side_effects();
        let layout = editor.widget_layout().unwrap();
        let item = find_layout_node_by_stable_key_suffix(&layout, "/edit-menu-instrument")
            .expect("edit instrument action");
        assert_finite_nonzero_rect(item, "edit instrument action");
        assert!(
            matches!(item.props.get("disabled"), Some(Value::Bool(value)) if *value == !enabled),
            "{kind} {instrument:?}: {:?}",
            item.props.get("disabled")
        );
        if enabled {
            editor.drain_host_commands();
            let callback = editor.runtime_mut().eval_str(
                "(get (nth (filter (lambda (item) (and item (= (get item :id) \"edit-menu-instrument\"))) (eseq.transport/application-menu-items \"Edit\")) 0) :on-select)"
            ).unwrap().unwrap();
            editor.runtime_mut().invoke(callback, vec![]).unwrap();
            assert!(editor.drain_host_commands().iter().any(|event| matches!(event,
                HostCommand::Custom { name, payload: Value::Map(map) }
                if name == "enter-edit-instrument" && map_string(map, "name").as_deref() == Some(instrument))));
        }
    }
}

/// eseq-63j4.4: Edit > Reload Instrument From Disk / Rescan Instruments &
/// Effects pick up files a coding agent wrote while the app runs.
#[test]
fn reload_from_disk_menu_items_queue_their_host_commands() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor
        .runtime_mut()
        .eval_str(
            r#"
        (set-window-buffer "*transport*")
        (eseq.transport/open-application-menu "Edit" (dict :at (dict :col 2 :row 1)))
    "#,
        )
        .unwrap();
    let invoke = |editor: &mut eseqlisp::Editor, id: &str| {
        editor.drain_host_commands();
        let callback = editor.runtime_mut().eval_str(&format!(
            "(get (nth (filter (lambda (item) (and item (= (get item :id) \"{id}\"))) (eseq.transport/application-menu-items \"Edit\")) 0) :on-select)"
        )).unwrap().unwrap();
        editor.runtime_mut().invoke(callback, vec![]).unwrap();
        editor.drain_host_commands()
    };
    for (kind, enabled) in [("custom", true), ("sampler", false)] {
        seed_browser_tracks(&mut editor, &[kind], 0);
        editor.refresh_runtime_side_effects();
        let layout = editor.widget_layout().unwrap();
        let item = find_layout_node_by_stable_key_suffix(&layout, "/edit-menu-reload-instrument")
            .expect("reload instrument action");
        assert!(
            matches!(item.props.get("disabled"), Some(Value::Bool(value)) if *value == !enabled),
            "{kind}: {:?}",
            item.props.get("disabled")
        );
        find_layout_node_by_stable_key_suffix(&layout, "/edit-menu-reload-effect")
            .expect("reload effect action");
        find_layout_node_by_stable_key_suffix(&layout, "/edit-menu-rescan-library")
            .expect("rescan library action");
    }
    assert!(invoke(&mut editor, "edit-menu-reload-instrument").iter().any(|event| matches!(event,
        HostCommand::Custom { name, payload: Value::Map(map) }
        if name == "reload-instrument-from-disk" && map_usize(map, "track") == Some(0))));
    assert!(invoke(&mut editor, "edit-menu-rescan-library").iter().any(|event| matches!(event,
        HostCommand::Custom { name, .. } if name == "reload-content-library")));
}

#[test]
fn application_menus_share_actions_and_fallback_layout() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor
        .runtime_mut()
        .eval_str(r#"(set-window-buffer "*transport*")"#)
        .unwrap();
    editor.refresh_runtime_side_effects();
    for (menu, entries) in [
        (
            "Create",
            vec![
                ("create-menu-instrument", "enter-new-instrument-editor"),
                ("create-menu-effect", "enter-new-effect-editor"),
                ("create-menu-midi", "add-track-empty"),
                ("create-menu-sampler", "add-track-sampler"),
            ],
        ),
        (
            "Pattern",
            vec![
                ("pattern-menu-double", "menu-pattern-double"),
                ("pattern-menu-half", "menu-pattern-half"),
                ("pattern-menu-clone", "menu-pattern-clone"),
                ("pattern-menu-capture-midi", "retrospective-open"),
            ],
        ),
    ] {
        editor
            .runtime_mut()
            .eval_str(&format!(
                "(eseq.transport/open-application-menu \"{menu}\" (dict :at (dict :col 2 :row 1)))"
            ))
            .unwrap();
        editor.refresh_runtime_side_effects();
        let layout = editor.widget_layout().unwrap();
        for (id, command) in entries {
            let key = format!("/{id}");
            let node = find_layout_node_by_stable_key_suffix(&layout, &key)
                .unwrap_or_else(|| panic!("missing {id}"));
            assert_finite_nonzero_rect(node, id);
            if id == "pattern-menu-double" || id == "pattern-menu-half" {
                assert!(
                    matches!(node.props.get("shortcut"), Some(Value::String(s)) if !s.is_empty())
                );
            }
            editor.drain_host_commands();
            editor
                .runtime_mut()
                .invoke_global(
                    "eseq.transport/run-menu-action",
                    vec![Value::String(menu.into()), Value::String(id.into())],
                )
                .unwrap();
            assert!(editor.drain_host_commands().iter().any(|event| matches!(event,
                HostCommand::Custom { name, payload: Value::String(target) } if name == "native-menu-activate" && target == id)));
            // The catalog still points to the existing application command.
            let callback = editor.runtime_mut().eval_str(&format!(
                "(get (nth (filter (lambda (item) (and item (= (get item :id) \"{id}\"))) (eseq.transport/application-menu-items \"{menu}\")) 0) :on-select)"
            )).unwrap().unwrap();
            editor.runtime_mut().invoke(callback, vec![]).unwrap();
            assert!(editor
                .drain_host_commands()
                .iter()
                .any(|event| matches!(event,
                HostCommand::Custom { name, .. } if name == command)));
        }
    }
    editor
        .runtime_mut()
        .eval_str("(eseq.transport/close-application-menu)")
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor
        .runtime_mut()
        .register_native("native-menu-installed?", |_, _| Ok(Value::Bool(true)));
    editor
        .runtime_mut()
        .invalidate_reactive_source(
            crate::application_menu::SOURCE,
            "installed",
            Value::Bool(true),
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().unwrap();
    for key in [
        "transport-File-menu-button",
        "transport-Create-menu-button",
        "transport-Pattern-menu-button",
    ] {
        assert!(
            find_layout_node_by_stable_key(&layout, key).is_none(),
            "native menus should replace {key}"
        );
    }
}

#[test]
fn lisp_registered_menu_appears_in_toolbar_without_rust_configuration() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor
        .runtime_mut()
        .eval_str(
            r#"
        (set-window-buffer "*transport*")
        (eseq.menus/register-menu
          (dict :id "tools" :label "Tools"
            :items (list
              (dict :id "tools-nested" :label "Actions"
                :items (list (dict :id "tools-action" :label "Action"
                  :on-select (lambda () (host-command "open-help" (dict)))))))))
    "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().unwrap();
    let button = find_layout_node_by_stable_key(&layout, "transport-tools-menu-button")
        .expect("custom menu button");
    assert_finite_nonzero_rect(button, "custom menu button");
    editor
        .runtime_mut()
        .eval_str(
            "(eseq.transport/open-application-menu \"tools\" (dict :at (dict :col 2 :row 1)))",
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().unwrap();
    let item =
        find_layout_node_by_stable_key_suffix(&layout, "/tools-nested").expect("custom submenu");
    assert_finite_nonzero_rect(item, "custom submenu");
}

#[test]
fn view_menu_tracks_panel_visibility_and_confirmation_cancel_is_inert() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor
        .runtime_mut()
        .eval_str(
            r#"
        (set-window-buffer "*transport*")
        (eseq.transport/open-application-menu "View" (dict :at (dict :col 2 :row 1)))
    "#,
        )
        .unwrap();
    for visible in [false, true] {
        editor
            .runtime_mut()
            .eval_str(&format!(
                "(set! eseq.seq-core-state/mixer-panel-visible {visible})"
            ))
            .unwrap();
        editor.refresh_runtime_side_effects();
        let layout = editor.widget_layout().unwrap();
        let node =
            find_layout_node_by_stable_key_suffix(&layout, "/view-mixer").expect("mixer toggle");
        assert_finite_nonzero_rect(node, "mixer toggle");
        assert!(matches!(node.props.get("checked"), Some(Value::Bool(value)) if *value == visible));
    }
    editor
        .runtime_mut()
        .eval_str(
            r#"
        (eseq.transport/close-application-menu)
        (set-window-buffer "*sequencer*")
        (eseq.file-dialogs/open-confirm "Clear pattern?"
          (lambda () (host-command "menu-pattern-transform" (dict :operation "clear"))))
    "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().unwrap();
    let cancel = find_layout_node_by_stable_key_suffix(&layout, "/menu-confirm-cancel")
        .expect("cancel action");
    assert_finite_nonzero_rect(cancel, "cancel action");
    editor.drain_host_commands();
    editor
        .runtime_mut()
        .eval_str("(eseq.file-dialogs/close-confirm)")
        .unwrap();
    assert!(editor.drain_host_commands().is_empty());
}

/// Edit > Edit Selected Effect… opens the effect armed for deletion (its
/// header selected) when the effect editor can (`device.builtin` false), on
/// the current track or a bus; Reload Selected Effect only a track's.
#[test]
fn edit_selected_effect_opens_the_armed_custom_effect() {
    let mut editor = full_grid_editor_for_scroll_tests();
    seed_browser_tracks(&mut editor, &["sampler"], 0);
    seed_kind_buses(&mut editor, &[(0, "Main"), (3, "Verb")]);
    let rt = editor.runtime_mut();
    let track = kind_track(rt, 0);
    let bus = rt.keyed_instance("eseq.kinds:bus", &[1]).unwrap();
    let effect = |rt: &mut Runtime, owner: (&str, eseqlisp::vm::InstanceId), name: &str| {
        let fx = rt
            .register_keyed_instance("eseq.kinds:device", &[owner.1, 9])
            .unwrap();
        let role = if owner.0 == "bus" {
            "bus-effect"
        } else {
            "effect"
        };
        for (field, value) in [
            (owner.0, Value::Instance(owner.1)),
            ("role", Value::String(role.into())),
            ("slot", Value::Number(1.0)),
            ("name", Value::String(name.into())),
        ] {
            set_field(rt, fx, field, value);
        }
        set_field(rt, owner.1, "devices", instance_list([fx]));
        fx
    };
    let track_fx = effect(rt, ("track", track), "Grit");
    let bus_fx = effect(rt, ("bus", bus), "Tape");
    rt.run_reactive_cycle();
    let item = |editor: &mut eseqlisp::Editor, id: &str, prop: &str| {
        let callback = (editor.runtime_mut())
            .eval_str(&crate::application_menu::menu_item_prop("Edit", id, prop))
            .unwrap()
            .unwrap();
        editor.drain_host_commands();
        let value = editor.runtime_mut().invoke(callback, vec![]).unwrap();
        (value, editor.drain_host_commands())
    };
    assert_eq!(
        item(&mut editor, "edit-menu-effect", "enabled-when").0,
        Some(Value::Bool(false))
    );
    for (fx, builtin, enabled) in [(track_fx, true, false), (track_fx, false, true)] {
        let rt = editor.runtime_mut();
        set_field(rt, fx, "builtin", Value::Bool(builtin));
        set_field(rt, fx, "delete-target", Value::Bool(true));
        rt.run_reactive_cycle();
        assert_eq!(
            item(&mut editor, "edit-menu-effect", "enabled-when").0,
            Some(Value::Bool(enabled)),
            "builtin {builtin}"
        );
    }
    let opened = |commands: &[HostCommand]| {
        commands.iter().find_map(|command| match command {
            HostCommand::Custom {
                name,
                payload: Value::Map(map),
            } if name == "enter-edit-effect" => Some((
                map_string(map, "name"),
                map_usize(map, "slot"),
                map_usize(map, "bus"),
            )),
            _ => None,
        })
    };
    let (_, commands) = item(&mut editor, "edit-menu-effect", "on-select");
    assert_eq!(
        opened(&commands),
        Some((Some("Grit".to_string()), Some(1), None))
    );
    let (_, commands) = item(&mut editor, "edit-menu-reload-effect", "on-select");
    assert!(commands.iter().any(|command| matches!(command,
        HostCommand::Custom { name, payload: Value::Map(map) }
        if name == "reload-effect-from-disk" && map_usize(map, "slot") == Some(1))));
    // A bus's effect: opened with its bus, never reloaded.
    let rt = editor.runtime_mut();
    set_field(rt, track_fx, "delete-target", Value::Bool(false));
    set_field(rt, bus_fx, "delete-target", Value::Bool(true));
    rt.run_reactive_cycle();
    let (_, commands) = item(&mut editor, "edit-menu-effect", "on-select");
    assert_eq!(
        opened(&commands),
        Some((Some("Tape".to_string()), Some(1), Some(1)))
    );
    assert_eq!(
        item(&mut editor, "edit-menu-reload-effect", "enabled-when").0,
        Some(Value::Bool(false))
    );
}
