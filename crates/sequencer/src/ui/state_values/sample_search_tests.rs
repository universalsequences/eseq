use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn install_browser(editor: &mut Editor, db: sequencer::sample_db::SampleDb) -> Rc<RefCell<DebouncedSampleBrowser>> {
    let browser = Rc::new(RefCell::new(DebouncedSampleBrowser::new(db, Duration::ZERO)));
    register_sample_browser_native(editor.runtime_mut(), browser.clone());
    browser
}

fn settle_browser(editor: &mut Editor, browser: &RefCell<DebouncedSampleBrowser>) -> Duration {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ui_time = Duration::ZERO;
    while browser.borrow().is_pending() {
        let start = Instant::now();
        publish_sample_browser_results(editor, browser).unwrap();
        ui_time += start.elapsed();
        assert!(Instant::now() < deadline, "search timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
    ui_time
}

fn focus_search(editor: &mut Editor) {
    editor.runtime_mut().eval_str(r#"(set! sbrowser-tab "samples")"#).unwrap();
    editor.refresh_runtime_side_effects();
    editor.set_active_buffer(browser_id(editor));
    editor.set_layout_viewport(72, 60);
    let layout = editor.widget_layout().unwrap();
    let input = find_layout_node_by_stable_key_suffix(&layout, "/search-input").unwrap();
    assert_finite_nonzero_rect(input, "sample search input");
    assert!(editor.focus_widget_by_stable_key(input.stable_key.as_ref().unwrap(), Some("text-input")));
}

#[test]
fn sample_search_result_arrival_keeps_focus_and_accepts_more_typing() {
    let mut db = sequencer::sample_db::SampleDb::open_in_memory().unwrap();
    for i in 0..2000 {
        db.insert_sample_with_tags(&format!("hash{i}"), Some(&format!("Kick {i}")), &[]).unwrap();
    }
    let mut editor = browser_editor_on_instrument_tab();
    let browser = install_browser(&mut editor, db);
    editor.runtime_mut().eval_str(r#"
        (effect-buffer "*embedded-sample-test*"
          (eseq.browser/sample-browser-widget true "" ""))
    "#).unwrap();
    focus_search(&mut editor);
    settle_browser(&mut editor, &browser);
    let focus = editor.focused_widget_id();
    editor.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    settle_browser(&mut editor, &browser);
    let layout = editor.widget_layout().unwrap();
    let tree = find_layout_node_by_stable_key_suffix(&layout, "/samples-tab-tree").unwrap();
    assert_finite_nonzero_rect(tree, "sample results");
    let Value::List(items) = &tree.props["items"] else { panic!("result items"); };
    assert_eq!(items.len(), 2000);
    assert_eq!(editor.focused_widget_id(), focus);
    editor.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(editor.runtime_mut().eval_str("eseq.browser/search-filter").unwrap(),
        Some(Value::String("ki".to_string())));
    settle_browser(&mut editor, &browser);
    assert_eq!(editor.focused_widget_id(), focus);
    let embedded_id = editor.buffers.iter().find(|b| b.name == "*embedded-sample-test*").unwrap().id;
    editor.set_active_buffer(embedded_id);
    let layout = editor.widget_layout().unwrap();
    let tree = find_layout_node_by_stable_key_suffix(&layout, "/learn-target-samples-tree")
        .unwrap_or_else(|| panic!("embedded picker layout: {layout:?}"));
    let Value::List(items) = &tree.props["items"] else { panic!("embedded result items"); };
    assert_eq!(items.len(), 2000, "query completion must refresh the embedded picker too");
}

/// Compare the old synchronous query and the worker using the same library,
/// UI, and binary. This measures CPU work through primitive collection, not
/// display/input-device latency. No audio engine or interactive window.
#[test]
#[ignore = "local sample library timing probe; run in release with --no-capture"]
fn sample_search_local_library_latency() {
    let path = sequencer::app_paths::app_paths().sample_db_path();
    for synchronous in [true, false] {
        let mut editor = browser_editor_on_instrument_tab();
        let db = sequencer::sample_db::SampleDb::open_read_only(&path).unwrap();
        let browser = if synchronous {
            // Preserve the previous browser's exact-request cache. The only
            // difference being measured is executing a matured query inline.
            let cached: RefCell<Option<(String, Value)>> = RefCell::new(None);
            editor.runtime_mut().register_native("seq-sample-browser", move |args, _ctx| {
                let query = match args.first() { Some(Value::String(s)) => s.as_str(), _ => "" };
                if let Some((key, value)) = &*cached.borrow() {
                    if key == query { return Ok(value.deep_clone()); }
                }
                let value = build_sample_browser_value_from_db(&db, query, &[]).map_err(|e| e.to_string())?;
                *cached.borrow_mut() = Some((query.to_string(), value.deep_clone()));
                Ok(value)
            });
            None
        } else { Some(install_browser(&mut editor, db)) };
        focus_search(&mut editor);
        if let Some(browser) = &browser { settle_browser(&mut editor, browser); }
        for ch in "kick".chars() {
            let start = Instant::now();
            editor.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            let input = start.elapsed();
            let publish = browser.as_ref().map(|b| settle_browser(&mut editor, b)).unwrap_or_default();
            let start = Instant::now();
            editor.refresh_runtime_side_effects();
            let layout = editor.widget_layout().unwrap();
            let layout_time = start.elapsed();
            let viewport = eseqlisp::widget_render::WidgetViewport {
                cell_w: 10.0, cell_h: 20.0, vp_w: 720.0, vp_h: 1200.0,
                time_seconds: 0.0, focused_widget_id: editor.focused_widget_id(),
                focused_branch: true, overlay_viewport_bottom: 60.0,
                scroll_top: 0.0, scroll_left: 0.0, inherited_hover: false,
            };
            let start = Instant::now();
            let (primitives, _) = eseqlisp::widget_render::collect_gpu_primitives(&layout, viewport, 0.0, 60);
            eprintln!("sample-search sync={synchronous} char={ch} input={input:?} publish={publish:?} layout={layout_time:?} paint={:?} primitives={}", start.elapsed(), primitives.len());
        }
    }
}
