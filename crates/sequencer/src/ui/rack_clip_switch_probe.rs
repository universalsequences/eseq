//! Saved-project rack clip click -> completed Metal frame benchmark.
//! Uses production pointer dispatch, host commands and reactive ticks. The
//! headless graph pump services DSP commands, but there is no scheduler/audio
//! device, OS event queue or display scanout in this measurement.

use crate::*;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use sha2::{Digest, Sha256};
use super::{find_layout_node_by_stable_key_suffix, layout_prop_number};

fn milliseconds(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn summary(values: impl Iterator<Item = f64>) -> serde_json::Value {
    let mut values: Vec<_> = values.collect();
    assert!(!values.is_empty());
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    let median = if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) * 0.5
    } else { values[middle] };
    serde_json::json!({
        "min": values[0], "median": median,
        "p95": values[(values.len() as f64 * 0.95).ceil() as usize - 1],
        "max": values[values.len() - 1],
    })
}

fn sequencer_tile(frame: &eseqlisp::backend::TiledRenderFrame) -> &eseqlisp::backend::TileFrame {
    frame.tiles.iter().find(|tile| tile.frame.buffer_name == "*sequencer*")
        .expect("visible sequencer tile")
}

fn clip_center(frame: &eseqlisp::backend::TiledRenderFrame, group: u64, clip: u64, cell_w: f32, cell_h: f32) -> (f32, f32) {
    let tile = sequencer_tile(frame);
    let layout = tile.frame.widget_layout.as_ref().expect("sequencer layout");
    let cell = find_layout_node_by_stable_key_suffix(layout, &format!("/rack-clip-{group}-{clip}"))
        .expect("rack clip cell");
    assert!(cell.rect.width.is_finite() && cell.rect.width > 0.0);
    assert!(cell.rect.height.is_finite() && cell.rect.height > 0.0);
    // Preserve the fractional tile/tab origin and the Metal pixel border.
    // Flooring the origin can miss these sub-cell-height clip squares.
    let border = if tile.show_border {
        eseqlisp::widget_render::ui_design_px(tile.border_width_px.max(0.0))
    } else { 0.0 };
    let x = tile.body_rect.col + border / cell_w + cell.rect.col + cell.rect.width * 0.5
        - tile.frame.widget_layout_scroll_left;
    let y = tile.body_rect.row + border / cell_h + cell.rect.row + cell.rect.height * 0.5
        - tile.frame.widget_scroll_top - tile.frame.text_scroll_top as f32;
    assert!(x.is_finite() && y.is_finite()
        && x >= tile.body_rect.col && x < tile.body_rect.col + tile.body_rect.width
        && y >= tile.body_rect.row && y < tile.body_rect.row + tile.body_rect.height,
        "clip {clip} must be visible, got ({x}, {y})");
    (x, y)
}

pub(super) fn run(editor: &mut Editor, app: &mut app::App, shared: &SharedHandles, snapshot: &std::path::Path) {
    let out = std::path::PathBuf::from(std::env::var("ESEQ_RACK_CLIP_OUT")
        .expect("set ESEQ_RACK_CLIP_OUT to an absolute report JSON path"));
    assert!(out.is_absolute());
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let project_bytes = std::fs::read(snapshot).unwrap();
    let project_sha256 = format!("{:x}", Sha256::digest(&project_bytes));
    std::fs::write(out.with_extension("project.json"), &project_bytes).unwrap();
    let playing = match std::env::var("ESEQ_RACK_CLIP_PLAYING").as_deref() {
        Ok("1") => true,
        Ok("0") | Err(_) => false,
        _ => panic!("ESEQ_RACK_CLIP_PLAYING must be 0 or 1"),
    };
    let requested_group = std::env::var("ESEQ_RACK_CLIP_GROUP").ok()
        .map(|value| value.parse::<u64>().expect("rack group ID"));
    let racks: Vec<_> = app.groups.iter().filter(|group|
        group.rack.is_some() && requested_group.is_none_or(|id| id == group.id)).collect();
    assert_eq!(racks.len(), 1, "choose one rack with ESEQ_RACK_CLIP_GROUP");
    let rack = racks[0];
    let group_id = rack.id;
    let group_name = rack.name.clone();
    let members = rack.members.clone();
    let collapsed = rack.collapsed;
    let expected_graphs = rack.rack.as_ref().unwrap().sequencers.len();
    let clips: Vec<u64> = app.rack_clip_bank(group_id).iter().map(|clip| clip.0).collect();
    assert!(clips.len() >= 2, "need distinct clip targets");
    let samples: usize = std::env::var("ESEQ_RACK_CLIP_SAMPLES").ok()
        .map(|value| value.parse().expect("sample count")).unwrap_or(clips.len());
    assert!(samples >= 2);
    let scene = shared.state.current_scene_index();
    // The common performance fixture parks on track zero. This probe retains
    // the project loader's saved cursor, as reopening the actual project does.
    let selected_track = app.ui.cursor_track;
    shared.current_track.store(selected_track, Ordering::Relaxed);

    // Match project-load ordering: publish topology before evaluating rack
    // scripts, whose tab labels and owner bindings read SEQ.groups.
    *shared.bus_state.lock().unwrap() = app.buses.clone();
    *shared.track_groups.lock().unwrap() = app.groups.clone();
    sync_groups_bindings(editor.runtime_mut(), &app.groups);
    let paths = sequencer::app_paths::app_paths();
    let (roots, errors) = paths.module_load_roots();
    assert!(errors.is_empty(), "{errors:?}");
    editor.runtime_mut().set_load_root(paths.factory_root());
    editor.runtime_mut().set_scoped_module_load_path(roots);
    evaluate_project_scratch_on_ui_runtime(editor, app).expect("restore project/rack scripts");
    assert!(app.state.published_sequencers().len() >= expected_graphs);
    if !collapsed {
        editor.runtime_mut().eval_str(&format!("(seq-toggle-group-collapsed {group_id})")).unwrap();
    }
    // No musical boundary wait in an input-to-frame benchmark.
    editor.runtime_mut().set_reactive("SEQ", "scene-launch-quantize", Value::String("off".into()));
    if playing { shared.state.start_playback(); } else { shared.state.stop_playback(); }
    editor.runtime_mut().set_reactive("SEQ", "playing", Value::Bool(playing));

    let dimension = |name: &str, default: u32| std::env::var(name).ok()
        .map(|value| value.parse::<u32>().expect("pixel dimension")).unwrap_or(default);
    let (width, height) = (dimension("ESEQ_RACK_CLIP_WIDTH", 2500), dimension("ESEQ_RACK_CLIP_HEIGHT", 1700));
    assert!(width > 0 && height > 0);
    let scale: f64 = std::env::var("ESEQ_RACK_CLIP_SCALE").ok()
        .map(|value| value.parse().expect("display scale")).unwrap_or(2.0);
    assert!(scale.is_finite() && scale > 0.0);
    let mut backend = create_offscreen_capture_backend(editor, width, height, scale).unwrap();
    let (cell_w, cell_h) = backend.cell_dimensions();
    let (cols, rows) = ((width as f32 / cell_w) as usize, (height as f32 / cell_h) as usize);
    let target = backend.create_tiled_capture_target(width, height)
        .unwrap_or_else(|_| panic!("create rack clip capture target"));
    editor.set_layout_viewport(cols as u16, rows as u16);
    editor.update_tile_rects(cols as u16, rows as u16);
    let mut sessions = EditSessionState::default();
    let mut frame = FrameDiffState::default();
    let mut gesture = GestureState::default();
    let mut track_names = app.tracks.clone();
    let mut stats = UiLoopStats::new();
    let mut meters = MeterCache {
        cached_peak_l_level: 0.0, cached_peak_r_level: 0.0,
        cached_track_peak_levels: vec![0.0; app.tracks.len()],
        cached_rack_slot_peak_levels: Vec::new(),
        cached_bus_peak_levels: vec![0.0; app.buses.len()],
        cached_modulator_phases: Vec::new(), cached_modulator_levels: Vec::new(),
        cached_mod_port_levels: Default::default(), cached_mod_display_values: Default::default(),
        watched_display_modulators: Default::default(),
        mod_display_poll_fx_epoch: usize::MAX, mod_display_poll_track: None,
        cached_cpu_load_bits: 0,
        last_meter_poll_at: Instant::now(), last_cpu_ui_poll_at: Instant::now(),
        last_neural_visualization_poll_at: Instant::now(),
        visualization_liveness: VisualizationLiveness::default(),
        last_voice_count_log_at: Instant::now(),
    };
    let mut ctx = LoopCtx {
        sessions: &mut sessions, meters: &mut meters, frame: &mut frame,
        gesture: &mut gesture, track_names: &mut track_names, shared,
    };
    let tick = TickInputs { cols, rows, playing_now: playing };
    for _ in 0..3 {
        editor.sync_reactive_bindings_for_visible_layouts();
        sync_reactive_tick(app, editor, &mut ctx, &tick, &mut stats);
        let tiled = eseqlisp::frame::build_tiled_render_frame_borderless(editor, cols, rows);
        backend.render_tiled_capture(&tiled, &target).unwrap_or_else(|_| panic!("warm initial frame"));
        editor.clear_needs_redraw();
    }
    assert!(app.groups.iter().find(|group| group.id == group_id).unwrap().collapsed);
    assert!(editor.drain_host_commands().is_empty(), "setup left pending host commands");
    target.save_png(&out.with_extension("before.png")).unwrap();
    let initial_active = app.rack_clip_bank(group_id).iter().find(|clip| clip.2).map(|clip| clip.0);
    let start_index = initial_active.and_then(|id| clips.iter().position(|clip| *clip == id))
        .map(|index| (index + 1) % clips.len()).unwrap_or(0);
    let mut reports = Vec::new();
    // Retain first-click and two warmup timings, but exclude all three from
    // the steady-state summary. Every click is a different clip from the last.
    for iteration in 0..samples + 3 {
        let clip_index = (start_index + iteration) % clips.len();
        let clip = clips[clip_index];
        let previous = app.rack_clip_bank(group_id).iter().find(|clip| clip.2).map(|clip| clip.0);
        assert_ne!(previous, Some(clip));
        let (x, y) = {
            let tiled = eseqlisp::frame::build_tiled_render_frame_borderless(editor, cols, rows);
            clip_center(&tiled, group_id, clip, cell_w, cell_h)
        };
        // These are separate human clicks, not an artificial rapid double-click.
        editor.active_leaf_mut().last_widget_click = None;
        let counters_before = editor.runtime().ui_work_counters();
        let started = Instant::now();
        let mut pointer_ms = 0.0;
        let mut host_ms = 0.0;
        let mut launches = 0;
        for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
            let phase = Instant::now();
            editor.handle_tiled_mouse_precise(MouseEvent {
                kind, column: x.floor() as u16, row: y.floor() as u16,
                modifiers: KeyModifiers::NONE,
            }, x, y, 0);
            let commands = editor.drain_host_commands();
            pointer_ms += milliseconds(phase);
            let phase = Instant::now();
            for command in commands {
                let eseqlisp::HostCommand::Custom { name, payload } = command else {
                    panic!("unexpected command from rack clip square");
                };
                assert_eq!(name, "launch-rack-clip");
                assert_eq!(extract_usize_from_payload(&payload, "group-id"), Some(group_id as usize));
                assert_eq!(extract_usize_from_payload(&payload, "clip-id"), Some(clip as usize));
                assert_eq!(extract_string_from_payload(&payload, "quantize").as_deref(), Some("off"));
                dispatch_custom_host_command(&name, payload, app, editor, &mut ctx);
                launches += 1;
            }
            host_ms += milliseconds(phase);
        }
        let phase = Instant::now();
        editor.sync_reactive_bindings_for_visible_layouts();
        sync_reactive_tick(app, editor, &mut ctx, &tick, &mut stats);
        let reactive_tick_ms = milliseconds(phase);
        let phase = Instant::now();
        let tiled = eseqlisp::frame::build_tiled_render_frame_borderless(editor, cols, rows);
        let frame_build_ms = milliseconds(phase);
        let phase = Instant::now();
        let render = backend.render_tiled_capture(&tiled, &target)
            .unwrap_or_else(|_| panic!("render launched rack clip"));
        let render_wall_ms = milliseconds(phase);
        let total_ms = milliseconds(started);
        editor.clear_needs_redraw();

        // Validation and PNG/JSON IO stay outside the measured interval.
        assert_eq!(launches, 1, "exactly one launch per click");
        assert_eq!(shared.state.current_scene_index(), scene, "rack clip launch stays in this scene");
        assert_eq!(app.rack_clip_bank(group_id).iter().find(|clip| clip.2).map(|clip| clip.0), Some(clip));
        app.state.with_project_scenes(|scenes| {
            assert_eq!(scenes.live_rack_clips.iter().find(|(id, _)| *id == group_id),
                Some(&(group_id, Some(clip))), "launch must install the clip's live lanes");
        });
        let layout = sequencer_tile(&tiled).frame.widget_layout.as_ref().unwrap();
        for id in &clips {
            let cell = find_layout_node_by_stable_key_suffix(layout, &format!("/rack-clip-{group_id}-{id}")).unwrap();
            assert_eq!(layout_prop_number(cell, "active"), Some(if *id == clip { 1.0 } else { 0.0 }));
        }
        let picker = find_layout_node_by_stable_key_suffix(layout, &format!("/rack-clip-number-{group_id}")).unwrap();
        assert_eq!(layout_prop_number(picker, "value"), Some((clip_index + 1) as f64));
        for member in &members {
            assert!(find_layout_node_by_stable_key_suffix(layout, &format!("/step-cell-{member}-0")).is_none(),
                "collapsed rack must not mount member step grids");
        }
        let counters = editor.runtime().ui_work_counters();
        let phase = if iteration == 0 { "first-click" } else if iteration < 3 { "warmup" } else { "sample" };
        eprintln!("rack clip {phase} {iteration}: {previous:?} -> {clip}: {total_ms:.2} ms (pointer {pointer_ms:.2}, host {host_ms:.2}, tick {reactive_tick_ms:.2}, frame {frame_build_ms:.2}, Metal {render_wall_ms:.2})");
        reports.push(serde_json::json!({
            "phase": phase, "iteration": iteration, "from_clip": previous, "to_clip": clip,
            "total_ms": total_ms, "pointer_ms": pointer_ms, "host_ms": host_ms,
            "reactive_tick_ms": reactive_tick_ms, "frame_build_ms": frame_build_ms,
            "render_wall_ms": render_wall_ms, "render": render,
            "full_buffer_reruns": counters.full_buffer_reruns - counters_before.full_buffer_reruns,
            "subtree_reruns": counters.subtree_reruns - counters_before.subtree_reruns,
            "relayout_full": counters.relayout_full - counters_before.relayout_full,
            "relayout_subtree": counters.relayout_subtree - counters_before.relayout_subtree,
        }));
        if iteration == 0 { target.save_png(&out.with_extension("png")).unwrap(); }
    }
    shared.state.stop_playback();
    let summaries: serde_json::Map<_, _> = ["total_ms", "pointer_ms", "host_ms", "reactive_tick_ms", "frame_build_ms", "render_wall_ms"]
        .into_iter().map(|key| (key.to_string(), summary(reports.iter().skip(3).map(|sample| sample[key].as_f64().unwrap())))).collect();
    let report = serde_json::json!({
        "scope": "rack clip pointer down/up -> production host dispatch -> reactive tick -> full tiled frame -> Metal GPU completion; excludes OS delivery, display scanout and audible scheduler latency",
        "project": std::env::var("ESEQ_RACK_CLIP_PROJECT").unwrap(), "project_sha256": project_sha256,
        "project_bytes": project_bytes.len(), "tracks": app.tracks.len(), "scenes": app.state.scene_count(),
        "scene_index": scene, "selected_track": selected_track, "group_id": group_id, "group_name": group_name,
        "rack_members": members, "clip_ids": clips, "rack_collapsed": true,
        "playing_ui_state": playing, "scheduler_running": false, "launch_quantize": "off",
        "width": width, "height": height, "scale_factor": scale,
        "samples_per_summary": samples, "summary": summaries, "samples": reports,
    });
    std::fs::write(&out, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    eprintln!("Rack clip benchmark: {}\n{}", out.display(), report["summary"]);
}
