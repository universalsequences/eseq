use super::*;

#[test]
fn about_sample_credits_have_visible_geometry_and_can_close() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let id = editor.buffers.iter().find(|buffer| buffer.name == "*sequencer*").unwrap().id;
    editor.set_active_buffer(id);
    editor.set_layout_viewport(160, 60);
    editor.runtime_mut().eval_str(r#"(eseq.file-dialogs/open-about "test")"#).unwrap();
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().unwrap();
    let credits = find_layout_node_by_stable_key_suffix(&layout, "/about-sample-credits")
        .expect("sample attribution container");
    let viewport = find_layout_node_by_stable_key_suffix(&layout, "/about-credit-scroll").unwrap();
    assert_finite_nonzero_rect(viewport, "sample credits viewport");
    assert_finite_nonzero_rect(credits, "sample credits");
    assert!(!credits.children.is_empty());
    for child in &credits.children {
        assert_finite_nonzero_rect(child, "sample attribution entry");
        assert!(child.rect.row >= viewport.rect.row);
        assert!(child.rect.row + child.rect.height <= viewport.rect.row + viewport.rect.height + 0.01);
        assert!(child.rect.col + child.rect.width <= viewport.rect.col + viewport.rect.width + 0.01);
    }
    let close = find_layout_node_by_stable_key_suffix(&layout, "/about-ok").unwrap();
    assert_finite_nonzero_rect(close, "close about");
    editor.runtime_mut().eval_str("(eseq.file-dialogs/close-about)").unwrap();
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    assert!(find_layout_node_by_stable_key_suffix(&editor.widget_layout().unwrap(), "/about-sample-credits").is_none());
}
