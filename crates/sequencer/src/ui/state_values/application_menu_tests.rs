use super::*;

#[test]
fn edit_instrument_menu_tracks_selection_and_opens_existing_editor() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor
        .runtime_mut()
        .eval_str(
            r#"
        (set-window-buffer "*transport*")
        (eseq.transport/open-application-menu "Edit" (dict :col 2 :row 1))
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
        editor
            .runtime_mut()
            .set_reactive("SEQ", "current-track", Value::Number(0.0));
        editor
            .runtime_mut()
            .set_reactive("SEQ", "num-tracks", Value::Number(1.0));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "track-instrument-types",
            build_string_list(&[kind.to_string()]),
        );
        editor.runtime_mut().set_reactive(
            "SEQ",
            "sidebar-instrument-name",
            Value::String(instrument.to_string()),
        );
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let layout = editor.widget_layout().unwrap();
        let item = find_layout_node_by_stable_key_suffix(&layout, "/edit-menu-instrument")
            .expect("edit instrument action");
        assert_finite_nonzero_rect(item, "edit instrument action");
        assert!(
            matches!(item.props.get("disabled"), Some(Value::Bool(value)) if *value == !enabled)
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
            ],
        ),
    ] {
        editor
            .runtime_mut()
            .eval_str(&format!(
                "(eseq.transport/open-application-menu \"{menu}\" (dict :col 2 :row 1))"
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
        .eval_str("(eseq.transport/open-application-menu \"tools\" (dict :col 2 :row 1))")
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
        (eseq.transport/open-application-menu "View" (dict :col 2 :row 1))
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
