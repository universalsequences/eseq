//! Boot an exported bundle's panel headlessly and render it to a PNG, the way
//! the plugin's editor draws it: `panel_capture "<X.clap>" out.png [name=value…]`
//! (values are applied through the param store, as host automation would).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use eseq_plugin::engine::ParamStore;
use eseq_plugin::panel::PanelHost;
use eseq_plugin::payload::Payload;

fn main() {
    let mut args = std::env::args().skip(1);
    let bundle = PathBuf::from(args.next().expect("bundle"));
    let out = PathBuf::from(args.next().unwrap_or_else(|| "/tmp/panel.png".into()));
    let payload = Payload::load(&Payload::dir_for_bundle(&bundle)).expect("payload");
    let params = Arc::new(ParamStore::new(payload.spec.params.iter().map(|p| p.default)));
    let started = Instant::now();
    let mod_display = Arc::new(eseq_plugin::engine::ModDisplay::default());
    let mut panel = PanelHost::boot(&payload, params.clone(), mod_display, 2.0).unwrap_or_else(|e| {
        eprintln!("boot failed: {e}");
        std::process::exit(1);
    });
    println!("boot {:?}, size {:?} pt", started.elapsed(), panel.size());
    let mut drag: Option<(f64, f64, f64)> = None;
    let rest: Vec<String> = args.collect();
    for assignment in rest {
        if let Some(spec) = assignment.strip_prefix("drag=") {
            // drag=x,y,dy : press at (x, y) points, move up by dy, release.
            let v: Vec<f64> = spec.split(',').map(|n| n.parse().expect("number")).collect();
            drag = Some((v[0], v[1], v[2]));
            continue;
        }
        let (name, value) = assignment.split_once('=').expect("name=value");
        let index = payload.spec.params.iter().position(|p| p.name == name).expect("param");
        params.set(index, value.parse().expect("number"));
    }
    let knobs = panel.widget_rects("knob-number");
    for (key, rect) in knobs.iter().take(4) {
        println!("knob {key} at {rect:?}");
    }
    if drag.is_none() && std::env::var_os("DRAG_FIRST_KNOB").is_some() {
        if let Some((_, (x, y, w, h))) = knobs.first() {
            drag = Some((x + w / 2.0, y + h / 2.0, 40.0));
        }
    }
    if std::env::var_os("INVOKE_FIRST_KNOB").is_some() {
        let r = panel.invoke_first("knob-number", "on-change", vec![eseqlisp::vm::Value::Number(1.5)]);
        panel.frame_headless();
        println!("invoke: {r:?} edits {:?}", panel.take_edits());
    }
    if let Some((x, y, dy)) = drag {
        println!("dragging at ({x:.1}, {y:.1})");
        use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
        let before: Vec<f32> = (0..params.len()).map(|i| params.get(i)).collect();
        panel.render_png(&out).expect("render");
        panel.mouse(MouseEventKind::Down(MouseButton::Left), x, y, KeyModifiers::NONE);
        println!("hidden drag active after down: {}", panel.editor_mut().hidden_drag_gesture_active());
        for step in 1..=10 {
            panel.mouse(MouseEventKind::Drag(MouseButton::Left), x, y - dy * step as f64 / 10.0, KeyModifiers::NONE);
            panel.frame_headless();
        }
        panel.mouse(MouseEventKind::Up(MouseButton::Left), x, y - dy, KeyModifiers::NONE);
        panel.frame_headless();
        for (i, p) in payload.spec.params.iter().enumerate() {
            if params.get(i) != before[i] {
                println!("changed {}: {} -> {}", p.name, before[i], params.get(i));
            }
        }
        println!("edits: {:?}", panel.take_edits());
        println!("status: {:?}", panel.editor_mut().runtime_mut().take_status_message());
        println!("hidden drag active after up: {}", panel.editor_mut().hidden_drag_gesture_active());
    }
    panel.render_png(&out).expect("render");
    if std::env::var_os("MODS_OPEN").is_some() {
        let r = panel.editor_mut().runtime_mut().eval_str("(eseq.effects.effect-panels/instrument-toggle-mods-view)");
        println!("toggle: {r:?}");
        panel.editor_mut().refresh_runtime_side_effects();
        let mods = out.with_file_name(format!("{}-mods.png", out.file_stem().unwrap().to_string_lossy()));
        panel.render_png(&mods).expect("render mods");
        let r = panel.invoke_keyed("knob-number", "-z3", "on-change", vec![eseqlisp::vm::Value::Number(2.5)]);
        println!("mods knob invoke: {r:?}");
        panel.editor_mut().refresh_runtime_side_effects();
        panel.frame_headless();
        for edit in panel.take_edits() {
            if let eseq_plugin::panel::PanelEdit::Value(index, value) = edit {
                println!("mods knob edit ({r:?}): {} = {value}", payload.spec.params[index].name);
            }
        }
        println!("{}", mods.display());
    }
    println!("{}", out.display());
}
